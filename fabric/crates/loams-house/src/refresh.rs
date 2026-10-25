//! Refreshable materialized views over lake tables (HE1 R9.64, SigNoz's
//! metric reduction, D1571).
//!
//! ```sql
//! CREATE MATERIALIZED VIEW mv REFRESH EVERY 1 MINUTE RANDOMIZE FOR 10 SECOND
//!     APPEND TO t (cols…) EMPTY AS SELECT …, now() AS computed_at FROM …;
//! ```
//!
//! * **Forms.** `REFRESH EVERY n UNIT [OFFSET …] [RANDOMIZE FOR …]`, `APPEND`,
//!   `TO` with or without a column list, and `EMPTY`. `REFRESH AFTER`,
//!   `DEPENDS ON`, the refresh's `SETTINGS`, a refresh that replaces its
//!   target (no `APPEND`), an inner table and `EVERY` in weeks, months or years
//!   are `48`.
//! * **Storage.** The view is recorded on its **target** as
//!   `loams.refresh.<id>` (a [`MaterializedView`] with its schedule), never as a
//!   `loams.mv.<id>` of a source, so no insert fires it. Its last run is the
//!   target's `loams.refresh_at.<id>`: the period's scheduled time (Unix
//!   seconds), committed **with the appended rows** and required unchanged, as
//!   the views' cursors are ([`crate::pipes::commit_cursor`]). So with several
//!   fronts only the commit winner appends a period, and a retry never appends
//!   one twice.
//! * **When.** Periods are aligned to the Unix epoch (UTC; ClickHouse aligns
//!   `EVERY` to the unit's boundaries), plus the `OFFSET`, plus a random shift
//!   in `[-r/2, r/2]` for `RANDOMIZE FOR r`, which is the same on every front
//!   (it is a hash of the view and the period). A view due for several periods
//!   (the House was down) runs once, for the latest, as ClickHouse does. With
//!   `EMPTY` the first run is the next period's; without it, one run at
//!   `CREATE`.
//! * **`now()`.** Inside a run, `now()`, `now(tz)` and `today()` are the run's
//!   scheduled time: the period's, plus its random shift (ClickHouse 26.9.2.8
//!   gives the time the run started, measured: `EVERY 1 MINUTE RANDOMIZE FOR
//!   10 SECOND` read `14:18:02` for the 14:18 period). So every front and every
//!   retry of one period computes the same rows ([`pin_now`]). The first run
//!   of a view without `EMPTY` reads the time it ran. `now64()` is not pinned.
//! * **Errors.** A run that fails with the view's own error skips its period
//!   (recorded as the view's last error); the House's own trouble (a worker, the
//!   store) is tried again at the next round.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::catalog::{Lake, LakeTable};
use crate::ddl::{self, TableClass};
use crate::errors::{ChError, HouseError};
use crate::pipes::{MaterializedView, Ran, ViewWriter};
use crate::request::tokens;

/// A target's property: a refreshable view writing into it.
pub const REFRESH_PREFIX: &str = "loams.refresh.";
/// A target's property: the scheduled time of a view's last run.
pub const REFRESH_AT_PREFIX: &str = "loams.refresh_at.";
/// `loams.refresh_at.<id>` before the first run of a view without `EMPTY`:
/// run now.
pub const RUN_NOW: &str = "now";

/// The unit of a refresh interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unit {
    /// Seconds.
    Second,
    /// Minutes.
    Minute,
    /// Hours.
    Hour,
    /// Days.
    Day,
}

impl Unit {
    fn seconds(self) -> i64 {
        match self {
            Self::Second => 1,
            Self::Minute => 60,
            Self::Hour => 3_600,
            Self::Day => 86_400,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Second => "SECOND",
            Self::Minute => "MINUTE",
            Self::Hour => "HOUR",
            Self::Day => "DAY",
        }
    }

