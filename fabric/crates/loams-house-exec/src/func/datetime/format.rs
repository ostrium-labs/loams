//! `formatDateTime(t, format[, zone])` (alias `DATE_FORMAT`): MySQL-style
//! format specifiers, ported from ClickHouse's
//! `src/Functions/formatDateTime.cpp` (Apache-2.0) with 26.9's default
//! settings: `%M` is the month's name, `%f` prints six digits, `%e` is not
//! space-padded, `%c`, `%k` and `%l` keep their leading zeros.

use std::fmt::Write as _;

use datafusion::arrow::array::ArrayRef;
use loams_house_types::{ChError, ChType};

use super::calendar::locals;
use super::udf::{Call, ChFunction, Sig, illegal};
use super::value::{Out, temporal, write};
use super::zone::{civil, days, offset, weekday};

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const WEEKDAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

fn check(format: &str) -> Result<(), ChError> {
    let mut chars = format.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            continue;
        }
        match chars.next() {
            None => {
                return Err(ChError::bad_arguments(
                    "'%' must not be the last character in the format string, use '%%' instead",
                ));
            }
            Some(
                'a' | 'b' | 'c' | 'C' | 'd' | 'D' | 'e' | 'M' | 'f' | 'F' | 'g' | 'G' | 'j' | 'm'
                | 'u' | 'V' | 'w' | 'W' | 'y' | 'Y' | 'Q' | 'z' | 'p' | 'r' | 'R' | 's' | 'S' | 'T'
                | 'h' | 'H' | 'i' | 'I' | 'k' | 'l' | 't' | 'n' | '%',
            ) => {}
            Some(other @ ('U' | 'v' | 'x' | 'X')) => {
                return Err(ChError::not_implemented(format!(
                    "The format %{other} is not supported"
                )));
            }
            Some(other) => {
                return Err(ChError::bad_arguments(format!(
                    "Incorrect syntax '%{other}', symbol is not supported in function formatDateTime"
                )));
            }
        }
    }
    Ok(())
}

fn format_type(sig: &Sig) -> Result<ChType, ChError> {
    sig.arity(2, 3)?;
    if temporal(&sig.types[0]).is_none() {
        return Err(illegal(sig.name, &sig.types[0], 0));
    }
    check(&sig.const_str(1)?.unwrap_or_default())?;
    sig.zone_arg(2)?;
    Ok(ChType::String)
}

/// ISO 8601 week-numbering year and week of day `z`.
fn iso_week(z: i64) -> (i64, i64) {
    let wd = i64::from(weekday(z));
    let thursday = z - wd + 4;
    let year = civil(thursday).0;
    let week = (thursday - days(year, 1, 1)) / 7 + 1;
    (year, week)
}

fn format_eval(call: &Call) -> Result<ArrayRef, ChError> {
    let format = call.const_str(1)?.unwrap_or_default();
    let tz = call.zone(2, temporal(&call.types[0]).and_then(|(_, z)| z))?;
    let ls = locals(call, 0, tz)?;
    let out = call.map(|row| {
        let Some(l) = ls[row] else { return Ok(None) };
        let (y, m, d) = civil(l.day);
        let (h, mi, s) = (l.sod / 3600, l.sod / 60 % 60, l.sod % 60);
        let h12 = if h % 12 == 0 { 12 } else { h % 12 };
        let wd = weekday(l.day) as usize;
        let mut o = String::with_capacity(format.len() + 16);
        let mut chars = format.chars();
        while let Some(c) = chars.next() {
            if c != '%' {
                o.push(c);
                continue;
            }
            let Some(spec) = chars.next() else { break };
            let _ = match spec {
                'a' => write!(o, "{}", &WEEKDAYS[wd - 1][..3]),
                'b' => write!(o, "{}", &MONTHS[m as usize - 1][..3]),
                'c' | 'm' => write!(o, "{m:02}"),
                'C' => write!(o, "{:02}", y / 100),
                'd' => write!(o, "{d:02}"),
                'D' => write!(o, "{m:02}/{d:02}/{:02}", y % 100),
                'e' => write!(o, "{d}"),
                'M' => write!(o, "{}", MONTHS[m as usize - 1]),
                'f' => write!(o, "{:06}", l.nanos / 1000),
                'F' => write!(o, "{y:04}-{m:02}-{d:02}"),
                'g' => write!(o, "{:02}", iso_week(l.day).0 % 100),
                'G' => write!(o, "{:04}", iso_week(l.day).0),
                'j' => write!(o, "{:03}", l.day - days(y, 1, 1) + 1),
                'u' => write!(o, "{wd}"),
                'V' => write!(o, "{:02}", iso_week(l.day).1),
                'w' => write!(o, "{}", wd % 7),
                'W' => write!(o, "{}", WEEKDAYS[wd - 1]),
                'y' => write!(o, "{:02}", y % 100),
                'Y' => write!(o, "{y:04}"),
                'Q' => write!(o, "{}", (m - 1) / 3 + 1),
                'z' => {
                    let off = l.secs.map(|t| offset(tz, t)).unwrap_or(0);
                    let sign = if off < 0 { '-' } else { '+' };
                    let a = off.abs();
                    write!(o, "{sign}{:02}{:02}", a / 3600, a / 60 % 60)
                }
                'p' => write!(o, "{}", if h < 12 { "AM" } else { "PM" }),
                'r' => write!(o, "{h12:02}:{mi:02} {}", if h < 12 { "AM" } else { "PM" }),
                'R' => write!(o, "{h:02}:{mi:02}"),
                's' | 'S' => write!(o, "{s:02}"),
                'T' => write!(o, "{h:02}:{mi:02}:{s:02}"),
                'h' | 'I' | 'l' => write!(o, "{h12:02}"),
                'H' | 'k' => write!(o, "{h:02}"),
                'i' => write!(o, "{mi:02}"),
                't' => write!(o, "\t"),
                'n' => writeln!(o),
                other => write!(o, "{other}"),
            };
        }
        Ok(Some(Out::Str(o)))
    })?;
    write(&call.ret, out)
}

/// The formatting functions.
pub fn functions() -> Vec<ChFunction> {
    vec![ChFunction::new("formatDateTime", format_type, format_eval)]
}
