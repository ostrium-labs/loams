//! The session-cluster controller (D1972, D1974, D1976): one Flink session
//! cluster per tenant, its lifecycle behind [`ManagedEngine`], and the
//! compose recovery record.
//!
//! - **create**: render the config, apply the objects, wait until the
//!   JobManager reports the TaskManagers and the SQL Gateway answers.
//! - **scale**: a TaskManager count change scales the Deployment; a change
//!   of slots or memory is a redeploy (as an upgrade, through savepoints).
//! - **upgrade**: stop every managed job with a CANONICAL savepoint, wait
//!   for each, redeploy with the new image, wait until ready, resubmit each
//!   job's SQL with `execution.state-recovery.path`. A savepoint that fails
//!   aborts before anything is redeployed, and the jobs already stopped are
//!   restarted from their savepoints.
//! - **suspend**: stop with savepoints, scale every role to zero.
//! - **resume**: scale up, wait, restore every job from its savepoint.
//! - **delete**: cancel the jobs, remove the workloads; the tenant's prefix
//!   only with `purge`.
//!
//! The cluster records are in memory here; ENG-UI's store (Loams Postgres)
//! replaces [`ClusterRecords`]' map. The jobs come from a [`JobCatalog`]
//! (FL-b's job store implements it).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::config::{
    ClusterSpec, FlinkConfig, HaMode, Platform, Resources, S3Credentials, Size, render,
};
use crate::engine::{
    Endpoint, EngineError, EngineKind, EngineStatus, EngineVersion, Health, ManagedEngine,
    MeteringHook, Phase, check_upgrade,
};
use crate::error::FlinkError;
use crate::k8s::Role;
use crate::rest::{FlinkRestClient, JobId, SavepointFormat, StopRequest};
use crate::sql_gateway::{
    ApiVersion, ExecuteStatement, FetchLimits, OpenSession, SqlGatewayClient,
};
use crate::storage::TenantId;

/// The Flink option a resubmitted job restores from (Flink 2.x).
pub const STATE_RECOVERY_PATH: &str = "execution.state-recovery.path";
/// The Flink option naming a job (how the controller finds it again).
pub const PIPELINE_NAME: &str = "pipeline.name";

/// Why a runtime call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeError {
    /// The platform cannot do this (e.g. scaling a compose stack).
    #[error("unsupported on this platform: {0}")]
    Unsupported(String),
    /// The platform refused or failed.
    #[error("{0}")]
    Failed(String),
}

impl From<RuntimeError> for EngineError {
    fn from(e: RuntimeError) -> Self {
        match e {
            RuntimeError::Unsupported(m) => EngineError::Invalid(m),
            RuntimeError::Failed(m) => EngineError::Runtime(m),
        }
    }
}

impl From<FlinkError> for EngineError {
    fn from(e: FlinkError) -> Self {
        EngineError::Runtime(e.to_string())
    }
}

/// Replicas per role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Replicas {
    /// JobManagers.
    pub jobmanager: u32,
    /// TaskManagers.
    pub taskmanager: u32,
    /// SQL Gateways.
    pub sql_gateway: u32,
}

impl Replicas {
    /// The running counts of `spec` ([`crate::k8s::replicas`]).
    pub fn running(spec: &ClusterSpec) -> Self {
        Self {
            jobmanager: crate::k8s::replicas(spec, Role::JobManager),
            taskmanager: crate::k8s::replicas(spec, Role::TaskManager),
            sql_gateway: crate::k8s::replicas(spec, Role::SqlGateway),
        }
    }

    /// The count of `role`.
    pub fn of(&self, role: Role) -> u32 {
        match role {
            Role::JobManager => self.jobmanager,
            Role::TaskManager => self.taskmanager,
            Role::SqlGateway => self.sql_gateway,
        }
    }
}

/// The base URLs the controller reaches a tenant's cluster on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterEndpoints {
    /// The JobManager REST API (`http://flink-jobmanager.<ns>.svc:8081`).
    pub rest: String,
    /// The SQL Gateway (`http://flink-sql-gateway.<ns>.svc:8083`).
    pub sql_gateway: String,
}

/// Where a tenant's cluster runs: Kubernetes ([`crate::runtime_kube`]) or
/// the compose stack ([`ComposeRuntime`]).
#[async_trait]
pub trait ClusterRuntime: Send + Sync {
    /// The platform.
    fn platform(&self) -> Platform;

    /// Creates or updates every object of the cluster (idempotent).
    async fn apply(
        &self,
        spec: &ClusterSpec,
        config: &FlinkConfig,
        creds: &S3Credentials,
        replicas: Replicas,
    ) -> Result<(), RuntimeError>;

    /// Sets `role`'s replicas.
    async fn scale(&self, tenant: &TenantId, role: Role, replicas: u32)
    -> Result<(), RuntimeError>;

    /// Ready replicas per role.
    async fn ready(&self, tenant: &TenantId) -> Result<Replicas, RuntimeError>;

    /// Removes the cluster's workloads (and namespace).
    async fn delete(&self, tenant: &TenantId) -> Result<(), RuntimeError>;

    /// The URLs of the tenant's cluster.
    fn endpoints(&self, tenant: &TenantId) -> ClusterEndpoints;
}

/// The compose `flink` profile: one cluster the stack runs, which the
/// controller adopts. Changing it is `docker compose`'s job, so scale and
/// delete are refused; readiness is the JobManager's REST answer.
#[derive(Debug, Clone)]
pub struct ComposeRuntime {
    endpoints: ClusterEndpoints,
}

