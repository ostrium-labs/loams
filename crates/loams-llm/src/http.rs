//! The HTTP transport shared by the providers: one `reqwest` client per
//! provider, JSON in and out, retries with jitter on 429 and 5xx.
//!
//! Nothing here logs a body, a header value or a URL path's query: events
//! carry the provider, the model name, the status, the attempt and timings.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use reqwest::header::{HeaderMap, RETRY_AFTER};
use serde_json::Value;
use url::{Host, Url};

use crate::error::LlmError;

/// Retries on 408, 429 and 5xx, and on transport failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Retries after the first attempt (3).
    pub max_retries: u32,
    /// The first backoff (500 ms), doubled per retry, with jitter.
    pub base_delay: Duration,
    /// The longest backoff (8 s).
    pub max_delay: Duration,
    /// The longest `Retry-After` honoured (30 s). A longer one ends the
    /// call with a retryable [`LlmError::Provider`], so the caller's
    /// durable job retries later instead of holding a task.
    pub max_retry_after: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(8),
            max_retry_after: Duration::from_secs(30),
        }
    }
}

/// Transport options of a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpOptions {
    /// The whole request's timeout (120 s).
    pub timeout: Duration,
    /// Retries on 408, 429, 5xx and transport failures.
    pub retry: RetryPolicy,
}

impl Default for HttpOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(120),
            retry: RetryPolicy::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Http {
    client: reqwest::Client,
    retry: RetryPolicy,
}

impl Http {
    pub(crate) fn new(base: &Url, opts: HttpOptions) -> Result<Self, LlmError> {
        // The process's provider; ring when none was installed, as `loams`
        // does for its own clients.
        let _ = rustls::crypto::CryptoProvider::install_default(
            rustls::crypto::ring::default_provider(),
        );
        let mut builder = reqwest::Client::builder()
            .timeout(opts.timeout)
            // A redirect could leave loopback or the allow-list, and would
            // carry the key header to another host.
            .redirect(reqwest::redirect::Policy::none());
        if let Some(Host::Domain(d)) = base.host()
            && d.eq_ignore_ascii_case("localhost")
        {
            // `localhost` is loopback whatever the resolver says.
            let port = base.port_or_known_default().unwrap_or(80);
            builder = builder.resolve(d, SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port));
        }
        let client = builder
            .build()
            .map_err(|_| LlmError::InvalidConfig("HTTP client could not be built".to_string()))?;
        Ok(Self {
            client,
            retry: opts.retry,
        })
    }

    /// POSTs `body` and answers the response's JSON, retrying per the
    /// policy.
    pub(crate) async fn post_json(
        &self,
        provider: &'static str,
        model: &str,
        url: Url,
        headers: HeaderMap,
        body: &Value,
    ) -> Result<Value, LlmError> {
        let mut attempt: u32 = 0;
        loop {
            let started = Instant::now();
            let sent = self
                .client
                .post(url.clone())
                .headers(headers.clone())
                .json(body)
                .send()
                .await;
            let wait = match sent {
                Err(e) => {
                    let kind = transport_kind(&e);
                    tracing::debug!(provider, model, attempt, kind, "llm transport failure");
                    if attempt >= self.retry.max_retries {
                        return Err(LlmError::Transport(kind.to_string()));
                    }
                    self.backoff(attempt)
                }
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    tracing::debug!(
                        provider,
                        model,
                        attempt,
                        status,
                        elapsed_ms =
                            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                        "llm call answered"
                    );
                    if resp.status().is_success() {
                        // A body that fails to arrive is a transport failure
                        // and retried; a body that arrives but is not JSON is
                        // invalid output.
                        match resp.bytes().await {
                            Ok(bytes) => {
                                return serde_json::from_slice::<Value>(&bytes).map_err(|_| {
                                    LlmError::InvalidOutput(
                                        "provider answer is not JSON".to_string(),
                                    )
                                });
                            }
                            Err(e) => {
                                let kind = transport_kind(&e);
                                tracing::debug!(
                                    provider,
                                    model,
                                    attempt,
                                    kind,
                                    "llm transport failure"
                                );
                                if attempt >= self.retry.max_retries {
                                    return Err(LlmError::Transport(kind.to_string()));
                                }
                                self.backoff(attempt)
                            }
                        }
                    } else {
                        let retryable = is_retryable(status);
                        if !retryable || attempt >= self.retry.max_retries {
                            return Err(LlmError::Provider { status, retryable });
                        }
                        match retry_after(resp.headers()) {
                            Some(d) if d > self.retry.max_retry_after => {
                                return Err(LlmError::Provider { status, retryable });
                            }
                            Some(d) => d,
                            None => self.backoff(attempt),
                        }
                    }
                }
            };
            tokio::time::sleep(wait).await;
            attempt += 1;
        }
    }

    fn backoff(&self, attempt: u32) -> Duration {
        let exp = self
            .retry
            .base_delay
            .saturating_mul(1u32.checked_shl(attempt).unwrap_or(u32::MAX))
            .min(self.retry.max_delay);
        exp.mul_f64(rand::random_range(0.5..=1.0))
    }
}

/// 408, 429, and every 5xx (Anthropic's 529 included).
fn is_retryable(status: u16) -> bool {
    matches!(status, 408 | 429) || (500..600).contains(&status)
}

/// `Retry-After` in delta-seconds. An HTTP-date is ignored (the backoff
/// applies).
fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let v = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    v.parse::<u64>().ok().map(Duration::from_secs)
}

/// A fixed description of a transport error (never its URL or body).
fn transport_kind(e: &reqwest::Error) -> &'static str {
    if e.is_timeout() {
        "timeout"
    } else if e.is_connect() {
        "connect"
    } else if e.is_body() || e.is_decode() {
        "body"
    } else {
        "request"
    }
}

/// `base` with `path` appended (a trailing `/` is added to `base` first, so
/// `…/v1` + `chat/completions` is `…/v1/chat/completions`).
pub(crate) fn join(base: &Url, path: &str) -> Result<Url, LlmError> {
    let mut b = base.clone();
    if !b.path().ends_with('/') {
        let p = format!("{}/", b.path());
        b.set_path(&p);
    }
    b.join(path)
        .map_err(|_| LlmError::InvalidConfig("base URL cannot be joined".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins() {
        let u = Url::parse("http://127.0.0.1:11434/v1").expect("url");
        assert_eq!(
            join(&u, "chat/completions").expect("join").as_str(),
            "http://127.0.0.1:11434/v1/chat/completions"
        );
        let a = Url::parse("https://api.anthropic.com").expect("url");
        assert_eq!(
            join(&a, "v1/messages").expect("join").as_str(),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn retryable_statuses() {
        assert!(is_retryable(429) && is_retryable(503) && is_retryable(529));
        assert!(!is_retryable(400) && !is_retryable(401) && !is_retryable(404));
    }
}
