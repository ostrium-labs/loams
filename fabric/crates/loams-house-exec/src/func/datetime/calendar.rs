//! Calendar parts and truncation: `toYear` … `toSecond`, `toYYYYMM`,
//! `toYYYYMMDD`, `toDayOfWeek`, the `toStartOf*` family,
//! `toStartOfInterval` and `dateTrunc`.
//!
//! Ported from ClickHouse's `src/Common/DateLUTImpl.h`,
//! `src/Functions/DateTimeTransforms.h`, `src/Functions/toStartOfInterval.cpp`
//! and `src/Functions/dateTrunc.cpp` (Apache-2.0): the rounding rules, the
//! result types and their saturation (a `Date` result clamps to
//! 1970-01-01..2149-06-06, a `DateTime` result to the `UInt32` range).

use chrono_tz::Tz;
use datafusion::arrow::array::ArrayRef;
use loams_house_types::{ChError, ChType};

use super::convert::{DATE_MAX, DATE32_RANGE, DATETIME_MAX};
use super::udf::{Call, ChFunction, Sig, illegal, own_zone};
use super::value::{Kind, Out, Point, ints, points, pow10, temporal, write};
use super::zone::{civil, day_start, days, epoch_facts, local, month_days, to_utc, weekday};

/// A row's local calendar position: the day, the second of the day and the
/// nanosecond, and the instant (for a time).
#[derive(Debug, Clone, Copy)]
pub struct Local {
    /// The local day.
    pub day: i64,
    /// Seconds into the local day.
    pub sod: i64,
    /// Nanoseconds into the second.
    pub nanos: u32,
    /// The instant, for a time; `None` for a date.
    pub secs: Option<i64>,
}

/// The local positions of argument `i` (a date or a time) in `tz`.
pub fn locals(call: &Call, i: usize, tz: Tz) -> Result<Vec<Option<Local>>, ChError> {
    Ok(points(call.arrays[i].as_ref())?
        .into_iter()
        .map(|p| {
            p.map(|p| match p {
                Point::Day(d) => Local {
                    day: d,
                    sod: 0,
                    nanos: 0,
                    secs: None,
                },
                Point::Time(s, n) => {
                    let (day, sod) = local(tz, s);
                    Local {
                        day,
                        sod,
                        nanos: n,
                        secs: Some(s),
                    }
                }
            })
        })
        .collect())
}

/// The zone a function of argument 0 reads it in: argument `zone_arg`, else
/// the type's own, else the session's.
fn zone0(call: &Call, zone_arg: usize) -> Result<Tz, ChError> {
    call.zone(zone_arg, temporal(&call.types[0]).and_then(|(_, z)| z))
}

fn need_temporal(sig: &Sig, times_only: bool) -> Result<Kind, ChError> {
    match temporal(&sig.types[0]) {
        Some((Kind::Date | Kind::Date32, _)) if times_only => {
            Err(illegal(sig.name, &sig.types[0], 0))
        }
        Some((k, _)) => Ok(k),
        None => Err(illegal(sig.name, &sig.types[0], 0)),
    }
}

// Parts ------------------------------------------------------------------

fn part_type(sig: &Sig) -> Result<ChType, ChError> {
    let times_only = matches!(sig.name, "toHour" | "toMinute" | "toSecond");
    let zone_at = if sig.name == "toDayOfWeek" {
        sig.arity(1, 3)?;
        sig.const_int(1)?;
        2
    } else {
        sig.arity(1, 2)?;
        1
    };
    need_temporal(sig, times_only)?;
    sig.zone_arg(zone_at)?;
    Ok(match sig.name {
        "toYear" | "toDayOfYear" => ChType::UInt16,
        "toYYYYMM" | "toYYYYMMDD" => ChType::UInt32,
        "toYYYYMMDDhhmmss" => ChType::UInt64,
        _ => ChType::UInt8,
    })
}

