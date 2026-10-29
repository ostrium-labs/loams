//! One value of any ClickHouse type the engine stores, read from and built
//! into the engine's Arrow layout (plan HE1 Task 21's table).
//!
//! Conversions run row at a time over [`Value`]s (R33.2): exact first, fast
//! later. A column is read with [`read`] and built with [`build`].

use std::sync::Arc;

use datafusion::arrow::array::{
    Array, ArrayRef, AsArray, ListArray, StructArray, new_empty_array, new_null_array,
};
use datafusion::arrow::buffer::{NullBuffer, OffsetBuffer, ScalarBuffer};
use datafusion::arrow::compute::cast;
use datafusion::arrow::datatypes::{
    DataType, Date32Type, Decimal128Type, Decimal256Type, Float32Type, Float64Type, Int8Type,
    Int16Type, Int32Type, Int64Type, TimeUnit, TimestampMicrosecondType, TimestampMillisecondType,
    TimestampNanosecondType, TimestampSecondType, UInt8Type, UInt16Type, UInt32Type, UInt64Type,
    i256,
};
use datafusion::common::ScalarValue;
use loams_house_types::arrow::{data_type, ticks_per_unit, to_arrow};
use loams_house_types::{ChError, ChType};

use crate::func::typed::arrow_error;

/// A value in flight between two ClickHouse types.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// `NULL`.
    Null,
    /// Any integer, or an `Interval`.
    Int(i128),
    /// `Float32` or `Float64`.
    Float(f64),
    /// `Bool`.
    Bool(bool),
    /// A decimal: its unscaled integer and scale.
    Dec(i128, u8),
    /// `String` or `FixedString` bytes.
    Str(Vec<u8>),
    /// `Date` or `Date32`: days since 1970-01-01.
    Date(i32),
    /// `DateTime` (precision 0) or `DateTime64(p)`: ticks of 10^-p seconds.
    Time(i64, u8),
    /// `UUID`, in text order.
    Uuid([u8; 16]),
    /// `IPv4`.
    Ip4(u32),
    /// `IPv6`.
    Ip6([u8; 16]),
    /// An `Enum` element: its value and name.
    Enum(i64, String),
    /// `Array(T)`.
    Array(Vec<Value>),
    /// `Tuple(…)`.
    Tuple(Vec<Value>),
}

impl Value {
    /// Whether this is `NULL`.
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
}

fn bad(t: &ChType, array: &dyn Array) -> ChError {
    ChError::logical_error(format!(
        "A column of {t} holds an Arrow {} array",
        array.data_type()
    ))
}

/// The bytes of row `row` of a string-like array.
pub fn bytes_at(array: &dyn Array, row: usize) -> Option<&[u8]> {
    Some(match array.data_type() {
        DataType::Utf8 => array.as_string::<i32>().value(row).as_bytes(),
        DataType::LargeUtf8 => array.as_string::<i64>().value(row).as_bytes(),
        DataType::Utf8View => array.as_string_view().value(row).as_bytes(),
        DataType::Binary => array.as_binary::<i32>().value(row),
        DataType::LargeBinary => array.as_binary::<i64>().value(row),
        DataType::BinaryView => array.as_binary_view().value(row),
        DataType::FixedSizeBinary(_) => array.as_fixed_size_binary().value(row),
        _ => return None,
    })
}

/// Row `row` of `array` as an integer, whatever its integer Arrow type.
pub fn int_at(array: &dyn Array, row: usize) -> Option<i128> {
    Some(match array.data_type() {
        DataType::Int8 => i128::from(array.as_primitive::<Int8Type>().value(row)),
        DataType::Int16 => i128::from(array.as_primitive::<Int16Type>().value(row)),
        DataType::Int32 => i128::from(array.as_primitive::<Int32Type>().value(row)),
        DataType::Int64 => i128::from(array.as_primitive::<Int64Type>().value(row)),
        DataType::UInt8 => i128::from(array.as_primitive::<UInt8Type>().value(row)),
        DataType::UInt16 => i128::from(array.as_primitive::<UInt16Type>().value(row)),
        DataType::UInt32 => i128::from(array.as_primitive::<UInt32Type>().value(row)),
        DataType::UInt64 => i128::from(array.as_primitive::<UInt64Type>().value(row)),
        DataType::Boolean => i128::from(array.as_boolean().value(row)),
        _ => return None,
    })
}

