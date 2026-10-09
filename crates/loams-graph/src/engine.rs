//! The embedded engine wrapper: opening graphs and running statements against Grafeo.
//!
//! This is the whole of Loams's in-process graph surface. Everything the Connect-RPC service needs
//! is here, so the service layer above is transport and nothing else, and the engine can be tested
//! without a server.
//!
//! Three things in this module are worth reading before changing anything:
//!
//! * **A statement reaches Grafeo unchanged.** Nothing here parses, rewrites or re-indents a
//!   statement; [`Graph::execute`] hands the caller's bytes to the engine and reports what came
//!   back. The one thing that *reads* a statement is [`writes`], the read-only guard, and it reads
//!   without modifying.
//! * **What the engine does not measure is reported as absent, not as zero.** Grafeo 0.5.43's
//!   `QueryResult` carries `execution_time_ms` and `rows_scanned` and nothing else; see
//!   [`GraphResult`] for exactly which of the contract's counters have an engine behind them.
//! * **The read-only guard is deliberately lopsided.** A read refused as a write costs a caller one
//!   retry; a write let through as a read costs correctness. So the guard knows the keywords that
//!   cannot write and treats everything else as a write.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use grafeo::{Error as GrafeoError, GrafeoDB, QueryResult, Value};
use loams_proto::loams::graph::v1::QueryLanguage;

use crate::classify::{Access, gate};
use grafeo_common::types::PropertyKey;

/// What this crate can be wrong about, all of it recoverable by the caller.
///
/// `PartialEq` so a caller -- and this crate's own tests -- can assert *which* refusal happened
/// without matching on the message text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GraphError {
    /// The engine refused a statement. Grafeo's own error text, kept whole: it carries the syntax
    /// span and a hint, and dropping either makes a GQL mistake much harder to fix than a message
    /// alone would.
    #[error("the graph engine refused the statement: {0}")]
    Engine(String),
    /// A namespace and name pair was opened with a different configuration than an existing graph,
    /// which would mean two callers disagree about the same graph's storage.
    #[error("graph {namespace}/{name} is already open with a different configuration")]
    Conflict {
        /// The namespace the graph was opened under.
        namespace: String,
        /// The name the graph was opened under.
        name: String,
    },
    /// A statement was marked read-only and tried to write.
    #[error("the statement writes, and this request is read-only")]
    ReadOnly,
    /// A statement was empty. Refused rather than run: the engine would answer a syntax error, and
    /// a batch that silently skipped a blank statement would report fewer results than statements.
    #[error("a statement is empty; send no statement rather than a blank one")]
    EmptyStatement,
    /// The engine does not have the query language this build was compiled with. Grafeo gates
    /// languages behind feature flags, and this crate's build enables only GQL (D634 (a)).
    #[error(
        "query language {0} is not available in this build; loams-graph compiles Grafeo with GQL only"
    )]
    LanguageUnavailable(&'static str),
    /// A wire value the engine cannot hold exactly (see [`crate::value::from_proto`]).
    #[error("invalid value: {0}")]
    InvalidValue(String),
    /// A parameter name in a statement has no binding.
    #[error(
        "statement refers to {name}, which has no binding; a graph value is never interpolated into the text"
    )]
    UnboundParameter {
        /// The parameter the statement named.
        name: String,
    },
    /// `START TRANSACTION`, `COMMIT`, `ROLLBACK` or a savepoint statement: the RPC owns the
    /// transaction (§48 §7.3).
    #[error(
        "GQL's transaction statements are not served: one RPC is one transaction (use ExecuteBatch with atomic for several statements)"
    )]
    TransactionStatement,
    /// A statement no caller may run: file access (R0.11) or graph management (R0.10 (b)).
    #[error("the statement is not allowed: {what}")]
    StatementNotAllowed {
        /// File access (`PERMISSION_DENIED`) rather than graph management (`FAILED_PRECONDITION`).
        file_access: bool,
        /// What was refused.
        what: String,
    },
    /// A variable-length pattern with no upper bound, or one above the limit (R0.8 (b)).
    #[error(
        "a variable-length pattern must have an upper bound of at most {max_hops} hops, for example *1..{max_hops}"
    )]
    UnboundedPath {
        /// The largest upper bound allowed.
        max_hops: u32,
    },
    /// The engine panicked inside this call. The graph is poisoned and reopens on its next call
    /// (R0.13).
    #[error("the graph engine failed inside this call; the graph is being reloaded")]
    EnginePanic,
    /// A statement chains more operators than the parser can safely take (re-review 2c).
    #[error(
        "the statement chains {links} operators and keywords; at most {limit} are accepted (split it, or bind values as parameters)"
    )]
    TooComplex {
        /// What the statement holds.
        links: usize,
        /// The limit.
        limit: usize,
    },
    /// The graph is poisoned by an earlier panic and has not been reopened yet.
    #[error("the graph is reloading after an engine failure; retry")]
    Reloading,
    /// The engine's own `query_timeout` stopped the statement (the backstop, R0.8 (a)).
    #[error("the statement ran past the graph engine's time limit")]
    StatementTimeout,
    /// An engine panic lost the graph and it cannot be reopened from storage (an in-memory graph,
    /// or a poisoned engine that would not close cleanly). It is never served again empty, so an
    /// acknowledged write is never silently lost (security review I2).
    #[error(
        "the graph was lost to an engine failure and cannot be reopened from storage; delete it and create it again"
    )]
    Failed,
}

impl GraphError {
    /// The `loams.errors.v1.ErrorInfo.reason` this error is answered with (§48 §8.3).
    #[must_use]
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Engine(text) if text.contains("syntax error") => "gql_syntax_error",
            Self::Engine(_)
            | Self::EmptyStatement
            | Self::UnboundParameter { .. }
            | Self::InvalidValue(_)
            | Self::TooComplex { .. } => "invalid_argument",
            Self::Conflict { .. } => "already_exists",
            Self::ReadOnly => "graph_read_only",
            Self::LanguageUnavailable(_) => "graph_language_disabled",
            Self::TransactionStatement => "graph_transaction_statement",
            Self::StatementNotAllowed { .. } => "graph_statement_not_allowed",
            Self::UnboundedPath { .. } => "graph_unbounded_path",
            Self::EnginePanic | Self::Failed => "graph_engine_panic",
            Self::Reloading => "graph_reloading",
            Self::StatementTimeout => "graph_statement_timeout",
        }
    }
}

