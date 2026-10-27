//! `loams-temporal-store`: the Temporal gateway's store (design §74 §5.5).
//!
//! The store keeps each workflow run's mutable state, its history, the
//! transactional outbox the relay drains into Resonate, and the visibility
//! row `DescribeWorkflowExecution` reads. It is protocol-agnostic: state and
//! events are opaque bytes, and an outbox request is opaque JSON. Nothing here
//! depends on the Temporal protos.
//!
//! Every state change goes through one compare-and-set transaction
//! ([`Store::commit`], §5.5 rule 1): the run's row is locked, its version and
//! next event ID are checked, and the events, the new state, the outbox rows
//! and the visibility row are written together or not at all.
//!
//! Around the commit path the store also offers what a production gateway
//! needs (D2060–D2065): opaque, run-bound history page tokens
//! ([`Store::history_page`]), the run listing with keyset page tokens
//! ([`Store::list_runs`]), retention of closed runs and of dispatched outbox
//! rows ([`Store::purge_expired`], driven by [`retention::spawn_sweeper`]),
//! and shard ownership for several relays ([`owned_shards`]).
//!
//! Two implementations:
//! - [`MemStore`]: in process, for tests and the dev mode.
//! - [`PgStore`]: Postgres through sqlx. It needs a **session-pooled or
//!   direct** endpoint, never a transaction-mode pooler (PgDog or PgBouncer
//!   in transaction mode; §5.5 rule 5): its transactions hold row locks over
//!   several statements and the outbox wake-up uses `LISTEN`. Never Loams SQL
//!   or MySQL (§5.5 rule 6).

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

mod mem;
mod page;
mod pg;
pub mod retention;
mod shard;

pub use mem::MemStore;
pub use page::MAX_PAGE_SIZE;
pub use pg::{OUTBOX_CHANNEL, PgStore, PgStoreOptions, SCHEMA_VERSION};
pub use shard::{DEFAULT_SHARDS, MAX_SHARDS, owned_shards, shard_of};

/// The status value the store treats as "running" in `current_runs`
/// (`temporal.api.enums.v1.WorkflowExecutionStatus.RUNNING`). Every other
/// value is a closed run. The store does not otherwise interpret statuses.
pub const STATUS_RUNNING: i32 = 1;

/// The first event ID of a run (Temporal's history starts at 1).
pub const FIRST_EVENT_ID: i64 = 1;

/// One run of one workflow.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RunKey {
    pub namespace_id: String,
    pub workflow_id: String,
    pub run_id: String,
}

impl RunKey {
    pub fn new(
        namespace_id: impl Into<String>,
        workflow_id: impl Into<String>,
        run_id: impl Into<String>,
    ) -> Self {
        Self {
            namespace_id: namespace_id.into(),
            workflow_id: workflow_id.into(),
            run_id: run_id.into(),
        }
    }
}

/// A Temporal namespace, bound to one Loams namespace.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NamespaceRecord {
    pub namespace_id: String,
    pub loams_namespace_id: String,
    /// Unique within its Loams namespace.
    pub name: String,
    pub retention_secs: i64,
    pub state: i32,
    pub config: serde_json::Value,
}

/// One history event, stored as an opaque blob.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventRow {
    pub event_id: i64,
    /// The first event of the batch this event was written in. It lies in
    /// the same commit: `commit's first event ≤ batch_first_event_id ≤ event_id`.
    pub batch_first_event_id: i64,
    pub event_type: i32,
    pub blob: Vec<u8>,
}

/// An outbox row a commit writes.
#[derive(Clone, Debug, PartialEq)]
pub struct OutboxInsert {
    pub kind: String,
    /// The deterministic Resonate ID (§74 §5.4).
    pub resonate_id: String,
    pub request: serde_json::Value,
}

/// A claimed outbox row.
#[derive(Clone, Debug, PartialEq)]
pub struct OutboxRow {
    pub id: i64,
    pub shard_id: u32,
    pub key: RunKey,
    pub kind: String,
    pub resonate_id: String,
    pub request: serde_json::Value,
    /// Claims so far, this one included (1 on the first claim).
    pub attempts: i32,
}

