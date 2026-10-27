//! TMP-api group B (D2083–D2085): signal-with-start, terminate with a
//! started workflow task, reset and reset points.

mod common;

use common::*;
use loams_temporal_history::{Attributes, EventType, HistoryEvent, MutableState, fold};
use loams_temporal_proto::temporal::api::common::v1::{WorkerVersionStamp, WorkflowType};
use loams_temporal_proto::temporal::api::enums::v1::WorkflowTaskFailedCause;
use loams_temporal_proto::temporal::api::taskqueue::v1::TaskQueue;
use loams_temporal_proto::temporal::api::workflowservice::v1::StartWorkflowExecutionRequest;
use loams_temporal_statemachine::reset::{ResetRequest, reset, reset_points};
use loams_temporal_statemachine::{
    Effect, HISTORY_SERVICE, Input, Rejection, StartSignal, Transition, apply,
};

fn types(events: &[HistoryEvent]) -> Vec<EventType> {
    events
        .iter()
        .map(|e| EventType::try_from(e.event_type).expect("known"))
        .collect()
}

fn signal(name: &str) -> Input {
    Input::Signal {
        name: name.into(),
        input: None,
        identity: "c".into(),
        header: None,
        request_id: format!("sig-{name}"),
    }
}

#[test]
fn signal_with_start_writes_the_signal_before_the_first_task() {
    let t = apply(
        &MutableState::default(),
        Input::SignalWithStart {
            namespace_id: "ns".into(),
            run_id: "run".into(),
            request: Box::new(StartWorkflowExecutionRequest {
                workflow_id: "wf".into(),
                workflow_type: Some(WorkflowType { name: "t".into() }),
                task_queue: Some(TaskQueue {
                    name: "q".into(),
                    ..Default::default()
                }),
                request_id: "r".into(),
                ..Default::default()
            }),
            signal: StartSignal {
                name: "go".into(),
                ..Default::default()
            },
        },
        1_000,
    )
    .expect("started");
    assert_eq!(
        types(&t.events),
        [
            EventType::WorkflowExecutionStarted,
            EventType::WorkflowExecutionSignaled,
            EventType::WorkflowTaskScheduled
        ]
    );
    assert_eq!(t.state.signal_count, 1);
    assert!(matches!(
        t.effects[..],
        [Effect::ScheduleWorkflowTask {
            scheduled_event_id: 3,
            ..
        }]
    ));
}

#[test]
fn signal_with_start_needs_a_signal_name() {
    let e = apply(
        &MutableState::default(),
        Input::SignalWithStart {
            namespace_id: "ns".into(),
            run_id: "run".into(),
            request: Box::default(),
            signal: StartSignal::default(),
        },
        1_000,
    )
    .expect_err("no name");
    assert!(matches!(e, Rejection::InvalidArgument(_)));
}

#[test]
fn terminate_fails_a_started_task_first_and_flushes_what_waited() {
    let s = start().state;
    let s = wft_started(&s, 2_000).state;
    let s = apply(&s, signal("late"), 2_050).expect("signal").state;
    let t = apply(
        &s,
        Input::Terminate {
            reason: "stop".into(),
            details: None,
            identity: "op".into(),
        },
        2_100,
    )
    .expect("terminated");
    assert_eq!(
        types(&t.events),
        [
            EventType::WorkflowTaskFailed,
            EventType::WorkflowExecutionSignaled,
            EventType::WorkflowExecutionTerminated
        ]
    );
    match t.events[0].attributes.as_ref() {
        Some(Attributes::WorkflowTaskFailedEventAttributes(a)) => {
            assert_eq!(a.cause, WorkflowTaskFailedCause::ForceCloseCommand as i32);
            assert_eq!(a.identity, HISTORY_SERVICE);
        }
        other => panic!("{other:?}"),
    }
    assert!(
        t.effects
            .iter()
            .any(|e| matches!(e, Effect::WorkflowTaskDone { .. }))
    );
}

/// The history of a run that ran one activity and completed, with every
/// event collected: the shape of the corpus's
/// `reset_to_first_workflow_task.run1`, plus a signal after the first task.
fn base_run() -> (MutableState, Vec<HistoryEvent>) {
    let mut events = Vec::new();
    let push = |t: Transition, events: &mut Vec<HistoryEvent>| {
        events.extend(t.events.clone());
        t.state
    };
    let s = push(start(), &mut events);
    let s = push(wft_started(&s, 2_000), &mut events);
    let s = push(
        wft_completed(&s, vec![schedule_activity("a1", 1)], 2_100).expect("first"),
        &mut events,
    );
    let s = push(
        apply(&s, signal("later"), 2_200).expect("signal"),
        &mut events,
    );
    (s, events)
}

