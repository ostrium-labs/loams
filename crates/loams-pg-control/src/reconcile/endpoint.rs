//! The endpoint reconciler (design §46 §7.1; PG2 Task 11): it makes each
//! endpoint's computes match what the API asked for, under the project's
//! lease, through a [`ComputeRuntime`] and each compute's `compute_ctl`.
//!
//! ```text
//!   idle/suspended ──start──► starting ──compute_ctl running──► running
//!         ▲                    │  60 s, or a 3rd crash in 10 min  │ suspend
//!         │                    ▼                                  ▼
//!         └──── suspended ◄── /terminate, stop ◄────────────── suspending
//!                                    failed (until StartEndpoint)
//! ```
//!
//! - **Starting.** Once the project and the branch are ready: the
//!   endpoint's address ([`ComputeRuntime::ensure_address`], recorded in
//!   `E/`), then one batch writes the compute's `C/` record (bound, `Pending`,
//!   the signing key's id, the version of its spec's inputs) and names it on
//!   the endpoint; then the runtime starts the pinned image of the project's
//!   major with the compute's upcall token ([`ComputeKeys::issue_upcall_token`],
//!   [`COMPUTE_TOKEN_TTL`]), and `compute_ctl`'s address goes into `C/`.
//!   Passes then poll `compute_ctl`'s `/status` (every
//!   [`Computes::poll`]); `running` binds the address and marks the compute
//!   and the endpoint `running`. Not running within [`START_TIMEOUT`] of the
//!   compute's creation fails the endpoint.
//! - **Crashes.** A compute whose container exited, or whose `compute_ctl`
//!   reports `failed`, is stopped and replaced (replace, never restart);
//!   the third crash within 10 minutes ([`CRASH_WINDOW_MS`]) puts the
//!   endpoint in `failed` with the reason, until `StartEndpoint`.
//! - **Running.** Each pass checks the compute, replaces one whose upcall
//!   token expires within [`REPLACE_MARGIN`] (R8.9), and pushes `/configure`
//!   when the version of its spec's inputs moved since it got its spec: a
//!   `notify-attach` that landed after the compute read its spec (Task 8),
//!   a role, database or setting change.
//! - **Idle** (Task 12, [`crate::idle`]): each pass over a running compute
//!   also samples its activity (`compute_ctl`'s `last_active`, PgDog's
//!   clients of the endpoint); once it has been idle for the endpoint's
//!   `suspend_timeout_s` the endpoint's `desired` becomes `suspended` and it
//!   suspends, as below. A pass comes back when the suspend falls due, if
//!   that is before the resync.
//! - **Lifecycle events** ([`crate::observer`]): `Started` when a compute
//!   runs, then `Suspended` (stopped on purpose) or `Failed` (crashed, or
//!   did not start), each after the batch that records it.
//! - **Suspending** writes `suspending` (the compute's record `Stopping`, so
//!   the upcall refuses it a spec), then `/terminate`, then the runtime's
//!   `stop`, then `suspended` with no compute. A start that arrives
//!   meanwhile finishes the suspend and starts a new compute.
//! - **Restart** (`replace_compute`) stops the current compute and starts a
//!   new one behind the same address.
//! - **Removal**: an endpoint `deleting`, or any endpoint of a branch or
//!   project being deleted (R7.1's cascade), has its compute stopped, its
//!   address given up ([`ComputeRuntime::remove_address`]), and its `E/`
//!   and `C/` records removed in one batch.
//! - **Operations.** The API appends each operation to the endpoint's
//!   `open_ops`; every batch that writes the endpoint finishes the ones its
//!   new state settles ([`resolve`]): a start or restart when the endpoint
//!   runs (or fails, `compute_start_failed`), a suspend when no compute
//!   runs, a delete with the removal. An operation the endpoint's later
//!   `desired` contradicts is `aborted` (superseded).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use loams_postgres::Lsn;
pub use loams_postgres::compute_ctl::FileCacheMetrics;
use loams_postgres::compute_ctl::{
    ComputeCtlClient, ComputeCtlConfig, ComputeStatus as CtlState, TerminateMode,
};
use loams_postgres::spec::ComputeSpec;

use super::project::Snapshot;
use super::{Act, ReconcileError};
use crate::autoscale::{Scaling, Tracked};
use crate::idle::IdleMonitor;
use crate::ids::ComputeId;
use crate::model::{
    BranchState, CRASH_WINDOW_MS, ComputeBinding, ComputeKey, ComputeRec, ComputeStatus, Cu,
    DesiredState, EndpointRec, EndpointState, EndpointType, OperationError, OperationKey,
    OperationKind, OperationRec, OperationState, PoolKey, ProjectState, Record as _,
};
use crate::neon::NeonWrite;
use crate::observer::{Lifecycle, LifecycleKind};
use crate::pgdog::NoPgDog;
use crate::pool::WarmPool;
use crate::runtime::{
    BackendAddr, ComputeHandle, ComputeLaunch, ComputeRuntime, RuntimeError, RuntimeStatus,
};
use crate::service::Clock;
use crate::store::{Batch, PgControlStore, Versioned};
use crate::upcall::spec::{self, SpecError};
use crate::upcall::{COMPUTE_TOKEN_TTL, CTL_TOKEN_TTL, ComputeKeys, SpecSources};

/// How long a compute has, from its creation, to reach `running` (§46
/// §7.1).
pub const START_TIMEOUT: Duration = Duration::from_secs(60);

/// A compute is replaced this long before its upcall token
/// ([`COMPUTE_TOKEN_TTL`]) expires, so it never runs without one.
pub const REPLACE_MARGIN: Duration = Duration::from_secs(24 * 3600);

/// The crashes within [`CRASH_WINDOW_MS`] that put an endpoint in `failed`.
pub const CRASHES_TO_FAIL: usize = 3;

/// What `compute_ctl` reports, as the reconciler reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CtlStatus {
    /// Waiting for a spec (`empty`): a warm-pool compute (Task 14), or a
    /// compute that has not read its spec yet.
    Empty,
    /// Starting Postgres with its spec.
    Starting,
    /// Postgres runs (also while it applies a new spec).
    Running,
    /// Postgres did not start or stopped with an error.
    Failed(String),
    /// Terminated, or terminating.
    Terminated,
}

/// `compute_ctl`'s `last_active` (Task 12's idle detection).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    /// None since the compute started.
    NotYet,
    /// Last active then (ms since the Unix epoch, the compute's clock).
    At(u64),
    /// `compute_ctl` sent something that is not a time: unknown, so the
    /// compute is not suspended on it.
    Unreadable,
}

/// One answer of `compute_ctl`'s `/status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CtlReport {
    pub status: CtlStatus,
    pub last_active: Activity,
}

/// A call to a compute's `compute_ctl` that did not answer as asked. The
/// text is for logs only (it can name hosts).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct CtlError(pub String);

/// A compute's `compute_ctl`, as the reconciler calls it
/// ([`HttpComputeCtl`]; tests fake it).
#[async_trait]
pub trait ComputeCtl: Send + Sync + 'static {
    /// `GET /status`.
    async fn status(&self, compute: &ComputeRec) -> Result<CtlReport, CtlError>;
    /// `POST /configure` with `spec`.
    async fn configure(&self, compute: &ComputeRec, spec: &ComputeSpec) -> Result<(), CtlError>;
    /// `POST /terminate?mode=immediate`.
    async fn terminate(&self, compute: &ComputeRec) -> Result<(), CtlError>;
    /// `GET /autoscaling_metrics`: the local file cache (Task 15). Not
    /// read by default.
    async fn file_cache(&self, compute: &ComputeRec) -> Result<FileCacheMetrics, CtlError> {
        let _ = compute;
        Err(CtlError("compute_ctl's metrics are not read".into()))
    }
    /// `POST /lfc/prewarm[?from_endpoint=]` (Task 16).
    async fn prewarm(
        &self,
        compute: &ComputeRec,
        from_endpoint: Option<&str>,
    ) -> Result<(), CtlError> {
        let _ = (compute, from_endpoint);
        Err(CtlError("prewarming is not supported".into()))
    }
    /// `GET /lfc/prewarm` (Task 16).
    async fn prewarm_state(&self, compute: &ComputeRec) -> Result<Prewarm, CtlError> {
        let _ = compute;
        Err(CtlError("prewarming is not supported".into()))
    }
    /// `POST /promote` with a primary's `spec`, once the replica replayed
    /// `wal_flush_lsn` (Task 16).
    async fn promote(
        &self,
        compute: &ComputeRec,
        spec: &ComputeSpec,
        wal_flush_lsn: Lsn,
    ) -> Result<(), PromoteError> {
        let _ = (compute, spec, wal_flush_lsn);
        Err(PromoteError::Refused("promotion is not supported".into()))
    }
}

