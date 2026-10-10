//! The embedded backend: MVCC on redb (LV1 plan Task 21, Ruling 4).
//!
//! One redb file holds every keyspace of a data directory. Its tables:
//!
//! - `versions`: `keyspace_id:u32 BE ‖ esc(root ‖ key) ‖ (u64::MAX −
//!   commit_ts) BE → [0x00 ‖ value] | [0x01]` (a tombstone; see
//!   [`mvcc`](self) for `esc`, row T21-1);
//! - `keyspaces`: `name → id`;
//! - `oracle`: `high_water` (the timestamp oracle's persisted mark) and
//!   `gc_safe_point`.
//!
//! redb locks its file, so a process opens each file once: a registry keyed
//! by the canonical path hands every [`Handle`] on a file the same database,
//! timestamp oracle, committer thread and GC thread, closed when the last
//! handle is dropped. A [`Handle`] is a keyspace and a root on it.
//!
//! Reads at a timestamp see the newest version at or below it.
//! Transactions buffer their writes; the committer applies them under
//! snapshot isolation with first-committer-wins, many commits per redb
//! write transaction (group commit). GC drops versions no read at or above
//! the safe point sees.

mod commit;
#[cfg(feature = "faults")]
mod faulty;
mod gc;
mod mvcc;
mod oracle;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use redb::{Database, DatabaseError, ReadableDatabase, ReadableTable, TableDefinition};

pub use gc::GcReport;
pub use mvcc::{Snap, Txn};

use self::commit::{Mutation, Request};
use self::gc::{GcState, OpenGuard};
use self::oracle::Oracle;
use crate::gc::Inner;
use crate::testing::TempDir;
use crate::{
    Committed, EmbeddedConfig, Fault, FaultPlan, FaultPoint, GcBarrier, KvError, Ts, TxnError,
    TxnOptions,
};

/// The GC life time of [`EmbeddedConfig::new`] (TiKV's default).
pub const DEFAULT_GC_LIFE_TIME: Duration = Duration::from_secs(10 * 60);
/// How often GC runs by default.
pub const DEFAULT_GC_INTERVAL: Duration = Duration::from_secs(60);
/// Reads stay inside `gc_life_time − GC_SAFE_MARGIN` (TiKV's window).
const GC_SAFE_MARGIN: Duration = Duration::from_secs(60);

const VERSIONS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("versions");
const KEYSPACES: TableDefinition<&str, u32> = TableDefinition::new("keyspaces");
const ORACLE: TableDefinition<&str, u64> = TableDefinition::new("oracle");
const HIGH_WATER: &str = "high_water";
const GC_SAFE_POINT: &str = "gc_safe_point";

/// How long opening waits for this process's previous database on the file
/// to close (its last reads, mark writes or GC round finishing).
const CLOSE_WAIT: Duration = Duration::from_secs(30);
/// How long opening retries a file still locked by this process's previous
/// database, which closed a moment ago.
const UNLOCK_WAIT: Duration = Duration::from_millis(500);

fn storage(e: impl std::fmt::Display) -> KvError {
    KvError::Embedded(e.to_string())
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// A monotonic counter.
#[derive(Debug, Default)]
struct Counter(AtomicU64);

impl Counter {
    fn inc(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    fn add(&self, n: impl TryInto<u64>) {
        self.0
            .fetch_add(n.try_into().unwrap_or(u64::MAX), Ordering::Relaxed);
    }

    fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Debug, Default)]
struct Counters {
    write_transactions: Counter,
    commits: Counter,
    conflicts: Counter,
    restarts: Counter,
    unknown_outcomes: Counter,
    gc_runs: Counter,
    versions_deleted: Counter,
    tokens_swept: Counter,
    scan_entries: Counter,
}

/// The counters of a store file (all its handles).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EmbeddedStats {
    /// redb write transactions committed: one per commit group, oracle
    /// mark, GC round or new keyspace.
    pub write_transactions: u64,
    /// Transactions committed.
    pub commits: u64,
    /// Commits refused with a conflict.
    pub conflicts: u64,
    /// Transactions the runner restarted.
    pub restarts: u64,
    /// Commits whose outcome was unknown (faults, failed writes).
    pub unknown_outcomes: u64,
    /// GC rounds.
    pub gc_runs: u64,
    /// Versions GC deleted.
    pub versions_deleted: u64,
    /// Expired commit tokens GC deleted.
    pub tokens_swept: u64,
    /// Table entries scans read.
    pub scan_entries: u64,
}

