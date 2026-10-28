//! `CreateEndpoint`, `GetEndpoint`, `ListEndpoints`, `UpdateEndpoint`,
//! `StartEndpoint`, `SuspendEndpoint`, `RestartEndpoint` and
//! `DeleteEndpoint` (design §46 §3, §7.1; PG2 Task 11).
//!
//! The service writes what the endpoint should be; the endpoint reconciler
//! ([`crate::reconcile::endpoint`]) makes the computes match it under the
//! project's lease.
//!
//! - **One read-write endpoint per branch** (`endpoint_exists_for_branch`).
//!   A create rewrites its branch's [`BranchGuardRec`](crate::model::BranchGuardRec)
//!   unchanged, so two creates on one branch conflict (the second then
//!   sees the first), and so does a create against `DeleteBranch` (which
//!   writes the guard `deleting`): no endpoint is created on a branch being
//!   deleted.
//! - **Start, suspend, restart and delete** set the endpoint's `desired`
//!   state (restart: `replace_compute`; delete: state `deleting`) and append
//!   an operation to the endpoint's `open_ops`, in one batch; the reconciler
//!   finishes it. `StartEndpoint` (and `RestartEndpoint`) on a `failed`
//!   endpoint clears the failure and its crash history (§46 §7.1).
//! - **Settings.** `pg_settings` are an allow-list ([`ALLOWED_PG_SETTINGS`]):
//!   nothing an endpoint sets may override what the platform owns
//!   (`shared_preload_libraries`, `synchronous_standby_names`, where
//!   Postgres listens, the `neon.*` settings, sizes tied to the CU), and a
//!   value is plain printable text that cannot leave its line in
//!   `postgresql.conf`. The spec builder checks the list again.
//! - **Sizes.** 0.25 to 16 CU in steps of 0.25 (§46 §7.4), `min_cu ≤
//!   max_cu`; a suspend timeout of 0 (never) or at least 60 s (default
//!   300 s).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::branches::live;
use super::operations::{OperationKind, OperationRec, add_operation, pending_endpoint};
use super::projects::check_version;
use super::{
    Applied, Begin, Caller, Mutation, NeonRead, PgService, Reason, ServiceError, list_error, page,
};
use crate::ids::{BranchId, EndpointId};
use crate::model::{
    ComputeKey, ComputeRec, Cu, DesiredState, EndpointKey, EndpointPrefix, EndpointRec,
    EndpointState, EndpointType, MAX_OPEN_ENDPOINT_OPS, PoolMode, ProjectKey, ProjectRec,
    ProjectState, Record as _,
};
use crate::store::{Batch, PgControlStore, Versioned};

/// The smallest compute (§46 §7.4): 0.25 CU.
pub const MIN_CU: Cu = Cu(1);
/// The largest compute by default (§46 §7.4): 16 CU. Task 47's limits
/// record lowers it per namespace.
pub const MAX_CU: Cu = Cu(64);
/// A suspend timeout an endpoint is created with when it names none.
pub const DEFAULT_SUSPEND_TIMEOUT_S: u32 = 300;
/// The shortest suspend timeout other than 0 (never).
pub const MIN_SUSPEND_TIMEOUT_S: u32 = 60;
/// The longest value of an endpoint setting, in bytes.
pub const MAX_SETTING_VALUE_LEN: usize = 256;

