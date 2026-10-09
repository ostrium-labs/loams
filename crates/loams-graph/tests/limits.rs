//! Limits v1, streaming and redaction (GR1 Task 6; §48 §13.1, §13.2; rulings R0.8, R6.x).
//!
//! * Every statement has a Loams-side deadline: at it the RPC answers `graph_statement_timeout`
//!   and the statement, which Grafeo cannot stop, is detached and counted (R0.8 (a)).
//! * Unary results are cut at `max_rows` (`truncated`) and refused past `max_result_bytes`
//!   (`graph_result_too_large`); `ExecuteStream` answers every row in chunks.
//! * Statement size, parameters and batch length are checked before anything parses.
//! * A namespace runs at most its share of statements at once (`quota_exceeded`).
//! * `redact_literals` and `fingerprint` let a statement reach a log without its values.

use std::sync::Arc;
use std::time::{Duration, Instant};

use connectrpc::{ConnectError, ErrorCode};
use futures::StreamExt as _;
use loams_common::meta::MetaStore;
use loams_graph::Engine;
use loams_graph::catalog::GraphCatalog;
use loams_graph::limits::StatementLimits;
use loams_graph::redact::{fingerprint, redact_literals};
use loams_graph::service::admin::GraphAdmin;
use loams_meta::{MetaClient, MetaClientConfig, MetaConfig, MetaNode, Router, SystemClock};
use loams_proto::loams::errors::v1::ErrorInfo;
use loams_proto::loams::graph::v1 as pb;
use loams_store::Store;
use pb::__buffa::oneof::value::Kind;
use tempfile::TempDir;

/// A single-node metastore, a bucket and a data directory for one test.
struct Fixture {
    _node: MetaNode,
    meta: Arc<dyn MetaStore>,
    store: Store,
    data_dir: TempDir,
    _meta_dir: TempDir,
}

impl Fixture {
    async fn start() -> Self {
        let meta_dir = TempDir::new().expect("temp dir");
        let clock = Arc::new(SystemClock);
        let mut config = MetaConfig::new(1, meta_dir.path(), Store::in_memory());
        config.clock = clock.clone();
        let node = MetaNode::start(config, &Router::new())
            .await
            .expect("start meta");
        node.initialize([1]).await.expect("initialize");
        node.wait_for_leader(Duration::from_secs(30))
            .await
            .expect("leader");
        let client = MetaClient::new(node.clone(), vec![], clock, MetaClientConfig::default());
        Self {
            _node: node,
            meta: Arc::new(client),
            store: Store::in_memory(),
            data_dir: TempDir::new().expect("data dir"),
            _meta_dir: meta_dir,
        }
    }

    fn admin(&self) -> GraphAdmin {
        GraphAdmin::new(
            Arc::new(Engine::with_data_dir(self.data_dir.path())),
            GraphCatalog::new(self.meta.clone(), self.store.clone()),
        )
    }
}

fn info(err: &ConnectError) -> ErrorInfo {
    use base64::Engine as _;
    use buffa::Message as _;
    let detail = err
        .details
        .iter()
        .find(|d| d.type_url == "loams.errors.v1.ErrorInfo")
        .unwrap_or_else(|| panic!("no ErrorInfo in {err:?}"));
    let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(detail.value.as_deref().unwrap_or_default())
        .expect("unpadded base64");
    ErrorInfo::decode_from_slice(&bytes).expect("an ErrorInfo")
}

fn reason(err: &ConnectError) -> String {
    info(err).reason
}

async fn create(admin: &GraphAdmin, ns: &str, name: &str, limits: Option<pb::GraphLimits>) {
    admin
        .create_graph(pb::CreateGraphRequest {
            namespace: ns.to_string(),
            name: name.to_string(),
            limits: limits.into(),
            ..Default::default()
        })
        .await
        .expect("create");
}

fn execute(ns: &str, graph: &str, statement: &str) -> pb::ExecuteRequest {
    pb::ExecuteRequest {
        namespace: ns.to_string(),
        graph: graph.to_string(),
        statement: statement.to_string(),
        ..Default::default()
    }
}

/// `kg` in `acme` with `nodes` nodes `(:T {i})`.
async fn graph_with_nodes(admin: &GraphAdmin, nodes: u32) {
    create(admin, "acme", "kg", None).await;
    admin
        .execute(execute(
            "acme",
            "kg",
            &format!("UNWIND range(1, {nodes}) AS i INSERT (:T {{i: i}})"),
        ))
        .await
        .expect("insert");
}

fn rows(response: &pb::ExecuteResponse) -> &[pb::Row] {
    &response.rows.as_option().expect("rows").rows
}

fn int(value: &pb::Value) -> i64 {
    match &value.kind {
        Some(Kind::Int64(n)) => *n,
        other => panic!("not an int: {other:?}"),
    }
}

/// A cartesian aggregate over three scans: one long operator, which Grafeo's own time limit
/// cannot stop (R0.8). Seconds over 100 nodes in a debug build.
const LONG: &str = "MATCH (a:T), (b:T), (c:T) WHERE a.i + b.i + c.i < 0 RETURN count(*)";

