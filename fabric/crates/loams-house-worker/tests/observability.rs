//! Observability and the query log (HS1 Task 23, design §49 §15) over the
//! ClickHouse HTTP interface and `loams.house.v1`, with real worker processes,
//! `house-cache` and a `file://` bucket shared by the catalog and the front.
//!
//! Run locally in a cgroup of its own (`systemd-run --user --scope`, R21.8):
//! chDB's memory limit reads the whole cgroup's memory.

#![cfg(target_os = "linux")]

mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use common::http::{Response, request, target};
use common::{small, tmp_root};
use connectrpc::ErrorCode;
use connectrpc::client::{ClientConfig, HttpClient};
use loams_cache::RangeCacheConfig;
use loams_house::catalog::{HouseCatalog, Lake, LakeRoot, MemCatalog, PrefixGrants};
use loams_house::config::{HouseConfig, UserMap};
use loams_house::http::{HouseHandle, serve_with_lake};
use loams_house::querylog::Entry;
use loams_house::trace::{Recorder, Span, Tracer, Value};
use loams_house::{ProcessLauncher, WorkerPool};
use loams_house_cache::{HouseCache, OneStore, StoreOptions, Upstream};
use loams_house_proto::loams::house::v1 as pb;
use loams_house_proto::loams::house::v1::HouseServiceClient;
use loams_store::Store;

const KV: &str = "CREATE TABLE t (k UInt64, s String) ENGINE = MergeTree ORDER BY k";

struct House {
    handle: HouseHandle,
    spans: Arc<Recorder>,
    lines: Arc<loams_house::oplog::Memory>,
    _pool: WorkerPool,
    _proxy: HouseCache,
}

impl House {
    fn addr(&self) -> SocketAddr {
        self.handle.local_addr()
    }
}

/// A lake House: alice (every scope, `house:admin` among them) and bob (no
/// `house:admin`) in namespace 2. Spans and operator lines are recorded; the
/// query log is written when a test flushes it.
async fn lake_house(test: &str) -> House {
    let dir = tmp_root(&format!("{test}-bucket"));
    let url = format!("file://{}", dir.display());
    let store = Store::from_url(&url, Vec::<(String, String)>::new()).expect("store");
    let catalog = Arc::new(MemCatalog::new(store.clone(), LakeRoot::local()));
    let upstream = Upstream::open(&url, StoreOptions::default(), RangeCacheConfig::default())
        .await
        .expect("upstream");
    let proxy = HouseCache::new(Arc::new(OneStore(Arc::new(upstream))));
    let spans = Arc::new(Recorder::default());
    let tracer = Tracer::new(spans.clone());
    proxy.set_tracer(Arc::clone(&tracer));
    let launcher = ProcessLauncher::new(common::WORKER, tmp_root(test))
        .with_forward_target(Arc::new(proxy.clone()));
    let pool = WorkerPool::start(small(3), Arc::new(launcher))
        .await
        .expect("the pool starts");
    let lake = Arc::new(
        Lake::new(catalog as Arc<dyn HouseCatalog>, LakeRoot::local())
            .with_store(store)
            .with_grants(Arc::new(proxy.clone()) as Arc<dyn PrefixGrants>),
    );
    let lines = Arc::new(loams_house::oplog::Memory::default());
    let mut bob = UserMap::dev("bob", "b", 2, false);
    bob.scopes.admin = false;
    let config = HouseConfig {
        listen: "127.0.0.1:0".parse().expect("addr"),
        admin_listen: Some("127.0.0.1:0".parse().expect("addr")),
        users: vec![UserMap::dev("alice", "a", 2, false), bob],
        tmp_dir: tmp_root(&format!("{test}-spool")),
        compaction_interval: Duration::ZERO,
        retention_interval: Duration::ZERO,
        storage_usage_interval: Duration::ZERO,
        query_log_interval: Duration::ZERO,
        tracer,
        operator_log: Some(lines.clone()),
        cache_metrics: Some(Arc::new(proxy.clone())),
        ..HouseConfig::default()
    };
    let handle = serve_with_lake(config, pool.clone(), lake)
        .await
        .expect("serves");
    House {
        handle,
        spans,
        lines,
        _pool: pool,
        _proxy: proxy,
    }
}

