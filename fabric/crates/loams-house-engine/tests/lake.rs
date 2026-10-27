//! `--lake` (plan HE1 Task 26): the lake fixtures of `corpus/he1/26` load
//! into the engine's catalog and read through the engine's front end.
//!
//! `regenerate_corpus_26_lake` (ignored; `scripts/house/lake-fixtures-26.sh`)
//! writes those fixtures: it runs the corpus's `setup.sql` on the oracle in a
//! database of its own, exports each table's rows as the seeding does, and
//! writes them with `loams-house-lake` in the House's encodings, step by step
//! as the setup made them (an `ALTER … ADD COLUMN` between two inserts, a
//! rename), so every encoding the chDB House writes is in the fixture.
//!
//! Until the fixtures are committed, the oracle job (`house-engine-oracle`)
//! writes them before it starts the dev server, then runs
//! `the_corpus_lake_loads_and_reads` (R26.8); `LOAMS_ORACLE_URL` names its
//! oracle (default `http://127.0.0.1:28123/`).

#![allow(clippy::unwrap_used)]

#[path = "../../loams-house-iceberg/tests/common/mod.rs"]
mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::{Lake, column};
use datafusion::arrow::array::ArrayRef;
use datafusion::arrow::compute::concat_batches;
use loams_house_engine::Engine;
use loams_house_iceberg::ChColumn;
use loams_house_types::ChType;

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../conformance/clickhouse/oracle/corpus/he1/26")
}

/// Every fixture table loads and reads through the engine.
#[tokio::test]
#[ignore = "needs corpus/he1/26/lake (regenerate_corpus_26_lake); the oracle job runs it"]
async fn the_corpus_lake_loads_and_reads() {
    let engine = Engine::new();
    let names = loams_house_engine::lake::attach(&engine, &corpus().join("lake"))
        .await
        .unwrap();
    for t in ["alter26", "part26", "ttl26", "types26"] {
        assert!(names.contains(&format!("default.{t}")), "{names:?}");
    }
    let answer = engine
        .query(
            "SELECT id, label, plus, n, z FROM alter26 ORDER BY id",
            &[],
            &[],
            "default",
        )
        .await
        .unwrap();
    let rows: usize = answer.batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(rows, 3);
    let answer = engine
        .query("SELECT id FROM ttl26 ORDER BY id", &[], &[], "default")
        .await
        .unwrap();
    let rows: usize = answer.batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(rows, 2, "expired rows are hidden");
    let answer = engine
        .query("SELECT * FROM types26 ORDER BY id", &[], &[], "default")
        .await
        .unwrap();
    assert_eq!(answer.schema.fields().len(), 34);
}

/// The oracle's URL: `LOAMS_ORACLE_URL`, or the local one of
/// `scripts/house/oracle.sh up`.
fn oracle_url() -> String {
    std::env::var("LOAMS_ORACLE_URL").unwrap_or_else(|_| "http://127.0.0.1:28123/".to_string())
}

const DB: &str = "he1_26_fixtures";

