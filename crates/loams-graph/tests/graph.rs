//! What Loams's embedded graph engine actually does, measured against Grafeo 0.5.43.
//!
//! Every test here drives the crate the way the Connect-RPC service does — `service::create_graph`,
//! `service::execute`, and the rest — so the protobuf conversions are exercised by the same
//! assertions that exercise the engine. Where a claim in the design is only true up to what an
//! engine does, the test says which part it measured; see `tests/probe*` history in the report for
//! the readings these assertions are drawn from.

use std::path::PathBuf;

use connectrpc::{ConnectError, ErrorCode};
use loams_graph::{BatchStatement, Engine, Graph, GraphError, OpenSpec, service};
use loams_proto::loams::graph::v1 as pb;
use pb::__buffa::oneof::value::Kind;

/// A fresh engine with no graphs open.
fn engine() -> Engine {
    Engine::new()
}

/// Opens an in-memory graph through the service, the way a caller would.
fn open(engine: &Engine, namespace: &str, name: &str) -> pb::Graph {
    service::create_graph(
        engine,
        pb::CreateGraphRequest {
            namespace: namespace.to_string(),
            name: name.to_string(),
            ..Default::default()
        },
    )
    .expect("creating an in-memory graph cannot fail")
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
            graph: name.to_string(),
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
    match &set.rows[0].values[column].kind {
        Some(Kind::String(text)) => text.clone(),
        other => format!("{other:?}"),
    }
}

/// An integer out of a response, by column position. Exact: `INT64` crosses as `int64` since
/// GR1 Task 2, not as a double.
fn number(response: &pb::ExecuteResponse, column: usize) -> i64 {
    match &rows(response).rows[0].values[column].kind {
        Some(Kind::Int64(n)) => *n,
        other => panic!("the column should hold an INT64, not {other:?}"),
    }
}

/// The message text of a refused call.
fn message(err: &ConnectError) -> &str {
    err.message.as_deref().unwrap_or_default()
}

/// A temporary data directory that no other run of this binary will use.
fn scratch_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "loams-graph-{label}-{}-{nanos}",
        std::process::id()
    ))
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
    assert_eq!(number(&traversal, 3), 2026);

    // The engine's elapsed clock is a real measurement. (`rows_read` and `bytes_read` left the
    // contract in GR1 Task 2: the engine counts neither, §48 §8.2.)
    assert!(
        traversal.elapsed_nanos > 0,
        "the engine times every statement, so an elapsed of zero would mean we stopped reading it"
    );

    // A whole node crosses as a typed `Node`: its id, its labels and its properties.
    let node = ok(
        &engine,
        "acme",
        "orders",
        "MATCH (c:Customer {name: 'Bo'}) RETURN c",
    );
    let returned = &rows(&node).rows[0];
    let Some(Kind::Node(bo)) = &returned.values[0].kind else {
        panic!("a node arrives as a Node: {returned:?}");
    };
    assert_eq!(bo.labels, ["Customer"]);
    assert!(
        matches!(bo.properties.get("name").and_then(|v| v.kind.as_ref()), Some(Kind::String(s)) if s == "Bo"),
        "the node's own properties travel with it: {bo:?}"
    );
    let alix = bo.id;

    // And the id addresses the node in a later statement, which is the point of lifting it.
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
        "the id addresses the node it came from"
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

    // Every statement is counted, so `statements_executed` is a measurement too.
    let graph = Graph::open(&engine, "acme", "orders", OpenSpec::default())
        .expect("reopening the same graph returns the same one");
    assert_eq!(
        graph.statements_executed(),
        7,
        "every statement above was counted: two inserts, the edge, the traversal, the node, the \
         lookup by id and the `_id` projection"
    );
    assert!(
        !graph.is_persistent(),
        "a graph created over the service is in memory"
    );
}

