//! A completed workflow task's commands, each turned into its events.

use loams_temporal_history::time::{dur, dur_ms, ts};
use loams_temporal_history::{
    ActivityInfo, Attributes, Batch, Limits, MutableState, TimerInfo, WorkflowExecutionStatus,
};
use loams_temporal_proto::temporal::api::command::v1::{
    Command, ScheduleActivityTaskCommandAttributes, command,
};
use loams_temporal_proto::temporal::api::common::v1::{Payloads, RetryPolicy};
use loams_temporal_proto::temporal::api::enums::v1::{
    RetryState, TaskQueueKind, WorkflowTaskFailedCause,
};
use loams_temporal_proto::temporal::api::history::v1::{
    ActivityTaskCancelRequestedEventAttributes, ActivityTaskCanceledEventAttributes,
    ActivityTaskScheduledEventAttributes, MarkerRecordedEventAttributes,
    TimerCanceledEventAttributes, TimerStartedEventAttributes,
    WorkflowExecutionCanceledEventAttributes, WorkflowExecutionCompletedEventAttributes,
    WorkflowExecutionFailedEventAttributes,
};
use loams_temporal_proto::temporal::api::taskqueue::v1::TaskQueue;
use prost::Message as _;

use crate::write::close;
use crate::{Effect, Rejection, TimeoutKind};

/// Checks the command list before anything is written: known kinds only,
/// and a close command only last.
pub(crate) fn validate(commands: &[Command]) -> Result<(), Rejection> {
    for (i, c) in commands.iter().enumerate() {
        let a = c
            .attributes
            .as_ref()
            .ok_or_else(|| Rejection::InvalidArgument(format!("command {i} has no attributes")))?;
        let close = match a {
            command::Attributes::ScheduleActivityTaskCommandAttributes(_)
            | command::Attributes::StartTimerCommandAttributes(_)
            | command::Attributes::CancelTimerCommandAttributes(_)
            | command::Attributes::RequestCancelActivityTaskCommandAttributes(_)
            | command::Attributes::RecordMarkerCommandAttributes(_)
            | command::Attributes::ProtocolMessageCommandAttributes(_)
            | command::Attributes::StartChildWorkflowExecutionCommandAttributes(_)
            | command::Attributes::SignalExternalWorkflowExecutionCommandAttributes(_)
            | command::Attributes::RequestCancelExternalWorkflowExecutionCommandAttributes(_)
            | command::Attributes::UpsertWorkflowSearchAttributesCommandAttributes(_)
            | command::Attributes::ModifyWorkflowPropertiesCommandAttributes(_) => false,
            command::Attributes::CompleteWorkflowExecutionCommandAttributes(_)
            | command::Attributes::FailWorkflowExecutionCommandAttributes(_)
            | command::Attributes::CancelWorkflowExecutionCommandAttributes(_)
            | command::Attributes::ContinueAsNewWorkflowExecutionCommandAttributes(_) => true,
            other => {
                return Err(Rejection::Unimplemented(format!(
                    "command {} is not supported yet (design 74 §5.8)",
                    kind(other)
                )));
            }
        };
        if close && i + 1 != commands.len() {
            return Err(Rejection::InvalidArgument(
                "a workflow-closing command must be the last command".into(),
            ));
        }
    }
    Ok(())
}

/// A completion whose commands break one of X1.R7's limits, as Temporal's
/// `workflowSizeChecker` (v1.32.1 `service/history/api/
/// respondworkflowtaskcompleted`) finds it.
pub(crate) struct LimitBreach {
    /// The workflow task's failed cause.
    pub cause: WorkflowTaskFailedCause,
    /// The checker's message (Temporal writes `"<CAUSE>: <message>"`).
    pub message: String,
    /// An oversized payload terminates the run; too many pending
    /// activities only fails the workflow task.
    pub terminate: bool,
}

impl LimitBreach {
    fn payload(cause: WorkflowTaskFailedCause, message: &str) -> Self {
        LimitBreach {
            cause,
            message: message.into(),
            terminate: true,
        }
    }
}

