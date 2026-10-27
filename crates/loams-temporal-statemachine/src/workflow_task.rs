//! The workflow task: started by a poll, then completed, failed or timed out.

use loams_temporal_history::time::{dur_ms, ms, ts};
use loams_temporal_history::{Attributes, Batch, WorkflowExecutionStatus, limit_messages};
use loams_temporal_proto::temporal::api::command::v1::{Command, command};
use loams_temporal_proto::temporal::api::common::v1::{MeteringMetadata, WorkerVersionStamp};
use loams_temporal_proto::temporal::api::enums::v1::{TimeoutType, WorkflowTaskFailedCause};
use loams_temporal_proto::temporal::api::failure::v1::{Failure, ServerFailureInfo, failure};
use loams_temporal_proto::temporal::api::history::v1::{
    WorkflowExecutionTerminatedEventAttributes, WorkflowTaskCompletedEventAttributes,
    WorkflowTaskFailedEventAttributes, WorkflowTaskStartedEventAttributes,
    WorkflowTaskTimedOutEventAttributes,
};
use loams_temporal_proto::temporal::api::sdk::v1::WorkflowTaskCompletedMetadata;

use loams_temporal_proto::temporal::api::protocol::v1::Message;

use crate::write::{close, ensure_workflow_task, flush, schedule_workflow_task};
use crate::{Effect, Rejection, Reply, TimeoutKind, commands, update};

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
    // Admitted updates go out with this task.
    update::mark_sent(b, scheduled_event_id);
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
    pub messages: Vec<Message>,
    pub new_run_id: String,
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
    if let Some(breach) = commands::check_limits(&c.commands, b.state_ref()) {
        return over_limit(b, fx, &wt, breach, c.identity);
    }
    fx.push(Effect::WorkflowTaskDone {
        scheduled_event_id: wt.scheduled_event_id,
        attempt: wt.attempt,
    });

    // A close command while new events (or admitted updates) wait would
    // hide them from the workflow: fail the task so the worker sees them
    // first.
    if c.commands.iter().any(is_close)
        && (!b.state_ref().buffered.is_empty() || update::has_admitted(b))
    {
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
        update::resend_after_failure(b, wt.scheduled_event_id);
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
    // Protocol messages go where a `ProtocolMessage` command puts them; the
    // rest after every command (before a closing one, so nothing follows
    // the close).
    let mut messages: Vec<Option<Message>> = c.messages.into_iter().map(Some).collect();
    for command in c.commands {
        if let Some(command::Attributes::ProtocolMessageCommandAttributes(p)) = &command.attributes
        {
            let m = messages
                .iter_mut()
                .find(|m| m.as_ref().is_some_and(|m| m.id == p.message_id))
                .and_then(Option::take)
                .ok_or_else(|| {
                    Rejection::InvalidArgument(format!(
                        "ProtocolMessageCommand names message {}, which the request does not carry",
                        p.message_id
                    ))
                })?;
            update::handle_message(b, m)?;
            continue;
        }
        if is_close(&command) {
            for m in messages.iter_mut().filter_map(Option::take) {
                update::handle_message(b, m)?;
            }
        }
        commands::apply(b, fx, command, completed_id, &c.identity, &c.new_run_id)?;
    }
    for m in messages.into_iter().flatten() {
        update::handle_message(b, m)?;
    }
    update::reject_unprocessed(b, wt.scheduled_event_id);
    let had_buffered = flush(b);
    if had_buffered || c.force_create_new_workflow_task || update::has_admitted(b) {
        ensure_workflow_task(b, fx);
    }
    Ok(Reply::Done)
}

/// A completion over one of X1.R7's limits, answered as Temporal's
/// `RespondWorkflowTaskCompleted` does: the workflow task fails with the
/// limit's cause and a server failure `"<CAUSE>: <message>"`; an oversized
/// payload also terminates the run (reason the same message, identity
/// `history-service`). A later attempt that breaks a limit again is refused
/// with `InvalidArgument` and nothing is written, so the task times out and
/// is retried instead of failing in a loop.
fn over_limit(
    b: &mut Batch<'_>,
    fx: &mut Vec<Effect>,
    wt: &loams_temporal_history::WorkflowTaskInfo,
    breach: commands::LimitBreach,
    identity: String,
) -> Result<Reply, Rejection> {
    let message = format!("{}: {}", breach.cause.as_str_name(), breach.message);
    if wt.attempt > 1 {
        return Err(Rejection::InvalidArgument(message));
    }
    fx.push(Effect::WorkflowTaskDone {
        scheduled_event_id: wt.scheduled_event_id,
        attempt: wt.attempt,
    });
    b.push(Attributes::WorkflowTaskFailedEventAttributes(
        WorkflowTaskFailedEventAttributes {
            scheduled_event_id: wt.scheduled_event_id,
            started_event_id: wt.started_event_id,
            cause: breach.cause as i32,
            failure: Some(server_failure(message.clone(), false)),
            identity,
            ..Default::default()
        },
    ));
    b.state().workflow_task = None;
    update::resend_after_failure(b, wt.scheduled_event_id);
    flush(b);
    if breach.terminate {
        let id = b.push(Attributes::WorkflowExecutionTerminatedEventAttributes(
            WorkflowExecutionTerminatedEventAttributes {
                reason: message,
                details: None,
                identity: limit_messages::HISTORY_SERVICE.into(),
            },
        ));
        close(b, fx, id, WorkflowExecutionStatus::Terminated);
    } else {
        schedule_workflow_task(b, fx, wt.attempt + 1);
    }
    Ok(Reply::Done)
}

/// Temporal's `failure.NewServerFailure`.
pub fn server_failure(message: String, non_retryable: bool) -> Failure {
    Failure {
        message,
        failure_info: Some(failure::FailureInfo::ServerFailureInfo(ServerFailureInfo {
            non_retryable,
        })),
        ..Default::default()
    }
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
    update::resend_after_failure(b, scheduled_event_id);
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
    update::resend_after_failure(b, scheduled_event_id);
    flush(b);
    schedule_workflow_task(b, fx, attempt + 1);
    Ok(Reply::Done)
}
