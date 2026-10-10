//! Statement classification and the gate in front of the engine (GR1 Task 3; §48 §11.2; R0.8,
//! R0.10, R0.11), measured against `conformance/graph/gql/classify/*.gql`.

use std::path::PathBuf;

use loams_graph::classify::{Access, classify, engine_classify, gate};
use loams_graph::{Engine, Graph, GraphError, OpenSpec};
use loams_proto::loams::graph::v1::QueryLanguage;

/// One corpus case.
#[derive(Debug)]
struct Case {
    file: String,
    statement: String,
    engine: Option<Access>,
    refused: Option<String>,
    parse_error: bool,
    conservative: bool,
}

fn corpus() -> Vec<Case> {
    let dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../conformance/graph/gql/classify");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("the corpus directory")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "gql"))
        .collect();
    files.sort();
    let mut cases = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(&path).expect("a corpus file");
        let file = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        for block in text.split("\n\n").filter(|b| !b.trim().is_empty()) {
            let mut case = Case {
                file: file.clone(),
                statement: String::new(),
                engine: None,
                refused: None,
                parse_error: false,
                conservative: false,
            };
            let mut body = Vec::new();
            for line in block.lines() {
                match line.strip_prefix("#! ") {
                    Some(meta) if body.is_empty() => match meta.trim() {
                        "conservative" => case.conservative = true,
                        "parse_error" => case.parse_error = true,
                        "engine=read" => case.engine = Some(Access::Read),
                        "engine=write" => case.engine = Some(Access::Write),
                        "engine=admin" => case.engine = Some(Access::Admin),
                        other => match other.strip_prefix("refused=") {
                            Some(reason) => case.refused = Some(reason.to_string()),
                            None => panic!("{file}: unknown metadata {other:?}"),
                        },
                    },
                    _ => body.push(line),
                }
            }
            case.statement = body.join("\n");
            assert!(
                case.engine.is_some() || case.refused.is_some() || case.parse_error,
                "{file}: a case without an expectation: {block}"
            );
            cases.push(case);
        }
    }
    assert!(cases.len() >= 50, "only {} cases", cases.len());
    cases
}

#[test]
fn guard_and_engine_agree_on_corpus() {
    let cases = corpus();
    let files: std::collections::BTreeSet<&str> = cases.iter().map(|c| c.file.as_str()).collect();
    assert_eq!(
        files.into_iter().collect::<Vec<_>>(),
        ["ddl.gql", "reads.gql", "refused.gql", "writes.gql"],
        "every corpus file is read"
    );
    for case in cases {
        let guard = classify(&case.statement, QueryLanguage::Gql);
        match (&case.engine, &case.refused, case.parse_error) {
            (Some(expected), None, false) => {
                let engine = engine_classify(&case.statement)
                    .unwrap_or_else(|err| panic!("{case:?}: {err}"));
                assert_eq!(engine, *expected, "{case:?}");
                // The safety property: the guard never calls a write a read.
                if engine != Access::Read {
                    assert_ne!(guard, Access::Read, "{case:?}: the guard says Read");
                }
                // And the list of conservative disagreements is exact.
                assert_eq!(
                    guard > engine,
                    case.conservative,
                    "{case:?}: guard {guard:?}, engine {engine:?}"
                );
                assert_eq!(
                    gate(&case.statement, QueryLanguage::Gql).ok(),
                    Some(guard.max(engine))
                );
            }
            (None, Some(reason), false) => {
                let err = gate(&case.statement, QueryLanguage::Gql)
                    .expect_err(&format!("{case:?} is refused"));
                assert_eq!(err.reason(), reason.as_str(), "{case:?}: {err}");
            }
            (None, None, true) => {
                let err = engine_classify(&case.statement).expect_err("does not parse");
                assert!(
                    matches!(err, GraphError::Engine { .. }),
                    "{case:?}: {err:?}"
                );
                // The guard still says Write for anything it cannot read as a read.
                assert_ne!(guard, Access::Read, "{case:?}");
            }
            _ => panic!("{case:?}: one expectation per case"),
        }
    }
}