/// How to open a graph.
///
/// Storage is either in memory (tests, and an engine with no data directory) or
/// `<data_dir>/graphs/<graph_id>/` (GR1 Task 3): there is no other way to name it, so no caller,
/// and no RPC field, can point a graph at a path of its choosing (Review Focus 4).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpenSpec {
    /// The catalog id the graph is opened for, when there is one (Task 4).
    id: Option<crate::GraphId>,
    /// The graph's storage directory, `None` for in memory.
    dir: Option<PathBuf>,
    /// Open read-only: every writing statement is refused before it reaches the engine.
    pub read_only: bool,
}

impl OpenSpec {
    /// An in-memory graph: it dies with this process.
    #[must_use]
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// A persistent graph under the engine's data directory, at `graphs/<id>/`. `None` when the
    /// engine has no data directory.
    #[must_use]
    pub fn persistent(engine: &Engine, id: crate::GraphId) -> Option<Self> {
        let data_dir = engine.data_dir.as_ref()?;
        Some(Self {
            id: Some(id),
            dir: Some(data_dir.join("graphs").join(id.to_string())),
            read_only: false,
        })
    }

    /// An in-memory graph for a catalog id: what an engine with no data directory opens (dev).
    #[must_use]
    pub fn in_memory_for(id: crate::GraphId) -> Self {
        Self {
            id: Some(id),
            ..Self::default()
        }
    }

    /// The spec a catalog graph opens with: persistent under the engine's data directory, or in
    /// memory when the engine has none.
    #[must_use]
    pub fn for_catalog(engine: &Engine, id: crate::GraphId) -> Self {
        Self::persistent(engine, id).unwrap_or_else(|| Self::in_memory_for(id))
    }

    /// The same spec, read-only.
    #[must_use]
    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    fn database_file(&self) -> Option<PathBuf> {
        self.dir.as_ref().map(|dir| dir.join("graph.grafeo"))
    }
}

/// A graph's shape, for `GetSchema`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SchemaSummary {
    /// `(label, node count)`, by label.
    pub labels: Vec<(String, u64)>,
    /// `(edge type, edge count)`, by type.
    pub edge_types: Vec<(String, u64)>,
    /// Every property key, sorted.
    pub property_keys: Vec<String>,
    /// `(name, target, kind)`, by name.
    pub indexes: Vec<(String, String, String)>,
}

/// Whether a graph is serving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphState {
    /// Serving.
    Ready,
    /// An engine call panicked; the graph reopens on its next call.
    Poisoned,
    /// An engine call panicked and the graph cannot be reopened from storage.
    Failed,
}

/// One row of a result. Values positionally aligned with the result's column names, because a graph
/// result legitimately repeats a key across a path and a map would collapse those duplicates.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphRow {
    /// One row's values, as the engine answered them. The wire form is
    /// [`crate::value::to_proto`]'s, which keeps every type (GR1 Task 2); the fabric-era JSON form
    /// turned every integer into a double.
    pub values: Vec<Value>,
}

/// One statement of a batch, with its parameters already bound.
///
/// Parameters are the engine's own value type, not JSON: the transport layer converts, so the engine
/// never has to know what protobuf is. A graph value travels as a parameter and never as part of the
/// text — interpolating one would let a value change the statement's meaning, which is the whole
/// reason the contract's `Statement` has a `parameters` map beside its `statement` string.
#[derive(Debug, Clone, Default)]
pub struct BatchStatement {
    /// The statement text, which reaches the engine unchanged.
    pub text: String,
    /// Parameters bound by name, as `$name` references in the text resolve against.
    pub parameters: HashMap<String, Value>,
}

impl BatchStatement {
    /// A statement with no bindings, which is the common case and the one a test or a
    /// single-statement caller wants. A batch that binds parameters builds the struct directly.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            parameters: HashMap::new(),
        }
    }
}

/// A statement's result: its rows and what it cost.
///
/// The contract (`loams.graph.v1.ExecuteResponse`) asks for five counters. What Grafeo 0.5.43 can
/// actually answer, read out of `grafeo_engine::database::QueryResult` and its two setters in
/// `session/mod.rs`:
///
/// | field | source | notes |
/// |---|---|---|
/// | `rows_affected` | **none** | `QueryResult` has no affected-count field, so this stays `None`. |
/// | `rows_read` | `QueryResult::rows_scanned` | the engine sets it to `result.rows.len()` — rows **returned**, not rows scanned. |
/// | `bytes_read` | **none** | no byte counter exists in `grafeo-{core,common,engine,storage}` 0.5.43. |
/// | `elapsed_nanos` | `QueryResult::execution_time_ms` | the engine's own `Instant`, in milliseconds. |
///
/// `None` therefore means "this version of the engine does not count it", and the service layer
/// says so in one place rather than putting a zero on the wire that reads like a measurement.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GraphResult {
    /// The result's column names, in order.
    pub columns: Vec<String>,
    /// The column type names the engine reported, as `grafeo`'s own `LogicalType` renders them.
    pub column_types: Vec<String>,
    /// The rows, in the order the engine returned them.
    pub rows: Vec<GraphRow>,
    /// The engine's row counter, or `None` when it reported none. See the table above.
    pub rows_read: Option<u64>,
    /// Bytes read from storage. Always `None` on Grafeo 0.5.43; see the table above.
    pub bytes_read: Option<u64>,
    /// The engine's own elapsed time, in nanoseconds, or `None` when it reported none.
    pub elapsed_nanos: Option<u64>,
    /// Rows the statement changed. Always `None` on Grafeo 0.5.43; see the table above.
    pub rows_affected: Option<u64>,
}

