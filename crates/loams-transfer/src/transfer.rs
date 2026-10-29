//! Chunked transfers (§85 §5–§9).
//!
//! A transfer is a file of `size` bytes cut into fixed `chunk_size` chunks
//! (D2423). Each chunk is written once to a key that carries its index and
//! hash (D2424), then committed into the transfer's journal under the
//! writer's lease (D2425, D2428). `complete` checks every chunk is in, and
//! records the root over the chunk list and the client's whole-file hash
//! (D2433).

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use futures::stream::{BoxStream, Stream, StreamExt};
use loams_common::NamespaceId;
use loams_common::meta::Fence;
use loams_store::{Store, StoreError};
use object_store::ObjectStoreExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{Semaphore, oneshot};

use crate::checkpoint::Checkpoints;
use crate::error::{Error, Result};
use crate::journal::{Head, Journal, Loaded};
use crate::keys;

/// Limits and timings for transfers.
#[derive(Clone, Debug)]
pub struct TransfersConfig {
    /// The lease's time to live (D2428: 30 s).
    pub lease_ttl: Duration,
    /// The smallest chunk size (D2423: 8 MiB). Tests lower it.
    pub min_chunk: u64,
    /// The largest chunk size (D2423: 64 MiB).
    pub max_chunk: u64,
    /// The most chunks in one transfer (D2423: 16,384).
    pub max_chunks: u64,
    /// The bucket's multipart part size for chunk writes.
    pub part_size: usize,
    /// The most chunk writes in flight in this process (§85 §13).
    pub max_inflight_chunks: usize,
}

impl Default for TransfersConfig {
    fn default() -> Self {
        TransfersConfig {
            lease_ttl: Duration::from_secs(30),
            min_chunk: 8 << 20,
            max_chunk: 64 << 20,
            max_chunks: 16_384,
            part_size: 8 << 20,
            max_inflight_chunks: 16,
        }
    }
}

/// A create request (§85 §6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateTransfer {
    /// A label for people. Never used as a path (D2443).
    pub name: String,
    pub size: u64,
    pub chunk_size: u64,
    /// Who created it: the desktop's install id.
    pub client_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Spec {
    name: String,
    size: u64,
    chunk_size: u64,
    client_id: String,
    created_ms: u64,
}

impl Spec {
    fn chunks(&self) -> u64 {
        self.size.div_ceil(self.chunk_size)
    }

    fn chunk_len(&self, i: u32) -> u64 {
        let start = u64::from(i) * self.chunk_size;
        self.chunk_size.min(self.size.saturating_sub(start))
    }
}

/// One committed fact in a transfer's journal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum TransferEntry {
    Chunk { i: u32, len: u64, sha: String },
    Complete { root: String, file_sha256: String },
}

/// A transfer's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferState {
    Open,
    Complete,
    Abandoned,
}

impl TransferState {
    fn as_str(self) -> &'static str {
        match self {
            TransferState::Open => "open",
            TransferState::Complete => "complete",
            TransferState::Abandoned => "abandoned",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        match s {
            "open" => Ok(TransferState::Open),
            "complete" => Ok(TransferState::Complete),
            "abandoned" => Ok(TransferState::Abandoned),
            other => Err(Error::Corrupt(format!("unknown transfer state {other}"))),
        }
    }
}

/// Who holds a transfer's lease.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseView {
    pub holder: String,
    pub epoch: u64,
    pub deadline_ms: u64,
}

/// What `GET /transfers/{tid}` answers: the checkpoint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferView {
    pub tid: String,
    pub name: String,
    pub size: u64,
    pub chunk_size: u64,
    pub chunks: u64,
    pub state: TransferState,
    /// Committed chunk indexes as half-open ranges `[start, end)`.
    pub committed: Vec<[u32; 2]>,
    pub committed_chunks: u64,
    pub committed_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease: Option<LeaseView>,
    pub client_id: String,
    pub created_ms: u64,
}

/// The outcome of a chunk PUT.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkPut {
    /// The chunk was already committed with this hash: nothing was written
    /// (D2427).
    pub already: bool,
    pub committed_chunks: u64,
    pub committed_bytes: u64,
}

