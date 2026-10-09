//! `CreateBranch`, `GetBranch`, `ListBranches`, `UpdateBranch`,
//! `DeleteBranch` and `SetDefaultBranch` (§46 §4, §10; PG2 Task 5).
//!
//! - **A branch point** (R5.9) is the parent's head, an LSN, or a time the
//!   pageserver resolves to an LSN (and leases). The head of a `ready`
//!   parent is recorded as its `loams-wal` `commit_lsn`. A point needs a
//!   `ready` parent and must lie between the parent's `ancestor_lsn` and its
//!   `commit_lsn`; a time older than the history retention, a time before
//!   the oldest WAL, or an LSN below the parent's `min_readable_lsn` is
//!   `lsn_out_of_retention`.
//! - **Races.** A create writes its parent's [`BranchGuardRec`]
//!   (`children + 1`), and a delete writes the branch's own (`deleting`), so
//!   a child created while its parent is deleted conflicts either way
//!   (snapshot isolation sees two writes of one key, never a read and a
//!   write). The parent's record and version are untouched. `SetDefaultBranch` writes the project and
//!   checks the branch; a delete writes the branch and checks the project.
//! - **Protection** (§46 §10): deleting a protected branch, or lifting its
//!   protection, needs `admin` (an agent's approval flow is Task 9's).

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::operations::{OperationKind, OperationRec, add_operation, pending};
use super::projects::check_version;
use super::{
    Applied, Begin, Caller, Mutation, NeonRead, PgService, Reason, ServiceError, list_error, page,
    seconds,
};
use crate::ids::{BranchId, timeline_id};
use crate::model::{
    BranchGuardKey, BranchGuardRec, BranchKey, BranchNameKey, BranchNameRec, BranchPrefix,
    BranchRec, BranchState, ProjectKey, ProjectRec, ProjectState, Record,
};
use crate::names::validate_name;
use crate::neon::{Lsn, LsnAtTime, TenantId, TimelineId, WalHeads};
use crate::store::{Batch, MAX_PAGE_SIZE, Page, PgControlStore, Versioned};

/// Where in its parent's history a branch starts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BranchPoint {
    /// The parent's head when the reconciler creates the timeline.
    #[default]
    Head,
    /// An LSN in Postgres's `X/Y` form.
    Lsn(String),
    /// A time, in ms since the Unix epoch.
    Time(u64),
}

/// `CreateBranchRequest`. An empty `parent_id` takes the project's default
/// branch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateBranch {
    pub namespace: String,
    pub project_id: String,
    pub name: String,
    pub parent_id: String,
    pub point: BranchPoint,
    pub protected: bool,
    pub ttl: Option<Duration>,
    pub idempotency_key: String,
}

/// A branch as the API answers it (`loams.postgres.v1.Branch`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchView {
    pub branch: Versioned<BranchRec>,
    /// Whether it is the project's default branch.
    pub is_default: bool,
    /// From `loams-wal`, on `GetBranch` of a `ready` branch, when it
    /// answered.
    pub wal: Option<WalHeads>,
    /// From the pageserver, likewise.
    pub logical_size_bytes: Option<u64>,
}

/// `CreateBranchResponse`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchCreated {
    pub operation: OperationRec,
    pub branch: BranchView,
}

/// `UpdateBranchRequest`: each `Some` field is in the mask; none is
/// `invalid_argument`. `expire_at_ms: Some(None)` clears the expiry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateBranch {
    pub namespace: String,
    pub project_id: String,
    pub branch_id: String,
    pub name: Option<String>,
    pub expire_at_ms: Option<Option<u64>>,
    pub protected: Option<bool>,
    pub expected_version: Option<u64>,
    pub idempotency_key: String,
}

/// `DeleteBranchRequest`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteBranch {
    pub namespace: String,
    pub project_id: String,
    pub branch_id: String,
    pub expected_version: Option<u64>,
    pub idempotency_key: String,
}

/// `SetDefaultBranchRequest`; `expected_version` is the project's.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetDefaultBranch {
    pub namespace: String,
    pub project_id: String,
    pub branch_id: String,
    pub expected_version: Option<u64>,
    pub idempotency_key: String,
}

/// `branch_protected` for `branch_id`.
pub(crate) fn protected(branch_id: &str) -> ServiceError {
    ServiceError::new(
        Reason::BranchProtected,
        format!("branch {branch_id} is protected: this needs the admin relation"),
    )
    .with("branch", branch_id)
}

