//! `WorkflowService`: the DX1 method set (design 74 §5.8, D1506). Every
//! other method answers `UNIMPLEMENTED` through [`crate::ImplementedOnly`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use loams_temporal_history::time::{dur, ts};
use loams_temporal_history::{Attributes, WorkflowExecutionStatus};
use loams_temporal_matching::{Handout, Matcher, QueueKey, TaskKind as QueueKind};
use loams_temporal_proto::loams::temporal::internal::v1::{TaskKind, TaskToken};
use loams_temporal_proto::temporal::api::common::v1::{
    WorkerVersionStamp, WorkflowExecution, WorkflowType,
};
use loams_temporal_proto::temporal::api::enums::v1::{
    HistoryEventFilterType, NamespaceState, PendingActivityState, PendingWorkflowTaskState,
    QueryResultType, TaskQueueKind,
};
use loams_temporal_proto::temporal::api::history::v1::History;
use loams_temporal_proto::temporal::api::namespace::v1::{NamespaceConfig, NamespaceInfo};
use loams_temporal_proto::temporal::api::taskqueue::v1::TaskQueue;
use loams_temporal_proto::temporal::api::workflow::v1::{
    PendingActivityInfo, PendingWorkflowTaskInfo, WorkflowExecutionConfig, WorkflowExecutionInfo,
};
use loams_temporal_proto::temporal::api::workflowservice::v1::workflow_service_server::WorkflowService;
use loams_temporal_proto::temporal::api::workflowservice::v1::*;
use loams_temporal_resonate::ids::{PromiseId, WorkflowRef};
use loams_temporal_resonate::{Durability, SettleAs};
use loams_temporal_statemachine::{Input, Reply};
use loams_temporal_store::{NamespaceRecord, RunKey, Store, StoreError};
use tonic::{Request, Response, Status};

use crate::auth::{Authenticator, Principal};
use crate::capabilities::capabilities;
use crate::effects::{EffectContext, TaskRef};
use crate::engine::{Engine, store_status};
use crate::queries::{Queries, QueryAnswer, QueryTask};
use crate::token::TokenKeys;

/// Settings of the frontend.
#[derive(Debug, Clone)]
pub struct FrontendConfig {
    /// The longest a poll waits (X1.R3: 60 s).
    pub poll_timeout: Duration,
    /// How long `QueryWorkflow` waits for a worker.
    pub query_timeout: Duration,
    /// How long `GetWorkflowExecutionHistory` waits for a new event.
    pub history_wait: Duration,
    /// What `GetSystemInfo` reports as the server version.
    pub server_version: String,
    /// The default page of `GetWorkflowExecutionHistory`.
    pub history_page_size: usize,
}

impl Default for FrontendConfig {
    fn default() -> Self {
        FrontendConfig {
            poll_timeout: Duration::from_secs(60),
            query_timeout: Duration::from_secs(30),
            history_wait: Duration::from_secs(60),
            server_version: format!("loams-temporal {}", env!("CARGO_PKG_VERSION")),
            history_page_size: 1000,
        }
    }
}

/// The frontend: the engine, the matcher and everything a request needs.
pub struct Frontend {
    pub(crate) engine: Engine,
    pub(crate) matcher: Arc<Matcher>,
    pub(crate) durability: Arc<dyn Durability>,
    pub(crate) auth: Arc<dyn Authenticator>,
    pub(crate) tokens: TokenKeys,
    pub(crate) queries: Queries,
    pub(crate) config: FrontendConfig,
    namespaces: Mutex<HashMap<String, NamespaceRecord>>,
}

impl std::fmt::Debug for Frontend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Frontend").finish_non_exhaustive()
    }
}

fn wf_ref_key(token: &TaskToken) -> RunKey {
    RunKey::new(&token.namespace_id, &token.workflow_id, &token.run_id)
}

fn build_id_stamp(build_id: &str) -> Option<WorkerVersionStamp> {
    (!build_id.is_empty()).then(|| WorkerVersionStamp {
        build_id: build_id.to_string(),
        use_versioning: false,
    })
}

/// The build ID a poller reports: the deprecated version capabilities (the
/// Go SDK) or its deployment options (sdk-core, so the TypeScript SDK).
fn poll_build_id(capabilities: Option<&str>, deployment: Option<&str>) -> String {
    capabilities
        .filter(|b| !b.is_empty())
        .or(deployment.filter(|b| !b.is_empty()))
        .unwrap_or_default()
        .to_string()
}

/// The poll deadline: the configured timeout, or 5 s before the request's
/// own deadline if that is sooner (X1.R3).
fn poll_deadline<T>(req: &Request<T>, max: Duration) -> Duration {
    let header = req
        .metadata()
        .get("grpc-timeout")
        .and_then(|v| v.to_str().ok())
        .and_then(parse_grpc_timeout);
    match header {
        Some(d) if d > Duration::from_secs(5) => (d - Duration::from_secs(5)).min(max),
        Some(d) => d.min(max),
        None => max,
    }
}

fn parse_grpc_timeout(v: &str) -> Option<Duration> {
    let (n, unit) = v.split_at(v.len().checked_sub(1)?);
    let n: u64 = n.parse().ok()?;
    Some(match unit {
        "H" => Duration::from_secs(n * 3600),
        "M" => Duration::from_secs(n * 60),
        "S" => Duration::from_secs(n),
        "m" => Duration::from_millis(n),
        "u" => Duration::from_micros(n),
        "n" => Duration::from_nanos(n),
        _ => return None,
    })
}

