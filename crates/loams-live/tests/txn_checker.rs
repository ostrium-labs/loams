//! LV1 plan Task 2: the transaction checker (design §20 §5.2, §14). A
//! list-append workload runs over Live documents: each key is a document of
//! the table `lists` whose field `list` is an array, a transaction reads
//! whole lists and appends unique values, and every committed transaction
//! (with what it read) goes into an Elle history (`loams_sim::elle`). The
//! checker then infers the dependency graph and looks for cycles.
//!
//! Mutations are snapshot isolated (§20 §5.2): no G0, G1 or G-single, on
//! either backend. Point reads (`db.get`) are promoted into the lock set, so
//! a workload that reads by id is serializable too (no G2). Index-range
//! reads are plain snapshot reads, so range write skew (G2) can occur
//! unless `RunnerOptions::serializable_ranges` is set (Q31); the tests pin
//! both sides so the documentation stays honest.
//!
//! Every test is a `live_test!`: `::embedded` always, `::tikv` with
//! `LOAMS_TEST_PD`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use futures::future::BoxFuture;
use loams_kv::TxnError;
use loams_live::system::{self, GET, INSERT};
use loams_live::testing::TestStore;
use loams_live::testing::workload::{SizeDefaults, Sizes};
use loams_live::{
    DocId, FnKind, Function, IndexId, IndexRange, LiveConfig, LiveError, LiveTxn, LiveValue,
    Runner, RunnerOptions, live_test,
};
use loams_sim::elle::{self, Analysis, AnomalyKind, Elem, History, Key, Mop, Op};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use tokio::sync::Barrier;

/// The table holding one document per key.
const TABLE: &str = "lists";
/// Keys (documents) of the random workloads.
const KEYS: u64 = 10;
/// Concurrent clients of the random workloads.
const CLIENTS: u64 = 8;
/// Tries per transaction: a retryable storage error is tried again with
/// the same idempotency key, as a client would (row T1-3).
const TRIES: u32 = 5;
/// Journal shards of the random workloads (the reactive checker's).
const SHARDS: u16 = 16;
/// Journal shards of the write-skew rounds: with 1 024, two mutations
/// rarely append to the same journal head, which would make them conflict
/// whatever they read.
const SKEW_SHARDS: u16 = 1_024;

// ---- values ----

fn s(v: &str) -> LiveValue {
    LiveValue::Str(v.to_string())
}

fn obj(pairs: &[(&str, LiveValue)]) -> LiveValue {
    LiveValue::Object(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    )
}

fn sys(name: &str) -> Arc<dyn Function> {
    system::lookup(name).expect("a system function")
}

fn invalid(message: impl Into<String>) -> LiveError {
    LiveError::InvalidArgument(message.into())
}

fn elems_value(list: &[Elem]) -> LiveValue {
    LiveValue::Array(
        list.iter()
            .map(|e| LiveValue::I64(i64::try_from(*e).expect("a small value")))
            .collect(),
    )
}

fn elems(v: Option<&LiveValue>) -> Result<Vec<Elem>, LiveError> {
    let Some(LiveValue::Array(items)) = v else {
        return Err(invalid(format!("a list, not {v:?}")));
    };
    items
        .iter()
        .map(|i| match i {
            LiveValue::I64(n) => u64::try_from(*n).map_err(|_| invalid("a negative value")),
            other => Err(invalid(format!("a list value, not {other:?}"))),
        })
        .collect()
}

fn field<'a>(v: &'a LiveValue, name: &str) -> Option<&'a LiveValue> {
    match v {
        LiveValue::Object(fields) => fields.get(name),
        _ => None,
    }
}

// ---- the transaction function ----

/// How a transaction reads a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reads {
    /// `db.get` by id: promoted into the lock set.
    Point,
    /// A scan of the table's `by_creation_time` index: a snapshot read.
    Range,
}

/// Waits at a barrier before the first append of the first attempt, so two
/// transactions both read before either writes; reruns do not wait.
struct Pause {
    barrier: Arc<Barrier>,
    waited: AtomicBool,
}

impl Pause {
    async fn wait(&self) {
        if !self.waited.swap(true, Ordering::SeqCst) {
            self.barrier.wait().await;
        }
    }
}

