//! `meter-loadgen`: end-to-end load for Loams Meter (BQ1-meter).
//!
//! Modes, each printing one JSON line:
//! - `gate`: N pinned threads call `LeaseGate::check` on lease hits over
//!   `--subjects` subjects (`--subjects 1` is one hot subject) for `--secs`.
//!   Reports decisions/s and p50/p99/p99.9 per call, timing one call in 64.
//! - `sink`: N pinned threads build and emit usage events into the
//!   best-effort `MeterSink` (D365) whose producer only counts, so the
//!   number is the sink's own ceiling: enqueue, batch, encode, flush.
//! - `agent`: N pinned agents each turn measurements into events, append
//!   them to a spool (`fdatasync` per batch), forward them with the real
//!   `StreamSink` over HTTP to a local mock of the produce route, and
//!   acknowledge. `--no-spool` sends straight from memory. The mock server
//!   runs on `--server-cores` and answers 200 to every request, so the
//!   number is the agent's side of ingest, not the stream's.
//!
//! Every measurement runs under `.he0-shared/benchslot.sh bq1-meter …`
//! with `taskset` around the whole process; `--cores` pins worker threads.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use hdrhistogram::Histogram;
use loams_meter_agent::event::Measurement;
use loams_meter_agent::{
    Attribution, Meter, MeterEvent, Sink, SourceKind, Spool, SpoolConfig, StreamSink,
    StreamSinkConfig, forward,
};
use loams_meter_gate::event::EncodedEvent;
use loams_meter_gate::gate::{
    CheckRequest, FixedSource, GateConfig, LeaseGate, LeaseGrant, LeaseId,
};
use loams_meter_gate::sink::{BestEffortConfig, BestEffortSink, ProduceError, Producer};
use loams_meter_gate::{Gate, MeterSink, UsageEvent};
use serde_json::{Value, json};

struct Args {
    mode: String,
    threads: usize,
    cores: Vec<usize>,
    server_cores: Vec<usize>,
    subjects: usize,
    secs: u64,
    batch: usize,
    partitions: u32,
    spool: bool,
    dir: PathBuf,
    /// Requests per `check_many` / events per `emit_batch`; 1 = unpipelined.
    depth: usize,
    /// Install the meter's unmetered grant (D2400: a pair without a policy,
    /// counting only) instead of a limiting one.
    unmetered: bool,
}

fn parse_cores(s: &str) -> Vec<usize> {
    let mut out = Vec::new();
    for part in s.split(',').filter(|p| !p.is_empty()) {
        match part.split_once('-') {
            Some((a, b)) => {
                let (a, b): (usize, usize) = (
                    a.parse().expect("core range start"),
                    b.parse().expect("core range end"),
                );
                out.extend(a..=b);
            }
            None => out.push(part.parse().expect("core")),
        }
    }
    out
}

fn args() -> Args {
    let mut it = std::env::args().skip(1);
    let mode = it.next().unwrap_or_else(|| {
        eprintln!("usage: meter-loadgen <gate|sink|agent> [--threads N] [--cores 0-5] [--server-cores 6-11] [--subjects N] [--secs N] [--batch N] [--partitions N] [--no-spool] [--dir PATH] [--depth N]");
        std::process::exit(2)
    });
    let mut a = Args {
        mode,
        threads: 1,
        cores: Vec::new(),
        server_cores: Vec::new(),
        subjects: 10_000,
        secs: 10,
        batch: 256,
        partitions: 32,
        spool: true,
        dir: PathBuf::from("/mnt/Projects/bench-data/bq1-meter"),
        depth: 1,
        unmetered: false,
    };
    while let Some(flag) = it.next() {
        let mut value = || it.next().unwrap_or_else(|| panic!("{flag} needs a value"));
        match flag.as_str() {
            "--threads" => a.threads = value().parse().expect("threads"),
            "--cores" => a.cores = parse_cores(&value()),
            "--server-cores" => a.server_cores = parse_cores(&value()),
            "--subjects" => a.subjects = value().parse().expect("subjects"),
            "--secs" => a.secs = value().parse().expect("secs"),
            "--batch" => a.batch = value().parse().expect("batch"),
            "--partitions" => a.partitions = value().parse().expect("partitions"),
            "--no-spool" => a.spool = false,
            "--dir" => a.dir = PathBuf::from(value()),
            "--depth" => a.depth = value().parse::<usize>().expect("depth").max(1),
            "--unmetered" => a.unmetered = true,
            other => panic!("unknown flag {other}"),
        }
    }
    a
}

