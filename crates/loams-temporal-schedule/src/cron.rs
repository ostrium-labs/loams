//! `ScheduleSpec.cron_string`: traditional cron strings, as the proto
//! documents them (D2073).
//!
//! - 5 fields: minute, hour, day of month, month, day of week;
//!   6 fields: the same and the year; 7 fields: the second first, then the
//!   six. A missing year is `*`, a missing second `0`.
//! - `@yearly` (`@annually`), `@monthly`, `@weekly`, `@daily` (`@midnight`)
//!   and `@hourly` instead of the fields.
//! - `@every <interval>[/<phase>]` becomes an `IntervalSpec`; each is a
//!   decimal integer with a unit `s`, `m`, `h` or `d` (several may be
//!   concatenated, `1h30m`).
//! - An optional `CRON_TZ=<zone>` or `TZ=<zone>` prefix sets the time zone,
//!   and an optional `#` starts a comment.

use loams_temporal_proto::temporal::api::schedule::v1::{CalendarSpec, IntervalSpec};

use crate::SpecError;

/// What one cron string compiles to.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Cron {
    pub calendar: Option<CalendarSpec>,
    pub interval: Option<IntervalSpec>,
    /// From a `CRON_TZ=` or `TZ=` prefix.
    pub timezone: Option<String>,
}

/// Parses `<int><unit>...` with units s, m, h, d into milliseconds.
pub fn parse_duration_ms(s: &str) -> Result<i64, SpecError> {
    let bad = || SpecError::Invalid(format!("{s:?} is not a duration like 30s, 5m, 1h or 2d"));
    let s = s.trim();
    if s.is_empty() {
        return Err(bad());
    }
    let mut total: i64 = 0;
    let mut num = String::new();
    for ch in s.chars() {
        if ch.is_ascii_digit() {
            num.push(ch);
            continue;
        }
        let unit: i64 = match ch {
            's' => 1_000,
            'm' => 60_000,
            'h' => 3_600_000,
            'd' => 86_400_000,
            _ => return Err(bad()),
        };
        if num.is_empty() {
            return Err(bad());
        }
        let n: i64 = num.parse().map_err(|_| bad())?;
        total = n
            .checked_mul(unit)
            .and_then(|v| total.checked_add(v))
            .ok_or_else(bad)?;
        num.clear();
    }
    if !num.is_empty() {
        return Err(bad());
    }
    Ok(total)
}

fn duration(ms: i64) -> prost_types::Duration {
    prost_types::Duration {
        seconds: ms.div_euclid(1000),
        nanos: (ms.rem_euclid(1000) * 1_000_000) as i32,
    }
}