/// A transfer in a namespace's listing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferSummary {
    pub tid: String,
    pub state: TransferState,
    pub committed_chunks: u64,
    pub committed_bytes: u64,
}

/// The root over a transfer's chunk list (D2433): SHA-256 over, for each
/// chunk in index order, its index (u32, big endian), its length (u64, big
/// endian) and its 32-byte SHA-256.
pub fn root<'a>(chunks: impl IntoIterator<Item = (u32, u64, &'a str)>) -> Result<String> {
    let mut h = Sha256::new();
    for (i, len, sha) in chunks {
        let raw = hex::decode(sha).map_err(|e| Error::Corrupt(format!("chunk hash: {e}")))?;
        h.update(i.to_be_bytes());
        h.update(len.to_be_bytes());
        h.update(&raw);
    }
    Ok(hex::encode(h.finalize()))
}

struct Pending {
    i: u32,
    len: u64,
    sha: String,
    epoch: u64,
    tx: oneshot::Sender<Result<ChunkPut>>,
}

#[derive(Default)]
struct Slot {
    loaded: tokio::sync::Mutex<Option<Loaded<TransferEntry>>>,
    pending: Mutex<Vec<Pending>>,
}

/// The transfer service over a bucket and the metastore.
pub struct Transfers {
    store: Store,
    ck: Arc<dyn Checkpoints>,
    cfg: TransfersConfig,
    slots: Mutex<HashMap<(u64, String), Arc<Slot>>>,
    inflight: Arc<Semaphore>,
}

impl std::fmt::Debug for Transfers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transfers")
            .field("cfg", &self.cfg)
            .finish_non_exhaustive()
    }
}

fn committed_map(entries: &[TransferEntry]) -> BTreeMap<u32, (u64, &str)> {
    entries
        .iter()
        .filter_map(|e| match e {
            TransferEntry::Chunk { i, len, sha } => Some((*i, (*len, sha.as_str()))),
            TransferEntry::Complete { .. } => None,
        })
        .collect()
}

fn ranges(indexes: impl Iterator<Item = u32>) -> Vec<[u32; 2]> {
    let mut out: Vec<[u32; 2]> = Vec::new();
    for i in indexes {
        match out.last_mut() {
            Some(r) if r[1] == i => r[1] = i + 1,
            _ => out.push([i, i + 1]),
        }
    }
    out
}

fn check_label(what: &str, s: &str, max: usize) -> Result<()> {
    if s.is_empty() || s.len() > max || s.chars().any(char::is_control) {
        return Err(Error::BadRequest(format!(
            "{what} must be 1..={max} bytes without control characters"
        )));
    }
    Ok(())
}

impl Transfers {
    pub fn new(store: Store, ck: Arc<dyn Checkpoints>, cfg: TransfersConfig) -> Self {
        let permits = cfg.max_inflight_chunks.max(1);
        Transfers {
            store,
            ck,
            cfg,
            slots: Mutex::new(HashMap::new()),
            inflight: Arc::new(Semaphore::new(permits)),
        }
    }

    pub fn config(&self) -> &TransfersConfig {
        &self.cfg
    }

    fn journal(&self, ns: NamespaceId, tid: &str) -> Journal {
        Journal::new(
            self.store.clone(),
            Arc::clone(&self.ck),
            ns,
            keys::transfer_pointer(tid),
            keys::transfer_journal(ns, tid),
        )
    }

    fn slot(&self, ns: NamespaceId, tid: &str) -> Arc<Slot> {
        let mut slots = self.slots.lock().unwrap_or_else(|p| p.into_inner());
        Arc::clone(slots.entry((ns.0, tid.to_string())).or_default())
    }

    fn forget(&self, ns: NamespaceId, tid: &str) {
        let mut slots = self.slots.lock().unwrap_or_else(|p| p.into_inner());
        slots.remove(&(ns.0, tid.to_string()));
    }

