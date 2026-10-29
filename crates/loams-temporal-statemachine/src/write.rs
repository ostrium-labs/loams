//! Writing events: directly, or buffered behind a started workflow task.

use std::collections::BTreeMap;

use loams_temporal_history::time::{ms, ts};
use loams_temporal_history::{
    Attributes, Batch, HistoryEvent, WorkflowExecutionStatus, WorkflowTaskInfo, event_type,
};
use loams_temporal_proto::temporal::api::enums::v1::TaskQueueKind;
use loams_temporal_proto::temporal::api::history::v1::WorkflowTaskScheduledEventAttributes;
use loams_temporal_proto::temporal::api::taskqueue::v1::TaskQueue;

use crate::Effect;

/// A not-yet-numbered event: its time and attributes.
pub(crate) type Pending = (i64, Attributes);

/// Whether a workflow task is started (a poll holds it).
pub(crate) fn wft_started(b: &Batch<'_>) -> bool {
    b.state_ref()
        .workflow_task
        .as_ref()
        .is_some_and(|w| w.started_event_id != 0)
}

/// Events from outside the workflow task (signals, activity outcomes,
/// cancel requests, timers): buffered while a workflow task is started,
/// otherwise written now, followed by a workflow task if none is
/// outstanding.
pub(crate) fn arrive(b: &mut Batch<'_>, fx: &mut Vec<Effect>, events: Vec<Pending>) {
    if wft_started(b) {
        for (at, attributes) in events {
            let event = HistoryEvent {
                event_time: Some(ts(at)),
                event_type: event_type(&attributes) as i32,
                attributes: Some(attributes),
                ..Default::default()
            };
            b.state().buffered.push(event);
        }
    } else {
        write(b, events);
        ensure_workflow_task(b, fx);
    }
}

/// Writes events in order, filling in each activity outcome's
/// `started_event_id` from the `ActivityTaskStarted` written just before it.
pub(crate) fn write(b: &mut Batch<'_>, events: Vec<Pending>) {
    let mut started: BTreeMap<i64, i64> = BTreeMap::new();
    let mut child_started: BTreeMap<i64, i64> = BTreeMap::new();
    for (at, mut attributes) in events {
        link_started(&mut attributes, &started);
        link_child_started(&mut attributes, &child_started);
        let scheduled = match &attributes {
            Attributes::ActivityTaskStartedEventAttributes(x) => Some(x.scheduled_event_id),
            _ => None,
        };
        let initiated = match &attributes {
            Attributes::ChildWorkflowExecutionStartedEventAttributes(x) => {
                Some(x.initiated_event_id)
            }
            _ => None,
        };
        let id = b.push_at(at, attributes);
        if let Some(s) = scheduled {
            started.insert(s, id);
        }
        if let Some(i) = initiated {
            child_started.insert(i, id);
            if let Some(c) = b
                .state()
                .children
                .iter_mut()
                .find(|c| c.initiated_event_id == i)
            {
                c.started_event_id = id;
            }
        }
    }
}

/// Writes the buffered events (after a workflow task's completion or
/// failure); returns whether there were any.
pub(crate) fn flush(b: &mut Batch<'_>) -> bool {
    let buffered = std::mem::take(&mut b.state().buffered);
    let any = !buffered.is_empty();
    let pending = buffered
        .into_iter()
        .map(|e| {
            let at = e.event_time.as_ref().map_or(b.now_ms(), ms);
            (at, e.attributes.expect("buffered events carry attributes"))
        })
        .collect();
    write(b, pending);
    any
}

fn link_started(attributes: &mut Attributes, started: &BTreeMap<i64, i64>) {
    let (scheduled, slot) = match attributes {
        Attributes::ActivityTaskCompletedEventAttributes(x) => {
            (x.scheduled_event_id, &mut x.started_event_id)
        }
        Attributes::ActivityTaskFailedEventAttributes(x) => {
            (x.scheduled_event_id, &mut x.started_event_id)
        }
        Attributes::ActivityTaskTimedOutEventAttributes(x) => {
            (x.scheduled_event_id, &mut x.started_event_id)
        }
        Attributes::ActivityTaskCanceledEventAttributes(x) => {
            (x.scheduled_event_id, &mut x.started_event_id)
        }
        _ => return,
    };
    if *slot == 0
        && let Some(id) = started.get(&scheduled)
    {
        *slot = *id;
    }
}

