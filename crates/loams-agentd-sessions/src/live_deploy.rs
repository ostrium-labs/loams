//! The Software Factory's deploy step on Loams Live (design §72 §8, D1434;
//! plan FT1, FT0 Task 8). It runs in agentd, which holds the Live
//! credential (owner ruling, 2026-10-24): the factory host only names the
//! app and the commit, with a settled deploy approval.
//!
//! What is deployed is read from the commit itself, never from a working
//! tree: the repository's `.loams/factory.toml` at that commit names the
//! bundle and the schema,
//!
//! ```toml
//! [deploy.live]
//! app = "notes"                 # optional; when set, the call's app must match
//! bundle = "live/dist/app.js"   # one ES module (design §45 §3.3)
//! schema = "live/schema.json"   # optional; loams.live.v1.Schema as proto JSON
//! health = "health:check"       # optional; a query that must answer
//! settle_ms = 2000              # optional; how long to watch for a crash loop
//! ```
//!
//! so the approved `{app, commit}` fixes the bytes. FT0 builds nothing:
//! the bundle is a file of the merge commit (building with esbuild waits for
//! `@loams/live-cli`, design §45 §9).
//!
//! The order (Task 0 ruling FT0-0-6): `Deploy` with `expected_active` = the
//! active deployment (Live activates it by compare-and-swap and answers when
//! it is active or failed), then the health check, then on a failed check
//! `ActivateDeployment` of the previous one. Every deployment's message
//! carries `factory deploy <approval id>`, so a call whose approval an
//! earlier call already spent finds that call's deployment and answers it
//! instead of deploying twice (`deploy_step_is_idempotent`).

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use loams_agentd_proto::factory_loop::{DeployLiveAppReply, LiveDeployState};
use loams_agentd_proto::git_write::PROVIDER_NOT_CONFIGURED;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::change_request_create::KEYRING_SERVICE;
use crate::git_write::{GitWriteError, bad, failed, git_try, redact};

/// The Live credential's engine URL (`factory:<app>:<field>`).
pub const LIVE_URL_ACCOUNT: &str = "factory:live:url";
/// Its bearer token; absent for an engine without authentication (the
/// local light stack's default).
pub const LIVE_TOKEN_ACCOUNT: &str = "factory:live:token";

/// The repository's factory policy file.
pub const FACTORY_TOML: &str = ".loams/factory.toml";
/// Live's per-app header (`loams_live::apps::APP_HEADER`).
const APP_HEADER: &str = "loams-live-app";
/// Design §45 §3.2's bundle limit.
pub const MAX_BUNDLE_BYTES: usize = 16 * 1024 * 1024;
const MAX_SCHEMA_BYTES: usize = 1024 * 1024;
const MAX_CONFIG_BYTES: usize = 64 * 1024;
const DEFAULT_SETTLE_MS: u64 = 2_000;
const MAX_SETTLE_MS: u64 = 60_000;
/// `Deploy` answers once validation scans and index backfills finish.
const DEPLOY_TIMEOUT: Duration = Duration::from_secs(600);
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
const GIT_TIMEOUT: Duration = Duration::from_secs(120);

/// A Live engine and the token for it.
#[derive(Clone, PartialEq, Eq)]
pub struct LiveCredential {
    /// For example `http://127.0.0.1:8080` (Live is served on the engine's
    /// own listener, D688).
    pub base_url: String,
    pub token: Option<String>,
}

impl std::fmt::Debug for LiveCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveCredential")
            .field("base_url", &self.base_url)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// Where the daemon's Live credential comes from.
pub trait LiveCredentials: Send + Sync + std::fmt::Debug {
    /// The credential, if one is configured.
    ///
    /// # Errors
    ///
    /// The store could not be read (for example, no keyring).
    fn live(&self) -> Result<Option<LiveCredential>, String>;
}

/// The OS keyring, service `dev.loams.agentd`.
#[derive(Debug, Default, Clone, Copy)]
pub struct KeyringLiveCredentials;