#[test]
fn values_cross_typed() {
    let engine = engine();
    open(&engine, "acme", "types");

    ok(
        &engine,
        "acme",
        "types",
        "INSERT (:Reading {n: 9007199254740993, ratio: 0.25, ok: true, missing: null, \
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
    // 2^53 + 1: the fabric-era double could not hold it (§48 §5 finding 6).
    assert_eq!(number(&read, 0), 9_007_199_254_740_993);
    assert!(matches!(row.values[1].kind, Some(Kind::Float64(f)) if f == 0.25));
    assert!(matches!(row.values[2].kind, Some(Kind::Boolean(true))));
    assert!(
        matches!(row.values[3].kind, Some(Kind::Null(_))),
        "null is null, not a zero: {:?}",
        row.values[3]
    );
    // `datetime(...)` is Grafeo's microsecond Timestamp, a wall-clock date and time.
    let Some(Kind::LocalDatetime(at)) = &row.values[4].kind else {
        panic!("a datetime is a local_datetime: {:?}", row.values[4]);
    };
    let date = at.date.as_option().expect("a date");
    let time = at.time.as_option().expect("a time");
    assert_eq!((date.year, date.month, date.day), (2026, 10, 4));
    assert_eq!((time.hour, time.minute, time.second), (9, 30, 0));
    let Some(Kind::Date(day)) = &row.values[5].kind else {
        panic!("a date is a date: {:?}", row.values[5]);
    };
    assert_eq!((day.year, day.month, day.day), (2026, 10, 4));
    let Some(Kind::List(tags)) = &row.values[6].kind else {
        panic!("a list crosses as a list: {:?}", row.values[6]);
    };
    assert_eq!(tags.values.len(), 2);
    assert!(matches!(&tags.values[1].kind, Some(Kind::String(s)) if s == "b"));
    let Some(Kind::Map(meta)) = &row.values[7].kind else {
        panic!("a map crosses as a map: {:?}", row.values[7]);
    };
    assert!(matches!(
        meta.entries.get("k").and_then(|v| v.kind.as_ref()),
        Some(Kind::Int64(1))
    ));
    assert_eq!(set.columns.len(), row.values.len());
}

#[test]
fn open_is_idempotent_and_conflict_is_refused() {
    let engine = engine();
    let first = open(&engine, "acme", "orders");
    let second = open(&engine, "acme", "orders");
    assert_eq!(first, second, "the same key is the same graph");
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
    assert_eq!(number(&count, 0), 1);

    // Reopening with a different read-only flag, or a different path, is a conflict rather than a
    // second opinion about one graph's storage. The service takes no path or flag from a client
    // (Review Focus 4), so the conflict is the engine's, and it is refused before anything opens.
    let err = Graph::open(&engine, "acme", "orders", OpenSpec::in_memory().read_only())
        .expect_err("read_only disagrees with the open graph");
    assert!(matches!(err, GraphError::Conflict { .. }), "{err:?}");
    assert!(err.to_string().contains("acme/orders"), "{err:?}");

    let data_dir = scratch_dir("conflict");
    let stored = Engine::with_data_dir(&data_dir);
    let id = loams_graph::GraphId::new();
    let spec = OpenSpec::persistent(&stored, id).expect("a data dir");
    Graph::open(&stored, "acme", "orders", spec).expect("open persistent");
    let err = Graph::open(&stored, "acme", "orders", OpenSpec::in_memory())
        .expect_err("different storage is a different graph");
    assert!(matches!(err, GraphError::Conflict { .. }), "{err:?}");
    let err = Graph::open(
        &stored,
        "acme",
        "orders",
        OpenSpec::persistent(&stored, loams_graph::GraphId::new()).expect("a data dir"),
    )
    .expect_err("another id is another graph's storage");
    assert!(matches!(err, GraphError::Conflict { .. }), "{err:?}");

    // `CreateGraph` of a name that is already open answers that graph, persistent or not.
    let created = service::create_graph(
        &stored,
        pb::CreateGraphRequest {
            namespace: "acme".to_string(),
            name: "orders".to_string(),
            ..Default::default()
        },
    )
    .expect("idempotent by name");
    assert_eq!(created.id, id.to_string());
    assert_eq!(stored.close("acme", "orders"), Ok(true));
    std::fs::remove_dir_all(&data_dir).ok();

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
        0,
        "the other graph in the namespace does not see the first graph's nodes"
    );
}

