//! `GraphAdminService` over the graph catalog, and the data plane's entry point that opens a
//! catalog graph lazily (GR1 Task 4).
//!
//! The catalog ([`GraphCatalog`]) is the source of truth for which graphs exist; the engine holds
//! the ones that are open. A graph is opened on its first statement (or `GetSchema`), from
//! `<data_dir>/graphs/<id>/`, so a restarted server serves every graph its catalog lists without
//! opening any of them up front.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use buffa_types::google::protobuf::Timestamp;
use connectrpc::{ConnectError, ErrorCode};
use loams_proto::loams::graph::v1 as pb;
use loams_proto::loams::operations::v1 as ops;

use super::errors::{map_catalog, map_engine, refuse, refuse_with};
use super::{data, engine_info, stream};
use crate::catalog::{
    CatalogError, CatalogState, GraphCatalog, GraphLimits, GraphMeta, GraphMode, NewGraph,
};
use crate::engine::{Engine, Graph, GraphState, OpenSpec};
use crate::limits::{
    DEFAULT_NAMESPACE_DETACHED, DEFAULT_NAMESPACE_STATEMENTS, Detached, NamespaceSlots,
    StatementLimits, StatementPool, Watch, namespace_cap,
};

/// How long a deleted graph's storage is kept before it is purged, by default.
pub const DEFAULT_RETENTION_HOLD: Duration = Duration::from_secs(24 * 3600);

/// How long [`GraphAdmin::shutdown`] waits for running statements, by default.
pub const DEFAULT_SHUTDOWN_WAIT: Duration = Duration::from_secs(10);

/// The largest statement cap [`GraphAdmin::with_statement_slots`] accepts.
pub const MAX_STATEMENT_SLOTS: usize = 4096;

/// The most followers a graph may ask for (review M6).
pub const MAX_REPLICAS: u32 = 8;
/// The longest idempotency key accepted (review M6).
pub const MAX_IDEMPOTENCY_KEY: usize = 128;
/// The server's per-statement maximums (§48 §13.1, [`StatementLimits::MAXIMUM`]); a graph's
/// limits are capped to them.
pub const MAX_LIMITS: GraphLimits = GraphLimits {
    timeout_ms: StatementLimits::MAXIMUM.timeout.as_millis() as u32,
    max_rows: StatementLimits::MAXIMUM.max_rows,
    max_result_bytes: StatementLimits::MAXIMUM.max_result_bytes,
    memory_bytes: 64 << 30,
    max_path_hops: StatementLimits::MAXIMUM.max_path_hops,
};

/// Pool workers beyond the statement slots, for opens that hold no slot (a direct
/// [`GraphAdmin::open`]).
const POOL_SPARE: usize = 2;

fn check_replicas(replicas: u32) -> Result<(), ConnectError> {
    if replicas > MAX_REPLICAS {
        return Err(refuse(
            ErrorCode::InvalidArgument,
            "invalid_argument",
            format!("replicas must be at most {MAX_REPLICAS}, got {replicas}"),
        ));
    }
    Ok(())
}

fn check_key(field: &str, key: &str) -> Result<Option<String>, ConnectError> {
    if key.len() > MAX_IDEMPOTENCY_KEY {
        return Err(refuse(
            ErrorCode::InvalidArgument,
            "invalid_argument",
            format!(
                "{field} must be at most {MAX_IDEMPOTENCY_KEY} bytes, got {}",
                key.len()
            ),
        ));
    }
    Ok((!key.is_empty()).then(|| key.to_string()))
}

/// The fields `UpdateGraph`'s mask may name.
const UPDATABLE: [&str; 3] = ["languages", "limits", "replicas"];

/// `GraphAdminService`, and the data plane over catalog graphs.
#[derive(Clone)]
pub struct GraphAdmin {
    engine: Arc<Engine>,
    catalog: GraphCatalog,
    retention_hold: Duration,
    /// The process-wide cap on graph statements running at once (review fix 1, I1). A permit is
    /// taken before the graph opens and released when the statement's work ends, so a statement
    /// whose client went away, or that ran past its deadline, still holds its slot.
    statements: Arc<tokio::sync::Semaphore>,
    statement_slots: usize,
    /// The threads statements and opens run on (GR1 Task 6), sized to the slots.
    pool: Arc<StatementPool>,
    /// The server's statement limits; a graph's own are applied on top (GR1 Task 6).
    limits: StatementLimits,
    /// Statements of one namespace at once (§48 §13.2).
    namespaces: Arc<NamespaceSlots>,
    /// The namespace cap asked for; [`namespace_cap`] keeps a reserve of the process slots
    /// from it (review fix 1, I1).
    namespace_configured: usize,
    /// Detached statements one namespace may have, across its graphs (review fix 1, I1).
    namespace_detached: usize,
    /// Statements running on past their deadline or their client (R0.8 (a)).
    detached: Arc<Detached>,
    /// The client's own deadline for the call this handle serves ([`GraphAdmin::for_call`]).
    client_deadline: Option<std::time::Instant>,
    /// Set when [`GraphAdmin::shutdown`] starts: from then on no graph opens and no statement
    /// starts; each answers `UNAVAILABLE` (review fix 1, I3).
    closed: Arc<AtomicBool>,
    #[cfg(feature = "test-hooks")]
    after_open_hook: Arc<std::sync::Mutex<Option<crate::catalog::AckHook>>>,
}

impl std::fmt::Debug for GraphAdmin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphAdmin")
            .field("engine", &self.engine)
            .field("catalog", &self.catalog)
            .field("retention_hold", &self.retention_hold)
            .field("statement_slots", &self.statement_slots)
            .field("limits", &self.limits)
            .field("namespace_statements", &self.namespaces.per_namespace())
            .finish_non_exhaustive()
    }
}

