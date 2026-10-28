//! Running the gateway from a configuration: what the standalone
//! `loams-temporal` binary and `loams-gateway`'s `temporal` module share
//! (plan DX1 Task 9).

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use loams_temporal_resonate::{Durability, FakeDurability, HttpDurability};
use loams_temporal_schedule::PgScheduleStore;
use loams_temporal_store::retention::{RetentionPolicy, spawn_sweeper};
use loams_temporal_store::{
    MemStore, NamespaceRecord, PgStore, PgStoreOptions, PurgeLimits, Store, StoreError,
};
use loams_temporal_visibility::PgVisibility;
use serde::Deserialize;

use crate::{
    Authenticator, DevAllowAll, FrontendConfig, Gateway, Parts, PartsExt, Principal, StaticKeys,
    TokenKeys,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_listen")]
    pub listen: SocketAddr,
    /// Allows the memory store, the in-process Resonate and no auth; only
    /// on a loopback listener.
    #[serde(default)]
    pub insecure_dev: bool,
    #[serde(default = "default_shards")]
    pub shards: u32,
    #[serde(default = "default_pid")]
    pub pid: String,
    pub store: StoreConfig,
    pub durability: DurabilityConfig,
    #[serde(default)]
    pub task_token_keys: Vec<TokenKey>,
    /// API keys: SHA-256 of each `lk_` key with its Loams namespace (X1.R5).
    #[serde(default)]
    pub keys: Vec<KeyRow>,
    /// With `insecure_dev`: the Loams namespace every caller gets.
    #[serde(default)]
    pub dev_loams_namespace: Option<String>,
    /// Temporal namespaces created at start when missing.
    #[serde(default)]
    pub bootstrap_namespaces: Vec<BootstrapNamespace>,
    /// Deleting closed runs after their namespace's retention (Q1163).
    #[serde(default)]
    pub retention: RetentionConfig,
    /// A second, loopback-only listener for the Temporal Web UI's
    /// `ui-server`, which cannot send an API key (D2050).
    #[serde(default)]
    pub ui: Option<UiConfig>,
}

/// The UI listener (D2050): every call on it is `loams_namespace_id`'s
/// (never admin), and with `read_only` (the default) only reads pass.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiConfig {
    /// A loopback address, such as `127.0.0.1:7234`.
    pub listen: SocketAddr,
    /// The Loams namespace whose Temporal namespaces the UI sees.
    pub loams_namespace_id: String,
    #[serde(default = "default_true")]
    pub read_only: bool,
}

fn default_true() -> bool {
    true
}

/// Where history, mutable state and the outbox live (design 74 §5.5).
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StoreConfig {
    /// In process; lost on restart (development only).
    Memory,
    /// A Loams Postgres database on a session or direct endpoint, never
    /// PgDog transaction mode (§5.5 rule 5) and never Loams SQL (rule 6).
    Postgres {
        /// The environment variable that holds the database URL.
        url_env: String,
        #[serde(default)]
        pool: PoolConfig,
    },
}

/// The Postgres store's connection pool (`[store.pool]`). Unset fields keep
/// [`PgStoreOptions::new`]'s defaults.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolConfig {
    /// At least 2: one connection is the outbox `LISTEN` (default 16).
    pub max_connections: Option<u32>,
    /// Default 0.
    pub min_connections: Option<u32>,
    /// Default 10.
    pub acquire_timeout_secs: Option<u64>,
    /// Default 600; 0 keeps idle connections forever.
    pub idle_timeout_secs: Option<u64>,
    /// Default 1800; 0 never recycles a connection.
    pub max_lifetime_secs: Option<u64>,
    /// Postgres `statement_timeout` (default 30000); 0 turns it off.
    pub statement_timeout_ms: Option<u64>,
    /// Postgres `idle_in_transaction_session_timeout` (default 60000); 0
    /// turns it off.
    pub idle_in_transaction_timeout_ms: Option<u64>,
    /// Default `loams-temporal`.
    pub application_name: Option<String>,
}

