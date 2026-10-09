//! The classifier (HS1 Task 4, FL2 Task 4): which half decides a statement, what
//! it is, and what text chDB gets. The session and settings half runs against
//! real workers in `loams-house-worker/tests/sessions.rs`.

use std::path::Path;

use loams_house::classify::{Classified, OUTSIDE_SURFACE, Stmt, classify, decide_unparsed};
use loams_house_ipc::QueryClass;

fn corpus() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../conformance/clickhouse/corpus/classify")
}

/// Every file under `corpus/classify/` carries its expected kind on its first line.
#[test]
fn classify_corpus() {
    let mut seen = 0;
    let mut wrong = Vec::new();
    for entry in std::fs::read_dir(corpus()).expect("the corpus") {
        let path = entry.expect("entry").path();
        if path.extension().is_none_or(|e| e != "sql") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read");
        let (head, sql) = text.split_once('\n').expect("a kind line");
        let expected = head
            .strip_prefix("-- kind: ")
            .expect("-- kind: <Kind>")
            .trim();
        let got = match classify(sql) {
            Ok(Classified::Known { stmt, .. }) => stmt.kind().to_string(),
            Ok(Classified::Unparsed { message, .. }) => format!("Unparsed ({message})"),
            Err(err) => format!("error {err}"),
        };
        if got != expected {
            wrong.push(format!(
                "{}: expected {expected}, got {got}",
                path.display()
            ));
        }
        seen += 1;
    }
    assert!(seen >= 30, "the corpus is there: {seen}");
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn trailing_format_is_extracted() {
    match classify("SELECT 1 AS x FORMAT JSONEachRow;").expect("classifies") {
        Classified::Known {
            stmt: Stmt::Query { text },
            format,
        } => {
            assert_eq!(text, "SELECT 1 AS x");
            assert_eq!(format.as_deref(), Some("JSONEachRow"));
        }
        other => panic!("{other:?}"),
    }
    // `FORMAT` inside the statement is not the clause.
    match classify("SELECT format('{}', 1)").expect("classifies") {
        Classified::Known { format, .. } => assert_eq!(format, None),
        other => panic!("{other:?}"),
    }
}

#[test]
fn insert_format_kept() {
    match classify("INSERT INTO t (a) FORMAT CSVWithNames").expect("classifies") {
        Classified::Known {
            stmt: Stmt::Insert(insert),
            format,
        } => {
            assert_eq!(insert.format.as_deref(), Some("CSVWithNames"));
            assert_eq!(format.as_deref(), Some("CSVWithNames"));
            assert_eq!(insert.text, "INSERT INTO t (a)");
            assert_eq!(insert.table.as_deref(), Some("t"));
        }
        other => panic!("{other:?}"),
    }
}

/// A form sqlparser cannot parse is decided by ClickHouse's class: a state
/// change answers 62 with sqlparser's message and the hint; a read, or text
/// ClickHouse cannot parse either, runs unchanged (chDB then answers its own 62).
#[test]
fn unparseable_ddl_is_62_with_hint() {
    let sql = "CREATE TABLE t (a UInt64 CODEC(ZSTD(3))) ENGINE = MergeTree ORDER BY a";
    let Classified::Unparsed { text, message, .. } = classify(sql).expect("classifies") else {
        panic!("sqlparser was expected not to parse this form");
    };
    for class in [
        QueryClass::Mutating,
        QueryClass::MutatingGlobal,
        QueryClass::Control,
    ] {
        let err = decide_unparsed(text.clone(), &message, class).expect_err("refused");
        assert_eq!(err.code(), 62, "{class:?}");
        assert!(err.message().contains(OUTSIDE_SURFACE), "{err}");
        assert!(
            err.message().contains(&message),
            "sqlparser's message: {err}"
        );
    }
    for class in [QueryClass::ReadOnly, QueryClass::Unknown] {
        assert_eq!(
            decide_unparsed(text.clone(), &message, class).expect("let through"),
            Stmt::Query { text: text.clone() }
        );
    }
}

/// The text chDB runs is the input minus a trailing `FORMAT` — no other rewrite.
#[test]
fn queries_are_never_rewritten() {
    for (sql, expected) in [
        ("SELECT  1 ,2", "SELECT  1 ,2"),
        ("select /* keep */ 1 FORMAT TSV", "select /* keep */ 1"),
        (
            "WITH 'a' AS x SELECT x -- trailing",
            "WITH 'a' AS x SELECT x -- trailing",
        ),
        ("(SELECT 1)\n", "(SELECT 1)"),
        ("EXPLAIN   SELECT 1", "EXPLAIN   SELECT 1"),
    ] {
        let text = match classify(sql).expect("classifies") {
            Classified::Known {
                stmt: Stmt::Query { text } | Stmt::Explain { text },
                ..
            } => text,
            other => panic!("{sql}: {other:?}"),
        };
        assert_eq!(text, expected, "{sql}");
    }
}

#[test]
fn one_statement_per_request() {
    let err = classify("SET a = 1; SET b = 2").expect_err("two statements");
    assert_eq!(err.code(), 62);
}