/// `POST /` as `user` (`alice` or `bob`) with `sql` as the body.
async fn post(
    addr: SocketAddr,
    user: &'static str,
    sql: &str,
    params: &[(&str, &str)],
) -> Response {
    post_with(addr, user, sql, params, &[]).await
}

async fn post_with(
    addr: SocketAddr,
    user: &'static str,
    sql: &str,
    params: &[(&str, &str)],
    headers: &[(&'static str, &'static str)],
) -> Response {
    let password = &user[..1];
    let mut all = vec![("user", user), ("password", password)];
    all.extend_from_slice(params);
    let target = target(&all);
    let body = sql.as_bytes().to_vec();
    let headers = headers.to_vec();
    tokio::task::spawn_blocking(move || request(addr, "POST", &target, &headers, &body))
        .await
        .expect("client")
}

async fn ok(addr: SocketAddr, user: &'static str, sql: &str) -> Response {
    let response = post(addr, user, sql, &[]).await;
    assert_eq!(response.status, 200, "{sql}: {}", response.text());
    response
}

fn code(response: &Response) -> Option<&str> {
    response.header("X-ClickHouse-Exception-Code")
}

fn client(addr: SocketAddr, user: &str, password: &str) -> HouseServiceClient<HttpClient> {
    let basic = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"));
    let uri: http::Uri = format!("http://{addr}").parse().expect("uri");
    HouseServiceClient::new(
        HttpClient::plaintext(),
        ClientConfig::new(uri).with_default_header("authorization", format!("Basic {basic}")),
    )
}

async fn eventually(mut check: impl FnMut() -> bool) -> bool {
    for _ in 0..100 {
        if check() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    check()
}

async fn scrape(house: &House) -> String {
    let admin = house.handle.admin_addr().expect("an admin listener");
    let scraped = tokio::task::spawn_blocking(move || request(admin, "GET", "/metrics", &[], &[]))
        .await
        .expect("scrape");
    assert_eq!(scraped.status, 200);
    scraped.text()
}

/// A sample's value: `name{labels} value`, or `name value`.
fn sample(text: &str, series: &str) -> Option<f64> {
    text.lines()
        .find(|l| l.starts_with(series) && l[series.len()..].starts_with(' '))
        .and_then(|l| l.rsplit(' ').next())
        .and_then(|v| v.parse().ok())
}

/// Every series of §49 §15 is on `/metrics` with its name, type and the
/// counts of what ran: statements, bytes, worker boots, `house-cache`,
/// commits, compaction, usage events and the query log.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn metrics_exposed_with_names() {
    let house = lake_house("obs-metrics").await;
    let addr = house.addr();
    ok(addr, "alice", KV).await;
    ok(addr, "alice", "INSERT INTO t VALUES (1, 'a'), (2, 'b')").await;
    ok(addr, "alice", "INSERT INTO t VALUES (3, 'c')").await;
    ok(addr, "alice", "SELECT count() FROM t").await;
    let missing = post(addr, "alice", "SELECT * FROM nope", &[]).await;
    assert_eq!(code(&missing), Some("60"));
    ok(addr, "alice", "OPTIMIZE TABLE t FINAL").await;
    house.handle.flush_query_log().await;

    let text = scrape(&house).await;
    for (name, kind) in [
        ("loams_house_queries_total", "counter"),
        ("loams_house_query_seconds", "histogram"),
        ("loams_house_read_bytes_total", "counter"),
        ("loams_house_written_bytes_total", "counter"),
        ("loams_house_queue_depth", "gauge"),
        ("loams_house_workers", "gauge"),
        ("loams_house_worker_boot_seconds", "histogram"),
        ("loams_house_worker_kills_total", "counter"),
        ("loams_house_cache_hits_total", "counter"),
        ("loams_house_cache_misses_total", "counter"),
        ("loams_house_cache_bytes_total", "counter"),
        ("loams_house_commit_seconds", "histogram"),
        ("loams_house_commit_conflicts_total", "counter"),
        ("loams_house_compaction_runs_total", "counter"),
        ("loams_house_compaction_seconds", "histogram"),
        ("loams_house_compaction_files_rewritten_total", "counter"),
        ("loams_house_pipe_lag_records", "gauge"),
        ("loams_house_quota_refusals_total", "counter"),
        ("loams_house_usage_events_total", "counter"),
        ("loams_house_query_log_rows_total", "counter"),
    ] {
        assert!(
            text.contains(&format!("# TYPE {name} {kind}\n")),
            "{name} ({kind}) is exposed:\n{text}"
        );
    }
    let at_least = |series: &str, n: f64| {
        let value = sample(&text, series).unwrap_or_else(|| panic!("{series}:\n{text}"));
        assert!(value >= n, "{series} = {value}, want at least {n}");
    };
    at_least(
        "loams_house_queries_total{kind=\"Query\",status=\"ok\"}",
        1.0,
    );
    at_least(
        "loams_house_queries_total{kind=\"Query\",status=\"error\"}",
        1.0,
    );
    at_least(
        "loams_house_queries_total{kind=\"Insert\",status=\"ok\"}",
        2.0,
    );
    at_least("loams_house_query_seconds_count", 6.0);
    at_least("loams_house_written_bytes_total", 1.0);
    at_least("loams_house_worker_boot_seconds_count", 1.0);
    at_least("loams_house_cache_bytes_total", 1.0);
    at_least("loams_house_cache_misses_total", 1.0);
    // Two inserts, the compaction's replace is not a queue commit, and the
    // query log's flush.
    at_least("loams_house_commits_total{result=\"committed\"}", 3.0);
    at_least("loams_house_commit_seconds_count", 3.0);
    at_least(
        "loams_house_compaction_runs_total{result=\"committed\"}",
        1.0,
    );
    at_least("loams_house_compaction_files_rewritten_total", 2.0);
    at_least("loams_house_usage_events_total{event=\"house.query\"}", 1.0);
    at_least(
        "loams_house_usage_events_total{event=\"house.insert\"}",
        2.0,
    );
    at_least("loams_house_query_log_rows_total{result=\"written\"}", 6.0);
    // No namespace label unless the operator asks.
    assert!(!text.contains("loams_house_queries_total{ns="), "{text}");
}

