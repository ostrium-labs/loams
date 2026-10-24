//! Plan FT1, FT0 Task 4 (design §72 §5.2): the factory loop's agentd tools,
//! against real git repositories in a temporary directory, a local fake
//! Forgejo and no real service. The role checks (an `mcp` run reaches only its
//! own session tree; `ResolveApproval` stays `owner`) are in
//! `crates/loams-agentd/src/roles.rs`.
#![allow(clippy::unwrap_used)]

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use loams_agentd_proto::HarnessId;
use loams_agentd_proto::approvals::ApprovalDecision;
use loams_agentd_proto::factory_loop::{APPROVAL_MISMATCH, APPROVAL_REQUIRED};
use loams_agentd_proto::rpc::{Role, method_spec};
use loams_agentd_rpc::{RpcClient, methods};
use loams_agentd_sessions::approvals::{merge_payload, payload_hash};
use loams_agentd_sessions::change_request_create::{
    ChangeRequestCreator, ForgejoCredential, StaticForgeCredentials,
};
use loams_agentd_sessions::{EngineCore, EngineRpc, HarnessRegistry};
use serde_json::{Value, json};

const CHAT: &str = "chat-factory";
const SPACE: &str = "space-factory";

fn environment() -> &'static Path {
    static ROOT: OnceLock<tempfile::TempDir> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = tempfile::tempdir().unwrap();
        // SAFETY: test-only, set once before any engine reads it.
        #[allow(unsafe_code)]
        unsafe {
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

fn init_repo(dir: &Path) -> PathBuf {
    environment();
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    for (key, value) in [
        ("user.name", "Loams Test"),
        ("user.email", "test@loams.invalid"),
        ("commit.gpgsign", "false"),
    ] {
        git(dir, &["config", key, value]);
    }
    std::fs::write(dir.join("notes.txt"), "first\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "initial"]);
    dir.canonicalize().unwrap()
}

struct Factory {
    tmp: tempfile::TempDir,
    core: EngineCore,
    client: RpcClient,
    repo: PathBuf,
}

impl Factory {
    async fn start(creator: Option<ChangeRequestCreator>) -> Self {
        environment();
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo(&tmp.path().join("repo"));
        let core = EngineCore::assemble(
            &tmp.path().join("data"),
            Arc::new(HarnessRegistry::new()),
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
        let mut rpc = EngineRpc::new(
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
        .with_approvals(core.approvals.clone());
        if let Some(creator) = creator {
            rpc = rpc.with_change_request_creator(creator);
        }
        let client = loams_agentd_rpc::memory_client(Arc::new(rpc));
        client
            .call(
                methods::MUTATE,
                json!({"op": "createChat", "chatId": CHAT, "deviceId": core.device_id, "spaceId": SPACE}),
            )
            .await
            .unwrap();
        Self {
            tmp,
            core,
            client,
            repo,
        }
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        self.client
            .call(method, params)
            .await
            .map_err(|e| e.to_string())
    }

    async fn worktree(&self, branch: &str) -> Value {
        self.call(
            methods::CREATE_RUN_WORKTREE,
            json!({"chatId": CHAT, "branch": branch}),
        )
        .await
        .unwrap()
    }

    /// A build thread in a worktree on `branch`, with one committed change.
    async fn build_thread(&self, chat: &str, branch: &str) -> PathBuf {
        let made = self.worktree(branch).await;
        let path = PathBuf::from(made["path"].as_str().unwrap());
        self.call(
            methods::MUTATE,
            json!({
                "op": "createChat", "chatId": chat, "deviceId": self.core.device_id,
                "spaceId": SPACE, "parentChatId": CHAT, "cwd": path.to_string_lossy(),
            }),
        )
        .await
        .unwrap();
        std::fs::write(path.join("notes.txt"), "first\nrate limited\n").unwrap();
        git(&path, &["commit", "-q", "-am", "rate limit signup"]);
        path
    }

    async fn stop(self) {
        self.core.shutdown().await;
    }
}

#[test]
fn factory_tools_are_mcp_methods() {
    for name in [
        methods::CREATE_RUN_WORKTREE,
        methods::GET_THREAD_DIFF,
        methods::POST_REVIEW_COMMENT,
        methods::OPEN_CHANGE_REQUEST,
        methods::REQUEST_APPROVAL,
        methods::GET_APPROVAL,
        methods::MERGE_CHANGE_REQUEST,
    ] {
        let spec = method_spec(name).unwrap();
        assert_eq!(spec.role, Role::Mcp, "{name}");
        assert!(!spec.renderer, "{name} is not a renderer method");
    }
    // A person settles an approval, never a run.
    assert_eq!(
        method_spec(methods::RESOLVE_APPROVAL).unwrap().role,
        Role::Owner
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn create_chat_in_new_worktree() {
    let factory = Factory::start(None).await;
    let made = factory.worktree("factory/run-1/1").await;
    assert_eq!(made["branch"], "factory/run-1/1");
    let path = PathBuf::from(made["path"].as_str().unwrap());
    assert!(path.starts_with(environment().join("worktrees").canonicalize().unwrap()));
    assert_eq!(git(&path, &["branch", "--show-current"]), "factory/run-1/1");
    assert_eq!(
        git(&path, &["rev-parse", "HEAD"]),
        git(&factory.repo, &["rev-parse", "main"])
    );
    // Idempotent by branch: a resumed run gets the same worktree.
    let again = factory.worktree("factory/run-1/1").await;
    assert_eq!(again["path"], made["path"]);
    // A worktree of a worktree's chat still hangs off the repository.
    factory
        .call(
            methods::MUTATE,
            json!({"op": "createChat", "chatId": "inner", "deviceId": factory.core.device_id,
                   "parentChatId": CHAT, "cwd": path.to_string_lossy()}),
        )
        .await
        .unwrap();
    let nested = factory
        .call(
            methods::CREATE_RUN_WORKTREE,
            json!({"chatId": "inner", "branch": "factory/run-1/2"}),
        )
        .await
        .unwrap();
    assert_eq!(nested["repoPath"], json!(factory.repo.to_string_lossy()));
    // Not a branch name: refused.
    let bad = factory
        .call(
            methods::CREATE_RUN_WORKTREE,
            json!({"chatId": CHAT, "branch": "-x"}),
        )
        .await
        .unwrap_err();
    assert!(bad.contains("not a branch name"), "{bad}");
    factory.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn get_thread_diff_matches_checksum() {
    let factory = Factory::start(None).await;
    let path = factory.build_thread("build-1", "factory/run-2/1").await;
    std::fs::write(path.join("extra.txt"), "uncommitted\n").unwrap();
    let thread = factory
        .call(
            methods::GET_THREAD_DIFF,
            json!({"chatId": "build-1", "baseRef": "main"}),
        )
        .await
        .unwrap();
    let checkout = factory
        .call(
            methods::GET_CHECKOUT_DIFF,
            json!({"cwd": path.to_string_lossy(), "mode": "branch", "baseRef": "main"}),
        )
        .await
        .unwrap();
    assert_eq!(thread["checksum"], checkout["checksum"]);
    assert!(thread["patch"].as_str().unwrap().contains("+rate limited"));
    let working = factory
        .call(methods::GET_THREAD_DIFF, json!({"chatId": "build-1"}))
        .await
        .unwrap();
    assert!(working["patch"].as_str().unwrap().contains("+uncommitted"));
    assert!(!working["patch"].as_str().unwrap().contains("+rate limited"));

    // A review comment anchored to that diff lands on the thread's checkout.
    let posted = factory
        .call(
            methods::POST_REVIEW_COMMENT,
            json!({"chatId": "build-1", "path": "notes.txt", "line": 2,
                   "body": "Say which limit.", "diffChecksum": thread["checksum"]}),
        )
        .await
        .unwrap();
    assert!(!posted["id"].as_str().unwrap().is_empty());
    // A comment off the diff's files is refused by the review store's checks.
    let error = factory
        .call(
            methods::POST_REVIEW_COMMENT,
            json!({"chatId": "build-1", "path": "../outside", "line": 1,
                   "body": "x", "diffChecksum": thread["checksum"]}),
        )
        .await
        .unwrap_err();
    assert!(!error.is_empty());
    factory.stop().await;
}

// ---------------------------------------------------------------------------
// OpenChangeRequest and MergeChangeRequest against a fake Forgejo
// ---------------------------------------------------------------------------

static OPENED: AtomicBool = AtomicBool::new(false);

/// `GET …/pulls` lists pull 7 once a `POST …/pulls` has opened it; `POST
/// …/pulls/7/merge` merges.
fn forgejo(method: &str, path: &str) -> (u16, Value) {
    let pull = json!({
        "number": 7, "html_url": "http://forgejo.invalid/dev/project/pulls/7",
        "state": "open", "head": {"ref": "factory/run-3/1"},
    });
    match (method, path) {
        ("POST", "/api/v1/repos/dev/project/pulls") => {
            OPENED.store(true, Ordering::SeqCst);
            (201, pull)
        }
        ("GET", "/api/v1/repos/dev/project/pulls") => {
            if OPENED.load(Ordering::SeqCst) {
                (200, json!([pull]))
            } else {
                (200, json!([]))
            }
        }
        ("POST", "/api/v1/repos/dev/project/pulls/7/merge") => (200, json!({})),
        _ => (404, json!({"message": "not found"})),
    }
}

async fn forgejo_factory() -> (
    Factory,
    Arc<std::sync::Mutex<Vec<support::forgejo::Recorded>>>,
) {
    let (base, seen) = support::forgejo::fake_forgejo(forgejo).await;
    let factory = Factory::start(Some(ChangeRequestCreator::new(Arc::new(
        StaticForgeCredentials::new(Some(ForgejoCredential {
            base_url: base.clone(),
            token: "forgejo-test-token".into(),
        })),
    ))))
    .await;
    // The remote's URL names the fake Forgejo (the credential's host); pushes
    // go to a local bare repository.
    let bare = factory.tmp.path().join("remote.git");
    git(
        factory.tmp.path(),
        &["init", "-q", "--bare", &bare.to_string_lossy()],
    );
    git(
        &factory.repo,
        &[
            "remote",
            "add",
            "origin",
            &format!("{base}/dev/project.git"),
        ],
    );
    git(
        &factory.repo,
        &["config", "remote.origin.pushurl", &bare.to_string_lossy()],
    );
    (factory, seen)
}

/// The one test of the fake's state: `OPENED` is process-wide.
#[tokio::test(flavor = "multi_thread")]
async fn open_change_request_is_idempotent_and_merge_requires_settled_approval() {
    let (factory, seen) = forgejo_factory().await;
    let path = factory.build_thread("build-3", "factory/run-3/1").await;
    let head_sha = git(&path, &["rev-parse", "HEAD"]);
    let open = json!({"chatId": "build-3", "provider": "forgejo", "base": "main",
                      "title": "Rate limit signup", "body": "Plan: plan.md"});

    // open_change_request_is_idempotent
    let first = factory
        .call(methods::OPEN_CHANGE_REQUEST, open.clone())
        .await
        .unwrap();
    assert_eq!(first["number"], 7);
    assert_eq!(first["existed"], false);
    assert_eq!(first["repo"], "dev/project");
    assert_eq!(first["head"], "factory/run-3/1");
    assert_eq!(first["headSha"], json!(head_sha));
    let bare = factory.tmp.path().join("remote.git");
    assert_eq!(
        git(&bare, &["rev-parse", "factory/run-3/1"]),
        head_sha,
        "pushed"
    );
    let second = factory
        .call(methods::OPEN_CHANGE_REQUEST, open)
        .await
        .unwrap();
    assert_eq!(second["number"], 7);
    assert_eq!(second["existed"], true);
    let posts = seen
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.method == "POST" && r.path.ends_with("/pulls"))
        .count();
    assert_eq!(posts, 1, "one pull request, however often the step runs");

    let merge = |approval: &str, sha: &str| {
        json!({"chatId": "build-3", "approvalId": approval, "provider": "forgejo",
               "number": 7, "headSha": sha})
    };

    // merge_requires_settled_approval: unknown, then pending, then denied.
    let error = factory
        .call(methods::MERGE_CHANGE_REQUEST, merge("fa-none", &head_sha))
        .await
        .unwrap_err();
    assert!(error.contains(APPROVAL_REQUIRED), "{error}");
    let payload = merge_payload("dev/project", 7, &head_sha);
    let ask = |chat: &str| json!({"chatId": chat, "kind": "merge", "summary": "Merge #7 into main", "payload": payload});
    let pending = factory
        .call(methods::REQUEST_APPROVAL, ask("build-3"))
        .await
        .unwrap();
    let id = pending["id"].as_str().unwrap().to_owned();
    assert_eq!(pending["state"], "pending");
    assert_eq!(pending["payloadHash"], json!(payload_hash(&payload)));
    // It is in Studio's queue, under the factory's tool name.
    let queue = factory.core.approvals.snapshot(None);
    assert!(
        queue
            .pending
            .iter()
            .any(|a| a.id == id && a.tool == "factory.merge")
    );
    let error = factory
        .call(methods::MERGE_CHANGE_REQUEST, merge(&id, &head_sha))
        .await
        .unwrap_err();
    assert!(
        error.contains(APPROVAL_REQUIRED) && error.contains("pending"),
        "{error}"
    );
    let denied = factory
        .call(methods::REQUEST_APPROVAL, ask("build-3"))
        .await
        .unwrap();
    let denied_id = denied["id"].as_str().unwrap();
    factory
        .core
        .approvals
        .resolve(Some("build-3"), denied_id, ApprovalDecision::Deny)
        .unwrap();
    let error = factory
        .call(methods::MERGE_CHANGE_REQUEST, merge(denied_id, &head_sha))
        .await
        .unwrap_err();
    assert!(
        error.contains(APPROVAL_REQUIRED) && error.contains("denied"),
        "{error}"
    );
    assert!(
        !seen
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.path.ends_with("/merge")),
        "nothing reached the code host without an approval"
    );

    // A person approves; the approval outlives the chat's turns.
    factory.core.approvals.deny_chat("build-3");
    factory
        .core
        .approvals
        .resolve(Some("build-3"), &id, ApprovalDecision::Once)
        .unwrap();
    let got = factory
        .call(
            methods::GET_APPROVAL,
            json!({"chatId": "build-3", "approvalId": id}),
        )
        .await
        .unwrap();
    assert_eq!(got["state"], "approved");

    // merge_refuses_hash_mismatch: another head, another chat.
    let error = factory
        .call(
            methods::MERGE_CHANGE_REQUEST,
            merge(&id, "0123456789abcdef0123456789abcdef01234567"),
        )
        .await
        .unwrap_err();
    assert!(error.contains(APPROVAL_MISMATCH), "{error}");
    let mut other_chat = merge(&id, &head_sha);
    other_chat["chatId"] = json!(CHAT);
    let error = factory
        .call(methods::MERGE_CHANGE_REQUEST, other_chat)
        .await
        .unwrap_err();
    assert!(error.contains(APPROVAL_MISMATCH), "{error}");

    // The right call merges, with the approved head, once.
    let merged = factory
        .call(methods::MERGE_CHANGE_REQUEST, merge(&id, &head_sha))
        .await
        .unwrap();
    assert_eq!(merged["merged"], true);
    let calls: Vec<_> = seen
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.path.ends_with("/pulls/7/merge"))
        .cloned()
        .collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].body,
        json!({"Do": "merge", "head_commit_id": head_sha})
    );
    assert_eq!(calls[0].authorization, "token forgejo-test-token");
    let error = factory
        .call(methods::MERGE_CHANGE_REQUEST, merge(&id, &head_sha))
        .await
        .unwrap_err();
    assert!(
        error.contains(APPROVAL_MISMATCH) && error.contains("already used"),
        "{error}"
    );
    factory.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn typed_approvals_survive_a_restart_and_need_a_word_kind() {
    let factory = Factory::start(None).await;
    let error = factory
        .call(
            methods::REQUEST_APPROVAL,
            json!({"chatId": CHAT, "kind": "Merge Now", "summary": "x", "payload": {}}),
        )
        .await
        .unwrap_err();
    assert!(error.contains("one lowercase word"), "{error}");
    let asked = factory
        .call(
            methods::REQUEST_APPROVAL,
            json!({"chatId": CHAT, "kind": "plan", "summary": "Approve plan.md", "payload": {"plan": "abc"}}),
        )
        .await
        .unwrap();
    let id = asked["id"].as_str().unwrap().to_owned();
    let data = factory.tmp.path().join("data");
    let tmp = factory.tmp;
    factory.core.shutdown().await;
    drop(factory.client);
    drop(factory.core);
    let core =
        EngineCore::assemble(&data, Arc::new(HarnessRegistry::new()), HarnessId::Mock).unwrap();
    assert!(
        core.approvals
            .snapshot(None)
            .pending
            .iter()
            .any(|a| a.id == id)
    );
    core.approvals
        .resolve(None, &id, ApprovalDecision::Always)
        .unwrap();
    let decided = core.approvals.typed(CHAT, &id).unwrap();
    assert_eq!(decided.state.as_str(), "approved");
    assert!(
        core.approvals.typed("someone-else", &id).is_err(),
        "another chat's approval is not found"
    );
    core.shutdown().await;
    drop(tmp);
}
