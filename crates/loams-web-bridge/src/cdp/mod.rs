//! The Chrome DevTools Protocol client: a transport seam and a thin client on
//! top of it, engine-neutral.
//!
//! The seam exists so a provider can be tested without a browser.
//! [`ws::WebSocketTransport`] is the real one; the conformance suite and the
//! unit tests use a scripted transport (`crate::testing`).

pub mod ax;
pub mod budget;
pub mod ws;

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::BridgeError;

/// The headers of the WebSocket handshake.
///
/// `Debug` is written by hand: a header may carry a bearer token, and a `Debug`
/// of a header list must never be able to print it.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct RequestHeaders(Vec<(String, String)>);

impl RequestHeaders {
    /// No headers.
    pub fn none() -> Self {
        Self::default()
    }

    /// Add a header.
    pub fn with(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.0.push((name.into(), value.into()));
        self
    }

    /// The headers as pairs, for a transport that wants them.
    pub fn as_pairs(&self) -> &[(String, String)] {
        &self.0
    }

    /// Whether a header is present.
    pub fn contains(&self, name: &str) -> bool {
        self.0.iter().any(|(key, _)| key.eq_ignore_ascii_case(name))
    }

    /// A header's value, for a test that has to check what was sent.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

impl std::fmt::Debug for RequestHeaders {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut map = f.debug_map();
        for (name, _) in &self.0 {
            map.entry(&name, &crate::redact::REDACTED);
        }
        map.finish()
    }
}

/// One CDP request.
#[derive(Debug, Serialize)]
pub struct CdpRequest {
    /// The message id, matched to the response.
    pub id: u64,
    /// The method, for example `Page.navigate`.
    pub method: String,
    /// The parameters.
    #[serde(default)]
    pub params: Value,
    /// The attached target's session, when the call is page-scoped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

impl CdpRequest {
    /// A browser-scoped call.
    pub fn browser(id: u64, method: &str, params: Value) -> Self {
        Self {
            id,
            method: method.to_string(),
            params,
            session_id: None,
        }
    }

    /// A page-scoped call.
    pub fn page(id: u64, session_id: &str, method: &str, params: Value) -> Self {
        Self {
            id,
            method: method.to_string(),
            params,
            session_id: Some(session_id.to_string()),
        }
    }
}

/// One CDP response.
#[derive(Debug, Deserialize)]
pub struct CdpResponse {
    /// The id of the request this answers.
    #[serde(default)]
    pub id: Option<u64>,
    /// The result, when the call succeeded.
    #[serde(default)]
    pub result: Option<Value>,
    /// The error, when it did not.
    #[serde(default)]
    pub error: Option<CdpErrorBody>,
}

/// The `error` member of a failed CDP response.
#[derive(Debug, Clone, Deserialize)]
pub struct CdpErrorBody {
    /// The protocol error code.
    #[serde(default)]
    pub code: i64,
    /// The message the browser sent.
    #[serde(default)]
    pub message: String,
}

impl std::fmt::Display for CdpErrorBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "code {}: {}", self.code, self.message)
    }
}

/// One unsolicited CDP message: an event such as `Fetch.requestPaused`.
#[derive(Debug, Clone, PartialEq)]
pub struct CdpEvent {
    /// The event's method, for example `Fetch.requestPaused`.
    pub method: String,
    /// Its parameters.
    pub params: Value,
    /// The attached target's session, when the event is page-scoped.
    pub session_id: Option<String>,
}

impl CdpEvent {
    /// Read an event from a decoded message, if it is one (it has a `method`
    /// and no `id`).
    pub fn from_message(message: &Value) -> Option<Self> {
        if message.get("id").is_some_and(|id| !id.is_null()) {
            return None;
        }
        let method = message.get("method")?.as_str()?.to_string();
        Some(Self {
            method,
            params: message.get("params").cloned().unwrap_or(Value::Null),
            session_id: message
                .get("sessionId")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }
}

/// An open CDP connection.
#[async_trait::async_trait]
pub trait CdpConnection: Send + Sync + std::fmt::Debug {
    /// Send a command and wait for its result.
    async fn send(&self, request: CdpRequest) -> Result<Value, BridgeError>;

    /// The connection's event stream, handed out once.
    ///
    /// A connection that cannot deliver events answers `None`, and a provider
    /// then does without the features that need them (request interception),
    /// relying on its checks before and after each navigation instead.
    fn take_events(&self) -> Option<tokio::sync::mpsc::UnboundedReceiver<CdpEvent>> {
        None
    }

    /// Close the socket. Closing twice is not an error.
    async fn close(&self) -> Result<(), BridgeError>;
}

/// How a provider gets a connection.
#[async_trait]
pub trait CdpTransport: Send + Sync + std::fmt::Debug {
    /// Open `endpoint` with `headers` and speak CDP on it.
    async fn connect(
        &self,
        endpoint: &str,
        headers: &RequestHeaders,
    ) -> Result<Arc<dyn CdpConnection>, BridgeError>;
}

/// A CDP client over a connection: ids, timeouts and the calls the provider
/// needs.
#[derive(Debug)]
pub struct CdpClient {
    connection: Arc<dyn CdpConnection>,
    next_id: std::sync::atomic::AtomicU64,
    timeout: std::time::Duration,
}

impl CdpClient {
    /// A client that gives up on a call after `timeout`.
    pub fn new(connection: Arc<dyn CdpConnection>, timeout: std::time::Duration) -> Self {
        Self {
            connection,
            next_id: std::sync::atomic::AtomicU64::new(1),
            timeout,
        }
    }

    /// A browser-scoped call.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, BridgeError> {
        let id = self.next_id();
        self.dispatch(CdpRequest::browser(id, method, params)).await
    }

    /// A page-scoped call.
    pub async fn call_page(
        &self,
        session_id: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, BridgeError> {
        let id = self.next_id();
        self.dispatch(CdpRequest::page(id, session_id, method, params))
            .await
    }

    /// Close the connection.
    pub async fn close(&self) -> Result<(), BridgeError> {
        self.connection.close().await
    }

    /// The connection's event stream, if it has one and nobody took it.
    pub fn take_events(&self) -> Option<tokio::sync::mpsc::UnboundedReceiver<CdpEvent>> {
        self.connection.take_events()
    }

    async fn dispatch(&self, request: CdpRequest) -> Result<Value, BridgeError> {
        let method = request.method.clone();
        match tokio::time::timeout(self.timeout, self.connection.send(request)).await {
            Ok(result) => result,
            Err(_) => Err(BridgeError::Timeout(
                u64::try_from(self.timeout.as_millis()).unwrap_or(u64::MAX),
            )),
        }
        .map_err(|error| {
            // Re-tag a protocol error with the method that produced it, which
            // is what makes an error actionable, and scrub the message.
            match error {
                BridgeError::Cdp(message) => BridgeError::cdp(format!("{method}: {message}")),
                other => other,
            }
        })
    }

    fn next_id(&self) -> u64 {
        self.next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }
}
