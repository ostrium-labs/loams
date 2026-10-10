//! The branch reconciler (PG2 Task 7).
//!
//! - **Create** ([`create`]): the timeline on the pageserver (bootstrapped
//!   for `main`, a branch of the parent's timeline at `ancestor_lsn`, or at
//!   its head when none is recorded, R5.9), then on `loams-wal`, starting
//!   where the pageserver says the timeline starts (`last_record_lsn`, as
//!   Neon's control plane does); then one batch: the branch `ready`, its
//!   operation succeeded. A 409 from the pageserver means a pass before
//!   this one created the timeline (ids are derived from the branch), so it
//!   counts as done and the timeline is read instead; `loams-wal`'s create
//!   is idempotent by id (R2.10).
//! - **Remove** ([`remove`]): once no endpoint and no child branch is left,
//!   delete the timeline and wait until the pageserver no longer has it
//!   (its delete runs in the background; a missing timeline counts as
//!   deleted). Then one fenced batch removes the [`BranchRec`], its name
//!   index, its guard, its roles and databases (`R/<branch>/`,
//!   `D/<branch>/`), takes one off the parent guard's `children`, and ends
//!   the operations (R5.10, R6.8). The roles' secrets are deleted after the
//!   batch commits; one left behind by a crash is the resync's to sweep.
//!
//! `loams-wal` keeps a removed branch's WAL: `WalClient` has no timeline
//! delete yet (the report names it as a gap).

use super::project::{Ops, Snapshot};
use super::{Act, ReconcileError, Step, all, refused};
use crate::model::{
    BranchGuardKey, BranchGuardRec, BranchNameKey, BranchNameRec, BranchRec, BranchScope,
    BranchState, DatabaseRec, OperationError, OperationKind, OperationRec, ProjectRec, Record as _,
    RoleRec,
};
use crate::neon::{Lsn, NeonWrite, TenantId, TimelineCreate, TimelineId, WalTimelineCreate};
use crate::service::Reason;
use crate::store::{Batch, PgControlStore, Versioned};

/// How a [`remove`] ended.
pub(crate) enum Removal {
    Removed,
    /// Endpoints, children or the timeline's deletion are still there.
    Waiting,
    /// The delete was refused for good: the branch is `failed`, and its
    /// delete may be asked again.
    Failed,
}

/// The branch's timeline on the pageserver and on `loams-wal`, created or
/// found.
pub(crate) async fn ensure_timeline<S: PgControlStore, N: NeonWrite>(
    act: &Act<'_, S, N>,
    project: &ProjectRec,
    branch: &BranchRec,
    parent: Option<&BranchRec>,
) -> Result<Step<()>, ReconcileError> {
    let neon = &act.ctx.neon;
    let tenant = TenantId(project.tenant_id);
    let timeline = TimelineId(branch.timeline_id);
    let create = match parent {
        None => TimelineCreate::bootstrap(timeline, project.pg_version),
        Some(p) => TimelineCreate::branch(
            timeline,
            TimelineId(p.timeline_id),
            branch.ancestor_lsn.map(Lsn),
        ),
    };
    act.renew().await?;
    let view = match neon.create_timeline(tenant, &create).await {
        Ok(view) => view,
        Err(e) if e.reason == Reason::AlreadyExists => {
            tracing::info!(branch = %branch.id, "the timeline exists: an earlier pass created it");
            match neon.timeline(tenant, timeline).await {
                Ok(view) => view,
                Err(e) => return refused("reading the timeline", e),
            }
        }
        Err(e) => return refused("creating the timeline", e),
    };
    let wal = WalTimelineCreate {
        tenant_id: tenant,
        timeline_id: timeline,
        pg_version: project.pg_version,
        start_lsn: view.last_record_lsn,
        system_id: None,
        wal_seg_size: None,
        commit_lsn: None,
    };
    act.renew().await?;
    match neon.wal_create_timeline(&wal).await {
        Ok(_) => Ok(Step::Done(())),
        Err(e) if e.reason == Reason::AlreadyExists => Ok(Step::Done(())),
        Err(e) => refused("creating the timeline on loams-wal", e),
    }
}

