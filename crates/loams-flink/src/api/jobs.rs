//! Saved SQL jobs: long-running `INSERT INTO …` pipelines with a lifecycle,
//! stop-with-savepoint and resume (D1988, ENG81 R21).
//!
//! A job is stored as SQL text with secret references (never values, D1985)
//! plus the Flink job id and the last savepoint. Starting it opens a session,
//! runs the bootstrap and the job's own setup, and submits the statement;
//! stopping runs `STOP JOB '<id>' WITH SAVEPOINT`; resuming re-runs the
//! bootstrap and the statement with `execution.state-recovery.path` set to
//! that savepoint.
//!
//! ```text
//! Draft ──start──▶ Starting ──▶ Running ──stop──▶ Stopping ──▶ Stopped
//!                     │            ▲  │                           │
//!                     ▼            │  └─(refresh)─▶ Failed/Finished/Canceled
//!                   Failed         └──── Resuming ◀──resume───────┘
//! ```
//!
//! One operation at a time: each transition is a compare-and-set on the
//! job's `version`, so a second operation racing the first gets `409`.

use std::collections::HashMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use super::run::{Collected, RunLimits, open_bootstrapped, run_to_end};
use super::split::{first_keyword, split_statements};
use super::{ApiError, FlinkTarget, SecretResolver};
use crate::catalog::SessionSpec;
use crate::catalog::quote::{SetOption, quote_literal};

/// Where a job is in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Draft,
    Starting,
    Running,
    Stopping,
    Stopped,
    Resuming,
    Failed,
    Finished,
    Canceled,
}

impl JobState {
    /// An operation is in flight.
    pub fn is_transitional(self) -> bool {
        matches!(self, Self::Starting | Self::Stopping | Self::Resuming)
    }
}

/// A saved job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedJob {
    /// `fj_<32 hex>`.
    pub id: String,
    pub namespace: String,
    /// The engine instance (`eng_<26>`).
    pub instance: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// One `INSERT INTO …` or `EXECUTE STATEMENT SET BEGIN … END`.
    pub statement: String,
    /// DDL run after the bootstrap (views, temporary functions, lake tables).
    #[serde(default)]
    pub setup: Vec<String>,
    /// The job's own catalogs, tables and settings, on top of the
    /// instance's bootstrap. Secret references only.
    #[serde(default)]
    pub session: SessionSpec,
    pub state: JobState,
    #[serde(default)]
    pub flink_job_id: Option<String>,
    /// The last savepoint, which `resume` restores.
    #[serde(default)]
    pub savepoint: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    pub version: u64,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
}

/// The body of `POST …/jobs`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewJob {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub statement: String,
    #[serde(default)]
    pub setup: Vec<String>,
    #[serde(default)]
    pub session: SessionSpec,
}

fn now() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// Check a new job: a plain name, one pipeline statement, DDL-only setup,
/// a session spec that renders and holds no inline secret.
pub fn validate_new(job: &NewJob) -> Result<(), ApiError> {
    let name_ok = !job.name.is_empty()
        && job.name.len() <= 100
        && job
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '-' | '.'));
    if !name_ok {
        return Err(ApiError::invalid(
            "job name must be 1-100 of [A-Za-z0-9 _.-]",
        ));
    }
    let (stmts, rest) =
        split_statements(&format!("{};", job.statement.trim().trim_end_matches(';')));
    if stmts.len() != 1 || !rest.trim().is_empty() {
        return Err(ApiError::invalid(
            "a job is exactly one INSERT or EXECUTE STATEMENT SET statement",
        ));
    }
    match first_keyword(&stmts[0]).as_deref() {
        Some("INSERT") | Some("EXECUTE") | Some("BEGIN") => {}
        _ => {
            return Err(ApiError::invalid(
                "a job's statement must be INSERT INTO … or EXECUTE STATEMENT SET BEGIN … END",
            ));
        }
    }
    for s in &job.setup {
        match first_keyword(s).as_deref() {
            Some("CREATE") | Some("USE") | Some("SET") | Some("ALTER") | Some("LOAD") => {}
            _ => {
                return Err(ApiError::invalid(
                    "job setup takes CREATE, ALTER, USE, LOAD and SET statements only",
                ));
            }
        }
    }
    let script = job.session.script()?;
    if script.has_inline_secret() {
        return Err(ApiError::invalid(
            "saved jobs keep secret references only: use {\"ref\": \"<name>\"}, not inline values",
        ));
    }
    Ok(())
}