impl ComposeRuntime {
    /// The stack's services (`deploy/release/compose.flink.yaml`).
    pub fn new() -> Self {
        Self::with_endpoints(ClusterEndpoints {
            rest: format!(
                "http://{}:{}",
                crate::config::addr::JOBMANAGER,
                crate::config::addr::REST_PORT
            ),
            sql_gateway: format!(
                "http://{}:{}",
                crate::config::addr::SQL_GATEWAY,
                crate::config::addr::SQL_GATEWAY_PORT
            ),
        })
    }

    /// Other URLs (tests, a remapped stack).
    pub fn with_endpoints(endpoints: ClusterEndpoints) -> Self {
        Self { endpoints }
    }
}

impl Default for ComposeRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ClusterRuntime for ComposeRuntime {
    fn platform(&self) -> Platform {
        Platform::Compose
    }

    async fn apply(
        &self,
        _spec: &ClusterSpec,
        _config: &FlinkConfig,
        _creds: &S3Credentials,
        _replicas: Replicas,
    ) -> Result<(), RuntimeError> {
        // The stack is already running from deploy/flink/compose/config.yaml;
        // adopting it changes nothing.
        Ok(())
    }

    async fn scale(
        &self,
        _tenant: &TenantId,
        _role: Role,
        _replicas: u32,
    ) -> Result<(), RuntimeError> {
        Err(RuntimeError::Unsupported(
            "compose: scale with `docker compose up --scale flink-taskmanager=N`".to_owned(),
        ))
    }

    async fn ready(&self, _tenant: &TenantId) -> Result<Replicas, RuntimeError> {
        let rest = FlinkRestClient::new(&self.endpoints.rest)
            .map_err(|e| RuntimeError::Failed(e.to_string()))?;
        match rest.overview().await {
            Ok(o) => Ok(Replicas {
                jobmanager: 1,
                taskmanager: o.taskmanagers,
                sql_gateway: 1,
            }),
            Err(_) => Ok(Replicas::default()),
        }
    }

    async fn delete(&self, _tenant: &TenantId) -> Result<(), RuntimeError> {
        Err(RuntimeError::Unsupported(
            "compose: remove the `flink` profile from COMPOSE_PROFILES".to_owned(),
        ))
    }

    fn endpoints(&self, _tenant: &TenantId) -> ClusterEndpoints {
        self.endpoints.clone()
    }
}

/// S3 credentials per tenant (prefix-scoped keys in production; one pair in
/// compose).
#[async_trait]
pub trait CredentialProvider: Send + Sync {
    /// The tenant's credentials, minted on first use.
    async fn credentials(&self, tenant: &TenantId) -> Result<S3Credentials, EngineError>;
    /// Revokes them (delete with purge).
    async fn revoke(&self, tenant: &TenantId) -> Result<(), EngineError>;
}

/// One pair of credentials for every tenant (compose, tests).
#[derive(Debug, Clone)]
pub struct StaticCredentials(pub S3Credentials);

#[async_trait]
impl CredentialProvider for StaticCredentials {
    async fn credentials(&self, _tenant: &TenantId) -> Result<S3Credentials, EngineError> {
        Ok(self.0.clone())
    }

    async fn revoke(&self, _tenant: &TenantId) -> Result<(), EngineError> {
        Ok(())
    }
}

/// A job the controller keeps running across upgrades and suspends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedJob {
    /// The job's name, unique per tenant; also its `pipeline.name`.
    pub name: String,
    /// Session statements run before it (`SET`, `CREATE CATALOG`,
    /// `CREATE TEMPORARY TABLE`, `USE`), through `configure-session`.
    #[serde(default)]
    pub setup: Vec<String>,
    /// The statement that starts it (`INSERT INTO …` or
    /// `EXECUTE STATEMENT SET BEGIN … END`).
    pub statement: String,
    /// The Flink job it runs as now, if running.
    #[serde(default)]
    pub flink_job: Option<JobId>,
    /// The savepoint it was last stopped with.
    #[serde(default)]
    pub last_savepoint: Option<String>,
}

/// Where the controller reads and records managed jobs (FL-b's job store
/// implements it; [`MemoryJobCatalog`] for compose and tests).
#[async_trait]
pub trait JobCatalog: Send + Sync {
    /// The tenant's managed jobs that should be running.
    async fn managed_jobs(&self, tenant: &TenantId) -> Result<Vec<ManagedJob>, EngineError>;
    /// `name` was stopped with `savepoint`.
    async fn record_stopped(
        &self,
        tenant: &TenantId,
        name: &str,
        savepoint: Option<String>,
    ) -> Result<(), EngineError>;
    /// `name` runs as `job`.
    async fn record_started(
        &self,
        tenant: &TenantId,
        name: &str,
        job: JobId,
    ) -> Result<(), EngineError>;
}

/// A [`JobCatalog`] in memory.
#[derive(Debug, Default)]
pub struct MemoryJobCatalog {
    jobs: Mutex<HashMap<TenantId, BTreeMap<String, ManagedJob>>>,
}

impl MemoryJobCatalog {
    /// Adds or replaces `job`.
    pub async fn put(&self, tenant: &TenantId, job: ManagedJob) {
        self.jobs
            .lock()
            .await
            .entry(tenant.clone())
            .or_default()
            .insert(job.name.clone(), job);
    }

    /// The job called `name`.
    pub async fn get(&self, tenant: &TenantId, name: &str) -> Option<ManagedJob> {
        self.jobs
            .lock()
            .await
            .get(tenant)
            .and_then(|m| m.get(name))
            .cloned()
    }
}

#[async_trait]
impl JobCatalog for MemoryJobCatalog {
    async fn managed_jobs(&self, tenant: &TenantId) -> Result<Vec<ManagedJob>, EngineError> {
        Ok(self
            .jobs
            .lock()
            .await
            .get(tenant)
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default())
    }