    /// A unit as written (`MINUTE`, `MINUTES`, …); `Err` names one the House
    /// does not schedule.
    fn parse(word: &str) -> Result<Self, String> {
        let upper = word.to_ascii_uppercase();
        let singular = upper.strip_suffix('S').unwrap_or(&upper);
        match singular {
            "SECOND" => Ok(Self::Second),
            "MINUTE" => Ok(Self::Minute),
            "HOUR" => Ok(Self::Hour),
            "DAY" => Ok(Self::Day),
            "WEEK" | "MONTH" | "QUARTER" | "YEAR" => Err(format!(
                "REFRESH EVERY … {singular} is not supported on the House: use SECOND, MINUTE, \
                 HOUR or DAY"
            )),
            other => Err(format!("Unknown interval unit {other:?} in REFRESH")),
        }
    }
}

/// When a refreshable view runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    /// `EVERY n`.
    pub every: i64,
    /// `EVERY n UNIT`.
    pub unit: Unit,
    /// `OFFSET …`, in seconds.
    #[serde(default)]
    pub offset_s: i64,
    /// `RANDOMIZE FOR …`, in seconds.
    #[serde(default)]
    pub randomize_s: i64,
    /// `APPEND`.
    pub append: bool,
    /// `EMPTY`: no run at `CREATE`.
    #[serde(default)]
    pub empty: bool,
}

impl Schedule {
    /// The period in seconds.
    pub fn period_s(&self) -> i64 {
        self.every * self.unit.seconds()
    }

    /// The scheduled time of the period `now_s` is in (its start, plus the
    /// offset).
    pub fn nominal_at(&self, now_s: i64) -> i64 {
        let p = self.period_s();
        (now_s - self.offset_s).div_euclid(p) * p + self.offset_s
    }

    /// The random shift of the run scheduled at `nominal`, in
    /// `[-randomize/2, randomize/2]`, the same on every front.
    pub fn shift_s(&self, id: &str, nominal: i64) -> i64 {
        if self.randomize_s <= 0 {
            return 0;
        }
        let mut h = Sha256::new();
        h.update(id.as_bytes());
        h.update(nominal.to_le_bytes());
        let digest = h.finalize();
        let n = u64::from_le_bytes(digest[..8].try_into().expect("8 bytes"));
        let span = self.randomize_s as u64 + 1;
        (n % span) as i64 - self.randomize_s / 2
    }

    /// The scheduled time of the run due at `now_s`, given the last run's
    /// (`last`: [`RUN_NOW`] or seconds); `None` when nothing is due.
    pub fn due(&self, id: &str, last: Option<&str>, now_s: i64) -> Option<i64> {
        let current = self.nominal_at(now_s);
        let last = match last {
            Some(RUN_NOW) | None => return Some(current),
            Some(text) => text.parse::<i64>().ok()?,
        };
        // The latest period whose (shifted) time has come: the next one's
        // when its shift is negative and has come, else this one's or the
        // last one's (a shift is at most half a period).
        let p = self.period_s();
        let mut candidate = current + p;
        while now_s < candidate + self.shift_s(id, candidate) {
            candidate -= p;
        }
        (candidate > last).then_some(candidate)
    }

    /// The clause as `SHOW CREATE` prints it.
    pub fn render(&self) -> String {
        let mut out = format!("REFRESH EVERY {} {}", self.every, self.unit.name());
        if self.offset_s != 0 {
            out.push_str(&format!(" OFFSET {} SECOND", self.offset_s));
        }
        if self.randomize_s != 0 {
            out.push_str(&format!(" RANDOMIZE FOR {} SECOND", self.randomize_s));
        }
        if self.append {
            out.push_str(" APPEND");
        }
        out
    }
}