fn out_of_retention(oldest: Option<Lsn>, retention_s: u64) -> ServiceError {
    let e = ServiceError::new(
        Reason::LsnOutOfRetention,
        "the branch point is older than the history the project keeps",
    )
    .with("history_retention", seconds(retention_s));
    match oldest {
        Some(lsn) => e.with("oldest_lsn", lsn.to_string()),
        None => e,
    }
}

/// A new branch's guard: no children, not deleting.
pub(crate) fn new_guard(project_id: &str, branch_id: &str) -> BranchGuardRec {
    BranchGuardRec {
        project_id: project_id.into(),
        branch_id: branch_id.into(),
        children: 0,
        deleting: false,
    }
}

fn live(state: BranchState) -> bool {
    matches!(state, BranchState::Creating | BranchState::Ready)
}

fn view(branch: Versioned<BranchRec>, project: &ProjectRec) -> BranchView {
    BranchView {
        is_default: project.default_branch_id.as_deref() == Some(branch.record.id.as_str()),
        branch,
        wal: None,
        logical_size_bytes: None,
    }
}

impl<N: NeonRead> PgService<N> {
    /// The branch `branch_id` of `project_id`.
    async fn branch(
        &self,
        project_id: &str,
        branch_id: &str,
        field: &str,
    ) -> Result<Versioned<BranchRec>, ServiceError> {
        branch_id
            .parse::<BranchId>()
            .map_err(|e| ServiceError::invalid(field, e.to_string()))?;
        let key = BranchKey {
            project_id: project_id.into(),
            id: branch_id.into(),
        };
        self.store
            .get::<BranchRec>(&key)
            .await?
            .ok_or_else(|| ServiceError::not_found("branch", branch_id))
    }

    /// The guard of a branch, and its version (`None`: absent, which reads
    /// as no children and not deleting).
    async fn guard(
        &self,
        project_id: &str,
        branch_id: &str,
    ) -> Result<(BranchGuardRec, Option<u64>), ServiceError> {
        let key = BranchGuardKey {
            project_id: project_id.into(),
            branch_id: branch_id.into(),
        };
        Ok(match self.store.get::<BranchGuardRec>(&key).await? {
            Some(v) => (v.record, Some(v.version)),
            None => (new_guard(project_id, branch_id), None),
        })
    }

    /// Every branch of a project.
    pub(crate) async fn all_branches(
        &self,
        project_id: &str,
    ) -> Result<Vec<Versioned<BranchRec>>, ServiceError> {
        let prefix = BranchPrefix {
            project_id: project_id.into(),
        };
        let mut all = Vec::new();
        let mut at = Page::first(MAX_PAGE_SIZE);
        loop {
            let (branches, next) = self.store.list::<BranchRec>(&prefix, at).await?;
            all.extend(branches);
            match next {
                Some(token) => at = Page::after(MAX_PAGE_SIZE, token),
                None => return Ok(all),
            }
        }
    }

    /// A `ready` parent, or `failed_precondition`: a branch point needs
    /// its timeline.
    fn need_ready(parent: &BranchRec) -> Result<(), ServiceError> {
        if parent.state == BranchState::Ready {
            return Ok(());
        }
        Err(ServiceError::failed_precondition(format!(
            "branch {} is not ready: a branch point needs its timeline",
            parent.id
        )))
    }

    /// The parent's WAL heads from `loams-wal`.
    async fn parent_heads(
        &self,
        project: &ProjectRec,
        parent: &BranchRec,
    ) -> Result<WalHeads, ServiceError> {
        self.neon
            .wal_heads(TenantId(project.tenant_id), TimelineId(parent.timeline_id))
            .await
            .map_err(|e| ServiceError::neon(&e))
    }

