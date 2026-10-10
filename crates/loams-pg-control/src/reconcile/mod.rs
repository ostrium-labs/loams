//! `pg-control`'s reconcilers (design §46 §6.3; PG2 Task 7): they make
//! Neon's storage match the records the API service writes, one project at
//! a time, under the project's fenced lease.
//!
//! - **Triggers.** [`Reconciler::run`] watches the projects (`x/`), their
//!   branches and guards (`X/`) and endpoints (`E/`), passes again every
//!   [`resync`](ReconcilerConfig::resync), and takes explicit
//!   [`Kick`]s. Each trigger marks a project; a marked project gets one
//!   pass at a time ([`Reconciler::reconcile_project`]).
//! - **The lease** ([`lease`]). A pass reads the records, and only when
//!   something is to be done takes the project's lease `e/pg/<project_id>`,
//!   reads them again and acts. Every write is a fenced compare-and-set
//!   ([`PgControlStore::commit`]), so a holder whose lease moved on writes
//!   nothing, and a record that moved meanwhile makes the pass read again.
//! - **Projects** ([`project`]): attach the tenant with its
//!   `pitr_interval`, create `main`'s timeline, then mark the project ready;
//!   delete: every branch (children first), then the project's records.
//! - **Branches** ([`branch`]): create the timeline on the pageserver and on
//!   `loams-wal`, then ready; delete: once the branch's endpoints and
//!   children are gone, delete the timeline, then remove the branch with its
//!   name index, guard, roles and databases in one fenced batch (R5.10,
//!   R6.8), and delete the roles' secrets.
//! - **Idempotence.** Every Neon step can be repeated: ids are derived from
//!   the records (Task 4), a 409 or an existing timeline counts as done, and
//!   a missing timeline counts as deleted. A pass that dies between a Neon
//!   call and the record's write is redone by the next pass, which finds
//!   the call's effect and goes on.
//! - **Operations** move from `Pending` through `Running` (with their
//!   progress, in steps) to `Succeeded` or `Failed` (R5.12) in the batch
//!   that finishes their work.
//! - **The resync's sweep** ([`resync`]) deletes role secrets no record
//!   names (R6.1).
//!
//! Endpoints are not reconciled here (Task 11): a branch or project delete
//! waits until its endpoints are gone.

pub mod branch;
mod lease;
pub mod project;
pub mod resync;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use futures::future::BoxFuture;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::model::{OperationError, OperationProgress, OperationRec, OperationState, Record};
use crate::neon::{NeonApiError, NeonWrite};
use crate::secrets::{SecretError, SecretStore};
use crate::service::{Clock, Reason};
use crate::store::{
    Batch, BatchError, Fence, MAX_PAGE_SIZE, Page, PgControlStore, StoreError, StoreEvent,
    Versioned,
};
use lease::{Leases, Taken};

/// A test seam: awaited with the step's name (`project.ready`,
/// `branch.remove`, ...) just before each fenced write, after the lease's
/// renewal, so a test can pause a holder there.
pub type BeforeWrite = Arc<dyn Fn(&'static str) -> BoxFuture<'static, ()> + Send + Sync>;

/// How a [`Reconciler`] runs.
#[derive(Clone)]
pub struct ReconcilerConfig {
    /// The lease holder's name: unique per `pg-control` instance.
    pub holder: String,
    /// How long a project's lease lasts without renewal (default 60 s; at
    /// most 10 minutes). Another instance takes over a project this long
    /// after its holder stopped.
    pub lease_ttl: Duration,
    /// How often every project gets a pass without a trigger (default
    /// 30 s).
    pub resync: Duration,
    /// Passes running at once (default 8).
    pub concurrency: usize,
    /// The first wait before a pass that failed is tried again (default
    /// 1 s); it doubles up to `retry_max` (default 30 s). A pass waiting
    /// for something outside (a timeline deletion, a parent) comes back
    /// after `retry_initial`.
    pub retry_initial: Duration,
    pub retry_max: Duration,
    /// How often the resync sweeps unused secrets (default 10 minutes).
    pub sweep_every: Duration,
    /// How old an unused secret must be before the sweep deletes it
    /// (default 15 minutes; never less than
    /// [`MIN_SECRET_GRACE`](resync::MIN_SECRET_GRACE), twice the service's
    /// [`ISSUE_WINDOW`](crate::service::ISSUE_WINDOW), within which a
    /// record naming a new secret must commit).
    pub secret_grace: Duration,
    /// The generation a tenant is attached with to a pageserver directly
    /// (single pageserver; the storage controller picks its own, R2.14).
    pub tenant_generation: u32,
    pub clock: Clock,
    /// See [`BeforeWrite`]; `None` in production.
    pub before_write: Option<BeforeWrite>,
}

impl ReconcilerConfig {
    /// The defaults, for `holder`.
    pub fn new(holder: impl Into<String>) -> Self {
        ReconcilerConfig {
            holder: holder.into(),
            lease_ttl: Duration::from_secs(60),
            resync: Duration::from_secs(30),
            concurrency: 8,
            retry_initial: Duration::from_secs(1),
            retry_max: Duration::from_secs(30),
            sweep_every: Duration::from_secs(600),
            secret_grace: Duration::from_secs(900),
            tenant_generation: 1,
            clock: Clock::system(),
            before_write: None,
        }
    }
}

impl fmt::Debug for ReconcilerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReconcilerConfig")
            .field("holder", &self.holder)
            .field("lease_ttl", &self.lease_ttl)
            .field("resync", &self.resync)
            .field("concurrency", &self.concurrency)
            .field("retry_initial", &self.retry_initial)
            .field("retry_max", &self.retry_max)
            .field("sweep_every", &self.sweep_every)
            .field("secret_grace", &self.secret_grace)
            .field("tenant_generation", &self.tenant_generation)
            .field("before_write", &self.before_write.is_some())
            .finish_non_exhaustive()
    }
}

