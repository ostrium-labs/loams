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
//! **Why no lease.** A record only ever names a reference made for its own
//! call, and the service commits it only within
//! [`ISSUE_WINDOW`](crate::service::ISSUE_WINDOW) of the reference's issue.
//! The grace is at least [`MIN_SECRET_GRACE`] (twice that window, which
//! leaves the commit itself minutes). So a reference older than the grace
//! that no record names now will never be named, and deleting it races
//! with nothing. Two instances sweeping at once delete the same secrets,
//! which is harmless.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use super::{Ctx, ReconcileError, all};
use crate::ids::ProjectId;
use crate::model::{BranchPrefix, BranchRec, BranchScope, RoleRec};
use crate::secrets::SecretRef;
use crate::store::PgControlStore;

/// The least grace the sweep allows, whatever the configuration says:
/// twice the service's issue window.
pub const MIN_SECRET_GRACE: Duration = Duration::from_secs(2 * 300);

const _: () = assert!(MIN_SECRET_GRACE.as_secs() >= 2 * crate::service::ISSUE_WINDOW.as_secs());

/// Sweeps; answers how many secrets it deleted. A secret it could not
/// delete does not stop it: it goes on with the others, then answers the
/// first such failure.
pub(crate) async fn sweep<S: PgControlStore, N>(ctx: &Ctx<S, N>) -> Result<usize, ReconcileError> {
    let refs = ctx.secrets.list().await.map_err(ReconcileError::Secrets)?;
    let now = ctx.now_ms();
    let grace = ctx.config.secret_grace.max(MIN_SECRET_GRACE);
    let grace = u64::try_from(grace.as_millis()).unwrap_or(u64::MAX);
    let mut old: BTreeMap<ProjectId, Vec<SecretRef>> = BTreeMap::new();
    for r in refs {
        if let Some((project, issued)) = r.role_parts()
            && now.saturating_sub(issued) > grace
        {
            old.entry(project).or_default().push(r);
        }
    }
    let mut deleted = 0;
    let mut failed = None;
    for (project, candidates) in old {
        let named = named(ctx, &project.to_string()).await?;
        for r in candidates.into_iter().filter(|r| !named.contains(r)) {
            match ctx.secrets.delete(&r).await {
                Ok(()) => {
                    tracing::info!(secret_ref = %r, "deleted a role secret no record names");
                    deleted += 1;
                }
                Err(e) => {
                    tracing::warn!(secret_ref = %r, error = %e, "an unused secret was not deleted");
                    failed.get_or_insert(e);
                }
            }
        }
    }
    match failed {
        Some(e) => Err(ReconcileError::Secrets(e)),
        None => Ok(deleted),
    }
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
