//! SD1 Task 13: supervised approvals in the engine (design §53 §8.1, §6.5
//! G17, G18, G22; DD1 Task 24's contract).
//!
//! The turns are a scripted harness's: `ask <callId> <tool> <command…>` asks
//! for one approval through the run's bridge (when the thread is supervised)
//! and reports what it was told; anything else just completes. Every test
//! drives the daemon through its ndjson methods, as a client would.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use loams_agentd_harness::mock::MockHarness;
use loams_agentd_harness::{ApprovalAsk, Harness, HarnessError, RunControls};
use loams_agentd_proto::approvals::ApprovalDecision;
use loams_agentd_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SessionStatus, SteeringMode,
};
use loams_agentd_rpc::{RpcClient, methods};
use loams_agentd_sessions::approvals::ApprovalSettings;
use loams_agentd_sessions::{EngineCore, HarnessRegistry};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(30);

/// What one run saw: whether it was supervised, its `autoApprove`, and the
/// decision its ask got (`None`: it never asked, or the bridge dropped).
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    chat_cwd: String,
    supervised: bool,
    auto_approve: bool,
    decision: Option<ApprovalDecision>,
}

#[derive(Debug, Default)]
struct Asker {
    seen: Arc<Mutex<Vec<Seen>>>,
    /// Whether this harness can ask (`supports_approvals`).
    cannot_ask: bool,
}

fn done(status: DoneStatus) -> AgentEvent {
    AgentEvent::Done {
        status,
        result: None,
        error: None,
        session_id: None,
    }
}

#[async_trait]
impl Harness for Asker {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Mock"
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn supports_approvals(&self) -> bool {
        !self.cannot_ask
    }
    fn steering_mode(&self) -> SteeringMode {
        MockHarness { script: Vec::new() }.steering_mode()
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        MockHarness { script: Vec::new() }.models().await
    }
    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let words: Vec<String> = request
            .prompt
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        let seen = self.seen.clone();
        let supervised = controls.request_approval.is_some();
        let auto_approve = request.auto_approve;
        let cwd = request.cwd.clone();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            // The controls live as long as the run.
            let controls = controls;
            let mut decision = None;
            if let [ask, call_id, tool, command @ ..] = words.as_slice()
                && ask == "ask"
            {
                let args = json!({ "command": command.join(" "), "token": "s3cret-value-123" });
                let _ = tx.send(Ok(AgentEvent::ToolCall {
                    id: call_id.clone(),
                    call: loams_agentd_proto::ToolCall::Exec {
                        command: command.join(" "),
                    },
                }));
                if let Some(request_approval) = &controls.request_approval {
                    let answer = request_approval(ApprovalAsk {
                        call_id: call_id.clone(),
                        tool: tool.clone(),
                        risk: loams_agentd_harness::approvals::tool_risk(tool, &args),
                        args,
                    });
                    decision = answer.await.ok();
                }
                let allowed = !supervised || decision.is_some_and(ApprovalDecision::allows);
                let _ = tx.send(Ok(AgentEvent::ToolResult {
                    id: call_id.clone(),
                    is_error: !allowed,
                    output: Some(if allowed { "ran" } else { "denied" }.into()),
                    diff: None,
                }));
            }
            seen.lock().unwrap().push(Seen {
                chat_cwd: cwd,
                supervised,
                auto_approve,
                decision,
            });
            let _ = tx.send(Ok(AgentEvent::TextDelta { text: "ok".into() }));
            let _ = tx.send(Ok(done(DoneStatus::Completed)));
        });
        Ok(futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|event| (event, rx))
        })
        .boxed())
    }
}