impl PoolConfig {
    /// The store options for `shards` shards.
    pub fn options(&self, shards: u32) -> PgStoreOptions {
        let mut o = PgStoreOptions::new(shards);
        let off_if_zero = |d: Duration| (!d.is_zero()).then_some(d);
        if let Some(n) = self.max_connections {
            o.max_connections = n;
        }
        if let Some(n) = self.min_connections {
            o.min_connections = n;
        }
        if let Some(s) = self.acquire_timeout_secs {
            o.acquire_timeout = Duration::from_secs(s);
        }
        if let Some(s) = self.idle_timeout_secs {
            o.idle_timeout = off_if_zero(Duration::from_secs(s));
        }
        if let Some(s) = self.max_lifetime_secs {
            o.max_lifetime = off_if_zero(Duration::from_secs(s));
        }
        if let Some(ms) = self.statement_timeout_ms {
            o.statement_timeout = off_if_zero(Duration::from_millis(ms));
        }
        if let Some(ms) = self.idle_in_transaction_timeout_ms {
            o.idle_in_transaction_timeout = off_if_zero(Duration::from_millis(ms));
        }
        if let Some(name) = &self.application_name {
            o.application_name = name.clone();
        }
        o
    }
}

/// The retention sweeper (`[retention]`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetentionConfig {
    /// Default true.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Seconds between rounds (default 60).
    #[serde(default = "default_retention_interval")]
    pub interval_secs: u64,
    /// Closed runs one purge call deletes (default 100).
    #[serde(default = "default_max_runs")]
    pub max_runs_per_call: usize,
    /// Dispatched outbox rows one purge call deletes (default 1000).
    #[serde(default = "default_max_outbox_rows")]
    pub max_outbox_rows_per_call: usize,
    /// How long a dispatched outbox row is kept, for debugging a relay
    /// (default 24).
    #[serde(default = "default_outbox_ttl_hours")]
    pub dispatched_outbox_ttl_hours: u64,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_secs: default_retention_interval(),
            max_runs_per_call: default_max_runs(),
            max_outbox_rows_per_call: default_max_outbox_rows(),
            dispatched_outbox_ttl_hours: default_outbox_ttl_hours(),
        }
    }
}

impl RetentionConfig {
    pub fn policy(&self) -> RetentionPolicy {
        RetentionPolicy {
            interval: Duration::from_secs(self.interval_secs.max(1)),
            limits: PurgeLimits {
                max_runs: self.max_runs_per_call,
                max_outbox_rows: self.max_outbox_rows_per_call,
                dispatched_outbox_ttl: Duration::from_secs(
                    self.dispatched_outbox_ttl_hours.saturating_mul(3600),
                ),
            },
            ..RetentionPolicy::default()
        }
    }
}

