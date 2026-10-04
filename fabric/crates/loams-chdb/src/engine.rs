//! The engine: one chDB connection set for the whole process, which is what
//! `libchdb.so` actually offers.
//!
//! libchdb is not a server Loams can ask for two engines in: one process holds
//! one engine, one storage path, one set of server settings. `chdb_connect`
//! returns a pointer to a connection, and `chdb_close_conn` shuts the engine
//! down when it closes the last one — with the caveat the pinned header spells
//! out, that repeatedly closing the last connection and reconnecting puts the
//! engine through a full shutdown and boot each time, which is slow and is
//! "known to corrupt the process allocator on macOS". So [`Engine`] keeps one
//! connection of its own for the life of the process (it lives in a `OnceLock`
//! and is never dropped) and every [`Session`](crate::Session) borrows its own
//! connection alongside it.
//!
//! [`Engine::start`] is process-global. The first call boots; a later call with
//! the same configuration returns the same engine, and one with a different
//! configuration is refused rather than silently ignored — a second engine is not
//! something this library can give, so the answer has to be an error the caller
//! can see.

use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use loams_chdb_sys::ffi;

use crate::error::ChdbError;
use crate::session::Session;

/// The filesystem cache size Ruling 2 asks for when it does not say otherwise:
/// 20 GiB.
pub const DEFAULT_CACHE_BYTES: u64 = 20 * 1024 * 1024 * 1024;

/// The memory ceiling a server gets when [`EngineConfig`] does not say otherwise.
pub const DEFAULT_MAX_SERVER_MEMORY: u64 = 6 * 1024 * 1024 * 1024;

/// How the engine is set up. [`EngineConfig::default`] is what the container
/// gets from Ruling 2: storage and cache under the system temporary directory,
/// a 20 GiB cache, and no signal handlers, because a library must not take SIGINT
/// away from the process that loaded it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineConfig {
    /// Where chDB keeps its data — the `--path` of the one engine in the
    /// process. Every session in the process must use this same path: the pinned
    /// header says that connecting with a different path requires closing all
    /// existing connections first, which shuts the engine down, and Task 1
    /// measured that the second connect then simply fails.
    pub tmp_dir: PathBuf,
    /// Where the filesystem cache lives (`--filesystem_cache_path`).
    pub cache_dir: PathBuf,
    /// The cache's size limit in bytes (`--filesystem_cache_size_limit`).
    pub cache_bytes: u64,
    /// The engine's memory ceiling in bytes (`--max_server_memory_usage`).
    pub max_server_memory: u64,
    /// Whether chDB installs its signal handlers.
    ///
    /// False by default, and the header says why: "for safe integration into
    /// applications". A server that installs them would swallow SIGINT meant for
    /// its own supervisor, so [`Engine::start`] calls
    /// `chdb_set_signal_handlers_enabled(0)` before it connects.
    pub install_signal_handlers: bool,
}

impl Default for EngineConfig {
    fn default() -> Self {
        let tmp_dir = std::env::temp_dir().join("loams-chdb");
        Self {
            cache_dir: tmp_dir.join("cache"),
            tmp_dir,
            cache_bytes: DEFAULT_CACHE_BYTES,
            max_server_memory: DEFAULT_MAX_SERVER_MEMORY,
            install_signal_handlers: false,
        }
    }
}

impl EngineConfig {
    /// The connection arguments this configuration means.
    ///
    /// Only the connection that boots the engine may pass server-level options,
    /// which is what [`Engine`] does; a session adds query-level settings on top
    /// of these.
    pub fn to_args(&self) -> Vec<String> {
        vec![
            format!("--path={}", self.tmp_dir.display()),
            format!("--filesystem_cache_path={}", self.cache_dir.display()),
            format!("--filesystem_cache_size_limit={}", self.cache_bytes),
            format!("--max_server_memory_usage={}", self.max_server_memory),
        ]
    }
}

/// The engine: one chDB connection set for the process.
///
/// Built by [`Engine::start`], which is process-global.
pub struct Engine {
    config: EngineConfig,
    /// `SELECT version()` through the C ABI: the ClickHouse version, which is
    /// not the same string as `chdb_version()`. Measured at v26.9.0 of chDB,
    /// this is `26.9.2.1`.
    version: String,
    /// `chdb_version()`: the chDB release, `26.9.0`.
    chdb_version: String,
    /// The connection that keeps the engine alive for the life of the process.
    /// Never dropped: it lives in a `OnceLock`, and a last-connection close would
    /// shut the engine down.
    _keepalive: Arc<ffi::Connection>,
}

