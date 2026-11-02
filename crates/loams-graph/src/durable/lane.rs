//! The per-graph write lane (design §48 §6.2 steps 1–8; rulings R0.5, R9.3, R9.8, R10.2).
//!
//! Every explicit transaction on a durable graph's engine runs here, one at a time: a write
//! statement, an atomic batch (read-only ones included, R0.5) and each statement of a non-atomic
//! batch that writes. Writers queue; the first becomes the **leader** and runs a **group**:
//!
//! 1. it takes the graph's gate exclusively (R9.3), so no client statement runs until step 6;
//! 2. it runs queued transactions back to back, each `begin` → statements → `commit` → capture
//!    (§6.3) → encode, until the queue is empty, the group's window (2 ms) has passed or its
//!    records hold 256 KiB;
//! 3. it appends the group's records to the graph's stream in one log batch, one record per
//!    transaction, `txn_seq` one per record (R10.4);
//! 4. once the append is acknowledged it publishes the durable position and records the
//!    idempotency keys;
//! 5. it runs Grafeo's `gc` (which prunes the CDC log, R9.9);
//! 6. it releases the gate, answers every transaction of the group, and hands the lead to the
//!    next queued writer, if any.
//!
//! A transaction the engine refuses (a statement error, a write conflict) is rolled back and
//! answered with its error. A failed append, a capture the engine's state makes impossible (R9.8)
//! or a record that cannot be encoded (R10.2) is not: the group's commits are in the engine but
//! not in the log, so the graph is marked for reload before the gate is released, and every
//! transaction of the group is answered `UNAVAILABLE`/`graph_reloading`. No client ever read
//! them (the gate), and none was acknowledged.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::{Mutex, PoisonError};
use std::time::Instant;

use bytes::Bytes;
use grafeo_engine::transaction::IsolationLevel;

use super::faults::KillPoint;
use super::idempotency::Outcome;
use super::{Commit, ConsistencyToken, Durable, Isolation, WriteOptions};
use crate::changeset::capture::CdcCapture;
use crate::changeset::{ChangeOp, Counters, GraphChangeSet, GraphRecord, LinkApply, encode};
use crate::classify::Access;
use crate::engine::{
    BatchStatement, Graph, GraphError, GraphResult, as_engine_error, run_on_session,
};
use loams_common::meta::LinkId;

/// The most records one group appends: well under the log writer's 10 000 per append.
pub const MAX_GROUP_RECORDS: usize = 1_024;

/// One transaction for the lane.
pub(crate) struct LaneJob {
    /// The statements, gated already, run in one transaction.
    pub(crate) statements: Vec<BatchStatement>,
    /// The engine role they need.
    pub(crate) access: Access,
    /// Whether the engine's plan of any statement has a mutation ([`GraphResult::wrote`]).
    pub(crate) writes: bool,
    /// Key, isolation and version.
    pub(crate) options: WriteOptions,
    /// Rows kept per result.
    pub(crate) max_rows: Option<usize>,
    /// A link target's commit instead of statements (GR1 Task 18; `statements` is empty).
    pub(crate) link: Option<LinkJob>,
}

/// What a link writes into a graph's engine: given the engine and the links' index of its
/// elements, applies a batch and answers the change operations it applied, in order.
pub(crate) type LinkApplyFn = Box<
    dyn FnOnce(&grafeo::GrafeoDB, &mut crate::link::LinkIndex) -> Result<Vec<ChangeOp>, GraphError>
        + Send,
>;

/// A link target's commit (R0.17, GR1 Task 18): a batch of its source applied in one record
/// together with the link's progress, at the link's expected version.
///
/// The batch is applied to the engine's store directly, as a replay applies a record (no Grafeo
/// transaction, no change capture: the operations are the record), under the exclusive gate that
/// already keeps every non-durable write from clients. A failure after the first write, or an
/// append that fails, reloads the graph from its stream, as for a GQL write.
pub(crate) struct LinkJob {
    /// The link's progress once the record is durable. Its `version` is the target's expected
    /// version plus one: the commit is refused (`VersionMismatch`) at any other version. `None`
    /// for a batch of operations no link writes: an algorithm's write-back (GR1 Task 22).
    pub(crate) progress: Option<LinkApply>,
    /// Applies the batch.
    pub(crate) apply: LinkApplyFn,
}