/// What every handle on a file and the committer thread share.
pub(crate) struct Core {
    db: Database,
    oracle: Oracle,
    gc: Mutex<GcState>,
    gc_tuning: gc::GcTuning,
    counters: Counters,
    #[cfg(feature = "faults")]
    io_faults: Arc<faulty::Switch>,
}

impl std::fmt::Debug for Core {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Core")
            .field("oracle", &self.oracle)
            .finish_non_exhaustive()
    }
}

impl Core {
    fn gc_state(&self) -> MutexGuard<'_, GcState> {
        self.gc.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn covered(&self, at: Ts) -> bool {
        self.gc_state().covers(at)
    }

    /// Whether GC is past `at`: what only reads at `at` saw may be gone.
    fn collected(&self, at: Ts) -> bool {
        self.gc_state().safe_point > at
    }

    /// Persists the oracle's mark at `mark` or above (blocking).
    fn persist_mark(&self, mark: Ts) -> Result<(), redb::Error> {
        let _one = self
            .oracle
            .persist
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if self.oracle.durable() >= mark {
            return Ok(());
        }
        let write = self.db.begin_write()?;
        let written = {
            let mut table = write.open_table(ORACLE)?;
            let stored = table.get(HIGH_WATER)?.map_or(0, |g| g.value());
            let next = stored.max(mark.0);
            table.insert(HIGH_WATER, next)?;
            next
        };
        write.commit()?;
        self.counters.write_transactions.inc();
        self.oracle.persisted(Ts(written));
        Ok(())
    }

    /// A fresh timestamp (blocking).
    fn now_blocking(&self) -> Result<Ts, KvError> {
        loop {
            match self.oracle.try_now() {
                Ok(ts) => return Ok(ts),
                Err(next) => self.persist_mark(Oracle::mark_for(next)).map_err(storage)?,
            }
        }
    }

    /// Persists a mark above `at` (blocking) when the oracle's is not.
    fn mark_above_blocking(&self, at: Ts) -> Result<(), KvError> {
        if at >= self.oracle.durable() {
            self.persist_mark(Oracle::mark_for(at)).map_err(storage)?;
        }
        Ok(())
    }
}

/// A store file open in this process.
struct Shared {
    path: PathBuf,
    core: Arc<Core>,
    sender: Option<Sender<Request>>,
    committer: Option<JoinHandle<()>>,
    /// Every (keyspace id, root) opened on the file: GC sweeps their commit
    /// tokens.
    roots: Mutex<BTreeSet<(u32, Vec<u8>)>>,
    /// The longest GC life time any handle asked for.
    gc_life_time: Mutex<Duration>,
    /// Directories removed once the file is closed (test stores).
    owned: Mutex<Vec<TempDir>>,
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shared")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        drop(self.sender.take());
        if let Some(committer) = self.committer.take() {
            let _ = committer.join();
        }
    }
}

/// A store file this process opened: its handles' shared state, and the
/// database itself, which outlives the last handle by a moment while its
/// last users (a mark write, a GC round) finish.
struct Opened {
    shared: Weak<Shared>,
    core: Weak<Core>,
}

/// The open store files of this process, by canonical path.
static REGISTRY: LazyLock<Mutex<HashMap<PathBuf, Opened>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn registry() -> MutexGuard<'static, HashMap<PathBuf, Opened>> {
    REGISTRY.lock().unwrap_or_else(|e| e.into_inner())
}