/// Row `row` of `array` as a float, whatever its float or integer type.
pub fn float_at(array: &dyn Array, row: usize) -> Option<f64> {
    Some(match array.data_type() {
        DataType::Float32 => f64::from(array.as_primitive::<Float32Type>().value(row)),
        DataType::Float64 => array.as_primitive::<Float64Type>().value(row),
        _ => int_at(array, row)? as f64,
    })
}

/// Row `row` of a timestamp array, in ticks of 10^-`precision` seconds.
fn ticks_at(array: &dyn Array, row: usize, precision: u8) -> Option<i64> {
    let (raw, unit_digits) = match array.data_type() {
        DataType::Timestamp(TimeUnit::Second, _) => {
            (array.as_primitive::<TimestampSecondType>().value(row), 0u8)
        }
        DataType::Timestamp(TimeUnit::Millisecond, _) => (
            array.as_primitive::<TimestampMillisecondType>().value(row),
            3,
        ),
        DataType::Timestamp(TimeUnit::Microsecond, _) => (
            array.as_primitive::<TimestampMicrosecondType>().value(row),
            6,
        ),
        DataType::Timestamp(TimeUnit::Nanosecond, _) => (
            array.as_primitive::<TimestampNanosecondType>().value(row),
            9,
        ),
        // A `DateTime` some writer kept as its seconds.
        _ => return int_at(array, row).map(|v| v as i64),
    };
    Some(rescale_ticks(raw, unit_digits, precision))
}

/// `ticks` of 10^-`from` seconds as ticks of 10^-`to` seconds (rounding
/// towards negative infinity when precision is lost).
pub fn rescale_ticks(ticks: i64, from: u8, to: u8) -> i64 {
    if from == to {
        ticks
    } else if from < to {
        ticks.saturating_mul(10i64.pow(u32::from(to - from)))
    } else {
        ticks.div_euclid(10i64.pow(u32::from(from - to)))
    }
}

