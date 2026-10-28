//! `kv-loadgen`: a closed-loop RESP load generator for the BQ1 KV benchmark
//! (`docs/benchmarks/2026-10-28-1b-qps/kv.md`).
//!
//! The host has no `valkey-benchmark` or `memtier_benchmark`, so this is the
//! smallest tool that measures what they would: `--conns` connections spread
//! round-robin over one or more `--addr` (one per shard), each keeping one
//! batch of `--pipeline` requests in flight. A request is `GET` or `SET` of a
//! uniformly random key out of `--keys`, with `--set-ratio` the share of
//! `SET`s. The latency of a request is its batch's round trip, as
//! memtier reports it. Output is one JSON object on stdout.
//!
//! `--prefill` first writes every key once (pipelined), so `GET`s hit.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use clap::Parser;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[derive(Parser, Debug, Clone)]
#[command(about = "Closed-loop RESP GET/SET load generator")]
struct Args {
    /// Server addresses (one per shard); connections are spread round-robin.
    #[arg(long, value_delimiter = ',', required = true)]
    addr: Vec<SocketAddr>,
    /// Total connections.
    #[arg(long, default_value_t = 64)]
    conns: usize,
    /// Requests in flight per connection.
    #[arg(long, default_value_t = 1)]
    pipeline: usize,
    /// Measured seconds.
    #[arg(long, default_value_t = 20)]
    duration: u64,
    /// Unmeasured seconds before the measurement.
    #[arg(long, default_value_t = 3)]
    warmup: u64,
    /// Key space size.
    #[arg(long, default_value_t = 100_000)]
    keys: u64,
    /// Value size in bytes.
    #[arg(long, default_value_t = 64)]
    value_size: usize,
    /// Share of SETs (0.0 = all GET, 1.0 = all SET).
    #[arg(long, default_value_t = 0.0)]
    set_ratio: f64,
    /// Write every key once before measuring.
    #[arg(long)]
    prefill: bool,
    /// Tokio worker threads.
    #[arg(long, default_value_t = 4)]
    threads: usize,
    /// Send PING instead of GET (the protocol and network ceiling; counted
    /// as GETs).
    #[arg(long)]
    ping: bool,
    /// A free-form label copied into the output.
    #[arg(long, default_value = "")]
    label: String,
}

/// A log-linear latency histogram in nanoseconds: 32 sub-buckets per power
/// of two (about 3% relative precision).
#[derive(Clone)]
struct Hist {
    counts: Vec<u64>,
    total: u64,
    max: u64,
}

const SUB: u32 = 5;

impl Hist {
    fn new() -> Self {
        Self {
            counts: vec![0; 64 << SUB],
            total: 0,
            max: 0,
        }
    }

    fn index(v: u64) -> usize {
        let v = v.max(1);
        let msb = 63 - v.leading_zeros();
        if msb < SUB {
            return v as usize;
        }
        let sub = (v >> (msb - SUB)) & ((1 << SUB) - 1);
        (((msb - SUB + 1) << SUB) as u64 + sub) as usize
    }

    fn lower(i: usize) -> u64 {
        let i = i as u64;
        let group = i >> SUB;
        if group == 0 {
            return i;
        }
        let msb = group + u64::from(SUB) - 1;
        (1 << msb) | ((i & ((1 << SUB) - 1)) << (msb - u64::from(SUB)))
    }

    fn record(&mut self, ns: u64, n: u64) {
        self.counts[Self::index(ns)] += n;
        self.total += n;
        self.max = self.max.max(ns);
    }

    fn merge(&mut self, o: &Hist) {
        for (a, b) in self.counts.iter_mut().zip(&o.counts) {
            *a += b;
        }
        self.total += o.total;
        self.max = self.max.max(o.max);
    }