/// The registry key of `path`: the canonical path of the file when it
/// exists (a symlinked file is the file it names, review fix 8), else its
/// canonical directory and file name.
fn canonical(path: &Path) -> std::io::Result<PathBuf> {
    if let Ok(file) = path.canonicalize() {
        return Ok(file);
    }
    let name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the store path names no file",
        )
    })?;
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    Ok(dir.canonicalize()?.join(name))
}

/// Whether this process has the store file at `path` open.
pub fn is_open(path: &Path) -> bool {
    canonical(path).is_ok_and(|key| {
        registry()
            .get(&key)
            .is_some_and(|o| o.shared.strong_count() > 0)
    })
}

/// Opens the redb file; with the `faults` feature through a backend whose
/// syncs a test can fail.
fn create_db(
    path: &Path,
    #[cfg(feature = "faults")] io_faults: &Arc<faulty::Switch>,
) -> Result<Database, DatabaseError> {
    #[cfg(feature = "faults")]
    return faulty::create(path, io_faults.clone());
    #[cfg(not(feature = "faults"))]
    Database::create(path)
}

/// Opens the file; `closing` is this process's previous database on it,
/// which is waited for (up to [`CLOSE_WAIT`]) rather than reported as
/// another process's lock.
fn create(
    path: &Path,
    closing: Option<Weak<Core>>,
    #[cfg(feature = "faults")] io_faults: &Arc<faulty::Switch>,
) -> Result<Database, KvError> {
    let ours = closing.is_some();
    if let Some(core) = closing {
        let give_up = Instant::now() + CLOSE_WAIT;
        while core.strong_count() > 0 {
            if Instant::now() >= give_up {
                return Err(KvError::Embedded(format!(
                    "the store file {} is still closing in this process after {CLOSE_WAIT:?}",
                    path.display()
                )));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let give_up = Instant::now() + UNLOCK_WAIT;
    loop {
        match create_db(
            path,
            #[cfg(feature = "faults")]
            io_faults,
        ) {
            Ok(db) => return Ok(db),
            // This process's previous database released its last reference
            // a moment ago and is unlocking the file.
            Err(DatabaseError::DatabaseAlreadyOpen) if ours && Instant::now() < give_up => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(DatabaseError::DatabaseAlreadyOpen) => {
                return Err(KvError::Embedded(format!(
                    "the store file {} is open in another process",
                    path.display()
                )));
            }
            Err(e) => {
                return Err(KvError::Embedded(format!(
                    "opening the store file {}: {e}",
                    path.display()
                )));
            }
        }
    }
}

impl Shared {
    /// The open file at `config.path`, opening it when this process has
    /// not.
    fn get_or_open(config: &EmbeddedConfig, owned: Option<TempDir>) -> Result<Arc<Self>, KvError> {
        if let Some(dir) = config.path.parent()
            && !dir.as_os_str().is_empty()
        {
            std::fs::create_dir_all(dir)
                .map_err(|e| KvError::Embedded(format!("creating {}: {e}", dir.display())))?;
        }
        let key = canonical(&config.path)
            .map_err(|e| KvError::Embedded(format!("{}: {e}", config.path.display())))?;
        let mut open = registry();
        let closing = match open.get(&key) {
            Some(o) => {
                if let Some(shared) = o.shared.upgrade() {
                    if let Some(dir) = owned {
                        shared.owned_dirs().push(dir);
                    }
                    return Ok(shared);
                }
                Some(o.core.clone())
            }
            None => None,
        };
        #[cfg(feature = "faults")]
        let io_faults = Arc::new(faulty::Switch::default());
        let db = create(
            &key,
            closing,
            #[cfg(feature = "faults")]
            &io_faults,
        )?;
        let (high_water, safe_point) = (|| -> Result<(u64, u64), redb::Error> {
            let write = db.begin_write()?;
            let marks = {
                write.open_table(VERSIONS)?;
                write.open_table(KEYSPACES)?;
                let oracle = write.open_table(ORACLE)?;
                let hw = oracle.get(HIGH_WATER)?.map_or(0, |g| g.value());
                let sp = oracle.get(GC_SAFE_POINT)?.map_or(0, |g| g.value());
                (hw, sp)
            };
            write.commit()?;
            Ok(marks)
        })()
        .map_err(storage)?;
        let core = Arc::new(Core {
            db,
            oracle: Oracle::new(Ts(high_water)),
            gc: Mutex::new(GcState::new(Ts(safe_point))),
            gc_tuning: gc::GcTuning::default(),
            counters: Counters::default(),
            #[cfg(feature = "faults")]
            io_faults,
        });
        core.counters.write_transactions.inc();
        let (sender, requests) = mpsc::channel();
        let committer = std::thread::Builder::new()
            .name("loams-kv-commit".into())
            .spawn({
                let core = core.clone();
                move || commit::committer(core, requests)
            })
            .map_err(|e| KvError::Embedded(format!("starting the committer: {e}")))?;
        let shared = Arc::new(Shared {
            path: key.clone(),
            core,
            sender: Some(sender),
            committer: Some(committer),
            roots: Mutex::new(BTreeSet::new()),
            gc_life_time: Mutex::new(config.gc_life_time),
            owned: Mutex::new(owned.into_iter().collect()),
        });
        gc::spawn(Arc::downgrade(&shared), config.gc_interval);
        open.retain(|_, o| o.shared.strong_count() > 0 || o.core.strong_count() > 0);
        open.insert(
            key,
            Opened {
                shared: Arc::downgrade(&shared),
                core: Arc::downgrade(&shared.core),
            },
        );
        Ok(shared)
    }

    fn owned_dirs(&self) -> MutexGuard<'_, Vec<TempDir>> {
        self.owned.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn roots(&self) -> Vec<(u32, Vec<u8>)> {
        self.roots
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
    }

    fn gc_life_time(&self) -> Duration {
        *self.gc_life_time.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The id of `keyspace`, assigned on first use.
    fn keyspace_id(&self, keyspace: &str) -> Result<u32, KvError> {
        let db = &self.core.db;
        {
            let read = db.begin_read().map_err(storage)?;
            let table = read.open_table(KEYSPACES).map_err(storage)?;
            if let Some(id) = table.get(keyspace).map_err(storage)? {
                return Ok(id.value());
            }
        }
        let id = (|| -> Result<u32, redb::Error> {
            let write = db.begin_write()?;
            let id = {
                let mut table = write.open_table(KEYSPACES)?;
                if let Some(id) = table.get(keyspace)? {
                    id.value()
                } else {
                    let mut next = 1;
                    for entry in table.iter()? {
                        let (_, id) = entry?;
                        next = next.max(id.value().saturating_add(1));
                    }
                    table.insert(keyspace, next)?;
                    next
                }
            };
            write.commit()?;
            Ok(id)
        })()
        .map_err(storage)?;
        self.core.counters.write_transactions.inc();
        Ok(id)
    }
}

/// A store on an embedded file: one keyspace, under one root. Cheap to
/// clone; the file closes when the last handle on it is dropped.
#[derive(Clone)]
pub struct Handle {
    shared: Arc<Shared>,
    keyspace: Arc<str>,
    ks_id: u32,
    root: Arc<[u8]>,
    gc_life_time: Duration,
    faults: Option<Arc<dyn FaultPlan>>,
}

impl std::fmt::Debug for Handle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Handle")
            .field("path", &self.shared.path)
            .field("keyspace", &self.keyspace)
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl Handle {
    /// Opens (or creates) the store at `config.path`, the keyspace
    /// `config.keyspace` in it (created on first use) and the root
    /// `config.root`. Refuses a `gc_life_time` of one minute or less (the
    /// read window is a minute shorter), a zero `gc_interval` and an empty
    /// keyspace name.
    pub async fn open(config: EmbeddedConfig) -> Result<Handle, KvError> {
        Self::open_with(config, None).await
    }

    /// [`open`](Self::open), removing `dir` once the file is closed.
    pub(crate) async fn open_owning(
        config: EmbeddedConfig,
        dir: TempDir,
    ) -> Result<Handle, KvError> {
        Self::open_with(config, Some(dir)).await
    }

    async fn open_with(config: EmbeddedConfig, owned: Option<TempDir>) -> Result<Handle, KvError> {
        if config.gc_life_time <= GC_SAFE_MARGIN {
            return Err(KvError::Embedded(
                "EmbeddedConfig.gc_life_time must exceed one minute".into(),
            ));
        }
        if config.gc_interval.is_zero() {
            return Err(KvError::Embedded(
                "EmbeddedConfig.gc_interval must be positive".into(),
            ));
        }
        if config.keyspace.is_empty() {
            return Err(KvError::Embedded(
                "EmbeddedConfig.keyspace must not be empty".into(),
            ));
        }
        tokio::task::spawn_blocking(move || {
            let shared = Shared::get_or_open(&config, owned)?;
            let ks_id = shared.keyspace_id(&config.keyspace)?;
            shared
                .roots
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert((ks_id, config.root.clone()));
            {
                let mut life = shared
                    .gc_life_time
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                *life = (*life).max(config.gc_life_time);
            }
            Ok(Handle {
                shared,
                keyspace: config.keyspace.into(),
                ks_id,
                root: config.root.into(),
                gc_life_time: config.gc_life_time,
                faults: None,
            })
        })
        .await
        .map_err(|e| KvError::Embedded(format!("opening the store: {e}")))?
    }

    /// A handle on another keyspace id and root of the same file (GC).
    fn scoped(shared: Arc<Shared>, ks_id: u32, root: Vec<u8>) -> Handle {
        let gc_life_time = shared.gc_life_time();
        Handle {
            shared,
            keyspace: Arc::from(""),
            ks_id,
            root: root.into(),
            gc_life_time,
            faults: None,
        }
    }

    /// The keyspace this handle is bound to.
    pub fn keyspace(&self) -> &str {
        &self.keyspace
    }

    /// The root prefix every key of this handle lives under.
    pub fn root(&self) -> &[u8] {
        &self.root
    }

    /// The counters of the store file.
    pub fn stats(&self) -> EmbeddedStats {
        let c = &self.shared.core.counters;
        EmbeddedStats {
            write_transactions: c.write_transactions.get(),
            commits: c.commits.get(),
            conflicts: c.conflicts.get(),
            restarts: c.restarts.get(),
            unknown_outcomes: c.unknown_outcomes.get(),
            gc_runs: c.gc_runs.get(),
            versions_deleted: c.versions_deleted.get(),
            tokens_swept: c.tokens_swept.get(),
            scan_entries: c.scan_entries.get(),
        }
    }

    /// The version prefix of `key` under this handle's root.
    fn prefix(&self, key: &[u8]) -> Vec<u8> {
        mvcc::prefix(self.ks_id, &self.root, key)
    }

    /// How long reads stay inside the GC safe window.
    fn safe_window(&self) -> Duration {
        self.gc_life_time.saturating_sub(GC_SAFE_MARGIN)
    }

    /// A fresh timestamp from the store's oracle.
    pub async fn now(&self) -> Result<Ts, KvError> {
        loop {
            match self.shared.core.oracle.try_now() {
                Ok(ts) => return Ok(ts),
                Err(next) => {
                    let core = self.shared.core.clone();
                    tokio::task::spawn_blocking(move || {
                        core.persist_mark(Oracle::mark_for(next)).map_err(storage)
                    })
                    .await
                    .map_err(storage)??;
                }
            }
        }
    }

    /// A read-only view at `at`. Refused with [`KvError::GcSafePoint`] when
    /// `at` is older than `now − (gc_life_time − 1 min)`, unless a
    /// [`GcBarrier`] covers it, and whenever GC is past `at`. Refused with
    /// [`KvError::TsAhead`] when `at` is more than a second past the later
    /// of the last timestamp issued and the wall clock (row T21-14). A view
    /// above every timestamp issued moves the oracle past it, so later
    /// commits stay invisible to it.
    pub async fn snapshot(&self, at: Ts) -> Result<Snap, KvError> {
        let core = self.shared.core.clone();
        let limit = core.oracle.read_limit();
        if at > limit {
            return Err(KvError::TsAhead {
                at: at.0,
                limit: limit.0,
            });
        }
        core.oracle.observe(at);
        if at >= core.oracle.durable() {
            let blocking = core.clone();
            tokio::task::spawn_blocking(move || blocking.mark_above_blocking(at))
                .await
                .map_err(storage)??;
        }
        core.oracle.wait_visible(at).await;
        let now_ms = oracle::wall_ms();
        let floor_ms = now_ms.saturating_sub(millis(self.safe_window()));
        let at_ms = at.physical_ms();
        let open = OpenGuard::open(&core, at, |gc| {
            if at_ms < floor_ms && !gc.covers(at) {
                return Err(KvError::GcSafePoint {
                    at: at.0,
                    safe_point: Ts::from_parts(floor_ms, 0).0,
                });
            }
            Ok(())
        })?;
        let left = Duration::from_millis(at_ms.saturating_sub(floor_ms));
        Ok(Snap::new(self.clone(), at, Instant::now() + left, open))
    }

    /// A new transaction at a fresh start timestamp, without the runner:
    /// the caller commits it with [`Txn::commit`] (or drops it).
    pub async fn begin(&self) -> Result<Txn, TxnError> {
        self.begin_attempt(1).await
    }

    async fn begin_attempt(&self, attempt: u32) -> Result<Txn, TxnError> {
        let core = self.shared.core.clone();
        let start = self
            .now()
            .await
            .map_err(|e| TxnError::NotApplied(e.to_string()))?;
        core.oracle.wait_visible(start).await;
        let open = OpenGuard::open(&core, start, |_| Ok(()))
            .map_err(|e| TxnError::NotApplied(e.to_string()))?;
        Ok(Txn::new(
            self.clone(),
            start,
            attempt,
            Instant::now() + self.safe_window(),
            open,
        ))
    }

    /// Queues a commit; `false` when the committer is gone.
    fn send(&self, request: Request) -> bool {
        self.shared
            .sender
            .as_ref()
            .is_some_and(|s| s.send(request).is_ok())
    }

    /// Commits `mutations`.
    async fn commit(&self, mutations: Vec<Mutation>) -> Result<Ts, TxnError> {
        let (reply, outcome) = tokio::sync::oneshot::channel();
        if !self.send(Request { mutations, reply }) {
            return Err(TxnError::NotApplied("the embedded store is closed".into()));
        }
        match outcome.await {
            Ok(result) => result.map_err(TxnError::from),
            Err(_) => Err(TxnError::Undetermined { token: None }),
        }
    }

    /// Runs `body` in a transaction with the runner's retries
    /// ([`Store::run`](crate::Store::run)).
    pub(crate) async fn run<T: Send, F>(
        &self,
        opts: TxnOptions,
        body: F,
    ) -> Result<Committed<T>, TxnError>
    where
        F: for<'t> FnMut(&'t mut crate::Txn) -> BoxFuture<'t, Result<T, TxnError>>,
    {
        commit::run(self, opts, body).await
    }

    /// This handle with `plan` consulted at every fault point of every run.
    #[cfg_attr(not(feature = "faults"), allow(dead_code))]
    pub(crate) fn with_faults(mut self, plan: Arc<dyn FaultPlan>) -> Self {
        self.faults = Some(plan);
        self
    }

    fn fault(&self, op: &str, point: FaultPoint, attempt: u32) -> Option<Fault> {
        self.faults.as_ref().and_then(|f| f.at(op, point, attempt))
    }

    /// Holds GC at or below `at` for `ttl` (whole seconds, at least 1) under
    /// the service id `loams/<name>`; setting it again moves it. Refused
    /// with [`KvError::BarrierBelowSafePoint`] when GC is past `at`.
    pub async fn barrier(&self, name: &str, at: Ts, ttl: Duration) -> Result<GcBarrier, KvError> {
        let service_id = format!("loams/{name}");
        if name.is_empty() || service_id.len() > 256 {
            return Err(KvError::Embedded(format!(
                "a GC barrier's service id must be loams/<purpose>/<id>, got '{service_id}'"
            )));
        }
        let expires = Instant::now() + Duration::from_secs(ttl.as_secs().max(1));
        self.shared
            .core
            .gc_state()
            .set_barrier(&service_id, at, expires)
            .map_err(|safe_point| KvError::BarrierBelowSafePoint {
                service_id: service_id.clone(),
                ts: at.0,
                min_safe_point: safe_point.0,
            })?;
        Ok(GcBarrier {
            service_id,
            at,
            inner: Inner::Embedded(self.clone()),
        })
    }

    /// Removes the barrier `service_id`.
    pub(crate) fn remove_barrier(&self, service_id: &str) {
        self.shared.core.gc_state().remove_barrier(service_id);
    }

    /// Runs a GC round now (it also runs every `gc_interval`).
    pub async fn gc_once(&self) -> Result<GcReport, KvError> {
        self.gc_once_at(oracle::wall_ms()).await
    }

    /// Runs a GC round as if the wall clock read `now_ms`. For tests only:
    /// a time ahead moves the safe point up to the last timestamp issued
    /// (never past it), after which reads below it are refused.
    #[doc(hidden)]
    pub async fn gc_once_at(&self, now_ms: u64) -> Result<GcReport, KvError> {
        let shared = self.shared.clone();
        tokio::task::spawn_blocking(move || shared.gc_blocking(now_ms))
            .await
            .map_err(storage)?
    }
}

/// Test hooks (feature `faults`).
#[cfg(feature = "faults")]
impl Handle {
    /// Fails the store file's next `n` syncs with an I/O error, so the
    /// write transactions that need them fail.
    pub fn fail_syncs(&self, n: u32) {
        self.shared.core.io_faults.fail_syncs(n);
    }

    /// Makes the committer panic in its next commit group, after it
    /// allocates a commit timestamp.
    pub fn panic_committer(&self) {
        self.shared.core.io_faults.panic_next_group();
    }

    /// Holds the committer before it drains its next group, until the
    /// returned hold is dropped: the commits sent meanwhile apply as one
    /// group.
    pub fn hold_committer(&self) -> CommitterHold {
        self.shared.core.io_faults.hold();
        CommitterHold {
            core: self.shared.core.clone(),
        }
    }

    /// Paces GC rounds: `batch` table entries per write transaction, and
    /// `pause` between them.
    pub fn tune_gc(&self, batch: usize, pause: Duration) {
        self.shared.core.gc_tuning.set(batch, pause);
    }

    /// The oracle's last timestamp issued and its persisted mark.
    pub fn oracle_marks(&self) -> (Ts, Ts) {
        let oracle = &self.shared.core.oracle;
        (oracle.last(), oracle.durable())
    }
}

/// A hold on a store file's committer, from [`Handle::hold_committer`]
/// (feature `faults`); dropping it releases the committer.
#[cfg(feature = "faults")]
#[derive(Debug)]
pub struct CommitterHold {
    core: Arc<Core>,
}

#[cfg(feature = "faults")]
impl Drop for CommitterHold {
    fn drop(&mut self) {
        self.core.io_faults.release();
    }
}
