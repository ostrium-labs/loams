//! Batch operations (TMP-api group F, D2230–D2231): terminate, cancel or
//! signal a list of workflow executions.
//!
//! What is built, honestly:
//! - Targets are the executions the request lists (`target_executions`, or
//!   the deprecated `executions`). A `visibility_query` needs the list
//!   filter grammar (lane TMP-sched) and is refused with `INVALID_ARGUMENT`
//!   until then.
//! - Terminate, cancel and signal. Deletion, reset and the activity and
//!   options operations answer `UNIMPLEMENTED`.
//! - Jobs live in this gateway's memory: `DescribeBatchOperation` and
//!   `ListBatchOperations` see the jobs started since it last started. Each
//!   step is idempotent, because a signal carries the request ID
//!   `batch:<job>:<workflow>:<run>`, which the target deduplicates, and
//!   terminate and cancel of a closed or already-cancelled run change
//!   nothing. So starting the same job again after a restart finishes it
//!   without doubling anything.
//! - `max_operations_per_second` is honoured (default 50 per second).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use loams_temporal_history::time::ts;
use loams_temporal_proto::temporal::api::batch::v1::BatchOperationInfo;
use loams_temporal_proto::temporal::api::common::v1::{Execution, WorkflowExecution};
use loams_temporal_proto::temporal::api::enums::v1::{
    BatchOperationState, BatchOperationType, ExecutionType,
};
use loams_temporal_proto::temporal::api::workflowservice::v1::start_batch_operation_request::Operation;
use loams_temporal_proto::temporal::api::workflowservice::v1::{
    DescribeBatchOperationRequest, DescribeBatchOperationResponse, ListBatchOperationsRequest,
    ListBatchOperationsResponse, StartBatchOperationRequest, StartBatchOperationResponse,
    StopBatchOperationRequest, StopBatchOperationResponse,
};
use loams_temporal_statemachine::Input;
use loams_temporal_store::{NamespaceRecord, RunKey};
use tonic::Status;

use crate::auth::Principal;
use crate::engine::store_status;
use crate::service::Frontend;

/// The rate when the request names none.
pub const DEFAULT_OPS_PER_SECOND: f32 = 50.0;
/// The most targets one job takes.
pub const MAX_TARGETS: usize = 10_000;

/// What a job does to each target.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum BatchAction {
    Terminate {
        reason: String,
        details: Option<loams_temporal_proto::temporal::api::common::v1::Payloads>,
        identity: String,
    },
    Cancel {
        reason: String,
        identity: String,
    },
    Signal {
        name: String,
        input: Option<loams_temporal_proto::temporal::api::common::v1::Payloads>,
        header: Option<loams_temporal_proto::temporal::api::common::v1::Header>,
        identity: String,
    },
}

impl BatchAction {
    fn kind(&self) -> BatchOperationType {
        match self {
            BatchAction::Terminate { .. } => BatchOperationType::TerminateWorkflow,
            BatchAction::Cancel { .. } => BatchOperationType::CancelWorkflow,
            BatchAction::Signal { .. } => BatchOperationType::SignalWorkflow,
        }
    }

    /// The input for one target of job `job_id`.
    pub(crate) fn input(&self, job_id: &str, target: &WorkflowExecution) -> Input {
        match self.clone() {
            BatchAction::Terminate {
                reason,
                details,
                identity,
            } => Input::Terminate {
                reason,
                details,
                identity,
            },
            BatchAction::Cancel { reason, identity } => Input::CancelRequested { reason, identity },
            BatchAction::Signal {
                name,
                input,
                header,
                identity,
            } => Input::Signal {
                name,
                input,
                identity,
                header,
                request_id: format!("batch:{job_id}:{}:{}", target.workflow_id, target.run_id),
            },
        }
    }
}

