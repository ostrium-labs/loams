//! The traits each engine crate implements (design 81, D2010).
//!
//! An adapter is stateless towards Loams: [`crate::EngineService`] owns the instance record,
//! the id, the phase machine, validation of the common fields, authorization, the HTTP facade
//! and metering. The adapter talks to its runtime (a compose profile service or the k8s
//! deploy) and reports what it sees as [`Observed`].
//!
//! | Crate | Implements |
//! |---|---|
//! | `loams-risingwave` (RW-a; RW-b `src/sql_api.rs`) | [`ManagedEngine`], [`SqlEngine`] |
//! | `loams-flink` (FL-a; FL-b `src/api.rs`) | [`ManagedEngine`], [`SqlEngine`] |
//! | `loams-spark` (SP-a; SP-b `src/jobs/*`) | [`ManagedEngine`], [`SparkEngine`] |

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::{
    Capabilities, EngineError, EngineKind, EngineSize, Instance, InstanceSpec, Observed,
    UsageSample,
};

/// One managed engine kind.
///
/// Every method gets the full stored [`Instance`]; `state_prefix` is where the runtime keeps
/// state and checkpoints in Loams object storage. Operations may return as soon as the runtime
/// accepted them, with the transitional phase in [`Observed::phase`]; the service calls
/// [`ManagedEngine::observe`] later to learn the outcome.
#[async_trait]
pub trait ManagedEngine: Send + Sync + 'static {
    fn kind(&self) -> EngineKind;

    fn capabilities(&self) -> Capabilities;

    /// Refuses a spec before anything is stored: unknown config keys, an unsupported version,
    /// a size the engine cannot run. Name, size bounds and kind are already checked.
    fn validate(&self, spec: &InstanceSpec) -> Result<(), EngineError>;

    async fn create(&self, inst: &Instance) -> Result<Observed, EngineError>;

    /// `inst.spec.size` is still the old size.
    async fn scale(&self, inst: &Instance, to: &EngineSize) -> Result<Observed, EngineError>;

    /// `inst.spec.version` is still the old version.
    async fn upgrade(&self, inst: &Instance, to_version: &str) -> Result<Observed, EngineError>;

    /// Drops compute and keeps state. Only called when `capabilities().suspend`.
    async fn suspend(&self, inst: &Instance) -> Result<Observed, EngineError>;

    async fn resume(&self, inst: &Instance) -> Result<Observed, EngineError>;

    /// Removes the runtime and the state under `state_prefix`. Idempotent: deleting what is
    /// already gone succeeds.
    async fn delete(&self, inst: &Instance) -> Result<(), EngineError>;

    /// Status, health and endpoints as the runtime sees them now.
    async fn observe(&self, inst: &Instance) -> Result<Observed, EngineError>;

    /// What the instance used over `[start, end)` (D2013). Adapters never emit usage events
    /// themselves; the service does, once per window.
    async fn usage(
        &self,
        inst: &Instance,
        start: OffsetDateTime,
        end: OffsetDateTime,
    ) -> Result<UsageSample, EngineError>;

    /// SQL, for RisingWave and Flink SQL.
    fn sql(&self) -> Option<&dyn SqlEngine> {
        None
    }

    /// Sessions and batch jobs, for Spark.
    fn spark(&self) -> Option<&dyn SparkEngine> {
        None
    }
}

/// The most rows a SQL call returns, and the default.
pub const MAX_SQL_ROWS: u32 = 10_000;
pub const DEFAULT_SQL_ROWS: u32 = 1_000;
/// The longest a SQL call may run, and the default.
pub const MAX_SQL_TIMEOUT_MS: u32 = 300_000;
pub const DEFAULT_SQL_TIMEOUT_MS: u32 = 30_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SqlRequest {
    pub sql: String,
    #[serde(default = "default_rows")]
    pub max_rows: u32,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u32,
}

fn default_rows() -> u32 {
    DEFAULT_SQL_ROWS
}

fn default_timeout() -> u32 {
    DEFAULT_SQL_TIMEOUT_MS
}

impl SqlRequest {
    pub fn new(sql: impl Into<String>) -> Self {
        Self {
            sql: sql.into(),
            max_rows: DEFAULT_SQL_ROWS,
            timeout_ms: DEFAULT_SQL_TIMEOUT_MS,
        }
    }

