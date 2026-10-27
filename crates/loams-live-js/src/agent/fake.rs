//! Scripted hosts for the spike's tests and measurements: a model proxy
//! that answers Chat Completions with one tool call (streamed or not), an
//! MCP shim with a `weather` server, an egress proxy that echoes and
//! records, and a fixed token. Nothing touches the network.

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::host::{AgentHosts, FetchRequest, FetchResponse, HostFuture};

/// The fake model's answer text, streamed word by word.
pub const MODEL_TEXT: &str = "Let me check the forecast for Paris.";

/// What the fakes saw, in order: `(kind, detail)`.
pub type Seen = Arc<Mutex<Vec<(String, Value)>>>;

/// The scripted hosts.
#[derive(Debug, Default, Clone)]
pub struct FakeHosts {
    pub seen: Seen,
}

impl FakeHosts {
    fn see(&self, kind: &str, detail: Value) {
        self.seen
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push((kind.to_string(), detail));
    }

    /// The kinds seen so far.
    pub fn kinds(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .map(|(k, _)| k.clone())
            .collect()
    }
}

fn completion(request: &Value) -> Value {
    // Offered tools and no tool result yet: the model calls the weather
    // tool. Otherwise it answers in words.
    let has_tool_result = request["messages"]
        .as_array()
        .is_some_and(|m| m.iter().any(|m| m["role"] == "tool"));
    let wants_tools = request["tools"].as_array().is_some_and(|t| !t.is_empty());
    let message = if has_tool_result || !wants_tools {
        json!({ "role": "assistant", "content": "It is sunny in Paris." })
    } else {
        json!({
            "role": "assistant",
            "content": MODEL_TEXT,
            "tool_calls": [{
                "id": "call-1",
                "type": "function",
                "function": { "name": "weather__get_forecast", "arguments": "{\"city\":\"Paris\"}" },
            }],
        })
    };
    json!({
        "id": "chatcmpl-fake",
        "object": "chat.completion",
        "created": 0,
        "model": "fake-model",
        "choices": [{ "index": 0, "message": message, "finish_reason": if has_tool_result || !wants_tools { "stop" } else { "tool_calls" } }],
        "usage": { "prompt_tokens": 12, "completion_tokens": 9, "total_tokens": 21 },
    })
}

impl AgentHosts for FakeHosts {
    fn fetch(&self, request: FetchRequest) -> HostFuture<FetchResponse> {
        self.see(
            "fetch",
            json!({ "url": request.url, "method": request.method, "bytes": request.body.len() }),
        );
        Box::pin(async move {
            // The model proxy's Chat Completions, reachable by `fetch` too
            // (what an AI SDK provider with a custom base URL calls).
            if request.url.ends_with("/chat/completions") {
                let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
                return Ok(FetchResponse {
                    status: 200,
                    status_text: "OK".into(),
                    headers: vec![("content-type".into(), "application/json".into())],
                    url: request.url,
                    body: completion(&body).to_string().into_bytes(),
                });
            }
            // `/bytes/<n>`: n bytes; anything else echoes the request.
            if let Some(n) = request
                .url
                .rsplit_once("/bytes/")
                .and_then(|(_, n)| n.parse::<usize>().ok())
            {
                return Ok(FetchResponse {
                    status: 200,
                    status_text: "OK".into(),
                    headers: vec![("content-type".into(), "application/octet-stream".into())],
                    url: request.url,
                    body: vec![b'x'; n],
                });
            }
            let echo = json!({
                "url": request.url,
                "method": request.method,
                "headers": request.headers,
                "bytes": request.body.len(),
            });
            Ok(FetchResponse {
                status: 200,
                status_text: "OK".into(),
                headers: vec![("content-type".into(), "application/json".into())],
                url: request.url,
                body: echo.to_string().into_bytes(),
            })
        })
    }

    fn model(
        &self,
        api: String,
        request: Value,
        stream: bool,
        events: mpsc::UnboundedSender<Value>,
    ) -> HostFuture<Value> {
        self.see("model", json!({ "api": api, "stream": stream }));
        Box::pin(async move {
            if request["hang"] == true {
                futures::future::pending::<()>().await;
            }
            if api != "chat" {
                return Err(format!("the fake model proxy serves only chat, not {api}"));
            }
            let response = completion(&request);
            if stream {
                let text = response["choices"][0]["message"]["content"]
                    .as_str()
                    .unwrap_or_default();
                for word in text.split_inclusive(' ') {
                    let _ = events
                        .send(json!({ "choices": [{ "index": 0, "delta": { "content": word } }] }));
                }
            }
            Ok(response)
        })
    }

    fn mcp_list(&self, server: String) -> HostFuture<Value> {
        self.see("mcp.list", json!({ "server": server }));
        Box::pin(async move {
            Ok(json!({ "tools": [
                { "name": "get_forecast", "description": "Forecast for a city" },
                { "name": "delete_city", "description": "An undeclared tool" },
            ] }))
        })
    }

    fn mcp_call(&self, server: String, tool: String, args: Value) -> HostFuture<Value> {
        self.see(
            "mcp.call",
            json!({ "server": server, "tool": tool, "args": args }),
        );
        Box::pin(async move {
            Ok(json!({
                "content": [{ "type": "text", "text": format!(
                    "{server}/{tool}: Sunny, 21°C in {}", args["city"].as_str().unwrap_or("?")
                ) }],
                "isError": false,
            }))
        })
    }

    fn token(&self) -> HostFuture<String> {
        self.see("token", Value::Null);
        Box::pin(async { Ok("place-token-fake".into()) })
    }
}

/// The manifest the fixtures declare.
pub fn fixture_manifest() -> super::host::Manifest {
    super::host::Manifest {
        egress_hosts: vec!["api.example.com".into(), "model-proxy.loams.local".into()],
        mcp_tools: vec!["weather/get_forecast".into()],
    }
}
