//! Settings on the Rust engine (plan HE1 Task 37, D1805).
//!
//! Every setting the House front accepts (`loams_house_ipc::policy`'s lists,
//! the front's `SATISFIED_SETTINGS`, `FRONT_SETTINGS` and Loams's own, and any
//! of chDB's settings at its default, R9.11) has a class in [`TABLE`]
//! (`settings.tsv`):
//!
//! * **honoured**: the engine behaves as ClickHouse does ([`Settings`] carries
//!   the value to whatever reads it);
//! * **noop**: performance, inserts or the front's own business, so the answer
//!   is the same at any value; the reason is in the table;
//! * **default_only**: it changes answers and the task that honours it has not
//!   landed, so any other value is `48` naming that task, but only where the
//!   statement has the construct it changes (`join_use_nulls = 1` without an
//!   outer join asks for nothing);
//! * **refused**: `164` at any value but its default.
//!
//! A name that is not in the table is chDB's rule: accepted at its default,
//! `164` at any other value, `115` when ClickHouse has no such setting. A
//! setting that changes answers is never ignored.
//!
//! Later tasks read the value from [`Settings`]: `join_use_nulls` (Task 35),
//! `final_` (Task 27), [`Settings::timezone`] (Task 30) and
//! [`Settings::text`] (Task 36).

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use datafusion::arrow::array::RecordBatch;
use datafusion::arrow::datatypes::DataType;
use datafusion::common::tree_node::{TreeNode, TreeNodeRecursion};
use datafusion::execution::runtime_env::{RuntimeEnv, RuntimeEnvBuilder};
use datafusion::logical_expr::expr::{AggregateFunction, Cast, TryCast};
use datafusion::logical_expr::{Expr, JoinType, LogicalPlan};
use datafusion::prelude::{SessionConfig, SessionContext};
use datafusion::sql::sqlparser::dialect::ClickHouseDialect;
use datafusion::sql::sqlparser::keywords::Keyword;
use datafusion::sql::sqlparser::tokenizer::{Token, Tokenizer};
use loams_house_ipc::policy::{chdb_default, same_setting_value};
use loams_house_sql::Resolution;
use loams_house_types::ChError;
use loams_house_types::text::{DateTimeOutput, TextSettings};

/// The table: one row per setting or prefix (see its header).
pub const TABLE: &str = include_str!("settings.tsv");

/// The server's zone, for zone-less `DateTime` values when `session_timezone`
/// is empty (the oracle and the House run in UTC).
pub const SERVER_TIMEZONE: &str = "UTC";

/// What the engine does with a setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// The engine behaves as ClickHouse does.
    Honoured,
    /// The answer is the same at any value.
    NoOp,
    /// Changes answers; any value but the default is `48` where it applies.
    DefaultOnly,
    /// `164` at any value but the default.
    Refused,
}

/// The construct a setting changes: where a value other than its default
/// matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Nowhere the engine has not honoured.
    Never,
    /// Every statement.
    Always,
    /// `LEFT`, `RIGHT` and `FULL` joins.
    OuterJoin,
    /// `ROLLUP`, `CUBE` and `GROUPING SETS`.
    GroupingSets,
    /// Aggregates and window functions.
    Aggregate,
    /// `COUNT(DISTINCT …)` and `countDistinct`.
    CountDistinct,
    /// A `UNION` with neither `ALL` nor `DISTINCT`.
    BareUnion,
    /// Reads of merging `MergeTree` engines (`ReplacingMergeTree`, …).
    MergeTables,
    /// Conversions, comparisons and functions of dates and times.
    Time,
    /// Query parameters, where they are dates or times.
    TimeParams,
    /// A result limit (`max_result_rows`, `max_result_bytes`) is set.
    ResultLimit,
}

/// One row of [`TABLE`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    /// The name, or the prefix when [`Spec::prefix`].
    pub name: String,
    /// Whether `name` is a prefix (`output_format_*`).
    pub prefix: bool,
    /// The class.
    pub class: Class,
    /// The default: `None` is chDB's for the name ([`chdb_default`]), and
    /// `Some("")` the empty string.
    pub default: Option<String>,
    /// Whether the row has a default at all (`-` in the table: Loams's own).
    pub has_default: bool,
    /// The task that honours it where [`Spec::scope`] applies.
    pub task: Option<u32>,
    /// Where a value other than the default matters.
    pub scope: Scope,
    /// Why.
    pub note: String,
}

fn parse_class(text: &str) -> Option<Class> {
    Some(match text {
        "honoured" => Class::Honoured,
        "noop" => Class::NoOp,
        "default_only" => Class::DefaultOnly,
        "refused" => Class::Refused,
        _ => return None,
    })
}

fn parse_scope(text: &str) -> Option<Scope> {
    Some(match text {
        "never" => Scope::Never,
        "always" => Scope::Always,
        "outer_join" => Scope::OuterJoin,
        "grouping_sets" => Scope::GroupingSets,
        "aggregate" => Scope::Aggregate,
        "count_distinct" => Scope::CountDistinct,
        "bare_union" => Scope::BareUnion,
        "merge_tables" => Scope::MergeTables,
        "time" => Scope::Time,
        "time_params" => Scope::TimeParams,
        "result_limit" => Scope::ResultLimit,
        _ => return None,
    })
}

/// [`TABLE`]'s rows, or the first malformed line.
pub fn parse_table(text: &str) -> Result<Vec<Spec>, String> {
    let mut rows = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let bad = || format!("settings.tsv line {}: {line}", i + 1);
        let cols: Vec<&str> = line.split('\t').collect();
        let [name, class, default, task, scope, note] = cols[..] else {
            return Err(bad());
        };
        let (name, prefix) = match name.strip_suffix('*') {
            Some(p) => (p, true),
            None => (name, false),
        };
        let (default, has_default) = match default {
            "*" => (None, true),
            "-" => (None, false),
            "''" => (Some(String::new()), true),
            d => (Some(d.to_string()), true),
        };
        let task = match task {
            "-" => None,
            t => Some(t.parse().map_err(|_| bad())?),
        };
        rows.push(Spec {
            name: name.to_string(),
            prefix,
            class: parse_class(class).ok_or_else(bad)?,
            default,
            has_default,
            task,
            scope: parse_scope(scope).ok_or_else(bad)?,
            note: note.to_string(),
        });
    }
    Ok(rows)
}

