//! Rebuilding a run's mutable state from its history (design 74 §5.5 rule 7).

use prost::Message as _;

use crate::time::{dur_ms, ms, ts};
use crate::{
    ActivityInfo, Attributes, EventType, HistoryEvent, MutableState, STATE_VERSION, TimerInfo,
    WorkflowExecutionStatus, WorkflowTaskInfo,
};

/// Why a history cannot be folded.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FoldError {
    /// The history is empty or does not start with `WorkflowExecutionStarted`.
    #[error("the history does not start with WorkflowExecutionStarted")]
    NotStarted,
    /// An event ID is not the previous one plus one (§13.2 invariant 1).
    #[error("event {found} follows event {previous}: IDs must be contiguous")]
    NonContiguous {
        /// The previous event's ID.
        previous: i64,
        /// The ID found.
        found: i64,
    },
    /// An event without attributes, or whose type does not match them.
    #[error("event {0} is malformed")]
    Malformed(i64),
    /// An event refers to an activity, timer or task the history never
    /// scheduled.
    #[error("event {event_id} refers to unknown {what} {id}")]
    Unknown {
        /// The referring event.
        event_id: i64,
        /// What it refers to.
        what: &'static str,
        /// The reference.
        id: String,
    },
    /// An event type the gateway does not build yet (§74 §5.8's phases).
    #[error("event {event_id}: {event_type:?} is not supported yet")]
    Unsupported {
        /// The event.
        event_id: i64,
        /// Its type.
        event_type: EventType,
    },
    /// An event after the run closed.
    #[error("event {0} follows the run's close")]
    AfterClose(i64),
}

/// The mutable state `events` (a run's whole history, in order) imply.
///
/// An activity that has started but not finished is not visible in history
/// (its started event is written with its outcome), so the folded state
/// shows it as scheduled; Resonate's lease decides who holds it.
pub fn fold(
    namespace_id: &str,
    run_id: &str,
    events: &[HistoryEvent],
) -> Result<MutableState, FoldError> {
    let first = events.first().ok_or(FoldError::NotStarted)?;
    let Some(Attributes::WorkflowExecutionStartedEventAttributes(started)) = &first.attributes
    else {
        return Err(FoldError::NotStarted);
    };
    if first.event_id != 1 {
        return Err(FoldError::NonContiguous {
            previous: 0,
            found: first.event_id,
        });
    }
    let mut s = MutableState {
        version: STATE_VERSION,
        namespace_id: namespace_id.to_string(),
        workflow_id: started.workflow_id.clone(),
        run_id: run_id.to_string(),
        workflow_type: started
            .workflow_type
            .as_ref()
            .map(|t| t.name.clone())
            .unwrap_or_default(),
        task_queue: started
            .task_queue
            .as_ref()
            .map(|t| t.name.clone())
            .unwrap_or_default(),
        workflow_task_timeout: started.workflow_task_timeout,
        workflow_run_timeout: started.workflow_run_timeout,
        workflow_execution_timeout: started.workflow_execution_timeout,
        start_time: first.event_time,
        status: WorkflowExecutionStatus::Running as i32,
        first_execution_run_id: started.first_execution_run_id.clone(),
        ..Default::default()
    };

    let mut previous = 0;
    for e in events {
        if e.event_id != previous + 1 {
            return Err(FoldError::NonContiguous {
                previous,
                found: e.event_id,
            });
        }
        previous = e.event_id;
        if s.completion_event_id != 0 {
            return Err(FoldError::AfterClose(e.event_id));
        }
        let attributes = e
            .attributes
            .as_ref()
            .ok_or(FoldError::Malformed(e.event_id))?;
        if crate::event_type(attributes) as i32 != e.event_type {
            return Err(FoldError::Malformed(e.event_id));
        }
        apply(&mut s, e, attributes)?;
        s.last_task_id = s.last_task_id.max(e.task_id);
        s.history_size_bytes += e.encoded_len() as i64;
    }
    s.next_event_id = previous + 1;
    Ok(s)
}

fn unknown(event_id: i64, what: &'static str, id: impl ToString) -> FoldError {
    FoldError::Unknown {
        event_id,
        what,
        id: id.to_string(),
    }
}

fn close(s: &mut MutableState, e: &HistoryEvent, status: WorkflowExecutionStatus) {
    s.status = status as i32;
    s.completion_event_id = e.event_id;
    s.close_time = e.event_time;
    s.workflow_task = None;
}

