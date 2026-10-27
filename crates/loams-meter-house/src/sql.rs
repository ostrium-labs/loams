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
        create_events_pipe(),
        create_events_view(),
    ]
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
        assert!(schema[3].contains("LoamsStream('_meter', 'CloudEvents', 'house-meter')"));
        assert!(schema[1].contains("ReplacingMergeTree PARTITION BY toYYYYMM(time)"));
        assert!(schema[1].contains("ORDER BY (subject, type, source, id)"));
        assert!(schema[2].contains("ReplacingMergeTree(close_run)"));
    }
}
