//! The providers under test, and the env gate that selects them (D2120).
//!
//! Both providers run the same engine, Obscura 0.2.3, and differ only in how it
//! is reached:
//!
//! - **`local`**: the suite launches the pinned binary itself, through the
//!   same [`ObscuraEngine`] the Desktop uses. It binds loopback, and each
//!   scenario gets a fresh engine and profile directory.
//! - **`service`**: the suite connects to an already-running engine, the
//!   `browser` service in compose or k8s, over its DevTools WebSocket with a
//!   bearer token. The engine is shared, so isolation comes from one browser
//!   context per scenario.
//!
//! # Environment
//!
//! | Variable | Meaning |
//! |---|---|
//! | `LOAMS_BROWSER_PARITY_PROVIDERS` | `local`, `service`, or `local,service`. Unset: every scenario reports *skipped* and passes. |
//! | `LOAMS_OBSCURA_BIN` | `local`: the engine binary (defaults to `obscura` on `PATH`). |
//! | `LOAMS_BROWSER_SERVICE_URL` | `service`: e.g. `ws://127.0.0.1:9222/devtools/browser`. |
//! | `LOAMS_BROWSER_SERVICE_TOKEN` | `service`: the engine's `OBSCURA_CDP_TOKEN`. Optional on a loopback bind. |
//! | `LOAMS_BROWSER_PARITY_REQUIRE` | `1`: a selected provider with missing config fails instead of skipping. CI sets it. |
//! | `LOAMS_BROWSER_PARITY_REPORT` | A file that receives one JSON line per (provider, scenario). |
//! | `LOAMS_BROWSER_PARITY_CANARY_ENV` | The name of an env var holding a canary Loams token. The credential-boundary scenario checks the engine never sees its value. |

use std::path::{Path, PathBuf};
use std::time::Duration;

use loams_sidebar_browser::{
    CdpClient, EmbeddedOrigin, EngineLaunchSpec, ObscuraEngine, ProfileKey, resolve_program,
};
use serde_json::{Value, json};
use tempfile::TempDir;

/// The env var that selects providers.
pub const PROVIDERS_ENV: &str = "LOAMS_BROWSER_PARITY_PROVIDERS";
/// The service's DevTools URL.
pub const SERVICE_URL_ENV: &str = "LOAMS_BROWSER_SERVICE_URL";
/// The service's bearer token.
pub const SERVICE_TOKEN_ENV: &str = "LOAMS_BROWSER_SERVICE_TOKEN";
/// Missing configuration is a failure, not a skip.
pub const REQUIRE_ENV: &str = "LOAMS_BROWSER_PARITY_REQUIRE";
/// Where the canary token lives.
pub const CANARY_ENV: &str = "LOAMS_BROWSER_PARITY_CANARY_ENV";

/// How long one CDP call may take before the suite calls it a failure.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(20);

/// Which provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Local,
    Service,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Local => "local",
            Kind::Service => "service",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "local" => Some(Kind::Local),
            "service" => Some(Kind::Service),
            _ => None,
        }
    }
}

/// The providers this run selected. Panics on an unknown name, because a typo
/// in CI would otherwise skip everything and look green.
pub fn selected() -> Vec<Kind> {
    let Ok(raw) = std::env::var(PROVIDERS_ENV) else {
        return Vec::new();
    };
    raw.split(',')
        .filter(|part| !part.trim().is_empty())
        .map(|part| {
            Kind::parse(part).unwrap_or_else(|| {
                panic!("{PROVIDERS_ENV}: unknown provider {part:?}; expected local or service")
            })
        })
        .collect()
}

/// Whether missing configuration must fail.
pub fn required() -> bool {
    std::env::var(REQUIRE_ENV).is_ok_and(|value| value == "1" || value == "true")
}

/// The canary Loams token, when the run supplies one.
pub fn canary() -> Option<String> {
    let name = std::env::var(CANARY_ENV).ok()?;
    std::env::var(name).ok().filter(|value| value.len() >= 16)
}

/// What the scenario is allowed to rely on, per provider.
#[derive(Clone, Copy, Debug)]
pub struct Capabilities {
    /// Cookies survive an engine restart through `--storage-dir` alone. Only
    /// a provider whose engine the suite launches can be restarted.
    pub engine_restart: bool,
    /// The engine's process environment is readable (`/proc/<pid>/environ`).
    pub process_env: bool,
}