/// An explicit trigger: pass over this project now (for example after the
/// API service wrote it, without waiting for the watch's poll).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kick {
    /// The project id.
    Project(String),
}

/// Sends [`Kick`]s to a running [`Reconciler`].
#[derive(Debug, Clone)]
pub struct Kicker(mpsc::UnboundedSender<Kick>);

impl Kicker {
    /// Marks the project for a pass. A kick for a project the reconciler
    /// has not seen yet is dropped: its watch reports the project anyway.
    pub fn kick(&self, kick: Kick) {
        // Dropped only when the reconciler is gone.
        let _ = self.0.send(kick);
    }
}

/// How a pass ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pass {
    /// Nothing was to be done (or the project is gone).
    Idle,
    /// The pass acted, and nothing is left to do now.
    Done,
    /// Waiting for something outside the pass (endpoints, a parent branch,
    /// a timeline being deleted): pass again after this long.
    Again(Duration),
    /// Another instance holds the project's lease.
    Held { holder: String },
}

/// Why a pass stopped. The run loop tries the project again later.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReconcileError {
    /// The project's lease moved on (it lapsed, or another holder took
    /// it): a write was fenced, or the renewal found it lost. The pass
    /// wrote nothing after that.
    #[error("the project's lease moved on")]
    Fenced,
    /// A record changed after the pass read it; the next pass reads again.
    #[error("a record moved under the pass")]
    Moved,
    #[error("control store: {0}")]
    Store(StoreError),
    /// A Neon component refused for a reason that may pass
    /// (`storage_unavailable`, `unavailable`, `aborted`, `internal`).
    #[error("storage: {0}")]
    Neon(NeonApiError),
    #[error("credential store: {0}")]
    Secrets(SecretError),
    /// [`Reconciler::run`] was called a second time on this reconciler.
    #[error("the reconciler is already running")]
    Running,
}

impl From<StoreError> for ReconcileError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::Fenced => ReconcileError::Fenced,
            StoreError::Conflict { .. } | StoreError::NotFound => ReconcileError::Moved,
            other => ReconcileError::Store(other),
        }
    }
}

impl From<BatchError> for ReconcileError {
    fn from(e: BatchError) -> Self {
        e.error.into()
    }
}

/// What a Neon step came to.
pub(crate) enum Step<T> {
    Done(T),
    /// A refusal that will not pass by retrying: the operation fails with it.
    Fail(OperationError),
}

