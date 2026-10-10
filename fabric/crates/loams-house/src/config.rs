//! The House front's configuration (HS1 Task 3), and the TOML file
//! `loams-fabric house --config` reads it from (HS1 Task 7): [`HouseFile`], with
//! the tables `[house]`, `[house.limits]`, `[house.pool]`, `[house.catalog]`,
//! `[house.store]` and `[house.tls]`.

use std::fmt;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::admission::PoolConfig;

use crate::errors::{ChError, HouseError};

/// Where the HTTP interface listens by default (§49 Shared contracts).
pub const DEFAULT_LISTEN: &str = "127.0.0.1:8123";

/// Where the native protocol listens by default (§49 Shared contracts; served from
/// HS1 Task 31).
pub const DEFAULT_NATIVE_LISTEN: &str = "127.0.0.1:9000";

/// Where admin and metrics listen by default (§49 Shared contracts; served from
/// HS1 Task 23).
pub const DEFAULT_ADMIN_LISTEN: &str = "127.0.0.1:8125";

/// The ClickHouse version the House reports, which the pinned chdb-core v26.9.0
/// answers to `SELECT version()` (HS1 R1.1). It is rendered into every error.
/// `versions.rs` will own it with the reference server's digest (FL2 Global
/// Constraint); until then the worker's `Ready` is checked against it.
pub const CLICKHOUSE_VERSION: &str = "26.9.2.1";

/// `X-ClickHouse-Server-Display-Name` (FL2 Task 2).
pub const DISPLAY_NAME: &str = "loams-house";

/// The House front's configuration.
#[derive(Clone, Debug)]
pub struct HouseConfig {
    /// The HTTP listener: loopback only until TLS and the verifier (D111, HS1 Task 20).
    pub listen: SocketAddr,
    /// The development users (FL2 Task 2's `UserMap`); Task 20 replaces them. None
    /// by default (Task 3 review M9): a House with no users serves no queries.
    pub users: Vec<UserMap>,
    /// Where `wait_end_of_query = 1` spools results past [`SPOOL_IN_MEMORY`]: a data
    /// directory on disk, never the RAM-backed `/tmp` by default (review I2).
    pub tmp_dir: PathBuf,
    /// The most one `wait_end_of_query = 1` response may spool: 1 GiB.
    pub wait_end_of_query_max_bytes: u64,
    /// The most all spools together may hold at once (review I2): 8 GiB.
    pub spool_budget_bytes: u64,
    /// How much output is held before the response head goes out, unless
    /// `buffer_size` says otherwise (clamped to [`BUFFER_SIZE_RANGE`]): ClickHouse's
    /// 1 MiB. A statement that fails inside it still gets a proper error status.
    pub default_buffer_size: usize,
    /// The most bytes of a POST body read as statement text: ClickHouse's
    /// `max_query_size` default, 256 KiB. Data after an `INSERT … FORMAT` line is
    /// streamed, not counted.
    pub max_query_size: usize,
    /// How long a client may go silent while it sends a request body (per frame):
    /// ClickHouse's `http_receive_timeout`, 30 s. It never bounds a statement.
    pub receive_timeout: Duration,
    /// How long a write to the client may stall: ClickHouse's `http_send_timeout`.
    pub send_timeout: Duration,
    /// Keep-alive between requests: advertised as `Keep-Alive: timeout=…` and
    /// enforced (with the time a request head may take) by hyper's header timer, so
    /// the two never differ (fix round 2, N4); zero turns keep-alive off.
    pub keep_alive: Duration,
    /// Connections served at once; more wait in the listen backlog (review I1).
    pub max_connections: usize,
    /// Request header fields at most; more answer `431` (review M5).
    pub max_headers: usize,
    /// The largest request head hyper buffers: 128 KiB (fix round 2, N6), room for
    /// hyper's 65 534-byte URI limit and the headers.
    pub max_head_bytes: usize,
    /// The version rendered into errors.
    pub version: String,
    /// How far a compressed request body may expand (review I3).
    pub body_limits: crate::compress::BodyLimits,
    /// The caps on settings (FL2 Ruling 10; HS1 Task 4).
    pub session_limits: crate::settings::SessionLimits,
    /// Sessions live at once (FL2 Ruling 10).
    pub max_live_sessions: usize,
}

