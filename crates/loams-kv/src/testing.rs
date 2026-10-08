//! The stores a test runs on (LV1 plan Global Constraints).
//!
//! [`stores`] yields the embedded store always and a TiKV store when
//! `LOAMS_TEST_PD` names a test cluster (feature `tikv`), so a plain
//! `cargo test` covers the embedded backend and CI's `tikv` job both.
//! TiKV-only tests take [`tikv`], or [`cluster`] for a configuration.
//! [`Factory`] opens stores of one backend for the conformance cases of
//! [`kv_conformance!`](crate::kv_conformance).
//!
//! Embedded stores live in a fresh directory under `LOAMS_TEST_TMPDIR`
//! ([`TMPDIR_ENV`]), or the system temporary directory when it is unset,
//! removed when the store (or the factory) is dropped.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::future::BoxFuture;
use rand::RngCore;

use crate::{Backend, EmbeddedConfig, Store, StoreConfig};

/// The cluster harness of `loams-tikv`: the test cluster and its keyspaces.
#[cfg(feature = "tikv")]
pub use loams_tikv::testing::{
    PD_ENV, TEST_KEYSPACES, TEST_LIVE, TEST_META, TEST_SQL, TestCluster, cluster,
};

/// The base directory of embedded test stores (default: the system
/// temporary directory).
pub const TMPDIR_ENV: &str = "LOAMS_TEST_TMPDIR";

/// The keyspace embedded test stores open (the name of TiKV's Live test
/// keyspace).
pub const EMBEDDED_KEYSPACE: &str = "loams_test_live";

/// The length of a test's random root prefix.
pub const ROOT_LEN: usize = 8;

/// A fresh random root prefix of [`ROOT_LEN`] bytes.
pub fn random_root() -> Vec<u8> {
    let mut root = vec![0u8; ROOT_LEN];
    rand::rng().fill_bytes(&mut root);
    root
}

/// One store per backend for the test `name`, each under a fresh root: the
/// embedded store, then the TiKV store on `TEST_LIVE` when `LOAMS_TEST_PD`
/// is set.
///
/// # Panics
///
/// When the embedded store cannot be opened, or `LOAMS_TEST_PD` is set and
/// the cluster does not answer.
pub async fn stores(name: &str) -> Vec<Store> {
    #[allow(unused_mut)]
    let mut stores = vec![embedded().await];
    #[cfg(feature = "tikv")]
    if let Some(store) = tikv().await {
        stores.push(store);
    }
    tracing::debug!(test = name, stores = stores.len(), "the test's stores");
    stores
}

/// An embedded store in a fresh directory (removed when the last handle on
/// it is dropped), on [`EMBEDDED_KEYSPACE`] under a fresh random root.
///
/// # Panics
///
/// When the directory or the store cannot be created.
pub async fn embedded() -> Store {
    let dir = TempDir::new_in(&tmp_base()).expect("a temporary directory for the store");
    let config = EmbeddedConfig {
        root: random_root(),
        ..EmbeddedConfig::new(dir.path().join("store.redb"), EMBEDDED_KEYSPACE)
    };
    let handle = crate::embedded::Handle::open_owning(config, dir)
        .await
        .expect("an embedded test store");
    Store::Embedded(handle)
}

/// A TiKV store on [`TEST_LIVE`] under a fresh random root, or `None` (and a
/// `skipped:` line naming the test) when `LOAMS_TEST_PD` is unset.
///
/// # Panics
///
/// When the variable is set and the cluster does not answer or the connect
/// fails.
#[cfg(feature = "tikv")]
pub async fn tikv() -> Option<Store> {
    let cluster = cluster().await?;
    Some(Store::Tikv(cluster.connect(TEST_LIVE).await))
}

/// What a conformance case asks a [`Factory`] for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    /// The store's root prefix.
    pub root: Vec<u8>,
    /// The GC life time, when the case needs its own.
    pub gc_life_time: Option<Duration>,
}

impl Spec {
    /// A fresh random root and the backend's default GC life time.
    pub fn fresh() -> Self {
        Spec {
            root: random_root(),
            gc_life_time: None,
        }
    }
}

type Open = dyn Fn(Spec) -> BoxFuture<'static, Option<Store>> + Send + Sync;

/// Opens stores of one backend for one conformance case: every store of a
/// factory shares the backend's keyspace (and, on embedded, one file), each
/// under the root its [`Spec`] names.
#[derive(Clone)]
pub struct Factory {
    backend: Backend,
    open: Arc<Open>,
}

impl std::fmt::Debug for Factory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Factory")
            .field("backend", &self.backend)
            .finish_non_exhaustive()
    }
}

impl Factory {
    /// A factory of `backend` stores from `open`, which yields `None` (after
    /// a `skipped:` line) when the backend is unavailable.
    pub fn new(
        backend: Backend,
        open: impl Fn(Spec) -> BoxFuture<'static, Option<Store>> + Send + Sync + 'static,
    ) -> Self {
        Factory {
            backend,
            open: Arc::new(open),
        }
    }

    /// The backend this factory opens.
    pub fn backend(&self) -> Backend {
        self.backend
    }

    /// A store as `spec` asks, or `None` when the backend is unavailable.
    pub async fn store(&self, spec: Spec) -> Option<Store> {
        (self.open)(spec).await
    }
}

/// Embedded stores in one fresh redb file in a new directory under `base`
/// (removed when the factory and its stores are dropped), on
/// [`EMBEDDED_KEYSPACE`].
///
/// # Panics
///
/// When the directory cannot be created, or (when a case asks) a store
/// cannot be opened.
pub fn embedded_factory(base: impl AsRef<Path>) -> Factory {
    let dir = Arc::new(TempDir::new_in(base.as_ref()).expect("a temporary directory"));
    Factory::new(Backend::Embedded, move |spec| {
        let dir = dir.clone();
        Box::pin(async move {
            let mut config = EmbeddedConfig {
                root: spec.root,
                ..EmbeddedConfig::new(dir.path().join("store.redb"), EMBEDDED_KEYSPACE)
            };
            if let Some(life) = spec.gc_life_time {
                config.gc_life_time = life;
            }
            Some(
                Store::open(StoreConfig::Embedded(config))
                    .await
                    .expect("an embedded store"),
            )
        })
    })
}

/// TiKV stores on [`TEST_LIVE`], or `None` (and a `skipped:` line) when
/// `LOAMS_TEST_PD` is unset.
#[cfg(feature = "tikv")]
pub fn tikv_factory() -> Factory {
    Factory::new(Backend::Tikv, |spec| {
        Box::pin(async move {
            let cluster = cluster().await?;
            let mut config = cluster.config(TEST_LIVE);
            config.root = spec.root;
            if let Some(life) = spec.gc_life_time {
                config.gc_life_time = life;
            }
            Some(
                Store::open(StoreConfig::Tikv(config))
                    .await
                    .expect("a TiKV store"),
            )
        })
    })
}

fn tmp_base() -> PathBuf {
    std::env::var_os(TMPDIR_ENV)
        .filter(|v| !v.is_empty())
        .map_or_else(std::env::temp_dir, PathBuf::from)
}

/// A directory removed (with everything in it) when dropped.
#[derive(Debug)]
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// A new, empty directory under `base` (created if needed).
    ///
    /// # Errors
    ///
    /// When the directory cannot be created.
    pub fn new_in(base: &Path) -> std::io::Result<Self> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = base.join(format!(
            "loams-kv-{}-{nanos}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path)?;
        Ok(TempDir { path })
    }

    /// The directory.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
