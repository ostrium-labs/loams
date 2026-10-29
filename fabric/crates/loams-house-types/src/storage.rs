//! How the House stores a ClickHouse type in an Iceberg lake table: the
//! encodings `loams-house`'s `types.rs` writes (HS1 Task 10, D1572, HE1
//! R9.14, R9.61, R9.63), which the engine's reads (plan HE1 Task 26) undo.
//!
//! Described without iceberg-rust's types, so this crate stays on Arrow only;
//! `loams-house-iceberg` maps [`Storage`] to Iceberg's and checks a table's
//! schema against it.

use crate::error::ChError;
use crate::ty::ChType;

/// An Iceberg type, as the House uses it for a ClickHouse type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Storage {
    /// `int`: `Int8`, `Int16`, `Int32`, `UInt8`, `UInt16`.
    Int,
    /// `long`: `Int64`, `UInt32`.
    Long,
    /// `long` holding nanoseconds since the epoch: a `DateTime64(7..9)` column
    /// of its own (D1572).
    LongNanos,
    /// `float`.
    Float,
    /// `double`.
    Double,
    /// `decimal(P, S)`; `UInt64` is `decimal(20, 0)`.
    Decimal(u8, u8),
    /// `boolean`.
    Boolean,
    /// `string`: `String`, and the types stored as their text (`IPv4`, `IPv6`,
    /// `Enum*` by name, `JSON` as its JSON text).
    String,
    /// `fixed[N]`.
    Fixed(u32),
    /// `date`.
    Date,
    /// `timestamptz` (microseconds).
    TimestampTz,
    /// `timestamptz_ns` (format v3): `DateTime64(7..9)` inside a composite.
    TimestampTzNs,
    /// `uuid`.
    Uuid,
    /// `binary`: `AggregateFunction` states, ClickHouse's own bytes.
    Binary,
    /// `list<T>`.
    List(Box<Storage>),
    /// `map<K, V>`.
    Map(Box<Storage>, Box<Storage>),
    /// `struct<…>`, unnamed elements named `1`, `2`, ….
    Struct(Vec<(String, Storage)>),
}

/// The storage of `t`. `top_level` is whether `t` is a column's own type
/// (`DateTime64(7..9)` and `JSON` are stored differently inside a composite).
pub fn storage(t: &ChType, top_level: bool) -> Result<Storage, ChError> {
    Ok(match t {
        ChType::Int8 | ChType::Int16 | ChType::Int32 | ChType::UInt8 | ChType::UInt16 => {
            Storage::Int
        }
        ChType::Int64 | ChType::UInt32 => Storage::Long,
        ChType::UInt64 => Storage::Decimal(20, 0),
        ChType::Float32 => Storage::Float,
        ChType::Float64 => Storage::Double,
        ChType::Decimal { precision, scale } if *precision <= 38 => {
            Storage::Decimal(*precision, *scale)
        }
        ChType::Bool => Storage::Boolean,
        ChType::String | ChType::IPv4 | ChType::IPv6 | ChType::Enum8(_) | ChType::Enum16(_) => {
            Storage::String
        }
        ChType::Json(_) if top_level => Storage::String,
        ChType::FixedString(n) => Storage::Fixed(*n),
        ChType::Date | ChType::Date32 => Storage::Date,
        ChType::DateTime(_) => Storage::TimestampTz,
        ChType::DateTime64(p, _) if *p <= 6 => Storage::TimestampTz,
        ChType::DateTime64(..) if top_level => Storage::LongNanos,
        ChType::DateTime64(..) => Storage::TimestampTzNs,
        ChType::Uuid => Storage::Uuid,
        ChType::LowCardinality(inner) | ChType::Nullable(inner) => storage(inner, top_level)?,
        ChType::SimpleAggregateFunction(spec) => storage(&spec.args[0], top_level)?,
        ChType::AggregateFunction(_) => Storage::Binary,
        ChType::Array(inner) => Storage::List(Box::new(storage(inner, false)?)),
        ChType::Map(k, v) => {
            Storage::Map(Box::new(storage(k, false)?), Box::new(storage(v, false)?))
        }
        ChType::Tuple(elements) => Storage::Struct(
            elements
                .iter()
                .enumerate()
                .map(|(i, (name, et))| {
                    Ok((
                        name.clone().unwrap_or_else(|| (i + 1).to_string()),
                        storage(et, false)?,
                    ))
                })
                .collect::<Result<_, ChError>>()?,
        ),
        other => return Err(ChError::unsupported(format!("Type {other} in lake tables"))),
    })
}
