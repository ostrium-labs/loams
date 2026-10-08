//! The runner's options, error classes and result (R1 plan Task 2; design
//! §20 §5.1), backend-neutral. [`Store::run`](crate::Store::run) runs a body
//! in a new transaction and commits it, restarting it at a new start
//! timestamp after a jittered backoff on a conflict or a not-applied error,
//! until `max_attempts` or the deadline; an unknown commit outcome is
//! resolved through the commit token when the options ask for one.

use std::time::Duration;

use crate::Ts;

/// Optimistic or pessimistic concurrency control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Reads at the start timestamp; conflicts show at commit (the default).
    #[default]
    Optimistic,
    /// Locks taken at once, so a second holder queues (R1 Ruling 2).
    Pessimistic,
}

/// How a transaction commits (R1 Ruling 3). The embedded backend has one
/// commit path and ignores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CommitMode {
    /// Async commit, and one-phase commit when the transaction fits one
    /// region (the default).
    #[default]
    Async1pc,
    /// Classic two-phase commit.
    TwoPc,
}

/// The options of one [`Store::run`](crate::Store::run).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxnOptions {
    pub mode: Mode,
    /// Attempts before the runner gives up (default 8).
    pub max_attempts: u32,
    /// The wall-clock budget of the whole run (default 10 s).
    pub deadline: Duration,
    /// Write a commit token, so an unknown outcome can be resolved.
    pub commit_token: bool,
    /// The operation's name, for the fault plan and metrics.
    pub op: &'static str,
    /// `None`: the store's configured [`CommitMode`].
    pub commit_mode: Option<CommitMode>,
}

impl TxnOptions {
    /// Optimistic, 8 attempts, 10 s, no commit token, the store's commit mode.
    pub fn new(op: &'static str) -> Self {
        TxnOptions {
            mode: Mode::Optimistic,
            max_attempts: 8,
            deadline: Duration::from_secs(10),
            commit_token: false,
            op,
            commit_mode: None,
        }
    }

    /// Like [`new`](Self::new), pessimistic.
    pub fn pessimistic(op: &'static str) -> Self {
        TxnOptions {
            mode: Mode::Pessimistic,
            ..TxnOptions::new(op)
        }
    }

    /// With a commit token.
    pub fn with_token(mut self) -> Self {
        self.commit_token = true;
        self
    }
}

/// Why a [`Store::run`](crate::Store::run) (or a read) failed. Messages never
/// carry a key with its keyspace prefix or root, and show at most 64 bytes
/// of one.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TxnError {
    /// A conflict the runner's attempts or deadline did not outlast. Returned
    /// by a body, it makes the runner restart the transaction.
    #[error("transaction conflict")]
    Conflict,
    /// The transaction certainly did not commit. The runner retries it like
    /// a conflict.
    #[error("not applied: {0}")]
    NotApplied(String),
    /// The commit may or may not have applied. `token` is the commit token
    /// whose presence would tell, when there is one and resolving it failed.
    #[error("commit outcome undetermined")]
    Undetermined { token: Option<[u8; 16]> },
    /// An insert found its key.
    #[error("key already exists: {0}")]
    AlreadyExists(String),
    /// Invalid arguments, misuse, an unknown error kind, or a refusal of the
    /// store (a value over 2 MiB, a read below the GC safe point).
    #[error("{0}")]
    Fatal(String),
    /// The deadline passed before the transaction could commit.
    #[error("transaction deadline passed")]
    Deadline,
}

/// A committed run.
#[derive(Debug, Clone, PartialEq)]
pub struct Committed<T> {
    /// What the body returned on the attempt that committed.
    pub value: T,
    /// The commit timestamp. For a read-only transaction, its start
    /// timestamp; for a commit resolved through its token, the timestamp of
    /// the resolving read (an upper bound of the real one).
    pub commit_ts: Ts,
    /// The attempts it took, from 1.
    pub attempts: u32,
    /// The committing attempt's outcome was unknown and was resolved through
    /// the commit token.
    pub earlier_unknown: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_the_plan() {
        let o = TxnOptions::new("op");
        assert_eq!(o.max_attempts, 8);
        assert_eq!(o.deadline, Duration::from_secs(10));
        assert!(!o.commit_token);
        assert_eq!(o.mode, Mode::Optimistic);
        assert_eq!(o.commit_mode, None);
        assert_eq!(CommitMode::default(), CommitMode::Async1pc);
        let p = TxnOptions::pessimistic("p").with_token();
        assert_eq!(p.mode, Mode::Pessimistic);
        assert!(p.commit_token);
    }
}
