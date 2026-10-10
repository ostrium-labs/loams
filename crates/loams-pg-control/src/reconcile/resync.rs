//! The resync's sweep of unused role secrets (R6.1; PG2 Task 7).
//!
//! The API service stores a secret before the record that names it
//! commits, and deletes a replaced or removed one after the record moved
//! on. A crash, a failed delete or an unknown outcome in between leaves a
//! secret no record names: harmless, but kept forever without this sweep.
//!
//! The sweep lists the credential store, and for each role reference
//! (`pg-role-<project>-<ulid>`) older than
//! [`secret_grace`](super::ReconcilerConfig::secret_grace) deletes it
//! unless a role of one of its project's branches names it. A project that
//! is gone has no branches, so its references go too. Names outside the
//! role grammar are never touched.
//!
//! **Why no lease.** A record only ever names a reference made for it, a
//! moment before it commits (one call, far shorter than the grace). So a
//! reference older than the grace that no record names now will never be
//! named, and deleting it races with nothing. Two instances sweeping at
//! once delete the same secrets, which is harmless.

use std::collections::{BTreeMap, BTreeSet};

use super::{Ctx, ReconcileError, all};
use crate::ids::ProjectId;
use crate::model::{BranchPrefix, BranchRec, BranchScope, RoleRec};
use crate::secrets::SecretRef;
use crate::store::PgControlStore;

/// Sweeps; answers how many secrets it deleted.
pub(crate) async fn sweep<S: PgControlStore, N>(ctx: &Ctx<S, N>) -> Result<usize, ReconcileError> {
    let refs = ctx.secrets.list().await.map_err(ReconcileError::Secrets)?;
    let now = ctx.now_ms();
    let grace = u64::try_from(ctx.config.secret_grace.as_millis()).unwrap_or(u64::MAX);
    let mut old: BTreeMap<ProjectId, Vec<SecretRef>> = BTreeMap::new();
    for r in refs {
        if let Some((project, issued)) = r.role_parts()
            && now.saturating_sub(issued) > grace
        {
            old.entry(project).or_default().push(r);
        }
    }
    let mut deleted = 0;
    for (project, candidates) in old {
        let named = named(ctx, &project.to_string()).await?;
        for r in candidates.into_iter().filter(|r| !named.contains(r)) {
            match ctx.secrets.delete(&r).await {
                Ok(()) => {
                    tracing::info!(secret_ref = %r, "deleted a role secret no record names");
                    deleted += 1;
                }
                Err(e) => {
                    tracing::warn!(secret_ref = %r, error = %e, "an unused secret was not deleted")
                }
            }
        }
    }
    Ok(deleted)
}

/// Every reference a role of the project's branches names.
async fn named<S: PgControlStore, N>(
    ctx: &Ctx<S, N>,
    project_id: &str,
) -> Result<BTreeSet<SecretRef>, ReconcileError> {
    let branches = all::<S, BranchRec>(
        &ctx.store,
        &BranchPrefix {
            project_id: project_id.into(),
        },
    )
    .await?;
    let mut named = BTreeSet::new();
    for b in branches {
        let scope = BranchScope {
            branch_id: b.record.id,
        };
        for role in all::<S, RoleRec>(&ctx.store, &scope).await? {
            named.insert(role.record.secret_ref);
        }
    }
    Ok(named)
}