/// Why `/promote` did not answer `completed` (the text is for logs).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PromoteError {
    /// Refused before `pg_promote()`: the compute is still a replica.
    #[error("refused: {0}")]
    Refused(String),
    /// Anything else (a timeout, a lost answer, a failure after
    /// `pg_promote()`): the compute may already be a primary.
    #[error("outcome unknown: {0}")]
    Unknown(String),
}

impl PromoteError {
    /// `compute_ctl`'s answer to `/promote`, read for whether it refused
    /// before `pg_promote()` (the fork's `compute_promote.rs`: not a
    /// replica, not prewarmed or prewarming, or not caught up with the
    /// primary) or answered a client error.
    fn of(e: &loams_postgres::NeonError) -> Self {
        let before = [
            "is not replica",
            "NotPrewarmed",
            "Prewarming",
            "didn't catch up",
        ];
        if (400..500).contains(&e.status) || before.iter().any(|b| e.msg.contains(b)) {
            PromoteError::Refused(e.to_string())
        } else {
            PromoteError::Unknown(e.to_string())
        }
    }
}

/// The longest a promotion waits for `/promote`: less than the project's
/// lease (60 s), so a promotion whose outcome is unknown is still settled
/// under the lease (R16.9).
pub const PROMOTE_WAIT: Duration = Duration::from_secs(45);

/// A replica's file cache prewarm, as `GET /lfc/prewarm` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prewarm {
    /// Not asked yet (the compute's start state).
    NotPrewarmed,
    Prewarming,
    /// Ended: `completed`, `skipped` (no saved state), `failed` (for
    /// example no endpoint storage) or `cancelled`. `compute_ctl` promotes
    /// after any of them.
    Done(String),
}

impl Prewarm {
    fn of(status: &str) -> Self {
        match status {
            "not_prewarmed" => Prewarm::NotPrewarmed,
            "prewarming" => Prewarm::Prewarming,
            other => Prewarm::Done(other.to_owned()),
        }
    }
}

/// How long a promotion waits for the replica's prewarm before asking
/// `compute_ctl` anyway (which then refuses an unprewarmed replica, R2.13).
pub const PREWARM_WAIT: Duration = Duration::from_secs(120);

/// [`ComputeCtl`] over HTTP at the record's `ctl_url`, with a
/// [`CTL_TOKEN_TTL`] token per call.
#[derive(Debug)]
pub struct HttpComputeCtl {
    keys: Arc<ComputeKeys>,
    clock: Clock,
}

impl HttpComputeCtl {
    pub fn new(keys: Arc<ComputeKeys>, clock: Clock) -> Self {
        HttpComputeCtl { keys, clock }
    }

    fn client(&self, compute: &ComputeRec) -> Result<ComputeCtlClient, CtlError> {
        let fail = |what: &str| CtlError(what.to_owned());
        let url = compute
            .ctl_url
            .as_deref()
            .ok_or_else(|| fail("the compute has no compute_ctl address"))?;
        let url = url::Url::parse(url).map_err(|_| fail("the compute_ctl address is not a URL"))?;
        let id: ComputeId = compute
            .id
            .parse()
            .map_err(|_| fail("the compute id is malformed"))?;
        let token = self
            .keys
            .issue_compute_ctl_token(&id, self.clock.now_ms(), CTL_TOKEN_TTL)
            .map_err(|e| CtlError(e.to_string()))?;
        ComputeCtlClient::new(url, token).map_err(|e| CtlError(e.to_string()))
    }
}

#[async_trait]
impl ComputeCtl for HttpComputeCtl {
    async fn status(&self, compute: &ComputeRec) -> Result<CtlReport, CtlError> {
        let status = self
            .client(compute)?
            .status()
            .await
            .map_err(|e| CtlError(e.to_string()))?;
        let last_active = match status.last_active_ms() {
            Ok(Some(at)) => Activity::At(at),
            Ok(None) => Activity::NotYet,
            Err(e) => {
                tracing::warn!(compute = %compute.id, error = %e, "compute_ctl's last_active is not a time");
                Activity::Unreadable
            }
        };
        let status = match status.status {
            CtlState::Empty => CtlStatus::Empty,
            CtlState::Init | CtlState::ConfigurationPending => CtlStatus::Starting,
            CtlState::Running
            | CtlState::Configuration
            | CtlState::RefreshConfigurationPending
            | CtlState::RefreshConfiguration => CtlStatus::Running,
            CtlState::Failed => CtlStatus::Failed(status.error.unwrap_or_default()),
            CtlState::TerminationPendingFast
            | CtlState::TerminationPendingImmediate
            | CtlState::Terminated => CtlStatus::Terminated,
        };
        Ok(CtlReport {
            status,
            last_active,
        })
    }

    async fn configure(&self, compute: &ComputeRec, spec: &ComputeSpec) -> Result<(), CtlError> {
        let config = ComputeCtlConfig {
            jwks: self.keys.jwks(),
            tls: None,
        };
        self.client(compute)?
            .configure(spec, &config)
            .await
            .map(|_| ())
            .map_err(|e| CtlError(e.to_string()))
    }

    async fn terminate(&self, compute: &ComputeRec) -> Result<(), CtlError> {
        self.client(compute)?
            .terminate(TerminateMode::Immediate)
            .await
            .map(|_| ())
            .map_err(|e| CtlError(e.to_string()))
    }

    async fn file_cache(&self, compute: &ComputeRec) -> Result<FileCacheMetrics, CtlError> {
        self.client(compute)?
            .autoscaling_metrics()
            .await
            .map_err(|e| CtlError(e.to_string()))
    }

    async fn prewarm(
        &self,
        compute: &ComputeRec,
        from_endpoint: Option<&str>,
    ) -> Result<(), CtlError> {
        self.client(compute)?
            .prewarm(from_endpoint)
            .await
            .map_err(|e| CtlError(e.to_string()))
    }

    async fn prewarm_state(&self, compute: &ComputeRec) -> Result<Prewarm, CtlError> {
        self.client(compute)?
            .prewarm_state()
            .await
            .map(|s| Prewarm::of(&s.status))
            .map_err(|e| CtlError(e.to_string()))
    }

    async fn promote(
        &self,
        compute: &ComputeRec,
        spec: &ComputeSpec,
        wal_flush_lsn: Lsn,
    ) -> Result<(), PromoteError> {
        self.client(compute)
            .map_err(|e| PromoteError::Refused(e.to_string()))?
            .promote(spec, wal_flush_lsn)
            .await
            .map_err(|e| PromoteError::of(&e))
    }
}

/// What the endpoint reconciler drives computes with.
#[derive(Clone)]
pub struct Computes {
    pub runtime: Arc<dyn ComputeRuntime>,
    pub ctl: Arc<dyn ComputeCtl>,
    /// Signs each compute's upcall token; its id goes in `C/`.
    pub keys: Arc<ComputeKeys>,
    /// The compute image per Postgres major, pinned by digest
    /// ([`crate::runtime::images`]).
    pub images: BTreeMap<u32, String>,
    /// Where computes find storage: the specs the reconciler pushes are
    /// the upcall's.
    pub sources: SpecSources,
    /// [`START_TIMEOUT`] by default.
    pub start_timeout: Duration,
    /// How soon a pass comes back while a compute starts (default 500 ms).
    pub poll: Duration,
    /// Idle detection (Task 12): PgDog's clients through [`NoPgDog`] by
    /// default.
    pub idle: Arc<IdleMonitor>,
    /// Where compute lifecycle events go (Task 12): a [`DefaultObserver`]
    /// by default.
    ///
    /// [`DefaultObserver`]: crate::observer::DefaultObserver
    pub lifecycle: Lifecycle,
    /// The warm pool a start takes a compute from (Task 14); `None` by
    /// default.
    pub pool: Option<Arc<WarmPool>>,
    /// The autoscaler (Task 15): the running computes it follows, their
    /// samples and the policy. `None` (the default): computes keep their
    /// start size.
    pub scaling: Option<Arc<Scaling>>,
}

impl Computes {
    /// The defaults: [`START_TIMEOUT`], a 500 ms poll, no PgDog, the
    /// default observer.
    pub fn new(
        runtime: Arc<dyn ComputeRuntime>,
        ctl: Arc<dyn ComputeCtl>,
        keys: Arc<ComputeKeys>,
        images: BTreeMap<u32, String>,
        sources: SpecSources,
    ) -> Self {
        Computes {
            runtime,
            ctl,
            keys,
            images,
            sources,
            start_timeout: START_TIMEOUT,
            poll: Duration::from_millis(500),
            idle: Arc::new(IdleMonitor::new(Arc::new(NoPgDog))),
            lifecycle: Lifecycle::default(),
            pool: None,
            scaling: None,
        }
    }
}

