//! The workflow task: started by a poll, then completed, failed or timed out.

use loams_temporal_history::time::{dur_ms, ms, ts};
use loams_temporal_history::{Attributes, Batch};
use loams_temporal_proto::temporal::api::command::v1::{Command, command};
use loams_temporal_proto::temporal::api::common::v1::{MeteringMetadata, WorkerVersionStamp};
use loams_temporal_proto::temporal::api::enums::v1::{TimeoutType, WorkflowTaskFailedCause};
use loams_temporal_proto::temporal::api::failure::v1::Failure;
use loams_temporal_proto::temporal::api::history::v1::{
    WorkflowTaskCompletedEventAttributes, WorkflowTaskFailedEventAttributes,
    WorkflowTaskStartedEventAttributes, WorkflowTaskTimedOutEventAttributes,
};
use loams_temporal_proto::temporal::api::sdk::v1::WorkflowTaskCompletedMetadata;

use crate::write::{ensure_workflow_task, flush, schedule_workflow_task};
use crate::{Effect, Rejection, Reply, TimeoutKind, commands};

fn stale(scheduled_event_id: i64) -> Rejection {
    Rejection::NotFound(format!(
        "Workflow task not found (scheduled event {scheduled_event_id})."
    ))
}

pub(crate) fn started(
    b: &mut Batch<'_>,
    fx: &mut Vec<Effect>,
    scheduled_event_id: i64,
    identity: String,
    request_id: String,
    worker_version: Option<WorkerVersionStamp>,
) -> Result<Reply, Rejection> {
    let previous = b.state_ref().last_completed_wft_started_event_id;
    let size = b.state_ref().history_size_bytes;
    let wt = b
        .state_ref()
        .workflow_task
        .clone()
        .filter(|w| w.scheduled_event_id == scheduled_event_id)
        .ok_or_else(|| stale(scheduled_event_id))?;
    if wt.started_event_id != 0 {
        // A retried poll (same request ID) gets the same task back.
        if wt.request_id == request_id {
            return Ok(Reply::WorkflowTaskStarted {
                scheduled_event_id,
                started_event_id: wt.started_event_id,
                attempt: wt.attempt,
                previous_started_event_id: previous,
                scheduled_time_ms: wt.scheduled_time.as_ref().map_or(0, ms),
                started_time_ms: wt.started_time.as_ref().map_or(0, ms),
            });
        }
        return Err(Rejection::NotFound("Workflow task already started.".into()));
    }
    let id = b.push(Attributes::WorkflowTaskStartedEventAttributes(
        WorkflowTaskStartedEventAttributes {
            scheduled_event_id,
            identity: identity.clone(),
            request_id: request_id.clone(),
            history_size_bytes: size,
            worker_version,
            ..Default::default()
        },
    ));
    let now = b.now_ms();
    let w = b.state().workflow_task.as_mut().expect("checked above");
    w.started_event_id = id;
    w.identity = identity;
    w.request_id = request_id;
    w.started_time = Some(ts(now));
    let timeout = dur_ms(w.start_to_close_timeout.as_ref());
    fx.push(Effect::ArmTimeout {
        kind: TimeoutKind::WorkflowTask,
        event_id: scheduled_event_id,
        attempt: wt.attempt,
        at_ms: now + timeout,
    });
    Ok(Reply::WorkflowTaskStarted {
        scheduled_event_id,
        started_event_id: id,
        attempt: wt.attempt,
        previous_started_event_id: previous,
        scheduled_time_ms: wt.scheduled_time.as_ref().map_or(0, ms),
        started_time_ms: now,
    })
}

/// The started workflow task for a worker's answer, or the stale-token
/// rejection.
fn take_started(
    b: &mut Batch<'_>,
    scheduled_event_id: i64,
    attempt: i32,
) -> Result<loams_temporal_history::WorkflowTaskInfo, Rejection> {
    let wt = b
        .state_ref()
        .workflow_task
        .clone()
        .filter(|w| {
            w.scheduled_event_id == scheduled_event_id
                && w.attempt == attempt
                && w.started_event_id != 0
        })
        .ok_or_else(|| stale(scheduled_event_id))?;
    Ok(wt)
}

pub(crate) struct Completion {
    pub scheduled_event_id: i64,
    pub attempt: i32,
    pub identity: String,
    pub commands: Vec<Command>,
    pub sdk_metadata: Option<WorkflowTaskCompletedMetadata>,
    pub metering_metadata: Option<MeteringMetadata>,
    pub worker_version: Option<WorkerVersionStamp>,
    pub force_create_new_workflow_task: bool,
}

