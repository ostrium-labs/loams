//! The lifecycle service: owns the records, the phase machine and metering, and calls the
//! adapters (D2010, D2012, D2013). The HTTP facade is a thin layer over this.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::meter::{
    self, GateOp, MeterGate, MeterOrigin, MeterSink, NoopMeterSink, OpenGate, Usage, UsageEvent,
    UsageLog,
};
use crate::{
    CreateSession, EngineCatalog, EngineError, EngineKind, EngineRegistry, Instance, InstanceId,
    InstanceSpec, InstanceStatus, InstanceStore, ManagedEngine, MemoryStore, Observed, Op, Phase,
    SizeRequest, SparkEngine, SparkJob, SparkSession, SqlRequest, SqlResult, SubmitJob,
    validate_name,
};

/// `POST /v1/namespaces/{p}/engines`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateInstance {
    pub name: String,
    pub kind: EngineKind,
    #[serde(default)]
    pub version: Option<String>,
    pub size: SizeRequest,
    #[serde(default)]
    pub config: BTreeMap<String, String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

/// `PATCH /v1/namespaces/{p}/engines/{id}`: exactly one of the two.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateInstance {
    #[serde(default)]
    pub size: Option<SizeRequest>,
    #[serde(default)]
    pub version: Option<String>,
}

/// Most labels and config entries on one instance, and the longest key or value.
const MAX_MAP_ENTRIES: usize = 64;
const MAX_MAP_TEXT: usize = 256;

fn check_map(what: &str, m: &BTreeMap<String, String>) -> Result<(), EngineError> {
    if m.len() > MAX_MAP_ENTRIES {
        return Err(EngineError::Invalid(format!(
            "at most {MAX_MAP_ENTRIES} {what} entries"
        )));
    }
    if m.iter()
        .any(|(k, v)| k.is_empty() || k.len() > MAX_MAP_TEXT || v.len() > MAX_MAP_TEXT)
    {
        return Err(EngineError::Invalid(format!(
            "{what} keys are 1-{MAX_MAP_TEXT} characters and values at most {MAX_MAP_TEXT}"
        )));
    }
    Ok(())
}

/// Removes the instance from the busy set when an operation ends, however it ends.
struct Busy<'a> {
    set: &'a Mutex<HashSet<InstanceId>>,
    id: InstanceId,
}

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        if let Ok(mut s) = self.set.lock() {
            s.remove(&self.id);
        }
    }
}

/// The managed-engines service.
pub struct EngineService {
    registry: EngineRegistry,
    store: Arc<dyn InstanceStore>,
    meter: Arc<dyn MeterSink>,
    origin: MeterOrigin,
    gate: Arc<dyn MeterGate>,
    usage: UsageLog,
    /// When each instance's storage gauge last went out.
    last_storage: Mutex<HashMap<InstanceId, OffsetDateTime>>,
    busy: Mutex<HashSet<InstanceId>>,
    /// Serializes create's name check and insert.
    creating: tokio::sync::Mutex<()>,
    /// The metering window, in seconds (60 by default).
    window_secs: i64,
}

impl std::fmt::Debug for EngineService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineService")
            .field("registry", &self.registry)
            .field("origin", &self.origin)
            .field("window_secs", &self.window_secs)
            .finish_non_exhaustive()
    }
}

impl EngineService {
    /// A service over `registry` with an in-memory store and metering off.
    pub fn new(registry: EngineRegistry) -> Self {
        Self {
            registry,
            store: Arc::new(MemoryStore::new()),
            meter: Arc::new(NoopMeterSink),
            origin: MeterOrigin::default(),
            gate: Arc::new(OpenGate),
            usage: UsageLog::default(),
            last_storage: Mutex::new(HashMap::new()),
            busy: Mutex::new(HashSet::new()),
            creating: tokio::sync::Mutex::new(()),
            window_secs: 60,
        }
    }

    #[must_use]
    pub fn with_store(mut self, store: Arc<dyn InstanceStore>) -> Self {
        self.store = store;
        self
    }

