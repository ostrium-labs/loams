//! Loams Bot as an Agent Client Protocol agent.
//!
//! Loams Desktop already drives agents over ACP (JSON-RPC 2.0, newline-delimited, on
//! stdio): the engine spawns the agent, owns the session, and renders
//! `session/update` notifications as transcript, tool cards and questions.
//! Making Loams Bot an ACP agent therefore gives it loams-desktop's whole chat surface
//! (threads, sync-free local persistence, queueing, steering, attachments)
//! without a single line of UI code. This module is that agent: it answers
//! `initialize`, `session/new`, `session/prompt` and `session/cancel`, and
//! turns each prompt into an A2A `SendMessage` (design 37 section 21).
//!
//! An A2A task is shown as an ACP tool call, so the transcript renders it as
//! a card with live status, like any sub-agent. `INPUT_REQUIRED` is relayed
//! as text and never answered here (design 37 section 18).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::sync::{Notify, mpsc};

use crate::a2a::{A2aClient, Message, Part, Reply, Role, TaskState};

/// The ACP protocol version loams-desktop speaks.
const PROTOCOL_VERSION: u64 = 1;

#[derive(Default)]
struct Session {
    context_id: String,
    cancel: Option<Arc<Notify>>,
}

type Sessions = Arc<Mutex<HashMap<String, Session>>>;

/// Serves ACP on `reader` / `writer` until the reader closes.
///
/// # Errors
///
/// I/O errors reading the request stream.
pub async fn serve<R, W>(reader: R, writer: W, a2a: Arc<dyn A2aClient>) -> std::io::Result<()>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (out, mut rx) = mpsc::unbounded_channel::<Value>();
    let writer_task = tokio::spawn(async move {
        let mut writer = writer;
        while let Some(message) = rx.recv().await {
            let mut line = message.to_string();
            line.push('\n');
            if writer.write_all(line.as_bytes()).await.is_err() || writer.flush().await.is_err() {
                break;
            }
        }
    });

    let sessions: Sessions = Arc::default();
    let mut counter = 0_u64;
    let mut lines = reader.lines();
    let mut prompts = Vec::new();
    while let Some(line) = lines.next_line().await? {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            tracing::debug!("loams-bot acp: ignoring a non-JSON line");
            continue;
        };
        let id = message.get("id").cloned();
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            continue; // a response to a request we never send
        };
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        match (method, id) {
            ("initialize", Some(id)) => reply(&out, &id, initialize_result()),
            ("session/new", Some(id)) => {
                counter += 1;
                let session_id = format!("loams-bot-{counter}");
                sessions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(
                        session_id.clone(),
                        Session {
                            context_id: format!("ctx-{session_id}"),
                            cancel: None,
                        },
                    );
                reply(&out, &id, json!({ "sessionId": session_id }));
            }
            ("session/prompt", Some(id)) => {
                let (out, sessions, a2a) = (out.clone(), sessions.clone(), a2a.clone());
                prompts.push(tokio::spawn(async move {
                    prompt(&out, &sessions, a2a.as_ref(), id, &params).await;
                }));
            }
            ("session/cancel", None) => {
                let session_id = params
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let cancel = sessions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(session_id)
                    .and_then(|s| s.cancel.clone());
                if let Some(cancel) = cancel {
                    cancel.notify_one();
                }
            }
            (_, Some(id)) => fail(&out, &id, -32601, &format!("method not found: {method}")),
            (_, None) => {}
        }
    }
    // The engine closes our stdin when it is done with us; a piped caller
    // (the smoke test, a shell) closes it right after its last request. Let
    // in-flight prompts answer, bounded, then stop.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    for task in prompts {
        if tokio::time::timeout_at(deadline, task).await.is_err() {
            tracing::warn!("loams-bot acp: a prompt was still running at shutdown");
        }
    }
    drop(out);
    let _ = writer_task.await;
    Ok(())
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "agentCapabilities": {
            "loadSession": false,
            "promptCapabilities": { "image": false, "audio": false, "embeddedContext": false },
        },
        "agentInfo": { "name": "loams-bot", "title": crate::brand::BOT_NAME, "version": env!("CARGO_PKG_VERSION") },
        "authMethods": [],
    })
}

