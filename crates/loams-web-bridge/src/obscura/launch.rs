//! The local endpoint: an `obscura serve` process on loopback, started by
//! agentd or Desktop on the user's machine the first time a page is opened
//! (D2111).
//!
//! It is the same engine and the same authentication as the `browser`
//! service, so the two providers differ only in where the process runs:
//!
//! * the engine binds `127.0.0.1` on a free port, never another interface;
//! * it is started with a fresh random `OBSCURA_CDP_TOKEN` (32 random bytes,
//!   hex), and every connection presents it as a bearer token, exactly as the
//!   service does — another local user cannot drive it by guessing the port;
//! * its environment is cleared and only a short pass-through list is kept, so
//!   a token in agentd's environment never reaches a process that runs
//!   untrusted page JavaScript;
//! * `--allow-private-network` is passed only when the egress policy allows
//!   private hosts, which is the engine-side half of the same policy the
//!   service image ships with (it never sets the flag);
//! * no `--storage-dir`: the engine's cookies and storage live in memory, and
//!   each page is a throwaway browser context on top of that.
//!
//! The process is killed when the endpoint is dropped (`kill_on_drop`), and a
//! process that exited is started again on the next page.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;

use crate::cdp::RequestHeaders;
use crate::config::{LocalConfig, PINNED_OBSCURA_VERSION, ProviderKind};
use crate::error::BridgeError;
use crate::obscura::{EngineEndpoint, bearer};
use crate::secret::SecretValue;

/// The only interface the local engine binds.
pub const BIND_HOST: &str = "127.0.0.1";

/// The environment variables the engine keeps. Everything else, including any
/// `LOAMS_*` credential, is cleared.
const PASS_THROUGH_ENV: [&str; 9] = [
    "PATH",
    "HOME",
    "TMPDIR",
    "TEMP",
    "TMP",
    "SYSTEMROOT",
    "LANG",
    "TZ",
    "RUST_LOG",
];

/// The command line for one engine process, kept apart from the spawn so it
/// can be checked without starting anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    program: PathBuf,
    args: Vec<String>,
    port: u16,
}

impl LaunchSpec {
    /// The invocation for `program` on `port`.
    pub fn new(program: impl Into<PathBuf>, port: u16, allow_private_network: bool) -> Self {
        let mut args = vec![
            "serve".to_string(),
            "--port".to_string(),
            port.to_string(),
            "--host".to_string(),
            BIND_HOST.to_string(),
            "--workers".to_string(),
            "1".to_string(),
            "--quiet".to_string(),
        ];
        if allow_private_network {
            args.push("--allow-private-network".to_string());
        }
        Self {
            program: program.into(),
            args,
            port,
        }
    }

    /// The binary.
    pub fn program(&self) -> &Path {
        &self.program
    }

    /// The arguments, without the program.
    pub fn args(&self) -> &[String] {
        &self.args
    }

    /// The port.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The DevTools URL of this invocation.
    pub fn devtools_url(&self) -> String {
        format!("ws://{BIND_HOST}:{}/devtools/browser", self.port)
    }
}

#[derive(Debug)]
struct Running {
    child: tokio::process::Child,
    url: String,
    token: SecretValue,
}

/// An Obscura process this endpoint starts and owns.
#[derive(Debug)]
pub struct LocalEngine {
    program: PathBuf,
    startup_timeout: Duration,
    allow_private_network: bool,
    running: tokio::sync::Mutex<Option<Running>>,
}

impl LocalEngine {
    /// An endpoint that will start `config`'s binary on first use.
    pub fn new(config: &LocalConfig, allow_private_network: bool) -> Self {
        Self {
            program: config.program(),
            startup_timeout: config.startup_timeout(),
            allow_private_network,
            running: tokio::sync::Mutex::new(None),
        }
    }

