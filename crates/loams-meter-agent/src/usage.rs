//! The agent's events on the wire: usage events in the Loams usage profile
//! (design §63 §6, D1121) and their stream records (D270's binary-mode
//! layout, §63 §21).
//!
//! The spool keeps the agent's own [`MeterEvent`] records, which carry the
//! counter readings and sequence numbers crash recovery needs. This module
//! turns each one into the CloudEvent the `_system/_meter` stream gets:
//!
//! | Meter | `type` | `data` |
//! |---|---|---|
//! | [`Meter::CpuUsec`] | `io.loams.dev.usage.node.cpu.v1` | `cpu_usec` |
//! | [`Meter::MemoryByteSeconds`] | `io.loams.dev.usage.node.memory.v1` | `byte_seconds`, `peak_bytes` |
//! | [`Meter::IngressBytes`], [`Meter::EgressBytes`] | `io.loams.dev.usage.node.network.v1` | `ingress_bytes`, `egress_bytes` |
//! | [`Meter::WasmFuel`] | `io.loams.dev.usage.function.fuel.v1` | `fuel`, `cpu_usec`, `estimated` |
//!
//! plus the dimensions `resource`, `node`, `function`, `tier`,
//! `window_start` and `window_end` (strings). `id` keeps the agent's key
//! `<node>/<source>/<seq>`; `source` is `loams://<cluster>/meter-agent/<node>`;
//! `subject` is the billing subject ([`billing_subject`]) and `tenantid` is
//! `<org>/<namespace>`. Records are keyed by `subject`, so the stream's
//! default partitioner (Kafka's murmur2) puts every event of a subject in the
//! partition its counter owner holds (§63 §11.1).

use bytes::Bytes;
use loams_cloudevents::partition::kafka_partition;
use loams_cloudevents::{Attr, CloudEvent};
use loams_log::Record;
use serde_json::{Map, Value, json};
use time::format_description::BorrowedFormatItem;
use time::macros::format_description;
use time::{OffsetDateTime, UtcOffset};

use crate::error::MeterError;
use crate::event::{Meter, MeterEvent, PLATFORM_ORG, Tier};

/// The prefix of every Loams usage type (§63 §6.1).
pub const USAGE_TYPE_PREFIX: &str = "io.loams.dev.usage.";
/// CPU time of a node's metered cgroups.
pub const NODE_CPU_TYPE: &str = "io.loams.dev.usage.node.cpu.v1";
/// Resident memory of a node's metered cgroups.
pub const NODE_MEMORY_TYPE: &str = "io.loams.dev.usage.node.memory.v1";
/// Network bytes of a node's metered resources.
pub const NODE_NETWORK_TYPE: &str = "io.loams.dev.usage.node.network.v1";
/// wasmtime fuel per invocation (the T1 host report, platform doc 06).
pub const FUNCTION_FUEL_TYPE: &str = "io.loams.dev.usage.function.fuel.v1";
/// The `datacontenttype` of every usage event (§63 §6.4).
pub const DATA_CONTENT_TYPE: &str = "application/json";
/// The extension naming the tenant (D364).
pub const TENANT_EXTENSION: &str = "tenantid";
/// The role named in `source`.
pub const SOURCE_ROLE: &str = "meter-agent";
/// The cluster name used when none is configured.
pub const DEFAULT_CLUSTER: &str = "local";

/// `time` and the window bounds: RFC 3339 in UTC with nanoseconds (D364).
const RFC3339_NANOS: &[BorrowedFormatItem<'static>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:9]Z");

/// The usage type of `meter`. Every meter the agent records has one.
pub fn usage_type(meter: Meter) -> &'static str {
    match meter {
        Meter::CpuUsec => NODE_CPU_TYPE,
        Meter::MemoryByteSeconds => NODE_MEMORY_TYPE,
        Meter::IngressBytes | Meter::EgressBytes => NODE_NETWORK_TYPE,
        Meter::WasmFuel => FUNCTION_FUEL_TYPE,
    }
}

/// The `dataschema` of a Loams usage type: `urn:loams:json:<type>`.
pub fn dataschema(ty: &str) -> String {
    format!("urn:loams:json:{ty}")
}

