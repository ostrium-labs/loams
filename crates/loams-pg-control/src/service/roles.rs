//! `CreateRole`, `ListRoles`, `ResetRolePassword` and `DeleteRole` (§46 §3,
//! §8.6, D711; PG2 Task 6).
//!
//! A role is a record of a branch ([`RoleRec`] at `R/<branch_id>/<role>`);
//! the compute spec applies it (Tasks 11, 24). Its password is 32 random
//! bytes in base64url, generated here, stored in the [`SecretStore`]
//! under a fresh [`SecretRef`], and answered once.
//!
//! - **Order.** The secret is stored first, then the record that names it is
//!   committed (with the idempotency entry, R5.2). A reset commits the new
//!   reference, then deletes the old secret. A secret whose record did not
//!   commit is deleted again ([`PgService::discard`]), unless the commit's
//!   outcome is unknown: then the record may name it, and it is kept.
//! - **Once.** The ledger records the role, never the password (R1.10). A
//!   replay answers `secret_already_issued`. A call whose batch lost to a
//!   concurrent call under the same key answers it too, since the winner's
//!   password is not this call's: the ledger's answer names the winner's
//!   reference.
//! - **Branches.** A child branch copies its parent's roles and databases
//!   when it is created ([`PgService::inherit`]), each role with a copy of
//!   its secret under the child's own reference: the timeline's catalog has
//!   the same roles and passwords, and from then on each branch's roles
//!   change on their own.
//! - Passwords never reach a log line, an error, a record or the ledger;
//!   references (not secrets) are logged.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{
    ATTEMPTS, Applied, Begin, Caller, Mutation, NeonRead, PgService, Reason, ServiceError,
    list_error, page,
};
use crate::ids::ProjectId;
use crate::model::{
    BranchRec, BranchScope, BranchState, DatabaseRec, ProjectRec, ProjectState, Record, RoleKey,
    RoleRec,
};
use crate::names::MAX_NAME_LEN;
use crate::secrets::{Secret, SecretError, SecretRef, generate_password};
use crate::store::{Batch, PgControlStore, Versioned};

/// Role names Postgres, Neon's compute or Loams keep (compared lower-cased):
/// Postgres refuses `public` and `none`; `compute_ctl` runs as `cloud_admin`
/// and grants through `neon_superuser` (or its `--privileged-role-name`);
/// `zenith_admin` is its legacy superuser.
const RESERVED_ROLES: [&str; 6] = [
    "public",
    "none",
    "cloud_admin",
    "neon_superuser",
    "databricks_superuser",
    "zenith_admin",
];

/// Prefixes of reserved role names: Postgres's `pg_`, credential
/// exchange's login roles `tok_` (§46 §8.6), and `loams_` for the roles
/// Loams manages (agents' `loams_ro`, Task 0 ruling 8).
const RESERVED_ROLE_PREFIXES: [&str; 3] = ["pg_", "tok_", "loams_"];

/// `CreateRoleRequest`. An empty `branch_id` takes the project's default
/// branch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateRole {
    pub namespace: String,
    pub project_id: String,
    pub branch_id: String,
    pub name: String,
    pub idempotency_key: String,
}

/// `ResetRolePasswordRequest`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResetRolePassword {
    pub namespace: String,
    pub project_id: String,
    pub branch_id: String,
    pub name: String,
    pub idempotency_key: String,
}

/// `DeleteRoleRequest`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteRole {
    pub namespace: String,
    pub project_id: String,
    pub branch_id: String,
    pub name: String,
    pub idempotency_key: String,
}

/// `CreateRoleResponse` and `ResetRolePasswordResponse`: the role, and its
/// password, answered this once. `Debug` prints the password `[redacted]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleIssued {
    pub role: Versioned<RoleRec>,
    pub password: Secret<String>,
}

/// The project and branch a role or database request names.
pub(crate) struct Scope {
    pub project: Versioned<ProjectRec>,
    pub branch: Versioned<BranchRec>,
}

impl Scope {
    /// Checks, in `batch`, that neither the project nor the branch moved
    /// since they were read: a delete of either meanwhile fails the call.
    pub fn check(&self, batch: &mut Batch) -> Result<(), ServiceError> {
        batch.check::<ProjectRec>(&self.project.record.key(), Some(self.project.version))?;
        batch.check::<BranchRec>(&self.branch.record.key(), Some(self.branch.version))?;
        Ok(())
    }

    pub fn branch_id(&self) -> &str {
        &self.branch.record.id
    }
}

