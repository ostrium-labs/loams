//! The classifier (HS1 Task 4, FL2 Task 4): which half decides a statement, what
//! it is, and what text chDB gets. The session and settings half runs against
//! real workers in `loams-house-worker/tests/sessions.rs`.

use std::path::Path;

use loams_house::classify::{
    Classified, Expect, OUTSIDE_SURFACE, Stmt, check_text, classify, decide, decide_unparsed,
};
use loams_house_ipc::{Classification, QueryClass};

fn corpus() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../conformance/clickhouse/corpus/classify")
}

/// Every file under `corpus/classify/` carries its expected kind on its first line:
/// a `Stmt` kind, `Unparsed (…)`, or `error <code>` for a refusal (fix round 1's
/// adversarial payloads, `adversarial_*.sql`).
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
            Err(err) if expected.starts_with("error ") => format!("error {}", err.code()),
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
/// change, or text ClickHouse cannot classify either (fix round 1, I2), answers 62
/// with sqlparser's message and the hint; a read runs unchanged.
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
    assert_eq!(
        decide_unparsed(text.clone(), &message, QueryClass::ReadOnly).expect("a read"),
        Stmt::Query { text: text.clone() }
    );
    // Fix round 1, I2: what ClickHouse cannot parse either is refused too.
    let err = decide_unparsed(text.clone(), &message, QueryClass::Unknown).expect_err("unknown");
    assert_eq!(err.code(), 62);
    assert!(err.message().contains(OUTSIDE_SURFACE), "{err}");
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
    // Fix round 1, I1: keyword-routed statements too, and wherever ClickHouse's
    // lexer sees the `;`.
    for sql in [
        "DROP TEMPORARY TABLE t; DROP TABLE x",
        "DROP TEMPORARY TABLE t;DROP TABLE x;",
        "SELECT 1; SELECT 2",
        "SELECT 1;;SELECT 2",
        "WITH 1 AS a SELECT a; SYSTEM SHUTDOWN",
        "(SELECT 1); CREATE DATABASE d",
        "SHOW TABLES; CREATE DATABASE d",
        "DESCRIBE t; CREATE DATABASE d",
        "EXISTS t; CREATE DATABASE d",
        "EXPLAIN SELECT 1; CREATE DATABASE d",
        "KILL QUERY WHERE query_id = 'x'; CREATE DATABASE d",
        "UNDROP TABLE t; CREATE DATABASE d",
        "INSERT INTO t VALUES (1); CREATE DATABASE d",
        "SELECT $$'$$; CREATE DATABASE d --'",
        "SELECT 1 /* /* */ ; */ ; CREATE DATABASE d",
        "SELECT 1 #x\n; CREATE DATABASE d",
    ] {
        let err = classify(sql).expect_err(sql);
        assert_eq!(err.code(), 62, "{sql}: {err}");
        assert!(err.message().contains("Multi-statements"), "{sql}: {err}");
    }
    for sql in [
        "SELECT 1;",
        "SELECT 1 ; ;",
        "SELECT 1; -- a comment",
        "SELECT 1; /* a comment */",
        "SELECT ';'",
        "SELECT $$;$$",
        "SELECT 1 -- ; SELECT 2",
        "SELECT `a;b`",
        "DROP TEMPORARY TABLE t;",
    ] {
        classify(sql).unwrap_or_else(|err| panic!("{sql}: {err}"));
    }
}

