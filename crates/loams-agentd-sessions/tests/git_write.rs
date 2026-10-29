//! SD1 Task 9: worktrees and git writes (design §53 §6.5 G1–G5, G21).
//!
//! Every test runs against real git repositories in a temporary directory.
//! The engine tests produce their changes with the Mock harness's edit knob
//! (`LOAMS_DESKTOP_MOCK_EDIT`, SD1 Task 8) in a thread's own worktree; the
//! change-request tests talk only to a local fake Forgejo and a fake `gh`.
//!
//! The knob is read from the process environment, so every test in this
//! binary sets the same values once, before any engine reads them.
#![allow(clippy::unwrap_used)]

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use loams_agentd_doc::{MessageRole, MessageStatus, SessionCommandPayload};
use loams_agentd_harness::mock::MockHarness;
use loams_agentd_proto::rpc::{Role, method_spec};
use loams_agentd_proto::{
    AgentEvent, ChangeRequestProvider, ChangeRequestState, CommitWorktreeParams,
    CreateChangeRequestParams, DoneStatus, HarnessId, MergeBranchParams, MergeStrategy,
    PushBranchParams, RunRequest, SandboxLevel, WorktreeInfo, WorktreeSpec,
};
use loams_agentd_rpc::{RpcClient, methods};
use loams_agentd_sessions::change_request_create::{
    ChangeRequestCreator, ForgejoCredential, StaticForgeCredentials,
};
use loams_agentd_sessions::git_write::{self, GitWriteError};
use loams_agentd_sessions::{EngineCore, HarnessRegistry, Repos, diff_sync};

const NAME: &str = "Loams Test";
const EMAIL: &str = "test@loams.invalid";

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// The knob and the worktree root, set once for this process.
fn environment() -> &'static Path {
    static ROOT: OnceLock<tempfile::TempDir> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = tempfile::tempdir().unwrap();
        // SAFETY: test-only. Every test in this binary calls `environment()`
        // before it starts an engine or spawns git, and all of them set the
        // same values, so a concurrent read sees either nothing or these.
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("LOAMS_DESKTOP_MOCK_EDIT", "notes.txt,src/new.txt");
            std::env::set_var("LOAMS_DESKTOP_WORKTREES_DIR", root.path().join("worktrees"));
        }
        root
    })
    .path()
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// The user's git identity, in the repository's own config, and no signing.
fn configure(repo: &Path) {
    git(repo, &["config", "user.name", NAME]);
    git(repo, &["config", "user.email", EMAIL]);
    git(repo, &["config", "commit.gpgsign", "false"]);
    git(repo, &["config", "tag.gpgsign", "false"]);
}

