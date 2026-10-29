//! SD1 Task 10: per-turn checkpoints and restore (design §53 §6.5 G6, G7).
//!
//! Real git repositories in temporary directories; the turns are the Mock
//! harness's file edits (SD1 Task 8), given explicitly through
//! `MockHarness::run_with` so no test reads the process environment.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::BoxStream;
use loams_agentd_harness::mock::{MockEdits, MockHarness};
use loams_agentd_harness::{Harness, HarnessError, RunControls};
use loams_agentd_proto::rpc::{Role, method_spec};
use loams_agentd_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SessionStatus, SteeringMode,
};
use loams_agentd_rpc::{RpcClient, methods};
use loams_agentd_sessions::{ApprovalSettings, Checkpoints, EngineCore, HarnessRegistry, Repos};
use serde_json::{Value, json};

const CHAT: &str = "chat-checkpoints";

/// The Mock harness with the edit knob on: each turn appends `mock edit <n>`
/// to `notes.txt` and `src/new.txt`. No steering, so every turn is a fresh
/// dispatch.
#[derive(Debug)]
struct EditingMock(MockHarness);

#[async_trait]
impl Harness for EditingMock {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Mock"
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn steering_mode(&self) -> SteeringMode {
        self.0.steering_mode()
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        self.0.reasoning_levels()
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        self.0.models().await
    }
    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.0.run_with(
            request,
            controls,
            MockEdits {
                paths: vec!["notes.txt".into(), "src/new.txt".into()],
                approve: false,
            },
        )
    }
}

/// A Mock whose every turn stays running until it is interrupted, so a test
/// can look at the thread while its turn is in flight.
#[derive(Debug)]
struct HeldMock(MockHarness);

#[async_trait]
impl Harness for HeldMock {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Mock"
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn steering_mode(&self) -> SteeringMode {
        self.0.steering_mode()
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        self.0.reasoning_levels()
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        self.0.models().await
    }
    async fn run(
        &self,
        _request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        use futures::StreamExt;
        let interrupt = controls.interrupt.clone();
        let started = futures::stream::iter([Ok(AgentEvent::TextDelta {
            text: "Working.".into(),
        })]);
        let interrupted = futures::stream::once(async move {
            // Hold the run's controls until the end, as a real driver does.
            let _controls = controls;
            interrupt.cancelled().await;
            Ok(AgentEvent::Done {
                status: DoneStatus::Interrupted,
                result: None,
                error: None,
                session_id: None,
            })
        });
        Ok(started.chain(interrupted).boxed())
    }
}

async fn git_output(cwd: &Path, args: &[&str]) -> std::process::Output {
    tokio::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@test")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@test")
        .output()
        .await
        .expect("git spawns")
}

async fn git(cwd: &Path, args: &[&str]) -> String {
    let output = git_output(cwd, args).await;
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A repository with a committed `notes.txt` and a `.gitignore` of `*.log`.
async fn repo(dir: &Path) -> PathBuf {
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]).await;
    std::fs::write(repo.join("notes.txt"), "first\n").unwrap();
    std::fs::write(repo.join(".gitignore"), "*.log\n").unwrap();
    git(&repo, &["add", "."]).await;
    git(&repo, &["commit", "-m", "initial"]).await;
    std::fs::canonicalize(&repo).unwrap()
}

