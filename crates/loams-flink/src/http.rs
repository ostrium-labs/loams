//! The JSON-over-HTTP plumbing both Flink clients share.

use std::time::Duration;

use reqwest::{Method, Url};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{FlinkError, MAX_ERROR_LEN, shorten};

/// The default per-request timeout.
pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// A base URL and a client.
#[derive(Debug, Clone)]
pub(crate) struct Http {
    client: reqwest::Client,
    /// Always ends with `/`, so [`Url::join`] appends.
    base: Url,
    bearer: Option<String>,
}

impl Http {
    pub(crate) fn new(base: &str, client: reqwest::Client) -> Result<Self, FlinkError> {
        let mut base =
            Url::parse(base).map_err(|e| FlinkError::Invalid(format!("base URL {base}: {e}")))?;
        if !matches!(base.scheme(), "http" | "https") {
            return Err(FlinkError::Invalid(format!(
                "base URL {base}: only http and https"
            )));
        }
        if base.query().is_some() || base.fragment().is_some() {
            return Err(FlinkError::Invalid(format!(
                "base URL {base}: no query or fragment"
            )));
        }
        if !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }
        Ok(Self {
            client,
            base,
            bearer: None,
        })
    }

    /// A client with this crate's defaults (timeout, no proxy from the
    /// environment: the SQL Gateway is on a private network).
    pub(crate) fn default_client() -> Result<reqwest::Client, FlinkError> {
        reqwest::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .no_proxy()
            .build()
            .map_err(|e| FlinkError::Invalid(format!("HTTP client: {e}")))
    }

    /// Sends `authorization: Bearer <token>` on every request (a gateway in
    /// front of Flink; Flink itself has no auth).
    pub(crate) fn set_bearer(&mut self, token: Option<String>) {
        self.bearer = token;
    }

    pub(crate) fn base(&self) -> &Url {
        &self.base
    }

    /// `path` (relative, no leading `/`) under the base URL.
    pub(crate) fn url(&self, path: &str) -> Result<Url, FlinkError> {
        let path = path.trim_start_matches('/');
        self.base
            .join(path)
            .map_err(|e| FlinkError::Invalid(format!("path {path}: {e}")))
    }

    pub(crate) async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, FlinkError> {
        self.call::<(), T>(Method::GET, self.url(path)?, None).await
    }

    pub(crate) async fn post<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, FlinkError> {
        self.call(Method::POST, self.url(path)?, Some(body)).await
    }

    pub(crate) async fn delete<T: DeserializeOwned>(&self, path: &str) -> Result<T, FlinkError> {
        self.call::<(), T>(Method::DELETE, self.url(path)?, None)
            .await
    }

    /// One request: `body` as JSON when given, the answer decoded as `T`.
    /// An empty 2xx body decodes as `{}`.
    pub(crate) async fn call<B: Serialize, T: DeserializeOwned>(
        &self,
        method: Method,
        url: Url,
        body: Option<&B>,
    ) -> Result<T, FlinkError> {
        let shown = url.to_string();
        let mut req = self
            .client
            .request(method, url)
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(token) = &self.bearer {
            req = req.bearer_auth(token);
        }
        if let Some(body) = body {
            req = req.json(body);
        }
        let resp = req.send().await.map_err(|source| FlinkError::Transport {
            url: shown.clone(),
            source,
        })?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|source| FlinkError::Transport {
            url: shown.clone(),
            source,
        })?;
        if !status.is_success() {
            return Err(api_error(&shown, status.as_u16(), &bytes));
        }
        let text: &[u8] = if bytes.iter().all(u8::is_ascii_whitespace) {
            b"{}"
        } else {
            &bytes
        };
        serde_json::from_slice(text).map_err(|e| FlinkError::Decode {
            url: shown,
            message: format!("{e}: {}", shorten(&String::from_utf8_lossy(text), 512)),
        })
    }
}

/// Flink's error body, `{"errors": [...]}`, or the raw text.
pub(crate) fn api_error(url: &str, status: u16, body: &[u8]) -> FlinkError {
    #[derive(serde::Deserialize)]
    struct Errors {
        #[serde(default)]
        errors: Vec<String>,
    }
    let errors: Vec<String> = match serde_json::from_slice::<Errors>(body) {
        Ok(e) if !e.errors.is_empty() => e.errors,
        _ => {
            let text = String::from_utf8_lossy(body).trim().to_owned();
            if text.is_empty() {
                Vec::new()
            } else {
                vec![text]
            }
        }
    };
    let errors: Vec<String> = errors.iter().map(|e| shorten(e, MAX_ERROR_LEN)).collect();
    let message = if errors.is_empty() {
        "no error message".to_owned()
    } else {
        // The first line of each error is the message; the rest is a trace.
        errors
            .iter()
            .map(|e| e.lines().next().unwrap_or_default())
            .collect::<Vec<_>>()
            .join("; ")
    };
    FlinkError::Api {
        url: url.to_owned(),
        status,
        errors,
        message,
    }
}

/// An empty JSON object answer (`{}`), and anything else ignored.
#[derive(Debug, Clone, Copy, Default, serde::Deserialize)]
pub(crate) struct Ignored {}

/// Checks a path segment Flink generated (a session or operation handle, a
/// job id, a trigger id): ASCII letters, digits and `-` only, 1 to 128
/// bytes, so it can never change the path it is put in.
pub(crate) fn check_segment(what: &str, value: &str) -> Result<(), FlinkError> {
    let ok = !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(FlinkError::Invalid(format!(
            "{what} must be 1-128 ASCII letters, digits or '-'"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_gets_a_trailing_slash_and_joins_relative_paths() {
        let http = Http::new("http://gw:8083/flink/t1", reqwest::Client::new()).unwrap();
        assert_eq!(
            http.url("/v3/sessions").unwrap().as_str(),
            "http://gw:8083/flink/t1/v3/sessions"
        );
        assert!(Http::new("ftp://x", reqwest::Client::new()).is_err());
        assert!(Http::new("http://x/?a=b", reqwest::Client::new()).is_err());
    }

    #[test]
    fn api_errors_keep_flink_messages_first_lines() {
        let body = br#"{"errors":["org.apache.flink.SqlException: boom\n\tat x.y","second"]}"#;
        let e = api_error("http://x", 500, body);
        match &e {
            FlinkError::Api {
                status,
                errors,
                message,
                ..
            } => {
                assert_eq!(*status, 500);
                assert_eq!(errors.len(), 2);
                assert_eq!(message, "org.apache.flink.SqlException: boom; second");
            }
            other => panic!("{other:?}"),
        }
        let raw = api_error("http://x", 502, b"bad gateway");
        assert!(raw.to_string().contains("bad gateway"));
        assert!(api_error("http://x", 404, b"").is_not_found());
    }

    #[test]
    fn segments_cannot_escape_the_path() {
        assert!(check_segment("h", "4c1c5d8e-1a2b-4c3d-8e9f-0a1b2c3d4e5f").is_ok());
        assert!(check_segment("h", "../jobs").is_err());
        assert!(check_segment("h", "a/b").is_err());
        assert!(check_segment("h", "").is_err());
        assert!(check_segment("h", "a?b").is_err());
    }
}
