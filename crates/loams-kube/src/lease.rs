//! Leader election on a `coordination.k8s.io/v1` Lease, with a self-fence (design 65 §7.1, D1180).
//!
//! kube-rs has no leader election, so this follows client-go's algorithm:
//!
//! * A candidate takes the Lease when it has no holder, or when the record has not
//!   changed for `lease` on the candidate's own monotonic clock. Clock skew between
//!   nodes therefore does not matter. The wall clock is only written to the
//!   record for people to read.
//! * The leader renews every `retry`. Every write is a `replace` that carries the
//!   `resourceVersion` it read, so two writers cannot both win.
//! * The leader that has not renewed within `renew_deadline` (monotonic, counted
//!   from the start of its last successful renewal) cancels [`LeaderToken::lost`],
//!   drops its work and returns [`LeaderExit::LostLease`]. The binary then exits
//!   non-zero so that a clean process takes its place. This is the self-fence: it
//!   fires before any other candidate may take over, because `renew_deadline`
//!   is shorter than `lease`.
//! * On a clean stop the leader releases the Lease (client-go's `ReleaseOnCancel`).
//!
//! `leaseTransitions` rises by one whenever the holder changes. It is the fencing
//! token that Task 4 stores in `status.operation.fence`.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use k8s_openapi::api::coordination::v1::{Lease, LeaseSpec};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{MicroTime, ObjectMeta};
use k8s_openapi::jiff::Timestamp;
use kube::api::PostParams;
use kube::{Api, Client};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// Errors of a [`LeaseStore`].
#[derive(Debug, thiserror::Error)]
pub enum LeaseError {
    /// The write lost a compare-and-swap: the Lease changed (or already exists).
    #[error("lease conflict")]
    Conflict,
    /// Any other API failure.
    #[error("lease api error: {0}")]
    Api(String),
}

/// Result of a [`LeaseStore`] call.
pub type Result<T, E = LeaseError> = std::result::Result<T, E>;

/// Settings of one candidate. Defaults are client-go's (15 s / 10 s / 2 s).
#[derive(Clone, Debug)]
pub struct LeaseConfig {
    /// Lease object name, `loams-operator-leader` by default.
    pub name: String,
    pub namespace: String,
    /// Unique per process (the Pod name).
    pub identity: String,
    /// How long a record may stay unchanged before another candidate takes it.
    pub lease: Duration,
    /// The leader fences itself when it cannot renew for this long.
    pub renew_deadline: Duration,
    /// Time between attempts.
    pub retry: Duration,
}

impl LeaseConfig {
    /// The defaults of design 65 §7.1 for `namespace` and `identity`.
    pub fn new(namespace: impl Into<String>, identity: impl Into<String>) -> Self {
        Self {
            name: "loams-operator-leader".to_owned(),
            namespace: namespace.into(),
            identity: identity.into(),
            lease: Duration::from_secs(15),
            renew_deadline: Duration::from_secs(10),
            retry: Duration::from_secs(2),
        }
    }
}

/// Storage of the Lease object.
#[async_trait]
pub trait LeaseStore: Send + Sync {
    async fn get(&self, name: &str) -> Result<Option<Lease>>;
    /// Create the Lease. [`LeaseError::Conflict`] if it exists.
    async fn create(&self, l: &Lease) -> Result<Lease>;
    /// Replace the Lease. `l` carries the `resourceVersion` it was read at;
    /// [`LeaseError::Conflict`] if it changed since.
    async fn replace(&self, l: &Lease) -> Result<Lease, LeaseError>;
}

/// The Lease in a real cluster.
#[derive(Clone, Debug)]
pub struct KubeLeaseStore {
    api: Api<Lease>,
}

impl KubeLeaseStore {
    pub fn new(client: Client, namespace: &str) -> Self {
        Self {
            api: Api::namespaced(client, namespace),
        }
    }
}

fn map_kube(e: kube::Error) -> LeaseError {
    match &e {
        kube::Error::Api(status) if status.code == 409 => LeaseError::Conflict,
        _ => LeaseError::Api(e.to_string()),
    }
}

#[async_trait]
impl LeaseStore for KubeLeaseStore {
    async fn get(&self, name: &str) -> Result<Option<Lease>> {
        self.api.get_opt(name).await.map_err(map_kube)
    }

