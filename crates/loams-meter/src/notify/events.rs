//! Threshold and lifecycle events (design §63 §15, D1129).
//!
//! The counter owner appends these to `_system/_meter` itself, so the House
//! keeps their history and the notifier reads them like any event. Their
//! ids are deterministic, so an event emitted again (after an owner
//! restart, or a failover that rebuilt the counters) is a duplicate that
//! every dedup layer drops (test `threshold_event_reemission_is_duplicate`).
//!
//! | Type | id |
//! |---|---|
//! | `io.loams.dev.meter.threshold.v1` | `<subject>/<feature>/<period start>/<threshold>` |
//! | `io.loams.dev.meter.access.changed.v1` | `<subject>/<feature>/<period start>/<seq>` |
//! | `io.loams.dev.meter.entitlement.reset.v1` | `<entitlement>/<period start>` |
//! | `io.loams.dev.meter.grant.v1` | `<grant>/<state>` |

use std::collections::HashMap;

use loams_meter_core::{Amount, EntitlementId, GrantId, Period, UtcNanos};
use serde_json::{Map, Value, json};

use crate::owner::CounterKey;

pub const THRESHOLD: &str = "io.loams.dev.meter.threshold.v1";
pub const ACCESS_CHANGED: &str = "io.loams.dev.meter.access.changed.v1";
pub const ENTITLEMENT_RESET: &str = "io.loams.dev.meter.entitlement.reset.v1";
pub const GRANT: &str = "io.loams.dev.meter.grant.v1";

/// The thresholds every metered entitlement notifies at, in percent of the
/// period's total available (§15).
pub const DEFAULT_THRESHOLDS: &[u32] = &[80, 100];

/// One threshold or lifecycle event.
#[derive(Clone, Debug, PartialEq)]
pub struct MeterEvent {
    pub id: String,
    pub r#type: String,
    pub source: String,
    pub subject: String,
    pub time: UtcNanos,
    pub data: Map<String, Value>,
}

impl MeterEvent {
    /// The event as a structured-mode CloudEvent (the webhook body).
    pub fn to_cloudevent_json(&self) -> Value {
        json!({
            "specversion": "1.0",
            "id": self.id,
            "source": self.source,
            "type": self.r#type,
            "subject": self.subject,
            "time": self.time.to_rfc3339(),
            "datacontenttype": "application/json",
            "data": Value::Object(self.data.clone()),
        })
    }
}

fn period_tag(p: UtcNanos) -> String {
    p.to_rfc3339()
}

/// `<subject>/<feature>/<period start>/<threshold>`.
pub fn threshold_id(key: &CounterKey, threshold: &str) -> String {
    format!(
        "{}/{}/{}/{threshold}",
        key.subject,
        key.feature,
        period_tag(key.period_start)
    )
}

/// What a threshold is a percentage of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Basis {
    /// The period's total available.
    Allowance,
    /// The spend cap (overage spend, in money micros).
    SpendCap,
}

/// Remembers which thresholds each counter has crossed this period, so a
/// running owner emits each once; a restarted one may emit them again, and
/// their ids make that a duplicate.
#[derive(Debug, Clone)]
pub struct ThresholdTracker {
    source: String,
    thresholds: Vec<u32>,
    crossed: HashMap<(CounterKey, Basis), u32>,
    access: HashMap<CounterKey, (bool, u64)>,
}

impl ThresholdTracker {
    /// `source` is the event source, such as `loams://meter/<instance>`.
    pub fn new(source: impl Into<String>, thresholds: &[u32]) -> Self {
        let mut t: Vec<u32> = thresholds.iter().copied().filter(|t| *t > 0).collect();
        t.sort_unstable();
        t.dedup();
        ThresholdTracker {
            source: source.into(),
            thresholds: t,
            crossed: HashMap::new(),
            access: HashMap::new(),
        }
    }

    fn event(
        &self,
        id: String,
        ty: &str,
        key: &CounterKey,
        now: UtcNanos,
        data: Map<String, Value>,
    ) -> MeterEvent {
        MeterEvent {
            id,
            r#type: ty.to_string(),
            source: self.source.clone(),
            subject: key.subject.to_string(),
            time: now,
            data,
        }
    }

    /// The threshold events `usage` of `total` crosses for `key` in
    /// `period`, lowest first; each at most once per tracker.
    pub fn observe(
        &mut self,
        key: &CounterKey,
        period: Period,
        usage: Amount,
        total: Amount,
        now: UtcNanos,
    ) -> Vec<MeterEvent> {
        self.observe_basis(
            key,
            period,
            Basis::Allowance,
            usage.raw(),
            total.raw(),
            now,
            |d| {
                d.insert("usage".into(), Value::String(usage.to_string()));
                d.insert("total_available".into(), Value::String(total.to_string()));
            },
        )
    }

