#![allow(deprecated)]
//! DX1 Task 4's golden tests: for every DX1 scenario of the corpus, the
//! inputs a real Temporal server received (read back from its history) are
//! driven through `apply`, and the history the state machine writes equals
//! the real one after normalisation (`loams-temporal-testkit`).

use loams_temporal_history::time::ms;
use loams_temporal_history::{Attributes, History, HistoryEvent, MutableState};
use loams_temporal_proto::temporal::api::command::v1::{
    CancelTimerCommandAttributes, CancelWorkflowExecutionCommandAttributes, Command,
    CompleteWorkflowExecutionCommandAttributes, ContinueAsNewWorkflowExecutionCommandAttributes,
    FailWorkflowExecutionCommandAttributes, RecordMarkerCommandAttributes,
    RequestCancelActivityTaskCommandAttributes, ScheduleActivityTaskCommandAttributes,
    StartTimerCommandAttributes, command,
};
use loams_temporal_proto::temporal::api::workflowservice::v1::StartWorkflowExecutionRequest;
use loams_temporal_statemachine::{Input, Reply, apply};
use loams_temporal_testkit::{Corpus, Options, compare, normalized_history};

const DX1: &[&str] = &[
    "hello",
    "two_activities_sequential",
    "activity_parallel_fanout_10",
    "signal_then_complete",
    "query_while_running",
    "cancel_requested",
    "terminated",
    "workflow_failed_application_error",
    "activity_heartbeat_details",
    // TMP-api (D2080).
    "update_accepted_completed",
    "signal_with_start",
    "continue_as_new_3",
];

fn nanos(e: &HistoryEvent) -> i128 {
    e.event_time.as_ref().map_or(0, |t| {
        i128::from(t.seconds) * 1_000_000_000 + i128::from(t.nanos)
    })
}

/// The command a history event records, if it is one.
fn command_of(a: &Attributes) -> Option<command::Attributes> {
    use command::Attributes as C;
    Some(match a {
        Attributes::ActivityTaskScheduledEventAttributes(x) => {
            C::ScheduleActivityTaskCommandAttributes(ScheduleActivityTaskCommandAttributes {
                activity_id: x.activity_id.clone(),
                activity_type: x.activity_type.clone(),
                task_queue: x.task_queue.clone(),
                header: x.header.clone(),
                input: x.input.clone(),
                schedule_to_close_timeout: x.schedule_to_close_timeout,
                schedule_to_start_timeout: x.schedule_to_start_timeout,
                start_to_close_timeout: x.start_to_close_timeout,
                heartbeat_timeout: x.heartbeat_timeout,
                retry_policy: x.retry_policy.clone(),
                #[allow(deprecated)]
                use_workflow_build_id: x.use_workflow_build_id,
                priority: x.priority.clone(),
                ..Default::default()
            })
        }
        Attributes::TimerStartedEventAttributes(x) => {
            C::StartTimerCommandAttributes(StartTimerCommandAttributes {
                timer_id: x.timer_id.clone(),
                start_to_fire_timeout: x.start_to_fire_timeout,
            })
        }
        Attributes::TimerCanceledEventAttributes(x) => {
            C::CancelTimerCommandAttributes(CancelTimerCommandAttributes {
                timer_id: x.timer_id.clone(),
            })
        }
        Attributes::ActivityTaskCancelRequestedEventAttributes(x) => {
            C::RequestCancelActivityTaskCommandAttributes(
                RequestCancelActivityTaskCommandAttributes {
                    scheduled_event_id: x.scheduled_event_id,
                },
            )
        }
        Attributes::MarkerRecordedEventAttributes(x) => {
            C::RecordMarkerCommandAttributes(RecordMarkerCommandAttributes {
                marker_name: x.marker_name.clone(),
                details: x.details.clone(),
                header: x.header.clone(),
                failure: x.failure.clone(),
            })
        }
        Attributes::WorkflowExecutionCompletedEventAttributes(x) => {
            C::CompleteWorkflowExecutionCommandAttributes(
                CompleteWorkflowExecutionCommandAttributes {
                    result: x.result.clone(),
                },
            )
        }
        Attributes::WorkflowExecutionFailedEventAttributes(x) => {
            C::FailWorkflowExecutionCommandAttributes(FailWorkflowExecutionCommandAttributes {
                failure: x.failure.clone(),
            })
        }
        Attributes::WorkflowExecutionCanceledEventAttributes(x) => {
            C::CancelWorkflowExecutionCommandAttributes(CancelWorkflowExecutionCommandAttributes {
                details: x.details.clone(),
            })
        }
        Attributes::WorkflowExecutionContinuedAsNewEventAttributes(x) => {
            C::ContinueAsNewWorkflowExecutionCommandAttributes(
                ContinueAsNewWorkflowExecutionCommandAttributes {
                    workflow_type: x.workflow_type.clone(),
                    task_queue: x.task_queue.clone(),
                    input: x.input.clone(),
                    workflow_run_timeout: x.workflow_run_timeout,
                    workflow_task_timeout: x.workflow_task_timeout,
                    backoff_start_interval: x.backoff_start_interval,
                    initiator: x.initiator,
                    failure: x.failure.clone(),
                    last_completion_result: x.last_completion_result.clone(),
                    header: x.header.clone(),
                    memo: x.memo.clone(),
                    search_attributes: x.search_attributes.clone(),
                    #[allow(deprecated)]
                    inherit_build_id: x.inherit_build_id,
                    ..Default::default()
                },
            )
        }
        _ => return None,
    })
}

