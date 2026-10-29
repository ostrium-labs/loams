//! A scripted HTTP server on loopback that answers recorded fixtures and
//! records the requests it gets. No test calls a real provider.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::Value;

/// One answer: status, extra headers, body.
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(&'static str, String)>,
    pub body: String,
}

impl Reply {
    pub fn ok(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    pub fn status(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    pub fn header(mut self, name: &'static str, value: &str) -> Self {
        self.headers.push((name, value.to_string()));
        self
    }
}

/// A request the server got.
#[derive(Debug, Clone)]
pub struct Seen {
    pub path: String,
    pub headers: HeaderMap,
    pub body: Value,
}

#[derive(Debug, Default)]
struct State {
    replies: VecDeque<Reply>,
    seen: Vec<Seen>,
}

/// The running server.
#[derive(Debug, Clone)]
pub struct MockServer {
    pub addr: SocketAddr,
    state: Arc<Mutex<State>>,
}

impl MockServer {
    /// Starts a server on `127.0.0.1` that answers `replies` in order (and
    /// 500 when they run out).
    pub async fn start(replies: Vec<Reply>) -> Self {
        let state = Arc::new(Mutex::new(State {
            replies: replies.into(),
            seen: Vec::new(),
        }));
        let s = state.clone();
        let app = Router::new().fallback(move |uri: Uri, headers: HeaderMap, body: Bytes| {
            let s = s.clone();
            async move {
                let mut st = s.lock().expect("lock");
                st.seen.push(Seen {
                    path: uri.path().to_string(),
                    headers,
                    body: serde_json::from_slice(&body).unwrap_or(Value::Null),
                });
                let reply = st
                    .replies
                    .pop_front()
                    .unwrap_or_else(|| Reply::status(500, "{}"));
                let mut resp: Response = (
                    StatusCode::from_u16(reply.status).expect("status"),
                    reply.body,
                )
                    .into_response();
                resp.headers_mut()
                    .insert("content-type", "application/json".parse().expect("header"));
                for (k, v) in reply.headers {
                    resp.headers_mut().insert(k, v.parse().expect("header"));
                }
                resp
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self { addr, state }
    }

    /// `http://127.0.0.1:<port><path>`.
    pub fn url(&self, path: &str) -> url::Url {
        url::Url::parse(&format!("http://{}{}", self.addr, path)).expect("url")
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.state.lock().expect("lock").seen.clone()
    }
}

/// A fixture file's text.
pub fn fixture(rel: &str) -> String {
    let p: PathBuf = [env!("CARGO_MANIFEST_DIR"), "fixtures", rel]
        .iter()
        .collect();
    std::fs::read_to_string(&p).unwrap_or_else(|_| panic!("fixture {}", p.display()))
}

/// The path of a fixture file.
pub fn fixture_path(rel: &str) -> PathBuf {
    [env!("CARGO_MANIFEST_DIR"), "fixtures", rel]
        .iter()
        .collect()
}

/// The schema the provider tests use.
pub fn person_schema() -> loams_llm::JsonSchema {
    loams_llm::JsonSchema::new(
        "person",
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "age": {"type": "integer", "minimum": 0},
                "tags": {"type": "array", "items": {"type": "string"}}
            },
            "required": ["name", "age"],
            "additionalProperties": false
        }),
    )
    .expect("schema")
}

pub fn person_request(text: &str) -> loams_llm::ChatRequest {
    loams_llm::ChatRequest {
        system: "Extract the person.".to_string(),
        messages: vec![loams_llm::ChatMessage::user(text)],
        max_output_tokens: 256,
        temperature: 0.0,
    }
}

/// Fast retries for tests.
pub fn fast_http() -> loams_llm::HttpOptions {
    loams_llm::HttpOptions {
        timeout: std::time::Duration::from_secs(10),
        retry: loams_llm::RetryPolicy {
            max_retries: 3,
            base_delay: std::time::Duration::from_millis(1),
            max_delay: std::time::Duration::from_millis(5),
            max_retry_after: std::time::Duration::from_secs(30),
        },
    }
}