fn attribute<'a>(span: &'a Span, key: &str) -> Option<&'a Value> {
    span.attribute(key)
}

fn root_of<'a>(spans: &'a [Span], query_id: &str) -> Option<&'a Span> {
    spans.iter().find(|s| {
        s.name == "house.query" && attribute(s, "loams.query_id") == Some(&Value::from(query_id))
    })
}

/// One trace per statement: its admission and its worker's execution are
/// children of its root (which names the query id), the `house-cache` reads
/// its worker made are children of the worker's span, and an `INSERT`'s commit
/// is a child of its root. A `traceparent` continues the caller's trace.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn trace_spans_link_front_worker_cache() {
    let house = lake_house("obs-trace").await;
    let addr = house.addr();
    ok(addr, "alice", KV).await;
    let insert = post(
        addr,
        "alice",
        "INSERT INTO t VALUES (1, 'a'), (2, 'b')",
        &[("query_id", "trace-insert")],
    )
    .await;
    assert_eq!(insert.status, 200, "{}", insert.text());
    let read = post_with(
        addr,
        "alice",
        "SELECT sum(k) FROM t",
        &[("query_id", "trace-read")],
        &[(
            "traceparent",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        )],
    )
    .await;
    assert_eq!(read.text().trim(), "3");

    assert!(
        eventually(|| {
            let spans = house.spans.spans();
            root_of(&spans, "trace-read").is_some() && root_of(&spans, "trace-insert").is_some()
        })
        .await,
        "both roots end"
    );
    let spans = house.spans.spans();
    let root = root_of(&spans, "trace-read").expect("the read's root");
    assert_eq!(
        loams_house::trace::hex(&root.context.trace_id),
        "4bf92f3577b34da6a3ce929d0e0e4736",
        "the caller's trace"
    );
    assert_eq!(
        root.parent,
        Some([0x00, 0xf0, 0x67, 0xaa, 0x0b, 0xa9, 0x02, 0xb7])
    );
    assert_eq!(attribute(root, "loams.namespace"), Some(&Value::Int(2)));
    let children = |parent: &Span, name: &str| -> Vec<Span> {
        spans
            .iter()
            .filter(|s| {
                s.name == name
                    && s.parent == Some(parent.context.span_id)
                    && s.context.trace_id == parent.context.trace_id
            })
            .cloned()
            .collect()
    };
    assert_eq!(children(root, "house.admission").len(), 1, "{spans:#?}");
    let workers = children(root, "house.worker");
    assert!(!workers.is_empty(), "{spans:#?}");
    let reads: Vec<Span> = workers
        .iter()
        .flat_map(|w| children(w, "house.cache.read"))
        .collect();
    assert!(
        !reads.is_empty(),
        "house-cache reads under the worker: {spans:#?}"
    );
    assert!(
        reads.iter().all(
            |r| matches!(attribute(r, "loams.key"), Some(Value::Text(k)) if k.starts_with("ns/2/"))
        ),
        "{reads:#?}"
    );
    let insert_root = root_of(&spans, "trace-insert").expect("the insert's root");
    let commits = children(insert_root, "house.commit");
    assert_eq!(commits.len(), 1, "{spans:#?}");
    assert!(matches!(
        attribute(&commits[0], "loams.snapshot_id"),
        Some(Value::Int(_))
    ));
    // No span carries the statement's text.
    for span in &spans {
        for (_, value) in &span.attributes {
            assert!(
                !matches!(value, Value::Text(t) if t.contains("INSERT INTO") || t.contains("sum(k)")),
                "{span:#?}"
            );
        }
    }
}

