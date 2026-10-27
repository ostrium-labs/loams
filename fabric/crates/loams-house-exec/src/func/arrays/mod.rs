//! ClickHouse's array functions on the Rust engine (plan HE1 Task 32).
//! Semantics are ClickHouse 26.9's (`src/Functions/array/`, Apache-2.0),
//! checked against the oracle (`conformance/clickhouse/oracle/corpus/he1/32`).
//!
//! The array functions that take a lambda (`arrayMap`, `arraySum(λ, …)`,
//! `arraySort(λ, …)`, …) are in [`crate::func::lambda`]; [`fold`] holds the
//! summing and sorting they share with their plain forms.

pub mod access;
pub mod build;
pub mod fold;
pub mod shape;

use loams_house_sql::Registry;

/// The canonical names of the scalar functions this module registers.
pub const NAMES: &[&str] = &[
    "array",
    "range",
    "arrayConcat",
    "arrayZip",
    "arrayJoin",
    "arrayElement",
    "has",
    "hasAll",
    "hasAny",
    "indexOf",
    "length",
    "empty",
    "notEmpty",
    "arraySlice",
    "arrayDistinct",
    "arrayEnumerate",
    "arrayEnumerateUniq",
    "arrayReverse",
    "arrayStringConcat",
];

/// Registers the array functions.
pub fn register(r: &mut Registry) {
    build::register(r);
    access::register(r);
    shape::register(r);
}
