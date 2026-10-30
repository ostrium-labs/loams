//! `kv-native`: the BQ1 KV benchmark through the native APIs
//! (`docs/benchmarks/2026-10-28-1b-qps/kv.md`; the owner's "native APIs
//! only" rule): no RESP, no CQL.
//!
//! Backends:
//! - `memory`: `loams_kv::Store` on the embedded backend's in-memory redb
//!   (no file, no fsync);
//! - `embedded`: `loams_kv::Store` on a redb file (group commit, fsync);
//! - `tikv`: `loams_kv::Store` on a TiKV keyspace (Percolator transactions,
//!   async commit + 1PC);
//! - `tikv-raw`: TiKV's RawKV API (`RawClient::batch_get`/`batch_put`) on a
//!   keyspace, no transactions.
//!
//! A request is one call carrying `--depth` keys: `get` is one transaction
//! doing `batch_get` of the keys (raw: one `batch_get`), `put` is one
//! transaction writing them (raw: one `batch_put`); `mix` is 90% `get`
//! requests. `--concurrency` requests are in flight. The output is one JSON
//! line in the BQ1-ALL format (`bench/all/CONTRACT.md`): `qps` counts keys
//! (operations) per second and `rps` calls per second.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use clap::Parser;
use loams_kv::{EmbeddedConfig, Store, StoreConfig, TikvConfig, TxnOptions};
use loams_tikv::tikv_client::{Config as RawConfig, RawClient};

#[path = "../../hist.rs"]
mod hist;
use hist::Hist;

#[derive(Parser, Debug, Clone)]
#[command(about = "Native-API KV benchmark (loams-kv and TiKV RawKV)")]
struct Args {
    /// memory | embedded | tikv | tikv-raw
    #[arg(long, default_value = "memory")]
    backend: String,
    /// get | put | mix (90% get requests)
    #[arg(long, default_value = "get")]
    op: String,
    /// Keys per request (the batch, or pipeline, depth).
    #[arg(long, default_value_t = 1)]
    depth: usize,
    /// Requests in flight.
    #[arg(long, default_value_t = 256)]
    concurrency: usize,
    /// Tokio worker threads (the core budget; pin with taskset).
    #[arg(long, default_value_t = 4)]
    threads: usize,
    #[arg(long, default_value_t = 15)]
    duration: u64,
    #[arg(long, default_value_t = 3)]
    warmup: u64,
    #[arg(long, default_value_t = 100_000)]
    keys: u64,
    #[arg(long, default_value_t = 64)]
    value_size: usize,
    /// Write every key before measuring (always done for get and mix).
    #[arg(long)]
    prefill: bool,
    #[arg(long, default_value = "127.0.0.1:19379")]
    pd: String,
    #[arg(long, default_value = "loams_test_live")]
    keyspace: String,
    /// The redb file of `embedded`.
    #[arg(long, default_value = "/mnt/Projects/bench-data/bq1-kv/native.redb")]
    path: String,
    /// The `cores` field of the output (the caller's taskset budget).
    #[arg(long, default_value_t = 0)]
    cores: usize,
}

#[derive(Clone)]
enum Backend {
    Kv(Store),
    Raw(RawClient, Vec<u8>),
}

