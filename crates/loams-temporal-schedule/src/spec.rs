//! A whole `ScheduleSpec`: calendars, intervals, exclusions, the start and
//! end bounds, the time zone and the jitter (D2073).
//!
//! Times are Unix milliseconds. Calendar times are whole seconds in the
//! spec's time zone, matched on the literal clock time as the proto says:
//! a local time a DST gap skips does not fire, and one a DST overlap repeats
//! fires twice. Interval times are `phase + k * interval` from the Unix
//! epoch.
//!
//! Jitter is deterministic (a hash of the seed and the nominal time), so
//! every replica and every retry computes the same actual time, and it is
//! capped so an action never moves past the next nominal time (as Temporal's
//! `GetNextTime`).

use chrono::{DateTime, NaiveDateTime, Offset, TimeZone, Utc};
use chrono_tz::Tz;
use loams_temporal_proto::temporal::api::schedule::v1::{CalendarSpec, IntervalSpec, ScheduleSpec};
use sha2::{Digest, Sha256};

use crate::SpecError;
use crate::calendar::{self, Calendar};
use crate::cron;

/// Three hours: wider than any DST shift, the window looked around a time
/// zone transition.
const TRANSITION_WINDOW_MS: i64 = 3 * 3_600_000;

/// A compiled interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interval {
    pub every_ms: i64,
    /// In `[0, every_ms)`.
    pub phase_ms: i64,
}

impl Interval {
    fn compile(i: &IntervalSpec) -> Result<Self, SpecError> {
        let every_ms = i.interval.as_ref().map_or(0, duration_ms);
        if every_ms <= 0 {
            return Err(SpecError::Invalid("an interval must be positive".into()));
        }
        let phase_ms = i.phase.as_ref().map_or(0, duration_ms);
        if phase_ms < 0 {
            return Err(SpecError::Invalid(
                "an interval's phase must not be negative".into(),
            ));
        }
        Ok(Interval {
            every_ms,
            phase_ms: phase_ms % every_ms,
        })
    }

    /// The first time `> after`.
    pub fn next_after(&self, after: i64) -> i64 {
        let k = (after - self.phase_ms).div_euclid(self.every_ms) + 1;
        self.phase_ms + k * self.every_ms
    }
}

/// A protobuf duration in milliseconds (truncated).
pub fn duration_ms(d: &prost_types::Duration) -> i64 {
    d.seconds
        .saturating_mul(1000)
        .saturating_add(i64::from(d.nanos) / 1_000_000)
}

/// Milliseconds as a protobuf duration.
pub fn ms_duration(ms: i64) -> prost_types::Duration {
    prost_types::Duration {
        seconds: ms.div_euclid(1000),
        nanos: (ms.rem_euclid(1000) * 1_000_000) as i32,
    }
}

/// A protobuf timestamp in milliseconds (truncated).
pub fn ts_ms(t: &prost_types::Timestamp) -> i64 {
    t.seconds
        .saturating_mul(1000)
        .saturating_add(i64::from(t.nanos) / 1_000_000)
}

/// Milliseconds as a protobuf timestamp.
pub fn ms_ts(ms: i64) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: ms.div_euclid(1000),
        nanos: (ms.rem_euclid(1000) * 1_000_000) as i32,
    }
}

/// One upcoming action time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NextTime {
    /// The time the spec names.
    pub nominal_ms: i64,
    /// `nominal_ms` plus the jitter: when the action runs.
    pub actual_ms: i64,
}

/// A compiled `ScheduleSpec`.
#[derive(Debug, Clone)]
pub struct CompiledSpec {
    pub calendars: Vec<Calendar>,
    pub intervals: Vec<Interval>,
    pub excludes: Vec<Calendar>,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub jitter_ms: i64,
    pub tz: Tz,
    pub tz_name: String,
}

