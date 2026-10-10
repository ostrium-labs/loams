//! The TiKV backend (production; feature `tikv`): a `loams-tikv` handle on
//! the metastore's keyspace and under its root, so `pg-control`'s keys sit
//! beside `loams-meta-tikv`'s (§46 §6.2), whose tags they never share.

use loams_kv::{Store, StoreConfig, TikvConfig};

use super::{KvControlStore, StoreError, StoreOptions};

/// Connects with `config`: the metastore's own (`loams_meta` and its root).
///
/// # Errors
///
/// `Unavailable` when the cluster does not answer, or the keyspace is
/// missing.
pub async fn open(config: TikvConfig, options: StoreOptions) -> Result<KvControlStore, StoreError> {
    Ok(KvControlStore::new(open_store(config).await?, options))
}

/// The `loams-kv` store under [`open`].
///
/// # Errors
///
/// As [`open`].
pub async fn open_store(config: TikvConfig) -> Result<Store, StoreError> {
    Store::open(StoreConfig::Tikv(config))
        .await
        .map_err(|e| StoreError::Unavailable(e.to_string()))
}
