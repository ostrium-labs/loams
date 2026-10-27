//! Goose's launch: the pinned release archive and its private environment
//! (MS1 Task 3; D1809, D1453, D1454).
//!
//! agentd spawns `goose acp` (ACP over stdio) as its own process and links
//! none of goose's crates. Unlike the other ACP harnesses, which inherit
//! agentd's environment, a Goose child starts from an **empty** environment
//! and gets exactly [`environment`]'s list:
//!
//! - `PATH` (the managed build's directory, then system directories), `LANG`;
//! - `HOME`, `TMPDIR`: `<agentd data>/harnesses/goose/home`;
//! - `GOOSE_PATH_ROOT`: `<agentd data>/harnesses/goose/root`, where goose
//!   keeps its config, sessions and logs;
//! - `GOOSE_DISABLE_KEYRING`, `GOOSE_TELEMETRY_ENABLED=false`,
//!   `GOOSE_DISABLE_SESSION_NAMING`;
//! - `GOOSE_MODE=approve`: every tool call asks over
//!   `session/request_permission`, and agentd's approvals decide (a
//!   full-access thread auto-accepts, as for every ACP harness);
//! - `GOOSE_PROVIDER`/`GOOSE_MODEL` with the provider's host pointed at
//!   agentd's model proxy and a dummy key;
//! - `CONTEXT_FILE_NAMES`, goose's default, pinned.
//!
//! Only the managed build launches: a user's own goose would read their
//! configuration, so it only marks the harness installed in the picker. And
//! the harness is not selectable until the model proxy is configured.
//!
//! Goose's ACP server ignores a client `systemPrompt`
//! (`GOOSE_SYSTEM_PROMPT_FILE_PATH` is CLI-only), so a persona is written to
//! goose's global hints file under `GOOSE_PATH_ROOT` before launch.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use loams_agentd_proto::Model;

use crate::HarnessError;

/// The pinned goose release: the newest at least 14 days old on 2026-10-25.
pub(crate) const GOOSE_VERSION: &str = "1.54.0";
const GOOSE_ARCHIVE_NAME: &str = "goose";

/// Overrides `<agentd data>/harnesses/goose` (tests, custom deployments).
pub const GOOSE_DIR_ENV: &str = "LOAMS_GOOSE_DIR";
/// The model proxy's loopback base URL (`http://127.0.0.1:<port>`), set by
/// agentd once its model proxy serves requests (§73 §3.4).
pub const MODEL_PROXY_ENV: &str = "LOAMS_MODEL_PROXY_URL";

/// The dummy key the model proxy replaces; keep in step with
/// `loams_agentd_marketplace::contract::PROXY_DUMMY_KEY`.
const PROXY_DUMMY_KEY: &str = "loams-model-proxy";
/// The model Goose starts on when the run names none; the session's `model`
/// config option then switches to the picked one.
const DEFAULT_MODEL: &str = "gpt-5.4";
/// goose's default context files, set explicitly so a release that changes
/// the default does not change behaviour.
const CONTEXT_FILE_NAMES: &str = r#"[".goosehints","AGENTS.md"]"#;
/// The only extension goose loads besides the run's `mcpServers`. goose
/// adds every platform extension missing from `config.yaml` with its own
/// default (`migrate_platform_extensions`, 1.54.0), so all are listed and
/// all but `developer` are off.
const CONFIG_YAML: &str = "\
# Written by Loams agentd before every launch; edits are replaced.
extensions:
  developer: {enabled: true, type: platform, name: developer, display_name: Developer, description: Write and edit files and execute shell commands, bundled: true}
  analyze: {enabled: false, type: platform, name: analyze, description: '', bundled: true}
  todo: {enabled: false, type: platform, name: todo, description: '', bundled: true}
  apps: {enabled: false, type: platform, name: apps, description: '', bundled: true}
  chatrecall: {enabled: false, type: platform, name: chatrecall, description: '', bundled: true}
  extensionmanager: {enabled: false, type: platform, name: extensionmanager, description: '', bundled: true}
  scheduler: {enabled: false, type: platform, name: scheduler, description: '', bundled: true}
  summon: {enabled: false, type: platform, name: summon, description: '', bundled: true}
  summarize: {enabled: false, type: platform, name: summarize, description: '', bundled: true}
  code_execution: {enabled: false, type: platform, name: code_execution, description: '', bundled: true}
  orchestrator: {enabled: false, type: platform, name: orchestrator, description: '', bundled: true}
  tom: {enabled: false, type: platform, name: tom, description: '', bundled: true}
  skills: {enabled: false, type: platform, name: skills, description: '', bundled: true}
