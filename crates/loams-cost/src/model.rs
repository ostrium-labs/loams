//! The model (design §84 §5): cost sources → pools → resource kinds →
//! drivers → consumers, per UTC day.

use std::collections::BTreeSet;
use std::fmt;

use loams_meter_core::Amount;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{Basis, CostError, Flag};

/// What a pool's cost buys, and so which driver splits it (§84 §5.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Cpu,
    Memory,
    Storage,
    NetworkEgress,
    NetworkIngress,
    NetworkInternal,
    Io,
    Gpu,
    /// No driver: support, licences, account-level charges.
    Fixed,
}

impl ResourceKind {
    pub const ALL: [ResourceKind; 9] = [
        ResourceKind::Cpu,
        ResourceKind::Memory,
        ResourceKind::Storage,
        ResourceKind::NetworkEgress,
        ResourceKind::NetworkIngress,
        ResourceKind::NetworkInternal,
        ResourceKind::Io,
        ResourceKind::Gpu,
        ResourceKind::Fixed,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            ResourceKind::Cpu => "cpu",
            ResourceKind::Memory => "memory",
            ResourceKind::Storage => "storage",
            ResourceKind::NetworkEgress => "network_egress",
            ResourceKind::NetworkIngress => "network_ingress",
            ResourceKind::NetworkInternal => "network_internal",
            ResourceKind::Io => "io",
            ResourceKind::Gpu => "gpu",
            ResourceKind::Fixed => "fixed",
        }
    }

    /// The driver's unit (FOCUS `UsageUnit` in the export).
    pub const fn driver_unit(self) -> &'static str {
        match self {
            ResourceKind::Cpu => "core_second",
            ResourceKind::Memory | ResourceKind::Storage => "byte_second",
            ResourceKind::NetworkEgress
            | ResourceKind::NetworkIngress
            | ResourceKind::NetworkInternal => "byte",
            ResourceKind::Io => "operation",
            ResourceKind::Gpu => "device_second",
            ResourceKind::Fixed => "none",
        }
    }

    pub fn parse(s: &str) -> Result<ResourceKind, CostError> {
        ResourceKind::ALL
            .into_iter()
            .find(|k| k.as_str() == s)
            .ok_or_else(|| CostError::invalid("kind", format!("unknown resource kind {s:?}")))
    }
}

impl fmt::Display for ResourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A UTC calendar day, `YYYY-MM-DD`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Day(time::Date);

impl Day {
    pub fn new(year: i32, month: u8, day: u8) -> Result<Day, CostError> {
        let month = time::Month::try_from(month)
            .map_err(|_| CostError::invalid("day", format!("bad month {month}")))?;
        time::Date::from_calendar_date(year, month, day)
            .map(Day)
            .map_err(|e| CostError::invalid("day", e.to_string()))
    }

    pub fn parse(s: &str) -> Result<Day, CostError> {
        let bad = || CostError::invalid("day", format!("{s:?} is not YYYY-MM-DD"));
        let mut it = s.splitn(3, '-');
        let (Some(y), Some(m), Some(d)) = (it.next(), it.next(), it.next()) else {
            return Err(bad());
        };
        if y.len() != 4 || m.len() != 2 || d.len() != 2 {
            return Err(bad());
        }
        Day::new(
            y.parse().map_err(|_| bad())?,
            m.parse().map_err(|_| bad())?,
            d.parse().map_err(|_| bad())?,
        )
    }

    pub fn from_date(date: time::Date) -> Day {
        Day(date)
    }

    pub fn date(self) -> time::Date {
        self.0
    }

    pub fn next(self) -> Option<Day> {
        self.0.next_day().map(Day)
    }

    /// Seconds since the Unix epoch at the day's start (00:00 UTC).
    pub fn start_unix(self) -> i64 {
        self.0.midnight().assume_utc().unix_timestamp()
    }

