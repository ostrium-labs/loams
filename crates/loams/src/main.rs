//! The `loams` binary.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use std::collections::BTreeMap;

use clap::{Parser, Subcommand};
use loams::{ClusterConfig, MetaBackend, Server, ServerConfig};
use loams_hot::Roles;

#[derive(Debug, Parser)]
#[command(
    name = "loams",
    version,
    about = "Loams: an object-storage-native database"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Background-work tuning, for tests (the crash gate runs everything within
/// seconds). Hidden from `--help`.
#[derive(Debug, Default, clap::Args)]
struct Tuning {
    /// Segment WAL runs once they hold this many bytes.
    #[arg(long, hide = true)]
    segment_min_bytes: Option<u64>,
    /// Segment WAL runs whose newest record is this old.
    #[arg(long, hide = true)]
    segment_max_wal_age_ms: Option<u64>,
    /// How often the worker polls for tasks.
    #[arg(long, hide = true)]
    poll_interval_ms: Option<u64>,
    /// The worker's task lease TTL.
    #[arg(long, hide = true)]
    lease_ttl_ms: Option<u64>,
    /// How often retention runs.
    #[arg(long, hide = true)]
    retention_interval_ms: Option<u64>,
    /// Garbage collection's grace period. Also sets every freshness deadline
    /// to half of it: the segmenter's swap deadline, the link (and
    /// collection) commit delay, the collection index commit delay, the
    /// maintenance commit delay and the hot artifact commit delay.
    #[arg(long, hide = true)]
    gc_grace_ms: Option<u64>,
    /// How often garbage collection runs.
    #[arg(long, hide = true)]
    gc_interval_ms: Option<u64>,
    /// How long a small link batch waits for more records.
    #[arg(long, hide = true)]
    link_batch_interval_ms: Option<u64>,
    /// Most records per link commit.
    #[arg(long, hide = true)]
    link_batch_records: Option<usize>,
    /// Build a metastore snapshot after this many log entries.
    #[arg(long, hide = true)]
    snapshot_every: Option<u64>,
    /// Whether collections' implicit streams are trimmed.
    #[arg(long, hide = true, action = clap::ArgAction::Set)]
    collection_trim: Option<bool>,
    /// Rows before a collection's first vector index is built.
    #[arg(long, hide = true)]
    collection_index_min_rows: Option<u64>,
    /// Unindexed rows before a delta vector index segment is built.
    #[arg(long, hide = true)]
    collection_index_delta_min_rows: Option<u64>,
    /// How long a superseded collection manifest stays readable.
    #[arg(long, hide = true)]
    collection_retention_ms: Option<u64>,
    /// How often an idle collection is checked for index work.
    #[arg(long, hide = true)]
    collection_index_poll_interval_ms: Option<u64>,
    /// Most bytes one collection's tail index holds.
    #[arg(long, hide = true)]
    tail_max_bytes: Option<usize>,
    /// How long strong and at-least-token reads wait for the tail.
    #[arg(long, hide = true)]
    consistency_wait_ms: Option<u64>,
    /// How often the hot tier reconciles (M1.3).
    #[arg(long, hide = true)]
    hot_reconcile_interval_ms: Option<u64>,
    /// How long a stale hot artifact may wait for its rebuild.
    #[arg(long, hide = true)]
    hot_rebuild_max_staleness_ms: Option<u64>,
    /// Inserted rows that make a stale hot artifact due for a rebuild.
    #[arg(long, hide = true)]
    hot_rebuild_min_inserted: Option<u64>,
    /// How often an unchanged hot column is checked for a build.
    #[arg(long, hide = true)]
    hot_build_poll_interval_ms: Option<u64>,
    /// Whether split merges and Lance compaction run.
    #[arg(long, hide = true, value_enum)]
    maintenance: Option<HotSwitch>,
    /// How often an unchanged collection is checked for maintenance.
    #[arg(long, hide = true)]
    merge_poll_interval_ms: Option<u64>,
    /// The merge policy's smallest level, in docs.
    #[arg(long, hide = true)]
    merge_min_level_docs: Option<usize>,
    /// Small Lance fragments before a compaction runs.
    #[arg(long, hide = true)]
    compaction_min_small_fragments: Option<usize>,
    /// Rows per compacted Lance fragment.
    #[arg(long, hide = true)]
    compaction_target_rows: Option<usize>,
}

impl Tuning {
    fn apply(&self, config: &mut ServerConfig) {
        let ms = Duration::from_millis;
        if let Some(v) = self.segment_min_bytes {
            config.segmenter.min_bytes = v;
        }
        if let Some(v) = self.segment_max_wal_age_ms {
            config.segmenter.max_wal_age = ms(v);
        }
        if let Some(v) = self.poll_interval_ms {
            config.worker_poll_interval = ms(v);
        }
        if let Some(v) = self.lease_ttl_ms {
            config.worker_lease_ttl = ms(v);
        }
        if let Some(v) = self.retention_interval_ms {
            config.retention.interval = ms(v);
        }
        if let Some(v) = self.gc_grace_ms {
            config.gc.grace = ms(v);
            // Freshness deadlines must stay strictly below the grace period
            // (ServerConfig::validate).
            config.segmenter.swap_deadline = ms(v / 2);
            config.link.max_commit_delay = ms(v / 2);
            config.collection.max_commit_delay = ms(v / 2);
            config.collection.index_commit_delay = ms(v / 2);
            config.maintenance.commit_delay = ms(v / 2);
            config.hot_build.artifact_commit_delay = ms(v / 2);
        }
        if let Some(v) = self.gc_interval_ms {
            config.gc.interval = ms(v);
        }
        if let Some(v) = self.link_batch_interval_ms {
            config.link.batch_interval = ms(v);
        }
        if let Some(v) = self.link_batch_records {
            config.link.batch_records = v;
        }
        if let Some(v) = self.snapshot_every {
            config.snapshot_every = v;
        }
        if let Some(v) = self.collection_trim {
            config.collection.trim = v;
        }
        if let Some(v) = self.collection_index_min_rows {
            config.collection.index_min_rows = v;
        }
        if let Some(v) = self.collection_index_delta_min_rows {
            config.collection.index_delta_min_rows = v;
        }
        if let Some(v) = self.collection_retention_ms {
            config.collection.time_travel_retention = ms(v);
        }
        if let Some(v) = self.collection_index_poll_interval_ms {
            config.collection.index_poll_interval = ms(v);
        }
        if let Some(v) = self.tail_max_bytes {
            config.query.tail.max_bytes = v;
            // The byte budget stays at most half the tail (Task 15 rule 8;
            // row 15.5).
            let budget = &mut config.query.backpressure.max_unapplied_bytes;
            *budget = (*budget).min((v / 2) as u64);
        }
        if let Some(v) = self.consistency_wait_ms {
            config.query.read.consistency_wait = ms(v);
        }
        if let Some(v) = self.hot_reconcile_interval_ms {
            config.hot.reconcile_interval = ms(v);
        }
        if let Some(v) = self.hot_rebuild_max_staleness_ms {
            config.hot_build.rebuild_max_staleness = ms(v);
        }
        if let Some(v) = self.hot_rebuild_min_inserted {
            config.hot_build.rebuild_min_inserted = v;
        }
        if let Some(v) = self.hot_build_poll_interval_ms {
            config.hot_build.poll_interval = ms(v);
        }
        if let Some(switch) = self.maintenance {
            let on = switch == HotSwitch::On;
            config.maintenance.merge = on;
            config.maintenance.compaction = on;
        }
        if let Some(v) = self.merge_poll_interval_ms {
            config.maintenance.poll_interval = ms(v);
        }
        if let Some(v) = self.merge_min_level_docs {
            config.maintenance.merge_policy.min_level_num_docs = v;
        }
        if let Some(v) = self.compaction_min_small_fragments {
            config.maintenance.compaction_min_small_fragments = v;
        }
        if let Some(v) = self.compaction_target_rows {
            config.maintenance.compaction_target_rows = v;
        }
    }
}

/// `--hot on|off` (and `--maintenance on|off`, `--backpressure on|off`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
enum HotSwitch {
    On,
    Off,
}

/// The surfaces beside the HTTP API, shared by `dev`, `standalone` and
/// `cluster`.
#[derive(Debug, clap::Args)]
struct Native {
    /// Loams Graph's data directory (feature `graph`): graphs live under
    /// <DIR>/graphs/<graph_id>/ [default: <data-dir>/graph for dev; required
    /// for standalone unless --graph-ephemeral or --no-graph].
    #[arg(long, conflicts_with_all = ["graph_ephemeral", "no_graph"])]
    graph_data_dir: Option<PathBuf>,
    /// Keep graphs in memory (development only): a graph is lost on restart,
    /// and fails rather than reopening after an engine panic.
    #[arg(long, conflicts_with = "no_graph")]
    graph_ephemeral: bool,
    /// Serve no Loams Graph API (its RPCs answer feature_not_in_variant).
    #[arg(long)]
    no_graph: bool,
    /// Loams Graph: graph statements that may run at once in this process;
    /// past it a statement is refused with RESOURCE_EXHAUSTED [default: twice
    /// the cores, at most 32; at most 256].
    #[arg(long)]
    graph_statement_slots: Option<usize>,
    /// Loams Graph: the engine's own statement time limit, a backstop (it
    /// does not stop a statement inside one long operator), and the largest
    /// timeout a request may ask for [default: 30000; at most 300000].
    #[arg(long)]
    graph_query_timeout_ms: Option<u64>,
    /// Address of the Arrow Flight SQL listener [default: 127.0.0.1:8082
    /// for dev, 0.0.0.0:8082 for standalone].
    #[arg(long, conflicts_with = "no_flight_sql")]
    flight_sql_listen: Option<SocketAddr>,
    /// Serve no Flight SQL.
    #[arg(long)]
    no_flight_sql: bool,
    /// MySQL wire listener for collection queries (loopback only).
    #[cfg(feature = "mysql-wire")]
    #[arg(long)]
    mysql_listen: Option<SocketAddr>,
    /// Namespace exposed on the MySQL wire listener.
    #[cfg(feature = "mysql-wire")]
    #[arg(long, requires = "mysql_listen")]
    mysql_namespace: Option<String>,
    /// PostgreSQL wire listener for collection queries (loopback only).
    #[cfg(feature = "pgwire")]
    #[arg(long)]
    pg_listen: Option<SocketAddr>,
    /// Namespace exposed on the PostgreSQL wire listener.
    #[cfg(feature = "pgwire")]
    #[arg(long, requires = "pg_listen")]
    pg_namespace: Option<String>,
    /// Native stream gRPC listener for Dapr protocol adapters (loopback only).
    #[cfg(feature = "stream-grpc")]
    #[arg(long)]
    stream_grpc_listen: Option<SocketAddr>,
    /// Loams SQL gate listener (TLS; plaintext only from loopback peers).
    #[cfg(feature = "sqldb")]
    #[arg(long, requires_all = ["sqlgate_tls_cert", "sqlgate_tls_key", "sqlgate_upstream_ca"])]
    sqlgate_listen: Option<SocketAddr>,
    /// The gate's certificate chain (PEM).
    #[cfg(feature = "sqldb")]
    #[arg(long, requires = "sqlgate_listen")]
    sqlgate_tls_cert: Option<std::path::PathBuf>,
    /// The gate's private key (PEM).
    #[cfg(feature = "sqldb")]
    #[arg(long, requires = "sqlgate_listen")]
    sqlgate_tls_key: Option<std::path::PathBuf>,
    /// The CA (PEM) that signs the TiDB pools' certificates.
    #[cfg(feature = "sqldb")]
    #[arg(long, requires = "sqlgate_listen")]
    sqlgate_upstream_ca: Option<std::path::PathBuf>,
    /// When the gate accepts plaintext: never, or loopback (from loopback
    /// peers only) [default: loopback for `loams dev`, never otherwise].
    #[cfg(feature = "sqldb")]
    #[arg(long, requires = "sqlgate_listen")]
    sqlgate_plaintext: Option<loams_sqlgate::server::PlaintextPolicy>,
    /// Address of the Qdrant REST API [default: 127.0.0.1:6333].
    #[arg(long, conflicts_with = "no_qdrant")]
    qdrant_listen: Option<SocketAddr>,
    /// Address of the Qdrant gRPC API [default: 127.0.0.1:6334].
    #[arg(long, conflicts_with = "no_qdrant")]
    qdrant_grpc_listen: Option<SocketAddr>,
    /// The namespace of Qdrant requests without a `Loams-Namespace`
    /// header [default: default].
    #[arg(long, conflicts_with = "no_qdrant")]
    qdrant_namespace: Option<String>,
    /// Serve no Qdrant API.
    #[arg(long)]
    no_qdrant: bool,
    /// Address of the Elasticsearch REST API [default: 127.0.0.1:9200].
    #[arg(long, conflicts_with = "no_es")]
    es_listen: Option<SocketAddr>,
    /// The namespace of Elasticsearch requests without an
    /// `Loams-Namespace` header [default: default].
    #[arg(long, conflicts_with = "no_es")]
    es_namespace: Option<String>,
    /// Serve no Elasticsearch API.
    #[arg(long)]
    no_es: bool,
    /// How long a CloudEvent's `source` + `id` is remembered, so a retry does
    /// not append it again, in seconds [default: 3600, at most 86400].
    #[arg(long)]
    cloudevents_dedup_window: Option<u64>,
    /// Whether this node runs a hot tier, and whether reads use it when a
    /// request does not say (`Loams-Hot`).
    #[arg(long, value_enum, default_value = "on")]
    hot: HotSwitch,
    /// Make every collection's vectors and text hot on this process, without
    /// a catalog change.
    #[arg(long)]
    hot_pin_all: bool,
    /// Local directory of the hot tier [default: <data-dir>/hot].
    #[arg(long)]
    hot_dir: Option<PathBuf>,
    /// Local disk the hot tier may use, in bytes [default: 100 GiB].
    #[arg(long)]
    hot_nvme_bytes: Option<u64>,
    /// Memory the hot tier may use, in bytes [default: 8 GiB].
    #[arg(long)]
    hot_ram_bytes: Option<u64>,
    /// Whether collection writes are refused (429) while a collection's
    /// unapplied data is over its budget.
    #[arg(long, value_enum, default_value = "on")]
    backpressure: HotSwitch,
    /// Unapplied records per collection before writes are refused
    /// [default: 1000000].
    #[arg(long)]
    max_unapplied_records: Option<u64>,
    /// Unapplied bytes per collection before writes are refused, at most
    /// half the tail's bound [default: 128 MiB].
    #[arg(long)]
    max_unapplied_bytes: Option<u64>,
    /// Address of the durable execution API, loopback only (D138)
    /// [default: 127.0.0.1:8001].
    #[arg(long, value_parser = parse_durable_listen, conflicts_with = "no_durable")]
    durable_listen: Option<SocketAddr>,
    /// Serve no durable execution API.
    #[arg(long)]
    no_durable: bool,
    /// Where durable state lives: sqlite:<path>, tikv://<pd-hosts>/<keyspace>,
    /// or legacy mysql://…. Cluster mode needs a shared store.
    #[arg(long, value_parser = DurableStoreParser, conflicts_with = "no_durable")]
    durable_store: Option<DurableStoreArg>,
    /// Deliver durable tasks to http:// and https:// targets (a server-side
    /// request forgery risk: any caller may name a target).
    #[arg(long, conflicts_with = "no_durable")]
    durable_push: bool,
    /// A setting of the embedded durable server, in Resonate's key space
    /// (repeatable; applied in order).
    #[arg(
        long,
        value_name = "KEY=VALUE",
        value_parser = parse_key_value,
        conflicts_with = "no_durable"
    )]
    durable_set: Vec<(String, String)>,
    /// The durable server's clock belongs to the caller (tests).
    #[arg(long, hide = true, conflicts_with = "no_durable")]
    durable_debug: bool,
}