/// The Postgres settings an endpoint may set, lower case. Session and
/// planner settings, timeouts, logging, autovacuum tuning and locale
/// formats: what a tenant tunes for its workload. Not here, and so never
/// settable: `shared_preload_libraries`, `synchronous_standby_names`,
/// `listen_addresses`, `port`, every `neon.*` setting, `wal_level` and the
/// replication lags, `max_connections`, `shared_buffers` and the other
/// sizes the compute's CU decides, `restart_after_crash`,
/// `password_encryption`, and anything that loads code or reads files.
pub const ALLOWED_PG_SETTINGS: &[&str] = &[
    "autovacuum_analyze_scale_factor",
    "autovacuum_analyze_threshold",
    "autovacuum_naptime",
    "autovacuum_vacuum_cost_delay",
    "autovacuum_vacuum_cost_limit",
    "autovacuum_vacuum_insert_scale_factor",
    "autovacuum_vacuum_insert_threshold",
    "autovacuum_vacuum_scale_factor",
    "autovacuum_vacuum_threshold",
    "bytea_output",
    "client_min_messages",
    "cpu_index_tuple_cost",
    "cpu_operator_cost",
    "cpu_tuple_cost",
    "datestyle",
    "deadlock_timeout",
    "default_statistics_target",
    "default_text_search_config",
    "default_transaction_isolation",
    "default_transaction_read_only",
    "effective_cache_size",
    "effective_io_concurrency",
    "enable_bitmapscan",
    "enable_hashagg",
    "enable_hashjoin",
    "enable_indexonlyscan",
    "enable_indexscan",
    "enable_material",
    "enable_mergejoin",
    "enable_nestloop",
    "enable_partitionwise_aggregate",
    "enable_partitionwise_join",
    "enable_seqscan",
    "enable_sort",
    "from_collapse_limit",
    "geqo",
    "geqo_threshold",
    "gin_pending_list_limit",
    "hash_mem_multiplier",
    "idle_in_transaction_session_timeout",
    "idle_session_timeout",
    "intervalstyle",
    "jit",
    "jit_above_cost",
    "join_collapse_limit",
    "lc_monetary",
    "lc_numeric",
    "lc_time",
    "lock_timeout",
    "log_lock_waits",
    "log_min_duration_statement",
    "log_statement",
    "log_temp_files",
    "logical_decoding_work_mem",
    "maintenance_io_concurrency",
    "maintenance_work_mem",
    "max_parallel_maintenance_workers",
    "max_parallel_workers",
    "max_parallel_workers_per_gather",
    "min_parallel_index_scan_size",
    "min_parallel_table_scan_size",
    "parallel_setup_cost",
    "parallel_tuple_cost",
    "plan_cache_mode",
    "random_page_cost",
    "search_path",
    "seq_page_cost",
    "statement_timeout",
    "temp_buffers",
    "timezone",
    "track_functions",
    "track_io_timing",
    "transaction_timeout",
    "vacuum_cost_delay",
    "vacuum_cost_limit",
    "work_mem",
];

/// Whether an endpoint may set `name` to `value`: `name` is in
/// [`ALLOWED_PG_SETTINGS`] (exactly, lower case), and `value` is 1 to
/// [`MAX_SETTING_VALUE_LEN`] bytes of printable ASCII with no quote or
/// backslash, so it stays inside its `name = '…'` line.
pub fn setting_allowed(name: &str, value: &str) -> bool {
    ALLOWED_PG_SETTINGS.binary_search(&name).is_ok()
        && !value.is_empty()
        && value.len() <= MAX_SETTING_VALUE_LEN
        && value
            .bytes()
            .all(|b| (0x20..0x7f).contains(&b) && b != b'\'' && b != b'\\')
}

/// `invalid_argument` (field `pg_settings`) unless every setting is
/// allowed ([`setting_allowed`]).
pub fn check_pg_settings(settings: &BTreeMap<String, String>) -> Result<(), ServiceError> {
    for (name, value) in settings {
        if ALLOWED_PG_SETTINGS.binary_search(&name.as_str()).is_err() {
            return Err(ServiceError::invalid(
                "pg_settings",
                format!("{name:?} is not a setting an endpoint may set"),
            ));
        }
        if !setting_allowed(name, value) {
            return Err(ServiceError::invalid(
                "pg_settings",
                format!(
                    "the value of {name} is 1 to {MAX_SETTING_VALUE_LEN} printable characters \
                     without quotes or backslashes"
                ),
            ));
        }
    }
    Ok(())
}

/// `CreateEndpointRequest`. An empty `branch_id` takes the project's default
/// branch; `None` sizes and timeouts take the defaults (0.25 CU, `max_cu`
/// = `min_cu`, 300 s, transaction pooling).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateEndpoint {
    pub namespace: String,
    pub project_id: String,
    pub branch_id: String,
    pub kind: EndpointType,
    pub min_cu: Option<Cu>,
    pub max_cu: Option<Cu>,
    /// 0: never suspend.
    pub suspend_timeout_s: Option<u32>,
    pub pool_mode: Option<PoolMode>,
    pub pg_settings: BTreeMap<String, String>,
    pub idempotency_key: String,
}