/// [`TABLE`], parsed once. A malformed table is a build defect that the
/// tests catch (`the_table_parses_and_names_known_settings`); it reads as
/// empty here, so every setting is held to chDB's rule rather than accepted.
pub fn table() -> &'static [Spec] {
    static ROWS: OnceLock<Vec<Spec>> = OnceLock::new();
    ROWS.get_or_init(|| parse_table(TABLE).unwrap_or_default())
}

/// How a setting's name is classed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    /// A row of the table, by name or by prefix.
    Spec(&'static Spec),
    /// One of chDB's settings without a row: accepted at its default (R9.11).
    Known(String),
    /// Not a ClickHouse setting.
    Unknown,
}

/// The class of `name`: its own row, else the longest prefix row (for a name
/// ClickHouse knows, or Loams's own), else chDB's rule.
pub fn lookup(name: &str) -> Lookup {
    let rows = table();
    if let Some(s) = rows.iter().find(|s| !s.prefix && s.name == name) {
        return Lookup::Spec(s);
    }
    let known = chdb_default(name);
    let by_prefix = rows
        .iter()
        .filter(|s| s.prefix && name.starts_with(s.name.as_str()))
        .filter(|s| known.is_some() || !s.has_default)
        .max_by_key(|s| s.name.len());
    if let Some(s) = by_prefix {
        return Lookup::Spec(s);
    }
    match known {
        Some(default) => Lookup::Known(default),
        None => Lookup::Unknown,
    }
}

/// `value` without the quotes and spaces a `SETTINGS` clause or a URL may
/// carry.
fn unquote(value: &str) -> &str {
    let v = value.trim();
    v.strip_prefix('\'')
        .and_then(|v| v.strip_suffix('\''))
        .unwrap_or(v)
}

/// Whether `value` is `name`'s default under `spec`.
fn at_default(spec: &Spec, name: &str, value: &str) -> bool {
    if !spec.has_default {
        return false;
    }
    match &spec.default {
        Some(d) => same_setting_value(value, d),
        None => chdb_default(name).is_some_and(|d| same_setting_value(value, &d)),
    }
}

fn cannot_parse(name: &str, value: &str, kind: &str) -> ChError {
    ChError::bad_arguments(format!(
        "Cannot parse '{value}' as the value of setting {name}: expected {kind}"
    ))
}

fn readonly(message: String) -> ChError {
    ChError {
        code: 164,
        name: "READONLY",
        message,
    }
}

/// `159 TIMEOUT_EXCEEDED`.
pub fn timeout_exceeded(elapsed: Duration, limit: Duration) -> ChError {
    ChError {
        code: 159,
        name: "TIMEOUT_EXCEEDED",
        message: format!(
            "Timeout exceeded: elapsed {} seconds, maximum: {}",
            elapsed.as_secs_f64(),
            limit.as_secs_f64()
        ),
    }
}

/// `396 TOO_MANY_ROWS_OR_BYTES`, in ClickHouse's words (`SizeLimits::check`).
fn too_many(what: &str, max: u64, current: u64) -> ChError {
    ChError {
        code: 396,
        name: "TOO_MANY_ROWS_OR_BYTES",
        message: format!("Limit for result exceeded, max {what}: {max}, current {what}: {current}"),
    }
}

/// `241 MEMORY_LIMIT_EXCEEDED`.
pub fn memory_limit_exceeded(detail: &str, limit: u64) -> ChError {
    ChError {
        code: 241,
        name: "MEMORY_LIMIT_EXCEEDED",
        message: format!("Query memory limit exceeded: maximum: {limit} bytes ({detail})"),
    }
}

/// `559 EXPECTED_ALL_OR_DISTINCT`.
fn expected_all_or_distinct() -> ChError {
    ChError {
        code: 559,
        name: "EXPECTED_ALL_OR_DISTINCT",
        message: "Expected ALL or DISTINCT in SelectWithUnion query, because setting \
                  (union_default_mode) is empty"
            .to_string(),
    }
}

/// A boolean setting's value as ClickHouse reads it.
pub fn parse_bool(name: &str, value: &str) -> Result<bool, ChError> {
    match value.to_ascii_lowercase().as_str() {
        "0" | "false" => Ok(false),
        "1" | "true" => Ok(true),
        _ => Err(cannot_parse(name, value, "0, 1, true or false")),
    }
}

/// An unsigned setting, with ClickHouse's size suffixes (`k`, `M`, `G`, `T`,
/// each optionally `i` for powers of 1024), as the front reads it.
pub fn parse_size(name: &str, value: &str) -> Result<u64, ChError> {
    let bad = || cannot_parse(name, value, "an unsigned number such as 1000, 4G or 4Gi");
    let text = value.strip_prefix('+').unwrap_or(value);
    let digits_end = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (digits, suffix) = text.split_at(digits_end);
    if digits.is_empty() {
        return Err(bad());
    }
    let n: u64 = digits.parse().map_err(|_| bad())?;
    let (unit, binary) = match suffix.strip_suffix('i') {
        Some(u) if !u.is_empty() => (u, true),
        _ => (suffix, false),
    };
    let power = match unit {
        "" => 0,
        "k" | "K" => 1,
        "M" => 2,
        "G" => 3,
        "T" => 4,
        _ => return Err(bad()),
    };
    let base: u64 = if binary { 1024 } else { 1000 };
    base.checked_pow(power)
        .and_then(|m| n.checked_mul(m))
        .ok_or_else(bad)
}

/// `max_threads`: `auto`, `auto(n)` and 0 are the cores (`None`).
fn parse_threads(name: &str, value: &str) -> Result<Option<usize>, ChError> {
    if value.starts_with("auto") {
        return Ok(None);
    }
    let n = parse_size(name, value)?;
    Ok(match n {
        0 => None,
        n => Some(usize::try_from(n).unwrap_or(usize::MAX)),
    })
}

