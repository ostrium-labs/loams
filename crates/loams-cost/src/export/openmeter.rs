//! OpenMeter-compatible cost events (design §84 §13.1, D2374).
//!
//! Each allocation line becomes a CloudEvent `io.loams.dev.cost.allocation.v1`
//! that OpenMeter's `POST /api/v1/events` accepts: JSON data, a subject
//! (the tenant, or `_operator` for pseudo-consumers), and an integer
//! `cost_micros` so OpenMeter's `SUM` stays exact. Micros are split per
//! pool-day by largest remainder, so a pool-day's micros sum to its cost
//! rounded to micros.
//!
//! A recompute of a day that was already sent is not re-sent (OpenMeter
//! would add it): [`adjustments`] emits `io.loams.dev.cost.adjustment.v1`
//! events with the difference, as §63 Q963 does for late usage.

use std::collections::BTreeMap;

use loams_meter_core::Amount;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{by_pool_day, split_by_amount};
use crate::{Consumer, CostError, Line, NONE, trace_id};

pub const ALLOCATION_TYPE: &str = "io.loams.dev.cost.allocation.v1";
pub const ADJUSTMENT_TYPE: &str = "io.loams.dev.cost.adjustment.v1";

/// The subject of pseudo-consumer lines.
pub const OPERATOR_SUBJECT: &str = "_operator";

/// One cost event's identity and value, without the run (for diffing).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostEvent {
    /// `cost/<day>/<pool>/<kind>/<consumer>` (`/` in names as `~`).
    pub key: String,
    pub subject: String,
    pub time: String,
    pub cost_micros: i64,
    pub data: serde_json::Value,
}

fn micros(a: Amount) -> Result<Amount, CostError> {
    // Round half away from zero to 10⁻⁶ (raw is 10⁻⁹).
    let raw = a.raw();
    let q = raw / 1_000;
    let r = raw % 1_000;
    let q = if r >= 500 {
        q + 1
    } else if r <= -500 {
        q - 1
    } else {
        q
    };
    q.checked_mul(1_000)
        .map(Amount::from_raw)
        .ok_or(CostError::Overflow)
}

/// The cost events of `lines`.
pub fn cost_events(
    lines: &[Line],
    currency: &str,
    report_digest: &str,
) -> Result<Vec<CostEvent>, CostError> {
    let mut out = Vec::new();
    for (key, idx) in by_pool_day(lines) {
        let total: Amount = idx.iter().map(|i| lines[*i].amount).sum();
        // Split whole micros (the raw value counts micros here), so the
        // parts are whole micros that sum to the pool-day's micros.
        let total_micros = Amount::from_raw(micros(total)?.raw() / 1_000);
        let Some(parts) = split_by_amount(total_micros, lines, &idx)? else {
            continue;
        };
        for (i, part) in idx.iter().zip(parts) {
            let l = &lines[*i];
            if l.amount.is_zero() {
                continue;
            }
            let cost_micros = i64::try_from(part.raw()).map_err(|_| CostError::Overflow)?;
            let (engine, tenant, project, class) = match &l.consumer {
                Consumer::Work(k) => (
                    k.engine.clone(),
                    k.tenant.clone(),
                    k.project.clone(),
                    k.query_class.clone(),
                ),
                other => (
                    other.engine().to_string(),
                    NONE.into(),
                    NONE.into(),
                    NONE.into(),
                ),
            };
            let subject = if l.consumer.work().is_some() && tenant != NONE {
                tenant.clone()
            } else {
                OPERATOR_SUBJECT.to_string()
            };
            let ev_key = format!(
                "cost/{}/{}/{}/{}",
                key.day,
                key.pool.as_str().replace('/', "~"),
                key.kind,
                l.consumer.to_string().replace('/', "~")
            );
            out.push(CostEvent {
                key: ev_key,
                subject,
                time: format!("{}T23:59:59Z", key.day),
                cost_micros,
                data: json!({
                    "cost_micros": cost_micros,
                    "currency": currency,
                    "day": key.day.to_string(),
                    "pool": key.pool.as_str(),
                    "kind": key.kind.as_str(),
                    "engine": engine,
                    "project": project,
                    "query_class": class,
                    "basis": l.basis.as_str(),
                    "amount_basis": l.amount_basis.as_str(),
                    "usage": l.quantity.to_string(),
                    "usage_unit": key.kind.driver_unit(),
                    "trace": trace_id(l),
                    "report_digest": report_digest,
                }),
            });
        }
    }
    Ok(out)
}

/// The CloudEvents of `events` for run `run` (`application/cloudevents-batch+json`).
pub fn cloudevents(events: &[CostEvent], source: &str, ty: &str, run: u64) -> serde_json::Value {
    serde_json::Value::Array(
        events
            .iter()
            .map(|e| {
                let mut data = e.data.clone();
                data["cost_run"] = json!(run);
                json!({
                    "specversion": "1.0",
                    "id": format!("{}/r{run}", e.key),
                    "source": source,
                    "type": ty,
                    "subject": e.subject,
                    "time": e.time,
                    "datacontenttype": "application/json",
                    "dataschema": format!("urn:loams:json:{ty}"),
                    "data": data,
                })
            })
            .collect(),
    )
}

