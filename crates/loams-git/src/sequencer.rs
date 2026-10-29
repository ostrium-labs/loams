//! `BucketRefLog`: the sequencer and group commit (§36 §4.4, D391; plan
//! GT1 Task 10, old GT1 Task 7).
//!
//! One sequencer task per [`BucketRefLog`] holds the state after the last
//! segment it has seen and a queue of transactions:
//!
//! 1. [`RefLog::commit`] refuses a transaction with no updates or more than
//!    `MAX_REFS_PER_TXN`, one larger than a segment, and one whose packs are
//!    not all in the `BlobStore` (checked with a HEAD before queueing).
//! 2. The loop takes a group (up to `max_group_txns`, and up to
//!    `max_segment_bytes` of encoded events), stamps it with one clock read,
//!    checks each transaction against a copy of the state with that time
//!    (earlier transactions of the group included), answers replays,
//!    rejections and mismatches at once, and appends the accepted ones as
//!    segment `seq + 1` with a fresh batch id. Each event carries the time
//!    `apply_txn` was given, so replaying the segment rebuilds the same
//!    idempotency entries (ruling T8.3).
//! 3. Committed: the state advances and every kept transaction gets its
//!    receipt. Fenced: the foreign segment and every later one are applied,
//!    and the kept transactions are checked again from scratch at the new
//!    tail, at most `max_fence_retries` fences each before `Unavailable`. An
//!    unknown outcome (the WAL's own retries ran out) answers `Unavailable`:
//!    a retry with the same key replays if the batch did land.
//! 4. One append is in flight at a time, so transactions that arrive during
//!    a PUT form the next group. The `head` hint is written by a side task,
//!    at most every `hint_every`, and once more on shutdown.
//!
//! Reads never wait for the loop: they start from the state the loop last
//! published and read the WAL forward until a segment is missing.

use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{self, BoxStream, StreamExt};
use loams_cloudevents::protobuf::encode_batch;
use loams_store::StoreError;
use rand::RngCore;
use sha2::{Digest, Sha256};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::blob::{BlobError, BlobId, BlobStore};
use crate::checkpointer::{self, CheckpointPolicy};
use crate::clock::Clock;
use crate::format::event::{TxnBody, TxnEvent, from_cloudevent, to_cloudevent};
use crate::format::pb;
use crate::format::segment::{WalBatch, WalSegment};
use crate::ids::{IdemKey, NamespaceId, RefName, RepoId, Seq, TraceParent};
use crate::limits::{
    HINT_EVERY, MAX_FENCE_RETRIES, MAX_GROUP_TXNS, MAX_REFS_PER_TXN, MAX_SEGMENT_BYTES,
};
use crate::reflog::{CommittedTxn, ReadAt, Receipt, RefError, RefLog, RefSnapshot, RefTxn};
use crate::state::{Check, RepoState};
use crate::wal::{Appended, Hint, WalError, WalStore};

/// Segments read per WAL request while catching up.
const READ_BATCH: usize = 64;
/// Transactions waiting for the loop before `commit` waits to queue.
const QUEUE_DEPTH: usize = 1024;
/// [`GitEvent`]s buffered per subscriber before it lags.
const EVENT_BUFFER: usize = 1024;
/// How often an idle [`RefLog::watch`] stream looks for other writers'
/// segments.
const WATCH_POLL: Duration = Duration::from_secs(1);

/// How a sequencer runs (old GT1 Task 7, plus `max_fence_retries`).
#[derive(Clone, Debug)]
pub struct SequencerConfig {
    /// Transactions in one group (`MAX_GROUP_TXNS`, 64).
    pub max_group_txns: usize,
    /// Encoded event bytes in one group (`MAX_SEGMENT_BYTES`, 1 MiB).
    pub max_segment_bytes: usize,
    /// How long the loop waits for more transactions before taking a group
    /// (0; for benchmarks).
    pub group_linger: Duration,
    /// The hint is written at most this often (`HINT_EVERY`, 1 s).
    pub hint_every: Duration,
    /// Fences one transaction survives before it is answered `Unavailable`
    /// (`MAX_FENCE_RETRIES`, 16).
    pub max_fence_retries: usize,
    /// When a checkpoint is written (every 256 segments or 8 MiB).
    pub checkpoint: CheckpointPolicy,
    /// `<org>/<ns>`, the events' `tenantid` (D182).
    pub tenant: String,
    /// The repository's namespace.
    pub ns: NamespaceId,
    /// The repository.
    pub repo: RepoId,
}