/// Reads a `REFRESH` clause from the words after `REFRESH` up to (not
/// including) `TO`, `AS`, `EMPTY`, a column list or an `ENGINE`: `Ok` with the
/// schedule (`empty` unset), or the `48` message for a form the House refuses.
pub fn parse_clause(words: &[String]) -> Result<Schedule, String> {
    let mut at = 0;
    let word = |i: usize| words.get(i).map(|w| w.to_ascii_uppercase());
    match word(at).as_deref() {
        Some("EVERY") => at += 1,
        Some("AFTER") => {
            return Err(
                "REFRESH AFTER is not supported on the House: use REFRESH EVERY".to_string(),
            );
        }
        _ => return Err("Expected EVERY after REFRESH".to_string()),
    }
    let every: i64 = words
        .get(at)
        .and_then(|w| w.parse().ok())
        .filter(|n: &i64| *n > 0)
        .ok_or_else(|| "Expected a positive number after REFRESH EVERY".to_string())?;
    at += 1;
    let unit = Unit::parse(
        words
            .get(at)
            .ok_or("Expected a unit after REFRESH EVERY n")?,
    )?;
    at += 1;
    if word(at).is_some_and(|w| w.parse::<i64>().is_ok()) {
        return Err("REFRESH EVERY takes one unit on the House".to_string());
    }
    let mut schedule = Schedule {
        every,
        unit,
        offset_s: 0,
        randomize_s: 0,
        append: false,
        empty: false,
    };
    // `n UNIT [n UNIT …]`, summed, in seconds.
    let interval = |at: &mut usize| -> Result<i64, String> {
        let mut total = 0;
        let mut any = false;
        while let Some(n) = words.get(*at).and_then(|w| w.parse::<i64>().ok()) {
            let unit = Unit::parse(words.get(*at + 1).ok_or("Expected a unit")?)?;
            total += n * unit.seconds();
            *at += 2;
            any = true;
        }
        if any {
            Ok(total)
        } else {
            Err("Expected an interval".to_string())
        }
    };
    loop {
        match word(at).as_deref() {
            None => break,
            Some("OFFSET") => {
                at += 1;
                schedule.offset_s = interval(&mut at)?;
            }
            Some("RANDOMIZE") => {
                at += 1;
                if word(at).as_deref() != Some("FOR") {
                    return Err("Expected FOR after RANDOMIZE".to_string());
                }
                at += 1;
                schedule.randomize_s = interval(&mut at)?;
            }
            Some("APPEND") => {
                at += 1;
                schedule.append = true;
            }
            Some("DEPENDS") => {
                return Err("REFRESH … DEPENDS ON is not supported on the House".to_string());
            }
            Some("SETTINGS") => {
                return Err(
                    "Refresh SETTINGS are not supported on the House (its runs are not \
                     retried within a period)"
                        .to_string(),
                );
            }
            Some(other) => return Err(format!("Unexpected {other} in REFRESH")),
        }
    }
    if schedule.offset_s < 0 || schedule.offset_s >= schedule.period_s() {
        return Err("REFRESH … OFFSET must be less than the period".to_string());
    }
    if schedule.randomize_s > schedule.period_s() {
        return Err("REFRESH … RANDOMIZE FOR must not be longer than the period".to_string());
    }
    if !schedule.append {
        return Err(
            "A refreshable materialized view that replaces its target is not supported on the \
             House: add APPEND"
                .to_string(),
        );
    }
    Ok(schedule)
}

