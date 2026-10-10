// The generated service traits return `impl Encodable<_>`; these impls name the concrete message
// type, which is the intended refinement (as `connect.rs` does).
#![allow(refining_impl_trait)]

//! `loams.graph.v1` on the main port (GR1 Task 5, design §48 §8, D741).
//!
//! With the `graph` feature and a graph runtime (dev and standalone), both services are served
//! over [`GraphAdmin`](loams_graph::service::admin::GraphAdmin): the catalog, the lazily opened
//! engine, and every engine call on the blocking pool. Otherwise every RPC answers
//! `unimplemented` with the reason `feature_not_in_variant` ([`GraphAbsent`]), so a caller can
//! tell "not in this build" from a missing route, and reflection never lists a service no route
//! answers.
//!
//! A statement's deadline is the earlier of its own limit (`timeout_ms`, or the graph's) and the
//! client's Connect timeout ([`GraphAdmin::for_call`](loams_graph::service::admin::GraphAdmin::for_call),
//! GR1 Task 6).
//!
//! Not served yet, answered `not_implemented`: `RestoreGraph`, `ExportGraph`, `ImportGraph`
//! (Task 29).

use std::sync::Arc;

#[cfg(feature = "graph")]
use connectrpc::Response;
use connectrpc::{RequestContext, Router, ServiceRequest, ServiceResult, ServiceStream};
use loams_proto::loams::graph::v1 as pb;
use loams_proto::loams::graph::v1::{
    GraphAdminService, GraphAdminServiceExt, GraphService, GraphServiceExt,
};
use loams_proto::loams::operations::v1::Operation;

use super::AppState;
#[cfg(feature = "graph")]
use super::connect::not_implemented;
use super::connect::not_in_variant;

/// Whether this server serves `loams.graph.v1` (for `GetInstance.services[]`).
pub(crate) fn served(state: &AppState) -> bool {
    #[cfg(feature = "graph")]
    {
        state.graph.is_some()
    }
    #[cfg(not(feature = "graph"))]
    {
        let _ = state;
        false
    }
}

/// Registers both graph services: served, or the `feature_not_in_variant` stub.
pub(super) fn register(router: Router, state: &AppState) -> Router {
    #[cfg(feature = "graph")]
    if let Some(admin) = &state.graph {
        let served = Arc::new(Served {
            admin: Arc::clone(admin),
        });
        let router = GraphAdminServiceExt::register(Arc::clone(&served), router);
        return GraphServiceExt::register(served, router);
    }
    let _ = state;
    let absent = Arc::new(GraphAbsent);
    let router = GraphAdminServiceExt::register(Arc::clone(&absent), router);
    GraphServiceExt::register(absent, router)
}

/// Both graph services when this server does not serve them.
#[derive(Debug)]
struct GraphAbsent;

impl GraphAdminService for GraphAbsent {
    async fn get_engine_info(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::GetEngineInfoRequest>,
    ) -> ServiceResult<pb::EngineInfo> {
        Err(not_in_variant(
            "loams.graph.v1.GraphAdminService/GetEngineInfo",
        ))
    }
    async fn create_graph(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::CreateGraphRequest>,
    ) -> ServiceResult<pb::Graph> {
        Err(not_in_variant(
            "loams.graph.v1.GraphAdminService/CreateGraph",
        ))
    }
    async fn get_graph(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::GetGraphRequest>,
    ) -> ServiceResult<pb::Graph> {
        Err(not_in_variant("loams.graph.v1.GraphAdminService/GetGraph"))
    }
    async fn list_graphs(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::ListGraphsRequest>,
    ) -> ServiceResult<pb::ListGraphsResponse> {
        Err(not_in_variant(
            "loams.graph.v1.GraphAdminService/ListGraphs",
        ))
    }
    async fn update_graph(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::UpdateGraphRequest>,
    ) -> ServiceResult<pb::Graph> {
        Err(not_in_variant(
            "loams.graph.v1.GraphAdminService/UpdateGraph",
        ))
    }
    async fn delete_graph(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::DeleteGraphRequest>,
    ) -> ServiceResult<Operation> {
        Err(not_in_variant(
            "loams.graph.v1.GraphAdminService/DeleteGraph",
        ))
    }
    async fn get_schema(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::GetSchemaRequest>,
    ) -> ServiceResult<pb::GraphSchema> {
        Err(not_in_variant("loams.graph.v1.GraphAdminService/GetSchema"))
    }
    async fn restore_graph(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::RestoreGraphRequest>,
    ) -> ServiceResult<Operation> {
        Err(not_in_variant(
            "loams.graph.v1.GraphAdminService/RestoreGraph",
        ))
    }
    async fn export_graph(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::ExportGraphRequest>,
    ) -> ServiceResult<Operation> {
        Err(not_in_variant(
            "loams.graph.v1.GraphAdminService/ExportGraph",
        ))
    }
    async fn import_graph(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::ImportGraphRequest>,
    ) -> ServiceResult<Operation> {
        Err(not_in_variant(
            "loams.graph.v1.GraphAdminService/ImportGraph",
        ))
    }
}

