//! Method-level tests on the in-process gateway.

mod common;

use std::time::Duration;

use loams_temporal_proto::temporal::api::common::v1::{
    Payload, Payloads, WorkflowExecution, WorkflowType,
};
use loams_temporal_proto::temporal::api::enums::v1::{
    EventType, HistoryEventFilterType, QueryResultType, WorkflowExecutionStatus,
};
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

/// DX1F.R1: a close-event long poll that reaches its deadline before the run
/// closes answers an empty page *with* a token, so the SDK polls again
/// instead of failing with "could not get last history event".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_event_long_poll_keeps_a_page_token_past_its_deadline() {
    let gw = common::start_with(|mut c| {
        c.history_wait = Duration::from_millis(300);
        c
    })
    .await;
    let mut c = gw.client.clone();
    let run = c
        .start_workflow_execution(start_req("wlp", "r1"))
        .await
        .expect("start")
        .into_inner();
    let execution = Some(WorkflowExecution {
        workflow_id: "wlp".into(),
        run_id: run.run_id.clone(),
    });
    let wait = |token: Vec<u8>| GetWorkflowExecutionHistoryRequest {
        namespace: "default".into(),
        execution: execution.clone(),
        wait_new_event: true,
        history_event_filter_type: HistoryEventFilterType::CloseEvent as i32,
        next_page_token: token.into(),
        ..Default::default()
    };
    // The SDK's first call carries no token; the run stays open past the
    // deadline, twice.
    let mut token = Vec::new();
    for _ in 0..2 {
        let r = c
            .get_workflow_execution_history(wait(token.clone()))
            .await
            .expect("long poll")
            .into_inner();
        assert!(r.history.is_some_and(|h| h.events.is_empty()));
        assert!(
            !r.next_page_token.is_empty(),
            "a timed-out long poll must keep a page token"
        );
        token = r.next_page_token.to_vec();
    }
    c.terminate_workflow_execution(TerminateWorkflowExecutionRequest {
        namespace: "default".into(),
        workflow_execution: execution.clone(),
        reason: "done".into(),
        ..Default::default()
    })
    .await
    .expect("terminate");
    let r = c
        .get_workflow_execution_history(wait(token))
        .await
        .expect("close event")
        .into_inner();
    let events = r.history.expect("history").events;
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].event_type,
        EventType::WorkflowExecutionTerminated as i32
    );
    assert!(r.next_page_token.is_empty(), "the history is complete");
}

/// X1.R7 / DX1F.R2: a payload over 2 MiB is refused with Temporal's
/// `InvalidArgument` "Blob data size exceeds limit." on start and signal.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_payload_over_2_mib_is_invalid_argument() {
    let gw = common::start().await;
    let mut c = gw.client.clone();
    let big = Some(Payloads {
        payloads: vec![Payload {
            data: vec![7; 2 * 1024 * 1024].into(),
            ..Default::default()
        }],
    });
    let mut req = start_req("wbig", "r1");
    req.input = big.clone();
    let e = c
        .start_workflow_execution(req)
        .await
        .expect_err("oversized input");
    assert_eq!(e.code(), tonic::Code::InvalidArgument);
    assert_eq!(e.message(), "Blob data size exceeds limit.");

    let run = c
        .start_workflow_execution(start_req("wbig", "r2"))
        .await
        .expect("start")
        .into_inner();
    let e = c
        .signal_workflow_execution(SignalWorkflowExecutionRequest {
            namespace: "default".into(),
            workflow_execution: Some(WorkflowExecution {
                workflow_id: "wbig".into(),
                run_id: run.run_id,
            }),
            signal_name: "s".into(),
            input: big,
            ..Default::default()
        })
        .await
        .expect_err("oversized signal");
    assert_eq!(e.code(), tonic::Code::InvalidArgument);
    assert_eq!(e.message(), "Blob data size exceeds limit.");
}

