//! Reading arguments and writing results in the engine's Arrow layout of
//! ClickHouse's date and time types (R21.3): `Date` and `Date32` are
//! `Date32` days, `DateTime` is `Timestamp(Second)`, `DateTime64(P)` is a
//! timestamp in the unit of [`loams_house_types::arrow::time_unit`], and an
//! `Interval*` is `Int64`.

use std::sync::Arc;

use datafusion::arrow::array::{
    Array, ArrayRef, AsArray, BooleanArray, Date32Array, Decimal128Array, Int32Array, Int64Array,
    StringArray, TimestampMicrosecondArray, TimestampMillisecondArray, TimestampNanosecondArray,
    TimestampSecondArray, UInt8Array, UInt16Array, UInt32Array, UInt64Array, new_null_array,
};
use datafusion::arrow::compute::cast;
use datafusion::arrow::datatypes::{
    DataType, Date32Type, Float32Type, Float64Type, Int8Type, Int16Type, Int32Type, Int64Type,
    TimeUnit, UInt8Type, UInt16Type, UInt32Type, UInt64Type,
};
use loams_house_types::arrow::{data_type, time_unit};
use loams_house_types::{ChError, ChType};

/// Which date or time type a value has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `Date`: days, 1970-01-01 to 2149-06-06.
    Date,
    /// `Date32`: days, 1900-01-01 to 2299-12-31.
    Date32,
    /// `DateTime`: seconds, 1970 to 2106.
    DateTime,
    /// `DateTime64(P)`.
    DateTime64(u8),
}

/// The [`Kind`] and the type's own zone of `t`, if it is a date or a time.
pub fn temporal(t: &ChType) -> Option<(Kind, Option<&str>)> {
    match t.base() {
        ChType::Date => Some((Kind::Date, None)),
        ChType::Date32 => Some((Kind::Date32, None)),
        ChType::DateTime(tz) => Some((Kind::DateTime, tz.as_deref())),
        ChType::DateTime64(p, tz) => Some((Kind::DateTime64(*p), tz.as_deref())),
        _ => None,
    }
}

/// A date or an instant read from a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Point {
    /// A calendar day (a `Date` or `Date32`), with no zone.
    Day(i64),
    /// An instant: seconds since the epoch and nanoseconds into the second.
    Time(i64, u32),
}

/// Nanoseconds in one tick of `unit`.
pub fn unit_nanos(unit: TimeUnit) -> i64 {
    match unit {
        TimeUnit::Second => 1_000_000_000,
        TimeUnit::Millisecond => 1_000_000,
        TimeUnit::Microsecond => 1_000,
        TimeUnit::Nanosecond => 1,
    }
}

/// `10^n`.
pub fn pow10(n: u8) -> i64 {
    10i64.pow(u32::from(n))
}

fn split(raw: i64, unit: TimeUnit) -> Point {
    let per = 1_000_000_000 / unit_nanos(unit);
    Point::Time(
        raw.div_euclid(per),
        (raw.rem_euclid(per) * unit_nanos(unit)) as u32,
    )
}

/// The dates or instants of a `Date32` or timestamp array.
///
/// # Errors
///
/// `43` for any other array.
pub fn points(array: &dyn Array) -> Result<Vec<Option<Point>>, ChError> {
    Ok(match array.data_type() {
        DataType::Date32 => array
            .as_primitive::<Date32Type>()
            .iter()
            .map(|v| v.map(|d| Point::Day(i64::from(d))))
            .collect(),
        DataType::Timestamp(unit, _) => {
            let unit = *unit;
            let raw: Vec<Option<i64>> = match unit {
                TimeUnit::Second => array
                    .as_any()
                    .downcast_ref::<TimestampSecondArray>()
                    .map(|a| a.iter().collect()),
                TimeUnit::Millisecond => array
                    .as_any()
                    .downcast_ref::<TimestampMillisecondArray>()
                    .map(|a| a.iter().collect()),
                TimeUnit::Microsecond => array
                    .as_any()
                    .downcast_ref::<TimestampMicrosecondArray>()
                    .map(|a| a.iter().collect()),
                TimeUnit::Nanosecond => array
                    .as_any()
                    .downcast_ref::<TimestampNanosecondArray>()
                    .map(|a| a.iter().collect()),
            }
            .unwrap_or_default();
            raw.into_iter().map(|v| v.map(|r| split(r, unit))).collect()
        }
        other => {
            return Err(ChError::illegal_type_of_argument(format!(
                "Expected a date or a time, got the Arrow type {other}"
            )));
        }
    })
}