    async fn spec(&self, ns: NamespaceId, tid: &str) -> Result<Spec> {
        keys::check_id("transfer", tid)?;
        match self.store.get(&keys::transfer_spec(ns, tid)).await {
            Ok((data, _)) => Ok(serde_json::from_slice(&data)?),
            Err(StoreError::NotFound { .. }) => Err(Error::NotFound(format!("transfer {tid}"))),
            Err(e) => Err(e.into()),
        }
    }

    async fn loaded(&self, ns: NamespaceId, tid: &str) -> Result<Loaded<TransferEntry>> {
        self.journal(ns, tid)
            .load()
            .await?
            .ok_or_else(|| Error::NotFound(format!("transfer {tid}")))
    }

    /// Creates transfer `tid` (a client-chosen ULID), or answers the existing
    /// one if it has the same spec: creating is idempotent (§85 §6).
    pub async fn create(
        &self,
        ns: NamespaceId,
        tid: &str,
        req: CreateTransfer,
    ) -> Result<TransferView> {
        keys::check_id("transfer", tid)?;
        check_label("name", &req.name, 1024)?;
        check_label("client_id", &req.client_id, 128)?;
        let cs = req.chunk_size;
        if !cs.is_power_of_two() || cs < self.cfg.min_chunk || cs > self.cfg.max_chunk {
            return Err(Error::BadRequest(format!(
                "chunk_size must be a power of two from {} to {} bytes",
                self.cfg.min_chunk, self.cfg.max_chunk
            )));
        }
        if req.size.div_ceil(cs) > self.cfg.max_chunks {
            return Err(Error::BadRequest(format!(
                "a transfer has at most {} chunks; use a larger chunk_size",
                self.cfg.max_chunks
            )));
        }
        let spec = Spec {
            name: req.name.clone(),
            size: req.size,
            chunk_size: cs,
            client_id: req.client_id.clone(),
            created_ms: self.ck.now_ms(),
        };
        let key = keys::transfer_spec(ns, tid);
        match self
            .store
            .put_if_absent(&key, Bytes::from(serde_json::to_vec(&spec)?))
            .await
        {
            Ok(_) => {}
            Err(StoreError::AlreadyExists { .. }) => {
                let have = self.spec(ns, tid).await?;
                if (have.name, have.size, have.chunk_size, have.client_id)
                    != (req.name, req.size, req.chunk_size, req.client_id)
                {
                    return Err(Error::conflict(
                        "transfer_exists",
                        format!("transfer {tid} exists with another spec"),
                    ));
                }
            }
            Err(e) => return Err(e.into()),
        }
        self.journal(ns, tid)
            .create(&Head::new(TransferState::Open.as_str()))
            .await?;
        self.view(ns, tid).await
    }

    /// The checkpoint (§85 §6 `GET /{tid}`).
    pub async fn view(&self, ns: NamespaceId, tid: &str) -> Result<TransferView> {
        let spec = self.spec(ns, tid).await?;
        let cur = self.loaded(ns, tid).await?;
        let lease = self
            .ck
            .lease(&keys::transfer_lease(ns, tid))
            .await?
            .and_then(|l| {
                let holder = l.owner.clone()?;
                Some(LeaseView {
                    holder,
                    epoch: l.epoch,
                    deadline_ms: l.deadline_ms,
                })
            });
        let map = committed_map(&cur.entries);
        let (mut root, mut file_sha256) = (None, None);
        for e in &cur.entries {
            if let TransferEntry::Complete {
                root: r,
                file_sha256: f,
            } = e
            {
                root = Some(r.clone());
                file_sha256 = Some(f.clone());
            }
        }
        Ok(TransferView {
            tid: tid.to_string(),
            name: spec.name.clone(),
            size: spec.size,
            chunk_size: spec.chunk_size,
            chunks: spec.chunks(),
            state: TransferState::parse(&cur.head.st)?,
            committed: ranges(map.keys().copied()),
            committed_chunks: map.len() as u64,
            committed_bytes: cur.head.bytes,
            root,
            file_sha256,
            lease,
            client_id: spec.client_id,
            created_ms: spec.created_ms,
        })
    }

