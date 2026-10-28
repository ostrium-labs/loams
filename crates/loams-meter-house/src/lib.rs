//! Loams Meter's House side (design §63 §7.5, §8; ME1 Tasks 5–8): the
//! `_meter` tables and the pipe that fills them from `_system/_meter`.
//!
//! - [`sql`]: the SQL, as text (std only; fabric's real-House test compiles
//!   it too).
//! - [`house`]: [`house::HouseSql`], the House as the meter reaches it, and
//!   [`house::ensure_schema`], which the `meter` role runs at start.
//! - [`close`]: the close job (Task 6), which the `meter` role runs every
//!   60 s on every instance ([`close::CloseJob::run_once`]).
//! - [`period`]: period totals and period close (Task 7), run after each
//!   close-job lap ([`period::PeriodCloser::run_once`]).
//! - [`query`]: `QueryMeter` and OpenMeter's `GET /api/v1/meters/{slug}/query`
//!   (Task 8), with ME-core's types.
//!
//! The `meter` role (`crates/loams-meter`, feature `meter` of `loams`) wires
//! this crate: it owns the House credentials and the schedule.

pub mod close;
pub mod house;
pub mod period;
pub mod query;
pub mod sql;

pub use close::{CloseError, CloseJob};
pub use house::{HouseError, HouseSql, HttpHouse, ensure_schema};
pub use period::{PeriodCloser, PeriodSpec};
pub use query::{MeterCatalog, MeterQuery, QueryError, openmeter_router, query_meter};
pub use sql::{
    Aggregation, CloseConfig, CloseSummary, FeatureDef, MeterDef, PeriodConfig, PeriodOutcome,
    PeriodSummary, WindowSize,
};