impl Frontend {
    /// A frontend over its parts.
    pub fn new(
        engine: Engine,
        matcher: Arc<Matcher>,
        durability: Arc<dyn Durability>,
        auth: Arc<dyn Authenticator>,
        tokens: TokenKeys,
        config: FrontendConfig,
    ) -> Self {
        Frontend {
            engine,
            matcher,
            durability,
            auth,
            tokens,
            queries: Queries::default(),
            config,
            namespaces: Mutex::new(HashMap::new()),
        }
    }

    fn principal<T>(&self, req: &Request<T>) -> Result<Principal, Status> {
        self.auth.authenticate(req.metadata())
    }

    fn store(&self) -> &Arc<dyn Store> {
        self.engine.store()
    }

    /// The namespace `name` of the caller's Loams namespace.
    async fn namespace(&self, p: &Principal, name: &str) -> Result<NamespaceRecord, Status> {
        if name.is_empty() {
            return Err(Status::invalid_argument("Namespace not set on request."));
        }
        self.store()
            .namespace_by_name(&p.loams_namespace_id, name)
            .await
            .map_err(store_status)?
            .ok_or_else(|| crate::errors::namespace_not_found(name))
    }

    /// A namespace by ID (cached: namespaces are never renamed in DX1).
    async fn namespace_by_id(&self, id: &str) -> Result<NamespaceRecord, Status> {
        if let Some(ns) = self
            .namespaces
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
        {
            return Ok(ns.clone());
        }
        let ns = self
            .store()
            .namespace_by_id(id)
            .await
            .map_err(store_status)?
            .ok_or_else(|| crate::errors::namespace_not_found(id))?;
        self.namespaces
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.to_string(), ns.clone());
        Ok(ns)
    }

    fn ctx(ns: &NamespaceRecord) -> EffectContext {
        EffectContext {
            namespace_id: ns.namespace_id.clone(),
            loams_namespace_id: ns.loams_namespace_id.clone(),
        }
    }

    /// The run a request names (its current run when `run_id` is empty).
    async fn run_key(
        &self,
        ns: &NamespaceRecord,
        execution: Option<&WorkflowExecution>,
    ) -> Result<RunKey, Status> {
        let ex = execution
            .ok_or_else(|| Status::invalid_argument("Execution is not set on request."))?;
        if ex.workflow_id.is_empty() {
            return Err(Status::invalid_argument(
                "WorkflowId is not set on request.",
            ));
        }
        let run_id = if ex.run_id.is_empty() {
            self.store()
                .current_run(&ns.namespace_id, &ex.workflow_id)
                .await
                .map_err(store_status)?
                .ok_or_else(|| {
                    Status::not_found(format!(
                        "workflow execution not found for workflow ID \"{}\"",
                        ex.workflow_id
                    ))
                })?
                .run_id
        } else {
            ex.run_id.clone()
        };
        Ok(RunKey::new(&ns.namespace_id, &ex.workflow_id, run_id))
    }

    /// The token's run and its namespace, checked against the caller.
    async fn token_run(
        &self,
        p: &Principal,
        raw: &[u8],
        kind: TaskKind,
    ) -> Result<(TaskToken, RunKey, NamespaceRecord), Status> {
        let token = self.tokens.decode(raw, kind)?;
        let ns = self.namespace_by_id(&token.namespace_id).await?;
        if ns.loams_namespace_id != p.loams_namespace_id {
            return Err(Status::permission_denied(
                "the task belongs to another namespace",
            ));
        }
        let key = wf_ref_key(&token);
        Ok((token, key, ns))
    }

    fn promise_id(ns: &NamespaceRecord, key: &RunKey, token: &TaskToken) -> String {
        let nsseg =
            loams_temporal_resonate::ids::namespace(&ns.loams_namespace_id, &ns.namespace_id);
        let wf = WorkflowRef::Id(key.workflow_id.clone());
        let id = if token.kind == TaskKind::Workflow as i32 {
            PromiseId::Wft {
                ns: nsseg,
                wf,
                run: key.run_id.clone(),
                scheduled_event_id: token.scheduled_event_id,
            }
        } else {
            PromiseId::Act {
                ns: nsseg,
                wf,
                run: key.run_id.clone(),
                scheduled_event_id: token.scheduled_event_id,
                attempt: token.attempt,
            }
        };
        id.encode().0
    }

    /// Settles a handed-out task the run no longer wants (stale or closed).
    async fn drop_claim(&self, claim: &loams_temporal_resonate::Claim) {
        self.matcher.finished(&claim.task_id);
        if let Err(e) = self
            .durability
            .task_fulfill(&claim.task_id, claim.version, SettleAs::Resolved)
            .await
        {
            tracing::warn!(error = %e, "settling a stale task failed");
        }
    }

    async fn full_history(&self, key: &RunKey, up_to: i64) -> Result<History, Status> {
        let mut events = self.engine.history(key, 1, usize::MAX).await?;
        events.retain(|e| e.event_id <= up_to);
        Ok(History { events })
    }

    async fn workflow_task_response(
        &self,
        p: &Principal,
        key_q: &QueueKey,
        identity: &str,
        build_id: &str,
    ) -> Result<PollWorkflowTaskQueueResponse, Status> {
        let _ = p;
        let deadline = tokio::time::Instant::now() + self.config.poll_timeout;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                return Ok(PollWorkflowTaskQueueResponse::default());
            }
            let handout = self
                .matcher
                .poll(key_q, left)
                .await
                .map_err(|e| Status::unavailable(e.to_string()))?;
            let Some(handout) = handout else {
                return Ok(PollWorkflowTaskQueueResponse::default());
            };
            match handout {
                Handout::Local(id) => {
                    if let Some(r) = self.query_task_response(id).await? {
                        return Ok(r);
                    }
                }
                Handout::Claim(claim) => {
                    let Some(r) = self.start_workflow_task(&claim, identity, build_id).await?
                    else {
                        self.drop_claim(&claim).await;
                        continue;
                    };
                    return Ok(r);
                }
            }
        }
    }

    async fn query_task_response(
        &self,
        id: u64,
    ) -> Result<Option<PollWorkflowTaskQueueResponse>, Status> {
        let Some(q) = self.queries.task(id) else {
            return Ok(None);
        };
        let run = self.engine.load(&q.key).await?;
        let last = run.state.next_event_id - 1;
        let token = self.tokens.encode(TaskToken {
            namespace_id: q.key.namespace_id.clone(),
            workflow_id: q.key.workflow_id.clone(),
            run_id: q.key.run_id.clone(),
            scheduled_event_id: id as i64,
            attempt: 1,
            kind: TaskKind::Query as i32,
            ..Default::default()
        });
        Ok(Some(PollWorkflowTaskQueueResponse {
            task_token: token,
            workflow_execution: Some(WorkflowExecution {
                workflow_id: q.key.workflow_id.clone(),
                run_id: q.key.run_id.clone(),
            }),
            workflow_type: Some(WorkflowType {
                name: run.state.workflow_type.clone(),
            }),
            previous_started_event_id: run.state.last_completed_wft_started_event_id,
            // The worker replays the whole history to answer a query, so
            // the task "starts" at the last event (the Go SDK warns when it
            // processes events past this ID).
            started_event_id: last,
            history: Some(self.full_history(&q.key, last).await?),
            query: Some(q.query.clone()),
            workflow_execution_task_queue: Some(TaskQueue {
                name: run.state.task_queue.clone(),
                kind: TaskQueueKind::Normal as i32,
                normal_name: String::new(),
            }),
            ..Default::default()
        }))
    }

    /// Starts the workflow task a claim names; `None` if the run no longer
    /// wants it.
    async fn start_workflow_task(
        &self,
        claim: &loams_temporal_resonate::Claim,
        identity: &str,
        build_id: &str,
    ) -> Result<Option<PollWorkflowTaskQueueResponse>, Status> {
        let Some(task) = claim
            .param
            .as_deref()
            .and_then(|p| serde_json::from_slice::<TaskRef>(p).ok())
        else {
            tracing::warn!(task = %claim.task_id, param = ?claim.param, "a task without a readable reference");
            return Ok(None);
        };
        let ns = self.namespace_by_id(&task.namespace_id).await?;
        let key = RunKey::new(&task.namespace_id, &task.workflow_id, &task.run_id);
        let t = match self
            .engine
            .execute(
                &key,
                &Self::ctx(&ns),
                Input::WorkflowTaskStarted {
                    scheduled_event_id: task.scheduled_event_id,
                    identity: identity.to_string(),
                    request_id: claim.task_id.clone(),
                    worker_version: build_id_stamp(build_id),
                },
            )
            .await
        {
            Ok(t) => t,
            Err(s) if s.code() == tonic::Code::NotFound => {
                tracing::debug!(task = %claim.task_id, status = %s, "a workflow task the run no longer wants");
                return Ok(None);
            }
            Err(s) => return Err(s),
        };
        let Reply::WorkflowTaskStarted {
            scheduled_event_id,
            started_event_id,
            attempt,
            previous_started_event_id,
            scheduled_time_ms,
            started_time_ms,
        } = t.reply
        else {
            return Ok(None);
        };
        let token = self.tokens.encode(TaskToken {
            namespace_id: key.namespace_id.clone(),
            workflow_id: key.workflow_id.clone(),
            run_id: key.run_id.clone(),
            scheduled_event_id,
            attempt,
            kind: TaskKind::Workflow as i32,
            started_event_id,
            ..Default::default()
        });
        Ok(Some(PollWorkflowTaskQueueResponse {
            task_token: token,
            workflow_execution: Some(WorkflowExecution {
                workflow_id: key.workflow_id.clone(),
                run_id: key.run_id.clone(),
            }),
            workflow_type: Some(WorkflowType {
                name: t.state.workflow_type.clone(),
            }),
            previous_started_event_id,
            started_event_id,
            attempt,
            history: Some(self.full_history(&key, started_event_id).await?),
            workflow_execution_task_queue: Some(TaskQueue {
                name: t.state.task_queue.clone(),
                kind: TaskQueueKind::Normal as i32,
                normal_name: String::new(),
            }),
            scheduled_time: Some(ts(scheduled_time_ms)),
            started_time: Some(ts(started_time_ms)),
            ..Default::default()
        }))
    }

    async fn start_activity_task(
        &self,
        claim: &loams_temporal_resonate::Claim,
        identity: &str,
        build_id: &str,
    ) -> Result<Option<PollActivityTaskQueueResponse>, Status> {
        let Some(task) = claim
            .param
            .as_deref()
            .and_then(|p| serde_json::from_slice::<TaskRef>(p).ok())
        else {
            return Ok(None);
        };
        let ns = self.namespace_by_id(&task.namespace_id).await?;
        let key = RunKey::new(&task.namespace_id, &task.workflow_id, &task.run_id);
        let scheduled = self
            .engine
            .history(&key, task.scheduled_event_id, 1)
            .await?;
        let Some(Attributes::ActivityTaskScheduledEventAttributes(a)) =
            scheduled.first().and_then(|e| e.attributes.clone())
        else {
            return Ok(None);
        };
        // A worker that names no build of its own runs the activity on the
        // workflow's build (`use_workflow_build_id`): the stamp of the
        // workflow task that scheduled it.
        #[allow(deprecated)]
        let worker_version = match build_id_stamp(build_id) {
            Some(v) => Some(v),
            None if a.use_workflow_build_id => {
                let wtc = self
                    .engine
                    .history(&key, a.workflow_task_completed_event_id, 1)
                    .await?;
                match wtc.first().and_then(|e| e.attributes.as_ref()) {
                    Some(Attributes::WorkflowTaskCompletedEventAttributes(c)) => {
                        c.worker_version.clone()
                    }
                    _ => None,
                }
            }
            None => None,
        };
        let t = match self
            .engine
            .execute(
                &key,
                &Self::ctx(&ns),
                Input::ActivityStarted {
                    scheduled_event_id: task.scheduled_event_id,
                    attempt: task.attempt,
                    identity: identity.to_string(),
                    request_id: claim.task_id.clone(),
                    worker_version,
                },
            )
            .await
        {
            Ok(t) => t,
            Err(s) if s.code() == tonic::Code::NotFound => return Ok(None),
            Err(s) => return Err(s),
        };
        let Reply::ActivityStarted {
            scheduled_event_id,
            attempt,
            attempt_scheduled_time_ms,
            started_time_ms,
            heartbeat_details,
        } = t.reply
        else {
            return Ok(None);
        };
        debug_assert_eq!(scheduled_event_id, task.scheduled_event_id);
        let scheduled_time = scheduled.first().and_then(|e| e.event_time);
        let token = self.tokens.encode(TaskToken {
            namespace_id: key.namespace_id.clone(),
            workflow_id: key.workflow_id.clone(),
            run_id: key.run_id.clone(),
            scheduled_event_id,
            attempt,
            kind: TaskKind::Activity as i32,
            ..Default::default()
        });
        Ok(Some(PollActivityTaskQueueResponse {
            task_token: token,
            workflow_namespace: ns.name.clone(),
            workflow_type: Some(WorkflowType {
                name: t.state.workflow_type.clone(),
            }),
            workflow_execution: Some(WorkflowExecution {
                workflow_id: key.workflow_id.clone(),
                run_id: key.run_id.clone(),
            }),
            activity_type: a.activity_type,
            activity_id: a.activity_id,
            header: a.header,
            input: a.input,
            heartbeat_details,
            scheduled_time,
            current_attempt_scheduled_time: Some(ts(attempt_scheduled_time_ms)),
            started_time: Some(ts(started_time_ms)),
            attempt,
            schedule_to_close_timeout: a.schedule_to_close_timeout,
            start_to_close_timeout: a.start_to_close_timeout,
            heartbeat_timeout: a.heartbeat_timeout,
            retry_policy: a.retry_policy,
            priority: a.priority,
            ..Default::default()
        }))
    }

    /// Runs `input` on the token's run, then stops keeping the task's lease.
    async fn finish_task(
        &self,
        p: &Principal,
        raw: &[u8],
        kind: TaskKind,
        input: impl FnOnce(&TaskToken) -> Input,
    ) -> Result<Reply, Status> {
        let (token, key, ns) = self.token_run(p, raw, kind).await?;
        let t = self
            .engine
            .execute(&key, &Self::ctx(&ns), input(&token))
            .await?;
        if !matches!(t.reply, Reply::Heartbeat { .. }) {
            self.matcher.finished(&Self::promise_id(&ns, &key, &token));
        }
        Ok(t.reply)
    }

    fn describe_namespace_response(ns: &NamespaceRecord) -> DescribeNamespaceResponse {
        DescribeNamespaceResponse {
            namespace_info: Some(NamespaceInfo {
                name: ns.name.clone(),
                state: NamespaceState::Registered as i32,
                description: ns.config["description"].as_str().unwrap_or("").to_string(),
                id: ns.namespace_id.clone(),
                ..Default::default()
            }),
            config: Some(NamespaceConfig {
                workflow_execution_retention_ttl: Some(dur(ns.retention_secs * 1000)),
                ..Default::default()
            }),
            ..Default::default()
        }
    }
}