impl std::fmt::Debug for Computes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Computes")
            .field("images", &self.images)
            .field("sources", &self.sources)
            .field("start_timeout", &self.start_timeout)
            .field("poll", &self.poll)
            .field("idle", &self.idle)
            .field("lifecycle", &self.lifecycle)
            .field("pool", &self.pool)
            .field("scaling", &self.scaling.is_some())
            .finish_non_exhaustive()
    }
}

/// Whether an endpoint has anything to do: it is not at rest (suspended
/// and asked to be, or failed and not asked again), has a compute, or has
/// open operations.
pub(crate) fn needs_work(ep: &EndpointRec) -> bool {
    !ep.open_ops.is_empty()
        || ep.replace_compute
        || ep.compute_id.is_some()
        || !matches!(
            (ep.desired, ep.state),
            (
                DesiredState::Suspended,
                EndpointState::Idle | EndpointState::Suspended
            ) | (DesiredState::Running, EndpointState::Failed)
        )
}

/// What one pass over a project's endpoints came to.
#[derive(Debug, Default)]
pub(crate) struct Outcome {
    /// An endpoint was removed: the project's records are to be read again.
    pub removed: bool,
    /// The soonest a pass should come back.
    pub again: Option<Duration>,
}

impl Outcome {
    fn again(&mut self, after: Duration) {
        self.again = Some(self.again.map_or(after, |a| a.min(after)));
    }
}

/// One pass over the project's endpoints. An endpoint whose step failed
/// for a reason that may pass is logged and tried again; a lost lease or a
/// record that moved ends the pass.
pub(crate) async fn reconcile<S: PgControlStore, N: NeonWrite>(
    act: &Act<'_, S, N>,
    computes: &Computes,
    snap: &Snapshot,
) -> Result<Outcome, ReconcileError> {
    let mut out = Outcome::default();
    for ep in &snap.endpoints {
        if !needs_work(&ep.record) && !doomed(snap, &ep.record) {
            continue;
        }
        let mut one = One {
            act,
            c: computes,
            snap,
            cur: ep.clone(),
            ops: Vec::new(),
            compute: None,
        };
        match one.run().await {
            Ok(Step::Removed) => out.removed = true,
            Ok(Step::Done) => {}
            Ok(Step::Again(after)) => out.again(after),
            Err(e @ (ReconcileError::Fenced | ReconcileError::Moved)) => return Err(e),
            Err(e) => {
                tracing::warn!(endpoint = %ep.record.id, error = %e, "endpoint step failed; retrying");
                out.again(act.ctx.config.retry_initial);
            }
        }
    }
    Ok(out)
}

/// Whether the endpoint's branch or project is going (R7.1's cascade).
fn doomed(snap: &Snapshot, ep: &EndpointRec) -> bool {
    snap.project.record.state == ProjectState::Deleting
        || snap
            .branch(&ep.branch_id)
            .is_none_or(|b| b.record.state == BranchState::Deleting)
}

enum Step {
    Done,
    Again(Duration),
    Removed,
}

/// What idle detection came to for a running compute.
enum Idle {
    /// It was idle: the endpoint is suspended.
    Suspended,
    /// Not idle (yet): when a pass should look again, if before the resync.
    Due(Option<Duration>),
}

/// How a batch that writes the endpoint settles its operations.
#[derive(Debug, Clone, Copy, Default)]
struct Settle {
    /// The batch just stopped a suspending endpoint's compute: its suspend
    /// operations succeed even when a start is already asked.
    suspended_now: bool,
}

/// What an open operation comes to once the endpoint is `rec`: `None` while
/// it waits, else its end (`Err`: failed with this error).
fn resolve(
    rec: &EndpointRec,
    op: &OperationRec,
    settle: Settle,
) -> Option<Result<(), OperationError>> {
    let aborted = |why: &str| {
        Some(Err(OperationError {
            reason: "aborted".into(),
            message: why.into(),
        }))
    };
    match op.kind {
        OperationKind::EndpointStart | OperationKind::EndpointRestart => {
            if rec.desired == DesiredState::Suspended {
                aborted("superseded by a suspend")
            } else if rec.state == EndpointState::Failed {
                Some(Err(OperationError {
                    reason: "compute_start_failed".into(),
                    message: rec
                        .failure
                        .clone()
                        .unwrap_or_else(|| "the compute did not start".into()),
                }))
            } else if rec.state == EndpointState::Running && !rec.replace_compute {
                Some(Ok(()))
            } else {
                None
            }
        }
        OperationKind::EndpointSuspend => {
            if settle.suspended_now {
                Some(Ok(()))
            } else if rec.desired == DesiredState::Running {
                aborted("superseded by a start")
            } else if rec.compute_id.is_none()
                && matches!(
                    rec.state,
                    EndpointState::Idle | EndpointState::Suspended | EndpointState::Failed
                )
            {
                Some(Ok(()))
            } else {
                None
            }
        }
        // Task 16: done when the types are swapped; refused when the flag
        // went without them.
        OperationKind::EndpointPromote => {
            if rec.promote {
                None
            } else if rec.kind == EndpointType::ReadWrite {
                Some(Ok(()))
            } else if rec.desired == DesiredState::Suspended || rec.state == EndpointState::Failed {
                aborted("the read-only endpoint stopped before the promotion")
            } else {
                Some(Err(OperationError {
                    reason: "failed_precondition".into(),
                    message: "the promotion did not complete: the branch's read-write endpoint \
                              runs, or compute_ctl refused (replace the replica, or prewarm it \
                              and retry)"
                        .into(),
                }))
            }
        }
        // Ended with the removal; no other kind is in `ops`.
        _ => None,
    }
}

/// Whether `kind` is an endpoint operation's.
fn is_endpoint_op(kind: OperationKind) -> bool {
    matches!(
        kind,
        OperationKind::EndpointStart
            | OperationKind::EndpointSuspend
            | OperationKind::EndpointRestart
            | OperationKind::EndpointDelete
            | OperationKind::EndpointPromote
    )
}

/// One endpoint's pass.
struct One<'a, 'b, S, N> {
    act: &'a Act<'b, S, N>,
    c: &'a Computes,
    snap: &'a Snapshot,
    /// The endpoint as this pass last wrote (or read) it.
    cur: Versioned<EndpointRec>,
    /// Its open operations, as last written.
    ops: Vec<Versioned<OperationRec>>,
    /// Its current compute's record.
    compute: Option<Versioned<ComputeRec>>,
}

fn runtime_error(what: &str, e: &RuntimeError) -> ReconcileError {
    ReconcileError::Runtime(format!("{what}: {e}"))
}

impl<S: PgControlStore, N: NeonWrite> One<'_, '_, S, N> {
    fn now(&self) -> u64 {
        self.act.ctx.now_ms()
    }

    fn ep(&self) -> &EndpointRec {
        &self.cur.record
    }

    /// The project's Loams namespace, which the runtime's calls name.
    fn ns(&self) -> &str {
        &self.snap.project.record.namespace
    }

    async fn run(&mut self) -> Result<Step, ReconcileError> {
        let store = &self.act.ctx.store;
        for id in &self.cur.record.open_ops {
            let key = OperationKey {
                project_id: self.cur.record.project_id.clone(),
                id: id.clone(),
            };
            // Only this endpoint's operations; anything else the list names
            // is dropped from it, untouched.
            if let Some(op) = store.get::<OperationRec>(&key).await?
                && is_endpoint_op(op.record.kind)
                && op.record.endpoint_id.as_deref() == Some(self.cur.record.id.as_str())
            {
                self.ops.push(op);
            }
        }
        if let Some(id) = &self.cur.record.compute_id {
            self.compute = store
                .get::<ComputeRec>(&ComputeKey { id: id.clone() })
                .await?;
        }
        if self.ep().state == EndpointState::Deleting || doomed(self.snap, self.ep()) {
            let why = if self.ep().state == EndpointState::Deleting {
                "the endpoint was deleted"
            } else {
                "the endpoint's branch or project is being deleted"
            };
            self.remove(why).await?;
            return Ok(Step::Removed);
        }
        if self.ep().replace_compute {
            self.replace("restart").await?;
        }
        match self.ep().desired {
            DesiredState::Suspended => self.suspend().await,
            DesiredState::Running => self.run_compute().await,
        }
    }

