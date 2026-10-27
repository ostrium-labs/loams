//! The one error type of the facade and the adapters.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    #[error("{0}")]
    NotFound(String),
    /// The request is malformed or names something the engine does not accept.
    #[error("{0}")]
    Invalid(String),
    /// The name is taken, or another operation holds the instance.
    #[error("{0}")]
    Conflict(String),
    /// The instance is in a phase that does not allow the operation.
    #[error("{0}")]
    FailedPrecondition(String),
    /// The engine does not do this (Spark sessions on RisingWave, suspend without support).
    #[error("{0}")]
    Unsupported(String),
    /// No adapter serves this engine kind on this install (its profile is off).
    #[error("{0}")]
    NotEnabled(String),
    /// The Meter refused: an entitlement is used up or a spend cap reached (design 63 §17.4).
    #[error("{0}")]
    QuotaExceeded(String),
    /// The runtime could not be reached; retry.
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    Internal(String),
}

impl EngineError {
    /// The stable code in the `{error, message}` body.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "not_found",
            Self::Invalid(_) => "invalid_argument",
            Self::Conflict(_) => "already_exists",
            Self::FailedPrecondition(_) => "failed_precondition",
            Self::Unsupported(_) => "unsupported",
            Self::NotEnabled(_) => "engine_not_enabled",
            Self::QuotaExceeded(_) => "entitlement_exhausted",
            Self::Unavailable(_) => "unavailable",
            Self::Internal(_) => "internal",
        }
    }

    pub fn status(&self) -> StatusCode {
        match self {
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Invalid(_) | Self::Unsupported(_) => StatusCode::BAD_REQUEST,
            Self::Conflict(_) | Self::FailedPrecondition(_) => StatusCode::CONFLICT,
            Self::NotEnabled(_) => StatusCode::NOT_IMPLEMENTED,
            Self::QuotaExceeded(_) => StatusCode::PAYMENT_REQUIRED,
            Self::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Whether a failed create should keep the record (in `failed`) rather than drop it: the
    /// spec was fine and the runtime failed, so the person sees what happened.
    pub fn keeps_record(&self) -> bool {
        matches!(self, Self::Unavailable(_) | Self::Internal(_))
    }
}

impl IntoResponse for EngineError {
    fn into_response(self) -> Response {
        if matches!(self, Self::Internal(_)) {
            tracing::error!(target: "loams_engines", error = %self, "internal error");
        }
        let body = serde_json::json!({ "error": self.code(), "message": self.to_string() });
        (self.status(), Json(body)).into_response()
    }
}

pub type Result<T, E = EngineError> = std::result::Result<T, E>;
