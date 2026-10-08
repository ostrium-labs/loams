//! The Connect-RPC surface over an embedded [`Engine`]: `loams.graph.v1`'s `GraphService` and the
//! part of `GraphAdminService` an in-process registry can answer.
//!
//! This layer is transport and nothing else. Every decision that matters is in [`crate::engine`]:
//! a statement reaches the engine unchanged and a read-only request is refused before the engine
//! sees it. Values cross through [`crate::value`], which keeps every type (GR1 Task 2).
//!
//! **Transitional (GR1 Task 2).** The proto is §48 §8.2's; the handlers are still the fabric-era
//! registry's, adapted to it. What is not here yet, and the task that adds it:
//!
//! * the catalog (`gr_<ULID>` ids, versions, AIP-158 pages, `GetSchema`, deleting through an
//!   operation that purges storage): Task 4. Until then `Graph.id` is empty, `ListGraphs` answers
//!   every graph in one page, and `DeleteGraph` closes the in-memory graph and answers a finished
//!   operation with no id;
//! * statement classification, engine roles and per-statement language checks: Task 3;
//! * limits, `truncated`, `ExecuteStream` and `Explain`: Task 6;
//! * consistency tokens, commit epochs, counters and idempotency: Tasks 11 and 15;
//! * LINKED graphs: Task 17. Restore, export and import: Task 29.
//!
//! A graph created here is in memory. The service takes no storage path from a client (Review
//! Focus 4); Task 3 derives it from the server's data directory.
//!
//! The functions are free rather than methods on [`GraphServiceImpl`] because they are what the
//! generated server traits will call one for one (Task 5), and a test can call them without a
//! router.

use std::collections::HashMap;
use std::sync::Arc;

use connectrpc::{ConnectError, ErrorCode};
use loams_proto::loams::graph::v1 as pb;
use loams_proto::loams::operations::v1 as ops;

use crate::engine::{BatchStatement, Engine, GraphError, GraphResult, OpenSpec};
use crate::value::{from_proto, to_proto};

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

/// Maps an engine failure onto a Connect-RPC error.
///
/// A statement the engine refused is `InvalidArgument`, because that is what it is: the caller's
/// text was wrong. The engine's own message is kept whole — its syntax span and hint are what make
/// a GQL mistake fixable. (Task 3 adds `ErrorInfo` reasons.)
fn map_engine(err: GraphError) -> ConnectError {
    match err {
        GraphError::Engine(_)
        | GraphError::EmptyStatement
        | GraphError::UnboundParameter { .. }
        | GraphError::InvalidValue(_) => {
            ConnectError::new(ErrorCode::InvalidArgument, err.to_string())
        }
        GraphError::ReadOnly => ConnectError::new(ErrorCode::PermissionDenied, err.to_string()),
        GraphError::LanguageUnavailable(_) => {
            ConnectError::new(ErrorCode::Unimplemented, err.to_string())
        }
        GraphError::Conflict { .. } => ConnectError::new(ErrorCode::AlreadyExists, err.to_string()),
    }
}

/// Maps an internal failure onto a Connect-RPC error, never `InvalidArgument`.
fn internal(err: GraphError) -> ConnectError {
    ConnectError::new(ErrorCode::Internal, err.to_string())
}

/// Refuses a query language this build does not have.
///
/// This build compiles Grafeo with `gql` only (root `Cargo.toml`, R0.2), so asking for another is
/// `Unimplemented` rather than a parse error deep in the engine. A wire number that names no
/// variant is refused rather than read as GQL: running a statement in a language the caller did
/// not ask for is worse than refusing it.
fn check_language(
    language: Option<pb::QueryLanguage>,
    wire_value: impl std::fmt::Display,
) -> Result<(), ConnectError> {
    use pb::QueryLanguage as L;
    match language {
        Some(L::Unspecified) | Some(L::Gql) => Ok(()),
        _ => Err(ConnectError::new(
            ErrorCode::Unimplemented,
            format!(
                "query language {wire_value} is not available in this build; Loams Graph serves \
                 GQL, ISO/IEC 39075 (D634 (a))"
            ),
        )),
    }
}