/// Creates a `creating` branch's timeline, then marks it `ready`; answers
/// the state the branch is left in.
pub(crate) async fn create<S: PgControlStore, N: NeonWrite>(
    act: &Act<'_, S, N>,
    project: &ProjectRec,
    b: &Versioned<BranchRec>,
    parent: Option<&BranchRec>,
    ops: &Ops,
) -> Result<BranchState, ReconcileError> {
    let mut op = ops.of(OperationKind::BranchCreate, Some(&b.record.id));
    act.progress(&mut op, 0, 2, "creating_timeline").await?;
    if let Step::Fail(e) = ensure_timeline(act, project, &b.record, parent).await? {
        fail_create(act, b, &op, &e).await?;
        return Ok(BranchState::Failed);
    }
    let mut ready = b.record.clone();
    ready.state = BranchState::Ready;
    ready.updated_at_ms = act.ctx.now_ms();
    let mut batch = Batch::new();
    batch.put(&ready, Some(b.version))?;
    act.finish(&mut batch, &op, None)?;
    act.commit("branch.ready", batch).await?;
    tracing::info!(project = %project.id, branch = %b.record.id, "branch ready");
    Ok(BranchState::Ready)
}

/// A branch create refused for good: the branch and its operations `op`
/// (as this pass last wrote them) fail.
pub(crate) async fn fail_create<S: PgControlStore, N: NeonWrite>(
    act: &Act<'_, S, N>,
    b: &Versioned<BranchRec>,
    op: &[Versioned<OperationRec>],
    error: &OperationError,
) -> Result<(), ReconcileError> {
    let mut failed = b.record.clone();
    failed.state = BranchState::Failed;
    failed.updated_at_ms = act.ctx.now_ms();
    let mut batch = Batch::new();
    batch.put(&failed, Some(b.version))?;
    act.finish(&mut batch, op, Some(error))?;
    act.commit("branch.failed", batch).await?;
    tracing::warn!(branch = %b.record.id, reason = %error.reason, "branch create failed");
    Ok(())
}

/// A branch delete refused for good: the branch is `failed`, its guard no
/// longer `deleting` (so `DeleteBranch` takes it again), its operation
/// failed.
async fn fail_delete<S: PgControlStore, N: NeonWrite>(
    act: &Act<'_, S, N>,
    b: &Versioned<BranchRec>,
    op: &[Versioned<OperationRec>],
    error: &OperationError,
) -> Result<(), ReconcileError> {
    let mut failed = b.record.clone();
    failed.state = BranchState::Failed;
    failed.updated_at_ms = act.ctx.now_ms();
    let mut batch = Batch::new();
    batch.put(&failed, Some(b.version))?;
    let guard_key = BranchGuardKey {
        project_id: b.record.project_id.clone(),
        branch_id: b.record.id.clone(),
    };
    if let Some(guard) = act.ctx.store.get::<BranchGuardRec>(&guard_key).await? {
        let mut open = guard.record.clone();
        open.deleting = false;
        batch.put(&open, Some(guard.version))?;
    }
    act.finish(&mut batch, op, Some(error))?;
    act.commit("branch.failed", batch).await?;
    tracing::warn!(branch = %b.record.id, reason = %error.reason, "branch delete failed");
    Ok(())
}