    async fn record_stopped(
        &self,
        tenant: &TenantId,
        name: &str,
        savepoint: Option<String>,
    ) -> Result<(), EngineError> {
        let mut jobs = self.jobs.lock().await;
        let job = jobs
            .get_mut(tenant)
            .and_then(|m| m.get_mut(name))
            .ok_or_else(|| EngineError::NotFound(format!("{tenant}/{name}")))?;
        job.flink_job = None;
        if savepoint.is_some() {
            job.last_savepoint = savepoint;
        }
        Ok(())
    }

    async fn record_started(
        &self,
        tenant: &TenantId,
        name: &str,
        job: JobId,
    ) -> Result<(), EngineError> {
        let mut jobs = self.jobs.lock().await;
        let entry = jobs
            .get_mut(tenant)
            .and_then(|m| m.get_mut(name))
            .ok_or_else(|| EngineError::NotFound(format!("{tenant}/{name}")))?;
        entry.flink_job = Some(job);
        Ok(())
    }
}

/// One job in the compose recovery record (D1974).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryEntry {
    /// The managed job.
    pub job: ManagedJob,
    /// The newest retained checkpoint or savepoint seen for it.
    #[serde(default)]
    pub restore_path: Option<String>,
    /// When the entry was written, ms since the epoch.
    pub updated_ms: u64,
}

/// The compose recovery record, `flink/<tenant>/recovery/jobs.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryRecord {
    /// The record's format.
    pub version: u32,
    /// By job name.
    pub jobs: BTreeMap<String, RecoveryEntry>,
}

/// Reads and writes recovery records on Loams object storage.
#[derive(Debug, Clone)]
pub struct RecoveryStore {
    store: loams_store::Store,
}

impl RecoveryStore {
    /// Records in `store` (the bucket of [`ClusterSpec::storage`], e.g.
    /// `loams_store::Store::from_url("s3://flink", …)`).
    pub fn new(store: loams_store::Store) -> Self {
        Self { store }
    }

    /// The tenant's record; empty when there is none yet.
    ///
    /// # Errors
    ///
    /// [`EngineError::Runtime`] for a store error or a record that does not
    /// parse.
    pub async fn load(&self, spec: &ClusterSpec) -> Result<RecoveryRecord, EngineError> {
        let key = spec.storage.tenant(&spec.tenant).recovery_key();
        match self.store.get(&key).await {
            Ok((bytes, _)) => serde_json::from_slice(&bytes)
                .map_err(|e| EngineError::Runtime(format!("recovery record {key}: {e}"))),
            Err(loams_store::StoreError::NotFound { .. }) => Ok(RecoveryRecord {
                version: 1,
                jobs: BTreeMap::new(),
            }),
            Err(e) => Err(EngineError::Runtime(format!("recovery record {key}: {e}"))),
        }
    }

    /// Writes the tenant's record.
    ///
    /// # Errors
    ///
    /// [`EngineError::Runtime`] for a store error.
    pub async fn save(
        &self,
        spec: &ClusterSpec,
        record: &RecoveryRecord,
    ) -> Result<(), EngineError> {
        let key = spec.storage.tenant(&spec.tenant).recovery_key();
        let bytes = serde_json::to_vec_pretty(record)
            .map_err(|e| EngineError::Runtime(format!("recovery record: {e}")))?;
        self.store
            .put(&key, Bytes::from(bytes))
            .await
            .map(|_| ())
            .map_err(|e| EngineError::Runtime(format!("recovery record {key}: {e}")))
    }

    /// Deletes every object under the tenant's prefix (delete with purge).
    ///
    /// # Errors
    ///
    /// [`EngineError::Runtime`] for a store error.
    pub async fn purge(&self, spec: &ClusterSpec) -> Result<u64, EngineError> {
        let prefix = spec.storage.tenant(&spec.tenant).key_prefix();
        let prefix = prefix.trim_end_matches('/');
        let objects = self
            .store
            .list(prefix)
            .await
            .map_err(|e| EngineError::Runtime(format!("listing {prefix}: {e}")))?;
        let mut deleted = 0;
        for obj in objects {
            match self.store.delete(&obj.path).await {
                Ok(()) | Err(loams_store::StoreError::NotFound { .. }) => deleted += 1,
                Err(e) => {
                    return Err(EngineError::Runtime(format!("deleting {}: {e}", obj.path)));
                }
            }
        }
        Ok(deleted)
    }
}

/// How long the controller waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    /// For one savepoint.
    pub savepoint: Duration,
    /// For the cluster to be ready after an apply or scale.
    pub ready: Duration,
    /// Between polls.
    pub poll: Duration,
    /// For a resubmitted statement to return its job id.
    pub submit: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            savepoint: Duration::from_secs(600),
            ready: Duration::from_secs(300),
            poll: Duration::from_secs(2),
            submit: Duration::from_secs(120),
        }
    }
}

/// A cluster the controller knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterRecord {
    /// Its spec.
    pub spec: ClusterSpec,
    /// Its phase.
    pub phase: Phase,
    /// What happened last.
    pub message: Option<String>,
}

/// The controller's cluster records (in memory; ENG-UI's store later).
#[derive(Debug, Default)]
pub struct ClusterRecords {
    map: Mutex<HashMap<TenantId, ClusterRecord>>,
}

/// A stopped job and where it can restart from.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stopped {
    job: ManagedJob,
    savepoint: Option<String>,
}

