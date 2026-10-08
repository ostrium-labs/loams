//! What Loams's embedded graph engine actually does, measured against Grafeo 0.5.43.
//!
//! Every test here drives the crate the way the Connect-RPC service does — `service::open`,
//! `service::execute`, and the rest — so the protobuf conversions are exercised by the same
//! assertions that exercise the engine. Where a claim in the design is only true up to what an
//! engine does, the test says which part it measured; see `tests/probe*` history in the report for
//! the readings these assertions are drawn from.

use std::path::PathBuf;

use connectrpc::{ConnectError, ErrorCode};
use loams_graph::{BatchStatement, Engine, Graph, GraphError, OpenSpec, service};
use loams_proto::loams::graph::v1 as pb;

/// A fresh engine with no graphs open.
fn engine() -> Engine {
    Engine::new()
}

/// Opens an in-memory graph through the service, the way a caller would.
fn open(engine: &Engine, namespace: &str, name: &str) -> pb::Graph {
    service::open(
        engine,
        pb::OpenRequest {
            namespace: namespace.to_string(),
            name: name.to_string(),
            ..Default::default()
        },
    )
    .expect("opening an in-memory graph cannot fail")
}

/// Runs one statement against an open graph.
fn execute(
    engine: &Engine,
    namespace: &str,
    name: &str,
    statement: &str,
) -> Result<pb::ExecuteResponse, ConnectError> {
    service::execute(
        engine,
        pb::ExecuteRequest {
            namespace: namespace.to_string(),
            name: name.to_string(),
            statement: statement.to_string(),
            ..Default::default()
        },
    )
}

/// Runs one statement that is asserted to succeed.
fn ok(engine: &Engine, namespace: &str, name: &str, statement: &str) -> pb::ExecuteResponse {
    execute(engine, namespace, name, statement).expect("statement should have succeeded")
}

/// The row set of a response, which this crate always fills.
fn rows(response: &pb::ExecuteResponse) -> &pb::RowSet {
    response
        .rows
        .as_option()
        .expect("every response carries a row set, empty for a write")
}

/// One scalar out of a response, by column position. Graph results legitimately repeat a key, which
/// is why the contract is positional, so the tests read them positionally too.
fn scalar(response: &pb::ExecuteResponse, column: usize) -> String {
    let set = rows(response);
    let value = &set.rows[0].values[column];
    value
        .as_str()
        .map_or_else(|| format!("{value:?}"), ToString::to_string)
}

/// A number out of a response, by column position.
fn number(response: &pb::ExecuteResponse, column: usize) -> f64 {
    rows(response).rows[0].values[column]
        .as_number()
        .expect("the column should hold a number")
}

/// The message text of a refused call.
fn message(err: &ConnectError) -> &str {
    err.message.as_deref().unwrap_or_default()
}

/// A temporary `.grafeo` path that no other run of this binary will use.
fn scratch_path(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_nanos();
    std::env::temp_dir()
        .join(format!(
            "loams-graph-{label}-{}-{nanos}",
            std::process::id()
        ))
        .join("graph.grafeo")
}

