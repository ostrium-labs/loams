//! The namespace-scoped HTTP API for managed Flink SQL (D1988, ENG81 R2,
//! R24, R25b). Preview.
//!
//! Mounted next to the engines facade (`loams-engines`), under
//! `/v1/namespaces/{ns}/engines/{instance}/flink`:
//!
//! | Method | Path | What |
//! |---|---|---|
//! | GET | `/bootstrap` | the session bootstrap script, secrets redacted |
//! | POST | `/sql` | one-shot: run statements in a fresh session, return the last result |
//! | POST | `/sessions` | open a bootstrapped session |
//! | DELETE | `/sessions/{s}` | close it |
//! | POST | `/sessions/{s}/statements` | submit a statement |
//! | GET | `/sessions/{s}/statements/{st}/results` | stream its results (NDJSON) |
//! | DELETE | `/sessions/{s}/statements/{st}` | cancel it |
//! | GET, POST | `/jobs` | list, create saved jobs |
//! | GET, DELETE | `/jobs/{j}` | read (refreshed from Flink), delete |
//! | POST | `/jobs/{j}/start`, `/stop`, `/resume` | lifecycle |
//!
//! and `GET /v1/flink/connector-jars` (the bundle, D1986).
//!
//! Errors are `{"error": "<code>", "message": "…"}`, as the console
//! contract. Reads need `can_view`, everything else `can_operate`, checked by
//! the host's [`Authorizer`] on the namespace (ENG81 R18).
//!
//! The NDJSON result stream is one JSON object per line:
//! `{"type":"columns","columns":[…]}`, `{"type":"rows","rows":[{"kind":"+I","fields":[…]}]}`,
//! `{"type":"job","job_id":"…"}`, and last `{"type":"end","rows":n,"truncated":b}` or
//! `{"type":"error","error":"…","message":"…"}`.

pub mod backend;
pub mod jobs;
pub mod run;
pub mod split;
pub mod sql_gateway_adapter;

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::Mutex;
use tokio::time::Instant;

use self::backend::{BackendError, FlinkSqlBackend, OperationHandle, ResultType, SessionHandle};
use self::jobs::{JobCtx, JobStore, NewJob, SavedJob, StoreError};
use self::run::{RunLimits, open_bootstrapped, run_to_end};
use self::split::{first_keyword, split_statements};
use crate::catalog::{RenderCtx, SessionSpec, SqlError};

// ---------------------------------------------------------------------------
// Errors

/// An API error: an HTTP status, a stable code and a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_argument", message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "conflict", message)
    }

    pub fn unauthenticated(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthenticated", message)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "permission_denied", message)
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "engine_unavailable",
            message,
        )
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
    }

    /// The statement failed in Flink (parse, validation, runtime).
    pub fn statement(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "statement_failed",
            message,
        )
    }

    fn body(&self) -> serde_json::Value {
        json!({ "error": self.code, "message": self.message })
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ApiError {}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body())).into_response()
    }
}

impl From<SqlError> for ApiError {
    fn from(e: SqlError) -> Self {
        Self::invalid(e.to_string())
    }
}

impl From<BackendError> for ApiError {
    fn from(e: BackendError) -> Self {
        match e {
            BackendError::Unavailable(_) => Self::unavailable(e.to_string()),
            BackendError::NotFound => {
                Self::not_found("the Flink session or statement expired or does not exist")
            }
            BackendError::Statement(m) => Self::statement(m),
            BackendError::Gateway { .. } | BackendError::Protocol(_) => {
                Self::new(StatusCode::BAD_GATEWAY, "engine_error", e.to_string())
            }
        }
    }
}

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::Exists(_) => Self::conflict(e.to_string()),
            StoreError::NotFound(_) => Self::not_found(e.to_string()),
            StoreError::Conflict(_) => Self::conflict(format!(
                "{e}: another operation is running on it; retry when it is done"
            )),
            StoreError::Backend(_) => Self::internal(e.to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// Seams the host fills

/// What a route needs on the namespace (ENG81 R18).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    View,
    Operate,
}

/// The caller, as the host's verifier saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    pub id: String,
}

