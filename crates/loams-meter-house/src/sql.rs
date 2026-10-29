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
//! | `_meter.period_totals` | `ReplacingMergeTree(close_run)` | per (subject, feature, period) the rated total (Task 7) |
//!
//! [`QueryRun`] answers a meter query from these tables (Task 8).

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
        create_period_totals(),
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

// ---------------------------------------------------------------------------
// Period totals and period close (§63 §8.3, §8.4; ME1 Task 7).

/// `_meter.period_totals` (§63 §8.4): per (subject, feature, period) the
/// value that rates the period. `closed = 1` once the period closed (2 hours
/// after its end); before that the row is provisional. Versioned by
/// `close_run`, so a re-close after a late event replaces the row.
pub fn create_period_totals() -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS {DATABASE}.period_totals (\
         subject String, \
         feature String, \
         meter LowCardinality(String), \
         period_start DateTime('UTC'), \
         period_end DateTime('UTC'), \
         value Decimal(38, 9), \
         events UInt64, \
         closed UInt8, \
         close_run UInt64) \
         ENGINE = ReplacingMergeTree(close_run) \
         PARTITION BY toYYYYMM(period_start) \
         ORDER BY (subject, feature, period_start)"
    )
}

/// A feature as the period totals need it: the std-only mirror of
/// `loams_meter_core::Feature` with its meter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureDef {
    /// The feature key.
    pub key: String,
    /// The meter it counts.
    pub meter: MeterDef,
    /// Group-by values selecting part of the meter (`model = gpt-x`), sorted.
    pub filter: Vec<(String, String)>,
    /// A gauge billed as byte-hours: the sum of the hourly `LATEST` values
    /// (§63 §8.3) instead of the meter's aggregation over the period.
    pub byte_hours: bool,
}

/// The filter of minute or hour windows by the feature's group-by values.
fn window_filter(feature: &FeatureDef) -> String {
    feature
        .filter
        .iter()
        .map(|(k, v)| {
            format!(
                " AND JSONExtractString(group_key, {}) = {}",
                quote(k),
                quote(v)
            )
        })
        .collect()
}

/// The filter of raw events by the feature's group-by values, through the
/// meter's paths.
pub fn event_filter(feature: &FeatureDef) -> String {
    feature
        .filter
        .iter()
        .map(|(k, v)| {
            let path = feature
                .meter
                .group_by
                .iter()
                .find(|(name, _)| name == k)
                .map(|(_, path)| path.as_str())
                .unwrap_or("$.__loams_no_such_dimension");
            format!(" AND JSON_VALUE(data, {}) = {}", quote(path), quote(v))
        })
        .collect()
}

/// Computes `subject`'s total of `feature` over `[start, end)` (Unix
/// seconds) and writes it with `closed` and version `run`.
///
/// * Byte-hours: the sum of the hour windows starting in the period.
/// * `SUM`, `COUNT`: the sum of the minute windows starting in the period;
///   `MIN`, `MAX`: their minimum or maximum.
/// * `UNIQUE_COUNT`, `AVG`, `LATEST`: recomputed from the raw events in the
///   period (they do not roll up).
///
/// Windows count at minute resolution: a minute window belongs to the
/// period its start is in (ruling T7-1).
pub fn period_total(
    subject: &str,
    feature: &FeatureDef,
    start: i64,
    end: i64,
    closed: bool,
    run: u64,
) -> String {
    let meter = &feature.meter;
    let head = format!(
        "INSERT INTO {DATABASE}.period_totals \
         (subject, feature, meter, period_start, period_end, value, events, closed, close_run) \
         SELECT {subject}, {key}, {slug}, {start_at}, {end_at}",
        subject = quote(subject),
        key = quote(&feature.key),
        slug = quote(&meter.slug),
        start_at = at(start),
        end_at = at(end),
    );
    let closed = u8::from(closed);
    if feature.byte_hours || meter.aggregation.rolls_up() {
        let (size, agg) = if feature.byte_hours {
            ("hour", "sum(value)")
        } else {
            ("minute", rollup_agg_expr(meter.aggregation))
        };
        format!(
            "{head}, ifNull({agg}, toDecimal128(0, 9)), sum(events), {closed}, {run} \
             FROM {DATABASE}.windows FINAL \
             WHERE meter = {slug} AND subject = {subject} AND window_size = '{size}' \
             AND window_start >= {lo} AND window_start < {hi}{filter}",
            slug = quote(&meter.slug),
            subject = quote(subject),
            lo = at(start),
            hi = at(end),
            filter = window_filter(feature),
        )
    } else {
        format!(
            "{head}, {agg}, count(), {closed}, {run} \
             FROM {DATABASE}.events FINAL \
             WHERE subject = {subject} AND {events} AND time >= {lo} AND time < {hi}{filter}",
            agg = raw_agg_expr(meter),
            subject = quote(subject),
            events = events_filter(meter),
            lo = at64(start),
            hi = at64(end),
            filter = event_filter(feature),
        )
    }
}

