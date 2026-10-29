//! `git-wal-bench`: drives `BucketRefLog` directly (no git processes) and
//! prints one JSON line with commits per second, commit and push latency,
//! the mean group size, and store PUTs and GETs per commit (design §36
//! §4.4; plan GT1 Task 11, the morning version's Task 13).
//!
//! ```text
//! git-wal-bench --store <url> [--opt key=value]... --repos <n> --pushers <p>
//!   --duration <s> --pack-bytes <b> [--put-delay-ms <d>] [--sorted-listing]
//!   [--mode push|ref-read|ref-read-cached|fetch] [--refs <n>] [--packs <n>]
//!   [--get-delay-ms <d>]
//! ```
//!
//! Modes (BQ1, 2026-10-28): `push` (the default) is the ref update path
//! above. `ref-read` takes a linearizable snapshot (`ReadAt::Latest`, one
//! store read of the WAL tail each) and resolves one ref; `ref-read-cached`
//! takes `ReadAt::AtLeast` the seeded tail, which the sequencer answers from
//! memory; `fetch` reads whole packs back with `get_range`, the object path a
//! `git fetch` takes after negotiation. `--pushers` is the number of
//! concurrent readers in the read modes. `--get-delay-ms` models a remote
//! store's GET latency.
//!
//! `--put-delay-ms` adds that latency to every segment append and pack
//! upload, the model of a remote store's PUT. Pusher `j` updates its own
//! branch of repository `j % repos`, uploading a pack of `--pack-bytes`
//! first when that is not 0.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use clap::Parser;
use loams_git::format::pb;
use loams_git::wal::{Appended, Hint, WalError, WalStore};
use loams_git::{
    BlobId, BlobStore, BucketRefLog, Expect, IdemKey, NamespaceId, ObjectId, Principal, ReadAt,
    RefLog, RefName, RefTxn, RefUpdate, RepoId, RepoPaths, Seq, SequencerConfig, StoreBlobStore,
    StoreWalStore, SystemClock, WalBatch, WalSegment, create_repo,
};
use loams_store::{FaultyStore, Op, Store};
use rand::{Rng, RngCore};

#[derive(Debug, Parser)]
#[command(name = "git-wal-bench", about = "Measures the Loams Git bucket WAL")]
struct Args {
    /// The store URL (`memory:///`, `file:///dir`, `s3://bucket/prefix`, …).
    #[arg(long)]
    store: String,
    /// A backend option, `key=value` (for example `aws_endpoint=http://…`).
    #[arg(long = "opt")]
    opts: Vec<String>,
    /// The store lists keys in order (memory, GCS, Azure, AWS S3).
    #[arg(long)]
    sorted_listing: bool,
    /// Repositories.
    #[arg(long, default_value_t = 1)]
    repos: usize,
    /// Concurrent pushers.
    #[arg(long, default_value_t = 32)]
    pushers: usize,
    /// Seconds to run.
    #[arg(long, default_value_t = 30)]
    duration: u64,
    /// Bytes of the pack each push uploads (0: none).
    #[arg(long, default_value_t = 0)]
    pack_bytes: usize,
    /// Latency added to every segment append and pack upload, in ms.
    #[arg(long, default_value_t = 0)]
    put_delay_ms: u64,
    /// Latency added to every WAL read and pack read, in ms (read modes).
    #[arg(long, default_value_t = 0)]
    get_delay_ms: u64,
    /// What to measure.
    #[arg(long, value_enum, default_value_t = Mode::Push)]
    mode: Mode,
    /// Refs seeded per repository before a read mode runs.
    #[arg(long, default_value_t = 1000)]
    refs: usize,
    /// Packs of `--pack-bytes` seeded per repository before `fetch` runs.
    #[arg(long, default_value_t = 16)]
    packs: usize,
    /// Pipelining depth (BQ1): refs updated by one push (`push`), refs resolved from one
    /// snapshot (`ref-read*`), or packs read concurrently by one request (`fetch`). 1 is the
    /// unbatched baseline; ops count every ref or pack, requests every call.
    #[arg(long, default_value_t = 1)]
    batch: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
enum Mode {
    Push,
    RefRead,
    RefReadCached,
    Fetch,
}

/// A WAL whose appends take `delay` longer, and reads `read_delay`.
#[derive(Debug)]
struct Delayed {
    inner: StoreWalStore,
    delay: Duration,
    read_delay: Duration,
}

#[async_trait]
impl WalStore for Delayed {
    async fn append(&self, expect_seq: Seq, batch: &WalBatch) -> Result<Appended, WalError> {
        tokio::time::sleep(self.delay).await;
        self.inner.append(expect_seq, batch).await
    }

