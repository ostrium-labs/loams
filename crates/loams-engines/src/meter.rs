//! Engine metering (D2013, ENG81 R17/D1920), in Loams Meter's event shape (design 63 §6), handed
//! to a [`MeterSink`]. Two types for all three engines:
//!
//! | Type | When | `id` | `data` |
//! |---|---|---|---|
//! | [`RUNTIME_TYPE`] | every window (60 s) while compute runs | `<instance>/<window_end unix>` | `namespace`, `instance`, `engine`, `size`, `cpu_ms`, `memory_mib_ms`, `window_start`, `window_end`, `estimated` |
//! | [`STORAGE_TYPE`] | hourly while state exists | `<instance>/storage/<unix>` | `namespace`, `instance`, `engine`, `state_bytes` |
//!
//! `source` is `loams://<cluster>/engines/<node>`, `subject` `org_<org>` (the billing subject),
//! `tenantid` `<org>/<namespace>`. Ids derive from the work, so a re-emission is a duplicate.
//! A suspended instance emits storage only. Adapters never emit.

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

/// The runtime meter's type (ENG81 R17, D1920): one type for all three engines.
pub const RUNTIME_TYPE: &str = "io.loams.dev.usage.engine.runtime.v1";
/// The storage gauge's type, emitted hourly.
pub const STORAGE_TYPE: &str = "io.loams.dev.usage.engine.storage.v1";
/// How often the storage gauge is emitted, in seconds.
pub const STORAGE_EVERY_SECS: i64 = 3600;

impl UsageEvent {
    fn envelope(
        origin: &MeterOrigin,
        inst: &Instance,
        ty: &str,
        id: String,
        at: OffsetDateTime,
        data: Value,
    ) -> Self {
        let org = inst.org.as_deref().unwrap_or(DEFAULT_ORG);
        Self {
            specversion: "1.0",
            id,
            source: format!("loams://{}/engines/{}", origin.cluster, origin.node),
            ty: ty.to_owned(),
            subject: format!("org_{org}"),
            time: rfc3339(at),
            datacontenttype: "application/json",
            dataschema: format!("urn:loams:json:{ty}"),
            tenantid: format!("{org}/{}", inst.namespace),
            data,
        }
    }

    /// The runtime event for one window of compute.
    pub fn runtime(origin: &MeterOrigin, inst: &Instance, sample: &UsageSample) -> Self {
        Self::envelope(
            origin,
            inst,
            RUNTIME_TYPE,
            format!("{}/{}", inst.id, sample.window_end.unix_timestamp()),
            sample.window_end,
            json!({
                "namespace": inst.namespace,
                "instance": inst.id.as_str(),
                "engine": inst.spec.kind.as_str(),
                "size": inst.spec.size.tier,
                "cpu_ms": sample.cpu_ms,
                "memory_mib_ms": sample.memory_mib_ms,
                "window_start": rfc3339(sample.window_start),
                "window_end": rfc3339(sample.window_end),
                "estimated": sample.estimated,
            }),
        )
    }

    /// The storage gauge at `at`.
    pub fn storage(
        origin: &MeterOrigin,
        inst: &Instance,
        state_bytes: u64,
        at: OffsetDateTime,
    ) -> Self {
        Self::envelope(
            origin,
            inst,
            STORAGE_TYPE,
            format!("{}/storage/{}", inst.id, at.unix_timestamp()),
            at,
            json!({
                "namespace": inst.namespace,
                "instance": inst.id.as_str(),
                "engine": inst.spec.kind.as_str(),
                "state_bytes": state_bytes,
            }),
        )
    }
}

/// Loams Meter's ingestion seam (design 63 §7.2, path 1: best effort). The production sink
/// produces to the `_meter` stream; that lands with ME1's `MeterSink`, which this trait
/// mirrors so the swap is one impl.
#[async_trait]
pub trait MeterSink: Send + Sync + 'static {
    async fn emit(&self, events: Vec<UsageEvent>) -> Result<(), EngineError>;
}

/// What the entitlement gate is asked about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GateOp {
    Create,
    Resume,
    /// A scale that adds vCPU or memory.
    ScaleUp,
}

/// Loams Meter's enforcement gate (design 63 §10), asked before an operation that adds
/// compute. A refusal is [`EngineError::QuotaExceeded`] (HTTP 402).
#[async_trait]
pub trait MeterGate: Send + Sync + 'static {
    async fn admit(&self, inst: &Instance, op: GateOp) -> Result<(), EngineError>;
}

/// Admits everything (no Meter, or the `meter` role is off).
#[derive(Debug, Default, Clone, Copy)]
pub struct OpenGate;

#[async_trait]
impl MeterGate for OpenGate {
    async fn admit(&self, _: &Instance, _: GateOp) -> Result<(), EngineError> {
        Ok(())
    }
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Usage {
    pub samples: Vec<UsageSample>,
    pub totals: UsageTotals,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UsageTotals {
    pub cpu_ms: u64,
    pub memory_mib_ms: u64,
    /// The newest sample's state bytes (a gauge, not a sum).
    pub state_bytes: u64,
}

impl Usage {
    pub fn from_samples(samples: Vec<UsageSample>) -> Self {
        let totals = UsageTotals {
            cpu_ms: samples
                .iter()
                .map(|s| s.cpu_ms)
                .fold(0, u64::saturating_add),
            memory_mib_ms: samples
                .iter()
                .map(|s| s.memory_mib_ms)
                .fold(0, u64::saturating_add),
            state_bytes: samples.last().map_or(0, |s| s.state_bytes),
        };
        Self { samples, totals }
    }
}
