//! The multi-node harness the ownership and fault tests share (GR1 Tasks 12, 15, 16): several
//! in-process nodes over one metastore and one bucket, each with its own engine, log writer,
//! view of the graph nodes and leases, forwarding to each other in process.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use connectrpc::{ConnectError, ErrorCode};
use loams_cache::{RangeCache, RangeCacheConfig};
use loams_common::StreamId;
use loams_common::meta::{Consistency, MetaStore};
use loams_graph::catalog::GraphCatalog;
use loams_graph::changeset::{GraphRecord, decode};
use loams_graph::durable::{ConsistencyToken, GraphLog, GroupCommit};
use loams_graph::owner::{
    FixedMembership, ForwardFuture, ForwardMethod, Forwarder, GraphNode, OwnerConfig, Ownership,
    owner_lease_key, rendezvous,
};
use loams_graph::service::admin::GraphAdmin;
use loams_graph::snapshot::CheckpointPolicy;
use loams_graph::{Engine, GraphId};
use loams_log::{FetchRequest, LogReader, LogWriter, OffsetRecord};
use loams_meta::{MetaClient, MetaClientConfig, MetaConfig, MetaNode, Router, SystemClock};
use loams_proto::loams::errors::v1::ErrorInfo;
use loams_proto::loams::graph::v1 as pb;
use loams_store::Store;
use pb::__buffa::oneof::value::Kind;
use tempfile::TempDir;

/// Calls another node's `serve_forwarded` in process; a node missing from the map is
/// unreachable.
#[derive(Debug, Default)]
pub struct InProcess {
    nodes: Mutex<HashMap<u64, GraphAdmin>>,
    /// Calls forwarded so far, reachable or not.
    forwards: std::sync::atomic::AtomicUsize,
}

impl InProcess {
    pub fn cut_off(&self, node_id: u64) -> Option<GraphAdmin> {
        self.nodes.lock().expect("lock").remove(&node_id)
    }

    pub fn reconnect(&self, node_id: u64, admin: GraphAdmin) {
        self.nodes.lock().expect("lock").insert(node_id, admin);
    }

