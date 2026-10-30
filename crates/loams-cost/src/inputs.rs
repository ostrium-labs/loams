//! Cost inputs (design §84 §6, D2352–D2357): one YAML document,
//! `costs.yaml`, with the operator's assets, price table, FX rates, mapping
//! rules, engine rules, query classes and options. Each apply is a new,
//! immutable version.

use std::collections::{BTreeMap, BTreeSet};

use loams_meter_core::{Amount, Currency};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};

use crate::{CostError, Day, PoolId, ResourceKind};

/// The default asset life when `costs.yaml` leaves it out (Q1490).
pub const DEFAULT_LIFE_MONTHS: u32 = 60;

/// The longest asset life accepted (50 years).
pub const MAX_LIFE_MONTHS: u32 = 600;

fn currency<'de, D: Deserializer<'de>>(d: D) -> Result<Currency, D::Error> {
    let s = String::deserialize(d)?;
    Currency::parse(&s).map_err(serde::de::Error::custom)
}

fn currency_out<S: serde::Serializer>(c: &Currency, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(c.as_str())
}

/// The whole document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CostInputs {
    /// Monotonic. The same version with other content is refused.
    pub version: u64,
    /// The reporting currency (D2357).
    #[serde(deserialize_with = "currency", serialize_with = "currency_out")]
    pub currency: Currency,
    #[serde(default)]
    pub assets: Vec<Asset>,
    #[serde(default)]
    pub prices: Vec<Price>,
    #[serde(default)]
    pub fx: Vec<FxRate>,
    #[serde(default)]
    pub mappings: Vec<MappingRule>,
    #[serde(default)]
    pub engines: Vec<EngineRule>,
    /// Which pool a node's (or an event type's) usage belongs to, beyond
    /// the nodes the assets list (§84 §7, CA1 Task 5).
    #[serde(default)]
    pub usage_pools: Vec<UsagePoolRule>,
    #[serde(default)]
    pub classes: Vec<ClassRule>,
    #[serde(default)]
    pub options: Options,
}

/// Hardware the operator owns, amortised per day (§84 §6.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    pub id: String,
    pub pool: PoolId,
    /// Purchase price, in the reporting currency.
    pub cost: Amount,
    /// Value at the end of its life; 0 when left out.
    #[serde(default)]
    pub residual: Option<Amount>,
    pub in_service: Day,
    /// Straight-line life; [`DEFAULT_LIFE_MONTHS`] when left out.
    #[serde(default)]
    pub life_months: Option<u32>,
    /// Declared monthly costs (hosting, power, support), spread per day of
    /// the month, for as long as the asset is in service.
    #[serde(default)]
    pub monthly: BTreeMap<String, Amount>,
    /// Until when the monthly costs run (exclusive); unbounded when left out.
    #[serde(default)]
    pub retired: Option<Day>,
    #[serde(default)]
    pub capacity: AssetCapacity,
    /// How the asset's cost divides over resource kinds; must sum to 1.
    /// Left out, everything goes to `cpu` and is `estimated` (§84 §5.2).
    #[serde(default)]
    pub split: Option<BTreeMap<ResourceKind, Amount>>,
    /// The node ids the meter events name, for the drivers (Task 5).
    #[serde(default)]
    pub nodes: Vec<String>,
}

/// What an asset can deliver per second.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetCapacity {
    #[serde(default)]
    pub cores: Option<Amount>,
    #[serde(default)]
    pub memory_bytes: Option<Amount>,
    #[serde(default)]
    pub disk_bytes: Option<Amount>,
    #[serde(default)]
    pub iops: Option<Amount>,
    #[serde(default)]
    pub gpus: Option<Amount>,
}

impl AssetCapacity {
    /// The per-second capacity for `kind`, if declared.
    pub fn per_second(&self, kind: ResourceKind) -> Option<Amount> {
        match kind {
            ResourceKind::Cpu => self.cores,
            ResourceKind::Memory => self.memory_bytes,
            ResourceKind::Storage => self.disk_bytes,
            ResourceKind::Io => self.iops,
            ResourceKind::Gpu => self.gpus,
            _ => None,
        }
    }
}

/// Where a price came from (§84 §6.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriceSource {
    /// A contract rate: `declared`.
    Contract,
    /// A published list price: `estimated` when it prices a pool.
    List,
    /// A judgement: `estimated`.
    Estimate,
}

