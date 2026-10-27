//! TMP-api group D (D2087): child workflows, continue-as-new, external
//! signals and cancels, upserts, parent close policy, first-task backoff.

mod common;

use common::*;
use loams_temporal_history::{Attributes, EventType, MutableState, ParentInfo, fold};
use loams_temporal_proto::temporal::api::command::v1::{
    Command, ContinueAsNewWorkflowExecutionCommandAttributes,
    RequestCancelExternalWorkflowExecutionCommandAttributes,
    SignalExternalWorkflowExecutionCommandAttributes, StartChildWorkflowExecutionCommandAttributes,
    UpsertWorkflowSearchAttributesCommandAttributes, command,
};
use loams_temporal_proto::temporal::api::common::v1::{
    Payload, SearchAttributes, WorkflowExecution, WorkflowType,
};
use loams_temporal_proto::temporal::api::enums::v1::{
    ParentClosePolicy, SignalExternalWorkflowExecutionFailedCause, WorkflowExecutionStatus,
};
use loams_temporal_proto::temporal::api::taskqueue::v1::TaskQueue;
use loams_temporal_proto::temporal::api::workflowservice::v1::StartWorkflowExecutionRequest;
use loams_temporal_statemachine::children::{ChildOutcome, ExternalKind};
use loams_temporal_statemachine::{Effect, Input, Reply, StartChain, Transition, apply};

fn types(t: &Transition) -> Vec<EventType> {
    t.events
        .iter()
        .map(|e| EventType::try_from(e.event_type).expect("known"))
        .collect()
}

fn complete_with(
    s: &MutableState,
    commands: Vec<Command>,
    now: i64,
    new_run_id: &str,
) -> Transition {
    let w = s.workflow_task.as_ref().expect("a workflow task");
    apply(
        s,
        Input::WorkflowTaskCompleted {
            scheduled_event_id: w.scheduled_event_id,
            attempt: w.attempt,
            identity: "worker".into(),
            commands,
            sdk_metadata: None,
            metering_metadata: None,
            worker_version: None,
            force_create_new_workflow_task: false,
            messages: Vec::new(),
            new_run_id: new_run_id.into(),
        },
        now,
    )
    .expect("completed")
}

fn start_child(policy: ParentClosePolicy) -> Command {
    cmd(
        command::Attributes::StartChildWorkflowExecutionCommandAttributes(
            StartChildWorkflowExecutionCommandAttributes {
                workflow_id: "child".into(),
                workflow_type: Some(WorkflowType {
                    name: "child_greeter".into(),
                }),
                parent_close_policy: policy as i32,
                ..Default::default()
            },
        ),
    )
}

/// A parent whose first task started a child.
fn parent_with_child(policy: ParentClosePolicy) -> Transition {
    let s = start().state;
    let s = wft_started(&s, 2_000).state;
    complete_with(&s, vec![start_child(policy)], 2_100, "")
}

#[test]
fn a_child_is_initiated_started_and_completed_as_temporal_writes_it() {
    let t = parent_with_child(ParentClosePolicy::Terminate);
    assert_eq!(
        types(&t),
        [
            EventType::WorkflowTaskCompleted,
            EventType::StartChildWorkflowExecutionInitiated
        ]
    );
    let initiated = t.events[1].event_id;
    assert!(t.effects.contains(&Effect::StartChild {
        initiated_event_id: initiated
    }));
    match t.events[1].attributes.as_ref() {
        Some(Attributes::StartChildWorkflowExecutionInitiatedEventAttributes(a)) => {
            assert_eq!(a.namespace_id, "ns");
            assert_eq!(a.task_queue.as_ref().map(|q| q.name.as_str()), Some("q"));
            assert_eq!(a.parent_close_policy, ParentClosePolicy::Terminate as i32);
        }
        other => panic!("{other:?}"),
    }
    let started = apply(
        &t.state,
        Input::ChildStarted {
            initiated_event_id: initiated,
            run_id: "c1".into(),
            header: None,
        },
        2_200,
    )
    .expect("started");
    assert_eq!(
        types(&started),
        [
            EventType::ChildWorkflowExecutionStarted,
            EventType::WorkflowTaskScheduled
        ]
    );
    let started_id = started.events[0].event_id;
    let s = wft_started(&started.state, 2_300).state;
    let s = complete_with(&s, vec![], 2_400, "").state;
    let done = apply(
        &s,
        Input::ChildClosed {
            initiated_event_id: initiated,
            run_id: "c1".into(),
            outcome: ChildOutcome::Completed(None),
        },
        2_500,
    )
    .expect("closed");
    assert_eq!(
        types(&done),
        [
            EventType::ChildWorkflowExecutionCompleted,
            EventType::WorkflowTaskScheduled
        ]
    );
    match done.events[0].attributes.as_ref() {
        Some(Attributes::ChildWorkflowExecutionCompletedEventAttributes(a)) => {
            assert_eq!(a.initiated_event_id, initiated);
            assert_eq!(a.started_event_id, started_id);
        }
        other => panic!("{other:?}"),
    }
    assert!(done.state.children.is_empty());
    // A duplicate answer changes nothing.
    let again = apply(
        &done.state,
        Input::ChildClosed {
            initiated_event_id: initiated,
            run_id: "c1".into(),
            outcome: ChildOutcome::Terminated,
        },
        2_600,
    )
    .expect("stale");
    assert_eq!(again.reply, Reply::Stale);
}

