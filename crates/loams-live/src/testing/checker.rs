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
//!   state, not only the updated queries, makes a missed invalidation show:
//!   a commit that changed a subscribed result is then reflected by the
//!   first Transition at or after it, or that Transition's state is stale;
//! - **versions strictly increase** per session (a heartbeat repeats its
//!   version by design, and so may the first Transition of a resumed
//!   session, which re-sends the full results at the client's last
//!   version), and every Transition starts at the client's version;
//! - **every session catches up** with the last commit;
//! - **resumed sessions converge**: a session disconnected mid-run resumes
//!   from its last version and its later states pass the same checks.
//!
//! The subscription manager runs with its safety rerun off, so a missed
//! invalidation stays visible instead of being repaired.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use buffa::MessageField;
use loams_kv::{CommitMode, EmbeddedConfig, Store, StoreConfig, Ts, TxnError, TxnOptions};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::workload::{Disturbance, Workload};
use crate::catalog::{self, IndexSpec};
use crate::session::{ClientState, QueryResult, Session, SessionConfig, Sessions, Start, Version};
use crate::subs::{SubsConfig, Subscriptions};
use crate::system::{DELETE, GET, INSERT, PATCH, QUERY};
use crate::{AppKeys, Function, LiveConfig, LiveError, LiveValue, Runner, deploy, pb, system};

/// What the checker saw.
#[derive(Debug, Clone, Default)]
pub struct Report {
    /// Complete Transitions applied, over every session.
    pub transitions: usize,
    /// Query results compared with a fresh evaluation.
    pub checked: usize,
    /// Sessions disconnected and resumed.
    pub resumes: usize,
    pub violations: Vec<Violation>,
}

/// Which guarantee broke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViolationKind {
    /// A held result differs from a fresh evaluation at the Transition's
    /// `end.ts`.
    Stale,
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
const SEED_DOCS: usize = 8;
/// Queries per session.
const QUERIES: u32 = 4;
/// Concurrent writers.
const WRITERS: usize = 8;
/// Tries per mutation (a retryable storage error is tried again).
const MUTATION_TRIES: u32 = 5;
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
struct Record {
    end: Version,
    results: BTreeMap<u32, QueryResult>,
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
    let mut rng = StdRng::seed_from_u64(w.seed);
    let insert = sys(INSERT)?;
    let mut seeded: Vec<Vec<LiveValue>> = vec![Vec::new(); tables];
    for (t, ids) in seeded.iter_mut().enumerate() {
        for _ in 0..SEED_DOCS {
            let args = obj(&[
                ("table", s(&table_name(t))),
                ("fields", obj(&[("n", n(rng.random_range(0..N_VALUES)))])),
            ]);
            let m = mutate(&runner, &insert, &args)
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
    let mut writers = Vec::new();
    for wid in 0..WRITERS {
        let (runner, next, newest, failures) = (
            runner.clone(),
            next.clone(),
            newest.clone(),
            failures.clone(),
        );
        let ops = w.ops;
        let seed = w.seed.wrapping_mul(1_000).wrapping_add(wid as u64);
        writers.push(tokio::spawn(async move {
            if let Err(e) = write(&runner, seed, wid, tables, ops, &next, &newest).await {
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
    shutdown.cancel();
    for watcher in &mut watchers {
        let consumer = std::mem::replace(&mut watcher.consumer, tokio::spawn(async {}));
        let _ = consumer.await;
    }

    // Every recorded state against fresh evaluations.
    let mut fresh: HashMap<(&str, String, u64), Result<LiveValue, String>> = HashMap::new();
    for watcher in &watchers {
        let log = std::mem::take(&mut *lock(&watcher.log));
        report.violations.extend(log.violations);
        report.transitions += log.records.len();
        for record in &log.records {
            for q in &watcher.queries {
                // Sessions watching the same query at the same tick share
                // one evaluation.
                let key = (q.function, format!("{:?}", q.args), record.end.ts);
                let want = match fresh.entry(key) {
                    Entry::Occupied(found) => found.into_mut(),
                    Entry::Vacant(slot) => {
                        let f = sys(q.function)?;
                        let got = runner
                            .query(&*f, q.args.clone(), Ts(record.end.ts))
                            .await
                            .map(|r| r.result)
                            .map_err(|e| e.to_string());
                        slot.insert(got)
                    }
                };
                report.checked += 1;
                let held = record.results.get(&q.id);
                let same = match (held, &*want) {
                    (Some(QueryResult::Value(v)), Ok(f)) => v == f,
                    (Some(QueryResult::Error(_)), Err(_)) => true,
                    _ => false,
                };
                if !same {
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
fn queries(rng: &mut StdRng, tables: usize, seeded: &[Vec<LiveValue>]) -> Vec<Query> {
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

/// Runs a mutation, retrying it on a retryable storage error (out of time,
/// a conflict, not applied, an unknown outcome) as a client would. Its
/// effects are checked against fresh evaluations, not against expected
/// contents, so a mutation applied twice is harmless.
async fn mutate(
    runner: &Runner,
    f: &Arc<dyn Function>,
    args: &LiveValue,
) -> Result<crate::Mutated, String> {
    let mut tries = 0;
    loop {
        tries += 1;
        match runner.mutate(f.clone(), args.clone(), None).await {
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

/// One writer: takes ops from `next` until `ops`, inserting into, patching
/// and deleting its own documents; `newest` keeps the latest commit.
async fn write(
    runner: &Runner,
    seed: u64,
    wid: usize,
    tables: usize,
    ops: usize,
    next: &AtomicUsize,
    newest: &AtomicU64,
) -> Result<(), String> {
    let mut rng = StdRng::seed_from_u64(seed);
    let (insert, patch, delete) = (sys(INSERT)?, sys(PATCH)?, sys(DELETE)?);
    let mut own: Vec<Vec<LiveValue>> = vec![Vec::new(); tables];
    loop {
        let op = next.fetch_add(1, Ordering::SeqCst);
        if op >= ops {
            return Ok(());
        }
        let t = rng.random_range(0..tables);
        let roll = rng.random_range(0..10);
        let value = n(rng.random_range(0..N_VALUES));
        let (f, args, inserted) = if roll < 5 || own[t].is_empty() {
            (
                insert.clone(),
                obj(&[
                    ("table", s(&table_name(t))),
                    ("fields", obj(&[("n", value), ("w", n(wid as i64))])),
                ]),
                true,
            )
        } else if roll < 8 {
            let id = own[t][rng.random_range(0..own[t].len())].clone();
            (
                patch.clone(),
                obj(&[("id", id), ("fields", obj(&[("n", value)]))]),
                false,
            )
        } else {
            let at = rng.random_range(0..own[t].len());
            let id = own[t].swap_remove(at);
            (delete.clone(), obj(&[("id", id)]), false)
        };
        let m = mutate(runner, &f, &args)
            .await
            .map_err(|e| format!("writer {wid}, op {op}: {e}"))?;
        if inserted {
            own[t].push(m.result);
        }
        newest.fetch_max(m.commit_ts.0, Ordering::SeqCst);
    }
}

/// Consumes `session`'s Transitions into `log` until stopped or closed.
async fn consume(index: usize, session: Session, log: Arc<Mutex<Log>>, stop: CancellationToken) {
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
