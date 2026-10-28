//! D2230–D2231: a batch operation picks its targets with a visibility query
//! and terminates them; a signal batch run twice signals each target once.

mod common;

use std::time::Duration;

use loams_temporal_proto::temporal::api::batch::v1::{
    BatchOperationSignal, BatchOperationTermination,
};
use loams_temporal_proto::temporal::api::common::v1::{Execution, WorkflowExecution, WorkflowType};
use loams_temporal_proto::temporal::api::enums::v1::{
    BatchOperationState, EventType, ExecutionType, WorkflowExecutionStatus,
};
use loams_temporal_proto::temporal::api::taskqueue::v1::TaskQueue;
use loams_temporal_proto::temporal::api::workflowservice::v1::start_batch_operation_request::Operation;
use loams_temporal_proto::temporal::api::workflowservice::v1::workflow_service_client::WorkflowServiceClient;
use loams_temporal_proto::temporal::api::workflowservice::v1::{
    DescribeBatchOperationRequest, DescribeWorkflowExecutionRequest,
    GetWorkflowExecutionHistoryRequest, StartBatchOperationRequest, StartWorkflowExecutionRequest,
};

fn start_req(id: &str, ty: &str) -> StartWorkflowExecutionRequest {
    StartWorkflowExecutionRequest {
        namespace: "default".into(),
        workflow_id: id.into(),
        workflow_type: Some(WorkflowType { name: ty.into() }),
        task_queue: Some(TaskQueue {
            name: "q".into(),
            ..Default::default()
        }),
        request_id: format!("req-{id}"),
        ..Default::default()
    }
}

async fn wait_done(
    c: &mut WorkflowServiceClient<tonic::transport::Channel>,
    job: &str,
) -> (i64, i64) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let d = c
            .describe_batch_operation(DescribeBatchOperationRequest {
                namespace: "default".into(),
                job_id: job.into(),
            })
            .await
            .expect("describe")
            .into_inner();
        if d.state != BatchOperationState::Running as i32 {
            assert_eq!(d.state, BatchOperationState::Completed as i32, "{d:?}");
            return (d.total_operation_count, d.complete_operation_count);
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "batch still running"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_query_batch_terminates_what_it_matches() {
    let gw = common::start().await;
    let mut c = gw.client.clone();
    for (id, ty) in [("b1", "doomed"), ("b2", "doomed"), ("b3", "kept")] {
        c.start_workflow_execution(start_req(id, ty))
            .await
            .expect("start");
    }
    c.start_batch_operation(StartBatchOperationRequest {
        namespace: "default".into(),
        job_id: "job-q".into(),
        reason: "cleanup".into(),
        visibility_query: "WorkflowType = 'doomed'".into(),
        max_operations_per_second: 1000.0,
        operation: Some(Operation::TerminationOperation(
            BatchOperationTermination::default(),
        )),
        ..Default::default()
    })
    .await
    .expect("batch");
    assert_eq!(wait_done(&mut c, "job-q").await, (2, 2));
    for (id, status) in [
        ("b1", WorkflowExecutionStatus::Terminated),
        ("b2", WorkflowExecutionStatus::Terminated),
        ("b3", WorkflowExecutionStatus::Running),
    ] {
        let d = c
            .describe_workflow_execution(DescribeWorkflowExecutionRequest {
                namespace: "default".into(),
                execution: Some(WorkflowExecution {
                    workflow_id: id.into(),
                    run_id: String::new(),
                }),
            })
            .await
            .expect("describe")
            .into_inner();
        assert_eq!(
            d.workflow_execution_info.expect("info").status,
            status as i32,
            "{id}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_signal_batch_run_twice_signals_once() {
    let gw = common::start().await;
    let mut c = gw.client.clone();
    let run = c
        .start_workflow_execution(start_req("s1", "t"))
        .await
        .expect("start")
        .into_inner()
        .run_id;
    for _ in 0..2 {
        c.start_batch_operation(StartBatchOperationRequest {
            namespace: "default".into(),
            job_id: "job-s".into(),
            reason: "poke".into(),
            target_executions: vec![Execution {
                r#type: ExecutionType::Workflow as i32,
                business_id: "s1".into(),
                run_id: run.clone(),
            }],
            max_operations_per_second: 1000.0,
            operation: Some(Operation::SignalOperation(BatchOperationSignal {
                signal: "poke".into(),
                ..Default::default()
            })),
            ..Default::default()
        })
        .await
        .expect("batch");
        assert_eq!(wait_done(&mut c, "job-s").await, (1, 1));
    }
    let h = c
        .get_workflow_execution_history(GetWorkflowExecutionHistoryRequest {
            namespace: "default".into(),
            execution: Some(WorkflowExecution {
                workflow_id: "s1".into(),
                run_id: run,
            }),
            ..Default::default()
        })
        .await
        .expect("history")
        .into_inner();
    let signals = h
        .history
        .expect("history")
        .events
        .iter()
        .filter(|e| e.event_type == EventType::WorkflowExecutionSignaled as i32)
        .count();
    assert_eq!(signals, 1);
}