    /// The parent LSN a point names (R5.9). The head of a `ready` parent is
    /// its `commit_lsn` on `loams-wal`, recorded so the branch starts where
    /// the parent's acknowledged commits end; the head of a parent still
    /// creating is `None`, and the reconciler branches at its head. A
    /// requested point must lie between the parent's start
    /// (`ancestor_lsn`) and `commit_lsn`, and not below what the
    /// pageserver keeps (`min_readable_lsn`).
    async fn resolve(
        &self,
        point: &BranchPoint,
        project: &ProjectRec,
        parent: &BranchRec,
    ) -> Result<Option<u64>, ServiceError> {
        let retention_s = project.history_retention_s;
        let (lsn, field) = match point {
            BranchPoint::Head => {
                if parent.state != BranchState::Ready {
                    return Ok(None);
                }
                let heads = self.parent_heads(project, parent).await?;
                return Ok(Some(heads.commit_lsn.0));
            }
            BranchPoint::Lsn(text) => {
                let lsn = text
                    .parse::<Lsn>()
                    .map_err(|e| ServiceError::invalid("lsn", e.to_string()))?;
                Self::need_ready(parent)?;
                (Some(lsn), "lsn")
            }
            BranchPoint::Time(at_ms) => {
                let window_ms = retention_s.saturating_mul(1000);
                if at_ms.saturating_add(window_ms) < self.now_ms() {
                    return Err(out_of_retention(None, retention_s));
                }
                Self::need_ready(parent)?;
                let at = self
                    .neon
                    .lsn_by_timestamp(
                        TenantId(project.tenant_id),
                        TimelineId(parent.timeline_id),
                        *at_ms,
                    )
                    .await
                    .map_err(|e| ServiceError::neon(&e))?;
                match at {
                    LsnAtTime::Present(lsn) => (Some(lsn), "time"),
                    // After the last commit the pageserver has: the head.
                    LsnAtTime::Future(_) => (None, "time"),
                    LsnAtTime::Past => return Err(out_of_retention(None, retention_s)),
                    LsnAtTime::NoData => {
                        return Err(ServiceError::failed_precondition(
                            "the parent has no commit to resolve a time by; branch at an LSN",
                        ));
                    }
                }
            }
        };
        let heads = self.parent_heads(project, parent).await?;
        let Some(lsn) = lsn else {
            return Ok(Some(heads.commit_lsn.0));
        };
        if lsn > heads.commit_lsn {
            return Err(ServiceError::invalid(
                field,
                format!("{lsn} is past the parent's commit_lsn {}", heads.commit_lsn),
            ));
        }
        if let Some(start) = parent.ancestor_lsn
            && lsn.0 < start
        {
            return Err(ServiceError::invalid(
                field,
                format!(
                    "{lsn} is before branch {} starts ({}): branch from its parent instead",
                    parent.id,
                    Lsn(start)
                ),
            )
            .with("oldest_lsn", Lsn(start).to_string()));
        }
        let timeline = self
            .neon
            .timeline(TenantId(project.tenant_id), TimelineId(parent.timeline_id))
            .await
            .map_err(|e| ServiceError::neon(&e))?;
        if lsn < timeline.min_readable_lsn {
            return Err(out_of_retention(
                Some(timeline.min_readable_lsn),
                retention_s,
            ));
        }
        Ok(Some(lsn.0))
    }

