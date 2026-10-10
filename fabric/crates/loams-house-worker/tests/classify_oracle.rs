//! The classifier's oracle (HS1 Task 4 fix round 1): on text ClickHouse parses as
//! one statement, the front's route agrees with ClickHouse's own class
//! (`chdb_classify_query_n`, asked of a real worker). A read is `ReadOnly`, and an
//! owned write has its kind's class. The corpus (fix round 1's adversarial
//! payloads included) and statement-shaped text are both checked.

mod common;

use std::path::Path;

use loams_house::classify::{Classified, Expect, Stmt, classify, decide};
use loams_house::request::split_format;
use loams_house::{Outcome, WorkerLease};
use loams_house_ipc::QueryClass;
use proptest::prelude::*;
use proptest::test_runner::{Config, TestCaseError, TestRunner};

/// The text chDB would parse for a statement the front lets run.
fn parsed_text(sql: &str, stmt: &Stmt) -> String {
    match stmt {
        Stmt::Insert(insert) => match &insert.format {
            Some(format) => format!("{} FORMAT {format}", insert.text),
            None => insert.text.clone(),
        },
        _ => split_format(sql).0,
    }
}

/// `Err` with the disagreement, if the front's route and ClickHouse's class
/// disagree on `sql`. Text the front refuses, text sqlparser cannot parse, text
/// ClickHouse cannot parse and more than one statement are not compared.
fn check(rt: &tokio::runtime::Runtime, lease: &mut WorkerLease, sql: &str) -> Result<(), String> {
    let Ok(Classified::Known { stmt, .. }) = classify(sql) else {
        return Ok(());
    };
    let Some(expect) = stmt.expect() else {
        return Ok(());
    };
    let text = parsed_text(sql, &stmt);
    let classification = rt
        .block_on(lease.classify(&text))
        .map_err(|err| format!("{text:?}: classify: {err}"))?;
    if classification.class == QueryClass::Unknown || classification.statements != 1 {
        return Ok(());
    }
    if stmt.is_read() != (classification.class == QueryClass::ReadOnly) {
        return Err(format!(
            "{sql:?}: the front routes it as {} ({}), ClickHouse classes it {:?}",
            if stmt.is_read() { "a read" } else { "a write" },
            stmt.kind(),
            classification.class
        ));
    }
    if matches!(expect, Expect::Write(_)) {
        decide(classification, expect, false, None)
            .map_err(|err| format!("{sql:?} ({}): {err}", stmt.kind()))?;
    }
    Ok(())
}

fn worker(rt: &tokio::runtime::Runtime, test: &str) -> (loams_house::WorkerPool, WorkerLease) {
    rt.block_on(async {
        let pool = common::pool(test, common::small(1)).await;
        let lease = pool.acquire("oracle").await.expect("a worker");
        (pool, lease)
    })
}

/// Every corpus statement, the adversarial payloads included.
#[test]
fn corpus_agrees_with_clickhouse() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let (pool, mut lease) = worker(&rt, "oracle-corpus");
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../conformance/clickhouse/corpus/classify");
    let mut seen = 0;
    let mut wrong = Vec::new();
    for entry in std::fs::read_dir(corpus).expect("the corpus") {
        let path = entry.expect("entry").path();
        if path.extension().is_none_or(|e| e != "sql") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read");
        let (_, sql) = text.split_once('\n').expect("a kind line");
        if let Err(err) = check(&rt, &mut lease, sql) {
            wrong.push(err);
        }
        seen += 1;
    }
    pool.release(lease, Outcome::Completed);
    assert!(seen >= 40, "the corpus is there: {seen}");
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

fn fragment() -> impl Strategy<Value = &'static str> {
    prop_oneof![
        Just("SELECT 1"),
        Just("SELECT * FROM system.one"),
        Just("WITH 1 AS a SELECT a"),
        Just("(SELECT 1)"),
        Just("SHOW TABLES"),
        Just("SHOW DATABASES"),
        Just("DESCRIBE TABLE system.one"),
        Just("EXISTS TABLE t"),
        Just("EXPLAIN SELECT 1"),
        Just("EXPLAIN AST INSERT INTO t VALUES (1)"),
        Just("INSERT INTO t VALUES (1)"),
        Just("INSERT INTO t SELECT 1"),
        Just("INSERT INTO t FORMAT TSV"),
        Just("INSERT INTO FUNCTION null('a UInt8') SELECT 1"),
        Just("CREATE TEMPORARY TABLE t (a UInt8) ENGINE = Memory"),
        Just("CREATE TEMPORARY TABLE t ENGINE = Memory AS SELECT 1"),
        Just("DROP TEMPORARY TABLE t"),
        Just("DROP TEMPORARY TABLE IF EXISTS t"),
        Just(" PARALLEL WITH "),
        Just(" UNION ALL "),
        Just(" INTO OUTFILE 'x'"),
        Just(" FROM INFILE 'x'"),
        Just(" SETTINGS max_threads = 1"),
        Just(" FORMAT TSV"),
        Just(" WHERE 1"),
        Just(";"),
        Just(" -- c\n"),
        Just(" /* c */ "),
        Just("$$"),
        Just("'"),
        Just("("),
        Just(")"),
        Just("\n"),
    ]
}

/// Statement-shaped text: whatever ClickHouse parses as one statement, the front
/// routes the same way.
#[test]
fn statement_shaped_text_agrees_with_clickhouse() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let (pool, lease) = worker(&rt, "oracle-shaped");
    let lease = std::cell::RefCell::new(lease);
    let mut runner = TestRunner::new(Config::with_cases(1_500));
    let result = runner.run(&proptest::collection::vec(fragment(), 1..6), |parts| {
        check(&rt, &mut lease.borrow_mut(), &parts.concat()).map_err(TestCaseError::fail)
    });
    pool.release(lease.into_inner(), Outcome::Completed);
    if let Err(err) = result {
        panic!("{err}");
    }
}
