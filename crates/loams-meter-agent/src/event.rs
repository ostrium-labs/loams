//! Meter events: CloudEvents 1.0 in JSON structured mode (design 01 §4,
//! PD2).
//!
//! One event is one unsigned delta of one meter, for one tenant, over one
//! window, measured on one resource. Its `id` is the idempotency key
//! `<node>/<source>/<seq>`: the agent assigns `seq` before the event enters
//! the spool and reuses it on every retry, so a replay carries the same id
//! and the aggregator (or D72's idempotent producers) drops it.

use std::fmt;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// The CloudEvents `type` of every meter event.
pub const EVENT_TYPE: &str = "dev.loams.meter.v1";

/// The CloudEvents spec version.
pub const SPEC_VERSION: &str = "1.0";

/// The org that unattributed usage is metered under (design 01 §7.5). It is never
/// billed; it is priced into the request fee.
pub const PLATFORM_ORG: &str = "_platform";

/// What is measured. The unit is part of the name (design 01 §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Meter {
    /// CPU microseconds (a counter).
    CpuUsec,
    /// Bytes × seconds of resident memory while the cgroup is populated
    /// (a gauge integral).
    MemoryByteSeconds,
    /// The peak resident bytes (a gauge).
    MemoryPeakBytes,
    /// wasmtime fuel units (a counter).
    WasmFuel,
    /// Requests at the edge (a counter).
    Requests,
    /// Bytes received at the edge (a counter).
    IngressBytes,
    /// Bytes sent (a counter).
    EgressBytes,
    /// Durable promises created (a counter).
    DurablePromises,
    /// Durable timers scheduled (a counter).
    DurableTimers,
}

impl Meter {
    /// The name on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Meter::CpuUsec => "cpu_usec",
            Meter::MemoryByteSeconds => "memory_byte_seconds",
            Meter::MemoryPeakBytes => "memory_peak_bytes",
            Meter::WasmFuel => "wasm_fuel",
            Meter::Requests => "requests",
            Meter::IngressBytes => "ingress_bytes",
            Meter::EgressBytes => "egress_bytes",
            Meter::DurablePromises => "durable_promises",
            Meter::DurableTimers => "durable_timers",
        }
    }
}

impl fmt::Display for Meter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where an event was read (the `<source>` of the idempotency key).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// cgroup v2 files (design 01 §5.1, §5.2).
    Cgroup,
    /// `/proc/<pid>/stat` (design 01 §5.2).
    Proc,
    /// A wasmtime host's reports (design 01 §5.3).
    Wasm,
    /// Envoy access logs (design 01 §5.4).
    Envoy,
    /// Resonate counts (design 01 §5.5).
    Durable,
}

impl SourceKind {
    /// The name on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            SourceKind::Cgroup => "cgroup",
            SourceKind::Proc => "proc",
            SourceKind::Wasm => "wasm",
            SourceKind::Envoy => "envoy",
            SourceKind::Durable => "durable",
        }
    }
}

/// The runtime tier (design §24 §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// workerd, one process per tenant.
    T0,
    /// wasmtime components.
    T1,
    /// gVisor sandboxes.
    T2,
    /// Firecracker microVMs (deferred).
    T3,
}

/// Who a measurement is billed to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Attribution {
    /// The billing unit (D65).
    pub org: String,
    /// The namespace inside the org.
    pub namespace: String,
    /// The function, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    /// The tier, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<Tier>,
}

impl Attribution {
    /// A tenant with no function or tier.
    pub fn new(org: impl Into<String>, namespace: impl Into<String>) -> Self {
        Self {
            org: org.into(),
            namespace: namespace.into(),
            function: None,
            tier: None,
        }
    }

    /// Usage no tenant owns (design 01 §7.5).
    pub fn platform() -> Self {
        Self::new(PLATFORM_ORG, PLATFORM_ORG)
    }

    /// The CloudEvents `subject`: `<org>/<namespace>`.
    pub fn subject(&self) -> String {
        format!("{}/{}", self.org, self.namespace)
    }
}

/// The event's `data` (design 01 §4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeterData {
    /// The org billed.
    pub org: String,
    /// The namespace billed.
    pub namespace: String,
    /// What was measured.
    pub meter: Meter,
    /// The delta over the window: never negative, never a running total.
    pub value: u64,
    /// The window's start (the previous reading).
    #[serde(with = "time::serde::rfc3339")]
    pub window_start: OffsetDateTime,
    /// The window's end (this reading).
    #[serde(with = "time::serde::rfc3339")]
    pub window_end: OffsetDateTime,
    /// The tier, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<Tier>,
    /// The function, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    /// What was measured, for audits (a cgroup with its inode, a pid, …).
    pub resource: String,
    /// The reading at `window_end`: the cumulative value for a counter, the
    /// instantaneous value for a gauge. The agent rebuilds its baselines from
    /// it after a crash (design 01 §6.2); the aggregator ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counter: Option<u64>,
    /// The node that measured it.
    pub node: String,
    /// The agent's sequence number, monotonic per node.
    pub seq: u64,
}

