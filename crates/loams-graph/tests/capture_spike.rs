//! GR1 Task 9: the change-capture spike (Q672), kept as a regression test.
//!
//! Every test runs against Grafeo 0.5.43 directly, through the database a graph is opened with,
//! because what is being measured is the engine: whether its CDC log, resolved against its store,
//! is a complete record of a commit (`cdc_capture_matches_model`), whether that record rebuilds the
//! graph with the same element ids (`replay_reproduces_dump_and_ids`), and how far a session's
//! viewing epoch isolates it from later and from uncommitted writes
//! (`viewing_epoch_hides_later_commits`, `uncommitted_writes_are_visible_to_pinned_sessions`,
//! `pinned_epoch_survives_gc`). The rulings these answer are R9.1–R9.8 in the GR1 plan.
//!
//! Tests whose name starts with `canary_` pin an engine gap: they fail when Grafeo fixes it, and
//! the failure message says which ruling to revisit.
//!
//! These are measurements of the build Loams ships, so they do not run with the spike-only
//! `grafeo-temporal` feature; `tests/capture_spike_temporal.rs` holds what that feature changes.

#![cfg(not(feature = "grafeo-temporal"))]

use std::collections::BTreeMap;
use std::sync::{Arc, Barrier, Mutex};

use grafeo::{Config, GrafeoDB, Role, Session, Value};
use grafeo_common::types::EpochId;
use loams_graph::Access;
use loams_graph::changeset::capture::{CdcCapture, ChangeCapture};
use loams_graph::changeset::{ChangeOp, ElementKind, PropMap, apply_to_store};
use loams_graph::classify::engine_classify;
use proptest::prelude::*;

// ---------------------------------------------------------------------------------------------
// The engine, the dump and the model
// ---------------------------------------------------------------------------------------------

/// A graph's database as GR1b opens it: CDC on (R0.6 (c)).
fn cdc_db() -> GrafeoDB {
    GrafeoDB::with_config(Config::in_memory().with_cdc()).expect("an in-memory database opens")
}

/// The rows of a statement asserted to succeed.
fn rows(session: &Session, statement: &str) -> Vec<Vec<Value>> {
    session
        .execute(statement)
        .unwrap_or_else(|err| panic!("{statement}: {err}"))
        .rows()
        .to_vec()
}

/// A node in a dump: sorted labels and properties.
type NodeState = (Vec<String>, PropMap);
/// An edge in a dump: type, source, destination and properties.
type EdgeState = (String, u64, u64, PropMap);

/// A canonical dump of a graph: every live node and edge, by id.
#[derive(Debug, Default, Clone, PartialEq)]
struct Dump {
    nodes: BTreeMap<u64, NodeState>,
    edges: BTreeMap<u64, EdgeState>,
}

/// Reads the dump from Grafeo's store (the latest state, which under the lane is the last commit).
fn dump(db: &GrafeoDB) -> Dump {
    let store = db.store();
    let mut out = Dump::default();
    for node in store.all_nodes() {
        let mut labels: Vec<String> = node.labels.iter().map(ToString::to_string).collect();
        labels.sort();
        let props = node
            .properties
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.clone()))
            .collect();
        out.nodes.insert(node.id.as_u64(), (labels, props));
    }
    for edge in store.all_edges() {
        let props = edge
            .properties
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.clone()))
            .collect();
        out.edges.insert(
            edge.id.as_u64(),
            (
                edge.edge_type.to_string(),
                edge.src.as_u64(),
                edge.dst.as_u64(),
                props,
            ),
        );
    }
    out
}

impl Dump {
    /// The pure model: applies change operations with their documented meaning, checking each
    /// one's precondition, so an operation the engine could not have meant fails the test.
    fn apply(&mut self, ops: &[ChangeOp]) {
        for op in ops {
            match op {
                ChangeOp::UpsertNode { id, labels, props } => {
                    assert!(labels.is_sorted(), "labels are sorted: {op:?}");
                    self.nodes.insert(*id, (labels.clone(), props.clone()));
                }
                ChangeOp::DeleteNode { id } => {
                    assert!(
                        self.nodes.remove(id).is_some(),
                        "delete of a missing node: {op:?}"
                    );
                    assert!(
                        !self.edges.values().any(|(_, s, d, _)| s == id || d == id),
                        "node {id} deleted with edges left: {op:?}"
                    );
                }
                ChangeOp::UpsertEdge {
                    id,
                    ty,
                    src,
                    dst,
                    props,
                } => {
                    assert!(
                        self.nodes.contains_key(src) && self.nodes.contains_key(dst),
                        "edge before its endpoints: {op:?}"
                    );
                    self.edges
                        .insert(*id, (ty.clone(), *src, *dst, props.clone()));
                }
                ChangeOp::DeleteEdge { id } => {
                    assert!(
                        self.edges.remove(id).is_some(),
                        "delete of a missing edge: {op:?}"
                    );
                }
                ChangeOp::SetProps {
                    id,
                    kind,
                    set,
                    removed,
                } => {
                    let props = match kind {
                        ElementKind::Node => &mut self.nodes.get_mut(id).expect("node exists").1,
                        ElementKind::Edge => &mut self.edges.get_mut(id).expect("edge exists").3,
                    };
                    for key in removed {
                        props.remove(key);
                    }
                    props.extend(set.iter().map(|(k, v)| (k.clone(), v.clone())));
                }
            }
        }
    }
}