/// `{ mops: [{ f: "r", id } | { f: "a", id, v } | { f: "w", id, list }] }`:
/// reads a list, appends `v` to it (reading it first), or (`w`, the broken
/// client of the control run) writes `list` blind. Returns the lists read,
/// in order.
struct ListTxn {
    reads: Reads,
    pause: Option<Pause>,
}

impl ListTxn {
    fn plain(reads: Reads) -> Arc<dyn Function> {
        Arc::new(ListTxn { reads, pause: None })
    }

    fn paused(reads: Reads, barrier: &Arc<Barrier>) -> Arc<dyn Function> {
        Arc::new(ListTxn {
            reads,
            pause: Some(Pause {
                barrier: barrier.clone(),
                waited: AtomicBool::new(false),
            }),
        })
    }

    async fn read(&self, txn: &mut LiveTxn<'_>, id: DocId) -> Result<Vec<Elem>, LiveError> {
        let doc = match self.reads {
            Reads::Point => txn.get(id).await?,
            Reads::Range => {
                let table = txn
                    .table(TABLE)
                    .await?
                    .ok_or_else(|| invalid("no lists table"))?;
                txn.query(IndexRange::all(table.id, IndexId::BY_CREATION_TIME))
                    .await?
                    .into_iter()
                    .find(|d| d.id == id)
            }
        };
        let doc = doc.ok_or_else(|| invalid(format!("no document {id}")))?;
        elems(doc.fields.get("list"))
    }
}

impl Function for ListTxn {
    fn name(&self) -> &str {
        match self.reads {
            Reads::Point => "test:list_txn_point",
            Reads::Range => "test:list_txn_range",
        }
    }

    fn kind(&self) -> FnKind {
        FnKind::Mutation
    }

    fn call<'a>(
        &'a self,
        txn: &'a mut LiveTxn<'_>,
        args: LiveValue,
    ) -> BoxFuture<'a, Result<LiveValue, LiveError>> {
        Box::pin(async move {
            let Some(LiveValue::Array(mops)) = field(&args, "mops") else {
                return Err(invalid("no mops"));
            };
            let mut out = Vec::new();
            let mut wrote = false;
            for m in mops {
                let id: DocId = match field(m, "id") {
                    Some(LiveValue::Str(id)) => id.parse()?,
                    other => return Err(invalid(format!("an id, not {other:?}"))),
                };
                let f = field(m, "f");
                if f == Some(&s("r")) {
                    out.push(elems_value(&self.read(txn, id).await?));
                    continue;
                }
                if !wrote {
                    wrote = true;
                    if let Some(pause) = &self.pause {
                        pause.wait().await;
                    }
                }
                let list = if f == Some(&s("a")) {
                    let doc = txn
                        .get(id)
                        .await?
                        .ok_or_else(|| invalid(format!("no document {id}")))?;
                    let mut list = elems(doc.fields.get("list"))?;
                    match field(m, "v") {
                        Some(LiveValue::I64(v)) => {
                            list.push(u64::try_from(*v).map_err(|_| invalid("a negative value"))?)
                        }
                        other => return Err(invalid(format!("a value, not {other:?}"))),
                    }
                    elems_value(&list)
                } else if f == Some(&s("w")) {
                    field(m, "list").cloned().unwrap_or(LiveValue::Null)
                } else {
                    return Err(invalid(format!("an op, not {f:?}")));
                };
                txn.patch(id, BTreeMap::from([("list".to_string(), list)]))
                    .await?;
            }
            Ok(LiveValue::Array(out))
        })
    }
}

// ---- the client ----

/// An app with one list per key, empty.
struct Lists {
    runner: Runner,
    ids: Vec<DocId>,
}

impl Lists {
    async fn open(store: &TestStore, keys: u64, options: RunnerOptions, shards: u16) -> Self {
        let root = store.fresh_root().await;
        let config = LiveConfig {
            journal_shards: shards,
            ..root.live_config("txn-checker")
        };
        let runner = Runner::open_with(root.store(), &config, options)
            .await
            .expect("the runner opens");
        let mut ids = Vec::new();
        for k in 0..keys {
            let fields = obj(&[
                ("k", LiveValue::I64(i64::try_from(k).expect("a small key"))),
                ("list", LiveValue::Array(Vec::new())),
            ]);
            let m = runner
                .mutate(
                    sys(INSERT),
                    obj(&[("table", s(TABLE)), ("fields", fields)]),
                    None,
                )
                .await
                .expect("a list is created");
            let LiveValue::Str(id) = m.result else {
                panic!("insert returns an id, not {:?}", m.result)
            };
            ids.push(id.parse().expect("a document id"));
        }
        Lists { runner, ids }
    }