/// A transaction the lane made durable (or answered from the idempotency index).
#[derive(Debug)]
pub(crate) struct Committed {
    /// One result per statement; empty when answered from the index.
    pub(crate) results: Vec<GraphResult>,
    /// Its token, epoch and counters.
    pub(crate) commit: Commit,
}

enum Message {
    Done(Result<Committed, GraphError>),
    /// The receiver leads the next group.
    Lead,
}

struct Queued {
    job: LaneJob,
    reply: mpsc::Sender<Message>,
}

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Queued>,
    leading: bool,
}

/// A graph's write lane.
#[derive(Default)]
pub(crate) struct WriteLane {
    queue: Mutex<Queue>,
}

/// Why a transaction did not commit.
enum Step {
    /// The engine refused it and rolled it back: answer the error.
    Refused(GraphError),
    /// It committed in the engine and cannot be logged: the graph must reload.
    Fatal(GraphError),
}

/// A transaction that committed in the engine, waiting for its group's append.
struct Ran {
    results: Vec<GraphResult>,
    /// Its record's index in the group, when it has one.
    record: Option<usize>,
    commit_epoch: u64,
    counters: Counters,
    key: Option<String>,
    /// The link progress its record carries.
    link: Option<LinkApply>,
}

/// What one queued writer is answered once the group ends.
enum Answer {
    /// Committed in the engine; its fate is the append's.
    Ran(Ran),
    /// The same key as an earlier transaction of this group: answered as its replay.
    SameKey(usize),
    /// Settled without touching the engine's state (a replay from the index, a version
    /// mismatch), or refused by the engine. Still answered `graph_reloading` if the group fails:
    /// a refusal may have been caused by a write the log then refused.
    Settled(Result<Committed, GraphError>),
}

impl WriteLane {
    fn queue(&self) -> std::sync::MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `job` in the lane and waits until it is durable (or refused). Called on a statement
    /// pool thread inside the graph's panic containment.
    pub(crate) fn submit(
        &self,
        graph: &Graph,
        durable: &Durable,
        job: LaneJob,
    ) -> Result<Committed, GraphError> {
        let (reply, answer) = mpsc::channel();
        let lead = {
            let mut queue = self.queue();
            queue.jobs.push_back(Queued { job, reply });
            !std::mem::replace(&mut queue.leading, true)
        };
        if lead {
            self.lead(graph, durable);
        }
        loop {
            match answer.recv() {
                Ok(Message::Done(result)) => return result,
                Ok(Message::Lead) => self.lead(graph, durable),
                // The leader that held this transaction panicked: the graph is poisoned.
                Err(_) => return Err(GraphError::Reloading),
            }
        }
    }

