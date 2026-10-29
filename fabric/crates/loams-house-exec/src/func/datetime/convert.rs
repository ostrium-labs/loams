//! Conversions: `toDate`, `toDate32`, `toDateTime`, `toDateTime64`,
//! `toTimeZone`, `toUnixTimestamp`, `toUnixTimestamp64*`,
//! `fromUnixTimestamp64*`, `now`, `now64` and `parseDateTimeBestEffort*`.
//!
//! Ranges as measured on the oracle (R30.5): a `Date` from a time, a string
//! or a number saturates to 1970-01-01..2149-06-06, a `DateTime` from a
//! string or a number to 1970-01-01 00:00:00..2106-02-07 06:28:15, and a
//! `DateTime` from a `Date32` or `DateTime64` wraps as a `UInt32`.

use chrono::Datelike;
use chrono_tz::Tz;
use datafusion::arrow::array::ArrayRef;
use datafusion::common::ScalarValue;
use datafusion::logical_expr::Expr;
use datafusion::logical_expr::simplify::SimplifyContext;
use loams_house_types::{ChError, ChType};

use super::parse::{Civil, basic, best_effort};
use super::udf::{Call, ChFunction, Sig, illegal, own_zone};
use super::value::{
    Kind, Out, Point, floats, ints, is_float, points, pow10, strings, temporal, write,
};
use super::zone::{DAY, day_start, local};

/// The largest `DateTime`.
pub const DATETIME_MAX: i64 = 4_294_967_295;
/// The largest `Date` (2149-06-06).
pub const DATE_MAX: i64 = 65_535;
/// The smallest and largest `Date32` (1900-01-01, 2299-12-31).
pub const DATE32_RANGE: (i64, i64) = (-25_567, 120_529);
/// The smallest and largest `DateTime64` second (1900-01-01 00:00:00,
/// 9999-12-31 23:59:59).
pub const DATETIME64_RANGE: (i64, i64) = (-2_208_988_800, 253_402_300_799);

pub(crate) fn cannot_parse_datetime(text: &str) -> ChError {
    ChError {
        code: 41,
        name: "CANNOT_PARSE_DATETIME",
        message: format!("Cannot parse DateTime from String {text:?}"),
    }
}

fn cannot_parse_date(text: &str) -> ChError {
    ChError {
        code: 38,
        name: "CANNOT_PARSE_DATE",
        message: format!(
            "Cannot parse date: expected a date in the YYYY-MM-DD or YYYYMMDD format: {text:?}"
        ),
    }
}

/// Whether `t` is text.
pub fn is_text(t: &ChType) -> bool {
    matches!(t, ChType::String | ChType::FixedString(_))
}

/// Whether `t` is a number (or `Bool`).
pub fn is_number(t: &ChType) -> bool {
    matches!(
        t,
        ChType::UInt8
            | ChType::UInt16
            | ChType::UInt32
            | ChType::UInt64
            | ChType::Int8
            | ChType::Int16
            | ChType::Int32
            | ChType::Int64
            | ChType::Float32
            | ChType::Float64
            | ChType::Bool
            | ChType::Decimal { .. }
    )
}

/// Argument `i` as instants (seconds, nanoseconds): a time as it is, a date
/// at its midnight in `tz`, text read in `tz` (`41` when it is not a time).
/// Numbers are not instants here; each conversion reads them its own way.
pub fn instants(call: &Call, i: usize, tz: Tz) -> Result<Vec<Option<(i64, u32)>>, ChError> {
    let t = &call.types[i];
    if temporal(t).is_some() {
        return Ok(points(call.arrays[i].as_ref())?
            .into_iter()
            .map(|p| {
                p.map(|p| match p {
                    Point::Day(d) => (day_start(tz, d), 0),
                    Point::Time(s, n) => (s, n),
                })
            })
            .collect());
    }
    if is_text(t) {
        let texts = strings(call.arrays[i].as_ref())?;
        return call.map(|row| {
            let s = texts[row].as_deref().unwrap_or("");
            let c = basic(s).ok_or_else(|| cannot_parse_datetime(s))?;
            Ok(Some((c.instant(tz), c.nanos)))
        });
    }
    Err(illegal(call.name, t, i))
}

fn check_source(sig: &Sig, numbers: bool) -> Result<(), ChError> {
    let t = &sig.types[0];
    if temporal(t).is_some() || is_text(t) || (numbers && is_number(t)) {
        Ok(())
    } else {
        Err(illegal(sig.name, t, 0))
    }
}