#[test]
fn a_persistent_graph_survives_a_close_and_a_reopen() {
    let directory = scratch_dir("persistent");
    let engine = Engine::with_data_dir(&directory);
    let spec = OpenSpec::persistent(&engine, loams_graph::GraphId::new()).expect("a data dir");
    let graph = Graph::open(&engine, "acme", "durable", spec.clone()).expect("open a new graph");
    assert!(
        graph.is_persistent(),
        "a data directory means the storage outlives the process"
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
    let reopened = Engine::with_data_dir(&directory);
    let read;
    {
        let graph = Graph::open(&reopened, "acme", "durable", spec.clone()).expect("reopen");
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
    let readonly = Engine::with_data_dir(&directory);
    let graph =
        Graph::open(&readonly, "acme", "durable", spec.read_only()).expect("open read-only");
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
            graph: "guard".to_string(),
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
                graph: "guard".to_string(),
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
    assert_eq!(number(&count, 0), 1, "only the first insert landed");

    // A graph opened read-only refuses a write even when the request forgot to ask for it.
    let readonly = Graph::open(&engine, "acme", "sealed", OpenSpec::in_memory().read_only())
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
            graph: "guard".to_string(),
            statement: "MATCH (d:Doc) WHERE d.body = 'INSERT' RETURN count(d) AS c".to_string(),
            read_only: true,
            ..Default::default()
        },
    )
    .expect("a read that mentions INSERT is still a read");
    assert_eq!(
        number(&contains, 0),
        0,
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
        matches!(value.kind, Some(Kind::Null(_))),
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
            graph: "batch".to_string(),
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
    assert_eq!(number(&read, 0), 3, "every insert in the batch landed");
    let edge = ok(
        &engine,
        "acme",
        "batch",
        "MATCH (:Line {sku: 'a'})-[:NEXT]->(:Line {sku: 'b'}) RETURN 1 AS ok",
    );
    assert_eq!(
        number(&edge, 0),
        1,
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
            graph: "batch".to_string(),
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
        3,
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
                graph: "batch".to_string(),
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
    assert_eq!(number(&after, 0), 3, "a refused batch wrote nothing");

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
            graph: "batch".to_string(),
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
    assert_eq!(response.committed_through, 2, "both statements committed");
    let after = ok(
        &engine,
        "acme",
        "batch",
        "MATCH (l:Line) RETURN count(l) AS c",
    );
    assert_eq!(number(&after, 0), 5);
}

#[test]
fn an_unavailable_language_is_unimplemented() {
    let engine = engine();
    open(&engine, "acme", "languages");

    // What this build has, per `EngineInfo`: Grafeo with `gql` and none of `cypher`, `sparql`,
    // `gremlin`, `graphql` or `sql-pgq` (root `Cargo.toml`, R0.2).
    let info = service::engine_info(&engine);
    assert_eq!(
        info.languages.as_slice(),
        [pb::QueryLanguage::Gql],
        "one language, and it is the only one this build has"
    );
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
                graph: "languages".to_string(),
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
                graph: "languages".to_string(),
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
            graph: "languages".to_string(),
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
                graph: "languages".to_string(),
                statement: "RETURN 1 AS one".to_string(),
                language: language.into(),
                ..Default::default()
            },
        )
        .unwrap_or_else(|err| panic!("{language:?} should be served: {err:?}"));
        assert_eq!(number(&response, 0), 1);
    }
}