#[test]
fn a_close_that_overtakes_the_start_writes_both_linked() {
    let t = parent_with_child(ParentClosePolicy::Abandon);
    let initiated = t.events[1].event_id;
    let s = wft_started(
        &apply(&t.state, signal("poke"), 2_150)
            .expect("signal")
            .state,
        2_160,
    );
    let done = apply(
        &s.state,
        Input::ChildClosed {
            initiated_event_id: initiated,
            run_id: "c1".into(),
            outcome: ChildOutcome::Failed(None, Default::default()),
        },
        2_200,
    )
    .expect("closed");
    assert!(done.events.is_empty(), "buffered behind the started task");
    let flushed = complete_with(&done.state, vec![], 2_300, "");
    let started = flushed
        .events
        .iter()
        .find(|e| e.event_type == EventType::ChildWorkflowExecutionStarted as i32)
        .expect("started")
        .event_id;
    match flushed
        .events
        .iter()
        .find_map(|e| match e.attributes.as_ref() {
            Some(Attributes::ChildWorkflowExecutionFailedEventAttributes(a)) => Some(a.clone()),
            _ => None,
        }) {
        Some(a) => assert_eq!(a.started_event_id, started),
        None => panic!("a failed event"),
    }
}

fn signal(name: &str) -> Input {
    Input::Signal {
        name: name.into(),
        input: None,
        identity: "c".into(),
        header: None,
        request_id: format!("s-{name}"),
    }
}

#[test]
fn a_failed_child_start_is_recorded() {
    let t = parent_with_child(ParentClosePolicy::Terminate);
    let initiated = t.events[1].event_id;
    let f = apply(
        &t.state,
        Input::ChildStartFailed {
            initiated_event_id: initiated,
            cause: 1,
        },
        2_200,
    )
    .expect("failed");
    assert_eq!(
        types(&f),
        [
            EventType::StartChildWorkflowExecutionFailed,
            EventType::WorkflowTaskScheduled
        ]
    );
    assert!(f.state.children.is_empty());
}

#[test]
fn closing_the_parent_applies_the_close_policy() {
    let t = parent_with_child(ParentClosePolicy::RequestCancel);
    let initiated = t.events[1].event_id;
    let s = apply(
        &t.state,
        Input::ChildStarted {
            initiated_event_id: initiated,
            run_id: "c1".into(),
            header: None,
        },
        2_200,
    )
    .expect("started")
    .state;
    let s = wft_started(&s, 2_300).state;
    let done = complete_with(&s, vec![complete()], 2_400, "");
    let policy = done.effects.iter().find_map(|e| match e {
        Effect::ParentClosePolicy { children } => Some(children.clone()),
        _ => None,
    });
    let children = policy.expect("a close policy effect");
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].run_id, "c1");
    assert_eq!(children[0].policy, ParentClosePolicy::RequestCancel as i32);
    assert!(done.state.children.is_empty());
}