/// Row `row` of `array`, a column of `t` (`Nullable` and `LowCardinality`
/// allowed; dictionaries are read through their keys).
pub fn read(array: &dyn Array, t: &ChType, row: usize) -> Result<Value, ChError> {
    if let DataType::Dictionary(..) = array.data_type() {
        let dict = array.as_any_dictionary();
        if dict.keys().is_null(row) {
            return Ok(Value::Null);
        }
        let key = dict.normalized_keys()[row];
        return read(dict.values().as_ref(), t, key);
    }
    if array.is_null(row) {
        return Ok(Value::Null);
    }
    let t = match t {
        ChType::Nullable(inner) | ChType::LowCardinality(inner) => {
            return read(array, inner, row);
        }
        ChType::SimpleAggregateFunction(spec) => {
            return match spec.args.first() {
                Some(inner) => read(array, inner, row),
                None => Err(bad(t, array)),
            };
        }
        other => other,
    };
    Ok(match t {
        ChType::UInt8
        | ChType::UInt16
        | ChType::UInt32
        | ChType::UInt64
        | ChType::Int8
        | ChType::Int16
        | ChType::Int32
        | ChType::Int64
        | ChType::Interval(_) => match array.data_type() {
            DataType::Float32 | DataType::Float64 => {
                Value::Int(float_at(array, row).ok_or_else(|| bad(t, array))? as i128)
            }
            _ => Value::Int(int_at(array, row).ok_or_else(|| bad(t, array))?),
        },
        ChType::Float32 | ChType::Float64 => {
            Value::Float(float_at(array, row).ok_or_else(|| bad(t, array))?)
        }
        ChType::Bool => match array.data_type() {
            DataType::Boolean => Value::Bool(array.as_boolean().value(row)),
            _ => Value::Bool(int_at(array, row).ok_or_else(|| bad(t, array))? != 0),
        },
        ChType::Decimal { scale, .. } => match array.data_type() {
            DataType::Decimal128(_, s) => Value::Dec(
                array.as_primitive::<Decimal128Type>().value(row),
                (*s).max(0) as u8,
            ),
            DataType::Decimal256(_, s) => {
                let v = array.as_primitive::<Decimal256Type>().value(row);
                let v = v.to_i128().ok_or_else(|| {
                    ChError::unsupported(format!("The Decimal value {v} wider than 128 bits"))
                })?;
                Value::Dec(v, (*s).max(0) as u8)
            }
            _ => Value::Dec(int_at(array, row).ok_or_else(|| bad(t, array))?, *scale),
        },
        ChType::String | ChType::Json(_) => {
            Value::Str(bytes_at(array, row).ok_or_else(|| bad(t, array))?.to_vec())
        }
        ChType::FixedString(_) => {
            Value::Str(bytes_at(array, row).ok_or_else(|| bad(t, array))?.to_vec())
        }
        ChType::Date | ChType::Date32 => match array.data_type() {
            DataType::Date32 => Value::Date(array.as_primitive::<Date32Type>().value(row)),
            _ => Value::Date(int_at(array, row).ok_or_else(|| bad(t, array))? as i32),
        },
        ChType::DateTime(_) => {
            Value::Time(ticks_at(array, row, 0).ok_or_else(|| bad(t, array))?, 0)
        }
        ChType::DateTime64(p, _) => {
            Value::Time(ticks_at(array, row, *p).ok_or_else(|| bad(t, array))?, *p)
        }
        ChType::Uuid => Value::Uuid(sixteen(bytes_at(array, row)).ok_or_else(|| bad(t, array))?),
        ChType::IPv6 => Value::Ip6(sixteen(bytes_at(array, row)).ok_or_else(|| bad(t, array))?),
        ChType::IPv4 => Value::Ip4(int_at(array, row).ok_or_else(|| bad(t, array))? as u32),
        ChType::Enum8(members) => {
            let v = int_at(array, row).ok_or_else(|| bad(t, array))? as i64;
            let name = members
                .iter()
                .find(|(_, m)| i64::from(*m) == v)
                .map(|(n, _)| n.clone())
                .unwrap_or_default();
            Value::Enum(v, name)
        }
        ChType::Enum16(members) => {
            let v = int_at(array, row).ok_or_else(|| bad(t, array))? as i64;
            let name = members
                .iter()
                .find(|(_, m)| i64::from(*m) == v)
                .map(|(n, _)| n.clone())
                .unwrap_or_default();
            Value::Enum(v, name)
        }
        ChType::Array(inner) => {
            let list = array.as_list_opt::<i32>().ok_or_else(|| bad(t, array))?;
            let items = list.value(row);
            let mut out = Vec::with_capacity(items.len());
            for i in 0..items.len() {
                out.push(read(items.as_ref(), inner, i)?);
            }
            Value::Array(out)
        }
        ChType::Tuple(elements) => {
            let s = array.as_struct_opt().ok_or_else(|| bad(t, array))?;
            let mut out = Vec::with_capacity(elements.len());
            for (i, (_, et)) in elements.iter().enumerate() {
                out.push(read(s.column(i).as_ref(), et, row)?);
            }
            Value::Tuple(out)
        }
        ChType::Nothing => Value::Null,
        other => {
            return Err(ChError::unsupported(format!(
                "Converting values of type {other}"
            )));
        }
    })
}

fn sixteen(b: Option<&[u8]>) -> Option<[u8; 16]> {
    b?.try_into().ok()
}