    /// Runs one group, then hands the lead on.
    fn lead(&self, graph: &Graph, durable: &Durable) {
        let answers = {
            let _exclusive = graph.gate().exclusive();
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.group(graph, durable)
            })) {
                Ok(answers) => answers,
                Err(_) => {
                    // The transactions taken into the group are dropped with their reply
                    // channels, and their writers answer `graph_reloading`.
                    graph.poison("the graph engine panicked in the write lane");
                    Vec::new()
                }
            }
        };
        for (reply, answer) in answers {
            let _ = reply.send(Message::Done(answer));
        }
        let mut queue = self.queue();
        while let Some(next) = queue.jobs.front() {
            if next.reply.send(Message::Lead).is_ok() {
                return;
            }
            queue.jobs.pop_front();
        }
        queue.leading = false;
    }

    /// The next transaction for the group, unless the group must end before it: one that names
    /// a version waits for a group of its own, so the version it is checked against is durable.
    fn next(&self, group_has_records: bool) -> Option<Queued> {
        let mut queue = self.queue();
        let front = queue.jobs.front()?;
        if group_has_records && front.job.options.if_version.is_some() {
            return None;
        }
        queue.jobs.pop_front()
    }

    /// Runs a group under the exclusive gate and answers how each writer is answered.
    #[allow(clippy::type_complexity)]
    fn group(
        &self,
        graph: &Graph,
        durable: &Durable,
    ) -> Vec<(mpsc::Sender<Message>, Result<Committed, GraphError>)> {
        let config = durable.log.group();
        let started = Instant::now();
        let mut answers: Vec<(mpsc::Sender<Message>, Answer)> = Vec::new();
        let mut records: Vec<Bytes> = Vec::new();
        let mut bytes = 0;
        let mut keys: HashMap<String, usize> = HashMap::new();
        // Each link's version after the records this group has taken so far.
        let mut link_versions: HashMap<LinkId, u64> = HashMap::new();
        let mut fatal = None;

        if graph.needs_reopen() {
            // Marked for reload by an earlier group: nothing more runs on this engine.
            let mut out = Vec::new();
            while let Some(queued) = self.next(false) {
                out.push((queued.reply, Err(GraphError::Reloading)));
            }
            return out;
        }

        while let Some(Queued { mut job, reply }) = self.next(!records.is_empty()) {
            let now = durable.log.now_ms();
            if let Some(key) = &job.options.idempotency_key {
                if let Some(&first) = keys.get(key) {
                    answers.push((reply, Answer::SameKey(first)));
                    continue;
                }
                let earlier = durable
                    .idempotency
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(key, now);
                if let Some(outcome) = earlier {
                    answers.push((reply, Answer::Settled(Ok(replayed(durable, outcome)))));
                    continue;
                }
            }
            if let Some(expected) = job.options.if_version {
                let current = durable.position.offset();
                if expected != current {
                    answers.push((
                        reply,
                        Answer::Settled(Err(GraphError::VersionMismatch { expected, current })),
                    ));
                    continue;
                }
            }
            let ran = match job.link.take() {
                Some(link) => {
                    let current = link.progress.as_ref().map(|progress| {
                        link_versions
                            .get(&progress.link)
                            .copied()
                            .unwrap_or_else(|| {
                                durable
                                    .link_progress(progress.link)
                                    .map_or(0, |progress| progress.version)
                            })
                    });
                    run_link(graph, durable, link, current)
                }
                None => run(graph, &job).map(|(results, captured, commit_epoch)| {
                    (results, captured, commit_epoch, None)
                }),
            };
            match ran {
                Err(Step::Refused(err)) => answers.push((reply, Answer::Settled(Err(err)))),
                Err(Step::Fatal(err)) => {
                    fatal = Some(err);
                    answers.push((reply, Answer::Settled(Err(GraphError::Reloading))));
                    break;
                }
                Ok((results, captured, commit_epoch, link)) => {
                    let key = job.options.idempotency_key.clone();
                    // A keyed transaction that changed nothing is appended anyway, so its key is
                    // durable (Task 11 ruling), and so is a link's commit, for its progress; an
                    // unkeyed one has nothing to log.
                    let record = if captured.ops.is_empty() && key.is_none() && link.is_none() {
                        None
                    } else {
                        let record = GraphRecord::ChangeSet(GraphChangeSet {
                            graph: durable.graph,
                            lease_epoch: durable.lease_epoch(),
                            commit_epoch,
                            txn_seq: durable.take_txn_seq(),
                            idempotency_key: key.clone(),
                            link: link.clone(),
                            ops: captured.ops,
                        });
                        match encode(&record) {
                            Ok(encoded) if encoded.len() > config.max_record_bytes => {
                                fatal = Some(GraphError::engine(
                                    "a change set larger than one record may be",
                                ));
                                let too_large = GraphError::OverLimit(format!(
                                    "the transaction changed {} bytes of the graph; one \
                                     transaction may change at most {} (split it)",
                                    encoded.len(),
                                    config.max_record_bytes
                                ));
                                answers.push((reply, Answer::Settled(Err(too_large))));
                                break;
                            }
                            Ok(encoded) => {
                                bytes += encoded.len();
                                records.push(encoded);
                                Some(records.len() - 1)
                            }
                            // R10.2: as a refused capture.
                            Err(err) => {
                                fatal = Some(GraphError::engine(format!(
                                    "encoding the change set: {err}"
                                )));
                                answers.push((reply, Answer::Settled(Err(GraphError::Reloading))));
                                break;
                            }
                        }
                    };
                    if let Some(key) = &key {
                        keys.insert(key.clone(), answers.len());
                    }
                    if let Some(link) = &link {
                        link_versions.insert(link.link, link.version);
                    }
                    answers.push((
                        reply,
                        Answer::Ran(Ran {
                            results,
                            record,
                            commit_epoch: if record.is_some() { commit_epoch } else { 0 },
                            counters: captured.counters,
                            key,
                            link,
                        }),
                    ));
                }
            }
            if bytes >= config.max_bytes
                || records.len() >= MAX_GROUP_RECORDS
                || started.elapsed() >= config.window
            {
                break;
            }
        }

        if let Some(err) = fatal {
            tracing::error!(
                graph = %durable.graph,
                error = %err,
                "a write committed in the engine cannot be logged; the graph reloads from its stream"
            );
            return fail(graph, answers);
        }
        let base = if records.is_empty() {
            None
        } else {
            let count = records.len() as u64;
            let bytes: u64 = records.iter().map(|record| record.len() as u64).sum();
            let now = durable.log.now_ms();
            let batch = records
                .into_iter()
                .map(|value| log_record(value, now))
                .collect();
            durable.log.reach_blocking(KillPoint::Enqueued);
            match durable
                .log
                .append(durable.stream, batch, durable.fence.as_ref())
            {
                Ok(ack) if ack.last_offset.checked_sub(ack.base_offset) == Some(count - 1) => {
                    durable.log.reach_blocking(KillPoint::Appended);
                    // The checkpointer's "64 MiB of log" trigger (GR1 Task 13).
                    durable.log_bytes.fetch_add(bytes, Ordering::AcqRel);
                    Some((ack.base_offset, ack.last_offset + 1, now))
                }
                Ok(ack) => {
                    tracing::error!(graph = %durable.graph, ?ack, count, "the log acknowledged another number of records than the group appended; the graph reloads");
                    return fail(graph, answers);
                }
                Err(err) if super::is_fenced(&err) => {
                    // Another node owns the graph now (I5): this engine never serves again, and
                    // nothing of the group is in the log.
                    tracing::warn!(graph = %durable.graph, epoch = durable.lease_epoch(), "a write group was refused: the graph's owner lease moved on; the graph is fenced here");
                    graph.mark_fenced();
                    return fail(graph, answers);
                }
                Err(err) => {
                    tracing::warn!(graph = %durable.graph, error = %err, "appending a write group failed; the graph reloads from its stream");
                    return fail(graph, answers);
                }
            }
        };

        // Durable: publish, remember the keys, and prune the engine's history.
        let db = graph.db();
        let offset = match base {
            Some((_, next, _)) => next,
            None => durable.position.offset(),
        };
        durable.position.publish(
            offset,
            db.current_epoch().as_u64(),
            (db.node_count() as u64, db.edge_count() as u64),
        );
        durable.log.reach_blocking(KillPoint::Published);
        let token_of = |ran: &Ran| ConsistencyToken {
            stream: durable.stream,
            offset: match (ran.record, base) {
                (Some(index), Some((base, _, _))) => base + index as u64 + 1,
                _ => offset,
            },
        };
        // The links' progress is theirs once durable: their targets' `load` reads it.
        {
            let mut links = durable.links.lock().unwrap_or_else(PoisonError::into_inner);
            for (_, answer) in &answers {
                if let Answer::Ran(Ran {
                    link: Some(progress),
                    ..
                }) = answer
                {
                    super::note_link(&mut links, progress);
                }
            }
        }
        if let Some((_, _, recorded)) = base {
            let mut index = durable
                .idempotency
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            for (_, answer) in &answers {
                if let Answer::Ran(ran) = answer
                    && let Some(key) = &ran.key
                {
                    let outcome = Outcome {
                        token_offset: token_of(ran).offset,
                        commit_epoch: ran.commit_epoch,
                    };
                    index.insert(key, outcome, recorded, durable.log.now_ms());
                }
            }
        }
        db.gc();

        let commits: Vec<Option<Commit>> = answers
            .iter()
            .map(|(_, answer)| match answer {
                Answer::Ran(ran) => Some(Commit {
                    token: token_of(ran),
                    commit_epoch: ran.commit_epoch,
                    counters: ran.counters,
                    replayed: false,
                }),
                _ => None,
            })
            .collect();
        answers
            .into_iter()
            .map(|(reply, answer)| {
                let result = match answer {
                    Answer::Ran(ran) => Ok(Committed {
                        commit: Commit {
                            token: token_of(&ran),
                            commit_epoch: ran.commit_epoch,
                            counters: ran.counters,
                            replayed: false,
                        },
                        results: ran.results,
                    }),
                    Answer::SameKey(first) => match commits.get(first).copied().flatten() {
                        Some(commit) => Ok(Committed {
                            results: Vec::new(),
                            commit: Commit {
                                replayed: true,
                                counters: Counters::default(),
                                ..commit
                            },
                        }),
                        None => Err(GraphError::Reloading),
                    },
                    Answer::Settled(result) => result,
                };
                (reply, result)
            })
            .collect()
    }
}