/// The Flink session-cluster controller.
pub struct FlinkController {
    runtime: Arc<dyn ClusterRuntime>,
    catalog: Arc<dyn JobCatalog>,
    creds: Arc<dyn CredentialProvider>,
    metering: Arc<dyn MeteringHook>,
    recovery: Option<RecoveryStore>,
    records: ClusterRecords,
    timeouts: Timeouts,
    http: reqwest::Client,
}

impl std::fmt::Debug for FlinkController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FlinkController")
            .field("platform", &self.runtime.platform())
            .field("recovery", &self.recovery.is_some())
            .field("timeouts", &self.timeouts)
            .finish_non_exhaustive()
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

impl FlinkController {
    /// A controller over `runtime`.
    pub fn new(
        runtime: Arc<dyn ClusterRuntime>,
        catalog: Arc<dyn JobCatalog>,
        creds: Arc<dyn CredentialProvider>,
        metering: Arc<dyn MeteringHook>,
    ) -> Self {
        Self {
            runtime,
            catalog,
            creds,
            metering,
            recovery: None,
            records: ClusterRecords::default(),
            timeouts: Timeouts::default(),
            http: reqwest::Client::new(),
        }
    }

    /// Keeps a recovery record on object storage (compose, D1974).
    pub fn with_recovery(mut self, recovery: RecoveryStore) -> Self {
        self.recovery = Some(recovery);
        self
    }

    /// Other timeouts.
    pub fn with_timeouts(mut self, timeouts: Timeouts) -> Self {
        self.timeouts = timeouts;
        self
    }

    /// Adopts a cluster that already runs (the compose stack, or a record
    /// restored from ENG-UI's store) without applying anything.
    pub async fn adopt(&self, spec: ClusterSpec, phase: Phase) {
        self.records.map.lock().await.insert(
            spec.tenant.clone(),
            ClusterRecord {
                spec,
                phase,
                message: None,
            },
        );
    }

    /// The tenant's record.
    pub async fn record(&self, tenant: &TenantId) -> Option<ClusterRecord> {
        self.records.map.lock().await.get(tenant).cloned()
    }

    fn rest(&self, tenant: &TenantId) -> Result<FlinkRestClient, EngineError> {
        Ok(FlinkRestClient::with_client(
            &self.runtime.endpoints(tenant).rest,
            self.http.clone(),
        )?)
    }

    fn sql(&self, tenant: &TenantId) -> Result<SqlGatewayClient, EngineError> {
        Ok(SqlGatewayClient::with_client(
            &self.runtime.endpoints(tenant).sql_gateway,
            ApiVersion::V3,
            self.http.clone(),
        )?)
    }

    fn status_of(&self, record: &ClusterRecord) -> EngineStatus {
        let endpoints = self.runtime.endpoints(&record.spec.tenant);
        EngineStatus {
            tenant: record.spec.tenant.clone(),
            kind: EngineKind::Flink,
            phase: record.phase.clone(),
            version: record.spec.flink_version.clone(),
            size: record.spec.size,
            resources: record.spec.resources,
            endpoints: vec![
                endpoint("sql-gateway", &endpoints.sql_gateway),
                endpoint("rest", &endpoints.rest),
            ],
            message: record.message.clone(),
        }
    }

    /// Moves the tenant into `busy` when its phase is one of `from`;
    /// returns its spec.
    async fn begin(
        &self,
        tenant: &TenantId,
        from: &[Phase],
        busy: Phase,
    ) -> Result<ClusterSpec, EngineError> {
        let status = {
            let mut map = self.records.map.lock().await;
            let rec = map
                .get_mut(tenant)
                .ok_or_else(|| EngineError::NotFound(tenant.to_string()))?;
            let allowed = from.iter().any(|p| match (p, &rec.phase) {
                (Phase::Failed { .. }, Phase::Failed { .. }) => true,
                (a, b) => a == b,
            });
            if !allowed {
                return Err(EngineError::Conflict {
                    phase: rec.phase.clone(),
                });
            }
            rec.phase = busy;
            rec.message = None;
            self.status_of(rec)
        };
        self.metering.on_change(&status).await;
        status_spec(&self.records, tenant).await
    }

    /// Ends an operation: the new phase, spec and message.
    async fn finish(
        &self,
        tenant: &TenantId,
        phase: Phase,
        spec: Option<ClusterSpec>,
        message: Option<String>,
    ) -> Result<EngineStatus, EngineError> {
        let status = {
            let mut map = self.records.map.lock().await;
            let rec = map
                .get_mut(tenant)
                .ok_or_else(|| EngineError::NotFound(tenant.to_string()))?;
            rec.phase = phase;
            if let Some(spec) = spec {
                rec.spec = spec;
            }
            rec.message = message;
            self.status_of(rec)
        };
        self.metering.on_change(&status).await;
        Ok(status)
    }

    /// Ends an operation that failed: `Failed{op}` with the error, which is
    /// returned.
    async fn fail(&self, tenant: &TenantId, op: &str, err: EngineError) -> EngineError {
        tracing::warn!(%tenant, op, error = %err, "flink cluster operation failed");
        let _ = self
            .finish(
                tenant,
                Phase::Failed { op: op.to_owned() },
                None,
                Some(err.to_string()),
            )
            .await;
        err
    }

    /// Waits until every role has `want` ready replicas, the JobManager
    /// reports the TaskManagers and the SQL Gateway answers.
    async fn wait_ready(&self, spec: &ClusterSpec, want: Replicas) -> Result<(), EngineError> {
        let deadline = Instant::now() + self.timeouts.ready;
        let tenant = &spec.tenant;
        loop {
            let last = match self.check_ready(spec, want).await {
                Ok(()) => return Ok(()),
                Err(why) => why,
            };
            if Instant::now() + self.timeouts.poll > deadline {
                return Err(EngineError::Runtime(format!(
                    "tenant {tenant}: not ready after {:?}: {last}",
                    self.timeouts.ready
                )));
            }
            tokio::time::sleep(self.timeouts.poll).await;
        }
    }

