//! The project reconciler (PG2 Task 7).
//!
//! - **`creating`:** attach the tenant with its `pitr_interval`, set it
//!   again (`tenant_config`, which also covers an attach the storage
//!   controller answered 409 for), create `main`'s timeline ([`branch`]),
//!   then one batch: the project and `main` `ready`, the operation
//!   succeeded. A refusal that retrying will not pass fails the project,
//!   its `creating` branches and their operations.
//! - **`ready`:** a changed `history_retention` is set as the tenant's
//!   `pitr_interval`; branches `creating` are created (parents first) and
//!   branches `deleting` removed (children first).
//! - **`deleting`:** once the project's endpoints are gone, every branch is
//!   removed, children first ([`branch::remove`]); then one batch removes
//!   the project and its name index, and ends the operation. The tenant
//!   stays attached: `loams-postgres` has no tenant delete yet (the report
//!   names it as a gap). The project's operations stay readable.
//! - **`failed`:** only branch deletes.
//!
//! [`branch`]: super::branch

use std::collections::BTreeMap;
use std::time::Duration;

use super::branch::{self, Removal};
use super::{Act, Ctx, Pass, ReconcileError, Step, all, refused};
use crate::model::{
    BranchPrefix, BranchRec, BranchState, EndpointPrefix, EndpointRec, OperationError,
    OperationKind, OperationPrefix, OperationRec, OperationState, ProjectKey, ProjectNameKey,
    ProjectNameRec, ProjectRec, ProjectState, Record as _,
};
use crate::neon::{NeonWrite, TenantConfig, TenantId};
use crate::store::{Batch, PgControlStore, Versioned};

/// A project's records, as one pass reads them.
pub(crate) struct Snapshot {
    pub project: Versioned<ProjectRec>,
    pub branches: Vec<Versioned<BranchRec>>,
    pub endpoints: Vec<Versioned<EndpointRec>>,
}

impl Snapshot {
    /// The project's records; `None` when the project is gone.
    pub async fn read<S: PgControlStore, N>(
        ctx: &Ctx<S, N>,
        namespace: &str,
        project_id: &str,
    ) -> Result<Option<Snapshot>, ReconcileError> {
        let key = ProjectKey {
            namespace: namespace.into(),
            id: project_id.into(),
        };
        let Some(project) = ctx.store.get::<ProjectRec>(&key).await? else {
            return Ok(None);
        };
        let branches = all::<S, BranchRec>(
            &ctx.store,
            &BranchPrefix {
                project_id: project_id.into(),
            },
        )
        .await?;
        let endpoints = all::<S, EndpointRec>(
            &ctx.store,
            &EndpointPrefix {
                project_id: project_id.into(),
            },
        )
        .await?;
        Ok(Some(Snapshot {
            project,
            branches,
            endpoints,
        }))
    }

    /// Whether a pass has anything to do (without the lease).
    pub fn needs_work<S, N>(&self, ctx: &Ctx<S, N>) -> bool {
        let p = &self.project.record;
        let any = |state| self.branches.iter().any(|b| b.record.state == state);
        match p.state {
            ProjectState::Creating | ProjectState::Deleting => true,
            ProjectState::Failed => any(BranchState::Deleting),
            ProjectState::Ready => {
                ctx.applied().get(&p.id) != Some(&p.history_retention_s)
                    || any(BranchState::Creating)
                    || any(BranchState::Deleting)
            }
        }
    }

    pub fn branch(&self, id: &str) -> Option<&Versioned<BranchRec>> {
        self.branches.iter().find(|b| b.record.id == id)
    }
}

/// A project's open (`Pending` or `Running`) operations.
pub(crate) struct Ops(Vec<Versioned<OperationRec>>);

impl Ops {
    pub async fn read<S: PgControlStore, N>(
        ctx: &Ctx<S, N>,
        project_id: &str,
    ) -> Result<Ops, ReconcileError> {
        let ops = all::<S, OperationRec>(
            &ctx.store,
            &OperationPrefix {
                project_id: project_id.into(),
            },
        )
        .await?;
        Ok(Ops(ops
            .into_iter()
            .filter(|o| {
                matches!(
                    o.record.state,
                    OperationState::Pending | OperationState::Running
                )
            })
            .collect()))
    }