fn pin(cores: &[usize], i: usize) {
    if cores.is_empty() {
        return;
    }
    let id = cores[i % cores.len()];
    if !core_affinity::set_for_current(core_affinity::CoreId { id }) {
        eprintln!("warning: could not pin to core {id}");
    }
}

fn histogram() -> Histogram<u64> {
    Histogram::new_with_bounds(1, 60_000_000_000, 3).expect("histogram")
}

fn latency_json(h: &Histogram<u64>) -> Value {
    json!({
        "samples": h.len(),
        "p50_ns": h.value_at_quantile(0.50),
        "p99_ns": h.value_at_quantile(0.99),
        "p999_ns": h.value_at_quantile(0.999),
        "max_ns": h.max(),
    })
}

/// Runs `work(thread, stop)` on `threads` pinned threads for `secs`; each
/// returns (operations, latency histogram).
fn run_threads<F>(a: &Args, work: F) -> (u64, f64, Histogram<u64>)
where
    F: Fn(usize, &AtomicBool) -> (u64, Histogram<u64>) + Send + Sync + 'static,
{
    let work = Arc::new(work);
    let stop = Arc::new(AtomicBool::new(false));
    let barrier = Arc::new(Barrier::new(a.threads + 1));
    let handles: Vec<_> = (0..a.threads)
        .map(|t| {
            let (work, stop, barrier, cores) = (
                Arc::clone(&work),
                Arc::clone(&stop),
                Arc::clone(&barrier),
                a.cores.clone(),
            );
            std::thread::spawn(move || {
                pin(&cores, t);
                barrier.wait();
                work(t, &stop)
            })
        })
        .collect();
    barrier.wait();
    let start = Instant::now();
    std::thread::sleep(Duration::from_secs(a.secs));
    stop.store(true, Ordering::Relaxed);
    let mut ops = 0;
    let mut merged = histogram();
    for h in handles {
        let (n, hist) = h.join().expect("worker");
        ops += n;
        merged.add(&hist).expect("merge");
    }
    (ops, start.elapsed().as_secs_f64(), merged)
}

/// Requests that were refused (gate) or dropped (sink): the run's errors.
static ERRORS: AtomicU64 = AtomicU64::new(0);

