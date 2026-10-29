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

use crate::classify::{Access, Verdict, gate_within};
use crate::limits::StatementLimits;
use grafeo_common::types::PropertyKey;

/// What this crate can be wrong about, all of it recoverable by the caller.
///
/// `PartialEq` so a caller -- and this crate's own tests -- can assert *which* refusal happened
/// without matching on the message text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GraphError {
    /// The engine refused a statement. Grafeo's own error text, kept whole: it carries the syntax
    /// span and a hint, and dropping either makes a GQL mistake much harder to fix than a message
    /// alone would. The span and the GQLSTATUS also travel apart, in `diagnostic`, so an editor
    /// can underline the error without reading the text (§48 §8.3, GR1 Task 7).
    #[error("the graph engine refused the statement: {message}")]
    Engine {
        /// The engine's text.
        message: String,
        /// Where the statement broke and its GQLSTATUS, when the engine said.
        diagnostic: Diagnostic,
    },
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
    /// The statement ran past its time limit: Loams's deadline, or the engine's own
    /// `query_timeout` backstop (R0.8 (a)).
    #[error("the statement ran past its time limit")]
    StatementTimeout,
    /// A statement, its parameters or a batch is over a limit (§48 §13.1, GR1 Task 6).
    #[error("{0}")]
    OverLimit(String),
    /// A unary answer is larger than its byte limit (§48 §13.1, GR1 Task 6).
    #[error(
        "the answer is larger than {limit} bytes; use ExecuteStream, or ask for fewer rows or columns"
    )]
    ResultTooLarge {
        /// The limit, in encoded bytes.
        limit: u64,
    },
    /// One row is larger than a Connect message may be, so no answer can carry it (review fix
    /// 1, M7).
    #[error(
        "a row of the answer is {bytes} bytes; a message carries at most {limit}: return fewer or smaller values per row"
    )]
    RowTooLarge {
        /// The row's encoded size.
        bytes: u64,
        /// The largest row a message carries.
        limit: u64,
    },
    /// An ALL SHORTEST search: its answer can grow exponentially with the graph (R6.4).
    #[error(
        "ALL SHORTEST path searches are not served: their answer can grow exponentially with the graph; use ANY SHORTEST"
    )]
    AllShortestPaths,
    /// An engine panic lost the graph and it cannot be reopened from storage (an in-memory graph,
    /// or a poisoned engine that would not close cleanly). It is never served again empty, so an
    /// acknowledged write is never silently lost (security review I2).
    #[error(
        "the graph was lost to an engine failure and cannot be reopened from storage; delete it and create it again"
    )]
    Failed,
    /// The transaction conflicted with another one and was rolled back; retry it (§48 §7.3).
    #[error("the transaction conflicted with another and was rolled back; retry it")]
    WriteConflict,
    /// `if_version` did not match the graph's version (§48 §7.3).
    #[error("the graph is at version {current}, not {expected}; read it again and retry")]
    VersionMismatch {
        /// The version the request named.
        expected: u64,
        /// The graph's version: the offset of its consistency token.
        current: u64,
    },
    /// The log the graph is durable in could not be read or written; retry (GR1 Task 11).
    #[error("{0}")]
    LogUnavailable(String),
    /// This node does not own the graph, or lost it while serving, and cannot pass the call on
    /// (a forwarded call is never forwarded again); retry, and it reaches the owner (GR1 Task 12).
    #[error("{0}")]
    OwnerMoved(String),
    /// A writing statement on a LINKED graph: only its links write it (GR1 Task 17).
    #[error("this graph is LINKED: its links write it, and GQL may only read it")]
    Linked,
    /// An `at_least` read waited its `consistency_wait` and the replica serving it had not yet
    /// applied the token's offset (GR1 Task 15, §48 §7.2); retry, or read `strong`.
    #[error(
        "the graph's replica has applied its stream up to offset {applied}, not the token's \
         {wanted}, after waiting {waited_ms} ms; retry, or read strong"
    )]
    ConsistencyWaitTimeout {
        /// The token's offset.
        wanted: u64,
        /// The offset the replica had applied when the wait ended.
        applied: u64,
        /// How long the read waited.
        waited_ms: u64,
    },
}

/// What the engine said about a statement it refused, besides its text (§48 §8.3, GR1 Task 7).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diagnostic {
    /// The five-character GQLSTATUS (ISO/IEC 39075 §23), as Grafeo maps its error.
    pub gqlstatus: Option<String>,
    /// Where in the statement, for a syntax or semantic error that has a span.
    pub position: Option<Position>,
}

/// A place in a statement: a 1-based `line`, a 1-based `column` in characters from the start of
/// that line, and a `length` in characters (what an editor underlines).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    /// 1-based line.
    pub line: u32,
    /// 1-based column.
    pub column: u32,
    /// How many characters the span covers; at least 1.
    pub length: u32,
}

impl GraphError {
    /// An engine refusal with no diagnostic beyond its text.
    pub(crate) fn engine(message: impl Into<String>) -> Self {
        Self::Engine {
            message: message.into(),
            diagnostic: Diagnostic::default(),
        }
    }

    /// The `loams.errors.v1.ErrorInfo.reason` this error is answered with (§48 §8.3).
    #[must_use]
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Engine { message, .. } if message.contains("syntax error") => "gql_syntax_error",
            Self::Engine { .. }
            | Self::EmptyStatement
            | Self::UnboundParameter { .. }
            | Self::InvalidValue(_)
            | Self::TooComplex { .. }
            | Self::OverLimit(_) => "invalid_argument",
            Self::Conflict { .. } => "already_exists",
            Self::ReadOnly => "graph_read_only",
            Self::LanguageUnavailable(_) => "graph_language_disabled",
            Self::TransactionStatement => "graph_transaction_statement",
            Self::StatementNotAllowed { .. } => "graph_statement_not_allowed",
            Self::UnboundedPath { .. } | Self::AllShortestPaths => "graph_unbounded_path",
            Self::ResultTooLarge { .. } | Self::RowTooLarge { .. } => "graph_result_too_large",
            Self::EnginePanic | Self::Failed => "graph_engine_panic",
            Self::Reloading | Self::OwnerMoved(_) => "graph_reloading",
            Self::StatementTimeout => "graph_statement_timeout",
            Self::WriteConflict => "graph_write_conflict",
            Self::VersionMismatch { .. } => "graph_version_mismatch",
            Self::LogUnavailable(_) => "unavailable",
            Self::ConsistencyWaitTimeout { .. } => "consistency_wait_timeout",
            Self::Linked => "graph_is_linked",
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
    /// Durable in a stream of Loams' log (GR1 Task 11): the graph is rebuilt from it when it
    /// opens, and every write goes through the write lane. `None` is GR1a's engine-only mode
    /// (Grafeo's own files), which the engine-level service functions and the mock use.
    durable: Option<crate::durable::DurableSpec>,
    /// A follower (GR1 Task 15): a durable graph's read-only replica, which tails its stream
    /// ([`crate::replica`]) and never appends. Every writing statement is refused.
    follower: bool,
    /// A LINKED graph (GR1 Task 17): only its links write it, so every writing statement is
    /// refused (`graph_is_linked`).
    linked: bool,
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
            durable: None,
            follower: false,
            linked: false,
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

