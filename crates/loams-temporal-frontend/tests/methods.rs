//! Method-level tests on the in-process gateway.

mod common;

use std::time::Duration;

use loams_temporal_proto::temporal::api::common::v1::{
    Payload, Payloads, WorkflowExecution, WorkflowType,
};
use loams_temporal_proto::temporal::api::enums::v1::{QueryResultType, WorkflowExecutionStatus};
use loams_temporal_proto::temporal::api::query::v1::WorkflowQuery;
use loams_temporal_proto::temporal::api::taskqueue::v1::TaskQueue;
use loams_temporal_proto::temporal::api::workflowservice::v1::*;

fn start_req(id: &str, request_id: &str) -> StartWorkflowExecutionRequest {
    StartWorkflowExecutionRequest {
        namespace: "default".into(),
        workflow_id: id.into(),
        workflow_type: Some(WorkflowType { name: "t".into() }),
        task_queue: Some(TaskQueue {
            name: "q".into(),
            ..Default::default()
        }),
        request_id: request_id.into(),
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_is_idempotent_by_request_id_and_refuses_a_second_run() {
    let gw = common::start().await;
    let mut c = gw.client.clone();
    let a = c
        .start_workflow_execution(start_req("w1", "r1"))
        .await
        .expect("start")
        .into_inner();
    assert!(a.started);
    let b = c
        .start_workflow_execution(start_req("w1", "r1"))
        .await
        .expect("retry")
        .into_inner();
    assert_eq!(a.run_id, b.run_id);
    assert!(!b.started);
    let e = c
        .start_workflow_execution(start_req("w1", "r2"))
        .await
        .expect_err("running");
    assert_eq!(e.code(), tonic::Code::AlreadyExists);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_namespace_is_not_found() {
    let gw = common::start().await;
    let mut req = start_req("w1", "r1");
    req.namespace = "nope".into();
    let e = gw
        .client
        .clone()
        .start_workflow_execution(req)
        .await
        .expect_err("no namespace");
    assert_eq!(e.code(), tonic::Code::NotFound);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_poll_after_its_deadline() {
    let gw = common::start().await;
    let started = std::time::Instant::now();
    let r = gw
        .client
        .clone()
        .poll_workflow_task_queue(PollWorkflowTaskQueueRequest {
            namespace: "default".into(),
            task_queue: Some(TaskQueue {
                name: "empty".into(),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .expect("poll")
        .into_inner();
    assert!(r.task_token.is_empty());
    assert!(started.elapsed() >= Duration::from_secs(2));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tampered_token_is_refused() {
    let gw = common::start().await;
    let mut c = gw.client.clone();
    c.start_workflow_execution(start_req("w1", "r1"))
        .await
        .expect("start");
    let task = c
        .poll_workflow_task_queue(PollWorkflowTaskQueueRequest {
            namespace: "default".into(),
            task_queue: Some(TaskQueue {
                name: "q".into(),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .expect("poll")
        .into_inner();
    let mut token = task.task_token.to_vec();
    let last = token.len() - 1;
    token[last] ^= 1;
    let e = c
        .respond_workflow_task_completed(RespondWorkflowTaskCompletedRequest {
            task_token: token.into(),
            namespace: "default".into(),
            ..Default::default()
        })
        .await
        .expect_err("tampered");
    assert_eq!(e.code(), tonic::Code::InvalidArgument);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn query_reaches_a_polling_worker_and_describe_reports_the_run() {
    let gw = common::start().await;
    let mut c = gw.client.clone();
    let run = c
        .start_workflow_execution(start_req("wq", "r1"))
        .await
        .expect("start")
        .into_inner();
    // The first workflow task: complete it with no commands, leaving the run open.
    let poll = PollWorkflowTaskQueueRequest {
        namespace: "default".into(),
        task_queue: Some(TaskQueue {
            name: "q".into(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let t = c
        .poll_workflow_task_queue(poll.clone())
        .await
        .expect("poll")
        .into_inner();
    c.respond_workflow_task_completed(RespondWorkflowTaskCompletedRequest {
        task_token: t.task_token,
        namespace: "default".into(),
        ..Default::default()
    })
    .await
    .expect("complete");
    let mut q = c.clone();
    let execution = Some(WorkflowExecution {
        workflow_id: "wq".into(),
        run_id: run.run_id.clone(),
    });
    let ex = execution.clone();
    let asked = tokio::spawn(async move {
        q.query_workflow(QueryWorkflowRequest {
            namespace: "default".into(),
            execution: ex,
            query: Some(WorkflowQuery {
                query_type: "state".into(),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
    });
    let task = c
        .poll_workflow_task_queue(poll)
        .await
        .expect("poll")
        .into_inner();
    assert_eq!(
        task.query.as_ref().map(|q| q.query_type.as_str()),
        Some("state")
    );
    assert!(task.history.as_ref().is_some_and(|h| !h.events.is_empty()));
    let answer = Payloads {
        payloads: vec![Payload {
            data: b"\"ok\"".to_vec().into(),
            ..Default::default()
        }],
    };
    c.respond_query_task_completed(RespondQueryTaskCompletedRequest {
        task_token: task.task_token,
        completed_type: QueryResultType::Answered as i32,
        query_result: Some(answer.clone()),
        namespace: "default".into(),
        ..Default::default()
    })
    .await
    .expect("answer");
    let got = asked.await.expect("join").expect("query").into_inner();
    assert_eq!(got.query_result, Some(answer));

    let d = c
        .describe_workflow_execution(DescribeWorkflowExecutionRequest {
            namespace: "default".into(),
            execution,
        })
        .await
        .expect("describe")
        .into_inner();
    let info = d.workflow_execution_info.expect("info");
    assert_eq!(info.status, WorkflowExecutionStatus::Running as i32);
    assert_eq!(info.history_length, 4);
}
