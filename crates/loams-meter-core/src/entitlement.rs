//! Features, entitlements, grants and balances (design §63 §9, D1124):
//! OpenMeter's model, meter → feature → entitlement → grant.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::{Month, OffsetDateTime};

use crate::amount::{Amount, Money};
use crate::ids::{EntitlementId, FeatureKey, GrantId, PlanId, Slug, SubjectKey, UtcNanos};

/// A feature's unit.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Unit {
    Count,
    Ms,
    /// Compute units × milliseconds (Postgres compute).
    CuMs,
    Bytes,
    ByteHours,
    Tokens,
    Requests,
    Other(String),
}

impl Unit {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Count => "count",
            Self::Ms => "ms",
            Self::CuMs => "cu_ms",
            Self::Bytes => "bytes",
            Self::ByteHours => "byte_hours",
            Self::Tokens => "tokens",
            Self::Requests => "requests",
            Self::Other(s) => s,
        }
    }

    pub fn parse(s: &str) -> Unit {
        match s {
            "count" => Self::Count,
            "ms" => Self::Ms,
            "cu_ms" => Self::CuMs,
            "bytes" => Self::Bytes,
            "byte_hours" => Self::ByteHours,
            "tokens" => Self::Tokens,
            "requests" => Self::Requests,
            other => Self::Other(other.to_owned()),
        }
    }
}

impl fmt::Display for Unit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for Unit {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Unit {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(Unit::parse(&s))
    }
}

/// What a customer is entitled to; optionally backed by a meter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feature {
    pub key: FeatureKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meter: Option<Slug>,
    /// Group-by values that select part of a meter (`model: gpt-x`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub filter: BTreeMap<String, String>,
    pub unit: Unit,
    #[serde(default)]
    pub archived: bool,
}

/// The length of a usage period or a recurrence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interval {
    Hour,
    Day,
    Week,
    Month,
    Year,
}

impl Interval {
    fn fixed_nanos(self) -> Option<i64> {
        match self {
            Self::Hour => Some(UtcNanos::HOUR),
            Self::Day => Some(UtcNanos::DAY),
            Self::Week => Some(UtcNanos::WEEK),
            Self::Month | Self::Year => None,
        }
    }

    fn months(self) -> i64 {
        match self {
            Self::Year => 12,
            _ => 1,
        }
    }
}

/// `[start, end)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Period {
    pub start: UtcNanos,
    pub end: UtcNanos,
}

impl Period {
    pub fn contains(&self, t: UtcNanos) -> bool {
        self.start <= t && t < self.end
    }
}

/// A usage period: an interval repeating from an anchor (OpenMeter's
/// `usagePeriod`). Calendar intervals keep the anchor's day of month and
/// time of day, clamped to the month's last day (31 Jan → 28 Feb → 31 Mar).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UsagePeriod {
    pub interval: Interval,
    pub anchor: UtcNanos,
}

impl UsagePeriod {
    /// The start of period `n` (period 0 starts at the anchor; negative `n`
    /// are before it).
    pub fn start_of(&self, n: i64) -> UtcNanos {
        match self.interval.fixed_nanos() {
            Some(len) => UtcNanos(self.anchor.0.saturating_add(n.saturating_mul(len))),
            None => add_months(self.anchor, n.saturating_mul(self.interval.months())),
        }
    }

    /// The index of the period holding `at`.
    pub fn index_at(&self, at: UtcNanos) -> i64 {
        if let Some(len) = self.interval.fixed_nanos() {
            return (at.0 - self.anchor.0).div_euclid(len);
        }
        let (Some(a), Some(t)) = (self.anchor.to_datetime(), at.to_datetime()) else {
            return 0;
        };
        let months = (i64::from(t.year()) - i64::from(a.year())) * 12
            + (i64::from(u8::from(t.month())) - i64::from(u8::from(a.month())));
        let mut n = months.div_euclid(self.interval.months());
        while self.start_of(n) > at {
            n -= 1;
        }
        while self.start_of(n + 1) <= at {
            n += 1;
        }
        n
    }

    pub fn period(&self, n: i64) -> Period {
        Period {
            start: self.start_of(n),
            end: self.start_of(n + 1),
        }
    }

    /// The period holding `at`.
    pub fn period_at(&self, at: UtcNanos) -> Period {
        self.period(self.index_at(at))
    }

    /// The period after `p`.
    pub fn next(&self, p: Period) -> Period {
        self.period(self.index_at(p.start) + 1)
    }
}