/// The bytes a `wait_end_of_query` spool keeps in memory before it moves to a file
/// (review I2: a constant, not the client's `buffer_size`).
pub const SPOOL_IN_MEMORY: usize = 1024 * 1024;

/// `buffer_size` is clamped to this (review I2).
pub const BUFFER_SIZE_RANGE: std::ops::RangeInclusive<usize> = 1..=16 * 1024 * 1024;

impl Default for HouseConfig {
    fn default() -> Self {
        Self {
            listen: DEFAULT_LISTEN
                .parse()
                .unwrap_or_else(|_| SocketAddr::from(([127, 0, 0, 1], 8123))),
            users: Vec::new(),
            tmp_dir: PathBuf::from("loams-house-data").join("spool"),
            wait_end_of_query_max_bytes: 1024 * 1024 * 1024,
            spool_budget_bytes: 8 * 1024 * 1024 * 1024,
            default_buffer_size: 1024 * 1024,
            max_query_size: 256 * 1024,
            receive_timeout: Duration::from_secs(30),
            send_timeout: Duration::from_secs(30),
            keep_alive: Duration::from_secs(10),
            max_connections: 1024,
            max_headers: 100,
            max_head_bytes: 128 * 1024,
            version: CLICKHOUSE_VERSION.to_string(),
            body_limits: crate::compress::BodyLimits::default(),
            session_limits: crate::settings::SessionLimits::default(),
            max_live_sessions: crate::session::MAX_LIVE_SESSIONS,
        }
    }
}

/// Refuses a listener that is not loopback, with FL2's message (D111). Plaintext
/// listeners stay on loopback until HS1 Task 20 brings TLS and the verifier.
pub fn check_listen(addr: SocketAddr) -> Result<(), HouseError> {
    if addr.ip().is_loopback() {
        return Ok(());
    }
    Err(HouseError::from(ChError::bad_arguments(format!(
        "house listen on {addr}: only loopback addresses are served until the unified auth \
         plan (D111)"
    ))))
}

/// A development user (FL2 Task 2): a name, the SHA-256 of its password, the
/// namespace its queries run in, and whether it may only read. Task 20 replaces
/// these with Loams identities.
#[derive(Clone, PartialEq, Eq)]
pub struct UserMap {
    /// The ClickHouse user name.
    pub user: String,
    /// Lower-case hex SHA-256 of the password.
    pub password_sha256: String,
    /// The namespace the user's queries are bound to.
    pub namespace: u64,
    /// Whether the user may only read (as GET does for everyone).
    pub readonly: bool,
}

impl UserMap {
    /// A user from a clear-text password, hashed here.
    pub fn dev(user: &str, password: &str, namespace: u64, readonly: bool) -> Self {
        Self {
            user: user.to_string(),
            password_sha256: sha256_hex(password),
            namespace,
            readonly,
        }
    }

    /// Whether `password` is this user's, compared in constant time over the digest.
    pub fn verifies(&self, password: &str) -> bool {
        let given = sha256_hex(password);
        let ours = self.password_sha256.to_ascii_lowercase();
        given.len() == ours.len()
            && given
                .bytes()
                .zip(ours.bytes())
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                == 0
    }
}

impl fmt::Debug for UserMap {
    /// The digest is a credential: never printed (HS1 Global Constraint).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UserMap")
            .field("user", &self.user)
            .field("password_sha256", &"[redacted]")
            .field("namespace", &self.namespace)
            .field("readonly", &self.readonly)
            .finish()
    }
}

fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Where workers run (`--workers=process|inproc`, §49 §4.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WorkersMode {
    /// `loams-house-worker` processes, sealed (the default; D761).
    #[default]
    Process,
    /// The worker's serve loop on threads of the front (feature `inproc-worker`):
    /// no isolation, single-node development and the desktop only (§49 §17).
    Inproc,
}

impl WorkersMode {
    /// The flag's value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Process => "process",
            Self::Inproc => "inproc",
        }
    }
}