impl GraphService for GraphAbsent {
    async fn execute(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::ExecuteRequest>,
    ) -> ServiceResult<pb::ExecuteResponse> {
        Err(not_in_variant("loams.graph.v1.GraphService/Execute"))
    }
    async fn execute_batch(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::ExecuteBatchRequest>,
    ) -> ServiceResult<pb::ExecuteBatchResponse> {
        Err(not_in_variant("loams.graph.v1.GraphService/ExecuteBatch"))
    }
    async fn explain(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::ExplainRequest>,
    ) -> ServiceResult<pb::Plan> {
        Err(not_in_variant("loams.graph.v1.GraphService/Explain"))
    }

    async fn execute_stream(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::ExecuteStreamRequest>,
    ) -> ServiceResult<ServiceStream<pb::ResultChunk>> {
        Err(not_in_variant("loams.graph.v1.GraphService/ExecuteStream"))
    }
}

/// Both graph services over the server's graph runtime.
#[cfg(feature = "graph")]
#[derive(Debug)]
struct Served {
    admin: Arc<loams_graph::service::admin::GraphAdmin>,
}

#[cfg(feature = "graph")]
impl GraphAdminService for Served {
    async fn get_engine_info(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::GetEngineInfoRequest>,
    ) -> ServiceResult<pb::EngineInfo> {
        Response::ok(self.admin.get_engine_info(request.to_owned_message()).await)
    }
    async fn create_graph(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::CreateGraphRequest>,
    ) -> ServiceResult<pb::Graph> {
        Response::ok(self.admin.create_graph(request.to_owned_message()).await?)
    }
    async fn get_graph(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::GetGraphRequest>,
    ) -> ServiceResult<pb::Graph> {
        Response::ok(self.admin.get_graph(request.to_owned_message()).await?)
    }
    async fn list_graphs(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListGraphsRequest>,
    ) -> ServiceResult<pb::ListGraphsResponse> {
        Response::ok(self.admin.list_graphs(request.to_owned_message()).await?)
    }
    async fn update_graph(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::UpdateGraphRequest>,
    ) -> ServiceResult<pb::Graph> {
        Response::ok(self.admin.update_graph(request.to_owned_message()).await?)
    }
    async fn delete_graph(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::DeleteGraphRequest>,
    ) -> ServiceResult<Operation> {
        Response::ok(self.admin.delete_graph(request.to_owned_message()).await?)
    }
    async fn get_schema(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::GetSchemaRequest>,
    ) -> ServiceResult<pb::GraphSchema> {
        Response::ok(
            self.admin
                .for_call(ctx.deadline())
                .get_schema(request.to_owned_message())
                .await?,
        )
    }
    async fn restore_graph(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::RestoreGraphRequest>,
    ) -> ServiceResult<Operation> {
        Err(not_implemented(
            "loams.graph.v1.GraphAdminService/RestoreGraph",
        ))
    }
    async fn export_graph(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::ExportGraphRequest>,
    ) -> ServiceResult<Operation> {
        Err(not_implemented(
            "loams.graph.v1.GraphAdminService/ExportGraph",
        ))
    }
    async fn import_graph(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::ImportGraphRequest>,
    ) -> ServiceResult<Operation> {
        Err(not_implemented(
            "loams.graph.v1.GraphAdminService/ImportGraph",
        ))
    }
}

#[cfg(feature = "graph")]
impl GraphService for Served {
    async fn execute(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ExecuteRequest>,
    ) -> ServiceResult<pb::ExecuteResponse> {
        Response::ok(
            self.admin
                .for_call(ctx.deadline())
                .execute(request.to_owned_message())
                .await?,
        )
    }
    async fn execute_batch(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ExecuteBatchRequest>,
    ) -> ServiceResult<pb::ExecuteBatchResponse> {
        Response::ok(
            self.admin
                .for_call(ctx.deadline())
                .execute_batch(request.to_owned_message())
                .await?,
        )
    }
    async fn explain(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ExplainRequest>,
    ) -> ServiceResult<pb::Plan> {
        Response::ok(
            self.admin
                .for_call(ctx.deadline())
                .explain(request.to_owned_message())
                .await?,
        )
    }

    async fn execute_stream(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, pb::ExecuteStreamRequest>,
    ) -> ServiceResult<ServiceStream<pb::ResultChunk>> {
        Response::ok(
            self.admin
                .for_call(ctx.deadline())
                .execute_stream(request.to_owned_message())
                .await?,
        )
    }
}
