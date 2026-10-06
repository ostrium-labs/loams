//! Server streams (design §44 §7.4, D610; runtime contract R7).
//!
//! The API has **server streams only** (D420): no client streaming, no bidi,
//! because a browser cannot stream full duplex over `fetch` and half-duplex
//! works through every proxy. In Rust a server stream is a
//! [`futures::Stream`] (design §44 §7.1), so a caller writes
//!
//! ```rust,ignore
//! let mut transitions = loams.live().watch(request).await?;
//! while let Some(transition) = transitions.next().await {
//!     let transition = transition?;
//!     // …
//! }
//! ```
//!
//! A stream is the one call where "retry it" is not enough. The server hands out
//! cursors; a reconnect has to resume from the last one the client applied, or
//! the client silently misses everything that changed in between — worse than an
//! error, because a sync UI that is quietly stale looks like one that works. So
//! [`watch`] tracks the cursor of every message, re-opens from it on a retryable
//! failure, and does not re-yield what it already yielded.
//!
//! A failure the retry class does not cover — notably an `unimplemented` stream,
//! which every `LiveService` RPC answers in every variant today — is **reported**
//! rather than spun on.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use connectrpc::ConnectError;
use futures::{Stream, StreamExt, stream};

use crate::error::LoamsError;
use crate::retry::{DEFAULT_MAX_RETRIES, backoff, should_retry};

/// One open attempt of a stream, as [`watch`] sees it: the messages, and the
/// terminal failure if the stream ends badly.
pub type Opened<M> = Pin<Box<dyn Stream<Item = Result<M, ConnectError>> + Send>>;

/// A server stream the SDK hands back: the messages, and a [`LoamsError`] for
/// each one that failed (R7, D641).
///
/// Named by the **generated** facade — `facade.rs` is emitted data and traits
/// only, and the traits it emits return `Result<ResponseStream<T>, LoamsError>`
/// — so the name lives here, beside the runtime, rather than in the generator.
/// The type is the one [`watch`] already returns, spelled out: the API has
/// server streams only (D420), so a stream is always this shape.
///
/// Boxed rather than an opaque `impl Stream`, for the reason given on
/// [`watch`]: a stream built from an `async` block is `!Unpin`, so an unboxed
/// one would force every caller into `Box::pin` and `pin!`.
pub type ResponseStream<M> = Pin<Box<dyn Stream<Item = Result<M, LoamsError>> + Send>>;

/// Reads the cursor off a message, when it carries one (R7).
pub type CursorOf<M> = Arc<dyn Fn(&M) -> Option<String> + Send + Sync>;

/// Builds the request a re-open carries, given the last cursor seen.
pub type ResumeWith<Req> = Arc<dyn Fn(Option<&str>, &Req) -> Req + Send + Sync>;

/// Told each message's cursor, so a caller can persist it.
pub type CursorObserver = Arc<dyn Fn(Option<&str>) + Send + Sync>;

/// How a stream re-opens from a cursor.
#[derive(Clone)]
pub struct WatchOptions<Req, M> {
    /// The request to re-open with, given the last cursor seen. Returning the
    /// original request reconnects from the beginning, which is correct — and
    /// loses nothing but time — for a stream whose snapshot is complete.
    pub resume: ResumeWith<Req>,
    /// Reads the cursor off a **message**. There is no default, because a cursor
    /// is the stream's own concept: `loams.live`'s `Transition` carries a
    /// `StateVersion` rather than a string, and the caller turns that into the
    /// string the re-open wants.
    pub cursor: CursorOf<M>,
    /// Called after each message, with the cursor it carried.
    pub on_cursor: Option<CursorObserver>,
    /// Retries after the first failure. Defaults to [`DEFAULT_MAX_RETRIES`].
    pub max_retries: Option<u32>,
    /// Whether the call may be retried at all. Defaults to `true`.
    pub retry_safe: Option<bool>,
    /// Full jitter, injectable so the tests do not actually wait.
    pub random: fn() -> f64,
}

impl<Req, M> std::fmt::Debug for WatchOptions<Req, M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WatchOptions")
            .field("max_retries", &self.max_retries)
            .field("retry_safe", &self.retry_safe)
            .finish_non_exhaustive()
    }
}