async fn until<F: Fn() -> bool>(what: &str, done: F) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !done() {
        assert!(Instant::now() < deadline, "never: {what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// R0.8 (a), (d): a statement past its deadline is answered `graph_statement_timeout` within a
/// second, whatever the engine is doing. The statement is a shortest-path search with no hop
/// bound (served since Task 6, R6.4) over 1 000 nodes: one search per pair of nodes, a million
/// searches inside one operator, which the engine's own time limit never interrupts. It runs
/// on, detached, for a few seconds; the test waits for it to end (review fix 1, M2) so it does
/// not burn a core under the rest of the suite.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn timeout_returns_statement_timeout() {
    let fixture = Fixture::start().await;
    let admin = fixture.admin();
    graph_with_nodes(&admin, 1_000).await;
    let started = Instant::now();
    let err = admin
        .execute(pb::ExecuteRequest {
            timeout_ms: 200,
            ..execute(
                "acme",
                "kg",
                "MATCH p = ANY SHORTEST (a:T)-[:N]-(b:T) RETURN count(*)",
            )
        })
        .await
        .expect_err("past its deadline");
    let took = started.elapsed();
    assert!(took < Duration::from_secs(1), "answered after {took:?}");
    assert!(
        took >= Duration::from_millis(200),
        "answered after {took:?}"
    );
    assert_eq!(err.code, ErrorCode::DeadlineExceeded, "{err:?}");
    assert_eq!(reason(&err), "graph_statement_timeout");
    // Grafeo cannot stop it: it runs on, detached and counted, holding its slot.
    assert_eq!(admin.detached_statements(), 1);
    assert_eq!(admin.statements_in_flight(), 1);
    // The graph serves other statements meanwhile.
    let answer = admin
        .execute(execute("acme", "kg", "MATCH (n:T) RETURN count(n)"))
        .await
        .expect("still serving");
    assert_eq!(int(&rows(&answer)[0].values[0]), 1_000);
    // Bounded: it ends on its own, and its slot frees.
    let ran = Instant::now();
    until("the detached statement ends", || {
        admin.detached_statements() == 0 && admin.statements_in_flight() == 0
    })
    .await;
    assert!(
        ran.elapsed() < Duration::from_secs(30),
        "{:?}",
        ran.elapsed()
    );
}

/// The client's own deadline (Connect's timeout header) bounds the statement too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_client_deadline_bounds_the_statement() {
    let fixture = Fixture::start().await;
    let admin = fixture.admin();
    graph_with_nodes(&admin, 150).await;
    let started = Instant::now();
    let err = admin
        .for_call(Some(Instant::now() + Duration::from_millis(150)))
        .execute(execute("acme", "kg", LONG))
        .await
        .expect_err("past the client's deadline");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(reason(&err), "graph_statement_timeout", "{err:?}");
}

/// R0.8 (c) (`client_cancel_stops_statement` in the plan's list): Grafeo cannot stop a running
/// statement, so a client that goes away leaves it detached and counted; it holds its slot until
/// it ends. Past `max_detached` (2) on one graph, that graph refuses new statements.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_cancel_answers_and_detaches() {
    let fixture = Fixture::start().await;
    let admin = Arc::new(fixture.admin());
    graph_with_nodes(&admin, 100).await;
    for n in 1..=2 {
        let client = {
            let admin = admin.clone();
            tokio::spawn(async move { admin.execute(execute("acme", "kg", LONG)).await })
        };
        until("running", || admin.statements_in_flight() == n).await;
        client.abort();
        let _ = client.await;
        assert_eq!(
            admin.detached_statements(),
            n,
            "the cancelled statement is counted"
        );
    }
    let err = admin
        .execute(execute("acme", "kg", "RETURN 1"))
        .await
        .expect_err("two detached on this graph");
    assert_eq!(err.code, ErrorCode::ResourceExhausted, "{err:?}");
    assert_eq!(reason(&err), "resource_exhausted");
    // When they end, the graph serves again and nothing is counted.
    until("detached statements end", || {
        admin.detached_statements() == 0
    })
    .await;
    until("slots free", || admin.statements_in_flight() == 0).await;
    admin
        .execute(execute("acme", "kg", "RETURN 1"))
        .await
        .expect("serving again");
}