impl SavedJob {
    pub fn new(namespace: &str, instance: &str, created_by: &str, new: NewJob) -> Self {
        let t = now();
        Self {
            id: format!("fj_{}", uuid::Uuid::new_v4().simple()),
            namespace: namespace.to_string(),
            instance: instance.to_string(),
            name: new.name,
            description: new.description,
            statement: new
                .statement
                .trim()
                .trim_end_matches(';')
                .trim()
                .to_string(),
            setup: new.setup,
            session: new.session,
            state: JobState::Draft,
            flink_job_id: None,
            savepoint: None,
            message: None,
            version: 0,
            created_by: created_by.to_string(),
            created_at: t.clone(),
            updated_at: t,
        }
    }

    /// The `pipeline.name` Flink shows for the job.
    pub fn pipeline_name(&self) -> String {
        format!("loams:{}:{}", self.id, self.name)
    }
}

/// A lifecycle operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobOp {
    Start,
    Stop,
    Resume,
    Delete,
}

/// The state an operation moves a job into first (`None` for delete, which
/// removes it), or why it may not run now.
pub fn begin(job: &SavedJob, op: JobOp) -> Result<Option<JobState>, ApiError> {
    use JobState::*;
    let s = job.state;
    let conflict = |what: &str| {
        ApiError::conflict(format!("job {} is {s:?}: cannot {what}", job.id).to_lowercase())
    };
    match op {
        JobOp::Start => match s {
            Draft | Stopped | Failed | Finished | Canceled => Ok(Some(Starting)),
            _ => Err(conflict("start")),
        },
        JobOp::Stop => match s {
            Running if job.flink_job_id.is_some() => Ok(Some(Stopping)),
            _ => Err(conflict("stop")),
        },
        JobOp::Resume => match s {
            Stopped | Failed if job.savepoint.is_some() => Ok(Some(Resuming)),
            Stopped | Failed => Err(ApiError::conflict(format!(
                "job {} has no savepoint to resume from: start it instead",
                job.id
            ))),
            _ => Err(conflict("resume")),
        },
        JobOp::Delete => match s {
            Starting | Running | Stopping | Resuming => Err(conflict("delete (stop it first)")),
            _ => Ok(None),
        },
    }
}

/// Flink's status of a job (`SHOW JOBS`) as a job state; `None` when it
/// still counts as running.
pub fn state_from_flink(status: &str) -> Option<JobState> {
    match status.to_ascii_uppercase().as_str() {
        "FAILED" | "FAILING" => Some(JobState::Failed),
        "CANCELED" | "CANCELLING" => Some(JobState::Canceled),
        "FINISHED" => Some(JobState::Finished),
        _ => None,
    }
}

/// Why the store refused a write.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("job {0} exists")]
    Exists(String),
    #[error("job {0} not found")]
    NotFound(String),
    #[error("job {0} changed since it was read")]
    Conflict(String),
    #[error("job store: {0}")]
    Backend(String),
}