    /// The same spec, durable in `durable`'s stream (GR1 Task 11). Needs a catalog id.
    #[must_use]
    pub fn durable(mut self, durable: crate::durable::DurableSpec) -> Self {
        self.durable = Some(durable);
        self
    }

    /// The same spec for a LINKED graph when `linked` (GR1 Task 17): every writing statement is
    /// refused with `graph_is_linked`.
    #[must_use]
    pub fn linked(mut self, linked: bool) -> Self {
        self.linked = linked;
        self
    }

    /// The same spec, as a follower: read-only, and never appending (GR1 Task 15).
    #[must_use]
    pub fn follower(mut self) -> Self {
        self.follower = true;
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
    /// A write could not be made durable (a failed append, a refused capture); the graph is
    /// rebuilt from its stream on its next call (GR1 Task 11).
    Reloading,
    /// An engine call panicked and the graph cannot be reopened from storage.
    Failed,
}

/// One row of a result. Values positionally aligned with the result's column names, because a graph
/// result legitimately repeats a key across a path and a map would collapse those duplicates.
#[derive(Clone, PartialEq)]
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
#[derive(Clone, Default)]
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
#[derive(Clone, Default, PartialEq)]
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
    /// The engine answered more rows than the caller's cap; `rows` holds the first ones
    /// (GR1 Task 6).
    pub truncated: bool,
    /// The statement can have committed a change (the engine's plan has a mutation; see
    /// [`Verdict::writes`]), so its answer must not be reported as a failure afterwards
    /// (review fix 1, I3). For a batch, whether any of its statements can.
    pub wrote: bool,
    /// Where a durable graph's write is durable, its commit epoch and counters (GR1 Task 11).
    /// `None` for a read, and on a graph that is not durable.
    pub commit: Option<crate::durable::Commit>,
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
    /// The reader-writer gate (R9.3): client statements hold it shared, a durable graph's write
    /// lane exclusively until its writes are durable.
    gate: crate::durable::visibility::Gate,
    /// The durable write path, when the graph is durable (GR1 Task 11).
    durable: Option<crate::durable::Durable>,
    /// Set when a write could not be made durable: the engine holds commits the log does not,
    /// so it is rebuilt from the log before it serves again.
    reloading: AtomicBool,
    /// Set when the graph's owner lease moved on while this engine served it (an append refused
    /// by its fence, or a lease this node lost): it never serves again here (GR1 Task 12).
    fenced: AtomicBool,
    /// How a durable graph's engine was recovered (GR1 Task 14).
    recovery: Option<crate::recovery::Recovery>,
    /// When the graph was last used, on [`clock_ms`]'s clock (GR1 Task 14's LRU).
    last_used_ms: AtomicU64,
}