fn part_eval(call: &Call) -> Result<ArrayRef, ChError> {
    let (zone_at, mode) = if call.name == "toDayOfWeek" {
        (2, call.const_int(1)?.unwrap_or(0))
    } else {
        (1, 0)
    };
    let tz = zone0(call, zone_at)?;
    let out = locals(call, 0, tz)?
        .into_iter()
        .map(|l| {
            l.map(|l| {
                let (y, m, d) = civil(l.day);
                let (h, mi, s) = (l.sod / 3600, l.sod / 60 % 60, l.sod % 60);
                let y_u = y as u64;
                Out::UInt(match call.name {
                    "toYear" => y_u,
                    "toQuarter" => u64::from((m - 1) / 3 + 1),
                    "toMonth" => u64::from(m),
                    "toDayOfMonth" => u64::from(d),
                    "toDayOfYear" => (l.day - days(y, 1, 1) + 1) as u64,
                    "toHour" => h as u64,
                    "toMinute" => mi as u64,
                    "toSecond" => s as u64,
                    "toYYYYMM" => y_u * 100 + u64::from(m),
                    "toYYYYMMDD" => (y_u * 100 + u64::from(m)) * 100 + u64::from(d),
                    "toYYYYMMDDhhmmss" => {
                        ((y_u * 100 + u64::from(m)) * 100 + u64::from(d)) * 1_000_000
                            + (h * 10_000 + mi * 100 + s) as u64
                    }
                    // toDayOfWeek: modes 0 (Monday 1–7), 1 (Monday 0–6),
                    // 2 (Sunday 0–6), 3 (Sunday 1–7).
                    _ => {
                        let iso = u64::from(weekday(l.day));
                        match mode {
                            1 => iso - 1,
                            2 => iso % 7,
                            3 => iso % 7 + 1,
                            _ => iso,
                        }
                    }
                })
            })
        })
        .collect();
    write(&call.ret, out)
}

// toStartOf* -------------------------------------------------------------

/// `DateLUTImpl::roundDown`: `t` rounded down to a multiple of `divisor`
/// seconds, counted from the epoch where the zone's offsets are whole hours
/// (and `t` is not before the epoch), else from the local day's start.
pub fn round_down(tz: Tz, t: i64, divisor: i64, whole: bool) -> i64 {
    if t >= 0 && whole {
        return t.div_euclid(divisor) * divisor;
    }
    let date = day_start(tz, local(tz, t).0);
    date + (t - date) / divisor * divisor
}

fn start_of_minute_interval(tz: Tz, t: i64, minutes: i64) -> i64 {
    round_down(
        tz,
        t,
        minutes.saturating_mul(60),
        epoch_facts(tz).whole_minutes,
    )
}

fn start_of_second_interval(tz: Tz, t: i64, seconds: i64) -> i64 {
    if seconds == 1 {
        t
    } else if seconds % 60 == 0 {
        start_of_minute_interval(tz, t, seconds / 60)
    } else {
        round_down(tz, t, seconds, epoch_facts(tz).whole_hours)
    }
}

/// `toStartOfHourInterval`: hours counted from the local midnight.
fn start_of_hour_interval(tz: Tz, t: i64, hours: i64) -> i64 {
    if hours == 1 {
        return round_down(tz, t, 3600, epoch_facts(tz).whole_hours);
    }
    let (day, sod) = local(tz, t);
    let h = sod / 3600;
    to_utc(tz, day, (h - h % hours) * 3600)
}

/// The Monday on or before day `d`.
fn monday(d: i64) -> i64 {
    d - i64::from(weekday(d)) + 1
}

fn first_of_month(d: i64) -> i64 {
    let (y, m, _) = civil(d);
    days(y, m, 1)
}

fn first_of_quarter(d: i64) -> i64 {
    let (y, m, _) = civil(d);
    days(y, (m - 1) / 3 * 3 + 1, 1)
}

fn first_of_year(d: i64) -> i64 {
    days(civil(d).0, 1, 1)
}

fn start_type(sig: &Sig) -> Result<ChType, ChError> {
    let zone_at = if sig.name == "toStartOfWeek" {
        sig.arity(1, 3)?;
        sig.const_int(1)?;
        2
    } else {
        sig.arity(1, 2)?;
        1
    };
    let sub_day = !matches!(
        sig.name,
        "toStartOfDay"
            | "toStartOfWeek"
            | "toMonday"
            | "toStartOfMonth"
            | "toStartOfQuarter"
            | "toStartOfYear"
            | "toLastDayOfMonth"
    );
    let kind = need_temporal(sig, sub_day)?;
    let zone = sig.zone_arg(zone_at)?.or_else(|| own_zone(&sig.types[0]));
    Ok(match sig.name {
        "toStartOfWeek" | "toMonday" | "toStartOfMonth" | "toStartOfQuarter" | "toStartOfYear"
        | "toLastDayOfMonth" => {
            if kind == Kind::Date32 {
                ChType::Date32
            } else {
                ChType::Date
            }
        }
        _ => ChType::DateTime(zone),
    })
}