fn keyring_read(account: &str) -> Result<Option<String>, String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, account).map_err(|e| e.to_string())?;
    match entry.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

impl LiveCredentials for KeyringLiveCredentials {
    fn live(&self) -> Result<Option<LiveCredential>, String> {
        let Some(base_url) = keyring_read(LIVE_URL_ACCOUNT)? else {
            return Ok(None);
        };
        let token = keyring_read(LIVE_TOKEN_ACCOUNT)?.filter(|t| !t.trim().is_empty());
        Ok(Some(LiveCredential { base_url, token }))
    }
}

/// A fixed credential (tests, and a host without a keyring).
#[derive(Debug, Clone)]
pub struct StaticLiveCredentials(Option<LiveCredential>);

impl StaticLiveCredentials {
    pub fn new(live: Option<LiveCredential>) -> Self {
        Self(live)
    }
}

impl LiveCredentials for StaticLiveCredentials {
    fn live(&self) -> Result<Option<LiveCredential>, String> {
        Ok(self.0.clone())
    }
}

fn not_configured(detail: impl std::fmt::Display) -> GitWriteError {
    failed(format!("{PROVIDER_NOT_CONFIGURED}: {detail}"))
}

/// `[deploy.live]` of `.loams/factory.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveDeployConfig {
    pub app: Option<String>,
    pub bundle: String,
    pub schema: Option<String>,
    pub health: Option<String>,
    pub settle_ms: u64,
}

