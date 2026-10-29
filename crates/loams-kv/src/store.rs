//! [`Store`]: the seam's handle, an enum over the embedded and TiKV backends
//! (LV1 plan Ruling 1).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::future::BoxFuture;

#[cfg(feature = "faults")]
use crate::FaultPlan;
use crate::metrics::{KvObserver, NoopObserver, Outcome, op_metrics};
use crate::{Committed, GcBarrier, Snap, Ts, Txn, TxnError, TxnOptions, embedded};

/// Errors of opening a store, reading its clock, taking a snapshot or
/// setting a GC barrier. A failed transaction is a [`TxnError`].
#[derive(Debug, thiserror::Error)]
pub enum KvError {
    /// The embedded store failed (its file, a bad config, a closed store).
    #[error("embedded store: {0}")]
    Embedded(String),
    /// An embedded read below the GC safe window or past GC (the TiKV
    /// backend's `GcSafePoint`, its text unchanged): `at` is older than
    /// `safe_point`.
    #[error(
        "read at ts {at} is below the GC safe point: reads older than ts {safe_point} \
         (now − (gc life time − 1 min)) are refused, because GC may have dropped the versions"
    )]
    GcSafePoint { at: u64, safe_point: u64 },
    /// An embedded read at a timestamp ahead of the store's clock: above
    /// `limit`, one second past the later of the last timestamp issued and
    /// the wall clock.
    #[error(
        "read at ts {at} is ahead of the store's clock: timestamps above ts {limit} are refused"
    )]
    TsAhead { at: u64, limit: u64 },
    /// An embedded GC barrier below the safe point (the TiKV backend's
    /// `BarrierBelowSafePoint`, its text unchanged).
    #[error(
        "GC barrier '{service_id}' at ts {ts} refused: the minimum service safe point is \
         already at ts {min_safe_point}"
    )]
    BarrierBelowSafePoint {
        service_id: String,
        ts: u64,
        min_safe_point: u64,
    },
    /// The TiKV layer's error, its text unchanged.
    #[cfg(feature = "tikv")]
    #[error(transparent)]
    Tikv(#[from] loams_tikv::TikvError),
}

/// Which backend a [`Store`] runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Backend {
    Embedded,
    Tikv,
}

/// What [`Store::open`] opens.
#[derive(Debug, Clone)]
pub enum StoreConfig {
    /// An embedded store in one redb file (LV1 plan Ruling 4).
    Embedded(EmbeddedConfig),
    /// A keyspace-scoped TiKV handle.
    #[cfg(feature = "tikv")]
    Tikv(loams_tikv::TikvConfig),
}

/// An embedded store: the redb file, the keyspace and the root prefix every
/// key lives under, how long GC keeps old versions and how often it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedConfig {
    /// The redb file (unused by an in-memory store, whose
    /// [`EmbeddedConfig::in_memory`] sets it to `mem:<name>` for messages).
    pub path: PathBuf,
    /// `Some(name)`: an in-memory store on redb's `InMemoryBackend`, shared
    /// in this process by every handle opened with the same name and gone
    /// when the last of them is dropped (KV0 Task 4). `None`: the file at
    /// `path`.
    pub memory: Option<String>,
    pub keyspace: String,
    pub root: Vec<u8>,
    /// Reads stay possible for `gc_life_time − 1 min` (more than a minute;
    /// default 10 min).
    pub gc_life_time: Duration,
    /// How often GC runs (default 60 s). The first handle on a file sets it.
    pub gc_interval: Duration,
}

impl EmbeddedConfig {
    /// The keyspace `keyspace` in the file `path`, with an empty root and
    /// the default GC life time and interval.
    pub fn new(path: impl Into<PathBuf>, keyspace: impl Into<String>) -> Self {
        EmbeddedConfig {
            path: path.into(),
            memory: None,
            keyspace: keyspace.into(),
            root: Vec::new(),
            gc_life_time: embedded::DEFAULT_GC_LIFE_TIME,
            gc_interval: embedded::DEFAULT_GC_INTERVAL,
        }
    }
}