/// The gRPC service over a shared [`Frontend`].
#[derive(Debug, Clone)]
pub struct WorkflowServiceImpl(pub Arc<Frontend>);

#[async_trait]
impl WorkflowService for WorkflowServiceImpl {
    async fn get_system_info(
        &self,
        _req: Request<GetSystemInfoRequest>,
    ) -> Result<Response<GetSystemInfoResponse>, Status> {
        Ok(Response::new(GetSystemInfoResponse {
            server_version: self.0.config.server_version.clone(),
            capabilities: Some(capabilities()),
        }))
    }

    async fn register_namespace(
        &self,
        req: Request<RegisterNamespaceRequest>,
    ) -> Result<Response<RegisterNamespaceResponse>, Status> {
        let p = self.0.principal(&req)?;
        if !p.admin {
            return Err(Status::permission_denied(
                "registering a namespace needs the admin role",
            ));
        }
        let r = req.into_inner();
        if r.namespace.is_empty() {
            return Err(Status::invalid_argument("Namespace not set on request."));
        }
        let retention = r
            .workflow_execution_retention_period
            .as_ref()
            .map_or(72 * 3600, |d| d.seconds);
        let record = NamespaceRecord {
            namespace_id: uuid::Uuid::new_v4().to_string(),
            loams_namespace_id: p.loams_namespace_id,
            name: r.namespace,
            retention_secs: retention,
            state: NamespaceState::Registered as i32,
            config: serde_json::json!({ "description": r.description }),
        };
        match self.0.store().create_namespace(record).await {
            Ok(()) => Ok(Response::new(RegisterNamespaceResponse {})),
            Err(StoreError::NamespaceExists) => Err(crate::errors::namespace_already_exists()),
            Err(e) => Err(store_status(e)),
        }
    }