/// The visibility row of a run: the columns `DescribeWorkflowExecution` reads.
/// Times are Unix milliseconds.
#[derive(Clone, Debug, PartialEq)]
pub struct VisibilityRow {
    pub workflow_type: String,
    pub status: i32,
    pub start_time_ms: i64,
    pub close_time_ms: Option<i64>,
    pub execution_time_ms: i64,
    pub task_queue: String,
    pub memo: Option<Vec<u8>>,
    pub search_attrs: serde_json::Value,
}

/// One state change of a run.
#[derive(Clone, Debug, PartialEq)]
pub struct Commit {
    /// Contiguous, starting at the run's stored next event ID (empty for a
    /// state-only change).
    pub events: Vec<EventRow>,
    pub state_blob: Vec<u8>,
    pub status: i32,
    /// Must equal the stored next event ID plus `events.len()`.
    pub next_event_id: i64,
    pub outbox: Vec<OutboxInsert>,
    /// Upserted (replaced whole) when present.
    pub visibility: Option<VisibilityRow>,
}

/// The first commit of a new run.
#[derive(Clone, Debug, PartialEq)]
pub struct StartExecution {
    pub key: RunKey,
    pub request_id: String,
    /// Its events start at [`FIRST_EVENT_ID`].
    pub commit: Commit,
}

/// What [`Store::start`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartOutcome {
    /// The run was created at `version`.
    Started { version: u64 },
    /// Nothing was written. The workflow ID's current run is `run_id`, which
    /// was started by `request_id`. Returned when that run is still running,
    /// and also when it is closed but `request_id` equals the new request's
    /// (a retried start). Same `request_id` on a running run is an idempotent
    /// retry; the caller decides what to answer.
    AlreadyStarted {
        run_id: String,
        request_id: String,
        running: bool,
    },
}

/// The current run of a workflow ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CurrentRun {
    pub run_id: String,
    pub status: i32,
    pub request_id: String,
}

/// A run's mutable state as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loaded {
    pub state_blob: Vec<u8>,
    pub version: u64,
    pub next_event_id: i64,
    pub status: i32,
}

/// One page of a run's history ([`Store::history_page`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryPage {
    pub events: Vec<EventRow>,
    /// Opaque; empty when this page is the last one written so far. The
    /// token is bound to the run: another run refuses it.
    pub next_page_token: Vec<u8>,
}

/// Which runs [`Store::list_runs`] returns. Every set field must match.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunFilter {
    /// `Some(true)`: running only; `Some(false)`: closed only.
    pub open: Option<bool>,
    pub workflow_id: Option<String>,
    pub workflow_type: Option<String>,
}

/// A [`Store::list_runs`] request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListRuns {
    pub namespace_id: String,
    pub filter: RunFilter,
    /// 1..=[`MAX_PAGE_SIZE`].
    pub page_size: usize,
    /// Empty for the first page; otherwise a previous page's token, with the
    /// same namespace and filter.
    pub page_token: Vec<u8>,
}

/// A run in a listing, with its visibility row.
#[derive(Clone, Debug, PartialEq)]
pub struct ListedRun {
    pub key: RunKey,
    pub visibility: VisibilityRow,
}

/// One page of [`Store::list_runs`]: newest start first, then by workflow ID
/// and run ID.
#[derive(Clone, Debug, PartialEq)]
pub struct RunPage {
    pub runs: Vec<ListedRun>,
    /// Opaque; empty on the last page.
    pub next_page_token: Vec<u8>,
}

/// How much one [`Store::purge_expired`] call may delete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PurgeLimits {
    /// Closed runs past their namespace's retention, oldest close first.
    pub max_runs: usize,
    /// Outbox rows dispatched more than `dispatched_outbox_ttl` ago.
    pub max_outbox_rows: usize,
    pub dispatched_outbox_ttl: Duration,
}

impl Default for PurgeLimits {
    fn default() -> Self {
        Self {
            max_runs: 100,
            max_outbox_rows: 1000,
            dispatched_outbox_ttl: Duration::from_secs(24 * 3600),
        }
    }
}

/// What one [`Store::purge_expired`] call deleted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PurgeReport {
    pub runs: u64,
    pub events: u64,
    /// Outbox rows of the purged runs (all dispatched: a run with an
    /// undispatched row is never purged).
    pub run_outbox_rows: u64,
    /// Dispatched outbox rows past the TTL, of runs still kept.
    pub dispatched_outbox_rows: u64,
}