async fn prompt(
    out: &mpsc::UnboundedSender<Value>,
    sessions: &Sessions,
    a2a: &dyn A2aClient,
    id: Value,
    params: &Value,
) {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let cancel = Arc::new(Notify::new());
    let context_id = {
        let mut guard = sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(session) = guard.get_mut(&session_id) else {
            return fail(out, &id, -32602, "unknown session");
        };
        session.cancel = Some(cancel.clone());
        session.context_id.clone()
    };
    let text = prompt_text(params);
    if text.trim().is_empty() {
        return fail(out, &id, -32602, "empty prompt");
    }

    let call_id = format!("a2a-{}", crate::auth::pkce::random_token(6));
    update(
        out,
        &session_id,
        json!({
            "sessionUpdate": "tool_call",
            "toolCallId": call_id,
            "title": "Ask the Loams agents",
            "kind": "other",
            "status": "in_progress",
        }),
    );

    let message = Message {
        message_id: format!("{call_id}-m"),
        context_id: Some(context_id),
        task_id: None,
        role: Role::User,
        parts: vec![Part::text(text)],
    };
    let outcome = tokio::select! {
        r = a2a.send_message(message) => Some(r),
        () = cancel.notified() => None,
    };
    if let Some(session) = sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_mut(&session_id)
    {
        session.cancel = None;
    }

    match outcome {
        None => {
            update(
                out,
                &session_id,
                json!({
                    "sessionUpdate": "tool_call_update", "toolCallId": call_id, "status": "failed",
                }),
            );
            reply(out, &id, json!({ "stopReason": "cancelled" }));
        }
        Some(Err(error)) => {
            update(
                out,
                &session_id,
                json!({
                    "sessionUpdate": "tool_call_update", "toolCallId": call_id, "status": "failed",
                }),
            );
            update(
                out,
                &session_id,
                chunk(&format!("The Loams agents could not be reached: {error}")),
            );
            reply(out, &id, json!({ "stopReason": "refusal" }));
        }
        Some(Ok(response)) => {
            let reply_text = Reply::from_response(&response);
            let status = match reply_text.state {
                TaskState::Failed | TaskState::Rejected | TaskState::Canceled => "failed",
                TaskState::Completed => "completed",
                _ => "in_progress",
            };
            update(
                out,
                &session_id,
                json!({
                    "sessionUpdate": "tool_call_update", "toolCallId": call_id, "status": status,
                }),
            );
            let shown = match reply_text.state {
                TaskState::InputRequired => format!("Waiting for you: {}", reply_text.text),
                TaskState::AuthRequired => {
                    format!("An agent's app account is not linked. {}", reply_text.text)
                }
                _ => reply_text.text,
            };
            if !shown.is_empty() {
                update(out, &session_id, chunk(&shown));
            }
            reply(out, &id, json!({ "stopReason": "end_turn" }));
        }
    }
}

fn prompt_text(params: &Value) -> String {
    params
        .get("prompt")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn chunk(text: &str) -> Value {
    json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": text } })
}

fn update(out: &mpsc::UnboundedSender<Value>, session_id: &str, update: Value) {
    let _ = out.send(json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": { "sessionId": session_id, "update": update },
    }));
}

fn reply(out: &mpsc::UnboundedSender<Value>, id: &Value, result: Value) {
    let _ = out.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
}

