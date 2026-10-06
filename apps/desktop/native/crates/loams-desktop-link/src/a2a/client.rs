//! The A2A client trait, a JSON-RPC implementation and a mock.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::types::{
    Artifact, Message, Part, Role, SendMessageResponse, Task, TaskState, TaskStatus,
};

/// A2A client failures.
#[derive(Debug, thiserror::Error)]
pub enum A2aError {
    /// Could not reach the agent.
    #[error("network error: {0}")]
    Network(String),
    /// The agent returned a JSON-RPC error.
    #[error("agent error {code}: {message}")]
    Rpc {
        /// The JSON-RPC error code.
        code: i64,
        /// The agent's message.
        message: String,
    },
    /// The response was not what A2A promises.
    #[error("malformed response: {0}")]
    Malformed(String),
}

/// What the chat needs from an A2A peer.
#[async_trait]
pub trait A2aClient: Send + Sync {
    /// `SendMessage`.
    async fn send_message(&self, message: Message) -> Result<SendMessageResponse, A2aError>;
    /// `CancelTask`.
    async fn cancel_task(&self, task_id: &str) -> Result<(), A2aError>;
}

/// A2A over JSON-RPC 2.0 on HTTP. The bearer is the user's attenuated,
/// audience-bound token (design 39 section 6); a stub caller may pass none.
#[derive(Debug, Clone)]
pub struct JsonRpcA2aClient {
    http: reqwest::Client,
    url: String,
    bearer: Option<String>,
    next_id: std::sync::Arc<AtomicU64>,
}

impl JsonRpcA2aClient {
    /// A client for an agent's JSON-RPC endpoint.
    #[must_use]
    pub fn new(url: impl Into<String>, bearer: Option<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .expect("static reqwest configuration is valid");
        Self {
            http,
            url: url.into(),
            bearer,
            next_id: std::sync::Arc::default(),
        }
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, A2aError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let mut request = self
            .http
            .post(&self.url)
            .json(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        if let Some(token) = &self.bearer {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .map_err(|e| A2aError::Network(e.to_string()))?;
        let status = response.status();
        let body: Value = response
            .json()
            .await
            .map_err(|e| A2aError::Malformed(format!("HTTP {status}: {e}")))?;
        if let Some(error) = body.get("error") {
            return Err(A2aError::Rpc {
                code: error.get("code").and_then(Value::as_i64).unwrap_or(-32000),
                message: error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            });
        }
        body.get("result")
            .cloned()
            .ok_or_else(|| A2aError::Malformed("no result".into()))
    }
}

#[async_trait]
impl A2aClient for JsonRpcA2aClient {
    async fn send_message(&self, message: Message) -> Result<SendMessageResponse, A2aError> {
        let result = self
            .call("SendMessage", json!({ "message": message }))
            .await?;
        serde_json::from_value(result).map_err(|e| A2aError::Malformed(e.to_string()))
    }

    async fn cancel_task(&self, task_id: &str) -> Result<(), A2aError> {
        self.call("CancelTask", json!({ "id": task_id }))
            .await
            .map(|_| ())
    }
}

/// A canned agent for `LOAMS_MOCK=1`, tests and demos. It completes every
/// message with an echo, except `/ask`, which parks in `INPUT_REQUIRED` with a
/// question, as an agent waiting for an approval would (D467, D468).
#[derive(Debug, Default)]
pub struct MockA2aClient {
    counter: AtomicU64,
}

#[async_trait]
impl A2aClient for MockA2aClient {
    async fn send_message(&self, message: Message) -> Result<SendMessageResponse, A2aError> {
        let n = self.counter.fetch_add(1, Ordering::Relaxed) + 1;
        let said: String = message
            .parts
            .iter()
            .filter_map(|p| p.text.as_deref())
            .collect();
        let context_id = message
            .context_id
            .clone()
            .unwrap_or_else(|| "ctx-mock".into());
        let agent_message = |text: String| Message {
            message_id: format!("mock-{n}"),
            context_id: Some(context_id.clone()),
            task_id: None,
            role: Role::Agent,
            parts: vec![Part::text(text)],
        };
        let (state, status_message, artifacts) = if said.trim_start().starts_with("/ask") {
            (
                TaskState::InputRequired,
                Some(agent_message(
                    "Approve creating the issue in project CHECKOUT?".into(),
                )),
                Vec::new(),
            )
        } else {
            (
                TaskState::Completed,
                None,
                vec![Artifact {
                    artifact_id: format!("mock-artifact-{n}"),
                    name: Some("reply".into()),
                    parts: vec![Part::text(format!("[mock agent] you said: {said}"))],
                }],
            )
        };
        Ok(SendMessageResponse::Task(Task {
            id: format!("op-mock-{n}"),
            context_id,
            status: TaskStatus {
                state,
                message: status_message,
            },
            artifacts,
        }))
    }

    async fn cancel_task(&self, _task_id: &str) -> Result<(), A2aError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::Reply;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;

    fn user(text: &str) -> Message {
        Message {
            message_id: "m1".into(),
            context_id: None,
            task_id: None,
            role: Role::User,
            parts: vec![Part::text(text)],
        }
    }

    #[tokio::test]
    async fn mock_completes_with_an_echo_and_parks_on_ask() {
        let mock = MockA2aClient::default();
        let done = Reply::from_response(&mock.send_message(user("hello")).await.unwrap());
        assert_eq!(done.state, TaskState::Completed);
        assert!(done.text.contains("you said: hello"));
        let parked = Reply::from_response(&mock.send_message(user("/ask plane")).await.unwrap());
        assert_eq!(parked.state, TaskState::InputRequired);
        assert!(parked.text.contains("Approve"));
    }

    /// One canned HTTP reply; returns the request body it saw.
    async fn serve_once(reply: &'static str) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/a2a", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0_u8; 4096];
            loop {
                let n = s.read(&mut chunk).await.unwrap();
                buf.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&buf).into_owned();
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let want: usize = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse().unwrap())
                        })
                        .unwrap_or(0);
                    if body.len() >= want {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&buf).into_owned();
            let seen = text
                .split_once("\r\n\r\n")
                .map(|(_, b)| b.to_owned())
                .unwrap_or_default();
            let out = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                reply.len()
            );
            s.write_all(out.as_bytes()).await.unwrap();
            let _ = s.shutdown().await;
            seen
        });
        (url, task)
    }

    #[tokio::test]
    async fn json_rpc_send_message_round_trips() {
        let (url, seen) = serve_once(
            r#"{"jsonrpc":"2.0","id":1,"result":{"task":{"id":"op-9","contextId":"c","status":{"state":"TASK_STATE_WORKING"}}}}"#,
        )
        .await;
        let client = JsonRpcA2aClient::new(url, Some("tok".into()));
        let reply = Reply::from_response(&client.send_message(user("hi")).await.unwrap());
        assert_eq!(reply.state, TaskState::Working);
        assert_eq!(reply.task_id.as_deref(), Some("op-9"));
        let request: Value = serde_json::from_str(&seen.await.unwrap()).unwrap();
        assert_eq!(request["method"], "SendMessage");
        assert_eq!(request["params"]["message"]["parts"][0]["text"], "hi");
    }

    #[tokio::test]
    async fn json_rpc_errors_surface() {
        let (url, _seen) = serve_once(
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32001,"message":"task not found"}}"#,
        )
        .await;
        let err = JsonRpcA2aClient::new(url, None)
            .send_message(user("x"))
            .await
            .unwrap_err();
        assert!(matches!(err, A2aError::Rpc { code: -32001, .. }));
    }
}