fn key(i: u64) -> Vec<u8> {
    format!("k{i:010}").into_bytes()
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

impl Backend {
    async fn get(&self, keys: Vec<Vec<u8>>) -> Result<usize, String> {
        match self {
            Backend::Kv(store) => store
                .run(TxnOptions::new("bq1.get"), move |t| {
                    let keys = keys.clone();
                    Box::pin(async move { Ok(t.batch_get(keys.iter()).await?.len()) })
                })
                .await
                .map(|c| c.value)
                .map_err(|e| e.to_string()),
            Backend::Raw(raw, prefix) => raw
                .batch_get(keys.into_iter().map(|k| [prefix.as_slice(), &k].concat()))
                .await
                .map(|v| v.len())
                .map_err(|e| e.to_string()),
        }
    }

    async fn put(&self, keys: Vec<Vec<u8>>, value: &[u8]) -> Result<usize, String> {
        let n = keys.len();
        match self {
            Backend::Kv(store) => {
                let value = value.to_vec();
                store
                    .run(TxnOptions::new("bq1.put"), move |t| {
                        let (keys, value) = (keys.clone(), value.clone());
                        Box::pin(async move {
                            for k in &keys {
                                t.put(k, value.clone()).await?;
                            }
                            Ok(keys.len())
                        })
                    })
                    .await
                    .map(|c| c.value)
                    .map_err(|e| e.to_string())
            }
            Backend::Raw(raw, prefix) => raw
                .batch_put(
                    keys.into_iter()
                        .map(|k| ([prefix.as_slice(), &k].concat(), value.to_vec())),
                )
                .await
                .map(|()| n)
                .map_err(|e| e.to_string()),
        }
    }
}

async fn open(args: &Args) -> Result<Backend, String> {
    let run_root = format!("bq1n/{}/", std::process::id()).into_bytes();
    let kv = |c| async move {
        Store::open(c)
            .await
            .map(Backend::Kv)
            .map_err(|e| e.to_string())
    };
    match args.backend.as_str() {
        "memory" => {
            let mut c = EmbeddedConfig::in_memory(&format!("bq1-{}", std::process::id()), "kv");
            c.root = run_root;
            kv(StoreConfig::Embedded(c)).await
        }
        "embedded" => {
            let mut c = EmbeddedConfig::new(&args.path, "kv");
            c.root = run_root;
            kv(StoreConfig::Embedded(c)).await
        }
        "tikv" => {
            let mut c = TikvConfig::new(vec![args.pd.clone()], args.keyspace.clone());
            c.root = run_root;
            kv(StoreConfig::Tikv(c)).await
        }
        "tikv-raw" => {
            let config = RawConfig::default().with_keyspace(&args.keyspace);
            RawClient::new_with_config(vec![args.pd.clone()], config)
                .await
                .map(|r| Backend::Raw(r, run_root))
                .map_err(|e| e.to_string())
        }
        other => Err(format!("unknown backend {other}")),
    }
}

async fn prefill(b: &Backend, args: &Args) -> Result<(), String> {
    let value = vec![b'v'; args.value_size];
    let tasks = 32u64;
    let mut handles = Vec::new();
    for t in 0..tasks {
        let (b, value, keys) = (b.clone(), value.clone(), args.keys);
        handles.push(tokio::spawn(async move {
            let mut i = t * 256;
            while i < keys {
                let batch: Vec<Vec<u8>> = (i..(i + 256).min(keys)).map(key).collect();
                b.put(batch, &value).await?;
                i += tasks * 256;
            }
            Ok::<_, String>(())
        }));
    }
    for h in handles {
        h.await.map_err(|e| e.to_string())??;
    }
    Ok(())
}

#[derive(Default)]
struct Res {
    hist: Hist,
    requests: u64,
    ops: u64,
    errors: u64,
    first_error: Option<String>,
}

async fn worker(
    b: Backend,
    args: Arc<Args>,
    seed: u64,
    measuring: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
) -> Res {
    let mut rng = Rng(seed | 1);
    let value = vec![b'v'; args.value_size];
    let mut res = Res::default();
    while !stop.load(Ordering::Relaxed) {
        let keys: Vec<Vec<u8>> = (0..args.depth)
            .map(|_| key(rng.next() % args.keys))
            .collect();
        let is_put = match args.op.as_str() {
            "put" => true,
            "mix" => rng.next() % 10 == 0,
            _ => false,
        };
        let t0 = Instant::now();
        let r = if is_put {
            b.put(keys, &value).await
        } else {
            b.get(keys).await
        };
        let ns = u64::try_from(t0.elapsed().as_nanos()).unwrap_or(u64::MAX);
        if measuring.load(Ordering::Relaxed) {
            res.requests += 1;
            match r {
                Ok(_) => {
                    res.ops += args.depth as u64;
                    res.hist.record(ns, 1);
                }
                Err(e) => {
                    res.errors += 1;
                    res.first_error.get_or_insert(e);
                }
            }
        }
    }
    res
}

fn main() -> Result<(), String> {
    let args = Arc::new(Args::parse());
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(args.threads)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async move {
        let b = open(&args).await?;
        if args.prefill || args.op != "put" {
            prefill(&b, &args).await?;
        }
        let measuring = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let handles: Vec<_> = (0..args.concurrency)
            .map(|c| {
                tokio::spawn(worker(
                    b.clone(),
                    args.clone(),
                    0x9E37_79B9_7F4A_7C15u64.wrapping_mul(c as u64 + 1),
                    measuring.clone(),
                    stop.clone(),
                ))
            })
            .collect();
        tokio::time::sleep(Duration::from_secs(args.warmup)).await;
        measuring.store(true, Ordering::Relaxed);
        let t0 = Instant::now();
        tokio::time::sleep(Duration::from_secs(args.duration)).await;
        measuring.store(false, Ordering::Relaxed);
        let secs = t0.elapsed().as_secs_f64();
        stop.store(true, Ordering::Relaxed);
        let mut all = Res::default();
        for h in handles {
            let r = h.await.map_err(|e| e.to_string())?;
            all.hist.merge(&r.hist);
            all.requests += r.requests;
            all.ops += r.ops;
            all.errors += r.errors;
            if all.first_error.is_none() {
                all.first_error = r.first_error;
            }
        }
        let ms = |q| all.hist.quantile_us(q) / 1000.0;
        let line = serde_json::json!({
            "engine": "kv",
            "op": format!("{}.{}", args.backend, args.op),
            "qps": (all.ops as f64 / secs).round(),
            "rps": (all.requests as f64 / secs).round(),
            "p50_ms": ms(0.50),
            "p99_ms": ms(0.99),
            "p999_ms": ms(0.999),
            "depth": args.depth,
            "cores": if args.cores > 0 { args.cores } else { args.threads },
            "errors": all.errors,
            "concurrency": args.concurrency,
            "value_size": args.value_size,
            "keys": args.keys,
            "first_error": all.first_error,
        });
        println!("{line}");
        if args.backend == "embedded" {
            let _ = std::fs::remove_file(&args.path);
        }
        Ok(())
    })
}
