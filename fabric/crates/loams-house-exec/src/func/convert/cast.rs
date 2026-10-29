//! ClickHouse's cast semantics (`src/Functions/FunctionsConversion.h`,
//! Apache-2.0), value by value (plan HE1 Task 33, R33.2–R33.4).
//!
//! * Integers wrap into narrower integers (`toUInt8(300)` is `44`); floats
//!   truncate towards zero and, out of range, give what x86's `cvttsd2si`
//!   gives (`toInt32(1e20)` is `-2147483648`'s low bits, R33.3).
//! * A `String` must read whole: `CAST('abc' AS UInt8)` is
//!   `6 CANNOT_PARSE_TEXT`, a bad date `38 CANNOT_PARSE_DATE`, a bad time
//!   `41 CANNOT_PARSE_DATETIME`.
//! * An `Enum` converts by name from strings and by value from numbers
//!   (`691 UNKNOWN_ELEMENT_OF_ENUM` otherwise).
//! * `NULL` into a type that is not `Nullable` is
//!   `349 CANNOT_INSERT_NULL_IN_ORDINARY_COLUMN` for `CAST` (as
//!   `cast_keep_nullable = 0` has it).
//! * `accurateCast` refuses what does not fit exactly with
//!   `70 CANNOT_CONVERT_TYPE`.

use std::sync::Arc;

use datafusion::arrow::array::{Array, ArrayRef};
use loams_house_types::{ChError, ChType};

use super::text::{
    Atom, Style, TextOf, cannot_parse, local_days, local_seconds, parse_bool, parse_date,
    parse_datetime, parse_decimal, parse_float, parse_int, parse_ipv4, parse_ipv6, parse_quoted,
    parse_uuid, write, zone,
};
use super::value::{Value, build, default_value, read};

/// How a conversion treats a value it cannot convert.
#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    /// `CAST`, `toX`: an error.
    Throw,
    /// `toXOrNull`: `NULL`.
    OrNull,
    /// `toXOrZero`: the type's default value.
    OrZero,
    /// `accurateCast`: an error, and lossy conversions are errors too.
    Accurate,
    /// `accurateCastOrNull`: `NULL`, lossy conversions included.
    AccurateOrNull,
    /// `toXOrDefault`, `accurateCastOrDefault`: these values, one per row.
    OrDefault(Vec<Value>),
}

impl Mode {
    fn accurate(&self) -> bool {
        matches!(self, Mode::Accurate | Mode::AccurateOrNull)
    }
}

/// `349 CANNOT_INSERT_NULL_IN_ORDINARY_COLUMN`.
pub fn null_into_ordinary(to: &ChType) -> ChError {
    ChError {
        code: 349,
        name: "CANNOT_INSERT_NULL_IN_ORDINARY_COLUMN",
        message: format!("Cannot convert NULL value to non-Nullable type {to}"),
    }
}

fn cannot_convert(from: &ChType, to: &ChType, what: &str) -> ChError {
    ChError::cannot_convert_type(format!(
        "Value {what} in column {from} cannot be safely converted into type {to}"
    ))
}

fn decimal_overflow(to: &ChType) -> ChError {
    ChError {
        code: 407,
        name: "DECIMAL_OVERFLOW",
        message: format!("Convert overflow: the value does not fit {to}"),
    }
}

fn unknown_enum_element(what: &str, to: &ChType) -> ChError {
    ChError {
        code: 691,
        name: "UNKNOWN_ELEMENT_OF_ENUM",
        message: format!("Unknown element '{what}' for enum {to}"),
    }
}

fn no_conversion(from: &ChType, to: &ChType) -> ChError {
    ChError::cannot_convert_type(format!("Conversion from {from} to {to} is not supported"))
}

/// Converts a column of `from` into a column of `to` under `mode`.
pub fn cast_column(
    array: &dyn Array,
    from: &ChType,
    to: &ChType,
    mode: &Mode,
) -> Result<ArrayRef, ChError> {
    let mut out = Vec::with_capacity(array.len());
    for row in 0..array.len() {
        let v = read(array, from, row)?;
        out.push(cast_row(v, from, to, mode, row)?);
    }
    build(to, &out)
}