/// A Neon refusal: transient ones end the pass (to be retried), the others
/// are the operation's failure. The component's message stays in the log:
/// it can name internal hosts.
pub(crate) fn refused<T>(what: &str, e: NeonApiError) -> Result<Step<T>, ReconcileError> {
    match e.reason {
        Reason::StorageUnavailable | Reason::Unavailable | Reason::Aborted | Reason::Internal => {
            tracing::warn!(reason = e.reason.as_str(), component = ?e.component, error = %e.message, "{what}: storage refused; retrying");
            Err(ReconcileError::Neon(e))
        }
        reason => {
            tracing::warn!(reason = reason.as_str(), component = ?e.component, error = %e.message, "{what}: storage refused for good");
            Ok(Step::Fail(OperationError {
                reason: reason.as_str().to_string(),
                message: format!("{what}: storage answered {}", reason.as_str()),
            }))
        }
    }
}

/// What every pass shares.
pub(crate) struct Ctx<S, N> {
    pub store: S,
    pub neon: N,
    pub secrets: Arc<dyn SecretStore>,
    pub config: ReconcilerConfig,
    leases: Leases,
    /// The `pitr_interval` (seconds) this instance last set per project,
    /// so a changed `history_retention` reaches the tenant once.
    pub applied: Mutex<HashMap<String, u64>>,
}

impl<S, N> Ctx<S, N> {
    pub fn now_ms(&self) -> u64 {
        self.config.clock.now_ms()
    }

    pub fn applied(&self) -> std::sync::MutexGuard<'_, HashMap<String, u64>> {
        self.applied
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// A pass that holds the project's lease: every write goes through
/// [`commit`](Act::commit).
pub(crate) struct Act<'a, S, N> {
    pub ctx: &'a Ctx<S, N>,
    pub project_id: String,
    fence: Fence,
}

impl<S: PgControlStore, N: NeonWrite> Act<'_, S, N> {
    /// Renews the lease before a step; a lost lease ends the pass.
    pub async fn renew(&self) -> Result<(), ReconcileError> {
        self.ctx
            .leases
            .renew(&self.ctx.store, &self.project_id, &self.fence)
            .await
    }

    /// Renews the lease, then applies `batch` under the fence.
    pub async fn commit(
        &self,
        step: &'static str,
        batch: Batch,
    ) -> Result<Vec<Option<u64>>, ReconcileError> {
        self.renew().await?;
        if let Some(hook) = &self.ctx.config.before_write {
            hook(step).await;
        }
        match self.ctx.store.commit(batch, &self.fence).await {
            Ok(out) => Ok(out),
            Err(e) => {
                if e.error == StoreError::Fenced {
                    self.ctx.leases.forget(&self.project_id);
                    tracing::warn!(project = %self.project_id, step, "a write was fenced: the lease moved on");
                }
                Err(e.into())
            }
        }
    }

    /// Moves the operations `ops` to `Running` at `progress`, unless they
    /// already say so.
    pub async fn progress(
        &self,
        ops: &mut [Versioned<OperationRec>],
        done: u64,
        total: u64,
        phase: &str,
    ) -> Result<(), ReconcileError> {
        let progress = steps(done, total, phase);
        let mut batch = Batch::new();
        let mut moved = Vec::new();
        for (i, op) in ops.iter().enumerate() {
            if op.record.state == OperationState::Running
                && op.record.progress.as_ref() == Some(&progress)
            {
                continue;
            }
            let mut rec = op.record.clone();
            rec.state = OperationState::Running;
            rec.progress = Some(progress.clone());
            rec.updated_at_ms = self.ctx.now_ms();
            batch.put(&rec, Some(op.version))?;
            moved.push((i, rec));
        }
        if moved.is_empty() {
            return Ok(());
        }
        let out = self.commit("operation.progress", batch).await?;
        for ((i, rec), version) in moved.into_iter().zip(out) {
            ops[i] = Versioned {
                record: rec,
                version: version.unwrap_or_default(),
            };
        }
        Ok(())
    }

    /// Adds to `batch` the end of the operations `ops`: `Succeeded` with
    /// their progress complete, or `Failed` with `error`.
    pub fn finish(
        &self,
        batch: &mut Batch,
        ops: &[Versioned<OperationRec>],
        error: Option<&OperationError>,
    ) -> Result<(), ReconcileError> {
        for op in ops {
            let mut rec = op.record.clone();
            let total = rec
                .progress
                .as_ref()
                .map_or(1, |p| p.total.max(p.done).max(1));
            match error {
                None => {
                    rec.state = OperationState::Succeeded;
                    rec.progress = Some(steps(total, total, "done"));
                }
                Some(e) => {
                    rec.state = OperationState::Failed;
                    rec.error = Some(e.clone());
                }
            }
            rec.updated_at_ms = self.ctx.now_ms();
            batch.put(&rec, Some(op.version))?;
        }
        Ok(())
    }
}