/// A repository on `main` with a committed `notes.txt`.
fn init_repo(dir: &Path) -> PathBuf {
    environment();
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    configure(dir);
    std::fs::write(dir.join("notes.txt"), "first\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "initial"]);
    dir.canonicalize().unwrap()
}

fn commit_file(repo: &Path, file: &str, text: &str, message: &str) -> String {
    if let Some(parent) = Path::new(file).parent() {
        std::fs::create_dir_all(repo.join(parent)).unwrap();
    }
    std::fs::write(repo.join(file), text).unwrap();
    git(repo, &["add", "--", file]);
    git(repo, &["commit", "-q", "-m", message]);
    git(repo, &["rev-parse", "HEAD"])
}

fn text(error: GitWriteError) -> String {
    error.to_string()
}

async fn checksum(root: &Path) -> String {
    let repos = Repos::new(&root.join(".git").join("loams-test-data"), "device-test");
    diff_sync::capture_diff(&repos, root)
        .await
        .unwrap()
        .checksum
}

fn repos_for(tmp: &Path) -> Repos {
    Repos::with_worktrees_root(&tmp.join("data"), "device-test", tmp.join("worktrees"))
}

/// An engine with one project, `repo`, and one thread whose Mock run edited
/// `notes.txt` and `src/new.txt` in the thread's own worktree.
struct Studio {
    _tmp: tempfile::TempDir,
    core: EngineCore,
    client: RpcClient,
    repo: PathBuf,
    worktree: PathBuf,
}

const CHAT: &str = "chat-git-write";
const SPACE: &str = "space-git-write";

impl Studio {
    async fn start() -> Self {
        environment();
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo(&tmp.path().join("repo"));

        let registry = HarnessRegistry::new();
        registry.register(Arc::new(MockHarness {
            script: vec![
                AgentEvent::TextDelta {
                    text: "Editing.".into(),
                },
                AgentEvent::Done {
                    status: DoneStatus::Completed,
                    result: None,
                    error: None,
                    session_id: None,
                },
            ],
        }));
        let core = EngineCore::assemble(
            &tmp.path().join("data"),
            Arc::new(registry),
            HarnessId::Mock,
        )
        .unwrap();
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
        client
            .call(
                methods::MUTATE,
                serde_json::json!({"op": "createChat", "chatId": CHAT, "deviceId": core.device_id}),
            )
            .await
            .unwrap();
        core.workspace.rename_chat(CHAT, "Pre-titled").unwrap();
        let repo_path = repo.to_string_lossy().into_owned();
        core.doc_host
            .queue_command(
                CHAT,
                SessionCommandPayload::Run {
                    request: RunRequest {
                        mcp_servers: Vec::new(),
                        mcp: None,
                        prompt: "edit the notes".into(),
                        harness: None,
                        model: None,
                        reasoning: None,
                        model_options: Default::default(),
                        cwd: repo_path.clone(),
                        sandbox: SandboxLevel::WorkspaceWrite,
                        auto_approve: true,
                        attachments: Vec::new(),
                        resume: None,
                        worktree: Some(WorktreeSpec {
                            repo_path,
                            base: "main".into(),
                            space_id: Some(SPACE.into()),
                        }),
                    },
                    message_id: "msg-git-write-1".into(),
                },
            )
            .unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let done = core
                .doc_host
                .open(CHAT)
                .ok()
                .and_then(|handle| handle.doc().read_entries().ok())
                .unwrap_or_default()
                .iter()
                .any(|entry| {
                    entry.role == MessageRole::Assistant
                        && entry.status == Some(MessageStatus::Complete)
                });
            if done {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline, "the Mock run");
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
        let worktree = PathBuf::from(core.workspace.chat(CHAT).unwrap().unwrap().cwd.unwrap())
            .canonicalize()
            .unwrap();
        assert_ne!(worktree, repo, "the thread runs in its own worktree");
        assert_eq!(
            std::fs::read_to_string(worktree.join("notes.txt")).unwrap(),
            "first\nmock edit 1\n"
        );
        core.diff_sync.reconcile_now().await;
        Self {
            _tmp: tmp,
            core,
            client,
            repo,
            worktree,
        }
    }

    /// `GetCheckoutDiff` of the thread's worktree: `(checkoutId, checksum)`.
    async fn diff(&self) -> (String, String) {
        let diff = self
            .client
            .call(
                methods::GET_CHECKOUT_DIFF,
                serde_json::json!({"cwd": self.worktree.to_string_lossy(), "mode": "working"}),
            )
            .await
            .unwrap();
        (
            diff["checkoutId"].as_str().unwrap().to_owned(),
            diff["checksum"].as_str().unwrap().to_owned(),
        )
    }

    async fn stop(self) {
        self.core.shutdown().await;
    }
}

// ---------------------------------------------------------------------------
// G1: ListWorktrees
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn list_worktrees_joins_chats() {
    let studio = Studio::start().await;
    let reply = studio
        .client
        .call(
            methods::LIST_WORKTREES,
            serde_json::json!({"repoPath": studio.repo.to_string_lossy()}),
        )
        .await
        .unwrap();
    let worktrees: Vec<WorktreeInfo> = serde_json::from_value(reply).unwrap();
    assert_eq!(worktrees.len(), 2, "{worktrees:?}");

    let main = &worktrees[0];
    assert_eq!(Path::new(&main.path), studio.repo);
    assert_eq!(main.branch, "main");
    assert!(main.chat_ids.is_empty());
    assert!(!main.dirty);
    assert_eq!(main.head, git(&studio.repo, &["rev-parse", "HEAD"]));

    let thread = &worktrees[1];
    assert_eq!(Path::new(&thread.path), studio.worktree);
    assert!(
        thread.branch.starts_with("loams-desktop/"),
        "{}",
        thread.branch
    );
    assert_eq!(thread.chat_ids, vec![CHAT.to_owned()]);
    assert!(thread.dirty, "the Mock run's edits are uncommitted");
    assert_eq!((thread.ahead, thread.behind), (0, 0), "no upstream");
    let identity = studio
        .core
        .repos
        .checkout_identity(&studio.worktree)
        .await
        .unwrap();
    assert_eq!(thread.checkout_id.as_deref(), Some(identity.id.as_str()));
    assert!(thread.last_activity_ms > 0);

    // An unknown path is no repository of this device's.
    let outside = tempfile::tempdir().unwrap();
    init_repo(outside.path());
    let error = studio
        .client
        .call(
            methods::LIST_WORKTREES,
            serde_json::json!({"repoPath": outside.path().to_string_lossy()}),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("not a known"), "{error}");
    studio.stop().await;
}

// ---------------------------------------------------------------------------
// G2: CommitWorktree
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn commit_stages_and_signs_off() {
    let studio = Studio::start().await;
    let (checkout_id, sum) = studio.diff().await;
    let reply = studio
        .client
        .call(
            methods::COMMIT_WORKTREE,
            serde_json::to_value(CommitWorktreeParams {
                checkout_id: checkout_id.clone(),
                cwd: studio.worktree.to_string_lossy().into_owned(),
                message: "Edit the notes".into(),
                paths: vec!["notes.txt".into()],
                all: false,
                amend: false,
                signoff: true,
                expected_checksum: sum,
            })
            .unwrap(),
        )
        .await
        .unwrap();
    let head = git(&studio.worktree, &["rev-parse", "HEAD"]);
    assert_eq!(reply["sha"], head.as_str());
    assert_eq!(reply["summary"], "Edit the notes");
    let message = git(&studio.worktree, &["log", "-1", "--format=%B"]);
    assert!(
        message.contains(&format!("Signed-off-by: {NAME} <{EMAIL}>")),
        "{message}"
    );
    // Only the named path was staged; the other edit is still untracked.
    assert_eq!(
        git(
            &studio.worktree,
            &["show", "--name-only", "--format=", "HEAD"]
        ),
        "notes.txt"
    );
    assert_eq!(
        git(
            &studio.worktree,
            &["status", "--porcelain", "--untracked-files=all"]
        ),
        "?? src/new.txt"
    );

    // `all` stages the rest, untracked files included.
    let (_, sum) = studio.diff().await;
    studio
        .client
        .call(
            methods::COMMIT_WORKTREE,
            serde_json::to_value(CommitWorktreeParams {
                checkout_id,
                cwd: studio.worktree.to_string_lossy().into_owned(),
                message: "Add the new file".into(),
                paths: Vec::new(),
                all: true,
                amend: false,
                signoff: false,
                expected_checksum: sum,
            })
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        git(
            &studio.worktree,
            &["status", "--porcelain", "--untracked-files=all"]
        ),
        ""
    );
    assert!(!git(&studio.worktree, &["log", "-1", "--format=%B"]).contains("Signed-off-by"));
    studio.stop().await;
}

#[tokio::test]
async fn commit_refuses_stale_checksum() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo(&tmp.path().join("repo"));
    std::fs::write(repo.join("notes.txt"), "first\nreviewed\n").unwrap();
    let reviewed = checksum(&repo).await;
    // The file changes after the person reviewed the diff.
    std::fs::write(repo.join("notes.txt"), "first\nreviewed\nand more\n").unwrap();
    let head = git(&repo, &["rev-parse", "HEAD"]);
    let error = git_write::commit(
        &repos_for(tmp.path()),
        &repo,
        &CommitWorktreeParams {
            checkout_id: "ignored".into(),
            cwd: repo.to_string_lossy().into_owned(),
            message: "Commit".into(),
            paths: Vec::new(),
            all: true,
            amend: false,
            signoff: false,
            expected_checksum: reviewed,
        },
    )
    .await
    .unwrap_err();
    assert!(text(error).starts_with("stale_diff"));
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), head, "no commit");
    assert_eq!(
        git(&repo, &["diff", "--cached", "--name-only"]),
        "",
        "nothing staged"
    );
}