/// `--durable-store`: `sqlite:<path>`, `tikv://<pd-hosts>/<keyspace>`, or
/// `mysql://…`. With the feature a
/// MySQL store is parsed (its TLS mode read) at once; without it the URL is
/// only kept. `Debug` never shows a password (D1 Task 4).
#[derive(Clone, PartialEq, Eq)]
enum DurableStoreArg {
    Sqlite(PathBuf),
    #[cfg(feature = "durable")]
    Tikv(loams_durable::DurableStore),
    #[cfg(not(feature = "durable"))]
    Tikv(String),
    #[cfg(feature = "durable")]
    Mysql(loams_durable::DurableStore),
    #[cfg(not(feature = "durable"))]
    Mysql(String),
}

impl std::fmt::Debug for DurableStoreArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sqlite(path) => f.debug_tuple("Sqlite").field(path).finish(),
            Self::Tikv(store) => f.debug_tuple("Tikv").field(store).finish(),
            #[cfg(feature = "durable")]
            Self::Mysql(store) => f.debug_tuple("Mysql").field(store).finish(),
            // Without the feature the URL is never used: show only its host.
            #[cfg(not(feature = "durable"))]
            Self::Mysql(url) => {
                let host = url.rsplit_once('@').map_or(url.as_str(), |(_, host)| host);
                f.debug_tuple("Mysql")
                    .field(&format!("mysql://…@{host}"))
                    .finish()
            }
        }
    }
}

fn parse_durable_store(text: &str) -> Result<DurableStoreArg, String> {
    if let Some(path) = text.strip_prefix("sqlite:") {
        // sqlite:///abs/path reads as the URL it looks like.
        let path = path.strip_prefix("//").unwrap_or(path);
        if path.is_empty() {
            return Err("sqlite: needs a path, as in sqlite:.loams/durable/default.db".into());
        }
        return Ok(DurableStoreArg::Sqlite(PathBuf::from(path)));
    }
    if text.starts_with("mysql://") {
        // The error names the URL without its password.
        #[cfg(feature = "durable")]
        return loams_durable::DurableStore::mysql(text)
            .map(DurableStoreArg::Mysql)
            .map_err(|err| err.to_string());
        #[cfg(not(feature = "durable"))]
        return Ok(DurableStoreArg::Mysql(text.to_string()));
    }
    if let Some(address) = text.strip_prefix("tikv://") {
        let (hosts, keyspace) = address.split_once('/').ok_or_else(|| {
            "tikv:// needs PD hosts and a keyspace, as in tikv://127.0.0.1:2379/loams_durable"
                .to_string()
        })?;
        let pd: Vec<String> = hosts.split(',').map(str::to_string).collect();
        #[cfg(feature = "durable")]
        return loams_durable::DurableStore::tikv(pd, keyspace)
            .map(DurableStoreArg::Tikv)
            .map_err(|err| err.to_string());
        #[cfg(not(feature = "durable"))]
        {
            if pd.is_empty() || pd.iter().any(|host| host.is_empty()) || keyspace.is_empty() {
                return Err("tikv:// needs nonempty PD hosts and a keyspace".into());
            }
            return Ok(DurableStoreArg::Tikv(address.to_string()));
        }
    }
    Err("expected sqlite:<path>, tikv://<pd-hosts>/<keyspace>, or mysql://…".into())
}

/// `--durable-store`'s value parser. A plain `fn` parser's error would quote
/// the value, and with it a MySQL password; this one says only what is wrong.
#[derive(Clone, Copy, Debug)]
struct DurableStoreParser;

impl clap::builder::TypedValueParser for DurableStoreParser {
    type Value = DurableStoreArg;

    fn parse_ref(
        &self,
        cmd: &clap::Command,
        _arg: Option<&clap::Arg>,
        value: &std::ffi::OsStr,
    ) -> Result<DurableStoreArg, clap::Error> {
        let refuse = |message: String| {
            clap::Error::raw(
                clap::error::ErrorKind::ValueValidation,
                format!("invalid value for '--durable-store': {message}\n"),
            )
            .with_cmd(cmd)
        };
        let text = value
            .to_str()
            .ok_or_else(|| refuse("the value is not UTF-8".into()))?;
        parse_durable_store(text).map_err(refuse)
    }
}

#[cfg(feature = "durable")]
impl DurableStoreArg {
    /// The store the flag names.
    fn to_store(&self) -> loams_durable::DurableStore {
        match self {
            Self::Sqlite(path) => loams_durable::DurableStore::Sqlite { path: path.clone() },
            Self::Tikv(store) => store.clone(),
            Self::Mysql(store) => store.clone(),
        }
    }
}

/// `--durable-listen`: an IP socket address or `localhost:<port>`, refused
/// unless it is loopback (D138).
#[cfg(feature = "durable")]
fn parse_durable_listen(text: &str) -> Result<SocketAddr, String> {
    loams_durable::parse_listen(text).map_err(|err| err.to_string())
}

/// Without the feature the address is only parsed: the flag is ignored.
#[cfg(not(feature = "durable"))]
fn parse_durable_listen(text: &str) -> Result<SocketAddr, String> {
    text.parse().map_err(|err| format!("{err}"))
}

/// `--durable-set key=value`.
fn parse_key_value(text: &str) -> Result<(String, String), String> {
    match text.split_once('=') {
        Some((key, value)) if !key.trim().is_empty() => {
            Ok((key.trim().to_string(), value.to_string()))
        }
        _ => Err(format!("expected key=value, got {text:?}")),
    }
}

impl Native {
    /// Loams Graph's settings (GR1 Task 5). `dev` keeps graphs under `<data-dir>/graph` by
    /// default; `standalone` and `cluster` need `--graph-data-dir` (or `--graph-ephemeral`, or
    /// `--no-graph`), and the server refuses to start without one.
    fn apply_graph(&self, config: &mut ServerConfig, dev: bool) {
        #[cfg(feature = "graph")]
        {
            config.graph.enabled = !self.no_graph;
            config.graph.ephemeral = self.graph_ephemeral;
            if let Some(slots) = self.graph_statement_slots {
                config.graph.statement_slots = slots;
            }
            if let Some(ms) = self.graph_query_timeout_ms {
                config.graph.query_timeout = Duration::from_millis(ms);
            }
            if let Some(dir) = &self.graph_data_dir {
                config.graph.data_dir = Some(dir.clone());
            } else if !dev || self.graph_ephemeral {
                config.graph.data_dir = None;
            }
        }
        #[cfg(not(feature = "graph"))]
        {
            let _ = (config, dev);
            if self.graph_data_dir.is_some()
                || self.graph_ephemeral
                || self.graph_statement_slots.is_some()
                || self.graph_query_timeout_ms.is_some()
            {
                tracing::warn!("this build has no Loams Graph (the graph feature is off)");
            }
        }
    }

    fn apply(&self, config: &mut ServerConfig, default_flight: SocketAddr) {
        config.flight_sql = if self.no_flight_sql {
            None
        } else {
            Some(self.flight_sql_listen.unwrap_or(default_flight))
        };
        #[cfg(feature = "mysql-wire")]
        {
            config.mysql_wire = self.mysql_listen.map(|addr| {
                loams::mysql_wire::MysqlConfig::new(
                    addr,
                    self.mysql_namespace
                        .clone()
                        .unwrap_or_else(|| "default".into()),
                )
            });
        }
        #[cfg(feature = "pgwire")]
        {
            config.pg = self.pg_listen.map(|addr| {
                loams::pg::PgConfig::new(
                    addr,
                    self.pg_namespace
                        .clone()
                        .unwrap_or_else(|| "default".into()),
                )
            });
        }
        #[cfg(feature = "stream-grpc")]
        {
            config.stream_grpc = self.stream_grpc_listen;
        }
        #[cfg(feature = "sqldb")]
        {
            config.sqlgate = match (
                self.sqlgate_listen,
                &self.sqlgate_tls_cert,
                &self.sqlgate_tls_key,
                &self.sqlgate_upstream_ca,
            ) {
                (Some(listen), Some(cert), Some(key), Some(ca)) => {
                    Some(loams::sqlgate::SqlgateConfig {
                        listen,
                        tls_cert: cert.clone(),
                        tls_key: key.clone(),
                        upstream_ca: ca.clone(),
                        plaintext: self
                            .sqlgate_plaintext
                            .unwrap_or(loams_sqlgate::server::PlaintextPolicy::Never),
                    })
                }
                _ => None,
            };
        }
        self.apply_qdrant(config);
        self.apply_durable(config);
        self.apply_es(config);
        let hot = self.hot == HotSwitch::On;
        config.query.hot_default = hot;
        config.hot.enabled = hot;
        config.hot.pin_all = self.hot_pin_all;
        if let Some(seconds) = self.cloudevents_dedup_window {
            config.cloudevents.dedup_window = std::time::Duration::from_secs(seconds);
        }
        if let Some(dir) = &self.hot_dir {
            config.hot.dir = dir.clone();
        }
        if let Some(v) = self.hot_nvme_bytes {
            config.hot.nvme_bytes = v;
        }
        if let Some(v) = self.hot_ram_bytes {
            config.hot.ram_bytes = v;
        }
        let backpressure = &mut config.query.backpressure;
        backpressure.enabled = self.backpressure == HotSwitch::On;
        if let Some(v) = self.max_unapplied_records {
            backpressure.max_unapplied_records = v;
        }
        if let Some(v) = self.max_unapplied_bytes {
            backpressure.max_unapplied_bytes = v;
        }
    }

