//! History and run listings come from the store's paged reads: walking the
//! page tokens returns every event once, in order, and the older listing
//! methods page by keyset.

mod common;

use loams_temporal_proto::temporal::api::common::v1::{WorkflowExecution, WorkflowType};
use loams_temporal_proto::temporal::api::taskqueue::v1::TaskQueue;
use loams_temporal_proto::temporal::api::workflowservice::v1::{
    GetWorkflowExecutionHistoryRequest, ListOpenWorkflowExecutionsRequest,
    SignalWorkflowExecutionRequest, StartWorkflowExecutionRequest,
};

fn start_req(id: &str) -> StartWorkflowExecutionRequest {
    StartWorkflowExecutionRequest {
        namespace: "default".into(),
        workflow_id: id.into(),
        workflow_type: Some(WorkflowType { name: "t".into() }),
        task_queue: Some(TaskQueue {
            name: "q".into(),
            ..Default::default()
        }),
        request_id: format!("req-{id}"),
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn history_pages_walk_every_event_once() {
    let gw = common::start().await;
    let mut c = gw.client.clone();
    let run = c
        .start_workflow_execution(start_req("h1"))
        .await
        .expect("start")
        .into_inner()
        .run_id;
    let execution = Some(WorkflowExecution {
        workflow_id: "h1".into(),
        run_id: run,
    });
    for i in 0..5 {
        c.signal_workflow_execution(SignalWorkflowExecutionRequest {
            namespace: "default".into(),
            workflow_execution: execution.clone(),
            signal_name: "s".into(),
            request_id: format!("sig-{i}"),
            ..Default::default()
        })
        .await
        .expect("signal");
    }
    let mut ids = Vec::new();
    let mut token = Vec::new();
    let mut pages = 0;
    loop {
        let r = c
            .get_workflow_execution_history(GetWorkflowExecutionHistoryRequest {
                namespace: "default".into(),
                execution: execution.clone(),
                maximum_page_size: 2,
                next_page_token: token.clone().into(),
                ..Default::default()
            })
            .await
            .expect("page")
            .into_inner();
        let events = r.history.expect("history").events;
        assert!(events.len() <= 2);
        ids.extend(events.iter().map(|e| e.event_id));
        pages += 1;
        if r.next_page_token.is_empty() {
            break;
        }
        token = r.next_page_token.to_vec();
        assert!(pages < 100, "the tokens never end");
    }
    let want: Vec<i64> = (1..=ids.len() as i64).collect();
    assert_eq!(ids, want, "contiguous from 1, each once");
    assert!(ids.len() >= 7, "started, a task and five signals at least");
    assert!(pages >= 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_runs_list_in_keyset_pages() {
    let gw = common::start().await;
    let mut c = gw.client.clone();
    for id in ["l1", "l2", "l3"] {
        c.start_workflow_execution(start_req(id))
            .await
            .expect("start");
    }
    let mut seen = Vec::new();
    let mut token = Vec::new();
    for _ in 0..10 {
        let r = c
            .list_open_workflow_executions(ListOpenWorkflowExecutionsRequest {
                namespace: "default".into(),
                maximum_page_size: 2,
                next_page_token: token.clone().into(),
                ..Default::default()
            })
            .await
            .expect("list")
            .into_inner();
        assert!(r.executions.len() <= 2);
        seen.extend(
            r.executions
                .into_iter()
                .map(|e| e.execution.expect("execution").workflow_id),
        );
        if r.next_page_token.is_empty() {
            break;
        }
        token = r.next_page_token.to_vec();
    }
    seen.sort();
    assert_eq!(seen, ["l1", "l2", "l3"]);
}