    /// The UTC day containing `unix` seconds.
    pub fn of_unix(unix: i64) -> Result<Day, CostError> {
        time::OffsetDateTime::from_unix_timestamp(unix)
            .map(|t| Day(t.date()))
            .map_err(|e| CostError::invalid("time", e.to_string()))
    }

    /// Days in this day's month.
    pub fn days_in_month(self) -> u8 {
        self.0.month().length(self.0.year())
    }
}

impl fmt::Display for Day {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02}",
            self.0.year(),
            u8::from(self.0.month()),
            self.0.day()
        )
    }
}

impl Serialize for Day {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Day {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Day::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// Checks a name used as a pool, engine, tenant, project or class: 1..=128
/// characters from `[A-Za-z0-9._~/:@-]` (no spaces, quotes or `|`).
fn check_name(field: &'static str, s: &str) -> Result<(), CostError> {
    if s.is_empty() || s.len() > 128 {
        return Err(CostError::invalid(field, "must be 1 to 128 characters"));
    }
    if let Some(c) = s
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || "._~/:@-".contains(*c)))
    {
        return Err(CostError::invalid(
            field,
            format!("{s:?} contains {c:?}; allowed are letters, digits and ._~/:@-"),
        ));
    }
    Ok(())
}

/// A pool: infrastructure whose cost the same consumers share.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct PoolId(String);

impl PoolId {
    pub fn new(s: impl Into<String>) -> Result<PoolId, CostError> {
        let s = s.into();
        check_name("pool", &s)?;
        Ok(PoolId(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for PoolId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        PoolId::new(String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for PoolId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The value of a consumer-key part that does not apply.
pub const NONE: &str = "_";

/// Who used a resource (§84 §5.3): engine, tenant (org), project
/// (namespace) and query class. A part that does not apply is `_`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ConsumerKey {
    pub engine: String,
    pub tenant: String,
    pub project: String,
    pub query_class: String,
}

/// Engine names a consumer may not use: they are the pseudo-consumers.
pub const RESERVED_ENGINES: [&str; 3] = ["idle", "unattributed", "shared"];

impl ConsumerKey {
    pub fn new(
        engine: impl Into<String>,
        tenant: impl Into<String>,
        project: impl Into<String>,
        query_class: impl Into<String>,
    ) -> Result<ConsumerKey, CostError> {
        let k = ConsumerKey {
            engine: engine.into(),
            tenant: tenant.into(),
            project: project.into(),
            query_class: query_class.into(),
        };
        k.validate()?;
        Ok(k)
    }

    pub fn validate(&self) -> Result<(), CostError> {
        check_name("engine", &self.engine)?;
        check_name("tenant", &self.tenant)?;
        check_name("project", &self.project)?;
        check_name("query_class", &self.query_class)?;
        if RESERVED_ENGINES.contains(&self.engine.as_str()) {
            return Err(CostError::invalid(
                "engine",
                format!("{:?} is a pseudo-consumer", self.engine),
            ));
        }
        Ok(())
    }
}

impl fmt::Display for ConsumerKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}/{}/{}/{}",
            self.engine, self.tenant, self.project, self.query_class
        )
    }
}

/// An allocation's consumer: real work, or a pseudo-consumer that keeps
/// cost visible instead of spreading it (D2364).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Consumer {
    Work(ConsumerKey),
    /// Capacity no consumer used.
    Idle,
    /// Cost with no driver usage at all.
    Unattributed,
    /// `fixed` pools and charges with no pool.
    Shared,
}

impl Consumer {
    /// The engine column: the work's engine or the pseudo-consumer's name.
    pub fn engine(&self) -> &str {
        match self {
            Consumer::Work(k) => &k.engine,
            Consumer::Idle => "idle",
            Consumer::Unattributed => "unattributed",
            Consumer::Shared => "shared",
        }
    }

