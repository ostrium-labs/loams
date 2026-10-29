//! The instances of one `connect` process: each one a running SDK pipeline,
//! its operator controls (pause, rate) kept in the bucket so they survive a
//! restart, and its status. CN2's `NativeRuntime` replaces this scheduler
//! later (D1821); the pipelines, the Fabric I/O and the state stay.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use loams_connector_sdk::{
    ConnectorContext, DeadLetterQueue, InstanceKey, LastError, Pipeline, PipelineHandle,
    PipelineState, Rate, RateLimit, Scrubber,
};
use loams_flow::secrets::{NamespaceResolver, SecretStore};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::engine::{EngineClient, EngineDeadLetters, EngineReader, EngineWriter};
use super::instances::{Dir, Instance};
use super::state::{BucketCheckpoints, PositionDoc, StateStore};

/// How often a sink's committed position is written to the bucket.
pub const POSITION_INTERVAL: Duration = Duration::from_secs(5);

/// What the role runs on.
#[derive(Clone)]
pub struct Deps {
    pub engine: EngineClient,
    pub state: StateStore,
    pub secrets: Arc<dyn SecretStore>,
    /// One scrubber for the process: every instance records its resolved
    /// secrets here, and the log, statuses and errors are scrubbed with it.
    pub scrubber: Arc<Scrubber>,
    /// The lock's epoch: every checkpoint is written under it.
    pub epoch: u64,
}

impl std::fmt::Debug for Deps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Deps")
            .field("engine", &self.engine)
            .field("state", &self.state)
            .field("epoch", &self.epoch)
            .finish_non_exhaustive()
    }
}

/// An operator's rate limit, as the admin API takes it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct RateLimitBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_per_sec: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requests_per_sec: Option<f64>,
}

impl RateLimitBody {
    pub fn of(r: &RateLimit) -> Self {
        Self {
            bytes_per_sec: r.bytes_per_sec(),
            requests_per_sec: r.requests_per_sec(),
        }
    }

    pub fn to_sdk(self) -> Result<RateLimit, String> {
        let rate = |v: Option<f64>, name: &str| match v {
            None => Ok(None),
            Some(n) if n.is_finite() && n > 0.0 => Ok(Some(Rate::per_second(n))),
            Some(_) => Err(format!("rate_limit.{name} must be a positive number")),
        };
        Ok(RateLimit {
            bytes: rate(self.bytes_per_sec, "bytes_per_sec")?,
            requests: rate(self.requests_per_sec, "requests_per_sec")?,
        })
    }
}

/// An instance's operator controls (`instances/<name>/control.json`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Control {
    #[serde(default)]
    pub paused: bool,
    /// Overrides the instance file's `rate_limit` while set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<RateLimitBody>,
}

struct Running {
    handle: PipelineHandle,
    position: Option<Arc<PositionDoc>>,
}

struct Slot {
    instance: Instance,
    control: Control,
    run: Option<Running>,
    /// Why the last start failed (scrubbed), until a start succeeds.
    start_error: Option<String>,
}

/// An admin operation's failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminError {
    NotFound(String),
    Invalid(String),
    Unavailable(String),
}

/// The process's instances.
pub struct Supervisor {
    deps: Deps,
    slots: Mutex<BTreeMap<String, Slot>>,
}

impl std::fmt::Debug for Supervisor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Supervisor")
            .field("epoch", &self.deps.epoch)
            .finish_non_exhaustive()
    }
}

impl Supervisor {
    pub fn new(deps: Deps, instances: Vec<Instance>) -> Arc<Self> {
        let slots = instances
            .into_iter()
            .map(|i| {
                (
                    i.name.clone(),
                    Slot {
                        instance: i,
                        control: Control::default(),
                        run: None,
                        start_error: None,
                    },
                )
            })
            .collect();
        Arc::new(Self {
            deps,
            slots: Mutex::new(slots),
        })
    }

    pub fn scrubber(&self) -> &Arc<Scrubber> {
        &self.deps.scrubber
    }

