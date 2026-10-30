//! One Loams process: a single-node metastore, the log, the range cache,
//! a worker running the background tasks, the collection service, the
//! native HTTP API and the Flight SQL listener (design §10 §1); or, with
//! [`ServerConfig::cluster`], one node of `loams cluster` running the
//! components of its roles (plan M1.3 Task 11).

use std::collections::BTreeMap;
use std::net::SocketAddr;
#[cfg(unix)]
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use loams_cache::{RangeCache, RangeCacheConfig};
use loams_collection::{
    CollectionConfig, CollectionContext, CollectionGcRoots, CollectionTargetFactory,
    CollectionTrimSource, CollectionWriter, IndexBuildSource, LanceCompactionSource, LanceConfig,
    LanceEnv, MaintenanceConfig, ManifestCache, PkGcRoots, SplitMergeSource,
};
use loams_common::meta::MetaStore;
use loams_hnsw::HnswEngine;
use loams_hot::{
    AlwaysLocal, ForwardStats, HotBuildConfig, HotBuildSource, HotTierConfig, HotTierImpl,
    NodeDescriptor, NodeRegistry, PlacementImpl, RegistryConfig, RemoteReadsConfig,
    RemoteReadsImpl, Roles,
};
use loams_link::{CounterTargetFactory, LinkApplySource, LinkConfig, LinkGcRoots, TargetRegistry};
use loams_log::gc::{GcConfig, GcSource};
use loams_log::{
    LogConfig, LogReader, LogWriter, RetentionConfig, RetentionSource, SegmenterConfig,
    SegmenterSource,
};
use loams_meta::rpc::{self as meta_rpc, JoinRequest, LeaveRequest};
use loams_meta::{
    HttpTransport, HttpTransportConfig, MetaClient, MetaClientConfig, MetaConfig, MetaNode, Router,
    SystemClock, Transport,
};
use loams_query::flight::{FlightConfig, PutTasks, serve_flight_sql_wrapped};
use loams_query::flight_ingest::StreamProducer;
use loams_query::placement::Placement;
use loams_query::{CollectionService, ServiceConfig};
use loams_store::Store;
use loams_worker::{TaskSource, Worker, WorkerConfig, WorkerHandle};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use ulid::Ulid;

use crate::api::internal::NodeInfo;
use crate::api::{self, AppState, ForwardedReads, NativeStreamProducer};
use crate::cluster::{self, ClusterInfo, LateRouter, MembershipSource};
use crate::meta_backend::MetaBackend;

/// The meta node id of a single-process Loams.
const NODE_ID: u64 = 1;
/// How long startup waits for the single meta node to become leader.
const LEADER_WAIT: Duration = Duration::from_secs(30);
/// How long shutdown lets in-flight HTTP requests (such as long-polls) finish.
const HTTP_GRACE: Duration = Duration::from_secs(10);
/// How long a cluster node waits for a leader after joining (rule 3.5).
const CLUSTER_LEADER_WAIT: Duration = Duration::from_secs(30);
/// How long a leaving learner tries to reach the leader at shutdown.
const LEAVE_WAIT: Duration = Duration::from_secs(10);

/// One node of `loams cluster` (plan M1.3 Task 11; Ruling 15).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClusterConfig {
    pub node_id: u64,
    pub roles: Roles,
    /// Where other nodes reach this one (`host:port`; default `--listen`).
    pub advertise: String,
    /// The `meta` nodes: id → `host:port`.
    pub peers: BTreeMap<u64, String>,
    /// Inherit a pre-bound listener from stdin (test harness only).
    pub listen_stdin: bool,
    /// Empty when unset.
    pub zone: String,
    /// Owners per collection (1).
    pub replication: usize,
    pub registry: RegistryConfig,
    pub transport: HttpTransportConfig,
    /// How long the start-up join may retry (120 s).
    pub join_deadline: Duration,
    /// A learner whose node lease expired this long ago is removed (10 min).
    pub learner_expiry: Duration,
    /// How often the `meta-membership` task runs (60 s).
    pub membership_interval: Duration,
}

impl ClusterConfig {
    /// The defaults for node `node_id` with `roles`, reachable at
    /// `advertise`, in a cluster whose `meta` nodes are `peers`.
    pub fn new(
        node_id: u64,
        roles: Roles,
        advertise: impl Into<String>,
        peers: BTreeMap<u64, String>,
    ) -> Self {
        Self {
            node_id,
            roles,
            advertise: advertise.into(),
            peers,
            listen_stdin: false,
            zone: String::new(),
            replication: 1,
            registry: RegistryConfig::default(),
            transport: HttpTransportConfig::default(),
            join_deadline: Duration::from_secs(120),
            learner_expiry: Duration::from_secs(600),
            membership_interval: Duration::from_secs(60),
        }
    }

    /// Rule 1.
    pub fn validate(&self) -> Result<(), ServerError> {
        let config = |message: String| Err(ServerError::Config(message));
        if self.roles.is_empty() {
            return config("--roles must name at least one role".to_string());
        }
        if self.peers.is_empty() {
            return config("--peers must name at least one meta node".to_string());
        }
        for (id, addr) in &self.peers {
            if !cluster::is_host_port(addr) {
                return config(format!(
                    "--peers: node {id}'s address {addr:?} is not host:port"
                ));
            }
        }
        if !cluster::is_host_port(&self.advertise) {
            return config(format!("--advertise {:?} is not host:port", self.advertise));
        }
        if let Ok(addr) = self.advertise.parse::<SocketAddr>()
            && addr.ip().is_unspecified()
        {
            return config(format!(
                "--advertise {addr} is an unspecified address; set --advertise to an address other nodes can reach"
            ));
        }
        match (self.roles.meta, self.peers.get(&self.node_id)) {
            (true, None) => {
                return config(format!(
                    "node {} has the meta role but is not in --peers",
                    self.node_id
                ));
            }
            (true, Some(addr)) if *addr != self.advertise => {
                return config(format!(
                    "node {}'s --peers address {addr} differs from --advertise {}",
                    self.node_id, self.advertise
                ));
            }
            (false, Some(_)) => {
                return config(format!(
                    "node {} is in --peers but has no meta role",
                    self.node_id
                ));
            }
            _ => {}
        }
        if self.replication == 0 {
            return config("--replication must be at least 1".to_string());
        }
        Ok(())
    }

    /// The roles the node runs: `gateway` adds `log` (the node that receives
    /// a write appends it).
    pub fn effective_roles(&self) -> Roles {
        let mut roles = self.roles;
        if roles.gateway && !roles.log {
            tracing::info!("the gateway role adds the log role");
            roles.log = true;
        }
        roles
    }
}

/// How to run a single-process Loams.
#[derive(Clone, Debug)]
pub struct ServerConfig {
    /// Holds the metastore's local database (`<data_dir>/meta`) and, without
    /// `bucket`, the local bucket (`<data_dir>/bucket`).
    pub data_dir: PathBuf,
    pub listen: SocketAddr,
    /// Object store URL (`s3://…`, `gs://…`, `az://…`, `file:///…`). Default
    /// `file://<data_dir>/bucket`.
    pub bucket: Option<String>,
    pub log: LogConfig,
    pub segmenter: SegmenterConfig,
    pub retention: RetentionConfig,
    pub cache: RangeCacheConfig,
    pub link: LinkConfig,
    pub gc: GcConfig,
    /// How collections commit, index, trim and keep manifests. The server
    /// overrides `max_commit_delay` with `link.max_commit_delay` and
    /// `keep_manifests` with `gc.keep_manifests`.
    pub collection: CollectionConfig,
    pub lance: LanceConfig,
    /// The metastore builds a snapshot after this many log entries. Default
    /// 10 000.
    pub snapshot_every: u64,
    /// How often the worker polls its task sources. Default 1 s.
    pub worker_poll_interval: Duration,
    /// The worker's task lease TTL. Default 30 s.
    pub worker_lease_ttl: Duration,
    /// The collection service: partitions, the hot default, tails, reads,
    /// search and SQL limits.
    pub query: ServiceConfig,
    /// Where Flight SQL listens (with the `flight` feature); `None` (the
    /// default here) serves no Flight SQL. `loams dev` and `standalone`
    /// set it.
    pub flight_sql: Option<SocketAddr>,
    /// Native stream gRPC listener for Dapr protocol adapters.
    #[cfg(feature = "stream-grpc")]
    pub stream_grpc: Option<SocketAddr>,
    /// The connect socket (`--connect-socket`, CC1 ruling R1.1): the main
    /// API on a Unix socket, mode 0660, for `loams-fabric connect`, which
    /// shares the socket's volume and nothing else with the engine. It takes
    /// API keys only, in every auth mode. `None` (the default) binds none.
    pub connect_socket: Option<PathBuf>,
    /// The Loams SQL gate (plan SQ1 Task 4).
    #[cfg(feature = "sqldb")]
    pub sqlgate: Option<crate::sqlgate::SqlgateConfig>,
    /// How Flight SQL bounds its statements and its ingest.
    pub flight: FlightConfig,
    /// CloudEvents ingest: the dedup window (design §02 §7.4).
    pub cloudevents: crate::api::events::EventsConfig,
    /// MySQL wire listener over collections, when the mysql-wire feature is on.
    #[cfg(feature = "mysql-wire")]
    pub mysql_wire: Option<crate::mysql_wire::MysqlConfig>,
    /// PostgreSQL wire listener over collections, when the pgwire feature is on.
    #[cfg(feature = "pgwire")]
    pub pg: Option<crate::pg::PgConfig>,
    /// Split merges and Lance compaction (plan M1.3 Tasks 1–2); a source
    /// whose switch (`merge`, `compaction`) is off is not run.
    pub maintenance: MaintenanceConfig,
    /// This node's hot tier (plan M1.3 Tasks 6–8). `enabled` false (`--hot
    /// off`): no tier, no artifact builds, every read cold.
    pub hot: HotTierConfig,
    /// Hot artifact builds (plan M1.3 Task 5). The server sets `pin_all` to
    /// `hot.pin_all`.
    pub hot_build: HotBuildConfig,
    /// The HNSW engine of artifact builds and the tier; `None` =
    /// `loams_hnsw::default_engine()` (qdrant-edge with the `hnsw` feature;
    /// without it, no artifact is built unless this is set). Tests set
    /// `FlatEngine`.
    pub hnsw_engine: Option<Arc<dyn HnswEngine>>,
    /// `None`: `dev` or `standalone` (one node, every role). `Some`: one
    /// node of `loams cluster` (plan M1.3 Task 11).
    pub cluster: Option<ClusterConfig>,
    /// The Qdrant gateway's listeners and limits (plan M1.4, feature
    /// `qdrant`), served on gateway nodes. `None` (the default here, E12)
    /// serves no Qdrant API; the CLI sets it unless `--no-qdrant`.
    #[cfg(feature = "qdrant")]
    pub qdrant: Option<loams_qdrant::QdrantConfig>,
    /// The embedded durable server (D1, feature `durable`): its loopback
    /// listener and its store. `None` (the default here) serves none; the
    /// CLI sets it unless `--no-durable`. A cluster node needs a MySQL store.
    #[cfg(feature = "durable")]
    pub durable: Option<loams_durable::DurableConfig>,
    /// The metastore (R1 plan Task 6, D124): the embedded openraft store
    /// (the default) or TiKV (`--meta tikv://…`, on `dev` and `standalone`
    /// only; `loams cluster` refuses it).
    pub meta: MetaBackend,
    /// The Elasticsearch gateway's listener and limits (plan M1.5, feature
    /// `es`), served on gateway nodes. `None` (the default here, row E13)
    /// serves no Elasticsearch API; the CLI sets it unless `--no-es`.
    #[cfg(feature = "es")]
    pub es: Option<loams_es::EsConfig>,
    /// Serve `grpc.reflection.v1` on the main port (design §44 §4). Off
    /// (the default here); `loams dev` turns it on, which is Q603's proposed
    /// answer. Reflection publishes the schema of the API to anyone who can
    /// reach the port, so it is a development convenience, not a default.
    pub reflection: bool,
    /// Loams Live (R1 plan Task 12, feature `live`): the `loams.live.v1` sync
    /// API on its own loopback listener, on `dev` and `standalone`. `None`
    /// (the default here) serves no Live API; the CLI sets it unless
    /// `--no-live`.
    #[cfg(feature = "live")]
    pub live: Option<loams_live::LiveConfig>,
    /// Loams Graph (GR1 Task 5, design §48, D741; feature `graph`): where graphs live and how
    /// often deleted graphs are purged. Served on `dev` and `standalone` (a cluster node gets
    /// the `graph` role in Task 12).
    #[cfg(feature = "graph")]
    pub graph: GraphConfig,
    /// Loams Git (GT1 Task 13, design §36; feature `git`): `RepoService` on the main port of a
    /// `gateway` node, over the bucket, and the purge of deleted repositories.
    #[cfg(feature = "git")]
    pub git: GitConfig,
    /// `[house]`: the Loams House front the main port proxies
    /// `/loams.house.v1.*` to (HS1 Task 7, design §49 §18.1). No endpoint (the
    /// default) answers `house_not_configured`.
    pub house: crate::api::house_proxy::HouseProxyConfig,
    /// Loams Meter's `meter` role (`LOAMS_METER_*`): its House (design §63
    /// §7.5; no URL, the default, runs it without its House side) and the
    /// key its webhook secrets are sealed to (D2205).
    pub meter: crate::api::meter::MeterConfig,
    /// Who may call the engine (`LOAMS_AUTH_MODE`, [`crate::auth`]). Off
    /// (the default) refuses to start unless every listener is loopback.
    /// `dev` and `standalone` only: a cluster node refuses any other mode
    /// until MT1.
    pub auth: crate::auth::AuthConfig,
    /// TLS on `--listen` (`LOAMS_TLS_*`, [`crate::tls`]; LV1 plan Task 18).
    /// Off by default. `dev` and `standalone` only. Loams Live on a
    /// non-loopback `--listen` needs it (or TLS ended at a trusted proxy)
    /// and authentication.
    pub tls: crate::tls::TlsConfig,
    /// The built console the main port and the `tailscale serve` target
    /// serve at `/ui/` ([`crate::console`]), on `dev` and `standalone`.
    /// `None` (the default here) serves none; the CLI sets it to
    /// `--console-dir`, else to [`crate::console::DEFAULT_DIR`] when that
    /// holds a build, unless `--no-console`.
    pub console_dir: Option<PathBuf>,
    /// How the console is served: the agentd hosts its CSP header lets
    /// Studio call (`LOAMS_AGENTD_ORIGINS`, [`crate::console`]).
    pub console: crate::console::ConsoleOptions,
}

/// Loams Git's server settings (GT1 Task 13).
#[cfg(feature = "git")]
#[derive(Clone, Debug)]
pub struct GitConfig {
    /// Serve `loams.repos.v1` at all (on a `gateway` node).
    pub enabled: bool,
    /// How long a `WatchRepo` stream stays quiet before it sends a heartbeat (AP0: 15 s).
    pub watch_heartbeat: Duration,
    /// How often deleted repositories are looked at for purging (default 1 min).
    pub purge_every: Duration,
    /// How long a deleted repository is kept before it is purged (`GC_GRACE`, 1 h).
    pub purge_grace: Duration,
    /// The git listener stock git authenticates against with git tokens (`--git-listen`, GT1
    /// Task 16); loopback only until MT1 (D111). `None`: not served.
    pub listen: Option<SocketAddr>,
    /// The git token file (`--git-tokens`): SHA-256 hashes and their grants. Needed with
    /// `listen`.
    pub tokens: Option<PathBuf>,
    /// How often the `_git` mirror is repaired: whatever a crash left unmirrored is published
    /// (GT1 Task 15; default 1 h).
    pub mirror_repair_every: Duration,
    /// Workspaces are read only by their owner and org admins (`--git-workspace-reads-owner`,
    /// GT-Q13's alternative). Off by default: whoever may read a repository reads its
    /// workspaces (GT1 Task 25).
    pub workspace_reads_owner_only: bool,
}

#[cfg(feature = "git")]
impl Default for GitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            watch_heartbeat: Duration::from_secs(15),
            purge_every: Duration::from_secs(60),
            purge_grace: loams_git::limits::GC_GRACE,
            listen: None,
            tokens: None,
            mirror_repair_every: Duration::from_secs(3600),
            workspace_reads_owner_only: false,
        }
    }
}

/// The largest engine statement time limit (§48 §13.1's maximum statement timeout).
#[cfg(feature = "graph")]
const MAX_GRAPH_QUERY_TIMEOUT: Duration = Duration::from_secs(300);