/// An endpoint as the API answers it (`loams.postgres.v1.Endpoint`): the
/// record and its current compute's, when it has one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointView {
    pub endpoint: Versioned<EndpointRec>,
    pub compute: Option<ComputeRec>,
}

/// `UpdateEndpointRequest`: each `Some` field is in the mask; none is
/// `invalid_argument`. `pg_settings` replaces the whole map.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateEndpoint {
    pub namespace: String,
    pub project_id: String,
    pub endpoint_id: String,
    pub min_cu: Option<Cu>,
    pub max_cu: Option<Cu>,
    pub suspend_timeout_s: Option<u32>,
    pub pool_mode: Option<PoolMode>,
    pub pg_settings: Option<BTreeMap<String, String>>,
    pub expected_version: Option<u64>,
    pub idempotency_key: String,
}

/// `StartEndpointRequest`, `SuspendEndpointRequest` and
/// `RestartEndpointRequest` (without `promote`, Task 16).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointAction {
    pub namespace: String,
    pub project_id: String,
    pub endpoint_id: String,
    pub idempotency_key: String,
}

/// `DeleteEndpointRequest`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteEndpoint {
    pub namespace: String,
    pub project_id: String,
    pub endpoint_id: String,
    pub expected_version: Option<u64>,
    pub idempotency_key: String,
}

/// A size of `field`: 0.25 to 16 CU.
fn check_cu(cu: Cu, field: &str) -> Result<Cu, ServiceError> {
    if cu < MIN_CU || cu > MAX_CU {
        return Err(ServiceError::invalid(
            field,
            "a compute is 0.25 to 16 CU, in steps of 0.25",
        ));
    }
    Ok(cu)
}

/// `min_cu` and `max_cu`, checked together.
fn cu_range(min: Cu, max: Cu) -> Result<(Cu, Cu), ServiceError> {
    let min = check_cu(min, "min_cu")?;
    let max = check_cu(max, "max_cu")?;
    if max < min {
        return Err(ServiceError::invalid("max_cu", "max_cu is at least min_cu"));
    }
    Ok((min, max))
}

fn check_suspend_timeout(s: u32) -> Result<u32, ServiceError> {
    if s != 0 && s < MIN_SUSPEND_TIMEOUT_S {
        return Err(ServiceError::invalid(
            "suspend_timeout",
            format!("a suspend timeout is 0 (never) or at least {MIN_SUSPEND_TIMEOUT_S} s"),
        ));
    }
    Ok(s)
}

/// A project an endpoint may be created on or started in: creating or
/// ready.
fn live_project(p: &ProjectRec) -> Result<(), ServiceError> {
    match p.state {
        ProjectState::Creating | ProjectState::Ready => Ok(()),
        _ => Err(ServiceError::failed_precondition(
            "the project is being deleted or has failed",
        )),
    }
}

/// A failed endpoint, asked to run again: no failure, no crash history.
fn clear_failure(rec: &mut EndpointRec) {
    if rec.state == EndpointState::Failed {
        rec.state = EndpointState::Idle;
    }
    rec.failure = None;
    rec.crashes.clear();
}

impl<N: NeonRead> PgService<N> {
    /// The endpoint `endpoint_id` of `project_id`.
    pub(crate) async fn endpoint(
        &self,
        project_id: &str,
        endpoint_id: &str,
        field: &str,
    ) -> Result<Versioned<EndpointRec>, ServiceError> {
        endpoint_id
            .parse::<EndpointId>()
            .map_err(|e| ServiceError::invalid(field, e.to_string()))?;
        self.store
            .get::<EndpointRec>(&EndpointKey {
                project_id: project_id.into(),
                id: endpoint_id.into(),
            })
            .await?
            .ok_or_else(|| ServiceError::not_found("endpoint", endpoint_id))
    }

    /// Every endpoint of a project.
    pub(crate) async fn all_endpoints(
        &self,
        project_id: &str,
    ) -> Result<Vec<Versioned<EndpointRec>>, ServiceError> {
        self.all::<EndpointRec>(&EndpointPrefix {
            project_id: project_id.into(),
        })
        .await
    }