#[test]
fn list_and_delete_manage_the_registry() {
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
    let held = Graph::open(&engine, "acme", "one", OpenSpec::in_memory())
        .expect("the same spec returns the open graph");
    let err = service::delete_graph(
        &engine,
        pb::DeleteGraphRequest {
            namespace: "acme".to_string(),
            name: "one".to_string(),
            ..Default::default()
        },
    )
    .expect_err("a held handle refuses the delete");
    assert_eq!(err.code, ErrorCode::Internal, "{err:?}");
    assert!(message(&err).contains("still in use"), "{err:?}");

    // Once the last handle goes, the close succeeds and the graph leaves the registry.
    drop(held);
    service::delete_graph(
        &engine,
        pb::DeleteGraphRequest {
            namespace: "acme".to_string(),
            name: "one".to_string(),
            ..Default::default()
        },
    )
    .expect("deleting with no holder succeeds");
    // The delete is answered as a finished operation on the graph.
    let operation = service::delete_graph(
        &engine,
        pb::DeleteGraphRequest {
            namespace: "acme".to_string(),
            name: "two".to_string(),
            ..Default::default()
        },
    )
    .expect("delete");
    assert_eq!(operation.kind, "graph.delete");
    assert_eq!(
        operation.target.get("graph").map(String::as_str),
        Some("two")
    );
    assert_eq!(
        operation.state.as_known(),
        Some(loams_proto::loams::operations::v1::OperationState::Succeeded)
    );
    let listed = service::list_graphs(
        &engine,
        pb::ListGraphsRequest {
            namespace: "acme".to_string(),
            ..Default::default()
        },
    )
    .expect("listing cannot fail");
    assert_eq!(listed.graphs.len(), 0);

    // A closed graph is gone, not dormant: executing against it is `NotFound`.
    let err =
        execute(&engine, "acme", "one", "RETURN 1 AS one").expect_err("a closed graph is not open");
    assert_eq!(err.code, ErrorCode::NotFound, "{err:?}");

    // Closing something that is not open is not an error; it is already in the state asked for.
    service::delete_graph(
        &engine,
        pb::DeleteGraphRequest {
            namespace: "acme".to_string(),
            name: "one".to_string(),
            ..Default::default()
        },
    )
    .expect("deleting an unopened graph is a no-op");

    // And a statement against a graph that was never opened is `NotFound` rather than a refusal to
    // open one implicitly.
    let err = execute(&engine, "acme", "never-opened", "RETURN 1 AS one")
        .expect_err("execute does not open a graph");
    assert_eq!(err.code, ErrorCode::NotFound, "{err:?}");
}

#[test]
fn sessions_do_not_share_state_and_no_graph_can_be_switched_to() {
    let engine = engine();
    open(&engine, "acme", "sessions");

    // Every call runs on a fresh session with the role it needs (GR1 Task 3), so there is no
    // session for two statements to share: a committed write is visible to the next call, and a
    // call cannot see a half-finished one. The fabric-era leak — `GrafeoDB::execute` wrote the
    // current graph back so `USE GRAPH` survived a call — is closed by refusing graph management
    // outright (R0.10 (b)): one Loams graph is one engine graph.

    ok(&engine, "acme", "sessions", "INSERT (:P {name: 'one'})");
    let count = ok(
        &engine,
        "acme",
        "sessions",
        "MATCH (p:P) RETURN count(p) AS c",
    );
    assert_eq!(
        number(&count, 0),
        1,
        "a committed write is visible to the next statement: there is no session affinity"
    );

    for statement in [
        "CREATE GRAPH scratch",
        "USE GRAPH scratch",
        "SESSION SET GRAPH scratch",
    ] {
        let err = execute(&engine, "acme", "sessions", statement).expect_err("refused");
        assert_eq!(
            err.code,
            ErrorCode::FailedPrecondition,
            "{statement}: {err:?}"
        );
        assert_eq!(reason(&err), "graph_statement_not_allowed");
    }
    // Nor is any other session command: each call is its own session.
    for statement in [
        "SESSION RESET GRAPH",
        "SESSION SET TIME ZONE 'UTC'",
        "SESSION CLOSE",
    ] {
        let err = execute(&engine, "acme", "sessions", statement).expect_err("refused");
        assert_eq!(
            reason(&err),
            "graph_statement_not_allowed",
            "{statement}: {err:?}"
        );
    }
    let restored = ok(
        &engine,
        "acme",
        "sessions",
        "MATCH (p:P) RETURN count(p) AS c",
    );
    assert_eq!(
        number(&restored, 0),
        1,
        "the default graph is the only graph"
    );

    // A write inside an open transaction is invisible to another session -- which is the property
    // `batch_is_one_transaction` leans on, asserted here from the read side as well.
    let graph =
        Graph::open(&engine, "acme", "sessions", OpenSpec::in_memory()).expect("the open graph");
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
        grafeo::Value::Int64(1),
        "only the committed node is counted"
    );
}

#[test]
fn graph_service_impl_serves_one_engine() {
    let engine = std::sync::Arc::new(engine());
    let service = loams_graph::GraphServiceImpl::new(std::sync::Arc::clone(&engine));
    assert!(std::sync::Arc::ptr_eq(service.engine(), &engine));
    let info = service.engine_info();
    assert_eq!(info.engine_version, loams_graph::ENGINE_VERSION);
    assert_eq!(engine.standard, loams_graph::GQL_STANDARD);
}