/// §48 §13.1: a unary answer holds at most `max_rows` rows (default 10 000, at most 100 000);
/// more are cut and `truncated` is set.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unary_truncates_at_max_rows() {
    let fixture = Fixture::start().await;
    let admin = fixture.admin();
    create(&admin, "acme", "kg", None).await;
    create(
        &admin,
        "acme",
        "small",
        Some(pb::GraphLimits {
            max_rows: 20,
            ..Default::default()
        }),
    )
    .await;
    let cases = [
        ("kg", "UNWIND range(1, 50) AS i RETURN i", 10, 10, true),
        ("kg", "UNWIND range(1, 50) AS i RETURN i", 0, 50, false),
        (
            "kg",
            "UNWIND range(1, 10001) AS i RETURN i",
            0,
            10_000,
            true,
        ),
        (
            "kg",
            "UNWIND range(1, 100001) AS i RETURN i",
            1_000_000,
            100_000,
            true,
        ),
        ("small", "UNWIND range(1, 50) AS i RETURN i", 0, 20, true),
        ("small", "UNWIND range(1, 50) AS i RETURN i", 30, 30, true),
    ];
    for (graph, statement, max_rows, want, truncated) in cases {
        let answer = admin
            .execute(pb::ExecuteRequest {
                max_rows,
                ..execute("acme", graph, statement)
            })
            .await
            .expect(statement);
        let got = rows(&answer);
        assert_eq!(got.len(), want, "{graph} {statement} max_rows={max_rows}");
        assert_eq!(
            answer.truncated, truncated,
            "{graph} {statement} {max_rows}"
        );
        assert_eq!(
            int(&got[0].values[0]),
            1,
            "the first rows are kept, in order"
        );
        assert_eq!(int(&got[want - 1].values[0]), want as i64);
    }
    // A batch's statements are cut the same way.
    let batch = admin
        .execute_batch(pb::ExecuteBatchRequest {
            namespace: "acme".to_string(),
            graph: "small".to_string(),
            statements: vec![pb::Statement {
                statement: "UNWIND range(1, 50) AS i RETURN i".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        })
        .await
        .expect("batch");
    assert_eq!(rows(&batch.results[0]).len(), 20);
    assert!(batch.results[0].truncated);
}

/// §48 §13.1: a unary answer larger than `max_result_bytes` (default 16 MiB, at most 64 MiB)
/// is refused with `graph_result_too_large`; `ExecuteStream` serves it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unary_bytes_cap_result_too_large() {
    let fixture = Fixture::start().await;
    let admin = fixture.admin();
    create(&admin, "acme", "kg", None).await;
    create(
        &admin,
        "acme",
        "small",
        Some(pb::GraphLimits {
            max_result_bytes: 1000,
            ..Default::default()
        }),
    )
    .await;
    let mut parameters = ::buffa::__private::HashMap::default();
    parameters.insert(
        "big".to_string(),
        pb::Value {
            kind: Some(Kind::String("x".repeat(10_000))),
            ..Default::default()
        },
    );
    // 2 000 rows of 10 kB: 20 MB, past the 16 MiB default.
    let big = pb::ExecuteRequest {
        parameters,
        ..execute("acme", "kg", "UNWIND range(1, 2000) AS i RETURN i, $big")
    };
    let err = admin.execute(big.clone()).await.expect_err("too large");
    assert_eq!(err.code, ErrorCode::ResourceExhausted, "{err:?}");
    assert_eq!(reason(&err), "graph_result_too_large");
    // The graph's own, smaller cap.
    let err = admin
        .execute(execute(
            "acme",
            "small",
            "UNWIND range(1, 100) AS i RETURN i, 'twenty characters...'",
        ))
        .await
        .expect_err("past the graph's cap");
    assert_eq!(reason(&err), "graph_result_too_large", "{err:?}");
    admin
        .execute(execute(
            "acme",
            "small",
            "UNWIND range(1, 10) AS i RETURN i",
        ))
        .await
        .expect("under the cap");
    // The stream carries it, in chunks of at most 1 MiB.
    let chunks: Vec<pb::ResultChunk> = admin
        .execute_stream(pb::ExecuteStreamRequest {
            request: big.into(),
            ..Default::default()
        })
        .await
        .expect("stream")
        .map(|chunk| chunk.expect("chunk"))
        .collect()
        .await;
    let total: usize = chunks.iter().map(|c| c.rows.len()).sum();
    assert_eq!(total, 2000);
    for chunk in &chunks {
        use buffa::Message as _;
        assert!(chunk.encoded_len() <= 1 << 20, "{}", chunk.encoded_len());
    }
}

/// `ExecuteStream` answers every row, in order, in chunks of at most 1 000 rows (or
/// `chunk_rows`); the columns come first, and the last chunk says so. Its `max_rows` caps the
/// whole stream.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stream_returns_all_rows_in_order() {
    let fixture = Fixture::start().await;
    let admin = fixture.admin();
    create(&admin, "acme", "kg", None).await;
    let stream = |chunk_rows: u32, max_rows: u32| {
        admin.execute_stream(pb::ExecuteStreamRequest {
            request: pb::ExecuteRequest {
                max_rows,
                ..execute("acme", "kg", "UNWIND range(1, 25000) AS i RETURN i")
            }
            .into(),
            chunk_rows,
            ..Default::default()
        })
    };
    for (chunk_rows, max_rows, want_rows, want_chunk, truncated) in [
        (0, 0, 25_000, 1000, false),
        (5000, 0, 25_000, 1000, false),
        (300, 0, 25_000, 300, false),
        (0, 1500, 1500, 1000, true),
    ] {
        let chunks: Vec<pb::ResultChunk> = stream(chunk_rows, max_rows)
            .await
            .expect("stream")
            .map(|chunk| chunk.expect("chunk"))
            .collect()
            .await;
        assert_eq!(chunks[0].columns, ["i"]);
        let mut next = 1;
        for (index, chunk) in chunks.iter().enumerate() {
            assert!(
                chunk.rows.len() <= want_chunk,
                "chunk {index}: {}",
                chunk.rows.len()
            );
            assert_eq!(chunk.last, index == chunks.len() - 1);
            for row in &chunk.rows {
                assert_eq!(int(&row.values[0]), next, "in order");
                next += 1;
            }
        }
        assert_eq!(
            next - 1,
            want_rows,
            "chunk_rows={chunk_rows} max_rows={max_rows}"
        );
        assert_eq!(chunks.last().expect("a chunk").truncated, truncated);
    }
    // An empty result is one last chunk with the columns.
    let chunks: Vec<pb::ResultChunk> = admin
        .execute_stream(pb::ExecuteStreamRequest {
            request: execute("acme", "kg", "MATCH (n:Nothing) RETURN n.i AS i").into(),
            ..Default::default()
        })
        .await
        .expect("stream")
        .map(|chunk| chunk.expect("chunk"))
        .collect()
        .await;
    assert_eq!(chunks.len(), 1);
    assert!(chunks[0].last && chunks[0].rows.is_empty());
    assert_eq!(chunks[0].columns, ["i"]);
    // A refused statement fails the call, before any chunk.
    let err = admin
        .execute_stream(pb::ExecuteStreamRequest {
            request: pb::ExecuteRequest {
                read_only: true,
                ..execute("acme", "kg", "INSERT (:T)")
            }
            .into(),
            ..Default::default()
        })
        .await
        .err()
        .expect("refused");
    assert_eq!(reason(&err), "graph_read_only");
}