/// A group that cannot be made durable: the graph is marked for reload (before the gate is
/// released) and every writer of the group is answered `graph_reloading`.
#[allow(clippy::type_complexity)]
fn fail(
    graph: &Graph,
    answers: Vec<(mpsc::Sender<Message>, Answer)>,
) -> Vec<(mpsc::Sender<Message>, Result<Committed, GraphError>)> {
    graph.mark_reloading();
    answers
        .into_iter()
        .map(|(reply, answer)| match answer {
            // The transaction that was too large for a record: retrying it cannot help.
            Answer::Settled(Err(err @ GraphError::OverLimit(_))) => (reply, Err(err)),
            _ => (reply, Err(GraphError::Reloading)),
        })
        .collect()
}

/// Reaches kill point `point` on `graph`'s node (GR1 Task 16; nothing without `test-hooks`).
fn reach(graph: &Graph, point: KillPoint) {
    if let Some(durable) = graph.durable_state() {
        durable.log.reach_blocking(point);
    }
}

/// The answer to a key the index already holds.
fn replayed(durable: &Durable, outcome: Outcome) -> Committed {
    Committed {
        results: Vec::new(),
        commit: Commit {
            token: ConsistencyToken {
                stream: durable.stream,
                offset: outcome.token_offset,
            },
            commit_epoch: outcome.commit_epoch,
            counters: Counters::default(),
            replayed: true,
        },
    }
}