    fn quantile_us(&self, q: f64) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        let want = ((self.total as f64) * q).ceil().max(1.0) as u64;
        let mut seen = 0;
        for (i, c) in self.counts.iter().enumerate() {
            seen += c;
            if seen >= want {
                return Self::lower(i) as f64 / 1000.0;
            }
        }
        self.max as f64 / 1000.0
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

fn key(i: u64) -> String {
    format!("bq1:{i:012}")
}

fn push_cmd(buf: &mut Vec<u8>, parts: &[&[u8]]) {
    buf.extend_from_slice(format!("*{}\r\n", parts.len()).as_bytes());
    for p in parts {
        buf.extend_from_slice(format!("${}\r\n", p.len()).as_bytes());
        buf.extend_from_slice(p);
        buf.extend_from_slice(b"\r\n");
    }
}

/// Parses one reply at the start of `buf`: `Some((len, is_error))`, or `None`
/// when it is incomplete.
fn reply(buf: &[u8]) -> Option<(usize, bool)> {
    let line_end = buf.windows(2).position(|w| w == b"\r\n")?;
    let head = &buf[..line_end];
    match head.first()? {
        b'+' | b':' | b'_' | b'#' | b',' => Some((line_end + 2, false)),
        b'-' => Some((line_end + 2, true)),
        b'$' => {
            let n: i64 = std::str::from_utf8(&head[1..]).ok()?.parse().ok()?;
            if n < 0 {
                return Some((line_end + 2, false));
            }
            let total = line_end + 2 + n as usize + 2;
            (buf.len() >= total).then_some((total, false))
        }
        b'*' => {
            let n: i64 = std::str::from_utf8(&head[1..]).ok()?.parse().ok()?;
            let mut at = line_end + 2;
            for _ in 0..n.max(0) {
                let (len, _) = reply(&buf[at..])?;
                at += len;
            }
            Some((at, false))
        }
        _ => Some((line_end + 2, true)),
    }
}

#[derive(Default)]
struct ConnResult {
    get: Option<Hist>,
    set: Option<Hist>,
    gets: u64,
    sets: u64,
    errors: u64,
}

async fn run_conn(
    addr: SocketAddr,
    args: Arc<Args>,
    seed: u64,
    measuring: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
) -> std::io::Result<ConnResult> {
    let mut s = TcpStream::connect(addr).await?;
    s.set_nodelay(true)?;
    let value = vec![b'v'; args.value_size];
    let mut rng = Rng(seed | 1);
    let mut out = Vec::with_capacity(args.pipeline * (args.value_size + 64));
    let mut inbuf = vec![0u8; 64 * 1024];
    let mut pending: Vec<u8> = Vec::with_capacity(64 * 1024);
    let mut res = ConnResult {
        get: Some(Hist::new()),
        set: Some(Hist::new()),
        ..Default::default()
    };
    let threshold = (args.set_ratio * u64::MAX as f64) as u64;
    while !stop.load(Ordering::Relaxed) {
        out.clear();
        let mut n_set = 0u64;
        for _ in 0..args.pipeline {
            let k = key(rng.next() % args.keys);
            if args.set_ratio > 0.0 && rng.next() <= threshold {
                push_cmd(&mut out, &[b"SET", k.as_bytes(), &value]);
                n_set += 1;
            } else if args.ping {
                push_cmd(&mut out, &[b"PING"]);
            } else {
                push_cmd(&mut out, &[b"GET", k.as_bytes()]);
            }
        }
        let t0 = Instant::now();
        s.write_all(&out).await?;
        let mut got = 0;
        let mut errs = 0;
        while got < args.pipeline {
            let mut parsed = 0;
            while let Some((len, err)) = reply(&pending[parsed..]) {
                parsed += len;
                got += 1;
                errs += u64::from(err);
                if got == args.pipeline {
                    break;
                }
            }
            pending.drain(..parsed);
            if got == args.pipeline {
                break;
            }
            let n = s.read(&mut inbuf).await?;
            if n == 0 {
                return Err(std::io::Error::other("server closed the connection"));
            }
            pending.extend_from_slice(&inbuf[..n]);
        }
        let ns = t0.elapsed().as_nanos() as u64;
        if measuring.load(Ordering::Relaxed) {
            let n_get = args.pipeline as u64 - n_set;
            if n_get > 0 {
                res.get.as_mut().unwrap().record(ns, n_get);
            }
            if n_set > 0 {
                res.set.as_mut().unwrap().record(ns, n_set);
            }
            res.gets += n_get;
            res.sets += n_set;
            res.errors += errs;
        }
    }
    Ok(res)
}

async fn prefill(args: &Args) -> std::io::Result<()> {
    let value = vec![b'v'; args.value_size];
    let shards = args.addr.len() as u64;
    let per_task = 16u64;
    let mut tasks = Vec::new();
    for t in 0..per_task * shards {
        let addr = args.addr[(t % shards) as usize];
        let keys = args.keys;
        let total = per_task * shards;
        let value = value.clone();
        tasks.push(tokio::spawn(async move {
            let mut s = TcpStream::connect(addr).await?;
            s.set_nodelay(true)?;
            let mut i = t;
            let mut buf = vec![0u8; 64 * 1024];
            let mut pending = Vec::new();
            while i < keys {
                let mut out = Vec::new();
                let mut n = 0;
                while i < keys && n < 64 {
                    push_cmd(&mut out, &[b"SET", key(i).as_bytes(), &value]);
                    i += total;
                    n += 1;
                }
                s.write_all(&out).await?;
                let mut got = 0;
                while got < n {
                    let mut parsed = 0;
                    while let Some((len, _)) = reply(&pending[parsed..]) {
                        parsed += len;
                        got += 1;
                    }
                    pending.drain(..parsed);
                    if got < n {
                        let r = s.read(&mut buf).await?;
                        if r == 0 {
                            return Err(std::io::Error::other("closed during prefill"));
                        }
                        pending.extend_from_slice(&buf[..r]);
                    }
                }
            }
            Ok::<_, std::io::Error>(())
        }));
    }
    for t in tasks {
        t.await.map_err(std::io::Error::other)??;
    }
    Ok(())
}

fn summary(h: &Hist, ops: u64, secs: f64) -> serde_json::Value {
    serde_json::json!({
        "ops": ops,
        "qps": (ops as f64 / secs).round(),
        "p50_us": h.quantile_us(0.50),
        "p99_us": h.quantile_us(0.99),
        "p999_us": h.quantile_us(0.999),
        "max_us": h.max as f64 / 1000.0,
    })
}

fn main() -> std::io::Result<()> {
    let args = Arc::new(Args::parse());
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(args.threads)
        .enable_all()
        .build()?;
    rt.block_on(async move {
        let mut prefill_secs = 0.0;
        if args.prefill {
            let t = Instant::now();
            prefill(&args).await?;
            prefill_secs = t.elapsed().as_secs_f64();
        }
        let measuring = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let mut tasks = Vec::new();
        for c in 0..args.conns {
            let addr = args.addr[c % args.addr.len()];
            tasks.push(tokio::spawn(run_conn(
                addr,
                args.clone(),
                0x9E37_79B9_7F4A_7C15u64.wrapping_mul(c as u64 + 1),
                measuring.clone(),
                stop.clone(),
            )));
        }
        tokio::time::sleep(Duration::from_secs(args.warmup)).await;
        measuring.store(true, Ordering::Relaxed);
        let t0 = Instant::now();
        tokio::time::sleep(Duration::from_secs(args.duration)).await;
        measuring.store(false, Ordering::Relaxed);
        let secs = t0.elapsed().as_secs_f64();
        stop.store(true, Ordering::Relaxed);
        let (mut get, mut set) = (Hist::new(), Hist::new());
        let (mut gets, mut sets, mut errors, mut failed) = (0, 0, 0, 0);
        let mut first_error = None;
        for t in tasks {
            match t.await.map_err(std::io::Error::other)? {
                Ok(r) => {
                    get.merge(r.get.as_ref().unwrap());
                    set.merge(r.set.as_ref().unwrap());
                    gets += r.gets;
                    sets += r.sets;
                    errors += r.errors;
                }
                Err(e) => {
                    failed += 1;
                    first_error.get_or_insert(e.to_string());
                }
            }
        }
        let mut all = get.clone();
        all.merge(&set);
        let out = serde_json::json!({
            "label": args.label,
            "addr": args.addr.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "conns": args.conns,
            "pipeline": args.pipeline,
            "keys": args.keys,
            "value_size": args.value_size,
            "set_ratio": args.set_ratio,
            "ping": args.ping,
            "threads": args.threads,
            "duration_s": secs,
            "prefill_s": prefill_secs,
            "total": summary(&all, gets + sets, secs),
            "get": summary(&get, gets, secs),
            "set": summary(&set, sets, secs),
            "error_replies": errors,
            "failed_conns": failed,
            "first_error": first_error,
        });
        println!("{out}");
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn histogram_quantiles_are_within_precision() {
        let mut h = Hist::new();
        for v in 1..=100_000u64 {
            h.record(v * 1000, 1);
        }
        let p50 = h.quantile_us(0.5);
        assert!((p50 - 50_000.0).abs() / 50_000.0 < 0.04, "{p50}");
        let p99 = h.quantile_us(0.99);
        assert!((p99 - 99_000.0).abs() / 99_000.0 < 0.04, "{p99}");
    }

    #[test]
    fn replies_parse() {
        assert_eq!(reply(b"+OK\r\n"), Some((5, false)));
        assert_eq!(reply(b"$-1\r\n"), Some((5, false)));
        assert_eq!(reply(b"$3\r\nabc\r\n"), Some((9, false)));
        assert_eq!(reply(b"$3\r\nab"), None);
        assert_eq!(reply(b"-ERR x\r\n"), Some((8, true)));
        assert_eq!(reply(b"*2\r\n:1\r\n$1\r\na\r\n"), Some((15, false)));
    }
}
