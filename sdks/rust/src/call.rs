//! The call path: the one place a facade method becomes an RPC
//! (design §44 §7.4; runtime contract R1–R4).
//!
//! It does four things the generated clients cannot, and nothing else:
//!
//! * attaches the bearer from the client's [`TokenSource`], and the consistency
//!   token a session holds (R1, R4);
//! * retries on the call's **class from the generated bindings**, with M1.6's
//!   backoff numbers, and refreshes the token **once** on `token_expired` (R1,
//!   R2);
//! * gives a mutating call an idempotency key **once per logical call** and
//!   reuses it on every retry, so a retried write is the same write (R3, D610);
//! * turns whatever came back into the typed [`LoamsError`], so a caller
//!   branches on `reason` and never on a message (R8).
//!
//! The loop is written against a **sender** rather than a transport, so the
//! retry policy, the idempotency-key lifecycle and the refresh-once behaviour
//! are the SDK's own logic and are pinned without a server
//! (`rust_retry_reuses_idempotency_key`, `rust_token_source_refresh`).

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use connectrpc::ConnectError;
use connectrpc::client::CallOptions;
use http::header::{AUTHORIZATION, HeaderName, HeaderValue};

use crate::error::LoamsError;
use crate::request::{CONSISTENCY_HEADER, Consistency, ConsistencySession};
use crate::retry::{backoff, should_retry};
use crate::token::TokenSource;

/// Per-call settings. Everything here is optional: the client's defaults apply,
/// and the generated binding supplies the retry class.
#[derive(Debug, Clone, Default)]
pub struct CallOptionsOverrides {
    /// Retries after the first attempt. `0` disables retrying for this call.
    pub max_retries: Option<u32>,
    /// The idempotency key for a mutating call. Supply your own to make a retry
    /// yours rather than the SDK's; omit it and the SDK mints one UUIDv7 per
    /// logical call and reuses it on every retry.
    pub idempotency_key: Option<String>,
    /// Extra request headers. `authorization` is set by the client and wins.
    pub headers: Vec<(HeaderName, HeaderValue)>,
    /// Overrides the call's retry class for this call only.
    pub retry_safe: Option<bool>,
    /// The consistency to read at (§44 §7.4, D609).
    pub consistency: Option<Consistency>,
    /// Records this call's response token into a session, so later reads are
    /// read-your-writes. `None` uses the client's session when it has one.
    pub session: Option<std::sync::Arc<ConsistencySession>>,
    /// This call's deadline.
    pub timeout: Option<Duration>,
}

impl CallOptionsOverrides {
    /// No overrides: the client's defaults and the binding's class.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the idempotency key for this call.
    #[must_use]
    pub fn idempotency_key(mut self, key: impl Into<String>) -> Self {
        self.idempotency_key = Some(key.into());
        self
    }

    /// Sets this call's retry budget.
    #[must_use]
    pub fn max_retries(mut self, retries: u32) -> Self {
        self.max_retries = Some(retries);
        self
    }

    /// Overrides the call's retry class.
    #[must_use]
    pub fn retry_safe(mut self, safe: bool) -> Self {
        self.retry_safe = Some(safe);
        self
    }

    /// Adds a request header.
    #[must_use]
    pub fn header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        self.headers.push((name, value));
        self
    }

    /// Reads at a consistency rather than the server's default.
    #[must_use]
    pub fn consistency(mut self, consistency: Consistency) -> Self {
        self.consistency = Some(consistency);
        self
    }

    /// Records every response's token into this session.
    #[must_use]
    pub fn session(mut self, session: std::sync::Arc<ConsistencySession>) -> Self {
        self.session = Some(session);
        self
    }

    /// Bounds this call.
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