    /// Acquires or renews the transfer's lease for `holder` (D2428).
    pub async fn lease(
        &self,
        ns: NamespaceId,
        tid: &str,
        holder: &str,
    ) -> Result<loams_common::meta::LeaseGrant> {
        check_label("holder", holder, 128)?;
        let cur = self.loaded(ns, tid).await?;
        if TransferState::parse(&cur.head.st)? != TransferState::Open {
            return Err(Error::State(format!("transfer {tid} is {}", cur.head.st)));
        }
        Ok(self
            .ck
            .acquire_lease(&keys::transfer_lease(ns, tid), holder, self.cfg.lease_ttl)
            .await?)
    }

    async fn check_epoch(&self, ns: NamespaceId, tid: &str, epoch: u64) -> Result<()> {
        // The fence on the commit is what counts (D2430); this early check
        // only avoids writing a chunk that could never commit.
        match self.ck.lease(&keys::transfer_lease(ns, tid)).await? {
            Some(l) if l.epoch == epoch && l.owner.is_some() => Ok(()),
            _ => Err(Error::LeaseLost),
        }
    }

    /// Streams chunk `i` in, checks its length and hash while writing, and
    /// commits it (§85 §6 `PUT /{tid}/chunks/{i}`).
    pub async fn put_chunk(
        &self,
        ns: NamespaceId,
        tid: &str,
        i: u32,
        sha: &str,
        epoch: u64,
        body: BoxStream<'static, io::Result<Bytes>>,
    ) -> Result<ChunkPut> {
        keys::check_sha256("chunk hash", sha)?;
        let spec = self.spec(ns, tid).await?;
        if u64::from(i) >= spec.chunks() {
            return Err(Error::BadRequest(format!(
                "chunk {i} is out of range (the transfer has {})",
                spec.chunks()
            )));
        }
        let len = spec.chunk_len(i);
        let slot = self.slot(ns, tid);
        {
            let mut guard = slot.loaded.lock().await;
            if guard.is_none() {
                *guard = Some(self.loaded(ns, tid).await?);
            }
            if let Some(cur) = guard.as_ref() {
                if TransferState::parse(&cur.head.st)? != TransferState::Open {
                    return Err(Error::State(format!("transfer {tid} is {}", cur.head.st)));
                }
                if let Some((_, have)) = committed_map(&cur.entries).get(&i) {
                    return if *have == sha {
                        Ok(ChunkPut {
                            already: true,
                            committed_chunks: cur.head.n,
                            committed_bytes: cur.head.bytes,
                        })
                    } else {
                        Err(Error::conflict(
                            "chunk_conflict",
                            format!("chunk {i} is committed with another hash"),
                        ))
                    };
                }
            }
        }
        self.check_epoch(ns, tid, epoch).await?;
        let _permit = Arc::clone(&self.inflight)
            .acquire_owned()
            .await
            .map_err(|_| Error::State("shutting down".into()))?;
        let verdict = Arc::new(Mutex::new(None));
        let checked = Verified {
            inner: body,
            hasher: Sha256::new(),
            seen: 0,
            len,
            sha: sha.to_string(),
            verdict: Arc::clone(&verdict),
            done: false,
        };
        let key = keys::chunk(ns, tid, i, sha);
        let written = self
            .store
            .put_multipart(&key, checked.boxed(), self.cfg.part_size)
            .await;
        if let Some(msg) = verdict.lock().unwrap_or_else(|p| p.into_inner()).take() {
            return Err(Error::Integrity(msg));
        }
        written?;
        self.commit(ns, tid, &slot, i, len, sha.to_string(), epoch)
            .await
    }