/// Converts a column of `from` into a column of `to` under `mode`, giving
/// the type's default value on `NULL` rows (functions under
/// `Nulls::Default`, whose result the framework masks there).
pub fn cast_column_skipping_nulls(
    array: &dyn Array,
    from: &ChType,
    to: &ChType,
    mode: &Mode,
) -> Result<ArrayRef, ChError> {
    let mut out = Vec::with_capacity(array.len());
    for row in 0..array.len() {
        let v = read(array, from, row)?;
        if v.is_null() {
            out.push(default_value(to));
        } else {
            out.push(cast_row(v, from, to, mode, row)?);
        }
    }
    build(to, &out)
}

/// One row's conversion under `mode`.
pub fn cast_row(
    v: Value,
    from: &ChType,
    to: &ChType,
    mode: &Mode,
    row: usize,
) -> Result<Value, ChError> {
    let target_nullable = to.is_nullable();
    if v.is_null() {
        return match mode {
            Mode::OrDefault(defaults) if !target_nullable => Ok(defaults
                .get(row)
                .cloned()
                .unwrap_or_else(|| default_value(to))),
            Mode::OrNull | Mode::AccurateOrNull => Ok(Value::Null),
            _ if target_nullable => Ok(Value::Null),
            Mode::OrZero => Ok(default_value(to)),
            _ => Err(null_into_ordinary(to)),
        };
    }
    match convert(v, from.base(), to.base(), mode.accurate()) {
        Ok(v) => Ok(v),
        Err(e) => match mode {
            Mode::Throw | Mode::Accurate => Err(e),
            Mode::OrNull | Mode::AccurateOrNull => Ok(Value::Null),
            Mode::OrZero => Ok(default_value(to.base())),
            Mode::OrDefault(defaults) => Ok(defaults
                .get(row)
                .cloned()
                .unwrap_or_else(|| default_value(to.base()))),
        },
    }
}

/// The range of an integer type.
pub fn int_range(t: &ChType) -> Option<(i128, i128)> {
    Some(match t {
        ChType::UInt8 | ChType::Bool => (0, i128::from(u8::MAX)),
        ChType::UInt16 => (0, i128::from(u16::MAX)),
        ChType::UInt32 | ChType::IPv4 => (0, i128::from(u32::MAX)),
        ChType::UInt64 => (0, i128::from(u64::MAX)),
        ChType::Int8 => (i128::from(i8::MIN), i128::from(i8::MAX)),
        ChType::Int16 => (i128::from(i16::MIN), i128::from(i16::MAX)),
        ChType::Int32 => (i128::from(i32::MIN), i128::from(i32::MAX)),
        ChType::Int64 | ChType::Interval(_) => (i128::from(i64::MIN), i128::from(i64::MAX)),
        _ => return None,
    })
}

/// `v` wrapped into integer type `t` (two's complement, as C++ casts do).
pub fn wrap_int(v: i128, t: &ChType) -> i128 {
    match t {
        ChType::UInt8 => i128::from(v as u8),
        ChType::UInt16 => i128::from(v as u16),
        ChType::UInt32 | ChType::IPv4 => i128::from(v as u32),
        ChType::UInt64 => i128::from(v as u64),
        ChType::Int8 => i128::from(v as i8),
        ChType::Int16 => i128::from(v as i16),
        ChType::Int32 => i128::from(v as i32),
        ChType::Int64 | ChType::Interval(_) => i128::from(v as i64),
        _ => v,
    }
}

const TWO_POW_63: f64 = 9_223_372_036_854_775_808.0;

/// What x86's truncating conversion gives for `f` as an `Int64`: the
/// "integer indefinite" `i64::MIN` for NaN and out-of-range values.
fn float_to_i64_x86(f: f64) -> i64 {
    if !(-TWO_POW_63..TWO_POW_63).contains(&f) {
        i64::MIN
    } else {
        f as i64
    }
}

/// `f` as integer type `t`, as ClickHouse's `static_cast` gives it on x86
/// (R33.3).
pub fn float_to_int(f: f64, t: &ChType) -> i128 {
    match t {
        ChType::UInt64 => {
            let u = if f >= TWO_POW_63 {
                (float_to_i64_x86(f - TWO_POW_63) as u64) ^ (1u64 << 63)
            } else {
                float_to_i64_x86(f) as u64
            };
            i128::from(u)
        }
        ChType::UInt32 | ChType::IPv4 => wrap_int(i128::from(float_to_i64_x86(f)), t),
        // Narrower types go through `Int32` (`cvttsd2si` with a 32-bit
        // destination), then their low bits.
        ChType::UInt8 | ChType::UInt16 | ChType::Int8 | ChType::Int16 | ChType::Int32 => {
            let i = if !(-2_147_483_648.0..2_147_483_648.0).contains(&f) {
                i128::from(i32::MIN)
            } else {
                i128::from(f as i32)
            };
            wrap_int(i, t)
        }
        _ => i128::from(float_to_i64_x86(f)),
    }
}