/// A column of `t` (any type the engine stores) holding `values`.
pub fn build(t: &ChType, values: &[Value]) -> Result<ArrayRef, ChError> {
    let (dt, _) = data_type(t)?;
    if values.is_empty() {
        return Ok(new_empty_array(&dt));
    }
    match t {
        ChType::Nullable(inner) => build(inner, values),
        ChType::LowCardinality(inner) => {
            let plain = build(inner, values)?;
            cast(&plain, &dt).map_err(arrow_error)
        }
        ChType::Nothing => Ok(new_null_array(&DataType::Null, values.len())),
        ChType::Array(inner) => {
            let mut offsets = Vec::with_capacity(values.len() + 1);
            offsets.push(0i32);
            let mut flat = Vec::new();
            let mut valid = Vec::with_capacity(values.len());
            for v in values {
                match v {
                    Value::Array(items) => {
                        flat.extend(items.iter().cloned());
                        valid.push(true);
                    }
                    _ => valid.push(false),
                }
                let len = i32::try_from(flat.len()).map_err(|_| {
                    ChError::unsupported("An array column of more than 2^31 elements")
                })?;
                offsets.push(len);
            }
            let child = build(inner, &flat)?;
            let field = Arc::new(to_arrow("item", inner)?);
            let nulls = valid.iter().any(|v| !v).then(|| NullBuffer::from(valid));
            let list = ListArray::try_new(
                field,
                OffsetBuffer::new(ScalarBuffer::from(offsets)),
                child,
                nulls,
            )
            .map_err(arrow_error)?;
            Ok(Arc::new(list))
        }
        ChType::Tuple(elements) => {
            let DataType::Struct(fields) = dt else {
                return Err(ChError::logical_error(format!("{t} is not a struct")));
            };
            let mut columns = Vec::with_capacity(elements.len());
            for (i, (_, et)) in elements.iter().enumerate() {
                let part: Vec<Value> = values
                    .iter()
                    .map(|v| match v {
                        Value::Tuple(items) => items.get(i).cloned().unwrap_or(Value::Null),
                        _ => default_value(et),
                    })
                    .collect();
                columns.push(build(et, &part)?);
            }
            let s = StructArray::try_new(fields, columns, None).map_err(arrow_error)?;
            Ok(Arc::new(s))
        }
        _ => {
            let mut scalars = Vec::with_capacity(values.len());
            for v in values {
                scalars.push(scalar(t, &dt, v)?);
            }
            ScalarValue::iter_to_array(scalars).map_err(|e| ChError::logical_error(e.to_string()))
        }
    }
}

