//! Default values: what ClickHouse puts where a value is missing and the type
//! is not `Nullable` (`join_use_nulls = 0`, a missing map key, `-OrDefault`,
//! `LEFT ARRAY JOIN` of an empty array, a column added by `ALTER`).
//!
//! As `defaultValueOfTypeName` answers on the 26.9.2.8 oracle: zero, the empty
//! string, zero bytes, 1970-01-01, the enum's smallest value, `NULL` for
//! `Nullable`, empty arrays and maps, a tuple of its elements' defaults.

use std::sync::Arc;

use arrow_array::types::Int32Type;
use arrow_array::{
    ArrayRef, BooleanArray, Date32Array, Decimal128Array, Decimal256Array, DictionaryArray,
    FixedSizeBinaryArray, Float32Array, Float64Array, Int8Array, Int16Array, Int32Array,
    Int64Array, ListArray, MapArray, StringArray, StructArray, TimestampMicrosecondArray,
    TimestampMillisecondArray, TimestampNanosecondArray, TimestampSecondArray, UInt8Array,
    UInt16Array, UInt32Array, UInt64Array, new_empty_array, new_null_array,
};
use arrow_buffer::{OffsetBuffer, i256};
use arrow_schema::{DataType, TimeUnit};

use crate::arrow::{data_type, time_unit};
use crate::error::ChError;
use crate::ty::ChType;

/// Days from 1970-01-01 to 1900-01-01, `Date32`'s smallest value.
pub const DATE32_MIN: i32 = -25567;

/// A `JSON` column's default: an object holding each typed path's default,
/// nested at its dots (`JSON(a String, b.c UInt32)` is `{"a":"","b":{"c":0}}`).
fn json_default(params: &crate::ty::JsonParams) -> Result<String, ChError> {
    use std::collections::BTreeMap;
    enum Node {
        Leaf(String),
        Object(BTreeMap<String, Node>),
    }
    fn render(node: &BTreeMap<String, Node>, out: &mut String) {
        out.push('{');
        for (i, (k, v)) in node.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&format!("{k:?}:"));
            match v {
                Node::Leaf(text) => out.push_str(text),
                Node::Object(inner) => render(inner, out),
            }
        }
        out.push('}');
    }
    let settings = crate::text::TextSettings::default();
    let mut root: BTreeMap<String, Node> = BTreeMap::new();
    for (path, t) in &params.typed_paths {
        let mut text = Vec::new();
        crate::text::write_value(
            t,
            default_array(t)?.as_ref(),
            0,
            crate::text::Style::Json,
            &settings,
            &mut text,
        )?;
        let parts: Vec<&str> = path.split('.').collect();
        let mut node = &mut root;
        for p in &parts[..parts.len() - 1] {
            let entry = node
                .entry((*p).to_string())
                .or_insert_with(|| Node::Object(BTreeMap::new()));
            node = match entry {
                Node::Object(inner) => inner,
                Node::Leaf(_) => {
                    return Err(ChError::bad_arguments(format!(
                        "JSON path {path} is under a typed value"
                    )));
                }
            };
        }
        node.insert(
            parts[parts.len() - 1].to_string(),
            Node::Leaf(String::from_utf8_lossy(&text).into_owned()),
        );
    }
    let mut out = String::new();
    render(&root, &mut out);
    Ok(out)
}