impl fmt::Debug for Engine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Engine")
            .field("config", &self.config)
            .field("version", &self.version)
            .field("chdb_version", &self.chdb_version)
            .finish_non_exhaustive()
    }
}

/// The engine, once [`Engine::start`] has built it. Never taken: the engine lives
/// for the life of the process.
static ENGINE: OnceLock<Engine> = OnceLock::new();
/// The configuration it was built with, so a second `start` can tell "same
/// engine" from "different engine".
static CONFIG: OnceLock<EngineConfig> = OnceLock::new();
/// Serialises the boot itself. `OnceLock::get_or_try_init` is unstable, and
/// `get_or_init` cannot report a failure.
static BOOT: Mutex<()> = Mutex::new(());

impl Engine {
    /// Starts the engine, or returns the one already running.
    ///
    /// The first call boots the engine: it creates the directories, installs or
    /// declines the signal handlers, connects, and reads the version through
    /// `SELECT version()`. A later call with an equal [`EngineConfig`] returns
    /// the same engine — libchdb has exactly one per process, and it cannot be
    /// reconfigured. A later call with a different one is refused with
    /// `ALREADY_STARTED`, naming both configurations, because quietly handing
    /// back an engine configured differently from the request is the one answer
    /// that cannot be debugged later.
    pub fn start(config: EngineConfig) -> Result<&'static Engine, ChdbError> {
        if let Some(started) = CONFIG.get() {
            return Self::joined(started, &config);
        }
        // A poisoned lock only means another `start` panicked; the engine may or
        // may not be there, and both cases are answered by re-reading CONFIG.
        let _guard: MutexGuard<()> = BOOT.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(started) = CONFIG.get() {
            return Self::joined(started, &config);
        }
        let engine = Engine::boot(config.clone())?;
        // Both `set`s can only lose to a racing `start`, and the lock above rules
        // that out; either way the values that are in are the ones that count.
        let _ = CONFIG.set(config);
        let _ = ENGINE.set(engine);
        ENGINE.get().ok_or_else(|| {
            ChdbError::loams(
                "ENGINE_MISSING",
                "the engine was built and then dropped, which should not be possible",
            )
        })
    }

    /// The answer for a `start` call that arrives after the engine is up.
    fn joined(started: &EngineConfig, asked: &EngineConfig) -> Result<&'static Engine, ChdbError> {
        if started != asked {
            return Err(ChdbError::loams(
                "ALREADY_STARTED",
                format!(
                    "libchdb runs one engine per process, and it was started with {started:?}; \
                     this call asked for {asked:?}"
                ),
            ));
        }
        ENGINE.get().ok_or_else(|| {
            ChdbError::loams(
                "ENGINE_MISSING",
                "the configuration is recorded but the engine is gone, which should not be possible",
            )
        })
    }

    /// Builds the engine and its keepalive connection.
    fn boot(config: EngineConfig) -> Result<Self, ChdbError> {
        for dir in [&config.tmp_dir, &config.cache_dir] {
            std::fs::create_dir_all(dir).map_err(|err| {
                ChdbError::loams(
                    "CANNOT_CREATE_DIRECTORY",
                    format!("{}: {err}", dir.display()),
                )
            })?;
        }
        // Before the first connection, as the header requires.
        ffi::set_signal_handlers_enabled(config.install_signal_handlers);

        let keepalive = Arc::new(ffi::Connection::open(&config.to_args()).map_err(engine_error)?);
        let version = keepalive
            .query("SELECT version()", "TabSeparated", &[])
            .map_err(engine_error)
            .map(|bytes| String::from_utf8_lossy(&bytes).trim_end().to_string())?;
        let chdb_version = ffi::chdb_version();

        Ok(Self {
            config,
            version,
            chdb_version,
            _keepalive: keepalive,
        })
    }

    /// The ClickHouse version, as `SELECT version()` answered at boot.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// The chDB release, as `chdb_version()` answered at boot.
    pub fn chdb_version(&self) -> &str {
        &self.chdb_version
    }

    /// The configuration the engine was started with.
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// Opens a session: its own chDB connection, its own settings, its own
    /// current database.
    ///
    /// The `id` is the House's `session_id`, which the HTTP layer will keep for
    /// `session_timeout` (Task 4); chDB has no session concept of its own, so
    /// the isolation a House session needs is the isolation of a connection.
    /// `settings` become connection arguments, which is where chDB takes
    /// query-level settings from.
    pub fn session(
        &self,
        id: crate::SessionId,
        settings: &crate::Settings,
    ) -> Result<Session, ChdbError> {
        Session::open(id, &self.config, settings)
    }

    /// [`Engine::version`] on a blocking thread.
    ///
    /// The version was read at boot, so this copies a string rather than calling
    /// into the library; it is here so that an async caller has one way to ask the
    /// engine what it is.
    pub async fn version_async(&self) -> Result<String, ChdbError> {
        let version = self.version.clone();
        tokio::task::spawn_blocking(move || Ok(version))
            .await
            .unwrap_or_else(|err| Err(ChdbError::loams("BLOCKING_TASK_FAILED", err.to_string())))
    }
}