    /// The view of `endpoint`: with its current compute's record.
    async fn view(&self, endpoint: Versioned<EndpointRec>) -> Result<EndpointView, ServiceError> {
        let compute = match &endpoint.record.compute_id {
            Some(id) => self
                .store
                .get::<ComputeRec>(&ComputeKey { id: id.clone() })
                .await?
                .map(|v| v.record),
            None => None,
        };
        Ok(EndpointView { endpoint, compute })
    }

    /// `CreateEndpoint`: the endpoint, `idle`, on its branch.
    ///
    /// # Errors
    ///
    /// `invalid_argument` (sizes, timeout, settings, ids);
    /// `project_not_found`; `not_found` for the branch;
    /// `endpoint_exists_for_branch` for a second read-write endpoint;
    /// `failed_precondition` when the project or the branch is being deleted
    /// or has failed.
    pub async fn create_endpoint(
        &self,
        caller: &Caller,
        req: CreateEndpoint,
    ) -> Result<EndpointView, ServiceError> {
        check_pg_settings(&req.pg_settings)?;
        let min = req.min_cu.unwrap_or(MIN_CU);
        let (min_cu, max_cu) = cu_range(min, req.max_cu.unwrap_or(min))?;
        let suspend =
            check_suspend_timeout(req.suspend_timeout_s.unwrap_or(DEFAULT_SUSPEND_TIMEOUT_S))?;
        for _ in 0..super::ATTEMPTS {
            let claim = match self
                .begin(caller, "CreateEndpoint", &req.idempotency_key, &req)
                .await?
            {
                Begin::Replay(first) => return Ok(first),
                Begin::Fresh(claim) => claim,
            };
            let project = self.project(&req.namespace, &req.project_id).await?;
            live_project(&project.record)?;
            let branch_id = match req.branch_id.as_str() {
                "" => project.record.default_branch_id.clone().ok_or_else(|| {
                    ServiceError::failed_precondition("the project has no default branch")
                })?,
                id => id.to_string(),
            };
            let branch = self
                .branch(&project.record.id, &branch_id, "branch_id")
                .await?;
            let (guard, guard_version) = self.guard(&project.record.id, &branch_id).await?;
            if !live(branch.record.state) || guard.deleting {
                return Err(ServiceError::failed_precondition(format!(
                    "branch {branch_id} is being deleted or has failed"
                )));
            }
            if req.kind == EndpointType::ReadWrite
                && let Some(other) = self
                    .all_endpoints(&project.record.id)
                    .await?
                    .into_iter()
                    .find(|e| {
                        // A read-only endpoint being promoted is the
                        // branch's read-write one to come (Task 16).
                        e.record.branch_id == branch_id
                            && (e.record.kind == EndpointType::ReadWrite || e.record.promote)
                    })
            {
                return Err(ServiceError::new(
                    Reason::EndpointExistsForBranch,
                    format!(
                        "branch {branch_id} has its read-write endpoint {}; add a read-only one",
                        other.record.id
                    ),
                )
                .with("endpoint", other.record.id));
            }
            let now = self.now_ms();
            let rec = EndpointRec {
                min_cu,
                max_cu,
                suspend_timeout_s: suspend,
                pool_mode: req.pool_mode.unwrap_or_default(),
                pg_settings: req.pg_settings.clone(),
                created_at_ms: now,
                updated_at_ms: now,
                ..EndpointRec::new(
                    project.record.id.clone(),
                    EndpointId::new().to_string(),
                    branch_id.clone(),
                    req.kind,
                )
            };
            let mut batch = Batch::new();
            let at = batch.put(&rec, None)?;
            // The branch's guard, unchanged: two creates on one branch, or a
            // create and the branch's delete, conflict.
            batch.put(&guard, guard_version)?;
            batch.check::<ProjectRec>(
                &ProjectKey {
                    namespace: req.namespace.clone(),
                    id: project.record.id.clone(),
                },
                Some(project.version),
            )?;
            let mutation = Mutation::new(batch, move |out| EndpointView {
                endpoint: Versioned {
                    record: rec.clone(),
                    version: out[at].unwrap_or_default(),
                },
                compute: None,
            });
            if let Applied::Done(created) = self
                .apply("CreateEndpoint", claim.as_ref(), mutation)
                .await?
            {
                return Ok(created);
            }
        }
        Err(Self::contended())
    }