    async fn check_ready(&self, spec: &ClusterSpec, want: Replicas) -> Result<(), String> {
        let tenant = &spec.tenant;
        let ready = self
            .runtime
            .ready(tenant)
            .await
            .map_err(|e| e.to_string())?;
        for role in Role::ALL {
            if ready.of(role) < want.of(role) {
                return Err(format!(
                    "{}: {} of {} ready",
                    role.name(),
                    ready.of(role),
                    want.of(role)
                ));
            }
        }
        if want.jobmanager == 0 {
            return Ok(());
        }
        let rest = self.rest(tenant).map_err(|e| e.to_string())?;
        let overview = rest.overview().await.map_err(|e| e.to_string())?;
        if overview.taskmanagers < want.taskmanager {
            return Err(format!(
                "{} of {} TaskManagers registered",
                overview.taskmanagers, want.taskmanager
            ));
        }
        if want.sql_gateway > 0 {
            self.sql(tenant)
                .map_err(|e| e.to_string())?
                .info()
                .await
                .map_err(|e| format!("SQL Gateway: {e}"))?;
        }
        Ok(())
    }

    /// Stops every managed job that runs with a savepoint. On a failure the
    /// jobs already stopped are restarted from their savepoints and the
    /// error is returned.
    async fn stop_all(&self, spec: &ClusterSpec) -> Result<Vec<Stopped>, EngineError> {
        let tenant = &spec.tenant;
        let managed = self.catalog.managed_jobs(tenant).await?;
        let rest = self.rest(tenant)?;
        let running = rest.list_jobs().await?;
        let target = spec.storage.tenant(tenant).savepoints();
        let mut stopped: Vec<Stopped> = Vec::new();
        for job in managed {
            let live = running.iter().find(|j| {
                !j.state.is_terminal()
                    && (Some(&j.jid) == job.flink_job.as_ref() || j.name == job.name)
            });
            let Some(live) = live else {
                // Not running: it restarts from its last savepoint, if any.
                let savepoint = job.last_savepoint.clone();
                stopped.push(Stopped { job, savepoint });
                continue;
            };
            let result = async {
                let trigger = rest
                    .stop_with_savepoint(
                        &live.jid,
                        &StopRequest {
                            target_directory: Some(target.clone()),
                            drain: false,
                            format_type: SavepointFormat::Canonical,
                            trigger_id: None,
                        },
                    )
                    .await?;
                rest.wait_savepoint(
                    &live.jid,
                    &trigger,
                    self.timeouts.poll,
                    self.timeouts.savepoint,
                )
                .await
            }
            .await;
            match result {
                Ok(location) => {
                    self.catalog
                        .record_stopped(tenant, &job.name, Some(location.clone()))
                        .await?;
                    stopped.push(Stopped {
                        job,
                        savepoint: Some(location),
                    });
                }
                Err(e) => {
                    let err = EngineError::Runtime(format!(
                        "stopping {} with a savepoint: {e}",
                        job.name
                    ));
                    // Undo: the cluster is unchanged, so restart what stopped.
                    if let Err(undo) = self.restore_all(spec, &stopped).await {
                        tracing::error!(%tenant, error = %undo, "restarting stopped jobs after a failed savepoint");
                    }
                    return Err(err);
                }
            }
        }
        Ok(stopped)
    }

