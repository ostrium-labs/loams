//! One fenced owner per graph (GR1 Task 12; design §48 §7.1, D744, I5; Review Focus 2).
//!
//! Several in-process nodes share one metastore and one bucket. Each has its own engine, its own
//! log writer and its own view of the graph nodes ([`FixedMembership`], which a test changes to
//! model a node leaving or a partition). Forwarding runs in process through
//! [`GraphAdmin::serve_forwarded`], with a node that can be cut off.
//!
//! * A graph has one writer: every record of its stream carries one lease epoch, `txn_seq` is
//!   dense, and a write sent to another node is forwarded and answers the owner's token.
//! * A stale owner's append is refused by the metastore (its fence), not by the stale owner, and
//!   no acknowledged write is lost.
//! * A reload takes a fresh epoch before it reads the stream, so a commit of the old engine that
//!   lands late is refused (R11.8's race).
//! * A graph's stream has the reserved name `_graph.<gr_id>`, and GR1 Task 11's `graph.<gr_id>`
//!   moves to it on open; a purge drops it.
//! * Consistency tokens and followers (GR1 Task 15, §48 §7.2): a read with a write's token sees
//!   the write on any node (D76), an `eventual` read is served by a follower without a hop, a
//!   `strong` read and every write go to the owner, and a token read that cannot be satisfied in
//!   time is `UNAVAILABLE`/`consistency_wait_timeout`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use connectrpc::ErrorCode;
use loams_common::StreamId;
use loams_common::meta::{
    Consistency, Retention, WalClass, graph_stream_name, legacy_graph_stream_name,
};
use loams_graph::catalog::NewGraph;
use loams_graph::consistency::Consistency as ReadConsistency;
use loams_graph::durable::{ConsistencyToken, GroupCommit};
use loams_graph::owner::{ForwardMethod, OwnerConfig, Placement, owner_lease_key};
use loams_graph::service::admin::GraphAdmin;
use loams_proto::loams::graph::v1 as pb;
use loams_store::{Fault, FaultyStore, Op, Store};
use pb::__buffa::oneof::value::Kind;

mod common;

