//! Group commit of lake writes (HS1 Task 12, §49 §6.1 step 3 and "Group
//! commit").
//!
//! Every write's data files are uploaded by the statement that made them; their
//! commit goes through the table's [`Committer`] queue. One task per table takes
//! what is queued and commits it as one `append` snapshot:
//!
//! * **At most one commit per interval per table.** A write that arrives when the
//!   table has not committed within `loams_lake_commit_interval_ms` commits at
//!   once; one that arrives sooner waits for the interval to end, and everything
//!   queued by then (up to `loams_lake_commit_bytes` of files) shares the commit.
//!   Back-to-back commits keep a gap of one to two commit cycles even at
//!   interval 0, so another front committing to the same table gets its turn.
//!   A write is answered after its commit, unless `async_insert = 1` and
//!   `wait_for_async_insert = 0` asked to be answered once queued.
//! * **Deduplication** ([`crate::dedup`]) is decided against the table's metadata
//!   as loaded for the commit, and again after every reload: a write whose key is
//!   held answers with the snapshot that holds it and no rows written; a key
//!   repeated within one commit keeps its first write.
//! * **Rebase and retry.** The commit requires `main` to be where the table was
//!   loaded (`assert-ref-snapshot-id`). On a conflict — another front, a
//!   compaction — the table is reloaded and the append rebuilt on the new `main`
//!   (appends never conflict on data), up to [`MAX_ATTEMPTS`] times with jitter.
//!   On a failure that may or may not have committed (the catalog timed out), the
//!   reload looks for this commit's own `loams.commit-id` before trying again, so
//!   nothing is applied twice.
//!
//! A write whose commit fails is answered with the error and its files stay
//! unreferenced, for the orphan collection of HS1 Task 15.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use loams_house_lake::spec::DataFile;
use loams_house_lake::{TableIdent, append};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use crate::catalog::{CatalogError, HouseCatalog, LakeRoot};
use crate::dedup;
use crate::errors::{ChError, HouseError};

/// How many times one batch is tried against the catalog.
pub const MAX_ATTEMPTS: u32 = 20;
/// How long a table's queue task stays without work before it ends.
const IDLE: Duration = Duration::from_secs(30);

/// One write waiting for its commit.
#[derive(Debug)]
pub struct Pending {
    /// Its data files, uploaded already.
    pub files: Vec<DataFile>,
    /// Its deduplication key, if it has one.
    pub key: Option<String>,
    /// `loams_dedup_window`.
    pub window: usize,
    /// `loams_lake_commit_interval_ms`.
    pub interval: Duration,
    /// `loams_lake_commit_bytes`.
    pub commit_bytes: u64,
    /// Where the answer goes.
    pub reply: oneshot::Sender<Result<Committed, HouseError>>,
}

impl Pending {
    fn file_bytes(&self) -> u64 {
        self.files.iter().map(DataFile::file_size_in_bytes).sum()
    }
}

/// A write's commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Committed {
    /// The snapshot holding its rows.
    pub snapshot_id: i64,
    /// Whether its key was held already, so nothing of it was committed.
    pub duplicate: bool,
}

/// What a commit needs besides the catalog: where table files live.
#[derive(Debug, Clone)]
pub struct CommitIo {
    /// The catalog.
    pub catalog: Arc<dyn HouseCatalog>,
    /// The bucket.
    pub store: Option<loams_store::Store>,
    /// The tables' root in it.
    pub root: LakeRoot,
}

impl CommitIo {
    fn key(&self, location: &str) -> Result<String, HouseError> {
        self.root
            .relative(location)
            .map(str::to_string)
            .ok_or_else(|| {
                HouseError::from(ChError::bad_arguments(format!(
                    "{location} is outside the House's store ({})",
                    self.root
                )))
            })
    }

    /// The store, or `48` for a House started without one, which can read
    /// its catalog but write no table files.
    pub fn store(&self) -> Result<&loams_store::Store, HouseError> {
        self.store.as_ref().ok_or_else(|| {
            HouseError::from(ChError::not_implemented(
                "this House writes no table files: it was started without --store",
            ))
        })
    }

    /// Reads a table file by its absolute location.
    pub async fn read(&self, location: &str) -> Result<bytes::Bytes, HouseError> {
        let key = self.key(location)?;
        self.store()?
            .get(&key)
            .await
            .map(|(bytes, _)| bytes)
            // The store's own words may name its endpoint or account (R9.2).
            .map_err(|_| {
                HouseError::from(ChError::network_error(
                    "the store failed to read a table file",
                ))
            })
    }

    /// Writes a table file at its absolute location.
    pub async fn write(&self, location: &str, bytes: bytes::Bytes) -> Result<(), HouseError> {
        let key = self.key(location)?;
        self.store()?
            .put(&key, bytes)
            .await
            .map(|_| ())
            .map_err(|_| {
                HouseError::from(ChError::network_error(
                    "the store failed to write a table file",
                ))
            })
    }