#[tokio::test]
async fn commit_nothing_to_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo(&tmp.path().join("repo"));
    let sum = checksum(&repo).await;
    let error = git_write::commit(
        &repos_for(tmp.path()),
        &repo,
        &CommitWorktreeParams {
            checkout_id: "ignored".into(),
            cwd: repo.to_string_lossy().into_owned(),
            message: "Commit".into(),
            paths: Vec::new(),
            all: true,
            amend: false,
            signoff: false,
            expected_checksum: sum,
        },
    )
    .await
    .unwrap_err();
    assert!(text(error).starts_with("nothing_to_commit"));
}

#[tokio::test]
async fn commit_uses_user_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo(&tmp.path().join("repo"));
    std::fs::write(repo.join("notes.txt"), "first\nsecond\n").unwrap();
    let sum = checksum(&repo).await;
    git_write::commit(
        &repos_for(tmp.path()),
        &repo,
        &CommitWorktreeParams {
            checkout_id: "ignored".into(),
            cwd: repo.to_string_lossy().into_owned(),
            message: "Second line".into(),
            paths: Vec::new(),
            all: true,
            amend: false,
            signoff: true,
            expected_checksum: sum,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        git(&repo, &["log", "-1", "--format=%an <%ae>|%cn <%ce>"]),
        format!("{NAME} <{EMAIL}>|{NAME} <{EMAIL}>")
    );
    // The daemon never sets an identity of its own.
    let source = include_str!("../src/git_write.rs");
    for forbidden in ["GIT_AUTHOR_", "GIT_COMMITTER_", "user.name", "user.email"] {
        assert!(
            !source.contains(forbidden),
            "git_write.rs mentions {forbidden}"
        );
    }
}

