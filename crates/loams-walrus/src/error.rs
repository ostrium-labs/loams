//! The fast path's typed errors.

use std::fmt;

use url::Url;

use crate::types::ContentDigest;

/// Why one aggregator's attempt failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttemptFailure {
    /// The aggregator answered 404.
    NotFound,
    /// The bytes did not hash to the expected digest; they were dropped.
    Integrity {
        /// The digest of what the aggregator sent.
        actual: ContentDigest,
    },
    /// The body was larger than `max_blob_bytes`.
    TooLarge {
        /// The size announced, or reached when reading stopped.
        size: u64,
        /// The configured limit.
        max: u64,
    },
    /// Anything else: transport errors, timeouts, other statuses, bad JSON.
    Other(String),
}

impl fmt::Display for AttemptFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("not found"),
            Self::Integrity { actual } => write!(f, "sha256 mismatch (got {actual})"),
            Self::TooLarge { size, max } => {
                write!(f, "body of {size} bytes over max_blob_bytes {max}")
            }
            Self::Other(msg) => f.write_str(msg),
        }
    }
}

/// An error of the Walrus fast path.
#[derive(Debug, thiserror::Error)]
pub enum WalrusError {
    /// `[walrus] enabled = false` (the default): nothing was contacted.
    #[error("the Walrus fast path is disabled ([walrus] enabled = false)")]
    Disabled,
    /// Every aggregator answered 404.
    #[error("Walrus blob {0} not found on any aggregator")]
    NotFound(String),
    /// Every aggregator that answered sent bytes whose sha256 is not the
    /// expected one. The bytes were dropped; nothing unverified is ever
    /// returned or cached.
    #[error("Walrus blob {blob}: expected sha256 {expected}, {aggregator} sent {actual}")]
    Integrity {
        /// The blob or quilt patch read.
        blob: String,
        /// The last aggregator that sent wrong bytes (its URL).
        aggregator: String,
        /// The digest the caller asked for.
        expected: ContentDigest,
        /// The digest of what that aggregator sent.
        actual: ContentDigest,
    },
    /// A body over `max_blob_bytes` (10 MiB by default, the public
    /// endpoints' limit): refused before upload, or while streaming a read.
    #[error("{what} of {size} bytes exceeds max_blob_bytes {max}")]
    TooLarge {
        /// What was too large.
        what: &'static str,
        /// Its size, or the size reached when reading stopped.
        size: u64,
        /// The configured limit.
        max: u64,
    },
    /// No aggregator produced verified bytes, for mixed reasons.
    #[error("Walrus blob {blob}: all aggregators failed: {}", render(.attempts))]
    AllAggregatorsFailed {
        /// The blob or quilt patch read.
        blob: String,
        /// Each aggregator tried and why it failed.
        attempts: Vec<(Url, AttemptFailure)>,
    },
    /// A write with no publisher configured: Mainnet has no public
    /// publisher (Q1330), so a Mainnet config without one is read-only.
    #[error("no Walrus publisher configured: this client is read-only")]
    NoPublisher,
    /// The publisher refused or failed a store.
    #[error("Walrus publisher: {0}")]
    Publisher(String),
    /// A blob id, patch id or digest that does not parse.
    #[error("invalid Walrus id {value:?}: {reason}")]
    InvalidId {
        /// The text given.
        value: String,
        /// Why it does not parse.
        reason: String,
    },
    /// A config or argument that cannot work.
    #[error("Walrus config: {0}")]
    Config(String),
    /// A local file error (the cache, a streamed read's destination, the
    /// token file).
    #[error("Walrus local I/O: {0}")]
    Io(#[from] std::io::Error),
}

fn render(attempts: &[(Url, AttemptFailure)]) -> String {
    attempts
        .iter()
        .map(|(url, why)| format!("{url}: {why}"))
        .collect::<Vec<_>>()
        .join("; ")
}
