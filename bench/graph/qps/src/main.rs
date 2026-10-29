//! `graph-qps-bench` (BQ1, 2026-10-28): query throughput and latency of Loams Graph, the
//! embedded Grafeo engine behind `loams.graph.v1`, on its hot path: [`Graph::execute_with_params`]
//! (the statement gate, the engine, and the row conversion), called from `--threads` OS threads.
//! There is no network in this path: the Connect-RPC layer is measured by BQ1-gw.
//!
//! ```text
//! graph-qps-bench --workload point|hop1|hop2|hop3|recall|mix --threads 6 --shards 1
//!   --persons 200000 --degree 8 --users 1000 --facts 200 --duration 20 --warmup 3
//! ```
//!
//! The data set, per shard (each shard is its own graph, as a namespace's graph would be):
//!
//! * `persons / shards` `(:Person {uid, name})` nodes, each with `degree` outgoing `:KNOWS` edges
//!   to uniformly random persons of the same shard;
//! * `users / shards` `(:User {uid})` nodes, each with `facts` `(:Fact {topic, ts, text})` nodes
//!   behind `:REMEMBERS`, topics drawn from 16, and every fact `:ABOUT` one random person.
//!
//! Property indexes on `Person.uid`, `User.uid` and `Fact.topic`. The workloads:
//!
//! | Workload | Statement |
//! |---|---|
//! | `point` | `MATCH (p:Person {uid: $id}) RETURN p.name` |
//! | `hop1` | `MATCH (p:Person {uid: $id})-[:KNOWS]->(f) RETURN count(f)` |
//! | `hop2` | two `:KNOWS` hops, `count(DISTINCT f)` |
//! | `hop3` | three `:KNOWS` hops, `count(DISTINCT f)` |
//! | `recall` | a user's 10 newest facts on a topic, with the person each is about |
//! | `mix` | 60 % point, 20 % hop1, 10 % hop2, 10 % recall |
//!
//! Prints one JSON line: queries per second, latency p50/p99/p99.9 in microseconds, the
//! engine's own share of the time (`GraphResult::elapsed_nanos`) and the load time.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use clap::Parser;
use grafeo::Value;
use loams_graph::{Access, Engine, Graph, OpenSpec};
use loams_proto::loams::graph::v1::QueryLanguage;