/// `10^n` as an `i128`, if it fits.
pub fn pow10(n: u32) -> Option<i128> {
    10i128.checked_pow(n)
}

/// `u` (scale `from`) at scale `to`, truncating.
fn rescale(u: i128, from: u8, to: u8) -> Option<i128> {
    if to >= from {
        u.checked_mul(pow10(u32::from(to - from))?)
    } else {
        Some(u / pow10(u32::from(from - to))?)
    }
}

fn check_precision(u: i128, precision: u8, to: &ChType) -> Result<i128, ChError> {
    match pow10(u32::from(precision)) {
        Some(limit) if u.abs() >= limit => Err(decimal_overflow(to)),
        _ => Ok(u),
    }
}

fn text_of(from: &ChType) -> TextOf<'_> {
    TextOf {
        float32: matches!(from, ChType::Float32),
        tz: match from {
            ChType::DateTime(tz) | ChType::DateTime64(_, tz) => tz.as_deref(),
            _ => None,
        },
    }
}

/// A `FixedString`'s bytes without its trailing zeros, for parsing.
fn trimmed<'a>(from: &ChType, b: &'a [u8]) -> &'a [u8] {
    match from {
        ChType::FixedString(_) => {
            let end = b.iter().rposition(|c| *c != 0).map_or(0, |i| i + 1);
            &b[..end]
        }
        _ => b,
    }
}

