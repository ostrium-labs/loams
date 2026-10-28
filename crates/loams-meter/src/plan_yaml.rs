//! Plans as YAML (design §63 §9.5, §22, D1137; ME1 Task 11).
//!
//! A plan file in the GitOps repository (`plans/<name>.yaml`) names the plan
//! and its version, may define the meters and features it uses, and lists
//! its entitlement templates:
//!
//! ```yaml
//! plan: free
//! version: 3
//! meters:
//!   - slug: house_cpu_ms
//!     event_type: io.loams.dev.usage.house.cpu.v1
//!     aggregation: SUM
//!     value_property: $.cpu_ms
//! features:
//!   - key: house_cpu
//!     meter: house_cpu_ms
//!     unit: ms
//! entitlements:
//!   - feature: house_cpu
//!     metered: { period: { interval: month }, mode: hard_limit, issue_after_reset: 36000000 }
//!   - feature: pg_branches
//!     static: { max: 10 }
//!   - feature: byoc
//!     boolean: false
//! ```
//!
//! [`parse`] checks the file and turns it into core types; the apply rules
//! (versions are immutable, and so is a meter's semantics once it has
//! recorded events) are [`crate::plans`]'.

use std::collections::BTreeMap;

use loams_meter_core::{
    Aggregation, Amount, Currency, EntitlementKind, Feature, FeatureKey, Interval, JsonPath, Meter,
    Mode, Money, PlanId, Slug, Unit, UsagePeriod, UtcNanos,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{MeterError, Result};

/// The largest plan file accepted.
pub const MAX_YAML_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanFile {
    pub plan: String,
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub meters: Vec<MeterSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<FeatureSpec>,
    pub entitlements: Vec<TemplateSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeterSpec {
    pub slug: String,
    pub event_type: String,
    /// `SUM`, `COUNT`, `UNIQUE_COUNT`, `AVG`, `MIN`, `MAX`, `LATEST`.
    pub aggregation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_property: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub group_by: BTreeMap<String, String>,
    /// RFC 3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_from: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeatureSpec {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meter: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub filter: BTreeMap<String, String>,
    pub unit: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateSpec {
    pub feature: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metered: Option<MeteredSpec>,
    #[serde(default, rename = "static", skip_serializing_if = "Option::is_none")]
    pub static_config: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boolean: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeteredSpec {
    pub period: PeriodSpec,
    /// `hard_limit`, `soft_limit` or `overage_allowed`.
    pub mode: String,
    /// A decimal in the feature's unit (a number or a string).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_after_reset: Option<serde_json::Value>,
    #[serde(default)]
    pub preserve_overage_at_reset: bool,
    /// With `overage_allowed`: `{ micros: 1500, currency: EUR }`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overage_price: Option<MoneySpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spend_cap: Option<MoneySpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeriodSpec {
    /// `hour`, `day`, `week`, `month`, `year`.
    pub interval: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoneySpec {
    pub micros: i64,
    pub currency: String,
}

/// One entitlement a plan gives each subject of a customer.
#[derive(Debug, Clone, PartialEq)]
pub struct Template {
    pub feature: FeatureKey,
    /// For a metered template, the period's anchor is set when the plan is
    /// assigned (the customer's billing anchor).
    pub kind: EntitlementKind,
}

/// A parsed, checked plan.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub name: PlanId,
    pub version: u32,
    pub meters: Vec<Meter>,
    pub features: Vec<Feature>,
    pub templates: Vec<Template>,
    /// SHA-256 of the canonical JSON of the file, hex: two applies of the
    /// same plan version must have the same digest.
    pub digest: String,
    pub yaml: String,
}

fn bad(field: impl Into<String>, msg: impl Into<String>) -> MeterError {
    MeterError::invalid(field, msg)
}

fn aggregation(s: &str, field: &str) -> Result<Aggregation> {
    Ok(match s.to_ascii_uppercase().as_str() {
        "SUM" => Aggregation::Sum,
        "COUNT" => Aggregation::Count,
        "UNIQUE_COUNT" => Aggregation::UniqueCount,
        "AVG" => Aggregation::Avg,
        "MIN" => Aggregation::Min,
        "MAX" => Aggregation::Max,
        "LATEST" => Aggregation::Latest,
        other => return Err(bad(field, format!("unknown aggregation {other}"))),
    })
}

fn interval(s: &str, field: &str) -> Result<Interval> {
    Ok(match s {
        "hour" => Interval::Hour,
        "day" => Interval::Day,
        "week" => Interval::Week,
        "month" => Interval::Month,
        "year" => Interval::Year,
        other => return Err(bad(field, format!("unknown interval {other}"))),
    })
}

fn money(m: &MoneySpec, field: &str) -> Result<Money> {
    let c = Currency::parse(&m.currency).map_err(|e| bad(field, e.to_string()))?;
    Ok(Money::new(m.micros, c))
}

fn amount(v: &serde_json::Value, field: &str) -> Result<Amount> {
    let a = match v {
        serde_json::Value::String(s) => Amount::parse(s).ok(),
        other => Amount::from_json(other),
    };
    a.ok_or_else(|| bad(field, "must be a decimal"))
}

fn meter(m: &MeterSpec, at: &str) -> Result<Meter> {
    let slug = Slug::new(m.slug.clone()).map_err(|e| bad(format!("{at}.slug"), e.to_string()))?;
    let path = |p: &str, f: String| JsonPath::parse(p).map_err(|e| bad(f, e.to_string()));
    let meter = Meter {
        slug,
        event_type: m.event_type.clone(),
        aggregation: aggregation(&m.aggregation, &format!("{at}.aggregation"))?,
        value_property: m
            .value_property
            .as_deref()
            .map(|p| path(p, format!("{at}.value_property")))
            .transpose()?,
        group_by: m
            .group_by
            .iter()
            .map(|(k, p)| Ok((k.clone(), path(p, format!("{at}.group_by.{k}"))?)))
            .collect::<Result<_>>()?,
        event_from: m
            .event_from
            .as_deref()
            .map(|t| {
                UtcNanos::from_rfc3339(t)
                    .map_err(|e| bad(format!("{at}.event_from"), e.to_string()))
            })
            .transpose()?,
    };
    meter
        .validate()
        .map_err(|e| bad(at.to_string(), e.to_string()))?;
    Ok(meter)
}

fn template(t: &TemplateSpec, at: &str) -> Result<Template> {
    let feature = FeatureKey::new(t.feature.clone())
        .map_err(|e| bad(format!("{at}.feature"), e.to_string()))?;
    let kinds = usize::from(t.metered.is_some())
        + usize::from(t.static_config.is_some())
        + usize::from(t.boolean.is_some());
    if kinds != 1 {
        return Err(bad(at, "exactly one of metered, static, boolean"));
    }
    let kind = if let Some(m) = &t.metered {
        let mode = match m.mode.as_str() {
            "hard_limit" => Mode::HardLimit,
            "soft_limit" => Mode::SoftLimit,
            "overage_allowed" => {
                let price = m.overage_price.as_ref().ok_or_else(|| {
                    bad(
                        format!("{at}.metered.overage_price"),
                        "required with overage_allowed",
                    )
                })?;
                let cap = m.spend_cap.as_ref().ok_or_else(|| {
                    bad(
                        format!("{at}.metered.spend_cap"),
                        "required with overage_allowed",
                    )
                })?;
                Mode::OverageAllowed {
                    overage_price: money(price, &format!("{at}.metered.overage_price"))?,
                    spend_cap: money(cap, &format!("{at}.metered.spend_cap"))?,
                }
            }
            other => {
                return Err(bad(
                    format!("{at}.metered.mode"),
                    format!("unknown mode {other}"),
                ));
            }
        };
        let kind = EntitlementKind::Metered {
            usage_period: UsagePeriod {
                interval: interval(&m.period.interval, &format!("{at}.metered.period.interval"))?,
                anchor: UtcNanos(0),
            },
            mode,
            issue_after_reset: m
                .issue_after_reset
                .as_ref()
                .map(|v| amount(v, &format!("{at}.metered.issue_after_reset")))
                .transpose()?,
            preserve_overage_at_reset: m.preserve_overage_at_reset,
        };
        crate::entitlements::validate_kind(&kind)?;
        kind
    } else if let Some(config) = &t.static_config {
        EntitlementKind::Static {
            config: config.clone(),
        }
    } else {
        EntitlementKind::Boolean {
            enabled: t.boolean.unwrap_or(false),
        }
    };
    Ok(Template { feature, kind })
}

/// Parses and checks a plan file.
pub fn parse(yaml: &str) -> Result<Plan> {
    if yaml.len() > MAX_YAML_BYTES {
        return Err(bad("yaml", format!("larger than {MAX_YAML_BYTES} bytes")));
    }
    let file: PlanFile = serde_yaml_ng::from_str(yaml).map_err(|e| bad("yaml", e.to_string()))?;
    let name = PlanId::new(file.plan.clone()).map_err(|e| bad("plan", e.to_string()))?;
    if !loams_meter_core::ids::valid_slug(&file.plan) {
        return Err(bad("plan", "a plan name is a slug: [a-z0-9]+(_[a-z0-9]+)*"));
    }
    if file.version == 0 {
        return Err(bad("version", "versions start at 1"));
    }
    let meters = file
        .meters
        .iter()
        .enumerate()
        .map(|(i, m)| meter(m, &format!("meters[{i}]")))
        .collect::<Result<Vec<_>>>()?;
    let mut slugs = std::collections::HashSet::new();
    for m in &meters {
        if !slugs.insert(m.slug.clone()) {
            return Err(bad("meters", format!("meter {} is defined twice", m.slug)));
        }
    }
    let features = file
        .features
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let at = format!("features[{i}]");
            Ok(Feature {
                key: FeatureKey::new(f.key.clone())
                    .map_err(|e| bad(format!("{at}.key"), e.to_string()))?,
                meter: f
                    .meter
                    .as_ref()
                    .map(|m| {
                        Slug::new(m.clone()).map_err(|e| bad(format!("{at}.meter"), e.to_string()))
                    })
                    .transpose()?,
                filter: f.filter.clone(),
                unit: Unit::parse(&f.unit),
                archived: false,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let templates = file
        .entitlements
        .iter()
        .enumerate()
        .map(|(i, t)| template(t, &format!("entitlements[{i}]")))
        .collect::<Result<Vec<_>>>()?;
    let mut seen = std::collections::HashSet::new();
    for t in &templates {
        if !seen.insert(t.feature.clone()) {
            return Err(bad(
                "entitlements",
                format!("feature {} appears twice", t.feature),
            ));
        }
    }
    let canonical = serde_json::to_vec(&file)
        .map_err(|e| MeterError::Unavailable(format!("plan digest: {e}")))?;
    let digest = Sha256::digest(&canonical)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(Plan {
        name,
        version: file.version,
        meters,
        features,
        templates,
        digest,
        yaml: yaml.to_string(),
    })
}

/// Whether two meters differ in what they count (display fields aside, a
/// meter has none in Loams): the event type, the aggregation, the value
/// property, the group-by and `event_from`.
pub fn semantics_differ(a: &Meter, b: &Meter) -> bool {
    a.event_type != b.event_type
        || a.aggregation != b.aggregation
        || a.value_property != b.value_property
        || a.group_by != b.group_by
        || a.event_from != b.event_from
}

#[cfg(test)]
mod tests {
    use super::*;

    const FREE: &str = r#"
plan: free
version: 3
meters:
  - slug: house_cpu_ms
    event_type: io.loams.dev.usage.house.cpu.v1
    aggregation: SUM
    value_property: $.cpu_ms
features:
  - key: house_cpu
    meter: house_cpu_ms
    unit: ms
entitlements:
  - feature: house_cpu
    metered: { period: { interval: month }, mode: hard_limit, issue_after_reset: 36000000 }
  - feature: pg_branches
    static: { max: 10 }
  - feature: byoc
    boolean: false
"#;

    #[test]
    fn the_design_example_parses() {
        let p = parse(FREE).expect("free");
        assert_eq!(p.name.as_str(), "free");
        assert_eq!(p.version, 3);
        assert_eq!(p.meters.len(), 1);
        assert_eq!(p.templates.len(), 3);
        assert!(matches!(
            &p.templates[0].kind,
            EntitlementKind::Metered {
                mode: Mode::HardLimit,
                issue_after_reset: Some(a),
                ..
            } if *a == Amount::from_units(36_000_000)
        ));
        assert!(matches!(
            p.templates[2].kind,
            EntitlementKind::Boolean { enabled: false }
        ));
        assert_eq!(p.digest.len(), 64);
        // Formatting does not change the digest; content does.
        let reformatted = FREE.replace("{ max: 10 }", "\n      max: 10");
        assert_eq!(parse(&reformatted).expect("same").digest, p.digest);
        let changed = FREE.replace("max: 10", "max: 11");
        assert_ne!(parse(&changed).expect("changed").digest, p.digest);
    }

    #[test]
    fn bad_files_are_refused_with_the_field() {
        let e = parse(&FREE.replace("mode: hard_limit", "mode: unlimited")).expect_err("mode");
        assert!(
            matches!(e, MeterError::InvalidArgument { ref field, .. } if field == "entitlements[0].metered.mode")
        );
        let e = parse(&FREE.replace("version: 3", "version: 0")).expect_err("version");
        assert!(matches!(e, MeterError::InvalidArgument { ref field, .. } if field == "version"));
        let e = parse(&FREE.replace("boolean: false", "boolean: false\n    static: 1"))
            .expect_err("two kinds");
        assert!(matches!(e, MeterError::InvalidArgument { .. }));
        let e = parse(&format!("{FREE}  - feature: byoc\n    boolean: true\n")).expect_err("twice");
        assert!(
            matches!(e, MeterError::InvalidArgument { ref field, .. } if field == "entitlements")
        );
        let e = parse("plan: free\nversion: 1\nentitlements: []\nsurprise: 1\n")
            .expect_err("unknown key");
        assert!(matches!(e, MeterError::InvalidArgument { ref field, .. } if field == "yaml"));
    }
}
