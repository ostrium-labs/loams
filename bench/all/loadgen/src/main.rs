//! `bq1-native`: the closed-loop load generator of the BQ1 combined run
//! (`bench/all`, `bench/all/CONTRACT.md`), for the engines whose lanes had no
//! native load tool yet. Every subcommand drives one engine through its
//! native API and prints one contract JSON line per operation:
//!
//! - `kv`: the loams-kv `Store` API, on the embedded (in-memory redb) backend
//!   or on TiKV (a keyspace through PD). `get` and `put` are one-shot
//!   transactions at depth 1, and one transaction of `--depth` keys
//!   (`batch_get`, or `--depth` puts) above it (`depth_kind: "batch"`).
//! - `pg`: PG wire through tokio-postgres. `--depth` queries are in flight on
//!   each connection at once; tokio-postgres pipelines them on the socket
//!   (`depth_kind: "inflight"`).
//! - `mysql`: MySQL wire (TiDB). The classic protocol has no pipelining, so
//!   `--depth` is the number of ids in one `IN (...)` point lookup
//!   (`depth_kind: "batch"`).
//! - `http`: HTTP/2 with prior knowledge (h2c), `--depth` streams in flight on
//!   each connection: a GET of `--path` (the gateway's native API) or a
//!   Resonate `promise.create` (`--kind resonate-create`).
//!
//! The measured window is `--duration` seconds from `--start-at` (a Unix
//! second; the combined run aligns every engine on it) or from the end of
//! `--warmup`. Setup (connecting, creating tables, prefilling) happens first.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use bytes::Bytes;
use clap::{Parser, Subcommand, ValueEnum};
use futures::future::BoxFuture;
use http_body_util::{BodyExt, Full};
use hyper_util::rt::{TokioExecutor, TokioIo};
use loams_kv::{EmbeddedConfig, Store, StoreConfig, TikvConfig, TxnOptions};
use mysql_async::prelude::Queryable;
use serde_json::json;
use tokio::sync::Mutex;

mod hist;
use hist::Hist;

#[derive(Parser, Debug)]
#[command(about = "Closed-loop native-API load for the BQ1 combined run")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
    /// The `engine` field of the output.
    #[arg(long, global = true, default_value = "")]
    engine: String,
    /// Operations to run concurrently (connections are split between them).
    #[arg(long, global = true, value_delimiter = ',')]
    ops: Vec<String>,
    /// Measured seconds.
    #[arg(long, global = true, default_value_t = 30)]
    duration: u64,
    /// Unmeasured seconds after setup, when there is no `--start-at`.
    #[arg(long, global = true, default_value_t = 3)]
    warmup: u64,
    /// The Unix second the measured window starts at.
    #[arg(long, global = true)]
    start_at: Option<u64>,
    /// Pipelining depth (in flight per connection, or operations per request).
    #[arg(long, global = true, default_value_t = 1)]
    depth: usize,
    /// Connections (or, for `kv`, concurrent callers).
    #[arg(long, global = true, default_value_t = 16)]
    conns: usize,
    /// Tokio worker threads.
    #[arg(long, global = true, default_value_t = 1)]
    threads: usize,
    /// The `cores` field of the output (the size of the taskset list).
    #[arg(long, global = true, default_value_t = 1)]
    cores: usize,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// The loams-kv Store API.
    Kv {
        #[arg(long, value_enum, default_value_t = KvBackend::Embedded)]
        backend: KvBackend,
        /// PD endpoint (`host:port`), for `--backend tikv`.
        #[arg(long)]
        pd: Option<String>,
        #[arg(long, default_value = "bq1all")]
        keyspace: String,
        #[arg(long, default_value_t = 100_000)]
        keys: u64,
        #[arg(long, default_value_t = 64)]
        value_size: usize,
    },
    /// PG wire (tokio-postgres, pipelined).
    Pg {
        /// A libpq-style connection string.
        #[arg(long)]
        url: String,
        #[arg(long, default_value_t = 100_000)]
        rows: u64,
    },
    /// MySQL wire (TiDB).
    Mysql {
        /// `mysql://user:pass@host:port/`.
        #[arg(long)]
        url: String,
        #[arg(long, default_value_t = 100_000)]
        rows: u64,
    },
    /// HTTP/2 (h2c).
    Http {
        /// `host:port`.
        #[arg(long)]
        addr: String,
        #[arg(long, default_value = "/")]
        path: String,
        #[arg(long, value_enum, default_value_t = HttpKind::Get)]
        kind: HttpKind,
    },
}

