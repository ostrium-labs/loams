//! `streams.subscribe` (runtime contract R14, SDKP §2.5 and SDKP-2).
//!
//! A client loop over unary `Fetch`, so it works on every transport and
//! through every proxy, as a [`futures::Stream`]:
//!
//! * the next offset is the last record's offset + 1;
//! * an empty fetch backs off 100 ms, doubling, up to 1 s, and the backoff
//!   resets when a record arrives;
//! * [`Subscription::offset`] is always the next offset to read, so a caller
//!   can drop the stream and resume without losing or repeating a record;
//! * dropping the stream stops the loop: no further fetch is sent.
//!
//! The stream is not `Send`: a unary call's future holds the client's runtime
//! by reference across its await, so poll it on the task that made it (or a
//! `LocalSet`).
//!
//! A fetch that fails ends the stream with that error (the call path has
//! already retried it as a `safe` call), and the offset still says where to
//! resume.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use futures::Stream;
use futures::stream::LocalBoxStream;

use crate::error::LoamsError;

/// The first pause after an empty fetch.
pub const SUBSCRIBE_MIN_BACKOFF: Duration = Duration::from_millis(100);
/// The longest pause after an empty fetch.
pub const SUBSCRIBE_MAX_BACKOFF: Duration = Duration::from_secs(1);

/// A record with a position in its partition.
pub trait OffsetRecord {
    /// The record's offset.
    fn offset(&self) -> u64;
}

/// A running subscription.
pub struct Subscription<R> {
    inner: LocalBoxStream<'static, Result<R, LoamsError>>,
    offset: Arc<AtomicU64>,
}

impl<R> std::fmt::Debug for Subscription<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subscription")
            .field("offset", &self.offset())
            .finish_non_exhaustive()
    }
}

impl<R> Subscription<R> {
    /// The next offset to read.
    #[must_use]
    pub fn offset(&self) -> u64 {
        self.offset.load(Ordering::SeqCst)
    }
}

impl<R> Stream for Subscription<R> {
    type Item = Result<R, LoamsError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

struct State<R, F> {
    fetch: F,
    buffer: VecDeque<R>,
    offset: Arc<AtomicU64>,
    backoff: Duration,
    done: bool,
}

/// Starts a subscription at `from_offset`. `fetch(offset)` is one `Fetch`
/// from that offset. Nothing is sent until the stream is first polled.
pub fn subscribe<R, F, Fut>(from_offset: u64, fetch: F) -> Subscription<R>
where
    R: OffsetRecord + 'static,
    F: FnMut(u64) -> Fut + 'static,
    Fut: Future<Output = Result<Vec<R>, LoamsError>> + 'static,
{
    let offset = Arc::new(AtomicU64::new(from_offset));
    let state = State {
        fetch,
        buffer: VecDeque::<R>::new(),
        offset: Arc::clone(&offset),
        backoff: SUBSCRIBE_MIN_BACKOFF,
        done: false,
    };
    let inner = futures::stream::unfold(state, |mut state| async move {
        if state.done {
            return None;
        }
        loop {
            if let Some(record) = state.buffer.pop_front() {
                state
                    .offset
                    .store(record.offset().saturating_add(1), Ordering::SeqCst);
                return Some((Ok(record), state));
            }
            let next = state.offset.load(Ordering::SeqCst);
            match (state.fetch)(next).await {
                Ok(records) => {
                    // A record below the next offset was already yielded.
                    state
                        .buffer
                        .extend(records.into_iter().filter(|r| r.offset() >= next));
                    if state.buffer.is_empty() {
                        tokio::time::sleep(state.backoff).await;
                        state.backoff = (state.backoff * 2).min(SUBSCRIBE_MAX_BACKOFF);
                    } else {
                        state.backoff = SUBSCRIBE_MIN_BACKOFF;
                    }
                }
                Err(error) => {
                    state.done = true;
                    return Some((Err(error), state));
                }
            }
        }
    });
    Subscription {
        inner: Box::pin(inner),
        offset,
    }
}
