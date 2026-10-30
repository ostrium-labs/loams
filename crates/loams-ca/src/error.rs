//! What the CA service answers when it refuses (`docs/api/reasons.md`, the
//! `loams.ca.internal.v1` rows).

use connectrpc::{ConnectError, ErrorCode, ErrorDetail};
use loams_proto::loams::errors::v1::ErrorInfo;
use loams_transport::{CaError, TokenError};

/// Why a CA operation was refused or failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CaServiceError {
    /// `Renew` without a verified client certificate.
    #[error("a client certificate is required")]
    Unauthenticated,
    #[error(transparent)]
    Token(TokenError),
    /// The CSR asks for an identity the grant does not hold.
    #[error("{0} is not granted")]
    NotGranted(String),
    #[error("{0} is revoked")]
    Revoked(String),
    #[error("the CA is not initialized: run `loams ca init`")]
    NotInitialized,
    /// This node has no key to open the intermediate's sealed key.
    #[error("this node cannot open the CA's intermediate key: {0}")]
    Sealed(String),
    #[error("the CA is already initialized")]
    AlreadyInitialized,
    #[error("{field}: {message}")]
    Invalid {
        field: &'static str,
        message: String,
    },
    #[error("the CA's state is unavailable: {0}")]
    Unavailable(String),
    #[error("{0}")]
    Internal(String),
}

impl CaServiceError {
    /// The `ErrorInfo.reason`.
    pub fn reason(&self) -> &'static str {
        match self {
            CaServiceError::Unauthenticated => "unauthenticated",
            CaServiceError::Token(TokenError::Malformed) | CaServiceError::Invalid { .. } => {
                "invalid_argument"
            }
            CaServiceError::Token(TokenError::Spent) => "join_token_spent",
            CaServiceError::Token(TokenError::Expired) => "join_token_expired",
            CaServiceError::Token(TokenError::Unknown) => "join_token_unknown",
            CaServiceError::Token(TokenError::Random) | CaServiceError::Internal(_) => "internal",
            CaServiceError::NotGranted(_) => "identity_not_granted",
            CaServiceError::Revoked(_) => "identity_revoked",
            CaServiceError::NotInitialized | CaServiceError::Sealed(_) => "ca_not_initialized",
            CaServiceError::AlreadyInitialized => "failed_precondition",
            CaServiceError::Unavailable(_) => "unavailable",
        }
    }

    pub fn code(&self) -> ErrorCode {
        match self {
            CaServiceError::Unauthenticated
            | CaServiceError::Token(
                TokenError::Spent | TokenError::Expired | TokenError::Unknown,
            ) => ErrorCode::Unauthenticated,
            CaServiceError::Token(TokenError::Malformed) | CaServiceError::Invalid { .. } => {
                ErrorCode::InvalidArgument
            }
            CaServiceError::NotGranted(_) | CaServiceError::Revoked(_) => {
                ErrorCode::PermissionDenied
            }
            CaServiceError::NotInitialized
            | CaServiceError::Sealed(_)
            | CaServiceError::AlreadyInitialized => ErrorCode::FailedPrecondition,
            CaServiceError::Unavailable(_) => ErrorCode::Unavailable,
            CaServiceError::Token(TokenError::Random) | CaServiceError::Internal(_) => {
                ErrorCode::Internal
            }
        }
    }

    /// As a Connect error with one `loams.errors.v1.ErrorInfo`.
    pub fn to_connect(&self) -> ConnectError {
        let mut metadata = Vec::new();
        match self {
            CaServiceError::NotGranted(id) => metadata.push(("identity".to_string(), id.clone())),
            CaServiceError::Invalid { field, .. } => {
                metadata.push(("field".to_string(), (*field).to_string()));
            }
            _ => {}
        }
        let info = ErrorInfo {
            reason: self.reason().to_string(),
            metadata: metadata.into_iter().collect(),
            ..Default::default()
        };
        // An internal error's details go to the log, not to the caller.
        let message = match self {
            CaServiceError::Internal(detail) => {
                tracing::error!(%detail, "the CA service failed");
                "internal error".to_string()
            }
            other => other.to_string(),
        };
        ConnectError::new(self.code(), message).with_detail(ErrorDetail::from_message(
            "loams.errors.v1.ErrorInfo",
            &info,
        ))
    }
}

impl From<CaError> for CaServiceError {
    fn from(e: CaError) -> Self {
        match e {
            CaError::NotGranted(id) => CaServiceError::NotGranted(id),
            CaError::WrongCluster(cluster) => {
                CaServiceError::NotGranted(format!("an identity in cluster {cluster:?}"))
            }
            CaError::NoIdentity | CaError::BadUri(_) | CaError::Csr(_) => CaServiceError::Invalid {
                field: "csr",
                message: e.to_string(),
            },
            CaError::KeyMismatch => CaServiceError::Invalid {
                field: "root_key",
                message: e.to_string(),
            },
            CaError::Rcgen(_) => CaServiceError::Internal(e.to_string()),
        }
    }
}
