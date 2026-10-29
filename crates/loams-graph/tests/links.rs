//! LINKED graphs (GR1 Tasks 17 and 18; design §07 §2.1, §48 §10.1; D743): a graph declared over
//! collections by a mapping, checked against their schemas when it is created, read-only to GQL,
//! and written by its `collection → graph` links, exactly once.

mod common;

use std::sync::Arc;
use std::time::Duration;

use connectrpc::{ConnectError, ErrorCode};
use loams_common::meta::MetaStore;
use loams_common::schema::{CollectionSchema, DynamicMapping, FieldKind, FieldSpec};
use loams_graph::Engine;
use loams_graph::catalog::GraphCatalog;
use loams_graph::mapping::{EdgeMap, Endpoint, GraphMapping, PRIMARY_KEY, VertexMap};
use loams_graph::service::admin::GraphAdmin;
use loams_meta::{MetaClient, MetaClientConfig, MetaConfig, MetaNode, Router, SystemClock};
use loams_proto::loams::errors::v1::ErrorInfo;
use loams_proto::loams::graph::v1 as pb;
use loams_store::Store;
use tempfile::TempDir;

/// A metastore with namespace `acme` and its collections, and an admin over it.
struct Fixture {
    _node: MetaNode,
    _meta_dir: TempDir,
    _data_dir: TempDir,
    admin: GraphAdmin,
}

fn field(name: &str, kind: FieldKind) -> FieldSpec {
    FieldSpec {
        name: name.to_string(),
        source_path: name.to_string(),
        // A text field cannot be fast.
        fast: !matches!(kind, FieldKind::Text { .. }),
        kind,
        indexed: true,
        ignore_malformed: false,
    }
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
        let meta: Arc<dyn MetaStore> = Arc::new(MetaClient::new(
            node.clone(),
            vec![],
            clock,
            MetaClientConfig::default(),
        ));
        let namespace = meta.create_namespace("acme").await.expect("namespace");
        for (name, fields) in [
            (
                "entities",
                vec![
                    field("name", FieldKind::Keyword),
                    field("entity_type", FieldKind::Keyword),
                ],
            ),
            (
                "chunks",
                vec![field(
                    "text",
                    FieldKind::Text {
                        analyzer: "standard".to_string(),
                        positions: true,
                    },
                )],
            ),
            (
                "relations",
                vec![
                    field("src", FieldKind::Keyword),
                    field("dst", FieldKind::Keyword),
                    field("rel_type", FieldKind::Keyword),
                    field("weight", FieldKind::F64),
                ],
            ),
            (
                "mentions",
                vec![
                    field("chunk_id", FieldKind::Keyword),
                    field("entity_id", FieldKind::Keyword),
                ],
            ),
        ] {
            meta.create_collection(
                namespace,
                name,
                CollectionSchema::new(fields, Vec::new(), DynamicMapping::Ignore),
                1,
            )
            .await
            .expect("collection");
        }
        let data_dir = TempDir::new().expect("data dir");
        let admin = GraphAdmin::new(
            Arc::new(Engine::with_data_dir(data_dir.path())),
            GraphCatalog::new(meta, Store::in_memory()),
        );
        Self {
            _node: node,
            _meta_dir: meta_dir,
            _data_dir: data_dir,
            admin,
        }
    }
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
    ErrorInfo::decode_from_slice(&bytes)
        .expect("an ErrorInfo")
        .reason
}

/// §07 §2.1's example graph.
fn kg() -> GraphMapping {
    let endpoint = |label: &str, field: &str| Endpoint {
        label: label.to_string(),
        field: field.to_string(),
    };
    GraphMapping {
        streams: Vec::new(),
        vertices: vec![
            VertexMap {
                label: "entity".to_string(),
                collection: "entities".to_string(),
                key_field: PRIMARY_KEY.to_string(),
            },
            VertexMap {
                label: "chunk".to_string(),
                collection: "chunks".to_string(),
                key_field: PRIMARY_KEY.to_string(),
            },
        ],
        edges: vec![
            EdgeMap {
                name: "RELATED".to_string(),
                type_field: Some("rel_type".to_string()),
                collection: "relations".to_string(),
                source: endpoint("entity", "src"),
                destination: endpoint("entity", "dst"),
            },
            EdgeMap {
                name: "MENTIONS".to_string(),
                type_field: None,
                collection: "mentions".to_string(),
                source: endpoint("chunk", "chunk_id"),
                destination: endpoint("entity", "entity_id"),
            },
        ],
    }
}

fn linked(name: &str, mapping: &GraphMapping) -> pb::CreateGraphRequest {
    pb::CreateGraphRequest {
        namespace: "acme".to_string(),
        name: name.to_string(),
        mode: pb::GraphMode::Linked.into(),
        mapping: mapping.to_proto().into(),
        ..Default::default()
    }
}

