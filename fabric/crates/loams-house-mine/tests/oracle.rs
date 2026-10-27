//! The oracle runner (HE0 Task 6): the normaliser hides only what it declares,
//! unordered results compare as multisets, and an untriaged mismatch fails the
//! run.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;

use loams_house_mine::oracle::corpus::detect_order;
use loams_house_mine::oracle::normalise::{Kind, Order, Outcome, Precision, ResultSet, compare};
use loams_house_mine::oracle::triage::{Status, Triage};
use loams_house_mine::oracle::{self, Run};
use proptest::prelude::*;
use serde_json::{Value, json};

fn rows(types: &[&str], rows: Vec<Vec<Value>>) -> Outcome {
    Outcome::Rows(ResultSet {
        names: (0..types.len()).map(|i| format!("c{i}")).collect(),
        types: types.iter().map(|t| (*t).to_string()).collect(),
        rows,
    })
}

fn kinds(a: &Outcome, b: &Outcome, order: Order) -> Vec<Kind> {
    compare(a, b, order, Precision::default())
        .into_iter()
        .map(|m| m.kind)
        .collect()
}

/// Review Focus 3: outputs that differ in row count or in types never match,
/// whatever the normaliser does with the values, in either order mode.
#[test]
fn normaliser_never_hides_row_count_or_types() {
    for order in [Order::Ordered, Order::Unordered] {
        // A duplicated row: same distinct values, one more row.
        let one = rows(&["UInt64"], vec![vec![json!("1")]]);
        let two = rows(&["UInt64"], vec![vec![json!("1")], vec![json!(1)]]);
        assert_eq!(kinds(&one, &two, order), [Kind::RowCount], "{order:?}");
        // No rows against one row.
        let none = rows(&["UInt64"], vec![]);
        assert_eq!(kinds(&none, &one, order), [Kind::RowCount], "{order:?}");
        // Equal values, different types: signedness, width, nullability,
        // low cardinality, float width.
        for (a, b, value) in [
            ("UInt64", "Int64", json!("7")),
            ("Int32", "Int64", json!(7)),
            ("Int32", "Nullable(Int32)", json!(7)),
            ("String", "LowCardinality(String)", json!("x")),
            ("Float64", "Float32", json!(0.5)),
            ("Decimal(9, 2)", "Decimal(18, 2)", json!("1.5")),
            ("DateTime", "DateTime64(3)", json!("2024-01-01 00:00:00")),
        ] {
            let x = rows(&[a], vec![vec![value.clone()]]);
            let y = rows(&[b], vec![vec![value]]);
            assert_eq!(kinds(&x, &y, order), [Kind::Types], "{a} vs {b}");
        }
        // Both at once are both reported, so triaging one never covers the other.
        let typed = rows(&["Int64"], vec![vec![json!("1")], vec![json!("1")]]);
        assert_eq!(
            kinds(&one, &typed, order),
            [Kind::Types, Kind::RowCount],
            "{order:?}"
        );
        // Different column names.
        let mut renamed = one.clone();
        if let Outcome::Rows(r) = &mut renamed {
            r.names = vec!["count()".into()];
        }
        assert_eq!(kinds(&one, &renamed, order), [Kind::Names]);
        // An error and a result, or two different error codes.
        let error = Outcome::Error {
            code: 43,
            message: "ILLEGAL_TYPE_OF_ARGUMENT".into(),
        };
        let other_error = Outcome::Error {
            code: 48,
            message: "NOT_IMPLEMENTED".into(),
        };
        assert_eq!(kinds(&one, &error, order), [Kind::Error]);
        assert_eq!(kinds(&error, &other_error, order), [Kind::Error]);
        assert!(kinds(&error, &error.clone(), order).is_empty());
    }
    // What the normaliser does hide: a quoted 64-bit integer, float noise past
    // the declared precision, decimal trailing zeros.
    let quoted = rows(
        &["UInt64", "Float64", "Decimal(9, 4)"],
        vec![vec![
            json!("18446744073709551615"),
            json!(0.1 + 0.2),
            json!("1.5000"),
        ]],
    );
    let plain = rows(
        &["UInt64", "Float64", "Decimal(9, 4)"],
        vec![vec![
            json!(18_446_744_073_709_551_615u64),
            json!(0.3),
            json!(1.5),
        ]],
    );
    assert!(kinds(&quoted, &plain, Order::Ordered).is_empty());
}

