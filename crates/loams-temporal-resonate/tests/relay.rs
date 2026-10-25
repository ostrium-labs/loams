//! The outbox relay against the fake (§74 §5.5 rule 2).
#![allow(clippy::unwrap_used)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use loams_temporal_resonate::{
    CreatePromise, Delivery, Durability, FakeDurability, OUTBOX_CONFLICT_METRIC, OutboxRequest,
    OutboxSource, PromiseStatus, Relay, SettleAs,
};

/// An in-memory outbox; `fail_marks` makes the next marks fail, as a crash
/// between the Resonate call and the commit of `dispatched_at` would.
#[derive(Default)]
struct MemOutbox {
    rows: Mutex<Vec<(i64, OutboxRequest, bool)>>,
    fail_marks: AtomicUsize,
}

impl MemOutbox {
    fn push(&self, req: OutboxRequest) {
        let mut rows = self.rows.lock().unwrap();
        let id = rows.len() as i64 + 1;
        rows.push((id, req, false));
    }

    fn undispatched(&self) -> usize {
        self.rows.lock().unwrap().iter().filter(|r| !r.2).count()
    }
}

#[async_trait]
impl OutboxSource for MemOutbox {
    async fn claim(&self, limit: usize) -> Result<Vec<(i64, OutboxRequest)>, String> {
        Ok(self
            .rows
            .lock()
            .unwrap()
            .iter()
            .filter(|r| !r.2)
            .take(limit)
            .map(|r| (r.0, r.1.clone()))
            .collect())
    }

    async fn mark_dispatched(&self, ids: &[i64]) -> Result<(), String> {
        if self
            .fail_marks
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            return Err("crashed before dispatched_at committed".into());
        }
        for r in self.rows.lock().unwrap().iter_mut() {
            if ids.contains(&r.0) {
                r.2 = true;
            }
        }
        Ok(())
    }

    async fn wait(&self) {
        tokio::task::yield_now().await;
    }
}

fn create_task(id: &str, param: &[u8]) -> OutboxRequest {
    OutboxRequest::CreatePromise(CreatePromise {
        id: id.into(),
        timeout_at_ms: loams_temporal_resonate::DEFAULT_START_MS + 600_000,
        param: Some(param.to_vec()),
        target_group: Some("g".into()),
        ..Default::default()
    })
}

fn relay(outbox: &Arc<MemOutbox>, fake: &FakeDurability) -> Relay {
    Relay::new(outbox.clone(), Arc::new(fake.clone()))
}

#[tokio::test]
async fn relay_crash_between_call_and_mark_is_idempotent() {
    let fake = FakeDurability::new();
    let mut rx = fake.subscribe("g").await.unwrap();
    let outbox = Arc::new(MemOutbox::default());
    outbox.push(create_task("wft:a", b"x"));
    outbox.push(OutboxRequest::CreatePromise(CreatePromise {
        id: "tmr:a".into(),
        timeout_at_ms: fake.now() + 1_000,
        timer: true,
        listener_group: Some("g".into()),
        ..Default::default()
    }));
    let relay = relay(&outbox, &fake);

    // The calls are made, then the process dies before marking.
    outbox.fail_marks.store(1, Ordering::SeqCst);
    assert!(relay.run_once().await.is_err());
    assert_eq!(outbox.undispatched(), 2);
    // A Resonate answer lost after it applied the call is the same case.
    fake.fail_next_calls(1, true);
    let pass = relay.run_once().await.unwrap();
    assert!(pass.unavailable);
    assert_eq!(pass.marked, 0);

    // The restarted relay repeats every call: same promises, one dispatch.
    let pass = relay.run_once().await.unwrap();
    assert_eq!((pass.claimed, pass.marked, pass.unavailable), (2, 2, false));
    assert_eq!(outbox.undispatched(), 0);
    assert_eq!(fake.promise_count(), 2);
    assert!(matches!(
        rx.try_recv(),
        Ok(Delivery::TaskReady { version: 0, .. })
    ));
    assert!(rx.try_recv().is_err(), "the repeats dispatched nothing");
    fake.advance(1_000);
    assert_eq!(
        rx.try_recv().unwrap(),
        Delivery::Settled {
            promise_id: "tmr:a".into(),
            status: PromiseStatus::Resolved
        }
    );
    assert!(
        rx.try_recv().is_err(),
        "one listener, however often registered"
    );
    assert_eq!(relay.metrics().conflict_total(), 0);
    assert_eq!(relay.metrics().dispatched_total(), 2 + 2);
}