/// What the Go SDK's `run.Get` does with a close-event long poll: poll again
/// while the page is empty and has a token; an empty page *without* one is
/// its `panic: could not get last history event for workflow`.
async fn sdk_get_close_event(
    mut c: loams_temporal_proto::temporal::api::workflowservice::v1::workflow_service_client::WorkflowServiceClient<tonic::transport::Channel>,
    execution: WorkflowExecution,
) -> (EventType, usize) {
    let mut token = Vec::new();
    let mut polls = 0;
    loop {
        polls += 1;
        let r = c
            .get_workflow_execution_history(GetWorkflowExecutionHistoryRequest {
                namespace: "default".into(),
                execution: Some(execution.clone()),
                wait_new_event: true,
                history_event_filter_type: HistoryEventFilterType::CloseEvent as i32,
                skip_archival: true,
                next_page_token: token.into(),
                ..Default::default()
            })
            .await
            .expect("long poll")
            .into_inner();
        let events = r.history.map(|h| h.events).unwrap_or_default();
        if let Some(e) = events.first() {
            return (
                EventType::try_from(e.event_type).expect("event type"),
                polls,
            );
        }
        assert!(
            !r.next_page_token.is_empty(),
            "could not get last history event for workflow {} (poll {polls})",
            execution.workflow_id
        );
        token = r.next_page_token.to_vec();
    }
}

/// The resonate-bench burst shape (DX1F.R1): many runs started at once,
/// each waited on with the SDK's close-event loop while the runs close at
/// different times, most of them after one or more long polls have timed
/// out. Every waiter gets its close event; none sees an empty, tokenless page.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn burst_close_event_waiters_all_get_their_close_event() {
    let gw = common::start_with(|mut c| {
        c.history_wait = Duration::from_millis(150);
        c
    })
    .await;
    let mut c = gw.client.clone();
    const RUNS: usize = 40;
    let mut runs = Vec::with_capacity(RUNS);
    for i in 0..RUNS {
        let id = format!("burst-{i}");
        let run = c
            .start_workflow_execution(start_req(&id, &format!("r{i}")))
            .await
            .expect("start")
            .into_inner();
        runs.push(WorkflowExecution {
            workflow_id: id,
            run_id: run.run_id,
        });
    }
    let waiters: Vec<_> = runs
        .iter()
        .cloned()
        .map(|e| tokio::spawn(sdk_get_close_event(gw.client.clone(), e)))
        .collect();
    // Close them in waves: some at once, the rest across several deadlines.
    let closers: Vec<_> = runs
        .iter()
        .cloned()
        .enumerate()
        .map(|(i, e)| {
            let mut c = gw.client.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(40 * (i % 13) as u64)).await;
                c.terminate_workflow_execution(TerminateWorkflowExecutionRequest {
                    namespace: "default".into(),
                    workflow_execution: Some(e),
                    reason: "burst".into(),
                    ..Default::default()
                })
                .await
                .expect("terminate");
            })
        })
        .collect();
    for h in closers {
        h.await.expect("closer");
    }
    let mut timed_out_first = 0;
    for h in waiters {
        let (event, polls) = h.await.expect("waiter");
        assert_eq!(event, EventType::WorkflowExecutionTerminated);
        if polls > 1 {
            timed_out_first += 1;
        }
    }
    // The test only covers the bug if some waiters outlived a deadline.
    assert!(timed_out_first > 0, "no long poll reached its deadline");
}

/// The full-history long poll (no filter) on an open run that does not move
/// before the deadline keeps a token too, so a reader following the history
/// does not take the page as the end of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_history_long_poll_keeps_a_token_while_the_run_is_open() {
    let gw = common::start_with(|mut c| {
        c.history_wait = Duration::from_millis(200);
        c
    })
    .await;
    let mut c = gw.client.clone();
    let run = c
        .start_workflow_execution(start_req("wfull", "r1"))
        .await
        .expect("start")
        .into_inner();
    let execution = Some(WorkflowExecution {
        workflow_id: "wfull".into(),
        run_id: run.run_id,
    });
    let first = c
        .get_workflow_execution_history(GetWorkflowExecutionHistoryRequest {
            namespace: "default".into(),
            execution: execution.clone(),
            ..Default::default()
        })
        .await
        .expect("history")
        .into_inner();
    let seen = first.history.expect("history").events;
    assert!(!seen.is_empty());
    let next = seen.last().expect("event").event_id + 1;
    let r = c
        .get_workflow_execution_history(GetWorkflowExecutionHistoryRequest {
            namespace: "default".into(),
            execution,
            wait_new_event: true,
            next_page_token: next.to_be_bytes().to_vec().into(),
            ..Default::default()
        })
        .await
        .expect("long poll")
        .into_inner();
    assert!(r.history.is_some_and(|h| h.events.is_empty()));
    assert_eq!(r.next_page_token.to_vec(), next.to_be_bytes().to_vec());
}
