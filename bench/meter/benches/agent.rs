//! The node agent's per-event work (BQ1-meter):
//! - `agent/measurement_to_record`: a cgroup delta becomes a `MeterEvent`
//!   and then the produce route's record (CloudEvent, D270 record layout,
//!   base64, the subject's partition), what `StreamSink::send` does per event;
//! - `agent/spool_append_{1,64,256}`: the spooled mode's durable append,
//!   one `fdatasync` per batch (§63 §7.2), so batch size sets events/s.
//!
//! The spool writes under `$METER_BENCH_DIR` (default
//! `/mnt/Projects/bench-data/bq1-meter`), never `/tmp`, and each timed run
//! deletes its directory.
//!
//! Run: `benchslot.sh bq1-meter taskset -c 2 cargo bench --bench agent`.

use std::hint::black_box;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use loams_meter_agent::event::Measurement;
use loams_meter_agent::stream::record;
use loams_meter_agent::{Attribution, Meter, MeterEvent, SourceKind, Spool, SpoolConfig};
use time::OffsetDateTime;

fn measurement(i: u64) -> Measurement {
    let start = OffsetDateTime::from_unix_timestamp(1_761_000_000).expect("time");
    Measurement {
        attribution: Attribution::new(format!("{}", 1 + i % 64), "7"),
        source: SourceKind::Cgroup,
        meter: Meter::CpuUsec,
        value: 1_000 + i,
        window_start: start,
        window_end: start + time::Duration::seconds(10),
        resource: format!("sandbox-{}", i % 512),
        counter: Some(10_000 + i),
        peak_bytes: None,
        cpu_usec: Some(1_000 + i),
        estimated: false,
    }
}

fn events(n: u64) -> Vec<MeterEvent> {
    (0..n).map(|i| measurement(i).into_event("node-1", i)).collect()
}

fn bench_dir() -> PathBuf {
    std::env::var_os("METER_BENCH_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/Projects/bench-data/bq1-meter"))
}

fn agent_benches(c: &mut Criterion) {
    let mut g = c.benchmark_group("agent");
    g.throughput(Throughput::Elements(1));
    let mut seq = 0u64;
    g.bench_function("measurement_to_record", |b| {
        b.iter(|| {
            seq += 1;
            let event = measurement(seq).into_event("node-1", seq);
            black_box(record(&event, "bench", 32).expect("record"))
        })
    });

    let root = bench_dir();
    for batch in [1u64, 64, 256] {
        let evs = events(batch);
        g.throughput(Throughput::Elements(batch));
        g.sample_size(20);
        g.measurement_time(Duration::from_secs(8));
        g.bench_function(format!("spool_append_{batch}"), |b| {
            b.iter_custom(|iters| {
                let dir = root.join(format!("spool-{batch}-{}", std::process::id()));
                let _ = std::fs::remove_dir_all(&dir);
                std::fs::create_dir_all(&dir).expect("bench dir");
                let mut spool = Spool::open(
                    &dir,
                    SpoolConfig {
                        max_bytes: u64::MAX,
                        segment_bytes: 8 << 20,
                    },
                )
                .expect("spool");
                let start = Instant::now();
                for _ in 0..iters {
                    spool.append(&evs).expect("append");
                }
                let took = start.elapsed();
                drop(spool);
                std::fs::remove_dir_all(&dir).expect("clean bench dir");
                took
            })
        });
    }
    g.finish();
}

criterion_group!(benches, agent_benches);
criterion_main!(benches);
