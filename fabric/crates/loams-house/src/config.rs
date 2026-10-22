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
    /// The most one `INSERT` into a lake table writes, as its worker's Arrow
    /// bytes: 1 GiB (HS1 Task 12, R12.2).
    pub lake_insert_max_bytes: u64,
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
    /// Sessions one user may hold at once (HS1 Task 21; Task 4's I5).
    pub max_sessions_per_user: usize,
    /// Workers one user's sessions may pin (HS1 Task 21; Task 4's I5).
    pub max_pinned_workers_per_user: usize,
    /// The namespace queues and node memory admission (§49 §12, HS1 Task 21).
    pub admission: crate::limits::AdmissionLimits,
    /// Pipes and materialized views per namespace (§49 §12, HS1 Task 21); the
    /// lake holds them ([`crate::catalog::Lake::with_object_caps`]).
    pub object_caps: crate::catalog::ObjectCaps,
    /// The TLS listener (8443, HS1 Task 20), which may face other hosts:
    /// where it listens and its rustls configuration ([`crate::tls`]).
    pub tls: Option<TlsListener>,
    /// The Loams principals the engine's link acts for (HS1 Task 20).
    pub principals: Vec<crate::identity::PrincipalMap>,
    /// Who an unmapped principal acts as; `None` (the default): nobody, it
    /// has no House access. Only `--single-node --dev-unmapped-principals`
    /// sets it.
    pub unmapped_principals: Option<UserMap>,
    /// Where every statement's audit record goes (HS1 Task 20); `None`, nowhere.
    pub audit: Option<std::sync::Arc<dyn crate::audit::AuditSink>>,
    /// How often background compaction looks at tables that took commits
    /// (HS1 Task 13): 30 s; zero turns it off.
    pub compaction_interval: Duration,
    /// How long lake tables keep their snapshots, and dropped tables wait for
    /// `UNDROP` (`loams_snapshot_retention`, §49 §11): 7 days, at least 24 h
    /// (HS1 Task 15).
    pub snapshot_retention: Duration,
    /// How often snapshot expiry, orphan removal and purges sweep the
    /// catalog (HS1 Task 15): 1 h; zero turns it off.
    pub retention_interval: Duration,
    /// The admin listener (`--admin-listen`, 8125, loopback only): `GET
    /// /metrics` (HS1 Task 23). `None`: not served.
    pub admin_listen: Option<SocketAddr>,
    /// The engine whose streams `LoamsStream` pipes read (HS1 Task 16);
    /// `None`: no pipe can be made.
    pub engine: Option<EngineLink>,
    /// How often the pipe runner looks for records (HS1 Task 16): 1 s; zero
    /// turns it off.
    pub pipe_interval: Duration,
    /// How often materialized views over lake tables catch up on commits no
    /// `INSERT` ran them for (HS1 Task 17): 5 s; zero turns it off.
    pub view_interval: Duration,
    /// Where each namespace's quotas come from (HS1 Task 22): `[house.quotas]`
    /// by default; none set, none limit.
    pub quotas: std::sync::Arc<dyn crate::quota::QuotaSource>,
    /// Where usage events go (HS1 Task 22, §27); nowhere by default.
    pub usage: std::sync::Arc<dyn crate::usage::UsageObserver>,
    /// How often every lake table's bytes are measured (`house.storage`, HS1
    /// Task 22): daily; zero turns it off.
    pub storage_usage_interval: Duration,
    /// Whether the statement series of `/metrics` carry a `ns` label (§49
    /// §15: the operator's choice, one series per namespace): off.
    pub metrics_namespace_labels: bool,
    /// Where the statements' spans go (HS1 Task 23, OTLP); nowhere by default.
    /// `house-cache` shares it to put its reads under their worker's span.
    pub tracer: std::sync::Arc<crate::trace::Tracer>,
    /// Where each statement's operator line goes (HS1 Task 23, literals
    /// redacted); nowhere by default (`loams-fabric house` writes them to
    /// stderr).
    pub operator_log: Option<std::sync::Arc<dyn crate::oplog::OperatorLog>>,
    /// How often each namespace's query log rows are written (§49 §15): 5 s;
    /// zero writes them only when asked ([`crate::http::HouseHandle::flush_query_log`]).
    pub query_log_interval: Duration,
    /// How long query log rows are kept unless a namespace's table says
    /// otherwise (§49 §15): 30 days.
    pub query_log_retention: Duration,
    /// `house-cache`'s counters for `/metrics` (HS1 Task 23), when the front
    /// has one.
    pub cache_metrics: Option<std::sync::Arc<dyn crate::metrics::CacheMetrics>>,
}