/// Loams Graph's server settings (GR1 Task 5).
#[cfg(feature = "graph")]
#[derive(Clone, Debug)]
pub struct GraphConfig {
    /// Serve Loams Graph at all (`--no-graph` turns it off; its RPCs then answer
    /// `feature_not_in_variant`).
    pub enabled: bool,
    /// The graph data directory: graphs live under `<data_dir>/graphs/<graph_id>/`. `None`
    /// refuses to start unless [`GraphConfig::ephemeral`] (a served graph must be persistent:
    /// an in-memory one fails rather than reopening after an engine panic, Task 3 review I2).
    pub data_dir: Option<PathBuf>,
    /// Keep graphs in memory, explicitly (`--graph-ephemeral`; dev and tests).
    pub ephemeral: bool,
    /// How long a deleted graph's storage is kept before it is purged.
    pub retention_hold: Duration,
    /// How long a superseded catalog document is kept (floor 5 minutes outside tests).
    pub sweep_grace: Duration,
    /// How often the purge and the catalog sweep run (config only, no flag; default 10 min;
    /// must be greater than zero).
    pub maintenance_every: Duration,
    /// When graphs are snapshotted and how long their history is kept (config only; GR1 Task
    /// 13): a native snapshot every 15 minutes or 64 MiB of log, a portable one weekly, a PITR
    /// window of 7 days (at least 24 hours).
    pub checkpoint: loams_graph::snapshot::CheckpointPolicy,
    /// How often this node checks its graphs for a due snapshot, and for graphs to evict
    /// (config only; default 1 min; must be greater than zero).
    pub checkpoint_every: Duration,
    /// When open graphs are evicted (config only; GR1 Task 14): after an hour with no
    /// statement (`idle_after`, `graph.idle_evict_after`; must be greater than zero), and,
    /// least recently used first, while their memory is over `node_memory` (none until Task
    /// 26 sets it). An evicted graph is checkpointed first and reopens on its next statement.
    pub evict: loams_graph::evict::EvictPolicy,
    /// The cap on graph statements running at once in this process (`--graph-statement-slots`;
    /// default twice the cores, at most 32). Past it a statement is refused with
    /// `RESOURCE_EXHAUSTED`. It also sizes the statement pool (GR1 Task 6).
    pub statement_slots: usize,
    /// The cap on one namespace's graph statements at once (config only; default 64, §48
    /// §13.2), never more than `statement_slots` less a reserve of an eighth (at least one), so
    /// one namespace cannot take every slot. Past it a statement is refused with
    /// `RESOURCE_EXHAUSTED`/`quota_exceeded`.
    pub namespace_statements: usize,
    /// The server's per-statement limits (config only; §48 §13.1's defaults, each capped at
    /// its maximum). A graph's own `GraphLimits` apply on top (GR1 Task 6).
    pub limits: loams_graph::limits::StatementLimits,
    /// Grafeo's own statement time limit (`--graph-query-timeout-ms`; default 30 s, at most
    /// 300 s). A backstop only: Grafeo checks it between pipeline chunks of a statement without
    /// parameters, so a statement inside one long operator runs on (R0.8). It is also the
    /// largest deadline a request may ask for (R0.8 (a), GR1 Task 6); Loams's own deadline
    /// answers the client at the request's limit either way.
    pub query_timeout: Duration,
    /// How long an `at_least` read waits for the replica serving it to apply its token (config
    /// only; §48 §7.2's `consistency_wait_ms`, default 5 s; GR1 Task 15). Past it the read is
    /// `UNAVAILABLE`/`consistency_wait_timeout`.
    pub consistency_wait: Duration,
    /// How long the shutdown phase `graph` waits for running statements (config only; default
    /// 10 s), and again for a maintenance pass in progress. A statement still running then is
    /// detached: its graph stays open until it ends or the process exits (it runs on a
    /// statement pool thread, which the exit does not wait for; GR1 Task 6, R6.5).
    pub shutdown_wait: Duration,
}

#[cfg(feature = "graph")]
impl GraphConfig {
    /// Graphs under `data_dir`, with the default hold, grace and interval.
    pub fn under(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            enabled: true,
            data_dir: Some(data_dir.into()),
            ephemeral: false,
            retention_hold: loams_graph::service::admin::DEFAULT_RETENTION_HOLD,
            sweep_grace: loams_graph::catalog::DOCUMENT_GRACE,
            maintenance_every: Duration::from_secs(600),
            checkpoint: loams_graph::snapshot::CheckpointPolicy::default(),
            checkpoint_every: Duration::from_secs(60),
            evict: loams_graph::evict::EvictPolicy::default(),
            statement_slots: loams_graph::service::admin::default_statement_slots(),
            namespace_statements: loams_graph::limits::DEFAULT_NAMESPACE_STATEMENTS,
            limits: loams_graph::limits::StatementLimits::DEFAULT,
            query_timeout: loams_graph::engine::DEFAULT_QUERY_TIMEOUT,
            consistency_wait: loams_graph::consistency::DEFAULT_CONSISTENCY_WAIT,
            shutdown_wait: loams_graph::service::admin::DEFAULT_SHUTDOWN_WAIT,
        }
    }
}

impl ServerConfig {
    /// The defaults, with data in `data_dir`, listening on 127.0.0.1:8080.
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        let data_dir: PathBuf = data_dir.into();
        #[cfg(feature = "graph")]
        let data_dir_for_graph = data_dir.join("graph");
        Self {
            hot: HotTierConfig::new(&data_dir),
            hot_build: HotBuildConfig::new(&data_dir),
            maintenance: MaintenanceConfig::default(),
            hnsw_engine: None,
            data_dir,
            listen: SocketAddr::from(([127, 0, 0, 1], 8080)),
            bucket: None,
            log: LogConfig::new(NODE_ID),
            segmenter: SegmenterConfig::default(),
            retention: RetentionConfig::default(),
            cache: RangeCacheConfig::default(),
            link: LinkConfig::default(),
            gc: GcConfig::default(),
            collection: CollectionConfig::default(),
            lance: LanceConfig::default(),
            snapshot_every: 10_000,
            worker_poll_interval: Duration::from_secs(1),
            worker_lease_ttl: Duration::from_secs(30),
            query: ServiceConfig::default(),
            flight_sql: None,
            #[cfg(feature = "stream-grpc")]
            stream_grpc: None,
            connect_socket: None,
            #[cfg(feature = "sqldb")]
            sqlgate: None,
            flight: FlightConfig::default(),
            cloudevents: crate::api::events::EventsConfig::default(),
            #[cfg(feature = "mysql-wire")]
            mysql_wire: None,
            #[cfg(feature = "pgwire")]
            pg: None,
            cluster: None,
            #[cfg(feature = "qdrant")]
            qdrant: None,
            #[cfg(feature = "durable")]
            durable: None,
            meta: MetaBackend::Raft,
            #[cfg(feature = "es")]
            es: None,
            reflection: false,
            #[cfg(feature = "live")]
            live: None,
            #[cfg(feature = "graph")]
            graph: GraphConfig::under(data_dir_for_graph),
            #[cfg(feature = "git")]
            git: GitConfig::default(),
            house: crate::api::house_proxy::HouseProxyConfig::default(),
            meter: crate::api::meter::MeterConfig::default(),
            auth: crate::auth::AuthConfig::off(),
            tls: crate::tls::TlsConfig::off(),
            console_dir: None,
            console: crate::console::ConsoleOptions::default(),
        }
    }

    /// Rejects an invalid `flight` config ([`FlightConfig::validate`]), then
    /// any freshness deadline at or above `gc.grace`:
    /// `segmenter.swap_deadline`, `link.max_commit_delay` (which is also the
    /// collection commit delay: the server sets `collection.max_commit_delay`
    /// to it) and `collection.index_commit_delay`. Called first thing by
    /// [`Server::start`]; the error names the first violating deadline in
    /// that order.
    ///
    /// Garbage collection deletes unreferenced objects older than its grace
    /// period, so a segment swap, link commit or index build must reference
    /// its new objects strictly within it (M0.4 ruling E7, re-review m1;
    /// plan M1.1 Ruling 22).
    pub fn validate(&self) -> Result<(), ServerError> {
        if let Some(cluster) = &self.cluster {
            cluster.validate()?;
            if self.meta != MetaBackend::Raft {
                return Err(ServerError::Config(
                    "loams cluster runs its own openraft metastore; --meta is for dev and \
                     standalone only"
                        .to_string(),
                ));
            }
        }
        self.validate_durable()?;
        self.validate_auth()?;
        #[cfg(feature = "graph")]
        self.validate_graph()?;
        #[cfg(feature = "git")]
        if self.git.purge_every.is_zero()
            || self.git.watch_heartbeat.is_zero()
            || self.git.mirror_repair_every.is_zero()
        {
            return Err(ServerError::Config(
                "git.purge_every, git.watch_heartbeat and git.mirror_repair_every must be greater \
                 than zero"
                    .to_string(),
            ));
        }
        #[cfg(feature = "git")]
        if let Some(addr) = self.git.listen {
            if !addr.ip().is_loopback() {
                return Err(ServerError::GitListenNotLoopback { addr });
            }
            if self.git.tokens.is_none() {
                return Err(ServerError::Config(
                    "--git-listen needs --git-tokens: the git listener admits git tokens only"
                        .to_string(),
                ));
            }
        }
        #[cfg(feature = "live")]
        if let Some(live) = &self.live {
            if self.cluster.is_some() {
                return Err(ServerError::Config(
                    "Loams Live runs on dev and standalone only in R1; pass --no-live".to_string(),
                ));
            }
            if loams_live::check_listen(live.listen).is_err() {
                return Err(ServerError::LiveListenNotLoopback { addr: live.listen });
            }
            // LV1 plan Task 18 (the rule of MT1 Task 7): beyond loopback,
            // Live is served only behind TLS and authentication.
            if !self.listen.ip().to_canonical().is_loopback()
                && !(self.tls.secures() && self.auth.enforced())
            {
                return Err(ServerError::LiveListenNeedsTlsAndAuth { addr: self.listen });
            }
            // LV1 plan Task 5: strangers' code never runs in this process.
            loams_live::check_isolation(live).map_err(|e| ServerError::Config(e.to_string()))?;
        }
        self.flight.validate().map_err(ServerError::Config)?;
        self.validate_backpressure()?;
        self.cloudevents.validate().map_err(ServerError::Config)?;
        self.gc
            .check_deadlines(&[
                ("segmenter.swap_deadline", self.segmenter.swap_deadline),
                ("link.max_commit_delay", self.link.max_commit_delay),
                (
                    "collection.index_commit_delay",
                    self.collection.index_commit_delay,
                ),
                ("maintenance.commit_delay", self.maintenance.commit_delay),
                (
                    "hot_build.artifact_commit_delay",
                    self.hot_build.artifact_commit_delay,
                ),
            ])
            .map_err(|err| ServerError::Config(err.to_string()))
    }
}

impl ServerConfig {
    /// D1 Task 3: SQLite is a single-node store, so a cluster node needs
    /// MySQL (TiDB).
    #[cfg(feature = "durable")]
    fn validate_durable(&self) -> Result<(), ServerError> {
        if self.cluster.is_some()
            && let Some(durable) = &self.durable
            && let loams_durable::DurableStore::Sqlite { .. } = durable.store
        {
            return Err(ServerError::Config(
                "the sqlite durable store is single-node; use --durable-store mysql://… or tikv://…"
                    .to_string(),
            ));
        }
        Ok(())
    }

    #[cfg(not(feature = "durable"))]
    fn validate_durable(&self) -> Result<(), ServerError> {
        Ok(())
    }

    /// The network listeners [`crate::auth`] guards, by flag name.
    fn guarded_listeners(&self) -> Vec<(&'static str, SocketAddr)> {
        let mut listeners = vec![("--listen", self.listen)];
        if let Some(addr) = flight_addr(self) {
            listeners.push(("--flight-sql-listen", addr));
        }
        #[cfg(feature = "qdrant")]
        if let Some(qdrant) = &self.qdrant {
            listeners.push(("--qdrant-listen", qdrant.rest_listen));
            listeners.push(("--qdrant-grpc-listen", qdrant.grpc_listen));
        }
        #[cfg(feature = "es")]
        if let Some(es) = &self.es {
            listeners.push(("--es-listen", es.listen));
        }
        listeners
    }

    /// `off` only on loopback (preview contract); a cluster node keeps its
    /// private-network routes and refuses any other mode until MT1.
    fn validate_auth(&self) -> Result<(), ServerError> {
        self.tls.check().map_err(ServerError::Config)?;
        if self.cluster.is_some() && self.tls.configured() {
            return Err(ServerError::Config(
                "LOAMS_TLS_* are for dev and standalone; a cluster node's listeners stay on a \
                 private network until MT1"
                    .to_string(),
            ));
        }
        if self.cluster.is_some() && self.connect_socket.is_some() {
            return Err(ServerError::Config(
                "--connect-socket is for dev and standalone, not a cluster node".to_string(),
            ));
        }
        if self.cluster.is_some() {
            if self.auth.enforced() {
                return Err(ServerError::Config(format!(
                    "{} is for dev and standalone; a cluster node's listeners must stay on a \
                     private network until MT1",
                    crate::auth::MODE_ENV
                )));
            }
            return Ok(());
        }
        self.auth
            .tailscale_listener()
            .map_err(ServerError::Config)?;
        self.auth
            .check_listeners(&self.guarded_listeners())
            .map_err(ServerError::Config)
    }

    /// Whether this server serves Loams Graph (GR1 Task 5): unless `--no-graph`; on a cluster
    /// node only with the `graph` role, which owns graphs and forwards calls for graphs another
    /// graph node owns (GR1 Task 12, R5.2). Otherwise every graph RPC answers
    /// `feature_not_in_variant`.
    #[cfg(feature = "graph")]
    #[must_use]
    pub fn serves_graph(&self) -> bool {
        self.graph.enabled
            && self
                .cluster
                .as_ref()
                .is_none_or(|cluster| cluster.effective_roles().graph)
    }

    /// Loams Graph (GR1 Task 5): graphs need a data directory unless they are explicitly
    /// ephemeral, and graph RPCs are authorized per graph from the admitted principal (GR1
    /// Task 24, D750), so without authentication they are served on a loopback address only.
    #[cfg(feature = "graph")]
    fn validate_graph(&self) -> Result<(), ServerError> {
        if !self.serves_graph() {
            return Ok(());
        }
        if self.graph.data_dir.is_none() && !self.graph.ephemeral {
            return Err(ServerError::GraphNeedsDataDir);
        }
        // The graph authorizer reads the admitted principal's roles (GR1 Task
        // 24): with authentication off everyone is the operator, which only a
        // loopback listener may serve.
        if !self.listen.ip().is_loopback() && !self.auth.enforced() {
            return Err(ServerError::GraphListenNotLoopback { addr: self.listen });
        }
        if self.graph.maintenance_every.is_zero() {
            return Err(ServerError::Config(
                "graph.maintenance_every must be greater than zero".to_string(),
            ));
        }
        if self.graph.checkpoint_every.is_zero() {
            return Err(ServerError::Config(
                "graph.checkpoint_every must be greater than zero".to_string(),
            ));
        }
        if self.graph.evict.idle_after.is_zero() {
            return Err(ServerError::Config(
                "graph.evict.idle_after must be greater than zero".to_string(),
            ));
        }
        let max_slots = loams_graph::service::admin::MAX_STATEMENT_SLOTS;
        if !(1..=max_slots).contains(&self.graph.statement_slots) {
            return Err(ServerError::Config(format!(
                "--graph-statement-slots must be between 1 and {max_slots}"
            )));
        }
        if self.graph.query_timeout.is_zero() || self.graph.query_timeout > MAX_GRAPH_QUERY_TIMEOUT
        {
            return Err(ServerError::Config(format!(
                "--graph-query-timeout-ms must be between 1 and {} ms",
                MAX_GRAPH_QUERY_TIMEOUT.as_millis()
            )));
        }
        Ok(())
    }

    /// Task 15 rule 8: the byte budget at most half the live tail (so a
    /// backlog at the budget fits the tail and strong reads need no range
    /// tail), non-zero budgets, `override_factor >= 1` and
    /// `min_retry_after <= max_retry_after`.
    fn validate_backpressure(&self) -> Result<(), ServerError> {
        let b = &self.query.backpressure;
        let config = |message: String| Err(ServerError::Config(message));
        let half_tail = (self.query.tail.max_bytes / 2) as u64;
        if b.max_unapplied_bytes > half_tail {
            return config(format!(
                "--max-unapplied-bytes {} is over half the tail's bound ({half_tail} of {} bytes)",
                b.max_unapplied_bytes, self.query.tail.max_bytes
            ));
        }
        if b.max_unapplied_records == 0 || b.max_unapplied_bytes == 0 {
            return config("the unapplied-data budgets must be above 0".to_string());
        }
        if b.override_factor < 1 {
            return config("backpressure.override_factor must be at least 1".to_string());
        }
        if b.min_retry_after > b.max_retry_after {
            return config(format!(
                "backpressure.min_retry_after {:?} is over max_retry_after {:?}",
                b.min_retry_after, b.max_retry_after
            ));
        }
        Ok(())
    }
}

/// Runs the startup gate (design §54 §4.3, UP1 Task 4) before any listener
/// of the node opens: a refused start is [`ServerError::Startup`], and an
/// empty store is stamped at this release.
async fn startup_gate(meta: &Arc<dyn MetaStore>, data_dir: &Path) -> Result<(), ServerError> {
    use loams_migrate::startup::{self, StartupVerdict};
    let release = env!("CARGO_PKG_VERSION");
    match startup::check_startup(meta.clone(), data_dir, release).await? {
        StartupVerdict::Ready => Ok(()),
        StartupVerdict::NewStack => {
            let holder = format!("loams-{}-{}", std::process::id(), Ulid::generate());
            startup::stamp_new_stack(meta.clone(), release, &holder, None).await?;
            tracing::info!(release, "new stack: stamped the stack record");
            Ok(())
        }
        StartupVerdict::Refuse { code, detail } => Err(ServerError::Startup { code, detail }),
    }
}