async fn refused(fixture: &Fixture, name: &str, mapping: &GraphMapping) -> ConnectError {
    fixture
        .admin
        .create_graph(linked(name, mapping))
        .await
        .expect_err("an invalid mapping")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mapping_validates_against_schema() {
    let fixture = Fixture::start().await;
    let created = fixture
        .admin
        .create_graph(linked("kg", &kg()))
        .await
        .expect("a valid mapping");
    assert_eq!(created.mode.as_known(), Some(pb::GraphMode::Linked));
    let got = fixture
        .admin
        .get_graph(pb::GetGraphRequest {
            namespace: "acme".to_string(),
            name: "kg".to_string(),
            ..Default::default()
        })
        .await
        .expect("get");
    assert_eq!(
        got.mapping.as_option().map(GraphMapping::from_proto),
        Some(kg()),
        "the catalog keeps the mapping"
    );

    // Each mistake against the schemas is INVALID_ARGUMENT, and creates nothing.
    let cases: Vec<(&str, GraphMapping)> = vec![
        ("no such collection", {
            let mut m = kg();
            m.vertices[1].collection = "pages".to_string();
            m
        }),
        ("no such key field", {
            let mut m = kg();
            m.vertices[0].key_field = "id".to_string();
            m
        }),
        ("a text key", {
            let mut m = kg();
            m.vertices[1].key_field = "text".to_string();
            m
        }),
        ("a float endpoint", {
            let mut m = kg();
            m.edges[0].destination.field = "weight".to_string();
            m
        }),
        ("a float type field", {
            let mut m = kg();
            m.edges[0].type_field = Some("weight".to_string());
            m
        }),
        ("an undeclared endpoint label", {
            let mut m = kg();
            m.edges[1].source.label = "document".to_string();
            m
        }),
    ];
    for (n, (what, mapping)) in cases.iter().enumerate() {
        let err = refused(&fixture, &format!("bad{n}"), mapping).await;
        assert_eq!(err.code, ErrorCode::InvalidArgument, "{what}: {err:?}");
        assert_eq!(reason(&err), "invalid_argument", "{what}");
    }

    // A mode the wire does not name is refused, not read as OWNED.
    let err = fixture
        .admin
        .create_graph(pb::CreateGraphRequest {
            mode: buffa::EnumValue::from(42),
            ..linked("unknown", &kg())
        })
        .await
        .expect_err("an unknown mode");
    assert_eq!(err.code, ErrorCode::InvalidArgument);

    // A LINKED graph needs a mapping; an OWNED one may not have one.
    let err = fixture
        .admin
        .create_graph(pb::CreateGraphRequest {
            namespace: "acme".to_string(),
            name: "nomap".to_string(),
            mode: pb::GraphMode::Linked.into(),
            ..Default::default()
        })
        .await
        .expect_err("no mapping");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let err = fixture
        .admin
        .create_graph(pb::CreateGraphRequest {
            mode: pb::GraphMode::Owned.into(),
            ..linked("owned", &kg())
        })
        .await
        .expect_err("an OWNED graph with a mapping");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let listed = fixture
        .admin
        .list_graphs(pb::ListGraphsRequest {
            namespace: "acme".to_string(),
            ..Default::default()
        })
        .await
        .expect("list");
    assert_eq!(
        listed
            .graphs
            .iter()
            .map(|g| g.name.as_str())
            .collect::<Vec<_>>(),
        vec!["kg"]
    );
}

/// One edge collection holds many relation types: `TYPE FROM (rel_type)` keeps the field, and
/// the declared name stays the mapping's name; without it, the name is every edge's type.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn type_from_column_maps_many_types() {
    let fixture = Fixture::start().await;
    let created = fixture
        .admin
        .create_graph(linked("kg", &kg()))
        .await
        .expect("create");
    let mapping = GraphMapping::from_proto(created.mapping.as_option().expect("a mapping"));
    let related = &mapping.edges[0];
    assert_eq!(related.name, "RELATED");
    assert_eq!(related.type_field.as_deref(), Some("rel_type"));
    let mentions = &mapping.edges[1];
    assert_eq!(mentions.name, "MENTIONS");
    assert_eq!(mentions.type_field, None);
    // On the wire, an absent TYPE FROM is the empty field.
    let wire = mapping.to_proto();
    assert_eq!(wire.edges[0].type_field, "rel_type");
    assert_eq!(wire.edges[1].type_field, "");
    // A replay of the create with the same key and mapping answers the same graph; another
    // mapping under that key is refused.
    let keyed = |mapping: &GraphMapping| pb::CreateGraphRequest {
        idempotency_key: "k1".to_string(),
        ..linked("keyed", mapping)
    };
    let first = fixture
        .admin
        .create_graph(keyed(&kg()))
        .await
        .expect("create");
    let again = fixture
        .admin
        .create_graph(keyed(&kg()))
        .await
        .expect("replay");
    assert_eq!(first.id, again.id);
    let mut other = kg();
    other.edges[0].type_field = None;
    let err = fixture
        .admin
        .create_graph(keyed(&other))
        .await
        .expect_err("another mapping under the key");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

/// GQL may only read a LINKED graph: its links write it (D743).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn linked_graph_refuses_gql_writes() {
    let fixture = Fixture::start().await;
    fixture
        .admin
        .create_graph(linked("kg", &kg()))
        .await
        .expect("create");
    let statement = |text: &str| pb::ExecuteRequest {
        namespace: "acme".to_string(),
        graph: "kg".to_string(),
        statement: text.to_string(),
        ..Default::default()
    };
    for write in [
        "INSERT (:entity {name: 'x'})",
        "MATCH (n) SET n.name = 'y'",
        "MATCH (n) DETACH DELETE n",
    ] {
        let err = fixture
            .admin
            .execute(statement(write))
            .await
            .expect_err("a write on a LINKED graph");
        assert_eq!(err.code, ErrorCode::FailedPrecondition, "{write}: {err:?}");
        assert_eq!(reason(&err), "graph_is_linked", "{write}");
    }
    let batch = fixture
        .admin
        .execute_batch(pb::ExecuteBatchRequest {
            namespace: "acme".to_string(),
            graph: "kg".to_string(),
            statements: vec![pb::Statement {
                statement: "INSERT (:entity {name: 'x'})".to_string(),
                ..Default::default()
            }],
            atomic: true,
            ..Default::default()
        })
        .await
        .expect_err("an atomic batch that writes");
    assert_eq!(reason(&batch), "graph_is_linked");
    // Reads are served.
    let read = fixture
        .admin
        .execute(statement("MATCH (n) RETURN count(n) AS n"))
        .await
        .expect("a read");
    assert_eq!(read.rows.as_option().expect("rows").rows.len(), 1);
}

// ---------------------------------------------------------------------------------------------
// GR1 Task 18: the `collection → graph` link
// ---------------------------------------------------------------------------------------------

mod collection_link {
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use loams_collection::{DocOp, Document, PatchMode, PrimaryKey, encode, partition_of};
    use loams_common::meta::LinkId;
    use loams_common::schema::{CollectionSchema, DynamicMapping, FieldKind};
    use loams_common::{NamespaceId, StreamId};
    use loams_graph::changeset::{GraphRecord, decode};
    use loams_graph::durable::GraphLog;
    use loams_graph::durable::faults::{KillHook, KillPoint};
    use loams_graph::link::{LinkRunner, collection_link_id};
    use loams_graph::mapping::{EdgeMap, Endpoint, GraphMapping, PRIMARY_KEY, VertexMap};
    use loams_link::LinkConfig;
    use loams_log::LogWriter;
    use loams_proto::loams::graph::v1 as pb;
    use serde_json::{Map, Value as Json, json};
    use tokio::sync::watch;

    use super::common::{Cluster, Deadline, SHORT, runtime, wait_out_the_lease};
    use super::field;

    /// The graph the tests link: entities, and relations between them typed by `rel_type`.
    fn mapping() -> GraphMapping {
        GraphMapping {
            streams: Vec::new(),
            vertices: vec![VertexMap {
                label: "entity".to_string(),
                collection: "entities".to_string(),
                key_field: PRIMARY_KEY.to_string(),
            }],
            edges: vec![EdgeMap {
                name: "RELATED".to_string(),
                type_field: Some("rel_type".to_string()),
                collection: "relations".to_string(),
                source: Endpoint {
                    label: "entity".to_string(),
                    field: "src".to_string(),
                },
                destination: Endpoint {
                    label: "entity".to_string(),
                    field: "dst".to_string(),
                },
            }],
        }
    }

    fn config() -> LinkConfig {
        LinkConfig {
            batch_records: 4,
            batch_interval: Duration::ZERO,
            ..LinkConfig::default()
        }
    }

    /// A cluster with namespace `acme`, collections `entities` (two partitions) and
    /// `relations`, a writer of their implicit streams, the LINKED graph `kg` over them, and the
    /// documents written so far: the model the graph must equal.
    struct Linked {
        cluster: Cluster,
        writer: LogWriter,
        streams: BTreeMap<String, (StreamId, u32)>,
        docs: Mutex<BTreeMap<(String, String), Map<String, Json>>>,
        ns: NamespaceId,
    }

    static WRITER_NODE: AtomicU64 = AtomicU64::new(900);