    /// Submits each job's SQL again, from its savepoint when it has one.
    async fn restore_all(&self, spec: &ClusterSpec, jobs: &[Stopped]) -> Result<(), EngineError> {
        let mut failures = Vec::new();
        for s in jobs {
            if let Err(e) = self.submit(spec, &s.job, s.savepoint.as_deref()).await {
                failures.push(format!("{}: {e}", s.job.name));
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(EngineError::Runtime(format!(
                "jobs not restarted: {}",
                failures.join("; ")
            )))
        }
    }

    /// Submits `job` through the SQL Gateway, restoring from `restore`.
    /// Returns its Flink job id.
    ///
    /// # Errors
    ///
    /// [`EngineError::Invalid`] for a restore path outside the tenant's
    /// prefix, else the gateway's error.
    pub async fn submit(
        &self,
        spec: &ClusterSpec,
        job: &ManagedJob,
        restore: Option<&str>,
    ) -> Result<JobId, EngineError> {
        let tenant = &spec.tenant;
        if let Some(path) = restore
            && !spec.storage.tenant(tenant).contains(path)
        {
            return Err(EngineError::Invalid(format!(
                "restore path {path} is outside the tenant's prefix"
            )));
        }
        let sql = self.sql(tenant)?;
        let session = sql
            .open_session(&OpenSession {
                session_name: Some(format!("loams-controller-{}", job.name)),
                properties: BTreeMap::from([(PIPELINE_NAME.to_owned(), job.name.clone())]),
            })
            .await?;
        let result = async {
            for stmt in &job.setup {
                sql.configure_session(&session, stmt, None).await?;
            }
            let mut exec =
                ExecuteStatement::new(job.statement.clone()).with_config(PIPELINE_NAME, &job.name);
            if let Some(path) = restore {
                exec = exec.with_config(STATE_RECOVERY_PATH, path);
            }
            sql.execute_and_collect(
                &session,
                &exec,
                FetchLimits {
                    max_rows: 16,
                    timeout: self.timeouts.submit,
                    ..FetchLimits::default()
                },
            )
            .await
        }
        .await;
        if let Err(e) = sql.close_session(&session).await {
            tracing::debug!(%tenant, error = %e, "closing the controller's session");
        }
        let collected = result?;
        // INSERT INTO answers with the job id as its one row and in jobID.
        let id = collected
            .job_id
            .clone()
            .or_else(|| {
                collected
                    .rows
                    .first()
                    .and_then(|r| r.fields.first())
                    .and_then(|v| v.as_str().map(str::to_owned))
            })
            .ok_or_else(|| {
                EngineError::Runtime(format!("{}: the statement started no job", job.name))
            })?;
        let id = JobId::new(id)?;
        self.catalog
            .record_started(tenant, &job.name, id.clone())
            .await?;
        Ok(id)
    }

    /// Stops through savepoints, applies `new_spec` and restores: the shared
    /// path of upgrade and of a scale that changes slots or memory.
    async fn redeploy(&self, old: &ClusterSpec, new_spec: &ClusterSpec) -> Result<(), EngineError> {
        let config = render(new_spec).map_err(|e| EngineError::Invalid(e.to_string()))?;
        let creds = self.creds.credentials(&new_spec.tenant).await?;
        let stopped = self.stop_all(old).await?;
        let want = Replicas::running(new_spec);
        let applied = async {
            self.runtime.apply(new_spec, &config, &creds, want).await?;
            self.wait_ready(new_spec, want).await
        }
        .await;
        if let Err(e) = applied {
            // The jobs stay stopped with their savepoints recorded; a retry
            // (or a rollback to the old spec) restores them.
            return Err(EngineError::Runtime(format!(
                "redeploying (jobs stopped with savepoints, retry to restore them): {e}"
            )));
        }
        self.restore_all(new_spec, &stopped).await?;
        self.sweep(new_spec).await;
        Ok(())
    }

    /// Updates the compose recovery record from the JobManager: each
    /// managed job's newest retained checkpoint (D1974). Errors are logged:
    /// the record is a best-effort cache of what Flink reports.
    pub async fn sweep(&self, spec: &ClusterSpec) {
        let Some(recovery) = &self.recovery else {
            return;
        };
        if let Err(e) = self.try_sweep(recovery, spec).await {
            tracing::warn!(tenant = %spec.tenant, error = %e, "recovery record sweep failed");
        }
    }

    async fn try_sweep(
        &self,
        recovery: &RecoveryStore,
        spec: &ClusterSpec,
    ) -> Result<(), EngineError> {
        let tenant = &spec.tenant;
        let managed = self.catalog.managed_jobs(tenant).await?;
        let rest = self.rest(tenant)?;
        let mut record = recovery.load(spec).await?;
        record.version = 1;
        let names: Vec<String> = managed.iter().map(|j| j.name.clone()).collect();
        record.jobs.retain(|name, _| names.contains(name));
        for job in managed {
            let mut restore_path = record
                .jobs
                .get(&job.name)
                .and_then(|e| e.restore_path.clone())
                .or_else(|| job.last_savepoint.clone());
            if let Some(id) = &job.flink_job
                && let Ok(stats) = rest.checkpoints(id).await
                && let Some(path) = stats.restore_path()
            {
                restore_path = Some(path.to_owned());
            }
            record.jobs.insert(
                job.name.clone(),
                RecoveryEntry {
                    job,
                    restore_path,
                    updated_ms: now_ms(),
                },
            );
        }
        recovery.save(spec, &record).await
    }

    /// Replays the recovery record after a JobManager restart (compose has
    /// no Flink HA): every recorded job that does not run is resubmitted
    /// from its newest checkpoint. Returns the names restarted.
    ///
    /// # Errors
    ///
    /// [`EngineError::NotFound`] without a cluster, else the first error.
    pub async fn recover(&self, tenant: &TenantId) -> Result<Vec<String>, EngineError> {
        let spec = status_spec(&self.records, tenant).await?;
        let Some(recovery) = &self.recovery else {
            return Ok(Vec::new());
        };
        let record = recovery.load(&spec).await?;
        let running = self.rest(tenant)?.list_jobs().await?;
        let mut restarted = Vec::new();
        for (name, entry) in &record.jobs {
            let alive = running
                .iter()
                .any(|j| !j.state.is_terminal() && j.name == *name);
            if alive {
                continue;
            }
            self.submit(&spec, &entry.job, entry.restore_path.as_deref())
                .await?;
            restarted.push(name.clone());
        }
        if !restarted.is_empty() {
            self.sweep(&spec).await;
        }
        Ok(restarted)
    }

    async fn scale_roles(
        &self,
        tenant: &TenantId,
        want: Replicas,
        order: [Role; 3],
    ) -> Result<(), EngineError> {
        for role in order {
            self.runtime.scale(tenant, role, want.of(role)).await?;
        }
        Ok(())
    }
}

fn endpoint(name: &str, url: &str) -> Endpoint {
    let parsed = url::Url::parse(url).ok();
    Endpoint {
        name: name.to_owned(),
        host: parsed
            .as_ref()
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_default(),
        port: parsed
            .as_ref()
            .and_then(url::Url::port_or_known_default)
            .unwrap_or_default(),
    }
}

async fn status_spec(
    records: &ClusterRecords,
    tenant: &TenantId,
) -> Result<ClusterSpec, EngineError> {
    records
        .map
        .lock()
        .await
        .get(tenant)
        .map(|r| r.spec.clone())
        .ok_or_else(|| EngineError::NotFound(tenant.to_string()))
}

#[async_trait]
impl ManagedEngine for FlinkController {
    type Spec = ClusterSpec;

    fn kind(&self) -> EngineKind {
        EngineKind::Flink
    }