impl<Req, M> WatchOptions<Req, M> {
    /// Options that re-open with `resume` and read the cursor with `cursor`.
    #[must_use]
    pub fn new(resume: ResumeWith<Req>, cursor: CursorOf<M>) -> Self {
        WatchOptions {
            resume,
            cursor,
            on_cursor: None,
            max_retries: None,
            retry_safe: None,
            random: crate::call::RetryPlan::jitter(),
        }
    }

    /// Observes each message's cursor.
    #[must_use]
    pub fn on_cursor(mut self, on_cursor: CursorObserver) -> Self {
        self.on_cursor = Some(on_cursor);
        self
    }

    /// Sets the retry budget for one run of disconnects.
    #[must_use]
    pub fn max_retries(mut self, retries: u32) -> Self {
        self.max_retries = Some(retries);
        self
    }

    /// Overrides whether the call may be retried.
    #[must_use]
    pub fn retry_safe(mut self, safe: bool) -> Self {
        self.retry_safe = Some(safe);
        self
    }

    /// Replaces the jitter source. [`no_jitter`] makes a backoff zero, which is
    /// what the tests use so a retry costs no wall-clock.
    #[must_use]
    pub fn random(mut self, random: fn() -> f64) -> Self {
        self.random = random;
        self
    }
}

/// A zero backoff, for a caller that wants no wait between a stream's retries.
pub const fn no_jitter() -> f64 {
    0.0
}

/// Wraps a cursor reader.
pub fn cursor_of<M, F>(read: F) -> CursorOf<M>
where
    F: Fn(&M) -> Option<String> + Send + Sync + 'static,
{
    Arc::new(read)
}

/// Wraps a resume builder.
pub fn resume_from_cursor<Req, F>(build: F) -> ResumeWith<Req>
where
    F: Fn(Option<&str>, &Req) -> Req + Send + Sync + 'static,
{
    Arc::new(build)
}

