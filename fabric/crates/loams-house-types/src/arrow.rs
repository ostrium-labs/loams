//! The engine's Arrow layout of ClickHouse types (plan HE1 Task 21's table).
//!
//! Each field carries its ClickHouse type's compact canonical text in the
//! metadata key [`CH_TYPE`], at every level, so a column keeps its exact type
//! (`Enum8(…)`, `LowCardinality(…)`, `DateTime64(2)`) through DataFusion's
//! plans; [`from_arrow`] reads it back, and infers a type from Arrow's alone
//! for data that has none (another writer's tables).

use std::collections::HashMap;
use std::sync::Arc;

use arrow_schema::{DataType, Field, Fields, TimeUnit};

use crate::error::ChError;
use crate::ty::ChType;

/// The field metadata key that holds a column's ClickHouse type.
pub const CH_TYPE: &str = "loams.ch.type";

/// The Arrow time unit a `DateTime64(precision)` is stored in: seconds for 0,
/// milliseconds for 1–3, microseconds for 4–6, nanoseconds for 7–9.
pub fn time_unit(precision: u8) -> TimeUnit {
    match precision {
        0 => TimeUnit::Second,
        1..=3 => TimeUnit::Millisecond,
        4..=6 => TimeUnit::Microsecond,
        _ => TimeUnit::Nanosecond,
    }
}

/// How many of the unit's ticks one ClickHouse tick of `DateTime64(precision)`
/// is (`DateTime64(2)` in milliseconds: 10).
pub fn ticks_per_unit(precision: u8) -> i64 {
    let unit_digits = match time_unit(precision) {
        TimeUnit::Second => 0,
        TimeUnit::Millisecond => 3,
        TimeUnit::Microsecond => 6,
        TimeUnit::Nanosecond => 9,
    };
    10i64.pow(u32::from(unit_digits - precision.min(unit_digits)))
}

/// A field named `name` holding `t`, with the ClickHouse type in its metadata.
pub fn to_arrow(name: &str, t: &ChType) -> Result<Field, ChError> {
    let (data_type, nullable) = data_type(t)?;
    Ok(with_type(Field::new(name, data_type, nullable), t))
}

fn with_type(field: Field, t: &ChType) -> Field {
    let mut metadata = HashMap::new();
    metadata.insert(CH_TYPE.to_string(), t.to_compact_string());
    field.with_metadata(metadata)
}

/// The Arrow data type of `t`, and whether its values may be `NULL`.
pub fn data_type(t: &ChType) -> Result<(DataType, bool), ChError> {
    let dt = match t {
        ChType::UInt8 => DataType::UInt8,
        ChType::UInt16 => DataType::UInt16,
        ChType::UInt32 => DataType::UInt32,
        ChType::UInt64 => DataType::UInt64,
        ChType::Int8 => DataType::Int8,
        ChType::Int16 => DataType::Int16,
        ChType::Int32 => DataType::Int32,
        ChType::Int64 => DataType::Int64,
        ChType::Float32 => DataType::Float32,
        ChType::Float64 => DataType::Float64,
        ChType::Bool => DataType::Boolean,
        ChType::Decimal { precision, scale } if *precision <= 38 => {
            DataType::Decimal128(*precision, *scale as i8)
        }
        ChType::Decimal { precision, scale } => DataType::Decimal256(*precision, *scale as i8),
        ChType::String | ChType::Json(_) => DataType::Utf8,
        ChType::FixedString(n) => DataType::FixedSizeBinary(*n as i32),
        ChType::Date | ChType::Date32 => DataType::Date32,
        ChType::DateTime(tz) => DataType::Timestamp(TimeUnit::Second, tz.as_deref().map(Arc::from)),
        ChType::DateTime64(p, tz) => {
            DataType::Timestamp(time_unit(*p), tz.as_deref().map(Arc::from))
        }
        ChType::Uuid | ChType::IPv6 => DataType::FixedSizeBinary(16),
        ChType::IPv4 => DataType::UInt32,
        ChType::Enum8(_) => DataType::Int8,
        ChType::Enum16(_) => DataType::Int16,
        ChType::Interval(_) => DataType::Int64,
        ChType::Nothing => return Ok((DataType::Null, true)),
        ChType::Nullable(inner) => return Ok((data_type(inner)?.0, true)),
        ChType::LowCardinality(inner) => {
            let (values, nullable) = data_type(inner)?;
            return Ok((
                DataType::Dictionary(Box::new(DataType::Int32), Box::new(values)),
                nullable,
            ));
        }
        ChType::Array(inner) => DataType::List(Arc::new(to_arrow("item", inner)?)),
        ChType::Map(k, v) => {
            let key = to_arrow("key", k)?;
            if key.is_nullable() {
                return Err(ChError::unsupported(format!("A Nullable map key ({t})")));
            }
            let entries = Field::new(
                "entries",
                DataType::Struct(Fields::from(vec![key, to_arrow("value", v)?])),
                false,
            );
            DataType::Map(Arc::new(entries), false)
        }
        ChType::Tuple(elements) => {
            let mut fields = Vec::with_capacity(elements.len());
            for (i, (name, et)) in elements.iter().enumerate() {
                let name = name.clone().unwrap_or_else(|| (i + 1).to_string());
                fields.push(to_arrow(&name, et)?);
            }
            DataType::Struct(Fields::from(fields))
        }
        ChType::AggregateFunction(_) => DataType::Binary,
        ChType::SimpleAggregateFunction(spec) => return data_type(&spec.args[0]),
        ChType::UInt128
        | ChType::UInt256
        | ChType::Int128
        | ChType::Int256
        | ChType::BFloat16
        | ChType::Nested(_)
        | ChType::Other(_) => return Err(ChError::unsupported(format!("Type {t}"))),
    };
    Ok((dt, false))
}

