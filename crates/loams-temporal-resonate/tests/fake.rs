//! [`FakeDurability`]: the profile scenarios on the manual clock, plus its
//! fault injection.
#![allow(clippy::unwrap_used)]

mod common;

use common::{Clock, Names, next};
use loams_temporal_resonate::{
    CreatePromise, Delivery, DeliveryFault, Durability, DurabilityError, FakeDurability,
    FakeTaskState, PENDING_RETRY_TTL_MS, PromiseStatus, SettleAs,
};

fn fake() -> (FakeDurability, Clock, Names) {
    let f = FakeDurability::new();
    (f.clone(), Clock::Fake(f), Names::new())
}

#[tokio::test]
async fn fake_timer_times_out_and_notifies() {
    let (f, c, n) = fake();
    common::timer_times_out_and_notifies(&f, &c, &n).await;
}

#[tokio::test]
async fn fake_lease_expiry_redelivers() {
    let (f, c, n) = fake();
    common::task_lease_expiry_redelivers(&f, &c, &n, "gw-1").await;
}

#[tokio::test]
async fn fake_idempotent_create() {
    let (f, c, n) = fake();
    common::idempotent_create_same_id(&f, &c, &n).await;
    common::idempotent_create_conflicting_param(&f, &c, &n).await;
}

#[tokio::test]
async fn fake_delayed_dispatch() {
    let (f, c, n) = fake();
    common::delayed_create(&f, &c, &n).await;
}

#[tokio::test]
async fn fake_listener_delivery() {
    let (f, c, n) = fake();
    common::listener_delivery(&f, &c, &n).await;
}

#[tokio::test]
async fn fake_settle_canceled() {
    let (f, c, n) = fake();
    common::settle_canceled(&f, &c, &n).await;
}

