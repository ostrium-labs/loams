//! Test support: one test body on every backend (LV1 plan Task 22).
//!
//! [`live_test!`](crate::live_test) expands a body into one `#[tokio::test]`
//! per backend, named `<name>::embedded` and `<name>::tikv`:
//!
//! ```ignore
//! loams_live::live_test!(inserts_are_visible, |store| async move {
//!     let runner = Runner::open(store.store(), &store.live_config("t"))
//!         .await
//!         .expect("a runner");
//!     // …
//! });
//!
//! // Or a named async fn of a `TestStore`, run on every backend:
//! async fn journal_is_dense(store: TestStore) { /* … */ }
//! loams_live::live_test!(journal_is_dense);
//! ```
//!
//! The embedded case always runs, on a fresh store file. The TiKV case runs
//! on `TEST_LIVE` under a fresh root when `LOAMS_TEST_PD` is set, and
//! otherwise returns after a `skipped:` line.

pub mod checker;
pub mod workload;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(feature = "tikv")]
use loams_kv::testing::TEST_LIVE;
use loams_kv::testing::{EMBEDDED_KEYSPACE, TMPDIR_ENV, TempDir, random_root};
use loams_kv::{Backend, EmbeddedConfig, Store, StoreConfig};

use crate::LiveConfig;

/// A test's store on one backend, with the configuration that opens it (so
/// a [`LiveConfig`] or a `LiveServer` can name the same store). Derefs to
/// the [`Store`].
#[derive(Debug, Clone)]
pub struct TestStore {
    store: Store,
    config: StoreConfig,
    /// The embedded store's directory, removed when the last clone goes.
    dir: Option<Arc<TempDir>>,
}

impl std::ops::Deref for TestStore {
    type Target = Store;

    fn deref(&self) -> &Store {
        &self.store
    }
}

fn tmp_base(fallback: Option<&str>) -> PathBuf {
    std::env::var_os(TMPDIR_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| fallback.map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir)
}

impl TestStore {
    /// An embedded store in a fresh file under `LOAMS_TEST_TMPDIR`, else
    /// `base`, else the system temporary directory; on the Live test
    /// keyspace under a fresh root.
    ///
    /// # Panics
    ///
    /// When the directory or the store cannot be created.
    pub async fn embedded(base: Option<&str>) -> Self {
        let dir = Arc::new(TempDir::new_in(&tmp_base(base)).expect("a directory for the store"));
        let config = StoreConfig::Embedded(EmbeddedConfig {
            root: random_root(),
            ..EmbeddedConfig::new(dir.path().join("store.redb"), EMBEDDED_KEYSPACE)
        });
        Self::open(config, Some(dir)).await
    }

    /// A TiKV store on `TEST_LIVE` under a fresh root, or `None` (after a
    /// `skipped:` line) when `LOAMS_TEST_PD` is unset, or in a build without
    /// the `tikv` feature.
    ///
    /// # Panics
    ///
    /// When the variable is set and the cluster does not answer.
    pub async fn tikv() -> Option<Self> {
        #[cfg(feature = "tikv")]
        {
            let cluster = loams_kv::testing::cluster().await?;
            Some(Self::open(StoreConfig::Tikv(cluster.config(TEST_LIVE)), None).await)
        }
        #[cfg(not(feature = "tikv"))]
        {
            eprintln!("skipped: the TiKV case needs loams-live's tikv feature");
            None
        }
    }

    async fn open(config: StoreConfig, dir: Option<Arc<TempDir>>) -> Self {
        let store = Store::open(config.clone())
            .await
            .unwrap_or_else(|e| panic!("the {:?} test store opens: {e}", config.backend()));
        TestStore { store, config, dir }
    }

    /// The store on the same backend, keyspace (and, on embedded, file)
    /// under another fresh root.
    pub async fn fresh_root(&self) -> Self {
        let config = match &self.config {
            StoreConfig::Embedded(c) => StoreConfig::Embedded(EmbeddedConfig {
                root: random_root(),
                ..c.clone()
            }),
            #[cfg(feature = "tikv")]
            StoreConfig::Tikv(c) => {
                let mut c = c.clone();
                c.root = random_root();
                StoreConfig::Tikv(c)
            }
        };
        Self::open(config, self.dir.clone()).await
    }

    /// The store (a cheap clone).
    pub fn store(&self) -> Store {
        self.store.clone()
    }

    /// The configuration that opens this store.
    pub fn store_config(&self) -> StoreConfig {
        self.config.clone()
    }

    /// App `app` on this store, with [`LiveConfig::with_store`]'s defaults.
    pub fn live_config(&self, app: &str) -> LiveConfig {
        LiveConfig::with_store(app, self.store_config())
    }

    /// Which backend this is.
    pub fn backend(&self) -> Backend {
        self.store.backend()
    }

    /// The embedded store's directory, if any.
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref().map(TempDir::path)
    }
}

/// Runs `body` on `store` (the expansion of [`live_test!`](crate::live_test)).
pub async fn run<F, Fut>(store: TestStore, body: F)
where
    F: FnOnce(TestStore) -> Fut,
    Fut: Future<Output = ()>,
{
    body(store).await;
}

/// One `#[tokio::test]` per backend for a test body (LV1 plan Task 22):
/// `live_test!(name, |store| async move { … })`, or `live_test!(name)` for
/// an `async fn name(store: TestStore)` in scope. The cases are
/// `name::embedded` (always) and `name::tikv` (with `LOAMS_TEST_PD`). Doc
/// comments and attributes before `name` go on the case module.
#[macro_export]
macro_rules! live_test {
    ($(#[$meta:meta])* $name:ident) => {
        $crate::live_test!($(#[$meta])* $name, $name);
    };
    ($(#[$meta:meta])* $name:ident, $body:expr $(,)?) => {
        $(#[$meta])*
        mod $name {
            #[allow(unused_imports)]
            use super::*;

            #[::tokio::test(flavor = "multi_thread", worker_threads = 4)]
            async fn embedded() {
                let store = $crate::testing::TestStore::embedded(
                    ::std::option_env!("CARGO_TARGET_TMPDIR"),
                )
                .await;
                $crate::testing::run(store, $body).await;
            }

            #[::tokio::test(flavor = "multi_thread", worker_threads = 4)]
            async fn tikv() {
                let Some(store) = $crate::testing::TestStore::tikv().await else {
                    return;
                };
                $crate::testing::run(store, $body).await;
            }
        }
    };
}
