//! Intervals and date arithmetic: `toInterval*`, `add*`/`subtract*`,
//! `plus`/`minus` of dates, times, intervals and numbers ([`DatePlanner`]),
//! `dateDiff` and `timeDiff`.
//!
//! Ported from ClickHouse's `src/Functions/FunctionDateOrDateTimeAddInterval.h`
//! and `src/Functions/dateDiff.cpp` (Apache-2.0), with the result types and
//! the wrapping measured on the oracle (R30.3, R30.5):
//!
//! * `Date ± days` wraps as a `UInt16`, `DateTime ± seconds` as a `UInt32`;
//!   `Date32` and `DateTime64` do not wrap;
//! * a `Date` plus hours, minutes or seconds is a `DateTime`, plus a
//!   sub-second unit is `43`; a `DateTime` plus a sub-second unit is a
//!   `DateTime64` of that unit's scale; days, weeks, months, quarters and
//!   years move the local calendar (a month keeps the day, clamped);
//! * a fractional count is truncated toward zero;
//! * `dateDiff` counts the unit's boundaries between the two values, each in
//!   its zone.

use std::sync::Arc;

use chrono_tz::Tz;
use datafusion::arrow::array::ArrayRef;
use datafusion::common::{DFSchema, Result};
use datafusion::logical_expr::expr::ScalarFunction;
use datafusion::logical_expr::planner::{ExprPlanner, PlannerResult, RawBinaryExpr};
use datafusion::logical_expr::{Expr, ExprSchemable, ScalarUDF};
use datafusion::sql::sqlparser::ast::BinaryOperator;
use loams_house_sql::context::external;
use loams_house_types::arrow::from_arrow;
use loams_house_types::{ChError, ChType};

use super::calendar::{Unit, interval_unit, locals};
use super::convert::{DATE_MAX, DATETIME_MAX, is_number, is_text};
use super::udf::{Call, ChFunction, Sig, illegal};
use super::value::{Kind, Out, Point, ints, points, pow10, temporal, write};
use super::zone::{add_months, civil, day_start, epoch_facts, local, to_utc, weekday};

/// The units, by the names of their functions.
pub const UNITS: [&str; 11] = [
    "Nanosecond",
    "Microsecond",
    "Millisecond",
    "Second",
    "Minute",
    "Hour",
    "Day",
    "Week",
    "Month",
    "Quarter",
    "Year",
];

// toInterval* --------------------------------------------------------------

fn to_interval_type(sig: &Sig) -> Result<ChType, ChError> {
    sig.arity(1, 1)?;
    let t = &sig.types[0];
    if !(is_number(t) || is_text(t)) {
        return Err(illegal(sig.name, t, 0));
    }
    Ok(ChType::Interval(
        sig.name.trim_start_matches("toInterval").to_string(),
    ))
}

fn to_interval_eval(call: &Call) -> Result<ArrayRef, ChError> {
    let v = ints(call.arrays[0].as_ref())?;
    write(&call.ret, v.into_iter().map(|x| x.map(Out::Int)).collect())
}

// add* / subtract* ---------------------------------------------------------

fn unit_of_add(name: &str) -> Option<(Unit, bool)> {
    let (rest, negate) = if let Some(r) = name.strip_prefix("add") {
        (r, false)
    } else {
        (name.strip_prefix("subtract")?, true)
    };
    Some((Unit::parse(rest.strip_suffix('s')?)?, negate))
}

/// The type of `t` moved by `unit`s.
fn moved_type(name: &str, t: &ChType, unit: Unit) -> Result<ChType, ChError> {
    let Some((kind, zone)) = temporal(t) else {
        return Err(illegal(name, t, 0));
    };
    let zone = zone.map(str::to_string);
    Ok(match (kind, unit.scale(), unit.seconds()) {
        (Kind::Date | Kind::Date32, Some(_), _) => {
            return Err(ChError::illegal_type_of_argument(format!(
                "{name} cannot be used with Date"
            )));
        }
        (Kind::Date, None, Some(_)) => ChType::DateTime(None),
        (Kind::Date32, None, Some(_)) => ChType::DateTime64(3, None),
        (Kind::Date, ..) => ChType::Date,
        (Kind::Date32, ..) => ChType::Date32,
        (Kind::DateTime, Some(s), _) => ChType::DateTime64(s, zone),
        (Kind::DateTime, ..) => ChType::DateTime(zone),
        (Kind::DateTime64(p), Some(s), _) => ChType::DateTime64(p.max(s), zone),
        (Kind::DateTime64(p), ..) => ChType::DateTime64(p, zone),
    })
}