/// Authentication and authorization, by the host (the shared Loams
/// verifier and OpenFGA checks on `environment:<ns>`).
#[async_trait]
pub trait Authorizer: Send + Sync + 'static {
    async fn authorize(
        &self,
        headers: &HeaderMap,
        namespace: &str,
        need: Need,
    ) -> Result<Principal, ApiError>;
}

/// One instance's SQL Gateway and the bootstrap every session gets
/// (the Iceberg catalog, the instance's Kafka credentials, the savepoint
/// directory under `engines/<ns>/<id>/savepoints`).
#[derive(Clone)]
pub struct FlinkTarget {
    pub backend: Arc<dyn FlinkSqlBackend>,
    pub session: SessionSpec,
}

impl std::fmt::Debug for FlinkTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FlinkTarget")
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}

/// Finds an instance's target; `not_found` for an instance that is not a
/// running Flink instance of the namespace.
#[async_trait]
pub trait InstanceResolver: Send + Sync + 'static {
    async fn resolve(&self, namespace: &str, instance: &str) -> Result<Arc<FlinkTarget>, ApiError>;
}

/// Resolves secret references (D1985) for a namespace: the instance's
/// scoped service credentials (ENG81 R5) and namespace secrets.
#[async_trait]
pub trait SecretResolver: Send + Sync + 'static {
    async fn resolve(
        &self,
        namespace: &str,
        names: &BTreeSet<String>,
    ) -> Result<BTreeMap<String, String>, ApiError>;
}

/// Secrets from a fixed map (compose, where `engines-init` writes them into
/// the environment, and tests).
#[derive(Clone, Default)]
pub struct StaticSecrets(pub BTreeMap<String, String>);

impl std::fmt::Debug for StaticSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StaticSecrets")
            .field("names", &self.0.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[async_trait]
impl SecretResolver for StaticSecrets {
    async fn resolve(
        &self,
        _namespace: &str,
        names: &BTreeSet<String>,
    ) -> Result<BTreeMap<String, String>, ApiError> {
        let mut out = BTreeMap::new();
        for n in names {
            let v = self
                .0
                .get(n)
                .ok_or_else(|| ApiError::invalid(format!("secret {n:?} is not set")))?;
            out.insert(n.clone(), v.clone());
        }
        Ok(out)
    }
}

/// Fixed targets by `(namespace, instance)` (compose's single instance,
/// tests).
#[derive(Debug, Clone, Default)]
pub struct StaticInstances(pub HashMap<(String, String), Arc<FlinkTarget>>);

#[async_trait]
impl InstanceResolver for StaticInstances {
    async fn resolve(&self, namespace: &str, instance: &str) -> Result<Arc<FlinkTarget>, ApiError> {
        self.0
            .get(&(namespace.to_string(), instance.to_string()))
            .cloned()
            .ok_or_else(|| {
                ApiError::not_found(format!("no Flink instance {instance} in {namespace}"))
            })
    }
}

// ---------------------------------------------------------------------------
// State

/// Limits for API calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiLimits {
    pub run: RunLimits,
    /// Default and ceiling for streamed rows.
    pub stream_rows_default: usize,
    pub stream_rows_max: usize,
    /// Default and ceiling for how long a result stream stays open.
    pub stream_wait_default: Duration,
    pub stream_wait_max: Duration,
    /// Open statements per session.
    pub statements_per_session: usize,
    /// Sessions per principal.
    pub sessions_per_principal: usize,
}

impl Default for ApiLimits {
    fn default() -> Self {
        Self {
            run: RunLimits::default(),
            stream_rows_default: 1_000,
            stream_rows_max: 100_000,
            stream_wait_default: Duration::from_secs(300),
            stream_wait_max: Duration::from_secs(3_600),
            statements_per_session: 256,
            sessions_per_principal: 16,
        }
    }
}

#[derive(Debug)]
struct StatementEntry {
    op: OperationHandle,
    streaming: bool,
}

#[derive(Debug)]
struct SessionEntry {
    namespace: String,
    instance: String,
    principal: String,
    handle: SessionHandle,
    target: Arc<FlinkTarget>,
    statements: HashMap<String, StatementEntry>,
    last_used: Instant,
}