/// Removes a branch: its timeline, then its records (see the module docs).
/// `for_project`: the project is being deleted, the caller hands over
/// leaves only, and a refusal for good is retried rather than failing the
/// branch.
pub(crate) async fn remove<S: PgControlStore, N: NeonWrite>(
    act: &Act<'_, S, N>,
    project: &ProjectRec,
    b: &Versioned<BranchRec>,
    snap: &Snapshot,
    ops: &Ops,
    for_project: bool,
) -> Result<Removal, ReconcileError> {
    let id = b.record.id.as_str();
    let mut op = ops.of(OperationKind::BranchDelete, Some(id));
    if snap.endpoints.iter().any(|e| e.record.branch_id == id) {
        act.progress(&mut op, 0, 3, "waiting_for_endpoints").await?;
        return Ok(Removal::Waiting);
    }
    if !for_project
        && snap
            .branches
            .iter()
            .any(|c| c.record.parent_id.as_deref() == Some(id))
    {
        act.progress(&mut op, 0, 3, "waiting_for_children").await?;
        return Ok(Removal::Waiting);
    }
    act.progress(&mut op, 1, 3, "deleting_timeline").await?;
    let neon = &act.ctx.neon;
    let (tenant, timeline) = (
        TenantId(project.tenant_id),
        TimelineId(b.record.timeline_id),
    );
    act.renew().await?;
    match neon.delete_timeline(tenant, timeline).await {
        Ok(()) => {}
        Err(e) if e.reason == Reason::NotFound => {}
        Err(e) if matches!(e.reason, Reason::Aborted | Reason::BranchHasChildren) => {
            tracing::info!(
                branch = id,
                reason = e.reason.as_str(),
                "the timeline's delete waits"
            );
            return Ok(Removal::Waiting);
        }
        Err(e) => {
            if let Step::Fail(error) = refused::<()>("deleting the timeline", e)? {
                if for_project {
                    return Ok(Removal::Waiting);
                }
                fail_delete(act, b, &op, &error).await?;
                return Ok(Removal::Failed);
            }
        }
    }
    // The pageserver deletes in the background: wait until it is gone.
    match neon.timeline(tenant, timeline).await {
        Err(e) if e.reason == Reason::NotFound => {}
        Ok(_) => return Ok(Removal::Waiting),
        Err(e) => {
            if let Step::Fail(_) = refused::<()>("reading the deleted timeline", e)? {
                return Ok(Removal::Waiting);
            }
        }
    }
    act.progress(&mut op, 2, 3, "removing_records").await?;
    let store = &act.ctx.store;
    let scope = BranchScope {
        branch_id: id.to_string(),
    };
    let roles = all::<S, RoleRec>(store, &scope).await?;
    let databases = all::<S, DatabaseRec>(store, &scope).await?;
    let mut batch = Batch::new();
    batch.delete::<BranchRec>(&b.record.key(), b.version)?;
    let name_key = BranchNameKey {
        project_id: project.id.clone(),
        name: b.record.name.clone(),
    };
    if let Some(name) = store.get::<BranchNameRec>(&name_key).await?
        && name.record.branch_id == id
    {
        batch.delete::<BranchNameRec>(&name_key, name.version)?;
    }
    let guard_key = BranchGuardKey {
        project_id: project.id.clone(),
        branch_id: id.to_string(),
    };
    if let Some(guard) = store.get::<BranchGuardRec>(&guard_key).await? {
        batch.delete::<BranchGuardRec>(&guard_key, guard.version)?;
    }
    if let Some(parent) = &b.record.parent_id {
        let parent_key = BranchGuardKey {
            project_id: project.id.clone(),
            branch_id: parent.clone(),
        };
        if let Some(guard) = store.get::<BranchGuardRec>(&parent_key).await? {
            let mut fewer = guard.record.clone();
            fewer.children = fewer.children.saturating_sub(1);
            batch.put(&fewer, Some(guard.version))?;
        }
    }
    for r in &roles {
        batch.delete::<RoleRec>(&r.record.key(), r.version)?;
    }
    for d in &databases {
        batch.delete::<DatabaseRec>(&d.record.key(), d.version)?;
    }
    act.finish(&mut batch, &op, None)?;
    let aborted = OperationError {
        reason: "aborted".into(),
        message: "the branch was deleted before it was ready".into(),
    };
    act.finish(
        &mut batch,
        &ops.of(OperationKind::BranchCreate, Some(id)),
        Some(&aborted),
    )?;
    act.commit("branch.remove", batch).await?;
    tracing::info!(project = %project.id, branch = id, roles = roles.len(), databases = databases.len(), "branch removed");
    for r in &roles {
        if let Err(e) = act.ctx.secrets.delete(&r.record.secret_ref).await {
            tracing::warn!(secret_ref = %r.record.secret_ref, error = %e, "a removed role's secret was not deleted; the resync sweeps it");
        }
    }
    Ok(Removal::Removed)
}