    /// The open operations of `kind` on `branch` (`None`: the project's).
    pub fn of(&self, kind: OperationKind, branch: Option<&str>) -> Vec<Versioned<OperationRec>> {
        self.0
            .iter()
            .filter(|o| o.record.kind == kind && o.record.branch_id.as_deref() == branch)
            .cloned()
            .collect()
    }
}

fn tenant_config(p: &ProjectRec) -> TenantConfig {
    TenantConfig {
        pitr_interval: Some(Duration::from_secs(p.history_retention_s)),
        ..TenantConfig::default()
    }
}

/// One pass under the lease.
pub(crate) async fn reconcile<S: PgControlStore, N: NeonWrite>(
    act: &Act<'_, S, N>,
    snapshot: Snapshot,
) -> Result<Pass, ReconcileError> {
    let ops = Ops::read(act.ctx, &act.project_id).await?;
    match snapshot.project.record.state {
        ProjectState::Creating => create(act, &snapshot, &ops).await,
        ProjectState::Ready => ready(act, snapshot, &ops).await,
        ProjectState::Deleting => delete(act, snapshot, &ops).await,
        ProjectState::Failed => branches(act, snapshot, &ops, false).await,
    }
}

/// `creating`: the tenant and `main`, then `ready`.
async fn create<S: PgControlStore, N: NeonWrite>(
    act: &Act<'_, S, N>,
    snap: &Snapshot,
    ops: &Ops,
) -> Result<Pass, ReconcileError> {
    let p = &snap.project.record;
    let mut op = ops.of(OperationKind::ProjectCreate, None);
    let Some(main) = p
        .default_branch_id
        .as_deref()
        .and_then(|id| snap.branch(id))
    else {
        let e = OperationError {
            reason: "internal".into(),
            message: "the project has no default branch record".into(),
        };
        tracing::error!(project = %p.id, "a creating project has no main branch");
        return fail(act, snap, &op, &e).await;
    };
    act.progress(&mut op, 0, 3, "attaching_tenant").await?;
    let tenant = TenantId(p.tenant_id);
    let config = tenant_config(p);
    act.renew().await?;
    let attached = match act
        .ctx
        .neon
        .attach_tenant(tenant, act.ctx.config.tenant_generation, &config)
        .await
    {
        Ok(()) => Step::Done(()),
        Err(e) if e.reason == crate::service::Reason::AlreadyExists => Step::Done(()),
        Err(e) => refused("attaching the tenant", e)?,
    };
    if let Step::Fail(e) = attached {
        return fail(act, snap, &op, &e).await;
    }
    act.renew().await?;
    if let Step::Fail(e) = match act.ctx.neon.tenant_config(tenant, &config).await {
        Ok(()) => Step::Done(()),
        Err(e) => refused("setting pitr_interval", e)?,
    } {
        return fail(act, snap, &op, &e).await;
    }
    act.progress(&mut op, 1, 3, "creating_timeline").await?;
    if let Step::Fail(e) = branch::ensure_timeline(act, p, &main.record, None).await? {
        return fail(act, snap, &op, &e).await;
    }
    let now = act.ctx.now_ms();
    let mut project = p.clone();
    project.state = ProjectState::Ready;
    project.updated_at_ms = now;
    let mut ready_main = main.record.clone();
    ready_main.state = BranchState::Ready;
    ready_main.updated_at_ms = now;
    let mut batch = Batch::new();
    batch.put(&project, Some(snap.project.version))?;
    batch.put(&ready_main, Some(main.version))?;
    act.finish(&mut batch, &op, None)?;
    act.commit("project.ready", batch).await?;
    act.ctx
        .applied()
        .insert(p.id.clone(), p.history_retention_s);
    tracing::info!(project = %p.id, "project ready");
    Ok(Pass::Done)
}