fn steps(done: u64, total: u64, phase: &str) -> OperationProgress {
    OperationProgress {
        done,
        total,
        unit: "steps".into(),
        phase: phase.into(),
    }
}

/// Every record under `prefix`, every page.
pub(crate) async fn all<S: PgControlStore, R: Record>(
    store: &S,
    prefix: &R::Prefix,
) -> Result<Vec<Versioned<R>>, StoreError> {
    let mut out = Vec::new();
    let mut at = Page::first(MAX_PAGE_SIZE);
    loop {
        let (records, next) = store.list::<R>(prefix, at).await?;
        out.extend(records);
        match next {
            Some(token) => at = Page::after(MAX_PAGE_SIZE, token),
            None => return Ok(out),
        }
    }
}

/// The project and branch reconcilers (see the module docs).
pub struct Reconciler<S, N> {
    ctx: Arc<Ctx<S, N>>,
    kicks: mpsc::UnboundedSender<Kick>,
    inbox: Mutex<Option<mpsc::UnboundedReceiver<Kick>>>,
}

impl<S, N> fmt::Debug for Reconciler<S, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reconciler")
            .field("config", &self.ctx.config)
            .finish_non_exhaustive()
    }
}

impl<S: PgControlStore, N: NeonWrite> Reconciler<S, N> {
    /// A reconciler over `store`, acting on Neon through `neon`, deleting
    /// role secrets in `secrets`. (The plan's `runtime` joins when endpoints
    /// are reconciled, Task 11.)
    pub fn new(store: S, neon: N, secrets: Arc<dyn SecretStore>, config: ReconcilerConfig) -> Self {
        let (kicks, inbox) = mpsc::unbounded_channel();
        Reconciler {
            ctx: Arc::new(Ctx {
                leases: Leases::new(config.holder.clone(), config.lease_ttl),
                store,
                neon,
                secrets,
                config,
                applied: Mutex::new(HashMap::new()),
            }),
            kicks,
            inbox: Mutex::new(Some(inbox)),
        }
    }

    /// A handle that kicks this reconciler's run loop.
    pub fn kicker(&self) -> Kicker {
        Kicker(self.kicks.clone())
    }

    /// One pass over the project `project_id` of `namespace`: reads its
    /// records and, when something is to be done, takes its lease and does
    /// it. The run loop calls this; tests call it directly.
    ///
    /// # Errors
    ///
    /// [`ReconcileError`]: the pass stopped, and is to be tried again.
    pub async fn reconcile_project(
        &self,
        namespace: &str,
        project_id: &str,
    ) -> Result<Pass, ReconcileError> {
        pass(&self.ctx, namespace, project_id).await
    }

    /// The resync's sweep: deletes the role secrets no record names that
    /// are older than [`secret_grace`](ReconcilerConfig::secret_grace), and
    /// answers how many it deleted.
    ///
    /// # Errors
    ///
    /// `Secrets` when listing failed, or when a delete failed (the sweep
    /// still tried the others); `Store` when the records could not be read
    /// (nothing more was deleted after that).
    pub async fn sweep_secrets(&self) -> Result<usize, ReconcileError> {
        resync::sweep(&self.ctx).await
    }