/// Why the server could not start.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    /// The configuration is inconsistent (see [`ServerConfig::validate`]).
    #[error("invalid configuration: {0}")]
    Config(String),
    #[error("data directory {path}: {source}")]
    DataDir {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("object store: {0}")]
    Store(#[from] loams_store::StoreError),
    #[error("metastore: {0}")]
    Meta(#[from] loams_meta::MetaError),
    /// The startup gate refused the data (design §54 §4.3, UP1 Task 4):
    /// `format_unreadable`, `migration_incomplete` or `stack_unrecognised`.
    /// The process exits 78.
    #[error("{code}: {detail}")]
    Startup { code: &'static str, detail: String },
    /// The startup gate could not read the ledger.
    #[error("the upgrade ledger: {0}")]
    Ledger(#[from] loams_migrate::ledger::Error),
    /// The TiKV metastore's GC loop could not start (R1 plan Task 6).
    #[cfg(feature = "tikv")]
    #[error("TiKV: {0}")]
    Tikv(#[from] loams_tikv::TikvError),
    /// Loams Graph has no data directory and graphs are not explicitly ephemeral (GR1 Task 5).
    #[cfg(feature = "graph")]
    #[error(
        "Loams Graph needs a graph data directory (--graph-data-dir): a served graph must be \
         persistent. Pass --graph-ephemeral to keep graphs in memory (development only), or \
         --no-graph"
    )]
    GraphNeedsDataDir,
    /// Graph RPCs on a non-loopback address without authentication (D750): every caller would
    /// be the operator.
    #[cfg(feature = "graph")]
    #[error(
        "--listen {addr} is not a loopback address, and authentication is off, so every Loams \
         Graph caller would be the operator (D750); turn authentication on, listen on a loopback \
         address, or pass --no-graph"
    )]
    GraphListenNotLoopback { addr: SocketAddr },
    /// `--git-listen` is not a loopback address (GT1 Task 16, D111): the git listener's tokens
    /// are a development file until MT1.
    #[cfg(feature = "git")]
    #[error(
        "git listen on {addr}: only loopback addresses are served until the unified auth plan \
         (D111)"
    )]
    GitListenNotLoopback { addr: SocketAddr },
    /// The git listener could not be bound, or its token file read (GT1 Task 16).
    #[cfg(feature = "git")]
    #[error("the git listener on {addr}: {reason}")]
    GitListen { addr: SocketAddr, reason: String },
    /// `--live-listen` is not a loopback address (D111, design §20 §7.1):
    /// the Live API has no authentication in R1.
    #[cfg(feature = "live")]
    #[error(
        "--live-listen {addr} is not a loopback address; the Live API has no authentication \
         until the unified auth plan (D111)"
    )]
    LiveListenNotLoopback { addr: SocketAddr },
    /// Loams Live on a non-loopback `--listen` without TLS and
    /// authentication (LV1 plan Task 18, design §45 §6.1, MT1 Task 7).
    #[cfg(feature = "live")]
    #[error(
        "--listen listen on {addr}: non-loopback addresses need [tls] and [auth] (MT1): set \
         LOAMS_TLS_CERT_FILE and LOAMS_TLS_KEY_FILE (or LOAMS_TLS_TERMINATED_BY_PROXY=true with \
         LOAMS_TLS_TRUSTED_PROXIES) and LOAMS_AUTH_MODE, bind --listen to 127.0.0.1, or pass \
         --no-live"
    )]
    LiveListenNeedsTlsAndAuth { addr: SocketAddr },
    /// Loams Live could not start (R1 plan Task 12).
    #[cfg(feature = "live")]
    #[error("Loams Live: {0}")]
    Live(loams_live::LiveError),
    #[error("cache: {0}")]
    Cache(#[from] loams_cache::CacheError),
    #[error("log: {0}")]
    Log(#[from] loams_log::LogError),
    #[error("hot tier: {0}")]
    Hot(#[from] loams_hot::TierError),
    #[error("listen on {addr}: {source}")]
    Listen {
        addr: SocketAddr,
        source: std::io::Error,
    },
    #[cfg(feature = "mysql-wire")]
    #[error("MySQL wire listen on {addr}: {source}")]
    MysqlListen {
        addr: SocketAddr,
        source: std::io::Error,
    },
    #[cfg(feature = "pgwire")]
    #[error("postgres listen on {addr}: {source}")]
    PgListen {
        addr: SocketAddr,
        source: std::io::Error,
    },
    #[cfg(feature = "stream-grpc")]
    #[error("stream gRPC listen on {addr}: {source}")]
    StreamGrpcListen {
        addr: SocketAddr,
        source: std::io::Error,
    },
    /// The Loams SQL gate could not start (plan SQ1 Task 4).
    #[cfg(feature = "sqldb")]
    #[error("Loams SQL gate on {addr}: {source}")]
    SqlgateListen {
        addr: SocketAddr,
        source: std::io::Error,
    },
    /// A Qdrant gateway listener could not be bound (plan M1.4 Task 2).
    #[cfg(feature = "qdrant")]
    #[error("qdrant listen on {addr}: {source}")]
    QdrantListen {
        addr: SocketAddr,
        source: std::io::Error,
    },
    /// The Elasticsearch gateway's listener could not be bound (plan M1.5
    /// Task 1).
    #[cfg(feature = "es")]
    #[error("elasticsearch listen on {addr}: {source}")]
    EsListen {
        addr: SocketAddr,
        source: std::io::Error,
    },
    /// The embedded durable server did not start (D1); its message as is.
    #[cfg(feature = "durable")]
    #[error("{0}")]
    Durable(#[from] loams_durable::DurableError),
}

/// The embedded durable server (D1 Task 3) and Loams's durable runtime on it
/// (Task 6), when the build has the `durable` feature and the config asks
/// for it.
#[cfg(feature = "durable")]
#[derive(Debug)]
struct Durable {
    server: Option<loams_durable::DurableServer>,
    runtime: Option<loams_durable::DurableRuntime>,
}

/// The embedded durable server: none in a build without `durable`.
#[cfg(not(feature = "durable"))]
#[derive(Debug)]
struct Durable;

impl Durable {
    /// No durable server (a placeholder until the caller hands one over).
    #[cfg(feature = "durable")]
    fn none() -> Self {
        Self {
            server: None,
            runtime: None,
        }
    }

    #[cfg(not(feature = "durable"))]
    fn none() -> Self {
        Self
    }

    /// Starts `config.durable`, if any, for node `node_id`. Runs after the
    /// metastore has a leader and before [`Server::assemble`] (row T0-6).
    #[cfg(feature = "durable")]
    async fn start(config: &ServerConfig, node_id: u64) -> Result<Self, ServerError> {
        match &config.durable {
            Some(durable) => {
                let server = match loams_durable::DurableServer::start(
                    durable.clone(),
                    &node_id.to_string(),
                )
                .await
                {
                    Ok(server) => server,
                    // A store whose schema is behind is not migrated by
                    // serving (UP1 Task 6): the startup gate's refusal.
                    Err(err) if err.to_string().contains("database is at migration") => {
                        return Err(ServerError::Startup {
                            code: "migration_incomplete",
                            detail: format!(
                                "the durable store needs its migrations: {err}; run \
                                     `loams migrate run` (or `loams durable migrate`)"
                            ),
                        });
                    }
                    Err(err) => return Err(err.into()),
                };
                Ok(Self {
                    server: Some(server),
                    runtime: None,
                })
            }
            None => Ok(Self::none()),
        }
    }

    #[cfg(not(feature = "durable"))]
    async fn start(_config: &ServerConfig, _node_id: u64) -> Result<Self, ServerError> {
        Ok(Self)
    }

    /// Starts Loams's durable runtime on the server, if there is one: after
    /// [`Server::assemble`], before the routes serve (row T0-6, X8). A
    /// failure stops the server too.
    #[cfg(feature = "durable")]
    async fn start_runtime(&mut self, node_id: u64) -> Result<(), ServerError> {
        let Some(server) = &self.server else {
            return Ok(());
        };
        match loams_durable::DurableRuntime::start(server, &node_id.to_string()).await {
            Ok(runtime) => {
                self.runtime = Some(runtime);
                Ok(())
            }
            Err(err) => {
                if let Some(server) = self.server.take() {
                    server.stop().await;
                }
                Err(err.into())
            }
        }
    }

    #[cfg(not(feature = "durable"))]
    #[allow(clippy::unused_async)]
    async fn start_runtime(&mut self, _node_id: u64) -> Result<(), ServerError> {
        Ok(())
    }

    /// Stops the runtime (it takes no more tasks), then drains and stops the
    /// server; its port and store are free afterwards. Each is a shutdown
    /// phase: `durable_runtime`, then `durable`.
    async fn stop(self) {
        shutdown_phase("durable_runtime");
        #[cfg(feature = "durable")]
        if let Some(runtime) = self.runtime {
            runtime.stop().await;
        }
        shutdown_phase("durable");
        #[cfg(feature = "durable")]
        if let Some(server) = self.server {
            server.stop().await;
        }
    }

    /// The server's protocol endpoint, for `POST /durable/` on the main
    /// port (B12).
    #[cfg(feature = "durable")]
    fn endpoint(&self) -> Option<api::durable::DurableHandle> {
        self.server
            .as_ref()
            .map(|server| Arc::new(server.gateway()) as api::durable::DurableHandle)
    }

    #[cfg(not(feature = "durable"))]
    fn endpoint(&self) -> Option<api::durable::DurableHandle> {
        None
    }

    #[cfg(feature = "durable")]
    fn addr(&self) -> Option<SocketAddr> {
        self.server.as_ref().map(|server| server.listen())
    }

    #[cfg(not(feature = "durable"))]
    fn addr(&self) -> Option<SocketAddr> {
        None
    }
}

/// Marks one step of [`Server::shutdown`], in order (target
/// `loams::shutdown`, field `phase`), so tests can check the stop order.
fn shutdown_phase(phase: &'static str) {
    tracing::debug!(target: "loams::shutdown", phase, "stopping");
}

/// A running Loams process.
#[derive(Debug)]
pub struct Server {
    local_addr: SocketAddr,
    /// The openraft metastore node; `None` on the TiKV metastore.
    node: Option<MetaNode>,
    /// The openraft metastore client; `None` on the TiKV metastore.
    meta: Option<MetaClient>,
    /// The cluster MVCC GC loop the TiKV metastore runs.
    #[cfg(feature = "tikv")]
    tikv_gc: Option<TikvGc>,
    /// Loams Live, when configured.
    #[cfg(feature = "live")]
    live: Option<LiveRuntime>,
    /// Loams Graph, when served (GR1 Task 5).
    #[cfg(feature = "graph")]
    graph: Option<GraphRuntime>,
    /// Loams Git, when served (GT1 Task 13).
    #[cfg(feature = "git")]
    git: Option<GitRuntimeHandle>,
    /// Loams Meter's counter owners, when served (ME1 role wiring, part 2).
    #[cfg(feature = "meter")]
    meter: Option<api::meter::MeterRuntime>,
    /// The metastore as the trait object every component holds.
    meta_store: Arc<dyn MetaStore>,
    writer: LogWriter,
    cache: RangeCache,
    collection_context: CollectionContext,
    collection_factory: Arc<CollectionTargetFactory>,
    collections: Arc<CollectionService>,
    /// `None` on a cluster node without the `worker` role.
    worker: Option<WorkerHandle>,
    /// The hot tier, unless `--hot off` (or no `query` role).
    hot: Option<HotTierImpl>,
    http: JoinHandle<()>,
    stop_http: oneshot::Sender<()>,
    /// The `tailscale serve` target (`LOAMS_AUTH_MODE=tailscale`).
    tailscale: Option<TailscaleTarget>,
    /// The connect socket (`--connect-socket`).
    connect_socket: Option<ConnectSocket>,
    flight: Option<Flight>,
    #[cfg(feature = "mysql-wire")]
    mysql_wire: Option<crate::mysql_wire::MysqlHandle>,
    #[cfg(feature = "pgwire")]
    pg: Option<crate::pg::PgHandle>,
    #[cfg(feature = "stream-grpc")]
    stream_grpc: Option<StreamGrpc>,
    #[cfg(feature = "sqldb")]
    sqlgate: Option<crate::sqlgate::SqlgateHandle>,
    #[cfg(feature = "qdrant")]
    qdrant: Option<Qdrant>,
    /// The embedded durable server (D1).
    durable: Durable,
    #[cfg(feature = "es")]
    es: Option<Es>,
    /// Cluster mode only.
    cluster: Option<ClusterRuntime>,
}

/// The connect socket (CC1 ruling R1.1): the main API on a Unix socket for
/// `loams-fabric connect`, behind [`crate::auth::AuthLayerState::connect_socket`].
/// The socket file is mode 0660 and is removed again on shutdown.
#[derive(Debug)]
struct ConnectSocket {
    path: PathBuf,
    stop: CancellationToken,
    task: JoinHandle<()>,
}

/// The connect socket's file mode: its owner and group (the connect role's
/// container runs as the same uid), nobody else.
pub const CONNECT_SOCKET_MODE: u32 = 0o660;

/// The connect socket's listener: a Unix socket. Elsewhere there is none
/// (`bind` refuses), so the type is uninhabited.
#[cfg(unix)]
type ConnectListener = tokio::net::UnixListener;
#[cfg(not(unix))]
type ConnectListener = std::convert::Infallible;

impl ConnectSocket {
    /// Bind `path` (a stale socket there from an earlier run is replaced;
    /// anything else there is refused) and set its mode.
    #[cfg(unix)]
    fn bind(path: &Path) -> Result<ConnectListener, ServerError> {
        use std::os::unix::fs::{FileTypeExt as _, PermissionsExt as _};
        let fail = |what: &str, err: std::io::Error| {
            ServerError::Config(format!(
                "--connect-socket {}: {what}: {err}",
                path.display()
            ))
        };
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_socket() => {
                std::fs::remove_file(path).map_err(|e| fail("removing the stale socket", e))?;
            }
            Ok(_) => {
                return Err(ServerError::Config(format!(
                    "--connect-socket {}: something other than a socket is there",
                    path.display()
                )));
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(fail("reading it", err)),
        }
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(|e| fail("creating its directory", e))?;
        }
        let listener = tokio::net::UnixListener::bind(path).map_err(|e| fail("binding", e))?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(CONNECT_SOCKET_MODE))
            .map_err(|e| fail("setting its mode", e))?;
        Ok(listener)
    }

    #[cfg(not(unix))]
    fn bind(path: &Path) -> Result<ConnectListener, ServerError> {
        Err(ServerError::Config(format!(
            "--connect-socket {}: Unix sockets need a Unix host",
            path.display()
        )))
    }

    #[cfg(not(unix))]
    fn start(listener: ConnectListener, _path: PathBuf, _app: axum::Router) -> Self {
        match listener {}
    }

    #[cfg(unix)]
    fn start(listener: ConnectListener, path: PathBuf, app: axum::Router) -> Self {
        let stop = CancellationToken::new();
        let stopped = stop.clone();
        let task = tokio::spawn(async move {
            let serve =
                axum::serve(listener, app).with_graceful_shutdown(stopped.cancelled_owned());
            if let Err(err) = serve.await {
                tracing::error!(%err, "the connect socket failed");
            }
        });
        Self { path, stop, task }
    }

    async fn stop(self) {
        self.stop.cancel();
        let mut task = self.task;
        if tokio::time::timeout(HTTP_GRACE, &mut task).await.is_err() {
            task.abort();
        }
        if let Err(err) = std::fs::remove_file(&self.path)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%err, path = %self.path.display(), "removing the connect socket");
        }
    }
}

/// The `tailscale serve` target: the main API on `LOAMS_TAILSCALE_LISTEN`,
/// the one listener where Tailscale identity headers name the caller.
#[derive(Debug)]
struct TailscaleTarget {
    addr: SocketAddr,
    stop: CancellationToken,
    task: JoinHandle<()>,
}

impl TailscaleTarget {
    fn start(listener: tokio::net::TcpListener, addr: SocketAddr, app: axum::Router) -> Self {
        let stop = CancellationToken::new();
        let stopped = stop.clone();
        let task = tokio::spawn(async move {
            let serve =
                axum::serve(listener, app).with_graceful_shutdown(stopped.cancelled_owned());
            if let Err(err) = serve.await {
                tracing::error!(%err, "the tailscale serve target failed");
            }
        });
        Self { addr, stop, task }
    }

    async fn stop(self) {
        self.stop.cancel();
        let mut task = self.task;
        if tokio::time::timeout(HTTP_GRACE, &mut task).await.is_err() {
            task.abort();
        }
    }
}

/// The running cluster MVCC GC loop of a server on the TiKV metastore (R1
/// plan Task 3).
#[cfg(feature = "tikv")]
#[derive(Debug)]
struct TikvGc {
    handle: loams_tikv::GcHandle,
    stop: CancellationToken,
}

#[cfg(feature = "tikv")]
impl TikvGc {
    /// Starts the GC loop on the metastore's handle. The loop sweeps its own
    /// handle's commit tokens and those of `sweep`: Loams Live's handle when
    /// Live runs in the process on the same cluster (R1 plan Task 12).
    fn start(meta: &loams_meta_tikv::TikvMeta, sweep: Sweep) -> Result<Self, ServerError> {
        Self::spawn(meta.tikv().clone(), sweep)
    }

    /// Starts the GC loop on `tikv` (its lease lives under that handle's
    /// root), sweeping `sweep` too.
    fn spawn(tikv: loams_tikv::Tikv, sweep: Sweep) -> Result<Self, ServerError> {
        let stop = CancellationToken::new();
        let handle = loams_tikv::GcLoop::spawn(
            tikv,
            loams_tikv::GcConfig {
                sweep,
                ..loams_tikv::GcConfig::default()
            },
            stop.clone(),
        )?;
        Ok(TikvGc { handle, stop })
    }

    async fn stop(self) {
        self.stop.cancel();
        self.handle.stopped().await;
    }
}

/// Further TiKV handles the metastore's GC loop sweeps (Loams Live's).
#[cfg(feature = "tikv")]
type Sweep = Vec<loams_tikv::Tikv>;
/// Without TiKV there is nothing to sweep.
#[cfg(not(feature = "tikv"))]
#[derive(Debug)]
struct Sweep;

/// Nothing beside the metastore's own handle to sweep.
#[cfg(not(feature = "live-tikv"))]
fn no_sweep() -> Sweep {
    #[cfg(feature = "tikv")]
    let sweep = Vec::new();
    #[cfg(not(feature = "tikv"))]
    let sweep = Sweep;
    sweep
}

/// A running Loams Live server and, when no metastore GC loop covers its
/// cluster, its own cluster GC loop (R1 plan Task 12).
#[cfg(feature = "live")]
#[derive(Debug)]
struct LiveRuntime {
    handle: loams_live::LiveHandle,
    /// Live's own cluster GC loop (a TiKV store not covered by the
    /// metastore's); the embedded store collects its own versions.
    #[cfg(feature = "live-tikv")]
    gc: Option<TikvGc>,
}

