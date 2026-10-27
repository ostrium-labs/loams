//! Which settings a statement may change: one list for the front's check and
//! the worker's settings profile (HS1 ruling R-ACL.1).
//!
//! The front refuses a setting outside these lists before anything runs (HS1
//! Task 4, R4.3). The worker writes the same lists into its chDB settings
//! profile: every other setting the engine has is pinned with a `<readonly/>`
//! constraint, so ClickHouse itself answers `452` to a `SET`, a URL parameter or
//! a `SETTINGS` clause that names one, whatever the front saw. The two cannot
//! drift, because they read this module.

/// §32 §8.5's allowlist, plus what HTTP clients routinely send (`max_block_size`,
/// `log_comment`, `http_headers_progress_interval_ms`, `extremes`): ruling R4.3.
pub const ALLOWED_SETTINGS: &[&str] = &[
    "max_execution_time",
    "max_result_rows",
    "max_result_bytes",
    "result_overflow_mode",
    "max_rows_to_read",
    "max_bytes_to_read",
    "max_memory_usage",
    "date_time_input_format",
    "date_time_output_format",
    "join_algorithm",
    "max_threads",
    "session_timezone",
    "insert_deduplicate",
    "async_insert",
    "wait_for_async_insert",
    "max_block_size",
    "log_comment",
    "http_headers_progress_interval_ms",
    "extremes",
];

/// What the gate applications' ClickHouse clients send on every query (HE1 Task
/// 9, R9.11): Langfuse v3 and v4 (`@clickhouse/client`) and OpenPanel 2.3.0, from
/// their `system.query_log` on the oracle. Each changes how one statement is
/// planned, executed or printed, inside the worker's own limits:
///
/// * `timeout_before_checking_execution_speed`: when `min_execution_speed` is
///   checked (the House sets none).
/// * `asterisk_include_materialized_columns`, `asterisk_include_alias_columns`:
///   what `SELECT *` expands to.
/// * `enable_producing_buckets_out_of_order_in_aggregation`, `join_use_nulls`,
///   `distributed_product_mode`, `query_plan_convert_any_join_to_semi_or_anti_join`,
///   `query_plan_optimize_lazy_materialization`: planner and join rules.
/// * `max_bytes_before_external_group_by`: when a `GROUP BY` spills; the memory
///   cap (`max_memory_usage`) still holds.
/// * `max_insert_block_size`: the staging insert's block size.
/// * `use_text_index_dictionary_cache`, `use_text_index_postings_cache`: text
///   index caches (lake tables have no text index, so nothing is cached).
/// * `send_progress_in_http_headers`: also a request parameter the front reads.
/// * `use_query_cache`: only off. A read view is re-made per snapshot under the
///   same name, so a cached result could be stale; the front refuses `1` (`164`)
///   and the worker's profile caps it at 0.
pub const CLIENT_SETTINGS: &[&str] = &[
    "timeout_before_checking_execution_speed",
    "asterisk_include_materialized_columns",
    "asterisk_include_alias_columns",
    "enable_producing_buckets_out_of_order_in_aggregation",
    "join_use_nulls",
    "distributed_product_mode",
    "query_plan_convert_any_join_to_semi_or_anti_join",
    "query_plan_optimize_lazy_materialization",
    "max_bytes_before_external_group_by",
    "max_insert_block_size",
    "use_text_index_dictionary_cache",
    "use_text_index_postings_cache",
    "send_progress_in_http_headers",
    "use_query_cache",
    // SigNoz (HE1 R9.16): how JSON input with a path given twice is read.
    "type_json_skip_duplicated_paths",
];

/// Settings the worker's profile leaves changeable only up to 0: on, they would
/// be wrong on the House (see [`CLIENT_SETTINGS`]).
pub const OFF_ONLY_SETTINGS: &[&str] = &["use_query_cache"];

/// §49 §12's `max_bytes_to_read` per query, 1 TiB: the front's cap (`164` past
/// it) and the worker profile's value and maximum (HS1 Task 21).
pub const MAX_BYTES_TO_READ: u64 = 1024 * 1024 * 1024 * 1024;

/// §49 §12's `max_result_bytes` per query, 10 GiB, held as
/// [`MAX_BYTES_TO_READ`] is.
pub const MAX_RESULT_BYTES: u64 = 10 * 1024 * 1024 * 1024;

/// Prefixes allowed whole (§32 §8.5): output and input format options, and the
/// CSV and TSV options, less the deny list's settings. `format_schema*` and
/// `format_template_*` name files and are not here.
pub const ALLOWED_PREFIXES: &[&str] = &[
    "output_format_",
    "input_format_",
    "format_csv_",
    "format_tsv_",
];

/// The analyzer experiments a query may switch (HS1 Task 4 fix round 1, M3). An
/// explicit list, not the `allow_experimental_` prefix, which also opens `eval`,
/// AI and URL-wildcard functions, catalogs and writers.
pub const ALLOWED_EXPERIMENTS: &[&str] = &[
    "allow_experimental_analyzer",
    "allow_experimental_correlated_subqueries",
    "allow_experimental_join_right_table_sorting",
];