    /// Group commit (§85 §5.1): queue the entry, then whoever holds the
    /// slot's lock commits everything queued with the same epoch in one
    /// segment and one swap.
    #[allow(clippy::too_many_arguments)]
    async fn commit(
        &self,
        ns: NamespaceId,
        tid: &str,
        slot: &Slot,
        i: u32,
        len: u64,
        sha: String,
        epoch: u64,
    ) -> Result<ChunkPut> {
        let (tx, rx) = oneshot::channel();
        slot.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(Pending {
                i,
                len,
                sha,
                epoch,
                tx,
            });
        {
            let mut guard = slot.loaded.lock().await;
            loop {
                let batch: Vec<Pending> = {
                    let mut q = slot.pending.lock().unwrap_or_else(|p| p.into_inner());
                    let Some(first) = q.first().map(|p| p.epoch) else {
                        break;
                    };
                    let (same, rest): (Vec<_>, Vec<_>) =
                        q.drain(..).partition(|p| p.epoch == first);
                    *q = rest;
                    same
                };
                self.commit_batch(ns, tid, &mut guard, batch).await;
            }
        }
        rx.await.map_err(|_| Error::Meta("commit dropped".into()))?
    }

    async fn commit_batch(
        &self,
        ns: NamespaceId,
        tid: &str,
        guard: &mut Option<Loaded<TransferEntry>>,
        batch: Vec<Pending>,
    ) {
        let Some(epoch) = batch.first().map(|p| p.epoch) else {
            return;
        };
        let fence = Fence {
            lease: keys::transfer_lease(ns, tid),
            epoch,
        };
        let journal = self.journal(ns, tid);
        let mut outcome: Result<()> = Err(Error::Meta("commit retries exhausted".into()));
        for _ in 0..16 {
            let cur = match guard.take() {
                Some(c) => c,
                None => match self.loaded(ns, tid).await {
                    Ok(c) => c,
                    Err(e) => {
                        outcome = Err(e);
                        break;
                    }
                },
            };
            match TransferState::parse(&cur.head.st) {
                Ok(TransferState::Open) => {}
                Ok(_) => {
                    outcome = Err(Error::State(format!("transfer {tid} is {}", cur.head.st)));
                    *guard = Some(cur);
                    break;
                }
                Err(e) => {
                    outcome = Err(e);
                    break;
                }
            }
            let have = committed_map(&cur.entries);
            let mut new = Vec::new();
            let mut added = 0u64;
            for p in &batch {
                if !have.contains_key(&p.i)
                    && !new
                        .iter()
                        .any(|e| matches!(e, TransferEntry::Chunk { i, .. } if *i == p.i))
                {
                    new.push(TransferEntry::Chunk {
                        i: p.i,
                        len: p.len,
                        sha: p.sha.clone(),
                    });
                    added += p.len;
                }
            }
            if new.is_empty() {
                *guard = Some(cur);
                outcome = Ok(());
                break;
            }
            let mut next = cur.head.clone();
            next.bytes += added;
            match journal.append(&cur, new, next, Some(fence.clone())).await {
                Ok(Some(l)) => {
                    *guard = Some(l);
                    outcome = Ok(());
                    break;
                }
                // The pointer moved (another server process): reload, recheck.
                Ok(None) => *guard = None,
                Err(e) => {
                    outcome = Err(e);
                    break;
                }
            }
        }
        let cur = guard.as_ref();
        for p in batch {
            let reply = match &outcome {
                Ok(()) => match cur.and_then(|c| {
                    committed_map(&c.entries)
                        .get(&p.i)
                        .map(|(_, s)| (s.to_string(), c.head.n, c.head.bytes))
                }) {
                    Some((s, n, bytes)) if s == p.sha => Ok(ChunkPut {
                        already: false,
                        committed_chunks: n,
                        committed_bytes: bytes,
                    }),
                    Some(_) => Err(Error::conflict(
                        "chunk_conflict",
                        format!("chunk {} is committed with another hash", p.i),
                    )),
                    None => Err(Error::Meta("chunk not committed".into())),
                },
                Err(Error::LeaseLost) => Err(Error::LeaseLost),
                Err(e) => Err(Error::Meta(e.to_string())),
            };
            let _ = p.tx.send(reply);
        }
    }

