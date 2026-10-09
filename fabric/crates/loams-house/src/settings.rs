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

/// Prefixes allowed whole (§32 §8.5): output and input format options, the CSV
/// and TSV options, and analyzer experiments chDB enables by default.
/// `format_schema*` and `format_template_*` name files and are not here.
pub const ALLOWED_PREFIXES: &[&str] = &[
    "output_format_",
    "input_format_",
    "format_csv_",
    "format_tsv_",
    "allow_experimental_",
];

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
    is_loams(name)
        || ALLOWED_SETTINGS.contains(&name)
        || ALLOWED_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// Checks one setting: `115`, `164`, or fine.
pub fn check(
    name: &str,
    value: &str,
    limits: &SessionLimits,
    known: &HashSet<String>,
) -> Result<(), HouseError> {
    if !is_allowed(name) {
        return Err(if known.contains(name) {
            readonly(format!(
                "Cannot modify '{name}' setting: it is not allowed on the House (see the surface page)"
            ))
        } else {
            HouseError::from(ChError::unknown_setting(format!(
                "Unknown setting '{name}'"
            )))
        });
    }
    let value = value.trim().trim_matches('\'');
    match name {
        "max_threads" => {
            if let Ok(n) = value.parse::<u64>()
                && n > limits.max_threads
            {
                return Err(over(name, limits.max_threads));
            }
        }
        "max_memory_usage" => {
            if let Ok(n) = value.parse::<u64>()
                && (n == 0 || n > limits.max_memory_usage)
            {
                return Err(over(name, limits.max_memory_usage));
            }
        }
        "max_execution_time" => {
            if let Ok(s) = value.parse::<f64>()
                && (s == 0.0 || s > limits.max_execution_time_s)
            {
                return Err(over(name, limits.max_execution_time_s));
            }
        }
        _ => {}
    }
    Ok(())
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
