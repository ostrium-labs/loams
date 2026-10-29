//! SD1 Task 12: run groups (design §53 §5.5, §6.5 G12–G16).
//!
//! Real git repositories in temporary directories. Each member is a Mock
//! harness run whose edits (SD1 Task 8) depend on the variant's model, so
//! members change different files: `a` writes `a.txt` and `shared.txt`, `b`
//! writes `b.txt`, `c` writes `c.txt`. The edits are given through
//! `MockHarness::run_with`, so no test reads the Mock's environment knobs. A
//! gate holds runs open where a test needs members to stay running.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use loams_agentd_harness::mock::{MockEdits, MockHarness};
use loams_agentd_harness::{Harness, HarnessError, RunControls};
use loams_agentd_proto::groups::{CheckState, RunGroup, RunGroupState, RunMemberStatus};
use loams_agentd_proto::notify::NotificationKind;
use loams_agentd_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ProjectActionDraft, ProjectActionIcon,
    ReasoningLevel, RunRequest, SteeringMode,
};
use loams_agentd_rpc::{RpcClient, methods};
use loams_agentd_sessions::change_request_create::{ChangeRequestCreator, StaticForgeCredentials};
use loams_agentd_sessions::{
    EngineCore, EngineRpc, HarnessRegistry, RunGroupLimits, TerminalShell,
};
use serde_json::{Value, json};
use tokio::sync::watch;

const SPACE: &str = "space-groups";

/// The worktree root for every engine in this binary.
fn environment() -> &'static Path {
    static ROOT: OnceLock<tempfile::TempDir> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = tempfile::tempdir().unwrap();
        // SAFETY: test-only. Every test in this binary calls `environment()`
        // before it starts an engine, and all of them set the same value, so
        // a concurrent read sees either nothing or this.
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("LOAMS_DESKTOP_WORKTREES_DIR", root.path().join("worktrees"));
        }
        root
    })
    .path()
}

fn script() -> Vec<AgentEvent> {
    vec![
        AgentEvent::TextDelta {
            text: "Editing.".into(),
        },
        AgentEvent::Done {
            status: DoneStatus::Completed,
            result: None,
            error: None,
            session_id: None,
        },
    ]
}

/// The files a variant's model edits.
fn edits_of(model: Option<&str>) -> Vec<String> {
    match model {
        Some("a") => vec!["a.txt".into(), "shared.txt".into()],
        Some("b") => vec!["b.txt".into()],
        Some("c") => vec!["c.txt".into()],
        _ => vec!["notes.txt".into()],
    }
}

/// The Mock, editing by model, counting the runs in flight, and held open
/// while the gate is shut.
#[derive(Debug)]
struct GroupMock {
    gate: watch::Receiver<bool>,
    running: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
}

#[async_trait]
impl Harness for GroupMock {
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
        SteeringMode::TurnBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[ReasoningLevel::Medium]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let edits = MockEdits {
            paths: edits_of(request.model.as_deref()),
            approve: false,
        };
        let (running, peak) = (self.running.clone(), self.peak.clone());
        let ended = self.running.clone();
        let mut gate = self.gate.clone();
        let start = futures::stream::once(async move {
            let now = running.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            let _ = gate.wait_for(|open| *open).await;
        })
        .filter_map(|()| std::future::ready(None));
        let body = futures::stream::once(async move {
            MockHarness { script: script() }.run_with(request, controls, edits)
        })
        .flat_map(|run| match run {
            Ok(events) => events,
            Err(error) => futures::stream::iter(vec![Err(error)]).boxed(),
        })
        .inspect(move |event| {
            if matches!(event, Ok(AgentEvent::Done { .. })) {
                ended.fetch_sub(1, Ordering::SeqCst);
            }
        });
        Ok(start.chain(body).boxed())
    }
}

