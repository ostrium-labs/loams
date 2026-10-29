//! Meters: how events become numbers (design §63 §8.1, OpenMeter's
//! `MeterCreate`).

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::amount::Amount;
use crate::event::Event;
use crate::ids::{Slug, UtcNanos};

/// At most this many group-by dimensions per meter (§8.1).
pub const MAX_GROUP_BY: usize = 8;

/// OpenMeter's aggregations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Aggregation {
    Sum,
    Count,
    UniqueCount,
    Avg,
    Min,
    Max,
    Latest,
}

impl Aggregation {
    /// Hour and day windows are rolled up from minute windows only for
    /// these; the others are recomputed from raw events (§8.2).
    pub fn rolls_up(self) -> bool {
        matches!(self, Self::Sum | Self::Count | Self::Min | Self::Max)
    }
}

/// Window sizes of a meter query.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum WindowSize {
    Minute,
    Hour,
    Day,
    Month,
}

impl WindowSize {
    /// The fixed length in nanoseconds, or `None` for `Month`.
    pub fn fixed_nanos(self) -> Option<i64> {
        match self {
            Self::Minute => Some(UtcNanos::MINUTE),
            Self::Hour => Some(UtcNanos::HOUR),
            Self::Day => Some(UtcNanos::DAY),
            Self::Month => None,
        }
    }
}

/// Why a meter definition or an event's value was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MeterError {
    #[error("not a supported JSONPath: '{0}' (use $.a.b, $.a[\"b c\"], $.a[0])")]
    Path(String),
    #[error("meter '{0}' needs an event type")]
    NoEventType(String),
    #[error("meter '{0}' with aggregation {1:?} needs a value property")]
    NoValueProperty(String, Aggregation),
    #[error("meter '{0}' has more than {MAX_GROUP_BY} group-by dimensions")]
    TooManyGroups(String),
    #[error("meter '{0}' has an empty group-by name")]
    EmptyGroupName(String),
    #[error("event has no number at '{0}'")]
    MissingValue(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Step {
    Key(String),
    Index(usize),
}

/// The JSONPath subset meters use: a root `$` followed by `.name`,
/// `["name"]` or `[index]` steps (D2163). Filters, wildcards and recursive
/// descent are refused.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct JsonPath {
    text: String,
    steps: Vec<Step>,
}

impl JsonPath {
    pub fn parse(text: &str) -> Result<JsonPath, MeterError> {
        let err = || MeterError::Path(text.to_owned());
        let mut rest = text.strip_prefix('$').ok_or_else(err)?;
        let mut steps = Vec::new();
        while !rest.is_empty() {
            if let Some(after) = rest.strip_prefix('.') {
                let end = after.find(['.', '[']).unwrap_or(after.len());
                let name = &after[..end];
                if name.is_empty() || name == "*" {
                    return Err(err());
                }
                steps.push(Step::Key(name.to_owned()));
                rest = &after[end..];
            } else if let Some(after) = rest.strip_prefix("[\"") {
                let end = after.find("\"]").ok_or_else(err)?;
                let name = &after[..end];
                if name.is_empty() || name.contains('"') {
                    return Err(err());
                }
                steps.push(Step::Key(name.to_owned()));
                rest = &after[end + 2..];
            } else if let Some(after) = rest.strip_prefix('[') {
                let end = after.find(']').ok_or_else(err)?;
                let digits = &after[..end];
                if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(err());
                }
                let index = digits.parse().map_err(|_| err())?;
                steps.push(Step::Index(index));
                rest = &after[end + 1..];
            } else {
                return Err(err());
            }
        }
        Ok(JsonPath {
            text: text.to_owned(),
            steps,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The value at this path in `root`, if any.
    pub fn get<'a>(&self, root: &'a Value) -> Option<&'a Value> {
        self.steps.iter().try_fold(root, |v, step| match step {
            Step::Key(k) => v.as_object()?.get(k),
            Step::Index(i) => v.as_array()?.get(*i),
        })
    }

    /// The value at this path in an event's `data` object.
    pub fn get_in<'a>(&self, data: &'a serde_json::Map<String, Value>) -> Option<&'a Value> {
        let (first, rest) = self.steps.split_first()?;
        let Step::Key(k) = first else {
            return None;
        };
        let start = data.get(k)?;
        rest.iter().try_fold(start, |v, step| match step {
            Step::Key(k) => v.as_object()?.get(k),
            Step::Index(i) => v.as_array()?.get(*i),
        })
    }
}

impl fmt::Debug for JsonPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JsonPath({})", self.text)
    }
}

impl fmt::Display for JsonPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl Serialize for JsonPath {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.text)
    }
}