#[derive(Copy, Clone, Debug, ValueEnum, PartialEq, Eq)]
enum KvBackend {
    Embedded,
    Tikv,
}

#[derive(Copy, Clone, Debug, ValueEnum, PartialEq, Eq)]
enum HttpKind {
    Get,
    ResonateCreate,
}

/// One call of an operation: `(task, random seed) -> operations done`.
type OpFn = Arc<dyn Fn(usize, u64) -> BoxFuture<'static, Result<u64>> + Send + Sync>;

/// An operation to drive: its name, its call, how many concurrent tasks run
/// it, and how many operations one call carries.
struct Op {
    name: String,
    call: OpFn,
    tasks: usize,
}

#[derive(Default)]
struct TaskStats {
    ops: u64,
    calls: u64,
    errors: u64,
    hist: Hist,
}

fn now_unix() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn mix(mut x: u64) -> u64 {
    // splitmix64
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn kv_key(i: u64) -> Vec<u8> {
    format!("k{i:010}").into_bytes()
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(cli.threads.max(1))
        .enable_all()
        .build()?;
    rt.block_on(run(cli))
}

async fn run(cli: Cli) -> Result<()> {
    let depth = cli.depth.max(1);
    let conns = cli.conns.max(1);
    let (default_ops, depth_kind, api): (&[&str], &str, String) = match &cli.cmd {
        Cmd::Kv { backend, .. } => (
            &["get"],
            "batch",
            match backend {
                KvBackend::Embedded => "loams-kv Store (embedded, in-memory redb)".into(),
                KvBackend::Tikv => "loams-kv Store on TiKV (txn, batch_get)".into(),
            },
        ),
        Cmd::Pg { .. } => (&["point-select"], "inflight", "PG wire (tokio-postgres, pipelined)".into()),
        Cmd::Mysql { .. } => (&["point-select"], "batch", "MySQL wire (IN-list batches)".into()),
        Cmd::Http { kind, path, .. } => match kind {
            HttpKind::Get => (&["get"], "inflight", format!("HTTP/2 GET {path}")),
            HttpKind::ResonateCreate => (&["promise.create"], "inflight", "Resonate HTTP/2 promise.create".into()),
        },
    };
    let names: Vec<String> = if cli.ops.is_empty() {
        default_ops.iter().map(|s| s.to_string()).collect()
    } else {
        cli.ops.clone()
    };
    let per_op_conns = (conns / names.len()).max(1);
    let mut ops = Vec::new();
    for name in &names {
        ops.push(build_op(&cli.cmd, name, per_op_conns, depth).await?);
    }

    // The window.
    let now = now_unix();
    let (start_in, late) = match cli.start_at {
        Some(at) => {
            let d = at as f64 - now;
            if d >= 0.0 { (d, 0.0) } else { (0.0, -d) }
        }
        None => (cli.warmup as f64, 0.0),
    };
    let t0 = Instant::now();
    let start = t0 + Duration::from_secs_f64(start_in);
    let end = start + Duration::from_secs(cli.duration);
    let t_start = now + start_in;
    eprintln!(
        "bq1-native: {} op(s), window starts in {start_in:.1} s for {} s",
        ops.len(),
        cli.duration
    );

    let seq = Arc::new(AtomicU64::new(1));
    let mut handles = Vec::new();
    for (oi, op) in ops.iter().enumerate() {
        for task in 0..op.tasks {
            let call = op.call.clone();
            let seq = seq.clone();
            handles.push((
                oi,
                tokio::spawn(async move {
                    let mut st = TaskStats::default();
                    let mut n: u64 = 0;
                    loop {
                        // An in-process engine can answer without ever
                        // returning Pending; yield now and then so every
                        // task (and every op) gets the runtime.
                        n += 1;
                        if n.is_multiple_of(16) {
                            tokio::task::yield_now().await;
                        }
                        let s = Instant::now();
                        if s >= end {
                            break;
                        }
                        let seed = mix(seq.fetch_add(1, Ordering::Relaxed));
                        let r = call(task, seed).await;
                        let e = Instant::now();
                        if s >= start {
                            st.calls += 1;
                            match r {
                                Ok(n) => {
                                    st.ops += n;
                                    st.hist.record(e.duration_since(s).as_nanos() as u64);
                                }
                                Err(err) => {
                                    if st.errors < 3 {
                                        eprintln!("bq1-native: {err:#}");
                                    }
                                    st.errors += 1;
                                }
                            }
                        } else if r.is_err() {
                            // Back off a failing warm-up instead of spinning.
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                    }
                    st
                }),
            ));
        }
    }
    let mut per_op: Vec<TaskStats> = ops.iter().map(|_| TaskStats::default()).collect();
    for (oi, h) in handles {
        let st = h.await?;
        let agg = &mut per_op[oi];
        agg.ops += st.ops;
        agg.calls += st.calls;
        agg.errors += st.errors;
        agg.hist.merge(&st.hist);
    }
    let window = cli.duration as f64;
    for (op, st) in ops.iter().zip(per_op) {
        let line = json!({
            "engine": cli.engine,
            "op": op.name,
            "qps": (st.ops as f64 / window).round(),
            "rps": (st.calls as f64 / window).round(),
            "p50_ms": st.hist.quantile_ms(0.50),
            "p99_ms": st.hist.quantile_ms(0.99),
            "p999_ms": st.hist.quantile_ms(0.999),
            "depth": depth,
            "depth_kind": depth_kind,
            "cores": cli.cores,
            "errors": st.errors,
            "conns": per_op_conns,
            "tasks": op.tasks,
            "window_s": window,
            "t_start": (t_start * 10.0).round() / 10.0,
            "t_end": ((t_start + window) * 10.0).round() / 10.0,
            "late_s": (late * 10.0_f64).round() / 10.0,
            "api": api,
        });
        println!("{line}");
    }
    Ok(())
}

async fn build_op(cmd: &Cmd, name: &str, conns: usize, depth: usize) -> Result<Op> {
    match cmd {
        Cmd::Kv { backend, pd, keyspace, keys, value_size } => {
            kv_op(*backend, pd.as_deref(), keyspace, *keys, *value_size, name, conns, depth).await
        }
        Cmd::Pg { url, rows } => pg_op(url, *rows, name, conns, depth).await,
        Cmd::Mysql { url, rows } => mysql_op(url, *rows, name, conns, depth).await,
        Cmd::Http { addr, path, kind } => http_op(addr, path, *kind, name, conns, depth).await,
    }
}

// ---- loams-kv ----

#[allow(clippy::too_many_arguments)]
async fn kv_op(
    backend: KvBackend,
    pd: Option<&str>,
    keyspace: &str,
    keys: u64,
    value_size: usize,
    name: &str,
    conns: usize,
    depth: usize,
) -> Result<Op> {
    let root = b"bq1all/".to_vec();
    let config = match backend {
        KvBackend::Embedded => StoreConfig::Embedded(EmbeddedConfig {
            root,
            ..EmbeddedConfig::in_memory("bq1-native", keyspace)
        }),
        KvBackend::Tikv => {
            let pd = pd.ok_or_else(|| anyhow!("--pd is needed for --backend tikv"))?;
            let mut c = TikvConfig::new(vec![pd.to_string()], keyspace);
            c.root = root;
            StoreConfig::Tikv(c)
        }
    };
    config.ensure_keyspace().await.context("create the keyspace")?;
    let store = Store::open(config).await.context("open the store")?;
    let value: Arc<Vec<u8>> = Arc::new(vec![b'v'; value_size]);
    // Prefill every key once (1000 per transaction), so gets hit.
    if store.get(&kv_key(keys - 1)).await?.is_none() {
        let started = Instant::now();
        let mut i = 0;
        while i < keys {
            let chunk: Arc<Vec<Vec<u8>>> = Arc::new((i..(i + 1000).min(keys)).map(kv_key).collect());
            let v = value.clone();
            store
                .run(TxnOptions::new("bq1.prefill"), move |txn| {
                    let (chunk, v) = (chunk.clone(), v.clone());
                    Box::pin(async move {
                        for k in chunk.iter() {
                            txn.put(k, v.to_vec()).await?;
                        }
                        Ok(())
                    })
                })
                .await
                .context("prefill")?;
            i += 1000;
        }
        eprintln!("bq1-native: prefilled {keys} keys in {:.1} s", started.elapsed().as_secs_f64());
    }
    let call: OpFn = match name {
        "get" => Arc::new(move |_task, seed| {
            let store = store.clone();
            Box::pin(async move {
                if depth == 1 {
                    store.get(&kv_key(seed % keys)).await?;
                    return Ok(1);
                }
                let ks: Arc<Vec<Vec<u8>>> =
                    Arc::new((0..depth as u64).map(|j| kv_key(mix(seed ^ j) % keys)).collect());
                store
                    .run(TxnOptions::new("kv.batch_get"), move |txn| {
                        let ks = ks.clone();
                        Box::pin(async move { txn.batch_get(ks.iter()).await })
                    })
                    .await?;
                Ok(depth as u64)
            })
        }),
        "put" => Arc::new(move |_task, seed| {
            let (store, value) = (store.clone(), value.clone());
            Box::pin(async move {
                if depth == 1 {
                    store.put(&kv_key(seed % keys), value.to_vec()).await?;
                    return Ok(1);
                }
                let ks: Arc<Vec<Vec<u8>>> =
                    Arc::new((0..depth as u64).map(|j| kv_key(mix(seed ^ j) % keys)).collect());
                store
                    .run(TxnOptions::new("kv.batch_put"), move |txn| {
                        let (ks, v) = (ks.clone(), value.clone());
                        Box::pin(async move {
                            for k in ks.iter() {
                                txn.put(k, v.to_vec()).await?;
                            }
                            Ok(())
                        })
                    })
                    .await?;
                Ok(depth as u64)
            })
        }),
        other => bail!("kv has no op {other} (get, put)"),
    };
    Ok(Op { name: name.into(), call, tasks: conns })
}

// ---- PG wire ----

async fn pg_connect(url: &str) -> Result<tokio_postgres::Client> {
    let (client, conn) = tokio_postgres::connect(url, tokio_postgres::NoTls)
        .await
        .context("connect (PG wire)")?;
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            eprintln!("bq1-native: pg connection: {e}");
        }
    });
    Ok(client)
}

