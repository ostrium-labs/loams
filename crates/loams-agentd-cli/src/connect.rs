//! Reaching the local agentd (design §50 §5.4, §7): the daemon directory, its
//! discovery file `agentd.json`, its token `agentd.token`, the loopback
//! WebSocket and `Hello`.
//!
//! The daemon directory is, in order: `--agentd-dir`; the directory of
//! `LOAMS_AGENTD_CONFIG` (the daemon's own override); `<LOAMS_DESKTOP_USER_DATA>/agentd`
//! (Loams Desktop's); else Loams Desktop's default `<userData>/agentd`:
//! `~/.config/Loams Desktop/agentd` on Linux (or under `XDG_CONFIG_HOME`),
//! `~/Library/Application Support/Loams Desktop/agentd` on macOS and
//! `%APPDATA%\Loams Desktop\agentd` on Windows.
//!
//! The discovery file is trusted only as far as `Hello` confirms it: a stale
//! file names a port where nothing answers with this token.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, anyhow, bail};
use loams_agentd_proto::daemon::Discovery;
use loams_agentd_proto::rpc::{AGENTD_PROTOCOL, ClientKind, Hello, HelloReply};
use loams_agentd_rpc::{RpcClient, connect_ws_with_token, methods};

/// The daemon's config override; its directory is the daemon directory.
pub const CONFIG_ENV: &str = "LOAMS_AGENTD_CONFIG";
/// Loams Desktop's `userData` override.
pub const USER_DATA_ENV: &str = "LOAMS_DESKTOP_USER_DATA";
/// Electron's `productName`, the `userData` directory's name.
pub const APP_DIR: &str = "Loams Desktop";

/// How long connecting and `Hello` may take together.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// The daemon directory, from `explicit` or the environment `env` reads.
/// `None` when no home directory is known either.
pub fn agentd_dir(
    explicit: Option<&Path>,
    env: impl Fn(&str) -> Option<OsString>,
) -> Option<PathBuf> {
    if let Some(dir) = explicit {
        return Some(dir.to_owned());
    }
    let set = |name: &str| env(name).filter(|value| !value.is_empty());
    if let Some(config) = set(CONFIG_ENV) {
        return PathBuf::from(config).parent().map(Path::to_path_buf);
    }
    if let Some(user_data) = set(USER_DATA_ENV) {
        return Some(PathBuf::from(user_data).join("agentd"));
    }
    app_data(&set).map(|dir| dir.join(APP_DIR).join("agentd"))
}

/// Electron's `appData` directory.
fn app_data(env: &impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if cfg!(windows) {
        return env("APPDATA").map(PathBuf::from);
    }
    let home = env("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        return home.map(|home| home.join("Library").join("Application Support"));
    }
    env("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| home.map(|home| home.join(".config")))
}

/// A connection to the daemon, after `Hello`.
#[derive(Debug)]
pub struct Agentd {
    pub client: RpcClient,
    pub hello: HelloReply,
}

impl Agentd {
    /// Whether the daemon advertises `feature`.
    pub fn has(&self, feature: &str) -> bool {
        self.hello.features.iter().any(|f| f == feature)
    }
}

/// Connect to the daemon whose directory is `dir`, with its owner token, and
/// say `Hello` as the CLI.
pub async fn connect(dir: &Path) -> anyhow::Result<Agentd> {
    let discovery_path = dir.join("agentd.json");
    let bytes = match std::fs::read(&discovery_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => bail!(
            "the Loams agent daemon is not running: there is no {}. Start Loams Desktop, or name the daemon's directory with --agentd-dir",
            discovery_path.display()
        ),
        Err(error) => {
            return Err(error).with_context(|| format!("read {}", discovery_path.display()));
        }
    };
    let discovery: Discovery = serde_json::from_slice(&bytes)
        .with_context(|| format!("{} is not a discovery file", discovery_path.display()))?;
    let token_path = dir.join("agentd.token");
    let token = std::fs::read_to_string(&token_path)
        .with_context(|| format!("read the daemon's token {}", token_path.display()))?;
    let url = format!("ws://127.0.0.1:{}", discovery.port);
    let dial = async {
        let client = connect_ws_with_token(&url, token.trim())
            .await
            .map_err(|error| {
                anyhow!(
                    "no agent daemon answers at {url} ({error}); {} may be stale: is Loams Desktop running?",
                    discovery_path.display()
                )
            })?;
        let hello = Hello {
            client: ClientKind::Cli,
            client_version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol: AGENTD_PROTOCOL,
        };
        let hello: HelloReply = client
            .call_as(methods::HELLO, serde_json::to_value(&hello)?)
            .await
            .map_err(|error| anyhow!("the agent daemon at {url} refused Hello: {error}"))?;
        Ok(Agentd { client, hello })
    };
    tokio::time::timeout(CONNECT_TIMEOUT, dial)
        .await
        .map_err(|_| anyhow!("the agent daemon at {url} did not answer within 5 s"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let pairs: Vec<(String, OsString)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), OsString::from(v)))
            .collect();
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        }
    }

    #[test]
    fn the_flag_wins() {
        let env = env_of(&[(CONFIG_ENV, "/c/agentd/config.toml")]);
        assert_eq!(
            agentd_dir(Some(Path::new("/x")), env),
            Some(PathBuf::from("/x"))
        );
    }

    #[test]
    fn the_config_override_names_its_directory() {
        let env = env_of(&[(CONFIG_ENV, "/c/agentd/config.toml"), (USER_DATA_ENV, "/u")]);
        assert_eq!(agentd_dir(None, env), Some(PathBuf::from("/c/agentd")));
    }

    #[test]
    fn the_user_data_override_is_next() {
        let env = env_of(&[(CONFIG_ENV, ""), (USER_DATA_ENV, "/u")]);
        assert_eq!(agentd_dir(None, env), Some(PathBuf::from("/u/agentd")));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_defaults_to_loams_desktops_user_data() {
        let env = env_of(&[("HOME", "/home/a")]);
        assert_eq!(
            agentd_dir(None, env),
            Some(PathBuf::from("/home/a/.config/Loams Desktop/agentd"))
        );
        let env = env_of(&[("HOME", "/home/a"), ("XDG_CONFIG_HOME", "/xdg")]);
        assert_eq!(
            agentd_dir(None, env),
            Some(PathBuf::from("/xdg/Loams Desktop/agentd"))
        );
        // A relative XDG_CONFIG_HOME is ignored, as the spec says.
        let env = env_of(&[("HOME", "/home/a"), ("XDG_CONFIG_HOME", "rel")]);
        assert_eq!(
            agentd_dir(None, env),
            Some(PathBuf::from("/home/a/.config/Loams Desktop/agentd"))
        );
        assert_eq!(agentd_dir(None, env_of(&[])), None);
    }

    #[tokio::test]
    async fn no_discovery_file_says_the_daemon_is_not_running() {
        let dir = tempfile::tempdir().expect("temp dir");
        let error = connect(dir.path()).await.expect_err("no daemon");
        assert!(error.to_string().contains("not running"), "{error}");
    }
}