    async fn create(&self, l: &Lease) -> Result<Lease> {
        self.api
            .create(&PostParams::default(), l)
            .await
            .map_err(map_kube)
    }

    async fn replace(&self, l: &Lease) -> Result<Lease, LeaseError> {
        let name = l.metadata.name.as_deref().unwrap_or_default();
        self.api
            .replace(name, &PostParams::default(), l)
            .await
            .map_err(map_kube)
    }
}

/// In-memory Lease with the compare-and-swap rules of the API server, for tests
/// (the crate's own and Task 4's). It is always public: it is small and has no
/// extra dependencies. Clones share one Lease.
#[derive(Clone, Debug, Default)]
pub struct MemLeaseStore {
    inner: Arc<Mutex<MemState>>,
    /// Stall of the first `get` of this handle only (not shared by clones).
    first_get_stall: Arc<Mutex<Option<Duration>>>,
}

#[derive(Debug, Default)]
struct MemState {
    lease: Option<Lease>,
    version: u64,
    stall: Option<Duration>,
    last_write: Option<Instant>,
}

impl MemLeaseStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Make every later call take `d` (on the tokio clock) before it is applied;
    /// `None` clears it. A call dropped meanwhile (a timeout) is not applied.
    pub fn set_stall(&self, d: Option<Duration>) {
        self.state().stall = d;
    }

    /// A handle on the same Lease whose first `get` takes `d` (a slow read), while
    /// every other call, and every other handle, is unaffected.
    pub fn with_first_get_stall(&self, d: Duration) -> Self {
        Self {
            inner: self.inner.clone(),
            first_get_stall: Arc::new(Mutex::new(Some(d))),
        }
    }

    /// When the last write was applied (tokio clock).
    pub fn last_write_at(&self) -> Option<Instant> {
        self.state().last_write
    }

    /// The current Lease, if any.
    pub fn snapshot(&self) -> Option<Lease> {
        self.state().lease.clone()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, MemState> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    async fn stall(&self) {
        let d = self.state().stall;
        if let Some(d) = d {
            tokio::time::sleep(d).await;
        }
    }
}

#[async_trait]
impl LeaseStore for MemLeaseStore {
    async fn get(&self, _name: &str) -> Result<Option<Lease>> {
        let once = self
            .first_get_stall
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(d) = once {
            tokio::time::sleep(d).await;
        }
        self.stall().await;
        Ok(self.state().lease.clone())
    }

    async fn create(&self, l: &Lease) -> Result<Lease> {
        self.stall().await;
        let mut s = self.state();
        if s.lease.is_some() {
            return Err(LeaseError::Conflict);
        }
        s.version += 1;
        let mut new = l.clone();
        new.metadata.resource_version = Some(s.version.to_string());
        s.lease = Some(new.clone());
        s.last_write = Some(Instant::now());
        Ok(new)
    }

    async fn replace(&self, l: &Lease) -> Result<Lease, LeaseError> {
        self.stall().await;
        let mut s = self.state();
        let current = s
            .lease
            .as_ref()
            .and_then(|c| c.metadata.resource_version.clone());
        if current.is_none() || current != l.metadata.resource_version {
            return Err(LeaseError::Conflict);
        }
        s.version += 1;
        let mut new = l.clone();
        new.metadata.resource_version = Some(s.version.to_string());
        s.lease = Some(new.clone());
        s.last_write = Some(Instant::now());
        Ok(new)
    }
}

/// Time source. `now` is monotonic and drives every deadline; `wall` is only
/// written into the Lease record.
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
    fn wall(&self) -> SystemTime;
}

/// The clock of the tokio runtime. With `start_paused` tests it follows paused
/// time, so a test can drive a lease by advancing it.
#[derive(Clone, Debug)]
pub struct TokioClock {
    base: Instant,
    base_wall: SystemTime,
}

impl TokioClock {
    pub fn new() -> Self {
        Self {
            base: Instant::now(),
            base_wall: SystemTime::now(),
        }
    }
}

impl Default for TokioClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for TokioClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn wall(&self) -> SystemTime {
        self.base_wall + Instant::now().saturating_duration_since(self.base)
    }
}

/// Proof of leadership, handed to the leader's work.
#[derive(Clone, Debug)]
pub struct LeaderToken {
    /// `leaseTransitions` of the Lease when this process took it: the fencing token.
    pub transitions: i32,
    /// Cancelled when the leader fences itself. The work is dropped at the same time.
    pub lost: CancellationToken,
}