    /// Every file under a table location (or any directory of the store), by
    /// absolute location, with when it was written (HS1 Task 15).
    pub async fn list(
        &self,
        location: &str,
    ) -> Result<Vec<loams_house_lake::orphans::StoredFile>, HouseError> {
        let key = self.key(location)?;
        let prefix = format!("{}/", key.trim_end_matches('/'));
        let root = self.root.to_string();
        let listed = self.store()?.list(&prefix).await.map_err(|_| {
            HouseError::from(ChError::network_error(
                "the store failed to list a table's files",
            ))
        })?;
        Ok(listed
            .into_iter()
            .filter(|info| info.path.starts_with(&prefix))
            .map(|info| loams_house_lake::orphans::StoredFile {
                location: format!("{root}/{}", info.path),
                last_modified_ms: info.last_modified_ms,
            })
            .collect())
    }

    /// Deletes a table file by its absolute location; a missing file is
    /// deleted already (HS1 Task 15).
    pub async fn delete(&self, location: &str) -> Result<(), HouseError> {
        let key = self.key(location)?;
        self.store()?.delete(&key).await.map_err(|_| {
            HouseError::from(ChError::network_error(
                "the store failed to delete a table file",
            ))
        })
    }
}

type Queues = Arc<Mutex<HashMap<String, mpsc::UnboundedSender<Pending>>>>;

/// Tables that took a commit since compaction (HS1 Task 13) and the views'
/// catch-up (HS1 Task 17) last looked.
#[derive(Debug, Default)]
struct DirtySets {
    compaction: std::collections::HashSet<TableIdent>,
    views: std::collections::HashSet<TableIdent>,
}

impl DirtySets {
    fn mark(&mut self, table: &TableIdent) {
        self.compaction.insert(table.clone());
        self.views.insert(table.clone());
    }
}

type Dirty = Arc<Mutex<DirtySets>>;

/// The lake's commit queues, one per table (see the module docs).
#[derive(Debug)]
pub struct Committer {
    io: Arc<CommitIo>,
    queues: Queues,
    dirty: Dirty,
    /// Where each catalog commit's time and result are counted (HS1 Task 23).
    metrics: Arc<crate::metrics::Metrics>,
}

impl Committer {
    /// Commits through `io`.
    pub fn new(io: CommitIo) -> Self {
        Self {
            io: Arc::new(io),
            queues: Arc::new(Mutex::new(HashMap::new())),
            dirty: Arc::new(Mutex::new(DirtySets::default())),
            metrics: Arc::default(),
        }
    }

    /// Counts its commits in `metrics` (HS1 Task 23).
    pub fn with_metrics(mut self, metrics: Arc<crate::metrics::Metrics>) -> Self {
        self.metrics = metrics;
        self
    }

    /// The tables that took commits since the last call (compaction's
    /// candidates, HS1 Task 13).
    pub fn take_dirty(&self) -> Vec<TableIdent> {
        let mut dirty = self.dirty.lock().unwrap_or_else(|e| e.into_inner());
        dirty.compaction.drain().collect()
    }

    /// The tables that took commits since the last call (the views'
    /// catch-up's candidates, HS1 Task 17).
    pub fn take_views_dirty(&self) -> Vec<TableIdent> {
        let mut dirty = self.dirty.lock().unwrap_or_else(|e| e.into_inner());
        dirty.views.drain().collect()
    }

    /// Marks a table for compaction's and the views' next look.
    pub fn mark_dirty(&self, table: &TableIdent) {
        self.dirty
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .mark(table);
    }

    /// The store and catalog it commits through.
    pub fn io(&self) -> &CommitIo {
        &self.io
    }

