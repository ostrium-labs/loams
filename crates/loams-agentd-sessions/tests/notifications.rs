//! SD1 Task 14: notifications and per-thread usage (design §53 §6.5 G19,
//! G20).
//!
//! The turns are the Mock harness's, scripted per prompt through
//! `MockHarness::run_with`, so no test reads the process environment:
//! `ok` completes, `fail` errors, `question` asks the person a question,
//! `ask` asks for an approval before its edit (in a supervised thread, SD1
//! Task 13), and `usage <in> <out> <cache> [cost]` reports that usage before
//! `Done`.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use loams_agentd_harness::mock::{MockEdits, MockHarness};
use loams_agentd_harness::{Harness, HarnessError, RunControls};
use loams_agentd_proto::notify::{BUFFER, NotificationKind};
use loams_agentd_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SessionStatus, SteeringMode,
};
use loams_agentd_rpc::{RpcClient, methods};
use loams_agentd_sessions::notify::TurnNotifier;
use loams_agentd_sessions::{EngineCore, HarnessRegistry, Notifications, TurnObserver};
use serde_json::{Value, json};

const CHAT: &str = "chat-notify";
const WAIT: Duration = Duration::from_secs(30);

/// A `WatchNotifications` stream over the ndjson client.
type Feed = tokio::sync::mpsc::Receiver<Value>;

/// The Mock harness, its script chosen by the prompt.
#[derive(Debug)]
struct ScriptedMock;

fn done(status: DoneStatus) -> AgentEvent {
    AgentEvent::Done {
        status,
        result: None,
        error: (status == DoneStatus::Errored).then(|| "the mock failed".to_owned()),
        session_id: None,
    }
}

fn text(text: &str) -> AgentEvent {
    AgentEvent::TextDelta {
        text: text.to_owned(),
    }
}

#[async_trait]
impl Harness for ScriptedMock {
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
        true
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
        let words: Vec<&str> = request.prompt.split_whitespace().collect();
        let mut edits = MockEdits::default();
        if words.as_slice() == ["question"] {
            let answer = (controls.request_input)(vec![loams_agentd_proto::UserInputQuestion {
                id: "q-continue".into(),
                header: "Choice".into(),
                question: "Continue?".into(),
                options: vec!["Yes".into(), "No".into()],
                prefill: None,
                multiline: false,
                multi_select: false,
            }]);
            return Ok(futures::stream::once(async move {
                let _ = answer.await;
                Ok(text("Answered."))
            })
            .chain(futures::stream::iter([Ok(done(DoneStatus::Completed))]))
            .boxed());
        }
        let script = match words.as_slice() {
            ["fail"] => vec![text("Trying."), done(DoneStatus::Errored)],
            ["ask"] => {
                edits = MockEdits {
                    paths: vec!["notes.txt".into()],
                    approve: true,
                };
                vec![text("Asking first."), done(DoneStatus::Completed)]
            }
            ["usage", input, output, cache, rest @ ..] => vec![
                text("Counting."),
                AgentEvent::Usage {
                    input_tokens: input.parse().unwrap(),
                    output_tokens: output.parse().unwrap(),
                    cache_tokens: Some(cache.parse().unwrap()),
                    cost_usd: rest.first().map(|c| c.parse().unwrap()),
                },
                done(DoneStatus::Completed),
            ],
            _ => vec![text("Done."), done(DoneStatus::Completed)],
        };
        MockHarness { script }.run_with(request, controls, edits)
    }
}