/// `t` plus `months` calendar months, the day clamped to the target month.
pub(crate) fn add_months(t: UtcNanos, months: i64) -> UtcNanos {
    let Some(dt) = t.to_datetime() else {
        return t;
    };
    let total = i64::from(dt.year()) * 12 + i64::from(u8::from(dt.month())) - 1 + months;
    let year = total.div_euclid(12);
    let month0 = total.rem_euclid(12);
    let (Ok(year), Ok(month)) = (
        i32::try_from(year),
        Month::try_from(u8::try_from(month0 + 1).unwrap_or(1)),
    ) else {
        return t;
    };
    let last = days_in_month(month, year);
    let day = dt.day().min(last);
    let Ok(date) = time::Date::from_calendar_date(year, month, day) else {
        return t;
    };
    let moved: OffsetDateTime = dt.replace_date(date);
    UtcNanos::from_datetime(moved).unwrap_or(t)
}

fn days_in_month(month: Month, year: i32) -> u8 {
    match month {
        Month::April | Month::June | Month::September | Month::November => 30,
        Month::February => {
            let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
            if leap { 29 } else { 28 }
        }
        _ => 31,
    }
}

/// What happens when the balance reaches zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Mode {
    /// Refuse when the balance reaches zero.
    HardLimit,
    /// Allow past zero; notify; overage is not billed (a grace allowance).
    SoftLimit,
    /// Allow past zero; overage is billed at `overage_price` per unit, up to
    /// `spend_cap`.
    OverageAllowed {
        overage_price: Money,
        spend_cap: Money,
    },
}

/// The three kinds of entitlement.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EntitlementKind {
    Metered {
        usage_period: UsagePeriod,
        mode: Mode,
        /// The balance each period starts from (a recurring grant made by
        /// the entitlement itself).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        issue_after_reset: Option<Amount>,
        #[serde(default)]
        preserve_overage_at_reset: bool,
    },
    /// On or off. The value lives on the entitlement (D2165).
    Boolean { enabled: bool },
    /// A configuration value, such as a maximum branch count.
    Static { config: serde_json::Value },
}

/// Where an entitlement came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum EntitlementSource {
    Plan { plan: PlanId, version: u32 },
    Manual { by: String },
}

/// A subject's right to a feature.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entitlement {
    pub id: EntitlementId,
    pub subject: SubjectKey,
    pub feature: FeatureKey,
    pub kind: EntitlementKind,
    pub created_at: UtcNanos,
    pub source: EntitlementSource,
}

impl Entitlement {
    pub fn usage_period(&self) -> Option<UsagePeriod> {
        match &self.kind {
            EntitlementKind::Metered { usage_period, .. } => Some(*usage_period),
            _ => None,
        }
    }

    pub fn mode(&self) -> Option<Mode> {
        match &self.kind {
            EntitlementKind::Metered { mode, .. } => Some(*mode),
            _ => None,
        }
    }
}

/// The unit of a grant's expiry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpiryUnit {
    Hour,
    Day,
    Week,
    Month,
    Year,
}

/// A grant's lifetime from `effective_at`: `count` × `duration`, 1..=1000.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Expiry {
    pub duration: ExpiryUnit,
    pub count: u16,
}

impl Expiry {
    pub const MAX_COUNT: u16 = 1000;

    pub fn after(&self, from: UtcNanos) -> UtcNanos {
        let n = i64::from(self.count);
        match self.duration {
            ExpiryUnit::Hour => from.add_nanos(n * UtcNanos::HOUR),
            ExpiryUnit::Day => from.add_nanos(n * UtcNanos::DAY),
            ExpiryUnit::Week => from.add_nanos(n * UtcNanos::WEEK),
            ExpiryUnit::Month => add_months(from, n),
            ExpiryUnit::Year => add_months(from, 12 * n),
        }
    }
}

/// What a grant keeps at a reset: `min(max, max(balance, min))`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rollover {
    pub max: Amount,
    pub min: Amount,
}

impl Rollover {
    pub fn apply(&self, balance_before_reset: Amount) -> Amount {
        balance_before_reset.max(self.min).min(self.max)
    }
}

/// The kind of a grant. At equal priority and expiry, promotional credit
/// burns before included, and paid credit last (§9.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GrantKind {
    Promotional,
    Included,
    Prepaid { paid: Money },
}

impl GrantKind {
    pub fn burn_rank(&self) -> u8 {
        match self {
            Self::Promotional => 0,
            Self::Included => 1,
            Self::Prepaid { .. } => 2,
        }
    }
}