/// The settings one statement runs with: the request's, then the statement's
/// own `SETTINGS` (the later value wins), checked against [`TABLE`].
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// Every setting the statement set, by name, at its final value.
    pub values: BTreeMap<String, String>,
    /// `max_execution_time` (`None`: unlimited).
    pub max_execution_time: Option<Duration>,
    /// `max_result_rows` (0: unlimited).
    pub max_result_rows: u64,
    /// `max_result_bytes` (0: unlimited).
    pub max_result_bytes: u64,
    /// `result_overflow_mode = 'break'`.
    pub result_overflow_break: bool,
    /// `max_threads` (`None`: the cores when set to `auto`; see
    /// [`Settings::target_partitions`]).
    pub max_threads: Option<usize>,
    /// Whether `max_threads` was set at all.
    pub max_threads_set: bool,
    /// `max_block_size`, DataFusion's batch size.
    pub max_block_size: usize,
    /// `max_memory_usage` (0: unlimited), DataFusion's memory pool.
    pub max_memory_usage: u64,
    /// `session_timezone` (`None`: the server's).
    pub session_timezone: Option<String>,
    /// `union_default_mode`: `""`, `"ALL"` or `"DISTINCT"`.
    pub union_default_mode: String,
    /// `asterisk_include_materialized_columns`.
    pub asterisk_include_materialized_columns: bool,
    /// `asterisk_include_alias_columns`.
    pub asterisk_include_alias_columns: bool,
    /// `join_use_nulls` (Task 35).
    pub join_use_nulls: bool,
    /// `final` (Task 27).
    pub final_: bool,
    /// `count_distinct_implementation` (Task 29).
    pub count_distinct_implementation: String,
    /// `aggregate_functions_null_for_empty` (Task 28).
    pub aggregate_functions_null_for_empty: bool,
    /// `group_by_use_nulls` (Task 34).
    pub group_by_use_nulls: bool,
    /// `prefer_column_name_to_alias` (Task 25).
    pub prefer_column_name_to_alias: bool,
    /// How values are written (`output_format_*`, `date_time_output_format`,
    /// the session's zone).
    text: TextSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            values: BTreeMap::new(),
            max_execution_time: None,
            max_result_rows: 0,
            max_result_bytes: 0,
            result_overflow_break: false,
            max_threads: None,
            max_threads_set: false,
            max_block_size: 65_409,
            max_memory_usage: 0,
            session_timezone: None,
            union_default_mode: String::new(),
            asterisk_include_materialized_columns: false,
            asterisk_include_alias_columns: false,
            join_use_nulls: false,
            final_: false,
            count_distinct_implementation: "uniqExact".to_string(),
            aggregate_functions_null_for_empty: false,
            group_by_use_nulls: false,
            prefer_column_name_to_alias: false,
            text: TextSettings {
                timezone: SERVER_TIMEZONE.to_string(),
                ..TextSettings::default()
            },
        }
    }
}

/// The `count_distinct_implementation` values ClickHouse accepts.
const COUNT_DISTINCT_FUNCTIONS: &[&str] = &[
    "uniq",
    "uniqCombined",
    "uniqCombined64",
    "uniqHLL12",
    "uniqExact",
    "uniqTheta",
];

