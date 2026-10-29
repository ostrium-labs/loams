//! `loams-meter-core` and the gate's event encoder (BQ1-meter):
//! - `event/parse_validate`: JSON CloudEvent bytes → `loams_cloudevents`
//!   parse → the usage profile (`Event::from_cloudevent`), what ingest does
//!   per external event;
//! - `event/encode_cloudevent`: `Event` → CloudEvent → JSON bytes;
//! - `event/gate_usage_event_encode`: what `MeterSink` pays per in-process
//!   event (`UsageEvent` built and encoded);
//! - `aggregate/sum_by_group`: a meter's value and group key per event,
//!   summed into windows, per event (the counter owner's and close job's
//!   inner loop);
//! - `lease/acquire_and_retire`: one `Allocator::acquire` plus the usage
//!   event that retires it (the counter owner's per-lease work).
//!
//! Run under the bench lock:
//! `benchslot.sh bq1-meter taskset -c 2 cargo bench --bench core`.

use std::collections::{BTreeMap, HashMap};
use std::hint::black_box;

use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use loams_cloudevents::json::{parse_event, write_event};
use loams_meter_core::lease::{Allocator, AllocatorConfig, LeasePolicy, LeaseRequest};
use loams_meter_core::{
    Aggregation, Amount, Event, FeatureKey, JsonPath, Meter, Origin, Slug, SubjectKey, UtcNanos,
};
use loams_meter_gate::UsageEvent;

/// §63 §6.2's example event.
const HOUSE_QUERY: &str = r#"{"specversion":"1.0","id":"q-7f3a9c/1","source":"loams://prod-eu1/house/node-3","type":"io.loams.dev.usage.house.query.v1","subject":"org_42","time":"2026-10-19T09:14:03.218734Z","datacontenttype":"application/json","dataschema":"urn:loams:json:io.loams.dev.usage.house.query.v1","tenantid":"42/7","loamslease":"l9k2m4q8x1","data":{"namespace":"7","cpu_ms":1840,"bytes_scanned":734003200,"rows_read":1200000,"result_bytes":52100,"worker_ms":2210,"statements":1,"code":0}}"#;

fn sample_event() -> Event {
    let ce = parse_event(HOUSE_QUERY.as_bytes()).expect("cloudevent");
    Event::from_cloudevent(&ce, Origin::Service).expect("usage profile")
}

/// 1,024 events over 64 subjects and 8 namespaces.
fn events() -> Vec<Event> {
    let base = sample_event();
    (0..1024u32)
        .map(|i| {
            let mut e = base.clone();
            e.id = format!("q-{i}/1");
            e.subject = SubjectKey::new(format!("org_{}", i % 64)).expect("subject");
            e.data.insert("namespace".into(), (i % 8).to_string().into());
            e.data.insert("cpu_ms".into(), (i % 5000).into());
            e
        })
        .collect()
}

fn house_cpu_meter() -> Meter {
    Meter {
        slug: Slug::new("house_cpu_ms").expect("slug"),
        event_type: "io.loams.dev.usage.house.query.v1".into(),
        aggregation: Aggregation::Sum,
        value_property: Some(JsonPath::parse("$.cpu_ms").expect("path")),
        group_by: BTreeMap::from([(
            "namespace".to_string(),
            JsonPath::parse("$.namespace").expect("path"),
        )]),
        event_from: None,
    }
}

fn event_benches(c: &mut Criterion) {
    let mut g = c.benchmark_group("event");
    g.throughput(Throughput::Elements(1));
    let bytes = HOUSE_QUERY.as_bytes();
    g.bench_function("parse_validate", |b| {
        b.iter(|| {
            let ce = parse_event(black_box(bytes)).expect("cloudevent");
            black_box(Event::from_cloudevent(&ce, Origin::Service).expect("profile"))
        })
    });
    let event = sample_event();
    g.bench_function("encode_cloudevent", |b| {
        b.iter(|| {
            let ce = black_box(&event).to_cloudevent().expect("cloudevent");
            black_box(write_event(&ce))
        })
    });
    let mut n: u64 = 0;
    g.bench_function("gate_usage_event_encode", |b| {
        b.iter(|| {
            n += 1;
            let e = UsageEvent::new("io.loams.dev.usage.house.query.v1", format!("q-{n}/1"))
                .source("loams://bench/house/node-1")
                .subject("org_42")
                .tenant("42/7")
                .at_ns(1_761_000_000_000_000_000)
                .text("namespace", "7")
                .num("cpu_ms", 1840)
                .num("bytes_scanned", 734_003_200)
                .num("rows_read", 1_200_000);
            black_box(e.encode())
        })
    });
    g.finish();
}

fn aggregate_benches(c: &mut Criterion) {
    let evs = events();
    let meter = house_cpu_meter();
    let mut g = c.benchmark_group("aggregate");
    g.throughput(Throughput::Elements(evs.len() as u64));
    g.bench_function("sum_by_group", |b| {
        b.iter(|| {
            let mut windows: HashMap<(String, String), Amount> = HashMap::new();
            for e in &evs {
                if let Some(v) = meter.value(e).expect("value") {
                    let key = (e.subject.as_str().to_owned(), meter.group_key(e));
                    *windows.entry(key).or_insert(Amount::ZERO) += v;
                }
            }
            black_box(windows.len())
        })
    });
    g.finish();
}

fn lease_benches(c: &mut Criterion) {
    let mut g = c.benchmark_group("lease");
    // Each iteration is 16 grants, one per node.
    g.throughput(Throughput::Elements(16));
    let subject = SubjectKey::new("org_1").expect("subject");
    let feature = FeatureKey::new("api_requests").expect("feature");
    let nodes: Vec<String> = (0..16).map(|i| format!("node-{i}")).collect();
    g.bench_function("acquire_and_retire", |b| {
        b.iter_batched_ref(
            || {
                Allocator::new(
                    AllocatorConfig::default(),
                    1,
                    Amount::from_units(1_000_000_000),
                    Amount::from_units(1),
                    LeasePolicy::HardLimit,
                    Amount::ZERO,
                )
            },
            |a| {
                for (i, node) in nodes.iter().enumerate() {
                    let grant = a.acquire(&LeaseRequest {
                        subject: subject.clone(),
                        feature: feature.clone(),
                        node: node.clone(),
                        want: Amount::from_units(1),
                        held: Vec::new(),
                        now: UtcNanos(i as i64),
                    });
                    a.on_usage(grant.id.as_ref(), grant.amount);
                }
            },
            BatchSize::SmallInput,
        )
    });
    g.finish();
}

criterion_group!(benches, event_benches, aggregate_benches, lease_benches);
criterion_main!(benches);