fn date_type(sig: &Sig) -> Result<ChType, ChError> {
    sig.arity(1, 2)?;
    check_source(sig, true)?;
    sig.zone_arg(1)?;
    Ok(if sig.name == "toDate32" {
        ChType::Date32
    } else {
        ChType::Date
    })
}

fn date_eval(call: &Call) -> Result<ArrayRef, ChError> {
    let wide = call.name == "toDate32";
    let (lo, hi) = if wide { DATE32_RANGE } else { (0, DATE_MAX) };
    let t = &call.types[0];
    let tz = call.zone(1, temporal(t).and_then(|(_, z)| z))?;
    let a = call.arrays[0].as_ref();
    // A `Date` from a `Date32` or a `DateTime64` wraps as a `UInt16`, as
    // measured; from a `DateTime`, text or a number it saturates.
    let wraps = !wide && matches!(temporal(t), Some((Kind::Date32 | Kind::DateTime64(_), _)));
    let days: Vec<Option<i64>> = if temporal(t).is_some() {
        points(a)?
            .into_iter()
            .map(|p| {
                p.map(|p| {
                    let d = match p {
                        Point::Day(d) => d,
                        Point::Time(s, _) => local(tz, s).0,
                    };
                    if wraps { d.rem_euclid(DATE_MAX + 1) } else { d }
                })
            })
            .collect()
    } else if is_text(t) {
        let texts = strings(a)?;
        call.map(|row| {
            let s = texts[row].as_deref().unwrap_or("");
            // A date reads its date and an optional ` hh:mm:ss`, never `T…`.
            if s.contains('T') {
                return Err(ChError::cannot_parse_text(format!(
                    "Cannot parse string {s:?} as Date"
                )));
            }
            let c = basic(s).ok_or_else(|| cannot_parse_date(s))?;
            Ok(Some(if c.unix || c.offset.is_some() {
                local(tz, c.instant(tz)).0
            } else {
                c.day
            }))
        })?
    } else {
        // A number below 65536 is a day number, a larger one a Unix
        // timestamp (`toDate32`: always a day number).
        ints(a)?
            .into_iter()
            .map(|v| {
                v.map(|n| {
                    if wide || n < DATE_MAX + 1 {
                        n
                    } else {
                        local(tz, n).0
                    }
                })
            })
            .collect()
    };
    // `toDate32` of text keeps the day it reads (26.9 measured:
    // `toDate32('1899-01-01')` is 1899-01-01); everything else saturates.
    let saturate = !(wide && is_text(t));
    write(
        &call.ret,
        days.into_iter()
            .map(|d| d.map(|d| Out::Day(if saturate { d.clamp(lo, hi) } else { d })))
            .collect(),
    )
}

fn datetime_type(sig: &Sig) -> Result<ChType, ChError> {
    sig.arity(1, 2)?;
    check_source(sig, true)?;
    let zone = sig.zone_arg(1)?.or_else(|| own_zone(&sig.types[0]));
    Ok(ChType::DateTime(zone))
}

fn datetime_eval(call: &Call) -> Result<ArrayRef, ChError> {
    let t = &call.types[0];
    let tz = call.zone(1, temporal(t).and_then(|(_, z)| z))?;
    let a = call.arrays[0].as_ref();
    let secs: Vec<Option<i64>> = match temporal(t) {
        Some((Kind::DateTime, _)) => points(a)?
            .into_iter()
            .map(|p| {
                p.map(|p| match p {
                    Point::Time(s, _) => s,
                    Point::Day(d) => d * DAY,
                })
            })
            .collect(),
        Some((Kind::Date, _)) => instants(call, 0, tz)?
            .into_iter()
            .map(|v| v.map(|(s, _)| s.clamp(0, DATETIME_MAX)))
            .collect(),
        // Wraps as a `UInt32`, as measured.
        Some(_) => instants(call, 0, tz)?
            .into_iter()
            .map(|v| v.map(|(s, _)| i64::from(s as u32)))
            .collect(),
        None if is_text(t) => instants(call, 0, tz)?
            .into_iter()
            .map(|v| v.map(|(s, _)| s.clamp(0, DATETIME_MAX)))
            .collect(),
        None => ints(a)?
            .into_iter()
            .map(|v| v.map(|s| s.clamp(0, DATETIME_MAX)))
            .collect(),
    };
    write(
        &call.ret,
        secs.into_iter()
            .map(|s| s.map(|s| Out::Time(s, 0)))
            .collect(),
    )
}