    /// `CreateBranch`: the branch in `creating`, and its operation.
    ///
    /// # Errors
    ///
    /// `project_not_found`; `invalid_argument`; `not_found` for the parent;
    /// `already_exists` for a taken name (another key);
    /// `lsn_out_of_retention`; `failed_precondition` when the project or
    /// the parent is deleting, or a point's parent is not ready;
    /// `storage_unavailable` when the pageserver does not answer.
    pub async fn create_branch(
        &self,
        caller: &Caller,
        req: CreateBranch,
    ) -> Result<BranchCreated, ServiceError> {
        validate_name(&req.name).map_err(|e| ServiceError::invalid("name", e.to_string()))?;
        if req.ttl.is_some_and(|t| t.is_zero()) {
            return Err(ServiceError::invalid("ttl", "a ttl is above zero"));
        }
        let claim = match self
            .begin(caller, "CreateBranch", &req.idempotency_key, &req)
            .await?
        {
            Begin::Replay(first) => return Ok(first),
            Begin::Fresh(claim) => claim,
        };
        for _ in 0..super::ATTEMPTS {
            let project = self.project(&req.namespace, &req.project_id).await?;
            if project.record.state != ProjectState::Creating
                && project.record.state != ProjectState::Ready
            {
                return Err(ServiceError::failed_precondition(
                    "the project is being deleted or has failed",
                ));
            }
            let parent_id = match req.parent_id.as_str() {
                "" => project.record.default_branch_id.clone().ok_or_else(|| {
                    ServiceError::failed_precondition("the project has no default branch")
                })?,
                id => id.to_string(),
            };
            let parent = self
                .branch(&project.record.id, &parent_id, "parent_id")
                .await?;
            let (mut parent_guard, guard_version) =
                self.guard(&project.record.id, &parent_id).await?;
            if !live(parent.record.state) || parent_guard.deleting {
                return Err(ServiceError::failed_precondition(format!(
                    "branch {parent_id} is being deleted or has failed"
                )));
            }
            parent_guard.children = parent_guard.children.saturating_add(1);
            let ancestor_lsn = self
                .resolve(&req.point, &project.record, &parent.record)
                .await?;
            let now = self.now_ms();
            let id = BranchId::new();
            let ttl_ms = req
                .ttl
                .map(|t| u64::try_from(t.as_millis()).unwrap_or(u64::MAX));
            let branch = BranchRec {
                project_id: project.record.id.clone(),
                id: id.to_string(),
                name: req.name.clone(),
                timeline_id: timeline_id(&id),
                parent_id: Some(parent_id.clone()),
                ancestor_lsn,
                expires_at_ms: ttl_ms.map(|t| now.saturating_add(t)),
                protected: req.protected,
                stripe_size: None,
                shards: Vec::new(),
                state: BranchState::Creating,
                created_at_ms: now,
                parent_time_ms: match req.point {
                    BranchPoint::Time(at_ms) => Some(at_ms),
                    _ => None,
                },
                updated_at_ms: now,
            };
            let operation = pending(
                OperationKind::BranchCreate,
                &req.namespace,
                &branch.project_id,
                Some(&branch.id),
                now,
            );
            let mut batch = Batch::new();
            let name_at = batch.put(
                &BranchNameRec {
                    project_id: branch.project_id.clone(),
                    name: branch.name.clone(),
                    branch_id: branch.id.clone(),
                },
                None,
            )?;
            let branch_at = batch.put(&branch, None)?;
            batch.put(&new_guard(&branch.project_id, &branch.id), None)?;
            // The parent's guard: a delete of the parent conflicts with it.
            batch.put(&parent_guard, guard_version)?;
            batch.check::<ProjectRec>(
                &ProjectKey {
                    namespace: req.namespace.clone(),
                    id: project.record.id.clone(),
                },
                Some(project.version),
            )?;
            add_operation(&mut batch, &operation)?;
            let taken = ServiceError::new(
                Reason::AlreadyExists,
                format!("a branch named {} exists in this project", req.name),
            );
            let mutation = Mutation::new(batch, move |out| BranchCreated {
                operation: operation.clone(),
                branch: BranchView {
                    branch: Versioned {
                        record: branch.clone(),
                        version: out[branch_at].unwrap_or_default(),
                    },
                    is_default: false,
                    wal: None,
                    logical_size_bytes: None,
                },
            })
            .conflict_means(name_at, taken);
            if let Applied::Done(created) =
                self.apply("CreateBranch", claim.as_ref(), mutation).await?
            {
                return Ok(created);
            }
        }
        Err(Self::contended())
    }

    /// `GetBranch`: the record and, for a `ready` branch, its WAL heads
    /// (from `loams-wal`) and logical size (from the pageserver). A
    /// component that does not answer leaves its part out, and is logged:
    /// the record still answers.
    ///
    /// # Errors
    ///
    /// `project_not_found`; `not_found`; `invalid_argument`.
    pub async fn get_branch(
        &self,
        namespace: &str,
        project_id: &str,
        branch_id: &str,
    ) -> Result<BranchView, ServiceError> {
        let project = self.project(namespace, project_id).await?;
        let branch = self.branch(project_id, branch_id, "branch_id").await?;
        let mut out = view(branch, &project.record);
        if out.branch.record.state == BranchState::Ready {
            let (t, tl) = (
                TenantId(project.record.tenant_id),
                TimelineId(out.branch.record.timeline_id),
            );
            match self.neon.wal_heads(t, tl).await {
                Ok(heads) => out.wal = Some(heads),
                Err(e) => tracing::warn!(branch = branch_id, error = %e, "no WAL heads"),
            }
            match self.neon.timeline(t, tl).await {
                Ok(timeline) => out.logical_size_bytes = Some(timeline.logical_size_bytes),
                Err(e) => tracing::warn!(branch = branch_id, error = %e, "no logical size"),
            }
        }
        Ok(out)
    }