/// Milliseconds on a monotonic clock that starts with the process: graphs' last-use times.
fn clock_ms() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    u64::try_from(
        START
            .get_or_init(std::time::Instant::now)
            .elapsed()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
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
        let fresh = Arc::new(Graph::open_db(namespace, name, spec(), engine)?);
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
    /// The engine's `query_timeout` is its own statement time limit: a backstop only, because
    /// Grafeo checks it between pipeline chunks, so it stops a statement that streams rows but
    /// not one stuck in a single operator (a cartesian aggregate, an expand), which runs on
    /// (R0.8). Its `max_property_bytes` is Grafeo's `max_property_size` (§48 §13.1).
    fn open_db(
        namespace: &str,
        name: &str,
        mut spec: OpenSpec,
        engine: &Engine,
    ) -> Result<Graph, GraphError> {
        // The engine's configuration for a database file (or in memory, `None`).
        let read_only = spec.read_only;
        let durable_cdc = spec.durable.is_some();
        let config = |file: Option<PathBuf>| {
            let config = match file {
                None => grafeo::Config::in_memory(),
                // `read_only` is the engine's own read-only mode: a shared lock and no WAL
                // replay, so a read-only graph cannot be made to write even by a statement the
                // gate mis-classifies. Grafeo requires an existing `.grafeo` file for it.
                Some(file) if read_only => grafeo::Config::read_only(file),
                Some(file) => grafeo::Config::persistent(file),
            };
            let config = match engine.query_timeout {
                Some(limit) => config.with_query_timeout(limit),
                None => config.without_query_timeout(),
            }
            .with_max_property_size(engine.max_property_bytes);
            // A durable graph's change sets are captured from Grafeo's CDC (R9.1, R9.9).
            if durable_cdc {
                let mut config = config.with_cdc();
                config.cdc_retention = crate::durable::cdc_retention();
                config
            } else {
                config
            }
        };
        // A durable graph's stream is the graph (I4): its local files are a cache, and the engine
        // starts from the base `recovery` chooses (its stamped local cache, a snapshot, or
        // nothing) and replays the records after it (GR1 Task 14).
        let (db, durable, recovery) = match &spec.durable {
            Some(durable) => {
                let id = spec
                    .id
                    .ok_or_else(|| GraphError::engine("a durable graph needs its catalog id"))?;
                if spec.read_only {
                    return Err(GraphError::engine(
                        "a durable graph is not opened read-only: it is rebuilt from its stream",
                    ));
                }
                let (db, state, recovery) =
                    crate::recovery::open(id, durable, spec.dir.as_deref(), &config)?;
                (db, Some(state), Some(recovery))
            }
            None => {
                if let (Some(dir), false) = (&spec.dir, spec.read_only) {
                    std::fs::create_dir_all(dir).map_err(|err| {
                        GraphError::engine(format!("creating the graph's storage: {err}"))
                    })?;
                }
                let db =
                    GrafeoDB::with_config(config(spec.database_file())).map_err(as_engine_error)?;
                (db, None, None)
            }
        };
        // A reload never adopts: by then the local files may hold commits the log refused.
        if let Some(durable) = &mut spec.durable {
            durable.adopt = false;
        }
        Ok(Graph {
            namespace: namespace.to_string(),
            name: name.to_string(),
            db,
            // A follower refuses every write before it reaches the engine: its writes would be
            // appended unfenced, beside its owner's (GR1 Task 15).
            read_only: spec.read_only || spec.follower,
            persistent: spec.dir.is_some(),
            spec,
            poisoned: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            statements_executed: AtomicU64::new(0),
            gate: crate::durable::visibility::Gate::default(),
            durable,
            reloading: AtomicBool::new(false),
            fenced: AtomicBool::new(false),
            recovery,
            last_used_ms: AtomicU64::new(clock_ms()),
        })
    }

    /// How this graph's engine was recovered, for a durable graph (GR1 Task 14).
    pub fn recovery(&self) -> Option<crate::recovery::Recovery> {
        self.recovery
    }

    /// Marks the graph used now, for least-recently-used eviction (GR1 Task 14).
    pub fn touch(&self) {
        self.last_used_ms.fetch_max(clock_ms(), Ordering::Relaxed);
    }

    /// How long since the graph was last used ([`Graph::touch`], or its open).
    pub fn idle_for(&self) -> Duration {
        Duration::from_millis(clock_ms().saturating_sub(self.last_used_ms.load(Ordering::Relaxed)))
    }

    /// The engine's own estimate of the memory it holds, in bytes (Grafeo's `memory_usage`,
    /// which under-counts: R0.9; Task 26 refines it). 0 for a graph that is not serving.
    pub fn resident_bytes(&self) -> u64 {
        self.call(|| Ok(self.db.memory_usage().total_bytes as u64))
            .unwrap_or(0)
    }

    /// The catalog id this graph was opened for, if any.
    pub fn id(&self) -> Option<crate::GraphId> {
        self.spec.id
    }

    /// Node and edge counts. A durable graph answers its durable state's, read without the gate
    /// (so `GetGraph` never waits on a write group, and never counts a write that is not
    /// durable).
    pub fn counts(&self) -> (u64, u64) {
        match &self.durable {
            Some(durable) => durable.position.counts(),
            None => (self.db.node_count() as u64, self.db.edge_count() as u64),
        }
    }

    /// Whether this graph is a follower: a read-only replica tailing its stream (GR1 Task 15).
    pub fn is_follower(&self) -> bool {
        self.spec.follower
    }

    /// Applies the records a follower read from its stream (GR1 Task 15), under the exclusive
    /// gate, so no read sees a record half applied. Answers how many it applied: records below
    /// the follower's position are skipped (a fetch that overlapped the last one).
    ///
    /// # Errors
    ///
    /// [`GraphError::Engine`] for a record that does not decode or apply, or a gap; the caller
    /// marks the follower for reload, since part of the batch may be in the engine already.
    pub(crate) fn follow(&self, records: &[loams_log::OffsetRecord]) -> Result<u64, GraphError> {
        let durable = self
            .durable
            .as_ref()
            .filter(|_| self.spec.follower)
            .ok_or_else(|| GraphError::engine("only a durable follower tails its stream"))?;
        let _exclusive = self.gate.exclusive();
        self.call(|| durable.follow(&self.db, records))
    }

    /// Commits a link target's batch through the write lane (GR1 Task 18), at the link's expected
    /// version, and waits until it is durable. Answers the graph's version after it (its token's
    /// offset). Blocks: run it on the statement pool. Only the owner commits: a follower's engine
    /// has no lane that appends.
    ///
    /// # Errors
    ///
    /// [`GraphError::VersionMismatch`] when the link's version moved, [`GraphError::Reloading`]
    /// when the record could not be made durable (the graph reloads; whether it landed, the
    /// stream tells), and what [`Graph::call`] answers.
    pub(crate) fn commit_link(
        &self,
        job: crate::durable::lane::LinkJob,
    ) -> Result<u64, GraphError> {
        let durable = self
            .durable
            .as_ref()
            .filter(|_| !self.spec.follower)
            .ok_or_else(|| GraphError::engine("only a durable graph's owner commits a link"))?;
        let job = crate::durable::lane::LaneJob {
            statements: Vec::new(),
            access: Access::Write,
            writes: true,
            options: crate::durable::WriteOptions::default(),
            max_rows: None,
            link: Some(job),
        };
        let committed = self.call(|| durable.lane.submit(self, durable, job))?;
        Ok(committed.commit.token.offset)
    }

    /// Link `link`'s latest durable progress in this graph (GR1 Task 18).
    pub(crate) fn link_progress(
        &self,
        link: loams_common::meta::LinkId,
    ) -> Option<crate::changeset::LinkApply> {
        self.durable
            .as_ref()
            .and_then(|durable| durable.link_progress(link))
    }

    /// Whether writes to this graph are durable in Loams' log (GR1 Task 11).
    pub fn is_durable(&self) -> bool {
        self.durable.is_some()
    }

    /// The durable position (the stream's next offset, R9.5) as it moves, for a durable graph:
    /// the shared contract's `durable_epoch` (Task 15 waits on it).
    pub fn durable_epoch(&self) -> Option<tokio::sync::watch::Receiver<u64>> {
        self.durable
            .as_ref()
            .map(|durable| durable.position.subscribe())
    }

    /// The token of the graph's durable state, for a durable graph.
    pub fn consistency_token(&self) -> Option<crate::durable::ConsistencyToken> {
        self.durable.as_ref().map(crate::durable::Durable::token)
    }

    /// The graph's stream, for a durable graph.
    pub fn stream(&self) -> Option<loams_common::StreamId> {
        self.durable.as_ref().map(|durable| durable.stream)
    }

    /// The engine's epoch at the durable state, for a durable graph.
    pub(crate) fn durable_engine_epoch(&self) -> Option<u64> {
        self.durable
            .as_ref()
            .map(|durable| durable.position.engine_epoch())
    }

    /// The graph's gate (R9.3).
    pub(crate) fn gate(&self) -> &crate::durable::visibility::Gate {
        &self.gate
    }

    /// The durable write path's state, for a durable graph.
    pub(crate) fn durable_state(&self) -> Option<&crate::durable::Durable> {
        self.durable.as_ref()
    }

    /// Every node and edge of the graph, in id order, read under its gate (shared), so it is the
    /// durable state of a durable graph: a canonical dump, and what a portable snapshot holds
    /// (GR1 Task 13).
    ///
    /// # Errors
    ///
    /// What [`Graph::call`] answers for a poisoned or panicking engine.
    pub fn rows(&self) -> Result<crate::snapshot::portable::GraphRows, GraphError> {
        self.read_call(|| Ok(crate::snapshot::portable::GraphRows::read(&self.db)))
    }

    /// The engine.
    pub(crate) fn db(&self) -> &GrafeoDB {
        &self.db
    }

    /// Counts one executed statement.
    pub(crate) fn count_statement(&self) {
        self.statements_executed.fetch_add(1, Ordering::Relaxed);
    }

    /// Marks the graph for reload: the engine holds commits its stream does not. Called by the
    /// lane under the exclusive gate, so no client statement runs on this engine again.
    pub(crate) fn mark_reloading(&self) {
        self.reloading.store(true, Ordering::Release);
    }

    /// Marks the graph fenced: its owner lease moved on, so this engine never serves again; the
    /// caller places the graph anew (GR1 Task 12). Also marks it for reload, so the lane and every
    /// call refuse it at once.
    pub fn mark_fenced(&self) {
        self.fenced.store(true, Ordering::Release);
        self.reloading.store(true, Ordering::Release);
    }

    /// Whether the graph's owner lease moved on while this engine served it.
    pub fn is_fenced(&self) -> bool {
        self.fenced.load(Ordering::Acquire)
    }

    /// The owner lease epoch this engine serves under, for a durable graph whose appends are
    /// fenced (GR1 Task 12).
    pub fn lease_epoch(&self) -> Option<u64> {
        self.durable
            .as_ref()
            .and_then(|durable| durable.fence.as_ref())
            .map(|fence| fence.epoch)
    }

    /// Marks the graph poisoned by an engine panic (R0.13).
    pub(crate) fn poison(&self, why: &str) {
        self.poisoned.store(true, Ordering::Release);
        tracing::error!(
            namespace = %self.namespace,
            name = %self.name,
            "{why}; the graph is poisoned and reopens on its next call"
        );
    }

    /// Whether the graph must be reopened before it serves: poisoned by a panic, or holding
    /// commits its stream refused.
    pub(crate) fn needs_reopen(&self) -> bool {
        self.is_poisoned() || self.reloading.load(Ordering::Acquire)
    }

    /// Labels, edge types (each with its count), property keys and indexes, sorted (GetSchema).
    ///
    /// # Errors
    ///
    /// What [`Graph::call`] answers for a poisoned or panicking engine.
    pub fn schema(&self) -> Result<SchemaSummary, GraphError> {
        self.read_call(|| {
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
        } else if self.reloading.load(Ordering::Acquire) {
            GraphState::Reloading
        } else {
            GraphState::Ready
        }
    }

    /// Whether the graph needs [`Engine::reopen_if_poisoned`] (or is failed) before it serves.
    pub fn is_poisoned_or_failed(&self) -> bool {
        self.needs_reopen() || self.failed.load(Ordering::Acquire)
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
        if self.needs_reopen() {
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

    /// [`Graph::call`] for a client statement: under the graph's gate, shared, so it never runs
    /// while a write is not durable (R9.3). The poisoned check inside `call` runs after the gate
    /// is taken, so a statement that waited on a group the log refused answers `graph_reloading`.
    pub(crate) fn read_call<T: Send>(
        &self,
        f: impl FnOnce() -> Result<T, GraphError> + Send,
    ) -> Result<T, GraphError> {
        let _shared = self.gate.shared();
        self.call(f)
    }

    /// The access a statement needs, refusing it on a read-only request or graph, and refusing
    /// schema DDL outright.
    ///
    /// Schema DDL (`Access::Admin`: node, edge and graph types, indexes, constraints, schemas,
    /// procedures) is not served yet (fix round 2; GR1 R24.6): Task 24 gives it to the graph's
    /// `admin` only, and it runs once a portable snapshot carries schema changes. Grafeo's `Session::execute`, which
    /// a statement without parameters takes for the engine's time limit, runs it directly, with
    /// no Loams authorisation and outside CDC; the parameterised path is refused the same way so
    /// the answer does not depend on the bindings.
    fn admit(
        &self,
        statement: &str,
        read_only: bool,
        limits: &StatementLimits,
    ) -> Result<Verdict, GraphError> {
        #[cfg(feature = "failpoints")]
        fail::fail_point!("loams_graph::gate");
        let verdict = crate::classify::gate_verdict_cached(statement, limits)?;
        if let Some(engine) = verdict.engine
            && engine != verdict.guard
        {
            crate::authz::note_disagreement(statement, verdict.guard, engine);
        }
        let access = verdict.access;
        if access == Access::Admin {
            return Err(GraphError::StatementNotAllowed {
                file_access: false,
                what: "schema DDL is not served yet: it needs the graph's admin relation (GR1 \
                       Task 24), and runs once a schema change is carried by every snapshot \
                       (R24.6)"
                    .to_string(),
            });
        }
        if self.spec.linked && access != Access::Read {
            return Err(GraphError::Linked);
        }
        if (read_only || self.read_only) && access != Access::Read {
            return Err(GraphError::ReadOnly);
        }
        Ok(verdict)
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
        self.execute_within(
            statement,
            parameters,
            read_only,
            &StatementLimits::DEFAULT,
            None,
        )
    }

    /// [`Graph::execute_with_params`] within a statement's limits (GR1 Task 6): its size,
    /// parameters, operator chains and paths are checked before it runs, and at most `max_rows`
    /// rows are kept (the rest are dropped before any path is resolved, and `truncated` is set).
    ///
    /// # Errors
    ///
    /// As [`Graph::execute_with_params`], and [`GraphError::OverLimit`].
    pub fn execute_within(
        &self,
        statement: &str,
        parameters: HashMap<String, Value>,
        read_only: bool,
        limits: &StatementLimits,
        max_rows: Option<usize>,
    ) -> Result<GraphResult, GraphError> {
        self.execute_with(
            statement,
            parameters,
            read_only,
            limits,
            max_rows,
            &crate::durable::WriteOptions::default(),
        )
    }

    /// [`Graph::execute_within`] with a write's options (GR1 Task 11). On a durable graph a
    /// statement that writes runs in the write lane and answers once it is durable, with its
    /// [`GraphResult::commit`]; `options` apply to such a statement only. A read runs under the
    /// gate, shared.
    ///
    /// # Errors
    ///
    /// As [`Graph::execute_within`]; on a durable graph also [`GraphError::Reloading`] when the
    /// write could not be made durable, and [`GraphError::WriteConflict`].
    pub fn execute_with(
        &self,
        statement: &str,
        parameters: HashMap<String, Value>,
        read_only: bool,
        limits: &StatementLimits,
        max_rows: Option<usize>,
        options: &crate::durable::WriteOptions,
    ) -> Result<GraphResult, GraphError> {
        limits.check_parameters(parameters.len())?;
        // The gate and the row building run inside the same panic containment as the engine
        // call (security review M5).
        let verdict = self.call(|| self.admit(statement, read_only, limits))?;
        // A statement that names a version runs in the lane even when it reads, so the version
        // is checked against the durable state before it runs (PR #424 review).
        if (verdict.access != Access::Read || options.if_version.is_some())
            && let Some(durable) = &self.durable
        {
            let job = crate::durable::lane::LaneJob {
                statements: vec![BatchStatement {
                    text: statement.to_string(),
                    parameters,
                }],
                access: verdict.access,
                writes: verdict.writes(),
                options: options.clone(),
                max_rows,
                link: None,
            };
            let committed = self.call(|| durable.lane.submit(self, durable, job))?;
            return Ok(batch_results(committed, 1).pop().unwrap_or_default());
        }
        self.read_call(|| {
            let mut result = self.run_engine(verdict.access, statement, parameters, max_rows)?;
            result.wrote = verdict.writes();
            Ok(result)
        })
    }

    /// Runs one admitted statement on the session for `access`: the one path every single
    /// statement takes into the engine. Always called inside [`Graph::call`].
    fn run_engine(
        &self,
        access: Access,
        statement: &str,
        parameters: HashMap<String, Value>,
        max_rows: Option<usize>,
    ) -> Result<GraphResult, GraphError> {
        let session = crate::durable::Visibility::session(self, access);
        let result = run_on_session(&session, statement, parameters).map_err(as_engine_error)?;
        self.statements_executed.fetch_add(1, Ordering::Relaxed);
        Ok(self.resolved(GraphResult::from_engine(result, max_rows)))
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
        self.read_call(|| self.run_engine(access, statement, parameters, None))
    }

    /// The plan of a statement, without running it (`Explain`, GR1 Task 6).
    ///
    /// The statement is gated like any other (its limits, file access, graph management,
    /// unbounded paths), then translated, bound and optimised the way the engine's own `EXPLAIN`
    /// does, from the caller's text as it is: nothing is prefixed to it (D634) and nothing runs,
    /// so a write's plan is answered too.
    ///
    /// # Errors
    ///
    /// A refusal of [`crate::classify::gate_within`], or the engine's error.
    pub fn explain_within(
        &self,
        statement: &str,
        limits: &StatementLimits,
    ) -> Result<ExplainedPlan, GraphError> {
        use grafeo_engine::query::Optimizer;
        use grafeo_engine::query::binder::Binder;
        use grafeo_engine::query::translators::gql::{GqlTranslationResult, translate_full};
        self.read_call(|| {
            gate_within(statement, QueryLanguage::Gql, limits)?;
            let plan = match translate_full(statement) {
                Ok(GqlTranslationResult::Plan(plan)) => plan,
                Ok(_) => {
                    return Err(GraphError::engine(
                        "the statement is a command, not a query; it has no plan",
                    ));
                }
                Err(err) => return Err(as_engine_error(err)),
            };
            Binder::new().bind(&plan).map_err(as_engine_error)?;
            let optimized = Optimizer::from_store(self.db.store())
                .optimize(plan)
                .map_err(as_engine_error)?;
            Ok(ExplainedPlan {
                text: optimized.root.explain_tree(),
                root: plan_node(&optimized.root),
            })
        })
    }

    /// Runs a `PROFILE` statement and answers its plan with what each operator produced
    /// (`Explain` with `profile`, GR1 Task 6).
    ///
    /// The caller's statement must itself be a `PROFILE`: Loams never adds the keyword (D634).
    /// It runs as a read, so a statement that writes is refused with `graph_read_only` before
    /// anything runs (Task 2).
    ///
    /// # Errors
    ///
    /// [`GraphError::InvalidValue`] without the keyword, [`GraphError::ReadOnly`] for a write, or
    /// what [`Graph::execute_within`] answers.
    pub fn profile_within(
        &self,
        statement: &str,
        parameters: HashMap<String, Value>,
        limits: &StatementLimits,
    ) -> Result<ExplainedPlan, GraphError> {
        use grafeo_engine::query::translators::gql::{GqlTranslationResult, translate_full};
        // Its limits and refusals first, before the translation below parses it; a write is
        // refused whether or not it says PROFILE (Task 2's `profile_refuses_a_write`).
        if gate_within(statement, QueryLanguage::Gql, limits)? != Access::Read {
            return Err(GraphError::ReadOnly);
        }
        let is_profile = crate::classify::on_big_stack(|| {
            matches!(translate_full(statement), Ok(GqlTranslationResult::Plan(plan)) if plan.profile)
        })
        .unwrap_or(false);
        if !is_profile {
            return Err(GraphError::InvalidValue(
                "a profile runs the statement as written: begin it with PROFILE (Loams does not \
                 rewrite a statement, D634)"
                    .to_string(),
            ));
        }
        let result = self.execute_within(statement, parameters, true, limits, None)?;
        let text = match result.rows.first().and_then(|row| row.values.first()) {
            Some(Value::String(text)) => text.to_string(),
            _ => {
                return Err(GraphError::engine("the engine answered no profile"));
            }
        };
        Ok(ExplainedPlan {
            root: parse_profile(&text),
            text,
        })
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
        self.execute_batch_within(statements, &StatementLimits::DEFAULT, None)
    }

    /// [`Graph::execute_batch`] within a statement's limits (GR1 Task 6): the batch's length
    /// and every statement are checked before any runs, and each result keeps at most
    /// `max_rows` rows.
    ///
    /// # Errors
    ///
    /// As [`Graph::execute_batch`], and [`GraphError::OverLimit`].
    pub fn execute_batch_within(
        &self,
        statements: &[BatchStatement],
        limits: &StatementLimits,
        max_rows: Option<usize>,
    ) -> Result<Vec<GraphResult>, GraphError> {
        self.execute_batch_with(
            statements,
            limits,
            max_rows,
            &crate::durable::WriteOptions::default(),
        )
    }

    /// [`Graph::execute_batch_within`] with the batch's options (GR1 Task 11). On a durable graph
    /// the transaction runs in the write lane, read-only or not (an explicit transaction commits
    /// through the lane, R0.5), and is one record in the graph's stream; every result carries the
    /// batch's [`GraphResult::commit`]. A batch answered from the idempotency index has one empty
    /// result per statement. `options` apply to a durable graph only.
    ///
    /// # Errors
    ///
    /// As [`Graph::execute_batch_within`], and on a durable graph as [`Graph::execute_with`]
    /// and [`GraphError::VersionMismatch`].
    pub fn execute_batch_with(
        &self,
        statements: &[BatchStatement],
        limits: &StatementLimits,
        max_rows: Option<usize>,
        options: &crate::durable::WriteOptions,
    ) -> Result<Vec<GraphResult>, GraphError> {
        limits.check_batch(statements.len())?;
        for statement in statements {
            limits.check_parameters(statement.parameters.len())?;
        }
        // Every statement is gated before any runs, so a refused one leaves nothing
        // half-applied; the gate runs inside the panic containment (M5).
        let (access, writes) = self.call(|| {
            let mut access = Access::Read;
            let mut writes = false;
            for statement in statements {
                let verdict = self.admit(&statement.text, false, limits)?;
                access = access.max(verdict.access);
                writes |= verdict.writes();
            }
            Ok((access, writes))
        })?;
        if let Some(durable) = &self.durable {
            let job = crate::durable::lane::LaneJob {
                statements: statements.to_vec(),
                access,
                writes,
                options: options.clone(),
                max_rows,
                link: None,
            };
            let committed = self.call(|| durable.lane.submit(self, durable, job))?;
            return Ok(batch_results(committed, statements.len()));
        }
        self.read_call(|| {
            let mut session = self.session_for(access);
            session.begin_transaction().map_err(as_engine_error)?;
            let mut out = Vec::with_capacity(statements.len());
            for statement in statements {
                // The engine takes the bindings by value, hence the copy.
                let bound = statement.parameters.clone();
                match run_on_session(&session, &statement.text, bound) {
                    Ok(result) => {
                        self.statements_executed.fetch_add(1, Ordering::Relaxed);
                        out.push(GraphResult::from_engine(result, max_rows));
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
                .map(|mut result| {
                    result.wrote = writes;
                    self.resolved(result)
                })
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
    /// as a `Node` or `Relationship`. The lookup reads the current state. On a durable graph that
    /// is the statement's own state, since the statement holds the gate (R9.3: a read's gate is
    /// shared, so no write commits meanwhile; a write resolves inside the lane). On a graph that
    /// is not durable, a write committed between the statement and this lookup is visible here,
    /// and an element deleted in that window stays an id (R2.4).
    pub(crate) fn resolved(&self, mut result: GraphResult) -> GraphResult {
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

/// One operator of a statement's plan (`Explain`, GR1 Task 6).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct PlanNode {
    /// The operator, for example `NodeScan`.
    pub name: String,
    /// What it works on, as the engine labels it (for example `n:Person`).
    pub label: String,
    /// The engine's one-line rendering of it.
    pub line: String,
    /// Its inputs.
    pub children: Vec<PlanNode>,
    /// PROFILE only: the rows it produced.
    pub rows: Option<u64>,
    /// PROFILE only: its own time, in nanoseconds.
    pub elapsed_nanos: Option<u64>,
}

/// A statement's plan (`Explain`, GR1 Task 6).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ExplainedPlan {
    /// The operator tree.
    pub root: PlanNode,
    /// The engine's own rendering.
    pub text: String,
}

/// A logical operator and its inputs, as a [`PlanNode`] tree.
fn plan_node(op: &grafeo_engine::query::LogicalOperator) -> PlanNode {
    let tree = op.explain_tree();
    let line = tree.lines().next().unwrap_or_default().trim().to_string();
    let name = line
        .split([' ', '('])
        .next()
        .unwrap_or_default()
        .to_string();
    PlanNode {
        name,
        label: op.display_label(),
        line,
        children: op.children().into_iter().map(plan_node).collect(),
        rows: None,
        elapsed_nanos: None,
    }
}

/// Reads Grafeo 0.5.43's PROFILE text (`grafeo-engine` `query/profile.rs`): one line per
/// operator, `{indent}{name} ({label})  rows={n}  time={ms}ms`, two spaces of indent per level,
/// then a blank line and the total.
fn parse_profile(text: &str) -> PlanNode {
    let mut stack: Vec<(usize, PlanNode)> = Vec::new();
    let mut roots = Vec::new();
    let attach = |stack: &mut Vec<(usize, PlanNode)>, roots: &mut Vec<PlanNode>, node| match stack
        .last_mut()
    {
        Some((_, parent)) => parent.children.push(node),
        None => roots.push(node),
    };
    for line in text.lines() {
        if line.trim().is_empty() {
            break;
        }
        let depth = (line.len() - line.trim_start().len()) / 2;
        let body = line.trim();
        let (head, stats) = body.rsplit_once("  rows=").unwrap_or((body, ""));
        let (rows, time) = stats.split_once("  time=").unwrap_or((stats, ""));
        let (name, label) = match head.split_once(" (") {
            Some((name, rest)) => (name, rest.strip_suffix(')').unwrap_or(rest)),
            None => (head, ""),
        };
        let node = PlanNode {
            name: name.to_string(),
            label: label.to_string(),
            // The operator alone: its row count and time are `rows` and `elapsed_nanos`, so the
            // line stays the same from run to run (GR1 Task 7's `explain.json`).
            line: head.to_string(),
            children: Vec::new(),
            rows: rows.trim().parse().ok(),
            elapsed_nanos: time
                .trim()
                .strip_suffix("ms")
                .and_then(|ms| ms.parse::<f64>().ok())
                .map(|ms| (ms * 1_000_000.0) as u64),
        };
        while stack.last().is_some_and(|(d, _)| *d >= depth) {
            let (_, done) = stack.pop().expect("checked");
            attach(&mut stack, &mut roots, done);
        }
        stack.push((depth, node));
    }
    while let Some((_, done)) = stack.pop() {
        attach(&mut stack, &mut roots, done);
    }
    match roots.len() {
        1 => roots.pop().expect("one root"),
        _ => PlanNode {
            name: "Profile".to_string(),
            children: roots,
            ..PlanNode::default()
        },
    }
}

// Debug forms that never print a value (review fix 1, M3; the Global Constraints' "Secrets and
// literals"): rows print their size, statements their redacted text and parameter names, plans
// their redacted labels.

impl fmt::Debug for GraphRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GraphRow")
            .field("values", &self.values.len())
            .finish()
    }
}

impl fmt::Debug for BatchStatement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut names: Vec<&String> = self.parameters.keys().collect();
        names.sort();
        f.debug_struct("BatchStatement")
            .field("text", &crate::redact::redact_literals(&self.text))
            .field("parameters", &names)
            .finish()
    }
}

impl fmt::Debug for GraphResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GraphResult")
            .field("columns", &self.columns)
            .field("column_types", &self.column_types)
            .field("rows", &self.rows.len())
            .field("rows_read", &self.rows_read)
            .field("elapsed_nanos", &self.elapsed_nanos)
            .field("truncated", &self.truncated)
            .field("wrote", &self.wrote)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for PlanNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlanNode")
            .field("name", &self.name)
            .field("label", &crate::redact::redact_literals(&self.label))
            .field("line", &crate::redact::redact_literals(&self.line))
            .field("children", &self.children)
            .field("rows", &self.rows)
            .field("elapsed_nanos", &self.elapsed_nanos)
            .finish()
    }
}

impl fmt::Debug for ExplainedPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExplainedPlan")
            .field("root", &self.root)
            .field("text", &crate::redact::redact_literals(&self.text))
            .finish()
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

/// What [`Engine::evict`] did with a graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Closed {
    /// It was not open here (with that id), and nothing was removed.
    NotOpen,
    /// A caller holds it: it stays open.
    Busy,
    /// Closed, its local files kept but not stamped: in memory, not durable, or not serving
    /// cleanly (its next open rebuilds it).
    Closed,
    /// Closed, and its local files stamped: its next open starts from them.
    Stamped,
    /// Closed (if it was open), and its local files removed.
    Removed,
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
    /// The largest property value a graph stores (Grafeo's `max_property_size`; §48 §13.1).
    max_property_bytes: usize,
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
            max_property_bytes: StatementLimits::DEFAULT.max_property_bytes,
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

    /// The same engine with another largest property value for the graphs it opens from now
    /// on, within [`StatementLimits::MAXIMUM`] (§48 §13.1: default 1 MiB, at most 16 MiB).
    #[must_use]
    pub fn with_max_property_bytes(mut self, bytes: usize) -> Self {
        self.max_property_bytes = bytes.clamp(1, StatementLimits::MAXIMUM.max_property_bytes);
        self
    }

    /// The largest property value graphs are opened with.
    #[must_use]
    pub fn max_property_bytes(&self) -> usize {
        self.max_property_bytes
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

    /// Reopens a graph an engine panic poisoned (R0.13), or whose write could not be made durable
    /// (GR1 Task 11), from its own spec: a durable graph is rebuilt from its stream; otherwise a
    /// persistent graph comes back with everything it committed, and an in-memory one fails. A
    /// graph that needs neither is answered as it is.
    ///
    /// # Errors
    ///
    /// [`GraphError::Reloading`] while another caller still holds the old handle (its file
    /// lock is not released yet), or the engine's (or the log's) error opening the storage.
    pub fn reopen_if_poisoned(&self, graph: Arc<Graph>) -> Result<Arc<Graph>, GraphError> {
        if !graph.needs_reopen() {
            return Ok(graph);
        }
        if graph.failed.load(Ordering::Acquire) {
            return Err(GraphError::Failed);
        }
        let durable = graph.durable.is_some();
        // An in-memory graph has nothing to reopen from: reopening it would serve an empty graph
        // in its place and lose every acknowledged write without a word (I2). It fails instead.
        // A durable one is rebuilt from its stream.
        if !graph.persistent && !durable {
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
            && !current.needs_reopen()
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
        // could serve a graph missing acknowledged writes, so the graph fails instead (I2). A
        // durable graph's files are not what it reopens from.
        if let Err(err) = old.db.close()
            && !durable
        {
            tracing::error!(%namespace, %name, error = %err, "closing a poisoned graph failed; it is failed, not reopened");
            old.failed.store(true, Ordering::Release);
            graphs.insert(key, old);
            return Err(GraphError::Failed);
        }
        drop(old);
        // The reopen runs outside the registry lock, under the graph's opening latch (PR #424
        // review): a durable graph replays its whole stream, which must not stall every other
        // graph's lookups, nor the async handlers that list graphs while the replay needs the
        // runtime. The old graph is out of the registry, so the latch opens a fresh one, or
        // answers one a concurrent opener registered meanwhile.
        drop(graphs);
        let fresh = Graph::open_latched(self, &namespace, &name, || spec)?;
        tracing::info!(%namespace, %name, durable, "reopened a graph");
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

    /// Drops a durable graph's engine that must not serve again: one whose owner lease moved on
    /// (fenced), or one opened under an older lease epoch than the one this node now holds (GR1
    /// Task 12). Its local files are a cache, so a close that fails is only logged. Answers whether
    /// a graph was dropped.
    ///
    /// # Errors
    ///
    /// [`GraphError::Reloading`] while a caller still holds the handle; it is dropped by the next
    /// call once that caller lets go.
    pub fn discard(&self, namespace: &str, name: &str) -> Result<bool, GraphError> {
        let key = (namespace.to_string(), name.to_string());
        let mut graphs = self
            .graphs
            .lock()
            .map_err(|_| poisoned("the graph registry is poisoned"))?;
        let Some(graph) = graphs.get(&key) else {
            return Ok(false);
        };
        if Arc::strong_count(graph) > 1 {
            return Err(GraphError::Reloading);
        }
        if let Err(err) = graph.db.close() {
            tracing::warn!(%namespace, %name, error = %err, "closing a discarded graph failed; dropping it");
        }
        Ok(graphs.remove(&key).is_some())
    }

    /// Closes graph `namespace`/`name` (the one with id `id`, when given) unless a caller holds
    /// it (GR1 Task 14: eviction, shutdown, and a deleted graph's release), under the graph's
    /// opening latch, so no open of it runs meanwhile:
    ///
    /// * `keep`: its local files stay. A durable graph on disk that is serving cleanly (not
    ///   reloading, poisoned, fenced or failed) is stamped after its close
    ///   ([`crate::recovery::CACHE_STAMP`]), so its next open starts from them.
    /// * otherwise its local directory is removed, open or not (a deleted graph's cache).
    ///
    /// # Errors
    ///
    /// A poisoned registry, or a local directory that cannot be removed.
    pub fn evict(
        &self,
        namespace: &str,
        name: &str,
        id: Option<crate::GraphId>,
        keep: bool,
    ) -> Result<Closed, GraphError> {
        validate_names(namespace, name)?;
        let key = (namespace.to_string(), name.to_string());
        let latch = {
            let mut latches = self
                .opening
                .lock()
                .map_err(|_| poisoned("the graph opening latches are poisoned"))?;
            Arc::clone(latches.entry(key.clone()).or_default())
        };
        let _release = LatchRelease {
            engine: self,
            key: &key,
            latch: &latch,
        };
        let _opening = latch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let graph = {
            let mut graphs = self
                .graphs
                .lock()
                .map_err(|_| poisoned("the graph registry is poisoned"))?;
            match graphs.get(&key) {
                Some(open) if id.is_none_or(|id| open.id() == Some(id)) => {
                    if Arc::strong_count(open) > 1 {
                        return Ok(Closed::Busy);
                    }
                    graphs.remove(&key)
                }
                _ => None,
            }
        };
        let dir = match (&graph, id) {
            (Some(graph), _) => graph.spec.dir.clone(),
            (None, Some(id)) => self
                .data_dir
                .as_ref()
                .map(|dir| dir.join("graphs").join(id.to_string())),
            (None, None) => None,
        };
        let mut closed = if graph.is_some() {
            Closed::Closed
        } else {
            Closed::NotOpen
        };
        if let Some(graph) = graph {
            let clean = keep
                && graph.persistent
                && !graph.needs_reopen()
                && !graph.is_fenced()
                && !graph.failed.load(Ordering::Acquire);
            let pending = graph
                .durable
                .as_ref()
                .filter(|_| clean)
                .map(crate::recovery::PendingStamp::of);
            match graph.db.close() {
                Ok(()) => {
                    if let (Some(pending), Some(dir)) = (pending, &dir) {
                        match crate::recovery::write_stamp(dir, pending) {
                            Ok(()) => closed = Closed::Stamped,
                            Err(err) => {
                                tracing::warn!(%namespace, %name, error = %err, "stamping a graph's local cache failed; its next open rebuilds it");
                            }
                        }
                    }
                }
                Err(err) => {
                    tracing::warn!(%namespace, %name, error = %err, "closing a graph failed; its local cache is not trusted");
                }
            }
        }
        if !keep && let Some(dir) = dir {
            let existed = dir.exists();
            crate::recovery::clear(&dir)?;
            if existed || closed != Closed::NotOpen {
                closed = Closed::Removed;
            }
        }
        Ok(closed)
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
            return Err(GraphError::engine(format!(
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
    GraphError::engine(what)
}

/// Runs one GQL statement on a session, the way Grafeo's own `execute_language` routes it:
/// `execute` when there are no parameters, `execute_with_params` otherwise.
///
/// Both paths enforce the session's role, and both honour Grafeo's `query_timeout` as the
/// engine backstop (R0.8): upstream 0.5.43 ran `execute_with_params` with no deadline, which the
/// fork's patch LG-0002 fixes. The split stays because `execute` is the path with the plan cache
/// and the session and schema commands (R0.10), and `execute_with_params` the only one that binds
/// parameters.
pub(crate) fn run_on_session(
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
pub(crate) fn as_engine_error(err: GrafeoError) -> GraphError {
    use grafeo_common::utils::error::ErrorCode;
    match err.error_code() {
        ErrorCode::QueryTimeout => return GraphError::StatementTimeout,
        // A commit that lost to another transaction, under snapshot or serializable isolation
        // (§48 §7.3): rolled back, and safe to retry.
        ErrorCode::TransactionConflict | ErrorCode::TransactionSerialization => {
            return GraphError::WriteConflict;
        }
        _ => {}
    }
    const MISSING: &str = "Missing parameter: $";
    let text = err.to_string();
    match text.split_once(MISSING) {
        Some((_, name)) if text.starts_with("GRAFEO-X001: Internal error: ") => {
            GraphError::UnboundParameter {
                name: name.trim().to_string(),
            }
        }
        _ => GraphError::Engine {
            message: text,
            diagnostic: diagnostic_of(&err),
        },
    }
}

/// The GQLSTATUS Grafeo maps the error to, and the error's place in the statement when it has a
/// span (GR1 Task 7).
///
/// The place is computed from the span's byte offsets over the statement Grafeo attached, so
/// `column` and `length` count characters (what an editor underlines), not bytes. A span with no
/// statement attached, or offsets that do not fit it, keeps Grafeo's own line and column.
fn diagnostic_of(err: &GrafeoError) -> Diagnostic {
    let gqlstatus = grafeo_common::utils::GqlStatus::from(err)
        .as_str()
        .to_string();
    let position = match err {
        // Grafeo's own line and column (with a byte length) when there is no statement attached,
        // or when the span's offsets are not character boundaries of it.
        GrafeoError::Query(query) => query.span.map(|span| {
            query
                .source_query
                .as_deref()
                .and_then(|source| position_in(source, span.start, span.end))
                .unwrap_or(Position {
                    line: span.line,
                    column: span.column,
                    length: u32::try_from(span.end.saturating_sub(span.start))
                        .unwrap_or(u32::MAX)
                        .max(1),
                })
        }),
        _ => None,
    };
    Diagnostic {
        gqlstatus: Some(gqlstatus),
        position,
    }
}

/// The 1-based line and column of byte `start` in `source`, and the characters up to byte `end`
/// (at least 1, so an error at the end of the statement still underlines something). `None` when
/// the offsets are not character boundaries of `source`.
fn position_in(source: &str, start: usize, end: usize) -> Option<Position> {
    let before = source.get(..start)?;
    let spanned = source.get(start..end.max(start))?;
    let line_start = before.rfind('\n').map_or(0, |at| at + 1);
    let count = |text: &str| u32::try_from(text.chars().count()).unwrap_or(u32::MAX);
    Some(Position {
        line: u32::try_from(before.matches('\n').count())
            .unwrap_or(u32::MAX)
            .saturating_add(1),
        column: count(&before[line_start..]).saturating_add(1),
        length: count(spanned).max(1),
    })
}

/// The results of a transaction the lane committed, each carrying its commit. A transaction
/// answered from the idempotency index kept no rows: it answers `statements` empty results, each
/// marked as having written (it committed, on its first run).
fn batch_results(
    committed: crate::durable::lane::Committed,
    statements: usize,
) -> Vec<GraphResult> {
    let commit = committed.commit;
    if commit.replayed {
        return (0..statements)
            .map(|_| GraphResult {
                wrote: true,
                commit: Some(commit),
                ..GraphResult::default()
            })
            .collect();
    }
    committed
        .results
        .into_iter()
        .map(|mut result| {
            result.commit = Some(commit);
            result
        })
        .collect()
}

/// Converts one engine result into Loams's shape.
impl From<QueryResult> for GraphResult {
    fn from(result: QueryResult) -> Self {
        Self::from_engine(result, None)
    }
}

impl GraphResult {
    /// Converts one engine result, keeping at most `max_rows` rows (GR1 Task 6). The rows are
    /// moved, not copied, and the ones past the cap are dropped here, before any path in them is
    /// resolved.
    pub(crate) fn from_engine(result: QueryResult, max_rows: Option<usize>) -> Self {
        // Read the engine's own counters before taking the rows: `rows_scanned` is set to the
        // number of rows *returned* (`session/mod.rs`), so it is a real measurement but not of rows
        // read from storage, and it is named here rather than at the call site.
        let rows_read = result.rows_scanned;
        let elapsed_nanos = result.execution_time_ms.map(|ms| (ms * 1_000_000.0) as u64);
        // `LogicalType`'s own rendering (`INT64`, `STRING`, `NODE`), which is a type *name*
        // rather than a GQL type tag: the surface is GQL, not SQL.
        let column_types = result
            .column_types
            .iter()
            .map(|ty| ty.to_string())
            .collect();
        let columns = result.columns.clone();
        let mut rows = result.into_rows();
        let truncated = max_rows.is_some_and(|max| rows.len() > max);
        if let Some(max) = max_rows {
            rows.truncate(max);
        }
        Self {
            column_types,
            columns,
            rows: rows.into_iter().map(|values| GraphRow { values }).collect(),
            rows_read,
            bytes_read: None,
            elapsed_nanos,
            rows_affected: None,
            truncated,
            wrote: false,
            commit: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grafeo_common::utils::error::TransactionError;

    #[test]
    fn conflicts_are_write_conflicts() {
        // Under the write lane transactions run one at a time, so a conflict cannot be provoked
        // through it (R11.7); the mapping is pinned here.
        for err in [
            TransactionError::Conflict,
            TransactionError::WriteConflict("node 1".to_string()),
            TransactionError::SerializationFailure("read-write".to_string()),
        ] {
            let mapped = as_engine_error(GrafeoError::Transaction(err));
            assert_eq!(mapped, GraphError::WriteConflict);
            assert_eq!(mapped.reason(), "graph_write_conflict");
        }
    }
}