proptest! {
    /// Adding or removing any row, anywhere, is always a row-count mismatch,
    /// with or without order.
    #[test]
    fn any_added_or_dropped_row_is_a_row_count_mismatch(
        values in proptest::collection::vec(-5i64..5, 0..20),
        at in 0usize..20,
        drop in any::<bool>(),
        ordered in any::<bool>(),
    ) {
        let base: Vec<Vec<Value>> = values.iter().map(|v| vec![json!(v)]).collect();
        let mut changed = base.clone();
        if drop && !changed.is_empty() {
            changed.remove(at % changed.len());
        } else {
            let row = base.get(at % base.len().max(1)).cloned().unwrap_or_else(|| vec![json!(0)]);
            changed.insert(at.min(changed.len()), row);
        }
        let order = if ordered { Order::Ordered } else { Order::Unordered };
        let k = kinds(&rows(&["Int64"], base), &rows(&["Int64"], changed), order);
        prop_assert_eq!(k, vec![Kind::RowCount]);
    }
}

#[test]
fn unordered_results_compare_as_multisets() {
    let a = rows(
        &["String", "UInt64"],
        vec![
            vec![json!("alpha"), json!("1")],
            vec![json!("beta"), json!("2")],
            vec![json!("beta"), json!("2")],
            vec![json!("gamma"), json!("3")],
        ],
    );
    let shuffled = rows(
        &["String", "UInt64"],
        vec![
            vec![json!("beta"), json!(2)],
            vec![json!("gamma"), json!("3")],
            vec![json!("beta"), json!("2")],
            vec![json!("alpha"), json!(1)],
        ],
    );
    // The same multiset in another order: a match without ORDER BY, a values
    // mismatch with it.
    assert!(kinds(&a, &shuffled, Order::Unordered).is_empty());
    assert_eq!(kinds(&a, &shuffled, Order::Ordered), [Kind::Values]);
    // The same set with other multiplicities (one beta became gamma): a values
    // mismatch even unordered, and the detail counts the rows on each side.
    let multiplicity = rows(
        &["String", "UInt64"],
        vec![
            vec![json!("alpha"), json!("1")],
            vec![json!("beta"), json!("2")],
            vec![json!("gamma"), json!("3")],
            vec![json!("gamma"), json!("3")],
        ],
    );
    let found = compare(&a, &multiplicity, Order::Unordered, Precision::default());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].kind, Kind::Values);
    assert!(
        found[0]
            .detail
            .contains("1 rows only in the baseline, 1 only in the target"),
        "{}",
        found[0].detail
    );
    // The runner picks the mode from the query.
    assert_eq!(
        detect_order("SELECT g, count() FROM t GROUP BY g"),
        Order::Unordered
    );
    assert_eq!(
        detect_order("SELECT g, count() FROM t GROUP BY g ORDER BY g"),
        Order::Ordered
    );
}

/// A ClickHouse-like HTTP server on a loopback port that answers each POST body
/// with `answer(body)`: (status, exception code, text).
fn serve(answer: impl Fn(&str) -> (u16, Option<u32>, String) + Send + Sync + 'static) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let answer = Arc::new(answer);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let answer = Arc::clone(&answer);
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream);
                loop {
                    let mut length = 0usize;
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    loop {
                        line.clear();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 {
                            return;
                        }
                        let header = line.trim_end();
                        if header.is_empty() {
                            break;
                        }
                        if let Some((k, v)) = header.split_once(':')
                            && k.eq_ignore_ascii_case("content-length")
                        {
                            length = v.trim().parse().unwrap_or(0);
                        }
                    }
                    let mut body = vec![0; length];
                    if reader.read_exact(&mut body).is_err() {
                        return;
                    }
                    let (status, code, text) = answer(&String::from_utf8_lossy(&body));
                    let code = code
                        .map(|c| format!("X-ClickHouse-Exception-Code: {c}\r\n"))
                        .unwrap_or_default();
                    let response = format!(
                        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\n{code}\r\n{text}",
                        text.len()
                    );
                    if reader.get_mut().write_all(response.as_bytes()).is_err() {
                        return;
                    }
                }
            });
        }
    });
    format!("http://{addr}/")
}

/// Answers like ClickHouse for the one case of the test corpus; `rows` is the
/// number of rows the target returns.
fn clickhouse(rows: usize) -> impl Fn(&str) -> (u16, Option<u32>, String) + Send + Sync {
    move |body: &str| {
        if body.contains("FORMAT JSONCompactEachRowWithNamesAndTypes") {
            let mut text = String::from("[\"k\"]\n[\"UInt64\"]\n");
            for i in 0..rows {
                text.push_str(&format!("[\"{i}\"]\n"));
            }
            (200, None, text)
        } else {
            (200, None, String::new())
        }
    }
}

fn write_inputs(dir: &Path, baseline: &str, target: &str) {
    std::fs::create_dir_all(dir.join("cases")).expect("dir");
    std::fs::write(
        dir.join("setup.sql"),
        "CREATE TABLE t (k UInt64) ENGINE = MergeTree ORDER BY k;\n",
    )
    .expect("setup");
    std::fs::write(dir.join("cases/count.sql"), "SELECT k FROM t\n").expect("case");
    std::fs::write(
        dir.join("corpora.toml"),
        "[[corpus]]\nname = \"t\"\nsetup = \"setup.sql\"\ncases = \"cases\"\n",
    )
    .expect("corpora");
    std::fs::write(
        dir.join("targets.toml"),
        format!(
            "[[target]]\nname = \"baseline\"\nurl = \"{baseline}\"\nbaseline = true\n\n\
             [[target]]\nname = \"engine\"\nurl = \"{target}\"\n"
        ),
    )
    .expect("targets");
}