/// A one-row array holding `t`'s default value, in the engine's layout.
pub fn default_array(t: &ChType) -> Result<ArrayRef, ChError> {
    Ok(match t {
        ChType::UInt8 => Arc::new(UInt8Array::from(vec![0])),
        ChType::UInt16 => Arc::new(UInt16Array::from(vec![0])),
        ChType::UInt32 | ChType::IPv4 => Arc::new(UInt32Array::from(vec![0])),
        ChType::UInt64 => Arc::new(UInt64Array::from(vec![0])),
        ChType::Int8 => Arc::new(Int8Array::from(vec![0])),
        ChType::Int16 => Arc::new(Int16Array::from(vec![0])),
        ChType::Int32 => Arc::new(Int32Array::from(vec![0])),
        ChType::Int64 | ChType::Interval(_) => Arc::new(Int64Array::from(vec![0])),
        ChType::Float32 => Arc::new(Float32Array::from(vec![0.0])),
        ChType::Float64 => Arc::new(Float64Array::from(vec![0.0])),
        ChType::Bool => Arc::new(BooleanArray::from(vec![false])),
        ChType::Decimal { precision, scale } if *precision <= 38 => Arc::new(
            Decimal128Array::from(vec![0i128])
                .with_precision_and_scale(*precision, *scale as i8)
                .map_err(|e| ChError::logical_error(e.to_string()))?,
        ),
        ChType::Decimal { precision, scale } => Arc::new(
            Decimal256Array::from(vec![i256::ZERO])
                .with_precision_and_scale(*precision, *scale as i8)
                .map_err(|e| ChError::logical_error(e.to_string()))?,
        ),
        ChType::String => Arc::new(StringArray::from(vec![""])),
        ChType::Json(params) => Arc::new(StringArray::from(vec![json_default(params)?])),
        ChType::FixedString(n) => fixed(*n as usize)?,
        ChType::Uuid | ChType::IPv6 => fixed(16)?,
        ChType::Date => Arc::new(Date32Array::from(vec![0])),
        // Date32's default is its smallest value, 1900-01-01 (measured).
        ChType::Date32 => Arc::new(Date32Array::from(vec![DATE32_MIN])),
        ChType::DateTime(tz) => {
            Arc::new(TimestampSecondArray::from(vec![0]).with_timezone_opt(tz.clone()))
        }
        ChType::DateTime64(p, tz) => match time_unit(*p) {
            TimeUnit::Second => {
                Arc::new(TimestampSecondArray::from(vec![0]).with_timezone_opt(tz.clone()))
            }
            TimeUnit::Millisecond => {
                Arc::new(TimestampMillisecondArray::from(vec![0]).with_timezone_opt(tz.clone()))
            }
            TimeUnit::Microsecond => {
                Arc::new(TimestampMicrosecondArray::from(vec![0]).with_timezone_opt(tz.clone()))
            }
            TimeUnit::Nanosecond => {
                Arc::new(TimestampNanosecondArray::from(vec![0]).with_timezone_opt(tz.clone()))
            }
        },
        ChType::Enum8(values) => Arc::new(Int8Array::from(vec![
            values.iter().map(|(_, v)| *v).min().unwrap_or(0),
        ])),
        ChType::Enum16(values) => Arc::new(Int16Array::from(vec![
            values.iter().map(|(_, v)| *v).min().unwrap_or(0),
        ])),
        ChType::Nothing | ChType::Nullable(_) => new_null_array(&data_type(t)?.0, 1),
        ChType::LowCardinality(inner) => {
            let values = default_array(inner)?;
            Arc::new(
                DictionaryArray::<Int32Type>::try_new(Int32Array::from(vec![0]), values)
                    .map_err(|e| ChError::logical_error(e.to_string()))?,
            )
        }
        ChType::Array(_) => {
            let DataType::List(field) = data_type(t)?.0 else {
                return Err(ChError::logical_error("Array is not a List"));
            };
            let child = new_empty_array(field.data_type());
            Arc::new(ListArray::new(
                field,
                OffsetBuffer::new(vec![0i32, 0].into()),
                child,
                None,
            ))
        }
        ChType::Map(..) => {
            let DataType::Map(field, sorted) = data_type(t)?.0 else {
                return Err(ChError::logical_error("Map is not a Map"));
            };
            let DataType::Struct(fields) = field.data_type() else {
                return Err(ChError::logical_error("Map entries are not a Struct"));
            };
            let entries = StructArray::new_empty_fields(0, None);
            let entries = if fields.is_empty() {
                entries
            } else {
                StructArray::try_new(
                    fields.clone(),
                    fields
                        .iter()
                        .map(|f| new_empty_array(f.data_type()))
                        .collect(),
                    None,
                )
                .map_err(|e| ChError::logical_error(e.to_string()))?
            };
            Arc::new(
                MapArray::try_new(
                    field,
                    OffsetBuffer::new(vec![0i32, 0].into()),
                    entries,
                    None,
                    sorted,
                )
                .map_err(|e| ChError::logical_error(e.to_string()))?,
            )
        }
        ChType::Tuple(elements) => {
            let DataType::Struct(fields) = data_type(t)?.0 else {
                return Err(ChError::logical_error("Tuple is not a Struct"));
            };
            let children = elements
                .iter()
                .map(|(_, et)| default_array(et))
                .collect::<Result<Vec<_>, _>>()?;
            Arc::new(
                StructArray::try_new(fields, children, None)
                    .map_err(|e| ChError::logical_error(e.to_string()))?,
            )
        }
        ChType::SimpleAggregateFunction(spec) => default_array(&spec.args[0])?,
        ChType::AggregateFunction(_)
        | ChType::UInt128
        | ChType::UInt256
        | ChType::Int128
        | ChType::Int256
        | ChType::BFloat16
        | ChType::Nested(_)
        | ChType::Other(_) => {
            return Err(ChError::unsupported(format!("The default value of {t}")));
        }
    })
}

fn fixed(n: usize) -> Result<ArrayRef, ChError> {
    Ok(Arc::new(
        FixedSizeBinaryArray::try_from_iter(std::iter::once(vec![0u8; n]))
            .map_err(|e| ChError::logical_error(e.to_string()))?,
    ))
}