    async fn describe_namespace(
        &self,
        req: Request<DescribeNamespaceRequest>,
    ) -> Result<Response<DescribeNamespaceResponse>, Status> {
        let p = self.0.principal(&req)?;
        let r = req.into_inner();
        let ns = if !r.id.is_empty() {
            let ns = self.0.namespace_by_id(&r.id).await?;
            if ns.loams_namespace_id != p.loams_namespace_id {
                return Err(crate::errors::namespace_not_found(&r.id));
            }
            ns
        } else {
            self.0.namespace(&p, &r.namespace).await?
        };
        Ok(Response::new(Frontend::describe_namespace_response(&ns)))
    }

    async fn list_namespaces(
        &self,
        req: Request<ListNamespacesRequest>,
    ) -> Result<Response<ListNamespacesResponse>, Status> {
        let p = self.0.principal(&req)?;
        let all = self
            .0
            .store()
            .list_namespaces(&p.loams_namespace_id)
            .await
            .map_err(store_status)?;
        Ok(Response::new(ListNamespacesResponse {
            namespaces: all
                .iter()
                .map(Frontend::describe_namespace_response)
                .collect(),
            next_page_token: Default::default(),
        }))
    }

    async fn start_workflow_execution(
        &self,
        req: Request<StartWorkflowExecutionRequest>,
    ) -> Result<Response<StartWorkflowExecutionResponse>, Status> {
        let p = self.0.principal(&req)?;
        let mut r = req.into_inner();
        let ns = self.0.namespace(&p, &r.namespace).await?;
        if r.request_id.is_empty() {
            r.request_id = uuid::Uuid::new_v4().to_string();
        }
        let run_id = uuid::Uuid::new_v4().to_string();
        let (run_id, started) = self.0.engine.start(&Frontend::ctx(&ns), run_id, r).await?;
        Ok(Response::new(StartWorkflowExecutionResponse {
            run_id,
            started,
            status: WorkflowExecutionStatus::Running as i32,
            ..Default::default()
        }))
    }

