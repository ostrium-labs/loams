//! What `tests/projects.rs` and `tests/branches.rs` share: a fake
//! [`NeonApi`], a local store and a service on a hand-driven clock.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use loams_pg_control::model::{BranchKey, BranchRec, BranchState};
use loams_pg_control::neon::{LsnAtTime, NeonApi, NeonApiError, TimelineView, WalHeads};
use loams_pg_control::service::{Caller, Clock, PgService, ServiceConfig};
use loams_pg_control::store::conformance::local_factory;
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

/// A fresh local store, and a service on it.
pub async fn harness() -> Harness {
    let store = local_factory(env!("CARGO_TARGET_TMPDIR"))
        .store(options())
        .await
        .expect("a local store");
    harness_on(store)
}

pub fn harness_on(store: KvControlStore) -> Harness {
    let neon = FakeNeon::default();
    let clock = new_clock();
    let service = PgService::new(store.clone(), neon.clone(), config(&clock));
    Harness {
        service,
        store,
        neon,
        clock,
    }
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
