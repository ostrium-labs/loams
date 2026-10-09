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
    /// `loams-wal`.
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

/// The call that failed: the same status means different things on
/// different routes (a 409 on a create is a conflict, on a delete a deletion
/// in progress).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Op {
    /// Building a client (a bad base URL, or no HTTP client).
    Setup,
    AttachTenant,
    CreateTimeline,
    ListTimelines,
    GetTimeline,
    DeleteTimeline,
    LsnByTimestamp,
    TenantConfig,
    WalCreateTimeline,
    WalTimelineStatus,
    ComputeStatus,
    Configure,
    Terminate,
    Promote,
    Prewarm,
    PrewarmState,
}

/// A failed call: the HTTP status (0 when the component could not be
/// reached or answered something unreadable), its message, the component
/// and the call. Neon's storage components answer errors as `{"msg": "..."}`
/// (`http-utils` `HttpErrorBody`), as `loams-wal` does; `compute_ctl` as
/// `{"error": "..."}` (`compute_api` `GenericAPIError`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{component} {op:?} {status}: {msg}")]
pub struct NeonError {
    pub status: u16,
    pub msg: String,
    pub component: Component,
    pub op: Op,
}

impl NeonError {
    pub(crate) fn transport(component: Component, op: Op, e: impl fmt::Display) -> Self {
        Self {
            status: 0,
            msg: e.to_string(),
            component,
            op,
        }
    }

    /// The `loams.errors.v1` reason this error is reported as (PG2 rulings
    /// R2.4 and R2.12). The status is read with the call:
    ///
    /// - 400 `invalid_argument`; 404 `not_found`; 408 `unavailable`.
    /// - 406 on a timeline create is the branch point: below the ancestor's
    ///   GC cutoff it is `lsn_out_of_retention`, otherwise (before the
    ///   ancestor's own branch point, an archived ancestor, a wait that timed
    ///   out) `failed_precondition`.
    /// - 409 on a delete is a deletion already in progress (`aborted`: retry);
    ///   elsewhere `already_exists`.
    /// - 412 on a delete is `branch_has_children` for a timeline with children
    ///   and `not_found` for a missing tenant; elsewhere `failed_precondition`.
    /// - 429 is a create of the same timeline already in progress, or another
    ///   request of the kind in flight: `aborted` (retry).
    /// - A promotion `compute_ctl` refused or that did not complete is
    ///   `failed_precondition`: replace the replica, or prewarm it and retry
    ///   (R2.13).
    /// - 401 and 403 are `internal`: the refused credential is Loams's own
    ///   storage token or compute JWT, and `unauthenticated` would tell the
    ///   caller to sign in again. So is any other status, and a client that
    ///   could not be built.
    /// - 0 and 5xx are `storage_unavailable` for the storage components and
    ///   `unavailable` for `compute_ctl`.
    pub fn reason(&self) -> &'static str {
        let msg = self.msg.as_str();
        match (self.op, self.status) {
            (Op::Setup, _) => "internal",
            (Op::Promote, 200 | 500) => "failed_precondition",
            (_, 400) => "invalid_argument",
            (_, 404) => "not_found",
            (Op::CreateTimeline, 406) if msg.contains("GC cutoff") => "lsn_out_of_retention",
            (Op::CreateTimeline, 406) => "failed_precondition",
            (_, 408) => "unavailable",
            (Op::DeleteTimeline, 409) => "aborted",
            (_, 409) => "already_exists",
            (Op::DeleteTimeline, 412) if msg.contains("child timelines") => "branch_has_children",
            (_, 412) if msg.contains("tenant is missing") => "not_found",
            (_, 412) => "failed_precondition",
            (_, 429) => "aborted",
            (_, 0 | 500..=599) if self.component.is_storage() => "storage_unavailable",
            (_, 0 | 500..=599) => "unavailable",
            _ => "internal",
        }
    }

    /// The `ErrorInfo` an RPC carries for it: the reason, and only the
    /// metadata `docs/api/reasons.md` registers for it (`component` for
    /// `storage_unavailable`, `children` for `branch_has_children`,
    /// `oldest_lsn` for `lsn_out_of_retention`, `kind` for `not_found`). The
    /// component's message stays in the log, not in the answer: it can name
    /// internal hosts.
    pub fn error_info(&self) -> ErrorInfo {
        let reason = self.reason();
        let mut metadata = HashMap::new();
        match reason {
            "storage_unavailable" => {
                metadata.insert("component".to_owned(), self.component.as_str().to_owned());
            }
            "branch_has_children" => {
                if let Some(n) = children(&self.msg) {
                    metadata.insert("children".to_owned(), n.to_string());
                }
            }
            "lsn_out_of_retention" => {
                if let Some(lsn) = gc_cutoff(&self.msg) {
                    metadata.insert("oldest_lsn".to_owned(), lsn.to_owned());
                }
            }
            "not_found" => {
                let kind = match self.op {
                    _ if self.msg.contains("tenant is missing") => "tenant",
                    Op::AttachTenant | Op::TenantConfig => "tenant",
                    _ => "timeline",
                };
                metadata.insert("kind".to_owned(), kind.to_owned());
            }
            _ => {}
        }
        ErrorInfo {
            reason: reason.to_owned(),
            metadata: metadata.into_iter().collect(),
            ..Default::default()
        }
    }
}

/// The number of children in the pageserver's "Cannot delete timeline which
/// has child timelines: [a, b]".
fn children(msg: &str) -> Option<usize> {
    let list = msg.rsplit_once('[')?.1.split_once(']')?.0.trim();
    Some(if list.is_empty() {
        0
    } else {
        list.split(',').count()
    })
}

/// The LSN in "invalid branch start lsn: less than latest GC cutoff X/Y".
fn gc_cutoff(msg: &str) -> Option<&str> {
    let rest = msg.split_once("GC cutoff ")?.1;
    let lsn = rest
        .split(|c: char| !(c.is_ascii_hexdigit() || c == '/'))
        .next()?;
    lsn.contains('/').then_some(lsn)
}