/// Where saved jobs live. The Postgres store (Loams Postgres, as the
/// engines facade's instance store) is a follow-up; [`MemoryJobStore`] serves
/// compose and tests.
#[async_trait]
pub trait JobStore: Send + Sync + 'static {
    async fn insert(&self, job: SavedJob) -> Result<SavedJob, StoreError>;
    async fn get(&self, namespace: &str, id: &str) -> Result<Option<SavedJob>, StoreError>;
    async fn list(&self, namespace: &str, instance: &str) -> Result<Vec<SavedJob>, StoreError>;
    /// Write `job` if the stored version is still `job.version`; the stored
    /// copy gets `version + 1`, which is returned.
    async fn update(&self, job: SavedJob) -> Result<SavedJob, StoreError>;
    async fn delete(&self, namespace: &str, id: &str) -> Result<bool, StoreError>;
}

/// Jobs in memory.
#[derive(Debug, Default)]
pub struct MemoryJobStore {
    jobs: Mutex<HashMap<(String, String), SavedJob>>,
}

#[async_trait]
impl JobStore for MemoryJobStore {
    async fn insert(&self, job: SavedJob) -> Result<SavedJob, StoreError> {
        let mut jobs = self.jobs.lock().await;
        let key = (job.namespace.clone(), job.id.clone());
        if jobs.contains_key(&key) {
            return Err(StoreError::Exists(job.id));
        }
        jobs.insert(key, job.clone());
        Ok(job)
    }

    async fn get(&self, namespace: &str, id: &str) -> Result<Option<SavedJob>, StoreError> {
        let jobs = self.jobs.lock().await;
        Ok(jobs.get(&(namespace.to_string(), id.to_string())).cloned())
    }

    async fn list(&self, namespace: &str, instance: &str) -> Result<Vec<SavedJob>, StoreError> {
        let jobs = self.jobs.lock().await;
        let mut out: Vec<SavedJob> = jobs
            .values()
            .filter(|j| j.namespace == namespace && j.instance == instance)
            .cloned()
            .collect();
        out.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        Ok(out)
    }

    async fn update(&self, mut job: SavedJob) -> Result<SavedJob, StoreError> {
        let mut jobs = self.jobs.lock().await;
        let key = (job.namespace.clone(), job.id.clone());
        let Some(current) = jobs.get(&key) else {
            return Err(StoreError::NotFound(job.id));
        };
        if current.version != job.version {
            return Err(StoreError::Conflict(job.id));
        }
        job.version += 1;
        job.updated_at = now();
        jobs.insert(key, job.clone());
        Ok(job)
    }

    async fn delete(&self, namespace: &str, id: &str) -> Result<bool, StoreError> {
        let mut jobs = self.jobs.lock().await;
        Ok(jobs
            .remove(&(namespace.to_string(), id.to_string()))
            .is_some())
    }
}

/// What lifecycle operations need.
#[derive(Clone, Copy)]
pub struct JobCtx<'a> {
    pub store: &'a dyn JobStore,
    pub target: &'a FlinkTarget,
    pub secrets: &'a dyn SecretResolver,
    pub limits: &'a RunLimits,
}

impl std::fmt::Debug for JobCtx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobCtx")
            .field("limits", self.limits)
            .finish_non_exhaustive()
    }
}

async fn load(ctx: &JobCtx<'_>, namespace: &str, id: &str) -> Result<SavedJob, ApiError> {
    ctx.store
        .get(namespace, id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("job {id} not found")))
}

/// A Flink job id is 32 hex digits.
fn check_flink_job_id(id: &str) -> Result<(), ApiError> {
    if id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(ApiError::internal(format!("{id:?} is not a Flink job id")))
    }
}

/// The settings-only session that `STOP JOB` and `SHOW JOBS` need: no
/// catalogs, no tables, no secrets.
fn control_spec(target: &FlinkTarget) -> SessionSpec {
    SessionSpec {
        settings: target.session.settings.clone(),
        checkpoint_interval: target.session.checkpoint_interval.clone(),
        savepoint_dir: target.session.savepoint_dir.clone(),
        ..SessionSpec::default()
    }
}

