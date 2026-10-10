//! [`LiveError`], the error of every Loams Live operation, and its wire code.

use loams_kv::TxnError;

use crate::pb;

/// Why a Loams Live operation failed. Each variant maps to one
/// [`pb::ErrorCode`], which is what clients see (design §20 §7.1).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LiveError {
    /// The request is malformed: a bad value, name, id or range.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// A pagination cursor that is malformed, forged, or from another app
    /// or index (LV1 plan Task 4): `INVALID_ARGUMENT`, reason
    /// `live_bad_cursor`.
    #[error("invalid argument: {0}")]
    BadCursor(String),
    /// A document, table or index does not exist.
    #[error("not found: {0}")]
    NotFound(String),
    /// The request is valid but the state refuses it (for example an index
    /// change on a non-empty table, R1 plan Ruling 5).
    #[error("failed precondition: {0}")]
    FailedPrecondition(String),
    /// A limit of [`Limits`](crate::Limits) was exceeded. `limit` is the
    /// field name of the limit.
    #[error("limit {limit} exceeded: {message}")]
    LimitExceeded {
        limit: &'static str,
        message: String,
    },
    /// A journal consumer's position is below what the janitor has kept:
    /// the entries after `position` in `shard` are gone (`first` is the
    /// oldest one left, if any). The consumer must resynchronise (rerun
    /// everything) and start again from the current heads.
    #[error(
        "journal shard {shard} was trimmed past position {position} (oldest entry left: {first:?})"
    )]
    JournalTrimmed {
        shard: u16,
        position: u64,
        first: Option<u64>,
    },
    /// The Live listener was asked to bind a non-loopback address, which R1
    /// refuses (D111, §20 §7.1; [`check_listen`](crate::check_listen)).
    #[error(
        "--live-listen {0} is not a loopback address; the Live API has no authentication \
         until the unified auth plan (D111)"
    )]
    NotLoopback(std::net::SocketAddr),
    /// `tenancy = "multi"` without isolated workers (design §45 §3.1, LV1
    /// plan Task 5): strangers' code never runs in the server's process.
    #[error(r#"live: tenancy = "multi" needs isolation = "isolated" (Linux only)"#)]
    IsolationRequired,
    /// `isolation = "isolated"` where the worker sandbox does not exist.
    #[error(r#"live: isolation = "isolated" needs Linux (seccomp and landlock)"#)]
    IsolationUnavailable,
    /// A stored record could not be decoded.
    #[error("corrupt record: {0}")]
    Corrupt(String),
    /// A failure of the server itself (the OS random source, a bug).
    #[error("internal error: {0}")]
    Internal(String),
    /// A deployed function threw, or its bundle is invalid (LV1 plan
    /// Task 3). The text is the function's own error message.
    #[error("function error: {0}")]
    FunctionError(String),
    /// A deployed function ran past its JavaScript CPU limit.
    #[error("function {function} ran past its CPU limit of {limit:?}")]
    FunctionTimeout {
        function: String,
        limit: std::time::Duration,
    },
    /// The isolated worker running the call crashed or was killed (LV1
    /// plan Task 5): `FUNCTION_ERROR`, reason `live_worker_crashed`. The
    /// call's transaction never committed; a mutation is not retried.
    #[error("function error: {0}")]
    WorkerCrashed(String),
    /// A deployed function ran past its runtime's memory limit.
    #[error("function {function} ran past its memory limit of {limit} bytes")]
    FunctionOutOfMemory { function: String, limit: usize },
    /// A store transaction or read failed. Inside [`Store::run`] bodies this
    /// carries the runner's retry signals ([`TxnError::Conflict`],
    /// [`TxnError::NotApplied`]); [`LiveError::into_txn`] hands them back.
    ///
    /// [`Store::run`]: loams_kv::Store::run
    #[error("storage: {0}")]
    Txn(#[from] TxnError),
}

impl LiveError {
    /// The wire code of this error.
    pub fn code(&self) -> pb::ErrorCode {
        match self {
            LiveError::InvalidArgument(_)
            | LiveError::BadCursor(_)
            | LiveError::NotLoopback(_)
            | LiveError::IsolationRequired
            | LiveError::IsolationUnavailable => pb::ErrorCode::ERROR_CODE_INVALID_ARGUMENT,
            LiveError::NotFound(_) => pb::ErrorCode::ERROR_CODE_NOT_FOUND,
            LiveError::FailedPrecondition(_) | LiveError::JournalTrimmed { .. } => {
                pb::ErrorCode::ERROR_CODE_FAILED_PRECONDITION
            }
            LiveError::LimitExceeded { .. } => pb::ErrorCode::ERROR_CODE_RESOURCE_EXHAUSTED,
            LiveError::Corrupt(_) | LiveError::Internal(_) => pb::ErrorCode::ERROR_CODE_INTERNAL,
            LiveError::FunctionError(_) | LiveError::WorkerCrashed(_) => {
                pb::ErrorCode::ERROR_CODE_FUNCTION_ERROR
            }
            LiveError::FunctionTimeout { .. } => pb::ErrorCode::ERROR_CODE_FUNCTION_TIMEOUT,
            LiveError::FunctionOutOfMemory { .. } => {
                pb::ErrorCode::ERROR_CODE_FUNCTION_OUT_OF_MEMORY
            }
            LiveError::Txn(e) => match e {
                TxnError::Conflict
                | TxnError::NotApplied(_)
                | TxnError::Undetermined { .. }
                | TxnError::Deadline => pb::ErrorCode::ERROR_CODE_UNAVAILABLE,
                TxnError::AlreadyExists(_) | TxnError::Fatal(_) => {
                    pb::ErrorCode::ERROR_CODE_INTERNAL
                }
            },
        }
    }

    /// The stable `ErrorInfo.reason` of this error, where it has one
    /// (`docs/api/reasons.md`); the other errors are told apart by their
    /// code.
    pub fn reason(&self) -> Option<&'static str> {
        match self {
            LiveError::BadCursor(_) => Some("live_bad_cursor"),
            LiveError::WorkerCrashed(_) => Some("live_worker_crashed"),
            _ => None,
        }
    }

    /// The wire form of this error.
    pub fn to_proto(&self) -> pb::LiveError {
        pb::LiveError {
            code: self.code().into(),
            message: self.to_string(),
            ..Default::default()
        }
    }

    /// Splits a storage error off: `Err(txn_error)` for [`LiveError::Txn`],
    /// so a [`Store::run`](loams_kv::Store::run) body can return it and let
    /// the runner retry, and `Ok(self)` for every other error, which the body
    /// returns inside its value so the runner does not retry it.
    pub fn into_txn(self) -> Result<LiveError, TxnError> {
        match self {
            LiveError::Txn(e) => Err(e),
            other => Ok(other),
        }
    }

    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        LiveError::InvalidArgument(message.into())
    }

    pub(crate) fn limit(limit: &'static str, message: impl Into<String>) -> Self {
        LiveError::LimitExceeded {
            limit,
            message: message.into(),
        }
    }
}