/// A role name: a Postgres identifier (1 to 63 bytes, no NUL) that no one
/// else keeps.
fn check_role_name(name: &str) -> Result<(), ServiceError> {
    let bad = |why: &str| ServiceError::invalid("name", format!("role name {name:?} {why}"));
    if name.is_empty() || name.len() > MAX_NAME_LEN {
        return Err(bad("is not 1 to 63 bytes"));
    }
    if name.contains('\0') {
        return Err(bad("contains NUL"));
    }
    let lower = name.to_lowercase();
    if RESERVED_ROLES.contains(&lower.as_str())
        || RESERVED_ROLE_PREFIXES.iter().any(|p| lower.starts_with(p))
    {
        return Err(bad("is reserved"));
    }
    Ok(())
}

/// `secret_already_issued` for `role` (R1.8's metadata and hint).
fn already_issued(role: &str) -> ServiceError {
    ServiceError::new(
        Reason::SecretAlreadyIssued,
        format!("the password of role {role} was answered once and is not kept"),
    )
    .with("role", role)
    .with(
        "hint",
        "call ResetRolePassword for a new password; the password was answered only to the first call",
    )
}

fn role_exists(name: &str) -> ServiceError {
    ServiceError::new(
        Reason::AlreadyExists,
        format!("a role named {name} exists on this branch"),
    )
}

/// `failed_precondition` for a role Loams manages.
fn system_role(name: &str) -> ServiceError {
    ServiceError::failed_precondition(format!(
        "role {name} is managed by Loams and cannot be changed through the API"
    ))
}

/// The ledger's answer, if it is this call's (its secret reference is
/// this call's); otherwise a concurrent call under the same key won, and
/// its password is not this call's to answer.
fn mine(
    first: Versioned<RoleRec>,
    secret_ref: &SecretRef,
) -> Result<Versioned<RoleRec>, ServiceError> {
    if first.record.secret_ref == *secret_ref {
        Ok(first)
    } else {
        Err(already_issued(&first.record.name))
    }
}

/// What a new branch copies from its parent.
pub(crate) struct Inherited {
    /// The parent's roles as read: the child's batch checks each.
    pub parents: Vec<Versioned<RoleRec>>,
    /// The child's roles, each naming its own copy of the secret.
    pub roles: Vec<RoleRec>,
    /// The child's databases: those of the parent whose owner it copied.
    pub databases: Vec<DatabaseRec>,
}

impl<N: NeonRead> PgService<N> {
    /// The project and the branch (empty: the default branch) a request
    /// names, in any state.
    pub(crate) async fn scope(
        &self,
        namespace: &str,
        project_id: &str,
        branch_id: &str,
    ) -> Result<Scope, ServiceError> {
        let project = self.project(namespace, project_id).await?;
        let id = match branch_id {
            "" => project.record.default_branch_id.clone().ok_or_else(|| {
                ServiceError::failed_precondition("the project has no default branch")
            })?,
            id => id.to_string(),
        };
        let branch = self.branch(project_id, &id, "branch_id").await?;
        Ok(Scope { project, branch })
    }

    /// As [`scope`](Self::scope), for a change: neither the project nor the
    /// branch may be deleting or failed.
    pub(crate) async fn writable_scope(
        &self,
        namespace: &str,
        project_id: &str,
        branch_id: &str,
    ) -> Result<Scope, ServiceError> {
        let scope = self.scope(namespace, project_id, branch_id).await?;
        if !matches!(
            scope.project.record.state,
            ProjectState::Creating | ProjectState::Ready
        ) {
            return Err(ServiceError::failed_precondition(
                "the project is being deleted or has failed",
            ));
        }
        if !matches!(
            scope.branch.record.state,
            BranchState::Creating | BranchState::Ready
        ) {
            return Err(ServiceError::failed_precondition(format!(
                "branch {} is being deleted or has failed",
                scope.branch_id()
            )));
        }
        Ok(scope)
    }

    /// The role `name` of a branch.
    async fn role(&self, branch_id: &str, name: &str) -> Result<Versioned<RoleRec>, ServiceError> {
        self.store
            .get::<RoleRec>(&RoleKey {
                branch_id: branch_id.into(),
                name: name.into(),
            })
            .await?
            .ok_or_else(|| ServiceError::not_found("role", name))
    }