/// `select` with `now()`, `now(tz)` and `today()` read as `at_s` (Unix
/// seconds): a run's rows are the same on every front and every retry.
pub fn pin_now(select: &str, at_s: i64) -> String {
    let bytes = select.as_bytes();
    let toks = tokens(bytes);
    let mut out = String::with_capacity(select.len());
    let mut copied = 0;
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        let raw = &select[t.start..t.end];
        let call = t.word
            && (raw.eq_ignore_ascii_case("now") || raw.eq_ignore_ascii_case("today"))
            && toks
                .get(i + 1)
                .is_some_and(|n| n.end == n.start + 1 && bytes[n.start] == b'(')
            // Not a qualified name (`t.now()`) nor a column named `now`.
            && !(i > 0 && {
                let p = toks[i - 1];
                p.end == p.start + 1 && bytes[p.start] == b'.'
            });
        if !call {
            i += 1;
            continue;
        }
        // The matching `)`.
        let open = toks[i + 1];
        let Some(close) = toks[i + 2..]
            .iter()
            .position(|c| c.depth == open.depth && c.end == c.start + 1 && bytes[c.start] == b')')
            .map(|p| i + 2 + p)
        else {
            break;
        };
        let args = select[open.end..toks[close].start].trim();
        let replacement = if raw.eq_ignore_ascii_case("today") {
            format!("toDate(toDateTime({at_s}))")
        } else if args.is_empty() {
            format!("toDateTime({at_s})")
        } else {
            format!("toDateTime({at_s}, {args})")
        };
        out.push_str(&select[copied..t.start]);
        out.push_str(&replacement);
        copied = toks[close].end;
        i = close + 1;
    }
    out.push_str(&select[copied..]);
    out
}

/// The refreshable views recorded on a target, by name.
pub fn refreshes_of(table: &LakeTable) -> Vec<MaterializedView> {
    let mut views: Vec<MaterializedView> = table
        .metadata
        .properties()
        .iter()
        .filter(|(k, _)| k.starts_with(REFRESH_PREFIX))
        .filter_map(|(_, v)| serde_json::from_str(v).ok())
        .collect();
    views.sort_by(|a, b| (&a.database, &a.name).cmp(&(&b.database, &b.name)));
    views
}

/// The key of a view's record on its target.
pub fn record_key(view: &MaterializedView) -> String {
    format!("{REFRESH_PREFIX}{}", view.id)
}

/// The key of a view's last run on its target.
pub fn at_key(view: &MaterializedView) -> String {
    format!("{REFRESH_AT_PREFIX}{}", view.id)
}

/// What one refresh did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refreshed {
    /// Nothing was due.
    NotDue,
    /// This front appended the period scheduled at `.0`.
    Appended(i64),
    /// Another front had run the period.
    Abandoned,
}

/// Runs refreshable views.
#[derive(Debug, Clone)]
pub struct RefreshRunner {
    writer: ViewWriter,
}

impl RefreshRunner {
    /// A runner writing with `writer`.
    pub fn new(writer: ViewWriter) -> Self {
        Self { writer }
    }

    fn lake(&self) -> &Arc<Lake> {
        &self.writer.lake
    }

    /// Runs each refreshable view writing into `database.name` that is due at
    /// `now_s`. Answers the first error; the other views still run.
    pub async fn run_due(
        &self,
        namespace: u64,
        database: &str,
        name: &str,
        now_s: i64,
    ) -> Result<Vec<Refreshed>, HouseError> {
        let Some(target) = self.lake().load(namespace, database, name).await? else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        let mut first = None;
        for view in refreshes_of(&target) {
            match self.run_one(namespace, &view, now_s).await {
                Ok(r) => out.push(r),
                Err(err) => {
                    if first.is_none() {
                        first = Some(err);
                    }
                }
            }
        }
        first.map_or(Ok(out), Err)
    }

    /// One view, if it is due at `now_s`.
    pub async fn run_one(
        &self,
        namespace: u64,
        view: &MaterializedView,
        now_s: i64,
    ) -> Result<Refreshed, HouseError> {
        let result = self.run_inner(namespace, view, now_s).await;
        let states = self.lake().pipe_states();
        states.update(namespace, view, |state| match &result {
            Ok(Refreshed::Appended(_)) => {
                state.last_error.clear();
                state.last_commit_ms = Some(crate::retention::now_ms());
            }
            Ok(_) => {}
            Err(err) => state.last_error = err.message().to_string(),
        });
        result
    }

