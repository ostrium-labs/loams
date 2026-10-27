//! What can go wrong reading meters, spooling and forwarding events.

use std::io;
use std::path::{Path, PathBuf};

/// An error from the metering agent.
#[derive(Debug, thiserror::Error)]
pub enum MeterError {
    /// A file could not be read or written.
    #[error("{path}: {source}")]
    Io {
        /// The file.
        path: PathBuf,
        /// The cause.
        source: io::Error,
    },
    /// A file's contents were not what the kernel or the agent writes.
    #[error("{path}: {message}")]
    Parse {
        /// The file.
        path: PathBuf,
        /// What was wrong.
        message: String,
    },
    /// The spool is at its size bound. The caller keeps its baselines, so
    /// the usage is carried in the next delta, not lost (design 01 §6.2).
    #[error("spool full: {bytes} bytes held, limit {limit}")]
    SpoolFull {
        /// Bytes held by unacknowledged segments.
        bytes: u64,
        /// The configured bound.
        limit: u64,
    },
    /// The configuration was refused.
    #[error("config: {0}")]
    Config(String),
    /// A sink could not take the events now; retry later.
    #[error("sink unavailable: {0}")]
    SinkUnavailable(String),
    /// A sink refused the events; retrying the same events will not help.
    #[error("sink rejected the events: {0}")]
    SinkRejected(String),
}

impl MeterError {
    pub(crate) fn io(path: impl AsRef<Path>, source: io::Error) -> Self {
        Self::Io {
            path: path.as_ref().to_path_buf(),
            source,
        }
    }

    pub(crate) fn parse(path: impl AsRef<Path>, message: impl Into<String>) -> Self {
        Self::Parse {
            path: path.as_ref().to_path_buf(),
            message: message.into(),
        }
    }
}