    async fn read_from(&self, from: Seq, max: usize) -> Result<Vec<WalSegment>, WalError> {
        if !self.read_delay.is_zero() {
            tokio::time::sleep(self.read_delay).await;
        }
        self.inner.read_from(from, max).await
    }

    async fn hint(&self) -> Result<Hint, WalError> {
        self.inner.hint().await
    }

    async fn publish_hint(&self, hint: &Hint) -> Result<(), WalError> {
        self.inner.publish_hint(hint).await
    }

    async fn put_checkpoint(&self, checkpoint: &pb::Checkpoint) -> Result<(), WalError> {
        self.inner.put_checkpoint(checkpoint).await
    }

    async fn latest_checkpoint(
        &self,
        at_or_below: Seq,
    ) -> Result<Option<pb::Checkpoint>, WalError> {
        self.inner.latest_checkpoint(at_or_below).await
    }
}

fn random_hex(bytes: usize) -> String {
    let mut raw = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut raw);
    raw.iter().map(|b| format!("{b:02x}")).collect()
}

fn random_oid() -> ObjectId {
    let mut raw = [0u8; 20];
    rand::rng().fill_bytes(&mut raw);
    ObjectId::try_from(&raw[..]).expect("20 bytes")
}

/// The `q` quantile of sorted `values`, in ms.
fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let at = ((sorted.len() as f64 - 1.0) * q).round() as usize;
    sorted[at]
}

