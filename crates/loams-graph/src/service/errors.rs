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
        | GraphError::TooComplex { .. } => ErrorCode::InvalidArgument,
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
