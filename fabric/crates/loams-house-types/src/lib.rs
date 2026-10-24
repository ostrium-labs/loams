//! ClickHouse types for the Loams House Rust engine (design §75 §6.3; plan
//! HE1 Task 21).
//!
//! The type grammar ClickHouse accepts, the engine's Arrow layout of each type
//! (the ClickHouse type kept in the field's `loams.ch.type` metadata), the
//! House's Iceberg encodings, default values, canonical names (`toTypeName`)
//! and value text. Every other engine crate builds on this one, so it depends
//! on Arrow only.