impl Settings {
    /// Reads `pairs` in order (a later value wins): `115` for an unknown name,
    /// `164` for a refused value, `36` for a value that does not parse.
    ///
    /// # Errors
    ///
    /// The first setting that is refused or does not parse.
    pub fn resolve<'a>(
        pairs: impl IntoIterator<Item = &'a (String, String)>,
    ) -> Result<Self, ChError> {
        let mut out = Self::default();
        for (name, value) in pairs {
            let value = unquote(value);
            match lookup(name) {
                Lookup::Unknown => {
                    return Err(ChError::unknown_setting(format!(
                        "Unknown setting '{name}'"
                    )));
                }
                Lookup::Known(default) => {
                    if !same_setting_value(value, &default) {
                        return Err(readonly(format!(
                            "Cannot modify '{name}' setting: it is not allowed on the House \
                             (see the surface page)"
                        )));
                    }
                }
                Lookup::Spec(spec) => {
                    if spec.class == Class::Refused && !at_default(spec, name, value) {
                        let default = spec
                            .default
                            .clone()
                            .or_else(|| chdb_default(name))
                            .unwrap_or_default();
                        return Err(readonly(format!(
                            "Setting {name} can only be {default} on the House: {}",
                            spec.note
                        )));
                    }
                    out.apply(name, value)?;
                }
            }
            out.values.insert(name.clone(), value.to_string());
        }
        Ok(out)
    }

    /// Records one setting the engine reads. Returns whether `name` is one.
    ///
    /// # Errors
    ///
    /// `36` when `value` does not parse as the setting's type.
    pub fn apply(&mut self, name: &str, value: &str) -> Result<bool, ChError> {
        match name {
            "max_execution_time" => {
                let seconds = value
                    .parse::<f64>()
                    .ok()
                    .filter(|s| s.is_finite() && *s >= 0.0)
                    .ok_or_else(|| cannot_parse(name, value, "a number of seconds"))?;
                // ClickHouse keeps whole microseconds; what truncates to 0 is
                // unlimited.
                let micros = (seconds * 1_000_000.0).trunc();
                self.max_execution_time = if micros < 1.0 {
                    None
                } else {
                    Some(Duration::from_micros(micros as u64))
                };
            }
            "max_result_rows" => self.max_result_rows = parse_size(name, value)?,
            "max_result_bytes" => self.max_result_bytes = parse_size(name, value)?,
            "result_overflow_mode" => {
                self.result_overflow_break = match value {
                    "throw" => false,
                    "break" => true,
                    _ => return Err(cannot_parse(name, value, "'throw' or 'break'")),
                };
            }
            "max_threads" => {
                self.max_threads = parse_threads(name, value)?;
                self.max_threads_set = true;
            }
            "max_block_size" => {
                let n = parse_size(name, value)?;
                if n == 0 {
                    return Err(ChError::argument_out_of_bound(
                        "Setting max_block_size must not be zero",
                    ));
                }
                self.max_block_size = usize::try_from(n).unwrap_or(usize::MAX);
            }
            "max_memory_usage" => self.max_memory_usage = parse_size(name, value)?,
            "session_timezone" => {
                if value.is_empty() {
                    self.session_timezone = None;
                    self.text.timezone = SERVER_TIMEZONE.to_string();
                } else {
                    // The same zone database the values are written with.
                    loams_house_types::text::datetime_text(0, 0, 0, value).map_err(|_| {
                        ChError::bad_arguments(format!("Invalid time zone: {value}"))
                    })?;
                    self.session_timezone = Some(value.to_string());
                    self.text.timezone = value.to_string();
                }
            }
            "date_time_output_format" => {
                self.text.date_time_output = DateTimeOutput::parse(value).ok_or_else(|| {
                    cannot_parse(name, value, "'simple', 'iso' or 'unix_timestamp'")
                })?;
            }
            "union_default_mode" => {
                let mode = value.to_ascii_uppercase();
                if !matches!(mode.as_str(), "" | "ALL" | "DISTINCT") {
                    return Err(cannot_parse(name, value, "'', 'ALL' or 'DISTINCT'"));
                }
                self.union_default_mode = mode;
            }
            "asterisk_include_materialized_columns" => {
                self.asterisk_include_materialized_columns = parse_bool(name, value)?;
            }
            "asterisk_include_alias_columns" => {
                self.asterisk_include_alias_columns = parse_bool(name, value)?;
            }
            "output_format_json_quote_64bit_integers" => {
                self.text.json_quote_64bit_integers = parse_bool(name, value)?;
            }
            "output_format_json_quote_denormals" => {
                self.text.json_quote_denormals = parse_bool(name, value)?;
            }
            "output_format_json_escape_forward_slashes" => {
                self.text.json_escape_forward_slashes = parse_bool(name, value)?;
            }
            "output_format_json_named_tuples_as_objects" => {
                self.text.json_named_tuples_as_objects = parse_bool(name, value)?;
            }
            "output_format_decimal_trailing_zeros" => {
                self.text.decimal_trailing_zeros = parse_bool(name, value)?;
            }
            "join_use_nulls" => self.join_use_nulls = parse_bool(name, value)?,
            "final" => self.final_ = parse_bool(name, value)?,
            "aggregate_functions_null_for_empty" => {
                self.aggregate_functions_null_for_empty = parse_bool(name, value)?;
            }
            "group_by_use_nulls" => self.group_by_use_nulls = parse_bool(name, value)?,
            "prefer_column_name_to_alias" => {
                self.prefer_column_name_to_alias = parse_bool(name, value)?;
            }
            "count_distinct_implementation" => {
                let f = COUNT_DISTINCT_FUNCTIONS
                    .iter()
                    .find(|f| f.eq_ignore_ascii_case(value))
                    .ok_or_else(|| cannot_parse(name, value, "uniq, uniqCombined, uniqExact, …"))?;
                self.count_distinct_implementation = (*f).to_string();
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// The zone of zone-less `DateTime` values: `session_timezone`, else the
    /// server's (Task 30 converts with it).
    pub fn timezone(&self) -> &str {
        self.session_timezone.as_deref().unwrap_or(SERVER_TIMEZONE)
    }

    /// How values are written (Task 36's formats read it).
    pub fn text(&self) -> TextSettings {
        self.text.clone()
    }

    /// The value the statement set for `name`, if it set one.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    /// DataFusion's `target_partitions`: an explicit `max_threads`, the cores
    /// for `auto`, and 1 when the statement did not set it (the engine's
    /// default since Task 23, which keeps unordered answers in a stable order
    /// for the oracle; the answer is the same either way).
    pub fn target_partitions(&self) -> usize {
        match (self.max_threads_set, self.max_threads) {
            (false, _) => 1,
            (true, Some(n)) => n.max(1),
            (true, None) => std::thread::available_parallelism().map_or(1, |n| n.get()),
        }
    }

    /// Refuses, with `48` naming the task, every setting at a value other
    /// than its default whose construct `shape` has and the engine does not
    /// honour yet; and answers a bare `UNION` as `union_default_mode` says.
    ///
    /// # Errors
    ///
    /// `48` naming the setting and the task, or `559` for a bare `UNION`
    /// under an empty `union_default_mode`.
    pub fn check(&self, shape: &Shape) -> Result<(), ChError> {
        if shape.bare_union {
            match self.union_default_mode.as_str() {
                "" => return Err(expected_all_or_distinct()),
                "DISTINCT" => {}
                _ => {
                    return Err(not_yet(
                        "union_default_mode",
                        &self.union_default_mode,
                        Some(35),
                        "a bare UNION read as UNION ALL",
                    ));
                }
            }
        }
        for (name, value) in &self.values {
            let Lookup::Spec(spec) = lookup(name) else {
                continue;
            };
            if !matches!(spec.class, Class::Honoured | Class::DefaultOnly)
                || name == "union_default_mode"
                || at_default(spec, name, value)
            {
                continue;
            }
            if self.applies(spec.scope, shape) {
                return Err(not_yet(name, value, spec.task, &spec.note));
            }
        }
        Ok(())
    }

    fn applies(&self, scope: Scope, shape: &Shape) -> bool {
        match scope {
            Scope::Never => false,
            Scope::Always => true,
            Scope::OuterJoin => shape.outer_join,
            Scope::GroupingSets => shape.grouping_sets,
            Scope::Aggregate => shape.aggregate,
            Scope::CountDistinct => shape.count_distinct,
            Scope::BareUnion => shape.bare_union,
            Scope::MergeTables => shape.merge_tables,
            Scope::Time => shape.time,
            Scope::TimeParams => shape.params && shape.time,
            Scope::ResultLimit => self.max_result_rows > 0 || self.max_result_bytes > 0,
        }
    }

    /// DataFusion's configuration for the statement: `max_threads` as
    /// `target_partitions` and `max_block_size` as `batch_size`.
    pub fn config(&self) -> SessionConfig {
        SessionConfig::new()
            .with_target_partitions(self.target_partitions())
            .with_batch_size(self.max_block_size.max(1))
    }

    /// DataFusion's runtime for the statement: `max_memory_usage` as the
    /// memory pool's limit.
    ///
    /// # Errors
    ///
    /// DataFusion's, should the runtime not build.
    pub fn runtime(&self) -> Result<Arc<RuntimeEnv>, ChError> {
        let mut runtime = RuntimeEnvBuilder::new();
        if self.max_memory_usage > 0 {
            runtime = runtime.with_memory_limit(
                usize::try_from(self.max_memory_usage).unwrap_or(usize::MAX),
                1.0,
            );
        }
        runtime.build_arc().map_err(loams_house_sql::plan::ch_error)
    }

    /// A DataFusion session for the statement, with [`Settings::config`] and
    /// [`Settings::runtime`].
    ///
    /// # Errors
    ///
    /// DataFusion's, should the runtime not build.
    pub fn session(&self) -> Result<SessionContext, ChError> {
        Ok(SessionContext::new_with_config_rt(
            self.config(),
            self.runtime()?,
        ))
    }

    /// Holds the answer to `max_result_rows` and `max_result_bytes` under
    /// `result_overflow_mode = 'throw'` (`break` never gets here with a limit
    /// set: [`Settings::check`] refuses it).
    ///
    /// # Errors
    ///
    /// `396` when the answer is over a limit.
    pub fn check_result(&self, batches: &[RecordBatch]) -> Result<(), ChError> {
        if self.max_result_rows > 0 {
            let rows: u64 = batches.iter().map(|b| b.num_rows() as u64).sum();
            if rows > self.max_result_rows {
                return Err(too_many("rows", self.max_result_rows, rows));
            }
        }
        if self.max_result_bytes > 0 {
            let bytes: u64 = batches
                .iter()
                .map(|b| b.get_array_memory_size() as u64)
                .sum();
            if bytes > self.max_result_bytes {
                return Err(too_many("bytes", self.max_result_bytes, bytes));
            }
        }
        Ok(())
    }
}

/// One row of `system.settings` as the engine sees a statement's settings
/// (Task 38 maps it onto its own row type).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemSetting {
    /// ClickHouse's name.
    pub name: String,
    /// The statement's value, else the default.
    pub value: String,
    /// Whether the statement set a value other than the default.
    pub changed: bool,
    /// The default (the oracle's where the table has one, else chDB's).
    pub default: String,
    /// ClickHouse's setting type where the engine reads the setting
    /// (`Bool`, `UInt64`, `Seconds`, `MaxThreads`, …), else `String`.
    pub ty: &'static str,
    /// Whether the House lets a statement change it at all: a refused
    /// setting, or one of chDB's without a row (only its default, R9.11).
    pub readonly: bool,
    /// The engine's class (`honoured`, `noop`, `default_only`, `refused`,
    /// or `default` for chDB's rule) and why.
    pub description: String,
}

/// The type ClickHouse gives a setting the engine reads.
fn setting_type(name: &str) -> &'static str {
    match name {
        "max_execution_time" => "Seconds",
        "max_threads" => "MaxThreads",
        "max_result_rows" | "max_result_bytes" | "max_block_size" | "max_rows_to_read"
        | "max_bytes_to_read" => "UInt64",
        "max_memory_usage" | "max_bytes_before_external_group_by" => "UInt64",
        "result_overflow_mode" => "OverflowMode",
        "date_time_output_format" => "DateTimeOutputFormat",
        "date_time_input_format" => "DateTimeInputFormat",
        "session_timezone" => "Timezone",
        "union_default_mode" => "SetOperationMode",
        "join_use_nulls"
        | "final"
        | "aggregate_functions_null_for_empty"
        | "group_by_use_nulls"
        | "prefer_column_name_to_alias"
        | "asterisk_include_materialized_columns"
        | "asterisk_include_alias_columns"
        | "extremes"
        | "use_query_cache"
        | "output_format_json_quote_64bit_integers"
        | "output_format_json_quote_denormals"
        | "output_format_json_escape_forward_slashes"
        | "output_format_json_named_tuples_as_objects"
        | "output_format_decimal_trailing_zeros" => "Bool",
        _ => "String",
    }
}

