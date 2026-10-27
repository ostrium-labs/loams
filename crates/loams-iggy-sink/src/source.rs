//! The Iggy side of the sink, as a trait so the runner can be tested without
//! a server ([`crate::memory::MemorySource`]).

use async_trait::async_trait;
use bytes::Bytes;

/// One topic of the sink's Iggy stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopicInfo {
    pub name: String,
    /// Iggy partitions, numbered `0..partitions`.
    pub partitions: u32,
}

/// One Iggy user header, as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    /// The key's bytes (whatever its kind).
    pub name: Vec<u8>,
    /// The value's raw bytes.
    pub value: Bytes,
    /// The value as a signed 64-bit integer, when its kind is `Int64`.
    pub int64: Option<i64>,
}

/// One polled Iggy message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawMessage {
    pub offset: u64,
    /// The message id the producer set (0 when it set none).
    pub id: u128,
    /// The server's append time, microseconds since the epoch.
    pub timestamp_us: u64,
    /// The producer's time, microseconds since the epoch (0: none).
    pub origin_timestamp_us: u64,
    pub payload: Bytes,
    /// The user headers (empty when there are none); `Err` holds the length
    /// of a header block that could not be parsed.
    pub headers: Result<Vec<Header>, u32>,
}

/// Why a call to Iggy failed. Every error is retried by the runner.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SourceError {
    /// The stream or topic does not exist (yet).
    #[error("not found: {0}")]
    NotFound(String),
    /// Anything else: the connection, a timeout, the server's answer.
    #[error("iggy: {0}")]
    Iggy(String),
}

#[async_trait]
pub trait Source: Send + Sync + 'static {
    /// The topics of `stream`.
    async fn topics(&self, stream: &str) -> Result<Vec<TopicInfo>, SourceError>;

    /// The last offset committed for this consumer on `partition`, if any.
    async fn committed(
        &self,
        stream: &str,
        topic: &str,
        partition: u32,
    ) -> Result<Option<u64>, SourceError>;

    /// Up to `count` messages from `offset` on, in offset order.
    async fn poll(
        &self,
        stream: &str,
        topic: &str,
        partition: u32,
        offset: u64,
        count: u32,
    ) -> Result<Vec<RawMessage>, SourceError>;

    /// Commits `offset` (the last message handled) for this consumer.
    async fn commit(
        &self,
        stream: &str,
        topic: &str,
        partition: u32,
        offset: u64,
    ) -> Result<(), SourceError>;
}