fn run_in(dir: &Path) -> Run {
    Run {
        corpora: dir.join("corpora.toml"),
        targets: dir.join("targets.toml"),
        triage: dir.join("triage.toml"),
        urls: Vec::new(),
        only: Vec::new(),
        cases: Vec::new(),
        check: false,
        skip_setup: false,
        task: None,
    }
}

#[test]
fn untriaged_mismatch_fails_the_run() {
    let dir = tempfile::tempdir().expect("tempdir");
    let baseline = serve(clickhouse(3));
    let engine = serve(clickhouse(2));
    write_inputs(dir.path(), &baseline, &engine);
    let run = run_in(dir.path());

    // First run: the mismatch is new, so it is written as untriaged and the run fails.
    let err = oracle::run(&run).expect_err("an untriaged mismatch fails the run");
    assert!(format!("{err:#}").contains("triage failures"), "{err:#}");
    let triage = Triage::load(&run.triage).expect("triage written");
    assert_eq!(triage.entries.len(), 1, "{triage:?}");
    let entry = &triage.entries[0];
    assert_eq!(
        (
            entry.case.as_str(),
            entry.target.as_str(),
            entry.kind,
            entry.status
        ),
        ("t/count", "engine", Kind::RowCount, Status::Untriaged)
    );

    // `--check` fails the same way and leaves the file alone.
    let before = std::fs::read_to_string(&run.triage).expect("read");
    let check = Run {
        check: true,
        ..run.clone()
    };
    assert!(oracle::run(&check).is_err());
    assert_eq!(std::fs::read_to_string(&run.triage).expect("read"), before);

    // An incompatibility without a reason still fails.
    let mut edited = triage.clone();
    edited.entries[0].status = Status::Incompatibility;
    edited.write(&run.triage).expect("write");
    let err = oracle::run(&run).expect_err("an undocumented incompatibility fails");
    assert!(format!("{err:#}").contains("triage failures"), "{err:#}");

    // Documented, the run passes, and the status survives the rewrite.
    edited.entries[0].reason = Some("the engine drops the last row (test fixture)".into());
    edited.write(&run.triage).expect("write");
    oracle::run(&run).expect("a triaged mismatch passes");
    let after = Triage::load(&run.triage).expect("triage");
    assert_eq!(after.entries[0].status, Status::Incompatibility);

    // A target that now agrees: the entry is resolved and dropped.
    let agreeing = serve(clickhouse(3));
    let fixed = Run {
        urls: vec![("engine".into(), agreeing)],
        ..run.clone()
    };
    let summary = oracle::run(&fixed).expect("no mismatch");
    assert_eq!(summary.reconciled.resolved.len(), 1);
    assert!(
        Triage::load(&run.triage)
            .expect("triage")
            .entries
            .is_empty()
    );
}

#[test]
fn an_unreachable_target_stops_the_run() {
    let dir = tempfile::tempdir().expect("tempdir");
    let baseline = serve(clickhouse(1));
    // A port nothing listens on.
    let closed = {
        let l = TcpListener::bind("127.0.0.1:0").expect("bind");
        format!("http://{}/", l.local_addr().expect("addr"))
    };
    write_inputs(dir.path(), &baseline, &closed);
    let err = oracle::run(&run_in(dir.path())).expect_err("unreachable");
    assert!(format!("{err:#}").contains("engine"), "{err:#}");
    assert!(!dir.path().join("triage.toml").exists());
}

#[test]
fn a_setup_statement_the_target_refuses_is_a_mismatch() {
    let dir = tempfile::tempdir().expect("tempdir");
    let baseline = serve(clickhouse(1));
    let refusing = serve(|body: &str| {
        if body.starts_with("CREATE") {
            (
                500,
                Some(48),
                "Code: 48. DB::Exception: not here. (NOT_IMPLEMENTED) (version 26.9.2.1)".into(),
            )
        } else {
            clickhouse(1)(body)
        }
    });
    write_inputs(dir.path(), &baseline, &refusing);
    assert!(oracle::run(&run_in(dir.path())).is_err());
    let triage = Triage::load(&dir.path().join("triage.toml")).expect("triage");
    let setup = triage
        .entries
        .iter()
        .find(|e| e.case == "t/@setup")
        .expect("the setup mismatch");
    assert_eq!(setup.kind, Kind::Error);
    assert!(setup.detail.contains("error 48"), "{}", setup.detail);
    assert!(!setup.detail.contains("version"), "{}", setup.detail);
}