impl SequencerConfig {
    /// The plan's defaults for `repo` in `ns`, owned by `tenant`.
    pub fn new(tenant: impl Into<String>, ns: NamespaceId, repo: RepoId) -> Self {
        Self {
            max_group_txns: MAX_GROUP_TXNS,
            max_segment_bytes: MAX_SEGMENT_BYTES as usize,
            group_linger: Duration::ZERO,
            hint_every: HINT_EVERY,
            max_fence_retries: MAX_FENCE_RETRIES,
            checkpoint: CheckpointPolicy::default(),
            tenant: tenant.into(),
            ns,
            repo,
        }
    }
}

/// What a sequencer reports in process, for the `_git` mirror (Task 15)
/// and metrics (Task 74).
#[derive(Clone, Debug)]
pub enum GitEvent {
    /// A segment was applied: this sequencer's own commit, or another
    /// writer's segment found by a fence or while catching up.
    Committed {
        /// The segment.
        seq: Seq,
        /// Its transactions, in batch order.
        txns: Arc<Vec<CommittedTxn>>,
    },
    /// An append at `at` found another writer's segment there.
    Fenced {
        /// The contested seq.
        at: Seq,
    },
    /// A group of this many transactions was taken from the queue.
    GroupSize(usize),
}

/// What a pending item commits.
enum Work {
    /// A ref transaction.
    Ref(RefTxn),
    /// A configuration change (`HEAD`, protections) under an idempotency
    /// key; its `request_digest` is set (ruling T13.2).
    Config {
        key: IdemKey,
        change: pb::ConfigChange,
        trace: Option<TraceParent>,
    },
}

impl Work {
    fn key(&self) -> &IdemKey {
        match self {
            Work::Ref(txn) => &txn.key,
            Work::Config { key, .. } => key,
        }
    }

    fn trace(&self) -> Option<TraceParent> {
        match self {
            Work::Ref(txn) => txn.trace.clone(),
            Work::Config { trace, .. } => trace.clone(),
        }
    }

    /// The event body, in `format`.
    fn body(&self, format: pb::ObjectFormat) -> TxnBody {
        match self {
            Work::Ref(txn) => TxnBody::RefTxn(txn.to_pb(format)),
            Work::Config { change, .. } => TxnBody::Config(change.clone()),
        }
    }
}

/// The request digest of a configuration change: SHA-256 over its encoding
/// with no digest, so a retry of the same change replays and another change
/// under the key is a mismatch.
pub fn config_digest(change: &pb::ConfigChange) -> [u8; 32] {
    use prost::Message;
    let mut plain = change.clone();
    plain.request_digest.clear();
    let mut hash = Sha256::new();
    hash.update(b"config\0");
    hash.update(plain.encode_to_vec());
    hash.finalize().into()
}

/// A transaction waiting for its group.
struct Pending {
    work: Work,
    /// The encoded size of its event, as an upper bound.
    size: usize,
    /// Fences it has been through.
    fences: usize,
    reply: oneshot::Sender<Result<Receipt, RefError>>,
}

impl Pending {
    fn answer(self, result: Result<Receipt, RefError>) {
        // The caller may have gone away; its answer is then unwanted.
        let _ = self.reply.send(result);
    }
}

enum Msg {
    Commit(Box<Pending>),
    Shutdown(oneshot::Sender<()>),
}

/// What every handle shares.
struct Shared {
    wal: Arc<dyn WalStore>,
    blobs: Arc<dyn BlobStore>,
    config: SequencerConfig,
    state: watch::Receiver<Arc<RepoState>>,
    queue: mpsc::Sender<Msg>,
    events: broadcast::Sender<GitEvent>,
    /// The last snapshot handed out, by seq. A snapshot copies every ref, so
    /// building one per read made a read O(refs): 8.4 k reads/s at 10 000
    /// refs against 1.3 M at 100 (BQ1, 2026-10-28). Two states at one seq
    /// are equal (the WAL is applied deterministically), so readers at the
    /// same seq share one.
    snapshots: std::sync::RwLock<Option<Arc<RefSnapshot>>>,
}

/// The ref log of one repository over its bucket WAL (§36 §5.2): a handle
/// to one sequencer task. Clones share the sequencer.
#[derive(Clone)]
pub struct BucketRefLog {
    shared: Arc<Shared>,
}

impl fmt::Debug for BucketRefLog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BucketRefLog")
            .field("ns", &self.shared.config.ns)
            .field("repo", &self.shared.config.repo)
            .field("seq", &self.seq())
            .finish()
    }
}