/// One pusher's loop; returns (commit ms, push ms) per acknowledged push
/// and the count of refused or failed ones.
async fn pusher(
    id: usize,
    log: BucketRefLog,
    blobs: Arc<StoreBlobStore>,
    deadline: Instant,
    pack_bytes: usize,
    delay: Duration,
    batch: usize,
) -> (Vec<(f64, f64)>, u64) {
    let names: Vec<RefName> = (0..batch.max(1))
        .map(|k| RefName::parse(&format!("refs/heads/p{id}-{k}")).expect("ref"))
        .collect();
    let principal = Principal::parse(&format!("pusher-{id}")).expect("principal");
    let mut current: Vec<Option<ObjectId>> = vec![None; names.len()];
    let mut done = Vec::new();
    let mut failed = 0;
    while Instant::now() < deadline {
        let pushed = Instant::now();
        let mut packs = Vec::new();
        if pack_bytes > 0 {
            let mut body = vec![0u8; pack_bytes];
            rand::rng().fill_bytes(&mut body);
            let id = BlobId::parse(&random_hex(20)).expect("blob id");
            tokio::time::sleep(delay).await;
            if let Err(err) = blobs.put(&id, Bytes::from(body)).await {
                eprintln!("pusher {id}: uploading pack {}: {err}", id.as_str());
                failed += 1;
                continue;
            }
            packs.push(pb::PackRef {
                checksum: id.as_str().to_string(),
                object_count: 1,
                pack_len: pack_bytes as u64,
                idx_offset: pack_bytes as u64,
                idx_len: 0,
                has_bitmap: false,
            });
        }
        let new: Vec<ObjectId> = names.iter().map(|_| random_oid()).collect();
        let mut digest = [0u8; 32];
        rand::rng().fill_bytes(&mut digest);
        let txn = RefTxn {
            key: IdemKey::parse(&format!("b{}", random_hex(12))).expect("key"),
            request_digest: digest,
            updates: names
                .iter()
                .zip(&current)
                .zip(&new)
                .map(|((name, cur), new)| RefUpdate {
                    name: name.clone(),
                    expect: cur.map_or(Expect::Absent, Expect::Oid),
                    new: Some(*new),
                })
                .collect(),
            packs,
            principal: principal.clone(),
            scope: None,
            trace: None,
            push_options: Vec::new(),
        };
        let committing = Instant::now();
        match log.commit(txn).await {
            Ok(_) => {
                current = new.into_iter().map(Some).collect();
                done.push((
                    committing.elapsed().as_secs_f64() * 1e3,
                    pushed.elapsed().as_secs_f64() * 1e3,
                ));
            }
            Err(err) => {
                failed += 1;
                eprintln!("pusher {id}: {err}");
            }
        }
    }
    (done, failed)
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let opts: Vec<(String, String)> = args
        .opts
        .iter()
        .map(|opt| {
            let (key, value) = opt.split_once('=').expect("--opt takes key=value");
            (key.to_string(), value.to_string())
        })
        .collect();
    let base = Store::from_url(&args.store, opts).expect("open the store");
    let counted = Arc::new(FaultyStore::new(Arc::clone(base.inner())));
    let store = Store::new(counted.clone()).with_sorted_listing(args.sorted_listing);
    let delay = Duration::from_millis(args.put_delay_ms);
    let run = random_hex(4);
    let ns = NamespaceId(rand::rng().random_range(1..u64::from(u32::MAX)));

    let mut logs = Vec::new();
    let mut blob_stores = Vec::new();
    for i in 0..args.repos {
        let repo = RepoId::parse(&format!("rbench{run}{i}")).expect("repo id");
        let paths = RepoPaths::new(ns, &repo);
        let wal = Arc::new(Delayed {
            inner: StoreWalStore::new(store.clone(), paths.clone()),
            delay,
            read_delay: Duration::from_millis(args.get_delay_ms),
        });
        create_repo(wal.as_ref(), pb::ObjectFormat::Sha1)
            .await
            .expect("create a repository");
        let blobs = Arc::new(StoreBlobStore::new(store.clone(), paths));
        let config = SequencerConfig::new(format!("bench/{ns}"), ns, repo);
        let log = BucketRefLog::open(wal, blobs.clone(), config, Arc::new(SystemClock))
            .await
            .expect("open a repository");
        logs.push(log);
        blob_stores.push(blobs);
    }

    if args.mode != Mode::Push {
        read_modes(&args, &logs, &blob_stores, &counted).await;
        for log in logs {
            log.shutdown().await;
        }
        return;
    }

    let (puts, gets) = (counted.calls(Op::Put), counted.calls(Op::Get));
    let started = Instant::now();
    let deadline = started + Duration::from_secs(args.duration);
    let tasks: Vec<_> = (0..args.pushers)
        .map(|j| {
            let repo = j % args.repos;
            tokio::spawn(pusher(
                j,
                logs[repo].clone(),
                blob_stores[repo].clone(),
                deadline,
                args.pack_bytes,
                delay,
                args.batch,
            ))
        })
        .collect();
    let mut commit_ms = Vec::new();
    let mut push_ms = Vec::new();
    let mut failed = 0;
    for task in tasks {
        let (done, errors) = task.await.expect("pusher");
        failed += errors;
        for (commit, push) in done {
            commit_ms.push(commit);
            push_ms.push(push);
        }
    }
    let elapsed = started.elapsed().as_secs_f64();
    let (puts, gets) = (counted.calls(Op::Put) - puts, counted.calls(Op::Get) - gets);
    let segments: u64 = logs.iter().map(|log| log.seq().0).sum();
    for log in logs {
        log.shutdown().await;
    }
    commit_ms.sort_by(f64::total_cmp);
    push_ms.sort_by(f64::total_cmp);
    let commits = commit_ms.len() as f64;
    let per_commit = |n: u64| {
        if commits > 0.0 {
            n as f64 / commits
        } else {
            0.0
        }
    };
    let line = serde_json::json!({
        "store": args.store,
        "repos": args.repos,
        "pushers": args.pushers,
        "duration_s": args.duration,
        "pack_bytes": args.pack_bytes,
        "put_delay_ms": args.put_delay_ms,
        "commits": commit_ms.len(),
        "failed": failed,
        "batch": args.batch,
        "commits_per_s": commits / elapsed,
        "ref_updates_per_s": commits * args.batch.max(1) as f64 / elapsed,
        "commit_p50_ms": quantile(&commit_ms, 0.5),
        "commit_p99_ms": quantile(&commit_ms, 0.99),
        "commit_p999_ms": quantile(&commit_ms, 0.999),
        "push_p50_ms": quantile(&push_ms, 0.5),
        "push_p99_ms": quantile(&push_ms, 0.99),
        "segments": segments,
        "mean_group": if segments > 0 { commits / segments as f64 } else { 0.0 },
        "puts_per_commit": per_commit(puts),
        "gets_per_commit": per_commit(gets),
    });
    println!("{line}");
}