/// A Live app name: lowercase letters, digits, `-` and `_`, 1 to 63.
pub fn valid_app(app: &str) -> bool {
    (1..=63).contains(&app.len())
        && app
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// A full commit id: 40 (SHA-1) or 64 (SHA-256) hex digits.
pub fn valid_commit(commit: &str) -> bool {
    matches!(commit.len(), 40 | 64) && commit.chars().all(|c| c.is_ascii_hexdigit())
}

/// A path inside the repository: relative, `/`-separated, no `..`.
fn valid_repo_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.starts_with('-')
        && !path.contains('\\')
        && !path.contains(':')
        && path
            .split('/')
            .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

/// Parse `[deploy.live]` from the policy file's text.
pub fn parse_deploy_config(text: &str) -> Result<LiveDeployConfig, GitWriteError> {
    let doc: toml::Table = toml::from_str(text).map_err(|e| bad(format!("{FACTORY_TOML}: {e}")))?;
    let live = doc
        .get("deploy")
        .and_then(|d| d.get("live"))
        .and_then(toml::Value::as_table)
        .ok_or_else(|| bad(format!("{FACTORY_TOML} has no [deploy.live] table")))?;
    let text_field = |key: &str| -> Result<Option<String>, GitWriteError> {
        match live.get(key) {
            None => Ok(None),
            Some(toml::Value::String(s)) if !s.trim().is_empty() => Ok(Some(s.trim().to_owned())),
            Some(_) => Err(bad(format!("deploy.live.{key} must be a non-empty string"))),
        }
    };
    let bundle =
        text_field("bundle")?.ok_or_else(|| bad("deploy.live.bundle names no file".to_owned()))?;
    let schema = text_field("schema")?;
    for path in std::iter::once(&bundle).chain(schema.iter()) {
        if !valid_repo_path(path) {
            return Err(bad(format!("not a path in the repository: {path:?}")));
        }
    }
    let app = text_field("app")?;
    if let Some(app) = &app
        && !valid_app(app)
    {
        return Err(bad(format!("deploy.live.app is not an app name: {app:?}")));
    }
    let health = text_field("health")?;
    if let Some(health) = &health
        && !health.contains(':')
    {
        return Err(bad(format!(
            "deploy.live.health is a function path (module:export), not {health:?}"
        )));
    }
    let settle_ms = match live.get("settle_ms") {
        None => DEFAULT_SETTLE_MS,
        Some(toml::Value::Integer(n)) if (0..=MAX_SETTLE_MS as i64).contains(n) => *n as u64,
        Some(_) => {
            return Err(bad(format!(
                "deploy.live.settle_ms is 0 to {MAX_SETTLE_MS}"
            )));
        }
    };
    Ok(LiveDeployConfig {
        app,
        bundle,
        schema,
        health,
        settle_ms,
    })
}

/// A file of `commit`, as bytes, at most `limit`.
async fn blob_at(
    root: &Path,
    commit: &str,
    path: &str,
    limit: usize,
) -> Result<Vec<u8>, GitWriteError> {
    let mut command = tokio::process::Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args(["cat-file", "blob", &format!("{commit}:{path}")])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(GIT_TIMEOUT, command.output())
        .await
        .map_err(|_| failed("git timed out"))?
        .map_err(|e| failed(format!("git failed: {e}")))?;
    if !output.status.success() {
        return Err(bad(format!(
            "commit {} has no file {path}",
            &commit[..12.min(commit.len())]
        )));
    }
    if output.stdout.len() > limit {
        return Err(bad(format!("{path} is larger than {limit} bytes")));
    }
    Ok(output.stdout)
}

/// What a deploy reads from its commit.
#[derive(Debug, Clone)]
pub struct LiveRelease {
    pub config: LiveDeployConfig,
    pub bundle: Vec<u8>,
    pub schema: Value,
}

/// Read the release at `commit` of the repository at `root`, fetching the
/// commit from `origin` when this clone does not have it yet (a merge made
/// on the code host).
pub async fn read_release(root: &Path, commit: &str) -> Result<LiveRelease, GitWriteError> {
    if !valid_commit(commit) {
        return Err(bad(format!(
            "a deploy names a full commit id, not {commit:?}"
        )));
    }
    let object = format!("{commit}^{{commit}}");
    if git_try(root, &["cat-file", "-e", &object]).await.is_none() {
        // Best effort: an unreachable commit is reported below.
        git_try(root, &["fetch", "--quiet", "--no-tags", "origin", commit]).await;
        if git_try(root, &["cat-file", "-e", &object]).await.is_none() {
            return Err(bad(format!("the repository has no commit {commit}")));
        }
    }
    let config = blob_at(root, commit, FACTORY_TOML, MAX_CONFIG_BYTES).await?;
    let config = parse_deploy_config(&String::from_utf8_lossy(&config))?;
    let bundle = blob_at(root, commit, &config.bundle, MAX_BUNDLE_BYTES).await?;
    let schema = match &config.schema {
        None => json!({}),
        Some(path) => {
            let bytes = blob_at(root, commit, path, MAX_SCHEMA_BYTES).await?;
            let value: Value = serde_json::from_slice(&bytes)
                .map_err(|e| bad(format!("{path} is not JSON: {e}")))?;
            if !value.is_object() {
                return Err(bad(format!(
                    "{path} is a loams.live.v1.Schema object in proto JSON"
                )));
            }
            value
        }
    };
    Ok(LiveRelease {
        config,
        bundle,
        schema,
    })
}

/// A deployment as `LiveAdminService` answers it (proto JSON).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Deployment {
    id: String,
    bundle_sha256: String,
    message: String,
    parent: String,
    state: String,
    failure_reason: String,
}

impl Deployment {
    fn from_json(value: &Value) -> Option<Self> {
        let text = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let id = text("id");
        if id.is_empty() {
            return None;
        }
        Some(Self {
            id,
            bundle_sha256: text("bundleSha256"),
            message: text("message"),
            parent: text("parent"),
            state: text("state"),
            failure_reason: text("failureReason"),
        })
    }

    /// The pointer names it (degraded is active, and serving).
    fn is_active(&self) -> bool {
        matches!(
            self.state.as_str(),
            "DEPLOYMENT_STATE_ACTIVE" | "DEPLOYMENT_STATE_DEGRADED"
        )
    }
}

/// The marker every factory deployment's message carries.
fn marker(approval_id: &str) -> String {
    format!("factory deploy {approval_id}")
}

/// Deploys to Live for the factory; one at a time per daemon, so a retry
/// that arrives while the first call still runs waits for it and then finds
/// its deployment.
#[derive(Debug, Clone)]
pub struct LiveDeployer {
    credentials: Arc<dyn LiveCredentials>,
    http: reqwest::Client,
    lock: Arc<tokio::sync::Mutex<()>>,
}

