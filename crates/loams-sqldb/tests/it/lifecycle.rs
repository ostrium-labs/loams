//! The lifecycle host (plan SQ1 Task 5): suspend and resume sagas over
//! `FakeRuntime`, crashes at every step, concurrent suspend and connect,
//! and the idle detector.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use loams_sqldb::model::{BranchId, Class};
use loams_sqldb::runtime::fake::{Call, FakeRuntime};
use loams_sqldb::runtime::{
    JobOutcome, JobSpec, Member, MemberState, PoolStatus, RuntimeError, SqlRuntime,
};
use loams_sqldb::sagas::{
    Admission, Close, EnsureError, FaultPoint, HostConfig, Lifecycles, MemoryStore, Prober,
};
use loams_sqlrouter::machines::lifecycle::{Record, State, Step};
use tokio::sync::Notify;

fn br(n: u8) -> BranchId {
    BranchId::parse(&format!("br_00000000000000{n:02}")).expect("id")
}

/// Answers `SELECT 1` for any ready member, unless told to fail.
#[derive(Debug, Default)]
struct FakeProber {
    fail: std::sync::atomic::AtomicBool,
    probes: AtomicUsize,
}

#[async_trait]
impl Prober for FakeProber {
    async fn probe(&self, _branch: &BranchId, member: &Member) -> Result<(), String> {
        self.probes.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) || member.state != MemberState::Ready {
            return Err("probe failed".into());
        }
        Ok(())
    }
}

/// FakeRuntime whose `scale(_, 0)` can be held until released.
#[derive(Debug)]
struct Gated {
    inner: Arc<FakeRuntime>,
    hold_scale_down: std::sync::atomic::AtomicBool,
    entered: Notify,
    release: Notify,
}