#[cfg(feature = "live")]
impl LiveRuntime {
    /// Starts Live with its function runtime: deployments' bundles in
    /// `bundles` (the data bucket, R1 plan Ruling 10) and run by
    /// `loams-live-js`, in process or, with `isolation = "isolated"`, in
    /// `loams live-worker` processes of this binary (LV1 plan Task 6).
    /// With `own_gc`, a cluster GC loop on Live's handle too (the openraft
    /// metastore, or a TiKV metastore on another cluster). The listener
    /// sits behind `auth`, the engine's authentication layer (LV1 plan Task
    /// 10), which names the caller for the admin service.
    #[cfg_attr(not(feature = "live-tikv"), allow(unused_variables))]
    async fn start(
        mut config: loams_live::LiveConfig,
        bundles: Store,
        own_gc: bool,
        auth: Arc<crate::auth::Auth>,
    ) -> Result<Self, ServerError> {
        // A node that authenticates its callers forwards session calls only
        // to nodes that do too (LV1 plan Task 24).
        config.forward.authenticated = auth.enforced();
        let loader = loams_live_js::JsLoader::new(loams_live_js::JsConfig {
            isolation: config.isolation,
            ..loams_live_js::JsConfig::default()
        })
        .map_err(ServerError::Live)?;
        let runtime = loams_live::deploy::Runtime {
            bundles,
            loader: Arc::new(loader),
        };
        let layer = crate::auth::AuthLayerState::new(auth, &[])
            .leaving_to_apps()
            .with_app_origins(app_origin_set(&config));
        let guard: loams_live::service::Guard = Arc::new(move |router: axum::Router| {
            layer.wrap(router.layer(axum::middleware::from_fn(crate::api::live::caller)))
        });
        let handle = loams_live::LiveServer::start_guarded(
            config,
            runtime,
            Some(guard),
            CancellationToken::new(),
        )
        .await
        .map_err(|err| match err {
            loams_live::LiveError::NotLoopback(addr) => ServerError::LiveListenNotLoopback { addr },
            err => ServerError::Live(err),
        })?;
        // Only a TiKV store needs the cluster GC loop.
        #[cfg(feature = "live-tikv")]
        let gc = match handle.store().as_tikv().cloned() {
            Some(tikv) if own_gc => match TikvGc::spawn(tikv, Vec::new()) {
                Ok(gc) => Some(gc),
                Err(err) => {
                    handle.stop().await;
                    return Err(err);
                }
            },
            _ => None,
        };
        Ok(LiveRuntime {
            handle,
            #[cfg(feature = "live-tikv")]
            gc,
        })
    }

    async fn stop(self) {
        self.handle.stop().await;
        #[cfg(feature = "live-tikv")]
        if let Some(gc) = self.gc {
            gc.stop().await;
        }
    }
}

/// The browser origins Live's active apps list, for the engine's CORS
/// layer (LV1 plan Task 18): the handle `live`'s app engines keep current.
#[cfg(feature = "live")]
fn app_origin_set(live: &loams_live::LiveConfig) -> crate::auth::layer::AppOriginSet {
    let origins = live.app_origins.clone();
    crate::auth::layer::AppOriginSet::new(move |origin| origins.contains(origin))
}

/// Whether the main port may serve Loams Live (LV1 plan Task 18, design
/// §45 §6.1): on loopback, or behind TLS (here or at a trusted proxy) and
/// authentication. [`ServerConfig::validate`] refuses to start Live on any
/// other `--listen`, so this only guards the router.
#[cfg(feature = "live")]
fn main_port_serves_live(config: &ServerConfig) -> bool {
    config.listen.ip().to_canonical().is_loopback()
        || (config.tls.secures() && config.auth.enforced())
}

/// Whether the main port may serve the durable API under `/durable/` (B12):
/// as for Loams Live ([`main_port_serves_live`]), on loopback, or behind TLS
/// and authentication. The durable listener itself is loopback only (D111).
fn main_port_serves_durable(config: &ServerConfig) -> bool {
    let serves = config.cluster.is_none()
        && (config.listen.ip().to_canonical().is_loopback()
            || (config.tls.secures() && config.auth.enforced()));
    #[cfg(feature = "durable")]
    if !serves && config.durable.is_some() {
        tracing::warn!(
            listen = %config.listen,
            "the durable API is not served under /durable/ on --listen {}: beyond loopback it \
             needs TLS and authentication",
            config.listen
        );
    }
    serves
}

/// The main port's state without Loams Live when it may not serve it
/// ([`main_port_serves_live`]; `None`: serve `state` as it is). The
/// `tailscale serve` target (loopback, with the tailnet's TLS) serves it
/// regardless.
#[cfg(feature = "live")]
fn without_live_beyond_loopback(state: &AppState, config: &ServerConfig) -> Option<AppState> {
    if state.live.is_none() || main_port_serves_live(config) {
        return None;
    }
    tracing::warn!(
        listen = %config.listen,
        "Loams Live is not served on --listen {}: beyond loopback it needs TLS and \
         authentication (LV1 Task 18)",
        config.listen
    );
    Some(AppState {
        live: None,
        ..state.clone()
    })
}

/// Whether two PD endpoint lists name one cluster (the same endpoints, in
/// any order, with or without `http://`).
#[cfg(feature = "live-tikv")]
fn same_cluster(a: &[String], b: &[String]) -> bool {
    let norm = |pd: &[String]| {
        pd.iter()
            .map(|p| {
                p.trim_start_matches("http://")
                    .trim_end_matches('/')
                    .to_string()
            })
            .collect::<std::collections::BTreeSet<_>>()
    };
    norm(a) == norm(b)
}

/// What a cluster node keeps for its shutdown.
#[derive(Debug)]
struct ClusterRuntime {
    node_id: u64,
    roles: Roles,
    registry: Arc<NodeRegistry>,
    transport: HttpTransport,
    seeds: Vec<String>,
    late: LateRouter,
}

/// How [`Server::assemble`] wires a node: single-node modes pass every role,
/// `AlwaysLocal` and no extras.
struct NodeSetup {
    node_id: u64,
    roles: Roles,
    meta_store: Arc<dyn MetaStore>,
    placement: Arc<dyn Placement>,
    /// The routed service's placement and transport (cluster mode).
    routing: Option<(Arc<PlacementImpl>, Arc<RemoteReadsImpl>)>,
    forward_stats: Arc<ForwardStats>,
    extra_sources: Vec<Arc<dyn TaskSource>>,
    node_info: Option<Arc<dyn NodeInfo>>,
    /// What the start built before the node (Loams Live, and the
    /// authentication its listener shares with the node's surfaces).
    pre: Prebuilt,
    /// The embedded durable server's endpoint for `POST /durable/` on the
    /// main port (B12); `None` on a cluster node, whose main port is not
    /// guarded.
    durable: Option<api::durable::DurableHandle>,
}

/// What `start_with_live` builds before the node: the authentication (LV1
/// plan Task 10), so Live's listener and the node's surfaces share it, built
/// from the config when `None`; and Live's services for the main port (LV1
/// plan Task 12).
#[derive(Debug, Default)]
struct Prebuilt {
    auth: Option<Arc<crate::auth::Auth>>,
    #[cfg(feature = "live")]
    live: Option<loams_live::LiveRoutes>,
}

/// Everything [`Server::assemble`] started, and the node's router.
struct Assembled {
    writer: LogWriter,
    cache: RangeCache,
    collection_context: CollectionContext,
    collection_factory: Arc<CollectionTargetFactory>,
    collections: Arc<CollectionService>,
    worker: Option<WorkerHandle>,
    hot: Option<HotTierImpl>,
    app: axum::Router,
    tailscale: Option<TailscaleTarget>,
    connect_socket: Option<ConnectSocket>,
    flight: Option<Flight>,
    #[cfg(feature = "mysql-wire")]
    mysql_wire: Option<crate::mysql_wire::MysqlHandle>,
    #[cfg(feature = "pgwire")]
    pg: Option<crate::pg::PgHandle>,
    #[cfg(feature = "stream-grpc")]
    stream_grpc: Option<StreamGrpc>,
    #[cfg(feature = "sqldb")]
    sqlgate: Option<crate::sqlgate::SqlgateHandle>,
    #[cfg(feature = "qdrant")]
    qdrant: Option<Qdrant>,
    /// Started by the caller before `assemble` and handed over here (a
    /// cluster node's start returns it with the parts).
    durable: Durable,
    #[cfg(feature = "es")]
    es: Option<Es>,
    #[cfg(feature = "graph")]
    graph: Option<GraphRuntime>,
    #[cfg(feature = "git")]
    git: Option<GitRuntimeHandle>,
    #[cfg(feature = "meter")]
    meter_runtime: Option<api::meter::MeterRuntime>,
}

/// Loams Graph as the server runs it (GR1 Task 5): the admin service (catalog and engine) and
/// the maintenance task that purges deleted graphs, sweeps catalog documents and snapshot
/// objects, takes the snapshots that are due (GR1 Task 13) and evicts idle graphs (GR1 Task 14).
#[cfg(feature = "graph")]
#[derive(Debug)]
struct GraphRuntime {
    admin: Arc<loams_graph::service::admin::GraphAdmin>,
    /// The graphs' own log writer (GR1 Task 11): it flushes every 2 ms or 256 KiB, the write
    /// lane's group commit, where the collection writer waits 250 ms.
    log: LogWriter,
    data_dir: Option<PathBuf>,
    stop: CancellationToken,
    maintenance: JoinHandle<()>,
    /// The task applying the LINKED graphs' links this node serves (GR1 Task 18).
    links: (loams_graph::link::LinkRunner, JoinHandle<()>),
    shutdown_wait: Duration,
    /// Catalog sweeps the maintenance task has completed.
    sweeps: Arc<std::sync::atomic::AtomicU64>,
    /// Graphs the maintenance task has checkpointed (GR1 Task 13).
    checkpoints: Arc<std::sync::atomic::AtomicU64>,
    /// Graphs the maintenance task has evicted (GR1 Task 14).
    evictions: Arc<std::sync::atomic::AtomicU64>,
}

#[cfg(feature = "graph")]
impl GraphRuntime {
    /// Starts the graph service: every graph is durable in Loams' log (GR1 Task 11), through a
    /// writer of its own on log node `log_node` and the server's `reader`.
    fn start(
        config: &GraphConfig,
        meta: Arc<dyn MetaStore>,
        store: Store,
        reader: LogReader,
        log_node: u64,
        registry: Option<Arc<NodeRegistry>>,
        link: LinkConfig,
    ) -> Result<Self, ServerError> {
        use loams_graph::Engine;
        use loams_graph::catalog::GraphCatalog;
        use loams_graph::durable::GraphLog;
        use loams_graph::service::admin::GraphAdmin;
        let log = LogWriter::start(meta.clone(), store.clone(), GraphLog::log_config(log_node))?;
        let engine = match (&config.data_dir, config.ephemeral) {
            (Some(dir), _) => Engine::with_data_dir(dir),
            (None, _) => Engine::new(),
        }
        .with_query_timeout(Some(config.query_timeout))
        .with_max_property_bytes(config.limits.max_property_bytes);
        let graph_log = GraphLog::new(
            meta.clone(),
            log.clone(),
            reader,
            tokio::runtime::Handle::current(),
        );
        let mut admin = GraphAdmin::new(Arc::new(engine), GraphCatalog::new(meta.clone(), store))
            .with_log(graph_log)
            .with_retention_hold(config.retention_hold)
            .with_statement_slots(config.statement_slots)
            .with_namespace_statements(config.namespace_statements)
            .with_limits(config.limits)
            .with_checkpoint_policy(config.checkpoint)
            .with_evict_policy(config.evict)
            .with_consistency_wait(config.consistency_wait);
        // A cluster's graph nodes own graphs by lease and forward to each other (GR1 Task 12);
        // a single node owns every graph and still fences its appends.
        if let Some(registry) = registry {
            let forwarder = crate::api::graph::HttpForwarder::new().map_err(|err| {
                ServerError::Config(format!("the graph forwarder's HTTP client: {err}"))
            })?;
            admin = admin.with_ownership(loams_graph::owner::Ownership::new(
                meta,
                Arc::new(crate::api::graph::RegistryMembership::new(registry)),
                Some(Arc::new(forwarder)),
                loams_graph::owner::OwnerConfig::default(),
            ));
        }
        let admin = Arc::new(admin);
        let stop = CancellationToken::new();
        let sweeps = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let checkpoints = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let evictions = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let maintenance = {
            let (admin, stop, sweeps, checkpoints, evictions) = (
                admin.clone(),
                stop.clone(),
                sweeps.clone(),
                checkpoints.clone(),
                evictions.clone(),
            );
            let (every, hold, grace, checkpoint_every) = (
                config.maintenance_every,
                config.retention_hold,
                config.sweep_grace,
                config.checkpoint_every,
            );
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(every);
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                let mut checkpoint = tokio::time::interval(checkpoint_every);
                checkpoint.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    tokio::select! {
                        () = stop.cancelled() => return,
                        _ = tick.tick() => {}
                        _ = checkpoint.tick() => {
                            // Snapshots of the graphs this node owns that are due (GR1 Task 13).
                            let done = admin.checkpoint_due().await;
                            if done > 0 {
                                tracing::debug!(graphs = done, "checkpointed graphs");
                                checkpoints.fetch_add(
                                    done as u64,
                                    std::sync::atomic::Ordering::Relaxed,
                                );
                            }
                            // Then idle graphs, and deleted graphs' caches (GR1 Task 14).
                            let evicted = admin.evict_due().await;
                            if !evicted.is_empty() {
                                tracing::debug!(graphs = evicted.len(), "evicted graphs");
                                evictions.fetch_add(
                                    evicted.len() as u64,
                                    std::sync::atomic::Ordering::Relaxed,
                                );
                            }
                            continue;
                        }
                    }
                    // This node's caches, then (on one node of the cluster at a time, under the
                    // maintenance lease) the purge and the catalog sweep (GR1 Task 12).
                    match admin.maintain(hold, grace).await {
                        Ok(purged) => {
                            if purged > 0 {
                                tracing::info!(purged, "purged deleted graphs");
                            }
                            sweeps.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                        Err(err) => tracing::warn!(%err, "graph maintenance failed"),
                    }
                }
            })
        };
        // The links of the LINKED graphs this node owns (GR1 Task 18): every pass applies each
        // link whose collection has records the graph has not.
        let links = {
            let runner = loams_graph::link::LinkRunner::new((*admin).clone(), link);
            let (task_runner, stop) = (runner.clone(), stop.clone());
            let task = tokio::spawn(async move {
                let mut tick = tokio::time::interval(loams_graph::link::LinkRunner::EVERY);
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    tokio::select! {
                        () = stop.cancelled() => return,
                        _ = tick.tick() => {}
                    }
                    let committed = task_runner.run_once().await;
                    if committed > 0 {
                        tracing::debug!(links = committed, "applied graph links");
                    }
                }
            });
            (runner, task)
        };
        Ok(Self {
            admin,
            log,
            data_dir: config.data_dir.clone(),
            stop,
            maintenance,
            links,
            shutdown_wait: config.shutdown_wait,
            sweeps,
            checkpoints,
            evictions,
        })
    }

    /// Stops the maintenance task, then the admin (review fix 1, I3): new requests answer
    /// `UNAVAILABLE`, running statements get `shutdown_wait` to end, and every graph is closed.
    /// Each wait is bounded; a statement still running after it is detached (see
    /// [`GraphConfig::shutdown_wait`]).
    async fn stop(self) {
        self.stop.cancel();
        let (runner, mut links) = self.links;
        runner.stop();
        if tokio::time::timeout(self.shutdown_wait, &mut links)
            .await
            .is_err()
        {
            tracing::warn!("a graph link pass outlasted the shutdown wait; abandoned");
            links.abort();
        }
        let mut maintenance = self.maintenance;
        if tokio::time::timeout(self.shutdown_wait, &mut maintenance)
            .await
            .is_err()
        {
            tracing::warn!("a graph maintenance pass outlasted the shutdown wait; abandoned");
            maintenance.abort();
        }
        self.admin.shutdown(self.shutdown_wait).await;
        // After the graphs: a write still in its lane is answered before its log goes.
        if let Err(err) = self.log.shutdown().await {
            tracing::warn!(%err, "stopping the graph log writer");
        }
    }
}

/// Loams Git as the server runs it (GT1 Task 13): the catalog and sequencers `RepoService`
/// shares, and the task that purges deleted repositories.
#[cfg(feature = "git")]
#[derive(Debug)]
struct GitRuntimeHandle {
    runtime: Arc<crate::api::repos::GitRuntime>,
    stop: CancellationToken,
    purge: JoinHandle<()>,
    /// The git listener (GT1 Task 16): its address and its serving task.
    listener: Option<(SocketAddr, JoinHandle<()>)>,
    /// The `_git` repair loop (GT1 Task 15).
    repair: (CancellationToken, JoinHandle<()>),
}

#[cfg(feature = "git")]
impl GitRuntimeHandle {
    /// `cluster` places each repository's sequencer on one gateway node under its lease and
    /// forwards commits there (GT1 Task 14); without it (dev, standalone) every sequencer runs
    /// here.
    fn start(
        config: &GitConfig,
        meta: Arc<dyn MetaStore>,
        store: Store,
        h1: RangeCache,
        sink: Arc<api::git_events::StreamSink>,
        cluster: Option<loams_git::Cluster>,
        listener: Option<(
            tokio::net::TcpListener,
            Arc<api::git_auth::LoopbackDevAuthorizer>,
        )>,
    ) -> Self {
        let catalog = loams_git::BucketRepoCatalog::new(store.clone());
        let meta_for_listener = meta.clone();
        let clock: Arc<dyn loams_git::Clock> = Arc::new(loams_git::SystemClock);
        let sequencers = match cluster {
            Some(cluster) => loams_git::RepoSequencers::clustered(store.clone(), clock, cluster),
            None => loams_git::RepoSequencers::new(store.clone(), clock),
        }
        .with_mirror(sink.clone());
        // The `_git` repair (GT1 Task 15): every `mirror_repair_every`, what a crash left
        // unmirrored is published; the stream's dedup ledger drops any repeat.
        let repair = {
            let (meta, store, every) = (meta.clone(), store.clone(), config.mirror_repair_every);
            let stop = CancellationToken::new();
            let task_stop = stop.clone();
            let task = tokio::spawn(async move {
                let mut tick = tokio::time::interval(every);
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    tokio::select! {
                        () = task_stop.cancelled() => return,
                        _ = tick.tick() => {}
                    }
                    api::git_events::repair_all(meta.as_ref(), &store, sink.as_ref()).await;
                }
            });
            (stop, task)
        };
        let runtime = Arc::new(crate::api::repos::GitRuntime {
            catalog: catalog.clone(),
            sequencers: Arc::new(sequencers),
            watch_heartbeat: config.watch_heartbeat,
            store: store.clone(),
            h1,
            // The workspace store (GT1 Task 25, ruling T18.1).
            workspaces: Arc::new(loams_git::ws::StoreWsLookup::new(store.clone())),
            ws_saved: tokio::sync::broadcast::channel(256).0,
            ws_reads_owner_only: config.workspace_reads_owner_only,
        });
        let stop = CancellationToken::new();
        let purge = {
            let (stop, every, grace) = (stop.clone(), config.purge_every, config.purge_grace);
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(every);
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    tokio::select! {
                        () = stop.cancelled() => return,
                        _ = tick.tick() => {}
                    }
                    purge_deleted_repos(meta.as_ref(), &catalog, grace).await;
                }
            })
        };
        let listener = listener.map(|(listener, authorizer)| {
            let addr = listener.local_addr().unwrap_or_else(|_| {
                config
                    .listen
                    .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 0)))
            });
            let state = api::git_auth::GitHttp {
                authorizer,
                meta: meta_for_listener,
                runtime: runtime.clone(),
            };
            let task = api::git_auth::serve_git_listener(listener, state, stop.clone());
            tracing::info!(%addr, "serving the git listener");
            (addr, task)
        });
        Self {
            runtime,
            stop,
            purge,
            listener,
            repair,
        }
    }

    /// The git listener's address, when it is served.
    fn listen_addr(&self) -> Option<SocketAddr> {
        self.listener.as_ref().map(|(addr, _)| *addr)
    }

    /// Stops the purge (abandoning a pass that outlasts `PURGE_STOP_WAIT`), then every
    /// sequencer.
    async fn stop(self) {
        const PURGE_STOP_WAIT: Duration = Duration::from_secs(5);
        self.stop.cancel();
        self.repair.0.cancel();
        self.repair.1.abort();
        let mut purge = self.purge;
        if tokio::time::timeout(PURGE_STOP_WAIT, &mut purge)
            .await
            .is_err()
        {
            tracing::warn!("a repository purge pass outlasted the shutdown wait; abandoned");
            purge.abort();
        }
        if let Some((_, listener)) = self.listener
            && tokio::time::timeout(PURGE_STOP_WAIT, listener)
                .await
                .is_err()
        {
            tracing::warn!("the git listener outlasted the shutdown wait; abandoned");
        }
        self.runtime.sequencers.shutdown().await;
    }
}