    /// `GetEndpoint`.
    ///
    /// # Errors
    ///
    /// `project_not_found`; `not_found`; `invalid_argument`.
    pub async fn get_endpoint(
        &self,
        namespace: &str,
        project_id: &str,
        endpoint_id: &str,
    ) -> Result<EndpointView, ServiceError> {
        self.project(namespace, project_id).await?;
        let endpoint = self
            .endpoint(project_id, endpoint_id, "endpoint_id")
            .await?;
        self.view(endpoint).await
    }

    /// `ListEndpoints`: one page in id order, only `branch_id`'s when it is
    /// set (so a page may hold fewer than `page_size`), and the next page's
    /// token (empty on the last page).
    ///
    /// # Errors
    ///
    /// `project_not_found`; `invalid_argument` for a bad branch id, size or
    /// token.
    pub async fn list_endpoints(
        &self,
        namespace: &str,
        project_id: &str,
        branch_id: &str,
        page_size: i32,
        page_token: &str,
    ) -> Result<(Vec<EndpointView>, String), ServiceError> {
        self.project(namespace, project_id).await?;
        if !branch_id.is_empty() {
            branch_id
                .parse::<BranchId>()
                .map_err(|e| ServiceError::invalid("branch_id", e.to_string()))?;
        }
        let prefix = EndpointPrefix {
            project_id: project_id.into(),
        };
        let (endpoints, next) = self
            .store
            .list::<EndpointRec>(&prefix, page(page_size, page_token)?)
            .await
            .map_err(list_error)?;
        let mut out = Vec::with_capacity(endpoints.len());
        for e in endpoints {
            if branch_id.is_empty() || e.record.branch_id == branch_id {
                out.push(self.view(e).await?);
            }
        }
        Ok((out, next.unwrap_or_default()))
    }

    /// `UpdateEndpoint`: sizes, the suspend timeout, the pool mode and the
    /// settings. A running compute gets changed settings through the
    /// reconciler's push of the new spec; sizes apply from the next compute
    /// (the autoscaler, Task 15, moves within them).
    ///
    /// # Errors
    ///
    /// `invalid_argument` (an empty mask too); `aborted` at another
    /// `expected_version`; `failed_precondition` while the endpoint or the
    /// project is being deleted; `not_found`.
    pub async fn update_endpoint(
        &self,
        caller: &Caller,
        req: UpdateEndpoint,
    ) -> Result<EndpointView, ServiceError> {
        if req.min_cu.is_none()
            && req.max_cu.is_none()
            && req.suspend_timeout_s.is_none()
            && req.pool_mode.is_none()
            && req.pg_settings.is_none()
        {
            return Err(ServiceError::invalid(
                "update_mask",
                "the update mask names at least one of min_cu, max_cu, suspend_timeout, \
                 pool_mode, pg_settings",
            ));
        }
        if let Some(settings) = &req.pg_settings {
            check_pg_settings(settings)?;
        }
        if let Some(s) = req.suspend_timeout_s {
            check_suspend_timeout(s)?;
        }
        for _ in 0..super::ATTEMPTS {
            let claim = match self
                .begin(caller, "UpdateEndpoint", &req.idempotency_key, &req)
                .await?
            {
                Begin::Replay(first) => return Ok(first),
                Begin::Fresh(claim) => claim,
            };
            let project = self.project(&req.namespace, &req.project_id).await?;
            if project.record.state == ProjectState::Deleting {
                return Err(ServiceError::failed_precondition(
                    "the project is being deleted",
                ));
            }
            let current = self
                .endpoint(&req.project_id, &req.endpoint_id, "endpoint.id")
                .await?;
            check_version(req.expected_version, current.version)?;
            if current.record.state == EndpointState::Deleting {
                return Err(ServiceError::failed_precondition(
                    "the endpoint is being deleted",
                ));
            }
            let mut rec = current.record.clone();
            let (min, max) = cu_range(
                req.min_cu.unwrap_or(rec.min_cu),
                req.max_cu.unwrap_or(rec.max_cu),
            )?;
            rec.min_cu = min;
            rec.max_cu = max;
            let mut spec_changed = false;
            if let Some(s) = req.suspend_timeout_s {
                spec_changed |= s != rec.suspend_timeout_s;
                rec.suspend_timeout_s = s;
            }
            if let Some(mode) = req.pool_mode {
                rec.pool_mode = mode;
            }
            if let Some(settings) = &req.pg_settings {
                spec_changed |= *settings != rec.pg_settings;
                rec.pg_settings = settings.clone();
            }
            if spec_changed {
                rec.settings_version = rec.settings_version.saturating_add(1);
            }
            rec.updated_at_ms = self.now_ms();
            let compute = self.view(current.clone()).await?.compute;
            let mut batch = Batch::new();
            let at = batch.put(&rec, Some(current.version))?;
            batch.check::<ProjectRec>(
                &ProjectKey {
                    namespace: req.namespace.clone(),
                    id: req.project_id.clone(),
                },
                Some(project.version),
            )?;
            let mutation = Mutation::new(batch, move |out| EndpointView {
                endpoint: Versioned {
                    record: rec.clone(),
                    version: out[at].unwrap_or_default(),
                },
                compute: compute.clone(),
            });
            if let Applied::Done(updated) = self
                .apply("UpdateEndpoint", claim.as_ref(), mutation)
                .await?
            {
                return Ok(updated);
            }
        }
        Err(Self::contended())
    }