/// How one transaction ended through the lane.
enum Outcome {
    /// Committed at this epoch with these operations.
    Committed(u64, Vec<ChangeOp>),
    /// Rolled back: asked for, or a statement failed (as an atomic batch does).
    RolledBack,
    /// Committed in the engine, but the capture refused it (R9.8): the lane must reload the graph
    /// from the log, which does not have it.
    Refused,
}

/// One write transaction the way GR1 Task 11's lane runs it (§48 §6.2 steps 1–5): begin, run the
/// statements, `prepare_commit().commit()`, then capture at the epoch that returns. The caller
/// serialises calls (R0.5), so that epoch is the transaction's own.
fn run_txn(
    db: &GrafeoDB,
    capture: &CdcCapture<'_>,
    statements: &[String],
    rollback: bool,
) -> Outcome {
    let mut session = db.session();
    capture.begin(&mut session);
    session.begin_transaction().expect("begin");
    for statement in statements {
        if session.execute(statement).is_err() {
            session
                .rollback()
                .expect("rollback after a failed statement");
            return Outcome::RolledBack;
        }
    }
    if rollback {
        session.rollback().expect("rollback");
        return Outcome::RolledBack;
    }
    let epoch = session
        .prepare_commit()
        .expect("prepare")
        .commit()
        .expect("commit");
    match capture.take(epoch.as_u64()) {
        Ok(ops) => Outcome::Committed(epoch.as_u64(), ops),
        Err(_) => Outcome::Refused,
    }
}

/// A committed transaction, asserted.
fn committed(outcome: Outcome) -> (u64, Vec<ChangeOp>) {
    match outcome {
        Outcome::Committed(epoch, ops) => (epoch, ops),
        Outcome::RolledBack => panic!("the transaction rolled back"),
        Outcome::Refused => panic!("the capture refused the commit"),
    }
}

/// A fresh engine rebuilt from a log of change sets: Task 11's reload and Task 14's recovery.
fn rebuild(log: &[(u64, Vec<ChangeOp>)]) -> GrafeoDB {
    let db = cdc_db();
    for (_, ops) in log {
        apply_to_store(&db, ops).expect("replay applies");
    }
    db
}

// ---------------------------------------------------------------------------------------------
// The random workload
// ---------------------------------------------------------------------------------------------

/// One GQL write. Nodes are found by a small integer `k`, so statements hit existing elements
/// often; several nodes may share a `k`, and then a statement touches all of them.
#[derive(Debug, Clone)]
enum Op {
    InsertNode {
        k: u8,
        labels: Vec<&'static str>,
        x: Option<i64>,
        s: Option<String>,
    },
    /// Two new nodes and an edge between them, in one pattern.
    InsertPath {
        from: u8,
        to: u8,
        ty: &'static str,
    },
    InsertEdge {
        from: u8,
        to: u8,
        ty: &'static str,
        w: Option<i64>,
    },
    Merge {
        k: u8,
    },
    SetProp {
        k: u8,
        key: &'static str,
        value: String,
    },
    SetMap {
        k: u8,
        replace: bool,
        x: i64,
    },
    RemoveProp {
        k: u8,
        key: &'static str,
    },
    AddLabel {
        k: u8,
        label: &'static str,
    },
    RemoveLabel {
        k: u8,
        label: &'static str,
    },
    DeleteNode {
        k: u8,
        detach: bool,
    },
    SetEdgeProp {
        from: u8,
        ty: &'static str,
        w: i64,
    },
    RemoveEdgeProp {
        from: u8,
        ty: &'static str,
    },
    DeleteEdge {
        from: u8,
        ty: &'static str,
    },
}