    fn id(&self, key: Key) -> String {
        self.ids[usize::try_from(key).expect("a small key")].to_string()
    }

    /// The function's arguments for `mops`.
    fn args(&self, mops: &[Mop]) -> LiveValue {
        let mops = mops
            .iter()
            .map(|m| match m {
                Mop::Read { key, .. } => obj(&[("f", s("r")), ("id", s(&self.id(*key)))]),
                Mop::Append { key, value } => obj(&[
                    ("f", s("a")),
                    ("id", s(&self.id(*key))),
                    (
                        "v",
                        LiveValue::I64(i64::try_from(*value).expect("a small value")),
                    ),
                ]),
            })
            .collect();
        obj(&[("mops", LiveValue::Array(mops))])
    }

    /// Runs `mops` as one transaction of `process` with `f`.
    async fn transact(
        &self,
        f: &Arc<dyn Function>,
        process: u64,
        mops: Vec<Mop>,
        key: String,
    ) -> Result<(Op, u32), String> {
        let args = self.args(&mops);
        self.transact_with(f, process, mops, args, key).await
    }

    /// Runs `args` with `f` as the transaction `mops` (reads with unknown
    /// lists) of `process`, retrying a retryable storage error with the
    /// same idempotency key `key`. Returns the history's op, with the lists
    /// read when it committed (`Info`, unknown, when every try failed with
    /// a retryable error), and the attempts of its committing try.
    async fn transact_with(
        &self,
        f: &Arc<dyn Function>,
        process: u64,
        mut mops: Vec<Mop>,
        args: LiveValue,
        key: String,
    ) -> Result<(Op, u32), String> {
        let mut tries = 0;
        let m = loop {
            tries += 1;
            match self
                .runner
                .mutate(f.clone(), args.clone(), Some(key.clone()))
                .await
            {
                Ok(m) => break m,
                Err(LiveError::Txn(
                    TxnError::Deadline
                    | TxnError::Conflict
                    | TxnError::NotApplied(_)
                    | TxnError::Undetermined { .. },
                )) if tries < TRIES => {}
                Err(LiveError::Txn(
                    TxnError::Deadline
                    | TxnError::Conflict
                    | TxnError::NotApplied(_)
                    | TxnError::Undetermined { .. },
                )) => return Ok((Op::info(process, mops), 0)),
                Err(e) => return Err(format!("{key}: {e} (try {tries})")),
            }
        };
        let LiveValue::Array(lists) = &m.result else {
            return Err(format!("{key}: a result of {:?}", m.result));
        };
        let mut lists = lists.iter();
        for mop in &mut mops {
            if let Mop::Read { list, .. } = mop {
                let read = lists
                    .next()
                    .ok_or_else(|| format!("{key}: fewer lists than reads"))?;
                *list = Some(elems(Some(read)).map_err(|e| format!("{key}: {e}"))?);
            }
        }
        Ok((Op::ok(process, mops), m.attempts))
    }

    /// A transaction reading every key (by id), so the history has each
    /// key's whole order.
    async fn read_all(&self, process: u64, key: String) -> Result<Op, String> {
        let mops = (0..self.ids.len() as u64)
            .map(|k| Mop::Read { key: k, list: None })
            .collect();
        let (op, _) = self
            .transact(&ListTxn::plain(Reads::Point), process, mops, key)
            .await?;
        Ok(op)
    }

    /// `key`'s list now, read outside any transaction.
    async fn list_now(&self, key: Key) -> Result<Vec<Elem>, String> {
        let at = self.runner.store().now().await.map_err(|e| e.to_string())?;
        let doc = self
            .runner
            .query(&*sys(GET), obj(&[("id", s(&self.id(key)))]), at)
            .await
            .map_err(|e| e.to_string())?
            .result;
        elems(field(&doc, "list")).map_err(|e| e.to_string())
    }
}

// ---- the random workload ----

/// How the random workload's transactions read and append.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Every read by id.
    Point,
    /// Every read by range.
    Range,
    /// Each transaction picks one of the two.
    Mixed,
    /// The control run's broken client: an append is a transaction of its
    /// own that writes the list read *before* it (outside it) plus the
    /// value, blind, so concurrent appends get lost.
    LostAppends,
}