fn apply(s: &mut MutableState, e: &HistoryEvent, a: &Attributes) -> Result<(), FoldError> {
    let id = e.event_id;
    match a {
        Attributes::WorkflowExecutionStartedEventAttributes(_) => {
            if id != 1 {
                return Err(FoldError::Malformed(id));
            }
        }
        Attributes::WorkflowTaskScheduledEventAttributes(x) => {
            s.workflow_task = Some(WorkflowTaskInfo {
                scheduled_event_id: id,
                attempt: x.attempt,
                scheduled_time: e.event_time,
                start_to_close_timeout: x.start_to_close_timeout,
                task_queue: x
                    .task_queue
                    .as_ref()
                    .map(|t| t.name.clone())
                    .unwrap_or_default(),
                ..Default::default()
            });
        }
        Attributes::WorkflowTaskStartedEventAttributes(x) => {
            let wt = s
                .workflow_task
                .as_mut()
                .filter(|w| w.scheduled_event_id == x.scheduled_event_id)
                .ok_or_else(|| unknown(id, "workflow task", x.scheduled_event_id))?;
            wt.started_event_id = id;
            wt.identity = x.identity.clone();
            wt.request_id = x.request_id.clone();
            wt.started_time = e.event_time;
        }
        Attributes::WorkflowTaskCompletedEventAttributes(x) => {
            let wt = s
                .workflow_task
                .take()
                .filter(|w| w.scheduled_event_id == x.scheduled_event_id)
                .ok_or_else(|| unknown(id, "workflow task", x.scheduled_event_id))?;
            s.last_completed_wft_started_event_id = wt.started_event_id;
        }
        Attributes::WorkflowTaskFailedEventAttributes(x) => {
            s.workflow_task
                .take()
                .filter(|w| w.scheduled_event_id == x.scheduled_event_id)
                .ok_or_else(|| unknown(id, "workflow task", x.scheduled_event_id))?;
        }
        Attributes::WorkflowTaskTimedOutEventAttributes(x) => {
            s.workflow_task
                .take()
                .filter(|w| w.scheduled_event_id == x.scheduled_event_id)
                .ok_or_else(|| unknown(id, "workflow task", x.scheduled_event_id))?;
        }
        Attributes::ActivityTaskScheduledEventAttributes(x) => {
            s.activities.push(ActivityInfo {
                scheduled_event_id: id,
                activity_id: x.activity_id.clone(),
                activity_type: x
                    .activity_type
                    .as_ref()
                    .map(|t| t.name.clone())
                    .unwrap_or_default(),
                task_queue: x
                    .task_queue
                    .as_ref()
                    .map(|t| t.name.clone())
                    .unwrap_or_default(),
                attempt: 1,
                schedule_to_close_timeout: x.schedule_to_close_timeout,
                schedule_to_start_timeout: x.schedule_to_start_timeout,
                start_to_close_timeout: x.start_to_close_timeout,
                heartbeat_timeout: x.heartbeat_timeout,
                scheduled_time: e.event_time,
                attempt_scheduled_time: e.event_time,
                retry_policy: x.retry_policy.clone(),
                ..Default::default()
            });
        }
        Attributes::ActivityTaskStartedEventAttributes(x) => {
            let act = crate::activity_mut(s, x.scheduled_event_id)
                .ok_or_else(|| unknown(id, "activity", x.scheduled_event_id))?;
            act.started_event_id = id;
            act.started = true;
            act.attempt = x.attempt;
            act.started_identity = x.identity.clone();
            act.started_request_id = x.request_id.clone();
            act.started_time = e.event_time;
        }
        Attributes::ActivityTaskCompletedEventAttributes(x) => {
            crate::take_activity(s, x.scheduled_event_id)
                .ok_or_else(|| unknown(id, "activity", x.scheduled_event_id))?;
        }
        Attributes::ActivityTaskFailedEventAttributes(x) => {
            crate::take_activity(s, x.scheduled_event_id)
                .ok_or_else(|| unknown(id, "activity", x.scheduled_event_id))?;
        }
        Attributes::ActivityTaskTimedOutEventAttributes(x) => {
            crate::take_activity(s, x.scheduled_event_id)
                .ok_or_else(|| unknown(id, "activity", x.scheduled_event_id))?;
        }
        Attributes::ActivityTaskCanceledEventAttributes(x) => {
            crate::take_activity(s, x.scheduled_event_id)
                .ok_or_else(|| unknown(id, "activity", x.scheduled_event_id))?;
        }
        Attributes::ActivityTaskCancelRequestedEventAttributes(x) => {
            let act = crate::activity_mut(s, x.scheduled_event_id)
                .ok_or_else(|| unknown(id, "activity", x.scheduled_event_id))?;
            act.cancel_requested = true;
            act.cancel_requested_event_id = id;
        }
        Attributes::TimerStartedEventAttributes(x) => {
            let fire =
                e.event_time.as_ref().map_or(0, ms) + dur_ms(x.start_to_fire_timeout.as_ref());
            s.timers.push(TimerInfo {
                timer_id: x.timer_id.clone(),
                started_event_id: id,
                fire_time: Some(ts(fire)),
            });
        }
        Attributes::TimerFiredEventAttributes(x) => {
            crate::take_timer(s, &x.timer_id).ok_or_else(|| unknown(id, "timer", &x.timer_id))?;
        }
        Attributes::TimerCanceledEventAttributes(x) => {
            crate::take_timer(s, &x.timer_id).ok_or_else(|| unknown(id, "timer", &x.timer_id))?;
        }
        Attributes::WorkflowExecutionSignaledEventAttributes(_) => s.signal_count += 1,
        Attributes::WorkflowExecutionCancelRequestedEventAttributes(_) => s.cancel_requested = true,
        Attributes::MarkerRecordedEventAttributes(_)
        | Attributes::UpsertWorkflowSearchAttributesEventAttributes(_)
        | Attributes::WorkflowPropertiesModifiedEventAttributes(_) => {}
        Attributes::WorkflowExecutionCompletedEventAttributes(_) => {
            close(s, e, WorkflowExecutionStatus::Completed);
        }
        Attributes::WorkflowExecutionFailedEventAttributes(_) => {
            close(s, e, WorkflowExecutionStatus::Failed);
        }
        Attributes::WorkflowExecutionCanceledEventAttributes(_) => {
            close(s, e, WorkflowExecutionStatus::Canceled);
        }
        Attributes::WorkflowExecutionTerminatedEventAttributes(_) => {
            close(s, e, WorkflowExecutionStatus::Terminated);
        }
        Attributes::WorkflowExecutionTimedOutEventAttributes(_) => {
            close(s, e, WorkflowExecutionStatus::TimedOut);
        }
        Attributes::WorkflowExecutionContinuedAsNewEventAttributes(_) => {
            close(s, e, WorkflowExecutionStatus::ContinuedAsNew);
        }
        other => {
            return Err(FoldError::Unsupported {
                event_id: id,
                event_type: crate::event_type(other),
            });
        }
    }
    Ok(())
}