/// Why a grant or entitlement definition was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GrantError {
    #[error("grant amount must be positive")]
    Amount,
    #[error("grant priority must be 1..=255")]
    Priority,
    #[error("grant expiry count must be 1..=1000")]
    ExpiryCount,
    #[error("rollover min must not exceed max, and neither may be negative")]
    Rollover,
}

/// An amount added to a metered entitlement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub id: GrantId,
    pub entitlement: EntitlementId,
    /// In the feature's unit.
    pub amount: Amount,
    /// 1..=255; lower burns first.
    pub priority: u8,
    /// Ceiled to the minute (see [`Grant::normalized`]).
    pub effective_at: UtcNanos,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<Expiry>,
    #[serde(default)]
    pub rollover: Rollover,
    /// When set, the grant is re-issued at each period of this recurrence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence: Option<UsagePeriod>,
    pub kind: GrantKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voided_at: Option<UtcNanos>,
    pub created_at: UtcNanos,
}

impl Grant {
    pub fn validate(&self) -> Result<(), GrantError> {
        if !self.amount.is_positive() {
            return Err(GrantError::Amount);
        }
        if self.priority == 0 {
            return Err(GrantError::Priority);
        }
        if let Some(e) = self.expires
            && !(1..=Expiry::MAX_COUNT).contains(&e.count)
        {
            return Err(GrantError::ExpiryCount);
        }
        if self.rollover.min.is_negative()
            || self.rollover.max.is_negative()
            || self.rollover.min > self.rollover.max
        {
            return Err(GrantError::Rollover);
        }
        Ok(())
    }

    /// The grant with `effective_at` ceiled to the minute (OpenMeter's rule).
    pub fn normalized(mut self) -> Grant {
        self.effective_at = self.effective_at.ceil_minute();
        self
    }

    pub fn expires_at(&self) -> Option<UtcNanos> {
        self.expires.map(|e| e.after(self.effective_at))
    }

    /// True while the grant may be burned: effective, not expired, not voided.
    pub fn is_live(&self, at: UtcNanos) -> bool {
        self.effective_at <= at
            && self.expires_at().is_none_or(|e| at < e)
            && self.voided_at.is_none_or(|v| at < v)
    }

    /// Sort key of the burn-down order: priority, nearest expiry (none
    /// last), kind (promotional, included, prepaid), oldest created, id.
    pub fn burn_order_key(&self) -> (u8, i64, u8, i64, &str) {
        (
            self.priority,
            self.expires_at().map_or(i64::MAX, |e| e.0),
            self.kind.burn_rank(),
            self.created_at.0,
            self.id.as_str(),
        )
    }
}

/// Where a balance's usage came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum AsOf {
    /// A counter owner's view, at its epoch and stream offset.
    Counter { owner_epoch: u64, offset: u64 },
    /// The House's closed windows at an Iceberg snapshot.
    House { snapshot: i64 },
    /// A replay of given usage (tests, history).
    Replay,
}