/// A server stream that reconnects from its cursor.
///
/// It yields until the stream ends; if the stream fails with a code a retry may
/// answer and the call is retryable, it re-opens from the last cursor and
/// carries on, so a node restart is a hiccup rather than a gap.
///
/// Progress earns a fresh budget: `max_retries` bounds the retries in one *run*
/// of disconnects, not for the life of the stream. A watch that recovers from a
/// node restart and then runs for days must not spend its budget on the first
/// failure of each of those days.
pub fn watch<Req, M, F, Fut>(
    open: F,
    request: Req,
    options: WatchOptions<Req, M>,
    rpc: &'static str,
) -> Pin<Box<dyn Stream<Item = Result<M, LoamsError>> + Send>>
where
    Req: Clone + Send + 'static,
    M: Send + 'static,
    F: Fn(Req) -> Fut + Send + 'static,
    Fut: Future<Output = Result<Opened<M>, ConnectError>> + Send + 'static,
{
    let state = Watcher {
        open,
        base: request,
        current: None,
        options,
        cursor: None,
        attempt: 0,
        inner: None,
        done: false,
        rpc,
    };
    // Boxed rather than an opaque `impl Stream`: a stream built from an `async`
    // block is `!Unpin`, so an unboxed one would force every caller into
    // `Box::pin` and `pin!`. Boxing once, here, is the difference between
    // `stream.next().await` working and not.
    Box::pin(stream::unfold(Some(state), |maybe| async move {
        let mut watcher = maybe?;
        loop {
            if watcher.done {
                return None;
            }
            if watcher.inner.is_none() {
                let request = watcher
                    .current
                    .clone()
                    .unwrap_or_else(|| (watcher.options.resume)(None, &watcher.base));
                watcher.current = Some(request.clone());
                match (watcher.open)(request).await {
                    Ok(opened) => watcher.inner = Some(opened),
                    Err(thrown) => {
                        // The *open* failed, so no message has been yielded on
                        // this attempt and the resume cursor is still the last
                        // one the caller applied: re-opening from it loses
                        // nothing.
                        let error = LoamsError::from_connect(thrown, Some(watcher.rpc));
                        if watcher.may_retry(&error) {
                            let delay = watcher.next_delay();
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                        watcher.done = true;
                        return Some((Err(error), Some(watcher)));
                    }
                }
            }
            let next = watcher.inner.as_mut().expect("opened above").next().await;
            match next {
                Some(Ok(message)) => {
                    // Progress earns a fresh budget.
                    watcher.attempt = 0;
                    let cursor =
                        (watcher.options.cursor)(&message).or_else(|| watcher.cursor.clone());
                    watcher.cursor.clone_from(&cursor);
                    if let Some(observe) = watcher.options.on_cursor.clone() {
                        observe(cursor.as_deref());
                    }
                    return Some((Ok(message), Some(watcher)));
                }
                Some(Err(thrown)) => {
                    // A message failed mid-stream. The messages already yielded
                    // are **not** re-yielded: the re-open resumes from the last
                    // cursor applied rather than replaying from the start.
                    watcher.inner = None;
                    let error = LoamsError::from_connect(thrown, Some(watcher.rpc));
                    if watcher.may_retry(&error) {
                        let delay = watcher.next_delay();
                        watcher.current = Some((watcher.options.resume)(
                            watcher.cursor.as_deref(),
                            &watcher.base,
                        ));
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    watcher.done = true;
                    return Some((Err(error), Some(watcher)));
                }
                None => {
                    // A clean end is the end, not a reason to re-open: a server
                    // that finished a stream has answered, and re-opening would
                    // turn a completed watch into an endless one.
                    watcher.done = true;
                    return None;
                }
            }
        }
    }))
}

struct Watcher<Req, M, F> {
    open: F,
    base: Req,
    current: Option<Req>,
    options: WatchOptions<Req, M>,
    cursor: Option<String>,
    attempt: u32,
    inner: Option<Opened<M>>,
    done: bool,
    rpc: &'static str,
}

impl<Req, M, F> Watcher<Req, M, F>
where
    Req: Clone,
{
    /// Whether one more open is allowed, and spends the attempt if so.
    fn may_retry(&mut self, error: &LoamsError) -> bool {
        let max = self.options.max_retries.unwrap_or(DEFAULT_MAX_RETRIES);
        let safe = self.options.retry_safe.unwrap_or(true);
        should_retry(error, safe, self.attempt, max)
    }

    /// The backoff before the next open, and the budget it spends.
    fn next_delay(&mut self) -> Duration {
        let delay = backoff(self.attempt, None, (self.options.random)());
        self.attempt += 1;
        delay
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facade;
    use crate::retry::RetryClass;
    use connectrpc::ErrorCode as Code;
    use futures::StreamExt;
    use std::sync::Mutex;

    /// A message shaped like a live `Transition`: it carries a cursor.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Message {
        session: String,
        cursor: String,
    }

    /// The request a re-open carries: the stream's own protocol decides, and
    /// the runtime only supplies the cursor.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Request {
        resume_from: String,
    }

    /// The boxed future `opener` returns, so the test's closure type is readable.
    type OpenedFuture = Pin<Box<dyn Future<Output = Result<Opened<Message>, ConnectError>> + Send>>;

    /// A stream that yields `messages` and then fails, if given one.
    fn session(messages: Vec<Message>, error: Option<ConnectError>) -> Opened<Message> {
        Box::pin(
            stream::iter(messages.into_iter().map(Ok))
                .chain(stream::iter(error.into_iter().map(Err))),
        )
    }

    fn message_cursor(message: &Message) -> Option<String> {
        Some(message.cursor.clone())
    }

    fn resume(cursor: Option<&str>, _base: &Request) -> Request {
        Request {
            resume_from: cursor.unwrap_or("start").to_owned(),
        }
    }

    /// An opener that hands out `batches` in order and records every request it
    /// was asked for.
    fn opener(
        batches: Vec<Opened<Message>>,
    ) -> (
        impl Fn(Request) -> OpenedFuture + Send,
        Arc<std::sync::Mutex<Vec<Request>>>,
    ) {
        let batches = Arc::new(Mutex::new(
            batches.into_iter().map(Some).collect::<Vec<_>>(),
        ));
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let opens = Arc::clone(&seen);
        (
            move |request: Request| {
                let seen = Arc::clone(&opens);
                let batches = Arc::clone(&batches);
                Box::pin(async move {
                    let at = {
                        let mut seen = seen.lock().expect("not poisoned");
                        let at = seen.len();
                        seen.push(request.clone());
                        at
                    };
                    Ok::<Opened<Message>, ConnectError>(
                        batches
                            .lock()
                            .expect("not poisoned")
                            .get_mut(at)
                            .and_then(Option::take)
                            .unwrap_or_else(|| session(vec![], None)),
                    )
                })
                    as std::pin::Pin<
                        Box<dyn Future<Output = Result<Opened<Message>, ConnectError>> + Send>,
                    >
            },
            seen,
        )
    }

    fn options() -> WatchOptions<Request, Message> {
        WatchOptions::new(resume_from_cursor(resume), cursor_of(message_cursor)).random(no_jitter)
    }

    #[test]
    fn the_generated_binding_says_watch_is_a_server_stream_and_the_rest_are_unary() {
        let watch = crate::binding::binding_of("live", "watch").expect("generated");
        assert_eq!(watch.streaming, facade::Streaming::Server);
        assert_eq!(watch.method, "Watch");
        assert_eq!(watch.module, "live");
        // D420: the API has no client stream and no bidi, so every other
        // generated RPC today is unary — including `Watch`'s siblings.
        for module in facade::MODULES {
            for call in module.calls {
                if call.method != "Watch" {
                    assert_eq!(call.streaming, facade::Streaming::Unary, "{}", call.rpc);
                }
            }
        }
        // A live RPC's proto declares no idempotency level, so the class is
        // `Manual`: a stream is not retried by its binding, and the resume is
        // what makes it safe rather than the class.
        assert_eq!(watch.retry_class(), RetryClass::Manual);
    }

    #[tokio::test]
    async fn a_re_open_resumes_from_the_last_cursor_and_repeats_nothing() {
        let (open, seen) = opener(vec![
            session(
                vec![
                    Message {
                        session: "s1".into(),
                        cursor: "c1".into(),
                    },
                    Message {
                        session: "s1".into(),
                        cursor: "c2".into(),
                    },
                ],
                Some(ConnectError::new(Code::Unavailable, "restarting")),
            ),
            session(
                vec![
                    Message {
                        session: "s1".into(),
                        cursor: "c3".into(),
                    },
                    Message {
                        session: "s1".into(),
                        cursor: "c4".into(),
                    },
                ],
                None,
            ),
        ]);
        let stream = watch(
            open,
            Request {
                resume_from: "start".into(),
            },
            options(),
            "r",
        );
        let cursors: Vec<String> = stream
            .map(|item| item.expect("a message").cursor)
            .collect()
            .await;
        assert_eq!(cursors, ["c1", "c2", "c3", "c4"]);
        let opens = seen.lock().expect("not poisoned").clone();
        assert_eq!(opens.len(), 2, "one re-open, not a restart from scratch");
        // The re-open resumes from the **second** message's cursor: the last one
        // applied before the failure, not the first and not a fresh start.
        assert_eq!(
            opens[0],
            Request {
                resume_from: "start".into()
            }
        );
        assert_eq!(
            opens[1],
            Request {
                resume_from: "c2".into()
            }
        );
    }

    #[tokio::test]
    async fn the_caller_observes_every_cursor() {
        let (open, _seen) = opener(vec![session(
            vec![
                Message {
                    session: "s1".into(),
                    cursor: "c1".into(),
                },
                Message {
                    session: "s1".into(),
                    cursor: "c2".into(),
                },
            ],
            None,
        )]);
        let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&observed);
        let stream = watch(
            open,
            Request {
                resume_from: "start".into(),
            },
            options().on_cursor(Arc::new(move |cursor: Option<&str>| {
                sink.lock()
                    .expect("not poisoned")
                    .push(cursor.unwrap_or("").to_owned());
            })),
            "r",
        );
        let mut stream = stream;
        while let Some(item) = stream.next().await {
            item.expect("a message");
        }
        assert_eq!(*observed.lock().expect("not poisoned"), ["c1", "c2"]);
    }

    #[tokio::test]
    async fn a_failure_the_class_does_not_cover_is_reported_not_spun_on() {
        // An `unimplemented` stream is what every `LiveService` RPC answers in
        // every variant today, so this is the common real failure (R5, R7).
        let (open, seen) = opener(vec![session(
            vec![],
            Some(ConnectError::new(
                Code::Unimplemented,
                "not in this variant",
            )),
        )]);
        let stream = watch(
            open,
            Request {
                resume_from: "start".into(),
            },
            options(),
            "r",
        );
        let mut failures = Vec::new();
        let mut stream = stream;
        while let Some(item) = stream.next().await {
            failures.push(item.expect_err("a failure"));
        }
        assert_eq!(failures.len(), 1, "reported once");
        assert!(failures[0].is_feature_not_in_variant() || failures[0].code == Code::Unimplemented);
        assert_eq!(
            seen.lock().expect("not poisoned").len(),
            1,
            "and not re-opened"
        );
    }

    #[tokio::test]
    async fn a_retry_budget_bounds_one_run_of_disconnects() {
        // Four sessions, each dying immediately: two retries are spent and the
        // third failure is reported.
        let batches = vec![
            session(
                vec![],
                Some(ConnectError::new(Code::Unavailable, "restarting")),
            ),
            session(
                vec![],
                Some(ConnectError::new(Code::Unavailable, "restarting")),
            ),
            session(
                vec![],
                Some(ConnectError::new(Code::Unavailable, "restarting")),
            ),
        ];
        let (open, seen) = opener(batches);
        let stream = watch(
            open,
            Request {
                resume_from: "start".into(),
            },
            options().max_retries(2),
            "r",
        );
        let mut failures = 0;
        let mut stream = stream;
        while let Some(item) = stream.next().await {
            assert!(item.is_err());
            failures += 1;
        }
        assert_eq!(failures, 1, "the last failure is reported, not swallowed");
        assert_eq!(
            seen.lock().expect("not poisoned").len(),
            3,
            "one attempt plus two retries"
        );
    }

    #[tokio::test]
    async fn a_message_earns_a_fresh_budget() {
        // Five sessions of one message and one failure each: the budget resets on
        // every message, so the stream runs to the end instead of dying on the
        // fourth failure.
        let batches = vec![
            session(
                vec![Message {
                    session: "s1".into(),
                    cursor: "c1".into(),
                }],
                Some(ConnectError::new(Code::Unavailable, "restarting")),
            ),
            session(
                vec![Message {
                    session: "s1".into(),
                    cursor: "c2".into(),
                }],
                Some(ConnectError::new(Code::Unavailable, "restarting")),
            ),
            session(
                vec![Message {
                    session: "s1".into(),
                    cursor: "c3".into(),
                }],
                Some(ConnectError::new(Code::Unavailable, "restarting")),
            ),
            session(
                vec![Message {
                    session: "s1".into(),
                    cursor: "c4".into(),
                }],
                None,
            ),
        ];
        let (open, seen) = opener(batches);
        let stream = watch(
            open,
            Request {
                resume_from: "start".into(),
            },
            options().max_retries(1),
            "r",
        );
        let cursors: Vec<String> = stream
            .map(|item| item.expect("a message").cursor)
            .collect()
            .await;
        assert_eq!(cursors, ["c1", "c2", "c3", "c4"]);
        assert_eq!(seen.lock().expect("not poisoned").len(), 4);
    }

    #[tokio::test]
    async fn a_retry_safe_false_never_re_opens() {
        let (open, seen) = opener(vec![session(
            vec![Message {
                session: "s1".into(),
                cursor: "c1".into(),
            }],
            Some(ConnectError::new(Code::Unavailable, "restarting")),
        )]);
        let stream = watch(
            open,
            Request {
                resume_from: "start".into(),
            },
            options().retry_safe(false),
            "r",
        );
        let mut seen_cursor = Vec::new();
        let mut failures = 0;
        let mut stream = stream;
        while let Some(item) = stream.next().await {
            match item {
                Ok(message) => seen_cursor.push(message.cursor),
                Err(_) => failures += 1,
            }
        }
        assert_eq!(seen_cursor, ["c1"]);
        assert_eq!(failures, 1);
        assert_eq!(seen.lock().expect("not poisoned").len(), 1, "not re-opened");
    }

    #[tokio::test]
    async fn a_clean_end_is_the_end_not_a_reason_to_re_open() {
        let (open, seen) = opener(vec![session(
            vec![Message {
                session: "s1".into(),
                cursor: "c1".into(),
            }],
            None,
        )]);
        let stream = watch(
            open,
            Request {
                resume_from: "start".into(),
            },
            options(),
            "r",
        );
        let mut count = 0;
        let mut stream = stream;
        while let Some(item) = stream.next().await {
            assert!(item.is_ok());
            count += 1;
        }
        assert_eq!(count, 1);
        assert_eq!(seen.lock().expect("not poisoned").len(), 1, "not re-opened");
    }
}
