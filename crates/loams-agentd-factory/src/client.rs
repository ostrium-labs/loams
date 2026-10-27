//! `@loams-core/http`'s `UpstreamClient`, ported: a base URL, one auth shape, an
//! `Accept: application/json` header, a query encoded as `URLSearchParams` does it,
//! and a non-2xx answer as an error carrying its status.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::Value;

use crate::error::OpError;
use crate::upstream::{Upstream, UpstreamRequest, UpstreamResponse};

/// How a request authenticates.
#[derive(Clone)]
pub enum Auth {
    None,
    Bearer(String),
    Basic {
        username: String,
        password: String,
    },
    /// The whole `Authorization` value (Forgejo's `token <pat>`).
    Raw(String),
    /// Extra headers (ItsAPlan's `x-api-key`, OpenPanel's client id and secret).
    Headers(Vec<(String, String)>),
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self {
            Auth::None => "none",
            Auth::Bearer(_) => "bearer",
            Auth::Basic { .. } => "basic",
            Auth::Raw(_) => "authorization-raw",
            Auth::Headers(_) => "headers",
        };
        write!(f, "Auth({kind}, <redacted>)")
    }
}

/// A query parameter value: absent ones are dropped, as `buildQuery` does.
pub type Query<'a> = Vec<(&'a str, Option<String>)>;

/// One app's client.
#[derive(Debug, Clone)]
pub struct Client {
    base: String,
    auth: Auth,
    upstream: Arc<dyn Upstream>,
}

impl Client {
    /// `base` loses its trailing slashes.
    pub fn new(base: &str, auth: Auth, upstream: Arc<dyn Upstream>) -> Self {
        Self {
            base: base.trim_end_matches('/').to_owned(),
            auth,
            upstream,
        }
    }

    /// The same base and transport with another auth (Langfuse's unauthenticated
    /// health probe).
    pub fn with_auth(&self, auth: Auth) -> Self {
        Self {
            base: self.base.clone(),
            auth,
            upstream: Arc::clone(&self.upstream),
        }
    }

    /// The absolute URL for `path` and `query`.
    pub fn resolve(&self, path: &str, query: &Query<'_>) -> String {
        let suffix = if path.starts_with('/') || path.starts_with('?') {
            path.to_owned()
        } else {
            format!("/{path}")
        };
        format!("{}{suffix}{}", self.base, encode_query(query))
    }

    fn headers(&self) -> Vec<(String, String)> {
        let mut headers = vec![("accept".to_owned(), "application/json".to_owned())];
        match &self.auth {
            Auth::None => {}
            Auth::Bearer(token) => {
                headers.push(("authorization".into(), format!("Bearer {token}")))
            }
            Auth::Basic { username, password } => {
                let encoded = base64::engine::general_purpose::STANDARD
                    .encode(format!("{username}:{password}"));
                headers.push(("authorization".into(), format!("Basic {encoded}")));
            }
            Auth::Raw(value) => headers.push(("authorization".into(), value.clone())),
            Auth::Headers(extra) => {
                for (name, value) in extra {
                    headers.push((name.to_ascii_lowercase(), value.clone()));
                }
            }
        }
        headers
    }

    /// GET `path` and return the raw answer, whatever its status.
    ///
    /// # Errors
    ///
    /// A network failure (status 0) or a timeout.
    pub async fn get_raw(
        &self,
        path: &str,
        query: &Query<'_>,
    ) -> Result<UpstreamResponse, OpError> {
        let url = self.resolve(path, query);
        self.upstream
            .send(UpstreamRequest {
                method: "GET",
                url,
                headers: self.headers(),
            })
            .await
    }

    /// GET `path` and parse its JSON body (an empty body is `null`).
    ///
    /// # Errors
    ///
    /// A network failure, a non-2xx status (with the body's text), or a 2xx body
    /// that is not JSON.
    pub async fn get(&self, path: &str, query: &Query<'_>) -> Result<Value, OpError> {
        let response = self.get_raw(path, query).await?;
        json_of(path, &response)
    }
}

/// A response's JSON, or the error `UpstreamClient.request` throws for it.
///
/// # Errors
///
/// A non-2xx status, or a body that is not JSON.
pub fn json_of(path: &str, response: &UpstreamResponse) -> Result<Value, OpError> {
    let text = String::from_utf8_lossy(&response.body);
    if !(200..300).contains(&response.status) {
        return Err(OpError::http(
            response.status,
            format!(
                "GET {path} failed with {}: {}",
                response.status,
                truncate(&text, 500)
            ),
        ));
    }
    if text.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&text).map_err(|_| {
        OpError::http(
            response.status,
            format!("GET {path}: non-JSON body: {}", truncate(&text, 200)),
        )
    })
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// `URLSearchParams.toString()`: `application/x-www-form-urlencoded`, in order.
pub fn encode_query(query: &Query<'_>) -> String {
    let mut out = url::form_urlencoded::Serializer::new(String::new());
    let mut any = false;
    for (key, value) in query {
        if let Some(value) = value {
            out.append_pair(key, value);
            any = true;
        }
    }
    if any {
        format!("?{}", out.finish())
    } else {
        String::new()
    }
}

/// `encodeURIComponent`, for path segments.
pub fn encode_segment(segment: &str) -> String {
    const KEEP: &[u8] = b"-_.!~*'()";
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || KEEP.contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_drops_absent_values_and_encodes_like_url_search_params() {
        let q: Query<'_> = vec![
            ("q", Some("shop & co".into())),
            ("page", None),
            ("narrow", Some(r#"[{"operator":"channel"}]"#.into())),
        ];
        assert_eq!(
            encode_query(&q),
            "?q=shop+%26+co&narrow=%5B%7B%22operator%22%3A%22channel%22%7D%5D"
        );
        assert_eq!(encode_query(&vec![("a", None)]), "");
    }

    #[test]
    fn segments_encode_like_encode_uri_component() {
        assert_eq!(encode_segment("sfux/shop x"), "sfux%2Fshop%20x");
        assert_eq!(encode_segment("a-b_c.d~e"), "a-b_c.d~e");
    }

    #[test]
    fn auth_never_debug_prints_a_secret() {
        let auth = Auth::Bearer("tok-SECRET".into());
        assert!(!format!("{auth:?}").contains("SECRET"));
    }
}