// ---------------------------------------------------------------------------
// G3: PushBranch
// ---------------------------------------------------------------------------

/// A bare `origin`, a clone `work` that pushed `main`, and a second clone
/// that then moved `origin/main` on.
fn pushed_clones(tmp: &Path) -> (PathBuf, PathBuf, String) {
    let seed = init_repo(&tmp.join("seed"));
    let origin = tmp.join("origin.git");
    git(
        tmp,
        &[
            "clone",
            "-q",
            "--bare",
            &seed.to_string_lossy(),
            &origin.to_string_lossy(),
        ],
    );
    let work = tmp.join("work");
    git(
        tmp,
        &[
            "clone",
            "-q",
            &origin.to_string_lossy(),
            &work.to_string_lossy(),
        ],
    );
    configure(&work);
    let other = tmp.join("other");
    git(
        tmp,
        &[
            "clone",
            "-q",
            &origin.to_string_lossy(),
            &other.to_string_lossy(),
        ],
    );
    configure(&other);
    let moved = commit_file(&other, "other.txt", "other\n", "someone else's work");
    git(&other, &["push", "-q", "origin", "main"]);
    // `work` diverges without fetching.
    commit_file(&work, "mine.txt", "mine\n", "my work");
    (work.canonicalize().unwrap(), origin, moved)
}

fn push_params(force_with_lease: bool) -> PushBranchParams {
    PushBranchParams {
        cwd: String::new(),
        remote: None,
        branch: "main".into(),
        set_upstream: false,
        force_with_lease,
    }
}

#[tokio::test]
async fn push_with_lease_rejected_when_remote_moved() {
    let tmp = tempfile::tempdir().unwrap();
    let (work, origin, moved) = pushed_clones(tmp.path());
    let error = git_write::push(&work, &push_params(true))
        .await
        .unwrap_err();
    assert!(text(error).starts_with("push_rejected"));
    assert_eq!(
        git(&origin, &["rev-parse", "main"]),
        moved,
        "origin kept its commit"
    );

    // Once this checkout has seen the remote's commit, the lease holds.
    git(&work, &["fetch", "-q", "origin"]);
    let reply = git_write::push(&work, &push_params(true)).await.unwrap();
    assert_eq!(reply.remote, "origin");
    assert_eq!(reply.ref_name, "refs/heads/main");
    assert_eq!(
        git(&origin, &["rev-parse", "main"]),
        git(&work, &["rev-parse", "HEAD"])
    );
}

