//! The CLI's offline mode (design §84 §11.1, §15; Q1499): usage events as
//! CloudEvents NDJSON aggregated in memory the way the House's driver
//! queries aggregate `_meter.events` (counters summed per key; gauges as
//! each object's mean sampled size × 86,400), with query classes evaluated
//! by [`crate::classes`]. The rows then go through the same
//! [`crate::drivers::drivers`] and [`crate::pipeline::compute`] as the
//! `meter` role, so offline and online give the same numbers for the same
//! events.

use std::collections::BTreeMap;

use loams_meter_core::Amount;
use serde_json::{Map, Value as Json};

use crate::classes::Condition;
use crate::drivers::{DEFAULT_CLASSES, DriverRow, counter_fields, engine_types, entry};
use crate::inputs::CostInputs;
use crate::{CostError, Day, NONE};

/// Compiled class rules: (event types, class, condition), in order.
pub fn class_rules(
    inputs: &CostInputs,
) -> Result<Vec<(Vec<String>, String, Condition)>, CostError> {
    let mut out = Vec::new();
    for c in &inputs.classes {
        out.push((
            engine_types(&c.engine),
            c.class.clone(),
            Condition::parse(&c.when)?,
        ));
    }
    for (engine, class, when) in DEFAULT_CLASSES {
        out.push((
            engine_types(engine),
            class.to_string(),
            Condition::parse(when)?,
        ));
    }
    Ok(out)
}

