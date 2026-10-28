//! Task lifecycle and resource limits. Durable callers must commit transitions
//! with the lease check in one transaction, and enforce actor authorization.
use serde::{Deserialize, Serialize};

use crate::{Lease, LeaseToken};

/// The allowed lifecycle states of a task.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum TaskState {
    /// The task is available for a lease holder to start.
    Ready,
    /// A lease holder is executing it.
    Running,
    /// Progress requires an external resolution or retry.
    Blocked,
    /// The task completed successfully.
    Done,
    /// The task ended unsuccessfully.
    Failed,
}

/// Hard per-task limits. `deadline_ms` is an absolute trusted-clock deadline.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskBudget {
    /// Maximum tokens consumed by this task.
    pub tokens: u64,
    /// Maximum steps consumed by this task.
    pub steps: u64,
    /// Exclusive absolute deadline in milliseconds.
    pub deadline_ms: u64,
}

/// A rejected task transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskError {
    /// The task identifier or budget is empty.
    InvalidTask,
    /// The lease is absent, expired, fenced or otherwise invalid.
    Lease,
    /// The requested state transition is not allowed.
    InvalidTransition,
    /// The task's absolute deadline has been reached.
    Deadline,
    /// A token or step budget would be exceeded.
    BudgetExceeded,
}

/// Durable task state and cumulative usage.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Task {
    id: String,
    budget: TaskBudget,
    state: TaskState,
    tokens_used: u64,
    steps_used: u64,
}

impl Task {
    /// Creates a ready task with positive token, step and wall-time limits.
    pub fn new(id: &str, budget: TaskBudget) -> Result<Self, TaskError> {
        if id.is_empty() || budget.tokens == 0 || budget.steps == 0 || budget.deadline_ms == 0 {
            return Err(TaskError::InvalidTask);
        }
        Ok(Self {
            id: id.to_owned(),
            budget,
            state: TaskState::Ready,
            tokens_used: 0,
            steps_used: 0,
        })
    }

    /// Returns the task's stable identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the lifecycle state.
    #[must_use]
    pub fn state(&self) -> TaskState {
        self.state
    }

    /// Returns the tokens consumed so far.
    #[must_use]
    pub fn tokens_used(&self) -> u64 {
        self.tokens_used
    }

    /// Returns the steps consumed so far.
    #[must_use]
    pub fn steps_used(&self) -> u64 {
        self.steps_used
    }

    fn authorize(&self, lease: &Lease, token: &LeaseToken, now_ms: u64) -> Result<(), TaskError> {
        lease.check(token, now_ms).map_err(|_| TaskError::Lease)?;
        if now_ms >= self.budget.deadline_ms {
            return Err(TaskError::Deadline);
        }
        Ok(())
    }

    fn transition(
        &mut self,
        lease: &Lease,
        token: &LeaseToken,
        now_ms: u64,
        from: TaskState,
        to: TaskState,
    ) -> Result<(), TaskError> {
        self.authorize(lease, token, now_ms)?;
        if self.state != from {
            return Err(TaskError::InvalidTransition);
        }
        self.state = to;
        Ok(())
    }

    /// Starts a ready task under its live lease.
    pub fn start(
        &mut self,
        lease: &Lease,
        token: &LeaseToken,
        now_ms: u64,
    ) -> Result<(), TaskError> {
        self.transition(lease, token, now_ms, TaskState::Ready, TaskState::Running)
    }

    /// Validates resuming a previously running task under a newly acquired lease.
    ///
    /// The caller must atomically acquire that lease and retain the last
    /// checkpoint; this transition does not itself establish exclusivity.
    pub fn recover(&self, lease: &Lease, token: &LeaseToken, now_ms: u64) -> Result<(), TaskError> {
        self.authorize(lease, token, now_ms)?;
        if self.state != TaskState::Running {
            return Err(TaskError::InvalidTransition);
        }
        Ok(())
    }

    /// Accounts a running task's tokens and steps without exceeding either limit.
    pub fn consume(
        &mut self,
        lease: &Lease,
        token: &LeaseToken,
        now_ms: u64,
        tokens: u64,
        steps: u64,
    ) -> Result<(), TaskError> {
        self.authorize(lease, token, now_ms)?;
        if self.state != TaskState::Running {
            return Err(TaskError::InvalidTransition);
        }
        let next_tokens = self
            .tokens_used
            .checked_add(tokens)
            .ok_or(TaskError::BudgetExceeded)?;
        let next_steps = self
            .steps_used
            .checked_add(steps)
            .ok_or(TaskError::BudgetExceeded)?;
        if next_tokens > self.budget.tokens || next_steps > self.budget.steps {
            return Err(TaskError::BudgetExceeded);
        }
        self.tokens_used = next_tokens;
        self.steps_used = next_steps;
        Ok(())
    }

    /// Marks a running task blocked.
    pub fn block(
        &mut self,
        lease: &Lease,
        token: &LeaseToken,
        now_ms: u64,
    ) -> Result<(), TaskError> {
        self.transition(lease, token, now_ms, TaskState::Running, TaskState::Blocked)
    }

    /// Returns a blocked task to ready after its caller resolves the blockage.
    pub fn requeue(
        &mut self,
        lease: &Lease,
        token: &LeaseToken,
        now_ms: u64,
    ) -> Result<(), TaskError> {
        self.transition(lease, token, now_ms, TaskState::Blocked, TaskState::Ready)
    }

    /// Marks a running task completed.
    pub fn finish(
        &mut self,
        lease: &Lease,
        token: &LeaseToken,
        now_ms: u64,
    ) -> Result<(), TaskError> {
        self.transition(lease, token, now_ms, TaskState::Running, TaskState::Done)
    }

    /// Marks a running task failed.
    pub fn fail(
        &mut self,
        lease: &Lease,
        token: &LeaseToken,
        now_ms: u64,
    ) -> Result<(), TaskError> {
        self.transition(lease, token, now_ms, TaskState::Running, TaskState::Failed)
    }
}
