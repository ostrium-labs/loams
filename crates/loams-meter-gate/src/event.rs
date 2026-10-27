//! The usage event (design §63 §6, D1121).
//!
//! A [`UsageEvent`] is built by a producer, checked against the Loams usage
//! profile ([`UsageEvent::validate`]) and encoded once ([`UsageEvent::encode`])
//! as a structured-mode CloudEvent 1.0 with JSON data. The encoded bytes are
//! what the sink queues, retries and spools, so a retry carries the same `id`
//! (dedup layer 1, §63 §7.3).
//!
//! The record layout of the stream (D270's `ce_` headers) is the producer's
//! job ([`crate::sink::Producer`]); this crate links no `loams-log`.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};

/// The `type` prefix of Loams' usage events (§63 §6.3).
pub const USAGE_TYPE_PREFIX: &str = "io.loams.dev.usage.";
/// The `type` prefix of the meter's own threshold and lifecycle events (§63 §15).
pub const METER_TYPE_PREFIX: &str = "io.loams.dev.meter.";
/// `datacontenttype` of every usage event (§63 §6.4).
pub const DATA_CONTENT_TYPE: &str = "application/json";

/// Loams' event types (§63 §6.3), as constants so producers cannot misspell them.
pub mod types {
    pub const HOUSE_QUERY: &str = "io.loams.dev.usage.house.query.v1";
    pub const HOUSE_INSERT: &str = "io.loams.dev.usage.house.insert.v1";
    pub const HOUSE_STORAGE: &str = "io.loams.dev.usage.house.storage.v1";
    pub const HOUSE_COMPACTION: &str = "io.loams.dev.usage.house.compaction.v1";
    pub const POSTGRES_COMPUTE: &str = "io.loams.dev.usage.postgres.compute.v1";
    pub const POSTGRES_STORAGE: &str = "io.loams.dev.usage.postgres.storage.v1";
    pub const GATEWAY_CONNECTION: &str = "io.loams.dev.usage.gateway.connection.v1";
    pub const GATEWAY_REQUEST: &str = "io.loams.dev.usage.gateway.request.v1";
    pub const LLM_TOKENS: &str = "io.loams.dev.usage.llm.tokens.v1";
    pub const MCP_CALL: &str = "io.loams.dev.usage.mcp.call.v1";
    pub const FUNCTION_INVOCATION: &str = "io.loams.dev.usage.function.invocation.v1";
    pub const JOBS_RUN: &str = "io.loams.dev.usage.jobs.run.v1";
    pub const STREAM_INGEST: &str = "io.loams.dev.usage.stream.ingest.v1";
    pub const STORAGE_LOGICAL: &str = "io.loams.dev.usage.storage.logical.v1";
    pub const GIT_STORAGE: &str = "io.loams.dev.usage.git.storage.v1";
    pub const GIT_TRANSFER: &str = "io.loams.dev.usage.git.transfer.v1";
    pub const SANDBOX_SESSION: &str = "io.loams.dev.usage.sandbox.session.v1";
}

/// One usage event before it is encoded.
#[derive(Clone, Debug, PartialEq)]
pub struct UsageEvent {
    /// Derived from the work, never random (§63 §6.1).
    pub id: String,
    /// `loams://<cluster>/<role>/<node>` for Loams' producers.
    pub source: String,
    /// `io.loams.dev.usage.<product>.<name>.v<major>`.
    pub r#type: String,
    /// The billing subject (`org_<id>`), from the credential.
    pub subject: String,
    /// The end of the measured interval, ns since the Unix epoch, UTC.
    pub time_ns: i64,
    /// `<org>/<namespace>`, from the credential.
    pub tenant: String,
    /// The lease that admitted the work (`loamslease`, §63 §11.3).
    pub lease: Option<String>,
    /// Measured values (numbers) and dimensions (strings).
    pub data: Map<String, Value>,
}

/// Why an event is not in the usage profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventError {
    /// A required attribute is empty: names it.
    Missing(&'static str),
    /// `type` is not a Loams usage or meter type.
    BadType(String),
}

