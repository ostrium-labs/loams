//! `StartWorkflowExecution`: the run's first two events.

use loams_temporal_history::time::{dur, dur_ms, ts};
use loams_temporal_history::{Attributes, Batch, MutableState, STATE_VERSION};
use loams_temporal_proto::temporal::api::enums::v1::{TaskQueueKind, WorkflowExecutionStatus};
use loams_temporal_proto::temporal::api::history::v1::WorkflowExecutionStartedEventAttributes;
use loams_temporal_proto::temporal::api::taskqueue::v1::TaskQueue;
use loams_temporal_proto::temporal::api::workflowservice::v1::StartWorkflowExecutionRequest;

use crate::write::schedule_workflow_task;
use crate::{Effect, Rejection, Reply};

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
) -> Result<Reply, Rejection> {
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
        first_execution_run_id: run_id.to_string(),
        ..Default::default()
    };
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
            first_execution_run_id: run_id.to_string(),
            retry_policy: r.retry_policy,
            attempt: 1,
            first_workflow_task_backoff: Some(dur(0)),
            memo: r.memo,
            search_attributes: r.search_attributes,
            header: Some(r.header.unwrap_or_default()),
            workflow_id: r.workflow_id,
            priority: r.priority,
            ..Default::default()
        },
    ));
    schedule_workflow_task(b, fx, 1);
    Ok(Reply::Done)
}
