//! Time zones and the civil calendar (plan HE1 Task 30).
//!
//! A `DateTime` is seconds since the epoch; its zone only decides how it
//! reads as a calendar date and a time of day. The zone is the function's
//! time-zone argument, else the column type's, else the session's
//! ([`session_zone`]: `session_timezone` once Task 37 sets it, else UTC,
//! the oracle's server zone). Zones come from `chrono-tz` (R30.2).
//!
//! Days are counted from 1970-01-01 (day 0), and the proleptic Gregorian
//! calendar is used throughout, as ClickHouse's `DateLUT` does.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Mutex, OnceLock};

use chrono::{NaiveDate, NaiveDateTime, Offset, TimeZone};
use chrono_tz::Tz;
use datafusion::common::config::ConfigOptions;
use loams_house_types::ChError;

/// Seconds in a day.
pub const DAY: i64 = 86_400;

/// The zone named `name`.
///
/// # Errors
///
/// `36 BAD_ARGUMENTS` for a name the time-zone database does not have.
pub fn parse_zone(name: &str) -> Result<Tz, ChError> {
    Tz::from_str(name).map_err(|_| ChError::bad_arguments(format!("Cannot load time zone {name}")))
}

/// The session's zone: DataFusion's `datafusion.execution.time_zone` when it
/// names a zone (Task 37 sets it from `session_timezone`), else UTC, the
/// engine's default and the oracle's server zone (R30.2).
pub fn session_zone(config: &ConfigOptions) -> Tz {
    config
        .execution
        .time_zone
        .as_deref()
        .and_then(|z| Tz::from_str(z).ok())
        .unwrap_or(Tz::UTC)
}

/// The zone's offset from UTC, in seconds east, at `secs`.
pub fn offset(tz: Tz, secs: i64) -> i64 {
    if tz == Tz::UTC {
        return 0;
    }
    match chrono::DateTime::from_timestamp(secs, 0) {
        Some(dt) => i64::from(
            tz.offset_from_utc_datetime(&dt.naive_utc())
                .fix()
                .local_minus_utc(),
        ),
        None => 0,
    }
}

/// The local day and the second of that day of `secs` in `tz`.
pub fn local(tz: Tz, secs: i64) -> (i64, i64) {
    let l = secs.saturating_add(offset(tz, secs));
    (l.div_euclid(DAY), l.rem_euclid(DAY))
}

/// The instant of the local wall-clock time `day` + `sod` seconds in `tz`.
/// An ambiguous time (a fall-back hour) is its earlier instant; a time in a
/// gap (a spring-forward hour) is read with the offset before the gap.
pub fn to_utc(tz: Tz, day: i64, sod: i64) -> i64 {
    let local = day.saturating_mul(DAY).saturating_add(sod);
    if tz == Tz::UTC {
        return local;
    }
    let naive = NaiveDate::from_num_days_from_ce_opt(
        i32::try_from(day.saturating_add(719_163)).unwrap_or(i32::MAX),
    )
    .and_then(|d| d.and_hms_opt(0, 0, 0))
    .map(|d| d + chrono::Duration::seconds(sod));
    let Some(naive): Option<NaiveDateTime> = naive else {
        return local - offset(tz, local);
    };
    match tz.from_local_datetime(&naive) {
        chrono::LocalResult::Single(dt) => dt.timestamp(),
        chrono::LocalResult::Ambiguous(a, b) => a.timestamp().min(b.timestamp()),
        chrono::LocalResult::None => local - offset(tz, local - offset(tz, local) - DAY / 2),
    }
}

/// The instant the local day `day` starts in `tz`.
pub fn day_start(tz: Tz, day: i64) -> i64 {
    to_utc(tz, day, 0)
}

/// What ClickHouse's `DateLUT` knows of a zone over the epoch (1970–2106):
/// whether its offset was always a whole number of hours, and of minutes,
/// and its offset at the start of the epoch.
#[derive(Debug, Clone, Copy)]
pub struct EpochFacts {
    /// Every offset in the epoch is a whole number of hours.
    pub whole_hours: bool,
    /// Every offset in the epoch is a whole number of minutes.
    pub whole_minutes: bool,
    /// The offset at 1970-01-01 00:00:00 UTC.
    pub offset_at_epoch: i64,
}

/// [`EpochFacts`] of `tz`, measured once per zone.
pub fn epoch_facts(tz: Tz) -> EpochFacts {
    static FACTS: OnceLock<Mutex<HashMap<Tz, EpochFacts>>> = OnceLock::new();
    let cache = FACTS.get_or_init(Default::default);
    if let Some(f) = cache.lock().ok().and_then(|c| c.get(&tz).copied()) {
        return f;
    }
    let mut facts = EpochFacts {
        whole_hours: true,
        whole_minutes: true,
        offset_at_epoch: offset(tz, 0),
    };
    if tz != Tz::UTC {
        // Daily samples catch every offset a zone held for a day or more.
        let mut t = 0i64;
        while t < 4_294_967_296 {
            let o = offset(tz, t);
            facts.whole_hours &= o % 3600 == 0;
            facts.whole_minutes &= o % 60 == 0;
            t += DAY;
        }
    }
    if let Ok(mut c) = cache.lock() {
        c.insert(tz, facts);
    }
    facts
}

/// The civil date (year, month 1–12, day 1–31) of day number `z`
/// (Howard Hinnant's `civil_from_days`).
pub fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The day number of a civil date (Howard Hinnant's `days_from_civil`).
pub fn days(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Whether `y` is a leap year.
pub fn leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Days in month `m` of year `y`.
pub fn month_days(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if leap(y) => 29,
        _ => 28,
    }
}

/// The ISO day of the week of day `z`: 1 for Monday to 7 for Sunday.
pub fn weekday(z: i64) -> u32 {
    ((z + 3).rem_euclid(7) + 1) as u32
}

/// Day `z` moved by `months` calendar months, the day of the month clamped
/// to the new month's length (`2024-01-31` + 1 month is `2024-02-29`).
pub fn add_months(z: i64, months: i64) -> i64 {
    let (y, m, d) = civil(z);
    let total = y * 12 + i64::from(m) - 1 + months;
    let (ny, nm) = (total.div_euclid(12), (total.rem_euclid(12) + 1) as u32);
    days(ny, nm, d.min(month_days(ny, nm)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_round_trips() {
        for z in [-719_468, -25_567, -1, 0, 1, 19_787, 65_535, 120_529] {
            let (y, m, d) = civil(z);
            assert_eq!(days(y, m, d), z);
        }
        assert_eq!(civil(19_787), (2024, 3, 5));
        assert_eq!(weekday(0), 4, "1970-01-01 was a Thursday");
        assert_eq!(civil(add_months(days(2024, 1, 31), 1)), (2024, 2, 29));
    }

    #[test]
    fn zones_read_local_time() {
        let ny = parse_zone("America/New_York").unwrap();
        // 2024-03-10 is the spring-forward day: 02:30 does not exist.
        let d = days(2024, 3, 10);
        assert_eq!(to_utc(ny, d, 0), 1_710_046_800);
        assert_eq!(local(ny, 1_710_046_800), (d, 0));
        assert!(parse_zone("Foo/Bar").is_err());
        let kolkata = parse_zone("Asia/Kolkata").unwrap();
        assert!(!epoch_facts(kolkata).whole_hours);
        assert!(epoch_facts(kolkata).whole_minutes);
        assert!(epoch_facts(ny).whole_hours);
    }
}