    pub fn forwards(&self) -> usize {
        self.forwards.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Forwarder for InProcess {
    fn forward(
        &self,
        to: &GraphNode,
        method: ForwardMethod,
        request: Bytes,
        _timeout: Duration,
    ) -> ForwardFuture {
        self.forwards
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let admin = self.nodes.lock().expect("lock").get(&to.node_id).cloned();
        Box::pin(async move {
            match admin {
                Some(admin) => admin.serve_forwarded(method, request).await,
                None => Err(ConnectError::new(
                    ErrorCode::Unavailable,
                    "the graph's owner is unreachable",
                )),
            }
        })
    }
}

/// One node: an engine, a log writer of its own, its view of the graph nodes, its leases.
pub struct Node {
    pub me: GraphNode,
    pub admin: GraphAdmin,
    pub log: GraphLog,
    pub membership: Arc<FixedMembership>,
    pub ownership: Arc<Ownership>,
    _writer: LogWriter,
    pub data: TempDir,
}

/// A metastore, a bucket and `n` nodes.
pub struct Cluster {
    pub meta_node: MetaNode,
    _meta_dir: TempDir,
    pub meta: Arc<dyn MetaStore>,
    pub store: Store,
    pub reader: LogReader,
    pub forwarder: Arc<InProcess>,
    pub nodes: Vec<Node>,
}

/// Leases short enough that a test can wait one out.
pub const SHORT: OwnerConfig = OwnerConfig {
    ttl: Duration::from_millis(1_500),
    renew_every: Duration::from_millis(300),
};

impl Cluster {
    pub async fn start(n: u64, config: OwnerConfig) -> Self {
        Self::start_on(n, config, Store::in_memory(), GroupCommit::default()).await
    }

    pub async fn start_on(n: u64, config: OwnerConfig, store: Store, group: GroupCommit) -> Self {
        Self::start_with(n, config, store, group, None).await
    }

    /// [`Cluster::start_on`], each node with checkpoint policy `checkpoint` when given.
    pub async fn start_with(
        n: u64,
        config: OwnerConfig,
        store: Store,
        group: GroupCommit,
        checkpoint: Option<CheckpointPolicy>,
    ) -> Self {
        let meta_dir = TempDir::new().expect("temp dir");
        let clock = Arc::new(SystemClock);
        let mut meta_config = MetaConfig::new(1, meta_dir.path(), Store::in_memory());
        meta_config.clock = clock.clone();
        let meta_node = MetaNode::start(meta_config, &Router::new())
            .await
            .expect("start meta");
        meta_node.initialize([1]).await.expect("initialize");
        meta_node
            .wait_for_leader(Duration::from_secs(30))
            .await
            .expect("leader");
        let meta: Arc<dyn MetaStore> = Arc::new(MetaClient::new(
            meta_node.clone(),
            vec![],
            clock,
            MetaClientConfig::default(),
        ));
        let cache = RangeCache::new(
            store.clone(),
            RangeCacheConfig {
                memory_bytes: 16 << 20,
                ..RangeCacheConfig::default()
            },
        )
        .await
        .expect("range cache");
        let reader = LogReader::new(meta.clone(), cache);
        let forwarder = Arc::new(InProcess::default());
        let everyone: Vec<GraphNode> = (1..=n)
            .map(|node_id| GraphNode {
                node_id,
                incarnation: ulid::Ulid::generate(),
                addr: format!("node-{node_id}"),
            })
            .collect();
        let mut nodes = Vec::new();
        for me in &everyone {
            let writer = LogWriter::start(
                meta.clone(),
                store.clone(),
                GraphLog::log_config(me.node_id),
            )
            .expect("log writer");
            let log = GraphLog::new(
                meta.clone(),
                writer.clone(),
                reader.clone(),
                tokio::runtime::Handle::current(),
            )
            .with_group_commit(group);
            let membership = Arc::new(FixedMembership::new(me.clone(), everyone.clone()));
            let ownership = Ownership::new(
                meta.clone(),
                membership.clone(),
                Some(forwarder.clone() as Arc<dyn Forwarder>),
                config,
            );
            let data = TempDir::new().expect("data dir");
            let admin = GraphAdmin::new(
                Arc::new(Engine::with_data_dir(data.path())),
                GraphCatalog::new(meta.clone(), store.clone()),
            )
            .with_log(log.clone())
            .with_ownership(ownership.clone());
            let admin = match checkpoint {
                Some(policy) => admin.with_checkpoint_policy(policy),
                None => admin,
            };
            forwarder.reconnect(me.node_id, admin.clone());
            nodes.push(Node {
                me: me.clone(),
                admin,
                log,
                membership,
                ownership,
                _writer: writer,
                data,
            });
        }
        Self {
            meta_node,
            _meta_dir: meta_dir,
            meta,
            store,
            reader,
            forwarder,
            nodes,
        }
    }

    pub fn node(&self, node_id: u64) -> &Node {
        &self.nodes[usize::try_from(node_id - 1).expect("small")]
    }

    pub fn catalog(&self) -> GraphCatalog {
        GraphCatalog::new(self.meta.clone(), self.store.clone())
    }

    pub async fn graph_id(&self, name: &str) -> GraphId {
        self.node(1)
            .admin
            .catalog()
            .get_by_name("acme", name)
            .await
            .expect("catalog record")
            .graph_id()
            .expect("graph id")
    }

    pub async fn stream_of(&self, name: &str) -> StreamId {
        self.node(1)
            .admin
            .catalog()
            .get_by_name("acme", name)
            .await
            .expect("catalog record")
            .stream
            .expect("a stream")
    }

    /// The node the rendezvous places `graph` on, among every node.
    pub fn preferred(&self, graph: GraphId) -> u64 {
        let all: Vec<GraphNode> = self.nodes.iter().map(|n| n.me.clone()).collect();
        rendezvous(graph, &all).expect("a node").node_id
    }

    /// Every record of a stream, from its start.
    pub async fn records(&self, stream: StreamId) -> Vec<OffsetRecord> {
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

    /// `(lease_epoch, txn_seq)` of every change set of a stream, in log order.
    pub async fn epochs(&self, stream: StreamId) -> Vec<(u64, u64)> {
        self.records(stream)
            .await
            .iter()
            .map(
                |r| match decode(r.record.value.as_deref().expect("value")) {
                    Ok(GraphRecord::ChangeSet(change)) => (change.lease_epoch, change.txn_seq),
                    other => panic!("{other:?}"),
                },
            )
            .collect()
    }
}

pub fn info(err: &ConnectError) -> ErrorInfo {
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

pub async fn create(admin: &GraphAdmin, name: &str) {
    admin
        .create_graph(pb::CreateGraphRequest {
            namespace: "acme".to_string(),
            name: name.to_string(),
            ..Default::default()
        })
        .await
        .expect("create");
}

pub fn request(graph: &str, statement: &str, key: &str) -> pb::ExecuteRequest {
    pb::ExecuteRequest {
        namespace: "acme".to_string(),
        graph: graph.to_string(),
        statement: statement.to_string(),
        idempotency_key: key.to_string(),
        ..Default::default()
    }
}

pub async fn run(admin: &GraphAdmin, graph: &str, statement: &str) -> pb::ExecuteResponse {
    admin
        .execute(request(graph, statement, ""))
        .await
        .unwrap_or_else(|err| panic!("{statement}: {err:?}"))
}

pub fn token(response: &pb::ExecuteResponse) -> ConsistencyToken {
    response
        .consistency_token
        .parse()
        .unwrap_or_else(|err| panic!("{:?}: {err}", response.consistency_token))
}

/// `k → v` of every `:N` node, read through `admin`.
pub async fn values(admin: &GraphAdmin, graph: &str) -> BTreeMap<i64, String> {
    run(admin, graph, "MATCH (n:N) RETURN n.k, n.v")
        .await
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

/// Whether `admin`'s engine has the graph open.
pub fn is_open(admin: &GraphAdmin, graph: &str) -> bool {
    admin
        .engine()
        .list(Some("acme"))
        .expect("list")
        .iter()
        .any(|g| g.name() == graph)
}

/// The longest [`wait_out_the_lease`] waits. A paused owner's lease runs out within its TTL, plus
/// one renewal still in flight, so this is reached only when something still renews the lease.
pub const LEASE_WAIT: Duration = Duration::from_secs(30);

/// Waits until `graph`'s owner lease is out once its owner's renewals are paused: no node holds
/// it any more ([`Ownership::held`]), and the metastore has it released or expired.
///
/// Both are read again each time round. A renewal the owner sent just before its renewals were
/// paused can still land after that, and move the deadline on. So a deadline read once is not
/// enough: the survivor then finds the lease held and is answered "served by node N now; retry"
/// (#1085). The owner's renewer lets go of the lease only after its last renewal has answered.
/// So once no node holds the lease, nothing in flight can extend it.
///
/// # Panics
///
/// When the lease is still held after [`LEASE_WAIT`].
pub async fn wait_out_the_lease(cluster: &Cluster, graph: GraphId) {
    let key = owner_lease_key(graph);
    let started = std::time::Instant::now();
    loop {
        let lease = cluster
            .meta
            .lease(Consistency::Linearizable, &key)
            .await
            .expect("read");
        let held_at_meta = lease
            .as_ref()
            .is_some_and(|lease| lease.is_held_at(cluster.meta.now_ms()));
        let holders: Vec<u64> = cluster
            .nodes
            .iter()
            .filter(|node| node.ownership.held(graph).is_some())
            .map(|node| node.me.node_id)
            .collect();
        if !held_at_meta && holders.is_empty() {
            return;
        }
        assert!(
            started.elapsed() < LEASE_WAIT,
            "graph {graph}'s owner lease is still held after {LEASE_WAIT:?}: {lease:?}, held by \
             nodes {holders:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A hard deadline over a whole test, its runtime's shutdown included (#1085): past `limit` it
/// prints the phase the test last named and aborts the process. A hang then fails in minutes, and
/// says where it was, instead of holding the CI job until its timeout. Dropping it disarms it.
///
/// Create it before the test's runtime, so the runtime drops first: a runtime waits for its
/// blocking tasks when it drops, and that wait is inside the deadline too.
pub struct Deadline {
    phase: Arc<Mutex<String>>,
    disarm: Option<std::sync::mpsc::Sender<()>>,
    watchdog: Option<std::thread::JoinHandle<()>>,
}

impl Deadline {
    pub fn start(test: &'static str, limit: Duration) -> Self {
        let phase = Arc::new(Mutex::new("starting".to_string()));
        let (disarm, disarmed) = std::sync::mpsc::channel::<()>();
        let seen = Arc::clone(&phase);
        let watchdog = std::thread::Builder::new()
            .name("test-deadline".to_string())
            .spawn(move || {
                if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
                    disarmed.recv_timeout(limit)
                {
                    let phase = seen
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone();
                    eprintln!(
                        "{test}: still running after {limit:?}, at {phase:?}: it hangs; aborting \
                         the process so the test fails now"
                    );
                    std::process::abort();
                }
            })
            .expect("the deadline's thread");
        Self {
            phase,
            disarm: Some(disarm),
            watchdog: Some(watchdog),
        }
    }

    /// Names the phase the test is in now, for the message if the deadline passes.
    pub fn phase(&self, phase: impl Into<String>) {
        *self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = phase.into();
    }
}

impl Drop for Deadline {
    fn drop(&mut self) {
        drop(self.disarm.take());
        if let Some(watchdog) = self.watchdog.take() {
            let _ = watchdog.join();
        }
    }
}

/// A multi-thread runtime of `workers` threads, as `#[tokio::test(flavor = "multi_thread")]`
/// builds, for a test that keeps a [`Deadline`] over it.
pub fn runtime(workers: usize) -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .enable_all()
        .build()
        .expect("a runtime")
}