/// Checks a completion's commands, in order, against the payload limit and
/// the pending-activity limit before anything is written. Temporal checks
/// each command as it handles it and then discards the whole completion, so
/// checking first gives the same result. Child workflows, external signals
/// and external cancel requests are not built yet (§5.8), so their counts
/// cannot be reached.
pub(crate) fn check_limits(commands: &[Command], state: &MutableState) -> Option<LimitBreach> {
    use WorkflowTaskFailedCause as C;
    let limits = Limits::default();
    let size = |p: Option<&Payloads>| p.map_or(0, |p| p.encoded_len());
    let mut pending = state.activities.len();
    for c in commands {
        match c.attributes.as_ref()? {
            command::Attributes::ScheduleActivityTaskCommandAttributes(a) => {
                if limits.blob_too_large(size(a.input.as_ref())) {
                    return Some(LimitBreach::payload(
                        C::BadScheduleActivityAttributes,
                        "ScheduleActivityTaskCommandAttributes.Input exceeds size limit.",
                    ));
                }
                if pending >= limits.pending_activities {
                    return Some(LimitBreach {
                        cause: C::PendingActivitiesLimitExceeded,
                        message: format!(
                            "the number of pending activities, {pending}, has reached the per-workflow limit of {}",
                            limits.pending_activities
                        ),
                        terminate: false,
                    });
                }
                pending += 1;
            }
            command::Attributes::RequestCancelActivityTaskCommandAttributes(a) => {
                // An activity not yet started is cancelled at once.
                if loams_temporal_history::activity(state, a.scheduled_event_id)
                    .is_some_and(|x| !x.started)
                {
                    pending = pending.saturating_sub(1);
                }
            }
            command::Attributes::RecordMarkerCommandAttributes(a) => {
                let details: usize = a
                    .details
                    .iter()
                    .map(|(k, v)| k.len() + v.encoded_len())
                    .sum();
                if limits.blob_too_large(details) {
                    return Some(LimitBreach::payload(
                        C::BadRecordMarkerAttributes,
                        "RecordMarkerCommandAttributes.Details exceeds size limit.",
                    ));
                }
            }
            command::Attributes::CompleteWorkflowExecutionCommandAttributes(a) => {
                if limits.blob_too_large(size(a.result.as_ref())) {
                    // Temporal's cause here is the schedule-activity one.
                    return Some(LimitBreach::payload(
                        C::BadScheduleActivityAttributes,
                        "CompleteWorkflowExecutionCommandAttributes.Result exceeds size limit.",
                    ));
                }
            }
            command::Attributes::FailWorkflowExecutionCommandAttributes(a)
                if limits.blob_too_large(a.failure.as_ref().map_or(0, |f| f.encoded_len())) =>
            {
                return Some(LimitBreach::payload(
                    C::BadFailWorkflowExecutionAttributes,
                    "FailWorkflowExecutionCommandAttributes.Failure exceeds size limit.",
                ));
            }
            _ => {}
        }
    }
    None
}

fn kind(a: &command::Attributes) -> &'static str {
    match a {
        command::Attributes::ContinueAsNewWorkflowExecutionCommandAttributes(_) => "ContinueAsNew",
        command::Attributes::StartChildWorkflowExecutionCommandAttributes(_) => {
            "StartChildWorkflow"
        }
        command::Attributes::SignalExternalWorkflowExecutionCommandAttributes(_) => {
            "SignalExternalWorkflow"
        }
        command::Attributes::RequestCancelExternalWorkflowExecutionCommandAttributes(_) => {
            "RequestCancelExternalWorkflow"
        }
        command::Attributes::UpsertWorkflowSearchAttributesCommandAttributes(_) => {
            "UpsertWorkflowSearchAttributes"
        }
        command::Attributes::ModifyWorkflowPropertiesCommandAttributes(_) => {
            "ModifyWorkflowProperties"
        }
        command::Attributes::ProtocolMessageCommandAttributes(_) => "ProtocolMessage",
        command::Attributes::ScheduleNexusOperationCommandAttributes(_)
        | command::Attributes::RequestCancelNexusOperationCommandAttributes(_) => "NexusOperation",
        _ => "this",
    }
}