    async fn poll_workflow_task_queue(
        &self,
        req: Request<PollWorkflowTaskQueueRequest>,
    ) -> Result<Response<PollWorkflowTaskQueueResponse>, Status> {
        let p = self.0.principal(&req)?;
        let wait = poll_deadline(&req, self.0.config.poll_timeout);
        let r = req.into_inner();
        let ns = self.0.namespace(&p, &r.namespace).await?;
        let tq = r
            .task_queue
            .as_ref()
            .map(|t| t.name.clone())
            .filter(|n| !n.is_empty())
            .ok_or_else(|| Status::invalid_argument("TaskQueue is not set on request."))?;
        #[allow(deprecated)]
        let build_id = poll_build_id(
            r.worker_version_capabilities
                .as_ref()
                .map(|c| c.build_id.as_str()),
            r.deployment_options.as_ref().map(|d| d.build_id.as_str()),
        );
        // sdk-core (the TypeScript SDK) names its build in the checksum.
        #[allow(deprecated)]
        let build_id = if build_id.is_empty() {
            r.binary_checksum.clone()
        } else {
            build_id
        };
        let key = QueueKey {
            namespace_id: ns.namespace_id.clone(),
            task_queue: tq,
            kind: QueueKind::Workflow,
        };
        let poll = self
            .0
            .workflow_task_response(&p, &key, &r.identity, &build_id);
        match tokio::time::timeout(wait, poll).await {
            Ok(r) => r.map(Response::new),
            Err(_) => Ok(Response::new(PollWorkflowTaskQueueResponse::default())),
        }
    }

    async fn respond_workflow_task_completed(
        &self,
        req: Request<RespondWorkflowTaskCompletedRequest>,
    ) -> Result<Response<RespondWorkflowTaskCompletedResponse>, Status> {
        let p = self.0.principal(&req)?;
        let r = req.into_inner();
        // The Go SDK stamps its build ID; sdk-core (TypeScript) sends it in
        // its deployment options. Either is what Temporal records.
        #[allow(deprecated)]
        let worker_version = r
            .worker_version_stamp
            .clone()
            .or_else(|| {
                r.deployment_options
                    .as_ref()
                    .and_then(|d| build_id_stamp(&d.build_id))
            })
            .or_else(|| build_id_stamp(&r.binary_checksum));
        let token = r.task_token.clone();
        self.0
            .finish_task(&p, &token, TaskKind::Workflow, |t| {
                Input::WorkflowTaskCompleted {
                    scheduled_event_id: t.scheduled_event_id,
                    attempt: t.attempt,
                    identity: r.identity,
                    commands: r.commands,
                    sdk_metadata: r.sdk_metadata,
                    metering_metadata: r.metering_metadata,
                    worker_version,
                    force_create_new_workflow_task: r.force_create_new_workflow_task,
                }
            })
            .await?;
        Ok(Response::new(
            RespondWorkflowTaskCompletedResponse::default(),
        ))
    }

    async fn respond_workflow_task_failed(
        &self,
        req: Request<RespondWorkflowTaskFailedRequest>,
    ) -> Result<Response<RespondWorkflowTaskFailedResponse>, Status> {
        let p = self.0.principal(&req)?;
        let r = req.into_inner();
        let token = r.task_token.clone();
        self.0
            .finish_task(&p, &token, TaskKind::Workflow, |t| {
                Input::WorkflowTaskFailed {
                    scheduled_event_id: t.scheduled_event_id,
                    attempt: t.attempt,
                    cause: r.cause,
                    failure: r.failure,
                    identity: r.identity,
                }
            })
            .await?;
        Ok(Response::new(RespondWorkflowTaskFailedResponse {}))
    }