impl std::str::FromStr for WorkersMode {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "process" => Ok(Self::Process),
            "inproc" => Ok(Self::Inproc),
            other => Err(format!("--workers {other:?}: expected process or inproc")),
        }
    }
}

/// The catalog of record (`--catalog=rest:<url>|local:<path>`; HS1 Task 10 uses
/// it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogSpec {
    /// An Iceberg REST catalog (Lakekeeper in production).
    Rest(String),
    /// The local SQLite catalog of `--single-node` (§49 §17).
    Local(PathBuf),
}

impl std::str::FromStr for CatalogSpec {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if let Some(url) = text.strip_prefix("rest:")
            && (url.starts_with("http://") || url.starts_with("https://"))
        {
            return Ok(Self::Rest(url.to_string()));
        }
        if let Some(path) = text.strip_prefix("local:")
            && !path.is_empty()
        {
            return Ok(Self::Local(PathBuf::from(path)));
        }
        Err(format!(
            "--catalog {text:?}: expected rest:<http(s) url> or local:<path>"
        ))
    }
}

impl fmt::Display for CatalogSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rest(url) => write!(f, "rest:{url}"),
            Self::Local(path) => write!(f, "local:{}", path.display()),
        }
    }
}

/// The `loams-fabric house --config` file (HS1 Task 7). Every key is optional;
/// a flag beats the file, and the file beats `--single-node`'s defaults (§49 §17)
/// and the built-in ones. Unknown keys are refused, so a misspelt limit is an
/// error rather than a default.
///
/// ```toml
/// [house]
/// listen = "127.0.0.1:8123"
/// single_node = true
///
/// [house.pool]
/// min_idle_workers = 1
///
/// [house.limits]
/// max_memory_usage = 2147483648
/// ```
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HouseFile {
    /// `[house]`.
    #[serde(default)]
    pub house: HouseSection,
}

/// `[house]`: listeners, the mode, and the tables below.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HouseSection {
    /// The HTTP (and, from HS1 Task 8, Connect) listener; `--house-listen`.
    pub listen: Option<SocketAddr>,
    /// The TLS HTTP listener (8443); `--house-tls-listen`. HS1 Task 20.
    pub tls_listen: Option<SocketAddr>,
    /// The native protocol listener (9000); `--native-listen`. HS1 Task 31.
    pub native_listen: Option<SocketAddr>,
    /// The TLS native listener (9440); `--native-tls-listen`. HS1 Task 20.
    pub native_tls_listen: Option<SocketAddr>,
    /// Admin and metrics (8125); `--admin-listen`. HS1 Task 23.
    pub admin_listen: Option<SocketAddr>,
    /// `--single-node` (§49 §17).
    pub single_node: Option<bool>,
    /// `--workers`: `process` or `inproc`.
    pub workers: Option<String>,
    /// `--sandbox`: `netns`, `pods` or `none` (HS1 R6.8).
    pub sandbox: Option<String>,
    /// `--data-dir`: the spool, the workers' private directories and, with
    /// `--single-node`, the local catalog, bucket and key.
    pub data_dir: Option<PathBuf>,
    /// Development users (FL2 Task 2's `UserMap`) until HS1 Task 20.
    #[serde(default)]
    pub users: Vec<FileUser>,
    /// `[house.limits]`.
    #[serde(default)]
    pub limits: LimitsSection,
    /// `[house.pool]`.
    #[serde(default)]
    pub pool: PoolSection,
    /// `[house.catalog]`.
    #[serde(default)]
    pub catalog: UrlSection,
    /// `[house.store]`.
    #[serde(default)]
    pub store: UrlSection,
    /// `[house.tls]`: the certificate and key HS1 Task 20's listeners use.
    #[serde(default)]
    pub tls: TlsSection,
}

/// One `[[house.users]]` entry.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileUser {
    /// The ClickHouse user name.
    pub user: String,
    /// Lower-case hex SHA-256 of the password; the file never holds the password.
    pub password_sha256: String,
    /// The namespace the user's queries run in.
    pub namespace: u64,
    /// Whether the user may only read.
    #[serde(default)]
    pub readonly: bool,
}

