//! gRPC errors with the Temporal failure details the SDKs decode.

use loams_temporal_proto::temporal::api::errordetails::v1::{
    NamespaceAlreadyExistsFailure, NamespaceNotFoundFailure, WorkflowExecutionAlreadyStartedFailure,
};
use prost::Message;
use tonic::{Code, Status};

/// `google.rpc.Status`, which carries the details (the SDKs read the first
/// one's type to pick an error).
#[derive(Clone, PartialEq, Message)]
struct RpcStatus {
    #[prost(int32, tag = "1")]
    code: i32,
    #[prost(string, tag = "2")]
    message: String,
    #[prost(message, repeated, tag = "3")]
    details: Vec<prost_types::Any>,
}

fn with_detail<M: Message>(code: Code, message: String, type_name: &str, detail: &M) -> Status {
    let status = RpcStatus {
        code: code as i32,
        message: message.clone(),
        details: vec![prost_types::Any {
            type_url: format!("type.googleapis.com/{type_name}"),
            value: detail.encode_to_vec(),
        }],
    };
    Status::with_details(code, message, status.encode_to_vec().into())
}

/// `ALREADY_EXISTS` with `WorkflowExecutionAlreadyStartedFailure`.
pub fn already_started(workflow_id: &str, run_id: &str, request_id: &str) -> Status {
    with_detail(
        Code::AlreadyExists,
        format!(
            "Workflow execution is already running. WorkflowId: {workflow_id}, RunId: {run_id}."
        ),
        "temporal.api.errordetails.v1.WorkflowExecutionAlreadyStartedFailure",
        &WorkflowExecutionAlreadyStartedFailure {
            start_request_id: request_id.to_string(),
            run_id: run_id.to_string(),
            ..Default::default()
        },
    )
}

/// `NOT_FOUND` with `NamespaceNotFoundFailure`.
pub fn namespace_not_found(name: &str) -> Status {
    with_detail(
        Code::NotFound,
        format!("Namespace {name} is not found."),
        "temporal.api.errordetails.v1.NamespaceNotFoundFailure",
        &NamespaceNotFoundFailure {
            namespace: name.to_string(),
        },
    )
}

/// `ALREADY_EXISTS` with `NamespaceAlreadyExistsFailure`.
pub fn namespace_already_exists() -> Status {
    with_detail(
        Code::AlreadyExists,
        "Namespace already exists.".to_string(),
        "temporal.api.errordetails.v1.NamespaceAlreadyExistsFailure",
        &NamespaceAlreadyExistsFailure {},
    )
}

/// `ALREADY_EXISTS` for CreateSchedule on a taken schedule ID, with the
/// `WorkflowExecutionAlreadyStartedFailure` detail the SDKs map to "schedule
/// already running" (D2071).
pub fn schedule_already_exists() -> Status {
    with_detail(
        Code::AlreadyExists,
        "schedule with this ID is already registered".to_string(),
        "temporal.api.errordetails.v1.WorkflowExecutionAlreadyStartedFailure",
        &WorkflowExecutionAlreadyStartedFailure::default(),
    )
}

/// `ALREADY_EXISTS` with `WorkflowExecutionAlreadyStartedFailure` and a
/// message of the caller's (an ID reuse policy refusing a closed run).
pub fn already_started_message(message: String, run_id: &str, request_id: &str) -> Status {
    with_detail(
        Code::AlreadyExists,
        message,
        "temporal.api.errordetails.v1.WorkflowExecutionAlreadyStartedFailure",
        &WorkflowExecutionAlreadyStartedFailure {
            start_request_id: request_id.to_string(),
            run_id: run_id.to_string(),
            ..Default::default()
        },
    )
}

/// `INVALID_ARGUMENT` with `QueryFailedFailure`: the worker's query handler
/// failed (the SDKs turn it into their query-failed error).
pub fn query_failed(
    message: String,
    failure: Option<loams_temporal_proto::temporal::api::failure::v1::Failure>,
) -> Status {
    with_detail(
        Code::InvalidArgument,
        message,
        "temporal.api.errordetails.v1.QueryFailedFailure",
        &loams_temporal_proto::temporal::api::errordetails::v1::QueryFailedFailure { failure },
    )
}