fn run_request(cwd: &Path) -> RunRequest {
    RunRequest {
        mcp_servers: Vec::new(),
        mcp: None,
        prompt: "edit the notes".into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: cwd.to_string_lossy().into_owned(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
    }
}

struct Daemon {
    _tmp: tempfile::TempDir,
    repo: PathBuf,
    core: EngineCore,
    client: RpcClient,
}

impl Daemon {
    async fn start() -> Self {
        Self::start_with(Arc::new(EditingMock(MockHarness {
            script: vec![
                AgentEvent::TextDelta {
                    text: "Editing the notes.".into(),
                },
                AgentEvent::Done {
                    status: DoneStatus::Completed,
                    result: None,
                    error: None,
                    session_id: None,
                },
            ],
        })))
        .await
    }

    async fn start_with(harness: Arc<dyn Harness>) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let repo = repo(tmp.path()).await;
        let registry = HarnessRegistry::new();
        registry.register(harness);
        let core = EngineCore::assemble_with_approvals(
            &tmp.path().join("data"),
            Arc::new(registry),
            HarnessId::Mock,
            ApprovalSettings::full_access_default(),
        )
        .unwrap();
        let client = loams_agentd_rpc::memory_client(core.rpc_service());
        client
            .call(
                methods::MUTATE,
                json!({
                    "op": "createChat",
                    "chatId": CHAT,
                    "deviceId": core.device_id,
                    "cwd": repo.to_string_lossy(),
                    "config": {
                        "harness": "mock",
                        "modelOptions": {},
                        "sandbox": "workspace-write"
                    }
                }),
            )
            .await
            .unwrap();
        Self {
            _tmp: tmp,
            repo,
            core,
            client,
        }
    }

    /// Send a turn and wait for its `post` checkpoint.
    async fn turn(&self, turn: u32, message_id: &str) {
        self.client
            .call(
                methods::QUEUE_COMMAND,
                json!({
                    "chatId": CHAT,
                    "command": {
                        "kind": "run",
                        "request": run_request(&self.repo),
                        "messageId": message_id,
                    }
                }),
            )
            .await
            .unwrap();
        let post = format!("refs/loams/checkpoints/{CHAT}/turn/{turn}/post");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let idle = self
                .core
                .sessions
                .session_status(CHAT)
                .is_some_and(|s| s.status == SessionStatus::Idle);
            let written = git_output(&self.repo, &["rev-parse", "--verify", "--quiet", &post])
                .await
                .status
                .success();
            if idle && written {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for turn {turn}'s post checkpoint"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Queue `command` to the thread; its id.
    async fn queue(&self, command: Value) -> String {
        let reply = self
            .client
            .call(
                methods::QUEUE_COMMAND,
                json!({ "chatId": CHAT, "command": command }),
            )
            .await
            .unwrap();
        reply["commandId"].as_str().unwrap().to_owned()
    }

    /// Whether every command queued to the thread is resolved, and whether
    /// its turn is in flight.
    fn settled_and_running(&self) -> (bool, bool) {
        let resolved = self
            .core
            .doc_host
            .open(CHAT)
            .unwrap()
            .doc()
            .read_commands()
            .unwrap()
            .iter()
            .all(|c| c.status != loams_agentd_doc::SessionCommandStatus::Pending);
        (resolved, self.core.sessions.turn_in_flight(CHAT))
    }

    /// Wait until the commands are resolved and turn `turn` runs, past its
    /// `pre` checkpoint (written once the run starts, after it shows as
    /// running).
    async fn wait_running(&self, turn: u32) {
        let pre = format!("refs/loams/checkpoints/{CHAT}/turn/{turn}/pre");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let (resolved, running) = self.settled_and_running();
            let written = git_output(&self.repo, &["rev-parse", "--verify", "--quiet", &pre])
                .await
                .status
                .success();
            if resolved && running && written {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for turn {turn} to run"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Wait until the commands are resolved and no turn is in flight.
    async fn wait_idle(&self) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while self.settled_and_running() != (true, false) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for the thread to settle"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn list(&self) -> Value {
        self.client
            .call(methods::LIST_CHECKPOINTS, json!({ "chatId": CHAT }))
            .await
            .unwrap()
    }

    async fn checksum(&self) -> String {
        let diff = self
            .client
            .call(
                methods::GET_CHECKOUT_DIFF,
                json!({ "cwd": self.repo.to_string_lossy() }),
            )
            .await
            .unwrap();
        diff["checksum"].as_str().unwrap().to_owned()
    }

    async fn restore(&self, turn_id: &str, checksum: &str, scope: &str) -> Result<Value, String> {
        self.client
            .call(
                methods::RESTORE_CHECKPOINT,
                json!({
                    "chatId": CHAT,
                    "turnId": turn_id,
                    "expectedChecksum": checksum,
                    "scope": scope,
                }),
            )
            .await
            .map_err(|e| e.to_string())
    }

    fn read(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(self.repo.join(path)).ok()
    }

    fn entry_ids(&self, chat_id: &str) -> Vec<String> {
        self.core
            .doc_host
            .open(chat_id)
            .unwrap()
            .doc()
            .read_entries()
            .unwrap()
            .into_iter()
            .map(|entry| entry.id)
            .collect()
    }
}

fn repos(dir: &Path) -> Repos {
    Repos::new(&dir.join("data"), "device-test")
}

#[tokio::test]
async fn checkpoint_written_per_turn() {
    let daemon = Daemon::start().await;
    let head = git(&daemon.repo, &["rev-parse", "HEAD"]).await;
    daemon.turn(1, "msg-1").await;
    daemon.turn(2, "msg-2").await;

    for turn in 1..=2 {
        for phase in ["pre", "post"] {
            let git_ref = format!("refs/loams/checkpoints/{CHAT}/turn/{turn}/{phase}");
            // Each checkpoint is a commit whose parent is the user's HEAD.
            let parent = git(&daemon.repo, &["rev-parse", &format!("{git_ref}^")]).await;
            assert_eq!(parent, head, "{git_ref}");
        }
    }
    // A turn starts where the one before it ended.
    let pre2 = git(
        &daemon.repo,
        &[
            "rev-parse",
            &format!("refs/loams/checkpoints/{CHAT}/turn/2/pre^{{tree}}"),
        ],
    )
    .await;
    let post1 = git(
        &daemon.repo,
        &[
            "rev-parse",
            &format!("refs/loams/checkpoints/{CHAT}/turn/1/post^{{tree}}"),
        ],
    )
    .await;
    assert_eq!(pre2, post1);
    let pre1_notes = git(
        &daemon.repo,
        &[
            "show",
            &format!("refs/loams/checkpoints/{CHAT}/turn/1/pre:notes.txt"),
        ],
    )
    .await;
    assert_eq!(pre1_notes, "first");

    let list = daemon.list().await;
    let checkpoints = list["checkpoints"].as_array().unwrap();
    assert_eq!(checkpoints.len(), 2, "{list}");
    assert_eq!(checkpoints[0]["turnId"], "msg-1");
    assert_eq!(
        checkpoints[0]["ref"],
        format!("refs/loams/checkpoints/{CHAT}/turn/1/pre")
    );
    // Turn 1 changed notes.txt and created src/new.txt; turn 2 changed both.
    assert_eq!(checkpoints[0]["filesChanged"], 2);
    assert_eq!(checkpoints[1]["turnId"], "msg-2");
    assert_eq!(checkpoints[1]["filesChanged"], 2);
    assert!(checkpoints[0]["createdAtMs"].as_i64().unwrap() > 0);
}

#[tokio::test]
async fn same_message_is_one_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(tmp.path()).await;
    let checkpoints = Checkpoints::new(repos(tmp.path()));
    assert_eq!(
        checkpoints
            .capture_turn_start("chat-a", &repo, "msg-1")
            .await
            .unwrap(),
        Some(1)
    );
    // A retry of the same message (a steer that fell back to a fresh
    // dispatch, a startup retry) is the same turn.
    assert_eq!(
        checkpoints
            .capture_turn_start("chat-a", &repo, "msg-1")
            .await
            .unwrap(),
        Some(1)
    );
    assert_eq!(
        checkpoints
            .capture_turn_start("chat-a", &repo, "msg-2")
            .await
            .unwrap(),
        Some(2)
    );
    // The numbering is read back from the refs: a new store (a restarted
    // daemon) goes on from there.
    let restarted = Checkpoints::new(repos(tmp.path()));
    assert_eq!(
        restarted
            .capture_turn_start("chat-a", &repo, "msg-3")
            .await
            .unwrap(),
        Some(3)
    );
    // Ids that cannot name a ref get no checkpoint.
    assert_eq!(
        checkpoints
            .capture_turn_start("../escape", &repo, "msg-1")
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn checkpoint_does_not_touch_index_or_head() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(tmp.path()).await;
    std::fs::write(repo.join("staged.txt"), "staged\n").unwrap();
    git(&repo, &["add", "staged.txt"]).await;
    std::fs::write(repo.join("notes.txt"), "first\nunstaged\n").unwrap();
    std::fs::write(repo.join("untracked.txt"), "untracked\n").unwrap();
    // Let git settle the index's stat cache before it is recorded.
    let status_before = git(&repo, &["status", "--porcelain"]).await;
    let index_before = std::fs::read(repo.join(".git/index")).unwrap();
    let head_before = git(&repo, &["rev-parse", "HEAD"]).await;
    let symbolic_before = git(&repo, &["symbolic-ref", "HEAD"]).await;
    let refs_before = git(
        &repo,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/heads",
            "refs/tags",
        ],
    )
    .await;

    let checkpoints = Checkpoints::new(repos(tmp.path()));
    let turn = checkpoints
        .capture_turn_start("chat-a", &repo, "msg-1")
        .await
        .unwrap();
    assert_eq!(turn, Some(1));

    assert_eq!(
        std::fs::read(repo.join(".git/index")).unwrap(),
        index_before
    );
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]).await, head_before);
    assert_eq!(git(&repo, &["symbolic-ref", "HEAD"]).await, symbolic_before);
    assert_eq!(
        git(
            &repo,
            &[
                "for-each-ref",
                "--format=%(refname) %(objectname)",
                "refs/heads",
                "refs/tags"
            ]
        )
        .await,
        refs_before
    );
    assert_eq!(git(&repo, &["status", "--porcelain"]).await, status_before);
    // The temporary index is gone.
    let leftovers: Vec<_> = std::fs::read_dir(repo.join(".git"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("loams-checkpoint-index")
        })
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    // The checkpoint holds the working tree, staged or not.
    let pre = "refs/loams/checkpoints/chat-a/turn/1/pre";
    assert_eq!(
        git(&repo, &["show", &format!("{pre}:notes.txt")]).await,
        "first\nunstaged"
    );
    assert_eq!(
        git(&repo, &["show", &format!("{pre}:staged.txt")]).await,
        "staged"
    );
    assert_eq!(
        git(&repo, &["show", &format!("{pre}:untracked.txt")]).await,
        "untracked"
    );
}

