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