    /// Completes the transfer once every chunk is committed, recording the
    /// root and the client's whole-file hash (§85 §6, §9). Idempotent.
    pub async fn complete(
        &self,
        ns: NamespaceId,
        tid: &str,
        epoch: u64,
        file_sha256: &str,
    ) -> Result<TransferView> {
        keys::check_sha256("file_sha256", file_sha256)?;
        let spec = self.spec(ns, tid).await?;
        let slot = self.slot(ns, tid);
        let journal = self.journal(ns, tid);
        {
            let mut guard = slot.loaded.lock().await;
            for _ in 0..16 {
                let cur = match guard.take() {
                    Some(c) => c,
                    None => self.loaded(ns, tid).await?,
                };
                match TransferState::parse(&cur.head.st)? {
                    TransferState::Complete => {
                        let same = cur.entries.iter().any(|e| {
                            matches!(e, TransferEntry::Complete { file_sha256: f, .. } if f == file_sha256)
                        });
                        *guard = Some(cur);
                        if !same {
                            return Err(Error::conflict(
                                "complete_conflict",
                                "the transfer completed with another file hash",
                            ));
                        }
                        break;
                    }
                    TransferState::Abandoned => {
                        return Err(Error::State(format!("transfer {tid} is abandoned")));
                    }
                    TransferState::Open => {}
                }
                let map = committed_map(&cur.entries);
                if (map.len() as u64) != spec.chunks() {
                    let n = map.len();
                    *guard = Some(cur);
                    return Err(Error::State(format!(
                        "{n} of {} chunks are committed",
                        spec.chunks()
                    )));
                }
                let r = root(map.iter().map(|(i, (len, sha))| (*i, *len, *sha)))?;
                let mut next = cur.head.clone();
                next.st = TransferState::Complete.as_str().to_string();
                let fence = Fence {
                    lease: keys::transfer_lease(ns, tid),
                    epoch,
                };
                let entry = TransferEntry::Complete {
                    root: r,
                    file_sha256: file_sha256.to_string(),
                };
                match journal.append(&cur, vec![entry], next, Some(fence)).await? {
                    Some(l) => {
                        *guard = Some(l);
                        break;
                    }
                    None => *guard = None,
                }
            }
        }
        let view = self.view(ns, tid).await?;
        if view.state == TransferState::Complete {
            self.forget(ns, tid);
        }
        Ok(view)
    }

    /// Abandons an open transfer (§85 §6). Committed bytes stay until the
    /// retention window passes (Q1513); nothing is deleted here.
    pub async fn abandon(&self, ns: NamespaceId, tid: &str) -> Result<TransferView> {
        let journal = self.journal(ns, tid);
        for _ in 0..16 {
            let Some((version, mut head)) = journal.head().await? else {
                return Err(Error::NotFound(format!("transfer {tid}")));
            };
            match TransferState::parse(&head.st)? {
                TransferState::Abandoned => break,
                TransferState::Complete => {
                    return Err(Error::State(format!("transfer {tid} is complete")));
                }
                TransferState::Open => {}
            }
            head.st = TransferState::Abandoned.as_str().to_string();
            if journal.set_head(version, &head, None).await?.is_some() {
                break;
            }
        }
        self.forget(ns, tid);
        self.view(ns, tid).await
    }

    /// A committed chunk's hash, length and bytes, streamed from the bucket.
    pub async fn open_chunk(
        &self,
        ns: NamespaceId,
        tid: &str,
        i: u32,
    ) -> Result<(String, u64, BoxStream<'static, Result<Bytes>>)> {
        let cur = self.loaded(ns, tid).await?;
        let (len, sha) = committed_map(&cur.entries)
            .get(&i)
            .map(|(len, sha)| (*len, sha.to_string()))
            .ok_or_else(|| Error::NotFound(format!("chunk {i} of transfer {tid}")))?;
        let key = keys::chunk(ns, tid, i, &sha);
        let path = object_store::path::Path::parse(&key)
            .map_err(|e| Error::Corrupt(format!("chunk key: {e}")))?;
        let got = self
            .store
            .inner()
            .get(&path)
            .await
            .map_err(|e| obj_err(&key, e))?;
        let stream = got
            .into_stream()
            .map(move |r| r.map_err(|e| obj_err(&key, e)))
            .boxed();
        Ok((sha, len, stream))
    }