/// Parses one cron string.
pub fn parse(input: &str) -> Result<Cron, SpecError> {
    let mut out = Cron::default();
    let (body, comment) = match input.split_once('#') {
        Some((b, c)) => (b.trim(), c.trim().to_string()),
        None => (input.trim(), String::new()),
    };
    let mut body = body;
    for prefix in ["CRON_TZ=", "TZ="] {
        if let Some(rest) = body.strip_prefix(prefix) {
            let (tz, rest) = rest.split_once(char::is_whitespace).ok_or_else(|| {
                SpecError::Invalid(format!("{input:?}: nothing after the time zone"))
            })?;
            out.timezone = Some(tz.to_string());
            body = rest.trim();
            break;
        }
    }
    if let Some(rest) = body.strip_prefix("@every") {
        let rest = rest.trim();
        let (iv, phase) = match rest.split_once('/') {
            Some((i, p)) => (i, Some(p)),
            None => (rest, None),
        };
        let iv = parse_duration_ms(iv)?;
        if iv <= 0 {
            return Err(SpecError::Invalid(format!(
                "{input:?}: the interval must be positive"
            )));
        }
        let phase = phase.map(parse_duration_ms).transpose()?.unwrap_or(0);
        out.interval = Some(IntervalSpec {
            interval: Some(duration(iv)),
            phase: (phase != 0).then(|| duration(phase)),
        });
        return Ok(out);
    }
    let fields: Vec<&str> = match body {
        "@yearly" | "@annually" => vec!["0", "0", "1", "1", "*"],
        "@monthly" => vec!["0", "0", "1", "*", "*"],
        "@weekly" => vec!["0", "0", "*", "*", "0"],
        "@daily" | "@midnight" => vec!["0", "0", "*", "*", "*"],
        "@hourly" => vec!["0", "*", "*", "*", "*"],
        b if b.starts_with('@') => {
            return Err(SpecError::Invalid(format!(
                "{input:?}: unknown shorthand {b}"
            )));
        }
        b => b.split_whitespace().collect(),
    };
    let (second, rest): (&str, &[&str]) = match fields.len() {
        5 | 6 => ("0", &fields[..]),
        7 => (fields[0], &fields[1..]),
        n => {
            return Err(SpecError::Invalid(format!(
                "{input:?}: a cron string has 5, 6 or 7 fields, not {n}"
            )));
        }
    };
    out.calendar = Some(CalendarSpec {
        second: second.to_string(),
        minute: rest[0].to_string(),
        hour: rest[1].to_string(),
        day_of_month: rest[2].to_string(),
        month: rest[3].to_string(),
        day_of_week: rest[4].to_string(),
        year: rest
            .get(5)
            .map_or_else(|| "*".to_string(), |y| (*y).to_string()),
        comment,
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn five_six_and_seven_fields() {
        let c = parse("*/5 9-17 * * mon-fri").unwrap().calendar.unwrap();
        assert_eq!(
            (
                c.second.as_str(),
                c.minute.as_str(),
                c.hour.as_str(),
                c.day_of_week.as_str(),
                c.year.as_str()
            ),
            ("0", "*/5", "9-17", "mon-fri", "*")
        );
        let c = parse("0 12 1 * * 2027").unwrap().calendar.unwrap();
        assert_eq!(c.year, "2027");
        let c = parse("30 0 12 1 * * 2027").unwrap().calendar.unwrap();
        assert_eq!(
            (c.second.as_str(), c.minute.as_str(), c.hour.as_str()),
            ("30", "0", "12")
        );
        assert!(parse("* * * *").is_err());
        assert!(parse("* * * * * * * *").is_err());
    }

    #[test]
    fn shorthands_comments_and_zones() {
        let c = parse("CRON_TZ=America/New_York @daily # nightly").unwrap();
        assert_eq!(c.timezone.as_deref(), Some("America/New_York"));
        let cal = c.calendar.unwrap();
        assert_eq!((cal.minute.as_str(), cal.hour.as_str()), ("0", "0"));
        assert_eq!(cal.comment, "nightly");
        let c = parse("TZ=UTC 0 * * * *").unwrap();
        assert_eq!(c.timezone.as_deref(), Some("UTC"));
        assert_eq!(parse("@weekly").unwrap().calendar.unwrap().day_of_week, "0");
        assert!(parse("@fortnightly").is_err());
        assert!(parse("TZ=UTC").is_err());
    }

    #[test]
    fn every_is_an_interval() {
        let c = parse("@every 90m/15m").unwrap();
        let iv = c.interval.unwrap();
        assert_eq!(iv.interval.unwrap().seconds, 5400);
        assert_eq!(iv.phase.unwrap().seconds, 900);
        assert!(c.calendar.is_none());
        let iv = parse("@every 1h30m").unwrap().interval.unwrap();
        assert_eq!(iv.interval.unwrap().seconds, 5400);
        assert!(iv.phase.is_none());
        assert!(parse("@every 0s").is_err());
        assert!(parse("@every 5x").is_err());
        assert!(parse("@every h").is_err());
        assert!(parse("@every 5").is_err());
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration_ms("2d").unwrap(), 172_800_000);
        assert_eq!(parse_duration_ms("1h1m1s").unwrap(), 3_661_000);
        assert!(parse_duration_ms("").is_err());
        assert!(parse_duration_ms("99999999999999999d").is_err());
    }
}