/// The billing subject of `org` (§63 §6.1: `org_<id>`, the org being the
/// billing unit, D65). An org key that already carries the `org_` prefix is
/// kept as is; unattributed usage stays under [`PLATFORM_ORG`], which is
/// metered and never billed.
pub fn billing_subject(org: &str) -> String {
    if org == PLATFORM_ORG || org.starts_with("org_") {
        org.to_string()
    } else {
        format!("org_{org}")
    }
}

/// The `source` of every event the agent on `node` sends from `cluster`.
pub fn source(cluster: &str, node: &str) -> String {
    format!("loams://{cluster}/{SOURCE_ROLE}/{node}")
}

/// Formats `at` as `time` is written.
pub fn format_time(at: OffsetDateTime) -> String {
    at.to_offset(UtcOffset::UTC)
        .format(RFC3339_NANOS)
        .unwrap_or_else(|_| "1970-01-01T00:00:00.000000000Z".to_string())
}

fn tier_name(tier: Tier) -> &'static str {
    match tier {
        Tier::T0 => "t0",
        Tier::T1 => "t1",
        Tier::T2 => "t2",
        Tier::T3 => "t3",
    }
}

/// The JSON `data` of `event`: the measured quantities as integers, the
/// dimensions as strings.
pub fn usage_data(event: &MeterEvent) -> Map<String, Value> {
    let d = &event.data;
    let mut data = Map::new();
    match d.meter {
        Meter::CpuUsec => {
            data.insert("cpu_usec".into(), json!(d.value));
            // A T1 host's CPU split by apportionment rather than by its
            // reports (crate::host::reconcile).
            if d.estimated {
                data.insert("estimated".into(), json!(true));
            }
        }
        Meter::MemoryByteSeconds => {
            data.insert("byte_seconds".into(), json!(d.value));
            data.insert("peak_bytes".into(), json!(d.peak_bytes.unwrap_or(0)));
        }
        Meter::IngressBytes => {
            data.insert("ingress_bytes".into(), json!(d.value));
            data.insert("egress_bytes".into(), json!(0));
        }
        Meter::EgressBytes => {
            data.insert("ingress_bytes".into(), json!(0));
            data.insert("egress_bytes".into(), json!(d.value));
        }
        Meter::WasmFuel => {
            data.insert("fuel".into(), json!(d.value));
            data.insert("cpu_usec".into(), json!(d.cpu_usec.unwrap_or(0)));
            data.insert("estimated".into(), json!(d.estimated));
        }
    }
    data.insert("namespace".into(), json!(d.namespace));
    data.insert("resource".into(), json!(d.resource));
    data.insert("node".into(), json!(d.node));
    if let Some(function) = &d.function {
        data.insert("function".into(), json!(function));
    }
    if let Some(tier) = d.tier {
        data.insert("tier".into(), json!(tier_name(tier)));
    }
    data.insert("window_start".into(), json!(format_time(d.window_start)));
    data.insert("window_end".into(), json!(format_time(d.window_end)));
    data
}

/// The usage event for the spooled `event`, sent from `cluster`.
///
/// Spooled records written before the §63 §6 change (type
/// `dev.loams.meter.v1`, subject `<org>/<namespace>`) convert the same way:
/// the conversion reads only `data`, and `id` is kept, so a record spooled
/// before an upgrade is still sent exactly once under its id.
pub fn to_cloudevent(event: &MeterEvent, cluster: &str) -> Result<CloudEvent, MeterError> {
    let d = &event.data;
    let ty = usage_type(d.meter);
    let data = serde_json::to_vec(&Value::Object(usage_data(event)))
        .map_err(|e| MeterError::SinkRejected(format!("event {}: {e}", event.id)))?;
    let attrs = vec![
        Attr::string("specversion", loams_cloudevents::SPEC_VERSION),
        Attr::string("id", event.id.clone()),
        Attr::string("source", source(cluster, &d.node)),
        Attr::string("type", ty),
        Attr::string("subject", billing_subject(&d.org)),
        Attr::string("time", format_time(d.window_end)),
        Attr::string("datacontenttype", DATA_CONTENT_TYPE),
        Attr::string("dataschema", dataschema(ty)),
        Attr::string(TENANT_EXTENSION, format!("{}/{}", d.org, d.namespace)),
    ];
    CloudEvent::new(attrs, Some(Bytes::from(data)))
        .map_err(|e| MeterError::SinkRejected(format!("event {}: {e}", event.id)))
}

/// The stream record of `event`: D270's binary-mode layout (`ce_` headers,
/// `content-type`, key = `subject`, the JSON data as the value).
pub fn to_record(event: &CloudEvent) -> Record {
    loams_cloudevents::record::to_record(event)
}

