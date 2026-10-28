//! `ResetWorkflowExecution` and reset points (TMP-api, D2083).
//!
//! A reset makes a new run whose history is the base run's up to and
//! including the started event of the chosen workflow task, then (as the
//! corpus's `reset_to_first_workflow_task` shows Temporal writing it):
//! - `WorkflowTaskFailed` for that task, cause `RESET_WORKFLOW`, identity
//!   `history-service`, a `ResetWorkflowFailureInfo` failure whose message
//!   is the reason, and the base and new run IDs;
//! - the base run's signals that came after the reset point, again (unless
//!   the request excludes signals);
//! - a new `WorkflowTaskScheduled` on the run's normal task queue.
//!
//! Activities and timers pending at the reset point are dispatched again.
//! Updates accepted after the reset point are not re-applied (Temporal
//! re-applies them as `WorkflowExecutionUpdateAdmitted` events; the gateway
//! does not deliver updates from history yet).

use loams_temporal_history::time::{dur_ms, ms, ts};
use loams_temporal_history::{Attributes, Batch, HistoryEvent, MutableState, fold};
use loams_temporal_proto::temporal::api::enums::v1::WorkflowTaskFailedCause;
use loams_temporal_proto::temporal::api::failure::v1::{
    Failure, ResetWorkflowFailureInfo, failure,
};
use loams_temporal_proto::temporal::api::history::v1::WorkflowTaskFailedEventAttributes;
use loams_temporal_proto::temporal::api::workflow::v1::ResetPointInfo;

use crate::external::HISTORY_SERVICE;
use crate::write::schedule_workflow_task;
use crate::{Effect, Rejection, TimeoutKind};

/// What to reset.
#[derive(Debug, Clone, PartialEq)]
pub struct ResetRequest {
    /// The gateway's namespace ID.
    pub namespace_id: String,
    /// The base run's ID.
    pub base_run_id: String,
    /// The new run's ID.
    pub new_run_id: String,
    /// A `WorkflowTaskCompleted`, `…Failed`, `…TimedOut` or `…Started`
    /// event of the base run: the workflow task to reset to.
    pub workflow_task_finish_event_id: i64,
    /// The reason (the failure's message).
    pub reason: String,
    /// Write the base run's later signals into the new run again.
    pub reapply_signals: bool,
}

/// The new run: its whole history, its state, and what to dispatch.
#[derive(Debug, Clone, PartialEq)]
pub struct ResetRun {
    /// The new run's state.
    pub state: MutableState,
    /// The new run's history from event 1.
    pub events: Vec<HistoryEvent>,
    /// The effects for the outbox (the new workflow task, pending
    /// activities and timers).
    pub effects: Vec<Effect>,
}

fn invalid(why: impl Into<String>) -> Rejection {
    Rejection::InvalidArgument(why.into())
}

/// The scheduled and started event IDs of the workflow task `event_id`
/// finishes (or starts).
fn reset_task(base: &[HistoryEvent], event_id: i64) -> Result<(i64, i64), Rejection> {
    let e = base
        .iter()
        .find(|e| e.event_id == event_id)
        .ok_or_else(|| invalid(format!("event {event_id} is not in the workflow's history")))?;
    let (scheduled, started) = match e.attributes.as_ref() {
        Some(Attributes::WorkflowTaskCompletedEventAttributes(x)) => {
            (x.scheduled_event_id, x.started_event_id)
        }
        Some(Attributes::WorkflowTaskFailedEventAttributes(x)) => {
            (x.scheduled_event_id, x.started_event_id)
        }
        Some(Attributes::WorkflowTaskTimedOutEventAttributes(x)) => {
            (x.scheduled_event_id, x.started_event_id)
        }
        Some(Attributes::WorkflowTaskStartedEventAttributes(x)) => (x.scheduled_event_id, event_id),
        _ => {
            return Err(invalid(format!(
                "event {event_id} does not finish a workflow task; reset needs the ID of a WorkflowTaskCompleted, WorkflowTaskFailed, WorkflowTaskTimedOut or WorkflowTaskStarted event"
            )));
        }
    };
    if started <= 0 {
        return Err(invalid(format!(
            "the workflow task of event {event_id} never started"
        )));
    }
    Ok((scheduled, started))
}