/// A connected provider, for one scenario.
#[derive(Debug)]
pub struct Provider {
    pub kind: Kind,
    pub client: CdpClient,
    pub profile_dir: PathBuf,
    engine: Option<ObscuraEngine>,
    origin: EmbeddedOrigin,
    _scratch: TempDir,
}

impl Provider {
    /// Connect `kind` for a scenario whose pages live on `origin`.
    ///
    /// `Err` carries the reason the provider is not configured, which the
    /// caller turns into a skip or, under `LOAMS_BROWSER_PARITY_REQUIRE`, a
    /// failure.
    pub async fn connect(kind: Kind, origin: &EmbeddedOrigin) -> Result<Self, String> {
        let scratch = tempfile::Builder::new()
            .prefix("loams-browser-parity-")
            .tempdir()
            .map_err(|error| format!("a scratch directory: {error}"))?;
        let key = ProfileKey::new("parity", "fixture").map_err(|error| error.to_string())?;
        let profile_dir = key
            .ensure_profile_dir(scratch.path())
            .map_err(|error| error.to_string())?;
        match kind {
            Kind::Local => {
                let engine = launch(&profile_dir, origin).await?;
                let client = CdpClient::connect_with_timeout(&engine.devtools_url(), CALL_TIMEOUT)
                    .await
                    .map_err(|error| format!("connecting to the local engine: {error}"))?;
                Ok(Self {
                    kind,
                    client,
                    profile_dir,
                    engine: Some(engine),
                    origin: origin.clone(),
                    _scratch: scratch,
                })
            }
            Kind::Service => {
                let url = std::env::var(SERVICE_URL_ENV)
                    .map_err(|_| format!("{SERVICE_URL_ENV} is not set"))?;
                let token = std::env::var(SERVICE_TOKEN_ENV).unwrap_or_default();
                let client = if token.is_empty() {
                    CdpClient::connect_with_timeout(&url, CALL_TIMEOUT).await
                } else {
                    CdpClient::connect_with_bearer(&url, &token, CALL_TIMEOUT).await
                }
                .map_err(|error| format!("connecting to the browser service: {error}"))?;
                Ok(Self {
                    kind,
                    client,
                    profile_dir,
                    engine: None,
                    origin: origin.clone(),
                    _scratch: scratch,
                })
            }
        }
    }

    pub fn capabilities(&self) -> Capabilities {
        match self.kind {
            Kind::Local => Capabilities {
                engine_restart: true,
                process_env: cfg!(target_os = "linux"),
            },
            Kind::Service => Capabilities {
                engine_restart: false,
                process_env: false,
            },
        }
    }

    /// The local engine's pid.
    pub fn engine_pid(&self) -> Option<u32> {
        self.engine.as_ref().and_then(ObscuraEngine::id)
    }

