//! X1.R7's limits in the state machine (DX1F.R2): an oversized command
//! payload, too many pending activities, and a history over its size or
//! event count, each answered the way Temporal v1.32.1's history service
//! answers it.

mod common;

use common::*;
use loams_temporal_history::{
    Attributes, EventType, Limits, MutableState, WorkflowExecutionStatus,
};
use loams_temporal_proto::temporal::api::command::v1::{
    CompleteWorkflowExecutionCommandAttributes, command,
};
use loams_temporal_proto::temporal::api::common::v1::{Payload, Payloads};
use loams_temporal_proto::temporal::api::enums::v1::WorkflowTaskFailedCause;
use loams_temporal_proto::temporal::api::failure::v1::failure;
use loams_temporal_statemachine::{Effect, Input, Rejection, Transition, apply};

fn types(t: &Transition) -> Vec<EventType> {
    t.events
        .iter()
        .map(|e| EventType::try_from(e.event_type).expect("known"))
        .collect()
}

fn signal(s: &MutableState, now: i64) -> Transition {
    apply(
        s,
        Input::Signal {
            name: "go".into(),
            input: None,
            identity: "c".into(),
            header: None,
            request_id: format!("s-{now}"),
        },
        now,
    )
    .expect("signal")
}

/// A run whose first workflow task completed with no commands: open, idle.
fn idle() -> MutableState {
    let s = start().state;
    let s = wft_started(&s, 2_000).state;
    wft_completed(&s, vec![], 2_100).expect("completed").state
}

fn terminated(t: &Transition) -> (String, String) {
    match t.events.last().and_then(|e| e.attributes.clone()) {
        Some(Attributes::WorkflowExecutionTerminatedEventAttributes(a)) => (a.reason, a.identity),
        other => panic!("expected a terminated event, got {other:?}"),
    }
}

#[test]
fn an_oversized_close_payload_fails_the_task_and_terminates_the_run() {
    let s = wft_started(&start().state, 2_000).state;
    let big = Payloads {
        payloads: vec![Payload {
            data: vec![0; Limits::default().blob_size].into(),
            ..Default::default()
        }],
    };
    let t = wft_completed(
        &s,
        vec![cmd(
            command::Attributes::CompleteWorkflowExecutionCommandAttributes(
                CompleteWorkflowExecutionCommandAttributes { result: Some(big) },
            ),
        )],
        2_100,
    )
    .expect("answered");
    assert_eq!(
        types(&t),
        [
            EventType::WorkflowTaskFailed,
            EventType::WorkflowExecutionTerminated
        ]
    );
    let message = "WORKFLOW_TASK_FAILED_CAUSE_BAD_SCHEDULE_ACTIVITY_ATTRIBUTES: \
                   CompleteWorkflowExecutionCommandAttributes.Result exceeds size limit.";
    let Some(Attributes::WorkflowTaskFailedEventAttributes(f)) = &t.events[0].attributes else {
        panic!("failed event");
    };
    assert_eq!(
        f.cause,
        WorkflowTaskFailedCause::BadScheduleActivityAttributes as i32
    );
    let failure = f.failure.as_ref().expect("a failure");
    assert_eq!(failure.message, message);
    assert!(matches!(
        failure.failure_info,
        Some(failure::FailureInfo::ServerFailureInfo(_))
    ));
    assert_eq!(terminated(&t), (message.into(), "history-service".into()));
    assert_eq!(t.state.status, WorkflowExecutionStatus::Terminated as i32);
}

#[test]
fn the_2001st_pending_activity_fails_the_workflow_task() {
    let limit = Limits::default().pending_activities;
    let s = wft_started(&start().state, 2_000).state;
    let at_limit: Vec<_> = (0..limit)
        .map(|i| schedule_activity(&format!("a{i}"), 1))
        .collect();
    let ok = wft_completed(&s, at_limit.clone(), 2_100).expect("2,000 is allowed");
    assert_eq!(ok.state.activities.len(), limit);

    let mut over = at_limit;
    over.push(schedule_activity("one-too-many", 1));
    let t = wft_completed(&s, over, 2_100).expect("answered");
    assert_eq!(
        types(&t),
        [
            EventType::WorkflowTaskFailed,
            EventType::WorkflowTaskScheduled
        ]
    );
    let Some(Attributes::WorkflowTaskFailedEventAttributes(f)) = &t.events[0].attributes else {
        panic!("failed event");
    };
    assert_eq!(
        f.cause,
        WorkflowTaskFailedCause::PendingActivitiesLimitExceeded as i32
    );
    assert_eq!(
        f.failure.as_ref().map(|x| x.message.as_str()),
        Some(
            "WORKFLOW_TASK_FAILED_CAUSE_PENDING_ACTIVITIES_LIMIT_EXCEEDED: the number of pending \
             activities, 2000, has reached the per-workflow limit of 2000"
        )
    );
    assert!(t.state.activities.is_empty(), "nothing was scheduled");
    assert_eq!(t.state.status, WorkflowExecutionStatus::Running as i32);
    assert!(
        t.effects
            .iter()
            .any(|e| matches!(e, Effect::ScheduleWorkflowTask { attempt: 2, .. }))
    );

    // The retry breaks the limit again: refused, nothing written.
    let s2 = wft_started(&t.state, 2_200).state;
    let again: Vec<_> = (0..=limit)
        .map(|i| schedule_activity(&format!("a{i}"), 1))
        .collect();
    let e = wft_completed(&s2, again, 2_300).expect_err("attempt 2");
    assert!(matches!(e, Rejection::InvalidArgument(m) if m.contains("per-workflow limit of 2000")));
}

#[test]
fn a_history_over_50_mib_terminates_the_run_on_its_next_change() {
    let mut s = idle();
    s.history_size_bytes = Limits::default().history_size;
    let at = signal(&s, 3_000);
    assert_eq!(
        types(&at),
        [
            EventType::WorkflowExecutionSignaled,
            EventType::WorkflowTaskScheduled
        ],
        "exactly 50 MiB is allowed"
    );

    s.history_size_bytes = Limits::default().history_size + 1;
    let next_id = s.next_event_id;
    let t = signal(&s, 3_000);
    assert_eq!(types(&t), [EventType::WorkflowExecutionTerminated]);
    assert_eq!(t.events[0].event_id, next_id, "the signal was discarded");
    assert_eq!(
        terminated(&t),
        (
            "Workflow history size exceeds limit.".into(),
            "history-service".into()
        )
    );
    assert_eq!(t.state.status, WorkflowExecutionStatus::Terminated as i32);
    assert!(t.effects.iter().any(|e| matches!(
        e,
        Effect::RunClosed {
            status: WorkflowExecutionStatus::Terminated
        }
    )));
}

#[test]
fn a_change_past_51200_events_terminates_the_run() {
    let limit = Limits::default().history_count;
    let mut s = idle();
    // A signal writes two events (signaled, workflow task scheduled).
    s.next_event_id = limit - 1;
    let at = signal(&s, 3_000);
    assert_eq!(at.state.next_event_id - 1, limit, "51,200 is allowed");
    assert_eq!(at.state.status, WorkflowExecutionStatus::Running as i32);

    s.next_event_id = limit;
    let t = signal(&s, 3_000);
    assert_eq!(types(&t), [EventType::WorkflowExecutionTerminated]);
    assert_eq!(
        terminated(&t),
        (
            "Workflow history count exceeds limit.".into(),
            "history-service".into()
        )
    );
    assert_eq!(t.state.status, WorkflowExecutionStatus::Terminated as i32);
}
