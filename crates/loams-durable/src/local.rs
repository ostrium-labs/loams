//! `server_local`: the local durable store of `loams dev` and `standalone`.
//!
//! Upstream's blob server (`resonate-server-blob`, the protocol's state
//! machine over conditional-write objects) on [`RedbStore`], a single redb
//! file. It replaces Resonate's SQLite server, which cannot be linked beside
//! Loams's own SQLite (D2450: `resonate-server-sqlite` needs rusqlite 0.31,
//! Loams has 0.32 and sqlx 0.9, and Cargo allows one `links = "sqlite3"`).
//!
//! The kernel, the timers, the schedules and the message delivery are all
//! upstream's; Loams supplies only the [`Store`] port: get, conditional put,
//! unconditional put, delete and an ordered prefix listing. One process owns
//! the file (`durable.lock` beside it, and redb's own lock), so a write
//! transaction is the compare-and-swap. Every write takes the next value of
//! one persisted sequence as its version, so an etag is never reused, even
//! after a delete.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use async_trait::async_trait;
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use resonate_core::router::ResonateRouter;
use resonate_core::types::{RequestEnvelope, ResponseEnvelope};
use resonate_core::{ResonateServer, Unavailable};
use resonate_server_blob::applier::{ApplierCfg, KeySpace};
use resonate_server_blob::kernel::state::KernelCfg;
use resonate_server_blob::server::{Server, ServerCfg};
use resonate_server_blob::store::{Etag, Store, StoreError};

/// The plugin: `[servers.server_local]`.
pub static PLUGIN: resonate_plugin::ServerPlugin =
    resonate_plugin::ServerPlugin::new("server-local", configure);

/// Every object: key to `version (8 bytes, big endian) ++ body`.
const OBJECTS: TableDefinition<&str, &[u8]> = TableDefinition::new("objects");
/// The version sequence.
const META: TableDefinition<&str, u64> = TableDefinition::new("meta");
const SEQ: &str = "seq";

fn configure(
    settings: &resonate_plugin::Settings<'_>,
    deps: resonate_plugin::ServerDependencies,
) -> Result<Arc<dyn ResonateServer>, resonate_plugin::ConfigError> {
    let config: Config = settings.extract()?;
    if config.path.as_os_str().is_empty() {
        return Err(settings.reject("path", "the local store needs a file path"));
    }
    if config.timer_shards == 0 {
        return Err(settings.reject("timer_shards", "must be at least 1 (got 0)"));
    }
    if config.cache_capacity == 0 {
        return Err(settings.reject("cache_capacity", "must be at least 1 (got 0)"));
    }
    Ok(Arc::new(LocalServer::new(config, deps.router)))
}

/// Everything under `[servers.server_local]`: upstream's blob settings that
/// apply to one local file, and the file.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The redb file. Loams sets it from `--durable-store`.
    #[serde(default)]
    pub path: PathBuf,
    #[serde(default = "default_timer_shards")]
    pub timer_shards: u32,
    #[serde(default = "default_cache_capacity")]
    pub cache_capacity: usize,
    #[serde(default = "default_max_cas_retries")]
    pub max_cas_retries: u32,
    #[serde(default = "default_preload_limit")]
    pub preload_limit: u32,
    /// Whether `promise.search` and `schedule.search` are answered. Each
    /// search reads every document; on a local file that is cheap enough, so
    /// it is on (the SQLite store always answered them).
    #[serde(default = "default_search")]
    pub search_enabled: bool,
    #[serde(default)]
    pub server_url: String,
    #[serde(default = "default_retry_timeout")]
    pub retry_timeout: i64,
}

fn default_timer_shards() -> u32 {
    4
}
fn default_cache_capacity() -> usize {
    10_000
}
fn default_max_cas_retries() -> u32 {
    8
}
fn default_preload_limit() -> u32 {
    10
}
fn default_search() -> bool {
    true
}
fn default_retry_timeout() -> i64 {
    60_000
}