#[tokio::test]
async fn push_never_plain_force() {
    // The source has no way to say it.
    let source = include_str!("../src/git_write.rs");
    for forbidden in [
        "\"--force\"",
        "\"-f\"",
        "\"+refs/",
        "\"+{",
        "--force-if-includes",
        "--mirror",
    ] {
        assert!(
            !source.contains(forbidden),
            "git_write.rs contains {forbidden}"
        );
    }
    for set_upstream in [false, true] {
        for lease in [
            None,
            Some(""),
            Some("0123456789abcdef0123456789abcdef01234567"),
        ] {
            let args = git_write::push_args("origin", "main", set_upstream, lease);
            for arg in &args {
                assert_ne!(arg, "--force");
                assert_ne!(arg, "-f");
                assert!(!arg.starts_with('+'), "{arg}");
                if arg.starts_with("--force") {
                    assert_eq!(
                        arg,
                        &format!("--force-with-lease=refs/heads/main:{}", lease.unwrap())
                    );
                }
            }
            assert_eq!(args.last().unwrap(), "refs/heads/main:refs/heads/main");
        }
    }

    // And a push without a lease never overwrites the remote.
    let tmp = tempfile::tempdir().unwrap();
    let (work, origin, moved) = pushed_clones(tmp.path());
    let error = git_write::push(&work, &push_params(false))
        .await
        .unwrap_err();
    assert!(text(error).starts_with("push_rejected"));
    assert_eq!(git(&origin, &["rev-parse", "main"]), moved);

    // A remote must be a configured remote's name, never a URL.
    let mut params = push_params(false);
    params.remote = Some(origin.to_string_lossy().into_owned());
    let error = git_write::push(&work, &params).await.unwrap_err();
    assert!(matches!(error, GitWriteError::BadParams(_)), "{error}");
}

// ---------------------------------------------------------------------------
// G4: CreateChangeRequest
// ---------------------------------------------------------------------------

/// The shared fake Forgejo, answering each `POST …/pulls` with pull 7.
async fn fake_forgejo() -> (String, Arc<Mutex<Vec<support::forgejo::Recorded>>>) {
    support::forgejo::fake_forgejo(|_, _| {
        (
            201,
            serde_json::json!({
                "number": 7,
                "html_url": "http://forgejo.invalid/dev/project/pulls/7",
                "state": "open",
            }),
        )
    })
    .await
}

fn change_request(provider: ChangeRequestProvider, repo: &Path) -> CreateChangeRequestParams {
    CreateChangeRequestParams {
        cwd: repo.to_string_lossy().into_owned(),
        provider,
        base: "main".into(),
        head: "feature".into(),
        title: "Fix the linker".into(),
        body: "It links again.".into(),
        draft: true,
        link_issue: Some("#42".into()),
    }
}

#[tokio::test]
async fn forgejo_change_request_request_shape() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo(&tmp.path().join("repo"));
    let (base, seen) = fake_forgejo().await;
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            &format!("{base}/dev/project.git"),
        ],
    );

    let creator = ChangeRequestCreator::new(Arc::new(StaticForgeCredentials::new(Some(
        ForgejoCredential {
            base_url: base.clone(),
            token: "forgejo-test-token".into(),
        },
    ))));
    let reply = creator
        .create(
            &repo,
            &change_request(ChangeRequestProvider::Forgejo, &repo),
        )
        .await
        .unwrap();
    assert_eq!(reply.provider, ChangeRequestProvider::Forgejo);
    assert_eq!(reply.number, 7);
    assert_eq!(reply.url, "http://forgejo.invalid/dev/project/pulls/7");
    assert_eq!(reply.state, ChangeRequestState::Open);

    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].path, "/api/v1/repos/dev/project/pulls");
    assert_eq!(seen[0].authorization, "token forgejo-test-token");
    assert_eq!(
        seen[0].body,
        serde_json::json!({
            "base": "main",
            "head": "feature",
            "title": "WIP: Fix the linker",
            "body": "It links again.\n\nCloses #42",
        })
    );
}