/// The House's link to the engine's stream API (HS1 Task 16, `[house.engine]`).
#[derive(Clone, PartialEq, Eq)]
pub struct EngineLink {
    /// The engine's HTTP API (`https://engine:8080`).
    pub url: String,
    /// The House's own API key for it (never a caller's).
    pub key: Option<String>,
    /// Which engine namespace each House namespace's pipes read; a House
    /// namespace not listed has no pipes.
    pub namespaces: std::collections::BTreeMap<u64, String>,
}

impl fmt::Debug for EngineLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EngineLink")
            .field("url", &self.url)
            .field("key", &self.key.as_ref().map(|_| "[redacted]"))
            .field("namespaces", &self.namespaces)
            .finish()
    }
}

/// A TLS listener (HS1 Task 20).
#[derive(Clone, Debug)]
pub struct TlsListener {
    /// Where it listens; any address.
    pub listen: SocketAddr,
    /// Its certificate and protocols.
    pub config: std::sync::Arc<rustls::ServerConfig>,
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
            lake_insert_max_bytes: crate::insert::DEFAULT_MAX_INSERT_BYTES,
            tls: None,
            principals: Vec::new(),
            unmapped_principals: None,
            audit: None,
            compaction_interval: Duration::from_secs(30),
            snapshot_retention: crate::catalog::DEFAULT_RETENTION,
            retention_interval: crate::retention::DEFAULT_SWEEP_INTERVAL,
            admin_listen: None,
            engine: None,
            pipe_interval: Duration::from_secs(1),
            view_interval: Duration::from_secs(5),
            quotas: std::sync::Arc::new(crate::quota::ConfigQuotas::default()),
            usage: std::sync::Arc::new(crate::usage::NoUsageObserver),
            storage_usage_interval: crate::usage::DEFAULT_STORAGE_INTERVAL,
            metrics_namespace_labels: false,
            tracer: crate::trace::Tracer::off(),
            operator_log: None,
            query_log_interval: crate::querylog::DEFAULT_FLUSH_INTERVAL,
            query_log_retention: crate::querylog::DEFAULT_RETENTION,
            cache_metrics: None,
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
            max_sessions_per_user: crate::limits::MAX_SESSIONS_PER_USER,
            max_pinned_workers_per_user: crate::limits::MAX_PINNED_WORKERS_PER_USER,
            admission: crate::limits::AdmissionLimits::default(),
            object_caps: crate::catalog::ObjectCaps::default(),
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

/// A TLS listener off loopback faces other hosts (HS1 Task 20): no user may
/// sign in there without a password.
pub fn check_tls_users(listen: SocketAddr, users: &[UserMap]) -> Result<(), HouseError> {
    if listen.ip().is_loopback() {
        return Ok(());
    }
    match users.iter().find(|u| u.has_empty_password()) {
        Some(user) => Err(HouseError::from(ChError::bad_arguments(format!(
            "house TLS listen on {listen}: user {:?} has an empty password, and a listener \
             other hosts reach takes none",
            user.user
        )))),
        None => Ok(()),
    }
}

/// SHA-256 of the empty string: the digest of a user with no password.
const EMPTY_PASSWORD_SHA256: &str =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

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
    /// What it may do (HS1 Task 20, [`crate::identity`]).
    pub scopes: crate::identity::Scopes,
    /// The tables it may touch; `None`, every table of its namespace.
    pub grants: Option<crate::identity::Grants>,
    /// The engine's link: it acts for the Loams principal each request names.
    pub link: bool,
    /// The Loams principal this identity acts for, when the engine's link
    /// named one (what the audit records).
    pub principal: Option<String>,
}