/// §48 §13.1: a statement over 1 MiB, or with more than 1 000 parameters, is refused with
/// `INVALID_ARGUMENT` before it parses.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_statement_rejected() {
    let fixture = Fixture::start().await;
    let admin = fixture.admin();
    create(&admin, "acme", "kg", None).await;
    let limit = StatementLimits::DEFAULT.max_statement_bytes;
    assert_eq!(limit, 1 << 20);
    let pad = |len: usize| {
        let head = "RETURN 1 /* ";
        let tail = " */";
        format!("{head}{}{tail}", "x".repeat(len - head.len() - tail.len()))
    };
    admin
        .execute(execute("acme", "kg", &pad(limit)))
        .await
        .expect("exactly the limit");
    let err = admin
        .execute(execute("acme", "kg", &pad(limit + 1)))
        .await
        .expect_err("one byte over");
    assert_eq!(err.code, ErrorCode::InvalidArgument, "{err:?}");
    assert_eq!(reason(&err), "invalid_argument");
    // Parameters.
    let mut parameters = ::buffa::__private::HashMap::default();
    for n in 0..=StatementLimits::DEFAULT.max_parameters {
        parameters.insert(
            format!("p{n}"),
            pb::Value {
                kind: Some(Kind::Int64(1)),
                ..Default::default()
            },
        );
    }
    let err = admin
        .execute(pb::ExecuteRequest {
            parameters,
            ..execute("acme", "kg", "RETURN $p0")
        })
        .await
        .expect_err("too many parameters");
    assert_eq!(err.code, ErrorCode::InvalidArgument, "{err:?}");
    // The same checks guard the stream and Explain.
    let err = admin
        .execute_stream(pb::ExecuteStreamRequest {
            request: execute("acme", "kg", &pad(limit + 1)).into(),
            ..Default::default()
        })
        .await
        .err()
        .expect("stream: too large");
    assert_eq!(err.code, ErrorCode::InvalidArgument, "{err:?}");
    let err = admin
        .explain(pb::ExplainRequest {
            namespace: "acme".to_string(),
            graph: "kg".to_string(),
            statement: pad(limit + 1),
            ..Default::default()
        })
        .await
        .expect_err("explain: too large");
    assert_eq!(err.code, ErrorCode::InvalidArgument, "{err:?}");
}

/// §48 §13.1: a batch of more than 1 000 statements is refused whole, before any runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn batch_over_limit_rejected() {
    let fixture = Fixture::start().await;
    let admin = fixture.admin();
    create(&admin, "acme", "kg", None).await;
    let batch = |n: usize, atomic: bool| pb::ExecuteBatchRequest {
        namespace: "acme".to_string(),
        graph: "kg".to_string(),
        statements: (0..n)
            .map(|_| pb::Statement {
                statement: "INSERT (:B)".to_string(),
                ..Default::default()
            })
            .collect(),
        atomic,
        ..Default::default()
    };
    let limit = StatementLimits::DEFAULT.max_batch_statements;
    for atomic in [true, false] {
        let err = admin
            .execute_batch(batch(limit + 1, atomic))
            .await
            .expect_err("over the limit");
        assert_eq!(err.code, ErrorCode::InvalidArgument, "{err:?}");
        assert_eq!(reason(&err), "invalid_argument");
    }
    let count = admin
        .execute(execute("acme", "kg", "MATCH (n:B) RETURN count(n)"))
        .await
        .expect("count");
    assert_eq!(int(&rows(&count)[0].values[0]), 0, "nothing ran");
    let ok = admin
        .execute_batch(batch(limit, true))
        .await
        .expect("exactly the limit");
    assert_eq!(ok.committed_through as usize, limit);
}

