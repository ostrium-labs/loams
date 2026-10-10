//! What the service cases (`tests/service/`) share: a fake [`NeonApi`], a
//! store of the test binary's backend (`crate::factory()`), an in-memory
//! secret store, and a service on a hand-driven clock.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use loams_pg_control::model::{BranchKey, BranchRec, BranchState};
use loams_pg_control::neon::{
    Component, LsnAtTime, NeonApiError, NeonRead, TenantId, TimelineId, TimelineView, WalHeads,
};
use loams_pg_control::secrets::{Secret, SecretError, SecretRef, SecretStore};
use loams_pg_control::service::Reason;
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
        reason: Reason::NotFound,
        component: Some(Component::Pageserver),
        message: format!("no {what}"),
    }
}

impl NeonRead for FakeNeon {
    async fn timeline(&self, t: TenantId, tl: TimelineId) -> Result<TimelineView, NeonApiError> {
        let mut inner = self.lock();
        inner.calls.push("timeline".into());
        inner
            .timelines
            .get(&(t.0, tl.0))
            .cloned()
            .ok_or_else(|| missing("timeline"))
    }

    async fn lsn_by_timestamp(
        &self,
        t: TenantId,
        tl: TimelineId,
        at_ms: u64,
    ) -> Result<LsnAtTime, NeonApiError> {
        let mut inner = self.lock();
        inner.calls.push(format!("lsn_by_timestamp {at_ms}"));
        inner
            .by_time
            .get(&(t.0, tl.0))
            .cloned()
            .ok_or_else(|| missing("timeline"))
    }

    async fn wal_heads(&self, t: TenantId, tl: TimelineId) -> Result<WalHeads, NeonApiError> {
        let mut inner = self.lock();
        inner.calls.push("wal_heads".into());
        inner
            .wal
            .get(&(t.0, tl.0))
            .cloned()
            .ok_or_else(|| NeonApiError {
                reason: Reason::StorageUnavailable,
                component: Some(Component::Wal),
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

/// A secret store in memory, which a test can read and make fail.
#[derive(Debug, Default)]
pub struct MemorySecrets {
    map: Mutex<BTreeMap<String, Vec<u8>>>,
    fail_puts: AtomicBool,
}

impl MemorySecrets {
    fn map(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Vec<u8>>> {
        self.map.lock().expect("secrets lock")
    }

    /// The bytes under `r`, if any.
    pub fn value(&self, r: &SecretRef) -> Option<Vec<u8>> {
        self.map().get(r.as_str()).cloned()
    }

    /// How many secrets are stored.
    pub fn len(&self) -> usize {
        self.map().len()
    }

    /// Every stored value.
    pub fn values(&self) -> Vec<Vec<u8>> {
        self.map().values().cloned().collect()
    }

    /// Makes every put fail (`unavailable`), or not.
    pub fn fail_puts(&self, fail: bool) {
        self.fail_puts.store(fail, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl SecretStore for MemorySecrets {
    async fn put(&self, r: &SecretRef, s: Secret<Vec<u8>>) -> Result<(), SecretError> {
        if self.fail_puts.load(Ordering::SeqCst) {
            return Err(SecretError::Unavailable("injected".into()));
        }
        self.map().insert(r.as_str().into(), s.expose().clone());
        Ok(())
    }

    async fn get(&self, r: &SecretRef) -> Result<Secret<Vec<u8>>, SecretError> {
        self.value(r)
            .map(Secret::new)
            .ok_or_else(|| SecretError::NotFound(r.to_string()))
    }

    async fn delete(&self, r: &SecretRef) -> Result<(), SecretError> {
        self.map().remove(r.as_str());
        Ok(())
    }
}

/// `LoseAck` at `point` of the first attempt of every `pg.batch`.
#[derive(Debug)]
pub struct FirstBatch {
    pub point: loams_kv::FaultPoint,
}

impl loams_kv::FaultPlan for FirstBatch {
    fn at(&self, op: &str, point: loams_kv::FaultPoint, attempt: u32) -> Option<loams_kv::Fault> {
        (op == "pg.batch" && point == self.point && attempt == 1)
            .then_some(loams_kv::Fault::LoseAck)
    }
}

/// Every log line of this test binary, at every level, from the first call
/// on: a process-wide subscriber writing into one buffer.
pub fn captured_logs() -> Arc<Mutex<Vec<u8>>> {
    static LOGS: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();
    LOGS.get_or_init(|| {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let writer = {
            let buf = buf.clone();
            move || LogWriter(buf.clone())
        };
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(writer)
            .with_ansi(false)
            .finish();
        tracing::subscriber::set_global_default(subscriber).expect("the only subscriber");
        buf
    })
    .clone()
}

struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogWriter {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("logs lock").extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub struct Harness {
    pub service: PgService<FakeNeon>,
    pub store: KvControlStore,
    pub neon: FakeNeon,
    pub secrets: Arc<MemorySecrets>,
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
    let secrets = Arc::new(MemorySecrets::default());
    let service = PgService::new(store.clone(), neon.clone(), secrets.clone(), config);
    Harness {
        service,
        store,
        neon,
        secrets,
        clock,
    }
}

/// As [`harness_with`], sharing `secrets`.
pub fn harness_sharing(
    store: KvControlStore,
    secrets: Arc<MemorySecrets>,
    before_commit: Option<BeforeCommit>,
) -> Harness {
    let mut h = harness_with(store, before_commit);
    let config = h.service.config().clone();
    h.service = PgService::new(h.store.clone(), h.neon.clone(), secrets.clone(), config);
    h.secrets = secrets;
    h
}

/// A service on `store` with no hook, its own fake and a clock at
/// [`T0_MS`]: what a hook uses to commit a competing write.
pub fn plain_service(store: &KvControlStore, neon: &FakeNeon) -> PgService<FakeNeon> {
    plain_service_with(store, neon, Arc::new(MemorySecrets::default()))
}

/// As [`plain_service`], on `secrets`.
pub fn plain_service_with(
    store: &KvControlStore,
    neon: &FakeNeon,
    secrets: Arc<MemorySecrets>,
) -> PgService<FakeNeon> {
    PgService::new(store.clone(), neon.clone(), secrets, config(&new_clock()))
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
    set_branch_state(store, project_id, branch_id, BranchState::Ready).await;
}

/// Moves a branch to `state`, as the reconciler (Task 7) would.
pub async fn set_branch_state(
    store: &KvControlStore,
    project_id: &str,
    branch_id: &str,
    state: BranchState,
) {
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
    rec.state = state;
    store
        .api_writer()
        .put(&rec, Some(got.version))
        .await
        .expect("ready");
}
