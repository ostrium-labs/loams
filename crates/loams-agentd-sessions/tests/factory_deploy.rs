//! Plan FT1, FT0 Task 8 (design §72 §8, D1434): `DeployLiveApp`, the
//! factory's deploy to a Loams Live app, run in agentd with the daemon's
//! Live credential. Real git repositories in a temporary directory and a
//! fake Live (`LiveAdminService` and `LiveService.Query` over Connect JSON)
//! on loopback; no real service.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};

use base64::Engine as _;
use loams_agentd_proto::HarnessId;
use loams_agentd_proto::approvals::ApprovalDecision;
use loams_agentd_proto::factory_loop::{APPROVAL_MISMATCH, APPROVAL_REQUIRED};
use loams_agentd_proto::rpc::{Role, method_spec};
use loams_agentd_rpc::{RpcClient, methods};
use loams_agentd_sessions::approvals::deploy_payload;
use loams_agentd_sessions::live_deploy::{LiveCredential, LiveDeployer, StaticLiveCredentials};
use loams_agentd_sessions::{EngineCore, EngineRpc, HarnessRegistry};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const CHAT: &str = "chat-deploy";
const SPACE: &str = "space-deploy";
const APP: &str = "notes";
const TOKEN: &str = "live-test-token";
const BUNDLE: &str = "export const list = query(async () => []);\n";

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

/// A repository whose one commit is a Live app: `.loams/factory.toml`, the
/// bundle and the schema. Answers its path and the commit.
fn live_repo(dir: &Path, health: bool) -> (PathBuf, String) {
    std::fs::create_dir_all(dir.join(".loams")).unwrap();
    std::fs::create_dir_all(dir.join("live")).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    for (key, value) in [
        ("user.name", "Loams Test"),
        ("user.email", "test@loams.invalid"),
        ("commit.gpgsign", "false"),
    ] {
        git(dir, &["config", key, value]);
    }
    let health = if health {
        "health = \"health:check\"\n"
    } else {
        ""
    };
    std::fs::write(
        dir.join(".loams/factory.toml"),
        format!(
            "[tools]\ndeny = []\n\n[deploy.live]\napp = \"{APP}\"\nbundle = \"live/app.js\"\nschema = \"live/schema.json\"\nsettle_ms = 0\n{health}"
        ),
    )
    .unwrap();
    std::fs::write(dir.join("live/app.js"), BUNDLE).unwrap();
    std::fs::write(
        dir.join("live/schema.json"),
        r#"{"tables":[{"name":"notes","indexes":[{"name":"by_n","fields":["n"]}]}]}"#,
    )
    .unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "a live app"]);
    let commit = git(dir, &["rev-parse", "HEAD"]);
    // A working-tree edit after the commit is never what gets deployed.
    std::fs::write(dir.join("live/app.js"), "not this\n").unwrap();
    (dir.canonicalize().unwrap(), commit)
}

/// One request the fake Live received.
#[derive(Debug, Clone)]
struct Seen {
    path: String,
    app: String,
    authorization: String,
    body: Value,
}

/// The fake Live's state: deployments newest first, and the active one.
#[derive(Debug, Default)]
struct LiveState {
    deployments: Vec<Value>,
    active: Option<String>,
    /// `health:check` fails.
    unhealthy: bool,
    /// The next `Deploy` answers 503 before recording anything.
    drop_next_deploy: bool,
    seen: Vec<Seen>,
}

type Live = Arc<Mutex<LiveState>>;

fn set_state(live: &mut LiveState, id: &str, state: &str) {
    for d in &mut live.deployments {
        if d["id"] == id {
            d["state"] = json!(state);
        }
    }
}

fn find(live: &LiveState, id: &str) -> Option<Value> {
    live.deployments.iter().find(|d| d["id"] == id).cloned()
}

