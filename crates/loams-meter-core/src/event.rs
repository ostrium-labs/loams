//! The usage event and the Loams usage profile (design §63 §6, D1121, D1133).
//!
//! A usage event is a CloudEvent 1.0 in the Loams profile (D364) that
//! OpenMeter's `POST /api/v1/events` accepts unchanged: `subject` is
//! required, `data` is a JSON object, `type` is
//! `io.loams.dev.usage.<product>.<name>.v<major>` for Loams' meters, and the
//! one Loams extension is `loamslease`. [`Event::from_cloudevent`] applies
//! the profile on top of `loams-cloudevents`' own CloudEvents validation.

use bytes::Bytes;
use loams_cloudevents::{Attr, CloudEvent, DataKind};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::ids::{IdError, LeaseId, SubjectKey, TenantId, UtcNanos};

/// Every type under this prefix is Loams' own and is refused from tenant
/// credentials (D1133).
pub const RESERVED_TYPE_PREFIX: &str = "io.loams.dev.";
/// Loams' usage types: `io.loams.dev.usage.<product>.<name>.v<major>`.
pub const USAGE_TYPE_PREFIX: &str = "io.loams.dev.usage.";
/// Threshold and lifecycle events (§15); excluded from every meter.
pub const METER_EVENT_TYPE_PREFIX: &str = "io.loams.dev.meter.";
/// The lease that admitted the work (§6.1, §11.3).
pub const LEASE_EXTENSION: &str = "loamslease";
/// `<org>/<namespace>` from the credential (D364).
pub const TENANT_EXTENSION: &str = "tenantid";
/// The only data content type of a usage event.
pub const JSON_CONTENT_TYPE: &str = "application/json";
/// The `dataschema` prefix of Loams' types; the schema is
/// `docs/api/meter/<type>.json`.
pub const DATASCHEMA_PREFIX: &str = "urn:loams:json:";
/// How far in the past an external event's `time` may be (Stripe's window).
pub const MAX_PAST: i64 = 35 * UtcNanos::DAY;
/// How far in the future an external event's `time` may be.
pub const MAX_FUTURE: i64 = 5 * UtcNanos::MINUTE;

/// Who sent an event, from the credential that sent it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// A Loams service identity (a product role, the node agent, the meter
    /// role itself). May send reserved types.
    Service,
    /// A namespace credential (user meters, partner systems). May not send
    /// reserved types (D1133).
    Tenant,
}

/// Why an event does not fit the usage profile.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EventError {
    #[error("usage events require 'subject'")]
    MissingSubject,
    #[error("usage events require 'time'")]
    MissingTime,
    #[error("'time' is not an RFC 3339 timestamp: '{0}'")]
    BadTime(String),
    #[error("'time' {0} is outside the accepted window (35 days past, 5 minutes future)")]
    TimeOutOfWindow(String),
    #[error("usage event data must be JSON ('application/json'), got {0:?}")]
    NotJsonData(Option<String>),
    #[error("usage event data must be a JSON object")]
    DataNotObject,
    #[error("type '{0}' is reserved for Loams service identities")]
    ReservedType(String),
    #[error("type '{0}' is not 'io.loams.dev.usage.<product>.<name>.v<major>'")]
    BadType(String),
    #[error("Loams types require 'tenantid'")]
    MissingTenant,
    #[error("'dataschema' must be '{expected}', got {got:?}")]
    Dataschema {
        expected: String,
        got: Option<String>,
    },
    #[error("invalid identifier: {0}")]
    Id(#[from] IdError),
    #[error("invalid CloudEvent: {0}")]
    CloudEvent(#[from] loams_cloudevents::Error),
}

/// True for any type under `io.loams.dev.`.
pub fn is_reserved_type(ty: &str) -> bool {
    ty.starts_with(RESERVED_TYPE_PREFIX)
}

/// True for a well-formed `io.loams.dev.usage.<product>.<name…>.v<major>`:
/// at least a product and a name, each segment lower-case letters, digits
/// and `_`, and a final `v<digits>` with no leading zero.
pub fn is_loams_usage_type(ty: &str) -> bool {
    let Some(rest) = ty.strip_prefix(USAGE_TYPE_PREFIX) else {
        return false;
    };
    let segments: Vec<&str> = rest.split('.').collect();
    let Some((version, names)) = segments.split_last() else {
        return false;
    };
    let version_ok = version.strip_prefix('v').is_some_and(|d| {
        !d.is_empty() && !d.starts_with('0') && d.bytes().all(|b| b.is_ascii_digit())
    });
    version_ok
        && names.len() >= 2
        && names.iter().all(|s| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        })
}