fn parse_tz(name: &str) -> Result<Tz, SpecError> {
    if name.is_empty() {
        return Ok(Tz::UTC);
    }
    name.parse::<Tz>()
        .map_err(|_| SpecError::Invalid(format!("unknown time zone {name:?}")))
}

// `exclude_calendar` is deprecated in the API but still accepted.
#[allow(deprecated)]
fn exclude_calendar(spec: &ScheduleSpec) -> &[CalendarSpec] {
    &spec.exclude_calendar
}

#[allow(deprecated)]
fn clear_exclude_calendar(spec: &mut ScheduleSpec) {
    spec.exclude_calendar.clear();
}

/// The canonical form of a spec, as DescribeSchedule answers it: cron
/// strings and calendar expressions become structured calendars and
/// intervals, the cron's time zone moves to `timezone_name`, and the
/// deprecated `exclude_calendar` becomes `exclude_structured_calendar`.
/// Refuses what [`CompiledSpec::compile`] would refuse.
pub fn canonicalize(spec: &ScheduleSpec) -> Result<ScheduleSpec, SpecError> {
    if !spec.timezone_data.is_empty() {
        return Err(SpecError::Invalid(
            "timezone_data (a TZif definition) is not supported by the Loams gateway; use timezone_name".into(),
        ));
    }
    let mut out = spec.clone();
    out.cron_string.clear();
    out.calendar.clear();
    clear_exclude_calendar(&mut out);
    let mut structured = spec.structured_calendar.clone();
    for c in &spec.calendar {
        structured.push(calendar::structure(c)?);
    }
    let mut tz = spec.timezone_name.clone();
    for s in &spec.cron_string {
        let c = cron::parse(s)?;
        if let Some(z) = c.timezone {
            if !tz.is_empty() && tz != z {
                return Err(SpecError::Invalid(format!(
                    "cron string {s:?} names time zone {z:?}, but the spec has {tz:?}"
                )));
            }
            tz = z;
        }
        if let Some(cal) = c.calendar {
            structured.push(calendar::structure(&cal)?);
        }
        if let Some(iv) = c.interval {
            out.interval.push(iv);
        }
    }
    // Normalise every calendar to explicit ranges.
    out.structured_calendar = structured
        .iter()
        .map(|s| Calendar::compile(s).map(|c| c.to_structured()))
        .collect::<Result<_, _>>()?;
    let mut excludes = spec.exclude_structured_calendar.clone();
    for c in exclude_calendar(spec) {
        excludes.push(calendar::structure(c)?);
    }
    out.exclude_structured_calendar = excludes
        .iter()
        .map(|s| Calendar::compile(s).map(|c| c.to_structured()))
        .collect::<Result<_, _>>()?;
    parse_tz(&tz)?;
    out.timezone_name = tz;
    for i in &out.interval {
        Interval::compile(i)?;
    }
    if let Some(j) = &spec.jitter
        && duration_ms(j) < 0
    {
        return Err(SpecError::Invalid("jitter must not be negative".into()));
    }
    if let (Some(s), Some(e)) = (&spec.start_time, &spec.end_time)
        && ts_ms(e) < ts_ms(s)
    {
        return Err(SpecError::Invalid("end_time is before start_time".into()));
    }
    Ok(out)
}

impl CompiledSpec {
    /// Compiles a spec (canonical or not).
    pub fn compile(spec: &ScheduleSpec) -> Result<Self, SpecError> {
        let c = canonicalize(spec)?;
        Ok(CompiledSpec {
            calendars: c
                .structured_calendar
                .iter()
                .map(Calendar::compile)
                .collect::<Result<_, _>>()?,
            intervals: c
                .interval
                .iter()
                .map(Interval::compile)
                .collect::<Result<_, _>>()?,
            excludes: c
                .exclude_structured_calendar
                .iter()
                .map(Calendar::compile)
                .collect::<Result<_, _>>()?,
            start_ms: c.start_time.as_ref().map(ts_ms),
            end_ms: c.end_time.as_ref().map(ts_ms),
            jitter_ms: c.jitter.as_ref().map_or(0, duration_ms).max(0),
            tz: parse_tz(&c.timezone_name)?,
            tz_name: c.timezone_name,
        })
    }