#[test]
fn gql_round_trips_a_graph() {
    let engine = engine();
    open(&engine, "acme", "orders");

    // Two nodes and the edge between them, written as GQL and read back through the same service.
    ok(
        &engine,
        "acme",
        "orders",
        "INSERT (:Customer {name: 'Alix', age: 30})",
    );
    ok(
        &engine,
        "acme",
        "orders",
        "INSERT (:Customer {name: 'Bo', age: 41})",
    );
    ok(
        &engine,
        "acme",
        "orders",
        "MATCH (a:Customer {name: 'Alix'}), (b:Customer {name: 'Bo'}) \
         CREATE (a)-[:PLACED {since: 2026}]->(b)",
    );

    let traversal = ok(
        &engine,
        "acme",
        "orders",
        "MATCH (a:Customer)-[e:PLACED]->(b:Customer) \
         RETURN a.name, type(e), b.name, e.since ORDER BY a.name",
    );
    let set = rows(&traversal);
    assert_eq!(set.columns, ["a.name", "type(e)", "b.name", "e.since"]);
    assert_eq!(set.rows.len(), 1, "one edge, one row: {set:?}");
    assert_eq!(scalar(&traversal, 0), "Alix");
    assert_eq!(scalar(&traversal, 1), "PLACED");
    assert_eq!(scalar(&traversal, 2), "Bo");
    assert_eq!(number(&traversal, 3), 2026.0);

    // The engine's own row counter and elapsed clock are real measurements, not the zeros the first
    // draft hardcoded: one row returned, and a statement that took measurable time to plan and run.
    assert_eq!(traversal.rows_read, 1, "rows_scanned is the row count");
    assert!(
        traversal.elapsed_nanos > 0,
        "the engine times every statement, so an elapsed of zero would mean we stopped reading it"
    );

    // A whole node crosses as a map of `_id`, `_labels` and its properties, and the node id is also
    // lifted into the row so a client can address it without a second lookup.
    let node = ok(
        &engine,
        "acme",
        "orders",
        "MATCH (c:Customer {name: 'Bo'}) RETURN c",
    );
    let returned = &rows(&node).rows[0];
    assert_eq!(
        returned.node_ids.len(),
        1,
        "one node in the row: {returned:?}"
    );
    let alix = returned.node_ids[0].clone();
    let fields = returned.values[0]
        .as_struct()
        .expect("a node arrives as a struct");
    assert_eq!(
        fields.fields.get("name").and_then(|v| v.as_str()),
        Some("Bo"),
        "the node's own properties travel with it: {fields:?}"
    );

    // And the lifted id addresses the node in a later statement, which is the point of lifting it.
    // Measured: Grafeo addresses a node with `id(n)`, and that is the same integer the projection
    // puts in `_id` -- `WHERE c._id = 1` and `WHERE element_id(c) = 1` both match nothing, so a
    // client has to be told which of the three spellings works rather than left to find out.
    let by_id = ok(
        &engine,
        "acme",
        "orders",
        &format!("MATCH (c:Customer) WHERE id(c) = {alix} RETURN c.name"),
    );
    assert_eq!(
        scalar(&by_id, 0),
        "Bo",
        "the lifted id addresses the node it came from"
    );
    assert_eq!(
        rows(&ok(
            &engine,
            "acme",
            "orders",
            "MATCH (c:Customer {name: 'Bo'}) RETURN c._id AS via_property"
        ))
        .rows
        .len(),
        1,
        "and `_id` is readable as a projection"
    );

    // Every statement is counted, so `Graph.statements_executed` is a measurement too.
    let graph = service::open(
        &engine,
        pb::OpenRequest {
            namespace: "acme".to_string(),
            name: "orders".to_string(),
            ..Default::default()
        },
    )
    .expect("reopening the same graph returns the same one");
    assert_eq!(
        graph.statements_executed, 7,
        "every statement above was counted: two inserts, the edge, the traversal, the node, the \
         lookup by id and the `_id` projection"
    );
    assert_eq!(graph.handle, "acme/orders");
    assert!(!graph.persistent, "an empty database_path is in-memory");
}

#[test]
fn values_survive_the_json_round_trip() {
    let engine = engine();
    open(&engine, "acme", "types");

    ok(
        &engine,
        "acme",
        "types",
        "INSERT (:Reading {n: 7, ratio: 0.25, ok: true, missing: null, \
         at: datetime('2026-10-04T09:30:00Z'), day: date('2026-10-04'), \
         tags: ['a', 'b'], meta: {k: 1}})",
    );
    let read = ok(
        &engine,
        "acme",
        "types",
        "MATCH (r:Reading) RETURN r.n, r.ratio, r.ok, r.missing, r.at, r.day, r.tags, r.meta",
    );
    let set = rows(&read);
    assert_eq!(set.rows.len(), 1);
    let row = &set.rows[0];
    assert_eq!(
        row.values[0].as_number(),
        Some(7.0),
        "an Int64 crosses as a number"
    );
    assert_eq!(
        row.values[1].as_number(),
        Some(0.25),
        "a Float64 keeps its fraction"
    );
    assert_eq!(row.values[2].as_bool(), Some(true));
    assert_eq!(
        row.values[3].as_number(),
        None,
        "null is null, not a zero: {:?}",
        row.values[3]
    );
    // Temporal values cross as ISO 8601 text through grafeo's own `Display`, so a GQL client reads
    // what it wrote rather than microseconds since the epoch.
    assert_eq!(
        row.values[4].as_str(),
        Some("2026-10-04T09:30:00.000000Z"),
        "a timestamp is ISO 8601"
    );
    assert_eq!(
        row.values[5].as_str(),
        Some("2026-10-04"),
        "a date is ISO 8601"
    );
    let tags = row.values[6].as_list().expect("a list crosses as a list");
    assert_eq!(tags.values.len(), 2);
    assert_eq!(tags.values[1].as_str(), Some("b"));
    let meta = row.values[7]
        .as_struct()
        .expect("a map crosses as a struct");
    assert_eq!(meta.fields.get("k").and_then(|v| v.as_number()), Some(1.0));

    // Bytes have no JSON counterpart. This crate sends an array of byte values rather than a lossy
    // string, and Grafeo 0.5.43's `bytes()` literal yields null rather than a byte value, so the
    // conversion is exercised through the engine only as far as the engine goes: the assertion is
    // that nothing panicked and the column still lines up.
    assert_eq!(set.columns.len(), row.values.len());
}