type Sessions = Arc<Mutex<HashMap<String, SessionEntry>>>;

/// The API's state; cheap to clone.
#[derive(Clone)]
pub struct ApiState {
    pub authz: Arc<dyn Authorizer>,
    pub instances: Arc<dyn InstanceResolver>,
    pub secrets: Arc<dyn SecretResolver>,
    pub jobs: Arc<dyn JobStore>,
    pub limits: ApiLimits,
    sessions: Sessions,
}

impl std::fmt::Debug for ApiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiState")
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl ApiState {
    pub fn new(
        authz: Arc<dyn Authorizer>,
        instances: Arc<dyn InstanceResolver>,
        secrets: Arc<dyn SecretResolver>,
        jobs: Arc<dyn JobStore>,
    ) -> Self {
        Self {
            authz,
            instances,
            secrets,
            jobs,
            limits: ApiLimits::default(),
            sessions: Arc::default(),
        }
    }

    pub fn with_limits(mut self, limits: ApiLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Close sessions idle longer than `idle` and heartbeat the rest, so
    /// the SQL Gateway does not expire a session a person is still using.
    /// Run it every minute or so ([`spawn_session_reaper`]).
    pub async fn reap_sessions(&self, idle: Duration) {
        let now = Instant::now();
        let (expired, alive): (Vec<_>, Vec<_>) = {
            let mut sessions = self.sessions.lock().await;
            let expired_ids: Vec<String> = sessions
                .iter()
                .filter(|(_, e)| now.duration_since(e.last_used) > idle)
                .map(|(id, _)| id.clone())
                .collect();
            let expired = expired_ids
                .iter()
                .filter_map(|id| sessions.remove(id))
                .map(|e| (e.target, e.handle))
                .collect();
            let alive = sessions
                .values()
                .map(|e| (e.target.clone(), e.handle.clone()))
                .collect();
            (expired, alive)
        };
        for (target, handle) in expired {
            let _ = target.backend.close_session(&handle).await;
        }
        for (target, handle) in alive {
            let _ = target.backend.heartbeat(&handle).await;
        }
    }

    fn ctx<'a>(&'a self, target: &'a FlinkTarget) -> JobCtx<'a> {
        JobCtx {
            store: self.jobs.as_ref(),
            target,
            secrets: self.secrets.as_ref(),
            limits: &self.limits.run,
        }
    }

    async fn enter(
        &self,
        headers: &HeaderMap,
        namespace: &str,
        instance: &str,
        need: Need,
    ) -> Result<(Principal, Arc<FlinkTarget>), ApiError> {
        let principal = self.authz.authorize(headers, namespace, need).await?;
        let target = self.instances.resolve(namespace, instance).await?;
        Ok((principal, target))
    }

    /// The caller's own session (another caller's is `not_found`).
    async fn session(
        &self,
        namespace: &str,
        instance: &str,
        principal: &Principal,
        id: &str,
    ) -> Result<(Arc<FlinkTarget>, SessionHandle), ApiError> {
        let mut sessions = self.sessions.lock().await;
        match sessions.get_mut(id) {
            Some(e)
                if e.namespace == namespace
                    && e.instance == instance
                    && e.principal == principal.id =>
            {
                e.last_used = Instant::now();
                Ok((e.target.clone(), e.handle.clone()))
            }
            _ => Err(ApiError::not_found(format!("session {id} not found"))),
        }
    }
}

/// Run [`ApiState::reap_sessions`] every `every`, closing sessions idle for
/// `idle`.
pub fn spawn_session_reaper(
    state: ApiState,
    every: Duration,
    idle: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        loop {
            tick.tick().await;
            state.reap_sessions(idle).await;
        }
    })
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}

// ---------------------------------------------------------------------------
// Router