/// The plugin's server: the config now, the file and the timer loop at
/// `init` (as upstream's `BlobServer`).
struct LocalServer {
    config: Config,
    router: Arc<dyn ResonateRouter>,
    inner: OnceLock<Arc<Server>>,
    /// The file, closed at `stop`: upstream's origin actors outlive the stop
    /// until they idle out, and a restart in the same process must be able
    /// to open the file again.
    store: OnceLock<RedbStore>,
    shutdown: tokio::sync::watch::Sender<bool>,
    timer: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl LocalServer {
    fn new(config: Config, router: Arc<dyn ResonateRouter>) -> Self {
        Self {
            config,
            router,
            inner: OnceLock::new(),
            store: OnceLock::new(),
            shutdown: tokio::sync::watch::channel(false).0,
            timer: Mutex::new(None),
        }
    }

    fn started(&self) -> Result<&Arc<Server>, Unavailable> {
        self.inner
            .get()
            .ok_or_else(|| Unavailable::new("the local durable store has not been started"))
    }
}

#[async_trait]
impl ResonateServer for LocalServer {
    async fn init(&self, debug: bool) -> Result<(), Unavailable> {
        let path = self.config.path.clone();
        let store = tokio::task::spawn_blocking(move || RedbStore::open(path))
            .await
            .map_err(|e| Unavailable::new(format!("opening the local durable store: {e}")))?
            .map_err(|e| Unavailable::new(e.to_string()))?;
        let _ = self.store.set(store.clone());
        let server = Server::build(
            Arc::new(store),
            Arc::clone(&self.router),
            ServerCfg {
                keys: KeySpace::new(String::new(), self.config.timer_shards),
                applier: ApplierCfg {
                    max_cas_retries: self.config.max_cas_retries,
                    kernel: KernelCfg {
                        retry_timeout: self.config.retry_timeout,
                        preload_limit: self.config.preload_limit,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                timerd: Default::default(),
                cache_capacity: self.config.cache_capacity,
                debug,
                search: self.config.search_enabled,
                server_url: self.config.server_url.clone(),
            },
        );
        // Under the debug flag the clock belongs to the caller: no timer loop.
        if !debug {
            let handle = Arc::clone(server.timerd()).spawn(self.shutdown.subscribe());
            *self.timer.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);
        }
        let _ = self.inner.set(server);
        Ok(())
    }

    async fn stop(&self) -> Result<(), Unavailable> {
        let _ = self.shutdown.send(true);
        let handle = self.timer.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(handle) = handle {
            let _ = handle.await;
        }
        if let Some(store) = self.store.get() {
            store.close().await;
        }
        Ok(())
    }

    async fn process(&self, req: &RequestEnvelope) -> Result<ResponseEnvelope, Unavailable> {
        self.started()?.process(req).await
    }

    async fn ready(&self) -> bool {
        match self.inner.get() {
            Some(server) => server.ready().await,
            None => false,
        }
    }
}

/// [`Store`] on one redb file (module docs).
#[derive(Clone)]
pub struct RedbStore {
    /// `None` once closed: every call is then unavailable.
    db: Arc<RwLock<Option<Arc<Database>>>>,
}

impl std::fmt::Debug for RedbStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedbStore").finish_non_exhaustive()
    }
}

/// What a write requires of the object already there.
enum Expect<'a> {
    Any,
    Absent,
    At(&'a Etag),
}

fn unavailable(e: impl std::fmt::Display) -> StoreError {
    StoreError::Unavailable(e.to_string())
}

fn split(stored: &[u8]) -> Result<(u64, &[u8]), StoreError> {
    if stored.len() < 8 {
        return Err(unavailable("a local durable object is truncated"));
    }
    let (version, body) = stored.split_at(8);
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(version);
    Ok((u64::from_be_bytes(bytes), body))
}

fn etag(version: u64) -> Etag {
    Etag(version.to_string())
}