/// Why [`run_leader`] returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaderExit {
    /// The lease could not be renewed in time. The binary exits non-zero.
    LostLease,
    /// The work finished and the lease was released.
    Shutdown,
}

/// What this candidate last saw of the record, and when (client-go's
/// `observedRecord` and `observedTime`).
#[derive(Default)]
struct Observed {
    key: Option<RecordKey>,
    at: Option<Instant>,
}

#[derive(PartialEq)]
struct RecordKey {
    holder: Option<String>,
    renew: Option<MicroTime>,
    acquire: Option<MicroTime>,
    transitions: Option<i32>,
}

impl RecordKey {
    fn of(l: &Lease) -> Self {
        let s = l.spec.clone().unwrap_or_default();
        Self {
            holder: s.holder_identity.filter(|h| !h.is_empty()),
            renew: s.renew_time,
            acquire: s.acquire_time,
            transitions: s.lease_transitions,
        }
    }
}

enum Attempt {
    /// This process holds the Lease with this transition count.
    Held(i32),
    /// Another process holds it.
    Other,
}

fn micro(t: SystemTime) -> Option<MicroTime> {
    Timestamp::try_from(t).ok().map(MicroTime)
}

fn secs(d: Duration) -> i32 {
    i32::try_from(d.as_secs().max(1)).unwrap_or(i32::MAX)
}

/// One pass of client-go's `tryAcquireOrRenew`.
async fn try_acquire_or_renew(
    cfg: &LeaseConfig,
    store: &dyn LeaseStore,
    clock: &dyn Clock,
    obs: &mut Observed,
) -> Result<Attempt> {
    let wall = micro(clock.wall());
    let got = store.get(&cfg.name).await?;
    // The observation time is taken after the read returns, never before it: the
    // record may have been written any time before the response, so a slow read
    // must not make the Lease look older than it is.
    let now = clock.now();
    let Some(existing) = got else {
        let l = Lease {
            metadata: ObjectMeta {
                name: Some(cfg.name.clone()),
                namespace: Some(cfg.namespace.clone()),
                ..ObjectMeta::default()
            },
            spec: Some(LeaseSpec {
                holder_identity: Some(cfg.identity.clone()),
                lease_duration_seconds: Some(secs(cfg.lease)),
                acquire_time: wall.clone(),
                renew_time: wall,
                lease_transitions: Some(0),
                ..LeaseSpec::default()
            }),
        };
        let created = store.create(&l).await?;
        observe(obs, &created, clock.now());
        return Ok(Attempt::Held(0));
    };

    let key = RecordKey::of(&existing);
    if obs.key.as_ref() != Some(&key) {
        obs.key = Some(key);
        obs.at = Some(now);
    }
    let spec = existing.spec.clone().unwrap_or_default();
    let holder = spec.holder_identity.clone().filter(|h| !h.is_empty());
    let mine = holder.as_deref() == Some(cfg.identity.as_str());
    if let Some(h) = &holder
        && !mine
    {
        let dur = spec
            .lease_duration_seconds
            .and_then(|s| u64::try_from(s).ok())
            .map_or(cfg.lease, Duration::from_secs);
        let seen = obs.at.unwrap_or(now);
        if now.saturating_duration_since(seen) < dur {
            tracing::trace!(holder = %h, "lease is held by another candidate");
            return Ok(Attempt::Other);
        }
    }

    let old = spec.lease_transitions.unwrap_or(0);
    let transitions = if mine { old } else { old.saturating_add(1) };
    let mut next = existing;
    next.spec = Some(LeaseSpec {
        holder_identity: Some(cfg.identity.clone()),
        lease_duration_seconds: Some(secs(cfg.lease)),
        acquire_time: if mine {
            spec.acquire_time
        } else {
            wall.clone()
        },
        renew_time: wall,
        lease_transitions: Some(transitions),
        ..spec
    });
    let written = store.replace(&next).await?;
    observe(obs, &written, clock.now());
    Ok(Attempt::Held(transitions))
}

fn observe(obs: &mut Observed, l: &Lease, now: Instant) {
    obs.key = Some(RecordKey::of(l));
    obs.at = Some(now);
}