/// Converts an engine result into its protobuf row set.
fn to_pb_rows(result: &GraphResult) -> pb::RowSet {
    pb::RowSet {
        columns: result.columns.clone(),
        column_types: result.column_types.clone(),
        rows: result
            .rows
            .iter()
            .map(|row| pb::Row {
                values: row.values.iter().map(to_proto).collect(),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

/// Fills one `ExecuteResponse` from an engine result. `counters`, the token and the commit epoch
/// come with the durable write path (Task 11).
fn to_pb_response(result: &GraphResult) -> pb::ExecuteResponse {
    pb::ExecuteResponse {
        rows: to_pb_rows(result).into(),
        elapsed_nanos: result.elapsed_nanos.unwrap_or_default(),
        ..Default::default()
    }
}

/// Decodes a parameter map into the engine's values.
fn to_parameters(
    parameters: &::buffa::__private::HashMap<String, pb::Value>,
) -> Result<HashMap<String, grafeo::Value>, ConnectError> {
    parameters
        .iter()
        .map(|(name, value)| {
            from_proto(value)
                .map(|value| (name.clone(), value))
                .map_err(|err| {
                    ConnectError::new(
                        ErrorCode::InvalidArgument,
                        format!("parameter ${name}: {err}"),
                    )
                })
        })
        .collect()
}

/// Fills a `Graph` message for an open graph.
fn graph_message(graph: &crate::engine::Graph) -> pb::Graph {
    pb::Graph {
        namespace: graph.namespace().to_string(),
        name: graph.name().to_string(),
        mode: pb::GraphMode::Owned.into(),
        languages: vec![pb::QueryLanguage::Gql.into()],
        state: pb::GraphState::Ready.into(),
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

/// `CreateGraph`: opens an in-memory OWNED graph, idempotently by name.
pub fn create_graph(
    engine: &Engine,
    req: pb::CreateGraphRequest,
) -> Result<pb::Graph, ConnectError> {
    if req.mode.as_known() == Some(pb::GraphMode::Linked) {
        return Err(ConnectError::new(
            ErrorCode::Unimplemented,
            "LINKED graphs are not served yet (GR1 Task 17)",
        ));
    }
    for language in &req.languages {
        check_language(language.as_known(), language)?;
    }
    let graph = crate::engine::Graph::open(engine, &req.namespace, &req.name, OpenSpec::default())
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

/// `Execute`: runs one statement. Parameters are bound by the engine, never interpolated.
pub fn execute(
    engine: &Engine,
    req: pb::ExecuteRequest,
) -> Result<pb::ExecuteResponse, ConnectError> {
    check_language(req.language.as_known(), req.language)?;
    let graph = find(engine, &req.namespace, &req.graph)?;
    let result = if req.parameters.is_empty() {
        graph.execute(&req.statement, req.read_only)
    } else {
        graph.execute_with_params(
            &req.statement,
            to_parameters(&req.parameters)?,
            req.read_only,
        )
    }
    .map_err(map_engine)?;
    Ok(to_pb_response(&result))
}

/// `ExecuteBatch`: runs statements as one transaction (`atomic`) or one transaction each.
pub fn execute_batch(
    engine: &Engine,
    req: pb::ExecuteBatchRequest,
) -> Result<pb::ExecuteBatchResponse, ConnectError> {
    check_language(req.language.as_known(), req.language)?;
    let graph = find(engine, &req.namespace, &req.graph)?;
    let statements = req
        .statements
        .iter()
        .map(|statement| {
            Ok(BatchStatement {
                text: statement.statement.clone(),
                parameters: to_parameters(&statement.parameters)?,
            })
        })
        .collect::<Result<Vec<_>, ConnectError>>()?;
    if req.atomic {
        // Grafeo's own transaction, not a Loams-side emulation.
        let results = graph.execute_batch(&statements).map_err(map_engine)?;
        Ok(pb::ExecuteBatchResponse {
            results: results.iter().map(to_pb_response).collect(),
            committed: true,
            committed_through: u32::try_from(results.len()).unwrap_or(u32::MAX),
            ..Default::default()
        })
    } else {
        // Each statement is its own transaction; a failure part-way leaves its predecessors
        // committed. Parameters of a non-atomic batch are bound from Task 3
        // (`non_atomic_batch_binds_parameters`).
        let mut results = Vec::with_capacity(statements.len());
        for statement in &statements {
            results.push(graph.execute(&statement.text, false).map_err(map_engine)?);
        }
        Ok(pb::ExecuteBatchResponse {
            results: results.iter().map(to_pb_response).collect(),
            committed: false,
            committed_through: u32::try_from(results.len()).unwrap_or(u32::MAX),
            ..Default::default()
        })
    }
}

/// Finds an open graph, or answers `NotFound`.
fn find(
    engine: &Engine,
    namespace: &str,
    name: &str,
) -> Result<Arc<crate::engine::Graph>, ConnectError> {
    engine
        .list(Some(namespace))
        .map_err(internal)?
        .into_iter()
        .find(|g| g.name() == name)
        .ok_or_else(|| {
            ConnectError::new(
                ErrorCode::NotFound,
                format!("no graph {namespace}/{name} is open"),
            )
        })
}