async fn control_session(
    ctx: &JobCtx<'_>,
    job: &SavedJob,
) -> Result<crate::api::backend::SessionHandle, ApiError> {
    let bare = FlinkTarget {
        backend: ctx.target.backend.clone(),
        session: control_spec(ctx.target),
    };
    open_bootstrapped(
        &bare,
        ctx.secrets,
        &job.namespace,
        &format!("loams-control-{}", job.id),
        &SessionSpec::default(),
        &[],
        ctx.limits,
    )
    .await
}

/// Submit the job's statement in a fresh bootstrapped session, restoring
/// from `savepoint` when given. Returns the Flink job id.
async fn submit(
    ctx: &JobCtx<'_>,
    job: &SavedJob,
    savepoint: Option<&str>,
) -> Result<String, ApiError> {
    let session = open_bootstrapped(
        ctx.target,
        ctx.secrets,
        &job.namespace,
        &format!("loams-job-{}", job.id),
        &job.session,
        &job.setup,
        ctx.limits,
    )
    .await?;
    let backend = ctx.target.backend.as_ref();
    let result = async {
        let mut sets = vec![SetOption::new("pipeline.name", job.pipeline_name())];
        if let Some(sp) = savepoint {
            sets.push(SetOption::new("execution.state-recovery.path", sp));
        }
        for s in &sets {
            let sql = s.render(&crate::catalog::RenderCtx::redacted())?;
            run_to_end(backend, &session, &sql, ctx.limits).await?;
        }
        let out: Collected = run_to_end(backend, &session, &job.statement, ctx.limits).await?;
        // The gateway reports the submitted job as `jobID`, and the
        // statement's single result row holds it too.
        let id = out
            .job_id
            .clone()
            .or_else(|| out.first_text())
            .ok_or_else(|| ApiError::statement("Flink returned no job id for the statement"))?;
        check_flink_job_id(&id)?;
        Ok::<String, ApiError>(id)
    }
    .await;
    // The session cluster keeps the job running after the session closes.
    let _ = backend.close_session(&session).await;
    result
}

/// Move `job` from `begin`'s transitional state to its outcome.
async fn finish(ctx: &JobCtx<'_>, job: SavedJob) -> Result<SavedJob, ApiError> {
    Ok(ctx.store.update(job).await?)
}

/// `create`: store a new job in `Draft`.
pub async fn create(
    ctx: &JobCtx<'_>,
    namespace: &str,
    instance: &str,
    principal: &str,
    new: NewJob,
) -> Result<SavedJob, ApiError> {
    validate_new(&new)?;
    let job = SavedJob::new(namespace, instance, principal, new);
    Ok(ctx.store.insert(job).await?)
}

/// `start` (fresh state) or `resume` (from the last savepoint).
pub async fn start(
    ctx: &JobCtx<'_>,
    namespace: &str,
    id: &str,
    resume: bool,
) -> Result<SavedJob, ApiError> {
    let job = load(ctx, namespace, id).await?;
    let op = if resume { JobOp::Resume } else { JobOp::Start };
    let Some(next) = begin(&job, op)? else {
        return Err(ApiError::internal("start has a next state"));
    };
    let mut j = job;
    j.state = next;
    j.message = None;
    let mut j = ctx.store.update(j).await?;
    let savepoint = if resume { j.savepoint.clone() } else { None };
    match submit(ctx, &j, savepoint.as_deref()).await {
        Ok(flink_id) => {
            j.state = JobState::Running;
            j.flink_job_id = Some(flink_id);
        }
        Err(e) => {
            j.state = JobState::Failed;
            j.flink_job_id = None;
            j.message = Some(e.message);
        }
    }
    finish(ctx, j).await
}

