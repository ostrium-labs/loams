//! Workflow updates (TMP-api, D2080–D2082): admission, delivery with a
//! workflow task, acceptance, rejection and completion, and what a failed
//! task or a closing run does to them.

mod common;

use common::*;
use loams_temporal_history::{EventType, MutableState, UpdateState, fold};
use loams_temporal_proto::temporal::api::command::v1::{ProtocolMessageCommandAttributes, command};
use loams_temporal_proto::temporal::api::common::v1::{Payload, Payloads};
use loams_temporal_proto::temporal::api::enums::v1::WorkflowTaskFailedCause;
use loams_temporal_proto::temporal::api::failure::v1::Failure;
use loams_temporal_proto::temporal::api::protocol::v1::{Message, message};
use loams_temporal_proto::temporal::api::update::v1::{
    Acceptance, Input as UpdateInput, Meta, Outcome, Rejection as UpdateRejection, Request,
    Response, outcome,
};
use loams_temporal_statemachine::update::{
    ACCEPTANCE_TYPE, REJECTION_TYPE, REQUEST_TYPE, RESPONSE_TYPE, UNPROCESSED_MESSAGE,
    UpdateStatus, request_message_id, status,
};
use loams_temporal_statemachine::{
    COMPLETED, Input, Rejection, Reply, Transition, apply, messages_for_task,
};
use prost::Message as _;

fn types(t: &Transition) -> Vec<EventType> {
    t.events
        .iter()
        .map(|e| EventType::try_from(e.event_type).expect("known"))
        .collect()
}

