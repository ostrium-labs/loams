//! The Connect-RPC surface over an embedded [`Engine`]: `loams.graph.v1`'s `GraphService`
//! ([`data`]) and the part of `GraphAdminService` an in-process registry can answer.
//!
//! This layer is transport. What a statement may do is decided in [`crate::classify`] and enforced
//! again by the engine's roles ([`crate::engine`]); values cross through [`crate::value`].
//!
//! **Transitional.** What is not here yet, and the task that adds it:
//!
//! * the catalog (versions, AIP-158 pages, `GetSchema`, deleting through an operation that purges
//!   storage): Task 4. Until then `ListGraphs` answers every graph in one page and `DeleteGraph`
//!   closes the graph and answers a finished operation with no id;
//! * limits, `truncated`, `ExecuteStream` and `Explain`'s plan: Task 6;
//! * consistency tokens, commit epochs, counters and idempotency: Tasks 11 and 15;
//! * LINKED graphs: Task 17. Restore, export and import: Task 29.
//!
//! A graph is created under the engine's data directory, `<data_dir>/graphs/<graph_id>/`, or in
//! memory when the engine has none (Task 3). No request names a path (Review Focus 4).
//!
//! The functions are free rather than methods on [`GraphServiceImpl`] because they are what the
//! generated server traits will call one for one (Task 5), and a test can call them without a
//! router.

pub mod data;
pub(crate) mod errors;

use std::sync::Arc;

use connectrpc::{ConnectError, ErrorCode};
use loams_proto::loams::graph::v1 as pb;
use loams_proto::loams::operations::v1 as ops;

pub use data::{execute, execute_batch, explain};
use errors::{internal, map_engine, refuse};

use crate::GraphId;
use crate::engine::{Engine, Graph, GraphState, OpenSpec};
use data::check_language;

/// `loams.graph.v1` over one embedded engine.
#[derive(Debug, Clone)]
pub struct GraphServiceImpl {
    engine: Arc<Engine>,
}

impl GraphServiceImpl {
    /// Serves `engine`.
    #[must_use]
    pub fn new(engine: Arc<Engine>) -> Self {
        Self { engine }
    }

    /// The engine behind this service, for a caller that also holds it.
    #[must_use]
    pub fn engine(&self) -> &Arc<Engine> {
        &self.engine
    }

    /// `GetEngineInfo`: what this build is.
    pub fn engine_info(&self) -> pb::EngineInfo {
        engine_info(&self.engine)
    }
}

/// Fills a `Graph` message for an open graph.
fn graph_message(graph: &Graph) -> pb::Graph {
    pb::Graph {
        namespace: graph.namespace().to_string(),
        name: graph.name().to_string(),
        // The storage directory is named by the id, so it is read back from there; an in-memory
        // graph has none until Task 4's catalog assigns every graph one.
        id: graph
            .storage_dir()
            .and_then(|dir| dir.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        mode: pb::GraphMode::Owned.into(),
        languages: vec![pb::QueryLanguage::Gql.into()],
        state: match graph.state() {
            GraphState::Ready => pb::GraphState::Ready,
            GraphState::Poisoned => pb::GraphState::Reloading,
            GraphState::Failed => pb::GraphState::Failed,
        }
        .into(),
        ..Default::default()
    }
}

/// `GetEngineInfo`: the engine and the languages this build has.
pub fn engine_info(engine: &Engine) -> pb::EngineInfo {
    pb::EngineInfo {
        engine_version: engine.engine_version.to_string(),
        languages: vec![pb::QueryLanguage::Gql.into()],
        ..Default::default()
    }
}

/// `CreateGraph`: opens an OWNED graph, idempotently by name, under `<data_dir>/graphs/<id>/`
/// (or in memory when the engine has no data directory).
pub fn create_graph(
    engine: &Engine,
    req: pb::CreateGraphRequest,
) -> Result<pb::Graph, ConnectError> {
    if req.mode.as_known() == Some(pb::GraphMode::Linked) {
        return Err(refuse(
            ErrorCode::Unimplemented,
            "not_implemented",
            "LINKED graphs are not served yet (GR1 Task 17)",
        ));
    }
    for language in &req.languages {
        check_language(language.as_known(), language)?;
    }
    crate::engine::validate_names(&req.namespace, &req.name).map_err(map_engine)?;
    let graph = Graph::open_or_existing(engine, &req.namespace, &req.name, || {
        OpenSpec::persistent(engine, GraphId::new()).unwrap_or_else(OpenSpec::in_memory)
    })
    .map_err(map_engine)?;
    Ok(graph_message(&graph))
}

/// `GetGraph`: one open graph.
pub fn get_graph(engine: &Engine, req: pb::GetGraphRequest) -> Result<pb::Graph, ConnectError> {
    let graph = find(engine, &req.namespace, &req.name)?;
    Ok(graph_message(&graph))
}

/// `ListGraphs`: the open graphs of a namespace, or of every namespace when it is empty, in one
/// page (Task 4 pages them).
pub fn list_graphs(
    engine: &Engine,
    req: pb::ListGraphsRequest,
) -> Result<pb::ListGraphsResponse, ConnectError> {
    let graphs = engine
        .list((!req.namespace.is_empty()).then_some(req.namespace.as_str()))
        .map_err(internal)?;
    Ok(pb::ListGraphsResponse {
        graphs: graphs.iter().map(|graph| graph_message(graph)).collect(),
        ..Default::default()
    })
}

/// `DeleteGraph`: closes the graph and answers a finished operation. Deleting a graph that is not
/// open succeeds: it is already in the state asked for.
pub fn delete_graph(
    engine: &Engine,
    req: pb::DeleteGraphRequest,
) -> Result<ops::Operation, ConnectError> {
    crate::engine::validate_names(&req.namespace, &req.name).map_err(map_engine)?;
    engine.close(&req.namespace, &req.name).map_err(internal)?;
    let mut operation = ops::Operation {
        kind: "graph.delete".to_string(),
        namespace: req.namespace.clone(),
        state: ops::OperationState::Succeeded.into(),
        ..Default::default()
    };
    operation.target.insert("graph".to_string(), req.name);
    Ok(operation)
}

/// Finds an open graph to run a statement on, reopening it first when an engine panic poisoned
/// it (R0.13).
pub(crate) fn find_serving(
    engine: &Engine,
    namespace: &str,
    name: &str,
) -> Result<Arc<Graph>, ConnectError> {
    let graph = find(engine, namespace, name)?;
    engine.reopen_if_poisoned(graph).map_err(map_engine)
}

/// Finds an open graph, or answers `NotFound`.
fn find(engine: &Engine, namespace: &str, name: &str) -> Result<Arc<Graph>, ConnectError> {
    crate::engine::validate_names(namespace, name).map_err(map_engine)?;
    engine
        .list(Some(namespace))
        .map_err(internal)?
        .into_iter()
        .find(|g| g.name() == name)
        .ok_or_else(|| {
            refuse(
                ErrorCode::NotFound,
                "graph_not_found",
                format!("no graph {namespace}/{name} is open"),
            )
        })
}