fn add_type(sig: &Sig) -> Result<ChType, ChError> {
    sig.arity(2, 2)?;
    let (unit, _) = unit_of_add(sig.name)
        .ok_or_else(|| ChError::logical_error(format!("{} is not an add function", sig.name)))?;
    let n = &sig.types[1];
    if !(is_number(n) || interval_unit(n).is_some()) {
        return Err(illegal(sig.name, n, 1));
    }
    moved_type(sig.name, &sig.types[0], unit)
}

/// `p` moved by `n` `unit`s in `tz`, for a result of type `ret`.
fn shift(p: Point, n: i64, unit: Unit, tz: Tz, ret: &ChType) -> Out {
    let wrap_date = matches!(ret, ChType::Date);
    let wrap_time = matches!(ret, ChType::DateTime(_));
    let date = |d: i64| {
        if wrap_date {
            Out::Day(d.rem_euclid(DATE_MAX + 1))
        } else {
            Out::Day(d)
        }
    };
    let time = |s: i64, nanos: u32| {
        if wrap_time {
            Out::Time(s.rem_euclid(DATETIME_MAX + 1), 0)
        } else {
            Out::Time(s, nanos)
        }
    };
    let calendar = |d: i64| match unit {
        Unit::Day => d.wrapping_add(n),
        Unit::Week => d.wrapping_add(n.wrapping_mul(7)),
        Unit::Month => add_months(d, n),
        Unit::Quarter => add_months(d, n.wrapping_mul(3)),
        _ => add_months(d, n.wrapping_mul(12)),
    };
    match (p, unit.scale(), unit.seconds()) {
        (Point::Day(d), None, None) => date(calendar(d)),
        (Point::Day(d), _, Some(per)) => time(day_start(tz, d) + n.wrapping_mul(per), 0),
        (Point::Day(d), Some(_), None) => date(d),
        (Point::Time(s, nanos), Some(scale), _) => {
            let per = pow10(scale);
            let total = s
                .wrapping_mul(per)
                .wrapping_add(i64::from(nanos) / pow10(9 - scale))
                .wrapping_add(n);
            let sub = i64::from(nanos) % pow10(9 - scale);
            time(
                total.div_euclid(per),
                (total.rem_euclid(per) * pow10(9 - scale) + sub) as u32,
            )
        }
        (Point::Time(s, nanos), None, Some(per)) => {
            time(s.wrapping_add(n.wrapping_mul(per)), nanos)
        }
        (Point::Time(s, nanos), None, None) => {
            let (d, sod) = local(tz, s);
            time(to_utc(tz, calendar(d), sod), nanos)
        }
    }
}

fn add_eval(call: &Call) -> Result<ArrayRef, ChError> {
    let (unit, negate) = unit_of_add(call.name)
        .ok_or_else(|| ChError::logical_error(format!("{} is not an add function", call.name)))?;
    let tz = call.zone(usize::MAX, temporal(&call.types[0]).and_then(|(_, z)| z))?;
    let ps = points(call.arrays[0].as_ref())?;
    let ns = ints(call.arrays[1].as_ref())?;
    let out = call.map(|row| {
        let (Some(p), Some(n)) = (ps[row], ns[row]) else {
            return Ok(None);
        };
        let n = if negate { n.wrapping_neg() } else { n };
        Ok(Some(shift(p, n, unit, tz, &call.ret)))
    })?;
    write(&call.ret, out)
}

// The date minus ------------------------------------------------------------

/// `minus` of two dates (`Int32` days), two `DateTime`s (`Int32` seconds),
/// or `DateTime64`s (`Decimal(18, P)` seconds), as ClickHouse types them.
pub const DATE_MINUS: &str = "__ch_date_minus";