impl BucketRefLog {
    /// Loads the repository (the latest checkpoint, by the hint or a
    /// listing, then every segment after it) and starts its sequencer.
    /// `NotFound` when the repository has no checkpoint.
    pub async fn open(
        wal: Arc<dyn WalStore>,
        blobs: Arc<dyn BlobStore>,
        config: SequencerConfig,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, RefError> {
        let checkpoint = wal
            .latest_checkpoint(Seq(u64::MAX))
            .await?
            .ok_or(RefError::NotFound)?;
        let checkpoint_seq = Arc::new(AtomicU64::new(checkpoint.seq));
        let mut state = RepoState::from_checkpoint(checkpoint)?;
        checkpointer::replay(wal.as_ref(), &mut state, None).await?;
        let state = Arc::new(state);
        let (publish, state_rx) = watch::channel(Arc::clone(&state));
        let (queue, rx) = mpsc::channel(QUEUE_DEPTH);
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        let (hint_tx, hint_rx) = watch::channel(None);
        let hint_task = tokio::spawn(publish_hints(Arc::clone(&wal), hint_rx, config.hint_every));
        let sequencer = Sequencer {
            wal: Arc::clone(&wal),
            config: config.clone(),
            clock,
            state,
            publish,
            events: events.clone(),
            rx,
            queue: VecDeque::new(),
            checkpoint_seq,
            since_checkpoint: (0, 0),
            checkpoint_task: None,
            hint_tx: Some(hint_tx),
            hint_task: Some(hint_task),
        };
        tokio::spawn(sequencer.run());
        Ok(Self {
            shared: Arc::new(Shared {
                wal,
                blobs,
                config,
                state: state_rx,
                queue,
                events,
                snapshots: std::sync::RwLock::new(None),
            }),
        })
    }

    /// Stops the sequencer: the group in flight finishes, queued
    /// transactions are answered `Unavailable`, the hint is written, and
    /// later commits on any clone are `Unavailable`.
    pub async fn shutdown(self) {
        let (ack, done) = oneshot::channel();
        if self.shared.queue.send(Msg::Shutdown(ack)).await.is_ok() {
            // An error means the sequencer is already gone.
            let _ = done.await;
        }
    }

    /// The seq of the state the sequencer last published.
    pub fn seq(&self) -> Seq {
        self.shared.state.borrow().seq
    }

    /// The sequencer's configuration.
    pub fn config(&self) -> &SequencerConfig {
        &self.shared.config
    }

    /// The checkpoint of the state the sequencer last published, with the
    /// idempotency entries still inside their window at `now_unix_ms`.
    pub fn checkpoint(&self, now_unix_ms: i64) -> pb::Checkpoint {
        self.shared.state.borrow().to_checkpoint(now_unix_ms)
    }

    /// The sequencer's in-process events, from now on.
    pub fn events(&self) -> broadcast::Receiver<GitEvent> {
        self.shared.events.subscribe()
    }

    /// `state`'s snapshot: the cached one when it is at the same seq, else a
    /// new one, cached when it is newer than the cached one.
    fn shared_snapshot(&self, state: &RepoState) -> Arc<RefSnapshot> {
        if let Ok(cached) = self.shared.snapshots.read()
            && let Some(snap) = cached.as_ref()
            && snap.seq == state.seq
        {
            return Arc::clone(snap);
        }
        let snap = Arc::new(state.snapshot());
        if let Ok(mut cached) = self.shared.snapshots.write()
            && cached.as_ref().is_none_or(|c| c.seq < snap.seq)
        {
            *cached = Some(Arc::clone(&snap));
        }
        snap
    }

    fn current(&self) -> Arc<RepoState> {
        Arc::clone(&self.shared.state.borrow())
    }

    /// The state after reading the WAL forward from `base` until a segment
    /// is missing, or until `stop` is reached.
    async fn read_forward(
        &self,
        base: Arc<RepoState>,
        stop: Option<Seq>,
    ) -> Result<Arc<RepoState>, RefError> {
        let mut state: Option<RepoState> = None;
        loop {
            let at = state.as_ref().map_or(base.seq, |s| s.seq);
            let want = match stop {
                Some(stop) if at >= stop => break,
                Some(stop) => {
                    usize::try_from(stop.0 - at.0).map_or(READ_BATCH, |n| n.min(READ_BATCH))
                }
                None => READ_BATCH,
            };
            let segments = self.shared.wal.read_from(at.next(), want).await?;
            if segments.is_empty() {
                break;
            }
            let state = state.get_or_insert_with(|| (*base).clone());
            for segment in &segments {
                state.apply_segment(segment)?;
            }
        }
        Ok(state.map_or(base, Arc::new))
    }

    /// The encoded size of `work`'s event, as an upper bound in any
    /// segment; `TooLarge` past a segment.
    fn event_size(&self, work: &Work) -> Result<usize, RefError> {
        let config = &self.shared.config;
        let format = self.shared.state.borrow().format;
        // The time is fixed width and the seq is the longest, so this is an
        // upper bound of the event's size in any segment.
        let probe = TxnEvent {
            ns: config.ns,
            repo: config.repo.clone(),
            tenant: config.tenant.clone(),
            key: work.key().clone(),
            trace: work.trace(),
            time_unix_ms: 0,
            body: work.body(format),
        };
        let size = encode_batch(&[to_cloudevent(&probe, Seq(u64::MAX))]).len();
        if size > config.max_segment_bytes {
            return Err(RefError::TooLarge);
        }
        Ok(size)
    }

    /// Queues `work` and waits for its answer.
    async fn submit(&self, work: Work, size: usize) -> Result<Receipt, RefError> {
        let (reply, answer) = oneshot::channel();
        let pending = Pending {
            work,
            size,
            fences: 0,
            reply,
        };
        self.shared
            .queue
            .send(Msg::Commit(Box::new(pending)))
            .await
            .map_err(|_| RefError::Unavailable)?;
        answer.await.unwrap_or(Err(RefError::Unavailable))
    }

    /// Commits a configuration change: its symbolic refs and protections
    /// replace the repository's, at one seq, through group commit (§36 §4.3's
    /// `ConfigChange`; GT1 Task 13). Idempotent on `key` as a ref
    /// transaction is: a retry of the same change inside the window answers
    /// the first receipt, another change under the key is
    /// `IdempotencyMismatch` (ruling T13.2). The change's `request_digest`
    /// binds the key: a caller that derives the change from current state
    /// sets it from its own request (so a retry after another change still
    /// replays); an empty one is [`config_digest`] of the change. A symbolic
    /// ref whose name or target is not a ref name, or a digest that is not 32
    /// bytes, is `Invalid`.
    pub async fn commit_config(
        &self,
        key: IdemKey,
        mut change: pb::ConfigChange,
    ) -> Result<Receipt, RefError> {
        for symref in &change.symrefs {
            let name_ok = symref.name == "HEAD" || RefName::parse(&symref.name).is_ok();
            if !name_ok || RefName::parse(&symref.target).is_err() {
                return Err(RefError::Invalid {
                    reason: format!(
                        "symbolic ref {:?} -> {:?} is not a ref name",
                        symref.name, symref.target
                    ),
                });
            }
        }
        match change.request_digest.len() {
            0 => change.request_digest = config_digest(&change).to_vec(),
            32 => {}
            n => {
                return Err(RefError::Invalid {
                    reason: format!("a request digest of {n} bytes"),
                });
            }
        }
        let work = Work::Config {
            key,
            change,
            trace: None,
        };
        let size = self.event_size(&work)?;
        self.submit(work, size).await
    }

    /// Checks a transaction before it is queued; returns its encoded size.
    async fn admit(&self, txn: &RefTxn) -> Result<usize, RefError> {
        if txn.updates.is_empty() || txn.updates.len() > MAX_REFS_PER_TXN {
            return Err(RefError::Invalid {
                reason: format!(
                    "a transaction has {} updates; 1 to {MAX_REFS_PER_TXN} are accepted",
                    txn.updates.len()
                ),
            });
        }
        let size = self.event_size(&Work::Ref(txn.clone()))?;
        for pack in &txn.packs {
            let id = BlobId::parse(&pack.checksum).map_err(|err| RefError::Invalid {
                reason: err.to_string(),
            })?;
            match self.shared.blobs.head(&id).await {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return Err(RefError::Wal(WalError::Store(StoreError::NotFound {
                        path: format!("packs/{id}.lpk"),
                    })));
                }
                Err(BlobError::Store(err)) => return Err(RefError::Wal(WalError::Store(err))),
                Err(err) => {
                    return Err(RefError::Invalid {
                        reason: err.to_string(),
                    });
                }
            }
        }
        Ok(size)
    }
}

