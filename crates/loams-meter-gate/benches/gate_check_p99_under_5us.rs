//! `gate_check_p99_under_5us` (ME1 Task 18, design §63 §10.3): the latency
//! of `Gate::check` on a lease hit, from 64 threads at once.
//!
//! std-only (D2183): no criterion, so the gate adds no third-party crate to
//! the Buck graph. Each thread times every call with `Instant`, the samples
//! are merged and the p50, p99 and max printed. The 5 µs p99 target is
//! enforced when `LOAMS_GATE_BENCH_ENFORCE=1` (a quiet, dedicated machine);
//! on shared CI runners with fewer cores than threads, preemption inside a
//! timed call dominates the tail, so the bench only reports there.
//!
//! Run: `cargo bench -p loams-meter-gate --bench gate_check_p99_under_5us`.

use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use loams_meter_gate::gate::{FixedSource, Gate, GateConfig, LeaseGate, LeaseGrant, LeaseId};

const THREADS: usize = 64;
const SUBJECTS: usize = 256;
const CALLS_PER_THREAD: usize = 20_000;
const TARGET_P99: Duration = Duration::from_micros(5);

fn main() {
    let grant = LeaseGrant {
        ttl: Duration::from_secs(3600),
        refresh_after: Duration::from_secs(3600),
        ..LeaseGrant::limit(LeaseId(1), u64::MAX / 4, 1)
    };
    let gate = Arc::new(
        LeaseGate::new(
            Arc::new(FixedSource::new(grant.clone())),
            GateConfig {
                node: "bench".into(),
                ..GateConfig::default()
            },
        )
        .expect("gate"),
    );
    let subjects: Vec<String> = (0..SUBJECTS).map(|i| format!("org_{i}")).collect();
    for s in &subjects {
        gate.install(s, "api_requests", &grant);
    }
    let subjects = Arc::new(subjects);
    let barrier = Arc::new(Barrier::new(THREADS));
    let handles: Vec<_> = (0..THREADS)
        .map(|t| {
            let gate = Arc::clone(&gate);
            let subjects = Arc::clone(&subjects);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let mut samples = Vec::with_capacity(CALLS_PER_THREAD);
                barrier.wait();
                for i in 0..CALLS_PER_THREAD {
                    let subject = &subjects[(t * 7 + i) % SUBJECTS];
                    let start = Instant::now();
                    let d = gate.check(subject, "api_requests", 1);
                    samples.push(start.elapsed());
                    assert!(d.is_allowed());
                }
                samples
            })
        })
        .collect();
    let mut all: Vec<Duration> = handles
        .into_iter()
        .flat_map(|h| h.join().expect("bench thread"))
        .collect();
    all.sort_unstable();
    let at = |q: f64| all[((all.len() as f64 * q) as usize).min(all.len() - 1)];
    let (p50, p99, max) = (at(0.50), at(0.99), all[all.len() - 1]);
    println!(
        "gate_check_p99_under_5us: {} calls, {THREADS} threads, p50 {p50:?}, p99 {p99:?}, max {max:?}",
        all.len()
    );
    if std::env::var("LOAMS_GATE_BENCH_ENFORCE").as_deref() == Ok("1") {
        assert!(p99 <= TARGET_P99, "p99 {p99:?} above {TARGET_P99:?}");
    }
}
