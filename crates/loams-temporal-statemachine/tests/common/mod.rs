//! Helpers shared by the table and property tests.
#![allow(dead_code)]

use loams_temporal_history::MutableState;
use loams_temporal_proto::temporal::api::command::v1::{
    Command, CompleteWorkflowExecutionCommandAttributes, ScheduleActivityTaskCommandAttributes,
    StartTimerCommandAttributes, command,
};
use loams_temporal_proto::temporal::api::common::v1::{ActivityType, RetryPolicy, WorkflowType};
use loams_temporal_proto::temporal::api::taskqueue::v1::TaskQueue;
use loams_temporal_proto::temporal::api::workflowservice::v1::StartWorkflowExecutionRequest;
use loams_temporal_statemachine::{Input, Transition, apply};

pub fn start() -> Transition {
    apply(
        &MutableState::default(),
        Input::Start {
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
        },
        1_000,
    )
    .expect("start")
}

pub fn wft_started(s: &MutableState, now: i64) -> Transition {
    let w = s.workflow_task.as_ref().expect("a workflow task");
    apply(
        s,
        Input::WorkflowTaskStarted {
            scheduled_event_id: w.scheduled_event_id,
            identity: "worker".into(),
            request_id: format!("poll-{now}"),
            worker_version: None,
        },
        now,
    )
    .expect("started")
}

pub fn wft_completed(
    s: &MutableState,
    commands: Vec<Command>,
    now: i64,
) -> Result<Transition, loams_temporal_statemachine::Rejection> {
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
            new_run_id: String::new(),
        },
        now,
    )
}

pub fn cmd(a: command::Attributes) -> Command {
    Command {
        attributes: Some(a),
        ..Default::default()
    }
}

pub fn schedule_activity(id: &str, max_attempts: i32) -> Command {
    cmd(command::Attributes::ScheduleActivityTaskCommandAttributes(
        ScheduleActivityTaskCommandAttributes {
            activity_id: id.into(),
            activity_type: Some(ActivityType { name: "a".into() }),
            start_to_close_timeout: Some(prost_types::Duration {
                seconds: 30,
                nanos: 0,
            }),
            retry_policy: Some(RetryPolicy {
                maximum_attempts: max_attempts,
                ..Default::default()
            }),
            ..Default::default()
        },
    ))
}

pub fn start_timer(id: &str, secs: i64) -> Command {
    cmd(command::Attributes::StartTimerCommandAttributes(
        StartTimerCommandAttributes {
            timer_id: id.into(),
            start_to_fire_timeout: Some(prost_types::Duration {
                seconds: secs,
                nanos: 0,
            }),
        },
    ))
}

pub fn complete() -> Command {
    cmd(
        command::Attributes::CompleteWorkflowExecutionCommandAttributes(
            CompleteWorkflowExecutionCommandAttributes { result: None },
        ),
    )
}