impl RedbStore {
    /// Open (or create) the file at `path`, creating its directory.
    pub fn open(path: PathBuf) -> Result<Self, StoreError> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(|e| {
                unavailable(format!(
                    "cannot create the durable store directory {}: {e}",
                    dir.display()
                ))
            })?;
        }
        let db = Database::create(&path).map_err(|e| {
            unavailable(format!(
                "cannot open the local durable store {}: {e}",
                path.display()
            ))
        })?;
        // Both tables exist from the first open, so a read never misses one.
        let write = db.begin_write().map_err(unavailable)?;
        write.open_table(OBJECTS).map_err(unavailable)?;
        write.open_table(META).map_err(unavailable)?;
        write.commit().map_err(unavailable)?;
        Ok(Self {
            db: Arc::new(RwLock::new(Some(Arc::new(db)))),
        })
    }

    /// Close the file: later calls are unavailable, and the file is released
    /// once the calls in flight end (waited for, up to 5 s).
    pub async fn close(&self) {
        let taken = self.db.write().unwrap_or_else(|e| e.into_inner()).take();
        let Some(mut db) = taken else {
            return;
        };
        for _ in 0..500 {
            match Arc::try_unwrap(db) {
                Ok(db) => {
                    drop(db);
                    return;
                }
                Err(still) => db = still,
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        tracing::warn!("the local durable store is still in use after stop");
    }

    async fn blocking<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Database) -> Result<T, StoreError> + Send + 'static,
    ) -> Result<T, StoreError> {
        let db = self
            .db
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(|| unavailable("the local durable store is closed"))?;
        tokio::task::spawn_blocking(move || f(&db))
            .await
            .map_err(unavailable)?
    }

    fn write(
        db: &Database,
        key: &str,
        body: &[u8],
        expect: Expect<'_>,
    ) -> Result<Etag, StoreError> {
        let txn = db.begin_write().map_err(unavailable)?;
        let version = {
            let mut objects = txn.open_table(OBJECTS).map_err(unavailable)?;
            let current = match objects.get(key).map_err(unavailable)? {
                Some(stored) => Some(split(stored.value())?.0),
                None => None,
            };
            let ok = match (&expect, current) {
                (Expect::Any, _) => true,
                (Expect::Absent, current) => current.is_none(),
                (Expect::At(want), Some(current)) => etag(current) == **want,
                (Expect::At(_), None) => false,
            };
            if !ok {
                return Err(StoreError::PreconditionFailed);
            }
            let mut meta = txn.open_table(META).map_err(unavailable)?;
            let version = meta.get(SEQ).map_err(unavailable)?.map_or(0, |v| v.value()) + 1;
            meta.insert(SEQ, version).map_err(unavailable)?;
            let mut stored = Vec::with_capacity(8 + body.len());
            stored.extend_from_slice(&version.to_be_bytes());
            stored.extend_from_slice(body);
            objects
                .insert(key, stored.as_slice())
                .map_err(unavailable)?;
            version
        };
        txn.commit().map_err(unavailable)?;
        Ok(etag(version))
    }
}

#[async_trait]
impl Store for RedbStore {
    async fn get(&self, key: &str) -> Result<Option<(Vec<u8>, Etag)>, StoreError> {
        let key = key.to_string();
        self.blocking(move |db| {
            let txn = db.begin_read().map_err(unavailable)?;
            let objects = txn.open_table(OBJECTS).map_err(unavailable)?;
            match objects.get(key.as_str()).map_err(unavailable)? {
                Some(stored) => {
                    let (version, body) = split(stored.value())?;
                    Ok(Some((body.to_vec(), etag(version))))
                }
                None => Ok(None),
            }
        })
        .await
    }

    async fn put_if_match(
        &self,
        key: &str,
        body: Vec<u8>,
        etag: &Etag,
    ) -> Result<Etag, StoreError> {
        let key = key.to_string();
        let etag = etag.clone();
        self.blocking(move |db| Self::write(db, &key, &body, Expect::At(&etag)))
            .await
    }

    async fn put_if_none_match(&self, key: &str, body: Vec<u8>) -> Result<Etag, StoreError> {
        let key = key.to_string();
        self.blocking(move |db| Self::write(db, &key, &body, Expect::Absent))
            .await
    }

    async fn put(&self, key: &str, body: Vec<u8>) -> Result<Etag, StoreError> {
        let key = key.to_string();
        self.blocking(move |db| Self::write(db, &key, &body, Expect::Any))
            .await
    }

