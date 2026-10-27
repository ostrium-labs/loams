//! The meter's House SQL, as text (design §63 §7.5, §8; ME1 Tasks 5–8).
//!
//! **std only.** This module is also compiled, with `#[path]`, by the
//! fabric workspace's real-House test
//! (`fabric/crates/loams-house-worker/tests/meter.rs`), which runs every
//! statement here against a House on chDB. So it uses nothing but `std`,
//! and every statement it builds is plain ClickHouse SQL that the House
//! accepts.
//!
//! Everything lives in the database [`DATABASE`] of the House's system
//! namespace:
//!
//! | Table | Engine | What |
//! |---|---|---|
//! | `_meter.events_q` | `LoamsStream('_meter', 'CloudEvents', 'house-meter')` | the pipe: `_system/_meter`'s records as `_ce_*` columns |
//! | `_meter.events` | `ReplacingMergeTree`, `ORDER BY (subject, type, source, id)` | every usage event once (read with `FINAL`) |
//! | `_meter.events_mv` | materialized view `events_q` → `events` | maps the CloudEvent onto the row |
//! | `_meter.windows` | `ReplacingMergeTree(close_run)` | closed minute, hour and day windows per meter |
//! | `_meter.close_runs` | `ReplacingMergeTree(close_run)` | each meter's last close run (Task 6) |

/// The House database of the meter's tables.
pub const DATABASE: &str = "_meter";

/// The stream the pipe reads: `_meter` of the `_system` namespace (the
/// House namespace's engine namespace is the operator's mapping).
pub const STREAM: &str = "_meter";

/// The pipe's consumer label (§63 §7.5).
pub const PIPE_CONSUMER: &str = "house-meter";

/// The most records one pipe batch takes (`loams_pipe_max_rows`).
pub const PIPE_MAX_ROWS: u64 = 100_000;

/// `'text'` with ClickHouse string escaping.
pub fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('\'');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// `CREATE DATABASE IF NOT EXISTS _meter`.
pub fn create_database() -> String {
    format!("CREATE DATABASE IF NOT EXISTS {DATABASE}")
}

/// The pipe: `_system/_meter`'s CloudEvents as the House's `_ce_*` columns
/// (HS1 Task 16's layout: the extensions, `tenantid` and `loamslease`
/// among them, are in `_ce_ext`).
pub fn create_events_pipe() -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS {DATABASE}.events_q (\
         _ce_id String, _ce_source String, _ce_type String, _ce_subject String, \
         _ce_time String, _ce_data String, _ce_ext Map(String, String)) \
         ENGINE = LoamsStream({}, 'CloudEvents', {}) \
         SETTINGS loams_pipe_max_rows = {PIPE_MAX_ROWS}",
        quote(STREAM),
        quote(PIPE_CONSUMER),
    )
}

/// `_meter.events`: every event once. `ReplacingMergeTree` keyed by
/// `(subject, type, source, id)` (dedupe layer 3, §63 §7.3): every billing
/// read uses `FINAL`. A duplicate has the same `time`, so it lands in its
/// original's partition and `FINAL` sees both.
///
/// `ingested_at` replaces §63 §7.5's `stream_partition` and `stream_offset`
/// (ruling T5-1): the pipe's `CloudEvents` format has no partition or offset
/// columns, and the close job finds late events by the time they reached
/// the House instead.
pub fn create_events() -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS {DATABASE}.events (\
         subject String, \
         type LowCardinality(String), \
         source String, \
         id String, \
         time DateTime64(6, 'UTC'), \
         tenantid String, \
         lease String, \
         data String, \
         ingested_at DateTime('UTC')) \
         ENGINE = ReplacingMergeTree \
         PARTITION BY toYYYYMM(time) \
         ORDER BY (subject, type, source, id)"
    )
}

/// The view from the pipe into `_meter.events`. A missing extension reads
/// as `''` (a `Map`'s default), so an event without `loamslease` has
/// `lease = ''`.
pub fn create_events_view() -> String {
    format!(
        "CREATE MATERIALIZED VIEW IF NOT EXISTS {DATABASE}.events_mv TO {DATABASE}.events AS \
         SELECT _ce_subject AS subject, _ce_type AS type, _ce_source AS source, _ce_id AS id, \
         parseDateTime64BestEffort(_ce_time, 6, 'UTC') AS time, \
         _ce_ext['tenantid'] AS tenantid, _ce_ext['loamslease'] AS lease, \
         _ce_data AS data, now() AS ingested_at \
         FROM {DATABASE}.events_q"
    )
}

