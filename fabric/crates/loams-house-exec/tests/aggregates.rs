//! ClickHouse's aggregates (plan HE1 Task 28): result types and errors against
//! the oracle's fixtures, `windowFunnel` over generated event streams in every
//! mode, and the partial-state path (`state` and `merge_batch`) against a
//! single pass. The fixtures are `scripts/house/capture-agg-fixtures.py`'s.

// `unwrap_used` is a warning and CI runs `-D warnings`; tests unwrap freely.
#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::Arc;

use datafusion::arrow::array::{
    ArrayRef, Float64Array, Int64Array, RecordBatch, StringArray, UInt8Array, UInt32Array,
};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::arrow::util::pretty::pretty_format_batches;
use datafusion::common::ScalarValue;
use datafusion::datasource::{MemTable, provider_as_source};
use datafusion::logical_expr::TableSource;
use datafusion::prelude::{SessionConfig, SessionContext};
use loams_house_exec::agg::registry::build;
use loams_house_sql::aggregates::parse;
use loams_house_sql::{Catalog, Registry, prepare};
use loams_house_types::ChType;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// Forms the engine answers differently, with the task that closes them.
const TYPE_GAPS: &[(&str, &str, &str)] = &[];

#[test]
fn aggregate_result_types_match_the_oracle() {
    let mut problems = Vec::new();
    let mut checked = 0;
    for line in fixture("agg_types.tsv").lines() {
        let mut cols = line.split('\t');
        let (name, args, expected) = (
            cols.next().unwrap(),
            cols.next().unwrap(),
            cols.next().unwrap(),
        );
        if TYPE_GAPS.iter().any(|(n, a, _)| *n == name && *a == args) {
            continue;
        }
        let types: Vec<ChType> = args
            .split('|')
            .map(|t| loams_house_types::parse(t).unwrap())
            .collect();
        let spec = parse(name).unwrap_or_else(|| panic!("{name} does not parse"));
        let got = build(&spec, &types);
        checked += 1;
        match (expected.strip_prefix("ERROR "), got) {
            (Some(code), Err(e)) => {
                if e.code.to_string() != code {
                    problems.push(format!(
                        "{name}({args}): error {} ({e}), the oracle {code}",
                        e.code
                    ));
                }
            }
            (Some(code), Ok(agg)) => problems.push(format!(
                "{name}({args}): {}, the oracle error {code}",
                agg.result()
            )),
            (None, Err(e)) => problems.push(format!("{name}({args}): {e}, the oracle {expected}")),
            (None, Ok(agg)) => {
                let want = loams_house_types::parse(expected).unwrap();
                if agg.result() != &want {
                    problems.push(format!(
                        "{name}({args}): {}, the oracle {expected}",
                        agg.result().to_compact_string()
                    ));
                }
            }
        }
    }
    assert!(
        problems.is_empty(),
        "{} of {checked}:\n{}",
        problems.len(),
        problems.join("\n")
    );
    assert!(checked > 1500, "only {checked} checked");
}

fn funnel_answer(name: &str, events: &[Vec<u64>]) -> u8 {
    let spec = parse(name).unwrap();
    let agg = build(
        &spec,
        &[ChType::UInt32, ChType::UInt8, ChType::UInt8, ChType::UInt8],
    )
    .unwrap();
    let mut acc = agg.accumulator().unwrap();
    acc.update_batch(&columns(events)).unwrap();
    match acc.evaluate().unwrap() {
        ScalarValue::UInt8(Some(v)) => v,
        other => panic!("{other:?}"),
    }
}

fn columns(events: &[Vec<u64>]) -> Vec<ArrayRef> {
    let ts: ArrayRef = Arc::new(UInt32Array::from_iter_values(
        events.iter().map(|e| e[0] as u32),
    ));
    let mut out = vec![ts];
    for c in 1..4 {
        out.push(Arc::new(UInt8Array::from_iter_values(
            events.iter().map(|e| e[c] as u8),
        )) as ArrayRef);
    }
    out
}

fn funnel_name(window: &str, modes: &str) -> String {
    let mut params = vec![window.to_string()];
    params.extend(
        modes
            .split(',')
            .filter(|m| !m.is_empty())
            .map(|m| format!("'{m}'")),
    );
    format!("windowFunnel({})", params.join(", "))
}