fn default_retention_interval() -> u64 {
    60
}
fn default_max_runs() -> usize {
    100
}
fn default_max_outbox_rows() -> usize {
    1000
}
fn default_outbox_ttl_hours() -> u64 {
    24
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DurabilityConfig {
    /// The in-process fake, on the wall clock (development only).
    InProcess,
    /// A Resonate server's protocol endpoint.
    Resonate {
        url: String,
        #[serde(default)]
        token_env: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenKey {
    pub id: String,
    pub secret_env: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyRow {
    pub sha256: String,
    pub loams_namespace_id: String,
    #[serde(default)]
    pub role: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapNamespace {
    pub loams_namespace_id: String,
    pub name: String,
    #[serde(default = "default_retention_hours")]
    pub retention_hours: i64,
}

fn default_listen() -> SocketAddr {
    "127.0.0.1:7233".parse().expect("valid")
}
fn default_shards() -> u32 {
    loams_temporal_store::DEFAULT_SHARDS
}
fn default_pid() -> String {
    "loams-temporal-1".into()
}
fn default_retention_hours() -> i64 {
    72
}

pub fn dev_config() -> Config {
    Config {
        listen: default_listen(),
        insecure_dev: true,
        shards: default_shards(),
        pid: default_pid(),
        store: StoreConfig::Memory,
        durability: DurabilityConfig::InProcess,
        task_token_keys: Vec::new(),
        keys: Vec::new(),
        dev_loams_namespace: Some("local".into()),
        bootstrap_namespaces: vec![BootstrapNamespace {
            loams_namespace_id: "local".into(),
            name: "default".into(),
            retention_hours: 72,
        }],
        ui: None,
        retention: RetentionConfig::default(),
    }
}

fn env(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("environment variable {name} is not set"))
}

/// The shortest task-token key accepted from the environment, in bytes
/// (HMAC-SHA256's block is 64; 32 is its output size, D2032).
pub const MIN_TOKEN_KEY_BYTES: usize = 32;

/// A task-token key read from `name`: refused when it is empty or shorter
/// than [`MIN_TOKEN_KEY_BYTES`], so an unset compose variable (`${VAR:-}`)
/// never signs tokens with an empty key (D2032).
pub fn token_secret(name: &str, value: String) -> Result<Vec<u8>, String> {
    if value.len() < MIN_TOKEN_KEY_BYTES {
        return Err(format!(
            "task-token key {name} must be at least {MIN_TOKEN_KEY_BYTES} bytes (it has {})",
            value.len()
        ));
    }
    Ok(value.into_bytes())
}

/// Checks that the development-only settings are on a loopback listener
/// with `insecure_dev` (plan DX1 Task 9).
pub fn check(c: &Config) -> Result<(), String> {
    let dev_parts = matches!(c.store, StoreConfig::Memory)
        || matches!(c.durability, DurabilityConfig::InProcess)
        || c.keys.is_empty()
        || c.task_token_keys.is_empty();
    if dev_parts && !c.insecure_dev {
        return Err("the memory store, the in-process Resonate, no API keys and generated token keys need insecure_dev = true".into());
    }
    if c.insecure_dev && !c.listen.ip().is_loopback() {
        return Err(format!(
            "insecure_dev is only allowed on a loopback listener, not {}",
            c.listen
        ));
    }
    if let Some(ui) = &c.ui {
        // Anyone who reaches the UI listener acts as its principal, so it
        // must not be reachable off the host (or the pod's network
        // namespace).
        if !ui.listen.ip().is_loopback() {
            return Err(format!(
                "[ui] listen must be a loopback address, not {}",
                ui.listen
            ));
        }
        if ui.listen == c.listen {
            return Err("[ui] listen must differ from listen".into());
        }
        if ui.loams_namespace_id.is_empty() {
            return Err("[ui] loams_namespace_id is empty".into());
        }
    }
    Ok(())
}

/// Builds the gateway `config` describes, serves the Temporal API on its
/// listener until `shutdown`, then stops the background loops.
pub async fn run(config: Config, shutdown: impl Future<Output = ()> + Send) -> Result<(), String> {
    check(&config)?;
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .map_err(|e| format!("bind {}: {e}", config.listen))?;
    run_on(config, listener, shutdown).await
}

/// [`run`] on an already bound listener (tests bind port 0).
pub async fn run_on(
    config: Config,
    listener: tokio::net::TcpListener,
    shutdown: impl Future<Output = ()> + Send,
) -> Result<(), String> {
    check(&config)?;

    let mut ext = PartsExt::default();
    let store: Arc<dyn Store> = match &config.store {
        StoreConfig::Memory => Arc::new(MemStore::new(config.shards).map_err(|e| e.to_string())?),
        StoreConfig::Postgres { url_env, pool } => {
            let pg = PgStore::connect_url(&env(url_env)?, &pool.options(config.shards))
                .await
                .map_err(|e| format!("postgres: {e}"))?;
            pg.migrate().await.map_err(|e| format!("migrate: {e}"))?;
            // Schedules live in their own table on the same database (D2072).
            let schedules = PgScheduleStore::with_pool(pg.pool().clone());
            schedules
                .migrate()
                .await
                .map_err(|e| format!("migrate schedules: {e}"))?;
            ext.schedules = Some(Arc::new(schedules));
            // Visibility reads the store's `visibility` table; its migration
            // adds the indexes and the search-attribute registry (D2075).
            let visibility = PgVisibility::with_pool(pg.pool().clone());
            visibility
                .migrate()
                .await
                .map_err(|e| format!("migrate visibility: {e}"))?;
            ext.visibility = Some(Arc::new(visibility));
            Arc::new(pg)
        }
    };
    let durability: Arc<dyn Durability> = match &config.durability {
        DurabilityConfig::InProcess => {
            let now = || {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0)
            };
            let fake = Arc::new(FakeDurability::starting_at(now()));
            let clock = fake.clone();
            // The fake's clock follows the wall clock, so timers fire.
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    let d = now() - clock.now();
                    if d > 0 {
                        clock.advance(d);
                    }
                }
            });
            fake
        }
        DurabilityConfig::Resonate { url, token_env } => {
            let token = match token_env {
                Some(name) => {
                    let t = env(name)?;
                    Some(Arc::new(move || Some(t.clone())) as loams_temporal_resonate::TokenFn)
                }
                None => None,
            };
            Arc::new(HttpDurability::new(url, &config.pid, token).map_err(|e| e.to_string())?)
        }
    };
    let auth: Arc<dyn Authenticator> = if config.keys.is_empty() {
        let ns = config
            .dev_loams_namespace
            .clone()
            .ok_or("insecure_dev without keys needs dev_loams_namespace")?;
        tracing::warn!("no API keys: every caller is admin of Loams namespace {ns} (insecure_dev)");
        Arc::new(DevAllowAll(Principal {
            loams_namespace_id: ns,
            admin: true,
        }))
    } else {
        let mut keys = StaticKeys::new();
        for k in &config.keys {
            keys.insert(
                k.sha256.clone(),
                Principal {
                    loams_namespace_id: k.loams_namespace_id.clone(),
                    admin: k.role == "admin",
                },
            );
        }
        Arc::new(keys)
    };
    let tokens = if config.task_token_keys.is_empty() {
        let secret: Vec<u8> = (0..32).map(|_| rand_byte()).collect();
        TokenKeys::new(vec![("dev".into(), secret)])
    } else {
        let mut ring = Vec::new();
        for k in &config.task_token_keys {
            ring.push((
                k.id.clone(),
                token_secret(&k.secret_env, env(&k.secret_env)?)?,
            ));
        }
        TokenKeys::new(ring)
    };
    for b in &config.bootstrap_namespaces {
        let record = NamespaceRecord {
            namespace_id: uuid::Uuid::new_v4().to_string(),
            loams_namespace_id: b.loams_namespace_id.clone(),
            name: b.name.clone(),
            retention_secs: b.retention_hours * 3600,
            state: 1,
            config: serde_json::json!({ "description": "bootstrapped" }),
        };
        match store.create_namespace(record).await {
            Ok(()) | Err(StoreError::NamespaceExists) => {}
            Err(e) => return Err(format!("bootstrap namespace {}: {e}", b.name)),
        }
    }

    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let sweeper = config
        .retention
        .enabled
        .then(|| spawn_sweeper(store.clone(), config.retention.policy(), stop_rx.clone()));
    let gateway = Gateway::start(
        Parts {
            store,
            durability,
            auth,
            tokens,
            pid: config.pid.clone(),
            shards: config.shards,
            lease: Duration::from_secs(60),
            config: FrontendConfig {
                history_shard_count: i32::try_from(config.shards).unwrap_or(i32::MAX),
                persistence_store: match &config.store {
                    StoreConfig::Memory => "memory".into(),
                    StoreConfig::Postgres { .. } => "postgres".into(),
                },
                ..FrontendConfig::default()
            },
            ext,
        },
        stop_rx,
    )
    .await?;
    let local = listener.local_addr().map_err(|e| e.to_string())?;
    tracing::info!(listen = %local, "loams-temporal serving the Temporal API");
    let (shutdown_all, _) = tokio::sync::broadcast::channel::<()>(1);
    let ui_server = match &config.ui {
        Some(ui) => {
            let ui_listener = tokio::net::TcpListener::bind(ui.listen)
                .await
                .map_err(|e| format!("bind [ui] {}: {e}", ui.listen))?;
            tracing::info!(
                listen = %ui.listen,
                read_only = ui.read_only,
                loams_namespace = %ui.loams_namespace_id,
                "loams-temporal serving the Temporal Web UI listener (no API key)"
            );
            let principal = Principal {
                loams_namespace_id: ui.loams_namespace_id.clone(),
                admin: false,
            };
            let router = tonic::transport::Server::builder()
                .add_service(crate::ui::UiListener::new(
                    gateway.service(),
                    principal.clone(),
                    ui.read_only,
                ))
                .add_service(crate::ui::UiListener::new(
                    gateway.operator_service(),
                    principal,
                    ui.read_only,
                ));
            let mut rx = shutdown_all.subscribe();
            Some(tokio::spawn(async move {
                router
                    .serve_with_incoming_shutdown(
                        tokio_stream::wrappers::TcpListenerStream::new(ui_listener),
                        async move {
                            let _ = rx.recv().await;
                        },
                    )
                    .await
            }))
        }
        None => None,
    };
    let served = tonic::transport::Server::builder()
        .add_service(gateway.service())
        .add_service(gateway.operator_service())
        .serve_with_incoming_shutdown(
            tokio_stream::wrappers::TcpListenerStream::new(listener),
            shutdown,
        )
        .await
        .map_err(|e| e.to_string());
    let _ = shutdown_all.send(());
    if let Some(h) = ui_server {
        match h.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!("the UI listener stopped: {e}"),
            Err(e) => tracing::warn!("the UI listener's task failed: {e}"),
        }
    }
    let _ = stop_tx.send(true);
    if let Some(h) = sweeper {
        // It stops at its next await; a purge in flight finishes first.
        let _ = h.await;
    }
    served
}