    impl Linked {
        async fn start(nodes: u64) -> Self {
            let cluster = Cluster::start(nodes, SHORT).await;
            let ns = cluster
                .meta
                .create_namespace("acme")
                .await
                .expect("namespace");
            let mut streams = BTreeMap::new();
            for (name, fields, partitions) in [
                (
                    "entities",
                    vec![
                        field("name", FieldKind::Keyword),
                        field("rank", FieldKind::I64),
                    ],
                    2,
                ),
                (
                    "relations",
                    vec![
                        field("src", FieldKind::Keyword),
                        field("dst", FieldKind::Keyword),
                        field("rel_type", FieldKind::Keyword),
                        field("weight", FieldKind::F64),
                    ],
                    1,
                ),
            ] {
                let (_, stream, _) = cluster
                    .meta
                    .create_collection(
                        ns,
                        name,
                        CollectionSchema::new(fields, Vec::new(), DynamicMapping::Ignore),
                        partitions,
                    )
                    .await
                    .expect("collection");
                streams.insert(name.to_string(), (stream, partitions));
            }
            let writer = LogWriter::start(
                cluster.meta.clone(),
                cluster.store.clone(),
                GraphLog::log_config(WRITER_NODE.fetch_add(1, Ordering::SeqCst)),
            )
            .expect("writer");
            cluster
                .node(1)
                .admin
                .create_graph(pb::CreateGraphRequest {
                    namespace: "acme".to_string(),
                    name: "kg".to_string(),
                    mode: pb::GraphMode::Linked.into(),
                    mapping: mapping().to_proto().into(),
                    ..Default::default()
                })
                .await
                .expect("create");
            Self {
                cluster,
                writer,
                streams,
                docs: Mutex::default(),
                ns,
            }
        }

        async fn append(&self, collection: &str, op: &DocOp) {
            let (stream, partitions) = self.streams[collection];
            let record = encode(op).expect("encode");
            self.writer
                .append(stream, partition_of(op.pk(), partitions), vec![record])
                .await
                .expect("append");
        }

        async fn upsert(&self, collection: &str, pk: &str, source: Json) {
            let source = source.as_object().expect("an object").clone();
            let op = DocOp::Upsert(Document {
                pk: PrimaryKey::Str(pk.to_string()),
                source: source.clone(),
                vectors: BTreeMap::new(),
                sparse_vectors: BTreeMap::new(),
            });
            self.append(collection, &op).await;
            self.docs
                .lock()
                .expect("lock")
                .insert((collection.to_string(), pk.to_string()), source);
        }

        /// A `MergeTop` patch, which changes nothing when the document is absent.
        async fn patch(&self, collection: &str, pk: &str, source: Json) {
            let source = source.as_object().expect("an object").clone();
            let op = DocOp::Patch {
                pk: PrimaryKey::Str(pk.to_string()),
                mode: PatchMode::MergeTop,
                source: source.clone(),
                delete_keys: Vec::new(),
                vectors: BTreeMap::new(),
                sparse_vectors: BTreeMap::new(),
                upsert: None,
            };
            self.append(collection, &op).await;
            if let Some(doc) = self
                .docs
                .lock()
                .expect("lock")
                .get_mut(&(collection.to_string(), pk.to_string()))
            {
                doc.extend(source);
            }
        }

        async fn delete(&self, collection: &str, pk: &str) {
            self.append(collection, &DocOp::Delete(PrimaryKey::Str(pk.to_string())))
                .await;
            self.docs
                .lock()
                .expect("lock")
                .remove(&(collection.to_string(), pk.to_string()));
        }

        fn runner(&self, node: u64) -> LinkRunner {
            LinkRunner::new(self.cluster.node(node).admin.clone(), config())
        }

        /// Applies every link of `kg` on `node` until none lags.
        async fn catch_up(&self, node: u64) {
            let runner = self.runner(node);
            for _ in 0..200 {
                runner.apply_graph("acme", "kg").await.expect("apply");
                let lag = runner.lag("acme", "kg").await.expect("lag");
                if lag.iter().all(|l| l.records == 0) {
                    return;
                }
            }
            panic!("the links never caught up");
        }

        /// The graph `node` serves, without element ids.
        fn graph(&self, node: u64) -> Canon {
            let graph = self
                .cluster
                .node(node)
                .admin
                .engine()
                .list(Some("acme"))
                .expect("list")
                .into_iter()
                .find(|g| g.name() == "kg")
                .expect("kg is open");
            Canon::of(&graph.rows().expect("rows"))
        }

        /// The mapping applied to the documents written (§48 §10.1).
        fn expected(&self) -> Canon {
            let docs = self.docs.lock().expect("lock");
            let mut canon = Canon::default();
            for ((collection, pk), source) in docs.iter() {
                if collection != "entities" {
                    continue;
                }
                let mut props = props(source);
                props.insert("_collection".to_string(), text("entities"));
                props.insert("_key".to_string(), text(pk));
                canon
                    .vertices
                    .insert(("entity".to_string(), pk.clone()), (false, props));
            }
            for ((collection, pk), source) in docs.iter() {
                if collection != "relations" {
                    continue;
                }
                let end = |field: &str| source[field].as_str().expect("a key").to_string();
                let (src, dst) = (end("src"), end("dst"));
                for key in [&src, &dst] {
                    canon
                        .vertices
                        .entry(("entity".to_string(), key.clone()))
                        .or_insert_with(|| {
                            (
                                true,
                                BTreeMap::from([
                                    ("_key".to_string(), text(key)),
                                    ("_stub".to_string(), "Bool(true)".to_string()),
                                ]),
                            )
                        });
                }
                let mut props = props(source);
                props.insert("_collection".to_string(), text("relations"));
                props.insert("_key".to_string(), text(pk));
                props.insert("_edge".to_string(), text("RELATED"));
                canon.edges.push((
                    source["rel_type"].as_str().expect("a type").to_string(),
                    ("entity".to_string(), src),
                    ("entity".to_string(), dst),
                    props,
                ));
            }
            canon.edges.sort();
            canon
        }

        /// Every link record of `kg`'s stream: `(link, version)` in log order.
        async fn link_versions(&self) -> Vec<(LinkId, u64)> {
            let stream = self.cluster.stream_of("kg").await;
            self.cluster
                .records(stream)
                .await
                .iter()
                .filter_map(
                    |r| match decode(r.record.value.as_deref().expect("value")) {
                        Ok(GraphRecord::ChangeSet(change)) => {
                            change.link.map(|link| (link.link, link.version))
                        }
                        _ => None,
                    },
                )
                .collect()
        }

        /// Each link's versions are 1, 2, 3, … in the log: no commit applied twice or lost.
        async fn assert_versions_consecutive(&self, case: &str) {
            let mut next: BTreeMap<LinkId, u64> = BTreeMap::new();
            for (link, version) in self.link_versions().await {
                let expected = next.entry(link).or_insert(1);
                assert_eq!(version, *expected, "{case}: link {link:?}");
                *expected += 1;
            }
        }
    }

    fn text(s: &str) -> String {
        format!("{:?}", grafeo::Value::String(s.into()))
    }