    /// Writes the endpoint as `rec` with `batch`, settling its operations;
    /// answers the batch's outcomes (the caller's operations keep their
    /// indexes: the endpoint and its operations come after them).
    async fn write(
        &mut self,
        step: &'static str,
        mut rec: EndpointRec,
        mut batch: Batch,
        settle: Settle,
    ) -> Result<Vec<Option<u64>>, ReconcileError> {
        let now = self.now();
        let mut kept = Vec::new();
        let mut moved = Vec::new();
        for op in &self.ops {
            if matches!(
                op.record.state,
                OperationState::Succeeded | OperationState::Failed
            ) {
                continue;
            }
            match resolve(&rec, &op.record, settle) {
                Some(end) => {
                    self.act
                        .finish(&mut batch, std::slice::from_ref(op), end.err().as_ref())?;
                }
                None => {
                    let mut running = op.record.clone();
                    if running.state == OperationState::Pending {
                        running.state = OperationState::Running;
                        running.updated_at_ms = now;
                        let at = batch.put(&running, Some(op.version))?;
                        moved.push((kept.len(), at));
                    }
                    kept.push(Versioned {
                        record: running,
                        version: op.version,
                    });
                }
            }
        }
        rec.open_ops = kept.iter().map(|o| o.record.id.clone()).collect();
        rec.updated_at_ms = now;
        let at = batch.put(&rec, Some(self.cur.version))?;
        let out = self.act.commit(step, batch).await?;
        for (i, at) in moved {
            kept[i].version = out[at].unwrap_or_default();
        }
        self.ops = kept;
        self.cur = Versioned {
            record: rec,
            version: out[at].unwrap_or_default(),
        };
        Ok(out)
    }

    /// Stops the current compute (with `/terminate` first when `clean`),
    /// leaving its record for the caller's batch to delete.
    async fn stop_compute(&self, clean: bool) -> Result<(), ReconcileError> {
        let Some(c) = &self.compute else {
            return Ok(());
        };
        if clean && c.record.ctl_url.is_some() {
            self.act.renew().await?;
            if let Err(e) = self.c.ctl.terminate(&c.record).await {
                tracing::info!(compute = %c.record.id, error = %e, "terminate did not answer; stopping the compute anyway");
            }
        }
        let id: ComputeId = c
            .record
            .id
            .parse()
            .map_err(|_| ReconcileError::Runtime(format!("compute id {}", c.record.id)))?;
        self.act.renew().await?;
        self.c
            .runtime
            .stop(self.ns(), &id)
            .await
            .map_err(|e| runtime_error("stopping the compute", &e))
    }

    /// A batch deleting the current compute's record, when there is one.
    fn without_compute(&self) -> Result<Batch, ReconcileError> {
        let mut batch = Batch::new();
        if let Some(c) = &self.compute {
            batch.delete::<ComputeRec>(&c.record.key(), c.version)?;
        }
        Ok(batch)
    }

    /// Sends `event` of `compute` to the lifecycle observer (Task 12), after
    /// the batch that made it true committed; an end also forgets the
    /// compute's idle state.
    fn emit(&self, compute: &ComputeRec, event: LifecycleKind) {
        let Some(binding) = &compute.binding else {
            return;
        };
        let ended = matches!(
            event,
            LifecycleKind::Suspended | LifecycleKind::Failed { .. }
        );
        self.c
            .lifecycle
            .emit(binding, &compute.id, event, self.now());
        if ended {
            self.c.idle.forget(&compute.id);
            if let Some(s) = &self.c.scaling {
                s.forget(&compute.id);
            }
        }
    }

    /// Clears the current compute, whose end (`event`) has committed.
    fn ended(&mut self, event: LifecycleKind) {
        if let Some(c) = self.compute.take() {
            self.emit(&c.record, event);
        }
    }

    /// Removes the endpoint: its compute, its address, its records.
    async fn remove(&mut self, why: &str) -> Result<(), ReconcileError> {
        self.stop_compute(true).await?;
        self.act.renew().await?;
        self.c
            .runtime
            .remove_address(self.ns(), self.ep())
            .await
            .map_err(|e| runtime_error("giving up the address", &e))?;
        let mut batch = self.without_compute()?;
        batch.delete::<EndpointRec>(&self.ep().key(), self.cur.version)?;
        let gone = OperationError {
            reason: "aborted".into(),
            message: why.into(),
        };
        for op in &self.ops {
            if matches!(
                op.record.state,
                OperationState::Succeeded | OperationState::Failed
            ) {
                continue;
            }
            let error = (op.record.kind != OperationKind::EndpointDelete).then_some(&gone);
            self.act
                .finish(&mut batch, std::slice::from_ref(op), error)?;
        }
        self.act.commit("endpoint.remove", batch).await?;
        self.ended(LifecycleKind::Suspended);
        tracing::info!(endpoint = %self.ep().id, why, "endpoint removed");
        Ok(())
    }

    /// Stops the current compute and clears it, the endpoint `starting` (or
    /// `suspended` when it should not run).
    async fn replace(&mut self, why: &'static str) -> Result<(), ReconcileError> {
        self.stop_compute(true).await?;
        let mut rec = self.ep().clone();
        rec.replace_compute = false;
        rec.compute_id = None;
        if matches!(rec.state, EndpointState::Starting | EndpointState::Running) {
            rec.state = match rec.desired {
                DesiredState::Running => EndpointState::Starting,
                DesiredState::Suspended => EndpointState::Suspended,
            };
        }
        let batch = self.without_compute()?;
        self.write("endpoint.replace", rec, batch, Settle::default())
            .await?;
        self.ended(LifecycleKind::Suspended);
        tracing::info!(endpoint = %self.ep().id, why, "compute replaced");
        Ok(())
    }

    /// Desired `suspended`.
    async fn suspend(&mut self) -> Result<Step, ReconcileError> {
        match self.ep().state {
            EndpointState::Idle | EndpointState::Suspended | EndpointState::Failed => {
                let mut rec = self.ep().clone();
                rec.promote = false;
                if rec.state == EndpointState::Failed {
                    rec.state = EndpointState::Suspended;
                    rec.failure = None;
                }
                if self.compute.is_some() || rec.compute_id.is_some() {
                    self.stop_compute(true).await?;
                    rec.compute_id = None;
                }
                let batch = self.without_compute()?;
                self.write("endpoint.at_rest", rec, batch, Settle::default())
                    .await?;
                self.ended(LifecycleKind::Suspended);
                Ok(Step::Done)
            }
            EndpointState::Starting | EndpointState::Running | EndpointState::Suspending => {
                self.stop_suspending().await?;
                Ok(Step::Done)
            }
            EndpointState::Deleting => Ok(Step::Done),
        }
    }

    /// `suspending`, then the compute stopped, then `suspended` (or
    /// `starting` when a start arrived meanwhile).
    async fn stop_suspending(&mut self) -> Result<(), ReconcileError> {
        if self.ep().state != EndpointState::Suspending {
            let mut rec = self.ep().clone();
            rec.state = EndpointState::Suspending;
            // A promotion asked of this replica ends with it.
            rec.promote = false;
            let mut batch = Batch::new();
            let stopping = self.compute.as_ref().map(|c| {
                let mut stopping = c.record.clone();
                stopping.status = ComputeStatus::Stopping;
                (stopping, c.version)
            });
            let at = match &stopping {
                Some((rec, version)) => Some(batch.put(rec, Some(*version))?),
                None => None,
            };
            let out = self
                .write("endpoint.suspending", rec, batch, Settle::default())
                .await?;
            if let (Some((record, _)), Some(at)) = (stopping, at) {
                self.compute = Some(Versioned {
                    record,
                    version: out[at].unwrap_or_default(),
                });
            }
        }
        self.stop_compute(true).await?;
        let mut rec = self.ep().clone();
        rec.compute_id = None;
        rec.state = match rec.desired {
            DesiredState::Running => EndpointState::Starting,
            DesiredState::Suspended => EndpointState::Suspended,
        };
        let batch = self.without_compute()?;
        self.write(
            "endpoint.suspended",
            rec,
            batch,
            Settle {
                suspended_now: true,
            },
        )
        .await?;
        self.ended(LifecycleKind::Suspended);
        tracing::info!(endpoint = %self.ep().id, "compute suspended");
        Ok(())
    }

