//! [`Store`]: the seam's handle, an enum over the embedded and TiKV backends
//! (LV1 plan Ruling 1).

use std::path::PathBuf;
#[cfg(feature = "faults")]
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;

#[cfg(feature = "faults")]
use crate::FaultPlan;
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
    pub path: PathBuf,
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
            keyspace: keyspace.into(),
            root: Vec::new(),
            gc_life_time: embedded::DEFAULT_GC_LIFE_TIME,
            gc_interval: embedded::DEFAULT_GC_INTERVAL,
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
}

/// A keyspace-scoped store under a root prefix. Cheap to clone.
#[derive(Debug, Clone)]
pub enum Store {
    Embedded(embedded::Handle),
    #[cfg(feature = "tikv")]
    Tikv(loams_tikv::Tikv),
}

#[cfg(feature = "tikv")]
impl From<loams_tikv::Tikv> for Store {
    fn from(tikv: loams_tikv::Tikv) -> Self {
        Store::Tikv(tikv)
    }
}

impl Store {
    /// Opens the store `config` names. Embedded: opens (or creates) the
    /// file and the keyspace in it ([`embedded::Handle::open`]). On TiKV:
    /// connects and checks that the keyspace exists and the cluster runs API
    /// v2 (`loams_tikv::Tikv::connect`).
    pub async fn open(config: StoreConfig) -> Result<Self, KvError> {
        match config {
            StoreConfig::Embedded(config) => {
                Ok(Store::Embedded(embedded::Handle::open(config).await?))
            }
            #[cfg(feature = "tikv")]
            StoreConfig::Tikv(config) => Ok(Store::Tikv(loams_tikv::Tikv::connect(config).await?)),
        }
    }

    /// Which backend this is.
    pub fn backend(&self) -> Backend {
        match self {
            Store::Embedded(_) => Backend::Embedded,
            #[cfg(feature = "tikv")]
            Store::Tikv(_) => Backend::Tikv,
        }
    }

    /// The TiKV handle of a tikv store (the cluster GC loop sweeps its commit
    /// tokens); `None` on the embedded backend.
    #[cfg(feature = "tikv")]
    pub fn as_tikv(&self) -> Option<&loams_tikv::Tikv> {
        match self {
            Store::Embedded(_) => None,
            Store::Tikv(t) => Some(t),
        }
    }

    /// A fresh timestamp from the store's oracle (PD's TSO on TiKV).
    pub async fn now(&self) -> Result<Ts, KvError> {
        match self {
            Store::Embedded(h) => h.now().await,
            #[cfg(feature = "tikv")]
            Store::Tikv(t) => Ok(Ts::from(t.now().await?)),
        }
    }

    /// A read-only view at `at`. Refused when `at` is older than `now −
    /// (gc_life_time − 1 min)` unless a [`GcBarrier`] of this store covers
    /// it (`loams_tikv::Tikv::snapshot`; [`embedded::Handle::snapshot`]).
    pub async fn snapshot(&self, at: Ts) -> Result<Snap, KvError> {
        match self {
            Store::Embedded(h) => Ok(Snap::Embedded(h.snapshot(at).await?)),
            #[cfg(feature = "tikv")]
            Store::Tikv(t) => Ok(Snap::Tikv(t.snapshot(at.into()).await?)),
        }
    }

    /// Runs `body` in a new transaction and commits it; reruns `body` at a
    /// new start timestamp on a conflict or a not-applied error. With
    /// `commit_token`, an undetermined commit is resolved through a token
    /// written in the transaction.
    ///
    /// A body is rerun, so it must be free of side effects outside `txn`. It
    /// returns `Err(TxnError::Conflict)` to ask for a restart; to reject
    /// without a retry, return `Ok` with the caller's own error inside `T`
    /// (the transaction then commits whatever it wrote), or `Fatal`.
    pub async fn run<T: Send, F>(&self, opts: TxnOptions, body: F) -> Result<Committed<T>, TxnError>
    where
        F: for<'t> FnMut(&'t mut Txn) -> BoxFuture<'t, Result<T, TxnError>>,
    {
        match self {
            Store::Embedded(h) => h.run(opts, body).await,
            #[cfg(feature = "tikv")]
            Store::Tikv(t) => crate::tikv::run(t, opts, body).await,
        }
    }

    /// The keyspace this store is bound to.
    pub fn keyspace(&self) -> &str {
        match self {
            Store::Embedded(h) => h.keyspace(),
            #[cfg(feature = "tikv")]
            Store::Tikv(t) => t.keyspace(),
        }
    }

    /// The root prefix every key of this store lives under.
    pub fn root(&self) -> &[u8] {
        match self {
            Store::Embedded(h) => h.root(),
            #[cfg(feature = "tikv")]
            Store::Tikv(t) => t.root(),
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
        match self {
            Store::Embedded(h) => Store::Embedded(h.with_faults(plan)),
            #[cfg(feature = "tikv")]
            Store::Tikv(t) => Store::Tikv(t.with_faults(crate::tikv::faults(plan))),
        }
    }

    /// Holds GC at or below `at` for `ttl` (whole seconds, at least 1) under
    /// the service id `loams/<name>` (`name` is `<purpose>/<id>`), and lets
    /// this store's snapshots read at the timestamps it covers while it
    /// lives. Setting the same `name` again moves it. Refused when GC is
    /// already past `at`.
    pub async fn barrier(&self, name: &str, at: Ts, ttl: Duration) -> Result<GcBarrier, KvError> {
        match self {
            Store::Embedded(h) => h.barrier(name, at, ttl).await,
            #[cfg(feature = "tikv")]
            Store::Tikv(t) => crate::tikv::barrier(t, name, at, ttl).await,
        }
    }
}