";

/// What a Goose launch is configured with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GooseEnvironment {
    /// `<agentd data>/harnesses/goose`. `None` resolves [`GOOSE_DIR_ENV`],
    /// else the directory beside agentd's adapters.
    pub dir: Option<PathBuf>,
    /// The model proxy's base URL. `None` resolves [`MODEL_PROXY_ENV`].
    pub model_proxy: Option<String>,
    /// Persona and instructions, written to goose's global hints file.
    pub instructions: Option<String>,
}

/// One launch's settings that come from the run rather than the harness.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct GooseLaunch<'a> {
    /// The run's model: seeds `GOOSE_MODEL` and picks the provider.
    pub model: Option<&'a str>,
}

/// Goose's private directories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GooseDirs {
    pub home: PathBuf,
    pub root: PathBuf,
}

impl GooseDirs {
    fn under(dir: &Path) -> Self {
        Self {
            home: dir.join("home"),
            root: dir.join("root"),
        }
    }

    fn tmp(&self) -> PathBuf {
        self.home.join("tmp")
    }

    /// `GOOSE_PATH_ROOT/config`, goose's config dir under a path root.
    fn config(&self) -> PathBuf {
        self.root.join("config")
    }

    /// goose's global hints file: `<config dir>/.goosehints`.
    pub(crate) fn hints(&self) -> PathBuf {
        self.config().join(".goosehints")
    }
}

impl GooseEnvironment {
    pub(crate) fn dirs(&self) -> Result<GooseDirs, HarnessError> {
        if let Some(dir) = &self.dir {
            return Ok(GooseDirs::under(dir));
        }
        if let Some(dir) = std::env::var_os(GOOSE_DIR_ENV).filter(|v| !v.is_empty()) {
            return Ok(GooseDirs::under(Path::new(&dir)));
        }
        crate::adapter_install::adapters_root()
            .and_then(|root| root.parent().map(Path::to_path_buf))
            .map(|data| GooseDirs::under(&data.join("harnesses").join("goose")))
            .ok_or_else(|| {
                HarnessError::Install(
                    "cannot locate agentd's data directory for Goose's private home".into(),
                )
            })
    }

    pub(crate) fn model_proxy(&self) -> Option<String> {
        self.model_proxy
            .clone()
            .or_else(|| std::env::var(MODEL_PROXY_ENV).ok())
            .map(|url| url.trim().trim_end_matches('/').to_owned())
            .filter(|url| !url.is_empty())
    }

    /// Goose has no provider but the model proxy, so it is selectable only
    /// once one is configured.
    pub(crate) fn model_proxy_configured(&self) -> bool {
        self.model_proxy().is_some()
    }
}

fn needs_proxy() -> HarnessError {
    HarnessError::NotInstalled(format!(
        "Goose needs agentd's model proxy ({MODEL_PROXY_ENV} is not set)"
    ))
}

/// Directories a Goose child may find programs in besides its own.
fn system_path_dirs(inherited: &dyn Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    if cfg!(windows) {
        let root = inherited("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        vec![
            root.join("System32"),
            root.clone(),
            root.join("System32").join("Wbem"),
            root.join("System32").join("WindowsPowerShell").join("v1.0"),
        ]
    } else {
        ["/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"]
            .into_iter()
            .map(PathBuf::from)
            .collect()
    }
}