/// `_meter.windows` (§63 §8.2): one row per (meter, subject, window size,
/// window start, group key), versioned by `close_run` so a later close run
/// wins and a run repeated gives the same rows.
pub fn create_windows() -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS {DATABASE}.windows (\
         meter LowCardinality(String), \
         subject String, \
         window_size Enum8('minute' = 1, 'hour' = 2, 'day' = 3), \
         window_start DateTime('UTC'), \
         group_key String, \
         value Decimal(38, 9), \
         events UInt64, \
         close_run UInt64) \
         ENGINE = ReplacingMergeTree(close_run) \
         PARTITION BY toYYYYMM(window_start) \
         ORDER BY (meter, subject, window_size, window_start, group_key)"
    )
}

/// Every statement that sets the meter's House tables up, in order. Each is
/// `IF NOT EXISTS`, so applying them again changes nothing.
pub fn schema() -> Vec<String> {
    vec![
        create_database(),
        create_events(),
        create_windows(),
        create_close_runs(),
        create_events_pipe(),
        create_events_view(),
    ]
}

// ---------------------------------------------------------------------------
// Meters and windows (§63 §8.1, §8.2; ME1 Task 6).

/// A meter's aggregation (OpenMeter's, §63 §8.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aggregation {
    Sum,
    Count,
    UniqueCount,
    Avg,
    Min,
    Max,
    Latest,
}

impl Aggregation {
    /// Whether an hour or a day window is the same aggregation of its minute
    /// windows. `UNIQUE_COUNT`, `AVG` and `LATEST` are not: they are
    /// recomputed from raw events for every window size (§63 §8.2).
    pub fn rolls_up(self) -> bool {
        matches!(self, Self::Sum | Self::Count | Self::Min | Self::Max)
    }

    /// OpenMeter's name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Sum => "SUM",
            Self::Count => "COUNT",
            Self::UniqueCount => "UNIQUE_COUNT",
            Self::Avg => "AVG",
            Self::Min => "MIN",
            Self::Max => "MAX",
            Self::Latest => "LATEST",
        }
    }
}

/// The window sizes `_meter.windows` holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WindowSize {
    Minute,
    Hour,
    Day,
}

impl WindowSize {
    /// The `window_size` enum value.
    pub fn name(self) -> &'static str {
        match self {
            Self::Minute => "minute",
            Self::Hour => "hour",
            Self::Day => "day",
        }
    }

    /// Its length in seconds.
    pub fn seconds(self) -> i64 {
        match self {
            Self::Minute => 60,
            Self::Hour => 3_600,
            Self::Day => 86_400,
        }
    }

    /// The ClickHouse function that truncates a time to it (UTC).
    pub fn trunc(self) -> &'static str {
        match self {
            Self::Minute => "toStartOfMinute",
            Self::Hour => "toStartOfHour",
            Self::Day => "toStartOfDay",
        }
    }

    /// `unix` truncated to the window's start (UTC; whole seconds).
    pub fn floor(self, unix: i64) -> i64 {
        unix.div_euclid(self.seconds()) * self.seconds()
    }
}

/// A meter as the House SQL needs it: the std-only mirror of
/// `loams_meter_core::Meter`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeterDef {
    /// `^[a-z0-9]+(?:_[a-z0-9]+)*$`, at most 64 characters (checked by core).
    pub slug: String,
    /// The event `type` it selects.
    pub event_type: String,
    pub aggregation: Aggregation,
    /// A JSONPath into `data` (`$.cpu_ms`); `None` for `COUNT`.
    pub value_property: Option<String>,
    /// Group-by dimensions, sorted by name: (name, JSONPath).
    pub group_by: Vec<(String, String)>,
    /// Events before this time (Unix seconds) are ignored.
    pub event_from: Option<i64>,
}

/// `toDateTime(<unix>, 'UTC')`.
pub fn at(unix: i64) -> String {
    format!("toDateTime({unix}, 'UTC')")
}