/// Give the Lease up so that the next candidate need not wait `lease`.
async fn release(cfg: &LeaseConfig, store: &dyn LeaseStore, clock: &dyn Clock) {
    let attempt = async {
        let Some(mut l) = store.get(&cfg.name).await? else {
            return Ok(());
        };
        let mut spec = l.spec.clone().unwrap_or_default();
        if spec.holder_identity.as_deref() != Some(cfg.identity.as_str()) {
            return Ok(());
        }
        spec.holder_identity = None;
        spec.lease_duration_seconds = Some(1);
        spec.renew_time = micro(clock.wall());
        l.spec = Some(spec);
        store.replace(&l).await.map(|_| ())
    };
    match tokio::time::timeout(cfg.retry, attempt).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!(error = %e, "could not release the leader lease"),
        Err(_) => tracing::warn!("timed out releasing the leader lease"),
    }
}

/// Wait for the Lease, run `work` while holding it, and fence on loss.
///
/// `work` gets a [`LeaderToken`] and runs until it returns (then the Lease is
/// released and the result is [`LeaderExit::Shutdown`]) or until the Lease is
/// lost (then `token.lost` is cancelled, `work` is dropped and the result is
/// [`LeaderExit::LostLease`]).
///
/// Renewal runs on the same task as `work`, so `work` must not block the
/// runtime thread (no long synchronous code, no blocking IO): it has to yield
/// often and stop when `token.lost` is cancelled. The fence is checked before
/// `work` is polled on every wake-up.
///
/// # Panics
///
/// If `cfg` does not satisfy `retry < renew_deadline < lease`.
pub async fn run_leader<F>(
    cfg: LeaseConfig,
    store: impl LeaseStore,
    clock: impl Clock,
    work: impl FnOnce(LeaderToken) -> F,
) -> LeaderExit
where
    F: Future<Output = ()>,
{
    assert!(
        cfg.retry < cfg.renew_deadline && cfg.renew_deadline < cfg.lease,
        "lease config needs retry < renew_deadline < lease"
    );
    let mut obs = Observed::default();

    // Acquire.
    let (transitions, mut last_renew) = loop {
        let started = clock.now();
        let attempt = tokio::time::timeout(
            cfg.renew_deadline,
            try_acquire_or_renew(&cfg, &store, &clock, &mut obs),
        )
        .await;
        match attempt {
            Ok(Ok(Attempt::Held(t))) => break (t, started),
            Ok(Ok(Attempt::Other)) => {}
            Ok(Err(LeaseError::Conflict)) => tracing::debug!("lease conflict while acquiring"),
            Ok(Err(e)) => tracing::warn!(error = %e, "lease acquire failed"),
            Err(_) => tracing::warn!("lease acquire timed out"),
        }
        tokio::time::sleep(cfg.retry).await;
    };
    tracing::info!(identity = %cfg.identity, transitions, "became leader");

    let lost = CancellationToken::new();
    let token = LeaderToken {
        transitions,
        lost: lost.clone(),
    };

    // Renew. Never finishes without losing the lease.
    let renew = async {
        loop {
            let deadline = last_renew + cfg.renew_deadline;
            let now = clock.now();
            if now >= deadline {
                return;
            }
            tokio::time::sleep(cfg.retry.min(deadline - now)).await;
            let started = clock.now();
            if started >= deadline {
                return;
            }
            let attempt = tokio::time::timeout(
                deadline - started,
                try_acquire_or_renew(&cfg, &store, &clock, &mut obs),
            )
            .await;
            match attempt {
                Ok(Ok(Attempt::Held(_))) => last_renew = started,
                Ok(Ok(Attempt::Other)) => {
                    tracing::error!("another candidate holds the leader lease");
                    return;
                }
                Ok(Err(e)) => tracing::warn!(error = %e, "lease renewal failed"),
                Err(_) => return,
            }
        }
    };

    tokio::pin!(renew);
    let work = work(token);
    tokio::pin!(work);
    // `biased`: the renewal branch is polled first, so a leader that was paused
    // past its deadline fences before `work` can run another step.
    tokio::select! {
        biased;
        () = &mut renew => {
            lost.cancel();
            tracing::error!(identity = %cfg.identity, "lost the leader lease, self-fenced");
            LeaderExit::LostLease
        }
        () = &mut work => {
            release(&cfg, &store, &clock).await;
            LeaderExit::Shutdown
        }
    }
}
