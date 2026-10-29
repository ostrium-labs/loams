//! The gate's per-request cost, one thread (BQ1-meter). The multi-thread
//! decisions/s and latency percentiles come from `meter-loadgen gate`.
//! - `gate/nogate_check`: the floor, a product without the meter;
//! - `gate/check_hit_hot`: one subject, a lease hit (lookup, CAS, counters);
//! - `gate/check_hit_100k_subjects`: a hit spread over 100,000 subjects, so
//!   the shard maps and slots fall out of cache as they would on a busy node;
//! - `gate/reserve_settle`: the LLM-style hold and settle on a hit;
//! - `gate/check_many_64_100k_subjects`: the pipelined API, 64 requests per
//!   call over the same 100,000 subjects (throughput is per request).
//!
//! Run: `benchslot.sh bq1-meter taskset -c 2 cargo bench --bench gate`.

use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use loams_meter_gate::gate::{
    CheckRequest, FixedSource, GateConfig, LeaseGate, LeaseGrant, LeaseId,
};
use loams_meter_gate::{Gate, NoGate};

fn long_grant() -> LeaseGrant {
    LeaseGrant {
        ttl: Duration::from_secs(3600),
        refresh_after: Duration::from_secs(3600),
        ..LeaseGrant::limit(LeaseId(1), u64::MAX / 4, 1)
    }
}

fn gate(subjects: &[String]) -> LeaseGate {
    let grant = long_grant();
    let gate = LeaseGate::new(
        Arc::new(FixedSource::new(grant.clone())),
        GateConfig {
            node: "bench".into(),
            ..GateConfig::default()
        },
    )
    .expect("gate");
    for s in subjects {
        gate.install(s, "api_requests", &grant);
    }
    gate
}

fn gate_benches(c: &mut Criterion) {
    let mut g = c.benchmark_group("gate");
    g.throughput(Throughput::Elements(1));

    g.bench_function("nogate_check", |b| {
        b.iter(|| black_box(NoGate.check(black_box("org_1"), "api_requests", 1)))
    });

    let hot = gate(&["org_1".to_string()]);
    g.bench_function("check_hit_hot", |b| {
        b.iter(|| black_box(hot.check(black_box("org_1"), "api_requests", 1)))
    });

    let subjects: Vec<String> = (0..100_000).map(|i| format!("org_{i}")).collect();
    let wide = gate(&subjects);
    let mut i = 0usize;
    g.bench_function("check_hit_100k_subjects", |b| {
        b.iter(|| {
            // A stride co-prime with the count walks every subject.
            i = (i + 7_919) % subjects.len();
            black_box(wide.check(&subjects[i], "api_requests", 1))
        })
    });

    let mut out = Vec::with_capacity(64);
    let mut j = 0usize;
    g.throughput(Throughput::Elements(64));
    g.bench_function("check_many_64_100k_subjects", |b| {
        b.iter(|| {
            let batch: Vec<CheckRequest<'_>> = (0..64)
                .map(|_| {
                    j = (j + 7_919) % subjects.len();
                    CheckRequest {
                        subject: &subjects[j],
                        feature: "api_requests",
                        cost: 1,
                    }
                })
                .collect();
            wide.check_many(&batch, &mut out);
            black_box(out.len())
        })
    });
    g.throughput(Throughput::Elements(1));

    g.bench_function("reserve_settle", |b| {
        b.iter(|| {
            let r = hot.reserve("org_1", "api_requests", 4_096).expect("reserve");
            black_box(r.settle(1_000))
        })
    });
    g.finish();
}

criterion_group!(benches, gate_benches);
criterion_main!(benches);