#[cfg(unix)]
#[tokio::test]
async fn github_change_request_uses_gh() {
    use std::os::unix::fs::PermissionsExt as _;

    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo(&tmp.path().join("repo"));
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/widgets.git",
        ],
    );
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = tmp.path().join("gh-args");
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

    let creator = ChangeRequestCreator::new(Arc::new(StaticForgeCredentials::new(None)))
        .with_gh_path(vec![bin]);
    let reply = creator
        .create(&repo, &change_request(ChangeRequestProvider::Github, &repo))
        .await
        .unwrap();
    assert_eq!(reply.provider, ChangeRequestProvider::Github);
    assert_eq!(reply.number, 12);
    assert_eq!(reply.url, "https://github.com/acme/widgets/pull/12");
    assert_eq!(reply.state, ChangeRequestState::Open);

    let args: Vec<String> = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        args,
        [
            "pr",
            "create",
            "--repo=github.com/acme/widgets",
            "--base=main",
            "--head=feature",
            "--title=Fix the linker",
            "--body=It links again.",
            "",
            "Closes #42",
            "--draft",
        ]
    );
}

#[tokio::test]
async fn change_request_without_credential_is_provider_not_configured() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo(&tmp.path().join("repo"));
    let (base, seen) = fake_forgejo().await;
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            &format!("{base}/dev/project.git"),
        ],
    );

    let none = ChangeRequestCreator::new(Arc::new(StaticForgeCredentials::new(None)));
    let error = none
        .create(
            &repo,
            &change_request(ChangeRequestProvider::Forgejo, &repo),
        )
        .await
        .unwrap_err();
    assert!(text(error).starts_with("provider_not_configured"));

    // A credential for another host is no credential for this one: the token
    // never leaves for a host it wasn't given for.
    let elsewhere = ChangeRequestCreator::new(Arc::new(StaticForgeCredentials::new(Some(
        ForgejoCredential {
            base_url: "https://forgejo.example.invalid".into(),
            token: "forgejo-test-token".into(),
        },
    ))));
    let error = elsewhere
        .create(
            &repo,
            &change_request(ChangeRequestProvider::Forgejo, &repo),
        )
        .await
        .unwrap_err();
    let message = text(error);
    assert!(message.starts_with("provider_not_configured"), "{message}");
    assert!(!message.contains("forgejo-test-token"));
    assert!(seen.lock().unwrap().is_empty(), "no request was sent");

    // Without `gh`, GitHub is not configured either.
    let empty_bin = tmp.path().join("empty-bin");
    std::fs::create_dir_all(&empty_bin).unwrap();
    git(
        &repo,
        &[
            "remote",
            "set-url",
            "origin",
            "https://github.com/acme/widgets.git",
        ],
    );
    let no_gh = ChangeRequestCreator::new(Arc::new(StaticForgeCredentials::new(None)))
        .with_gh_program("loams-test-no-such-gh")
        .with_gh_path(vec![empty_bin]);
    let error = no_gh
        .create(&repo, &change_request(ChangeRequestProvider::Github, &repo))
        .await
        .unwrap_err();
    assert!(text(error).starts_with("provider_not_configured"));
}

// ---------------------------------------------------------------------------
// G5: MergeBranch
// ---------------------------------------------------------------------------

fn merge_params(repo: &Path, strategy: MergeStrategy, source_head: &str) -> MergeBranchParams {
    MergeBranchParams {
        repo_path: repo.to_string_lossy().into_owned(),
        source: "feature".into(),
        target: "main".into(),
        strategy,
        message: None,
        delete_source: false,
        remove_worktree: false,
        expected_source_head: source_head.into(),
    }
}

/// `main` with one more commit than the fork point, and `feature` with one
/// non-conflicting commit of its own. Returns `(main head, feature head)`.
fn diverged(repo: &Path, main_moves: bool) -> (String, String) {
    git(repo, &["checkout", "-q", "-b", "feature"]);
    let feature = commit_file(repo, "feature.txt", "feature\n", "the feature");
    git(repo, &["checkout", "-q", "main"]);
    let main = if main_moves {
        commit_file(repo, "main.txt", "main\n", "main moves on")
    } else {
        git(repo, &["rev-parse", "HEAD"])
    };
    (main, feature)
}