    /// `ListBranches`: one page in id order (records only: no WAL heads),
    /// and the next page's token (empty on the last page).
    ///
    /// # Errors
    ///
    /// `project_not_found`; `invalid_argument` for a negative size or a
    /// foreign token.
    pub async fn list_branches(
        &self,
        namespace: &str,
        project_id: &str,
        page_size: i32,
        page_token: &str,
    ) -> Result<(Vec<BranchView>, String), ServiceError> {
        let project = self.project(namespace, project_id).await?;
        let prefix = BranchPrefix {
            project_id: project_id.into(),
        };
        let (branches, next) = self
            .store
            .list::<BranchRec>(&prefix, page(page_size, page_token)?)
            .await
            .map_err(list_error)?;
        let views = branches
            .into_iter()
            .map(|b| view(b, &project.record))
            .collect();
        Ok((views, next.unwrap_or_default()))
    }

    /// `UpdateBranch`: `name` (the name index moves with it), the expiry,
    /// and `protected` (lifting it needs `admin`).
    ///
    /// # Errors
    ///
    /// `invalid_argument` (an empty mask too); `permission_denied`;
    /// `already_exists`; `aborted`; `failed_precondition` while deleting.
    pub async fn update_branch(
        &self,
        caller: &Caller,
        req: UpdateBranch,
    ) -> Result<BranchView, ServiceError> {
        if req.name.is_none() && req.expire_at_ms.is_none() && req.protected.is_none() {
            return Err(ServiceError::invalid(
                "update_mask",
                "the update mask names at least one field: name, expire_time, protected",
            ));
        }
        if let Some(name) = &req.name {
            validate_name(name).map_err(|e| ServiceError::invalid("branch.name", e.to_string()))?;
        }
        let claim = match self
            .begin(caller, "UpdateBranch", &req.idempotency_key, &req)
            .await?
        {
            Begin::Replay(first) => return Ok(first),
            Begin::Fresh(claim) => claim,
        };
        for _ in 0..super::ATTEMPTS {
            let project = self.project(&req.namespace, &req.project_id).await?;
            let current = self
                .branch(&req.project_id, &req.branch_id, "branch.id")
                .await?;
            check_version(req.expected_version, current.version)?;
            if !live(current.record.state) {
                return Err(ServiceError::failed_precondition(
                    "the branch is being deleted or has failed",
                ));
            }
            if current.record.protected && req.protected == Some(false) && !caller.admin {
                return Err(ServiceError::new(
                    Reason::PermissionDenied,
                    "lifting a branch's protection needs the admin relation",
                ));
            }
            let mut branch = current.record.clone();
            let mut batch = Batch::new();
            let mut renamed_at = None;
            if let Some(name) = req.name.as_ref().filter(|n| **n != branch.name) {
                let old = BranchNameKey {
                    project_id: branch.project_id.clone(),
                    name: branch.name.clone(),
                };
                if let Some(index) = self.store.get::<BranchNameRec>(&old).await? {
                    batch.delete::<BranchNameRec>(&old, index.version)?;
                }
                renamed_at = Some(batch.put(
                    &BranchNameRec {
                        project_id: branch.project_id.clone(),
                        name: name.clone(),
                        branch_id: branch.id.clone(),
                    },
                    None,
                )?);
                branch.name = name.clone();
            }
            if let Some(expiry) = req.expire_at_ms {
                branch.expires_at_ms = expiry;
            }
            if let Some(p) = req.protected {
                branch.protected = p;
            }
            branch.updated_at_ms = self.now_ms();
            let branch_at = batch.put(&branch, Some(current.version))?;
            let project_rec = project.record;
            let mut mutation = Mutation::new(batch, move |out| {
                view(
                    Versioned {
                        record: branch.clone(),
                        version: out[branch_at].unwrap_or_default(),
                    },
                    &project_rec,
                )
            });
            if let (Some(at), Some(name)) = (renamed_at, &req.name) {
                mutation = mutation.conflict_means(
                    at,
                    ServiceError::new(
                        Reason::AlreadyExists,
                        format!("a branch named {name} exists in this project"),
                    ),
                );
            }
            if let Applied::Done(updated) =
                self.apply("UpdateBranch", claim.as_ref(), mutation).await?
            {
                return Ok(updated);
            }
        }
        Err(Self::contended())
    }