/// The `loams.errors.v1.ErrorInfo` reason a refusal carries.
fn reason(err: &ConnectError) -> String {
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
    loams_proto::loams::errors::v1::ErrorInfo::decode_from_slice(&bytes)
        .expect("an ErrorInfo")
        .reason
}

/// Grafeo builds a path of element ids; the server answers full elements, in the path's order,
/// with each relationship's stored direction (GR1 Task 2 review).
#[test]
fn path_elements_are_full_and_keep_direction() {
    let engine = engine();
    open(&engine, "acme", "paths");
    ok(
        &engine,
        "acme",
        "paths",
        "INSERT (:A {name: 'a'})-[:R {w: 2}]->(:B {name: 'b'})",
    );
    // Walked against the edge: b first, then a.
    let read = ok(
        &engine,
        "acme",
        "paths",
        "MATCH p = (b:B)<-[r:R]-(a:A) RETURN p, a, b",
    );
    let row = &rows(&read).rows[0];
    let (Some(Kind::Path(path)), Some(Kind::Node(a)), Some(Kind::Node(b))) = (
        &row.values[0].kind,
        &row.values[1].kind,
        &row.values[2].kind,
    ) else {
        panic!("a path and two nodes: {row:?}");
    };
    assert_eq!(path.nodes.len(), 2);
    assert_eq!(path.relationships.len(), 1);
    // The path's own order.
    assert_eq!(path.nodes[0].id, b.id);
    assert_eq!(path.nodes[1].id, a.id);
    // Full elements.
    assert_eq!(path.nodes[0].labels, ["B"]);
    assert_eq!(path.nodes[1].labels, ["A"]);
    assert!(matches!(
        path.nodes[1].properties.get("name").and_then(|v| v.kind.as_ref()),
        Some(Kind::String(name)) if name == "a"
    ));
    let rel = &path.relationships[0];
    assert_eq!(rel.r#type, "R");
    assert!(matches!(
        rel.properties.get("w").and_then(|v| v.kind.as_ref()),
        Some(Kind::Int64(2))
    ));
    // The stored direction, not the walk's: a -> b.
    assert_eq!(rel.src, a.id, "{rel:?}");
    assert_eq!(rel.dst, b.id, "{rel:?}");
}

/// A non-atomic batch stops at the first failing statement and still answers: the committed
/// results, `committed_through` and the failed statement's error (GR1 Task 2 review).
#[test]
fn non_atomic_batch_reports_the_failed_statement() {
    let engine = engine();
    open(&engine, "acme", "partial");
    let statements = [
        "INSERT (:Line {sku: 'a'})",
        "MATCH (n RETURN n",
        "INSERT (:Line {sku: 'c'})",
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
            graph: "partial".to_string(),
            statements,
            atomic: false,
            ..Default::default()
        },
    )
    .expect("a non-atomic batch answers even when a statement fails");
    assert!(!response.committed);
    assert_eq!(
        response.results.len(),
        1,
        "the statement before the failure"
    );
    assert_eq!(response.committed_through, 1);
    let error = response.error.as_option().expect("the failed statement");
    assert_eq!(error.index, 1);
    assert_eq!(error.code, "invalid_argument");
    assert!(error.message.contains("Expected RParen"), "{error:?}");
    assert_eq!(
        error.info.as_option().map(|info| info.reason.as_str()),
        Some("gql_syntax_error")
    );
    let count = ok(
        &engine,
        "acme",
        "partial",
        "MATCH (l:Line) RETURN count(l) AS c",
    );
    assert_eq!(
        number(&count, 0),
        1,
        "the statement after the failure did not run"
    );

    // A batch with no failure has no error.
    let response = service::execute_batch(
        &engine,
        pb::ExecuteBatchRequest {
            namespace: "acme".to_string(),
            graph: "partial".to_string(),
            statements: vec![pb::Statement {
                statement: "INSERT (:Line {sku: 'd'})".to_string(),
                ..Default::default()
            }],
            atomic: false,
            ..Default::default()
        },
    )
    .expect("runs");
    assert!(response.error.as_option().is_none());
    assert_eq!(response.committed_through, 1);
}