fn minus_type(sig: &Sig) -> Result<ChType, ChError> {
    sig.arity(2, 2)?;
    let (a, b) = (temporal(&sig.types[0]), temporal(&sig.types[1]));
    let bad = || {
        ChError::illegal_type_of_argument(format!(
            "Illegal types {} and {} of arguments of function minus",
            sig.types[0], sig.types[1]
        ))
    };
    Ok(match (a.map(|x| x.0), b.map(|x| x.0)) {
        (Some(Kind::Date), Some(Kind::Date)) | (Some(Kind::Date32), Some(Kind::Date32)) => {
            ChType::Int32
        }
        (Some(Kind::DateTime), Some(Kind::DateTime)) => ChType::Int32,
        (Some(Kind::DateTime64(p)), Some(Kind::DateTime64(q))) => ChType::Decimal {
            precision: 18,
            scale: p.max(q),
        },
        (Some(Kind::DateTime64(p)), Some(Kind::DateTime))
        | (Some(Kind::DateTime), Some(Kind::DateTime64(p))) => ChType::Decimal {
            precision: 18,
            scale: p,
        },
        _ => return Err(bad()),
    })
}

fn minus_eval(call: &Call) -> Result<ArrayRef, ChError> {
    let a = points(call.arrays[0].as_ref())?;
    let b = points(call.arrays[1].as_ref())?;
    let scale = match call.ret {
        ChType::Decimal { scale, .. } => Some(scale),
        _ => None,
    };
    let out = call.map(|row| {
        Ok(match (a[row], b[row]) {
            (Some(Point::Day(x)), Some(Point::Day(y))) => Some(Out::Int(x - y)),
            (Some(Point::Time(s1, n1)), Some(Point::Time(s2, n2))) => Some(match scale {
                None => Out::Int(i64::from((s1 - s2) as i32)),
                Some(sc) => {
                    let per = i128::from(pow10(sc));
                    let div = i128::from(pow10(9 - sc));
                    let v = |s: i64, n: u32| i128::from(s) * per + i128::from(n) / div;
                    Out::Dec(v(s1, n1) - v(s2, n2))
                }
            }),
            _ => None,
        })
    })?;
    write(&call.ret, out)
}

// dateDiff ------------------------------------------------------------------

fn diff_unit(name: &str, unit: &str) -> Result<Unit, ChError> {
    Ok(match unit.to_ascii_lowercase().as_str() {
        "year" | "years" | "yy" | "yyyy" => Unit::Year,
        "quarter" | "quarters" | "qq" | "q" => Unit::Quarter,
        "month" | "months" | "mm" | "m" => Unit::Month,
        "week" | "weeks" | "wk" | "ww" => Unit::Week,
        "day" | "days" | "dd" | "d" => Unit::Day,
        "hour" | "hours" | "hh" | "h" => Unit::Hour,
        "minute" | "minutes" | "mi" | "n" => Unit::Minute,
        "second" | "seconds" | "ss" | "s" => Unit::Second,
        "millisecond" | "milliseconds" | "ms" => Unit::Millisecond,
        "microsecond" | "microseconds" | "us" | "u" => Unit::Microsecond,
        "nanosecond" | "nanoseconds" | "ns" => Unit::Nanosecond,
        other => {
            return Err(ChError::bad_arguments(format!(
                "Function {name} does not support '{other}' unit"
            )));
        }
    })
}

fn diff_type(sig: &Sig) -> Result<ChType, ChError> {
    let first = if sig.name == "timeDiff" {
        sig.arity(2, 3)?;
        0
    } else {
        sig.arity(3, 4)?;
        diff_unit(sig.name, &sig.const_str(0)?.unwrap_or_default())?;
        1
    };
    for i in [first, first + 1] {
        if temporal(&sig.types[i]).is_none() {
            return Err(illegal(sig.name, &sig.types[i], i));
        }
    }
    sig.zone_arg(first + 2)?;
    Ok(ChType::Int64)
}