    async fn run_inner(
        &self,
        namespace: u64,
        view: &MaterializedView,
        now_s: i64,
    ) -> Result<Refreshed, HouseError> {
        let Some(schedule) = view.refresh.as_ref() else {
            return Ok(Refreshed::NotDue);
        };
        let lake = Arc::clone(self.lake());
        let target = lake
            .lake_table(
                namespace,
                &view.target_database,
                &view.target,
                "a refreshable materialized view's target",
            )
            .await?;
        if ddl::class_of(&target.metadata) != TableClass::Lake {
            return Ok(Refreshed::NotDue);
        }
        let key = at_key(view);
        let last = target.metadata.properties().get(&key).cloned();
        let Some(nominal) = schedule.due(&view.id, last.as_deref(), now_s) else {
            return Ok(Refreshed::NotDue);
        };
        let read: BTreeMap<String, String> = last
            .clone()
            .map(|v| BTreeMap::from([(key.clone(), v)]))
            .unwrap_or_default();
        let next = BTreeMap::from([(key.clone(), nominal.to_string())]);
        let committer = lake.committer();
        let io = committer.io();
        let at = if last.as_deref() == Some(RUN_NOW) {
            now_s
        } else {
            nominal + schedule.shift_s(&view.id, nominal)
        };
        let mut pinned = view.clone();
        pinned.select = pin_now(&view.select, at);
        let (rows, bytes, outcome) =
            match self.writer.write(namespace, &pinned, &target, None).await {
                Ok(written) => {
                    let outcome = crate::pipes::commit_cursor(
                        io,
                        &target.ident,
                        &read,
                        &next,
                        vec![(format!("loams.refresh.{}.at", view.id), nominal.to_string())],
                        written.data_files,
                    )
                    .await?;
                    (written.rows, written.input_bytes, outcome)
                }
                Err(err) if crate::mv::transient(&err) => return Err(err),
                Err(err) => {
                    // The view's own error: the period is skipped.
                    crate::pipes::commit_cursor(
                        io,
                        &target.ident,
                        &read,
                        &next,
                        Vec::new(),
                        Vec::new(),
                    )
                    .await?;
                    return Err(err);
                }
            };
        match outcome {
            Ran::Committed { .. } => {
                self.writer.report_insert(namespace, view, rows, bytes);
                committer.mark_dirty(&target.ident);
                Ok(Refreshed::Appended(nominal))
            }
            Ran::Abandoned | Ran::Idle => Ok(Refreshed::Abandoned),
        }
    }
}

/// The lake tables of a namespace that refreshable views write into.
pub async fn targets(lake: &Lake, namespace: u64) -> Result<Vec<(String, String)>, HouseError> {
    let mut out = Vec::new();
    for database in lake.databases(namespace).await? {
        for name in lake.table_names(namespace, &database.name).await? {
            if let Some(table) = lake.load(namespace, &database.name, &name).await?
                && ddl::class_of(&table.metadata) == TableClass::Lake
                && !refreshes_of(&table).is_empty()
            {
                out.push((database.name.clone(), name));
            }
        }
    }
    Ok(out)
}