/// One pass of the purge of deleted repositories over every namespace (GT1 Task 13); a
/// failure is logged and the next pass tries again.
#[cfg(feature = "git")]
async fn purge_deleted_repos(
    meta: &dyn MetaStore,
    catalog: &loams_git::BucketRepoCatalog,
    grace: Duration,
) {
    // A kept upgrade snapshot may restore a repository deleted since: purge
    // nothing until it is pruned (UP1 Task 8).
    match loams_common::meta::snapshot_gc_floor(meta).await {
        Ok(None) => {}
        Ok(Some(_)) => {
            tracing::debug!("an upgrade snapshot is kept: the repository purge waits");
            return;
        }
        Err(err) => {
            tracing::warn!(%err, "reading the upgrade snapshots for the repository purge failed");
            return;
        }
    }
    let namespaces = match meta
        .namespaces(loams_common::meta::Consistency::Local)
        .await
    {
        Ok(namespaces) => namespaces,
        Err(err) => {
            tracing::warn!(%err, "listing namespaces for the repository purge failed");
            return;
        }
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    for ns in namespaces {
        match catalog.purge_deleted(ns.id, now, grace).await {
            Ok(report) if report.purged > 0 => {
                tracing::info!(ns = %ns.id, purged = report.purged, "purged deleted repositories");
            }
            Ok(_) => {}
            Err(err) => tracing::warn!(ns = %ns.id, %err, "purging deleted repositories failed"),
        }
    }
}

/// The running Qdrant gateway (plan M1.4 Task 2).
#[cfg(feature = "qdrant")]
#[derive(Debug)]
struct Qdrant {
    handle: loams_qdrant::QdrantHandle,
    stop: CancellationToken,
}

/// The running Elasticsearch gateway (plan M1.5 Task 1).
#[cfg(feature = "es")]
#[derive(Debug)]
struct Es {
    handle: loams_es::EsHandle,
    stop: CancellationToken,
}

/// Binds the Qdrant gateway's REST and gRPC listeners.
#[cfg(feature = "qdrant")]
async fn bind_qdrant(
    config: &loams_qdrant::QdrantConfig,
) -> Result<(tokio::net::TcpListener, tokio::net::TcpListener), ServerError> {
    let bind_one = async |addr: SocketAddr| {
        tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|source| ServerError::QdrantListen { addr, source })
    };
    let rest = bind_one(config.rest_listen).await?;
    let grpc = bind_one(config.grpc_listen).await?;
    Ok((rest, grpc))
}

/// The running Flight SQL server.
#[derive(Debug)]
struct Flight {
    addr: SocketAddr,
    task: JoinHandle<()>,
    stop: CancellationToken,
    /// The `DoPut` tasks, which outlive an aborted `task`.
    puts: PutTasks,
}

#[cfg(feature = "stream-grpc")]
#[derive(Debug)]
struct StreamGrpc {
    addr: SocketAddr,
    task: JoinHandle<()>,
    stop: CancellationToken,
}

#[cfg(feature = "stream-grpc")]
impl StreamGrpc {
    fn start(
        listener: tokio::net::TcpListener,
        addr: SocketAddr,
        streams: Arc<dyn StreamProducer>,
        events: Arc<dyn loams_stream_grpc::EventProducer>,
    ) -> Self {
        let stop = CancellationToken::new();
        let task_stop = stop.clone();
        let task = tokio::spawn(async move {
            if let Err(err) = loams_stream_grpc::serve(listener, streams, events, task_stop).await {
                tracing::error!(%err, "native stream gRPC server failed");
            }
        });
        Self { addr, task, stop }
    }

    async fn stop(self) {
        self.stop.cancel();
        let mut task = self.task;
        if tokio::time::timeout(HTTP_GRACE, &mut task).await.is_err() {
            tracing::warn!("in-flight stream gRPC calls did not finish; aborting them");
            task.abort();
        }
    }
}

impl Flight {
    /// Serves Flight SQL over `collections` on `listener` until stopped
    /// (Task 12), with stream ingest through `streams` (Task 13).
    fn start(
        listener: tokio::net::TcpListener,
        addr: SocketAddr,
        collections: Arc<CollectionService>,
        streams: Arc<dyn StreamProducer>,
        config: FlightConfig,
        guard: crate::auth::AuthLayerState,
        graph_callers: loams_query::flight::GraphCallers,
    ) -> Self {
        let stop = CancellationToken::new();
        let puts = PutTasks::new();
        let task = tokio::spawn(serve(
            listener,
            collections,
            streams,
            config,
            stop.clone(),
            puts.clone(),
            guard,
            graph_callers,
        ));
        Self {
            addr,
            task,
            stop,
            puts,
        }
    }