/// The values of an integer, float or boolean array as `i64`: floats are
/// truncated toward zero (saturating), and `UInt64` wraps, as ClickHouse
/// reinterprets it.
pub fn ints(array: &dyn Array) -> Result<Vec<Option<i64>>, ChError> {
    macro_rules! prim {
        ($t:ty) => {
            array
                .as_primitive::<$t>()
                .iter()
                .map(|v| v.map(|x| x as i64))
                .collect()
        };
    }
    Ok(match array.data_type() {
        DataType::Int8 => prim!(Int8Type),
        DataType::Int16 => prim!(Int16Type),
        DataType::Int32 => prim!(Int32Type),
        DataType::Int64 => prim!(Int64Type),
        DataType::UInt8 => prim!(UInt8Type),
        DataType::UInt16 => prim!(UInt16Type),
        DataType::UInt32 => prim!(UInt32Type),
        DataType::UInt64 => prim!(UInt64Type),
        DataType::Float32 => prim!(Float32Type),
        DataType::Float64 => prim!(Float64Type),
        DataType::Boolean => array
            .as_boolean()
            .iter()
            .map(|v| v.map(i64::from))
            .collect(),
        DataType::Null => vec![None; array.len()],
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => strings(array)?
            .into_iter()
            .map(|s| match s {
                None => Ok(None),
                Some(s) => s.trim().parse::<i64>().map(Some).map_err(|_| {
                    ChError::cannot_parse_text(format!("Cannot parse the number {s:?}"))
                }),
            })
            .collect::<Result<_, _>>()?,
        other => {
            return Err(ChError::illegal_type_of_argument(format!(
                "Expected a number, got the Arrow type {other}"
            )));
        }
    })
}

/// The values of a numeric array as `f64`.
pub fn floats(array: &dyn Array) -> Result<Vec<Option<f64>>, ChError> {
    match array.data_type() {
        DataType::Float32 => Ok(array
            .as_primitive::<Float32Type>()
            .iter()
            .map(|v| v.map(f64::from))
            .collect()),
        DataType::Float64 => Ok(array.as_primitive::<Float64Type>().iter().collect()),
        DataType::UInt64 => Ok(array
            .as_primitive::<UInt64Type>()
            .iter()
            .map(|v| v.map(|x| x as f64))
            .collect()),
        _ => Ok(ints(array)?
            .into_iter()
            .map(|v| v.map(|x| x as f64))
            .collect()),
    }
}

/// Whether an Arrow type is a float.
pub fn is_float(dt: &DataType) -> bool {
    matches!(dt, DataType::Float32 | DataType::Float64)
}

/// The values of a string array (`String`, or a `FixedString`'s bytes).
pub fn strings(array: &dyn Array) -> Result<Vec<Option<String>>, ChError> {
    let utf8 =
        match array.data_type() {
            DataType::Utf8 => None,
            DataType::FixedSizeBinary(_) | DataType::Binary => {
                let a = array.as_any();
                let values: Vec<Option<String>> = if let Some(b) =
                    a.downcast_ref::<datafusion::arrow::array::FixedSizeBinaryArray>()
                {
                    b.iter()
                        .map(|v| {
                            v.map(|b| {
                                String::from_utf8_lossy(b)
                                    .trim_end_matches('\0')
                                    .to_string()
                            })
                        })
                        .collect()
                } else {
                    array
                        .as_binary::<i32>()
                        .iter()
                        .map(|v| v.map(|b| String::from_utf8_lossy(b).to_string()))
                        .collect()
                };
                return Ok(values);
            }
            _ => Some(cast(array, &DataType::Utf8).map_err(|e| {
                ChError::illegal_type_of_argument(format!("Expected a string: {e}"))
            })?),
        };
    let s: &StringArray = match &utf8 {
        Some(a) => a.as_string::<i32>(),
        None => array.as_string::<i32>(),
    };
    Ok(s.iter().map(|v| v.map(str::to_string)).collect())
}

/// A result value, written as the result type asks.
#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    /// A day number, for `Date` and `Date32`.
    Day(i64),
    /// An instant (seconds, nanoseconds), for `DateTime` and `DateTime64`.
    Time(i64, u32),
    /// An integer (also `Interval*` values).
    Int(i64),
    /// An unsigned integer.
    UInt(u64),
    /// Text.
    Str(String),
    /// A decimal's unscaled value.
    Dec(i128),
}

