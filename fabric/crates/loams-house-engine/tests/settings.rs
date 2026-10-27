//! Settings on the Rust engine (plan HE1 Task 37, D1805). The full check of
//! the honoured settings is the oracle corpus `he1-37`; these tests pin the
//! classing and the engine's side without the oracle.

// `unwrap_used` is a warning and CI runs `-D warnings`; tests unwrap freely.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use datafusion::arrow::array::{ArrayRef, Float64Array, Int64Array, RecordBatch, StringArray};
use datafusion::arrow::compute::cast;
use datafusion::arrow::datatypes::Schema;
use datafusion::datasource::MemTable;
use loams_house_engine::Engine;
use loams_house_engine::catalog::Table;
use loams_house_engine::settings::{Class, Lookup, lookup, table};
use loams_house_ipc::policy::{
    ALLOWED_EXPERIMENTS, ALLOWED_PREFIXES, ALLOWED_SETTINGS, CLIENT_SETTINGS, HOUSE_SETTINGS,
    chdb_defaults,
};
use loams_house_types::arrow::to_arrow;
use loams_house_types::parse;

/// The front's `SATISFIED_SETTINGS` (`loams-house/src/settings.rs`), copied:
/// the engine never depends on the front. `front_lists_stay_in_sync` reads
/// the front's text.
const SATISFIED_SETTINGS: &[&str] = &[
    "mutations_sync",
    "alter_sync",
    "lightweight_deletes_sync",
    "allow_experimental_lightweight_update",
    "parallel_view_processing",
    "enable_full_text_index",
    "flatten_nested",
    "materialize_ttl_after_modify",
];

/// The front's `FRONT_SETTINGS`, copied likewise.
const FRONT_SETTINGS: &[&str] = &[
    "insert_deduplication_token",
    "materialized_views_ignore_errors",
];

/// The front's `LOAMS_SETTINGS`, copied likewise.
const LOAMS_SETTINGS: &[&str] = &[
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

/// The quoted names of `pub const NAME: &[&str] = &[ … ];` in `source`.
fn const_list(source: &str, name: &str) -> Vec<String> {
    let start = source
        .find(&format!("pub const {name}: &[&str] = &["))
        .unwrap_or_else(|| panic!("{name} is not in the front's settings.rs"));
    let body = &source[start..];
    let body = &body[body.find("&[").unwrap() + 2..body.find("];").unwrap()];
    body.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with("//"))
        .filter_map(|l| {
            let l = l.strip_prefix('"')?;
            Some(l[..l.find('"')?].to_string())
        })
        .collect()
}

#[test]
fn front_lists_stay_in_sync() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../loams-house/src/settings.rs"
    );
    let source = std::fs::read_to_string(path).unwrap();
    for (name, ours) in [
        ("SATISFIED_SETTINGS", SATISFIED_SETTINGS),
        ("FRONT_SETTINGS", FRONT_SETTINGS),
        ("LOAMS_SETTINGS", LOAMS_SETTINGS),
    ] {
        let theirs = const_list(&source, name);
        let ours: Vec<String> = ours.iter().map(|s| (*s).to_string()).collect();
        assert_eq!(
            theirs, ours,
            "the front's {name} changed: copy it here and class every new name in settings.tsv"
        );
    }
}

/// Every setting the front accepts has a class here: its lists, the names it
/// keeps, every chDB setting under an allowed prefix, and every chDB setting
/// at all (R9.11 accepts any at its default). It fails when the front adds a
/// setting the engine has not classed.
#[test]
fn every_front_setting_is_classed_by_the_engine() {
    let mut named: Vec<&str> = Vec::new();
    named.extend(ALLOWED_SETTINGS);
    named.extend(CLIENT_SETTINGS);
    named.extend(ALLOWED_EXPERIMENTS);
    named.extend(HOUSE_SETTINGS);
    named.extend(SATISFIED_SETTINGS);
    named.extend(FRONT_SETTINGS);
    named.extend(LOAMS_SETTINGS);
    named.push("final");
    for name in named {
        assert!(
            matches!(lookup(name), Lookup::Spec(_)),
            "{name} is on the front's lists but has no row in settings.tsv"
        );
    }
    for name in chdb_defaults().keys() {
        let by_prefix = ALLOWED_PREFIXES.iter().any(|p| name.starts_with(p));
        match lookup(name) {
            Lookup::Spec(_) => {}
            Lookup::Known(_) => {
                assert!(!by_prefix, "{name} is allowed by a prefix but has no class")
            }
            Lookup::Unknown => panic!("{name} is chDB's but unknown to the engine"),
        }
    }
}