/// The partition of `event` among `partitions`: the stream's default
/// partitioner over the record key (`subject`), which is how the meter role
/// assigns a subject to its counter owner.
pub fn partition(event: &CloudEvent, partitions: u32) -> u32 {
    loams_cloudevents::partition::partition_of(event, partitions)
}

/// The partition of `subject`'s events among `partitions`.
pub fn subject_partition(subject: &str, partitions: u32) -> u32 {
    kafka_partition(subject.as_bytes(), partitions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Attribution, EVENT_TYPE, Measurement, SourceKind, time_from_unix_ms};

    fn event(meter: Meter, org: &str) -> MeterEvent {
        Measurement {
            attribution: Attribution {
                org: org.into(),
                namespace: "prod".into(),
                function: Some("chat".into()),
                tier: Some(Tier::T0),
            },
            source: SourceKind::Cgroup,
            meter,
            value: 1834,
            window_start: time_from_unix_ms(1_790_000_000_000),
            window_end: time_from_unix_ms(1_790_000_001_000),
            resource: "loams-org_a.scope#ino=7".into(),
            counter: Some(99_183_422),
            peak_bytes: Some(4096),
            cpu_usec: None,
            estimated: false,
        }
        .into_event("node-7", 42)
    }

    #[test]
    fn types_follow_the_usage_profile() {
        for meter in [
            Meter::CpuUsec,
            Meter::MemoryByteSeconds,
            Meter::IngressBytes,
            Meter::EgressBytes,
            Meter::WasmFuel,
        ] {
            let ty = usage_type(meter);
            assert!(ty.starts_with(USAGE_TYPE_PREFIX), "{ty}");
            assert!(ty.ends_with(".v1"), "{ty}");
        }
    }

    #[test]
    fn subject_is_the_billing_subject() {
        assert_eq!(billing_subject("org_a"), "org_a");
        assert_eq!(billing_subject("42"), "org_42");
        assert_eq!(billing_subject(PLATFORM_ORG), PLATFORM_ORG);
    }

    #[test]
    fn cloudevent_carries_section_6_attributes() {
        let ce = to_cloudevent(&event(Meter::CpuUsec, "org_a"), "eu1").unwrap();
        assert_eq!(ce.id(), "node-7/cgroup/42");
        assert_eq!(ce.source(), "loams://eu1/meter-agent/node-7");
        assert_eq!(ce.ty(), NODE_CPU_TYPE);
        assert_eq!(ce.attr("subject"), Some("org_a"));
        assert_eq!(ce.attr("tenantid"), Some("org_a/prod"));
        assert_eq!(ce.attr("time"), Some("2026-09-21T14:13:21.000000000Z"));
        assert_eq!(
            ce.attr("dataschema"),
            Some("urn:loams:json:io.loams.dev.usage.node.cpu.v1")
        );
        let data: Value = serde_json::from_slice(ce.data().unwrap()).unwrap();
        assert_eq!(data["cpu_usec"], 1834);
        assert_eq!(data["tier"], "t0");
        assert_eq!(data["function"], "chat");
        assert_eq!(data["window_start"], "2026-09-21T14:13:20.000000000Z");
    }

    #[test]
    fn memory_event_carries_byte_seconds_and_peak() {
        let ce = to_cloudevent(&event(Meter::MemoryByteSeconds, "org_a"), "eu1").unwrap();
        assert_eq!(ce.ty(), NODE_MEMORY_TYPE);
        let data: Value = serde_json::from_slice(ce.data().unwrap()).unwrap();
        assert_eq!(data["byte_seconds"], 1834);
        assert_eq!(data["peak_bytes"], 4096);
    }

    #[test]
    fn legacy_spooled_record_converts_under_its_id() {
        let mut legacy = event(Meter::CpuUsec, "org_a");
        legacy.event_type = EVENT_TYPE.to_string();
        legacy.subject = "org_a/prod".to_string();
        legacy.source = "loams-meter://node-7/cgroup".to_string();
        let ce = to_cloudevent(&legacy, "eu1").unwrap();
        assert_eq!(ce.id(), "node-7/cgroup/42");
        assert_eq!(ce.ty(), NODE_CPU_TYPE);
        assert_eq!(ce.attr("subject"), Some("org_a"));
    }
}