use common::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_nodes_one_writer() {
    let cluster = Cluster::start(2, OwnerConfig::default()).await;
    create(&cluster.node(1).admin, "kg").await;
    let graph = cluster.graph_id("kg").await;
    let owner = cluster.preferred(graph);

    // Both nodes take writes at once; each is keyed, so a retry would be safe.
    let mut tasks = Vec::new();
    for i in 0..16_i64 {
        let admin = cluster
            .node(1 + u64::try_from(i % 2).expect("0 or 1"))
            .admin
            .clone();
        tasks.push(tokio::spawn(async move {
            // Sixteen at once can pass the namespace's statement cap (an
            // eighth of the process slots is kept back, so a small runner
            // admits fewer): the refusal says retry, and the key makes it safe.
            let mut attempt = 0;
            loop {
                match admin
                    .execute(request(
                        "kg",
                        &format!("INSERT (:N {{k: {i}, v: 'w{i}'}})"),
                        &format!("w{i}"),
                    ))
                    .await
                {
                    Err(err) if err.code == ErrorCode::ResourceExhausted && attempt < 100 => {
                        attempt += 1;
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    other => break other.expect("a write through either node"),
                }
            }
        }));
    }
    let mut offsets = Vec::new();
    for task in tasks {
        offsets.push(token(&task.await.expect("join")).offset);
    }
    offsets.sort_unstable();
    assert_eq!(
        offsets,
        (1..=16).collect::<Vec<u64>>(),
        "one token per record"
    );

    // One writer: one epoch on every record, `txn_seq` dense, and one engine.
    let stream = cluster.stream_of("kg").await;
    let epochs = cluster.epochs(stream).await;
    assert_eq!(epochs.len(), 16);
    let epoch = epochs[0].0;
    assert!(epoch >= 1);
    assert!(epochs.iter().all(|(e, _)| *e == epoch), "{epochs:?}");
    assert_eq!(
        epochs.iter().map(|(_, seq)| *seq).collect::<Vec<_>>(),
        (1..=16).collect::<Vec<u64>>()
    );
    let other = 3 - owner;
    assert!(is_open(&cluster.node(owner).admin, "kg"));
    assert!(!is_open(&cluster.node(other).admin, "kg"));
    assert_eq!(
        cluster.node(owner).ownership.held(graph),
        Some(epoch),
        "the owner holds the lease at the records' epoch"
    );
    assert_eq!(cluster.node(other).ownership.held(graph), None);
    // Reads through either node see every write.
    assert_eq!(values(&cluster.node(other).admin, "kg").await.len(), 16);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forwarded_write_answers_owner_token() {
    let cluster = Cluster::start(2, OwnerConfig::default()).await;
    create(&cluster.node(1).admin, "kg").await;
    let graph = cluster.graph_id("kg").await;
    let owner = cluster.preferred(graph);
    let other = 3 - owner;
    run(
        &cluster.node(owner).admin,
        "kg",
        "INSERT (:N {k: 1, v: 'a'})",
    )
    .await;

    let forwarded = run(
        &cluster.node(other).admin,
        "kg",
        "INSERT (:N {k: 2, v: 'b'})",
    )
    .await;
    let owner_graph = cluster
        .node(owner)
        .admin
        .engine()
        .list(Some("acme"))
        .expect("list")
        .into_iter()
        .find(|g| g.name() == "kg")
        .expect("open on the owner");
    // The answer is the owner's: its token names the owner's durable state.
    assert_eq!(Some(token(&forwarded)), owner_graph.consistency_token());
    assert_eq!(token(&forwarded).offset, 2);
    assert!(forwarded.commit_epoch > 0);
    assert!(
        !is_open(&cluster.node(other).admin, "kg"),
        "the other node never opens it"
    );

    // Every data-plane call is forwarded the same way.
    let batch = cluster
        .node(other)
        .admin
        .execute_batch(pb::ExecuteBatchRequest {
            namespace: "acme".to_string(),
            graph: "kg".to_string(),
            statements: vec![pb::Statement {
                statement: "INSERT (:N {k: 3, v: 'c'})".to_string(),
                ..Default::default()
            }],
            atomic: true,
            ..Default::default()
        })
        .await
        .expect("a forwarded batch");
    let batch_token: ConsistencyToken = batch.consistency_token.parse().expect("a token");
    assert_eq!(batch_token.offset, 3);
    let schema = cluster
        .node(other)
        .admin
        .get_schema(pb::GetSchemaRequest {
            namespace: "acme".to_string(),
            name: "kg".to_string(),
            ..Default::default()
        })
        .await
        .expect("a forwarded GetSchema");
    assert_eq!(schema.labels.first().map(|l| l.count), Some(3));
    let plan = cluster
        .node(other)
        .admin
        .explain(pb::ExplainRequest {
            namespace: "acme".to_string(),
            graph: "kg".to_string(),
            statement: "MATCH (n:N) RETURN n.k".to_string(),
            ..Default::default()
        })
        .await
        .expect("a forwarded Explain");
    assert!(!plan.text.is_empty());
    let stream = cluster
        .node(other)
        .admin
        .execute_stream(pb::ExecuteStreamRequest {
            request: request("kg", "MATCH (n:N) RETURN n.k ORDER BY n.k", "").into(),
            chunk_rows: 1,
            ..Default::default()
        })
        .await
        .expect("a forwarded stream");
    use futures::StreamExt as _;
    let chunks: Vec<pb::ResultChunk> = stream.map(|c| c.expect("chunk")).collect().await;
    assert_eq!(chunks.iter().map(|c| c.rows.len()).sum::<usize>(), 3);
    assert!(chunks.last().is_some_and(|c| c.last));
    assert!(!is_open(&cluster.node(other).admin, "kg"));
}

/// Review Focus 2: partition the owner, let another node take the lease, and release the old
/// owner's pending append. The metastore refuses it by its fence; nothing acknowledged is lost.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stale_owner_append_is_fenced() {
    let cluster = Cluster::start(2, SHORT).await;
    create(&cluster.node(1).admin, "kg").await;
    let graph = cluster.graph_id("kg").await;
    let old = cluster.preferred(graph);
    let new = 3 - old;
    let acked = run(
        &cluster.node(old).admin,
        "kg",
        "INSERT (:N {k: 1, v: 'kept'})",
    )
    .await;
    let first_epoch = cluster.node(old).ownership.held(graph).expect("held");

    // The old owner's next append is held in its log...
    let (reached_tx, reached) = tokio::sync::oneshot::channel::<()>();
    let reached_tx = Arc::new(Mutex::new(Some(reached_tx)));
    let release = Arc::new(tokio::sync::Notify::new());
    {
        let (reached_tx, release) = (reached_tx.clone(), release.clone());
        cluster
            .node(old)
            .log
            .set_append_fault(Some(Arc::new(move || {
                let (reached_tx, release) = (reached_tx.clone(), release.clone());
                Box::pin(async move {
                    let first = reached_tx.lock().expect("lock").take();
                    if let Some(tx) = first {
                        let _ = tx.send(());
                        release.notified().await;
                    }
                    Ok(())
                })
            })));
    }
    let stale = {
        let admin = cluster.node(old).admin.clone();
        tokio::spawn(async move {
            admin
                .execute(request("kg", "INSERT (:N {k: 2, v: 'stale'})", ""))
                .await
        })
    };
    reached.await.expect("the stale append is pending");

    // ...and the old owner is cut off: it renews nothing, the others no longer see it, and
    // nobody can reach it.
    cluster.node(old).ownership.pause_renewals(true);
    let cut = cluster.forwarder.cut_off(old).expect("connected");
    cluster
        .node(new)
        .membership
        .set(vec![cluster.node(new).me.clone()]);
    wait_out_the_lease(&cluster, graph).await;

    // The new owner takes the lease at a newer epoch and serves every acknowledged write.
    let taken = run(
        &cluster.node(new).admin,
        "kg",
        "INSERT (:N {k: 3, v: 'new'})",
    )
    .await;
    let second_epoch = cluster.node(new).ownership.held(graph).expect("held");
    assert!(second_epoch > first_epoch);
    assert_eq!(token(&taken).offset, token(&acked).offset + 1);

    // The old owner's append goes out under its old epoch now: the metastore refuses it.
    release.notify_one();
    let refused = stale
        .await
        .expect("join")
        .expect_err("a fenced write is not acknowledged");
    assert_eq!(refused.code, ErrorCode::Unavailable);
    assert_eq!(info(&refused).reason, "graph_reloading");
    let epochs = cluster.epochs(cluster.stream_of("kg").await).await;
    assert_eq!(
        epochs.iter().map(|(epoch, _)| *epoch).collect::<Vec<_>>(),
        vec![first_epoch, second_epoch],
        "the stale write never reached the log"
    );

    // Back on the network, the old owner forwards to the new one and never serves its stale
    // engine again.
    cluster.node(old).log.set_append_fault(None);
    cluster.forwarder.reconnect(old, cut);
    let seen = values(&cluster.node(old).admin, "kg").await;
    assert_eq!(
        seen,
        BTreeMap::from([(1, "kept".to_string()), (3, "new".to_string())])
    );
    assert!(!is_open(&cluster.node(old).admin, "kg"));
    assert_eq!(cluster.node(old).ownership.held(graph), None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn owner_moves_on_node_leave() {
    let cluster = Cluster::start(2, OwnerConfig::default()).await;
    create(&cluster.node(1).admin, "kg").await;
    let graph = cluster.graph_id("kg").await;
    let old = cluster.preferred(graph);
    let new = 3 - old;
    run(&cluster.node(old).admin, "kg", "INSERT (:N {k: 1, v: 'a'})").await;
    let first_epoch = cluster.node(old).ownership.held(graph).expect("held");

    // A graceful leave: the node stops serving and gives its leases up.
    cluster
        .node(old)
        .admin
        .shutdown(Duration::from_secs(5))
        .await;
    cluster.forwarder.cut_off(old);
    cluster
        .node(new)
        .membership
        .set(vec![cluster.node(new).me.clone()]);
    let lease = cluster
        .meta
        .lease(Consistency::Linearizable, &owner_lease_key(graph))
        .await
        .expect("read")
        .expect("a lease");
    assert_eq!(lease.owner, None, "released, not left to expire");

    // The next node takes it at once (no TTL wait) and opens it from the stream.
    let started = std::time::Instant::now();
    let moved = run(&cluster.node(new).admin, "kg", "INSERT (:N {k: 2, v: 'b'})").await;
    assert!(started.elapsed() < OwnerConfig::default().ttl);
    assert_eq!(token(&moved).offset, 2);
    assert!(cluster.node(new).ownership.held(graph).expect("held") > first_epoch);
    assert_eq!(
        values(&cluster.node(new).admin, "kg").await,
        BTreeMap::from([(1, "a".to_string()), (2, "b".to_string())])
    );
}

/// Fault injection for forwarding: an owner nobody can reach is `UNAVAILABLE` (retryable) and
/// nothing runs; once its lease is out and it has left, the call is served.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unreachable_owner_is_unavailable_until_its_lease_moves() {
    let cluster = Cluster::start(2, SHORT).await;
    create(&cluster.node(1).admin, "kg").await;
    let graph = cluster.graph_id("kg").await;
    let old = cluster.preferred(graph);
    let new = 3 - old;
    run(&cluster.node(old).admin, "kg", "INSERT (:N {k: 1, v: 'a'})").await;

    cluster.forwarder.cut_off(old);
    let err = cluster
        .node(new)
        .admin
        .execute(request("kg", "INSERT (:N {k: 2, v: 'b'})", "k2"))
        .await
        .expect_err("the owner is unreachable");
    assert_eq!(err.code, ErrorCode::Unavailable);
    assert!(!is_open(&cluster.node(new).admin, "kg"));

    // The owner dies: its lease runs out and the registry drops it.
    cluster.node(old).ownership.pause_renewals(true);
    cluster
        .node(new)
        .membership
        .set(vec![cluster.node(new).me.clone()]);
    wait_out_the_lease(&cluster, graph).await;
    // The client's retry, with its key, is served by the new owner, once.
    cluster
        .node(new)
        .admin
        .execute(request("kg", "INSERT (:N {k: 2, v: 'b'})", "k2"))
        .await
        .expect("served by the new owner");
    assert_eq!(values(&cluster.node(new).admin, "kg").await.len(), 2);
}

/// #1085: a renewal the owner sent just before its renewals paused can land after, and move the
/// lease's deadline on. Waiting out the lease waits that out too, so the next node finds the lease
/// free. Before the fix the wait read the deadline once, returned at the old deadline, and the
/// survivor of `link_apply_exactly_once_under_kill` was answered "served by node N now; retry".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn waiting_out_a_lease_outlasts_a_renewal_that_lands_late() {
    let cluster = Cluster::start(2, SHORT).await;
    create(&cluster.node(1).admin, "kg").await;
    let graph = cluster.graph_id("kg").await;
    let old = cluster.preferred(graph);
    let new = 3 - old;
    run(&cluster.node(old).admin, "kg", "INSERT (:N {k: 1, v: 'a'})").await;

    cluster.node(old).ownership.pause_renewals(true);
    cluster.forwarder.cut_off(old);
    cluster
        .node(new)
        .membership
        .set(vec![cluster.node(new).me.clone()]);
    // The renewal in flight lands half a second before the deadline the lease had when the
    // renewals paused: it holds the lease about a second past that deadline.
    let key = owner_lease_key(graph);
    let paused = cluster
        .meta
        .lease(Consistency::Linearizable, &key)
        .await
        .expect("read")
        .expect("a lease");
    let late = {
        let (meta, key, paused) = (cluster.meta.clone(), key.clone(), paused.clone());
        tokio::spawn(async move {
            let lands = paused.deadline_ms.saturating_sub(500);
            while meta.now_ms() < lands {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            meta.renew_lease(
                &key,
                paused.owner.as_deref().expect("an owner"),
                paused.epoch,
                SHORT.ttl,
            )
            .await
        })
    };

    wait_out_the_lease(&cluster, graph).await;
    let renewed = late
        .await
        .expect("join")
        .expect("the late renewal lands while the lease is held");
    assert!(renewed.deadline_ms > paused.deadline_ms);
    assert!(
        cluster.meta.now_ms() >= renewed.deadline_ms,
        "the wait ended at the old deadline, before the late renewal's"
    );
    // The next node takes the graph at once, as the links' survivor does.
    assert!(matches!(
        cluster.node(new).ownership.place(graph, false).await,
        Ok(Placement::Here(_))
    ));
}

/// A forwarded call reaching a node that is not the owner is refused, never sent on: two nodes
/// with different views cannot bounce a call between them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_forwarded_call_is_never_forwarded_again() {
    use buffa::Message as _;
    let cluster = Cluster::start(2, OwnerConfig::default()).await;
    create(&cluster.node(1).admin, "kg").await;
    let graph = cluster.graph_id("kg").await;
    let owner = cluster.preferred(graph);
    let other = 3 - owner;
    run(
        &cluster.node(owner).admin,
        "kg",
        "INSERT (:N {k: 1, v: 'a'})",
    )
    .await;
    let err = cluster
        .node(other)
        .admin
        .serve_forwarded(
            ForwardMethod::Execute,
            request("kg", "INSERT (:N {k: 2, v: 'b'})", "").encode_to_bytes(),
        )
        .await
        .expect_err("not the owner");
    assert_eq!(err.code, ErrorCode::Unavailable);
    assert_eq!(info(&err).reason, "graph_reloading");
    assert!(!is_open(&cluster.node(other).admin, "kg"));
    assert_eq!(values(&cluster.node(owner).admin, "kg").await.len(), 1);
}

/// R11.8's residual race, closed: an append whose outcome is unknown (its WAL object is slow to
/// write) makes the graph reload; the reload takes a fresh epoch before it reads the stream, so
/// when the old append's commit finally lands it is refused, and the stream never holds a
/// record the rebuilt engine did not read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_late_commit_after_a_reload_is_fenced() {
    let faulty = Arc::new(FaultyStore::new(Store::in_memory().inner().clone()));
    let group = GroupCommit {
        append_timeout: Duration::from_millis(300),
        ..GroupCommit::default()
    };
    let cluster =
        Cluster::start_on(1, OwnerConfig::default(), Store::new(faulty.clone()), group).await;
    let admin = &cluster.node(1).admin;
    create(admin, "kg").await;
    let graph = cluster.graph_id("kg").await;
    run(admin, "kg", "INSERT (:N {k: 1, v: 'a'})").await;
    let first_epoch = cluster.node(1).ownership.held(graph).expect("held");

    // The next WAL object takes 2 s to write: the lane gives up on it after 300 ms.
    faulty.inject(Op::PutCreate, Fault::Delay(Duration::from_secs(2)));
    let err = admin
        .execute(request("kg", "INSERT (:N {k: 2, v: 'late'})", ""))
        .await
        .expect_err("an unknown outcome is not acknowledged");
    assert_eq!(info(&err).reason, "graph_reloading");
    // The next statement reloads the graph under a fresh epoch, before the object is written.
    assert_eq!(values(admin, "kg").await.len(), 1);
    let second_epoch = cluster.node(1).ownership.held(graph).expect("held");
    assert!(second_epoch > first_epoch);

    // The late commit lands under the old epoch, and is refused.
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    let stream = cluster.stream_of("kg").await;
    assert_eq!(
        cluster.epochs(stream).await,
        vec![(first_epoch, 1)],
        "the late append never reached the log"
    );
    let next = run(admin, "kg", "INSERT (:N {k: 3, v: 'c'})").await;
    assert_eq!(token(&next).offset, 2);
    assert_eq!(
        values(admin, "kg").await,
        BTreeMap::from([(1, "a".to_string()), (3, "c".to_string())])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn graph_streams_have_the_reserved_name_and_old_ones_move_to_it() {
    let cluster = Cluster::start(1, OwnerConfig::default()).await;
    let admin = &cluster.node(1).admin;
    create(admin, "kg").await;
    let graph = cluster.graph_id("kg").await;
    let ns = cluster
        .meta
        .namespace_by_name(Consistency::Linearizable, "acme")
        .await
        .expect("read")
        .expect("namespace")
        .id;
    let stream = cluster
        .meta
        .stream_by_name(
            Consistency::Linearizable,
            ns,
            &graph_stream_name(&graph.to_string()),
        )
        .await
        .expect("read")
        .expect("the reserved stream");
    assert_eq!(Some(stream.id), Some(cluster.stream_of("kg").await));

    // A graph from GR1 Task 11, whose stream is `graph.<gr_id>`: its record names that stream.
    let catalog = cluster.catalog();
    let old = catalog
        .create(
            "acme",
            "old",
            NewGraph {
                with_stream: false,
                ..NewGraph::default()
            },
        )
        .await
        .expect("create");
    let legacy = cluster
        .meta
        .create_stream(
            ns,
            &legacy_graph_stream_name(&old.id),
            1,
            WalClass::Standard,
            Retention::default(),
        )
        .await
        .expect("legacy stream");
    catalog
        .set_stream("acme", &old.id, legacy)
        .await
        .expect("record it");
    // Opening it moves the stream to the reserved name, keeping its id; it serves as before.
    run(admin, "old", "INSERT (:N {k: 1, v: 'a'})").await;
    let moved = cluster
        .meta
        .stream(Consistency::Linearizable, legacy)
        .await
        .expect("read")
        .expect("the same stream");
    assert_eq!(moved.name, graph_stream_name(&old.id));
    assert_eq!(cluster.records(legacy).await.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_purge_drops_the_graphs_stream_under_the_maintenance_lease() {
    let cluster = Cluster::start(2, OwnerConfig::default()).await;
    create(&cluster.node(1).admin, "kg").await;
    let graph = cluster.graph_id("kg").await;
    let owner = cluster.preferred(graph);
    run(
        &cluster.node(owner).admin,
        "kg",
        "INSERT (:N {k: 1, v: 'a'})",
    )
    .await;
    let stream = cluster.stream_of("kg").await;
    let other = 3 - owner;
    cluster
        .node(other)
        .admin
        .delete_graph(pb::DeleteGraphRequest {
            namespace: "acme".to_string(),
            name: "kg".to_string(),
            ..Default::default()
        })
        .await
        .expect("delete");
    tokio::time::sleep(Duration::from_millis(20)).await;
    // The owner's maintenance closes its engine and gives the lease up; the purge (on whichever
    // node takes the maintenance lease) drops the stream and the record.
    cluster
        .node(owner)
        .admin
        .maintain(Duration::ZERO, Duration::from_secs(600))
        .await
        .expect("maintain");
    assert!(!is_open(&cluster.node(owner).admin, "kg"));
    assert_eq!(cluster.node(owner).ownership.held(graph), None);
    assert_eq!(
        cluster
            .meta
            .stream(Consistency::Linearizable, stream)
            .await
            .expect("read"),
        None
    );
    assert_eq!(
        cluster
            .node(other)
            .admin
            .maintain(Duration::ZERO, Duration::from_secs(600))
            .await
            .expect("maintain again"),
        0
    );
}

// ---------------------------------------------------------------------------------------------
// GR1 Task 15: consistency tokens and followers (§48 §7.2)
// ---------------------------------------------------------------------------------------------

async fn create_replicated(admin: &GraphAdmin, name: &str, replicas: u32) {
    admin
        .create_graph(pb::CreateGraphRequest {
            namespace: "acme".to_string(),
            name: name.to_string(),
            replicas,
            ..Default::default()
        })
        .await
        .expect("create");
}

fn eventual() -> pb::Consistency {
    ReadConsistency::Eventual.to_proto()
}

fn at_least(token: ConsistencyToken) -> pb::Consistency {
    ReadConsistency::AtLeast(token).to_proto()
}

fn read_at(graph: &str, statement: &str, consistency: pb::Consistency) -> pb::ExecuteRequest {
    pb::ExecuteRequest {
        consistency: consistency.into(),
        ..request(graph, statement, "")
    }
}

/// `k → v` of every `:N` node and the answer's token, read through `admin` at `consistency`.
async fn values_at(
    admin: &GraphAdmin,
    graph: &str,
    consistency: pb::Consistency,
) -> (BTreeMap<i64, String>, ConsistencyToken) {
    let response = admin
        .execute(read_at(graph, "MATCH (n:N) RETURN n.k, n.v", consistency))
        .await
        .unwrap_or_else(|err| panic!("a read: {err:?}"));
    let values = response
        .rows
        .as_option()
        .expect("rows")
        .rows
        .iter()
        .map(|row| match (&row.values[0].kind, &row.values[1].kind) {
            (Some(Kind::Int64(k)), Some(Kind::String(v))) => (*k, v.clone()),
            other => panic!("{other:?}"),
        })
        .collect();
    (values, token(&response))
}

/// Whether `admin`'s node has a follower of the graph open.
fn follows(admin: &GraphAdmin, graph: &str) -> bool {
    admin
        .followers()
        .expect("followers")
        .engine()
        .list(Some("acme"))
        .expect("list")
        .iter()
        .any(|g| g.name() == graph)
}

fn kv(pairs: &[(i64, &str)]) -> BTreeMap<i64, String> {
    pairs.iter().map(|(k, v)| (*k, (*v).to_string())).collect()
}

/// D76's case: a client writes through the owner, then reads on another node with the write's
/// token, and sees the write. On a graph with a follower there the read waits for the follower to
/// apply it and never leaves the node; on a graph with none it goes to the owner.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn token_read_on_other_node_sees_write() {
    let cluster = Cluster::start(2, OwnerConfig::default()).await;
    create_replicated(&cluster.node(1).admin, "kg", 1).await;
    let graph = cluster.graph_id("kg").await;
    let owner = cluster.preferred(graph);
    let other = 3 - owner;
    run(
        &cluster.node(owner).admin,
        "kg",
        "INSERT (:N {k: 1, v: 'a'})",
    )
    .await;
    // The follower opens on its node's first read, from the stream.
    let (seen, _) = values_at(&cluster.node(other).admin, "kg", eventual()).await;
    assert_eq!(seen, kv(&[(1, "a")]));
    assert!(follows(&cluster.node(other).admin, "kg"));
    assert!(
        !is_open(&cluster.node(other).admin, "kg"),
        "it does not own it"
    );

    let forwards = cluster.forwarder.forwards();
    let written = run(
        &cluster.node(owner).admin,
        "kg",
        "INSERT (:N {k: 2, v: 'b'})",
    )
    .await;
    let (seen, read) = values_at(&cluster.node(other).admin, "kg", at_least(token(&written))).await;
    assert_eq!(seen, kv(&[(1, "a"), (2, "b")]));
    assert!(read.offset >= token(&written).offset, "{read:?}");
    assert_eq!(
        cluster.forwarder.forwards(),
        forwards,
        "the follower served it"
    );

    // A graph with no follower: the token read goes to the owner, which sees the write too.
    create_replicated(&cluster.node(1).admin, "solo", 0).await;
    let solo = cluster.graph_id("solo").await;
    let solo_owner = cluster.preferred(solo);
    let solo_other = 3 - solo_owner;
    let written = run(
        &cluster.node(solo_owner).admin,
        "solo",
        "INSERT (:N {k: 7, v: 'z'})",
    )
    .await;
    let forwards = cluster.forwarder.forwards();
    let (seen, _) = values_at(
        &cluster.node(solo_other).admin,
        "solo",
        at_least(token(&written)),
    )
    .await;
    assert_eq!(seen, kv(&[(7, "z")]));
    assert_eq!(cluster.forwarder.forwards(), forwards + 1);
    assert!(!follows(&cluster.node(solo_other).admin, "solo"));
}

/// A token read while the graph's owner is cut off and its lease has not moved yet: the follower
/// serves it, with the write, without the owner. Once the lease moves and the other node owns the
/// graph, the same token still reads the write, now from the owner's engine.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn token_read_during_owner_move_sees_write() {
    let cluster = Cluster::start(2, SHORT).await;
    create_replicated(&cluster.node(1).admin, "kg", 1).await;
    let graph = cluster.graph_id("kg").await;
    let old = cluster.preferred(graph);
    let new = 3 - old;
    let written = run(
        &cluster.node(old).admin,
        "kg",
        "INSERT (:N {k: 1, v: 'kept'})",
    )
    .await;

    // The owner is cut off: it renews nothing, nobody reaches it, and the other node sees only
    // itself.
    cluster.node(old).ownership.pause_renewals(true);
    let _cut = cluster.forwarder.cut_off(old).expect("connected");
    cluster
        .node(new)
        .membership
        .set(vec![cluster.node(new).me.clone()]);
    let (seen, _) = values_at(&cluster.node(new).admin, "kg", at_least(token(&written))).await;
    assert_eq!(seen, kv(&[(1, "kept")]));
    assert!(follows(&cluster.node(new).admin, "kg"));
    let lease = cluster
        .meta
        .lease(Consistency::Linearizable, &owner_lease_key(graph))
        .await
        .expect("read")
        .expect("a lease");
    assert_eq!(
        lease.owner.as_deref(),
        Some(cluster.node(old).me.lease_owner().as_str()),
        "read while the old owner still held the lease"
    );

    // The lease runs out; the other node takes the graph on its next write, and its follower
    // goes. The old token still reads the write.
    wait_out_the_lease(&cluster, graph).await;
    let moved = run(
        &cluster.node(new).admin,
        "kg",
        "INSERT (:N {k: 2, v: 'new'})",
    )
    .await;
    assert_eq!(token(&moved).offset, token(&written).offset + 1);
    assert!(cluster.node(new).ownership.held(graph).is_some());
    let (seen, _) = values_at(&cluster.node(new).admin, "kg", at_least(token(&written))).await;
    assert_eq!(seen, kv(&[(1, "kept"), (2, "new")]));
    assert!(
        !follows(&cluster.node(new).admin, "kg"),
        "the owner serves it now"
    );
}

/// A token no replica reaches in time is `UNAVAILABLE`/`consistency_wait_timeout`, after the wait,
/// on a follower and on the owner. A token that names another stream, or is not a token, is
/// refused at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn token_wait_times_out_with_reason() {
    let cluster = Cluster::start(2, OwnerConfig::default()).await;
    create_replicated(&cluster.node(1).admin, "kg", 1).await;
    let graph = cluster.graph_id("kg").await;
    let owner = cluster.preferred(graph);
    let other = 3 - owner;
    let written = run(
        &cluster.node(owner).admin,
        "kg",
        "INSERT (:N {k: 1, v: 'a'})",
    )
    .await;
    let ahead = ConsistencyToken {
        offset: token(&written).offset + 100,
        ..token(&written)
    };
    let wait = Duration::from_millis(300);
    for node in [other, owner] {
        let admin = cluster.node(node).admin.clone().with_consistency_wait(wait);
        // Open the replica first, so the wait is the whole of the second call.
        values_at(&admin, "kg", eventual()).await;
        let started = std::time::Instant::now();
        let err = admin
            .execute(read_at("kg", "MATCH (n:N) RETURN n.k", at_least(ahead)))
            .await
            .expect_err("never applied");
        assert_eq!(err.code, ErrorCode::Unavailable, "{err:?}");
        assert_eq!(info(&err).reason, "consistency_wait_timeout");
        assert!(started.elapsed() >= wait, "{:?}", started.elapsed());
    }
    assert!(follows(&cluster.node(other).admin, "kg"));

    let elsewhere = ConsistencyToken {
        stream: StreamId(token(&written).stream.0 + 1_000),
        ..token(&written)
    };
    let err = cluster
        .node(owner)
        .admin
        .execute(read_at("kg", "MATCH (n:N) RETURN n.k", at_least(elsewhere)))
        .await
        .expect_err("another stream's token");
    assert_eq!(err.code, ErrorCode::InvalidArgument, "{err:?}");
    let mut bad = eventual();
    bad.level = Some(pb::__buffa::oneof::consistency::Level::AtLeast(
        "not-a-token".to_string(),
    ));
    let err = cluster
        .node(other)
        .admin
        .execute(read_at("kg", "MATCH (n:N) RETURN n.k", bad))
        .await
        .expect_err("not a token");
    assert_eq!(err.code, ErrorCode::InvalidArgument, "{err:?}");
}

/// An `eventual` read on a follower's node is served there, even while the follower lags: a
/// paused tail leaves it at the older state, and its token says so. An `at_least` read with the
/// newer token waits for the tail.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn eventual_read_served_by_follower() {
    let cluster = Cluster::start(2, OwnerConfig::default()).await;
    create_replicated(&cluster.node(1).admin, "kg", 1).await;
    let graph = cluster.graph_id("kg").await;
    let owner = cluster.preferred(graph);
    let other = 3 - owner;
    let first = run(
        &cluster.node(owner).admin,
        "kg",
        "INSERT (:N {k: 1, v: 'a'})",
    )
    .await;
    let follower_node = &cluster.node(other).admin;
    values_at(follower_node, "kg", eventual()).await;
    let forwards = cluster.forwarder.forwards();

    follower_node.followers().expect("followers").pause(true);
    let second = run(
        &cluster.node(owner).admin,
        "kg",
        "INSERT (:N {k: 2, v: 'b'})",
    )
    .await;
    let (seen, read) = values_at(follower_node, "kg", eventual()).await;
    assert_eq!(seen, kv(&[(1, "a")]), "the lagging follower's state");
    assert_eq!(read, token(&first));
    // A stream and a plan are served by the follower as well.
    let chunks: Vec<pb::ResultChunk> = {
        use futures::StreamExt as _;
        follower_node
            .execute_stream(pb::ExecuteStreamRequest {
                request: read_at("kg", "MATCH (n:N) RETURN n.k", eventual()).into(),
                ..Default::default()
            })
            .await
            .expect("a stream")
            .map(|chunk| chunk.expect("chunk"))
            .collect()
            .await
    };
    assert_eq!(chunks.iter().map(|c| c.rows.len()).sum::<usize>(), 1);
    assert_eq!(
        chunks.last().map(|c| c.consistency_token.clone()),
        Some(token(&first).to_string())
    );
    follower_node
        .explain(pb::ExplainRequest {
            namespace: "acme".to_string(),
            graph: "kg".to_string(),
            statement: "MATCH (n:N) RETURN n.k".to_string(),
            consistency: eventual().into(),
            ..Default::default()
        })
        .await
        .expect("a plan");
    assert_eq!(
        cluster.forwarder.forwards(),
        forwards,
        "nothing left the node"
    );

    follower_node.followers().expect("followers").pause(false);
    let (seen, read) = values_at(follower_node, "kg", at_least(token(&second))).await;
    assert_eq!(seen, kv(&[(1, "a"), (2, "b")]));
    assert_eq!(read, token(&second));
    assert_eq!(cluster.forwarder.forwards(), forwards);
}