/// HS1 Task 4 fix round 1, C1 and C2: chDB's client layer writes `INTO OUTFILE`
/// and reads `FROM INFILE` on the host whatever `readonly` and the grants say
/// (R1.10), so no statement of any kind may carry either: `344`.
#[test]
fn host_file_io_is_344_everywhere() {
    for sql in [
        "SELECT 1 INTO OUTFILE '/abs/path/out.tsv' APPEND",
        "SELECT 1 INTO OUTFILE '/abs/path/out.tsv' TRUNCATE FORMAT CSV",
        "select 1 into /* a comment */ outfile 'x.tsv'",
        "WITH 1 AS a SELECT a INTO OUTFILE 'x'",
        "(SELECT 1) INTO OUTFILE 'x'",
        "SHOW TABLES INTO OUTFILE '/abs/path/tables.tsv'",
        "SHOW CREATE TABLE system.one INTO OUTFILE 'x'",
        "DESCRIBE TABLE system.one INTO OUTFILE '/abs/path/d.tsv'",
        "DESC system.one INTO OUTFILE 'x'",
        "EXPLAIN SELECT 1 INTO OUTFILE '/abs/path/e.tsv'",
        "EXISTS TABLE t INTO OUTFILE 'x'",
        "INSERT INTO t FROM INFILE '/etc/passwd' FORMAT CSV",
        "INSERT INTO t FROM INFILE '/etc/passwd' COMPRESSION 'gzip' FORMAT TSV",
        "insert into t from infile 'data.csv'",
        "INSERT INTO t SELECT * FROM numbers(1) INTO OUTFILE 'x'",
        "CREATE TEMPORARY TABLE t ENGINE = Memory AS SELECT 1 INTO OUTFILE 'x'",
        "SELEC 1 INTO OUTFILE 'x'",
        // ClickHouse's lexer, not the request scanner's (both measured on chDB):
        // a heredoc hides a quote, Unicode spaces are whitespace, comments nest.
        "SELECT $$'$$ INTO OUTFILE '/abs/path/h.tsv' --'",
        "SELECT $tag$'$tag$ INTO OUTFILE 'x' --'",
        "INSERT INTO FUNCTION null($$'$$) FROM INFILE '/etc/passwd' --')",
        "INSERT INTO t FROM INFILE $$/etc/passwd$$ FORMAT CSV",
        "SELECT 1 INTO\u{a0}OUTFILE 'x'",
        "SELECT 1 INTO\u{3000}OUTFILE 'x'",
        "SELECT 1 /* a /* nested */ comment */ INTO OUTFILE 'x'",
        "SELECT 1 #x\nINTO OUTFILE 'x'",
    ] {
        let err = classify(sql).expect_err(sql);
        assert_eq!(err.code(), 344, "{sql}: {err}");
    }
    // The words inside strings, comments or quoted names are not the clauses.
    for sql in [
        "SELECT 'INTO OUTFILE x'",
        "SELECT 1 -- INTO OUTFILE 'x'",
        "SELECT `into` FROM `outfile`",
        "SELECT * FROM `infile`",
        "SELECT $$INTO OUTFILE$$",
        "SELECT 1 /* /* */ INTO OUTFILE 'x' */",
        "SELECT 1 # INTO OUTFILE 'x'",
        "SELECT 1 #!INTO OUTFILE 'x'",
    ] {
        classify(sql).unwrap_or_else(|err| panic!("{sql}: {err}"));
    }
    // The INSERT head the front streams a body into is checked the same way.
    assert_eq!(
        check_text("INSERT INTO t FROM INFILE '/etc/passwd'")
            .expect_err("infile")
            .code(),
        344
    );
}

fn class(class: QueryClass, statements: u32) -> Classification {
    Classification { class, statements }
}