/// A project create refused for good: the project, its `creating`
/// branches and their operations fail.
async fn fail<S: PgControlStore, N: NeonWrite>(
    act: &Act<'_, S, N>,
    snap: &Snapshot,
    op: &[Versioned<OperationRec>],
    error: &OperationError,
) -> Result<Pass, ReconcileError> {
    let ops = Ops::read(act.ctx, &act.project_id).await?;
    let now = act.ctx.now_ms();
    let mut project = snap.project.record.clone();
    project.state = ProjectState::Failed;
    project.updated_at_ms = now;
    let mut batch = Batch::new();
    batch.put(&project, Some(snap.project.version))?;
    act.finish(&mut batch, op, Some(error))?;
    for b in &snap.branches {
        if b.record.state != BranchState::Creating {
            continue;
        }
        let mut failed = b.record.clone();
        failed.state = BranchState::Failed;
        failed.updated_at_ms = now;
        batch.put(&failed, Some(b.version))?;
        act.finish(
            &mut batch,
            &ops.of(OperationKind::BranchCreate, Some(&b.record.id)),
            Some(error),
        )?;
    }
    act.commit("project.failed", batch).await?;
    tracing::warn!(project = %project.id, reason = %error.reason, "project create failed");
    Ok(Pass::Done)
}

/// `ready`: the retention, then the branches.
async fn ready<S: PgControlStore, N: NeonWrite>(
    act: &Act<'_, S, N>,
    snap: Snapshot,
    ops: &Ops,
) -> Result<Pass, ReconcileError> {
    let p = &snap.project.record;
    if act.ctx.applied().get(&p.id) != Some(&p.history_retention_s) {
        act.renew().await?;
        match act
            .ctx
            .neon
            .tenant_config(TenantId(p.tenant_id), &tenant_config(p))
            .await
        {
            Ok(()) => {
                act.ctx
                    .applied()
                    .insert(p.id.clone(), p.history_retention_s);
            }
            Err(e) => {
                // A refusal for good is logged and not retried until the
                // retention changes again or the process restarts.
                if let Step::Fail(e) = refused::<()>("setting pitr_interval", e)? {
                    tracing::error!(project = %p.id, reason = %e.reason, "pitr_interval not set");
                    act.ctx
                        .applied()
                        .insert(p.id.clone(), p.history_retention_s);
                }
            }
        }
    }
    branches(act, snap, ops, true).await
}

/// Creates the `creating` branches (when `create`) and removes the
/// `deleting` ones.
async fn branches<S: PgControlStore, N: NeonWrite>(
    act: &Act<'_, S, N>,
    snap: Snapshot,
    ops: &Ops,
    create: bool,
) -> Result<Pass, ReconcileError> {
    let p = snap.project.record.clone();
    // Branch states as this pass leaves them, so a child created after its
    // parent in the same pass sees the parent ready.
    let mut states: BTreeMap<String, BranchState> = snap
        .branches
        .iter()
        .map(|b| (b.record.id.clone(), b.record.state))
        .collect();
    let mut waiting = false;
    if create {
        let mut creating: Vec<&Versioned<BranchRec>> = snap
            .branches
            .iter()
            .filter(|b| b.record.state == BranchState::Creating)
            .collect();
        // A parent is always created before its children.
        creating.sort_by_key(|b| (b.record.created_at_ms, b.record.id.clone()));
        for b in creating {
            let parent = b.record.parent_id.as_deref().and_then(|id| snap.branch(id));
            let parent_state = b
                .record
                .parent_id
                .as_ref()
                .map(|id| states.get(id).copied());
            match parent_state {
                Some(Some(BranchState::Creating)) => {
                    waiting = true;
                    continue;
                }
                Some(Some(BranchState::Ready)) | None => {}
                Some(_) => {
                    let e = OperationError {
                        reason: "failed_precondition".into(),
                        message: "the parent branch is gone, failed or deleting".into(),
                    };
                    let op = ops.of(OperationKind::BranchCreate, Some(&b.record.id));
                    branch::fail_create(act, b, &op, &e).await?;
                    states.insert(b.record.id.clone(), BranchState::Failed);
                    continue;
                }
            }
            let state = branch::create(act, &p, b, parent.map(|v| &v.record), ops).await?;
            states.insert(b.record.id.clone(), state);
        }
    }
    let mut deleting: Vec<&Versioned<BranchRec>> = snap
        .branches
        .iter()
        .filter(|b| b.record.state == BranchState::Deleting)
        .collect();
    // Children first: the newest branches are the deepest.
    deleting.sort_by_key(|b| std::cmp::Reverse((b.record.created_at_ms, b.record.id.clone())));
    for b in deleting {
        match branch::remove(act, &p, b, &snap, ops, false).await? {
            Removal::Removed | Removal::Failed => {}
            Removal::Waiting => waiting = true,
        }
    }
    Ok(if waiting {
        Pass::Again(act.ctx.config.retry_initial)
    } else {
        Pass::Done
    })
}

