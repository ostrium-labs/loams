//! Leader election and self-fence (OP2-A Task 3, D1180). All tests run on paused
//! tokio time, so seconds cost nothing.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use loams_kube::{
    LeaderExit, LeaderToken, LeaseConfig, LeaseStore, MemLeaseStore, TokioClock, run_leader,
};
use tokio::time::{Instant, sleep};

fn cfg(identity: &str) -> LeaseConfig {
    LeaseConfig::new("loams-system", identity)
}

/// Counts live leaders and remembers the highest count.
#[derive(Default)]
struct Alive {
    now: AtomicUsize,
    max: AtomicUsize,
}

struct Guard(Arc<Alive>);

impl Alive {
    fn enter(self: &Arc<Self>) -> Guard {
        let n = self.now.fetch_add(1, Ordering::SeqCst) + 1;
        self.max.fetch_max(n, Ordering::SeqCst);
        Guard(self.clone())
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.0.now.fetch_sub(1, Ordering::SeqCst);
    }
}

#[tokio::test(start_paused = true)]
async fn single_leader_under_contention() {
    let store = MemLeaseStore::new();
    let alive = Arc::new(Alive::default());
    let led = Arc::new(Mutex::new(Vec::new()));
    let mut tasks = Vec::new();
    for id in ["a", "b", "c"] {
        let (store, alive, led) = (store.clone(), alive.clone(), led.clone());
        tasks.push(tokio::spawn(async move {
            run_leader(
                cfg(id),
                store,
                TokioClock::new(),
                |_token: LeaderToken| async move {
                    let _g = alive.enter();
                    led.lock().unwrap().push(id);
                    sleep(Duration::from_secs(20)).await;
                },
            )
            .await
        }));
    }
    // Sample at every tick of 100 ms while the three take turns.
    let sampler = {
        let alive = alive.clone();
        tokio::spawn(async move {
            for _ in 0..2000 {
                assert!(alive.now.load(Ordering::SeqCst) <= 1, "two leaders at once");
                sleep(Duration::from_millis(100)).await;
            }
        })
    };
    for t in tasks {
        assert_eq!(t.await.unwrap(), LeaderExit::Shutdown);
    }
    sampler.abort();
    assert_eq!(alive.max.load(Ordering::SeqCst), 1);
    let mut led = led.lock().unwrap().clone();
    led.sort_unstable();
    assert_eq!(led, ["a", "b", "c"], "every candidate led once");
}

#[tokio::test(start_paused = true)]
async fn self_fence_after_missed_renewal() {
    let store = MemLeaseStore::new();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let s = store.clone();
    let leader = tokio::spawn(async move {
        run_leader(
            cfg("a"),
            s,
            TokioClock::new(),
            |token: LeaderToken| async move {
                let _ = tx.send(token.lost.clone());
                token.lost.cancelled().await;
                // The work is dropped at the fence and never gets here.
                unreachable!("work must be dropped, not resumed, at the fence");
            },
        )
        .await
    });
    let lost = rx.await.unwrap();
    sleep(Duration::from_secs(3)).await;
    assert!(!lost.is_cancelled());

    // The store stalls for 11 s, longer than the 10 s renew deadline.
    let stalled_at = Instant::now();
    store.set_stall(Some(Duration::from_secs(11)));
    lost.cancelled().await;
    let fenced_after = stalled_at.elapsed();
    assert!(
        fenced_after <= Duration::from_secs(10) && fenced_after >= Duration::from_secs(8),
        "fenced {fenced_after:?} after the stall began"
    );
    assert_eq!(leader.await.unwrap(), LeaderExit::LostLease);
}

#[tokio::test(start_paused = true)]
async fn lease_transitions_increase() {
    let store = MemLeaseStore::new();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let a = tokio::spawn({
        let store = store.clone();
        async move {
            run_leader(
                cfg("a"),
                store,
                TokioClock::new(),
                |t: LeaderToken| async move {
                    let _ = tx.send(t.transitions);
                    sleep(Duration::from_secs(5)).await;
                },
            )
            .await
        }
    });
    let a_transitions = rx.await.unwrap();
    assert_eq!(a.await.unwrap(), LeaderExit::Shutdown);

    let (tx, rx) = tokio::sync::oneshot::channel();
    let b = tokio::spawn({
        let store = store.clone();
        async move {
            run_leader(
                cfg("b"),
                store,
                TokioClock::new(),
                |t: LeaderToken| async move {
                    let _ = tx.send(t.transitions);
                },
            )
            .await
        }
    });
    let b_transitions = rx.await.unwrap();
    assert_eq!(b.await.unwrap(), LeaderExit::Shutdown);
    assert_eq!(b_transitions, a_transitions + 1);
    let spec = store.snapshot().unwrap().spec.unwrap();
    assert_eq!(spec.lease_transitions, Some(b_transitions));
}

#[tokio::test(start_paused = true)]
async fn expired_lease_taken_only_after_lease_duration() {
    let store = MemLeaseStore::new();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let a = tokio::spawn({
        let store = store.clone();
        async move {
            run_leader(
                cfg("a"),
                store,
                TokioClock::new(),
                |_t: LeaderToken| async move {
                    let _ = tx.send(());
                    std::future::pending::<()>().await;
                },
            )
            .await
        }
    });
    rx.await.unwrap();
    sleep(Duration::from_secs(5)).await;
    // The holder goes silent: its task stops, with no release.
    a.abort();
    let _ = a.await;
    let silent_since = Instant::now();

    let (tx, rx) = tokio::sync::oneshot::channel();
    let b = tokio::spawn({
        let store = store.clone();
        async move {
            run_leader(
                cfg("b"),
                store,
                TokioClock::new(),
                |_t: LeaderToken| async move {
                    let _ = tx.send(Instant::now());
                },
            )
            .await
        }
    });
    // Not before 15 s.
    sleep(Duration::from_secs(14)).await;
    assert_eq!(
        store
            .snapshot()
            .unwrap()
            .spec
            .unwrap()
            .holder_identity
            .as_deref(),
        Some("a"),
        "B took the lease before it expired"
    );
    let took = rx.await.unwrap().duration_since(silent_since);
    assert!(took >= Duration::from_secs(15), "B took it after {took:?}");
    assert!(
        took <= Duration::from_secs(15 + 2 + 2),
        "B took it after {took:?}"
    );
    assert_eq!(b.await.unwrap(), LeaderExit::Shutdown);
}

#[tokio::test(start_paused = true)]
async fn replace_with_stale_resource_version_conflicts() {
    let store = MemLeaseStore::new();
    let l = k8s_openapi::api::coordination::v1::Lease::default();
    let created = store.create(&l).await.unwrap();
    assert!(store.create(&l).await.is_err());
    let updated = store.replace(&created).await.unwrap();
    assert!(matches!(
        store.replace(&created).await,
        Err(loams_kube::LeaseError::Conflict)
    ));
    store.replace(&updated).await.unwrap();
}
