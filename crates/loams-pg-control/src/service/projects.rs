//! `CreateProject`, `GetProject`, `ListProjects`, `UpdateProject` and
//! `DeleteProject` (§46 §4; PG2 Task 5).
//!
//! A create writes, in one batch: the name index (a taken name is
//! `already_exists`), the project and its default branch `main`, both
//! `creating`, their `Pending` operation and the idempotency entry. Task 7's
//! reconciler attaches the tenant and creates `main`'s timeline.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::operations::{OperationKind, OperationRec, add_operation, pending};
use super::{
    Applied, Begin, Caller, Mutation, NeonRead, PgService, Reason, ServiceError, check_namespace,
    list_error, page,
};
use crate::ids::{BranchId, ProjectId, tenant_id, timeline_id};
use crate::model::{
    BranchNameRec, BranchRec, BranchState, ProjectKey, ProjectNameKey, ProjectNameRec,
    ProjectPrefix, ProjectRec, ProjectState, WalService,
};
use crate::names::validate_name;
use crate::store::{Batch, PgControlStore, Versioned};

/// The name of the branch every project starts with (§46 §3).
pub const DEFAULT_BRANCH: &str = "main";

/// `CreateProjectRequest`. Zero or empty fields take the service's defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateProject {
    pub namespace: String,
    pub name: String,
    pub pg_version: u32,
    pub region: String,
    pub history_retention: Option<Duration>,
    pub wal_pool: String,
    pub idempotency_key: String,
}

/// `CreateProjectResponse`: the operation, and the project as created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectCreated {
    pub operation: OperationRec,
    pub project: Versioned<ProjectRec>,
}

/// `UpdateProjectRequest`: each `Some` field is in the update mask; none is
/// `invalid_argument` (R1.7).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateProject {
    pub namespace: String,
    pub project_id: String,
    pub name: Option<String>,
    pub history_retention: Option<Duration>,
    pub expected_version: Option<u64>,
    pub idempotency_key: String,
}

/// `DeleteProjectRequest`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteProject {
    pub namespace: String,
    pub project_id: String,
    pub expected_version: Option<u64>,
    pub idempotency_key: String,
}

fn check_name(field: &str, name: &str) -> Result<(), ServiceError> {
    validate_name(name).map_err(|e| ServiceError::invalid(field, e.to_string()))
}

/// `expected_version` against the record's.
pub(crate) fn check_version(expected: Option<u64>, current: u64) -> Result<(), ServiceError> {
    match expected {
        Some(v) if v != current => Err(ServiceError::new(
            Reason::Aborted,
            format!("the resource is at version {current}, not {v}"),
        )),
        _ => Ok(()),
    }
}

impl<N: NeonRead> PgService<N> {
    /// The project `project_id` of `namespace`.
    pub(crate) async fn project(
        &self,
        namespace: &str,
        project_id: &str,
    ) -> Result<Versioned<ProjectRec>, ServiceError> {
        check_namespace(namespace)?;
        project_id
            .parse::<ProjectId>()
            .map_err(|e| ServiceError::invalid("project_id", e.to_string()))?;
        let key = ProjectKey {
            namespace: namespace.into(),
            id: project_id.into(),
        };
        self.store
            .get::<ProjectRec>(&key)
            .await?
            .ok_or_else(|| ServiceError::project_not_found(project_id))
    }

    /// A retention for a request: positive, clamped to the limit.
    fn retention(&self, asked: Option<Duration>) -> Result<u64, ServiceError> {
        let r = asked.unwrap_or(self.config.default_history_retention);
        if r.is_zero() {
            return Err(ServiceError::invalid(
                "history_retention",
                "history_retention is above zero",
            ));
        }
        Ok(r.min(self.config.max_history_retention).as_secs().max(1))
    }