/// `v` as a scalar of `t`'s Arrow type `dt` (`t` a scalar type).
fn scalar(t: &ChType, dt: &DataType, v: &Value) -> Result<ScalarValue, ChError> {
    if v.is_null() {
        return ScalarValue::try_from(dt).map_err(|e| ChError::logical_error(e.to_string()));
    }
    let wrong = || ChError::logical_error(format!("A {v:?} cannot be stored in a column of {t}"));
    let int = || match v {
        Value::Int(i) => Ok(*i),
        Value::Bool(b) => Ok(i128::from(*b)),
        Value::Enum(i, _) => Ok(i128::from(*i)),
        Value::Ip4(i) => Ok(i128::from(*i)),
        Value::Date(d) => Ok(i128::from(*d)),
        Value::Time(s, _) => Ok(i128::from(*s)),
        _ => Err(wrong()),
    };
    Ok(match t {
        ChType::UInt8 => ScalarValue::UInt8(Some(int()? as u8)),
        ChType::UInt16 => ScalarValue::UInt16(Some(int()? as u16)),
        ChType::UInt32 => ScalarValue::UInt32(Some(int()? as u32)),
        ChType::UInt64 => ScalarValue::UInt64(Some(int()? as u64)),
        ChType::Int8 => ScalarValue::Int8(Some(int()? as i8)),
        ChType::Int16 => ScalarValue::Int16(Some(int()? as i16)),
        ChType::Int32 => ScalarValue::Int32(Some(int()? as i32)),
        ChType::Int64 | ChType::Interval(_) => ScalarValue::Int64(Some(int()? as i64)),
        ChType::Float32 => match v {
            Value::Float(f) => ScalarValue::Float32(Some(*f as f32)),
            _ => ScalarValue::Float32(Some(int()? as f32)),
        },
        ChType::Float64 => match v {
            Value::Float(f) => ScalarValue::Float64(Some(*f)),
            _ => ScalarValue::Float64(Some(int()? as f64)),
        },
        ChType::Bool => match v {
            Value::Bool(b) => ScalarValue::Boolean(Some(*b)),
            _ => ScalarValue::Boolean(Some(int()? != 0)),
        },
        ChType::Decimal { precision, scale } => {
            let unscaled = match v {
                Value::Dec(u, _) => *u,
                _ => int()?,
            };
            if *precision <= 38 {
                ScalarValue::Decimal128(Some(unscaled), *precision, *scale as i8)
            } else {
                ScalarValue::Decimal256(Some(i256::from_i128(unscaled)), *precision, *scale as i8)
            }
        }
        ChType::String | ChType::Json(_) => match v {
            Value::Str(b) => ScalarValue::Utf8(Some(String::from_utf8_lossy(b).into_owned())),
            _ => return Err(wrong()),
        },
        ChType::FixedString(n) => match v {
            Value::Str(b) => {
                let mut b = b.clone();
                b.resize(*n as usize, 0);
                ScalarValue::FixedSizeBinary(*n as i32, Some(b))
            }
            _ => return Err(wrong()),
        },
        ChType::Uuid => match v {
            Value::Uuid(b) => ScalarValue::FixedSizeBinary(16, Some(b.to_vec())),
            _ => return Err(wrong()),
        },
        ChType::IPv6 => match v {
            Value::Ip6(b) => ScalarValue::FixedSizeBinary(16, Some(b.to_vec())),
            _ => return Err(wrong()),
        },
        ChType::IPv4 => ScalarValue::UInt32(Some(int()? as u32)),
        ChType::Date | ChType::Date32 => ScalarValue::Date32(Some(int()? as i32)),
        ChType::DateTime(tz) => {
            let secs = match v {
                Value::Time(s, p) => rescale_ticks(*s, *p, 0),
                _ => int()? as i64,
            };
            ScalarValue::TimestampSecond(Some(secs), tz.as_deref().map(Arc::from))
        }
        ChType::DateTime64(p, tz) => {
            let ticks = match v {
                Value::Time(s, from) => rescale_ticks(*s, *from, *p),
                _ => int()? as i64,
            };
            let raw = ticks.saturating_mul(ticks_per_unit(*p));
            let tz = tz.as_deref().map(Arc::from);
            match dt {
                DataType::Timestamp(TimeUnit::Second, _) => {
                    ScalarValue::TimestampSecond(Some(raw), tz)
                }
                DataType::Timestamp(TimeUnit::Millisecond, _) => {
                    ScalarValue::TimestampMillisecond(Some(raw), tz)
                }
                DataType::Timestamp(TimeUnit::Microsecond, _) => {
                    ScalarValue::TimestampMicrosecond(Some(raw), tz)
                }
                _ => ScalarValue::TimestampNanosecond(Some(raw), tz),
            }
        }
        ChType::Enum8(_) => ScalarValue::Int8(Some(int()? as i8)),
        ChType::Enum16(_) => ScalarValue::Int16(Some(int()? as i16)),
        other => {
            return Err(ChError::unsupported(format!(
                "Building values of type {other}"
            )));
        }
    })
}

