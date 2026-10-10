//! Settings a request or a session may set (HS1 Task 4; FL2 Task 4, §32 §8.5,
//! FL2 Ruling 10).
//!
//! A setting is allowed by name (or by a prefix of formats, I/O format options and
//! analyzer experiments). An allowed setting with a cap is refused over it with
//! `164 READONLY` naming the cap — never clamped silently. A name that is not
//! allowed is `164` when the engine knows it (the worker's `Ready` lists every name
//! in `system.settings`) and `115 UNKNOWN_SETTING` when it does not. Loams's own
//! settings (`loams_*`) are the front's: allowed, and never passed to chDB.

use std::collections::HashSet;

use crate::errors::{ChError, HouseError};

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

/// Prefixes allowed whole (§32 §8.5): output and input format options, and the
/// CSV and TSV options, less [`DENIED_SETTINGS`]. `format_schema*` and
/// `format_template_*` name files and are not here.
pub const ALLOWED_PREFIXES: &[&str] = &[
    "output_format_",
    "input_format_",
    "format_csv_",
    "format_tsv_",
];

/// The analyzer experiments a query may switch (fix round 1, M3). An explicit
/// list, not the `allow_experimental_` prefix, which also opens `eval`, AI and
/// URL-wildcard functions, catalogs and writers.
pub const ALLOWED_EXPERIMENTS: &[&str] = &[
    "allow_experimental_analyzer",
    "allow_experimental_correlated_subqueries",
    "allow_experimental_join_right_table_sorting",
];

/// Settings no request may change, whatever list would allow them: the deny
/// list's (HS1 Task 5), which also covers the `SETTINGS` clause.
pub use crate::deny::DENIED_SETTINGS;

/// Loams's settings (HS1 Shared contracts): the front's own.
pub const LOAMS_SETTINGS: &[&str] = &[
    "loams_table_class",
    "loams_snapshot_id",
    "loams_as_of",
    "loams_lake_commit_interval_ms",
    "loams_lake_commit_bytes",
    "loams_lake_target_file_bytes",
    "loams_dedup_window",
    "loams_pipe_max_rows",
    "loams_pipe_max_delay_ms",
    "loams_wake_timeout_ms",
    "loams_consistency_wait_ms",
    "loams_consistency_token",
    "loams_bucket_key",
    "loams_tiering_freshness",
];

/// The caps (FL2 Ruling 10).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SessionLimits {
    /// `max_memory_usage`, at most 4 GiB (0, unlimited, is over it).
    pub max_memory_usage: u64,
    /// `max_execution_time`, at most 300 s (0, unlimited, is over it).
    pub max_execution_time_s: f64,
    /// `max_threads`, at most the node's cores (0 means "auto", which is allowed).
    pub max_threads: u64,
}

impl Default for SessionLimits {
    fn default() -> Self {
        Self {
            max_memory_usage: 4 * 1024 * 1024 * 1024,
            max_execution_time_s: 300.0,
            max_threads: std::thread::available_parallelism().map_or(1, |n| n.get() as u64),
        }
    }
}

fn readonly(message: String) -> HouseError {
    HouseError::from(ChError::readonly(message))
}

fn over(name: &str, cap: impl std::fmt::Display) -> HouseError {
    readonly(format!("Setting {name} shouldn't be greater than {cap}"))
}

/// Whether `name` is one of Loams's settings.
pub fn is_loams(name: &str) -> bool {
    LOAMS_SETTINGS.contains(&name)
}

/// Whether `name` may be set at all.
pub fn is_allowed(name: &str) -> bool {
    (is_loams(name)
        || ALLOWED_SETTINGS.contains(&name)
        || ALLOWED_EXPERIMENTS.contains(&name)
        || ALLOWED_PREFIXES.iter().any(|p| name.starts_with(p)))
        && !crate::deny::is_denied_setting(name)
}

