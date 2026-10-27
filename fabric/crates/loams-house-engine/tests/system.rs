//! `system` tables, metadata statements and safe table functions (plan HE1
//! Task 38). The oracle job (`corpus/he1/38/`) checks the answers against the
//! pinned clickhouse-server; these tests pin them without it.

// `unwrap_used` is a warning and CI runs `-D warnings`; tests unwrap freely.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use datafusion::arrow::array::{Int64Array, RecordBatch, StringArray};
use datafusion::arrow::datatypes::Schema;
use datafusion::datasource::MemTable;
use loams_house_engine::Engine;
use loams_house_engine::catalog::{ColumnInfo, Table, TableInfo};
use loams_house_engine::system::{fill, shapes};
use loams_house_types::arrow::to_arrow;
use loams_house_types::parse;

const FORMAT: &str = "JSONCompactEachRowWithNamesAndTypes";

/// `d38.t (a Int64, s String)` with three rows, as the oracle seeds it.
fn engine() -> Engine {
    let engine = Engine::new();
    let schema = Arc::new(Schema::new(vec![
        to_arrow("a", &parse("Int64").unwrap()).unwrap(),
        to_arrow("s", &parse("String").unwrap()).unwrap(),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(Int64Array::from(vec![1, 2, 3])),
            Arc::new(StringArray::from(vec!["x", "y", "z"])),
        ],
    )
    .unwrap();
    let data = Arc::new(MemTable::try_new(schema, vec![vec![batch]]).unwrap());
    engine.catalog.insert(
        "d38",
        "t",
        Table {
            data,
            engine: "MergeTree".into(),
            sorting_key: "a".into(),
        },
    );
    engine.catalog.set_info(
        "d38",
        "t",
        TableInfo {
            engine_full: "MergeTree ORDER BY a SETTINGS index_granularity = 8192".into(),
            primary_key: "a".into(),
            create_table_query: "CREATE TABLE d38.t (`a` Int64, `s` String COMMENT 'text') \
                                 ENGINE = MergeTree ORDER BY a SETTINGS index_granularity = 8192"
                .into(),
            show_create: "CREATE TABLE d38.t\n(\n    `a` Int64,\n    `s` String COMMENT 'text'\n)\nENGINE = MergeTree\nORDER BY a\nSETTINGS index_granularity = 8192".into(),
            columns: vec![
                ColumnInfo {
                    name: "a".into(),
                    ty: "Int64".into(),
                    is_in_sorting_key: true,
                    is_in_primary_key: true,
                    ..ColumnInfo::default()
                },
                ColumnInfo {
                    name: "s".into(),
                    ty: "String".into(),
                    comment: "text".into(),
                    codec: "ZSTD(1)".into(),
                    ..ColumnInfo::default()
                },
            ],
            ..TableInfo::default()
        },
    );
    engine
}

async fn run(engine: &Engine, sql: &str) -> Result<String, loams_house_types::ChError> {
    let answer = engine
        .query(&format!("{sql} FORMAT {FORMAT}"), &[], &[], "d38")
        .await?;
    let cols = loams_house_formats::columns(&answer.schema).unwrap();
    let mut out = Vec::new();
    loams_house_formats::json_compact::write(&cols, &answer.batches, &Default::default(), &mut out)
        .unwrap();
    Ok(String::from_utf8(out).unwrap())
}

#[tokio::test]
async fn system_tables_list_the_catalog() {
    let e = engine();
    let out = run(
        &e,
        "SELECT name, engine, sorting_key, primary_key, total_rows FROM system.tables \
         WHERE database = currentDatabase() ORDER BY name",
    )
    .await
    .unwrap();
    assert_eq!(
        out,
        "[\"name\",\"engine\",\"sorting_key\",\"primary_key\",\"total_rows\"]\n\
         [\"String\",\"String\",\"String\",\"String\",\"Nullable(UInt64)\"]\n\
         [\"t\",\"MergeTree\",\"a\",\"a\",\"3\"]\n"
    );
    let out = run(
        &e,
        "SELECT name, type, position, is_in_sorting_key FROM system.columns \
         WHERE database = 'd38' AND table = 't' ORDER BY position",
    )
    .await
    .unwrap();
    assert!(
        out.ends_with("[\"a\",\"Int64\",\"1\",1]\n[\"s\",\"String\",\"2\",0]\n"),
        "{out}"
    );
    let out = run(&e, "SELECT name FROM system.databases ORDER BY name")
        .await
        .unwrap();
    for db in ["d38", "default", "system", "information_schema"] {
        assert!(out.contains(&format!("[\"{db}\"]")), "{db}: {out}");
    }
    let out = run(
        &e,
        "SELECT DISTINCT partition, rows FROM system.parts WHERE table = 't' AND database = 'd38'",
    )
    .await
    .unwrap();
    assert!(out.ends_with("[\"tuple()\",\"3\"]\n"), "{out}");
    // SigNoz's existence check and Langfuse's listing.
    let out = run(
        &e,
        "SELECT count() FROM system.tables WHERE database = 'd38' AND name = 't'",
    )
    .await
    .unwrap();
    assert!(out.ends_with("[\"1\"]\n"), "{out}");
    let out = run(
        &e,
        "SELECT name FROM system.tables WHERE database = 'd38' AND name LIKE 't%' ORDER BY name",
    )
    .await
    .unwrap();
    assert!(out.ends_with("[\"t\"]\n"), "{out}");
}

#[tokio::test]
async fn show_lists_read_system_tables() {
    let e = engine();
    let out = run(&e, "SHOW TABLES").await.unwrap();
    assert_eq!(out, "[\"name\"]\n[\"String\"]\n[\"t\"]\n");
    let out = run(&e, "SHOW TABLES FROM system LIKE 'numbers%'")
        .await
        .unwrap();
    assert!(out.ends_with("[\"numbers\"]\n[\"numbers_mt\"]\n"), "{out}");
    let out = run(&e, "SHOW DATABASES LIKE 'd3%'").await.unwrap();
    assert!(out.ends_with("[\"d38\"]\n"), "{out}");
    let err = run(&e, "SHOW TABLES FROM nope").await.unwrap_err();
    assert_eq!(err.code, 81);
}

#[tokio::test]
async fn describe_matches_the_oracle() {
    // 26.9.2.8's `DESCRIBE TABLE` of the table above, in this format.
    let e = engine();
    let out = run(&e, "DESCRIBE TABLE t").await.unwrap();
    assert_eq!(
        out,
        "[\"name\",\"type\",\"default_type\",\"default_expression\",\"comment\",\"codec_expression\",\"ttl_expression\"]\n\
         [\"String\",\"String\",\"String\",\"String\",\"String\",\"String\",\"String\"]\n\
         [\"a\",\"Int64\",\"\",\"\",\"\",\"\",\"\"]\n\
         [\"s\",\"String\",\"\",\"\",\"text\",\"ZSTD(1)\",\"\"]\n"
    );
    let out = run(&e, "DESC system.one").await.unwrap();
    assert!(
        out.ends_with("[\"dummy\",\"UInt8\",\"\",\"\",\"\",\"\",\"\"]\n"),
        "{out}"
    );
    let out = run(&e, "DESCRIBE system.tables").await.unwrap();
    assert!(
        out.contains("[\"table\",\"String\",\"ALIAS\",\"name\",\"\",\"\",\"\"]"),
        "{out}"
    );
    assert_eq!(run(&e, "DESCRIBE nope").await.unwrap_err().code, 60);
}

#[tokio::test]
async fn show_create_and_exists_answer() {
    let e = engine();
    let out = run(&e, "SHOW CREATE TABLE t").await.unwrap();
    assert!(
        out.starts_with("[\"statement\"]\n[\"String\"]\n[\"CREATE TABLE d38.t\\n("),
        "{out}"
    );
    for (sql, expected) in [
        ("EXISTS t", "1"),
        ("EXISTS TABLE d38.t", "1"),
        ("EXISTS VIEW t", "0"),
        ("EXISTS DICTIONARY t", "0"),
        ("EXISTS nope", "0"),
        ("EXISTS DATABASE d38", "1"),
        ("EXISTS DATABASE nope", "0"),
        ("EXISTS system.numbers", "1"),
    ] {
        let out = run(&e, sql).await.unwrap();
        assert_eq!(
            out,
            format!("[\"result\"]\n[\"UInt8\"]\n[{expected}]\n"),
            "{sql}"
        );
    }
    let out = run(&e, "SHOW CREATE DATABASE d38").await.unwrap();
    assert!(
        out.ends_with("[\"CREATE DATABASE d38\\nENGINE = Atomic\"]\n"),
        "{out}"
    );
}

#[tokio::test]
async fn safe_table_functions_run() {
    let e = engine();
    let out = run(&e, "SELECT number FROM numbers(3)").await.unwrap();
    assert_eq!(
        out,
        "[\"number\"]\n[\"UInt64\"]\n[\"0\"]\n[\"1\"]\n[\"2\"]\n"
    );
    let out = run(&e, "SELECT number FROM numbers(10, 10, 4)")
        .await
        .unwrap();
    assert!(out.ends_with("[\"10\"]\n[\"14\"]\n[\"18\"]\n"), "{out}");
    let out = run(&e, "SELECT number FROM system.numbers LIMIT 2")
        .await
        .unwrap();
    assert!(out.ends_with("[\"0\"]\n[\"1\"]\n"), "{out}");
    let out = run(&e, "SELECT zero FROM zeros(2)").await.unwrap();
    assert_eq!(out, "[\"zero\"]\n[\"UInt8\"]\n[0]\n[0]\n");
    let out = run(&e, "SELECT dummy FROM system.one").await.unwrap();
    assert!(out.ends_with("[0]\n"), "{out}");
    let out = run(&e, "SELECT currentDatabase(), timezone()")
        .await
        .unwrap();
    assert!(out.ends_with("[\"d38\",\"UTC\"]\n"), "{out}");
}

#[tokio::test]
async fn outside_table_functions_are_344() {
    let e = engine();
    for sql in [
        "SELECT * FROM url('http://example.test/', CSV, 'a String')",
        "SELECT * FROM file('/etc/passwd', 'LineAsString')",
        "SELECT * FROM s3('http://bucket/x', 'Parquet')",
        "SELECT * FROM remote('127.0.0.1', system.one)",
        "SELECT * FROM icebergS3('http://bucket/x')",
        "SELECT * FROM clusterAllReplicas('c', d38.t)",
        "SELECT * FROM URL('http://example.test/')",
    ] {
        assert_eq!(run(&e, sql).await.unwrap_err().code, 344, "{sql}");
    }
    // `clusterAllReplicas(c, system.t)` reads `system.t` (HE1 R9.62).
    let out = run(
        &e,
        "SELECT count() FROM clusterAllReplicas('c', system.tables) WHERE database = 'd38' AND name = 't'",
    )
    .await
    .unwrap();
    assert!(out.ends_with("[\"1\"]\n"), "{out}");
    // Others that read nothing outside are not built yet: `48`.
    assert_eq!(
        run(&e, "SELECT * FROM generateRandom('a Int8')")
            .await
            .unwrap_err()
            .code,
        48
    );
}

#[tokio::test]
async fn every_system_table_builds() {
    let e = engine();
    for (name, _, shape) in fill::SYSTEM_TABLES {
        let limit = if name.starts_with("numbers") || name.starts_with("zeros") {
            " LIMIT 1"
        } else {
            ""
        };
        let answer = e
            .query(
                &format!("SELECT * FROM system.{name}{limit}"),
                &[],
                &[],
                "d38",
            )
            .await
            .unwrap_or_else(|err| panic!("system.{name}: {err}"));
        let names: Vec<String> = answer
            .schema
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect();
        let expected: Vec<String> = shape.iter().map(|(n, _)| (*n).to_string()).collect();
        assert_eq!(names, expected, "system.{name}");
    }
}

#[tokio::test]
async fn system_functions_list_the_engine_registry() {
    let e = engine();
    let out = run(
        &e,
        "SELECT name, alias_to FROM system.functions WHERE name IN ('version', 'DATABASE', 'currentDatabase', 'toStartOfDay') ORDER BY name",
    )
    .await
    .unwrap();
    // `toStartOfDay` is not registered yet (Task 30): not offered.
    assert!(out.contains("[\"DATABASE\",\"currentDatabase\"]"), "{out}");
    assert!(out.contains("[\"version\",\"\"]"), "{out}");
    assert!(!out.contains("toStartOfDay"), "{out}");
}

/// The cluster tables' shapes are the front's (`loams-house/src/system.rs`):
/// the engine copies them because it never depends on the front.
#[test]
fn shapes_match_the_front() {
    let front = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../loams-house/src/system.rs"
    ))
    .unwrap();
    let compact = |s: &str| s.split_whitespace().collect::<String>();
    let front = compact(&front);
    for (constant, shape) in [
        ("CLUSTERS_COLUMNS", shapes::CLUSTERS),
        ("MACROS_COLUMNS", shapes::MACROS),
        ("MUTATIONS_COLUMNS", shapes::MUTATIONS),
        ("DDL_QUEUE_COLUMNS", shapes::DISTRIBUTED_DDL_QUEUE),
        ("QUERY_LOG_COLUMNS", shapes::QUERY_LOG),
    ] {
        let body: String = shape
            .iter()
            .map(|(n, t)| format!("({n:?},{t:?}),"))
            .collect();
        let start = front
            .find(&format!("pubconst{constant}:&[(&str,&str)]=&["))
            .unwrap_or_else(|| panic!("{constant} is not in the front"));
        let rest = &front[start..];
        let end = rest.find("];").unwrap();
        let list = &rest[..end];
        let list = list.split_once("=&[").unwrap().1;
        let list = if list.ends_with(',') {
            list.to_string()
        } else {
            format!("{list},")
        };
        assert_eq!(compact(&list).replace(",)", ")"), body, "{constant}");
    }
}