impl fmt::Debug for FileUser {
    /// The digest is a credential: never printed (HS1 Global Constraint).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileUser")
            .field("user", &self.user)
            .field("password_sha256", &"[redacted]")
            .field("namespace", &self.namespace)
            .field("readonly", &self.readonly)
            .finish()
    }
}

impl From<&FileUser> for UserMap {
    fn from(user: &FileUser) -> Self {
        Self {
            user: user.user.clone(),
            password_sha256: user.password_sha256.to_ascii_lowercase(),
            namespace: user.namespace,
            readonly: user.readonly,
        }
    }
}

/// `[house.limits]`: the front's request and session caps ([`HouseConfig`]).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LimitsSection {
    /// [`HouseConfig::max_query_size`].
    pub max_query_size: Option<usize>,
    /// [`HouseConfig::max_connections`].
    pub max_connections: Option<usize>,
    /// [`HouseConfig::receive_timeout`], in milliseconds.
    pub receive_timeout_ms: Option<u64>,
    /// [`HouseConfig::send_timeout`], in milliseconds.
    pub send_timeout_ms: Option<u64>,
    /// [`HouseConfig::keep_alive`], in milliseconds.
    pub keep_alive_ms: Option<u64>,
    /// [`HouseConfig::wait_end_of_query_max_bytes`].
    pub wait_end_of_query_max_bytes: Option<u64>,
    /// [`HouseConfig::spool_budget_bytes`].
    pub spool_budget_bytes: Option<u64>,
    /// [`HouseConfig::max_live_sessions`].
    pub max_live_sessions: Option<usize>,
    /// The `max_memory_usage` cap (§49 §12).
    pub max_memory_usage: Option<u64>,
    /// The `max_execution_time` cap, in seconds (§49 §12).
    pub max_execution_time_s: Option<f64>,
    /// The `max_threads` cap (§49 §12).
    pub max_threads: Option<u64>,
}

impl LimitsSection {
    /// Writes every key present onto `config`.
    pub fn apply(&self, config: &mut HouseConfig) {
        let ms = Duration::from_millis;
        set(&mut config.max_query_size, self.max_query_size);
        set(&mut config.max_connections, self.max_connections);
        set(&mut config.receive_timeout, self.receive_timeout_ms.map(ms));
        set(&mut config.send_timeout, self.send_timeout_ms.map(ms));
        set(&mut config.keep_alive, self.keep_alive_ms.map(ms));
        set(
            &mut config.wait_end_of_query_max_bytes,
            self.wait_end_of_query_max_bytes,
        );
        set(&mut config.spool_budget_bytes, self.spool_budget_bytes);
        set(&mut config.max_live_sessions, self.max_live_sessions);
        let caps = &mut config.session_limits;
        set(&mut caps.max_memory_usage, self.max_memory_usage);
        set(&mut caps.max_execution_time_s, self.max_execution_time_s);
        set(&mut caps.max_threads, self.max_threads);
    }
}

/// `[house.pool]`: the worker pool (§49 §10.1) and how workers are started.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolSection {
    /// The `loams-house-worker` binary; `--worker-binary`.
    pub worker_binary: Option<PathBuf>,
    /// [`PoolConfig::min_idle_workers`].
    pub min_idle_workers: Option<usize>,
    /// [`PoolConfig::max_workers`].
    pub max_workers: Option<usize>,
    /// [`PoolConfig::max_workers_per_namespace`].
    pub max_workers_per_namespace: Option<usize>,
    /// [`PoolConfig::max_pinned_workers_per_namespace`].
    pub max_pinned_workers_per_namespace: Option<usize>,
    /// [`PoolConfig::max_queries_per_worker`].
    pub max_queries_per_worker: Option<u32>,
    /// [`PoolConfig::idle_unbind_after`], in milliseconds.
    pub idle_unbind_after_ms: Option<u64>,
    /// [`PoolConfig::worker_rss_ceiling`].
    pub worker_rss_ceiling: Option<u64>,
    /// [`PoolConfig::acquire_timeout`], in milliseconds.
    pub acquire_timeout_ms: Option<u64>,
    /// [`PoolConfig::boot_timeout`], in milliseconds.
    pub boot_timeout_ms: Option<u64>,
    /// The memory each worker sizes its engine and caches for, and its cgroup's
    /// `memory.max`.
    pub worker_memory_limit: Option<u64>,
    /// A delegated cgroup v2 directory the front owns; each worker gets a child
    /// (HS1 R6.5); `--cgroup-root`.
    pub cgroup_root: Option<PathBuf>,
}