    async fn poll_activity_task_queue(
        &self,
        req: Request<PollActivityTaskQueueRequest>,
    ) -> Result<Response<PollActivityTaskQueueResponse>, Status> {
        let p = self.0.principal(&req)?;
        let wait = poll_deadline(&req, self.0.config.poll_timeout);
        let r = req.into_inner();
        let ns = self.0.namespace(&p, &r.namespace).await?;
        let tq = r
            .task_queue
            .as_ref()
            .map(|t| t.name.clone())
            .filter(|n| !n.is_empty())
            .ok_or_else(|| Status::invalid_argument("TaskQueue is not set on request."))?;
        #[allow(deprecated)]
        let build_id = poll_build_id(
            r.worker_version_capabilities
                .as_ref()
                .map(|c| c.build_id.as_str()),
            r.deployment_options.as_ref().map(|d| d.build_id.as_str()),
        );
        let key = QueueKey {
            namespace_id: ns.namespace_id.clone(),
            task_queue: tq,
            kind: QueueKind::Activity,
        };
        let me = self.0.clone();
        let poll = async move {
            let deadline = tokio::time::Instant::now() + wait;
            loop {
                let left = deadline.saturating_duration_since(tokio::time::Instant::now());
                let Some(h) = me
                    .matcher
                    .poll(&key, left)
                    .await
                    .map_err(|e| Status::unavailable(e.to_string()))?
                else {
                    return Ok(PollActivityTaskQueueResponse::default());
                };
                let Handout::Claim(claim) = h else { continue };
                match me
                    .start_activity_task(&claim, &r.identity, &build_id)
                    .await?
                {
                    Some(resp) => return Ok(resp),
                    None => me.drop_claim(&claim).await,
                }
            }
        };
        match tokio::time::timeout(wait + Duration::from_millis(100), poll).await {
            Ok(r) => r.map(Response::new),
            Err(_) => Ok(Response::new(PollActivityTaskQueueResponse::default())),
        }
    }

    async fn respond_activity_task_completed(
        &self,
        req: Request<RespondActivityTaskCompletedRequest>,
    ) -> Result<Response<RespondActivityTaskCompletedResponse>, Status> {
        let p = self.0.principal(&req)?;
        let r = req.into_inner();
        let token = r.task_token.clone();
        self.0
            .finish_task(&p, &token, TaskKind::Activity, |t| {
                Input::ActivityCompleted {
                    scheduled_event_id: t.scheduled_event_id,
                    attempt: t.attempt,
                    result: r.result,
                    identity: r.identity,
                }
            })
            .await?;
        Ok(Response::new(RespondActivityTaskCompletedResponse {}))
    }

    async fn respond_activity_task_failed(
        &self,
        req: Request<RespondActivityTaskFailedRequest>,
    ) -> Result<Response<RespondActivityTaskFailedResponse>, Status> {
        let p = self.0.principal(&req)?;
        let r = req.into_inner();
        let token = r.task_token.clone();
        self.0
            .finish_task(&p, &token, TaskKind::Activity, |t| Input::ActivityFailed {
                scheduled_event_id: t.scheduled_event_id,
                attempt: t.attempt,
                failure: r.failure,
                identity: r.identity,
                last_heartbeat_details: r.last_heartbeat_details,
            })
            .await?;
        Ok(Response::new(RespondActivityTaskFailedResponse::default()))
    }

    async fn respond_activity_task_canceled(
        &self,
        req: Request<RespondActivityTaskCanceledRequest>,
    ) -> Result<Response<RespondActivityTaskCanceledResponse>, Status> {
        let p = self.0.principal(&req)?;
        let r = req.into_inner();
        let token = r.task_token.clone();
        self.0
            .finish_task(&p, &token, TaskKind::Activity, |t| {
                Input::ActivityCanceled {
                    scheduled_event_id: t.scheduled_event_id,
                    attempt: t.attempt,
                    details: r.details,
                    identity: r.identity,
                }
            })
            .await?;
        Ok(Response::new(RespondActivityTaskCanceledResponse {}))
    }

    async fn record_activity_task_heartbeat(
        &self,
        req: Request<RecordActivityTaskHeartbeatRequest>,
    ) -> Result<Response<RecordActivityTaskHeartbeatResponse>, Status> {
        let p = self.0.principal(&req)?;
        let r = req.into_inner();
        let token = r.task_token.clone();
        let reply = self
            .0
            .finish_task(&p, &token, TaskKind::Activity, |t| {
                Input::ActivityHeartbeat {
                    scheduled_event_id: t.scheduled_event_id,
                    attempt: t.attempt,
                    details: r.details,
                }
            })
            .await?;
        let cancel_requested = matches!(
            reply,
            Reply::Heartbeat {
                cancel_requested: true
            }
        );
        Ok(Response::new(RecordActivityTaskHeartbeatResponse {
            cancel_requested,
            ..Default::default()
        }))
    }

    async fn signal_workflow_execution(
        &self,
        req: Request<SignalWorkflowExecutionRequest>,
    ) -> Result<Response<SignalWorkflowExecutionResponse>, Status> {
        let p = self.0.principal(&req)?;
        let r = req.into_inner();
        let ns = self.0.namespace(&p, &r.namespace).await?;
        let key = self.0.run_key(&ns, r.workflow_execution.as_ref()).await?;
        if r.signal_name.is_empty() {
            return Err(Status::invalid_argument(
                "SignalName is not set on request.",
            ));
        }
        self.0
            .engine
            .execute(
                &key,
                &Frontend::ctx(&ns),
                Input::Signal {
                    name: r.signal_name,
                    input: r.input,
                    identity: r.identity,
                    header: r.header,
                    request_id: r.request_id,
                },
            )
            .await?;
        Ok(Response::new(SignalWorkflowExecutionResponse::default()))
    }