fn language_name(language: pb::QueryLanguage) -> &'static str {
    use pb::QueryLanguage as L;
    match language {
        L::Unspecified | L::Gql => "GQL",
        L::Cypher => "CYPHER",
        L::SqlPgq => "SQL_PGQ",
        L::Gremlin => "GREMLIN",
        L::Graphql => "GRAPHQL",
        L::Sparql => "SPARQL",
    }
}

fn language_value(name: &str) -> pb::QueryLanguage {
    use pb::QueryLanguage as L;
    match name {
        "CYPHER" => L::Cypher,
        "SQL_PGQ" => L::SqlPgq,
        "GREMLIN" => L::Gremlin,
        "GRAPHQL" => L::Graphql,
        "SPARQL" => L::Sparql,
        _ => L::Gql,
    }
}

/// The languages of a request, checked against this build and named.
fn languages(
    requested: &[buffa::EnumValue<pb::QueryLanguage>],
) -> Result<Vec<String>, ConnectError> {
    let mut out = Vec::new();
    for language in requested {
        data::check_language(language.as_known(), language)?;
        let name = language.as_known().map_or("GQL", language_name).to_string();
        if !out.contains(&name) {
            out.push(name);
        }
    }
    Ok(out)
}

fn limits_from(limits: Option<&pb::GraphLimits>) -> GraphLimits {
    limits.map_or_else(GraphLimits::default, |l| GraphLimits {
        // Each capped to the server's maximum, as `GraphLimits` documents.
        timeout_ms: l.timeout_ms.min(MAX_LIMITS.timeout_ms),
        max_rows: l.max_rows.min(MAX_LIMITS.max_rows),
        max_result_bytes: l.max_result_bytes.min(MAX_LIMITS.max_result_bytes),
        memory_bytes: l.memory_bytes.min(MAX_LIMITS.memory_bytes),
        max_path_hops: l.max_path_hops.min(MAX_LIMITS.max_path_hops),
    })
}

fn limits_to(limits: &GraphLimits) -> pb::GraphLimits {
    pb::GraphLimits {
        timeout_ms: limits.timeout_ms,
        max_rows: limits.max_rows,
        max_result_bytes: limits.max_result_bytes,
        memory_bytes: limits.memory_bytes,
        max_path_hops: limits.max_path_hops,
        ..Default::default()
    }
}

/// Runs blocking engine work (a close, a purge) on the blocking pool, off the async runtime
/// (review M8). Opens and statements run on the statement pool ([`GraphAdmin::on_pool`]).
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, ConnectError> + Send + 'static,
) -> Result<T, ConnectError> {
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or_else(|err| {
            tracing::error!(error = %err, "a blocking graph task failed");
            Err(refuse(
                ErrorCode::Internal,
                "internal",
                "internal error running the statement",
            ))
        })
}

/// The default cap on graph statements running at once: twice the cores, at most 32.
#[must_use]
pub fn default_statement_slots() -> usize {
    std::thread::available_parallelism()
        .map_or(4, std::num::NonZeroUsize::get)
        .saturating_mul(2)
        .min(32)
}

/// `UNAVAILABLE` once [`GraphAdmin::shutdown`] has started.
fn shutting_down() -> ConnectError {
    refuse(
        ErrorCode::Unavailable,
        "unavailable",
        "the graph service is shutting down",
    )
}

/// A short operation id: `op-` and 26 hex characters (D146).
fn operation_id() -> String {
    let hex = format!("{:032x}", u128::from(ulid::Ulid::generate()));
    format!("op-{}", &hex[..26])
}

impl GraphAdmin {
    /// Admin over `engine` and `catalog`, with the default retention hold.
    #[must_use]
    pub fn new(engine: Arc<Engine>, catalog: GraphCatalog) -> Self {
        let statement_slots = default_statement_slots();
        Self {
            engine,
            catalog,
            retention_hold: DEFAULT_RETENTION_HOLD,
            statements: Arc::new(tokio::sync::Semaphore::new(statement_slots)),
            statement_slots,
            pool: Arc::new(StatementPool::new(statement_slots + POOL_SPARE)),
            limits: StatementLimits::DEFAULT,
            namespaces: Arc::new(NamespaceSlots::new(namespace_cap(
                DEFAULT_NAMESPACE_STATEMENTS,
                statement_slots,
            ))),
            namespace_configured: DEFAULT_NAMESPACE_STATEMENTS,
            namespace_detached: DEFAULT_NAMESPACE_DETACHED,
            detached: Arc::default(),
            client_deadline: None,
            closed: Arc::default(),
            #[cfg(feature = "test-hooks")]
            after_open_hook: Arc::default(),
        }
    }

    /// The same admin with another retention hold.
    #[must_use]
    pub fn with_retention_hold(mut self, hold: Duration) -> Self {
        self.retention_hold = hold;
        self
    }

    /// The same admin with another cap on statements running at once (1 to
    /// [`MAX_STATEMENT_SLOTS`]).
    #[must_use]
    pub fn with_statement_slots(mut self, slots: usize) -> Self {
        let slots = slots.clamp(1, MAX_STATEMENT_SLOTS);
        self.statements = Arc::new(tokio::sync::Semaphore::new(slots));
        self.statement_slots = slots;
        self.pool = Arc::new(StatementPool::new(slots + POOL_SPARE));
        self.namespaces = Arc::new(NamespaceSlots::new(namespace_cap(
            self.namespace_configured,
            slots,
        )));
        self
    }

    /// The same admin with other server statement limits (each capped at
    /// [`StatementLimits::MAXIMUM`]); a graph's own limits apply on top.
    #[must_use]
    pub fn with_limits(mut self, limits: StatementLimits) -> Self {
        self.limits = limits.capped();
        self
    }

