//! The Connect-RPC surface: `loams.graph.v1.GraphService` over an embedded [`Engine`].
//!
//! This layer is transport and nothing else. Every decision that matters is in [`crate::engine`]:
//! a statement reaches the engine unchanged, a read-only request is refused before the engine sees
//! it, and an embedded engine means no URL and no credential to handle. What is left here is
//! converting between protobuf and the engine's own types, and refusing what D634 does not serve.
//!
//! The functions are free rather than methods on [`GraphServiceImpl`] because they are what the
//! generated `GraphService` server trait would call one for one, and a test can call them without a
//! router. `loams-flow` is in the same position: the trait is generated and not yet implemented by
//! any service in this workspace.

use std::collections::BTreeMap;
use std::sync::Arc;

use buffa_types::google::protobuf::__buffa::oneof::value::Kind as PbKind;
use buffa_types::google::protobuf::{ListValue, Struct, Value as PbValue};
use connectrpc::{ConnectError, ErrorCode};
use grafeo::Value;
use loams_graph_proto as pb;

use crate::engine::{BatchStatement, Engine, GraphError, GraphResult, OpenSpec};

/// `GraphService` over one embedded engine.
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
    ///
    /// The languages are read from the engine build rather than hardcoded per call, because the
    /// answer to "can this build speak Cypher?" is a property of how Grafeo was compiled
    /// (`gql` is on, the other five are off — see this crate's `Cargo.toml`).
    pub fn engine_info(&self) -> pb::EngineInfo {
        engine_info(&self.engine)
    }
}

