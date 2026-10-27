//! Running the gateway from a configuration: what the standalone
//! `loams-temporal` binary and `loams-gateway`'s `temporal` module share
//! (plan DX1 Task 9).

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use loams_temporal_resonate::{Durability, FakeDurability, HttpDurability};
use loams_temporal_schedule::PgScheduleStore;
use loams_temporal_store::{MemStore, NamespaceRecord, PgStore, Store, StoreError};
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
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StoreConfig {
    Memory,
    Postgres { url_env: String },
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
    }
}

fn env(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("environment variable {name} is not set"))
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
        StoreConfig::Postgres { url_env } => {
            let pg = PgStore::connect(&env(url_env)?, config.shards)
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
                    Some(Arc::new(move || Some(t.clone()))
                        as loams_temporal_resonate::wire_interim::TokenFn)
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
            ring.push((k.id.clone(), env(&k.secret_env)?.into_bytes()));
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
    let gateway = Gateway::start(
        Parts {
            store,
            durability,
            auth,
            tokens,
            pid: config.pid.clone(),
            shards: config.shards,
            lease: Duration::from_secs(60),
            config: FrontendConfig::default(),
            ext,
        },
        stop_rx,
    )
    .await?;
    let local = listener.local_addr().map_err(|e| e.to_string())?;
    tracing::info!(listen = %local, "loams-temporal serving the Temporal API");
    tonic::transport::Server::builder()
        .add_service(gateway.service())
        .serve_with_incoming_shutdown(
            tokio_stream::wrappers::TcpListenerStream::new(listener),
            shutdown,
        )
        .await
        .map_err(|e| e.to_string())?;
    let _ = stop_tx.send(true);
    Ok(())
}

/// A random byte for the development token key (from the OS through
/// `uuid`'s v4 generator; no new dependency).
fn rand_byte() -> u8 {
    uuid::Uuid::new_v4().as_bytes()[0]
}
