//! Calendar specs: Temporal's `CalendarSpec` expressions and
//! `StructuredCalendarSpec` ranges, compiled to sets of clock-time fields and
//! searched for the next matching local time (D2073).
//!
//! The semantics are the ones `temporal/api/schedule/v1/message.proto`
//! documents:
//!
//! - An empty `second`, `minute` or `hour` means `0`; every other empty field
//!   means `*`.
//! - An expression is a comma-separated list of items. An item is `*`, a
//!   value, `start-end`, or either of those followed by `/step`; `start/step`
//!   runs to the field's maximum. Months may be named (`jan`, `january`) and
//!   days of the week too (`sun`, `sunday`); `7` is Sunday as well as `0`.
//! - A structured `Range` with `end < start` means just `start`; a zero step
//!   means 1.
//! - Day of month and day of week are both required to match (no cron "or").
//! - Years run from [`MIN_YEAR`] to [`MAX_YEAR`].

use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};
use loams_temporal_proto::temporal::api::schedule::v1::{
    CalendarSpec, Range as PbRange, StructuredCalendarSpec,
};

use crate::SpecError;

/// The first year a calendar spec matches.
pub const MIN_YEAR: i32 = 2000;
/// The last year a calendar spec matches (Temporal's `maxCalendarYear`).
pub const MAX_YEAR: i32 = 2100;

/// One field of a calendar: which values match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSet {
    /// Inclusive `(start, end, step)` with `start <= end` and `step >= 1`.
    ranges: Vec<(i32, i32, i32)>,
}

impl FieldSet {
    fn from_ranges(
        rs: &[PbRange],
        default: (i32, i32),
        min: i32,
        max: i32,
        what: &str,
    ) -> Result<Self, SpecError> {
        if rs.is_empty() {
            return Ok(FieldSet {
                ranges: vec![(default.0, default.1, 1)],
            });
        }
        let mut ranges = Vec::with_capacity(rs.len());
        for r in rs {
            let start = r.start;
            let end = if r.end < r.start { r.start } else { r.end };
            let step = if r.step <= 0 { 1 } else { r.step };
            if start < min || end > max {
                return Err(SpecError::Invalid(format!(
                    "{what} range {start}-{end} is outside {min}-{max}"
                )));
            }
            ranges.push((start, end, step));
        }
        Ok(FieldSet { ranges })
    }

    /// Whether `v` matches.
    pub fn contains(&self, v: i32) -> bool {
        self.ranges
            .iter()
            .any(|&(s, e, step)| (s..=e).contains(&v) && (v - s) % step == 0)
    }

    /// The smallest matching value `>= v`, if any.
    pub fn next_ge(&self, v: i32) -> Option<i32> {
        let mut best: Option<i32> = None;
        for &(s, e, step) in &self.ranges {
            let c = if v <= s {
                s
            } else {
                let d = v - s;
                let k = d / step + i32::from(d % step != 0);
                s + k * step
            };
            if c <= e {
                best = Some(best.map_or(c, |b| b.min(c)));
            }
        }
        best
    }

    /// The ranges, as structured `Range`s (for the canonical spec).
    pub fn to_ranges(&self) -> Vec<PbRange> {
        self.ranges
            .iter()
            .map(|&(start, end, step)| PbRange {
                start,
                end: if end == start { 0 } else { end },
                step: if step == 1 { 0 } else { step },
            })
            .collect()
    }
}

/// A compiled calendar: one set per clock-time field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Calendar {
    pub second: FieldSet,
    pub minute: FieldSet,
    pub hour: FieldSet,
    pub day_of_month: FieldSet,
    pub month: FieldSet,
    pub year: FieldSet,
    /// 0 = Sunday.
    pub day_of_week: FieldSet,
    pub comment: String,
}

const MONTHS: [&str; 12] = [
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];
const DAYS: [&str; 7] = [
    "sunday",
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
];

/// The parse rules of one field.
#[derive(Debug, Clone, Copy)]
struct Field {
    name: &'static str,
    min: i32,
    max: i32,
    /// The default when the expression is empty.
    default: &'static str,
    names: Option<(&'static [&'static str], i32)>,
}

