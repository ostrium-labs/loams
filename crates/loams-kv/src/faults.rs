//! Fault hooks of the transaction runner (R1 plan Task 2), backend-neutral.
//!
//! A [`FaultPlan`] is consulted at four points of every attempt of
//! [`Store::run`](crate::Store::run). Each point and fault means the same on
//! both backends (on TiKV, exactly what `loams_tikv::faults` documents; the
//! conformance case `fault_points_and_commit_tokens` holds both to it):
//!
//! | Point | `Refuse` | `Conflict` | `LoseAck` | `Delay(d)` |
//! |---|---|---|---|---|
//! | `BeforeBegin` | not applied | conflict | not applied | sleep `d` |
//! | `BeforePrewrite` (the body has run; before the commit token is written) | roll back, not applied | roll back, conflict | roll back, then an unknown outcome | sleep `d` |
//! | `BeforeCommit` (right before the commit) | roll back, not applied | roll back, conflict | roll back, then an unknown outcome | sleep `d` |
//! | `AfterCommit` (the commit succeeded) | unknown outcome | unknown outcome | unknown outcome | sleep `d` |

use std::time::Duration;

/// Where in an attempt a [`FaultPlan`] is consulted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FaultPoint {
    BeforeBegin,
    BeforePrewrite,
    BeforeCommit,
    AfterCommit,
}

/// What a [`FaultPlan`] injects (see the module docs for its effect at each
/// point).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    Refuse,
    Conflict,
    LoseAck,
    Delay(Duration),
}

/// Decides the fault, if any, at `point` of attempt `attempt` (from 1) of the
/// transaction named `op` ([`TxnOptions::op`](crate::TxnOptions::op)).
pub trait FaultPlan: Send + Sync {
    fn at(&self, op: &str, point: FaultPoint, attempt: u32) -> Option<Fault>;
}
