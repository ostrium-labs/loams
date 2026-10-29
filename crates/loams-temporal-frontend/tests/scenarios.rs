//! `gateway_dx1_<scenario>_matches_corpus`: a scripted worker drives the
//! in-process gateway over gRPC with exactly what the corpus's real worker
//! sent (read back from the real history), and the gateway's history equals
//! the real one after normalisation.
#![allow(deprecated)]

mod common;

use std::collections::{BTreeMap, HashMap};

use loams_temporal_history::{Attributes, HistoryEvent};
use loams_temporal_proto::temporal::api::command::v1::{
    CancelTimerCommandAttributes, CancelWorkflowExecutionCommandAttributes, Command,
    CompleteWorkflowExecutionCommandAttributes, FailWorkflowExecutionCommandAttributes,
    RecordMarkerCommandAttributes, RequestCancelActivityTaskCommandAttributes,
    ScheduleActivityTaskCommandAttributes, StartTimerCommandAttributes, command,
};
use loams_temporal_proto::temporal::api::common::v1::{
    WorkerVersionCapabilities, WorkflowExecution,
};
use loams_temporal_proto::temporal::api::history::v1::History;
use loams_temporal_proto::temporal::api::taskqueue::v1::TaskQueue;
use loams_temporal_proto::temporal::api::workflowservice::v1::*;
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
];

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
        Attributes::WorkflowExecutionCanceledEventAttributes(x) => {
            x.workflow_task_completed_event_id
        }
        _ => 0,
    }
}