/// Checks one setting: `115`, `164`, or fine.
pub fn check(
    name: &str,
    value: &str,
    limits: &SessionLimits,
    known: &HashSet<String>,
) -> Result<(), HouseError> {
    if !is_allowed(name) {
        // A denied name is `164` even where the engine has no such setting
        // (`user_files_path` is a server setting): it is refused, not unknown.
        return Err(
            if known.contains(name) || crate::deny::is_denied_setting(name) {
                readonly(format!(
                    "Cannot modify '{name}' setting: it is not allowed on the House (see the surface page)"
                ))
            } else {
                HouseError::from(ChError::unknown_setting(format!(
                    "Unknown setting '{name}'"
                )))
            },
        );
    }
    // The value exactly as chDB will read it: the worker sets it as a quoted
    // string (`SET name = 'value'`), so it is parsed the way ClickHouse parses a
    // setting's string (fix round 1, I3). What cannot be parsed so is refused, and
    // so is what converts to 0 (unlimited) or past a cap; the worker's profile
    // enforces the same bounds after the engine's own conversion, for the
    // `SETTINGS` clause the front never parses.
    let unparseable = |kind: &str| {
        HouseError::from(ChError::bad_arguments(format!(
            "Cannot parse '{value}' as the value of setting {name}: expected {kind}"
        )))
    };
    match name {
        "max_threads" => {
            // `auto`, `auto(n)` and 0 are the node's cores: bounded.
            if !value.starts_with("auto") {
                let n =
                    parse_unsigned(value).ok_or_else(|| unparseable("a thread count or 'auto'"))?;
                if n > limits.max_threads {
                    return Err(over(name, limits.max_threads));
                }
            }
        }
        "max_memory_usage" => {
            let n = parse_size(value)
                .ok_or_else(|| unparseable("a byte count such as 1000000000, 4G or 4Gi"))?;
            if n == 0 {
                return Err(under(name, 1));
            }
            if n > limits.max_memory_usage {
                return Err(over(name, limits.max_memory_usage));
            }
        }
        "max_execution_time" => {
            let seconds = value
                .parse::<f64>()
                .ok()
                .filter(|s| s.is_finite())
                .ok_or_else(|| unparseable("a number of seconds"))?;
            // ClickHouse keeps it in whole microseconds: what truncates to 0 (or
            // is below it) is unlimited.
            if seconds < MIN_EXECUTION_TIME_S {
                return Err(under(name, MIN_EXECUTION_TIME_S));
            }
            if seconds > limits.max_execution_time_s {
                return Err(over(name, limits.max_execution_time_s));
            }
        }
        _ => {}
    }
    Ok(())
}

/// The smallest `max_execution_time` that is not unlimited: one microsecond.
pub const MIN_EXECUTION_TIME_S: f64 = 0.000_001;

fn under(name: &str, floor: impl std::fmt::Display) -> HouseError {
    readonly(format!(
        "Setting {name} shouldn't be less than {floor} (0 is unlimited)"
    ))
}

/// An unsigned integer setting's string as ClickHouse reads it: an optional `+`,
/// then decimal digits (leading zeros allowed), nothing else. ClickHouse wraps an
/// overflow (`'18446744073709551616'` reads as 0); here it does not parse.
pub fn parse_unsigned(text: &str) -> Option<u64> {
    let digits = text.strip_prefix('+').unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// A byte-size setting's string as ClickHouse reads it (`parseWithSizeSuffix`,
/// measured on ClickHouse 26.7): [`parse_unsigned`], then optionally `k` or `K`
/// (10^3), `M`, `G` or `T` (10^6, 10^9, 10^12), each optionally followed by `i`
/// for the power of 1024 instead (`4Gi` is 4 294 967 296). Nothing else: no `B`,
/// no `P`, no space, no fraction. ClickHouse wraps an overflow; here it does not
/// parse.
pub fn parse_size(text: &str) -> Option<u64> {
    let unit_at = text
        .bytes()
        .position(|b| !(b.is_ascii_digit() || b == b'+'))
        .unwrap_or(text.len());
    let (number, suffix) = text.split_at(unit_at);
    let number = parse_unsigned(number)?;
    let power = |p: u32, binary: bool| {
        if binary {
            1024u64.checked_pow(p)
        } else {
            1000u64.checked_pow(p)
        }
    };
    let (unit, binary) = match suffix.strip_suffix('i') {
        Some(unit) if !unit.is_empty() => (unit, true),
        _ => (suffix, false),
    };
    let multiplier = match unit {
        "" if !binary => 1,
        "k" | "K" => power(1, binary)?,
        "M" => power(2, binary)?,
        "G" => power(3, binary)?,
        "T" => power(4, binary)?,
        _ => return None,
    };
    number.checked_mul(multiplier)
}

/// Settings in order, later values replacing earlier ones of the same name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pairs: Vec<(String, String)>,
}