    fn props(source: &Map<String, Json>) -> BTreeMap<String, String> {
        source
            .iter()
            .map(|(k, v)| {
                let value = match v {
                    Json::String(s) => grafeo::Value::String(s.as_str().into()),
                    Json::Number(n) => match n.as_i64() {
                        Some(i) => grafeo::Value::Int64(i),
                        None => grafeo::Value::Float64(n.as_f64().expect("a number")),
                    },
                    Json::Bool(b) => grafeo::Value::Bool(*b),
                    other => panic!("the tests write no {other}"),
                };
                (k.clone(), format!("{value:?}"))
            })
            .collect()
    }

    /// A graph without element ids: vertices by `(label, _key)` with whether they are stubs and
    /// their properties, and edges by type and endpoints.
    #[derive(Debug, Default, PartialEq)]
    struct Canon {
        vertices: BTreeMap<(String, String), (bool, BTreeMap<String, String>)>,
        #[allow(clippy::type_complexity)]
        edges: Vec<(
            String,
            (String, String),
            (String, String),
            BTreeMap<String, String>,
        )>,
    }

    impl Canon {
        fn of(rows: &loams_graph::snapshot::portable::GraphRows) -> Self {
            let key = |props: &loams_graph::changeset::PropMap| match props.get("_key") {
                Some(grafeo::Value::String(s)) => s.to_string(),
                other => panic!("a link element without a key: {other:?}"),
            };
            let mut ends = BTreeMap::new();
            let mut canon = Self::default();
            for node in &rows.nodes {
                let label = node.labels.first().expect("a label").clone();
                let k = key(&node.props);
                ends.insert(node.id, (label.clone(), k.clone()));
                let stub = matches!(node.props.get("_stub"), Some(grafeo::Value::Bool(true)));
                let props = node
                    .props
                    .iter()
                    .map(|(k, v)| (k.clone(), format!("{v:?}")))
                    .collect();
                let previous = canon.vertices.insert((label, k), (stub, props));
                assert!(previous.is_none(), "two vertices of one key");
            }
            for edge in &rows.edges {
                canon.edges.push((
                    edge.ty.clone(),
                    ends[&edge.src].clone(),
                    ends[&edge.dst].clone(),
                    edge.props
                        .iter()
                        .map(|(k, v)| (k.clone(), format!("{v:?}")))
                        .collect(),
                ));
            }
            canon.edges.sort();
            canon
        }
    }

    /// A workload that exercises every op: upserts, patches, deletes, edges before their
    /// endpoints. An entity is deleted only after the relations that name it.
    async fn workload(linked: &Linked, round: u64) {
        let e = |i: u64| format!("e{}", (i + round * 3) % 9);
        for i in 0..6 {
            linked
                .upsert(
                    "entities",
                    &e(i),
                    json!({"name": format!("E{i}r{round}"), "rank": i}),
                )
                .await;
            linked
                .upsert(
                    "relations",
                    &format!("r{round}-{i}"),
                    json!({"src": e(i), "dst": e(i + 4), "rel_type": if i % 2 == 0 { "KNOWS" } else { "LIKES" }, "weight": 0.5}),
                )
                .await;
        }
        linked
            .patch(
                "entities",
                &e(1),
                json!({"name": format!("patched{round}")}),
            )
            .await;
        linked
            .patch(
                "relations",
                &format!("r{round}-2"),
                json!({"rel_type": "WORKS_WITH"}),
            )
            .await;
        linked.delete("relations", &format!("r{round}-3")).await;
        // An entity no relation names any more, deleted.
        let named: Vec<String> = linked
            .docs
            .lock()
            .expect("lock")
            .iter()
            .filter(|((c, _), _)| c == "relations")
            .flat_map(|(_, s)| {
                [
                    s["src"].as_str().map(str::to_string),
                    s["dst"].as_str().map(str::to_string),
                ]
            })
            .flatten()
            .collect();
        let lonely = (0..9).map(|i| format!("e{i}")).find(|k| {
            !named.contains(k)
                && linked
                    .docs
                    .lock()
                    .expect("lock")
                    .contains_key(&("entities".to_string(), k.clone()))
        });
        if let Some(lonely) = lonely {
            linked.delete("entities", &lonely).await;
        }
    }

    /// The write-path points the kill tests stop the owner at.
    pub(super) const KILL_POINTS: [KillPoint; 7] = [
        KillPoint::Begin,
        KillPoint::Executed,
        KillPoint::Committed,
        KillPoint::Captured,
        KillPoint::Enqueued,
        KillPoint::Appended,
        KillPoint::Published,
    ];

    /// How long a kill test (all seven cases) may run. It takes about 12 s, or a minute on a
    /// loaded CI runner.
    pub(super) const KILL_DEADLINE: Duration = Duration::from_secs(300);

    /// Kills the owner applying the links at `point`, during the second commit of a batch; the
    /// other node takes the graph and the links, and the graph equals the mapping applied to the
    /// collections, each commit in the log once (Review Focus 6).
    async fn kill_case(point: KillPoint, deadline: &Deadline) {
        let case = format!("{point:?}");
        let at = |phase: &str| deadline.phase(format!("{case}: {phase}"));
        at("starting the cluster");
        let linked = Linked::start(2).await;
        let graph = linked.cluster.graph_id("kg").await;
        let victim = linked.cluster.preferred(graph);
        let survivor = 3 - victim;
        at("the first workload");
        workload(&linked, 0).await;
        linked.catch_up(victim).await;
        assert_eq!(
            linked.graph(victim),
            linked.expected(),
            "{case}: before the kill"
        );
        workload(&linked, 1).await;

        let (reached_tx, mut reached) = watch::channel(false);
        let (release, released) = watch::channel(false);
        linked
            .cluster
            .node(victim)
            .log
            .set_kill_hook(Some(kill_at(point, 2, reached_tx, released)));
        at("waiting for the kill point");
        let runner = linked.runner(victim);
        let stuck = tokio::spawn(async move { runner.apply_graph("acme", "kg").await });
        tokio::time::timeout(Duration::from_secs(30), reached.wait_for(|r| *r))
            .await
            .unwrap_or_else(|_| panic!("{case}: the kill point was never reached"))
            .expect("the hook");
        // The node dies: it renews nothing and nobody reaches it.
        linked.cluster.node(victim).ownership.pause_renewals(true);
        linked.cluster.forwarder.cut_off(victim);
        linked
            .cluster
            .node(survivor)
            .membership
            .set(vec![linked.cluster.node(survivor).me.clone()]);
        at("waiting out the dead owner's lease");
        wait_out_the_lease(&linked.cluster, graph).await;
        at("the survivor catching up");

        linked.catch_up(survivor).await;
        assert_eq!(
            linked.graph(survivor),
            linked.expected(),
            "{case}: after the kill"
        );
        linked.assert_versions_consecutive(&case).await;

        // The dead node comes back with its commit in flight: it is fenced, and nothing changes.
        at("the dead node coming back");
        let _ = release.send(true);
        let _ = tokio::time::timeout(Duration::from_secs(30), stuck).await;
        linked.catch_up(survivor).await;
        assert_eq!(
            linked.graph(survivor),
            linked.expected(),
            "{case}: after the release"
        );
        linked.assert_versions_consecutive(&case).await;
    }

