//! What the service cases (`tests/service/`) share: a fake [`NeonApi`], a
//! store of the test binary's backend (`crate::factory()`), an in-memory
//! secret store, and a service on a hand-driven clock.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use loams_pg_control::model::{BranchKey, BranchRec, BranchState};
use loams_pg_control::neon::{
    Component, Lsn, LsnAtTime, NeonApiError, NeonRead, NeonWrite, TenantConfig, TenantId,
    TimelineCreate, TimelineId, TimelineView, WalHeads, WalTimelineCreate,
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
    /// Attached tenants (`NeonWrite`).
    tenants: HashSet<[u8; 16]>,
    /// Each created timeline's ancestor (`NeonWrite`).
    ancestors: HashMap<Tl, Option<[u8; 16]>>,
    /// Failures to answer, per call name, in order.
    failures: HashMap<&'static str, VecDeque<NeonApiError>>,
}

/// Where a bootstrapped timeline of the fake starts: initdb's end.
pub const INITDB_LSN: Lsn = Lsn(0x0169_6F10);

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

    /// How many calls start with `prefix`.
    pub fn count(&self, prefix: &str) -> usize {
        self.lock()
            .calls
            .iter()
            .filter(|c| c.starts_with(prefix))
            .count()
    }

    /// The next call named `call` (`attach`, `tenant_config`,
    /// `create_timeline`, `delete_timeline`, `wal_create_timeline`) fails
    /// with `reason`, before it changes anything.
    pub fn fail_next(&self, call: &'static str, reason: Reason) {
        self.lock()
            .failures
            .entry(call)
            .or_default()
            .push_back(NeonApiError {
                reason,
                component: Some(Component::Pageserver),
                message: format!("injected {}", reason.as_str()),
            });
    }

    /// Whether the fake holds the timeline.
    pub fn has_timeline(&self, t: [u8; 16], tl: [u8; 16]) -> bool {
        self.lock().timelines.contains_key(&(t, tl))
    }
}

impl Inner {
    fn injected(&mut self, call: &'static str) -> Result<(), NeonApiError> {
        match self.failures.get_mut(call).and_then(VecDeque::pop_front) {
            Some(e) => {
                self.calls.push(format!("{call} -> {}", e.reason.as_str()));
                Err(e)
            }
            None => Ok(()),
        }
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

fn already_exists(what: &str) -> NeonApiError {
    NeonApiError {
        reason: Reason::AlreadyExists,
        component: Some(Component::Pageserver),
        message: format!("{what} already exists"),
    }
}

/// The fake's storage: tenants and timelines it keeps, as the pageserver
/// and `loams-wal` would. A create of an existing timeline is a 409
/// (`already_exists`); a delete of a timeline with children is
/// `branch_has_children`, of a missing one `not_found`.
impl NeonWrite for FakeNeon {
    async fn attach_tenant(
        &self,
        t: TenantId,
        generation: u32,
        _config: &TenantConfig,
    ) -> Result<(), NeonApiError> {
        let mut inner = self.lock();
        inner.injected("attach")?;
        inner
            .calls
            .push(format!("attach {t} generation={generation}"));
        inner.tenants.insert(t.0);
        Ok(())
    }

    async fn tenant_config(&self, t: TenantId, config: &TenantConfig) -> Result<(), NeonApiError> {
        let mut inner = self.lock();
        inner.injected("tenant_config")?;
        let pitr = config.pitr_interval.map_or(0, |d| d.as_secs());
        inner.calls.push(format!("tenant_config {t} pitr={pitr}"));
        if !inner.tenants.contains(&t.0) {
            return Err(missing("tenant"));
        }
        Ok(())
    }

    async fn create_timeline(
        &self,
        t: TenantId,
        create: &TimelineCreate,
    ) -> Result<TimelineView, NeonApiError> {
        let mut inner = self.lock();
        inner.injected("create_timeline")?;
        let tl = create.new_timeline_id;
        let line = format!(
            "create_timeline {tl} ancestor={} at={} pg={}",
            create.ancestor.map_or("-".to_string(), |a| a.to_string()),
            create
                .ancestor_start_lsn
                .map_or("-".to_string(), |l| l.to_string()),
            create.pg_version.map_or("-".to_string(), |v| v.to_string()),
        );
        if !inner.tenants.contains(&t.0) {
            inner.calls.push(format!("{line} -> not_found"));
            return Err(missing("tenant"));
        }
        if inner.timelines.contains_key(&(t.0, tl.0)) {
            inner.calls.push(format!("{line} -> already_exists"));
            return Err(already_exists("timeline"));
        }
        let start = match create.ancestor {
            None => INITDB_LSN,
            Some(a) => match inner.timelines.get(&(t.0, a.0)) {
                Some(parent) => create.ancestor_start_lsn.unwrap_or(parent.last_record_lsn),
                None => {
                    inner.calls.push(format!("{line} -> not_found"));
                    return Err(missing("ancestor timeline"));
                }
            },
        };
        inner.calls.push(line);
        let view = TimelineView {
            last_record_lsn: start,
            min_readable_lsn: start,
            logical_size_bytes: 0,
        };
        inner.timelines.insert((t.0, tl.0), view.clone());
        inner
            .ancestors
            .insert((t.0, tl.0), create.ancestor.map(|a| a.0));
        Ok(view)
    }

    async fn delete_timeline(&self, t: TenantId, tl: TimelineId) -> Result<(), NeonApiError> {
        let mut inner = self.lock();
        inner.injected("delete_timeline")?;
        inner.calls.push(format!("delete_timeline {tl}"));
        if !inner.timelines.contains_key(&(t.0, tl.0)) {
            return Err(missing("timeline"));
        }
        if inner.ancestors.values().any(|a| *a == Some(tl.0)) {
            return Err(NeonApiError {
                reason: Reason::BranchHasChildren,
                component: Some(Component::Pageserver),
                message: "timeline has child timelines".into(),
            });
        }
        inner.timelines.remove(&(t.0, tl.0));
        inner.ancestors.remove(&(t.0, tl.0));
        inner.wal.remove(&(t.0, tl.0));
        Ok(())
    }

    async fn wal_create_timeline(
        &self,
        create: &WalTimelineCreate,
    ) -> Result<WalHeads, NeonApiError> {
        let mut inner = self.lock();
        inner.injected("wal_create_timeline")?;
        inner.calls.push(format!(
            "wal_create_timeline {} start={} pg={}",
            create.timeline_id, create.start_lsn, create.pg_version
        ));
        let key = (create.tenant_id.0, create.timeline_id.0);
        let heads = inner.wal.entry(key).or_insert(WalHeads {
            commit_lsn: create.start_lsn,
            flush_lsn: create.start_lsn,
            remote_consistent_lsn: create.start_lsn,
            backup_lsn: create.start_lsn,
        });
        Ok(heads.clone())
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

    async fn list(&self) -> Result<Vec<SecretRef>, SecretError> {
        Ok(self
            .map()
            .keys()
            .filter_map(|k| SecretRef::parse(k).ok())
            .collect())
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
