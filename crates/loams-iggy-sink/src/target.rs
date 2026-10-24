//! The Loams side of the sink: where the events go.

use std::time::Duration;

use async_trait::async_trait;
use loams_cloudevents::CloudEvent;

/// What one write did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WriteOutcome {
    /// Events appended by this write.
    pub appended: usize,
    /// Events the target already had (a replay).
    pub duplicates: usize,
    /// Events another writer is still appending: the whole batch is written
    /// again after this long (the ones that landed come back as duplicates).
    pub in_flight: Option<Duration>,
}

/// Why a write failed. The runner retries every kind; the batch is never
/// dropped and its offset never committed until a write succeeds.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TargetError {
    /// The target is overloaded (HTTP 429/503): wait `retry_after` (or the
    /// backoff) before trying again.
    #[error("backpressure: {message}")]
    Backpressure {
        retry_after: Option<Duration>,
        message: String,
    },
    /// The target cannot take this batch now (it is starting, a dependency
    /// is down, the outcome is unknown).
    #[error("unavailable: {0}")]
    Unavailable(String),
    /// The target refused the batch for a reason a retry will not fix by
    /// itself (configuration, a partition out of range). Retried slowly, and
    /// surfaced in the metrics, so an operator can fix it without losing data.
    #[error("rejected: {0}")]
    Rejected(String),
}

#[async_trait]
pub trait Target: Send + Sync + 'static {
    /// Makes sure `namespace/stream` exists with at least `partitions`
    /// partitions (creating it when it does not).
    async fn ensure_stream(
        &self,
        namespace: &str,
        stream: &str,
        partitions: u32,
    ) -> Result<(), TargetError>;

    /// Appends `events` to `partition` of `namespace/stream`, deduplicated by
    /// each event's `source` + `id`.
    async fn write(
        &self,
        namespace: &str,
        stream: &str,
        partition: u32,
        events: &[CloudEvent],
    ) -> Result<WriteOutcome, TargetError>;
}