/// A random workload's run.
struct Run {
    history: History,
    /// Transactions whose committing try took more than one attempt.
    reran: usize,
}

/// `ops` transactions of 1 to 4 micro-ops (reads and appends of the
/// workload's keys, half each) from `clients` concurrent clients, then one
/// that reads every key. Client `c` draws its transactions from ChaCha8 at
/// `seed × 1 000 + c`, and transaction `op` carries the idempotency key
/// `txn-{seed}-{c}-{op}`; a seed reproduces the mix, not the interleaving.
/// Values are unique: transaction `op`'s `j`-th micro-op appends
/// `op × 8 + j + 1`.
async fn random_workload(
    lists: &Arc<Lists>,
    seed: u64,
    ops: usize,
    clients: u64,
    mode: Mode,
) -> Result<Run, String> {
    let next = Arc::new(AtomicUsize::new(0));
    let keys = lists.ids.len() as u64;
    let mut tasks = Vec::new();
    for c in 0..clients {
        let (lists, next) = (lists.clone(), next.clone());
        tasks.push(tokio::spawn(async move {
            let mut rng = ChaCha8Rng::seed_from_u64(seed.wrapping_mul(1_000).wrapping_add(c));
            let (point, range) = (ListTxn::plain(Reads::Point), ListTxn::plain(Reads::Range));
            let mut out = Vec::new();
            loop {
                let op = next.fetch_add(1, Ordering::SeqCst);
                if op >= ops {
                    return Ok::<_, String>(out);
                }
                let key = format!("txn-{seed}-{c}-{op}");
                if mode == Mode::LostAppends {
                    let k = rng.random_range(0..keys);
                    let value = op as u64 * 8 + 1;
                    if rng.random_bool(0.5) {
                        let mut list = lists.list_now(k).await?;
                        list.push(value);
                        tokio::task::yield_now().await;
                        let args = obj(&[(
                            "mops",
                            LiveValue::Array(vec![obj(&[
                                ("f", s("w")),
                                ("id", s(&lists.id(k))),
                                ("list", elems_value(&list)),
                            ])]),
                        )]);
                        let mops = vec![Mop::append(k, value)];
                        out.push(lists.transact_with(&point, c, mops, args, key).await?);
                    } else {
                        let mops = vec![Mop::Read { key: k, list: None }];
                        out.push(lists.transact(&point, c, mops, key).await?);
                    }
                    continue;
                }
                let len = rng.random_range(1..=4);
                let mops = (0..len)
                    .map(|j| {
                        let k = rng.random_range(0..keys);
                        if rng.random_bool(0.5) {
                            Mop::Read { key: k, list: None }
                        } else {
                            Mop::append(k, op as u64 * 8 + j + 1)
                        }
                    })
                    .collect();
                let f = match mode {
                    Mode::Point => &point,
                    Mode::Range => &range,
                    _ if rng.random_bool(0.5) => &point,
                    _ => &range,
                };
                out.push(lists.transact(f, c, mops, key).await?);
            }
        }));
    }
    let mut history = History::new();
    let mut reran = 0;
    for t in tasks {
        for (op, attempts) in t.await.map_err(|e| e.to_string())?? {
            reran += usize::from(attempts > 1);
            history.push(op);
        }
    }
    history.push(lists.read_all(clients, format!("txn-{seed}-final")).await?);
    Ok(Run { history, reran })
}

// ---- the write-skew rounds ----

/// `rounds` rounds of the classic write skew on keys `2r` and `2r + 1`: one
/// transaction reads `x` and appends to `y`, the other reads `y` and
/// appends to `x`, both reading before either writes (a barrier). Returns
/// the history (with a final read of every key) and each round's attempts.
async fn skew_rounds(
    lists: &Lists,
    reads: Reads,
    rounds: u64,
) -> Result<(History, Vec<u32>), String> {
    let mut history = History::new();
    let mut attempts = Vec::new();
    for r in 0..rounds {
        let (x, y) = (2 * r, 2 * r + 1);
        let barrier = Arc::new(Barrier::new(2));
        let (fa, fb) = (
            ListTxn::paused(reads, &barrier),
            ListTxn::paused(reads, &barrier),
        );
        let a = lists.transact(
            &fa,
            0,
            vec![Mop::Read { key: x, list: None }, Mop::append(y, 2 * r + 1)],
            format!("skew-{r}-a"),
        );
        let b = lists.transact(
            &fb,
            1,
            vec![Mop::Read { key: y, list: None }, Mop::append(x, 2 * r + 2)],
            format!("skew-{r}-b"),
        );
        let ((a, na), (b, nb)) = tokio::try_join!(a, b)?;
        history.push(a);
        history.push(b);
        attempts.push(na + nb);
    }
    history.push(lists.read_all(2, "skew-final".into()).await?);
    Ok((history, attempts))
}