#[async_trait]
impl SqlRuntime for Gated {
    async fn ensure_pool(
        &self,
        b: &BranchId,
        c: Class,
        n: u32,
    ) -> Result<PoolStatus, RuntimeError> {
        self.inner.ensure_pool(b, c, n).await
    }
    async fn scale(&self, b: &BranchId, n: u32) -> Result<PoolStatus, RuntimeError> {
        if n == 0 && self.hold_scale_down.load(Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        self.inner.scale(b, n).await
    }
    async fn pool_status(&self, b: &BranchId) -> Result<Option<PoolStatus>, RuntimeError> {
        self.inner.pool_status(b).await
    }
    async fn delete_pool(&self, b: &BranchId) -> Result<(), RuntimeError> {
        self.inner.delete_pool(b).await
    }
    async fn run_job(&self, spec: &JobSpec) -> Result<JobOutcome, RuntimeError> {
        self.inner.run_job(spec).await
    }
}

struct Setup {
    host: Lifecycles,
    rt: Arc<FakeRuntime>,
    store: Arc<MemoryStore>,
    prober: Arc<FakeProber>,
}

async fn setup(state: State, config: HostConfig) -> Setup {
    let rt = Arc::new(FakeRuntime::new());
    setup_with(rt.clone(), rt, state, config).await
}

async fn setup_with(
    rt: Arc<FakeRuntime>,
    runtime: Arc<dyn SqlRuntime>,
    state: State,
    config: HostConfig,
) -> Setup {
    let replicas = u32::from(state == State::Running);
    rt.ensure_pool(&br(1), Class::Xs, replicas)
        .await
        .expect("pool");
    let store = Arc::new(MemoryStore::default());
    let prober = Arc::new(FakeProber::default());
    let host = Lifecycles::new(runtime, store.clone(), prober.clone(), config);
    host.register(&br(1), state).await.expect("register");
    Setup {
        host,
        rt,
        store,
        prober,
    }
}

fn quick() -> HostConfig {
    HostConfig {
        suspend_after: Duration::ZERO,
        ..HostConfig::default()
    }
}

async fn wait_state(host: &Lifecycles, b: &BranchId, state: State) {
    let r = tokio::time::timeout(
        Duration::from_secs(120),
        host.wait_for(b, |r| r.state == state && r.step == Step::None),
    )
    .await;
    assert!(
        r.is_ok(),
        "{b} never reached {state:?}: {:?}",
        host.record(b)
    );
}

fn replicas(rt: &FakeRuntime) -> u32 {
    rt.calls()
        .iter()
        .rev()
        .find_map(|c| match c {
            Call::Scale(_, n) | Call::EnsurePool(_, _, n) => Some(*n),
            _ => None,
        })
        .unwrap_or(0)
}

/// A session's lease stays open until the test drops it; `closing` is
/// watched in the background.
fn watch_closing(mut a: Admission) -> (Arc<Mutex<Vec<Close>>>, tokio::task::JoinHandle<()>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    let h = tokio::spawn(async move {
        loop {
            let c = a.lease.closing().await;
            s.lock().expect("seen").push(c);
            if c == Close::Now {
                return;
            }
        }
    });
    (seen, h)
}

#[tokio::test(start_paused = true)]
async fn suspended_branch_wakes_on_connect() {
    let s = setup(State::Suspended, quick()).await;
    let a = s.host.ensure_running(&br(1)).await.expect("admitted");
    assert_eq!(a.members.len(), 1);
    assert_eq!(a.members[0].state, MemberState::Ready);
    assert_eq!(s.host.record(&br(1)).map(|r| r.state), Some(State::Running));
    assert_eq!(replicas(&s.rt), 1);
    // Stored.
    assert_eq!(s.store.get(&br(1)).map(|r| r.state), Some(State::Running));
}

#[tokio::test(start_paused = true)]
async fn resume_deadline_is_resuming_error() {
    let s = setup(State::Suspended, quick()).await;
    s.prober.fail.store(true, Ordering::SeqCst);
    let t0 = tokio::time::Instant::now();
    let e = s
        .host
        .ensure_running(&br(1))
        .await
        .expect_err("held, then refused");
    assert_eq!(e, EnsureError::Resuming);
    let waited = t0.elapsed();
    assert!(
        waited >= Duration::from_secs(30) && waited < Duration::from_secs(32),
        "{waited:?}"
    );
    // The resume goes on; the next connection is admitted once it can be.
    s.prober.fail.store(false, Ordering::SeqCst);
    s.host.ensure_running(&br(1)).await.expect("admitted");
}

#[tokio::test(start_paused = true)]
async fn suspend_closes_idle_then_kills_after_30s() {
    let s = setup(State::Running, quick()).await;
    let a = s.host.ensure_running(&br(1)).await.expect("admitted");
    let (seen, h) = watch_closing(a);
    let t0 = tokio::time::Instant::now();
    s.host.suspend_now(&br(1));
    tokio::time::timeout(Duration::from_secs(60), h)
        .await
        .expect("killed")
        .expect("join");
    assert_eq!(*seen.lock().expect("seen"), [Close::WhenIdle, Close::Now]);
    assert!(t0.elapsed() >= Duration::from_secs(30));
    wait_state(&s.host, &br(1), State::Suspended).await;
    assert_eq!(replicas(&s.rt), 0);
}

/// Crash points, counted across a run: a crash before a record is stored,
/// before a runtime or probe call, and after its effect (its result lost).
fn crash_at(host: &Lifecycles, k: usize) -> Arc<AtomicUsize> {
    let seen = Arc::new(AtomicUsize::new(0));
    let s = seen.clone();
    host.set_fault(Arc::new(move |_p: &FaultPoint| {
        s.fetch_add(1, Ordering::SeqCst) == k
    }));
    seen
}

#[tokio::test(start_paused = true)]
async fn suspend_resumes_after_crash_at_each_step() {
    let mut k = 0;
    loop {
        let s = setup(State::Running, quick()).await;
        let seen = crash_at(&s.host, k);
        // A busy session the suspend must kill, and an idle one it closes.
        let busy = s.host.ensure_running(&br(1)).await.expect("busy");
        let idle = s.host.ensure_running(&br(1)).await.expect("idle");
        let (seen_busy, h) = watch_closing(busy);
        let mut idle = idle;
        let idle_closed = tokio::spawn(async move {
            let c = idle.lease.closing().await;
            drop(idle);
            c
        });
        // A crash before the first record is stored loses the Idle: the
        // detector would fire again, so the test does too.
        for _ in 0..3 {
            s.host.suspend_now(&br(1));
            tokio::time::sleep(Duration::from_millis(10)).await;
            if s.host
                .record(&br(1))
                .is_some_and(|r| r.state != State::Running)
            {
                break;
            }
        }
        wait_state(&s.host, &br(1), State::Suspended).await;
        assert!(
            h.is_finished()
                || tokio::time::timeout(Duration::from_secs(1), h)
                    .await
                    .is_ok()
        );
        assert_eq!(
            seen_busy.lock().expect("seen").last(),
            Some(&Close::Now),
            "k={k}"
        );
        assert!(idle_closed.await.is_ok(), "k={k}");
        assert_eq!(replicas(&s.rt), 0, "k={k}");
        let st =
            s.rt.pool_status(&br(1))
                .await
                .expect("status")
                .expect("pool");
        assert!(st.members.is_empty(), "k={k}");
        assert_eq!(s.store.get(&br(1)).map(|r| r.state), Some(State::Suspended));
        let crashed = s.host.restarts(&br(1));
        if seen.load(Ordering::SeqCst) <= k {
            assert_eq!(crashed, 0);
            assert!(k > 3, "too few crash points: {k}");
            println!("crash points: {k}");
            break;
        }
        assert_eq!(crashed, 1, "k={k}: one crash, one recovery");
        k += 1;
    }
}

#[tokio::test(start_paused = true)]
async fn resume_resumes_after_crash_at_each_step() {
    let mut k = 0;
    loop {
        let s = setup(State::Suspended, quick()).await;
        let seen = crash_at(&s.host, k);
        let a = s.host.ensure_running(&br(1)).await;
        let a = match a {
            Ok(a) => a,
            // A crash may cost the first connection its 30 s; the next
            // one is admitted.
            Err(EnsureError::Resuming) => s.host.ensure_running(&br(1)).await.expect("admitted"),
            Err(e) => panic!("k={k}: {e:?}"),
        };
        assert_eq!(a.members.len(), 1, "k={k}");
        let st =
            s.rt.pool_status(&br(1))
                .await
                .expect("status")
                .expect("pool");
        assert_eq!(st.ready(), 1, "k={k}: admitted to a running pool");
        assert_eq!(
            s.store.get(&br(1)).map(|r| r.state),
            Some(State::Running),
            "k={k}"
        );
        let crashed = s.host.restarts(&br(1));
        if seen.load(Ordering::SeqCst) <= k {
            assert_eq!(crashed, 0);
            assert!(k > 3, "too few crash points: {k}");
            println!("crash points: {k}");
            break;
        }
        assert_eq!(crashed, 1, "k={k}");
        k += 1;
    }
}

/// Review Focus 4: a connection racing a suspend is never lost or sent to
/// a stopped pool. Before the scale-down it aborts the suspend; after, the
/// suspend finishes and the connection wakes the branch.
#[tokio::test(start_paused = true)]
async fn concurrent_suspend_and_connect_one_wins() {
    // The connection wins: a busy session holds the suspend in quiesce.
    let s = setup(State::Running, quick()).await;
    let busy = s.host.ensure_running(&br(1)).await.expect("busy");
    let (seen, _h) = watch_closing(busy);
    s.host.suspend_now(&br(1));
    s.host.wait_for(&br(1), |r| r.step == Step::Quiesce).await;
    let a = s
        .host
        .ensure_running(&br(1))
        .await
        .expect("the connection wins");
    assert_eq!(a.members.len(), 1);
    assert_eq!(s.host.record(&br(1)).map(|r| r.state), Some(State::Running));
    assert!(
        !s.rt.calls().contains(&Call::Scale(br(1), 0)),
        "no scale-down"
    );
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(*seen.lock().expect("seen"), [Close::WhenIdle, Close::Open]);
    drop(a);

    // The suspend wins: the connection arrives during the scale-down.
    let rt = Arc::new(FakeRuntime::new());
    let gated = Arc::new(Gated {
        inner: rt.clone(),
        hold_scale_down: true.into(),
        entered: Notify::new(),
        release: Notify::new(),
    });
    let s = setup_with(rt.clone(), gated.clone(), State::Running, quick()).await;
    s.host.suspend_now(&br(1));
    gated.entered.notified().await;
    let host = s.host.clone();
    let connect = tokio::spawn(async move { host.ensure_running(&br(1)).await });
    s.host.wait_for(&br(1), |r| r.step == Step::ScaleDown).await;
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(!connect.is_finished(), "held during the scale-down");
    gated.release.notify_one();
    let a = connect
        .await
        .expect("join")
        .expect("admitted after the resume");
    assert_eq!(a.members.len(), 1);
    let scales: Vec<u32> = rt
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::Scale(_, n) => Some(n),
            _ => None,
        })
        .collect();
    assert_eq!(scales, [0, 1], "suspend finished, then resumed");
    let st = rt.pool_status(&br(1)).await.expect("status").expect("pool");
    assert_eq!(st.ready(), 1);

    // Many at once, with suspends at random times: every admission lands on
    // a running pool, every other connection gets Resuming.
    let s = setup(State::Running, quick()).await;
    let mut tasks = Vec::new();
    for i in 0..40u64 {
        let host = s.host.clone();
        let rt = s.rt.clone();
        tasks.push(tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(i * 37 % 500)).await;
            if i % 5 == 0 {
                host.suspend_now(&br(1));
                return;
            }
            match host.ensure_running(&br(1)).await {
                Ok(a) => {
                    let st = rt.pool_status(&br(1)).await.expect("status").expect("pool");
                    assert_eq!(st.ready(), 1, "admitted to a stopped pool");
                    tokio::time::sleep(Duration::from_millis(i * 13 % 200)).await;
                    drop(a);
                }
                Err(e) => assert_eq!(e, EnsureError::Resuming),
            }
        }));
    }
    for t in tasks {
        t.await.expect("task");
    }
}

