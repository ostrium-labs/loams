//! [`FakeEngine`]: an in-memory adapter for any kind, for tests of this crate and of hosts
//! that mount the facade, and for the console without engines. It runs nothing.

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::json;
use time::OffsetDateTime;

use crate::{
    Capabilities, CreateSession, Endpoint, EndpointProtocol, EngineError, EngineKind, EngineSize,
    HealthCheck, HealthState, Instance, InstanceSpec, ManagedEngine, Observed, Phase, SparkEngine,
    SparkJob, SparkJobState, SparkSession, SparkSessionState, SqlColumn, SqlDialect, SqlEngine,
    SqlRequest, SqlResult, Stability, SubmitJob, UsageSample,
};

/// State bytes the fake reports for every instance: 1 GiB.
pub const FAKE_STATE_BYTES: u64 = 1 << 30;

#[derive(Debug, Default)]
struct FakeState {
    calls: Vec<String>,
    fail_next: Option<EngineError>,
    sessions: BTreeMap<String, Vec<SparkSession>>,
    jobs: BTreeMap<String, Vec<SparkJob>>,
    seq: u64,
}

/// An adapter that answers every operation at once, as if the runtime finished it.
#[derive(Debug)]
pub struct FakeEngine {
    kind: EngineKind,
    suspend: bool,
    state: Mutex<FakeState>,
}

impl FakeEngine {
    pub fn new(kind: EngineKind) -> Self {
        Self {
            kind,
            suspend: true,
            state: Mutex::new(FakeState::default()),
        }
    }

    /// A fake whose engine cannot suspend.
    #[must_use]
    pub fn without_suspend(mut self) -> Self {
        self.suspend = false;
        self
    }

    /// The next adapter call fails with `e`.
    pub fn fail_next(&self, e: EngineError) {
        if let Ok(mut s) = self.state.lock() {
            s.fail_next = Some(e);
        }
    }

    /// `"<op> <instance id>"` per call, in order.
    pub fn calls(&self) -> Vec<String> {
        self.state
            .lock()
            .map(|s| s.calls.clone())
            .unwrap_or_default()
    }

    fn record(&self, op: &str, inst: &Instance) -> Result<u64, EngineError> {
        let mut s = self
            .state
            .lock()
            .map_err(|_| EngineError::Internal("fake lock poisoned".into()))?;
        s.calls.push(format!("{op} {}", inst.id));
        if let Some(e) = s.fail_next.take() {
            return Err(e);
        }
        s.seq += 1;
        Ok(s.seq)
    }

    pub fn versions(kind: EngineKind) -> (&'static [&'static str], &'static str) {
        match kind {
            EngineKind::Risingwave => (&["2.6", "2.5"][..], "2.6"),
            EngineKind::Flink => (&["2.1", "1.20"][..], "2.1"),
            EngineKind::Spark => (&["4.0", "3.5"][..], "4.0"),
        }
    }

    fn endpoints(&self, inst: &Instance) -> Vec<Endpoint> {
        let host = format!("{}.engines.loams.internal", inst.id);
        let ui = Endpoint {
            name: "dashboard".into(),
            protocol: EndpointProtocol::WebUi,
            url: format!("https://{host}/ui/"),
            internal: false,
        };
        let main = match self.kind {
            EngineKind::Risingwave => Endpoint {
                name: "sql".into(),
                protocol: EndpointProtocol::Postgres,
                url: format!("postgres://{host}:5432/dev"),
                internal: false,
            },
            EngineKind::Flink => Endpoint {
                name: "sql-gateway".into(),
                protocol: EndpointProtocol::FlinkSqlGateway,
                url: format!("https://{host}/sql-gateway/v1"),
                internal: false,
            },
            EngineKind::Spark => Endpoint {
                name: "connect".into(),
                protocol: EndpointProtocol::SparkConnect,
                url: format!("sc://{host}:443/;use_ssl=true"),
                internal: false,
            },
        };
        vec![main, ui]
    }

    fn running(&self, inst: &Instance, version: Option<&str>) -> Observed {
        let (_, default) = Self::versions(self.kind);
        Observed {
            phase: Phase::Running,
            health: HealthState::Healthy,
            checks: vec![HealthCheck {
                name: "runtime".into(),
                state: HealthState::Healthy,
                message: None,
            }],
            version: Some(
                version
                    .or(inst.spec.version.as_deref())
                    .unwrap_or(default)
                    .to_owned(),
            ),
            message: None,
            endpoints: self.endpoints(inst),
        }
    }
}