/// One graph, open in this process.
pub struct Graph {
    namespace: String,
    name: String,
    db: GrafeoDB,
    /// The spec this graph was opened with: `open` compares it to decide whether a reopen agrees,
    /// and a poisoned graph is reopened from it.
    spec: OpenSpec,
    read_only: bool,
    persistent: bool,
    /// Set when an engine call panicked (R0.13): the graph's in-memory state is whatever the
    /// panicking call left, so it is not served again until it is reopened.
    poisoned: AtomicBool,
    /// Set when a poisoned graph cannot be reopened from storage (I2); terminal.
    failed: AtomicBool,
    /// A counter rather than a plain field because `execute` takes `&self`: several statements can
    /// be in flight against one graph, and a lost update here would be a wrong number on the wire.
    statements_executed: AtomicU64,
}

impl fmt::Debug for Graph {
    /// Prints what identifies a graph, not what is inside it.
    ///
    /// `GrafeoDB` is a foreign type with no `Debug`, and a `Debug` that reached into it would be
    /// noise: it would dump the store's internals for every `unwrap_err()` in a test. The engine
    /// is opaque by design (D634 (b) embeds it rather than exposing it), so the identity and the
    /// flags are the whole story.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Graph")
            .field("namespace", &self.namespace)
            .field("name", &self.name)
            .field("read_only", &self.read_only)
            .field("persistent", &self.persistent)
            .field("storage", &self.spec.dir)
            .field("poisoned", &self.is_poisoned())
            .field("statements_executed", &self.statements_executed())
            .finish_non_exhaustive()
    }
}

impl Graph {
    /// Opens a graph, or returns the already-open one when the request agrees with it.
    ///
    /// Keyed by `(namespace, name)` and refusing to reopen with a different configuration: two
    /// callers disagreeing about one graph's storage is a bug worth surfacing, not a race to win.
    pub fn open(
        engine: &Engine,
        namespace: &str,
        name: &str,
        spec: OpenSpec,
    ) -> Result<Arc<Graph>, GraphError> {
        let wanted = spec.clone();
        let graph = Self::open_latched(engine, namespace, name, || spec)?;
        if graph.spec != wanted {
            return Err(GraphError::Conflict {
                namespace: namespace.to_string(),
                name: name.to_string(),
            });
        }
        Ok(graph)
    }

    /// The graph open under `(namespace, name)`, or one opened with `spec()` (review M8).
    ///
    /// The disk open (`GrafeoDB::open`, a WAL replay) runs **outside** the registry lock, under a
    /// per-graph latch: concurrent openers of one graph wait for the first and share its graph,
    /// and a slow open never blocks another graph's lookups.
    fn open_latched(
        engine: &Engine,
        namespace: &str,
        name: &str,
        spec: impl FnOnce() -> OpenSpec,
    ) -> Result<Arc<Graph>, GraphError> {
        validate_names(namespace, name)?;
        let key = (namespace.to_string(), name.to_string());
        if let Some(existing) = engine.registered(&key)? {
            return Ok(existing);
        }
        let latch = {
            let mut latches = engine
                .opening
                .lock()
                .map_err(|_| poisoned("the graph opening latches are poisoned"))?;
            Arc::clone(latches.entry(key.clone()).or_default())
        };
        // Declared before the latch's guard, so it drops after it: the latch leaves the map on
        // every path (success, a failed open, a panic), and only when no other opener holds it.
        let _release = LatchRelease {
            engine,
            key: &key,
            latch: &latch,
        };
        #[cfg(feature = "test-hooks")]
        engine.open_hook.call(OpenPoint::LatchTaken, name);
        // The latch guards nothing but the open itself, so a panic in another opener leaves
        // nothing half-done behind it.
        let _opening = latch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Someone else may have opened it while this caller waited for the latch.
        if let Some(existing) = engine.registered(&key)? {
            return Ok(existing);
        }
        #[cfg(feature = "test-hooks")]
        engine.open_hook.call(OpenPoint::BeforeOpen, name);
        let fresh = Arc::new(Graph::open_db(
            namespace,
            name,
            spec(),
            engine.query_timeout,
        )?);
        let graph = {
            let mut graphs = engine
                .graphs
                .lock()
                .map_err(|_| poisoned("the graph registry is poisoned"))?;
            match graphs.entry(key.clone()) {
                std::collections::hash_map::Entry::Vacant(slot) => Arc::clone(slot.insert(fresh)),
                // The latch makes this unreachable: only an opener holding it registers a graph
                // under this key. Were it reached, the registered graph stays (overwriting it
                // would leave two engines on one store) and the fresh one is closed.
                std::collections::hash_map::Entry::Occupied(existing) => {
                    let existing = Arc::clone(existing.get());
                    drop(graphs);
                    tracing::error!(%namespace, %name, "a graph was registered while it was being opened; keeping the registered one");
                    if let Err(err) = fresh.db.close() {
                        tracing::warn!(%namespace, %name, error = %err, "closing the duplicate graph failed");
                    }
                    return Ok(existing);
                }
            }
        };
        tracing::debug!(
            namespace = %graph.namespace,
            name = %graph.name,
            persistent = graph.persistent,
            read_only = graph.read_only,
            "opened a graph"
        );
        Ok(graph)
    }

