//! `connect`'s state in the bucket (D1821), under `--state-bucket-prefix`
//! (default `_meta/connect/`):
//!
//! ```text
//! lock/epoch-<20 digits>                      one per process start, create-only
//! instances/<name>/checkpoint/v-<20 digits>   a source's split positions
//! instances/<name>/fabric-position/v-<…>      a sink's committed stream position
//! instances/<name>/control.json               paused, rate_limit (the admin API's)
//! instances/<name>/status.json                the last status, for operators
//! ```
//!
//! **The lock** (as D1354's create-only epoch object): a starting process
//! creates `lock/epoch-<n+1>` with a create-only write, where `n` is the
//! highest epoch it lists, and writes every checkpoint under that epoch. A
//! process that later finds a higher epoch is fenced: it stops its instances
//! and exits, and a checkpoint store refuses its saves once it has seen the
//! successor's. So at most one process per install writes positions.
//!
//! **Versioned documents** are written create-only (`v-<seq+1>`), so two
//! writers can never silently overwrite each other; the reader takes the
//! highest version, and the writer prunes all but the last few.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use loams_connector_sdk::{
    Checkpoint, CheckpointError, CheckpointStore, InstanceKey, Position, SplitId,
};
use loams_store::{Store, StoreError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;
use tokio::time::Instant;

/// How many versions of a document are kept.
const KEEP_VERSIONS: u64 = 3;

/// A state error. Never carries a secret (paths and store errors only).
#[derive(Debug, Clone, thiserror::Error)]
pub enum StateError {
    #[error("connect state: {0}")]
    Store(String),
    #[error("connect state at {path}: {message}")]
    Corrupt { path: String, message: String },
    #[error(
        "this process (epoch {mine}) was fenced: another loams-fabric connect started with epoch {theirs}"
    )]
    Fenced { mine: u64, theirs: u64 },
}

fn store_err(e: StoreError) -> StateError {
    StateError::Store(e.to_string())
}

/// The bucket and the role's prefix in it.
#[derive(Clone)]
pub struct StateStore {
    store: Store,
    prefix: String,
}

impl fmt::Debug for StateStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StateStore")
            .field("prefix", &self.prefix)
            .finish_non_exhaustive()
    }
}

impl StateStore {
    pub fn new(store: Store, prefix: &str) -> Self {
        let prefix = prefix.trim_matches('/');
        let prefix = if prefix.is_empty() {
            String::new()
        } else {
            format!("{prefix}/")
        };
        Self { store, prefix }
    }

    fn path(&self, rel: &str) -> String {
        format!("{}{rel}", self.prefix)
    }