    /// Runs until `shutdown` resolves: watches, the resync timer and kicks
    /// mark projects; marked projects get passes, at most
    /// [`concurrency`](ReconcilerConfig::concurrency) at once and one per
    /// project at a time. A pass that failed or waits is tried again after a
    /// delay. Passes still running at shutdown are dropped: each step is
    /// safe to cut short (the module docs).
    ///
    /// # Errors
    ///
    /// `Running` when this reconciler is running or has run (its kick
    /// inbox is taken once); `Store` when a watch cannot start.
    pub async fn run(
        &self,
        shutdown: impl Future<Output = ()> + Send,
    ) -> Result<(), ReconcileError> {
        let mut kicks = self
            .inbox
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .ok_or(ReconcileError::Running)?;
        let store = &self.ctx.store;
        let mut projects = store.watch(b"x/").map_err(ReconcileError::Store)?;
        let mut branches = store.watch(b"X/").map_err(ReconcileError::Store)?;
        let mut endpoints = store.watch(b"E/").map_err(ReconcileError::Store)?;
        let config = &self.ctx.config;
        let mut resync = tokio::time::interval(config.resync);
        resync.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut sched = Schedule::default();
        let mut passes: JoinSet<(String, Result<Pass, ReconcileError>)> = JoinSet::new();
        let mut delays: JoinSet<String> = JoinSet::new();
        let mut sweeps: JoinSet<()> = JoinSet::new();
        let mut last_sweep: Option<tokio::time::Instant> = None;
        tokio::pin!(shutdown);
        tracing::info!(holder = %config.holder, "pg-control reconciler started");
        loop {
            tokio::select! {
                biased;
                () = &mut shutdown => break,
                Some(event) = projects.next() => sched.project_event(&event),
                Some(event) = branches.next() => sched.child_event(&event),
                Some(event) = endpoints.next() => sched.child_event(&event),
                Some(kick) = kicks.recv() => sched.kick(kick),
                Some(done) = passes.join_next(), if !passes.is_empty() => {
                    if let Ok((id, out)) = done
                        && let Some(after) = sched.finished(&id, &out, config)
                    {
                        delays.spawn(async move {
                            tokio::time::sleep(after).await;
                            id
                        });
                    }
                }
                Some(done) = delays.join_next(), if !delays.is_empty() => {
                    if let Ok(id) = done {
                        sched.mark(&id);
                    }
                }
                Some(_) = sweeps.join_next(), if !sweeps.is_empty() => {}
                _ = resync.tick() => {
                    sched.mark_all();
                    let due = last_sweep.is_none_or(|at| at.elapsed() >= config.sweep_every);
                    if due && sweeps.is_empty() {
                        last_sweep = Some(tokio::time::Instant::now());
                        let ctx = self.ctx.clone();
                        sweeps.spawn(async move {
                            match resync::sweep(&ctx).await {
                                Ok(0) => {}
                                Ok(n) => tracing::info!(deleted = n, "swept unused role secrets"),
                                Err(e) => tracing::warn!(error = %e, "the secret sweep stopped"),
                            }
                        });
                    }
                }
            }
            while passes.len() < config.concurrency.max(1) {
                let Some((namespace, id)) = sched.next() else {
                    break;
                };
                let ctx = self.ctx.clone();
                passes.spawn(async move {
                    let out = pass(&ctx, &namespace, &id).await;
                    (id, out)
                });
            }
        }
        passes.abort_all();
        delays.abort_all();
        sweeps.abort_all();
        tracing::info!(holder = %config.holder, "pg-control reconciler stopped");
        Ok(())
    }
}

/// One pass (see [`Reconciler::reconcile_project`]).
async fn pass<S: PgControlStore, N: NeonWrite>(
    ctx: &Ctx<S, N>,
    namespace: &str,
    project_id: &str,
) -> Result<Pass, ReconcileError> {
    let Some(snapshot) = project::Snapshot::read(ctx, namespace, project_id).await? else {
        ctx.leases.forget(project_id);
        return Ok(Pass::Idle);
    };
    if !snapshot.needs_work(ctx) {
        return Ok(Pass::Idle);
    }
    let fence = match ctx.leases.take(&ctx.store, project_id).await? {
        Taken::Held(fence) => fence,
        Taken::Other(holder) => return Ok(Pass::Held { holder }),
    };
    // Read again under the lease: what the first read saw may be done.
    let Some(snapshot) = project::Snapshot::read(ctx, namespace, project_id).await? else {
        return Ok(Pass::Idle);
    };
    let act = Act {
        ctx,
        project_id: project_id.to_string(),
        fence,
    };
    project::reconcile(&act, snapshot).await
}

/// Which projects the run loop is to pass over.
#[derive(Debug, Default)]
struct Schedule {
    /// Every project the watch reported, by id: its namespace.
    known: HashMap<String, String>,
    /// Marked for a pass.
    marked: BTreeSet<String>,
    running: HashSet<String>,
    /// The next wait after a failed pass, per project.
    backoff: HashMap<String, Duration>,
}