// ---- reporting ----

/// Writes `history` to a file under the target's temporary directory and
/// returns a line naming it. Called only on a failure.
fn dump(name: &str, history: &History) -> String {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("txn-checker");
    let path = dir.join(format!("{name}-{}.log", std::process::id()));
    let text = history
        .ops
        .iter()
        .enumerate()
        .map(|(i, op)| format!("T{i} {op:?}\n"))
        .collect::<String>();
    std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(&path, text))
        .map_or_else(
            |e| format!("(no dump: {e})"),
            |()| format!("history: {}", path.display()),
        )
}

fn sizes(store: &TestStore) -> Sizes {
    Sizes::from_env_or(store.backend(), SizeDefaults::TXN, &|name| {
        std::env::var(name).ok()
    })
    .expect("the checker's sizes")
}

/// The graph has every kind of edge: the run had concurrency to check.
fn assert_dense(analysis: &Analysis, context: &str) {
    let e = analysis.edges;
    assert!(
        e.ww > 0 && e.wr > 0 && e.rw > 0,
        "{context}: a sparse graph ({e:?}, {} committed)",
        analysis.committed
    );
}

// ---- the tests ----

/// Seeded random list-append workloads, reads by id and by range mixed:
/// no anomaly that snapshot isolation forbids (G2 is allowed). 5 seeds on
/// the embedded store and 2 on TiKV by default, 1 000 transactions each
/// (`LOAMS_CHECKER_SEEDS`, `LOAMS_CHECKER_OPS`,
/// `LOAMS_CHECKER_SEED_OFFSET`, shared with the reactive checker).
async fn live_histories_are_snapshot_isolated(store: TestStore) {
    let sizes = sizes(&store);
    eprintln!("txn checker sizes on {:?}: {sizes:?}", store.backend());
    for seed in sizes.offset..sizes.offset + sizes.seeds {
        let rerun = format!(
            "seed {seed} on {:?} (rerun it alone with LOAMS_CHECKER_SEED_OFFSET={seed} \
             LOAMS_CHECKER_SEEDS=1 LOAMS_CHECKER_OPS={})",
            store.backend(),
            sizes.ops
        );
        let lists = Arc::new(Lists::open(&store, KEYS, RunnerOptions::default(), SHARDS).await);
        let run = random_workload(&lists, seed, sizes.ops, CLIENTS, Mode::Mixed)
            .await
            .unwrap_or_else(|e| panic!("{rerun}: the workload failed: {e}"));
        let analysis = elle::analyze(&run.history);
        eprintln!(
            "seed {seed}: {} committed, {} reran, {:?}, G2: {}",
            analysis.committed,
            run.reran,
            analysis.edges,
            analysis.g2().is_some()
        );
        if let Some(a) = analysis.snapshot_isolation() {
            panic!(
                "{rerun}: not snapshot isolated: {a}\n{}",
                dump(&format!("si-seed-{seed}"), &run.history)
            );
        }
        assert_dense(&analysis, &rerun);
        assert!(run.reran > 0, "{rerun}: no transaction reran on a conflict");
    }
}
live_test!(live_histories_are_snapshot_isolated);