/// One record of the graph's stream: no key, the encoded [`GraphRecord`], stamped with the
/// metastore clock (the idempotency window counts from it, R10.4).
pub(crate) fn log_record(value: Bytes, now_ms: u64) -> loams_log::Record {
    loams_log::Record {
        key: None,
        value: Some(value),
        headers: Vec::new(),
        timestamp_ms: i64::try_from(now_ms).unwrap_or(i64::MAX),
    }
}

/// Runs a link's commit (GR1 Task 18) at the link's `current` version: applies its batch to the
/// engine's store, with the links' index of the graph's elements (built here the first time),
/// and answers the operations as its capture. A batch no link writes (`current` and the job's
/// progress `None`: a write-back, GR1 Task 22) is not version-checked and gets an empty index.
/// The caller holds the gate exclusively.
#[allow(clippy::type_complexity)]
fn run_link(
    graph: &Graph,
    durable: &Durable,
    job: LinkJob,
    current: Option<u64>,
) -> Result<
    (
        Vec<GraphResult>,
        crate::changeset::capture::Captured,
        u64,
        Option<LinkApply>,
    ),
    Step,
> {
    if let (Some(progress), Some(current)) = (&job.progress, current) {
        let expected = progress.version.saturating_sub(1);
        if expected != current {
            return Err(Step::Refused(GraphError::VersionMismatch {
                expected,
                current,
            }));
        }
    }
    reach(graph, KillPoint::Begin);
    let db = graph.db();
    let mut slot = durable
        .link_index
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let mut unlinked = crate::link::LinkIndex::default();
    let index = if job.progress.is_some() {
        slot.get_or_insert_with(|| crate::link::LinkIndex::build(db))
    } else {
        &mut unlinked
    };
    let applied = (job.apply)(db, index);
    let ops = match applied {
        Ok(ops) => ops,
        Err(err) => {
            // Some of the batch may be in the engine: the graph reloads, and the index with it.
            *slot = None;
            return Err(Step::Fatal(err));
        }
    };
    drop(slot);
    for point in [
        KillPoint::Executed,
        KillPoint::Committed,
        KillPoint::Captured,
    ] {
        reach(graph, point);
    }
    // A link's answer carries no rows or counters: its target answers only its version.
    let captured = crate::changeset::capture::Captured {
        counters: Counters::default(),
        ops,
    };
    Ok((
        Vec::new(),
        captured,
        graph.db().current_epoch().as_u64(),
        job.progress,
    ))
}

