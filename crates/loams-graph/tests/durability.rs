//! The durable write path (GR1 Task 11; design §48 §6.2, §7.3; D742; rulings R9.3, R9.5, R9.8,
//! R11.x).
//!
//! * A write is acknowledged only once its change set is durable in the graph's stream, and the
//!   answer's consistency token names it (I1).
//! * No read sees a write that is not durable (I2): the write lane holds the graph's gate
//!   exclusively from `begin_transaction` until the append is acknowledged, and every client
//!   statement holds it shared (R9.3).
//! * A failed append, or a commit the capture refuses (R9.8; injected, see
//!   `a_refused_capture_reloads_and_hides`), fails every writer of its group with
//!   `graph_reloading`, and the graph is rebuilt from its stream before it serves again.
//! * Idempotency keys answer the first result's token, across a restart (the index is rebuilt
//!   from the stream).
//!
//! The append's fault injection is per [`GraphLog`] (feature `test-hooks`), so these tests need
//! no process-wide failpoint and run in parallel. The one failpoint (`loams_graph::capture`,
//! feature `failpoints`) names the graph it fails, so it does not touch the other tests either.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use connectrpc::{ConnectError, ErrorCode};
use loams_cache::{RangeCache, RangeCacheConfig};
use loams_common::StreamId;
use loams_common::meta::MetaStore;
use loams_graph::catalog::GraphCatalog;
use loams_graph::changeset::{ChangeOp, GraphRecord, apply_to_store, decode};
use loams_graph::durable::{AppendFault, ConsistencyToken, GraphLog, GroupCommit};
use loams_graph::service::admin::GraphAdmin;
use loams_graph::{Engine, GraphId};
use loams_log::{FetchRequest, LogError, LogReader, LogWriter, OffsetRecord};
use loams_meta::{MetaClient, MetaClientConfig, MetaConfig, MetaNode, Router, SystemClock};
use loams_proto::loams::errors::v1::ErrorInfo;
use loams_proto::loams::graph::v1 as pb;
use loams_store::Store;
use pb::__buffa::oneof::value::Kind;
use tempfile::TempDir;