    /// Opens the engine for a spec. An embedded engine, so this is a directory and never a URL:
    /// there is no connection to make and nothing to authenticate (D634 (b)).
    ///
    /// `query_timeout` is the engine's own statement time limit: a backstop only, because Grafeo
    /// checks it between pipeline chunks, so it stops a statement that streams rows but not one
    /// stuck in a single operator (a cartesian aggregate, an expand), which runs on (R0.8).
    fn open_db(
        namespace: &str,
        name: &str,
        spec: OpenSpec,
        query_timeout: Option<Duration>,
    ) -> Result<Graph, GraphError> {
        let config = match spec.database_file() {
            None => grafeo::Config::in_memory(),
            // `read_only` is the engine's own read-only mode: a shared lock and no WAL
            // replay, so a read-only graph cannot be made to write even by a statement the gate
            // mis-classifies. Grafeo requires an existing `.grafeo` file for it.
            Some(file) if spec.read_only => grafeo::Config::read_only(file),
            Some(file) => {
                if let Some(dir) = &spec.dir {
                    std::fs::create_dir_all(dir).map_err(|err| {
                        GraphError::Engine(format!("creating the graph's storage: {err}"))
                    })?;
                }
                grafeo::Config::persistent(file)
            }
        };
        let config = match query_timeout {
            Some(limit) => config.with_query_timeout(limit),
            None => config.without_query_timeout(),
        };
        let db = GrafeoDB::with_config(config).map_err(as_engine_error)?;
        Ok(Graph {
            namespace: namespace.to_string(),
            name: name.to_string(),
            db,
            read_only: spec.read_only,
            persistent: spec.dir.is_some(),
            spec,
            poisoned: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            statements_executed: AtomicU64::new(0),
        })
    }

    /// The catalog id this graph was opened for, if any.
    pub fn id(&self) -> Option<crate::GraphId> {
        self.spec.id
    }

    /// Node and edge counts.
    pub fn counts(&self) -> (u64, u64) {
        (self.db.node_count() as u64, self.db.edge_count() as u64)
    }

    /// Labels, edge types (each with its count), property keys and indexes, sorted (GetSchema).
    ///
    /// # Errors
    ///
    /// What [`Graph::call`] answers for a poisoned or panicking engine.
    pub fn schema(&self) -> Result<SchemaSummary, GraphError> {
        self.call(|| {
            let mut summary = SchemaSummary::default();
            if let grafeo_engine::admin::SchemaInfo::Lpg(info) = self.db.schema() {
                summary.labels = info
                    .labels
                    .into_iter()
                    .map(|l| (l.name, l.count as u64))
                    .collect();
                summary.edge_types = info
                    .edge_types
                    .into_iter()
                    .map(|t| (t.name, t.count as u64))
                    .collect();
                summary.property_keys = info.property_keys;
            }
            summary.indexes = self
                .db
                .list_indexes()
                .into_iter()
                .map(|i| (i.name, i.target, i.index_type))
                .collect();
            summary.labels.sort();
            summary.edge_types.sort();
            summary.property_keys.sort();
            summary.indexes.sort();
            Ok(summary)
        })
    }

    /// The graph's storage directory, `<data_dir>/graphs/<graph_id>/`, or `None` in memory.
    pub fn storage_dir(&self) -> Option<&Path> {
        self.spec.dir.as_deref()
    }

    /// Whether the graph is serving or waiting to be reopened.
    pub fn state(&self) -> GraphState {
        if self.failed.load(Ordering::Acquire) {
            GraphState::Failed
        } else if self.is_poisoned() {
            GraphState::Poisoned
        } else {
            GraphState::Ready
        }
    }

    /// Whether the graph needs [`Engine::reopen_if_poisoned`] (or is failed) before it serves.
    pub fn is_poisoned_or_failed(&self) -> bool {
        self.is_poisoned() || self.failed.load(Ordering::Acquire)
    }

    fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::Acquire)
    }

    /// The engine's current epoch.
    pub fn current_epoch(&self) -> grafeo_common::types::EpochId {
        self.db.current_epoch()
    }

    /// A Grafeo session with the role an access needs (R0.10): `ReadOnly` for a read,
    /// `ReadWrite` for a write, `Admin` for schema DDL. The engine refuses anything the role does
    /// not allow, whatever the gate concluded.
    pub fn session_for(&self, access: Access) -> grafeo::Session {
        let role = match access {
            Access::Read => grafeo_engine::auth::Role::ReadOnly,
            Access::Write => grafeo_engine::auth::Role::ReadWrite,
            Access::Admin => grafeo_engine::auth::Role::Admin,
        };
        self.db.session_with_role(role)
    }

    /// Runs one engine call with panic containment (R0.13): a panic poisons this graph and
    /// answers [`GraphError::EnginePanic`]; other graphs are untouched. A poisoned graph answers
    /// [`GraphError::Reloading`] until it is reopened ([`Engine::reopen_if_poisoned`]).
    fn call<T: Send>(
        &self,
        f: impl FnOnce() -> Result<T, GraphError> + Send,
    ) -> Result<T, GraphError> {
        if self.failed.load(Ordering::Acquire) {
            return Err(GraphError::Failed);
        }
        if self.is_poisoned() {
            return Err(GraphError::Reloading);
        }
        // On a large stack (the engine re-parses the statement; re-review 2c), with the panic
        // contained: a panic on that thread comes back as `Err` from its join.
        let outcome = crate::classify::on_big_stack(|| {
            #[cfg(feature = "failpoints")]
            fail::fail_point!("loams_graph::engine_call");
            f()
        });
        match outcome {
            Ok(result) => result,
            Err(_) => {
                self.poisoned.store(true, Ordering::Release);
                tracing::error!(
                    namespace = %self.namespace,
                    name = %self.name,
                    "the graph engine panicked; the graph is poisoned and reopens on its next call"
                );
                Err(GraphError::EnginePanic)
            }
        }
    }

    /// The access a statement needs, refusing it on a read-only request or graph, and refusing
    /// schema DDL outright.
    ///
    /// Schema DDL (`Access::Admin`: node, edge and graph types, indexes, constraints, schemas,
    /// procedures) is not served until Task 24 (fix round 2). Grafeo's `Session::execute`, which
    /// a statement without parameters takes for the engine's time limit, runs it directly, with
    /// no Loams authorisation and outside CDC; the parameterised path is refused the same way so
    /// the answer does not depend on the bindings.
    fn admit(&self, statement: &str, read_only: bool) -> Result<Access, GraphError> {
        #[cfg(feature = "failpoints")]
        fail::fail_point!("loams_graph::gate");
        let access = gate(statement, QueryLanguage::Gql)?;
        if access == Access::Admin {
            return Err(GraphError::StatementNotAllowed {
                file_access: false,
                what: "schema DDL is not served until Task 24".to_string(),
            });
        }
        if (read_only || self.read_only) && access != Access::Read {
            return Err(GraphError::ReadOnly);
        }
        Ok(access)
    }

    /// Answers the graph open under `(namespace, name)`, or opens it with `spec` when there is
    /// none, as one step under the registry lock, so concurrent callers all get the same graph
    /// (security review M6). Unlike [`Graph::open`], an existing graph is answered whatever its
    /// spec: this is `CreateGraph`'s idempotency by name.
    pub fn open_or_existing(
        engine: &Engine,
        namespace: &str,
        name: &str,
        spec: impl FnOnce() -> OpenSpec,
    ) -> Result<Arc<Graph>, GraphError> {
        Self::open_latched(engine, namespace, name, spec)
    }

    /// This graph's namespace.
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// This graph's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether the graph refuses writing statements.
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Whether the graph's storage outlives this process.
    pub fn is_persistent(&self) -> bool {
        self.persistent
    }

    /// Statements executed against this graph so far.
    pub fn statements_executed(&self) -> u64 {
        self.statements_executed.load(Ordering::Relaxed)
    }

    /// Runs one statement and returns its rows.
    ///
    /// The statement reaches the engine **unchanged**. `read_only` is checked here rather than
    /// trusted to the engine: a refusal with Loams's own message is a better answer than a parse
    /// error, and the check is a few words long. A persistent graph opened read-only is *also*
    /// read-only inside the engine, which is the belt to this braces.
    pub fn execute(&self, statement: &str, read_only: bool) -> Result<GraphResult, GraphError> {
        self.execute_with_params(statement, HashMap::new(), read_only)
    }

    /// Runs one statement with `$name` parameters bound by the engine, as its own transaction.
    ///
    /// The statement is gated ([`gate`]) and then run, unchanged, on a session whose role matches
    /// what it needs; a value is never interpolated into it.
    pub fn execute_with_params(
        &self,
        statement: &str,
        parameters: HashMap<String, Value>,
        read_only: bool,
    ) -> Result<GraphResult, GraphError> {
        // The gate and the row building run inside the same panic containment as the engine
        // call (security review M5).
        self.call(|| {
            let access = self.admit(statement, read_only)?;
            self.run_engine(access, statement, parameters)
        })
    }

    /// Runs one admitted statement on the session for `access`: the one path every single
    /// statement takes into the engine. Always called inside [`Graph::call`].
    fn run_engine(
        &self,
        access: Access,
        statement: &str,
        parameters: HashMap<String, Value>,
    ) -> Result<GraphResult, GraphError> {
        let result = run_on_session(&self.session_for(access), statement, parameters)
            .map_err(as_engine_error)?;
        self.statements_executed.fetch_add(1, Ordering::Relaxed);
        Ok(self.resolved(GraphResult::from(result)))
    }

    /// **Tests only** (feature `test-hooks`): runs a statement on Loams's own execution path with
    /// its access forced, skipping the gate, so a test can show the engine role alone refuses what
    /// the gate would have (security review I4). Never compiled into a server build.
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn execute_forced(
        &self,
        access: Access,
        statement: &str,
        parameters: HashMap<String, Value>,
    ) -> Result<GraphResult, GraphError> {
        self.call(|| self.run_engine(access, statement, parameters))
    }

    /// Runs statements as one engine transaction, so a write batch lands whole or not at all.
    ///
    /// One [`grafeo::Session`] with an explicit transaction, rather than a loop over
    /// `GrafeoDB::execute`: that convenience makes a fresh one-shot session per statement and
    /// therefore a fresh transaction per statement, which is the opposite of what the contract's
    /// `ExecuteBatch` promises. Grafeo has full ACID with MVCC snapshot isolation, so this is the
    /// engine's real transaction rather than a Loams-side emulation, and a statement that fails
    /// rolls the batch back rather than leaving its predecessors committed.
    ///
    /// A statement the engine rejects fails the whole batch. A partial batch would be the one place
    /// in the graph surface where a caller's "all or nothing" was a claim rather than a fact.
    pub fn execute_batch(
        &self,
        statements: &[BatchStatement],
    ) -> Result<Vec<GraphResult>, GraphError> {
        // Gate, engine and row building all inside the panic containment (M5).
        self.call(|| {
            // Every statement is gated before any runs, so a refused one leaves nothing
            // half-applied.
            let mut access = Access::Read;
            for statement in statements {
                access = access.max(self.admit(&statement.text, false)?);
            }
            let mut session = self.session_for(access);
            session.begin_transaction().map_err(as_engine_error)?;
            let mut out = Vec::with_capacity(statements.len());
            for statement in statements {
                // The engine takes the bindings by value, hence the copy.
                let bound = statement.parameters.clone();
                match run_on_session(&session, &statement.text, bound) {
                    Ok(result) => {
                        self.statements_executed.fetch_add(1, Ordering::Relaxed);
                        out.push(GraphResult::from(result));
                    }
                    Err(err) => {
                        // Nothing before the failure becomes durable, so a retry of the whole
                        // batch is safe; a partial batch would not be.
                        let _ = self.rollback(&mut session);
                        return Err(as_engine_error(err));
                    }
                }
            }
            if let Err(err) = session.commit() {
                let _ = self.rollback(&mut session);
                return Err(as_engine_error(err));
            }
            Ok(out
                .into_iter()
                .map(|result| self.resolved(result))
                .collect())
        })
    }

    /// Rolls a failed batch back, saying so if the rollback itself fails.
    ///
    /// A rollback that fails leaves the session to be dropped, which the engine also treats as a
    /// rollback; the log line is here because a batch that failed *and* could not be rolled back
    /// explicitly is worth a reviewer's attention.
    fn rollback(&self, session: &mut grafeo::Session) -> Result<(), GrafeoError> {
        session.rollback().inspect_err(|err| {
            tracing::warn!(
                namespace = %self.namespace,
                name = %self.name,
                error = %err,
                "rolling a failed batch back failed; the session drop still rolls it back"
            );
        })
    }

    /// Resolves every path in a result to full elements (GR1 Task 2 review).
    ///
    /// Grafeo 0.5.43 builds `Value::Path` from element **ids**; the contract promises full nodes
    /// and relationships. Each id is looked up here and replaced by the same map the engine's own
    /// projection builds for a node (`_id`, `_labels`, properties) or a relationship (`_id`,
    /// `_type`, `_source`, `_target`, properties), which [`crate::value::to_proto`] then answers
    /// as a `Node` or `Relationship`. The lookup reads the current state: until Task 11 pins a
    /// read's epoch, a write committed between the statement and this lookup is visible here, and
    /// an element deleted in that window stays an id.
    fn resolved(&self, mut result: GraphResult) -> GraphResult {
        #[cfg(feature = "failpoints")]
        fail::fail_point!("loams_graph::resolve");
        for row in &mut result.rows {
            for value in &mut row.values {
                self.resolve(value);
            }
        }
        result
    }

    fn resolve(&self, value: &mut Value) {
        match value {
            Value::Path { nodes, edges } => {
                let nodes: Vec<Value> = nodes.iter().map(|n| self.node_value(n)).collect();
                let edges: Vec<Value> = edges.iter().map(|e| self.edge_value(e)).collect();
                *value = Value::Path {
                    nodes: Arc::from(nodes),
                    edges: Arc::from(edges),
                };
            }
            Value::List(items) => {
                let mut items = items.to_vec();
                items.iter_mut().for_each(|item| self.resolve(item));
                *value = Value::List(Arc::from(items));
            }
            Value::Map(map) => {
                let mut map = (**map).clone();
                map.values_mut().for_each(|item| self.resolve(item));
                *value = Value::Map(Arc::new(map));
            }
            _ => {}
        }
    }

    fn node_value(&self, element: &Value) -> Value {
        let Value::Int64(id) = element else {
            return element.clone();
        };
        let Some(node) = self
            .db
            .get_node(grafeo_common::types::NodeId::new(*id as u64))
        else {
            return element.clone();
        };
        // Properties first, then the real fields, so a property named like a reserved key never
        // overrides them (GR1 Task 2 N3; the property is shadowed in the element, R2.4).
        let mut map = std::collections::BTreeMap::new();
        for (key, value) in node.properties.iter() {
            map.insert(key.clone(), value.clone());
        }
        map.insert(PropertyKey::new("_id"), Value::Int64(*id));
        map.insert(
            PropertyKey::new("_labels"),
            Value::List(Arc::from(
                node.labels
                    .iter()
                    .map(|label| Value::String(label.clone()))
                    .collect::<Vec<_>>(),
            )),
        );
        Value::Map(Arc::new(map))
    }

    fn edge_value(&self, element: &Value) -> Value {
        let Value::Int64(id) = element else {
            return element.clone();
        };
        let Some(edge) = self
            .db
            .get_edge(grafeo_common::types::EdgeId::new(*id as u64))
        else {
            return element.clone();
        };
        // As in `node_value`: properties first, then the real fields.
        let mut map = std::collections::BTreeMap::new();
        for (key, value) in edge.properties.iter() {
            map.insert(key.clone(), value.clone());
        }
        map.insert(PropertyKey::new("_id"), Value::Int64(*id));
        map.insert(
            PropertyKey::new("_type"),
            Value::String(edge.edge_type.clone()),
        );
        map.insert(PropertyKey::new("_source"), Value::Int64(edge.src.0 as i64));
        map.insert(PropertyKey::new("_target"), Value::Int64(edge.dst.0 as i64));
        Value::Map(Arc::new(map))
    }
}

