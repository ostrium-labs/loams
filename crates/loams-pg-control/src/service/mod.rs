//! `pg-control`'s API service: the project and branch RPCs of
//! `loams.postgres.v1` (design §46 §4; PG2 Task 5).
//!
//! The service writes records and answers; it changes no storage. A create
//! writes its record in state `creating` with a `Pending` operation, and a
//! delete marks the record `deleting` with one; the project's reconciler
//! (Task 7) does the Neon calls under its lease and moves both on. The
//! service holds the store's [`ApiWriter`], the only unfenced writer (R3.10),
//! and reads Neon's components through [`NeonRead`] only to resolve a branch
//! point and to report WAL heads.
//!
//! **One transaction per mutation.** Every mutation's records, its operation
//! and its idempotency entry go into one [`Batch`]: all apply or none. Name
//! uniqueness is a name-index record created in the same batch; a write
//! another must not miss (a child against its parent's delete) rewrites the
//! record both sides touch, so snapshot isolation sees the conflict.
//!
//! **Idempotency** ([`IdempotencyLedger`]): `(principal, rpc, key)` maps to
//! the first answer for [`LEDGER_TTL`]. A replay answers it; the same key on
//! another request is `invalid_argument`. A write whose outcome is unknown
//! (`Undetermined`), or that conflicts at the store, is resolved through the
//! ledger: if the entry is there the write was this call's (or a concurrent
//! call's under the same key), and its answer is answered (R3.14).
//!
//! The request and answer types are plain Rust; Task 9 maps them to and
//! from `loams.postgres.v1`'s messages, and its `Authorizer` decides
//! [`Caller::admin`].

pub mod branches;
mod idempotency;
pub mod operations;
pub mod projects;

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::future::BoxFuture;
use serde::Serialize;
use serde::de::DeserializeOwned;

pub use idempotency::{IdempotencyLedger, LEDGER_TTL};

use crate::neon::NeonRead;
use crate::store::{
    ApiWriter, Batch, BatchError, DEFAULT_PAGE_SIZE, KvControlStore, Page, StoreError,
};
use idempotency::{Begin, Claim};

/// How many times a mutation is redone after its batch met a concurrent
/// write, before answering `aborted`.
const ATTEMPTS: usize = 5;

/// Who calls, as the authorizer (Task 9) established it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    /// The authenticated principal: the idempotency ledger's scope.
    pub principal: String,
    /// The `admin` relation on the project (§46 §4.3): needed to delete,
    /// or lift the protection of, a protected branch.
    pub admin: bool,
}

/// Milliseconds since the Unix epoch, from a clock tests can drive.
#[derive(Clone)]
pub struct Clock(Arc<dyn Fn() -> u64 + Send + Sync>);

impl Clock {
    /// The system clock.
    pub fn system() -> Self {
        Clock(Arc::new(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        }))
    }

    /// A clock that reads `f`.
    pub fn from_fn(f: impl Fn() -> u64 + Send + Sync + 'static) -> Self {
        Clock(Arc::new(f))
    }

    /// Now, in ms since the Unix epoch.
    pub fn now_ms(&self) -> u64 {
        (self.0)()
    }
}

impl fmt::Debug for Clock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Clock")
    }
}

/// The service's defaults and limits.
#[derive(Clone)]
pub struct ServiceConfig {
    /// The Postgres major a create without one gets (Task 0 ruling 7: 17).
    pub default_pg_version: u32,
    /// The majors a project may have.
    pub pg_versions: Vec<u32>,
    pub default_region: String,
    /// The `loams-wal` pool a create without one gets.
    pub default_wal_pool: String,
    /// §46 §10: 7 days.
    pub default_history_retention: Duration,
    /// A longer retention is clamped to this (§46 §14's limit; Task 47
    /// takes it from the namespace's limits record).
    pub max_history_retention: Duration,
    pub clock: Clock,
    /// A test seam: awaited with the RPC's name after a mutation has read
    /// what it needs and before its batch commits, so a test can commit a
    /// competing write at exactly that point. `None` in production.
    pub before_commit: Option<BeforeCommit>,
}

