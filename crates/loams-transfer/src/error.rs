//! The errors every transfer and run operation reports. Each one has a stable
//! machine code, which the HTTP layer sends as `{"code": ...}`.

use loams_store::StoreError;

use crate::checkpoint::CheckpointError;

/// The result of a transfer or run operation.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Why a transfer or run operation failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The transfer, run, chunk or namespace does not exist.
    #[error("not found: {0}")]
    NotFound(String),
    /// The request is malformed or out of bounds.
    #[error("bad request: {0}")]
    BadRequest(String),
    /// Another holder has the lease until `deadline_ms` (D2429).
    #[error("lease held by {holder} until {deadline_ms}")]
    LeaseHeld { holder: String, deadline_ms: u64 },
    /// The caller's lease epoch is no longer current (D2430).
    #[error("lease lost")]
    LeaseLost,
    /// A transfer with this id exists with another spec, or a chunk was
    /// committed with another hash (D2426, D2427).
    #[error("conflict ({code}): {message}")]
    Conflict { code: &'static str, message: String },
    /// A log append was not at the committed length; resend from `committed`
    /// (D2441).
    #[error("log offset mismatch: committed length is {committed}")]
    Offset { committed: u64 },
    /// The bytes received do not match the declared hash or length.
    #[error("integrity: {0}")]
    Integrity(String),
    /// The transfer or run is not in a state that allows this.
    #[error("state: {0}")]
    State(String),
    /// The bucket failed.
    #[error("store: {0}")]
    Store(#[from] StoreError),
    /// The metastore failed (not a rejection this crate maps).
    #[error("metastore: {0}")]
    Meta(String),
    /// A stored object could not be decoded.
    #[error("corrupt: {0}")]
    Corrupt(String),
}

impl Error {
    /// The stable machine code sent to clients.
    pub fn code(&self) -> &'static str {
        match self {
            Error::NotFound(_) => "not_found",
            Error::BadRequest(_) => "bad_request",
            Error::LeaseHeld { .. } => "lease_held",
            Error::LeaseLost => "lease_lost",
            Error::Conflict { code, .. } => code,
            Error::Offset { .. } => "offset_mismatch",
            Error::Integrity(_) => "integrity",
            Error::State(_) => "state",
            Error::Store(_) => "store",
            Error::Meta(_) => "metastore",
            Error::Corrupt(_) => "corrupt",
        }
    }

    /// Whether a client should retry the same request after a pause.
    pub fn is_transient(&self) -> bool {
        match self {
            Error::Store(e) => e.is_retryable(),
            Error::Meta(_) => true,
            _ => false,
        }
    }

    pub(crate) fn conflict(code: &'static str, message: impl Into<String>) -> Self {
        Error::Conflict {
            code,
            message: message.into(),
        }
    }
}

impl From<CheckpointError> for Error {
    fn from(e: CheckpointError) -> Self {
        match e {
            CheckpointError::LeaseHeld { owner, deadline_ms } => Error::LeaseHeld {
                holder: owner,
                deadline_ms,
            },
            CheckpointError::LeaseLost | CheckpointError::Fenced => Error::LeaseLost,
            CheckpointError::NamespaceNotFound => Error::NotFound("namespace".into()),
            CheckpointError::VersionMismatch { .. } => {
                Error::Meta("pointer changed concurrently".into())
            }
            CheckpointError::Other(m) => Error::Meta(m),
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Corrupt(e.to_string())
    }
}
