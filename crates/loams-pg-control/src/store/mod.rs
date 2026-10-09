//! `PgControlStore`: where `pg-control` keeps its records (design §46 §6.2,
//! D705; PG2 Task 3).
//!
//! One implementation, [`KvControlStore`], serves both backends through
//! `loams-kv`'s store seam (ruling R3.1): [`local`] opens the embedded
//! store (MVCC on redb) for single-node mode, and [`tikv`] (feature `tikv`)
//! a `loams-tikv` handle on the metastore's keyspace and root for
//! production. [`conformance`] holds both to the same semantics
//! ([`pg_control_store_conformance!`](crate::pg_control_store_conformance)).
//!
//! **Versions.** Every record carries a version, a `u64` the store assigns
//! on each write: the writing transaction's start timestamp, which under
//! snapshot isolation grows with every committed write of a key, across a
//! delete and a re-create too. Writes are compare-and-set on it: `put` with
//! `expected: None` creates, with `Some(v)` replaces version `v`; `delete`
//! needs the current version.
//!
//! **Fences.** Every write of [`PgControlStore`] carries a [`Fence`] from
//! [`acquire_lease`](PgControlStore::acquire_lease). The write checks,
//! in its own transaction, that the lease is still at the fence's epoch and
//! held, and locks the lease record, so a takeover committed meanwhile
//! conflicts with it: `loams-meta-tikv`'s `check_fence` (R3.2). A stale
//! fence's write fails with [`StoreError::Fenced`]. Unfenced writes (the API
//! service's, which hold no project lease) exist only on [`ApiWriter`], from
//! [`KvControlStore::api_writer`], not on the trait that reconcilers take
//! (R3.10).
//!
//! **Unknown outcomes.** Writes carry a commit token, so a lost
//! acknowledgement is resolved; when it cannot be, the write fails with
//! [`StoreError::Undetermined`] and may have applied.

pub mod conformance;
mod kv;
pub mod local;
#[cfg(feature = "tikv")]
pub mod tikv;

use std::future::Future;
use std::time::Duration;

use futures::stream::BoxStream;

pub use kv::{ApiWriter, DEFAULT_POLL, KvControlStore, StoreOptions};

use crate::model::Record;

/// Every lease scope of `pg-control` starts with this: a project's is
/// `e/pg/<project_id>` ([`project_lease`](crate::model::project_lease)).
pub const LEASE_SCOPE: &str = "e/pg/";

/// The longest lease (as the metastore's `MAX_LEASE_TTL_MS`: 10 minutes).
pub const MAX_LEASE_TTL: Duration = Duration::from_secs(600);

/// The page size of a listing that names none.
pub const DEFAULT_PAGE_SIZE: u32 = 100;

/// The largest page a listing returns.
pub const MAX_PAGE_SIZE: u32 = 1000;

/// Why a store call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// The compare-and-set failed: the record is at `current` (`None`:
    /// absent).
    #[error("version conflict (current: {current:?})")]
    Conflict { current: Option<u64> },
    /// The write's fence is stale: the lease moved to another epoch or was
    /// released.
    #[error("fenced: the lease moved on")]
    Fenced,
    /// A delete found no record.
    #[error("not found")]
    NotFound,
    /// The lease is held by another holder until `deadline_ms` (the store's
    /// clock).
    #[error("lease held by {holder} until {deadline_ms}")]
    Held { holder: String, deadline_ms: u64 },
    /// [`renew_lease`](PgControlStore::renew_lease) found the lease expired,
    /// released or at another epoch.
    #[error("lease lost")]
    LeaseLost,
    /// A bad key part, page token, lease scope or TTL.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// The store did not answer, or the write did not commit (contention
    /// past the runner's attempts or deadline). Nothing was written.
    #[error("store unavailable: {0}")]
    Unavailable(String),
    /// A write's outcome is unknown: it may have applied.
    #[error("write outcome undetermined")]
    Undetermined,
    /// A stored value does not decode.
    #[error("corrupt record: {0}")]
    Corrupt(String),
}

/// A record and its version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Versioned<R> {
    pub record: R,
    pub version: u64,
}

/// One page of a listing: at most `size` records (0: [`DEFAULT_PAGE_SIZE`];
/// capped at [`MAX_PAGE_SIZE`]) after `token`, the token the previous page
/// returned.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Page {
    pub size: u32,
    pub token: Option<String>,
}

impl Page {
    /// The first page of `size` records.
    pub fn first(size: u32) -> Self {
        Page { size, token: None }
    }

    /// The page of `size` records after `token`.
    pub fn after(size: u32, token: impl Into<String>) -> Self {
        Page {
            size,
            token: Some(token.into()),
        }
    }
}

