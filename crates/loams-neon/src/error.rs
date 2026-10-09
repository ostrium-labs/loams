//! Errors from Neon's components and `loams-wal`, with their mapping to
//! `loams.errors.v1` reasons (`docs/api/reasons.md`).

use std::collections::HashMap;
use std::fmt;

use loams_proto::loams::errors::v1::ErrorInfo;

/// The component a call went to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Component {
    Pageserver,
    StorageController,
    /// `loams-wal` (or a stock safekeeper).
    Wal,
    ComputeCtl,
}

impl Component {
    /// The name `storage_unavailable`'s `component` metadata carries.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pageserver => "pageserver",
            Self::StorageController => "storage_controller",
            Self::Wal => "loams_wal",
            Self::ComputeCtl => "compute_ctl",
        }
    }

    fn is_storage(self) -> bool {
        !matches!(self, Self::ComputeCtl)
    }
}

impl fmt::Display for Component {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A failed call: the HTTP status (0 when the component could not be
/// reached or answered something unreadable), its message and the
/// component. Neon's storage components answer errors as `{"msg": "..."}`
/// (`http-utils` `HttpErrorBody`), as `loams-wal` does; `compute_ctl` as
/// `{"error": "..."}` (`compute_api` `GenericAPIError`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{component} {status}: {msg}")]
pub struct NeonError {
    pub status: u16,
    pub msg: String,
    pub component: Component,
}

impl NeonError {
    pub(crate) fn transport(component: Component, e: impl fmt::Display) -> Self {
        Self {
            status: 0,
            msg: e.to_string(),
            component,
        }
    }

    /// The `loams.errors.v1` reason this error is reported as (PG2 rulings
    /// R2.3 and R2.4).
    ///
    /// A refused credential (401, 403) is Loams's own storage token or
    /// compute JWT, so it is `internal`, never `unauthenticated` (which tells
    /// the caller to sign in again). A status the components do not use for
    /// a caller's mistake is `internal` too: Loams sent a request it should
    /// not have.
    pub fn reason(&self) -> &'static str {
        match self.status {
            400 => "invalid_argument",
            404 => "not_found",
            409 => "already_exists",
            412 => "failed_precondition",
            429 => "resource_exhausted",
            0 | 500..=599 if self.component.is_storage() => "storage_unavailable",
            0 | 500..=599 => "unavailable",
            _ => "internal",
        }
    }

    /// The `ErrorInfo` an RPC carries for it: the reason, the `component`
    /// and the HTTP `status`. The component's message stays in the log, not
    /// in the answer: it can name internal hosts.
    pub fn error_info(&self) -> ErrorInfo {
        ErrorInfo {
            reason: self.reason().to_owned(),
            metadata: HashMap::from([
                ("component".to_owned(), self.component.as_str().to_owned()),
                ("status".to_owned(), self.status.to_string()),
            ])
            .into_iter()
            .collect(),
            ..Default::default()
        }
    }
}