    /// Stores `password` for this call under a fresh reference, once:
    /// a later try of the same call reuses it, for the role at `key`.
    async fn issue(
        &self,
        project_id: &str,
        key: &RoleKey,
        password: &Secret<String>,
        issued: &mut Option<(RoleKey, SecretRef)>,
    ) -> Result<SecretRef, ServiceError> {
        if let Some((at, r)) = issued {
            *at = key.clone();
            return Ok(r.clone());
        }
        let project: ProjectId = project_id
            .parse()
            .map_err(|e: crate::ids::IdError| ServiceError::invalid("project_id", e.to_string()))?;
        let r = SecretRef::new_role(&project);
        self.secrets
            .put(&r, Secret::new(password.expose().as_bytes().to_vec()))
            .await
            .map_err(|e| ServiceError::secret(&e))?;
        *issued = Some((key.clone(), r.clone()));
        Ok(r)
    }

    /// Deletes each secret its role record does not name: secrets of
    /// writes that did not commit. A secret whose record cannot be read is
    /// kept, and so is any secret after an unknown outcome (the caller does
    /// not call this then). A kept secret no record names is unused.
    pub(crate) async fn discard(&self, secrets: Vec<(RoleKey, SecretRef)>) {
        for (key, r) in secrets {
            match self.store.get::<RoleRec>(&key).await {
                Ok(Some(rec)) if rec.record.secret_ref == r => {}
                Ok(_) => self.forget(&r).await,
                Err(e) => {
                    tracing::warn!(secret_ref = %r, error = %e, "kept a secret whose use is unknown");
                }
            }
        }
    }

    /// Deletes a secret no record names any more; a failure leaves it,
    /// unused, and is logged.
    async fn forget(&self, r: &SecretRef) {
        if let Err(e) = self.secrets.delete(r).await {
            tracing::warn!(secret_ref = %r, error = %e, "an unused secret was not deleted");
        }
    }

    /// `CreateRole`: a login role on the branch, and its password, once.
    ///
    /// # Errors
    ///
    /// `invalid_argument` (a reserved or malformed name); `not_found` for
    /// the branch, `project_not_found`; `failed_precondition` while the
    /// project or branch is deleting or failed; `already_exists`;
    /// `secret_already_issued` on a replay; `unavailable` when the
    /// credential store or the control store is down.
    pub async fn create_role(
        &self,
        caller: &Caller,
        req: CreateRole,
    ) -> Result<RoleIssued, ServiceError> {
        check_role_name(&req.name)?;
        let password = generate_password().map_err(|e| ServiceError::secret(&e))?;
        let mut issued = None;
        let out = self
            .create_role_tries(caller, &req, &password, &mut issued)
            .await;
        match out {
            Ok(role) => {
                tracing::info!(branch = %role.record.branch_id, role = %role.record.name, secret_ref = %role.record.secret_ref, "issued a role password");
                Ok(RoleIssued { role, password })
            }
            Err(e) => {
                if let Some(s) = issued.filter(|_| !e.is_undetermined()) {
                    self.discard(vec![s]).await;
                }
                Err(e)
            }
        }
    }

    async fn create_role_tries(
        &self,
        caller: &Caller,
        req: &CreateRole,
        password: &Secret<String>,
        issued: &mut Option<(RoleKey, SecretRef)>,
    ) -> Result<Versioned<RoleRec>, ServiceError> {
        for _ in 0..ATTEMPTS {
            let claim = match self
                .begin::<Versioned<RoleRec>, _>(caller, "CreateRole", &req.idempotency_key, req)
                .await?
            {
                Begin::Replay(first) => return Err(already_issued(&first.record.name)),
                Begin::Fresh(claim) => claim,
            };
            let scope = self
                .writable_scope(&req.namespace, &req.project_id, &req.branch_id)
                .await?;
            let key = RoleKey {
                branch_id: scope.branch_id().into(),
                name: req.name.clone(),
            };
            if self.store.get::<RoleRec>(&key).await?.is_some() {
                return Err(role_exists(&req.name));
            }
            let secret_ref = self
                .issue(&scope.project.record.id, &key, password, issued)
                .await?;
            let now = self.now_ms();
            let rec = RoleRec {
                project_id: scope.project.record.id.clone(),
                branch_id: key.branch_id.clone(),
                name: req.name.clone(),
                secret_ref: secret_ref.clone(),
                login: true,
                pool_mode: None,
                system: false,
                created_at_ms: now,
                updated_at_ms: now,
            };
            let mut batch = Batch::new();
            let at = batch.put(&rec, None)?;
            scope.check(&mut batch)?;
            let mutation = Mutation::new(batch, move |out| Versioned {
                record: rec.clone(),
                version: out[at].unwrap_or_default(),
            })
            .conflict_means(at, role_exists(&req.name));
            if let Applied::Done(first) = self.apply("CreateRole", claim.as_ref(), mutation).await?
            {
                return mine(first, &secret_ref);
            }
        }
        Err(Self::contended())
    }