/// The unit a price is quoted per. Rates per byte-second would need far
/// more than nine decimals, so prices name a human unit and the arithmetic
/// divides by its size in driver units, exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriceUnit {
    CoreSecond,
    CoreHour,
    ByteSecond,
    GbHour,
    GibHour,
    /// 10⁹ bytes for a mean Gregorian month (2,629,746 s).
    GbMonth,
    GibMonth,
    Byte,
    Gb,
    Gib,
    Operation,
    MillionOperations,
    DeviceSecond,
    DeviceHour,
}

/// Seconds in a mean Gregorian month (365.2425 × 86,400 / 12).
pub const MEAN_MONTH_SECONDS: u128 = 2_629_746;

impl PriceUnit {
    /// The unit's size in `kind`'s driver unit, or `None` when the unit does
    /// not fit the kind.
    pub fn size(self, kind: ResourceKind) -> Option<u128> {
        use PriceUnit::*;
        use ResourceKind as K;
        const GB: u128 = 1_000_000_000;
        const GIB: u128 = 1 << 30;
        match (kind, self) {
            (K::Cpu, CoreSecond) | (K::Gpu, DeviceSecond) => Some(1),
            (K::Cpu, CoreHour) | (K::Gpu, DeviceHour) => Some(3_600),
            (K::Memory | K::Storage, ByteSecond) => Some(1),
            (K::Memory | K::Storage, GbHour) => Some(GB * 3_600),
            (K::Memory | K::Storage, GibHour) => Some(GIB * 3_600),
            (K::Memory | K::Storage, GbMonth) => Some(GB * MEAN_MONTH_SECONDS),
            (K::Memory | K::Storage, GibMonth) => Some(GIB * MEAN_MONTH_SECONDS),
            (K::NetworkEgress | K::NetworkIngress | K::NetworkInternal, Byte) => Some(1),
            (K::NetworkEgress | K::NetworkIngress | K::NetworkInternal, Gb) => Some(GB),
            (K::NetworkEgress | K::NetworkIngress | K::NetworkInternal, Gib) => Some(GIB),
            (K::Io, Operation) => Some(1),
            (K::Io, MillionOperations) => Some(1_000_000),
            _ => None,
        }
    }
}

/// A rate per [`PriceUnit`] for a pool's kind, in the reporting currency.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Price {
    pub id: String,
    pub kind: ResourceKind,
    pub scope: PoolId,
    pub rate: Amount,
    pub unit: PriceUnit,
    pub source: PriceSource,
    pub effective: Day,
    /// Exclusive.
    #[serde(default)]
    pub until: Option<Day>,
    /// Where the rate came from (a URL for a list price).
    #[serde(default)]
    pub note: Option<String>,
}

impl Price {
    pub fn covers(&self, day: Day) -> bool {
        day >= self.effective && self.until.is_none_or(|u| day < u)
    }
}

/// A declared exchange rate for one day: `amount_in_to = amount_in_from ×
/// rate`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FxRate {
    #[serde(deserialize_with = "currency", serialize_with = "currency_out")]
    pub from: Currency,
    #[serde(deserialize_with = "currency", serialize_with = "currency_out")]
    pub to: Currency,
    pub rate: Amount,
    pub day: Day,
    pub source: String,
}

/// Turns bill lines into pools and kinds (§84 §6.3). Matching is in
/// `import::mapping`; here only the shape is checked.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MappingRule {
    #[serde(rename = "match")]
    pub matcher: LineMatch,
    /// A pool id or a template such as `{tags.loams-pool}`.
    pub pool: String,
    /// A kind or a template such as `{tags.loams-kind|cpu}`.
    pub kind: String,
}

/// What a mapping rule matches; every given field must match. Values are
/// globs (`*` and `?`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineMatch {
    #[serde(default)]
    pub resource: Option<String>,
    #[serde(default)]
    pub service: Option<String>,
    #[serde(default)]
    pub sku: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub tags: BTreeMap<String, String>,
}

/// Maps node-agent resources (cgroup paths) to engines (§84 §7.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineRule {
    #[serde(rename = "match")]
    pub matcher: EngineMatch,
    pub engine: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineMatch {
    pub resource: String,
}

/// Puts usage on a pool: the first rule whose given fields all match wins
/// (after the assets' own `nodes`). Values are globs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsagePoolRule {
    #[serde(rename = "match")]
    pub matcher: UsageMatch,
    pub pool: PoolId,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageMatch {
    /// The node id (the event's `node`, or the last segment of its source).
    #[serde(default)]
    pub node: Option<String>,
    /// The event type, such as `io.loams.dev.usage.house.storage.v1`.
    #[serde(rename = "type", default)]
    pub event_type: Option<String>,
    #[serde(default)]
    pub kind: Option<ResourceKind>,
}

