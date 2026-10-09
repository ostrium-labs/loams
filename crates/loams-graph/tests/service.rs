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

/// Security review I3: the registry is keyed by `(namespace, name)`, and both are validated, so
/// `("a/b", "c")` and `("a", "b/c")` cannot name the same graph.
#[test]
fn names_are_validated_and_cannot_collide() {
    let engine = Engine::new();
    for (namespace, name) in [
        ("a/b", "c"),
        ("a", "b/c"),
        ("", "g"),
        ("a", ""),
        ("a", "Upper"),
        ("a", "9lives"),
        ("a", &"g".repeat(64)),
        (&"n".repeat(64) as &str, "g"),
        ("a", "has space"),
        ("a\u{0}", "g"),
    ] {
        let err = service::create_graph(
            &engine,
            pb::CreateGraphRequest {
                namespace: namespace.to_string(),
                name: name.to_string(),
                ..Default::default()
            },
        )
        .expect_err("an invalid name is refused");
        assert_eq!(
            err.code,
            ErrorCode::InvalidArgument,
            "{namespace:?}/{name:?}: {err:?}"
        );
        assert_eq!(reason(&err), "invalid_argument");
        let err =
            loams_graph::Graph::open(&engine, namespace, name, loams_graph::OpenSpec::in_memory())
                .expect_err("and the engine refuses it too");
        assert_eq!(err.reason(), "invalid_argument");
    }
    // A delete that would have matched `a/b` + `c` under a string key is refused, not a no-op on
    // someone else's graph.
    let err = service::delete_graph(
        &engine,
        pb::DeleteGraphRequest {
            namespace: "a".to_string(),
            name: "b/c".to_string(),
            ..Default::default()
        },
    )
    .expect_err("refused");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    // Valid names at the limits are fine.
    for (namespace, name) in [
        ("a", "g"),
        (&"n".repeat(63) as &str, &"g".repeat(63) as &str),
    ] {
        service::create_graph(
            &engine,
            pb::CreateGraphRequest {
                namespace: namespace.to_string(),
                name: name.to_string(),
                ..Default::default()
            },
        )
        .expect("valid");
    }
}

/// Security review M6: concurrent `CreateGraph` of one name all answer the same graph.
#[test]
fn concurrent_create_is_idempotent() {
    let data_dir = std::env::temp_dir().join(format!("loams-graph-m6-{}", std::process::id()));
    let engine = std::sync::Arc::new(Engine::with_data_dir(&data_dir));
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let (engine, barrier) = (engine.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                service::create_graph(
                    &engine,
                    pb::CreateGraphRequest {
                        namespace: "acme".to_string(),
                        name: "race".to_string(),
                        ..Default::default()
                    },
                )
            })
        })
        .collect();
    let ids: std::collections::BTreeSet<String> = handles
        .into_iter()
        .map(|h| h.join().expect("thread").expect("every create succeeds").id)
        .collect();
    assert_eq!(ids.len(), 1, "one graph: {ids:?}");
    std::fs::remove_dir_all(&data_dir).ok();
}

// ---------------------------------------------------------------------------------------------
// The graph catalog and GraphAdminService (GR1 Task 4)
// ---------------------------------------------------------------------------------------------

mod admin {
    use std::sync::Arc;
    use std::time::Duration;

    use connectrpc::ErrorCode;
    use loams_common::meta::MetaStore;
    use loams_graph::Engine;
    use loams_graph::catalog::GraphCatalog;
    use loams_graph::service::admin::GraphAdmin;
    use loams_meta::{MetaClient, MetaClientConfig, MetaConfig, MetaNode, Router, SystemClock};
    use loams_proto::loams::graph::v1 as pb;
    use loams_store::{Fault, FaultRates, FaultyStore, Op, Store};
    use tempfile::TempDir;

    use super::reason;

    /// A single-node metastore and a bucket, kept for the life of a test so a second catalog
    /// and engine can be started over the same state (`graphs_survive_restart`).
    struct Fixture {
        _node: MetaNode,
        meta: Arc<dyn MetaStore>,
        store: Store,
        /// The bucket under `store`, for injected delays and call counts.
        faulty: Arc<FaultyStore>,
        data_dir: TempDir,
        _meta_dir: TempDir,
    }