    /// The Qdrant gateway, unless `--no-qdrant` (plan M1.4 Task 2, E12).
    #[cfg(feature = "qdrant")]
    fn apply_qdrant(&self, config: &mut ServerConfig) {
        config.qdrant = (!self.no_qdrant).then(|| {
            let mut qdrant = loams_qdrant::QdrantConfig::default();
            if let Some(addr) = self.qdrant_listen {
                qdrant.rest_listen = addr;
            }
            if let Some(addr) = self.qdrant_grpc_listen {
                qdrant.grpc_listen = addr;
            }
            if let Some(ns) = &self.qdrant_namespace {
                qdrant.namespace = ns.clone();
            }
            qdrant
        });
    }

    #[cfg(not(feature = "qdrant"))]
    fn apply_qdrant(&self, _config: &mut ServerConfig) {
        if self.qdrant_listen.is_some() || self.qdrant_grpc_listen.is_some() {
            tracing::warn!("this build has no Qdrant API (the qdrant feature is off)");
        }
    }

    /// Whether any `--durable-*` flag is given (`--no-durable` is not one).
    fn any_durable_flag(&self) -> bool {
        self.durable_listen.is_some()
            || self.durable_store.is_some()
            || self.durable_push
            || !self.durable_set.is_empty()
            || self.durable_debug
    }

    /// The embedded durable server, unless `--no-durable` (D1 Task 3). `dev`
    /// and `standalone` default to a SQLite store in the data directory; a
    /// cluster node has no default store and serves durable execution only
    /// with `--durable-store` (`ServerConfig::validate` refuses `sqlite:`
    /// there).
    #[cfg(feature = "durable")]
    fn apply_durable(&self, config: &mut ServerConfig) {
        use loams_durable::{DurableConfig, DurableStore};
        if self.no_durable {
            config.durable = None;
            return;
        }
        let store = match (&self.durable_store, &config.cluster) {
            (Some(store), _) => store.to_store(),
            (None, None) => DurableStore::Sqlite {
                path: config.data_dir.join("durable").join("default.db"),
            },
            (None, Some(_)) => {
                if self.any_durable_flag() {
                    tracing::warn!(
                        "the --durable-* flags are ignored: loams cluster serves durable \
                         execution only with --durable-store tikv://… or mysql://…"
                    );
                }
                config.durable = None;
                return;
            }
        };
        let mut durable = DurableConfig::new(store);
        if let Some(addr) = self.durable_listen {
            durable.listen = addr;
        }
        durable.push = self.durable_push;
        durable.debug = self.durable_debug;
        durable.overrides = self.durable_set.clone();
        config.durable = Some(durable);
    }

    #[cfg(not(feature = "durable"))]
    fn apply_durable(&self, _config: &mut ServerConfig) {
        if self.any_durable_flag() {
            tracing::warn!("this build has no durable execution (the durable feature is off)");
        }
    }

    /// The Elasticsearch gateway, unless `--no-es` (plan M1.5 Task 1, row
    /// E13).
    #[cfg(feature = "es")]
    fn apply_es(&self, config: &mut ServerConfig) {
        config.es = (!self.no_es).then(|| {
            let mut es = loams_es::EsConfig::default();
            if let Some(addr) = self.es_listen {
                es.listen = addr;
            }
            if let Some(ns) = &self.es_namespace {
                es.namespace = ns.clone();
            }
            es
        });
    }

    #[cfg(not(feature = "es"))]
    fn apply_es(&self, _config: &mut ServerConfig) {
        if self.es_listen.is_some() {
            tracing::warn!("this build has no Elasticsearch API (the es feature is off)");
        }
    }
}

/// Loams Live (R1 plan Task 12, feature `live`), on `dev` and `standalone`.
#[cfg(feature = "live")]
#[derive(Debug, clap::Args)]
struct LiveArgs {
    /// Address of the Loams Live sync API; loopback only (127.0.0.0/8,
    /// ::1, localhost), since the Live API has no authentication in R1
    /// (D111).
    #[arg(long, default_value = "127.0.0.1:7710", value_parser = parse_live_listen)]
    live_listen: SocketAddr,
    // The help names `tikv://` only in a build that has it: the desktop app
    // reads `dev --help` to tell the two apart (row T23-9).
    #[cfg_attr(
        feature = "live-tikv",
        doc = "Where Live keeps its data: `embedded` (a store under <data-dir>/live/) or \
               `tikv://<pd>[,<pd>]/<keyspace>` (the keyspace defaults to loams_live_<app>) \
               [default: embedded]."
    )]
    #[cfg_attr(
        not(feature = "live-tikv"),
        doc = "Where Live keeps its data: `embedded`, a store under <data-dir>/live/ (this \
               build has no other) [default: embedded]."
    )]
    #[arg(long, value_parser = parse_live_store, conflicts_with_all = ["live_pd", "live_keyspace"])]
    live_store: Option<LiveStore>,
    /// Deprecated: use --live-store tikv://<pd>/<keyspace>. PD endpoints of
    /// a Live TiKV cluster, comma-separated. Hidden, and refused, in a build
    /// without the live-tikv feature.
    #[arg(
        long,
        value_delimiter = ',',
        value_parser = parse_live_tikv_alias,
        hide = !cfg!(feature = "live-tikv")
    )]
    live_pd: Vec<String>,
    /// Deprecated: use --live-store tikv://<pd>/<keyspace>. The Live app's
    /// TiKV keyspace [default: loams_live_<app>]. Hidden, and refused, in a
    /// build without the live-tikv feature.
    #[arg(long, value_parser = parse_live_tikv_alias, hide = !cfg!(feature = "live-tikv"))]
    live_keyspace: Option<String>,
    /// The Live app.
    #[arg(long, default_value = "dev", value_parser = parse_live_app)]
    live_app: String,
    /// How far behind a fresh timestamp each subscription tick reads, in
    /// milliseconds (R1 plan row T12-1).
    #[arg(long, default_value_t = 50)]
    live_tick_read_lag_ms: u64,
    /// Where Live functions run: in_process (trusted, single-tenant code)
    /// or isolated (sandboxed worker processes; Linux only).
    #[arg(long, value_enum, default_value_t = LiveIsolation::InProcess)]
    live_isolation: LiveIsolation,
    /// Whose Live code this node serves: single, or multi (which needs
    /// --live-isolation isolated).
    #[arg(long, value_enum, default_value_t = LiveTenancy::Single)]
    live_tenancy: LiveTenancy,
    /// Serve no Loams Live API.
    #[arg(
        long,
        conflicts_with_all = ["live_listen", "live_store", "live_pd", "live_keyspace", "live_app"]
    )]
    no_live: bool,
}

/// `--live-isolation` (design §45 §3.1, `[live] isolation`).
#[cfg(feature = "live")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum LiveIsolation {
    #[value(name = "in_process")]
    InProcess,
    #[value(name = "isolated")]
    Isolated,
}

/// `--live-tenancy` (design §45 §3.1, `[live] tenancy`).
#[cfg(feature = "live")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum LiveTenancy {
    #[value(name = "single")]
    Single,
    #[value(name = "multi")]
    Multi,
}

/// What `--live-store` names (LV1 plan Task 23).
#[cfg(feature = "live")]
#[derive(Debug, Clone, PartialEq, Eq)]
enum LiveStore {
    /// The embedded store under `<data_dir>/live/`.
    Embedded,
    /// A TiKV keyspace (`None`: `loams_live_<app>`).
    #[cfg(feature = "live-tikv")]
    Tikv {
        pd: Vec<String>,
        keyspace: Option<String>,
    },
}

/// The PD the deprecated `--live-keyspace` alone implies: the dev
/// playground's.
#[cfg(feature = "live-tikv")]
const LIVE_PD_DEFAULT: &str = "127.0.0.1:19379";

/// `--live-pd` and `--live-keyspace`: they select Live on TiKV, so a build
/// without the live-tikv feature refuses them rather than running Live on
/// the embedded store a user did not ask for (row T23-7).
#[cfg(feature = "live")]
fn parse_live_tikv_alias(value: &str) -> Result<String, String> {
    #[cfg(feature = "live-tikv")]
    {
        Ok(value.to_string())
    }
    #[cfg(not(feature = "live-tikv"))]
    {
        let _ = value;
        Err(
            "--live-pd and --live-keyspace select Live on TiKV and need a build with the \
             live-tikv feature; this build runs Live on the embedded store (drop the flag, \
             or build with --features live-tikv)"
                .to_string(),
        )
    }
}

/// `--live-store`: `embedded` or `tikv://<pd>[,<pd>][/<keyspace>]`; the
/// latter only in a build with the live-tikv feature.
#[cfg(feature = "live")]
fn parse_live_store(value: &str) -> Result<LiveStore, String> {
    if value == "embedded" {
        return Ok(LiveStore::Embedded);
    }
    let Some(rest) = value.strip_prefix("tikv://") else {
        return Err(format!(
            "--live-store {value:?}: expected embedded or tikv://<pd>[,<pd>]/<keyspace>"
        ));
    };
    #[cfg(not(feature = "live-tikv"))]
    {
        let _ = rest;
        Err("--live-store tikv:// needs a build with the live-tikv feature".to_string())
    }
    #[cfg(feature = "live-tikv")]
    {
        let (hosts, keyspace) = match rest.split_once('/') {
            Some((hosts, keyspace)) => (hosts, Some(keyspace)),
            None => (rest, None),
        };
        let pd: Vec<String> = hosts
            .split(',')
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .map(str::to_string)
            .collect();
        if pd.is_empty() {
            return Err(format!("--live-store {value:?}: names no PD endpoint"));
        }
        if keyspace.is_some_and(|k| k.contains('/')) {
            return Err(format!(
                "--live-store {value:?}: a keyspace has no '/'; expected \
                 tikv://<pd>[,<pd>]/<keyspace>"
            ));
        }
        Ok(LiveStore::Tikv {
            pd,
            keyspace: keyspace.filter(|k| !k.is_empty()).map(str::to_string),
        })
    }
}

#[cfg(feature = "live")]
impl LiveArgs {
    fn apply(&self, config: &mut ServerConfig) {
        if self.no_live {
            config.live = None;
            return;
        }
        let store = self.store(&config.data_dir);
        let mut live = loams_live::LiveConfig::with_store(&self.live_app, store);
        live.listen = self.live_listen;
        live.subs.tick_read_lag = Duration::from_millis(self.live_tick_read_lag_ms);
        live.isolation = match self.live_isolation {
            LiveIsolation::InProcess => loams_live::Isolation::InProcess,
            LiveIsolation::Isolated => loams_live::Isolation::Isolated,
        };
        live.tenancy = match self.live_tenancy {
            LiveTenancy::Single => loams_live::Tenancy::Single,
            LiveTenancy::Multi => loams_live::Tenancy::Multi,
        };
        config.live = Some(live);
    }

    /// The store the flags name: `--live-store`, else the deprecated
    /// `--live-pd`/`--live-keyspace` (TiKV; refused at parsing without the
    /// live-tikv feature, row T23-7), else the embedded store.
    fn store(&self, data_dir: &std::path::Path) -> loams_live::StoreConfig {
        let embedded = || {
            loams_live::StoreConfig::Embedded(loams_live::EmbeddedConfig::new(
                loams_live::store_path(data_dir),
                loams_live::keyspace_of(&self.live_app),
            ))
        };
        match &self.live_store {
            Some(LiveStore::Embedded) => embedded(),
            #[cfg(feature = "live-tikv")]
            Some(LiveStore::Tikv { pd, keyspace }) => {
                loams_live::StoreConfig::Tikv(loams_tikv::TikvConfig::new(
                    pd.clone(),
                    keyspace
                        .clone()
                        .unwrap_or_else(|| loams_live::keyspace_of(&self.live_app)),
                ))
            }
            #[cfg(feature = "live-tikv")]
            None if !self.live_pd.is_empty() || self.live_keyspace.is_some() => {
                eprintln!(
                    "loams: warning: --live-pd and --live-keyspace are deprecated; use \
                     --live-store tikv://<pd>[,<pd>]/<keyspace>"
                );
                let pd = if self.live_pd.is_empty() {
                    vec![LIVE_PD_DEFAULT.to_string()]
                } else {
                    self.live_pd.clone()
                };
                let keyspace = self
                    .live_keyspace
                    .clone()
                    .unwrap_or_else(|| loams_live::keyspace_of(&self.live_app));
                loams_live::StoreConfig::Tikv(loams_tikv::TikvConfig::new(pd, keyspace))
            }
            None => embedded(),
        }
    }
}