    pub fn work(&self) -> Option<&ConsumerKey> {
        match self {
            Consumer::Work(k) => Some(k),
            _ => None,
        }
    }
}

impl fmt::Display for Consumer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Consumer::Work(k) => k.fmt(f),
            other => f.write_str(other.engine()),
        }
    }
}

/// Where a cost source came from, for the trace (§84 §10.2).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum SourceRef {
    /// An asset's amortised day, from a cost-input version.
    Asset { asset: String, inputs_version: u64 },
    /// An asset's declared monthly cost (hosting, power, …) for the day.
    AssetMonthly {
        asset: String,
        item: String,
        inputs_version: u64,
    },
    /// A bill line's share of the day.
    BillLine { import: String, line: u64 },
    /// A bill import's lines for one (day, pool, kind), summed (the role
    /// keeps bills aggregated; the file digest names the file).
    BillImport { import: String, lines: u64 },
    /// A price-table rate times a declared quantity.
    Price { price: String, inputs_version: u64 },
    /// Anything else, named.
    Other { note: String },
}

impl fmt::Display for SourceRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceRef::Asset {
                asset,
                inputs_version,
            } => write!(f, "asset:{asset}@v{inputs_version}"),
            SourceRef::AssetMonthly {
                asset,
                item,
                inputs_version,
            } => write!(f, "asset:{asset}/{item}@v{inputs_version}"),
            SourceRef::BillLine { import, line } => write!(f, "bill:{import}#{line}"),
            SourceRef::BillImport { import, lines } => write!(f, "bill:{import} ({lines} lines)"),
            SourceRef::Price {
                price,
                inputs_version,
            } => write!(f, "price:{price}@v{inputs_version}"),
            SourceRef::Other { note } => write!(f, "other:{note}"),
        }
    }
}

/// One priced input for one pool, kind and day. `amount` is in the
/// reporting currency (negative for credits); `basis` is `billed`,
/// `declared` or `estimated`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostSource {
    pub day: Day,
    pub pool: PoolId,
    pub kind: ResourceKind,
    pub amount: Amount,
    pub basis: Basis,
    pub source: SourceRef,
    /// Facts the lines from this source must carry (a default input, a
    /// default split).
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub flags: BTreeSet<Flag>,
}

/// What a pool could deliver of a kind in a day, in the driver unit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capacity {
    pub day: Day,
    pub pool: PoolId,
    pub kind: ResourceKind,
    pub quantity: Amount,
    pub basis: Basis,
}

/// A consumer's driver usage of a pool's kind in a day, in the driver unit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub day: Day,
    pub pool: PoolId,
    pub kind: ResourceKind,
    pub consumer: ConsumerKey,
    pub quantity: Amount,
    pub basis: Basis,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_round_trips() {
        let d = Day::parse("2026-10-28").expect("day");
        assert_eq!(d.to_string(), "2026-10-28");
        assert_eq!(d.next().expect("next").to_string(), "2026-10-29");
        assert_eq!(d.days_in_month(), 31);
        assert_eq!(Day::of_unix(d.start_unix()).expect("day"), d);
        assert!(Day::parse("2026-13-01").is_err());
        assert!(Day::parse("2026-1-01").is_err());
    }

    #[test]
    fn names_are_checked() {
        assert!(PoolId::new("prod-eu1/compute").is_ok());
        assert!(PoolId::new("").is_err());
        assert!(PoolId::new("a b").is_err());
        assert!(ConsumerKey::new("house", "org_42", "7", "scan").is_ok());
        assert!(ConsumerKey::new("idle", "org_42", "7", "scan").is_err());
        assert!(ConsumerKey::new("house", "org|42", "7", "scan").is_err());
    }

    #[test]
    fn kinds_parse() {
        for k in ResourceKind::ALL {
            assert_eq!(ResourceKind::parse(k.as_str()).expect("kind"), k);
        }
        assert!(ResourceKind::parse("disk").is_err());
    }
}