fn start_eval(call: &Call) -> Result<ArrayRef, ChError> {
    let (zone_at, mode) = if call.name == "toStartOfWeek" {
        (2, call.const_int(1)?.unwrap_or(0))
    } else {
        (1, 0)
    };
    let tz = zone0(call, zone_at)?;
    let date_range = if call.ret == ChType::Date32 {
        DATE32_RANGE
    } else {
        (0, DATE_MAX)
    };
    let out = locals(call, 0, tz)?
        .into_iter()
        .map(|l| {
            l.map(|l| {
                let secs = l.secs.unwrap_or(0);
                let time = |t: i64| Out::Time(t.clamp(0, DATETIME_MAX), 0);
                let day = |d: i64| Out::Day(d.clamp(date_range.0, date_range.1));
                match call.name {
                    "toStartOfMinute" => time(start_of_minute_interval(tz, secs, 1)),
                    "toStartOfFiveMinutes" | "toStartOfFiveMinute" => {
                        time(start_of_minute_interval(tz, secs, 5))
                    }
                    "toStartOfTenMinutes" => time(start_of_minute_interval(tz, secs, 10)),
                    "toStartOfFifteenMinutes" => time(start_of_minute_interval(tz, secs, 15)),
                    "toStartOfHour" => time(start_of_hour_interval(tz, secs, 1)),
                    "toStartOfDay" => time(day_start(tz, l.day)),
                    "toMonday" => day(monday(l.day)),
                    "toStartOfWeek" => {
                        if mode % 2 == 1 {
                            day(monday(l.day))
                        } else {
                            day(l.day - i64::from(weekday(l.day) % 7))
                        }
                    }
                    "toStartOfMonth" => day(first_of_month(l.day)),
                    "toStartOfQuarter" => day(first_of_quarter(l.day)),
                    "toStartOfYear" => day(first_of_year(l.day)),
                    _ => {
                        // toLastDayOfMonth
                        let (y, m, _) = civil(l.day);
                        day(days(y, m, month_days(y, m)))
                    }
                }
            })
        })
        .collect();
    write(&call.ret, out)
}

/// An interval unit by the name its type carries (`IntervalDay`: `Day`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    /// Nanoseconds.
    Nanosecond,
    /// Microseconds.
    Microsecond,
    /// Milliseconds.
    Millisecond,
    /// Seconds.
    Second,
    /// Minutes.
    Minute,
    /// Hours.
    Hour,
    /// Days.
    Day,
    /// Weeks.
    Week,
    /// Months.
    Month,
    /// Quarters.
    Quarter,
    /// Years.
    Year,
}

impl Unit {
    /// The unit named `name` (`Day`).
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "Nanosecond" => Self::Nanosecond,
            "Microsecond" => Self::Microsecond,
            "Millisecond" => Self::Millisecond,
            "Second" => Self::Second,
            "Minute" => Self::Minute,
            "Hour" => Self::Hour,
            "Day" => Self::Day,
            "Week" => Self::Week,
            "Month" => Self::Month,
            "Quarter" => Self::Quarter,
            "Year" => Self::Year,
            _ => return None,
        })
    }

    /// The digits after the second of a sub-second unit.
    pub fn scale(self) -> Option<u8> {
        match self {
            Self::Millisecond => Some(3),
            Self::Microsecond => Some(6),
            Self::Nanosecond => Some(9),
            _ => None,
        }
    }

    /// Seconds in one unit, for the units of fixed length.
    pub fn seconds(self) -> Option<i64> {
        match self {
            Self::Second => Some(1),
            Self::Minute => Some(60),
            Self::Hour => Some(3600),
            _ => None,
        }
    }
}

/// The unit of an `Interval*` type.
pub fn interval_unit(t: &ChType) -> Option<Unit> {
    match t.base() {
        ChType::Interval(u) => Unit::parse(u),
        _ => None,
    }
}