async fn pg_op(url: &str, rows: u64, name: &str, conns: usize, depth: usize) -> Result<Op> {
    let admin = pg_connect(url).await?;
    admin
        .batch_execute(
            "CREATE TABLE IF NOT EXISTS bq1_accounts \
             (aid int PRIMARY KEY, abalance int NOT NULL, filler char(84) NOT NULL DEFAULT '')",
        )
        .await?;
    let have: i64 = admin.query_one("SELECT count(*) FROM bq1_accounts", &[]).await?.get(0);
    if have as u64 != rows {
        admin.batch_execute("TRUNCATE bq1_accounts").await?;
        admin
            .execute(
                "INSERT INTO bq1_accounts (aid, abalance) SELECT g, 0 FROM generate_series(1, $1::int) g",
                &[&(rows as i32)],
            )
            .await?;
        admin.batch_execute("VACUUM ANALYZE bq1_accounts").await?;
    }
    let sql = match name {
        "point-select" => "SELECT abalance FROM bq1_accounts WHERE aid = $1",
        "update" => "UPDATE bq1_accounts SET abalance = abalance + 1 WHERE aid = $1",
        other => bail!("pg has no op {other} (point-select, update)"),
    };
    let mut clients = Vec::new();
    for _ in 0..conns {
        let c = pg_connect(url).await?;
        let stmt = c.prepare(sql).await?;
        clients.push(Arc::new((c, stmt)));
    }
    let clients = Arc::new(clients);
    let select = name == "point-select";
    let call: OpFn = Arc::new(move |task, seed| {
        let cs = clients[task % clients.len()].clone();
        Box::pin(async move {
            let aid = (seed % rows) as i32 + 1;
            if select {
                cs.0.query_one(&cs.1, &[&aid]).await?;
            } else {
                cs.0.execute(&cs.1, &[&aid]).await?;
            }
            Ok(1)
        })
    });
    Ok(Op { name: name.into(), call, tasks: conns * depth })
}