/// A single-node metastore, a bucket, the graph log and a data directory for one test.
struct Fixture {
    _node: MetaNode,
    meta: Arc<dyn MetaStore>,
    store: Store,
    writer: LogWriter,
    reader: LogReader,
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
        let meta: Arc<dyn MetaStore> = Arc::new(MetaClient::new(
            node.clone(),
            vec![],
            clock,
            MetaClientConfig::default(),
        ));
        let store = Store::in_memory();
        let cache = RangeCache::new(
            store.clone(),
            RangeCacheConfig {
                memory_bytes: 16 << 20,
                ..RangeCacheConfig::default()
            },
        )
        .await
        .expect("range cache");
        let writer = LogWriter::start(meta.clone(), store.clone(), GraphLog::log_config(1))
            .expect("log writer");
        let reader = LogReader::new(meta.clone(), cache);
        Self {
            _node: node,
            meta,
            store,
            writer,
            reader,
            data_dir: TempDir::new().expect("data dir"),
            _meta_dir: meta_dir,
        }
    }

    fn log(&self) -> GraphLog {
        GraphLog::new(
            self.meta.clone(),
            self.writer.clone(),
            self.reader.clone(),
            tokio::runtime::Handle::current(),
        )
    }

    fn catalog(&self) -> GraphCatalog {
        GraphCatalog::new(self.meta.clone(), self.store.clone())
    }

    /// An admin over a fresh engine on the fixture's data directory, writing through `log`.
    fn admin_with(&self, log: &GraphLog) -> GraphAdmin {
        GraphAdmin::new(
            Arc::new(Engine::with_data_dir(self.data_dir.path())),
            self.catalog(),
        )
        .with_log(log.clone())
    }

    /// Every record of a stream, from its start.
    async fn records(&self, stream: StreamId) -> Vec<OffsetRecord> {
        let mut out = Vec::new();
        let mut offset = 0;
        loop {
            let fetched = self
                .reader
                .fetch(FetchRequest {
                    stream,
                    partition: 0,
                    offset,
                    max_bytes: 1 << 20,
                    max_wait: Duration::ZERO,
                })
                .await
                .expect("fetch");
            out.extend(fetched.records);
            if fetched.next_offset >= fetched.high_watermark {
                return out;
            }
            offset = fetched.next_offset;
        }
    }

    /// The graph's stream, as its catalog record names it.
    async fn stream_of(&self, name: &str) -> StreamId {
        self.catalog()
            .get_by_name("acme", name)
            .await
            .expect("catalog record")
            .stream
            .expect("a durable graph has its stream from its creation")
    }

    async fn graph_id(&self, name: &str) -> GraphId {
        self.catalog()
            .get_by_name("acme", name)
            .await
            .expect("catalog record")
            .graph_id()
            .expect("graph id")
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

async fn create(admin: &GraphAdmin, name: &str) {
    admin
        .create_graph(pb::CreateGraphRequest {
            namespace: "acme".to_string(),
            name: name.to_string(),
            ..Default::default()
        })
        .await
        .expect("create");
}

fn request(graph: &str, statement: &str, key: &str) -> pb::ExecuteRequest {
    pb::ExecuteRequest {
        namespace: "acme".to_string(),
        graph: graph.to_string(),
        statement: statement.to_string(),
        idempotency_key: key.to_string(),
        ..Default::default()
    }
}

async fn write(admin: &GraphAdmin, graph: &str, statement: &str) -> pb::ExecuteResponse {
    admin
        .execute(request(graph, statement, ""))
        .await
        .unwrap_or_else(|err| panic!("{statement}: {err:?}"))
}

/// The single value of a one-row, one-column answer.
fn single(response: &pb::ExecuteResponse) -> Option<Kind> {
    response
        .rows
        .as_option()
        .expect("rows")
        .rows
        .first()
        .map(|row| row.values[0].kind.clone().expect("a value with a kind"))
}

async fn count(admin: &GraphAdmin, graph: &str, statement: &str) -> i64 {
    match single(&write(admin, graph, statement).await) {
        Some(Kind::Int64(n)) => n,
        other => panic!("{statement}: {other:?}"),
    }
}

fn token(response: &pb::ExecuteResponse) -> ConsistencyToken {
    response
        .consistency_token
        .parse()
        .unwrap_or_else(|err| panic!("{:?}: {err}", response.consistency_token))
}

fn decoded(records: &[OffsetRecord]) -> Vec<(u64, GraphRecord)> {
    records
        .iter()
        .map(|r| {
            let value = r.record.value.as_deref().expect("a record value");
            (r.offset, decode(value).expect("a GraphRecord"))
        })
        .collect()
}

/// A fault that fails every append, as a refused log would.
fn failing() -> AppendFault {
    Arc::new(|| Box::pin(async { Err(LogError::Closed) }))
}

/// `(k, v)` of every `:N` node, by `k`, from a fresh engine that replays `records`.
fn replayed(records: &[OffsetRecord]) -> BTreeMap<i64, String> {
    let db = grafeo::GrafeoDB::new_in_memory();
    for (_, record) in decoded(records) {
        if let GraphRecord::ChangeSet(change) = record {
            apply_to_store(&db, &change.ops).expect("replay");
        }
    }
    let rows = db
        .execute("MATCH (n:N) RETURN n.k, n.v")
        .expect("read the replay");
    rows.rows()
        .iter()
        .map(|row| match (&row[0], &row[1]) {
            (grafeo::Value::Int64(k), grafeo::Value::String(v)) => (*k, v.to_string()),
            other => panic!("{other:?}"),
        })
        .collect()
}

async fn values(admin: &GraphAdmin, graph: &str) -> BTreeMap<i64, String> {
    let response = write(admin, graph, "MATCH (n:N) RETURN n.k, n.v").await;
    response
        .rows
        .as_option()
        .expect("rows")
        .rows
        .iter()
        .map(|row| match (&row.values[0].kind, &row.values[1].kind) {
            (Some(Kind::Int64(k)), Some(Kind::String(v))) => (*k, v.clone()),
            other => panic!("{other:?}"),
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acked_write_is_in_log() {
    let fx = Fixture::start().await;
    let admin = fx.admin_with(&fx.log());
    create(&admin, "kg").await;
    // The stream exists with the graph, before its id is visible to anyone (R11.2).
    let created = fx.stream_of("kg").await;
    let response = write(&admin, "kg", "INSERT (:Person {name: 'ada', born: 1815})").await;
    let token = token(&response);
    assert!(
        response.commit_epoch > 0,
        "a write answers its commit epoch"
    );
    assert_eq!(token.stream, created);
    assert!(token.offset > 0, "the token is the offset after the write");

    let records = fx.records(token.stream).await;
    let record = records
        .iter()
        .find(|r| r.offset == token.offset - 1)
        .expect("the record before the token's offset");
    let GraphRecord::ChangeSet(change) =
        decode(record.record.value.as_deref().expect("value")).expect("decode")
    else {
        panic!("a change set");
    };
    assert_eq!(change.graph, fx.graph_id("kg").await);
    assert_eq!(change.commit_epoch, response.commit_epoch);
    assert!(change.txn_seq >= 1);
    assert_eq!(change.idempotency_key, None);
    match change.ops.as_slice() {
        [ChangeOp::UpsertNode { labels, props, .. }] => {
            assert_eq!(labels, &["Person".to_string()]);
            assert_eq!(props.get("name"), Some(&grafeo::Value::from("ada")));
            assert_eq!(props.get("born"), Some(&grafeo::Value::Int64(1815)));
        }
        other => panic!("{other:?}"),
    }
    let counters = response.counters.as_option().expect("counters");
    assert_eq!(counters.nodes_created, 1);
    assert_eq!(counters.properties_set, 2);
    assert_eq!(counters.labels_added, 1);

    // A read answers no token and no epoch: it changed nothing.
    let read = write(&admin, "kg", "MATCH (n:Person) RETURN count(n)").await;
    assert_eq!(read.commit_epoch, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_read_sees_non_durable_write() {
    let fx = Fixture::start().await;
    let log = fx.log();
    let admin = fx.admin_with(&log);
    create(&admin, "kg").await;
    write(&admin, "kg", "INSERT (:N {k: 1, v: 'old'})").await;

    // The next append stops until the test releases it: by then the write has committed in
    // Grafeo, and only the log stands between it and the clients.
    let (reached_tx, reached_rx) = tokio::sync::oneshot::channel::<()>();
    let reached = Arc::new(std::sync::Mutex::new(Some(reached_tx)));
    let release = Arc::new(tokio::sync::Notify::new());
    let released = Arc::new(AtomicBool::new(false));
    {
        let (reached, release) = (reached.clone(), release.clone());
        log.set_append_fault(Some(Arc::new(move || {
            let (reached, release) = (reached.clone(), release.clone());
            Box::pin(async move {
                let first = reached.lock().expect("lock").take();
                if let Some(tx) = first {
                    let _ = tx.send(());
                    release.notified().await;
                }
                Ok(())
            })
        })));
    }
    let writer = {
        let admin = admin.clone();
        tokio::spawn(async move { write(&admin, "kg", "MATCH (n:N {k: 1}) SET n.v = 'new'").await })
    };
    reached_rx.await.expect("the write reached its append");

    // A strong read through the service, and a read on a new engine session of the open graph.
    let strong = {
        let (admin, released) = (admin.clone(), released.clone());
        tokio::spawn(async move {
            let values = values(&admin, "kg").await;
            (values, released.load(Ordering::SeqCst))
        })
    };
    let graph = admin.open("acme", "kg").await.expect("open");
    let session = {
        let released = released.clone();
        tokio::task::spawn_blocking(move || {
            let result = graph
                .execute("MATCH (n:N {k: 1}) RETURN n.v", true)
                .expect("read");
            let value = match &result.rows[0].values[0] {
                grafeo::Value::String(v) => v.to_string(),
                other => panic!("{other:?}"),
            };
            (value, released.load(Ordering::SeqCst))
        })
    };
    // A read with a deadline shorter than the append answers its timeout, never the value.
    let short = admin
        .execute(pb::ExecuteRequest {
            timeout_ms: 200,
            ..request("kg", "MATCH (n:N {k: 1}) RETURN n.v", "")
        })
        .await;
    match short {
        Err(err) => assert_eq!(reason(&err), "graph_statement_timeout"),
        Ok(response) => panic!("a read answered while a write was not durable: {response:?}"),
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !strong.is_finished(),
        "a strong read answered before the append completed"
    );
    assert!(
        !session.is_finished(),
        "a session read answered before the append completed"
    );

    released.store(true, Ordering::SeqCst);
    release.notify_one();
    let written = writer.await.expect("writer");
    assert!(!written.consistency_token.is_empty());
    let (strong_values, strong_after) = strong.await.expect("strong read");
    let (session_value, session_after) = session.await.expect("session read");
    // Whatever a read saw, it saw after the append completed: the new value is durable then.
    assert!(strong_after && session_after);
    assert_eq!(strong_values.get(&1).map(String::as_str), Some("new"));
    assert_eq!(session_value, "new");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_append_reloads_and_hides() {
    let fx = Fixture::start().await;
    let log = fx.log();
    let admin = fx.admin_with(&log);
    create(&admin, "kg").await;
    write(&admin, "kg", "INSERT (:N {k: 1, v: 'old'})").await;

    log.set_append_fault(Some(failing()));
    let err = admin
        .execute(request("kg", "MATCH (n:N {k: 1}) SET n.v = 'lost'", ""))
        .await
        .expect_err("an append that failed is not acknowledged");
    assert_eq!(err.code, ErrorCode::Unavailable);
    assert_eq!(reason(&err), "graph_reloading");
    let listed = admin
        .get_graph(pb::GetGraphRequest {
            namespace: "acme".to_string(),
            name: "kg".to_string(),
            ..Default::default()
        })
        .await
        .expect("get");
    assert_eq!(
        listed.state.as_known(),
        Some(pb::GraphState::Reloading),
        "the graph reloads before it serves again"
    );

    log.set_append_fault(None);
    // The next statement reopens the graph from its stream, which never had the write.
    assert_eq!(
        values(&admin, "kg").await.get(&1).map(String::as_str),
        Some("old")
    );
    let records = fx.records(fx.stream_of("kg").await).await;
    for (_, record) in decoded(&records) {
        assert!(
            !format!("{record:?}").contains("lost"),
            "an unacknowledged write reached the log: {record:?}"
        );
    }
    assert_eq!(replayed(&records), values(&admin, "kg").await);

    // And it serves writes again, durably.
    write(&admin, "kg", "INSERT (:N {k: 2, v: 'after'})").await;
    let records = fx.records(fx.stream_of("kg").await).await;
    assert_eq!(replayed(&records), values(&admin, "kg").await);
    assert_eq!(values(&admin, "kg").await.len(), 2);
}

/// A commit the capture refuses (R9.8) is never acknowledged, never logged and never seen: the
/// graph reloads from the log. Grafeo no longer commits the dangling edge that used to provoke
/// such a refusal (the fork's LG-0004), so the failure is injected with the
/// `loams_graph::capture` failpoint, for this test's graph only (`cargo test -p loams-graph
/// --features failpoints --test durability`).
#[cfg(feature = "failpoints")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_capture_reloads_and_hides() {
    let fx = Fixture::start().await;
    let admin = fx.admin_with(&fx.log());
    create(&admin, "refused").await;
    write(
        &admin,
        "refused",
        "INSERT (:N {k: 1, v: 'a'}), (:N {k: 2, v: 'b'})",
    )
    .await;
    // Not counted (`1*`): captures of other tests' graphs, running in parallel, evaluate the
    // failpoint too and must not use it up.
    fail::cfg("loams_graph::capture", "return(refused)").expect("cfg");
    let err = admin
        .execute_batch(pb::ExecuteBatchRequest {
            namespace: "acme".to_string(),
            graph: "refused".to_string(),
            atomic: true,
            statements: [
                "MATCH (a:N {k: 1}), (b:N {k: 2}) INSERT (a)-[:E]->(b)",
                "MATCH (a:N {k: 1}) DETACH DELETE a",
            ]
            .iter()
            .map(|s| pb::Statement {
                statement: (*s).to_string(),
                ..Default::default()
            })
            .collect(),
            ..Default::default()
        })
        .await
        .expect_err("a commit the capture refuses is not acknowledged");
    fail::remove("loams_graph::capture");
    assert_eq!(reason(&err), "graph_reloading");
    assert_eq!(values(&admin, "refused").await.len(), 2, "the node is back");
    assert_eq!(
        count(&admin, "refused", "MATCH ()-[e]->() RETURN count(e)").await,
        0
    );
    let records = fx.records(fx.stream_of("refused").await).await;
    assert_eq!(records.len(), 1, "only the insert is in the log");
}

/// The same batch with Grafeo's fix (the fork's LG-0004, R9.8): `DETACH DELETE` takes the edge the
/// transaction itself inserted, so the commit captures cleanly and is logged, and the graph the
/// log rebuilds is the graph the engine has.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn detach_delete_of_an_edge_inserted_in_the_batch_is_logged() {
    let fx = Fixture::start().await;
    let admin = fx.admin_with(&fx.log());
    create(&admin, "kg").await;
    write(
        &admin,
        "kg",
        "INSERT (:N {k: 1, v: 'a'}), (:N {k: 2, v: 'b'})",
    )
    .await;
    admin
        .execute_batch(pb::ExecuteBatchRequest {
            namespace: "acme".to_string(),
            graph: "kg".to_string(),
            atomic: true,
            statements: [
                "MATCH (a:N {k: 1}), (b:N {k: 2}) INSERT (a)-[:E]->(b)",
                "MATCH (a:N {k: 1}) DETACH DELETE a",
            ]
            .iter()
            .map(|s| pb::Statement {
                statement: (*s).to_string(),
                ..Default::default()
            })
            .collect(),
            ..Default::default()
        })
        .await
        .expect("committed");
    assert_eq!(values(&admin, "kg").await.len(), 1);
    assert_eq!(
        count(&admin, "kg", "MATCH ()-[e]->() RETURN count(e)").await,
        0
    );
    let records = fx.records(fx.stream_of("kg").await).await;
    assert_eq!(records.len(), 2, "the insert and the batch");
    assert_eq!(replayed(&records), values(&admin, "kg").await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idempotent_retry_returns_first_token() {
    let fx = Fixture::start().await;
    let log = fx.log();
    let admin = fx.admin_with(&log);
    create(&admin, "kg").await;
    let first = admin
        .execute(request("kg", "INSERT (:N {k: 1, v: 'once'})", "key-1"))
        .await
        .expect("first");
    let again = admin
        .execute(request("kg", "INSERT (:N {k: 1, v: 'once'})", "key-1"))
        .await
        .expect("retry");
    assert_eq!(again.consistency_token, first.consistency_token);
    assert_eq!(again.commit_epoch, first.commit_epoch);
    assert_eq!(
        again.notifications.first().map(|n| n.gqlstatus.as_str()),
        Some("01000"),
        "a replayed answer says it ran before"
    );
    assert_eq!(count(&admin, "kg", "MATCH (n:N) RETURN count(n)").await, 1);

    // After a restart the index is rebuilt from the stream.
    admin.shutdown(Duration::from_secs(5)).await;
    let restarted = fx.admin_with(&log);
    let after = restarted
        .execute(request("kg", "INSERT (:N {k: 1, v: 'once'})", "key-1"))
        .await
        .expect("retry after a restart");
    assert_eq!(after.consistency_token, first.consistency_token);
    assert_eq!(
        count(&restarted, "kg", "MATCH (n:N) RETURN count(n)").await,
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn different_key_runs_again() {
    let fx = Fixture::start().await;
    let admin = fx.admin_with(&fx.log());
    create(&admin, "kg").await;
    let first = admin
        .execute(request("kg", "INSERT (:N {k: 1, v: 'x'})", "key-1"))
        .await
        .expect("first");
    let second = admin
        .execute(request("kg", "INSERT (:N {k: 1, v: 'x'})", "key-2"))
        .await
        .expect("second");
    assert_ne!(second.consistency_token, first.consistency_token);
    assert!(token(&second).offset > token(&first).offset);
    assert_eq!(count(&admin, "kg", "MATCH (n:N) RETURN count(n)").await, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keyed_write_that_changed_nothing_is_appended() {
    // Controller ruling for Task 11: the key must be durable even when nothing changed, so a
    // retry after a restart answers the first run, not a second one.
    let fx = Fixture::start().await;
    let admin = fx.admin_with(&fx.log());
    create(&admin, "kg").await;
    let stream = {
        write(&admin, "kg", "INSERT (:N {k: 1, v: 'x'})").await;
        fx.stream_of("kg").await
    };
    let before = fx.records(stream).await.len();
    let response = admin
        .execute(request("kg", "MATCH (n:Missing) SET n.v = 'y'", "noop"))
        .await
        .expect("a no-op write");
    let records = fx.records(stream).await;
    assert_eq!(records.len(), before + 1);
    match decoded(&records).last() {
        Some((offset, GraphRecord::ChangeSet(change))) => {
            assert_eq!(*offset + 1, token(&response).offset);
            assert!(change.ops.is_empty());
            assert_eq!(change.idempotency_key.as_deref(), Some("noop"));
        }
        other => panic!("{other:?}"),
    }
    // Without a key, nothing is appended.
    write(&admin, "kg", "MATCH (n:Missing) SET n.v = 'y'").await;
    assert_eq!(fx.records(stream).await.len(), before + 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn batch_atomic_one_record() {
    let fx = Fixture::start().await;
    let admin = fx.admin_with(&fx.log());
    create(&admin, "kg").await;
    write(&admin, "kg", "INSERT (:Seed)").await;
    let stream = fx.stream_of("kg").await;
    let before = fx.records(stream).await.len();
    let statements = (0..50)
        .map(|i| {
            let mut parameters = ::buffa::__private::HashMap::default();
            parameters.insert(
                "i".to_string(),
                pb::Value {
                    kind: Some(Kind::Int64(i)),
                    ..Default::default()
                },
            );
            pb::Statement {
                statement: "INSERT (:B {i: $i})".to_string(),
                parameters,
                ..Default::default()
            }
        })
        .collect();
    let response = admin
        .execute_batch(pb::ExecuteBatchRequest {
            namespace: "acme".to_string(),
            graph: "kg".to_string(),
            atomic: true,
            statements,
            ..Default::default()
        })
        .await
        .expect("batch");
    assert!(response.committed);
    assert_eq!(response.results.len(), 50);
    assert!(response.commit_epoch > 0);
    let records = fx.records(stream).await;
    assert_eq!(records.len(), before + 1, "one record for the whole batch");
    let token: ConsistencyToken = response.consistency_token.parse().expect("token");
    assert_eq!(token.offset, records.last().expect("record").offset + 1);
    match decoded(&records).pop() {
        Some((_, GraphRecord::ChangeSet(change))) => {
            assert_eq!(change.ops.len(), 50);
            assert!(
                change
                    .ops
                    .iter()
                    .all(|op| matches!(op, ChangeOp::UpsertNode { labels, .. } if labels == &["B".to_string()]))
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn group_commit_preserves_epoch_order() {
    use proptest::test_runner::{Config, TestRunner};

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("runtime");
    let fx = runtime.block_on(Fixture::start());
    let (log, admin) = runtime.block_on(async {
        let log = fx.log();
        let admin = fx.admin_with(&log);
        (log, admin)
    });
    let _ = &log;
    let mut runner = TestRunner::new(Config {
        cases: 8,
        failure_persistence: None,
        ..Config::default()
    });
    let case = std::sync::atomic::AtomicU32::new(0);
    runner
        .run(&(2usize..7, 1usize..6), |(writers, each)| {
            let name = format!("g{}", case.fetch_add(1, Ordering::SeqCst));
            runtime.block_on(async {
                create(&admin, &name).await;
                let tasks: Vec<_> = (0..writers)
                    .map(|w| {
                        let (admin, name) = (admin.clone(), name.clone());
                        tokio::spawn(async move {
                            let mut tokens = Vec::new();
                            for i in 0..each {
                                let statement =
                                    format!("INSERT (:N {{k: {}, v: 'w{w}'}})", w * 100 + i);
                                tokens.push(token(&write(&admin, &name, &statement).await));
                            }
                            tokens
                        })
                    })
                    .collect();
                let mut all = Vec::new();
                for task in tasks {
                    let tokens = task.await.expect("writer");
                    // One writer's acknowledged writes are in its own order.
                    assert!(tokens.windows(2).all(|w| w[0].offset < w[1].offset));
                    all.extend(tokens);
                }
                let mut offsets: Vec<u64> = all.iter().map(|t| t.offset).collect();
                offsets.sort_unstable();
                offsets.dedup();
                assert_eq!(
                    offsets.len(),
                    writers * each,
                    "every write has its own record"
                );

                let records = fx.records(fx.stream_of(&name).await).await;
                assert_eq!(records.len(), writers * each);
                let mut last: Option<(u64, u64)> = None;
                for (_, record) in decoded(&records) {
                    let GraphRecord::ChangeSet(change) = record else {
                        panic!("a change set");
                    };
                    if let Some((seq, epoch)) = last {
                        assert_eq!(change.txn_seq, seq + 1, "txn_seq is dense in log order");
                        assert!(
                            change.commit_epoch > epoch,
                            "commit epochs follow log order"
                        );
                    }
                    last = Some((change.txn_seq, change.commit_epoch));
                }
                assert_eq!(replayed(&records), values(&admin, &name).await);
            });
            Ok(())
        })
        .expect("group commit keeps the log's order");
    runtime.block_on(admin.shutdown(Duration::from_secs(5)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn write_conflict_aborted_and_retryable() {
    // Write skew: each batch takes its doctor off call only if two are on call. Under SERIALIZABLE
    // at most one may succeed as written; the lane runs transactions one at a time (R9.3), so the
    // second sees the first's write. A batch that does conflict is ABORTED/graph_write_conflict
    // and is retried with its idempotency key.
    let fx = Fixture::start().await;
    let admin = fx.admin_with(&fx.log());
    create(&admin, "kg").await;
    write(
        &admin,
        "kg",
        "INSERT (:Doc {name: 'a', on: true}), (:Doc {name: 'b', on: true})",
    )
    .await;
    let batch = |me: &'static str| pb::ExecuteBatchRequest {
        namespace: "acme".to_string(),
        graph: "kg".to_string(),
        atomic: true,
        isolation: pb::Isolation::Serializable.into(),
        idempotency_key: format!("off-{me}"),
        statements: vec![pb::Statement {
            statement: format!(
                "MATCH (me:Doc {{name: '{me}'}}) \
                 WHERE COUNT {{ MATCH (d:Doc) WHERE d.on = true }} >= 2 SET me.on = false"
            ),
            ..Default::default()
        }],
        ..Default::default()
    };
    let run = |me: &'static str| {
        let admin = admin.clone();
        tokio::spawn(async move {
            for _ in 0..5 {
                match admin.execute_batch(batch(me)).await {
                    Ok(response) => return response,
                    Err(err) => {
                        assert_eq!(err.code, ErrorCode::Aborted, "{err:?}");
                        assert_eq!(reason(&err), "graph_write_conflict");
                    }
                }
            }
            panic!("a conflicting batch kept failing");
        })
    };
    let (a, b) = (run("a"), run("b"));
    let (a, b) = (a.await.expect("a"), b.await.expect("b"));
    assert!(a.committed && b.committed);
    assert_eq!(
        count(
            &admin,
            "kg",
            "MATCH (d:Doc) WHERE d.on = true RETURN count(d)"
        )
        .await,
        1,
        "no write skew: exactly one doctor went off call"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn if_version_mismatch_fails_precondition() {
    let fx = Fixture::start().await;
    let admin = fx.admin_with(&fx.log());
    create(&admin, "kg").await;
    let at = token(&write(&admin, "kg", "INSERT (:N {k: 1, v: 'a'})").await).offset;
    let batch = |version: u64, v: &str| pb::ExecuteBatchRequest {
        namespace: "acme".to_string(),
        graph: "kg".to_string(),
        atomic: true,
        if_version: Some(version),
        statements: vec![pb::Statement {
            statement: format!("MATCH (n:N {{k: 1}}) SET n.v = '{v}'"),
            ..Default::default()
        }],
        ..Default::default()
    };
    let err = admin
        .execute_batch(batch(at - 1, "stale"))
        .await
        .expect_err("a stale version");
    assert_eq!(err.code, ErrorCode::FailedPrecondition);
    assert_eq!(reason(&err), "graph_version_mismatch");
    assert_eq!(
        values(&admin, "kg").await.get(&1).map(String::as_str),
        Some("a")
    );

    let response = admin
        .execute_batch(batch(at, "b"))
        .await
        .expect("the current version");
    assert!(response.committed);
    assert_eq!(
        values(&admin, "kg").await.get(&1).map(String::as_str),
        Some("b")
    );
    let next: ConsistencyToken = response.consistency_token.parse().expect("token");
    assert_eq!(next.offset, at + 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reopen_rebuilds_from_the_log() {
    // The stream is the graph (I4): a restart rebuilds it from the stream, and a write Grafeo
    // committed but the log refused is not resurrected from the engine's local files.
    let fx = Fixture::start().await;
    let log = fx.log();
    let admin = fx.admin_with(&log);
    create(&admin, "kg").await;
    write(&admin, "kg", "INSERT (:N {k: 1, v: 'kept'})").await;
    log.set_append_fault(Some(failing()));
    let _ = admin
        .execute(request("kg", "INSERT (:N {k: 2, v: 'lost'})", ""))
        .await
        .expect_err("refused");
    log.set_append_fault(None);
    admin.shutdown(Duration::from_secs(5)).await;

    let restarted = fx.admin_with(&log);
    let kept = values(&restarted, "kg").await;
    assert_eq!(kept.len(), 1);
    assert_eq!(kept.get(&1).map(String::as_str), Some("kept"));
    // New ids do not collide with replayed ones.
    write(&restarted, "kg", "INSERT (:N {k: 3, v: 'next'})").await;
    assert_eq!(values(&restarted, "kg").await.len(), 2);
    let records = fx.records(fx.stream_of("kg").await).await;
    assert_eq!(replayed(&records), values(&restarted, "kg").await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_graph_from_before_the_log_is_adopted() {
    // A graph written before Task 11 has local files and no stream: its first durable open
    // appends its whole state as one change set, records the stream, and serves on.
    let fx = Fixture::start().await;
    let before = GraphAdmin::new(
        Arc::new(Engine::with_data_dir(fx.data_dir.path())),
        fx.catalog(),
    );
    create(&before, "kg").await;
    write(
        &before,
        "kg",
        "INSERT (:N {k: 1, v: 'a'})-[:E {w: 2}]->(:N {k: 2, v: 'b'})",
    )
    .await;
    before.shutdown(Duration::from_secs(5)).await;
    assert_eq!(
        fx.catalog()
            .get_by_name("acme", "kg")
            .await
            .expect("record")
            .stream,
        None
    );

    let log = fx.log();
    let admin = fx.admin_with(&log);
    assert_eq!(values(&admin, "kg").await.len(), 2);
    let records = fx.records(fx.stream_of("kg").await).await;
    assert_eq!(records.len(), 1, "one baseline record");
    assert_eq!(replayed(&records), values(&admin, "kg").await);
    admin.shutdown(Duration::from_secs(5)).await;

    // From now on the stream is the graph.
    let again = fx.admin_with(&log);
    assert_eq!(values(&again, "kg").await.len(), 2);
    assert_eq!(
        count(&again, "kg", "MATCH ()-[e:E]->() RETURN count(e)").await,
        1
    );
}

/// Runs `writers` concurrent single-insert writers on graph `kg` while the first append is held
/// for 300 ms, and answers how many appends there were.
async fn grouped_writes(fx: &Fixture, group: GroupCommit, writers: usize) -> usize {
    let log = fx.log().with_group_commit(group);
    // Every writer must be admitted at once: the default slot cap is twice the cores, which a
    // 4 vCPU runner puts below the writer count.
    let admin = fx.admin_with(&log).with_statement_slots(writers + 4);
    create(&admin, "kg").await;
    write(&admin, "kg", "INSERT (:Seed)").await;
    let appends = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    {
        let appends = appends.clone();
        log.set_append_fault(Some(Arc::new(move || {
            let first = appends.fetch_add(1, Ordering::SeqCst) == 0;
            Box::pin(async move {
                if first {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
                Ok(())
            })
        })));
    }
    let tasks: Vec<_> = (0..writers)
        .map(|i| {
            let admin = admin.clone();
            tokio::spawn(async move {
                let statement = format!("INSERT (:N {{k: {i}, v: 'w'}})");
                token(&write(&admin, "kg", &statement).await)
            })
        })
        .collect();
    for task in tasks {
        task.await.expect("writer");
    }
    let records = fx.records(fx.stream_of("kg").await).await;
    assert_eq!(records.len(), writers + 1);
    assert_eq!(replayed(&records).len(), writers);
    appends.load(Ordering::SeqCst)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn writers_queued_behind_an_append_share_the_next_one() {
    // While one group's append is in flight, the writers that queue behind it run as the next
    // group, under one exclusive hold and one append (§48 §6.2, R9.3).
    let fx = Fixture::start().await;
    let appends = grouped_writes(
        &fx,
        GroupCommit {
            window: Duration::from_secs(5),
            max_bytes: 1 << 20,
            ..GroupCommit::default()
        },
        12,
    )
    .await;
    assert!(appends <= 3, "12 writers took {appends} appends");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_group_ends_at_its_byte_limit() {
    let fx = Fixture::start().await;
    let appends = grouped_writes(
        &fx,
        GroupCommit {
            window: Duration::from_secs(5),
            max_bytes: 1,
            ..GroupCommit::default()
        },
        6,
    )
    .await;
    assert_eq!(appends, 6, "one record per group past the byte limit");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_change_set_larger_than_a_record_is_refused_and_undone() {
    let fx = Fixture::start().await;
    let log = fx.log().with_group_commit(GroupCommit {
        max_record_bytes: 4096,
        ..GroupCommit::default()
    });
    let admin = fx.admin_with(&log);
    create(&admin, "kg").await;
    write(&admin, "kg", "INSERT (:N {k: 1, v: 'small'})").await;
    let err = admin
        .execute(request(
            "kg",
            "UNWIND range(1, 500) AS i INSERT (:Big {i: i, pad: 'xxxxxxxxxxxxxxxxxxxxxxxx'})",
            "",
        ))
        .await
        .expect_err("a change set past the record limit");
    assert_eq!(err.code, ErrorCode::InvalidArgument, "{err:?}");
    assert_eq!(
        count(&admin, "kg", "MATCH (n:Big) RETURN count(n)").await,
        0
    );
    assert_eq!(values(&admin, "kg").await.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_non_atomic_batch_checks_its_version_before_a_leading_read() {
    // `if_version` guards a non-atomic batch's first statement, even when that one only reads:
    // the write after it must not run at a stale version.
    let fx = Fixture::start().await;
    let admin = fx.admin_with(&fx.log());
    create(&admin, "kg").await;
    let at = token(&write(&admin, "kg", "INSERT (:N {k: 1, v: 'a'})").await).offset;
    let response = admin
        .execute_batch(pb::ExecuteBatchRequest {
            namespace: "acme".to_string(),
            graph: "kg".to_string(),
            atomic: false,
            if_version: Some(at - 1),
            statements: [
                "MATCH (n:N) RETURN count(n)",
                "MATCH (n:N {k: 1}) SET n.v = 'b'",
            ]
            .iter()
            .map(|s| pb::Statement {
                statement: (*s).to_string(),
                ..Default::default()
            })
            .collect(),
            ..Default::default()
        })
        .await
        .expect("a non-atomic batch answers its failure in `error`");
    assert_eq!(response.committed_through, 0);
    let error = response
        .error
        .as_option()
        .expect("the first statement failed");
    assert_eq!(error.index, 0);
    assert_eq!(
        error.info.as_option().expect("info").reason,
        "graph_version_mismatch"
    );
    assert_eq!(
        values(&admin, "kg").await.get(&1).map(String::as_str),
        Some("a")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stalled_append_times_out_and_reloads() {
    // The lane holds the gate while it appends: an append that takes too long is given up as
    // unknown, so reads are never stalled behind the log for longer than the timeout.
    let fx = Fixture::start().await;
    let log = fx.log().with_group_commit(GroupCommit {
        append_timeout: Duration::from_millis(200),
        ..GroupCommit::default()
    });
    let admin = fx.admin_with(&log);
    create(&admin, "kg").await;
    write(&admin, "kg", "INSERT (:N {k: 1, v: 'old'})").await;
    log.set_append_fault(Some(Arc::new(|| {
        Box::pin(async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok(())
        })
    })));
    let started = std::time::Instant::now();
    let err = admin
        .execute(request("kg", "MATCH (n:N {k: 1}) SET n.v = 'stalled'", ""))
        .await
        .expect_err("a stalled append is not acknowledged");
    assert_eq!(reason(&err), "graph_reloading");
    assert!(started.elapsed() < Duration::from_secs(10));
    log.set_append_fault(None);
    assert_eq!(
        values(&admin, "kg").await.get(&1).map(String::as_str),
        Some("old")
    );
}