/// Seeds `--refs` refs per repository in batches of 100 updates per
/// transaction; returns each repository's ref names and its tail.
async fn seed_refs(logs: &[BucketRefLog], refs: usize) -> Vec<(Vec<RefName>, Seq)> {
    let principal = Principal::parse("seeder").expect("principal");
    let mut out = Vec::new();
    for log in logs {
        let names: Vec<RefName> = (0..refs)
            .map(|i| RefName::parse(&format!("refs/heads/b{i}")).expect("ref"))
            .collect();
        let mut tail = Seq(0);
        for chunk in names.chunks(100) {
            let mut digest = [0u8; 32];
            rand::rng().fill_bytes(&mut digest);
            let txn = RefTxn {
                key: IdemKey::parse(&format!("s{}", random_hex(12))).expect("key"),
                request_digest: digest,
                updates: chunk
                    .iter()
                    .map(|name| RefUpdate {
                        name: name.clone(),
                        expect: Expect::Absent,
                        new: Some(random_oid()),
                    })
                    .collect(),
                packs: Vec::new(),
                principal: principal.clone(),
                scope: None,
                trace: None,
                push_options: Vec::new(),
            };
            log.commit(txn).await.expect("seed refs");
            tail = log.seq();
        }
        out.push((names, tail));
    }
    out
}