/// The operator line of a statement carries its query id, namespace, kind,
/// timing and code, and its text with every literal redacted; the full text
/// stays in the namespace's own query log.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn operator_logs_redact_literals() {
    let house = lake_house("obs-oplog").await;
    let addr = house.addr();
    let response = post(
        addr,
        "alice",
        "SELECT 'hunter2' AS password, 4111111111111111 AS card /* 'secret' */",
        &[("query_id", "redacted-1")],
    )
    .await;
    assert_eq!(response.status, 200, "{}", response.text());
    assert!(
        eventually(|| house
            .lines
            .lines()
            .iter()
            .any(|l| l.query_id == "redacted-1"))
        .await,
        "the line is written"
    );
    let lines = house.lines.lines();
    let line = lines
        .iter()
        .find(|l| l.query_id == "redacted-1")
        .expect("the line");
    assert_eq!(line.query, "SELECT ? AS password, ? AS card");
    assert_eq!(line.namespace, Some(2));
    assert_eq!(line.user, "alice");
    assert_eq!(line.kind, "Query");
    assert_eq!(line.code, 0);
    assert_eq!(line.result_rows, 1);
    for line in &lines {
        let json = serde_json::to_string(line).expect("json");
        for secret in ["hunter2", "4111111111111111", "secret"] {
            assert!(!json.contains(secret), "{json}");
        }
    }
    // The tenant's own log keeps the full text.
    house.handle.flush_query_log().await;
    let full = ok(
        addr,
        "alice",
        "SELECT query FROM system.query_log WHERE query_id = 'redacted-1' FORMAT TSVRaw",
    )
    .await;
    assert!(full.text().contains("'hunter2'"), "{}", full.text());
}