    async fn create(
        &self,
        tenant: &TenantId,
        spec: ClusterSpec,
    ) -> Result<EngineStatus, EngineError> {
        if spec.tenant != *tenant {
            return Err(EngineError::Invalid(format!(
                "the spec is for tenant {}, not {tenant}",
                spec.tenant
            )));
        }
        if spec.platform != self.runtime.platform() {
            return Err(EngineError::Invalid(format!(
                "the spec is for {:?}; this controller runs {:?}",
                spec.platform,
                self.runtime.platform()
            )));
        }
        let config = render(&spec).map_err(|e| EngineError::Invalid(e.to_string()))?;
        {
            let mut map = self.records.map.lock().await;
            if let Some(existing) = map.get(tenant)
                && existing.phase != Phase::Deleted
            {
                return Err(EngineError::AlreadyExists(tenant.to_string()));
            }
            map.insert(
                tenant.clone(),
                ClusterRecord {
                    spec: spec.clone(),
                    phase: Phase::Provisioning,
                    message: None,
                },
            );
        }
        let result = async {
            let creds = self.creds.credentials(tenant).await?;
            let want = Replicas::running(&spec);
            self.runtime.apply(&spec, &config, &creds, want).await?;
            self.wait_ready(&spec, want).await
        }
        .await;
        match result {
            Ok(()) => self.finish(tenant, Phase::Running, None, None).await,
            Err(e) => Err(self.fail(tenant, "create", e).await),
        }
    }

    async fn scale(
        &self,
        tenant: &TenantId,
        size: Size,
        overrides: Option<Resources>,
    ) -> Result<EngineStatus, EngineError> {
        let resources = overrides.unwrap_or_else(|| size.resources());
        resources
            .check()
            .map_err(|e| EngineError::Invalid(e.to_string()))?;
        let old = self
            .begin(
                tenant,
                &[Phase::Running, Phase::Failed { op: String::new() }],
                Phase::Scaling,
            )
            .await?;
        let mut new_spec = old.clone();
        new_spec.size = size;
        new_spec.resources = resources;
        let count_only = Resources {
            taskmanagers: old.resources.taskmanagers,
            ..resources
        } == old.resources;
        let result = if count_only {
            async {
                self.runtime
                    .scale(tenant, Role::TaskManager, resources.taskmanagers)
                    .await?;
                self.wait_ready(&new_spec, Replicas::running(&new_spec))
                    .await
            }
            .await
        } else {
            self.redeploy(&old, &new_spec).await
        };
        match result {
            Ok(()) => {
                self.finish(tenant, Phase::Running, Some(new_spec), None)
                    .await
            }
            Err(e) => Err(self.fail(tenant, "scale", e).await),
        }
    }

    async fn upgrade(
        &self,
        tenant: &TenantId,
        version: EngineVersion,
    ) -> Result<EngineStatus, EngineError> {
        let current = status_spec(&self.records, tenant).await?;
        check_upgrade(&current.flink_version, &version.version)?;
        if version.image.is_empty() || version.image.contains(char::is_whitespace) {
            return Err(EngineError::Invalid(format!("image {:?}", version.image)));
        }
        let old = self
            .begin(
                tenant,
                &[Phase::Running, Phase::Failed { op: String::new() }],
                Phase::Upgrading,
            )
            .await?;
        let mut new_spec = old.clone();
        new_spec.image = version.image;
        new_spec.flink_version = version.version;
        match self.redeploy(&old, &new_spec).await {
            Ok(()) => {
                self.finish(tenant, Phase::Running, Some(new_spec), None)
                    .await
            }
            Err(e) => Err(self.fail(tenant, "upgrade", e).await),
        }
    }

    async fn suspend(&self, tenant: &TenantId) -> Result<EngineStatus, EngineError> {
        let spec = self
            .begin(tenant, &[Phase::Running], Phase::Suspending)
            .await?;
        let result = async {
            self.stop_all(&spec).await?;
            self.sweep(&spec).await;
            // Clients first, the JobManager last.
            self.scale_roles(
                tenant,
                Replicas::default(),
                [Role::SqlGateway, Role::TaskManager, Role::JobManager],
            )
            .await
        }
        .await;
        match result {
            Ok(()) => self.finish(tenant, Phase::Suspended, None, None).await,
            Err(e) => Err(self.fail(tenant, "suspend", e).await),
        }
    }

    async fn resume(&self, tenant: &TenantId) -> Result<EngineStatus, EngineError> {
        let spec = self
            .begin(
                tenant,
                &[Phase::Suspended, Phase::Failed { op: String::new() }],
                Phase::Resuming,
            )
            .await?;
        let result = async {
            let want = Replicas::running(&spec);
            self.scale_roles(
                tenant,
                want,
                [Role::JobManager, Role::TaskManager, Role::SqlGateway],
            )
            .await?;
            self.wait_ready(&spec, want).await?;
            let jobs: Vec<Stopped> = self
                .catalog
                .managed_jobs(tenant)
                .await?
                .into_iter()
                .map(|job| {
                    let savepoint = job.last_savepoint.clone();
                    Stopped { job, savepoint }
                })
                .collect();
            self.restore_all(&spec, &jobs).await?;
            self.sweep(&spec).await;
            Ok::<(), EngineError>(())
        }
        .await;
        match result {
            Ok(()) => self.finish(tenant, Phase::Running, None, None).await,
            Err(e) => Err(self.fail(tenant, "resume", e).await),
        }
    }