    pub fn validate(&self) -> Result<(), EngineError> {
        if self.sql.trim().is_empty() {
            return Err(EngineError::Invalid("sql is empty".into()));
        }
        if self.max_rows == 0 || self.max_rows > MAX_SQL_ROWS {
            return Err(EngineError::Invalid(format!(
                "max_rows must be between 1 and {MAX_SQL_ROWS}"
            )));
        }
        if self.timeout_ms == 0 || self.timeout_ms > MAX_SQL_TIMEOUT_MS {
            return Err(EngineError::Invalid(format!(
                "timeout_ms must be between 1 and {MAX_SQL_TIMEOUT_MS}"
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SqlColumn {
    pub name: String,
    /// The engine's type name (`varchar`, `BIGINT`, `TIMESTAMP(3)`).
    #[serde(rename = "type")]
    pub ty: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SqlResult {
    pub columns: Vec<SqlColumn>,
    pub rows: Vec<Vec<serde_json::Value>>,
    /// Rows the statement produced or affected, which exceeds `rows.len()` when truncated.
    pub row_count: u64,
    pub truncated: bool,
    pub elapsed_ms: u64,
    /// For a statement that starts a streaming job (a Flink `INSERT`), the job it started.
    #[serde(default)]
    pub notice: Option<String>,
}

#[async_trait]
pub trait SqlEngine: Send + Sync {
    /// Runs one statement batch, returning at most `req.max_rows` rows.
    async fn execute(&self, inst: &Instance, req: SqlRequest) -> Result<SqlResult, EngineError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SparkSessionKind {
    Sql,
    Python,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SparkSessionState {
    Starting,
    Idle,
    Busy,
    Closed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SparkSession {
    pub id: String,
    pub kind: SparkSessionKind,
    pub state: SparkSessionState,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// The Spark Connect address through loams-gateway (`sc://…`).
    #[serde(default)]
    pub connect_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSession {
    pub kind: SparkSessionKind,
    #[serde(default)]
    pub conf: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SparkJobKind {
    Sql,
    Python,
    Jar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SparkJobState {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl SparkJobState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SparkJob {
    pub id: String,
    pub name: String,
    pub kind: SparkJobKind,
    pub state: SparkJobState,
    pub entrypoint: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub submitted_at: OffsetDateTime,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub finished_at: Option<OffsetDateTime>,
    #[serde(default)]
    pub message: Option<String>,
    /// The adapter's full run status (attempts, driver, timings), for `GET …/jobs/{job}`;
    /// lists leave it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
}

/// A job to submit. The facade checks the fields named here; anything else in the body is the
/// engine's own job spec (SP-b's `SparkJobSpec` is a superset) and reaches the adapter in
/// `extra`, untouched.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitJob {
    pub name: String,
    pub kind: SparkJobKind,
    /// SQL text, or an object-storage path to a `.py` file or a `.jar`.
    pub entrypoint: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub conf: BTreeMap<String, String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl SubmitJob {
    pub fn validate(&self) -> Result<(), EngineError> {
        if self.name.trim().is_empty() || self.name.len() > 200 {
            return Err(EngineError::Invalid(
                "a job needs a name of 1-200 characters".into(),
            ));
        }
        if self.entrypoint.trim().is_empty() {
            return Err(EngineError::Invalid("a job needs an entrypoint".into()));
        }
        let path_ok = |suffix: &str| {
            (self.entrypoint.starts_with("s3://") || self.entrypoint.starts_with("s3a://"))
                && self.entrypoint.ends_with(suffix)
        };
        match self.kind {
            SparkJobKind::Sql => Ok(()),
            SparkJobKind::Python if path_ok(".py") => Ok(()),
            SparkJobKind::Jar if path_ok(".jar") => Ok(()),
            SparkJobKind::Python => Err(EngineError::Invalid(
                "a python job's entrypoint is an s3:// path to a .py file".into(),
            )),
            SparkJobKind::Jar => Err(EngineError::Invalid(
                "a jar job's entrypoint is an s3:// path to a .jar".into(),
            )),
        }
    }
}

/// The most bytes `GET …/jobs/{job}/logs` returns; adapters return the tail.
pub const MAX_LOG_BYTES: usize = 1 << 20;

/// `POST …/schedules`: run `job` on `cron` (UTC, five fields, or six with seconds).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSchedule {
    /// Chosen by the caller, so a retry is idempotent: 1-63 of `a-z`, `0-9`, `-`.
    pub id: String,
    pub cron: String,
    pub job: SubmitJob,
}

impl CreateSchedule {
    pub fn validate(&self) -> Result<(), EngineError> {
        let id_ok = !self.id.is_empty()
            && self.id.len() <= 63
            && self
                .id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && !self.id.starts_with('-')
            && !self.id.ends_with('-');
        if !id_ok {
            return Err(EngineError::Invalid(format!(
                "schedule id {:?} must be 1-63 of a-z, 0-9 and '-', not starting or ending with '-'",
                self.id
            )));
        }
        let fields = self.cron.split_whitespace().count();
        if !(5..=6).contains(&fields) {
            return Err(EngineError::Invalid(format!(
                "cron {:?} must have five fields (or six with seconds)",
                self.cron
            )));
        }
        self.job.validate()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SparkSchedule {
    pub id: String,
    pub cron: String,
    pub job: SubmitJob,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub next_run_at: Option<OffsetDateTime>,
}

#[async_trait]
pub trait SparkEngine: Send + Sync {
    async fn list_sessions(&self, inst: &Instance) -> Result<Vec<SparkSession>, EngineError>;
    async fn create_session(
        &self,
        inst: &Instance,
        req: CreateSession,
    ) -> Result<SparkSession, EngineError>;
    async fn close_session(&self, inst: &Instance, session: &str) -> Result<(), EngineError>;
    async fn list_jobs(&self, inst: &Instance) -> Result<Vec<SparkJob>, EngineError>;
    async fn submit_job(&self, inst: &Instance, req: SubmitJob) -> Result<SparkJob, EngineError>;
    async fn cancel_job(&self, inst: &Instance, job: &str) -> Result<SparkJob, EngineError>;

    /// One job with its full status in [`SparkJob::detail`]. The default finds it in
    /// [`SparkEngine::list_jobs`].
    async fn get_job(&self, inst: &Instance, job: &str) -> Result<SparkJob, EngineError> {
        self.list_jobs(inst)
            .await?
            .into_iter()
            .find(|j| j.id == job)
            .ok_or_else(|| EngineError::NotFound(format!("no job {job}")))
    }

    /// A job's driver log as text, the last [`MAX_LOG_BYTES`] at most; `attempt` picks a
    /// retry (the latest when `None`).
    async fn job_logs(
        &self,
        _inst: &Instance,
        _job: &str,
        _attempt: Option<u32>,
    ) -> Result<String, EngineError> {
        Err(EngineError::Unsupported(
            "this engine keeps no job logs".into(),
        ))
    }

    async fn list_schedules(&self, _inst: &Instance) -> Result<Vec<SparkSchedule>, EngineError> {
        Ok(Vec::new())
    }

    /// Creates the schedule, or answers the existing one when `req` is the same (a retry).
    async fn create_schedule(
        &self,
        _inst: &Instance,
        _req: CreateSchedule,
    ) -> Result<SparkSchedule, EngineError> {
        Err(EngineError::Unsupported(
            "this engine has no schedules".into(),
        ))
    }

    async fn delete_schedule(&self, _inst: &Instance, _id: &str) -> Result<(), EngineError> {
        Err(EngineError::Unsupported(
            "this engine has no schedules".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_request_defaults_and_bounds() {
        let r: SqlRequest = serde_json::from_str(r#"{"sql":"select 1"}"#).unwrap();
        assert_eq!(r.max_rows, DEFAULT_SQL_ROWS);
        assert_eq!(r.timeout_ms, DEFAULT_SQL_TIMEOUT_MS);
        assert!(r.validate().is_ok());
        assert!(SqlRequest::new("  ").validate().is_err());
        let mut big = SqlRequest::new("select 1");
        big.max_rows = MAX_SQL_ROWS + 1;
        assert!(big.validate().is_err());
        assert!(serde_json::from_str::<SqlRequest>(r#"{"sql":"x","db":"y"}"#).is_err());
    }

    #[test]
    fn job_entrypoints() {
        let job = |kind, entrypoint: &str| SubmitJob {
            name: "j".into(),
            kind,
            entrypoint: entrypoint.into(),
            args: Vec::new(),
            conf: BTreeMap::new(),
            extra: serde_json::Map::new(),
        };
        assert!(job(SparkJobKind::Sql, "select 1").validate().is_ok());
        assert!(
            job(SparkJobKind::Python, "s3://b/etl.py")
                .validate()
                .is_ok()
        );
        assert!(job(SparkJobKind::Python, "/etc/passwd").validate().is_err());
        assert!(job(SparkJobKind::Jar, "s3a://b/app.jar").validate().is_ok());
        assert!(job(SparkJobKind::Jar, "s3://b/app.py").validate().is_err());
        assert!(job(SparkJobKind::Sql, " ").validate().is_err());
    }

    #[test]
    fn sql_column_wire_name() {
        let c = SqlColumn {
            name: "n".into(),
            ty: "bigint".into(),
        };
        assert_eq!(
            serde_json::to_value(&c).unwrap(),
            serde_json::json!({"name": "n", "type": "bigint"})
        );
    }

    #[test]
    fn submit_job_keeps_the_engines_own_fields() {
        let j: SubmitJob = serde_json::from_value(serde_json::json!({
            "name": "etl", "kind": "python", "entrypoint": "s3://b/etl.py",
            "retries": 3, "resources": { "executors": 4 }
        }))
        .unwrap();
        assert!(j.validate().is_ok());
        assert_eq!(j.extra["retries"], 3);
        let back = serde_json::to_value(&j).unwrap();
        assert_eq!(back["resources"]["executors"], 4);
        assert_eq!(back["name"], "etl");
    }

    #[test]
    fn schedules_validate() {
        let job = SubmitJob {
            name: "j".into(),
            kind: SparkJobKind::Sql,
            entrypoint: "select 1".into(),
            args: Vec::new(),
            conf: BTreeMap::new(),
            extra: serde_json::Map::new(),
        };
        let s = |id: &str, cron: &str| CreateSchedule {
            id: id.into(),
            cron: cron.into(),
            job: job.clone(),
        };
        assert!(s("nightly", "0 3 * * *").validate().is_ok());
        assert!(s("every-second", "* * * * * *").validate().is_ok());
        assert!(s("Nightly", "0 3 * * *").validate().is_err());
        assert!(s("-x", "0 3 * * *").validate().is_err());
        assert!(s("x", "0 3 * *").validate().is_err());
        assert!(s("x", "").validate().is_err());
    }
}
