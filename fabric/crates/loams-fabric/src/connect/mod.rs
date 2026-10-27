//! `loams-fabric connect` (CC1 Task 1, design §80 §4.4, D1821): native
//! connector instances on a BYOC VM, until CN2's `NativeRuntime` absorbs
//! them.
//!
//! - [`instances`]: the instance files, checked against the registry
//!   (native runtimes only: `runtime_not_supported_on_vm`).
//! - [`engine`]: the engine's streams over its connect socket
//!   (`ProduceCloudEvents`, deduplicated by `source` + `id`;
//!   `FetchCloudEvents`), CC1 ruling R1.1.
//! - [`state`]: the process lock (an epoch, create-only), checkpoints, sink
//!   positions, controls and statuses in the bucket.
//! - [`supervisor`]: one SDK pipeline per instance, pause, resume, rate.
//! - [`admin`]: the loopback admin API.
//! - [`log`]: every log line scrubbed of resolved secrets.
//!
//! Secrets come from `FileSecretStore` under `--secrets-root`:
//! `<root>/<namespace_id>/<secret>/<key>`, owner-only files, which the BYOC
//! fetcher writes (`secrets/connect/0/confluent-<integration>/password`).

pub mod admin;
pub mod engine;
pub mod instances;
pub mod log;
pub mod state;
pub mod supervisor;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use loams_connector_sdk::{Scrubber, Secret};
use loams_flow::secrets::file::FileSecretStore;
use loams_store::Store;

use crate::cli::ConnectArgs;
use engine::{EngineClient, EngineUrl};
use instances::{Catalog, Factories, Instance};
use state::{Lock, StateError, StateStore};
use supervisor::{Deps, Supervisor};

/// How the role ended.
#[derive(Debug)]
pub enum Exit {
    /// SIGINT or SIGTERM: everything stopped and checkpointed.
    Stopped,
    /// A later process took the lock; nothing was checkpointed after that.
    Fenced(String),
}

/// A start-up failure, with the exit code it maps to.
#[derive(Debug)]
pub struct StartError {
    pub code: i32,
    pub message: String,
}

impl StartError {
    fn config(message: impl Into<String>) -> Self {
        Self {
            code: 78,
            message: message.into(),
        }
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            code: 69,
            message: message.into(),
        }
    }
}

/// A secret file: owner-only (the same rule as `FileSecretStore`), trailing
/// newline trimmed. `Ok(None)` when it does not exist.
pub fn read_secret_file(path: &Path) -> Result<Option<Secret>, String> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(format!(
                "{}: mode {:o}; it must be readable by its owner only",
                path.display(),
                meta.permissions().mode() & 0o777
            ));
        }
    }
    let _ = meta;
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let end = bytes
        .iter()
        .rposition(|b| *b != b'\n' && *b != b'\r')
        .map_or(0, |i| i + 1);
    Ok(Some(Secret::new(&bytes[..end])))
}

/// What a running role is made of; [`Connect::start`] builds it from flags,
/// tests from parts.
pub struct Connect {
    pub supervisor: Arc<Supervisor>,
    pub admin: admin::Admin,
    lock: Lock,
    fence_every: Duration,
}

impl std::fmt::Debug for Connect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connect")
            .field("epoch", &self.lock.epoch())
            .field("admin", &self.admin.local_addr())
            .finish_non_exhaustive()
    }
}

/// The parts of a role, for [`Connect::start_with`].
pub struct Parts {
    pub instances: Vec<Instance>,
    pub engine: EngineClient,
    pub state: StateStore,
    pub secrets: Arc<dyn loams_flow::secrets::SecretStore>,
    pub scrubber: Arc<Scrubber>,
    pub admin_listen: std::net::SocketAddr,
    pub admin_token: Option<Secret>,
    pub fence_every: Duration,
}

impl std::fmt::Debug for Parts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Parts")
            .field("instances", &self.instances.len())
            .field("engine", &self.engine)
            .field("admin_listen", &self.admin_listen)
            .finish_non_exhaustive()
    }
}

