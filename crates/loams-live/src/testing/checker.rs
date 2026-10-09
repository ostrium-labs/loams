//! The reactive correctness checker (LV1 plan Task 1; design §20 §14 item
//! 3).
//!
//! [`run_reactive_checker`] runs a seeded [`Workload`] on a store: tables
//! with an index, sessions watching bounded queries over them, and writers
//! inserting, patching and deleting documents, with the workload's
//! disturbances on the way. Every Transition a session receives is applied
//! to a [`ClientState`], and the client's whole state is recorded at the
//! Transition's end. After the run the checker verifies:
//!
//! - **every recorded state equals a fresh snapshot evaluation** of each of
//!   the session's queries at the Transition's `end.ts`. Checking the whole
//!   state, not only the updated queries, makes a missed invalidation that
//!   is never repaired show as a stale state;
//! - **a change reaches the session at the first tick at or after it**. The
//!   checker records every tick of the subscription manager. For
//!   consecutive Transitions of a session at `p` and `e`, a tick `T` with
//!   `p < T < e` must not have a fresh result that differs from the state
//!   at `p`: the change at `T` was then held back to a later tick. An
//!   Outbox merge (or a resume) legitimately skips ticks, but delivers
//!   promptly, so a tick is checked only when `e` arrived more than
//!   [`LATE_BOUND`] after it;
//! - **versions strictly increase** per session (a heartbeat repeats its
//!   version by design, and so may the first Transition of a resumed
//!   session, which re-sends the full results at the client's last
//!   version), and every Transition starts at the client's version;
//! - **every session catches up** with the last commit;
//! - **resumed sessions converge**: a session disconnected mid-run resumes
//!   from its last version and its later states pass the same checks.
//!
//! **What a seed reproduces.** The seed fixes the op mix: each session's
//! queries and each writer's sequence of ops (kind, table, value), drawn
//! from a ChaCha8 stream per writer. It does not fix the interleaving: which
//! op number a writer takes, when ticks fall and when sessions read depend
//! on scheduling. A failing run's [`Report::dump`] holds what is needed to
//! study it: every writer's op log and every session's records.
//!
//! The subscription manager runs with its safety rerun off, so a missed
//! invalidation stays visible instead of being repaired. The report counts,
//! per session, the update-carrying Transitions that arrived while the
//! writers ran, so a run whose sessions only caught up at the end is seen.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use buffa::MessageField;
use loams_kv::{CommitMode, EmbeddedConfig, Store, StoreConfig, Ts, TxnError, TxnOptions};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::workload::{Disturbance, Workload};
use crate::catalog::{self, IndexSpec};
use crate::session::{ClientState, QueryResult, Session, SessionConfig, Sessions, Start, Version};
use crate::subs::{SubsConfig, SubsStats, Subscriptions};
use crate::system::{DELETE, GET, INSERT, PATCH, QUERY};
use crate::{AppKeys, Function, LiveConfig, LiveError, LiveValue, Runner, deploy, pb, system};

/// What the checker saw.
#[derive(Debug, Clone, Default)]
pub struct Report {
    /// The workload's seed.
    pub seed: u64,
    /// Complete Transitions applied, over every session.
    pub transitions: usize,
    /// Query results compared with a fresh evaluation.
    pub checked: usize,
    /// Sessions disconnected and resumed.
    pub resumes: usize,
    /// Ticks of the subscription manager recorded.
    pub ticks: usize,
    /// Per session, the Transitions carrying updates that arrived before
    /// the writers finished (the first Transition and a resumed session's
    /// re-sent one excluded).
    pub live_updates: Vec<usize>,
    /// The subscription manager's counters at the end of the run.
    pub subs: SubsStats,
    /// Every committed op of the writers, by op number.
    pub ops: Vec<OpRecord>,
    /// Every session's queries and records.
    pub sessions: Vec<SessionTrace>,
    pub violations: Vec<Violation>,
}

impl Report {
    /// The run in text: the violations, every writer's op log and every
    /// session's records (for a failing run; it can be large).
    pub fn dump(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let _ = writeln!(
            out,
            "reactive checker, seed {}: {} transitions, {} checks, {} ticks, {} resumes; {:?}",
            self.seed, self.transitions, self.checked, self.ticks, self.resumes, self.subs
        );
        let _ = writeln!(out, "violations ({}):", self.violations.len());
        for v in &self.violations {
            let _ = writeln!(out, "  {v:?}");
        }
        let writers = self.ops.iter().map(|o| o.writer + 1).max().unwrap_or(0);
        for writer in 0..writers {
            let _ = writeln!(out, "writer {writer}:");
            for o in self.ops.iter().filter(|o| o.writer == writer) {
                let _ = writeln!(
                    out,
                    "  op {}: {:?} t{} n={} target={:?}{} -> {:?} at ts {}{}",
                    o.op,
                    o.kind,
                    o.table,
                    o.n,
                    o.target,
                    if o.seeded { " (seeded)" } else { "" },
                    o.result,
                    o.commit_ts,
                    if o.replayed { " (replayed)" } else { "" },
                );
            }
        }
        for session in &self.sessions {
            let _ = writeln!(out, "session {}:", session.index);
            for q in &session.queries {
                let _ = writeln!(out, "  query {q}");
            }
            for r in &session.records {
                let _ = writeln!(
                    out,
                    "  {:?}{}{}: {:?}",
                    r.end,
                    if r.updates { " updates" } else { "" },
                    if r.resent { " resent" } else { "" },
                    r.results
                );
            }
        }
        out
    }
}