impl EmbeddedConfig {
    /// An in-memory store named `name` on the keyspace `keyspace`, with an
    /// empty root and the default GC life time and interval: redb's
    /// `InMemoryBackend`, no file and no `fsync`. Every handle this process
    /// opens with the same name shares one database, timestamp oracle,
    /// committer and GC thread, as handles on one file do; the data is gone
    /// when the last handle is dropped. Nothing survives the process.
    pub fn in_memory(name: &str, keyspace: impl Into<String>) -> Self {
        EmbeddedConfig {
            path: PathBuf::from(format!("{}{name}", embedded::MEMORY_PREFIX)),
            memory: Some(name.to_string()),
            ..EmbeddedConfig::new(PathBuf::new(), keyspace)
        }
    }
}

impl StoreConfig {
    /// The keyspace the store is bound to.
    pub fn keyspace(&self) -> &str {
        match self {
            StoreConfig::Embedded(c) => &c.keyspace,
            #[cfg(feature = "tikv")]
            StoreConfig::Tikv(c) => &c.keyspace,
        }
    }

    /// Which backend the store runs on.
    pub fn backend(&self) -> Backend {
        match self {
            StoreConfig::Embedded(_) => Backend::Embedded,
            #[cfg(feature = "tikv")]
            StoreConfig::Tikv(_) => Backend::Tikv,
        }
    }

    /// The root prefix every key of the store lives under.
    pub fn root(&self) -> &[u8] {
        match self {
            StoreConfig::Embedded(c) => &c.root,
            #[cfg(feature = "tikv")]
            StoreConfig::Tikv(c) => &c.root,
        }
    }

    /// This configuration on the keyspace `keyspace` (of the same file or
    /// cluster, under the same root).
    pub fn with_keyspace(&self, keyspace: &str) -> StoreConfig {
        let mut config = self.clone();
        match &mut config {
            StoreConfig::Embedded(c) => c.keyspace = keyspace.to_string(),
            #[cfg(feature = "tikv")]
            StoreConfig::Tikv(c) => c.keyspace = keyspace.to_string(),
        }
        config
    }

    /// This configuration under the root `root` (of the same keyspace).
    pub fn with_root(&self, root: Vec<u8>) -> StoreConfig {
        let mut config = self.clone();
        match &mut config {
            StoreConfig::Embedded(c) => c.root = root,
            #[cfg(feature = "tikv")]
            StoreConfig::Tikv(c) => c.root = root,
        }
        config
    }

    /// Creates the keyspace when the backend needs it first: on TiKV through
    /// PD's HTTP API (`loams_tikv::ensure_keyspace`, idempotent). An embedded
    /// keyspace is created when a store first opens it, so this does
    /// nothing there.
    pub async fn ensure_keyspace(&self) -> Result<(), KvError> {
        match self {
            StoreConfig::Embedded(_) => Ok(()),
            #[cfg(feature = "tikv")]
            StoreConfig::Tikv(c) => {
                loams_tikv::ensure_keyspace(&c.pd_http_url(), &c.keyspace).await?;
                Ok(())
            }
        }
    }

    /// Moves the keyspace to `state` on TiKV (PD's keyspace state,
    /// `loams_tikv::set_keyspace_state`). An embedded keyspace has no state,
    /// so this does nothing there.
    pub async fn set_keyspace_state(&self, state: KeyspaceState) -> Result<(), KvError> {
        match self {
            StoreConfig::Embedded(_) => {
                let _ = state;
                Ok(())
            }
            #[cfg(feature = "tikv")]
            StoreConfig::Tikv(c) => {
                loams_tikv::set_keyspace_state(&c.pd_http_url(), &c.keyspace, state.as_pd())
                    .await?;
                Ok(())
            }
        }
    }
}

/// A keyspace's state in PD ([`StoreConfig::set_keyspace_state`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyspaceState {
    Enabled,
    Disabled,
    Archived,
}