/// An operator query class (§84 §7.4): `when` is `field op value` clauses
/// joined by `and`; compiled to SQL in `loams-meter-house` (Task 5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassRule {
    pub engine: String,
    pub class: String,
    pub when: String,
}

/// Show or spread idle and shared cost (D2364).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    #[default]
    Show,
    Spread,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    #[serde(default)]
    pub idle: Disposition,
    #[serde(default)]
    pub shared: Disposition,
    /// Lets an org's `billing_viewer` read that org's costs (D2373).
    #[serde(default)]
    pub share_with_orgs: bool,
}

fn glob_ok(field: &str, g: &str) -> Result<(), CostError> {
    if g.is_empty() || g.len() > 512 {
        return Err(CostError::invalid(
            field,
            "a pattern is 1 to 512 characters",
        ));
    }
    Ok(())
}

impl CostInputs {
    /// Parses and validates `yaml`.
    pub fn parse(yaml: &str) -> Result<CostInputs, CostError> {
        let inputs: CostInputs = serde_yaml_ng::from_str(yaml)
            .map_err(|e| CostError::invalid("costs.yaml", e.to_string()))?;
        inputs.validate()?;
        Ok(inputs)
    }

    /// Every rule of §84 §6 that does not need a bill or usage.
    pub fn validate(&self) -> Result<(), CostError> {
        if self.version == 0 {
            return Err(CostError::invalid("version", "versions start at 1"));
        }
        let mut ids = BTreeSet::new();
        for a in &self.assets {
            let f = |m: &str| CostError::invalid(format!("assets[{}]", a.id), m.to_string());
            if a.id.is_empty() || !ids.insert(a.id.as_str()) {
                return Err(f("asset ids are non-empty and unique"));
            }
            if a.cost.is_negative() {
                return Err(f("cost is negative"));
            }
            let residual = a.residual.unwrap_or(Amount::ZERO);
            if residual.is_negative() || residual > a.cost {
                return Err(f("residual must be between 0 and the cost"));
            }
            if let Some(l) = a.life_months
                && !(1..=MAX_LIFE_MONTHS).contains(&l)
            {
                return Err(f("life_months must be 1 to 600"));
            }
            if a.monthly.values().any(|m| m.is_negative()) {
                return Err(f("a monthly cost is negative"));
            }
            if a.retired.is_some_and(|r| r <= a.in_service) {
                return Err(f("retired must be after in_service"));
            }
            for kind in ResourceKind::ALL {
                if a.capacity.per_second(kind).is_some_and(|c| c.is_negative()) {
                    return Err(f("a capacity is negative"));
                }
            }
            if let Some(split) = &a.split {
                if split.is_empty() {
                    return Err(f("split is empty; leave it out to send everything to cpu"));
                }
                if split.contains_key(&ResourceKind::Fixed) {
                    return Err(f("split cannot name `fixed`"));
                }
                if split.values().any(|w| w.is_negative()) {
                    return Err(f("split weights are negative"));
                }
                if split.values().copied().sum::<Amount>() != Amount::from_units(1) {
                    return Err(f("split weights must sum to exactly 1"));
                }
            }
        }
        let mut price_ids = BTreeSet::new();
        for (i, p) in self.prices.iter().enumerate() {
            let f = |m: &str| CostError::invalid(format!("prices[{i}]"), m.to_string());
            if p.id.is_empty() || !price_ids.insert(p.id.as_str()) {
                return Err(f("price ids are non-empty and unique"));
            }
            if p.rate.is_negative() {
                return Err(f("rate is negative"));
            }
            if p.until.is_some_and(|u| u <= p.effective) {
                return Err(f("until must be after effective"));
            }
            if p.kind == ResourceKind::Fixed {
                return Err(f("`fixed` has no driver to price"));
            }
            if p.unit.size(p.kind).is_none() {
                return Err(f("the unit does not fit the kind"));
            }
        }
        // Two prices for one (kind, scope) may not overlap in time.
        for (i, p) in self.prices.iter().enumerate() {
            for q in &self.prices[i + 1..] {
                if p.kind == q.kind && p.scope == q.scope {
                    let p_end = p.until;
                    let q_end = q.until;
                    let overlap = p_end.is_none_or(|e| q.effective < e)
                        && q_end.is_none_or(|e| p.effective < e);
                    if overlap {
                        return Err(CostError::invalid(
                            "prices",
                            format!(
                                "prices {} and {} overlap for {} {}",
                                p.id, q.id, p.scope, p.kind
                            ),
                        ));
                    }
                }
            }
        }
        let mut fx_days = BTreeSet::new();
        for (i, r) in self.fx.iter().enumerate() {
            let f = |m: &str| CostError::invalid(format!("fx[{i}]"), m.to_string());
            if r.to != self.currency {
                return Err(f("fx rates convert into the reporting currency"));
            }
            if r.from == r.to {
                return Err(f("fx from and to are the same"));
            }
            if !r.rate.is_positive() {
                return Err(f("an fx rate is positive"));
            }
            if r.source.is_empty() {
                return Err(f("an fx rate names its source"));
            }
            if !fx_days.insert((r.from, r.day)) {
                return Err(f("one rate per currency and day"));
            }
        }
        for (i, m) in self.mappings.iter().enumerate() {
            let field = format!("mappings[{i}]");
            let lm = &m.matcher;
            for g in [&lm.resource, &lm.service, &lm.sku, &lm.region, &lm.category]
                .into_iter()
                .flatten()
            {
                glob_ok(&field, g)?;
            }
            for g in lm.tags.values() {
                glob_ok(&field, g)?;
            }
            if m.pool.is_empty() || m.kind.is_empty() {
                return Err(CostError::invalid(field, "pool and kind are required"));
            }
            if !m.kind.starts_with('{') {
                ResourceKind::parse(&m.kind)?;
            }
            if !m.pool.contains('{') {
                PoolId::new(m.pool.clone())?;
            }
        }
        for (i, e) in self.engines.iter().enumerate() {
            glob_ok(&format!("engines[{i}]"), &e.matcher.resource)?;
            crate::ConsumerKey::new(e.engine.clone(), "_", "_", "_")?;
        }
        for (i, r) in self.usage_pools.iter().enumerate() {
            let m = &r.matcher;
            if m.node.is_none() && m.event_type.is_none() && m.kind.is_none() {
                return Err(CostError::invalid(
                    format!("usage_pools[{i}]"),
                    "a rule matches on node, type or kind",
                ));
            }
            for g in [&m.node, &m.event_type].into_iter().flatten() {
                glob_ok(&format!("usage_pools[{i}]"), g)?;
            }
        }
        for (i, c) in self.classes.iter().enumerate() {
            crate::ConsumerKey::new(c.engine.clone(), "_", "_", c.class.clone())?;
            if c.when.trim().is_empty() {
                return Err(CostError::invalid(format!("classes[{i}]"), "when is empty"));
            }
            crate::classes::Condition::parse(&c.when)
                .map_err(|e| CostError::invalid(format!("classes[{i}]"), e.to_string()))?;
        }
        Ok(())
    }