/// The routes, to merge into the host's router.
pub fn router(state: ApiState) -> Router {
    let base = "/v1/namespaces/{ns}/engines/{instance}/flink";
    Router::new()
        .route("/v1/flink/connector-jars", get(connector_jars))
        .route(&format!("{base}/bootstrap"), get(bootstrap))
        .route(&format!("{base}/sql"), post(sql_once))
        .route(&format!("{base}/sessions"), post(open_session))
        .route(
            &format!("{base}/sessions/{{session}}"),
            delete(close_session),
        )
        .route(
            &format!("{base}/sessions/{{session}}/statements"),
            post(submit_statement),
        )
        .route(
            &format!("{base}/sessions/{{session}}/statements/{{statement}}"),
            delete(cancel_statement),
        )
        .route(
            &format!("{base}/sessions/{{session}}/statements/{{statement}}/results"),
            get(stream_results),
        )
        .route(&format!("{base}/jobs"), get(list_jobs).post(create_job))
        .route(
            &format!("{base}/jobs/{{job}}"),
            get(get_job).delete(delete_job),
        )
        .route(&format!("{base}/jobs/{{job}}/start"), post(start_job))
        .route(&format!("{base}/jobs/{{job}}/stop"), post(stop_job))
        .route(&format!("{base}/jobs/{{job}}/resume"), post(resume_job))
        .with_state(state)
}

async fn connector_jars() -> Json<serde_json::Value> {
    use crate::connectors::jars;
    Json(json!({
        "flink_version": jars::FLINK_VERSION,
        "base_image": jars::FLINK_IMAGE,
        "image": jars::image_ref(),
        "bundle": jars::BUNDLE_REVISION,
        "jars": jars::BUNDLE.iter().map(|j| json!({
            "group": j.group,
            "artifact": j.artifact,
            "version": j.version,
            "sha256": j.sha256,
            "license": j.license,
            "purpose": j.purpose,
            "url": j.url(jars::MAVEN_CENTRAL),
        })).collect::<Vec<_>>(),
        "stability": "preview",
    }))
}