// ---- MySQL wire ----

async fn mysql_op(url: &str, rows: u64, name: &str, conns: usize, depth: usize) -> Result<Op> {
    if name != "point-select" {
        bail!("mysql has no op {name} (point-select)");
    }
    let opts = mysql_async::Opts::from_url(url).context("--url")?;
    let mut admin = mysql_async::Conn::new(opts.clone()).await.context("connect (MySQL wire)")?;
    admin.query_drop("CREATE DATABASE IF NOT EXISTS bq1").await?;
    admin
        .query_drop("CREATE TABLE IF NOT EXISTS bq1.sbtest1 (id INT PRIMARY KEY, k INT NOT NULL, c CHAR(120) NOT NULL)")
        .await?;
    let have: Option<u64> = admin.query_first("SELECT count(*) FROM bq1.sbtest1").await?;
    if have.unwrap_or(0) != rows {
        admin.query_drop("TRUNCATE TABLE bq1.sbtest1").await?;
        let started = Instant::now();
        let mut i = 1;
        while i <= rows {
            let hi = (i + 999).min(rows);
            let values: Vec<String> = (i..=hi).map(|id| format!("({id},{},'c{id:0119}')", id % 1000)).collect();
            admin
                .query_drop(format!("INSERT INTO bq1.sbtest1 (id, k, c) VALUES {}", values.join(",")))
                .await?;
            i = hi + 1;
        }
        eprintln!("bq1-native: loaded {rows} rows in {:.1} s", started.elapsed().as_secs_f64());
    }
    drop(admin);
    let placeholders = vec!["?"; depth].join(",");
    let sql = format!("SELECT c FROM bq1.sbtest1 WHERE id IN ({placeholders})");
    let mut slots = Vec::new();
    for _ in 0..conns {
        let mut c = mysql_async::Conn::new(opts.clone()).await?;
        let stmt = c.prep(sql.as_str()).await?;
        slots.push(Arc::new(Mutex::new((c, stmt))));
    }
    let slots = Arc::new(slots);
    let call: OpFn = Arc::new(move |task, seed| {
        let slot = slots[task % slots.len()].clone();
        Box::pin(async move {
            let ids: Vec<mysql_async::Value> = (0..depth as u64)
                .map(|j| mysql_async::Value::from(mix(seed ^ j) % rows + 1))
                .collect();
            let mut g = slot.lock().await;
            let (conn, stmt) = &mut *g;
            let got: Vec<String> = conn.exec(&*stmt, mysql_async::Params::Positional(ids)).await?;
            if got.is_empty() {
                bail!("point select found nothing");
            }
            Ok(depth as u64)
        })
    });
    Ok(Op { name: name.into(), call, tasks: conns })
}