fn git_output(cwd: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git spawns")
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = git_output(cwd, args);
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A repository on `main` with a committed `notes.txt`, and its own git
/// identity (the daemon never sets one).
fn init_repo(repo: &Path) -> PathBuf {
    std::fs::create_dir_all(repo).unwrap();
    git(repo, &["init", "-b", "main"]);
    git(repo, &["config", "user.email", "t@example.com"]);
    git(repo, &["config", "user.name", "Test"]);
    std::fs::write(repo.join("notes.txt"), "first\n").unwrap();
    git(repo, &["add", "."]);
    git(repo, &["commit", "-m", "initial"]);
    std::fs::canonicalize(repo).unwrap()
}

struct Studio {
    tmp: tempfile::TempDir,
    /// `None` only while a restart swaps engines.
    engine: Option<EngineCore>,
    client: RpcClient,
    repo: PathBuf,
    gate: watch::Sender<bool>,
    running: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
}

impl Studio {
    /// An engine with one project, `repo`. `open` is the gate's first state.
    async fn start(open: bool) -> Self {
        environment();
        let tmp = tempfile::tempdir().unwrap();
        // Worktrees go to `<root>/<repository's name>/<generated name>`, and
        // the tests share the root and run at once: a name of its own each.
        let name = tmp.path().file_name().unwrap().to_owned();
        let repo = init_repo(&tmp.path().join(name));
        let (gate, _) = watch::channel(open);
        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let core = Self::assemble(tmp.path(), &gate, &running, &peak);
        core.workspace
            .create_space(
                SPACE,
                &core.device_id,
                &repo.to_string_lossy(),
                Some("Repo".into()),
                true,
            )
            .unwrap();
        let client = loams_agentd_rpc::memory_client(core.rpc_service());
        Self {
            tmp,
            engine: Some(core),
            client,
            repo,
            gate,
            running,
            peak,
        }
    }

    fn core(&self) -> &EngineCore {
        self.engine.as_ref().unwrap()
    }

    fn assemble(
        dir: &Path,
        gate: &watch::Sender<bool>,
        running: &Arc<AtomicUsize>,
        peak: &Arc<AtomicUsize>,
    ) -> EngineCore {
        let registry = HarnessRegistry::new();
        registry.register(Arc::new(GroupMock {
            gate: gate.subscribe(),
            running: running.clone(),
            peak: peak.clone(),
        }));
        let core =
            EngineCore::assemble(&dir.join("data"), Arc::new(registry), HarnessId::Mock).unwrap();
        // Checks run /bin/sh with a temporary HOME, never the developer's
        // login shell (plan DD1 T1-10).
        let home = dir.join("shell-home");
        std::fs::create_dir_all(&home).unwrap();
        core.terminals
            .set_shell(Some(TerminalShell::isolated("/bin/sh", &home)));
        core
    }

    /// Stop the engine and start a new one on the same data, as a daemon
    /// restart does.
    async fn restart(&mut self) {
        let old = self.engine.take().unwrap();
        old.shutdown().await;
        self.client = loams_agentd_rpc::memory_client(Arc::new(NoEngine));
        // The old engine holds the data directory's lock until it drops.
        drop(old);
        let core = Self::assemble(self.tmp.path(), &self.gate, &self.running, &self.peak);
        self.client = loams_agentd_rpc::memory_client(core.rpc_service());
        self.engine = Some(core);
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        self.client
            .call(method, params)
            .await
            .map_err(|e| e.to_string())
    }

    /// `CreateRunGroup` with one Mock variant per model.
    async fn create(&self, models: &[&str], check: Option<&str>) -> (String, Vec<String>) {
        let variants: Vec<Value> = models
            .iter()
            .map(|model| json!({"harness": "mock", "model": model}))
            .collect();
        let created = self
            .call(
                methods::CREATE_RUN_GROUP,
                json!({
                    "spaceId": SPACE,
                    "repoPath": self.repo.to_string_lossy(),
                    "base": "main",
                    "prompt": "Fix the build",
                    "variants": variants,
                    "checkActionId": check,
                    "approvalMode": "full-access",
                }),
            )
            .await
            .unwrap();
        let chat_ids = created["chatIds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|id| id.as_str().unwrap().to_owned())
            .collect();
        (created["groupId"].as_str().unwrap().to_owned(), chat_ids)
    }

    fn group(&self, group_id: &str) -> RunGroup {
        self.core()
            .run_groups
            .snapshot()
            .into_iter()
            .find(|group| group.id == group_id)
            .unwrap()
    }

    /// Wait until `ready` holds of the group.
    async fn wait(
        &self,
        group_id: &str,
        what: &str,
        ready: impl Fn(&RunGroup) -> bool,
    ) -> RunGroup {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let group = self.group(group_id);
            if ready(&group) {
                return group;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for {what}: {group:#?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn wait_ready(&self, group_id: &str) -> RunGroup {
        self.wait(group_id, "the group to be ready", |group| {
            group.state == RunGroupState::Ready
        })
        .await
    }

    async fn wait_running(&self, count: usize) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while self.running.load(Ordering::SeqCst) < count {
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for {count} runs"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// `GetCheckoutDiff` of a worktree: `(checkoutId, checksum)`.
    async fn diff(&self, cwd: &str) -> (String, String) {
        let diff = self
            .call(methods::GET_CHECKOUT_DIFF, json!({ "cwd": cwd }))
            .await
            .unwrap();
        (
            diff["checkoutId"].as_str().unwrap().to_owned(),
            diff["checksum"].as_str().unwrap().to_owned(),
        )
    }
}

/// Stands in for the engine while a studio is being (re)assembled.
#[derive(Debug)]
struct NoEngine;

#[async_trait]
impl loams_agentd_rpc::RpcService for NoEngine {
    async fn handle(
        &self,
        method: &str,
        _params: Value,
    ) -> Result<loams_agentd_rpc::RpcReply, loams_agentd_rpc::RpcError> {
        Err(loams_agentd_rpc::RpcError::UnknownMethod(method.into()))
    }
}

fn member<'a>(
    group: &'a RunGroup,
    chat_id: &str,
) -> &'a loams_agentd_proto::groups::RunGroupMember {
    group
        .members
        .iter()
        .find(|member| member.chat_id == chat_id)
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn group_creates_n_worktrees_from_base() {
    let studio = Studio::start(true).await;
    let base = git(&studio.repo, &["rev-parse", "main"]);
    let (group_id, chat_ids) = studio.create(&["a", "b", "c"], None).await;
    assert_eq!(chat_ids.len(), 3);

    let group = studio.group(&group_id);
    assert_eq!(group.base, "main");
    assert_eq!(group.base_sha, base);
    assert_eq!(group.space_id.as_deref(), Some(SPACE));
    assert_eq!(group.repo_path, studio.repo.to_string_lossy());
    let mut cwds = std::collections::BTreeSet::new();
    for (chat_id, member) in chat_ids.iter().zip(&group.members) {
        assert_eq!(&member.chat_id, chat_id, "members keep the variants' order");
        let chat = studio.core().workspace.chat(chat_id).unwrap().unwrap();
        assert_eq!(chat.cwd.as_deref(), Some(member.cwd.as_str()));
        assert_eq!(chat.branch.as_deref(), Some(member.branch.as_str()));
        assert_eq!(chat.space_id.as_deref(), Some(SPACE));
        assert_eq!(chat.config.unwrap().harness, HarnessId::Mock);
        assert!(member.branch.starts_with("loams-desktop/"));
        let cwd = Path::new(&member.cwd);
        assert!(cwd.starts_with(environment()), "under the worktree root");
        assert_ne!(cwd, studio.repo);
        assert_eq!(git(cwd, &["rev-parse", "HEAD"]), base, "made from the base");
        assert!(cwds.insert(member.cwd.clone()), "one worktree each");
    }

    let group = studio.wait_ready(&group_id).await;
    for (member, file) in group.members.iter().zip(["a.txt", "b.txt", "c.txt"]) {
        assert_eq!(member.status, RunMemberStatus::Done);
        assert_eq!(
            std::fs::read_to_string(Path::new(&member.cwd).join(file)).unwrap(),
            "mock edit 1\n",
            "the same run, in the member's own worktree"
        );
        assert!(
            !studio.repo.join(file).exists(),
            "the main checkout is untouched"
        );
    }

    for bad in [
        json!({"repoPath": studio.repo.to_string_lossy(), "base": "-x", "prompt": "p",
               "variants": [{"harness": "mock"}]}),
        json!({"repoPath": studio.repo.to_string_lossy(), "base": "main", "prompt": "p",
               "variants": []}),
        json!({"repoPath": studio.repo.to_string_lossy(), "base": "nope", "prompt": "p",
               "variants": [{"harness": "mock"}]}),
        json!({"repoPath": "/nowhere", "base": "main", "prompt": "p",
               "variants": [{"harness": "mock"}]}),
    ] {
        let error = studio
            .call(methods::CREATE_RUN_GROUP, bad.clone())
            .await
            .unwrap_err();
        assert!(error.contains("bad params"), "{bad}: {error}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn group_respects_concurrency_limits() {
    let studio = Studio::start(false).await;
    studio.core().run_groups.set_limits(RunGroupLimits {
        max_running_per_group: 2,
        max_running: 3,
    });
    let (first, _) = studio.create(&["a", "b", "c"], None).await;
    let (second, _) = studio.create(&["a", "b"], None).await;

    studio.wait_running(3).await;
    // Give a fourth run every chance to start.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        studio.peak.load(Ordering::SeqCst),
        3,
        "never more than max_running"
    );
    let count = |group: &RunGroup, status: RunMemberStatus| {
        group.members.iter().filter(|m| m.status == status).count()
    };
    let one = studio.group(&first);
    assert_eq!(
        count(&one, RunMemberStatus::Running),
        2,
        "max_running_per_group"
    );
    assert_eq!(count(&one, RunMemberStatus::Queued), 1);
    assert_eq!(one.state, RunGroupState::Running);
    let two = studio.group(&second);
    assert_eq!(
        count(&two, RunMemberStatus::Running),
        1,
        "the daemon's limit"
    );
    assert_eq!(count(&two, RunMemberStatus::Queued), 1);

    studio.gate.send_replace(true);
    for group_id in [&first, &second] {
        let group = studio.wait_ready(group_id).await;
        assert!(
            group
                .members
                .iter()
                .all(|m| m.status == RunMemberStatus::Done)
        );
    }
    assert_eq!(studio.peak.load(Ordering::SeqCst), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn group_stats_follow_member_diffs() {
    let studio = Studio::start(true).await;
    let (group_id, chats) = studio.create(&["a", "b"], None).await;
    studio.wait_ready(&group_id).await;
    studio.core().diff_sync.reconcile_now().await;
    let group = studio
        .wait(&group_id, "the members' diff stats", |group| {
            let (a, b) = (member(group, &chats[0]), member(group, &chats[1]));
            a.files_changed == 2 && b.files_changed == 1
        })
        .await;
    let (a, b) = (member(&group, &chats[0]), member(&group, &chats[1]));
    assert_eq!((a.additions, a.deletions), (2, 0));
    assert_eq!((b.additions, b.deletions), (1, 0));
    assert!(a.elapsed_ms >= 0 && a.started_at_ms.is_some());

    // A change in b's worktree reaches its stats.
    std::fs::write(Path::new(&b.cwd).join("more.txt"), "one\ntwo\n").unwrap();
    studio.core().diff_sync.reconcile_now().await;
    let group = studio
        .wait(&group_id, "b's new file in its stats", |group| {
            member(group, &chats[1]).files_changed == 2
        })
        .await;
    assert_eq!(member(&group, &chats[1]).additions, 3);
    assert_eq!(member(&group, &chats[0]).files_changed, 2, "a is unchanged");
}

#[tokio::test(flavor = "multi_thread")]
async fn comparison_matrix_and_pair_diff() {
    let studio = Studio::start(true).await;
    let (group_id, chats) = studio.create(&["a", "b"], None).await;
    let group = studio.wait_ready(&group_id).await;

    let compared = studio
        .call(
            methods::GET_RUN_GROUP_COMPARISON,
            json!({"groupId": group_id, "a": chats[0], "b": chats[1]}),
        )
        .await
        .unwrap();
    let rows: Vec<(String, Vec<String>)> = compared["matrix"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["path"].as_str().unwrap().to_owned(),
                row["perMember"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|cell| {
                        assert_eq!(cell["additions"], 1);
                        cell["chatId"].as_str().unwrap().to_owned()
                    })
                    .collect(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            ("a.txt".to_owned(), vec![chats[0].clone()]),
            ("b.txt".to_owned(), vec![chats[1].clone()]),
            ("shared.txt".to_owned(), vec![chats[0].clone()]),
        ]
    );
    let pair = &compared["pairDiff"];
    let mut files: Vec<(String, String)> = pair["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["path"].as_str().unwrap().to_owned(),
                f["status"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    files.sort();
    // a's `a.txt` and b's `b.txt` hold the same line, so git sees a rename.
    assert_eq!(
        files,
        vec![
            ("b.txt".to_owned(), "renamed".to_owned()),
            ("shared.txt".to_owned(), "deleted".to_owned()),
        ],
        "{pair}"
    );
    assert!(pair["patch"].as_str().unwrap().contains("-mock edit 1"));
    assert_eq!(pair["additions"], 0);
    assert_eq!(pair["deletions"], 1);
    assert_eq!(pair["cwd"], group.members[0].cwd.as_str());

    // One path only.
    let one = studio
        .call(
            methods::GET_RUN_GROUP_COMPARISON,
            json!({"groupId": group_id, "a": chats[0], "b": chats[1], "path": "b.txt"}),
        )
        .await
        .unwrap();
    assert_eq!(one["matrix"].as_array().unwrap().len(), 1);
    assert_eq!(one["matrix"][0]["path"], "b.txt");
    assert_eq!(one["pairDiff"]["files"].as_array().unwrap().len(), 1);
    assert_eq!(one["pairDiff"]["files"][0]["path"], "b.txt");

    // Without `b`: from the base to a's result.
    let alone = studio
        .call(
            methods::GET_RUN_GROUP_COMPARISON,
            json!({"groupId": group_id, "a": chats[0]}),
        )
        .await
        .unwrap();
    assert_eq!(alone["pairDiff"]["files"].as_array().unwrap().len(), 2);

    // Nothing was committed or staged to compute it.
    for member in &group.members {
        let cwd = Path::new(&member.cwd);
        assert_eq!(git(cwd, &["rev-parse", "HEAD"]), group.base_sha);
        assert!(
            git_output(cwd, &["diff", "--cached", "--quiet"])
                .status
                .success()
        );
    }
    let error = studio
        .call(
            methods::GET_RUN_GROUP_COMPARISON,
            json!({"groupId": group_id, "a": chats[0], "path": "../x"}),
        )
        .await
        .unwrap_err();
    assert!(error.contains("bad params"), "{error}");
}

#[tokio::test(flavor = "multi_thread")]
async fn group_check_records_exit_codes() {
    let studio = Studio::start(true).await;
    let snapshot = studio
        .core()
        .project_actions
        .upsert(
            SPACE,
            &studio.repo,
            None,
            ProjectActionDraft {
                name: "Check".into(),
                command: "echo checking the result; test -f a.txt".into(),
                icon: ProjectActionIcon::Test,
                run_on_worktree_create: false,
            },
        )
        .unwrap();
    let action_id = snapshot.actions[0].id.clone();
    let (group_id, chats) = studio.create(&["a", "b"], Some(&action_id)).await;
    studio.wait_ready(&group_id).await;

    let runs = studio
        .call(methods::RUN_GROUP_CHECK, json!({ "groupId": group_id }))
        .await
        .unwrap();
    assert_eq!(runs["runs"].as_array().unwrap().len(), 2);
    let group = studio
        .wait(&group_id, "both checks to finish", |group| {
            group.members.iter().all(|m| {
                m.check
                    .as_ref()
                    .is_some_and(|c| matches!(c.state, CheckState::Passed | CheckState::Failed))
            })
        })
        .await;
    let a = member(&group, &chats[0]).check.clone().unwrap();
    let b = member(&group, &chats[1]).check.clone().unwrap();
    assert_eq!((a.state, a.exit_code), (CheckState::Passed, Some(0)));
    assert_eq!((b.state, b.exit_code), (CheckState::Failed, Some(1)));
    // The group's notifications (SD1 Task 14): ready once, and the failed
    // check, with the member that failed.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let notifications = loop {
        let recent = studio.core().notifications.recent();
        if recent
            .iter()
            .any(|n| n.kind == NotificationKind::CheckFailed)
        {
            break recent;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no check_failed notification: {recent:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let of = |kind| {
        notifications
            .iter()
            .filter(|n| n.kind == kind && n.group_id.as_deref() == Some(group_id.as_str()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        of(NotificationKind::GroupReady).len(),
        1,
        "{notifications:?}"
    );
    let failed = of(NotificationKind::CheckFailed);
    assert_eq!(failed.len(), 1, "{notifications:?}");
    assert_eq!(failed[0].chat_id.as_deref(), Some(chats[1].as_str()));
    assert!(
        a.log_tail
            .iter()
            .any(|line| line.contains("checking the result")),
        "{:?}",
        a.log_tail
    );
    assert_eq!(
        a.terminal_id.as_deref(),
        runs["runs"][0]["terminalId"].as_str()
    );

    // One member only.
    let one = studio
        .call(
            methods::RUN_GROUP_CHECK,
            json!({ "groupId": group_id, "chatId": chats[1] }),
        )
        .await
        .unwrap();
    assert_eq!(one["runs"].as_array().unwrap().len(), 1);
    assert_eq!(one["runs"][0]["chatId"], chats[1].as_str());
}

#[tokio::test(flavor = "multi_thread")]
async fn resolve_merge_archives_others() {
    let studio = Studio::start(true).await;
    let (group_id, chats) = studio.create(&["a", "b"], None).await;
    let group = studio.wait_ready(&group_id).await;
    let (a, b) = (
        member(&group, &chats[0]).clone(),
        member(&group, &chats[1]).clone(),
    );
    let (_, checksum) = studio.diff(&a.cwd).await;
    let base = group.base_sha.clone();
    let resolve = |checksum: &str| {
        json!({
            "groupId": group_id,
            "winnerChatId": a.chat_id,
            "action": "merge",
            "merge": {
                "repoPath": studio.repo.to_string_lossy(),
                "source": a.branch,
                "target": "main",
                "strategy": "merge",
                "message": "Take variant a",
                "expectedSourceHead": base,
            },
            "archiveOthers": true,
            "removeOtherWorktrees": true,
            "expectedChecksum": checksum,
        })
    };

    // The person's confirmation is checked first: nothing moves on a stale one.
    let error = studio
        .call(methods::RESOLVE_RUN_GROUP, resolve("stale"))
        .await
        .unwrap_err();
    assert!(error.contains("stale_diff"), "{error}");
    assert_eq!(git(&studio.repo, &["rev-parse", "main"]), base);
    assert!(Path::new(&b.cwd).is_dir());

    let resolved = studio
        .call(methods::RESOLVE_RUN_GROUP, resolve(&checksum))
        .await
        .unwrap();
    let main = git(&studio.repo, &["rev-parse", "main"]);
    assert_eq!(resolved["sha"], main.as_str());
    assert_ne!(main, base);
    assert_eq!(
        std::fs::read_to_string(studio.repo.join("a.txt")).unwrap(),
        "mock edit 1\n",
        "the winner's work is merged into main"
    );
    assert!(!studio.repo.join("b.txt").exists());
    assert!(
        git_output(&studio.repo, &["diff", "--quiet", "HEAD"])
            .status
            .success()
    );

    let b_chat = studio.core().workspace.chat(&b.chat_id).unwrap().unwrap();
    assert!(b_chat.archived, "the other member is archived");
    assert!(!Path::new(&b.cwd).exists(), "and its worktree removed");
    let a_chat = studio.core().workspace.chat(&a.chat_id).unwrap().unwrap();
    assert!(!a_chat.archived);

    let group = studio.group(&group_id);
    assert_eq!(group.state, RunGroupState::Resolved);
    let resolution = group.resolution.clone().unwrap();
    assert_eq!(resolution.winner_chat_id, a.chat_id);
    assert_eq!(resolution.sha.as_deref(), Some(main.as_str()));
    assert!(member(&group, &b.chat_id).archived);

    let again = studio
        .call(methods::RESOLVE_RUN_GROUP, resolve(&checksum))
        .await
        .unwrap_err();
    assert!(again.contains("already resolved"), "{again}");
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn resolve_pr_pushes_and_opens() {
    use std::os::unix::fs::PermissionsExt;

    let studio = Studio::start(true).await;
    // GitHub names the repository; pushes go to a bare repository.
    let remote = studio.tmp.path().join("remote.git");
    git(
        studio.tmp.path(),
        &["init", "--bare", "-b", "main", &remote.to_string_lossy()],
    );
    git(
        &studio.repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/widgets.git",
        ],
    );
    git(
        &studio.repo,
        &["config", "remote.origin.pushurl", &remote.to_string_lossy()],
    );
    let bin = studio.tmp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = studio.tmp.path().join("gh-args");
    let gh = bin.join("gh");
    std::fs::write(
        &gh,
        format!(
            "#!/bin/sh\nfor arg in \"$@\"; do printf '%s\\n' \"$arg\" >> '{}'; done\n\
             echo 'https://github.com/acme/widgets/pull/12'\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let core = studio.core();
    let rpc = EngineRpc::new(
        core.sessions.clone(),
        core.doc_host.clone(),
        core.workspace.clone(),
        core.registry.clone(),
        core.repos.clone(),
        core.workspace_files.clone(),
        core.terminals.clone(),
        core.project_actions.clone(),
        core.change_requests.clone(),
        core.diff_sync.clone(),
        core.uploads.clone(),
        core.agent_accounts.clone(),
        core.workspace_scope(),
    )
    .with_checkpoints(core.checkpoints.clone())
    .with_run_groups(core.run_groups.clone())
    .with_change_request_creator(
        ChangeRequestCreator::new(Arc::new(StaticForgeCredentials::new(None)))
            .with_gh_path(vec![bin]),
    );
    let client = loams_agentd_rpc::memory_client(Arc::new(rpc));

    let (group_id, chats) = studio.create(&["a", "b"], None).await;
    let group = studio.wait_ready(&group_id).await;
    let (a, b) = (
        member(&group, &chats[0]).clone(),
        member(&group, &chats[1]).clone(),
    );
    let (_, checksum) = studio.diff(&a.cwd).await;
    let resolved = client
        .call(
            methods::RESOLVE_RUN_GROUP,
            json!({
                "groupId": group_id,
                "winnerChatId": a.chat_id,
                "action": "pr",
                "pr": {
                    "cwd": "",
                    "provider": "github",
                    "base": "main",
                    "head": "",
                    "title": "Fix the build",
                    "body": "Variant a.",
                },
                "archiveOthers": true,
                "expectedChecksum": checksum,
            }),
        )
        .await
        .unwrap();
    assert_eq!(resolved["url"], "https://github.com/acme/widgets/pull/12");

    // The winner's work was committed with the person's identity and pushed.
    let pushed = git(&remote, &["rev-parse", &format!("refs/heads/{}", a.branch)]);
    assert_eq!(pushed, git(Path::new(&a.cwd), &["rev-parse", "HEAD"]));
    assert_eq!(
        git(&remote, &["show", &format!("{pushed}:a.txt")]),
        "mock edit 1"
    );
    assert_eq!(
        git(&remote, &["log", "-1", "--format=%ae %s", &pushed]),
        "t@example.com Fix the build"
    );
    let args = std::fs::read_to_string(&log).unwrap();
    assert!(args.contains(&format!("--head={}", a.branch)), "{args}");
    assert!(args.contains("--base=main"), "{args}");
    assert!(args.contains("--title=Fix the build"), "{args}");

    assert!(
        studio
            .core()
            .workspace
            .chat(&b.chat_id)
            .unwrap()
            .unwrap()
            .archived
    );
    assert!(
        Path::new(&b.cwd).is_dir(),
        "worktrees stay without removeOtherWorktrees"
    );
    let group = studio.group(&group_id);
    assert_eq!(group.state, RunGroupState::Resolved);
    assert_eq!(
        group.resolution.unwrap().url.as_deref(),
        Some("https://github.com/acme/widgets/pull/12")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn resolve_refuses_while_running() {
    let studio = Studio::start(false).await;
    let (group_id, chats) = studio.create(&["a", "b"], None).await;
    studio.wait_running(2).await;
    let keep = |force: bool| {
        json!({
            "groupId": group_id,
            "winnerChatId": chats[0],
            "action": "keep",
            "force": force,
        })
    };
    let error = studio
        .call(methods::RESOLVE_RUN_GROUP, keep(false))
        .await
        .unwrap_err();
    assert!(error.starts_with("group_not_ready"), "{error}");
    let group = studio.group(&group_id);
    assert_eq!(group.state, RunGroupState::Running);
    assert!(group.resolution.is_none());

    // `force` resolves anyway and stops the members.
    let resolved = studio
        .call(methods::RESOLVE_RUN_GROUP, keep(true))
        .await
        .unwrap();
    assert!(resolved["result"].as_str().unwrap().contains("kept"));
    let group = studio.group(&group_id);
    assert_eq!(group.state, RunGroupState::Resolved);
    assert!(
        member(&group, &chats[0]).detached,
        "keep detaches the winner"
    );
    studio.gate.send_replace(true);
}

#[tokio::test(flavor = "multi_thread")]
async fn group_survives_restart() {
    let mut studio = Studio::start(false).await;
    studio.core().run_groups.set_limits(RunGroupLimits {
        max_running_per_group: 1,
        max_running: 8,
    });
    let (group_id, chats) = studio.create(&["a", "b"], None).await;
    studio.wait_running(1).await;
    let before = studio.group(&group_id);
    assert_eq!(member(&before, &chats[1]).status, RunMemberStatus::Queued);

    studio.gate.send_replace(true);
    studio.restart().await;
    let after = studio.group(&group_id);
    assert_eq!(after.id, before.id);
    assert_eq!(after.base_sha, before.base_sha);
    assert_eq!(
        after.members.iter().map(|m| &m.chat_id).collect::<Vec<_>>(),
        chats.iter().collect::<Vec<_>>()
    );

    // The queued member is sent by the new daemon, and the group settles.
    let group = studio.wait_ready(&group_id).await;
    let b = member(&group, &chats[1]);
    assert_eq!(b.status, RunMemberStatus::Done);
    assert_eq!(
        std::fs::read_to_string(Path::new(&b.cwd).join("b.txt")).unwrap(),
        "mock edit 1\n"
    );
    assert!(member(&group, &chats[0]).status != RunMemberStatus::Queued);
}

/// A member whose run command the last daemon took, but whose turn start it
/// never recorded, is settled by the next daemon instead of staying running
/// (#970). Here an accepted harness update holds the run before its turn
/// starts until the daemon stops; in #970 the daemon stopped before its
/// scheduler had noted the turn's start.
#[tokio::test(flavor = "multi_thread")]
async fn taken_member_settles_after_restart() {
    let mut studio = Studio::start(true).await;
    studio.core().registry.begin_update(HarnessId::Mock);
    let (group_id, chats) = studio.create(&["a"], None).await;
    // The member's run is dispatched (its command taken), its turn held.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !studio.core().sessions.turn_in_flight(&chats[0]) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for the member's run to be dispatched"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let before = studio.group(&group_id);
    assert_eq!(member(&before, &chats[0]).status, RunMemberStatus::Running);
    assert_eq!(studio.running.load(Ordering::SeqCst), 0);

    studio.restart().await;
    let group = studio.wait_ready(&group_id).await;
    assert_eq!(member(&group, &chats[0]).status, RunMemberStatus::Done);
    // Settled as the run the last daemon started, not run again.
    assert_eq!(studio.peak.load(Ordering::SeqCst), 0);
}