impl Op {
    fn gql(&self) -> String {
        match self {
            Op::InsertNode { k, labels, x, s } => {
                let mut props = vec![format!("k: {k}")];
                if let Some(x) = x {
                    props.push(format!("x: {x}"));
                }
                if let Some(s) = s {
                    props.push(format!("s: '{s}'"));
                }
                let labels: String = labels.iter().map(|l| format!(":{l}")).collect();
                format!("INSERT ({labels} {{{}}})", props.join(", "))
            }
            Op::InsertPath { from, to, ty } => {
                format!("INSERT (:A {{k: {from}}})-[:{ty} {{w: 0}}]->(:B {{k: {to}}})")
            }
            Op::InsertEdge { from, to, ty, w } => {
                let props = w.map(|w| format!(" {{w: {w}}}")).unwrap_or_default();
                format!("MATCH (a {{k: {from}}}), (b {{k: {to}}}) INSERT (a)-[:{ty}{props}]->(b)")
            }
            Op::Merge { k } => format!("MERGE (n:M {{k: {k}}})"),
            Op::SetProp { k, key, value } => format!("MATCH (n {{k: {k}}}) SET n.{key} = {value}"),
            Op::SetMap { k, replace, x } => {
                let op = if *replace { "=" } else { "+=" };
                format!("MATCH (n {{k: {k}}}) SET n {op} {{k: {k}, x: {x}}}")
            }
            Op::RemoveProp { k, key } => format!("MATCH (n {{k: {k}}}) REMOVE n.{key}"),
            Op::AddLabel { k, label } => format!("MATCH (n {{k: {k}}}) SET n:{label}"),
            Op::RemoveLabel { k, label } => format!("MATCH (n {{k: {k}}}) REMOVE n:{label}"),
            Op::DeleteNode { k, detach } => {
                let detach = if *detach { "DETACH " } else { "" };
                format!("MATCH (n {{k: {k}}}) {detach}DELETE n")
            }
            Op::SetEdgeProp { from, ty, w } => {
                format!("MATCH ({{k: {from}}})-[r:{ty}]->() SET r.w = {w}")
            }
            Op::RemoveEdgeProp { from, ty } => {
                format!("MATCH ({{k: {from}}})-[r:{ty}]->() REMOVE r.w")
            }
            Op::DeleteEdge { from, ty } => format!("MATCH ({{k: {from}}})-[r:{ty}]->() DELETE r"),
        }
    }
}

fn op() -> impl Strategy<Value = Op> {
    let k = 0u8..4;
    let label = prop::sample::select(vec!["A", "B", "C"]);
    let ty = prop::sample::select(vec!["R", "S"]);
    let key = prop::sample::select(vec!["x", "s", "b", "l"]);
    let value = prop_oneof![
        any::<i32>().prop_map(|v| v.to_string()),
        "[a-z]{0,6}".prop_map(|s| format!("'{s}'")),
        any::<bool>().prop_map(|b| b.to_string()),
        prop::collection::vec(any::<i16>(), 0..3).prop_map(|v| format!("{v:?}")),
    ];
    prop_oneof![
        3 => (k.clone(), prop::sample::subsequence(vec!["A", "B", "C"], 0..=2),
              prop::option::of(any::<i32>()), prop::option::of("[a-z]{1,4}"))
            .prop_map(|(k, labels, x, s)| Op::InsertNode { k, labels, x: x.map(i64::from), s }),
        1 => (k.clone(), k.clone(), ty.clone())
            .prop_map(|(from, to, ty)| Op::InsertPath { from, to, ty }),
        4 => (k.clone(), k.clone(), ty.clone(), prop::option::of(any::<i32>()))
            .prop_map(|(from, to, ty, w)| Op::InsertEdge { from, to, ty, w: w.map(i64::from) }),
        1 => k.clone().prop_map(|k| Op::Merge { k }),
        2 => (k.clone(), key.clone(), value).prop_map(|(k, key, value)| Op::SetProp { k, key, value }),
        1 => (k.clone(), any::<bool>(), any::<i32>())
            .prop_map(|(k, replace, x)| Op::SetMap { k, replace, x: i64::from(x) }),
        1 => (k.clone(), key).prop_map(|(k, key)| Op::RemoveProp { k, key }),
        1 => (k.clone(), label.clone()).prop_map(|(k, label)| Op::AddLabel { k, label }),
        1 => (k.clone(), label).prop_map(|(k, label)| Op::RemoveLabel { k, label }),
        1 => (k.clone(), any::<bool>()).prop_map(|(k, detach)| Op::DeleteNode { k, detach }),
        2 => (k.clone(), ty.clone(), any::<i32>())
            .prop_map(|(from, ty, w)| Op::SetEdgeProp { from, ty, w: i64::from(w) }),
        1 => (k.clone(), ty.clone()).prop_map(|(from, ty)| Op::RemoveEdgeProp { from, ty }),
        2 => (k, ty).prop_map(|(from, ty)| Op::DeleteEdge { from, ty }),
    ]
}