#[async_trait]
impl RefLog for BucketRefLog {
    async fn commit(&self, txn: RefTxn) -> Result<Receipt, RefError> {
        let size = self.admit(&txn).await?;
        self.submit(Work::Ref(txn), size).await
    }

    async fn snapshot(&self, at: ReadAt) -> Result<Arc<RefSnapshot>, RefError> {
        let base = self.current();
        let state = match at {
            ReadAt::Latest => self.read_forward(base, None).await?,
            ReadAt::AtLeast(seq) if base.seq >= seq => base,
            ReadAt::AtLeast(seq) => {
                let state = self.read_forward(base, None).await?;
                if state.seq < seq {
                    return Err(RefError::Invalid {
                        reason: format!("seq {seq} is not committed; the tail is {}", state.seq),
                    });
                }
                state
            }
            ReadAt::Exactly(seq) if seq == base.seq => base,
            ReadAt::Exactly(seq) if seq > base.seq => {
                let state = self.read_forward(base, Some(seq)).await?;
                if state.seq != seq {
                    return Err(RefError::Invalid {
                        reason: format!("seq {seq} is not committed; the tail is {}", state.seq),
                    });
                }
                state
            }
            ReadAt::Exactly(seq) => {
                Arc::new(checkpointer::state_at(self.shared.wal.as_ref(), seq).await?)
            }
        };
        Ok(self.shared_snapshot(&state))
    }