/// `system.settings` for a statement run with `settings` (plan HE1 Task 38):
/// every ClickHouse setting chDB has, sorted by name, at the statement's
/// value where it set one. Loams's own and the front's settings are not
/// ClickHouse's and are not listed.
pub fn system_rows(settings: &Settings) -> Vec<SystemSetting> {
    let mut names: Vec<&String> = loams_house_ipc::policy::chdb_defaults().keys().collect();
    names.sort();
    names
        .into_iter()
        .map(|name| {
            let chdb = chdb_default(name).unwrap_or_default();
            let (default, readonly, description) = match lookup(name) {
                Lookup::Spec(spec) => {
                    let default = spec.default.clone().unwrap_or_else(|| chdb.clone());
                    let class = match spec.class {
                        Class::Honoured => "honoured",
                        Class::NoOp => "noop",
                        Class::DefaultOnly => "default_only",
                        Class::Refused => "refused",
                    };
                    (
                        default,
                        spec.class == Class::Refused,
                        format!("{class}: {}", spec.note),
                    )
                }
                _ => (
                    chdb.clone(),
                    true,
                    "default: accepted at its default only (R9.11)".to_string(),
                ),
            };
            let set = settings.get(name);
            let changed = set.is_some_and(|v| !same_setting_value(v, &default));
            SystemSetting {
                name: name.clone(),
                value: set.map_or_else(|| default.clone(), str::to_string),
                changed,
                default,
                ty: setting_type(name),
                readonly,
                description,
            }
        })
        .collect()
}

fn not_yet(name: &str, value: &str, task: Option<u32>, why: &str) -> ChError {
    let when = match task {
        Some(t) => format!(" yet (plan HE1 Task {t})"),
        None => String::new(),
    };
    ChError::not_implemented(format!(
        "The setting {name} = '{value}' is not supported by the Loams House engine for this \
         statement{when}: {why}"
    ))
}

/// What a statement has that a setting may change.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Shape {
    /// A `LEFT`, `RIGHT` or `FULL` join.
    pub outer_join: bool,
    /// `ROLLUP`, `CUBE` or `GROUPING SETS`.
    pub grouping_sets: bool,
    /// An aggregate or a window function.
    pub aggregate: bool,
    /// `COUNT(DISTINCT …)` or `countDistinct`.
    pub count_distinct: bool,
    /// A `UNION` with neither `ALL` nor `DISTINCT`.
    pub bare_union: bool,
    /// A read of a merging `MergeTree` engine.
    pub merge_tables: bool,
    /// A conversion, comparison or function of dates and times.
    pub time: bool,
    /// Query parameters.
    pub params: bool,
}

