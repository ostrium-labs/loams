//! Reading a `system.query_log` dump.
//!
//! The dump is ClickHouse `TSVWithNames`: a header line of column names, then
//! one row per line with ClickHouse's TSV escaping. Two columns are required,
//! `query_id` and `query`; the others are optional and used when present:
//!
//! - `type`: rows of type `ExceptionBeforeStart` are skipped (the server never
//!   parsed or ran them, so they are not evidence that the application uses a
//!   feature).
//! - `is_initial_query`: rows with `0` are skipped (the server made them, for
//!   example a `Distributed` table's shard queries).
//! - `Settings`: the settings the client changed for that query, as a
//!   ClickHouse `Map(String, String)`; only the names are kept.
//!
//! A row whose `query_id` was already seen is skipped (`query_log` writes a
//! `QueryStart` and a `QueryFinish` row for the same query). The harness dumps
//! it with:
//!
//! ```sql
//! SELECT query_id, type, is_initial_query, query, Settings
//! FROM system.query_log
//! WHERE type != 'QueryStart'
//! ORDER BY event_time_microseconds
//! FORMAT TSVWithNames
//! ```

use std::collections::HashSet;

use anyhow::{Context as _, bail};

/// One query the application sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggedQuery {
    /// `query_id`.
    pub id: String,
    /// The statement text.
    pub query: String,
    /// The names of the settings the client changed for this query, sorted.
    pub settings: Vec<String>,
}

/// What reading a dump skipped, by reason.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Skipped {
    /// `type = 'ExceptionBeforeStart'`.
    pub failed_before_start: u64,
    /// `is_initial_query = 0`.
    pub not_initial: u64,
    /// A second row for a `query_id` already read.
    pub duplicate: u64,
    /// The harness's own queries of `system.query_log` or `system.part_log`.
    pub harness: u64,
}

/// Parses a `TSVWithNames` dump.
///
/// # Errors
///
/// A missing `query_id` or `query` column, or a row with the wrong number of
/// fields.
pub fn parse(text: &str) -> anyhow::Result<(Vec<LoggedQuery>, Skipped)> {
    let mut lines = text.lines();
    let header: Vec<String> = match lines.next() {
        Some(line) => split_row(line),
        None => return Ok((Vec::new(), Skipped::default())),
    };
    let column = |name: &str| header.iter().position(|h| h == name);
    let id_col = column("query_id").context("query log: no `query_id` column")?;
    let query_col = column("query").context("query log: no `query` column")?;
    let type_col = column("type");
    let initial_col = column("is_initial_query");
    let settings_col = column("Settings");

    let mut seen = HashSet::new();
    let mut skipped = Skipped::default();
    let mut out = Vec::new();
    for (index, line) in lines.enumerate() {
        if line.is_empty() {
            continue;
        }
        let row = split_row(line);
        if row.len() != header.len() {
            bail!(
                "query log line {}: {} fields, the header has {}",
                index + 2,
                row.len(),
                header.len()
            );
        }
        if type_col.is_some_and(|c| row[c] == "ExceptionBeforeStart") {
            skipped.failed_before_start += 1;
            continue;
        }
        if initial_col.is_some_and(|c| row[c] == "0") {
            skipped.not_initial += 1;
            continue;
        }
        let query = &row[query_col];
        if is_harness_query(query) {
            skipped.harness += 1;
            continue;
        }
        if !seen.insert(row[id_col].clone()) {
            skipped.duplicate += 1;
            continue;
        }
        let settings = match settings_col {
            Some(c) => map_keys(&row[c]),
            None => Vec::new(),
        };
        out.push(LoggedQuery {
            id: row[id_col].clone(),
            query: query.clone(),
            settings,
        });
    }
    Ok((out, skipped))
}

/// The harness reads `system.query_log` and `system.part_log` to dump them;
/// those reads are not the application's.
fn is_harness_query(query: &str) -> bool {
    let lower = query.to_ascii_lowercase();
    lower.contains("system.query_log") || lower.contains("system.part_log")
}

/// Splits one TSV row into unescaped fields.
fn split_row(line: &str) -> Vec<String> {
    line.split('\t').map(unescape).collect()
}

/// ClickHouse's TSV escaping: `\t`, `\n`, `\\` and friends.
pub fn unescape(field: &str) -> String {
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
            Some('b') => out.push('\u{8}'),
            Some('f') => out.push('\u{c}'),
            Some('0') => out.push('\0'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// The keys of a ClickHouse `Map(String, String)` value, `{'k':'v',...}`,
/// sorted and deduplicated. A value that is not a map yields no keys.
pub fn map_keys(value: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut chars = value.trim().chars().peekable();
    if chars.next() != Some('{') {
        return keys;
    }
    // Alternate: key string, ':', value string, ',' ...
    let mut expect_key = true;
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                let mut s = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => {
                            if let Some(e) = chars.next() {
                                s.push(e);
                            }
                        }
                        '\'' => break,
                        other => s.push(other),
                    }
                }
                if expect_key {
                    keys.push(s);
                }
            }
            ':' => expect_key = false,
            ',' => expect_key = true,
            _ => {}
        }
    }
    keys.sort();
    keys.dedup();
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_required_and_optional_columns() {
        let dump = "query_id\ttype\tis_initial_query\tquery\tSettings\n\
                    a\tQueryFinish\t1\tSELECT 1\\nFROM t\t{'max_threads':'4','async_insert':'1'}\n\
                    a\tQueryFinish\t1\tSELECT 1\t{}\n\
                    b\tExceptionBeforeStart\t1\tSELEC 1\t{}\n\
                    c\tQueryFinish\t0\tSELECT 2\t{}\n\
                    d\tQueryFinish\t1\tSELECT * FROM system.query_log\t{}\n";
        let (rows, skipped) = parse(dump).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].query, "SELECT 1\nFROM t");
        assert_eq!(rows[0].settings, ["async_insert", "max_threads"]);
        assert_eq!(
            skipped,
            Skipped {
                failed_before_start: 1,
                not_initial: 1,
                duplicate: 1,
                harness: 1
            }
        );
    }

    #[test]
    fn refuses_a_dump_without_the_query_column() {
        assert!(parse("query_id\n1\n").is_err());
    }

    #[test]
    fn map_keys_handles_escapes() {
        assert_eq!(map_keys(r"{'a\'b':'x:y','c':'1'}"), ["a'b", "c"]);
        assert!(map_keys("not a map").is_empty());
    }
}