    /// One endpoint action: `change` the record, and append an operation of
    /// `kind` that the reconciler finishes.
    async fn endpoint_action(
        &self,
        caller: &Caller,
        rpc: &'static str,
        kind: OperationKind,
        req: &EndpointAction,
        expected_version: Option<u64>,
        change: impl Fn(&mut EndpointRec) -> Result<(), ServiceError>,
    ) -> Result<OperationRec, ServiceError> {
        let promote = kind == OperationKind::EndpointPromote;
        for _ in 0..super::ATTEMPTS {
            let claim = match self
                .begin(caller, rpc, &req.idempotency_key, &(req, expected_version))
                .await?
            {
                Begin::Replay(first) => return Ok(first),
                Begin::Fresh(claim) => claim,
            };
            let project = self.project(&req.namespace, &req.project_id).await?;
            let current = self
                .endpoint(&req.project_id, &req.endpoint_id, "endpoint_id")
                .await?;
            check_version(expected_version, current.version)?;
            if current.record.state == EndpointState::Deleting {
                return Err(ServiceError::failed_precondition(
                    "the endpoint is being deleted",
                ));
            }
            if current.record.open_ops.len() >= MAX_OPEN_ENDPOINT_OPS {
                return Err(ServiceError::new(
                    Reason::Aborted,
                    "too many operations are open on this endpoint; wait for them to finish",
                ));
            }
            if kind != OperationKind::EndpointDelete {
                live_project(&project.record)?;
                let branch = self
                    .branch(&req.project_id, &current.record.branch_id, "branch_id")
                    .await?;
                if !live(branch.record.state) {
                    return Err(ServiceError::failed_precondition(format!(
                        "branch {} is being deleted or has failed",
                        current.record.branch_id
                    )));
                }
            }
            let mut rec = current.record.clone();
            change(&mut rec)?;
            let now = self.now_ms();
            let op = pending_endpoint(kind, &req.namespace, &rec, now);
            rec.open_ops.push(op.id.clone());
            rec.updated_at_ms = now;
            let mut batch = Batch::new();
            batch.put(&rec, Some(current.version))?;
            if promote {
                self.promotable(&mut batch, &rec).await?;
            } else if rec.kind == EndpointType::ReadWrite
                && matches!(
                    kind,
                    OperationKind::EndpointStart | OperationKind::EndpointRestart
                )
            {
                self.no_promotion(&mut batch, &rec).await?;
            }
            add_operation(&mut batch, &op)?;
            batch.check::<ProjectRec>(
                &ProjectKey {
                    namespace: req.namespace.clone(),
                    id: req.project_id.clone(),
                },
                Some(project.version),
            )?;
            let mutation = Mutation::new(batch, move |_| op.clone());
            if let Applied::Done(op) = self.apply(rpc, claim.as_ref(), mutation).await? {
                return Ok(op);
            }
        }
        Err(Self::contended())
    }