impl UserMap {
    /// A user from a clear-text password, hashed here: every scope, or `read`
    /// alone when `readonly`; every table of its namespace.
    pub fn dev(user: &str, password: &str, namespace: u64, readonly: bool) -> Self {
        Self {
            user: user.to_string(),
            password_sha256: sha256_hex(password),
            namespace,
            readonly,
            scopes: if readonly {
                crate::identity::Scopes::READ
            } else {
                crate::identity::Scopes::ALL
            },
            grants: None,
            link: false,
            principal: None,
        }
    }

    /// An internal identity for the House's own work (compaction, pipes): every
    /// scope in `namespace`, and no password, so it can never sign in
    /// ([`Self::verifies`] compares against an empty digest, which no
    /// password's SHA-256 equals).
    pub fn system(user: &str, namespace: u64) -> Self {
        Self {
            user: user.to_string(),
            password_sha256: String::new(),
            namespace,
            readonly: false,
            scopes: crate::identity::Scopes::ALL,
            grants: None,
            link: false,
            principal: None,
        }
    }

    /// Whether this user signs in with the empty password.
    pub fn has_empty_password(&self) -> bool {
        self.password_sha256
            .eq_ignore_ascii_case(EMPTY_PASSWORD_SHA256)
    }

    /// The engine's link user (HS1 Task 20): signs in with `key` and acts for
    /// the principals of [`HouseConfig::principals`].
    pub fn link(user: &str, key: &str) -> Self {
        let mut link = Self::dev(user, key, 0, true);
        link.scopes = crate::identity::Scopes::default();
        link.link = true;
        link
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
            .field("scopes", &self.scopes)
            .field("grants", &self.grants)
            .field("link", &self.link)
            .field("principal", &self.principal)
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

/// The catalog of record (`--catalog=rest:<url>?warehouse=<name>|local:<path>`;
/// HS1 Task 10).
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
    /// Users, with their scopes and tables (HS1 Task 20).
    #[serde(default)]
    pub users: Vec<FileUser>,
    /// The Loams principals the engine's link acts for (HS1 Task 20).
    #[serde(default)]
    pub principals: Vec<FilePrincipal>,
    /// The file the audit is appended to (HS1 Task 20); `--audit-log`.
    pub audit_log: Option<PathBuf>,
    /// How long lake tables keep their snapshots and dropped tables wait for
    /// `UNDROP`, in hours (`loams_snapshot_retention`; 168, at least 24; HS1
    /// Task 15).
    pub snapshot_retention_hours: Option<u64>,
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
    /// `[house.engine]`: the engine whose streams pipes read (HS1 Task 16).
    #[serde(default)]
    pub engine: EngineSection,
    /// `[house.quotas]`: every namespace's quotas, and per-namespace ones
    /// (HS1 Task 22).
    #[serde(default)]
    pub quotas: crate::quota::QuotasSection,
    /// `[house.observability]`: metrics, traces, operator logs and the query
    /// log (HS1 Task 23).
    #[serde(default)]
    pub observability: ObservabilitySection,
}

/// `[house.observability]` (HS1 Task 23).
///
/// ```toml
/// [house.observability]
/// metrics_namespace_labels = false
/// otlp_endpoint = "http://otel-collector:4318"
/// operator_log = true
/// query_log_retention_days = 30
/// ```
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservabilitySection {
    /// A `ns` label on the statement series of `/metrics`.
    pub metrics_namespace_labels: Option<bool>,
    /// The OpenTelemetry collector the spans go to (OTLP/HTTP, JSON);
    /// `--otlp-endpoint`. None: no spans.
    pub otlp_endpoint: Option<String>,
    /// One redacted line per statement on stderr; on by default.
    pub operator_log: Option<bool>,
    /// How long the query log keeps its rows, in days (30).
    pub query_log_retention_days: Option<u64>,
}

impl ObservabilitySection {
    /// Sets what it names on `config` (the OTLP endpoint is the binary's to
    /// start, on its runtime).
    pub fn apply(&self, config: &mut HouseConfig) -> Result<(), String> {
        if let Some(on) = self.metrics_namespace_labels {
            config.metrics_namespace_labels = on;
        }
        if self.operator_log.unwrap_or(true) {
            config.operator_log = Some(std::sync::Arc::new(crate::oplog::StderrLines));
        }
        if let Some(days) = self.query_log_retention_days {
            if days == 0 {
                return Err(
                    "[house.observability] query_log_retention_days: at least 1".to_string()
                );
            }
            config.query_log_retention = Duration::from_secs(days.saturating_mul(24 * 3600));
        }
        Ok(())
    }
}

/// `[house.engine]` (HS1 Task 16).
///
/// ```toml
/// [house.engine]
/// url = "https://engine:8080"
/// key_file = "/run/secrets/house-engine-key"
/// [house.engine.namespaces]
/// 7 = "orders"
/// ```
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineSection {
    /// The engine's HTTP API.
    pub url: Option<String>,
    /// A file holding the House's API key for the engine.
    pub key_file: Option<PathBuf>,
    /// House namespace id → the engine namespace its pipes read.
    #[serde(default)]
    pub namespaces: std::collections::BTreeMap<String, String>,
}

impl EngineSection {
    /// The link, when a URL is set; the key file is read here.
    pub fn link(&self) -> Result<Option<EngineLink>, String> {
        let Some(url) = &self.url else {
            return Ok(None);
        };
        let key = match &self.key_file {
            Some(path) => Some(
                std::fs::read_to_string(path)
                    .map_err(|err| format!("[house.engine] key_file {}: {err}", path.display()))?
                    .trim()
                    .to_string(),
            ),
            None => None,
        };
        let mut namespaces = std::collections::BTreeMap::new();
        for (id, name) in &self.namespaces {
            let id: u64 = id
                .parse()
                .map_err(|_| format!("[house.engine.namespaces] {id:?} is not a namespace id"))?;
            namespaces.insert(id, name.clone());
        }
        Ok(Some(EngineLink {
            url: url.clone(),
            key,
            namespaces,
        }))
    }
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
    /// Its scopes (HS1 Task 20); absent, every scope (or `read` when
    /// `readonly`).
    #[serde(default)]
    pub scopes: Option<Vec<String>>,
    /// Its tables, `database.table` with `*` for any; absent, every table.
    #[serde(default)]
    pub tables: Option<Vec<String>>,
    /// The engine's link: it acts for `[[house.principals]]`.
    #[serde(default)]
    pub link: bool,
}

impl fmt::Debug for FileUser {
    /// The digest is a credential: never printed (HS1 Global Constraint).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileUser")
            .field("user", &self.user)
            .field("password_sha256", &"[redacted]")
            .field("namespace", &self.namespace)
            .field("readonly", &self.readonly)
            .field("scopes", &self.scopes)
            .field("tables", &self.tables)
            .field("link", &self.link)
            .finish()
    }
}