fn is_close(c: &Command) -> bool {
    matches!(
        c.attributes,
        Some(
            command::Attributes::CompleteWorkflowExecutionCommandAttributes(_)
                | command::Attributes::FailWorkflowExecutionCommandAttributes(_)
                | command::Attributes::CancelWorkflowExecutionCommandAttributes(_)
                | command::Attributes::ContinueAsNewWorkflowExecutionCommandAttributes(_)
        )
    )
}

pub(crate) fn completed(
    b: &mut Batch<'_>,
    fx: &mut Vec<Effect>,
    c: Completion,
) -> Result<Reply, Rejection> {
    let wt = take_started(b, c.scheduled_event_id, c.attempt)?;
    commands::validate(&c.commands)?;
    fx.push(Effect::WorkflowTaskDone {
        scheduled_event_id: wt.scheduled_event_id,
        attempt: wt.attempt,
    });

    // A close command while new events wait would hide them from the
    // workflow: fail the task so the worker sees them first.
    if c.commands.iter().any(is_close) && !b.state_ref().buffered.is_empty() {
        b.push(Attributes::WorkflowTaskFailedEventAttributes(
            WorkflowTaskFailedEventAttributes {
                scheduled_event_id: wt.scheduled_event_id,
                started_event_id: wt.started_event_id,
                cause: WorkflowTaskFailedCause::UnhandledCommand as i32,
                identity: c.identity,
                ..Default::default()
            },
        ));
        b.state().workflow_task = None;
        flush(b);
        ensure_workflow_task(b, fx);
        return Ok(Reply::Done);
    }

    let completed_id = b.push(Attributes::WorkflowTaskCompletedEventAttributes(
        WorkflowTaskCompletedEventAttributes {
            scheduled_event_id: wt.scheduled_event_id,
            started_event_id: wt.started_event_id,
            identity: c.identity.clone(),
            worker_version: c.worker_version,
            sdk_metadata: Some(c.sdk_metadata.unwrap_or_default()),
            metering_metadata: Some(c.metering_metadata.unwrap_or_default()),
            ..Default::default()
        },
    ));
    {
        let s = b.state();
        s.last_completed_wft_started_event_id = wt.started_event_id;
        s.workflow_task = None;
    }
    for command in c.commands {
        commands::apply(b, fx, command, completed_id, &c.identity)?;
    }
    let had_buffered = flush(b);
    if had_buffered || c.force_create_new_workflow_task {
        ensure_workflow_task(b, fx);
    }
    Ok(Reply::Done)
}

pub(crate) fn failed(
    b: &mut Batch<'_>,
    fx: &mut Vec<Effect>,
    scheduled_event_id: i64,
    attempt: i32,
    cause: i32,
    failure: Option<Failure>,
    identity: String,
) -> Result<Reply, Rejection> {
    let wt = take_started(b, scheduled_event_id, attempt)?;
    b.push(Attributes::WorkflowTaskFailedEventAttributes(
        WorkflowTaskFailedEventAttributes {
            scheduled_event_id,
            started_event_id: wt.started_event_id,
            cause,
            failure,
            identity,
            ..Default::default()
        },
    ));
    fx.push(Effect::WorkflowTaskDone {
        scheduled_event_id,
        attempt,
    });
    b.state().workflow_task = None;
    flush(b);
    // Every failed attempt is written in DX1; Temporal's transient attempts
    // (not written until one succeeds) are DX2's.
    schedule_workflow_task(b, fx, attempt + 1);
    Ok(Reply::Done)
}

pub(crate) fn timed_out(
    b: &mut Batch<'_>,
    fx: &mut Vec<Effect>,
    scheduled_event_id: i64,
    attempt: i32,
) -> Result<Reply, Rejection> {
    let Ok(wt) = take_started(b, scheduled_event_id, attempt) else {
        return Ok(Reply::Stale);
    };
    b.push(Attributes::WorkflowTaskTimedOutEventAttributes(
        WorkflowTaskTimedOutEventAttributes {
            scheduled_event_id,
            started_event_id: wt.started_event_id,
            timeout_type: TimeoutType::StartToClose as i32,
        },
    ));
    fx.push(Effect::WorkflowTaskDone {
        scheduled_event_id,
        attempt,
    });
    b.state().workflow_task = None;
    flush(b);
    schedule_workflow_task(b, fx, attempt + 1);
    Ok(Reply::Done)
}