/// ClickHouse's default value of `t` (`toXOrZero`'s zero, a missing JSON
/// value's, a tuple element's).
pub fn default_value(t: &ChType) -> Value {
    match t {
        ChType::Nullable(_) | ChType::Nothing => Value::Null,
        ChType::LowCardinality(inner) => default_value(inner),
        ChType::Float32 | ChType::Float64 => Value::Float(0.0),
        ChType::Bool => Value::Bool(false),
        ChType::Decimal { scale, .. } => Value::Dec(0, *scale),
        ChType::String | ChType::Json(_) => Value::Str(Vec::new()),
        ChType::FixedString(n) => Value::Str(vec![0; *n as usize]),
        ChType::Date | ChType::Date32 => Value::Date(0),
        ChType::DateTime(_) => Value::Time(0, 0),
        ChType::DateTime64(p, _) => Value::Time(0, *p),
        ChType::Uuid => Value::Uuid([0; 16]),
        ChType::IPv4 => Value::Ip4(0),
        ChType::IPv6 => Value::Ip6([0; 16]),
        // The first element in value order (`DataTypeEnum::getDefault`).
        ChType::Enum8(members) => members
            .iter()
            .min_by_key(|(_, v)| *v)
            .map(|(n, v)| Value::Enum(i64::from(*v), n.clone()))
            .unwrap_or(Value::Int(0)),
        ChType::Enum16(members) => members
            .iter()
            .min_by_key(|(_, v)| *v)
            .map(|(n, v)| Value::Enum(i64::from(*v), n.clone()))
            .unwrap_or(Value::Int(0)),
        ChType::Array(_) => Value::Array(Vec::new()),
        ChType::Tuple(elements) => {
            Value::Tuple(elements.iter().map(|(_, t)| default_value(t)).collect())
        }
        _ => Value::Int(0),
    }
}

/// Every row of `array` (a column of `t`).
pub fn read_all(array: &dyn Array, t: &ChType) -> Result<Vec<Value>, ChError> {
    (0..array.len()).map(|row| read(array, t, row)).collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use datafusion::arrow::array::{Int64Array, StringArray};
    use loams_house_types::parse;

    #[test]
    fn values_round_trip_through_columns() {
        for (text, values) in [
            ("UInt8", vec![Value::Int(1), Value::Int(255)]),
            ("Nullable(Int32)", vec![Value::Int(-5), Value::Null]),
            ("Float64", vec![Value::Float(1.5)]),
            ("String", vec![Value::Str(b"ab".to_vec())]),
            ("Decimal(18, 2)", vec![Value::Dec(1234, 2)]),
            ("Date", vec![Value::Date(20_000)]),
            ("DateTime('UTC')", vec![Value::Time(1_700_000_000, 0)]),
            ("DateTime64(3)", vec![Value::Time(1_700_000_000_123, 3)]),
            (
                "Array(Nullable(UInt8))",
                vec![
                    Value::Array(vec![Value::Int(1), Value::Null]),
                    Value::Array(vec![]),
                ],
            ),
            ("LowCardinality(String)", vec![Value::Str(b"x".to_vec())]),
            ("Bool", vec![Value::Bool(true)]),
        ] {
            let t = parse(text).unwrap();
            let column = build(&t, &values).unwrap();
            assert_eq!(column.data_type(), &data_type(&t).unwrap().0, "{text}");
            assert_eq!(read_all(column.as_ref(), &t).unwrap(), values, "{text}");
        }
    }

    #[test]
    fn enums_read_their_names() {
        let t = parse("Enum8('a' = 1, 'b' = 2)").unwrap();
        let column = build(&t, &[Value::Int(2)]).unwrap();
        assert_eq!(
            read(column.as_ref(), &t, 0).unwrap(),
            Value::Enum(2, "b".into())
        );
        assert_eq!(default_value(&t), Value::Enum(1, "a".into()));
    }

    #[test]
    fn plain_arrow_columns_read() {
        let ints: ArrayRef = Arc::new(Int64Array::from(vec![Some(7), None]));
        assert_eq!(
            read(ints.as_ref(), &ChType::Int64, 0).unwrap(),
            Value::Int(7)
        );
        assert_eq!(read(ints.as_ref(), &ChType::Int64, 1).unwrap(), Value::Null);
        let strings: ArrayRef = Arc::new(StringArray::from(vec!["q"]));
        assert_eq!(
            read(strings.as_ref(), &ChType::String, 0).unwrap(),
            Value::Str(b"q".to_vec())
        );
        assert_eq!(rescale_ticks(1_500, 3, 0), 1);
        assert_eq!(rescale_ticks(-1_500, 3, 0), -2);
        assert_eq!(rescale_ticks(2, 0, 3), 2_000);
    }
}