    /// The declared rate converting `from` into the reporting currency on
    /// `day`, if any.
    pub fn fx_rate(&self, from: Currency, day: Day) -> Option<&FxRate> {
        self.fx.iter().find(|r| r.from == from && r.day == day)
    }

    /// The price for (`scope`, `kind`) on `day`, if any.
    pub fn price(&self, scope: &PoolId, kind: ResourceKind, day: Day) -> Option<&Price> {
        self.prices
            .iter()
            .find(|p| &p.scope == scope && p.kind == kind && p.covers(day))
    }
}

/// The SHA-256 of a document's bytes, hex.
pub fn yaml_digest(yaml: &str) -> String {
    hex::encode(Sha256::digest(yaml.as_bytes()))
}

/// The stored version an apply compares against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredVersion {
    pub version: u64,
    pub digest: String,
}

/// What an apply does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// A new version to store.
    New(Box<CostInputs>),
    /// The same version with the same bytes: nothing to do.
    Unchanged,
}

/// Validates `yaml` against the latest stored version: the same version
/// with other bytes is refused (`cost_inputs_version_immutable`), and a
/// version must not go backwards.
pub fn apply(latest: Option<&StoredVersion>, yaml: &str) -> Result<ApplyOutcome, CostError> {
    let inputs = CostInputs::parse(yaml)?;
    if let Some(prev) = latest {
        if inputs.version == prev.version {
            if yaml_digest(yaml) == prev.digest {
                return Ok(ApplyOutcome::Unchanged);
            }
            return Err(CostError::invalid(
                "version",
                format!(
                    "cost_inputs_version_immutable: version {} exists with other content",
                    prev.version
                ),
            ));
        }
        if inputs.version < prev.version {
            return Err(CostError::invalid(
                "version",
                format!(
                    "version {} is older than the latest, {}",
                    inputs.version, prev.version
                ),
            ));
        }
    }
    Ok(ApplyOutcome::New(Box::new(inputs)))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const EXAMPLE: &str = r#"
version: 7
currency: USD
assets:
  - id: rack1-node3
    pool: prod-eu1/compute
    cost: "9600.00"
    in_service: 2026-03-01
    monthly: { hosting: "45.00", power: "18.00" }
    capacity: { cores: 16, memory_bytes: 68719476736, iops: 400000 }
    split: { cpu: "0.6", memory: "0.3", io: "0.1" }
    nodes: [node-3]
prices:
  - id: bucket-contract
    kind: storage
    scope: prod-eu1/bucket
    rate: "0.0207"
    unit: gb_month
    source: contract
    effective: 2026-10-01
mappings:
  - match: { tags: { loams-pool: "*" } }
    pool: "{tags.loams-pool}"
    kind: "{tags.loams-kind|cpu}"
  - match: { service: "Amazon Simple Storage Service", region: eu-central-1 }
    pool: prod-eu1/bucket
    kind: storage
engines:
  - match: { resource: "*/loams-house-worker*" }
    engine: house
classes:
  - engine: house
    class: heavy_scan
    when: "bytes_scanned >= 10737418240"
options:
  idle: show
  share_with_orgs: false
"#;

    #[test]
    fn example_parses() {
        let i = CostInputs::parse(EXAMPLE).expect("parse");
        assert_eq!(i.version, 7);
        assert_eq!(i.currency.as_str(), "USD");
        assert_eq!(i.assets[0].capacity.cores, Some(Amount::from_units(16)));
        let day = Day::parse("2026-10-28").expect("day");
        let bucket = PoolId::new("prod-eu1/bucket").expect("p");
        assert!(i.price(&bucket, ResourceKind::Storage, day).is_some());
        assert!(
            i.price(
                &bucket,
                ResourceKind::Storage,
                Day::parse("2026-09-30").expect("d")
            )
            .is_none()
        );
    }

    #[test]
    fn unknown_fields_refused() {
        let bad = EXAMPLE.replace("share_with_orgs", "share_with_everyone");
        assert!(CostInputs::parse(&bad).is_err());
    }

    #[test]
    fn split_must_sum_to_one() {
        let bad = EXAMPLE.replace("io: \"0.1\"", "io: \"0.2\"");
        let e = CostInputs::parse(&bad).expect_err("refused");
        assert!(e.to_string().contains("sum to exactly 1"), "{e}");
    }

    #[test]
    fn overlapping_prices_refused() {
        let bad = EXAMPLE.replace(
            "    effective: 2026-10-01\n",
            "    effective: 2026-10-01\n  - id: bucket-list\n    kind: storage\n    scope: prod-eu1/bucket\n    rate: \"0.023\"\n    unit: gb_month\n    source: list\n    effective: 2026-10-15\n",
        );
        let e = CostInputs::parse(&bad).expect_err("refused");
        assert!(e.to_string().contains("overlap"), "{e}");
    }

    #[test]
    fn currency_mismatch_refused() {
        let bad = EXAMPLE.replace(
            "assets:",
            "fx:\n  - { from: USD, to: EUR, rate: \"0.9\", day: 2026-10-28, source: ECB }\nassets:",
        );
        let e = CostInputs::parse(&bad).expect_err("refused");
        assert!(e.to_string().contains("reporting currency"), "{e}");
    }

    #[test]
    fn inputs_version_immutable() {
        let stored = StoredVersion {
            version: 7,
            digest: yaml_digest(EXAMPLE),
        };
        assert_eq!(apply(Some(&stored), EXAMPLE), Ok(ApplyOutcome::Unchanged));
        let changed = EXAMPLE.replace("9600.00", "9700.00");
        let e = apply(Some(&stored), &changed).expect_err("refused");
        assert!(
            e.to_string().contains("cost_inputs_version_immutable"),
            "{e}"
        );
        let older = EXAMPLE.replace("version: 7", "version: 6");
        assert!(apply(Some(&stored), &older).is_err());
        let newer = changed.replace("version: 7", "version: 8");
        assert!(matches!(
            apply(Some(&stored), &newer),
            Ok(ApplyOutcome::New(_))
        ));
    }

    #[test]
    fn price_unit_must_fit_kind() {
        let bad = EXAMPLE.replace("unit: gb_month", "unit: core_hour");
        let e = CostInputs::parse(&bad).expect_err("refused");
        assert!(e.to_string().contains("does not fit"), "{e}");
    }

    #[test]
    fn mapping_kind_checked_unless_template() {
        let bad = EXAMPLE.replace("    kind: storage\nengines", "    kind: disk\nengines");
        assert!(CostInputs::parse(&bad).is_err());
    }
}
