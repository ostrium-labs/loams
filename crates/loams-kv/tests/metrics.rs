//! Operation metrics and the observer (KV1 plan KV0 Task 7, review focus
//! 5), on the embedded store (TiKV records through the same `Store::run`).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use loams_kv::metrics::{KvObserver, Outcome, op_metrics};
use loams_kv::{Backend, Fault, FaultPlan, FaultPoint, Store, TxnError, TxnOptions, testing};

async fn store() -> (testing::Factory, Store) {
    let factory = testing::embedded_factory(env!("CARGO_TARGET_TMPDIR"));
    let store = factory
        .store(testing::Spec::fresh())
        .await
        .expect("an embedded store");
    (factory, store)
}

/// One fault at one point of every attempt of one op.
#[derive(Debug)]
struct Always(&'static str, FaultPoint, Fault);

impl FaultPlan for Always {
    fn at(&self, op: &str, point: FaultPoint, _attempt: u32) -> Option<Fault> {
        (op == self.0 && point == self.1).then_some(self.2)
    }
}

fn once(op: &'static str) -> TxnOptions {
    let mut opts = TxnOptions::new(op);
    opts.max_attempts = 1;
    opts
}

fn count(op: &str, outcome: Outcome) -> u64 {
    op_metrics().ops_total(Backend::Embedded, op, outcome)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metrics_count_each_outcome() {
    let (_factory, store) = store().await;
    // ok
    store
        .run(once("kv.test.m.ok"), |txn| {
            Box::pin(async move { txn.put(b"k", b"v".to_vec()).await })
        })
        .await
        .expect("a put");
    // already_exists
    let r = store
        .run(once("kv.test.m.exists"), |txn| {
            Box::pin(async move { txn.insert(b"k", b"x".to_vec()).await })
        })
        .await;
    assert!(matches!(r, Err(TxnError::AlreadyExists(_))), "{r:?}");
    // fatal
    let r = store
        .run(once("kv.test.m.fatal"), |_| {
            Box::pin(async move { Err::<(), _>(TxnError::Fatal("planted".into())) })
        })
        .await;
    assert!(matches!(r, Err(TxnError::Fatal(_))), "{r:?}");
    // conflict: the body asks for a restart, and one attempt is all it has
    let r = store
        .run(once("kv.test.m.conflict"), |_| {
            Box::pin(async move { Err::<(), _>(TxnError::Conflict) })
        })
        .await;
    assert_eq!(r, Err(TxnError::Conflict));
    // deadline
    let mut opts = once("kv.test.m.deadline");
    opts.deadline = Duration::from_millis(50);
    let r = store
        .run(opts, |_| {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_secs(5)).await;
                Ok(())
            })
        })
        .await;
    assert_eq!(r, Err(TxnError::Deadline));
    // not_applied: refused before the begin
    let refused = store.clone().with_faults(Arc::new(Always(
        "kv.test.m.not_applied",
        FaultPoint::BeforeBegin,
        Fault::Refuse,
    )));
    let r = refused
        .run(once("kv.test.m.not_applied"), |txn| {
            Box::pin(async move { txn.put(b"n", b"v".to_vec()).await })
        })
        .await;
    assert!(matches!(r, Err(TxnError::NotApplied(_))), "{r:?}");
    // undetermined: the acknowledgement of a commit is lost, and there is
    // no commit token to resolve it
    let lost = store.clone().with_faults(Arc::new(Always(
        "kv.test.m.undetermined",
        FaultPoint::AfterCommit,
        Fault::LoseAck,
    )));
    let r = lost
        .run(once("kv.test.m.undetermined"), |txn| {
            Box::pin(async move { txn.put(b"u", b"v".to_vec()).await })
        })
        .await;
    assert!(matches!(r, Err(TxnError::Undetermined { .. })), "{r:?}");

    for (op, outcome) in [
        ("kv.test.m.ok", Outcome::Ok),
        ("kv.test.m.exists", Outcome::AlreadyExists),
        ("kv.test.m.fatal", Outcome::Fatal),
        ("kv.test.m.conflict", Outcome::Conflict),
        ("kv.test.m.deadline", Outcome::Deadline),
        ("kv.test.m.not_applied", Outcome::NotApplied),
        ("kv.test.m.undetermined", Outcome::Undetermined),
    ] {
        assert_eq!(count(op, outcome), 1, "{op} {outcome}");
        for other in Outcome::ALL.into_iter().filter(|o| *o != outcome) {
            assert_eq!(count(op, other), 0, "{op} {other}");
        }
        assert_eq!(
            op_metrics().op_seconds_count(Backend::Embedded, op),
            1,
            "{op}"
        );
    }
    let text = op_metrics().render();
    assert!(
        text.contains(
            "loams_kv_ops_total{backend=\"embedded\",op=\"kv.test.m.deadline\",outcome=\"deadline\"} 1"
        ),
        "{text}"
    );
}

/// Every label of the rendered registry is `backend`, `op`, `outcome` or
/// `le`: no keyspace, namespace, tenant or root (D1021).
#[tokio::test]
async fn metrics_have_no_tenant_label() {
    let (_factory, store) = store().await;
    store
        .run(TxnOptions::new("kv.test.m.labels"), |txn| {
            Box::pin(async move { txn.put(b"k", b"v".to_vec()).await })
        })
        .await
        .expect("a put");
    let text = op_metrics().render();
    let mut names = std::collections::BTreeSet::new();
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        let (Some(open), Some(close)) = (line.find('{'), line.rfind('}')) else {
            continue;
        };
        for pair in line[open + 1..close].split("\",") {
            let name = pair.split('=').next().unwrap_or_default();
            names.insert(name.to_string());
        }
    }
    assert!(!names.is_empty(), "{text}");
    for forbidden in ["keyspace", "namespace", "tenant", "root"] {
        assert!(!names.contains(forbidden), "label {forbidden} in {text}");
    }
    let allowed: std::collections::BTreeSet<String> = ["backend", "op", "outcome", "le"]
        .into_iter()
        .map(String::from)
        .collect();
    assert!(names.is_subset(&allowed), "{names:?}");
    // And no value is the store's keyspace or root.
    assert!(
        !text.contains(store.keyspace()),
        "the keyspace leaked into {text}"
    );
}

#[derive(Debug, Default)]
struct Seen(Mutex<Vec<(String, &'static str, Outcome)>>);

impl KvObserver for Seen {
    fn on_op(&self, keyspace: &str, op: &'static str, outcome: Outcome, _elapsed: Duration) {
        self.0
            .lock()
            .expect("unpoisoned")
            .push((keyspace.to_string(), op, outcome));
    }
}

#[tokio::test]
async fn observer_sees_keyspace() {
    let (_factory, store) = store().await;
    let seen = Arc::new(Seen::default());
    let observed = store.with_observer(seen.clone());
    observed
        .run(TxnOptions::new("kv.test.observed"), |txn| {
            Box::pin(async move { txn.put(b"k", b"v".to_vec()).await })
        })
        .await
        .expect("a put");
    let r = observed
        .clone()
        .run(once("kv.test.observed.insert"), |txn| {
            Box::pin(async move { txn.insert(b"k", b"x".to_vec()).await })
        })
        .await;
    assert!(r.is_err());
    let seen = seen.0.lock().expect("unpoisoned").clone();
    let ks = observed.keyspace().to_string();
    assert_eq!(
        seen,
        vec![
            (ks.clone(), "kv.test.observed", Outcome::Ok),
            (ks, "kv.test.observed.insert", Outcome::AlreadyExists),
        ]
    );
}