impl<'de> Deserialize<'de> for JsonPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        JsonPath::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// A meter (OpenMeter's definition).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meter {
    pub slug: Slug,
    pub event_type: String,
    pub aggregation: Aggregation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_property: Option<JsonPath>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub group_by: BTreeMap<String, JsonPath>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_from: Option<UtcNanos>,
}

impl Meter {
    /// Checks the rules §8.1 adds to the types: an event type, a value
    /// property unless `COUNT`, at most eight named group-by dimensions.
    pub fn validate(&self) -> Result<(), MeterError> {
        let slug = self.slug.as_str().to_owned();
        if self.event_type.is_empty() {
            return Err(MeterError::NoEventType(slug));
        }
        if self.aggregation != Aggregation::Count && self.value_property.is_none() {
            return Err(MeterError::NoValueProperty(slug, self.aggregation));
        }
        if self.group_by.len() > MAX_GROUP_BY {
            return Err(MeterError::TooManyGroups(slug));
        }
        if self.group_by.keys().any(String::is_empty) {
            return Err(MeterError::EmptyGroupName(slug));
        }
        Ok(())
    }

    /// True when this meter selects `event`: same type, not before
    /// `event_from`, and not a `io.loams.dev.meter.*` notification.
    pub fn matches(&self, event: &Event) -> bool {
        event.is_usage()
            && event.r#type == self.event_type
            && self.event_from.is_none_or(|from| event.time >= from)
    }

    /// The numeric value `event` contributes: 1 for `COUNT`, the number at
    /// `value_property` otherwise (a JSON number, or a string holding a
    /// decimal, as OpenMeter accepts). `Ok(None)` when the meter does not
    /// select the event or is `UNIQUE_COUNT` (use [`Meter::unique_value`]).
    pub fn value(&self, event: &Event) -> Result<Option<Amount>, MeterError> {
        if !self.matches(event) {
            return Ok(None);
        }
        match self.aggregation {
            Aggregation::Count => Ok(Some(Amount::from_units(1))),
            Aggregation::UniqueCount => Ok(None),
            _ => {
                let Some(path) = &self.value_property else {
                    return Err(MeterError::NoValueProperty(
                        self.slug.as_str().to_owned(),
                        self.aggregation,
                    ));
                };
                let missing = || MeterError::MissingValue(path.as_str().to_owned());
                let v = path.get_in(&event.data).ok_or_else(missing)?;
                let amount = match v {
                    Value::String(s) => Amount::parse(s).ok(),
                    other => Amount::from_json(other),
                };
                amount.map(Some).ok_or_else(missing)
            }
        }
    }

    /// For `UNIQUE_COUNT`: the string whose distinct values are counted.
    /// Numbers are counted by their JSON text.
    pub fn unique_value(&self, event: &Event) -> Option<String> {
        if self.aggregation != Aggregation::UniqueCount || !self.matches(event) {
            return None;
        }
        match self.value_property.as_ref()?.get_in(&event.data)? {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        }
    }

    /// The group-by values of `event`: strings as they are, other JSON
    /// values by their JSON text, missing dimensions as `""`.
    pub fn group_values(&self, event: &Event) -> BTreeMap<String, String> {
        self.group_by
            .iter()
            .map(|(name, path)| {
                let v = match path.get_in(&event.data) {
                    Some(Value::String(s)) => s.clone(),
                    Some(Value::Null) | None => String::new(),
                    Some(other) => other.to_string(),
                };
                (name.clone(), v)
            })
            .collect()
    }

    /// `_meter.windows.group_key`: the canonical JSON of the group-by
    /// values (keys sorted, no whitespace).
    pub fn group_key(&self, event: &Event) -> String {
        canonical_group_key(&self.group_values(event))
    }
}