/// Settings the House's own statements set on a worker's connections, which the
/// worker's profile must therefore leave changeable (R-ACL.1). None is on the
/// front's lists, so a request still cannot set them (`164`); with the front
/// bypassed, each changes only how the namespace's own data is read or shown.
///
/// * `readonly`, `allow_ddl`: every user connection is opened with `readonly = 2`
///   and `allow_ddl = 0` (R1.9, R5.6). Both are sticky by the engine's own rules:
///   neither can be set back on that connection (measured).
/// * `flatten_nested`: the front's `DESCRIBE TABLE null(…)` of a `CREATE TABLE`'s
///   columns (R10.3).
/// * `describe_compact_output`: the API's result header (R8.5).
/// * `iceberg_snapshot_id`: a time-travel view's own `SETTINGS` (R11.1). A view's
///   settings are checked against the profile when it is created (measured).
/// * `final`: a statement's `SETTINGS final = 1`, which the front reads to pick
///   merge views (R11.1, R11.4).
/// * `database`: `USE d` sets it (chDB 26.9, measured: with it pinned `USE` is
///   `452`), and the worker `USE`s the request's database (R11.2).
pub const HOUSE_SETTINGS: &[&str] = &[
    "readonly",
    "allow_ddl",
    "database",
    "flatten_nested",
    "describe_compact_output",
    "iceberg_snapshot_id",
    "final",
];

/// Whether `name` is on the front's lists (before the deny list, which removes
/// some names these lists would allow; HS1 Task 5).
pub fn on_the_lists(name: &str) -> bool {
    ALLOWED_SETTINGS.contains(&name)
        || CLIENT_SETTINGS.contains(&name)
        || ALLOWED_EXPERIMENTS.contains(&name)
        || ALLOWED_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// chDB's query-level settings with their defaults (`system.settings` of chDB
/// 26.9.2.1, TSV-escaped): the front's "a known setting at its default is
/// accepted" rule (HE1 R9.11), and the Rust engine's table of every setting it
/// may be sent (HE1 Task 37, D1805).
pub const CHDB_SETTING_DEFAULTS: &str = include_str!("chdb_setting_defaults.tsv");

/// TSV's escapes undone.
fn unescape_tsv(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
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
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// Every setting in [`CHDB_SETTING_DEFAULTS`], by name, with its default.
pub fn chdb_defaults() -> &'static std::collections::HashMap<String, String> {
    use std::collections::HashMap;
    use std::sync::OnceLock;
    static DEFAULTS: OnceLock<HashMap<String, String>> = OnceLock::new();
    DEFAULTS.get_or_init(|| {
        CHDB_SETTING_DEFAULTS
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
            .filter_map(|l| l.split_once('\t'))
            .map(|(n, d)| (unescape_tsv(n), unescape_tsv(d)))
            .collect()
    })
}

/// chDB's default for `name`, if it is one of its settings.
pub fn chdb_default(name: &str) -> Option<String> {
    chdb_defaults().get(name).cloned()
}

/// Whether `value` says the same as `default`, as ClickHouse would read a
/// setting: the same text (`value`'s surrounding spaces and quotes aside), or the same boolean (`0`/`false`,
/// `1`/`true`), or the same number.
pub fn same_setting_value(value: &str, default: &str) -> bool {
    let value = value.trim().trim_matches('\'');
    if value == default {
        return true;
    }
    let boolean = |v: &str| match v.to_ascii_lowercase().as_str() {
        "0" | "false" => Some(false),
        "1" | "true" => Some(true),
        _ => None,
    };
    if let (Some(a), Some(b)) = (boolean(value), boolean(default)) {
        return a == b;
    }
    matches!((value.parse::<f64>(), default.parse::<f64>()), (Ok(a), Ok(b)) if a == b)
}

/// Whether `value` is `name`'s default, as ClickHouse would read it (HE1
/// R9.11). Clients send settings at their defaults (the query log, which
/// records only changed settings, never shows them), and asking for the default
/// changes nothing, so such a setting is accepted whatever list it is on.
pub fn is_default(name: &str, value: &str) -> bool {
    chdb_default(name).is_some_and(|default| same_setting_value(value, &default))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_are_distinct_and_house_settings_are_not_on_them() {
        let mut names: Vec<&str> = ALLOWED_SETTINGS
            .iter()
            .chain(CLIENT_SETTINGS)
            .chain(ALLOWED_EXPERIMENTS)
            .chain(HOUSE_SETTINGS)
            .copied()
            .collect();
        let all = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), all);
        for name in HOUSE_SETTINGS {
            assert!(!on_the_lists(name), "{name} would be a request's to set");
        }
        assert!(on_the_lists("output_format_json_quote_64bit_integers"));
        assert!(!on_the_lists("max_bytes_before_external_sort"));
        for name in OFF_ONLY_SETTINGS {
            assert!(CLIENT_SETTINGS.contains(name), "{name}");
        }
    }

    #[test]
    fn defaults_read_as_clickhouse_reads_a_setting() {
        assert!(chdb_default("format_custom_field_delimiter").is_some_and(|d| d.contains('\t')));
        assert!(is_default("join_use_nulls", "false"));
        assert!(is_default("join_use_nulls", "'0'"));
        assert!(!is_default("join_use_nulls", "1"));
        assert!(is_default("max_block_size", "65409.0"));
        assert!(is_default("session_timezone", "''"));
        assert!(!is_default("no_such_setting", "0"));
        assert!(same_setting_value("true", "1"));
    }
}