/// A transaction: one to four statements (a multi-statement batch when more than one), and a
/// rollback one time in eight.
fn txn() -> impl Strategy<Value = (Vec<Op>, bool)> {
    (
        prop::collection::vec(op(), 1..5),
        prop::bool::weighted(0.125),
    )
}

fn workload() -> impl Strategy<Value = Vec<(Vec<Op>, bool)>> {
    prop::collection::vec(txn(), 1..8)
}

/// Runs a workload through the lane. After every commit, the captured operations applied to the
/// model must give exactly the engine's dump; after a rollback, nothing may have changed; after a
/// refused capture, the graph is rebuilt from the log and must equal the model. Returns the final
/// engine and every committed change set, in order.
fn run_workload(txns: &[(Vec<Op>, bool)]) -> (GrafeoDB, Vec<(u64, Vec<ChangeOp>)>) {
    let mut db = cdc_db();
    let mut model = Dump::default();
    let mut log = Vec::new();
    for (ops, rollback) in txns {
        let statements: Vec<String> = ops.iter().map(Op::gql).collect();
        let outcome = {
            let capture = CdcCapture::new(&db).expect("CDC is on");
            run_txn(&db, &capture, &statements, *rollback)
        };
        match outcome {
            Outcome::Committed(epoch, ops) => {
                model.apply(&ops);
                log.push((epoch, ops));
            }
            Outcome::RolledBack => {}
            Outcome::Refused => db = rebuild(&log),
        }
        assert_eq!(model, dump(&db), "model after {statements:?}");
    }
    (db, log)
}

// ---------------------------------------------------------------------------------------------
// Step 2: the capture is complete
// ---------------------------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig { cases: 10_000, ..ProptestConfig::default() })]

    /// Mechanism B is complete (R9.1): for 10 000 random workloads of inserts, property sets,
    /// map sets (`=` and `+=`), removes, label adds and removes, `MERGE`, deletes, `DETACH DELETE`,
    /// edge inserts, updates and deletes, multi-statement transactions, failed statements and
    /// rollbacks, the captured operations applied to a pure model equal Grafeo's dump after every
    /// transaction.
    #[test]
    fn cdc_capture_matches_model(txns in workload()) {
        run_workload(&txns);
    }
}

// ---------------------------------------------------------------------------------------------
// Step 3: replay keeps the dump and the ids
// ---------------------------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig { cases: 1_000, ..ProptestConfig::default() })]

    /// Replaying the captured change sets into a fresh engine through `apply_to_store` gives the
    /// same dump, element ids included (R9.4). The replayed engine answers GQL over the replayed
    /// data, its next GQL insert takes an id above every replayed one (R0.7), and replaying the
    /// whole log a second time changes nothing.
    #[test]
    fn replay_reproduces_dump_and_ids(txns in workload()) {
        let (live, log) = run_workload(&txns);
        let expected = dump(&live);

        let fresh = rebuild(&log);
        prop_assert_eq!(&dump(&fresh), &expected);

        // GQL sees the replayed graph: labels, properties and edges through the planner's
        // indexes, not only the store.
        let session = fresh.session();
        let live_session = live.session();
        for statement in [
            "MATCH (n) RETURN count(n)",
            "MATCH (n:A) RETURN count(n)",
            "MATCH (n:B) RETURN count(n)",
            "MATCH (n) WHERE n.k = 1 RETURN count(n)",
            "MATCH ()-[r:R]->() RETURN count(r)",
            "MATCH (a)-[r]->(b) RETURN count(r)",
        ] {
            prop_assert_eq!(rows(&session, statement), rows(&live_session, statement), "{}", statement);
        }

        // Replay is idempotent.
        for (_, ops) in &log {
            apply_to_store(&fresh, ops).expect("replay applies twice");
        }
        prop_assert_eq!(&dump(&fresh), &expected);

        // The allocator moved past every replayed id.
        let max_node = expected.nodes.keys().max().copied();
        session.execute("INSERT (:Fresh)").expect("insert after replay");
        let new_id = dump(&fresh)
            .nodes
            .iter()
            .find(|(_, (labels, _))| labels == &["Fresh".to_string()])
            .map(|(id, _)| *id)
            .expect("the new node");
        prop_assert!(max_node.is_none_or(|max| new_id > max), "new id {} vs replayed max {:?}", new_id, max_node);
    }
}