fn completed_by(a: &Attributes) -> i64 {
    match a {
        Attributes::ActivityTaskScheduledEventAttributes(x) => x.workflow_task_completed_event_id,
        Attributes::TimerStartedEventAttributes(x) => x.workflow_task_completed_event_id,
        Attributes::TimerCanceledEventAttributes(x) => x.workflow_task_completed_event_id,
        Attributes::ActivityTaskCancelRequestedEventAttributes(x) => {
            x.workflow_task_completed_event_id
        }
        Attributes::MarkerRecordedEventAttributes(x) => x.workflow_task_completed_event_id,
        Attributes::WorkflowExecutionCompletedEventAttributes(x) => {
            x.workflow_task_completed_event_id
        }
        Attributes::WorkflowExecutionFailedEventAttributes(x) => x.workflow_task_completed_event_id,
        Attributes::WorkflowExecutionContinuedAsNewEventAttributes(x) => {
            x.workflow_task_completed_event_id
        }
        Attributes::WorkflowExecutionCanceledEventAttributes(x) => {
            x.workflow_task_completed_event_id
        }
        _ => 0,
    }
}

fn any<M: prost::Message>(type_url: &str, m: &M) -> Option<prost_types::Any> {
    Some(prost_types::Any {
        type_url: type_url.into(),
        value: m.encode_to_vec(),
    })
}

/// The update protocol messages each workflow task completion carried, by
/// its completed event: an update's accepted and completed events follow
/// the completion that answered it.
fn update_messages(
    events: &[HistoryEvent],
) -> std::collections::BTreeMap<i64, Vec<loams_temporal_proto::temporal::api::protocol::v1::Message>>
{
    use loams_temporal_proto::temporal::api::protocol::v1::Message;
    use loams_temporal_proto::temporal::api::update::v1::{Acceptance, Response};
    use loams_temporal_statemachine::update::{ACCEPTANCE_TYPE, RESPONSE_TYPE};
    let mut out: std::collections::BTreeMap<i64, Vec<Message>> = Default::default();
    let mut last_completed = 0;
    for e in events {
        match e.attributes.as_ref() {
            Some(Attributes::WorkflowTaskCompletedEventAttributes(_)) => {
                last_completed = e.event_id
            }
            Some(Attributes::WorkflowExecutionUpdateAcceptedEventAttributes(x)) => {
                out.entry(last_completed).or_default().push(Message {
                    id: format!("{}/accept", x.protocol_instance_id),
                    protocol_instance_id: x.protocol_instance_id.clone(),
                    sequencing_id: None,
                    body: any(
                        ACCEPTANCE_TYPE,
                        &Acceptance {
                            accepted_request_message_id: x.accepted_request_message_id.clone(),
                            accepted_request_sequencing_event_id: x
                                .accepted_request_sequencing_event_id,
                            accepted_request: x.accepted_request.clone(),
                        },
                    ),
                });
            }
            Some(Attributes::WorkflowExecutionUpdateCompletedEventAttributes(x)) => {
                let id = x
                    .meta
                    .as_ref()
                    .map(|m| m.update_id.clone())
                    .unwrap_or_default();
                out.entry(last_completed).or_default().push(Message {
                    id: format!("{id}/complete"),
                    protocol_instance_id: id,
                    sequencing_id: None,
                    body: any(
                        RESPONSE_TYPE,
                        &Response {
                            meta: x.meta.clone(),
                            outcome: x.outcome.clone(),
                        },
                    ),
                });
            }
            _ => {}
        }
    }
    out
}