/// One row per client statement in `_house.query_log`, read as
/// `system.query_log`: before its flush (from the front's memory) and after
/// it (from the lake table), once each; failures with their code and type.
/// `_house` itself is the House's: not listed, not readable by name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn query_log_row_per_query() {
    let house = lake_house("obs-query-log").await;
    let addr = house.addr();
    let run = |sql: &'static str, id: &'static str| async move {
        post(addr, "alice", sql, &[("query_id", id)]).await
    };
    assert_eq!(run(KV, "ql-1-create").await.status, 200);
    assert_eq!(
        run("INSERT INTO t VALUES (1, 'a')", "ql-2-insert")
            .await
            .status,
        200
    );
    assert_eq!(run("SELECT * FROM t", "ql-3-select").await.status, 200);
    assert_eq!(
        code(&run("SELECT * FROM nope", "ql-4-missing").await),
        Some("60")
    );
    assert_eq!(code(&run("SELEC 1", "ql-5-syntax").await), Some("62"));

    let rows = |when: &'static str| async move {
        let response = ok(
            addr,
            "alice",
            "SELECT query_id, type, query_kind, exception_code, written_rows, result_rows \
             FROM system.query_log WHERE query_id LIKE 'ql-%' ORDER BY query_id FORMAT TSV",
        )
        .await;
        let text = response.text();
        assert_eq!(text.lines().count(), 5, "{when}: {text}");
        text
    };
    let want = |text: &str| {
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "ql-1-create\tQueryFinish\tCreate\t0\t0\t0");
        assert_eq!(lines[1], "ql-2-insert\tQueryFinish\tInsert\t0\t1\t0");
        assert_eq!(lines[2], "ql-3-select\tQueryFinish\tSelect\t0\t0\t1");
        assert!(lines[3].starts_with("ql-4-missing\tException"), "{text}");
        assert!(lines[3].contains("\t60\t"), "{text}");
        assert!(
            lines[4].starts_with("ql-5-syntax\tExceptionBeforeStart"),
            "{text}"
        );
    };
    want(&rows("before the flush").await);
    house.handle.flush_query_log().await;
    assert_eq!(
        house
            .handle
            .query_log()
            .expect("a query log")
            .pending(2)
            .iter()
            .filter(|e| e.query_id.starts_with("ql-"))
            .count(),
        0,
        "flushed"
    );
    want(&rows("after the flush").await);

    // `_house` is not the client's.
    let direct = post(addr, "alice", "SELECT * FROM _house.query_log", &[]).await;
    assert_eq!(code(&direct), Some("497"), "{}", direct.text());
    let dropped = post(addr, "alice", "DROP TABLE _house.query_log", &[]).await;
    assert_eq!(code(&dropped), Some("497"), "{}", dropped.text());
    let made = post(addr, "alice", "CREATE DATABASE _house", &[]).await;
    assert_eq!(code(&made), Some("497"), "{}", made.text());
    let shown = ok(addr, "alice", "SHOW DATABASES").await;
    assert!(!shown.text().contains("_house"), "{}", shown.text());
    // `system.processes` is the front's: the statement asking sees itself.
    let processes = ok(
        addr,
        "alice",
        "SELECT query_id, user FROM system.processes FORMAT TSV",
    )
    .await;
    assert!(processes.text().contains("\talice"), "{}", processes.text());
}