    fn watch(&self, from: Seq) -> BoxStream<'static, Result<CommittedTxn, RefError>> {
        let watch = Watch {
            wal: Arc::clone(&self.shared.wal),
            state: self.shared.state.clone(),
            events: Some(self.shared.events.subscribe()),
            next: from.max(Seq(1)),
            buffer: VecDeque::new(),
            done: false,
        };
        stream::unfold(watch, |mut watch| async move {
            let item = watch.next_item().await?;
            Some((item, watch))
        })
        .boxed()
    }
}

/// The transactions of a committed segment.
fn committed_txns(segment: &WalSegment) -> Result<Vec<CommittedTxn>, RefError> {
    segment
        .batch
        .events
        .iter()
        .enumerate()
        .map(|(index, event)| {
            let event = from_cloudevent(event).map_err(|err| {
                RefError::Wal(WalError::Corrupt {
                    seq: segment.seq,
                    reason: format!("event {index}: {err}"),
                })
            })?;
            Ok(CommittedTxn {
                seq: segment.seq,
                index: u32::try_from(index).unwrap_or(u32::MAX),
                event,
            })
        })
        .collect()
}

/// The state of one [`RefLog::watch`] stream.
struct Watch {
    wal: Arc<dyn WalStore>,
    state: watch::Receiver<Arc<RepoState>>,
    /// `None` once the sequencer has stopped: the stream then only polls.
    events: Option<broadcast::Receiver<GitEvent>>,
    next: Seq,
    buffer: VecDeque<CommittedTxn>,
    done: bool,
}

impl Watch {
    async fn next_item(&mut self) -> Option<Result<CommittedTxn, RefError>> {
        loop {
            if self.done {
                return None;
            }
            if let Some(txn) = self.buffer.pop_front() {
                return Some(Ok(txn));
            }
            // Read before the WAL: a seq the sequencer had applied then and
            // the WAL no longer holds was deleted by GC.
            let known = self.state.borrow().seq;
            let segments = match self.wal.read_from(self.next, READ_BATCH).await {
                Ok(segments) => segments,
                Err(err) => return Some(Err(self.fail(err.into()))),
            };
            if !segments.is_empty() {
                for segment in &segments {
                    match committed_txns(segment) {
                        Ok(txns) => self.buffer.extend(txns),
                        Err(err) => return Some(Err(self.fail(err))),
                    }
                    self.next = segment.seq.next();
                }
                continue;
            }
            if self.next <= known {
                let reason = format!("segments from seq {} are no longer retained", self.next);
                return Some(Err(self.fail(RefError::Invalid { reason })));
            }
            self.wait().await;
        }
    }

    fn fail(&mut self, err: RefError) -> RefError {
        self.done = true;
        err
    }

    /// Waits for the sequencer's next segment (taken directly when it is
    /// the next one) or for the poll interval, whichever comes first.
    async fn wait(&mut self) {
        let Some(events) = self.events.as_mut() else {
            tokio::time::sleep(WATCH_POLL).await;
            return;
        };
        let received = tokio::select! {
            received = events.recv() => received,
            () = tokio::time::sleep(WATCH_POLL) => return,
        };
        match received {
            Ok(GitEvent::Committed { seq, txns }) if seq == self.next => {
                self.buffer.extend(txns.iter().cloned());
                self.next = seq.next();
            }
            // Another event, an older or later segment, or a lag: the WAL
            // is read again.
            Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
            Err(broadcast::error::RecvError::Closed) => self.events = None,
        }
    }
}

/// Writes the newest hint it is given, at most every `every`.
async fn publish_hints(
    wal: Arc<dyn WalStore>,
    mut hints: watch::Receiver<Option<Hint>>,
    every: Duration,
) {
    // A hint that arrived while the last write was being spaced out.
    let mut waiting = false;
    loop {
        if !waiting && hints.changed().await.is_err() {
            return;
        }
        waiting = false;
        let hint = *hints.borrow_and_update();
        if let Some(hint) = hint
            && let Err(err) = wal.publish_hint(&hint).await
        {
            tracing::warn!(error = %err, seq = %hint.seq, "publishing the WAL hint failed");
        }
        let pause = tokio::time::sleep(every);
        tokio::pin!(pause);
        loop {
            tokio::select! {
                () = &mut pause => break,
                changed = hints.changed() => match changed {
                    Ok(()) => waiting = true,
                    // Closed: the sequencer writes the last hint itself.
                    Err(_) => return,
                },
            }
        }
    }
}

