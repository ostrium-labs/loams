//! `OperatorService` (D2052): the part the Temporal Web UI and the
//! `temporal` CLI read.
//!
//! - `ListSearchAttributes`: the system attributes the gateway records for
//!   every run (its visibility row). There are no custom attributes yet;
//!   adding and removing them, and the listing that queries them, belong to
//!   the schedules and visibility work (TMP-sched), which extends this file.
//! - The Nexus endpoint methods (`ListNexusEndpoints`, ...) answer
//!   `UNIMPLEMENTED` with a message that says Nexus is not supported
//!   ([`crate::capabilities::unimplemented_message`]), so a UI page or an
//!   SDK can tell "no Nexus here" from a transient error (D2053).
//! - Every other method answers `UNIMPLEMENTED` with the matrix link, as
//!   `WorkflowService`'s do (X1.R18).

use std::collections::HashMap;
use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use async_trait::async_trait;
use loams_temporal_proto::temporal::api::enums::v1::IndexedValueType;
use loams_temporal_proto::temporal::api::operatorservice::v1::operator_service_server::{
    OperatorService, OperatorServiceServer,
};
use loams_temporal_proto::temporal::api::operatorservice::v1::*;
use tonic::body::Body;
use tonic::server::NamedService;
use tonic::{Request, Response, Status};

use crate::capabilities::{OPERATOR_IMPLEMENTED, unimplemented_message};
use crate::service::Frontend;

/// The full gRPC name of `OperatorService`.
pub const OPERATOR_SERVICE: &str = "temporal.api.operatorservice.v1.OperatorService";

/// The system search attributes: the fields of the gateway's visibility row
/// (Temporal's names and types). Temporal lists more (`HistoryLength`,
/// `BuildIds`, ...); the gateway lists only what it stores.
pub const SYSTEM_SEARCH_ATTRIBUTES: &[(&str, IndexedValueType)] = &[
    ("WorkflowId", IndexedValueType::Keyword),
    ("RunId", IndexedValueType::Keyword),
    ("WorkflowType", IndexedValueType::Keyword),
    ("ExecutionStatus", IndexedValueType::Keyword),
    ("TaskQueue", IndexedValueType::Keyword),
    ("StartTime", IndexedValueType::Datetime),
    ("CloseTime", IndexedValueType::Datetime),
    ("ExecutionTime", IndexedValueType::Datetime),
];

/// The `OperatorService` over a shared [`Frontend`].
#[derive(Debug, Clone)]
pub struct OperatorServiceImpl(pub Arc<Frontend>);

#[async_trait]
impl OperatorService for OperatorServiceImpl {
    async fn list_search_attributes(
        &self,
        req: Request<ListSearchAttributesRequest>,
    ) -> Result<Response<ListSearchAttributesResponse>, Status> {
        let p = self.0.principal(&req)?;
        let r = req.into_inner();
        // Temporal's SQL visibility needs the namespace; check it is the
        // caller's.
        self.0.namespace(&p, &r.namespace).await?;
        Ok(Response::new(ListSearchAttributesResponse {
            custom_attributes: HashMap::new(),
            system_attributes: SYSTEM_SEARCH_ATTRIBUTES
                .iter()
                .map(|(name, t)| ((*name).to_string(), *t as i32))
                .collect(),
            storage_schema: HashMap::new(),
        }))
    }
}

/// Answers `UNIMPLEMENTED` (with the matrix link, or the Nexus message) for
/// every `OperatorService` method outside [`OPERATOR_IMPLEMENTED`].
#[derive(Debug, Clone)]
pub struct OperatorImplementedOnly<S>(pub S);

const PREFIX: &str = "/temporal.api.operatorservice.v1.OperatorService/";

/// The service to mount for `frontend`'s `OperatorService`.
pub fn operator_service(
    frontend: Arc<Frontend>,
) -> OperatorImplementedOnly<OperatorServiceServer<OperatorServiceImpl>> {
    OperatorImplementedOnly(
        OperatorServiceServer::new(OperatorServiceImpl(frontend))
            .accept_compressed(tonic::codec::CompressionEncoding::Gzip)
            .send_compressed(tonic::codec::CompressionEncoding::Gzip),
    )
}

impl<S> tower::Service<http::Request<Body>> for OperatorImplementedOnly<S>
where
    S: tower::Service<http::Request<Body>, Response = http::Response<Body>, Error = Infallible>
        + Send,
    S::Future: Send + 'static,
{
    type Response = http::Response<Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Infallible>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        self.0.poll_ready(cx)
    }

    fn call(&mut self, req: http::Request<Body>) -> Self::Future {
        if let Some(method) = req.uri().path().strip_prefix(PREFIX)
            && !OPERATOR_IMPLEMENTED.contains(&method)
        {
            let status = Status::unimplemented(unimplemented_message(method));
            return Box::pin(async move { Ok(status.into_http()) });
        }
        Box::pin(self.0.call(req))
    }
}

impl<S> NamedService for OperatorImplementedOnly<S> {
    const NAME: &'static str = OPERATOR_SERVICE;
}