/// `stop`: with a savepoint (the default) the job can be resumed; `drain`
/// flushes event-time windows first (use it only for a final stop).
pub async fn stop(
    ctx: &JobCtx<'_>,
    namespace: &str,
    id: &str,
    savepoint: bool,
    drain: bool,
) -> Result<SavedJob, ApiError> {
    let job = load(ctx, namespace, id).await?;
    let Some(next) = begin(&job, JobOp::Stop)? else {
        return Err(ApiError::internal("stop has a next state"));
    };
    let flink_id = job.flink_job_id.clone().unwrap_or_default();
    check_flink_job_id(&flink_id)?;
    let mut j = job;
    j.state = next;
    j.message = None;
    let mut j = ctx.store.update(j).await?;
    let mut sql = format!("STOP JOB {}", quote_literal(&flink_id, "job id")?);
    if savepoint {
        sql.push_str(" WITH SAVEPOINT");
    }
    if drain {
        sql.push_str(" WITH DRAIN");
    }
    let outcome = async {
        let session = control_session(ctx, &j).await?;
        let backend = ctx.target.backend.as_ref();
        let r = run_to_end(backend, &session, &sql, ctx.limits).await;
        let _ = backend.close_session(&session).await;
        let out = r?;
        if savepoint {
            out.first_text()
                .map(Some)
                .ok_or_else(|| ApiError::statement("STOP JOB returned no savepoint path"))
        } else {
            Ok(None)
        }
    }
    .await;
    match outcome {
        Ok(path) => {
            j.state = JobState::Stopped;
            j.flink_job_id = None;
            // A stop without a savepoint leaves nothing to resume from: an
            // older savepoint would rewind the pipeline.
            j.savepoint = path;
        }
        Err(e) => {
            // The job is most likely still running; say why the stop failed.
            j.state = JobState::Running;
            j.message = Some(format!("stop failed: {}", e.message));
        }
    }
    finish(ctx, j).await
}

/// Ask Flink how a running job is doing (`SHOW JOBS`) and record a failure,
/// cancellation or finish. Other states are returned as stored.
pub async fn refresh(ctx: &JobCtx<'_>, namespace: &str, id: &str) -> Result<SavedJob, ApiError> {
    let job = load(ctx, namespace, id).await?;
    let (JobState::Running, Some(flink_id)) = (job.state, job.flink_job_id.clone()) else {
        return Ok(job);
    };
    let session = control_session(ctx, &job).await?;
    let backend = ctx.target.backend.as_ref();
    let r = run_to_end(backend, &session, "SHOW JOBS", ctx.limits).await;
    let _ = backend.close_session(&session).await;
    let out = r?;
    let status = out.rows.iter().find_map(|row| {
        let jid = out
            .field(row, "job id")
            .or_else(|| row.fields.first())
            .and_then(|v| v.as_str())?;
        if jid != flink_id {
            return None;
        }
        out.field(row, "status")
            .or_else(|| row.fields.get(2))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    });
    let next = match status {
        // Flink no longer lists it (a restarted session cluster without HA).
        None => Some(JobState::Failed),
        Some(s) => state_from_flink(&s),
    };
    let Some(next) = next else {
        return Ok(job);
    };
    let mut j = job;
    j.message = Some(match next {
        JobState::Failed => {
            "Flink reports the job failed or no longer lists it; resume from the last savepoint"
                .to_string()
        }
        other => format!("Flink reports the job {other:?}").to_lowercase(),
    });
    j.state = next;
    j.flink_job_id = None;
    finish(ctx, j).await
}