/// `toDateTime64(<unix>, 6, 'UTC')`, comparable with `_meter.events.time`.
pub fn at64(unix: i64) -> String {
    format!("toDateTime64({unix}, 6, 'UTC')")
}

/// The value an event contributes, as `Decimal(38, 9)` or `NULL` when its
/// `value_property` is missing or not a number (such an event is not
/// counted by `SUM`, `AVG`, `MIN`, `MAX`, `LATEST`, as OpenMeter skips it).
pub fn value_expr(meter: &MeterDef) -> String {
    match &meter.value_property {
        Some(path) => format!("toDecimal128OrNull(JSON_VALUE(data, {}), 9)", quote(path)),
        None => "CAST(NULL, 'Nullable(Decimal(38, 9))')".to_string(),
    }
}

/// The canonical JSON of the group-by values (`{}` without group-by): the
/// dimensions sorted by name, each value the JSON text's string.
pub fn group_key_expr(meter: &MeterDef) -> String {
    if meter.group_by.is_empty() {
        return "'{}'".to_string();
    }
    let pairs: Vec<String> = meter
        .group_by
        .iter()
        .map(|(name, path)| format!("{}, JSON_VALUE(data, {})", quote(name), quote(path)))
        .collect();
    format!("toJSONString(map({}))", pairs.join(", "))
}

/// The aggregation over raw events (rows of `_meter.events FINAL`), as
/// `Decimal(38, 9)`.
pub fn raw_agg_expr(meter: &MeterDef) -> String {
    let v = value_expr(meter);
    match meter.aggregation {
        Aggregation::Sum => format!("ifNull(sum({v}), toDecimal128(0, 9))"),
        Aggregation::Count => "toDecimal128(count(), 9)".to_string(),
        Aggregation::UniqueCount => match &meter.value_property {
            Some(path) => format!(
                "toDecimal128(uniqExactIf(JSON_VALUE(data, {p}), JSON_VALUE(data, {p}) != ''), 9)",
                p = quote(path)
            ),
            None => "toDecimal128(0, 9)".to_string(),
        },
        Aggregation::Avg => format!(
            "if(count({v}) = 0, toDecimal128(0, 9), toDecimal128(sum({v}) / count({v}), 9))"
        ),
        Aggregation::Min => format!("ifNull(min({v}), toDecimal128(0, 9))"),
        Aggregation::Max => format!("ifNull(max({v}), toDecimal128(0, 9))"),
        Aggregation::Latest => {
            format!("ifNull(argMaxIf({v}, time, isNotNull({v})), toDecimal128(0, 9))")
        }
    }
}

/// The roll-up of minute windows' `value` for an aggregation that
/// [rolls up](Aggregation::rolls_up).
fn rollup_agg_expr(aggregation: Aggregation) -> &'static str {
    match aggregation {
        Aggregation::Min => "min(value)",
        Aggregation::Max => "max(value)",
        _ => "sum(value)",
    }
}

/// The filter of a meter's events.
pub fn events_filter(meter: &MeterDef) -> String {
    let mut filter = format!("type = {}", quote(&meter.event_type));
    if let Some(from) = meter.event_from {
        filter.push_str(&format!(" AND time >= {}", at64(from)));
    }
    filter
}

/// `_meter.close_runs`: each meter's last close run (its start and how far
/// it closed), so the next run knows which events arrived since.
pub fn create_close_runs() -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS {DATABASE}.close_runs (\
         meter String, \
         close_run UInt64, \
         started_at DateTime('UTC'), \
         closed_to DateTime('UTC')) \
         ENGINE = ReplacingMergeTree(close_run) \
         ORDER BY meter"
    )
}

/// The meter's last close run: `started_at\tclosed_to` (Unix seconds), or
/// nothing.
pub fn last_close_run(slug: &str) -> String {
    format!(
        "SELECT toUnixTimestamp(started_at), toUnixTimestamp(closed_to) \
         FROM {DATABASE}.close_runs FINAL WHERE meter = {} \
         ORDER BY close_run DESC LIMIT 1",
        quote(slug)
    )
}