fn parents(repo: &Path, rev: &str) -> usize {
    git(repo, &["rev-list", "--parents", "-n", "1", rev])
        .split_whitespace()
        .count()
        - 1
}

fn worktree_count(repo: &Path) -> usize {
    git(repo, &["worktree", "list", "--porcelain"])
        .lines()
        .filter(|line| line.starts_with("worktree "))
        .count()
}

#[tokio::test]
async fn merge_each_strategy() {
    for strategy in [
        MergeStrategy::Merge,
        MergeStrategy::Squash,
        MergeStrategy::Rebase,
        MergeStrategy::Ff,
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo(&tmp.path().join("repo"));
        let repos = repos_for(tmp.path());
        let (main, feature) = diverged(&repo, strategy != MergeStrategy::Ff);
        let reply = git_write::merge(&repos, &repo, &merge_params(&repo, strategy, &feature))
            .await
            .unwrap_or_else(|error| panic!("{strategy:?}: {error}"));
        let head = git(&repo, &["rev-parse", "main"]);
        assert_eq!(reply.sha, head, "{strategy:?}");
        assert!(repo.join("feature.txt").is_file(), "{strategy:?}");
        match strategy {
            MergeStrategy::Merge => {
                assert_eq!(parents(&repo, "main"), 2);
                assert_eq!(git(&repo, &["rev-parse", "main^2"]), feature);
            }
            MergeStrategy::Squash => {
                assert_eq!(parents(&repo, "main"), 1);
                assert_eq!(git(&repo, &["rev-parse", "main^"]), main);
            }
            MergeStrategy::Rebase => {
                assert_eq!(parents(&repo, "main"), 1);
                assert_eq!(git(&repo, &["rev-parse", "main^"]), main);
                assert_eq!(
                    git(&repo, &["log", "-1", "--format=%s", "main"]),
                    "the feature"
                );
            }
            MergeStrategy::Ff => assert_eq!(head, feature),
        }
        assert_eq!(git(&repo, &["status", "--porcelain"]), "", "{strategy:?}");
        assert_eq!(
            worktree_count(&repo),
            1,
            "{strategy:?}: no temporary worktree left"
        );
    }

    // A target that isn't checked out is merged in a temporary worktree; the
    // source's worktree and branch are removed on request.
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo(&tmp.path().join("repo"));
    let repos = repos_for(tmp.path());
    git(&repo, &["branch", "release"]);
    let wt = tmp.path().join("feature-wt");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            &wt.to_string_lossy(),
        ],
    );
    let feature = commit_file(&wt, "feature.txt", "feature\n", "the feature");
    let mut params = merge_params(&repo, MergeStrategy::Merge, &feature);
    params.target = "release".into();
    params.message = Some("Release the feature".into());
    params.delete_source = true;
    params.remove_worktree = true;
    let reply = git_write::merge(&repos, &repo, &params).await.unwrap();
    assert_eq!(git(&repo, &["rev-parse", "release"]), reply.sha);
    assert_eq!(
        git(&repo, &["log", "-1", "--format=%s", "release"]),
        "Release the feature"
    );
    assert!(!wt.exists(), "the source worktree is removed");
    assert_eq!(git(&repo, &["branch", "--list", "feature"]), "");
    assert_eq!(worktree_count(&repo), 1);
    assert_eq!(
        git(&repo, &["symbolic-ref", "--short", "HEAD"]),
        "main",
        "main untouched"
    );

    // A source that moved since the person chose it is refused.
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo(&tmp.path().join("repo"));
    let (_, feature) = diverged(&repo, true);
    let stale = git(&repo, &["rev-parse", "feature^"]);
    assert_ne!(stale, feature);
    let error = git_write::merge(
        &repos_for(tmp.path()),
        &repo,
        &merge_params(&repo, MergeStrategy::Merge, &stale),
    )
    .await
    .unwrap_err();
    assert!(text(error).starts_with("stale_diff"));
}

