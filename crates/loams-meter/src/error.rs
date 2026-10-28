//! The meter role's errors and their reasons (`ErrorInfo.reason`, §49
//! §18.2; §63 §17.4).

use loams_meter_core::RefusalReason;

/// An error from a meter service, with the reason clients branch on.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MeterError {
    /// No caller identity on the request.
    #[error("unauthenticated")]
    Unauthenticated,
    /// The caller may not do this; `reason` says which rule refused it.
    #[error("{message}")]
    PermissionDenied {
        reason: &'static str,
        message: String,
    },
    #[error("{field}: {message}")]
    InvalidArgument { field: String, message: String },
    #[error("{what} {id} not found")]
    NotFound { what: &'static str, id: String },
    #[error("{what} {id} already exists")]
    AlreadyExists { what: &'static str, id: String },
    /// A rule of the meter refuses the change (`payment_method_required`,
    /// `spend_cap_required`, `meter_semantics_immutable`, …).
    #[error("{message}")]
    FailedPrecondition {
        reason: &'static str,
        message: String,
    },
    /// Out of allowance (§63 §17.4).
    #[error("{message}")]
    Refused {
        reason: RefusalReason,
        message: String,
    },
    #[error("unavailable: {0}")]
    Unavailable(String),
    #[error("{0} is not implemented yet")]
    Unimplemented(&'static str),
}

pub type Result<T, E = MeterError> = std::result::Result<T, E>;

impl MeterError {
    pub fn invalid(field: impl Into<String>, message: impl Into<String>) -> Self {
        MeterError::InvalidArgument {
            field: field.into(),
            message: message.into(),
        }
    }

    pub fn denied(reason: &'static str, message: impl Into<String>) -> Self {
        MeterError::PermissionDenied {
            reason,
            message: message.into(),
        }
    }

    pub fn precondition(reason: &'static str, message: impl Into<String>) -> Self {
        MeterError::FailedPrecondition {
            reason,
            message: message.into(),
        }
    }

    pub fn not_found(what: &'static str, id: impl Into<String>) -> Self {
        MeterError::NotFound {
            what,
            id: id.into(),
        }
    }

    pub fn exists(what: &'static str, id: impl Into<String>) -> Self {
        MeterError::AlreadyExists {
            what,
            id: id.into(),
        }
    }

    /// The `ErrorInfo.reason` of this error.
    pub fn reason(&self) -> &'static str {
        match self {
            MeterError::Unauthenticated => "unauthenticated",
            MeterError::PermissionDenied { reason, .. } => reason,
            MeterError::InvalidArgument { .. } => "invalid_argument",
            MeterError::NotFound { .. } => "not_found",
            MeterError::AlreadyExists { .. } => "already_exists",
            MeterError::FailedPrecondition { reason, .. } => reason,
            MeterError::Refused { reason, .. } => reason.reason(),
            MeterError::Unavailable(_) => "meter_unavailable",
            MeterError::Unimplemented(_) => "not_implemented",
        }
    }
}
