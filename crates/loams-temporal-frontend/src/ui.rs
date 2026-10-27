//! What the stock Temporal Web UI (`temporalio/ui`, its `ui-server`) and the
//! `temporal` CLI need beyond the DX1 set (D2050–D2053):
//!
//! - `GetClusterInfo` and `GetWorkflowExecutionHistoryReverse`, which the UI
//!   reads first, are served by `describe.rs` and `service.rs` (lane TMP-api).
//! - [`UiListener`]: the UI's own listener. `ui-server` cannot send a static
//!   API key (it forwards only OIDC tokens or browser headers), so the
//!   gateway gives it a second, loopback-only listener whose every call is
//!   one configured principal (D2050). The principal travels as a request
//!   extension, which no network client can set; by default the listener
//!   accepts only [`READ_METHODS`].

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tonic::Status;
use tonic::body::Body;
use tonic::server::NamedService;

use crate::auth::Principal;

/// The principal of a call that came in on the UI listener (D2050). Set by
/// [`UiListener`] as a request extension; [`crate::service::Frontend`] prefers it to the
/// `authorization` header.
#[derive(Debug, Clone)]
pub struct TrustedPrincipal(pub Principal);

/// The methods a read-only UI listener passes (D2050): what the UI and the
/// CLI read. Anything else answers `PERMISSION_DENIED`. Methods here that
/// the gateway does not implement still answer `UNIMPLEMENTED`.
pub const READ_METHODS: &[&str] = &[
    // WorkflowService
    "/temporal.api.workflowservice.v1.WorkflowService/GetSystemInfo",
    "/temporal.api.workflowservice.v1.WorkflowService/GetClusterInfo",
    "/temporal.api.workflowservice.v1.WorkflowService/ListNamespaces",
    "/temporal.api.workflowservice.v1.WorkflowService/DescribeNamespace",
    "/temporal.api.workflowservice.v1.WorkflowService/DescribeWorkflowExecution",
    "/temporal.api.workflowservice.v1.WorkflowService/GetWorkflowExecutionHistory",
    "/temporal.api.workflowservice.v1.WorkflowService/GetWorkflowExecutionHistoryReverse",
    "/temporal.api.workflowservice.v1.WorkflowService/QueryWorkflow",
    "/temporal.api.workflowservice.v1.WorkflowService/ListWorkflowExecutions",
    "/temporal.api.workflowservice.v1.WorkflowService/CountWorkflowExecutions",
    "/temporal.api.workflowservice.v1.WorkflowService/ListOpenWorkflowExecutions",
    "/temporal.api.workflowservice.v1.WorkflowService/ListClosedWorkflowExecutions",
    "/temporal.api.workflowservice.v1.WorkflowService/ListArchivedWorkflowExecutions",
    "/temporal.api.workflowservice.v1.WorkflowService/GetSearchAttributes",
    "/temporal.api.workflowservice.v1.WorkflowService/DescribeTaskQueue",
    "/temporal.api.workflowservice.v1.WorkflowService/ListSchedules",
    "/temporal.api.workflowservice.v1.WorkflowService/DescribeSchedule",
    "/temporal.api.workflowservice.v1.WorkflowService/CountSchedules",
    "/temporal.api.workflowservice.v1.WorkflowService/ListBatchOperations",
    "/temporal.api.workflowservice.v1.WorkflowService/DescribeBatchOperation",
    "/temporal.api.workflowservice.v1.WorkflowService/ListWorkers",
    "/temporal.api.workflowservice.v1.WorkflowService/CountWorkers",
    "/temporal.api.workflowservice.v1.WorkflowService/ListWorkerDeployments",
    // OperatorService
    "/temporal.api.operatorservice.v1.OperatorService/ListSearchAttributes",
    "/temporal.api.operatorservice.v1.OperatorService/ListNexusEndpoints",
    "/temporal.api.operatorservice.v1.OperatorService/GetNexusEndpoint",
    "/temporal.api.operatorservice.v1.OperatorService/ListClusters",
];

/// The message of a write refused on a read-only UI listener.
pub const READ_ONLY_MESSAGE: &str = "the Loams Temporal gateway's UI listener is read-only (`[ui] read_only = true`); use the main listener with an API key to change workflows";

/// Wraps a service on the UI listener: every request carries `principal`
/// as a [`TrustedPrincipal`], and with `read_only` only [`READ_METHODS`]
/// pass.
#[derive(Debug, Clone)]
pub struct UiListener<S> {
    inner: S,
    principal: Principal,
    read_only: bool,
}

impl<S> UiListener<S> {
    /// `inner` on the UI listener as `principal`.
    pub fn new(inner: S, principal: Principal, read_only: bool) -> Self {
        UiListener {
            inner,
            principal,
            read_only,
        }
    }
}

/// Whether a read-only UI listener lets `path` through.
pub fn ui_allows(path: &str, read_only: bool) -> bool {
    !read_only || READ_METHODS.contains(&path)
}

impl<S> tower::Service<http::Request<Body>> for UiListener<S>
where
    S: tower::Service<http::Request<Body>, Response = http::Response<Body>, Error = Infallible>
        + Send,
    S::Future: Send + 'static,
{
    type Response = http::Response<Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Infallible>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: http::Request<Body>) -> Self::Future {
        if !ui_allows(req.uri().path(), self.read_only) {
            let status = Status::permission_denied(READ_ONLY_MESSAGE);
            return Box::pin(async move { Ok(status.into_http()) });
        }
        // Whatever a client sent, the UI listener's principal is the one
        // configured; a header cannot override it.
        req.extensions_mut()
            .insert(TrustedPrincipal(self.principal.clone()));
        Box::pin(self.inner.call(req))
    }
}

impl<S: NamedService> NamedService for UiListener<S> {
    const NAME: &'static str = S::NAME;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_only_ui_listener_passes_reads_only() {
        let read = "/temporal.api.workflowservice.v1.WorkflowService/DescribeWorkflowExecution";
        let write = "/temporal.api.workflowservice.v1.WorkflowService/TerminateWorkflowExecution";
        let op = "/temporal.api.operatorservice.v1.OperatorService/ListSearchAttributes";
        let op_write = "/temporal.api.operatorservice.v1.OperatorService/CreateNexusEndpoint";
        assert!(ui_allows(read, true));
        assert!(ui_allows(op, true));
        assert!(!ui_allows(write, true));
        assert!(!ui_allows(op_write, true));
        assert!(ui_allows(write, false));
        assert!(ui_allows(op_write, false));
    }
}
