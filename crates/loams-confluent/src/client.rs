//! The Confluent Cloud API with a Cloud API key: retries and error mapping.

use std::fmt;
use std::time::Duration;

use reqwest::{Method, StatusCode};
use serde_json::Value;

use crate::Secret;

/// The Confluent Cloud API.
pub const DEFAULT_BASE: &str = "https://api.confluent.cloud";

/// The longest `Retry-After` the client honours before giving up waiting.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

/// A Confluent Cloud API key (id and secret). `Debug` shows the id only.
#[derive(Clone)]
pub struct CloudKey {
    id: String,
    secret: Secret,
}

impl CloudKey {
    #[must_use]
    pub fn new(id: impl Into<String>, secret: Secret) -> Self {
        Self {
            id: id.into(),
            secret,
        }
    }

    /// `CONFLUENT_CLOUD_API_KEY` and `CONFLUENT_CLOUD_API_SECRET`, the names
    /// Confluent's provider and CLI read.
    pub fn from_env() -> Result<Self, Error> {
        let id = std::env::var("CONFLUENT_CLOUD_API_KEY").unwrap_or_default();
        let secret = std::env::var("CONFLUENT_CLOUD_API_SECRET").unwrap_or_default();
        if id.is_empty() || secret.is_empty() {
            return Err(Error::NoCredentials);
        }
        Ok(Self::new(id, Secret::new(secret)))
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
}

impl fmt::Debug for CloudKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CloudKey")
            .field("id", &self.id)
            .field("secret", &self.secret)
            .finish()
    }
}

/// What can go wrong. No variant ever holds a credential.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "set CONFLUENT_CLOUD_API_KEY and CONFLUENT_CLOUD_API_SECRET to your Confluent Cloud API key"
    )]
    NoCredentials,
    #[error("Confluent rejected the Cloud API key (HTTP 401)")]
    Unauthorized,
    #[error("the Cloud API key may not {0} (HTTP 403)")]
    Forbidden(String),
    #[error("{0}: not found (HTTP 404)")]
    NotFound(String),
    #[error("{what}: Confluent answered HTTP {status}: {message}")]
    Api {
        status: u16,
        what: String,
        message: String,
    },
    #[error("{what}: still rate limited by Confluent after {tries} tries")]
    RateLimited { what: String, tries: u32 },
    #[error("{what}: could not reach Confluent: {message}")]
    Transport { what: String, message: String },
    #[error("{what}: unexpected response: {message}")]
    Decode { what: String, message: String },
    #[error("{0}")]
    Usage(String),
    #[error("secret store: {0}")]
    Store(String),
}

/// A client for one Cloud API key.
#[derive(Debug, Clone)]
pub struct Client {
    base: String,
    http: reqwest::Client,
    key: CloudKey,
    tries: u32,
    backoff: Duration,
}

impl Client {
    /// `base` is [`DEFAULT_BASE`] except in tests. The caller builds the
    /// `reqwest::Client`, so TLS is configured where the process configures it.
    #[must_use]
    pub fn new(base: &str, key: CloudKey, http: reqwest::Client) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            http,
            key,
            tries: 4,
            backoff: Duration::from_millis(500),
        }
    }

    /// The first retry's wait (doubled for each later one); tests shorten it.
    #[must_use]
    pub fn with_backoff(mut self, backoff: Duration) -> Self {
        self.backoff = backoff;
        self
    }

    /// Another API with another key (a cluster's REST endpoint with a Kafka
    /// key), sharing this client's HTTP settings and retries.
    #[must_use]
    pub fn for_endpoint(&self, base: &str, key: CloudKey) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            http: self.http.clone(),
            key,
            tries: self.tries,
            backoff: self.backoff,
        }
    }

    #[must_use]
    pub fn key_id(&self) -> &str {
        self.key.id()
    }

    /// One API call. `path` is relative to the base (`/iam/v2/api-keys?…`) or
    /// a full URL on the same base (a page's `metadata.next`). 429 is retried
    /// after `Retry-After`, 5xx and transport errors with a doubling backoff.
    pub(crate) async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        what: &str,
    ) -> Result<Value, Error> {
        let url = if path.starts_with("http://") || path.starts_with("https://") {
            if !path.starts_with(&self.base) {
                return Err(Error::Decode {
                    what: what.into(),
                    message: "a next-page link points outside the API".into(),
                });
            }
            path.to_string()
        } else {
            format!("{}{path}", self.base)
        };
        let mut wait = self.backoff;
        let mut last_transport = String::new();
        for attempt in 1..=self.tries {
            let mut req = self
                .http
                .request(method.clone(), &url)
                .basic_auth(&self.key.id, Some(self.key.secret.expose()))
                .header("accept", "application/json");
            if let Some(b) = body {
                req = req.json(b);
            }
            let resp = match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    // reqwest's message names the URL, never the header.
                    last_transport = e.without_url().to_string();
                    if attempt < self.tries {
                        tokio::time::sleep(wait).await;
                        wait *= 2;
                        continue;
                    }
                    return Err(Error::Transport {
                        what: what.into(),
                        message: last_transport,
                    });
                }
            };
            let status = resp.status();
            if status == StatusCode::TOO_MANY_REQUESTS {
                if attempt == self.tries {
                    return Err(Error::RateLimited {
                        what: what.into(),
                        tries: self.tries,
                    });
                }
                let after = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .map_or(wait, Duration::from_secs);
                tokio::time::sleep(after.min(MAX_RETRY_AFTER)).await;
                wait *= 2;
                continue;
            }
            if status.is_server_error() && attempt < self.tries {
                tokio::time::sleep(wait).await;
                wait *= 2;
                continue;
            }
            let text = resp.text().await.unwrap_or_default();
            return match status {
                s if s.is_success() => {
                    if text.trim().is_empty() {
                        Ok(Value::Null)
                    } else {
                        serde_json::from_str(&text).map_err(|e| Error::Decode {
                            what: what.into(),
                            message: e.to_string(),
                        })
                    }
                }
                StatusCode::UNAUTHORIZED => Err(Error::Unauthorized),
                StatusCode::FORBIDDEN => Err(Error::Forbidden(what.into())),
                StatusCode::NOT_FOUND => Err(Error::NotFound(what.into())),
                s => Err(Error::Api {
                    status: s.as_u16(),
                    what: what.into(),
                    message: api_message(&text),
                }),
            };
        }
        Err(Error::Transport {
            what: what.into(),
            message: last_transport,
        })
    }
}

/// Confluent's error body (`{"errors":[{"detail": …}]}` or `{"error": …}`),
/// shortened. A request body is never echoed into it by this client.
fn api_message(text: &str) -> String {
    let v: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let msg = v["errors"][0]["detail"]
        .as_str()
        .or_else(|| v["errors"][0]["title"].as_str())
        .or_else(|| v["error"]["message"].as_str())
        .or_else(|| v["message"].as_str())
        .unwrap_or("no detail")
        .to_string();
    msg.chars().take(300).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_shows_the_id_only() {
        let k = CloudKey::new("CKEY1", Secret::new("cloud-secret-PLANTED"));
        let d = format!("{k:?}");
        assert!(d.contains("CKEY1") && !d.contains("PLANTED"), "{d}");
    }

    #[test]
    fn api_message_reads_confluent_errors() {
        assert_eq!(
            api_message(r#"{"errors":[{"status":"400","detail":"bad owner"}]}"#),
            "bad owner"
        );
        assert_eq!(api_message("not json"), "no detail");
    }
}
