//! Temporal's default limits (v1.32.1 `common/dynamicconfig/constants.go`,
//! plan ruling R0.9; D1508, X1.R7). The gateway checks them where Temporal
//! does; a namespace may lower them later (DX2).

/// Size and count limits for one namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// `limit.blobSize.error`: one payload blob.
    pub blob_size: usize,
    /// One gRPC request (`MaxHTTPAPIRequestBytes`).
    pub grpc_message: usize,
    /// `limit.historySize.error`: the encoded history of one run.
    pub history_size: i64,
    /// `limit.historyCount.error`: events in one run.
    pub history_count: i64,
    /// `limit.numPendingActivities.error`.
    pub pending_activities: usize,
    /// `limit.numPendingSignals.error` (DX1 counts recorded signals).
    pub pending_signals: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            blob_size: 2 * 1024 * 1024,
            grpc_message: 4 * 1024 * 1024,
            history_size: 50 * 1024 * 1024,
            history_count: 51_200,
            pending_activities: 2_000,
            pending_signals: 2_000,
        }
    }
}

impl Limits {
    /// Whether a blob of `size` encoded bytes is over `limit.blobSize.error`
    /// (Temporal's `CheckEventBlobSizeLimit` errors on `size > limit`).
    pub fn blob_too_large(&self, size: usize) -> bool {
        size > self.blob_size
    }
}

/// Temporal's messages for the limits (v1.32.1 `common/util.go`), which the
/// SDKs and their users see verbatim.
pub mod messages {
    /// `ErrBlobSizeExceedsLimit`, an `InvalidArgument` (start, signal).
    pub const BLOB_SIZE: &str = "Blob data size exceeds limit.";
    /// `FailureReasonCompleteResultExceedsLimit`: an activity's result.
    pub const COMPLETE_RESULT: &str = "Complete result exceeds size limit.";
    /// `FailureReasonFailureExceedsLimit`: an activity's or workflow task's
    /// failure.
    pub const FAILURE: &str = "Failure exceeds size limit.";
    /// `FailureReasonCancelDetailsExceedsLimit`: an activity's cancel details.
    pub const CANCEL_DETAILS: &str = "Cancel details exceed size limit.";
    /// `FailureReasonHeartbeatExceedsLimit`: an activity's heartbeat details.
    pub const HEARTBEAT: &str = "Heartbeat details exceed size limit.";
    /// `FailureReasonHistorySizeExceedsLimit`: the terminate reason.
    pub const HISTORY_SIZE: &str = "Workflow history size exceeds limit.";
    /// `FailureReasonHistoryCountExceedsLimit`: the terminate reason.
    pub const HISTORY_COUNT: &str = "Workflow history count exceeds limit.";
    /// `consts.IdentityHistoryService`: who terminates a run over a limit.
    pub const HISTORY_SERVICE: &str = "history-service";
}