/// The ClickHouse type of `field`: its metadata's when present, else one
/// inferred from the Arrow type (`Nullable` when the field is).
pub fn from_arrow(field: &Field) -> Result<ChType, ChError> {
    if let Some(text) = field.metadata().get(CH_TYPE) {
        return crate::parse::parse(text);
    }
    let t = infer(field.data_type())?;
    Ok(if field.is_nullable() { nullable(t) } else { t })
}

/// `Nullable(t)` where ClickHouse allows it, else `t`.
pub fn nullable(t: ChType) -> ChType {
    match t {
        ChType::Nullable(_)
        | ChType::Array(_)
        | ChType::Map(..)
        | ChType::Tuple(_)
        | ChType::Json(_)
        | ChType::AggregateFunction(_)
        | ChType::Nothing => t,
        ChType::LowCardinality(inner) => ChType::LowCardinality(Box::new(nullable(*inner))),
        other => ChType::Nullable(Box::new(other)),
    }
}

fn infer(dt: &DataType) -> Result<ChType, ChError> {
    Ok(match dt {
        DataType::Null => ChType::Nullable(Box::new(ChType::Nothing)),
        DataType::Boolean => ChType::Bool,
        DataType::UInt8 => ChType::UInt8,
        DataType::UInt16 => ChType::UInt16,
        DataType::UInt32 => ChType::UInt32,
        DataType::UInt64 => ChType::UInt64,
        DataType::Int8 => ChType::Int8,
        DataType::Int16 => ChType::Int16,
        DataType::Int32 => ChType::Int32,
        DataType::Int64 => ChType::Int64,
        DataType::Float16 | DataType::Float32 => ChType::Float32,
        DataType::Float64 => ChType::Float64,
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => ChType::String,
        DataType::Binary | DataType::LargeBinary | DataType::BinaryView => ChType::String,
        DataType::FixedSizeBinary(n) => ChType::FixedString(*n as u32),
        DataType::Date32 | DataType::Date64 => ChType::Date32,
        DataType::Timestamp(unit, tz) => {
            let tz = tz.as_deref().map(str::to_string);
            match unit {
                TimeUnit::Second => ChType::DateTime(tz),
                TimeUnit::Millisecond => ChType::DateTime64(3, tz),
                TimeUnit::Microsecond => ChType::DateTime64(6, tz),
                TimeUnit::Nanosecond => ChType::DateTime64(9, tz),
            }
        }
        DataType::Decimal32(p, s)
        | DataType::Decimal64(p, s)
        | DataType::Decimal128(p, s)
        | DataType::Decimal256(p, s) => ChType::Decimal {
            precision: *p,
            scale: (*s).max(0) as u8,
        },
        DataType::List(f)
        | DataType::LargeList(f)
        | DataType::ListView(f)
        | DataType::FixedSizeList(f, _) => ChType::Array(Box::new(from_arrow(f)?)),
        DataType::Map(entries, _) => match entries.data_type() {
            DataType::Struct(kv) if kv.len() == 2 => {
                let key = from_arrow(&kv[0])?;
                ChType::Map(
                    Box::new(key.strip_nullable().clone()),
                    Box::new(from_arrow(&kv[1])?),
                )
            }
            other => {
                return Err(ChError::unsupported(format!(
                    "Arrow map entries of type {other}"
                )));
            }
        },
        DataType::Struct(fields) => {
            let mut elements = Vec::with_capacity(fields.len());
            let positional = fields
                .iter()
                .enumerate()
                .all(|(i, f)| f.name() == &(i + 1).to_string());
            for f in fields {
                elements.push(((!positional).then(|| f.name().clone()), from_arrow(f)?));
            }
            ChType::Tuple(elements)
        }
        DataType::Dictionary(_, values) => ChType::LowCardinality(Box::new(infer(values)?)),
        other => return Err(ChError::unsupported(format!("Arrow type {other}"))),
    })
}