    /// Queues a write for `table`'s next commit.
    pub fn submit(&self, table: &TableIdent, pending: Pending) {
        let name = crate::catalog::table_text(table);
        let mut queues = self.queues.lock().unwrap_or_else(|e| e.into_inner());
        let pending = match queues.get(&name) {
            Some(sender) => match sender.send(pending) {
                Ok(()) => return,
                // The task ended between its last check and now: start another.
                Err(mpsc::error::SendError(pending)) => pending,
            },
            None => pending,
        };
        let (sender, receiver) = mpsc::unbounded_channel();
        let _ = sender.send(pending);
        queues.insert(name.clone(), sender);
        tokio::spawn(run_table(
            Arc::clone(&self.io),
            table.clone(),
            receiver,
            Arc::clone(&self.queues),
            name,
            Arc::clone(&self.dirty),
            Arc::clone(&self.metrics),
        ));
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// One table's queue: batches, commits, answers.
async fn run_table(
    io: Arc<CommitIo>,
    table: TableIdent,
    mut receiver: mpsc::UnboundedReceiver<Pending>,
    queues: Queues,
    name: String,
    dirty: Dirty,
    metrics: Arc<crate::metrics::Metrics>,
) {
    let mut last_commit: Option<Instant> = None;
    let mut last_cycle = Duration::ZERO;
    loop {
        let first = match tokio::time::timeout(IDLE, receiver.recv()).await {
            Ok(Some(pending)) => pending,
            Ok(None) => return,
            Err(_) => {
                // Idle: end, unless a write came in meanwhile. Writers send under
                // the same lock, so none is lost between the check and the removal.
                let mut map = queues.lock().unwrap_or_else(|e| e.into_inner());
                match receiver.try_recv() {
                    Ok(pending) => {
                        drop(map);
                        pending
                    }
                    Err(_) => {
                        map.remove(&name);
                        return;
                    }
                }
            }
        };
        let interval = first.interval;
        let cap = first.commit_bytes;
        let mut bytes = first.file_bytes();
        let mut batch = vec![first];
        // Back-to-back commits keep a gap of one to two commit cycles even at
        // interval 0: another front that lost a race to this queue reloads and
        // retries, and needs a whole cycle to land; without the gap a queue that
        // never empties wins every next race too (R12.3, measured in
        // `concurrent_commits_all_land`).
        let spread = 1.0 + f64::from(uuid::Uuid::new_v4().as_u128() as u16) / f64::from(u16::MAX);
        let gap = interval.max(last_cycle.mul_f64(spread));
        let deadline = last_commit.map_or_else(Instant::now, |at| at + gap);
        while bytes < cap {
            let next = if Instant::now() >= deadline {
                // The interval is over: take only what is queued already.
                receiver.try_recv().ok()
            } else {
                tokio::time::timeout_at(deadline, receiver.recv())
                    .await
                    .ok()
                    .flatten()
            };
            match next {
                Some(pending) => {
                    bytes += pending.file_bytes();
                    batch.push(pending);
                }
                None => break,
            }
        }
        let cycle = Instant::now();
        commit_batch(&io, &table, batch, &metrics).await;
        dirty.lock().unwrap_or_else(|e| e.into_inner()).mark(&table);
        last_cycle = cycle.elapsed();
        last_commit = Some(Instant::now());
    }
}

/// What became of one write of a batch.
#[derive(Clone, Copy, Debug)]
enum Fate {
    /// Committed in this batch's snapshot.
    Commit,
    /// Its key is held by this snapshot already.
    HeldBy(i64),
    /// Its key repeats an earlier write of the batch.
    Repeat,
}

async fn commit_batch(
    io: &CommitIo,
    table: &TableIdent,
    batch: Vec<Pending>,
    metrics: &crate::metrics::Metrics,
) {
    let answers = commit(io, table, &batch, metrics).await;
    for (pending, answer) in batch.into_iter().zip(answers) {
        let _ = pending.reply.send(answer);
    }
}

/// Commits a batch; answers one result per write, in order.
async fn commit(
    io: &CommitIo,
    table: &TableIdent,
    batch: &[Pending],
    metrics: &crate::metrics::Metrics,
) -> Vec<Result<Committed, HouseError>> {
    let fail_all = |err: HouseError| batch.iter().map(|_| Err(err.clone())).collect();
    let commit_id = uuid::Uuid::new_v4().to_string();
    // The parent and the fates of an attempt that may have committed.
    let mut sent: Option<(Option<i64>, Vec<Fate>)> = None;
    let mut attempt = 0;
    loop {
        attempt += 1;
        let loaded = match io.catalog.load_table(table).await {
            Ok(loaded) => loaded,
            Err(err) => return fail_all(err.into()),
        };
        let metadata = &loaded.metadata;
        // An earlier attempt whose answer was lost may have committed.
        if let Some((parent, fates)) = &sent
            && let Some(snapshot_id) = dedup::committed(metadata, &commit_id, *parent)
        {
            return answers(fates, snapshot_id);
        }
        let now = now_ms();
        let fates = fates(batch, metadata, now);
        let keep: Vec<&Pending> = batch
            .iter()
            .zip(&fates)
            .filter(|(_, fate)| matches!(fate, Fate::Commit))
            .map(|(pending, _)| pending)
            .collect();
        if keep.is_empty() {
            return answers(&fates, 0);
        }
        let parent = metadata.current_snapshot().map(|s| s.snapshot_id());
        let parent_list = match metadata.current_snapshot() {
            Some(snapshot) => match io.read(snapshot.manifest_list()).await {
                Ok(bytes) => Some(bytes),
                Err(err) => return fail_all(err),
            },
            None => None,
        };
        let keys: Vec<String> = keep.iter().filter_map(|p| p.key.clone()).collect();
        let files: Vec<DataFile> = keep.iter().flat_map(|p| p.files.clone()).collect();
        let built = append::append_files(
            metadata,
            parent_list.as_deref(),
            files,
            dedup::summary(&keys, &commit_id),
            now,
        )
        .await;
        let built = match built {
            Ok(built) => built,
            Err(err) => {
                return fail_all(HouseError::from(ChError::bad_arguments(format!(
                    "the commit could not be built: {err}"
                ))));
            }
        };
        for (location, bytes) in built.files {
            if let Err(err) = io.write(&location, bytes).await {
                return fail_all(err);
            }
        }
        let committing = Instant::now();
        let committed = io
            .catalog
            .commit(table, built.requirements, built.updates)
            .await;
        metrics.commit(
            match &committed {
                Ok(_) => "committed",
                Err(CatalogError::Conflict { .. }) => "conflict",
                Err(_) => "failed",
            },
            committing.elapsed(),
        );
        match committed {
            Ok(_) => return answers(&fates, built.snapshot_id),
            Err(err) if attempt < MAX_ATTEMPTS && retryable(&err) => {
                if matches!(err, CatalogError::Unavailable(_)) {
                    // It may have landed: the next load looks for it.
                    sent = Some((parent, fates));
                }
                // Jitter only, growing slowly: the writer that just won is
                // already loading for its next commit, and a fixed back-off
                // would hand it that race too.
                let spread = 5 * u64::from(attempt);
                let jitter = (uuid::Uuid::new_v4().as_u128() as u64) % spread.max(1);
                tokio::time::sleep(Duration::from_millis(jitter)).await;
            }
            Err(err) => return fail_all(err.into()),
        }
    }
}

fn retryable(err: &CatalogError) -> bool {
    matches!(
        err,
        CatalogError::Conflict { .. } | CatalogError::Unavailable(_)
    )
}

/// Each write's fate against `metadata` at `now_ms`. A key repeated within the
/// batch shares its first write's fate as a duplicate.
fn fates(
    batch: &[Pending],
    metadata: &loams_house_lake::spec::TableMetadata,
    now_ms: i64,
) -> Vec<Fate> {
    let mut first: HashMap<&str, Fate> = HashMap::new();
    batch
        .iter()
        .map(|pending| {
            let Some(key) = pending.key.as_deref() else {
                return Fate::Commit;
            };
            if let Some(fate) = first.get(key) {
                return match fate {
                    Fate::HeldBy(id) => Fate::HeldBy(*id),
                    _ => Fate::Repeat,
                };
            }
            let fate = match dedup::find(metadata, key, pending.window, now_ms) {
                Some(snapshot_id) => Fate::HeldBy(snapshot_id),
                None => Fate::Commit,
            };
            first.insert(key, fate);
            fate
        })
        .collect()
}

fn answers(fates: &[Fate], snapshot_id: i64) -> Vec<Result<Committed, HouseError>> {
    fates
        .iter()
        .map(|fate| {
            Ok(match fate {
                Fate::Commit => Committed {
                    snapshot_id,
                    duplicate: false,
                },
                Fate::HeldBy(id) => Committed {
                    snapshot_id: *id,
                    duplicate: true,
                },
                Fate::Repeat => Committed {
                    snapshot_id,
                    duplicate: true,
                },
            })
        })
        .collect()
}

/// `TRUNCATE` of a lake table (§49 §6.3): a `delete` snapshot with no files,
/// rebased and retried like an append. Answers the new snapshot, or `None` when
/// the table had none.
pub async fn truncate(io: &CommitIo, table: &TableIdent) -> Result<Option<i64>, HouseError> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        let loaded = io.catalog.load_table(table).await?;
        let built = append::truncate(&loaded.metadata, now_ms())
            .await
            .map_err(|err| HouseError::from(ChError::bad_arguments(err.to_string())))?;
        let Some(built) = built else {
            return Ok(None);
        };
        for (location, bytes) in built.files {
            io.write(&location, bytes).await?;
        }
        match io
            .catalog
            .commit(table, built.requirements, built.updates)
            .await
        {
            Ok(_) => return Ok(Some(built.snapshot_id)),
            // A truncate that landed but was not confirmed is found as `main`
            // having no files on the next load, which truncates nothing more.
            Err(err) if attempt < MAX_ATTEMPTS && retryable(&err) => {
                let loaded = io.catalog.load_table(table).await?;
                if loaded
                    .metadata
                    .current_snapshot()
                    .is_some_and(|s| s.snapshot_id() == built.snapshot_id)
                {
                    return Ok(Some(built.snapshot_id));
                }
            }
            Err(err) => return Err(err.into()),
        }
    }
}