    impl Fixture {
        async fn start() -> Self {
            let meta_dir = TempDir::new().expect("temp dir");
            let clock = Arc::new(SystemClock);
            let mut config = MetaConfig::new(1, meta_dir.path(), Store::in_memory());
            config.clock = clock.clone();
            let node = MetaNode::start(config, &Router::new())
                .await
                .expect("start meta");
            node.initialize([1]).await.expect("initialize");
            node.wait_for_leader(Duration::from_secs(30))
                .await
                .expect("leader");
            let client = MetaClient::new(node.clone(), vec![], clock, MetaClientConfig::default());
            let faulty = Arc::new(FaultyStore::new(Store::in_memory().inner().clone()));
            Self {
                _node: node,
                meta: Arc::new(client),
                store: Store::new(faulty.clone()),
                faulty,
                data_dir: TempDir::new().expect("data dir"),
                _meta_dir: meta_dir,
            }
        }

        /// A fresh engine and catalog over the fixture's metastore, bucket and data dir: what a
        /// restarted server has.
        fn admin(&self) -> GraphAdmin {
            GraphAdmin::new(
                Arc::new(Engine::with_data_dir(self.data_dir.path())),
                GraphCatalog::new(self.meta.clone(), self.store.clone()),
            )
        }
    }

    fn create(ns: &str, name: &str, key: &str) -> pb::CreateGraphRequest {
        pb::CreateGraphRequest {
            namespace: ns.to_string(),
            name: name.to_string(),
            idempotency_key: key.to_string(),
            ..Default::default()
        }
    }

    fn get(ns: &str, name: &str) -> pb::GetGraphRequest {
        pb::GetGraphRequest {
            namespace: ns.to_string(),
            name: name.to_string(),
            ..Default::default()
        }
    }