/// `delete`: only a job that is not running.
pub async fn delete(ctx: &JobCtx<'_>, namespace: &str, id: &str) -> Result<(), ApiError> {
    let job = load(ctx, namespace, id).await?;
    begin(&job, JobOp::Delete)?;
    ctx.store.delete(namespace, id).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(state: JobState) -> SavedJob {
        let mut j = SavedJob::new(
            "ns1",
            "eng_x",
            "user:a",
            NewJob {
                name: "clicks per minute".into(),
                description: None,
                statement: "INSERT INTO loams.analytics.t SELECT 1;".into(),
                setup: vec![],
                session: SessionSpec::default(),
            },
        );
        j.state = state;
        j
    }

    #[test]
    fn transitions() {
        use JobState::*;
        assert_eq!(
            begin(&job(Draft), JobOp::Start).ok().flatten(),
            Some(Starting)
        );
        assert!(begin(&job(Running), JobOp::Start).is_err());
        let mut r = job(Running);
        assert!(begin(&r, JobOp::Stop).is_err(), "no flink job id");
        r.flink_job_id = Some("0".repeat(32));
        assert_eq!(begin(&r, JobOp::Stop).ok().flatten(), Some(Stopping));
        let mut s = job(Stopped);
        assert!(begin(&s, JobOp::Resume).is_err(), "no savepoint");
        s.savepoint = Some("s3://b/sp-1".into());
        assert_eq!(begin(&s, JobOp::Resume).ok().flatten(), Some(Resuming));
        assert!(begin(&job(Running), JobOp::Delete).is_err());
        assert_eq!(begin(&job(Stopped), JobOp::Delete).ok(), Some(None));
        assert!(begin(&job(Stopping), JobOp::Stop).is_err());
    }

    #[test]
    fn statement_is_trimmed_and_validated() {
        let j = job(JobState::Draft);
        assert_eq!(j.statement, "INSERT INTO loams.analytics.t SELECT 1");
        let bad = |statement: &str| NewJob {
            name: "x".into(),
            description: None,
            statement: statement.into(),
            setup: vec![],
            session: SessionSpec::default(),
        };
        assert!(validate_new(&bad("SELECT 1")).is_err());
        assert!(validate_new(&bad("INSERT INTO a SELECT 1; INSERT INTO b SELECT 2")).is_err());
        assert!(validate_new(&bad("INSERT INTO a SELECT 1;")).is_ok());
        assert!(
            validate_new(&bad(
                "EXECUTE STATEMENT SET BEGIN INSERT INTO a SELECT 1; INSERT INTO b SELECT 2; END"
            ))
            .is_ok()
        );
        let mut n = bad("INSERT INTO a SELECT 1");
        n.setup = vec!["DROP TABLE x".into()];
        assert!(validate_new(&n).is_err());
        n.name = "bad/name".into();
        n.setup.clear();
        assert!(validate_new(&n).is_err());
    }

    #[test]
    fn inline_secrets_are_refused() {
        use crate::catalog::JdbcCatalog;
        use crate::catalog::quote::Secret;
        let mut n = NewJob {
            name: "x".into(),
            description: None,
            statement: "INSERT INTO a SELECT 1".into(),
            setup: vec![],
            session: SessionSpec::default(),
        };
        n.session.postgres.push(JdbcCatalog {
            name: "loams_pg".into(),
            host: "compute1".into(),
            port: 5432,
            database: "app".into(),
            username: "u".into(),
            password: Secret::inline("hunter2"),
            extra: Default::default(),
        });
        assert!(validate_new(&n).is_err());
        n.session.postgres[0].password = Secret::reference("pg");
        assert!(validate_new(&n).is_ok());
    }

    #[test]
    fn flink_states() {
        assert_eq!(state_from_flink("FAILED"), Some(JobState::Failed));
        assert_eq!(state_from_flink("finished"), Some(JobState::Finished));
        assert_eq!(state_from_flink("RUNNING"), None);
        assert_eq!(state_from_flink("RESTARTING"), None);
    }

    #[tokio::test]
    async fn memory_store_compare_and_set() {
        let store = MemoryJobStore::default();
        let j = store
            .insert(job(JobState::Draft))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let mut a = j.clone();
        a.state = JobState::Starting;
        let a = store.update(a).await.unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(a.version, 1);
        let mut stale = j;
        stale.state = JobState::Starting;
        assert!(matches!(
            store.update(stale).await,
            Err(StoreError::Conflict(_))
        ));
        assert_eq!(
            store.list("ns1", "eng_x").await.map(|v| v.len()).ok(),
            Some(1)
        );
        assert_eq!(store.delete("ns1", &a.id).await.ok(), Some(true));
    }
}