/// The sequencer task.
struct Sequencer {
    wal: Arc<dyn WalStore>,
    config: SequencerConfig,
    clock: Arc<dyn Clock>,
    state: Arc<RepoState>,
    publish: watch::Sender<Arc<RepoState>>,
    events: broadcast::Sender<GitEvent>,
    rx: mpsc::Receiver<Msg>,
    queue: VecDeque<Pending>,
    /// The newest checkpoint known written; the hint names it.
    checkpoint_seq: Arc<AtomicU64>,
    /// Segments and event bytes applied since the last checkpoint was
    /// started.
    since_checkpoint: (u64, u64),
    /// The checkpoint write in flight, if any.
    checkpoint_task: Option<JoinHandle<()>>,
    hint_tx: Option<watch::Sender<Option<Hint>>>,
    hint_task: Option<JoinHandle<()>>,
}

impl Sequencer {
    async fn run(mut self) {
        loop {
            if self.queue.is_empty() {
                match self.rx.recv().await {
                    Some(Msg::Commit(pending)) => self.queue.push_back(*pending),
                    Some(Msg::Shutdown(ack)) => return self.stop(Some(ack)).await,
                    // Every handle is gone.
                    None => return self.stop(None).await,
                }
            }
            if !self.config.group_linger.is_zero() {
                tokio::time::sleep(self.config.group_linger).await;
            }
            let mut shutdown = None;
            while let Ok(msg) = self.rx.try_recv() {
                match msg {
                    Msg::Commit(pending) => self.queue.push_back(*pending),
                    Msg::Shutdown(ack) => {
                        shutdown = Some(ack);
                        break;
                    }
                }
            }
            if let Some(ack) = shutdown {
                return self.stop(Some(ack)).await;
            }
            let group = self.take_group();
            self.commit_group(group).await;
        }
    }

    /// The next group: at least one transaction, at most `max_group_txns`
    /// and `max_segment_bytes` of encoded events.
    fn take_group(&mut self) -> Vec<Pending> {
        let mut group = Vec::new();
        let mut bytes = 0;
        while let Some(next) = self.queue.front() {
            let full = group.len() >= self.config.max_group_txns
                || (!group.is_empty() && bytes + next.size > self.config.max_segment_bytes);
            if full {
                break;
            }
            bytes += next.size;
            group.extend(self.queue.pop_front());
        }
        group
    }

