//! Compute runtimes (plan SQ1 "Shared contracts": `SqlRuntime`).
//!
//! A runtime owns one `tidb-server` pool per branch: `ensure_pool` creates
//! or resizes it, `scale` changes its member count (0 suspends it; the pool,
//! its class and its rendered config remain), `delete_pool` removes it, and
//! `run_job` runs a one-shot container (BR, Task 15). Drivers:
//! [`local::LocalRuntime`] (Podman or Docker), [`fake::FakeRuntime`] (saga
//! tests) and, in Task 14, `KubernetesRuntime`.

pub mod fake;
pub mod local;

use std::fmt;
use std::net::SocketAddr;
use std::time::Duration;

use async_trait::async_trait;

use crate::images::ImagePin;
use crate::model::{BranchId, Class};

/// A runtime call that failed.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    /// The runtime (container engine, API server) failed or is unreachable:
    /// `sqldb_runtime_unavailable`. Retrying is safe.
    #[error("runtime unavailable: {0}")]
    Unavailable(String),
    /// `scale` on a branch with no pool.
    #[error("no pool for branch {0}")]
    NoPool(BranchId),
    /// No free port, or the pool's state on disk is unreadable.
    #[error("runtime state: {0}")]
    State(String),
    /// A job ran past its timeout and was killed.
    #[error("job {0} timed out")]
    JobTimeout(String),
}

/// What a member is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberState {
    /// Created or running; its MySQL port is not answering yet (bootstrap,
    /// schema load).
    Starting,
    /// Running, MySQL port open.
    Ready,
    /// The process ended; the runtime replaces it on the next `scale`.
    Exited {
        /// Exit code, where the runtime knows it.
        code: Option<i32>,
    },
}

/// One `tidb-server` of a pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// 0-based; members `0..replicas` exist.
    pub index: u32,
    /// Container or pod name.
    pub name: String,
    /// The MySQL protocol address the gate connects to.
    pub mysql_addr: SocketAddr,
    /// TiDB's status (HTTP) address; never reachable from the gate network.
    pub status_addr: SocketAddr,
    /// State.
    pub state: MemberState,
}

/// A branch's pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolStatus {
    /// The branch (and keyspace).
    pub branch: BranchId,
    /// Its class.
    pub class: Class,
    /// The desired member count.
    pub replicas: u32,
    /// Members that exist, by index.
    pub members: Vec<Member>,
}

impl PoolStatus {
    /// Members whose MySQL port answers.
    pub fn ready(&self) -> usize {
        self.members
            .iter()
            .filter(|m| m.state == MemberState::Ready)
            .count()
    }
}

/// A one-shot container (BR backup or restore). Arguments and environment
/// carry no secrets; credentials reach the job through the runtime's own
/// secret mechanism (Task 15).
#[derive(Clone, PartialEq, Eq)]
pub struct JobSpec {
    /// Unique per job; the container or Job name.
    pub name: String,
    /// The image, by digest.
    pub image: ImagePin,
    /// Arguments after the image's entrypoint.
    pub args: Vec<String>,
    /// Environment, non-secret.
    pub env: Vec<(String, String)>,
    /// Killed after this.
    pub timeout: Duration,
}

impl fmt::Debug for JobSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JobSpec")
            .field("name", &self.name)
            .field("image", &self.image.reference())
            .field("args", &self.args)
            .field("env", &self.env.iter().map(|(k, _)| k).collect::<Vec<_>>())
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// How a job ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobOutcome {
    /// The process exit code.
    pub exit_code: i32,
    /// The last lines of its output.
    pub log_tail: String,
}

/// The compute runtime contract. Every pool method is idempotent, so a saga
/// step can be replayed after a crash. `run_job` is replay-safe only in the
/// sense that a replay is not refused: it runs the job again (a stale
/// container of the same name is replaced), so the job itself must be
/// idempotent or checkpointed (Task 15, BR).
#[async_trait]
pub trait SqlRuntime: Send + Sync + fmt::Debug {
    /// Creates `branch`'s pool, or brings an existing one to `class` and
    /// `replicas`. Returns without waiting for members to become ready.
    async fn ensure_pool(
        &self,
        branch: &BranchId,
        class: Class,
        replicas: u32,
    ) -> Result<PoolStatus, RuntimeError>;

    /// Sets the member count of an existing pool; 0 suspends it. Class
    /// limits on pods (§47 §15) are the control plane's policy, not checked
    /// here.
    async fn scale(&self, branch: &BranchId, replicas: u32) -> Result<PoolStatus, RuntimeError>;

    /// The pool, or `None` when the branch has none.
    async fn pool_status(&self, branch: &BranchId) -> Result<Option<PoolStatus>, RuntimeError>;

    /// Removes the pool, its members and its rendered config. Deleting a
    /// missing pool succeeds.
    async fn delete_pool(&self, branch: &BranchId) -> Result<(), RuntimeError>;

    /// Runs a job to completion. A non-zero exit is an outcome, not an error.
    /// A replay runs the job again; see the trait docs.
    async fn run_job(&self, spec: &JobSpec) -> Result<JobOutcome, RuntimeError>;
}