#[async_trait]
impl ManagedEngine for FakeEngine {
    fn kind(&self) -> EngineKind {
        self.kind
    }

    fn capabilities(&self) -> Capabilities {
        let (versions, default) = Self::versions(self.kind);
        Capabilities {
            versions: versions.iter().map(|v| (*v).to_owned()).collect(),
            default_version: default.to_owned(),
            sql_dialect: match self.kind {
                EngineKind::Risingwave => Some(SqlDialect::Postgres),
                EngineKind::Flink => Some(SqlDialect::FlinkSql),
                EngineKind::Spark => Some(SqlDialect::SparkSql),
            },
            sessions: self.kind == EngineKind::Spark,
            jobs: self.kind == EngineKind::Spark,
            suspend: self.suspend,
            stability: Stability::Preview,
        }
    }

    fn validate(&self, spec: &InstanceSpec) -> Result<(), EngineError> {
        if let Some(k) = spec.config.keys().find(|k| !k.starts_with("fake.")) {
            return Err(EngineError::Invalid(format!(
                "unknown config key {k:?} (the fake accepts fake.*)"
            )));
        }
        Ok(())
    }

    async fn create(&self, inst: &Instance) -> Result<Observed, EngineError> {
        self.record("create", inst)?;
        Ok(self.running(inst, None))
    }

    async fn scale(&self, inst: &Instance, _to: &EngineSize) -> Result<Observed, EngineError> {
        self.record("scale", inst)?;
        if inst.status.phase == Phase::Suspended {
            return Ok(Observed::phase(Phase::Suspended));
        }
        Ok(self.running(inst, None))
    }

    async fn upgrade(&self, inst: &Instance, to_version: &str) -> Result<Observed, EngineError> {
        self.record("upgrade", inst)?;
        if inst.status.phase == Phase::Suspended {
            let mut o = Observed::phase(Phase::Suspended);
            o.version = Some(to_version.to_owned());
            return Ok(o);
        }
        Ok(self.running(inst, Some(to_version)))
    }

    async fn suspend(&self, inst: &Instance) -> Result<Observed, EngineError> {
        self.record("suspend", inst)?;
        Ok(Observed::phase(Phase::Suspended))
    }

    async fn resume(&self, inst: &Instance) -> Result<Observed, EngineError> {
        self.record("resume", inst)?;
        Ok(self.running(inst, None))
    }

    async fn delete(&self, inst: &Instance) -> Result<(), EngineError> {
        self.record("delete", inst)?;
        if let Ok(mut s) = self.state.lock() {
            s.sessions.remove(inst.id.as_str());
            s.jobs.remove(inst.id.as_str());
        }
        Ok(())
    }

    async fn observe(&self, inst: &Instance) -> Result<Observed, EngineError> {
        self.record("observe", inst)?;
        Ok(match inst.status.phase {
            Phase::Suspending | Phase::Suspended => Observed::phase(Phase::Suspended),
            Phase::Failed => Observed::phase(Phase::Failed),
            Phase::Deleting | Phase::Deleted => Observed::phase(Phase::Deleted),
            _ => self.running(inst, None),
        })
    }

    async fn usage(
        &self,
        inst: &Instance,
        start: OffsetDateTime,
        end: OffsetDateTime,
    ) -> Result<UsageSample, EngineError> {
        Ok(UsageSample::provisioned(
            &inst.spec.size,
            start,
            end,
            FAKE_STATE_BYTES,
        ))
    }

    fn sql(&self) -> Option<&dyn SqlEngine> {
        (self.kind != EngineKind::Spark).then_some(self as &dyn SqlEngine)
    }

    fn spark(&self) -> Option<&dyn SparkEngine> {
        (self.kind == EngineKind::Spark).then_some(self as &dyn SparkEngine)
    }
}