    /// Desired `running`.
    async fn run_compute(&mut self) -> Result<Step, ReconcileError> {
        match self.ep().state {
            EndpointState::Failed => {
                if !self.ops.is_empty() {
                    let rec = self.ep().clone();
                    self.write("endpoint.failed", rec, Batch::new(), Settle::default())
                        .await?;
                }
                return Ok(Step::Done);
            }
            EndpointState::Deleting => return Ok(Step::Done),
            EndpointState::Suspending => self.stop_suspending().await?,
            EndpointState::Idle | EndpointState::Suspended => {
                let mut rec = self.ep().clone();
                rec.state = EndpointState::Starting;
                self.write("endpoint.starting", rec, Batch::new(), Settle::default())
                    .await?;
            }
            EndpointState::Starting | EndpointState::Running => {}
        }
        match self.ep().state {
            EndpointState::Running => self.check_running().await,
            _ => self.start().await,
        }
    }

    /// Fails the endpoint for good (until `StartEndpoint`): the compute
    /// stopped and cleared.
    async fn fail(&mut self, why: String) -> Result<Step, ReconcileError> {
        let compute_why = why.clone();
        self.fail_with(why, compute_why).await
    }

    /// [`fail`](Self::fail), the endpoint failing for `why` and its compute
    /// for `compute_why`.
    async fn fail_with(
        &mut self,
        why: String,
        compute_why: String,
    ) -> Result<Step, ReconcileError> {
        self.stop_compute(false).await?;
        let mut rec = self.ep().clone();
        rec.state = EndpointState::Failed;
        rec.promote = false;
        rec.compute_id = None;
        rec.failure = Some(why);
        let batch = self.without_compute()?;
        self.write("endpoint.failed", rec, batch, Settle::default())
            .await?;
        self.ended(LifecycleKind::Failed {
            reason: compute_why,
        });
        tracing::warn!(endpoint = %self.ep().id, failure = ?self.ep().failure, "endpoint failed");
        Ok(Step::Done)
    }

    /// A crash: the compute is stopped and cleared; the third within the
    /// window fails the endpoint, otherwise a new compute starts.
    async fn crash(&mut self, why: &str) -> Result<Step, ReconcileError> {
        let now = self.now();
        let mut crashes: Vec<u64> = self
            .ep()
            .crashes
            .iter()
            .copied()
            .filter(|t| now.saturating_sub(*t) < CRASH_WINDOW_MS)
            .collect();
        crashes.push(now);
        tracing::warn!(endpoint = %self.ep().id, why, crashes = crashes.len(), "compute crashed");
        if crashes.len() >= CRASHES_TO_FAIL {
            let mut rec = self.ep().clone();
            rec.crashes = crashes;
            self.cur.record = rec;
            return self
                .fail_with(
                    format!(
                        "the compute crashed {CRASHES_TO_FAIL} times in 10 minutes; StartEndpoint \
                         tries again"
                    ),
                    why.to_owned(),
                )
                .await;
        }
        self.stop_compute(false).await?;
        let mut rec = self.ep().clone();
        rec.crashes = crashes;
        rec.compute_id = None;
        rec.state = EndpointState::Starting;
        let batch = self.without_compute()?;
        self.write("endpoint.crashed", rec, batch, Settle::default())
            .await?;
        self.ended(LifecycleKind::Failed {
            reason: why.to_owned(),
        });
        Ok(Step::Again(self.act.ctx.config.retry_initial))
    }

    /// `starting`: a compute created, started and polled until it runs.
    async fn start(&mut self) -> Result<Step, ReconcileError> {
        let project = &self.snap.project.record;
        let Some(branch) = self.snap.branch(&self.ep().branch_id) else {
            return Ok(Step::Done);
        };
        if project.state == ProjectState::Failed || branch.record.state == BranchState::Failed {
            return self
                .fail("the project or the branch has failed".into())
                .await;
        }
        if project.state != ProjectState::Ready || branch.record.state != BranchState::Ready {
            return Ok(Step::Again(self.act.ctx.config.retry_initial));
        }
        // The branch's major, which after an upgrade by branch can differ
        // from the project's (design 62 §9.2).
        let major = branch.record.major(project);
        let Some(image) = self.c.images.get(&major).cloned() else {
            return self
                .fail(format!(
                    "no compute image is configured for Postgres {major}"
                ))
                .await;
        };
        if let Err(why) = crate::runtime::images::check_compute_major(&image, major) {
            return self.fail(why).await;
        }
        if self.compute.is_none() && !self.take_warm(&image).await? {
            self.create_compute().await?;
        }
        let Some(compute) = self.compute.clone() else {
            // The runtime refused the endpoint: it failed.
            return Ok(Step::Done);
        };
        let id: ComputeId =
            compute.record.id.parse().map_err(|_| {
                ReconcileError::Runtime(format!("compute id {}", compute.record.id))
            })?;
        if compute.record.ctl_url.is_none() {
            self.launch(&compute, id, image).await?;
        }
        let Some(compute) = self.compute.clone() else {
            // The runtime refused the compute: the endpoint failed.
            return Ok(Step::Done);
        };
        self.act.renew().await?;
        match self.c.runtime.status(self.ns(), &id).await {
            Ok(RuntimeStatus::Exited { code }) => {
                return self
                    .crash(&format!("the compute's container exited ({code:?})"))
                    .await;
            }
            Ok(RuntimeStatus::Missing) => {
                return self.crash("the compute's container is gone").await;
            }
            Ok(RuntimeStatus::Pending | RuntimeStatus::Running) => {}
            Err(e) => return Err(runtime_error("reading the compute's status", &e)),
        }
        let status = self.c.ctl.status(&compute.record).await;
        if matches!(status.as_ref().map(|r| &r.status), Ok(CtlStatus::Empty))
            && compute.record.pool.is_some()
        {
            // A warm-pool compute does not read its spec again: the spec
            // pushed when it was bound did not reach it.
            self.configure_warm(&compute.record).await?;
        }
        match status.as_ref().map(|r| &r.status) {
            Ok(CtlStatus::Running) => {
                let last_active = status.map_or(Activity::NotYet, |r| r.last_active);
                self.mark_running(compute, id, last_active).await
            }
            Ok(CtlStatus::Failed(why)) => {
                tracing::warn!(compute = %compute.record.id, error = %why, "compute_ctl reports a failure");
                self.crash("compute_ctl reported a failure").await
            }
            Ok(CtlStatus::Terminated) => self.crash("compute_ctl terminated").await,
            Ok(CtlStatus::Empty | CtlStatus::Starting) | Err(_) => {
                if let Err(e) = &status {
                    tracing::debug!(compute = %compute.record.id, error = %e, "compute_ctl not answering yet");
                }
                // A warm-pool compute's start counts from its binding.
                let since = compute
                    .record
                    .bound_at_ms
                    .unwrap_or(compute.record.created_at_ms);
                let age = self.now().saturating_sub(since);
                let limit = u64::try_from(self.c.start_timeout.as_millis()).unwrap_or(u64::MAX);
                if age >= limit {
                    return self
                        .fail(format!(
                            "the compute did not start within {} s",
                            self.c.start_timeout.as_secs()
                        ))
                        .await;
                }
                Ok(Step::Again(self.c.poll))
            }
        }
    }

    /// The endpoint's address ([`ComputeRuntime::ensure_address`]); `None`
    /// when the runtime refused it, and the endpoint failed.
    async fn address(&mut self) -> Result<Option<BackendAddr>, ReconcileError> {
        self.act.renew().await?;
        match self.c.runtime.ensure_address(self.ns(), self.ep()).await {
            Ok(a) => Ok(Some(a)),
            Err(RuntimeError::InvalidArgument(why)) => {
                tracing::error!(endpoint = %self.ep().id, why, "the runtime refused the endpoint's address");
                self.fail("the runtime refused the endpoint's address".into())
                    .await?;
                Ok(None)
            }
            Err(e) => Err(runtime_error("the endpoint's address", &e)),
        }
    }

    /// The endpoint naming compute `id` at `addr`, its binding, and the
    /// version of its spec's inputs as they are now, before the compute can
    /// read anything (a later change makes it differ).
    async fn binding_for(
        &self,
        id: &str,
        cu: Cu,
        addr: &BackendAddr,
    ) -> Result<(EndpointRec, ComputeBinding, u64), ReconcileError> {
        let project = &self.snap.project.record;
        let binding = ComputeBinding {
            namespace: project.namespace.clone(),
            project_id: project.id.clone(),
            endpoint_id: self.ep().id.clone(),
        };
        let mut endpoint = self.ep().clone();
        endpoint.compute_id = Some(id.to_owned());
        endpoint.backend_addr = Some(addr.to_string());
        let spec_version =
            match spec::read_inputs(&self.act.ctx.store, &self.c.sources, &binding, None).await {
                Ok(mut inputs) => {
                    inputs.endpoint = endpoint.clone();
                    spec::build(&self.c.sources, id, cu, &inputs).map_or(0, |(_, version)| version)
                }
                Err(SpecError::Store(e)) => return Err(e.into()),
                Err(_) => 0,
            };
        Ok((endpoint, binding, spec_version))
    }