#[test]
fn continue_as_new_closes_the_run_and_names_the_next() {
    let s = start().state;
    let s = wft_started(&s, 2_000).state;
    let t = complete_with(
        &s,
        vec![cmd(
            command::Attributes::ContinueAsNewWorkflowExecutionCommandAttributes(
                ContinueAsNewWorkflowExecutionCommandAttributes::default(),
            ),
        )],
        2_100,
        "next",
    );
    assert_eq!(
        types(&t),
        [
            EventType::WorkflowTaskCompleted,
            EventType::WorkflowExecutionContinuedAsNew
        ]
    );
    assert_eq!(
        t.state.status,
        WorkflowExecutionStatus::ContinuedAsNew as i32
    );
    assert!(t.effects.contains(&Effect::ContinuedAsNew {
        new_run_id: "next".into()
    }));
    match t.events[1].attributes.as_ref() {
        Some(Attributes::WorkflowExecutionContinuedAsNewEventAttributes(a)) => {
            assert_eq!(a.new_execution_run_id, "next");
            assert_eq!(a.workflow_type.as_ref().map(|t| t.name.as_str()), Some("t"));
            assert_eq!(a.task_queue.as_ref().map(|q| q.name.as_str()), Some("q"));
            assert_eq!(a.workflow_task_completed_event_id, t.events[0].event_id);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_chained_start_records_its_lineage_and_waits_out_its_backoff() {
    let t = apply(
        &MutableState::default(),
        Input::StartChained {
            namespace_id: "ns".into(),
            run_id: "run2".into(),
            request: Box::new(StartWorkflowExecutionRequest {
                namespace: "default".into(),
                workflow_id: "wf".into(),
                workflow_type: Some(WorkflowType { name: "t".into() }),
                task_queue: Some(TaskQueue {
                    name: "q".into(),
                    ..Default::default()
                }),
                request_id: "r2".into(),
                ..Default::default()
            }),
            chain: Box::new(StartChain {
                parent: Some(ParentInfo {
                    namespace: "default".into(),
                    namespace_id: "ns".into(),
                    workflow_id: "parent".into(),
                    run_id: "p1".into(),
                    initiated_event_id: 5,
                }),
                continued_execution_run_id: "run1".into(),
                initiator: 1,
                first_execution_run_id: "run0".into(),
                backoff_ms: 1_000,
                ..Default::default()
            }),
        },
        1_000,
    )
    .expect("started");
    assert_eq!(types(&t), [EventType::WorkflowExecutionStarted]);
    assert!(t.effects.contains(&Effect::StartTimer {
        started_event_id: 1,
        fire_at_ms: 2_000
    }));
    match t.events[0].attributes.as_ref() {
        Some(Attributes::WorkflowExecutionStartedEventAttributes(a)) => {
            assert_eq!(a.continued_execution_run_id, "run1");
            assert_eq!(a.first_execution_run_id, "run0");
            assert_eq!(a.original_execution_run_id, "run2");
            assert_eq!(a.parent_initiated_event_id, 5);
        }
        other => panic!("{other:?}"),
    }
    // A signal during the backoff does not schedule a task early.
    let s = apply(&t.state, signal("early"), 1_500).expect("signal");
    assert_eq!(types(&s), [EventType::WorkflowExecutionSignaled]);
    let fired = apply(
        &s.state,
        Input::TimerFired {
            started_event_id: 1,
        },
        2_000,
    )
    .expect("fired");
    assert_eq!(types(&fired), [EventType::WorkflowTaskScheduled]);
    let again = apply(
        &fired.state,
        Input::TimerFired {
            started_event_id: 1,
        },
        2_001,
    )
    .expect("stale");
    assert_eq!(again.reply, Reply::Stale);
    let folded = fold(
        "ns",
        "run2",
        &[t.events.clone(), s.events.clone(), fired.events.clone()].concat(),
    )
    .expect("folds");
    assert_eq!(folded.parent, t.state.parent);
    assert!(!folded.first_task_backoff);
}

#[test]
fn external_signals_and_cancels_round_trip() {
    let s = start().state;
    let s = wft_started(&s, 2_000).state;
    let t = complete_with(
        &s,
        vec![
            cmd(
                command::Attributes::SignalExternalWorkflowExecutionCommandAttributes(
                    SignalExternalWorkflowExecutionCommandAttributes {
                        execution: Some(WorkflowExecution {
                            workflow_id: "other".into(),
                            run_id: String::new(),
                        }),
                        signal_name: "hello".into(),
                        ..Default::default()
                    },
                ),
            ),
            cmd(
                command::Attributes::RequestCancelExternalWorkflowExecutionCommandAttributes(
                    RequestCancelExternalWorkflowExecutionCommandAttributes {
                        workflow_id: "other".into(),
                        ..Default::default()
                    },
                ),
            ),
        ],
        2_100,
        "",
    );
    assert_eq!(
        types(&t),
        [
            EventType::WorkflowTaskCompleted,
            EventType::SignalExternalWorkflowExecutionInitiated,
            EventType::RequestCancelExternalWorkflowExecutionInitiated,
        ]
    );
    let (sig, can) = (t.events[1].event_id, t.events[2].event_id);
    let a = apply(
        &t.state,
        Input::ExternalResult {
            kind: ExternalKind::Signal,
            initiated_event_id: sig,
            failed_cause: Some(
                SignalExternalWorkflowExecutionFailedCause::ExternalWorkflowExecutionNotFound
                    as i32,
            ),
        },
        2_200,
    )
    .expect("signal failed");
    let b = apply(
        &a.state,
        Input::ExternalResult {
            kind: ExternalKind::Cancel,
            initiated_event_id: can,
            failed_cause: None,
        },
        2_300,
    )
    .expect("cancel requested");
    assert_eq!(
        types(&a)[0],
        EventType::SignalExternalWorkflowExecutionFailed
    );
    assert_eq!(
        types(&b)[0],
        EventType::ExternalWorkflowExecutionCancelRequested
    );
    assert!(b.state.external_signals.is_empty() && b.state.external_cancels.is_empty());
}

#[test]
fn upserted_search_attributes_are_kept() {
    let s = start().state;
    let s = wft_started(&s, 2_000).state;
    let mut sa = SearchAttributes::default();
    sa.indexed_fields.insert(
        "CustomKeywordField".into(),
        Payload {
            data: b"\"x\"".to_vec().into(),
            ..Default::default()
        },
    );
    let t = complete_with(
        &s,
        vec![cmd(
            command::Attributes::UpsertWorkflowSearchAttributesCommandAttributes(
                UpsertWorkflowSearchAttributesCommandAttributes {
                    search_attributes: Some(sa.clone()),
                },
            ),
        )],
        2_100,
        "",
    );
    assert_eq!(
        types(&t),
        [
            EventType::WorkflowTaskCompleted,
            EventType::UpsertWorkflowSearchAttributes
        ]
    );
    assert_eq!(t.state.search_attributes, Some(sa));
}