/// The `dataschema` a Loams type must carry.
pub fn dataschema_for(ty: &str) -> String {
    format!("{DATASCHEMA_PREFIX}{ty}")
}

/// One usage event as the core sees it, after the profile is applied.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub id: String,
    pub source: String,
    pub r#type: String,
    pub subject: SubjectKey,
    pub time: UtcNanos,
    /// `tenantid`; required on Loams' types, optional on user meters' events
    /// until the ingest sets it from the credential.
    pub tenant: Option<TenantId>,
    pub lease: Option<LeaseId>,
    pub data: Map<String, Value>,
}

impl Event {
    /// Applies the usage profile to a CloudEvent already valid under
    /// CloudEvents 1.0 (which `loams-cloudevents` checked when it built it).
    pub fn from_cloudevent(ce: &CloudEvent, origin: Origin) -> Result<Event, EventError> {
        let ty = ce.ty();
        if is_reserved_type(ty) && origin == Origin::Tenant {
            return Err(EventError::ReservedType(ty.to_owned()));
        }
        let loams_type = is_reserved_type(ty);
        if ty.starts_with(USAGE_TYPE_PREFIX) && !is_loams_usage_type(ty) {
            return Err(EventError::BadType(ty.to_owned()));
        }

        let subject = match ce.attr("subject") {
            Some(s) if !s.is_empty() => SubjectKey::new(s)?,
            _ => return Err(EventError::MissingSubject),
        };
        let time_text = ce.attr("time").ok_or(EventError::MissingTime)?;
        let time = UtcNanos::from_rfc3339(time_text)
            .map_err(|_| EventError::BadTime(time_text.to_owned()))?;

        let content_type = ce.datacontenttype();
        let json_type = content_type
            .map(loams_cloudevents::media_type)
            .is_some_and(|m| m == JSON_CONTENT_TYPE);
        // OpenMeter accepts only `application/json`; Loams' own types must
        // say so, an external event may leave it out (JSON is the default).
        if !(json_type || (content_type.is_none() && !loams_type)) {
            return Err(EventError::NotJsonData(content_type.map(str::to_owned)));
        }
        let data = match (ce.data(), ce.data_kind()) {
            (Some(bytes), DataKind::Json) => match serde_json::from_slice::<Value>(bytes) {
                Ok(Value::Object(map)) => map,
                Ok(_) => return Err(EventError::DataNotObject),
                Err(_) => return Err(EventError::NotJsonData(content_type.map(str::to_owned))),
            },
            (None, _) => return Err(EventError::DataNotObject),
            _ => return Err(EventError::NotJsonData(content_type.map(str::to_owned))),
        };

        let tenant = match ce.attr(TENANT_EXTENSION) {
            Some(t) => Some(TenantId::new(t)?),
            None if loams_type => return Err(EventError::MissingTenant),
            None => None,
        };
        if loams_type {
            let expected = dataschema_for(ty);
            let got = ce.attr("dataschema");
            if got != Some(expected.as_str()) {
                return Err(EventError::Dataschema {
                    expected,
                    got: got.map(str::to_owned),
                });
            }
        }
        let lease = ce.attr(LEASE_EXTENSION).map(LeaseId::new).transpose()?;

        Ok(Event {
            id: ce.id().to_owned(),
            source: ce.source().to_owned(),
            r#type: ty.to_owned(),
            subject,
            time,
            tenant,
            lease,
            data,
        })
    }

    /// The dedup key: SHA-256(`source` ‖ 0x00 ‖ `id`), as
    /// `loams_cloudevents::CloudEvent::dedup_key`.
    pub fn dedup_key(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(self.source.as_bytes());
        hash.update([0u8]);
        hash.update(self.id.as_bytes());
        hash.finalize().into()
    }