    /// `CreateProject`: the project and `main`, both `creating`, and the
    /// operation that finishes them.
    ///
    /// # Errors
    ///
    /// `invalid_argument`; `already_exists` for a taken name (another key);
    /// `unavailable` when the store is down or the outcome is unknown
    /// (retry with the same key).
    pub async fn create_project(
        &self,
        caller: &Caller,
        req: CreateProject,
    ) -> Result<ProjectCreated, ServiceError> {
        check_namespace(&req.namespace)?;
        check_name("name", &req.name)?;
        let pg_version = match req.pg_version {
            0 => self.config.default_pg_version,
            v if self.config.pg_versions.contains(&v) => v,
            v => {
                return Err(ServiceError::invalid(
                    "pg_version",
                    format!("Postgres {v} is not offered: {:?}", self.config.pg_versions),
                ));
            }
        };
        let retention_s = self.retention(req.history_retention)?;
        let region = match req.region.as_str() {
            "" => self.config.default_region.clone(),
            r => r.to_string(),
        };
        let pool = match req.wal_pool.as_str() {
            "" => self.config.default_wal_pool.clone(),
            p => p.to_string(),
        };
        let claim = match self
            .begin(caller, "CreateProject", &req.idempotency_key, &req)
            .await?
        {
            Begin::Replay(first) => return Ok(first),
            Begin::Fresh(claim) => claim,
        };
        for _ in 0..super::ATTEMPTS {
            let now = self.now_ms();
            let project_id = ProjectId::new();
            let main_id = BranchId::new();
            let project = ProjectRec {
                namespace: req.namespace.clone(),
                id: project_id.to_string(),
                name: req.name.clone(),
                tenant_id: tenant_id(&project_id),
                pg_version,
                wal: WalService::LoamsWal { pool: pool.clone() },
                history_retention_s: retention_s,
                region: region.clone(),
                default_branch_id: Some(main_id.to_string()),
                settings: Default::default(),
                state: ProjectState::Creating,
                created_at_ms: now,
                updated_at_ms: now,
            };
            let main = BranchRec {
                project_id: project.id.clone(),
                id: main_id.to_string(),
                name: DEFAULT_BRANCH.into(),
                timeline_id: timeline_id(&main_id),
                parent_id: None,
                ancestor_lsn: None,
                expires_at_ms: None,
                protected: false,
                stripe_size: None,
                shards: Vec::new(),
                state: BranchState::Creating,
                created_at_ms: now,
                parent_time_ms: None,
                updated_at_ms: now,
            };
            let operation = pending(
                OperationKind::ProjectCreate,
                &req.namespace,
                &project.id,
                None,
                now,
            );
            let mut batch = Batch::new();
            let name_at = batch.put(
                &ProjectNameRec {
                    namespace: req.namespace.clone(),
                    name: req.name.clone(),
                    project_id: project.id.clone(),
                },
                None,
            )?;
            let project_at = batch.put(&project, None)?;
            batch.put(
                &BranchNameRec {
                    project_id: project.id.clone(),
                    name: main.name.clone(),
                    branch_id: main.id.clone(),
                },
                None,
            )?;
            batch.put(
                &super::branches::new_guard(&main.project_id, &main.id),
                None,
            )?;
            batch.put(&main, None)?;
            add_operation(&mut batch, &operation)?;
            let taken = ServiceError::new(
                Reason::AlreadyExists,
                format!("a project named {} exists in this namespace", req.name),
            );
            let mutation = Mutation::new(batch, move |out| ProjectCreated {
                operation: operation.clone(),
                project: Versioned {
                    record: project.clone(),
                    version: out[project_at].unwrap_or_default(),
                },
            })
            .conflict_means(name_at, taken);
            if let Applied::Done(created) = self
                .apply("CreateProject", claim.as_ref(), mutation)
                .await?
            {
                return Ok(created);
            }
        }
        Err(Self::contended())
    }

    /// `GetProject`.
    ///
    /// # Errors
    ///
    /// `project_not_found` when the namespace has no such project.
    pub async fn get_project(
        &self,
        namespace: &str,
        project_id: &str,
    ) -> Result<Versioned<ProjectRec>, ServiceError> {
        self.project(namespace, project_id).await
    }

    /// `ListProjects`: one page, in id order, and the next page's token
    /// (empty on the last page).
    ///
    /// # Errors
    ///
    /// `invalid_argument` for a negative size or a foreign token.
    pub async fn list_projects(
        &self,
        namespace: &str,
        page_size: i32,
        page_token: &str,
    ) -> Result<(Vec<Versioned<ProjectRec>>, String), ServiceError> {
        check_namespace(namespace)?;
        let prefix = ProjectPrefix {
            namespace: namespace.into(),
        };
        let (projects, next) = self
            .store
            .list::<ProjectRec>(&prefix, page(page_size, page_token)?)
            .await
            .map_err(list_error)?;
        Ok((projects, next.unwrap_or_default()))
    }