// ---- HTTP/2 ----

type Sender = hyper::client::conn::http2::SendRequest<Full<Bytes>>;

async fn h2_connect(addr: &str) -> Result<Sender> {
    let tcp = tokio::net::TcpStream::connect(addr)
        .await
        .with_context(|| format!("connect {addr}"))?;
    tcp.set_nodelay(true)?;
    let (sender, conn) = hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(tcp))
        .await
        .context("h2c handshake")?;
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            eprintln!("bq1-native: h2 connection: {e}");
        }
    });
    Ok(sender)
}

async fn http_op(addr: &str, path: &str, kind: HttpKind, name: &str, conns: usize, depth: usize) -> Result<Op> {
    let mut senders = Vec::new();
    for _ in 0..conns {
        senders.push(h2_connect(addr).await?);
    }
    let senders = Arc::new(senders);
    let uri: Arc<str> = format!("http://{addr}{path}").into();
    let run = format!("{:x}", now_unix() as u64);
    let call: OpFn = Arc::new(move |task, seed| {
        let mut sender = senders[task % senders.len()].clone();
        let uri = uri.clone();
        let run = run.clone();
        Box::pin(async move {
            let req = match kind {
                HttpKind::Get => http::Request::get(&*uri).body(Full::new(Bytes::new()))?,
                HttpKind::ResonateCreate => {
                    let now_ms = (now_unix() * 1000.0) as u64;
                    let body = json!({
                        "kind": "promise.create",
                        "head": {"corrId": format!("{seed:x}"), "version": "2026-04-01"},
                        "data": {
                            "id": format!("bq1all.{run}.{task}.{seed:x}"),
                            "timeoutAt": now_ms + 3_600_000,
                            "param": {"headers": {}, "data": ""},
                            "tags": {},
                        },
                    });
                    http::Request::post(&*uri)
                        .header("content-type", "application/json")
                        .body(Full::new(Bytes::from(serde_json::to_vec(&body)?)))?
                }
            };
            sender.ready().await?;
            let resp = sender.send_request(req).await?;
            let status = resp.status();
            let body = resp.into_body().collect().await?.to_bytes();
            if !status.is_success() {
                bail!("HTTP {status}: {}", String::from_utf8_lossy(&body[..body.len().min(200)]));
            }
            if kind == HttpKind::ResonateCreate {
                let v: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
                if let Some(s) = v.pointer("/head/status").and_then(|s| s.as_u64())
                    && s >= 300
                {
                    bail!("resonate status {s}: {}", String::from_utf8_lossy(&body[..body.len().min(200)]));
                }
            }
            Ok(1)
        })
    });
    Ok(Op { name: name.into(), call, tasks: conns * depth })
}