    fn kill_at(
        point: KillPoint,
        occurrence: u64,
        reached: watch::Sender<bool>,
        release: watch::Receiver<bool>,
    ) -> KillHook {
        let seen = std::sync::Arc::new(AtomicU64::new(0));
        let reached = std::sync::Arc::new(reached);
        std::sync::Arc::new(move |at| {
            let stop = at == point && seen.fetch_add(1, Ordering::SeqCst) + 1 == occurrence;
            let (reached, mut release) = (reached.clone(), release.clone());
            Box::pin(async move {
                if stop {
                    let _ = reached.send(true);
                    let _ = release.wait_for(|released| *released).await;
                }
            })
        })
    }

    /// The owner killed at each write-path point mid-batch: the other node takes over, and every
    /// record is applied once. Under a hard deadline, its runtime's shutdown included: a hang
    /// fails in minutes and names its phase (#1085).
    #[test]
    fn link_apply_exactly_once_under_kill() {
        let deadline = Deadline::start("link_apply_exactly_once_under_kill", KILL_DEADLINE);
        let runtime = runtime(4);
        runtime.block_on(async {
            for point in KILL_POINTS {
                kill_case(point, &deadline).await;
            }
        });
        deadline.phase("the runtime shutting down");
        drop(runtime);
    }

    /// An edge whose endpoint has no document yet makes a stub; the endpoint's upsert fills it,
    /// and a stub that loses its last edge goes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stub_vertex_filled_by_later_upsert() {
        let linked = Linked::start(1).await;
        linked
            .upsert(
                "relations",
                "r1",
                json!({"src": "a", "dst": "b", "rel_type": "KNOWS", "weight": 1.0}),
            )
            .await;
        linked.catch_up(1).await;
        let graph = linked.graph(1);
        for key in ["a", "b"] {
            let (stub, props) = &graph.vertices[&("entity".to_string(), key.to_string())];
            assert!(stub, "{key} is a stub");
            assert_eq!(props.len(), 2, "a stub has only _key and _stub: {props:?}");
        }
        assert_eq!(graph, linked.expected());

        linked
            .upsert("entities", "a", json!({"name": "Alice"}))
            .await;
        linked.catch_up(1).await;
        let graph = linked.graph(1);
        let (stub, props) = &graph.vertices[&("entity".to_string(), "a".to_string())];
        assert!(!stub, "the upsert filled the stub");
        assert_eq!(props["name"], text("Alice"));
        assert_eq!(graph.edges.len(), 1, "the edge stays on the filled vertex");
        assert_eq!(graph, linked.expected());

        linked.delete("relations", "r1").await;
        linked.catch_up(1).await;
        let graph = linked.graph(1);
        assert!(graph.edges.is_empty());
        assert!(
            !graph
                .vertices
                .contains_key(&("entity".to_string(), "b".to_string())),
            "a stub with no edge left goes"
        );
        assert_eq!(graph, linked.expected());
    }

    /// Deleting a vertex document deletes the vertex and its edges, in and out.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn vertex_delete_removes_incident_edges() {
        let linked = Linked::start(1).await;
        for key in ["a", "b", "c"] {
            linked.upsert("entities", key, json!({"name": key})).await;
        }
        for (pk, src, dst) in [("r1", "a", "b"), ("r2", "c", "a"), ("r3", "b", "c")] {
            linked
                .upsert(
                    "relations",
                    pk,
                    json!({"src": src, "dst": dst, "rel_type": "KNOWS"}),
                )
                .await;
        }
        linked.catch_up(1).await;
        assert_eq!(linked.graph(1).edges.len(), 3);
        linked.delete("entities", "a").await;
        linked.catch_up(1).await;
        let graph = linked.graph(1);
        assert!(
            !graph
                .vertices
                .contains_key(&("entity".to_string(), "a".to_string()))
        );
        assert_eq!(graph.vertices.len(), 2);
        assert_eq!(
            graph
                .edges
                .iter()
                .map(|(ty, s, d, _)| (ty.as_str(), s.1.as_str(), d.1.as_str()))
                .collect::<Vec<_>>(),
            vec![("KNOWS", "b", "c")],
            "only the edge that did not touch the vertex is left"
        );
    }

    /// Upserting a key again merges into the same vertex (its properties replaced, its edges
    /// kept); a patch is applied to the document the vertex holds.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pk_upsert_is_entity_merge() {
        let linked = Linked::start(1).await;
        linked
            .upsert("entities", "a", json!({"name": "A", "rank": 1}))
            .await;
        linked
            .upsert(
                "relations",
                "r1",
                json!({"src": "a", "dst": "a", "rel_type": "SELF"}),
            )
            .await;
        linked.catch_up(1).await;
        let first = linked.cluster.node(1).admin.clone();
        let id_of_a = || async {
            first
                .execute(pb::ExecuteRequest {
                    namespace: "acme".to_string(),
                    graph: "kg".to_string(),
                    statement: "MATCH (n:entity) RETURN id(n)".to_string(),
                    ..Default::default()
                })
                .await
                .expect("read")
                .rows
                .as_option()
                .expect("rows")
                .rows
                .len()
        };
        assert_eq!(id_of_a().await, 1);
        linked.upsert("entities", "a", json!({"name": "A2"})).await;
        linked.patch("entities", "a", json!({"rank": 7})).await;
        linked.catch_up(1).await;
        assert_eq!(id_of_a().await, 1, "one vertex per key");
        let graph = linked.graph(1);
        let (_, props) = &graph.vertices[&("entity".to_string(), "a".to_string())];
        assert_eq!(props["name"], text("A2"));
        assert_eq!(props["rank"], format!("{:?}", grafeo::Value::Int64(7)));
        assert_eq!(graph.edges.len(), 1, "the merged vertex keeps its edge");
        assert_eq!(graph, linked.expected());
    }

    /// The lag of each link is reported, falls to zero once applied, and a dropped collection
    /// is reported missing. A reopened graph keeps its links' progress: nothing is applied twice.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn link_lag_metric_reported() {
        let linked = Linked::start(1).await;
        let runner = linked.runner(1);
        for i in 0..5 {
            linked
                .upsert("entities", &format!("e{i}"), json!({"name": "x"}))
                .await;
        }
        let lag = runner.lag("acme", "kg").await.expect("lag");
        let by_name: BTreeMap<&str, u64> =
            lag.iter().map(|l| (l.source.as_str(), l.records)).collect();
        assert_eq!(by_name, BTreeMap::from([("entities", 5), ("relations", 0)]));
        linked.catch_up(1).await;
        let records = linked.link_versions().await.len();

        // Evicted and reopened from its stamped local files: the progress is there.
        assert!(
            linked
                .cluster
                .node(1)
                .admin
                .evict("acme", "kg")
                .await
                .expect("evict")
        );
        let lag = runner.lag("acme", "kg").await.expect("lag");
        assert!(lag.iter().all(|l| l.records == 0), "{lag:?}");
        runner.apply_graph("acme", "kg").await.expect("apply");
        assert_eq!(
            linked.link_versions().await.len(),
            records,
            "nothing applied twice"
        );
        assert_eq!(linked.graph(1), linked.expected());

        let entities = lag
            .iter()
            .find(|l| l.source == "entities")
            .expect("entities");
        assert_eq!(
            entities.link,
            collection_link_id(
                linked
                    .cluster
                    .meta
                    .resolve_collection(
                        loams_common::meta::Consistency::Linearizable,
                        linked.ns,
                        "entities"
                    )
                    .await
                    .expect("read")
                    .expect("entities")
                    .id
            )
        );
        linked
            .cluster
            .meta
            .drop_collection(linked.ns, "relations")
            .await
            .expect("drop");
        let lag = runner.lag("acme", "kg").await.expect("lag");
        assert!(
            lag.iter().any(|l| l.source == "relations" && l.missing),
            "{lag:?}"
        );
        runner
            .apply_graph("acme", "kg")
            .await
            .expect("a missing source stops only its link");
    }
}