impl From<&FileUser> for UserMap {
    /// [`HouseFile::parse`] has checked the scopes and grants.
    fn from(user: &FileUser) -> Self {
        let scopes = match &user.scopes {
            Some(list) => crate::identity::Scopes::parse(&list.join(",")).unwrap_or_default(),
            None if user.readonly => crate::identity::Scopes::READ,
            None => crate::identity::Scopes::ALL,
        };
        Self {
            user: user.user.clone(),
            password_sha256: user.password_sha256.to_ascii_lowercase(),
            namespace: user.namespace,
            readonly: user.readonly || scopes.read_only(),
            scopes: if user.link {
                crate::identity::Scopes::default()
            } else {
                scopes
            },
            grants: user
                .tables
                .as_ref()
                .map(|t| crate::identity::Grants::parse(t).unwrap_or_default()),
            link: user.link,
            principal: None,
        }
    }
}

/// One `[[house.principals]]` entry (HS1 Task 20).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilePrincipal {
    /// The Loams principal (`tailscale:<login>`, `apikey:<id>`, `user:<sub>`,
    /// `session:<id>`, `dev-loopback`) or `email:<address>`.
    pub principal: String,
    /// The namespace its statements run in.
    pub namespace: u64,
    /// Its scopes: `read`, `write`, `ddl`, `admin`.
    pub scopes: Vec<String>,
    /// Its tables, `database.table` with `*` for any; absent, every table.
    #[serde(default)]
    pub tables: Option<Vec<String>>,
}