    /// Stops accepting calls, waits up to [`HTTP_GRACE`] for the calls in
    /// flight (a `DoGet` may stream for minutes) and aborts the rest, then
    /// waits up to [`HTTP_GRACE`] for the puts, which stop before their next
    /// chunk, and aborts the rest, so none writes after the collection
    /// service and the writer stop.
    async fn stop(self) {
        self.stop.cancel();
        let mut task = self.task;
        if tokio::time::timeout(HTTP_GRACE, &mut task).await.is_err() {
            tracing::warn!("in-flight Flight SQL calls did not finish; aborting them");
            task.abort();
        }
        self.puts.close();
        if tokio::time::timeout(HTTP_GRACE, self.puts.wait())
            .await
            .is_err()
        {
            tracing::warn!("a Flight put did not finish its chunk in flight; aborting it");
            self.puts.abort();
            self.puts.wait().await;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn serve(
    listener: tokio::net::TcpListener,
    collections: Arc<CollectionService>,
    streams: Arc<dyn StreamProducer>,
    config: FlightConfig,
    stop: CancellationToken,
    puts: PutTasks,
    guard: crate::auth::AuthLayerState,
    graph_callers: loams_query::flight::GraphCallers,
) {
    let served = serve_flight_sql_wrapped(
        listener,
        collections,
        Some(streams),
        config,
        stop,
        puts,
        graph_callers,
        |routes| guard.wrap_grpc(routes),
    )
    .await;
    if let Err(err) = served {
        tracing::error!(%err, "Flight SQL server failed");
    }
}

/// Where Flight SQL listens: `config.flight_sql` with the `flight` feature,
/// never without it.
fn flight_addr(config: &ServerConfig) -> Option<SocketAddr> {
    if cfg!(feature = "flight") {
        config.flight_sql
    } else {
        if config.flight_sql.is_some() {
            tracing::warn!("this build has no Flight SQL (the flight feature is off)");
        }
        None
    }
}

/// The object store URL for a config: its bucket, or a directory in the data
/// directory.
fn bucket_url(config: &ServerConfig) -> Result<String, ServerError> {
    if let Some(bucket) = &config.bucket {
        return Ok(bucket.clone());
    }
    let dir = config.data_dir.join("bucket");
    let data_dir_error = |source| ServerError::DataDir {
        path: dir.clone(),
        source,
    };
    std::fs::create_dir_all(&dir).map_err(data_dir_error)?;
    let dir = dir.canonicalize().map_err(data_dir_error)?;
    url::Url::from_directory_path(&dir)
        .map(String::from)
        .map_err(|()| ServerError::DataDir {
            path: dir,
            source: std::io::Error::other("not an absolute path"),
        })
}

impl Server {
    /// Opens (or creates) the data directory, starts the metastore, the log
    /// and its background loops, and serves the HTTP API; with
    /// `config.cluster`, starts one cluster node instead (plan M1.3 Task 11
    /// rule 3).
    ///
    /// On failure, everything already started is stopped again (the
    /// metastore releases its local database), so a retry in the same process
    /// can succeed.
    pub async fn start(config: ServerConfig) -> Result<Self, ServerError> {
        config.validate()?;
        config.log.validate()?;
        if config.cluster.is_some() {
            return Self::start_cluster(config).await;
        }
        #[cfg(feature = "live")]
        let started = Self::start_with_live(config).await;
        #[cfg(not(feature = "live"))]
        let started = Self::start_single(config, no_sweep(), Prebuilt::default()).await;
        started
    }

    /// `dev` or `standalone` with Loams Live: Live starts first, so the
    /// metastore's GC loop can sweep its handle when both are on one cluster
    /// (R1 plan Task 12).
    #[cfg(feature = "live")]
    async fn start_with_live(config: ServerConfig) -> Result<Self, ServerError> {
        {
            // Built once, before anything is bound: Live's listener and the
            // node's surfaces share it (LV1 plan Task 10).
            let auth = crate::auth::Auth::from_config(&config.auth).map_err(ServerError::Config)?;
            let live = match config.live.clone() {
                Some(live) => {
                    #[cfg(feature = "live-tikv")]
                    let covered = matches!(
                        &config.meta,
                        MetaBackend::Tikv(meta) if matches!(
                            &live.store,
                            loams_live::StoreConfig::Tikv(t) if same_cluster(&meta.tikv.pd, &t.pd)
                        )
                    );
                    #[cfg(not(feature = "live-tikv"))]
                    let covered = false;
                    let bundles =
                        Store::from_url(&bucket_url(&config)?, Vec::<(String, String)>::new())?;
                    Some(LiveRuntime::start(live, bundles, !covered, auth.clone()).await?)
                }
                None => None,
            };
            #[cfg(feature = "live-tikv")]
            let sweep: Sweep = live
                .iter()
                .filter(|l| l.gc.is_none())
                .filter_map(|l| l.handle.store().as_tikv().cloned())
                .collect();
            #[cfg(not(feature = "live-tikv"))]
            let sweep = no_sweep();
            let pre = Prebuilt {
                auth: Some(auth),
                live: live.as_ref().map(|live| live.handle.routes()),
            };
            match Self::start_single(config, sweep, pre).await {
                Ok(mut server) => {
                    server.live = live;
                    Ok(server)
                }
                Err(err) => {
                    if let Some(live) = live {
                        live.stop().await;
                    }
                    Err(err)
                }
            }
        }
    }

    /// `dev` or `standalone`: the metastore, then every role.
    async fn start_single(
        config: ServerConfig,
        sweep: Sweep,
        pre: Prebuilt,
    ) -> Result<Self, ServerError> {
        let store = Store::from_url(&bucket_url(&config)?, Vec::<(String, String)>::new())?;
        #[cfg(feature = "tikv")]
        if let MetaBackend::Tikv(tikv) = &config.meta {
            return Self::start_on_tikv(tikv.clone(), store, config, sweep, pre).await;
        }
        #[cfg(not(feature = "tikv"))]
        let Sweep = sweep;
        let mut meta_config = MetaConfig::new(NODE_ID, config.data_dir.join("meta"), store.clone());
        meta_config.snapshot_every = config.snapshot_every;
        let node = MetaNode::start(meta_config, &Router::new()).await?;
        match Self::start_on(node.clone(), store, config, pre).await {
            Ok(server) => Ok(server),
            Err(err) => {
                if let Err(shutdown) = node.shutdown().await {
                    tracing::warn!(%shutdown, "stopping the metastore after a failed start");
                }
                Err(err)
            }
        }
    }

    /// Everything after the single meta node started.
    async fn start_on(
        node: MetaNode,
        store: Store,
        config: ServerConfig,
        pre: Prebuilt,
    ) -> Result<Self, ServerError> {
        // A no-op once the node is initialized, so restarts keep their state.
        node.initialize([NODE_ID]).await?;
        node.wait_for_leader(LEADER_WAIT).await?;
        let meta = MetaClient::new(
            node.clone(),
            Vec::new(),
            Arc::new(SystemClock),
            MetaClientConfig::default(),
        );
        let meta_store: Arc<dyn MetaStore> = meta.clone().into();
        let mut server = Self::serve_single(meta_store, store, config, pre).await?;
        server.node = Some(node);
        server.meta = Some(meta);
        Ok(server)
    }

    /// `dev` or `standalone` on the TiKV metastore (R1 plan Task 6): opens
    /// the metastore, starts the cluster GC loop on its handle, then every
    /// role as on the openraft store. Nothing is kept in `<data_dir>/meta`.
    #[cfg(feature = "tikv")]
    async fn start_on_tikv(
        tikv: loams_meta_tikv::TikvMetaConfig,
        store: Store,
        config: ServerConfig,
        sweep: Sweep,
        pre: Prebuilt,
    ) -> Result<Self, ServerError> {
        let meta = loams_meta_tikv::TikvMeta::open(tikv).await?;
        let gc = TikvGc::start(&meta, sweep)?;
        let meta_store: Arc<dyn MetaStore> = Arc::new(meta);
        match Self::serve_single(meta_store, store, config, pre).await {
            Ok(mut server) => {
                server.tikv_gc = Some(gc);
                Ok(server)
            }
            Err(err) => {
                gc.stop().await;
                Err(err)
            }
        }
    }

    /// Every role of a single-process server on `meta_store`, and the HTTP
    /// API. The caller keeps its metastore handles in the result.
    async fn serve_single(
        meta_store: Arc<dyn MetaStore>,
        store: Store,
        mut config: ServerConfig,
        pre: Prebuilt,
    ) -> Result<Self, ServerError> {
        startup_gate(&meta_store, &config.data_dir).await?;
        let (listener, local_addr) = bind(config.listen).await?;
        // LV1 plan Task 18: TLS (or a trusted proxy's connections only).
        let listener =
            crate::tls::MainListener::new(listener, &config.tls).map_err(ServerError::Config)?;
        // After the leader, before the collection service (row T0-6).
        let durable = Durable::start(&config, NODE_ID).await?;
        let setup = NodeSetup {
            node_id: NODE_ID,
            roles: Roles::all(),
            meta_store: meta_store.clone(),
            placement: Arc::new(AlwaysLocal),
            routing: None,
            forward_stats: Arc::new(ForwardStats::default()),
            extra_sources: Vec::new(),
            node_info: None,
            pre,
            durable: durable.endpoint(),
        };
        let parts = match Self::assemble(&mut config, store, setup).await {
            Ok(parts) => parts,
            Err(err) => {
                durable.stop().await;
                return Err(err);
            }
        };
        let (stop_http, stopped) = oneshot::channel::<()>();
        // The listener serves once the durable runtime has started (T0-6,
        // X8); a dropped gate means the start failed.
        let (serve_now, serve_gate) = oneshot::channel::<()>();
        let app = parts.app;
        // HTTP/3 at the main port's number on UDP, with its TLS files (design
        // 85, D2437): the same app; the TCP listener is the HTTP/2 fallback.
        #[cfg(feature = "transfer")]
        let h3 = api::transfers::h3_endpoint(&config.tls, local_addr);
        let http = tokio::spawn(async move {
            if serve_gate.await.is_err() {
                return;
            }
            #[cfg(feature = "transfer")]
            let h3_task =
                h3.map(|ep| tokio::spawn(loams_transfer::net::h3::serve(ep, app.clone())));
            let serve = axum::serve(listener, app).with_graceful_shutdown(async move {
                let _ = stopped.await;
            });
            if let Err(err) = serve.await {
                tracing::error!(%err, "HTTP server failed");
            }
            #[cfg(feature = "transfer")]
            if let Some(t) = h3_task {
                t.abort();
            }
        });
        let mut server = Self {
            local_addr,
            node: None,
            meta: None,
            #[cfg(feature = "tikv")]
            tikv_gc: None,
            #[cfg(feature = "live")]
            live: None,
            meta_store,
            writer: parts.writer,
            cache: parts.cache,
            collection_context: parts.collection_context,
            collection_factory: parts.collection_factory,
            collections: parts.collections,
            worker: parts.worker,
            hot: parts.hot,
            http,
            stop_http,
            tailscale: parts.tailscale,
            connect_socket: parts.connect_socket,
            flight: parts.flight,
            #[cfg(feature = "mysql-wire")]
            mysql_wire: parts.mysql_wire,
            #[cfg(feature = "pgwire")]
            pg: parts.pg,
            #[cfg(feature = "stream-grpc")]
            stream_grpc: parts.stream_grpc,
            #[cfg(feature = "sqldb")]
            sqlgate: parts.sqlgate,
            #[cfg(feature = "qdrant")]
            qdrant: parts.qdrant,
            durable,
            #[cfg(feature = "es")]
            es: parts.es,
            #[cfg(feature = "graph")]
            graph: parts.graph,
            #[cfg(feature = "git")]
            git: parts.git,
            #[cfg(feature = "meter")]
            meter: parts.meter_runtime,
            cluster: None,
        };
        // The runtime after the collection service, before the listener
        // serves (T0-6, X8). A failure shuts everything down again.
        if let Err(err) = server.durable.start_runtime(NODE_ID).await {
            drop(serve_now);
            if let Err(shutdown) = server.shutdown().await {
                tracing::warn!(%shutdown, "stopping after a failed start");
            }
            return Err(err);
        }
        let _ = serve_now.send(());
        tracing::info!(%local_addr, "loams is serving");
        Ok(server)
    }

    /// Rule 3: one cluster node. The metastore routes are served (with
    /// `/health`, and `503` for everything else) as soon as `--listen` is
    /// bound, so peers reach the replica during bootstrap.
    async fn start_cluster(mut config: ServerConfig) -> Result<Self, ServerError> {
        let cluster = config.cluster.clone().expect("cluster mode");
        let roles = cluster.effective_roles();
        let node_id = cluster.node_id;
        config.log.node_id = node_id;
        let store = Store::from_url(&bucket_url(&config)?, Vec::<(String, String)>::new())?;
        let (listener, local_addr) = if cluster.listen_stdin {
            inherited_listener(config.listen)?
        } else {
            bind(config.listen).await?
        };
        let transport = HttpTransport::new(cluster.transport)?;
        let mut meta_config = MetaConfig::new(node_id, config.data_dir.join("meta"), store.clone());
        meta_config.snapshot_every = config.snapshot_every;
        // A learner may start empty over existing snapshots (Task 9 rule 7).
        meta_config.allow_fresh_start_with_existing_snapshots = !roles.meta;
        let node = MetaNode::start_with(meta_config, Transport::Http(transport.clone())).await?;
        let late = LateRouter::default();
        let early = {
            let late = late.clone();
            meta_rpc::router(node.clone())
                .route(
                    "/health",
                    axum::routing::get(|| async { http::StatusCode::OK }),
                )
                .fallback(move |request: axum::extract::Request| late.clone().handle(request))
        };
        let (stop_http, stopped) = oneshot::channel::<()>();
        let http = tokio::spawn(async move {
            let serve = axum::serve(listener, early).with_graceful_shutdown(async move {
                let _ = stopped.await;
            });
            if let Err(err) = serve.await {
                tracing::error!(%err, "HTTP server failed");
            }
        });
        let started = Self::start_cluster_on(
            &mut config,
            &cluster,
            roles,
            node.clone(),
            store,
            transport.clone(),
        )
        .await;
        match started {
            Ok((meta, meta_store, registry, parts)) => {
                let app = parts.app.clone();
                let serve = late.clone();
                let mut server = Self {
                    local_addr,
                    node: Some(node),
                    meta: Some(meta),
                    #[cfg(feature = "tikv")]
                    tikv_gc: None,
                    #[cfg(feature = "live")]
                    live: None,
                    meta_store,
                    writer: parts.writer,
                    cache: parts.cache,
                    collection_context: parts.collection_context,
                    collection_factory: parts.collection_factory,
                    collections: parts.collections,
                    worker: parts.worker,
                    hot: parts.hot,
                    http,
                    stop_http,
                    tailscale: parts.tailscale,
                    connect_socket: parts.connect_socket,
                    flight: parts.flight,
                    #[cfg(feature = "mysql-wire")]
                    mysql_wire: parts.mysql_wire,
                    #[cfg(feature = "pgwire")]
                    pg: parts.pg,
                    #[cfg(feature = "stream-grpc")]
                    stream_grpc: parts.stream_grpc,
                    #[cfg(feature = "sqldb")]
                    sqlgate: parts.sqlgate,
                    #[cfg(feature = "qdrant")]
                    qdrant: parts.qdrant,
                    durable: parts.durable,
                    #[cfg(feature = "es")]
                    es: parts.es,
                    #[cfg(feature = "graph")]
                    graph: parts.graph,
                    #[cfg(feature = "git")]
                    git: parts.git,
                    #[cfg(feature = "meter")]
                    meter: parts.meter_runtime,
                    cluster: Some(ClusterRuntime {
                        node_id,
                        roles,
                        registry,
                        transport,
                        seeds: cluster.peers.values().cloned().collect(),
                        late,
                    }),
                };
                // The runtime after the collection service, before the
                // routes serve (T0-6, X8). A failure shuts the node down.
                if let Err(err) = server.durable.start_runtime(node_id).await {
                    if let Err(shutdown) = server.shutdown().await {
                        tracing::warn!(%shutdown, "stopping the node after a failed start");
                    }
                    return Err(err);
                }
                serve.set(app);
                tracing::info!(%local_addr, node_id, %roles, "loams cluster node is serving");
                Ok(server)
            }
            Err(err) => {
                if let Err(shutdown) = node.shutdown().await {
                    tracing::warn!(%shutdown, "stopping the metastore after a failed start");
                }
                let _ = stop_http.send(());
                http.abort();
                Err(err)
            }
        }
    }

    /// Rule 3 steps 4–8.
    #[allow(clippy::too_many_arguments)]
    async fn start_cluster_on(
        config: &mut ServerConfig,
        cluster: &ClusterConfig,
        roles: Roles,
        node: MetaNode,
        store: Store,
        transport: HttpTransport,
    ) -> Result<(MetaClient, Arc<dyn MetaStore>, Arc<NodeRegistry>, Assembled), ServerError> {
        let node_id = cluster.node_id;
        let lowest = cluster.peers.keys().next().copied();
        if roles.meta && lowest == Some(node_id) {
            // A no-op once initialized.
            node.initialize_with(cluster.peers.clone()).await?;
        }
        let seeds: Vec<String> = cluster.peers.values().cloned().collect();
        // Resolved before the join, so a name that does not resolve adds no
        // learner (PR #40 review).
        let addr = resolve(&cluster.advertise).await?;
        // A learner that never registered has no node lease, so the
        // membership task would never evict it: after a failed start, leave
        // (PR #40 review). That includes a failed join, which may have added
        // the learner anyway (a timeout while it caught up, PR #42 review);
        // leaving a node that is not a member changes nothing.
        let leave_after_failure = async || {
            if roles.meta {
                return;
            }
            let left =
                meta_rpc::leave(&transport, &seeds, LeaveRequest { node_id }, LEAVE_WAIT).await;
            if let Err(err) = left {
                tracing::warn!(%err, "leaving the metastore membership after a failed start");
            }
        };
        let joined = meta_rpc::join(
            &transport,
            &seeds,
            JoinRequest {
                node_id,
                addr: cluster.advertise.clone(),
            },
            cluster.join_deadline,
        )
        .await;
        let join_changed = match joined {
            Ok(changed) => changed,
            Err(err) => {
                leave_after_failure().await;
                return Err(err.into());
            }
        };
        tracing::info!(node_id, changed = join_changed, "joined the metastore");
        let started = Self::start_joined(
            config,
            cluster,
            roles,
            node,
            store,
            transport.clone(),
            addr,
            join_changed,
        )
        .await;
        if started.is_err() {
            leave_after_failure().await;
        }
        started
    }

    /// Rule 3 steps 5–8, after the join.
    #[allow(clippy::too_many_arguments)]
    async fn start_joined(
        config: &mut ServerConfig,
        cluster: &ClusterConfig,
        roles: Roles,
        node: MetaNode,
        store: Store,
        transport: HttpTransport,
        addr: SocketAddr,
        join_changed: bool,
    ) -> Result<(MetaClient, Arc<dyn MetaStore>, Arc<NodeRegistry>, Assembled), ServerError> {
        let node_id = cluster.node_id;
        let seeds: Vec<String> = cluster.peers.values().cloned().collect();
        node.wait_for_leader(CLUSTER_LEADER_WAIT).await?;
        let (meta, meta_store) = cluster::metastore(&node, &transport);
        let registry = NodeRegistry::register(
            meta_store.clone(),
            NodeDescriptor {
                node_id,
                incarnation: Ulid::generate(),
                addr,
                roles,
                zone: cluster.zone.clone(),
            },
            cluster.registry,
        )
        .await?;
        let placement = Arc::new(PlacementImpl::new(registry.clone(), cluster.replication));
        let forward_stats = Arc::new(ForwardStats::default());
        let remote = match RemoteReadsImpl::new(
            placement.clone(),
            forward_stats.clone(),
            RemoteReadsConfig::default(),
        ) {
            Ok(remote) => Arc::new(remote),
            Err(err) => {
                registry.deregister().await;
                return Err(err.into());
            }
        };
        let mut extra_sources: Vec<Arc<dyn TaskSource>> = Vec::new();
        if roles.worker {
            extra_sources.push(Arc::new(MembershipSource::new(
                node.clone(),
                transport.clone(),
                seeds,
                cluster.learner_expiry,
                cluster.membership_interval,
            )));
        }
        let setup = NodeSetup {
            node_id,
            roles,
            meta_store: meta_store.clone(),
            placement: placement.clone(),
            routing: Some((placement, remote)),
            forward_stats,
            extra_sources,
            node_info: Some(Arc::new(ClusterInfo {
                node: node.clone(),
                join_changed,
            })),
            pre: Prebuilt::default(),
            durable: None,
        };
        // After the leader, before the collection service (row T0-6).
        let durable = match Durable::start(config, node_id).await {
            Ok(durable) => durable,
            Err(err) => {
                registry.deregister().await;
                return Err(err);
            }
        };
        let mut parts = match Self::assemble(config, store, setup).await {
            Ok(parts) => parts,
            Err(err) => {
                durable.stop().await;
                registry.deregister().await;
                return Err(err);
            }
        };
        parts.durable = durable;
        // The caller starts the durable runtime, then serves the routes
        // (`late.set`), once the node is whole enough to shut down.
        Ok((meta, meta_store, registry, parts))
    }

    /// The components of a node, per its roles (Task 11 rule 2; single-node
    /// modes run every role). Every fallible step runs before any task is
    /// spawned, so a failure leaves only the caller's metastore to stop.
    async fn assemble(
        config: &mut ServerConfig,
        store: Store,
        setup: NodeSetup,
    ) -> Result<Assembled, ServerError> {
        let NodeSetup {
            node_id,
            roles,
            meta_store,
            placement,
            routing,
            forward_stats,
            extra_sources,
            node_info,
            pre,
            durable,
        } = setup;
        // Built before anything is bound: a bad key file or OIDC setting
        // stops the start with nothing to undo. A cluster node is not
        // guarded (validate_auth keeps it off).
        #[cfg(feature = "live")]
        let live = pre.live;
        let auth = match (pre.auth, &config.cluster) {
            (Some(auth), None) => auth,
            (None, None) => {
                crate::auth::Auth::from_config(&config.auth).map_err(ServerError::Config)?
            }
            (_, Some(_)) => crate::auth::Auth::off(),
        };
        // The principal store (API2 Tasks 6 and 7): its sessions and API
        // keys authenticate through `auth` from here on.
        let principals = crate::auth::Principals::attach(&auth, meta_store.clone(), store.clone());
        // The `can_*` checks (design 76 §9, IA2 Task 4): `LOAMS_AUTHZ`, built-in by default.
        let principals = {
            let authz = config
                .auth
                .authz
                .build(&principals.directory)
                .map_err(ServerError::Config)?;
            principals.with_authz(authz)
        };
        // The identity sync (design 76 §8.1, plan IA1 IA3): off unless LOAMS_IDENTITY_SYNC=on,
        // with LOAMS_AUTHZ=openfga. Its state is a document of its own in the metastore.
        if config.cluster.is_none()
            && let Some(sync) = crate::auth::identity_sync::SyncSettings::from_vars(
                |k| std::env::var(k).ok(),
                &config.auth.authz,
                config
                    .auth
                    .oidc
                    .as_ref()
                    .and_then(|o| o.issuers.first())
                    .map(|i| i.issuer.as_str()),
            )
            .map_err(ServerError::Config)?
        {
            let state = Arc::new(crate::auth::identity_sync::MetaSyncState::new(
                meta_store.clone(),
                store.clone(),
            ));
            crate::auth::identity_sync::start(
                &sync,
                &config.auth.authz,
                principals.directory.clone(),
                state,
            )
            .await
            .map_err(ServerError::Config)?;
        }
        // LV1 plan Task 17: a revoked key or a removed member closes its
        // Live sessions.
        #[cfg(feature = "live")]
        if let Some(live) = &live {
            tokio::spawn(crate::api::live::close_revoked(
                principals.directory.subscribe(),
                live.clone(),
            ));
        }
        let guard = crate::auth::AuthLayerState::new(auth.clone(), &[]);
        // Whether requests are authenticated, for the wire listeners' graph caller below.
        #[cfg(any(feature = "pgwire", feature = "mysql-wire"))]
        let auth_enforced = auth.enforced();
        let main_guard =
            crate::auth::AuthLayerState::new(auth.clone(), crate::auth::layer::MAIN_PUBLIC_PATHS);
        let connect_guard = crate::auth::AuthLayerState::connect_socket(auth.clone());
        let tailscale_guard = crate::auth::AuthLayerState::tailscale_target(
            auth,
            crate::auth::layer::MAIN_PUBLIC_PATHS,
        );
        // LV1 plan Task 16: where Live is served, its end users' calls go to
        // the app (the main port serves Live on loopback, or behind TLS and
        // authentication, Task 18).
        #[cfg(feature = "live")]
        let (main_guard, tailscale_guard) = match &live {
            Some(_) if main_port_serves_live(config) => {
                let origins = config.live.as_ref().map(app_origin_set);
                let main_guard = main_guard.leaving_to_apps();
                (
                    match origins {
                        Some(origins) => main_guard.with_app_origins(origins),
                        None => main_guard,
                    },
                    tailscale_guard.leaving_to_apps(),
                )
            }
            Some(_) => (main_guard, tailscale_guard.leaving_to_apps()),
            None => (main_guard, tailscale_guard),
        };
        // Scan plans name the Lance datasets under the bucket (Task 14 rule 8).
        config.query.lance_base_url = Some(bucket_url(config)?);
        let cache = RangeCache::new(store.clone(), config.cache.clone()).await?;
        // Flight SQL listens after the HTTP API (rule 5.3), on gateways only.
        let flight_listener = match flight_addr(config).filter(|_| roles.gateway) {
            Some(addr) => match bind(addr).await {
                Ok(bound) => Some(bound),
                Err(err) => {
                    if let Err(err) = cache.close().await {
                        tracing::warn!(%err, "closing the cache after a failed start");
                    }
                    return Err(err);
                }
            },
            None => None,
        };
        // The `tailscale serve` target, on a standalone gateway in tailscale
        // mode only (validate_auth checked it is loopback).
        let tailscale_addr = config
            .auth
            .tailscale_listener()
            .map_err(ServerError::Config)?
            .filter(|_| roles.gateway && config.cluster.is_none());
        let tailscale_listener = match tailscale_addr {
            Some(addr) => match bind(addr).await {
                Ok(bound) => Some(bound),
                Err(err) => {
                    if let Err(err) = cache.close().await {
                        tracing::warn!(%err, "closing the cache after a failed start");
                    }
                    return Err(err);
                }
            },
            None => None,
        };
        // The connect socket, on a standalone gateway only (validate refused
        // it on a cluster node).
        let connect_listener = match config
            .connect_socket
            .as_deref()
            .filter(|_| roles.gateway && config.cluster.is_none())
        {
            Some(path) => match ConnectSocket::bind(path) {
                Ok(listener) => Some((listener, path.to_path_buf())),
                Err(err) => {
                    if let Err(err) = cache.close().await {
                        tracing::warn!(%err, "closing the cache after a failed start");
                    }
                    return Err(err);
                }
            },
            None => None,
        };
        // The MySQL wire listener, on gateways only: bound here, before any
        // task is spawned, and served once the collection service exists.
        #[cfg(feature = "mysql-wire")]
        let mysql_listener = match config.mysql_wire.clone().filter(|_| roles.gateway) {
            Some(mysql_config) => match crate::mysql_wire::listen(&mysql_config).await {
                Ok(bound) => Some((bound, mysql_config)),
                Err(source) => {
                    if let Err(err) = cache.close().await {
                        tracing::warn!(%err, "closing the cache after a failed start");
                    }
                    return Err(ServerError::MysqlListen {
                        addr: mysql_config.listen,
                        source,
                    });
                }
            },
            None => None,
        };
        // The PostgreSQL wire listener, on gateways only: bound here, before
        // any task is spawned, and served once the collection service exists.
        #[cfg(feature = "pgwire")]
        let pg_listener = match config.pg.clone().filter(|_| roles.gateway) {
            Some(pg_config) => match crate::pg::listen(&pg_config).await {
                Ok(bound) => Some((bound, pg_config)),
                Err(source) => {
                    if let Err(err) = cache.close().await {
                        tracing::warn!(%err, "closing the cache after a failed start");
                    }
                    return Err(ServerError::PgListen {
                        addr: pg_config.listen,
                        source,
                    });
                }
            },
            None => None,
        };
        #[cfg(feature = "stream-grpc")]
        let stream_grpc_listener = match config.stream_grpc.filter(|_| roles.gateway) {
            Some(addr) => match loams_stream_grpc::bind(addr)
                .await
                .and_then(|listener| listener.local_addr().map(|bound| (listener, bound)))
            {
                Ok(bound) => Some(bound),
                Err(source) => {
                    if let Err(err) = cache.close().await {
                        tracing::warn!(%err, "closing the cache after a failed start");
                    }
                    return Err(ServerError::StreamGrpcListen { addr, source });
                }
            },
            None => None,
        };
        // The Loams SQL gate, on gateways only (plan SQ1 Task 4).
        #[cfg(feature = "sqldb")]
        let sqlgate_listener = match config.sqlgate.clone().filter(|_| roles.gateway) {
            Some(gate_config) => match crate::sqlgate::listen(&gate_config).await {
                Ok((listener, addr)) => Some((listener, addr, gate_config)),
                Err(source) => {
                    if let Err(err) = cache.close().await {
                        tracing::warn!(%err, "closing the cache after a failed start");
                    }
                    return Err(ServerError::SqlgateListen {
                        addr: gate_config.listen,
                        source,
                    });
                }
            },
            None => None,
        };
        #[cfg(feature = "sqldb")]
        let sqlgate = match sqlgate_listener {
            Some((listener, addr, gate_config)) => {
                match crate::sqlgate::start(listener, addr, &gate_config) {
                    Ok(handle) => Some(handle),
                    Err(source) => {
                        if let Err(err) = cache.close().await {
                            tracing::warn!(%err, "closing the cache after a failed start");
                        }
                        return Err(ServerError::SqlgateListen { addr, source });
                    }
                }
            }
            None => None,
        };
        // The Qdrant gateway's listeners, on gateways only (plan M1.4 Task
        // 2): bound here, before any task is spawned, and served once the
        // collection service exists (row T2-2).
        #[cfg(feature = "qdrant")]
        let qdrant_listeners = match config.qdrant.clone().filter(|_| roles.gateway) {
            Some(qdrant) => match bind_qdrant(&qdrant).await {
                Ok(bound) => Some((qdrant, bound)),
                Err(err) => {
                    if let Err(err) = cache.close().await {
                        tracing::warn!(%err, "closing the cache after a failed start");
                    }
                    return Err(err);
                }
            },
            None => None,
        };
        // The Elasticsearch gateway's listener, next to Qdrant's (plan M1.5
        // Task 1, row E13): bound here and served once the collection
        // service exists.
        #[cfg(feature = "es")]
        let es_listener = match config.es.clone().filter(|_| roles.gateway) {
            Some(es) => match tokio::net::TcpListener::bind(es.listen).await {
                Ok(bound) => Some((es, bound)),
                Err(source) => {
                    if let Err(err) = cache.close().await {
                        tracing::warn!(%err, "closing the cache after a failed start");
                    }
                    return Err(ServerError::EsListen {
                        addr: es.listen,
                        source,
                    });
                }
            },
            None => None,
        };

        // Loams Git's listener (GT1 Task 16): bound, and its token file read, here, where a
        // failure still has nothing to undo but the cache; served once the git runtime starts.
        #[cfg(feature = "git")]
        let git_listener = match config
            .git
            .listen
            .filter(|_| roles.gateway && config.git.enabled)
        {
            Some(addr) => {
                let bound = async {
                    let tokens = config
                        .git
                        .tokens
                        .as_deref()
                        .ok_or_else(|| "--git-tokens is required".to_string())?;
                    let authorizer =
                        api::git_auth::LoopbackDevAuthorizer::load(tokens, meta_store.clone())?;
                    let listener = tokio::net::TcpListener::bind(addr)
                        .await
                        .map_err(|err| err.to_string())?;
                    Ok::<_, String>((listener, Arc::new(authorizer)))
                }
                .await;
                match bound {
                    Ok(bound) => Some(bound),
                    Err(reason) => {
                        if let Err(err) = cache.close().await {
                            tracing::warn!(%err, "closing the cache after a failed start");
                        }
                        return Err(ServerError::GitListen { addr, reason });
                    }
                }
            }
            None => None,
        };

        // Validated above, so this cannot fail. A node without the `log`
        // role still holds a writer (the service needs one), but serves no
        // write route, so it never appends.
        let writer = LogWriter::start(meta_store.clone(), store.clone(), config.log.clone())?;
        let reader = LogReader::new(meta_store.clone(), cache.clone());
        let stop_early = |writer: LogWriter, cache: RangeCache| async move {
            if let Err(err) = writer.shutdown().await {
                tracing::warn!(%err, "stopping the log writer after a failed start");
            }
            if let Err(err) = cache.close().await {
                tracing::warn!(%err, "closing the cache after a failed start");
            }
        };
        // Unique per process incarnation, as leases require.
        let owner = format!("node-{node_id}-{}", Ulid::generate());
        let mut worker = Worker::new(
            meta_store.clone(),
            WorkerConfig {
                poll_interval: config.worker_poll_interval,
                lease_ttl: config.worker_lease_ttl,
                ..WorkerConfig::new(owner)
            },
        );
        let mut collection = config.collection.clone();
        collection.max_commit_delay = config.link.max_commit_delay;
        collection.keep_manifests = config.gc.keep_manifests;
        // Lance reads go through the range cache (M1.3 Ruling 9), which the
        // hot tier's fragment prefetch fills.
        let collection_context = CollectionContext {
            meta: meta_store.clone(),
            store: store.clone(),
            cache: cache.clone(),
            lance: LanceEnv::with_cache(store.clone(), cache.clone(), config.lance.clone()),
            manifests: ManifestCache::new(collection.manifest_cache_entries),
            config: collection,
        };
        let collection_factory = Arc::new(CollectionTargetFactory::new(collection_context.clone()));
        let registry = TargetRegistry::new()
            .with(Arc::new(CounterTargetFactory::new(
                store.clone(),
                config.link.max_commit_delay,
            )))
            .with(collection_factory.clone());
        // The API reports which links this source finds unregistered.
        let link_apply = LinkApplySource::new(
            meta_store.clone(),
            reader.clone(),
            registry.clone(),
            config.link.clone(),
        );
        worker.add_source(Arc::new(link_apply.clone()));
        worker.add_source(Arc::new(SegmenterSource::new(
            store.clone(),
            cache.clone(),
            config.segmenter.clone(),
        )));
        worker.add_source(Arc::new(IndexBuildSource::new(collection_context.clone())));
        // M1.3: maintenance, then hot artifact builds (Task 8 rule 5).
        if config.maintenance.merge {
            worker.add_source(Arc::new(SplitMergeSource::new(
                collection_context.clone(),
                config.maintenance.clone(),
            )));
        }
        if config.maintenance.compaction {
            worker.add_source(Arc::new(LanceCompactionSource::new(
                collection_context.clone(),
                config.maintenance.clone(),
            )));
        }
        let engine = config
            .hnsw_engine
            .clone()
            .unwrap_or_else(loams_hnsw::default_engine);
        if config.hot.enabled && (cfg!(feature = "hnsw") || config.hnsw_engine.is_some()) {
            let hot_build = HotBuildConfig {
                pin_all: config.hot.pin_all,
                ..config.hot_build.clone()
            };
            match HotBuildSource::new(collection_context.clone(), hot_build, engine.clone()) {
                Ok(source) => worker.add_source(Arc::new(source)),
                Err(err) => {
                    stop_early(writer, cache).await;
                    return Err(err.into());
                }
            }
        }
        worker.add_source(Arc::new(RetentionSource::new(config.retention.clone())));
        worker.add_source(Arc::new(CollectionTrimSource::new(
            collection_context.clone(),
        )));
        worker.add_source(Arc::new(GcSource::with_roots(
            store.clone(),
            config.gc.clone(),
            vec![
                Arc::new(LinkGcRoots),
                Arc::new(CollectionGcRoots::new(collection_context.clone())),
                Arc::new(PkGcRoots),
            ],
        )));
        for source in extra_sources {
            worker.add_source(source);
        }
        let hot = match config.hot.enabled && roles.query {
            true => match HotTierImpl::start(
                collection_context.clone(),
                config.hot.clone(),
                node_id,
                placement.clone(),
                engine,
            )
            .await
            {
                Ok(tier) => Some(tier),
                Err(err) => {
                    stop_early(writer, cache).await;
                    return Err(err.into());
                }
            },
            false => None,
        };
        // Rule 5.1: after the collection context, before the router, on the
        // reader built above (the server keeps none, row 0.52).
        let collections = CollectionService::new(
            collection_context.clone(),
            CollectionWriter::new(meta_store.clone(), writer.clone()),
            reader.clone(),
            config.query.clone(),
        );
        if let Some(tier) = &hot {
            collections.set_hot_tier(Arc::new(tier.clone()));
        }
        // The node registry, for Loams Graph's ownership on a cluster node (GR1 Task 12).
        #[cfg(feature = "graph")]
        let node_registry = routing
            .as_ref()
            .map(|(placement, _)| placement.registry().clone());
        // The placement, for Loams Git's sequencer owners on a cluster node (GT1 Task 14).
        #[cfg(feature = "git")]
        let git_placement = routing.as_ref().map(|(placement, _)| placement.clone());
        // Where other nodes reach this one, for the meter's counter reads
        // forwarded to a partition's owner.
        #[cfg(feature = "meter")]
        let meter_advertise = routing
            .as_ref()
            .map(|(placement, _)| placement.registry().me().addr.to_string());
        if let Some((placement, remote)) = routing {
            collections.set_placement(placement, remote);
        }
        // Who the PostgreSQL and MySQL wire sessions' graph reads are for (GR1 R24.12): they
        // authenticate nobody (loopback only), so with authentication off it is the operator,
        // as on the main port, and with it on nobody, who reads no graph.
        #[cfg(any(feature = "pgwire", feature = "mysql-wire"))]
        let wire_caller = {
            let operator = (!auth_enforced).then(crate::auth::Auth::dev_loopback);
            api::graph_caller(&principals, operator.as_ref())
        };
        // Every fallible PostgreSQL step (the `pg_catalog` setup) runs here,
        // before the worker starts; the accept loop is spawned only after.
        #[cfg(feature = "pgwire")]
        let pg = match pg_listener {
            Some((bound, pg_config)) => {
                match crate::pg::prepare(
                    collections.clone(),
                    bound,
                    pg_config.clone(),
                    wire_caller.clone(),
                )
                .await
                {
                    Ok(prepared) => Some(prepared),
                    Err(source) => {
                        collections.shutdown().await;
                        if let Some(tier) = &hot {
                            tier.shutdown().await;
                        }
                        collection_factory.close().await;
                        stop_early(writer, cache).await;
                        return Err(ServerError::PgListen {
                            addr: pg_config.listen,
                            source,
                        });
                    }
                }
            }
            None => None,
        };
        let worker = roles.worker.then(|| worker.start());
        #[cfg(feature = "pgwire")]
        let pg = pg.map(crate::pg::serve);
        // After every fallible step, like the PostgreSQL accept loop.
        #[cfg(feature = "mysql-wire")]
        let mysql_wire = mysql_listener.map(|(bound, mysql_config)| {
            crate::mysql_wire::start(collections.clone(), bound, mysql_config, wire_caller)
        });
        #[cfg(feature = "qdrant")]
        let qdrant = match qdrant_listeners {
            Some((qdrant, (rest, grpc))) => {
                let stop = CancellationToken::new();
                let handle = loams_qdrant::QdrantGateway::new(collections.clone(), qdrant)
                    .serve_with(
                        rest,
                        grpc,
                        stop.clone(),
                        |router| guard.wrap(router),
                        |routes| guard.wrap_grpc(routes),
                    )
                    .await;
                Some(Qdrant { handle, stop })
            }
            None => None,
        };
        #[cfg(feature = "es")]
        let es = es_listener.map(|(es, listener)| {
            let stop = CancellationToken::new();
            let handle = loams_es::EsGateway::new(collections.clone(), es).serve_with(
                listener,
                stop.clone(),
                |router| guard.wrap(router),
            );
            Es { handle, stop }
        });
        let internal = reqwest::Client::builder()
            .connect_timeout(api::hot::OWNER_CONNECT_TIMEOUT)
            .timeout(api::hot::OWNER_TIMEOUT)
            .build()
            .unwrap_or_default();
        // Loams Graph on dev and standalone (a cluster node gets the `graph` role in GR1 Task 12).
        #[cfg(feature = "graph")]
        let graph = match config.serves_graph() {
            true => Some(GraphRuntime::start(
                &config.graph,
                meta_store.clone(),
                store.clone(),
                reader.clone(),
                config.log.node_id,
                node_registry,
                config.link.clone(),
            )?),
            false => None,
        };
        // `Search.expand` runs on this node's graphs (GR1 Task 20).
        #[cfg(feature = "graph")]
        if let Some(graph) = &graph {
            let expander = Arc::new(loams_graph::retrieval::LoamsGraphExpander::new(
                (*graph.admin).clone(),
            ));
            collections.set_graph_expander(expander.clone());
            // And the graph table functions (GR1 Task 21).
            collections.set_graph_tables(expander);
        }
        let house = match roles.gateway {
            true => crate::api::house_proxy::HouseProxy::start(&config.house)
                .map_err(|err| ServerError::Config(format!("the House proxy: {err}")))?,
            false => None,
        };
        // Loams Git on every gateway node (GT1 Task 13), after the last fallible step so its
        // purge task never outlives a failed start. On a cluster node each repository's
        // sequencer runs on its rendezvous owner under the `task/git-seq/<ns>/<repo>` lease, and
        // other nodes forward commits there (GT1 Task 14); fencing by the bucket keeps any
        // second writer safe.
        #[cfg(feature = "git")]
        let git = (roles.gateway && config.git.enabled).then(|| {
            let cluster = git_placement.as_ref().map(|placement| {
                let me = placement.registry().me();
                loams_git::Cluster {
                    placement: Arc::new(api::git_internal::RegistryPlacement::new(
                        placement.clone(),
                    )),
                    leases: Arc::new(api::git_internal::MetaLeases::new(
                        meta_store.clone(),
                        format!("v1;{};{};{}", me.node_id, me.incarnation, me.addr),
                    )),
                    forwarder: Arc::new(api::git_internal::HttpSeqForwarder::new(internal.clone())),
                    config: loams_git::OwnerConfig::default(),
                }
            });
            let sink = Arc::new(api::git_events::StreamSink::new(
                meta_store.clone(),
                writer.clone(),
                config.cloudevents,
                node_id,
            ));
            GitRuntimeHandle::start(
                &config.git,
                meta_store.clone(),
                store.clone(),
                cache.clone(),
                sink,
                cluster,
                git_listener,
            )
        });
        // Loams Meter's public services, on gateway nodes (ME1 role wiring,
        // part 1): records in the metastore and the bucket, beside the
        // principal directory. A node that cannot build it serves without it
        // (its RPCs answer `meter_not_served`) rather than not starting.
        //
        // Part 2: the counter owners run beside them (partitions of
        // `_system/_meter` by fenced leases in the metastore), and a cluster
        // node serves the internal `LeaseService` on its cluster listener.
        // The deployment CA (design §83 §6.3, CT1 Task 3): `meta` nodes of
        // a cluster serve `loams.ca.internal.v1.CertificateService` on the
        // cluster listener. It is internal, so never on a port with end-user
        // authentication.
        #[cfg(feature = "transport")]
        let ca_routes = (roles.meta && config.cluster.is_some()).then(|| {
            crate::ca::routes(Arc::new(crate::ca::authority(
                meta_store.clone(),
                store.clone(),
                &config.data_dir,
            )))
        });
        #[cfg(feature = "meter")]
        let (meter, meter_runtime, meter_internal, meter_main, meter_public, meter_gate) =
            if roles.gateway {
                match api::meter::build(
                    meta_store.clone(),
                    store.clone(),
                    reader.clone(),
                    node_id,
                    &principals,
                    writer.clone(),
                    &config.meter,
                    &config.data_dir,
                    meter_advertise,
                ) {
                    Ok(node) => {
                        let started = node.start();
                        (
                            Some(started.role),
                            Some(started.runtime),
                            Some(started.internal),
                            Some(started.main),
                            Some(started.public),
                            Some(started.entitlements),
                        )
                    }
                    Err(err) => {
                        tracing::warn!(%err, "Loams Meter's services are off on this node");
                        (None, None, None, None, None, None)
                    }
                }
            } else {
                (None, None, None, None, None, None)
            };
        let state = AppState {
            meta: meta_store.clone(),
            writer: writer.clone(),
            reader,
            store: store.clone(),
            registry,
            link_apply,
            collections: collections.clone(),
            hot: hot.clone(),
            placement,
            node_id,
            internal,
            hot_pin_all: config.hot.pin_all,
            roles,
            forwarded: roles.query.then(|| ForwardedReads {
                service: collections.clone(),
                stats: forward_stats.clone(),
            }),
            forward_stats,
            node_info,
            cloudevents: config.cloudevents,
            reflection: config.reflection,
            #[cfg(feature = "graph")]
            graph: graph.as_ref().map(|g| g.admin.clone()),
            house,
            #[cfg(feature = "git")]
            git: git.as_ref().map(|g| g.runtime.clone()),
            principals: principals.clone(),
            #[cfg(feature = "live")]
            live,
            durable: durable.filter(|_| main_port_serves_durable(config)),
            #[cfg(feature = "meter")]
            meter,
            #[cfg(feature = "meter")]
            meter_gate: meter_gate.clone(),
        };
        // Resumable transfers and job runs (design 85, DQ1 Task 5): one set of
        // routes on this node's metastore and bucket, on every gateway router.
        #[cfg(feature = "transfer")]
        let transfers = api::transfers::routes(meta_store.clone(), store.clone());
        let (app, tailscale, connect_socket) = match (roles.gateway, config.cluster.is_some()) {
            (true, false) => {
                #[cfg(feature = "live")]
                let main_state = without_live_beyond_loopback(&state, config);
                let router = api::router(state);
                #[cfg(feature = "transfer")]
                let router = router.merge(transfers.clone());
                #[cfg(feature = "live")]
                let main_router = match main_state {
                    // A main state without Live builds its own router, which
                    // needs the transfer routes too.
                    Some(main_state) => {
                        let r = api::router(main_state);
                        #[cfg(feature = "transfer")]
                        let r = r.merge(transfers.clone());
                        r
                    }
                    None => router.clone(),
                };
                #[cfg(not(feature = "live"))]
                let main_router = router.clone();
                // The meter's service ingest on a single node's main port, for
                // the operator (its node agent's key).
                #[cfg(feature = "meter")]
                let main_router = match (meter_main, meter_public) {
                    (Some(main), Some(public)) => main_router.merge(main).merge(public),
                    _ => main_router,
                };
                // The console ships with the deployment (crate::console):
                // served next to the API on both listeners, outside the
                // authentication layer (its files are public; its calls are
                // not), so it is always same-origin with its engine.
                let console = config.console_dir.as_deref().and_then(|dir| {
                    let router = crate::console::router(dir, &config.console);
                    match &router {
                        Some(_) => {
                            tracing::info!(path = %dir.display(), "the console is served at /ui/")
                        }
                        None => tracing::warn!(
                            path = %dir.display(),
                            "no console build here (no index.html): /ui/ is not served"
                        ),
                    }
                    router
                });
                let with_console = |router: axum::Router| match &console {
                    Some(console) => router.merge(console.clone()),
                    None => router,
                };
                // The same API behind the layer that honours Tailscale
                // identity headers; the main port strips them.
                let tailscale = tailscale_listener.map(|(listener, addr)| {
                    TailscaleTarget::start(
                        listener,
                        addr,
                        with_console(tailscale_guard.wrap(router)),
                    )
                });
                // The same API on the connect socket, behind its API-key-only
                // layer, without the console.
                let connect_socket = connect_listener.map(|(listener, path)| {
                    tracing::info!(path = %path.display(), "the connect socket is served");
                    ConnectSocket::start(listener, path, connect_guard.wrap(main_router.clone()))
                });
                (
                    with_console(main_guard.wrap(main_router)),
                    tailscale,
                    connect_socket,
                )
            }
            (true, true) => {
                let router = api::router(state);
                #[cfg(feature = "transfer")]
                let router = router.merge(transfers);
                // The meter's `LeaseService` is internal: only on the cluster
                // listener, never on a port with end-user authentication.
                #[cfg(feature = "meter")]
                let router = match (meter_internal, meter_public) {
                    (Some(internal), Some(public)) => router.merge(internal).merge(public),
                    _ => router,
                };
                #[cfg(feature = "transport")]
                let router = match ca_routes {
                    Some(ca) => router.merge(ca),
                    None => router,
                };
                (router, None, None)
            }
            (false, _) => {
                let router = api::internal_router(state);
                #[cfg(feature = "transport")]
                let router = match ca_routes {
                    Some(ca) => router.merge(ca),
                    None => router,
                };
                (router, None, None)
            }
        };
        let flight = flight_listener.map(|(listener, addr)| {
            let streams: Arc<dyn StreamProducer> = Arc::new(NativeStreamProducer {
                meta: meta_store.clone(),
                writer: writer.clone(),
            });
            // The meter's gate on produce (D2400): stream_ingest_bytes.
            #[cfg(feature = "meter")]
            let streams: Arc<dyn StreamProducer> = match &meter_gate {
                Some(e) => Arc::new(loams_query::metered::MeteredProducer::new(
                    streams,
                    e.clone(),
                )),
                None => streams,
            };
            // Each statement's graph reads are its principal's (GR1 R24.12).
            let principals = principals.clone();
            let graph_callers: loams_query::flight::GraphCallers = Arc::new(move |extensions| {
                api::graph_caller(&principals, extensions.get::<crate::auth::Principal>())
            });
            Flight::start(
                listener,
                addr,
                collections.clone(),
                streams,
                config.flight.clone(),
                guard.clone(),
                graph_callers,
            )
        });
        #[cfg(feature = "stream-grpc")]
        let stream_grpc = stream_grpc_listener.map(|(listener, addr)| {
            let streams: Arc<dyn StreamProducer> = Arc::new(NativeStreamProducer {
                meta: meta_store.clone(),
                writer: writer.clone(),
            });
            // The meter's gate on produce (D2400): stream_ingest_bytes.
            #[cfg(feature = "meter")]
            let streams: Arc<dyn StreamProducer> = match &meter_gate {
                Some(e) => Arc::new(loams_query::metered::MeteredProducer::new(
                    streams,
                    e.clone(),
                )),
                None => streams,
            };
            let events: Arc<dyn loams_stream_grpc::EventProducer> =
                Arc::new(api::events::NativeEventProducer {
                    meta: meta_store.clone(),
                    writer: writer.clone(),
                    config: config.cloudevents,
                    node_id,
                });
            StreamGrpc::start(listener, addr, streams, events)
        });
        Ok(Assembled {
            writer,
            cache,
            collection_context,
            collection_factory,
            collections,
            worker,
            hot,
            app,
            tailscale,
            connect_socket,
            flight,
            #[cfg(feature = "mysql-wire")]
            mysql_wire,
            #[cfg(feature = "pgwire")]
            pg,
            #[cfg(feature = "stream-grpc")]
            stream_grpc,
            #[cfg(feature = "sqldb")]
            sqlgate,
            #[cfg(feature = "qdrant")]
            qdrant,
            durable: Durable::none(),
            #[cfg(feature = "es")]
            es,
            #[cfg(feature = "graph")]
            graph,
            #[cfg(feature = "git")]
            git,
            #[cfg(feature = "meter")]
            meter_runtime,
        })
    }

    /// The address the HTTP API listens on.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// The git listener's address (`--git-listen`, GT1 Task 16), when it is served.
    #[cfg(feature = "git")]
    pub fn git_listen_addr(&self) -> Option<SocketAddr> {
        self.git.as_ref().and_then(GitRuntimeHandle::listen_addr)
    }

    /// How many scheduled graph catalog sweeps have completed, when Loams Graph is served
    /// (GR1 Task 5, R4.9).
    #[cfg(feature = "graph")]
    #[must_use]
    pub fn graph_sweeps(&self) -> Option<u64> {
        self.graph
            .as_ref()
            .map(|g| g.sweeps.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// How many graph checkpoints the maintenance task has taken, when Loams Graph is served
    /// (GR1 Task 13).
    #[cfg(feature = "graph")]
    #[must_use]
    pub fn graph_checkpoints(&self) -> Option<u64> {
        self.graph
            .as_ref()
            .map(|g| g.checkpoints.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// How many graphs the maintenance task has evicted, when Loams Graph is served (GR1 Task
    /// 14).
    #[cfg(feature = "graph")]
    #[must_use]
    pub fn graph_evictions(&self) -> Option<u64> {
        self.graph
            .as_ref()
            .map(|g| g.evictions.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// The graph data directory, when Loams Graph is served with one (GR1 Task 5).
    #[cfg(feature = "graph")]
    pub fn graph_data_dir(&self) -> Option<&std::path::Path> {
        self.graph.as_ref().and_then(|g| g.data_dir.as_deref())
    }

    /// The openraft metastore client, for embedding and tests; `None` on the
    /// TiKV metastore (use [`Server::meta_store`]).
    pub fn meta(&self) -> Option<&MetaClient> {
        self.meta.as_ref()
    }

    /// The metastore as the trait object the server hands to every
    /// component (the log, the worker and its sources, collection storage
    /// and the HTTP API): the same handle each of them holds.
    pub fn meta_store(&self) -> Arc<dyn MetaStore> {
        self.meta_store.clone()
    }

    /// The collection storage context the server's tasks run on; M1.2 builds
    /// its `CollectionService` on it.
    pub fn collection_context(&self) -> &CollectionContext {
        &self.collection_context
    }

    /// The hot tier, unless `--hot off`.
    pub fn hot_tier(&self) -> Option<&HotTierImpl> {
        self.hot.as_ref()
    }

    /// The collection service behind the native API.
    pub fn collections(&self) -> Arc<CollectionService> {
        self.collections.clone()
    }

    /// The address the Loams Live sync API listens on, when it runs.
    #[cfg(feature = "live")]
    pub fn live_addr(&self) -> Option<SocketAddr> {
        self.live.as_ref().map(|live| live.handle.addr)
    }

    /// Loams Live's subscription counters, when it runs; `missed_invalidations`
    /// is `live_missed_invalidation_total` (R1 plan row T12-8).
    #[cfg(feature = "live")]
    pub fn live_stats(&self) -> Option<loams_live::subs::SubsStats> {
        self.live.as_ref().map(|live| live.handle.stats())
    }

    /// Whether Loams Live's TiKV handle is swept by the metastore's GC loop
    /// (`true`), or Live runs its own GC (`false`: its own cluster GC loop,
    /// or the embedded store's GC), or Live is off (`None`).
    #[cfg(feature = "live")]
    pub fn live_swept_by_metastore_gc(&self) -> Option<bool> {
        #[cfg(feature = "live-tikv")]
        return self
            .live
            .as_ref()
            .map(|live| live.gc.is_none() && live.handle.store().as_tikv().is_some());
        #[cfg(not(feature = "live-tikv"))]
        self.live.as_ref().map(|_| false)
    }

    /// The `tailscale serve` target's address, in tailscale mode.
    pub fn tailscale_addr(&self) -> Option<SocketAddr> {
        self.tailscale.as_ref().map(|t| t.addr)
    }

    /// The connect socket's path, when one is served.
    pub fn connect_socket_path(&self) -> Option<&Path> {
        self.connect_socket.as_ref().map(|c| c.path.as_path())
    }

    /// The address Flight SQL listens on, when it does.
    pub fn flight_sql_addr(&self) -> Option<SocketAddr> {
        self.flight.as_ref().map(|flight| flight.addr)
    }

    /// The MySQL wire listener, when configured.
    #[cfg(feature = "mysql-wire")]
    pub fn mysql_wire_addr(&self) -> Option<SocketAddr> {
        self.mysql_wire.as_ref().map(|mysql| mysql.addr)
    }

    /// The PostgreSQL wire listener, when configured.
    #[cfg(feature = "pgwire")]
    pub fn pg_addr(&self) -> Option<SocketAddr> {
        self.pg.as_ref().map(|pg| pg.addr)
    }

    /// The Loams SQL gate's address, when configured.
    #[cfg(feature = "sqldb")]
    pub fn sqlgate_addr(&self) -> Option<SocketAddr> {
        self.sqlgate.as_ref().map(|gate| gate.addr)
    }

    /// The native stream gRPC listener, when configured.
    #[cfg(feature = "stream-grpc")]
    pub fn stream_grpc_addr(&self) -> Option<SocketAddr> {
        self.stream_grpc.as_ref().map(|stream| stream.addr)
    }

    /// The address the durable execution API listens on, when it does (D1,
    /// feature `durable`).
    pub fn durable_addr(&self) -> Option<SocketAddr> {
        self.durable.addr()
    }

    /// The address the Qdrant REST API listens on, when it does.
    #[cfg(feature = "qdrant")]
    pub fn qdrant_rest_addr(&self) -> Option<SocketAddr> {
        self.qdrant.as_ref().map(|q| q.handle.rest_addr)
    }

    /// The address the Qdrant gRPC API listens on, when it does.
    #[cfg(feature = "qdrant")]
    pub fn qdrant_grpc_addr(&self) -> Option<SocketAddr> {
        self.qdrant.as_ref().map(|q| q.handle.grpc_addr)
    }

    /// The address the Elasticsearch API listens on, when it does.
    #[cfg(feature = "es")]
    pub fn es_addr(&self) -> Option<SocketAddr> {
        self.es.as_ref().map(|es| es.handle.addr)
    }

    /// The server's log writer; M1.2 builds its `CollectionWriter` on it.
    pub fn log_writer(&self) -> &LogWriter {
        &self.writer
    }

    /// Stops Loams Live (ending its sessions), then the Qdrant and Elasticsearch
    /// gateways (waiting up to 10 s for their requests), stops accepting
    /// requests, stops Flight SQL, stops the durable server (D1: before anything
    /// it may call into), stops the collection
    /// service's tails, flushes the writer (buffered appends are
    /// acknowledged), stops the worker (releasing its task leases), stops
    /// the hot tier, closes the collection targets' PK index handles, waits
    /// for in-flight requests (up to 10 s), and shuts the metastore down
    /// (rule 5.4).
    ///
    /// A cluster node first answers `503` on its client routes, releases
    /// its node lease and, as a learner, leaves the membership; its
    /// metastore routes keep serving until the replica stops (Task 11
    /// rule 4).
    pub async fn shutdown(self) -> Result<(), ServerError> {
        // Loams Live's sessions end first; its own GC loop, if any, with it.
        #[cfg(feature = "live")]
        if let Some(live) = self.live {
            live.stop().await;
        }
        // Then the Qdrant gateway, within the HTTP grace period
        // (plan M1.4 Task 2 rule 3).
        shutdown_phase("qdrant");
        // The Elasticsearch gateway stops together with it (plan M1.5 Task
        // 1, row E13): both tokens are cancelled before either is awaited.
        #[cfg(feature = "es")]
        if let Some(es) = &self.es {
            es.stop.cancel();
        }
        #[cfg(feature = "qdrant")]
        if let Some(qdrant) = self.qdrant {
            qdrant.stop.cancel();
            if let Err(err) = qdrant.handle.stop_within(HTTP_GRACE).await {
                tracing::warn!(%err, "the Qdrant gateway stopped with an error");
            }
        }
        #[cfg(feature = "es")]
        if let Some(es) = self.es
            && let Err(err) = es.handle.stop_within(HTTP_GRACE).await
        {
            tracing::warn!(%err, "the Elasticsearch gateway stopped with an error");
        }
        let mut stop_http = Some(self.stop_http);
        shutdown_phase("http");
        match &self.cluster {
            Some(cluster) => {
                cluster.late.close();
                cluster.registry.deregister().await;
                if !cluster.roles.meta {
                    let left = meta_rpc::leave(
                        &cluster.transport,
                        &cluster.seeds,
                        LeaveRequest {
                            node_id: cluster.node_id,
                        },
                        LEAVE_WAIT,
                    )
                    .await;
                    if let Err(err) = left {
                        tracing::warn!(%err, "leaving the metastore membership");
                    }
                }
            }
            None => {
                if let Some(stop) = stop_http.take() {
                    let _ = stop.send(());
                }
            }
        }
        shutdown_phase("flight");
        if let Some(tailscale) = self.tailscale {
            tailscale.stop().await;
        }
        if let Some(connect) = self.connect_socket {
            connect.stop().await;
        }
        if let Some(flight) = self.flight {
            flight.stop().await;
        }
        #[cfg(feature = "mysql-wire")]
        if let Some(mysql) = self.mysql_wire {
            mysql.stop_within(HTTP_GRACE).await;
        }
        #[cfg(feature = "pgwire")]
        if let Some(pg) = self.pg {
            pg.stop_within(HTTP_GRACE).await;
        }
        #[cfg(feature = "stream-grpc")]
        if let Some(stream_grpc) = self.stream_grpc {
            stream_grpc.stop().await;
        }
        #[cfg(feature = "sqldb")]
        if let Some(sqlgate) = self.sqlgate {
            sqlgate.stop().await;
        }
        // D1 (T0-6, X8): after Flight (and, in cluster mode, after
        // `late.close()`), before the collection service: the runtime first,
        // then the server (phases `durable_runtime`, `durable`).
        self.durable.stop().await;
        // Loams Graph closes its graphs (flushing their WAL) before the rest stops.
        #[cfg(feature = "graph")]
        if let Some(graph) = self.graph {
            shutdown_phase("graph");
            graph.stop().await;
        }
        // Loams Git's sequencers answer what they hold and write their last hints.
        #[cfg(feature = "git")]
        if let Some(git) = self.git {
            shutdown_phase("git");
            git.stop().await;
        }
        // Loams Meter's counter owners give their partitions back.
        #[cfg(feature = "meter")]
        if let Some(meter) = self.meter {
            shutdown_phase("meter");
            meter.stop().await;
        }
        shutdown_phase("collections");
        self.collections.shutdown().await;
        shutdown_phase("writer");
        if let Err(err) = self.writer.shutdown().await {
            tracing::warn!(%err, "the final flush failed");
        }
        shutdown_phase("worker");
        if let Some(worker) = self.worker {
            worker.stop().await;
        }
        // The tier stops after the worker and before the metastore (Task 8
        // rule 5).
        shutdown_phase("hot");
        if let Some(tier) = &self.hot {
            tier.shutdown().await;
        }
        self.collection_factory.close().await;
        let mut http = self.http;
        if stop_http.is_none() && tokio::time::timeout(HTTP_GRACE, &mut http).await.is_err() {
            tracing::warn!("in-flight requests did not finish; aborting them");
            http.abort();
        }
        if let Err(err) = self.cache.close().await {
            tracing::warn!(%err, "closing the cache failed");
        }
        shutdown_phase("metastore");
        #[cfg(feature = "tikv")]
        if let Some(gc) = self.tikv_gc {
            gc.stop().await;
        }
        let stopped = match &self.node {
            Some(node) => node.shutdown().await,
            None => Ok(()),
        };
        // A cluster node serves its metastore routes until the replica stops.
        if let Some(stop) = stop_http.take() {
            let _ = stop.send(());
            if tokio::time::timeout(HTTP_GRACE, &mut http).await.is_err() {
                http.abort();
            }
        }
        stopped?;
        Ok(())
    }
}

/// Binds `addr`.
async fn bind(addr: SocketAddr) -> Result<(tokio::net::TcpListener, SocketAddr), ServerError> {
    let bound = async {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let local_addr = listener.local_addr()?;
        Ok::<_, std::io::Error>((listener, local_addr))
    }
    .await;
    bound.map_err(|source| ServerError::Listen { addr, source })
}

#[cfg(unix)]
/// Takes the test harness's bound listener from stdin and validates its address.
fn inherited_listener(
    addr: SocketAddr,
) -> Result<(tokio::net::TcpListener, SocketAddr), ServerError> {
    // The child receives a clone of the parent's bound listener on stdin.
    // Clone it here so the server owns its descriptor independently of stdin.
    let fd = std::io::stdin()
        .as_fd()
        .try_clone_to_owned()
        .map_err(|source| ServerError::Listen { addr, source })?;
    let listener = std::net::TcpListener::from(fd);
    let bound = listener
        .local_addr()
        .map_err(|source| ServerError::Listen { addr, source })?;
    if bound != addr {
        return Err(ServerError::Config(format!(
            "inherited listener is bound to {bound}, not --listen {addr}"
        )));
    }
    listener
        .set_nonblocking(true)
        .map_err(|source| ServerError::Listen { addr, source })?;
    let listener = tokio::net::TcpListener::from_std(listener)
        .map_err(|source| ServerError::Listen { addr, source })?;
    Ok((listener, bound))
}

#[cfg(not(unix))]
/// Rejects the Unix-only listener handoff on other platforms.
fn inherited_listener(
    _addr: SocketAddr,
) -> Result<(tokio::net::TcpListener, SocketAddr), ServerError> {
    Err(ServerError::Config(
        "--listen-stdin requires a Unix socket descriptor".to_string(),
    ))
}

/// `--advertise` as a socket address (E49): an `ip:port` as is, a host name
/// resolved once (its first address).
async fn resolve(advertise: &str) -> Result<SocketAddr, ServerError> {
    if let Ok(addr) = advertise.parse() {
        return Ok(addr);
    }
    tokio::net::lookup_host(advertise)
        .await
        .ok()
        .and_then(|mut addrs| addrs.next())
        .ok_or_else(|| ServerError::Config(format!("--advertise {advertise:?} does not resolve")))
}
