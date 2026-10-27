//! Running the gateway from a configuration: what the standalone
//! `loams-temporal` binary and `loams-gateway`'s `temporal` module share
//! (plan DX1 Task 9).

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use loams_temporal_resonate::{Durability, FakeDurability, HttpDurability};
use loams_temporal_store::{MemStore, NamespaceRecord, PgStore, Store, StoreError};
use serde::Deserialize;

use crate::{
    Authenticator, DevAllowAll, FrontendConfig, Gateway, Parts, Principal, StaticKeys, TokenKeys,
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
        ui: None,
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

    let store: Arc<dyn Store> = match &config.store {
        StoreConfig::Memory => Arc::new(MemStore::new(config.shards).map_err(|e| e.to_string())?),
        StoreConfig::Postgres { url_env } => {
            let pg = PgStore::connect(&env(url_env)?, config.shards)
                .await
                .map_err(|e| format!("postgres: {e}"))?;
            pg.migrate().await.map_err(|e| format!("migrate: {e}"))?;
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
            config: FrontendConfig {
                history_shard_count: i32::try_from(config.shards).unwrap_or(i32::MAX),
                persistence_store: match &config.store {
                    StoreConfig::Memory => "memory".into(),
                    StoreConfig::Postgres { .. } => "postgres".into(),
                },
                ..FrontendConfig::default()
            },
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
    served
}

/// A random byte for the development token key (from the OS through
/// `uuid`'s v4 generator; no new dependency).
fn rand_byte() -> u8 {
    uuid::Uuid::new_v4().as_bytes()[0]
}