/// `--live-app`: a Live app name (the catalog's name rules).
#[cfg(feature = "live")]
fn parse_live_app(value: &str) -> Result<String, String> {
    loams_live::catalog::check_name("app", value)
        .map(|()| value.to_string())
        .map_err(|err| format!("--live-app {value:?}: {err}"))
}

/// `--live-listen`: an `ip:port`, or `localhost:<port>` (127.0.0.1). The
/// loopback check runs at startup, with the error of design §20 §7.1.
#[cfg(feature = "live")]
fn parse_live_listen(value: &str) -> Result<SocketAddr, String> {
    if let Some(port) = value.strip_prefix("localhost:") {
        let port: u16 = port
            .parse()
            .map_err(|_| format!("--live-listen {value:?}: the port is not a number"))?;
        return Ok(SocketAddr::from(([127, 0, 0, 1], port)));
    }
    value
        .parse()
        .map_err(|_| format!("--live-listen {value:?}: expected ip:port or localhost:port"))
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run everything in one process, with data in a local directory.
    Dev {
        /// Holds the metastore and the local bucket.
        #[arg(long, default_value = ".loams")]
        data_dir: PathBuf,
        /// Address of the HTTP API.
        #[arg(long, default_value = "127.0.0.1:8080")]
        listen: SocketAddr,
        /// How long appends are buffered before a WAL flush.
        #[arg(long)]
        flush_interval_ms: Option<u64>,
        /// The metastore: tikv://<pd-host:port>[,<pd…>]/<keyspace> runs it on
        /// TiKV [default: the embedded store in --data-dir].
        #[arg(long, value_parser = MetaBackend::parse)]
        meta: Option<MetaBackend>,
        #[command(flatten)]
        native: Native,
        #[cfg(feature = "live")]
        #[command(flatten)]
        live: LiveArgs,
        #[command(flatten)]
        tuning: Box<Tuning>,
    },
    /// Run everything in one process, with data in an object-store bucket.
    Standalone {
        /// Object store URL, such as s3://bucket/prefix, gs://bucket or file:///dir.
        #[arg(long)]
        bucket: String,
        /// Holds the metastore's local database.
        #[arg(long, default_value = ".loams")]
        data_dir: PathBuf,
        /// Address of the HTTP API.
        #[arg(long, default_value = "127.0.0.1:8080")]
        listen: SocketAddr,
        /// The metastore: tikv://<pd-host:port>[,<pd…>]/<keyspace> runs it on
        /// TiKV [default: the embedded store in --data-dir].
        #[arg(long, value_parser = MetaBackend::parse)]
        meta: Option<MetaBackend>,
        #[command(flatten)]
        native: Native,
        #[cfg(feature = "live")]
        #[command(flatten)]
        live: LiveArgs,
    },
    /// Run one node of a cluster: the roles given, a metastore replica over
    /// HTTP, and data in an object-store bucket. `--listen` must be on a
    /// private network: the internal routes are unauthenticated.
    Cluster {
        /// This node's id (unique in the cluster).
        #[arg(long)]
        node_id: u64,
        /// A comma-separated subset of meta,log,query,worker,gateway.
        #[arg(long, value_parser = parse_roles)]
        roles: Roles,
        /// Address of this node's HTTP listener (API, internal and metastore routes).
        #[arg(long)]
        listen: SocketAddr,
        /// Use the listener inherited on stdin (cluster test harness only).
        #[arg(long, hide = true)]
        listen_stdin: bool,
        /// The ip:port other nodes reach this node at [default: --listen].
        #[arg(long)]
        advertise: Option<String>,
        /// The meta nodes: id=host:port,…
        #[arg(long, value_parser = loams::cluster::parse_peers)]
        peers: BTreeMap<u64, String>,
        /// Object store URL, such as s3://bucket/prefix, gs://bucket or file:///dir.
        #[arg(long)]
        bucket: String,
        /// Holds the metastore replica's local database and the hot tier.
        #[arg(long, default_value = ".loams")]
        data_dir: PathBuf,
        /// This node's zone; owners of a collection spread across zones.
        #[arg(long, default_value = "")]
        zone: String,
        /// Owners per collection.
        #[arg(long, default_value_t = 1)]
        replication: usize,
        /// How long appends are buffered before a WAL flush.
        #[arg(long, hide = true)]
        flush_interval_ms: Option<u64>,
        /// The node registry's lease TTL.
        #[arg(long, hide = true)]
        registry_ttl_ms: Option<u64>,
        /// How long a learner's node lease may be expired before the
        /// learner is removed from the metastore.
        #[arg(long, hide = true)]
        learner_expiry_ms: Option<u64>,
        /// How often the learner eviction task runs.
        #[arg(long, hide = true)]
        membership_interval_ms: Option<u64>,
        #[command(flatten)]
        native: Native,
        #[command(flatten)]
        tuning: Box<Tuning>,
    },
    /// Warm a collection on the node that owns it (`POST …/warm`).
    Warm {
        /// `<namespace>/<collection>`.
        target: String,
        /// The server to ask.
        #[arg(long, default_value = "http://127.0.0.1:8080")]
        server: String,
    },
    /// Durable execution's store (D1).
    Durable {
        #[command(subcommand)]
        command: DurableCommand,
    },
    /// One isolated Loams Live function worker (LV1 plan Task 5): started
    /// by the server with piped stdio, never by hand. It speaks
    /// `loams.live.worker.v1` frames on stdin and stdout and sandboxes
    /// itself before it runs any function code.
    #[cfg(feature = "live")]
    #[command(hide = true)]
    LiveWorker,
}

/// `loams durable …`: commands that start no server.
#[derive(Debug, Subcommand)]
enum DurableCommand {
    /// Create or update the durable store's schema (Resonate's migrations),
    /// then exit. `loams standalone` and `cluster` never migrate a MySQL
    /// store: run this once per database, and again after an upgrade.
    Migrate {
        /// The store: mysql://user:pass@host:port/db[?ssl-mode=required|disabled|
        /// verify_ca|verify_identity][&ssl-ca=<path>] (or sqlite:<path>, which
        /// `loams dev` also migrates itself).
        #[arg(long, value_parser = DurableStoreParser)]
        durable_store: DurableStoreArg,
    },
}

/// Flight SQL's default address for `loams dev`.
const DEV_FLIGHT_SQL: SocketAddr = SocketAddr::V4(std::net::SocketAddrV4::new(
    std::net::Ipv4Addr::LOCALHOST,
    8082,
));
/// Flight SQL's default address for `loams standalone`.
const STANDALONE_FLIGHT_SQL: SocketAddr = SocketAddr::V4(std::net::SocketAddrV4::new(
    std::net::Ipv4Addr::UNSPECIFIED,
    8082,
));

fn parse_roles(s: &str) -> Result<Roles, String> {
    Roles::parse(s).map_err(|err| err.to_string())
}

fn config(command: Command) -> ServerConfig {
    match command {
        Command::Cluster {
            node_id,
            roles,
            listen,
            listen_stdin,
            advertise,
            peers,
            bucket,
            data_dir,
            zone,
            replication,
            flush_interval_ms,
            registry_ttl_ms,
            learner_expiry_ms,
            membership_interval_ms,
            native,
            tuning,
        } => {
            let ms = Duration::from_millis;
            let mut config = ServerConfig::new(data_dir);
            config.listen = listen;
            config.bucket = Some(bucket);
            if let Some(v) = flush_interval_ms {
                config.log.flush_interval = ms(v);
            }
            let advertise = advertise.unwrap_or_else(|| listen.to_string());
            let mut cluster = ClusterConfig::new(node_id, roles, advertise, peers);
            cluster.listen_stdin = listen_stdin;
            cluster.zone = zone;
            cluster.replication = replication;
            if let Some(v) = registry_ttl_ms {
                // Renew three times per TTL; refresh at least that often.
                cluster.registry.lease_ttl = ms(v);
                cluster.registry.renew_every = ms((v / 3).max(1));
                cluster.registry.refresh_every = ms((v / 4).clamp(1, 1000));
            }
            if let Some(v) = learner_expiry_ms {
                cluster.learner_expiry = ms(v);
            }
            if let Some(v) = membership_interval_ms {
                cluster.membership_interval = ms(v);
            }
            config.cluster = Some(cluster);
            // After `cluster`: a cluster node has no default durable store.
            native.apply(&mut config, STANDALONE_FLIGHT_SQL);
            native.apply_graph(&mut config, false);
            tuning.apply(&mut config);
            config
        }
        Command::Warm { .. } => unreachable!("loams warm starts no server"),
        Command::Durable { .. } => unreachable!("loams durable starts no server"),
        #[cfg(feature = "live")]
        Command::LiveWorker => unreachable!("loams live-worker starts no server"),
        Command::Dev {
            data_dir,
            listen,
            flush_interval_ms,
            meta,
            native,
            #[cfg(feature = "live")]
            live,
            tuning,
        } => {
            let mut config = ServerConfig::new(data_dir);
            config.listen = listen;
            config.meta = meta.unwrap_or_default();
            #[cfg(feature = "live")]
            live.apply(&mut config);
            if let Some(ms) = flush_interval_ms {
                config.log.flush_interval = Duration::from_millis(ms);
            }
            #[cfg(feature = "sqldb")]
            let dev_plaintext = native.sqlgate_plaintext.is_none();
            native.apply(&mut config, DEV_FLIGHT_SQL);
            native.apply_graph(&mut config, true);
            tuning.apply(&mut config);
            // The desktop's gate takes plaintext from loopback peers unless
            // told otherwise; a gateway never does (fix round 1, M6).
            #[cfg(feature = "sqldb")]
            if let Some(gate) = config.sqlgate.as_mut().filter(|_| dev_plaintext) {
                gate.plaintext = loams_sqlgate::server::PlaintextPolicy::LoopbackOnly;
            }
            // Q603's proposed default: `loams dev` publishes the schema of
            // the Connect API on the main port (design §44 §4), a production
            // deployment does not unless it asks.
            config.reflection = true;
            config
        }
        Command::Standalone {
            bucket,
            data_dir,
            listen,
            meta,
            native,
            #[cfg(feature = "live")]
            live,
        } => {
            let mut config = ServerConfig::new(data_dir);
            config.listen = listen;
            config.meta = meta.unwrap_or_default();
            #[cfg(feature = "live")]
            live.apply(&mut config);
            config.bucket = Some(bucket);
            native.apply(&mut config, STANDALONE_FLIGHT_SQL);
            native.apply_graph(&mut config, false);
            config
        }
    }
}

/// Resolves on SIGINT or SIGTERM.
async fn shutdown_signal() {
    let interrupt = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = interrupt => {}
                    _ = terminate.recv() => {}
                }
            }
            Err(_) => {
                let _ = interrupt.await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = interrupt.await;
    }
}

/// The crash gate's kill -9 proxy (M0.4 plan ruling 2): with the
/// `failpoints` feature, `LOAMS_FAILPOINTS="name[,name…]"` arms each named
/// failpoint to call `std::process::abort()` (no destructors, no flush) on
/// its `LOAMS_FAILPOINT_HIT`-th hit (default 1).
#[cfg(feature = "failpoints")]
fn arm_failpoints() -> Result<(), String> {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    let Ok(names) = std::env::var("LOAMS_FAILPOINTS") else {
        return Ok(());
    };
    let hit: u64 = match std::env::var("LOAMS_FAILPOINT_HIT") {
        Ok(n) => n
            .parse()
            .map_err(|_| format!("LOAMS_FAILPOINT_HIT must be a number, got {n:?}"))?,
        Err(_) => 1,
    };
    for name in names.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        let hits = Arc::new(AtomicU64::new(0));
        let point = name.to_string();
        fail::cfg_callback(name, move || {
            if hits.fetch_add(1, Ordering::SeqCst) + 1 == hit {
                eprintln!("loams: failpoint {point} hit {hit} times; aborting");
                std::process::abort();
            }
        })?;
    }
    Ok(())
}