/// The guard's disabled table functions are the matrix's
/// (`conformance/clickhouse/matrix/classes.toml`).
#[test]
fn disabled_table_functions_match_the_matrix() {
    let classes = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../conformance/clickhouse/matrix/classes.toml"
    ))
    .unwrap();
    let mut names = Vec::new();
    let mut prefixes = Vec::new();
    let mut in_rule = false;
    for line in classes.lines() {
        let line = line.trim();
        if line == "[[rule]]" {
            in_rule = false;
        }
        if line == "kind = \"table_function\"" {
            in_rule = true;
        }
        let list = |l: &str| -> Vec<String> {
            let inner = l.split_once('[').unwrap().1.rsplit_once(']').unwrap().0;
            inner
                .split(',')
                .map(|s| s.trim().trim_matches('"').to_string())
                .filter(|s| !s.is_empty())
                .collect()
        };
        if in_rule && line.starts_with("names = ") {
            names.extend(list(line));
        }
        if in_rule && line.starts_with("prefixes = ") {
            prefixes.extend(list(line));
        }
    }
    let ours: Vec<String> = loams_house_sql::guard::DISABLED_TABLE_FUNCTIONS
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    let our_prefixes: Vec<String> = loams_house_sql::guard::DISABLED_TABLE_FUNCTION_PREFIXES
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    assert_eq!(ours, names);
    assert_eq!(our_prefixes, prefixes);
}
