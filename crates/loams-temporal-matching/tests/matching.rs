//! DX1 Task 7's tests, on the in-process fake Resonate.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use loams_temporal_matching::{Handout, Matcher, QueueKey, Ready, TaskKind};
use loams_temporal_resonate::Claim;
use loams_temporal_resonate::{CreatePromise, Durability, FakeDurability, FakeTaskState};

fn key() -> QueueKey {
    QueueKey {
        namespace_id: "ns".into(),
        task_queue: "q".into(),
        kind: TaskKind::Activity,
    }
}

async fn task(fake: &FakeDurability, id: &str) {
    fake.promise_create(&CreatePromise {
        id: id.into(),
        timeout_at_ms: fake.now() + 3_600_000,
        target_group: Some(key().group()),
        ..Default::default()
    })
    .await
    .expect("create");
}

fn claim(h: Option<Handout>) -> Claim {
    match h.expect("a hand-out") {
        Handout::Claim(c) => c,
        other => panic!("not a claim: {other:?}"),
    }
}

fn setup() -> (Arc<FakeDurability>, Arc<Matcher>) {
    let fake = Arc::new(FakeDurability::new());
    let m = Matcher::new(fake.clone(), "gw-1", 60_000);
    (fake, m)
}

#[test]
fn group_names_are_valid_and_stable() {
    let g = key().group();
    assert!(loams_temporal_resonate::valid_group(&g), "{g}");
    assert_eq!(g, key().group());
    let other = QueueKey {
        kind: TaskKind::Workflow,
        ..key()
    };
    assert_ne!(g, other.group());
}

#[tokio::test]
async fn fifo_oldest_poll_first() {
    let (fake, m) = setup();
    m.ensure_subscribed(&key()).await.expect("subscribe");
    let m1 = m.clone();
    let first = tokio::spawn(async move { m1.poll(&key(), Duration::from_secs(5)).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let m2 = m.clone();
    let second = tokio::spawn(async move { m2.poll(&key(), Duration::from_millis(300)).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    task(&fake, "t1").await;
    let got = claim(first.await.expect("join").expect("poll"));
    assert_eq!(got.task_id, "t1");
    assert!(second.await.expect("join").expect("poll").is_none());
}

#[tokio::test]
async fn backlog_serves_later_poll() {
    let (fake, m) = setup();
    m.ensure_subscribed(&key()).await.expect("subscribe");
    task(&fake, "t1").await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let got = m.poll(&key(), Duration::from_secs(1)).await.expect("poll");
    assert_eq!(claim(got).task_id, "t1");
    assert_eq!(
        fake.task("t1").expect("task").state,
        FakeTaskState::Acquired
    );
}

#[tokio::test]
async fn claim_refused_skips_task() {
    let (fake, m) = setup();
    m.ensure_subscribed(&key()).await.expect("subscribe");
    // A ready task at a version Resonate has moved past, then a good one.
    m.offer(
        &key(),
        Ready {
            task_id: "nope".into(),
            version: 7,
        },
    )
    .await;
    task(&fake, "t2").await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let got = m.poll(&key(), Duration::from_secs(1)).await.expect("poll");
    assert_eq!(claim(got).task_id, "t2");
}

#[tokio::test]
async fn poll_deadline_returns_empty() {
    let (_fake, m) = setup();
    let started = std::time::Instant::now();
    let got = m
        .poll(&key(), Duration::from_millis(100))
        .await
        .expect("poll");
    assert!(got.is_none());
    assert!(started.elapsed() >= Duration::from_millis(100));
}

#[tokio::test]
async fn poll_cancelled_task_not_lost() {
    let (fake, m) = setup();
    m.ensure_subscribed(&key()).await.expect("subscribe");
    let m1 = m.clone();
    let gone = tokio::spawn(async move { m1.poll(&key(), Duration::from_secs(5)).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    gone.abort();
    let _ = gone.await;
    task(&fake, "t3").await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    // Either kept in the backlog or released: never held by nobody.
    let state = fake.task("t3").expect("task").state;
    assert_eq!(state, FakeTaskState::Pending);
    let got = m.poll(&key(), Duration::from_secs(1)).await.expect("poll");
    assert_eq!(claim(got).task_id, "t3");
}

#[tokio::test]
async fn finished_stops_heartbeats_and_held_count() {
    let (fake, m) = setup();
    m.ensure_subscribed(&key()).await.expect("subscribe");
    task(&fake, "t4").await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let claim = claim(m.poll(&key(), Duration::from_secs(1)).await.expect("poll"));
    assert_eq!(m.held(), 1);
    m.heartbeat_once().await.expect("heartbeat");
    m.finished(&claim.task_id);
    assert_eq!(m.held(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_double_handout_under_race() {
    let (fake, m) = setup();
    m.ensure_subscribed(&key()).await.expect("subscribe");
    let n = 300;
    let mut pollers = Vec::new();
    for _ in 0..16 {
        let m = m.clone();
        pollers.push(tokio::spawn(async move {
            let mut got = Vec::new();
            while let Some(h) = m
                .poll(&key(), Duration::from_millis(500))
                .await
                .expect("poll")
            {
                let c = claim(Some(h));
                m.finished(&c.task_id);
                got.push(c.task_id);
            }
            got
        }));
    }
    for i in 0..n {
        task(&fake, &format!("race-{i}")).await;
    }
    let mut all = Vec::new();
    for p in pollers {
        all.extend(p.await.expect("join"));
    }
    let unique: HashSet<_> = all.iter().collect();
    assert_eq!(all.len(), unique.len(), "a task was handed out twice");
    assert_eq!(unique.len(), n, "every task was handed out");
}

#[tokio::test]
async fn local_items_go_to_the_oldest_poll() {
    let (_fake, m) = setup();
    m.ensure_subscribed(&key()).await.expect("subscribe");
    let m1 = m.clone();
    let p = tokio::spawn(async move { m1.poll(&key(), Duration::from_secs(5)).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    m.offer_local(&key(), 42);
    assert_eq!(
        p.await.expect("join").expect("poll"),
        Some(Handout::Local(42))
    );
    // Queued with no poll waiting, taken by the next one, or cancelled.
    m.offer_local(&key(), 7);
    assert!(m.cancel_local(&key(), 7));
    m.offer_local(&key(), 8);
    assert_eq!(
        m.poll(&key(), Duration::from_millis(100))
            .await
            .expect("poll"),
        Some(Handout::Local(8))
    );
}

#[tokio::test]
async fn task_offered_before_the_first_poll_is_kept() {
    let (fake, m) = setup();
    // The relay's fast path offers a task for a group nobody polled yet.
    task(&fake, "early").await;
    m.offer_group(
        &key().group(),
        Ready {
            task_id: "early".into(),
            version: 0,
        },
    )
    .await;
    let got = m.poll(&key(), Duration::from_secs(1)).await.expect("poll");
    assert_eq!(claim(got).task_id, "early");
}