// ---------------------------------------------------------------------------------------------
// GR1 Task 19: the `stream → graph` link
// ---------------------------------------------------------------------------------------------

mod stream_link {
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use bytes::Bytes;
    use loams_common::StreamId;
    use loams_common::meta::{LinkId, Retention, WalClass};
    use loams_graph::changeset::{GraphRecord, decode};
    use loams_graph::durable::GraphLog;
    use loams_graph::durable::faults::{KillHook, KillPoint};
    use loams_graph::link::LinkRunner;
    use loams_graph::mapping::{GraphMapping, StreamMap, StreamOp, StreamRule};
    use loams_link::LinkConfig;
    use loams_log::{LogWriter, Record};
    use loams_proto::loams::graph::v1 as pb;
    use serde_json::{Map, Value as Json, json};
    use tokio::sync::watch;

    use super::common::{Cluster, Deadline, SHORT, runtime, wait_out_the_lease};

    const PARTITIONS: u32 = 2;

    fn rule(name: &str, event_type: &str, op: StreamOp) -> StreamRule {
        StreamRule {
            name: name.to_string(),
            event_type: event_type.to_string(),
            op,
            label: String::new(),
            key_field: String::new(),
            source_label: String::new(),
            source_key_field: String::new(),
            destination_label: String::new(),
            destination_key_field: String::new(),
            edge_type: String::new(),
            edge_type_field: String::new(),
            edge_key_field: String::new(),
            properties_field: String::new(),
        }
    }

    /// People and who knows whom, from CloudEvents.
    fn mapping() -> GraphMapping {
        let node = |name: &str, ty: &str, op| StreamRule {
            label: "Person".to_string(),
            key_field: "data.id".to_string(),
            ..rule(name, ty, op)
        };
        let edge = |name: &str, ty: &str, op| StreamRule {
            source_label: "Person".to_string(),
            source_key_field: "data.from".to_string(),
            destination_label: "Person".to_string(),
            destination_key_field: "data.to".to_string(),
            edge_type: "KNOWS".to_string(),
            edge_key_field: "data.edge".to_string(),
            ..rule(name, ty, op)
        };
        GraphMapping {
            streams: vec![StreamMap {
                stream: "events".to_string(),
                rules: vec![
                    node("person", "person.upserted", StreamOp::UpsertNode),
                    node("person_gone", "person.deleted", StreamOp::DeleteNode),
                    edge("knows", "knows.added", StreamOp::UpsertEdge),
                    edge("knows_gone", "knows.removed", StreamOp::DeleteEdge),
                ],
            }],
            ..GraphMapping::default()
        }
    }

    /// What the graph must hold: people by id (stub or not, properties) and edges by key.
    #[derive(Debug, Default, PartialEq, Clone)]
    struct Model {
        nodes: BTreeMap<i64, (bool, BTreeMap<String, String>)>,
        edges: BTreeMap<i64, (i64, i64, BTreeMap<String, String>)>,
    }

    fn debug(v: grafeo::Value) -> String {
        format!("{v:?}")
    }

    fn json_props(data: &Map<String, Json>) -> BTreeMap<String, String> {
        data.iter()
            .map(|(k, v)| {
                let value = match v {
                    Json::String(s) => grafeo::Value::String(s.as_str().into()),
                    Json::Number(n) => grafeo::Value::Int64(n.as_i64().expect("an integer")),
                    other => panic!("the tests write no {other}"),
                };
                (k.clone(), debug(value))
            })
            .collect()
    }

    impl Model {
        fn stub(id: i64) -> (bool, BTreeMap<String, String>) {
            (
                true,
                BTreeMap::from([
                    ("_key".to_string(), debug(grafeo::Value::Int64(id))),
                    ("_stub".to_string(), debug(grafeo::Value::Bool(true))),
                ]),
            )
        }

        fn link_props(mut props: BTreeMap<String, String>, key: i64) -> BTreeMap<String, String> {
            props.insert(
                "_stream".to_string(),
                debug(grafeo::Value::String("events".into())),
            );
            props.insert("_key".to_string(), debug(grafeo::Value::Int64(key)));
            props
        }

        fn upsert_node(&mut self, data: &Map<String, Json>) {
            let id = data["id"].as_i64().expect("id");
            let entry = self
                .nodes
                .entry(id)
                .or_insert_with(|| (false, BTreeMap::new()));
            entry.0 = false;
            entry.1.remove("_stub");
            entry.1.extend(json_props(data));
            entry.1 = Self::link_props(std::mem::take(&mut entry.1), id);
        }

        fn delete_node(&mut self, id: i64) {
            if self.nodes.remove(&id).is_none() {
                return;
            }
            let gone: Vec<i64> = self
                .edges
                .iter()
                .filter(|(_, (s, d, _))| *s == id || *d == id)
                .map(|(k, _)| *k)
                .collect();
            for edge in gone {
                self.delete_edge(edge);
            }
        }

        fn upsert_edge(&mut self, data: &Map<String, Json>) {
            let (key, from, to) = (
                data["edge"].as_i64().expect("edge"),
                data["from"].as_i64().expect("from"),
                data["to"].as_i64().expect("to"),
            );
            for end in [from, to] {
                self.nodes.entry(end).or_insert_with(|| Self::stub(end));
            }
            let old = self.edges.get(&key).cloned();
            let mut props = match &old {
                Some((_, _, props)) => props.clone(),
                None => BTreeMap::new(),
            };
            props.extend(json_props(data));
            props = Self::link_props(props, key);
            props.insert(
                "_edge".to_string(),
                debug(grafeo::Value::String("stream:events".into())),
            );
            self.edges.insert(key, (from, to, props));
            if let Some((s, d, _)) = old {
                self.drop_stubs([s, d]);
            }
        }

        fn delete_edge(&mut self, key: i64) {
            if let Some((s, d, _)) = self.edges.remove(&key) {
                self.drop_stubs([s, d]);
            }
        }

        fn drop_stubs(&mut self, ends: [i64; 2]) {
            for end in ends {
                let used = self.edges.values().any(|(s, d, _)| *s == end || *d == end);
                if !used && self.nodes.get(&end).is_some_and(|(stub, _)| *stub) {
                    self.nodes.remove(&end);
                }
            }
        }