    async fn delete(&self, key: &str) -> Result<(), StoreError> {
        let key = key.to_string();
        self.blocking(move |db| {
            let txn = db.begin_write().map_err(unavailable)?;
            {
                let mut objects = txn.open_table(OBJECTS).map_err(unavailable)?;
                objects.remove(key.as_str()).map_err(unavailable)?;
            }
            txn.commit().map_err(unavailable)
        })
        .await
    }

    async fn list(&self, prefix: &str, max_keys: usize) -> Result<Vec<String>, StoreError> {
        let prefix = prefix.to_string();
        self.blocking(move |db| {
            let txn = db.begin_read().map_err(unavailable)?;
            let objects = txn.open_table(OBJECTS).map_err(unavailable)?;
            let mut keys = Vec::new();
            for entry in objects.range(prefix.as_str()..).map_err(unavailable)? {
                if keys.len() >= max_keys {
                    break;
                }
                let (key, _) = entry.map_err(unavailable)?;
                let key = key.value();
                if !key.starts_with(prefix.as_str()) {
                    break;
                }
                keys.push(key.to_string());
            }
            Ok(keys)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, RedbStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = RedbStore::open(dir.path().join("d").join("default.redb")).expect("open");
        (dir, store)
    }

    #[tokio::test]
    async fn conditional_writes_compare_and_swap() {
        let (_dir, s) = store();
        assert_eq!(s.get("wf/a").await.unwrap(), None);
        let v1 = s.put_if_none_match("wf/a", b"one".to_vec()).await.unwrap();
        assert_eq!(
            s.put_if_none_match("wf/a", b"again".to_vec()).await,
            Err(StoreError::PreconditionFailed)
        );
        let v2 = s.put_if_match("wf/a", b"two".to_vec(), &v1).await.unwrap();
        assert_ne!(v1, v2);
        assert_eq!(
            s.put_if_match("wf/a", b"stale".to_vec(), &v1).await,
            Err(StoreError::PreconditionFailed)
        );
        assert_eq!(
            s.get("wf/a").await.unwrap(),
            Some((b"two".to_vec(), v2.clone()))
        );
        // A deleted and recreated object never takes an old version back.
        s.delete("wf/a").await.unwrap();
        s.delete("wf/a").await.unwrap();
        assert_eq!(
            s.put_if_match("wf/a", b"x".to_vec(), &v2).await,
            Err(StoreError::PreconditionFailed)
        );
        let v3 = s
            .put_if_none_match("wf/a", b"three".to_vec())
            .await
            .unwrap();
        assert!(v3 != v1 && v3 != v2);
    }

    #[tokio::test]
    async fn list_is_ordered_bounded_and_prefixed() {
        let (_dir, s) = store();
        for key in ["t/01/3", "t/01/1", "t/02/0", "t/01/2", "wf/x", "t0"] {
            s.put(key, Vec::new()).await.unwrap();
        }
        assert_eq!(
            s.list("t/01/", 10).await.unwrap(),
            ["t/01/1", "t/01/2", "t/01/3"]
        );
        assert_eq!(s.list("t/01/", 2).await.unwrap(), ["t/01/1", "t/01/2"]);
        assert_eq!(s.list("t", 100).await.unwrap().len(), 5);
        s.delete_prefix("t/").await.unwrap();
        assert_eq!(s.list("", 100).await.unwrap(), ["t0", "wf/x"]);
    }

    #[tokio::test]
    async fn objects_survive_a_reopen() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("default.redb");
        let v1 = {
            let s = RedbStore::open(path.clone()).unwrap();
            s.put_if_none_match("wf/a", b"kept".to_vec()).await.unwrap()
        };
        let s = RedbStore::open(path).unwrap();
        assert_eq!(
            s.get("wf/a").await.unwrap(),
            Some((b"kept".to_vec(), v1.clone()))
        );
        let v2 = s.put("wf/b", Vec::new()).await.unwrap();
        assert_ne!(v1, v2, "the sequence carries over");
    }

    #[test]
    fn its_id_is_server_local() {
        assert_eq!(PLUGIN.id(), "server_local");
    }
}