#[tokio::test]
async fn relay_conflict_counts() {
    let fake = FakeDurability::new();
    let outbox = Arc::new(MemOutbox::default());
    outbox.push(create_task("wft:c", b"first"));
    outbox.push(create_task("wft:c", b"second"));
    outbox.push(OutboxRequest::Settle {
        id: "wft:c".into(),
        state: SettleAs::Rejected,
    });
    let relay = relay(&outbox, &fake);
    let pass = relay.run_once().await.unwrap();
    // The conflicting row is counted, logged and marked: a bug, not a retry.
    assert_eq!((pass.claimed, pass.marked), (3, 3));
    assert_eq!(relay.metrics().conflict_total(), 1);
    assert_eq!(outbox.undispatched(), 0);
    assert_eq!(relay.run_once().await.unwrap().claimed, 0);
    let v = fake.promise_get("wft:c").await.unwrap();
    assert_eq!(v.param.as_deref(), Some(&b"first"[..]));
    assert_eq!(v.status, PromiseStatus::Rejected);
    let text = relay.metrics().render();
    assert!(
        text.contains(&format!("{OUTBOX_CONFLICT_METRIC} 1\n")),
        "{text}"
    );
    assert!(!text.contains('{'), "no labels: {text}");
}

#[tokio::test]
async fn relay_unavailable_retries() {
    let fake = FakeDurability::new();
    let outbox = Arc::new(MemOutbox::default());
    outbox.push(create_task("wft:u", b"x"));
    outbox.push(OutboxRequest::Settle {
        id: "wft:u".into(),
        state: SettleAs::RejectedCanceled,
    });
    let relay = relay(&outbox, &fake);

    fake.fail_next_calls(1, false);
    let pass = relay.run_once().await.unwrap();
    assert!(pass.unavailable);
    assert_eq!(pass.marked, 0, "the settle waits behind its create");
    assert_eq!(outbox.undispatched(), 2);
    assert_eq!(fake.promise_count(), 0);
    assert_eq!(relay.metrics().unavailable_total(), 1);

    let pass = relay.run_once().await.unwrap();
    assert_eq!((pass.marked, pass.unavailable), (2, false));
    assert_eq!(
        fake.promise_get("wft:u").await.unwrap().status,
        PromiseStatus::RejectedCanceled
    );
    let calls = fake.calls();
    assert_eq!(
        calls,
        [
            "promise.create wft:u",
            "promise.create wft:u",
            "promise.settle wft:u",
            "promise.get wft:u"
        ]
    );
}

#[tokio::test]
async fn relay_definite_refusal_is_marked() {
    // A settle of a promise Resonate does not have (state lost, §5.5 rule 7)
    // is a definite answer: marked and counted, never retried forever.
    let fake = FakeDurability::new();
    let outbox = Arc::new(MemOutbox::default());
    outbox.push(OutboxRequest::Settle {
        id: "tmr:gone".into(),
        state: SettleAs::RejectedCanceled,
    });
    let relay = relay(&outbox, &fake);
    let pass = relay.run_once().await.unwrap();
    assert_eq!(pass.marked, 1);
    assert_eq!(relay.metrics().rejected_total(), 1);
}