    /// The instance's directory.
    pub fn instance_dir(&self, name: &str) -> String {
        self.path(&format!("instances/{name}"))
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// A small JSON document, overwritten (control and status).
    pub async fn put_json(&self, rel: &str, v: &Value) -> Result<(), StateError> {
        self.store
            .put(&self.path(rel), Bytes::from(v.to_string()))
            .await
            .map(|_| ())
            .map_err(store_err)
    }

    pub async fn get_json(&self, rel: &str) -> Result<Option<Value>, StateError> {
        let path = self.path(rel);
        match self.store.get(&path).await {
            Ok((b, _)) => serde_json::from_slice(&b)
                .map(Some)
                .map_err(|e| StateError::Corrupt {
                    path,
                    message: e.to_string(),
                }),
            Err(StoreError::NotFound { .. }) => Ok(None),
            Err(e) => Err(store_err(e)),
        }
    }

    /// Versioned numbers under `dir` (`<stem><20 digits>`), ascending.
    async fn numbered(&self, dir: &str, stem: &str) -> Result<Vec<u64>, StateError> {
        let dir = self.path(dir);
        let mut out: Vec<u64> = self
            .store
            .list(&dir)
            .await
            .map_err(store_err)?
            .into_iter()
            .filter_map(|o| {
                let name = o.path.rsplit('/').next()?.to_owned();
                name.strip_prefix(stem)?.parse().ok()
            })
            .collect();
        out.sort_unstable();
        Ok(out)
    }
}

fn numbered_name(stem: &str, n: u64) -> String {
    format!("{stem}{n:020}")
}

// --------------------------------------------------------------------- lock

/// This process's epoch, fenced by any later one.
#[derive(Debug, Clone)]
pub struct Lock {
    state: StateStore,
    epoch: u64,
}

impl Lock {
    /// Create the next epoch object (create-only); retry on a race.
    pub async fn acquire(state: &StateStore, owner: &str) -> Result<Self, StateError> {
        for _ in 0..16 {
            let epochs = state.numbered("lock", "epoch-").await?;
            let epoch = epochs.last().copied().unwrap_or(0) + 1;
            let body = serde_json::json!({
                "epoch": epoch,
                "owner": owner,
                "started_ms": now_ms(),
            });
            let path = state.path(&format!("lock/{}", numbered_name("epoch-", epoch)));
            match state
                .store
                .put_if_absent(&path, Bytes::from(body.to_string()))
                .await
            {
                Ok(_) => {
                    // Older epochs are history; keep a few for operators.
                    for old in epochs.iter().filter(|e| **e + KEEP_VERSIONS < epoch) {
                        let p = state.path(&format!("lock/{}", numbered_name("epoch-", *old)));
                        let _ = state.store.delete(&p).await;
                    }
                    return Ok(Self {
                        state: state.clone(),
                        epoch,
                    });
                }
                Err(StoreError::AlreadyExists { .. } | StoreError::PreconditionFailed { .. }) => {
                    continue;
                }
                Err(e) => return Err(store_err(e)),
            }
        }
        Err(StateError::Store(
            "could not create the lock's epoch object after 16 tries".to_owned(),
        ))
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// `Err(Fenced)` once a later process has started.
    pub async fn check(&self) -> Result<(), StateError> {
        let epochs = self.state.numbered("lock", "epoch-").await?;
        match epochs.last() {
            Some(&theirs) if theirs > self.epoch => Err(StateError::Fenced {
                mine: self.epoch,
                theirs,
            }),
            _ => Ok(()),
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

// -------------------------------------------------------- versioned document

/// A document written create-only as `v-<seq>`; the highest is current.
#[derive(Debug)]
struct Versioned {
    state: StateStore,
    dir: String,
    seq: Mutex<Option<u64>>,
}

impl Versioned {
    fn new(state: StateStore, dir: String) -> Self {
        Self {
            state,
            dir,
            seq: Mutex::new(None),
        }
    }

    /// The current version and its body.
    async fn load(&self) -> Result<Option<(u64, Bytes)>, StateError> {
        let rel_dir = self.dir.clone();
        let versions = self.state.numbered(&rel_dir, "v-").await?;
        let Some(&last) = versions.last() else {
            *self.seq.lock().await = Some(0);
            return Ok(None);
        };
        let path = self
            .state
            .path(&format!("{rel_dir}/{}", numbered_name("v-", last)));
        match self.state.store.get(&path).await {
            Ok((b, _)) => {
                *self.seq.lock().await = Some(last);
                Ok(Some((last, b)))
            }
            Err(e) => Err(store_err(e)),
        }
    }

    /// Write the next version. `Ok(false)`: someone else wrote it first.
    async fn save(&self, body: Bytes) -> Result<bool, StateError> {
        let mut seq = self.seq.lock().await;
        let current = match *seq {
            Some(s) => s,
            None => self
                .state
                .numbered(&self.dir, "v-")
                .await?
                .last()
                .copied()
                .unwrap_or(0),
        };
        let next = current + 1;
        let path = self
            .state
            .path(&format!("{}/{}", self.dir, numbered_name("v-", next)));
        match self.state.store.put_if_absent(&path, body).await {
            Ok(_) => {
                *seq = Some(next);
                if next > KEEP_VERSIONS {
                    let old = next - KEEP_VERSIONS;
                    let p = self
                        .state
                        .path(&format!("{}/{}", self.dir, numbered_name("v-", old)));
                    let _ = self.state.store.delete(&p).await;
                }
                Ok(true)
            }
            Err(StoreError::AlreadyExists { .. } | StoreError::PreconditionFailed { .. }) => {
                *seq = None;
                Ok(false)
            }
            Err(e) => Err(store_err(e)),
        }
    }
}

// --------------------------------------------------------------- checkpoints

#[derive(Serialize, Deserialize)]
struct StoredPosition {
    opaque: String,
    display: String,
}

impl StoredPosition {
    fn of(p: &Position) -> Self {
        Self {
            opaque: STANDARD.encode(&p.opaque),
            display: p.display.clone(),
        }
    }

    fn position(&self) -> Option<Position> {
        Some(Position::new(
            STANDARD.decode(&self.opaque).ok()?,
            self.display.clone(),
        ))
    }
}

#[derive(Serialize, Deserialize)]
struct StoredCheckpoint {
    epoch: u64,
    splits: BTreeMap<String, StoredPosition>,
}

/// A source instance's checkpoints, fenced by epoch.
#[derive(Debug)]
pub struct BucketCheckpoints {
    doc: Versioned,
    /// The highest epoch seen in the store.
    seen_epoch: Mutex<u64>,
}

impl BucketCheckpoints {
    pub fn new(state: &StateStore, instance: &str) -> Self {
        Self {
            doc: Versioned::new(state.clone(), format!("instances/{instance}/checkpoint")),
            seen_epoch: Mutex::new(0),
        }
    }

    fn decode(bytes: &[u8]) -> Result<StoredCheckpoint, CheckpointError> {
        serde_json::from_slice(bytes)
            .map_err(|e| CheckpointError::Other(format!("a stored checkpoint does not parse: {e}")))
    }
}

fn unavailable(e: StateError) -> CheckpointError {
    CheckpointError::Unavailable(e.to_string())
}

#[async_trait]
impl CheckpointStore for BucketCheckpoints {
    async fn load(&self, i: &InstanceKey) -> Result<Option<Checkpoint>, CheckpointError> {
        let Some((_, bytes)) = self.doc.load().await.map_err(unavailable)? else {
            return Ok(None);
        };
        let stored = Self::decode(&bytes)?;
        {
            let mut seen = self.seen_epoch.lock().await;
            *seen = (*seen).max(stored.epoch);
        }
        let mut cp = Checkpoint::new(i.clone(), stored.epoch);
        for (split, pos) in stored.splits {
            let pos = pos.position().ok_or_else(|| {
                CheckpointError::Other("a stored position is not base64".to_owned())
            })?;
            cp.splits.insert(SplitId::new(split), pos);
        }
        Ok(Some(cp))
    }

    async fn save(&self, cp: &Checkpoint) -> Result<(), CheckpointError> {
        let stored = StoredCheckpoint {
            epoch: cp.epoch,
            splits: cp
                .splits
                .iter()
                .map(|(k, v)| (k.as_str().to_owned(), StoredPosition::of(v)))
                .collect(),
        };
        let body = Bytes::from(serde_json::to_vec(&stored).unwrap_or_default());
        for _ in 0..4 {
            {
                let seen = *self.seen_epoch.lock().await;
                if seen > cp.epoch {
                    return Err(CheckpointError::Fenced {
                        stored: seen,
                        attempted: cp.epoch,
                    });
                }
            }
            if self.doc.save(body.clone()).await.map_err(unavailable)? {
                let mut seen = self.seen_epoch.lock().await;
                *seen = (*seen).max(cp.epoch);
                return Ok(());
            }
            // Someone else wrote a version: read it, which fences us if it
            // is a successor's, and try again otherwise.
            if let Some((_, bytes)) = self.doc.load().await.map_err(unavailable)? {
                let theirs = Self::decode(&bytes)?.epoch;
                let mut seen = self.seen_epoch.lock().await;
                *seen = (*seen).max(theirs);
            }
        }
        Err(CheckpointError::Unavailable(
            "the checkpoint kept changing under this writer".to_owned(),
        ))
    }
}

// -------------------------------------------------------- a sink's position

/// A sink's committed stream position, written at most every `interval`
/// (and on [`PositionDoc::flush`]); a crash re-delivers at most that much,
/// which an effectively-once sink absorbs and an exactly-once one recovers
/// from its own checkpoint.
#[derive(Debug)]
pub struct PositionDoc {
    doc: Versioned,
    interval: Duration,
    pending: Mutex<(Option<Position>, Option<Instant>)>,
}

impl PositionDoc {
    pub fn new(state: &StateStore, instance: &str, interval: Duration) -> Arc<Self> {
        Arc::new(Self {
            doc: Versioned::new(
                state.clone(),
                format!("instances/{instance}/fabric-position"),
            ),
            interval,
            pending: Mutex::new((None, None)),
        })
    }

    pub async fn load(&self) -> Result<Option<Position>, StateError> {
        let Some((_, bytes)) = self.doc.load().await? else {
            return Ok(None);
        };
        let stored: StoredPosition =
            serde_json::from_slice(&bytes).map_err(|e| StateError::Corrupt {
                path: "fabric-position".to_owned(),
                message: e.to_string(),
            })?;
        Ok(stored.position())
    }

    pub async fn commit(&self, upto: Position) -> Result<(), StateError> {
        let mut p = self.pending.lock().await;
        p.0 = Some(upto);
        let due = self.interval.is_zero() || p.1.is_none_or(|t| t.elapsed() >= self.interval);
        if due {
            self.write(&mut p).await?;
        }
        Ok(())
    }

    pub async fn flush(&self) {
        let mut p = self.pending.lock().await;
        if let Err(e) = self.write(&mut p).await {
            tracing::warn!("flushing a sink's position: {e}");
        }
    }

    async fn write(&self, p: &mut (Option<Position>, Option<Instant>)) -> Result<(), StateError> {
        let Some(pos) = p.0.take() else {
            return Ok(());
        };
        let body = Bytes::from(serde_json::to_vec(&StoredPosition::of(&pos)).unwrap_or_default());
        if !self.doc.save(body.clone()).await? {
            // A race on the version number (a fenced predecessor): retry once.
            self.doc.save(body).await?;
        }
        p.1 = Some(Instant::now());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> StateStore {
        StateStore::new(Store::in_memory(), "_meta/connect/")
    }

    #[tokio::test]
    async fn the_second_lock_fences_the_first() {
        let s = state();
        let a = Lock::acquire(&s, "a").await.expect("lock a");
        assert!(a.check().await.is_ok());
        let b = Lock::acquire(&s, "b").await.expect("lock b");
        assert_eq!(b.epoch(), a.epoch() + 1);
        assert!(matches!(a.check().await, Err(StateError::Fenced { .. })));
        assert!(b.check().await.is_ok());
    }

    #[tokio::test]
    async fn checkpoints_round_trip_and_fence() {
        let s = state();
        let key = InstanceKey::new(0, "orders-in");
        let a = BucketCheckpoints::new(&s, "orders-in");
        let mut cp = Checkpoint::new(key.clone(), 1);
        cp.splits
            .insert(SplitId::new("orders/0"), Position::from_u64(42));
        a.save(&cp).await.expect("save");
        a.save(&cp).await.expect("save again");
        let b = BucketCheckpoints::new(&s, "orders-in");
        let loaded = b.load(&key).await.expect("load").expect("a checkpoint");
        assert_eq!(loaded, cp);
        // A successor saves under epoch 2; the old writer is fenced.
        let mut cp2 = cp.clone();
        cp2.epoch = 2;
        b.save(&cp2).await.expect("successor saves");
        assert!(matches!(
            a.save(&cp).await,
            Err(CheckpointError::Fenced {
                stored: 2,
                attempted: 1
            })
        ));
        // Only the last few versions are kept.
        for _ in 0..5 {
            b.save(&cp2).await.expect("save");
        }
        let versions = s
            .numbered("instances/orders-in/checkpoint", "v-")
            .await
            .expect("list");
        assert!(versions.len() <= KEEP_VERSIONS as usize, "{versions:?}");
    }

    #[tokio::test]
    async fn positions_are_written_at_most_every_interval() {
        let s = state();
        let doc = PositionDoc::new(&s, "out", Duration::from_secs(3600));
        doc.commit(Position::from_u64(1))
            .await
            .expect("first commit writes");
        doc.commit(Position::from_u64(2)).await.expect("held");
        let again = PositionDoc::new(&s, "out", Duration::ZERO);
        assert_eq!(
            again.load().await.expect("load"),
            Some(Position::from_u64(1))
        );
        doc.flush().await;
        assert_eq!(
            again.load().await.expect("load"),
            Some(Position::from_u64(2))
        );
    }
}