/// The complete environment of a Goose child (the caller clears the
/// inherited one first). `inherited` is consulted for `LANG` and, on
/// Windows, the variables a process cannot start without.
pub(crate) fn environment(
    proxy: &str,
    launch: GooseLaunch<'_>,
    dirs: &GooseDirs,
    exe_dir: Option<&Path>,
    inherited: &dyn Fn(&str) -> Option<OsString>,
) -> Vec<(String, OsString)> {
    let mut env: Vec<(String, OsString)> = Vec::new();
    let mut set = |key: &str, value: OsString| env.push((key.to_owned(), value));

    let mut path: Vec<PathBuf> = exe_dir.map(Path::to_path_buf).into_iter().collect();
    path.extend(system_path_dirs(inherited));
    if let Ok(joined) = std::env::join_paths(path) {
        set("PATH", joined);
    }
    if let Some(lang) = inherited("LANG").filter(|v| !v.is_empty()) {
        set("LANG", lang);
    }
    set("HOME", dirs.home.clone().into_os_string());
    set("TMPDIR", dirs.tmp().into_os_string());
    if cfg!(windows) {
        // Ruling (MS1 Task 3): Windows processes need these to start and to
        // find a temp dir and profile; they carry no user configuration.
        for key in ["SystemRoot", "ComSpec", "PATHEXT"] {
            if let Some(value) = inherited(key).filter(|v| !v.is_empty()) {
                set(key, value);
            }
        }
        set("USERPROFILE", dirs.home.clone().into_os_string());
        set("TEMP", dirs.tmp().into_os_string());
        set("TMP", dirs.tmp().into_os_string());
    }
    set("GOOSE_PATH_ROOT", dirs.root.clone().into_os_string());
    set("GOOSE_DISABLE_KEYRING", "1".into());
    set("GOOSE_TELEMETRY_ENABLED", "false".into());
    // goose reads it as a bool: `1` would parse as a number and be ignored.
    set("GOOSE_DISABLE_SESSION_NAMING", "true".into());
    set("GOOSE_MODE", "approve".into());
    let model = launch.model.unwrap_or(DEFAULT_MODEL);
    if is_anthropic_shape(model) {
        // goose's Anthropic provider posts to `ANTHROPIC_HOST/v1/messages`;
        // the contract's `ANTHROPIC_BASE_URL` is `<proxy>/anthropic`.
        set("GOOSE_PROVIDER", "anthropic".into());
        set("ANTHROPIC_HOST", format!("{proxy}/anthropic").into());
        set("ANTHROPIC_API_KEY", PROXY_DUMMY_KEY.into());
    } else {
        // goose's OpenAI provider posts to `OPENAI_HOST/OPENAI_BASE_PATH`,
        // keeping the host's path prefix; the contract's `OPENAI_BASE_URL`
        // is `<proxy>/openai/v1`.
        set("GOOSE_PROVIDER", "openai".into());
        set("OPENAI_HOST", format!("{proxy}/openai").into());
        set("OPENAI_BASE_PATH", "v1/chat/completions".into());
        set("OPENAI_API_KEY", PROXY_DUMMY_KEY.into());
    }
    set("GOOSE_MODEL", model.into());
    set("CONTEXT_FILE_NAMES", CONTEXT_FILE_NAMES.into());
    env
}

/// The model's API shape (D1454): Anthropic Messages for Claude models,
/// Chat Completions otherwise.
fn is_anthropic_shape(model: &str) -> bool {
    let model = model.rsplit('/').next().unwrap_or(model);
    model.starts_with("claude")
}

/// Create the private dirs, write the developer-only `config.yaml`, and
/// write (or clear) the persona hints.
pub(crate) fn prepare(dirs: &GooseDirs, instructions: Option<&str>) -> std::io::Result<()> {
    for dir in [dirs.tmp(), dirs.config()] {
        std::fs::create_dir_all(dir)?;
    }
    write_atomic(&dirs.config().join("config.yaml"), CONFIG_YAML)?;
    let hints = dirs.hints();
    match instructions.map(str::trim).filter(|text| !text.is_empty()) {
        Some(text) => write_atomic(&hints, &format!("{text}\n")),
        None => match std::fs::remove_file(&hints) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        },
    }
}

fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// Clear the child's environment and apply Goose's.
pub(crate) fn configure(
    cmd: &mut crate::process::Command,
    settings: &GooseEnvironment,
    launch: GooseLaunch<'_>,
    executable: &Path,
) -> Result<(), HarnessError> {
    let proxy = settings.model_proxy().ok_or_else(needs_proxy)?;
    let dirs = settings.dirs()?;
    prepare(&dirs, settings.instructions.as_deref())?;
    let env = environment(
        &proxy,
        launch,
        &dirs,
        executable.parent().filter(|d| !d.as_os_str().is_empty()),
        &|key| std::env::var_os(key),
    );
    cmd.env_clear();
    for (key, value) in env {
        cmd.env(key, value);
    }
    Ok(())
}