/// What replay does *not* carry over (R9.5): the replayed engine's epoch. Direct store writes run
/// outside transactions, so the fresh engine is still at epoch 0 whatever the commit epochs in the
/// log were, and its own next commit is epoch 1. A change set's `commit_epoch` is therefore not an
/// order that survives recovery; the log offset (and Loams' `txn_seq`) is.
#[test]
fn replay_does_not_carry_the_engine_epoch() {
    let live = cdc_db();
    let capture = CdcCapture::new(&live).expect("CDC is on");
    let log: Vec<_> = (0..5)
        .map(|k| {
            committed(run_txn(
                &live,
                &capture,
                &[format!("INSERT (:P {{k: {k}}})")],
                false,
            ))
        })
        .collect();
    let epochs: Vec<u64> = log.iter().map(|(e, _)| *e).collect();
    assert_eq!(epochs, vec![1, 2, 3, 4, 5]);

    let fresh = rebuild(&log);
    assert_eq!(fresh.store().current_epoch(), EpochId::new(0));
    let fresh_capture = CdcCapture::new(&fresh).expect("CDC is on");
    let (epoch, _) = committed(run_txn(
        &fresh,
        &fresh_capture,
        &["INSERT (:P {k: 9})".to_string()],
        false,
    ));
    assert_eq!(epoch, 1, "the fresh engine's epochs restart");
}

/// Measured (R9.8): Grafeo 0.5.43 commits a dangling edge when a transaction inserts an edge and
/// then `DETACH DELETE`s a node that existed before it: the node goes, the new edge stays, and no
/// event says the edge was deleted. The capture refuses such a commit, and the graph rebuilt from
/// the log is the graph before it. A second shape of the same bug: when the node is new too, the
/// `DETACH DELETE` deletes nothing. Fails when Grafeo fixes either (revisit R9.8).
#[test]
fn canary_detach_delete_leaves_a_dangling_edge() {
    let db = cdc_db();
    let capture = CdcCapture::new(&db).expect("CDC is on");
    let seed = committed(run_txn(
        &db,
        &capture,
        &["INSERT (:A {k: 1})".into(), "INSERT (:B {k: 2})".into()],
        false,
    ));
    let statements = [
        "MATCH (a {k: 1}), (b {k: 2}) INSERT (a)-[:R]->(b)".to_string(),
        "MATCH (n {k: 2}) DETACH DELETE n".to_string(),
    ];
    assert!(
        matches!(run_txn(&db, &capture, &statements, false), Outcome::Refused),
        "the dangling-edge commit is no longer produced: revisit R9.8"
    );
    let engine = dump(&db);
    assert_eq!(engine.nodes.len(), 1, "node k = 2 is gone in the engine");
    assert_eq!(
        engine.edges.values().next().map(|(_, _, dst, _)| *dst),
        Some(1),
        "its edge is not"
    );
    let rebuilt = dump(&rebuild(&[seed]));
    assert_eq!(
        (rebuilt.nodes.len(), rebuilt.edges.len()),
        (2, 0),
        "the log has the graph before it"
    );

    let db = cdc_db();
    let capture = CdcCapture::new(&db).expect("CDC is on");
    let (_, ops) = committed(run_txn(
        &db,
        &capture,
        &[
            "INSERT (:A {k: 1})-[:R]->(:B {k: 2})".into(),
            "MATCH (n {k: 2}) DETACH DELETE n".into(),
        ],
        false,
    ));
    assert_eq!(
        (dump(&db).nodes.len(), ops.len()),
        (2, 3),
        "DETACH DELETE of a node created in the same transaction now deletes: revisit R9.8"
    );
}

// ---------------------------------------------------------------------------------------------
// Step 4: viewing epochs and GC
// ---------------------------------------------------------------------------------------------

/// A read-only session pinned at the database's current epoch, as §48 §6.2 pins client sessions
/// at the durable epoch.
fn pinned(db: &GrafeoDB) -> (Session, EpochId) {
    let epoch = db.store().current_epoch();
    let session = db.session_with_role(Role::ReadOnly);
    session.set_viewing_epoch(epoch);
    (session, epoch)
}

const NODES: &str = "MATCH (n:P) RETURN n.k, n.name, labels(n) ORDER BY n.k";
const EDGES: &str = "MATCH (a)-[r]->(b) RETURN a.k, b.k, r.w ORDER BY a.k";

/// The seed every visibility test starts from: two edges between four nodes.
fn seeded() -> GrafeoDB {
    let db = cdc_db();
    let session = db.session();
    session
        .execute("INSERT (:P {k: 1, name: 'a'})-[:R {w: 1}]->(:P {k: 2, name: 'b'})")
        .expect("seed");
    session
        .execute("INSERT (:P {k: 3, name: 'c'})-[:R {w: 2}]->(:P {k: 4, name: 'd'})")
        .expect("seed");
    db
}

fn p(k: i64, name: Option<&str>, labels: &[&str]) -> Vec<Value> {
    vec![
        Value::Int64(k),
        name.map_or(Value::Null, Value::from),
        Value::List(
            labels
                .iter()
                .map(|l| Value::from(*l))
                .collect::<Vec<_>>()
                .into(),
        ),
    ]
}