/// §48 §13.2: a namespace runs at most `namespace_statements` (default 64) statements at once;
/// past that it is refused with `RESOURCE_EXHAUSTED`/`quota_exceeded`, naming the quota. Other
/// namespaces are not affected.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn namespace_concurrency_limit_resource_exhausted() {
    let fixture = Fixture::start().await;
    let admin = Arc::new(fixture.admin().with_namespace_statements(1));
    graph_with_nodes(&admin, 120).await;
    create(&admin, "acme", "other", None).await;
    create(&admin, "beta", "kg", None).await;
    let running = {
        let admin = admin.clone();
        tokio::spawn(async move { admin.execute(execute("acme", "kg", LONG)).await })
    };
    until("running", || admin.statements_in_flight() == 1).await;
    let err = admin
        .execute(execute("acme", "other", "RETURN 1"))
        .await
        .expect_err("the namespace is at its limit");
    assert_eq!(err.code, ErrorCode::ResourceExhausted, "{err:?}");
    let info = info(&err);
    assert_eq!(info.reason, "quota_exceeded");
    assert_eq!(
        info.metadata.get("quota").map(String::as_str),
        Some("concurrent_statements"),
        "{info:?}"
    );
    admin
        .execute(execute("beta", "kg", "RETURN 1"))
        .await
        .expect("another namespace runs");
    running
        .await
        .expect("task")
        .expect("the long statement ends");
    admin
        .execute(execute("acme", "other", "RETURN 1"))
        .await
        .expect("the namespace is free again");
}

/// Explain answers the plan without running the statement; PROFILE runs a read and answers what
/// each operator produced.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explain_answers_the_plan_without_running() {
    let fixture = Fixture::start().await;
    let admin = fixture.admin();
    graph_with_nodes(&admin, 5).await;
    let explain = |statement: &str, profile: bool| {
        admin.explain(pb::ExplainRequest {
            namespace: "acme".to_string(),
            graph: "kg".to_string(),
            statement: statement.to_string(),
            profile,
            ..Default::default()
        })
    };
    let plan = explain("MATCH (n:T) WHERE n.i > 2 RETURN n.i", false)
        .await
        .expect("plan");
    assert!(plan.text.contains("NodeScan"), "{}", plan.text);
    let root = plan.root.as_option().expect("a root");
    assert!(!root.name.is_empty());
    fn names(op: &pb::PlanOperator, out: &mut Vec<String>) {
        out.push(op.name.clone());
        op.children.iter().for_each(|c| names(c, out));
    }
    let mut all = Vec::new();
    names(root, &mut all);
    assert!(all.iter().any(|n| n == "NodeScan"), "{all:?}");
    // A write's plan is answered, and nothing is written.
    explain("INSERT (:Written)", false)
        .await
        .expect("a write's plan");
    let count = admin
        .execute(execute("acme", "kg", "MATCH (n:Written) RETURN count(n)"))
        .await
        .expect("count");
    assert_eq!(int(&rows(&count)[0].values[0]), 0, "EXPLAIN ran nothing");
    // PROFILE: per-operator rows.
    let profile = explain("PROFILE MATCH (n:T) RETURN n.i", true)
        .await
        .expect("profile");
    let mut ops = Vec::new();
    fn walk<'a>(op: &'a pb::PlanOperator, out: &mut Vec<&'a pb::PlanOperator>) {
        out.push(op);
        op.children.iter().for_each(|c| walk(c, out));
    }
    walk(profile.root.as_option().expect("root"), &mut ops);
    assert!(ops.iter().any(|op| op.rows == 5), "{profile:?}");
    // PROFILE of a write is refused (Task 2), and a PROFILE without the keyword is not
    // rewritten to have one (D634).
    let err = explain("PROFILE INSERT (:Written)", true)
        .await
        .expect_err("a write");
    assert_eq!(reason(&err), "graph_read_only", "{err:?}");
    let err = explain("MATCH (n:T) RETURN n.i", true)
        .await
        .expect_err("no PROFILE keyword");
    assert_eq!(err.code, ErrorCode::InvalidArgument, "{err:?}");
}

/// R6.4: a single shortest-path search is served within the statement's limits; ALL SHORTEST,
/// whose answer can grow exponentially with the graph, stays refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shortest_paths_are_served_within_limits() {
    let fixture = Fixture::start().await;
    let admin = fixture.admin();
    create(&admin, "acme", "kg", None).await;
    admin
        .execute(execute(
            "acme",
            "kg",
            "INSERT (:P {i: 1})-[:N]->(:P {i: 2})-[:N]->(:P {i: 3})-[:N]->(:P {i: 4})",
        ))
        .await
        .expect("a chain");
    for statement in [
        "MATCH p = ANY SHORTEST (a:P {i: 1})-[:N]->+(b:P {i: 4}) RETURN count(*)",
        "MATCH (a:P {i: 1}), (b:P {i: 4}) MATCH p = shortestPath((a)-[:N*]->(b)) RETURN count(*)",
    ] {
        let answer = admin
            .execute(execute("acme", "kg", statement))
            .await
            .unwrap_or_else(|err| panic!("{statement}: {err:?}"));
        assert_eq!(int(&rows(&answer)[0].values[0]), 1, "{statement}");
    }
    for statement in [
        "MATCH p = ALL SHORTEST (a:P)-[:N]->+(b:P) RETURN count(*)",
        "MATCH (a:P), (b:P) MATCH p = allShortestPaths((a)-[:N*]->(b)) RETURN count(*)",
        "MATCH (a:P) WHERE EXISTS { MATCH p = ALL SHORTEST (a)-[:N]->+(b:P) } RETURN a",
    ] {
        let err = admin
            .execute(execute("acme", "kg", statement))
            .await
            .expect_err(statement);
        assert_eq!(reason(&err), "graph_unbounded_path", "{statement}: {err:?}");
    }
}