/// The checker is not vacuous on Live: a client whose appends write a list
/// read outside the transaction loses appends, and the history is flagged.
/// The same seed with real appends (the control run) passes.
async fn txn_checker_flags_lost_appends(store: TestStore) {
    let (seed, ops, keys) = (3, 300, 3);
    let lists = Arc::new(Lists::open(&store, keys, RunnerOptions::default(), SHARDS).await);
    let control = random_workload(&lists, seed, ops, CLIENTS, Mode::Point)
        .await
        .expect("the control run");
    if let Err(a) = elle::check_list_append(&control.history) {
        panic!(
            "the control run: {a}\n{}",
            dump("control", &control.history)
        );
    }
    let lists = Arc::new(Lists::open(&store, keys, RunnerOptions::default(), SHARDS).await);
    let broken = random_workload(&lists, seed, ops, CLIENTS, Mode::LostAppends)
        .await
        .expect("the broken run");
    let a = elle::check_list_append(&broken.history).expect_err("lost appends are flagged");
    assert!(
        matches!(
            a.kind,
            AnomalyKind::IncompatibleOrder | AnomalyKind::GSingle | AnomalyKind::G1c
        ),
        "{a}"
    );
}
live_test!(txn_checker_flags_lost_appends);

/// Semantics 2 (§20 §5.2) through the checker: point reads are promoted
/// into the lock set, so the classic write skew cannot happen (one side
/// reruns), and a random workload that reads by id is serializable: no G2.
async fn point_read_promotion_prevents_write_skew(store: TestStore) {
    let lists = Lists::open(&store, 6, RunnerOptions::default(), SKEW_SHARDS).await;
    let (history, attempts) = skew_rounds(&lists, Reads::Point, 3)
        .await
        .expect("the rounds");
    if let Err(a) = elle::check_serializable(&history) {
        panic!(
            "write skew on point reads: {a}\n{}",
            dump("point-skew", &history)
        );
    }
    assert!(
        attempts.iter().all(|n| *n >= 3),
        "one side of every round reran: {attempts:?}"
    );

    let sizes = sizes(&store);
    let lists = Arc::new(Lists::open(&store, KEYS, RunnerOptions::default(), SHARDS).await);
    let run = random_workload(&lists, sizes.offset, sizes.ops / 2, CLIENTS, Mode::Point)
        .await
        .expect("the workload");
    let analysis = elle::analyze(&run.history);
    if let Some(a) = analysis.snapshot_isolation().or_else(|| analysis.g2()) {
        panic!(
            "seed {}: point reads are not serializable: {a}\n{}",
            sizes.offset,
            dump("point-random", &run.history)
        );
    }
    assert_dense(&analysis, "point reads");
}
live_test!(point_read_promotion_prevents_write_skew);

/// Q31, documented: index-range reads are snapshot reads, so without
/// `serializable_ranges` the classic write skew **can** occur (a G2 the
/// checker finds, in a history that is still snapshot isolated). With
/// `serializable_ranges` it never does: every round has one side rerun,
/// and a random workload that reads by range is serializable.
async fn range_write_skew_is_possible_without_serializable_ranges(store: TestStore) {
    let rounds = 3;
    let lists = Lists::open(&store, 2 * rounds, RunnerOptions::default(), SKEW_SHARDS).await;
    let (history, _) = skew_rounds(&lists, Reads::Range, rounds)
        .await
        .expect("the rounds");
    let analysis = elle::analyze(&history);
    if let Some(a) = analysis.snapshot_isolation() {
        panic!("range reads: {a}\n{}", dump("range-off", &history));
    }
    let g2 = analysis
        .g2()
        .unwrap_or_else(|| panic!("no write skew in {rounds} rounds without serializable_ranges"));
    assert!(
        g2.cycle.iter().filter(|s| s.dep == elle::Dep::Rw).count() >= 2,
        "{g2}"
    );

    let on = RunnerOptions {
        serializable_ranges: true,
        ..RunnerOptions::default()
    };
    let lists = Lists::open(&store, 2 * rounds, on.clone(), SKEW_SHARDS).await;
    let (history, attempts) = skew_rounds(&lists, Reads::Range, rounds)
        .await
        .expect("the rounds");
    if let Err(a) = elle::check_serializable(&history) {
        panic!("serializable_ranges: {a}\n{}", dump("range-on", &history));
    }
    assert!(
        attempts.iter().all(|n| *n >= 3),
        "one side of every round reran: {attempts:?}"
    );

    let lists = Arc::new(Lists::open(&store, KEYS, on, SHARDS).await);
    let run = random_workload(&lists, sizes(&store).offset, 200, 4, Mode::Range)
        .await
        .expect("the workload");
    if let Err(a) = elle::check_serializable(&run.history) {
        panic!(
            "serializable_ranges, random: {a}\n{}",
            dump("range-on-random", &run.history)
        );
    }
}
live_test!(range_write_skew_is_possible_without_serializable_ranges);