async fn bootstrap(
    State(st): State<ApiState>,
    Path((ns, instance)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (_, target) = st.enter(&headers, &ns, &instance, Need::View).await?;
    let script = target.session.script()?;
    Ok(Json(json!({
        "script": script.render_script(&RenderCtx::redacted())?,
        "secrets": script.secret_refs(),
    })))
}

// --- one-shot SQL ---------------------------------------------------------

/// `POST …/sql`.
#[derive(Debug, Clone, Deserialize)]
pub struct SqlRequest {
    pub sql: String,
    #[serde(default)]
    pub max_rows: Option<u32>,
    #[serde(default)]
    pub timeout_ms: Option<u32>,
}

/// The engines facade's `SqlResult` shape (ENG-UI D2015), plus row kinds.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SqlResult {
    pub columns: Vec<SqlColumn>,
    pub rows: Vec<Vec<serde_json::Value>>,
    /// `+I`, `-U`, `+U`, `-D` per row.
    pub kinds: Vec<backend::RowKind>,
    pub row_count: u64,
    pub truncated: bool,
    pub elapsed_ms: u64,
    pub notice: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SqlColumn {
    pub name: String,
    #[serde(rename = "type")]
    pub data_type: String,
}

/// Run `req.sql` (one or more statements) in a fresh bootstrapped session
/// and return the last statement's result. What the engines facade's
/// `SqlEngine::execute` calls for Flink.
pub async fn execute_sql(
    target: &FlinkTarget,
    secrets: &dyn SecretResolver,
    namespace: &str,
    req: &SqlRequest,
    limits: &RunLimits,
) -> Result<SqlResult, ApiError> {
    let started = Instant::now();
    let (stmts, rest) = split_statements(&format!("{};", req.sql.trim().trim_end_matches(';')));
    if stmts.is_empty() || !rest.trim().is_empty() {
        return Err(ApiError::invalid("no complete statement in sql"));
    }
    for s in &stmts {
        refuse_protected(s)?;
    }
    let limits = RunLimits {
        max_rows: req
            .max_rows
            .map_or(limits.max_rows, |m| (m as usize).min(limits.max_rows)),
        wait: req.timeout_ms.map_or(Duration::from_secs(30), |t| {
            Duration::from_millis(u64::from(t)).min(limits.wait)
        }),
        ..*limits
    };
    let session = open_bootstrapped(
        target,
        secrets,
        namespace,
        &new_id("loams-sql"),
        &SessionSpec::default(),
        &[],
        &limits,
    )
    .await?;
    let backend = target.backend.as_ref();
    let mut last = None;
    let mut failure = None;
    for s in &stmts {
        match run_to_end(backend, &session, s, &limits).await {
            Ok(out) => last = Some(out),
            Err(e) => {
                failure = Some(ApiError::from(e));
                break;
            }
        }
    }
    let _ = backend.close_session(&session).await;
    if let Some(e) = failure {
        return Err(e);
    }
    let out = last.unwrap_or_default();
    let notice = match (&out.job_id, out.is_query) {
        (Some(id), false) => Some(format!("job submitted: {id}")),
        _ => None,
    };
    Ok(SqlResult {
        columns: out
            .columns
            .iter()
            .map(|c| SqlColumn {
                name: c.name.clone(),
                data_type: c.data_type.clone(),
            })
            .collect(),
        kinds: out.rows.iter().map(|r| r.kind).collect(),
        row_count: out.rows.len() as u64,
        rows: out.rows.into_iter().map(|r| r.fields).collect(),
        truncated: out.truncated,
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        notice,
    })
}

async fn sql_once(
    State(st): State<ApiState>,
    Path((ns, instance)): Path<(String, String)>,
    headers: HeaderMap,
    Json(req): Json<SqlRequest>,
) -> Result<Json<SqlResult>, ApiError> {
    let (_, target) = st.enter(&headers, &ns, &instance, Need::Operate).await?;
    Ok(Json(
        execute_sql(&target, st.secrets.as_ref(), &ns, &req, &st.limits.run).await?,
    ))
}

/// Keys a statement may not `SET` or `RESET`: the instance's state layout
/// and recovery, which the controller and saved jobs own (FL-a D1977).
const PROTECTED_KEYS: &[&str] = &[
    "execution.checkpointing.dir",
    "execution.checkpointing.savepoint-dir",
    "execution.state-recovery.path",
    "execution.savepoint.path",
    "state.checkpoints.dir",
    "state.savepoints.dir",
    "state.backend",
    "high-availability",
    "kubernetes.",
    "table.catalog-store",
];

fn refuse_protected(stmt: &str) -> Result<(), ApiError> {
    if !matches!(first_keyword(stmt).as_deref(), Some("SET" | "RESET")) {
        return Ok(());
    }
    let lower = stmt.to_ascii_lowercase();
    match PROTECTED_KEYS.iter().find(|k| lower.contains(*k)) {
        Some(k) => Err(ApiError::forbidden(format!(
            "{k} belongs to the instance; it cannot be set from a session"
        ))),
        None => Ok(()),
    }
}

// --- sessions and statements ---------------------------------------------

/// `POST …/sessions`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OpenSessionRequest {
    /// Tables, catalogs and settings on top of the instance's bootstrap.
    /// Inline secrets are allowed here: a session is not stored.
    #[serde(default)]
    pub spec: SessionSpec,
}

async fn open_session(
    State(st): State<ApiState>,
    Path((ns, instance)): Path<(String, String)>,
    headers: HeaderMap,
    body: Option<Json<OpenSessionRequest>>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let (principal, target) = st.enter(&headers, &ns, &instance, Need::Operate).await?;
    let req = body.map(|Json(b)| b).unwrap_or_default();
    {
        let sessions = st.sessions.lock().await;
        let mine = sessions
            .values()
            .filter(|e| e.principal == principal.id)
            .count();
        if mine >= st.limits.sessions_per_principal {
            return Err(ApiError::conflict(format!(
                "{mine} sessions are open; close one first"
            )));
        }
    }
    let id = new_id("fs");
    let handle = open_bootstrapped(
        &target,
        st.secrets.as_ref(),
        &ns,
        &id,
        &req.spec,
        &[],
        &st.limits.run,
    )
    .await?;
    st.sessions.lock().await.insert(
        id.clone(),
        SessionEntry {
            namespace: ns,
            instance,
            principal: principal.id,
            handle,
            target,
            statements: HashMap::new(),
            last_used: Instant::now(),
        },
    );
    Ok((StatusCode::CREATED, Json(json!({ "session": id }))))
}