fn oracle(sql: &str, database: &str) -> Vec<u8> {
    let out = Command::new("curl")
        .args(["-fsS", "--data-binary", "@-", "-u", "oracle:oracle"])
        .arg(format!("{}?database={database}", oracle_url()))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(sql.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(
        out.status.success(),
        "{sql}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

/// The rows of `select` (in `database`) for the fixture's columns `names`.
fn export(lake: &Lake, select: &str) -> Vec<(String, ArrayRef)> {
    let schema = lake.metadata.current_schema().clone();
    let columns: Vec<(String, ChType)> = schema
        .as_struct()
        .fields()
        .iter()
        .map(|f| {
            let c = lake.columns.iter().find(|c| c.name == f.name).unwrap();
            (
                c.name.clone(),
                loams_house_types::parse(&c.ch_type).unwrap(),
            )
        })
        .collect();
    let bytes = oracle(&format!("{select} FORMAT Parquet"), DB);
    let (schema, batches) =
        loams_house_engine::seed::read_parquet(&columns, bytes::Bytes::from(bytes)).unwrap();
    let batch = concat_batches(&schema, &batches).unwrap();
    columns
        .iter()
        .enumerate()
        .map(|(i, (n, _))| (n.clone(), batch.column(i).clone()))
        .collect()
}

async fn insert(lake: &mut Lake, select: &str) {
    let rows = export(lake, select);
    lake.insert(rows.iter().map(|(n, a)| (n.as_str(), a.clone())).collect())
        .await;
}

/// The columns of `table` as the oracle describes them.
fn columns(table: &str) -> Vec<ChColumn> {
    let text = oracle(
        &format!(
            "SELECT name, type, default_kind, default_expression FROM system.columns \
             WHERE database = '{DB}' AND table = '{table}' ORDER BY position FORMAT JSONCompactEachRow"
        ),
        DB,
    );
    String::from_utf8(text)
        .unwrap()
        .lines()
        .map(|l| {
            let v: Vec<String> = serde_json::from_str(l).unwrap();
            ChColumn {
                name: v[0].clone(),
                ch_type: v[1].clone(),
                default_kind: v[2].clone(),
                default_expression: v[3].clone(),
            }
        })
        .collect()
}

#[tokio::test]
#[ignore = "needs the oracle (scripts/house/oracle.sh up); run by scripts/house/lake-fixtures-26.sh"]
async fn regenerate_corpus_26_lake() {
    oracle(&format!("DROP DATABASE IF EXISTS {DB}"), "default");
    oracle(&format!("CREATE DATABASE {DB}"), "default");
    let setup = std::fs::read_to_string(corpus().join("setup.sql")).unwrap();
    let text: String = setup
        .lines()
        .filter(|l| !l.trim_start().starts_with("--"))
        .collect::<Vec<_>>()
        .join("\n");
    for statement in text.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        oracle(statement, DB);
    }
    let root = corpus().join("lake");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let mut types = Lake::create(
        &root,
        "default",
        "types26",
        columns("types26"),
        "MergeTree",
        "id",
        None,
        "",
    );
    insert(&mut types, "SELECT * FROM types26 ORDER BY id").await;

    let mut plus = column("plus", "UInt64");
    plus.default_kind = "ALIAS".into();
    plus.default_expression = "id + 1".into();
    let mut alter = Lake::create(
        &root,
        "default",
        "alter26",
        vec![column("id", "UInt32"), column("s", "String"), plus],
        "MergeTree",
        "id",
        None,
        "",
    );
    insert(
        &mut alter,
        "SELECT id, label AS s FROM alter26 WHERE id <= 2 ORDER BY id",
    )
    .await;
    let mut n = column("n", "UInt32");
    n.default_kind = "DEFAULT".into();
    n.default_expression = "id * 10".into();
    alter.add_column(n);
    alter.add_column(column("z", "String"));
    alter.rename_column("s", "label");
    insert(
        &mut alter,
        "SELECT id, label, n, z FROM alter26 WHERE id = 3",
    )
    .await;

    let mut ttl = Lake::create(
        &root,
        "default",
        "ttl26",
        vec![column("id", "UInt32"), column("ts", "DateTime('UTC')")],
        "MergeTree",
        "id",
        None,
        "ts",
    );
    insert(&mut ttl, "SELECT id, ts FROM ttl26_src ORDER BY id").await;

    let mut part = Lake::create(
        &root,
        "default",
        "part26",
        columns("part26"),
        "MergeTree",
        "id",
        Some("p"),
        "",
    );
    for p in 0..4 {
        insert(
            &mut part,
            &format!("SELECT * FROM part26 WHERE p = {p} ORDER BY id"),
        )
        .await;
    }
    oracle(&format!("DROP DATABASE IF EXISTS {DB}"), "default");
}