fn answer(live: &mut LiveState, path: &str, body: &Value) -> (u16, Value) {
    let conflict = || {
        (
            400,
            json!({"code": "failed_precondition", "message": "live_deploy_conflict"}),
        )
    };
    match path {
        "/loams.live.v1.LiveAdminService/ListDeployments" => {
            (200, json!({ "deployments": live.deployments }))
        }
        "/loams.live.v1.LiveAdminService/GetDeployment" => {
            match find(live, body["id"].as_str().unwrap_or_default()) {
                Some(d) => (200, json!({ "deployment": d })),
                None => (
                    404,
                    json!({"code": "not_found", "message": "no such deployment"}),
                ),
            }
        }
        "/loams.live.v1.LiveAdminService/Deploy" => {
            if std::mem::take(&mut live.drop_next_deploy) {
                return (503, json!({"code": "unavailable", "message": "restarting"}));
            }
            if body.get("expectedActive").and_then(Value::as_str) != live.active.as_deref() {
                return conflict();
            }
            let bundle = base64::engine::general_purpose::STANDARD
                .decode(body["bundle"].as_str().unwrap())
                .unwrap();
            let sha: String = Sha256::digest(&bundle)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let id = format!("d{}", live.deployments.len());
            if let Some(previous) = live.active.clone() {
                set_state(live, &previous, "DEPLOYMENT_STATE_SUPERSEDED");
            }
            let deployment = json!({
                "id": id, "bundleSha256": sha, "message": body["message"],
                "parent": live.active.clone().unwrap_or_default(),
                "state": "DEPLOYMENT_STATE_ACTIVE", "createdBy": "owner",
            });
            live.deployments.insert(0, deployment.clone());
            live.active = Some(id);
            (200, json!({ "deployment": deployment, "changes": {} }))
        }
        "/loams.live.v1.LiveAdminService/ActivateDeployment" => {
            if body.get("expectedActive").and_then(Value::as_str) != live.active.as_deref() {
                return conflict();
            }
            let id = body["id"].as_str().unwrap().to_owned();
            if let Some(current) = live.active.clone() {
                set_state(live, &current, "DEPLOYMENT_STATE_SUPERSEDED");
            }
            set_state(live, &id, "DEPLOYMENT_STATE_ACTIVE");
            live.active = Some(id.clone());
            (200, json!({ "deployment": find(live, &id) }))
        }
        "/loams.live.v1.LiveService/Query" => {
            if live.unhealthy {
                (
                    500,
                    json!({"code": "internal", "message": "health:check threw"}),
                )
            } else {
                (200, json!({ "result": { "stringValue": "ok" } }))
            }
        }
        _ => (404, json!({"code": "unimplemented", "message": path})),
    }
}

/// A Live with one active deployment, `d0`.
async fn fake_live() -> (String, Live) {
    use axum::Json;
    use axum::body::Bytes;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode, Uri};

    let live: Live = Arc::default();
    {
        let mut state = live.lock().unwrap();
        state.deployments.push(json!({
            "id": "d0", "bundleSha256": "00", "message": "by hand", "parent": "",
            "state": "DEPLOYMENT_STATE_ACTIVE", "createdBy": "owner",
        }));
        state.active = Some("d0".into());
    }
    let app = axum::Router::new()
        .fallback(
            |State(live): State<Live>, uri: Uri, headers: HeaderMap, body: Bytes| async move {
                let header = |name: &str| {
                    headers
                        .get(name)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_owned()
                };
                let body: Value = serde_json::from_slice(&body).unwrap_or_default();
                let mut state = live.lock().unwrap();
                state.seen.push(Seen {
                    path: uri.path().to_owned(),
                    app: header("loams-live-app"),
                    authorization: header("authorization"),
                    body: body.clone(),
                });
                let (status, reply) = answer(&mut state, uri.path(), &body);
                (
                    StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                    Json(reply),
                )
            },
        )
        .with_state(live.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (base, live)
}

struct Factory {
    _tmp: tempfile::TempDir,
    core: EngineCore,
    client: RpcClient,
    commit: String,
    live: Live,
}

impl Factory {
    async fn start(health: bool) -> Self {
        environment();
        let tmp = tempfile::tempdir().unwrap();
        let (repo, commit) = live_repo(&tmp.path().join("repo"), health);
        let (base, live) = fake_live().await;
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
        .with_approvals(core.approvals.clone())
        .with_live_deployer(LiveDeployer::new(Arc::new(StaticLiveCredentials::new(
            Some(LiveCredential {
                base_url: base,
                token: Some(TOKEN.into()),
            }),
        ))));
        let client = loams_agentd_rpc::memory_client(Arc::new(rpc));
        client
            .call(
                methods::MUTATE,
                json!({"op": "createChat", "chatId": CHAT, "deviceId": core.device_id, "spaceId": SPACE}),
            )
            .await
            .unwrap();
        Self {
            _tmp: tmp,
            core,
            client,
            commit,
            live,
        }
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        self.client
            .call(method, params)
            .await
            .map_err(|e| e.to_string())
    }

    /// A deploy approval for `commit`, as the factory host asks it.
    async fn ask(&self, commit: &str) -> String {
        let asked = self
            .call(
                methods::REQUEST_APPROVAL,
                json!({"chatId": CHAT, "kind": "deploy",
                       "summary": format!("Deploy {commit} to Live app {APP}"),
                       "payload": deploy_payload(APP, commit)}),
            )
            .await
            .unwrap();
        asked["id"].as_str().unwrap().to_owned()
    }

