//! Load on the Temporal gateway's store API (lane BQ1-durable; the native
//! durable path of docs/benchmarks/2026-10-28-1b-qps/durable.md).
//!
//! ```text
//! store_load <start|commit> [--backend memory|postgres] [--url URL]
//!            [--depth N] [--duration-s S] [--warmup-s S] [--threads T]
//!            [--pool P] [--shards N] [--cores LABEL] [--drain]
//! ```
//!
//! - `start`: every operation is `Store::start` of a new run (three
//!   events, one outbox row, the visibility row), as `StartWorkflowExecution`
//!   commits it.
//! - `commit`: every operation is a compare-and-set `Store::commit` of two
//!   events, a new state blob and one outbox row on the caller's own run, as
//!   a completed workflow task commits it.
//!
//! `--depth` operations are in flight at once (one task each, so on
//! Postgres up to `--pool` transactions commit concurrently and Postgres
//! group-commits their WAL flushes). `--drain` runs a relay that claims and
//! marks the outbox, as the gateway does. One JSON line goes to stdout.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use loams_temporal_store::{
    Commit, EventRow, MemStore, OutboxInsert, PgStore, PgStoreOptions, RunKey, StartExecution,
    Store, VisibilityRow,
};

struct Args {
    op: String,
    backend: String,
    url: String,
    depth: usize,
    duration: Duration,
    warmup: Duration,
    threads: usize,
    pool: u32,
    shards: u32,
    cores: String,
    drain: bool,
}

fn args() -> Args {
    let mut a = Args {
        op: "commit".into(),
        backend: "memory".into(),
        url: std::env::var("LOAMS_TEMPORAL_TEST_PG_URL").unwrap_or_default(),
        depth: 64,
        duration: Duration::from_secs(20),
        warmup: Duration::from_secs(3),
        threads: 4,
        pool: 32,
        shards: 64,
        cores: String::new(),
        drain: true,
    };
    let mut it = std::env::args().skip(1);
    while let Some(x) = it.next() {
        let mut v = || it.next().unwrap_or_default();
        match x.as_str() {
            "--backend" => a.backend = v(),
            "--url" => a.url = v(),
            "--depth" => a.depth = v().parse().unwrap_or(64),
            "--duration-s" => a.duration = Duration::from_secs(v().parse().unwrap_or(20)),
            "--warmup-s" => a.warmup = Duration::from_secs(v().parse().unwrap_or(3)),
            "--threads" => a.threads = v().parse().unwrap_or(4),
            "--pool" => a.pool = v().parse().unwrap_or(32),
            "--shards" => a.shards = v().parse().unwrap_or(64),
            "--cores" => a.cores = v(),
            "--no-drain" => a.drain = false,
            "--drain" => a.drain = true,
            op if !op.starts_with('-') => a.op = op.to_string(),
            other => {
                eprintln!("unknown flag {other}");
                std::process::exit(2);
            }
        }
    }
    a
}

fn event(id: i64, batch_first: i64) -> EventRow {
    EventRow {
        event_id: id,
        batch_first_event_id: batch_first,
        event_type: 1,
        // About the size of a small workflow-task event.
        blob: vec![7u8; 160],
    }
}

fn outbox(id: &str) -> OutboxInsert {
    OutboxInsert {
        kind: "wft".into(),
        resonate_id: id.to_string(),
        request: serde_json::json!({"op": "create_promise", "id": id, "timeout_at_ms": 0}),
    }
}