/// Converts one non-`NULL` value of `from` to `to` (both without
/// `Nullable`/`LowCardinality` at the top).
pub fn convert(v: Value, from: &ChType, to: &ChType, accurate: bool) -> Result<Value, ChError> {
    if from == to {
        return Ok(v);
    }
    match to {
        ChType::Nullable(inner) | ChType::LowCardinality(inner) => {
            convert(v, from, inner, accurate)
        }
        ChType::UInt8
        | ChType::UInt16
        | ChType::UInt32
        | ChType::UInt64
        | ChType::Int8
        | ChType::Int16
        | ChType::Int32
        | ChType::Int64
        | ChType::Interval(_) => to_int(v, from, to, accurate).map(Value::Int),
        ChType::Float32 | ChType::Float64 => to_float(v, from, to).map(Value::Float),
        ChType::Bool => to_bool(v, from, to).map(Value::Bool),
        ChType::Decimal { precision, scale } => {
            to_decimal(v, from, to, *precision, *scale).map(|u| Value::Dec(u, *scale))
        }
        ChType::String | ChType::Json(_) => {
            Ok(Value::Str(write(&v, &text_of(from), Style::Plain)?))
        }
        ChType::FixedString(n) => {
            let bytes = write(&v, &text_of(from), Style::Plain)?;
            if bytes.len() > *n as usize {
                return Err(ChError {
                    code: 131,
                    name: "TOO_LARGE_STRING_SIZE",
                    message: format!("String too long for type {to}"),
                });
            }
            Ok(Value::Str(bytes))
        }
        ChType::Date | ChType::Date32 => to_date(v, from, to).map(Value::Date),
        ChType::DateTime(tz) => to_time(v, from, to, 0, tz.as_deref()).map(|t| Value::Time(t, 0)),
        ChType::DateTime64(p, tz) => {
            to_time(v, from, to, *p, tz.as_deref()).map(|t| Value::Time(t, *p))
        }
        ChType::Uuid => match v {
            Value::Uuid(b) => Ok(Value::Uuid(b)),
            Value::Str(b) => parse_uuid(trimmed(from, &b))
                .map(Value::Uuid)
                .ok_or_else(|| ChError {
                    code: 376,
                    name: "CANNOT_PARSE_UUID",
                    message: format!("Cannot parse uuid {}", String::from_utf8_lossy(&b)),
                }),
            _ => Err(no_conversion(from, to)),
        },
        ChType::IPv4 => match v {
            Value::Ip4(a) => Ok(Value::Ip4(a)),
            Value::Int(i) => Ok(Value::Ip4(wrap_int(i, to) as u32)),
            Value::Str(b) => parse_ipv4(trimmed(from, &b))
                .map(Value::Ip4)
                .ok_or_else(|| ChError {
                    code: 441,
                    name: "CANNOT_PARSE_IPV4",
                    message: format!("Invalid IPv4 value: {}", String::from_utf8_lossy(&b)),
                }),
            _ => Err(no_conversion(from, to)),
        },
        ChType::IPv6 => match v {
            Value::Ip6(a) => Ok(Value::Ip6(a)),
            Value::Ip4(a) => Ok(Value::Ip6(
                std::net::Ipv4Addr::from(a).to_ipv6_mapped().octets(),
            )),
            Value::Str(b) => parse_ipv6(trimmed(from, &b))
                .map(Value::Ip6)
                .ok_or_else(|| ChError {
                    code: 442,
                    name: "CANNOT_PARSE_IPV6",
                    message: format!("Invalid IPv6 value: {}", String::from_utf8_lossy(&b)),
                }),
            _ => Err(no_conversion(from, to)),
        },
        ChType::Enum8(members) => {
            let members: Vec<(String, i64)> = members
                .iter()
                .map(|(n, v)| (n.clone(), i64::from(*v)))
                .collect();
            to_enum(v, from, to, &members)
        }
        ChType::Enum16(members) => {
            let members: Vec<(String, i64)> = members
                .iter()
                .map(|(n, v)| (n.clone(), i64::from(*v)))
                .collect();
            to_enum(v, from, to, &members)
        }
        ChType::Array(inner_to) => {
            let (items, inner_from) = match (v, from) {
                (Value::Array(items), ChType::Array(inner_from)) => {
                    (items, inner_from.as_ref().clone())
                }
                (Value::Str(b), _) => match parse_quoted(trimmed(from, &b)) {
                    Some(Atom::List(atoms)) => {
                        (atoms.into_iter().map(atom_value).collect(), ChType::String)
                    }
                    _ => return Err(cannot_parse(&b, &to.to_string())),
                },
                _ => return Err(no_conversion(from, to)),
            };
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(convert_nested(item, &inner_from, inner_to, accurate)?);
            }
            Ok(Value::Array(out))
        }
        ChType::Tuple(elements) => {
            let (items, froms): (Vec<Value>, Vec<ChType>) = match (v, from) {
                (Value::Tuple(items), ChType::Tuple(fs)) if fs.len() == elements.len() => {
                    (items, fs.iter().map(|(_, t)| t.clone()).collect())
                }
                (Value::Str(b), _) => match parse_quoted(trimmed(from, &b)) {
                    Some(Atom::List(atoms)) if atoms.len() == elements.len() => (
                        atoms.into_iter().map(atom_value).collect(),
                        vec![ChType::String; elements.len()],
                    ),
                    _ => return Err(cannot_parse(&b, &to.to_string())),
                },
                _ => return Err(no_conversion(from, to)),
            };
            let mut out = Vec::with_capacity(items.len());
            for ((item, f), (_, t)) in items.into_iter().zip(&froms).zip(elements) {
                out.push(convert_nested(item, f, t, accurate)?);
            }
            Ok(Value::Tuple(out))
        }
        ChType::Nothing => Err(no_conversion(from, to)),
        other => Err(ChError::unsupported(format!("CAST to {other}"))),
    }
}

/// An element's conversion: `NULL` only into a `Nullable` element.
fn convert_nested(v: Value, from: &ChType, to: &ChType, accurate: bool) -> Result<Value, ChError> {
    if v.is_null() {
        return if to.is_nullable() {
            Ok(Value::Null)
        } else {
            Err(null_into_ordinary(to))
        };
    }
    convert(v, from.base(), to.base(), accurate)
}

fn atom_value(a: Atom) -> Value {
    match a {
        Atom::Null => Value::Null,
        Atom::Text(t) => Value::Str(t),
        Atom::List(items) => Value::Array(items.into_iter().map(atom_value).collect()),
    }
}