/// A graph's own `max_path_hops` bounds its variable-length patterns (R0.8 (b)).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_graphs_max_path_hops_applies() {
    let fixture = Fixture::start().await;
    let admin = fixture.admin();
    create(
        &admin,
        "acme",
        "kg",
        Some(pb::GraphLimits {
            max_path_hops: 3,
            ..Default::default()
        }),
    )
    .await;
    admin
        .execute(execute(
            "acme",
            "kg",
            "MATCH p = (a)-[:N]->{1,3}(b) RETURN count(p)",
        ))
        .await
        .expect("within 3");
    let err = admin
        .execute(execute(
            "acme",
            "kg",
            "MATCH p = (a)-[:N]->{1,4}(b) RETURN count(p)",
        ))
        .await
        .expect_err("past 3");
    assert_eq!(reason(&err), "graph_unbounded_path", "{err:?}");
}

/// The Global Constraints' redaction: string and number literals become `?`; words,
/// parameters and punctuation stay; comments go (they can hold anything).
#[test]
fn redact_literals_strips_strings_and_numbers() {
    let cases = [
        (
            "MATCH (n:Person {name: 'Ada', age: 36}) RETURN n",
            "MATCH (n:Person {name: ?, age: ?}) RETURN n",
        ),
        ("RETURN \"secret\", 1.5e3, -7, $p1", "RETURN ?, ?, -?, $p1"),
        (
            "MATCH p = (a)-[:N]->{1,3}(b) WHERE a.x = 'it''s' RETURN p",
            "MATCH p = (a)-[:N]->{?,?}(b) WHERE a.x = ?? RETURN p",
        ),
        ("RETURN 'esc\\'aped' AS s", "RETURN ? AS s"),
        (
            "MATCH (n) -- password=hunter2\nRETURN n",
            "MATCH (n) \nRETURN n",
        ),
        ("RETURN /* key: 42 */ n.v2", "RETURN  n.v2"),
        ("MATCH (`odd 1`) RETURN 0x1F", "MATCH (`odd 1`) RETURN ?"),
        ("RETURN 'unterminated", "RETURN ?"),
    ];
    for (statement, want) in cases {
        assert_eq!(redact_literals(statement), want, "{statement}");
    }
    let redacted = redact_literals("INSERT (:U {token: 'tok_9f8e7d', pin: 4242})");
    assert!(
        !redacted.contains("tok_9f8e7d") && !redacted.contains("4242"),
        "{redacted}"
    );
}

/// Two statements that differ only in literals and spacing share a fingerprint; a different
/// shape does not.
#[test]
fn fingerprint_ignores_literals() {
    let a = fingerprint("MATCH (n:Person {name: 'Ada'}) WHERE n.age > 30 RETURN n");
    let b = fingerprint("MATCH (n:Person {name: \"Grace\"})\n  WHERE n.age > 41   RETURN n");
    let c = fingerprint("MATCH (n:Person {name: 'Ada'}) WHERE n.age < 30 RETURN n");
    let d = fingerprint("MATCH (n:City {name: 'Ada'}) WHERE n.age > 30 RETURN n");
    assert_eq!(a, b);
    assert_ne!(a, c);
    assert_ne!(a, d);
    // Stable across runs and builds: a fixed hash, not the process's random one.
    assert_eq!(fingerprint("RETURN 1"), fingerprint("RETURN 2"));
    assert_eq!(fingerprint(""), 0xcbf2_9ce4_8422_2325);
}

/// `name` in `ns` with `nodes` nodes `(:T {i})`.
async fn graph_in(admin: &GraphAdmin, ns: &str, name: &str, nodes: u32) {
    create(admin, ns, name, None).await;
    admin
        .execute(execute(
            ns,
            name,
            &format!("UNWIND range(1, {nodes}) AS i INSERT (:T {{i: i}})"),
        ))
        .await
        .expect("insert");
}

/// Review fix 1, I1: a namespace's cap keeps a reserve of the process slots (an eighth, at
/// least one), so one namespace can never take every slot; another namespace still runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_namespace_cannot_take_every_process_slot() {
    let fixture = Fixture::start().await;
    let default = fixture.admin();
    assert!(
        default.namespace_statements() < default.statement_slots(),
        "{} of {}",
        default.namespace_statements(),
        default.statement_slots()
    );
    let admin = Arc::new(
        fixture
            .admin()
            .with_statement_slots(4)
            .with_namespace_statements(64),
    );
    assert_eq!(admin.namespace_statements(), 3);
    for g in ["g1", "g2", "g3"] {
        graph_in(&admin, "acme", g, 100).await;
    }
    create(&admin, "beta", "kg", None).await;
    let running: Vec<_> = ["g1", "g2", "g3"]
        .into_iter()
        .map(|g| {
            let admin = admin.clone();
            tokio::spawn(async move { admin.execute(execute("acme", g, LONG)).await })
        })
        .collect();
    until("three running", || admin.statements_in_flight() == 3).await;
    let err = admin
        .execute(execute("acme", "g1", "RETURN 1"))
        .await
        .expect_err("the namespace is at its cap");
    assert_eq!(info(&err).reason, "quota_exceeded", "{err:?}");
    admin
        .execute(execute("beta", "kg", "RETURN 1"))
        .await
        .expect("the reserved slot serves another namespace");
    for task in running {
        task.await.expect("task").expect("ends");
    }
}