    /// `UpdateProject`: `name` (the name index moves with it) and
    /// `history_retention` (clamped).
    ///
    /// # Errors
    ///
    /// `invalid_argument` (an empty mask too); `already_exists` for a taken
    /// name; `aborted` at another `expected_version`; `failed_precondition`
    /// while the project is deleting.
    pub async fn update_project(
        &self,
        caller: &Caller,
        req: UpdateProject,
    ) -> Result<Versioned<ProjectRec>, ServiceError> {
        if req.name.is_none() && req.history_retention.is_none() {
            return Err(ServiceError::invalid(
                "update_mask",
                "the update mask names at least one field: name, history_retention",
            ));
        }
        if let Some(name) = &req.name {
            check_name("project.name", name)?;
        }
        let retention_s = match req.history_retention {
            Some(r) => Some(self.retention(Some(r))?),
            None => None,
        };
        let claim = match self
            .begin(caller, "UpdateProject", &req.idempotency_key, &req)
            .await?
        {
            Begin::Replay(first) => return Ok(first),
            Begin::Fresh(claim) => claim,
        };
        for _ in 0..super::ATTEMPTS {
            let current = self.project(&req.namespace, &req.project_id).await?;
            check_version(req.expected_version, current.version)?;
            if current.record.state == ProjectState::Deleting {
                return Err(ServiceError::failed_precondition(
                    "the project is being deleted",
                ));
            }
            let mut project = current.record.clone();
            let mut batch = Batch::new();
            let mut renamed_at = None;
            if let Some(name) = req.name.as_ref().filter(|n| **n != project.name) {
                let old = ProjectNameKey {
                    namespace: project.namespace.clone(),
                    name: project.name.clone(),
                };
                if let Some(index) = self.store.get::<ProjectNameRec>(&old).await? {
                    batch.delete::<ProjectNameRec>(&old, index.version)?;
                }
                renamed_at = Some(batch.put(
                    &ProjectNameRec {
                        namespace: project.namespace.clone(),
                        name: name.clone(),
                        project_id: project.id.clone(),
                    },
                    None,
                )?);
                project.name = name.clone();
            }
            if let Some(r) = retention_s {
                project.history_retention_s = r;
            }
            project.updated_at_ms = self.now_ms();
            let project_at = batch.put(&project, Some(current.version))?;
            let mut mutation = Mutation::new(batch, move |out| Versioned {
                record: project.clone(),
                version: out[project_at].unwrap_or_default(),
            });
            if let (Some(at), Some(name)) = (renamed_at, &req.name) {
                mutation = mutation.conflict_means(
                    at,
                    ServiceError::new(
                        Reason::AlreadyExists,
                        format!("a project named {name} exists in this namespace"),
                    ),
                );
            }
            if let Applied::Done(updated) = self
                .apply("UpdateProject", claim.as_ref(), mutation)
                .await?
            {
                return Ok(updated);
            }
        }
        Err(Self::contended())
    }

    /// `DeleteProject`: marks the project `deleting` and returns the
    /// operation; the reconciler (Task 7) removes its endpoints, branches,
    /// tenant and records. A project with a protected branch needs `admin`.
    ///
    /// # Errors
    ///
    /// `project_not_found`; `branch_protected`; `aborted` at another
    /// `expected_version`; `failed_precondition` when already deleting.
    pub async fn delete_project(
        &self,
        caller: &Caller,
        req: DeleteProject,
    ) -> Result<OperationRec, ServiceError> {
        let claim = match self
            .begin(caller, "DeleteProject", &req.idempotency_key, &req)
            .await?
        {
            Begin::Replay(first) => return Ok(first),
            Begin::Fresh(claim) => claim,
        };
        for _ in 0..super::ATTEMPTS {
            let current = self.project(&req.namespace, &req.project_id).await?;
            check_version(req.expected_version, current.version)?;
            if current.record.state == ProjectState::Deleting {
                return Err(ServiceError::failed_precondition(
                    "the project is already being deleted",
                ));
            }
            if !caller.admin
                && let Some(protected) = self
                    .all_branches(&current.record.id)
                    .await?
                    .into_iter()
                    .find(|b| b.record.protected)
            {
                return Err(super::branches::protected(&protected.record.id));
            }
            let mut project = current.record;
            project.state = ProjectState::Deleting;
            project.updated_at_ms = self.now_ms();
            let operation = pending(
                OperationKind::ProjectDelete,
                &req.namespace,
                &project.id,
                None,
                self.now_ms(),
            );
            let mut batch = Batch::new();
            batch.put(&project, Some(current.version))?;
            add_operation(&mut batch, &operation)?;
            let mutation = Mutation::new(batch, move |_| operation.clone());
            if let Applied::Done(op) = self
                .apply("DeleteProject", claim.as_ref(), mutation)
                .await?
            {
                return Ok(op);
            }
        }
        Err(Self::contended())
    }
}