/// What a writer's op did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Insert,
    Patch,
    Delete,
}

/// One committed op of a writer.
#[derive(Debug, Clone, PartialEq)]
pub struct OpRecord {
    pub writer: usize,
    /// The op number (from the writers' shared counter).
    pub op: usize,
    pub kind: OpKind,
    pub table: usize,
    /// The value of `n` written (unused by a delete).
    pub n: i64,
    /// The patched or deleted document.
    pub target: Option<LiveValue>,
    /// The target is a seeded document.
    pub seeded: bool,
    /// What the mutation returned (an insert's id).
    pub result: LiveValue,
    pub commit_ts: u64,
    /// The result came from the op's idempotency record.
    pub replayed: bool,
}

/// One session's queries and the client's state after each Transition.
#[derive(Debug, Clone)]
pub struct SessionTrace {
    pub index: usize,
    pub queries: Vec<String>,
    pub records: Vec<Record>,
}

/// Which guarantee broke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViolationKind {
    /// A held result differs from a fresh evaluation at the Transition's
    /// `end.ts`.
    Stale,
    /// A tick changed a session's result, and the session's next
    /// Transition came at a later tick, more than [`LATE_BOUND`] after it:
    /// the change was not reflected by the first tick at or after it.
    Late,
    /// A Transition's end does not follow its start.
    VersionOrder,
    /// A Transition did not start at the client's version.
    Gap,
    /// A session did not reach the last commit.
    NotCaughtUp,
    /// A session ended with an error.
    SessionClosed,
    /// A disturbance that a later task wires.
    Unwired,
    /// The workload itself failed (a mutation, a table definition).
    Workload,
}

/// A broken guarantee.
#[derive(Debug, Clone, PartialEq)]
pub struct Violation {
    pub kind: ViolationKind,
    /// The session (its index in the workload), when one is concerned.
    pub session: Option<usize>,
    pub detail: String,
}

impl Violation {
    fn new(kind: ViolationKind, session: Option<usize>, detail: impl Into<String>) -> Self {
        Violation {
            kind,
            session,
            detail: detail.into(),
        }
    }
}

/// Values of `n`, the indexed field.
const N_VALUES: i64 = 20;
/// Documents per table before the sessions open.
pub const SEED_DOCS: usize = 8;
/// Queries per session.
const QUERIES: u32 = 4;
/// Concurrent writers.
const WRITERS: usize = 8;
/// Tries per mutation (a retryable storage error is tried again).
const MUTATION_TRIES: u32 = 5;
/// How long after a tick the Transition reflecting it may arrive. A
/// session's Transition for a tick is queued as the tick is published; an
/// Outbox merge or a resume delivers a later tick instead, still promptly.
pub const LATE_BOUND: Duration = Duration::from_secs(1);
/// How long sessions may take to reach the last commit.
const CATCH_UP: Duration = Duration::from_secs(30);

/// One of a session's queries.
#[derive(Debug, Clone)]
struct Query {
    id: u32,
    function: &'static str,
    args: LiveValue,
}

/// A client's state at the end of one Transition.
#[derive(Debug, Clone)]
pub struct Record {
    pub end: Version,
    pub results: BTreeMap<u32, QueryResult>,
    /// When the consumer applied it.
    pub arrived: Instant,
    /// It carried updates.
    pub updates: bool,
    /// A resumed session's first Transition, re-sending its results.
    pub resent: bool,
}

/// When a session's Transition arrived, as the first-tick check needs it.
#[derive(Debug, Clone, Copy)]
struct Arrival {
    ts: u64,
    arrived: Instant,
    resent: bool,
}