    /// A deploy approval a person approved.
    async fn approved(&self) -> String {
        let id = self.ask(&self.commit).await;
        self.core
            .approvals
            .resolve(Some(CHAT), &id, ApprovalDecision::Once)
            .unwrap();
        id
    }

    async fn deploy(&self, approval: &str) -> Result<Value, String> {
        self.call(
            methods::DEPLOY_LIVE_APP,
            json!({"chatId": CHAT, "approvalId": approval, "app": APP,
                   "commit": self.commit, "message": "Rate limit signup (#7)"}),
        )
        .await
    }

    fn calls(&self, method: &str) -> Vec<Seen> {
        self.live
            .lock()
            .unwrap()
            .seen
            .iter()
            .filter(|s| s.path.ends_with(method))
            .cloned()
            .collect()
    }

    async fn stop(self) {
        self.core.shutdown().await;
    }
}

fn bundle_sha() -> String {
    Sha256::digest(BUNDLE.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn deploy_is_an_mcp_method() {
    let spec = method_spec(methods::DEPLOY_LIVE_APP).unwrap();
    assert_eq!(spec.role, Role::Mcp);
    assert!(!spec.renderer);
}

#[tokio::test(flavor = "multi_thread")]
async fn deploy_requires_settled_approval() {
    let factory = Factory::start(false).await;
    // Unknown, pending, denied.
    let error = factory.deploy("fa-none").await.unwrap_err();
    assert!(error.contains(APPROVAL_REQUIRED), "{error}");
    let pending = factory.ask(&factory.commit).await;
    let error = factory.deploy(&pending).await.unwrap_err();
    assert!(
        error.contains(APPROVAL_REQUIRED) && error.contains("pending"),
        "{error}"
    );
    factory
        .core
        .approvals
        .resolve(Some(CHAT), &pending, ApprovalDecision::Deny)
        .unwrap();
    let error = factory.deploy(&pending).await.unwrap_err();
    assert!(
        error.contains(APPROVAL_REQUIRED) && error.contains("denied"),
        "{error}"
    );
    // Approved for another commit, or a merge approval: a mismatch.
    let other = factory.ask(&"b".repeat(40)).await;
    factory
        .core
        .approvals
        .resolve(Some(CHAT), &other, ApprovalDecision::Once)
        .unwrap();
    let error = factory.deploy(&other).await.unwrap_err();
    assert!(error.contains(APPROVAL_MISMATCH), "{error}");
    let merge = factory
        .call(
            methods::REQUEST_APPROVAL,
            json!({"chatId": CHAT, "kind": "merge", "summary": "Merge #7",
                   "payload": deploy_payload(APP, &factory.commit)}),
        )
        .await
        .unwrap();
    let merge = merge["id"].as_str().unwrap().to_owned();
    factory
        .core
        .approvals
        .resolve(Some(CHAT), &merge, ApprovalDecision::Once)
        .unwrap();
    let error = factory.deploy(&merge).await.unwrap_err();
    assert!(error.contains(APPROVAL_MISMATCH), "{error}");
    // Another app than the commit's factory.toml names: refused before
    // the approval is looked at.
    let error = factory
        .call(
            methods::DEPLOY_LIVE_APP,
            json!({"chatId": CHAT, "approvalId": other, "app": "billing", "commit": factory.commit}),
        )
        .await
        .unwrap_err();
    assert!(error.contains("deploys app"), "{error}");
    // A short commit id is refused.
    let error = factory
        .call(
            methods::DEPLOY_LIVE_APP,
            json!({"chatId": CHAT, "approvalId": other, "app": APP, "commit": &factory.commit[..12]}),
        )
        .await
        .unwrap_err();
    assert!(error.contains("full commit id"), "{error}");
    assert!(
        factory.calls("/Deploy").is_empty(),
        "nothing reached Live without an approval"
    );
    factory.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn deploy_activates_on_health() {
    let factory = Factory::start(true).await;
    let approval = factory.approved().await;
    let reply = factory.deploy(&approval).await.unwrap();
    assert_eq!(reply["state"], "active", "{reply}");
    assert_eq!(reply["target"], "live");
    assert_eq!(reply["app"], APP);
    assert_eq!(reply["commit"], json!(factory.commit));
    assert_eq!(reply["deploymentId"], "d1");
    assert_eq!(reply["previousDeploymentId"], "d0");
    assert_eq!(reply["bundleSha256"], json!(bundle_sha()));
    assert_eq!(reply["resumed"], false);
    let deploys = factory.calls("/Deploy");
    assert_eq!(deploys.len(), 1);
    let deploy = &deploys[0];
    assert_eq!(deploy.app, APP);
    assert_eq!(deploy.authorization, format!("Bearer {TOKEN}"));
    assert_eq!(deploy.body["expectedActive"], "d0");
    // The committed bundle, not the edited working tree.
    let sent = base64::engine::general_purpose::STANDARD
        .decode(deploy.body["bundle"].as_str().unwrap())
        .unwrap();
    assert_eq!(sent, BUNDLE.as_bytes());
    assert_eq!(deploy.body["schema"]["tables"][0]["name"], "notes");
    let message = deploy.body["message"].as_str().unwrap();
    assert!(
        message.starts_with("Rate limit signup (#7)")
            && message.contains(&format!("factory deploy {approval}")),
        "{message}"
    );
    let health = factory.calls(".LiveService/Query");
    assert_eq!(health.len(), 1);
    assert_eq!(health[0].body["function"], "health:check");
    assert!(factory.calls("/ActivateDeployment").is_empty());
    assert_eq!(factory.live.lock().unwrap().active.as_deref(), Some("d1"));
    factory.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn deploy_rolls_back_on_failed_health() {
    let factory = Factory::start(true).await;
    factory.live.lock().unwrap().unhealthy = true;
    let approval = factory.approved().await;
    let reply = factory.deploy(&approval).await.unwrap();
    assert_eq!(reply["state"], "rolled_back", "{reply}");
    assert_eq!(reply["deploymentId"], "d1");
    assert_eq!(reply["previousDeploymentId"], "d0");
    assert!(
        reply["health"].as_str().unwrap().contains("health:check"),
        "{reply}"
    );
    let rollbacks = factory.calls("/ActivateDeployment");
    assert_eq!(rollbacks.len(), 1);
    assert_eq!(
        rollbacks[0].body,
        json!({"id": "d0", "expectedActive": "d1"})
    );
    assert_eq!(factory.live.lock().unwrap().active.as_deref(), Some("d0"));
    // Asked again with the same approval: the earlier call's answer, found,
    // not a second deploy or a second rollback.
    let again = factory.deploy(&approval).await.unwrap();
    assert_eq!(again["state"], "rolled_back", "{again}");
    assert_eq!(again["resumed"], true);
    assert_eq!(factory.calls("/Deploy").len(), 1);
    assert_eq!(factory.calls("/ActivateDeployment").len(), 1);
    factory.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn deploy_step_is_idempotent() {
    let factory = Factory::start(false).await;
    let approval = factory.approved().await;
    let first = factory.deploy(&approval).await.unwrap();
    assert_eq!(first["state"], "active", "{first}");
    assert_eq!(first["resumed"], false);
    // The step runs again (a crash after the call, a resumed workflow).
    let second = factory.deploy(&approval).await.unwrap();
    assert_eq!(second["deploymentId"], first["deploymentId"]);
    assert_eq!(second["state"], "active");
    assert_eq!(second["resumed"], true);
    assert_eq!(factory.calls("/Deploy").len(), 1, "one deploy per approval");
    let used = factory
        .call(
            methods::GET_APPROVAL,
            json!({"chatId": CHAT, "approvalId": approval}),
        )
        .await
        .unwrap();
    assert_eq!(used["state"], "used");
    // A spent approval is still refused to another chat.
    factory
        .call(
            methods::MUTATE,
            json!({"op": "createChat", "chatId": "other", "deviceId": factory.core.device_id, "spaceId": SPACE}),
        )
        .await
        .unwrap();
    let error = factory
        .call(
            methods::DEPLOY_LIVE_APP,
            json!({"chatId": "other", "approvalId": approval, "app": APP, "commit": factory.commit}),
        )
        .await
        .unwrap_err();
    assert!(error.contains(APPROVAL_MISMATCH), "{error}");
    factory.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deploy_that_never_reached_live_runs_once_on_retry() {
    let factory = Factory::start(false).await;
    factory.live.lock().unwrap().drop_next_deploy = true;
    let approval = factory.approved().await;
    let error = factory.deploy(&approval).await.unwrap_err();
    assert!(error.contains("unavailable"), "{error}");
    // The approval is spent, Live has nothing of it: the retry deploys.
    let retry = factory.deploy(&approval).await.unwrap();
    assert_eq!(retry["state"], "active", "{retry}");
    assert_eq!(retry["resumed"], true);
    assert_eq!(retry["deploymentId"], "d1");
    let again = factory.deploy(&approval).await.unwrap();
    assert_eq!(again["deploymentId"], "d1");
    let recorded = factory
        .live
        .lock()
        .unwrap()
        .deployments
        .iter()
        .filter(|d| d["id"] != "d0")
        .count();
    assert_eq!(recorded, 1, "exactly one deployment");
    factory.stop().await;
}