fn interval_type(sig: &Sig) -> Result<ChType, ChError> {
    sig.arity(2, 3)?;
    let kind = need_temporal(sig, false)?;
    let unit = interval_unit(&sig.types[1]).ok_or_else(|| illegal(sig.name, &sig.types[1], 1))?;
    if sig.types.len() == 3 && !super::convert::is_text(&sig.types[2]) {
        return Err(ChError::unsupported(
            "toStartOfInterval with an origin argument",
        ));
    }
    let zone = sig.zone_arg(2)?.or_else(|| own_zone(&sig.types[0]));
    let is_date = matches!(kind, Kind::Date | Kind::Date32);
    Ok(match unit {
        Unit::Nanosecond | Unit::Microsecond | Unit::Millisecond => match kind {
            Kind::DateTime64(_) => ChType::DateTime64(unit.scale().unwrap_or(3), zone),
            _ => {
                return Err(ChError::illegal_type_of_argument(format!(
                    "Illegal interval kind for argument data type {}",
                    if is_date { "Date" } else { "DateTime" }
                )));
            }
        },
        Unit::Second | Unit::Minute | Unit::Hour if is_date => {
            return Err(ChError::illegal_type_of_argument(
                "Illegal interval kind for argument data type Date",
            ));
        }
        Unit::Second | Unit::Minute | Unit::Hour | Unit::Day => ChType::DateTime(zone),
        Unit::Week | Unit::Month | Unit::Quarter | Unit::Year => ChType::Date,
    })
}

fn interval_eval(call: &Call) -> Result<ArrayRef, ChError> {
    let unit =
        interval_unit(&call.types[1]).ok_or_else(|| illegal(call.name, &call.types[1], 1))?;
    let tz = zone0(call, 2)?;
    let n_values = ints(call.arrays[1].as_ref())?;
    let ls = locals(call, 0, tz)?;
    let out = call.map(|row| {
        let (Some(l), Some(n)) = (ls[row], n_values[row]) else {
            return Ok(None);
        };
        if n <= 0 {
            return Err(ChError::argument_out_of_bound(format!(
                "Value for 2nd argument of function {} must be positive",
                call.name
            )));
        }
        let secs = l.secs.unwrap_or(0);
        let time = |t: i64| Out::Time(t.clamp(0, DATETIME_MAX), 0);
        let date = |d: i64| Out::Day(d.clamp(0, DATE_MAX));
        Ok(Some(match unit {
            Unit::Nanosecond | Unit::Microsecond | Unit::Millisecond => {
                let s = unit.scale().unwrap_or(3);
                let per = pow10(s);
                let total = secs
                    .saturating_mul(per)
                    .saturating_add(i64::from(l.nanos) / pow10(9 - s));
                let floored = total.div_euclid(n) * n;
                Out::Time(
                    floored.div_euclid(per),
                    (floored.rem_euclid(per) * pow10(9 - s)) as u32,
                )
            }
            Unit::Second => time(start_of_second_interval(tz, secs, n)),
            Unit::Minute => time(start_of_minute_interval(tz, secs, n)),
            Unit::Hour => time(start_of_hour_interval(tz, secs, n)),
            Unit::Day => time(day_start(tz, l.day.div_euclid(n) * n)),
            Unit::Week => {
                if n == 1 {
                    date(monday(l.day))
                } else {
                    date(4 + (l.day - 4).div_euclid(7 * n) * 7 * n)
                }
            }
            Unit::Month | Unit::Quarter => {
                let months = if unit == Unit::Quarter { 3 * n } else { n };
                let (y, m, _) = civil(l.day);
                let idx = (y - 1900) * 12 + i64::from(m) - 1;
                let idx = idx.div_euclid(months) * months;
                date(days(
                    1900 + idx.div_euclid(12),
                    (idx.rem_euclid(12) + 1) as u32,
                    1,
                ))
            }
            Unit::Year => {
                let y = (civil(l.day).0.div_euclid(n) * n).max(1900);
                date(days(y, 1, 1))
            }
        }))
    })?;
    write(&call.ret, out)
}

// dateTrunc --------------------------------------------------------------

fn trunc_unit(name: &str, unit: &str) -> Result<Unit, ChError> {
    Ok(match unit.to_ascii_lowercase().as_str() {
        "nanosecond" => Unit::Nanosecond,
        "microsecond" => Unit::Microsecond,
        "millisecond" => Unit::Millisecond,
        "second" => Unit::Second,
        "minute" => Unit::Minute,
        "hour" => Unit::Hour,
        "day" => Unit::Day,
        "week" => Unit::Week,
        "month" => Unit::Month,
        "quarter" => Unit::Quarter,
        "year" => Unit::Year,
        other => {
            return Err(ChError::bad_arguments(format!(
                "First argument for function {name} must be a datetime unit, not {other:?}"
            )));
        }
    })
}