impl Connect {
    /// From the command line: load and check the instances, open the bucket,
    /// take the lock, start the instances and the admin API.
    pub async fn start(args: &ConnectArgs, scrubber: Arc<Scrubber>) -> Result<Self, StartError> {
        let catalog = Catalog::load(&args.connectors_dir).map_err(StartError::config)?;
        let factories = Factories::built_in();
        let instances =
            instances::load_dir(&args.instances, &catalog, &factories).map_err(|errors| {
                StartError::config(
                    errors
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
            })?;
        let url = EngineUrl::parse(&args.engine).map_err(StartError::config)?;
        let key_file = args
            .engine_key_file
            .clone()
            .unwrap_or_else(|| args.secrets_root.join("engine-key"));
        let key = read_secret_file(&key_file).map_err(StartError::config)?;
        if key.is_none() {
            tracing::warn!(
                file = %key_file.display(),
                "no engine key: calls to the engine carry no credential, which only an engine \
                 with authentication off accepts on its main port (never on the connect socket)"
            );
        }
        if let Some(k) = &key {
            scrubber.add(k);
        }
        let token_file = args
            .admin_token_file
            .clone()
            .unwrap_or_else(|| args.secrets_root.join("connect-admin-token"));
        let token = read_secret_file(&token_file).map_err(StartError::config)?;
        if let Some(t) = &token {
            scrubber.add(t);
        }
        let store = Store::from_url(&args.store, Vec::<(String, String)>::new())
            .map_err(|e| StartError::config(format!("--store {:?}: {e}", args.store)))?;
        Self::start_with(Parts {
            instances,
            engine: EngineClient::new(url, key),
            state: StateStore::new(store, &args.state_bucket_prefix),
            secrets: Arc::new(FileSecretStore::new(&args.secrets_root)),
            scrubber,
            admin_listen: args.admin_listen,
            admin_token: token,
            fence_every: Duration::from_secs(10),
        })
        .await
    }

    pub async fn start_with(p: Parts) -> Result<Self, StartError> {
        let owner = format!(
            "{}:{}",
            std::env::var("HOSTNAME").unwrap_or_else(|_| "connect".to_owned()),
            std::process::id()
        );
        let lock = Lock::acquire(&p.state, &owner)
            .await
            .map_err(|e| StartError::unavailable(e.to_string()))?;
        tracing::info!(
            epoch = lock.epoch(),
            instances = p.instances.len(),
            engine = %p.engine.url(),
            "connect: took the lock"
        );
        let supervisor = Supervisor::new(
            Deps {
                engine: p.engine,
                state: p.state,
                secrets: p.secrets,
                scrubber: p.scrubber,
                epoch: lock.epoch(),
            },
            p.instances,
        );
        supervisor.start_all().await;
        let admin = admin::serve(p.admin_listen, p.admin_token, supervisor.clone())
            .await
            .map_err(StartError::config)?;
        tracing::info!(admin = %admin.local_addr(), "connect: admin API listening");
        Ok(Self {
            supervisor,
            admin,
            lock,
            fence_every: p.fence_every,
        })
    }

    pub fn epoch(&self) -> u64 {
        self.lock.epoch()
    }

    /// Run until `stop` resolves (then stop gracefully) or a successor
    /// fences this process (then abort, with no final checkpoint).
    pub async fn run(self, stop: impl std::future::Future<Output = ()>) -> Exit {
        tokio::pin!(stop);
        let mut tick = tokio::time::interval(self.fence_every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut statuses = tokio::time::interval(Duration::from_secs(30));
        statuses.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = &mut stop => {
                    tracing::info!("connect: stopping");
                    self.supervisor.stop_all().await;
                    return Exit::Stopped;
                }
                _ = tick.tick() => {
                    let fenced = match self.lock.check().await {
                        Err(e @ StateError::Fenced { .. }) => Some(e.to_string()),
                        Err(e) => {
                            tracing::warn!("connect: checking the lock: {e}");
                            None
                        }
                        Ok(()) if self.supervisor.any_fenced().await => {
                            Some("a checkpoint save was fenced by a newer process".to_owned())
                        }
                        Ok(()) => None,
                    };
                    if let Some(why) = fenced {
                        tracing::error!("connect: {why}; stopping every instance without a checkpoint");
                        self.supervisor.abort_all().await;
                        return Exit::Fenced(why);
                    }
                    self.supervisor.retry_failed_starts().await;
                }
                _ = statuses.tick() => {
                    self.supervisor.write_statuses().await;
                }
            }
        }
    }
}