#[test]
fn window_funnel_matches_the_oracle() {
    let mut problems = Vec::new();
    let mut checked = 0;
    for line in fixture("funnel.tsv").lines() {
        let cols: Vec<&str> = line.split('\t').collect();
        let events: Vec<Vec<u64>> = serde_json::from_str(cols[2]).unwrap();
        let expected: u8 = cols[3].parse().unwrap();
        let name = funnel_name(cols[0], cols[1]);
        let got = funnel_answer(&name, &events);
        checked += 1;
        if got != expected {
            problems.push(format!(
                "{name} over {}: {got}, the oracle {expected}",
                cols[2]
            ));
        }
        // The same stream through two partial states merged.
        let spec = parse(&name).unwrap();
        let agg = build(
            &spec,
            &[ChType::UInt32, ChType::UInt8, ChType::UInt8, ChType::UInt8],
        )
        .unwrap();
        let half = events.len() / 2;
        let mut final_acc = agg.accumulator().unwrap();
        for part in [&events[..half], &events[half..]] {
            let mut a = agg.accumulator().unwrap();
            a.update_batch(&columns(part)).unwrap();
            let state: Vec<ArrayRef> = a
                .state()
                .unwrap()
                .into_iter()
                .map(|s| s.to_array().unwrap())
                .collect();
            final_acc.merge_batch(&state).unwrap();
        }
        let merged = match final_acc.evaluate().unwrap() {
            ScalarValue::UInt8(Some(v)) => v,
            other => panic!("{other:?}"),
        };
        if merged != expected {
            problems.push(format!(
                "{name} merged over {}: {merged}, the oracle {expected}",
                cols[2]
            ));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    assert_eq!(checked, 600);
}

#[derive(Debug)]
struct Tables(HashMap<String, Arc<MemTable>>);

impl Catalog for Tables {
    fn table(&self, _database: &str, name: &str) -> Option<Arc<dyn TableSource>> {
        self.0.get(name).map(|t| provider_as_source(t.clone()))
    }
}

fn table() -> Arc<MemTable> {
    let g = Field::new("g", DataType::Utf8, false);
    let x = Field::new("x", DataType::Int64, false);
    let f = Field::new("f", DataType::Float64, false);
    let schema = Arc::new(Schema::new(vec![g, x, f]));
    let part = |k: i64| {
        let n = 50;
        RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from_iter_values(
                    (0..n).map(|i| ["a", "b", "c"][((i + k) % 3) as usize]),
                )),
                Arc::new(Int64Array::from_iter_values((0..n).map(|i| i * k - 40))),
                Arc::new(Float64Array::from_iter_values(
                    (0..n).map(|i| (i as f64) * 0.25),
                )),
            ],
        )
        .unwrap()
    };
    Arc::new(
        MemTable::try_new(
            schema.clone(),
            vec![vec![part(1)], vec![part(2)], vec![part(3)], vec![part(5)]],
        )
        .unwrap(),
    )
}

async fn run(sql: &str, partitions: usize) -> String {
    let mut registry = Registry::default();
    loams_house_exec::agg::register(&mut registry);
    let catalog = Tables(HashMap::from([("t".to_string(), table())]));
    let p = prepare(sql, &[], &catalog, &registry, "default").unwrap();
    let ctx =
        SessionContext::new_with_config(SessionConfig::new().with_target_partitions(partitions));
    let batches = ctx
        .execute_logical_plan(p.plan)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    pretty_format_batches(&batches).unwrap().to_string()
}

/// Order-independent aggregates answer the same through partial states
/// (several partitions) as in one pass.
#[tokio::test]
async fn partial_states_merge_as_one_pass() {
    let sql = "SELECT g, count(), sum(x), avg(f), min(x), max(x), argMax(x, f), \
               sumIf(x, x > 0), countDistinct(x), sum(DISTINCT x), topK(2)(x > 0), \
               sumOrNull(x), groupUniqArrayIf(1)(g, x > 1000), anyLast(g), \
               countIfOrNull(x > 1000), avgDistinctIf(f, x > 0) \
               FROM t GROUP BY g ORDER BY g";
    let one = run(sql, 1).await;
    let many = run(sql, 4).await;
    assert_eq!(one, many);
}