impl LiveDeployer {
    pub fn new(credentials: Arc<dyn LiveCredentials>) -> Self {
        Self {
            credentials,
            http: reqwest::Client::builder()
                // The token never follows a redirect anywhere.
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            lock: Arc::default(),
        }
    }

    /// The daemon's own: the OS keyring.
    pub fn from_keyring() -> Self {
        Self::new(Arc::new(KeyringLiveCredentials))
    }

    /// Hold while spending the approval and deploying.
    pub async fn serialize(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.lock.lock().await
    }

    fn credential(&self) -> Result<(reqwest::Url, Option<String>), GitWriteError> {
        let credential = self
            .credentials
            .live()
            .map_err(|e| not_configured(format!("the keyring could not be read: {e}")))?
            .ok_or_else(|| not_configured("no Live credential is configured"))?;
        let url = reqwest::Url::parse(credential.base_url.trim())
            .map_err(|_| not_configured("the Live credential's URL is not valid"))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(not_configured("the Live credential's URL is not http(s)"));
        }
        Ok((url, credential.token))
    }

    /// One Connect JSON unary call of `service`/`method` for `app`.
    async fn call(
        &self,
        app: &str,
        service: &str,
        method: &str,
        body: &Value,
        timeout: Duration,
    ) -> Result<Result<Value, (String, String)>, GitWriteError> {
        let (base, token) = self.credential()?;
        let mut url = base;
        url.path_segments_mut()
            .map_err(|_| not_configured("the Live credential's URL is not valid"))?
            .pop_if_empty()
            .extend([service, method]);
        let mut request = self
            .http
            .post(url)
            .timeout(timeout)
            .header("content-type", "application/json")
            .header("connect-protocol-version", "1")
            .header(APP_HEADER, app)
            .json(body);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.map_err(|error| {
            failed(format!(
                "live: the request failed: {}",
                redact(&error.without_url().to_string())
            ))
        })?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(not_configured(format!(
                "Live refused the credential (HTTP {})",
                status.as_u16()
            )));
        }
        let reply: Value = response.json().await.unwrap_or_default();
        if status.is_success() {
            return Ok(Ok(reply));
        }
        let code = reply
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        let message: String = reply
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .chars()
            .take(512)
            .collect();
        Ok(Err((code, redact(&message))))
    }

    async fn admin(
        &self,
        app: &str,
        method: &str,
        body: &Value,
        timeout: Duration,
    ) -> Result<Value, GitWriteError> {
        self.call(app, "loams.live.v1.LiveAdminService", method, body, timeout)
            .await?
            .map_err(|(code, message)| failed(format!("live: {method}: {code}: {message}")))
    }

    /// The newest page of the app's deployments, newest first.
    async fn deployments(&self, app: &str) -> Result<Vec<Deployment>, GitWriteError> {
        let reply = self
            .admin(
                app,
                "ListDeployments",
                &json!({ "pageSize": 100 }),
                CALL_TIMEOUT,
            )
            .await?;
        Ok(reply
            .get("deployments")
            .and_then(Value::as_array)
            .map(|list| list.iter().filter_map(Deployment::from_json).collect())
            .unwrap_or_default())
    }

    async fn deployment(&self, app: &str, id: &str) -> Result<Deployment, GitWriteError> {
        let reply = self
            .admin(app, "GetDeployment", &json!({ "id": id }), CALL_TIMEOUT)
            .await?;
        reply
            .get("deployment")
            .and_then(Deployment::from_json)
            .ok_or_else(|| failed(format!("live: no deployment {id}")))
    }

    /// Whether `deployment` is healthy: still active, not degraded, after
    /// the settle time, and the health query (if any) answers.
    async fn health(
        &self,
        app: &str,
        deployment: &Deployment,
        config: &LiveDeployConfig,
    ) -> Result<(bool, String), GitWriteError> {
        if config.settle_ms > 0 {
            tokio::time::sleep(Duration::from_millis(config.settle_ms)).await;
        }
        let now = self.deployment(app, &deployment.id).await?;
        if now.state != "DEPLOYMENT_STATE_ACTIVE" {
            return Ok((
                false,
                format!("the deployment is {}", short_state(&now.state)),
            ));
        }
        if let Some(function) = &config.health {
            let answer = self
                .call(
                    app,
                    "loams.live.v1.LiveService",
                    "Query",
                    &json!({ "function": function, "args": { "objectValue": { "fields": {} } } }),
                    CALL_TIMEOUT,
                )
                .await?;
            if let Err((code, message)) = answer {
                return Ok((false, format!("{function} failed: {code}: {message}")));
            }
            return Ok((true, format!("active; {function} answered")));
        }
        Ok((true, "active".to_owned()))
    }

    /// Check `deployment`'s health and roll back to `previous` on failure.
    async fn settle(
        &self,
        app: &str,
        deployment: &Deployment,
        previous: &str,
        config: &LiveDeployConfig,
    ) -> Result<(LiveDeployState, String), GitWriteError> {
        let (healthy, health) = self.health(app, deployment, config).await?;
        if healthy {
            return Ok((LiveDeployState::Active, health));
        }
        if previous.is_empty() {
            return Ok((LiveDeployState::Unhealthy, health));
        }
        self.admin(
            app,
            "ActivateDeployment",
            &json!({ "id": previous, "expectedActive": deployment.id }),
            DEPLOY_TIMEOUT,
        )
        .await?;
        Ok((LiveDeployState::RolledBack, health))
    }

    /// Deploy `release` of `commit` to `app` under the spent approval
    /// `approval_id`. With `resumed`, an earlier call spent it: its
    /// deployment, when Live has one, is answered (and settled, if it is
    /// still the active one) instead of a second deploy.
    pub async fn deploy(
        &self,
        app: &str,
        commit: &str,
        approval_id: &str,
        message: &str,
        release: &LiveRelease,
        resumed: bool,
    ) -> Result<DeployLiveAppReply, GitWriteError> {
        let bundle_sha256 = hex(&Sha256::digest(&release.bundle));
        let reply = |deployment: &Deployment,
                     previous: &str,
                     state: LiveDeployState,
                     health: String,
                     resumed: bool| DeployLiveAppReply {
            target: "live".to_owned(),
            app: app.to_owned(),
            commit: commit.to_owned(),
            deployment_id: deployment.id.clone(),
            bundle_sha256: if deployment.bundle_sha256.is_empty() {
                bundle_sha256.clone()
            } else {
                deployment.bundle_sha256.clone()
            },
            previous_deployment_id: previous.to_owned(),
            state,
            health,
            failure_reason: Some(deployment.failure_reason.clone()).filter(|r| !r.is_empty()),
            resumed,
        };
        let list = self.deployments(app).await?;
        let active = list.iter().find(|d| d.is_active()).cloned();
        if resumed {
            let mark = marker(approval_id);
            if let Some(found) = list.iter().find(|d| d.message.contains(&mark)) {
                let previous = found.parent.clone();
                let (state, health) = match found.state.as_str() {
                    "DEPLOYMENT_STATE_FAILED" => (LiveDeployState::Failed, String::new()),
                    "DEPLOYMENT_STATE_PENDING"
                    | "DEPLOYMENT_STATE_VALIDATING"
                    | "DEPLOYMENT_STATE_BACKFILLING" => (LiveDeployState::Pending, String::new()),
                    "DEPLOYMENT_STATE_SUPERSEDED"
                        if !previous.is_empty()
                            && active.as_ref().is_some_and(|a| a.id == previous) =>
                    {
                        (
                            LiveDeployState::RolledBack,
                            "rolled back by an earlier call".to_owned(),
                        )
                    }
                    "DEPLOYMENT_STATE_SUPERSEDED" => (LiveDeployState::Superseded, String::new()),
                    _ if active.as_ref().is_some_and(|a| a.id == found.id) => {
                        self.settle(app, found, &previous, &release.config).await?
                    }
                    other => (
                        LiveDeployState::Superseded,
                        format!("the deployment is {}", short_state(other)),
                    ),
                };
                return Ok(reply(found, &previous, state, health, true));
            }
            // The earlier call spent the approval but never reached Live:
            // this call deploys, once.
        }
        let previous = active.map(|a| a.id).unwrap_or_default();
        let note = message.trim();
        let full_message = if note.is_empty() {
            format!("{} of {commit}", marker(approval_id))
        } else {
            let note: String = note.chars().take(500).collect();
            format!("{note} ({} of {commit})", marker(approval_id))
        };
        let mut request = json!({
            "bundle": base64::engine::general_purpose::STANDARD.encode(&release.bundle),
            "schema": release.schema,
            "message": full_message,
        });
        if !previous.is_empty() {
            request["expectedActive"] = json!(previous);
        }
        let answer = self.admin(app, "Deploy", &request, DEPLOY_TIMEOUT).await?;
        let deployment = answer
            .get("deployment")
            .and_then(Deployment::from_json)
            .ok_or_else(|| failed("live: Deploy answered no deployment"))?;
        if deployment.state == "DEPLOYMENT_STATE_FAILED" {
            return Ok(reply(
                &deployment,
                &previous,
                LiveDeployState::Failed,
                String::new(),
                resumed,
            ));
        }
        let (state, health) = self
            .settle(app, &deployment, &previous, &release.config)
            .await?;
        Ok(reply(&deployment, &previous, state, health, resumed))
    }
}

