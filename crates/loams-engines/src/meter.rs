//! Engine metering (D2013): one usage event per instance per window, in Loams Meter's event
//! shape (design 63 §6), handed to a [`MeterSink`].
//!
//! | Field | Value |
//! |---|---|
//! | `type` | `io.loams.dev.usage.engine.<kind>.v1` |
//! | `id` | `<instance>/<window_end unix seconds>`, derived from the work so a re-emission is a duplicate |
//! | `source` | `loams://<cluster>/engines/<node>` |
//! | `subject` | `org_<org>` (the billing subject) |
//! | `tenantid` | `<org>/<project>` |
//! | `data` | `instance`, `kind`, `tier`, `replicas`, `vcpu_seconds`, `memory_gb_seconds`, `state_bytes`, `window_start`, `window_end` |
//!
//! A suspended instance still emits `state_bytes` with zero compute. Adapters never emit.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Mutex, RwLock};

use async_trait::async_trait;
use serde::Serialize;
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::{EngineError, Instance, InstanceId, UsageSample};

/// A usage CloudEvent (design 63 §6.1), JSON data.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UsageEvent {
    pub specversion: &'static str,
    pub id: String,
    pub source: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub subject: String,
    pub time: String,
    pub datacontenttype: &'static str,
    pub dataschema: String,
    pub tenantid: String,
    pub data: Value,
}

/// The org an instance bills to when its record has none (single-org installs).
pub const DEFAULT_ORG: &str = "default";

/// Where this process sits, for `source`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeterOrigin {
    pub cluster: String,
    pub node: String,
}

impl Default for MeterOrigin {
    fn default() -> Self {
        Self {
            cluster: "local".into(),
            node: "node-0".into(),
        }
    }
}

fn rfc3339(t: OffsetDateTime) -> String {
    t.format(&Rfc3339)
        .unwrap_or_else(|_| t.unix_timestamp().to_string())
}

impl UsageEvent {
    pub fn event_type(inst: &Instance) -> String {
        format!("io.loams.dev.usage.engine.{}.v1", inst.spec.kind.as_str())
    }

    /// The event for one sample of `inst`.
    pub fn new(origin: &MeterOrigin, inst: &Instance, sample: &UsageSample) -> Self {
        let ty = Self::event_type(inst);
        let org = inst.org.as_deref().unwrap_or(DEFAULT_ORG);
        Self {
            specversion: "1.0",
            id: format!("{}/{}", inst.id, sample.window_end.unix_timestamp()),
            source: format!("loams://{}/engines/{}", origin.cluster, origin.node),
            dataschema: format!("urn:loams:json:{ty}"),
            ty,
            subject: format!("org_{org}"),
            time: rfc3339(sample.window_end),
            datacontenttype: "application/json",
            tenantid: format!("{org}/{}", inst.project),
            data: json!({
                "instance": inst.id.as_str(),
                "kind": inst.spec.kind.as_str(),
                "tier": inst.spec.size.tier,
                "replicas": inst.spec.size.replicas,
                "vcpu_seconds": sample.vcpu_seconds,
                "memory_gb_seconds": sample.memory_gb_seconds,
                "state_bytes": sample.state_bytes,
                "window_start": rfc3339(sample.window_start),
                "window_end": rfc3339(sample.window_end),
            }),
        }
    }
}

/// Loams Meter's ingestion seam (design 63 §7.2, path 1: best effort). The production sink
/// produces to the `_meter` stream; that lands with ME1's `MeterSink`, which this trait
/// mirrors so the swap is one impl.
#[async_trait]
pub trait MeterSink: Send + Sync + 'static {
    async fn emit(&self, events: Vec<UsageEvent>) -> Result<(), EngineError>;
}

/// Drops every event (metering off).
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopMeterSink;

#[async_trait]
impl MeterSink for NoopMeterSink {
    async fn emit(&self, _: Vec<UsageEvent>) -> Result<(), EngineError> {
        Ok(())
    }
}

/// Keeps every event, for tests.
#[derive(Debug, Default)]
pub struct MemoryMeterSink {
    events: Mutex<Vec<UsageEvent>>,
}

impl MemoryMeterSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn events(&self) -> Vec<UsageEvent> {
        self.events.lock().map(|e| e.clone()).unwrap_or_default()
    }
}

#[async_trait]
impl MeterSink for MemoryMeterSink {
    async fn emit(&self, mut events: Vec<UsageEvent>) -> Result<(), EngineError> {
        self.events
            .lock()
            .map_err(|_| EngineError::Internal("meter sink lock poisoned".into()))?
            .append(&mut events);
        Ok(())
    }
}

/// Samples kept per instance for `GET …/usage`: a day of one-minute windows.
pub const USAGE_HISTORY: usize = 1440;

/// The recent samples of each instance, for the console's metrics. Billing reads the Meter,
/// not this.
#[derive(Debug, Default)]
pub struct UsageLog {
    samples: RwLock<BTreeMap<InstanceId, VecDeque<UsageSample>>>,
}

impl UsageLog {
    pub fn push(&self, id: &InstanceId, sample: UsageSample) {
        if let Ok(mut map) = self.samples.write() {
            let q = map.entry(id.clone()).or_default();
            if q.len() == USAGE_HISTORY {
                q.pop_front();
            }
            q.push_back(sample);
        }
    }

    /// The end of the newest window, where the next one starts.
    pub fn last_end(&self, id: &InstanceId) -> Option<OffsetDateTime> {
        self.samples
            .read()
            .ok()?
            .get(id)?
            .back()
            .map(|s| s.window_end)
    }

    /// Samples whose window overlaps `[from, to)`.
    pub fn range(
        &self,
        id: &InstanceId,
        from: Option<OffsetDateTime>,
        to: Option<OffsetDateTime>,
    ) -> Vec<UsageSample> {
        let Ok(map) = self.samples.read() else {
            return Vec::new();
        };
        map.get(id)
            .map(|q| {
                q.iter()
                    .filter(|s| from.is_none_or(|f| s.window_end > f))
                    .filter(|s| to.is_none_or(|t| s.window_start < t))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn forget(&self, id: &InstanceId) {
        if let Ok(mut map) = self.samples.write() {
            map.remove(id);
        }
    }
}

/// `GET …/usage`'s body.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Usage {
    pub samples: Vec<UsageSample>,
    pub totals: UsageTotals,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UsageTotals {
    pub vcpu_seconds: f64,
    pub memory_gb_seconds: f64,
    /// The newest sample's state bytes (a gauge, not a sum).
    pub state_bytes: u64,
}

impl Usage {
    pub fn from_samples(samples: Vec<UsageSample>) -> Self {
        let totals = UsageTotals {
            vcpu_seconds: samples.iter().map(|s| s.vcpu_seconds).sum(),
            memory_gb_seconds: samples.iter().map(|s| s.memory_gb_seconds).sum(),
            state_bytes: samples.last().map_or(0, |s| s.state_bytes),
        };
        Self { samples, totals }
    }
}
