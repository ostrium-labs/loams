//! The deployment CA on this server (design §83 §6.2–§6.3, CT1 Task 3;
//! feature `transport`).
//!
//! - The CA document ([`loams_ca::doc::CaDoc`]) lives in the metastore and
//!   the bucket, the way the principal directory and the meter's documents
//!   do: a versioned pointer (`ca-state` in the system namespace) to a JSON
//!   object under `system/ca/state/` ([`MetaCaDoc`]).
//! - The intermediate's key in it is sealed with age to the key file in
//!   `LOAMS_CA_SEALING_KEY_FILE` (default `<data-dir>/ca/sealing.key`,
//!   [`sealing_key_file`]), which `loams ca init` creates and every `meta`
//!   node needs a copy of.
//! - `meta` nodes serve `loams.ca.internal.v1.CertificateService` on the
//!   cluster listener ([`routes`]).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use loams_ca::{Authority, CaDocStore, KeySealer};
use loams_common::meta::MetaStore;
use loams_store::Store;

use crate::auth::directory::{DirectoryStore, MetaDirectoryStore};

/// The metastore pointer of the CA document, in the system namespace.
pub const POINTER: &str = "ca-state";
/// The bucket prefix of the CA document's objects.
pub const PREFIX: &str = "system/ca/state/";
/// The environment variable naming the sealing key file.
pub const SEALING_KEY_ENV: &str = "LOAMS_CA_SEALING_KEY_FILE";

/// The CA document in the metastore and the bucket.
#[derive(Debug)]
pub struct MetaCaDoc(MetaDirectoryStore);

impl MetaCaDoc {
    pub fn new(meta: Arc<dyn MetaStore>, store: Store) -> Self {
        MetaCaDoc(MetaDirectoryStore::with_key(meta, store, POINTER, PREFIX))
    }
}

#[async_trait]
impl CaDocStore for MetaCaDoc {
    async fn load(&self) -> Result<Option<(u64, Vec<u8>)>, String> {
        Ok(self
            .0
            .load()
            .await
            .map_err(|e| e.0)?
            .map(|(v, b)| (v, b.to_vec())))
    }

    async fn save(&self, expected: Option<u64>, doc: Vec<u8>) -> Result<Option<u64>, String> {
        self.0
            .save(expected, Bytes::from(doc))
            .await
            .map_err(|e| e.0)
    }
}

/// The sealing key file: `explicit`, else `LOAMS_CA_SEALING_KEY_FILE`, else
/// `<data_dir>/ca/sealing.key`.
pub fn sealing_key_file(explicit: Option<&Path>, data_dir: &Path) -> PathBuf {
    explicit
        .map(Path::to_path_buf)
        .or_else(|| {
            std::env::var_os(SEALING_KEY_ENV)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| data_dir.join("ca").join("sealing.key"))
}

/// The CA of a `meta` node: the document in `meta` and `store`, the
/// intermediate's key opened with the existing key file. A node without the
/// key file still serves `TrustBundle`; `Issue` and `Renew` answer
/// `ca_not_initialized` there.
pub fn authority(meta: Arc<dyn MetaStore>, store: Store, data_dir: &Path) -> Authority {
    let key_file = sealing_key_file(None, data_dir);
    let sealer = KeySealer::open_file(&key_file);
    if let Err(err) = &sealer {
        tracing::info!(%err, "the CA's sealing key is not readable: this node does not issue");
    }
    Authority::new(Arc::new(MetaCaDoc::new(meta, store)), sealer)
}

/// `loams.ca.internal.v1.CertificateService` as axum routes, for the
/// cluster listener of a `meta` node. Until CT1 Task 6 puts that listener on
/// mTLS, nothing inserts a verified peer, so `Renew` answers
/// `unauthenticated`; `Issue` takes a join token and `TrustBundle` is public.
pub fn routes(authority: Arc<Authority>) -> axum::Router {
    let rpc = loams_ca::connect::register(connectrpc::Router::new(), authority);
    let paths: Vec<String> = rpc.methods().map(|path| format!("/{path}")).collect();
    let service = rpc.into_axum_service();
    let mut router = axum::Router::new();
    for path in paths {
        router = router.route_service(&path, service.clone());
    }
    router
}