impl PurgeReport {
    /// Whether a limit was reached, so another call may find more.
    pub fn saturated(&self, l: &PurgeLimits) -> bool {
        (l.max_runs > 0 && self.runs >= l.max_runs as u64)
            || (l.max_outbox_rows > 0 && self.dispatched_outbox_rows >= l.max_outbox_rows as u64)
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// The run's version moved on: reload and apply again, or reject.
    #[error("version conflict: expected {expected}, stored {actual}")]
    Conflict { expected: u64, actual: u64 },
    /// The commit's events do not continue the stored history.
    #[error("events are not contiguous: expected event {expected}, got {got}")]
    NonContiguous { expected: i64, got: i64 },
    #[error("namespace already exists")]
    NamespaceExists,
    /// [`Store::start`] for a run key that already has a run (a reused run ID).
    #[error("run already exists")]
    RunExists,
    #[error("not found")]
    NotFound,
    /// The database was created with another shard count (X1.R6).
    #[error("the database has {stored} shards, the store was configured with {configured}")]
    ShardCountMismatch { stored: u32, configured: u32 },
    /// A malformed request: a bad commit shape, a zero shard count, a string
    /// Postgres cannot store (NUL).
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// The database cannot be reached or the request timed out; retryable.
    #[error("store unavailable: {0}")]
    Unavailable(String),
    /// Stored data the store cannot read back.
    #[error("store data corrupt: {0}")]
    Corrupt(String),
    /// Any other database error.
    #[error("database error: {0}")]
    Database(String),
}

/// The store the Temporal gateway's engine runs on.
#[async_trait]
pub trait Store: Send + Sync + 'static {
    /// [`StoreError::NamespaceExists`] if the ID, or the name within its Loams
    /// namespace, is taken.
    async fn create_namespace(&self, ns: NamespaceRecord) -> Result<(), StoreError>;
    async fn namespace_by_name(
        &self,
        loams_namespace_id: &str,
        name: &str,
    ) -> Result<Option<NamespaceRecord>, StoreError>;
    async fn namespace_by_id(
        &self,
        namespace_id: &str,
    ) -> Result<Option<NamespaceRecord>, StoreError>;
    /// Replaces a namespace's mutable fields (retention, state, config) by
    /// ID; its name and Loams namespace never change. [`StoreError::NotFound`]
    /// if there is no such namespace. Stores that cannot do it yet answer
    /// [`StoreError::Unavailable`] (TMP-api D2088; the Postgres store's is
    /// lane TMP-store's).
    async fn update_namespace(&self, ns: NamespaceRecord) -> Result<(), StoreError> {
        let _ = ns;
        Err(StoreError::Unavailable(
            "this store cannot update namespaces yet".into(),
        ))
    }
    /// Ordered by name.
    async fn list_namespaces(
        &self,
        loams_namespace_id: &str,
    ) -> Result<Vec<NamespaceRecord>, StoreError>;

    /// Starts a run: the current-run check, the first commit and the
    /// current-run upsert in one transaction. A closed current run is
    /// replaced (ID reuse policy "allow duplicate"); a running one, or any
    /// current run started by the same request ID, answers
    /// [`StartOutcome::AlreadyStarted`] and writes nothing. The new run's
    /// version is 1.
    async fn start(&self, s: StartExecution) -> Result<StartOutcome, StoreError>;
    async fn current_run(
        &self,
        namespace_id: &str,
        workflow_id: &str,
    ) -> Result<Option<CurrentRun>, StoreError>;
    async fn load(&self, key: &RunKey) -> Result<Option<Loaded>, StoreError>;
    /// The compare-and-set commit (§5.5 rule 1). Returns the new version
    /// (`expected_version + 1`). [`StoreError::NotFound`] if the run does not
    /// exist, [`StoreError::Conflict`] if its version is not
    /// `expected_version`, [`StoreError::NonContiguous`] if the events do not
    /// continue its history. On any error nothing is written.
    async fn commit(
        &self,
        key: &RunKey,
        expected_version: u64,
        c: Commit,
    ) -> Result<u64, StoreError>;
    /// Up to `max_events` events from `from_event_id` on, in order. An empty
    /// page past the end; [`StoreError::NotFound`] if the run does not exist.
    async fn history(
        &self,
        key: &RunKey,
        from_event_id: i64,
        max_events: usize,
    ) -> Result<Vec<EventRow>, StoreError>;

