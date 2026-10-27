//! The lifecycle service: owns the records, the phase machine and metering, and calls the
//! adapters (D2010, D2012, D2013). The HTTP facade is a thin layer over this.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::meter::{MeterOrigin, MeterSink, NoopMeterSink, Usage, UsageEvent, UsageLog};
use crate::{
    CreateSession, EngineCatalog, EngineError, EngineKind, EngineRegistry, Instance, InstanceId,
    InstanceSpec, InstanceStatus, InstanceStore, ManagedEngine, MemoryStore, Observed, Op, Phase,
    SizeRequest, SparkEngine, SparkJob, SparkSession, SqlRequest, SqlResult, SubmitJob,
    validate_name,
};

/// `POST /v1/projects/{p}/engines`.
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
    #[serde(default)]
    pub environment: Option<String>,
}

/// `PATCH /v1/projects/{p}/engines/{id}`: exactly one of the two.
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
    usage: UsageLog,
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
            usage: UsageLog::default(),
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

    /// The instance, if it is in `project`. Another project's instance is not found, never
    /// forbidden, so ids do not leak across projects.
    async fn load(&self, project: &str, id: &str) -> Result<Instance, EngineError> {
        let id = InstanceId(id.to_owned());
        match self.store.get(&id).await? {
            Some(inst) if inst.project == project => Ok(inst),
            _ => Err(EngineError::NotFound(format!(
                "no engine instance {id} in project {project}"
            ))),
        }
    }

    pub async fn list(
        &self,
        project: &str,
        kind: Option<EngineKind>,
    ) -> Result<Vec<Instance>, EngineError> {
        let mut all = self.store.list(project).await?;
        if let Some(kind) = kind {
            all.retain(|i| i.spec.kind == kind);
        }
        Ok(all)
    }

    /// One instance. While it is in a transitional phase and nothing else holds it, this asks
    /// the adapter first, so polling shows progress without a reconciler.
    pub async fn get(&self, project: &str, id: &str) -> Result<Instance, EngineError> {
        let inst = self.load(project, id).await?;
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
        project: &str,
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
            state_prefix: Instance::state_prefix_for(project, &id),
            id,
            project: project.to_owned(),
            org,
            environment: req.environment,
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
            let taken = self
                .store
                .list(project)
                .await?
                .iter()
                .any(|i| i.spec.name == inst.spec.name && i.status.phase != Phase::Deleted);
            if taken {
                return Err(EngineError::Conflict(format!(
                    "project {project} already has an engine instance named {:?}",
                    inst.spec.name
                )));
            }
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
        project: &str,
        id: &str,
        op: Op,
    ) -> Result<(Busy<'_>, Instance, Arc<dyn ManagedEngine>), EngineError> {
        let inst = self.load(project, id).await?;
        let busy = self.claim(&inst.id)?;
        // Reload under the claim: another operation may have finished in between.
        let inst = self.load(project, id).await?;
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
        project: &str,
        id: &str,
        req: UpdateInstance,
    ) -> Result<Instance, EngineError> {
        match (req.size, req.version) {
            (Some(size), None) => self.scale(project, id, size).await,
            (None, Some(version)) => self.upgrade(project, id, &version).await,
            _ => Err(EngineError::Invalid(
                "set exactly one of size (scale) or version (upgrade)".into(),
            )),
        }
    }

    pub async fn scale(
        &self,
        project: &str,
        id: &str,
        size: SizeRequest,
    ) -> Result<Instance, EngineError> {
        let to = size.resolve()?;
        let (_busy, before, engine) = self.begin(project, id, Op::Scale).await?;
        let mut after = before.clone();
        after.spec.size = to;
        if let Err(e) = engine.validate(&after.spec) {
            // Put back the phase `begin` moved.
            self.store.put(&before).await?;
            return Err(e);
        }
        let outcome = engine.scale(&before, &to).await;
        self.finish(before, after, outcome).await
    }

    pub async fn upgrade(
        &self,
        project: &str,
        id: &str,
        version: &str,
    ) -> Result<Instance, EngineError> {
        let (_busy, before, engine) = self.begin(project, id, Op::Upgrade).await?;
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

    pub async fn suspend(&self, project: &str, id: &str) -> Result<Instance, EngineError> {
        let (_busy, before, engine) = self.begin(project, id, Op::Suspend).await?;
        // Bill the compute up to now before it stops.
        self.meter_one(&before, OffsetDateTime::now_utc()).await;
        let outcome = engine.suspend(&before).await;
        let after = before.clone();
        self.finish(before, after, outcome).await
    }

    pub async fn resume(&self, project: &str, id: &str) -> Result<Instance, EngineError> {
        let (_busy, before, engine) = self.begin(project, id, Op::Resume).await?;
        // Close the suspended window (state only) before compute restarts.
        self.meter_one(&before, OffsetDateTime::now_utc()).await;
        let outcome = engine.resume(&before).await;
        let after = before.clone();
        self.finish(before, after, outcome).await
    }

    /// Deletes the runtime, its state and the record. Answers the record in `deleted`.
    pub async fn delete(&self, project: &str, id: &str) -> Result<Instance, EngineError> {
        let (_busy, before, engine) = self.begin(project, id, Op::Delete).await?;
        // The last partial window.
        self.meter_one(&before, OffsetDateTime::now_utc()).await;
        match engine.delete(&before).await {
            Ok(()) => {
                self.store.remove(&before.id).await?;
                self.usage.forget(&before.id);
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
        project: &str,
        id: &str,
        from: Option<OffsetDateTime>,
        to: Option<OffsetDateTime>,
    ) -> Result<Usage, EngineError> {
        let inst = self.load(project, id).await?;
        Ok(Usage::from_samples(self.usage.range(&inst.id, from, to)))
    }

    /// The instance and adapter for a call that needs the engine up.
    async fn running(
        &self,
        project: &str,
        id: &str,
    ) -> Result<(Instance, Arc<dyn ManagedEngine>), EngineError> {
        let inst = self.load(project, id).await?;
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
        project: &str,
        id: &str,
        req: SqlRequest,
    ) -> Result<SqlResult, EngineError> {
        req.validate()?;
        let (inst, engine) = self.running(project, id).await?;
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
        project: &str,
        id: &str,
    ) -> Result<Vec<SparkSession>, EngineError> {
        let (inst, engine) = self.running(project, id).await?;
        Self::spark_of(&inst, &engine)?.list_sessions(&inst).await
    }

    pub async fn create_session(
        &self,
        project: &str,
        id: &str,
        req: CreateSession,
    ) -> Result<SparkSession, EngineError> {
        check_map("conf", &req.conf)?;
        let (inst, engine) = self.running(project, id).await?;
        Self::spark_of(&inst, &engine)?
            .create_session(&inst, req)
            .await
    }

    pub async fn close_session(
        &self,
        project: &str,
        id: &str,
        session: &str,
    ) -> Result<(), EngineError> {
        let (inst, engine) = self.running(project, id).await?;
        Self::spark_of(&inst, &engine)?
            .close_session(&inst, session)
            .await
    }

    pub async fn list_jobs(&self, project: &str, id: &str) -> Result<Vec<SparkJob>, EngineError> {
        let (inst, engine) = self.running(project, id).await?;
        Self::spark_of(&inst, &engine)?.list_jobs(&inst).await
    }

    pub async fn submit_job(
        &self,
        project: &str,
        id: &str,
        req: SubmitJob,
    ) -> Result<SparkJob, EngineError> {
        req.validate()?;
        check_map("conf", &req.conf)?;
        let (inst, engine) = self.running(project, id).await?;
        Self::spark_of(&inst, &engine)?.submit_job(&inst, req).await
    }

    pub async fn cancel_job(
        &self,
        project: &str,
        id: &str,
        job: &str,
    ) -> Result<SparkJob, EngineError> {
        let (inst, engine) = self.running(project, id).await?;
        Self::spark_of(&inst, &engine)?.cancel_job(&inst, job).await
    }

    /// Samples one instance up to `now`, records it and returns its event. `None` when the
    /// window is empty, the adapter is gone or it could not measure (the next window covers
    /// the gap, since it starts where the last recorded one ended).
    async fn sample(&self, inst: &Instance, now: OffsetDateTime) -> Option<UsageEvent> {
        if !inst.status.phase.has_state() {
            return None;
        }
        let window = time::Duration::seconds(self.window_secs);
        let start = self
            .usage
            .last_end(&inst.id)
            .unwrap_or_else(|| inst.created_at.max(now - window));
        if now <= start {
            return None;
        }
        let engine = self.registry.get(inst.spec.kind).ok()?;
        let mut s = match engine.usage(inst, start, now).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(target: "loams_engines", id = %inst.id, error = %e, "usage sample failed");
                return None;
            }
        };
        // The service owns the window, so windows never overlap or leave gaps.
        s.window_start = start;
        s.window_end = now;
        if !inst.status.phase.has_compute() {
            s.vcpu_seconds = 0.0;
            s.memory_gb_seconds = 0.0;
        }
        self.usage.push(&inst.id, s.clone());
        Some(UsageEvent::new(&self.origin, inst, &s))
    }

    async fn meter_one(&self, inst: &Instance, now: OffsetDateTime) {
        if let Some(ev) = self.sample(inst, now).await
            && let Err(e) = self.meter.emit(vec![ev]).await
        {
            tracing::warn!(target: "loams_engines", id = %inst.id, error = %e, "usage emit failed");
        }
    }

    /// One metering round: a sample per instance that holds state, emitted as one batch.
    /// Returns how many events went out.
    pub async fn meter_tick(&self, now: OffsetDateTime) -> Result<usize, EngineError> {
        let mut events = Vec::new();
        for inst in self.store.list_all().await? {
            if self.is_busy(&inst.id) {
                // The operation holding it meters its own boundary.
                continue;
            }
            if let Some(ev) = self.sample(&inst, now).await {
                events.push(ev);
            }
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