    /// Whether the spec can ever fire.
    pub fn is_empty(&self) -> bool {
        self.calendars.is_empty() && self.intervals.is_empty()
    }

    fn local(&self, ms: i64) -> Option<NaiveDateTime> {
        let utc = DateTime::<Utc>::from_timestamp_millis(ms)?;
        Some(utc.with_timezone(&self.tz).naive_local())
    }

    fn offset_secs(&self, ms: i64) -> Option<i32> {
        let utc = DateTime::<Utc>::from_timestamp_millis(ms)?;
        Some(
            self.tz
                .offset_from_utc_datetime(&utc.naive_utc())
                .fix()
                .local_minus_utc(),
        )
    }

    /// Whether the time zone's offset is the same three hours either side.
    fn quiet(&self, ms: i64) -> bool {
        self.offset_secs(ms - TRANSITION_WINDOW_MS) == self.offset_secs(ms + TRANSITION_WINDOW_MS)
    }

    /// Every instant a local clock time stands for: none in a gap, two in
    /// an overlap.
    fn instants(&self, local: &NaiveDateTime) -> Vec<i64> {
        match self.tz.from_local_datetime(local) {
            chrono::LocalResult::Single(t) => vec![t.timestamp_millis()],
            chrono::LocalResult::Ambiguous(a, b) => {
                let (a, b) = (a.timestamp_millis(), b.timestamp_millis());
                vec![a.min(b), a.max(b)]
            }
            chrono::LocalResult::None => Vec::new(),
        }
    }

    /// Scans local times from `from` and answers the earliest instant
    /// `>= min_ms`. With `window`, keeps looking `window` of local time past
    /// the first hit (an overlap can put a later clock time earlier).
    fn scan(&self, cal: &Calendar, from: NaiveDateTime, min_ms: i64, window: i64) -> Option<i64> {
        let mut best: Option<i64> = None;
        let mut limit: Option<NaiveDateTime> = None;
        let mut c = from;
        loop {
            let m = cal.next_local(c)?;
            if let Some(l) = limit
                && m > l
            {
                break;
            }
            for i in self.instants(&m) {
                if i >= min_ms {
                    best = Some(best.map_or(i, |b| b.min(i)));
                }
            }
            if best.is_some() && limit.is_none() {
                if window == 0 {
                    break;
                }
                limit = Some(m + chrono::Duration::milliseconds(window));
            }
            c = m + chrono::Duration::seconds(1);
        }
        best
    }

    /// The first time of `cal` strictly after `after`.
    fn calendar_next(&self, cal: &Calendar, after: i64) -> Option<i64> {
        // The first whole second after `after`.
        let min_ms = after.div_euclid(1000) * 1000 + 1000;
        let from = self.local(min_ms)?;
        let simple = self.scan(cal, from, min_ms, 0);
        if self.quiet(min_ms) && simple.is_none_or(|t| self.quiet(t)) {
            return simple;
        }
        // Near a transition: start three hours of clock time earlier and
        // look three hours past the first hit.
        let from = from - chrono::Duration::milliseconds(TRANSITION_WINDOW_MS);
        self.scan(cal, from, min_ms, TRANSITION_WINDOW_MS)
    }

    fn excluded(&self, ms: i64) -> bool {
        if self.excludes.is_empty() {
            return false;
        }
        match self.local(ms) {
            Some(l) => self.excludes.iter().any(|c| c.matches(&l)),
            None => false,
        }
    }

    /// The first nominal time of any calendar or interval strictly after
    /// `after`, ignoring bounds and exclusions.
    pub fn raw_next(&self, after: i64) -> Option<i64> {
        let cal = self
            .calendars
            .iter()
            .filter_map(|c| self.calendar_next(c, after));
        let iv = self.intervals.iter().map(|i| i.next_after(after));
        cal.chain(iv).min()
    }

