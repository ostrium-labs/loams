//! `StartWorkflowExecution`: the run's first two events.

use loams_temporal_history::time::{dur, dur_ms, ts};
use loams_temporal_history::{Attributes, Batch, MutableState, STATE_VERSION};
use loams_temporal_proto::temporal::api::enums::v1::{TaskQueueKind, WorkflowExecutionStatus};
use loams_temporal_proto::temporal::api::history::v1::{
    WorkflowExecutionSignaledEventAttributes, WorkflowExecutionStartedEventAttributes,
};
use loams_temporal_proto::temporal::api::taskqueue::v1::TaskQueue;
use loams_temporal_proto::temporal::api::workflowservice::v1::StartWorkflowExecutionRequest;

use crate::write::schedule_workflow_task;
use crate::{Effect, Rejection, Reply, StartSignal};

/// A workflow task's start-to-close timeout when the request sets none.
pub const DEFAULT_WORKFLOW_TASK_TIMEOUT_MS: i64 = 10_000;
/// The longest workflow task timeout Temporal accepts.
pub const MAX_WORKFLOW_TASK_TIMEOUT_MS: i64 = 120_000;

pub(crate) fn start(
    b: &mut Batch<'_>,
    fx: &mut Vec<Effect>,
    namespace_id: &str,
    run_id: &str,
    r: StartWorkflowExecutionRequest,
    signal: Option<StartSignal>,
    chain: Option<StartChain>,
) -> Result<Reply, Rejection> {
    let chain = chain.unwrap_or_default();
    if let Some(sig) = &signal
        && sig.name.is_empty()
    {
        return Err(Rejection::InvalidArgument(
            "SignalName is not set on request.".into(),
        ));
    }
    if r.workflow_id.is_empty() {
        return Err(Rejection::InvalidArgument(
            "WorkflowId is not set on request.".into(),
        ));
    }
    let workflow_type = r
        .workflow_type
        .clone()
        .filter(|t| !t.name.is_empty())
        .ok_or_else(|| Rejection::InvalidArgument("WorkflowType is not set on request.".into()))?;
    let task_queue = r
        .task_queue
        .as_ref()
        .map(|t| t.name.clone())
        .filter(|n| !n.is_empty())
        .ok_or_else(|| Rejection::InvalidArgument("TaskQueue is not set on request.".into()))?;
    if r.workflow_start_delay
        .is_some_and(|d| d.seconds > 0 || d.nanos > 0)
    {
        return Err(Rejection::Unimplemented(
            "workflow_start_delay is not supported yet (DX2)".into(),
        ));
    }
    if !r.cron_schedule.is_empty() {
        return Err(Rejection::Unimplemented(
            "cron_schedule is not supported".into(),
        ));
    }
    let mut wft_ms = dur_ms(r.workflow_task_timeout.as_ref());
    if wft_ms <= 0 {
        wft_ms = DEFAULT_WORKFLOW_TASK_TIMEOUT_MS;
    }
    wft_ms = wft_ms.min(MAX_WORKFLOW_TASK_TIMEOUT_MS);
    let now = b.now_ms();

    *b.state() = MutableState {
        version: STATE_VERSION,
        namespace_id: namespace_id.to_string(),
        workflow_id: r.workflow_id.clone(),
        run_id: run_id.to_string(),
        workflow_type: workflow_type.name.clone(),
        task_queue: task_queue.clone(),
        workflow_task_timeout: Some(dur(wft_ms)),
        workflow_run_timeout: r.workflow_run_timeout,
        workflow_execution_timeout: r.workflow_execution_timeout,
        start_time: Some(ts(now)),
        status: WorkflowExecutionStatus::Running as i32,
        next_event_id: 1,
        start_request_id: r.request_id.clone(),
        first_execution_run_id: if chain.first_execution_run_id.is_empty() {
            run_id.to_string()
        } else {
            chain.first_execution_run_id.clone()
        },
        namespace_name: r.namespace.clone(),
        parent: chain.parent.clone(),
        memo: r.memo.clone(),
        search_attributes: r.search_attributes.clone(),
        first_task_backoff: chain.backoff_ms > 0,
        ..Default::default()
    };
    let first_execution_run_id = b.state_ref().first_execution_run_id.clone();
    b.push(Attributes::WorkflowExecutionStartedEventAttributes(
        WorkflowExecutionStartedEventAttributes {
            workflow_type: Some(workflow_type),
            task_queue: Some(TaskQueue {
                name: task_queue,
                kind: TaskQueueKind::Normal as i32,
                normal_name: String::new(),
            }),
            input: r.input,
            workflow_execution_timeout: r.workflow_execution_timeout,
            workflow_run_timeout: r.workflow_run_timeout,
            workflow_task_timeout: Some(dur(wft_ms)),
            original_execution_run_id: run_id.to_string(),
            identity: r.identity,
            first_execution_run_id,
            retry_policy: r.retry_policy,
            attempt: 1,
            first_workflow_task_backoff: Some(dur(chain.backoff_ms.max(0))),
            parent_workflow_namespace: chain
                .parent
                .as_ref()
                .map(|p| p.namespace.clone())
                .unwrap_or_default(),
            parent_workflow_namespace_id: chain
                .parent
                .as_ref()
                .map(|p| p.namespace_id.clone())
                .unwrap_or_default(),
            parent_workflow_execution: chain.parent.as_ref().map(|p| {
                loams_temporal_proto::temporal::api::common::v1::WorkflowExecution {
                    workflow_id: p.workflow_id.clone(),
                    run_id: p.run_id.clone(),
                }
            }),
            parent_initiated_event_id: chain.parent.as_ref().map_or(0, |p| p.initiated_event_id),
            continued_execution_run_id: chain.continued_execution_run_id.clone(),
            initiator: chain.initiator,
            continued_failure: chain.continued_failure.clone(),
            last_completion_result: chain.last_completion_result.clone(),
            prev_auto_reset_points: chain.prev_auto_reset_points.clone(),
            memo: r.memo,
            search_attributes: r.search_attributes,
            header: Some(r.header.unwrap_or_default()),
            workflow_id: r.workflow_id,
            priority: r.priority,
            ..Default::default()
        },
    ));
    if let Some(sig) = signal {
        b.state().signal_count += 1;
        b.push(Attributes::WorkflowExecutionSignaledEventAttributes(
            WorkflowExecutionSignaledEventAttributes {
                signal_name: sig.name,
                input: sig.input,
                identity: sig.identity,
                header: Some(sig.header.unwrap_or_default()),
                request_id: sig.request_id,
                ..Default::default()
            },
        ));
    }
    if chain.backoff_ms > 0 {
        // The first workflow task waits for the backoff: a timer on the
        // started event (ID 1, which no user timer can have).
        let now = b.now_ms();
        fx.push(Effect::StartTimer {
            started_event_id: 1,
            fire_at_ms: now + chain.backoff_ms,
        });
    } else {
        schedule_workflow_task(b, fx, 1);
    }
    Ok(Reply::Done)
}

/// A run's lineage: its parent, the run it continues, its first task's
/// backoff (TMP-api D2087).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StartChain {
    /// The parent, for a child workflow.
    pub parent: Option<loams_temporal_history::ParentInfo>,
    /// The run this one continues (continue-as-new).
    pub continued_execution_run_id: String,
    /// `temporal.api.enums.v1.ContinueAsNewInitiator`.
    pub initiator: i32,
    /// The first run of the chain (empty: this one).
    pub first_execution_run_id: String,
    /// The first workflow task's backoff.
    pub backoff_ms: i64,
    /// The previous run's result (cron) or failure (retry).
    pub last_completion_result: Option<loams_temporal_proto::temporal::api::common::v1::Payloads>,
    /// The previous run's failure.
    pub continued_failure: Option<loams_temporal_proto::temporal::api::failure::v1::Failure>,
    /// The previous runs' reset points.
    pub prev_auto_reset_points:
        Option<loams_temporal_proto::temporal::api::workflow::v1::ResetPoints>,
}