/// goose's release builds: `(os, arch, asset, entry, sha512)`; `goose` at
/// the tarballs' root, `goose-package/goose.exe` in the Windows zip. Each
/// SHA-512 was taken from the downloaded asset after its SHA-256 matched the
/// digest GitHub publishes for it; both are in `tests/fixtures/goose-1.54.0-digests.txt`. Linux
/// pins the glibc build (ruling: a host without glibc sets
/// `GOOSE_EXECUTABLE` to goose's musl build).
pub(crate) const GOOSE_PINS: &[(&str, &str, &str, &str, &str)] = &[
    (
        "linux",
        "x86_64",
        "goose-x86_64-unknown-linux-gnu.tar.gz",
        "goose",
        "8b3f2e1aeae0933fd28a50a30c8bbaabe41a782f097e38b82a9da71544ec79483048c14d90c437898b22c8ae54d1a2fc7d42b239511ca227f514fd0c782f3005",
    ),
    (
        "linux",
        "aarch64",
        "goose-aarch64-unknown-linux-gnu.tar.gz",
        "goose",
        "01505234365b943ad25fea0e9fc5339bd78128e91d4844ea2f714717e02ead86b546d522838b98bfd821cbcf3028dd3a9fedb673d89eea6c2e1362b50bdb3d7c",
    ),
    (
        "macos",
        "aarch64",
        "goose-aarch64-apple-darwin.tar.gz",
        "goose",
        "56f83d4166bf1ed5ae906a9beccbd935a26834e08cd640e88e3ddaf6e92c55d442ebf47de930f7a84747fccec32a69023b38f702ecb8e4de9ebe50557f7f0f69",
    ),
    (
        "macos",
        "x86_64",
        "goose-x86_64-apple-darwin.tar.gz",
        "goose",
        "358898d3c9846f3830f5353dc29c9a4157e52da8e27cb92765c83fee7a7304a937b7c30fc59773a6fc2dd6e8b8ec3baa45ca525c8b5b71b4f55a1ba0d09acd3e",
    ),
    (
        "windows",
        "x86_64",
        "goose-x86_64-pc-windows-msvc.zip",
        "goose-package/goose.exe",
        "662870720d34b20b22fc43c4a90182b47ca8c66aac2d117ab3188b7362c75b0743a3b2794493091261e31431040847307d4919381bfd5670356291277f75727a",
    ),
];

const RELEASE_BASE: &str = "https://github.com/aaif-goose/goose/releases/download/v1.54.0/";

fn release_url(asset: &str) -> String {
    format!("{RELEASE_BASE}{asset}")
}

/// The pin for this platform, `None` where goose ships no build.
pub(crate) fn goose_archive() -> Option<crate::archive_install::ArchivePin> {
    let (_, _, asset, entry, sha512) = GOOSE_PINS
        .iter()
        .find(|(os, arch, ..)| *os == std::env::consts::OS && *arch == std::env::consts::ARCH)?;
    // `ArchivePin` holds `&'static str`s: the url is built once per process.
    static URL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let url = URL.get_or_init(|| release_url(asset));
    Some(crate::archive_install::ArchivePin {
        name: GOOSE_ARCHIVE_NAME,
        version: GOOSE_VERSION,
        url,
        entry,
        sha512,
    })
}

/// Where a user's own goose lives: it marks the harness installed, never
/// launches.
pub(crate) fn user_install_paths() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = crate::executable::home_dir() {
        // `download_cli.sh`'s default GOOSE_BIN_DIR.
        dirs.push(home.join(".local").join("bin").join("goose"));
    }
    dirs.push(PathBuf::from("/opt/homebrew/bin/goose"));
    dirs.push(PathBuf::from("/usr/local/bin/goose"));
    dirs
}

pub(crate) fn fallback_models() -> Vec<Model> {
    vec![Model {
        id: DEFAULT_MODEL.into(),
        label: "GPT-5.4".into(),
        description: Some("Through agentd's model proxy".into()),
        reasoning_levels: Vec::new(),
        options: Vec::new(),
    }]
}

pub(crate) fn can_install() -> bool {
    goose_archive().is_some()
}