#[test]
fn read_session_refuses_every_corpus_write() {
    let engine = Engine::new();
    let graph = Graph::open(&engine, "acme", "ro", OpenSpec::default()).expect("open");
    graph
        .session_for(Access::Write)
        .execute("INSERT (:Person {name: 'a'})-[:KNOWS]->(:Person {name: 'b'})")
        .expect("seed");
    let state = |g: &Graph| {
        let rows = g
            .session_for(Access::Read)
            .execute("MATCH (n) RETURN count(n) AS c")
            .expect("count");
        (format!("{:?}", rows.rows()[0][0]), g.current_epoch())
    };
    let mut checked = 0;
    for case in corpus() {
        if !matches!(case.engine, Some(Access::Write | Access::Admin)) {
            continue;
        }
        let before = state(&graph);
        // Loams's own execution path, with the access forced to Read so the gate is skipped and
        // only the engine role stands between the statement and the graph (I4).
        let result = graph.execute_forced(
            Access::Read,
            &case.statement,
            std::collections::HashMap::new(),
        );
        // A built-in procedure that only reads is allowed by the ReadOnly role; the gate files
        // every CALL as a write anyway (I1). It must still change nothing.
        let read_only_procedure = case.statement.trim_start().starts_with("CALL grafeo.");
        if !read_only_procedure {
            assert!(result.is_err(), "{case:?}: a ReadOnly session ran it");
        }
        assert_eq!(state(&graph), before, "{case:?}: it changed the graph");
        checked += 1;
    }
    assert!(checked >= 20, "only {checked} writes");
}