/// Replays `events`' worker and client calls against the gateway; returns
/// the gateway's history.
async fn drive(gw: &mut common::TestGateway, events: &[HistoryEvent]) -> History {
    let c = &mut gw.client;
    let mut order: Vec<(i128, i64)> = events
        .iter()
        .map(|e| {
            let t = e.event_time.map_or(0, |t| {
                i128::from(t.seconds) * 1_000_000_000 + i128::from(t.nanos)
            });
            (t, e.event_id)
        })
        .collect();
    order.sort();
    let by_id: BTreeMap<i64, &HistoryEvent> = events.iter().map(|e| (e.event_id, e)).collect();
    let activity_ids: HashMap<String, i64> = events
        .iter()
        .filter_map(|e| match &e.attributes {
            Some(Attributes::ActivityTaskScheduledEventAttributes(a)) => {
                Some((a.activity_id.clone(), e.event_id))
            }
            _ => None,
        })
        .collect();
    let mut tq = String::new();
    let mut workflow_id = String::new();
    let mut run_id = String::new();
    let mut wft_token = Vec::new();
    let mut act_tokens: HashMap<i64, Vec<u8>> = HashMap::new();
    let exec = |w: &str, r: &str| {
        Some(WorkflowExecution {
            workflow_id: w.into(),
            run_id: r.into(),
        })
    };
    for (_, id) in order {
        let e = by_id[&id];
        match e.attributes.as_ref().expect("attributes") {
            Attributes::WorkflowExecutionStartedEventAttributes(x) => {
                tq = x.task_queue.as_ref().expect("tq").name.clone();
                workflow_id = x.workflow_id.clone();
                run_id = c
                    .start_workflow_execution(StartWorkflowExecutionRequest {
                        namespace: "default".into(),
                        workflow_id: x.workflow_id.clone(),
                        workflow_type: x.workflow_type.clone(),
                        task_queue: x.task_queue.clone(),
                        input: x.input.clone(),
                        workflow_execution_timeout: x.workflow_execution_timeout,
                        workflow_run_timeout: x.workflow_run_timeout,
                        workflow_task_timeout: x.workflow_task_timeout,
                        identity: x.identity.clone(),
                        request_id: "start-1".into(),
                        retry_policy: x.retry_policy.clone(),
                        memo: x.memo.clone(),
                        search_attributes: x.search_attributes.clone(),
                        header: x.header.clone(),
                        priority: x.priority.clone(),
                        ..Default::default()
                    })
                    .await
                    .expect("start")
                    .into_inner()
                    .run_id;
            }
            Attributes::WorkflowTaskStartedEventAttributes(x) => {
                let resp = c
                    .poll_workflow_task_queue(PollWorkflowTaskQueueRequest {
                        namespace: "default".into(),
                        task_queue: Some(TaskQueue {
                            name: tq.clone(),
                            ..Default::default()
                        }),
                        identity: x.identity.clone(),
                        worker_version_capabilities: x.worker_version.as_ref().map(|v| {
                            WorkerVersionCapabilities {
                                build_id: v.build_id.clone(),
                                ..Default::default()
                            }
                        }),
                        ..Default::default()
                    })
                    .await
                    .expect("poll workflow task")
                    .into_inner();
                assert!(
                    !resp.task_token.is_empty(),
                    "a workflow task for event {id}"
                );
                assert_eq!(resp.started_event_id, id, "the started event's ID");
                wft_token = resp.task_token.to_vec();
            }
            Attributes::WorkflowTaskCompletedEventAttributes(x) => {
                let commands = events
                    .iter()
                    .filter_map(|ce| {
                        let a = ce.attributes.as_ref()?;
                        (completed_by(a) == id).then(|| Command {
                            attributes: command_of(a),
                            user_metadata: ce.user_metadata.clone(),
                            ..Default::default()
                        })
                    })
                    .collect();
                c.respond_workflow_task_completed(RespondWorkflowTaskCompletedRequest {
                    task_token: wft_token.clone().into(),
                    commands,
                    identity: x.identity.clone(),
                    namespace: "default".into(),
                    sdk_metadata: x.sdk_metadata.clone(),
                    metering_metadata: x.metering_metadata,
                    worker_version_stamp: x.worker_version.clone(),
                    ..Default::default()
                })
                .await
                .expect("complete workflow task");
            }
            Attributes::ActivityTaskStartedEventAttributes(x) => {
                while !act_tokens.contains_key(&x.scheduled_event_id) {
                    let resp = c
                        .poll_activity_task_queue(PollActivityTaskQueueRequest {
                            namespace: "default".into(),
                            task_queue: Some(TaskQueue {
                                name: tq.clone(),
                                ..Default::default()
                            }),
                            identity: x.identity.clone(),
                            worker_version_capabilities: x.worker_version.as_ref().map(|v| {
                                WorkerVersionCapabilities {
                                    build_id: v.build_id.clone(),
                                    ..Default::default()
                                }
                            }),
                            ..Default::default()
                        })
                        .await
                        .expect("poll activity")
                        .into_inner();
                    assert!(
                        !resp.task_token.is_empty(),
                        "an activity task for {}",
                        x.scheduled_event_id
                    );
                    act_tokens.insert(activity_ids[&resp.activity_id], resp.task_token.to_vec());
                }
            }
            Attributes::ActivityTaskCompletedEventAttributes(x) => {
                c.respond_activity_task_completed(RespondActivityTaskCompletedRequest {
                    task_token: act_tokens[&x.scheduled_event_id].clone().into(),
                    result: x.result.clone(),
                    identity: x.identity.clone(),
                    namespace: "default".into(),
                    ..Default::default()
                })
                .await
                .expect("complete activity");
            }
            Attributes::ActivityTaskFailedEventAttributes(x) => {
                c.respond_activity_task_failed(RespondActivityTaskFailedRequest {
                    task_token: act_tokens[&x.scheduled_event_id].clone().into(),
                    failure: x.failure.clone(),
                    identity: x.identity.clone(),
                    namespace: "default".into(),
                    ..Default::default()
                })
                .await
                .expect("fail activity");
            }
            Attributes::WorkflowExecutionSignaledEventAttributes(x) => {
                c.signal_workflow_execution(SignalWorkflowExecutionRequest {
                    namespace: "default".into(),
                    workflow_execution: exec(&workflow_id, &run_id),
                    signal_name: x.signal_name.clone(),
                    input: x.input.clone(),
                    identity: x.identity.clone(),
                    request_id: x.request_id.clone(),
                    header: x.header.clone(),
                    ..Default::default()
                })
                .await
                .expect("signal");
            }
            Attributes::WorkflowExecutionCancelRequestedEventAttributes(x) => {
                c.request_cancel_workflow_execution(RequestCancelWorkflowExecutionRequest {
                    namespace: "default".into(),
                    workflow_execution: exec(&workflow_id, &run_id),
                    identity: x.identity.clone(),
                    reason: x.cause.clone(),
                    ..Default::default()
                })
                .await
                .expect("cancel");
            }
            Attributes::WorkflowExecutionTerminatedEventAttributes(x) => {
                c.terminate_workflow_execution(TerminateWorkflowExecutionRequest {
                    namespace: "default".into(),
                    workflow_execution: exec(&workflow_id, &run_id),
                    reason: x.reason.clone(),
                    details: x.details.clone(),
                    identity: x.identity.clone(),
                    ..Default::default()
                })
                .await
                .expect("terminate");
            }
            _ => {}
        }
    }
    let mut out = History::default();
    let mut token = Vec::new();
    loop {
        let page = c
            .get_workflow_execution_history(GetWorkflowExecutionHistoryRequest {
                namespace: "default".into(),
                execution: exec(&workflow_id, &run_id),
                next_page_token: token.into(),
                ..Default::default()
            })
            .await
            .expect("history")
            .into_inner();
        out.events.extend(page.history.unwrap_or_default().events);
        if page.next_page_token.is_empty() {
            break;
        }
        token = page.next_page_token.to_vec();
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gateway_dx1_scenarios_match_corpus() {
    let Some(corpus) = Corpus::for_tests() else {
        return;
    };
    let mut gw = common::start().await;
    let mut failures = Vec::new();
    for lang in ["go", "ts"] {
        for scenario in DX1 {
            let h = corpus.runs(lang, scenario)[0];
            let written = drive(&mut gw, &h.history.events).await;
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
}
