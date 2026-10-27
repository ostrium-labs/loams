//! Loams Meter's House side (design §63 §7.5, §8; ME1 Tasks 5–8): the
//! `_meter` tables and the pipe that fills them from `_system/_meter`.
//!
//! - [`sql`]: the SQL, as text (std only; fabric's real-House test compiles
//!   it too).
//! - [`house`]: [`house::HouseSql`], the House as the meter reaches it, and
//!   [`house::ensure_schema`], which the `meter` role runs at start.
//!
//! The `meter` role (`crates/loams-meter`, feature `meter` of `loams`) wires
//! this crate: it owns the House credentials and the schedule.

pub mod house;
pub mod sql;

pub use house::{HouseError, HouseSql, HttpHouse, ensure_schema};