async fn task(f: &FakeDurability, id: &str, group: &str) {
    f.promise_create(&CreatePromise {
        id: id.into(),
        timeout_at_ms: f.now() + 600_000,
        target_group: Some(group.into()),
        ..Default::default()
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn fake_pending_task_is_redispatched_every_30s() {
    let (f, c, _) = fake();
    let mut rx = f.subscribe("g").await.unwrap();
    task(&f, "wft:a", "g").await;
    assert!(next(&mut rx, &c, 0).await.is_some());
    f.advance(PENDING_RETRY_TTL_MS - 1);
    assert!(rx.try_recv().is_err());
    f.advance(1);
    assert!(matches!(
        rx.try_recv(),
        Ok(Delivery::TaskReady { version: 0, .. })
    ));
    // One advance across two periods fires each at its own time.
    f.advance(2 * PENDING_RETRY_TTL_MS);
    assert!(rx.try_recv().is_ok());
    assert!(rx.try_recv().is_ok());
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn fake_delivery_faults() {
    let (f, _, _) = fake();
    let mut rx = f.subscribe("g").await.unwrap();
    f.fault_next_delivery(DeliveryFault::Drop);
    task(&f, "wft:drop", "g").await;
    assert!(rx.try_recv().is_err(), "dropped");

    f.fault_next_delivery(DeliveryFault::Duplicate);
    task(&f, "wft:dup", "g").await;
    assert_eq!(rx.try_recv().unwrap(), rx.try_recv().unwrap());
    assert!(rx.try_recv().is_err());

    f.fault_next_delivery(DeliveryFault::Delay(500));
    task(&f, "wft:late", "g").await;
    f.advance(499);
    assert!(rx.try_recv().is_err());
    f.advance(1);
    assert!(
        matches!(rx.try_recv(), Ok(Delivery::TaskReady { task_id, .. }) if task_id == "wft:late")
    );
}

#[tokio::test]
async fn fake_unsubscribed_group_loses_messages() {
    let (f, _, _) = fake();
    task(&f, "wft:a", "nobody").await;
    let mut rx = f.subscribe("nobody").await.unwrap();
    assert!(rx.try_recv().is_err(), "sent before the subscription: lost");
    f.advance(PENDING_RETRY_TTL_MS);
    assert!(rx.try_recv().is_ok(), "the redispatch reaches it");
    // A dropped receiver frees the group for a new subscription.
    drop(rx);
    assert!(f.subscribe("nobody").await.is_ok());
}

#[tokio::test]
async fn fake_call_faults() {
    let (f, _, _) = fake();
    let req = CreatePromise {
        id: "tmr:x".into(),
        timeout_at_ms: f.now() + 1_000,
        timer: true,
        ..Default::default()
    };
    f.fail_next_calls(1, false);
    assert!(matches!(
        f.promise_create(&req).await,
        Err(DurabilityError::Unavailable(_))
    ));
    assert_eq!(f.promise_count(), 0, "not applied");
    f.fail_next_calls(1, true);
    assert!(matches!(
        f.promise_create(&req).await,
        Err(DurabilityError::Unavailable(_))
    ));
    assert_eq!(f.promise_count(), 1, "applied, answer lost");
    assert_eq!(
        f.promise_create(&req).await.unwrap().status,
        PromiseStatus::Pending
    );
    assert_eq!(f.calls().len(), 3);
}

#[tokio::test]
async fn fake_lose_all_state() {
    // §74 §5.5 rule 7: Resonate's state is gone; re-emitting the same
    // deterministic creates rebuilds it, and the tasks dispatch again.
    let (f, _, _) = fake();
    let mut rx = f.subscribe("g").await.unwrap();
    task(&f, "wft:a", "g").await;
    let _ = rx.try_recv();
    f.task_acquire("wft:a", 0, "p", 10_000).await.unwrap();
    f.lose_all_state();
    assert_eq!(f.promise_count(), 0);
    assert_eq!(f.promise_get("wft:a").await, Err(DurabilityError::NotFound));
    assert_eq!(
        f.task_fulfill("wft:a", 1, SettleAs::Resolved).await,
        Err(DurabilityError::NotFound)
    );
    task(&f, "wft:a", "g").await;
    assert!(matches!(
        rx.try_recv(),
        Ok(Delivery::TaskReady { version: 0, .. })
    ));
    assert_eq!(f.task("wft:a").unwrap().state, FakeTaskState::Pending);
}

#[tokio::test]
async fn fake_task_promise_times_out_and_fulfils_the_task() {
    let (f, _, _) = fake();
    let mut rx = f.subscribe("g").await.unwrap();
    f.promise_create(&CreatePromise {
        id: "wft:t".into(),
        timeout_at_ms: f.now() + 1_000,
        target_group: Some("g".into()),
        listener_group: Some("g".into()),
        ..Default::default()
    })
    .await
    .unwrap();
    let _ = rx.try_recv();
    f.task_acquire("wft:t", 0, "p", 60_000).await.unwrap();
    f.advance(1_000);
    assert_eq!(
        rx.try_recv().unwrap(),
        Delivery::Settled {
            promise_id: "wft:t".into(),
            status: PromiseStatus::RejectedTimedout
        }
    );
    assert_eq!(f.task("wft:t").unwrap().state, FakeTaskState::Fulfilled);
    // A create whose deadline has passed is settled at once.
    let v = f
        .promise_create(&CreatePromise {
            id: "tmr:past".into(),
            timeout_at_ms: f.now() - 1,
            timer: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(v.status, PromiseStatus::Resolved);
}

#[tokio::test]
async fn fake_sql_family_ignores_delay() {
    // As Resonate's SQLite, Postgres and MySQL stores do at 31ffb201.
    let (f, _, _) = fake();
    f.set_ignores_delay(true);
    assert_eq!(
        f.delay_support(),
        loams_temporal_resonate::DelaySupport::Defer
    );
    let mut rx = f.subscribe("g").await.unwrap();
    f.promise_create(&CreatePromise {
        id: "act:x".into(),
        timeout_at_ms: f.now() + 600_000,
        target_group: Some("g".into()),
        delay_until_ms: Some(f.now() + 5_000),
        ..Default::default()
    })
    .await
    .unwrap();
    assert!(rx.try_recv().is_ok(), "dispatched at create");
}