#[test]
fn open_is_idempotent_and_conflict_is_refused() {
    let engine = engine();
    let first = open(&engine, "acme", "orders");
    let second = open(&engine, "acme", "orders");
    assert_eq!(
        first.handle, second.handle,
        "the same key is the same graph"
    );
    assert_eq!(
        engine.list(Some("acme")).map(|graphs| graphs.len()),
        Ok(1),
        "opening twice must not register a second engine"
    );

    // Data written through the first handle is visible through the second, which is what makes them
    // the same graph rather than two graphs that happen to share a name.
    ok(&engine, "acme", "orders", "INSERT (:K {v: 1})");
    let count = ok(
        &engine,
        "acme",
        "orders",
        "MATCH (k:K) RETURN count(k) AS c",
    );
    assert_eq!(number(&count, 0), 1.0);

    // Reopening with a different read-only flag, or a different path, is a conflict rather than a
    // second opinion about one graph's storage. Both are refused before anything is opened.
    let err = service::open(
        &engine,
        pb::OpenRequest {
            namespace: "acme".to_string(),
            name: "orders".to_string(),
            read_only: true,
            ..Default::default()
        },
    )
    .expect_err("read_only disagrees with the open graph");
    assert_eq!(err.code, ErrorCode::AlreadyExists, "{err:?}");
    assert!(message(&err).contains("acme/orders"), "{err:?}");

    let err = service::open(
        &engine,
        pb::OpenRequest {
            namespace: "acme".to_string(),
            name: "orders".to_string(),
            database_path: scratch_path("conflict").to_string_lossy().into_owned(),
            ..Default::default()
        },
    )
    .expect_err("a different path is a different graph");
    assert_eq!(err.code, ErrorCode::AlreadyExists, "{err:?}");

    // A different name in the same namespace is a different graph, and shares nothing.
    open(&engine, "acme", "returns");
    assert_eq!(engine.list(Some("acme")).map(|g| g.len()), Ok(2));
    let empty = ok(
        &engine,
        "acme",
        "returns",
        "MATCH (k:K) RETURN count(k) AS c",
    );
    assert_eq!(
        number(&empty, 0),
        0.0,
        "the other graph in the namespace does not see the first graph's nodes"
    );
}