/// The minute windows (Unix seconds) with an event that reached the House at
/// or after `since` and that end at or before `close_to`: the new windows
/// and the late events' windows alike.
pub fn touched_minutes(meter: &MeterDef, since: i64, close_to: i64) -> String {
    format!(
        "SELECT DISTINCT toUnixTimestamp(toStartOfMinute(time)) AS w \
         FROM {DATABASE}.events WHERE {} AND ingested_at >= {} AND time < {} ORDER BY w",
        events_filter(meter),
        at(since),
        at64(close_to)
    )
}

fn list(starts: &[i64]) -> String {
    starts
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Computes the minute windows starting at `starts` from the raw events and
/// writes them with version `run`.
pub fn close_minutes(meter: &MeterDef, starts: &[i64], run: u64) -> String {
    let lo = starts.iter().min().copied().unwrap_or(0);
    let hi = starts.iter().max().copied().unwrap_or(0) + 60;
    format!(
        "INSERT INTO {DATABASE}.windows \
         (meter, subject, window_size, window_start, group_key, value, events, close_run) \
         SELECT {slug}, subject, 'minute', toDateTime(toStartOfMinute(time), 'UTC') AS ws, \
         {gk} AS gk, {agg}, count(), {run} \
         FROM {DATABASE}.events FINAL \
         WHERE {filter} AND time >= {lo64} AND time < {hi64} \
         AND toUnixTimestamp(toStartOfMinute(time)) IN ({list}) \
         GROUP BY subject, ws, gk",
        slug = quote(&meter.slug),
        gk = group_key_expr(meter),
        agg = raw_agg_expr(meter),
        filter = events_filter(meter),
        lo64 = at64(lo),
        hi64 = at64(hi),
        list = list(starts),
    )
}

/// Computes the `size` (hour or day) windows starting at `starts`, closed
/// up to `close_to`, and writes them with version `run`: rolled up from the
/// minute windows when the aggregation [rolls up](Aggregation::rolls_up),
/// recomputed from the raw events otherwise.
pub fn close_rollup(
    meter: &MeterDef,
    size: WindowSize,
    starts: &[i64],
    close_to: i64,
    run: u64,
) -> String {
    let lo = starts.iter().min().copied().unwrap_or(0);
    let hi = (starts.iter().max().copied().unwrap_or(0) + size.seconds()).min(close_to);
    let trunc = size.trunc();
    if meter.aggregation.rolls_up() {
        format!(
            "INSERT INTO {DATABASE}.windows \
             (meter, subject, window_size, window_start, group_key, value, events, close_run) \
             SELECT meter, subject, {name}, toDateTime({trunc}(window_start), 'UTC') AS ws, \
             group_key, {agg}, sum(events), {run} \
             FROM {DATABASE}.windows FINAL \
             WHERE meter = {slug} AND window_size = 'minute' \
             AND window_start >= {lo} AND window_start < {hi} \
             AND toUnixTimestamp({trunc}(window_start)) IN ({list}) \
             GROUP BY meter, subject, ws, group_key",
            name = quote(size.name()),
            agg = rollup_agg_expr(meter.aggregation),
            slug = quote(&meter.slug),
            lo = at(lo),
            hi = at(hi),
            list = list(starts),
        )
    } else {
        format!(
            "INSERT INTO {DATABASE}.windows \
             (meter, subject, window_size, window_start, group_key, value, events, close_run) \
             SELECT {slug}, subject, {name}, toDateTime({trunc}(time), 'UTC') AS ws, \
             {gk} AS gk, {agg}, count(), {run} \
             FROM {DATABASE}.events FINAL \
             WHERE {filter} AND time >= {lo64} AND time < {hi64} \
             AND toUnixTimestamp({trunc}(time)) IN ({list}) \
             GROUP BY subject, ws, gk",
            slug = quote(&meter.slug),
            name = quote(size.name()),
            gk = group_key_expr(meter),
            agg = raw_agg_expr(meter),
            filter = events_filter(meter),
            lo64 = at64(lo),
            hi64 = at64(hi),
            list = list(starts),
        )
    }
}

/// Records a finished close run of `slug`.
pub fn record_close_run(slug: &str, run: u64, started_at: i64, closed_to: i64) -> String {
    format!(
        "INSERT INTO {DATABASE}.close_runs (meter, close_run, started_at, closed_to) \
         SELECT {}, {run}, {}, {}",
        quote(slug),
        at(started_at),
        at(closed_to)
    )
}

/// How the close job closes (§63 §8.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CloseConfig {
    /// A minute window closes once it ended this long ago (2 minutes).
    pub grace_secs: i64,
    /// Events that reached the House up to this long before the previous
    /// run started are looked at again (10 minutes): longer than a pipe
    /// batch takes to commit, a run's interval and the 5 minutes an event
    /// may be in the future, so no event is missed between runs.
    pub margin_secs: i64,
    /// At most this many windows in one statement's `IN` list.
    pub batch: usize,
}