    fn past_end(&self, t: i64) -> bool {
        if self.end_ms.is_some_and(|e| t > e) {
            return true;
        }
        self.local(t)
            .is_none_or(|l| chrono::Datelike::year(&l) > calendar::MAX_YEAR)
    }

    /// The first action strictly after `after` (by nominal time): the
    /// nominal time within the bounds and not excluded, and its jittered
    /// actual time. `seed` makes the jitter deterministic per schedule.
    pub fn next(&self, seed: &str, after: i64) -> Option<NextTime> {
        // Start just before the range so its first instant can match.
        let mut after = match self.start_ms {
            Some(s) => after.max(s - 1),
            None => after,
        };
        let nominal = loop {
            let n = self.raw_next(after)?;
            if self.past_end(n) {
                return None;
            }
            if !self.excluded(n) {
                break n;
            }
            after = n;
        };
        let mut max_jitter = self.jitter_ms;
        if max_jitter > 0
            && let Some(following) = self.raw_next(nominal)
        {
            max_jitter = max_jitter.min(following - nominal);
        }
        Some(NextTime {
            nominal_ms: nominal,
            actual_ms: nominal + jitter(seed, nominal, max_jitter),
        })
    }

    /// The actions whose nominal times are in `[from, to]`, at most `limit`.
    pub fn times_between(&self, seed: &str, from: i64, to: i64, limit: usize) -> Vec<NextTime> {
        let mut out = Vec::new();
        let mut t = from - 1;
        while out.len() < limit {
            let Some(n) = self.next(seed, t) else {
                break;
            };
            if n.nominal_ms > to {
                break;
            }
            out.push(n);
            t = n.nominal_ms;
        }
        out
    }

    /// The next `n` actions strictly after `after`.
    pub fn upcoming(&self, seed: &str, after: i64, n: usize) -> Vec<NextTime> {
        let mut out = Vec::with_capacity(n);
        let mut t = after;
        while out.len() < n {
            let Some(x) = self.next(seed, t) else {
                break;
            };
            out.push(x);
            t = x.nominal_ms;
        }
        out
    }
}