    /// Stop the engine, if it runs.
    pub async fn stop(&self) {
        if let Some(mut running) = self.running.lock().await.take() {
            let _ = running.child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(5), running.child.wait()).await;
        }
    }

    async fn start(&self) -> Result<Running, BridgeError> {
        let port = free_port()?;
        let spec = LaunchSpec::new(&self.program, port, self.allow_private_network);
        let token = SecretValue::new(random_token());
        crate::redact::register(token.expose());

        let mut command = tokio::process::Command::new(spec.program());
        command
            .args(spec.args())
            .env_clear()
            .env("OBSCURA_CDP_TOKEN", token.expose())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        for name in PASS_THROUGH_ENV {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        let mut child = command.spawn().map_err(|error| {
            BridgeError::unavailable(format!(
                "starting {}: {error}. Install the Obscura {PINNED_OBSCURA_VERSION} release \
                 (the unsuffixed asset, not -no-render) and set LOAMS_OBSCURA_BIN, or select \
                 the service provider (LOAMS_BROWSER_PROVIDER=service)",
                spec.program().display()
            ))
        })?;
        if let Err(error) = wait_for_port(port, self.startup_timeout).await {
            let _ = child.start_kill();
            return Err(BridgeError::unavailable(format!(
                "{} did not accept a connection on {BIND_HOST}:{port} within {} ms: {error}",
                spec.program().display(),
                self.startup_timeout.as_millis()
            )));
        }
        tracing::info!(port, "started a local Obscura engine");
        Ok(Running {
            child,
            url: spec.devtools_url(),
            token,
        })
    }
}

#[async_trait]
impl EngineEndpoint for LocalEngine {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Local
    }

    async fn handshake(&self) -> Result<(String, RequestHeaders), BridgeError> {
        let mut running = self.running.lock().await;
        let alive = match running.as_mut() {
            Some(engine) => matches!(engine.child.try_wait(), Ok(None)),
            None => false,
        };
        if !alive {
            *running = Some(self.start().await?);
        }
        let engine = running
            .as_ref()
            .ok_or_else(|| BridgeError::unavailable("the local engine is not running"))?;
        Ok((engine.url.clone(), bearer(&engine.token)))
    }
}

/// A port nobody is listening on right now. The engine binds it a moment
/// later; losing that race shows up as a startup timeout, not a wrong engine,
/// because the token is fresh.
fn free_port() -> Result<u16, BridgeError> {
    let listener = std::net::TcpListener::bind((BIND_HOST, 0))?;
    Ok(listener.local_addr()?.port())
}

/// 32 random bytes as hex: the engine requires at least 32 bytes.
fn random_token() -> String {
    let bytes: [u8; 32] = rand::random();
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

async fn wait_for_port(port: u16, timeout: Duration) -> std::io::Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut last: Option<std::io::Error> = None;
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(last.unwrap_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out")
            }));
        }
        match tokio::net::TcpStream::connect((BIND_HOST, port)).await {
            Ok(_) => return Ok(()),
            Err(error) => last = Some(error),
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_engine_binds_loopback_and_relaxes_the_network_only_when_asked() {
        let strict = LaunchSpec::new("obscura", 9333, false);
        assert_eq!(
            strict.args(),
            [
                "serve",
                "--port",
                "9333",
                "--host",
                "127.0.0.1",
                "--workers",
                "1",
                "--quiet"
            ]
        );
        assert_eq!(
            strict.devtools_url(),
            "ws://127.0.0.1:9333/devtools/browser"
        );
        let relaxed = LaunchSpec::new("obscura", 9333, true);
        assert!(
            relaxed
                .args()
                .iter()
                .any(|arg| arg == "--allow-private-network")
        );
        assert!(
            !relaxed
                .args()
                .iter()
                .any(|arg| arg == "--allow-file-access" || arg == "--storage-dir"),
            "no file access and no persistent store"
        );
    }

    #[test]
    fn a_token_is_long_enough_for_the_engine_and_never_repeats() {
        let first = random_token();
        assert_eq!(first.len(), 64);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(first, random_token());
    }

    #[tokio::test]
    async fn a_missing_binary_names_the_fix() {
        let config = LocalConfig {
            obscura_bin: Some(PathBuf::from("/nonexistent/obscura-for-a-test")),
            ..LocalConfig::default()
        };
        let engine = LocalEngine::new(&config, false);
        let error = engine
            .handshake()
            .await
            .err()
            .unwrap_or_else(|| panic!("a missing binary cannot start"));
        let message = error.to_string();
        assert!(message.contains("LOAMS_OBSCURA_BIN"), "{message}");
        assert!(
            message.contains("LOAMS_BROWSER_PROVIDER=service"),
            "{message}"
        );
    }
}