/// A `DateTime64` precision argument: a constant 0 to 9.
fn precision(sig_name: &str, p: Option<i64>, default: u8) -> Result<u8, ChError> {
    match p {
        None => Ok(default),
        Some(p) if (0..=9).contains(&p) => Ok(p as u8),
        Some(p) => Err(ChError::argument_out_of_bound(format!(
            "Scale {p} of function {sig_name} is out of bounds (the maximum is 9)"
        ))),
    }
}

/// A `DateTime64` scale argument: a constant integer; anything else is
/// `43`, as ClickHouse's `toDecimal()` scale check answers (a signed bound
/// parameter, `_CAST(3, 'Int64')`, is not folded into a constant, R30.4).
fn scale_arg(sig: &Sig, i: usize) -> Result<Option<i64>, ChError> {
    sig.const_int(i).map_err(|_| {
        ChError::illegal_type_of_argument(format!(
            "Illegal type of toDecimal() scale {}",
            sig.types[i]
        ))
    })
}

fn datetime64_type(sig: &Sig) -> Result<ChType, ChError> {
    sig.arity(2, 3)?;
    check_source(sig, true)?;
    let p = precision(sig.name, scale_arg(sig, 1)?, 3)?;
    let zone = sig.zone_arg(2)?.or_else(|| own_zone(&sig.types[0]));
    Ok(ChType::DateTime64(p, zone))
}

fn datetime64_eval(call: &Call) -> Result<ArrayRef, ChError> {
    let t = &call.types[0];
    let tz = call.zone(2, temporal(t).and_then(|(_, z)| z))?;
    let a = call.arrays[0].as_ref();
    let (lo, hi) = DATETIME64_RANGE;
    let values: Vec<Option<(i64, u32)>> = if temporal(t).is_some() || is_text(t) {
        instants(call, 0, tz)?
    } else if is_float(a.data_type()) {
        floats(a)?
            .into_iter()
            .map(|v| {
                v.map(|f| {
                    let s = f.floor();
                    let n = ((f - s) * 1e9).round() as u32;
                    if s < lo as f64 {
                        (lo, 0)
                    } else if s > hi as f64 {
                        (hi, 999_999_999)
                    } else {
                        (s as i64, n.min(999_999_999))
                    }
                })
            })
            .collect()
    } else {
        ints(a)?
            .into_iter()
            .map(|v| {
                v.map(|s| {
                    if s > hi {
                        (hi, 999_999_999)
                    } else {
                        (s.max(lo), 0)
                    }
                })
            })
            .collect()
    };
    write(
        &call.ret,
        values
            .into_iter()
            .map(|v| v.map(|(s, n)| Out::Time(s, n)))
            .collect(),
    )
}

fn timezone_type(sig: &Sig) -> Result<ChType, ChError> {
    sig.arity(2, 2)?;
    let zone = sig.zone_arg(1)?;
    match &sig.types[0] {
        ChType::DateTime(_) => Ok(ChType::DateTime(zone)),
        ChType::DateTime64(p, _) => Ok(ChType::DateTime64(*p, zone)),
        other => Err(illegal(sig.name, other, 0)),
    }
}

fn timezone_eval(call: &Call) -> Result<ArrayRef, ChError> {
    let vals = points(call.arrays[0].as_ref())?;
    write(
        &call.ret,
        vals.into_iter()
            .map(|p| match p {
                Some(Point::Time(s, n)) => Some(Out::Time(s, n)),
                _ => None,
            })
            .collect(),
    )
}

fn unix_type(sig: &Sig) -> Result<ChType, ChError> {
    sig.arity(1, 2)?;
    check_source(sig, false)?;
    sig.zone_arg(1)?;
    Ok(ChType::UInt32)
}

fn unix_eval(call: &Call) -> Result<ArrayRef, ChError> {
    let t = &call.types[0];
    let tz = call.zone(1, temporal(t).and_then(|(_, z)| z))?;
    let v = instants(call, 0, tz)?;
    let out = v
        .into_iter()
        .map(|x| {
            x.map(|(s, _)| {
                if matches!(temporal(t), Some((Kind::DateTime64(_), _))) && s < 0 {
                    Err(ChError {
                        code: 407,
                        name: "DECIMAL_OVERFLOW",
                        message: "Convert overflow".into(),
                    })
                } else {
                    Ok(Out::UInt(s.clamp(0, DATETIME_MAX) as u64))
                }
            })
            .transpose()
        })
        .collect::<Result<Vec<_>, _>>()?;
    write(&call.ret, out)
}