/// `Statement.language` is checked, not just the batch's.
#[test]
fn batch_checks_each_statements_language() {
    let engine = engine();
    open(&engine, "acme", "langs");
    for atomic in [true, false] {
        let err = service::execute_batch(
            &engine,
            pb::ExecuteBatchRequest {
                namespace: "acme".to_string(),
                graph: "langs".to_string(),
                statements: vec![
                    pb::Statement {
                        statement: "INSERT (:X)".to_string(),
                        ..Default::default()
                    },
                    pb::Statement {
                        statement: "CREATE (:X)".to_string(),
                        language: pb::QueryLanguage::Cypher.into(),
                        ..Default::default()
                    },
                ],
                atomic,
                ..Default::default()
            },
        )
        .expect_err("a Cypher statement is refused in a GQL-only build");
        assert_eq!(err.code, ErrorCode::Unimplemented, "{err:?}");
        assert_eq!(reason(&err), "graph_language_disabled");
    }
    let count = ok(&engine, "acme", "langs", "MATCH (x:X) RETURN count(x) AS c");
    assert_eq!(number(&count, 0), 0, "nothing ran");
}

/// PROFILE runs the statement, so a writing statement is refused with `graph_read_only`
/// (GR1 Task 2 review).
#[test]
fn profile_refuses_a_write() {
    let engine = engine();
    open(&engine, "acme", "explain");
    let explain = |statement: &str, profile: bool| {
        service::explain(
            &engine,
            pb::ExplainRequest {
                namespace: "acme".to_string(),
                graph: "explain".to_string(),
                statement: statement.to_string(),
                profile,
                timeout_ms: 1000,
                ..Default::default()
            },
        )
    };
    for statement in [
        "INSERT (:Doc {body: 'x'})",
        "MATCH (d:Doc) SET d.body = 'y'",
        "/* hidden */ MERGE (:Doc)",
    ] {
        let err = explain(statement, true).expect_err("a writing PROFILE is refused");
        assert_eq!(
            err.code,
            ErrorCode::PermissionDenied,
            "{statement}: {err:?}"
        );
        assert_eq!(reason(&err), "graph_read_only", "{statement}");
    }
    let count = ok(
        &engine,
        "acme",
        "explain",
        "MATCH (d:Doc) RETURN count(d) AS c",
    );
    assert_eq!(number(&count, 0), 0, "nothing was written");

    // EXPLAIN of a write, and PROFILE of a read, pass the guard (the plan itself is Task 6's).
    for (statement, profile) in [("INSERT (:Doc)", false), ("MATCH (d:Doc) RETURN d", true)] {
        let err = explain(statement, profile).expect_err("not implemented yet");
        assert_eq!(err.code, ErrorCode::Unimplemented, "{err:?}");
        assert_eq!(reason(&err), "not_implemented");
    }
}

