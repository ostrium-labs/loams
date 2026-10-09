//! What the service cases (`tests/service/`) share: a fake [`NeonApi`], a
//! store of the test binary's backend (`crate::factory()`), and a service
//! on a hand-driven clock.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use loams_pg_control::model::{BranchKey, BranchRec, BranchState};
use loams_pg_control::neon::{LsnAtTime, NeonApi, NeonApiError, TimelineView, WalHeads};
use loams_pg_control::service::{BeforeCommit, Caller, Clock, PgService, ServiceConfig};
use loams_pg_control::{KvControlStore, PgControlStore, StoreOptions};

/// 2026-10-09T00:00:00Z.
pub const T0_MS: u64 = 1_791_590_400_000;

type Tl = ([u8; 16], [u8; 16]);

#[derive(Debug, Default)]
struct Inner {
    timelines: HashMap<Tl, TimelineView>,
    wal: HashMap<Tl, WalHeads>,
    by_time: HashMap<Tl, LsnAtTime>,
    calls: Vec<String>,
}

/// A fake of Neon's components: answers what the test set, records calls.
#[derive(Debug, Clone, Default)]
pub struct FakeNeon {
    inner: Arc<Mutex<Inner>>,
}

impl FakeNeon {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().expect("fake neon lock")
    }

    pub fn set_timeline(&self, t: [u8; 16], tl: [u8; 16], view: TimelineView) {
        self.lock().timelines.insert((t, tl), view);
    }

    pub fn set_wal(&self, t: [u8; 16], tl: [u8; 16], heads: WalHeads) {
        self.lock().wal.insert((t, tl), heads);
    }

    pub fn set_lsn_at_time(&self, t: [u8; 16], tl: [u8; 16], at: LsnAtTime) {
        self.lock().by_time.insert((t, tl), at);
    }

    pub fn calls(&self) -> Vec<String> {
        self.lock().calls.clone()
    }
}

fn missing(what: &str) -> NeonApiError {
    NeonApiError {
        reason: "not_found".into(),
        component: Some("pageserver".into()),
        message: format!("no {what}"),
    }
}

impl NeonApi for FakeNeon {
    async fn timeline(&self, t: [u8; 16], tl: [u8; 16]) -> Result<TimelineView, NeonApiError> {
        let mut inner = self.lock();
        inner.calls.push("timeline".into());
        inner
            .timelines
            .get(&(t, tl))
            .cloned()
            .ok_or_else(|| missing("timeline"))
    }

    async fn lsn_by_timestamp(
        &self,
        t: [u8; 16],
        tl: [u8; 16],
        at_ms: u64,
    ) -> Result<LsnAtTime, NeonApiError> {
        let mut inner = self.lock();
        inner.calls.push(format!("lsn_by_timestamp {at_ms}"));
        inner
            .by_time
            .get(&(t, tl))
            .cloned()
            .ok_or_else(|| missing("timeline"))
    }

    async fn wal_heads(&self, t: [u8; 16], tl: [u8; 16]) -> Result<WalHeads, NeonApiError> {
        let mut inner = self.lock();
        inner.calls.push("wal_heads".into());
        inner
            .wal
            .get(&(t, tl))
            .cloned()
            .ok_or_else(|| NeonApiError {
                reason: "storage_unavailable".into(),
                component: Some("loams_wal".into()),
                message: "no such timeline".into(),
            })
    }
}

/// A clock the test moves.
#[derive(Debug, Clone)]
pub struct TestClock(pub Arc<AtomicU64>);

impl TestClock {
    pub fn advance_ms(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }

    pub fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

pub struct Harness {
    pub service: PgService<FakeNeon>,
    pub store: KvControlStore,
    pub neon: FakeNeon,
    pub clock: TestClock,
}

pub fn options() -> StoreOptions {
    StoreOptions {
        poll: std::time::Duration::from_millis(50),
        ..StoreOptions::default()
    }
}

pub fn config(clock: &TestClock) -> ServiceConfig {
    let at = clock.0.clone();
    ServiceConfig {
        clock: Clock::from_fn(move || at.load(Ordering::SeqCst)),
        ..ServiceConfig::default()
    }
}

pub fn new_clock() -> TestClock {
    TestClock(Arc::new(AtomicU64::new(T0_MS)))
}

/// A fresh store of the binary's backend, and a service on it; `None` when
/// the backend is unavailable (TiKV without `LOAMS_TEST_PD`).
pub async fn harness() -> Option<Harness> {
    Some(harness_on(fresh_store().await?))
}

/// A fresh store of the binary's backend.
pub async fn fresh_store() -> Option<KvControlStore> {
    crate::factory().store(options()).await
}

/// The service on `store`, with a new clock and fake.
pub fn harness_on(store: KvControlStore) -> Harness {
    harness_with(store, None)
}

/// As [`harness_on`], with a hook before each commit.
pub fn harness_with(store: KvControlStore, before_commit: Option<BeforeCommit>) -> Harness {
    let neon = FakeNeon::default();
    let clock = new_clock();
    let config = ServiceConfig {
        before_commit,
        ..config(&clock)
    };
    let service = PgService::new(store.clone(), neon.clone(), config);
    Harness {
        service,
        store,
        neon,
        clock,
    }
}

/// A service on `store` with no hook, its own fake and a clock at
/// [`T0_MS`]: what a hook uses to commit a competing write.
pub fn plain_service(store: &KvControlStore, neon: &FakeNeon) -> PgService<FakeNeon> {
    PgService::new(store.clone(), neon.clone(), config(&new_clock()))
}

/// A hook that runs `f` once, the first time `rpc` is about to commit.
pub fn once_before<F, Fut>(rpc: &'static str, f: F) -> BeforeCommit
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let f = Arc::new(f);
    Arc::new(move |at: &'static str| {
        let run = at == rpc && !fired.swap(true, Ordering::SeqCst);
        let f = f.clone();
        Box::pin(async move {
            if run {
                f().await;
            }
        })
    })
}

/// `let h = harness!();`: the harness, or return (skipped).
macro_rules! harness {
    () => {
        match crate::common::harness().await {
            Some(h) => h,
            None => {
                eprintln!("skipped: no store (LOAMS_TEST_PD unset?)");
                return;
            }
        }
    };
}

pub fn user() -> Caller {
    Caller {
        principal: "user:alice".into(),
        admin: false,
    }
}

pub fn admin() -> Caller {
    Caller {
        principal: "user:root".into(),
        admin: true,
    }
}

/// Moves a branch to `ready`, as the reconciler (Task 7) would.
pub async fn mark_branch_ready(store: &KvControlStore, project_id: &str, branch_id: &str) {
    let key = BranchKey {
        project_id: project_id.into(),
        id: branch_id.into(),
    };
    let got = store
        .get::<BranchRec>(&key)
        .await
        .expect("get")
        .expect("the branch");
    let mut rec = got.record;
    rec.state = BranchState::Ready;
    store
        .api_writer()
        .put(&rec, Some(got.version))
        .await
        .expect("ready");
}
