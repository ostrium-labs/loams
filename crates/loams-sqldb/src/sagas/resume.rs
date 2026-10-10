//! The resume saga's effects (§47 §14; plan SQ1 Task 5). The `Lifecycle`
//! machine orders the steps and persists each; the host executes them:
//!
//! 1. `scale_up`: [`scale_up`], `scale(branch, n)`.
//! 2. `probe`: [`probe`] waits for a member's MySQL port and a `SELECT 1`
//!    through the gate's connector, then the held connections are
//!    released to the ready members.
//!
//! Both are idempotent, so a step repeated after a crash is safe.

use std::time::Duration;

use loams_sqlrouter::machines::lifecycle::Input;

use super::Prober;
use crate::model::BranchId;
use crate::runtime::{Member, MemberState, SqlRuntime};

/// How often a resume looks for a ready member.
const POLL: Duration = Duration::from_millis(100);

/// Scales `branch`'s pool to `replicas` members.
pub(crate) async fn scale_up(runtime: &dyn SqlRuntime, branch: &BranchId, replicas: u32) -> Input {
    match runtime.scale(branch, replicas).await {
        Ok(_) => Input::Scaled { replicas },
        Err(e) => {
            tracing::warn!(%branch, error = %e, "resume: scale failed; retrying");
            Input::ScaleFailed
        }
    }
}

/// The ready members of `branch`, or none when the runtime cannot say.
pub(crate) async fn ready_members(runtime: &dyn SqlRuntime, branch: &BranchId) -> Vec<Member> {
    match runtime.pool_status(branch).await {
        Ok(Some(st)) => st
            .members
            .into_iter()
            .filter(|m| m.state == MemberState::Ready)
            .collect(),
        Ok(None) => Vec::new(),
        Err(e) => {
            tracing::warn!(%branch, error = %e, "pool status failed");
            Vec::new()
        }
    }
}

/// Waits up to `wait` for a member whose port is open, then probes it with
/// `SELECT 1`. Answers `Probed { ok }` and the members that passed.
pub(crate) async fn probe(
    runtime: &dyn SqlRuntime,
    prober: &dyn Prober,
    branch: &BranchId,
    wait: Duration,
) -> (Input, Vec<Member>) {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let ready = ready_members(runtime, branch).await;
        let mut passed = Vec::new();
        for m in ready {
            match prober.probe(branch, &m).await {
                Ok(()) => passed.push(m),
                Err(e) => tracing::debug!(%branch, member = %m.name, error = %e, "probe failed"),
            }
        }
        if !passed.is_empty() {
            return (Input::Probed { ok: true }, passed);
        }
        if tokio::time::Instant::now() >= deadline {
            return (Input::Probed { ok: false }, Vec::new());
        }
        tokio::time::sleep(POLL).await;
    }
}