/// The stored total of a period: `closed\tclose_run`, or nothing.
pub fn period_state(subject: &str, feature: &str, start: i64) -> String {
    format!(
        "SELECT closed, close_run FROM {DATABASE}.period_totals FINAL \
         WHERE subject = {} AND feature = {} AND period_start = {} \
         ORDER BY close_run DESC LIMIT 1",
        quote(subject),
        quote(feature),
        at(start)
    )
}

/// The newest version of the meter's minute windows of `subject` in
/// `[start, end)`: newer than the closed total means a late event
/// re-closed one of them.
pub fn windows_version(subject: &str, slug: &str, start: i64, end: i64) -> String {
    format!(
        "SELECT max(close_run) FROM {DATABASE}.windows FINAL \
         WHERE meter = {} AND subject = {} AND window_size = 'minute' \
         AND window_start >= {} AND window_start < {}",
        quote(slug),
        quote(subject),
        at(start),
        at(end)
    )
}

/// How periods close (§63 §8.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeriodConfig {
    /// A period closes this long after its end (2 hours: a close-job lap,
    /// the minute grace and margin).
    pub grace_secs: i64,
}

impl Default for PeriodConfig {
    fn default() -> Self {
        PeriodConfig { grace_secs: 7_200 }
    }
}

/// What happened to one period in a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeriodOutcome {
    /// Not due yet: its provisional total was written.
    Provisional,
    /// Due and closed by this run.
    Closed,
    /// Closed before, and closed again: a late event re-closed a window in
    /// it.
    Reclosed,
    /// Closed before and unchanged.
    AlreadyClosed,
}

/// What a period run did, per period (`start`, `end`, outcome).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeriodSummary {
    pub run: u64,
    pub periods: Vec<(i64, i64, PeriodOutcome)>,
}

#[derive(Debug)]
enum PeriodState {
    Next,
    Stored,
    Windows { stored: Option<(bool, u64)> },
    Done,
}

/// The period totals of one (subject, feature), as a sans-I/O machine like
/// [`CloseRun`]. It is given the periods to look at (from the entitlement's
/// usage period, with the customer's anchor: the current one and enough
/// earlier ones to reach a closed one). For each:
///
/// * not due (`now < end + grace`): writes the provisional total
///   (`closed = 0`);
/// * due and never closed: writes the closed total (`closed = 1`);
/// * due and closed, but a minute window in it was re-closed since: writes
///   the closed total again with a newer version (an exported period's
///   exporter sends an adjustment, §63 §16);
/// * due, closed and unchanged: nothing.
#[derive(Debug)]
pub struct PeriodRun {
    subject: String,
    feature: FeatureDef,
    periods: std::collections::VecDeque<(i64, i64)>,
    current: Option<(i64, i64)>,
    now: i64,
    config: PeriodConfig,
    summary: PeriodSummary,
    state: PeriodState,
}

impl PeriodRun {
    /// A run at `now_ms` (Unix milliseconds; also the run's version) over
    /// `periods` (`[start, end)` in Unix seconds).
    pub fn new(
        subject: impl Into<String>,
        feature: FeatureDef,
        periods: Vec<(i64, i64)>,
        now_ms: i64,
        config: PeriodConfig,
    ) -> Self {
        PeriodRun {
            subject: subject.into(),
            feature,
            periods: periods.into(),
            current: None,
            now: now_ms.div_euclid(1_000),
            config,
            summary: PeriodSummary {
                run: now_ms.max(0) as u64,
                periods: Vec::new(),
            },
            state: PeriodState::Next,
        }
    }