fn trunc_type(sig: &Sig) -> Result<ChType, ChError> {
    sig.arity(2, 3)?;
    let unit = trunc_unit(sig.name, &sig.const_str(0)?.unwrap_or_default())?;
    let kind = match temporal(&sig.types[1]) {
        Some((k, _)) => k,
        None => return Err(illegal(sig.name, &sig.types[1], 1)),
    };
    let zone = sig.zone_arg(2)?.or_else(|| own_zone(&sig.types[1]));
    let wide = matches!(kind, Kind::Date32 | Kind::DateTime64(_));
    let is_date = matches!(kind, Kind::Date | Kind::Date32);
    Ok(match unit {
        Unit::Week | Unit::Month | Unit::Quarter | Unit::Year => {
            if wide {
                ChType::Date32
            } else {
                ChType::Date
            }
        }
        Unit::Second | Unit::Minute | Unit::Hour if is_date => {
            return Err(illegal(sig.name, &sig.types[1], 1));
        }
        Unit::Nanosecond | Unit::Microsecond | Unit::Millisecond => {
            return Err(ChError::unsupported(format!(
                "dateTrunc to {unit:?} (plan HE1 Task 30)"
            )));
        }
        _ if wide => ChType::DateTime64(0, zone),
        _ => ChType::DateTime(zone),
    })
}

fn trunc_eval(call: &Call) -> Result<ArrayRef, ChError> {
    let unit = trunc_unit(call.name, &call.const_str(0)?.unwrap_or_default())?;
    let tz = call.zone(2, temporal(&call.types[1]).and_then(|(_, z)| z))?;
    let wide = matches!(call.ret, ChType::Date32 | ChType::DateTime64(..));
    let out = locals(call, 1, tz)?
        .into_iter()
        .map(|l| {
            l.map(|l| {
                let secs = l.secs.unwrap_or_else(|| day_start(tz, l.day));
                let time = |t: i64| Out::Time(if wide { t } else { t.clamp(0, DATETIME_MAX) }, 0);
                let date = |d: i64| {
                    Out::Day(if wide {
                        d.clamp(DATE32_RANGE.0, DATE32_RANGE.1)
                    } else {
                        d.clamp(0, DATE_MAX)
                    })
                };
                match unit {
                    Unit::Second => time(secs),
                    Unit::Minute => time(start_of_minute_interval(tz, secs, 1)),
                    Unit::Hour => time(start_of_hour_interval(tz, secs, 1)),
                    Unit::Day => time(day_start(tz, l.day)),
                    Unit::Week => date(monday(l.day)),
                    Unit::Month => date(first_of_month(l.day)),
                    Unit::Quarter => date(first_of_quarter(l.day)),
                    _ => date(first_of_year(l.day)),
                }
            })
        })
        .collect();
    write(&call.ret, out)
}

/// The calendar functions.
pub fn functions() -> Vec<ChFunction> {
    let mut v = Vec::new();
    for name in [
        "toYear",
        "toQuarter",
        "toMonth",
        "toDayOfMonth",
        "toDayOfWeek",
        "toDayOfYear",
        "toHour",
        "toMinute",
        "toSecond",
        "toYYYYMM",
        "toYYYYMMDD",
        "toYYYYMMDDhhmmss",
    ] {
        v.push(ChFunction::new(name, part_type, part_eval));
    }
    for name in [
        "toStartOfMinute",
        "toStartOfFiveMinutes",
        "toStartOfFiveMinute",
        "toStartOfTenMinutes",
        "toStartOfFifteenMinutes",
        "toStartOfHour",
        "toStartOfDay",
        "toStartOfWeek",
        "toMonday",
        "toStartOfMonth",
        "toStartOfQuarter",
        "toStartOfYear",
        "toLastDayOfMonth",
    ] {
        v.push(ChFunction::new(name, start_type, start_eval));
    }
    v.push(ChFunction::new(
        "toStartOfInterval",
        interval_type,
        interval_eval,
    ));
    v.push(ChFunction::new("dateTrunc", trunc_type, trunc_eval));
    v
}