/// The array of `values` for the result type `t` (`Nullable` stripped).
///
/// # Errors
///
/// `1` for a value that does not fit the type, a bug in the function.
pub fn write(t: &ChType, values: Vec<Option<Out>>) -> Result<ArrayRef, ChError> {
    let bad = |v: &Out| ChError::logical_error(format!("A value {v:?} written as {t}"));
    macro_rules! collect {
        ($arr:ty, $pat:pat => $e:expr) => {{
            let mut out = Vec::with_capacity(values.len());
            for v in &values {
                out.push(match v {
                    None => None,
                    Some($pat) => Some($e),
                    Some(other) => return Err(bad(other)),
                });
            }
            Arc::new(<$arr>::from(out)) as ArrayRef
        }};
    }
    let base = t.base();
    Ok(match base {
        ChType::Date | ChType::Date32 => {
            collect!(Date32Array, Out::Day(d) => i32::try_from(*d).unwrap_or(if *d < 0 { i32::MIN } else { i32::MAX }))
        }
        ChType::DateTime(tz) => {
            let a = collect!(TimestampSecondArray, Out::Time(s, _) => *s);
            retz(a, tz.as_deref())
        }
        ChType::DateTime64(p, tz) => {
            let unit = time_unit(*p);
            let per_sec = 1_000_000_000 / unit_nanos(unit);
            let keep = pow10(9 - (*p).min(9));
            let tick = |s: i64, n: u32| {
                let n = i64::from(n) - i64::from(n) % keep;
                s.wrapping_mul(per_sec)
                    .wrapping_add(n / unit_nanos(unit))
            };
            let a = match unit {
                TimeUnit::Second => collect!(TimestampSecondArray, Out::Time(s, n) => tick(*s, *n)),
                TimeUnit::Millisecond => {
                    collect!(TimestampMillisecondArray, Out::Time(s, n) => tick(*s, *n))
                }
                TimeUnit::Microsecond => {
                    collect!(TimestampMicrosecondArray, Out::Time(s, n) => tick(*s, *n))
                }
                TimeUnit::Nanosecond => {
                    collect!(TimestampNanosecondArray, Out::Time(s, n) => tick(*s, *n))
                }
            };
            retz(a, tz.as_deref())
        }
        ChType::UInt8 => collect!(UInt8Array, Out::UInt(v) => *v as u8),
        ChType::UInt16 => collect!(UInt16Array, Out::UInt(v) => *v as u16),
        ChType::UInt32 => collect!(UInt32Array, Out::UInt(v) => *v as u32),
        ChType::UInt64 => collect!(UInt64Array, Out::UInt(v) => *v),
        ChType::Int32 => collect!(Int32Array, Out::Int(v) => *v as i32),
        ChType::Int64 | ChType::Interval(_) => collect!(Int64Array, Out::Int(v) => *v),
        ChType::String => collect!(StringArray, Out::Str(v) => v.clone()),
        ChType::Bool => {
            let out: Vec<Option<bool>> = values
                .iter()
                .map(|v| match v {
                    Some(Out::UInt(x)) => Some(*x != 0),
                    _ => None,
                })
                .collect();
            Arc::new(BooleanArray::from(out))
        }
        ChType::Decimal { precision, scale } => {
            let mut out = Vec::with_capacity(values.len());
            for v in &values {
                out.push(match v {
                    None => None,
                    Some(Out::Dec(x)) => Some(*x),
                    Some(other) => return Err(bad(other)),
                });
            }
            let a = Decimal128Array::from(out)
                .with_precision_and_scale(*precision, *scale as i8)
                .map_err(|e| ChError::logical_error(e.to_string()))?;
            Arc::new(a)
        }
        ChType::Nothing => new_null_array(&DataType::Null, values.len()),
        other => {
            return Err(ChError::logical_error(format!(
                "The date and time functions do not write {other}"
            )));
        }
    })
    .and_then(|a| {
        // The Arrow type must be the layout's exactly (a zone, a unit).
        let (want, _) = data_type(base)?;
        if a.data_type() == &want {
            Ok(a)
        } else {
            cast(&a, &want).map_err(|e| ChError::logical_error(e.to_string()))
        }
    })
}

fn retz(a: ArrayRef, tz: Option<&str>) -> ArrayRef {
    let tz: Option<Arc<str>> = tz.map(Arc::from);
    let any = a.as_any();
    if let Some(x) = any.downcast_ref::<TimestampSecondArray>() {
        return Arc::new(x.clone().with_timezone_opt(tz));
    }
    if let Some(x) = any.downcast_ref::<TimestampMillisecondArray>() {
        return Arc::new(x.clone().with_timezone_opt(tz));
    }
    if let Some(x) = any.downcast_ref::<TimestampMicrosecondArray>() {
        return Arc::new(x.clone().with_timezone_opt(tz));
    }
    if let Some(x) = any.downcast_ref::<TimestampNanosecondArray>() {
        return Arc::new(x.clone().with_timezone_opt(tz));
    }
    a
}