#[tokio::test]
async fn checkpoint_includes_untracked_not_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(tmp.path()).await;
    std::fs::create_dir_all(repo.join("dir")).unwrap();
    std::fs::write(repo.join("dir/new.txt"), "new\n").unwrap();
    std::fs::write(repo.join("debug.log"), "noise\n").unwrap();
    std::fs::write(repo.join("dir/trace.log"), "noise\n").unwrap();

    let checkpoints = Checkpoints::new(repos(tmp.path()));
    checkpoints
        .capture_turn_start("chat-a", &repo, "msg-1")
        .await
        .unwrap();
    let files = git(
        &repo,
        &[
            "ls-tree",
            "-r",
            "--name-only",
            "refs/loams/checkpoints/chat-a/turn/1/pre",
        ],
    )
    .await;
    let files: Vec<&str> = files.lines().collect();
    assert_eq!(files, vec![".gitignore", "dir/new.txt", "notes.txt"]);
}

#[tokio::test]
async fn restore_worktree_matches_pre() {
    let daemon = Daemon::start().await;
    let head = git(&daemon.repo, &["rev-parse", "HEAD"]).await;
    daemon.turn(1, "msg-1").await;
    daemon.turn(2, "msg-2").await;
    // Work after the turns: an untracked file, and an ignored one.
    std::fs::write(daemon.repo.join("scratch.txt"), "later\n").unwrap();
    std::fs::write(daemon.repo.join("build.log"), "kept\n").unwrap();

    // Back to the start of turn 2: turn 1's edits only.
    let reply = daemon
        .restore("msg-2", &daemon.checksum().await, "worktree")
        .await
        .unwrap();
    let pre2 = git(
        &daemon.repo,
        &[
            "rev-parse",
            &format!("refs/loams/checkpoints/{CHAT}/turn/2/pre"),
        ],
    )
    .await;
    assert_eq!(reply["restoredTo"], pre2);
    assert!(reply.get("forkedChatId").is_none(), "{reply}");
    assert_eq!(daemon.read("notes.txt").unwrap(), "first\nmock edit 1\n");
    assert_eq!(daemon.read("src/new.txt").unwrap(), "mock edit 1\n");
    assert_eq!(daemon.read("scratch.txt"), None);
    assert_eq!(daemon.read("build.log").unwrap(), "kept\n");

    // Back to the start of turn 1: the commit as it was.
    daemon
        .restore("msg-1", &daemon.checksum().await, "worktree")
        .await
        .unwrap();
    assert_eq!(daemon.read("notes.txt").unwrap(), "first\n");
    assert_eq!(daemon.read("src/new.txt"), None);
    assert_eq!(daemon.read("build.log").unwrap(), "kept\n");
    assert_eq!(git(&daemon.repo, &["status", "--porcelain"]).await, "");
    // HEAD never moves.
    assert_eq!(git(&daemon.repo, &["rev-parse", "HEAD"]).await, head);
    assert_eq!(
        git(&daemon.repo, &["symbolic-ref", "HEAD"]).await,
        "refs/heads/main"
    );
}