/// Review fix 1, I1: detached statements spread over many graphs of one namespace hit the
/// namespace's own cap (`quota_exceeded`, quota `detached_statements`), though no graph is at
/// its own `max_detached`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detached_statements_across_graphs_hit_the_namespace_cap() {
    let fixture = Fixture::start().await;
    let admin = fixture.admin().with_namespace_detached(2);
    for g in ["g1", "g2"] {
        graph_in(&admin, "acme", g, 100).await;
    }
    create(&admin, "acme", "g3", None).await;
    create(&admin, "beta", "kg", None).await;
    for (n, g) in ["g1", "g2"].into_iter().enumerate() {
        let err = admin
            .execute(pb::ExecuteRequest {
                timeout_ms: 50,
                ..execute("acme", g, LONG)
            })
            .await
            .expect_err("past its deadline");
        assert_eq!(reason(&err), "graph_statement_timeout");
        assert_eq!(admin.detached_statements(), n + 1);
    }
    let err = admin
        .execute(execute("acme", "g3", "RETURN 1"))
        .await
        .expect_err("the namespace has two detached");
    let info = info(&err);
    assert_eq!(info.reason, "quota_exceeded", "{err:?}");
    assert_eq!(
        info.metadata.get("quota").map(String::as_str),
        Some("detached_statements")
    );
    admin
        .execute(execute("beta", "kg", "RETURN 1"))
        .await
        .expect("another namespace runs");
    until("detached end", || admin.detached_statements() == 0).await;
    admin
        .execute(execute("acme", "g3", "RETURN 1"))
        .await
        .expect("serving again");
}

/// Review fix 1, I2: once a statement of a graph is detached, the statements in flight on it
/// count against `max_detached` (2) too, since each could run past its deadline.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn in_flight_statements_count_against_max_detached() {
    let fixture = Fixture::start().await;
    let admin = Arc::new(fixture.admin());
    graph_with_nodes(&admin, 100).await;
    // Nothing detached: two at once, and a third, are all admitted.
    let quick = "MATCH (n:T) RETURN count(n)";
    let three: Vec<_> = (0..3)
        .map(|_| {
            let admin = admin.clone();
            tokio::spawn(async move { admin.execute(execute("acme", "kg", quick)).await })
        })
        .collect();
    for task in three {
        task.await.expect("task").expect("admitted");
    }
    // One detached.
    let err = admin
        .execute(pb::ExecuteRequest {
            timeout_ms: 50,
            ..execute("acme", "kg", LONG)
        })
        .await
        .expect_err("past its deadline");
    assert_eq!(reason(&err), "graph_statement_timeout");
    // One more in flight: 1 detached + 1 running = 2, the limit.
    let running = {
        let admin = admin.clone();
        tokio::spawn(async move { admin.execute(execute("acme", "kg", LONG)).await })
    };
    until("two in flight", || admin.statements_in_flight() == 2).await;
    let err = admin
        .execute(execute("acme", "kg", "RETURN 1"))
        .await
        .expect_err("detached plus running at the limit");
    assert_eq!(err.code, ErrorCode::ResourceExhausted, "{err:?}");
    assert_eq!(reason(&err), "resource_exhausted");
    running.await.expect("task").expect("ends");
    until("detached end", || admin.detached_statements() == 0).await;
    admin
        .execute(execute("acme", "kg", "RETURN 1"))
        .await
        .expect("serving again");
}

fn count_of(answer: &pb::ExecuteResponse) -> i64 {
    int(&rows(answer)[0].values[0])
}

fn batch(graph: &str, statements: &[&str], atomic: bool) -> pb::ExecuteBatchRequest {
    pb::ExecuteBatchRequest {
        namespace: "acme".to_string(),
        graph: graph.to_string(),
        statements: statements
            .iter()
            .map(|s| pb::Statement {
                statement: (*s).to_string(),
                ..Default::default()
            })
            .collect(),
        atomic,
        ..Default::default()
    }
}