fn run_request(cwd: &Path, prompt: &str) -> RunRequest {
    RunRequest {
        mcp_servers: Vec::new(),
        mcp: None,
        prompt: prompt.into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: cwd.to_string_lossy().into_owned(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: false,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
    }
}

struct Daemon {
    cwd: PathBuf,
    data: PathBuf,
    core: EngineCore,
    client: RpcClient,
}

impl Daemon {
    async fn start(root: &Path) -> Self {
        let cwd = root.join("work");
        std::fs::create_dir_all(&cwd).unwrap();
        let cwd = std::fs::canonicalize(&cwd).unwrap();
        let data = root.join("data");
        let registry = HarnessRegistry::new();
        registry.register(Arc::new(ScriptedMock));
        let core = EngineCore::assemble(&data, Arc::new(registry), HarnessId::Mock).unwrap();
        let client = loams_agentd_rpc::memory_client(core.rpc_service());
        Self {
            cwd,
            data,
            core,
            client,
        }
    }

    async fn create_chat(&self, title: &str) {
        self.client
            .call(
                methods::MUTATE,
                json!({
                    "op": "createChat",
                    "chatId": CHAT,
                    "deviceId": self.core.device_id,
                    "cwd": self.cwd.to_string_lossy(),
                    "config": {
                        "harness": "mock",
                        "modelOptions": {},
                        "sandbox": "workspace-write"
                    }
                }),
            )
            .await
            .unwrap();
        self.client
            .call(
                methods::MUTATE,
                json!({ "op": "renameChat", "chatId": CHAT, "title": title }),
            )
            .await
            .unwrap();
    }

    async fn send(&self, prompt: &str, message_id: &str) {
        self.client
            .call(
                methods::QUEUE_COMMAND,
                json!({
                    "chatId": CHAT,
                    "command": {
                        "kind": "run",
                        "request": run_request(&self.cwd, prompt),
                        "messageId": message_id,
                    }
                }),
            )
            .await
            .unwrap();
    }

    /// Wait until the chat's turn has settled (Idle or Errored).
    async fn settled(&self) {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let status = self.core.sessions.session_status(CHAT).map(|s| s.status);
            if matches!(status, Some(SessionStatus::Idle | SessionStatus::Errored)) {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for the turn to settle ({status:?})"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Send a turn and wait for it to settle.
    async fn turn(&self, prompt: &str, message_id: &str) {
        self.send(prompt, message_id).await;
        // The run starts asynchronously: wait for it to leave Idle first.
        let deadline = tokio::time::Instant::now() + WAIT;
        while self.usage().await["turns"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["turnId"] != message_id)
        {
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for turn {message_id}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        self.settled().await;
    }

    /// The unanswered question in the transcript: its request and first
    /// question ids.
    async fn pending_question(&self) -> (String, String) {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let entries = self
                .core
                .doc_host
                .open(CHAT)
                .unwrap()
                .doc()
                .read_entries()
                .unwrap();
            let found = entries
                .iter()
                .flat_map(|e| e.parts.iter())
                .find_map(|p| match p {
                    loams_agentd_proto::MessagePart::Input {
                        request_id,
                        questions,
                        resolved: false,
                        ..
                    } => Some((request_id.clone(), questions[0].id.clone())),
                    _ => None,
                });
            if let Some(found) = found {
                return found;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for the question"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn usage(&self) -> Value {
        self.client
            .call(methods::GET_THREAD_USAGE, json!({ "chatId": CHAT }))
            .await
            .unwrap()
    }

    async fn watch(&self) -> Feed {
        self.client
            // Not `subscribe_checked`: its acknowledgement is the first item,
            // and an empty buffer sends none.
            .subscribe(methods::WATCH_NOTIFICATIONS, json!({}))
            .await
            .unwrap()
    }
}

/// The next notification on `feed`.
async fn next(feed: &mut Feed) -> Value {
    tokio::time::timeout(WAIT, feed.recv())
        .await
        .expect("a notification in time")
        .expect("the feed is open")
}

#[tokio::test]
async fn notifications_for_each_kind() {
    let tmp = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(tmp.path()).await;
    daemon.create_chat("Fix the login").await;
    let mut feed = daemon.watch().await;

    // A turn that completes.
    daemon.send("ok", "msg-1").await;
    let n = next(&mut feed).await;
    assert_eq!(n["kind"], "turn_done", "{n}");
    assert_eq!(n["chatId"], CHAT);
    assert_eq!(n["title"], "Fix the login");
    assert!(n.get("groupId").is_none(), "{n}");
    assert!(n["id"].as_str().is_some_and(|id| !id.is_empty()));
    assert!(n["createdAtMs"].as_i64().unwrap() > 0);
    daemon.settled().await;

    // A turn that fails.
    daemon.send("fail", "msg-2").await;
    let n = next(&mut feed).await;
    assert_eq!(n["kind"], "error", "{n}");
    assert_eq!(n["title"], "Fix the login");
    daemon.settled().await;

    // A question. Answered, the run goes on and its turn completes.
    daemon.send("question", "msg-3").await;
    let n = next(&mut feed).await;
    assert_eq!(n["kind"], "question", "{n}");
    assert_eq!(n["chatId"], CHAT);
    assert_eq!(n["title"], "Fix the login");
    let (request_id, question_id) = daemon.pending_question().await;
    daemon
        .client
        .call(
            methods::QUEUE_COMMAND,
            json!({ "chatId": CHAT, "command": { "kind": "respondInput", "requestId": request_id,
                    "answers": [{ "questionId": question_id, "labels": ["No"] }] } }),
        )
        .await
        .unwrap();
    let n = next(&mut feed).await;
    assert_eq!(n["kind"], "turn_done", "{n}");
    daemon.settled().await;

    // An approval (SD1 Task 13): the Mock's edit in a supervised thread.
    // Denied, the run goes on and its turn completes.
    daemon
        .client
        .call(
            methods::SET_THREAD_APPROVAL_MODE,
            json!({ "chatId": CHAT, "mode": "supervised" }),
        )
        .await
        .unwrap();
    daemon.send("ask", "msg-4").await;
    let n = next(&mut feed).await;
    assert_eq!(n["kind"], "approval", "{n}");
    assert_eq!(n["chatId"], CHAT);
    assert_eq!(n["title"], "Fix the login");
    daemon
        .client
        .call(
            methods::RESOLVE_APPROVAL,
            json!({ "chatId": CHAT, "callId": "mock-edit-t1-0", "decision": "deny" }),
        )
        .await
        .unwrap();
    let n = next(&mut feed).await;
    assert_eq!(n["kind"], "turn_done", "{n}");
    daemon.settled().await;

    // An interrupted turn is the person's own doing, and a subagent's end
    // belongs to its parent turn: neither posts.
    let quiet = Notifications::new();
    let notifier = TurnNotifier::new(quiet.clone(), Arc::new(|_: &str| None));
    notifier.event(CHAT, &done(DoneStatus::Interrupted));
    notifier.event(
        CHAT,
        &AgentEvent::Subagent {
            parent_tool_use_id: "tool-1".into(),
            event: Box::new(done(DoneStatus::Completed)),
        },
    );
    assert!(quiet.recent().is_empty());
    notifier.event(CHAT, &done(DoneStatus::Completed));
    assert_eq!(quiet.recent()[0].title, "New thread");

    // Approvals (SD1 Task 13) and run groups (SD1 Task 12) post through the
    // engine's hub.
    let notifications = &daemon.core.notifications;
    notifications.push(
        NotificationKind::Approval,
        Some(CHAT.into()),
        None,
        "Fix the login",
    );
    notifications.push(
        NotificationKind::GroupReady,
        None,
        Some("group-1".into()),
        "Run group ready",
    );
    notifications.push(
        NotificationKind::CheckFailed,
        None,
        Some("group-1".into()),
        "Check failed: cargo test",
    );
    for (kind, chat, group) in [
        ("approval", Some(CHAT), None),
        ("group_ready", None, Some("group-1")),
        ("check_failed", None, Some("group-1")),
    ] {
        let n = next(&mut feed).await;
        assert_eq!(n["kind"], kind, "{n}");
        assert_eq!(n["chatId"].as_str(), chat, "{n}");
        assert_eq!(n["groupId"].as_str(), group, "{n}");
    }
    // Nothing else.
    assert!(
        tokio::time::timeout(Duration::from_millis(300), feed.recv())
            .await
            .is_err(),
        "an unexpected notification"
    );

    // A watcher that connects later gets the buffer, oldest first.
    let mut late = daemon.watch().await;
    let mut kinds = Vec::new();
    for _ in 0..9 {
        kinds.push(next(&mut late).await["kind"].as_str().unwrap().to_owned());
    }
    assert_eq!(
        kinds,
        [
            "turn_done",
            "error",
            "question",
            "turn_done",
            "approval",
            "turn_done",
            "approval",
            "group_ready",
            "check_failed"
        ]
    );
    daemon.core.shutdown().await;
}

#[tokio::test]
async fn notification_buffer_bounded() {
    let hub = Notifications::new();
    for n in 0..BUFFER + 50 {
        hub.push(
            NotificationKind::TurnDone,
            Some(format!("chat-{n}")),
            None,
            format!("Turn {n}"),
        );
    }
    let recent = hub.recent();
    assert_eq!(recent.len(), BUFFER);
    assert_eq!(recent[0].chat_id.as_deref(), Some("chat-50"));
    assert_eq!(
        recent[BUFFER - 1].chat_id.as_deref(),
        Some(format!("chat-{}", BUFFER + 49).as_str())
    );

    // A watcher gets exactly the buffer, then the live ones, in order.
    let mut watch = hub.watch();
    let first: Vec<_> = (&mut watch).take(BUFFER).collect().await;
    assert_eq!(first, recent);
    hub.push(NotificationKind::Error, None, None, "x".repeat(1000));
    let live = tokio::time::timeout(WAIT, watch.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(live.kind, NotificationKind::Error);
    // Titles are one line's worth.
    assert!(live.title.chars().count() <= 160, "{}", live.title.len());
    assert_eq!(hub.recent().len(), BUFFER);
}

#[tokio::test]
async fn usage_persisted_per_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(tmp.path()).await;
    daemon.create_chat("Count").await;
    daemon.turn("usage 100 20 5", "msg-1").await;
    daemon.turn("usage 7 3 0", "msg-2").await;
    // A turn whose harness reports no usage still has its duration.
    daemon.turn("ok", "msg-3").await;

    let check = |usage: &Value| {
        let turns = usage["turns"].as_array().unwrap();
        assert_eq!(turns.len(), 3, "{usage}");
        assert_eq!(turns[0]["turnId"], "msg-1");
        assert_eq!(turns[0]["inputTokens"], 100);
        assert_eq!(turns[0]["outputTokens"], 20);
        assert_eq!(turns[0]["cacheTokens"], 5);
        assert_eq!(turns[1]["turnId"], "msg-2");
        assert_eq!(turns[1]["inputTokens"], 7);
        assert_eq!(turns[2]["turnId"], "msg-3");
        assert!(turns[2].get("inputTokens").is_none(), "{usage}");
        let mut duration = 0;
        for turn in turns {
            assert!(turn["durationMs"].as_i64().unwrap() >= 0, "{usage}");
            duration += turn["durationMs"].as_i64().unwrap();
        }
        let totals = &usage["totals"];
        assert_eq!(totals["inputTokens"], 107);
        assert_eq!(totals["outputTokens"], 23);
        assert_eq!(totals["cacheTokens"], 5);
        assert_eq!(totals["durationMs"], duration);
    };
    let before = daemon.usage().await;
    check(&before);

    // The rows outlive the daemon.
    let Daemon { core, data, .. } = daemon;
    core.shutdown().await;
    drop(core);
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(ScriptedMock));
    let core = EngineCore::assemble(&data, Arc::new(registry), HarnessId::Mock).unwrap();
    let client = loams_agentd_rpc::memory_client(core.rpc_service());
    let after = client
        .call(methods::GET_THREAD_USAGE, json!({ "chatId": CHAT }))
        .await
        .unwrap();
    assert_eq!(after, before);

    // A deleted thread's usage goes with it.
    client
        .call(
            methods::MUTATE,
            json!({ "op": "deleteChat", "chatId": CHAT }),
        )
        .await
        .unwrap();
    let gone = client
        .call(methods::GET_THREAD_USAGE, json!({ "chatId": CHAT }))
        .await
        .unwrap();
    assert_eq!(gone["turns"], json!([]));
    core.shutdown().await;
}

#[tokio::test]
async fn usage_cost_only_when_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(tmp.path()).await;
    daemon.create_chat("Cost").await;
    // No cost reported: none is invented, in the turn or the totals.
    daemon.turn("usage 10 2 0", "msg-1").await;
    let usage = daemon.usage().await;
    assert!(usage["turns"][0].get("costUsd").is_none(), "{usage}");
    assert!(usage["totals"].get("costUsd").is_none(), "{usage}");

    // The harness's own figure is kept as it reported it.
    daemon.turn("usage 10 2 0 0.0125", "msg-2").await;
    let usage = daemon.usage().await;
    assert!(usage["turns"][0].get("costUsd").is_none(), "{usage}");
    assert_eq!(usage["turns"][1]["costUsd"], 0.0125);
    assert_eq!(usage["totals"]["costUsd"], 0.0125);
    daemon.core.shutdown().await;
}