fn to_int(v: Value, from: &ChType, to: &ChType, accurate: bool) -> Result<i128, ChError> {
    let (min, max) = int_range(to).ok_or_else(|| no_conversion(from, to))?;
    let fit = |i: i128, what: &dyn Fn() -> String| -> Result<i128, ChError> {
        if accurate && !(min..=max).contains(&i) {
            Err(cannot_convert(from, to, &what()))
        } else {
            Ok(wrap_int(i, to))
        }
    };
    match v {
        Value::Int(i) => fit(i, &|| i.to_string()),
        Value::Bool(b) => Ok(i128::from(b)),
        Value::Float(f) => {
            if accurate {
                let t = f.trunc();
                if !f.is_finite() || t != f || !(min as f64..=max as f64).contains(&t) {
                    return Err(cannot_convert(from, to, &f.to_string()));
                }
                Ok(t as i128)
            } else {
                Ok(float_to_int(f, to))
            }
        }
        Value::Dec(u, s) => {
            let i = u / pow10(u32::from(s)).ok_or_else(|| decimal_overflow(to))?;
            if !(min..=max).contains(&i) {
                return Err(decimal_overflow(to));
            }
            Ok(i)
        }
        Value::Str(b) => {
            let text = trimmed(from, &b);
            let i = parse_int(text).ok_or_else(|| cannot_parse(text, &to.to_string()))?;
            fit(i, &|| String::from_utf8_lossy(text).into_owned())
        }
        Value::Date(d) => fit(i128::from(d), &|| d.to_string()),
        Value::Time(t, p) => {
            let secs = t.div_euclid(10i64.pow(u32::from(p)));
            fit(i128::from(secs), &|| secs.to_string())
        }
        Value::Enum(i, _) => fit(i128::from(i), &|| i.to_string()),
        Value::Ip4(a) => fit(i128::from(a), &|| a.to_string()),
        _ => Err(no_conversion(from, to)),
    }
}

fn to_float(v: Value, from: &ChType, to: &ChType) -> Result<f64, ChError> {
    let f = match v {
        Value::Int(i) => i as f64,
        Value::Float(f) => f,
        Value::Bool(b) => f64::from(u8::from(b)),
        Value::Dec(u, s) => u as f64 / 10f64.powi(i32::from(s)),
        Value::Str(b) => {
            let text = trimmed(from, &b);
            parse_float(text).ok_or_else(|| cannot_parse(text, &to.to_string()))?
        }
        Value::Date(d) => f64::from(d),
        Value::Time(t, p) => t as f64 / 10f64.powi(i32::from(p)),
        Value::Enum(i, _) => i as f64,
        Value::Ip4(a) => f64::from(a),
        _ => return Err(no_conversion(from, to)),
    };
    Ok(if matches!(to, ChType::Float32) {
        f64::from(f as f32)
    } else {
        f
    })
}

fn to_bool(v: Value, from: &ChType, to: &ChType) -> Result<bool, ChError> {
    Ok(match v {
        Value::Bool(b) => b,
        Value::Int(i) => i != 0,
        Value::Float(f) => f != 0.0,
        Value::Dec(u, _) => u != 0,
        Value::Str(b) => {
            let text = trimmed(from, &b);
            parse_bool(text).ok_or_else(|| ChError {
                code: 467,
                name: "CANNOT_PARSE_BOOL",
                message: format!(
                    "Cannot parse boolean value here: '{}'",
                    String::from_utf8_lossy(text)
                ),
            })?
        }
        _ => return Err(no_conversion(from, to)),
    })
}

fn to_decimal(
    v: Value,
    from: &ChType,
    to: &ChType,
    precision: u8,
    scale: u8,
) -> Result<i128, ChError> {
    if precision > 38 {
        return Err(ChError::unsupported(format!("CAST to {to}")));
    }
    let multiplier = pow10(u32::from(scale)).ok_or_else(|| decimal_overflow(to))?;
    let u = match v {
        Value::Int(i) => i
            .checked_mul(multiplier)
            .ok_or_else(|| decimal_overflow(to))?,
        Value::Bool(b) => i128::from(b) * multiplier,
        Value::Float(f) => {
            if !f.is_finite() {
                return Err(ChError {
                    code: 407,
                    name: "DECIMAL_OVERFLOW",
                    message: "Decimal convert overflow. Cannot convert infinity or NaN to decimal"
                        .into(),
                });
            }
            let scaled = (f * multiplier as f64).trunc();
            if scaled.abs() >= 1e38 {
                return Err(decimal_overflow(to));
            }
            scaled as i128
        }
        Value::Dec(u, s) => rescale(u, s, scale).ok_or_else(|| decimal_overflow(to))?,
        Value::Str(b) => {
            let text = trimmed(from, &b);
            parse_decimal(text, scale).ok_or_else(|| cannot_parse(text, &to.to_string()))??
        }
        Value::Date(d) => i128::from(d)
            .checked_mul(multiplier)
            .ok_or_else(|| decimal_overflow(to))?,
        Value::Time(t, p) => {
            rescale(i128::from(t), p, scale).ok_or_else(|| decimal_overflow(to))?
        }
        Value::Enum(i, _) => i128::from(i) * multiplier,
        _ => return Err(no_conversion(from, to)),
    };
    check_precision(u, precision, to)
}