/// The engine's own statement time limit (Grafeo's `query_timeout`), set on every graph as a
/// backstop (R0.8 (a); §48 §13.1's 30 s default). Grafeo checks it between pipeline chunks only,
/// so it ends a statement that streams rows but not one inside a single long operator; Task 6's
/// Loams-side deadline answers the client either way.
pub const DEFAULT_QUERY_TIMEOUT: Duration = Duration::from_secs(30);

/// A per-graph opening latch.
type Latch = Arc<Mutex<()>>;

/// Takes a graph's opening latch out of the map when its opener is done (review fix 1, I2), on
/// the success and the error path alike, and only when no other opener holds it: a waiter
/// still holding a latch that left the map would open beside a newcomer with a fresh latch.
struct LatchRelease<'a> {
    engine: &'a Engine,
    key: &'a (String, String),
    latch: &'a Latch,
}

impl Drop for LatchRelease<'_> {
    fn drop(&mut self) {
        let mut latches = self
            .engine
            .opening
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Under the map's lock nobody can take a new clone, so two holders (the map and this
        // opener) means nobody else is waiting.
        if latches
            .get(self.key)
            .is_some_and(|held| Arc::ptr_eq(held, self.latch))
            && Arc::strong_count(self.latch) == 2
        {
            latches.remove(self.key);
        }
    }
}

/// **Tests only** (feature `test-hooks`): the points of an open a test can pause at.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenPoint {
    /// The opener holds a clone of the graph's latch and has not locked it yet.
    LatchTaken,
    /// The opener holds the latch, found no graph registered, and is about to open storage.
    BeforeOpen,
}

/// **Tests only** (feature `test-hooks`): called at each [`OpenPoint`] with the graph's name.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub type OpenHook = Arc<dyn Fn(OpenPoint, &str) + Send + Sync>;

#[cfg(feature = "test-hooks")]
#[derive(Default)]
struct OpenHookSlot(Mutex<Option<OpenHook>>);

#[cfg(feature = "test-hooks")]
impl fmt::Debug for OpenHookSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OpenHookSlot")
    }
}

#[cfg(feature = "test-hooks")]
impl OpenHookSlot {
    fn call(&self, point: OpenPoint, name: &str) {
        let hook = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(hook) = hook {
            hook(point, name);
        }
    }
}