impl KeyspaceState {
    /// PD's name of the state.
    pub fn as_pd(self) -> &'static str {
        match self {
            KeyspaceState::Enabled => "ENABLED",
            KeyspaceState::Disabled => "DISABLED",
            KeyspaceState::Archived => "ARCHIVED",
        }
    }
}

/// A keyspace-scoped store under a root prefix, on one backend, with the
/// [`KvObserver`] its runs report to. Cheap to clone.
#[derive(Clone)]
pub struct Store {
    inner: Inner,
    observer: Arc<dyn KvObserver>,
}

/// The backend's handle.
#[derive(Debug, Clone)]
enum Inner {
    Embedded(embedded::Handle),
    #[cfg(feature = "tikv")]
    Tikv(loams_tikv::Tikv),
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("inner", &self.inner)
            .field("observer", &self.observer)
            .finish()
    }
}

impl Inner {
    fn into_store(self) -> Store {
        Store {
            inner: self,
            observer: Arc::new(NoopObserver),
        }
    }
}

impl From<embedded::Handle> for Store {
    fn from(handle: embedded::Handle) -> Self {
        Inner::Embedded(handle).into_store()
    }
}

#[cfg(feature = "tikv")]
impl From<loams_tikv::Tikv> for Store {
    fn from(tikv: loams_tikv::Tikv) -> Self {
        Inner::Tikv(tikv).into_store()
    }
}

impl Store {
    /// Opens the store `config` names. Embedded: opens (or creates) the
    /// file and the keyspace in it ([`embedded::Handle::open`]). On TiKV:
    /// connects and checks that the keyspace exists and the cluster runs API
    /// v2 (`loams_tikv::Tikv::connect`).
    pub async fn open(config: StoreConfig) -> Result<Self, KvError> {
        match config {
            StoreConfig::Embedded(config) => Ok(embedded::Handle::open(config).await?.into()),
            #[cfg(feature = "tikv")]
            StoreConfig::Tikv(config) => Ok(loams_tikv::Tikv::connect(config).await?.into()),
        }
    }

    /// Which backend this is.
    pub fn backend(&self) -> Backend {
        match &self.inner {
            Inner::Embedded(_) => Backend::Embedded,
            #[cfg(feature = "tikv")]
            Inner::Tikv(_) => Backend::Tikv,
        }
    }

    /// The TiKV handle of a tikv store (the cluster GC loop sweeps its commit
    /// tokens); `None` on the embedded backend.
    #[cfg(feature = "tikv")]
    pub fn as_tikv(&self) -> Option<&loams_tikv::Tikv> {
        match &self.inner {
            Inner::Embedded(_) => None,
            Inner::Tikv(t) => Some(t),
        }
    }

    /// A fresh timestamp from the store's oracle (PD's TSO on TiKV).
    pub async fn now(&self) -> Result<Ts, KvError> {
        match &self.inner {
            Inner::Embedded(h) => h.now().await,
            #[cfg(feature = "tikv")]
            Inner::Tikv(t) => Ok(Ts::from(t.now().await?)),
        }
    }

    /// A read-only view at `at`. Refused when `at` is older than `now −
    /// (gc_life_time − 1 min)` unless a [`GcBarrier`] of this store covers
    /// it (`loams_tikv::Tikv::snapshot`; [`embedded::Handle::snapshot`]).
    pub async fn snapshot(&self, at: Ts) -> Result<Snap, KvError> {
        match &self.inner {
            Inner::Embedded(h) => Ok(Snap::Embedded(h.snapshot(at).await?)),
            #[cfg(feature = "tikv")]
            Inner::Tikv(t) => Ok(Snap::Tikv(t.snapshot(at.into()).await?)),
        }
    }