/// Everything the retry loop needs, so it can be driven with a stub sender.
///
/// It **owns** its credentials and headers rather than borrowing them: the loop
/// outlives the scope that built the plan, and a plan that borrowed the client's
/// token source could not be handed to an async helper without threading a
/// lifetime through every module method.
#[derive(Debug, Clone)]
pub struct RetryPlan {
    /// The call's retry class, or `retry_safe` when the caller overrode it.
    pub retry_safe: bool,
    /// Retries after the first attempt.
    pub max_retries: u32,
    /// The bearer source, when the client has one.
    pub token_source: Option<Arc<dyn TokenSource>>,
    /// The caller's own headers.
    pub headers: Vec<(HeaderName, HeaderValue)>,
    /// The consistency to send, when there is one.
    pub consistency: Option<String>,
    /// The session to fold this call's response token into.
    pub session: Option<Arc<ConsistencySession>>,
    /// This call's deadline.
    pub timeout: Option<Duration>,
    /// Full jitter is `random()` in production; the tests pass a fixed value so
    /// a backoff is exact rather than merely bounded.
    pub random: fn() -> f64,
}

impl RetryPlan {
    /// The default source of jitter: uniform over `[0, 1)`.
    ///
    /// `f64::from_bits` on 53 random bits is uniform without the modulo bias a
    /// narrow integer draw would have, which matters because the whole point of
    /// full jitter is that two clients do not pick the same delay.
    #[must_use]
    pub fn jitter() -> fn() -> f64 {
        crate::uuidv7::unit_jitter
    }

    /// The headers one attempt carries: the caller's, plus the bearer and the
    /// consistency token the runtime owns.
    ///
    /// A bearer is **never** put in the query string or a URL (R1): only in
    /// `authorization`, which is the one header a proxy log redacts.
    pub async fn headers(&self) -> Result<Vec<(HeaderName, HeaderValue)>, LoamsError> {
        let mut headers: Vec<(HeaderName, HeaderValue)> = self.headers.clone();
        // `Authorization` is *replaced* rather than appended to: a caller who
        // set one by hand has it overridden by the client's credential, which is
        // what "the client sets it and wins" means.
        headers.retain(|(name, _)| name != AUTHORIZATION);
        // The bearer is read **per attempt**, so a refreshing source can answer
        // differently on the retry than it did on the first attempt.
        let bearer = match &self.token_source {
            Some(source) => source.token().await,
            None => None,
        };
        if let Some(bearer) = bearer {
            let value = HeaderValue::from_str(&format!("Bearer {bearer}"))
                .map_err(|_| LoamsError::internal("the bearer is not a valid header value"))?;
            headers.push((AUTHORIZATION, value));
        }
        if let Some(consistency) = &self.consistency {
            let name = HeaderName::from_static(CONSISTENCY_HEADER);
            let value = HeaderValue::from_str(consistency).map_err(|_| {
                LoamsError::internal("the consistency token is not a valid header value")
            })?;
            headers.retain(|(existing, _)| existing != name);
            headers.push((name, value));
        }
        Ok(headers)
    }

    /// The connect-rust options one attempt carries.
    async fn call_options(&self) -> Result<CallOptions, LoamsError> {
        let mut options = CallOptions::default();
        for (name, value) in self.headers().await? {
            options = options.with_header(name, value);
        }
        if let Some(timeout) = self.timeout {
            options = options.with_timeout(timeout);
        }
        Ok(options)
    }
}

/// What one attempt carries.
#[derive(Debug)]
pub struct Attempt {
    /// The connect-rust options for this attempt, headers included.
    pub options: CallOptions,
    /// Zero for the first attempt, then one per retry.
    pub attempt: u32,
    /// Whether this attempt follows a credential refresh.
    pub refreshed: bool,
}

