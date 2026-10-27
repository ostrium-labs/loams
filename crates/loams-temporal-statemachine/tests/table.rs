//! DX1 Task 4's table tests: one case per behaviour the corpus or the SDKs
//! rely on.

mod common;

use common::*;
use loams_temporal_history::{Attributes, EventType, WorkflowExecutionStatus};
use loams_temporal_proto::temporal::api::command::v1::{
    ScheduleNexusOperationCommandAttributes, command,
};
use loams_temporal_proto::temporal::api::enums::v1::WorkflowTaskFailedCause;
use loams_temporal_statemachine::{COMPLETED, Effect, Input, Rejection, Reply, TimeoutKind, apply};

fn types(t: &loams_temporal_statemachine::Transition) -> Vec<EventType> {
    t.events
        .iter()
        .map(|e| EventType::try_from(e.event_type).expect("known"))
        .collect()
}

#[test]
fn start_writes_started_and_scheduled() {
    let t = start();
    assert_eq!(
        types(&t),
        [
            EventType::WorkflowExecutionStarted,
            EventType::WorkflowTaskScheduled
        ]
    );
    assert_eq!(t.state.next_event_id, 3);
    assert!(matches!(
        t.effects[..],
        [Effect::ScheduleWorkflowTask {
            scheduled_event_id: 2,
            attempt: 1,
            ..
        }]
    ));
}

#[test]
fn signal_while_started_is_buffered_then_flushed_with_a_new_task() {
    let s = start().state;
    let s = wft_started(&s, 2_000).state;
    let signal = apply(
        &s,
        Input::Signal {
            name: "go".into(),
            input: None,
            identity: "c".into(),
            header: None,
            request_id: "s1".into(),
        },
        2_100,
    )
    .expect("signal");
    assert!(signal.events.is_empty(), "buffered, not written");
    assert_eq!(signal.state.buffered.len(), 1);
    let done = wft_completed(&signal.state, vec![], 2_200).expect("completed");
    assert_eq!(
        types(&done),
        [
            EventType::WorkflowTaskCompleted,
            EventType::WorkflowExecutionSignaled,
            EventType::WorkflowTaskScheduled
        ]
    );
}

#[test]
fn close_with_buffered_events_fails_the_task() {
    let s = start().state;
    let s = wft_started(&s, 2_000).state;
    let s = apply(
        &s,
        Input::Signal {
            name: "late".into(),
            input: None,
            identity: "c".into(),
            header: None,
            request_id: "s1".into(),
        },
        2_100,
    )
    .expect("signal")
    .state;
    let t = wft_completed(&s, vec![complete()], 2_200).expect("answered");
    assert_eq!(
        types(&t),
        [
            EventType::WorkflowTaskFailed,
            EventType::WorkflowExecutionSignaled,
            EventType::WorkflowTaskScheduled
        ]
    );
    let Some(Attributes::WorkflowTaskFailedEventAttributes(f)) = &t.events[0].attributes else {
        panic!("failed event");
    };
    assert_eq!(f.cause, WorkflowTaskFailedCause::UnhandledCommand as i32);
    assert_eq!(t.state.status, WorkflowExecutionStatus::Running as i32);
}

#[test]
fn stale_token_is_not_found() {
    let s = start().state;
    let s = wft_started(&s, 2_000).state;
    let w = s.workflow_task.clone().expect("task");
    let r = apply(
        &s,
        Input::WorkflowTaskCompleted {
            scheduled_event_id: w.scheduled_event_id,
            attempt: w.attempt + 1,
            identity: "w".into(),
            commands: vec![],
            sdk_metadata: None,
            metering_metadata: None,
            worker_version: None,
            force_create_new_workflow_task: false,
            messages: Vec::new(),
            new_run_id: String::new(),
        },
        2_100,
    );
    assert!(matches!(r, Err(Rejection::NotFound(_))));
}

#[test]
fn closed_run_refuses_signals_and_drops_timeouts() {
    let s = start().state;
    let s = wft_started(&s, 2_000).state;
    let s = wft_completed(&s, vec![complete()], 2_100)
        .expect("done")
        .state;
    assert_eq!(s.status, WorkflowExecutionStatus::Completed as i32);
    let r = apply(
        &s,
        Input::Signal {
            name: "x".into(),
            input: None,
            identity: "c".into(),
            header: None,
            request_id: "s".into(),
        },
        3_000,
    );
    assert_eq!(r, Err(Rejection::NotFound(COMPLETED.into())));
    let t = apply(
        &s,
        Input::TimerFired {
            started_event_id: 5,
        },
        3_000,
    )
    .expect("stale");
    assert_eq!(t.reply, Reply::Stale);
    assert!(t.events.is_empty() && t.effects.is_empty());
}

#[test]
fn timer_fires_once_and_a_second_firing_is_stale() {
    let s = start().state;
    let s = wft_started(&s, 2_000).state;
    let t = wft_completed(&s, vec![start_timer("t1", 60)], 2_100).expect("done");
    assert!(t.effects.iter().any(|e| matches!(
        e,
        Effect::StartTimer {
            started_event_id: 5,
            fire_at_ms: 62_100
        }
    )));
    let fired = apply(
        &t.state,
        Input::TimerFired {
            started_event_id: 5,
        },
        62_100,
    )
    .expect("fired");
    assert_eq!(
        types(&fired),
        [EventType::TimerFired, EventType::WorkflowTaskScheduled]
    );
    let again = apply(
        &fired.state,
        Input::TimerFired {
            started_event_id: 5,
        },
        62_200,
    )
    .expect("ok");
    assert_eq!(again.reply, Reply::Stale);
}

