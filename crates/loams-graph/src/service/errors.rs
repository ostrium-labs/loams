//! Connect errors for `loams.graph.v1`: a Connect code and one `loams.errors.v1.ErrorInfo` whose
//! `reason` is one of §48 §8.3's (GR1 Task 3).

use connectrpc::{ConnectError, ErrorCode, ErrorDetail};
use loams_proto::loams::errors::v1::ErrorInfo;

use crate::engine::GraphError;

/// A failed RPC: the Connect code and one `loams.errors.v1.ErrorInfo` carrying `reason`.
pub(crate) fn refuse(code: ErrorCode, reason: &str, message: impl Into<String>) -> ConnectError {
    ConnectError::new(code, message).with_detail(ErrorDetail::from_message(
        "loams.errors.v1.ErrorInfo",
        &error_info(reason),
    ))
}

/// [`refuse`], with `metadata` in the `ErrorInfo` (a quota's name, D65).
pub(crate) fn refuse_with(
    code: ErrorCode,
    reason: &str,
    message: impl Into<String>,
    metadata: &[(&str, &str)],
) -> ConnectError {
    let mut info = error_info(reason);
    for (key, value) in metadata {
        info.metadata
            .insert((*key).to_string(), (*value).to_string());
    }
    ConnectError::new(code, message).with_detail(ErrorDetail::from_message(
        "loams.errors.v1.ErrorInfo",
        &info,
    ))
}

pub(crate) fn error_info(reason: &str) -> ErrorInfo {
    ErrorInfo {
        reason: reason.to_owned(),
        ..Default::default()
    }
}

/// The Connect code of an engine failure; the reason is [`GraphError::reason`].
///
/// A statement the engine refused is `InvalidArgument`: the caller's text was wrong. The engine's
/// own message is kept whole, because its syntax span and hint are what make a GQL mistake
/// fixable.
pub(crate) fn code_of(err: &GraphError) -> ErrorCode {
    match err {
        GraphError::Engine(_)
        | GraphError::EmptyStatement
        | GraphError::UnboundParameter { .. }
        | GraphError::InvalidValue(_)
        | GraphError::TransactionStatement
        | GraphError::UnboundedPath { .. }
        | GraphError::TooComplex { .. }
        | GraphError::OverLimit(_)
        | GraphError::AllShortestPaths => ErrorCode::InvalidArgument,
        GraphError::ResultTooLarge { .. } | GraphError::RowTooLarge { .. } => {
            ErrorCode::ResourceExhausted
        }
        GraphError::ReadOnly
        | GraphError::StatementNotAllowed {
            file_access: true, ..
        } => ErrorCode::PermissionDenied,
        GraphError::StatementNotAllowed {
            file_access: false, ..
        } => ErrorCode::FailedPrecondition,
        GraphError::LanguageUnavailable(_) => ErrorCode::Unimplemented,
        GraphError::Conflict { .. } => ErrorCode::AlreadyExists,
        GraphError::EnginePanic => ErrorCode::Internal,
        GraphError::Reloading => ErrorCode::Unavailable,
        GraphError::StatementTimeout => ErrorCode::DeadlineExceeded,
        GraphError::Failed => ErrorCode::FailedPrecondition,
    }
}

/// Maps an engine failure onto a Connect-RPC error.
pub(crate) fn map_engine(err: GraphError) -> ConnectError {
    refuse(code_of(&err), err.reason(), err.to_string())
}

/// Maps a registry failure onto `INTERNAL`, never `InvalidArgument`.
pub(crate) fn internal(err: GraphError) -> ConnectError {
    refuse(ErrorCode::Internal, "internal", err.to_string())
}

/// Maps a catalog failure onto a Connect-RPC error.
pub(crate) fn map_catalog(err: crate::catalog::CatalogError) -> ConnectError {
    use crate::catalog::CatalogError as E;
    let (code, reason) = match &err {
        E::Invalid(_) => (ErrorCode::InvalidArgument, "invalid_argument"),
        E::NotFound { .. } => (ErrorCode::NotFound, "graph_not_found"),
        E::AlreadyExists { .. } => (ErrorCode::AlreadyExists, "already_exists"),
        E::VersionMismatch { .. } => (ErrorCode::Aborted, "graph_catalog_version_mismatch"),
        E::Unavailable(_) => (ErrorCode::Unavailable, "unavailable"),
        E::Corrupt(_) => (ErrorCode::Internal, "internal"),
    };
    // Backend detail (object keys, metastore errors) stays in the server log; a client gets a
    // generic message (review M7).
    let message = match &err {
        E::Unavailable(_) => {
            tracing::warn!(error = %err, "the graph catalog is unavailable");
            "the graph catalog is unavailable; retry".to_string()
        }
        E::Corrupt(_) => {
            tracing::error!(error = %err, "the graph catalog is corrupt");
            "internal error in the graph catalog".to_string()
        }
        _ => err.to_string(),
    };
    refuse(code, reason, message)
}