    /// Claims up to `limit` undispatched rows of `shards`, oldest first, for
    /// `lease`. A row whose lease expired is claimable again (at least once:
    /// a relay crash never strands a row); each claim adds one to `attempts`.
    /// No transaction is held after it returns. Ordered by ID.
    async fn claim_outbox(
        &self,
        shards: &[u32],
        limit: usize,
        lease: Duration,
        owner: &str,
    ) -> Result<Vec<OutboxRow>, StoreError>;
    /// Marks rows dispatched; they are never claimed again. Unknown or
    /// already-dispatched IDs are ignored.
    async fn mark_dispatched(&self, ids: &[i64]) -> Result<(), StoreError>;
    /// A counter that moves on every commit that wrote outbox rows (and on a
    /// `LISTEN` reconnect, when notifications may have been missed). A relay
    /// waits on `changed()` between polls; it still polls on a timer.
    fn outbox_wakeups(&self) -> tokio::sync::watch::Receiver<u64>;

    async fn visibility(&self, key: &RunKey) -> Result<Option<VisibilityRow>, StoreError>;

    /// One page of a run's history: up to `page_size` events (1..=
    /// [`MAX_PAGE_SIZE`]) from where `page_token` points (the first event when
    /// it is empty). [`StoreError::InvalidArgument`] for a token that is
    /// malformed or was issued for another run; [`StoreError::NotFound`] if
    /// the run does not exist.
    async fn history_page(
        &self,
        key: &RunKey,
        page_token: &[u8],
        page_size: usize,
    ) -> Result<HistoryPage, StoreError> {
        page::check_page_size(page_size)?;
        let from = page::decode_history(key, page_token)?;
        // One more than asked, to know whether another page exists.
        let mut events = self.history(key, from, page_size + 1).await?;
        let next_page_token = if events.len() > page_size {
            events.truncate(page_size);
            let next = events.last().map_or(from, |e| e.event_id + 1);
            page::encode_history(key, next)
        } else {
            Vec::new()
        };
        Ok(HistoryPage {
            events,
            next_page_token,
        })
    }

    /// The runs of one namespace that have a visibility row, newest start
    /// first, filtered and paged by keyset (a run started while paging never
    /// shifts a page). [`StoreError::InvalidArgument`] for a bad page size or
    /// a token issued for another namespace or filter.
    async fn list_runs(&self, req: &ListRuns) -> Result<RunPage, StoreError>;

    /// Deletes, within `limits`: closed runs whose close time plus their
    /// namespace's retention has passed and that have no undispatched outbox
    /// row (each with its history, visibility, outbox rows, and its
    /// current-run row if it is still the current run); then dispatched
    /// outbox rows older than the TTL. Safe to run from several replicas at
    /// once: a run being committed is skipped, never waited for.
    async fn purge_expired(&self, limits: &PurgeLimits) -> Result<PurgeReport, StoreError>;
}

/// Checks a commit's shape against the run's stored next event ID.
pub(crate) fn check_commit(c: &Commit, stored_next: i64) -> Result<(), StoreError> {
    for (i, e) in c.events.iter().enumerate() {
        let expected = stored_next + i as i64;
        if e.event_id != expected {
            return Err(StoreError::NonContiguous {
                expected,
                got: e.event_id,
            });
        }
        if e.batch_first_event_id < stored_next || e.batch_first_event_id > e.event_id {
            return Err(StoreError::InvalidArgument(format!(
                "event {}: batch_first_event_id {} is outside [{}, {}]",
                e.event_id, e.batch_first_event_id, stored_next, e.event_id
            )));
        }
    }
    let want = stored_next + c.events.len() as i64;
    if c.next_event_id != want {
        return Err(StoreError::InvalidArgument(format!(
            "next_event_id {} after {} events from {}; expected {}",
            c.next_event_id,
            c.events.len(),
            stored_next,
            want
        )));
    }
    Ok(())
}

pub(crate) fn check_shards(shards: u32) -> Result<(), StoreError> {
    if shards == 0 || shards > MAX_SHARDS {
        return Err(StoreError::InvalidArgument(format!(
            "shard count {shards} is outside 1..={MAX_SHARDS}"
        )));
    }
    Ok(())
}