impl Default for CloseConfig {
    fn default() -> Self {
        CloseConfig {
            grace_secs: 120,
            margin_secs: 600,
            batch: 1_440,
        }
    }
}

/// What a close run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CloseSummary {
    /// The run's version (`close_run`).
    pub run: u64,
    /// Minute windows ending at or before this (Unix seconds) are closed.
    pub closed_to: i64,
    /// Minute, hour and day windows written.
    pub minutes: usize,
    pub hours: usize,
    pub days: usize,
}

#[derive(Debug)]
enum CloseState {
    Start,
    LastRun,
    Touched,
    Writing,
    Done,
}

/// One close run of one meter, as a sans-I/O machine: it hands out SQL,
/// one statement at a time, and reads each statement's answer. The `meter`
/// role's driver and fabric's House test run the same machine.
///
/// ```text
/// let mut run = CloseRun::new(meter, now, CloseConfig::default());
/// let mut answer = None;
/// while let Some(sql) = run.next(answer.as_deref())? { answer = Some(house.query(&sql)?); }
/// ```
///
/// 1. reads the meter's last run from `_meter.close_runs`;
/// 2. lists the minute windows with events that reached the House since that
///    run started (less the margin), up to `closed_to`: new windows and late
///    events' windows alike (a late event re-closes its windows, §63 §8.2);
/// 3. writes those minute windows, then their hours and days, all with
///    version `run`, then records the run.
///
/// Running it twice, or two runs racing, gives the same final rows: every
/// window row is keyed and versioned, and a run recomputes whole windows.
#[derive(Debug)]
pub struct CloseRun {
    meter: MeterDef,
    config: CloseConfig,
    now: i64,
    summary: CloseSummary,
    state: CloseState,
    queue: std::collections::VecDeque<String>,
}

fn parse_i64(field: &str) -> Result<i64, String> {
    field
        .trim()
        .parse()
        .map_err(|_| format!("not a number: {field:?}"))
}

fn chunks(values: &[i64], size: usize) -> Vec<Vec<i64>> {
    values.chunks(size.max(1)).map(<[i64]>::to_vec).collect()
}

impl CloseRun {
    /// A run of `meter` at `now_ms` (Unix milliseconds, which is also the
    /// run's version).
    pub fn new(meter: MeterDef, now_ms: i64, config: CloseConfig) -> Self {
        let now = now_ms.div_euclid(1_000);
        let closed_to = WindowSize::Minute.floor(now - config.grace_secs);
        CloseRun {
            meter,
            config,
            now,
            summary: CloseSummary {
                run: now_ms.max(0) as u64,
                closed_to,
                ..CloseSummary::default()
            },
            state: CloseState::Start,
            queue: std::collections::VecDeque::new(),
        }
    }

    /// What the run did (complete once [`CloseRun::next`] returned `None`).
    pub fn summary(&self) -> &CloseSummary {
        &self.summary
    }