/// Runs one transaction: `begin` with its isolation, its statements, `commit`, then the capture
/// of what it committed. Its results are resolved (paths to full elements) at the state it left.
fn run(
    graph: &Graph,
    job: &LaneJob,
) -> Result<(Vec<GraphResult>, crate::changeset::capture::Captured, u64), Step> {
    let mut session = graph.session_for(job.access);
    let level = match job.options.isolation {
        Isolation::Snapshot => IsolationLevel::SnapshotIsolation,
        Isolation::Serializable => IsolationLevel::Serializable,
    };
    reach(graph, KillPoint::Begin);
    session
        .begin_transaction_with_isolation(level)
        .map_err(|err| Step::Refused(as_engine_error(err)))?;
    let mut results = Vec::with_capacity(job.statements.len());
    for statement in &job.statements {
        match run_on_session(&session, &statement.text, statement.parameters.clone()) {
            Ok(result) => {
                graph.count_statement();
                results.push(GraphResult::from_engine(result, job.max_rows));
            }
            Err(err) => {
                let _ = session.rollback();
                return Err(Step::Refused(as_engine_error(err)));
            }
        }
    }
    reach(graph, KillPoint::Executed);
    if let Err(err) = session.commit() {
        let _ = session.rollback();
        return Err(match as_engine_error(err) {
            // A conflict changed nothing: the caller retries.
            conflict @ GraphError::WriteConflict => Step::Refused(conflict),
            // Anything else may have left the commit half-applied: rebuild from the log.
            other => Step::Fatal(other),
        });
    }
    // The commit's epoch is the engine's current epoch: the lane holds the graph's exclusive gate
    // from `begin` until the change set is durable, so nothing else commits in between (R0.5).
    // Official Grafeo has no commit listener (D2450 dropped the fork's LG-0001), so the capture
    // reads the commit back from the CDC log; if retention or eviction has already pruned its
    // events (R9.9), the capture fails and the graph reloads from the log.
    let commit_epoch = graph.db().current_epoch().as_u64();
    reach(graph, KillPoint::Committed);
    // A failed capture, injected for one graph by name (`durability.rs`,
    // `a_refused_capture_reloads_and_hides`). The engine bug that used to provoke one (R9.8) is
    // fixed only by the fork's LG-0004, which official Grafeo lacks (D2450).
    #[cfg(feature = "failpoints")]
    if fail::eval("loams_graph::capture", |target| {
        target.as_deref() == Some(graph.name())
    })
    .unwrap_or(false)
    {
        return Err(Step::Fatal(GraphError::engine(
            "change capture: injected failure",
        )));
    }
    let captured = CdcCapture::new(graph.db())
        .and_then(|capture| capture.capture(commit_epoch))
        .map_err(Step::Fatal)?;
    reach(graph, KillPoint::Captured);
    let results = results
        .into_iter()
        .map(|mut result| {
            result.wrote = job.writes;
            graph.resolved(result)
        })
        .collect();
    Ok((results, captured, commit_epoch))
}