    /// The namespace's transfers, oldest first (ids are ULIDs).
    pub async fn list(&self, ns: NamespaceId) -> Result<Vec<TransferSummary>> {
        let mut out = Vec::new();
        for (key, p) in self
            .ck
            .pointers_with_prefix(ns, keys::TRANSFER_POINTER_PREFIX)
            .await?
        {
            let head: Head = serde_json::from_str(&p.value)?;
            out.push(TransferSummary {
                tid: key
                    .strip_prefix(keys::TRANSFER_POINTER_PREFIX)
                    .unwrap_or(&key)
                    .to_string(),
                state: TransferState::parse(&head.st)?,
                committed_chunks: head.n,
                committed_bytes: head.bytes,
            });
        }
        Ok(out)
    }

    /// Re-reads every committed chunk and recomputes the chunk hashes, the
    /// root and the whole-file hash (`?verify=full`, §85 §9). Returns the
    /// recomputed whole-file hash.
    pub async fn verify_full(&self, ns: NamespaceId, tid: &str) -> Result<String> {
        let view = self.view(ns, tid).await?;
        if view.state != TransferState::Complete {
            return Err(Error::State(format!("transfer {tid} is not complete")));
        }
        let mut whole = Sha256::new();
        for i in 0..u32::try_from(view.chunks).map_err(|_| Error::Corrupt("chunks".into()))? {
            let (sha, len, mut s) = self.open_chunk(ns, tid, i).await?;
            let mut h = Sha256::new();
            let mut n = 0u64;
            while let Some(b) = s.next().await {
                let b = b?;
                n += b.len() as u64;
                h.update(&b);
                whole.update(&b);
            }
            if n != len || hex::encode(h.finalize()) != sha {
                return Err(Error::Integrity(format!(
                    "chunk {i} does not match its hash"
                )));
            }
        }
        let got = hex::encode(whole.finalize());
        if Some(&got) != view.file_sha256.as_ref() {
            return Err(Error::Integrity("the file hash does not match".into()));
        }
        Ok(got)
    }
}

fn obj_err(key: &str, e: object_store::Error) -> Error {
    match e {
        object_store::Error::NotFound { .. } => Error::Store(StoreError::NotFound {
            path: key.to_string(),
        }),
        other => Error::Store(StoreError::Backend(other)),
    }
}

/// A body stream that counts and hashes what passes, and fails at the end if
/// the length or hash is wrong. The failure aborts the multipart write, so a
/// bad chunk never becomes an object at its key.
struct Verified {
    inner: BoxStream<'static, io::Result<Bytes>>,
    hasher: Sha256,
    seen: u64,
    len: u64,
    sha: String,
    verdict: Arc<Mutex<Option<String>>>,
    done: bool,
}

impl Verified {
    fn fail(&mut self, msg: String) -> Poll<Option<io::Result<Bytes>>> {
        self.done = true;
        *self.verdict.lock().unwrap_or_else(|p| p.into_inner()) = Some(msg.clone());
        Poll::Ready(Some(Err(io::Error::new(io::ErrorKind::InvalidData, msg))))
    }
}

impl Stream for Verified {
    type Item = io::Result<Bytes>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.done {
            return Poll::Ready(None);
        }
        match self.inner.poll_next_unpin(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(Ok(b))) => {
                self.seen += b.len() as u64;
                if self.seen > self.len {
                    let (seen, len) = (self.seen, self.len);
                    return self.fail(format!("chunk is longer than {len} bytes ({seen} so far)"));
                }
                self.hasher.update(&b);
                Poll::Ready(Some(Ok(b)))
            }
            Poll::Ready(Some(Err(e))) => {
                self.done = true;
                Poll::Ready(Some(Err(e)))
            }
            Poll::Ready(None) => {
                if self.seen != self.len {
                    let (seen, len) = (self.seen, self.len);
                    return self.fail(format!("chunk is {seen} bytes, expected {len}"));
                }
                let got = hex::encode(std::mem::take(&mut self.hasher).finalize());
                if got != self.sha {
                    return self.fail("chunk does not match its declared SHA-256".into());
                }
                self.done = true;
                Poll::Ready(None)
            }
        }
    }
}