#[cfg(not(feature = "failpoints"))]
fn arm_failpoints() -> Result<(), String> {
    if std::env::var_os("LOAMS_FAILPOINTS").is_some() {
        return Err("LOAMS_FAILPOINTS is set, but this build has no failpoints \
                    (build with --features failpoints)"
            .to_string());
    }
    Ok(())
}

/// `loams warm <namespace>/<collection>`: POSTs the warm request, prints
/// the response body, and exits 0 on a 2xx status, else 1 with the error
/// message on stderr.
async fn warm(target: &str, server: &str) -> ExitCode {
    let Some((ns, collection)) = target.split_once('/') else {
        eprintln!("loams: expected <namespace>/<collection>, got {target:?}");
        return ExitCode::FAILURE;
    };
    let url = format!(
        "{}/v1/namespaces/{ns}/collections/{collection}/warm",
        server.trim_end_matches('/')
    );
    let response = match reqwest::Client::new()
        .post(&url)
        .json(&serde_json::json!({}))
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => {
            eprintln!("loams: {url}: {err}");
            return ExitCode::FAILURE;
        }
    };
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if status.is_success() {
        println!("{body}");
        return ExitCode::SUCCESS;
    }
    let message = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|value| value["message"].as_str().map(str::to_string))
        .unwrap_or(body);
    eprintln!("loams: {status}: {message}");
    ExitCode::FAILURE
}