#[derive(Debug, Parser)]
#[command(name = "graph-qps-bench", about = "Loams Graph query throughput (BQ1)")]
struct Args {
    #[arg(long, value_enum, default_value_t = Workload::Point)]
    workload: Workload,
    /// Query threads (each runs statements back to back).
    #[arg(long, default_value_t = 6)]
    threads: usize,
    /// Independent graphs; thread `t` queries shard `t % shards`.
    #[arg(long, default_value_t = 1)]
    shards: usize,
    #[arg(long, default_value_t = 200_000)]
    persons: u64,
    #[arg(long, default_value_t = 8)]
    degree: u64,
    #[arg(long, default_value_t = 1_000)]
    users: u64,
    /// Facts per user.
    #[arg(long, default_value_t = 200)]
    facts: u64,
    /// Seconds measured.
    #[arg(long, default_value_t = 20)]
    duration: u64,
    /// Seconds run before measuring.
    #[arg(long, default_value_t = 3)]
    warmup: u64,
    /// Keys per request for the batched workloads (`point-batch`, `hop1-batch`): pipelining
    /// depth in BQ1's sense. 1 is the unbatched baseline.
    #[arg(long, default_value_t = 1)]
    batch: usize,
    /// `native`: Grafeo's own session API (`Session::execute_with_params`), the headline path.
    /// `loams`: `Graph::execute_with_params`, Loams's statement gate (with its verdict cache) in
    /// front of the same call. `gate`: the uncached gate (`classify::gate_verdict`) and then the
    /// native call, which is what `loams` cost before the verdict cache (BQ1 before/after).
    #[arg(long, value_enum, default_value_t = Path::Native)]
    path: Path,
    /// Instead of measuring: print each workload's plan and the time of a few single queries.
    #[arg(long)]
    probe: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
enum Path {
    Native,
    Loams,
    Gate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
enum Workload {
    Point,
    Hop1,
    Hop2,
    Hop3,
    Recall,
    Mix,
    /// `--batch` point lookups in one statement (`uid IN $ids`).
    PointBatch,
    /// `--batch` one-hop counts in one statement.
    Hop1Batch,
}

const TOPICS: u64 = 16;

const POINT: &str = "MATCH (p:Person {uid: $id}) RETURN p.name";
const HOP1: &str = "MATCH (p:Person {uid: $id})-[:KNOWS]->(f) RETURN count(f) AS n";
const HOP2: &str =
    "MATCH (p:Person {uid: $id})-[:KNOWS]->()-[:KNOWS]->(f) RETURN count(DISTINCT f) AS n";
const HOP3: &str = "MATCH (p:Person {uid: $id})-[:KNOWS]->()-[:KNOWS]->()-[:KNOWS]->(f) \
                    RETURN count(DISTINCT f) AS n";
const POINT_BATCH: &str = "MATCH (p:Person) WHERE p.uid IN $ids RETURN p.uid, p.name";
const HOP1_BATCH: &str =
    "MATCH (p:Person)-[:KNOWS]->(f) WHERE p.uid IN $ids RETURN p.uid, count(f) AS n";
const RECALL: &str = "MATCH (u:User {uid: $id})-[:REMEMBERS]->(m:Fact {topic: $topic})-[:ABOUT]->(p) \
                      RETURN m.text, m.ts, p.name ORDER BY m.ts DESC LIMIT 10";

/// A small, fast PRNG (splitmix64): the generator must not be what is measured.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

/// Runs a load or DDL statement on an admin session, outside the measured path.
fn admin(graph: &Graph, statement: &str) -> Result<(), String> {
    graph
        .session_for(Access::Admin)
        .execute(statement)
        .map(|_| ())
        .map_err(|e| format!("{e}: {}", &statement[..statement.len().min(160)]))
}

/// Builds one shard's data set. Returns the shard's (persons, users).
fn load(graph: &Graph, args: &Args, shard: u64) -> Result<(u64, u64), String> {
    let persons = (args.persons / args.shards as u64).max(1);
    let users = (args.users / args.shards as u64).max(1);
    // Bulk load through the session's node and edge API: a GQL `UNWIND … MATCH` load binds
    // its MATCH values from the list, which the planner cannot turn into an index lookup, so it
    // scans every node per row (seconds per thousand edges at 5 000 nodes).
    let session = graph.session_for(Access::Admin);
    let mut rng = Rng(0x5EED ^ shard);
    let mut person_ids = Vec::with_capacity(persons as usize);
    for uid in 0..persons {
        let id = session
            .create_node_with_props(
                &["Person"],
                [
                    ("uid", Value::Int64(uid as i64)),
                    ("name", Value::from(format!("p{uid}"))),
                ],
            )
            .map_err(|e| e.to_string())?;
        person_ids.push(id);
    }
    for &a in &person_ids {
        for _ in 0..args.degree {
            let b = person_ids[rng.below(persons) as usize];
            session.create_edge(a, b, "KNOWS");
        }
    }
    for uid in 0..users {
        let user = session
            .create_node_with_props(&["User"], [("uid", Value::Int64(uid as i64))])
            .map_err(|e| e.to_string())?;
        for i in 0..args.facts {
            let fact = session
                .create_node_with_props(
                    &["Fact"],
                    [
                        ("topic", Value::Int64(rng.below(TOPICS) as i64)),
                        ("ts", Value::Int64(1_700_000_000 + i as i64 * 60)),
                        ("text", Value::from("fact")),
                    ],
                )
                .map_err(|e| e.to_string())?;
            session.create_edge(user, fact, "REMEMBERS");
            session.create_edge(fact, person_ids[rng.below(persons) as usize], "ABOUT");
        }
    }
    // Indexes after the load: Grafeo builds a property index from the nodes present.
    for (name, label, prop) in [
        ("person_uid", "Person", "uid"),
        ("user_uid", "User", "uid"),
        ("fact_topic", "Fact", "topic"),
    ] {
        admin(
            graph,
            &format!("CREATE INDEX {name} FOR (n:{label}) ON (n.{prop})"),
        )?;
    }
    Ok((persons, users))
}

/// One query of the workload, its statement and parameters.
fn pick(
    workload: Workload,
    rng: &mut Rng,
    persons: u64,
    users: u64,
    batch: usize,
) -> (&'static str, HashMap<String, Value>) {
    if matches!(workload, Workload::PointBatch | Workload::Hop1Batch) {
        let ids: Vec<Value> = (0..batch.max(1))
            .map(|_| Value::Int64(rng.below(persons) as i64))
            .collect();
        let mut params = HashMap::new();
        params.insert("ids".to_string(), Value::from(ids));
        let statement = if workload == Workload::PointBatch {
            POINT_BATCH
        } else {
            HOP1_BATCH
        };
        return (statement, params);
    }
    let w = match workload {
        Workload::Mix => match rng.below(10) {
            0..=5 => Workload::Point,
            6 | 7 => Workload::Hop1,
            8 => Workload::Hop2,
            _ => Workload::Recall,
        },
        w => w,
    };
    let mut params = HashMap::new();
    let statement = match w {
        Workload::Recall => {
            params.insert("id".to_string(), Value::Int64(rng.below(users) as i64));
            params.insert("topic".to_string(), Value::Int64(rng.below(TOPICS) as i64));
            RECALL
        }
        other => {
            params.insert("id".to_string(), Value::Int64(rng.below(persons) as i64));
            match other {
                Workload::Point => POINT,
                Workload::Hop1 => HOP1,
                Workload::Hop2 => HOP2,
                Workload::Hop3 => HOP3,
                _ => unreachable!("batched workloads return above"),
            }
        }
    };
    (statement, params)
}

fn quantile(sorted: &[u64], q: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    sorted[((sorted.len() - 1) as f64 * q).round() as usize]
}

fn rss_bytes() -> u64 {
    std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok())
        .map_or(0, |pages| pages * 4096)
}

/// Plans and single-query times, for finding out why a workload is slow.
fn probe(graph: &Graph, (persons, users): (u64, u64)) {
    let limits = loams_graph::StatementLimits::DEFAULT;
    let mut rng = Rng(7);
    for w in [
        Workload::Point,
        Workload::Hop1,
        Workload::Hop2,
        Workload::Hop3,
        Workload::Recall,
    ] {
        let (statement, _) = pick(w, &mut rng, persons, users, 1);
        match graph.explain_within(statement, &limits) {
            Ok(plan) => eprintln!("== {w:?}\n{}", plan.text),
            Err(e) => eprintln!("== {w:?}: explain failed: {e}"),
        }
        for _ in 0..3 {
            let (statement, params) = pick(w, &mut rng, persons, users, 1);
            let t0 = Instant::now();
            let out = graph.execute_with_params(statement, params, true);
            eprintln!(
                "   {:?} in {:?}: {:?}",
                w,
                t0.elapsed(),
                out.map(|r| (r.rows.len(), r.elapsed_nanos))
            );
        }
    }
    for statement in [
        "MATCH (p:Person {uid: 123}) RETURN p.name",
        "MATCH (p:Person) WHERE p.uid = 123 RETURN p.name",
        "MATCH (p {uid: 123}) RETURN p.name",
    ] {
        for _ in 0..3 {
            let t0 = Instant::now();
            let out = graph.execute(statement, true);
            eprintln!(
                "   literal {statement:?} in {:?}: {:?}",
                t0.elapsed(),
                out.map(|r| (r.rows.len(), r.elapsed_nanos))
            );
        }
    }
}

fn main() {
    let args = Args::parse();
    let engine = Engine::new();
    let load_started = Instant::now();
    let mut shards = Vec::new();
    for s in 0..args.shards {
        let graph = Graph::open(&engine, "bench", &format!("s{s}"), OpenSpec::default())
            .expect("open a graph");
        let sizes = load(&graph, &args, s as u64).unwrap_or_else(|e| {
            eprintln!("loading shard {s}: {e}");
            std::process::exit(2)
        });
        shards.push((graph, sizes));
    }
    let load_s = load_started.elapsed().as_secs_f64();
    let (nodes, edges) = shards.iter().fold((0, 0), |(n, e), (g, _)| {
        let (gn, ge) = g.counts();
        (n + gn, e + ge)
    });
    eprintln!("loaded {nodes} nodes and {edges} edges in {load_s:.1} s");

    if args.probe {
        probe(&shards[0].0, shards[0].1);
        return;
    }
    let shards = Arc::new(shards);
    let measuring = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let workers: Vec<_> = (0..args.threads)
        .map(|t| {
            let shards = Arc::clone(&shards);
            let measuring = Arc::clone(&measuring);
            let stop = Arc::clone(&stop);
            let workload = args.workload;
            let batch = args.batch;
            let path = args.path;
            std::thread::spawn(move || {
                let (graph, (persons, users)) = &shards[t % shards.len()];
                let session = graph.session_for(Access::Read);
                let limits = loams_graph::StatementLimits::DEFAULT;
                let mut rng = Rng(0xC0FFEE ^ (t as u64).wrapping_mul(0x1000_0001));
                let mut lat = Vec::with_capacity(1 << 20);
                let (mut errors, mut engine_ns, mut rows) = (0u64, 0u64, 0u64);
                while !stop.load(Ordering::Relaxed) {
                    let (statement, params) = pick(workload, &mut rng, *persons, *users, batch);
                    let t0 = Instant::now();
                    let out = match path {
                        Path::Loams => graph
                            .execute_with_params(statement, params, true)
                            .map(|r| (r.rows.len(), r.elapsed_nanos.unwrap_or(0)))
                            .map_err(|e| e.to_string()),
                        Path::Gate => loams_graph::classify::gate_verdict(
                            statement,
                            QueryLanguage::Gql,
                            &limits,
                        )
                        .map_err(|e| e.to_string())
                        .and_then(|_| {
                            session
                                .execute_with_params(statement, params)
                                .map(|r| {
                                    let ms = r.execution_time_ms().unwrap_or(0.0);
                                    (r.row_count(), (ms * 1e6) as u64)
                                })
                                .map_err(|e| e.to_string())
                        }),
                        Path::Native => session
                            .execute_with_params(statement, params)
                            .map(|r| {
                                let ms = r.execution_time_ms().unwrap_or(0.0);
                                (r.row_count(), (ms * 1e6) as u64)
                            })
                            .map_err(|e| e.to_string()),
                    };
                    let ns = t0.elapsed().as_nanos() as u64;
                    if !measuring.load(Ordering::Relaxed) {
                        continue;
                    }
                    match out {
                        Ok((n, eng)) => {
                            lat.push(ns / 1000);
                            engine_ns += eng;
                            rows += n as u64;
                        }
                        Err(e) => {
                            if errors == 0 {
                                eprintln!("thread {t}: {e}");
                            }
                            errors += 1;
                        }
                    }
                }
                (lat, errors, engine_ns, rows)
            })
        })
        .collect();
    std::thread::sleep(Duration::from_secs(args.warmup));
    measuring.store(true, Ordering::Relaxed);
    let started = Instant::now();
    std::thread::sleep(Duration::from_secs(args.duration));
    measuring.store(false, Ordering::Relaxed);
    let elapsed = started.elapsed().as_secs_f64();
    stop.store(true, Ordering::Relaxed);
    let mut lat = Vec::new();
    let (mut errors, mut engine_ns, mut rows) = (0, 0, 0);
    for w in workers {
        let (l, e, en, r) = w.join().expect("worker");
        lat.extend(l);
        errors += e;
        engine_ns += en;
        rows += r;
    }
    lat.sort_unstable();
    let n = lat.len() as f64;
    let total_us: u64 = lat.iter().sum();
    let workload = format!("{:?}", args.workload).to_lowercase();
    let line = serde_json::json!({
        "format": "loams.bench.bq1.graph/1",
        "engine": "grafeo",
        "engine_version": loams_graph::ENGINE_VERSION,
        "workload": workload,
        "path": format!("{:?}", args.path).to_lowercase(),
        "threads": args.threads,
        "shards": args.shards,
        "persons": args.persons,
        "degree": args.degree,
        "users": args.users,
        "facts_per_user": args.facts,
        "nodes": nodes,
        "edges": edges,
        "load_s": load_s,
        "duration_s": elapsed,
        "batch": args.batch,
        "queries": lat.len(),
        "errors": errors,
        "qps": n / elapsed,
        "lookups_per_s": if matches!(args.workload, Workload::PointBatch | Workload::Hop1Batch) {
            n * args.batch as f64 / elapsed
        } else {
            n / elapsed
        },
        "p50_us": quantile(&lat, 0.5),
        "p99_us": quantile(&lat, 0.99),
        "p999_us": quantile(&lat, 0.999),
        "mean_us": if n > 0.0 { total_us as f64 / n } else { 0.0 },
        "engine_share": if total_us > 0 { engine_ns as f64 / 1000.0 / total_us as f64 } else { 0.0 },
        "rows_per_query": if n > 0.0 { rows as f64 / n } else { 0.0 },
        "rss_bytes": rss_bytes(),
    });
    println!("{line}");
}