    /// The server's statement limits.
    #[must_use]
    pub fn limits(&self) -> StatementLimits {
        self.limits
    }

    /// The same admin with another cap on one namespace's statements at once (§48 §13.2;
    /// default [`DEFAULT_NAMESPACE_STATEMENTS`]). The cap in force is never more than the
    /// process slots less a reserve ([`namespace_cap`]), so one namespace cannot take every slot.
    #[must_use]
    pub fn with_namespace_statements(mut self, per_namespace: usize) -> Self {
        self.namespace_configured = per_namespace;
        self.namespaces = Arc::new(NamespaceSlots::new(namespace_cap(
            per_namespace,
            self.statement_slots,
        )));
        self
    }

    /// The same admin with another cap on one namespace's detached statements, across its
    /// graphs (default [`DEFAULT_NAMESPACE_DETACHED`], at least 1).
    #[must_use]
    pub fn with_namespace_detached(mut self, per_namespace: usize) -> Self {
        self.namespace_detached = per_namespace.max(1);
        self
    }

    /// The cap on one namespace's statements at once, as in force.
    #[must_use]
    pub fn namespace_statements(&self) -> usize {
        self.namespaces.per_namespace()
    }

    /// This admin for one call whose client set its own deadline (Connect's timeout header):
    /// the call's statement ends at the earlier of that and its own limit.
    #[must_use]
    pub fn for_call(&self, client_deadline: Option<std::time::Instant>) -> Self {
        Self {
            client_deadline,
            ..self.clone()
        }
    }

    /// Statements running on past their deadline or their client, on every graph (R0.8 (a);
    /// Task 27's `loams_graph_detached_statements`).
    #[must_use]
    pub fn detached_statements(&self) -> usize {
        self.detached.total()
    }

    /// The largest deadline a request may ask for: [`StatementLimits::MAXIMUM`]'s, or the
    /// engine's own `query_timeout` when that is lower, since the engine may end the statement
    /// then anyway (R0.8 (a)).
    fn timeout_ceiling(&self) -> Duration {
        self.engine
            .query_timeout()
            .map_or(StatementLimits::MAXIMUM.timeout, |engine| {
                engine.min(StatementLimits::MAXIMUM.timeout)
            })
    }