/// Canonical JSON for a set of group-by values.
pub fn canonical_group_key(values: &BTreeMap<String, String>) -> String {
    let mut out = String::from("{");
    for (i, (k, v)) in values.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&Value::String(k.clone()).to_string());
        out.push(':');
        out.push_str(&Value::String(v.clone()).to_string());
    }
    out.push('}');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::SubjectKey;
    use serde_json::json;

    fn event(ty: &str, data: Value) -> Event {
        Event {
            id: "1".into(),
            source: "loams://c/house/n1".into(),
            r#type: ty.into(),
            subject: SubjectKey::new("org_1").expect("subject"),
            time: UtcNanos(1_000),
            tenant: None,
            lease: None,
            data: data.as_object().cloned().expect("object"),
        }
    }

    fn meter(aggregation: Aggregation, value: Option<&str>) -> Meter {
        Meter {
            slug: Slug::new("llm_output_tokens").expect("slug"),
            event_type: "io.loams.dev.usage.llm.tokens.v1".into(),
            aggregation,
            value_property: value.map(|v| JsonPath::parse(v).expect("path")),
            group_by: BTreeMap::from([("model".into(), JsonPath::parse("$.model").expect("path"))]),
            event_from: None,
        }
    }

    #[test]
    fn json_path_subset() {
        let doc = json!({"a": {"b c": [10, {"d": 4}]}, "x": 1});
        assert_eq!(
            JsonPath::parse("$.x").expect("p").get(&doc),
            Some(&json!(1))
        );
        assert_eq!(
            JsonPath::parse("$.a[\"b c\"][1].d").expect("p").get(&doc),
            Some(&json!(4))
        );
        assert_eq!(JsonPath::parse("$").expect("p").get(&doc), Some(&doc));
        assert_eq!(JsonPath::parse("$.missing").expect("p").get(&doc), None);
        for bad in ["x", "$.", "$..a", "$.*", "$[a]", "$[\"a\"", "$.a[?(@.b)]"] {
            assert!(JsonPath::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn values_by_aggregation() {
        let e = event(
            "io.loams.dev.usage.llm.tokens.v1",
            json!({"model": "m1", "output_tokens": 120, "frac": "0.5"}),
        );
        let sum = meter(Aggregation::Sum, Some("$.output_tokens"));
        assert_eq!(sum.value(&e), Ok(Some(Amount::from_units(120))));
        let frac = meter(Aggregation::Sum, Some("$.frac"));
        assert_eq!(frac.value(&e), Ok(Some(Amount::from_raw(500_000_000))));
        let count = meter(Aggregation::Count, None);
        assert_eq!(count.value(&e), Ok(Some(Amount::from_units(1))));
        let missing = meter(Aggregation::Max, Some("$.nope"));
        assert!(matches!(
            missing.value(&e),
            Err(MeterError::MissingValue(_))
        ));
        let unique = meter(Aggregation::UniqueCount, Some("$.model"));
        assert_eq!(unique.value(&e), Ok(None));
        assert_eq!(unique.unique_value(&e), Some("m1".into()));
    }

    #[test]
    fn selection_by_type_time_and_notifications() {
        let mut m = meter(Aggregation::Count, None);
        let other = event("io.loams.dev.usage.house.query.v1", json!({}));
        assert_eq!(m.value(&other), Ok(None));
        m.event_from = Some(UtcNanos(2_000));
        let early = event("io.loams.dev.usage.llm.tokens.v1", json!({}));
        assert_eq!(m.value(&early), Ok(None));
        m.event_type = "io.loams.dev.meter.threshold.v1".into();
        m.event_from = None;
        let note = event("io.loams.dev.meter.threshold.v1", json!({}));
        assert!(!m.matches(&note), "notifications are never metered");
    }

    #[test]
    fn group_key_is_canonical() {
        let mut m = meter(Aggregation::Count, None);
        m.group_by.insert(
            "a_provider".into(),
            JsonPath::parse("$.provider").expect("p"),
        );
        let e = event(
            "io.loams.dev.usage.llm.tokens.v1",
            json!({"model": "m\"1", "provider": 3}),
        );
        assert_eq!(m.group_key(&e), r#"{"a_provider":"3","model":"m\"1"}"#);
        let none = event("io.loams.dev.usage.llm.tokens.v1", json!({}));
        assert_eq!(m.group_key(&none), r#"{"a_provider":"","model":""}"#);
    }

    #[test]
    fn validation_rules() {
        assert!(meter(Aggregation::Count, None).validate().is_ok());
        assert!(matches!(
            meter(Aggregation::Sum, None).validate(),
            Err(MeterError::NoValueProperty(..))
        ));
        let mut many = meter(Aggregation::Count, None);
        for i in 0..MAX_GROUP_BY {
            many.group_by
                .insert(format!("g{i}"), JsonPath::parse("$.g").expect("p"));
        }
        assert!(matches!(many.validate(), Err(MeterError::TooManyGroups(_))));
    }

    #[test]
    fn serde_matches_openmeter_names() {
        let m = meter(Aggregation::UniqueCount, Some("$.model"));
        let v = serde_json::to_value(&m).expect("ser");
        assert_eq!(v["aggregation"], json!("UNIQUE_COUNT"));
        assert_eq!(v["value_property"], json!("$.model"));
        let back: Meter = serde_json::from_value(v).expect("de");
        assert_eq!(back, m);
        assert_eq!(
            serde_json::to_value(WindowSize::Hour).expect("ser"),
            json!("HOUR")
        );
    }
}
