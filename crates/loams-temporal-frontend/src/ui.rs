//! What the stock Temporal Web UI (`temporalio/ui`, its `ui-server`) and the
//! `temporal` CLI need beyond the DX1 set (D2050–D2053):
//!
//! - [`cluster_info`]: `GetClusterInfo`, which the UI reads first
//!   (`/api/v1/cluster-info`) and `temporal operator cluster describe` prints.
//! - [`history_reverse`]: `GetWorkflowExecutionHistoryReverse`, which the
//!   UI's workflow page reads for its summary (`.../history-reverse`).
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

use loams_temporal_proto::temporal::api::history::v1::History;
use loams_temporal_proto::temporal::api::version::v1::VersionInfo;
use loams_temporal_proto::temporal::api::workflowservice::v1::{
    GetClusterInfoResponse, GetWorkflowExecutionHistoryReverseRequest,
    GetWorkflowExecutionHistoryReverseResponse,
};
use tonic::body::Body;
use tonic::server::NamedService;
use tonic::{Request, Status};

use crate::auth::Principal;
use crate::service::Frontend;

/// The principal of a call that came in on the UI listener (D2050). Set by
/// [`UiListener`] as a request extension; [`Frontend`] prefers it to the
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

/// `GetClusterInfo` (D2051). The server version is the gateway's own
/// (`loams-temporal x.y.z`), never a Temporal server version: the UI gates
/// features on it, and a Temporal version would turn on pages whose methods
/// the gateway does not serve. `visibility_store` stays empty until the
/// gateway has a visibility listing (TMP-sched), so the UI does not offer
/// advanced visibility queries.
pub fn cluster_info(f: &Frontend) -> GetClusterInfoResponse {
    let c = &f.config;
    GetClusterInfoResponse {
        supported_clients: Default::default(),
        server_version: c.server_version.clone(),
        cluster_id: c.cluster_name.clone(),
        version_info: Some(VersionInfo::default()),
        cluster_name: c.cluster_name.clone(),
        history_shard_count: c.history_shard_count,
        persistence_store: c.persistence_store.clone(),
        visibility_store: String::new(),
        initial_failover_version: 1,
        failover_version_increment: 10,
    }
}

/// One page of a run's history, newest event first.
///
/// The page token is the 8-byte big-endian ID of the oldest event of the
/// page just returned, which is the exclusive upper bound of the next page;
/// without a token the page ends at the run's last event. The last page
/// (the one holding event 1) has no token.
pub async fn history_reverse(
    f: &Frontend,
    req: Request<GetWorkflowExecutionHistoryReverseRequest>,
) -> Result<GetWorkflowExecutionHistoryReverseResponse, Status> {
    let p = f.principal(&req)?;
    let r = req.into_inner();
    let ns = f.namespace(&p, &r.namespace).await?;
    let key = f.run_key(&ns, r.execution.as_ref()).await?;
    let run = f.engine.load(&key).await?;
    let upper = reverse_upper(&r.next_page_token, run.state.next_event_id)?;
    let page = match r.maximum_page_size {
        n if n > 0 => n as usize,
        _ => f.config.history_page_size,
    };
    let (from, count) = reverse_window(upper, page);
    let mut events = if count == 0 {
        Vec::new()
    } else {
        f.engine.history(&key, from, count).await?
    };
    events.reverse();
    Ok(GetWorkflowExecutionHistoryReverseResponse {
        history: Some(History { events }),
        next_page_token: if from > 1 {
            from.to_be_bytes().to_vec().into()
        } else {
            Default::default()
        },
    })
}

/// The exclusive upper event ID of a reverse page: the token's, or the
/// run's next event ID on the first page.
fn reverse_upper(token: &[u8], next_event_id: i64) -> Result<i64, Status> {
    if token.is_empty() {
        return Ok(next_event_id);
    }
    let bytes: [u8; 8] = token
        .try_into()
        .map_err(|_| Status::invalid_argument("Invalid NextPageToken."))?;
    let upper = i64::from_be_bytes(bytes);
    if upper < 1 || upper > next_event_id {
        return Err(Status::invalid_argument("Invalid NextPageToken."));
    }
    Ok(upper)
}

/// The first event ID and the event count of a reverse page that ends
/// before `upper` and holds at most `page` events.
fn reverse_window(upper: i64, page: usize) -> (i64, usize) {
    let page = i64::try_from(page).unwrap_or(i64::MAX).max(1);
    let from = (upper - page).max(1);
    let count = usize::try_from(upper - from).unwrap_or(0);
    (from, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_pages_walk_down_to_the_first_event() {
        // A run with events 1..=10 (next event 11), pages of 4.
        let upper = reverse_upper(&[], 11).expect("first page");
        assert_eq!(reverse_window(upper, 4), (7, 4));
        assert_eq!(reverse_window(7, 4), (3, 4));
        assert_eq!(reverse_window(3, 4), (1, 2));
        // A page larger than the history.
        assert_eq!(reverse_window(11, 1000), (1, 10));
        // An empty history.
        assert_eq!(reverse_window(1, 10), (1, 0));
        // A zero page size still moves.
        assert_eq!(reverse_window(5, 0), (4, 1));
    }

    #[test]
    fn reverse_tokens_are_checked() {
        assert_eq!(reverse_upper(&7i64.to_be_bytes(), 11).expect("ok"), 7);
        assert!(reverse_upper(&[1, 2, 3], 11).is_err());
        assert!(reverse_upper(&12i64.to_be_bytes(), 11).is_err());
        assert!(reverse_upper(&0i64.to_be_bytes(), 11).is_err());
    }

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