/// A refused or not-yet-honoured setting is an error, never a silent answer.
#[tokio::test]
async fn refused_settings_are_never_ignored() {
    let engine = seeded();
    // Each non-default value of a setting that changes answers, where it
    // applies, is refused.
    for (sql, code) in [
        ("SELECT id FROM t37 SETTINGS use_query_cache = 1", 164),
        ("SELECT id FROM t37 SETTINGS use_skip_indexes = 0", 164),
        ("SELECT id FROM t37 SETTINGS no_such_setting = 0", 115),
        (
            "SELECT id FROM t37 SETTINGS prefer_column_name_to_alias = 1",
            48,
        ),
        ("SELECT id FROM t37 SETTINGS extremes = 1", 48),
        (
            "SELECT id FROM t37 SETTINGS output_format_pretty_max_rows = 5",
            48,
        ),
        ("SELECT id FROM t37 SETTINGS max_rows_to_read = 1", 48),
        ("SELECT id FROM r37 SETTINGS final = 1", 48),
        (
            "SELECT id FROM t37 WHERE ts > '2026-01-01 00:00:00' SETTINGS session_timezone = 'Asia/Kolkata'",
            48,
        ),
        ("SELECT id FROM t37 SETTINGS max_result_rows = 1", 396),
        (
            "SELECT id FROM t37 SETTINGS max_result_rows = 1, result_overflow_mode = 'break'",
            48,
        ),
        ("SELECT id FROM t37 UNION SELECT id FROM t37", 559),
        (
            "SELECT id FROM t37 UNION SELECT id FROM t37 SETTINGS union_default_mode = 'ALL'",
            48,
        ),
        (
            "SELECT id FROM t37 SETTINGS date_time_output_format = 'fancy'",
            36,
        ),
    ] {
        let e = engine.query(sql, &[], &[], "default").await.unwrap_err();
        assert_eq!(e.code, code, "{sql}: {e}");
    }
    // The same from the request.
    let e = engine
        .query(
            "SELECT id FROM t37",
            &[],
            &[("use_query_cache".into(), "1".into())],
            "default",
        )
        .await
        .unwrap_err();
    assert_eq!(e.code, 164);
    // Every honoured or default-only row is still a row: none is silently
    // dropped from the table.
    assert!(table().iter().any(|r| r.class == Class::DefaultOnly));
    assert!(table().iter().any(|r| r.class == Class::Refused));
}

fn column(t: &str, values: ArrayRef) -> (datafusion::arrow::datatypes::Field, ArrayRef) {
    let field = to_arrow("v", &parse(t).unwrap()).unwrap();
    let array = cast(&values, field.data_type()).unwrap();
    (field, array)
}

fn table_of(columns: Vec<(&str, &str, ArrayRef)>) -> Arc<MemTable> {
    let mut fields = Vec::new();
    let mut arrays = Vec::new();
    for (name, t, values) in columns {
        let (field, array) = column(t, values);
        fields.push(field.with_name(name));
        arrays.push(array);
    }
    let schema = Arc::new(Schema::new(fields));
    let batch = RecordBatch::try_new(schema.clone(), arrays).unwrap();
    Arc::new(MemTable::try_new(schema, vec![vec![batch]]).unwrap())
}

/// The engine over `t37` (the oracle corpus's table) and `r37`, a
/// `ReplacingMergeTree`.
fn seeded() -> Engine {
    let engine = Engine::new();
    let t37 = table_of(vec![
        ("id", "Int64", Arc::new(Int64Array::from(vec![1, 2]))),
        (
            "ts",
            "DateTime",
            Arc::new(Int64Array::from(vec![1_792_697_523, 0])),
        ),
        (
            "big",
            "Int64",
            Arc::new(Int64Array::from(vec![9_007_199_254_740_993, -1])),
        ),
        (
            "f",
            "Float64",
            Arc::new(Float64Array::from(vec![f64::NAN, 1.5])),
        ),
        ("s", "String", Arc::new(StringArray::from(vec!["a/b", "c"]))),
    ]);
    engine.catalog.insert(
        "default",
        "t37",
        Table {
            data: t37,
            engine: "MergeTree".into(),
            sorting_key: "id".into(),
        },
    );
    let r37 = table_of(vec![(
        "id",
        "Int64",
        Arc::new(Int64Array::from(vec![1, 1])),
    )]);
    engine.catalog.insert(
        "default",
        "r37",
        Table {
            data: r37,
            engine: "ReplacingMergeTree".into(),
            sorting_key: "id".into(),
        },
    );
    engine
}