/// Measured (R9.3): in 0.5.43 a viewing epoch hides only element *creation* committed after it.
/// Property sets and removes, label changes, node and edge deletes and edge property changes
/// committed after the pin are all visible to the pinned session, on every statement it runs.
/// `count(n)` is not pinned even for creation. The pinned session's answers after the commits are
/// spelled out exactly, so this fails when Grafeo starts to isolate any of them (revisit R9.3).
#[test]
fn viewing_epoch_hides_later_commits() {
    let db = seeded();
    let (reader, pin) = pinned(&db);
    let before = rows(&reader, NODES);
    assert_eq!(
        before,
        vec![
            p(1, Some("a"), &["P"]),
            p(2, Some("b"), &["P"]),
            p(3, Some("c"), &["P"]),
            p(4, Some("d"), &["P"]),
        ]
    );

    let writer = db.session();
    for statement in [
        "INSERT (:P {k: 5, name: 'e'})",
        "MATCH (n:P {k: 1}) SET n.name = 'CHANGED'",
        "MATCH (n:P {k: 2}) SET n:Extra",
        "MATCH (n:P {k: 3}) REMOVE n.name",
        "MATCH (n:P {k: 4}) DETACH DELETE n",
        "MATCH (:P {k: 1})-[r:R]->() SET r.w = 99",
    ] {
        writer.execute(statement).expect(statement);
    }
    assert!(db.store().current_epoch() > pin);

    // Hidden: the node created after the pin (k = 5). Not hidden: everything else.
    assert_eq!(
        rows(&reader, NODES),
        vec![
            p(1, Some("CHANGED"), &["P"]),
            p(2, Some("b"), &["Extra", "P"]),
            p(3, None, &["P"]),
        ],
        "Grafeo's viewing epoch now isolates more than creation: revisit R9.3"
    );
    assert_eq!(
        rows(&reader, EDGES),
        vec![vec![Value::Int64(1), Value::Int64(2), Value::Int64(99)]],
        "edge property and delete visibility changed: revisit R9.3"
    );
    // `count(n)` is not pinned at all: it answers the live count (k = 1, 2, 3 and 5), which
    // includes the node the pattern above hides.
    assert_eq!(
        rows(&reader, "MATCH (n) RETURN count(n)"),
        vec![vec![Value::Int64(4)]],
        "count(n) now honours the viewing epoch: revisit R9.3"
    );
}

/// Measured (R9.3): writes of a transaction that has not committed are visible to sessions
/// pinned at the current epoch, the case of every client session when no write is pending
/// (property sets, label adds, deletes); only uncommitted creations are hidden. After a
/// rollback the old state is back. Fails when Grafeo hides uncommitted writes (revisit R9.3).
#[test]
fn uncommitted_writes_are_visible_to_pinned_sessions() {
    let db = seeded();
    let (reader, _) = pinned(&db);
    let mut txn = db.session();
    txn.begin_transaction().expect("begin");
    for statement in [
        "INSERT (:P {k: 5, name: 'e'})",
        "MATCH (n:P {k: 1}) SET n.name = 'DIRTY', n:Dirty",
        "MATCH (n:P {k: 4}) DETACH DELETE n",
    ] {
        txn.execute(statement).expect(statement);
    }
    let during = rows(&reader, NODES);
    txn.rollback().expect("rollback");
    let after = rows(&reader, NODES);
    assert_eq!(
        during,
        vec![
            p(1, Some("DIRTY"), &["Dirty", "P"]),
            p(2, Some("b"), &["P"]),
            p(3, Some("c"), &["P"]),
        ],
        "Grafeo now hides uncommitted writes from pinned sessions: revisit R9.3"
    );
    assert_eq!(
        after,
        vec![
            p(1, Some("a"), &["P"]),
            p(2, Some("b"), &["P"]),
            p(3, Some("c"), &["P"]),
            p(4, Some("d"), &["P"]),
        ]
    );
}

