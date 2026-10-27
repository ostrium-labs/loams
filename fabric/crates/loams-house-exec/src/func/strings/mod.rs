//! ClickHouse's string functions on the Rust engine (plan HE1 Task 31):
//! case, length, concatenation and slices, prefixes, positions, replacement,
//! `LIKE`, regular expressions (RE2 read by the `regex` crate, R31.3) and
//! UUIDs. Semantics are ClickHouse 26.9's (`src/Functions/`, Apache-2.0),
//! measured on the oracle (`conformance/clickhouse/oracle/corpus/he1/31`).
//!
//! The engine's `String` is Arrow `Utf8` (R21.3), so a byte-wise function
//! whose result would split a UTF-8 character (`left('ä', 1)`) is refused
//! with `48` rather than answered differently (R31.2).

mod basic;
mod case;
mod pattern;
pub mod re2;
mod replace;
mod search;
mod uuid;

use loams_house_sql::Registry;

/// The canonical names of the functions this module registers.
pub const NAMES: &[&str] = &[
    "lower",
    "upper",
    "lowerUTF8",
    "upperUTF8",
    "lengthUTF8",
    "concat",
    "left",
    "right",
    "leftUTF8",
    "rightUTF8",
    "substring",
    "substringUTF8",
    "startsWith",
    "endsWith",
    "startsWithUTF8",
    "endsWithUTF8",
    "ascii",
    "trimBoth",
    "trimLeft",
    "trimRight",
    "position",
    "positionUTF8",
    "positionCaseInsensitive",
    "positionCaseInsensitiveUTF8",
    "locate",
    "multiSearchAny",
    "multiSearchAnyUTF8",
    "multiSearchAnyCaseInsensitive",
    "multiSearchAnyCaseInsensitiveUTF8",
    "replaceOne",
    "replaceAll",
    "replaceRegexpOne",
    "replaceRegexpAll",
    "like",
    "notLike",
    "ilike",
    "notILike",
    "match",
    "generateUUIDv4",
    "toUUID",
    "toUUIDOrNull",
    "toUUIDOrZero",
];

/// Registers the string functions.
pub fn register(r: &mut Registry) {
    case::register(r);
    basic::register(r);
    search::register(r);
    replace::register(r);
    pattern::register(r);
    uuid::register(r);
}