fn short_state(state: &str) -> String {
    state
        .strip_prefix("DEPLOYMENT_STATE_")
        .unwrap_or(state)
        .to_ascii_lowercase()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deploy_config_reads_the_live_table() {
        let config = parse_deploy_config(
            "[tools]\ndeny = []\n[deploy.live]\napp = \"notes\"\nbundle = \"live/app.js\"\nschema = \"live/schema.json\"\nhealth = \"health:check\"\nsettle_ms = 0\n",
        )
        .unwrap();
        assert_eq!(config.app.as_deref(), Some("notes"));
        assert_eq!(config.bundle, "live/app.js");
        assert_eq!(config.schema.as_deref(), Some("live/schema.json"));
        assert_eq!(config.health.as_deref(), Some("health:check"));
        assert_eq!(config.settle_ms, 0);
        let defaults = parse_deploy_config("[deploy.live]\nbundle = \"app.js\"\n").unwrap();
        assert_eq!(defaults.settle_ms, DEFAULT_SETTLE_MS);
        assert!(defaults.schema.is_none() && defaults.app.is_none());
    }

    #[test]
    fn deploy_config_refuses_paths_outside_the_repository() {
        for bundle in ["../x.js", "/etc/passwd", "a/../../b", "-x", "c:\\x", ""] {
            let text = format!("[deploy.live]\nbundle = {bundle:?}\n");
            assert!(parse_deploy_config(&text).is_err(), "{bundle}");
        }
        assert!(
            parse_deploy_config("[tools]\n").is_err(),
            "no [deploy.live]"
        );
        assert!(
            parse_deploy_config("[deploy.live]\nbundle = \"a.js\"\nsettle_ms = 999999\n").is_err()
        );
        assert!(
            parse_deploy_config("[deploy.live]\nbundle = \"a.js\"\napp = \"Bad App\"\n").is_err()
        );
    }

    #[test]
    fn commits_are_full_ids() {
        assert!(valid_commit(&"a".repeat(40)));
        assert!(valid_commit(&"0".repeat(64)));
        assert!(!valid_commit("abcdef0"));
        assert!(!valid_commit(&"g".repeat(40)));
    }

    #[test]
    fn the_credential_never_prints_its_token() {
        let credential = LiveCredential {
            base_url: "http://127.0.0.1:1".into(),
            token: Some("live-secret".into()),
        };
        assert!(!format!("{credential:?}").contains("live-secret"));
    }
}