/// Review fix 1, I3: a statement that committed is never reported as a failure. Past the byte
/// limit, a write's answer drops its rows and sets `truncated` (with a `01000` notification); a
/// read's is still `graph_result_too_large`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_committed_write_is_never_reported_as_too_large() {
    let fixture = Fixture::start().await;
    let admin = fixture.admin();
    create(
        &admin,
        "acme",
        "small",
        Some(pb::GraphLimits {
            max_result_bytes: 1000,
            ..Default::default()
        }),
    )
    .await;
    let big_write =
        "UNWIND range(1, 100) AS i INSERT (w:W {i: i}) RETURN w.i, 'twenty characters...'";
    let big_read = "UNWIND range(1, 100) AS i RETURN i, 'twenty characters...'";
    let count = |label: &str| {
        let admin = &admin;
        let statement = format!("MATCH (n:{label}) RETURN count(n)");
        async move {
            count_of(
                &admin
                    .execute(execute("acme", "small", &statement))
                    .await
                    .expect("count"),
            )
        }
    };
    // Unary.
    let answer = admin
        .execute(execute("acme", "small", big_write))
        .await
        .expect("a committed write succeeds");
    assert!(answer.truncated);
    assert!(rows(&answer).is_empty());
    assert_eq!(answer.notifications[0].gqlstatus, "01000", "{answer:?}");
    assert_eq!(count("W").await, 100);
    let err = admin
        .execute(execute("acme", "small", big_read))
        .await
        .expect_err("a read is still refused");
    assert_eq!(reason(&err), "graph_result_too_large");
    // Atomic batch: it wrote, so it committed, and every answer that does not fit is cut.
    let answer = admin
        .execute_batch(batch("small", &["INSERT (:X)", big_read], true))
        .await
        .expect("a committed batch succeeds");
    assert!(answer.committed);
    assert_eq!(answer.committed_through, 2);
    assert!(answer.results[1].truncated && rows(&answer.results[1]).is_empty());
    assert_eq!(count("X").await, 1);
    // A read-only atomic batch committed nothing: refused.
    let err = admin
        .execute_batch(batch("small", &["RETURN 1", big_read], true))
        .await
        .expect_err("read-only and too large");
    assert_eq!(reason(&err), "graph_result_too_large");
    // Non-atomic: a write past the limit is answered without rows and the batch goes on; a
    // read past it fails and stops the batch before what follows.
    let answer = admin
        .execute_batch(batch("small", &[big_write, "INSERT (:Y)"], false))
        .await
        .expect("batch");
    assert!(answer.error.as_option().is_none(), "{answer:?}");
    assert_eq!(answer.committed_through, 2);
    assert!(answer.results[0].truncated);
    assert_eq!(count("Y").await, 1);
    let answer = admin
        .execute_batch(batch("small", &[big_read, "INSERT (:Z)"], false))
        .await
        .expect("batch");
    let error = answer.error.as_option().expect("the read failed");
    assert_eq!(error.index, 0);
    assert_eq!(answer.committed_through, 0);
    assert_eq!(count("Z").await, 0, "nothing after the failed read ran");
}

/// Review fix 1, I4: a stream holds its statement's slots until it is dropped, and its total
/// size is capped in bytes even with `max_rows` 0 (it then ends `truncated`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stream_holds_its_slots_and_is_capped_in_bytes() {
    let fixture = Fixture::start().await;
    let admin = fixture.admin().with_limits(StatementLimits {
        max_stream_bytes: 100_000,
        ..StatementLimits::DEFAULT
    });
    create(&admin, "acme", "kg", None).await;
    let request = || pb::ExecuteStreamRequest {
        request: execute("acme", "kg", "UNWIND range(1, 25000) AS i RETURN i").into(),
        ..Default::default()
    };
    let mut stream = admin.execute_stream(request()).await.expect("stream");
    let first = stream.next().await.expect("a chunk").expect("ok");
    assert!(!first.last);
    assert_eq!(
        admin.statements_in_flight(),
        1,
        "the open stream holds its slot"
    );
    drop(stream);
    assert_eq!(
        admin.statements_in_flight(),
        0,
        "dropping it frees the slot"
    );
    // Read to the end: capped in bytes, so truncated well short of 25 000 rows.
    let chunks: Vec<pb::ResultChunk> = admin
        .execute_stream(request())
        .await
        .expect("stream")
        .map(|chunk| chunk.expect("chunk"))
        .collect()
        .await;
    let total: usize = chunks.iter().map(|c| c.rows.len()).sum();
    let last = chunks.last().expect("a last chunk");
    assert!(last.last && last.truncated, "{total} rows");
    assert!(total > 1000 && total < 25_000, "{total}");
    let bytes: u64 = chunks
        .iter()
        .map(|c| {
            use buffa::Message as _;
            u64::from(c.encoded_len())
        })
        .sum();
    assert!(bytes <= 100_000, "{bytes}");
    assert_eq!(
        admin.statements_in_flight(),
        0,
        "a finished stream frees its slot"
    );
}

/// Review fix 1, M3: the engine types' `Debug` never prints a value: rows print their size,
/// statements their redacted text and parameter names, plans their redacted text.
#[test]
fn debug_forms_never_print_values() {
    use loams_graph::{BatchStatement, Graph, OpenSpec};
    let statement = BatchStatement {
        text: "MATCH (u {token: 'tok_secret'}) WHERE u.pin = 4242 RETURN u".to_string(),
        parameters: [("pw".to_string(), grafeo::Value::from("hunter2"))].into(),
    };
    let shown = format!("{statement:?}");
    assert!(
        !shown.contains("tok_secret") && !shown.contains("4242") && !shown.contains("hunter2"),
        "{shown}"
    );
    assert!(shown.contains("pw"), "{shown}");
    let engine = Engine::new();
    let graph = Graph::open(&engine, "acme", "dbg", OpenSpec::default()).expect("open");
    let result = graph
        .execute("RETURN 'tok_secret' AS s, 4242 AS n", true)
        .expect("run");
    let shown = format!("{result:?}");
    assert!(
        !shown.contains("tok_secret") && !shown.contains("4242"),
        "{shown}"
    );
    let plan = graph
        .explain_within(
            "MATCH (n:T) WHERE n.secret = 'tok_secret' RETURN n",
            &StatementLimits::DEFAULT,
        )
        .expect("plan");
    let shown = format!("{plan:?}");
    assert!(!shown.contains("tok_secret"), "{shown}");
}
