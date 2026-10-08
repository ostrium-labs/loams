//! The stores a test runs on (LV1 plan Global Constraints).
//!
//! [`stores`] yields the embedded store always (from Task 21) and a TiKV
//! store when `LOAMS_TEST_PD` names a test cluster (feature `tikv`), so a
//! plain `cargo test` covers the embedded backend and CI's `tikv` job both.
//! Until Task 21 it yields only the TiKV store, and nothing (after a
//! `skipped:` line) without a cluster. TiKV-only tests take [`tikv`], or
//! [`cluster`] for a configuration.

use crate::Store;

/// The cluster harness of `loams-tikv`: the test cluster, its keyspaces and
/// random roots.
#[cfg(feature = "tikv")]
pub use loams_tikv::testing::{
    PD_ENV, ROOT_LEN, TEST_KEYSPACES, TEST_LIVE, TEST_META, TEST_SQL, TestCluster, cluster,
    random_root,
};

/// One store per backend for the test `name`, each under a fresh root: the
/// embedded store (from Task 21), then the TiKV store on [`TEST_LIVE`] when
/// `LOAMS_TEST_PD` is set.
pub async fn stores(name: &str) -> Vec<Store> {
    #[allow(unused_mut)]
    let mut stores = Vec::new();
    #[cfg(feature = "tikv")]
    if let Some(store) = tikv().await {
        stores.push(store);
    }
    tracing::debug!(test = name, stores = stores.len(), "the test's stores");
    stores
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
