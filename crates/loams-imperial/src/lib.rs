//! State transitions for Imperial Orchestrator leases.
//!
//! This is a pure state machine, not a lock service. A caller must apply each
//! transition atomically to durable storage (e.g. a compare-and-swap of the
//! complete serialized state). A fenced task write must check the lease and
//! change the task in the **same transaction**. The caller authenticates the
//! holder, obtains time from a trusted clock and proves progress by committing
//! the corresponding checkpoint or state change in that transaction.

use serde::{Deserialize, Serialize};

mod task;
pub use task::{Task, TaskBudget, TaskError, TaskState};

/// A grant identifying the holder and fencing epoch of one lease acquisition.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LeaseToken {
    holder: String,
    epoch: u64,
}

impl LeaseToken {
    /// The authenticated principal the caller must bind to this token.
    #[must_use]
    pub fn holder(&self) -> &str {
        &self.holder
    }

    /// The monotonically increasing fencing epoch.
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

/// A refused lease transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeaseError {
    /// A live acquisition already exists.
    Held,
    /// The token is for a revoked or replaced acquisition.
    Fenced,
    /// The acquisition has reached its deadline.
    Expired,
    /// Renewal did not advance the committed progress marker.
    NoProgress,
    /// Holder names must not be empty.
    InvalidHolder,
    /// A zero lease duration is invalid.
    InvalidTtl,
    /// The duration and timestamp cannot be added without overflow.
    TimeOverflow,
    /// The supplied time precedes the last committed lease transition.
    ClockRegression,
    /// No further fencing epoch can be represented.
    EpochOverflow,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Grant {
    holder: String,
    deadline_ms: u64,
    progress: u64,
}

/// Persisted state of a named leader or task lease.
///
/// Epochs must never be reset by deleting this row: a recreated lease would
/// otherwise make an old token valid again. Store a revoked state as a
/// tombstone, not as a deleted key.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Lease {
    epoch: u64,
    last_transition_ms: u64,
    grant: Option<Grant>,
}

impl Lease {
    /// Creates a never-acquired lease with an initial epoch of zero.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn deadline(&self, now_ms: u64, ttl_ms: u64) -> Result<u64, LeaseError> {
        if now_ms < self.last_transition_ms {
            return Err(LeaseError::ClockRegression);
        }
        if ttl_ms == 0 {
            return Err(LeaseError::InvalidTtl);
        }
        now_ms.checked_add(ttl_ms).ok_or(LeaseError::TimeOverflow)
    }

    /// Acquires an absent or expired lease, issuing a new epoch.
    pub fn claim(
        &mut self,
        holder: &str,
        now_ms: u64,
        ttl_ms: u64,
    ) -> Result<LeaseToken, LeaseError> {
        if holder.is_empty() {
            return Err(LeaseError::InvalidHolder);
        }
        let deadline_ms = self.deadline(now_ms, ttl_ms)?;
        if self
            .grant
            .as_ref()
            .is_some_and(|grant| grant.deadline_ms > now_ms)
        {
            return Err(LeaseError::Held);
        }
        let epoch = self.epoch.checked_add(1).ok_or(LeaseError::EpochOverflow)?;
        self.epoch = epoch;
        self.last_transition_ms = now_ms;
        self.grant = Some(Grant {
            holder: holder.to_owned(),
            deadline_ms,
            progress: 0,
        });
        Ok(LeaseToken {
            holder: holder.to_owned(),
            epoch,
        })
    }

    /// Checks that the token is the currently live acquisition at `now_ms`.
    pub fn check(&self, token: &LeaseToken, now_ms: u64) -> Result<(), LeaseError> {
        if now_ms < self.last_transition_ms {
            return Err(LeaseError::ClockRegression);
        }
        let grant = self.grant.as_ref().ok_or(LeaseError::Fenced)?;
        if token.epoch != self.epoch || token.holder != grant.holder {
            return Err(LeaseError::Fenced);
        }
        if now_ms >= grant.deadline_ms {
            return Err(LeaseError::Expired);
        }
        Ok(())
    }

    /// Renews a live acquisition only when its committed progress advances.
    pub fn renew(
        &mut self,
        token: &LeaseToken,
        now_ms: u64,
        ttl_ms: u64,
        progress: u64,
    ) -> Result<(), LeaseError> {
        self.check(token, now_ms)?;
        let deadline_ms = self.deadline(now_ms, ttl_ms)?;
        let grant = self.grant.as_mut().expect("check requires a grant");
        if progress <= grant.progress {
            return Err(LeaseError::NoProgress);
        }
        grant.progress = progress;
        grant.deadline_ms = deadline_ms;
        self.last_transition_ms = now_ms;
        Ok(())
    }

    /// Revokes a live acquisition and advances the epoch before reassignment.
    pub fn revoke(&mut self, token: &LeaseToken, now_ms: u64) -> Result<(), LeaseError> {
        self.check(token, now_ms)?;
        self.epoch = self.epoch.checked_add(1).ok_or(LeaseError::EpochOverflow)?;
        self.last_transition_ms = now_ms;
        self.grant = None;
        Ok(())
    }

    // Called only after the supervisor's lease is verified in the same transaction.
    pub(crate) fn revoke_by_supervisor(&mut self, now_ms: u64) -> Result<(), LeaseError> {
        if now_ms < self.last_transition_ms {
            return Err(LeaseError::ClockRegression);
        }
        self.epoch = self.epoch.checked_add(1).ok_or(LeaseError::EpochOverflow)?;
        self.last_transition_ms = now_ms;
        self.grant = None;
        Ok(())
    }
}

/// Durable lease and checkpoint transactions.
pub mod durable;
/// Leader election and transactional cancellation.
pub mod supervisor;
/// Transactional task state and budget accounting.
pub mod task_store;
