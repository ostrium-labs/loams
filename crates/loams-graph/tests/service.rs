//! The service handlers over the engine (GR1 Task 3): parameters are bound on every path,
//! languages are checked per statement, and GQL's own transaction statements are refused because
//! the RPC owns the transaction (§48 §7.3).

use connectrpc::{ConnectError, ErrorCode};
use loams_graph::{Engine, service};
use loams_proto::loams::graph::v1 as pb;
use pb::__buffa::oneof::value::Kind;

fn engine_with(name: &str) -> Engine {
    let engine = Engine::new();
    service::create_graph(
        &engine,
        pb::CreateGraphRequest {
            namespace: "acme".to_string(),
            name: name.to_string(),
            ..Default::default()
        },
    )
    .expect("create");
    engine
}

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

fn value(kind: Kind) -> pb::Value {
    pb::Value {
        kind: Some(kind),
        ..Default::default()
    }
}

fn params(entries: &[(&str, Kind)]) -> ::buffa::__private::HashMap<String, pb::Value> {
    let mut out = ::buffa::__private::HashMap::default();
    for (name, kind) in entries {
        out.insert((*name).to_string(), value(kind.clone()));
    }
    out
}

fn execute(
    engine: &Engine,
    graph: &str,
    statement: &str,
) -> Result<pb::ExecuteResponse, ConnectError> {
    service::execute(
        engine,
        pb::ExecuteRequest {
            namespace: "acme".to_string(),
            graph: graph.to_string(),
            statement: statement.to_string(),
            ..Default::default()
        },
    )
}

fn count(engine: &Engine, graph: &str, statement: &str) -> i64 {
    let response = execute(engine, graph, statement).expect("count");
    match &response.rows.as_option().expect("rows").rows[0].values[0].kind {
        Some(Kind::Int64(n)) => *n,
        other => panic!("{other:?}"),
    }
}

#[test]
fn execute_binds_parameters() {
    let engine = engine_with("params");
    service::execute(
        &engine,
        pb::ExecuteRequest {
            namespace: "acme".to_string(),
            graph: "params".to_string(),
            statement: "INSERT (:P {name: $name, n: $n})".to_string(),
            parameters: params(&[
                ("name", Kind::String("x'); DROP".to_string())),
                ("n", Kind::Int64(9_007_199_254_740_993)),
            ]),
            ..Default::default()
        },
    )
    .expect("a bound insert");
    // The value was bound, never interpolated: the quote in it is data.
    let read = service::execute(
        &engine,
        pb::ExecuteRequest {
            namespace: "acme".to_string(),
            graph: "params".to_string(),
            statement: "MATCH (p:P) WHERE p.name = $name RETURN p.n".to_string(),
            parameters: params(&[("name", Kind::String("x'); DROP".to_string()))]),
            read_only: true,
            ..Default::default()
        },
    )
    .expect("a bound read");
    let rows = read.rows.as_option().expect("rows");
    assert_eq!(rows.rows.len(), 1);
    assert!(matches!(
        rows.rows[0].values[0].kind,
        Some(Kind::Int64(9_007_199_254_740_993))
    ));
    // A missing binding is the caller's error.
    let err = execute(
        &engine,
        "params",
        "MATCH (p:P) WHERE p.name = $missing RETURN p",
    )
    .expect_err("unbound");
    assert_eq!(err.code, ErrorCode::InvalidArgument, "{err:?}");
}

#[test]
fn non_atomic_batch_binds_parameters() {
    let engine = engine_with("batch");
    let statements = (1..=3)
        .map(|i| pb::Statement {
            statement: "INSERT (:B {i: $i})".to_string(),
            parameters: params(&[("i", Kind::Int64(i))]),
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
    .expect("runs");
    assert!(response.error.as_option().is_none(), "{:?}", response.error);
    assert_eq!(response.committed_through, 3);
    assert_eq!(
        count(&engine, "batch", "MATCH (b:B) RETURN sum(b.i) AS s"),
        6
    );
}

#[test]
fn statement_language_override_is_checked() {
    let engine = engine_with("langs");
    for (batch, statement) in [
        (pb::QueryLanguage::Gql, pb::QueryLanguage::Cypher),
        (pb::QueryLanguage::Unspecified, pb::QueryLanguage::Sparql),
    ] {
        let err = service::execute_batch(
            &engine,
            pb::ExecuteBatchRequest {
                namespace: "acme".to_string(),
                graph: "langs".to_string(),
                language: batch.into(),
                statements: vec![pb::Statement {
                    statement: "MATCH (n) RETURN n".to_string(),
                    language: statement.into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .expect_err("the statement's own language is checked");
        assert_eq!(err.code, ErrorCode::Unimplemented);
        assert_eq!(reason(&err), "graph_language_disabled");
    }
    // A statement naming GQL in a batch that names GQL runs.
    service::execute_batch(
        &engine,
        pb::ExecuteBatchRequest {
            namespace: "acme".to_string(),
            graph: "langs".to_string(),
            statements: vec![pb::Statement {
                statement: "RETURN 1 AS one".to_string(),
                language: pb::QueryLanguage::Gql.into(),
                ..Default::default()
            }],
            ..Default::default()
        },
    )
    .expect("GQL runs");
}

#[test]
fn transaction_statements_refused() {
    let engine = engine_with("txn");
    for statement in [
        "START TRANSACTION",
        "START TRANSACTION READ ONLY",
        "COMMIT",
        "ROLLBACK",
        "SAVEPOINT s1",
        "/* hidden */ COMMIT",
    ] {
        let err = execute(&engine, "txn", statement).expect_err("refused");
        assert_eq!(err.code, ErrorCode::InvalidArgument, "{statement}: {err:?}");
        assert_eq!(reason(&err), "graph_transaction_statement", "{statement}");
        // And inside a batch, atomic or not, before anything runs.
        for atomic in [true, false] {
            let err = service::execute_batch(
                &engine,
                pb::ExecuteBatchRequest {
                    namespace: "acme".to_string(),
                    graph: "txn".to_string(),
                    statements: vec![
                        pb::Statement {
                            statement: "INSERT (:T)".to_string(),
                            ..Default::default()
                        },
                        pb::Statement {
                            statement: statement.to_string(),
                            ..Default::default()
                        },
                    ],
                    atomic,
                    ..Default::default()
                },
            );
            match err {
                Err(err) => assert_eq!(reason(&err), "graph_transaction_statement"),
                Ok(response) => {
                    let error = response.error.as_option().expect("the failed statement");
                    assert_eq!(error.index, 1);
                    assert_eq!(
                        error.info.as_option().map(|i| i.reason.as_str()),
                        Some("graph_transaction_statement")
                    );
                }
            }
        }
    }
}