/// The days of `Date` (0..=65535, 1970-01-01 to 2149-06-06) and `Date32`
/// (1900-01-01 to 2299-12-31).
fn date_range(t: &ChType) -> (i64, i64) {
    match t {
        ChType::Date32 => (-25_567, 120_529),
        _ => (0, 65_535),
    }
}

fn to_date(v: Value, from: &ChType, to: &ChType) -> Result<i32, ChError> {
    let (min, max) = date_range(to);
    let clamp = |d: i64| d.clamp(min, max) as i32;
    // A number up to the last day is a day count, above it a time
    // (`toDate(1700000000)`), in the session's zone.
    let from_number = |n: i64| -> Result<i32, ChError> {
        if n <= max {
            Ok(clamp(n))
        } else {
            Ok(clamp(i64::from(local_days(n, zone(None)?))))
        }
    };
    match v {
        Value::Int(i) => from_number(i.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64),
        Value::Float(f) => from_number(if f.is_finite() { f.trunc() as i64 } else { 0 }),
        Value::Bool(b) => Ok(i32::from(b)),
        Value::Date(d) => Ok(clamp(i64::from(d))),
        Value::Time(t, p) => {
            let secs = t.div_euclid(10i64.pow(u32::from(p)));
            let tz = match from {
                ChType::DateTime(tz) | ChType::DateTime64(_, tz) => tz.as_deref(),
                _ => None,
            };
            Ok(clamp(i64::from(local_days(secs, zone(tz)?))))
        }
        Value::Str(b) => {
            let text = trimmed(from, &b);
            let d = parse_date(text).ok_or_else(|| ChError {
                code: 38,
                name: "CANNOT_PARSE_DATE",
                message: format!(
                    "Cannot parse date: value is not a date '{}'",
                    String::from_utf8_lossy(text)
                ),
            })?;
            Ok(clamp(i64::from(d)))
        }
        _ => Err(no_conversion(from, to)),
    }
}

fn to_time(
    v: Value,
    from: &ChType,
    to: &ChType,
    precision: u8,
    tz: Option<&str>,
) -> Result<i64, ChError> {
    let scale = 10i64.pow(u32::from(precision));
    let zone_to = zone(tz)?;
    let ticks = match v {
        Value::Int(i) => {
            (i.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64).saturating_mul(scale)
        }
        Value::Float(f) => {
            if !f.is_finite() {
                0
            } else {
                (f * scale as f64).round() as i64
            }
        }
        Value::Bool(b) => i64::from(b) * scale,
        Value::Dec(u, s) => {
            let t = rescale(u, s, precision).ok_or_else(|| decimal_overflow(to))?;
            t.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
        }
        Value::Date(d) => {
            let date = chrono::NaiveDate::from_ymd_opt(1970, 1, 1)
                .and_then(|e| e.checked_add_signed(chrono::TimeDelta::days(i64::from(d))))
                .ok_or_else(|| no_conversion(from, to))?;
            local_seconds(date.and_time(chrono::NaiveTime::MIN), zone_to).saturating_mul(scale)
        }
        Value::Time(t, p) => super::value::rescale_ticks(t, p, precision),
        Value::Str(b) => {
            let text = trimmed(from, &b);
            parse_datetime(text, precision, zone_to).ok_or_else(|| ChError {
                code: 41,
                name: "CANNOT_PARSE_DATETIME",
                message: format!(
                    "Cannot parse datetime: value is not a date and time '{}'",
                    String::from_utf8_lossy(text)
                ),
            })?
        }
        _ => return Err(no_conversion(from, to)),
    };
    // `DateTime` is a `UInt32` of seconds: earlier is the epoch, later the
    // last second it holds.
    Ok(if precision == 0 && matches!(to, ChType::DateTime(_)) {
        ticks.clamp(0, i64::from(u32::MAX))
    } else {
        ticks
    })
}