/// Adjustment events: for each key in `new` whose micros differ from
/// `sent`, an event with `cost_micros = new − sent` (and keys only in
/// `sent` go to zero).
pub fn adjustments(sent: &[CostEvent], new: &[CostEvent]) -> Vec<CostEvent> {
    let old: BTreeMap<&str, &CostEvent> = sent.iter().map(|e| (e.key.as_str(), e)).collect();
    let fresh: BTreeMap<&str, &CostEvent> = new.iter().map(|e| (e.key.as_str(), e)).collect();
    let mut out = Vec::new();
    for (k, e) in &fresh {
        let before = old.get(k).map_or(0, |o| o.cost_micros);
        if e.cost_micros != before {
            let mut a = (*e).clone();
            a.cost_micros = e.cost_micros - before;
            a.data["cost_micros"] = json!(a.cost_micros);
            out.push(a);
        }
    }
    for (k, o) in &old {
        if !fresh.contains_key(k) && o.cost_micros != 0 {
            let mut a = (*o).clone();
            a.cost_micros = -o.cost_micros;
            a.data["cost_micros"] = json!(a.cost_micros);
            out.push(a);
        }
    }
    out
}

/// Suggested OpenMeter meters for the cost events (YAML for OpenMeter's
/// config), also in `docs/api/meter/openmeter-cost-meters.yaml`.
pub const OPENMETER_METERS: &str = "meters:\n  - slug: loams_cost_micros\n    description: Loams cost attribution (design 84), micro-units of the reporting currency\n    eventType: io.loams.dev.cost.allocation.v1\n    aggregation: SUM\n    valueProperty: $.cost_micros\n    groupBy:\n      engine: $.engine\n      project: $.project\n      query_class: $.query_class\n      kind: $.kind\n      basis: $.basis\n  - slug: loams_cost_adjustment_micros\n    eventType: io.loams.dev.cost.adjustment.v1\n    aggregation: SUM\n    valueProperty: $.cost_micros\n    groupBy:\n      engine: $.engine\n      project: $.project\n      query_class: $.query_class\n      kind: $.kind\n";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Basis, ConsumerKey, CostSource, Day, PoolId, ResourceKind, SourceRef, Usage, allocate_all,
    };

    fn lines(cost: &str) -> Vec<Line> {
        let day = Day::parse("2026-10-28").expect("d");
        let pool = PoolId::new("eu/compute").expect("p");
        let src = CostSource {
            day,
            pool: pool.clone(),
            kind: ResourceKind::Cpu,
            amount: Amount::parse(cost).expect("a"),
            basis: Basis::Billed,
            source: SourceRef::Other { note: "x".into() },
            flags: Default::default(),
        };
        let u = |e: &str, t: &str| Usage {
            day,
            pool: pool.clone(),
            kind: ResourceKind::Cpu,
            consumer: ConsumerKey::new(e, t, "7", "_").expect("k"),
            quantity: Amount::from_units(1),
            basis: Basis::Measured,
        };
        allocate_all(
            &[src],
            &[],
            &[u("house", "org_1"), u("pg", "org_2"), u("kv", "org_3")],
        )
        .expect("alloc")
        .lines
    }

    #[test]
    fn cost_micros_conserve_per_pool_day() {
        let ev = cost_events(&lines("1.0000004"), "USD", "d").expect("events");
        assert_eq!(ev.len(), 3);
        assert_eq!(ev.iter().map(|e| e.cost_micros).sum::<i64>(), 1_000_000);
        assert_eq!(ev[0].subject, "org_1");
        assert_eq!(ev[0].data["engine"], "house");
    }

    #[test]
    fn openmeter_event_passes_usage_profile() {
        let ev = cost_events(&lines("3"), "USD", "d").expect("events");
        let batch = cloudevents(&ev, "loams://prod/meter/cost", ALLOCATION_TYPE, 12);
        let first = &batch[0];
        assert_eq!(first["specversion"], "1.0");
        assert_eq!(first["datacontenttype"], "application/json");
        assert!(first["subject"].is_string());
        assert!(first["id"].as_str().expect("id").ends_with("/r12"));
        assert_eq!(first["data"]["cost_micros"], 1_000_000);
        assert_eq!(first["data"]["cost_run"], 12);
        // The event passes Loams' own usage-profile checks for its shape.
        let ce = loams_meter_core::Event {
            id: first["id"].as_str().expect("id").into(),
            source: first["source"].as_str().expect("s").into(),
            r#type: first["type"].as_str().expect("t").into(),
            subject: loams_meter_core::SubjectKey::new(first["subject"].as_str().expect("s"))
                .expect("subject"),
            time: loams_meter_core::UtcNanos::from_rfc3339(first["time"].as_str().expect("t"))
                .expect("time"),
            tenant: None,
            lease: None,
            data: first["data"].as_object().expect("obj").clone(),
        };
        assert!(ce.to_openmeter_json().is_object());
    }

    #[test]
    fn recompute_sends_adjustment() {
        let sent = cost_events(&lines("3"), "USD", "d1").expect("events");
        let new = cost_events(&lines("6"), "USD", "d2").expect("events");
        let adj = adjustments(&sent, &new);
        assert_eq!(adj.len(), 3);
        assert!(adj.iter().all(|a| a.cost_micros == 1_000_000));
        assert!(adjustments(&new, &new).is_empty());
        let gone = adjustments(&sent, &[]);
        assert_eq!(gone.iter().map(|a| a.cost_micros).sum::<i64>(), -3_000_000);
    }
}