    /// The next statement, given the previous one's answer (`None` at the
    /// start); `Ok(None)` once the run is done.
    pub fn next(&mut self, answer: Option<&str>) -> Result<Option<String>, String> {
        loop {
            match std::mem::replace(&mut self.state, CloseState::Done) {
                CloseState::Start => {
                    self.state = CloseState::LastRun;
                    return Ok(Some(last_close_run(&self.meter.slug)));
                }
                CloseState::LastRun => {
                    let line = answer.unwrap_or("").lines().next().unwrap_or("");
                    let since = if line.trim().is_empty() {
                        0
                    } else {
                        let started = parse_i64(line.split('\t').next().unwrap_or(""))?;
                        (started - self.config.margin_secs).max(0)
                    };
                    self.state = CloseState::Touched;
                    return Ok(Some(touched_minutes(
                        &self.meter,
                        since,
                        self.summary.closed_to,
                    )));
                }
                CloseState::Touched => {
                    let mut minutes = Vec::new();
                    for line in answer.unwrap_or("").lines() {
                        if !line.trim().is_empty() {
                            minutes.push(parse_i64(line)?);
                        }
                    }
                    minutes.sort_unstable();
                    minutes.dedup();
                    self.plan(&minutes);
                    self.state = CloseState::Writing;
                }
                CloseState::Writing => match self.queue.pop_front() {
                    Some(sql) => {
                        self.state = CloseState::Writing;
                        return Ok(Some(sql));
                    }
                    None => return Ok(None),
                },
                CloseState::Done => return Ok(None),
            }
        }
    }

    fn plan(&mut self, minutes: &[i64]) {
        let run = self.summary.run;
        let closed_to = self.summary.closed_to;
        let batch = self.config.batch;
        for chunk in chunks(minutes, batch) {
            self.queue
                .push_back(close_minutes(&self.meter, &chunk, run));
        }
        self.summary.minutes = minutes.len();
        for size in [WindowSize::Hour, WindowSize::Day] {
            let mut starts: Vec<i64> = minutes.iter().map(|m| size.floor(*m)).collect();
            starts.dedup();
            for chunk in chunks(&starts, batch) {
                self.queue
                    .push_back(close_rollup(&self.meter, size, &chunk, closed_to, run));
            }
            match size {
                WindowSize::Hour => self.summary.hours = starts.len(),
                _ => self.summary.days = starts.len(),
            }
        }
        self.queue
            .push_back(record_close_run(&self.meter.slug, run, self.now, closed_to));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_escapes() {
        assert_eq!(quote("a'b\\c"), "'a\\'b\\\\c'");
        assert_eq!(quote("x\ny"), "'x\\ny'");
    }

    #[test]
    fn schema_is_idempotent_ddl_in_order() {
        let schema = schema();
        assert!(schema.iter().all(|s| s.contains("IF NOT EXISTS")));
        // The pipe's target exists before its view, and the pipe before the view.
        let pos = |needle: &str| {
            schema
                .iter()
                .position(|s| s.contains(needle))
                .expect(needle)
        };
        assert!(pos("_meter.events (") < pos("events_mv"));
        assert!(pos("_meter.events_q") < pos("events_mv"));
        assert!(
            schema[pos("_meter.events_q")]
                .contains("LoamsStream('_meter', 'CloudEvents', 'house-meter')")
        );
        assert!(schema[1].contains("ReplacingMergeTree PARTITION BY toYYYYMM(time)"));
        assert!(schema[1].contains("ORDER BY (subject, type, source, id)"));
        assert!(schema[2].contains("ReplacingMergeTree(close_run)"));
        assert!(schema[pos("_meter.close_runs")].contains("ORDER BY meter"));
    }
}

#[cfg(test)]
mod close_tests {
    use super::*;

    fn sum_meter() -> MeterDef {
        MeterDef {
            slug: "house_cpu_ms".to_string(),
            event_type: "io.loams.dev.usage.house.query.v1".to_string(),
            aggregation: Aggregation::Sum,
            value_property: Some("$.cpu_ms".to_string()),
            group_by: vec![("namespace".to_string(), "$.namespace".to_string())],
            event_from: None,
        }
    }

    #[test]
    fn rolls_up_only_additive_aggregations() {
        let rolling: Vec<_> = [
            Aggregation::Sum,
            Aggregation::Count,
            Aggregation::UniqueCount,
            Aggregation::Avg,
            Aggregation::Min,
            Aggregation::Max,
            Aggregation::Latest,
        ]
        .into_iter()
        .filter(|a| a.rolls_up())
        .map(Aggregation::name)
        .collect();
        assert_eq!(rolling, ["SUM", "COUNT", "MIN", "MAX"]);
    }

    #[test]
    fn floors_to_windows() {
        assert_eq!(WindowSize::Minute.floor(119), 60);
        assert_eq!(WindowSize::Hour.floor(7_199), 3_600);
        assert_eq!(WindowSize::Day.floor(86_401), 86_400);
        assert_eq!(WindowSize::Minute.floor(-1), -60);
    }

