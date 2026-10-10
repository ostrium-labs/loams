//! The local backend (single-node mode, §46 §18.1): `loams-kv`'s embedded
//! store, MVCC on one redb file under the engine's data directory.

use std::path::Path;

use loams_kv::{EmbeddedConfig, Store, StoreConfig};

use super::{KvControlStore, StoreError, StoreOptions};

/// The keyspace of the local store's file.
pub const LOCAL_KEYSPACE: &str = "loams_pg";

/// Opens (or creates) the local store in the redb file `path`, on
/// [`LOCAL_KEYSPACE`] with an empty root.
///
/// # Errors
///
/// `Unavailable` when the file cannot be opened.
pub async fn open(
    path: impl AsRef<Path>,
    options: StoreOptions,
) -> Result<KvControlStore, StoreError> {
    let config = EmbeddedConfig::new(path.as_ref(), LOCAL_KEYSPACE);
    let store = Store::open(StoreConfig::Embedded(config))
        .await
        .map_err(|e| StoreError::Unavailable(e.to_string()))?;
    Ok(KvControlStore::new(store, options))
}