/// What is left of a metered entitlement (OpenMeter's `EntitlementValue`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Balance {
    pub entitlement: EntitlementId,
    pub period: Period,
    pub has_access: bool,
    /// ≥ 0.
    pub balance: Amount,
    /// Usage in the period.
    pub usage: Amount,
    /// Usage past every grant in the period.
    pub overage: Amount,
    /// What the period started with plus grants that became effective in it.
    pub total_available: Amount,
    /// Remaining balance per live grant, in burn order.
    pub grants: Vec<(GrantId, Amount)>,
    /// True when every grant that funded the period is prepaid; such an
    /// entitlement stops at zero whatever its mode (§9.4).
    pub prepaid_only: bool,
    pub as_of: AsOf,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amount::Currency;

    fn t(s: &str) -> UtcNanos {
        UtcNanos::from_rfc3339(s).expect("time")
    }

    #[test]
    fn monthly_periods_clamp_and_return_to_the_anchor_day() {
        let p = UsagePeriod {
            interval: Interval::Month,
            anchor: t("2026-01-31T10:00:00Z"),
        };
        assert_eq!(p.start_of(1), t("2026-02-28T10:00:00Z"));
        assert_eq!(p.start_of(2), t("2026-03-31T10:00:00Z"));
        assert_eq!(p.start_of(-2), t("2025-11-30T10:00:00Z"));
        let at = t("2026-03-01T00:00:00Z");
        assert_eq!(
            p.period_at(at),
            Period {
                start: t("2026-02-28T10:00:00Z"),
                end: t("2026-03-31T10:00:00Z")
            }
        );
        assert_eq!(p.index_at(t("2026-01-31T09:59:59Z")), -1);
        assert_eq!(p.index_at(t("2026-01-31T10:00:00Z")), 0);
    }

    #[test]
    fn fixed_periods() {
        let p = UsagePeriod {
            interval: Interval::Day,
            anchor: t("2026-10-01T00:00:00Z"),
        };
        let period = p.period_at(t("2026-10-03T12:00:00Z"));
        assert_eq!(period.start, t("2026-10-03T00:00:00Z"));
        assert_eq!(p.next(period).start, t("2026-10-04T00:00:00Z"));
        assert_eq!(p.index_at(t("2026-09-30T23:00:00Z")), -1);
    }

    #[test]
    fn yearly_periods() {
        let p = UsagePeriod {
            interval: Interval::Year,
            anchor: t("2024-02-29T00:00:00Z"),
        };
        assert_eq!(p.start_of(1), t("2025-02-28T00:00:00Z"));
        assert_eq!(p.start_of(4), t("2028-02-29T00:00:00Z"));
        assert_eq!(p.index_at(t("2026-06-01T00:00:00Z")), 2);
    }

    fn grant(id: &str, priority: u8, kind: GrantKind) -> Grant {
        Grant {
            id: GrantId::new(id).expect("id"),
            entitlement: EntitlementId::new("e1").expect("id"),
            amount: Amount::from_units(100),
            priority,
            effective_at: UtcNanos(0),
            expires: None,
            rollover: Rollover::default(),
            recurrence: None,
            kind,
            voided_at: None,
            created_at: UtcNanos(0),
        }
    }

    #[test]
    fn burn_order() {
        let paid = GrantKind::Prepaid {
            paid: Money::new(5_000_000, Currency::USD),
        };
        let mut gs = [
            grant("paid", 5, paid),
            grant("incl", 5, GrantKind::Included),
            grant("promo", 5, GrantKind::Promotional),
            grant("first", 1, paid),
            {
                let mut g = grant("expiring", 5, paid);
                g.expires = Some(Expiry {
                    duration: ExpiryUnit::Day,
                    count: 1,
                });
                g
            },
        ];
        gs.sort_by(|a, b| a.burn_order_key().cmp(&b.burn_order_key()));
        let order: Vec<&str> = gs.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(order, ["first", "expiring", "promo", "incl", "paid"]);
    }

    #[test]
    fn grant_rules() {
        let mut g = grant("g", 1, GrantKind::Included);
        assert!(g.validate().is_ok());
        g.priority = 0;
        assert_eq!(g.validate(), Err(GrantError::Priority));
        g.priority = 1;
        g.rollover = Rollover {
            max: Amount::from_units(1),
            min: Amount::from_units(2),
        };
        assert_eq!(g.validate(), Err(GrantError::Rollover));
        g.rollover = Rollover::default();
        g.expires = Some(Expiry {
            duration: ExpiryUnit::Month,
            count: 0,
        });
        assert_eq!(g.validate(), Err(GrantError::ExpiryCount));
        g.effective_at = UtcNanos(1);
        assert_eq!(g.normalized().effective_at, UtcNanos(UtcNanos::MINUTE));
    }

    #[test]
    fn rollover_formula() {
        let r = Rollover {
            max: Amount::from_units(50),
            min: Amount::from_units(10),
        };
        assert_eq!(r.apply(Amount::from_units(70)), Amount::from_units(50));
        assert_eq!(r.apply(Amount::from_units(30)), Amount::from_units(30));
        assert_eq!(r.apply(Amount::from_units(0)), Amount::from_units(10));
    }

    #[test]
    fn plan_yaml_shapes_deserialize() {
        let kind: EntitlementKind = serde_json::from_value(serde_json::json!({
            "type": "metered",
            "usage_period": {"interval": "month", "anchor": 0},
            "mode": {"mode": "overage_allowed",
                     "overage_price": {"micros": 20, "currency": "USD"},
                     "spend_cap": {"micros": 50000000, "currency": "USD"}},
            "issue_after_reset": "36000000"
        }))
        .expect("metered");
        assert!(matches!(
            kind,
            EntitlementKind::Metered {
                mode: Mode::OverageAllowed { .. },
                ..
            }
        ));
        let b: EntitlementKind =
            serde_json::from_value(serde_json::json!({"type": "boolean", "enabled": false}))
                .expect("boolean");
        assert_eq!(b, EntitlementKind::Boolean { enabled: false });
    }
}