/// The digits after the second of `toUnixTimestamp64<Unit>` and
/// `fromUnixTimestamp64<Unit>`.
fn unit_digits(name: &str) -> u8 {
    if name.ends_with("Milli") {
        3
    } else if name.ends_with("Micro") {
        6
    } else if name.ends_with("Nano") {
        9
    } else {
        0
    }
}

fn to_unix64_type(sig: &Sig) -> Result<ChType, ChError> {
    sig.arity(1, 1)?;
    match &sig.types[0] {
        ChType::DateTime64(..) => Ok(ChType::Int64),
        other => Err(ChError::illegal_type_of_argument(format!(
            "A value of illegal type was provided as 1st argument 'value' to function '{}'. Expected: DateTime64, got: {other}",
            sig.name
        ))),
    }
}

fn to_unix64_eval(call: &Call) -> Result<ArrayRef, ChError> {
    let d = unit_digits(call.name);
    let per = pow10(d);
    let out = points(call.arrays[0].as_ref())?
        .into_iter()
        .map(|p| match p {
            Some(Point::Time(s, n)) => Some(Out::Int(
                s.wrapping_mul(per)
                    .wrapping_add(i64::from(n) / pow10(9 - d)),
            )),
            _ => None,
        })
        .collect();
    write(&call.ret, out)
}

fn from_unix64_type(sig: &Sig) -> Result<ChType, ChError> {
    sig.arity(1, 2)?;
    if !matches!(
        sig.types[0],
        ChType::Int8
            | ChType::Int16
            | ChType::Int32
            | ChType::Int64
            | ChType::UInt8
            | ChType::UInt16
            | ChType::UInt32
            | ChType::UInt64
    ) {
        return Err(illegal(sig.name, &sig.types[0], 0));
    }
    let zone = sig.zone_arg(1)?;
    Ok(ChType::DateTime64(unit_digits(sig.name), zone))
}

fn from_unix64_eval(call: &Call) -> Result<ArrayRef, ChError> {
    let per = pow10(unit_digits(call.name));
    let nanos = pow10(9 - unit_digits(call.name));
    let out = ints(call.arrays[0].as_ref())?
        .into_iter()
        .map(|v| v.map(|t| Out::Time(t.div_euclid(per), (t.rem_euclid(per) * nanos) as u32)))
        .collect();
    write(&call.ret, out)
}

fn now_type(sig: &Sig) -> Result<ChType, ChError> {
    if sig.name == "now64" {
        sig.arity(0, 2)?;
        let p = precision(sig.name, scale_arg(sig, 0)?, 3)?;
        Ok(ChType::DateTime64(p, sig.zone_arg(1)?))
    } else {
        sig.arity(0, 1)?;
        Ok(ChType::DateTime(sig.zone_arg(0)?))
    }
}

fn now_value(t: &ChType, at: chrono::DateTime<chrono::Utc>) -> ScalarValue {
    let s = at.timestamp();
    let n = i64::from(at.timestamp_subsec_nanos());
    match t {
        ChType::DateTime(z) => {
            ScalarValue::TimestampSecond(Some(s), z.as_deref().map(std::sync::Arc::from))
        }
        ChType::DateTime64(p, z) => {
            let unit = loams_house_types::arrow::time_unit(*p);
            let per_sec = 1_000_000_000 / super::value::unit_nanos(unit);
            let keep = pow10(9 - *p);
            let v = s * per_sec + (n - n % keep) / super::value::unit_nanos(unit);
            let z = z.as_deref().map(std::sync::Arc::from);
            match unit {
                datafusion::arrow::datatypes::TimeUnit::Second => {
                    ScalarValue::TimestampSecond(Some(v), z)
                }
                datafusion::arrow::datatypes::TimeUnit::Millisecond => {
                    ScalarValue::TimestampMillisecond(Some(v), z)
                }
                datafusion::arrow::datatypes::TimeUnit::Microsecond => {
                    ScalarValue::TimestampMicrosecond(Some(v), z)
                }
                datafusion::arrow::datatypes::TimeUnit::Nanosecond => {
                    ScalarValue::TimestampNanosecond(Some(v), z)
                }
            }
        }
        _ => ScalarValue::Null,
    }
}