/// `ListQueryHistory` with scope `MINE` answers the caller's own statements;
/// `NAMESPACE` needs `house:admin` and answers everyone's. `GetQuery` finds
/// another user's statement only for an admin. `system.query_log` follows the
/// same rule.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn history_scope_mine_vs_namespace() {
    let house = lake_house("obs-history").await;
    let addr = house.addr();
    let a = post(
        addr,
        "alice",
        "SELECT 'from alice'",
        &[("query_id", "h-alice")],
    )
    .await;
    assert_eq!(a.status, 200);
    let b = post(addr, "bob", "SELECT 'from bob'", &[("query_id", "h-bob")]).await;
    assert_eq!(b.status, 200);
    house.handle.flush_query_log().await;

    let alice = client(addr, "alice", "a");
    let bob = client(addr, "bob", "b");
    let ids = |r: &pb::ListQueryHistoryResponse| -> Vec<String> {
        let mut ids: Vec<String> = r
            .queries
            .iter()
            .map(|q| q.query_id.clone())
            .filter(|id| id.starts_with("h-"))
            .collect();
        ids.sort();
        ids
    };
    let mine = bob
        .list_query_history(pb::ListQueryHistoryRequest::default())
        .await
        .expect("bob's own")
        .into_owned();
    assert_eq!(ids(&mine), vec!["h-bob"]);
    let row = mine
        .queries
        .iter()
        .find(|q| q.query_id == "h-bob")
        .expect("row");
    assert_eq!(row.principal, "bob");
    assert_eq!(row.query, "SELECT 'from bob'");
    assert_eq!(row.result_rows, 1);
    assert_eq!(
        row.status.as_known(),
        Some(pb::QueryStatus::QUERY_STATUS_FINISHED)
    );
    let err = bob
        .list_query_history(pb::ListQueryHistoryRequest {
            scope: pb::HistoryScope::HISTORY_SCOPE_NAMESPACE.into(),
            ..Default::default()
        })
        .await
        .expect_err("bob is no admin");
    assert_eq!(err.code, ErrorCode::PermissionDenied);
    let all = alice
        .list_query_history(pb::ListQueryHistoryRequest {
            scope: pb::HistoryScope::HISTORY_SCOPE_NAMESPACE.into(),
            ..Default::default()
        })
        .await
        .expect("alice's namespace")
        .into_owned();
    assert_eq!(ids(&all), vec!["h-alice", "h-bob"]);
    let found = alice
        .list_query_history(pb::ListQueryHistoryRequest {
            scope: pb::HistoryScope::HISTORY_SCOPE_NAMESPACE.into(),
            text_contains: "FROM BOB".to_string(),
            ..Default::default()
        })
        .await
        .expect("by text")
        .into_owned();
    assert_eq!(ids(&found), vec!["h-bob"]);

    let err = bob
        .get_query(pb::GetQueryRequest {
            query_id: "h-alice".to_string(),
            ..Default::default()
        })
        .await
        .expect_err("not bob's");
    assert_eq!(err.code, ErrorCode::NotFound);
    let got = alice
        .get_query(pb::GetQueryRequest {
            query_id: "h-bob".to_string(),
            ..Default::default()
        })
        .await
        .expect("an admin sees it")
        .into_owned();
    assert_eq!(
        got.query.as_option().expect("row").query,
        "SELECT 'from bob'"
    );

    let seen = ok(
        addr,
        "bob",
        "SELECT query_id FROM system.query_log WHERE query_id LIKE 'h-%' ORDER BY query_id \
         FORMAT TSV",
    )
    .await;
    assert_eq!(seen.text().trim(), "h-bob");
    let seen = ok(
        addr,
        "alice",
        "SELECT query_id FROM system.query_log WHERE query_id LIKE 'h-%' ORDER BY query_id \
         FORMAT TSV",
    )
    .await;
    assert_eq!(seen.text().trim(), "h-alice\nh-bob");
}

/// Days past the retention (30 days) are removed from `_house.query_log` by
/// a `delete` snapshot; newer days stay.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn query_log_retention() {
    let house = lake_house("obs-retention").await;
    let addr = house.addr();
    let log = house.handle.query_log().expect("a query log").clone();
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("now")
        .as_micros() as i64;
    let day = 86_400_000_000i64;
    for (id, age_days) in [("r-old", 40), ("r-edge", 29), ("r-new", 0)] {
        let at = now - age_days * day;
        log.record(Entry {
            namespace: Some(2),
            query_id: id.to_string(),
            surface: "http".to_string(),
            user: "alice".to_string(),
            kind: "Query".to_string(),
            database: "default".to_string(),
            query: format!("SELECT '{id}'"),
            started_us: at,
            ended_us: at,
            ..Entry::default()
        });
        // Each day its own file.
        log.flush().await;
    }
    let ids = |text: String| -> Vec<String> {
        text.lines()
            .filter(|l| l.starts_with("r-"))
            .map(str::to_string)
            .collect()
    };
    let listed = ok(
        addr,
        "alice",
        "SELECT query_id FROM system.query_log WHERE query_id LIKE 'r-%' ORDER BY query_id \
         FORMAT TSV",
    )
    .await;
    assert_eq!(ids(listed.text()), vec!["r-edge", "r-new", "r-old"]);
    // Nothing is past the retention on the day it was written.
    assert_eq!(
        log.retention_round(SystemTime::now() - Duration::from_secs(45 * 86_400))
            .await,
        0
    );
    assert_eq!(log.retention_round(SystemTime::now()).await, 1);
    let listed = ok(
        addr,
        "alice",
        "SELECT query_id FROM system.query_log WHERE query_id LIKE 'r-%' ORDER BY query_id \
         FORMAT TSV",
    )
    .await;
    assert_eq!(ids(listed.text()), vec!["r-edge", "r-new"]);
    // A second round finds nothing more.
    assert_eq!(log.retention_round(SystemTime::now()).await, 0);
}