/// A random byte for the development token key (from the OS through
/// `uuid`'s v4 generator; no new dependency).
fn rand_byte() -> u8 {
    uuid::Uuid::new_v4().as_bytes()[0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_secret_refuses_empty_and_short_keys() {
        assert!(token_secret("K", String::new()).is_err());
        assert!(token_secret("K", "x".repeat(MIN_TOKEN_KEY_BYTES - 1)).is_err());
        assert_eq!(
            token_secret("K", "x".repeat(MIN_TOKEN_KEY_BYTES)).unwrap(),
            vec![b'x'; MIN_TOKEN_KEY_BYTES]
        );
    }

    #[test]
    fn postgres_store_pool_and_retention_parse() {
        let text = r#"
listen = "0.0.0.0:7233"
[store]
kind = "postgres"
url_env = "LOAMS_TEMPORAL_DATABASE_URL"
[store.pool]
max_connections = 32
statement_timeout_ms = 0
application_name = "temporal-a"
[durability]
kind = "resonate"
url = "http://127.0.0.1:8001"
[retention]
interval_secs = 30
max_runs_per_call = 10
"#;
        let c: Config = toml::from_str(text).expect("parses");
        let StoreConfig::Postgres { url_env, pool } = &c.store else {
            panic!("postgres store expected");
        };
        assert_eq!(url_env, "LOAMS_TEMPORAL_DATABASE_URL");
        let o = pool.options(c.shards);
        assert_eq!(o.shards, 64);
        assert_eq!(o.max_connections, 32);
        assert_eq!(o.statement_timeout, None);
        assert_eq!(o.application_name, "temporal-a");
        // Unset fields keep the store's defaults.
        assert_eq!(o.min_connections, PgStoreOptions::new(64).min_connections);
        let p = c.retention.policy();
        assert_eq!(p.interval, Duration::from_secs(30));
        assert_eq!(p.limits.max_runs, 10);
        assert_eq!(p.limits.max_outbox_rows, 1000);
        assert_eq!(
            p.limits.dispatched_outbox_ttl,
            Duration::from_secs(24 * 3600)
        );
        assert!(c.retention.enabled);
    }

    #[test]
    fn store_without_pool_and_unknown_pool_key() {
        let base = r#"
[store]
kind = "postgres"
url_env = "U"
[durability]
kind = "in_process"
"#;
        let c: Config = toml::from_str(base).expect("parses");
        let StoreConfig::Postgres { pool, .. } = &c.store else {
            panic!("postgres store expected");
        };
        assert_eq!(pool.options(64), PgStoreOptions::new(64));
        let bad = format!("{base}\n[store.pool]\nmax_conns = 3\n");
        assert!(toml::from_str::<Config>(&bad).is_err());
        let off = format!("{base}\n[retention]\nenabled = false\n");
        let c: Config = toml::from_str(&off).expect("parses");
        assert!(!c.retention.enabled);
    }
}