/// Runs `send` until it answers or the plan says stop, and returns the last
/// failure as a [`LoamsError`].
///
/// `request` is passed **by value** and handed to `send` on every attempt: the
/// idempotency key lives inside it (R3), so every attempt carries the identical
/// key whatever the sender does with the value.
pub async fn call_with_retry<Req, Resp, F, Fut>(
    request: Req,
    mut send: F,
    plan: &RetryPlan,
    rpc: &str,
) -> Result<Resp, LoamsError>
where
    Req: Clone,
    F: FnMut(Req, Attempt) -> Fut,
    Fut: Future<Output = Result<Resp, ConnectError>>,
{
    let mut refreshed = false;
    let mut attempt: u32 = 0;
    loop {
        let options = plan.call_options().await?;
        match send(
            request.clone(),
            Attempt {
                options,
                attempt,
                refreshed,
            },
        )
        .await
        {
            Ok(response) => return Ok(response),
            Err(thrown) => {
                let error = LoamsError::from_connect(thrown, Some(rpc));
                // A source that cannot refresh (an API key) makes the refresh
                // branch a no-op, and a second expiry is reported rather than
                // looped on — refreshing more than once turns an auth outage
                // into a refresh storm.
                let source = plan
                    .token_source
                    .as_ref()
                    .filter(|source| source.can_refresh());
                // R1: a 401 whose reason is `token_expired` gets **exactly one**
                // refresh and one retry, and only from a source that can refresh.
                if error.is_token_expired()
                    && !refreshed
                    && let Some(source) = source
                {
                    refreshed = true;
                    // The refresh itself may fail, and that failure is the honest
                    // one to report — not the expiry it was answering, which the
                    // caller cannot act on.
                    if let Err(failure) = source.refresh().await {
                        return Err(failure.with_rpc(rpc));
                    }
                    continue;
                }
                if !should_retry(&error, plan.retry_safe, attempt, plan.max_retries) {
                    return Err(error);
                }
                tokio::time::sleep(backoff(attempt, None, (plan.random)())).await;
                attempt += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;
    use crate::token::ApiKey;
    use connectrpc::ErrorCode as Code;

    fn plan(retry_safe: bool, max_retries: u32) -> RetryPlan {
        RetryPlan {
            retry_safe,
            max_retries,
            token_source: None,
            headers: Vec::new(),
            consistency: None,
            session: None,
            timeout: None,
            random: RetryPlan::jitter(),
        }
    }

    /// A `401` carrying `reason = token_expired`, the way connect-rust shapes
    /// one.
    pub(crate) fn expiry() -> ConnectError {
        let mut error = ConnectError::new(Code::Unauthenticated, "the access token expired");
        error.details.push(connectrpc::ErrorDetail::from_message(
            "loams.errors.v1.ErrorInfo",
            &loams_proto::loams::errors::v1::ErrorInfo {
                reason: "token_expired".to_owned(),
                ..Default::default()
            },
        ));
        error
    }

    #[tokio::test]
    async fn a_retryable_failure_is_retried_up_to_the_budget_and_then_reported() {
        let mut attempts = 0;
        let result: Result<(), _> = call_with_retry(
            (),
            |_, _| {
                attempts += 1;
                async { Err::<(), _>(ConnectError::new(Code::Unavailable, "restarting")) }
            },
            &plan(true, 3),
            "loams.test.v1.ThingService/Read",
        )
        .await;
        assert_eq!(attempts, 4, "one attempt plus three retries");
        let error = result.unwrap_err();
        assert_eq!(error.code, Code::Unavailable);
        assert_eq!(error.rpc(), Some("loams.test.v1.ThingService/Read"));
    }

    #[tokio::test]
    async fn a_non_retryable_failure_is_reported_on_the_first_attempt() {
        let mut attempts = 0;
        let result: Result<(), _> = call_with_retry(
            (),
            |_, _| {
                attempts += 1;
                async { Err::<(), _>(ConnectError::new(Code::Internal, "a bug")) }
            },
            &plan(true, 3),
            "loams.test.v1.ThingService/Read",
        )
        .await;
        assert_eq!(attempts, 1);
        assert_eq!(result.unwrap_err().kind, ErrorKind::Internal);
    }

    #[tokio::test]
    async fn an_api_key_cannot_refresh_so_an_expiry_is_reported_not_retried() {
        let key = ApiKey::new("k").expect("not empty");
        let mut attempts = 0;
        let result: Result<(), _> = call_with_retry(
            (),
            |_, _| {
                attempts += 1;
                async { Err::<(), _>(expiry()) }
            },
            &RetryPlan {
                token_source: Some(Arc::new(key.clone())),
                ..plan(true, 3)
            },
            "r",
        )
        .await;
        assert_eq!(attempts, 1, "a key that cannot refresh is not retried");
        assert!(result.unwrap_err().is_token_expired());
    }

    #[tokio::test]
    async fn the_request_goes_out_identical_on_every_attempt() {
        // The request carries the key, and the loop never rewrites it (R3).
        let seen: std::sync::Arc<std::sync::Mutex<Vec<Vec<u8>>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorder = std::sync::Arc::clone(&seen);
        let attempts = std::sync::Arc::new(std::sync::Mutex::new(0usize));
        let result: Result<(), _> = call_with_retry(
            vec![7u8; 4],
            move |request, _| {
                let attempts = std::sync::Arc::clone(&attempts);
                let recorder = std::sync::Arc::clone(&recorder);
                async move {
                    recorder.lock().expect("not poisoned").push(request.clone());
                    let mut attempts = attempts.lock().expect("not poisoned");
                    *attempts += 1;
                    if *attempts < 3 {
                        Err(ConnectError::new(Code::Unavailable, "restarting"))
                    } else {
                        Ok(())
                    }
                }
            },
            &plan(true, 3),
            "r",
        )
        .await;
        assert!(result.is_ok());
        let seen = seen.lock().expect("not poisoned").clone();
        assert_eq!(seen.len(), 3);
        assert!(seen.iter().all(|body| body == &seen[0]));
    }

    #[tokio::test]
    async fn the_bearer_is_a_header_and_nothing_else_carries_it() {
        let key = ApiKey::new("secret").expect("not empty");
        let headers = RetryPlan {
            token_source: Some(Arc::new(key.clone())),
            ..plan(true, 3)
        }
        .headers()
        .await
        .expect("built");
        let auth = headers
            .iter()
            .find(|(name, _)| name == AUTHORIZATION)
            .expect("a bearer");
        assert_eq!(auth.1.to_str().expect("ascii"), "Bearer secret");
        // Nothing else carries the credential, so a proxy log cannot record it.
        assert_eq!(headers.len(), 1);
    }

    #[tokio::test]
    async fn a_caller_set_authorization_header_is_replaced_by_the_clients_credential() {
        let key = ApiKey::new("real").expect("not empty");
        let headers = vec![
            (AUTHORIZATION, HeaderValue::from_static("Bearer not-mine")),
            (
                HeaderName::from_static("x-request-id"),
                HeaderValue::from_static("req_1"),
            ),
        ];
        let built = RetryPlan {
            token_source: Some(Arc::new(key.clone())),
            headers,
            ..plan(true, 3)
        }
        .headers()
        .await
        .expect("built");
        assert_eq!(built.len(), 2);
        let auth = built
            .iter()
            .find(|(name, _)| name == AUTHORIZATION)
            .expect("a bearer");
        assert_eq!(auth.1.to_str().expect("ascii"), "Bearer real");
        assert!(
            built
                .iter()
                .any(|(name, _)| name.as_str() == "x-request-id")
        );
    }

    #[tokio::test]
    async fn the_consistency_token_rides_a_header_and_is_not_duplicated() {
        let headers = vec![(
            HeaderName::from_static(CONSISTENCY_HEADER),
            HeaderValue::from_static("at_least:stale"),
        )];
        let built = RetryPlan {
            headers,
            consistency: Some("at_least:v1:a".to_owned()),
            ..plan(true, 3)
        }
        .headers()
        .await
        .expect("built");
        assert_eq!(built.len(), 1);
        assert_eq!(built[0].1.to_str().expect("ascii"), "at_least:v1:a");
    }
}