/// A user property named like one of the engine's reserved keys (`_id`, `_labels`, `_type`,
/// `_source`, `_target`) never overrides the real field of a resolved path element (GR1 Task 2
/// N3).
#[test]
fn reserved_property_names_do_not_override_path_elements() {
    let engine = engine();
    open(&engine, "acme", "reserved");
    ok(
        &engine,
        "acme",
        "reserved",
        "INSERT (:A {_id: 999, _labels: 'fake'})-[:R {_id: 998, _type: 'FAKE', _source: 997, _target: 996}]->(:B {_id: 995})",
    );
    let read = ok(
        &engine,
        "acme",
        "reserved",
        "MATCH p = (a:A)-[r:R]->(b:B) RETURN p, id(a), id(r), id(b)",
    );
    let row = &rows(&read).rows[0];
    let Some(Kind::Path(path)) = &row.values[0].kind else {
        panic!("a path: {row:?}");
    };
    let id = |i: usize| match row.values[i].kind {
        Some(Kind::Int64(n)) => n as u64,
        ref other => panic!("{other:?}"),
    };
    assert_eq!(path.nodes[0].id, id(1), "{path:?}");
    assert_eq!(path.nodes[0].labels, ["A"]);
    assert_eq!(path.nodes[1].id, id(3));
    let rel = &path.relationships[0];
    assert_eq!(rel.id, id(2));
    assert_eq!(rel.r#type, "R");
    assert_eq!((rel.src, rel.dst), (id(1), id(3)));
}

/// A statement's own language wins over the batch's (GR1 Task 2 N4): a batch that names Cypher
/// runs when every statement names GQL, and the batch's language applies only to statements that
/// name none.
#[test]
fn statement_language_wins_over_the_batch() {
    let engine = engine();
    open(&engine, "acme", "override");
    let gql = |text: &str| pb::Statement {
        statement: text.to_string(),
        language: pb::QueryLanguage::Gql.into(),
        ..Default::default()
    };
    for atomic in [true, false] {
        let response = service::execute_batch(
            &engine,
            pb::ExecuteBatchRequest {
                namespace: "acme".to_string(),
                graph: "override".to_string(),
                language: pb::QueryLanguage::Cypher.into(),
                statements: vec![gql("INSERT (:O)"), gql("RETURN 1 AS one")],
                atomic,
                ..Default::default()
            },
        )
        .expect("every statement names GQL");
        assert_eq!(response.committed_through, 2);
    }
    // One statement that names nothing takes the batch's Cypher, which this build lacks.
    let err = service::execute_batch(
        &engine,
        pb::ExecuteBatchRequest {
            namespace: "acme".to_string(),
            graph: "override".to_string(),
            language: pb::QueryLanguage::Cypher.into(),
            statements: vec![
                gql("INSERT (:O)"),
                pb::Statement {
                    statement: "RETURN 1 AS one".to_string(),
                    ..Default::default()
                },
            ],
            atomic: true,
            ..Default::default()
        },
    )
    .expect_err("the second statement is Cypher");
    assert_eq!(err.code, ErrorCode::Unimplemented);
    assert_eq!(reason(&err), "graph_language_disabled");
}

/// GR1 Task 5 (Task 4 review M8): a graph opens outside the registry lock under a per-graph
/// latch, so concurrent openers share one graph.
#[test]
fn concurrent_opens_share_one_graph() {
    let data_dir = scratch_dir("latch");
    let engine = std::sync::Arc::new(Engine::with_data_dir(&data_dir));
    let id = loams_graph::GraphId::new();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let graphs: Vec<_> = (0..8)
        .map(|_| {
            let (engine, barrier) = (engine.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                Graph::open_or_existing(&engine, "acme", "shared", || {
                    OpenSpec::persistent(&engine, id).expect("a data dir")
                })
                .expect("open")
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|h| h.join().expect("thread"))
        .collect();
    for graph in &graphs[1..] {
        assert!(
            std::sync::Arc::ptr_eq(&graphs[0], graph),
            "one graph for every opener"
        );
    }
    drop(graphs);
    assert_eq!(engine.close("acme", "shared"), Ok(true));
    std::fs::remove_dir_all(&data_dir).ok();
}

/// A gate a test hook blocks on until the test opens it, telling the test once it is waiting.
struct Gate {
    entered: std::sync::mpsc::SyncSender<()>,
    open: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
}

impl Gate {
    fn new() -> (
        std::sync::Arc<Gate>,
        std::sync::mpsc::Receiver<()>,
        std::sync::mpsc::SyncSender<()>,
    ) {
        let (entered, entered_rx) = std::sync::mpsc::sync_channel(1);
        let (open_tx, open) = std::sync::mpsc::sync_channel(1);
        (
            std::sync::Arc::new(Gate {
                entered,
                open: std::sync::Mutex::new(open),
            }),
            entered_rx,
            open_tx,
        )
    }

    fn pass(&self) {
        self.entered.send(()).expect("the test waits");
        self.open
            .lock()
            .expect("gate")
            .recv()
            .expect("the test opens it");
    }
}

/// GR1 Task 5 fix round 1 (I2): a waiter on a graph's opening latch keeps the latch in the map,
/// so after the graph is opened and closed, it and a newcomer still share one latch and the
/// graph is opened once, not twice (the second registration overwriting the first).
#[test]
fn opens_around_a_close_never_double_open() {
    use loams_graph::engine::OpenPoint;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let engine = Arc::new(Engine::new());
    let (waiter_gate, waiter_in, open_waiter) = Gate::new();
    let (newcomer_gate, newcomer_in, open_newcomer) = Gate::new();
    let latch_taken = Arc::new(AtomicUsize::new(0));
    let opens = Arc::new(AtomicUsize::new(0));
    {
        let (latch_taken, opens) = (latch_taken.clone(), opens.clone());
        engine.set_open_hook(Some(Arc::new(move |point, _name: &str| match point {
            // The first opener to take the latch is the waiter: it stops before locking it.
            OpenPoint::LatchTaken => {
                if latch_taken.fetch_add(1, Ordering::SeqCst) == 0 {
                    waiter_gate.pass();
                }
            }
            // The second disk open is the newcomer's: it stops holding the latch.
            OpenPoint::BeforeOpen => {
                if opens.fetch_add(1, Ordering::SeqCst) == 1 {
                    newcomer_gate.pass();
                }
            }
        })));
    }
    let open = |engine: Arc<Engine>| {
        std::thread::spawn(move || {
            Graph::open_or_existing(&engine, "acme", "g", OpenSpec::in_memory).expect("open")
        })
    };
    let waiter = open(engine.clone());
    waiter_in.recv().expect("the waiter holds the latch");
    // Opened and closed while the waiter still holds the latch.
    let first = Graph::open_or_existing(&engine, "acme", "g", OpenSpec::in_memory).expect("open");
    drop(first);
    assert_eq!(engine.close("acme", "g"), Ok(true));
    let newcomer = open(engine.clone());
    newcomer_in.recv().expect("the newcomer is opening");
    open_waiter.send(()).expect("release the waiter");
    std::thread::sleep(std::time::Duration::from_millis(200));
    open_newcomer.send(()).expect("release the newcomer");
    let (waiter, newcomer) = (
        waiter.join().expect("waiter"),
        newcomer.join().expect("newcomer"),
    );
    assert!(Arc::ptr_eq(&waiter, &newcomer), "one graph after the close");
    assert_eq!(
        opens.load(Ordering::SeqCst),
        2,
        "opened once before and once after the close"
    );
    assert_eq!(engine.list(None).expect("list").len(), 1);
    assert_eq!(engine.opening_latches(), 0, "no latch left behind");
}

/// GR1 Task 5 fix round 1 (I2): an open that fails takes its latch out of the map too.
#[test]
fn a_failed_open_leaves_no_latch() {
    let data_dir = scratch_dir("latch-fail");
    let engine = Engine::with_data_dir(&data_dir);
    // Read-only needs an existing store, and this one has none.
    let spec = OpenSpec::persistent(&engine, loams_graph::GraphId::new())
        .expect("a data dir")
        .read_only();
    Graph::open(&engine, "acme", "missing", spec).expect_err("no store to open read-only");
    assert_eq!(engine.opening_latches(), 0);
    std::fs::remove_dir_all(&data_dir).ok();
}

/// GR1 Task 5 (Task 4 review M8): an open of one graph does not wait on another's, nor do
/// lookups of the open graphs.
#[test]
fn open_of_one_graph_does_not_wait_on_another() {
    use loams_graph::engine::OpenPoint;
    use std::sync::Arc;
    let engine = Arc::new(Engine::new());
    let (slow_gate, slow_in, open_slow) = Gate::new();
    engine.set_open_hook(Some(Arc::new(move |point, name: &str| {
        if point == OpenPoint::BeforeOpen && name == "slow" {
            slow_gate.pass();
        }
    })));
    let slow = {
        let engine = engine.clone();
        std::thread::spawn(move || {
            Graph::open_or_existing(&engine, "acme", "slow", OpenSpec::in_memory).expect("open")
        })
    };
    slow_in.recv().expect("the slow open is under way");
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    {
        let engine = engine.clone();
        std::thread::spawn(move || {
            let fast = Graph::open_or_existing(&engine, "acme", "fast", OpenSpec::in_memory)
                .expect("open");
            let listed = engine.list(None).expect("list").len();
            done_tx.send((fast, listed)).expect("the test waits");
        });
    }
    let (fast, listed) = done_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the other graph opened while the slow open was under way");
    assert_eq!(fast.name(), "fast");
    assert_eq!(listed, 1, "only the fast graph is registered yet");
    open_slow.send(()).expect("release the slow open");
    assert_eq!(slow.join().expect("slow").name(), "slow");
}