    fn execute(ns: &str, graph: &str, statement: &str) -> pb::ExecuteRequest {
        pb::ExecuteRequest {
            namespace: ns.to_string(),
            graph: graph.to_string(),
            statement: statement.to_string(),
            ..Default::default()
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn create_is_idempotent_by_key() {
        let fixture = Fixture::start().await;
        let admin = fixture.admin();
        let first = admin
            .create_graph(create("acme", "kg", "k1"))
            .await
            .expect("create");
        assert!(first.id.starts_with("gr_"), "{first:?}");
        assert_eq!(first.version, 1);
        assert_eq!(first.state.as_known(), Some(pb::GraphState::Ready));
        let again = admin
            .create_graph(create("acme", "kg", "k1"))
            .await
            .expect("a retry");
        assert_eq!(again.id, first.id, "the same key answers the same graph");
        assert_eq!(again.version, first.version);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn create_duplicate_name_already_exists() {
        let fixture = Fixture::start().await;
        let admin = fixture.admin();
        admin
            .create_graph(create("acme", "kg", "k1"))
            .await
            .expect("create");
        for key in ["k2", ""] {
            let err = admin
                .create_graph(create("acme", "kg", key))
                .await
                .expect_err("taken");
            assert_eq!(err.code, ErrorCode::AlreadyExists, "{err:?}");
            assert_eq!(reason(&err), "already_exists");
        }
        // The same name in another namespace is another graph.
        admin
            .create_graph(create("globex", "kg", "k1"))
            .await
            .expect("another namespace");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn list_paginates_aip158() {
        let fixture = Fixture::start().await;
        let admin = fixture.admin();
        for name in ["c", "a", "e", "b", "d"] {
            admin
                .create_graph(create("acme", name, name))
                .await
                .expect("create");
        }
        let page = |token: &str| pb::ListGraphsRequest {
            namespace: "acme".to_string(),
            page_size: 2,
            page_token: token.to_string(),
            ..Default::default()
        };
        let first = admin.list_graphs(page("")).await.expect("page 1");
        let names = |r: &pb::ListGraphsResponse| {
            r.graphs.iter().map(|g| g.name.clone()).collect::<Vec<_>>()
        };
        assert_eq!(names(&first), ["a", "b"]);
        assert!(!first.next_page_token.is_empty());
        // Inserts before and after the cursor do not shift the next page.
        admin
            .create_graph(create("acme", "aa", "aa"))
            .await
            .expect("insert before");
        admin
            .create_graph(create("acme", "f", "f"))
            .await
            .expect("insert after");
        let second = admin
            .list_graphs(page(&first.next_page_token))
            .await
            .expect("page 2");
        assert_eq!(names(&second), ["c", "d"]);
        let third = admin
            .list_graphs(page(&second.next_page_token))
            .await
            .expect("page 3");
        assert_eq!(names(&third), ["e", "f"]);
        assert!(
            third.next_page_token.is_empty(),
            "the last page has no token: {third:?}"
        );
        // A token is opaque and bound to its namespace.
        let err = admin
            .list_graphs(pb::ListGraphsRequest {
                namespace: "globex".to_string(),
                page_size: 2,
                page_token: first.next_page_token.clone(),
                ..Default::default()
            })
            .await
            .expect_err("another namespace's token");
        assert_eq!(err.code, ErrorCode::InvalidArgument);
        let err = admin
            .list_graphs(page("garbage!"))
            .await
            .expect_err("a bad token");
        assert_eq!(err.code, ErrorCode::InvalidArgument);
        // Page size 0 is the default, which covers all of these.
        let all = admin.list_graphs(page("")).await.map(|_| ());
        assert!(all.is_ok());
        let all = admin
            .list_graphs(pb::ListGraphsRequest {
                namespace: "acme".to_string(),
                ..Default::default()
            })
            .await
            .expect("default page");
        assert_eq!(names(&all), ["a", "aa", "b", "c", "d", "e", "f"]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn update_cas_conflict_aborted() {
        let fixture = Fixture::start().await;
        let admin = fixture.admin();
        let graph = admin
            .create_graph(create("acme", "kg", "k"))
            .await
            .expect("create");
        let update = |version: Option<u64>, replicas: u32| pb::UpdateGraphRequest {
            graph: pb::Graph {
                namespace: "acme".to_string(),
                name: "kg".to_string(),
                replicas,
                ..Default::default()
            }
            .into(),
            update_mask: buffa_types::google::protobuf::FieldMask {
                paths: vec!["replicas".to_string()],
                ..Default::default()
            }
            .into(),
            expected_version: version,
            ..Default::default()
        };
        let updated = admin
            .update_graph(update(Some(graph.version), 2))
            .await
            .expect("at version");
        assert_eq!(updated.replicas, 2);
        assert_eq!(updated.version, graph.version + 1);
        let err = admin
            .update_graph(update(Some(graph.version), 3))
            .await
            .expect_err("stale version");
        assert_eq!(err.code, ErrorCode::Aborted, "{err:?}");
        assert_eq!(reason(&err), "graph_catalog_version_mismatch");
        // No expected version: last writer wins.
        let latest = admin
            .update_graph(update(None, 1))
            .await
            .expect("unconditional");
        assert_eq!(latest.replicas, 1);
        // A field outside the mask, or an unknown path, is refused.
        let mut bad = update(None, 1);
        bad.update_mask = buffa_types::google::protobuf::FieldMask {
            paths: vec!["name".to_string()],
            ..Default::default()
        }
        .into();
        let err = admin
            .update_graph(bad)
            .await
            .expect_err("name is not updatable");
        assert_eq!(err.code, ErrorCode::InvalidArgument);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn delete_then_get_not_found() {
        let fixture = Fixture::start().await;
        let admin = fixture.admin();
        let deleted_id = admin
            .create_graph(create("acme", "kg", "k"))
            .await
            .expect("create")
            .id;
        admin
            .execute(execute("acme", "kg", "INSERT (:X)"))
            .await
            .expect("write");
        let operation = admin
            .delete_graph(pb::DeleteGraphRequest {
                namespace: "acme".to_string(),
                name: "kg".to_string(),
                idempotency_key: "d1".to_string(),
                ..Default::default()
            })
            .await
            .expect("delete");
        assert_eq!(operation.kind, "graph.delete");
        // A retry with the same key answers again; another key finds nothing to delete.
        for (key, ok) in [("d1", true), ("d2", false)] {
            let again = admin
                .delete_graph(pb::DeleteGraphRequest {
                    namespace: "acme".to_string(),
                    name: "kg".to_string(),
                    idempotency_key: key.to_string(),
                    ..Default::default()
                })
                .await;
            assert_eq!(again.is_ok(), ok, "{key}: {again:?}");
        }
        assert!(operation.id.starts_with("op-"), "{operation:?}");
        let err = admin.get_graph(get("acme", "kg")).await.expect_err("gone");
        assert_eq!(err.code, ErrorCode::NotFound);
        assert_eq!(reason(&err), "graph_not_found");
        let err = admin
            .execute(execute("acme", "kg", "RETURN 1 AS x"))
            .await
            .expect_err("gone");
        assert_eq!(err.code, ErrorCode::NotFound);
        // Not listed, and the name can be reused for a new graph.
        let listed = admin
            .list_graphs(pb::ListGraphsRequest {
                namespace: "acme".to_string(),
                ..Default::default()
            })
            .await
            .expect("list");
        assert!(listed.graphs.is_empty());
        let again = admin
            .create_graph(create("acme", "kg", "k2"))
            .await
            .expect("recreate");
        let fresh = admin
            .execute(execute("acme", "kg", "MATCH (x:X) RETURN count(x) AS c"))
            .await
            .expect("read");
        assert_eq!(
            format!(
                "{:?}",
                fresh.rows.as_option().expect("rows").rows[0].values[0].kind
            ),
            "Some(Int64(0))",
            "a recreated graph does not see the deleted one's data ({again:?})"
        );
        // Storage of the deleted graph is purged once its retention hold has passed.
        let deleted_dir = fixture.data_dir.path().join("graphs").join(&deleted_id);
        assert!(deleted_dir.exists(), "kept through the retention hold");
        assert_eq!(
            admin
                .purge_expired(Duration::from_secs(3600))
                .await
                .expect("purge"),
            0
        );
        let purged = admin.purge_expired(Duration::ZERO).await.expect("purge");
        assert_eq!(purged, 1);
        assert!(!deleted_dir.exists(), "storage purged");
        assert!(
            fixture
                .data_dir
                .path()
                .join("graphs")
                .join(&again.id)
                .exists(),
            "the recreated graph's storage is untouched"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn graphs_survive_restart() {
        let fixture = Fixture::start().await;
        let id = {
            let admin = fixture.admin();
            let graph = admin
                .create_graph(create("acme", "kg", "k"))
                .await
                .expect("create");
            admin
                .execute(execute("acme", "kg", "INSERT (:Kept {v: 1})"))
                .await
                .expect("write");
            admin.shutdown().await;
            graph.id
        };
        // A new engine and a new catalog over the same metastore, bucket and data dir.
        let admin = fixture.admin();
        let graph = admin
            .get_graph(get("acme", "kg"))
            .await
            .expect("the catalog reloads");
        assert_eq!(graph.id, id);
        let read = admin
            .execute(execute("acme", "kg", "MATCH (k:Kept) RETURN k.v"))
            .await
            .expect("the engine reopens lazily");
        assert_eq!(read.rows.as_option().expect("rows").rows.len(), 1);
    }

    /// Concurrent writers of one namespace's catalog all land: each CAS that loses retries on
    /// the winner's document.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_catalog_writes_all_land() {
        let fixture = Fixture::start().await;
        // Every bucket call may wait up to 20 ms, which holds read-modify-write windows open.
        fixture.faulty.set_rates(FaultRates {
            delay: 1.0,
            max_delay: Duration::from_millis(20),
            ..FaultRates::none()
        });
        let admin = Arc::new(fixture.admin());
        let tasks: Vec<_> = (0..8)
            .map(|i| {
                let admin = admin.clone();
                tokio::spawn(async move {
                    admin
                        .create_graph(create("acme", &format!("g{i}"), &format!("k{i}")))
                        .await
                })
            })
            .collect();
        for task in tasks {
            task.await.expect("task").expect("create");
        }
        let all = admin
            .list_graphs(pb::ListGraphsRequest {
                namespace: "acme".to_string(),
                ..Default::default()
            })
            .await
            .expect("list");
        assert_eq!(all.graphs.len(), 8);
        assert!(
            admin
                .catalog()
                .counters()
                .cas_retries
                .load(std::sync::atomic::Ordering::Relaxed)
                > 0,
            "the writers really did race (review I3)"
        );
    }

    /// Review I3: creates, deletes and `expected_version` updates race each other while
    /// readers run `GetGraph` and `Execute`; the final document is exactly the sum of the
    /// writes, every reader answers, and the race is real (CAS retries happened).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn mixed_concurrent_writes_and_reads_end_exactly() {
        let fixture = Fixture::start().await;
        let admin = Arc::new(fixture.admin());
        let mut versions = std::collections::BTreeMap::new();
        for i in 0..6 {
            let g = admin
                .create_graph(create("acme", &format!("g{i}"), &format!("seed{i}")))
                .await
                .expect("seed");
            versions.insert(format!("g{i}"), g.version);
        }
        admin
            .execute(execute("acme", "g3", "INSERT (:R {v: 1})"))
            .await
            .expect("seed data");
        fixture.faulty.set_rates(FaultRates {
            delay: 1.0,
            max_delay: Duration::from_millis(15),
            ..FaultRates::none()
        });
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut writers = Vec::new();
        for i in 0..4 {
            let admin = admin.clone();
            writers.push(tokio::spawn(async move {
                admin
                    .create_graph(create("acme", &format!("n{i}"), &format!("new{i}")))
                    .await
                    .map(|_| ())
            }));
        }
        for i in 0..2 {
            let admin = admin.clone();
            writers.push(tokio::spawn(async move {
                admin
                    .delete_graph(pb::DeleteGraphRequest {
                        namespace: "acme".to_string(),
                        name: format!("g{i}"),
                        idempotency_key: format!("del{i}"),
                        ..Default::default()
                    })
                    .await
                    .map(|_| ())
            }));
        }
        for i in 2..6 {
            let admin = admin.clone();
            let version = versions[&format!("g{i}")];
            writers.push(tokio::spawn(async move {
                admin
                    .update_graph(pb::UpdateGraphRequest {
                        graph: pb::Graph {
                            namespace: "acme".to_string(),
                            name: format!("g{i}"),
                            replicas: i,
                            ..Default::default()
                        }
                        .into(),
                        update_mask: buffa_types::google::protobuf::FieldMask {
                            paths: vec!["replicas".to_string()],
                            ..Default::default()
                        }
                        .into(),
                        expected_version: Some(version),
                        ..Default::default()
                    })
                    .await
                    .map(|_| ())
            }));
        }
        let mut readers = Vec::new();
        for _ in 0..2 {
            let (admin, stop) = (admin.clone(), stop.clone());
            readers.push(tokio::spawn(async move {
                let mut reads = 0u32;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    admin
                        .get_graph(get("acme", "g4"))
                        .await
                        .expect("a reader of a stable graph");
                    let rows = admin
                        .execute(execute("acme", "g3", "MATCH (r:R) RETURN count(r) AS c"))
                        .await
                        .expect("a statement on a stable graph");
                    assert_eq!(rows.rows.as_option().expect("rows").rows.len(), 1);
                    reads += 1;
                }
                reads
            }));
        }
        for writer in writers {
            writer.await.expect("task").expect("every write lands");
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        for reader in readers {
            assert!(reader.await.expect("reader") > 0);
        }
        fixture.faulty.set_rates(FaultRates::none());

        // The exact final document.
        let all = admin
            .list_graphs(pb::ListGraphsRequest {
                namespace: "acme".to_string(),
                ..Default::default()
            })
            .await
            .expect("list");
        let summary: Vec<(String, u64, u32)> = all
            .graphs
            .iter()
            .map(|g| (g.name.clone(), g.version, g.replicas))
            .collect();
        assert_eq!(
            summary,
            [
                ("g2".to_string(), 2, 2),
                ("g3".to_string(), 2, 3),
                ("g4".to_string(), 2, 4),
                ("g5".to_string(), 2, 5),
                ("n0".to_string(), 1, 0),
                ("n1".to_string(), 1, 0),
                ("n2".to_string(), 1, 0),
                ("n3".to_string(), 1, 0),
            ]
        );
        let deleting = admin
            .catalog()
            .deleting("acme", u64::MAX)
            .await
            .expect("deleting");
        let mut deleted: Vec<_> = deleting.iter().map(|g| g.name.clone()).collect();
        deleted.sort();
        assert_eq!(deleted, ["g0", "g1"]);
        assert!(
            admin
                .catalog()
                .counters()
                .cas_retries
                .load(std::sync::atomic::Ordering::Relaxed)
                > 0,
            "the writers really did race"
        );
    }

    /// Review I1: a superseded document is never deleted at once; the sweep keeps the pointer's
    /// target and anything inside the grace window, and a reader whose document was swept
    /// between its pointer read and its GET follows the pointer again.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn superseded_documents_stay_readable_and_are_swept_after_grace() {
        let fixture = Fixture::start().await;
        let admin = fixture.admin();
        for name in ["a", "b", "c"] {
            admin
                .create_graph(create("acme", name, name))
                .await
                .expect("create");
        }
        let documents = || async {
            fixture
                .store
                .list("graphs/")
                .await
                .expect("list")
                .into_iter()
                .filter(|o| o.path.contains("/catalog/"))
                .count()
        };
        assert_eq!(documents().await, 3, "every superseded document is kept");
        assert_eq!(
            admin
                .sweep_documents(Duration::from_secs(600))
                .await
                .expect("sweep"),
            0,
            "all inside the grace window"
        );
        assert_eq!(
            admin.sweep_documents(Duration::ZERO).await.expect("sweep"),
            2
        );
        assert_eq!(documents().await, 1, "the pointer's target is kept");
        admin
            .get_graph(get("acme", "c"))
            .await
            .expect("still readable");

        // A reader reads the pointer, then its GET is held for 300 ms; meanwhile a writer moves
        // the pointer and a sweep with no grace deletes the document the reader was about to
        // read. The reader follows the pointer and answers.
        fixture
            .faulty
            .inject(Op::Get, Fault::Delay(Duration::from_millis(300)));
        // A fresh admin, so the reader has no cached document (review I4) and must GET.
        let reader_admin = fixture.admin();
        let reader = {
            let admin = reader_admin.clone();
            tokio::spawn(async move { admin.get_graph(get("acme", "a")).await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        admin
            .create_graph(create("acme", "d", "d"))
            .await
            .expect("a writer");
        admin.sweep_documents(Duration::ZERO).await.expect("sweep");
        reader
            .await
            .expect("task")
            .expect("the reader re-read the pointer instead of failing");
        assert!(
            reader_admin
                .catalog()
                .counters()
                .document_rereads
                .load(std::sync::atomic::Ordering::Relaxed)
                > 0,
            "the reader really did hit a swept document"
        );
    }

    /// A hook that, once, lets `intrude` commit on top of the write that just committed, and
    /// then reports that write's acknowledgement as lost (review I2).
    fn ack_lost_once<F, Fut>(intrude: F) -> loams_graph::catalog::AckHook
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let intrude = Arc::new(intrude);
        Arc::new(move || {
            let fired = fired.clone();
            let intrude = intrude.clone();
            Box::pin(async move {
                if fired.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    return false;
                }
                intrude().await;
                true
            })
        })
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn lost_ack_with_a_writer_on_top_is_idempotent() {
        use loams_graph::catalog::{CatalogState, GraphCatalog, NewGraph};
        let fixture = Fixture::start().await;
        let catalog = GraphCatalog::new(fixture.meta.clone(), fixture.store.clone());
        let intruder = GraphCatalog::new(fixture.meta.clone(), fixture.store.clone());

        // create: the retry finds its own graph id, not an "already exists".
        let other = intruder.clone();
        catalog.set_ack_hook(Some(ack_lost_once(move || {
            let other = other.clone();
            async move {
                other
                    .create("acme", "intruder", NewGraph::default())
                    .await
                    .expect("the other writer");
            }
        })));
        let created = catalog
            .create("acme", "kg", NewGraph::default())
            .await
            .expect("a lost ack is not an error");
        let page = catalog.list("acme", 0, "").await.expect("list");
        let names: Vec<_> = page.graphs.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["intruder", "kg"]);
        assert_eq!(
            page.graphs[1].id, created.id,
            "exactly one kg, the one this call made"
        );

        // update with expected_version: applied once, under the other writer's later change.
        let other = intruder.clone();
        catalog.set_ack_hook(Some(ack_lost_once(move || {
            let other = other.clone();
            async move {
                other
                    .update("acme", "kg", None, |g| {
                        g.replicas = 5;
                        Ok(())
                    })
                    .await
                    .expect("the other writer");
            }
        })));
        let updated = catalog
            .update("acme", "kg", Some(created.version), |g| {
                g.replicas = 2;
                Ok(())
            })
            .await
            .expect("a lost ack is not a version mismatch");
        // The answer is the record as it now stands (this write applied, the later one on top).
        assert_eq!(updated.version, created.version + 2);
        let now = catalog.get_by_name("acme", "kg").await.expect("get");
        assert_eq!(now.version, created.version + 2, "two writes, not three");
        assert_eq!(
            now.replicas, 5,
            "the later writer's value stands; the retry did not reapply"
        );

        // mark_deleting without a key: deleted once; the retry does not answer "not found".
        let other = intruder.clone();
        catalog.set_ack_hook(Some(ack_lost_once(move || {
            let other = other.clone();
            async move {
                other
                    .create("acme", "late", NewGraph::default())
                    .await
                    .expect("the other writer");
            }
        })));
        let deleted = catalog
            .mark_deleting("acme", "kg", None)
            .await
            .expect("a lost ack is not a not-found");
        assert!(matches!(deleted.state, CatalogState::Deleting { .. }));
        let gone = catalog.deleting("acme", u64::MAX).await.expect("deleting");
        assert_eq!(gone.len(), 1, "one deleting record: {gone:?}");
        let page = catalog.list("acme", 0, "").await.expect("list");
        let names: Vec<_> = page.graphs.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["intruder", "late"]);
    }

    /// Review I4: a namespace document is fetched again only when its pointer moves, and an open
    /// graph a recent catalog read vouched for runs statements without reading the catalog.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn catalog_reads_are_cached() {
        use std::sync::atomic::Ordering::Relaxed;
        let fixture = Fixture::start().await;
        let admin = fixture.admin();
        admin
            .create_graph(create("acme", "kg", "k"))
            .await
            .expect("create");
        let counters = admin.catalog().counters();

        admin.get_graph(get("acme", "kg")).await.expect("get");
        let gets = counters.document_gets.load(Relaxed);
        admin.get_graph(get("acme", "kg")).await.expect("get");
        assert_eq!(counters.document_gets.load(Relaxed), gets, "cached");

        admin
            .execute(execute("acme", "kg", "RETURN 1 AS x"))
            .await
            .expect("first statement");
        let loads = counters.loads.load(Relaxed);
        for _ in 0..5 {
            admin
                .execute(execute("acme", "kg", "RETURN 1 AS x"))
                .await
                .expect("statement");
        }
        assert_eq!(
            counters.loads.load(Relaxed),
            loads,
            "no catalog read for a vouched-for graph"
        );

        // A write moves the pointer: the next read fetches the new document.
        admin
            .create_graph(create("acme", "other", "o"))
            .await
            .expect("create");
        let gets = counters.document_gets.load(Relaxed);
        admin.get_graph(get("acme", "other")).await.expect("get");
        assert!(counters.document_gets.load(Relaxed) <= gets + 1);

        // A delete is seen at once on this node.
        admin
            .delete_graph(pb::DeleteGraphRequest {
                namespace: "acme".to_string(),
                name: "kg".to_string(),
                ..Default::default()
            })
            .await
            .expect("delete");
        let err = admin
            .execute(execute("acme", "kg", "RETURN 1 AS x"))
            .await
            .expect_err("deleted");
        assert_eq!(err.code, ErrorCode::NotFound);
    }

    /// Review M1: a delete that lands between the catalog read and the engine open is caught by
    /// the re-check after the open: the graph is closed again and the statement answers
    /// NOT_FOUND.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_delete_during_a_lazy_open_is_caught() {
        let fixture = Fixture::start().await;
        let admin = fixture.admin();
        admin
            .create_graph(create("acme", "kg", "k"))
            .await
            .expect("create");
        let other = Arc::new(fixture.admin());
        admin.set_after_open_hook(Some(ack_lost_once(move || {
            let other = other.clone();
            async move {
                other
                    .delete_graph(pb::DeleteGraphRequest {
                        namespace: "acme".to_string(),
                        name: "kg".to_string(),
                        ..Default::default()
                    })
                    .await
                    .expect("the concurrent delete");
            }
        })));
        let err = admin
            .execute(execute("acme", "kg", "RETURN 1 AS x"))
            .await
            .expect_err("deleted while opening");
        assert_eq!(err.code, ErrorCode::NotFound, "{err:?}");
        assert!(
            admin.engine().list(Some("acme")).expect("list").is_empty(),
            "the graph opened for the statement was closed again"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn invalid_names_refused() {
        let fixture = Fixture::start().await;
        let admin = fixture.admin();
        for (ns, name) in [
            ("acme", "Kg"),
            ("acme", "1kg"),
            ("acme", "k/g"),
            ("a/b", "kg"),
            ("", "kg"),
            ("acme", ""),
        ] {
            let err = admin
                .create_graph(create(ns, name, "k"))
                .await
                .expect_err("invalid");
            assert_eq!(
                err.code,
                ErrorCode::InvalidArgument,
                "{ns:?}/{name:?}: {err:?}"
            );
            let err = admin.get_graph(get(ns, name)).await.expect_err("invalid");
            assert_eq!(err.code, ErrorCode::InvalidArgument, "{ns:?}/{name:?}");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn get_schema_reports_labels_types_counts() {
        let fixture = Fixture::start().await;
        let admin = fixture.admin();
        admin
            .create_graph(create("acme", "kg", "k"))
            .await
            .expect("create");
        admin
            .execute(execute(
                "acme",
                "kg",
                "INSERT (:Person {name: 'a'})-[:KNOWS {since: 1}]->(:Person {name: 'b'})-[:KNOWS]->(:City {name: 'c'})",
            ))
            .await
            .expect("write");
        let schema = admin
            .get_schema(pb::GetSchemaRequest {
                namespace: "acme".to_string(),
                name: "kg".to_string(),
                ..Default::default()
            })
            .await
            .expect("schema");
        let labels: Vec<(String, u64)> = schema
            .labels
            .iter()
            .map(|l| (l.label.clone(), l.count))
            .collect();
        assert_eq!(labels, [("City".to_string(), 1), ("Person".to_string(), 2)]);
        let types: Vec<(String, u64)> = schema
            .edge_types
            .iter()
            .map(|t| (t.r#type.clone(), t.count))
            .collect();
        assert_eq!(types, [("KNOWS".to_string(), 2)]);
        assert_eq!(schema.property_keys, ["name", "since"]);
        let err = admin
            .get_schema(pb::GetSchemaRequest {
                namespace: "acme".to_string(),
                name: "nope".to_string(),
                ..Default::default()
            })
            .await
            .expect_err("missing");
        assert_eq!(err.code, ErrorCode::NotFound);
    }
}
