//! The client pipelines (§85 §7): uploads and downloads that resume from a
//! checkpoint, with bounded memory, and the progress they report.

pub mod download;
pub mod local;
pub mod progress;
pub mod tail;
pub mod upload;

use std::io;
use std::time::Duration;

use crate::api::ClientError;

pub use download::download;
pub use local::StateDir;
pub use progress::{Direction, Phase, Progress, Tracker};
pub use upload::{UploadOpts, pick_chunk_size, upload};

/// Why an upload or download stopped.
#[derive(Debug, thiserror::Error)]
pub enum TransferError {
    /// The server refused, or the network kept failing.
    #[error("{0}")]
    Client(#[from] ClientError),
    /// The local file or state.
    #[error("local: {0}")]
    Local(String),
    /// The source changed under an upload: it must start over (D2426).
    #[error("source changed: {0}")]
    SourceChanged(String),
    /// Bytes did not match their hash.
    #[error("integrity: {0}")]
    Integrity(String),
}

impl From<io::Error> for TransferError {
    fn from(e: io::Error) -> Self {
        TransferError::Local(e.to_string())
    }
}

impl From<serde_json::Error> for TransferError {
    fn from(e: serde_json::Error) -> Self {
        TransferError::Local(e.to_string())
    }
}

impl TransferError {
    /// A short machine code for the desktop.
    pub fn code(&self) -> &str {
        match self {
            TransferError::Client(e) => e.code().unwrap_or("network"),
            TransferError::Local(_) => "local",
            TransferError::SourceChanged(_) => "source_changed",
            TransferError::Integrity(_) => "integrity",
        }
    }
}

/// 1, 2, 4 … 30 s (§85 §7.1).
pub fn backoff(attempt: u32) -> Duration {
    let secs = 1u64 << attempt.saturating_sub(1).min(5);
    Duration::from_secs(secs.min(30))
}