#[async_trait]
impl SqlEngine for FakeEngine {
    /// Echoes the statement: one row with the statement text and its length. A statement that
    /// contains `fail` errors the way an engine does, with a position.
    async fn execute(&self, inst: &Instance, req: SqlRequest) -> Result<SqlResult, EngineError> {
        self.record("sql", inst)?;
        if let Some(at) = req.sql.find("fail") {
            return Err(EngineError::Invalid(format!(
                "syntax error at or near \"fail\" (position {})",
                at + 1
            )));
        }
        let notice = (self.kind == EngineKind::Flink
            && req
                .sql
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("insert"))
        .then(|| format!("job submitted: fake-{}", inst.id));
        Ok(SqlResult {
            columns: vec![
                SqlColumn {
                    name: "statement".into(),
                    ty: "varchar".into(),
                },
                SqlColumn {
                    name: "length".into(),
                    ty: "bigint".into(),
                },
            ],
            rows: vec![vec![json!(req.sql), json!(req.sql.len())]],
            row_count: 1,
            truncated: false,
            elapsed_ms: 1,
            notice,
        })
    }
}

#[async_trait]
impl SparkEngine for FakeEngine {
    async fn list_sessions(&self, inst: &Instance) -> Result<Vec<SparkSession>, EngineError> {
        self.record("list_sessions", inst)?;
        Ok(self
            .state
            .lock()
            .map(|s| {
                s.sessions
                    .get(inst.id.as_str())
                    .cloned()
                    .unwrap_or_default()
            })
            .unwrap_or_default())
    }

    async fn create_session(
        &self,
        inst: &Instance,
        req: CreateSession,
    ) -> Result<SparkSession, EngineError> {
        let n = self.record("create_session", inst)?;
        let session = SparkSession {
            id: format!("ses_{n}"),
            kind: req.kind,
            state: SparkSessionState::Idle,
            created_at: OffsetDateTime::now_utc(),
            connect_url: Some(format!(
                "sc://{}.engines.loams.internal:443/;session_id=ses_{n}",
                inst.id
            )),
        };
        if let Ok(mut s) = self.state.lock() {
            s.sessions
                .entry(inst.id.0.clone())
                .or_default()
                .push(session.clone());
        }
        Ok(session)
    }

    async fn close_session(&self, inst: &Instance, session: &str) -> Result<(), EngineError> {
        self.record("close_session", inst)?;
        let mut s = self
            .state
            .lock()
            .map_err(|_| EngineError::Internal("fake lock poisoned".into()))?;
        let list = s.sessions.entry(inst.id.0.clone()).or_default();
        let before = list.len();
        list.retain(|x| x.id != session);
        if list.len() == before {
            return Err(EngineError::NotFound(format!("no session {session}")));
        }
        Ok(())
    }

    async fn list_jobs(&self, inst: &Instance) -> Result<Vec<SparkJob>, EngineError> {
        self.record("list_jobs", inst)?;
        Ok(self
            .state
            .lock()
            .map(|s| s.jobs.get(inst.id.as_str()).cloned().unwrap_or_default())
            .unwrap_or_default())
    }

    async fn submit_job(&self, inst: &Instance, req: SubmitJob) -> Result<SparkJob, EngineError> {
        let n = self.record("submit_job", inst)?;
        let job = SparkJob {
            id: format!("job_{n}"),
            name: req.name,
            kind: req.kind,
            state: SparkJobState::Queued,
            entrypoint: req.entrypoint,
            args: req.args,
            submitted_at: OffsetDateTime::now_utc(),
            finished_at: None,
            message: None,
        };
        if let Ok(mut s) = self.state.lock() {
            s.jobs
                .entry(inst.id.0.clone())
                .or_default()
                .push(job.clone());
        }
        Ok(job)
    }

    async fn cancel_job(&self, inst: &Instance, job: &str) -> Result<SparkJob, EngineError> {
        self.record("cancel_job", inst)?;
        let mut s = self
            .state
            .lock()
            .map_err(|_| EngineError::Internal("fake lock poisoned".into()))?;
        let Some(j) = s
            .jobs
            .get_mut(inst.id.as_str())
            .and_then(|jobs| jobs.iter_mut().find(|j| j.id == job))
        else {
            return Err(EngineError::NotFound(format!("no job {job}")));
        };
        if j.state.is_terminal() {
            return Err(EngineError::FailedPrecondition(format!(
                "job {job} already finished"
            )));
        }
        j.state = SparkJobState::Cancelled;
        j.finished_at = Some(OffsetDateTime::now_utc());
        Ok(j.clone())
    }
}