/// The retry policy Temporal records for an activity: the command's, with
/// unset fields defaulted (initial 1 s, coefficient 2, maximum 100 × the
/// initial interval, unlimited attempts).
pub(crate) fn activity_retry_policy(p: Option<RetryPolicy>) -> RetryPolicy {
    let mut p = p.unwrap_or_default();
    let initial = match dur_ms(p.initial_interval.as_ref()) {
        0 => 1_000,
        v => v,
    };
    p.initial_interval = Some(dur(initial));
    if p.backoff_coefficient == 0.0 {
        p.backoff_coefficient = 2.0;
    }
    if dur_ms(p.maximum_interval.as_ref()) == 0 {
        p.maximum_interval = Some(dur(initial * 100));
    }
    p
}

/// The four activity timeouts, normalised the way the corpus shows them:
/// every one present (zero for none), start-to-close falling back to
/// schedule-to-close, and the others capped by schedule-to-close.
fn activity_timeouts(
    a: &ScheduleActivityTaskCommandAttributes,
    run_timeout_ms: i64,
) -> Result<[i64; 4], Rejection> {
    let mut s2c = dur_ms(a.schedule_to_close_timeout.as_ref());
    let mut s2s = dur_ms(a.schedule_to_start_timeout.as_ref());
    let mut stc = dur_ms(a.start_to_close_timeout.as_ref());
    let mut hb = dur_ms(a.heartbeat_timeout.as_ref());
    if s2c < 0 || s2s < 0 || stc < 0 || hb < 0 {
        return Err(Rejection::InvalidArgument(
            "negative activity timeout".into(),
        ));
    }
    if s2c == 0 && stc == 0 {
        return Err(Rejection::InvalidArgument(
            "A valid StartToClose or ScheduleToCloseTimeout is not set on ScheduleActivityTaskCommand."
                .into(),
        ));
    }
    if run_timeout_ms > 0 && (s2c == 0 || s2c > run_timeout_ms) {
        s2c = run_timeout_ms;
    }
    if s2c > 0 {
        if s2s == 0 || s2s > s2c {
            s2s = s2c;
        }
        if stc == 0 || stc > s2c {
            stc = s2c;
        }
    }
    if hb > stc && stc > 0 {
        hb = stc;
    }
    Ok([s2c, s2s, stc, hb])
}