/// The engine: the open graphs of one Loams process.
///
/// Deliberately not a global. `Engine` is held by the service and injected, so a test can stand up
/// two engines and be sure they share nothing.
#[derive(Debug)]
pub struct Engine {
    /// Keyed by `(namespace, name)`, never by a joined string, so no two pairs share a key
    /// (security review I3).
    graphs: Mutex<HashMap<(String, String), Arc<Graph>>>,
    /// Per-graph latches held while a graph opens outside the registry lock (review M8).
    opening: Mutex<HashMap<(String, String), Latch>>,
    /// The engine's own statement time limit for every graph it opens (R0.8 (a)), or `None`.
    query_timeout: Option<Duration>,
    #[cfg(feature = "test-hooks")]
    open_hook: OpenHookSlot,
    /// Where persistent graphs live: `<data_dir>/graphs/<graph_id>/`. `None` keeps every graph in
    /// memory.
    data_dir: Option<PathBuf>,
    /// The GQL standard this engine's surface targets, reported over the wire.
    pub standard: &'static str,
    /// Grafeo's own version.
    pub engine_version: &'static str,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    /// A fresh engine with no graphs open.
    #[must_use]
    pub fn new() -> Self {
        Self {
            graphs: Mutex::new(HashMap::new()),
            opening: Mutex::new(HashMap::new()),
            query_timeout: Some(DEFAULT_QUERY_TIMEOUT),
            #[cfg(feature = "test-hooks")]
            open_hook: OpenHookSlot::default(),
            data_dir: None,
            standard: crate::GQL_STANDARD,
            engine_version: crate::ENGINE_VERSION,
        }
    }

    /// The graph registered under `key`, if any.
    fn registered(&self, key: &(String, String)) -> Result<Option<Arc<Graph>>, GraphError> {
        Ok(self
            .graphs
            .lock()
            .map_err(|_| poisoned("the graph registry is poisoned"))?
            .get(key)
            .cloned())
    }

    /// **Tests only** (feature `test-hooks`): sets the hook every open calls at its
    /// [`OpenPoint`]s.
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn set_open_hook(&self, hook: Option<OpenHook>) {
        *self
            .open_hook
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = hook;
    }

