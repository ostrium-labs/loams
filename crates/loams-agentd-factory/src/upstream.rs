//! The one door to the network: an HTTP GET with headers. The ops are written
//! against [`Upstream`] so the parity suite can replay a scripted upstream.

use std::time::Duration;

use async_trait::async_trait;

use crate::error::OpError;

/// A request the ops send. Every op is a read, so there is no body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamRequest {
    pub method: &'static str,
    /// Absolute, with its query.
    pub url: String,
    /// Names are lower-case.
    pub headers: Vec<(String, String)>,
}

/// What came back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamResponse {
    pub status: u16,
    /// Names are lower-case.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl UpstreamResponse {
    /// A header's value by name (any case).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Sends a request. An `Err` is a network failure (no HTTP answer at all).
#[async_trait]
pub trait Upstream: Send + Sync + std::fmt::Debug {
    async fn send(&self, request: UpstreamRequest) -> Result<UpstreamResponse, OpError>;
}

/// The daemon's HTTP client: rustls with webpki's and the OS's roots, up to five
/// redirects followed (as `fetch` does), 15 s per request.
#[derive(Debug, Clone)]
pub struct HttpUpstream {
    client: reqwest::Client,
}

/// The longest a single upstream request may take.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

impl HttpUpstream {
    /// # Errors
    ///
    /// The TLS backend could not be set up.
    pub fn new() -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::limited(5))
            .user_agent(concat!("loams-agentd-factory/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { client })
    }
}

#[async_trait]
impl Upstream for HttpUpstream {
    async fn send(&self, request: UpstreamRequest) -> Result<UpstreamResponse, OpError> {
        let method = reqwest::Method::from_bytes(request.method.as_bytes())
            .map_err(|e| OpError::other(e.to_string()))?;
        let mut builder = self.client.request(method, &request.url);
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        let response = builder.send().await.map_err(|e| {
            if e.is_timeout() {
                OpError::timeout()
            } else {
                OpError::network(e.without_url().to_string())
            }
        })?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(n, v)| {
                v.to_str()
                    .ok()
                    .map(|v| (n.as_str().to_ascii_lowercase(), v.to_owned()))
            })
            .collect();
        let body = response
            .bytes()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    OpError::timeout()
                } else {
                    OpError::network(e.without_url().to_string())
                }
            })?
            .to_vec();
        Ok(UpstreamResponse {
            status,
            headers,
            body,
        })
    }
}