/// The Loams-side answer to I2 (R9.3): the graph's gate. The lane holds it exclusively from
/// `begin_transaction` until its change set is durable (here: until after the commit and the
/// capture, standing in for the append); a client statement holds it shared. A client statement
/// therefore never runs while a write is pending, and sees exactly the last durable state, for
/// every kind of write the two tests above show leaking.
#[test]
fn exclusive_gate_hides_non_durable_writes() {
    let db = Arc::new(seeded());
    let gate = Arc::new(std::sync::RwLock::new(()));
    let capture_db = Arc::clone(&db);
    let before = {
        let _shared = gate.read().expect("gate");
        rows(&pinned(&db).0, NODES)
    };

    let lane_holds = Arc::new(Barrier::new(2));
    let lane = {
        let gate = Arc::clone(&gate);
        let lane_holds = Arc::clone(&lane_holds);
        std::thread::spawn(move || {
            let _exclusive = gate.write().expect("gate");
            lane_holds.wait();
            let capture = CdcCapture::new(&capture_db).expect("CDC is on");
            let statements: Vec<String> = [
                "INSERT (:P {k: 5, name: 'e'})",
                "MATCH (n:P {k: 1}) SET n.name = 'CHANGED', n:Extra",
                "MATCH (n:P {k: 4}) DETACH DELETE n",
            ]
            .iter()
            .map(ToString::to_string)
            .collect();
            // Linger inside the window where the commit is in the engine but not yet durable.
            let outcome = run_txn(&capture_db, &capture, &statements, false);
            std::thread::sleep(std::time::Duration::from_millis(50));
            matches!(outcome, Outcome::Committed(..))
        })
    };
    lane_holds.wait();
    // The reader blocks until the lane releases: it reads either the state before or after the
    // whole durable write, never the window in between.
    let during = {
        let _shared = gate.read().expect("gate");
        rows(&pinned(&db).0, NODES)
    };
    assert!(lane.join().expect("lane"), "the write committed");
    assert_ne!(during, before, "the reader waited for the lane");
    assert_eq!(
        during,
        vec![
            p(1, Some("CHANGED"), &["Extra", "P"]),
            p(2, Some("b"), &["P"]),
            p(3, Some("c"), &["P"]),
            p(5, Some("e"), &["P"]),
        ]
    );
}

/// GC and pinned sessions (R9.3, R9.6). Without Grafeo's `temporal` feature the only per-epoch
/// state is element existence, and `GrafeoDB::gc` keeps a creation hidden from a session pinned
/// before it: the pinned answer is the same before and after GC, and so is a new session pinned
/// at the same epoch. (With `temporal`, GC drops the property and label versions a pinned session
/// still needs; see `tests/capture_spike_temporal.rs`.)
#[test]
fn pinned_epoch_survives_gc() {
    let db = seeded();
    let (reader, pin) = pinned(&db);
    let writer = db.session();
    writer.execute("INSERT (:P {k: 5})").expect("insert");
    writer
        .execute("INSERT (:P {k: 6})-[:R]->(:P {k: 7})")
        .expect("insert");
    let before_gc = (rows(&reader, NODES), rows(&reader, EDGES));
    db.gc();
    let after_gc = (rows(&reader, NODES), rows(&reader, EDGES));
    assert_eq!(before_gc, after_gc);
    let again = db.session_with_role(Role::ReadOnly);
    again.set_viewing_epoch(pin);
    assert_eq!((rows(&again, NODES), rows(&again, EDGES)), after_gc);
    assert_eq!(after_gc.0.len(), 4, "creations after the pin stay hidden");
}

// ---------------------------------------------------------------------------------------------
// DDL and the commit epoch
// ---------------------------------------------------------------------------------------------

/// DDL is not in CDC and does not advance the epoch (R0.6), and the classifier marks it Admin, so
/// it takes the `SchemaChange` branch: recorded as its statement text, never through capture
/// (R0.6 (d), R9.7).
#[test]
fn ddl_capture_or_schema_record() {
    let db = cdc_db();
    let capture = CdcCapture::new(&db).expect("CDC is on");
    let admin = db.session_with_role(Role::Admin);
    for statement in [
        "CREATE NODE TYPE Person (name STRING)",
        "CREATE INDEX idx_person_name FOR (p:Person) ON (p.name)",
        "DROP INDEX idx_person_name",
        "DROP NODE TYPE Person",
    ] {
        assert_eq!(
            engine_classify(statement).expect(statement),
            Access::Admin,
            "{statement} is routed to SchemaChange"
        );
        let epoch = db.store().current_epoch();
        let events = db
            .changes_between(EpochId::new(0), EpochId::new(u64::MAX - 1))
            .expect("cdc")
            .len();
        admin.execute(statement).expect(statement);
        assert_eq!(
            db.store().current_epoch(),
            epoch,
            "{statement} advanced the epoch"
        );
        assert_eq!(
            db.changes_between(EpochId::new(0), EpochId::new(u64::MAX - 1))
                .expect("cdc")
                .len(),
            events,
            "{statement} produced CDC events: revisit R9.7"
        );
        assert_eq!(capture.take(epoch.as_u64()).expect("capture"), vec![]);
    }
}