/// Fix round 1, C1: ClickHouse's class gates every statement the front sends to
/// chDB as text. On a read-only path (GET, read-only users) anything but
/// `ReadOnly` is refused; on any path, a statement routed as a read that ClickHouse
/// says changes state is refused.
#[test]
fn clickhouse_class_gates_what_runs() {
    decide(class(QueryClass::ReadOnly, 1), Expect::Read, true, None).expect("a read");
    decide(class(QueryClass::ReadOnly, 1), Expect::Read, false, None).expect("a read");
    for c in [
        QueryClass::Mutating,
        QueryClass::MutatingGlobal,
        QueryClass::Control,
    ] {
        assert_eq!(
            decide(class(c, 1), Expect::Read, true, None)
                .expect_err("read-only")
                .code(),
            164,
            "{c:?}"
        );
        let err = decide(class(c, 1), Expect::Read, false, None).expect_err("not a read");
        assert_eq!(err.code(), 62, "{c:?}");
        assert!(err.message().contains(OUTSIDE_SURFACE), "{err}");
    }
    // An owned write must be what ClickHouse calls a write, and nothing more.
    for (sql, wanted) in [
        (
            "INSERT INTO t VALUES (1)",
            &[QueryClass::Mutating, QueryClass::Control][..],
        ),
        (
            "INSERT INTO FUNCTION null('a UInt8') SELECT 1",
            &[QueryClass::Mutating, QueryClass::Control][..],
        ),
        (
            "CREATE TEMPORARY TABLE t (a UInt8) ENGINE = Memory",
            &[QueryClass::Control][..],
        ),
        ("DROP TEMPORARY TABLE t", &[QueryClass::Control][..]),
    ] {
        let Ok(Classified::Known { stmt, .. }) = classify(sql) else {
            panic!("{sql}");
        };
        let expect = stmt.expect().expect("runs in chDB");
        assert_eq!(expect, Expect::Write(wanted), "{sql}");
        for c in [
            QueryClass::ReadOnly,
            QueryClass::Mutating,
            QueryClass::MutatingGlobal,
            QueryClass::Control,
        ] {
            if wanted.contains(&c) {
                decide(class(c, 1), expect, false, None).expect("the write");
                // PARALLEL WITH's arms count as statements.
                assert_eq!(
                    decide(class(c, 2), expect, false, None)
                        .expect_err("two")
                        .code(),
                    62
                );
                continue;
            }
            let err = decide(class(c, 1), expect, false, None).expect_err("not the write");
            assert_eq!(err.code(), 62, "{sql} {c:?}");
            assert!(err.message().contains(OUTSIDE_SURFACE), "{err}");
        }
    }
}

/// Fix round 1, I1 and I2: more than one statement by ClickHouse's count is `62`,
/// and so is what ClickHouse cannot parse (with the hint), on every path —
/// read-only ones included — and for either expectation.
#[test]
fn multi_statement_and_unknown_are_refused() {
    for expect in [
        Expect::Read,
        Expect::Write(&[QueryClass::Mutating, QueryClass::Control]),
        Expect::Write(&[QueryClass::Control]),
    ] {
        for readonly in [true, false] {
            for c in [QueryClass::ReadOnly, QueryClass::Mutating] {
                let err = decide(class(c, 2), expect, readonly, None).expect_err("two statements");
                assert_eq!(err.code(), 62);
                assert!(err.message().contains("Multi-statements"), "{err}");
            }
            let err =
                decide(class(QueryClass::Unknown, 0), expect, readonly, None).expect_err("unknown");
            assert_eq!(err.code(), 62, "{expect:?} {readonly}");
            assert!(err.message().contains(OUTSIDE_SURFACE), "{err}");
            let err = decide(
                class(QueryClass::Unknown, 0),
                expect,
                readonly,
                Some("sqlparser's"),
            )
            .expect_err("unknown");
            assert!(err.message().starts_with("sqlparser's"), "{err}");
        }
    }
}

/// Fix round 1, the classifier oracle's finding: the keyword routes go by
/// ClickHouse's lexing, so a heredoc or a nested comment cannot show the route one
/// statement while chDB parses another.
#[test]
fn routes_follow_clickhouse_lexing() {
    let route = |sql: &str| match classify(sql) {
        Ok(Classified::Known { stmt, .. }) => stmt.kind().to_string(),
        Ok(Classified::Unparsed { .. }) => "Unparsed".to_string(),
        Err(err) => format!("error {}", err.code()),
    };
    // A heredoc is a string to ClickHouse: no INSERT here.
    assert_ne!(route("$$INSERT INTO t VALUES (1)$$"), "Insert");
    // Comments nest: what follows the outer comment is the statement.
    assert_eq!(
        route("/* /* */ INSERT INTO t VALUES (1) */ SELECT 1"),
        "Query"
    );
    assert_eq!(
        route("/* /* */ SELECT 1 */ INSERT INTO t VALUES (1)"),
        "Insert"
    );
    // `#` before a space is a comment.
    assert_eq!(route("# INSERT INTO t\nSELECT 1"), "Query");
}