/// `deleting`: every branch, children first, then the project's records.
async fn delete<S: PgControlStore, N: NeonWrite>(
    act: &Act<'_, S, N>,
    snap: Snapshot,
    ops: &Ops,
) -> Result<Pass, ReconcileError> {
    let p = snap.project.record.clone();
    let mut op = ops.of(OperationKind::ProjectDelete, None);
    if !snap.endpoints.is_empty() {
        act.progress(&mut op, 0, 0, "waiting_for_endpoints").await?;
        return Ok(Pass::Again(act.ctx.config.retry_initial));
    }
    // One step per branch, and one for the project's records; the total is
    // the first pass's (the branches only go from there).
    let total = op
        .first()
        .and_then(|o| o.record.progress.as_ref())
        .filter(|pr| pr.phase == "deleting_branches")
        .map_or(snap.branches.len() as u64 + 1, |pr| pr.total);
    let mut left: Vec<&Versioned<BranchRec>> = snap.branches.iter().collect();
    left.sort_by_key(|b| std::cmp::Reverse((b.record.created_at_ms, b.record.id.clone())));
    while !left.is_empty() {
        let done = total.saturating_sub(left.len() as u64 + 1);
        act.progress(&mut op, done, total, "deleting_branches")
            .await?;
        // A leaf: no branch left names it as its parent.
        let Some(at) = left.iter().position(|b| {
            !left
                .iter()
                .any(|c| c.record.parent_id.as_deref() == Some(b.record.id.as_str()))
        }) else {
            tracing::error!(project = %p.id, "the project's branches form a cycle");
            return Ok(Pass::Again(act.ctx.config.retry_max));
        };
        match branch::remove(act, &p, left[at], &snap, ops, true).await? {
            Removal::Removed => {
                left.remove(at);
            }
            Removal::Waiting | Removal::Failed => {
                return Ok(Pass::Again(act.ctx.config.retry_initial));
            }
        }
    }
    let name_key = ProjectNameKey {
        namespace: p.namespace.clone(),
        name: p.name.clone(),
    };
    let mut batch = Batch::new();
    batch.delete::<ProjectRec>(&p.key(), snap.project.version)?;
    if let Some(name) = act.ctx.store.get::<ProjectNameRec>(&name_key).await?
        && name.record.project_id == p.id
    {
        batch.delete::<ProjectNameRec>(&name_key, name.version)?;
    }
    act.finish(&mut batch, &op, None)?;
    let aborted = OperationError {
        reason: "aborted".into(),
        message: "the project was deleted before it was ready".into(),
    };
    act.finish(
        &mut batch,
        &ops.of(OperationKind::ProjectCreate, None),
        Some(&aborted),
    )?;
    act.commit("project.remove", batch).await?;
    act.ctx.applied().remove(&p.id);
    act.ctx.leases.forget(&p.id);
    tracing::info!(project = %p.id, "project removed");
    Ok(Pass::Done)
}