    /// Binds a warm-pool compute (Task 14) of the endpoint's key and size,
    /// when the pool has one: one batch binds its record (compare-and-set,
    /// so no other endpoint takes it) and names it on the endpoint; then it
    /// gets its spec through `/configure`, and the pool refills. `false`:
    /// none was taken.
    async fn take_warm(&mut self, image: &str) -> Result<bool, ReconcileError> {
        let Some(pool) = self.c.pool.clone() else {
            return Ok(false);
        };
        if !pool.enabled() || self.ep().min_cu != pool.config().cu {
            return Ok(false);
        }
        let project = &self.snap.project.record;
        let key = PoolKey {
            namespace: project.namespace.clone(),
            pg_version: project.pg_version,
            image: image.to_owned(),
        };
        let store = &self.act.ctx.store;
        while let Some(id) = pool.pop(&key) {
            let Some(warm) = store.get::<ComputeRec>(&ComputeKey { id }).await? else {
                continue;
            };
            let rec = &warm.record;
            if rec.binding.is_some()
                || rec.pool.as_ref() != Some(&key)
                || rec.status != ComputeStatus::Running
                || rec.ctl_url.is_none()
                || rec.jwt_key_id.as_deref() != Some(self.c.keys.kid())
            {
                continue;
            }
            // It still waits for a spec (a dead one would count as the
            // endpoint's crash).
            if !matches!(
                self.c.ctl.status(rec).await.map(|r| r.status),
                Ok(CtlStatus::Empty)
            ) {
                continue;
            }
            let Some(addr) = self.address().await? else {
                return Ok(true);
            };
            let (endpoint, binding, spec_version) =
                self.binding_for(&rec.id, rec.cu, &addr).await?;
            let mut bound = rec.clone();
            bound.binding = Some(binding);
            bound.status = ComputeStatus::Pending;
            bound.bound_at_ms = Some(self.now());
            bound.spec_version = spec_version;
            let mut batch = Batch::new();
            let at = batch.put(&bound, Some(warm.version))?;
            let out = self
                .write("compute.take", endpoint, batch, Settle::default())
                .await?;
            self.compute = Some(Versioned {
                record: bound,
                version: out[at].unwrap_or_default(),
            });
            pool.kick();
            tracing::info!(endpoint = %self.ep().id, compute = %warm.record.id, address = %addr, "warm compute taken");
            if let Some(c) = self.compute.clone() {
                self.configure_warm(&c.record).await?;
            }
            return Ok(true);
        }
        Ok(false)
    }

    /// Hands a bound warm-pool compute its spec through `/configure`,
    /// waiting at most the pool's `push_wait`: `compute_ctl` takes the spec
    /// at once and answers only once Postgres runs, which the passes poll.
    async fn configure_warm(&self, compute: &ComputeRec) -> Result<(), ReconcileError> {
        let Some(binding) = &compute.binding else {
            return Ok(());
        };
        let inputs = match spec::read_inputs(
            &self.act.ctx.store,
            &self.c.sources,
            binding,
            Some(&compute.id),
        )
        .await
        {
            Ok(i) => i,
            Err(SpecError::Store(e)) => return Err(e.into()),
            Err(e) => {
                tracing::info!(compute = %compute.id, error = ?e, "no spec for the warm compute yet");
                return Ok(());
            }
        };
        let spec = match spec::build(&self.c.sources, &compute.id, compute.cu, &inputs) {
            Ok((spec, _)) => spec,
            Err(e) => {
                tracing::info!(compute = %compute.id, error = ?e, "no spec for the warm compute yet");
                return Ok(());
            }
        };
        let wait = self
            .c
            .pool
            .as_ref()
            .map_or(Duration::from_secs(2), |p| p.config().push_wait);
        self.act.renew().await?;
        match tokio::time::timeout(wait, self.c.ctl.configure(compute, &spec)).await {
            Ok(Ok(())) => tracing::debug!(compute = %compute.id, "the warm compute runs its spec"),
            Ok(Err(e)) => {
                tracing::info!(compute = %compute.id, error = %e, "the warm compute did not take its spec; retrying");
            }
            Err(_) => tracing::debug!(compute = %compute.id, "the warm compute is starting"),
        }
        Ok(())
    }

    /// The endpoint's address, then one batch: the compute's `C/` record and
    /// the endpoint naming it.
    async fn create_compute(&mut self) -> Result<(), ReconcileError> {
        let Some(addr) = self.address().await? else {
            return Ok(());
        };
        let id = ComputeId::new();
        let (endpoint, binding, spec_version) = self
            .binding_for(&id.to_string(), self.ep().min_cu, &addr)
            .await?;
        let compute = ComputeRec {
            id: id.to_string(),
            binding: Some(binding),
            pool: None,
            bound_at_ms: None,
            spec_version,
            status: ComputeStatus::Pending,
            jwt_key_id: Some(self.c.keys.kid().to_owned()),
            ctl_url: None,
            cu: endpoint.min_cu,
            created_at_ms: self.now(),
            last_active_ms: None,
        };
        let mut batch = Batch::new();
        let at = batch.put(&compute, None)?;
        let out = self
            .write("compute.create", endpoint, batch, Settle::default())
            .await?;
        self.compute = Some(Versioned {
            record: compute,
            version: out[at].unwrap_or_default(),
        });
        tracing::info!(endpoint = %self.ep().id, compute = %id, address = %addr, "compute created");
        Ok(())
    }

    /// Starts the compute in the runtime and records `compute_ctl`'s
    /// address.
    async fn launch(
        &mut self,
        compute: &Versioned<ComputeRec>,
        id: ComputeId,
        image: String,
    ) -> Result<(), ReconcileError> {
        let address = self
            .ep()
            .backend_addr
            .as_deref()
            .map(BackendAddr::parse)
            .transpose()
            .map_err(|e| runtime_error("the endpoint's address", &e))?;
        let token = self
            .c
            .keys
            .issue_upcall_token(&id, self.now(), COMPUTE_TOKEN_TTL)
            .map_err(|e| ReconcileError::Runtime(format!("signing the upcall token: {e}")))?;
        let launch = ComputeLaunch {
            compute_id: id,
            namespace: self.ns().to_owned(),
            binding: compute.record.binding.clone(),
            address,
            image,
            cu: compute.record.cu,
            upcall_token: token,
        };
        self.act.renew().await?;
        let handle = match self.c.runtime.start(&launch).await {
            Ok(h) => h,
            Err(RuntimeError::InvalidArgument(why)) => {
                tracing::error!(compute = %id, why, "the runtime refused the compute");
                self.fail("the runtime refused the compute".into()).await?;
                return Ok(());
            }
            Err(e) => return Err(runtime_error("starting the compute", &e)),
        };
        let mut started = compute.record.clone();
        started.ctl_url = Some(handle.ctl_url);
        let mut batch = Batch::new();
        batch.put(&started, Some(compute.version))?;
        let out = self.act.commit("compute.started", batch).await?;
        self.compute = Some(Versioned {
            record: started,
            version: out[0].unwrap_or_default(),
        });
        tracing::info!(endpoint = %self.ep().id, compute = %id, "compute started");
        Ok(())
    }

    /// `compute_ctl` runs: the address points at the compute, which is
    /// `running`, and so is the endpoint. The compute's idle time starts
    /// now, and the observer hears it started.
    async fn mark_running(
        &mut self,
        compute: Versioned<ComputeRec>,
        id: ComputeId,
        last_active: Activity,
    ) -> Result<Step, ReconcileError> {
        let handle = ComputeHandle {
            compute_id: id,
            ctl_url: compute.record.ctl_url.clone().unwrap_or_default(),
            address: self
                .ep()
                .backend_addr
                .as_deref()
                .map(BackendAddr::parse)
                .transpose()
                .map_err(|e| runtime_error("the endpoint's address", &e))?,
        };
        self.act.renew().await?;
        self.c
            .runtime
            .bind_address(self.ns(), self.ep(), &handle)
            .await
            .map_err(|e| runtime_error("binding the address", &e))?;
        let mut running = compute.record.clone();
        running.status = ComputeStatus::Running;
        let mut batch = Batch::new();
        let at = batch.put(&running, Some(compute.version))?;
        let mut rec = self.ep().clone();
        rec.state = EndpointState::Running;
        let out = self
            .write("endpoint.running", rec, batch, Settle::default())
            .await?;
        let reported = match last_active {
            Activity::At(at) => Some(at),
            Activity::NotYet | Activity::Unreadable => None,
        };
        self.c.idle.observe(&running.id, self.now(), reported, 0);
        self.emit(&running, LifecycleKind::Started { cu: running.cu });
        self.track(&running);
        self.compute = Some(Versioned {
            record: running,
            version: out[at].unwrap_or_default(),
        });
        tracing::info!(endpoint = %self.ep().id, compute = %id, "endpoint running");
        self.push_spec().await
    }