/// The read modes: `--pushers` readers per run, spread over the repositories,
/// for `--duration` seconds; prints one JSON line like the push mode.
async fn read_modes(
    args: &Args,
    logs: &[BucketRefLog],
    blobs: &[Arc<StoreBlobStore>],
    counted: &Arc<FaultyStore>,
) {
    let seeded = seed_refs(logs, args.refs).await;
    let mut packs: Vec<Vec<BlobId>> = vec![Vec::new(); logs.len()];
    if args.mode == Mode::Fetch {
        assert!(args.pack_bytes > 0, "fetch needs --pack-bytes");
        for (repo, store) in blobs.iter().enumerate() {
            for _ in 0..args.packs {
                let mut body = vec![0u8; args.pack_bytes];
                rand::rng().fill_bytes(&mut body);
                let id = BlobId::parse(&random_hex(20)).expect("blob id");
                store
                    .put(&id, Bytes::from(body))
                    .await
                    .expect("seed a pack");
                packs[repo].push(id);
            }
        }
    }
    let get_delay = Duration::from_millis(args.get_delay_ms);
    let gets_before = counted.calls(Op::Get);
    let started = Instant::now();
    let deadline = started + Duration::from_secs(args.duration);
    let tasks: Vec<_> = (0..args.pushers)
        .map(|j| {
            let repo = j % logs.len();
            let log = logs[repo].clone();
            let store = Arc::clone(&blobs[repo]);
            let (names, tail) = seeded[repo].clone();
            let packs = packs[repo].clone();
            let mode = args.mode;
            let pack_len = args.pack_bytes as u64;
            let batch = args.batch.max(1);
            tokio::spawn(async move {
                let mut lat = Vec::new();
                let mut failed = 0u64;
                let mut bytes = 0u64;
                let mut i = j;
                while Instant::now() < deadline {
                    i = i
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    let t = Instant::now();
                    let ok = match mode {
                        Mode::RefRead | Mode::RefReadCached => {
                            let at = if mode == Mode::RefRead {
                                ReadAt::Latest
                            } else {
                                ReadAt::AtLeast(tail)
                            };
                            match log.snapshot(at).await {
                                Ok(snap) => (0..batch).all(|k| {
                                    let at = (i >> 17).wrapping_add(k * 7919) % names.len();
                                    snap.refs.contains_key(&names[at])
                                }),
                                Err(err) => {
                                    eprintln!("reader {j}: {err}");
                                    false
                                }
                            }
                        }
                        Mode::Fetch => {
                            // `batch` packs requested at once, as a fetch of several packs
                            // issues its range reads concurrently.
                            let reads = (0..batch).map(|k| {
                                let id = &packs[(i >> 17).wrapping_add(k) % packs.len()];
                                let store = &store;
                                async move {
                                    if !get_delay.is_zero() {
                                        tokio::time::sleep(get_delay).await;
                                    }
                                    store.get_range(id, 0..pack_len).await
                                }
                            });
                            let mut ok = true;
                            for got in futures::future::join_all(reads).await {
                                match got {
                                    Ok(b) => bytes += b.len() as u64,
                                    Err(err) => {
                                        eprintln!("reader {j}: {err}");
                                        ok = false;
                                    }
                                }
                            }
                            ok
                        }
                        Mode::Push => unreachable!("push runs in main"),
                    };
                    if ok {
                        lat.push(t.elapsed().as_secs_f64() * 1e3);
                    } else {
                        failed += 1;
                    }
                }
                (lat, failed, bytes)
            })
        })
        .collect();
    let mut lat = Vec::new();
    let mut failed = 0;
    let mut bytes = 0;
    for task in tasks {
        let (l, f, b) = task.await.expect("reader");
        lat.extend(l);
        failed += f;
        bytes += b;
    }
    let elapsed = started.elapsed().as_secs_f64();
    let gets = counted.calls(Op::Get) - gets_before;
    lat.sort_by(f64::total_cmp);
    let ops = lat.len() as f64;
    let mode = match args.mode {
        Mode::Push => "push",
        Mode::RefRead => "ref-read",
        Mode::RefReadCached => "ref-read-cached",
        Mode::Fetch => "fetch",
    };
    let line = serde_json::json!({
        "mode": mode,
        "store": args.store,
        "repos": args.repos,
        "readers": args.pushers,
        "duration_s": args.duration,
        "refs": args.refs,
        "pack_bytes": args.pack_bytes,
        "get_delay_ms": args.get_delay_ms,
        "batch": args.batch,
        "requests": lat.len(),
        "failed": failed,
        "requests_per_s": ops / elapsed,
        "ops_per_s": ops * args.batch.max(1) as f64 / elapsed,
        "p50_ms": quantile(&lat, 0.5),
        "p99_ms": quantile(&lat, 0.99),
        "p999_ms": quantile(&lat, 0.999),
        "bytes_per_s": bytes as f64 / elapsed,
        "gets_per_request": if ops > 0.0 { gets as f64 / ops } else { 0.0 },
    });
    println!("{line}");
}