impl PoolSection {
    /// Writes every pool key present onto `config`.
    pub fn apply(&self, config: &mut PoolConfig) {
        let ms = Duration::from_millis;
        set(&mut config.min_idle_workers, self.min_idle_workers);
        set(&mut config.max_workers, self.max_workers);
        set(
            &mut config.max_workers_per_namespace,
            self.max_workers_per_namespace,
        );
        set(
            &mut config.max_pinned_workers_per_namespace,
            self.max_pinned_workers_per_namespace,
        );
        set(
            &mut config.max_queries_per_worker,
            self.max_queries_per_worker,
        );
        set(
            &mut config.idle_unbind_after,
            self.idle_unbind_after_ms.map(ms),
        );
        set(&mut config.worker_rss_ceiling, self.worker_rss_ceiling);
        set(&mut config.acquire_timeout, self.acquire_timeout_ms.map(ms));
        set(&mut config.boot_timeout, self.boot_timeout_ms.map(ms));
    }
}

/// `[house.catalog]` and `[house.store]`: one `url` each (`--catalog`, `--store`).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UrlSection {
    /// The value of the matching flag.
    pub url: Option<String>,
}

/// `[house.tls]`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TlsSection {
    /// The certificate chain (PEM).
    pub cert: Option<PathBuf>,
    /// The private key (PEM).
    pub key: Option<PathBuf>,
}

impl HouseFile {
    /// Parses a configuration file's text.
    pub fn parse(text: &str) -> Result<Self, HouseError> {
        let file: Self = toml::from_str(text).map_err(|err| {
            HouseError::from(ChError::bad_arguments(format!(
                "the house configuration file: {err}"
            )))
        })?;
        // A malformed digest would load and then never match (PR #398 review).
        for user in &file.house.users {
            let digest = &user.password_sha256;
            if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(HouseError::from(ChError::bad_arguments(format!(
                    "the house configuration file: user {:?}: password_sha256 must be 64 hex \
                     digits (the SHA-256 of the password)",
                    user.user
                ))));
            }
        }
        Ok(file)
    }

    /// Reads and parses `path`.
    pub fn load(path: &Path) -> Result<Self, HouseError> {
        let text = std::fs::read_to_string(path).map_err(|err| {
            HouseError::from(ChError::bad_arguments(format!(
                "the house configuration file {}: {err}",
                path.display()
            )))
        })?;
        Self::parse(&text)
    }
}