/// A lease epoch a write is fenced by: the lease `scope`, its `holder`, at
/// `epoch`. Only [`acquire_lease`](PgControlStore::acquire_lease) and
/// [`renew_lease`](PgControlStore::renew_lease) make one, so every write through
/// [`PgControlStore`] is fenced by a lease the writer took (R3.10). The API
/// service's unfenced writes go through [`ApiWriter`] instead, which the
/// trait cannot hand out.
///
/// A fence cannot be built by hand:
///
/// ```compile_fail
/// let fence = loams_pg_control::Fence {
///     scope: "e/pg/prj-1".into(),
///     holder: "pg-control-a".into(),
///     epoch: 1,
/// };
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Fence {
    scope: String,
    holder: String,
    epoch: u64,
}

impl Fence {
    pub(crate) fn new(scope: String, holder: String, epoch: u64) -> Self {
        Fence {
            scope,
            holder,
            epoch,
        }
    }

    /// The lease scope (`e/pg/<project_id>`).
    pub fn scope(&self) -> &str {
        &self.scope
    }

    /// Who took the lease.
    pub fn holder(&self) -> &str {
        &self.holder
    }

    /// The lease's epoch.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

/// A change a [`watch`](PgControlStore::watch) saw. Keys are relative to the
/// store's root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreEvent {
    /// A record was written at `version` (a write between two polls may
    /// show only its latest version).
    Put { key: Vec<u8>, version: u64 },
    /// A record was deleted.
    Delete { key: Vec<u8> },
    /// Every record the prefix held when the watch began has been sent as a
    /// `Put`; what follows are changes.
    Synced,
}

/// The store of `pg-control`'s records (PG2's shared contract). Every write
/// is a compare-and-set on the record's version, fenced by a lease epoch;
/// there is no unfenced write here (R3.10). Code generic over this trait,
/// as the reconcilers are, therefore cannot write without a lease:
///
/// ```compile_fail
/// use loams_pg_control::PgControlStore;
/// use loams_pg_control::model::BranchRec;
/// async fn reconcile<S: PgControlStore>(store: &S, rec: &BranchRec) {
///     let _ = store.api_writer().put(rec, None).await;
/// }
/// ```
///
/// while the fenced write compiles:
///
/// ```no_run
/// use loams_pg_control::{Fence, PgControlStore};
/// use loams_pg_control::model::BranchRec;
/// async fn reconcile<S: PgControlStore>(store: &S, rec: &BranchRec, fence: &Fence) {
///     let _ = store.put(rec, None, fence).await;
/// }
/// ```
pub trait PgControlStore: Send + Sync + 'static {
    /// The record at `key`, if any.
    fn get<R: Record>(
        &self,
        key: &R::Key,
    ) -> impl Future<Output = Result<Option<Versioned<R>>, StoreError>> + Send;

    /// Writes `rec` if its current version is `expected` (`None`: absent),
    /// and returns its new version.
    fn put<R: Record>(
        &self,
        rec: &R,
        expected: Option<u64>,
        fence: &Fence,
    ) -> impl Future<Output = Result<u64, StoreError>> + Send;

    /// Deletes the record at `key` if its version is `expected`.
    fn delete<R: Record>(
        &self,
        key: &R::Key,
        expected: u64,
        fence: &Fence,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// One page of the records under `prefix`, in key order, and the token
    /// of the next page (`None`: the last page).
    fn list<R: Record>(
        &self,
        prefix: &R::Prefix,
        page: Page,
    ) -> impl Future<Output = Result<(Vec<Versioned<R>>, Option<String>), StoreError>> + Send;

    /// Takes the lease `scope` (under [`LEASE_SCOPE`]) for `holder` for
    /// `ttl`. While `holder` holds it, this extends it at the same epoch.
    /// Otherwise (the lease free, expired or released) the holder gets the
    /// next epoch, which fences every write of the earlier one, its own
    /// earlier fences included: a holder that let its lease expire gets a
    /// new fence here, never its old one back. To keep a fence, use
    /// [`renew_lease`](Self::renew_lease).
    fn acquire_lease(
        &self,
        scope: &str,
        holder: &str,
        ttl: Duration,
    ) -> impl Future<Output = Result<Fence, StoreError>> + Send;

    /// Extends `fence`'s lease by `ttl` from now and returns the same fence,
    /// if the lease is still at its epoch, held by its holder and not
    /// expired (the metastore's `renew_lease`). Otherwise
    /// [`StoreError::LeaseLost`]: the holder must stop acting and acquire
    /// again (R3.12).
    fn renew_lease(
        &self,
        fence: &Fence,
        ttl: Duration,
    ) -> impl Future<Output = Result<Fence, StoreError>> + Send;

    /// The records under `prefix` (raw key bytes, relative to the root):
    /// every record already there as a `Put`, then [`StoreEvent::Synced`],
    /// then each change. Never ends while the store lives.
    fn watch(&self, prefix: &[u8]) -> BoxStream<'static, StoreEvent>;
}