async fn text(engine: &Engine, sql: &str) -> String {
    let answer = engine.query(sql, &[], &[], "default").await.unwrap();
    let cols = loams_house_formats::columns(&answer.schema).unwrap();
    let mut out = Vec::new();
    loams_house_formats::json_compact::write(
        &cols,
        &answer.batches,
        &answer.settings.text(),
        &mut out,
    )
    .unwrap();
    String::from_utf8(out).unwrap()
}

/// The honoured settings change the answer as ClickHouse documents; the
/// oracle corpus `he1-37` compares the same statements with 26.9.2.8.
#[tokio::test]
async fn honoured_settings_match_the_oracle() {
    let engine = seeded();
    let base = "SELECT ts FROM t37 WHERE id = 1";
    assert!(
        text(&engine, base)
            .await
            .ends_with("[\"2026-10-22 19:32:03\"]\n")
    );
    let t = text(
        &engine,
        &format!("{base} SETTINGS session_timezone = 'Asia/Kolkata'"),
    )
    .await;
    assert!(t.ends_with("[\"2026-10-23 01:02:03\"]\n"), "{t}");
    let t = text(
        &engine,
        &format!("{base} SETTINGS date_time_output_format = 'iso'"),
    )
    .await;
    assert!(t.ends_with("[\"2026-10-22T19:32:03Z\"]\n"), "{t}");
    let t = text(
        &engine,
        &format!("{base} SETTINGS date_time_output_format = 'unix_timestamp'"),
    )
    .await;
    assert!(t.ends_with("[\"1792697523\"]\n"), "{t}");

    let big = "SELECT big FROM t37 WHERE id = 1";
    assert!(text(&engine, big).await.ends_with("[9007199254740993]\n"));
    let t = text(
        &engine,
        &format!("{big} SETTINGS output_format_json_quote_64bit_integers = 1"),
    )
    .await;
    assert!(t.ends_with("[\"9007199254740993\"]\n"), "{t}");

    let f = "SELECT f FROM t37 WHERE id = 1";
    assert!(text(&engine, f).await.ends_with("[null]\n"));
    let t = text(
        &engine,
        &format!("{f} SETTINGS output_format_json_quote_denormals = 1"),
    )
    .await;
    assert!(t.ends_with("[\"nan\"]\n"), "{t}");

    let s = "SELECT s FROM t37 WHERE id = 1";
    assert!(text(&engine, s).await.ends_with("[\"a\\/b\"]\n"));
    let t = text(
        &engine,
        &format!("{s} SETTINGS output_format_json_escape_forward_slashes = 0"),
    )
    .await;
    assert!(t.ends_with("[\"a/b\"]\n"), "{t}");

    // A bare UNION under 'DISTINCT' is DataFusion's own.
    let t = text(
        &engine,
        "SELECT id FROM (SELECT id FROM t37 UNION SELECT id FROM t37) ORDER BY id \
         SETTINGS union_default_mode = 'DISTINCT'",
    )
    .await;
    assert!(t.ends_with("[1]\n[2]\n"), "{t}");

    // Settings that ask for nothing here are accepted.
    for sql in [
        "SELECT id FROM t37 ORDER BY id SETTINGS join_use_nulls = 1",
        "SELECT id FROM t37 ORDER BY id SETTINGS final = 1",
        "SELECT id FROM t37 ORDER BY id SETTINGS max_threads = 4, max_block_size = 1",
        "SELECT id FROM t37 ORDER BY id SETTINGS log_comment = 'x', max_execution_time = 60",
        "SELECT id FROM t37 ORDER BY id SETTINGS aggregate_functions_null_for_empty = 1",
        "SELECT id FROM t37 ORDER BY id SETTINGS max_result_rows = 2",
        "SELECT id FROM t37 ORDER BY id SETTINGS max_memory_usage = 1000000000",
    ] {
        let t = text(&engine, sql).await;
        assert!(t.ends_with("[1]\n[2]\n"), "{sql}: {t}");
    }
}