/// A `strong` read on a follower's node goes to the owner and sees every acknowledged write,
/// however far the follower lags; so does a write sent with a replica consistency.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn strong_read_routed_to_owner() {
    let cluster = Cluster::start(2, OwnerConfig::default()).await;
    create_replicated(&cluster.node(1).admin, "kg", 1).await;
    let graph = cluster.graph_id("kg").await;
    let owner = cluster.preferred(graph);
    let other = 3 - owner;
    run(
        &cluster.node(owner).admin,
        "kg",
        "INSERT (:N {k: 1, v: 'a'})",
    )
    .await;
    let follower_node = &cluster.node(other).admin;
    values_at(follower_node, "kg", eventual()).await;
    follower_node.followers().expect("followers").pause(true);
    let written = run(
        &cluster.node(owner).admin,
        "kg",
        "INSERT (:N {k: 2, v: 'b'})",
    )
    .await;

    let forwards = cluster.forwarder.forwards();
    let (seen, read) = values_at(follower_node, "kg", ReadConsistency::Strong.to_proto()).await;
    assert_eq!(seen, kv(&[(1, "a"), (2, "b")]));
    assert_eq!(read, token(&written));
    // The default (no consistency) is strong.
    assert_eq!(values(follower_node, "kg").await.len(), 2);
    assert_eq!(cluster.forwarder.forwards(), forwards + 2);

    // A write is the owner's, whatever consistency it carries.
    let wrote = follower_node
        .execute(read_at("kg", "INSERT (:N {k: 3, v: 'c'})", eventual()))
        .await
        .expect("a write through the follower's node");
    assert_eq!(token(&wrote).offset, token(&written).offset + 1);
    assert_eq!(cluster.forwarder.forwards(), forwards + 3);
    let (seen, _) = values_at(follower_node, "kg", eventual()).await;
    assert_eq!(
        seen,
        kv(&[(1, "a")]),
        "the paused follower never took a write"
    );
}