pub(crate) fn apply(
    b: &mut Batch<'_>,
    fx: &mut Vec<Effect>,
    c: Command,
    completed_id: i64,
    identity: &str,
    new_run_id: &str,
) -> Result<(), Rejection> {
    let now = b.now_ms();
    let md = c.user_metadata;
    match c.attributes.expect("validated") {
        command::Attributes::ScheduleActivityTaskCommandAttributes(a) => {
            if a.activity_id.is_empty() {
                return Err(Rejection::InvalidArgument("ActivityId is not set.".into()));
            }
            let s = b.state_ref();
            if s.activities.iter().any(|x| x.activity_id == a.activity_id) {
                return Err(Rejection::InvalidArgument(format!(
                    "Activity with id {} already exists.",
                    a.activity_id
                )));
            }
            let run_ms = dur_ms(s.workflow_run_timeout.as_ref());
            let [s2c, s2s, stc, hb] = activity_timeouts(&a, run_ms)?;
            let task_queue = a
                .task_queue
                .as_ref()
                .map(|t| t.name.clone())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| s.task_queue.clone());
            let retry_policy = activity_retry_policy(a.retry_policy);
            let activity_type = a.activity_type.clone().unwrap_or_default();
            let id = b.push_with_metadata(
                md.clone(),
                Attributes::ActivityTaskScheduledEventAttributes(
                    ActivityTaskScheduledEventAttributes {
                        activity_id: a.activity_id.clone(),
                        activity_type: Some(activity_type.clone()),
                        task_queue: Some(TaskQueue {
                            name: task_queue.clone(),
                            kind: TaskQueueKind::Normal as i32,
                            normal_name: String::new(),
                        }),
                        header: Some(a.header.unwrap_or_default()),
                        input: a.input,
                        schedule_to_close_timeout: Some(dur(s2c)),
                        schedule_to_start_timeout: Some(dur(s2s)),
                        start_to_close_timeout: Some(dur(stc)),
                        heartbeat_timeout: Some(dur(hb)),
                        workflow_task_completed_event_id: completed_id,
                        retry_policy: Some(retry_policy.clone()),
                        #[allow(deprecated)]
                        use_workflow_build_id: a.use_workflow_build_id,
                        priority: a.priority,
                    },
                ),
            );
            b.state().activities.push(ActivityInfo {
                scheduled_event_id: id,
                activity_id: a.activity_id,
                activity_type: activity_type.name,
                task_queue: task_queue.clone(),
                attempt: 1,
                schedule_to_close_timeout: Some(dur(s2c)),
                schedule_to_start_timeout: Some(dur(s2s)),
                start_to_close_timeout: Some(dur(stc)),
                heartbeat_timeout: Some(dur(hb)),
                scheduled_time: Some(ts(now)),
                attempt_scheduled_time: Some(ts(now)),
                retry_policy: Some(retry_policy),
                ..Default::default()
            });
            fx.push(Effect::ScheduleActivity {
                scheduled_event_id: id,
                attempt: 1,
                task_queue,
                not_before_ms: 0,
            });
            if s2s > 0 {
                fx.push(Effect::ArmTimeout {
                    kind: TimeoutKind::ScheduleToStart,
                    event_id: id,
                    attempt: 1,
                    at_ms: now + s2s,
                });
            }
            if s2c > 0 {
                fx.push(Effect::ArmTimeout {
                    kind: TimeoutKind::ScheduleToClose,
                    event_id: id,
                    attempt: 1,
                    at_ms: now + s2c,
                });
            }
        }
        command::Attributes::RequestCancelActivityTaskCommandAttributes(a) => {
            let scheduled = a.scheduled_event_id;
            let act = loams_temporal_history::activity(b.state_ref(), scheduled)
                .cloned()
                .ok_or_else(|| {
                    Rejection::InvalidArgument(format!(
                        "Unable to find activity with schedule event id: {scheduled}."
                    ))
                })?;
            let requested = b.push_with_metadata(
                md.clone(),
                Attributes::ActivityTaskCancelRequestedEventAttributes(
                    ActivityTaskCancelRequestedEventAttributes {
                        scheduled_event_id: scheduled,
                        workflow_task_completed_event_id: completed_id,
                    },
                ),
            );
            if act.started {
                let a = loams_temporal_history::activity_mut(b.state(), scheduled)
                    .expect("found above");
                a.cancel_requested = true;
                a.cancel_requested_event_id = requested;
            } else {
                // Not started: cancelled at once.
                b.push(Attributes::ActivityTaskCanceledEventAttributes(
                    ActivityTaskCanceledEventAttributes {
                        latest_cancel_requested_event_id: requested,
                        scheduled_event_id: scheduled,
                        identity: identity.to_string(),
                        ..Default::default()
                    },
                ));
                loams_temporal_history::take_activity(b.state(), scheduled);
                fx.push(Effect::ActivityDone {
                    scheduled_event_id: scheduled,
                    attempt: act.attempt,
                });
            }
        }
        command::Attributes::StartTimerCommandAttributes(a) => {
            if a.timer_id.is_empty() {
                return Err(Rejection::InvalidArgument("TimerId is not set.".into()));
            }
            if loams_temporal_history::timer(b.state_ref(), &a.timer_id).is_some() {
                return Err(Rejection::InvalidArgument(format!(
                    "Timer with id {} already exists.",
                    a.timer_id
                )));
            }
            let fire = dur_ms(a.start_to_fire_timeout.as_ref());
            if fire <= 0 {
                return Err(Rejection::InvalidArgument(
                    "A valid StartToFireTimeout is not set on StartTimerCommand.".into(),
                ));
            }
            let id = b.push_with_metadata(
                md.clone(),
                Attributes::TimerStartedEventAttributes(TimerStartedEventAttributes {
                    timer_id: a.timer_id.clone(),
                    start_to_fire_timeout: a.start_to_fire_timeout,
                    workflow_task_completed_event_id: completed_id,
                }),
            );
            b.state().timers.push(TimerInfo {
                timer_id: a.timer_id,
                started_event_id: id,
                fire_time: Some(ts(now + fire)),
            });
            fx.push(Effect::StartTimer {
                started_event_id: id,
                fire_at_ms: now + fire,
            });
        }
        command::Attributes::CancelTimerCommandAttributes(a) => {
            let t =
                loams_temporal_history::take_timer(b.state(), &a.timer_id).ok_or_else(|| {
                    Rejection::InvalidArgument(format!("Timer with id {} not found.", a.timer_id))
                })?;
            b.push_with_metadata(
                md.clone(),
                Attributes::TimerCanceledEventAttributes(TimerCanceledEventAttributes {
                    timer_id: a.timer_id,
                    started_event_id: t.started_event_id,
                    workflow_task_completed_event_id: completed_id,
                    identity: identity.to_string(),
                }),
            );
            fx.push(Effect::CancelTimer {
                started_event_id: t.started_event_id,
            });
        }
        command::Attributes::RecordMarkerCommandAttributes(a) => {
            b.push_with_metadata(
                md.clone(),
                Attributes::MarkerRecordedEventAttributes(MarkerRecordedEventAttributes {
                    marker_name: a.marker_name,
                    details: a.details,
                    workflow_task_completed_event_id: completed_id,
                    header: a.header,
                    failure: a.failure,
                }),
            );
        }
        command::Attributes::CompleteWorkflowExecutionCommandAttributes(a) => {
            let id = b.push_with_metadata(
                md.clone(),
                Attributes::WorkflowExecutionCompletedEventAttributes(
                    WorkflowExecutionCompletedEventAttributes {
                        result: a.result,
                        workflow_task_completed_event_id: completed_id,
                        new_execution_run_id: String::new(),
                    },
                ),
            );
            close(b, fx, id, WorkflowExecutionStatus::Completed);
        }
        command::Attributes::FailWorkflowExecutionCommandAttributes(a) => {
            let id = b.push_with_metadata(
                md.clone(),
                Attributes::WorkflowExecutionFailedEventAttributes(
                    WorkflowExecutionFailedEventAttributes {
                        failure: a.failure,
                        retry_state: RetryState::RetryPolicyNotSet as i32,
                        workflow_task_completed_event_id: completed_id,
                        new_execution_run_id: String::new(),
                    },
                ),
            );
            close(b, fx, id, WorkflowExecutionStatus::Failed);
        }
        command::Attributes::CancelWorkflowExecutionCommandAttributes(a) => {
            let id = b.push_with_metadata(
                md.clone(),
                Attributes::WorkflowExecutionCanceledEventAttributes(
                    WorkflowExecutionCanceledEventAttributes {
                        workflow_task_completed_event_id: completed_id,
                        details: a.details,
                    },
                ),
            );
            close(b, fx, id, WorkflowExecutionStatus::Canceled);
        }
        command::Attributes::ContinueAsNewWorkflowExecutionCommandAttributes(a) => {
            crate::children::continue_as_new(b, fx, a, md, completed_id, new_run_id)?;
        }
        command::Attributes::StartChildWorkflowExecutionCommandAttributes(a) => {
            crate::children::start_child(b, fx, a, md, completed_id)?;
        }
        command::Attributes::SignalExternalWorkflowExecutionCommandAttributes(a) => {
            crate::children::signal_external(b, fx, a, md, completed_id)?;
        }
        command::Attributes::RequestCancelExternalWorkflowExecutionCommandAttributes(a) => {
            crate::children::cancel_external(b, fx, a, md, completed_id)?;
        }
        command::Attributes::UpsertWorkflowSearchAttributesCommandAttributes(a) => {
            crate::children::upsert_search_attributes(b, a, md, completed_id);
        }
        command::Attributes::ModifyWorkflowPropertiesCommandAttributes(a) => {
            crate::children::modify_properties(b, a, md, completed_id);
        }
        _ => unreachable!("validate refuses other commands"),
    }
    Ok(())
}