    /// Runs `body` in a new transaction and commits it; reruns `body` at a
    /// new start timestamp on a conflict or a not-applied error. With
    /// `commit_token`, an undetermined commit is resolved through a token
    /// written in the transaction.
    ///
    /// [`Mode::Pessimistic`](crate::Mode::Pessimistic) takes each key's lock as the body reads it
    /// with [`Txn::get_for_update`] or writes it (D851; KV0 Task 5), so a
    /// second holder waits rather than conflicting at commit.
    ///
    /// A body is rerun, so it must be free of side effects outside `txn`. It
    /// returns `Err(TxnError::Conflict)` to ask for a restart; to reject
    /// without a retry, return `Ok` with the caller's own error inside `T`
    /// (the transaction then commits whatever it wrote), or `Fatal`.
    pub async fn run<T: Send, F>(&self, opts: TxnOptions, body: F) -> Result<Committed<T>, TxnError>
    where
        F: for<'t> FnMut(&'t mut Txn) -> BoxFuture<'t, Result<T, TxnError>>,
    {
        let (op, started) = (opts.op, Instant::now());
        let result = match &self.inner {
            Inner::Embedded(h) => h.run(opts, body).await,
            #[cfg(feature = "tikv")]
            Inner::Tikv(t) => crate::tikv::run(t, opts, body).await,
        };
        let (outcome, elapsed) = (Outcome::of(&result), started.elapsed());
        op_metrics().record(self.backend(), op, outcome, elapsed);
        self.observer.on_op(self.keyspace(), op, outcome, elapsed);
        result
    }

    /// This store with its runs reported to `observer`, with the keyspace
    /// (the only per-tenant view of them; the metrics carry no tenant,
    /// D1021). Clones share it.
    pub fn with_observer(self, observer: Arc<dyn KvObserver>) -> Self {
        Store { observer, ..self }
    }

    /// The embedded handle of an embedded store; `None` on TiKV.
    pub fn as_embedded(&self) -> Option<&embedded::Handle> {
        match &self.inner {
            Inner::Embedded(h) => Some(h),
            #[cfg(feature = "tikv")]
            Inner::Tikv(_) => None,
        }
    }

    /// The keyspace this store is bound to.
    pub fn keyspace(&self) -> &str {
        match &self.inner {
            Inner::Embedded(h) => h.keyspace(),
            #[cfg(feature = "tikv")]
            Inner::Tikv(t) => t.keyspace(),
        }
    }

    /// The root prefix every key of this store lives under.
    pub fn root(&self) -> &[u8] {
        match &self.inner {
            Inner::Embedded(h) => h.root(),
            #[cfg(feature = "tikv")]
            Inner::Tikv(t) => t.root(),
        }
    }

    /// `root ‖ suffix`: the key of `suffix` under this store's root.
    pub fn key(&self, suffix: &[u8]) -> Vec<u8> {
        let root = self.root();
        let mut key = Vec::with_capacity(root.len() + suffix.len());
        key.extend_from_slice(root);
        key.extend_from_slice(suffix);
        key
    }

    /// This store with `plan` consulted at every fault point of every
    /// [`run`](Self::run) (R1 plan Task 2; feature `faults`).
    #[cfg(feature = "faults")]
    pub fn with_faults(self, plan: Arc<dyn FaultPlan>) -> Self {
        let inner = match self.inner {
            Inner::Embedded(h) => Inner::Embedded(h.with_faults(plan)),
            #[cfg(feature = "tikv")]
            Inner::Tikv(t) => Inner::Tikv(t.with_faults(crate::tikv::faults(plan))),
        };
        Store { inner, ..self }
    }

    /// Holds GC at or below `at` for `ttl` (whole seconds, at least 1) under
    /// the service id `loams/<name>` (`name` is `<purpose>/<id>`), and lets
    /// this store's snapshots read at the timestamps it covers while it
    /// lives. Setting the same `name` again moves it. Refused when GC is
    /// already past `at`.
    pub async fn barrier(&self, name: &str, at: Ts, ttl: Duration) -> Result<GcBarrier, KvError> {
        match &self.inner {
            Inner::Embedded(h) => h.barrier(name, at, ttl).await,
            #[cfg(feature = "tikv")]
            Inner::Tikv(t) => crate::tikv::barrier(t, name, at, ttl).await,
        }
    }
}