    #[must_use]
    pub fn with_meter(mut self, meter: Arc<dyn MeterSink>, origin: MeterOrigin) -> Self {
        self.meter = meter;
        self.origin = origin;
        self
    }

    /// The Meter's entitlement gate, asked on create, resume and scale up (ENG81 R17).
    #[must_use]
    pub fn with_gate(mut self, gate: Arc<dyn MeterGate>) -> Self {
        self.gate = gate;
        self
    }

    /// The metering window; the background loop should tick at the same interval.
    #[must_use]
    pub fn with_window(mut self, every: std::time::Duration) -> Self {
        self.window_secs = i64::try_from(every.as_secs()).unwrap_or(60).max(1);
        self
    }

    pub fn registry(&self) -> &EngineRegistry {
        &self.registry
    }

    pub fn catalog(&self) -> EngineCatalog {
        self.registry.catalog()
    }

    fn claim(&self, id: &InstanceId) -> Result<Busy<'_>, EngineError> {
        let mut set = self
            .busy
            .lock()
            .map_err(|_| EngineError::Internal("busy set lock poisoned".into()))?;
        if !set.insert(id.clone()) {
            return Err(EngineError::Conflict(format!(
                "another operation on {id} is in progress"
            )));
        }
        Ok(Busy {
            set: &self.busy,
            id: id.clone(),
        })
    }

    fn is_busy(&self, id: &InstanceId) -> bool {
        self.busy.lock().map(|s| s.contains(id)).unwrap_or(true)
    }

    /// The instance, if it is in `namespace`. Another namespace's instance is not found, never
    /// forbidden, so ids do not leak across namespaces.
    async fn load(&self, namespace: &str, id: &str) -> Result<Instance, EngineError> {
        let id = InstanceId(id.to_owned());
        match self.store.get(&id).await? {
            Some(inst) if inst.namespace == namespace => Ok(inst),
            _ => Err(EngineError::NotFound(format!(
                "no engine instance {id} in namespace {namespace}"
            ))),
        }
    }

    pub async fn list(
        &self,
        namespace: &str,
        kind: Option<EngineKind>,
    ) -> Result<Vec<Instance>, EngineError> {
        let mut all = self.store.list(namespace).await?;
        if let Some(kind) = kind {
            all.retain(|i| i.spec.kind == kind);
        }
        Ok(all)
    }

    /// One instance. While it is in a transitional phase and nothing else holds it, this asks
    /// the adapter first, so polling shows progress without a reconciler.
    pub async fn get(&self, namespace: &str, id: &str) -> Result<Instance, EngineError> {
        let inst = self.load(namespace, id).await?;
        if !is_transitional(inst.status.phase) || self.is_busy(&inst.id) {
            return Ok(inst);
        }
        Ok(self.refresh(inst).await)
    }

    /// Observes `inst` and stores what the adapter saw; on failure, the stored record.
    async fn refresh(&self, mut inst: Instance) -> Instance {
        let Ok(engine) = self.registry.get(inst.spec.kind).cloned() else {
            return inst;
        };
        let Ok(_busy) = self.claim(&inst.id) else {
            return inst;
        };
        match engine.observe(&inst).await {
            Ok(obs) => {
                inst.apply(obs, OffsetDateTime::now_utc());
                if let Err(e) = self.store.put(&inst).await {
                    tracing::warn!(target: "loams_engines", id = %inst.id, error = %e, "storing an observation failed");
                }
            }
            Err(e) => {
                tracing::debug!(target: "loams_engines", id = %inst.id, error = %e, "observe failed");
            }
        }
        inst
    }

    /// Observes every instance in a transitional phase (the background loop calls this).
    pub async fn reconcile(&self) -> Result<usize, EngineError> {
        let mut n = 0;
        for inst in self.store.list_all().await? {
            if is_transitional(inst.status.phase) && !self.is_busy(&inst.id) {
                self.refresh(inst).await;
                n += 1;
            }
        }
        Ok(n)
    }

    pub async fn create(
        &self,
        namespace: &str,
        org: Option<String>,
        req: CreateInstance,
    ) -> Result<Instance, EngineError> {
        validate_name(&req.name)?;
        check_map("config", &req.config)?;
        check_map("labels", &req.labels)?;
        let size = req.size.resolve()?;
        let engine = self.registry.get(req.kind)?.clone();
        let caps = engine.capabilities();
        if let Some(v) = &req.version
            && !caps.versions.iter().any(|known| known == v)
        {
            return Err(EngineError::Invalid(format!(
                "{} does not offer version {v:?} (offered: {})",
                req.kind.label(),
                caps.versions.join(", ")
            )));
        }
        let spec = InstanceSpec {
            name: req.name,
            kind: req.kind,
            version: req.version,
            size,
            config: req.config,
            labels: req.labels,
        };
        engine.validate(&spec)?;

        let now = OffsetDateTime::now_utc();
        let id = InstanceId::generate();
        let mut inst = Instance {
            state_prefix: Instance::state_prefix_for(namespace, &id),
            id,
            namespace: namespace.to_owned(),
            org,
            spec,
            status: InstanceStatus::new(Phase::Provisioning, now),
            endpoints: Vec::new(),
            stability: caps.stability,
            created_at: now,
            updated_at: now,
        };
        let _busy = self.claim(&inst.id)?;
        {
            let _creating = self.creating.lock().await;
            let taken = self.store.name_taken(namespace, &inst.spec.name).await?;
            if taken {
                return Err(EngineError::Conflict(format!(
                    "namespace {namespace} already has an engine instance named {:?}",
                    inst.spec.name
                )));
            }
            self.gate.admit(&inst, GateOp::Create).await?;
            self.store.put(&inst).await?;
        }
        match engine.create(&inst).await {
            Ok(obs) => {
                inst.apply(obs, OffsetDateTime::now_utc());
                self.store.put(&inst).await?;
                Ok(inst)
            }
            Err(e) if e.keeps_record() => {
                inst.status.phase = Phase::Failed;
                inst.status.message = Some(e.to_string());
                inst.status.observed_at = OffsetDateTime::now_utc();
                inst.updated_at = inst.status.observed_at;
                self.store.put(&inst).await?;
                Ok(inst)
            }
            Err(e) => {
                self.store.remove(&inst.id).await?;
                Err(e)
            }
        }
    }

    /// Claims the instance, checks the phase allows `op` and stores the transitional phase.
    /// Returns the guard, the record as it was and the adapter.
    async fn begin(
        &self,
        namespace: &str,
        id: &str,
        op: Op,
    ) -> Result<(Busy<'_>, Instance, Arc<dyn ManagedEngine>), EngineError> {
        let inst = self.load(namespace, id).await?;
        let busy = self.claim(&inst.id)?;
        // Reload under the claim: another operation may have finished in between.
        let inst = self.load(namespace, id).await?;
        let phase = inst.status.phase;
        if !phase.allows(op) {
            return Err(EngineError::FailedPrecondition(format!(
                "cannot {} an instance that is {}",
                op.as_str(),
                phase.as_str()
            )));
        }
        let engine = self.registry.get(inst.spec.kind)?.clone();
        if op == Op::Suspend && !engine.capabilities().suspend {
            return Err(EngineError::Unsupported(format!(
                "{} cannot suspend",
                inst.spec.kind.label()
            )));
        }
        if let Some(t) = Phase::transitional(op) {
            let mut moving = inst.clone();
            moving.status.phase = t;
            moving.status.message = None;
            moving.updated_at = OffsetDateTime::now_utc();
            self.store.put(&moving).await?;
        }
        Ok((busy, inst, engine))
    }

    /// Stores the outcome of an adapter call made from `before`. On failure the record goes
    /// back to its earlier phase with the error as its message.
    async fn finish(
        &self,
        before: Instance,
        mut after: Instance,
        outcome: Result<Observed, EngineError>,
    ) -> Result<Instance, EngineError> {
        match outcome {
            Ok(obs) => {
                after.apply(obs, OffsetDateTime::now_utc());
                self.store.put(&after).await?;
                Ok(after)
            }
            Err(e) => {
                let mut back = before;
                back.status.message = Some(e.to_string());
                back.updated_at = OffsetDateTime::now_utc();
                self.store.put(&back).await?;
                Err(e)
            }
        }
    }

    /// Scales or upgrades, by which field the request sets.
    pub async fn update(
        &self,
        namespace: &str,
        id: &str,
        req: UpdateInstance,
    ) -> Result<Instance, EngineError> {
        match (req.size, req.version) {
            (Some(size), None) => self.scale(namespace, id, size).await,
            (None, Some(version)) => self.upgrade(namespace, id, &version).await,
            _ => Err(EngineError::Invalid(
                "set exactly one of size (scale) or version (upgrade)".into(),
            )),
        }
    }

    pub async fn scale(
        &self,
        namespace: &str,
        id: &str,
        size: SizeRequest,
    ) -> Result<Instance, EngineError> {
        let to = size.resolve()?;
        let (_busy, before, engine) = self.begin(namespace, id, Op::Scale).await?;
        let mut after = before.clone();
        after.spec.size = to;
        let grows = to.total_vcpu_milli() > before.spec.size.total_vcpu_milli()
            || to.total_memory_mib() > before.spec.size.total_memory_mib();
        let admitted = match engine.validate(&after.spec) {
            Ok(()) if grows => self.gate.admit(&after, GateOp::ScaleUp).await,
            other => other,
        };
        if let Err(e) = admitted {
            // Put back the phase `begin` moved.
            self.store.put(&before).await?;
            return Err(e);
        }
        let outcome = engine.scale(&before, &to).await;
        self.finish(before, after, outcome).await
    }

    pub async fn upgrade(
        &self,
        namespace: &str,
        id: &str,
        version: &str,
    ) -> Result<Instance, EngineError> {
        let (_busy, before, engine) = self.begin(namespace, id, Op::Upgrade).await?;
        let caps = engine.capabilities();
        if !caps.versions.iter().any(|v| v == version) {
            let e = EngineError::Invalid(format!(
                "{} does not offer version {version:?} (offered: {})",
                before.spec.kind.label(),
                caps.versions.join(", ")
            ));
            // Put back the phase `begin` moved.
            self.store.put(&before).await?;
            return Err(e);
        }
        let mut after = before.clone();
        after.spec.version = Some(version.to_owned());
        let outcome = engine.upgrade(&before, version).await;
        self.finish(before, after, outcome).await
    }

    pub async fn suspend(&self, namespace: &str, id: &str) -> Result<Instance, EngineError> {
        let (_busy, before, engine) = self.begin(namespace, id, Op::Suspend).await?;
        // Bill the compute up to now before it stops.
        self.meter_one(&before, OffsetDateTime::now_utc()).await;
        let outcome = engine.suspend(&before).await;
        let after = before.clone();
        self.finish(before, after, outcome).await
    }

    pub async fn resume(&self, namespace: &str, id: &str) -> Result<Instance, EngineError> {
        let (_busy, before, engine) = self.begin(namespace, id, Op::Resume).await?;
        if let Err(e) = self.gate.admit(&before, GateOp::Resume).await {
            self.store.put(&before).await?;
            return Err(e);
        }
        // Close the suspended window (state only) before compute restarts.
        self.meter_one(&before, OffsetDateTime::now_utc()).await;
        let outcome = engine.resume(&before).await;
        let after = before.clone();
        self.finish(before, after, outcome).await
    }

    /// What the Meter does at a hard limit (ENG81 R17): suspend, never delete. The reason
    /// stays on the instance's status until it is resumed.
    pub async fn suspend_for_limit(
        &self,
        namespace: &str,
        id: &str,
        reason: &str,
    ) -> Result<Instance, EngineError> {
        let mut inst = self.suspend(namespace, id).await?;
        inst.status.message = Some(format!("suspended at a usage limit: {reason}"));
        self.store.put(&inst).await?;
        Ok(inst)
    }

    /// Deletes the runtime, its state and the record. Answers the record in `deleted`.
    pub async fn delete(&self, namespace: &str, id: &str) -> Result<Instance, EngineError> {
        let (_busy, before, engine) = self.begin(namespace, id, Op::Delete).await?;
        // The last partial window.
        self.meter_one(&before, OffsetDateTime::now_utc()).await;
        match engine.delete(&before).await {
            Ok(()) => {
                self.store.remove(&before.id).await?;
                self.usage.forget(&before.id);
                if let Ok(mut m) = self.last_storage.lock() {
                    m.remove(&before.id);
                }
                let mut gone = before;
                let now = OffsetDateTime::now_utc();
                gone.status = InstanceStatus::new(Phase::Deleted, now);
                gone.endpoints.clear();
                gone.updated_at = now;
                Ok(gone)
            }
            Err(e) => self.finish(before.clone(), before, Err(e)).await,
        }
    }

    /// The samples this service emitted for an instance.
    pub async fn usage(
        &self,
        namespace: &str,
        id: &str,
        from: Option<OffsetDateTime>,
        to: Option<OffsetDateTime>,
    ) -> Result<Usage, EngineError> {
        let inst = self.load(namespace, id).await?;
        Ok(Usage::from_samples(self.usage.range(&inst.id, from, to)))
    }

    /// The instance and adapter for a call that needs the engine up.
    async fn running(
        &self,
        namespace: &str,
        id: &str,
    ) -> Result<(Instance, Arc<dyn ManagedEngine>), EngineError> {
        let inst = self.load(namespace, id).await?;
        let engine = self.registry.get(inst.spec.kind)?.clone();
        if !inst.status.phase.allows(Op::Use) {
            return Err(EngineError::FailedPrecondition(format!(
                "the instance is {}; it must be running",
                inst.status.phase.as_str()
            )));
        }
        Ok((inst, engine))
    }

    pub async fn sql(
        &self,
        namespace: &str,
        id: &str,
        req: SqlRequest,
    ) -> Result<SqlResult, EngineError> {
        req.validate()?;
        let (inst, engine) = self.running(namespace, id).await?;
        let Some(sql) = engine.sql() else {
            return Err(EngineError::Unsupported(format!(
                "{} has no SQL endpoint here; use sessions and jobs",
                inst.spec.kind.label()
            )));
        };
        sql.execute(&inst, req).await
    }

    fn spark_of<'a>(
        inst: &Instance,
        engine: &'a Arc<dyn ManagedEngine>,
    ) -> Result<&'a dyn SparkEngine, EngineError> {
        engine.spark().ok_or_else(|| {
            EngineError::Unsupported(format!(
                "{} has no sessions or jobs; sessions and jobs are Spark's",
                inst.spec.kind.label()
            ))
        })
    }

    pub async fn list_sessions(
        &self,
        namespace: &str,
        id: &str,
    ) -> Result<Vec<SparkSession>, EngineError> {
        let (inst, engine) = self.running(namespace, id).await?;
        Self::spark_of(&inst, &engine)?.list_sessions(&inst).await
    }

    pub async fn create_session(
        &self,
        namespace: &str,
        id: &str,
        req: CreateSession,
    ) -> Result<SparkSession, EngineError> {
        check_map("conf", &req.conf)?;
        let (inst, engine) = self.running(namespace, id).await?;
        Self::spark_of(&inst, &engine)?
            .create_session(&inst, req)
            .await
    }

    pub async fn close_session(
        &self,
        namespace: &str,
        id: &str,
        session: &str,
    ) -> Result<(), EngineError> {
        let (inst, engine) = self.running(namespace, id).await?;
        Self::spark_of(&inst, &engine)?
            .close_session(&inst, session)
            .await
    }

    pub async fn list_jobs(&self, namespace: &str, id: &str) -> Result<Vec<SparkJob>, EngineError> {
        let (inst, engine) = self.running(namespace, id).await?;
        Self::spark_of(&inst, &engine)?.list_jobs(&inst).await
    }

    pub async fn submit_job(
        &self,
        namespace: &str,
        id: &str,
        req: SubmitJob,
    ) -> Result<SparkJob, EngineError> {
        req.validate()?;
        check_map("conf", &req.conf)?;
        let (inst, engine) = self.running(namespace, id).await?;
        Self::spark_of(&inst, &engine)?.submit_job(&inst, req).await
    }

    pub async fn cancel_job(
        &self,
        namespace: &str,
        id: &str,
        job: &str,
    ) -> Result<SparkJob, EngineError> {
        let (inst, engine) = self.running(namespace, id).await?;
        Self::spark_of(&inst, &engine)?.cancel_job(&inst, job).await
    }

    /// Samples one instance up to `now`, records the sample and returns its events: the
    /// runtime event while compute runs, and the storage gauge when an hour has passed since
    /// the last one. Empty when the window is empty, the adapter is gone or it could not
    /// measure (the next window covers the gap, since it starts where the last one ended).
    async fn sample(&self, inst: &Instance, now: OffsetDateTime) -> Vec<UsageEvent> {
        let mut out = Vec::new();
        if !inst.status.phase.has_state() {
            return out;
        }
        let window = time::Duration::seconds(self.window_secs);
        let start = self
            .usage
            .last_end(&inst.id)
            .unwrap_or_else(|| inst.created_at.max(now - window));
        if now <= start {
            return out;
        }
        let Ok(engine) = self.registry.get(inst.spec.kind) else {
            return out;
        };
        let mut s = match engine.usage(inst, start, now).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(target: "loams_engines", id = %inst.id, error = %e, "usage sample failed");
                return out;
            }
        };
        // The service owns the window, so windows never overlap or leave gaps.
        s.window_start = start;
        s.window_end = now;
        let compute = inst.status.phase.has_compute();
        if !compute {
            s.cpu_ms = 0;
            s.memory_mib_ms = 0;
        }
        if compute {
            out.push(UsageEvent::runtime(&self.origin, inst, &s));
        }
        let storage_due = self.last_storage.lock().is_ok_and(|mut m| {
            let due = m.get(&inst.id).is_none_or(|last| {
                now - *last >= time::Duration::seconds(meter::STORAGE_EVERY_SECS)
            });
            if due {
                m.insert(inst.id.clone(), now);
            }
            due
        });
        if storage_due {
            out.push(UsageEvent::storage(&self.origin, inst, s.state_bytes, now));
        }
        self.usage.push(&inst.id, s);
        out
    }

    async fn meter_one(&self, inst: &Instance, now: OffsetDateTime) {
        let events = self.sample(inst, now).await;
        if !events.is_empty()
            && let Err(e) = self.meter.emit(events).await
        {
            tracing::warn!(target: "loams_engines", id = %inst.id, error = %e, "usage emit failed");
        }
    }

    /// One metering round over every instance that holds state, emitted as one batch.
    /// Returns how many events went out.
    pub async fn meter_tick(&self, now: OffsetDateTime) -> Result<usize, EngineError> {
        let mut events = Vec::new();
        for inst in self.store.list_all().await? {
            if self.is_busy(&inst.id) {
                // The operation holding it meters its own boundary.
                continue;
            }
            events.extend(self.sample(&inst, now).await);
        }
        let n = events.len();
        if n > 0 {
            self.meter.emit(events).await?;
        }
        Ok(n)
    }

    /// Runs metering and reconciliation every `every` until the handle is aborted.
    pub fn spawn_background(
        self: &Arc<Self>,
        every: std::time::Duration,
    ) -> tokio::task::JoinHandle<()> {
        let svc = Arc::clone(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                if let Err(e) = svc.meter_tick(OffsetDateTime::now_utc()).await {
                    tracing::warn!(target: "loams_engines", error = %e, "metering round failed");
                }
                if let Err(e) = svc.reconcile().await {
                    tracing::warn!(target: "loams_engines", error = %e, "reconcile round failed");
                }
            }
        })
    }
}

fn is_transitional(p: Phase) -> bool {
    matches!(
        p,
        Phase::Pending
            | Phase::Provisioning
            | Phase::Scaling
            | Phase::Upgrading
            | Phase::Suspending
            | Phase::Resuming
            | Phase::Deleting
    )
}