/// A meter event: a CloudEvents 1.0 envelope around [`MeterData`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeterEvent {
    /// Always [`SPEC_VERSION`].
    pub specversion: String,
    /// The idempotency key, `<node>/<source>/<seq>`.
    pub id: String,
    /// `loams-meter://<node>/<source>`.
    pub source: String,
    /// Always [`EVENT_TYPE`].
    #[serde(rename = "type")]
    pub event_type: String,
    /// `<org>/<namespace>`: the billed customer.
    pub subject: String,
    /// The window's end.
    #[serde(with = "time::serde::rfc3339")]
    pub time: OffsetDateTime,
    /// Always `application/json`.
    pub datacontenttype: String,
    /// The measurement.
    pub data: MeterData,
}

/// A measurement before the agent gives it a sequence number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Measurement {
    /// Who is billed.
    pub attribution: Attribution,
    /// Where it was read.
    pub source: SourceKind,
    /// What was measured.
    pub meter: Meter,
    /// The delta.
    pub value: u64,
    /// The window's start.
    pub window_start: OffsetDateTime,
    /// The window's end.
    pub window_end: OffsetDateTime,
    /// The resource measured.
    pub resource: String,
    /// The reading at `window_end` (see [`MeterData::counter`]).
    pub counter: Option<u64>,
}

impl Measurement {
    /// The event for this measurement, taken on `node` as number `seq`.
    pub fn into_event(self, node: &str, seq: u64) -> MeterEvent {
        let source = self.source.as_str();
        MeterEvent {
            specversion: SPEC_VERSION.to_string(),
            id: idempotency_key(node, self.source, seq),
            source: format!("loams-meter://{node}/{source}"),
            event_type: EVENT_TYPE.to_string(),
            subject: self.attribution.subject(),
            time: self.window_end,
            datacontenttype: "application/json".to_string(),
            data: MeterData {
                org: self.attribution.org,
                namespace: self.attribution.namespace,
                meter: self.meter,
                value: self.value,
                window_start: self.window_start,
                window_end: self.window_end,
                tier: self.attribution.tier,
                function: self.attribution.function,
                resource: self.resource,
                counter: self.counter,
                node: node.to_string(),
                seq,
            },
        }
    }
}

/// The idempotency key of event `seq` from `source` on `node`.
pub fn idempotency_key(node: &str, source: SourceKind, seq: u64) -> String {
    format!("{node}/{}/{seq}", source.as_str())
}

/// Milliseconds since the Unix epoch as a UTC time. Out-of-range values
/// clamp to the epoch.
pub fn time_from_unix_ms(ms: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measurement() -> Measurement {
        Measurement {
            attribution: Attribution {
                org: "org_a".into(),
                namespace: "prod".into(),
                function: Some("chat".into()),
                tier: Some(Tier::T0),
            },
            source: SourceKind::Cgroup,
            meter: Meter::CpuUsec,
            value: 1834,
            window_start: time_from_unix_ms(1_790_000_000_000),
            window_end: time_from_unix_ms(1_790_000_001_000),
            resource: "loams-org_a.scope#ino=7".into(),
            counter: Some(99_183_422),
        }
    }

    #[test]
    fn event_is_a_cloudevent_with_the_idempotency_key() {
        let event = measurement().into_event("node-7", 42);
        assert_eq!(event.id, "node-7/cgroup/42");
        assert_eq!(event.source, "loams-meter://node-7/cgroup");
        assert_eq!(event.subject, "org_a/prod");
        assert_eq!(event.time, event.data.window_end);

        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["specversion"], "1.0");
        assert_eq!(json["type"], EVENT_TYPE);
        assert_eq!(json["data"]["meter"], "cpu_usec");
        assert_eq!(json["data"]["tier"], "t0");
        assert_eq!(json["data"]["window_end"], "2026-09-21T14:13:21Z");
        assert_eq!(json["data"]["seq"], 42);
    }

    #[test]
    fn event_round_trips_through_json() {
        let mut m = measurement();
        m.attribution.function = None;
        m.attribution.tier = None;
        m.counter = None;
        let event = m.into_event("n", 1);
        let json = serde_json::to_string(&event).unwrap();
        assert!(!json.contains("\"tier\""));
        assert!(!json.contains("\"counter\""));
        let back: MeterEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, event);
    }

    #[test]
    fn meter_names_match_serde() {
        for meter in [
            Meter::CpuUsec,
            Meter::MemoryByteSeconds,
            Meter::MemoryPeakBytes,
            Meter::WasmFuel,
            Meter::Requests,
            Meter::IngressBytes,
            Meter::EgressBytes,
            Meter::DurablePromises,
            Meter::DurableTimers,
        ] {
            let json = serde_json::to_value(meter).unwrap();
            assert_eq!(json, meter.as_str());
        }
    }
}