/// The action a request asks for.
pub(crate) fn action(r: &StartBatchOperationRequest) -> Result<BatchAction, Status> {
    match r.operation.as_ref() {
        Some(Operation::TerminationOperation(t)) => Ok(BatchAction::Terminate {
            reason: r.reason.clone(),
            details: t.details.clone(),
            identity: t.identity.clone(),
        }),
        Some(Operation::CancellationOperation(c)) => Ok(BatchAction::Cancel {
            reason: r.reason.clone(),
            identity: c.identity.clone(),
        }),
        Some(Operation::SignalOperation(s)) => {
            if s.signal.is_empty() {
                return Err(Status::invalid_argument(
                    "Signal name is not set on request.",
                ));
            }
            Ok(BatchAction::Signal {
                name: s.signal.clone(),
                input: s.input.clone(),
                header: s.header.clone(),
                identity: s.identity.clone(),
            })
        }
        Some(_) => Err(Status::unimplemented(
            "the Loams Temporal gateway runs terminate, cancel and signal batch operations only",
        )),
        None => Err(Status::invalid_argument(
            "Batch operation is not set on request.",
        )),
    }
}

/// The workflow executions a request lists.
pub(crate) fn targets(r: &StartBatchOperationRequest) -> Result<Vec<WorkflowExecution>, Status> {
    if !r.visibility_query.is_empty() {
        return Err(Status::invalid_argument(
            "batch operations by visibility query are not supported yet; list the executions",
        ));
    }
    let mut out: Vec<WorkflowExecution> = r
        .target_executions
        .iter()
        .filter(|e: &&Execution| {
            matches!(
                ExecutionType::try_from(e.r#type).unwrap_or_default(),
                ExecutionType::Unspecified | ExecutionType::Workflow
            )
        })
        .map(|e| WorkflowExecution {
            workflow_id: e.business_id.clone(),
            run_id: e.run_id.clone(),
        })
        .collect();
    #[allow(deprecated)]
    out.extend(r.executions.iter().cloned());
    out.retain(|e| !e.workflow_id.is_empty());
    out.dedup();
    if out.is_empty() {
        return Err(Status::invalid_argument(
            "Executions are not set on request.",
        ));
    }
    if out.len() > MAX_TARGETS {
        return Err(Status::invalid_argument(format!(
            "a batch operation takes at most {MAX_TARGETS} executions"
        )));
    }
    Ok(out)
}

/// One job's record.
#[derive(Debug)]
pub(crate) struct Job {
    namespace_id: String,
    kind: BatchOperationType,
    targets: Vec<WorkflowExecution>,
    identity: String,
    reason: String,
    start_ms: i64,
    close_ms: AtomicI64,
    completed: AtomicI64,
    failed: AtomicI64,
    stop: AtomicBool,
    state: Mutex<BatchOperationState>,
}

impl Job {
    fn state(&self) -> BatchOperationState {
        *self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn info(&self, job_id: &str) -> BatchOperationInfo {
        let close = self.close_ms.load(Ordering::Relaxed);
        BatchOperationInfo {
            job_id: job_id.to_string(),
            state: self.state() as i32,
            start_time: Some(ts(self.start_ms)),
            close_time: (close > 0).then(|| ts(close)),
            operation_type: self.kind as i32,
        }
    }
}

/// The jobs this gateway runs or ran.
#[derive(Debug, Default)]
pub struct Batches {
    jobs: Mutex<BTreeMap<String, Arc<Job>>>,
}

impl Batches {
    fn get(&self, namespace_id: &str, job_id: &str) -> Option<Arc<Job>> {
        self.jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(job_id)
            .filter(|j| j.namespace_id == namespace_id)
            .cloned()
    }
}

impl Frontend {
    /// `StartBatchOperation`.
    pub(crate) async fn start_batch(
        self: &Arc<Self>,
        p: &Principal,
        r: StartBatchOperationRequest,
    ) -> Result<StartBatchOperationResponse, Status> {
        let ns = self.namespace(p, &r.namespace).await?;
        if r.job_id.is_empty() {
            return Err(Status::invalid_argument("JobId is not set on request."));
        }
        if r.reason.is_empty() {
            return Err(Status::invalid_argument("Reason is not set on request."));
        }
        let action = action(&r)?;
        let targets = targets(&r)?;
        let identity = match &action {
            BatchAction::Terminate { identity, .. }
            | BatchAction::Cancel { identity, .. }
            | BatchAction::Signal { identity, .. } => identity.clone(),
        };
        let job = Arc::new(Job {
            namespace_id: ns.namespace_id.clone(),
            kind: action.kind(),
            targets: targets.clone(),
            identity,
            reason: r.reason.clone(),
            start_ms: self.engine.now_ms(),
            close_ms: AtomicI64::new(0),
            completed: AtomicI64::new(0),
            failed: AtomicI64::new(0),
            stop: AtomicBool::new(false),
            state: Mutex::new(BatchOperationState::Running),
        });
        {
            let mut jobs = self
                .batches
                .jobs
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(existing) = jobs.get(&r.job_id)
                && existing.state() == BatchOperationState::Running
            {
                return Err(Status::already_exists(format!(
                    "batch operation {} is already running",
                    r.job_id
                )));
            }
            jobs.insert(r.job_id.clone(), job.clone());
        }
        let rate = if r.max_operations_per_second > 0.0 {
            r.max_operations_per_second
        } else {
            DEFAULT_OPS_PER_SECOND
        };
        let me = self.clone();
        let job_id = r.job_id.clone();
        tokio::spawn(async move { me.run_batch(ns, job_id, job, action, rate).await });
        Ok(StartBatchOperationResponse {})
    }

    async fn run_batch(
        &self,
        ns: NamespaceRecord,
        job_id: String,
        job: Arc<Job>,
        action: BatchAction,
        rate: f32,
    ) {
        let gap = Duration::from_secs_f64(1.0 / f64::from(rate.max(0.001)));
        let ctx = Self::ctx(&ns);
        for t in &job.targets {
            if job.stop.load(Ordering::Relaxed) {
                break;
            }
            let ok = match self.batch_target(&ns, t).await {
                Ok(key) => match self
                    .engine
                    .execute(&key, &ctx, action.input(&job_id, t))
                    .await
                {
                    Ok(_) => true,
                    // Temporal counts an already-closed target as done for
                    // terminate and cancel.
                    Err(s) if s.code() == tonic::Code::NotFound => {
                        !matches!(action, BatchAction::Signal { .. })
                    }
                    Err(_) => false,
                },
                Err(_) => false,
            };
            if ok {
                job.completed.fetch_add(1, Ordering::Relaxed);
            } else {
                job.failed.fetch_add(1, Ordering::Relaxed);
            }
            tokio::time::sleep(gap).await;
        }
        let state = if job.stop.load(Ordering::Relaxed) || job.failed.load(Ordering::Relaxed) > 0 {
            BatchOperationState::Failed
        } else {
            BatchOperationState::Completed
        };
        *job.state.lock().unwrap_or_else(PoisonError::into_inner) = state;
        job.close_ms.store(self.engine.now_ms(), Ordering::Relaxed);
    }

    async fn batch_target(
        &self,
        ns: &NamespaceRecord,
        t: &WorkflowExecution,
    ) -> Result<RunKey, Status> {
        let run_id = if t.run_id.is_empty() {
            self.store()
                .current_run(&ns.namespace_id, &t.workflow_id)
                .await
                .map_err(store_status)?
                .ok_or_else(|| Status::not_found("workflow execution not found"))?
                .run_id
        } else {
            t.run_id.clone()
        };
        Ok(RunKey::new(&ns.namespace_id, &t.workflow_id, run_id))
    }

    /// `StopBatchOperation`.
    pub(crate) async fn stop_batch(
        &self,
        p: &Principal,
        r: StopBatchOperationRequest,
    ) -> Result<StopBatchOperationResponse, Status> {
        let ns = self.namespace(p, &r.namespace).await?;
        let job = self
            .batches
            .get(&ns.namespace_id, &r.job_id)
            .ok_or_else(|| Status::not_found(format!("batch operation {} not found", r.job_id)))?;
        job.stop.store(true, Ordering::Relaxed);
        Ok(StopBatchOperationResponse {})
    }

    /// `DescribeBatchOperation`.
    pub(crate) async fn describe_batch(
        &self,
        p: &Principal,
        r: DescribeBatchOperationRequest,
    ) -> Result<DescribeBatchOperationResponse, Status> {
        let ns = self.namespace(p, &r.namespace).await?;
        let job = self
            .batches
            .get(&ns.namespace_id, &r.job_id)
            .ok_or_else(|| Status::not_found(format!("batch operation {} not found", r.job_id)))?;
        let info = job.info(&r.job_id);
        Ok(DescribeBatchOperationResponse {
            operation_type: info.operation_type,
            job_id: r.job_id,
            state: info.state,
            start_time: info.start_time,
            close_time: info.close_time,
            total_operation_count: job.targets.len() as i64,
            complete_operation_count: job.completed.load(Ordering::Relaxed),
            failure_operation_count: job.failed.load(Ordering::Relaxed),
            identity: job.identity.clone(),
            reason: job.reason.clone(),
            ..Default::default()
        })
    }

    /// `ListBatchOperations`: one page, newest job IDs last (by ID).
    pub(crate) async fn list_batches(
        &self,
        p: &Principal,
        r: ListBatchOperationsRequest,
    ) -> Result<ListBatchOperationsResponse, Status> {
        let ns = self.namespace(p, &r.namespace).await?;
        let jobs = self
            .batches
            .jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        Ok(ListBatchOperationsResponse {
            operation_info: jobs
                .iter()
                .filter(|(_, j)| j.namespace_id == ns.namespace_id)
                .map(|(id, j)| j.info(id))
                .collect(),
            next_page_token: Default::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loams_temporal_proto::temporal::api::batch::v1::{
        BatchOperationDeletion, BatchOperationSignal, BatchOperationTermination,
    };

    fn req(op: Operation) -> StartBatchOperationRequest {
        StartBatchOperationRequest {
            namespace: "default".into(),
            job_id: "j1".into(),
            reason: "r".into(),
            target_executions: vec![
                Execution {
                    r#type: ExecutionType::Workflow as i32,
                    business_id: "a".into(),
                    run_id: String::new(),
                },
                Execution {
                    r#type: ExecutionType::Activity as i32,
                    business_id: "act".into(),
                    run_id: String::new(),
                },
            ],
            operation: Some(op),
            ..Default::default()
        }
    }

    #[test]
    fn requests_map_to_actions_and_targets() {
        let r = req(Operation::TerminationOperation(
            BatchOperationTermination::default(),
        ));
        assert!(matches!(action(&r), Ok(BatchAction::Terminate { .. })));
        let t = targets(&r).expect("targets");
        assert_eq!(t.len(), 1, "activities are not workflow targets");
        assert_eq!(t[0].workflow_id, "a");

        let bad = req(Operation::DeletionOperation(
            BatchOperationDeletion::default(),
        ));
        assert_eq!(
            action(&bad).expect_err("unsupported").code(),
            tonic::Code::Unimplemented
        );

        let mut q = r.clone();
        q.visibility_query = "WorkflowType = 'x'".into();
        assert_eq!(
            targets(&q).expect_err("query").code(),
            tonic::Code::InvalidArgument
        );
    }

    #[test]
    fn a_batch_signal_carries_a_deterministic_request_id() {
        let r = req(Operation::SignalOperation(BatchOperationSignal {
            signal: "go".into(),
            ..Default::default()
        }));
        let a = action(&r).expect("signal");
        let target = WorkflowExecution {
            workflow_id: "a".into(),
            run_id: "r1".into(),
        };
        match a.input("j1", &target) {
            Input::Signal { request_id, .. } => assert_eq!(request_id, "batch:j1:a:r1"),
            other => panic!("{other:?}"),
        }
    }
}
