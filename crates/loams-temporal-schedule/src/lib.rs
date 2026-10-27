//! `loams-temporal-schedule`: Temporal Schedules for the Loams Temporal
//! gateway (design 74 §5, DX4's Schedules brought forward; D2070–D2073).
//!
//! - [`spec`]: a `ScheduleSpec` compiled ([`CompiledSpec`]): calendar specs
//!   ([`calendar`]), cron strings ([`cron`]), intervals, exclusions, start
//!   and end bounds, IANA time zones, and deterministic jitter.
//! - [`state`]: the runtime state beside the `Schedule` proto and the pure
//!   steps that move it: due times and the catch-up window, the buffer and
//!   the six overlap policies, limited actions, pause on failure.
//! - [`store`]: where schedules live, by compare-and-set
//!   ([`MemScheduleStore`], [`PgScheduleStore`]).
//!
//! The gateway's driver (`loams-temporal-frontend::schedules`) arms one
//! Resonate timer per wake-up with the deterministic `sch` promise ID
//! (`<ns>.<schedule_id>.<generation>:sch.<wake_ms>`, D2070) and starts each
//! workflow with a deterministic request ID, so a retried or duplicated
//! wake-up never starts a workflow twice. Nothing in this crate does I/O
//! except [`store`].

pub mod calendar;
pub mod cron;
pub mod spec;
pub mod state;
pub mod store;

pub use spec::{CompiledSpec, NextTime, canonicalize};
pub use state::{BufferedStart, Overlap, Plan, Policies, Runtime};
pub use store::{
    MemScheduleStore, PgScheduleStore, ScheduleRecord, ScheduleStore, ScheduleStoreError,
};

/// Why a spec or a request is refused (answered `INVALID_ARGUMENT`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpecError {
    #[error("invalid schedule spec: {0}")]
    Invalid(String),
}