/// Whether a function's name is about dates or times (conservative: a false
/// positive only refuses a setting the statement could have had).
fn time_function(name: &str) -> bool {
    const WORDS: &[&str] = &[
        "date",
        "time",
        "day",
        "week",
        "month",
        "year",
        "hour",
        "minute",
        "second",
        "quarter",
        "now",
        "today",
        "yesterday",
        "zone",
        "unix",
        "interval",
        "relative",
        "monday",
    ];
    let lower = name.to_ascii_lowercase();
    WORDS.iter().any(|w| lower.contains(w))
}

fn temporal(t: &DataType) -> bool {
    matches!(
        t,
        DataType::Timestamp(..)
            | DataType::Date32
            | DataType::Date64
            | DataType::Time32(_)
            | DataType::Time64(_)
            | DataType::Interval(_)
            | DataType::Duration(_)
    ) || matches!(t, DataType::Dictionary(_, v) if temporal(v))
}

fn stringy(t: &DataType) -> bool {
    matches!(t, DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View)
        || matches!(t, DataType::Dictionary(_, v) if stringy(v))
}

/// A merging `MergeTree` engine: `FINAL` changes what it answers. Plain
/// `MergeTree` and non-MergeTree engines read the same with `FINAL`.
pub fn merging_engine(engine: &str) -> bool {
    let e = engine
        .strip_prefix("Replicated")
        .or_else(|| engine.strip_prefix("Shared"))
        .unwrap_or(engine);
    e.ends_with("MergeTree") && e != "MergeTree"
}

/// Whether `sql` has a `UNION` with neither `ALL` nor `DISTINCT`, by its
/// tokens (string literals and quoted names are single tokens, so a `UNION`
/// inside one is not counted).
pub fn has_bare_union(sql: &str) -> bool {
    let dialect = ClickHouseDialect {};
    let Ok(tokens) = Tokenizer::new(&dialect, sql).tokenize() else {
        return false;
    };
    let mut words = tokens
        .iter()
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .peekable();
    while let Some(t) = words.next() {
        if let Token::Word(w) = t
            && w.quote_style.is_none()
            && w.keyword == Keyword::UNION
        {
            let quantified = matches!(
                words.peek(),
                Some(Token::Word(n)) if n.quote_style.is_none()
                    && matches!(n.keyword, Keyword::ALL | Keyword::DISTINCT)
            );
            if !quantified {
                return true;
            }
        }
    }
    false
}