    async fn commit_group(&mut self, mut group: Vec<Pending>) {
        let _ = self.events.send(GitEvent::GroupSize(group.len()));
        loop {
            // One clock read: every check, `apply_txn` and event time of
            // this attempt uses it (ruling T8.3).
            let now = self.clock.now_unix_ms();
            let seq = self.state.seq.next();
            let mut next = (*self.state).clone();
            // In arrival order; `None` is accepted. A rejection is answered
            // only once the state it was decided on is known to be the
            // tail: after this group's segment commits, or after a read
            // finds no segment past this state (ruling T10.2).
            let mut decided: Vec<(Pending, Option<RefError>)> = Vec::new();
            let mut events = Vec::new();
            // A key accepted earlier in this attempt is not committed yet: a
            // second transaction under it waits for the attempt's outcome,
            // then replays (or is refused) against committed state (PR #452
            // review).
            let mut in_attempt: HashSet<IdemKey> = HashSet::new();
            let mut deferred: Vec<Pending> = Vec::new();
            for pending in group.drain(..) {
                if in_attempt.contains(pending.work.key()) {
                    deferred.push(pending);
                    continue;
                }
                let check = match &pending.work {
                    Work::Ref(txn) => next.check(txn, now),
                    Work::Config { key, change, .. } => {
                        let digest: [u8; 32] = change
                            .request_digest
                            .as_slice()
                            .try_into()
                            .unwrap_or([0; 32]);
                        next.check_key(key, &digest, now).unwrap_or(Check::Accept)
                    }
                };
                let refusal = match check {
                    Check::Accept => None,
                    // A key the state holds was committed: the answer does
                    // not depend on the tail.
                    Check::Replay(receipt) => {
                        pending.answer(Ok(receipt));
                        continue;
                    }
                    Check::Mismatch => {
                        pending.answer(Err(RefError::IdempotencyMismatch));
                        continue;
                    }
                    Check::Reject(rejected) => Some(RefError::Rejected(rejected)),
                };
                if refusal.is_some() {
                    decided.push((pending, refusal));
                    continue;
                }
                let index = u32::try_from(events.len()).expect("a group is small");
                let body = pending.work.body(next.format);
                let applied = match (&pending.work, &body) {
                    (Work::Ref(txn), TxnBody::RefTxn(pb_txn)) => {
                        next.apply_txn(pb_txn, &txn.key, txn.request_digest, seq, index, now)
                    }
                    (Work::Config { key, change, .. }, _) => {
                        next.apply_config(change, key, seq, index, now)
                    }
                    (Work::Ref(_), _) => unreachable!("a ref transaction's body"),
                };
                if let Err(err) = applied {
                    pending.answer(Err(err.into()));
                    continue;
                }
                let event = TxnEvent {
                    ns: self.config.ns,
                    repo: self.config.repo.clone(),
                    tenant: self.config.tenant.clone(),
                    key: pending.work.key().clone(),
                    trace: pending.work.trace(),
                    time_unix_ms: now,
                    body,
                };
                events.push(to_cloudevent(&event, seq));
                in_attempt.insert(pending.work.key().clone());
                decided.push((pending, None));
            }
            if decided.is_empty() {
                self.requeue(deferred);
                return;
            }
            let outcome = if events.is_empty() {
                // Only rejections: they stand if no segment follows.
                match self.wal.read_from(seq, READ_BATCH).await {
                    Ok(segments) if segments.is_empty() => {
                        answer_decided(decided, None);
                        self.requeue(deferred);
                        return;
                    }
                    Ok(segments) => Ok(Appended::Fenced {
                        existing: segments.into_iter().next().expect("not empty"),
                    }),
                    Err(err) => Err(err),
                }
            } else {
                let mut batch_id = [0u8; 16];
                rand::rng().fill_bytes(&mut batch_id);
                let batch = WalBatch { batch_id, events };
                let bytes: u64 = decided
                    .iter()
                    .filter(|(_, refusal)| refusal.is_none())
                    .map(|(pending, _)| pending.size as u64)
                    .sum();
                match self.wal.append(seq, &batch).await {
                    Ok(Appended::Committed { .. }) => {
                        next.seq = seq;
                        let segment = WalSegment {
                            seq,
                            batch,
                            created_unix_ms: now,
                        };
                        let txns = committed_txns(&segment).map(Arc::new);
                        self.adopt(next);
                        if let Ok(txns) = txns {
                            let _ = self.events.send(GitEvent::Committed { seq, txns });
                        }
                        answer_decided(decided, Some(seq));
                        // After the receipts: a checkpoint never delays one.
                        self.applied(1, bytes);
                        self.requeue(deferred);
                        return;
                    }
                    other => other,
                }
            };
            match outcome {
                Ok(Appended::Committed { .. }) => unreachable!("answered above"),
                Ok(Appended::Fenced { existing }) => {
                    let _ = self.events.send(GitEvent::Fenced { at: seq });
                    if let Err(err) = self.catch_up(existing).await {
                        tracing::warn!(error = %err, seq = %seq, "catching up after a fence failed");
                        answer_all(decided.into_iter().map(|(p, _)| p), || match &err {
                            RefError::State(err) => RefError::State(err.clone()),
                            _ => RefError::Unavailable,
                        });
                        self.requeue(deferred);
                        return;
                    }
                    // Everything is decided again from scratch at the new
                    // tail, in arrival order.
                    group = decided
                        .into_iter()
                        .filter_map(|(mut pending, _)| {
                            pending.fences += 1;
                            if pending.fences > self.config.max_fence_retries {
                                pending.answer(Err(RefError::Unavailable));
                                None
                            } else {
                                Some(pending)
                            }
                        })
                        .collect();
                    group.extend(deferred);
                }
                Err(err) => {
                    // The WAL could not learn the outcome. A retry with the
                    // same key replays if the batch did land; the next append
                    // at this seq is fenced by it and applies it.
                    tracing::warn!(error = %err, seq = %seq, "a WAL request failed; answering unavailable");
                    answer_all(decided.into_iter().map(|(p, _)| p), || {
                        RefError::Unavailable
                    });
                    self.requeue(deferred);
                    return;
                }
            }
        }
    }

    /// Puts transactions deferred by an attempt back at the head of the
    /// queue, in arrival order: they form the next group.
    fn requeue(&mut self, deferred: Vec<Pending>) {
        for pending in deferred.into_iter().rev() {
            self.queue.push_front(pending);
        }
    }