    /// `StartEndpoint`: desired `running`; a `failed` endpoint's failure is
    /// cleared. Operation `postgres.endpoint.start`, finished when the
    /// endpoint runs (or fails).
    ///
    /// # Errors
    ///
    /// `failed_precondition` while the endpoint, its branch or the project
    /// is being deleted; `aborted` with too many open operations;
    /// `not_found`; `project_not_found`.
    pub async fn start_endpoint(
        &self,
        caller: &Caller,
        req: EndpointAction,
    ) -> Result<OperationRec, ServiceError> {
        self.may_wake(&req.namespace)?;
        self.endpoint_action(
            caller,
            "StartEndpoint",
            OperationKind::EndpointStart,
            &req,
            None,
            |rec| {
                rec.desired = DesiredState::Running;
                clear_failure(rec);
                Ok(())
            },
        )
        .await
    }

    /// `SuspendEndpoint`: desired `suspended`. Operation
    /// `postgres.endpoint.suspend`, finished once no compute runs.
    ///
    /// # Errors
    ///
    /// As [`start_endpoint`](Self::start_endpoint).
    pub async fn suspend_endpoint(
        &self,
        caller: &Caller,
        req: EndpointAction,
    ) -> Result<OperationRec, ServiceError> {
        self.endpoint_action(
            caller,
            "SuspendEndpoint",
            OperationKind::EndpointSuspend,
            &req,
            None,
            |rec| {
                rec.desired = DesiredState::Suspended;
                Ok(())
            },
        )
        .await
    }

    /// `RestartEndpoint`: a new compute replaces the current one behind the
    /// same address (an endpoint with no compute just starts). Operation
    /// `postgres.endpoint.restart`.
    ///
    /// # Errors
    ///
    /// As [`start_endpoint`](Self::start_endpoint).
    pub async fn restart_endpoint(
        &self,
        caller: &Caller,
        req: EndpointAction,
    ) -> Result<OperationRec, ServiceError> {
        self.may_wake(&req.namespace)?;
        self.endpoint_action(
            caller,
            "RestartEndpoint",
            OperationKind::EndpointRestart,
            &req,
            None,
            |rec| {
                rec.desired = DesiredState::Running;
                clear_failure(rec);
                if rec.compute_id.is_some()
                    || matches!(rec.state, EndpointState::Starting | EndpointState::Running)
                {
                    rec.replace_compute = true;
                }
                Ok(())
            },
        )
        .await
    }

    /// `RestartEndpoint{promote}` (Task 16): this read-only endpoint becomes
    /// its branch's read-write one, and the read-write one (if any) a
    /// read-only one. Operation `postgres.endpoint.restart` (kind
    /// [`OperationKind::EndpointPromote`]), finished when the types are
    /// swapped, or failed `failed_precondition` when `compute_ctl` refused.
    ///
    /// # Errors
    ///
    /// `failed_precondition` unless the endpoint is read-only and running,
    /// with no promotion already asked, and its branch's read-write
    /// endpoint is down (no compute, not asked to run); otherwise as
    /// [`start_endpoint`](Self::start_endpoint).
    pub async fn promote_endpoint(
        &self,
        caller: &Caller,
        req: EndpointAction,
    ) -> Result<OperationRec, ServiceError> {
        self.endpoint_action(
            caller,
            "RestartEndpoint",
            OperationKind::EndpointPromote,
            &req,
            None,
            |rec| {
                if rec.kind != EndpointType::ReadOnly {
                    return Err(ServiceError::failed_precondition(
                        "only a read-only endpoint is promoted",
                    ));
                }
                if rec.promote {
                    return Err(ServiceError::failed_precondition(
                        "a promotion of this endpoint is under way",
                    ));
                }
                if rec.desired != DesiredState::Running
                    || rec.state != EndpointState::Running
                    || rec.replace_compute
                {
                    return Err(ServiceError::failed_precondition(
                        "the read-only endpoint must be running to be promoted",
                    ));
                }
                rec.promote = true;
                Ok(())
            },
        )
        .await
    }