    /// **Tests only** (feature `test-hooks`): how many opening latches are in the map.
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn opening_latches(&self) -> usize {
        self.opening
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// An engine whose persistent graphs live under `data_dir/graphs/`.
    #[must_use]
    pub fn with_data_dir(data_dir: impl AsRef<Path>) -> Self {
        Self {
            data_dir: Some(data_dir.as_ref().to_path_buf()),
            ..Self::new()
        }
    }

    /// The same engine with another statement time limit for the graphs it opens from now on
    /// (`None`: none). A backstop, not a deadline: see [`DEFAULT_QUERY_TIMEOUT`].
    #[must_use]
    pub fn with_query_timeout(mut self, limit: Option<Duration>) -> Self {
        self.query_timeout = limit;
        self
    }

    /// The statement time limit graphs are opened with.
    #[must_use]
    pub fn query_timeout(&self) -> Option<Duration> {
        self.query_timeout
    }

    /// The data directory, if persistent graphs have one.
    pub fn data_dir(&self) -> Option<&Path> {
        self.data_dir.as_deref()
    }

    /// Reopens a graph an engine panic poisoned (R0.13), from its own spec: a persistent graph
    /// comes back with everything it committed, an in-memory one comes back empty. A graph that
    /// is not poisoned is answered as it is.
    ///
    /// # Errors
    ///
    /// [`GraphError::Reloading`] while another caller still holds the poisoned handle (its file
    /// lock is not released yet), or the engine's error opening the storage.
    pub fn reopen_if_poisoned(&self, graph: Arc<Graph>) -> Result<Arc<Graph>, GraphError> {
        if !graph.is_poisoned() {
            return Ok(graph);
        }
        if graph.failed.load(Ordering::Acquire) {
            return Err(GraphError::Failed);
        }
        // An in-memory graph has nothing to reopen from: reopening it would serve an empty graph
        // in its place and lose every acknowledged write without a word (I2). It fails instead.
        if !graph.persistent {
            graph.failed.store(true, Ordering::Release);
            tracing::error!(
                namespace = %graph.namespace,
                name = %graph.name,
                "an in-memory graph was lost to an engine panic; it is failed, not reopened"
            );
            return Err(GraphError::Failed);
        }
        let key = (graph.namespace.clone(), graph.name.clone());
        let mut graphs = self
            .graphs
            .lock()
            .map_err(|_| poisoned("the graph registry is poisoned"))?;
        // Someone may have reopened it already.
        if let Some(current) = graphs.get(&key)
            && !current.is_poisoned()
        {
            return Ok(Arc::clone(current));
        }
        let Some(old) = graphs.remove(&key) else {
            return Err(GraphError::Reloading);
        };
        drop(graph);
        if Arc::strong_count(&old) > 1 {
            graphs.insert(key, old);
            return Err(GraphError::Reloading);
        }
        let spec = old.spec.clone();
        let (namespace, name) = (old.namespace.clone(), old.name.clone());
        // The close flushes the WAL. If it fails, buffered commits may not be on disk, and a reopen
        // could serve a graph missing acknowledged writes, so the graph fails instead (I2).
        if let Err(err) = old.db.close() {
            tracing::error!(%namespace, %name, error = %err, "closing a poisoned graph failed; it is failed, not reopened");
            old.failed.store(true, Ordering::Release);
            graphs.insert(key, old);
            return Err(GraphError::Failed);
        }
        drop(old);
        let fresh = Arc::new(Graph::open_db(&namespace, &name, spec, self.query_timeout)?);
        graphs.insert(key, Arc::clone(&fresh));
        tracing::info!(%namespace, %name, "reopened a poisoned graph");
        Ok(fresh)
    }

    /// The open graphs, for `ListGraphs`.
    ///
    /// Clones the handles rather than lending out the map: a caller must not be able to hold the
    /// registry lock across an engine call.
    pub fn list(&self, namespace: Option<&str>) -> Result<Vec<Arc<Graph>>, GraphError> {
        let graphs = self
            .graphs
            .lock()
            .map_err(|_| poisoned("the graph registry is poisoned"))?;
        Ok(graphs
            .values()
            .filter(|g| namespace.is_none_or(|ns| g.namespace == ns))
            .map(Arc::clone)
            .collect())
    }

    /// Closes a graph and releases its storage.
    ///
    /// Refuses while a caller still holds the handle: dropping the engine's reference is not enough
    /// to say nobody is mid-statement, and a close that raced a query would surface as a failure
    /// inside the engine rather than as an error in Loams.
    ///
    /// A closed graph is closed **through the engine** rather than by dropping it, so a persistent
    /// graph checkpoints and releases its file lock. Dropping the last `Arc` would drop the lock
    /// too, but only after the engine's own shutdown work had been skipped.
    pub fn close(&self, namespace: &str, name: &str) -> Result<bool, GraphError> {
        validate_names(namespace, name)?;
        let key = (namespace.to_string(), name.to_string());
        let mut graphs = self
            .graphs
            .lock()
            .map_err(|_| poisoned("the graph registry is poisoned"))?;
        let Some(graph) = graphs.get(&key) else {
            return Ok(false);
        };
        if Arc::strong_count(graph) > 1 {
            return Err(GraphError::Engine(format!(
                "graph {namespace}/{name} is still in use by another holder"
            )));
        }
        // `GrafeoDB::close` takes `&self`, so this runs with the graph still registered; the
        // registry entry is dropped immediately afterwards.
        // A poisoned or failed engine may not close cleanly; it is dropped either way, which
        // releases its file lock, because the caller asked for it gone.
        if let Err(err) = graph.db.close() {
            if graph.is_poisoned() {
                tracing::warn!(%namespace, %name, error = %err, "closing a poisoned graph failed; dropping it");
            } else {
                return Err(as_engine_error(err));
            }
        }
        Ok(graphs.remove(&key).is_some())
    }
}

/// Validates a namespace and a graph name (security review I3; Task 4's catalog keeps the same
/// rules):
///
/// * a graph name is `[a-z][a-z0-9_-]{0,62}`;
/// * a namespace is `[A-Za-z0-9_-]{1,63}`.
///
/// Neither may hold `/`, so no pair can be mistaken for another, and neither may be empty.
///
/// # Errors
///
/// [`GraphError::InvalidValue`] naming the rule that failed.
pub fn validate_names(namespace: &str, name: &str) -> Result<(), GraphError> {
    let namespace_ok = (1..=63).contains(&namespace.len())
        && namespace
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if !namespace_ok {
        return Err(GraphError::InvalidValue(format!(
            "namespace {namespace:?} must be 1 to 63 characters of A-Z, a-z, 0-9, _ and -"
        )));
    }
    let mut bytes = name.bytes();
    let name_ok = name.len() <= 63
        && bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
    if !name_ok {
        return Err(GraphError::InvalidValue(format!(
            "graph name {name:?} must match [a-z][a-z0-9_-]{{0,62}}"
        )));
    }
    Ok(())
}

/// A poisoned registry lock. Named so the message does not carry a `PoisonError`'s debug form.
fn poisoned(what: &str) -> GraphError {
    GraphError::Engine(what.to_string())
}

/// Runs one GQL statement on a session, the way Grafeo's own `execute_language` routes it:
/// `execute` when there are no parameters, `execute_with_params` otherwise.
///
/// The split matters for the time limit (review fix 1, I1): `execute_with_params` runs through
/// the query processor with no deadline, so Grafeo's `query_timeout` never applies to it; only
/// `execute` honours it. A statement with parameters therefore has no engine backstop until
/// Grafeo threads the deadline through (upstream ask, R0.8; Task 6's Loams-side deadline still
/// answers the client). Both paths enforce the session's role.
fn run_on_session(
    session: &grafeo::Session,
    statement: &str,
    parameters: HashMap<String, Value>,
) -> Result<QueryResult, GrafeoError> {
    if parameters.is_empty() {
        session.execute(statement)
    } else {
        session.execute_with_params(statement, parameters)
    }
}

/// Wraps the engine's error, keeping its text whole.
///
/// One case is translated: the engine reports a statement's unbound `$parameter` as an *internal*
/// error, which is honest from where it stands — it parsed the statement and the text is fine, the
/// binding is missing — but the contract says this is the caller's problem
/// (`InvalidArgument`). The engine's message is what carries the parameter's name, so this matches
/// on it. If Grafeo ever rewords the message the case degrades to `Internal`, which loses the
/// classification but never the diagnostic.
fn as_engine_error(err: GrafeoError) -> GraphError {
    if err.error_code() == grafeo_common::utils::error::ErrorCode::QueryTimeout {
        return GraphError::StatementTimeout;
    }
    const MISSING: &str = "Missing parameter: $";
    let text = err.to_string();
    match text.split_once(MISSING) {
        Some((_, name)) if text.starts_with("GRAFEO-X001: Internal error: ") => {
            GraphError::UnboundParameter {
                name: name.trim().to_string(),
            }
        }
        _ => GraphError::Engine(text),
    }
}

/// Converts one engine result into Loams's shape.
impl From<QueryResult> for GraphResult {
    fn from(result: QueryResult) -> Self {
        // Read the engine's own counters before borrowing the rows: `rows_scanned` is set to the
        // number of rows *returned* (`session/mod.rs`), so it is a real measurement but not of rows
        // read from storage, and it is named here rather than at the call site.
        let rows_read = result.rows_scanned;
        let elapsed_nanos = result.execution_time_ms.map(|ms| (ms * 1_000_000.0) as u64);

        let rows = result
            .rows()
            .iter()
            .map(|row| GraphRow {
                values: row.to_vec(),
            })
            .collect();

        Self {
            // `LogicalType`'s own rendering (`INT64`, `STRING`, `NODE`), which is a type *name*
            // rather than a GQL type tag: the surface is GQL, not SQL.
            column_types: result
                .column_types
                .iter()
                .map(|ty| ty.to_string())
                .collect(),
            columns: result.columns.clone(),
            rows,
            rows_read,
            bytes_read: None,
            elapsed_nanos,
            rows_affected: None,
        }
    }
}