#[test]
fn workflow_task_timeout_schedules_the_next_attempt() {
    let s = start().state;
    let s = wft_started(&s, 2_000).state;
    let t = apply(
        &s,
        Input::WorkflowTaskTimedOut {
            scheduled_event_id: 2,
            attempt: 1,
        },
        12_000,
    )
    .expect("timed out");
    assert_eq!(
        types(&t),
        [
            EventType::WorkflowTaskTimedOut,
            EventType::WorkflowTaskScheduled
        ]
    );
    assert_eq!(t.state.workflow_task.as_ref().map(|w| w.attempt), Some(2));
    // The same timeout again is stale.
    let again = apply(
        &t.state,
        Input::WorkflowTaskTimedOut {
            scheduled_event_id: 2,
            attempt: 1,
        },
        12_001,
    )
    .expect("ok");
    assert_eq!(again.reply, Reply::Stale);
}

#[test]
fn activity_failure_retries_without_history_then_fails() {
    let s = start().state;
    let s = wft_started(&s, 2_000).state;
    let s = wft_completed(&s, vec![schedule_activity("a1", 2)], 2_100)
        .expect("done")
        .state;
    let started = |s: &loams_temporal_history::MutableState, attempt, now| {
        apply(
            s,
            Input::ActivityStarted {
                scheduled_event_id: 5,
                attempt,
                identity: "w".into(),
                request_id: format!("p{attempt}"),
                worker_version: None,
            },
            now,
        )
        .expect("activity started")
    };
    let failed = |s: &loams_temporal_history::MutableState, attempt, now| {
        apply(
            s,
            Input::ActivityFailed {
                scheduled_event_id: 5,
                attempt,
                failure: None,
                identity: "w".into(),
                last_heartbeat_details: None,
            },
            now,
        )
        .expect("activity failed")
    };
    let s1 = started(&s, 1, 3_000);
    assert!(
        s1.events.is_empty(),
        "the started event waits for the outcome"
    );
    let f1 = failed(&s1.state, 1, 3_100);
    assert!(f1.events.is_empty(), "a retried attempt writes nothing");
    assert!(f1.effects.iter().any(|e| matches!(
        e,
        Effect::ScheduleActivity {
            attempt: 2,
            not_before_ms: 4_100,
            ..
        }
    )));
    let s2 = started(&f1.state, 2, 4_200);
    let f2 = failed(&s2.state, 2, 4_300);
    assert_eq!(
        types(&f2),
        [
            EventType::ActivityTaskStarted,
            EventType::ActivityTaskFailed,
            EventType::WorkflowTaskScheduled
        ]
    );
}

#[test]
fn heartbeat_reports_cancel_requests_and_rearms_its_timeout() {
    let s = start().state;
    let s = wft_started(&s, 2_000).state;
    let mut cmd = schedule_activity("a1", 1);
    if let Some(command::Attributes::ScheduleActivityTaskCommandAttributes(a)) = &mut cmd.attributes
    {
        a.heartbeat_timeout = Some(prost_types::Duration {
            seconds: 5,
            nanos: 0,
        });
    }
    let s = wft_completed(&s, vec![cmd], 2_100).expect("done").state;
    let s = apply(
        &s,
        Input::ActivityStarted {
            scheduled_event_id: 5,
            attempt: 1,
            identity: "w".into(),
            request_id: "p".into(),
            worker_version: None,
        },
        3_000,
    )
    .expect("started")
    .state;
    let hb = apply(
        &s,
        Input::ActivityHeartbeat {
            scheduled_event_id: 5,
            attempt: 1,
            details: None,
        },
        4_000,
    )
    .expect("heartbeat");
    assert_eq!(
        hb.reply,
        Reply::Heartbeat {
            cancel_requested: false
        }
    );
    assert!(hb.effects.iter().any(|e| matches!(
        e,
        Effect::ArmTimeout {
            kind: TimeoutKind::Heartbeat,
            at_ms: 9_000,
            ..
        }
    )));
    // The deadline armed at start (8 000) is stale after the heartbeat.
    let stale = apply(
        &hb.state,
        Input::ActivityTimedOut {
            scheduled_event_id: 5,
            attempt: 1,
            kind: TimeoutKind::Heartbeat,
        },
        8_000,
    )
    .expect("ok");
    assert_eq!(stale.reply, Reply::Stale);
}

#[test]
fn later_phase_commands_are_unimplemented() {
    let s = start().state;
    let s = wft_started(&s, 2_000).state;
    let r = wft_completed(
        &s,
        vec![cmd(
            command::Attributes::ScheduleNexusOperationCommandAttributes(
                ScheduleNexusOperationCommandAttributes::default(),
            ),
        )],
        2_100,
    );
    assert!(matches!(r, Err(Rejection::Unimplemented(_))));
}

#[test]
fn terminate_closes_the_run() {
    let s = start().state;
    let t = apply(
        &s,
        Input::Terminate {
            reason: "stop".into(),
            details: None,
            identity: "c".into(),
        },
        2_000,
    )
    .expect("terminated");
    assert_eq!(types(&t), [EventType::WorkflowExecutionTerminated]);
    assert!(t.effects.contains(&Effect::RunClosed {
        status: WorkflowExecutionStatus::Terminated
    }));
}