fn request(id: &str) -> Request {
    Request {
        meta: Some(Meta {
            update_id: id.into(),
            identity: "client".into(),
        }),
        input: Some(UpdateInput {
            name: "setValue".into(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn admit(s: &MutableState, id: &str, now: i64) -> Transition {
    apply(
        s,
        Input::UpdateAdmitted {
            request: Box::new(request(id)),
        },
        now,
    )
    .expect("admitted")
}

fn body<M: prost::Message>(type_url: &str, m: &M) -> Option<prost_types::Any> {
    Some(prost_types::Any {
        type_url: type_url.into(),
        value: m.encode_to_vec(),
    })
}

fn acceptance(id: &str, seq: i64) -> Message {
    Message {
        id: format!("{id}/accept"),
        protocol_instance_id: id.into(),
        sequencing_id: None,
        body: body(
            ACCEPTANCE_TYPE,
            &Acceptance {
                accepted_request_message_id: request_message_id(id),
                accepted_request_sequencing_event_id: seq,
                accepted_request: Some(request(id)),
            },
        ),
    }
}

fn response(id: &str, value: &[u8]) -> Message {
    Message {
        id: format!("{id}/complete"),
        protocol_instance_id: id.into(),
        sequencing_id: None,
        body: body(
            RESPONSE_TYPE,
            &Response {
                meta: Some(Meta {
                    update_id: id.into(),
                    identity: "worker".into(),
                }),
                outcome: Some(Outcome {
                    value: Some(outcome::Value::Success(Payloads {
                        payloads: vec![Payload {
                            data: value.to_vec().into(),
                            ..Default::default()
                        }],
                    })),
                }),
            },
        ),
    }
}

fn rejection(id: &str) -> Message {
    Message {
        id: format!("{id}/reject"),
        protocol_instance_id: id.into(),
        sequencing_id: None,
        body: body(
            REJECTION_TYPE,
            &UpdateRejection {
                rejected_request_message_id: request_message_id(id),
                failure: Some(Failure {
                    message: "invalid value".into(),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ),
    }
}

fn protocol(m: &Message) -> loams_temporal_proto::temporal::api::command::v1::Command {
    cmd(command::Attributes::ProtocolMessageCommandAttributes(
        ProtocolMessageCommandAttributes {
            message_id: m.id.clone(),
        },
    ))
}

fn complete_with(
    s: &MutableState,
    commands: Vec<loams_temporal_proto::temporal::api::command::v1::Command>,
    messages: Vec<Message>,
    now: i64,
) -> Result<Transition, Rejection> {
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
            messages,
            new_run_id: String::new(),
        },
        now,
    )
}

/// A run whose first workflow task completed with nothing, so it is idle.
fn idle() -> MutableState {
    let s = start().state;
    let s = wft_started(&s, 2_000).state;
    complete_with(&s, vec![], vec![], 2_100)
        .expect("first task")
        .state
}

#[test]
fn admission_on_an_idle_run_schedules_a_workflow_task_and_writes_no_update_event() {
    let t = admit(&idle(), "u1", 3_000);
    assert_eq!(types(&t), [EventType::WorkflowTaskScheduled]);
    assert_eq!(status(&t.state, "u1"), Some(UpdateStatus::Admitted));
}

#[test]
fn admission_while_a_task_is_scheduled_writes_nothing() {
    let s = start().state;
    let t = admit(&s, "u1", 1_500);
    assert!(t.events.is_empty());
    assert_eq!(t.state.updates.len(), 1);
}

#[test]
fn a_retried_update_id_changes_nothing() {
    let s = admit(&idle(), "u1", 3_000).state;
    let again = apply(
        &s,
        Input::UpdateAdmitted {
            request: Box::new(request("u1")),
        },
        3_100,
    )
    .expect("retry");
    assert_eq!(again.reply, Reply::Stale);
    assert_eq!(again.state, s);
}

#[test]
fn admission_needs_an_id_and_a_name() {
    let s = idle();
    let mut r = request("");
    let e = apply(
        &s,
        Input::UpdateAdmitted {
            request: Box::new(r.clone()),
        },
        3_000,
    )
    .expect_err("no id");
    assert!(matches!(e, Rejection::InvalidArgument(_)));
    r = request("u1");
    r.input = None;
    let e = apply(
        &s,
        Input::UpdateAdmitted {
            request: Box::new(r),
        },
        3_000,
    )
    .expect_err("no name");
    assert!(matches!(e, Rejection::InvalidArgument(_)));
}

#[test]
fn the_started_task_carries_the_request_message() {
    let s = admit(&idle(), "u1", 3_000).state;
    let t = wft_started(&s, 3_100);
    let Reply::WorkflowTaskStarted {
        scheduled_event_id,
        started_event_id,
        ..
    } = t.reply
    else {
        panic!("started");
    };
    let messages = messages_for_task(&t.state, scheduled_event_id, started_event_id);
    assert_eq!(messages.len(), 1);
    let m = &messages[0];
    assert_eq!(m.id, "u1/request");
    assert_eq!(m.protocol_instance_id, "u1");
    assert_eq!(
        m.sequencing_id,
        Some(message::SequencingId::EventId(started_event_id - 1))
    );
    let body = m.body.as_ref().expect("body");
    assert_eq!(body.type_url, REQUEST_TYPE);
    assert_eq!(
        Request::decode(body.value.as_ref()).expect("request"),
        request("u1")
    );
    assert_eq!(
        loams_temporal_history::update(&t.state, "u1").map(|u| u.state()),
        Some(UpdateState::Sent)
    );
}

#[test]
fn accept_and_complete_in_one_task_write_both_events_in_command_order() {
    let s = admit(&idle(), "u1", 3_000).state;
    let s = wft_started(&s, 3_100).state;
    let started = s.workflow_task.as_ref().expect("wft").started_event_id;
    let accept = acceptance("u1", started - 1);
    let respond = response("u1", b"42");
    let t = complete_with(
        &s,
        vec![protocol(&accept), protocol(&respond)],
        vec![accept, respond],
        3_200,
    )
    .expect("completed");
    assert_eq!(
        types(&t),
        [
            EventType::WorkflowTaskCompleted,
            EventType::WorkflowExecutionUpdateAccepted,
            EventType::WorkflowExecutionUpdateCompleted,
        ]
    );
    let accepted_id = t.events[1].event_id;
    match status(&t.state, "u1") {
        Some(UpdateStatus::Completed {
            outcome: None,
            completed_event_id,
        }) => assert_eq!(completed_event_id, t.events[2].event_id),
        other => panic!("completed, got {other:?}"),
    }
    match t.events[2].attributes.as_ref() {
        Some(
            loams_temporal_history::Attributes::WorkflowExecutionUpdateCompletedEventAttributes(a),
        ) => {
            assert_eq!(a.accepted_event_id, accepted_id);
            assert_eq!(a.meta.as_ref().map(|m| m.update_id.as_str()), Some("u1"));
        }
        other => panic!("completed event, got {other:?}"),
    }
    match t.events[1].attributes.as_ref() {
        Some(
            loams_temporal_history::Attributes::WorkflowExecutionUpdateAcceptedEventAttributes(a),
        ) => {
            assert_eq!(a.protocol_instance_id, "u1");
            assert_eq!(a.accepted_request_message_id, "u1/request");
            assert_eq!(a.accepted_request_sequencing_event_id, started - 1);
            assert_eq!(a.accepted_request, Some(request("u1")));
        }
        other => panic!("accepted event, got {other:?}"),
    }
}

#[test]
fn messages_without_commands_are_handled_after_the_commands() {
    let s = admit(&idle(), "u1", 3_000).state;
    let s = wft_started(&s, 3_100).state;
    let t = complete_with(
        &s,
        vec![start_timer("t1", 10)],
        vec![acceptance("u1", 0)],
        3_200,
    )
    .expect("completed");
    assert_eq!(
        types(&t),
        [
            EventType::WorkflowTaskCompleted,
            EventType::TimerStarted,
            EventType::WorkflowExecutionUpdateAccepted,
        ]
    );
    assert_eq!(status(&t.state, "u1"), Some(UpdateStatus::Accepted));
}

#[test]
fn messages_go_before_a_closing_command() {
    let s = admit(&idle(), "u1", 3_000).state;
    let s = wft_started(&s, 3_100).state;
    let t = complete_with(
        &s,
        vec![complete()],
        vec![acceptance("u1", 0), response("u1", b"x")],
        3_200,
    )
    .expect("completed");
    assert_eq!(
        types(&t),
        [
            EventType::WorkflowTaskCompleted,
            EventType::WorkflowExecutionUpdateAccepted,
            EventType::WorkflowExecutionUpdateCompleted,
            EventType::WorkflowExecutionCompleted,
        ]
    );
}

#[test]
fn a_rejection_writes_no_update_event_and_keeps_the_failure() {
    let s = admit(&idle(), "u1", 3_000).state;
    let s = wft_started(&s, 3_100).state;
    let t = complete_with(&s, vec![], vec![rejection("u1")], 3_200).expect("completed");
    assert_eq!(types(&t), [EventType::WorkflowTaskCompleted]);
    match status(&t.state, "u1") {
        Some(UpdateStatus::Completed {
            outcome:
                Some(Outcome {
                    value: Some(outcome::Value::Failure(f)),
                }),
            completed_event_id: 0,
        }) => assert_eq!(f.message, "invalid value"),
        other => panic!("rejected, got {other:?}"),
    }
}

#[test]
fn an_unanswered_update_is_rejected_as_unprocessed() {
    let s = admit(&idle(), "u1", 3_000).state;
    let s = wft_started(&s, 3_100).state;
    let t = complete_with(&s, vec![], vec![], 3_200).expect("completed");
    match status(&t.state, "u1") {
        Some(UpdateStatus::Completed {
            outcome:
                Some(Outcome {
                    value: Some(outcome::Value::Failure(f)),
                }),
            ..
        }) => assert_eq!(f.message, UNPROCESSED_MESSAGE),
        other => panic!("rejected, got {other:?}"),
    }
}

#[test]
fn a_response_before_acceptance_is_refused() {
    let s = admit(&idle(), "u1", 3_000).state;
    let s = wft_started(&s, 3_100).state;
    let e = complete_with(&s, vec![], vec![response("u1", b"x")], 3_200).expect_err("refused");
    assert!(matches!(e, Rejection::InvalidArgument(_)));
}

#[test]
fn a_message_for_an_unknown_update_is_refused() {
    let s = wft_started(&idle_with_task(), 3_100).state;
    let e = complete_with(&s, vec![], vec![acceptance("nope", 0)], 3_200).expect_err("refused");
    assert!(matches!(e, Rejection::InvalidArgument(_)));
}

fn idle_with_task() -> MutableState {
    let s = idle();
    apply(
        &s,
        Input::Signal {
            name: "s".into(),
            input: None,
            identity: "c".into(),
            header: None,
            request_id: "r".into(),
        },
        3_000,
    )
    .expect("signal")
    .state
}

#[test]
fn an_update_admitted_during_a_task_gets_the_next_task() {
    let s = idle_with_task();
    let s = wft_started(&s, 3_100).state;
    let s = admit(&s, "u1", 3_150);
    assert!(s.events.is_empty(), "the started task finishes first");
    let t = complete_with(&s.state, vec![], vec![], 3_200).expect("completed");
    assert_eq!(
        types(&t),
        [
            EventType::WorkflowTaskCompleted,
            EventType::WorkflowTaskScheduled
        ]
    );
    assert_eq!(status(&t.state, "u1"), Some(UpdateStatus::Admitted));
}

#[test]
fn closing_with_an_admitted_update_fails_the_task() {
    let s = idle_with_task();
    let s = wft_started(&s, 3_100).state;
    let s = admit(&s, "u1", 3_150).state;
    let t = complete_with(&s, vec![complete()], vec![], 3_200).expect("handled");
    assert_eq!(
        types(&t),
        [
            EventType::WorkflowTaskFailed,
            EventType::WorkflowTaskScheduled
        ]
    );
    match t.events[0].attributes.as_ref() {
        Some(loams_temporal_history::Attributes::WorkflowTaskFailedEventAttributes(a)) => {
            assert_eq!(a.cause, WorkflowTaskFailedCause::UnhandledCommand as i32);
        }
        other => panic!("failed event, got {other:?}"),
    }
}

#[test]
fn a_failed_task_sends_its_updates_again() {
    let s = admit(&idle(), "u1", 3_000).state;
    let s = wft_started(&s, 3_100).state;
    let w = s.workflow_task.clone().expect("wft");
    let failed = apply(
        &s,
        Input::WorkflowTaskFailed {
            scheduled_event_id: w.scheduled_event_id,
            attempt: w.attempt,
            cause: WorkflowTaskFailedCause::WorkflowWorkerUnhandledFailure as i32,
            failure: None,
            identity: "worker".into(),
        },
        3_200,
    )
    .expect("failed");
    assert_eq!(status(&failed.state, "u1"), Some(UpdateStatus::Admitted));
    let again = wft_started(&failed.state, 3_300);
    let Reply::WorkflowTaskStarted {
        scheduled_event_id,
        started_event_id,
        ..
    } = again.reply
    else {
        panic!("started");
    };
    assert_eq!(
        messages_for_task(&again.state, scheduled_event_id, started_event_id).len(),
        1
    );
}

#[test]
fn closing_aborts_what_is_in_flight() {
    // u1 accepted, then the workflow completes; u2 admitted, then the run
    // is terminated.
    let s = admit(&idle(), "u1", 3_000).state;
    let s = wft_started(&s, 3_100).state;
    let s = complete_with(&s, vec![], vec![acceptance("u1", 0)], 3_200)
        .expect("accepted")
        .state;
    let s = admit(&s, "u2", 3_300).state;
    let t = apply(
        &s,
        Input::Terminate {
            reason: "stop".into(),
            details: None,
            identity: "op".into(),
        },
        3_400,
    )
    .expect("terminated");
    match status(&t.state, "u1") {
        Some(UpdateStatus::Completed {
            outcome:
                Some(Outcome {
                    value: Some(outcome::Value::Failure(f)),
                }),
            ..
        }) => assert_eq!(f.message, loams_temporal_history::UPDATE_CLOSED_MESSAGE),
        other => panic!("failed outcome, got {other:?}"),
    }
    assert_eq!(status(&t.state, "u2"), Some(UpdateStatus::Aborted));
    // A new update on the closed run is refused as Temporal refuses it.
    let e = apply(
        &t.state,
        Input::UpdateAdmitted {
            request: Box::new(request("u3")),
        },
        3_500,
    )
    .expect_err("closed");
    assert_eq!(e, Rejection::NotFound(COMPLETED.into()));
}

#[test]
fn the_in_flight_limit_holds() {
    let mut s = start().state;
    for i in 0..loams_temporal_statemachine::update::MAX_IN_FLIGHT_UPDATES {
        s = admit(&s, &format!("u{i}"), 1_500).state;
    }
    let e = apply(
        &s,
        Input::UpdateAdmitted {
            request: Box::new(request("one-too-many")),
        },
        1_600,
    )
    .expect_err("limit");
    assert!(matches!(e, Rejection::ResourceExhausted(_)));
}

#[test]
fn the_history_folds_back_to_the_accepted_and_completed_updates() {
    let s0 = start();
    let mut events = s0.events.clone();
    let t = wft_started(&s0.state, 2_000);
    events.extend(t.events.clone());
    let t = complete_with(&t.state, vec![], vec![], 2_100).expect("first");
    events.extend(t.events.clone());
    let t = admit(&t.state, "u1", 3_000);
    events.extend(t.events.clone());
    let t = wft_started(&t.state, 3_100);
    events.extend(t.events.clone());
    let t = complete_with(
        &t.state,
        vec![],
        vec![acceptance("u1", 0), response("u1", b"ok")],
        3_200,
    )
    .expect("done");
    events.extend(t.events.clone());
    let folded = fold("ns", "run", &events).expect("folds");
    assert_eq!(folded.next_event_id, t.state.next_event_id);
    let u = loams_temporal_history::update(&folded, "u1").expect("u1");
    assert_eq!(u.state(), UpdateState::Completed);
    let live = loams_temporal_history::update(&t.state, "u1").expect("u1");
    assert_eq!(u.accepted_event_id, live.accepted_event_id);
    assert_eq!(u.completed_event_id, live.completed_event_id);
}