    /// Stop the local engine and start a new one on the same profile directory,
    /// reconnecting the client. Only for providers with `engine_restart`.
    pub async fn restart_engine(&mut self) -> Result<(), String> {
        let Some(engine) = self.engine.take() else {
            return Err("this provider's engine is not the suite's to restart".to_string());
        };
        // Obscura (v0.2.3 `server.rs`) merges a connection's cookie changes
        // into `--storage-dir` when that WebSocket connection closes, and
        // flushes on SIGTERM only what closed connections merged. A
        // connection still open at shutdown loses its cookies (the first
        // two CI runs). So close the connection that did the work first:
        // swap in a second connection, `Browser.close` the original (the
        // engine closes it server-side), give the merge a moment, then
        // SIGTERM, then (in `stop`) kill.
        let spare = CdpClient::connect_with_timeout(&engine.devtools_url(), CALL_TIMEOUT)
            .await
            .map_err(|error| format!("a spare connection before the restart: {error}"))?;
        let working = std::mem::replace(&mut self.client, spare);
        working.close().await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        if let Some(pid) = engine.id() {
            let _ = tokio::process::Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .status()
                .await;
            let proc_dir = std::path::PathBuf::from(format!("/proc/{pid}"));
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while proc_dir.exists() && std::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        engine.stop().await.map_err(|error| error.to_string())?;
        let engine = launch(&self.profile_dir, &self.origin).await?;
        self.client = CdpClient::connect_with_timeout(&engine.devtools_url(), CALL_TIMEOUT)
            .await
            .map_err(|error| format!("reconnecting to the local engine: {error}"))?;
        self.engine = Some(engine);
        Ok(())
    }

    /// Open a fresh page. `isolated` gives it its own browser context; the
    /// default context is the one `--storage-dir` persists.
    pub async fn open(&mut self, isolated: bool) -> Result<PageHandle, String> {
        open_on(&mut self.client, isolated).await
    }

    /// A second, independent connection to the same engine, with its own
    /// per-call timeout. Sessions are per connection, so pages for it are
    /// opened with [`open_on`].
    pub async fn second_client(&self, call_timeout: Duration) -> Result<CdpClient, String> {
        match self.kind {
            Kind::Local => {
                let engine = self
                    .engine
                    .as_ref()
                    .ok_or("the local engine is not running")?;
                CdpClient::connect_with_timeout(&engine.devtools_url(), call_timeout)
                    .await
                    .map_err(|error| format!("a second connection: {error}"))
            }
            Kind::Service => {
                let url = std::env::var(SERVICE_URL_ENV)
                    .map_err(|_| format!("{SERVICE_URL_ENV} is not set"))?;
                let token = std::env::var(SERVICE_TOKEN_ENV).unwrap_or_default();
                if token.is_empty() {
                    CdpClient::connect_with_timeout(&url, call_timeout).await
                } else {
                    CdpClient::connect_with_bearer(&url, &token, call_timeout).await
                }
                .map_err(|error| format!("a second connection: {error}"))
            }
        }
    }

    /// Close a page and its context. Never `Browser.close`: the service is
    /// shared.
    pub async fn close(&mut self, page: &PageHandle) {
        close_on(&mut self.client, page).await;
    }

    /// Release the provider: stop a local engine.
    pub async fn shutdown(mut self) {
        if let Some(engine) = self.engine.take() {
            let _ = engine.stop().await;
        }
    }
}

/// Close a page opened on `client`.
pub async fn close_on(client: &mut CdpClient, page: &PageHandle) {
    let _ = client
        .call(
            "Target.closeTarget",
            json!({ "targetId": page.target_id }),
            None,
        )
        .await;
    if let Some(id) = &page.context_id {
        let _ = client
            .call(
                "Target.disposeBrowserContext",
                json!({ "browserContextId": id }),
                None,
            )
            .await;
    }
}

/// Open a page on `client`. See [`Provider::open`].
pub async fn open_on(client: &mut CdpClient, isolated: bool) -> Result<PageHandle, String> {
    let context_id = if isolated {
        let context = client
            .call("Target.createBrowserContext", json!({}), None)
            .await
            .map_err(|error| format!("Target.createBrowserContext: {error}"))?;
        Some(
            context
                .get("browserContextId")
                .and_then(Value::as_str)
                .ok_or("Target.createBrowserContext returned no browserContextId")?
                .to_string(),
        )
    } else {
        None
    };
    let mut params = json!({ "url": "about:blank" });
    if let Some(id) = &context_id {
        params["browserContextId"] = json!(id);
    }
    let target = client
        .call("Target.createTarget", params, None)
        .await
        .map_err(|error| format!("Target.createTarget: {error}"))?;
    let target_id = target
        .get("targetId")
        .and_then(Value::as_str)
        .ok_or("Target.createTarget returned no targetId")?
        .to_string();
    let attached = client
        .call(
            "Target.attachToTarget",
            json!({ "targetId": target_id, "flatten": true }),
            None,
        )
        .await
        .map_err(|error| format!("Target.attachToTarget: {error}"))?;
    let session_id = attached
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or("Target.attachToTarget returned no sessionId")?
        .to_string();
    for method in ["Page.enable", "Runtime.enable"] {
        client
            .call(method, json!({}), Some(&session_id))
            .await
            .map_err(|error| format!("{method}: {error}"))?;
    }
    Ok(PageHandle {
        session_id,
        target_id,
        context_id,
    })
}

/// One open page.
#[derive(Clone, Debug)]
pub struct PageHandle {
    pub session_id: String,
    pub target_id: String,
    pub context_id: Option<String>,
}

async fn launch(profile_dir: &Path, origin: &EmbeddedOrigin) -> Result<ObscuraEngine, String> {
    let port = free_port().map_err(|error| format!("a free port: {error}"))?;
    // `for_origin` decides the network policy for the fixture origin. It is
    // loopback, so the engine gets `--allow-private-network`, exactly as the
    // Desktop does for a console on 127.0.0.1.
    let spec = EngineLaunchSpec::for_origin(resolve_program(), profile_dir, port, origin)
        .map_err(|error| error.to_string())?;
    ObscuraEngine::start(spec, Duration::from_secs(30))
        .await
        .map_err(|error| error.to_string())
}

fn free_port() -> std::io::Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}