const SECOND: Field = Field {
    name: "second",
    min: 0,
    max: 59,
    default: "0",
    names: None,
};
const MINUTE: Field = Field {
    name: "minute",
    min: 0,
    max: 59,
    default: "0",
    names: None,
};
const HOUR: Field = Field {
    name: "hour",
    min: 0,
    max: 23,
    default: "0",
    names: None,
};
const DOM: Field = Field {
    name: "day_of_month",
    min: 1,
    max: 31,
    default: "*",
    names: None,
};
const MONTH: Field = Field {
    name: "month",
    min: 1,
    max: 12,
    default: "*",
    names: Some((&MONTHS, 1)),
};
const YEAR: Field = Field {
    name: "year",
    min: MIN_YEAR,
    max: MAX_YEAR,
    default: "*",
    names: None,
};
// Day of week accepts 7 for Sunday; it is folded to 0 after parsing.
const DOW: Field = Field {
    name: "day_of_week",
    min: 0,
    max: 7,
    default: "*",
    names: Some((&DAYS, 0)),
};

fn parse_value(f: Field, s: &str) -> Result<i32, SpecError> {
    let t = s.trim();
    if let Ok(v) = t.parse::<i32>() {
        if !(f.min..=f.max).contains(&v) {
            return Err(SpecError::Invalid(format!(
                "{} value {v} is outside {}-{}",
                f.name, f.min, f.max
            )));
        }
        return Ok(v);
    }
    if let Some((names, base)) = f.names {
        let lower = t.to_ascii_lowercase();
        if lower.len() >= 3 {
            for (i, n) in names.iter().enumerate() {
                if *n == lower || (lower.len() == 3 && n.starts_with(&lower)) {
                    return Ok(base + i as i32);
                }
            }
        }
    }
    Err(SpecError::Invalid(format!(
        "{}: {s:?} is not a number{}",
        f.name,
        if f.names.is_some() { " or a name" } else { "" }
    )))
}

/// Parses one field's expression to ranges.
fn parse_expr(f: Field, expr: &str) -> Result<Vec<PbRange>, SpecError> {
    let expr = if expr.trim().is_empty() {
        f.default
    } else {
        expr.trim()
    };
    let mut out = Vec::new();
    for item in expr.split(',') {
        let item = item.trim();
        if item.is_empty() {
            return Err(SpecError::Invalid(format!(
                "{}: empty item in {expr:?}",
                f.name
            )));
        }
        let (body, step) = match item.split_once('/') {
            Some((b, st)) => {
                let st: i32 = st
                    .trim()
                    .parse()
                    .map_err(|_| SpecError::Invalid(format!("{}: bad step in {item:?}", f.name)))?;
                if st < 1 {
                    return Err(SpecError::Invalid(format!(
                        "{}: step must be positive in {item:?}",
                        f.name
                    )));
                }
                (b.trim(), Some(st))
            }
            None => (item, None),
        };
        let (start, end) = if body == "*" {
            (f.min, f.max)
        } else if let Some((a, b)) = body.split_once('-') {
            let a = parse_value(f, a)?;
            let b = parse_value(f, b)?;
            if b < a {
                return Err(SpecError::Invalid(format!(
                    "{}: range {item:?} ends before it starts",
                    f.name
                )));
            }
            (a, b)
        } else {
            let a = parse_value(f, body)?;
            // `a/step` runs to the maximum.
            (a, if step.is_some() { f.max } else { a })
        };
        out.push(PbRange {
            start,
            end,
            step: step.unwrap_or(1),
        });
    }
    Ok(out)
}

/// Folds day-of-week 7 into 0 (both are Sunday).
fn fold_sunday(rs: Vec<PbRange>) -> Vec<PbRange> {
    let mut out = Vec::with_capacity(rs.len() + 1);
    for r in rs {
        let end = if r.end < r.start { r.start } else { r.end };
        let step = r.step.max(1);
        if end == 7 && (7 - r.start) % step == 0 {
            // 7 is in the range: keep the rest and add Sunday.
            if r.start <= 6 {
                out.push(PbRange {
                    start: r.start,
                    end: 6,
                    step,
                });
            }
            out.push(PbRange {
                start: 0,
                end: 0,
                step: 1,
            });
        } else {
            out.push(PbRange {
                start: r.start,
                end: end.min(6),
                step,
            });
        }
    }
    out
}