fn start_of(key: RunKey, now_ms: i64) -> StartExecution {
    StartExecution {
        request_id: format!("req-{}", key.run_id),
        commit: Commit {
            events: (1..=3).map(|i| event(i, 1)).collect(),
            state_blob: vec![1u8; 512],
            status: 1,
            next_event_id: 4,
            outbox: vec![outbox(&format!("{}:wft.2", key.run_id))],
            visibility: Some(VisibilityRow {
                workflow_type: "bq1".into(),
                status: 1,
                start_time_ms: now_ms,
                close_time_ms: None,
                execution_time_ms: now_ms,
                task_queue: "bq1".into(),
                memo: None,
                search_attrs: serde_json::json!({}),
            }),
        },
        key,
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn pct(sorted: &[u64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = (p * (sorted.len() - 1) as f64) as usize;
    sorted[i] as f64 / 1e6
}

fn main() {
    let a = args();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(a.threads.max(1))
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(run(a));
}

async fn run(a: Args) {
    let store: Arc<dyn Store> = match a.backend.as_str() {
        "memory" => Arc::new(MemStore::new(a.shards).expect("memory store")),
        "postgres" => {
            let mut o = PgStoreOptions::new(a.shards);
            o.max_connections = a.pool.max(2);
            o.application_name = "bq1-store-load".into();
            let pg = PgStore::connect_url(&a.url, &o).await.expect("postgres");
            pg.migrate().await.expect("migrate");
            Arc::new(pg)
        }
        other => {
            eprintln!("unknown backend {other}");
            std::process::exit(2);
        }
    };
    let ns = format!("bq1-{}", now_ms());
    let stop = Arc::new(AtomicBool::new(false));
    let measuring = Arc::new(AtomicBool::new(false));
    let errors = Arc::new(AtomicU64::new(0));
    let seq = Arc::new(AtomicU64::new(0));
    let first_error = Arc::new(std::sync::Mutex::new(None::<String>));

    let drained = Arc::new(AtomicU64::new(0));
    if a.drain {
        let (s, stop, drained) = (store.clone(), stop.clone(), drained.clone());
        let shards: Vec<u32> = (0..a.shards).collect();
        tokio::spawn(async move {
            while !stop.load(Ordering::Relaxed) {
                match s
                    .claim_outbox(&shards, 512, Duration::from_secs(30), "bq1-relay")
                    .await
                {
                    Ok(rows) if rows.is_empty() => {
                        tokio::time::sleep(Duration::from_millis(2)).await
                    }
                    Ok(rows) => {
                        let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
                        if s.mark_dispatched(&ids).await.is_ok() {
                            drained.fetch_add(ids.len() as u64, Ordering::Relaxed);
                        }
                    }
                    Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
                }
            }
        });
    }

    let mut tasks = Vec::new();
    for w in 0..a.depth {
        let (store, stop, measuring, errors, seq, first_error) = (
            store.clone(),
            stop.clone(),
            measuring.clone(),
            errors.clone(),
            seq.clone(),
            first_error.clone(),
        );
        let (ns, op) = (ns.clone(), a.op.clone());
        tasks.push(tokio::spawn(async move {
            let mut lat: Vec<u64> = Vec::new();
            // The worker's own run for `commit`.
            let key = RunKey::new(&ns, format!("wf-{w}"), format!("run-{w}"));
            let mut version = 0u64;
            let mut next = 0i64;
            if op == "commit" {
                match store.start(start_of(key.clone(), now_ms())).await {
                    Ok(_) => {
                        version = 1;
                        next = 4;
                    }
                    Err(e) => {
                        eprintln!("start of worker {w}: {e}");
                        return lat;
                    }
                }
            }
            while !stop.load(Ordering::Relaxed) {
                let t = Instant::now();
                let r = if op == "start" {
                    let n = seq.fetch_add(1, Ordering::Relaxed);
                    let k = RunKey::new(&ns, format!("s-{w}-{n}"), format!("r-{w}-{n}"));
                    store.start(start_of(k, now_ms())).await.map(|_| ())
                } else {
                    let c = Commit {
                        events: vec![event(next, next), event(next + 1, next)],
                        state_blob: vec![2u8; 512],
                        status: 1,
                        next_event_id: next + 2,
                        outbox: vec![outbox(&format!("{}:wft.{}", key.run_id, next + 1))],
                        visibility: None,
                    };
                    match store.commit(&key, version, c).await {
                        Ok(v) => {
                            version = v;
                            next += 2;
                            Ok(())
                        }
                        Err(e) => Err(e),
                    }
                };
                let d = t.elapsed().as_nanos() as u64;
                if measuring.load(Ordering::Relaxed) {
                    match r {
                        Ok(()) => lat.push(d),
                        Err(e) => {
                            errors.fetch_add(1, Ordering::Relaxed);
                            let mut f = first_error.lock().unwrap_or_else(|p| p.into_inner());
                            if f.is_none() {
                                *f = Some(e.to_string());
                            }
                        }
                    }
                }
            }
            lat
        }));
    }
    tokio::time::sleep(a.warmup).await;
    let drained_before = drained.load(Ordering::Relaxed);
    measuring.store(true, Ordering::Relaxed);
    tokio::time::sleep(a.duration).await;
    measuring.store(false, Ordering::Relaxed);
    let drained_in = drained.load(Ordering::Relaxed) - drained_before;
    stop.store(true, Ordering::Relaxed);
    let mut all = Vec::new();
    for t in tasks {
        if let Ok(l) = t.await {
            all.extend(l);
        }
    }
    all.sort_unstable();
    let secs = a.duration.as_secs_f64();
    let line = serde_json::json!({
        "engine": "durable",
        "op": format!("temporal-store.{}", a.op),
        "backend": a.backend,
        "qps": all.len() as f64 / secs,
        "p50_ms": pct(&all, 0.50),
        "p99_ms": pct(&all, 0.99),
        "p999_ms": pct(&all, 0.999),
        "depth": a.depth,
        "cores": a.cores,
        "threads": a.threads,
        "errors": errors.load(Ordering::Relaxed),
        "first_error": first_error.lock().unwrap_or_else(|p| p.into_inner()).clone(),
        "outbox_drained_per_s": drained_in as f64 / secs,
        "pool": a.pool,
    });
    println!("{line}");
}
