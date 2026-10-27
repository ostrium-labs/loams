//! The loopback Connect listener in front of a real, in-process engine with
//! the Mock harness (plan SD1 Task 5): the §50 §7 rules, and unary and
//! streaming calls that answer what the ndjson wire answers.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use connectrpc::ErrorCode;
use connectrpc::client::{ClientConfig, HttpClient};
use loams_agentd_connect::map::Wire;
use loams_agentd_connect::proto::loams::agentd::v1 as pb;
use loams_agentd_connect::{Agentd, ConnectListener, HostInfo, LoopbackOptions};
use loams_agentd_proto as rs;
use loams_agentd_proto::daemon::DaemonMode;
use loams_agentd_rpc::{RpcClient, RpcService, Secret, memory_client, methods};
use loams_agentd_sessions::{Engine, EngineConfig, EngineProfile, EngineRuntime, HarnessId};

const TOKEN: &str = "loopback-test-token";

struct Daemon {
    _dir: tempfile::TempDir,
    runtime: EngineRuntime,
    listener: ConnectListener,
    ndjson: RpcClient,
}

impl Daemon {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config = EngineConfig {
            data_dir: dir.path().to_path_buf(),
            default_harness: HarnessId::Mock,
            terminal_shell: None,
            tool_outputs: Default::default(),
            mcp_endpoint: None,
            approvals: Default::default(),
        };
        let runtime = Engine::assemble_runtime(&config, EngineProfile::local(dir.path()).unwrap())
            .await
            .unwrap();
        let engine: Arc<dyn RpcService> = runtime.core().rpc_service();
        let agentd = Agentd::new(
            engine.clone(),
            HostInfo {
                protocol: 1,
                version: "0.0.0-test".into(),
                build_sha: "test".into(),
                mode: DaemonMode::Child,
                pid: std::process::id(),
                started_at_ms: 1,
                features: vec!["studio.v1".into()],
            },
        );
        let listener = ConnectListener::bind(
            agentd,
            LoopbackOptions {
                token: Secret::new(TOKEN),
                auth_failure_delay: Duration::from_millis(10),
            },
        )
        .await
        .unwrap();
        Self {
            _dir: dir,
            ndjson: memory_client(engine),
            runtime,
            listener,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.listener.port())
    }

    fn config(&self) -> ClientConfig {
        ClientConfig::new(self.url("").parse().unwrap())
            .with_default_header("authorization", format!("Bearer {TOKEN}"))
    }

    async fn stop(self) {
        self.listener.stop().await;
        self.runtime.shutdown().await;
    }
}

/// The next stream message, owned, within 5 s.
macro_rules! next {
    ($stream:expr) => {
        tokio::time::timeout(Duration::from_secs(5), $stream.message())
            .await
            .expect("a stream message within 5 s")
            .unwrap()
            .expect("the stream is open")
            .to_owned_message()
    };
}

const LIST_HARNESSES: &str = "/loams.agentd.v1.HarnessService/ListHarnesses";

/// A raw Connect unary POST with JSON, so headers are fully under test.
async fn post(url: &str, headers: &[(&str, &str)]) -> (u16, serde_json::Value) {
    // Feature unification can build reqwest with `rustls-no-provider` (as
    // loams-walrus asks for), and then a client panics without a process
    // provider; install ring's, as the remote listener uses, if none is set.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut request = reqwest::Client::new()
        .post(url)
        .header("content-type", "application/json")
        .body("{}");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    let body = response.json().await.unwrap_or(serde_json::Value::Null);
    (status, body)
}

#[tokio::test]
async fn connect_without_token_is_401() {
    let daemon = Daemon::start().await;
    let url = daemon.url(LIST_HARNESSES);
    let (status, body) = post(&url, &[]).await;
    assert_eq!(status, 401, "{body}");
    assert_eq!(body["code"], "unauthenticated");
    let (status, _) = post(&url, &[("authorization", "Bearer wrong-token")]).await;
    assert_eq!(status, 401);
    let (status, _) = post(&url, &[("authorization", TOKEN)]).await;
    assert_eq!(status, 401, "a token without the Bearer scheme");
    let bearer = format!("Bearer {TOKEN}");
    let (status, body) = post(&url, &[("authorization", &bearer)]).await;
    assert_eq!(status, 200, "{body}");
    daemon.stop().await;
}