fn text(data: &Map<String, Json>, k: &str) -> String {
    match data.get(k) {
        Some(Json::String(s)) => s.clone(),
        Some(Json::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

type Key = (String, String, String, String, String, String, String);

/// Aggregates usage CloudEvents (structured JSON, one per item) into driver
/// rows per UTC day.
pub fn rows_from_events(
    inputs: &CostInputs,
    events: impl IntoIterator<Item = Json>,
) -> Result<BTreeMap<Day, Vec<DriverRow>>, CostError> {
    let rules = class_rules(inputs)?;
    let fields = counter_fields();
    let mut counters: BTreeMap<(Day, Key), (u64, BTreeMap<String, Amount>)> = BTreeMap::new();
    // (day, key, object) → (Σ value, samples)
    let mut gauges: BTreeMap<(Day, Key, String), (Amount, u64)> = BTreeMap::new();
    let empty = Map::new();
    for ev in events {
        let ty = ev
            .get("type")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string();
        let time = ev.get("time").and_then(Json::as_str).unwrap_or_default();
        let unix = crate::import::formats::parse_time(time)?;
        let day = Day::of_unix(unix)?;
        let data = ev.get("data").and_then(Json::as_object).unwrap_or(&empty);
        let source = ev.get("source").and_then(Json::as_str).unwrap_or_default();
        let node = match text(data, "node") {
            n if !n.is_empty() => n,
            _ => source.rsplit('/').next().unwrap_or_default().to_string(),
        };
        let subject = ev
            .get("subject")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string();
        let tenantid = ev
            .get("tenantid")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string();
        let e = entry(&ty);
        let class = match e {
            Some(e) if e.is_node_agent() || e.gauge.is_some() => NONE.to_string(),
            _ => {
                let producer = text(data, "query_class");
                if !producer.is_empty() {
                    producer
                } else {
                    rules
                        .iter()
                        .find(|(types, _, c)| types.contains(&ty) && c.eval(data))
                        .map_or_else(|| "default".to_string(), |(_, class, _)| class.clone())
                }
            }
        };
        let key: Key = (
            ty.clone(),
            subject.clone(),
            tenantid.clone(),
            node,
            text(data, "engine"),
            text(data, "resource"),
            class,
        );
        match e.and_then(|e| e.gauge) {
            Some(field) => {
                let object = [
                    subject.as_str(),
                    tenantid.as_str(),
                    &text(data, "database"),
                    &text(data, "table"),
                    &text(data, "branch"),
                    &text(data, "endpoint"),
                    &text(data, "repository"),
                    &text(data, "namespace"),
                ]
                .join("|");
                let v = data
                    .get(field)
                    .and_then(Amount::from_json)
                    .unwrap_or_default();
                let slot = gauges
                    .entry((day, key, object))
                    .or_insert((Amount::ZERO, 0));
                slot.0 = slot.0.checked_add(v).ok_or(CostError::Overflow)?;
                slot.1 += 1;
            }
            None => {
                let slot = counters.entry((day, key)).or_default();
                slot.0 += 1;
                for f in &fields {
                    if let Some(v) = data.get(f).and_then(Amount::from_json) {
                        let s = slot.1.entry(f.clone()).or_default();
                        *s = s.checked_add(v).ok_or(CostError::Overflow)?;
                    }
                }
            }
        }
    }
    let mut out: BTreeMap<Day, Vec<DriverRow>> = BTreeMap::new();
    let row = |key: &Key, events: u64, sums: BTreeMap<String, Amount>| DriverRow {
        event_type: key.0.clone(),
        subject: key.1.clone(),
        tenantid: key.2.clone(),
        node: key.3.clone(),
        engine_label: key.4.clone(),
        resource: key.5.clone(),
        class: key.6.clone(),
        events,
        sums,
    };
    for ((day, key), (events, sums)) in counters {
        out.entry(day).or_default().push(row(&key, events, sums));
    }
    let mut g: BTreeMap<(Day, Key), (Amount, u64)> = BTreeMap::new();
    for ((day, key, _), (sum, n)) in gauges {
        let bs = Amount::from_raw(
            sum.raw().checked_mul(86_400).ok_or(CostError::Overflow)? / i128::from(n.max(1)),
        );
        let slot = g.entry((day, key)).or_insert((Amount::ZERO, 0));
        slot.0 = slot.0.checked_add(bs).ok_or(CostError::Overflow)?;
        slot.1 += n;
    }
    for ((day, key), (bs, n)) in g {
        out.entry(day)
            .or_default()
            .push(row(&key, n, [("byte_seconds".to_string(), bs)].into()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const YAML: &str = "version: 1\ncurrency: USD\nclasses:\n  - { engine: house, class: big, when: \"rows_read > 5000000\" }\n";

    fn ev(ty: &str, time: &str, data: Json) -> Json {
        serde_json::json!({
            "specversion": "1.0", "id": "x", "source": "loams://c/house/node-1",
            "type": format!("io.loams.dev.usage.{ty}"), "subject": "org_1", "tenantid": "1/7",
            "time": time, "datacontenttype": "application/json", "data": data,
        })
    }

    #[test]
    fn aggregates_like_the_house_queries() {
        let inputs = CostInputs::parse(YAML).expect("inputs");
        let events = vec![
            ev(
                "house.query.v1",
                "2026-10-28T10:00:00Z",
                serde_json::json!({"cpu_ms": 100, "statements": 1, "rows_read": 9000000}),
            ),
            ev(
                "house.query.v1",
                "2026-10-28T11:00:00Z",
                serde_json::json!({"cpu_ms": 50, "statements": 1, "rows_read": 9000001}),
            ),
            ev(
                "house.query.v1",
                "2026-10-28T12:00:00Z",
                serde_json::json!({"cpu_ms": 5, "statements": 1, "rows_read": 10}),
            ),
            ev(
                "house.storage.v1",
                "2026-10-28T00:00:00Z",
                serde_json::json!({"bytes": 1000, "table": "t"}),
            ),
            ev(
                "house.storage.v1",
                "2026-10-28T12:00:00Z",
                serde_json::json!({"bytes": 3000, "table": "t"}),
            ),
            ev(
                "house.query.v1",
                "2026-10-29T00:00:00Z",
                serde_json::json!({"cpu_ms": 1}),
            ),
        ];
        let rows = rows_from_events(&inputs, events).expect("rows");
        let d = Day::parse("2026-10-28").expect("d");
        let day = &rows[&d];
        let big = day.iter().find(|r| r.class == "big").expect("big");
        assert_eq!(big.events, 2);
        assert_eq!(big.sums["cpu_ms"], Amount::from_units(150));
        assert_eq!(big.node, "node-1");
        assert!(day.iter().any(|r| r.class == "point"));
        let storage = day
            .iter()
            .find(|r| r.event_type.ends_with("storage.v1"))
            .expect("storage");
        // Mean 2,000 bytes × 86,400 s.
        assert_eq!(
            storage.sums["byte_seconds"],
            Amount::from_units(2000 * 86_400)
        );
        assert_eq!(rows.len(), 2);
    }
}