/// The deterministic jitter in `[0, max_ms)`: SHA-256 of the seed and the
/// nominal time.
pub fn jitter(seed: &str, nominal_ms: i64, max_ms: i64) -> i64 {
    if max_ms <= 0 {
        return 0;
    }
    let mut h = Sha256::new();
    h.update(seed.as_bytes());
    h.update([0u8]);
    h.update(nominal_ms.to_be_bytes());
    let d = h.finalize();
    let mut b = [0u8; 8];
    b.copy_from_slice(&d[..8]);
    (u64::from_be_bytes(b) % max_ms as u64) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use loams_temporal_proto::temporal::api::schedule::v1::StructuredCalendarSpec;

    fn utc(s: &str) -> i64 {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
            .unwrap()
            .and_utc()
            .timestamp_millis()
    }

    fn show(ms: i64) -> String {
        DateTime::<Utc>::from_timestamp_millis(ms)
            .unwrap()
            .format("%Y-%m-%d %H:%M:%S")
            .to_string()
    }

    fn spec(f: impl FnOnce(&mut ScheduleSpec)) -> CompiledSpec {
        let mut s = ScheduleSpec::default();
        f(&mut s);
        CompiledSpec::compile(&s).unwrap()
    }

    fn noms(c: &CompiledSpec, after: &str, n: usize) -> Vec<String> {
        c.upcoming("seed", utc(after), n)
            .into_iter()
            .map(|t| show(t.nominal_ms))
            .collect()
    }

    #[test]
    fn cron_strings_fire_in_order() {
        let c = spec(|s| s.cron_string = vec!["*/30 9 * * mon".into()]);
        assert_eq!(
            noms(&c, "2026-10-27 00:00:00", 3),
            [
                "2026-11-02 09:00:00",
                "2026-11-02 09:30:00",
                "2026-11-09 09:00:00"
            ]
        );
    }

    #[test]
    fn several_specs_merge_and_dedupe() {
        let c = spec(|s| {
            s.cron_string = vec!["0 * * * *".into()];
            s.calendar = vec![CalendarSpec {
                minute: "0,30".into(),
                hour: "*".into(),
                ..Default::default()
            }];
        });
        assert_eq!(
            noms(&c, "2026-10-27 10:10:00", 3),
            [
                "2026-10-27 10:30:00",
                "2026-10-27 11:00:00",
                "2026-10-27 11:30:00"
            ]
        );
    }

    #[test]
    fn intervals_with_phase() {
        let c = spec(|s| s.cron_string = vec!["@every 1h/15m".into()]);
        assert_eq!(
            noms(&c, "2026-10-27 10:15:00", 2),
            ["2026-10-27 11:15:00", "2026-10-27 12:15:00"]
        );
        let i = Interval {
            every_ms: 10,
            phase_ms: 3,
        };
        assert_eq!(i.next_after(-100), -97);
        assert_eq!(i.next_after(3), 13);
        assert_eq!(i.next_after(2), 3);
    }

    #[test]
    fn exclusions_and_bounds() {
        let c = spec(|s| {
            s.cron_string = vec!["0 12 * * *".into()];
            // Not on weekends.
            s.exclude_structured_calendar = vec![StructuredCalendarSpec {
                second: vec![],
                minute: vec![],
                hour: vec![loams_temporal_proto::temporal::api::schedule::v1::Range {
                    start: 12,
                    end: 0,
                    step: 0,
                }],
                day_of_week: vec![
                    loams_temporal_proto::temporal::api::schedule::v1::Range {
                        start: 0,
                        end: 0,
                        step: 0,
                    },
                    loams_temporal_proto::temporal::api::schedule::v1::Range {
                        start: 6,
                        end: 0,
                        step: 0,
                    },
                ],
                ..Default::default()
            }];
            s.start_time = Some(ms_ts(utc("2026-10-30 12:00:00")));
            s.end_time = Some(ms_ts(utc("2026-11-03 12:00:00")));
        });
        // Fri 30 (the start, inclusive), skip Sat 31 and Sun 1, Mon 2, Tue 3
        // (the end, inclusive), then nothing.
        assert_eq!(
            noms(&c, "2026-10-01 00:00:00", 10),
            [
                "2026-10-30 12:00:00",
                "2026-11-02 12:00:00",
                "2026-11-03 12:00:00"
            ]
        );
    }

    #[test]
    fn time_zones_and_dst() {
        // 2026-11-01: New York falls back at 02:00 EDT to 01:00 EST.
        let c = spec(|s| {
            s.cron_string = vec!["30 1 * * *".into()];
            s.timezone_name = "America/New_York".into();
        });
        // 01:30 happens twice: 05:30Z (EDT) and 06:30Z (EST).
        assert_eq!(
            noms(&c, "2026-10-31 12:00:00", 3),
            [
                "2026-11-01 05:30:00",
                "2026-11-01 06:30:00",
                "2026-11-02 06:30:00"
            ]
        );
        // 2027-03-14: New York springs forward at 02:00; 02:30 never happens.
        let c = spec(|s| {
            s.cron_string = vec!["30 2 * * *".into()];
            s.timezone_name = "America/New_York".into();
        });
        assert_eq!(
            noms(&c, "2027-03-13 00:00:00", 2),
            ["2027-03-13 07:30:00", "2027-03-15 06:30:00"]
        );
        // The CRON_TZ prefix is the same as timezone_name.
        let c = spec(|s| s.cron_string = vec!["CRON_TZ=Asia/Kolkata 0 9 * * *".into()]);
        assert_eq!(noms(&c, "2026-10-27 00:00:00", 1), ["2026-10-27 03:30:00"]);
        assert_eq!(c.tz_name, "Asia/Kolkata");
    }

    #[test]
    fn refuses_bad_specs() {
        let bad = |f: fn(&mut ScheduleSpec)| {
            let mut s = ScheduleSpec::default();
            f(&mut s);
            CompiledSpec::compile(&s).is_err()
        };
        assert!(bad(|s| s.timezone_name = "Mars/Olympus".into()));
        assert!(bad(|s| s.timezone_data = vec![1u8].into()));
        assert!(bad(|s| s.cron_string = vec!["61 * * * *".into()]));
        assert!(bad(|s| {
            s.timezone_name = "UTC".into();
            s.cron_string = vec!["TZ=Asia/Tokyo * * * * *".into()];
        }));
        assert!(bad(|s| s.interval = vec![IntervalSpec::default()]));
        assert!(bad(|s| {
            s.start_time = Some(ms_ts(10_000));
            s.end_time = Some(ms_ts(5_000));
        }));
    }

    #[test]
    fn jitter_is_deterministic_and_capped() {
        let c = spec(|s| {
            s.cron_string = vec!["@every 1m".into()];
            s.jitter = Some(ms_duration(3_600_000));
        });
        let a = c.upcoming("ns/sched", utc("2026-10-27 00:00:00"), 50);
        let b = c.upcoming("ns/sched", utc("2026-10-27 00:00:00"), 50);
        assert_eq!(a, b);
        let mut moved = 0;
        for t in &a {
            // Never past the next nominal time (one minute on).
            assert!((t.nominal_ms..t.nominal_ms + 60_000).contains(&t.actual_ms));
            if t.actual_ms != t.nominal_ms {
                moved += 1;
            }
        }
        assert!(moved > 40, "jitter moved only {moved} of 50");
        let other = c.upcoming("ns/other", utc("2026-10-27 00:00:00"), 50);
        assert_ne!(a, other, "the seed changes the jitter");
        assert_eq!(jitter("x", 1, 0), 0);
    }

    #[test]
    #[allow(deprecated)]
    fn canonical_form_round_trips() {
        let s = ScheduleSpec {
            cron_string: vec![
                "CRON_TZ=Europe/Berlin 15 8 * * 1-5".into(),
                "@every 2h".into(),
            ],
            exclude_calendar: vec![CalendarSpec {
                day_of_month: "25".into(),
                month: "dec".into(),
                hour: "*".into(),
                minute: "*".into(),
                second: "*".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let c = canonicalize(&s).unwrap();
        assert!(c.cron_string.is_empty() && c.calendar.is_empty());
        assert_eq!(c.timezone_name, "Europe/Berlin");
        assert_eq!(c.structured_calendar.len(), 1);
        assert_eq!(c.interval.len(), 1);
        assert_eq!(c.exclude_structured_calendar.len(), 1);
        // Canonical is a fixed point, and fires the same times.
        assert_eq!(canonicalize(&c).unwrap(), c);
        let a = CompiledSpec::compile(&s)
            .unwrap()
            .upcoming("s", utc("2026-12-20 00:00:00"), 30);
        let b = CompiledSpec::compile(&c)
            .unwrap()
            .upcoming("s", utc("2026-12-20 00:00:00"), 30);
        assert_eq!(a, b);
    }

    #[test]
    fn times_between_is_inclusive() {
        let c = spec(|s| s.cron_string = vec!["0 * * * *".into()]);
        let t = c.times_between(
            "s",
            utc("2026-10-27 01:00:00"),
            utc("2026-10-27 03:00:00"),
            100,
        );
        let got: Vec<_> = t.iter().map(|x| show(x.nominal_ms)).collect();
        assert_eq!(
            got,
            [
                "2026-10-27 01:00:00",
                "2026-10-27 02:00:00",
                "2026-10-27 03:00:00"
            ]
        );
        assert_eq!(c.times_between("s", 0, i64::MAX / 4, 5).len(), 5);
    }
}