/// The inputs a server received, in the order they happened.
pub fn script(events: &[HistoryEvent]) -> Vec<(i64, Input)> {
    let mut out: Vec<(i128, i64, Input)> = Vec::new();
    let mut wft_attempt = std::collections::BTreeMap::new();
    let mut act_attempt = std::collections::BTreeMap::new();
    let mut messages = update_messages(events);
    // An update was admitted just before the workflow task it went out with
    // was scheduled (its acceptance's sequencing event).
    for e in events {
        if let Some(Attributes::WorkflowExecutionUpdateAcceptedEventAttributes(x)) =
            e.attributes.as_ref()
            && let Some(at) = events
                .iter()
                .find(|s| s.event_id == x.accepted_request_sequencing_event_id)
        {
            out.push((
                nanos(at),
                at.event_id,
                Input::UpdateAdmitted {
                    request: Box::new(x.accepted_request.clone().unwrap_or_default()),
                },
            ));
        }
    }
    for e in events {
        let t = nanos(e);
        let a = e.attributes.as_ref().expect("attributes");
        let input = match a {
            Attributes::WorkflowExecutionStartedEventAttributes(x) => Input::Start {
                namespace_id: "ns".into(),
                run_id: x.original_execution_run_id.clone(),
                request: Box::new(StartWorkflowExecutionRequest {
                    namespace: "default".into(),
                    workflow_id: x.workflow_id.clone(),
                    workflow_type: x.workflow_type.clone(),
                    task_queue: x.task_queue.clone(),
                    input: x.input.clone(),
                    workflow_execution_timeout: x.workflow_execution_timeout,
                    workflow_run_timeout: x.workflow_run_timeout,
                    workflow_task_timeout: x.workflow_task_timeout,
                    identity: x.identity.clone(),
                    request_id: "start".into(),
                    retry_policy: x.retry_policy.clone(),
                    memo: x.memo.clone(),
                    search_attributes: x.search_attributes.clone(),
                    header: x.header.clone(),
                    priority: x.priority.clone(),
                    ..Default::default()
                }),
            },
            Attributes::WorkflowTaskScheduledEventAttributes(x) => {
                wft_attempt.insert(e.event_id, x.attempt);
                continue;
            }
            Attributes::WorkflowTaskStartedEventAttributes(x) => Input::WorkflowTaskStarted {
                scheduled_event_id: x.scheduled_event_id,
                identity: x.identity.clone(),
                request_id: x.request_id.clone(),
                worker_version: x.worker_version.clone(),
            },
            Attributes::WorkflowTaskCompletedEventAttributes(x) => Input::WorkflowTaskCompleted {
                scheduled_event_id: x.scheduled_event_id,
                attempt: wft_attempt[&x.scheduled_event_id],
                identity: x.identity.clone(),
                commands: events
                    .iter()
                    .filter_map(|c| {
                        let ca = c.attributes.as_ref()?;
                        (completed_by(ca) == e.event_id).then(|| Command {
                            attributes: command_of(ca),
                            user_metadata: c.user_metadata.clone(),
                            ..Default::default()
                        })
                    })
                    .collect(),
                sdk_metadata: x.sdk_metadata.clone(),
                metering_metadata: x.metering_metadata,
                worker_version: x.worker_version.clone(),
                force_create_new_workflow_task: false,
                messages: messages.remove(&e.event_id).unwrap_or_default(),
                new_run_id: events
                    .iter()
                    .find_map(|c| match c.attributes.as_ref() {
                        Some(Attributes::WorkflowExecutionContinuedAsNewEventAttributes(x))
                            if x.workflow_task_completed_event_id == e.event_id =>
                        {
                            Some(x.new_execution_run_id.clone())
                        }
                        _ => None,
                    })
                    .unwrap_or_default(),
            },
            Attributes::WorkflowTaskFailedEventAttributes(x) => Input::WorkflowTaskFailed {
                scheduled_event_id: x.scheduled_event_id,
                attempt: wft_attempt[&x.scheduled_event_id],
                cause: x.cause,
                failure: x.failure.clone(),
                identity: x.identity.clone(),
            },
            Attributes::WorkflowTaskTimedOutEventAttributes(x) => Input::WorkflowTaskTimedOut {
                scheduled_event_id: x.scheduled_event_id,
                attempt: wft_attempt[&x.scheduled_event_id],
            },
            Attributes::ActivityTaskStartedEventAttributes(x) => {
                act_attempt.insert(x.scheduled_event_id, x.attempt);
                Input::ActivityStarted {
                    scheduled_event_id: x.scheduled_event_id,
                    attempt: x.attempt,
                    identity: x.identity.clone(),
                    request_id: x.request_id.clone(),
                    worker_version: x.worker_version.clone(),
                }
            }
            Attributes::ActivityTaskCompletedEventAttributes(x) => Input::ActivityCompleted {
                scheduled_event_id: x.scheduled_event_id,
                attempt: act_attempt.get(&x.scheduled_event_id).copied().unwrap_or(1),
                result: x.result.clone(),
                identity: x.identity.clone(),
            },
            Attributes::ActivityTaskFailedEventAttributes(x) => Input::ActivityFailed {
                scheduled_event_id: x.scheduled_event_id,
                attempt: act_attempt.get(&x.scheduled_event_id).copied().unwrap_or(1),
                failure: x.failure.clone(),
                identity: x.identity.clone(),
                last_heartbeat_details: None,
            },
            Attributes::ActivityTaskCanceledEventAttributes(x) if x.started_event_id != 0 => {
                Input::ActivityCanceled {
                    scheduled_event_id: x.scheduled_event_id,
                    attempt: act_attempt.get(&x.scheduled_event_id).copied().unwrap_or(1),
                    details: x.details.clone(),
                    identity: x.identity.clone(),
                }
            }
            Attributes::WorkflowExecutionSignaledEventAttributes(x) => Input::Signal {
                name: x.signal_name.clone(),
                input: x.input.clone(),
                identity: x.identity.clone(),
                header: x.header.clone(),
                request_id: x.request_id.clone(),
            },
            Attributes::WorkflowExecutionCancelRequestedEventAttributes(x) => {
                Input::CancelRequested {
                    reason: x.cause.clone(),
                    identity: x.identity.clone(),
                }
            }
            Attributes::WorkflowExecutionTerminatedEventAttributes(x) => Input::Terminate {
                reason: x.reason.clone(),
                details: x.details.clone(),
                identity: x.identity.clone(),
            },
            Attributes::TimerFiredEventAttributes(x) => Input::TimerFired {
                started_event_id: x.started_event_id,
            },
            _ => continue,
        };
        out.push((t, e.event_id, input));
    }
    // Signal-with-start: the signal written as event 2, between the
    // started event and the first workflow task, came with the start.
    if let Some(si) = out
        .iter()
        .position(|(_, id, i)| *id == 2 && matches!(i, Input::Signal { .. }))
        && let Some(st) = out.iter().position(|(_, id, _)| *id == 1)
    {
        let (_, _, sig) = out.remove(si);
        let Input::Signal {
            name,
            input,
            identity,
            header,
            request_id,
        } = sig
        else {
            unreachable!("matched above")
        };
        if let (
            t,
            id,
            Input::Start {
                namespace_id,
                run_id,
                request,
            },
        ) = out.remove(st)
        {
            out.insert(
                st,
                (
                    t,
                    id,
                    Input::SignalWithStart {
                        namespace_id,
                        run_id,
                        request,
                        signal: loams_temporal_statemachine::StartSignal {
                            name,
                            input,
                            identity,
                            header,
                            request_id,
                        },
                    },
                ),
            );
        }
    }
    out.sort_by_key(|(t, id, _)| (*t, *id));
    out.into_iter()
        .map(|(t, _, i)| ((t / 1_000_000) as i64, i))
        .collect()
}

/// Drives `script` through `apply`; returns the history written.
pub fn run(script: Vec<(i64, Input)>) -> (MutableState, History) {
    let mut state = MutableState::default();
    let mut events = Vec::new();
    for (now, input) in script {
        let t =
            apply(&state, input.clone(), now).unwrap_or_else(|r| panic!("{input:?} at {now}: {r}"));
        assert_ne!(t.reply, Reply::Stale, "{input:?}");
        state = t.state;
        events.extend(t.events);
    }
    (state, History { events })
}

#[test]
fn golden_dx1() {
    let Some(corpus) = Corpus::for_tests() else {
        return;
    };
    let mut failures = Vec::new();
    for lang in ["go", "ts"] {
        for scenario in DX1 {
            let h = corpus.runs(lang, scenario)[0];
            let (_, written) = run(script(&h.history.events));
            let o = Options::gateway(scenario);
            let diff = compare(
                &normalized_history(&h.history, &o),
                &normalized_history(&written, &o),
            );
            if !diff.is_empty() {
                failures.push(format!("{lang}/{scenario}:\n  {}", diff.join("\n  ")));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    let _ = ms;
}