/// The security finding of Task 0 (R0.11): `LOAD DATA` reads any server file, even on a read-only
/// session. Every execution path refuses it before the engine runs anything.
#[test]
fn load_data_never_reads_a_file() {
    let dir = std::env::temp_dir().join(format!("loams-graph-load-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let secret = dir.join("secret.csv");
    std::fs::write(&secret, "name\nhunter2\n").expect("write");
    let engine = Engine::new();
    let graph = Graph::open(&engine, "acme", "load", OpenSpec::default()).expect("open");
    let statements = [
        format!(
            "LOAD DATA FROM '{}' FORMAT CSV WITH HEADERS AS row RETURN row.name",
            secret.display()
        ),
        format!(
            "load data from '{}' format csv as row return row",
            secret.display()
        ),
        format!(
            "MATCH (n) RETURN n NEXT LOAD DATA FROM '{}' FORMAT JSONL AS r RETURN r",
            secret.display()
        ),
        format!(
            "LOAD DATA FROM '{}' FORMAT CSV AS row INSERT (:Leak {{v: row}})",
            secret.display()
        ),
        // A statement the translator cannot parse still meets the keyword backstop.
        format!(
            "LOAD DATA FROM '{}' FORMAT CSV AS row RETURN row // trailing",
            secret.display()
        ),
    ];
    for statement in &statements {
        for read_only in [false, true] {
            let err = graph
                .execute(statement, read_only)
                .expect_err("LOAD DATA is refused");
            assert_eq!(
                err.reason(),
                "graph_statement_not_allowed",
                "{statement}: {err}"
            );
            assert!(!err.to_string().contains("hunter2"), "{err}");
        }
        let err = graph
            .execute_batch(&[loams_graph::BatchStatement::text(statement.clone())])
            .expect_err("and in a batch");
        assert_eq!(err.reason(), "graph_statement_not_allowed");
    }
    // The translator's plan walk refuses it on its own, not only the keyword backstop.
    for statement in &statements[..4] {
        let err = engine_classify(statement).expect_err("the plan has a LoadData operator");
        assert!(
            matches!(
                err,
                GraphError::StatementNotAllowed {
                    file_access: true,
                    ..
                }
            ),
            "{statement}: {err:?}"
        );
    }
    let count = graph
        .execute("MATCH (l:Leak) RETURN count(l) AS c", true)
        .expect("count");
    assert_eq!(count.rows[0].values[0], grafeo::Value::Int64(0));
    std::fs::remove_dir_all(&dir).ok();
}

/// One Loams graph is one Grafeo database's default graph (R0.10 (b)).
#[test]
fn graph_management_statements_are_refused() {
    let engine = Engine::new();
    let graph = Graph::open(&engine, "acme", "mgmt", OpenSpec::default()).expect("open");
    for statement in [
        "CREATE GRAPH other",
        "CREATE GRAPH IF NOT EXISTS other",
        "DROP GRAPH IF EXISTS other",
        "USE GRAPH other",
        "SESSION SET GRAPH other",
        "CREATE PROJECTION p LABELS (Person) EDGE_TYPES (KNOWS)",
        "DROP PROJECTION p",
    ] {
        let err = graph.execute(statement, false).expect_err("refused");
        assert_eq!(
            err.reason(),
            "graph_statement_not_allowed",
            "{statement}: {err}"
        );
    }
}

/// R0.8 (b): Grafeo cannot stop a runaway path expansion, so an unbounded or over-long
/// variable-length pattern is refused before it runs.
#[test]
fn unbounded_paths_are_refused() {
    let engine = Engine::new();
    let graph = Graph::open(&engine, "acme", "paths", OpenSpec::default()).expect("open");
    for statement in [
        "MATCH p = (a)-[*]->(b) RETURN count(p)",
        "MATCH p = (a)-[:N*]->(b) RETURN count(p)",
        "MATCH p = (a)-[:N*3..]->(b) RETURN count(p)",
        "MATCH p = (a)-[:N*1..11]->(b) RETURN count(p)",
    ] {
        let err = graph.execute(statement, true).expect_err("refused");
        assert_eq!(err.reason(), "graph_unbounded_path", "{statement}: {err}");
    }
    graph
        .execute("MATCH p = (a)-[:N*1..10]->(b) RETURN count(p)", true)
        .expect("ten hops is allowed");
}

/// `<data_dir>/graphs/<graph_id>/` is the only place a graph's storage can be (Task 3; Review
/// Focus 4).
#[test]
fn storage_path_derives_from_data_dir_and_graph_id() {
    let data_dir = std::env::temp_dir().join(format!("loams-graph-data-{}", std::process::id()));
    let id = loams_graph::GraphId::new();
    let engine = Engine::with_data_dir(&data_dir);
    let graph = Graph::open(
        &engine,
        "acme",
        "stored",
        OpenSpec::persistent(&engine, id).expect("a data dir is configured"),
    )
    .expect("open");
    let expected = data_dir.join("graphs").join(id.to_string());
    assert!(
        graph.storage_dir().is_some_and(|d| d == expected),
        "{graph:?}"
    );
    assert!(expected.exists());
    assert!(id.to_string().starts_with("gr_"));
    drop(graph);
    engine.close("acme", "stored").expect("close");
    // An engine with no data dir has nowhere to put a persistent graph.
    assert!(OpenSpec::persistent(&Engine::new(), id).is_none());
    std::fs::remove_dir_all(&data_dir).ok();
}

/// Security review C1: a variable-length pattern inside an EXISTS, COUNT or VALUE subquery is
/// held to the same bound as one in the main pattern.
#[test]
fn subquery_paths_are_bounded_too() {
    let engine = Engine::new();
    let graph = Graph::open(&engine, "acme", "subq", OpenSpec::default()).expect("open");
    for statement in [
        "MATCH (a) WHERE a.v = 1 AND EXISTS { MATCH (a)-[*]->(b:Nope) } RETURN count(a)",
        "MATCH (x) WHERE COUNT { MATCH (x)-[*]->(y:Nope) } > 0 RETURN x",
        "MATCH (a) RETURN COUNT { MATCH (a)-[*]->(b) } AS c",
        "RETURN VALUE { MATCH (a)-[*1..50]->(b) RETURN count(b) }",
    ] {
        let err = graph.execute(statement, true).expect_err("refused");
        assert_eq!(err.reason(), "graph_unbounded_path", "{statement}: {err}");
    }
    // A bounded one inside a subquery is fine.
    graph
        .execute(
            "MATCH (a) WHERE EXISTS { MATCH (a)-[*1..3]->(b) } RETURN count(a) AS c",
            true,
        )
        .expect("a bounded subquery path runs");
}

/// Security review I1: a procedure call is at least a write to the gate, wherever it sits, and the
/// guard reads a statement the way Grafeo's lexer does (`--` is an edge unless a space follows,
/// strings take backslash escapes, keywords fold Unicode case). A read-only request refuses every
/// payload.
#[test]
fn read_only_refuses_lexer_and_call_payloads() {
    let engine = Engine::new();
    let graph = Graph::open(&engine, "acme", "lexer", OpenSpec::default()).expect("open");
    graph.execute("INSERT (:N {y: 0})", false).expect("seed");
    for statement in [
        "MATCH (a)--(b) CALL grafeo.labels() RETURN 1 AS x",
        r"RETURN '\'' NEXT CALL grafeo.labels() /*'*/",
        "MATCH (n) ſET n.y = 1 RETURN n",
        // `<--` followed by a space is an arrow, not a comment (re-review 2a).
        "MATCH (a)<-- (b) SET a.y = 1 RETURN a",
        "MATCH (a)<-- (b) CALL { INSERT (:Z) } RETURN a",
        "MATCH (a)<-- (b) CALL grafeo.labels() RETURN 1 AS x",
    ] {
        let err = graph
            .execute(statement, true)
            .expect_err("a read-only request refuses it");
        assert_eq!(err.reason(), "graph_read_only", "{statement}: {err}");
        assert_ne!(
            classify(statement, QueryLanguage::Gql),
            Access::Read,
            "{statement}"
        );
        if let Ok(engine) = engine_classify(statement) {
            assert_ne!(engine, Access::Read, "{statement}");
        }
    }
    let n = graph.execute("MATCH (n:N) RETURN n.y", true).expect("read");
    assert_eq!(
        n.rows[0].values[0],
        grafeo::Value::Int64(0),
        "nothing was written"
    );
}

/// Security review M2, as Task 6 settles it (R6.4): a single shortest-path search is served
/// (one breadth-first search per input row, bounded by the statement's deadline), and ALL
/// SHORTEST, whose answer can grow exponentially with the graph, is refused by keyword and by
/// plan, subqueries included.
#[test]
fn shortest_path_searches_are_bounded() {
    let engine = Engine::new();
    let graph = Graph::open(&engine, "acme", "sp", OpenSpec::default()).expect("open");
    // Admitted. (Grafeo 0.5.43 cannot `RETURN p` for a `shortestPath` path, "Variable 'p' not
    // found in input", so these count.)
    for statement in [
        "MATCH (a), (b) MATCH p = shortestPath((a)-[*]-(b)) RETURN count(*)",
        "MATCH p = ANY SHORTEST (a)-[*]-(b) RETURN count(*)",
        "MATCH p = ANY SHORTEST (a)-[*1..3]-(b) RETURN count(*)",
        "MATCH (a) WHERE EXISTS { MATCH p = ANY SHORTEST (a)-[*]-(b) } RETURN a",
    ] {
        graph.execute(statement, true).expect(statement);
    }
    for statement in [
        "MATCH (a), (b) MATCH p = allShortestPaths((a)-[*]-(b)) RETURN p",
        "MATCH p = ALL SHORTEST (a)-[*]-(b) RETURN p",
        "MATCH (a) WHERE EXISTS { MATCH p = ALL SHORTEST (a)-[*]-(b) } RETURN a",
        "MATCH (a) WHERE EXISTS { MATCH p = ALL\n  SHORTEST (a)-[*]-(b) } RETURN a",
    ] {
        let err = graph.execute(statement, true).expect_err("refused");
        assert_eq!(err, GraphError::AllShortestPaths, "{statement}");
        assert_eq!(err.reason(), "graph_unbounded_path", "{statement}: {err}");
    }
    // The plan check alone, without the keyword: a `ShortestPath` operator with `all_paths`.
    let err = loams_graph::classify::engine_classify(
        "MATCH (a), (b) MATCH p = allShortestPaths((a)-[*]-(b)) RETURN p",
    )
    .expect_err("refused by plan");
    assert_eq!(err, GraphError::AllShortestPaths);
}

/// Security review M7: GQL's own quantifier spellings are held to the same bound.
#[test]
fn gql_quantifiers_are_bounded() {
    let engine = Engine::new();
    let graph = Graph::open(&engine, "acme", "quant", OpenSpec::default()).expect("open");
    for statement in [
        "MATCH p = (a)-[:N]->{1,}(b) RETURN count(p)",
        "MATCH p = (a)-[:N]->+(b) RETURN count(p)",
        "MATCH p = (a)-[:N]->*(b) RETURN count(p)",
        "MATCH p = (a)-[:N]->{1,11}(b) RETURN count(p)",
        "MATCH p = ((a)-[:N]->(b)){1,} RETURN count(p)",
    ] {
        let err = graph.execute(statement, true).expect_err("refused");
        assert_eq!(err.reason(), "graph_unbounded_path", "{statement}: {err}");
    }
    graph
        .execute("MATCH p = (a)-[:N]->{1,10}(b) RETURN count(p)", true)
        .expect("a bounded quantifier runs");
}

/// Re-review 2b: the plan check reads Grafeo's derived `Debug` rendering. This canary pins the
/// names it relies on, so a Grafeo bump that changes them fails here, loudly, rather than quietly
/// weakening the classifier.
#[test]
fn grafeo_debug_format_canary() {
    use grafeo_engine::query::translators::gql::{GqlTranslationResult, translate_full};
    const CHANGED: &str = "Grafeo Debug format changed; re-verify the classifier (D759)";
    let render = |statement: &str| match translate_full(statement) {
        Ok(GqlTranslationResult::Plan(plan)) => format!("{:?}", plan.root),
        other => panic!(
            "{CHANGED}: {statement} no longer translates to a plan: {}",
            other.is_ok()
        ),
    };
    let exists = render("MATCH (a) WHERE EXISTS { MATCH (a)-[*]->(b) } RETURN a");
    assert!(
        exists.contains("ExistsSubquery("),
        "{CHANGED}: no `ExistsSubquery(` in {exists}"
    );
    assert!(
        exists.contains("max_hops: None"),
        "{CHANGED}: no `max_hops: None` in {exists}"
    );
    let bounded = render("MATCH (a)-[*1..3]->(b) RETURN b");
    assert!(
        bounded.contains("max_hops: Some(3)"),
        "{CHANGED}: no `max_hops: Some(3)` in {bounded}"
    );
    let call = render("CALL grafeo.labels()");
    assert!(
        call.contains("CallProcedure("),
        "{CHANGED}: no `CallProcedure(` in {call}"
    );
    let load = render("LOAD DATA FROM '/x' FORMAT CSV AS r RETURN r");
    assert!(
        load.contains("LoadData("),
        "{CHANGED}: no `LoadData(` in {load}"
    );
    // GR1 Task 6 (R6.4): ALL SHORTEST is found by `all_paths: true` inside a subquery too.
    let all = render("MATCH (a) WHERE EXISTS { MATCH p = ALL SHORTEST (a)-[*]-(b) } RETURN a");
    assert!(
        all.contains("ShortestPath(") && all.contains("all_paths: true"),
        "{CHANGED}: no `ShortestPath(` with `all_paths: true` in {all}"
    );
}

/// Re-review 2c: Grafeo's parser recurses once per operator-chain link and a stack overflow
/// aborts the whole process. Every parse runs on a large stack, and a chain past the limit is
/// refused before anything parses. Running this test at all (on the test harness's 2 MiB thread,
/// in a debug build, where about 70 links used to abort) is the assertion that nothing crashes.
#[test]
fn deep_operator_chains_do_not_crash_the_process() {
    let engine = Engine::new();
    let graph = Graph::open(&engine, "acme", "deep", OpenSpec::default()).expect("open");
    let nots = format!("RETURN {}true AS x", "NOT ".repeat(2000));
    let result = graph
        .execute(&nots, true)
        .expect("2000 chained NOTs parse and run");
    assert_eq!(result.rows[0].values[0], grafeo::Value::Bool(true));
    for statement in [
        format!("RETURN 1{} AS x", " + 1".repeat(2000)),
        format!(
            "MATCH (a) WHERE a.x = 1{} RETURN a",
            " AND a.x = 1".repeat(1300)
        ),
    ] {
        graph
            .execute(&statement, true)
            .expect("a long chain under the limit runs");
        engine_classify(&statement).expect("and classifies");
    }
    // A long NEXT chain reaches the engine, which answers (here, a semantic error) rather than
    // crashing.
    let next = format!("RETURN 1 AS x{}", " NEXT RETURN 1 AS x".repeat(1500));
    let _ = graph.execute(&next, true);
    // Past the limit: refused before any parse, on every path.
    for statement in [
        format!("RETURN {}true AS x", "NOT ".repeat(5000)),
        format!("RETURN 1{} AS x", " + 1".repeat(5000)),
        format!("RETURN 1{} AS x", "[0]".repeat(5000)),
    ] {
        let err = gate(&statement, QueryLanguage::Gql).expect_err("too long a chain");
        assert!(matches!(err, GraphError::TooComplex { .. }), "{err:?}");
        assert_eq!(err.reason(), "invalid_argument");
        let err = graph.execute(&statement, true).expect_err("refused");
        assert!(matches!(err, GraphError::TooComplex { .. }), "{err:?}");
    }
    // Bracket nesting Grafeo caps itself (128); deeper is its syntax error, not a crash.
    let parens = format!("RETURN {}1{} AS x", "(".repeat(127), ")".repeat(127));
    graph
        .execute(&parens, true)
        .expect("127 nested parentheses run");
    let parens = format!("RETURN {}1{} AS x", "(".repeat(500), ")".repeat(500));
    assert!(graph.execute(&parens, true).is_err());
}