#[tokio::test]
async fn relay_run_drains_and_stops() {
    let fake = FakeDurability::new();
    let outbox = Arc::new(MemOutbox::default());
    for i in 0..150 {
        outbox.push(create_task(&format!("wft:{i}"), b"x"));
    }
    let relay = relay(&outbox, &fake);
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let o = outbox.clone();
    let watcher = tokio::spawn(async move {
        while o.undispatched() > 0 {
            tokio::task::yield_now().await;
        }
        let _ = stop.send(());
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        relay.run(async {
            let _ = stopped.await;
        }),
    )
    .await
    .unwrap();
    watcher.await.unwrap();
    assert_eq!(fake.promise_count(), 150);
}

#[tokio::test]
async fn relay_defers_delayed_create_when_the_store_ignores_delay() {
    // Resonate's SQL stores dispatch a `resonate:delay` task at create
    // (31ffb201); the relay keeps the row until its time instead.
    let fake = FakeDurability::new();
    fake.set_ignores_delay(true);
    let mut rx = fake.subscribe("g").await.unwrap();
    let outbox = Arc::new(MemOutbox::default());
    let due = fake.now() + 5_000;
    outbox.push(OutboxRequest::CreatePromise(CreatePromise {
        id: "act:r:2".into(),
        timeout_at_ms: fake.now() + 600_000,
        target_group: Some("g".into()),
        delay_until_ms: Some(due),
        ..Default::default()
    }));
    outbox.push(OutboxRequest::Settle {
        id: "act:r:2".into(),
        state: SettleAs::RejectedCanceled,
    });
    outbox.push(create_task("wft:other", b"x"));
    let relay = relay(&outbox, &fake);

    let pass = relay.run_once().await.unwrap();
    assert_eq!((pass.claimed, pass.marked, pass.deferred), (3, 1, 2));
    assert_eq!(pass.next_due_ms, Some(due));
    assert!(
        matches!(rx.try_recv(), Ok(Delivery::TaskReady { task_id, .. }) if task_id == "wft:other")
    );
    assert!(rx.try_recv().is_err(), "the delayed task is not dispatched");
    assert_eq!(
        fake.promise_get("act:r:2").await,
        Err(loams_temporal_resonate::DurabilityError::NotFound)
    );

    fake.advance(4_999);
    assert_eq!(relay.run_once().await.unwrap().deferred, 2);
    fake.advance(1);
    let pass = relay.run_once().await.unwrap();
    assert_eq!((pass.marked, pass.deferred), (2, 0));
    assert!(
        matches!(rx.try_recv(), Ok(Delivery::TaskReady { task_id, .. }) if task_id == "act:r:2")
    );
    // The settle behind it kept its order.
    assert_eq!(
        fake.promise_get("act:r:2").await.unwrap().status,
        PromiseStatus::RejectedCanceled
    );
    assert_eq!(outbox.undispatched(), 0);
}

#[tokio::test]
async fn relay_deferred_rows_do_not_starve_the_outbox() {
    let fake = FakeDurability::new();
    fake.set_ignores_delay(true);
    let outbox = Arc::new(MemOutbox::default());
    for i in 0..100 {
        outbox.push(OutboxRequest::CreatePromise(CreatePromise {
            id: format!("act:d:{i}"),
            timeout_at_ms: fake.now() + 600_000,
            target_group: Some("g".into()),
            delay_until_ms: Some(fake.now() + 60_000),
            ..Default::default()
        }));
    }
    outbox.push(create_task("wft:behind", b"x"));
    let relay = relay(&outbox, &fake).with_config(loams_temporal_resonate::RelayConfig {
        batch: 64,
        ..Default::default()
    });
    assert_eq!(relay.run_once().await.unwrap().deferred, 64);
    let pass = relay.run_once().await.unwrap();
    assert_eq!((pass.deferred, pass.marked), (100, 1));
    assert!(fake.promise_get("wft:behind").await.is_ok());
}