/// A `CalendarSpec` as the equivalent `StructuredCalendarSpec`.
pub fn structure(c: &CalendarSpec) -> Result<StructuredCalendarSpec, SpecError> {
    Ok(StructuredCalendarSpec {
        second: parse_expr(SECOND, &c.second)?,
        minute: parse_expr(MINUTE, &c.minute)?,
        hour: parse_expr(HOUR, &c.hour)?,
        day_of_month: parse_expr(DOM, &c.day_of_month)?,
        month: parse_expr(MONTH, &c.month)?,
        year: parse_expr(YEAR, &c.year)?,
        day_of_week: fold_sunday(parse_expr(DOW, &c.day_of_week)?),
        comment: c.comment.clone(),
    })
}

impl Calendar {
    /// Compiles a structured calendar spec.
    pub fn compile(s: &StructuredCalendarSpec) -> Result<Self, SpecError> {
        Ok(Calendar {
            second: FieldSet::from_ranges(&s.second, (0, 0), 0, 59, "second")?,
            minute: FieldSet::from_ranges(&s.minute, (0, 0), 0, 59, "minute")?,
            hour: FieldSet::from_ranges(&s.hour, (0, 0), 0, 23, "hour")?,
            day_of_month: FieldSet::from_ranges(&s.day_of_month, (1, 31), 1, 31, "day_of_month")?,
            month: FieldSet::from_ranges(&s.month, (1, 12), 1, 12, "month")?,
            year: FieldSet::from_ranges(&s.year, (MIN_YEAR, MAX_YEAR), 0, 9999, "year")?,
            day_of_week: FieldSet::from_ranges(
                &fold_sunday(s.day_of_week.clone()),
                (0, 6),
                0,
                6,
                "day_of_week",
            )?,
            comment: s.comment.clone(),
        })
    }

    /// The structured form (canonical: explicit ranges for every field).
    pub fn to_structured(&self) -> StructuredCalendarSpec {
        StructuredCalendarSpec {
            second: self.second.to_ranges(),
            minute: self.minute.to_ranges(),
            hour: self.hour.to_ranges(),
            day_of_month: self.day_of_month.to_ranges(),
            month: self.month.to_ranges(),
            year: self.year.to_ranges(),
            day_of_week: self.day_of_week.to_ranges(),
            comment: self.comment.clone(),
        }
    }

    /// Whether the local clock time `t` matches.
    pub fn matches(&self, t: &NaiveDateTime) -> bool {
        self.year.contains(t.year())
            && self.month.contains(t.month() as i32)
            && self.day_of_month.contains(t.day() as i32)
            && self
                .day_of_week
                .contains(t.weekday().num_days_from_sunday() as i32)
            && self.hour.contains(t.hour() as i32)
            && self.minute.contains(t.minute() as i32)
            && self.second.contains(t.second() as i32)
    }

    fn day_matches(&self, y: i32, m: i32, d: i32) -> bool {
        let Some(date) = NaiveDate::from_ymd_opt(y, m as u32, d as u32) else {
            return false;
        };
        self.day_of_month.contains(d)
            && self
                .day_of_week
                .contains(date.weekday().num_days_from_sunday() as i32)
    }