/// See [`ServiceConfig::before_commit`].
pub type BeforeCommit = Arc<dyn Fn(&'static str) -> BoxFuture<'static, ()> + Send + Sync>;

impl fmt::Debug for ServiceConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServiceConfig")
            .field("default_pg_version", &self.default_pg_version)
            .field("pg_versions", &self.pg_versions)
            .field("default_region", &self.default_region)
            .field("default_wal_pool", &self.default_wal_pool)
            .field("default_history_retention", &self.default_history_retention)
            .field("max_history_retention", &self.max_history_retention)
            .field("before_commit", &self.before_commit.is_some())
            .finish_non_exhaustive()
    }
}

impl Default for ServiceConfig {
    fn default() -> Self {
        ServiceConfig {
            default_pg_version: 17,
            pg_versions: vec![17],
            default_region: "local".into(),
            default_wal_pool: "default".into(),
            default_history_retention: Duration::from_secs(7 * 24 * 3600),
            max_history_retention: Duration::from_secs(30 * 24 * 3600),
            clock: Clock::system(),
            before_commit: None,
        }
    }
}

/// A registered reason (`docs/api/reasons.md`): §46 §4.1's, then the
/// generic ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reason {
    ProjectNotFound,
    BranchHasChildren,
    BranchProtected,
    LsnOutOfRetention,
    StorageUnavailable,
    InvalidArgument,
    NotFound,
    AlreadyExists,
    PermissionDenied,
    Unauthenticated,
    FailedPrecondition,
    ResourceExhausted,
    Aborted,
    Unavailable,
    Internal,
}

impl Reason {
    /// The reason as registered.
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::ProjectNotFound => "project_not_found",
            Reason::BranchHasChildren => "branch_has_children",
            Reason::BranchProtected => "branch_protected",
            Reason::LsnOutOfRetention => "lsn_out_of_retention",
            Reason::StorageUnavailable => "storage_unavailable",
            Reason::InvalidArgument => "invalid_argument",
            Reason::NotFound => "not_found",
            Reason::AlreadyExists => "already_exists",
            Reason::PermissionDenied => "permission_denied",
            Reason::Unauthenticated => "unauthenticated",
            Reason::FailedPrecondition => "failed_precondition",
            Reason::ResourceExhausted => "resource_exhausted",
            Reason::Aborted => "aborted",
            Reason::Unavailable => "unavailable",
            Reason::Internal => "internal",
        }
    }

    /// The Connect code it is registered with.
    pub fn code(self) -> &'static str {
        match self {
            Reason::ProjectNotFound | Reason::NotFound => "not_found",
            Reason::BranchHasChildren
            | Reason::BranchProtected
            | Reason::LsnOutOfRetention
            | Reason::FailedPrecondition => "failed_precondition",
            Reason::StorageUnavailable | Reason::Unavailable => "unavailable",
            Reason::InvalidArgument => "invalid_argument",
            Reason::AlreadyExists => "already_exists",
            Reason::PermissionDenied => "permission_denied",
            Reason::Unauthenticated => "unauthenticated",
            Reason::ResourceExhausted => "resource_exhausted",
            Reason::Aborted => "aborted",
            Reason::Internal => "internal",
        }
    }

    /// The reason registered as `name`, or `internal` for a name this
    /// service does not raise.
    pub fn from_name(name: &str) -> Reason {
        [
            Reason::ProjectNotFound,
            Reason::BranchHasChildren,
            Reason::BranchProtected,
            Reason::LsnOutOfRetention,
            Reason::StorageUnavailable,
            Reason::InvalidArgument,
            Reason::NotFound,
            Reason::AlreadyExists,
            Reason::PermissionDenied,
            Reason::Unauthenticated,
            Reason::FailedPrecondition,
            Reason::ResourceExhausted,
            Reason::Aborted,
            Reason::Unavailable,
        ]
        .into_iter()
        .find(|r| r.as_str() == name)
        .unwrap_or(Reason::Internal)
    }
}

/// A refused or failed call: a reason, a message for the caller and the
/// reason's metadata. Never carries a secret or an internal address.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}: {message}", reason.as_str())]
pub struct ServiceError {
    pub reason: Reason,
    pub message: String,
    pub metadata: BTreeMap<String, String>,
}