    /// The promotion's checks against the endpoint's branch (Task 16): its
    /// read-write endpoint is down, and stays so until the batch commits
    /// (its version is checked), and the branch's guard is rewritten
    /// unchanged, so a `CreateEndpoint` of a read-write endpoint racing the
    /// promotion conflicts with it.
    async fn promotable(&self, batch: &mut Batch, rec: &EndpointRec) -> Result<(), ServiceError> {
        let (guard, guard_version) = self.guard(&rec.project_id, &rec.branch_id).await?;
        batch.put(&guard, guard_version)?;
        for other in self.all_endpoints(&rec.project_id).await? {
            let o = &other.record;
            if o.id == rec.id || o.branch_id != rec.branch_id {
                continue;
            }
            if o.promote {
                return Err(ServiceError::failed_precondition(format!(
                    "endpoint {} of this branch is being promoted",
                    o.id
                )));
            }
            if o.kind != EndpointType::ReadWrite {
                continue;
            }
            if o.desired == DesiredState::Running || o.compute_id.is_some() {
                return Err(ServiceError::failed_precondition(format!(
                    "the branch's read-write endpoint {} is running; suspend it first",
                    o.id
                ))
                .with("endpoint", o.id.clone()));
            }
            batch.check::<EndpointRec>(&o.key(), Some(other.version))?;
        }
        Ok(())
    }

    /// A read-write endpoint does not start while a read-only endpoint of
    /// its branch is being promoted (two primaries could result, Task 16).
    /// The branch's guard is rewritten unchanged, as [`Self::promotable`]
    /// does, so a start and a promotion conflict whichever commits first.
    async fn no_promotion(&self, batch: &mut Batch, rec: &EndpointRec) -> Result<(), ServiceError> {
        let (guard, guard_version) = self.guard(&rec.project_id, &rec.branch_id).await?;
        batch.put(&guard, guard_version)?;
        if let Some(other) = self
            .all_endpoints(&rec.project_id)
            .await?
            .into_iter()
            .find(|o| {
                o.record.id != rec.id && o.record.branch_id == rec.branch_id && o.record.promote
            })
        {
            return Err(ServiceError::failed_precondition(format!(
                "read-only endpoint {} of this branch is being promoted",
                other.record.id
            ))
            .with("endpoint", other.record.id));
        }
        Ok(())
    }

    /// `DeleteEndpoint`: state `deleting`; the reconciler stops the compute,
    /// gives up the address and removes the records. Operation
    /// `postgres.endpoint.delete`.
    ///
    /// # Errors
    ///
    /// `failed_precondition` when already deleting; `aborted` at another
    /// `expected_version`; `not_found`; `project_not_found`.
    pub async fn delete_endpoint(
        &self,
        caller: &Caller,
        req: DeleteEndpoint,
    ) -> Result<OperationRec, ServiceError> {
        let action = EndpointAction {
            namespace: req.namespace,
            project_id: req.project_id,
            endpoint_id: req.endpoint_id,
            idempotency_key: req.idempotency_key,
        };
        self.endpoint_action(
            caller,
            "DeleteEndpoint",
            OperationKind::EndpointDelete,
            &action,
            req.expected_version,
            |rec| {
                rec.state = EndpointState::Deleting;
                rec.desired = DesiredState::Suspended;
                rec.replace_compute = false;
                Ok(())
            },
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_allow_list_is_sorted_and_lower_case() {
        let mut sorted = ALLOWED_PG_SETTINGS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted, ALLOWED_PG_SETTINGS, "binary_search needs it sorted");
        for name in ALLOWED_PG_SETTINGS {
            assert_eq!(*name, name.to_lowercase());
            assert!(!name.starts_with("neon"), "{name}");
        }
    }

    #[test]
    fn platform_settings_are_never_allowed() {
        for name in [
            "shared_preload_libraries",
            "synchronous_standby_names",
            "listen_addresses",
            "port",
            "restart_after_crash",
            "password_encryption",
            "max_replication_write_lag",
            "max_replication_flush_lag",
            "neon.max_cluster_size",
            "wal_level",
            "max_connections",
            "shared_buffers",
            "session_preload_libraries",
            "local_preload_libraries",
            "dynamic_library_path",
            "ssl",
            "hba_file",
        ] {
            assert!(!setting_allowed(name, "x"), "{name}");
        }
        assert!(setting_allowed("work_mem", "64MB"));
        assert!(!setting_allowed("work_mem", "64MB'"));
        assert!(!setting_allowed("work_mem", "a\\b"));
        assert!(!setting_allowed("work_mem", "a\nb"));
        assert!(!setting_allowed(
            "work_mem",
            &"x".repeat(MAX_SETTING_VALUE_LEN + 1)
        ));
    }
}
