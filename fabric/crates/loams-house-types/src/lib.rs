//! ClickHouse types for the Loams House Rust engine (design §75 §6.3; plan
//! HE1 Task 21).
//!
//! The type grammar ClickHouse accepts ([`parse`]), its canonical text
//! ([`ChType`]'s `Display`, what `toTypeName` prints), the engine's Arrow
//! layout of each type ([`arrow`], the ClickHouse type kept in the field's
//! `loams.ch.type` metadata), the House's Iceberg encodings ([`storage`]),
//! default values ([`default`]) and value text per format family ([`text`]).
//! Every other engine crate builds on this one, so it depends on Arrow only
//! (and `chrono-tz` for time zones).
//!
//! Every rule here was measured on `clickhouse-server` 26.9.2.8, the oracle
//! (R0.3); `tests/` keeps the oracle's answers as fixtures, captured by
//! `scripts/house/capture-type-fixtures.sh`.

pub mod arrow;
pub mod default;
pub mod error;
pub mod parse;
pub mod storage;
pub mod text;
pub mod ty;

pub use error::ChError;
pub use parse::parse;
pub use ty::{AggregateSpec, ChType, JsonParams};
