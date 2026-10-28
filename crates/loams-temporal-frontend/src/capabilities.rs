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
    // Temporal Schedules (D2070-D2073).
    "CreateSchedule",
    "DescribeSchedule",
    "UpdateSchedule",
    "PatchSchedule",
    "DeleteSchedule",
    "ListSchedules",
    "ListScheduleMatchingTimes",
    // Visibility (D2074-D2076).
    "ListWorkflowExecutions",
    "ScanWorkflowExecutions",
    "CountWorkflowExecutions",
    "GetSearchAttributes",
];

/// The `OperatorService` methods the frontend implements (D2052). Every
/// other one answers `UNIMPLEMENTED`: the Nexus endpoint methods with
/// [`NEXUS_UNSUPPORTED`], the rest with [`MATRIX_URL`].
pub const OPERATOR_IMPLEMENTED: &[&str] = &[
    "ListSearchAttributes",
    // The search-attribute registry (D2075).
    "AddSearchAttributes",
    "RemoveSearchAttributes",
];

/// Why every Nexus method answers `UNIMPLEMENTED` (D2053).
pub const NEXUS_UNSUPPORTED: &str = "Nexus is not supported by the Loams Temporal gateway: it has no Nexus endpoints, and Nexus operations and Nexus task queues are not served";

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
        supports_schedules: true,
        count_group_by_execution_status: true,
        ..Default::default()
    }
}

/// The message of an `UNIMPLEMENTED` answer for `method` (of either
/// service). A Nexus method says that Nexus is not supported (D2053).
pub fn unimplemented_message(method: &str) -> String {
    if method.contains("Nexus") {
        return format!(
            "{method}: {NEXUS_UNSUPPORTED}; see the compatibility matrix: {MATRIX_URL}"
        );
    }
    format!(
        "{method} is not implemented by the Loams Temporal gateway yet; see the compatibility matrix: {MATRIX_URL}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nexus_methods_say_nexus_is_unsupported() {
        for m in [
            "ListNexusEndpoints",
            "CreateNexusEndpoint",
            "PollNexusTaskQueue",
            "StartNexusOperationExecution",
        ] {
            let msg = unimplemented_message(m);
            assert!(msg.contains(NEXUS_UNSUPPORTED), "{msg}");
            assert!(msg.contains(MATRIX_URL), "{msg}");
        }
        let msg = unimplemented_message("CreateSchedule");
        assert!(!msg.contains("Nexus") && msg.contains(MATRIX_URL), "{msg}");
    }
}