    #[test]
    fn group_key_is_sorted_canonical_json() {
        let mut m = sum_meter();
        assert_eq!(
            group_key_expr(&m),
            "toJSONString(map('namespace', JSON_VALUE(data, '$.namespace')))"
        );
        m.group_by.clear();
        assert_eq!(group_key_expr(&m), "'{}'");
    }

    /// The machine's statements for a first run and for a later one.
    #[test]
    fn close_run_plans_minutes_rollups_and_record() {
        // 2026-10-19T10:00:30Z
        let now_ms = 1_792_404_030_000;
        let mut run = CloseRun::new(sum_meter(), now_ms, CloseConfig::default());
        let first = run.next(None).expect("ok").expect("sql");
        assert!(first.contains("FROM _meter.close_runs FINAL WHERE meter = 'house_cpu_ms'"));
        // No earlier run: everything since 0, closed to 09:58.
        let touched = run.next(Some("")).expect("ok").expect("sql");
        assert!(
            touched.contains("ingested_at >= toDateTime(0, 'UTC')"),
            "{touched}"
        );
        let closed_to = 1_792_404_030 - 120 - (1_792_404_030 - 120) % 60;
        assert_eq!(run.summary().closed_to, closed_to);
        assert!(touched.contains(&format!("time < {}", at64(closed_to))));
        // Two minutes in one hour.
        let m1 = 1_792_400_040 - 1_792_400_040 % 60;
        let m2 = m1 + 60;
        let mut statements = Vec::new();
        let mut answer = Some(format!("{m1}\n{m2}\n"));
        while let Some(sql) = run.next(answer.as_deref()).expect("ok") {
            statements.push(sql);
            answer = Some(String::new());
        }
        assert_eq!(statements.len(), 4, "{statements:#?}");
        assert!(
            statements[0].contains("'minute'")
                && statements[0].contains(&format!("IN ({m1}, {m2})"))
        );
        assert!(
            statements[1].contains("'hour'") && statements[1].contains("FROM _meter.windows FINAL")
        );
        assert!(statements[2].contains("'day'"));
        assert!(statements[3].starts_with("INSERT INTO _meter.close_runs"));
        assert!(statements[3].contains(&format!("{now_ms}")));
        assert_eq!(
            (
                run.summary().minutes,
                run.summary().hours,
                run.summary().days
            ),
            (2, 1, 1)
        );
        assert_eq!(run.summary().run, now_ms as u64);

        // A later run looks again at events since the last start less 10 minutes.
        let mut later = CloseRun::new(sum_meter(), now_ms + 60_000, CloseConfig::default());
        later.next(None).expect("ok");
        let touched = later
            .next(Some("1792404030\t1792403880\n"))
            .expect("ok")
            .expect("sql");
        assert!(
            touched.contains("ingested_at >= toDateTime(1792403430, 'UTC')"),
            "{touched}"
        );
        // Nothing touched: only the record.
        let record = later.next(Some("")).expect("ok").expect("sql");
        assert!(record.starts_with("INSERT INTO _meter.close_runs"));
        assert_eq!(later.next(Some("")).expect("ok"), None);
    }

    #[test]
    fn non_additive_rollups_read_raw_events() {
        let mut m = sum_meter();
        m.aggregation = Aggregation::UniqueCount;
        m.value_property = Some("$.user".to_string());
        let sql = close_rollup(&m, WindowSize::Hour, &[3_600], 10_000, 7);
        assert!(sql.contains("FROM _meter.events FINAL"), "{sql}");
        assert!(sql.contains("uniqExactIf"));
        assert!(sql.contains(&format!("time < {}", at64(7_200))));
        let sql = close_rollup(&sum_meter(), WindowSize::Hour, &[3_600], 5_000, 7);
        assert!(sql.contains("FROM _meter.windows FINAL"));
        // An hour still open is closed only up to `close_to`.
        assert!(sql.contains(&format!("window_start < {}", at(5_000))));
    }

    #[test]
    fn bad_answers_are_errors() {
        let mut run = CloseRun::new(sum_meter(), 0, CloseConfig::default());
        run.next(None).expect("ok");
        assert!(run.next(Some("yesterday\t1\n")).is_err());
    }
}