fn to_enum(
    v: Value,
    from: &ChType,
    to: &ChType,
    members: &[(String, i64)],
) -> Result<Value, ChError> {
    let by_name = |name: &str| {
        members
            .iter()
            .find(|(n, _)| n == name)
            .map(|(n, v)| Value::Enum(*v, n.clone()))
            .ok_or_else(|| unknown_enum_element(name, to))
    };
    let by_value = |i: i128| {
        members
            .iter()
            .find(|(_, v)| i128::from(*v) == i)
            .map(|(n, v)| Value::Enum(*v, n.clone()))
            .ok_or_else(|| ChError {
                code: 691,
                name: "UNKNOWN_ELEMENT_OF_ENUM",
                message: format!("Unexpected value {i} in enum {to}"),
            })
    };
    match v {
        Value::Str(b) => by_name(&String::from_utf8_lossy(trimmed(from, &b))),
        Value::Enum(_, name) => by_name(&name),
        Value::Int(i) => by_value(i),
        Value::Bool(b) => by_value(i128::from(b)),
        Value::Float(f) if f.fract() == 0.0 && f.is_finite() => by_value(f as i128),
        _ => Err(no_conversion(from, to)),
    }
}

/// `array` (a column of `from`) as a column of `to`, by [`Mode::Throw`];
/// `array` itself when the types are equal.
pub fn cast_to(array: &ArrayRef, from: &ChType, to: &ChType) -> Result<ArrayRef, ChError> {
    if from == to {
        return Ok(Arc::clone(array));
    }
    cast_column(array.as_ref(), from, to, &Mode::Throw)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use loams_house_types::parse;

    fn s(text: &str) -> Value {
        Value::Str(text.as_bytes().to_vec())
    }

    fn cv(v: Value, from: &str, to: &str) -> Result<Value, ChError> {
        convert(v, &parse(from).unwrap(), &parse(to).unwrap(), false)
    }

    #[test]
    fn integers_wrap_and_strings_must_parse() {
        assert_eq!(
            cv(Value::Int(300), "Int64", "UInt8").unwrap(),
            Value::Int(44)
        );
        assert_eq!(
            cv(Value::Int(-1), "Int64", "UInt8").unwrap(),
            Value::Int(255)
        );
        assert_eq!(cv(s("42"), "String", "UInt8").unwrap(), Value::Int(42));
        assert_eq!(cv(s("abc"), "String", "UInt8").unwrap_err().code, 6);
        assert_eq!(cv(s("1.5"), "String", "Int32").unwrap_err().code, 6);
        assert_eq!(cv(s(""), "String", "Int32").unwrap_err().code, 6);
    }

    #[test]
    fn floats_truncate_towards_zero() {
        assert_eq!(
            cv(Value::Float(1.9), "Float64", "Int32").unwrap(),
            Value::Int(1)
        );
        assert_eq!(
            cv(Value::Float(-1.9), "Float64", "Int32").unwrap(),
            Value::Int(-1)
        );
        assert_eq!(
            cv(Value::Float(f64::NAN), "Float64", "Int64").unwrap(),
            Value::Int(i128::from(i64::MIN))
        );
        assert_eq!(
            cv(Value::Float(300.5), "Float64", "UInt8").unwrap(),
            Value::Int(44)
        );
        assert_eq!(
            cv(Value::Float(1e19), "Float64", "UInt64").unwrap(),
            Value::Int(10_000_000_000_000_000_000)
        );
    }

    #[test]
    fn accurate_casts_refuse_what_does_not_fit() {
        let acc =
            |v, from: &str, to: &str| convert(v, &parse(from).unwrap(), &parse(to).unwrap(), true);
        assert_eq!(acc(Value::Int(-1), "Int64", "UInt8").unwrap_err().code, 70);
        assert_eq!(
            acc(Value::Float(1.5), "Float64", "UInt8").unwrap_err().code,
            70
        );
        assert_eq!(
            acc(Value::Int(255), "Int64", "UInt8").unwrap(),
            Value::Int(255)
        );
    }

    #[test]
    fn decimals_scale_and_overflow() {
        assert_eq!(
            cv(Value::Int(5), "Int64", "Decimal(9, 2)").unwrap(),
            Value::Dec(500, 2)
        );
        assert_eq!(
            cv(Value::Float(1.239), "Float64", "Decimal(9, 2)").unwrap(),
            Value::Dec(123, 2)
        );
        assert_eq!(
            cv(Value::Int(10_000_000_000), "Int64", "Decimal(9, 2)")
                .unwrap_err()
                .code,
            407
        );
        assert_eq!(
            cv(Value::Dec(12345, 2), "Decimal(9, 2)", "Int32").unwrap(),
            Value::Int(123)
        );
        assert_eq!(
            cv(Value::Dec(12345, 2), "Decimal(9, 2)", "String").unwrap(),
            s("123.45")
        );
    }

    #[test]
    fn dates_and_times_convert() {
        assert_eq!(
            cv(s("2026-01-02"), "String", "Date").unwrap(),
            Value::Date(20_455)
        );
        assert_eq!(cv(s("x"), "String", "Date").unwrap_err().code, 38);
        assert_eq!(cv(s("x"), "String", "DateTime").unwrap_err().code, 41);
        assert_eq!(
            cv(s("1970-01-01 00:00:01.5"), "String", "DateTime64(3)").unwrap(),
            Value::Time(1_500, 3)
        );
        assert_eq!(cv(Value::Int(1), "Int64", "Date").unwrap(), Value::Date(1));
        assert_eq!(
            cv(Value::Int(86_400 * 70_000), "Int64", "Date").unwrap(),
            Value::Date(65_535)
        );
        assert_eq!(
            cv(Value::Time(86_399, 0), "DateTime('UTC')", "Date").unwrap(),
            Value::Date(0)
        );
        assert_eq!(
            cv(Value::Date(1), "Date", "DateTime('UTC')").unwrap(),
            Value::Time(86_400, 0)
        );
        assert_eq!(
            cv(Value::Time(1, 0), "DateTime('UTC')", "String").unwrap(),
            s("1970-01-01 00:00:01")
        );
        assert_eq!(
            cv(Value::Int(-5), "Int64", "DateTime").unwrap(),
            Value::Time(0, 0)
        );
    }

    #[test]
    fn enums_convert_by_name_and_value() {
        let e = "Enum8('a' = 1, 'b' = 2)";
        assert_eq!(cv(s("b"), "String", e).unwrap(), Value::Enum(2, "b".into()));
        assert_eq!(cv(s("z"), "String", e).unwrap_err().code, 691);
        assert_eq!(
            cv(Value::Int(1), "Int64", e).unwrap(),
            Value::Enum(1, "a".into())
        );
        assert_eq!(cv(Value::Int(3), "Int64", e).unwrap_err().code, 691);
        assert_eq!(cv(Value::Enum(2, "b".into()), e, "String").unwrap(), s("b"));
    }

    #[test]
    fn composites_convert() {
        assert_eq!(
            cv(s("[1,2]"), "String", "Array(UInt8)").unwrap(),
            Value::Array(vec![Value::Int(1), Value::Int(2)])
        );
        assert_eq!(
            cv(
                Value::Array(vec![Value::Int(1)]),
                "Array(Int64)",
                "Array(String)"
            )
            .unwrap(),
            Value::Array(vec![s("1")])
        );
        assert_eq!(
            cv(Value::Array(vec![Value::Int(1)]), "Array(Int64)", "String").unwrap(),
            s("[1]")
        );
        assert_eq!(
            cv(s("[NULL]"), "String", "Array(UInt8)").unwrap_err().code,
            349
        );
    }

    #[test]
    fn modes_choose_what_a_failure_gives() {
        let from = ChType::String;
        let to = ChType::UInt8;
        assert_eq!(
            cast_row(s("x"), &from, &to, &Mode::OrNull, 0).unwrap(),
            Value::Null
        );
        assert_eq!(
            cast_row(s("x"), &from, &to, &Mode::OrZero, 0).unwrap(),
            Value::Int(0)
        );
        assert_eq!(
            cast_row(s("x"), &from, &to, &Mode::OrDefault(vec![Value::Int(9)]), 0).unwrap(),
            Value::Int(9)
        );
        assert_eq!(
            cast_row(Value::Null, &from, &to, &Mode::Throw, 0)
                .unwrap_err()
                .code,
            349
        );
        assert_eq!(
            cast_row(
                Value::Null,
                &from,
                &ChType::Nullable(Box::new(ChType::UInt8)),
                &Mode::Throw,
                0
            )
            .unwrap(),
            Value::Null
        );
        assert_eq!(
            cv(s("toolong"), "String", "FixedString(2)")
                .unwrap_err()
                .code,
            131
        );
        assert_eq!(cv(s("yes"), "String", "Bool").unwrap(), Value::Bool(true));
    }
}