fn set<T>(slot: &mut T, value: Option<T>) {
    if let Some(value) = value {
        *slot = value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_tables_apply_and_unknown_keys_are_refused() {
        let file = HouseFile::parse(
            r#"
            [house]
            listen = "127.0.0.1:18123"
            single_node = true
            workers = "process"
            sandbox = "netns"
            data_dir = "/srv/house"

            [[house.users]]
            user = "alice"
            password_sha256 = "ABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABAB"
            namespace = 7

            [house.limits]
            max_query_size = 1024
            keep_alive_ms = 2500
            max_memory_usage = 1073741824

            [house.pool]
            min_idle_workers = 1
            idle_unbind_after_ms = 5000
            worker_binary = "/opt/loams/loams-house-worker"

            [house.catalog]
            url = "local:/srv/house/catalog.sqlite"

            [house.store]
            url = "file:///srv/house/bucket"

            [house.tls]
            cert = "/etc/house/cert.pem"
            key = "/etc/house/key.pem"
            "#,
        )
        .expect("parses");
        let house = &file.house;
        assert_eq!(house.listen, Some("127.0.0.1:18123".parse().expect("addr")));
        assert_eq!(house.single_node, Some(true));
        assert_eq!(
            UserMap::from(&house.users[0]).password_sha256,
            "ab".repeat(32)
        );
        let mut config = HouseConfig::default();
        house.limits.apply(&mut config);
        assert_eq!(config.max_query_size, 1024);
        assert_eq!(config.keep_alive, Duration::from_millis(2500));
        assert_eq!(config.session_limits.max_memory_usage, 1 << 30);
        assert_eq!(
            config.max_connections,
            HouseConfig::default().max_connections
        );
        let mut pool = PoolConfig::default();
        house.pool.apply(&mut pool);
        assert_eq!(pool.min_idle_workers, 1);
        assert_eq!(pool.idle_unbind_after, Duration::from_secs(5));
        assert_eq!(
            house.catalog.url.as_deref().map(str::parse::<CatalogSpec>),
            Some(Ok(CatalogSpec::Local("/srv/house/catalog.sqlite".into())))
        );
        assert!(
            !format!("{file:?}").contains("ABAB"),
            "the digest is redacted"
        );

        for bad in [
            "[house]\nlisten_typo = \"127.0.0.1:1\"",
            "[house.limits]\nmax_memory = 1",
            "[[house.users]]\nuser = \"bob\"\npassword_sha256 = \"abc\"\nnamespace = 1",
            "[[house.users]]\nuser = \"bob\"\npassword_sha256 = \"\"\nnamespace = 1",
            "[housee]",
        ] {
            let err = HouseFile::parse(bad).expect_err(bad);
            assert_eq!(err.code(), 36, "{bad}: {err}");
        }
    }

    #[test]
    fn catalog_and_workers_flags_parse() {
        assert_eq!(
            "rest:http://lakekeeper:8181/catalog".parse(),
            Ok(CatalogSpec::Rest("http://lakekeeper:8181/catalog".into()))
        );
        assert_eq!(
            "local:x.sqlite".parse(),
            Ok(CatalogSpec::Local("x.sqlite".into()))
        );
        for bad in ["rest:lakekeeper", "local:", "sqlite:x", ""] {
            assert!(bad.parse::<CatalogSpec>().is_err(), "{bad}");
        }
        assert_eq!("inproc".parse(), Ok(WorkersMode::Inproc));
        assert_eq!("process".parse(), Ok(WorkersMode::Process));
        assert!("thread".parse::<WorkersMode>().is_err());
    }

    #[test]
    fn loopback_only() {
        for ok in ["127.0.0.1:8123", "127.0.0.7:0", "[::1]:8123"] {
            assert!(check_listen(ok.parse().expect("addr")).is_ok(), "{ok}");
        }
        for bad in ["0.0.0.0:8123", "[::]:8123", "10.0.0.1:8123"] {
            let err = check_listen(bad.parse().expect("addr")).expect_err(bad);
            assert_eq!(
                err.message(),
                format!(
                    "house listen on {bad}: only loopback addresses are served until the \
                     unified auth plan (D111)"
                )
            );
        }
    }

    #[test]
    fn defaults_have_no_users_and_no_tmpfs_spool() {
        let config = HouseConfig::default();
        assert!(config.users.is_empty(), "review M9");
        assert!(!config.tmp_dir.starts_with("/tmp"), "review I2");
    }

    #[test]
    fn config_debug_redacts_credentials() {
        let config = HouseConfig {
            users: vec![UserMap::dev("alice", "secret", 1, false)],
            ..HouseConfig::default()
        };
        let digest = config.users[0].password_sha256.clone();
        let shown = format!("{config:?}");
        assert!(
            !shown.contains(&digest) && !shown.contains("secret"),
            "{shown}"
        );
    }

    #[test]
    fn passwords_verify_and_never_print() {
        let user = UserMap::dev("alice", "secret", 2, false);
        assert!(user.verifies("secret"));
        assert!(!user.verifies("Secret"));
        assert!(!user.verifies(""));
        let shown = format!("{user:?}");
        assert!(!shown.contains(&user.password_sha256), "{shown}");
        assert!(shown.contains("[redacted]"));
    }
}