fn gate_mode(a: &Args) -> Value {
    let grant = LeaseGrant {
        ttl: Duration::from_secs(3600),
        refresh_after: Duration::from_secs(3600),
        // The meter's unmetered grant (lease_client::unmetered_grant).
        counting: a.unmetered,
        lease: if a.unmetered { LeaseId::NONE } else { LeaseId(1) },
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
    let subjects: Arc<Vec<String>> =
        Arc::new((0..a.subjects.max(1)).map(|i| format!("org_{i}")).collect());
    for s in subjects.iter() {
        gate.install(s, "api_requests", &grant);
    }
    let g = Arc::clone(&gate);
    let subs = Arc::clone(&subjects);
    let depth = a.depth;
    let (ops, secs, hist) = run_threads(a, move |t, stop| {
        let mut h = histogram();
        let mut n: u64 = 0;
        let mut refused: u64 = 0;
        let mut i = t.wrapping_mul(104_729);
        let len = subs.len();
        if depth == 1 {
            while !stop.load(Ordering::Relaxed) {
                for k in 0..1024u32 {
                    i = i.wrapping_add(7_919);
                    let subject = &subs[i % len];
                    let d = if k % 64 == 0 {
                        let s = Instant::now();
                        let d = g.check(subject, "api_requests", 1);
                        let ns = u64::try_from(s.elapsed().as_nanos()).unwrap_or(u64::MAX);
                        h.record(ns.max(1)).expect("record");
                        d
                    } else {
                        g.check(subject, "api_requests", 1)
                    };
                    refused += u64::from(!std::hint::black_box(d).is_allowed());
                }
                n += 1024;
            }
        } else {
            // Pipelined: `depth` requests per `check_many`, as a product that
            // drains a socket or queue in batches would send them. The latency
            // recorded is the batch call's, which every request in it waits.
            let mut out = Vec::with_capacity(depth);
            let mut batch: Vec<CheckRequest<'_>> = Vec::with_capacity(depth);
            let mut calls: u64 = 0;
            while !stop.load(Ordering::Relaxed) {
                batch.clear();
                for _ in 0..depth {
                    i = i.wrapping_add(7_919);
                    batch.push(CheckRequest {
                        subject: &subs[i % len],
                        feature: "api_requests",
                        cost: 1,
                    });
                }
                if calls % 16 == 0 {
                    let s = Instant::now();
                    g.check_many(&batch, &mut out);
                    let ns = u64::try_from(s.elapsed().as_nanos()).unwrap_or(u64::MAX);
                    h.record(ns.max(1)).expect("record");
                } else {
                    g.check_many(&batch, &mut out);
                }
                refused += out.iter().filter(|d| !d.is_allowed()).count() as u64;
                calls += 1;
                n += depth as u64;
            }
        }
        ERRORS.fetch_add(refused, Ordering::Relaxed);
        (n, h)
    });
    let stats = gate.stats();
    json!({
        "mode": "gate",
        "unmetered": a.unmetered,
        "depth": a.depth,
        "errors": ERRORS.load(Ordering::Relaxed),
        "threads": a.threads,
        "cores": a.cores,
        "subjects": a.subjects,
        "secs": secs,
        "ops": ops,
        "ops_per_sec": ops as f64 / secs,
        "ops_per_sec_per_thread": ops as f64 / secs / a.threads as f64,
        "latency": latency_json(&hist),
        "cold_misses": stats.cold_misses,
    })
}

#[derive(Debug, Default)]
struct CountingProducer {
    events: AtomicU64,
    bytes: AtomicU64,
    batches: AtomicU64,
}

impl Producer for CountingProducer {
    fn produce(&self, batch: &[EncodedEvent]) -> Result<(), ProduceError> {
        let bytes: usize = batch.iter().map(EncodedEvent::size).sum();
        self.events.fetch_add(batch.len() as u64, Ordering::Relaxed);
        self.bytes.fetch_add(bytes as u64, Ordering::Relaxed);
        self.batches.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

fn sink_mode(a: &Args) -> Value {
    let producer = Arc::new(CountingProducer::default());
    let sink = Arc::new(
        BestEffortSink::new(
            Arc::clone(&producer) as Arc<dyn Producer>,
            BestEffortConfig::default(),
        )
        .expect("sink"),
    );
    let s = Arc::clone(&sink);
    let subjects = a.subjects.max(1);
    let depth = a.depth;
    let (ops, secs, hist) = run_threads(a, move |t, stop| {
        let mut h = histogram();
        let mut n: u64 = 0;
        let event = |seq: u64| {
            UsageEvent::new("io.loams.dev.usage.gateway.request.v1", format!("t{t}/{seq}"))
                .source("loams://bench/gateway/node-1")
                .subject(format!("org_{}", seq % subjects as u64))
                .tenant("42/7")
                .at_ns(1_761_000_000_000_000_000)
                .text("route", "/v1/query")
                .num("bytes_in", 512)
                .num("bytes_out", 2048)
                .num("status", 200)
        };
        let mut calls: u64 = 0;
        while !stop.load(Ordering::Relaxed) {
            if depth == 1 {
                let e = event(n);
                if calls % 64 == 0 {
                    let st = Instant::now();
                    s.emit(e);
                    let ns = u64::try_from(st.elapsed().as_nanos()).unwrap_or(u64::MAX);
                    h.record(ns.max(1)).expect("record");
                } else {
                    s.emit(e);
                }
            } else {
                // Pipelined: `depth` events per `emit_batch`; building them is
                // timed too, as it is in the depth-1 path.
                if calls % 16 == 0 {
                    let st = Instant::now();
                    s.emit_batch((n..n + depth as u64).map(&event).collect());
                    let ns = u64::try_from(st.elapsed().as_nanos()).unwrap_or(u64::MAX);
                    h.record(ns.max(1)).expect("record");
                } else {
                    s.emit_batch((n..n + depth as u64).map(&event).collect());
                }
            }
            calls += 1;
            n += depth as u64;
        }
        (n, h)
    });
    sink.close();
    let st = sink.stats();
    let produced = producer.events.load(Ordering::Relaxed);
    json!({
        "mode": "sink",
        "depth": a.depth,
        "errors": st.dropped_queue_full + st.dropped_invalid,
        "threads": a.threads,
        "cores": a.cores,
        "secs": secs,
        "emitted": ops,
        "emitted_per_sec": ops as f64 / secs,
        "produced": produced,
        "produced_per_sec": produced as f64 / secs,
        "produced_bytes": producer.bytes.load(Ordering::Relaxed),
        "batches": producer.batches.load(Ordering::Relaxed),
        "dropped_queue_full": st.dropped_queue_full,
        "emit_latency": latency_json(&hist),
    })
}

/// A local stand-in for the engine's produce route: HTTP/1.1 keep-alive,
/// reads each request's body and answers 200 `{}`.
fn mock_server(cores: Vec<usize>, requests: Arc<AtomicU64>, bytes: Arc<AtomicU64>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    std::thread::spawn(move || {
        for (i, conn) in listener.incoming().enumerate() {
            let Ok(conn) = conn else { continue };
            let (cores, requests, bytes) = (cores.clone(), Arc::clone(&requests), Arc::clone(&bytes));
            std::thread::spawn(move || {
                pin(&cores, i);
                serve(conn, &requests, &bytes);
            });
        }
    });
    format!("http://{addr}")
}

fn serve(conn: TcpStream, requests: &AtomicU64, bytes: &AtomicU64) {
    let _ = conn.set_nodelay(true);
    let mut writer = match conn.try_clone() {
        Ok(w) => w,
        Err(_) => return,
    };
    let mut reader = BufReader::new(conn);
    let mut line = String::new();
    loop {
        let mut length = 0usize;
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        loop {
            line.clear();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
            let header = line.trim_end();
            if header.is_empty() {
                break;
            }
            if let Some((k, v)) = header.split_once(':')
                && k.eq_ignore_ascii_case("content-length")
            {
                length = v.trim().parse().unwrap_or(0);
            }
        }
        let mut body = vec![0u8; length];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        requests.fetch_add(1, Ordering::Relaxed);
        bytes.fetch_add(length as u64, Ordering::Relaxed);
        let reply = b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n{}";
        if writer.write_all(reply).is_err() {
            return;
        }
    }
}

fn measurement(agent: usize, i: u64) -> Measurement {
    let start = time::OffsetDateTime::from_unix_timestamp(1_761_000_000).expect("time");
    Measurement {
        attribution: Attribution::new(format!("{}", 1 + i % 64), "7"),
        source: SourceKind::Cgroup,
        meter: Meter::CpuUsec,
        value: 1_000 + i,
        window_start: start,
        window_end: start + time::Duration::seconds(10),
        resource: format!("a{agent}-sandbox-{}", i % 512),
        counter: Some(10_000 + i),
        peak_bytes: None,
        cpu_usec: Some(1_000 + i),
        estimated: false,
    }
}

fn agent_mode(a: &Args) -> Value {
    let requests = Arc::new(AtomicU64::new(0));
    let bytes = Arc::new(AtomicU64::new(0));
    let url = mock_server(a.server_cores.clone(), Arc::clone(&requests), Arc::clone(&bytes));
    let (batch, partitions, spool, dir) = (a.batch, a.partitions, a.spool, a.dir.clone());
    std::fs::create_dir_all(&dir).expect("bench dir");
    let (ops, secs, hist) = run_threads(a, move |t, stop| {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let mut sink =
            StreamSink::new(StreamSinkConfig::new(url.clone(), partitions)).expect("sink");
        let spool_dir = dir.join(format!("agent-{t}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&spool_dir);
        let mut sp = spool.then(|| {
            std::fs::create_dir_all(&spool_dir).expect("spool dir");
            Spool::open(
                &spool_dir,
                SpoolConfig {
                    max_bytes: u64::MAX,
                    segment_bytes: 8 << 20,
                },
            )
            .expect("spool")
        });
        let node = format!("node-{t}");
        let mut h = histogram();
        let mut seq: u64 = 0;
        while !stop.load(Ordering::Relaxed) {
            let started = Instant::now();
            let events: Vec<MeterEvent> = (0..batch as u64)
                .map(|k| measurement(t, seq + k).into_event(&node, seq + k))
                .collect();
            rt.block_on(async {
                match sp.as_mut() {
                    Some(s) => {
                        s.append(&events).expect("append");
                        let mut left = events.len();
                        while left > 0 {
                            let n = forward(s, &mut sink, batch).await.expect("forward");
                            assert!(n > 0, "spool drained early");
                            left -= n;
                        }
                    }
                    None => sink.send(&events).await.expect("send"),
                }
            });
            seq += batch as u64;
            let ns = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
            h.record(ns.max(1)).expect("record");
        }
        drop(sp);
        let _ = std::fs::remove_dir_all(&spool_dir);
        (seq, h)
    });
    json!({
        "mode": "agent",
        "agents": a.threads,
        "cores": a.cores,
        "server_cores": a.server_cores,
        "batch": a.batch,
        "partitions": a.partitions,
        "spool": a.spool,
        "secs": secs,
        "events": ops,
        "events_per_sec": ops as f64 / secs,
        "http_requests": requests.load(Ordering::Relaxed),
        "http_body_bytes": bytes.load(Ordering::Relaxed),
        "batch_latency": latency_json(&hist),
    })
}

/// (busy, iowait, total) jiffies from /proc/stat's first line.
fn cpu_jiffies() -> (u64, u64, u64) {
    let stat = std::fs::read_to_string("/proc/stat").unwrap_or_default();
    let v: Vec<u64> = stat
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .skip(1)
        .filter_map(|x| x.parse().ok())
        .collect();
    let total: u64 = v.iter().sum();
    let idle = v.get(3).copied().unwrap_or(0);
    let iowait = v.get(4).copied().unwrap_or(0);
    (total.saturating_sub(idle + iowait), iowait, total)
}

/// This process's user + system CPU seconds (/proc/self/stat, 100 Hz).
fn process_cpu_secs() -> f64 {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let after = stat.rsplit_once(')').map_or("", |(_, r)| r);
    let f: Vec<&str> = after.split_whitespace().collect();
    let tick = |i: usize| f.get(i).and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0);
    // Fields 14 and 15 (utime, stime) are 12 and 13 after the command name.
    (tick(11) + tick(12)) / 100.0
}

fn main() {
    let a = args();
    let (busy0, io0, total0) = cpu_jiffies();
    let cpu0 = process_cpu_secs();
    let mut out = match a.mode.as_str() {
        "gate" => gate_mode(&a),
        "sink" => sink_mode(&a),
        "agent" => agent_mode(&a),
        other => {
            eprintln!("unknown mode {other}");
            std::process::exit(2)
        }
    };
    let host: BTreeMap<&str, Value> = BTreeMap::from([
        (
            "available_parallelism",
            json!(std::thread::available_parallelism().map_or(0, |n| n.get())),
        ),
        ("lane", json!("bq1-meter")),
    ]);
    out["host"] = json!(host);
    let (busy1, io1, total1) = cpu_jiffies();
    let cpu = process_cpu_secs() - cpu0;
    let ticks = (total1 - total0).max(1) as f64;
    let ncpu = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
    let secs = out["secs"].as_f64().unwrap_or(1.0);
    let ops = ["ops", "produced", "events"]
        .iter()
        .find_map(|k| out[*k].as_f64())
        .unwrap_or(0.0);
    out["system"] = json!({
        "loadavg": std::fs::read_to_string("/proc/loadavg").unwrap_or_default().trim(),
        "iowait_pct": 100.0 * (io1 - io0) as f64 / ticks,
        "machine_busy_cores": ncpu * (busy1 - busy0) as f64 / ticks,
        "process_cpu_secs": cpu,
        "process_busy_cores": cpu / secs,
        "ops_per_busy_core_sec": if cpu > 0.0 { ops / cpu } else { 0.0 },
    });
    println!("{out}");
}