impl ServiceError {
    pub fn new(reason: Reason, message: impl Into<String>) -> Self {
        ServiceError {
            reason,
            message: message.into(),
            metadata: BTreeMap::new(),
        }
    }

    /// Adds a metadata entry.
    pub fn with(mut self, key: &str, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// `invalid_argument` on `field`.
    pub fn invalid(field: &str, message: impl Into<String>) -> Self {
        ServiceError::new(Reason::InvalidArgument, message).with("field", field)
    }

    /// `not_found` for a `kind` named `name`.
    pub fn not_found(kind: &str, name: &str) -> Self {
        ServiceError::new(Reason::NotFound, format!("no {kind} {name}"))
            .with("kind", kind)
            .with("name", name)
    }

    /// `project_not_found`.
    pub fn project_not_found(project_id: &str) -> Self {
        ServiceError::new(
            Reason::ProjectNotFound,
            format!("no project {project_id} in this namespace"),
        )
        .with("project", project_id)
    }

    pub fn failed_precondition(message: impl Into<String>) -> Self {
        ServiceError::new(Reason::FailedPrecondition, message)
    }

    fn neon(e: &crate::neon::NeonApiError) -> Self {
        tracing::warn!(reason = e.reason.as_str(), component = ?e.component, error = %e.message, "a Neon component refused");
        let out = ServiceError::new(e.reason, format!("storage answered {}", e.reason.as_str()));
        match (e.component, e.reason) {
            (Some(c), Reason::StorageUnavailable) => out.with("component", c.as_str()),
            _ => out,
        }
    }
}

impl From<StoreError> for ServiceError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::Conflict { .. } => {
                ServiceError::new(Reason::Aborted, "a concurrent write won; retry")
            }
            StoreError::NotFound => ServiceError::new(Reason::NotFound, "not found"),
            StoreError::InvalidArgument(m) => ServiceError::new(Reason::InvalidArgument, m),
            StoreError::Unavailable(m) => {
                tracing::warn!(error = %m, "the control store is unavailable");
                ServiceError::new(Reason::Unavailable, "the control store is unavailable")
            }
            StoreError::Undetermined => ServiceError::new(
                Reason::Unavailable,
                "the write's outcome is unknown; retry with the same idempotency_key",
            ),
            other => {
                tracing::error!(error = %other, "pg-control store failure");
                ServiceError::new(Reason::Internal, "internal error")
            }
        }
    }
}

/// Makes a mutation's answer from its batch's outcomes.
pub(crate) type Answer<T> = Arc<dyn Fn(&[Option<u64>]) -> T + Send + Sync>;

/// What a mutation writes: its batch, how to answer once the batch's
/// outcomes (versions) are known, and what a conflict at one of its
/// operations means (after any other conflict the call reads again and
/// retries).
pub(crate) struct Mutation<T> {
    pub batch: Batch,
    pub answer: Answer<T>,
    pub on_conflict: Vec<(usize, ServiceError)>,
}

impl<T> Mutation<T> {
    pub fn new(batch: Batch, answer: impl Fn(&[Option<u64>]) -> T + Send + Sync + 'static) -> Self {
        Mutation {
            batch,
            answer: Arc::new(answer),
            on_conflict: Vec::new(),
        }
    }

    /// A conflict at `index` answers `error`.
    pub fn conflict_means(mut self, index: usize, error: ServiceError) -> Self {
        self.on_conflict.push((index, error));
        self
    }
}

/// How [`PgService::apply`] ended.
pub(crate) enum Applied<T> {
    Done(T),
    /// A concurrent write won: redo it.
    Again,
}

/// The project and branch RPCs (see the module docs).
#[derive(Debug, Clone)]
pub struct PgService<N> {
    store: KvControlStore,
    writer: ApiWriter,
    ledger: IdempotencyLedger,
    neon: N,
    config: ServiceConfig,
}

impl<N: NeonRead> PgService<N> {
    pub fn new(store: KvControlStore, neon: N, config: ServiceConfig) -> Self {
        PgService {
            writer: store.api_writer(),
            ledger: IdempotencyLedger::new(store.clone()),
            store,
            neon,
            config,
        }
    }