        fn of(rows: &loams_graph::snapshot::portable::GraphRows) -> Self {
            let key = |props: &loams_graph::changeset::PropMap| match props.get("_key") {
                Some(grafeo::Value::Int64(k)) => *k,
                other => panic!("an element without an integer key: {other:?}"),
            };
            let mut ids = BTreeMap::new();
            let mut model = Self::default();
            for node in &rows.nodes {
                assert_eq!(node.labels, vec!["Person".to_string()]);
                let k = key(&node.props);
                ids.insert(node.id, k);
                let stub = matches!(node.props.get("_stub"), Some(grafeo::Value::Bool(true)));
                let props = node
                    .props
                    .iter()
                    .map(|(k, v)| (k.clone(), debug(v.clone())))
                    .collect();
                assert!(
                    model.nodes.insert(k, (stub, props)).is_none(),
                    "two nodes of one key"
                );
            }
            for edge in &rows.edges {
                assert_eq!(edge.ty, "KNOWS");
                let props = edge
                    .props
                    .iter()
                    .map(|(k, v)| (k.clone(), debug(v.clone())))
                    .collect();
                model
                    .edges
                    .insert(key(&edge.props), (ids[&edge.src], ids[&edge.dst], props));
            }
            model
        }
    }

    static WRITER_NODE: AtomicU64 = AtomicU64::new(950);

    struct Linked {
        cluster: Cluster,
        writer: LogWriter,
        stream: StreamId,
        model: Mutex<Model>,
        sent: AtomicU64,
    }

    impl Linked {
        async fn start(nodes: u64) -> Self {
            let cluster = Cluster::start(nodes, SHORT).await;
            let ns = cluster
                .meta
                .create_namespace("acme")
                .await
                .expect("namespace");
            let stream = cluster
                .meta
                .create_stream(
                    ns,
                    "events",
                    PARTITIONS,
                    WalClass::Standard,
                    Retention::default(),
                )
                .await
                .expect("stream");
            let writer = LogWriter::start(
                cluster.meta.clone(),
                cluster.store.clone(),
                GraphLog::log_config(WRITER_NODE.fetch_add(1, Ordering::SeqCst)),
            )
            .expect("writer");
            cluster
                .node(1)
                .admin
                .create_graph(pb::CreateGraphRequest {
                    namespace: "acme".to_string(),
                    name: "sg".to_string(),
                    mode: pb::GraphMode::Linked.into(),
                    mapping: mapping().to_proto().into(),
                    ..Default::default()
                })
                .await
                .expect("create");
            Self {
                cluster,
                writer,
                stream,
                model: Mutex::default(),
                sent: AtomicU64::new(0),
            }
        }

        /// Appends a raw record to partition `partition`.
        async fn raw(&self, partition: u32, value: &str, headers: Vec<(&str, &str)>) {
            self.writer
                .append(
                    self.stream,
                    partition,
                    vec![Record {
                        key: None,
                        value: Some(Bytes::from(value.to_string())),
                        headers: headers
                            .into_iter()
                            .map(|(k, v)| (k.to_string(), Some(Bytes::from(v.to_string()))))
                            .collect(),
                        timestamp_ms: -1,
                    }],
                )
                .await
                .expect("append");
        }

        /// Sends one CloudEvent (structured mode, or binary mode every third event) and applies
        /// it to the model. Events go to one partition, so their order is the model's.
        async fn event(&self, ty: &str, data: Json) {
            let n = self.sent.fetch_add(1, Ordering::SeqCst);
            let object = data.as_object().expect("data").clone();
            if n % 3 == 2 {
                let id = format!("e{n}");
                self.raw(
                    0,
                    &data.to_string(),
                    vec![
                        ("ce_specversion", "1.0"),
                        ("ce_type", ty),
                        ("ce_id", &id),
                        ("ce_source", "test"),
                    ],
                )
                .await;
            } else {
                let event = json!({"specversion": "1.0", "type": ty, "id": format!("e{n}"), "source": "test", "data": data});
                self.raw(0, &event.to_string(), vec![]).await;
            }
            let mut model = self.model.lock().expect("lock");
            match ty {
                "person.upserted" => model.upsert_node(&object),
                "person.deleted" => model.delete_node(object["id"].as_i64().expect("id")),
                "knows.added" => model.upsert_edge(&object),
                "knows.removed" => model.delete_edge(object["edge"].as_i64().expect("edge")),
                _ => {}
            }
        }

        fn runner(&self, node: u64) -> LinkRunner {
            LinkRunner::new(
                self.cluster.node(node).admin.clone(),
                LinkConfig {
                    batch_records: 3,
                    batch_interval: Duration::ZERO,
                    ..LinkConfig::default()
                },
            )
        }

        async fn catch_up(&self, node: u64) {
            let runner = self.runner(node);
            for _ in 0..200 {
                runner.apply_graph("acme", "sg").await.expect("apply");
                if runner
                    .lag("acme", "sg")
                    .await
                    .expect("lag")
                    .iter()
                    .all(|l| l.records == 0)
                {
                    return;
                }
            }
            panic!("the stream link never caught up");
        }

        fn graph(&self, node: u64) -> Model {
            let graph = self
                .cluster
                .node(node)
                .admin
                .engine()
                .list(Some("acme"))
                .expect("list")
                .into_iter()
                .find(|g| g.name() == "sg")
                .expect("sg is open");
            Model::of(&graph.rows().expect("rows"))
        }

        fn expected(&self) -> Model {
            self.model.lock().expect("lock").clone()
        }

        async fn assert_versions_consecutive(&self, case: &str) {
            let stream = self.cluster.stream_of("sg").await;
            let mut next: BTreeMap<LinkId, u64> = BTreeMap::new();
            for record in self.cluster.records(stream).await {
                if let Ok(GraphRecord::ChangeSet(change)) =
                    decode(record.record.value.as_deref().expect("value"))
                    && let Some(link) = change.link
                {
                    let expected = next.entry(link.link).or_insert(1);
                    assert_eq!(link.version, *expected, "{case}: link {:?}", link.link);
                    *expected += 1;
                }
            }
        }
    }

    async fn workload(linked: &Linked, round: i64) {
        let base = round * 10;
        for i in 0..4 {
            linked
                .event(
                    "person.upserted",
                    json!({"id": base + i, "name": format!("p{}", base + i)}),
                )
                .await;
        }
        // An edge to a person not seen yet: a stub, filled below.
        linked
            .event(
                "knows.added",
                json!({"edge": base + 1, "from": base, "to": base + 7, "since": 2020}),
            )
            .await;
        linked
            .event(
                "knows.added",
                json!({"edge": base + 2, "from": base + 1, "to": base + 2}),
            )
            .await;
        linked
            .event(
                "knows.added",
                json!({"edge": base + 3, "from": base + 2, "to": base + 8}),
            )
            .await;
        linked
            .event("person.upserted", json!({"id": base + 7, "age": 41}))
            .await;
        linked
            .event("person.upserted", json!({"id": base, "age": 30}))
            .await;
        // A merge into an edge, and one that moves it.
        linked
            .event(
                "knows.added",
                json!({"edge": base + 2, "from": base + 1, "to": base + 2, "since": 1999}),
            )
            .await;
        linked
            .event(
                "knows.added",
                json!({"edge": base + 3, "from": base + 2, "to": base + 3}),
            )
            .await;
        linked
            .event(
                "knows.removed",
                json!({"edge": base + 1, "from": base, "to": base + 7}),
            )
            .await;
        linked
            .event("person.deleted", json!({"id": base + 2}))
            .await;
        // A type no rule maps is skipped.
        linked.event("person.viewed", json!({"id": base})).await;
    }