/// The ticks to check between a session's Transitions: for consecutive
/// Transitions `p` and `e` (by `p`'s index), each tick `T` with
/// `p.ts < T < e.ts` (once per timestamp, at its first arrival) that `e`
/// arrived more than `bound` after. The bound counts from the tick, or from
/// `p` when `p` is a resumed session's re-sent Transition (the session
/// could deliver nothing before it). At each, the session's results must
/// still be those at `p`.
fn late_ticks(records: &[Arrival], ticks: &[(u64, Instant)], bound: Duration) -> Vec<(usize, u64)> {
    let mut out = Vec::new();
    for (i, pair) in records.windows(2).enumerate() {
        let (p, e) = (pair[0], pair[1]);
        let from = ticks.partition_point(|t| t.0 <= p.ts);
        let mut last = None;
        for &(ts, arrived) in &ticks[from..] {
            if ts >= e.ts {
                break;
            }
            if last == Some(ts) {
                continue;
            }
            last = Some(ts);
            let since = if p.resent {
                arrived.max(p.arrived)
            } else {
                arrived
            };
            if e.arrived.saturating_duration_since(since) > bound {
                out.push((i, ts));
            }
        }
    }
    out
}

/// Fresh evaluations, shared between sessions watching the same query at
/// the same tick.
type Fresh = HashMap<(&'static str, String, u64), Result<LiveValue, pb::LiveError>>;

/// `q`'s fresh result at `ts`.
async fn fresh_at<'a>(
    fresh: &'a mut Fresh,
    runner: &Runner,
    q: &Query,
    ts: u64,
) -> Result<&'a Result<LiveValue, pb::LiveError>, String> {
    Ok(
        match fresh.entry((q.function, format!("{:?}", q.args), ts)) {
            Entry::Occupied(found) => found.into_mut(),
            Entry::Vacant(slot) => {
                let f = sys(q.function)?;
                let got = runner
                    .query(&*f, q.args.clone(), Ts(ts))
                    .await
                    .map(|r| r.result)
                    .map_err(|e| e.to_proto());
                slot.insert(got)
            }
        },
    )
}

/// Whether a held result is the fresh one: the same value, or an error
/// with the same code (messages may name a timestamp or a key).
fn same(held: Option<&QueryResult>, want: &Result<LiveValue, pb::LiveError>) -> bool {
    match (held, want) {
        (Some(QueryResult::Value(v)), Ok(f)) => v == f,
        (Some(QueryResult::Error(e)), Err(f)) => e.code == f.code,
        _ => false,
    }
}

#[derive(Debug, Default)]
struct Log {
    client: ClientState,
    /// The session was just resumed: its first Transition may re-send the
    /// full results at the client's last version (`start == end`).
    resumed: bool,
    records: Vec<Record>,
    violations: Vec<Violation>,
}

/// A session of the workload and the task consuming its Transitions.
struct Watcher {
    index: usize,
    queries: Vec<Query>,
    set: pb::QuerySet,
    log: Arc<Mutex<Log>>,
    session: Session,
    stop: CancellationToken,
    consumer: JoinHandle<()>,
}

fn lock(log: &Mutex<Log>) -> std::sync::MutexGuard<'_, Log> {
    log.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn obj(pairs: &[(&str, LiveValue)]) -> LiveValue {
    LiveValue::Object(
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect(),
    )
}

fn s(v: &str) -> LiveValue {
    LiveValue::Str(v.to_string())
}

fn n(v: i64) -> LiveValue {
    LiveValue::I64(v)
}

fn sys(name: &str) -> Result<Arc<dyn Function>, String> {
    system::lookup(name).ok_or_else(|| format!("no system function {name}"))
}

/// Whether `end` follows `start`: no part goes back, and something moves.
fn follows(start: Version, end: Version) -> bool {
    end.query_set >= start.query_set
        && end.identity >= start.identity
        && end.ts >= start.ts
        && end != start
}

/// Runs `w` on `store` and checks every session's Transitions. `store`
/// should be fresh (its own root): the checker defines its tables there.
pub async fn run_reactive_checker(store: Store, w: Workload) -> Report {
    let mut report = Report::default();
    if let Err(e) = run(store, &w, &mut report).await {
        report
            .violations
            .push(Violation::new(ViolationKind::Workload, None, e));
    }
    report
}

/// The task (of the LV1 plan) that wires `d`, when it is not wired yet.
fn unwired(d: Disturbance) -> Option<&'static str> {
    match d {
        Disturbance::Deploy { .. } | Disturbance::Rollback { .. } => Some("Task 7"),
        Disturbance::AddIndex { .. } | Disturbance::DropIndex { .. } => Some("Task 9"),
        Disturbance::IdentityChange { .. } => Some("Task 17"),
        Disturbance::NodeKill { .. } => Some("Task 24"),
        Disturbance::Disconnect { .. } | Disturbance::DropInvalidation { .. } => None,
    }
}