fn fail(out: &mpsc::UnboundedSender<Value>, id: &Value, code: i64, message: &str) {
    let _ = out
        .send(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::{A2aError, MockA2aClient, SendMessageResponse};
    use async_trait::async_trait;
    use tokio::io::{BufReader, duplex};

    struct Peer {
        to_agent: tokio::io::DuplexStream,
        from_agent: tokio::io::Lines<BufReader<tokio::io::DuplexStream>>,
    }

    fn start(a2a: Arc<dyn A2aClient>) -> Peer {
        let (to_agent, agent_in) = duplex(64 * 1024);
        let (agent_out, from_agent) = duplex(64 * 1024);
        tokio::spawn(async move {
            let _ = serve(BufReader::new(agent_in), agent_out, a2a).await;
        });
        Peer {
            to_agent,
            from_agent: BufReader::new(from_agent).lines(),
        }
    }

    impl Peer {
        async fn send(&mut self, value: Value) {
            self.to_agent
                .write_all(format!("{value}\n").as_bytes())
                .await
                .unwrap();
        }
        async fn next(&mut self) -> Value {
            let line = self
                .from_agent
                .next_line()
                .await
                .unwrap()
                .expect("agent closed");
            serde_json::from_str(&line).unwrap()
        }
        /// Reads until the response with `id`, returning it and every update seen first.
        async fn until_response(&mut self, id: u64) -> (Value, Vec<Value>) {
            let mut updates = Vec::new();
            loop {
                let m = self.next().await;
                if m.get("id") == Some(&json!(id)) {
                    return (m, updates);
                }
                updates.push(m["params"]["update"].clone());
            }
        }
    }

    async fn open_session(peer: &mut Peer) -> String {
        peer.send(
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}),
        )
        .await;
        let (init, _) = peer.until_response(1).await;
        assert_eq!(init["result"]["protocolVersion"], 1);
        assert_eq!(init["result"]["agentInfo"]["title"], "Loams Bot");
        peer.send(json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/","mcpServers":[]}})).await;
        let (new, _) = peer.until_response(2).await;
        new["result"]["sessionId"].as_str().unwrap().to_owned()
    }

    #[tokio::test]
    async fn a_prompt_becomes_a_tool_card_and_a_reply() {
        let mut peer = start(Arc::new(MockA2aClient::default()));
        let sid = open_session(&mut peer).await;
        peer.send(json!({"jsonrpc":"2.0","id":3,"method":"session/prompt",
            "params":{"sessionId":sid,"prompt":[{"type":"text","text":"file an issue"}]}}))
            .await;
        let (done, updates) = peer.until_response(3).await;
        assert_eq!(done["result"]["stopReason"], "end_turn");
        let kinds: Vec<_> = updates
            .iter()
            .map(|u| u["sessionUpdate"].as_str().unwrap())
            .collect();
        assert_eq!(
            kinds,
            ["tool_call", "tool_call_update", "agent_message_chunk"]
        );
        assert_eq!(updates[1]["status"], "completed");
        assert!(
            updates[2]["content"]["text"]
                .as_str()
                .unwrap()
                .contains("you said: file an issue")
        );
    }

    #[tokio::test]
    async fn input_required_is_relayed_not_answered() {
        let mut peer = start(Arc::new(MockA2aClient::default()));
        let sid = open_session(&mut peer).await;
        peer.send(json!({"jsonrpc":"2.0","id":3,"method":"session/prompt",
            "params":{"sessionId":sid,"prompt":[{"type":"text","text":"/ask plane"}]}}))
            .await;
        let (done, updates) = peer.until_response(3).await;
        assert_eq!(done["result"]["stopReason"], "end_turn");
        assert_eq!(updates[1]["status"], "in_progress");
        assert!(
            updates[2]["content"]["text"]
                .as_str()
                .unwrap()
                .starts_with("Waiting for you:")
        );
    }

    struct Hangs;
    #[async_trait]
    impl A2aClient for Hangs {
        async fn send_message(&self, _m: Message) -> Result<SendMessageResponse, A2aError> {
            std::future::pending().await
        }
        async fn cancel_task(&self, _id: &str) -> Result<(), A2aError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn cancel_ends_the_turn_as_cancelled() {
        let mut peer = start(Arc::new(Hangs));
        let sid = open_session(&mut peer).await;
        peer.send(json!({"jsonrpc":"2.0","id":3,"method":"session/prompt",
            "params":{"sessionId":sid,"prompt":[{"type":"text","text":"slow"}]}}))
            .await;
        // The tool card opens before the agent hangs.
        assert_eq!(
            peer.next().await["params"]["update"]["sessionUpdate"],
            "tool_call"
        );
        peer.send(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":sid}}))
            .await;
        let (done, _) = peer.until_response(3).await;
        assert_eq!(done["result"]["stopReason"], "cancelled");
    }

    #[tokio::test]
    async fn eof_after_the_last_request_still_lets_the_prompt_answer() {
        let input = [
            r#"{"jsonrpc":"2.0","id":1,"method":"session/new","params":{}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"session/prompt","params":{"sessionId":"loams-bot-1","prompt":[{"type":"text","text":"hi"}]}}"#,
        ]
        .join("\n")
            + "\n";
        let (out, mut rx) = duplex(64 * 1024);
        serve(
            BufReader::new(input.as_bytes()),
            out,
            Arc::new(MockA2aClient::default()),
        )
        .await
        .unwrap();
        let mut text = String::new();
        tokio::io::AsyncReadExt::read_to_string(&mut rx, &mut text)
            .await
            .unwrap();
        assert!(text.contains(r#""stopReason":"end_turn""#), "{text}");
    }

    #[tokio::test]
    async fn unknown_methods_and_sessions_are_errors() {
        let mut peer = start(Arc::new(MockA2aClient::default()));
        peer.send(json!({"jsonrpc":"2.0","id":7,"method":"fs/read","params":{}}))
            .await;
        assert_eq!(peer.next().await["error"]["code"], -32601);
        peer.send(json!({"jsonrpc":"2.0","id":8,"method":"session/prompt",
            "params":{"sessionId":"nope","prompt":[{"type":"text","text":"x"}]}}))
            .await;
        assert_eq!(peer.next().await["error"]["code"], -32602);
    }
}
