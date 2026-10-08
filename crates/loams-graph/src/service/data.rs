//! `GraphService`: statements against one graph (GR1 Task 3).
//!
//! Every statement is gated ([`crate::classify::gate`]) before it runs and then runs, unchanged,
//! on a Grafeo session whose role matches what it needs. Parameters are bound by the engine on
//! every path; a value is never interpolated into a statement.

use std::collections::HashMap;

use connectrpc::{ConnectError, ErrorCode};
use loams_proto::loams::graph::v1 as pb;

use super::errors::{code_of, error_info, map_engine, refuse};
use super::find_serving;
use crate::classify::{Access, gate};
use crate::engine::{BatchStatement, Engine, GraphResult};
use crate::value::{from_proto, to_proto};

/// Refuses a query language this build does not have.
///
/// This build compiles Grafeo with `gql` only (root `Cargo.toml`, R0.2), so asking for another is
/// `Unimplemented` rather than a parse error deep in the engine. A wire number that names no
/// variant is refused rather than read as GQL: running a statement in a language the caller did
/// not ask for is worse than refusing it.
pub(crate) fn check_language(
    language: Option<pb::QueryLanguage>,
    wire_value: impl std::fmt::Display,
) -> Result<(), ConnectError> {
    use pb::QueryLanguage as L;
    match language {
        Some(L::Unspecified) | Some(L::Gql) => Ok(()),
        _ => Err(refuse(
            ErrorCode::Unimplemented,
            "graph_language_disabled",
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
                    refuse(
                        ErrorCode::InvalidArgument,
                        "invalid_argument",
                        format!("parameter ${name}: {err}"),
                    )
                })
        })
        .collect()
}

/// `Execute`: runs one statement. Parameters are bound by the engine, never interpolated.
pub fn execute(
    engine: &Engine,
    req: pb::ExecuteRequest,
) -> Result<pb::ExecuteResponse, ConnectError> {
    check_language(req.language.as_known(), req.language)?;
    let graph = find_serving(engine, &req.namespace, &req.graph)?;
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
///
/// The contract is `ExecuteBatchResponse`'s: an atomic batch fails as a whole; a non-atomic one
/// stops at the first failing statement and answers the committed results, `committed_through`
/// and a `StatementError` for the failed one.
pub fn execute_batch(
    engine: &Engine,
    req: pb::ExecuteBatchRequest,
) -> Result<pb::ExecuteBatchResponse, ConnectError> {
    // A statement's own language wins over the batch's (`Statement.language`); the batch's
    // applies to a statement that names none, and is checked when no statement names one (an
    // empty batch included) so a batch in a language this build lacks is still refused.
    let mut batch_language_used = req.statements.is_empty();
    for statement in &req.statements {
        if statement.language.as_known() == Some(pb::QueryLanguage::Unspecified) {
            batch_language_used = true;
        } else {
            check_language(statement.language.as_known(), statement.language)?;
        }
    }
    if batch_language_used {
        check_language(req.language.as_known(), req.language)?;
    }
    let graph = find_serving(engine, &req.namespace, &req.graph)?;
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
        // Grafeo's own transaction, not a Loams-side emulation. `commit_epoch` comes with the
        // write lane (Task 11, R0.5).
        let results = graph.execute_batch(&statements).map_err(map_engine)?;
        Ok(pb::ExecuteBatchResponse {
            results: results.iter().map(to_pb_response).collect(),
            committed: true,
            committed_through: u32::try_from(results.len()).unwrap_or(u32::MAX),
            ..Default::default()
        })
    } else {
        // Each statement is its own transaction, with its parameters bound; a failure stops the
        // batch and leaves its predecessors committed.
        let mut results = Vec::with_capacity(statements.len());
        let mut error = None;
        for (index, statement) in statements.iter().enumerate() {
            match graph.execute_with_params(&statement.text, statement.parameters.clone(), false) {
                Ok(result) => results.push(to_pb_response(&result)),
                Err(err) => {
                    error = Some(pb::StatementError {
                        index: u32::try_from(index).unwrap_or(u32::MAX),
                        code: code_of(&err).as_str().to_string(),
                        message: err.to_string(),
                        info: error_info(err.reason()).into(),
                        ..Default::default()
                    });
                    break;
                }
            }
        }
        Ok(pb::ExecuteBatchResponse {
            committed_through: u32::try_from(results.len()).unwrap_or(u32::MAX),
            results,
            committed: false,
            error: error.into(),
            ..Default::default()
        })
    }
}

/// `Explain`: the plan of a statement, or its profile.
///
/// The statement is gated like any other (file access, graph management and unbounded paths are
/// refused). A PROFILE runs the statement, so one that writes is refused with `graph_read_only`
/// before anything runs. The plan itself is Task 6's; until then a statement that passes is
/// answered `not_implemented`.
pub fn explain(engine: &Engine, req: pb::ExplainRequest) -> Result<pb::Plan, ConnectError> {
    check_language(req.language.as_known(), req.language)?;
    find_serving(engine, &req.namespace, &req.graph)?;
    let access = gate(&req.statement, pb::QueryLanguage::Gql).map_err(map_engine)?;
    if req.profile && access != Access::Read {
        return Err(refuse(
            ErrorCode::PermissionDenied,
            "graph_read_only",
            "PROFILE runs the statement, and this statement writes; use EXPLAIN for its plan",
        ));
    }
    Err(refuse(
        ErrorCode::Unimplemented,
        "not_implemented",
        "loams.graph.v1.GraphService/Explain is not implemented yet (GR1 Task 6)",
    ))
}