/// A child's closing event written in the same batch as its started event
/// gets that event's ID.
fn link_child_started(attributes: &mut Attributes, started: &BTreeMap<i64, i64>) {
    let (initiated, slot) = match attributes {
        Attributes::ChildWorkflowExecutionCompletedEventAttributes(x) => {
            (x.initiated_event_id, &mut x.started_event_id)
        }
        Attributes::ChildWorkflowExecutionFailedEventAttributes(x) => {
            (x.initiated_event_id, &mut x.started_event_id)
        }
        Attributes::ChildWorkflowExecutionCanceledEventAttributes(x) => {
            (x.initiated_event_id, &mut x.started_event_id)
        }
        Attributes::ChildWorkflowExecutionTimedOutEventAttributes(x) => {
            (x.initiated_event_id, &mut x.started_event_id)
        }
        Attributes::ChildWorkflowExecutionTerminatedEventAttributes(x) => {
            (x.initiated_event_id, &mut x.started_event_id)
        }
        _ => return,
    };
    if *slot <= 0
        && let Some(id) = started.get(&initiated)
    {
        *slot = *id;
    }
}

/// Schedules a workflow task (attempt 1) if the run is open and has none
/// (and is not waiting out its first task's backoff).
pub(crate) fn ensure_workflow_task(b: &mut Batch<'_>, fx: &mut Vec<Effect>) {
    if b.state_ref().workflow_task.is_none()
        && !b.state_ref().first_task_backoff
        && loams_temporal_history::is_open(b.state_ref())
    {
        schedule_workflow_task(b, fx, 1);
    }
}

/// Writes `WorkflowTaskScheduled` for `attempt` on the run's task queue.
pub(crate) fn schedule_workflow_task(b: &mut Batch<'_>, fx: &mut Vec<Effect>, attempt: i32) {
    let s = b.state_ref();
    let task_queue = s.task_queue.clone();
    let timeout = s.workflow_task_timeout;
    b.state().first_task_backoff = false;
    let id = b.push(Attributes::WorkflowTaskScheduledEventAttributes(
        WorkflowTaskScheduledEventAttributes {
            task_queue: Some(TaskQueue {
                name: task_queue.clone(),
                kind: TaskQueueKind::Normal as i32,
                normal_name: String::new(),
            }),
            start_to_close_timeout: timeout,
            attempt,
        },
    ));
    let now = b.now_ms();
    b.state().workflow_task = Some(WorkflowTaskInfo {
        scheduled_event_id: id,
        attempt,
        scheduled_time: Some(ts(now)),
        start_to_close_timeout: timeout,
        task_queue: task_queue.clone(),
        ..Default::default()
    });
    fx.push(Effect::ScheduleWorkflowTask {
        scheduled_event_id: id,
        attempt,
        task_queue,
    });
}

/// Marks the run closed by the event just written as `event_id`.
pub(crate) fn close(
    b: &mut Batch<'_>,
    fx: &mut Vec<Effect>,
    event_id: i64,
    status: WorkflowExecutionStatus,
) {
    let children = crate::children::close_policy_children(b);
    if !children.is_empty() {
        fx.push(Effect::ParentClosePolicy { children });
    }
    let now = b.now_ms();
    let s = b.state();
    s.children.clear();
    s.external_signals.clear();
    s.external_cancels.clear();
    s.first_task_backoff = false;
    s.status = status as i32;
    s.completion_event_id = event_id;
    s.close_time = Some(ts(now));
    s.workflow_task = None;
    s.timers.clear();
    s.activities.clear();
    loams_temporal_history::abort_updates(s);
    fx.push(Effect::RunClosed { status });
}