    /// The idempotency ledger (for its periodic [`prune`](IdempotencyLedger::prune)).
    pub fn ledger(&self) -> &IdempotencyLedger {
        &self.ledger
    }

    pub fn config(&self) -> &ServiceConfig {
        &self.config
    }

    fn now_ms(&self) -> u64 {
        self.config.clock.now_ms()
    }

    /// The ledger's answer to a replay, or the claim a fresh call writes.
    async fn begin<T: DeserializeOwned, Q: Serialize>(
        &self,
        caller: &Caller,
        rpc: &'static str,
        key: &str,
        request: &Q,
    ) -> Result<Begin<T>, ServiceError> {
        self.ledger
            .begin(caller, rpc, key, request, self.now_ms())
            .await
    }

    /// Commits `mutation` with the ledger entry of `claim`, and answers.
    async fn apply<T>(
        &self,
        rpc: &'static str,
        claim: Option<&Claim>,
        mutation: Mutation<T>,
    ) -> Result<Applied<T>, ServiceError>
    where
        T: Serialize + DeserializeOwned + Send + 'static,
    {
        let Mutation {
            mut batch,
            answer,
            on_conflict,
        } = mutation;
        let now = self.now_ms();
        if let Some(claim) = claim {
            let answer = answer.clone();
            self.ledger
                .record(&mut batch, claim, now, move |out| answer(out))?;
        }
        if let Some(hook) = &self.config.before_commit {
            hook(rpc).await;
        }
        match self.writer.commit(batch).await {
            Ok(out) => Ok(Applied::Done(answer(&out))),
            Err(BatchError {
                index: Some(index),
                error: StoreError::Conflict { .. } | StoreError::NotFound,
            }) => {
                // Another call under this key may have won (R3.14).
                if let Some(claim) = claim
                    && let Some(first) = self.ledger.find(claim, now).await?
                {
                    return Ok(Applied::Done(first));
                }
                match on_conflict.into_iter().find(|(i, _)| *i == index) {
                    Some((_, e)) => Err(e),
                    None => Ok(Applied::Again),
                }
            }
            Err(BatchError {
                error: StoreError::Undetermined,
                ..
            }) => {
                // The write may have applied; if it did, its ledger entry
                // is there and holds the answer (R3.14).
                if let Some(claim) = claim
                    && let Ok(Some(first)) = self.ledger.find(claim, now).await
                {
                    return Ok(Applied::Done(first));
                }
                Err(StoreError::Undetermined.into())
            }
            Err(e) => Err(e.error.into()),
        }
    }

    /// `aborted` after [`ATTEMPTS`] tries.
    fn contended() -> ServiceError {
        ServiceError::new(Reason::Aborted, "too many concurrent writes; retry")
    }
}

/// The store's page for a request's `page_size` and `page_token`.
fn page(page_size: i32, page_token: &str) -> Result<Page, ServiceError> {
    let size = u32::try_from(page_size)
        .map_err(|_| ServiceError::invalid("page_size", "page_size is not negative"))?;
    let size = if size == 0 { DEFAULT_PAGE_SIZE } else { size };
    Ok(Page {
        size,
        token: (!page_token.is_empty()).then(|| page_token.to_string()),
    })
}

/// A store error of a listing: a bad token is the request's.
fn list_error(e: StoreError) -> ServiceError {
    match e {
        StoreError::InvalidArgument(m) => ServiceError::invalid("page_token", m),
        other => other.into(),
    }
}

/// A namespace names a key part: not empty, no `/`, at most 255 bytes.
fn check_namespace(namespace: &str) -> Result<(), ServiceError> {
    if namespace.is_empty()
        || namespace.contains('/')
        || namespace.len() > crate::model::MAX_PART_LEN
    {
        return Err(ServiceError::invalid(
            "namespace",
            "a namespace is 1 to 255 bytes without '/'",
        ));
    }
    Ok(())
}

/// A duration in the proto's JSON form, for error metadata: `604800s`.
fn seconds(s: u64) -> String {
    format!("{s}s")
}