impl fmt::Display for EventError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(attr) => write!(f, "usage event without `{attr}`"),
            Self::BadType(t) => write!(f, "`{t}` is not a Loams usage type"),
        }
    }
}

impl std::error::Error for EventError {}

impl UsageEvent {
    /// An event of `type` with `id`; the rest is set with the builder methods.
    pub fn new(r#type: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            source: String::new(),
            r#type: r#type.into(),
            subject: String::new(),
            time_ns: 0,
            tenant: String::new(),
            lease: None,
            data: Map::new(),
        }
    }

    pub fn source(mut self, source: impl Into<String>) -> Self {
        self.source = source.into();
        self
    }

    pub fn subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = subject.into();
        self
    }

    pub fn tenant(mut self, tenant: impl Into<String>) -> Self {
        self.tenant = tenant.into();
        self
    }

    pub fn at(mut self, at: SystemTime) -> Self {
        self.time_ns = system_time_ns(at);
        self
    }

    pub fn at_ns(mut self, ns: i64) -> Self {
        self.time_ns = ns;
        self
    }

    pub fn lease(mut self, lease: impl Into<String>) -> Self {
        self.lease = Some(lease.into());
        self
    }

    /// A measured value (bytes, milliseconds, tokens …).
    pub fn num(mut self, key: &str, value: u64) -> Self {
        self.data.insert(key.to_owned(), Value::from(value));
        self
    }

    /// A signed measured value (a ClickHouse code, say).
    pub fn int(mut self, key: &str, value: i64) -> Self {
        self.data.insert(key.to_owned(), Value::from(value));
        self
    }

    /// A dimension.
    pub fn text(mut self, key: &str, value: &str) -> Self {
        self.data.insert(key.to_owned(), Value::from(value));
        self
    }

    pub fn flag(mut self, key: &str, value: bool) -> Self {
        self.data.insert(key.to_owned(), Value::from(value));
        self
    }

    /// Checks the attributes the usage profile requires (§63 §6.1).
    pub fn validate(&self) -> Result<(), EventError> {
        let required = [
            ("id", &self.id),
            ("source", &self.source),
            ("type", &self.r#type),
            ("subject", &self.subject),
            ("tenantid", &self.tenant),
        ];
        for (name, value) in required {
            if value.is_empty() {
                return Err(EventError::Missing(name));
            }
        }
        if !(self.r#type.starts_with(USAGE_TYPE_PREFIX)
            || self.r#type.starts_with(METER_TYPE_PREFIX))
        {
            return Err(EventError::BadType(self.r#type.clone()));
        }
        Ok(())
    }

    /// The structured-mode CloudEvent (JSON format), attributes in the order
    /// of §63 §6.2. `loamslease` is written only when set.
    pub fn to_json(&self) -> Value {
        let mut ce = Map::new();
        ce.insert("specversion".into(), Value::from("1.0"));
        ce.insert("id".into(), Value::from(self.id.as_str()));
        ce.insert("source".into(), Value::from(self.source.as_str()));
        ce.insert("type".into(), Value::from(self.r#type.as_str()));
        ce.insert("subject".into(), Value::from(self.subject.as_str()));
        ce.insert("time".into(), Value::from(rfc3339_nanos(self.time_ns)));
        ce.insert("datacontenttype".into(), Value::from(DATA_CONTENT_TYPE));
        ce.insert(
            "dataschema".into(),
            Value::from(format!("urn:loams:json:{}", self.r#type)),
        );
        ce.insert("tenantid".into(), Value::from(self.tenant.as_str()));
        if let Some(lease) = &self.lease {
            ce.insert("loamslease".into(), Value::from(lease.as_str()));
        }
        ce.insert("data".into(), Value::Object(self.data.clone()));
        Value::Object(ce)
    }

    /// Encodes the event once; the sink queues, retries and spools these bytes.
    pub fn encode(&self) -> EncodedEvent {
        EncodedEvent {
            key: self.subject.clone().into_boxed_str(),
            payload: self.to_json().to_string().into_bytes(),
        }
    }
}

/// An event as the sink carries it: the partition key (`subject`, §63 §7.1)
/// and the structured-mode CloudEvent JSON.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedEvent {
    pub key: Box<str>,
    pub payload: Vec<u8>,
}

impl EncodedEvent {
    /// Bytes counted against the sink's limits.
    pub fn size(&self) -> usize {
        self.key.len() + self.payload.len()
    }
}

/// ns since the Unix epoch, negative before it, saturating at `i64`'s range.
pub fn system_time_ns(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_nanos()).unwrap_or(i64::MAX),
        Err(e) => i64::try_from(e.duration().as_nanos())
            .map(|n| -n)
            .unwrap_or(i64::MIN),
    }
}

/// RFC 3339 in UTC with nine fractional digits (D364).
pub fn rfc3339_nanos(ns: i64) -> String {
    const NS: i64 = 1_000_000_000;
    let secs = ns.div_euclid(NS);
    let frac = ns.rem_euclid(NS);
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{frac:09}Z",
        sod / 3600,
        (sod / 60) % 60,
        sod % 60
    )
}

/// (year, month, day) of a day count since 1970-01-01 (H. Hinnant's
/// `civil_from_days`, public domain).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> UsageEvent {
        UsageEvent::new(types::HOUSE_QUERY, "q-7f3a9c/1")
            .source("loams://prod-eu1/house/node-3")
            .subject("org_42")
            .tenant("42/7")
            .at_ns(1_792_401_243_218_734_000)
            .text("namespace", "7")
            .num("cpu_ms", 1840)
            .int("code", 0)
    }

    #[test]
    fn rfc3339_formats_nanoseconds() {
        assert_eq!(
            rfc3339_nanos(1_792_401_243_218_734_000),
            "2026-10-19T09:14:03.218734000Z"
        );
        assert_eq!(rfc3339_nanos(0), "1970-01-01T00:00:00.000000000Z");
        assert_eq!(
            rfc3339_nanos(951_868_799 * 1_000_000_000),
            "2000-02-29T23:59:59.000000000Z"
        );
        assert_eq!(
            rfc3339_nanos(-1_000_000_000),
            "1969-12-31T23:59:59.000000000Z"
        );
        assert_eq!(rfc3339_nanos(-1), "1969-12-31T23:59:59.999999999Z");
    }

    #[test]
    fn encoded_event_is_a_usage_profile_cloudevent() {
        let ev = sample().lease("l9k2m4q8x1");
        ev.validate().unwrap();
        let enc = ev.encode();
        assert_eq!(&*enc.key, "org_42");
        let v: Value = serde_json::from_slice(&enc.payload).unwrap();
        assert_eq!(v["specversion"], "1.0");
        assert_eq!(v["id"], "q-7f3a9c/1");
        assert_eq!(v["type"], types::HOUSE_QUERY);
        assert_eq!(v["subject"], "org_42");
        assert_eq!(v["time"], "2026-10-19T09:14:03.218734000Z");
        assert_eq!(v["datacontenttype"], "application/json");
        assert_eq!(
            v["dataschema"],
            "urn:loams:json:io.loams.dev.usage.house.query.v1"
        );
        assert_eq!(v["tenantid"], "42/7");
        assert_eq!(v["loamslease"], "l9k2m4q8x1");
        assert_eq!(v["data"]["cpu_ms"], 1840);
        assert_eq!(v["data"]["namespace"], "7");
    }

    #[test]
    fn lease_is_omitted_when_unset() {
        let v = sample().to_json();
        assert!(v.get("loamslease").is_none());
    }

    #[test]
    fn encoding_is_deterministic() {
        assert_eq!(sample().encode(), sample().encode());
    }

    #[test]
    fn event_without_subject_is_refused() {
        let ev = UsageEvent {
            subject: String::new(),
            ..sample()
        };
        assert_eq!(ev.validate(), Err(EventError::Missing("subject")));
    }

    #[test]
    fn non_loams_type_is_refused() {
        let ev = UsageEvent {
            r#type: "com.example.thing".into(),
            ..sample()
        };
        assert!(matches!(ev.validate(), Err(EventError::BadType(_))));
    }
}
