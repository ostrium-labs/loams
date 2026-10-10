//! `CreateDatabase`, `ListDatabases` and `DeleteDatabase` (§46 §3; PG2
//! Task 6). A database is a record of a branch ([`DatabaseRec`] at
//! `D/<branch_id>/<db>`), applied through the compute spec; its owner is a
//! role of the same branch.
//!
//! **Owner and delete race.** A create rewrites its owner's [`RoleRec`]
//! unchanged, and `DeleteRole` deletes that record, so a database created
//! for a role while the role is deleted conflicts whichever commits first:
//! the create then finds no owner, or the delete finds the database.

use serde::{Deserialize, Serialize};

use super::{
    ATTEMPTS, Applied, Begin, Caller, Mutation, NeonRead, PgService, Reason, ServiceError,
    list_error, page,
};
use crate::model::{BranchScope, DatabaseKey, DatabaseRec, Record, RoleKey, RoleRec};
use crate::names::validate_database;
use crate::store::{Batch, PgControlStore, Versioned};

/// Databases every Postgres cluster already has (compared lower-cased).
const RESERVED_DATABASES: [&str; 3] = ["postgres", "template0", "template1"];

/// `CreateDatabaseRequest`. An empty `branch_id` takes the project's
/// default branch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateDatabase {
    pub namespace: String,
    pub project_id: String,
    pub branch_id: String,
    pub name: String,
    pub owner_role: String,
    pub idempotency_key: String,
}

/// `DeleteDatabaseRequest`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteDatabase {
    pub namespace: String,
    pub project_id: String,
    pub branch_id: String,
    pub name: String,
    pub idempotency_key: String,
}

fn check_database_name(name: &str) -> Result<(), ServiceError> {
    validate_database(name).map_err(|e| ServiceError::invalid("name", e.to_string()))?;
    if RESERVED_DATABASES.contains(&name.to_lowercase().as_str()) {
        return Err(ServiceError::invalid(
            "name",
            format!("database name {name:?} is reserved"),
        ));
    }
    Ok(())
}

fn database_exists(name: &str) -> ServiceError {
    ServiceError::new(
        Reason::AlreadyExists,
        format!("a database named {name} exists on this branch"),
    )
}

impl<N: NeonRead> PgService<N> {
    /// `CreateDatabase`: a database of the branch, owned by one of its
    /// roles.
    ///
    /// # Errors
    ///
    /// `invalid_argument` (a reserved or malformed name, no owner);
    /// `failed_precondition` when the owner is not a role of the branch,
    /// or the project or branch is deleting; `already_exists`; `not_found`
    /// for the branch.
    pub async fn create_database(
        &self,
        caller: &Caller,
        req: CreateDatabase,
    ) -> Result<Versioned<DatabaseRec>, ServiceError> {
        check_database_name(&req.name)?;
        if req.owner_role.is_empty() {
            return Err(ServiceError::invalid(
                "owner_role",
                "a database needs an owner role",
            ));
        }
        for _ in 0..ATTEMPTS {
            let claim = match self
                .begin(caller, "CreateDatabase", &req.idempotency_key, &req)
                .await?
            {
                Begin::Replay(first) => return Ok(first),
                Begin::Fresh(claim) => claim,
            };
            let scope = self
                .writable_scope(&req.namespace, &req.project_id, &req.branch_id)
                .await?;
            let owner = self
                .store
                .get::<RoleRec>(&RoleKey {
                    branch_id: scope.branch_id().into(),
                    name: req.owner_role.clone(),
                })
                .await?
                .ok_or_else(|| {
                    ServiceError::failed_precondition(format!(
                        "role {} does not exist on branch {}: create it first",
                        req.owner_role,
                        scope.branch_id()
                    ))
                })?;
            let key = DatabaseKey {
                branch_id: scope.branch_id().into(),
                name: req.name.clone(),
            };
            if self.store.get::<DatabaseRec>(&key).await?.is_some() {
                return Err(database_exists(&req.name));
            }
            let rec = DatabaseRec {
                project_id: scope.project.record.id.clone(),
                branch_id: key.branch_id,
                name: req.name.clone(),
                owner: req.owner_role.clone(),
                created_at_ms: self.now_ms(),
            };
            let mut batch = Batch::new();
            let at = batch.put(&rec, None)?;
            // The owner, unchanged: a delete of it conflicts with this.
            batch.put(&owner.record, Some(owner.version))?;
            scope.check(&mut batch)?;
            let mutation = Mutation::new(batch, move |out| Versioned {
                record: rec.clone(),
                version: out[at].unwrap_or_default(),
            })
            .conflict_means(at, database_exists(&req.name));
            if let Applied::Done(created) = self
                .apply("CreateDatabase", claim.as_ref(), mutation)
                .await?
            {
                return Ok(created);
            }
        }
        Err(Self::contended())
    }

    /// `ListDatabases`: one page of a branch's databases in name order, and
    /// the next page's token (empty on the last page).
    ///
    /// # Errors
    ///
    /// `project_not_found`; `not_found` for the branch; `invalid_argument`
    /// for a negative size or a foreign token.
    pub async fn list_databases(
        &self,
        namespace: &str,
        project_id: &str,
        branch_id: &str,
        page_size: i32,
        page_token: &str,
    ) -> Result<(Vec<Versioned<DatabaseRec>>, String), ServiceError> {
        let scope = self.scope(namespace, project_id, branch_id).await?;
        let prefix = BranchScope {
            branch_id: scope.branch_id().into(),
        };
        let (databases, next) = self
            .store
            .list::<DatabaseRec>(&prefix, page(page_size, page_token)?)
            .await
            .map_err(list_error)?;
        Ok((databases, next.unwrap_or_default()))
    }

    /// `DeleteDatabase`.
    ///
    /// # Errors
    ///
    /// `not_found`; `failed_precondition` while the project or branch is
    /// deleting; `aborted`.
    pub async fn delete_database(
        &self,
        caller: &Caller,
        req: DeleteDatabase,
    ) -> Result<(), ServiceError> {
        for _ in 0..ATTEMPTS {
            let claim = match self
                .begin::<(), _>(caller, "DeleteDatabase", &req.idempotency_key, &req)
                .await?
            {
                Begin::Replay(()) => return Ok(()),
                Begin::Fresh(claim) => claim,
            };
            let scope = self
                .writable_scope(&req.namespace, &req.project_id, &req.branch_id)
                .await?;
            let key = DatabaseKey {
                branch_id: scope.branch_id().into(),
                name: req.name.clone(),
            };
            let current = self
                .store
                .get::<DatabaseRec>(&key)
                .await?
                .ok_or_else(|| ServiceError::not_found("database", &req.name))?;
            let mut batch = Batch::new();
            batch.delete::<DatabaseRec>(&current.record.key(), current.version)?;
            scope.check(&mut batch)?;
            let mutation = Mutation::new(batch, |_| ());
            if let Applied::Done(()) = self
                .apply("DeleteDatabase", claim.as_ref(), mutation)
                .await?
            {
                return Ok(());
            }
        }
        Err(Self::contended())
    }
}

#[cfg(test)]
mod tests {
    use super::check_database_name;

    #[test]
    fn database_names() {
        for ok in ["app", "my.db", "a__b", "Postgres2", &"d".repeat(63)] {
            assert!(check_database_name(ok).is_ok(), "{ok:?}");
        }
        for bad in ["", "postgres", "Template1", "a\0", &"d".repeat(64)] {
            assert!(check_database_name(bad).is_err(), "{bad:?}");
        }
    }
}