/// The relative number of `unit` that `dateDiff` subtracts (ClickHouse's
/// `toRelative*Num`), of a local position in `tz`.
fn relative(unit: Unit, tz: Tz, l: &super::calendar::Local) -> i64 {
    let secs = l.secs.unwrap_or_else(|| day_start(tz, l.day));
    let sub = |scale: u8| {
        secs.wrapping_mul(pow10(scale))
            .wrapping_add(i64::from(l.nanos) / pow10(9 - scale))
    };
    match unit {
        Unit::Year => civil(l.day).0,
        Unit::Quarter => {
            let (y, m, _) = civil(l.day);
            y * 4 + i64::from((m - 1) / 3)
        }
        Unit::Month => {
            let (y, m, _) = civil(l.day);
            y * 12 + i64::from(m)
        }
        // Weeks start on Monday.
        Unit::Week => (l.day + 8 - i64::from(weekday(l.day))).div_euclid(7),
        Unit::Day => l.day,
        Unit::Hour => {
            let f = epoch_facts(tz);
            if secs >= 0 && f.whole_hours {
                secs / 3600
            } else {
                (secs + 86_400 - f.offset_at_epoch).div_euclid(3600) - 24
            }
        }
        Unit::Minute => secs.div_euclid(60),
        Unit::Second => secs,
        Unit::Millisecond => sub(3),
        Unit::Microsecond => sub(6),
        Unit::Nanosecond => sub(9),
    }
}

fn diff_eval(call: &Call) -> Result<ArrayRef, ChError> {
    let (unit, first) = if call.name == "timeDiff" {
        (Unit::Second, 0)
    } else {
        (
            diff_unit(call.name, &call.const_str(0)?.unwrap_or_default())?,
            1,
        )
    };
    let explicit = call.const_str(first + 2)?;
    let zone_of = |i: usize| -> Result<Tz, ChError> {
        match &explicit {
            Some(z) => super::zone::parse_zone(z),
            None => call.zone(usize::MAX, temporal(&call.types[i]).and_then(|(_, z)| z)),
        }
    };
    let (tz_a, tz_b) = (zone_of(first)?, zone_of(first + 1)?);
    let a = locals(call, first, tz_a)?;
    let b = locals(call, first + 1, tz_b)?;
    let out = call.map(|row| {
        Ok(match (&a[row], &b[row]) {
            (Some(x), Some(y)) => Some(Out::Int(
                relative(unit, tz_b, y).wrapping_sub(relative(unit, tz_a, x)),
            )),
            _ => None,
        })
    })?;
    write(&call.ret, out)
}

// The planner -----------------------------------------------------------------

/// Plans `plus` and `minus` where a date, a time or an interval is an
/// operand, as ClickHouse resolves them: `t ± INTERVAL n UNIT` is
/// `addUnits(t, n)` / `subtractUnits(t, n)`, `t ± n` moves a date by days
/// and a time by seconds, and `t1 - t2` is [`DATE_MINUS`]. Any other
/// operator on an interval is `43`; an interval plus an interval (a tuple of
/// intervals in ClickHouse) is `48` (R30.3).
#[derive(Debug)]
pub struct DatePlanner {
    adds: std::collections::HashMap<String, Arc<ScalarUDF>>,
    minus: Arc<ScalarUDF>,
}

impl DatePlanner {
    fn call(&self, name: &str, args: Vec<Expr>) -> Result<Expr> {
        let f =
            self.adds.get(name).cloned().ok_or_else(|| {
                external(ChError::logical_error(format!("{name} is not registered")))
            })?;
        Ok(Expr::ScalarFunction(ScalarFunction::new_udf(f, args)))
    }
}

fn ch_type(e: &Expr, schema: &DFSchema) -> Result<ChType> {
    let (_, field) = e.to_field(schema)?;
    Ok(from_arrow(&field).map_err(external)?.base().clone())
}

