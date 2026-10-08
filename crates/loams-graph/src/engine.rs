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
    /// The graph is poisoned by an earlier panic and has not been reopened yet.
    #[error("the graph is reloading after an engine failure; retry")]
    Reloading,
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
            | Self::InvalidValue(_) => "invalid_argument",
            Self::Conflict { .. } => "already_exists",
            Self::ReadOnly => "graph_read_only",
            Self::LanguageUnavailable(_) => "graph_language_disabled",
            Self::TransactionStatement => "graph_transaction_statement",
            Self::StatementNotAllowed { .. } => "graph_statement_not_allowed",
            Self::UnboundedPath { .. } => "graph_unbounded_path",
            Self::EnginePanic => "graph_engine_panic",
            Self::Reloading => "graph_reloading",
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
            dir: Some(data_dir.join("graphs").join(id.to_string())),
            read_only: false,
        })
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

/// Whether a graph is serving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphState {
    /// Serving.
    Ready,
    /// An engine call panicked; the graph reopens on its next call.
    Poisoned,
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
        let key = format!("{namespace}/{name}");
        let mut graphs = engine
            .graphs
            .lock()
            .map_err(|_| poisoned("the graph registry is poisoned"))?;

        if let Some(existing) = graphs.get(&key) {
            if existing.spec != spec {
                return Err(GraphError::Conflict {
                    namespace: namespace.to_string(),
                    name: name.to_string(),
                });
            }
            return Ok(Arc::clone(existing));
        }
        let graph = Arc::new(Graph::open_db(namespace, name, spec)?);
        graphs.insert(key, Arc::clone(&graph));
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
    fn open_db(namespace: &str, name: &str, spec: OpenSpec) -> Result<Graph, GraphError> {
        let db = match spec.database_file() {
            None => GrafeoDB::new_in_memory(),
            // `open_read_only` is the engine's own read-only mode: a shared lock and no WAL
            // replay, so a read-only graph cannot be made to write even by a statement the gate
            // mis-classifies. Grafeo requires an existing `.grafeo` file for it.
            Some(file) if spec.read_only => {
                GrafeoDB::open_read_only(&file).map_err(as_engine_error)?
            }
            Some(file) => {
                if let Some(dir) = &spec.dir {
                    std::fs::create_dir_all(dir).map_err(|err| {
                        GraphError::Engine(format!("creating the graph's storage: {err}"))
                    })?;
                }
                GrafeoDB::open(&file).map_err(as_engine_error)?
            }
        };
        Ok(Graph {
            namespace: namespace.to_string(),
            name: name.to_string(),
            db,
            read_only: spec.read_only,
            persistent: spec.dir.is_some(),
            spec,
            poisoned: AtomicBool::new(false),
            statements_executed: AtomicU64::new(0),
        })
    }

    /// The graph's storage directory, `<data_dir>/graphs/<graph_id>/`, or `None` in memory.
    pub fn storage_dir(&self) -> Option<&Path> {
        self.spec.dir.as_deref()
    }

    /// Whether the graph is serving or waiting to be reopened.
    pub fn state(&self) -> GraphState {
        if self.is_poisoned() {
            GraphState::Poisoned
        } else {
            GraphState::Ready
        }
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
    fn call<T>(&self, f: impl FnOnce() -> Result<T, GraphError>) -> Result<T, GraphError> {
        if self.is_poisoned() {
            return Err(GraphError::Reloading);
        }
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            #[cfg(feature = "failpoints")]
            fail::fail_point!("loams_graph::engine_call");
            f()
        }));
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

    /// The access a statement needs, refusing it on a read-only request or graph.
    fn admit(&self, statement: &str, read_only: bool) -> Result<Access, GraphError> {
        let access = gate(statement, QueryLanguage::Gql)?;
        if (read_only || self.read_only) && access != Access::Read {
            return Err(GraphError::ReadOnly);
        }
        Ok(access)
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
        let access = self.admit(statement, read_only)?;
        let result = self.call(|| {
            self.session_for(access)
                .execute_with_params(statement, parameters)
                .map_err(as_engine_error)
        })?;
        self.statements_executed.fetch_add(1, Ordering::Relaxed);
        Ok(self.resolved(GraphResult::from(result)))
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
        // Every statement is gated before any runs, so a refused one leaves nothing half-applied.
        let mut access = Access::Read;
        for statement in statements {
            access = access.max(self.admit(&statement.text, false)?);
        }
        let out = self.call(|| {
            let mut session = self.session_for(access);
            session.begin_transaction().map_err(as_engine_error)?;
            let mut out = Vec::with_capacity(statements.len());
            for statement in statements {
                // `execute_with_params` takes the bindings by value, hence the copy.
                let bound = statement.parameters.clone();
                match session.execute_with_params(&statement.text, bound) {
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
            Ok(out)
        })?;
        Ok(out
            .into_iter()
            .map(|result| self.resolved(result))
            .collect())
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

/// The engine: the open graphs of one Loams process.
///
/// Deliberately not a global. `Engine` is held by the service and injected, so a test can stand up
/// two engines and be sure they share nothing.
#[derive(Debug)]
pub struct Engine {
    graphs: Mutex<HashMap<String, Arc<Graph>>>,
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
            data_dir: None,
            standard: crate::GQL_STANDARD,
            engine_version: crate::ENGINE_VERSION,
        }
    }

    /// An engine whose persistent graphs live under `data_dir/graphs/`.
    #[must_use]
    pub fn with_data_dir(data_dir: impl AsRef<Path>) -> Self {
        Self {
            data_dir: Some(data_dir.as_ref().to_path_buf()),
            ..Self::new()
        }
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
        let key = format!("{}/{}", graph.namespace, graph.name);
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
        // Best effort: the panicking call may have left the engine unable to close cleanly; the
        // drop that follows releases its file lock either way.
        if let Err(err) = old.db.close() {
            tracing::warn!(%namespace, %name, error = %err, "closing a poisoned graph failed");
        }
        drop(old);
        let fresh = Arc::new(Graph::open_db(&namespace, &name, spec)?);
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
        let key = format!("{namespace}/{name}");
        let mut graphs = self
            .graphs
            .lock()
            .map_err(|_| poisoned("the graph registry is poisoned"))?;
        let Some(graph) = graphs.get(&key) else {
            return Ok(false);
        };
        if Arc::strong_count(graph) > 1 {
            return Err(GraphError::Engine(format!(
                "graph {key} is still in use by another holder"
            )));
        }
        // `GrafeoDB::close` takes `&self`, so this runs with the graph still registered; the
        // registry entry is dropped immediately afterwards.
        graph.db.close().map_err(as_engine_error)?;
        Ok(graphs.remove(&key).is_some())
    }
}

/// A poisoned registry lock. Named so the message does not carry a `PoisonError`'s debug form.
fn poisoned(what: &str) -> GraphError {
    GraphError::Engine(what.to_string())
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