    /// `running`: the compute checked, replaced when its token runs out,
    /// and its spec pushed when its inputs moved.
    async fn check_running(&mut self) -> Result<Step, ReconcileError> {
        let Some(compute) = self.compute.clone() else {
            // Running with no compute: start one.
            let mut rec = self.ep().clone();
            rec.state = EndpointState::Starting;
            rec.compute_id = None;
            self.write("endpoint.starting", rec, Batch::new(), Settle::default())
                .await?;
            return Ok(Step::Again(Duration::ZERO));
        };
        let ttl = COMPUTE_TOKEN_TTL.saturating_sub(REPLACE_MARGIN);
        let ttl = u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX);
        if self.now().saturating_sub(compute.record.created_at_ms) >= ttl {
            self.replace("the compute's upcall token expires").await?;
            return Ok(Step::Again(Duration::ZERO));
        }
        let id: ComputeId =
            compute.record.id.parse().map_err(|_| {
                ReconcileError::Runtime(format!("compute id {}", compute.record.id))
            })?;
        self.act.renew().await?;
        match self.c.runtime.status(self.ns(), &id).await {
            Ok(RuntimeStatus::Exited { code }) => {
                return self
                    .crash(&format!("the compute's container exited ({code:?})"))
                    .await;
            }
            Ok(RuntimeStatus::Missing) => {
                return self.crash("the compute's container is gone").await;
            }
            Ok(_) => {}
            Err(e) => return Err(runtime_error("reading the compute's status", &e)),
        }
        let last_active = match self.c.ctl.status(&compute.record).await {
            Ok(report) => match report.status {
                CtlStatus::Failed(why) => {
                    tracing::warn!(compute = %compute.record.id, error = %why, "compute_ctl reports a failure");
                    return self.crash("compute_ctl reported a failure").await;
                }
                CtlStatus::Terminated => return self.crash("compute_ctl terminated").await,
                CtlStatus::Empty | CtlStatus::Starting | CtlStatus::Running => report.last_active,
            },
            Err(e) => {
                tracing::info!(compute = %compute.record.id, error = %e, "compute_ctl did not answer");
                Activity::Unreadable
            }
        };
        self.track(&compute.record);
        // An operation settled here (a start or restart of a running
        // endpoint) was asked just now: the sample counts it as activity.
        let asked = !self.ops.is_empty();
        if asked {
            let rec = self.ep().clone();
            self.write("endpoint.running", rec, Batch::new(), Settle::default())
                .await?;
        }
        if self.ep().promote {
            return self.promote(compute).await;
        }
        let due = match self.idle(&compute.record, last_active, asked).await? {
            Idle::Suspended => return Ok(Step::Done),
            Idle::Due(due) => due,
        };
        Ok(match (self.push_spec().await?, due) {
            (Step::Again(a), Some(b)) => Step::Again(a.min(b)),
            (Step::Done, Some(b)) => Step::Again(b),
            (step, _) => step,
        })
    }

    /// `RestartEndpoint{promote}` (Task 16) on this running read-only
    /// endpoint: once its branch's read-write endpoint is down, prewarm the
    /// replica's file cache (from the read-write endpoint's saved state;
    /// waited for at most [`PREWARM_WAIT`]), then `compute_ctl`'s
    /// `/promote` with a primary's spec at the WAL's flushed LSN, then one
    /// batch swaps the two endpoints' types. A read-write endpoint that runs
    /// (or is asked to) or a refusal ends the promotion, failed.
    async fn promote(&mut self, compute: Versioned<ComputeRec>) -> Result<Step, ReconcileError> {
        let ep = self.ep().clone();
        let sibling = self
            .snap
            .endpoints
            .iter()
            .find(|e| {
                e.record.id != ep.id
                    && e.record.branch_id == ep.branch_id
                    && e.record.kind == EndpointType::ReadWrite
            })
            .cloned();
        let rw_runs = sibling.as_ref().is_some_and(|o| {
            o.record.desired == DesiredState::Running || o.record.compute_id.is_some()
        });
        if ep.kind != EndpointType::ReadOnly || rw_runs {
            tracing::warn!(endpoint = %ep.id, "the branch's read-write endpoint runs: the promotion ends");
            return self.end_promotion(None).await;
        }
        let asked_at = self
            .ops
            .iter()
            .filter(|o| o.record.kind == OperationKind::EndpointPromote)
            .map(|o| o.record.created_at_ms)
            .min()
            .unwrap_or_else(|| self.now());
        let waited = self.now().saturating_sub(asked_at)
            >= u64::try_from(PREWARM_WAIT.as_millis()).unwrap_or(u64::MAX);
        if !waited {
            match self.c.ctl.prewarm_state(&compute.record).await {
                Ok(Prewarm::NotPrewarmed) => {
                    let from = sibling.as_ref().map(|o| o.record.id.as_str());
                    if let Err(e) = self.c.ctl.prewarm(&compute.record, from).await {
                        tracing::info!(compute = %compute.record.id, error = %e, "the prewarm was not started; retrying");
                    }
                    return Ok(Step::Again(self.c.poll));
                }
                Ok(Prewarm::Prewarming) => return Ok(Step::Again(self.c.poll)),
                Ok(Prewarm::Done(status)) => {
                    tracing::info!(compute = %compute.record.id, %status, "the replica's prewarm ended");
                }
                Err(e) => {
                    tracing::info!(compute = %compute.record.id, error = %e, "the prewarm state is unknown; retrying");
                    return Ok(Step::Again(self.act.ctx.config.retry_initial));
                }
            }
        }
        let project = &self.snap.project.record;
        let Some(branch) = self.snap.branch(&ep.branch_id) else {
            return Ok(Step::Done);
        };
        let heads = match self
            .act
            .ctx
            .neon
            .wal_heads(
                loams_postgres::TenantId(project.tenant_id),
                loams_postgres::TimelineId(branch.record.timeline_id),
            )
            .await
        {
            Ok(h) => h,
            Err(e) => {
                tracing::info!(endpoint = %ep.id, error = %e.message, "the WAL's heads are unknown; retrying");
                return Ok(Step::Again(self.act.ctx.config.retry_initial));
            }
        };
        let Some(binding) = compute.record.binding.clone() else {
            return Ok(Step::Done);
        };
        let mut inputs = match spec::read_inputs(
            &self.act.ctx.store,
            &self.c.sources,
            &binding,
            Some(&compute.record.id),
        )
        .await
        {
            Ok(i) => i,
            Err(SpecError::Store(e)) => return Err(e.into()),
            Err(e) => {
                tracing::info!(endpoint = %ep.id, error = ?e, "no spec to promote with yet");
                return Ok(Step::Again(self.act.ctx.config.retry_initial));
            }
        };
        inputs.endpoint.kind = EndpointType::ReadWrite;
        // The pinned compute_ctl panics applying the catalog inside
        // `/promote` (it blocks on its runtime from within it; R16.6): the
        // promotion's spec leaves the catalog alone (the replica has the
        // primary's), and the compute's spec version is then recorded as
        // unknown (0), so the next pass pushes the full spec through
        // `/configure`.
        let mut spec = match spec::build(
            &self.c.sources,
            &compute.record.id,
            compute.record.cu,
            &inputs,
        ) {
            Ok((spec, _)) => spec,
            Err(e) => {
                tracing::info!(endpoint = %ep.id, error = ?e, "no spec to promote with yet");
                return Ok(Step::Again(self.act.ctx.config.retry_initial));
            }
        };
        spec.skip_pg_catalog_updates = true;
        self.act.renew().await?;
        let answer = tokio::time::timeout(
            PROMOTE_WAIT,
            self.c.ctl.promote(&compute.record, &spec, heads.flush_lsn),
        )
        .await
        .unwrap_or_else(|_| {
            Err(PromoteError::Unknown(format!(
                "no answer within {} s",
                PROMOTE_WAIT.as_secs()
            )))
        });
        match answer {
            Ok(()) => {
                tracing::info!(endpoint = %ep.id, compute = %compute.record.id, "replica promoted");
                self.end_promotion(Some((sibling, compute, 0))).await
            }
            Err(PromoteError::Refused(e)) => {
                tracing::warn!(endpoint = %ep.id, compute = %compute.record.id, error = %e, "compute_ctl refused the promotion");
                self.end_promotion(None).await
            }
            Err(PromoteError::Unknown(e)) => {
                // It may be a primary now: it must not serve as a replica,
                // so a fresh replica replaces it.
                tracing::warn!(endpoint = %ep.id, compute = %compute.record.id, error = %e, "the promotion's outcome is unknown; replacing the compute");
                self.cur.record.replace_compute = true;
                self.end_promotion(None).await
            }
        }
    }

    /// Ends a promotion: with `swapped` (the read-write endpoint, if any,
    /// the promoted compute and its primary spec's version) this endpoint
    /// becomes read-write and that one read-only; without, nothing changes
    /// but the flag, and the operation fails.
    async fn end_promotion(
        &mut self,
        swapped: Option<(Option<Versioned<EndpointRec>>, Versioned<ComputeRec>, u64)>,
    ) -> Result<Step, ReconcileError> {
        let mut rec = self.ep().clone();
        rec.promote = false;
        let mut batch = Batch::new();
        let mut compute_at = None;
        if let Some((sibling, compute, version)) = &swapped {
            rec.kind = EndpointType::ReadWrite;
            if let Some(o) = sibling {
                let mut demoted = o.record.clone();
                demoted.kind = EndpointType::ReadOnly;
                demoted.updated_at_ms = self.now();
                batch.put(&demoted, Some(o.version))?;
            }
            let mut c = compute.record.clone();
            c.spec_version = *version;
            compute_at = Some((batch.put(&c, Some(compute.version))?, c));
        }
        let out = self
            .write("endpoint.promote", rec, batch, Settle::default())
            .await?;
        if let Some((at, c)) = compute_at {
            self.compute = Some(Versioned {
                record: c,
                version: out[at].unwrap_or_default(),
            });
        }
        Ok(Step::Done)
    }

    /// The autoscaler follows the running compute (Task 15), with the
    /// endpoint's range as this pass read it.
    fn track(&self, compute: &ComputeRec) {
        if let Some(s) = &self.c.scaling {
            s.track(Tracked {
                namespace: self.ns().to_owned(),
                project_id: self.ep().project_id.clone(),
                endpoint_id: self.ep().id.clone(),
                compute: compute.clone(),
                min_cu: self.ep().min_cu,
                max_cu: self.ep().max_cu,
            });
        }
    }

    /// Idle detection ([`crate::idle`]): one sample of the running compute
    /// (its `compute_ctl`'s `last_active`, PgDog's clients of the endpoint),
    /// and the suspend once it has been idle for the endpoint's
    /// `suspend_timeout_s`. A sample that could not be taken, an endpoint
    /// with open operations or a replacement asked, and a timeout of 0
    /// never suspend; an operation this pass settled (`asked`) is activity.
    async fn idle(
        &mut self,
        compute: &ComputeRec,
        last_active: Activity,
        asked: bool,
    ) -> Result<Idle, ReconcileError> {
        let ep = self.ep();
        if ep.suspend_timeout_s == 0
            || ep.desired != DesiredState::Running
            || ep.replace_compute
            || !self.ops.is_empty()
        {
            return Ok(Idle::Due(None));
        }
        let reported = match last_active {
            Activity::At(at) => Some(at),
            Activity::NotYet => None,
            Activity::Unreadable => return Ok(Idle::Due(None)),
        };
        let Some(branch) = self.snap.branch(&ep.branch_id) else {
            return Ok(Idle::Due(None));
        };
        let clients = match self
            .c
            .idle
            .clients(&self.snap.project.record, &branch.record, ep)
            .await
        {
            Ok(n) => n,
            Err(e) => {
                tracing::info!(endpoint = %ep.id, error = %e, "PgDog did not answer; not suspending");
                return Ok(Idle::Due(None));
            }
        };
        let idle_ms = self.c.idle.observe(
            &compute.id,
            self.now(),
            reported,
            clients.saturating_add(u64::from(asked)),
        );
        let timeout_ms = u64::from(ep.suspend_timeout_s) * 1000;
        if idle_ms < timeout_ms {
            // Come back when the suspend is due, unless the resync comes
            // first.
            let due = Duration::from_millis(timeout_ms - idle_ms);
            return Ok(Idle::Due((due < self.act.ctx.config.resync).then_some(due)));
        }
        tracing::info!(endpoint = %ep.id, compute = %compute.id, idle_s = idle_ms / 1000, "idle: suspending");
        self.cur.record.desired = DesiredState::Suspended;
        self.stop_suspending().await?;
        Ok(Idle::Suspended)
    }

    /// Pushes the compute's spec when the version of its inputs moved since
    /// the compute got one, and records the new version.
    async fn push_spec(&mut self) -> Result<Step, ReconcileError> {
        let Some(compute) = self.compute.clone() else {
            return Ok(Step::Done);
        };
        let Some(binding) = compute.record.binding.clone() else {
            return Ok(Step::Done);
        };
        let inputs = match spec::read_inputs(
            &self.act.ctx.store,
            &self.c.sources,
            &binding,
            Some(&compute.record.id),
        )
        .await
        {
            Ok(i) => i,
            Err(SpecError::Store(e)) => return Err(e.into()),
            Err(e) => {
                tracing::info!(compute = %compute.record.id, error = ?e, "no spec to push");
                return Ok(Step::Done);
            }
        };
        let (spec, version) = match spec::build(
            &self.c.sources,
            &compute.record.id,
            compute.record.cu,
            &inputs,
        ) {
            Ok(built) => built,
            Err(e) => {
                tracing::info!(compute = %compute.record.id, error = ?e, "no spec to push");
                return Ok(Step::Done);
            }
        };
        if version == compute.record.spec_version {
            return Ok(Step::Done);
        }
        self.act.renew().await?;
        if let Err(e) = self.c.ctl.configure(&compute.record, &spec).await {
            tracing::warn!(compute = %compute.record.id, error = %e, "the new spec was not taken; retrying");
            return Ok(Step::Again(self.act.ctx.config.retry_initial));
        }
        let mut pushed = compute.record.clone();
        pushed.spec_version = version;
        let mut batch = Batch::new();
        batch.put(&pushed, Some(compute.version))?;
        let out = self.act.commit("compute.spec", batch).await?;
        self.compute = Some(Versioned {
            record: pushed,
            version: out[0].unwrap_or_default(),
        });
        tracing::info!(compute = %compute.record.id, "pushed a changed spec");
        Ok(Step::Done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::EndpointType;

    fn ep(desired: DesiredState, state: EndpointState) -> EndpointRec {
        let mut e = EndpointRec::new("prj-a", "ep-a", "br-a", EndpointType::ReadWrite);
        e.desired = desired;
        e.state = state;
        e
    }

    /// Only `compute_ctl`'s refusals before `pg_promote()` (and client
    /// errors) leave the compute a replica; anything else is unknown.
    #[test]
    fn promote_errors_are_refusals_only_before_pg_promote() {
        let e = |status: u16, msg: &str| loams_postgres::NeonError {
            status,
            msg: msg.into(),
            component: loams_postgres::Component::ComputeCtl,
            op: loams_postgres::Op::Promote,
        };
        for (status, msg) in [
            (500, "compute NotPrewarmed"),
            (500, "compute Prewarming"),
            (500, "didn't catch up with primary in 20 retries"),
            (500, "compute mode \"primary\" is not replica"),
            (400, "invalid spec"),
        ] {
            assert!(
                matches!(PromoteError::of(&e(status, msg)), PromoteError::Refused(_)),
                "{msg}"
            );
        }
        for (status, msg) in [
            (500, "promote sender dropped"),
            (500, "pg_promote() failed"),
            (500, "replica in read only mode after promotion"),
            (0, "operation timed out"),
        ] {
            assert!(
                matches!(PromoteError::of(&e(status, msg)), PromoteError::Unknown(_)),
                "{msg}"
            );
        }
    }

    #[test]
    fn endpoints_at_rest_need_no_work() {
        assert!(!needs_work(&ep(
            DesiredState::Suspended,
            EndpointState::Idle
        )));
        assert!(!needs_work(&ep(
            DesiredState::Suspended,
            EndpointState::Suspended
        )));
        assert!(!needs_work(&ep(
            DesiredState::Running,
            EndpointState::Failed
        )));
        assert!(needs_work(&ep(DesiredState::Running, EndpointState::Idle)));
        assert!(needs_work(&ep(
            DesiredState::Running,
            EndpointState::Running
        )));
        assert!(needs_work(&ep(
            DesiredState::Suspended,
            EndpointState::Failed
        )));
        let mut open = ep(DesiredState::Suspended, EndpointState::Idle);
        open.open_ops.push("op-1".into());
        assert!(needs_work(&open));
    }
}