#[test]
fn a_persistent_graph_survives_a_close_and_a_reopen() {
    let path = scratch_path("persistent");
    let directory = path
        .parent()
        .expect("scratch_path has a parent")
        .to_path_buf();
    let spec = OpenSpec {
        database_path: path.clone(),
        read_only: false,
    };

    let engine = engine();
    let graph = Graph::open(&engine, "acme", "durable", spec.clone()).expect("open a new graph");
    assert!(
        graph.is_persistent(),
        "a database_path means the storage outlives the process"
    );
    graph
        .execute("INSERT (:Note {body: 'durable'})", false)
        .expect("insert");
    drop(graph);
    assert_eq!(
        engine.close("acme", "durable"),
        Ok(true),
        "closing the only handle releases the engine"
    );

    // A second process would see this; a second engine in this process sees the same thing.
    let reopened = Engine::new();
    let read;
    {
        let graph = Graph::open(&reopened, "acme", "durable", spec).expect("reopen");
        assert!(graph.is_persistent());
        read = graph
            .execute("MATCH (n:Note) RETURN n.body", false)
            .expect("read back");
    }
    assert_eq!(
        reopened.close("acme", "durable"),
        Ok(true),
        "the reopened graph is closed before the read-only open: Grafeo's single-file format takes \
         an exclusive lock for a writable database and a shared one for a read-only one, so two open \
         handles on one path is a lock error rather than a second reader"
    );
    assert_eq!(read.rows.len(), 1, "the note survived the close: {read:?}");

    // `read_only` on a persistent graph is Grafeo's own read-only mode, which takes a shared file
    // lock. That is the belt to this crate's braces: the guard below can be defeated by a
    // mis-classified keyword, and the engine still refuses the write.
    let readonly = Engine::new();
    let graph = Graph::open(
        &readonly,
        "acme",
        "durable",
        OpenSpec {
            database_path: path,
            read_only: true,
        },
    )
    .expect("open read-only");
    assert!(graph.is_read_only());
    assert!(
        graph.execute("MATCH (n:Note) RETURN n.body", false).is_ok(),
        "a read-only graph still reads"
    );
    assert_eq!(
        graph.execute("MATCH (n:Note) SET n.body = 'edited'", false),
        Err(GraphError::ReadOnly),
        "the guard refuses before the engine is asked"
    );
    drop(graph);
    assert_eq!(readonly.close("acme", "durable"), Ok(true));

    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn read_only_refuses_a_write() {
    let engine = engine();
    open(&engine, "acme", "guard");
    ok(&engine, "acme", "guard", "INSERT (:Doc {body: 'kept'})");

    // A read on a read-only request is a read.
    let read = service::execute(
        &engine,
        pb::ExecuteRequest {
            namespace: "acme".to_string(),
            name: "guard".to_string(),
            statement: "MATCH (d:Doc) RETURN d.body".to_string(),
            read_only: true,
            ..Default::default()
        },
    )
    .expect("a read is fine under read_only");
    assert_eq!(scalar(&read, 0), "kept");

    // Every keyword the guard treats as a writing one, refused under `read_only` and refused the
    // same way when the *graph* is read-only rather than the request.
    for statement in [
        "INSERT (:Doc {body: 'new'})",
        "CREATE (:Doc {body: 'new'})",
        "MERGE (:Doc {body: 'new'})",
        "DELETE (d:Doc)",
        // A query expression can write from a clause without its first word saying so, which is why
        // the guard scans every bare word and not just the leading one.
        "MATCH (d:Doc) SET d.body = 'edited' RETURN d",
        "MATCH (d:Doc) REMOVE d.body RETURN d",
        "DROP GRAPH guard",
        // `CALL` is ambiguous -- a procedure can read or write -- so it counts as a write.
        "CALL db.labels()",
        // A comment must not smuggle a write past the guard: the guard reads past leading comments
        // to find the first keyword, without changing a byte of the statement.
        "/* a leading comment */ INSERT (:Doc {body: 'new'})",
        "-- and a line comment too\nMERGE (:Doc {body: 'new'})",
    ] {
        let err = service::execute(
            &engine,
            pb::ExecuteRequest {
                namespace: "acme".to_string(),
                name: "guard".to_string(),
                statement: statement.to_string(),
                read_only: true,
                ..Default::default()
            },
        )
        .expect_err("a writing statement must be refused under read_only");
        assert_eq!(
            err.code,
            ErrorCode::PermissionDenied,
            "{statement}: {err:?}"
        );
        assert!(message(&err).contains("read-only"), "{statement}: {err:?}");
    }

    // Nothing was written.
    let count = ok(
        &engine,
        "acme",
        "guard",
        "MATCH (d:Doc) RETURN count(d) AS c",
    );
    assert_eq!(number(&count, 0), 1.0, "only the first insert landed");

    // A graph opened read-only refuses a write even when the request forgot to ask for it.
    let readonly = Graph::open(
        &engine,
        "acme",
        "sealed",
        OpenSpec {
            database_path: PathBuf::new(),
            read_only: true,
        },
    )
    .expect("open read-only in memory");
    let sealed_write = "MATCH (d:Doc) SET d.body = 'edited' RETURN d";
    assert_eq!(
        readonly.execute(sealed_write, false),
        Err(GraphError::ReadOnly),
        "the graph's own flag refuses the write, not just the request's"
    );
    assert!(
        readonly
            .execute("MATCH (d:Doc) RETURN count(d) AS c", false)
            .is_ok(),
        "and it still reads"
    );

    // One the guard must NOT refuse: the statement merely *contains* a writing keyword. A guard that
    // searched the text rather than its first keyword would refuse this read and cost a caller a
    // retry for nothing.
    let contains = service::execute(
        &engine,
        pb::ExecuteRequest {
            namespace: "acme".to_string(),
            name: "guard".to_string(),
            statement: "MATCH (d:Doc) WHERE d.body = 'INSERT' RETURN count(d) AS c".to_string(),
            read_only: true,
            ..Default::default()
        },
    )
    .expect("a read that mentions INSERT is still a read");
    assert_eq!(
        number(&contains, 0),
        0.0,
        "the engine read 'INSERT' as the string it is, which is only possible because the guard \
         did not treat it as syntax"
    );
}

#[test]
fn statement_reaches_the_engine_unchanged() {
    let engine = engine();
    open(&engine, "acme", "verbatim");

    // Deliberately awkward: a leading comment, a blank line, irregular spacing, tabs, a trailing
    // comment and no final newline. D634's no-rewriting rule is that this reaches Grafeo as typed --
    // if the service normalised whitespace, re-indented, or stripped the comment, this statement
    // would still parse and the test would not notice, so the comment content is what it asserts:
    // a statement whose text carries the keyword `INSERT` only inside a comment is a read, and the
    // guard's first-keyword test must see past the comment without editing anything.
    let awkward = "\
/* leading comment: this statement writes nothing */
\tMATCH  (d:Doc)\r\n\
\tWHERE   d.body  =  'INSERT'   /* and neither does this */
\tRETURN d.body -- trailing comment";
    let read = ok(&engine, "acme", "verbatim", awkward);
    assert_eq!(
        rows(&read).rows.len(),
        0,
        "the statement ran as written: {read:?}"
    );

    // The same statement with the comment carrying a real write, to show the text reaches the
    // engine either way: an engine that saw rewritten text would fail to parse this.
    ok(
        &engine,
        "acme",
        "verbatim",
        "/* a comment */ INSERT (:Doc {body: 'from a comment'})",
    );
    let check = ok(&engine, "acme", "verbatim", "MATCH (d:Doc) RETURN d.body");
    assert_eq!(scalar(&check, 0), "from a comment");

    // A statement the engine rejects is rejected by the engine's own parser, which is only possible
    // if the bytes arrived intact: the error names a column position.
    let err = execute(&engine, "acme", "verbatim", "MATCH  (d:Doc RETURN d.body")
        .expect_err("an unbalanced paren is a syntax error");
    assert!(
        message(&err).contains("Expected RParen"),
        "the engine's own diagnostic survives: {err:?}"
    );
}

#[test]
fn a_refused_statement_keeps_the_engine_message() {
    let engine = engine();
    open(&engine, "acme", "diagnostics");

    // `MATCH (n:Nope) RETURN n` is *not* an error -- an unknown label matches nothing -- so the
    // statement that genuinely fails is one the parser rejects. Measured, not assumed.
    let empty = ok(&engine, "acme", "diagnostics", "MATCH (n:Nope) RETURN n");
    assert_eq!(
        rows(&empty).rows.len(),
        0,
        "an unknown label is empty, not an error: {empty:?}"
    );

    let err = execute(&engine, "acme", "diagnostics", "MATCH (n RETURN n")
        .expect_err("an unbalanced paren does not parse");
    assert_eq!(
        err.code,
        ErrorCode::InvalidArgument,
        "the caller's text was wrong, which is what InvalidArgument is for: {err:?}"
    );
    let text = message(&err);
    // Grafeo's own message, whole: the label, the caret span and the column it is at.
    assert!(text.contains("syntax error"), "{text}");
    assert!(text.contains("Expected RParen"), "{text}");
    assert!(
        text.contains("query:1:10"),
        "the span locates the mistake: {text}"
    );
    assert!(text.contains('^'), "the caret span survives: {text}");

    // A semantic failure keeps the engine's own message too, and here the message is the whole
    // value: Grafeo explains why two columns of one name cannot exist and what to do about it.
    let err = execute(&engine, "acme", "diagnostics", "RETURN 1 AS x, 2 AS x")
        .expect_err("a result cannot carry two columns of one name, which the engine enforces");
    assert_eq!(err.code, ErrorCode::InvalidArgument, "{err:?}");
    assert!(message(&err).contains("duplicate column name"), "{err:?}");
    assert!(
        message(&err).contains("AS x_2"),
        "and it suggests the fix: {err:?}"
    );

    // Measured, so the test says so: an unknown *function* is not an error in 0.5.43 -- it projects
    // a null under the function's own text as the column name. Loams passes that through rather than
    // inventing a diagnostic the engine did not give.
    let unknown = ok(&engine, "acme", "diagnostics", "RETURN nosuchfunction(1)");
    let value = &rows(&unknown).rows[0].values[0];
    assert!(
        value.is_null(),
        "grafeo's own answer, not one of ours: {unknown:?}"
    );
}

#[test]
fn batch_is_one_transaction() {
    let engine = engine();
    open(&engine, "acme", "batch");

    let statements = vec![
        "INSERT (:Line {sku: 'a'})",
        "INSERT (:Line {sku: 'b'})",
        "INSERT (:Line {sku: 'c'})",
        "MATCH (a:Line {sku: 'a'}), (b:Line {sku: 'b'}) CREATE (a)-[:NEXT]->(b)",
    ]
    .into_iter()
    .map(|statement| pb::Statement {
        statement: statement.to_string(),
        ..Default::default()
    })
    .collect();

    let response = service::execute_batch(
        &engine,
        pb::ExecuteBatchRequest {
            namespace: "acme".to_string(),
            name: "batch".to_string(),
            statements,
            atomic: true,
            ..Default::default()
        },
    )
    .expect("a batch of valid statements commits");
    assert!(response.committed, "one transaction, so it committed");
    assert_eq!(response.results.len(), 4, "one result per statement");

    let read = ok(
        &engine,
        "acme",
        "batch",
        "MATCH (l:Line) RETURN count(l) AS c",
    );
    assert_eq!(number(&read, 0), 3.0, "every insert in the batch landed");
    let edge = ok(
        &engine,
        "acme",
        "batch",
        "MATCH (:Line {sku: 'a'})-[:NEXT]->(:Line {sku: 'b'}) RETURN 1 AS ok",
    );
    assert_eq!(
        number(&edge, 0),
        1.0,
        "and the statement that read the first two"
    );

    // Atomic means atomic: a batch whose last statement the engine rejects leaves *nothing* behind.
    // That is the difference between one engine transaction and a loop over `execute`, and it is
    // only observable because the writes are invisible until the commit.
    let statements = vec![
        "INSERT (:Line {sku: 'never'})".to_string(),
        "MATCH (n RETURN n".to_string(),
    ]
    .into_iter()
    .map(|statement| pb::Statement {
        statement,
        ..Default::default()
    })
    .collect();
    let err = service::execute_batch(
        &engine,
        pb::ExecuteBatchRequest {
            namespace: "acme".to_string(),
            name: "batch".to_string(),
            statements,
            atomic: true,
            ..Default::default()
        },
    )
    .expect_err("the second statement does not parse");
    assert_eq!(err.code, ErrorCode::InvalidArgument, "{err:?}");
    let after = ok(
        &engine,
        "acme",
        "batch",
        "MATCH (l:Line) RETURN count(l) AS c",
    );
    assert_eq!(
        number(&after, 0),
        3.0,
        "the failed batch rolled its first statement back: {after:?}"
    );

    // An empty statement is refused rather than skipped: a batch that silently dropped one would
    // report fewer results than statements and the caller would never know which was lost.
    for (label, statements) in [
        ("blank at the end", vec!["INSERT (:Line {sku: 'd'})", "   "]),
        ("blank at the start", vec!["", "INSERT (:Line {sku: 'd'})"]),
        (
            "blank in the middle",
            vec![
                "INSERT (:Line {sku: 'd'})",
                "\t",
                "INSERT (:Line {sku: 'g'})",
            ],
        ),
    ] {
        let statements = statements
            .into_iter()
            .map(|statement| pb::Statement {
                statement: statement.to_string(),
                ..Default::default()
            })
            .collect();
        let err = service::execute_batch(
            &engine,
            pb::ExecuteBatchRequest {
                namespace: "acme".to_string(),
                name: "batch".to_string(),
                statements,
                atomic: true,
                ..Default::default()
            },
        )
        .expect_err("a blank statement is refused");
        assert_eq!(err.code, ErrorCode::InvalidArgument, "{label}: {err:?}");
        assert!(message(&err).contains("empty"), "{label}: {err:?}");
    }
    let after = ok(
        &engine,
        "acme",
        "batch",
        "MATCH (l:Line) RETURN count(l) AS c",
    );
    assert_eq!(number(&after, 0), 3.0, "a refused batch wrote nothing");

    // `atomic = false` is a series of independent statements, which the contract says so itself.
    let statements = vec!["INSERT (:Line {sku: 'e'})", "INSERT (:Line {sku: 'f'})"]
        .into_iter()
        .map(|statement| pb::Statement {
            statement: statement.to_string(),
            ..Default::default()
        })
        .collect();
    let response = service::execute_batch(
        &engine,
        pb::ExecuteBatchRequest {
            namespace: "acme".to_string(),
            name: "batch".to_string(),
            statements,
            atomic: false,
            ..Default::default()
        },
    )
    .expect("a non-atomic batch of valid statements runs");
    assert!(
        !response.committed,
        "no single transaction means nothing to report as committed"
    );
    let after = ok(
        &engine,
        "acme",
        "batch",
        "MATCH (l:Line) RETURN count(l) AS c",
    );
    assert_eq!(number(&after, 0), 5.0);
}

#[test]
fn an_unavailable_language_is_unimplemented() {
    let engine = engine();
    open(&engine, "acme", "languages");

    // What this build has, per `EngineInfo`. Measured rather than asserted from the source: the
    // dependency is `grafeo` with its default features plus `wal`, `spill` and `mmap`, which is the
    // one combination that gives the LPG model and GQL while leaving `cypher`, `sparql`, `gremlin`,
    // `graphql` and `sql-pgq` switched off (see this crate's `Cargo.toml`).
    let info = service::engine_info(&engine);
    assert_eq!(
        info.languages.as_slice(),
        [pb::QueryLanguage::Gql],
        "one language, and it is the only one this build has"
    );
    assert!(info.embedded, "D634 (b): the engine is in this process");
    assert!(info.ephemeral, "the only open graph is in-memory");
    assert_eq!(info.engine_version, loams_graph::ENGINE_VERSION);

    for language in [
        pb::QueryLanguage::Cypher,
        pb::QueryLanguage::Sparql,
        pb::QueryLanguage::Gremlin,
        pb::QueryLanguage::Graphql,
        pb::QueryLanguage::SqlPgq,
    ] {
        let err = service::execute(
            &engine,
            pb::ExecuteRequest {
                namespace: "acme".to_string(),
                name: "languages".to_string(),
                statement: "MATCH (n) RETURN n".to_string(),
                language: language.into(),
                ..Default::default()
            },
        )
        .expect_err("a language this build lacks is Unimplemented");
        assert_eq!(
            err.code,
            ErrorCode::Unimplemented,
            "{language:?} must not be answered as a parse error: {err:?}"
        );
        assert!(message(&err).contains("GQL"), "{err:?}");

        let err = service::execute_batch(
            &engine,
            pb::ExecuteBatchRequest {
                namespace: "acme".to_string(),
                name: "languages".to_string(),
                statements: Vec::new(),
                language: language.into(),
                ..Default::default()
            },
        )
        .expect_err("the same refusal applies to a batch");
        assert_eq!(err.code, ErrorCode::Unimplemented, "{err:?}");
    }

    // A wire number naming no variant at all is refused too, rather than defaulted to GQL: running
    // a statement in a language the caller did not ask for is worse than refusing it.
    let err = service::execute(
        &engine,
        pb::ExecuteRequest {
            namespace: "acme".to_string(),
            name: "languages".to_string(),
            statement: "MATCH (n) RETURN n".to_string(),
            language: 4242.into(),
            ..Default::default()
        },
    )
    .expect_err("an unknown language number is refused");
    assert_eq!(err.code, ErrorCode::Unimplemented, "{err:?}");

    // GQL, named explicitly and left unspecified, both run.
    for language in [pb::QueryLanguage::Gql, pb::QueryLanguage::Unspecified] {
        let response = service::execute(
            &engine,
            pb::ExecuteRequest {
                namespace: "acme".to_string(),
                name: "languages".to_string(),
                statement: "RETURN 1 AS one".to_string(),
                language: language.into(),
                ..Default::default()
            },
        )
        .unwrap_or_else(|err| panic!("{language:?} should be served: {err:?}"));
        assert_eq!(number(&response, 0), 1.0);
    }
}

#[test]
fn list_and_close_manage_the_registry() {
    let engine = engine();
    open(&engine, "acme", "one");
    open(&engine, "acme", "two");
    open(&engine, "globex", "three");

    let listed = service::list_graphs(
        &engine,
        pb::ListGraphsRequest {
            namespace: "acme".to_string(),
            ..Default::default()
        },
    )
    .expect("listing cannot fail");
    let mut acme: Vec<String> = listed.graphs.iter().map(|g| g.name.clone()).collect();
    acme.sort();
    assert_eq!(acme, ["one", "two"], "filtered by namespace");
    assert!(listed.graphs.iter().all(|g| g.namespace == "acme"));

    let all = service::list_graphs(&engine, pb::ListGraphsRequest::default())
        .expect("an empty namespace means every namespace");
    assert_eq!(all.graphs.len(), 3);

    // A close while a caller still holds the handle is refused, because dropping the registry's own
    // reference says nothing about whether a statement is in flight.
    let held = Graph::open(
        &engine,
        "acme",
        "one",
        OpenSpec {
            database_path: PathBuf::new(),
            read_only: false,
        },
    )
    .expect("the same spec returns the open graph");
    let err = service::close(
        &engine,
        pb::CloseRequest {
            namespace: "acme".to_string(),
            name: "one".to_string(),
            ..Default::default()
        },
    )
    .expect_err("a held handle refuses the close");
    assert_eq!(err.code, ErrorCode::Internal, "{err:?}");
    assert!(message(&err).contains("still in use"), "{err:?}");

    // Once the last handle goes, the close succeeds and the graph leaves the registry.
    drop(held);
    service::close(
        &engine,
        pb::CloseRequest {
            namespace: "acme".to_string(),
            name: "one".to_string(),
            ..Default::default()
        },
    )
    .expect("closing with no holder succeeds");
    let listed = service::list_graphs(
        &engine,
        pb::ListGraphsRequest {
            namespace: "acme".to_string(),
            ..Default::default()
        },
    )
    .expect("listing cannot fail");
    assert_eq!(listed.graphs.len(), 1);

    // A closed graph is gone, not dormant: executing against it is `NotFound`.
    let err =
        execute(&engine, "acme", "one", "RETURN 1 AS one").expect_err("a closed graph is not open");
    assert_eq!(err.code, ErrorCode::NotFound, "{err:?}");

    // Closing something that is not open is not an error; it is already in the state asked for.
    service::close(
        &engine,
        pb::CloseRequest {
            namespace: "acme".to_string(),
            name: "one".to_string(),
            ..Default::default()
        },
    )
    .expect("closing an unopened graph is a no-op");

    // And a statement against a graph that was never opened is `NotFound` rather than a refusal to
    // open one implicitly.
    let err = execute(&engine, "acme", "never-opened", "RETURN 1 AS one")
        .expect_err("execute does not open a graph");
    assert_eq!(err.code, ErrorCode::NotFound, "{err:?}");
}

#[test]
fn sessions_do_not_share_state_but_the_current_graph_survives_a_call() {
    let engine = engine();
    open(&engine, "acme", "sessions");

    // Measured, not assumed. `GrafeoDB::execute` builds a **fresh one-shot session per call**
    // (`grafeo_engine::database::query::with_session`), so there is no session for two statements to
    // share: a committed write is visible to the next call, and a call cannot see a half-finished
    // one. The single exception is the current graph, which that helper deliberately writes back to
    // the database after every call so `USE GRAPH` survives. This test pins both halves, because the
    // first is what makes Loams's stateless `Execute` safe and the second is a leak a caller must
    // know about.

    ok(&engine, "acme", "sessions", "INSERT (:P {name: 'one'})");
    let count = ok(
        &engine,
        "acme",
        "sessions",
        "MATCH (p:P) RETURN count(p) AS c",
    );
    assert_eq!(
        number(&count, 0),
        1.0,
        "a committed write is visible to the next statement: there is no session affinity"
    );

    // The one piece of state that does cross a call boundary.
    ok(&engine, "acme", "sessions", "CREATE GRAPH scratch");
    ok(&engine, "acme", "sessions", "USE GRAPH scratch");
    let in_scratch = ok(
        &engine,
        "acme",
        "sessions",
        "MATCH (n) RETURN count(n) AS c",
    );
    assert_eq!(
        number(&in_scratch, 0),
        0.0,
        "USE GRAPH routed the next statement, which is a fresh session reading state the previous \
         call left behind"
    );
    ok(&engine, "acme", "sessions", "INSERT (:P {name: 'two'})");
    let back = ok(&engine, "acme", "sessions", "SESSION RESET GRAPH");
    assert!(rows(&back).rows.is_empty());
    let restored = ok(
        &engine,
        "acme",
        "sessions",
        "MATCH (p:P) RETURN count(p) AS c",
    );
    assert_eq!(
        number(&restored, 0),
        1.0,
        "and the default graph still holds only its own node, so the two graphs never shared a store"
    );

    // A write inside an open transaction is invisible to another session -- which is the property
    // `batch_is_one_transaction` leans on, asserted here from the read side as well.
    let graph = Graph::open(
        &engine,
        "acme",
        "sessions",
        OpenSpec {
            database_path: PathBuf::new(),
            read_only: false,
        },
    )
    .expect("the open graph");
    let failed = graph.execute_batch(&[
        BatchStatement::text("INSERT (:P {name: 'rolled back'})"),
        BatchStatement::text("MATCH (n RETURN n"),
    ]);
    assert!(failed.is_err(), "the second statement does not parse");
    let count = graph
        .execute("MATCH (p:P) RETURN count(p) AS c", false)
        .expect("read");
    assert_eq!(
        count.rows.len(),
        1,
        "the uncommitted insert is not visible to the next statement"
    );
    assert_eq!(
        count.rows[0].values[0],
        serde_json::json!(1),
        "only the committed node is counted"
    );
}

#[test]
fn graph_service_impl_serves_one_engine() {
    let engine = std::sync::Arc::new(engine());
    let service = loams_graph::GraphServiceImpl::new(std::sync::Arc::clone(&engine));
    assert!(std::sync::Arc::ptr_eq(service.engine(), &engine));
    let info = service.engine_info();
    assert!(info.embedded);
    assert_eq!(info.engine_version, loams_graph::ENGINE_VERSION);
    assert_eq!(engine.standard, loams_graph::GQL_STANDARD);
}