    /// CloudEvents of each type become node and edge operations: stubs filled, properties
    /// merged, edges moved and deleted, a node's delete taking its edges.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cloudevents_map_to_nodes_and_edges() {
        let linked = Linked::start(1).await;
        workload(&linked, 0).await;
        linked.catch_up(1).await;
        let graph = linked.graph(1);
        assert_eq!(graph, linked.expected());
        // The checks the model encodes, spelled out.
        let (stub, seven) = &graph.nodes[&7];
        assert!(!stub, "the stub was filled");
        assert_eq!(seven["age"], debug(grafeo::Value::Int64(41)));
        let (_, zero) = &graph.nodes[&0];
        assert!(
            zero.contains_key("name") && zero.contains_key("age"),
            "merged: {zero:?}"
        );
        assert!(!graph.nodes.contains_key(&2), "deleted");
        assert!(
            !graph.nodes.contains_key(&8),
            "a stub the moved edge left goes"
        );
        assert!(graph.edges.is_empty(), "{:?}", graph.edges);
        let lag = linked.runner(1).lag("acme", "sg").await.expect("lag");
        assert_eq!(lag.len(), 1);
        assert_eq!((lag[0].records, lag[0].dead_letters), (0, 0));
    }

    /// A record a rule cannot read is dead-lettered with why, and the records after it apply.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn malformed_record_dead_lettered_not_blocking() {
        let linked = Linked::start(1).await;
        linked.raw(1, "not json", vec![]).await;
        linked
            .raw(1, &json!({"specversion": "1.0", "type": "person.upserted", "id": "x", "data": {"name": "no id"}}).to_string(), vec![])
            .await;
        linked
            .raw(
                1,
                "{",
                vec![("ce_type", "person.upserted"), ("ce_specversion", "1.0")],
            )
            .await;
        linked
            .event("person.upserted", json!({"id": 9, "name": "ok"}))
            .await;
        linked.catch_up(1).await;
        assert_eq!(linked.graph(1), linked.expected());
        let letters = linked
            .runner(1)
            .dead_letters("acme", "sg", "events")
            .await
            .expect("dead letters");
        assert_eq!(
            letters
                .iter()
                .map(|l| (l.partition, l.offset))
                .collect::<Vec<_>>(),
            vec![(1, 0), (1, 1), (1, 2)]
        );
        assert_eq!(letters[0].value, "not json");
        assert_eq!(letters[1].rule.as_deref(), Some("person"));
        assert!(
            letters[1].reason.contains("data.id"),
            "{}",
            letters[1].reason
        );
        let lag = linked.runner(1).lag("acme", "sg").await.expect("lag");
        assert_eq!((lag[0].records, lag[0].dead_letters), (0, 3));
        // Applying again neither re-applies nor re-letters anything.
        linked
            .runner(1)
            .apply_graph("acme", "sg")
            .await
            .expect("apply");
        assert_eq!(
            linked
                .runner(1)
                .dead_letters("acme", "sg", "events")
                .await
                .expect("again")
                .len(),
            3
        );
        assert_eq!(linked.graph(1), linked.expected());
    }

    fn kill_at(
        point: KillPoint,
        occurrence: u64,
        reached: watch::Sender<bool>,
        release: watch::Receiver<bool>,
    ) -> KillHook {
        let seen = std::sync::Arc::new(AtomicU64::new(0));
        let reached = std::sync::Arc::new(reached);
        std::sync::Arc::new(move |at| {
            let stop = at == point && seen.fetch_add(1, Ordering::SeqCst) + 1 == occurrence;
            let (reached, mut release) = (reached.clone(), release.clone());
            Box::pin(async move {
                if stop {
                    let _ = reached.send(true);
                    let _ = release.wait_for(|released| *released).await;
                }
            })
        })
    }

    async fn kill_case(point: KillPoint, deadline: &Deadline) {
        let case = format!("{point:?}");
        let at = |phase: &str| deadline.phase(format!("{case}: {phase}"));
        at("starting the cluster");
        let linked = Linked::start(2).await;
        let graph = linked.cluster.graph_id("sg").await;
        let victim = linked.cluster.preferred(graph);
        let survivor = 3 - victim;
        at("the first workload");
        workload(&linked, 0).await;
        linked.catch_up(victim).await;
        assert_eq!(linked.graph(victim), linked.expected(), "{case}: before");
        workload(&linked, 1).await;
        let (reached_tx, mut reached) = watch::channel(false);
        let (release, released) = watch::channel(false);
        linked
            .cluster
            .node(victim)
            .log
            .set_kill_hook(Some(kill_at(point, 2, reached_tx, released)));
        at("waiting for the kill point");
        let runner = linked.runner(victim);
        let stuck = tokio::spawn(async move { runner.apply_graph("acme", "sg").await });
        tokio::time::timeout(Duration::from_secs(30), reached.wait_for(|r| *r))
            .await
            .unwrap_or_else(|_| panic!("{case}: the kill point was never reached"))
            .expect("the hook");
        linked.cluster.node(victim).ownership.pause_renewals(true);
        linked.cluster.forwarder.cut_off(victim);
        linked
            .cluster
            .node(survivor)
            .membership
            .set(vec![linked.cluster.node(survivor).me.clone()]);
        at("waiting out the dead owner's lease");
        wait_out_the_lease(&linked.cluster, graph).await;
        at("the survivor catching up");
        linked.catch_up(survivor).await;
        assert_eq!(
            linked.graph(survivor),
            linked.expected(),
            "{case}: after the kill"
        );
        linked.assert_versions_consecutive(&case).await;
        at("the dead node coming back");
        let _ = release.send(true);
        let _ = tokio::time::timeout(Duration::from_secs(30), stuck).await;
        linked.catch_up(survivor).await;
        assert_eq!(
            linked.graph(survivor),
            linked.expected(),
            "{case}: after the release"
        );
        linked.assert_versions_consecutive(&case).await;
    }

    /// The stream link's owner killed at each write-path point mid-batch: the other node takes
    /// over, and every event is applied once. Under a hard deadline, its runtime's shutdown
    /// included: a hang fails in minutes and names its phase (#1085).
    #[test]
    fn stream_link_exactly_once_under_kill() {
        use super::collection_link::{KILL_DEADLINE, KILL_POINTS};
        let deadline = Deadline::start("stream_link_exactly_once_under_kill", KILL_DEADLINE);
        let runtime = runtime(4);
        runtime.block_on(async {
            for point in KILL_POINTS {
                kill_case(point, &deadline).await;
            }
        });
        deadline.phase("the runtime shutting down");
        drop(runtime);
    }
}