    async fn delete(&self, tenant: &TenantId, purge: bool) -> Result<EngineStatus, EngineError> {
        let spec = self
            .begin(
                tenant,
                &[
                    Phase::Running,
                    Phase::Suspended,
                    Phase::Failed { op: String::new() },
                ],
                Phase::Deleting,
            )
            .await?;
        let result = async {
            // Cancel what runs (no savepoint: the cluster goes away). A
            // suspended cluster has no JobManager to ask.
            if let Ok(rest) = self.rest(tenant)
                && let Ok(jobs) = rest.list_jobs().await
            {
                for j in jobs.iter().filter(|j| !j.state.is_terminal()) {
                    if let Err(e) = rest.cancel_job(&j.jid).await {
                        tracing::warn!(%tenant, job = %j.jid, error = %e, "cancelling before delete");
                    }
                }
            }
            match self.runtime.delete(tenant).await {
                Ok(()) | Err(RuntimeError::Unsupported(_)) if spec.platform == Platform::Compose => {}
                Ok(()) => {}
                Err(e) => return Err(EngineError::from(e)),
            }
            if purge {
                if let Some(recovery) = &self.recovery {
                    let n = recovery.purge(&spec).await?;
                    tracing::info!(%tenant, objects = n, "purged the tenant's Flink state");
                }
                self.creds.revoke(tenant).await?;
            }
            Ok::<(), EngineError>(())
        }
        .await;
        match result {
            Ok(()) => self.finish(tenant, Phase::Deleted, None, None).await,
            Err(e) => Err(self.fail(tenant, "delete", e).await),
        }
    }

    async fn status(&self, tenant: &TenantId) -> Result<EngineStatus, EngineError> {
        let map = self.records.map.lock().await;
        let rec = map
            .get(tenant)
            .ok_or_else(|| EngineError::NotFound(tenant.to_string()))?;
        Ok(self.status_of(rec))
    }

    async fn health(&self, tenant: &TenantId) -> Result<Health, EngineError> {
        let spec = status_spec(&self.records, tenant).await?;
        let want = Replicas::running(&spec);
        Ok(match self.check_ready(&spec, want).await {
            Ok(()) => Health {
                healthy: true,
                detail: format!(
                    "JobManager answers, {} TaskManagers, SQL Gateway answers",
                    want.taskmanager
                ),
            },
            Err(why) => Health {
                healthy: false,
                detail: why,
            },
        })
    }
}

/// Whether HA is Flink's own (Kubernetes) or the controller's record.
pub fn uses_recovery_record(spec: &ClusterSpec) -> bool {
    spec.ha == HaMode::ControllerRecovery
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant() -> TenantId {
        TenantId::new("t1").unwrap()
    }

    #[test]
    fn replicas_follow_the_spec() {
        let spec = ClusterSpec::new(tenant(), Platform::Kubernetes, Size::Medium);
        let r = Replicas::running(&spec);
        assert_eq!((r.jobmanager, r.taskmanager, r.sql_gateway), (2, 4, 1));
        let spec = ClusterSpec::new(tenant(), Platform::Compose, Size::Dev);
        assert_eq!(Replicas::running(&spec).jobmanager, 1);
        assert!(uses_recovery_record(&spec));
    }

    #[test]
    fn endpoints_parse_host_and_port() {
        let e = endpoint("rest", "http://flink-jobmanager:8081");
        assert_eq!((e.host.as_str(), e.port), ("flink-jobmanager", 8081));
    }

    #[tokio::test]
    async fn memory_catalog_records_stops_and_starts() {
        let c = MemoryJobCatalog::default();
        let t = tenant();
        c.put(
            &t,
            ManagedJob {
                name: "orders".into(),
                setup: vec![],
                statement: "INSERT INTO a SELECT * FROM b".into(),
                flink_job: None,
                last_savepoint: None,
            },
        )
        .await;
        let id = JobId::new("fbf9ad9e7fb4c3c3a8bb3b4e3a1f2d10").unwrap();
        c.record_started(&t, "orders", id.clone()).await.unwrap();
        assert_eq!(c.get(&t, "orders").await.unwrap().flink_job, Some(id));
        c.record_stopped(
            &t,
            "orders",
            Some("s3://flink/flink/t1/savepoints/sp-1".into()),
        )
        .await
        .unwrap();
        let job = c.get(&t, "orders").await.unwrap();
        assert_eq!(job.flink_job, None);
        assert_eq!(
            job.last_savepoint.as_deref(),
            Some("s3://flink/flink/t1/savepoints/sp-1")
        );
        assert!(c.record_stopped(&t, "nope", None).await.is_err());
    }

    #[tokio::test]
    async fn recovery_records_round_trip_and_purge_only_the_tenant() {
        let store = loams_store::Store::in_memory();
        let recovery = RecoveryStore::new(store.clone());
        let spec = ClusterSpec::new(tenant(), Platform::Compose, Size::Dev);
        assert!(recovery.load(&spec).await.unwrap().jobs.is_empty());
        let mut record = RecoveryRecord {
            version: 1,
            jobs: BTreeMap::new(),
        };
        record.jobs.insert(
            "orders".into(),
            RecoveryEntry {
                job: ManagedJob {
                    name: "orders".into(),
                    setup: vec!["SET 'table.exec.state.ttl' = '1 h'".into()],
                    statement: "INSERT INTO a SELECT * FROM b".into(),
                    flink_job: None,
                    last_savepoint: None,
                },
                restore_path: Some("s3://flink/flink/t1/checkpoints/x/chk-3".into()),
                updated_ms: 1,
            },
        );
        recovery.save(&spec, &record).await.unwrap();
        assert_eq!(recovery.load(&spec).await.unwrap(), record);
        store
            .put("flink/t10/recovery/jobs.json", Bytes::from_static(b"{}"))
            .await
            .unwrap();
        assert_eq!(recovery.purge(&spec).await.unwrap(), 1);
        assert!(store.get("flink/t10/recovery/jobs.json").await.is_ok());
    }
}