#[test]
fn reset_to_the_first_task_copies_the_prefix_and_fails_the_task() {
    let (_, base) = base_run();
    let first_completed = base
        .iter()
        .find(|e| e.event_type == EventType::WorkflowTaskCompleted as i32)
        .expect("a completed task")
        .event_id;
    let run = reset(
        &base,
        &ResetRequest {
            namespace_id: "ns".into(),
            base_run_id: "run".into(),
            new_run_id: "run2".into(),
            workflow_task_finish_event_id: first_completed,
            reason: "corpus: reset to the first workflow task".into(),
            reapply_signals: true,
        },
        9_000,
    )
    .expect("reset");
    assert_eq!(
        types(&run.events),
        [
            EventType::WorkflowExecutionStarted,
            EventType::WorkflowTaskScheduled,
            EventType::WorkflowTaskStarted,
            EventType::WorkflowTaskFailed,
            EventType::WorkflowExecutionSignaled,
            EventType::WorkflowTaskScheduled,
        ]
    );
    match run.events[3].attributes.as_ref() {
        Some(Attributes::WorkflowTaskFailedEventAttributes(a)) => {
            assert_eq!(a.cause, WorkflowTaskFailedCause::ResetWorkflow as i32);
            assert_eq!(a.identity, HISTORY_SERVICE);
            assert_eq!(a.base_run_id, "run");
            assert_eq!(a.new_run_id, "run2");
            assert_eq!(
                a.failure.as_ref().map(|f| f.message.as_str()),
                Some("corpus: reset to the first workflow task")
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(run.state.run_id, "run2");
    assert!(run.state.activities.is_empty(), "the activity came later");
    assert_eq!(run.state.next_event_id, 7);
    // The new run's history folds back to its state.
    let folded = fold("ns", "run2", &run.events).expect("folds");
    assert_eq!(folded.next_event_id, run.state.next_event_id);
    assert_eq!(folded.workflow_task.map(|w| w.scheduled_event_id), Some(6));
    assert!(matches!(
        run.effects.last(),
        Some(Effect::ScheduleWorkflowTask {
            scheduled_event_id: 6,
            attempt: 1,
            ..
        })
    ));
}

#[test]
fn reset_without_signals_writes_none() {
    let (_, base) = base_run();
    let first_started = base
        .iter()
        .find(|e| e.event_type == EventType::WorkflowTaskStarted as i32)
        .expect("started")
        .event_id;
    let run = reset(
        &base,
        &ResetRequest {
            namespace_id: "ns".into(),
            base_run_id: "run".into(),
            new_run_id: "run2".into(),
            workflow_task_finish_event_id: first_started,
            reason: "r".into(),
            reapply_signals: false,
        },
        9_000,
    )
    .expect("reset");
    assert!(
        !types(&run.events).contains(&EventType::WorkflowExecutionSignaled),
        "signals excluded"
    );
}

#[test]
fn reset_redispatches_activities_pending_at_the_point() {
    // Start, task 1 schedules an activity, task 2 starts: reset to task 2.
    let mut events = Vec::new();
    let t = start();
    events.extend(t.events.clone());
    let t = wft_started(&t.state, 2_000);
    events.extend(t.events.clone());
    let t = wft_completed(&t.state, vec![schedule_activity("a1", 1)], 2_100).expect("first");
    events.extend(t.events.clone());
    let t = apply(&t.state, signal("poke"), 2_200).expect("signal");
    events.extend(t.events.clone());
    let t = wft_started(&t.state, 2_300);
    events.extend(t.events.clone());
    let started = t.events[0].event_id;
    let run = reset(
        &events,
        &ResetRequest {
            namespace_id: "ns".into(),
            base_run_id: "run".into(),
            new_run_id: "run2".into(),
            workflow_task_finish_event_id: started,
            reason: "r".into(),
            reapply_signals: true,
        },
        9_000,
    )
    .expect("reset");
    assert_eq!(run.state.activities.len(), 1);
    assert!(run.effects.iter().any(|e| matches!(
        e,
        Effect::ScheduleActivity {
            attempt: 1,
            not_before_ms: 0,
            ..
        }
    )));
}

#[test]
fn reset_refuses_an_event_that_is_not_a_task_boundary() {
    let (_, base) = base_run();
    let e = reset(
        &base,
        &ResetRequest {
            namespace_id: "ns".into(),
            base_run_id: "run".into(),
            new_run_id: "run2".into(),
            workflow_task_finish_event_id: 1,
            reason: "r".into(),
            reapply_signals: true,
        },
        9_000,
    )
    .expect_err("not a task");
    assert!(matches!(e, Rejection::InvalidArgument(_)));
}

#[test]
fn reset_points_are_the_first_completion_of_each_build() {
    let mut events = Vec::new();
    let t = start();
    events.extend(t.events.clone());
    let mut s = t.state;
    for (i, build) in ["b1", "b1", "b2"].iter().enumerate() {
        let now = 2_000 + i as i64 * 100;
        let t = wft_started(&s, now);
        events.extend(t.events.clone());
        let w = t.state.workflow_task.clone().expect("wft");
        let t = apply(
            &t.state,
            Input::WorkflowTaskCompleted {
                scheduled_event_id: w.scheduled_event_id,
                attempt: w.attempt,
                identity: "worker".into(),
                commands: vec![],
                sdk_metadata: None,
                metering_metadata: None,
                worker_version: Some(WorkerVersionStamp {
                    build_id: (*build).into(),
                    use_versioning: false,
                }),
                force_create_new_workflow_task: true,
                messages: Vec::new(),
                new_run_id: String::new(),
            },
            now + 50,
        )
        .expect("completed");
        events.extend(t.events.clone());
        s = t.state;
    }
    let points = reset_points("run", &events);
    assert_eq!(
        points
            .iter()
            .map(|p| p.build_id.as_str())
            .collect::<Vec<_>>(),
        ["b1", "b2"]
    );
    assert!(points.iter().all(|p| p.resettable && p.run_id == "run"));
}
