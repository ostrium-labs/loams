//! GR1 Task 9: why Grafeo's `temporal` feature is not the answer to I2 (R9.6).
//!
//! `cargo test -p loams-graph --features grafeo-temporal --test capture_spike_temporal`. The
//! feature exists only to reproduce these measurements; no build Loams ships enables it. With it,
//! Grafeo versions properties and labels per epoch, so a session pinned *below* the current epoch
//! reads the values of its epoch. Three gaps make it unusable for client isolation in 0.5.43:
//!
//! * `GrafeoDB::gc` prunes versions below the oldest transaction, not the oldest pinned session,
//!   and the pinned session then reads a property as `NULL` (and, in the probe of R9.6 with more
//!   label versions, its labels as empty);
//! * a session pinned *at* the current epoch, which is every client session while no write is
//!   pending, reads the latest values, uncommitted ones included (`get_node_at_epoch`'s fast path);
//! * setting a property and deleting its node in one transaction panics in a debug build
//!   (`VersionLog::append` after a leftover `PENDING` entry), and leaves the version log out of
//!   order in a release build.

#![cfg(feature = "grafeo-temporal")]

use grafeo::{Config, GrafeoDB, Role, Session, Value};

fn db() -> GrafeoDB {
    GrafeoDB::with_config(Config::in_memory().with_cdc()).expect("an in-memory database opens")
}

fn rows(session: &Session, statement: &str) -> Vec<Vec<Value>> {
    session
        .execute(statement)
        .unwrap_or_else(|err| panic!("{statement}: {err}"))
        .rows()
        .to_vec()
}

const NODE: &str = "MATCH (n:P {k: 1}) RETURN n.name, labels(n)";

fn pinned(db: &GrafeoDB) -> Session {
    let session = db.session_with_role(Role::ReadOnly);
    session.set_viewing_epoch(db.store().current_epoch());
    session
}

fn name_and_labels(name: Value, labels: &[&str]) -> Vec<Vec<Value>> {
    let labels: Vec<Value> = labels.iter().map(|l| Value::from(*l)).collect();
    vec![vec![name, Value::List(labels.into())]]
}

#[test]
fn canary_temporal_gc_erases_pinned_history() {
    let db = db();
    let writer = db.session();
    writer
        .execute("INSERT (:P {k: 1, name: 'a'})")
        .expect("seed");
    let reader = pinned(&db);
    writer
        .execute("MATCH (n:P {k: 1}) SET n.name = 'b'")
        .expect("set");
    writer
        .execute("MATCH (n:P {k: 1}) SET n.name = 'c', n:Extra")
        .expect("set");
    assert_eq!(
        rows(&reader, NODE),
        name_and_labels(Value::from("a"), &["P"]),
        "a pin below the current epoch reads its epoch"
    );
    db.gc();
    assert_eq!(
        rows(&reader, NODE),
        name_and_labels(Value::Null, &["P"]),
        "temporal GC now respects pinned sessions: revisit R9.6"
    );
}

#[test]
fn canary_temporal_pin_at_current_reads_uncommitted() {
    let db = db();
    db.session()
        .execute("INSERT (:P {k: 1, name: 'a'})")
        .expect("seed");
    let reader = pinned(&db);
    let mut txn = db.session();
    txn.begin_transaction().expect("begin");
    txn.execute("MATCH (n:P {k: 1}) SET n.name = 'DIRTY', n:Dirty")
        .expect("set");
    assert_eq!(
        rows(&reader, NODE),
        name_and_labels(Value::from("DIRTY"), &["Dirty", "P"]),
        "temporal now hides uncommitted writes at the current epoch: revisit R9.6"
    );
    txn.rollback().expect("rollback");
}

/// Debug builds only: the panic is a `debug_assert!`, and a release build corrupts the version
/// log's order silently instead.
#[test]
#[cfg(debug_assertions)]
fn canary_temporal_set_then_delete_panics() {
    let db = db();
    db.session().execute("INSERT (:P {k: 3})").expect("seed");
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut txn = db.session();
        txn.begin_transaction().expect("begin");
        txn.execute("MATCH (n {k: 3}) SET n.x = 0").expect("set");
        txn.execute("MATCH (n {k: 3}) DELETE n").expect("delete");
        txn.commit()
    }));
    assert!(
        outcome.is_err(),
        "set-then-delete under temporal no longer panics: revisit R9.6"
    );
}