    /// The spend-cap thresholds `spent_micros` of `cap_micros` crosses.
    pub fn observe_spend(
        &mut self,
        key: &CounterKey,
        period: Period,
        spent_micros: i64,
        cap_micros: i64,
        now: UtcNanos,
    ) -> Vec<MeterEvent> {
        self.observe_basis(
            key,
            period,
            Basis::SpendCap,
            i128::from(spent_micros),
            i128::from(cap_micros),
            now,
            |d| {
                d.insert("spent_micros".into(), json!(spent_micros));
                d.insert("spend_cap_micros".into(), json!(cap_micros));
            },
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn observe_basis(
        &mut self,
        key: &CounterKey,
        period: Period,
        basis: Basis,
        used: i128,
        total: i128,
        now: UtcNanos,
        fill: impl Fn(&mut Map<String, Value>),
    ) -> Vec<MeterEvent> {
        if total <= 0 {
            return Vec::new();
        }
        let done = self
            .crossed
            .get(&(key.clone(), basis))
            .copied()
            .unwrap_or(0);
        let mut out = Vec::new();
        let mut highest = done;
        for t in &self.thresholds {
            if *t <= done || used.saturating_mul(100) < total.saturating_mul(i128::from(*t)) {
                continue;
            }
            highest = highest.max(*t);
            let tag = match basis {
                Basis::Allowance => t.to_string(),
                Basis::SpendCap => format!("spend_cap_{t}"),
            };
            let mut data = Map::new();
            data.insert("feature".into(), Value::String(key.feature.to_string()));
            data.insert("threshold".into(), json!(t));
            data.insert(
                "basis".into(),
                Value::String(match basis {
                    Basis::Allowance => "allowance".into(),
                    Basis::SpendCap => "spend_cap".into(),
                }),
            );
            data.insert(
                "period_start".into(),
                Value::String(period.start.to_rfc3339()),
            );
            data.insert("period_end".into(), Value::String(period.end.to_rfc3339()));
            fill(&mut data);
            out.push(self.event(threshold_id(key, &tag), THRESHOLD, key, now, data));
        }
        if highest > done {
            self.crossed.insert((key.clone(), basis), highest);
        }
        out
    }

    /// An `access.changed` event when `has_access` flips for `key`.
    pub fn access(
        &mut self,
        key: &CounterKey,
        has_access: bool,
        reason: Option<&str>,
        now: UtcNanos,
    ) -> Option<MeterEvent> {
        let (last, seq) = self.access.get(key).copied().unwrap_or((true, 0));
        if last == has_access {
            return None;
        }
        let seq = seq + 1;
        self.access.insert(key.clone(), (has_access, seq));
        let mut data = Map::new();
        data.insert("feature".into(), Value::String(key.feature.to_string()));
        data.insert("has_access".into(), Value::Bool(has_access));
        if let Some(r) = reason {
            data.insert("reason".into(), Value::String(r.to_string()));
        }
        let id = format!(
            "{}/{}/{}/{seq}",
            key.subject,
            key.feature,
            period_tag(key.period_start)
        );
        Some(self.event(id, ACCESS_CHANGED, key, now, data))
    }

    /// Forgets the counters of ended periods.
    pub fn expire(&mut self, before: UtcNanos) {
        self.crossed.retain(|(k, _), _| k.period_start >= before);
        self.access.retain(|k, _| k.period_start >= before);
    }
}

/// `entitlement.reset`: `<entitlement>/<period start>`.
pub fn reset_event(
    source: &str,
    subject: &str,
    entitlement: &EntitlementId,
    period: Period,
    now: UtcNanos,
) -> MeterEvent {
    let mut data = Map::new();
    data.insert("entitlement".into(), Value::String(entitlement.to_string()));
    data.insert(
        "period_start".into(),
        Value::String(period.start.to_rfc3339()),
    );
    data.insert("period_end".into(), Value::String(period.end.to_rfc3339()));
    MeterEvent {
        id: format!("{entitlement}/{}", period_tag(period.start)),
        r#type: ENTITLEMENT_RESET.into(),
        source: source.into(),
        subject: subject.into(),
        time: now,
        data,
    }
}

/// The states a grant event reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrantState {
    Created,
    Voided,
    Expired,
}

/// `grant`: `<grant>/<state>`.
pub fn grant_event(
    source: &str,
    subject: &str,
    grant: &GrantId,
    state: GrantState,
    amount: Amount,
    now: UtcNanos,
) -> MeterEvent {
    let state = match state {
        GrantState::Created => "created",
        GrantState::Voided => "voided",
        GrantState::Expired => "expired",
    };
    let mut data = Map::new();
    data.insert("grant".into(), Value::String(grant.to_string()));
    data.insert("state".into(), Value::String(state.into()));
    data.insert("amount".into(), Value::String(amount.to_string()));
    MeterEvent {
        id: format!("{grant}/{state}"),
        r#type: GRANT.into(),
        source: source.into(),
        subject: subject.into(),
        time: now,
        data,
    }
}
