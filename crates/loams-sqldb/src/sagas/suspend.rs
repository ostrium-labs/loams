//! The suspend saga's effects (§47 §14; plan SQ1 Task 5). The `Lifecycle`
//! machine orders the steps and persists each; the host executes them:
//!
//! 1. `close_idle`: the gate stops admitting (new connections are held by
//!    `EnsureRunning`) and closes idle sessions with 1053; every other
//!    session closes as soon as it is idle ([`close_sessions`] with
//!    [`Close::WhenIdle`]).
//! 2. `quiesce`: wait up to 30 s for the rest.
//! 3. `kill`: close them all ([`Close::Now`]).
//! 4. `scale_down`: [`scale_down`], `scale(branch, 0)`.
//!
//! Each effect is idempotent, so a step repeated after a crash is safe.

use std::collections::BTreeMap;

use loams_sqlrouter::machines::lifecycle::{ConnId, Input};
use tokio::sync::watch;

use super::Close;
use crate::model::BranchId;
use crate::runtime::SqlRuntime;

/// Asks every session to close as `close` says; [`Close::Open`] cancels
/// an earlier request (an aborted suspend). A stronger request is never
/// weakened except by `Open`.
pub(crate) fn close_sessions(leases: &BTreeMap<ConnId, watch::Sender<Close>>, close: Close) {
    for tx in leases.values() {
        tx.send_if_modified(|c| {
            let next = match (close, *c) {
                (Close::Open, _) => Close::Open,
                (Close::WhenIdle, Close::Now) => Close::Now,
                (other, _) => other,
            };
            let changed = next != *c;
            *c = next;
            changed
        });
    }
}

/// Scales `branch`'s pool to zero: the pool, its class and its rendered
/// config remain, no data moves and the keyspace stays enabled.
pub(crate) async fn scale_down(runtime: &dyn SqlRuntime, branch: &BranchId) -> Input {
    match runtime.scale(branch, 0).await {
        Ok(_) => Input::Scaled { replicas: 0 },
        Err(e) => {
            tracing::warn!(%branch, error = %e, "suspend: scale to zero failed; retrying");
            Input::ScaleFailed
        }
    }
}