#[tokio::test]
async fn restore_refuses_stale_checksum() {
    let daemon = Daemon::start().await;
    daemon.turn(1, "msg-1").await;
    let checksum = daemon.checksum().await;
    // The checkout moves after the person confirmed.
    std::fs::write(daemon.repo.join("notes.txt"), "edited by hand\n").unwrap();

    let error = daemon
        .restore("msg-1", &checksum, "worktree")
        .await
        .unwrap_err();
    assert!(error.contains("stale_diff"), "{error}");
    assert_eq!(daemon.read("notes.txt").unwrap(), "edited by hand\n");
    assert_eq!(daemon.read("src/new.txt").unwrap(), "mock edit 1\n");

    // No checksum is no confirmation.
    let error = daemon.restore("msg-1", "", "worktree").await.unwrap_err();
    assert!(error.contains("expectedChecksum is required"), "{error}");
    // A turn with no checkpoint.
    let error = daemon
        .restore("msg-unknown", &daemon.checksum().await, "worktree")
        .await
        .unwrap_err();
    assert!(error.contains("checkpoint_missing"), "{error}");
    assert_eq!(daemon.read("notes.txt").unwrap(), "edited by hand\n");
}

/// What the Connect/ndjson parity suite compares after Steer and Interrupt
/// (its 0.1.1 CI flake): a queued command answers before the daemon applies
/// it, so a restore sent right after it can meet the turn it starts. While a
/// turn is in flight the restore is refused as running; once the commands
/// are resolved and the turn is over, the same restore reaches the
/// checkpoints, and a Steer to an idle thread has started a turn of its own.
#[tokio::test]
async fn restore_waits_for_the_queued_turn_to_end() {
    let daemon = Daemon::start_with(Arc::new(HeldMock(MockHarness { script: Vec::new() }))).await;
    let restore_missing = || async {
        daemon
            .restore("no-such-turn", "stale", "worktree")
            .await
            .unwrap_err()
    };

    daemon
        .queue(json!({ "kind": "run", "request": run_request(&daemon.repo), "messageId": "msg-1" }))
        .await;
    daemon.wait_running(1).await;
    let error = restore_missing().await;
    assert!(error.contains("The thread is running"), "{error}");

    daemon.queue(json!({ "kind": "interrupt" })).await;
    daemon.wait_idle().await;
    let error = restore_missing().await;
    assert!(error.contains("checkpoint_missing"), "{error}");

    // A steer to the idle thread is a new turn, with its own checkpoint.
    daemon
        .queue(json!({ "kind": "steer", "prompt": "Also this", "messageId": "msg-2" }))
        .await;
    daemon.wait_running(2).await;
    let error = restore_missing().await;
    assert!(error.contains("The thread is running"), "{error}");
    daemon.queue(json!({ "kind": "interrupt" })).await;
    daemon.wait_idle().await;
    let error = restore_missing().await;
    assert!(error.contains("checkpoint_missing"), "{error}");
    let list = daemon.list().await;
    let turns: Vec<&str> = list["checkpoints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["turnId"].as_str().unwrap())
        .collect();
    assert_eq!(turns, ["msg-1", "msg-2"]);
}

#[tokio::test]
async fn restore_with_transcript_forks_not_deletes() {
    let daemon = Daemon::start().await;
    daemon.turn(1, "msg-1").await;
    daemon.turn(2, "msg-2").await;
    let before = daemon.entry_ids(CHAT);
    assert!(before.iter().any(|id| id == "msg-2"), "{before:?}");

    let reply = daemon
        .restore("msg-2", &daemon.checksum().await, "worktree+transcript")
        .await
        .unwrap();
    let fork = reply["forkedChatId"].as_str().unwrap().to_owned();
    assert_ne!(fork, CHAT);
    assert_eq!(daemon.read("notes.txt").unwrap(), "first\nmock edit 1\n");

    // The thread keeps its whole history.
    assert_eq!(daemon.entry_ids(CHAT), before);
    // The fork holds what came before the turn, then the fork marker.
    let forked = daemon.entry_ids(&fork);
    let at = before.iter().position(|id| id == "msg-2").unwrap();
    assert_eq!(forked[..at], before[..at]);
    assert_eq!(forked.len(), at + 1, "{forked:?}");
    assert_eq!(forked[at], format!("fork:{fork}"));
    let row = daemon.core.workspace.chat(&fork).unwrap().unwrap();
    assert_eq!(row.parent_chat_id.as_deref(), Some(CHAT));
    assert_eq!(
        row.cwd,
        daemon.core.workspace.chat(CHAT).unwrap().unwrap().cwd
    );
}

#[tokio::test]
async fn refs_deleted_with_thread() {
    let daemon = Daemon::start().await;
    daemon.turn(1, "msg-1").await;
    // Another thread's checkpoint in the same repository.
    daemon
        .core
        .checkpoints
        .capture_turn_start("chat-other", &daemon.repo, "msg-x")
        .await
        .unwrap();
    let prefix = format!("refs/loams/checkpoints/{CHAT}/");
    assert!(
        !git(&daemon.repo, &["for-each-ref", &prefix])
            .await
            .is_empty()
    );

    daemon
        .client
        .call(
            methods::MUTATE,
            json!({ "op": "deleteChat", "chatId": CHAT }),
        )
        .await
        .unwrap();
    assert_eq!(git(&daemon.repo, &["for-each-ref", &prefix]).await, "");
    assert!(
        !git(
            &daemon.repo,
            &["for-each-ref", "refs/loams/checkpoints/chat-other/"]
        )
        .await
        .is_empty()
    );
}

#[test]
fn checkpoints_not_in_mcp_role() {
    for name in ["ListCheckpoints", "RestoreCheckpoint"] {
        let spec = method_spec(name).unwrap();
        assert_eq!(spec.role, Role::Owner, "{name}");
        assert!(spec.renderer, "{name}");
    }
}
