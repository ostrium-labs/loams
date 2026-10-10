//! A seeded `loams.graph.v1` for the desktop Graph page (design §48 §18.2, GR1 Task 7).
//!
//! The namespace `default` holds `movies` (OWNED) and `kg` (LINKED), as
//! `conformance/graph/desktop/list_graphs.json` lists them. The catalog is real and in memory:
//! `CreateGraph`, `GetGraph`, `ListGraphs` (AIP-158 pages by name) and `DeleteGraph` work on it.
//! Statements are not run: `Execute`, `ExecuteStream`, `Explain` and `GetSchema` answer a request
//! that equals one of the [`contract`] fixtures with that fixture's answer, so the mock and the
//! server answer the page alike (`crates/loams-graph/tests/desktop_contract.rs` checks both). Any
//! other statement answers `unimplemented` with reason `not_implemented`, and so do
//! `UpdateGraph`, `ExecuteBatch` and the restore, export and import RPCs. No call needs a token,
//! as on a loopback `loams dev`.

// The generated service traits return `impl Encodable<_>`; these impls name
// the concrete message type, which is the intended refinement.
#![allow(refining_impl_trait)]

pub mod contract;

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::SystemTime;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use connectrpc::{
    ConnectError, ErrorCode, ErrorDetail, RequestContext, Response, ServiceRequest, ServiceResult,
    ServiceStream,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use self::contract::Answer;
use crate::proto::loams::errors::v1::ErrorInfo;
use crate::proto::loams::graph::v1 as pb;
use crate::proto::loams::operations::v1::{Operation, OperationState};
use crate::{not_implemented, refuse};

/// The engine version the server reports (`grafeo`, D759); the mock links no engine.
const ENGINE_VERSION: &str = "0.5.43";

/// AIP-158: the page size a request of 0 gets, and the largest one served.
const DEFAULT_PAGE_SIZE: usize = 50;
const MAX_PAGE_SIZE: usize = 1000;

/// A fixture's call, its request in canonical proto3 JSON.
#[derive(Debug)]
struct Canned {
    method: String,
    request: Value,
    answer: Answer,
}

/// Both graph services.
#[derive(Debug)]
pub(crate) struct GraphMock {
    /// `(namespace, name)` → graph.
    graphs: Mutex<BTreeMap<(String, String), pb::Graph>>,
    canned: Vec<Canned>,
}

impl GraphMock {
    /// The seeded mock: the graphs of `list_graphs.json` and every fixture's answer.
    ///
    /// # Panics
    ///
    /// When a compiled-in fixture does not parse as its method's messages: the fixtures are part
    /// of this build, and the crate's tests load them.
    pub(crate) fn seeded() -> Self {
        let mut canned = Vec::new();
        let mut graphs = BTreeMap::new();
        for (name, fixture) in contract::fixtures() {
            for exchange in fixture.exchanges {
                let request = canonical(&exchange.method, &exchange.request)
                    .unwrap_or_else(|err| panic!("{name}: {}: {err}", exchange.method));
                let Some(answer) = exchange.answer() else {
                    panic!("{name}: {} has no answer", exchange.method);
                };
                if exchange.method == "loams.graph.v1.GraphAdminService/ListGraphs"
                    && let Answer::Response(response) = &answer
                {
                    let listed: pb::ListGraphsResponse = serde_json::from_value(response.clone())
                        .unwrap_or_else(|err| panic!("{name}: {err}"));
                    for graph in listed.graphs {
                        graphs.insert((graph.namespace.clone(), graph.name.clone()), graph);
                    }
                }
                canned.push(Canned {
                    method: exchange.method,
                    request,
                    answer,
                });
            }
        }
        Self {
            graphs: Mutex::new(graphs),
            canned,
        }
    }

    fn graphs(&self) -> std::sync::MutexGuard<'_, BTreeMap<(String, String), pb::Graph>> {
        self.graphs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The graph, or `not_found`/`graph_not_found`.
    fn graph(&self, namespace: &str, name: &str) -> Result<pb::Graph, ConnectError> {
        self.graphs()
            .get(&(namespace.to_string(), name.to_string()))
            .cloned()
            .ok_or_else(|| {
                refuse(
                    ErrorCode::NotFound,
                    "graph_not_found",
                    format!("graph {namespace}/{name} not found"),
                )
            })
    }

    /// The fixture answer to `request`, if a fixture has exactly this call.
    fn canned<T: Serialize>(&self, method: &str, request: &T) -> Option<&Answer> {
        let request = serde_json::to_value(request).ok()?;
        self.canned
            .iter()
            .find(|c| c.method == method && c.request == request)
            .map(|c| &c.answer)
    }

    /// A statement RPC's answer: the graph must exist, and the call must be a fixture's.
    fn statement<T: Serialize>(
        &self,
        method: &str,
        namespace: &str,
        graph: &str,
        request: &T,
    ) -> Result<&Answer, ConnectError> {
        self.graph(namespace, graph)?;
        self.canned(method, request).ok_or_else(|| {
            refuse(
                ErrorCode::Unimplemented,
                "not_implemented",
                format!(
                    "{method}: loams-apps-mock runs no statements; it answers only the calls in conformance/graph/desktop"
                ),
            )
        })
    }
}

/// `request` parsed as `method`'s request message and written back: the form the mock compares.
fn canonical(method: &str, request: &Value) -> Result<Value, String> {
    fn via<T: Serialize + DeserializeOwned>(request: &Value) -> Result<Value, String> {
        let typed: T = serde_json::from_value(request.clone()).map_err(|err| err.to_string())?;
        serde_json::to_value(&typed).map_err(|err| err.to_string())
    }
    match method {
        "loams.instance.v1.InstanceService/GetInstance" => {
            via::<crate::proto::loams::instance::v1::GetInstanceRequest>(request)
        }
        "loams.graph.v1.GraphAdminService/ListGraphs" => via::<pb::ListGraphsRequest>(request),
        "loams.graph.v1.GraphAdminService/GetSchema" => via::<pb::GetSchemaRequest>(request),
        "loams.graph.v1.GraphService/Execute" => via::<pb::ExecuteRequest>(request),
        "loams.graph.v1.GraphService/ExecuteStream" => via::<pb::ExecuteStreamRequest>(request),
        "loams.graph.v1.GraphService/Explain" => via::<pb::ExplainRequest>(request),
        other => Err(format!("the mock has no fixture method {other}")),
    }
}

/// A fixture's unary answer as `T`, or its error.
fn unary<T: DeserializeOwned>(answer: &Answer) -> Result<T, ConnectError> {
    match answer {
        Answer::Response(value) => serde_json::from_value(value.clone())
            .map_err(|err| ConnectError::internal(format!("a fixture answer: {err}"))),
        Answer::Error(error) => Err(error_of(error)),
        Answer::Chunks(_) => Err(ConnectError::internal("a stream fixture for a unary call")),
    }
}

/// A fixture's error as the Connect error the server sends.
fn error_of(error: &contract::ErrorAnswer) -> ConnectError {
    let code = error.code.parse().unwrap_or(ErrorCode::Unknown);
    let info = ErrorInfo {
        reason: error.reason.clone(),
        metadata: error
            .metadata
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        ..Default::default()
    };
    ConnectError::new(code, error.message.clone()).with_detail(ErrorDetail::from_message(
        "loams.errors.v1.ErrorInfo",
        &info,
    ))
}

/// The server's name rule (`[a-z][a-z0-9_-]{0,62}`, GR1 R3.9 I3).
fn valid_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= 63
        && bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// An opaque page token naming the last graph of a page, bound to its namespace.
fn page_token(namespace: &str, last: &str) -> String {
    URL_SAFE_NO_PAD.encode(format!("g1\n{namespace}\n{last}"))
}

fn invalid(message: impl Into<String>) -> ConnectError {
    refuse(ErrorCode::InvalidArgument, "invalid_argument", message)
}

impl pb::GraphAdminService for GraphMock {
    async fn get_engine_info(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::GetEngineInfoRequest>,
    ) -> ServiceResult<pb::EngineInfo> {
        Response::ok(pb::EngineInfo {
            engine_version: ENGINE_VERSION.into(),
            languages: vec![pb::QueryLanguage::QUERY_LANGUAGE_GQL.into()],
            ..Default::default()
        })
    }

    async fn create_graph(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::CreateGraphRequest>,
    ) -> ServiceResult<pb::Graph> {
        let req = request.to_owned_message();
        if req.namespace.is_empty() || !valid_name(&req.name) {
            return Err(invalid(format!(
                "graph name {:?} must match [a-z][a-z0-9_-]{{0,62}}, in a namespace",
                req.name
            )));
        }
        if req.mode.as_known() == Some(pb::GraphMode::GRAPH_MODE_LINKED) {
            return Err(refuse(
                ErrorCode::Unimplemented,
                "not_implemented",
                "LINKED graphs are not served yet (GR1 Task 17)",
            ));
        }
        let mut graphs = self.graphs();
        let key = (req.namespace.clone(), req.name.clone());
        if let Some(existing) = graphs.get(&key) {
            return Err(refuse(
                ErrorCode::AlreadyExists,
                "already_exists",
                format!(
                    "graph {}/{} already exists",
                    existing.namespace, existing.name
                ),
            ));
        }
        let graph = pb::Graph {
            namespace: req.namespace,
            name: req.name,
            id: format!("gr_{}", ulid::Ulid::generate()),
            mode: pb::GraphMode::GRAPH_MODE_OWNED.into(),
            languages: vec![pb::QueryLanguage::QUERY_LANGUAGE_GQL.into()],
            limits: req.limits,
            replicas: req.replicas,
            state: pb::GraphState::GRAPH_STATE_READY.into(),
            create_time: crate::seed::ts(SystemTime::now()),
            version: 1,
            ..Default::default()
        };
        graphs.insert(key, graph.clone());
        Response::ok(graph)
    }

    async fn get_graph(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::GetGraphRequest>,
    ) -> ServiceResult<pb::Graph> {
        let req = request.to_owned_message();
        Response::ok(self.graph(&req.namespace, &req.name)?)
    }

    async fn list_graphs(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::ListGraphsRequest>,
    ) -> ServiceResult<pb::ListGraphsResponse> {
        let req = request.to_owned_message();
        if req.namespace.is_empty() {
            return Err(invalid("ListGraphs needs a namespace"));
        }
        let size = match usize::try_from(req.page_size) {
            Ok(0) => DEFAULT_PAGE_SIZE,
            Ok(size) => size.min(MAX_PAGE_SIZE),
            Err(_) => return Err(invalid("page_size must not be negative")),
        };
        let after = if req.page_token.is_empty() {
            None
        } else {
            let text = URL_SAFE_NO_PAD
                .decode(&req.page_token)
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .ok_or_else(|| invalid("page_token is not one this mock issued"))?;
            match text.split('\n').collect::<Vec<_>>()[..] {
                ["g1", namespace, last] if namespace == req.namespace => Some(last.to_string()),
                _ => return Err(invalid("page_token is for another namespace or listing")),
            }
        };
        let graphs = self.graphs();
        let mut page: Vec<pb::Graph> = graphs
            .values()
            .filter(|g| g.namespace == req.namespace)
            .filter(|g| after.as_ref().is_none_or(|last| &g.name > last))
            .take(size + 1)
            .cloned()
            .collect();
        let next_page_token = if page.len() > size {
            page.truncate(size);
            page.last()
                .map(|g| page_token(&req.namespace, &g.name))
                .unwrap_or_default()
        } else {
            String::new()
        };
        Response::ok(pb::ListGraphsResponse {
            graphs: page,
            next_page_token,
            ..Default::default()
        })
    }

    async fn update_graph(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::UpdateGraphRequest>,
    ) -> ServiceResult<pb::Graph> {
        Err(not_implemented(
            "loams.graph.v1.GraphAdminService/UpdateGraph",
        ))
    }

    async fn delete_graph(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::DeleteGraphRequest>,
    ) -> ServiceResult<Operation> {
        let req = request.to_owned_message();
        // One lock: the removal is the existence check, so two deletes cannot both succeed.
        let graph = self
            .graphs()
            .remove(&(req.namespace.clone(), req.name.clone()))
            .ok_or_else(|| {
                refuse(
                    ErrorCode::NotFound,
                    "graph_not_found",
                    format!("graph {}/{} not found", req.namespace, req.name),
                )
            })?;
        let mut operation = Operation {
            id: format!("op-{}", ulid::Ulid::generate().to_string().to_lowercase()),
            kind: "graph.delete".into(),
            namespace: req.namespace,
            state: OperationState::OPERATION_STATE_SUCCEEDED.into(),
            ..Default::default()
        };
        operation.target.insert("graph".into(), req.name);
        operation.target.insert("graph_id".into(), graph.id);
        Response::ok(operation)
    }

    async fn get_schema(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::GetSchemaRequest>,
    ) -> ServiceResult<pb::GraphSchema> {
        let req = request.to_owned_message();
        self.graph(&req.namespace, &req.name)?;
        // A graph no fixture describes (`kg`, or one created here) is empty.
        match self.canned("loams.graph.v1.GraphAdminService/GetSchema", &req) {
            Some(answer) => Response::ok(unary::<pb::GraphSchema>(answer)?),
            None => Response::ok(pb::GraphSchema::default()),
        }
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

impl pb::GraphService for GraphMock {
    async fn execute(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::ExecuteRequest>,
    ) -> ServiceResult<pb::ExecuteResponse> {
        let req = request.to_owned_message();
        let answer = self.statement(
            "loams.graph.v1.GraphService/Execute",
            &req.namespace,
            &req.graph,
            &req,
        )?;
        Response::ok(unary::<pb::ExecuteResponse>(answer)?)
    }

    async fn execute_batch(
        &self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, pb::ExecuteBatchRequest>,
    ) -> ServiceResult<pb::ExecuteBatchResponse> {
        Err(not_implemented("loams.graph.v1.GraphService/ExecuteBatch"))
    }

    async fn execute_stream(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::ExecuteStreamRequest>,
    ) -> ServiceResult<ServiceStream<pb::ResultChunk>> {
        let req = request.to_owned_message();
        let inner = req.request.as_option().cloned().unwrap_or_default();
        let answer = self.statement(
            "loams.graph.v1.GraphService/ExecuteStream",
            &inner.namespace,
            &inner.graph,
            &req,
        )?;
        let chunks: Vec<Result<pb::ResultChunk, ConnectError>> = match answer {
            Answer::Chunks(chunks) => chunks
                .iter()
                .map(|chunk| {
                    serde_json::from_value(chunk.clone())
                        .map_err(|err| ConnectError::internal(format!("a fixture chunk: {err}")))
                })
                .collect(),
            Answer::Error(error) => return Err(error_of(error)),
            Answer::Response(_) => {
                return Err(ConnectError::internal("a unary fixture for a stream"));
            }
        };
        Response::ok(Box::pin(futures::stream::iter(chunks)) as ServiceStream<pb::ResultChunk>)
    }

    async fn explain(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, pb::ExplainRequest>,
    ) -> ServiceResult<pb::Plan> {
        let req = request.to_owned_message();
        let answer = self.statement(
            "loams.graph.v1.GraphService/Explain",
            &req.namespace,
            &req.graph,
            &req,
        )?;
        Response::ok(unary::<pb::Plan>(answer)?)
    }
}