impl Schedule {
    /// `x/<ns>/<project_id>` (not the name index `x/<ns>/n/<name>`).
    fn project_event(&mut self, event: &StoreEvent) {
        let (key, put) = match event {
            StoreEvent::Put { key, .. } => (key, true),
            StoreEvent::Delete { key } => (key, false),
            StoreEvent::Synced => return,
        };
        let Ok(key) = std::str::from_utf8(key) else {
            return;
        };
        let mut parts = key.splitn(3, '/');
        let (Some("x"), Some(ns), Some(id)) = (parts.next(), parts.next(), parts.next()) else {
            return;
        };
        if !id.starts_with("prj-") || id.contains('/') {
            return;
        }
        if put {
            self.known.insert(id.to_string(), ns.to_string());
            self.marked.insert(id.to_string());
        } else {
            self.known.remove(id);
            self.marked.remove(id);
            self.backoff.remove(id);
        }
    }

    /// `X/<project_id>/…` or `E/<project_id>/…`: marks the project.
    fn child_event(&mut self, event: &StoreEvent) {
        let key = match event {
            StoreEvent::Put { key, .. } | StoreEvent::Delete { key } => key,
            StoreEvent::Synced => return,
        };
        let Ok(key) = std::str::from_utf8(key) else {
            return;
        };
        if let Some(id) = key.split('/').nth(1) {
            self.mark(id);
        }
    }

    fn kick(&mut self, kick: Kick) {
        let Kick::Project(id) = kick;
        self.mark(&id);
    }

    fn mark(&mut self, id: &str) {
        if self.known.contains_key(id) {
            self.marked.insert(id.to_string());
        }
    }

    fn mark_all(&mut self) {
        self.marked.extend(self.known.keys().cloned());
    }

    /// A marked project with no pass running, now running.
    fn next(&mut self) -> Option<(String, String)> {
        let id = self
            .marked
            .iter()
            .find(|id| !self.running.contains(*id))?
            .clone();
        self.marked.remove(&id);
        let Some(ns) = self.known.get(&id).cloned() else {
            return self.next();
        };
        self.running.insert(id.clone());
        Some((ns, id))
    }

    /// A pass ended: when to pass again, if the pass asks for a delay.
    fn finished(
        &mut self,
        id: &str,
        out: &Result<Pass, ReconcileError>,
        config: &ReconcilerConfig,
    ) -> Option<Duration> {
        self.running.remove(id);
        match out {
            Ok(Pass::Idle | Pass::Done | Pass::Held { .. }) => {
                self.backoff.remove(id);
                None
            }
            Ok(Pass::Again(after)) => {
                self.backoff.remove(id);
                if after.is_zero() {
                    self.mark(id);
                    None
                } else {
                    Some(*after)
                }
            }
            Err(e) => {
                let wait = self
                    .backoff
                    .get(id)
                    .map_or(config.retry_initial, |w| (*w * 2).min(config.retry_max));
                self.backoff.insert(id.to_string(), wait);
                match e {
                    ReconcileError::Moved | ReconcileError::Fenced => {
                        tracing::debug!(project = id, error = %e, "pass stopped");
                    }
                    _ => tracing::warn!(project = id, error = %e, retry_in = ?wait, "pass failed"),
                }
                Some(wait)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(key: &str) -> StoreEvent {
        StoreEvent::Put {
            key: key.as_bytes().to_vec(),
            version: 1,
        }
    }

    #[test]
    fn the_schedule_reads_project_keys_and_marks_known_projects() {
        let mut s = Schedule::default();
        s.child_event(&put("X/prj-A/br-1"));
        assert!(s.next().is_none(), "an unknown project is not passed over");
        s.project_event(&put("x/acme/n/shop"));
        s.project_event(&put("x/acme/prj-A"));
        assert_eq!(s.next(), Some(("acme".into(), "prj-A".into())));
        // Marked while running: passed over again once the pass ends.
        s.child_event(&put("E/prj-A/ep-1"));
        assert!(s.next().is_none());
        let config = ReconcilerConfig::new("a");
        assert_eq!(s.finished("prj-A", &Ok(Pass::Done), &config), None);
        assert_eq!(s.next(), Some(("acme".into(), "prj-A".into())));
        // Failures back off, doubling up to the cap.
        let failed = Err(ReconcileError::Moved);
        assert_eq!(
            s.finished("prj-A", &failed, &config),
            Some(config.retry_initial)
        );
        s.mark("prj-A");
        s.next();
        assert_eq!(
            s.finished("prj-A", &failed, &config),
            Some(config.retry_initial * 2)
        );
        s.project_event(&StoreEvent::Delete {
            key: b"x/acme/prj-A".to_vec(),
        });
        s.kick(Kick::Project("prj-A".into()));
        assert!(s.next().is_none(), "a deleted project is forgotten");
    }
}