impl Shape {
    /// The shape of a planned statement. `sql` is its text, `merging` says
    /// whether a table read is a merging engine, and `params` whether the
    /// statement had query parameters.
    pub fn of(
        sql: &str,
        plan: &LogicalPlan,
        resolution: &Resolution,
        params: bool,
        merging: impl Fn(&[String]) -> bool,
    ) -> Self {
        let mut s = Self {
            bare_union: has_bare_union(sql),
            params,
            ..Self::default()
        };
        s.merge_tables = resolution.tables.iter().any(|t| merging(&t.name));
        for f in &resolution.functions {
            if f.eq_ignore_ascii_case("countDistinct") {
                s.count_distinct = true;
            }
            if time_function(f) {
                s.time = true;
            }
        }
        let mut casts_to_text = false;
        let mut temporal_columns = false;
        let _ = plan.apply_with_subqueries(|node| {
            match node {
                LogicalPlan::Join(j) => {
                    if matches!(
                        j.join_type,
                        JoinType::Left | JoinType::Right | JoinType::Full
                    ) {
                        s.outer_join = true;
                    }
                }
                LogicalPlan::Aggregate(a) => {
                    if !a.aggr_expr.is_empty() {
                        s.aggregate = true;
                    }
                    if a.group_expr
                        .iter()
                        .any(|e| matches!(e, Expr::GroupingSet(_)))
                    {
                        s.grouping_sets = true;
                    }
                }
                LogicalPlan::Window(_) => s.aggregate = true,
                _ => {}
            }
            if node
                .schema()
                .fields()
                .iter()
                .any(|f| temporal(f.data_type()))
            {
                temporal_columns = true;
            }
            node.apply_expressions(|e| {
                e.apply(|e| {
                    match e {
                        Expr::Cast(Cast { field, .. }) | Expr::TryCast(TryCast { field, .. }) => {
                            if temporal(field.data_type()) {
                                s.time = true;
                            } else if stringy(field.data_type()) {
                                casts_to_text = true;
                            }
                        }
                        Expr::AggregateFunction(AggregateFunction { func, params }) => {
                            s.aggregate = true;
                            if params.distinct && func.name().eq_ignore_ascii_case("count") {
                                s.count_distinct = true;
                            }
                        }
                        Expr::WindowFunction(_) => s.aggregate = true,
                        Expr::ScalarFunction(f) => {
                            if time_function(f.name()) {
                                s.time = true;
                            }
                        }
                        Expr::Literal(v, _) => {
                            let t = v.data_type();
                            if temporal(&t) {
                                s.time = true;
                            } else if stringy(&t) {
                                // Compared with or cast to a time by the
                                // analyzer, after this plan.
                                casts_to_text = true;
                            }
                        }
                        _ => {}
                    }
                    Ok(TreeNodeRecursion::Continue)
                })
            })
        });
        // A date or time written as text, or compared with text (the
        // analyzer adds that cast after this plan), is converted in the
        // session's zone. Conservative: any text beside a time counts.
        if casts_to_text && temporal_columns {
            s.time = true;
        }
        s
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn the_table_parses_and_names_known_settings() {
        let rows = parse_table(TABLE).unwrap();
        assert!(rows.len() > 60, "{}", rows.len());
        assert_eq!(rows.len(), table().len());
        let mut names: Vec<_> = rows.iter().map(|r| (&r.name, r.prefix)).collect();
        let all = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), all, "a name is classed twice");
        for r in &rows {
            if r.prefix {
                continue;
            }
            assert!(
                chdb_default(&r.name).is_some() || front_only(&r.name),
                "{} is not a ClickHouse setting",
                r.name
            );
            if r.class == Class::DefaultOnly {
                assert!(r.scope != Scope::Never, "{} never applies", r.name);
            }
        }
    }

    /// Names the front keeps for itself (never chDB's).
    pub(crate) fn front_only(name: &str) -> bool {
        matches!(
            name,
            "insert_deduplication_token" | "materialized_views_ignore_errors"
        ) || name.starts_with("loams_")
    }

    /// The table's defaults are the oracle's (26.9.2.8); chDB's table differs
    /// only where chDB itself is configured differently.
    #[test]
    fn defaults_match_chdb_but_for_known_differences() {
        const DIFFERENT: &[&str] = &["max_threads", "date_time_input_format", "async_insert"];
        for r in table() {
            let (Some(ours), false) = (&r.default, r.prefix) else {
                continue;
            };
            let Some(theirs) = chdb_default(&r.name) else {
                continue;
            };
            if DIFFERENT.contains(&r.name.as_str()) {
                continue;
            }
            assert!(
                same_setting_value(ours, &theirs),
                "{}: {ours} vs chDB's {theirs}",
                r.name
            );
        }
    }

    #[test]
    fn honoured_rows_are_read_by_the_engine() {
        for r in table() {
            if r.prefix {
                continue;
            }
            let default = r
                .default
                .clone()
                .or_else(|| chdb_default(&r.name))
                .unwrap_or_default();
            let mut s = Settings::default();
            let read = s.apply(&r.name, &default);
            if r.class == Class::Honoured {
                assert!(
                    matches!(read, Ok(true)),
                    "{} is honoured but not read: {read:?}",
                    r.name
                );
                // Its default changes nothing (`max_threads` records that it
                // was set).
                if r.name != "max_threads" {
                    assert_eq!(
                        s,
                        Settings::default(),
                        "{}'s default changes the settings",
                        r.name
                    );
                }
            }
        }
    }

    #[test]
    fn lookup_classes_names_prefixes_and_the_rest() {
        assert!(
            matches!(lookup("join_use_nulls"), Lookup::Spec(s) if s.class == Class::DefaultOnly)
        );
        assert!(
            matches!(lookup("output_format_json_quote_64bit_integers"), Lookup::Spec(s) if s.class == Class::Honoured)
        );
        assert!(
            matches!(lookup("output_format_pretty_max_rows"), Lookup::Spec(s) if s.prefix && s.class == Class::DefaultOnly)
        );
        assert!(
            matches!(lookup("input_format_null_as_default"), Lookup::Spec(s) if s.class == Class::NoOp)
        );
        assert!(matches!(lookup("loams_table_class"), Lookup::Spec(s) if s.class == Class::NoOp));
        // A prefix never makes an unknown name known.
        assert_eq!(lookup("output_format_no_such_thing"), Lookup::Unknown);
        assert!(matches!(lookup("use_skip_indexes"), Lookup::Known(_)));
        assert_eq!(lookup("no_such_setting"), Lookup::Unknown);
    }

    #[test]
    fn resolve_reads_values_and_refuses_what_it_must() {
        let s = Settings::resolve(&pairs(&[
            ("max_threads", "4"),
            ("session_timezone", "'Asia/Kolkata'"),
            ("date_time_output_format", "iso"),
            ("output_format_json_quote_64bit_integers", "true"),
            ("max_execution_time", "1.5"),
            ("max_result_rows", "10k"),
            ("join_use_nulls", "1"),
            ("log_comment", "anything"),
        ]))
        .unwrap();
        assert_eq!(s.target_partitions(), 4);
        assert_eq!(s.timezone(), "Asia/Kolkata");
        assert_eq!(s.text().timezone, "Asia/Kolkata");
        assert_eq!(s.text().date_time_output, DateTimeOutput::Iso);
        assert!(s.text().json_quote_64bit_integers);
        assert_eq!(s.max_execution_time, Some(Duration::from_millis(1500)));
        assert_eq!(s.max_result_rows, 10_000);
        assert!(s.join_use_nulls);
        assert_eq!(s.get("log_comment"), Some("anything"));

        // The later value wins.
        let s =
            Settings::resolve(&pairs(&[("max_threads", "4"), ("max_threads", "auto")])).unwrap();
        assert_eq!(s.max_threads, None);
        assert!(s.target_partitions() >= 1);
        assert_eq!(Settings::default().target_partitions(), 1);

        let e = Settings::resolve(&pairs(&[("no_such_setting", "1")])).unwrap_err();
        assert_eq!(e.code, 115);
        let e = Settings::resolve(&pairs(&[("use_query_cache", "1")])).unwrap_err();
        assert_eq!(e.code, 164);
        assert!(Settings::resolve(&pairs(&[("use_query_cache", "0")])).is_ok());
        // chDB's rule: a setting without a row only at its default.
        assert!(Settings::resolve(&pairs(&[("use_skip_indexes", "1")])).is_ok());
        let e = Settings::resolve(&pairs(&[("use_skip_indexes", "0")])).unwrap_err();
        assert_eq!(e.code, 164);
        for (name, value) in [
            ("max_threads", "many"),
            ("session_timezone", "Mars/Olympus"),
            ("date_time_output_format", "fancy"),
            ("join_use_nulls", "maybe"),
            ("max_execution_time", "-1"),
            ("union_default_mode", "SOME"),
            ("count_distinct_implementation", "count"),
        ] {
            let e = Settings::resolve(&pairs(&[(name, value)])).unwrap_err();
            assert_eq!(e.code, 36, "{name} = {value}: {e}");
        }
    }

    #[test]
    fn sizes_parse_as_clickhouse_reads_them() {
        assert_eq!(parse_size("x", "1000").unwrap(), 1000);
        assert_eq!(parse_size("x", "4G").unwrap(), 4_000_000_000);
        assert_eq!(parse_size("x", "4Gi").unwrap(), 4 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("x", "+1k").unwrap(), 1000);
        assert!(parse_size("x", "1.5G").is_err());
        assert!(parse_size("x", "G").is_err());
        assert!(parse_size("x", "1i").is_err());
        assert!(parse_size("x", "99999999999T").is_err());
    }

    #[test]
    fn scoped_settings_are_refused_only_where_they_apply() {
        let s = Settings::resolve(&pairs(&[("join_use_nulls", "1")])).unwrap();
        assert!(s.check(&Shape::default()).is_ok());
        let e = s
            .check(&Shape {
                outer_join: true,
                ..Shape::default()
            })
            .unwrap_err();
        assert_eq!(e.code, 48);
        assert!(e.message.contains("Task 35"), "{}", e.message);

        let s = Settings::resolve(&pairs(&[("final", "1")])).unwrap();
        assert!(s.check(&Shape::default()).is_ok());
        let merge = Shape {
            merge_tables: true,
            ..Shape::default()
        };
        assert_eq!(s.check(&merge).unwrap_err().code, 48);

        let s = Settings::resolve(&pairs(&[("session_timezone", "Asia/Kolkata")])).unwrap();
        assert!(s.check(&Shape::default()).is_ok());
        let time = Shape {
            time: true,
            ..Shape::default()
        };
        assert_eq!(s.check(&time).unwrap_err().code, 48);
        // At its default it applies nowhere.
        let s = Settings::resolve(&pairs(&[("session_timezone", "")])).unwrap();
        assert!(s.check(&time).is_ok());

        let s = Settings::resolve(&pairs(&[("prefer_column_name_to_alias", "1")])).unwrap();
        assert_eq!(s.check(&Shape::default()).unwrap_err().code, 48);

        let s = Settings::resolve(&pairs(&[("date_time_input_format", "best_effort")])).unwrap();
        assert!(s.check(&time).is_ok(), "no parameters");
        let e = s
            .check(&Shape {
                time: true,
                params: true,
                ..Shape::default()
            })
            .unwrap_err();
        assert_eq!(e.code, 48);

        let s = Settings::resolve(&pairs(&[("result_overflow_mode", "break")])).unwrap();
        assert!(s.check(&Shape::default()).is_ok(), "no limit set");
        let s = Settings::resolve(&pairs(&[
            ("result_overflow_mode", "break"),
            ("max_result_rows", "1"),
        ]))
        .unwrap();
        assert_eq!(s.check(&Shape::default()).unwrap_err().code, 48);
    }

    #[test]
    fn a_bare_union_follows_union_default_mode() {
        let bare = Shape {
            bare_union: true,
            ..Shape::default()
        };
        let e = Settings::default().check(&bare).unwrap_err();
        assert_eq!(e.code, 559);
        let s = Settings::resolve(&pairs(&[("union_default_mode", "DISTINCT")])).unwrap();
        assert!(s.check(&bare).is_ok());
        let s = Settings::resolve(&pairs(&[("union_default_mode", "ALL")])).unwrap();
        assert_eq!(s.check(&bare).unwrap_err().code, 48);
        assert!(s.check(&Shape::default()).is_ok());
    }

    #[test]
    fn bare_unions_are_found_by_their_tokens() {
        assert!(has_bare_union("SELECT 1 UNION SELECT 2"));
        assert!(has_bare_union("select 1 union\n  select 2"));
        assert!(!has_bare_union("SELECT 1 UNION ALL SELECT 2"));
        assert!(!has_bare_union("SELECT 1 UNION DISTINCT SELECT 2"));
        assert!(!has_bare_union("SELECT 'a UNION b'"));
        assert!(!has_bare_union("SELECT `union` FROM t"));
        assert!(has_bare_union(
            "SELECT * FROM (SELECT 1 UNION ALL SELECT 2 UNION SELECT 3)"
        ));
    }

    #[test]
    fn merging_engines_are_the_ones_final_changes() {
        assert!(merging_engine("ReplacingMergeTree"));
        assert!(merging_engine("ReplicatedCollapsingMergeTree"));
        assert!(merging_engine("SharedAggregatingMergeTree"));
        assert!(!merging_engine("MergeTree"));
        assert!(!merging_engine("ReplicatedMergeTree"));
        assert!(!merging_engine("Memory"));
        assert!(!merging_engine(""));
    }

    #[test]
    fn result_limits_throw_396() {
        use datafusion::arrow::array::Int64Array;
        let batch =
            RecordBatch::try_from_iter(vec![("a", Arc::new(Int64Array::from(vec![1, 2, 3])) as _)])
                .unwrap();
        let s = Settings::resolve(&pairs(&[("max_result_rows", "2")])).unwrap();
        assert_eq!(
            s.check_result(std::slice::from_ref(&batch))
                .unwrap_err()
                .code,
            396
        );
        let s = Settings::resolve(&pairs(&[("max_result_rows", "3")])).unwrap();
        assert!(s.check_result(std::slice::from_ref(&batch)).is_ok());
        let s = Settings::resolve(&pairs(&[("max_result_bytes", "1")])).unwrap();
        assert_eq!(s.check_result(&[batch]).unwrap_err().code, 396);
    }

    #[test]
    fn system_rows_list_every_setting_at_the_statements_values() {
        let s =
            Settings::resolve(&pairs(&[("max_threads", "4"), ("join_use_nulls", "0")])).unwrap();
        let rows = system_rows(&s);
        assert_eq!(rows.len(), loams_house_ipc::policy::chdb_defaults().len());
        let row = |n: &str| rows.iter().find(|r| r.name == n).unwrap();
        assert_eq!(row("max_threads").value, "4");
        assert!(row("max_threads").changed);
        assert_eq!(row("max_threads").ty, "MaxThreads");
        assert!(!row("join_use_nulls").changed, "set, but to its default");
        assert_eq!(row("session_timezone").value, "");
        assert!(row("use_query_cache").readonly);
        assert!(row("use_skip_indexes").readonly);
        assert!(!row("join_use_nulls").readonly);
        assert!(
            row("join_use_nulls")
                .description
                .starts_with("default_only")
        );
        assert!(rows.windows(2).all(|w| w[0].name < w[1].name));
    }

    #[test]
    fn time_functions_are_recognised_conservatively() {
        for f in [
            "toDateTime",
            "now",
            "toStartOfDay",
            "formatDateTime",
            "dateDiff",
            "toYear",
        ] {
            assert!(time_function(f), "{f}");
        }
        for f in ["plus", "length", "lower", "count"] {
            assert!(!time_function(f), "{f}");
        }
    }
}