    /// `DeleteBranch`: marks the branch `deleting` and returns the
    /// operation; the reconciler (Task 7) deletes the timeline once the
    /// branch's endpoints are gone, then the record and its name index.
    ///
    /// # Errors
    ///
    /// `branch_has_children` (children still deleting count);
    /// `branch_protected` without `admin`; `failed_precondition` for the
    /// default branch or one already deleting (a `failed` one may be
    /// deleted); `aborted`; `not_found`.
    pub async fn delete_branch(
        &self,
        caller: &Caller,
        req: DeleteBranch,
    ) -> Result<OperationRec, ServiceError> {
        let claim = match self
            .begin(caller, "DeleteBranch", &req.idempotency_key, &req)
            .await?
        {
            Begin::Replay(first) => return Ok(first),
            Begin::Fresh(claim) => claim,
        };
        for _ in 0..super::ATTEMPTS {
            let project = self.project(&req.namespace, &req.project_id).await?;
            let current = self
                .branch(&req.project_id, &req.branch_id, "branch_id")
                .await?;
            check_version(req.expected_version, current.version)?;
            if project.record.default_branch_id.as_deref() == Some(req.branch_id.as_str()) {
                return Err(ServiceError::failed_precondition(
                    "the default branch cannot be deleted: make another branch the default first",
                ));
            }
            if current.record.protected && !caller.admin {
                return Err(protected(&req.branch_id));
            }
            // A failed branch may be deleted; a deleting one is already.
            if current.record.state == BranchState::Deleting {
                return Err(ServiceError::failed_precondition(
                    "the branch is already being deleted",
                ));
            }
            let (mut guard, guard_version) = self.guard(&req.project_id, &req.branch_id).await?;
            if guard.deleting {
                return Err(ServiceError::failed_precondition(
                    "the branch is already being deleted",
                ));
            }
            let children = guard.children;
            if children > 0 {
                return Err(ServiceError::new(
                    Reason::BranchHasChildren,
                    format!(
                        "branch {} has {children} child branches: delete them first",
                        req.branch_id
                    ),
                )
                .with("children", children.to_string()));
            }
            let mut branch = current.record;
            branch.state = BranchState::Deleting;
            branch.updated_at_ms = self.now_ms();
            let operation = pending(
                OperationKind::BranchDelete,
                &req.namespace,
                &branch.project_id,
                Some(&branch.id),
                self.now_ms(),
            );
            guard.deleting = true;
            let mut batch = Batch::new();
            batch.put(&branch, Some(current.version))?;
            // The guard: a child created meanwhile conflicts with it.
            batch.put(&guard, guard_version)?;
            batch.check::<ProjectRec>(
                &ProjectKey {
                    namespace: req.namespace.clone(),
                    id: req.project_id.clone(),
                },
                Some(project.version),
            )?;
            add_operation(&mut batch, &operation)?;
            let mutation = Mutation::new(batch, move |_| operation.clone());
            if let Applied::Done(op) = self.apply("DeleteBranch", claim.as_ref(), mutation).await? {
                return Ok(op);
            }
        }
        Err(Self::contended())
    }

    /// `SetDefaultBranch`: the project's default branch, which an empty
    /// `parent_id` and the routed name `<project>` take.
    ///
    /// # Errors
    ///
    /// `project_not_found`; `not_found`; `failed_precondition` when the
    /// project or branch is deleting; `aborted` at another
    /// `expected_version` (the project's).
    pub async fn set_default_branch(
        &self,
        caller: &Caller,
        req: SetDefaultBranch,
    ) -> Result<Versioned<ProjectRec>, ServiceError> {
        let claim = match self
            .begin(caller, "SetDefaultBranch", &req.idempotency_key, &req)
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
            let branch = self
                .branch(&req.project_id, &req.branch_id, "branch_id")
                .await?;
            if !live(branch.record.state) {
                return Err(ServiceError::failed_precondition(
                    "the branch is being deleted or has failed",
                ));
            }
            let mut project = current.record;
            project.default_branch_id = Some(req.branch_id.clone());
            project.updated_at_ms = self.now_ms();
            let mut batch = Batch::new();
            let project_at = batch.put(&project, Some(current.version))?;
            batch.check::<BranchRec>(&branch.record.key(), Some(branch.version))?;
            let mutation = Mutation::new(batch, move |out| Versioned {
                record: project.clone(),
                version: out[project_at].unwrap_or_default(),
            });
            if let Applied::Done(updated) = self
                .apply("SetDefaultBranch", claim.as_ref(), mutation)
                .await?
            {
                return Ok(updated);
            }
        }
        Err(Self::contended())
    }
}