struct Daemon {
    root: PathBuf,
    core: EngineCore,
    client: RpcClient,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Daemon {
    fn start(root: &Path, settings: ApprovalSettings) -> Self {
        Self::start_with(root, settings, false)
    }

    fn start_with(root: &Path, settings: ApprovalSettings, cannot_ask: bool) -> Self {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let registry = HarnessRegistry::new();
        registry.register(Arc::new(Asker {
            seen: seen.clone(),
            cannot_ask,
        }));
        let core = EngineCore::assemble_with_approvals(
            &root.join("data"),
            Arc::new(registry),
            HarnessId::Mock,
            settings,
        )
        .unwrap();
        let client = loams_agentd_rpc::memory_client(core.rpc_service());
        Self {
            root: root.to_owned(),
            core,
            client,
            seen,
        }
    }

    fn cwd(&self, chat: &str) -> PathBuf {
        let cwd = self.root.join("work").join(chat);
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::canonicalize(&cwd).unwrap()
    }

    async fn create_chat(&self, chat: &str) {
        self.client
            .call(
                methods::MUTATE,
                json!({
                    "op": "createChat",
                    "chatId": chat,
                    "deviceId": self.core.device_id,
                    "cwd": self.cwd(chat).to_string_lossy(),
                    "config": { "harness": "mock", "modelOptions": {}, "sandbox": "workspace-write" }
                }),
            )
            .await
            .unwrap();
    }

    async fn set_mode(&self, chat: &str, mode: &str) {
        self.client
            .call(
                methods::SET_THREAD_APPROVAL_MODE,
                json!({ "chatId": chat, "mode": mode }),
            )
            .await
            .unwrap();
    }

    async fn send(&self, chat: &str, prompt: &str, message_id: &str, auto_approve: bool) {
        self.client
            .call(
                methods::QUEUE_COMMAND,
                json!({
                    "chatId": chat,
                    "command": {
                        "kind": "run",
                        "request": RunRequest {
                            mcp_servers: Vec::new(),
                            mcp: None,
                            prompt: prompt.into(),
                            harness: None,
                            model: None,
                            reasoning: None,
                            model_options: Default::default(),
                            cwd: self.cwd(chat).to_string_lossy().into_owned(),
                            sandbox: SandboxLevel::WorkspaceWrite,
                            auto_approve,
                            attachments: Vec::new(),
                            worktree: None,
                            resume: None,
                        },
                        "messageId": message_id,
                    }
                }),
            )
            .await
            .unwrap();
    }

    /// The runs seen so far, once there are `n`.
    async fn seen(&self, n: usize) -> Vec<Seen> {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let seen = self.seen.lock().unwrap().clone();
            if seen.len() >= n {
                return seen;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for run {n}: {seen:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// The first `WatchApprovals` item that satisfies `test`.
    async fn approvals_until(&self, params: Value, test: impl Fn(&Value) -> bool) -> Value {
        let mut feed = self
            .client
            .subscribe(methods::WATCH_APPROVALS, params)
            .await
            .unwrap();
        tokio::time::timeout(WAIT, async {
            loop {
                let item = feed.recv().await.expect("the watch is open");
                if test(&item) {
                    return item;
                }
            }
        })
        .await
        .expect("the approvals reached the expected state in time")
    }

    async fn resolve(
        &self,
        chat: Option<&str>,
        call_id: &str,
        decision: &str,
    ) -> Result<Value, String> {
        let mut params = json!({ "callId": call_id, "decision": decision });
        if let Some(chat) = chat {
            params["chatId"] = json!(chat);
        }
        self.client
            .call(methods::RESOLVE_APPROVAL, params)
            .await
            .map_err(|e| e.to_string())
    }

    async fn status(&self, chat: &str) -> Option<SessionStatus> {
        self.core.sessions.session_status(chat).map(|s| s.status)
    }
}

fn pending(list: &Value) -> &Vec<Value> {
    list["pending"].as_array().unwrap()
}

#[tokio::test]
async fn full_access_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(tmp.path(), ApprovalSettings::full_access_default());
    // A thread with no mode on a configured full-access default: the
    // behaviour before Task 13, whatever `autoApprove` says.
    daemon.create_chat("chat-a").await;
    daemon
        .send("chat-a", "ask c1 Bash rm -rf build", "m1", false)
        .await;
    // An explicit full-access thread: its runs carry `autoApprove`.
    daemon.create_chat("chat-b").await;
    daemon.set_mode("chat-b", "full-access").await;
    daemon
        .send("chat-b", "ask c2 Bash rm -rf build", "m2", false)
        .await;
    let seen = daemon.seen(2).await;
    for run in &seen {
        assert!(!run.supervised, "{run:?}");
        assert_eq!(run.decision, None, "{run:?}");
    }
    let b = seen
        .iter()
        .find(|r| r.chat_cwd.ends_with("chat-b"))
        .unwrap();
    assert!(
        b.auto_approve,
        "an explicit full-access thread runs auto-approved"
    );
    let a = seen
        .iter()
        .find(|r| r.chat_cwd.ends_with("chat-a"))
        .unwrap();
    assert!(
        !a.auto_approve,
        "a thread with no mode keeps its request's autoApprove"
    );
    // Nothing is pending, and the thread watch says full access.
    let list = daemon
        .approvals_until(json!({ "chatId": "chat-a" }), |_| true)
        .await;
    assert!(pending(&list).is_empty(), "{list}");
    assert_eq!(list["mode"], "full-access");
}

/// Threat model I-1 (owner, 2026-10-15; SD1 T30-19): on the default
/// settings a thread nobody set a mode for is supervised, so its run waits;
/// full access is asked for.
#[tokio::test]
async fn no_mode_is_supervised_by_default() {
    let tmp = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(tmp.path(), ApprovalSettings::default());
    daemon.create_chat("chat-n").await;
    daemon
        .send("chat-n", "ask call-n Bash rm -rf build", "m1", true)
        .await;
    let list = daemon
        .approvals_until(json!({ "chatId": "chat-n" }), |l| !pending(l).is_empty())
        .await;
    assert_eq!(list["mode"], "supervised");
    assert_eq!(pending(&list)[0]["id"], "call-n");
    assert!(daemon.seen.lock().unwrap().is_empty(), "nothing ran yet");
    daemon
        .resolve(Some("chat-n"), "call-n", "deny")
        .await
        .unwrap();
    let seen = daemon.seen(1).await;
    assert!(seen[0].supervised && !seen[0].auto_approve, "{seen:?}");
    assert_eq!(seen[0].decision, Some(ApprovalDecision::Deny));
}

#[tokio::test]
async fn supervised_run_waits_for_resolve() {
    let tmp = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(tmp.path(), ApprovalSettings::default());
    daemon.create_chat("chat-s").await;
    daemon.set_mode("chat-s", "supervised").await;
    let mut notifications = daemon
        .client
        .subscribe(methods::WATCH_NOTIFICATIONS, json!({}))
        .await
        .unwrap();
    // `autoApprove` cannot open a supervised thread.
    daemon
        .send("chat-s", "ask call-1 Bash rm -rf build", "m1", true)
        .await;
    let list = daemon
        .approvals_until(json!({ "chatId": "chat-s" }), |l| !pending(l).is_empty())
        .await;
    assert_eq!(list["mode"], "supervised");
    let approval = &pending(&list)[0];
    assert_eq!(approval["id"], "call-1");
    assert_eq!(approval["chatId"], "chat-s");
    assert_eq!(approval["turnId"], "m1");
    assert_eq!(approval["tool"], "Bash");
    assert_eq!(approval["risk"], "destructive");
    assert_eq!(approval["args"]["command"], "rm -rf build");
    assert_eq!(approval["args"]["token"], "[redacted]", "args are redacted");
    assert!(approval["createdAtMs"].as_i64().unwrap() > 0);
    // The thread waits, and the person is told.
    assert_eq!(
        daemon.status("chat-s").await,
        Some(SessionStatus::AwaitingInput)
    );
    let n = tokio::time::timeout(WAIT, async {
        loop {
            let n = notifications.recv().await.unwrap();
            if n["kind"] == "approval" {
                return n;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(n["chatId"], "chat-s");
    assert!(daemon.seen.lock().unwrap().is_empty(), "nothing ran yet");

    // The unfiltered watch lists it too; an unknown id is not pending.
    let all = daemon
        .approvals_until(json!({}), |l| !pending(l).is_empty())
        .await;
    assert!(all.get("mode").is_none(), "{all}");
    let error = daemon
        .resolve(Some("chat-s"), "nope", "once")
        .await
        .unwrap_err();
    assert!(error.contains("approval_not_pending"), "{error}");

    daemon
        .resolve(Some("chat-s"), "call-1", "once")
        .await
        .unwrap();
    let seen = daemon.seen(1).await;
    assert!(seen[0].supervised);
    assert!(
        !seen[0].auto_approve,
        "a supervised run is never auto-approved"
    );
    assert_eq!(seen[0].decision, Some(ApprovalDecision::Once));
    daemon
        .approvals_until(json!({ "chatId": "chat-s" }), |l| pending(l).is_empty())
        .await;
    // Answered twice: the second is refused.
    let error = daemon.resolve(None, "call-1", "once").await.unwrap_err();
    assert!(error.contains("approval_not_pending"), "{error}");
}

#[tokio::test]
async fn always_remembered_per_thread_and_tool() {
    let tmp = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(tmp.path(), ApprovalSettings::default());
    for chat in ["chat-1", "chat-2"] {
        daemon.create_chat(chat).await;
        daemon.set_mode(chat, "supervised").await;
    }
    daemon
        .send("chat-1", "ask c1 Bash cargo test", "m1", false)
        .await;
    daemon
        .approvals_until(json!({}), |l| !pending(l).is_empty())
        .await;
    // DD1's `sessionId` names the thread too.
    daemon
        .client
        .call(
            methods::RESOLVE_APPROVAL,
            json!({ "callId": "c1", "sessionId": "chat-1", "decision": "always" }),
        )
        .await
        .unwrap();
    daemon.seen(1).await;

    // The same tool in the same thread: allowed without asking.
    daemon
        .send("chat-1", "ask c2 Bash cargo build", "m2", false)
        .await;
    let seen = daemon.seen(2).await;
    assert_eq!(seen[1].decision, Some(ApprovalDecision::Always), "{seen:?}");

    // Another tool in the same thread asks.
    daemon
        .send("chat-1", "ask c3 Edit src/main.rs", "m3", false)
        .await;
    let list = daemon
        .approvals_until(json!({ "chatId": "chat-1" }), |l| !pending(l).is_empty())
        .await;
    assert_eq!(pending(&list)[0]["id"], "c3");
    daemon.resolve(Some("chat-1"), "c3", "deny").await.unwrap();
    let seen = daemon.seen(3).await;
    assert_eq!(seen[2].decision, Some(ApprovalDecision::Deny));

    // The same tool in another thread asks.
    daemon
        .send("chat-2", "ask c4 Bash cargo test", "m4", false)
        .await;
    let list = daemon
        .approvals_until(json!({ "chatId": "chat-2" }), |l| !pending(l).is_empty())
        .await;
    assert_eq!(pending(&list)[0]["tool"], "Bash");
    daemon.resolve(Some("chat-2"), "c4", "once").await.unwrap();
    daemon.seen(4).await;
}

#[tokio::test]
async fn undecided_approval_times_out_to_deny() {
    let tmp = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(
        tmp.path(),
        ApprovalSettings {
            timeout: Duration::from_millis(400),
            ..ApprovalSettings::default()
        },
    );
    daemon.create_chat("chat-t").await;
    daemon.set_mode("chat-t", "supervised").await;
    daemon
        .send("chat-t", "ask c1 Bash cargo test", "m1", false)
        .await;
    let seen = daemon.seen(1).await;
    assert_eq!(seen[0].decision, Some(ApprovalDecision::Deny));
    daemon
        .approvals_until(json!({}), |l| pending(l).is_empty())
        .await;
    let error = daemon
        .resolve(Some("chat-t"), "c1", "once")
        .await
        .unwrap_err();
    assert!(error.contains("approval_not_pending"), "{error}");
}

#[tokio::test]
async fn pending_approval_listed_after_restart() {
    let tmp = tempfile::tempdir().unwrap();
    {
        let daemon = Daemon::start(tmp.path(), ApprovalSettings::default());
        daemon.create_chat("chat-r").await;
        daemon.set_mode("chat-r", "supervised").await;
        daemon
            .send("chat-r", "ask c1 Bash rm -rf build", "m1", false)
            .await;
        daemon
            .approvals_until(json!({}), |l| !pending(l).is_empty())
            .await;
        daemon.core.shutdown().await;
    }
    let daemon = Daemon::start(tmp.path(), ApprovalSettings::default());
    let list = daemon
        .approvals_until(json!({ "chatId": "chat-r" }), |_| true)
        .await;
    assert_eq!(list["mode"], "supervised", "the mode survives too");
    let approval = &pending(&list)[0];
    assert_eq!(approval["id"], "c1");
    assert_eq!(approval["tool"], "Bash");
    assert_eq!(approval["args"]["command"], "rm -rf build");
    assert_eq!(approval["args"]["token"], "[redacted]", "stored redacted");
    // Its run is gone; answering it clears it.
    daemon.resolve(Some("chat-r"), "c1", "deny").await.unwrap();
    daemon
        .approvals_until(json!({}), |l| pending(l).is_empty())
        .await;
}

#[tokio::test]
async fn supervised_thread_refused_on_a_harness_that_cannot_ask() {
    let tmp = tempfile::tempdir().unwrap();
    let daemon = Daemon::start_with(tmp.path(), ApprovalSettings::default(), true);
    daemon.create_chat("chat-p").await;
    daemon.set_mode("chat-p", "supervised").await;
    daemon
        .send("chat-p", "ask c1 Bash cargo test", "m1", false)
        .await;
    let handle = daemon.core.doc_host.open("chat-p").unwrap();
    let deadline = tokio::time::Instant::now() + WAIT;
    let resolution = loop {
        let command = handle.doc().read_commands().unwrap().into_iter().next();
        if let Some(command) = command
            && command.status != loams_agentd_doc::SessionCommandStatus::Pending
        {
            assert_eq!(
                command.status,
                loams_agentd_doc::SessionCommandStatus::Rejected
            );
            break command.resolution.unwrap_or_default();
        }
        assert!(tokio::time::Instant::now() < deadline, "the run is refused");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(resolution.contains("full access"), "{resolution}");
    assert!(
        daemon.seen.lock().unwrap().is_empty(),
        "nothing ran unsupervised"
    );
}

/// SD1 Task 30: a fork of a supervised thread hung under a thread with no
/// mode (on a configured full-access default) stays supervised.
#[tokio::test]
async fn fork_keeps_its_source_mode() {
    let tmp = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(tmp.path(), ApprovalSettings::full_access_default());
    daemon.create_chat("src").await;
    daemon.create_chat("other").await;
    daemon.send("src", "hello", "m1", false).await;
    daemon.seen(1).await;
    daemon.set_mode("src", "supervised").await;
    let deadline = tokio::time::Instant::now() + WAIT;
    let forked = loop {
        match daemon
            .client
            .call(
                methods::FORK_SIDE_CHAT,
                json!({ "chatId": "fork", "sourceChatId": "src", "parentChatId": "other" }),
            )
            .await
        {
            Ok(chat) => break chat,
            Err(error) => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "the fork failed: {error}"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    };
    assert_eq!(forked["parentChatId"], "other");
    let list = daemon
        .approvals_until(json!({ "chatId": "fork" }), |_| true)
        .await;
    assert_eq!(list["mode"], "supervised");
    let list = daemon
        .approvals_until(json!({ "chatId": "other" }), |_| true)
        .await;
    assert_eq!(list["mode"], "full-access");
}

#[tokio::test]
async fn set_mode_refuses_bad_params() {
    let tmp = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(tmp.path(), ApprovalSettings::default());
    let error = daemon
        .client
        .call(
            methods::SET_THREAD_APPROVAL_MODE,
            json!({ "chatId": "", "mode": "supervised" }),
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("bad params: chatId is required"), "{error}");
}