#[tokio::test(start_paused = true)]
async fn zero_suspend_after_never_suspends() {
    let s = setup(State::Running, quick()).await;
    s.host.set_suspend_after(&br(1), Duration::ZERO);
    tokio::time::sleep(Duration::from_secs(24 * 3600)).await;
    assert_eq!(s.host.record(&br(1)).map(|r| r.state), Some(State::Running));
    assert!(!s.rt.calls().contains(&Call::Scale(br(1), 0)));
}

#[tokio::test(start_paused = true)]
async fn idle_database_suspends_after_suspend_after() {
    let config = HostConfig {
        suspend_after: Duration::from_secs(300),
        ..HostConfig::default()
    };
    let s = setup(State::Running, config).await;
    // Activity keeps it running.
    for _ in 0..10 {
        tokio::time::sleep(Duration::from_secs(60)).await;
        s.host.activity(&br(1));
    }
    assert_eq!(s.host.record(&br(1)).map(|r| r.state), Some(State::Running));
    tokio::time::sleep(Duration::from_secs(290)).await;
    assert_eq!(s.host.record(&br(1)).map(|r| r.state), Some(State::Running));
    tokio::time::sleep(Duration::from_secs(15)).await;
    wait_state(&s.host, &br(1), State::Suspended).await;
    assert_eq!(replicas(&s.rt), 0);
    // Woken, it gets a full suspend_after again.
    let a = s.host.ensure_running(&br(1)).await.expect("woken");
    drop(a);
    tokio::time::sleep(Duration::from_secs(290)).await;
    assert_eq!(s.host.record(&br(1)).map(|r| r.state), Some(State::Running));
    tokio::time::sleep(Duration::from_secs(15)).await;
    wait_state(&s.host, &br(1), State::Suspended).await;
}

#[tokio::test(start_paused = true)]
async fn unknown_branch_and_other_states() {
    let s = setup(State::Running, quick()).await;
    assert_eq!(
        s.host.ensure_running(&br(9)).await.map(|_| ()),
        Err(EnsureError::UnknownBranch)
    );
    s.host
        .register(&br(2), State::Creating)
        .await
        .expect("register");
    assert_eq!(
        s.host.ensure_running(&br(2)).await.map(|_| ()),
        Err(EnsureError::Unavailable)
    );
    // Registering again keeps the stored record.
    s.host
        .register(&br(1), State::Suspended)
        .await
        .expect("again");
    assert_eq!(s.host.record(&br(1)).map(|r| r.state), Some(State::Running));
    let _ = Record::new(State::Running);
}