    /// True when the type is a Loams usage type a meter may select;
    /// `io.loams.dev.meter.*` notifications never are.
    pub fn is_usage(&self) -> bool {
        !self.r#type.starts_with(METER_EVENT_TYPE_PREFIX)
    }

    /// Refuses an external event whose `time` is outside
    /// [now − 35 days, now + 5 minutes] (§6.1, §14).
    pub fn check_time_window(&self, now: UtcNanos) -> Result<(), EventError> {
        let earliest = now.add_nanos(-MAX_PAST);
        let latest = now.add_nanos(MAX_FUTURE);
        if self.time < earliest || self.time > latest {
            return Err(EventError::TimeOutOfWindow(self.time.to_rfc3339()));
        }
        Ok(())
    }

    /// The event's attributes in the Loams profile's order.
    fn attrs(&self, with_lease: bool) -> Vec<(&'static str, String)> {
        let mut attrs = vec![
            ("specversion", loams_cloudevents::SPEC_VERSION.to_owned()),
            ("id", self.id.clone()),
            ("source", self.source.clone()),
            ("type", self.r#type.clone()),
            ("subject", self.subject.as_str().to_owned()),
            ("time", self.time.to_rfc3339()),
            ("datacontenttype", JSON_CONTENT_TYPE.to_owned()),
        ];
        if is_reserved_type(&self.r#type) {
            attrs.push(("dataschema", dataschema_for(&self.r#type)));
        }
        if let Some(t) = &self.tenant {
            attrs.push((TENANT_EXTENSION, t.as_str().to_owned()));
        }
        if with_lease && let Some(l) = &self.lease {
            attrs.push((LEASE_EXTENSION, l.as_str().to_owned()));
        }
        attrs
    }

    /// The event as a CloudEvent with JSON data, for `MeterSink`.
    pub fn to_cloudevent(&self) -> Result<CloudEvent, EventError> {
        let attrs = self
            .attrs(true)
            .into_iter()
            .map(|(name, value)| Attr::string(name, value))
            .collect();
        let data = serde_json::to_vec(&self.data).map_err(|_| EventError::DataNotObject)?;
        Ok(CloudEvent::new(attrs, Some(Bytes::from(data)))?)
    }

    /// The event in OpenMeter's ingest shape (structured JSON), with
    /// `loamslease` stripped (§16).
    pub fn to_openmeter_json(&self) -> Value {
        let mut obj: Map<String, Value> = self
            .attrs(false)
            .into_iter()
            .map(|(name, value)| (name.to_owned(), Value::String(value)))
            .collect();
        obj.insert("data".to_owned(), Value::Object(self.data.clone()));
        Value::Object(obj)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loams_cloudevents::json::parse_event;

    const HOUSE_QUERY: &str = r#"{
      "specversion": "1.0",
      "id": "q-7f3a9c/1",
      "source": "loams://prod-eu1/house/node-3",
      "type": "io.loams.dev.usage.house.query.v1",
      "subject": "org_42",
      "time": "2026-10-19T09:14:03.218734Z",
      "datacontenttype": "application/json",
      "dataschema": "urn:loams:json:io.loams.dev.usage.house.query.v1",
      "tenantid": "42/7",
      "loamslease": "l9k2m4q8x1",
      "data": {"namespace": "7", "cpu_ms": 1840, "bytes_scanned": 734003200,
               "rows_read": 1200000, "result_bytes": 52100, "worker_ms": 2210,
               "statements": 1, "code": 0}
    }"#;

    fn ce(json: &str) -> CloudEvent {
        parse_event(json.as_bytes()).expect("valid CloudEvent")
    }

    fn without(json: &str, member: &str) -> String {
        let mut v: Value = serde_json::from_str(json).expect("json");
        v.as_object_mut().expect("object").remove(member);
        v.to_string()
    }

    fn with(json: &str, member: &str, value: Value) -> String {
        let mut v: Value = serde_json::from_str(json).expect("json");
        v.as_object_mut()
            .expect("object")
            .insert(member.to_owned(), value);
        v.to_string()
    }

    #[test]
    fn design_example_passes_the_profile() {
        let e = Event::from_cloudevent(&ce(HOUSE_QUERY), Origin::Service).expect("profile");
        assert_eq!(e.subject.as_str(), "org_42");
        assert_eq!(e.tenant.as_ref().map(TenantId::as_str), Some("42/7"));
        assert_eq!(e.lease.as_ref().map(LeaseId::as_str), Some("l9k2m4q8x1"));
        assert_eq!(e.data.get("cpu_ms"), Some(&Value::from(1840)));
        assert_eq!(e.dedup_key(), ce(HOUSE_QUERY).dedup_key());
    }

    #[test]
    fn usage_event_requires_subject() {
        let no_subject = without(HOUSE_QUERY, "subject");
        assert_eq!(
            Event::from_cloudevent(&ce(&no_subject), Origin::Service),
            Err(EventError::MissingSubject)
        );
        let empty = with(HOUSE_QUERY, "subject", Value::from(""));
        // An empty `subject` is refused by CloudEvents itself or by the profile.
        if let Ok(event) = parse_event(empty.as_bytes()) {
            assert_eq!(
                Event::from_cloudevent(&event, Origin::Service),
                Err(EventError::MissingSubject)
            );
        }
    }

    #[test]
    fn reserved_type_prefix_detected() {
        assert!(is_reserved_type("io.loams.dev.usage.house.query.v1"));
        assert!(is_reserved_type("io.loams.dev.meter.threshold.v1"));
        assert!(!is_reserved_type("io.loams.devx.usage"));
        assert!(!is_reserved_type("com.example.api.request"));
        assert!(!is_reserved_type("io.loams.de"));

        assert_eq!(
            Event::from_cloudevent(&ce(HOUSE_QUERY), Origin::Tenant),
            Err(EventError::ReservedType(
                "io.loams.dev.usage.house.query.v1".to_owned()
            ))
        );
        let meter_type = with(
            HOUSE_QUERY,
            "type",
            Value::from("io.loams.dev.meter.threshold.v1"),
        );
        assert!(matches!(
            Event::from_cloudevent(&ce(&meter_type), Origin::Tenant),
            Err(EventError::ReservedType(_))
        ));
    }

    #[test]
    fn usage_type_shape() {
        for ok in [
            "io.loams.dev.usage.house.query.v1",
            "io.loams.dev.usage.node.cpu.v1",
            "io.loams.dev.usage.git.storage.v12",
            "io.loams.dev.usage.llm.tokens.v2",
        ] {
            assert!(is_loams_usage_type(ok), "{ok}");
        }
        for bad in [
            "io.loams.dev.usage.house.v1",
            "io.loams.dev.usage.house.query",
            "io.loams.dev.usage.house.query.v0",
            "io.loams.dev.usage.house.query.v01",
            "io.loams.dev.usage.House.query.v1",
            "io.loams.dev.usage..query.v1",
        ] {
            assert!(!is_loams_usage_type(bad), "{bad}");
        }
        let bad = with(
            HOUSE_QUERY,
            "type",
            Value::from("io.loams.dev.usage.house.v1"),
        );
        assert!(matches!(
            Event::from_cloudevent(&ce(&bad), Origin::Service),
            Err(EventError::BadType(_))
        ));
    }

    #[test]
    fn data_must_be_a_json_object() {
        let array = with(HOUSE_QUERY, "data", serde_json::json!([1, 2]));
        assert_eq!(
            Event::from_cloudevent(&ce(&array), Origin::Service),
            Err(EventError::DataNotObject)
        );
        let text = with(HOUSE_QUERY, "datacontenttype", Value::from("text/plain"));
        let text = with(&text, "data", Value::from("hello"));
        assert!(matches!(
            Event::from_cloudevent(&ce(&text), Origin::Service),
            Err(EventError::NotJsonData(_))
        ));
        let no_data = without(HOUSE_QUERY, "data");
        assert_eq!(
            Event::from_cloudevent(&ce(&no_data), Origin::Service),
            Err(EventError::DataNotObject)
        );
    }

    #[test]
    fn loams_types_need_tenant_and_dataschema() {
        let no_tenant = without(HOUSE_QUERY, "tenantid");
        assert_eq!(
            Event::from_cloudevent(&ce(&no_tenant), Origin::Service),
            Err(EventError::MissingTenant)
        );
        let no_schema = without(HOUSE_QUERY, "dataschema");
        assert!(matches!(
            Event::from_cloudevent(&ce(&no_schema), Origin::Service),
            Err(EventError::Dataschema { .. })
        ));
    }

    #[test]
    fn external_event_needs_no_loams_attributes() {
        let ext = r#"{"specversion":"1.0","id":"r-1","source":"loams-ns://7/billing",
            "type":"com.example.api.request","subject":"customer-1",
            "time":"2026-10-19T09:00:00Z","data":{"route":"/v1/x","bytes":12}}"#;
        let e = Event::from_cloudevent(&ce(ext), Origin::Tenant).expect("external");
        assert_eq!(e.tenant, None);
        assert_eq!(e.lease, None);
    }

    #[test]
    fn bad_lease_refused() {
        let bad = with(HOUSE_QUERY, "loamslease", Value::from("NOT-A-LEASE"));
        assert!(matches!(
            Event::from_cloudevent(&ce(&bad), Origin::Service),
            Err(EventError::Id(_))
        ));
    }

    #[test]
    fn time_window_for_external_events() {
        let e = Event::from_cloudevent(&ce(HOUSE_QUERY), Origin::Service).expect("profile");
        let now = e.time;
        assert!(e.check_time_window(now).is_ok());
        assert!(e.check_time_window(now.add_nanos(MAX_PAST)).is_ok());
        assert!(e.check_time_window(now.add_nanos(MAX_PAST + 1)).is_err());
        assert!(e.check_time_window(now.add_nanos(-MAX_FUTURE - 1)).is_err());
    }

    #[test]
    fn cloudevent_round_trip() {
        let e = Event::from_cloudevent(&ce(HOUSE_QUERY), Origin::Service).expect("profile");
        let back = e.to_cloudevent().expect("to cloudevent");
        assert_eq!(back.attr(LEASE_EXTENSION), Some("l9k2m4q8x1"));
        let again = Event::from_cloudevent(&back, Origin::Service).expect("round trip");
        assert_eq!(again, e);
    }

    /// OpenMeter's ingest `Event` schema, transcribed from
    /// `api/openapi.yaml` (components.schemas.Event) as read on 2026-10-19:
    /// required `id`, `source`, `specversion`, `type`, `subject`;
    /// `specversion` "1.0"; `time` a date-time; `datacontenttype` only
    /// `application/json`; `data` an object. Task 15's conformance test
    /// checks the same against the pinned file.
    fn openmeter_event_schema_errors(v: &Value) -> Vec<String> {
        let mut errors = Vec::new();
        let Some(obj) = v.as_object() else {
            return vec!["not an object".to_owned()];
        };
        for required in ["id", "source", "specversion", "type", "subject"] {
            match obj.get(required) {
                Some(Value::String(s)) if !s.is_empty() => {}
                _ => errors.push(format!("{required} missing or not a non-empty string")),
            }
        }
        if obj.get("specversion") != Some(&Value::from("1.0")) {
            errors.push("specversion must be 1.0".to_owned());
        }
        if let Some(t) = obj.get("time") {
            let ok = t
                .as_str()
                .is_some_and(|s| UtcNanos::from_rfc3339(s).is_ok());
            if !ok {
                errors.push("time is not a date-time".to_owned());
            }
        }
        if let Some(ct) = obj.get("datacontenttype")
            && ct != &Value::from(JSON_CONTENT_TYPE)
        {
            errors.push("datacontenttype must be application/json".to_owned());
        }
        if let Some(d) = obj.get("data")
            && !d.is_object()
            && !d.is_null()
        {
            errors.push("data must be an object".to_owned());
        }
        errors
    }

    #[test]
    fn openmeter_schema_accepts_usage_event() {
        let e = Event::from_cloudevent(&ce(HOUSE_QUERY), Origin::Service).expect("profile");
        let om = e.to_openmeter_json();
        assert_eq!(openmeter_event_schema_errors(&om), Vec::<String>::new());
        assert!(om.get(LEASE_EXTENSION).is_none(), "loamslease is stripped");
        // The schema check itself rejects what OpenMeter rejects.
        let mut no_subject = om.clone();
        no_subject
            .as_object_mut()
            .expect("object")
            .remove("subject");
        assert!(!openmeter_event_schema_errors(&no_subject).is_empty());
    }
}