    /// `ListRoles`: one page of a branch's roles in name order, and the
    /// next page's token (empty on the last page). No secret.
    ///
    /// # Errors
    ///
    /// `project_not_found`; `not_found` for the branch; `invalid_argument`
    /// for a negative size or a foreign token.
    pub async fn list_roles(
        &self,
        namespace: &str,
        project_id: &str,
        branch_id: &str,
        page_size: i32,
        page_token: &str,
    ) -> Result<(Vec<Versioned<RoleRec>>, String), ServiceError> {
        let scope = self.scope(namespace, project_id, branch_id).await?;
        let prefix = BranchScope {
            branch_id: scope.branch_id().into(),
        };
        let (roles, next) = self
            .store
            .list::<RoleRec>(&prefix, page(page_size, page_token)?)
            .await
            .map_err(list_error)?;
        Ok((roles, next.unwrap_or_default()))
    }

    /// `ResetRolePassword`: a new password, once; the old one stops
    /// working when the compute spec and PgDog take the new record (Tasks
    /// 11, 24), and its secret is deleted.
    ///
    /// # Errors
    ///
    /// As [`create_role`](Self::create_role), with `not_found` for the role
    /// and `failed_precondition` for a role Loams manages.
    pub async fn reset_role_password(
        &self,
        caller: &Caller,
        req: ResetRolePassword,
    ) -> Result<RoleIssued, ServiceError> {
        if req.name.is_empty() {
            return Err(ServiceError::invalid("name", "a role name is required"));
        }
        let password = generate_password().map_err(|e| ServiceError::secret(&e))?;
        let mut issued = None;
        match self.reset_tries(caller, &req, &password, &mut issued).await {
            Ok((role, old)) => {
                self.forget(&old).await;
                tracing::info!(branch = %role.record.branch_id, role = %role.record.name, secret_ref = %role.record.secret_ref, "rotated a role password");
                Ok(RoleIssued { role, password })
            }
            Err(e) => {
                if let Some(s) = issued.filter(|_| !e.is_undetermined()) {
                    self.discard(vec![s]).await;
                }
                Err(e)
            }
        }
    }

    /// The reset's tries: the new record, and the reference it replaced.
    async fn reset_tries(
        &self,
        caller: &Caller,
        req: &ResetRolePassword,
        password: &Secret<String>,
        issued: &mut Option<(RoleKey, SecretRef)>,
    ) -> Result<(Versioned<RoleRec>, SecretRef), ServiceError> {
        for _ in 0..ATTEMPTS {
            let claim = match self
                .begin::<Versioned<RoleRec>, _>(
                    caller,
                    "ResetRolePassword",
                    &req.idempotency_key,
                    req,
                )
                .await?
            {
                Begin::Replay(first) => return Err(already_issued(&first.record.name)),
                Begin::Fresh(claim) => claim,
            };
            let scope = self
                .writable_scope(&req.namespace, &req.project_id, &req.branch_id)
                .await?;
            let current = self.role(scope.branch_id(), &req.name).await?;
            if current.record.system {
                return Err(system_role(&req.name));
            }
            let secret_ref = self
                .issue(
                    &scope.project.record.id,
                    &current.record.key(),
                    password,
                    issued,
                )
                .await?;
            let old = current.record.secret_ref.clone();
            let mut rec = current.record;
            rec.secret_ref = secret_ref.clone();
            rec.updated_at_ms = self.now_ms();
            let mut batch = Batch::new();
            let at = batch.put(&rec, Some(current.version))?;
            scope.check(&mut batch)?;
            let mutation = Mutation::new(batch, move |out| Versioned {
                record: rec.clone(),
                version: out[at].unwrap_or_default(),
            });
            if let Applied::Done(first) = self
                .apply("ResetRolePassword", claim.as_ref(), mutation)
                .await?
            {
                return mine(first, &secret_ref).map(|role| (role, old));
            }
        }
        Err(Self::contended())
    }

