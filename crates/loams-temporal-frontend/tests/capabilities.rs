//! `capabilities_match_impl` (D1506): the methods with an implementation,
//! the methods that do not answer `UNIMPLEMENTED`, and design 74 §5.8's DX1
//! set are the same set; `GetSystemInfo` advertises X1.R2's flags.

mod common;

use loams_temporal_frontend::capabilities::{IMPLEMENTED, MATRIX_URL, capabilities};
use loams_temporal_proto::temporal::api::workflowservice::v1::GetSystemInfoRequest;

/// Design 74 §5.8, DX1.
const DX1: &[&str] = &[
    "GetSystemInfo",
    "DescribeNamespace",
    "RegisterNamespace",
    "ListNamespaces",
    "StartWorkflowExecution",
    "DescribeWorkflowExecution",
    "GetWorkflowExecutionHistory",
    "TerminateWorkflowExecution",
    "RequestCancelWorkflowExecution",
    "PollWorkflowTaskQueue",
    "RespondWorkflowTaskCompleted",
    "RespondWorkflowTaskFailed",
    "PollActivityTaskQueue",
    "RespondActivityTaskCompleted",
    "RespondActivityTaskFailed",
    "RespondActivityTaskCanceled",
    "RecordActivityTaskHeartbeat",
    "SignalWorkflowExecution",
    "QueryWorkflow",
    "RespondQueryTaskCompleted",
    "ResetStickyTaskQueue",
];

/// Temporal Schedules (D2070-D2073).
const SCHEDULES: &[&str] = &[
    "CreateSchedule",
    "DescribeSchedule",
    "UpdateSchedule",
    "PatchSchedule",
    "DeleteSchedule",
    "ListSchedules",
    "ListScheduleMatchingTimes",
];

/// The methods lane TMP-api added on top of DX1 (2026-10-27).
const TMP_API: &[&str] = &[
    "UpdateWorkflowExecution",
    "PollWorkflowExecutionUpdate",
    "SignalWithStartWorkflowExecution",
    "ResetWorkflowExecution",
    "RespondActivityTaskCompletedById",
    "RespondActivityTaskFailedById",
    "RespondActivityTaskCanceledById",
    "RecordActivityTaskHeartbeatById",
    "GetWorkflowExecutionHistoryReverse",
    "DescribeTaskQueue",
    "GetClusterInfo",
    "ListTaskQueuePartitions",
    "ShutdownWorker",
    "UpdateNamespace",
    "DeprecateNamespace",
    "GetWorkerBuildIdCompatibility",
    "GetWorkerVersioningRules",
    "GetWorkerTaskReachability",
];

/// Visibility (D2074-D2076).
const VISIBILITY: &[&str] = &[
    "ListOpenWorkflowExecutions",
    "ListClosedWorkflowExecutions",
    "ListWorkflowExecutions",
    "ScanWorkflowExecutions",
    "CountWorkflowExecutions",
    "GetSearchAttributes",
];

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn capabilities_match_impl() {
    let mut sorted_impl: Vec<_> = IMPLEMENTED.to_vec();
    sorted_impl.sort_unstable();
    let mut sorted_dx1: Vec<_> = DX1
        .iter()
        .chain(TMP_API)
        .chain(SCHEDULES)
        .chain(VISIBILITY)
        .copied()
        .collect();
    sorted_dx1.sort_unstable();
    assert_eq!(
        sorted_impl, sorted_dx1,
        "IMPLEMENTED is design 74's DX1 set plus TMP-api's and the schedule methods"
    );

    let gw = common::start().await;
    let all = loams_temporal_proto::workflow_service_methods();
    for m in IMPLEMENTED {
        assert!(
            all.iter().any(|x| x == m),
            "{m} is a WorkflowService method"
        );
    }
    let mut grpc = tonic::client::Grpc::new(gw.channel.clone());
    for method in &all {
        // The polls wait; skip them here (the scenario tests cover them).
        if method.starts_with("Poll") && IMPLEMENTED.contains(&method.as_str()) {
            continue;
        }
        grpc.ready().await.expect("ready");
        let path: http::uri::PathAndQuery =
            format!("/temporal.api.workflowservice.v1.WorkflowService/{method}")
                .parse()
                .expect("path");
        let codec = tonic_prost::ProstCodec::<(), ()>::default();
        let r = grpc.unary(tonic::Request::new(()), path, codec).await;
        let unimplemented = matches!(&r, Err(s) if s.code() == tonic::Code::Unimplemented);
        if IMPLEMENTED.contains(&method.as_str()) {
            assert!(!unimplemented, "{method} is implemented but answered {r:?}");
        } else {
            let s = r.expect_err("unimplemented");
            assert_eq!(s.code(), tonic::Code::Unimplemented, "{method}");
            assert!(
                s.message().contains(MATRIX_URL),
                "{method}: {}",
                s.message()
            );
        }
    }

    let info = gw
        .client
        .clone()
        .get_system_info(GetSystemInfoRequest {})
        .await
        .expect("info")
        .into_inner();
    let caps = info.capabilities.expect("capabilities");
    assert_eq!(caps, capabilities());
    assert!(caps.sdk_metadata && !caps.eager_workflow_start && caps.supports_schedules);
}