    async fn request_cancel_workflow_execution(
        &self,
        req: Request<RequestCancelWorkflowExecutionRequest>,
    ) -> Result<Response<RequestCancelWorkflowExecutionResponse>, Status> {
        let p = self.0.principal(&req)?;
        let r = req.into_inner();
        let ns = self.0.namespace(&p, &r.namespace).await?;
        let key = self.0.run_key(&ns, r.workflow_execution.as_ref()).await?;
        self.0
            .engine
            .execute(
                &key,
                &Frontend::ctx(&ns),
                Input::CancelRequested {
                    reason: r.reason,
                    identity: r.identity,
                },
            )
            .await?;
        Ok(Response::new(RequestCancelWorkflowExecutionResponse {}))
    }

    async fn terminate_workflow_execution(
        &self,
        req: Request<TerminateWorkflowExecutionRequest>,
    ) -> Result<Response<TerminateWorkflowExecutionResponse>, Status> {
        let p = self.0.principal(&req)?;
        let r = req.into_inner();
        let ns = self.0.namespace(&p, &r.namespace).await?;
        let key = self.0.run_key(&ns, r.workflow_execution.as_ref()).await?;
        self.0
            .engine
            .execute(
                &key,
                &Frontend::ctx(&ns),
                Input::Terminate {
                    reason: r.reason,
                    details: r.details,
                    identity: r.identity,
                },
            )
            .await?;
        Ok(Response::new(TerminateWorkflowExecutionResponse {}))
    }

    async fn get_workflow_execution_history(
        &self,
        req: Request<GetWorkflowExecutionHistoryRequest>,
    ) -> Result<Response<GetWorkflowExecutionHistoryResponse>, Status> {
        let p = self.0.principal(&req)?;
        let wait_max = poll_deadline(&req, self.0.config.history_wait);
        let r = req.into_inner();
        let ns = self.0.namespace(&p, &r.namespace).await?;
        let key = self.0.run_key(&ns, r.execution.as_ref()).await?;
        let close_only = r.history_event_filter_type == HistoryEventFilterType::CloseEvent as i32;
        let from = if r.next_page_token.len() == 8 {
            i64::from_be_bytes(r.next_page_token[..8].try_into().expect("8 bytes"))
        } else {
            1
        };
        let page = match r.maximum_page_size {
            n if n > 0 => n as usize,
            _ => self.0.config.history_page_size,
        };
        if r.wait_new_event {
            let mut rx = self.0.engine.watch(&key).await?;
            let deadline = tokio::time::Instant::now() + wait_max;
            loop {
                let progress = *rx.borrow_and_update();
                let ready = if close_only {
                    progress.closed
                } else {
                    progress.next_event_id > from || progress.closed
                };
                if ready {
                    break;
                }
                match tokio::time::timeout_at(deadline, rx.changed()).await {
                    Ok(Ok(())) => {}
                    // Deadline or watcher gone: answer what there is.
                    _ => {
                        if close_only {
                            // Not closed yet: an empty page *with* a token, so
                            // the SDK polls again. An empty token would end
                            // its iterator without the close event ("could
                            // not get last history event").
                            return Ok(Response::new(GetWorkflowExecutionHistoryResponse {
                                history: Some(History::default()),
                                next_page_token: from.to_be_bytes().to_vec().into(),
                                ..Default::default()
                            }));
                        }
                        break;
                    }
                }
            }
        }
        if close_only {
            let run = self.0.engine.load(&key).await?;
            if run.state.completion_event_id == 0 {
                return Ok(Response::new(GetWorkflowExecutionHistoryResponse {
                    history: Some(History::default()),
                    ..Default::default()
                }));
            }
            let events = self
                .0
                .engine
                .history(&key, run.state.completion_event_id, 1)
                .await?;
            return Ok(Response::new(GetWorkflowExecutionHistoryResponse {
                history: Some(History { events }),
                ..Default::default()
            }));
        }
        let events = self.0.engine.history(&key, from, page).await?;
        let run = self.0.engine.load(&key).await?;
        let next = events.last().map_or(from, |e| e.event_id + 1);
        let more = next < run.state.next_event_id
            || (r.wait_new_event && run.state.completion_event_id == 0);
        Ok(Response::new(GetWorkflowExecutionHistoryResponse {
            history: Some(History { events }),
            next_page_token: if more {
                next.to_be_bytes().to_vec().into()
            } else {
                Default::default()
            },
            ..Default::default()
        }))
    }