    /// `DeleteRole`: removes the role's record, then its secret. A role
    /// that owns a database of the branch is refused, as Postgres refuses
    /// `DROP ROLE` for it.
    ///
    /// # Errors
    ///
    /// `not_found`; `failed_precondition` (it owns a database, it is
    /// managed by Loams, or the project or branch is deleting); `aborted`.
    pub async fn delete_role(&self, caller: &Caller, req: DeleteRole) -> Result<(), ServiceError> {
        for _ in 0..ATTEMPTS {
            let claim = match self
                .begin::<(), _>(caller, "DeleteRole", &req.idempotency_key, &req)
                .await?
            {
                Begin::Replay(()) => return Ok(()),
                Begin::Fresh(claim) => claim,
            };
            let scope = self
                .writable_scope(&req.namespace, &req.project_id, &req.branch_id)
                .await?;
            let current = self.role(scope.branch_id(), &req.name).await?;
            if current.record.system {
                return Err(system_role(&req.name));
            }
            let databases = self
                .all::<DatabaseRec>(&BranchScope {
                    branch_id: scope.branch_id().into(),
                })
                .await?;
            if let Some(owned) = databases.iter().find(|d| d.record.owner == req.name) {
                return Err(ServiceError::failed_precondition(format!(
                    "role {} owns database {}: delete the database first",
                    req.name, owned.record.name
                )));
            }
            let mut batch = Batch::new();
            // A database created for the role meanwhile rewrites the role
            // record, so this delete conflicts with it.
            batch.delete::<RoleRec>(&current.record.key(), current.version)?;
            scope.check(&mut batch)?;
            let mutation = Mutation::new(batch, |_| ());
            if let Applied::Done(()) = self.apply("DeleteRole", claim.as_ref(), mutation).await? {
                self.forget(&current.record.secret_ref).await;
                return Ok(());
            }
        }
        Err(Self::contended())
    }

    /// What a branch created from `parent` (as `child_id`) copies: every
    /// role, each with a copy of its secret under a new reference (pushed
    /// to `copies` as it is stored, so the caller can discard them), and
    /// the databases whose owner it copied. `None`: a parent role moved
    /// while it was copied; the caller tries again.
    ///
    /// # Errors
    ///
    /// The credential store's, or `internal` for a role whose secret is
    /// missing (its password must be reset).
    pub(crate) async fn inherit(
        &self,
        parent: &BranchRec,
        child_id: &str,
        now_ms: u64,
        copies: &mut Vec<(RoleKey, SecretRef)>,
    ) -> Result<Option<Inherited>, ServiceError> {
        let scope = BranchScope {
            branch_id: parent.id.clone(),
        };
        // Databases first: an owner cannot be deleted while it owns one,
        // so every owner read here is among the roles read next, unless the
        // database went too (the filter below drops it then).
        let databases = self.all::<DatabaseRec>(&scope).await?;
        let parents = self.all::<RoleRec>(&scope).await?;
        let project: ProjectId = parent
            .project_id
            .parse()
            .map_err(|e: crate::ids::IdError| ServiceError::invalid("project_id", e.to_string()))?;
        let mut roles = Vec::with_capacity(parents.len());
        for p in &parents {
            let secret = match self.secrets.get(&p.record.secret_ref).await {
                Ok(s) => s,
                Err(SecretError::NotFound(_)) => {
                    // A reset or delete of the parent's role moved it on.
                    match self.store.get::<RoleRec>(&p.record.key()).await? {
                        Some(now) if now.record.secret_ref == p.record.secret_ref => {
                            tracing::error!(role = %p.record.name, branch = %parent.id, secret_ref = %p.record.secret_ref, "a role's secret is missing from the credential store");
                            return Err(ServiceError::new(
                                Reason::Internal,
                                format!(
                                    "the password of role {} is lost: reset it, then branch again",
                                    p.record.name
                                ),
                            ));
                        }
                        _ => return Ok(None),
                    }
                }
                Err(e) => return Err(ServiceError::secret(&e)),
            };
            let r = SecretRef::new_role(&project);
            self.secrets
                .put(&r, secret)
                .await
                .map_err(|e| ServiceError::secret(&e))?;
            let rec = RoleRec {
                branch_id: child_id.into(),
                secret_ref: r.clone(),
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
                ..p.record.clone()
            };
            copies.push((rec.key(), r));
            roles.push(rec);
        }
        let names: BTreeSet<&str> = roles.iter().map(|r| r.name.as_str()).collect();
        let databases = databases
            .into_iter()
            .filter(|d| names.contains(d.record.owner.as_str()))
            .map(|d| DatabaseRec {
                branch_id: child_id.into(),
                created_at_ms: now_ms,
                ..d.record
            })
            .collect();
        Ok(Some(Inherited {
            parents,
            roles,
            databases,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::check_role_name;

    #[test]
    fn role_names() {
        for ok in [
            "app",
            "App",
            "a/b",
            "my role",
            "ro",
            "pgx",
            "toke",
            &"a".repeat(63),
        ] {
            assert!(check_role_name(ok).is_ok(), "{ok:?}");
        }
        for bad in [
            "",
            "PG_X",
            "Public",
            "loams_ro",
            "tok_1",
            "cloud_admin",
            "a\0",
            &"a".repeat(64),
        ] {
            assert!(check_role_name(bad).is_err(), "{bad:?}");
        }
    }
}