    /// What the run did (complete once [`PeriodRun::next`] returned `None`).
    pub fn summary(&self) -> &PeriodSummary {
        &self.summary
    }

    fn record(&mut self, outcome: PeriodOutcome) {
        if let Some((start, end)) = self.current {
            self.summary.periods.push((start, end, outcome));
        }
    }

    fn total(&self, closed: bool) -> Option<String> {
        self.current.map(|(start, end)| {
            period_total(
                &self.subject,
                &self.feature,
                start,
                end,
                closed,
                self.summary.run,
            )
        })
    }

    /// The next statement, given the previous one's answer (`None` at the
    /// start); `Ok(None)` once the run is done.
    pub fn next(&mut self, answer: Option<&str>) -> Result<Option<String>, String> {
        loop {
            match std::mem::replace(&mut self.state, PeriodState::Done) {
                PeriodState::Next => {
                    let Some((start, end)) = self.periods.pop_front() else {
                        return Ok(None);
                    };
                    self.current = Some((start, end));
                    if self.now < end + self.config.grace_secs {
                        self.record(PeriodOutcome::Provisional);
                        self.state = PeriodState::Next;
                        return Ok(self.total(false));
                    }
                    self.state = PeriodState::Stored;
                    return Ok(Some(period_state(&self.subject, &self.feature.key, start)));
                }
                PeriodState::Stored => {
                    let line = answer.unwrap_or("").lines().next().unwrap_or("").trim();
                    let stored = if line.is_empty() {
                        None
                    } else {
                        let mut fields = line.split('\t');
                        let closed = parse_i64(fields.next().unwrap_or(""))? != 0;
                        let run = parse_i64(fields.next().unwrap_or(""))?;
                        Some((closed, run.max(0) as u64))
                    };
                    let (start, end) = self.current.unwrap_or((0, 0));
                    self.state = PeriodState::Windows { stored };
                    return Ok(Some(windows_version(
                        &self.subject,
                        &self.feature.meter.slug,
                        start,
                        end,
                    )));
                }
                PeriodState::Windows { stored } => {
                    let text = answer.unwrap_or("").trim();
                    let windows = if text.is_empty() || text == "\\N" {
                        0
                    } else {
                        parse_i64(text)?.max(0) as u64
                    };
                    self.state = PeriodState::Next;
                    match stored {
                        Some((true, run)) if windows <= run => {
                            self.record(PeriodOutcome::AlreadyClosed);
                        }
                        Some((true, _)) => {
                            self.record(PeriodOutcome::Reclosed);
                            return Ok(self.total(true));
                        }
                        _ => {
                            self.record(PeriodOutcome::Closed);
                            return Ok(self.total(true));
                        }
                    }
                }
                PeriodState::Done => return Ok(None),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The meter query (§63 §8.5; ME1 Task 8).

/// The windows of a meter query (OpenMeter's `windowSize`, or none: one
/// window over the whole range).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryWindow {
    Minute,
    Hour,
    Day,
    Month,
    Whole,
}

impl QueryWindow {
    /// The window start (Unix seconds) of time column `column`, in UTC.
    fn start(self, column: &str, from: i64) -> String {
        match self {
            Self::Minute => format!("toUnixTimestamp(toStartOfMinute({column}))"),
            Self::Hour => format!("toUnixTimestamp(toStartOfHour({column}))"),
            Self::Day => format!("toUnixTimestamp(toStartOfDay({column}))"),
            Self::Month => {
                format!("toUnixTimestamp(toDateTime(toStartOfMonth({column}), 'UTC'))")
            }
            Self::Whole => from.to_string(),
        }
    }

    /// The window end (Unix seconds) of time column `column`.
    fn end(self, column: &str, to: i64) -> String {
        match self {
            Self::Minute => format!("toUnixTimestamp(toStartOfMinute({column})) + 60"),
            Self::Hour => format!("toUnixTimestamp(toStartOfHour({column})) + 3600"),
            Self::Day => format!("toUnixTimestamp(toStartOfDay({column})) + 86400"),
            Self::Month => format!(
                "toUnixTimestamp(toDateTime(addMonths(toStartOfMonth({column}), 1), 'UTC'))"
            ),
            Self::Whole => to.to_string(),
        }
    }
}

/// A meter query, as the SQL needs it (`loams_meter_house::query` parses
/// OpenMeter's parameters into it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuerySpec {
    /// `[from, to)`, Unix seconds.
    pub from: i64,
    pub to: i64,
    pub window: QueryWindow,
    /// Only these subjects (all when empty).
    pub subjects: Vec<String>,
    /// Whether rows are per subject (`groupBy=subject`, or a subject filter).
    pub by_subject: bool,
    /// The meter's dimensions the rows are grouped by.
    pub group_by: Vec<String>,
    /// Dimension filters: each dimension one of its values.
    pub filters: Vec<(String, Vec<String>)>,
}

/// One row of a meter query's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryRow {
    pub window_start: i64,
    pub window_end: i64,
    pub subject: Option<String>,
    /// The `group_by` dimensions' values, in [`QuerySpec::group_by`] order.
    pub group: Vec<String>,
    /// The value × 10^9 (`Decimal(38, 9)`).
    pub value: i128,
    /// The window reaches past the latest closed minute: its value may
    /// still change (§63 §8.5).
    pub provisional: bool,
}

/// A query's answer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueryAnswer {
    /// The latest closed minute (Unix seconds; the meter's `closed_to`), or
    /// `None` before the meter's first close run.
    pub as_of: Option<i64>,
    pub rows: Vec<QueryRow>,
}