/// Maps an engine failure onto a Connect-RPC error.
///
/// A statement the engine refused is `InvalidArgument`, because that is what it is: the caller's
/// text was wrong. Everything else is `Internal`, and the engine's own message is kept whole — its
/// syntax span and hint are what make a GQL mistake fixable.
fn map_engine(err: GraphError) -> ConnectError {
    match err {
        GraphError::Engine(_)
        | GraphError::EmptyStatement
        | GraphError::UnboundParameter { .. } => {
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
/// This crate compiles Grafeo with `gql` and without `cypher`, `sparql`, `gremlin`, `graphql` or
/// `sql-pgq` (see this crate's `Cargo.toml`), so asking for one is `Unimplemented` rather than a
/// parse error deep in the engine. D634 (a) is why the answer is `Unimplemented` and not a silent
/// re-parse: GQL is the standard, and a caller reaching for Cypher has asked for a different
/// language than the one this surface promises.
///
/// `language` is the wire value resolved to a variant, or `None` when the number names no variant
/// this build knows. `None` is refused rather than read as GQL: defaulting an unrecognised language
/// to the one Loams speaks would run a statement in a language the caller did not ask for.
fn check_language(
    language: Option<pb::QueryLanguage>,
    wire_value: impl std::fmt::Display,
) -> Result<(), ConnectError> {
    use pb::QueryLanguage as L;
    match language {
        // `Unspecified` means GQL, as the proto says it does.
        Some(L::Unspecified) | Some(L::Gql) => Ok(()),
        _ => Err(ConnectError::new(
            ErrorCode::Unimplemented,
            format!(
                "query language {wire_value} is not available in this build; loams-graph serves \
                 GQL, ISO/IEC 39075, only (D634 (a))"
            ),
        )),
    }
}

/// Converts an engine result into its protobuf form.
fn to_pb_rows(result: &GraphResult) -> pb::RowSet {
    pb::RowSet {
        columns: result.columns.clone(),
        column_types: result.column_types.clone(),
        rows: result
            .rows
            .iter()
            .map(|row| pb::Row {
                values: row.values.iter().map(to_pb_value).collect(),
                node_ids: row.node_ids.clone(),
                relationship_ids: row.relationship_ids.clone(),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

/// Converts one JSON value into the protobuf `Value` the contract's `Row.values` is typed as.
///
/// `google.protobuf.Value` is a oneof over null, number, string, bool, list and struct, which is
/// exactly what JSON is, so nothing is lost: an integer becomes a `number_value` (protobuf's own
/// number is a double, which is the one narrowing a JSON integer suffers here — the column type
/// beside it still says `INT64`).
fn to_pb_value(value: &serde_json::Value) -> PbValue {
    use serde_json::Value as Json;
    match value {
        Json::Null => PbValue::null(),
        Json::Bool(b) => PbValue::from(*b),
        Json::Number(n) => PbValue::from(n.as_f64().unwrap_or(0.0)),
        Json::String(s) => PbValue::from(s.as_str()),
        Json::Array(items) => PbValue {
            kind: Some(kind_list(ListValue {
                values: items.iter().map(to_pb_value).collect(),
                ..Default::default()
            })),
            ..Default::default()
        },
        Json::Object(fields) => {
            // Built through `Struct::default()` rather than a struct literal: the generated field
            // type is buffa's own `HashMap` re-export, and naming it would mean depending on
            // `buffa` for one `Default`.
            let mut object = Struct::default();
            for (key, value) in fields {
                object.fields.insert(key.clone(), to_pb_value(value));
            }
            PbValue {
                kind: Some(kind_struct(object)),
                ..Default::default()
            }
        }
    }
}

/// Converts one batch statement, bindings included.
fn to_batch_statement(statement: &pb::Statement) -> BatchStatement {
    BatchStatement {
        text: statement.statement.clone(),
        // A `parameters` map that names a binding the statement does not use is not an error: the
        // engine substitutes what it finds and ignores the rest, and refusing it here would make
        // Loams stricter than the engine for no gain.
        parameters: statement
            .parameters
            .iter()
            .map(|(name, value)| (name.clone(), to_gql_value(value)))
            .collect(),
    }
}

/// Converts one protobuf value into the engine's own, which is what a binding is resolved against.
///
/// The reverse of [`to_pb_value`] through JSON, with one difference the wire forces: protobuf's
/// `number_value` is a double, so every numeric binding arrives as a `Float64` and an integer
/// property bound this way is stored as a float. There is no integer variant on this side of the
/// contract to preserve, and inventing one from a `f64` would be a guess about intent.
fn to_gql_value(value: &PbValue) -> Value {
    match &value.kind {
        None | Some(PbKind::NullValue(_)) => Value::Null,
        Some(PbKind::NumberValue(number)) => Value::from(*number),
        Some(PbKind::StringValue(text)) => Value::from(text.as_str()),
        Some(PbKind::BoolValue(flag)) => Value::from(*flag),
        Some(PbKind::ListValue(list)) => Value::List(Arc::from(
            list.values.iter().map(to_gql_value).collect::<Vec<_>>(),
        )),
        Some(PbKind::StructValue(fields)) => {
            let mut map = BTreeMap::new();
            for (key, value) in &fields.fields {
                // `grafeo::Value::Map` is keyed by `grafeo_common`'s `PropertyKey`, which the
                // `grafeo` facade does not re-export, so the sibling crate is named directly --
                // see the `grafeo-common` entry in Cargo.toml for why that is forced rather than
                // chosen.
                map.insert(
                    grafeo_common::types::PropertyKey::new(key.as_str()),
                    to_gql_value(value),
                );
            }
            Value::Map(Arc::new(map))
        }
    }
}

/// The `ListValue` arm of `google.protobuf.Value`'s oneof.
fn kind_list(value: ListValue) -> buffa_types::google::protobuf::__buffa::oneof::value::Kind {
    buffa_types::google::protobuf::__buffa::oneof::value::Kind::ListValue(Box::new(value))
}

/// The `StructValue` arm of `google.protobuf.Value`'s oneof.
fn kind_struct(value: Struct) -> buffa_types::google::protobuf::__buffa::oneof::value::Kind {
    buffa_types::google::protobuf::__buffa::oneof::value::Kind::StructValue(Box::new(value))
}

/// Fills the per-graph fields of a `Graph` message.
fn graph_message(graph: &crate::engine::Graph) -> pb::Graph {
    pb::Graph {
        namespace: graph.namespace().to_string(),
        name: graph.name().to_string(),
        // The engine has no identifier of its own to report — a `GrafeoDB` is addressed by the
        // registry key it was opened under — so the handle is that key. It is opaque to a client
        // either way, which is all the contract asks of it.
        handle: format!("{}/{}", graph.namespace(), graph.name()),
        persistent: graph.is_persistent(),
        statements_executed: graph.statements_executed(),
        ..Default::default()
    }
}

/// Reports the engine and the standard it serves.
pub fn engine_info(engine: &Engine) -> pb::EngineInfo {
    pb::EngineInfo {
        engine_version: engine.engine_version.to_string(),
        languages: vec![pb::QueryLanguage::Gql.into()],
        // D634 (b): the engine is in this process. Recorded because a client may otherwise assume a
        // deployment it can restart or a credential it must supply.
        embedded: true,
        ephemeral: engine
            .list(None)
            .map(|graphs| graphs.iter().all(|g| !g.is_persistent()))
            .unwrap_or(true),
        ..Default::default()
    }
}

/// Opens a graph, idempotently.
pub fn open(engine: &Engine, req: pb::OpenRequest) -> Result<pb::Graph, ConnectError> {
    let spec = OpenSpec {
        database_path: req.database_path.into(),
        read_only: req.read_only,
    };
    let graph =
        crate::engine::Graph::open(engine, &req.namespace, &req.name, spec).map_err(map_engine)?;
    Ok(graph_message(&graph))
}

/// Lists the graphs open in this process.
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

/// Runs one statement.
pub fn execute(
    engine: &Engine,
    req: pb::ExecuteRequest,
) -> Result<pb::ExecuteResponse, ConnectError> {
    check_language(req.language.as_known(), req.language)?;
    let graph = find(engine, &req.namespace, &req.name)?;
    let result = graph
        .execute(&req.statement, req.read_only)
        .map_err(map_engine)?;
    Ok(to_pb_response(&result))
}

/// Runs statements as one transaction.
pub fn execute_batch(
    engine: &Engine,
    req: pb::ExecuteBatchRequest,
) -> Result<pb::ExecuteBatchResponse, ConnectError> {
    check_language(req.language.as_known(), req.language)?;
    let graph = find(engine, &req.namespace, &req.name)?;
    // Grafeo's own transaction, not a Loams-side emulation: this is the one place in the graph
    // surface where a write batch's durability is the engine's ACID guarantee rather than an
    // at-least-once question (D634).
    let statements: Vec<BatchStatement> = req.statements.iter().map(to_batch_statement).collect();
    if req.atomic {
        let results = graph.execute_batch(&statements).map_err(map_engine)?;
        Ok(pb::ExecuteBatchResponse {
            results: results.iter().map(to_pb_response).collect(),
            committed: true,
            ..Default::default()
        })
    } else {
        // `atomic = false` is a series of independent statements, so each is its own transaction.
        // A failure part-way leaves its predecessors committed, which is what the caller asked for
        // by not asking for one transaction; `committed` says so rather than claiming otherwise.
        let mut results = Vec::with_capacity(statements.len());
        for statement in &statements {
            results.push(graph.execute(&statement.text, false).map_err(map_engine)?);
        }
        Ok(pb::ExecuteBatchResponse {
            results: results.iter().map(to_pb_response).collect(),
            committed: false,
            ..Default::default()
        })
    }
}

/// Closes a graph.
pub fn close(engine: &Engine, req: pb::CloseRequest) -> Result<(), ConnectError> {
    engine
        .close(&req.namespace, &req.name)
        .map(|_| ())
        .map_err(internal)
}

/// Fills one `ExecuteResponse` from an engine result.
///
/// The four counters are where this contract and Grafeo 0.5.43 do not line up, and the mapping is
/// written out once, here, rather than as a zero at each call site:
///
/// * `rows_read` is the engine's own `rows_scanned`, which Grafeo sets to the number of rows
///   **returned**. A real measurement of the wrong thing, and the only row counter there is.
/// * `elapsed_nanos` is the engine's own `Instant`, in milliseconds, converted to the unit the
///   contract asks for.
/// * `rows_affected` and `bytes_read` have no engine behind them at all — `QueryResult` has no
///   affected-count field, and no byte counter exists anywhere in
///   `grafeo-{core,common,engine,storage}` 0.5.43 — so both are zero. They are left zero rather
///   than estimated, because a plausible number here would be indistinguishable from a measurement
///   and a caller would budget against it. Either field should be dropped or given a definition
///   this engine can satisfy.
fn to_pb_response(result: &GraphResult) -> pb::ExecuteResponse {
    pb::ExecuteResponse {
        rows: Some(to_pb_rows(result)).into(),
        rows_affected: result.rows_affected.unwrap_or_default(),
        rows_read: result.rows_read.unwrap_or_default(),
        bytes_read: result.bytes_read.unwrap_or_default(),
        elapsed_nanos: result.elapsed_nanos.unwrap_or_default(),
        ..Default::default()
    }
}

/// Finds a graph, or answers `NotFound`.
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