pub(crate) async fn install() -> Result<(), HarnessError> {
    let pin = goose_archive().ok_or_else(|| {
        HarnessError::NotInstalled(
            "no pinned Goose build for this platform; set GOOSE_EXECUTABLE".into(),
        )
    })?;
    crate::archive_install::ensure_installed(pin, "Goose").await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn dirs() -> GooseDirs {
        GooseDirs::under(Path::new("/data/harnesses/goose"))
    }

    fn env_map(model: Option<&str>, inherited: &[(&str, &str)]) -> BTreeMap<String, String> {
        let inherited: BTreeMap<String, OsString> = inherited
            .iter()
            .map(|(k, v)| ((*k).to_owned(), OsString::from(v)))
            .collect();
        environment(
            "http://127.0.0.1:7801",
            GooseLaunch { model },
            &dirs(),
            Some(Path::new("/data/adapters/goose/1.54.0")),
            &|key| inherited.get(key).cloned(),
        )
        .into_iter()
        .map(|(k, v)| (k, v.to_string_lossy().into_owned()))
        .collect()
    }

    #[test]
    fn goose_env_is_private() {
        let env = env_map(
            Some("gpt-5.5"),
            &[
                ("PATH", "/home/user/.cargo/bin:/usr/bin"),
                ("LANG", "C.UTF-8"),
                ("HOME", "/home/user"),
                ("XDG_CONFIG_HOME", "/home/user/.config"),
                ("SSH_AUTH_SOCK", "/tmp/ssh.sock"),
                ("OPENAI_API_KEY", "sk-user"),
                ("ANTHROPIC_API_KEY", "sk-ant-user"),
                ("GOOSE_PROVIDER", "databricks"),
                ("AWS_SECRET_ACCESS_KEY", "leak"),
                ("SystemRoot", r"C:\Windows"),
            ],
        );
        let mut expected: Vec<&str> = vec![
            "PATH",
            "LANG",
            "HOME",
            "TMPDIR",
            "GOOSE_PATH_ROOT",
            "GOOSE_DISABLE_KEYRING",
            "GOOSE_TELEMETRY_ENABLED",
            "GOOSE_DISABLE_SESSION_NAMING",
            "GOOSE_MODE",
            "GOOSE_PROVIDER",
            "OPENAI_HOST",
            "OPENAI_BASE_PATH",
            "OPENAI_API_KEY",
            "GOOSE_MODEL",
            "CONTEXT_FILE_NAMES",
        ];
        if cfg!(windows) {
            expected.extend(["SystemRoot", "USERPROFILE", "TEMP", "TMP"]);
        }
        let mut keys: Vec<&str> = env.keys().map(String::as_str).collect();
        keys.sort_unstable();
        expected.sort_unstable();
        assert_eq!(keys, expected);
        assert_eq!(env["HOME"], "/data/harnesses/goose/home");
        assert_eq!(env["TMPDIR"], "/data/harnesses/goose/home/tmp");
        assert_eq!(env["GOOSE_PATH_ROOT"], "/data/harnesses/goose/root");
        assert_eq!(env["GOOSE_DISABLE_KEYRING"], "1");
        assert_eq!(env["GOOSE_TELEMETRY_ENABLED"], "false");
        assert_eq!(env["GOOSE_MODE"], "approve");
        assert_eq!(env["LANG"], "C.UTF-8");
        assert_eq!(env["CONTEXT_FILE_NAMES"], r#"[".goosehints","AGENTS.md"]"#);
        // The user's key never reaches goose; the proxy's dummy one does.
        assert_eq!(env["OPENAI_API_KEY"], PROXY_DUMMY_KEY);
        assert!(!env["PATH"].contains(".cargo"), "{}", env["PATH"]);
        if cfg!(unix) {
            assert!(env["PATH"].starts_with("/data/adapters/goose/1.54.0:"));
            assert!(env["PATH"].ends_with("/usr/bin:/bin:/usr/sbin:/sbin"));
        }
    }

    #[test]
    fn goose_env_points_provider_at_model_proxy() {
        let env = env_map(Some("gpt-5.5"), &[]);
        assert_eq!(env["GOOSE_PROVIDER"], "openai");
        assert_eq!(env["GOOSE_MODEL"], "gpt-5.5");
        assert_eq!(env["OPENAI_HOST"], "http://127.0.0.1:7801/openai");
        assert_eq!(env["OPENAI_BASE_PATH"], "v1/chat/completions");

        let env = env_map(Some("claude-sonnet-5"), &[]);
        assert_eq!(env["GOOSE_PROVIDER"], "anthropic");
        assert_eq!(env["GOOSE_MODEL"], "claude-sonnet-5");
        assert_eq!(env["ANTHROPIC_HOST"], "http://127.0.0.1:7801/anthropic");
        assert_eq!(env["ANTHROPIC_API_KEY"], PROXY_DUMMY_KEY);
        assert!(!env.contains_key("OPENAI_HOST"));

        let env = env_map(None, &[]);
        assert_eq!(env["GOOSE_MODEL"], DEFAULT_MODEL);
    }

    #[test]
    fn model_proxy_url_is_normalized_and_gates_selection() {
        let mut settings = GooseEnvironment {
            model_proxy: Some(" http://127.0.0.1:7801/ ".into()),
            ..Default::default()
        };
        assert_eq!(
            settings.model_proxy().as_deref(),
            Some("http://127.0.0.1:7801")
        );
        assert!(settings.model_proxy_configured());
        settings.model_proxy = Some("  ".into());
        // An explicit blank falls through to the variable, which tests never set.
        if std::env::var_os(MODEL_PROXY_ENV).is_none() {
            assert!(!settings.model_proxy_configured());
        }
    }

    #[test]
    fn goose_persona_written_to_hints_file() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = GooseDirs::under(dir.path());
        prepare(&dirs, Some("You are the Loams release agent.")).unwrap();
        assert_eq!(
            std::fs::read_to_string(dirs.hints()).unwrap(),
            "You are the Loams release agent.\n"
        );
        assert_eq!(dirs.hints(), dir.path().join("root/config/.goosehints"));
        let config = std::fs::read_to_string(dir.path().join("root/config/config.yaml")).unwrap();
        assert!(config.contains("developer:"), "{config}");
        assert!(dir.path().join("home/tmp").is_dir());
        prepare(&dirs, None).unwrap();
        assert!(!dirs.hints().exists());
        prepare(&dirs, Some("   ")).unwrap();
        assert!(!dirs.hints().exists());
    }

    #[test]
    fn goose_archive_pins_match_release_digests() {
        // `<asset> <sha256> <sha512>` per line, from the release download.
        let fixture = include_str!("../../tests/fixtures/goose-1.54.0-digests.txt");
        let digests: BTreeMap<&str, (&str, &str)> = fixture
            .lines()
            .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
            .map(|line| {
                let mut parts = line.split_whitespace();
                let asset = parts.next().unwrap();
                (asset, (parts.next().unwrap(), parts.next().unwrap()))
            })
            .collect();
        assert_eq!(digests.len(), GOOSE_PINS.len());
        for (os, arch, asset, entry, sha512) in GOOSE_PINS {
            let (sha256, pinned) = digests[asset];
            assert_eq!(sha256.len(), 64, "{asset}");
            assert_eq!(*sha512, pinned, "{os}/{arch}: {asset}");
            assert_eq!(sha512.len(), 128);
            assert!(sha512.bytes().all(|b| b.is_ascii_hexdigit()));
            assert_eq!(
                *entry,
                if *os == "windows" {
                    "goose-package/goose.exe"
                } else {
                    "goose"
                }
            );
            assert!(asset.contains(arch));
        }
        if let Some(pin) = goose_archive() {
            assert_eq!(pin.name, "goose");
            assert_eq!(pin.version, GOOSE_VERSION);
            assert!(pin.url.starts_with(
                "https://github.com/aaif-goose/goose/releases/download/v1.54.0/goose-"
            ));
            assert!(pin.url.contains(GOOSE_VERSION));
        }
    }

    #[test]
    fn anthropic_shape_follows_the_model_family() {
        assert!(is_anthropic_shape("claude-sonnet-5"));
        assert!(is_anthropic_shape("anthropic/claude-opus-5"));
        assert!(!is_anthropic_shape("gpt-5.5"));
        assert!(!is_anthropic_shape("openrouter/qwen3"));
    }
}
