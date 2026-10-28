//! Sending one request until the provider accepts it, refuses it for good,
//! or the policy runs out. The caller builds the request again for each
//! attempt with the same idempotency key.

// Used only by the provider features.
#![cfg_attr(
    not(any(
        feature = "stripe",
        feature = "openmeter",
        feature = "lago",
        feature = "webhook"
    )),
    allow(dead_code)
)]

use std::future::Future;
use std::time::Duration;

use crate::{ExportError, short};

/// How often and how long an exporter retries one request.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Attempts in one run, the first included (at least 1).
    pub attempts: u32,
    /// The first back-off; it doubles after each attempt.
    pub base: Duration,
    /// The longest back-off, also the cap on a provider's `Retry-After`.
    pub max: Duration,
}

impl Default for RetryPolicy {
    fn default() -> RetryPolicy {
        RetryPolicy {
            attempts: 5,
            base: Duration::from_millis(200),
            max: Duration::from_secs(10),
        }
    }
}

/// A provider response, reduced to what the retry loop needs.
#[derive(Debug)]
pub(crate) struct Reply {
    pub status: u16,
    pub body: String,
    pub retry_after: Option<Duration>,
}

impl Reply {
    pub(crate) async fn from_response(r: reqwest::Response) -> Reply {
        let status = r.status().as_u16();
        let retry_after = r
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        let body = r.text().await.unwrap_or_default();
        Reply {
            status,
            body,
            retry_after,
        }
    }
}

/// Whether a status may succeed if sent again: timeouts, conflicts with a
/// request in flight, rate limits and server errors.
pub(crate) fn retryable(status: u16) -> bool {
    matches!(status, 408 | 409 | 425 | 429) || status >= 500
}

/// Sends with `attempt` until a 2xx (returned), a final refusal or the end
/// of the policy. `message` turns a refusal body into the error's text.
pub(crate) async fn send<F, Fut>(
    provider: &'static str,
    policy: &RetryPolicy,
    message: impl Fn(&str) -> String,
    mut attempt: F,
) -> Result<Reply, ExportError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<reqwest::Response, reqwest::Error>>,
{
    let attempts = policy.attempts.max(1);
    let mut backoff = policy.base;
    let mut last = String::new();
    for n in 1..=attempts {
        let wait = match attempt().await {
            Ok(r) => {
                let reply = Reply::from_response(r).await;
                if (200..300).contains(&reply.status) {
                    return Ok(reply);
                }
                if !retryable(reply.status) {
                    return Err(ExportError::Rejected {
                        provider,
                        status: reply.status,
                        message: short(&message(&reply.body)),
                    });
                }
                last = format!("{}: {}", reply.status, short(&message(&reply.body)));
                reply.retry_after.map(|d| d.min(policy.max))
            }
            Err(e) => {
                // reqwest's error names the URL, never the body or headers.
                last = short(&e.to_string());
                None
            }
        };
        if n < attempts {
            tracing::debug!(provider, attempt = n, error = %last, "export: retrying");
            tokio::time::sleep(wait.unwrap_or(backoff)).await;
            backoff = (backoff * 2).min(policy.max);
        }
    }
    Err(ExportError::Exhausted {
        provider,
        attempts,
        last,
    })
}

#[cfg(test)]
mod tests {
    use super::retryable;

    #[test]
    fn which_statuses_retry() {
        for s in [408, 409, 425, 429, 500, 502, 503] {
            assert!(retryable(s), "{s}");
        }
        for s in [400, 401, 403, 404, 422] {
            assert!(!retryable(s), "{s}");
        }
    }
}