/// Builds the new run of a reset at `now_ms`.
pub fn reset(base: &[HistoryEvent], r: &ResetRequest, now_ms: i64) -> Result<ResetRun, Rejection> {
    let (scheduled, started) = reset_task(base, r.workflow_task_finish_event_id)?;
    let prefix: Vec<HistoryEvent> = base
        .iter()
        .take_while(|e| e.event_id <= started)
        .cloned()
        .collect();
    let mut state = fold(&r.namespace_id, &r.new_run_id, &prefix).map_err(|e| {
        invalid(format!(
            "the history before the reset point cannot be replayed: {e}"
        ))
    })?;
    if !state
        .workflow_task
        .as_ref()
        .is_some_and(|w| w.scheduled_event_id == scheduled && w.started_event_id == started)
    {
        return Err(invalid(
            "the reset point's workflow task is not outstanding there",
        ));
    }

    let mut fx = Vec::new();
    let events = {
        let mut b = Batch::new(&mut state, now_ms);
        b.push(Attributes::WorkflowTaskFailedEventAttributes(
            WorkflowTaskFailedEventAttributes {
                scheduled_event_id: scheduled,
                started_event_id: started,
                cause: WorkflowTaskFailedCause::ResetWorkflow as i32,
                failure: Some(Failure {
                    message: r.reason.clone(),
                    failure_info: Some(failure::FailureInfo::ResetWorkflowFailureInfo(
                        ResetWorkflowFailureInfo::default(),
                    )),
                    ..Default::default()
                }),
                identity: HISTORY_SERVICE.to_string(),
                base_run_id: r.base_run_id.clone(),
                new_run_id: r.new_run_id.clone(),
                ..Default::default()
            },
        ));
        b.state().workflow_task = None;
        if r.reapply_signals {
            for e in base.iter().filter(|e| e.event_id > started) {
                if let Some(Attributes::WorkflowExecutionSignaledEventAttributes(x)) =
                    e.attributes.as_ref()
                {
                    b.state().signal_count += 1;
                    b.push(Attributes::WorkflowExecutionSignaledEventAttributes(
                        x.clone(),
                    ));
                }
            }
        }
        redispatch(&mut b, &mut fx);
        schedule_workflow_task(&mut b, &mut fx, 1);
        b.finish()
    };
    let mut all = prefix;
    all.extend(events);
    Ok(ResetRun {
        state,
        events: all,
        effects: fx,
    })
}

/// Dispatches again what the new run has pending: activities from their
/// first attempt (their clocks start now) and timers at their fire time.
fn redispatch(b: &mut Batch<'_>, fx: &mut Vec<Effect>) {
    let now = b.now_ms();
    for a in &mut b.state().activities {
        a.attempt = 1;
        a.started = false;
        a.scheduled_time = Some(ts(now));
        a.attempt_scheduled_time = Some(ts(now));
        fx.push(Effect::ScheduleActivity {
            scheduled_event_id: a.scheduled_event_id,
            attempt: 1,
            task_queue: a.task_queue.clone(),
            not_before_ms: 0,
        });
        let s2s = dur_ms(a.schedule_to_start_timeout.as_ref());
        if s2s > 0 {
            fx.push(Effect::ArmTimeout {
                kind: TimeoutKind::ScheduleToStart,
                event_id: a.scheduled_event_id,
                attempt: 1,
                at_ms: now + s2s,
            });
        }
        let s2c = dur_ms(a.schedule_to_close_timeout.as_ref());
        if s2c > 0 {
            fx.push(Effect::ArmTimeout {
                kind: TimeoutKind::ScheduleToClose,
                event_id: a.scheduled_event_id,
                attempt: 1,
                at_ms: now + s2c,
            });
        }
    }
    for t in &b.state_ref().timers {
        fx.push(Effect::StartTimer {
            started_event_id: t.started_event_id,
            fire_at_ms: t.fire_time.as_ref().map_or(now, ms).max(now),
        });
    }
}

/// The run's automatic reset points: the first `WorkflowTaskCompleted` of
/// each worker build (by build ID, or the binary checksum older workers
/// send), as `DescribeWorkflowExecution` reports them.
pub fn reset_points(run_id: &str, events: &[HistoryEvent]) -> Vec<ResetPointInfo> {
    let mut out: Vec<ResetPointInfo> = Vec::new();
    for e in events {
        let Some(Attributes::WorkflowTaskCompletedEventAttributes(x)) = e.attributes.as_ref()
        else {
            continue;
        };
        let build_id = x
            .worker_version
            .as_ref()
            .map(|v| v.build_id.clone())
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| x.binary_checksum.clone());
        if out.iter().any(|p| p.build_id == build_id) {
            continue;
        }
        out.push(ResetPointInfo {
            build_id: build_id.clone(),
            binary_checksum: x.binary_checksum.clone(),
            run_id: run_id.to_string(),
            first_workflow_task_completed_id: e.event_id,
            create_time: e.event_time,
            expire_time: None,
            resettable: true,
        });
    }
    out
}
