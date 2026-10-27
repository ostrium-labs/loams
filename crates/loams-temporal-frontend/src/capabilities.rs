//! What the frontend implements and advertises (D1506; X1.R2). The CI test
//! `capabilities_match_impl` checks this table against the service.

use loams_temporal_proto::temporal::api::workflowservice::v1::get_system_info_response::Capabilities;

/// The `WorkflowService` methods DX1 implements (design 74 §5.8). Every
/// other method answers `UNIMPLEMENTED` with [`MATRIX_URL`].
pub const IMPLEMENTED: &[&str] = &[
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
    // TMP-api (design 74 §5.8's DX4 update methods, brought forward).
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
    "StartBatchOperation",
    "StopBatchOperation",
    "DescribeBatchOperation",
    "ListBatchOperations",
];

/// Where the per-method, per-SDK compatibility matrix lives (D1512).
pub const MATRIX_URL: &str =
    "https://github.com/ostrium-labs/loams/blob/dev/docs/guides/durable/temporal-gateway.md";

/// The `GetSystemInfo` capabilities: only behaviour that is built (X1.R2).
pub fn capabilities() -> Capabilities {
    Capabilities {
        signal_and_query_header: true,
        internal_error_differentiation: true,
        activity_failure_include_heartbeat: true,
        encoded_failure_attributes: true,
        sdk_metadata: true,
        ..Default::default()
    }
}

/// The message of an `UNIMPLEMENTED` answer for `method`.
pub fn unimplemented_message(method: &str) -> String {
    format!(
        "{method} is not implemented by the Loams Temporal gateway yet; see the compatibility matrix: {MATRIX_URL}"
    )
}