impl ExprPlanner for DatePlanner {
    fn plan_binary_op(
        &self,
        expr: RawBinaryExpr,
        schema: &DFSchema,
    ) -> Result<PlannerResult<RawBinaryExpr>> {
        let RawBinaryExpr { op, left, right } = expr;
        let (lt, rt) = (ch_type(&left, schema)?, ch_type(&right, schema)?);
        let (l_time, r_time) = (temporal(&lt).is_some(), temporal(&rt).is_some());
        let (l_iv, r_iv) = (interval_unit(&lt), interval_unit(&rt));
        if !(l_time || r_time || l_iv.is_some() || r_iv.is_some()) {
            return Ok(PlannerResult::Original(RawBinaryExpr { op, left, right }));
        }
        let unit_name = |u: Unit| format!("{u:?}s");
        let is_minus = op == BinaryOperator::Minus;
        let planned = match (&op, l_time, r_time, l_iv, r_iv) {
            (BinaryOperator::Plus | BinaryOperator::Minus, true, false, None, Some(u)) => {
                let verb = if is_minus { "subtract" } else { "add" };
                self.call(&format!("{verb}{}", unit_name(u)), vec![left, right])?
            }
            (BinaryOperator::Plus, false, true, Some(u), None) => {
                self.call(&format!("add{}", unit_name(u)), vec![right, left])?
            }
            (BinaryOperator::Plus | BinaryOperator::Minus, true, false, None, None)
                if is_number(&rt) =>
            {
                let verb = if is_minus { "subtract" } else { "add" };
                let unit = if matches!(temporal(&lt), Some((Kind::Date | Kind::Date32, _))) {
                    "Days"
                } else {
                    "Seconds"
                };
                self.call(&format!("{verb}{unit}"), vec![left, right])?
            }
            (BinaryOperator::Plus, false, true, None, None) if is_number(&lt) => {
                let unit = if matches!(temporal(&rt), Some((Kind::Date | Kind::Date32, _))) {
                    "Days"
                } else {
                    "Seconds"
                };
                self.call(&format!("add{unit}"), vec![right, left])?
            }
            (BinaryOperator::Minus, true, true, None, None) => Expr::ScalarFunction(
                ScalarFunction::new_udf(self.minus.clone(), vec![left, right]),
            ),
            (BinaryOperator::Plus, _, _, Some(_), Some(_)) => {
                return Err(external(ChError::unsupported(
                    "The sum of two intervals (a tuple of intervals)",
                )));
            }
            (BinaryOperator::Plus | BinaryOperator::Minus, ..)
            | (
                BinaryOperator::Multiply
                | BinaryOperator::Divide
                | BinaryOperator::Modulo
                | BinaryOperator::DuckIntegerDivide
                | BinaryOperator::MyIntegerDivide,
                ..,
            ) => {
                let name = match op {
                    BinaryOperator::Plus => "plus",
                    BinaryOperator::Minus => "minus",
                    BinaryOperator::Multiply => "multiply",
                    BinaryOperator::Divide => "divide",
                    _ => "modulo",
                };
                return Err(external(ChError::illegal_type_of_argument(format!(
                    "Illegal types {lt} and {rt} of arguments of function {name}"
                ))));
            }
            _ if l_iv.is_some() || r_iv.is_some() => {
                return Err(external(ChError::unsupported(format!(
                    "The operator {op} on an interval"
                ))));
            }
            _ => return Ok(PlannerResult::Original(RawBinaryExpr { op, left, right })),
        };
        Ok(PlannerResult::Planned(planned))
    }
}

/// The interval functions, and the planner over the `add*`/`subtract*`
/// functions among them.
pub fn functions() -> (Vec<ChFunction>, ChFunction) {
    let mut v = Vec::new();
    for u in UNITS {
        v.push(ChFunction::new(
            &format!("toInterval{u}"),
            to_interval_type,
            to_interval_eval,
        ));
        v.push(ChFunction::new(&format!("add{u}s"), add_type, add_eval));
        v.push(ChFunction::new(
            &format!("subtract{u}s"),
            add_type,
            add_eval,
        ));
    }
    v.push(ChFunction::new("dateDiff", diff_type, diff_eval));
    v.push(ChFunction::new("timeDiff", diff_type, diff_eval));
    (v, ChFunction::new(DATE_MINUS, minus_type, minus_eval))
}

/// The planner, over the registered `add*`/`subtract*` functions.
pub fn planner(scalars: &std::collections::HashMap<String, Arc<ScalarUDF>>) -> Option<DatePlanner> {
    let adds = scalars
        .iter()
        .filter(|(k, _)| k.starts_with("add") || k.starts_with("subtract"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    Some(DatePlanner {
        adds,
        minus: scalars.get(DATE_MINUS)?.clone(),
    })
}