/// The `48` of a refused refresh form.
pub fn refused(message: String) -> HouseError {
    HouseError::from(ChError::not_implemented(message))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn signoz_clauses_are_read() {
        let s = parse_clause(&words("EVERY 1 MINUTE RANDOMIZE FOR 10 SECOND APPEND")).unwrap();
        assert_eq!(
            (s.every, s.unit, s.randomize_s, s.append),
            (1, Unit::Minute, 10, true)
        );
        assert_eq!(s.period_s(), 60);
        let s = parse_clause(&words("EVERY 30 MINUTE RANDOMIZE FOR 2 MINUTE APPEND")).unwrap();
        assert_eq!((s.period_s(), s.randomize_s), (1800, 120));
        let s = parse_clause(&words("EVERY 1 DAY OFFSET 2 HOUR 30 MINUTES APPEND")).unwrap();
        assert_eq!(s.offset_s, 9000);
        assert_eq!(s.render(), "REFRESH EVERY 1 DAY OFFSET 9000 SECOND APPEND");
    }

    #[test]
    fn other_forms_are_refused() {
        for (text, needle) in [
            ("AFTER 1 MINUTE APPEND", "REFRESH AFTER"),
            ("EVERY 1 MINUTE", "APPEND"),
            ("EVERY 1 MONTH APPEND", "MONTH"),
            ("EVERY 1 WEEK APPEND", "WEEK"),
            ("EVERY 1 MINUTE DEPENDS ON db.v APPEND", "DEPENDS ON"),
            (
                "EVERY 1 MINUTE SETTINGS refresh_retries = 1 APPEND",
                "SETTINGS",
            ),
            ("EVERY 1 HOUR OFFSET 2 HOUR APPEND", "OFFSET"),
            ("EVERY 0 MINUTE APPEND", "positive"),
        ] {
            let err = parse_clause(&words(text)).unwrap_err();
            assert!(err.contains(needle), "{text}: {err}");
        }
    }

    #[test]
    fn periods_align_to_the_epoch() {
        let s = parse_clause(&words("EVERY 5 MINUTE APPEND")).unwrap();
        // 2026-10-23 12:07:30 UTC.
        let now = 1_792_757_250;
        assert_eq!(s.nominal_at(now), 1_792_757_100);
        // The current period's run is due once, then nothing until the next.
        assert_eq!(s.due("v", Some("1792756800"), now), Some(1_792_757_100));
        assert_eq!(s.due("v", Some("1792757100"), now), None);
        assert_eq!(
            s.due("v", Some("1792757100"), now + 300),
            Some(1_792_757_400)
        );
        // Missed periods run once, for the latest.
        assert_eq!(s.due("v", Some("1792700000"), now), Some(1_792_757_100));
        // Without EMPTY, at once.
        assert_eq!(s.due("v", Some(RUN_NOW), now), Some(1_792_757_100));
    }

    #[test]
    fn randomized_runs_wait_for_their_shift() {
        let s = parse_clause(&words("EVERY 1 MINUTE RANDOMIZE FOR 10 SECOND APPEND")).unwrap();
        let nominal = 1_792_757_100;
        let last = (nominal - 60).to_string();
        let mut shifts = std::collections::BTreeSet::new();
        for id in ["a", "b", "c", "d", "e", "f", "g", "h"] {
            let shift = s.shift_s(id, nominal);
            assert!((-5..=5).contains(&shift), "{shift}");
            assert_eq!(shift, s.shift_s(id, nominal), "the same on every front");
            shifts.insert(shift);
            assert_eq!(s.due(id, Some(&last), nominal + shift - 1), None, "{id}");
            assert_eq!(
                s.due(id, Some(&last), nominal + shift),
                Some(nominal),
                "{id}"
            );
        }
        assert!(shifts.len() > 1, "the shift varies by view: {shifts:?}");
        let err = parse_clause(&words("EVERY 1 MINUTE RANDOMIZE FOR 2 MINUTE APPEND"));
        assert!(err.unwrap_err().contains("RANDOMIZE"));
    }

    #[test]
    fn now_is_pinned() {
        let at = 1_792_757_100;
        assert_eq!(
            pin_now(
                "SELECT now() AS computed_at, x FROM t WHERE ts < now() - 120",
                at
            ),
            "SELECT toDateTime(1792757100) AS computed_at, x FROM t WHERE ts < \
             toDateTime(1792757100) - 120"
        );
        assert_eq!(
            pin_now(
                "SELECT NOW('UTC'), today(), now64(), 'now()', t.now(), `now`",
                at
            ),
            "SELECT toDateTime(1792757100, 'UTC'), toDate(toDateTime(1792757100)), now64(), \
             'now()', t.now(), `now`"
        );
        assert_eq!(
            pin_now(
                "SELECT toUnixTimestamp(now() - toIntervalMinute(11)) * 1000",
                at
            ),
            "SELECT toUnixTimestamp(toDateTime(1792757100) - toIntervalMinute(11)) * 1000"
        );
    }
}