async fn close_session(
    State(st): State<ApiState>,
    Path((ns, instance, session)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let (principal, _) = st.enter(&headers, &ns, &instance, Need::Operate).await?;
    let (target, handle) = st.session(&ns, &instance, &principal, &session).await?;
    st.sessions.lock().await.remove(&session);
    let _ = target.backend.close_session(&handle).await;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST …/statements`.
#[derive(Debug, Clone, Deserialize)]
pub struct StatementRequest {
    pub statement: String,
}

async fn submit_statement(
    State(st): State<ApiState>,
    Path((ns, instance, session)): Path<(String, String, String)>,
    headers: HeaderMap,
    Json(req): Json<StatementRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let (principal, _) = st.enter(&headers, &ns, &instance, Need::Operate).await?;
    let (target, handle) = st.session(&ns, &instance, &principal, &session).await?;
    let (stmts, rest) =
        split_statements(&format!("{};", req.statement.trim().trim_end_matches(';')));
    if stmts.len() != 1 || !rest.trim().is_empty() {
        return Err(ApiError::invalid("send one statement at a time"));
    }
    refuse_protected(&stmts[0])?;
    {
        let sessions = st.sessions.lock().await;
        if let Some(e) = sessions.get(&session)
            && e.statements.len() >= st.limits.statements_per_session
        {
            return Err(ApiError::conflict(
                "too many open statements in this session; delete finished ones",
            ));
        }
    }
    let op = target.backend.execute(&handle, &stmts[0]).await?;
    let id = new_id("st");
    let inserted = {
        let mut sessions = st.sessions.lock().await;
        match sessions.get_mut(&session) {
            Some(e) => {
                e.statements.insert(
                    id.clone(),
                    StatementEntry {
                        op: op.clone(),
                        streaming: false,
                    },
                );
                true
            }
            None => false,
        }
    };
    if !inserted {
        let _ = target.backend.cancel(&handle, &op).await;
        let _ = target.backend.close_operation(&handle, &op).await;
        return Err(ApiError::not_found(format!("session {session} was closed")));
    }
    Ok((
        StatusCode::CREATED,
        Json(json!({ "session": session, "statement": id })),
    ))
}

async fn cancel_statement(
    State(st): State<ApiState>,
    Path((ns, instance, session, statement)): Path<(String, String, String, String)>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let (principal, _) = st.enter(&headers, &ns, &instance, Need::Operate).await?;
    let (target, handle) = st.session(&ns, &instance, &principal, &session).await?;
    let entry = {
        let mut sessions = st.sessions.lock().await;
        sessions
            .get_mut(&session)
            .and_then(|e| e.statements.remove(&statement))
    };
    let Some(entry) = entry else {
        return Err(ApiError::not_found(format!(
            "statement {statement} not found"
        )));
    };
    let _ = target.backend.cancel(&handle, &entry.op).await;
    let _ = target.backend.close_operation(&handle, &entry.op).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Query of `GET …/results`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ResultsQuery {
    #[serde(default)]
    pub max_rows: Option<usize>,
    #[serde(default)]
    pub wait_ms: Option<u64>,
}

/// Cancels and closes an operation when a result stream is dropped before
/// its end (the client went away).
struct OpGuard {
    backend: Arc<dyn FlinkSqlBackend>,
    session: SessionHandle,
    op: OperationHandle,
    armed: bool,
}

impl Drop for OpGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            let backend = self.backend.clone();
            let session = self.session.clone();
            let op = self.op.clone();
            rt.spawn(async move {
                let _ = backend.cancel(&session, &op).await;
                let _ = backend.close_operation(&session, &op).await;
            });
        }
    }
}

struct ResultStream {
    guard: OpGuard,
    sessions: Sessions,
    session_id: String,
    statement_id: String,
    token: u64,
    max_rows: usize,
    emitted: usize,
    columns_sent: bool,
    job_sent: bool,
    deadline: Instant,
    delay: Duration,
    poll_min: Duration,
    poll_max: Duration,
    queue: VecDeque<Bytes>,
    done: bool,
}

fn line(v: &serde_json::Value) -> Bytes {
    let mut b = serde_json::to_vec(v).unwrap_or_default();
    b.push(b'\n');
    Bytes::from(b)
}

impl ResultStream {
    async fn end(&mut self, truncated: bool, cancel: bool) {
        self.queue.push_back(line(&json!({
            "type": "end",
            "rows": self.emitted,
            "truncated": truncated,
        })));
        self.close(cancel).await;
    }

    async fn fail(&mut self, e: ApiError) {
        self.queue.push_back(line(&json!({
            "type": "error",
            "error": e.code,
            "message": e.message,
        })));
        self.close(false).await;
    }

    async fn close(&mut self, cancel: bool) {
        self.done = true;
        self.guard.armed = false;
        let g = &self.guard;
        if cancel {
            let _ = g.backend.cancel(&g.session, &g.op).await;
        }
        let _ = g.backend.close_operation(&g.session, &g.op).await;
        if let Some(e) = self.sessions.lock().await.get_mut(&self.session_id) {
            e.statements.remove(&self.statement_id);
        }
    }

    async fn step(&mut self) {
        if Instant::now() >= self.deadline {
            return self.end(true, true).await;
        }
        let page = {
            let g = &self.guard;
            g.backend.fetch(&g.session, &g.op, self.token).await
        };
        let page = match page {
            Ok(p) => p,
            Err(e) => return self.fail(ApiError::from(e)).await,
        };
        if !self.job_sent
            && let Some(id) = &page.job_id
        {
            self.job_sent = true;
            self.queue
                .push_back(line(&json!({ "type": "job", "job_id": id })));
        }
        if !self.columns_sent && !page.columns.is_empty() {
            self.columns_sent = true;
            self.queue
                .push_back(line(&json!({ "type": "columns", "columns": page.columns })));
        }
        match page.result_type {
            ResultType::Eos => return self.end(false, false).await,
            ResultType::NotReady => {
                tokio::time::sleep(self.delay).await;
                self.delay = (self.delay * 2).min(self.poll_max);
            }
            ResultType::Payload => {
                self.delay = self.poll_min;
                let mut rows = page.rows;
                let room = self.max_rows - self.emitted;
                let over = rows.len() > room;
                rows.truncate(room);
                self.emitted += rows.len();
                if !rows.is_empty() {
                    self.queue
                        .push_back(line(&json!({ "type": "rows", "rows": rows })));
                }
                if over || (self.emitted >= self.max_rows && page.next_token.is_some()) {
                    return self.end(true, true).await;
                }
            }
        }
        match page.next_token {
            Some(t) => self.token = t,
            None => self.end(false, false).await,
        }
    }
}

async fn stream_results(
    State(st): State<ApiState>,
    Path((ns, instance, session, statement)): Path<(String, String, String, String)>,
    Query(q): Query<ResultsQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let (principal, _) = st.enter(&headers, &ns, &instance, Need::View).await?;
    let (target, handle) = st.session(&ns, &instance, &principal, &session).await?;
    let op = {
        let mut sessions = st.sessions.lock().await;
        let entry = sessions
            .get_mut(&session)
            .and_then(|e| e.statements.get_mut(&statement))
            .ok_or_else(|| ApiError::not_found(format!("statement {statement} not found")))?;
        if entry.streaming {
            return Err(ApiError::conflict(format!(
                "statement {statement}'s results are already being read"
            )));
        }
        entry.streaming = true;
        entry.op.clone()
    };
    let limits = st.limits;
    let max_rows = q
        .max_rows
        .unwrap_or(limits.stream_rows_default)
        .clamp(1, limits.stream_rows_max);
    let wait = q
        .wait_ms
        .map_or(limits.stream_wait_default, Duration::from_millis)
        .min(limits.stream_wait_max);
    let state = ResultStream {
        guard: OpGuard {
            backend: target.backend.clone(),
            session: handle,
            op,
            armed: true,
        },
        sessions: st.sessions.clone(),
        session_id: session,
        statement_id: statement,
        token: 0,
        max_rows,
        emitted: 0,
        columns_sent: false,
        job_sent: false,
        deadline: Instant::now() + wait,
        delay: limits.run.poll_min,
        poll_min: limits.run.poll_min,
        poll_max: limits.run.poll_max,
        queue: VecDeque::new(),
        done: false,
    };
    let stream = futures::stream::unfold(state, |mut s| async move {
        loop {
            if let Some(b) = s.queue.pop_front() {
                return Some((Ok::<Bytes, std::io::Error>(b), s));
            }
            if s.done {
                return None;
            }
            s.step().await;
        }
    });
    Ok((
        [(header::CONTENT_TYPE, "application/x-ndjson")],
        Body::from_stream(stream),
    )
        .into_response())
}

// --- saved jobs -------------------------------------------------------------

async fn list_jobs(
    State(st): State<ApiState>,
    Path((ns, instance)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    st.enter(&headers, &ns, &instance, Need::View).await?;
    let jobs = st.jobs.list(&ns, &instance).await?;
    Ok(Json(json!({ "jobs": jobs })))
}

async fn create_job(
    State(st): State<ApiState>,
    Path((ns, instance)): Path<(String, String)>,
    headers: HeaderMap,
    Json(new): Json<NewJob>,
) -> Result<(StatusCode, Json<SavedJob>), ApiError> {
    let (principal, target) = st.enter(&headers, &ns, &instance, Need::Operate).await?;
    let job = jobs::create(&st.ctx(&target), &ns, &instance, &principal.id, new).await?;
    Ok((StatusCode::CREATED, Json(job)))
}

/// A job of this instance (one of another instance is `not_found`).
async fn job_of(st: &ApiState, ns: &str, instance: &str, id: &str) -> Result<SavedJob, ApiError> {
    match st.jobs.get(ns, id).await? {
        Some(j) if j.instance == instance => Ok(j),
        _ => Err(ApiError::not_found(format!("job {id} not found"))),
    }
}

async fn get_job(
    State(st): State<ApiState>,
    Path((ns, instance, job)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Json<SavedJob>, ApiError> {
    let (_, target) = st.enter(&headers, &ns, &instance, Need::View).await?;
    let stored = job_of(&st, &ns, &instance, &job).await?;
    // Best effort: a Flink that does not answer leaves the stored state.
    Ok(Json(
        jobs::refresh(&st.ctx(&target), &ns, &job)
            .await
            .unwrap_or(stored),
    ))
}

async fn delete_job(
    State(st): State<ApiState>,
    Path((ns, instance, job)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let (_, target) = st.enter(&headers, &ns, &instance, Need::Operate).await?;
    job_of(&st, &ns, &instance, &job).await?;
    jobs::delete(&st.ctx(&target), &ns, &job).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn start_job(
    State(st): State<ApiState>,
    Path((ns, instance, job)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Json<SavedJob>, ApiError> {
    let (_, target) = st.enter(&headers, &ns, &instance, Need::Operate).await?;
    job_of(&st, &ns, &instance, &job).await?;
    Ok(Json(jobs::start(&st.ctx(&target), &ns, &job, false).await?))
}

async fn resume_job(
    State(st): State<ApiState>,
    Path((ns, instance, job)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Json<SavedJob>, ApiError> {
    let (_, target) = st.enter(&headers, &ns, &instance, Need::Operate).await?;
    job_of(&st, &ns, &instance, &job).await?;
    Ok(Json(jobs::start(&st.ctx(&target), &ns, &job, true).await?))
}

/// `POST …/jobs/{j}/stop`.
#[derive(Debug, Clone, Deserialize)]
pub struct StopRequest {
    #[serde(default = "yes")]
    pub savepoint: bool,
    #[serde(default)]
    pub drain: bool,
}

fn yes() -> bool {
    true
}

impl Default for StopRequest {
    fn default() -> Self {
        Self {
            savepoint: true,
            drain: false,
        }
    }
}

async fn stop_job(
    State(st): State<ApiState>,
    Path((ns, instance, job)): Path<(String, String, String)>,
    headers: HeaderMap,
    body: Option<Json<StopRequest>>,
) -> Result<Json<SavedJob>, ApiError> {
    let (_, target) = st.enter(&headers, &ns, &instance, Need::Operate).await?;
    job_of(&st, &ns, &instance, &job).await?;
    let req = body.map(|Json(b)| b).unwrap_or_default();
    Ok(Json(
        jobs::stop(&st.ctx(&target), &ns, &job, req.savepoint, req.drain).await?,
    ))
}

#[cfg(test)]
mod tests;
