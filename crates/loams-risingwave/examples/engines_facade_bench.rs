//! BQ1-stream's control-plane benchmark host (docs/benchmarks/2026-10-28-1b-qps/stream.md).
//!
//! Serves `loams_engines::http::router` with the real RisingWave adapter
//! ([`RisingWaveEngine`]) over in-memory seams (no runtime, a ready prober), plus
//! `loams-engines`' fake Flink and Spark adapters, so the numbers are the facade's and the
//! adapter's own cost: routing, JSON, the instance store, the phase machine, the adapter's
//! config rendering.
//!
//! ```text
//! engines_facade_bench serve <addr> <instances>       # oha against GET routes
//! engines_facade_bench creates <n> <concurrency>      # POST create in-process, JSON result
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode};
use loams_engines::http::router;
use loams_engines::testing::FakeEngine;
use loams_engines::{
    CreateInstance, EngineKind, EngineRegistry, EngineService, SizeRequest, SizeTier,
};
use loams_risingwave::adapter::{AdapterConfig, Seams};
use loams_risingwave::credentials::MemoryCredentialStore;
use loams_risingwave::probe::FixedProber;
use loams_risingwave::provision::MemoryProvisioner;
use loams_risingwave::runtime::MemoryRuntime;
use loams_risingwave::{RisingWaveAdapter, RisingWaveEngine};
use tower::ServiceExt;

const NAMESPACE: &str = "bench";

fn service() -> Arc<EngineService> {
    let prov = MemoryProvisioner::default();
    let seams = Seams {
        meta_db: Arc::new(prov.clone()),
        object_store: Arc::new(prov),
        runtime: Arc::new(MemoryRuntime::default()),
        prober: Arc::new(FixedProber::ready()),
        credentials: Arc::new(MemoryCredentialStore::default()),
    };
    let rw = RisingWaveEngine::new(RisingWaveAdapter::new(
        AdapterConfig {
            bootstrap_users: false,
            ready_timeout: Duration::from_secs(1),
            poll_interval: Duration::from_millis(1),
            ..AdapterConfig::default()
        },
        seams,
    ));
    let registry = EngineRegistry::new()
        .with(Arc::new(rw))
        .with(Arc::new(FakeEngine::new(EngineKind::Flink)))
        .with(Arc::new(FakeEngine::new(EngineKind::Spark)));
    Arc::new(EngineService::new(registry))
}

fn request(name: &str) -> CreateInstance {
    CreateInstance {
        name: name.to_owned(),
        kind: EngineKind::Risingwave,
        version: None,
        size: SizeRequest {
            tier: SizeTier::Xs,
            replicas: None,
        },
        config: Default::default(),
        labels: Default::default(),
    }
}

async fn serve(addr: &str, instances: usize) -> Result<(), Box<dyn std::error::Error>> {
    let svc = service();
    let mut first = None;
    for i in 0..instances {
        let inst = svc
            .create(NAMESPACE, None, request(&format!("rw-{i}")))
            .await?;
        first.get_or_insert(inst.id.0.clone());
    }
    let listener = tokio::net::TcpListener::bind(addr).await?;
    eprintln!(
        "serving {} instances on http://{}; one id: {}",
        instances,
        listener.local_addr()?,
        first.unwrap_or_default()
    );
    axum::serve(listener, router(svc)).await?;
    Ok(())
}

async fn creates(n: usize, concurrency: usize) -> Result<(), Box<dyn std::error::Error>> {
    let app = router(service());
    let next = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let started = Instant::now();
    let mut tasks = Vec::new();
    for _ in 0..concurrency.max(1) {
        let app = app.clone();
        let next = next.clone();
        tasks.push(tokio::spawn(async move {
            let mut lat = Vec::new();
            let mut failed = 0usize;
            loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if i >= n {
                    break;
                }
                let body = serde_json::to_vec(&request(&format!("c{i}"))).unwrap_or_default();
                let req = Request::builder()
                    .method(Method::POST)
                    .uri(format!("/v1/namespaces/{NAMESPACE}/engines"))
                    .header("content-type", "application/json")
                    .body(Body::from(body));
                let Ok(req) = req else {
                    failed += 1;
                    continue;
                };
                let t = Instant::now();
                match app.clone().oneshot(req).await {
                    Ok(res) => {
                        let ok = res.status().is_success();
                        let _ = to_bytes(res.into_body(), 1 << 20).await;
                        if ok {
                            lat.push(t.elapsed());
                        } else {
                            failed += 1;
                        }
                    }
                    Err(_) => failed += 1,
                }
            }
            (lat, failed)
        }));
    }
    let mut lat = Vec::new();
    let mut failed = 0;
    for t in tasks {
        let (l, f) = t.await?;
        lat.extend(l);
        failed += f;
    }
    let secs = started.elapsed().as_secs_f64();
    lat.sort_unstable();
    let pct = |q: f64| {
        lat.get(((lat.len().max(1) - 1) as f64 * q) as usize)
            .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
    };
    let out = serde_json::json!({
        "mode": "creates",
        "instances": n,
        "concurrency": concurrency,
        "ok": lat.len(),
        "failed": failed,
        "seconds": secs,
        "creates_per_s": lat.len() as f64 / secs,
        "latency": { "p50_ms": pct(0.5), "p99_ms": pct(0.99), "p999_ms": pct(0.999) },
        "status_ok": StatusCode::OK.as_u16(),
    });
    println!("{out}");
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let threads: usize = std::env::var("BENCH_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(threads)
        .enable_all()
        .build()?;
    match args.get(1).map(String::as_str) {
        Some("serve") => {
            let addr = args.get(2).map_or("127.0.0.1:18090", String::as_str);
            let n = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(100);
            rt.block_on(serve(addr, n))
        }
        Some("creates") => {
            let n = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(10_000);
            let c = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(8);
            rt.block_on(creates(n, c))
        }
        _ => Err(
            "usage: engines_facade_bench serve <addr> <instances> | creates <n> <concurrency>"
                .into(),
        ),
    }
}