fn now_eval(call: &Call) -> Result<ArrayRef, ChError> {
    let v = now_value(&call.ret, chrono::Utc::now());
    v.to_array_of_size(call.rows)
        .map_err(|e| ChError::logical_error(e.to_string()))
}

/// `now()` is the query's start time, the same in every row and every call
/// of the query (as ClickHouse's), folded into a constant.
fn now_fold(info: &SimplifyContext, t: &ChType, _args: &[Expr]) -> Option<ScalarValue> {
    Some(now_value(
        t,
        info.query_execution_start_time()
            .unwrap_or_else(chrono::Utc::now),
    ))
}

fn best_effort_type(sig: &Sig) -> Result<ChType, ChError> {
    let wide = sig.name.starts_with("parseDateTime64");
    if wide {
        sig.arity(1, 3)?;
    } else {
        sig.arity(1, 2)?;
    }
    if !is_text(&sig.types[0]) {
        return Err(illegal(sig.name, &sig.types[0], 0));
    }
    let t = if wide {
        let p = precision(sig.name, scale_arg(sig, 1)?, 3)?;
        ChType::DateTime64(p, sig.zone_arg(2)?)
    } else {
        ChType::DateTime(sig.zone_arg(1)?)
    };
    Ok(if sig.name.ends_with("OrNull") {
        ChType::Nullable(Box::new(t))
    } else {
        t
    })
}

fn best_effort_eval(call: &Call) -> Result<ArrayRef, ChError> {
    let wide = call.name.starts_with("parseDateTime64");
    let tz = call.zone(if wide { 2 } else { 1 }, None)?;
    let us = call.name.contains("US");
    let or_null = call.name.ends_with("OrNull");
    let or_zero = call.name.ends_with("OrZero");
    let year = i64::from(chrono::Utc::now().with_timezone(&tz).year());
    let texts = strings(call.arrays[0].as_ref())?;
    let out = call.map(|row| {
        let s = texts[row].as_deref().unwrap_or("");
        match best_effort(s, us, year) {
            Some(c) => Ok(Some(civil_out(&c, tz, wide))),
            None if or_null => Ok(None),
            None if or_zero => Ok(Some(Out::Time(0, 0))),
            None => Err(cannot_parse_datetime(s)),
        }
    })?;
    write(&call.ret, out)
}

fn civil_out(c: &Civil, tz: Tz, wide: bool) -> Out {
    let s = c.instant(tz);
    if wide {
        Out::Time(s.clamp(DATETIME64_RANGE.0, DATETIME64_RANGE.1), c.nanos)
    } else {
        Out::Time(s.clamp(0, DATETIME_MAX), 0)
    }
}

/// The conversion functions.
pub fn functions() -> Vec<ChFunction> {
    let mut v = vec![
        ChFunction::new("toDate", date_type, date_eval),
        ChFunction::new("toDate32", date_type, date_eval),
        ChFunction::new("toDateTime", datetime_type, datetime_eval),
        ChFunction::new("toDateTime64", datetime64_type, datetime64_eval),
        ChFunction::new("toTimeZone", timezone_type, timezone_eval),
        ChFunction::new("toTimezone", timezone_type, timezone_eval),
        ChFunction::new("toUnixTimestamp", unix_type, unix_eval),
        ChFunction::stable("now", now_type, now_eval, now_fold),
        ChFunction::stable("now64", now_type, now_eval, now_fold),
    ];
    for unit in ["Second", "Milli", "Micro", "Nano"] {
        v.push(ChFunction::new(
            &format!("toUnixTimestamp64{unit}"),
            to_unix64_type,
            to_unix64_eval,
        ));
        v.push(ChFunction::new(
            &format!("fromUnixTimestamp64{unit}"),
            from_unix64_type,
            from_unix64_eval,
        ));
    }
    for base in ["parseDateTimeBestEffort", "parseDateTime64BestEffort"] {
        for us in ["", "US"] {
            for or in ["", "OrNull", "OrZero"] {
                v.push(ChFunction::new(
                    &format!("{base}{us}{or}"),
                    best_effort_type,
                    best_effort_eval,
                ));
            }
        }
    }
    v
}