impl Settings {
    /// No settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Checks and records one setting.
    pub fn apply(
        &mut self,
        name: &str,
        value: &str,
        limits: &SessionLimits,
        known: &HashSet<String>,
    ) -> Result<(), HouseError> {
        check(name, value, limits, known)?;
        match self.pairs.iter_mut().find(|(n, _)| n == name) {
            Some(pair) => pair.1 = value.to_string(),
            None => self.pairs.push((name.to_string(), value.to_string())),
        }
        Ok(())
    }

    /// Every setting, Loams's included.
    pub fn as_slice(&self) -> &[(String, String)] {
        &self.pairs
    }

    /// The value of one setting.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.pairs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// What chDB gets: everything but Loams's own settings.
    pub fn for_engine(&self) -> Vec<(String, String)> {
        self.pairs
            .iter()
            .filter(|(n, _)| !is_loams(n))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> HashSet<String> {
        ["max_threads", "readonly", "allow_ddl", "format_schema"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn unknown_disallowed_and_capped() {
        let limits = SessionLimits {
            max_threads: 8,
            ..SessionLimits::default()
        };
        let known = known();
        assert_eq!(
            check("no_such_setting", "1", &limits, &known)
                .expect_err("unknown")
                .code(),
            115
        );
        assert_eq!(
            check("readonly", "0", &limits, &known)
                .expect_err("disallowed")
                .code(),
            164
        );
        assert_eq!(
            check("format_schema", "x", &limits, &known)
                .expect_err("a file path")
                .code(),
            164
        );
        let over = check("max_threads", "9", &limits, &known).expect_err("over");
        assert_eq!(over.code(), 164);
        assert!(
            over.message().contains("shouldn't be greater than 8"),
            "{over}"
        );
        check("max_threads", "8", &limits, &known).expect("at the cap");
        check("max_threads", "0", &limits, &known).expect("auto");
        assert_eq!(
            check("max_memory_usage", "0", &limits, &known)
                .expect_err("unlimited")
                .code(),
            164
        );
        assert_eq!(
            check("max_execution_time", "300.5", &limits, &known)
                .expect_err("over")
                .code(),
            164
        );
        check(
            "output_format_json_quote_64bit_integers",
            "0",
            &limits,
            &known,
        )
        .expect("prefix");
        check("loams_snapshot_id", "42", &limits, &known).expect("Loams's own");
    }

    /// Fix round 1, I3: values are read the way ClickHouse reads a setting's
    /// string, and what converts to 0 (unlimited), past a cap, or cannot be read
    /// is refused — not passed on unchecked.
    #[test]
    fn capped_values_are_parsed_the_clickhouse_way() {
        let limits = SessionLimits {
            max_threads: 8,
            ..SessionLimits::default()
        };
        let known = known();
        let code =
            |name: &str, value: &str| check(name, value, &limits, &known).err().map(|e| e.code());
        // Byte sizes (measured on ClickHouse 26.7).
        assert_eq!(parse_size("4Gi"), Some(4 * 1024 * 1024 * 1024));
        assert_eq!(parse_size("4G"), Some(4_000_000_000));
        assert_eq!(parse_size("4ki"), Some(4096));
        assert_eq!(parse_size("4K"), Some(4000));
        assert_eq!(parse_size("+018"), Some(18));
        assert_eq!(parse_size("4Ti"), Some(4 * 1024u64.pow(4)));
        for bad in [
            "",
            "+",
            "4GiB",
            "4gi",
            "4KI",
            "1.5Gi",
            "1e9",
            "0x10",
            " 5",
            "5 ",
            "4m",
            "4P",
            "4i",
            "-1",
            "1_000",
            "18446744073709551616",
            "20000000Ti",
        ] {
            assert_eq!(parse_size(bad), None, "{bad:?}");
        }
        for (value, expected) in [
            ("4Gi", None),
            ("1", None),
            ("4294967296", None),
            ("4G", None),
            ("0", Some(164)),
            ("+0", Some(164)),
            ("0Gi", Some(164)),
            ("4294967297", Some(164)),
            ("5G", Some(164)),
            ("4Ti", Some(164)),
            // ClickHouse wraps these to 0 and to 3.5e18.
            ("18446744073709551616", Some(36)),
            ("20000000Ti", Some(36)),
            ("1e30", Some(36)),
            ("1.5Gi", Some(36)),
            ("unlimited", Some(36)),
            ("'4Gi'", Some(36)),
            (" 4Gi", Some(36)),
        ] {
            assert_eq!(code("max_memory_usage", value), expected, "{value:?}");
        }
        for (value, expected) in [
            ("300", None),
            ("0.5", None),
            (".5", None),
            ("1E2", None),
            ("+5", None),
            ("0.000001", None),
            ("0", Some(164)),
            ("-0", Some(164)),
            ("-1", Some(164)),
            ("1e-7", Some(164)),
            ("0.0000005", Some(164)),
            ("300.5", Some(164)),
            ("1e3", Some(164)),
            ("inf", Some(36)),
            ("Infinity", Some(36)),
            ("nan", Some(36)),
            ("5m", Some(36)),
            ("0x10", Some(36)),
            ("", Some(36)),
        ] {
            assert_eq!(code("max_execution_time", value), expected, "{value:?}");
        }
        for (value, expected) in [
            ("8", None),
            ("+4", None),
            ("0", None),
            ("auto", None),
            ("auto(4)", None),
            ("9", Some(164)),
            ("99999999999999999999", Some(36)),
            ("018446744073709551617", Some(36)),
            ("1e3", Some(36)),
            ("4Ki", Some(36)),
        ] {
            assert_eq!(code("max_threads", value), expected, "{value:?}");
        }
    }

    /// Fix round 1, I4 and M3: file-naming settings under allowed prefixes, and
    /// experiments outside the explicit list, are refused.
    #[test]
    fn path_settings_and_other_experiments_are_refused() {
        let limits = SessionLimits::default();
        let known: HashSet<String> = [
            "input_format_record_errors_file_path",
            "output_format_schema",
            "allow_experimental_eval_table_function",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        for name in known.iter() {
            assert!(!is_allowed(name), "{name}");
            assert_eq!(
                check(name, "/abs/path/x", &limits, &known)
                    .expect_err("refused")
                    .code(),
                164,
                "{name}"
            );
        }
        assert!(is_allowed("allow_experimental_analyzer"));
        assert!(is_allowed("input_format_skip_unknown_fields"));
    }

    #[test]
    fn settings_replace_and_keep_loams_settings_from_the_engine() {
        let limits = SessionLimits::default();
        let known = known();
        let mut settings = Settings::new();
        settings
            .apply("max_block_size", "100", &limits, &known)
            .expect("ok");
        settings
            .apply("max_block_size", "200", &limits, &known)
            .expect("ok");
        settings
            .apply("loams_as_of", "2026-01-01", &limits, &known)
            .expect("ok");
        assert_eq!(settings.get("max_block_size"), Some("200"));
        assert_eq!(
            settings.for_engine(),
            vec![("max_block_size".to_string(), "200".to_string())]
        );
    }
}