/// Turns a library failure into a [`ChdbError`].
///
/// ClickHouse's own exceptions arrive as text and parse; anything else — a
/// refused connection, a null handle — is an engine message with no code.
pub(crate) fn engine_error(err: ffi::Error) -> ChdbError {
    ChdbError::parse(&err.message).unwrap_or_else(|| ChdbError::engine(err.message))
}

/// The settings a session applies before each of its statements.
///
/// The pinned header says query-level settings are applied at connect
/// (`chdb_connect(argc, argv) -- "Query-level settings are applied at connect
/// time"`), and that is what `Settings::to_args` builds. It cannot be how they
/// reach chDB here: Task 1 measured that `chdb_connect` **refuses** a second
/// connection whose arguments differ from the first one's, so a session's settings
/// would take the whole engine down with them. They are therefore applied as a
/// `SET` before each statement instead — see
/// [`crate::Session::execute`]. Task 4 adds the allowlist, the validation and the
/// caps.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Settings {
    pairs: Vec<(String, String)>,
}

impl Settings {
    /// No settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a setting.
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<String>) -> &mut Self {
        self.pairs.push((name.into(), value.into()));
        self
    }

    /// The settings, in the order they were set.
    pub fn as_slice(&self) -> &[(String, String)] {
        &self.pairs
    }

    /// The connection arguments for these settings, which is the form the header
    /// documents and the one `chdb_connect` refuses a differing one over.
    pub fn to_args(&self) -> Vec<String> {
        self.pairs
            .iter()
            .map(|(name, value)| format!("--{name}={value}"))
            .collect()
    }
}

impl FromIterator<(String, String)> for Settings {
    fn from_iter<T: IntoIterator<Item = (String, String)>>(iter: T) -> Self {
        Self {
            pairs: iter.into_iter().collect(),
        }
    }
}

/// A session id: the House's `session_id`, opaque to chDB.
///
/// A newtype rather than a `String` because it travels next to query ids and
/// settings, and mixing the two up would be silent.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SessionId(String);

impl SessionId {
    /// Wraps an id.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The id as it goes on the wire.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_CACHE_BYTES, EngineConfig, Settings};

    #[test]
    fn default_config_is_ruling_2s() {
        let config = EngineConfig::default();
        assert_eq!(config.cache_bytes, DEFAULT_CACHE_BYTES);
        assert!(
            !config.install_signal_handlers,
            "a library must not take SIGINT from the process that loaded it"
        );
        let args = config.to_args();
        assert!(args.iter().any(|arg| arg.starts_with("--path=")));
        assert!(
            args.iter()
                .any(|arg| arg.starts_with("--filesystem_cache_path=")),
            "Ruling 2 asks for a filesystem cache: {args:?}"
        );
        assert!(
            args.iter()
                .any(|arg| arg == "--max_server_memory_usage=6442450944")
        );
    }

    #[test]
    fn settings_become_connection_arguments() {
        let mut settings = Settings::new();
        settings
            .set("max_threads", "4")
            .set("max_block_size", "8192");
        assert_eq!(
            settings.to_args(),
            vec![
                "--max_threads=4".to_string(),
                "--max_block_size=8192".to_string()
            ]
        );
    }
}
