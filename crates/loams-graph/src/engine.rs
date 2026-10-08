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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use grafeo::{Error as GrafeoError, GrafeoDB, QueryResult, Value};

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
}

/// How to open a graph.
#[derive(Debug, Clone, Default)]
pub struct OpenSpec {
    /// Where the graph's storage lives. Empty means in memory, and the graph is then ephemeral: it
    /// dies with this process.
    pub database_path: PathBuf,
    /// Open read-only: every writing statement is refused before it reaches the engine.
    pub read_only: bool,
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
    /// The path this graph was opened with, kept because `GrafeoDB` does not expose it back and
    /// `open` has to compare it to decide whether a reopen agrees.
    path: Option<PathBuf>,
    read_only: bool,
    persistent: bool,
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
            if existing.read_only != spec.read_only
                || existing.db_path() != spec.database_path.as_path()
            {
                return Err(GraphError::Conflict {
                    namespace: namespace.to_string(),
                    name: name.to_string(),
                });
            }
            return Ok(Arc::clone(existing));
        }

        // An embedded engine, so this is a path and never a URL: there is no connection to make and
        // nothing to authenticate (D634 (b)). The engine is opened while the registry lock is held
        // so that two callers racing on one key cannot both create a store; opening a file is
        // bounded by the filesystem, and the alternative is two engines for one name.
        let in_memory = spec.database_path.as_os_str().is_empty();
        let db = match (&spec.database_path, spec.read_only) {
            // `open_read_only` is the engine's own read-only mode: it takes a shared lock and skips
            // WAL replay, so a read-only graph cannot be made to write even by a statement this
            // crate's guard mis-classifies. Grafeo requires an existing `.grafeo` file for it, so
            // an in-memory read-only graph falls through to the guarded path below.
            (path, true) if !path.as_os_str().is_empty() => {
                GrafeoDB::open_read_only(path).map_err(as_engine_error)?
            }
            (path, _) if path.as_os_str().is_empty() => GrafeoDB::new_in_memory(),
            (path, _) => GrafeoDB::open(path).map_err(as_engine_error)?,
        };

        let graph = Arc::new(Graph {
            namespace: namespace.to_string(),
            name: name.to_string(),
            db,
            path: (!in_memory).then(|| spec.database_path.clone()),
            read_only: spec.read_only,
            persistent: !in_memory,
            statements_executed: AtomicU64::new(0),
        });
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

    fn db_path(&self) -> &Path {
        // `GrafeoDB` does not expose its path, so it is kept alongside rather than read back. An
        // empty OpenSpec path means in-memory, which is what the comparison in `open` needs.
        self.path.as_deref().unwrap_or(Path::new(""))
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
        Self::check(statement, read_only || self.read_only)?;
        self.run(statement)
    }

    /// Runs one statement with `$name` parameters bound by the engine, as its own transaction.
    ///
    /// The statement reaches the engine unchanged and a value is never interpolated into it; the
    /// read-only guard is the same one [`Graph::execute`] applies.
    pub fn execute_with_params(
        &self,
        statement: &str,
        parameters: HashMap<String, Value>,
        read_only: bool,
    ) -> Result<GraphResult, GraphError> {
        Self::check(statement, read_only || self.read_only)?;
        let result = self
            .db
            .session()
            .execute_with_params(statement, parameters)
            .map_err(as_engine_error)?;
        self.statements_executed.fetch_add(1, Ordering::Relaxed);
        Ok(GraphResult::from(result))
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
        let mut session = self.db.session();
        session.begin_transaction().map_err(as_engine_error)?;
        let mut out = Vec::with_capacity(statements.len());
        for statement in statements {
            // Checked before the statement runs, so a read-only batch is refused whole rather than
            // half-applied: `Self::check` is the same guard `execute` uses.
            if let Err(err) = Self::check(&statement.text, self.read_only) {
                let _ = self.rollback(&mut session);
                return Err(err);
            }
            // `execute_with_params` takes the bindings by value, hence the copy. An empty map is the
            // same call as `execute`, so a statement without parameters takes no special path.
            let bound = statement.parameters.clone();
            match session.execute_with_params(&statement.text, bound) {
                Ok(result) => {
                    self.statements_executed.fetch_add(1, Ordering::Relaxed);
                    out.push(GraphResult::from(result));
                }
                Err(err) => {
                    // Nothing before the failure becomes durable, so a retry of the whole batch is
                    // safe; a partial batch would not be.
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

    /// The checks every statement path shares.
    fn check(statement: &str, read_only: bool) -> Result<(), GraphError> {
        if statement.trim().is_empty() {
            return Err(GraphError::EmptyStatement);
        }
        if read_only && writes(statement) {
            return Err(GraphError::ReadOnly);
        }
        Ok(())
    }

    /// Hands one statement to the engine and counts it.
    fn run(&self, statement: &str) -> Result<GraphResult, GraphError> {
        self.statements_executed.fetch_add(1, Ordering::Relaxed);
        let result = self.db.execute(statement).map_err(as_engine_error)?;
        Ok(GraphResult::from(result))
    }
}

/// The engine: the open graphs of one Loams process.
///
/// Deliberately not a global. `Engine` is held by the service and injected, so a test can stand up
/// two engines and be sure they share nothing.
#[derive(Debug)]
pub struct Engine {
    graphs: Mutex<HashMap<String, Arc<Graph>>>,
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
            standard: crate::GQL_STANDARD,
            engine_version: crate::ENGINE_VERSION,
        }
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

/// Whether a statement writes, judged by its bare words.
///
/// A keyword test rather than a parse, deliberately. The two failure modes are not symmetric: a read
/// refused as a write costs a caller one retry, while a write let through as a read corrupts a graph
/// under a promise that it would not. So both tests default to "writes".
///
/// Two tests, because one is not enough — measured against Grafeo 0.5.43, not assumed:
///
/// * **The leading keyword.** Anything that is not a reading keyword is a write, so a keyword this
///   crate has never heard of, a statement starting with punctuation, and a statement that is only a
///   comment are all writes.
/// * **Any writing word anywhere.** A GQL query expression writes from a *clause*, not only from its
///   first word: `MATCH (d:Doc) SET d.body = 'x'` begins with `MATCH` and is a write. A
///   first-keyword-only guard lets that through, so the guard also scans for a writing keyword
///   outside any string or comment.
///
/// The words come from [`bare_words`], which skips string literals, quoted identifiers and comments.
/// That is what makes a statement that merely *mentions* a writing keyword a read:
/// `MATCH (n) WHERE n.name = 'INSERT' RETURN n` has no `INSERT` word in it. The cost of skipping is
/// that an unquoted word which merely contains a keyword is refused — `RETURN n.insert_time` reads as
/// `INSERT` — which is the conservative direction the first paragraph is about.
///
/// This is not a parser and does not try to be: it reads a statement, never modifies it, and the
/// bytes the engine sees are the bytes the caller sent (D634's no-rewriting rule).
fn writes(statement: &str) -> bool {
    /// Leading keywords that cannot write. GQL's read shapes: a query expression with its optional
    /// `MATCH`, `FILTER`/`WHERE`, `RETURN`, `LET`/`FOR` and `ORDER BY`/`SKIP`/`LIMIT` clauses, plus
    /// `EXPLAIN` and `PROFILE`, which plan a statement without running it.
    const READS: [&str; 12] = [
        "MATCH", "RETURN", "FILTER", "WHERE", "FOR", "LET", "QUERY", "ORDER", "SKIP", "LIMIT",
        "EXPLAIN", "PROFILE",
    ];
    /// Words that write wherever they appear. `CALL` is here rather than in `READS` because a
    /// procedure can read or write, and a guard cannot tell which without running it.
    const WRITES: [&str; 12] = [
        "INSERT", "CREATE", "DELETE", "MERGE", "SET", "REMOVE", "DROP", "ALTER", "CALL", "LOAD",
        "UPSERT", "GRANT",
    ];
    let words = bare_words(statement);
    // No words at all means punctuation or a comment, which is not a statement anyone can read.
    let reads = words
        .first()
        .is_some_and(|first| READS.contains(&first.as_str()));
    !reads || words.iter().any(|word| WRITES.contains(&word.as_str()))
}

/// A statement's bare words: uppercased, with strings, quoted identifiers and comments removed.
///
/// Comments are dropped because a comment is not a statement: `/* sync */ INSERT ...` writes, and a
/// guard that read the comment as the first word would call it a read. Strings are dropped for the
/// other reason: a value is data, not syntax. Neither step changes a byte of the statement — they
/// only decide which side of the guard it falls on.
fn bare_words(statement: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut chars = statement.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // A quoted string, or a backquoted identifier, runs to its own closing quote.
            quote @ ('\'' | '"' | '`') => {
                push_word(&mut word, &mut words);
                for c in chars.by_ref() {
                    if c == quote {
                        break;
                    }
                }
            }
            // `--` and `//` run to the end of the line; `/* */` to its own terminator.
            '-' | '/' if matches!(chars.peek(), Some('-' | '/')) => {
                push_word(&mut word, &mut words);
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                push_word(&mut word, &mut words);
                chars.next();
                let mut previous = '\0';
                for c in chars.by_ref() {
                    if previous == '*' && c == '/' {
                        break;
                    }
                    previous = c;
                }
            }
            c if c.is_ascii_alphabetic() => word.push(c.to_ascii_uppercase()),
            _ => push_word(&mut word, &mut words),
        }
    }
    push_word(&mut word, &mut words);
    words
}

/// Completes the word being built, if there is one.
fn push_word(word: &mut String, words: &mut Vec<String>) {
    if !word.is_empty() {
        words.push(std::mem::take(word));
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