    async fn describe_workflow_execution(
        &self,
        req: Request<DescribeWorkflowExecutionRequest>,
    ) -> Result<Response<DescribeWorkflowExecutionResponse>, Status> {
        let p = self.0.principal(&req)?;
        let r = req.into_inner();
        let ns = self.0.namespace(&p, &r.namespace).await?;
        let key = self.0.run_key(&ns, r.execution.as_ref()).await?;
        let s = self.0.engine.load(&key).await?.state;
        let tq = TaskQueue {
            name: s.task_queue.clone(),
            kind: TaskQueueKind::Normal as i32,
            normal_name: String::new(),
        };
        let pending_activities = s
            .activities
            .iter()
            .map(|a| PendingActivityInfo {
                activity_id: a.activity_id.clone(),
                activity_type: Some(
                    loams_temporal_proto::temporal::api::common::v1::ActivityType {
                        name: a.activity_type.clone(),
                    },
                ),
                state: if a.cancel_requested {
                    PendingActivityState::CancelRequested as i32
                } else if a.started {
                    PendingActivityState::Started as i32
                } else {
                    PendingActivityState::Scheduled as i32
                },
                heartbeat_details: a.heartbeat_details.clone(),
                last_heartbeat_time: a.last_heartbeat_time,
                last_started_time: a.started_time,
                attempt: a.attempt,
                scheduled_time: a.attempt_scheduled_time,
                last_worker_identity: a.started_identity.clone(),
                last_failure: a.last_failure.clone(),
                ..Default::default()
            })
            .collect();
        let pending_workflow_task = s.workflow_task.as_ref().map(|w| PendingWorkflowTaskInfo {
            state: if w.started_event_id != 0 {
                PendingWorkflowTaskState::Started as i32
            } else {
                PendingWorkflowTaskState::Scheduled as i32
            },
            scheduled_time: w.scheduled_time,
            original_scheduled_time: w.scheduled_time,
            started_time: w.started_time,
            attempt: w.attempt,
        });
        Ok(Response::new(DescribeWorkflowExecutionResponse {
            execution_config: Some(WorkflowExecutionConfig {
                task_queue: Some(tq.clone()),
                workflow_execution_timeout: s.workflow_execution_timeout,
                workflow_run_timeout: s.workflow_run_timeout,
                default_workflow_task_timeout: s.workflow_task_timeout,
                ..Default::default()
            }),
            workflow_execution_info: Some(WorkflowExecutionInfo {
                execution: Some(WorkflowExecution {
                    workflow_id: key.workflow_id.clone(),
                    run_id: key.run_id.clone(),
                }),
                r#type: Some(WorkflowType {
                    name: s.workflow_type.clone(),
                }),
                start_time: s.start_time,
                close_time: s.close_time,
                status: s.status,
                history_length: s.next_event_id - 1,
                execution_time: s.start_time,
                task_queue: s.task_queue.clone(),
                history_size_bytes: s.history_size_bytes,
                first_run_id: s.first_execution_run_id.clone(),
                ..Default::default()
            }),
            pending_activities,
            pending_workflow_task,
            ..Default::default()
        }))
    }

    async fn query_workflow(
        &self,
        req: Request<QueryWorkflowRequest>,
    ) -> Result<Response<QueryWorkflowResponse>, Status> {
        let p = self.0.principal(&req)?;
        let wait = poll_deadline(&req, self.0.config.query_timeout);
        let r = req.into_inner();
        let ns = self.0.namespace(&p, &r.namespace).await?;
        let key = self.0.run_key(&ns, r.execution.as_ref()).await?;
        let query = r
            .query
            .ok_or_else(|| Status::invalid_argument("Query is not set on request."))?;
        let state = self.0.engine.load(&key).await?.state;
        let (id, rx) = self.0.queries.register(QueryTask {
            key: key.clone(),
            loams_namespace_id: ns.loams_namespace_id.clone(),
            query,
        });
        let queue = QueueKey {
            namespace_id: ns.namespace_id.clone(),
            task_queue: state.task_queue.clone(),
            kind: QueueKind::Workflow,
        };
        self.0.matcher.offer_local(&queue, id);
        let answer = tokio::time::timeout(wait, rx).await;
        self.0.matcher.cancel_local(&queue, id);
        self.0.queries.remove(id);
        match answer {
            Ok(Ok(QueryAnswer::Answered(result))) => Ok(Response::new(QueryWorkflowResponse {
                query_result: result,
                ..Default::default()
            })),
            Ok(Ok(QueryAnswer::Failed(message))) => Err(Status::invalid_argument(message)),
            _ => Err(Status::deadline_exceeded(
                "no worker answered the query in time (is a worker polling the task queue?)",
            )),
        }
    }

    async fn respond_query_task_completed(
        &self,
        req: Request<RespondQueryTaskCompletedRequest>,
    ) -> Result<Response<RespondQueryTaskCompletedResponse>, Status> {
        let p = self.0.principal(&req)?;
        let r = req.into_inner();
        let (token, _, _) = self.0.token_run(&p, &r.task_token, TaskKind::Query).await?;
        let answer = if r.completed_type == QueryResultType::Answered as i32 {
            QueryAnswer::Answered(r.query_result)
        } else {
            QueryAnswer::Failed(r.error_message)
        };
        if !self
            .0
            .queries
            .complete(token.scheduled_event_id as u64, answer)
        {
            return Err(Status::not_found("query not found or already timed out"));
        }
        Ok(Response::new(RespondQueryTaskCompletedResponse {}))
    }

    async fn reset_sticky_task_queue(
        &self,
        req: Request<ResetStickyTaskQueueRequest>,
    ) -> Result<Response<ResetStickyTaskQueueResponse>, Status> {
        self.0.principal(&req)?;
        // No sticky queues in DX1 (Q1156): nothing to reset.
        Ok(Response::new(ResetStickyTaskQueueResponse {}))
    }
}

#[cfg(test)]
mod tests {
    use super::parse_grpc_timeout;
    use std::time::Duration;

    #[test]
    fn grpc_timeouts_parse() {
        assert_eq!(parse_grpc_timeout("70S"), Some(Duration::from_secs(70)));
        assert_eq!(
            parse_grpc_timeout("69999m"),
            Some(Duration::from_millis(69_999))
        );
        assert_eq!(parse_grpc_timeout("x"), None);
        assert_eq!(parse_grpc_timeout(""), None);
    }
}