/// `loams durable migrate`: runs Resonate's migrations on the store once and
/// exits 0, or prints the error and exits 1 (D1 Task 4).
#[cfg(feature = "durable")]
async fn durable(command: &DurableCommand) -> ExitCode {
    match command {
        DurableCommand::Migrate { durable_store } => {
            let store = durable_store.to_store();
            let shown = store.to_string();
            match loams_durable::DurableServer::migrate(store).await {
                Ok(()) => {
                    println!("loams durable: {shown} is migrated");
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("loams: {err}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

#[cfg(not(feature = "durable"))]
async fn durable(_command: &DurableCommand) -> ExitCode {
    eprintln!("loams: this build has no durable execution (the durable feature is off)");
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    // LV1 plan Task 5: the worker runs before an async runtime or a log
    // subscriber exists, so it is single-threaded when it sandboxes itself.
    #[cfg(feature = "live")]
    if let Command::LiveWorker = cli.command {
        return loams_live_js::worker_main();
    }
    serve(cli)
}

#[tokio::main]
async fn serve(cli: Cli) -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,openraft=warn")),
        )
        .init();
    if let Command::Warm { target, server } = &cli.command {
        return warm(target, server).await;
    }
    if let Command::Durable { command } = &cli.command {
        return durable(command).await;
    }
    if let Err(err) = arm_failpoints() {
        eprintln!("loams: {err}");
        return ExitCode::FAILURE;
    }
    let server = match Server::start(config(cli.command)).await {
        Ok(server) => server,
        Err(err) => {
            eprintln!("loams: {err}");
            return ExitCode::FAILURE;
        }
    };
    // Plan M1.4 Task 2 rule 2 and D1 T0-7: before the HTTP line, which
    // harnesses wait for.
    if let Some(addr) = server.durable_addr() {
        println!("loams durable listening on http://{addr}");
    }
    #[cfg(feature = "qdrant")]
    if let (Some(rest), Some(grpc)) = (server.qdrant_rest_addr(), server.qdrant_grpc_addr()) {
        println!("loams qdrant REST listening on http://{rest}");
        println!("loams qdrant gRPC listening on grpc://{grpc}");
    }
    // Plan M1.5 Task 1 (row E13): also before the HTTP line.
    #[cfg(feature = "es")]
    if let Some(addr) = server.es_addr() {
        println!("loams es listening on http://{addr}");
    }
    // R1 plan Task 12 semantics 7: before the HTTP line.
    #[cfg(feature = "live")]
    if let Some(addr) = server.live_addr() {
        println!("loams live listening on http://{addr}");
    }
    println!("loams listening on http://{}", server.local_addr());
    // M1.6 W14, M1.7 A4: printed once the listener is bound.
    if let Some(addr) = server.flight_sql_addr() {
        println!("loams flight sql listening on grpc://{addr}");
    }
    #[cfg(feature = "mysql-wire")]
    if let Some(addr) = server.mysql_wire_addr() {
        println!("loams MySQL wire listening on mysql://{addr}");
    }
    #[cfg(feature = "pgwire")]
    if let Some(addr) = server.pg_addr() {
        println!("loams PostgreSQL wire listening on postgres://{addr}");
    }
    #[cfg(feature = "stream-grpc")]
    if let Some(addr) = server.stream_grpc_addr() {
        println!("loams stream gRPC listening on grpc://{addr}");
    }
    #[cfg(feature = "sqldb")]
    if let Some(addr) = server.sqlgate_addr() {
        println!("loams SQL gate listening on mysql://{addr}");
    }
    shutdown_signal().await;
    tracing::info!("shutting down");
    match server.shutdown().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("loams: shutdown failed: {err}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev_config(args: &[&str]) -> ServerConfig {
        let cli = Cli::try_parse_from(["loams", "dev"].iter().chain(args)).expect("parse");
        config(cli.command)
    }

    /// GR1 Task 5: `loams dev` keeps graphs under `<data-dir>/graph`; `standalone` has no graph
    /// data dir unless one is given (and the server then refuses to start), and
    /// `--graph-ephemeral` / `--no-graph` say so explicitly.
    #[cfg(feature = "graph")]
    #[test]
    fn graph_data_dir_defaults_per_command() {
        let dev = dev_config(&["--data-dir", "/d"]);
        assert_eq!(
            dev.graph.data_dir,
            Some(std::path::PathBuf::from("/d/graph"))
        );
        assert!(dev.graph.enabled && !dev.graph.ephemeral);
        let parse = |args: &[&str]| {
            let cli = Cli::try_parse_from(
                ["loams", "standalone", "--bucket", "file:///b"]
                    .iter()
                    .chain(args),
            )
            .expect("parse");
            config(cli.command)
        };
        let standalone = parse(&[]);
        assert_eq!(standalone.graph.data_dir, None);
        assert!(
            standalone.validate().is_err(),
            "refuses without a graph data dir"
        );
        let given = parse(&["--graph-data-dir", "/g"]);
        assert_eq!(given.graph.data_dir, Some(std::path::PathBuf::from("/g")));
        let ephemeral = parse(&["--graph-ephemeral"]);
        assert!(ephemeral.graph.ephemeral && ephemeral.graph.data_dir.is_none());
        let off = parse(&["--no-graph"]);
        assert!(!off.graph.enabled);
        let capped = parse(&[
            "--graph-ephemeral",
            "--graph-statement-slots",
            "3",
            "--graph-query-timeout-ms",
            "1500",
        ]);
        assert_eq!(capped.graph.statement_slots, 3);
        assert_eq!(
            capped.graph.query_timeout,
            std::time::Duration::from_millis(1500)
        );
        assert!(capped.validate().is_ok());
        assert!(
            parse(&["--graph-ephemeral", "--graph-statement-slots", "0"])
                .validate()
                .is_err()
        );
        // At most 256 (GR1 Task 6 fix round 1, M4).
        assert!(
            parse(&["--graph-ephemeral", "--graph-statement-slots", "256"])
                .validate()
                .is_ok()
        );
        assert!(
            parse(&["--graph-ephemeral", "--graph-statement-slots", "257"])
                .validate()
                .is_err()
        );
        assert!(
            parse(&["--graph-ephemeral", "--graph-query-timeout-ms", "0"])
                .validate()
                .is_err()
        );
    }

    /// Q603's proposed default: `loams dev` publishes the Connect schema on
    /// the main port, and nothing else does unless it asks.
    #[test]
    fn dev_serves_reflection_and_the_other_commands_do_not() {
        assert!(dev_config(&[]).reflection);
        assert!(!ServerConfig::new("/tmp/x").reflection);
    }

    /// Ruling 22, controller ruling P2: `--gc-grace-ms` lowers every
    /// freshness deadline to half of it, so the config still validates.
    #[test]
    fn gc_grace_lowers_every_deadline_to_half() {
        let config = dev_config(&["--gc-grace-ms", "1500"]);
        let half = Duration::from_millis(750);
        assert_eq!(config.gc.grace, Duration::from_millis(1500));
        assert_eq!(config.segmenter.swap_deadline, half);
        assert_eq!(config.link.max_commit_delay, half);
        assert_eq!(config.collection.max_commit_delay, half);
        assert_eq!(config.collection.index_commit_delay, half);
        assert_eq!(config.maintenance.commit_delay, half);
        assert_eq!(config.hot_build.artifact_commit_delay, half);
        config.validate().expect("valid");
    }

    #[test]
    fn flight_sql_and_hot_flags_set_the_config() {
        let config = dev_config(&[]);
        assert_eq!(config.flight_sql, Some("127.0.0.1:8082".parse().unwrap()));
        assert!(config.query.hot_default);
        let config = dev_config(&["--flight-sql-listen", "127.0.0.1:9000", "--hot=off"]);
        assert_eq!(config.flight_sql, Some("127.0.0.1:9000".parse().unwrap()));
        assert!(!config.query.hot_default);
        let config = dev_config(&["--no-flight-sql", "--hot", "on"]);
        assert_eq!(config.flight_sql, None);
        assert!(config.query.hot_default);
        assert!(
            Cli::try_parse_from([
                "loams",
                "dev",
                "--no-flight-sql",
                "--flight-sql-listen",
                "127.0.0.1:1"
            ])
            .is_err()
        );
        assert!(Cli::try_parse_from(["loams", "dev", "--hot", "maybe"]).is_err());
        let cli = Cli::try_parse_from(["loams", "standalone", "--bucket", "file:///tmp/b"])
            .expect("parse");
        assert_eq!(
            config_of(cli).flight_sql,
            Some("0.0.0.0:8082".parse().unwrap())
        );
        let config = dev_config(&["--tail-max-bytes", "4096", "--consistency-wait-ms", "250"]);
        assert_eq!(config.query.tail.max_bytes, 4096);
        assert_eq!(config.query.backpressure.max_unapplied_bytes, 2048);
        config.validate().expect("a lowered budget");
        assert_eq!(
            config.query.read.consistency_wait,
            Duration::from_millis(250)
        );
    }

    #[cfg(feature = "qdrant")]
    #[cfg(feature = "sqldb")]
    #[test]
    fn sqlgate_flags_set_the_config() {
        assert_eq!(dev_config(&[]).sqlgate, None);
        let config = dev_config(&[
            "--sqlgate-listen",
            "127.0.0.1:3307",
            "--sqlgate-tls-cert",
            "/c.pem",
            "--sqlgate-tls-key",
            "/k.pem",
            "--sqlgate-upstream-ca",
            "/ca.pem",
        ]);
        let gate = config.sqlgate.expect("configured");
        assert_eq!(gate.listen, "127.0.0.1:3307".parse().unwrap());
        assert_eq!(gate.tls_key, std::path::PathBuf::from("/k.pem"));
        // M6: `loams dev` (the desktop) takes plaintext from loopback.
        use loams_sqlgate::server::PlaintextPolicy;
        assert_eq!(gate.plaintext, PlaintextPolicy::LoopbackOnly);
        let flags = [
            "--sqlgate-listen",
            "127.0.0.1:3307",
            "--sqlgate-tls-cert",
            "/c.pem",
            "--sqlgate-tls-key",
            "/k.pem",
            "--sqlgate-upstream-ca",
            "/ca.pem",
            "--sqlgate-plaintext",
            "never",
        ];
        let gate = dev_config(&flags).sqlgate.expect("configured");
        assert_eq!(gate.plaintext, PlaintextPolicy::Never);
    }

    #[cfg(feature = "stream-grpc")]
    #[test]
    fn stream_grpc_flag_sets_the_config() {
        assert_eq!(dev_config(&[]).stream_grpc, None);
        let config = dev_config(&["--stream-grpc-listen", "127.0.0.1:8091"]);
        assert_eq!(config.stream_grpc, Some("127.0.0.1:8091".parse().unwrap()));
    }

    #[test]
    fn qdrant_flags_set_the_config() {
        let qdrant = dev_config(&[]).qdrant.expect("on by default");
        assert_eq!(qdrant.rest_listen, "127.0.0.1:6333".parse().unwrap());
        assert_eq!(qdrant.grpc_listen, "127.0.0.1:6334".parse().unwrap());
        assert_eq!(qdrant.namespace, "default");
        let qdrant = dev_config(&[
            "--qdrant-listen",
            "127.0.0.1:0",
            "--qdrant-grpc-listen",
            "127.0.0.1:1",
            "--qdrant-namespace",
            "acme",
        ])
        .qdrant
        .expect("on");
        assert_eq!(qdrant.rest_listen, "127.0.0.1:0".parse().unwrap());
        assert_eq!(qdrant.grpc_listen, "127.0.0.1:1".parse().unwrap());
        assert_eq!(qdrant.namespace, "acme");
        assert!(dev_config(&["--no-qdrant"]).qdrant.is_none());
        assert!(
            Cli::try_parse_from([
                "loams",
                "dev",
                "--no-qdrant",
                "--qdrant-listen",
                "127.0.0.1:1"
            ])
            .is_err()
        );
        let cli = Cli::try_parse_from(["loams", "standalone", "--bucket", "file:///tmp/b"])
            .expect("parse");
        assert!(config_of(cli).qdrant.is_some());
        let cluster = cluster_config(&[
            "--node-id",
            "1",
            "--roles",
            "meta,gateway",
            "--listen",
            "127.0.0.1:7001",
            "--peers",
            "1=127.0.0.1:7001",
            "--bucket",
            "file:///tmp/b",
            "--no-qdrant",
        ])
        .expect("parse");
        assert!(cluster.qdrant.is_none());
        // ServerConfig::new serves no Qdrant API (E12).
        assert!(ServerConfig::new("/tmp/x").qdrant.is_none());
    }

    #[cfg(feature = "es")]
    #[test]
    fn es_flags_set_the_config() {
        let es = dev_config(&[]).es.expect("on by default");
        assert_eq!(es.listen, "127.0.0.1:9200".parse().unwrap());
        assert_eq!(es.namespace, "default");
        let es = dev_config(&["--es-listen", "127.0.0.1:0", "--es-namespace", "acme"])
            .es
            .expect("on");
        assert_eq!(es.listen, "127.0.0.1:0".parse().unwrap());
        assert_eq!(es.namespace, "acme");
        assert!(dev_config(&["--no-es"]).es.is_none());
        assert!(
            Cli::try_parse_from(["loams", "dev", "--no-es", "--es-listen", "127.0.0.1:1"]).is_err()
        );
        // D111: loopback in every mode.
        let cli = Cli::try_parse_from(["loams", "standalone", "--bucket", "file:///tmp/b"])
            .expect("parse");
        let es = config_of(cli).es.expect("standalone serves it");
        assert_eq!(es.listen, "127.0.0.1:9200".parse().unwrap());
        let cluster = cluster_config(&[
            "--node-id",
            "1",
            "--roles",
            "meta,gateway",
            "--listen",
            "127.0.0.1:7001",
            "--peers",
            "1=127.0.0.1:7001",
            "--bucket",
            "file:///tmp/b",
        ])
        .expect("parse");
        assert_eq!(
            cluster.es.expect("cluster serves it").listen,
            "127.0.0.1:9200".parse().unwrap()
        );
        let cluster = cluster_config(&[
            "--node-id",
            "1",
            "--roles",
            "meta,gateway",
            "--listen",
            "127.0.0.1:7001",
            "--peers",
            "1=127.0.0.1:7001",
            "--bucket",
            "file:///tmp/b",
            "--no-es",
        ])
        .expect("parse");
        assert!(cluster.es.is_none());
        // ServerConfig::new serves no Elasticsearch API (row E13).
        assert!(ServerConfig::new("/tmp/x").es.is_none());
    }

    #[test]
    fn backpressure_flags_set_the_config() {
        let config = dev_config(&[]);
        assert!(config.query.backpressure.enabled);
        assert_eq!(config.query.backpressure.max_unapplied_records, 1_000_000);
        assert_eq!(config.query.backpressure.max_unapplied_bytes, 128 << 20);
        let config = dev_config(&[
            "--backpressure",
            "off",
            "--max-unapplied-records",
            "7",
            "--max-unapplied-bytes",
            "1000",
        ]);
        assert!(!config.query.backpressure.enabled);
        assert_eq!(config.query.backpressure.max_unapplied_records, 7);
        assert_eq!(config.query.backpressure.max_unapplied_bytes, 1000);
        assert!(Cli::try_parse_from(["loams", "dev", "--backpressure", "maybe"]).is_err());
        let cli = Cli::try_parse_from([
            "loams",
            "standalone",
            "--bucket",
            "file:///tmp/b",
            "--max-unapplied-records",
            "9",
        ])
        .expect("parse");
        assert_eq!(config_of(cli).query.backpressure.max_unapplied_records, 9);
    }

    fn config_of(cli: Cli) -> ServerConfig {
        config(cli.command)
    }

    #[test]
    fn hot_and_maintenance_flags_set_the_config() {
        let config = dev_config(&[]);
        assert!(config.hot.enabled && !config.hot.pin_all);
        assert!(config.maintenance.merge && config.maintenance.compaction);
        let config = dev_config(&[
            "--hot=off",
            "--hot-pin-all",
            "--hot-dir",
            "/tmp/h",
            "--hot-nvme-bytes",
            "1000",
            "--hot-ram-bytes",
            "2000",
            "--hot-reconcile-interval-ms",
            "50",
            "--hot-rebuild-max-staleness-ms",
            "500",
            "--hot-rebuild-min-inserted",
            "7",
            "--hot-build-poll-interval-ms",
            "100",
            "--maintenance",
            "off",
            "--merge-poll-interval-ms",
            "30",
            "--merge-min-level-docs",
            "5",
            "--compaction-min-small-fragments",
            "4",
            "--compaction-target-rows",
            "99",
        ]);
        assert!(!config.hot.enabled && !config.query.hot_default);
        assert!(config.hot.pin_all);
        assert_eq!(config.hot.dir, PathBuf::from("/tmp/h"));
        assert_eq!((config.hot.nvme_bytes, config.hot.ram_bytes), (1000, 2000));
        assert_eq!(config.hot.reconcile_interval, Duration::from_millis(50));
        assert_eq!(
            config.hot_build.rebuild_max_staleness,
            Duration::from_millis(500)
        );
        assert_eq!(config.hot_build.rebuild_min_inserted, 7);
        assert_eq!(config.hot_build.poll_interval, Duration::from_millis(100));
        assert!(!config.maintenance.merge && !config.maintenance.compaction);
        assert_eq!(config.maintenance.poll_interval, Duration::from_millis(30));
        assert_eq!(config.maintenance.merge_policy.min_level_num_docs, 5);
        assert_eq!(config.maintenance.compaction_min_small_fragments, 4);
        assert_eq!(config.maintenance.compaction_target_rows, 99);
        let cli = Cli::try_parse_from([
            "loams",
            "standalone",
            "--bucket",
            "file:///tmp/b",
            "--hot-pin-all",
        ])
        .expect("parse");
        assert!(config_of(cli).hot.pin_all);
        let cli = Cli::try_parse_from(["loams", "warm", "acme/docs"]).expect("parse");
        assert!(matches!(
            cli.command,
            Command::Warm { ref target, ref server }
                if target == "acme/docs" && server == "http://127.0.0.1:8080"
        ));
    }

    #[test]
    fn collection_tuning_flags_set_the_collection_config() {
        let config = dev_config(&[
            "--collection-trim",
            "false",
            "--collection-index-min-rows",
            "40",
            "--collection-index-delta-min-rows",
            "41",
            "--collection-retention-ms",
            "0",
            "--collection-index-poll-interval-ms",
            "200",
        ]);
        assert!(!config.collection.trim);
        assert_eq!(config.collection.index_min_rows, 40);
        assert_eq!(config.collection.index_delta_min_rows, 41);
        assert_eq!(config.collection.time_travel_retention, Duration::ZERO);
        assert_eq!(
            config.collection.index_poll_interval,
            Duration::from_millis(200)
        );
        assert!(dev_config(&["--collection-trim", "true"]).collection.trim);
    }

    /// R1 plan Task 6: `--meta` on `dev` and `standalone` selects the TiKV
    /// metastore; `cluster` has no such flag, and a cluster config with a
    /// TiKV metastore is refused.
    #[cfg(feature = "tikv")]
    #[test]
    fn meta_flag_selects_the_tikv_metastore_on_dev_and_standalone_only() {
        assert_eq!(dev_config(&[]).meta, MetaBackend::Raft);
        let url = "tikv://127.0.0.1:2379/loams_meta";
        let MetaBackend::Tikv(tikv) = dev_config(&["--meta", url]).meta else {
            panic!("expected the TiKV metastore");
        };
        assert_eq!(tikv.tikv.keyspace, "loams_meta");
        assert_eq!(tikv.tikv.pd, ["127.0.0.1:2379"]);
        let standalone = Cli::try_parse_from([
            "loams",
            "standalone",
            "--bucket",
            "file:///tmp/b",
            "--meta",
            url,
        ])
        .map(config_of)
        .expect("parse");
        assert!(matches!(standalone.meta, MetaBackend::Tikv(_)));
        assert!(Cli::try_parse_from(["loams", "dev", "--meta", "raft://x"]).is_err());
        let cluster = Cli::try_parse_from([
            "loams",
            "cluster",
            "--node-id",
            "1",
            "--roles",
            "meta",
            "--listen",
            "127.0.0.1:7001",
            "--peers",
            "1=127.0.0.1:7001",
            "--bucket",
            "file:///tmp/b",
            "--meta",
            url,
        ]);
        assert!(cluster.is_err(), "loams cluster has no --meta");
        let mut config = cluster_config(&[
            "--node-id",
            "1",
            "--roles",
            "meta",
            "--listen",
            "127.0.0.1:7001",
            "--peers",
            "1=127.0.0.1:7001",
            "--bucket",
            "file:///tmp/b",
        ])
        .expect("parse");
        config.meta = MetaBackend::parse(url).expect("url");
        let err = config.validate().expect_err("refused").to_string();
        assert!(err.contains("dev and standalone only"), "{err}");
    }

    /// The store of a Live config.
    #[cfg(feature = "live")]
    fn live_store(live: &loams_live::LiveConfig) -> &loams_live::StoreConfig {
        &live.store
    }

    /// The embedded store of a Live config.
    #[cfg(feature = "live")]
    fn live_embedded(live: &loams_live::LiveConfig) -> &loams_live::EmbeddedConfig {
        match live_store(live) {
            loams_live::StoreConfig::Embedded(e) => e,
            #[allow(unreachable_patterns)]
            other => panic!("expected the embedded store, got {other:?}"),
        }
    }

    /// The TiKV handle of a Live config.
    #[cfg(feature = "live-tikv")]
    fn live_tikv(live: &loams_live::LiveConfig) -> &loams_tikv::TikvConfig {
        match live_store(live) {
            loams_live::StoreConfig::Tikv(tikv) => tikv,
            other => panic!("expected a TiKV store, got {other:?}"),
        }
    }

    /// R1 plan Task 12 and LV1 plan Task 23: `--live-*` on `dev` and
    /// `standalone` configure Loams Live, on by default on the embedded store
    /// under `<data_dir>/live/` (`--no-live` turns it off); the tick read lag
    /// is a Live config key (row T12-1).
    #[cfg(feature = "live")]
    #[test]
    fn live_flags_set_the_live_config() {
        let config = dev_config(&[]);
        let live = config.live.clone().expect("on by default");
        assert_eq!(live.listen, SocketAddr::from(([127, 0, 0, 1], 7710)));
        let store = live_embedded(&live);
        assert_eq!(
            store.path,
            config.data_dir.join("live").join("store.redb"),
            "the embedded store under <data_dir>/live/"
        );
        assert_eq!(store.keyspace, "loams_live_dev");
        assert_eq!(live.app, "dev");
        assert_eq!(live.subs.tick_read_lag, Duration::from_millis(50));
        let explicit = dev_config(&["--live-store", "embedded"]).live.expect("on");
        assert_eq!(
            live_store(&explicit).backend(),
            loams_live::Backend::Embedded
        );
        let live = dev_config(&[
            "--live-listen",
            "localhost:7711",
            "--live-app",
            "chat",
            "--live-tick-read-lag-ms",
            "0",
        ])
        .live
        .expect("configured");
        assert_eq!(live.listen, SocketAddr::from(([127, 0, 0, 1], 7711)));
        assert_eq!(live_embedded(&live).keyspace, "loams_live_chat");
        assert_eq!(live.subs.tick_read_lag, Duration::ZERO);
        assert!(dev_config(&["--no-live"]).live.is_none());
        let standalone = Cli::try_parse_from([
            "loams",
            "standalone",
            "--bucket",
            "file:///tmp/b",
            "--no-live",
        ])
        .map(config_of)
        .expect("parse");
        assert!(standalone.live.is_none());
        assert!(Cli::try_parse_from(["loams", "dev", "--live-app", "no spaces"]).is_err());
        assert!(Cli::try_parse_from(["loams", "dev", "--no-live", "--live-app", "x"]).is_err());
        assert!(
            Cli::try_parse_from(["loams", "dev", "--no-live", "--live-store", "embedded"]).is_err()
        );
        for bad in ["tikv://", "tikv:///ks", "mem://x", "embedded/x"] {
            assert!(
                Cli::try_parse_from(["loams", "dev", "--live-store", bad]).is_err(),
                "{bad}"
            );
        }
    }

    /// LV1 plan Task 23: `--live-store tikv://<pd>[,<pd>]/<keyspace>` runs
    /// Live on TiKV (the keyspace defaults to `loams_live_<app>`).
    #[cfg(feature = "live-tikv")]
    #[test]
    fn live_store_tikv_selects_the_tikv_store() {
        let live = dev_config(&["--live-store", "tikv://10.0.0.1:2379,10.0.0.2:2379/ks"])
            .live
            .expect("on");
        assert_eq!(live_tikv(&live).pd, ["10.0.0.1:2379", "10.0.0.2:2379"]);
        assert_eq!(live_tikv(&live).keyspace, "ks");
        let live = dev_config(&["--live-store", "tikv://pd:2379", "--live-app", "chat"])
            .live
            .expect("on");
        assert_eq!(live_tikv(&live).pd, ["pd:2379"]);
        assert_eq!(live_tikv(&live).keyspace, "loams_live_chat");
        let err = Cli::try_parse_from(["loams", "dev", "--live-store", "tikv://pd:2379/ks/extra"])
            .expect_err("a / in the keyspace")
            .to_string();
        assert!(err.contains("a keyspace has no '/'"), "{err}");
    }

    /// Row T23-9: `dev --help` names `tikv://` for `--live-store` (and lists
    /// the deprecated aliases) only in a build with live-tikv, so the desktop
    /// app's probe can tell the builds apart.
    #[cfg(feature = "live")]
    #[test]
    fn live_store_help_names_tikv_only_with_live_tikv() {
        use clap::CommandFactory;
        let mut cli = Cli::command();
        let dev = cli.find_subcommand_mut("dev").expect("dev");
        let help = dev.render_long_help().to_string();
        // The flag's line and its description, up to the next flag.
        let mut lines = help
            .lines()
            .skip_while(|l| !l.trim_start().starts_with("--live-store"));
        let first = lines.next().unwrap_or_default().to_string();
        let block: String = std::iter::once(first)
            .chain(
                lines
                    .take_while(|l| !l.trim_start().starts_with('-'))
                    .map(str::to_string),
            )
            .collect::<Vec<_>>()
            .join("\n");
        assert!(block.contains("--live-store"), "{help}");
        assert_eq!(
            block.contains("tikv://"),
            cfg!(feature = "live-tikv"),
            "{block}"
        );
        assert_eq!(
            help.contains("--live-pd"),
            cfg!(feature = "live-tikv"),
            "{help}"
        );
    }

    /// LV1 plan Task 23: `--live-pd` and `--live-keyspace` stay one release
    /// as aliases of `--live-store tikv://…` (the PD default stays
    /// 127.0.0.1:19379 when only the keyspace is given).
    #[cfg(feature = "live-tikv")]
    #[test]
    fn live_pd_alias_maps_to_tikv_store() {
        let live = dev_config(&[
            "--live-pd",
            "10.0.0.1:2379,10.0.0.2:2379",
            "--live-app",
            "chat",
        ])
        .live
        .expect("on");
        assert_eq!(live_tikv(&live).pd, ["10.0.0.1:2379", "10.0.0.2:2379"]);
        assert_eq!(live_tikv(&live).keyspace, "loams_live_chat");
        let live = dev_config(&["--live-keyspace", "other"]).live.expect("on");
        assert_eq!(live_tikv(&live).pd, ["127.0.0.1:19379"]);
        assert_eq!(live_tikv(&live).keyspace, "other");
        assert!(
            Cli::try_parse_from([
                "loams",
                "dev",
                "--live-pd",
                "a:1",
                "--live-store",
                "embedded"
            ])
            .is_err(),
            "--live-store and the aliases exclude each other"
        );
    }

    /// LV1 plan Task 23: a build without `live-tikv` refuses
    /// `--live-store tikv://…` with the feature named.
    #[cfg(all(feature = "live", not(feature = "live-tikv")))]
    #[test]
    fn live_store_tikv_requires_feature() {
        let err = Cli::try_parse_from(["loams", "dev", "--live-store", "tikv://pd:2379/ks"])
            .expect_err("no TiKV backend in this build")
            .to_string();
        assert!(
            err.contains("--live-store tikv:// needs a build with the live-tikv feature"),
            "{err}"
        );
    }

    /// LV1 row T23-7: a build without `live-tikv` refuses the deprecated
    /// `--live-pd` and `--live-keyspace` (they ask for TiKV) instead of
    /// running Live on the embedded store.
    #[cfg(all(feature = "live", not(feature = "live-tikv")))]
    #[test]
    fn live_pd_requires_live_tikv() {
        for args in [
            ["loams", "dev", "--live-pd", "127.0.0.1:19379"],
            ["loams", "dev", "--live-keyspace", "loams_live_dev"],
        ] {
            let err = Cli::try_parse_from(args)
                .expect_err("no TiKV backend in this build")
                .to_string();
            assert!(
                err.contains("need a build with the live-tikv feature"),
                "{err}"
            );
        }
    }

    /// R1 plan Task 12 semantics 7 and the loopback rule (D111): a
    /// non-loopback `--live-listen` fails startup with the error of design
    /// §20 §7.1; loopback addresses pass.
    #[cfg(feature = "live")]
    use loams::ServerError;

    #[cfg(feature = "live")]
    #[test]
    fn a_non_loopback_live_listen_fails_startup() {
        for bad in ["0.0.0.0:7710", "192.168.1.10:7710", "[::]:7710"] {
            let err = dev_config(&["--live-listen", bad])
                .validate()
                .expect_err(bad);
            assert!(
                matches!(err, ServerError::LiveListenNotLoopback { .. }),
                "{bad}: {err}"
            );
            assert_eq!(
                err.to_string(),
                format!(
                    "--live-listen {} is not a loopback address; the Live API has no \
                     authentication until the unified auth plan (D111)",
                    bad.parse::<SocketAddr>().expect("an address")
                )
            );
        }
        for ok in [
            "127.0.0.1:7710",
            "localhost:7710",
            "[::1]:7710",
            "127.0.0.2:1",
        ] {
            dev_config(&["--live-listen", ok])
                .validate()
                .unwrap_or_else(|e| panic!("{ok}: {e}"));
        }
        assert!(Cli::try_parse_from(["loams", "dev", "--live-listen", "nohost:1"]).is_err());
    }

    /// LV1 plan Task 5: `--live-isolation` and `--live-tenancy` set
    /// `[live] isolation` and `tenancy`; multi-tenancy in process fails
    /// startup, and `live-worker` is a hidden subcommand.
    #[cfg(feature = "live")]
    #[test]
    fn live_isolation_and_tenancy_flags() {
        let live = |args: &[&str]| dev_config(args).live.expect("live is on");
        let defaults = live(&[]);
        assert_eq!(defaults.isolation, loams_live::Isolation::InProcess);
        assert_eq!(defaults.tenancy, loams_live::Tenancy::Single);
        let isolated = live(&["--live-isolation", "isolated", "--live-tenancy", "multi"]);
        assert_eq!(isolated.isolation, loams_live::Isolation::Isolated);
        assert_eq!(isolated.tenancy, loams_live::Tenancy::Multi);
        let err = dev_config(&["--live-tenancy", "multi"])
            .validate()
            .expect_err("multi tenancy in process");
        assert_eq!(
            err.to_string(),
            r#"invalid configuration: live: tenancy = "multi" needs isolation = "isolated" (Linux only)"#
        );
        #[cfg(target_os = "linux")]
        dev_config(&["--live-isolation", "isolated", "--live-tenancy", "multi"])
            .validate()
            .expect("multi tenancy with isolated workers");
        for bad in [
            ["--live-isolation", "in-process"],
            ["--live-tenancy", "many"],
        ] {
            assert!(
                Cli::try_parse_from(["loams", "dev"].iter().chain(&bad)).is_err(),
                "{bad:?}"
            );
        }
        assert!(matches!(
            Cli::try_parse_from(["loams", "live-worker"]).map(|c| c.command),
            Ok(Command::LiveWorker)
        ));
        let help = Cli::try_parse_from(["loams", "--help"])
            .expect_err("help")
            .to_string();
        assert!(!help.contains("live-worker"), "{help}");
    }

    /// Owner ruling T7-3: a build without the `tikv` feature refuses
    /// `--meta tikv://…` at parse time with an error naming the feature.
    #[cfg(not(feature = "tikv"))]
    #[test]
    fn meta_flag_without_the_tikv_feature_is_refused_naming_it() {
        let url = "tikv://127.0.0.1:2379/loams_meta";
        for args in [
            &["loams", "dev", "--meta", url][..],
            &[
                "loams",
                "standalone",
                "--bucket",
                "file:///tmp/b",
                "--meta",
                url,
            ],
        ] {
            let command = args[1];
            let err = Cli::try_parse_from(args)
                .map(|_| ())
                .expect_err("refused")
                .to_string();
            assert!(
                err.contains("built without the tikv feature"),
                "{command}: {err}"
            );
            assert!(err.contains("--features tikv"), "{command}: {err}");
        }
    }

    fn cluster_config(args: &[&str]) -> Result<ServerConfig, clap::Error> {
        Cli::try_parse_from(["loams", "cluster"].iter().chain(args)).map(config_of)
    }

    #[test]
    fn cluster_flags_set_the_cluster_config() {
        let base = [
            "--node-id",
            "2",
            "--roles",
            "query,meta",
            "--listen",
            "127.0.0.1:7002",
            "--peers",
            "1=127.0.0.1:7001,2=127.0.0.1:7002,3=10.0.0.3:7003",
            "--bucket",
            "file:///tmp/b",
        ];
        let config = cluster_config(&base).expect("parse");
        let cluster = config.cluster.clone().expect("cluster");
        assert_eq!(cluster.node_id, 2);
        assert_eq!(cluster.roles.to_string(), "meta,query");
        assert_eq!(cluster.advertise, "127.0.0.1:7002", "defaults to --listen");
        assert_eq!(cluster.peers.len(), 3);
        assert_eq!((cluster.replication, cluster.zone.as_str()), (1, ""));
        assert_eq!(config.flight_sql, Some("0.0.0.0:8082".parse().unwrap()));
        config.validate().expect("valid");

        let mut args = base.to_vec();
        args.extend([
            "--no-flight-sql",
            "--zone",
            "a",
            "--replication",
            "2",
            "--flush-interval-ms",
            "20",
            "--registry-ttl-ms",
            "1500",
            "--learner-expiry-ms",
            "3000",
            "--membership-interval-ms",
            "500",
            "--lease-ttl-ms",
            "1500",
            "--poll-interval-ms",
            "50",
        ]);
        let config = cluster_config(&args).expect("parse");
        let cluster = config.cluster.clone().expect("cluster");
        assert_eq!(config.flight_sql, None);
        assert_eq!((cluster.zone.as_str(), cluster.replication), ("a", 2));
        assert_eq!(config.log.flush_interval, Duration::from_millis(20));
        assert_eq!(cluster.registry.lease_ttl, Duration::from_millis(1500));
        assert_eq!(cluster.registry.renew_every, Duration::from_millis(500));
        assert_eq!(cluster.learner_expiry, Duration::from_millis(3000));
        assert_eq!(cluster.membership_interval, Duration::from_millis(500));
        assert_eq!(config.worker_lease_ttl, Duration::from_millis(1500));

        assert!(cluster_config(&["--roles", "cook"]).is_err());
        // Rule 1: a meta node must be in --peers, with its advertise address.
        let invalid = |edit: &dyn Fn(&mut Vec<&str>)| {
            let mut args = base.to_vec();
            edit(&mut args);
            let config = cluster_config(&args).expect("parse");
            config.validate().expect_err("invalid").to_string()
        };
        let err = invalid(&|a| a[1] = "4");
        assert!(err.contains("not in --peers"), "{err}");
        let err = invalid(&|a| a.extend(["--advertise", "127.0.0.1:9999"]));
        assert!(err.contains("differs from --advertise"), "{err}");
        let err = invalid(&|a| a[3] = "query");
        assert!(err.contains("no meta role"), "{err}");
        let err = invalid(&|a| a[5] = "0.0.0.0:7002");
        assert!(err.contains("unspecified"), "{err}");
        let err = invalid(&|a| a[7] = "1=nowhere,2=127.0.0.1:7002");
        assert!(err.contains("not host:port"), "{err}");
        let err = invalid(&|a| a.extend(["--replication", "0"]));
        assert!(err.contains("replication"), "{err}");
    }

    #[cfg(feature = "durable")]
    const CLUSTER: [&str; 10] = [
        "--node-id",
        "1",
        "--roles",
        "meta,gateway",
        "--listen",
        "127.0.0.1:7001",
        "--peers",
        "1=127.0.0.1:7001",
        "--bucket",
        "file:///tmp/b",
    ];

    #[cfg(feature = "durable")]
    fn parse_error(args: &[&str]) -> String {
        Cli::try_parse_from(["loams", "dev"].iter().chain(args))
            .expect_err("refused")
            .to_string()
    }

    #[test]
    fn durable_store_and_set_values_parse() {
        assert_eq!(
            parse_durable_store("sqlite:/tmp/d.db"),
            Ok(DurableStoreArg::Sqlite("/tmp/d.db".into()))
        );
        assert_eq!(
            parse_durable_store("sqlite:///tmp/d.db"),
            Ok(DurableStoreArg::Sqlite("/tmp/d.db".into()))
        );
        assert!(matches!(
            parse_durable_store("mysql://root@127.0.0.1:4000/loams_durable_default"),
            Ok(DurableStoreArg::Mysql(_))
        ));
        assert!(matches!(
            parse_durable_store("tikv://127.0.0.1:2379,127.0.0.1:2381/loams_durable"),
            Ok(DurableStoreArg::Tikv(_))
        ));
        assert!(parse_durable_store("tikv://127.0.0.1:2379").is_err());
        assert!(parse_durable_store("tikv://127.0.0.1:2379/").is_err());
        assert!(parse_durable_store("tikv://127.0.0.1:2379,/loams_durable").is_err());
        assert!(parse_durable_store("sqlite:").is_err());
        assert!(parse_durable_store("postgres://x").is_err());
        assert_eq!(parse_key_value("a.b=c=d"), Ok(("a.b".into(), "c=d".into())));
        assert!(parse_key_value("a.b").is_err());
        assert!(parse_key_value("=x").is_err());
    }

    /// Without the feature every durable flag still parses (and logs that
    /// it is ignored), so scripts work against either build.
    #[cfg(not(feature = "durable"))]
    #[test]
    fn durable_flags_parse_without_the_feature() {
        dev_config(&[
            "--durable-listen",
            "127.0.0.1:9001",
            "--durable-store",
            "sqlite:/tmp/d.db",
            "--durable-push",
            "--durable-set",
            "a.b=c",
        ]);
        dev_config(&["--no-durable"]);
    }

    /// D1 Task 3 semantics 2: the defaults on dev and standalone.
    #[cfg(feature = "durable")]
    #[test]
    fn durable_defaults_on_dev_and_standalone() {
        use loams_durable::{DEFAULT_LISTEN, DurableStore};
        let durable = dev_config(&[]).durable.expect("on by default");
        assert_eq!(durable.listen, DEFAULT_LISTEN);
        assert_eq!(durable.listen, "127.0.0.1:8001".parse().unwrap());
        assert_eq!(
            durable.store,
            DurableStore::Sqlite {
                path: PathBuf::from(".loams/durable/default.db")
            }
        );
        assert!(!durable.push && !durable.debug && durable.overrides.is_empty());
        let cli = Cli::try_parse_from([
            "loams",
            "standalone",
            "--bucket",
            "file:///tmp/b",
            "--data-dir",
            "/var/lib/loams",
        ])
        .expect("parse");
        let durable = config_of(cli).durable.expect("on by default");
        assert_eq!(
            durable.store,
            DurableStore::Sqlite {
                path: PathBuf::from("/var/lib/loams/durable/default.db")
            }
        );
        // ServerConfig::new serves no durable API; the CLI sets it.
        assert!(ServerConfig::new("/tmp/x").durable.is_none());
    }

    #[cfg(feature = "durable")]
    #[test]
    fn durable_flags_set_the_config() {
        use loams_durable::DurableStore;
        assert!(dev_config(&["--no-durable"]).durable.is_none());
        let durable = dev_config(&[
            "--durable-listen",
            "localhost:9001",
            "--durable-store",
            "sqlite:/tmp/d.db",
            "--durable-push",
            "--durable-debug",
            "--durable-set",
            "servers.server_sqlite.preload_limit=5",
            "--durable-set",
            "gateways.gateway_http.cors_allow_origins=[\"*\"]",
        ])
        .durable
        .expect("on");
        assert_eq!(durable.listen, "127.0.0.1:9001".parse().unwrap());
        assert_eq!(
            durable.store,
            DurableStore::Sqlite {
                path: PathBuf::from("/tmp/d.db")
            }
        );
        assert!(durable.push && durable.debug);
        assert_eq!(
            durable.overrides,
            vec![
                (
                    "servers.server_sqlite.preload_limit".to_string(),
                    "5".to_string()
                ),
                (
                    "gateways.gateway_http.cors_allow_origins".to_string(),
                    "[\"*\"]".to_string()
                ),
            ]
        );
        // D138: only loopback.
        for addr in ["0.0.0.0:8001", "192.168.1.5:8001", "example.com:8001"] {
            let err = parse_error(&["--durable-listen", addr]);
            assert!(err.contains("--durable-listen"), "{addr}: {err}");
        }
        let err = parse_error(&["--durable-listen", "0.0.0.0:8001"]);
        assert!(
            err.contains("durable listener must be loopback until authentication is configured"),
            "{err}"
        );
        for flag in [
            &["--durable-listen", "127.0.0.1:1"][..],
            &["--durable-store", "sqlite:/tmp/d.db"],
            &["--durable-push"],
            &["--durable-set", "a.b=c"],
            &["--durable-debug"],
        ] {
            let mut args = vec!["--no-durable"];
            args.extend_from_slice(flag);
            parse_error(&args);
        }
        parse_error(&["--durable-set", "no-equals"]);
        parse_error(&["--durable-store", "redis://x"]);
    }

    /// `loams durable migrate --durable-store …` (D1 Task 4): a store is
    /// required, and nothing else.
    #[test]
    fn durable_migrate_parses() {
        let cli = Cli::try_parse_from([
            "loams",
            "durable",
            "migrate",
            "--durable-store",
            "mysql://root@127.0.0.1:4000/loams_durable_default",
        ])
        .expect("parse");
        let Command::Durable {
            command: DurableCommand::Migrate { durable_store },
        } = cli.command
        else {
            panic!("not durable migrate: {:?}", cli.command);
        };
        assert!(matches!(durable_store, DurableStoreArg::Mysql(_)));
        assert!(Cli::try_parse_from(["loams", "durable", "migrate"]).is_err());
        assert!(
            Cli::try_parse_from([
                "loams",
                "durable",
                "migrate",
                "--durable-store",
                "sqlite:/tmp/d.db",
                "--listen",
                "127.0.0.1:1",
            ])
            .is_err()
        );
    }

    /// The security fix of Task 4: no rendering of the parsed flags, and no
    /// parse error, carries a MySQL password.
    #[test]
    fn durable_store_password_never_printed() {
        let url = "mysql://loams:hunter22@tidb.internal:4000/loams_durable_default";
        let cli = Cli::try_parse_from(["loams", "dev", "--durable-store", url]).expect("parse");
        let debug = format!("{cli:?}");
        assert!(!debug.contains("hunter22"), "{debug}");
        assert!(debug.contains("tidb.internal:4000"), "{debug}");
        let migrate = Cli::try_parse_from(["loams", "durable", "migrate", "--durable-store", url])
            .expect("parse");
        assert!(!format!("{migrate:?}").contains("hunter22"));
        for bad in [
            "mysql://loams:hunter22@tidb.internal:4000/d?ssl-mode=preferred",
            "mysql://loams:hunter22@/d",
        ] {
            let err = Cli::try_parse_from(["loams", "dev", "--durable-store", bad]);
            #[cfg(feature = "durable")]
            {
                let err = err.expect_err(bad).to_string();
                assert!(!err.contains("hunter22"), "{err}");
                assert!(err.contains("--durable-store"), "{err}");
            }
            #[cfg(not(feature = "durable"))]
            let _ = err;
        }
        #[cfg(feature = "durable")]
        {
            let config = dev_config(&["--durable-store", url]);
            let debug = format!("{config:?}");
            assert!(!debug.contains("hunter22"), "{debug}");
        }
    }

    /// `ssl-mode` on the URL, else TLS except on this machine (D1 Task 4).
    #[cfg(feature = "durable")]
    #[test]
    fn durable_mysql_store_maps_tls() {
        use loams_durable::{DurableStore, MysqlTls};
        let tls = |url: &str| match dev_config(&["--durable-store", url])
            .durable
            .map(|d| d.store)
        {
            Some(DurableStore::Mysql { url: kept, tls }) => {
                assert_eq!(kept, url);
                tls
            }
            other => panic!("{url}: {other:?}"),
        };
        assert_eq!(
            tls("mysql://loams:pw@tidb.internal:4000/d"),
            MysqlTls::Required
        );
        assert_eq!(tls("mysql://root@127.0.0.1:4000/d"), MysqlTls::Disabled);
        assert_eq!(tls("mysql://root@localhost:4000/d"), MysqlTls::Disabled);
        assert_eq!(
            tls("mysql://loams@tidb.internal:4000/d?ssl-mode=disabled"),
            MysqlTls::Disabled
        );
        assert_eq!(
            tls("mysql://root@127.0.0.1:4000/d?ssl-mode=required"),
            MysqlTls::Required
        );
        // Owner ruling Q8: the verifying modes, for managed TiDB.
        assert_eq!(
            tls("mysql://loams:pw@gateway.tidbcloud.com:4000/d?ssl-mode=verify_identity"),
            MysqlTls::VerifyIdentity
        );
        assert_eq!(
            tls("mysql://loams:pw@tidb.internal:4000/d?ssl-mode=verify_ca"),
            MysqlTls::VerifyCa
        );
    }

    /// A cluster node has no default durable store and refuses a SQLite one.
    #[cfg(feature = "durable")]
    #[test]
    fn cluster_durable_needs_a_mysql_store() {
        use loams_durable::DurableStore;
        let config = cluster_config(&CLUSTER).expect("parse");
        assert!(config.durable.is_none(), "no default store on a cluster");
        config.validate().expect("valid");

        let mut args = CLUSTER.to_vec();
        args.extend(["--durable-store", "sqlite:/tmp/d.db"]);
        let config = cluster_config(&args).expect("parse");
        let err = config
            .validate()
            .expect_err("sqlite on a cluster")
            .to_string();
        assert!(
            err.contains("the sqlite durable store is single-node; use --durable-store mysql://…"),
            "{err}"
        );

        let mut args = CLUSTER.to_vec();
        args.extend([
            "--durable-store",
            "mysql://root@127.0.0.1:4000/loams_durable_default",
            "--durable-listen",
            "127.0.0.1:9001",
        ]);
        let config = cluster_config(&args).expect("parse");
        let durable = config.durable.clone().expect("a mysql store");
        assert!(matches!(durable.store, DurableStore::Mysql { .. }));
        assert_eq!(durable.listen, "127.0.0.1:9001".parse().unwrap());
        config.validate().expect("valid");
    }
}