#[tokio::test]
async fn connect_origin_is_403() {
    let daemon = Daemon::start().await;
    let bearer = format!("Bearer {TOKEN}");
    for origin in ["https://evil.example", "null", "loams-app://console"] {
        let (status, body) = post(
            &daemon.url(LIST_HARNESSES),
            &[("authorization", &bearer), ("origin", origin)],
        )
        .await;
        assert_eq!(status, 403, "{origin}: {body}");
        assert_eq!(body["code"], "permission_denied");
    }
    daemon.stop().await;
}

#[tokio::test]
async fn connect_foreign_host_is_403() {
    let daemon = Daemon::start().await;
    let bearer = format!("Bearer {TOKEN}");
    let port = daemon.listener.port();
    for host in [
        format!("localhost:{port}"),
        "127.0.0.1".to_owned(),
        format!("127.0.0.1:{}", port.wrapping_add(1)),
        "evil.example".to_owned(),
    ] {
        let (status, _) = post(
            &daemon.url(LIST_HARNESSES),
            &[("authorization", &bearer), ("host", &host)],
        )
        .await;
        assert_eq!(status, 403, "Host {host}");
    }
    daemon.stop().await;
}

#[tokio::test]
async fn connect_unary_and_stream_work() {
    let daemon = Daemon::start().await;

    // Unary: ListHarnesses answers what the ndjson wire answers.
    let harnesses = pb::HarnessServiceClient::new(HttpClient::plaintext(), daemon.config())
        .list_harnesses(pb::ListHarnessesRequest::default())
        .await
        .unwrap()
        .into_owned();
    let ndjson: rs::HarnessCatalog = daemon
        .ndjson
        .call_as(methods::LIST_HARNESSES, serde_json::json!({}))
        .await
        .unwrap();
    assert!(!harnesses.harnesses.is_empty());
    assert_eq!(harnesses.harnesses, ndjson.0.to_pb());

    // Stream: WatchThreads sends the list, then the list again on a change.
    let threads = pb::ThreadServiceClient::new(HttpClient::plaintext(), daemon.config());
    let mut stream = threads
        .watch_threads(pb::WatchThreadsRequest::default())
        .await
        .unwrap();
    let first = next!(stream);
    assert!(first.chats.is_empty(), "{first:?}");
    let host = pb::StudioServiceClient::new(HttpClient::plaintext(), daemon.config())
        .get_host(pb::GetHostRequest::default())
        .await
        .unwrap()
        .into_owned();
    let device_id = host.host.as_option().unwrap().device_id.clone();
    let created = threads
        .create_thread(pb::CreateThreadRequest {
            chat_id: "chat-connect-1".into(),
            device_id: Some(device_id),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_owned();
    assert!(created.ok);
    let mut seen = next!(stream);
    for _ in 0..10 {
        if !seen.chats.is_empty() {
            break;
        }
        seen = next!(stream);
    }
    assert_eq!(seen.chats.len(), 1);
    assert_eq!(seen.chats[0].id, "chat-connect-1");

    // The same frame over ndjson maps to the same message.
    let mut ndjson = daemon
        .ndjson
        .subscribe(methods::WATCH_CHATS, serde_json::json!({}))
        .await
        .unwrap();
    let item = tokio::time::timeout(Duration::from_secs(5), ndjson.recv())
        .await
        .unwrap()
        .unwrap();
    let chats: rs::ChatList = serde_json::from_value(item).unwrap();
    assert_eq!(chats.0.to_pb(), seen.chats);
    daemon.stop().await;
}

/// Studio's "New thread in <project>" (Sidebar, Mod+N) sends only the chat id,
/// the project's path and a config: no spaceId, no deviceId. The daemon hosts
/// it in the project at that path (a 500 "createChat needs a spaceId or a
/// deviceId" before).
#[tokio::test]
async fn create_thread_with_only_a_cwd_lands_in_the_project() {
    let daemon = Daemon::start().await;
    let project = tempfile::tempdir().unwrap();
    let path = project.path().to_string_lossy().into_owned();
    let threads = pb::ThreadServiceClient::new(HttpClient::plaintext(), daemon.config());
    let created = threads
        .create_thread(pb::CreateThreadRequest {
            chat_id: "chat-cwd-only".into(),
            cwd: Some(path.clone()),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_owned();
    assert!(created.ok);

    let host = pb::StudioServiceClient::new(HttpClient::plaintext(), daemon.config())
        .get_host(pb::GetHostRequest::default())
        .await
        .unwrap()
        .into_owned();
    let device_id = host.host.as_option().unwrap().device_id.clone();
    let mut stream = threads
        .watch_threads(pb::WatchThreadsRequest::default())
        .await
        .unwrap();
    let mut seen = next!(stream);
    for _ in 0..10 {
        if !seen.chats.is_empty() {
            break;
        }
        seen = next!(stream);
    }
    let chat = seen
        .chats
        .iter()
        .find(|chat| chat.id == "chat-cwd-only")
        .expect("the new thread");
    assert_eq!(chat.device_id, device_id);
    assert_eq!(chat.cwd.as_deref(), Some(path.as_str()));
    assert!(
        chat.space_id.is_some(),
        "the thread belongs to a project: {chat:?}"
    );
    daemon.stop().await;
}

#[tokio::test]
async fn bad_requests_answer_connect_errors() {
    // Every §53 §6.5 gap is served since SD1 Task 13, so no RPC answers
    // `unimplemented` any more; a request the daemon cannot map is refused.
    let daemon = Daemon::start().await;
    let harness = pb::HarnessServiceClient::new(HttpClient::plaintext(), daemon.config());
    let error = harness
        .list_models(pb::ListModelsRequest::default())
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidArgument, "an unset harness");
    assert!(!error.details.is_empty(), "the ErrorInfo detail");

    let host = pb::StudioServiceClient::new(HttpClient::plaintext(), daemon.config())
        .get_host(pb::GetHostRequest::default())
        .await
        .unwrap()
        .into_owned();
    let host = host.host.as_option().unwrap();
    assert!(host.features.iter().any(|feature| feature == "studio.v1"));
    assert!(!host.device_id.is_empty());
    daemon.stop().await;
}

fn git(cwd: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// SD1 Task 9: the git writes are served over Connect and answer what the
/// ndjson wire answers, their refusals with the Shared contracts' codes.
#[tokio::test]
async fn git_writes_over_connect() {
    let daemon = Daemon::start().await;
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.name", "Loams Test"]);
    git(&repo, &["config", "user.email", "test@loams.invalid"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);
    std::fs::write(repo.join("notes.txt"), "first\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "initial"]);
    git(&repo, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(repo.join("notes.txt"), "feature\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "feature"]);
    let feature = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["checkout", "-q", "main"]);
    std::fs::write(repo.join("notes.txt"), "main\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "main"]);
    let repo_path = repo.canonicalize().unwrap().to_string_lossy().into_owned();

    let git_service = pb::GitServiceClient::new(HttpClient::plaintext(), daemon.config());
    // A repository the daemon doesn't know is refused.
    let error = git_service
        .list_worktrees(pb::ListWorktreesRequest {
            repo_path: repo_path.clone(),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidArgument);

    daemon
        .ndjson
        .call(methods::ADD_REPO, serde_json::json!({ "path": repo_path }))
        .await
        .unwrap();
    let listed = git_service
        .list_worktrees(pb::ListWorktreesRequest {
            repo_path: repo_path.clone(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_owned();
    let ndjson: rs::WorktreeList = daemon
        .ndjson
        .call_as(
            methods::LIST_WORKTREES,
            serde_json::json!({ "repoPath": repo_path }),
        )
        .await
        .unwrap();
    assert_eq!(listed.worktrees, ndjson.0.to_pb());
    assert_eq!(listed.worktrees.len(), 1);
    assert_eq!(listed.worktrees[0].branch, "main");

    // A stale checksum is failed_precondition (`stale_diff`).
    std::fs::write(repo.join("notes.txt"), "edited\n").unwrap();
    let diff = pb::DiffServiceClient::new(HttpClient::plaintext(), daemon.config())
        .get_checkout_diff(pb::GetCheckoutDiffRequest {
            cwd: repo_path.clone(),
            mode: "working".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_owned();
    let diff = diff.diff.as_option().unwrap().clone();
    let error = git_service
        .commit_worktree(pb::CommitWorktreeRequest {
            checkout_id: diff.checkout_id.clone(),
            cwd: repo_path.clone(),
            message: "edit".into(),
            all: true,
            expected_checksum: "stale".into(),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::FailedPrecondition);
    assert!(
        error
            .message
            .as_deref()
            .unwrap_or_default()
            .starts_with("stale_diff")
    );
    let committed = git_service
        .commit_worktree(pb::CommitWorktreeRequest {
            checkout_id: diff.checkout_id,
            cwd: repo_path.clone(),
            message: "edit".into(),
            all: true,
            expected_checksum: diff.checksum,
            ..Default::default()
        })
        .await
        .unwrap()
        .into_owned();
    assert_eq!(committed.sha, git(&repo, &["rev-parse", "HEAD"]));
    assert_eq!(committed.summary, "edit");

    // A conflict is `aborted` (`merge_conflict`), with a clean tree.
    let head = git(&repo, &["rev-parse", "HEAD"]);
    let error = git_service
        .merge_branch(pb::MergeBranchRequest {
            repo_path: repo_path.clone(),
            source: "feature".into(),
            target: "main".into(),
            strategy: pb::MergeStrategy::MERGE_STRATEGY_MERGE.into(),
            expected_source_head: feature,
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Aborted);
    assert_eq!(
        error.message.as_deref(),
        Some("merge_conflict: [\"notes.txt\"]")
    );
    assert_eq!(error.details.len(), 1, "the ErrorInfo with its paths");
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&repo, &["status", "--porcelain"]), "");

    // An unset strategy is bad_params.
    let error = git_service
        .merge_branch(pb::MergeBranchRequest {
            repo_path: repo_path.clone(),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    daemon.stop().await;
}

/// SD1 Task 6: the renderer reaches `CloneRepo` directly through the proxy,
/// so the daemon refuses what git would read as an option, a local path or a
/// command transport, before git runs.
#[tokio::test]
async fn clone_repo_refuses_options_and_local_urls() {
    let daemon = Daemon::start().await;
    let projects = pb::ProjectServiceClient::new(HttpClient::plaintext(), daemon.config());
    for url in [
        "--upload-pack=touch pwned",
        "-oProxyCommand=touch pwned",
        "file:///etc",
        "/etc",
        "ext::sh -c touch% pwned",
        "fd::17",
        "https://example.com/a b.git",
        "http://example.com/a.git",
        "C:/repo",
        // SD1 Task 30: an ssh host or user read as an option, and a URL
        // whose folder would be `repos/..`, the data directory.
        "ssh://-oProxyCommand=touch%20pwned/x",
        "ssh://git@-oProxyCommand=touch/x",
        "https://example.com/..",
    ] {
        let error = projects
            .clone_repo(pb::CloneRepoRequest {
                url: url.into(),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidArgument, "{url}");
        assert!(
            error
                .message
                .as_deref()
                .unwrap_or_default()
                .starts_with("invalid_url"),
            "{url}: {error:?}"
        );
    }
    daemon.stop().await;
}

async fn git_async(cwd: &std::path::Path, args: &[&str]) -> String {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@test")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@test")
        .output()
        .await
        .unwrap();
    assert!(output.status.success(), "git {args:?}");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// SD1 Task 10: `ListCheckpoints` and `RestoreCheckpoint` over Connect
/// answer what the ndjson wire answers, and a stale checksum or an unknown
/// turn is the reason's Connect code.
#[tokio::test]
async fn checkpoints_over_connect_match_ndjson() {
    let daemon = Daemon::start().await;
    let repo = daemon._dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git_async(&repo, &["init", "-b", "main"]).await;
    std::fs::write(repo.join("notes.txt"), "first\n").unwrap();
    git_async(&repo, &["add", "."]).await;
    git_async(&repo, &["commit", "-m", "initial"]).await;
    let core = daemon.runtime.core();
    daemon
        .ndjson
        .call(
            methods::MUTATE,
            serde_json::json!({
                "op": "createChat",
                "chatId": "chat-cp",
                "deviceId": core.device_id,
                "cwd": repo.to_string_lossy(),
            }),
        )
        .await
        .unwrap();
    core.checkpoints
        .capture_turn_start("chat-cp", &repo, "msg-1")
        .await
        .unwrap();
    std::fs::write(repo.join("notes.txt"), "changed\n").unwrap();

    let threads = pb::ThreadServiceClient::new(HttpClient::plaintext(), daemon.config());
    let listed = threads
        .list_checkpoints(pb::ListCheckpointsRequest {
            chat_id: "chat-cp".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_owned();
    let ndjson: rs::checkpoints::CheckpointList = daemon
        .ndjson
        .call_as(
            methods::LIST_CHECKPOINTS,
            serde_json::json!({ "chatId": "chat-cp" }),
        )
        .await
        .unwrap();
    assert_eq!(listed.checkpoints.len(), 1);
    assert_eq!(listed.checkpoints[0].turn_id, "msg-1");
    assert_eq!(listed.checkpoints, ndjson.to_pb().checkpoints);

    let restore = |checksum: &str, turn_id: &str| pb::RestoreCheckpointRequest {
        chat_id: "chat-cp".into(),
        turn_id: turn_id.into(),
        expected_checksum: checksum.into(),
        scope: pb::CheckpointScope::CHECKPOINT_SCOPE_WORKTREE.into(),
        ..Default::default()
    };
    let error = threads
        .restore_checkpoint(restore("stale", "msg-1"))
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::FailedPrecondition, "stale_diff");
    assert!(!error.details.is_empty(), "the ErrorInfo detail");
    assert_eq!(
        std::fs::read_to_string(repo.join("notes.txt")).unwrap(),
        "changed\n"
    );

    let diff: rs::CheckoutDiff = daemon
        .ndjson
        .call_as(
            methods::GET_CHECKOUT_DIFF,
            serde_json::json!({ "cwd": repo.to_string_lossy() }),
        )
        .await
        .unwrap();
    let error = threads
        .restore_checkpoint(restore(&diff.checksum, "msg-unknown"))
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::NotFound, "checkpoint_missing");

    let restored = threads
        .restore_checkpoint(restore(&diff.checksum, "msg-1"))
        .await
        .unwrap()
        .into_owned();
    let pre = git_async(
        &repo,
        &["rev-parse", "refs/loams/checkpoints/chat-cp/turn/1/pre"],
    )
    .await;
    assert_eq!(restored.restored_to, pre);
    assert_eq!(restored.forked_chat_id, None);
    assert_eq!(
        std::fs::read_to_string(repo.join("notes.txt")).unwrap(),
        "first\n"
    );
    daemon.stop().await;
}

/// SD1 Task 11: review comments and content search are served over Connect
/// and answer what the ndjson wire answers.
#[tokio::test]
async fn review_and_search_over_connect() {
    let daemon = Daemon::start().await;
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.name", "Loams Test"]);
    git(&repo, &["config", "user.email", "test@loams.invalid"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);
    std::fs::write(repo.join("notes.txt"), "first\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "initial"]);
    std::fs::write(repo.join("notes.txt"), "first\nsecond needle\n").unwrap();
    let repo_path = repo.canonicalize().unwrap().to_string_lossy().into_owned();
    daemon
        .ndjson
        .call(methods::ADD_REPO, serde_json::json!({ "path": repo_path }))
        .await
        .unwrap();
    let diff = pb::DiffServiceClient::new(HttpClient::plaintext(), daemon.config())
        .get_checkout_diff(pb::GetCheckoutDiffRequest {
            cwd: repo_path.clone(),
            mode: "working".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_owned();
    let diff = diff.diff.as_option().unwrap().clone();

    let review = pb::ReviewServiceClient::new(HttpClient::plaintext(), daemon.config());
    let comment = |side: pb::DiffSide| pb::UpsertReviewCommentRequest {
        comment: buffa::MessageField::some(pb::ReviewComment {
            checkout_id: diff.checkout_id.clone(),
            path: "notes.txt".into(),
            side: side.into(),
            line: 2,
            body: "Why?".into(),
            diff_checksum: diff.checksum.clone(),
            ..Default::default()
        }),
        ..Default::default()
    };
    // An unset side is bad_params.
    let error = review
        .upsert_review_comment(comment(pb::DiffSide::DIFF_SIDE_UNSPECIFIED))
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    let created = review
        .upsert_review_comment(comment(pb::DiffSide::DIFF_SIDE_NEW))
        .await
        .unwrap()
        .into_owned();
    assert!(!created.id.is_empty());

    let mut stream = review
        .watch_review_comments(pb::WatchReviewCommentsRequest {
            checkout_id: diff.checkout_id.clone(),
            ..Default::default()
        })
        .await
        .unwrap();
    let first = next!(stream);
    let mut ndjson = daemon
        .ndjson
        .subscribe(
            methods::WATCH_REVIEW_COMMENTS,
            serde_json::json!({ "checkoutId": diff.checkout_id }),
        )
        .await
        .unwrap();
    let item = tokio::time::timeout(Duration::from_secs(5), ndjson.recv())
        .await
        .unwrap()
        .unwrap();
    let frame: rs::ReviewCommentsFrame = serde_json::from_value(item).unwrap();
    assert_eq!(first, frame.to_pb());
    let rs::ReviewCommentsFrame::Reset { reset } = frame else {
        panic!("a reset first");
    };
    assert_eq!(reset.len(), 1);
    assert_eq!(reset[0].id, created.id);

    // A missing comment is not_found.
    let error = review
        .resolve_review_comment(pb::ResolveReviewCommentRequest {
            checkout_id: diff.checkout_id.clone(),
            id: "no-such-comment".into(),
            resolved: true,
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::NotFound);

    // Content search over a project.
    let host = pb::StudioServiceClient::new(HttpClient::plaintext(), daemon.config())
        .get_host(pb::GetHostRequest::default())
        .await
        .unwrap()
        .into_owned();
    let device_id = host.host.as_option().unwrap().device_id.clone();
    daemon
        .ndjson
        .call(
            methods::MUTATE,
            serde_json::json!({
                "op": "createSpace", "spaceId": "space-connect", "deviceId": device_id,
                "path": repo_path, "gitDetected": true,
            }),
        )
        .await
        .unwrap();
    let files = pb::FileServiceClient::new(HttpClient::plaintext(), daemon.config());
    let mut found = files
        .search_content(pb::SearchContentRequest {
            space_id: Some("space-connect".into()),
            query: "needle".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let hit = next!(found);
    assert_eq!(
        (
            hit.path.as_str(),
            hit.line,
            hit.column,
            hit.preview.as_str()
        ),
        ("notes.txt", 2, 8, "second needle")
    );
    daemon.stop().await;
}

/// SD1 Task 12: `RunGroupService` is served (no longer `unimplemented`), its
/// stream answers what `WatchRunGroups` answers, and refusals keep their
/// codes. A group's whole life is `crates/loams-agentd-sessions/tests/groups.rs`:
/// making members creates worktrees under the daemon's worktree root, which
/// only an environment variable moves, and this crate forbids `unsafe`.
#[tokio::test]
async fn run_groups_over_connect() {
    let daemon = Daemon::start().await;
    let groups = pb::RunGroupServiceClient::new(HttpClient::plaintext(), daemon.config());

    let mut stream = groups
        .watch_run_groups(pb::WatchRunGroupsRequest::default())
        .await
        .unwrap();
    let first = next!(stream);
    let ndjson: rs::groups::RunGroupList = {
        let mut items = daemon
            .ndjson
            .subscribe(methods::WATCH_RUN_GROUPS, serde_json::json!({}))
            .await
            .unwrap();
        let item = tokio::time::timeout(Duration::from_secs(5), items.recv())
            .await
            .unwrap()
            .unwrap();
        serde_json::from_value(item).unwrap()
    };
    assert!(first.groups.is_empty());
    assert_eq!(first, ndjson.to_pb());

    let error = groups
        .create_run_group(pb::CreateRunGroupRequest {
            repo_path: "/nowhere".into(),
            base: "main".into(),
            prompt: "Fix the build".into(),
            approval_mode: pb::ApprovalMode::APPROVAL_MODE_FULL_ACCESS.into(),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidArgument, "no variants");
    let error = groups
        .create_run_group(pb::CreateRunGroupRequest::default())
        .await
        .unwrap_err();
    assert_eq!(
        error.code,
        ErrorCode::InvalidArgument,
        "an unset approval mode"
    );

    let error = groups
        .get_run_group_comparison(pb::GetRunGroupComparisonRequest {
            group_id: "nope".into(),
            a: "chat-1".into(),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidArgument, "no such group");
    let error = groups
        .run_group_check(pb::RunGroupCheckRequest {
            group_id: "nope".into(),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    let error = groups
        .resolve_run_group(pb::ResolveRunGroupRequest {
            group_id: "nope".into(),
            winner_chat_id: "chat-1".into(),
            action: pb::ResolveAction::RESOLVE_ACTION_KEEP.into(),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    daemon.stop().await;
}

/// MK1 Task 4b: `MarketplaceService` over Connect, against the marketplace
/// testkit's signed fake index, answers what the ndjson wire answers; an
/// install's harness then shows in `ListHarnesses` with its listing id.
#[tokio::test]
async fn marketplace_over_connect() {
    use loams_agentd_marketplace::testkit::{HARNESS_ID, MCP_ID, World};
    let daemon = Daemon::start().await;
    let world = World::new(&daemon._dir.path().join("agentd"));
    let host = loams_agentd_marketplace::MarketplaceHost::open(world.deps()).unwrap();
    assert!(host.refresh().await.is_empty());
    daemon.runtime.core().attach_marketplace(host);
    let market = pb::MarketplaceServiceClient::new(HttpClient::plaintext(), daemon.config());

    let sources = market
        .list_sources(pb::ListSourcesRequest::default())
        .await
        .unwrap()
        .into_owned();
    assert_eq!(sources.sources.len(), 1);
    assert!(sources.sources[0].official);

    let found = market
        .search_listings(pb::SearchListingsRequest {
            kinds: vec![
                loams_agentd_connect::proto::loams::marketplace::v1::Kind::KIND_HARNESS.into(),
            ],
            ..Default::default()
        })
        .await
        .unwrap()
        .into_owned();
    let ndjson: rs::marketplace::ListingSearchResults = daemon
        .ndjson
        .call_as(
            methods::SEARCH_LISTINGS,
            serde_json::json!({ "kinds": ["harness"] }),
        )
        .await
        .unwrap();
    assert_eq!(found, ndjson.to_pb());
    assert!(found.results.iter().any(|r| r.id == HARNESS_ID));

    let detail = market
        .get_listing(pb::GetListingRequest {
            id: "ostrium/claude-code".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_owned();
    let listing = detail.listing.as_option().unwrap();
    assert_eq!(listing.id, "ostrium/claude-code");
    assert_eq!(listing.title, "Claude Code");
    let harness = listing.harness.as_option().unwrap();
    assert_eq!(harness.launch.as_option().unwrap().command, "claude");
    // The proto listing maps back to the index's JSON.
    let ndjson: rs::marketplace::ListingDetail = daemon
        .ndjson
        .call_as(
            methods::GET_LISTING,
            serde_json::json!({ "id": "ostrium/claude-code" }),
        )
        .await
        .unwrap();
    assert_eq!(
        rs::marketplace::ListingDetail::from_pb(detail).unwrap(),
        ndjson
    );

    let plan = market
        .plan_install(pb::PlanInstallRequest {
            id: MCP_ID.into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_owned();
    let plan = plan.plan.as_option().unwrap().clone();
    assert_eq!(plan.secrets[0].name, "token");

    let error = market
        .install(pb::InstallRequest {
            id: HARNESS_ID.into(),
            plan_digest: "00".repeat(32),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::FailedPrecondition, "plan_changed");

    let harness_plan = market
        .plan_install(pb::PlanInstallRequest {
            id: HARNESS_ID.into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_owned();
    let installed = market
        .install(pb::InstallRequest {
            id: HARNESS_ID.into(),
            plan_digest: harness_plan.plan.as_option().unwrap().digest.clone(),
            ..Default::default()
        })
        .await
        .unwrap()
        .into_owned();
    assert_eq!(installed.installed.as_option().unwrap().version, "1.1.0");

    let harnesses = pb::HarnessServiceClient::new(HttpClient::plaintext(), daemon.config())
        .list_harnesses(pb::ListHarnessesRequest::default())
        .await
        .unwrap()
        .into_owned();
    let entry = harnesses
        .harnesses
        .iter()
        .find(|h| h.marketplace_listing.as_deref() == Some(HARNESS_ID))
        .expect("the installed harness is listed");
    assert_eq!(
        entry.id,
        buffa::EnumValue::Known(pb::HarnessId::HARNESS_ID_MARKETPLACE)
    );

    let listed = market
        .list_installed(pb::ListInstalledRequest::default())
        .await
        .unwrap()
        .into_owned();
    assert_eq!(listed.installed.len(), 1);
    market
        .uninstall(pb::UninstallRequest {
            id: HARNESS_ID.into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let updates = market
        .check_updates(pb::CheckUpdatesRequest::default())
        .await
        .unwrap()
        .into_owned();
    assert!(updates.updates.is_empty());
    daemon.stop().await;
}