fn at_op(d: Disturbance) -> usize {
    match d {
        Disturbance::Deploy { at_op }
        | Disturbance::Rollback { at_op }
        | Disturbance::AddIndex { at_op }
        | Disturbance::DropIndex { at_op }
        | Disturbance::IdentityChange { at_op }
        | Disturbance::NodeKill { at_op }
        | Disturbance::Disconnect { at_op }
        | Disturbance::DropInvalidation { at_op } => at_op,
    }
}

fn table_name(t: usize) -> String {
    format!("t{t}")
}

#[allow(clippy::too_many_lines)]
async fn run(store: Store, w: &Workload, report: &mut Report) -> Result<(), String> {
    report.seed = w.seed;
    let mut disturb: Vec<Disturbance> = Vec::new();
    for &d in &w.disturb {
        match unwired(d) {
            Some(task) => report.violations.push(Violation::new(
                ViolationKind::Unwired,
                None,
                format!("{d:?} is wired by {task} of the LV1 plan; skipped"),
            )),
            None => disturb.push(d),
        }
    }
    disturb.sort_by_key(|d| at_op(*d));

    // The runner takes the store as given; the config gives it its limits
    // and journal shards (its `store` names nothing the runner opens).
    let config = LiveConfig {
        journal_shards: 16,
        ..LiveConfig::with_store(
            "checker",
            StoreConfig::Embedded(EmbeddedConfig::new("unused", "checker")),
        )
    };
    let runner = Runner::open(store.clone(), &config)
        .await
        .map_err(|e| format!("opening the runner: {e}"))?;
    let tables = w.tables.max(1);
    for t in 0..tables {
        define(&store, &table_name(t)).await?;
    }
    let mut rng = ChaCha8Rng::seed_from_u64(w.seed);
    let insert = sys(INSERT)?;
    let mut seeded: Vec<Vec<LiveValue>> = vec![Vec::new(); tables];
    for (t, ids) in seeded.iter_mut().enumerate() {
        for i in 0..SEED_DOCS {
            let args = obj(&[
                ("table", s(&table_name(t))),
                ("fields", obj(&[("n", n(rng.random_range(0..N_VALUES)))])),
            ]);
            let m = mutate(
                &runner,
                &insert,
                &args,
                format!("chk-{}-seed-{t}-{i}", w.seed),
            )
            .await
            .map_err(|e| format!("seeding: {e}"))?;
            ids.push(m.result);
        }
    }

    let shutdown = CancellationToken::new();
    let subs = Arc::new(Subscriptions::spawn(
        runner.clone(),
        SubsConfig {
            // Off: a missed invalidation must stay visible.
            safety_rerun: Duration::from_secs(24 * 3600),
            ..SubsConfig::default()
        },
        shutdown.clone(),
    ));
    let sessions = Sessions::new(
        subs.clone(),
        Arc::new(deploy::resolve),
        SessionConfig::default(),
        "checker".into(),
        shutdown.clone(),
    );

    // Every tick, as it is published (before any session opens).
    let ticks = Arc::new(Mutex::new(Vec::<(u64, Instant)>::new()));
    let ticks_lagged = Arc::new(AtomicU64::new(0));
    let recorder = {
        let (ticks, lagged, stop) = (ticks.clone(), ticks_lagged.clone(), shutdown.clone());
        let mut updates = subs.updates();
        tokio::spawn(async move {
            loop {
                let tick = tokio::select! {
                    () = stop.cancelled() => return,
                    tick = updates.recv() => tick,
                };
                match tick {
                    Ok(tick) => ticks
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push((tick.at.0, Instant::now())),
                    Err(RecvError::Lagged(n)) => {
                        lagged.fetch_add(n, Ordering::SeqCst);
                    }
                    Err(RecvError::Closed) => return,
                }
            }
        })
    };

    let mut watchers = Vec::new();
    for index in 0..w.sessions {
        let queries = queries(&mut rng, tables, &seeded);
        let set = pb::QuerySet {
            version: 1,
            queries: queries
                .iter()
                .map(|q| pb::QuerySpec {
                    query_id: q.id,
                    function: q.function.into(),
                    args: MessageField::some(q.args.to_proto()),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        let log = Arc::new(Mutex::new(Log::default()));
        let session = sessions
            .open(Start::Initial(set.clone()))
            .map_err(|e| format!("opening session {index}: {e}"))?;
        let stop = CancellationToken::new();
        let consumer = tokio::spawn(consume(index, session.clone(), log.clone(), stop.clone()));
        watchers.push(Watcher {
            index,
            queries,
            set,
            log,
            session,
            stop,
            consumer,
        });
    }

    // The writers: each patches and deletes only documents it inserted.
    let next = Arc::new(AtomicUsize::new(0));
    let newest = Arc::new(AtomicU64::new(0));
    let failures = Arc::new(Mutex::new(Vec::<String>::new()));
    let op_log = Arc::new(Mutex::new(Vec::<OpRecord>::new()));
    let mut writers = Vec::new();
    for wid in 0..WRITERS {
        let (runner, next, newest, failures, op_log) = (
            runner.clone(),
            next.clone(),
            newest.clone(),
            failures.clone(),
            op_log.clone(),
        );
        let (ops, seed) = (w.ops, w.seed);
        writers.push(tokio::spawn(async move {
            let mut log = Vec::new();
            let result = write(&runner, seed, wid, tables, ops, &next, &newest, &mut log).await;
            op_log
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend(log);
            if let Err(e) = result {
                failures
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(e);
            }
        }));
    }

    // The disturbances, as the writers reach their ops.
    for d in disturb {
        let target = at_op(d);
        while next.load(Ordering::SeqCst) < target.min(w.ops)
            && !writers.iter().all(JoinHandle::is_finished)
        {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        match d {
            Disturbance::DropInvalidation { .. } => subs.drop_next_batch(),
            Disturbance::Disconnect { .. } => {
                for watcher in &mut watchers {
                    resume(watcher, &sessions).await?;
                    report.resumes += 1;
                }
            }
            _ => {}
        }
    }
    for writer in writers {
        writer.await.map_err(|e| format!("a writer: {e}"))?;
    }
    let writers_done = Instant::now();
    report.ops = std::mem::take(
        &mut *op_log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    report.ops.sort_by_key(|o| o.op);
    for e in failures
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .drain(..)
    {
        report
            .violations
            .push(Violation::new(ViolationKind::Workload, None, e));
    }

    // Every session reaches the last commit: told about it as a client that
    // made it would be, it advances its version to it.
    let target = newest.load(Ordering::SeqCst);
    for watcher in &watchers {
        sessions.mutation_committed(&watcher.session.id, target);
    }
    let give_up = Instant::now() + CATCH_UP;
    loop {
        let behind: Vec<usize> = watchers
            .iter()
            .filter(|w| lock(&w.log).client.version.ts < target)
            .map(|w| w.index)
            .collect();
        if behind.is_empty() {
            break;
        }
        if Instant::now() >= give_up {
            for index in behind {
                report.violations.push(Violation::new(
                    ViolationKind::NotCaughtUp,
                    Some(index),
                    format!("did not reach the last commit ({target}) within {CATCH_UP:?}"),
                ));
            }
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    for watcher in &watchers {
        watcher.stop.cancel();
    }
    report.subs = subs.stats();
    shutdown.cancel();
    for watcher in &mut watchers {
        let consumer = std::mem::replace(&mut watcher.consumer, tokio::spawn(async {}));
        let _ = consumer.await;
    }
    let _ = recorder.await;
    let ticks = std::mem::take(
        &mut *ticks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    report.ticks = ticks.len();
    let lagged = ticks_lagged.load(Ordering::SeqCst);
    if lagged > 0 {
        report.violations.push(Violation::new(
            ViolationKind::Workload,
            None,
            format!("the tick recorder lagged and missed {lagged} ticks"),
        ));
    }

    // Every recorded state against fresh evaluations, then the ticks
    // between Transitions.
    let mut fresh = Fresh::new();
    for watcher in &watchers {
        let log = std::mem::take(&mut *lock(&watcher.log));
        report.violations.extend(log.violations);
        report.transitions += log.records.len();
        for record in &log.records {
            if record.results.len() != watcher.queries.len() {
                report.violations.push(Violation::new(
                    ViolationKind::Stale,
                    Some(watcher.index),
                    format!(
                        "at ts {}: {} results held for {} queries",
                        record.end.ts,
                        record.results.len(),
                        watcher.queries.len()
                    ),
                ));
            }
            for q in &watcher.queries {
                let want = fresh_at(&mut fresh, &runner, q, record.end.ts).await?;
                report.checked += 1;
                let held = record.results.get(&q.id);
                if !same(held, want) {
                    report.violations.push(Violation::new(
                        ViolationKind::Stale,
                        Some(watcher.index),
                        format!(
                            "query {} ({} {:?}) at ts {}: held {}, fresh {}",
                            q.id,
                            q.function,
                            q.args,
                            record.end.ts,
                            short(&format!("{held:?}")),
                            short(&format!("{want:?}")),
                        ),
                    ));
                }
            }
        }

        let arrivals: Vec<Arrival> = log
            .records
            .iter()
            .map(|r| Arrival {
                ts: r.end.ts,
                arrived: r.arrived,
                resent: r.resent,
            })
            .collect();
        let mut late = BTreeSet::new();
        for (p, tick) in late_ticks(&arrivals, &ticks, LATE_BOUND) {
            if late.contains(&p) {
                continue;
            }
            let (record, next) = (&log.records[p], &log.records[p + 1]);
            for q in &watcher.queries {
                let want = fresh_at(&mut fresh, &runner, q, tick).await?;
                report.checked += 1;
                let held = record.results.get(&q.id);
                if !same(held, want) {
                    late.insert(p);
                    report.violations.push(Violation::new(
                        ViolationKind::Late,
                        Some(watcher.index),
                        format!(
                            "query {} ({} {:?}) changed by tick {tick}: held {} since ts {}, fresh {}; \
                             the next Transition was at ts {}, {:?} after the Transition at ts {}",
                            q.id,
                            q.function,
                            q.args,
                            short(&format!("{held:?}")),
                            record.end.ts,
                            short(&format!("{want:?}")),
                            next.end.ts,
                            next.arrived.saturating_duration_since(record.arrived),
                            record.end.ts,
                        ),
                    ));
                    break;
                }
            }
        }

        report.sessions.push(SessionTrace {
            index: watcher.index,
            queries: watcher
                .queries
                .iter()
                .map(|q| format!("{}: {} {:?}", q.id, q.function, q.args))
                .collect(),
            records: Vec::new(),
        });
        report.live_updates.push(
            log.records
                .iter()
                .skip(1)
                .filter(|r| r.updates && !r.resent && r.arrived < writers_done)
                .count(),
        );
        if let Some(trace) = report.sessions.last_mut() {
            trace.records = log.records;
        }
    }
    Ok(())
}

fn short(text: &str) -> String {
    if text.len() <= 160 {
        text.to_string()
    } else {
        let mut end = 160;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &text[..end])
    }
}

/// Defines `name` with the index `by_n` on `n`.
async fn define(store: &Store, name: &str) -> Result<(), String> {
    let name = name.to_string();
    let specs = vec![IndexSpec {
        name: "by_n".into(),
        fields: vec!["n".into()],
    }];
    let mut opts = TxnOptions::new("checker.define");
    opts.commit_mode = Some(CommitMode::TwoPc);
    store
        .run(opts, move |txn| {
            let (name, specs) = (name.clone(), specs.clone());
            Box::pin(async move {
                catalog::define_table(
                    txn,
                    &AppKeys::dedicated(),
                    &name,
                    &specs,
                    &crate::Limits::default(),
                )
                .await
                .map_err(|e| TxnError::Fatal(e.to_string()))
            })
        })
        .await
        .map(|_| ())
        .map_err(|e| format!("defining {}: {e}", "a table"))
}

/// A session's queries, each bounded: ranges and equalities on `by_n` with
/// a limit, the top of the index, a seeded document by id, and the newest
/// documents.
fn queries(rng: &mut ChaCha8Rng, tables: usize, seeded: &[Vec<LiveValue>]) -> Vec<Query> {
    (1..=QUERIES)
        .map(|id| {
            let t = rng.random_range(0..tables);
            let table = s(&table_name(t));
            let (function, args) = match rng.random_range(0..5) {
                0 => {
                    let lo = rng.random_range(0..N_VALUES - 4);
                    let hi = lo + rng.random_range(2..6);
                    (
                        QUERY,
                        obj(&[
                            ("table", table),
                            ("index", s("by_n")),
                            ("lower", obj(&[("value", n(lo))])),
                            (
                                "upper",
                                obj(&[("value", n(hi)), ("inclusive", LiveValue::Bool(false))]),
                            ),
                            ("limit", n(20)),
                        ]),
                    )
                }
                1 => (
                    QUERY,
                    obj(&[
                        ("table", table),
                        ("index", s("by_n")),
                        ("order", s("desc")),
                        ("limit", n(5)),
                    ]),
                ),
                2 => (
                    QUERY,
                    obj(&[
                        ("table", table),
                        ("index", s("by_n")),
                        (
                            "eq",
                            LiveValue::Array(vec![n(rng.random_range(0..N_VALUES))]),
                        ),
                        ("limit", n(20)),
                    ]),
                ),
                3 => {
                    let ids = &seeded[t];
                    let id = ids[rng.random_range(0..ids.len())].clone();
                    (GET, obj(&[("id", id)]))
                }
                _ => (
                    QUERY,
                    obj(&[("table", table), ("order", s("desc")), ("limit", n(10))]),
                ),
            };
            Query { id, function, args }
        })
        .collect()
}

/// Runs a mutation with the idempotency key `key`, retrying it on a
/// retryable storage error (out of time, a conflict, not applied, an
/// unknown outcome) as a client would. Every try carries the same key, so a
/// try after one that committed without an answer is replayed (`replayed`,
/// a success with the first commit's result) instead of applying the op
/// twice: a delete retried that way does not fail with "not found".
async fn mutate(
    runner: &Runner,
    f: &Arc<dyn Function>,
    args: &LiveValue,
    key: String,
) -> Result<crate::Mutated, String> {
    let mut tries = 0;
    loop {
        tries += 1;
        match runner
            .mutate(f.clone(), args.clone(), Some(key.clone()))
            .await
        {
            Ok(m) => return Ok(m),
            Err(LiveError::Txn(
                TxnError::Deadline
                | TxnError::Conflict
                | TxnError::NotApplied(_)
                | TxnError::Undetermined { .. },
            )) if tries < MUTATION_TRIES => {}
            Err(e) => return Err(format!("{e} (try {tries})")),
        }
    }
}

/// One writer of the workload seeded `seed`: takes ops from `next` until
/// `ops`, inserting into, patching and deleting its own documents, logging
/// each committed op in `log`; `newest` keeps the latest commit. Op `op`
/// carries the idempotency key `chk-{seed}-{wid}-{op}`.
#[allow(clippy::too_many_arguments)]
async fn write(
    runner: &Runner,
    seed: u64,
    wid: usize,
    tables: usize,
    ops: usize,
    next: &AtomicUsize,
    newest: &AtomicU64,
    log: &mut Vec<OpRecord>,
) -> Result<(), String> {
    let mut rng = ChaCha8Rng::seed_from_u64(seed.wrapping_mul(1_000).wrapping_add(wid as u64));
    let (insert, patch, delete) = (sys(INSERT)?, sys(PATCH)?, sys(DELETE)?);
    let mut own: Vec<Vec<LiveValue>> = vec![Vec::new(); tables];
    loop {
        let op = next.fetch_add(1, Ordering::SeqCst);
        if op >= ops {
            return Ok(());
        }
        let t = rng.random_range(0..tables);
        let roll = rng.random_range(0..10);
        let value = rng.random_range(0..N_VALUES);
        let (f, args, kind, target) = if roll < 5 || own[t].is_empty() {
            (
                insert.clone(),
                obj(&[
                    ("table", s(&table_name(t))),
                    ("fields", obj(&[("n", n(value)), ("w", n(wid as i64))])),
                ]),
                OpKind::Insert,
                None,
            )
        } else if roll < 8 {
            let id = own[t][rng.random_range(0..own[t].len())].clone();
            (
                patch.clone(),
                obj(&[("id", id.clone()), ("fields", obj(&[("n", n(value))]))]),
                OpKind::Patch,
                Some(id),
            )
        } else {
            let at = rng.random_range(0..own[t].len());
            let id = own[t].swap_remove(at);
            (
                delete.clone(),
                obj(&[("id", id.clone())]),
                OpKind::Delete,
                Some(id),
            )
        };
        let m = mutate(runner, &f, &args, format!("chk-{seed}-{wid}-{op}"))
            .await
            .map_err(|e| format!("writer {wid}, op {op}: {e}"))?;
        if kind == OpKind::Insert {
            own[t].push(m.result.clone());
        }
        newest.fetch_max(m.commit_ts.0, Ordering::SeqCst);
        log.push(OpRecord {
            writer: wid,
            op,
            kind,
            table: t,
            n: value,
            target,
            seeded: false,
            result: m.result,
            commit_ts: m.commit_ts.0,
            replayed: m.replayed,
        });
    }
}

/// Consumes `session`'s Transitions into `log` until stopped or closed.
async fn consume(index: usize, session: Session, log: Arc<Mutex<Log>>, stop: CancellationToken) {
    // Whether the chunks of the Transition being received carried updates.
    let mut carried = false;
    loop {
        let item = tokio::select! {
            item = session.outbox.pop() => item,
            () = stop.cancelled() => return,
        };
        let t = match item {
            None => return,
            Some(Ok(t)) => t,
            Some(Err(e)) => {
                if !stop.is_cancelled() {
                    lock(&log).violations.push(Violation::new(
                        ViolationKind::SessionClosed,
                        Some(index),
                        e.to_string(),
                    ));
                }
                return;
            }
        };
        let mut log = lock(&log);
        let start = Version::from_proto(t.start.as_option());
        let end = Version::from_proto(t.end.as_option());
        let heartbeat = start == end && t.updates.is_empty();
        let resent = start == end && log.resumed;
        carried |= !t.updates.is_empty();
        if !t.more {
            log.resumed = false;
        }
        if !heartbeat && !resent && !t.more && !follows(start, end) {
            log.violations.push(Violation::new(
                ViolationKind::VersionOrder,
                Some(index),
                format!("a Transition from {start:?} to {end:?}"),
            ));
        }
        match log.client.apply(&t) {
            Ok(true) => {
                let record = Record {
                    end: log.client.version,
                    results: log.client.results.clone(),
                    arrived: Instant::now(),
                    updates: std::mem::take(&mut carried),
                    resent,
                };
                log.records.push(record);
            }
            Ok(false) => {}
            Err(e) => {
                log.violations.push(Violation::new(
                    ViolationKind::Gap,
                    Some(index),
                    e.to_string(),
                ));
                return;
            }
        }
    }
}

/// Disconnects `watcher`'s session and resumes it from the client's last
/// version with the same query set.
async fn resume(watcher: &mut Watcher, sessions: &Sessions) -> Result<(), String> {
    watcher.stop.cancel();
    watcher.session.outbox.client_gone();
    let consumer = std::mem::replace(&mut watcher.consumer, tokio::spawn(async {}));
    let _ = consumer.await;
    let last = {
        let mut log = lock(&watcher.log);
        log.resumed = true;
        log.client.version
    };
    let session = sessions
        .open(Start::Resume {
            last,
            set: watcher.set.clone(),
        })
        .map_err(|e| format!("resuming session {}: {e}", watcher.index))?;
    let stop = CancellationToken::new();
    watcher.consumer = tokio::spawn(consume(
        watcher.index,
        session.clone(),
        watcher.log.clone(),
        stop.clone(),
    ));
    watcher.session = session;
    watcher.stop = stop;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    fn arrival(ts: u64, arrived: Instant, resent: bool) -> Arrival {
        Arrival {
            ts,
            arrived,
            resent,
        }
    }

    /// A tick between two Transitions is checked only when the next
    /// Transition came more than the bound after it.
    #[test]
    fn ticks_between_transitions_are_checked_past_the_bound() {
        let base = Instant::now();
        let ticks = vec![
            (10, at(base, 0)),
            (20, at(base, 100)),
            (30, at(base, 200)),
            (40, at(base, 1_500)),
        ];
        // On time: the Transition at 30 came 50 ms after tick 20.
        let prompt = [
            arrival(10, at(base, 5), false),
            arrival(30, at(base, 250), false),
        ];
        assert!(late_ticks(&prompt, &ticks, LATE_BOUND).is_empty());
        // Late: the Transition at 40 came 1.4 s after tick 20 and 1.3 s
        // after tick 30; tick 40 itself is the Transition's own.
        let late = [
            arrival(10, at(base, 5), false),
            arrival(40, at(base, 1_501), false),
        ];
        assert_eq!(
            late_ticks(&late, &ticks, LATE_BOUND),
            vec![(0, 20), (0, 30)]
        );
        // No tick strictly between: nothing to check.
        let adjacent = [
            arrival(20, at(base, 101), false),
            arrival(30, at(base, 3_000), false),
        ];
        assert!(late_ticks(&adjacent, &ticks, LATE_BOUND).is_empty());
    }

    /// After a resume the bound counts from the re-sent Transition: the
    /// session could not deliver while it was disconnected.
    #[test]
    fn a_resume_restarts_the_bound() {
        let base = Instant::now();
        let ticks = vec![
            (10, at(base, 0)),
            (20, at(base, 100)),
            (30, at(base, 1_400)),
        ];
        let resumed = [
            arrival(10, at(base, 5), false),
            arrival(10, at(base, 1_300), true),
            arrival(30, at(base, 1_450), false),
        ];
        assert!(late_ticks(&resumed, &ticks, LATE_BOUND).is_empty());
        let stalled = [
            arrival(10, at(base, 5), false),
            arrival(10, at(base, 1_300), true),
            arrival(30, at(base, 2_400), false),
        ];
        assert_eq!(late_ticks(&stalled, &ticks, LATE_BOUND), vec![(1, 20)]);
    }

    /// A held error matches a fresh one with the same code only.
    #[test]
    fn held_errors_compare_by_code() {
        let not_found = LiveError::NotFound("a".into()).to_proto();
        let invalid = LiveError::InvalidArgument("b".into()).to_proto();
        let held = |e: &pb::LiveError| Some(QueryResult::Error(e.clone()));
        assert!(same(held(&not_found).as_ref(), &Err(not_found.clone())));
        assert!(!same(held(&invalid).as_ref(), &Err(not_found.clone())));
        assert!(!same(None, &Err(not_found)));
    }

    /// Repeated tick timestamps count once, at their first arrival.
    #[test]
    fn repeated_ticks_count_once() {
        let base = Instant::now();
        let ticks = vec![
            (10, at(base, 0)),
            (20, at(base, 100)),
            (20, at(base, 1_200)),
        ];
        let records = [
            arrival(10, at(base, 5), false),
            arrival(30, at(base, 1_250), false),
        ];
        assert_eq!(late_ticks(&records, &ticks, LATE_BOUND), vec![(0, 20)]);
    }
}