/// R0.5: under the lane's mutex, `prepare_commit().commit()` returns the transaction's own commit
/// epoch, which is also the epoch of its CDC events. Four threads, 25 transactions each.
#[test]
fn commit_epoch_is_exact_under_lane() {
    let db = Arc::new(cdc_db());
    let lane = Arc::new(Mutex::new(()));
    let start = Arc::new(Barrier::new(4));
    let threads: Vec<_> = (0..4)
        .map(|t| {
            let (db, lane, start) = (Arc::clone(&db), Arc::clone(&lane), Arc::clone(&start));
            std::thread::spawn(move || {
                start.wait();
                let mut seen = Vec::new();
                for i in 0..25 {
                    let _held = lane.lock().expect("lane");
                    let capture = CdcCapture::new(&db).expect("CDC is on");
                    let statement = format!("INSERT (:T {{t: {t}, i: {i}}})");
                    let (epoch, ops) = committed(run_txn(&db, &capture, &[statement], false));
                    let [ChangeOp::UpsertNode { props, .. }] = ops.as_slice() else {
                        panic!("one node per transaction, got {ops:?}");
                    };
                    assert_eq!(props.get("t"), Some(&Value::Int64(t)));
                    assert_eq!(props.get("i"), Some(&Value::Int64(i)));
                    let events = db
                        .changes_between(EpochId::new(epoch), EpochId::new(epoch))
                        .expect("cdc");
                    // The create and one update per property, all for this node, all here.
                    assert_eq!(
                        events.len(),
                        3,
                        "this transaction's events at exactly its epoch"
                    );
                    assert!(events.iter().all(|e| e.entity_id == events[0].entity_id));
                    seen.push(epoch);
                }
                seen
            })
        })
        .collect();
    let mut epochs: Vec<u64> = threads
        .into_iter()
        .flat_map(|t| t.join().expect("thread"))
        .collect();
    epochs.sort_unstable();
    assert_eq!(epochs, (1..=100).collect::<Vec<_>>());
}

/// Replay writes do not reach Grafeo's own WAL (R9.5). A persistent engine filled with
/// `apply_to_store` keeps them across a clean `close` (which writes the store to its file) and
/// after `wal_checkpoint`, but not across a crash before either: the engine is leaked here, as a
/// killed process leaves it, and the reopened file has none of the replayed elements. Recovery
/// (Task 14) therefore checkpoints a replayed engine before its local files may stand in for the
/// log.
#[test]
fn replayed_writes_need_a_checkpoint_to_survive_a_crash() {
    let ops = vec![
        ChangeOp::UpsertNode {
            id: 7,
            labels: vec!["P".into()],
            props: PropMap::from([("k".to_string(), Value::Int64(1))]),
        },
        ChangeOp::UpsertNode {
            id: 9,
            labels: vec!["P".into()],
            props: PropMap::new(),
        },
        ChangeOp::UpsertEdge {
            id: 3,
            ty: "R".into(),
            src: 7,
            dst: 9,
            props: PropMap::new(),
        },
    ];
    #[derive(Clone, Copy, Debug)]
    enum End {
        Close,
        Crash,
        CheckpointThenCrash,
    }
    // A crash is simulated by leaking the engine and opening a copy of its directory as it is on
    // disk at that moment (the leaked engine still holds the file lock).
    fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
        std::fs::create_dir_all(to).expect("mkdir");
        for entry in std::fs::read_dir(from).expect("read_dir") {
            let entry = entry.expect("entry");
            let target = to.join(entry.file_name());
            if entry.file_type().expect("type").is_dir() {
                copy_dir(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).expect("copy");
            }
        }
    }
    let reopened = |end: End| {
        let dir = tempfile::tempdir().expect("tempdir");
        let live = dir.path().join("live");
        let after = dir.path().join("after");
        std::fs::create_dir_all(&live).expect("mkdir");
        let db = GrafeoDB::with_config(Config::persistent(live.join("graph.grafeo")).with_cdc())
            .expect("open");
        apply_to_store(&db, &ops).expect("replay");
        match end {
            End::Close => {
                db.close().expect("close");
                drop(db);
            }
            End::Crash => std::mem::forget(db),
            End::CheckpointThenCrash => {
                db.wal_checkpoint().expect("checkpoint");
                std::mem::forget(db);
            }
        }
        copy_dir(&live, &after);
        let db = GrafeoDB::with_config(Config::persistent(after.join("graph.grafeo")).with_cdc())
            .unwrap_or_else(|err| panic!("reopen after {end:?}: {err}"));
        let dumped = dump(&db);
        db.close().expect("close");
        (dumped.nodes.len(), dumped.edges.len())
    };
    assert_eq!(reopened(End::Close), (2, 1));
    assert_eq!(reopened(End::CheckpointThenCrash), (2, 1));
    assert_eq!(
        reopened(End::Crash),
        (0, 0),
        "replayed writes now reach Grafeo's WAL: revisit R9.5"
    );
}