    /// The first local clock time `>= from` (whole seconds) that matches, or
    /// `None` past [`MAX_YEAR`] (or the year set's end).
    pub fn next_local(&self, from: NaiveDateTime) -> Option<NaiveDateTime> {
        let mut c = Cursor {
            y: from.year(),
            mo: from.month() as i32,
            d: from.day() as i32,
            h: from.hour() as i32,
            mi: from.minute() as i32,
            s: from.second() as i32,
        };
        // Every pass moves the cursor forward; the year bound ends it.
        loop {
            if c.y > MAX_YEAR {
                return None;
            }
            match self.year.next_ge(c.y) {
                None => return None,
                Some(y) if y > MAX_YEAR => return None,
                Some(y) if y != c.y => {
                    c = Cursor::year_start(y);
                }
                Some(_) => {}
            }
            match self.month.next_ge(c.mo) {
                Some(m) if m != c.mo => {
                    c.mo = m;
                    c.d = 1;
                    c.h = 0;
                    c.mi = 0;
                    c.s = 0;
                }
                Some(_) => {}
                None => {
                    c = Cursor::year_start(c.y + 1);
                    continue;
                }
            }
            let dim = days_in_month(c.y, c.mo);
            let mut day = None;
            let mut d = c.d;
            while d <= dim {
                if self.day_matches(c.y, c.mo, d) {
                    day = Some(d);
                    break;
                }
                d += 1;
            }
            match day {
                Some(d) if d != c.d => {
                    c.d = d;
                    c.h = 0;
                    c.mi = 0;
                    c.s = 0;
                }
                Some(_) => {}
                None => {
                    c.next_month();
                    continue;
                }
            }
            match self.hour.next_ge(c.h) {
                Some(h) if h <= 23 => {
                    if h != c.h {
                        c.h = h;
                        c.mi = 0;
                        c.s = 0;
                    }
                }
                _ => {
                    c.next_day();
                    continue;
                }
            }
            match self.minute.next_ge(c.mi) {
                Some(mi) if mi <= 59 => {
                    if mi != c.mi {
                        c.mi = mi;
                        c.s = 0;
                    }
                }
                _ => {
                    c.next_hour();
                    continue;
                }
            }
            match self.second.next_ge(c.s) {
                Some(s) if s <= 59 => {
                    return NaiveDate::from_ymd_opt(c.y, c.mo as u32, c.d as u32)?.and_hms_opt(
                        c.h as u32,
                        c.mi as u32,
                        s as u32,
                    );
                }
                _ => {
                    c.next_minute();
                    continue;
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Cursor {
    y: i32,
    mo: i32,
    d: i32,
    h: i32,
    mi: i32,
    s: i32,
}

impl Cursor {
    fn year_start(y: i32) -> Self {
        Cursor {
            y,
            mo: 1,
            d: 1,
            h: 0,
            mi: 0,
            s: 0,
        }
    }

    fn next_month(&mut self) {
        self.mo += 1;
        if self.mo > 12 {
            self.y += 1;
            self.mo = 1;
        }
        self.d = 1;
        self.h = 0;
        self.mi = 0;
        self.s = 0;
    }

    fn next_day(&mut self) {
        self.d += 1;
        self.h = 0;
        self.mi = 0;
        self.s = 0;
        if self.d > days_in_month(self.y, self.mo) {
            self.next_month();
        }
    }

    fn next_hour(&mut self) {
        self.h += 1;
        self.mi = 0;
        self.s = 0;
        if self.h > 23 {
            self.next_day();
        }
    }

    fn next_minute(&mut self) {
        self.mi += 1;
        self.s = 0;
        if self.mi > 59 {
            self.next_hour();
        }
    }
}

/// Days in month `m` (1-12) of year `y`.
pub fn days_in_month(y: i32, m: i32) -> i32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cal(f: impl FnOnce(&mut CalendarSpec)) -> Calendar {
        let mut c = CalendarSpec::default();
        f(&mut c);
        Calendar::compile(&structure(&c).unwrap()).unwrap()
    }

    fn dt(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap()
    }

    #[test]
    fn defaults_are_midnight_every_day() {
        let c = cal(|_| {});
        assert_eq!(
            c.next_local(dt("2026-10-27 10:00:00")),
            Some(dt("2026-10-28 00:00:00"))
        );
        assert_eq!(
            c.next_local(dt("2026-10-27 00:00:00")),
            Some(dt("2026-10-27 00:00:00"))
        );
    }

    #[test]
    fn expressions_parse() {
        let r = parse_expr(MINUTE, "*/15").unwrap();
        assert_eq!(
            r,
            vec![PbRange {
                start: 0,
                end: 59,
                step: 15
            }]
        );
        let r = parse_expr(MINUTE, "5,10-20/5,30/10").unwrap();
        assert_eq!(
            r,
            vec![
                PbRange {
                    start: 5,
                    end: 5,
                    step: 1
                },
                PbRange {
                    start: 10,
                    end: 20,
                    step: 5
                },
                PbRange {
                    start: 30,
                    end: 59,
                    step: 10
                },
            ]
        );
        assert_eq!(parse_expr(MONTH, "Jan,MARCH").unwrap()[1].start, 3);
        assert_eq!(
            parse_expr(DOW, "mon-fri").unwrap()[0],
            PbRange {
                start: 1,
                end: 5,
                step: 1
            }
        );
        assert!(parse_expr(MINUTE, "60").is_err());
        assert!(parse_expr(HOUR, "5-3").is_err());
        assert!(parse_expr(HOUR, "1,,2").is_err());
        assert!(parse_expr(HOUR, "*/0").is_err());
        assert!(parse_expr(MONTH, "ja").is_err());
    }

    #[test]
    fn sunday_is_zero_and_seven() {
        let c = cal(|c| c.day_of_week = "7".into());
        // 2026-11-01 is a Sunday.
        assert_eq!(
            c.next_local(dt("2026-10-27 00:00:01")),
            Some(dt("2026-11-01 00:00:00"))
        );
        let c = cal(|c| c.day_of_week = "5-7".into());
        assert!(
            c.day_of_week.contains(0) && c.day_of_week.contains(5) && c.day_of_week.contains(6)
        );
        assert!(!c.day_of_week.contains(1));
    }

    #[test]
    fn day_of_month_and_week_are_anded() {
        // Friday the 13th.
        let c = cal(|c| {
            c.day_of_month = "13".into();
            c.day_of_week = "fri".into();
        });
        assert_eq!(
            c.next_local(dt("2026-10-27 00:00:00")),
            Some(dt("2026-11-13 00:00:00"))
        );
        assert_eq!(
            c.next_local(dt("2026-11-13 00:00:01")),
            Some(dt("2027-08-13 00:00:00"))
        );
    }

    #[test]
    fn leap_days_and_short_months() {
        let c = cal(|c| {
            c.day_of_month = "29".into();
            c.month = "2".into();
        });
        assert_eq!(
            c.next_local(dt("2026-01-01 00:00:00")),
            Some(dt("2028-02-29 00:00:00"))
        );
        let c = cal(|c| c.day_of_month = "31".into());
        assert_eq!(
            c.next_local(dt("2026-11-01 00:00:00")),
            Some(dt("2026-12-31 00:00:00"))
        );
    }

    #[test]
    fn carries_through_every_field() {
        let c = cal(|c| {
            c.second = "30".into();
            c.minute = "*/20".into();
            c.hour = "*".into();
        });
        assert_eq!(
            c.next_local(dt("2026-12-31 23:40:31")),
            Some(dt("2027-01-01 00:00:30"))
        );
        assert_eq!(
            c.next_local(dt("2026-12-31 23:40:30")),
            Some(dt("2026-12-31 23:40:30"))
        );
        assert_eq!(
            c.next_local(dt("2026-12-31 23:20:31")),
            Some(dt("2026-12-31 23:40:30"))
        );
    }

    #[test]
    fn years_end() {
        let c = cal(|c| c.year = "2027".into());
        assert_eq!(
            c.next_local(dt("2026-05-05 05:05:05")),
            Some(dt("2027-01-01 00:00:00"))
        );
        assert_eq!(c.next_local(dt("2028-01-01 00:00:00")), None);
        let c = cal(|_| {});
        assert_eq!(c.next_local(dt("2100-12-31 00:00:01")), None);
    }

    #[test]
    fn structured_ranges_follow_the_proto_rules() {
        let s = StructuredCalendarSpec {
            hour: vec![PbRange {
                start: 9,
                end: 0,
                step: 0,
            }],
            day_of_week: vec![PbRange {
                start: 1,
                end: 5,
                step: 0,
            }],
            ..Default::default()
        };
        let c = Calendar::compile(&s).unwrap();
        // 2026-10-31 is a Saturday.
        assert_eq!(
            c.next_local(dt("2026-10-30 09:00:01")),
            Some(dt("2026-11-02 09:00:00"))
        );
        assert!(
            Calendar::compile(&StructuredCalendarSpec {
                hour: vec![PbRange {
                    start: 25,
                    end: 0,
                    step: 0
                }],
                ..Default::default()
            })
            .is_err()
        );
        // The canonical form compiles back to the same calendar.
        assert_eq!(Calendar::compile(&c.to_structured()).unwrap(), c);
    }

    #[test]
    fn matches_agrees_with_next_local() {
        let c = cal(|c| {
            c.second = "0,30".into();
            c.minute = "1-59/7".into();
            c.hour = "3,15".into();
            c.day_of_week = "tue-thu".into();
        });
        // Brute force over two days, second by second: the matches are
        // exactly the times next_local walks through.
        let start = dt("2026-10-27 00:00:00");
        let end = start + chrono::Duration::days(2);
        let mut brute = Vec::new();
        let mut t = start;
        while t < end {
            if c.matches(&t) {
                brute.push(t);
            }
            t += chrono::Duration::seconds(1);
        }
        let mut walked = Vec::new();
        let mut t = start;
        while let Some(n) = c.next_local(t) {
            if n >= end {
                break;
            }
            walked.push(n);
            t = n + chrono::Duration::seconds(1);
        }
        assert!(!brute.is_empty());
        assert_eq!(walked, brute);
    }
}