    /// Applies another writer's segment `first` and every segment after it.
    async fn catch_up(&mut self, first: WalSegment) -> Result<(), RefError> {
        let mut state = (*self.state).clone();
        let mut applied = Vec::new();
        state.apply_segment(&first)?;
        applied.push(first);
        loop {
            let segments = self.wal.read_from(state.seq.next(), READ_BATCH).await?;
            if segments.is_empty() {
                break;
            }
            for segment in segments {
                state.apply_segment(&segment)?;
                applied.push(segment);
            }
        }
        self.adopt(state);
        let bytes = applied
            .iter()
            .map(|segment| encode_batch(&segment.batch.events).len() as u64)
            .sum();
        self.applied(applied.len() as u64, bytes);
        for segment in &applied {
            if let Ok(txns) = committed_txns(segment) {
                let txns = Arc::new(txns);
                let _ = self.events.send(GitEvent::Committed {
                    seq: segment.seq,
                    txns,
                });
            }
        }
        Ok(())
    }

    /// Counts `segments` segments of `bytes` event bytes applied, and
    /// starts a checkpoint of the current state when the policy says so
    /// and none is in flight.
    fn applied(&mut self, segments: u64, bytes: u64) {
        self.since_checkpoint.0 += segments;
        self.since_checkpoint.1 += bytes;
        let (segments, bytes) = self.since_checkpoint;
        if !self.config.checkpoint.is_due(segments, bytes) {
            return;
        }
        if self
            .checkpoint_task
            .as_ref()
            .is_some_and(|task| !task.is_finished())
        {
            return;
        }
        self.since_checkpoint = (0, 0);
        let checkpoint = self.state.to_checkpoint(self.clock.now_unix_ms());
        let (wal, written) = (Arc::clone(&self.wal), Arc::clone(&self.checkpoint_seq));
        self.checkpoint_task = Some(tokio::spawn(async move {
            let seq = checkpoint.seq;
            match wal.put_checkpoint(&checkpoint).await {
                Ok(()) => {
                    written.fetch_max(seq, Ordering::SeqCst);
                }
                Err(err) => {
                    tracing::warn!(error = %err, seq, "writing a checkpoint failed; the next one retries");
                }
            }
        }));
    }

    /// Makes `state` current: readers see it, and the hint follows it.
    fn adopt(&mut self, state: RepoState) {
        self.state = Arc::new(state);
        self.publish.send_replace(Arc::clone(&self.state));
        if let Some(hints) = &self.hint_tx {
            hints.send_replace(Some(self.hint()));
        }
    }

    fn hint(&self) -> Hint {
        Hint {
            seq: self.state.seq,
            checkpoint: Seq(self.checkpoint_seq.load(Ordering::SeqCst)).min(self.state.seq),
            written_unix_ms: self.clock.now_unix_ms(),
        }
    }

    /// Answers every waiting transaction `Unavailable`, writes the hint
    /// once more, and acknowledges `ack`.
    async fn stop(mut self, ack: Option<oneshot::Sender<()>>) {
        self.rx.close();
        let mut acks: Vec<oneshot::Sender<()>> = ack.into_iter().collect();
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Commit(pending) => self.queue.push_back(*pending),
                Msg::Shutdown(ack) => acks.push(ack),
            }
        }
        answer_all(self.queue.drain(..), || RefError::Unavailable);
        // The side task stops once its channel closes; the last hint is
        // written after it, so an older one never lands last.
        drop(self.hint_tx.take());
        if let Some(task) = self.checkpoint_task.take() {
            let _ = task.await;
        }
        if let Some(task) = self.hint_task.take() {
            let _ = task.await;
        }
        if self.state.seq > Seq::ZERO {
            let hint = self.hint();
            if let Err(err) = self.wal.publish_hint(&hint).await {
                tracing::warn!(error = %err, seq = %hint.seq, "publishing the final WAL hint failed");
            }
        }
        for ack in acks {
            let _ = ack.send(());
        }
    }
}

/// Answers a decided group: accepted transactions get their receipts in
/// segment `seq` (indices in batch order), refused ones their refusal.
fn answer_decided(decided: Vec<(Pending, Option<RefError>)>, seq: Option<Seq>) {
    let mut index = 0u32;
    for (pending, refusal) in decided {
        match (refusal, seq) {
            (Some(refusal), _) => pending.answer(Err(refusal)),
            (None, Some(seq)) => {
                pending.answer(Ok(Receipt {
                    seq,
                    index,
                    replayed: false,
                }));
                index += 1;
            }
            (None, None) => pending.answer(Err(RefError::Unavailable)),
        }
    }
}

fn answer_all(pending: impl IntoIterator<Item = Pending>, error: impl Fn() -> RefError) {
    for pending in pending {
        pending.answer(Err(error()));
    }
}