    /// Load every instance's controls and start it. A start that fails is
    /// recorded and retried by [`Supervisor::retry_failed_starts`].
    pub async fn start_all(&self) {
        let mut slots = self.slots.lock().await;
        for slot in slots.values_mut() {
            match self
                .deps
                .state
                .get_json(&format!("instances/{}/control.json", slot.instance.name))
                .await
            {
                Ok(Some(v)) => {
                    if let Ok(c) = serde_json::from_value(v) {
                        slot.control = c;
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(instance = %slot.instance.name, "reading its controls: {e}")
                }
            }
            self.start_slot(slot).await;
        }
    }

    /// Start every instance whose last start failed.
    pub async fn retry_failed_starts(&self) {
        let mut slots = self.slots.lock().await;
        for slot in slots.values_mut() {
            if slot.run.is_none() && slot.start_error.is_some() {
                self.start_slot(slot).await;
            }
        }
    }

    fn effective_rate(slot: &Slot) -> RateLimit {
        slot.control
            .rate_limit
            .and_then(|r| r.to_sdk().ok())
            .unwrap_or(slot.instance.rate_limit)
    }

    async fn start_slot(&self, slot: &mut Slot) {
        match self.start(slot).await {
            Ok(run) => {
                tracing::info!(
                    instance = %slot.instance.name,
                    connector = %slot.instance.connector,
                    direction = slot.instance.direction.as_str(),
                    paused = slot.control.paused,
                    "started"
                );
                slot.run = Some(run);
                slot.start_error = None;
            }
            Err(e) => {
                let e = self.deps.scrubber.scrub(&e);
                tracing::warn!(instance = %slot.instance.name, "could not start: {e}");
                slot.start_error = Some(e);
            }
        }
    }

    async fn start(&self, slot: &Slot) -> Result<Running, String> {
        let i = &slot.instance;
        let d = &self.deps;
        let key = InstanceKey::new(i.namespace_id, i.name.clone());
        let resolver = Arc::new(NamespaceResolver::new(d.secrets.clone(), i.namespace_id));
        let mut ctx =
            ConnectorContext::new(key.clone(), i.config.clone(), resolver, d.scrubber.clone());
        ctx.profile = i.profile.clone();
        let dlq: Option<Arc<dyn DeadLetterQueue>> = match &i.dead_letter_stream {
            Some(s) => {
                d.engine
                    .ensure_stream(&i.namespace, s, 1)
                    .await
                    .map_err(|e| format!("the dead-letter stream {s}: {e}"))?;
                Some(Arc::new(EngineDeadLetters::new(EngineWriter::new(
                    d.engine.clone(),
                    i.namespace.clone(),
                    s.clone(),
                ))))
            }
            None => None,
        };
        let rate = Self::effective_rate(slot);
        match i.direction {
            Dir::Source => {
                d.engine
                    .ensure_stream(&i.namespace, &i.stream, i.stream_partitions)
                    .await
                    .map_err(|e| format!("the stream {}/{}: {e}", i.namespace, i.stream))?;
                let (enumerator, sources) = i
                    .factory
                    .source(&ctx)
                    .map_err(|e| e.message)?
                    .ok_or_else(|| format!("{} has no source", i.connector))?;
                let mut p = Pipeline::source(key, i.pipeline.clone())
                    .epoch(d.epoch)
                    .scrubber(d.scrubber.clone())
                    .rate_limit(rate)
                    .start_paused(slot.control.paused)
                    .enumerator(enumerator)
                    .sources(Arc::from(sources))
                    .writer(Arc::new(EngineWriter::new(
                        d.engine.clone(),
                        i.namespace.clone(),
                        i.stream.clone(),
                    )))
                    .checkpoints(Arc::new(BucketCheckpoints::new(&d.state, &i.name)));
                if let Some(q) = dlq {
                    p = p.dead_letters(q);
                }
                let handle = p.start().await.map_err(|e| e.message)?;
                Ok(Running {
                    handle,
                    position: None,
                })
            }
            Dir::Sink => {
                let sink = i
                    .factory
                    .sink(&ctx)
                    .map_err(|e| e.message)?
                    .ok_or_else(|| format!("{} has no sink", i.connector))?;
                let position = PositionDoc::new(&d.state, &i.name, POSITION_INTERVAL);
                let reader = EngineReader::open(
                    d.engine.clone(),
                    i.namespace.clone(),
                    i.stream.clone(),
                    position.clone(),
                )
                .await
                .map_err(|e| format!("the stream {}/{}: {}", i.namespace, i.stream, e.message))?;
                let mut p = Pipeline::sink(key, i.pipeline.clone())
                    .epoch(d.epoch)
                    .scrubber(d.scrubber.clone())
                    .rate_limit(rate)
                    .start_paused(slot.control.paused)
                    .reader(Box::new(reader))
                    .sink(Arc::from(sink));
                if let Some(q) = dlq {
                    p = p.dead_letters(q);
                }
                let handle = p.start().await.map_err(|e| e.message)?;
                Ok(Running {
                    handle,
                    position: Some(position),
                })
            }
        }
    }

    async fn save_control(&self, name: &str, c: &Control) -> Result<(), AdminError> {
        let v = serde_json::to_value(c).unwrap_or(Value::Null);
        self.deps
            .state
            .put_json(&format!("instances/{name}/control.json"), &v)
            .await
            .map_err(|e| AdminError::Unavailable(e.to_string()))
    }

    /// Pause: no new reads until resumed; survives a restart.
    pub async fn pause(&self, name: &str) -> Result<Value, AdminError> {
        let mut slots = self.slots.lock().await;
        let slot = slots
            .get_mut(name)
            .ok_or_else(|| AdminError::NotFound(name.to_owned()))?;
        let mut c = slot.control.clone();
        c.paused = true;
        self.save_control(name, &c).await?;
        slot.control = c;
        if let Some(r) = &slot.run {
            r.handle.pause();
        }
        tracing::info!(instance = %name, "paused");
        Ok(self.status_of(slot))
    }

    pub async fn resume(&self, name: &str) -> Result<Value, AdminError> {
        let mut slots = self.slots.lock().await;
        let slot = slots
            .get_mut(name)
            .ok_or_else(|| AdminError::NotFound(name.to_owned()))?;
        let mut c = slot.control.clone();
        c.paused = false;
        self.save_control(name, &c).await?;
        slot.control = c;
        if let Some(r) = &slot.run {
            r.handle.resume();
        }
        tracing::info!(instance = %name, "resumed");
        Ok(self.status_of(slot))
    }

    /// Set (or, with `None`, clear) the operator's rate limit.
    pub async fn set_rate(
        &self,
        name: &str,
        rate: Option<RateLimitBody>,
    ) -> Result<Value, AdminError> {
        if let Some(r) = rate {
            r.to_sdk().map_err(AdminError::Invalid)?;
        }
        let mut slots = self.slots.lock().await;
        let slot = slots
            .get_mut(name)
            .ok_or_else(|| AdminError::NotFound(name.to_owned()))?;
        let mut c = slot.control.clone();
        c.rate_limit = rate;
        self.save_control(name, &c).await?;
        slot.control = c;
        let eff = Self::effective_rate(slot);
        if let Some(r) = &slot.run {
            r.handle.set_rate_limit(eff);
        }
        tracing::info!(instance = %name, bytes_per_sec = ?eff.bytes_per_sec(), "rate limit set");
        Ok(self.status_of(slot))
    }

    /// Stop gracefully and start again (new secrets, after a key rotation).
    pub async fn restart(&self, name: &str) -> Result<Value, AdminError> {
        let mut slots = self.slots.lock().await;
        let slot = slots
            .get_mut(name)
            .ok_or_else(|| AdminError::NotFound(name.to_owned()))?;
        if let Some(r) = slot.run.take() {
            Self::stop_running(r).await;
        }
        self.start_slot(slot).await;
        tracing::info!(instance = %name, "restarted");
        Ok(self.status_of(slot))
    }

    async fn stop_running(r: Running) {
        r.handle.stop().await;
        if let Some(p) = r.position {
            p.flush().await;
        }
    }

    /// Stop every instance gracefully (drain, checkpoint, close).
    pub async fn stop_all(&self) {
        let mut slots = self.slots.lock().await;
        for slot in slots.values_mut() {
            if let Some(r) = slot.run.take() {
                Self::stop_running(r).await;
            }
        }
        drop(slots);
        self.write_statuses().await;
    }

    /// Fenced: abort everything at once, with no final checkpoint.
    pub async fn abort_all(&self) {
        let mut slots = self.slots.lock().await;
        for slot in slots.values_mut() {
            if let Some(r) = slot.run.take() {
                r.handle.abort();
            }
        }
    }

    /// Whether any pipeline found itself fenced by a successor.
    pub async fn any_fenced(&self) -> bool {
        let slots = self.slots.lock().await;
        slots.values().any(|s| {
            s.run
                .as_ref()
                .is_some_and(|r| r.handle.status().state == PipelineState::Fenced)
        })
    }

    pub async fn list(&self) -> Value {
        let slots = self.slots.lock().await;
        json!({"instances": slots.values().map(|s| self.status_of(s)).collect::<Vec<_>>()})
    }

    pub async fn status(&self, name: &str) -> Result<Value, AdminError> {
        let slots = self.slots.lock().await;
        slots
            .get(name)
            .map(|s| self.status_of(s))
            .ok_or_else(|| AdminError::NotFound(name.to_owned()))
    }

    /// The handle's status of a running instance (tests wait on it).
    pub async fn pipeline_state(&self, name: &str) -> Option<PipelineState> {
        let slots = self.slots.lock().await;
        slots
            .get(name)
            .and_then(|s| s.run.as_ref())
            .map(|r| r.handle.status().state)
    }

    /// Every instance's status into `instances/<name>/status.json`.
    pub async fn write_statuses(&self) {
        let statuses: Vec<(String, Value)> = {
            let slots = self.slots.lock().await;
            slots
                .iter()
                .map(|(n, s)| (n.clone(), self.status_of(s)))
                .collect()
        };
        for (name, v) in statuses {
            if let Err(e) = self
                .deps
                .state
                .put_json(&format!("instances/{name}/status.json"), &v)
                .await
            {
                tracing::warn!(instance = %name, "writing its status: {e}");
            }
        }
    }

    fn status_of(&self, s: &Slot) -> Value {
        let i = &s.instance;
        let scrub = |t: &str| self.deps.scrubber.scrub(t);
        let (state, last_error, splits) = match &s.run {
            Some(r) => {
                let st = r.handle.status();
                (
                    match st.state {
                        PipelineState::Running => "running",
                        PipelineState::Paused => "paused",
                        PipelineState::Stopped => "stopped",
                        PipelineState::Failed => "failed",
                        PipelineState::Fenced => "fenced",
                    },
                    st.last_error.as_ref().map(|e| error_json(e, &scrub)),
                    st.splits
                        .iter()
                        .map(|s| s.as_str().to_owned())
                        .collect::<Vec<_>>(),
                )
            }
            None => (
                if s.start_error.is_some() {
                    "start_failed"
                } else {
                    "stopped"
                },
                s.start_error
                    .as_ref()
                    .map(|e| json!({"class": "start", "message": scrub(e)})),
                Vec::new(),
            ),
        };
        let rate = s
            .run
            .as_ref()
            .map_or_else(|| Self::effective_rate(s), |r| r.handle.rate_limit());
        json!({
            "name": i.name,
            "connector": i.connector,
            "profile": i.profile.as_ref().map(|p| p.id.clone()),
            "direction": i.direction.as_str(),
            "namespace": i.namespace,
            "stream": i.stream,
            "state": state,
            "paused_by_operator": s.control.paused,
            "rate_limit": RateLimitBody::of(&rate),
            "rate_limit_overridden": s.control.rate_limit.is_some(),
            "last_error": last_error,
            "splits": splits,
            "epoch": self.deps.epoch,
        })
    }
}

fn error_json(e: &LastError, scrub: &dyn Fn(&str) -> String) -> Value {
    json!({
        "class": format!("{:?}", e.class),
        "message": scrub(&e.message),
        "hint": e.hint,
    })
}