impl FilePrincipal {
    /// The principal's map, or why the entry is not valid.
    pub fn to_map(&self) -> Result<crate::identity::PrincipalMap, String> {
        Ok(crate::identity::PrincipalMap {
            principal: self.principal.clone(),
            namespace: self.namespace,
            scopes: crate::identity::Scopes::parse(&self.scopes.join(","))?,
            grants: self
                .tables
                .as_ref()
                .map(crate::identity::Grants::parse)
                .transpose()?,
        })
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
    /// [`HouseConfig::lake_insert_max_bytes`].
    pub lake_insert_max_bytes: Option<u64>,
    /// [`HouseConfig::max_live_sessions`].
    pub max_live_sessions: Option<usize>,
    /// The `max_memory_usage` cap (§49 §12).
    pub max_memory_usage: Option<u64>,
    /// The `max_execution_time` cap, in seconds (§49 §12).
    pub max_execution_time_s: Option<f64>,
    /// The `max_threads` cap (§49 §12).
    pub max_threads: Option<u64>,
    /// The `max_bytes_to_read` cap (§49 §12); the worker profile's is 1 TiB.
    pub max_bytes_to_read: Option<u64>,
    /// The `max_result_bytes` cap (§49 §12); the worker profile's is 10 GiB.
    pub max_result_bytes: Option<u64>,
    /// [`HouseConfig::max_sessions_per_user`].
    pub max_sessions_per_user: Option<usize>,
    /// [`HouseConfig::max_pinned_workers_per_user`].
    pub max_pinned_workers_per_user: Option<usize>,
    /// Statements running at once per namespace (§49 §12: 16).
    pub max_running_per_namespace: Option<usize>,
    /// Statements queued per namespace (§49 §12: 64).
    pub max_queued_per_namespace: Option<usize>,
    /// How long a queued statement waits before `202`, in milliseconds (§49
    /// §12: 30 000).
    pub queue_timeout_ms: Option<u64>,
    /// The node's memory, in bytes, of which admitted statements' caps may use
    /// 85 % (default: the front's cgroup limit, else the machine's memory).
    pub node_memory_bytes: Option<u64>,
    /// Pipes per namespace (§49 §12: 32).
    pub max_pipes_per_namespace: Option<usize>,
    /// Materialized views per namespace (§49 §12: 64).
    pub max_views_per_namespace: Option<usize>,
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
        set(
            &mut config.lake_insert_max_bytes,
            self.lake_insert_max_bytes,
        );
        set(&mut config.max_live_sessions, self.max_live_sessions);
        let caps = &mut config.session_limits;
        set(&mut caps.max_memory_usage, self.max_memory_usage);
        set(&mut caps.max_execution_time_s, self.max_execution_time_s);
        set(&mut caps.max_threads, self.max_threads);
        set(&mut caps.max_bytes_to_read, self.max_bytes_to_read);
        set(&mut caps.max_result_bytes, self.max_result_bytes);
        set(
            &mut config.max_sessions_per_user,
            self.max_sessions_per_user,
        );
        set(
            &mut config.max_pinned_workers_per_user,
            self.max_pinned_workers_per_user,
        );
        let admission = &mut config.admission;
        set(
            &mut admission.max_running_per_namespace,
            self.max_running_per_namespace,
        );
        set(
            &mut admission.max_queued_per_namespace,
            self.max_queued_per_namespace,
        );
        set(&mut admission.queue_timeout, self.queue_timeout_ms.map(ms));
        set(
            &mut admission.node_memory_budget,
            self.node_memory_bytes
                .map(crate::limits::node_memory_budget),
        );
        set(&mut config.object_caps.pipes, self.max_pipes_per_namespace);
        set(&mut config.object_caps.views, self.max_views_per_namespace);
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
            let bad = |why: String| {
                HouseError::from(ChError::bad_arguments(format!(
                    "the house configuration file: user {:?}: {why}",
                    user.user
                )))
            };
            if let Some(scopes) = &user.scopes {
                crate::identity::Scopes::parse(&scopes.join(",")).map_err(bad)?;
            }
            if let Some(tables) = &user.tables {
                crate::identity::Grants::parse(tables).map_err(bad)?;
            }
        }
        file.house.quotas.to_quotas().map_err(|why| {
            HouseError::from(ChError::bad_arguments(format!(
                "the house configuration file: {why}"
            )))
        })?;
        for principal in &file.house.principals {
            principal.to_map().map_err(|why| {
                HouseError::from(ChError::bad_arguments(format!(
                    "the house configuration file: principal {:?}: {why}",
                    principal.principal
                )))
            })?;
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