/// A `Decimal(38, 9)` as ClickHouse prints it (`12`, `-0.5`), × 10^9.
pub fn parse_decimal9(text: &str) -> Result<i128, String> {
    let text = text.trim();
    let bad = || format!("not a decimal: {text:?}");
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    if whole.is_empty() && fraction.is_empty()
        || fraction.len() > 9
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(bad());
    }
    let whole: i128 = if whole.is_empty() {
        0
    } else {
        whole.parse().map_err(|_| bad())?
    };
    let mut frac: i128 = if fraction.is_empty() {
        0
    } else {
        fraction.parse().map_err(|_| bad())?
    };
    for _ in fraction.len()..9 {
        frac *= 10;
    }
    let value = whole
        .checked_mul(1_000_000_000)
        .and_then(|w| w.checked_add(frac))
        .ok_or_else(bad)?;
    Ok(if negative { -value } else { value })
}

/// A `TabSeparated` field, unescaped.
pub fn unescape_tsv(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    let mut chars = field.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some('b') => out.push('\u{8}'),
            Some('f') => out.push('\u{c}'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

fn in_list(values: &[String]) -> String {
    values
        .iter()
        .map(|v| quote(v))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The closed part of a query whose aggregation rolls up: the minute
/// windows in `[from, until)`, combined into the query's windows.
pub fn query_closed(meter: &MeterDef, spec: &QuerySpec, until: i64) -> String {
    let mut select = vec![
        format!("{} AS w", spec.window.start("window_start", spec.from)),
        format!("{} AS wend", spec.window.end("window_start", spec.to)),
    ];
    let mut group = vec!["w".to_string(), "wend".to_string()];
    if spec.by_subject {
        select.push("subject".to_string());
        group.push("subject".to_string());
    }
    for (i, dim) in spec.group_by.iter().enumerate() {
        select.push(format!(
            "JSONExtractString(group_key, {}) AS g{i}",
            quote(dim)
        ));
        group.push(format!("g{i}"));
    }
    select.push(rollup_agg_expr(meter.aggregation).to_string());
    let mut filter = format!(
        "meter = {} AND window_size = 'minute' AND window_start >= {} AND window_start < {}",
        quote(&meter.slug),
        at(spec.from),
        at(until)
    );
    if !spec.subjects.is_empty() {
        filter.push_str(&format!(" AND subject IN ({})", in_list(&spec.subjects)));
    }
    for (dim, values) in &spec.filters {
        filter.push_str(&format!(
            " AND JSONExtractString(group_key, {}) IN ({})",
            quote(dim),
            in_list(values)
        ));
    }
    format!(
        "SELECT {} FROM {DATABASE}.windows FINAL WHERE {filter} GROUP BY {} ORDER BY {}",
        select.join(", "),
        group.join(", "),
        group.join(", ")
    )
}

/// The raw part of a query: events in `[since, to)`, aggregated into the
/// query's windows.
pub fn query_raw(meter: &MeterDef, spec: &QuerySpec, since: i64) -> String {
    let path_of = |dim: &str| {
        meter
            .group_by
            .iter()
            .find(|(name, _)| name == dim)
            .map(|(_, path)| path.clone())
            .unwrap_or_else(|| "$.__loams_no_such_dimension".to_string())
    };
    let mut select = vec![
        format!("{} AS w", spec.window.start("time", spec.from)),
        format!("{} AS wend", spec.window.end("time", spec.to)),
    ];
    let mut group = vec!["w".to_string(), "wend".to_string()];
    if spec.by_subject {
        select.push("subject".to_string());
        group.push("subject".to_string());
    }
    for (i, dim) in spec.group_by.iter().enumerate() {
        select.push(format!(
            "JSON_VALUE(data, {}) AS g{i}",
            quote(&path_of(dim))
        ));
        group.push(format!("g{i}"));
    }
    select.push(raw_agg_expr(meter));
    let mut filter = format!(
        "{} AND time >= {} AND time < {}",
        events_filter(meter),
        at64(since),
        at64(spec.to)
    );
    if !spec.subjects.is_empty() {
        filter.push_str(&format!(" AND subject IN ({})", in_list(&spec.subjects)));
    }
    for (dim, values) in &spec.filters {
        filter.push_str(&format!(
            " AND JSON_VALUE(data, {}) IN ({})",
            quote(&path_of(dim)),
            in_list(values)
        ));
    }
    format!(
        "SELECT {} FROM {DATABASE}.events FINAL WHERE {filter} GROUP BY {} ORDER BY {}",
        select.join(", "),
        group.join(", "),
        group.join(", ")
    )
}

#[derive(Debug)]
enum QueryState {
    Start,
    AsOf,
    ClosedRows { raw_from: i64 },
    RawRows,
    Done,
}

/// A meter query (§63 §8.5) as a sans-I/O machine like [`CloseRun`]:
///
/// 1. reads the meter's latest closed minute (`as_of`, its last close run's
///    `closed_to`);
/// 2. for an aggregation that rolls up, reads `[from, as_of)` from the closed
///    minute windows (`_meter.windows FINAL`) and `[as_of, to)` from the raw
///    events, and combines the two in the windows that straddle `as_of`;
///    `UNIQUE_COUNT`, `AVG` and `LATEST` do not combine, so they read the
///    raw events over the whole range (ruling T8-1);
/// 3. marks every window that ends after `as_of` provisional.
#[derive(Debug)]
pub struct QueryRun {
    meter: MeterDef,
    spec: QuerySpec,
    state: QueryState,
    answer: QueryAnswer,
}

impl QueryRun {
    pub fn new(meter: MeterDef, spec: QuerySpec) -> Self {
        QueryRun {
            meter,
            spec,
            state: QueryState::Start,
            answer: QueryAnswer::default(),
        }
    }

    /// The answer (complete once [`QueryRun::next`] returned `None`).
    pub fn answer(&self) -> &QueryAnswer {
        &self.answer
    }

    /// Consumes the run for its answer.
    pub fn into_answer(self) -> QueryAnswer {
        self.answer
    }

    fn rows(&mut self, text: &str) -> Result<(), String> {
        let dims = self.spec.group_by.len();
        let width = 3 + usize::from(self.spec.by_subject) + dims;
        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() != width {
                return Err(format!(
                    "expected {width} columns, not {}: {line:?}",
                    fields.len()
                ));
            }
            let mut i = 0;
            let mut next = || {
                i += 1;
                fields[i - 1]
            };
            let window_start = parse_i64(next())?;
            let window_end = parse_i64(next())?;
            let subject = if self.spec.by_subject {
                Some(unescape_tsv(next()))
            } else {
                None
            };
            let group: Vec<String> = (0..dims).map(|_| unescape_tsv(next())).collect();
            let value = parse_decimal9(next())?;
            let row = QueryRow {
                window_start,
                window_end,
                subject,
                group,
                value,
                provisional: false,
            };
            self.merge(row);
        }
        Ok(())
    }

    fn merge(&mut self, row: QueryRow) {
        let found = self.answer.rows.iter_mut().find(|r| {
            r.window_start == row.window_start && r.subject == row.subject && r.group == row.group
        });
        match found {
            Some(existing) => {
                existing.value = match self.meter.aggregation {
                    Aggregation::Min => existing.value.min(row.value),
                    Aggregation::Max => existing.value.max(row.value),
                    _ => existing.value.saturating_add(row.value),
                };
            }
            None => self.answer.rows.push(row),
        }
    }

    fn finish(&mut self) {
        let as_of = self.answer.as_of;
        for row in &mut self.answer.rows {
            row.provisional = as_of.is_none_or(|a| row.window_end > a);
        }
        self.answer.rows.sort_by(|a, b| {
            (a.window_start, &a.subject, &a.group).cmp(&(b.window_start, &b.subject, &b.group))
        });
    }

    /// The next statement, given the previous one's answer (`None` at the
    /// start); `Ok(None)` once the answer is complete.
    pub fn next(&mut self, answer: Option<&str>) -> Result<Option<String>, String> {
        match std::mem::replace(&mut self.state, QueryState::Done) {
            QueryState::Start => {
                self.state = QueryState::AsOf;
                Ok(Some(last_close_run(&self.meter.slug)))
            }
            QueryState::AsOf => {
                let line = answer.unwrap_or("").lines().next().unwrap_or("").trim();
                self.answer.as_of = if line.is_empty() {
                    None
                } else {
                    Some(parse_i64(line.split('\t').nth(1).unwrap_or(""))?)
                };
                match self.answer.as_of {
                    Some(as_of) if self.meter.aggregation.rolls_up() && as_of > self.spec.from => {
                        let until = as_of.min(self.spec.to);
                        self.state = QueryState::ClosedRows { raw_from: until };
                        Ok(Some(query_closed(&self.meter, &self.spec, until)))
                    }
                    _ => {
                        self.state = QueryState::RawRows;
                        Ok(Some(query_raw(&self.meter, &self.spec, self.spec.from)))
                    }
                }
            }
            QueryState::ClosedRows { raw_from } => {
                self.rows(answer.unwrap_or(""))?;
                if raw_from < self.spec.to {
                    self.state = QueryState::RawRows;
                    Ok(Some(query_raw(&self.meter, &self.spec, raw_from)))
                } else {
                    self.finish();
                    Ok(None)
                }
            }
            QueryState::RawRows => {
                self.rows(answer.unwrap_or(""))?;
                self.finish();
                Ok(None)
            }
            QueryState::Done => Ok(None),
        }
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
        let m1 = 1_792_400_040; // 08:54, on a minute
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

#[cfg(test)]
mod period_tests {
    use super::*;

    fn feature(aggregation: Aggregation, byte_hours: bool) -> FeatureDef {
        FeatureDef {
            key: "house_cpu".to_string(),
            meter: MeterDef {
                slug: "house_cpu_ms".to_string(),
                event_type: "io.loams.dev.usage.house.query.v1".to_string(),
                aggregation,
                value_property: Some("$.cpu_ms".to_string()),
                group_by: vec![("namespace".to_string(), "$.namespace".to_string())],
                event_from: None,
            },
            filter: vec![("namespace".to_string(), "7".to_string())],
            byte_hours,
        }
    }

    #[test]
    fn totals_read_the_right_source() {
        let sum = period_total(
            "org_42",
            &feature(Aggregation::Sum, false),
            0,
            3_600,
            true,
            9,
        );
        assert!(sum.contains("FROM _meter.windows FINAL"), "{sum}");
        assert!(sum.contains("window_size = 'minute'"));
        assert!(sum.contains("JSONExtractString(group_key, 'namespace') = '7'"));
        assert!(sum.contains(", 1, 9 FROM"));
        let gauge = period_total(
            "org_42",
            &feature(Aggregation::Latest, true),
            0,
            3_600,
            false,
            9,
        );
        assert!(gauge.contains("window_size = 'hour'") && gauge.contains("sum(value)"));
        assert!(gauge.contains(", 0, 9 FROM"));
        let unique = period_total(
            "org_42",
            &feature(Aggregation::UniqueCount, false),
            0,
            3_600,
            true,
            9,
        );
        assert!(unique.contains("FROM _meter.events FINAL"), "{unique}");
        assert!(unique.contains("JSON_VALUE(data, '$.namespace') = '7'"));
    }

    /// `period_closes_after_grace`, the machine's half: a period is
    /// provisional until 2 hours after its end, then closed once, then
    /// closed again only when a window in it was re-closed.
    #[test]
    fn period_machine_closes_after_grace() {
        let f = feature(Aggregation::Sum, false);
        let periods = vec![(3_600, 7_200)];
        let drive = |now_ms: i64, stored: &str, windows: &str| {
            let mut run = PeriodRun::new(
                "org_42",
                f.clone(),
                periods.clone(),
                now_ms,
                PeriodConfig::default(),
            );
            let mut statements = Vec::new();
            let mut answer: Option<String> = None;
            while let Some(sql) = run.next(answer.as_deref()).expect("ok") {
                answer = Some(if sql.contains("FROM _meter.period_totals FINAL") {
                    stored.to_string()
                } else if sql.contains("max(close_run)") {
                    windows.to_string()
                } else {
                    String::new()
                });
                statements.push(sql);
            }
            (run.summary().periods.clone(), statements)
        };
        // An hour before the grace ends: provisional, no reads.
        let (outcome, statements) = drive((7_200 + 7_199) * 1_000, "", "");
        assert_eq!(outcome, [(3_600, 7_200, PeriodOutcome::Provisional)]);
        assert_eq!(statements.len(), 1);
        assert!(statements[0].contains(", 0, "));
        // At end + 2 h: closed.
        let (outcome, statements) = drive((7_200 + 7_200) * 1_000, "", "5000\n");
        assert_eq!(outcome, [(3_600, 7_200, PeriodOutcome::Closed)]);
        assert_eq!(statements.len(), 3);
        assert!(
            statements[2].contains(", 1, 14400000 FROM"),
            "{}",
            statements[2]
        );
        // Closed at 14400000 with nothing newer: unchanged.
        let (outcome, statements) = drive(15_000_000, "1\t14400000\n", "5000\n");
        assert_eq!(outcome, [(3_600, 7_200, PeriodOutcome::AlreadyClosed)]);
        assert_eq!(statements.len(), 2);
        // A minute window re-closed after the period closed: closed again.
        let (outcome, statements) = drive(16_000_000, "1\t14400000\n", "15500000\n");
        assert_eq!(outcome, [(3_600, 7_200, PeriodOutcome::Reclosed)]);
        assert!(statements[2].contains(", 1, 16000000 FROM"));
        // A provisional row of a due period is closed.
        let (outcome, _) = drive(16_000_000, "0\t9000000\n", "0\n");
        assert_eq!(outcome, [(3_600, 7_200, PeriodOutcome::Closed)]);
    }
}

#[cfg(test)]
mod query_tests {
    use super::*;

    fn meter(aggregation: Aggregation) -> MeterDef {
        MeterDef {
            slug: "house_cpu_ms".to_string(),
            event_type: "io.loams.dev.usage.house.query.v1".to_string(),
            aggregation,
            value_property: Some("$.cpu_ms".to_string()),
            group_by: vec![("namespace".to_string(), "$.namespace".to_string())],
            event_from: None,
        }
    }

    fn spec(window: QueryWindow) -> QuerySpec {
        QuerySpec {
            from: 3_600,
            to: 7_200,
            window,
            subjects: vec!["org_42".to_string()],
            by_subject: true,
            group_by: vec!["namespace".to_string()],
            filters: vec![("namespace".to_string(), vec!["7".to_string()])],
        }
    }

    #[test]
    fn decimals_and_fields() {
        assert_eq!(parse_decimal9("12"), Ok(12_000_000_000));
        assert_eq!(parse_decimal9("-0.5"), Ok(-500_000_000));
        assert_eq!(parse_decimal9("0.000000001"), Ok(1));
        assert!(parse_decimal9("1e3").is_err());
        assert!(parse_decimal9("").is_err());
        assert!(parse_decimal9("0.0000000001").is_err());
        assert_eq!(unescape_tsv("a\\tb\\\\c"), "a\tb\\c");
    }

    #[test]
    fn closed_and_raw_sql() {
        let closed = query_closed(&meter(Aggregation::Sum), &spec(QueryWindow::Minute), 5_000);
        assert!(closed.contains("FROM _meter.windows FINAL"), "{closed}");
        assert!(closed.contains("window_size = 'minute'"));
        assert!(closed.contains(&format!("window_start < {}", at(5_000))));
        assert!(closed.contains("subject IN ('org_42')"));
        assert!(closed.contains("JSONExtractString(group_key, 'namespace') IN ('7')"));
        assert!(closed.contains("sum(value)"));
        let raw = query_raw(&meter(Aggregation::Sum), &spec(QueryWindow::Month), 5_000);
        assert!(raw.contains("FROM _meter.events FINAL"), "{raw}");
        assert!(raw.contains(&format!("time >= {}", at64(5_000))));
        assert!(raw.contains("JSON_VALUE(data, '$.namespace') IN ('7')"));
        assert!(raw.contains("toStartOfMonth(time)"));
        let whole = query_raw(&meter(Aggregation::Sum), &spec(QueryWindow::Whole), 3_600);
        assert!(
            whole.starts_with("SELECT 3600 AS w, 7200 AS wend"),
            "{whole}"
        );
    }

    /// `query_marks_provisional_open_window`, the machine's half: the closed
    /// minutes and the raw tail combine in the window that straddles
    /// `as_of`, which is provisional.
    #[test]
    fn query_machine_combines_and_marks_provisional() {
        let mut run = QueryRun::new(meter(Aggregation::Sum), spec(QueryWindow::Hour));
        let first = run.next(None).expect("ok").expect("as_of");
        assert!(first.contains("_meter.close_runs"));
        // Closed up to 6000 (01:40).
        let closed = run.next(Some("10\t6000\n")).expect("ok").expect("closed");
        assert!(closed.contains("_meter.windows"));
        let raw = run
            .next(Some("3600\t7200\torg_42\t7\t1840\n"))
            .expect("ok")
            .expect("raw");
        assert!(raw.contains(&format!("time >= {}", at64(6_000))), "{raw}");
        assert_eq!(
            run.next(Some("3600\t7200\torg_42\t7\t999.5\n"))
                .expect("ok"),
            None
        );
        let answer = run.into_answer();
        assert_eq!(answer.as_of, Some(6_000));
        assert_eq!(
            answer.rows,
            [QueryRow {
                window_start: 3_600,
                window_end: 7_200,
                subject: Some("org_42".to_string()),
                group: vec!["7".to_string()],
                value: 2_839_500_000_000,
                provisional: true,
            }]
        );
    }

    #[test]
    fn non_additive_and_unclosed_meters_read_raw_events_only() {
        let mut run = QueryRun::new(meter(Aggregation::UniqueCount), spec(QueryWindow::Hour));
        run.next(None).expect("ok");
        let raw = run.next(Some("10\t6000\n")).expect("ok").expect("raw");
        assert!(raw.contains("_meter.events") && raw.contains(&format!("time >= {}", at64(3_600))));
        assert_eq!(run.next(Some("")).expect("ok"), None);
        // No close run yet: raw, and everything provisional.
        let mut run = QueryRun::new(meter(Aggregation::Sum), spec(QueryWindow::Minute));
        run.next(None).expect("ok");
        run.next(Some("")).expect("ok").expect("raw");
        assert_eq!(
            run.next(Some("3600\t3660\torg_42\t7\t5\n")).expect("ok"),
            None
        );
        assert_eq!(run.answer().as_of, None);
        assert!(run.answer().rows[0].provisional);
        // A malformed row is an error.
        let mut run = QueryRun::new(meter(Aggregation::Sum), spec(QueryWindow::Minute));
        run.next(None).expect("ok");
        run.next(Some("")).expect("ok");
        assert!(run.next(Some("3600\t3660\n")).is_err());
    }
}