    /// Runs `work` on the statement pool (GR1 Task 6): a statement, or a disk open.
    async fn on_pool<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> Result<T, ConnectError> + Send + 'static,
    ) -> Result<T, ConnectError> {
        match self.pool.run(work).await {
            Ok(Ok(result)) => result,
            _ => {
                tracing::error!("a graph statement failed on its worker");
                Err(refuse(
                    ErrorCode::Internal,
                    "internal",
                    "internal error running the statement",
                ))
            }
        }
    }

    /// The cap on statements running at once.
    #[must_use]
    pub fn statement_slots(&self) -> usize {
        self.statement_slots
    }

    /// Statements running now, including any whose client has gone away.
    #[must_use]
    pub fn statements_in_flight(&self) -> usize {
        self.statement_slots
            .saturating_sub(self.statements.available_permits())
    }

    /// A statement slot, or `RESOURCE_EXHAUSTED` when every one is taken: the caller retries,
    /// rather than queueing behind work that may never end (R0.8).
    fn statement_slot(&self) -> Result<tokio::sync::OwnedSemaphorePermit, ConnectError> {
        if self.is_shutting_down() {
            return Err(shutting_down());
        }
        let slot = Arc::clone(&self.statements)
            .try_acquire_owned()
            .map_err(|err| match err {
                tokio::sync::TryAcquireError::NoPermits => refuse(
                    ErrorCode::ResourceExhausted,
                    "resource_exhausted",
                    format!(
                        "all {} graph statement slots on this server are busy; retry",
                        self.statement_slots
                    ),
                ),
                tokio::sync::TryAcquireError::Closed => shutting_down(),
            })?;
        // Shutdown may have started meanwhile; it waits for this slot, but need not.
        if self.is_shutting_down() {
            return Err(shutting_down());
        }
        Ok(slot)
    }

    /// Whether [`GraphAdmin::shutdown`] has started.
    #[must_use]
    pub fn is_shutting_down(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Runs `work` on a catalog graph within its limits (GR1 Task 6).
    ///
    /// 1. The graph's catalog record gives its limits, and the request's `timeout_ms` (or the
    ///    graph's timeout) and the client's own deadline give the statement's deadline, counted
    ///    from now.
    /// 2. It takes a slot of its namespace (`quota_exceeded` when none is free, §48 §13.2), is
    ///    refused when the graph already has `max_detached` statements running past their
    ///    deadline, and takes a process slot (`resource_exhausted`). Nothing queues.
    /// 3. The open and the work run as one spawned task on the statement pool that owns both
    ///    slots, so a client that goes away cannot free them early (fix round 2): they are freed
    ///    when the work ends.
    /// 4. At the deadline the RPC answers `graph_statement_timeout`. Grafeo cannot stop the
    ///    statement (R0.8), so it runs on, detached and counted, until it ends; a client that
    ///    disconnects detaches it the same way.
    async fn on_graph<T: Send + 'static>(
        &self,
        namespace: &str,
        name: &str,
        timeout_ms: u32,
        work: impl FnOnce(&Graph, &StatementLimits) -> Result<T, ConnectError> + Send + 'static,
    ) -> Result<T, ConnectError> {
        self.on_graph_held(namespace, name, timeout_ms, work)
            .await
            .map(|(result, _slots)| result)
    }

    /// [`GraphAdmin::on_graph`], answering the statement's slots with its result, so a caller
    /// that goes on using the result (a stream) keeps them (review fix 1, I4).
    async fn on_graph_held<T: Send + 'static>(
        &self,
        namespace: &str,
        name: &str,
        timeout_ms: u32,
        work: impl FnOnce(&Graph, &StatementLimits) -> Result<T, ConnectError> + Send + 'static,
    ) -> Result<(T, Slots), ConnectError> {
        let started = tokio::time::Instant::now();
        if self.is_shutting_down() {
            return Err(shutting_down());
        }
        let meta = self
            .catalog
            .get_by_name(namespace, name)
            .await
            .map_err(map_catalog)?;
        let limits = self.limits.for_graph(&meta.limits);
        let timeout = limits.timeout_for(timeout_ms, self.timeout_ceiling());
        let mut deadline = started + timeout;
        if let Some(client) = self.client_deadline {
            deadline = deadline.min(tokio::time::Instant::from_std(client));
        }
        let namespace_slot = self.namespaces.try_acquire(namespace).ok_or_else(|| {
            refuse_with(
                ErrorCode::ResourceExhausted,
                "quota_exceeded",
                format!(
                    "namespace {namespace} already runs {} graph statements at once; retry",
                    self.namespaces.per_namespace()
                ),
                &[("quota", "concurrent_statements")],
            )
        })?;
        let namespace_detached = self.detached.of_namespace(namespace);
        if namespace_detached >= self.namespace_detached {
            return Err(refuse_with(
                ErrorCode::ResourceExhausted,
                "quota_exceeded",
                format!(
                    "namespace {namespace} has {namespace_detached} graph statements still \
                     running past their deadline; retry when they end"
                ),
                &[("quota", "detached_statements")],
            ));
        }
        let slot = self.statement_slot()?;
        // Once a statement of this graph is detached, every one in flight counts against
        // `max_detached` (review fix 1, I2).
        let watch = Watch::admit(
            Arc::clone(&self.detached),
            namespace,
            name,
            limits.max_detached,
        )
        .ok_or_else(|| {
            refuse(
                ErrorCode::ResourceExhausted,
                "resource_exhausted",
                format!(
                    "graph {namespace}/{name} has {} statements running past their deadline and \
                     {} more in flight, against a limit of {}; retry when they end",
                    self.detached.of(namespace, name),
                    self.detached.running(namespace, name),
                    limits.max_detached
                ),
            )
        })?;
        let task = {
            let admin = self.clone();
            let watch = watch.clone();
            tokio::spawn(async move {
                // Settles the detached count however the task ends.
                let _finished = Finished(watch);
                // Freed when the task ends, unless the result carries them on.
                let slots = Slots {
                    _process: slot,
                    _namespace: namespace_slot,
                };
                let graph = admin.open_meta(&meta).await?;
                let result = admin
                    .on_pool(move || {
                        let result = work(&graph, &limits);
                        drop(graph);
                        result
                    })
                    .await?;
                Ok((result, slots))
            })
        };
        // Dropped when this call ends, however it ends: a call whose client went away (this
        // future dropped) or that timed out leaves the statement detached.
        let _waiting = Waiting(watch);
        match tokio::time::timeout_at(deadline, task).await {
            Ok(joined) => joined.unwrap_or_else(|err| {
                tracing::error!(error = %err, "a graph statement task failed");
                Err(refuse(
                    ErrorCode::Internal,
                    "internal",
                    "internal error running the statement",
                ))
            }),
            Err(_) => Err(refuse(
                ErrorCode::DeadlineExceeded,
                "graph_statement_timeout",
                format!(
                    "the statement ran past its {} ms deadline; it is detached and ends on its \
                     own (Loams cannot stop a running graph statement yet)",
                    deadline.saturating_duration_since(started).as_millis()
                ),
            )),
        }
    }

    /// The engine.
    #[must_use]
    pub fn engine(&self) -> &Arc<Engine> {
        &self.engine
    }

    /// The catalog.
    #[must_use]
    pub fn catalog(&self) -> &GraphCatalog {
        &self.catalog
    }

    /// Fills a `Graph` message: the catalog record, with the engine's state and counts when the
    /// graph is open.
    fn message(&self, meta: &GraphMeta) -> pb::Graph {
        let open = self
            .open_graph(&meta.namespace, &meta.name)
            .filter(|g| g.id().is_some_and(|id| id.to_string() == meta.id));
        let state = match (meta.state, open.as_ref().map(|g| g.state())) {
            (CatalogState::Deleting { .. }, _) => pb::GraphState::Deleting,
            (_, Some(GraphState::Poisoned)) => pb::GraphState::Reloading,
            (_, Some(GraphState::Failed)) => pb::GraphState::Failed,
            (_, Some(GraphState::Ready)) => pb::GraphState::Ready,
            // Listed and not open: it is served, and opens on its next statement. (EVICTED is
            // Task 14's, for a graph closed to free memory.)
            (_, None) => pb::GraphState::Ready,
        };
        let stats = open.as_ref().map(|g| {
            let (nodes, edges) = g.counts();
            pb::GraphStats {
                nodes,
                edges,
                ..Default::default()
            }
        });
        pb::Graph {
            namespace: meta.namespace.clone(),
            name: meta.name.clone(),
            id: meta.id.clone(),
            mode: match meta.mode {
                GraphMode::Owned => pb::GraphMode::Owned,
                GraphMode::Linked => pb::GraphMode::Linked,
            }
            .into(),
            languages: meta
                .languages
                .iter()
                .map(|l| language_value(l).into())
                .collect(),
            limits: limits_to(&meta.limits).into(),
            replicas: meta.replicas,
            state: state.into(),
            stats: stats.into(),
            create_time: Timestamp {
                seconds: i64::try_from(meta.created_at_ms / 1000).unwrap_or(i64::MAX),
                nanos: i32::try_from((meta.created_at_ms % 1000) * 1_000_000).unwrap_or(0),
                ..Default::default()
            }
            .into(),
            version: meta.version,
            ..Default::default()
        }
    }

    /// The engine's open graph under a name, if any.
    fn open_graph(&self, namespace: &str, name: &str) -> Option<Arc<Graph>> {
        self.engine
            .list(Some(namespace))
            .ok()?
            .into_iter()
            .find(|g| g.name() == name)
    }

    /// Opens a catalog graph for a statement: from its own storage, lazily, and never a stale
    /// graph of the same name that a delete could not close.
    ///
    /// # Errors
    ///
    /// `NOT_FOUND`/`graph_not_found` when the catalog has no such graph, or the engine's error.
    pub async fn open(&self, namespace: &str, name: &str) -> Result<Arc<Graph>, ConnectError> {
        if self.is_shutting_down() {
            return Err(shutting_down());
        }
        let meta = self
            .catalog
            .get_by_name(namespace, name)
            .await
            .map_err(map_catalog)?;
        self.open_meta(&meta).await
    }

    /// [`GraphAdmin::open`] for a catalog record the caller has just read.
    async fn open_meta(&self, meta: &GraphMeta) -> Result<Arc<Graph>, ConnectError> {
        let (namespace, name) = (meta.namespace.as_str(), meta.name.as_str());
        if self.is_shutting_down() {
            return Err(shutting_down());
        }
        let graph = self.open_catalog_graph(meta).await?;
        // A shutdown that started while this graph opened may already have closed the others:
        // close this one too rather than leave it open (review fix 1, I3). A statement holds a
        // slot across its open, so shutdown waits for it; this covers any other caller.
        if self.is_shutting_down() {
            drop(graph);
            let (engine, ns, nm) = (
                Arc::clone(&self.engine),
                namespace.to_string(),
                name.to_string(),
            );
            let _ = blocking(move || {
                let _ = engine.close(&ns, &nm);
                Ok(())
            })
            .await;
            return Err(shutting_down());
        }
        Ok(graph)
    }

    /// [`GraphAdmin::open_meta`] without the shutdown checks. Opens run on the statement pool
    /// (Task 4 review M8, GR1 Task 6).
    async fn open_catalog_graph(&self, meta: &GraphMeta) -> Result<Arc<Graph>, ConnectError> {
        let (namespace, name) = (meta.namespace.as_str(), meta.name.as_str());
        let id = meta.graph_id().map_err(map_catalog)?;
        // Already open with this id: the one up-to-date catalog read above (a pointer read; the
        // document is cached by pointer version) is the whole cost of a statement, and a delete
        // is effective for every statement that starts after `DeleteGraph` returns (re-review 2).
        if let Some(graph) = self
            .open_graph(namespace, name)
            .filter(|g| g.id() == Some(id))
        {
            if !graph.is_poisoned_or_failed() {
                return Ok(graph);
            }
            let engine = Arc::clone(&self.engine);
            return self
                .on_pool(move || engine.reopen_if_poisoned(graph).map_err(map_engine))
                .await;
        }
        for _ in 0..2 {
            let graph = {
                let (engine, ns, nm) = (
                    Arc::clone(&self.engine),
                    namespace.to_string(),
                    name.to_string(),
                );
                self.on_pool(move || {
                    Graph::open_or_existing(&engine, &ns, &nm, || {
                        OpenSpec::for_catalog(&engine, id)
                    })
                    .map_err(map_engine)
                })
                .await?
            };
            if graph.id() == Some(id) {
                #[cfg(feature = "test-hooks")]
                self.after_open().await;
                // Re-check after the (possibly slow) open: a delete that landed meanwhile must
                // not leave this graph open and serving (review M1). Cheap: the document is
                // cached unless the pointer moved.
                let still = self.catalog.get_by_name(namespace, name).await;
                if !matches!(&still, Ok(now) if now.id == meta.id) {
                    drop(graph);
                    let (engine, ns, nm) = (
                        Arc::clone(&self.engine),
                        namespace.to_string(),
                        name.to_string(),
                    );
                    let _ = blocking(move || {
                        let _ = engine.close(&ns, &nm);
                        Ok(())
                    })
                    .await;
                    return Err(match still {
                        Err(err) => map_catalog(err),
                        Ok(_) => map_catalog(CatalogError::NotFound {
                            namespace: namespace.to_string(),
                            name: name.to_string(),
                        }),
                    });
                }
                let engine = Arc::clone(&self.engine);
                return self
                    .on_pool(move || engine.reopen_if_poisoned(graph).map_err(map_engine))
                    .await;
            }
            // An older graph of this name is still open: close it and open this one.
            drop(graph);
            let (engine, ns, nm) = (
                Arc::clone(&self.engine),
                namespace.to_string(),
                name.to_string(),
            );
            blocking(move || {
                engine.close(&ns, &nm).map(|_| ()).map_err(|err| {
                    refuse(ErrorCode::Unavailable, "graph_reloading", err.to_string())
                })
            })
            .await?;
        }
        Err(refuse(
            ErrorCode::Unavailable,
            "graph_reloading",
            format!("graph {namespace}/{name} is being replaced; retry"),
        ))
    }

    /// **Tests only** (feature `test-hooks`): runs between the engine open and the catalog
    /// re-check of [`GraphAdmin::open`] (review M1).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn set_after_open_hook(&self, hook: Option<crate::catalog::AckHook>) {
        if let Ok(mut slot) = self.after_open_hook.lock() {
            *slot = hook;
        }
    }

    #[cfg(feature = "test-hooks")]
    async fn after_open(&self) {
        let hook = self
            .after_open_hook
            .lock()
            .ok()
            .and_then(|slot| slot.clone());
        if let Some(hook) = hook {
            let _ = hook().await;
        }
    }

    /// `GetEngineInfo`.
    pub async fn get_engine_info(&self, _req: pb::GetEngineInfoRequest) -> pb::EngineInfo {
        engine_info(&self.engine)
    }

    /// `CreateGraph`: idempotent by `idempotency_key`; the name must be free.
    ///
    /// # Errors
    ///
    /// `INVALID_ARGUMENT` for a bad name, `UNIMPLEMENTED` for LINKED or a language this build
    /// lacks, `ALREADY_EXISTS`, or `UNAVAILABLE` when the catalog is.
    pub async fn create_graph(
        &self,
        req: pb::CreateGraphRequest,
    ) -> Result<pb::Graph, ConnectError> {
        crate::engine::validate_names(&req.namespace, &req.name).map_err(map_engine)?;
        if req.mode.as_known() == Some(pb::GraphMode::Linked) {
            return Err(refuse(
                ErrorCode::Unimplemented,
                "not_implemented",
                "LINKED graphs are not served yet (GR1 Task 17)",
            ));
        }
        check_replicas(req.replicas)?;
        let new = NewGraph {
            mode: GraphMode::Owned,
            languages: languages(&req.languages)?,
            limits: limits_from(req.limits.as_option()),
            replicas: req.replicas,
            idempotency_key: check_key("idempotency_key", &req.idempotency_key)?,
        };
        let meta = self
            .catalog
            .create(&req.namespace, &req.name, new)
            .await
            .map_err(map_catalog)?;
        Ok(self.message(&meta))
    }

    /// `GetGraph`.
    ///
    /// # Errors
    ///
    /// `INVALID_ARGUMENT`, `NOT_FOUND` or `UNAVAILABLE`.
    pub async fn get_graph(&self, req: pb::GetGraphRequest) -> Result<pb::Graph, ConnectError> {
        let meta = self
            .catalog
            .get_by_name(&req.namespace, &req.name)
            .await
            .map_err(map_catalog)?;
        Ok(self.message(&meta))
    }

    /// `ListGraphs` (AIP-158).
    ///
    /// # Errors
    ///
    /// `INVALID_ARGUMENT` for a bad namespace, size or token, or `UNAVAILABLE`.
    pub async fn list_graphs(
        &self,
        req: pb::ListGraphsRequest,
    ) -> Result<pb::ListGraphsResponse, ConnectError> {
        let page = self
            .catalog
            .list(&req.namespace, req.page_size, &req.page_token)
            .await
            .map_err(map_catalog)?;
        Ok(pb::ListGraphsResponse {
            graphs: page.graphs.iter().map(|g| self.message(g)).collect(),
            next_page_token: page.next_page_token.unwrap_or_default(),
            ..Default::default()
        })
    }

    /// `UpdateGraph`: the fields `update_mask` names (`languages`, `limits`, `replicas`; all
    /// three when the mask is empty), at `expected_version` when given.
    ///
    /// # Errors
    ///
    /// `INVALID_ARGUMENT` for another field or a bad value, `NOT_FOUND`, `ABORTED` with
    /// `graph_catalog_version_mismatch`, or `UNAVAILABLE`.
    pub async fn update_graph(
        &self,
        req: pb::UpdateGraphRequest,
    ) -> Result<pb::Graph, ConnectError> {
        let graph = req.graph.as_option().cloned().unwrap_or_default();
        let mut paths: Vec<String> = req
            .update_mask
            .as_option()
            .map(|m| m.paths.clone())
            .unwrap_or_default();
        if paths.is_empty() {
            paths = UPDATABLE.iter().map(ToString::to_string).collect();
        }
        if let Some(bad) = paths.iter().find(|p| !UPDATABLE.contains(&p.as_str())) {
            return Err(refuse(
                ErrorCode::InvalidArgument,
                "invalid_argument",
                format!(
                    "update_mask path {bad:?} is not updatable; only languages, limits and replicas are"
                ),
            ));
        }
        if paths.iter().any(|p| p == "replicas") {
            check_replicas(graph.replicas)?;
        }
        check_key("idempotency_key", &req.idempotency_key)?;
        let new_languages = languages(&graph.languages)?;
        let new_limits = limits_from(graph.limits.as_option());
        let meta = self
            .catalog
            .update(
                &graph.namespace,
                &graph.name,
                req.expected_version,
                |meta| {
                    for path in &paths {
                        match path.as_str() {
                            "languages" => {
                                meta.languages = if new_languages.is_empty() {
                                    vec!["GQL".to_string()]
                                } else {
                                    new_languages.clone()
                                };
                            }
                            "limits" => meta.limits = new_limits,
                            "replicas" => meta.replicas = graph.replicas,
                            _ => return Err(CatalogError::Invalid(format!("path {path}"))),
                        }
                    }
                    Ok(())
                },
            )
            .await
            .map_err(map_catalog)?;
        Ok(self.message(&meta))
    }

    /// `DeleteGraph`: the graph leaves the catalog's listing and its engine is closed at once;
    /// its storage is purged by [`GraphAdmin::purge_expired`] after the retention hold. Answers
    /// the finished operation. A retry with the same `idempotency_key` answers it again.
    ///
    /// # Errors
    ///
    /// `INVALID_ARGUMENT`, `NOT_FOUND` or `UNAVAILABLE`.
    pub async fn delete_graph(
        &self,
        req: pb::DeleteGraphRequest,
    ) -> Result<ops::Operation, ConnectError> {
        let key = check_key("idempotency_key", &req.idempotency_key)?;
        let meta = self
            .catalog
            .mark_deleting(&req.namespace, &req.name, key)
            .await
            .map_err(map_catalog)?;
        if let Some(open) = self.open_graph(&req.namespace, &req.name)
            && open.id().is_some_and(|id| id.to_string() == meta.id)
        {
            drop(open);
            // A holder still mid-statement keeps it open; the catalog no longer serves it, and
            // `open` replaces it if the name is created again.
            let (engine, ns, nm) = (
                Arc::clone(&self.engine),
                req.namespace.clone(),
                req.name.clone(),
            );
            let _ = blocking(move || {
                if let Err(err) = engine.close(&ns, &nm) {
                    tracing::warn!(namespace = %ns, name = %nm, error = %err, "a deleted graph is still in use; it closes when released");
                }
                Ok(())
            })
            .await;
        }
        let mut operation = ops::Operation {
            id: operation_id(),
            kind: "graph.delete".to_string(),
            namespace: req.namespace.clone(),
            state: ops::OperationState::Succeeded.into(),
            ..Default::default()
        };
        operation.target.insert("graph".to_string(), req.name);
        operation.target.insert("graph_id".to_string(), meta.id);
        Ok(operation)
    }

    /// `GetSchema`: labels, edge types, property keys, counts and indexes. Opens the graph.
    ///
    /// # Errors
    ///
    /// `NOT_FOUND`, or the engine's error.
    pub async fn get_schema(
        &self,
        req: pb::GetSchemaRequest,
    ) -> Result<pb::GraphSchema, ConnectError> {
        let summary = self
            .on_graph(&req.namespace, &req.name, 0, |graph, _| {
                graph.schema().map_err(map_engine)
            })
            .await?;
        Ok(pb::GraphSchema {
            labels: summary
                .labels
                .into_iter()
                .map(|(label, count)| pb::LabelInfo {
                    label,
                    count,
                    ..Default::default()
                })
                .collect(),
            edge_types: summary
                .edge_types
                .into_iter()
                .map(|(r#type, count)| pb::EdgeTypeInfo {
                    r#type,
                    count,
                    ..Default::default()
                })
                .collect(),
            property_keys: summary.property_keys,
            indexes: summary
                .indexes
                .into_iter()
                .map(|(name, target, kind)| pb::IndexInfo {
                    name,
                    target,
                    kind,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        })
    }

    /// `Execute` on a catalog graph, opening it lazily.
    ///
    /// # Errors
    ///
    /// As [`data::execute`], and `NOT_FOUND` for a graph the catalog does not list.
    pub async fn execute(
        &self,
        req: pb::ExecuteRequest,
    ) -> Result<pb::ExecuteResponse, ConnectError> {
        let (namespace, name, timeout) = (req.namespace.clone(), req.graph.clone(), req.timeout_ms);
        self.on_graph(&namespace, &name, timeout, move |graph, limits| {
            data::execute_on(graph, req, limits)
        })
        .await
    }

    /// `ExecuteStream` on a catalog graph: every row of the statement, in chunks of at most
    /// 1 000 rows (or `chunk_rows`) and 1 MiB (GR1 Task 6). The request's `max_rows` caps the
    /// whole stream (0: no cap); the unary row and byte limits do not apply. The statement runs
    /// to its end, within its deadline, before the first chunk (R6.3).
    ///
    /// # Errors
    ///
    /// As [`GraphAdmin::execute`], before any chunk.
    pub async fn execute_stream(
        &self,
        req: pb::ExecuteStreamRequest,
    ) -> Result<connectrpc::ServiceStream<pb::ResultChunk>, ConnectError> {
        let request = req.request.as_option().cloned().unwrap_or_default();
        let (namespace, name, timeout) = (
            request.namespace.clone(),
            request.graph.clone(),
            request.timeout_ms,
        );
        let max_rows = (request.max_rows > 0).then_some(request.max_rows as usize);
        let ((result, max_bytes), slots) = self
            .on_graph_held(&namespace, &name, timeout, move |graph, limits| {
                data::run_statement(graph, &request, limits, max_rows)
                    .map(|result| (result, limits.max_stream_bytes))
            })
            .await?;
        // The stream keeps the statement's slots until it ends or is dropped (I4).
        Ok(Box::pin(futures::stream::iter(
            stream::Chunks::new(result, req.chunk_rows, max_bytes)
                .holding(slots)
                .map(Ok),
        )))
    }

    /// `ExecuteBatch` on a catalog graph, opening it lazily.
    ///
    /// # Errors
    ///
    /// As [`data::execute_batch`].
    pub async fn execute_batch(
        &self,
        req: pb::ExecuteBatchRequest,
    ) -> Result<pb::ExecuteBatchResponse, ConnectError> {
        let (namespace, name, timeout) = (req.namespace.clone(), req.graph.clone(), req.timeout_ms);
        self.on_graph(&namespace, &name, timeout, move |graph, limits| {
            data::execute_batch_on(graph, req, limits)
        })
        .await
    }

    /// `Explain` on a catalog graph.
    ///
    /// # Errors
    ///
    /// As [`data::explain`].
    pub async fn explain(&self, req: pb::ExplainRequest) -> Result<pb::Plan, ConnectError> {
        let (namespace, name, timeout) = (req.namespace.clone(), req.graph.clone(), req.timeout_ms);
        self.on_graph(&namespace, &name, timeout, move |graph, limits| {
            data::explain_on(graph, req, limits)
        })
        .await
    }

    /// Purges every deleted graph whose retention hold (`hold`, or the admin's own when `None`)
    /// has passed: its engine is closed, its storage directory removed and its catalog record
    /// dropped. Answers how many were purged. Meant for a periodic sweep (Task 5 wires it).
    ///
    /// # Errors
    ///
    /// `UNAVAILABLE` when the catalog is.
    pub async fn purge_expired(&self, hold: Duration) -> Result<usize, ConnectError> {
        let now = self.catalog.now_ms();
        let before = now.saturating_sub(u64::try_from(hold.as_millis()).unwrap_or(u64::MAX));
        let mut purged = 0;
        for namespace in self.catalog.namespaces().await.map_err(map_catalog)? {
            // A namespace whose catalog fails is logged and skipped; the sweep goes on (M3).
            let deleting = match self.catalog.deleting(&namespace, before).await {
                Ok(deleting) => deleting,
                Err(err) => {
                    tracing::warn!(%namespace, error = %err, "purging deleted graphs: the namespace's catalog failed; skipped");
                    continue;
                }
            };
            for meta in deleting {
                if let Some(open) = self.open_graph(&namespace, &meta.name)
                    && open.id().is_some_and(|id| id.to_string() == meta.id)
                {
                    drop(open);
                    let (engine, ns, nm) = (
                        Arc::clone(&self.engine),
                        namespace.clone(),
                        meta.name.clone(),
                    );
                    let closed = blocking(move || Ok(engine.close(&ns, &nm).is_ok())).await;
                    if !matches!(closed, Ok(true)) {
                        // Still in use: purge on a later sweep.
                        continue;
                    }
                }
                if let (Some(data_dir), Ok(id)) = (self.engine.data_dir(), meta.graph_id()) {
                    let dir = data_dir.join("graphs").join(id.to_string());
                    let removed = blocking(move || {
                        Ok(match std::fs::remove_dir_all(&dir) {
                            Ok(()) => true,
                            Err(err) if err.kind() == std::io::ErrorKind::NotFound => true,
                            Err(err) => {
                                tracing::warn!(dir = %dir.display(), error = %err, "could not purge a deleted graph's storage");
                                false
                            }
                        })
                    })
                    .await;
                    if !matches!(removed, Ok(true)) {
                        continue;
                    }
                }
                match self.catalog.purge(&namespace, &meta.id).await {
                    Ok(true) => purged += 1,
                    Ok(false) => {}
                    Err(err) => {
                        tracing::warn!(%namespace, id = %meta.id, error = %err, "purging a deleted graph's record failed; retried on the next sweep");
                    }
                }
            }
        }
        Ok(purged)
    }

    /// Deletes catalog documents older than `grace` that no reader still needs (the catalog's
    /// [`GraphCatalog::sweep_documents`]); run with [`crate::catalog::DOCUMENT_GRACE`] beside
    /// [`GraphAdmin::purge_expired`].
    ///
    /// # Errors
    ///
    /// `UNAVAILABLE` when the catalog is.
    pub async fn sweep_documents(&self, grace: Duration) -> Result<usize, ConnectError> {
        self.catalog
            .sweep_documents(grace)
            .await
            .map_err(map_catalog)
    }

    /// The retention hold this admin purges with.
    #[must_use]
    pub fn retention_hold(&self) -> Duration {
        self.retention_hold
    }

    /// Stops serving and closes every open graph, releasing its storage (for a shutdown or a test
    /// restart; review fix 1, I3). Answers how many statements were still running when it gave
    /// up waiting.
    ///
    /// 1. From the start, `open` and every statement answer `UNAVAILABLE`.
    /// 2. It waits, at most `wait`, for running statements to end and free their slots.
    /// 3. It closes each graph on the blocking pool, flushing its WAL.
    ///
    /// A statement still running after `wait` (one whose client went away, R0.8: Grafeo cannot
    /// stop it) is detached and logged. Its graph cannot be closed while the statement holds it,
    /// so it stays registered in the engine until the engine itself drops (the last `Arc` of
    /// the engine and of the statement's handle). Grafeo's `Drop for GrafeoDB` calls the same
    /// `close` (WAL sync and checkpoint), so the data is flushed then, but a failure there is
    /// only logged by Grafeo, not reported to Loams. It runs on a statement pool thread, not
    /// tokio's blocking pool, so it no longer delays the runtime's drop (GR1 Task 6, R6.5):
    /// the process exits with it unfinished, and its transaction never commits (its client
    /// was already answered with an error). Task 26's watchdog is what bounds such a statement
    /// while the process runs.
    pub async fn shutdown(&self, wait: Duration) -> usize {
        self.closed.store(true, Ordering::SeqCst);
        let slots = u32::try_from(self.statement_slots).unwrap_or(u32::MAX);
        // Every slot, so nothing is running; held until the graphs are closed.
        let drained =
            tokio::time::timeout(wait, Arc::clone(&self.statements).acquire_many_owned(slots))
                .await;
        let running = match &drained {
            Ok(Ok(_)) => 0,
            _ => self.statements_in_flight(),
        };
        if running > 0 {
            tracing::warn!(
                running,
                ?wait,
                "graph statements still running at shutdown are detached; their graphs stay registered until the engine drops, and they end with the process"
            );
        }
        for graph in self.engine.list(None).unwrap_or_default() {
            let (namespace, name) = (graph.namespace().to_string(), graph.name().to_string());
            drop(graph);
            let (engine, ns, nm) = (Arc::clone(&self.engine), namespace.clone(), name.clone());
            if let Err(err) = blocking(move || engine.close(&ns, &nm).map_err(map_engine)).await {
                tracing::warn!(%namespace, %name, error = %err, "closing a graph at shutdown failed");
            }
        }
        drop(drained);
        running
    }
}

/// Held by a statement's task: settles its [`Watch`] when the task ends, however it ends.
struct Finished(Watch);

impl Drop for Finished {
    fn drop(&mut self) {
        self.0.finish();
    }
}

/// Held by the caller waiting for a statement: when the wait ends without the statement having
/// ended (its deadline, or its client went away), the statement is detached.
struct Waiting(Watch);

impl Drop for Waiting {
    fn drop(&mut self) {
        self.0.abandon();
    }
}

/// A statement's process and namespace slots, freed when dropped.
pub(crate) struct Slots {
    _process: tokio::sync::OwnedSemaphorePermit,
    _namespace: tokio::sync::OwnedSemaphorePermit,
}