#[tokio::test]
async fn merge_conflict_leaves_clean_tree() {
    for strategy in [
        MergeStrategy::Merge,
        MergeStrategy::Squash,
        MergeStrategy::Rebase,
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo(&tmp.path().join("repo"));
        git(&repo, &["checkout", "-q", "-b", "feature"]);
        let feature = commit_file(&repo, "notes.txt", "feature's line\n", "feature edits");
        git(&repo, &["checkout", "-q", "main"]);
        let main = commit_file(&repo, "notes.txt", "main's line\n", "main edits");

        let error = git_write::merge(
            &repos_for(tmp.path()),
            &repo,
            &merge_params(&repo, strategy, &feature),
        )
        .await
        .unwrap_err();
        let message = text(error);
        let paths = message
            .strip_prefix("merge_conflict: ")
            .unwrap_or_else(|| panic!("{strategy:?}: {message}"));
        let paths: Vec<String> = serde_json::from_str(paths).unwrap();
        assert_eq!(paths, vec!["notes.txt".to_owned()], "{strategy:?}");

        assert_eq!(git(&repo, &["rev-parse", "main"]), main, "{strategy:?}");
        assert_eq!(
            git(&repo, &["rev-parse", "feature"]),
            feature,
            "{strategy:?}"
        );
        assert_eq!(
            git(&repo, &["status", "--porcelain", "--untracked-files=all"]),
            "",
            "{strategy:?}: a clean tree"
        );
        assert!(!repo.join(".git/MERGE_HEAD").exists(), "{strategy:?}");
        assert_eq!(
            std::fs::read_to_string(repo.join("notes.txt")).unwrap(),
            "main's line\n"
        );
        assert_eq!(worktree_count(&repo), 1, "{strategy:?}");
    }
}

// ---------------------------------------------------------------------------
// G21: DiscardPaths
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn discard_paths_restores_only_those() {
    let studio = Studio::start().await;
    let (checkout_id, sum) = studio.diff().await;

    // A stale checksum discards nothing.
    let error = studio
        .client
        .call(
            methods::DISCARD_PATHS,
            serde_json::json!({
                "checkoutId": checkout_id, "paths": ["notes.txt"], "expectedChecksum": "stale"
            }),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("stale_diff"), "{error}");
    assert!(
        std::fs::read_to_string(studio.worktree.join("notes.txt"))
            .unwrap()
            .contains("mock edit")
    );

    // A path outside the checkout is refused.
    let error = studio
        .client
        .call(
            methods::DISCARD_PATHS,
            serde_json::json!({
                "checkoutId": checkout_id, "paths": ["../repo/notes.txt"], "expectedChecksum": sum
            }),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("relative"), "{error}");

    studio
        .client
        .call(
            methods::DISCARD_PATHS,
            serde_json::json!({
                "checkoutId": checkout_id, "paths": ["notes.txt"], "expectedChecksum": sum
            }),
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(studio.worktree.join("notes.txt")).unwrap(),
        "first\n"
    );
    assert_eq!(
        std::fs::read_to_string(studio.worktree.join("src/new.txt")).unwrap(),
        "mock edit 1\n",
        "the other edit stays"
    );

    // An untracked file is discarded too, and only it.
    let (_, sum) = studio.diff().await;
    studio
        .client
        .call(
            methods::DISCARD_PATHS,
            serde_json::json!({
                "checkoutId": checkout_id, "paths": ["src/new.txt"], "expectedChecksum": sum
            }),
        )
        .await
        .unwrap();
    assert!(!studio.worktree.join("src").exists());
    assert_eq!(
        git(
            &studio.worktree,
            &["status", "--porcelain", "--untracked-files=all"]
        ),
        ""
    );
    studio.stop().await;
}

// ---------------------------------------------------------------------------
// Roles
// ---------------------------------------------------------------------------

#[test]
fn git_writes_not_in_mcp_role() {
    for name in [
        methods::LIST_WORKTREES,
        methods::COMMIT_WORKTREE,
        methods::PUSH_BRANCH,
        methods::CREATE_CHANGE_REQUEST,
        methods::MERGE_BRANCH,
        methods::DISCARD_PATHS,
    ] {
        let spec = method_spec(name).unwrap_or_else(|| panic!("{name} is in the table"));
        assert_eq!(spec.role, Role::Owner, "{name} must never be open to mcp");
        assert!(spec.renderer, "{name} is a Studio method");
    }
}
