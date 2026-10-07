//! The answers of recently-keyed `WriteDocuments` calls (design §44 §7.3,
//! D610; API1 Task 3).
//!
//! `WriteDocuments` is the first RPC of the API that is not idempotent by
//! construction (plan ruling 2.4): a client that retries after a lost answer
//! would otherwise write the same documents twice. `idempotency_key` says "this
//! is a retry", and this module is what makes that true — the same key under
//! the same request **replays** the first answer (the same token, the same
//! per-op results and the same positions) instead of writing again.
//!
//! ## What it is, precisely
//!
//! - **Per process.** The ledger is this node's memory. A retry that lands on
//!   another node of a cluster is a fresh write there. That is the same gap
//!   every REST write has, and closing it needs a shared store rather than a
//!   local map, so it is stated here rather than assumed away.
//! - **The immediate retry, and nothing longer.** An answer is kept for
//!   [`WINDOW`] and while the ledger holds fewer than [`ENTRIES`] others; past
//!   either bound a repeat is a fresh write. A stale answer is worse than a
//!   late one: a client that is told "created" for a write that did not happen,
//!   or that is handed a token for another request's write, has a wrong answer
//!   with no way to notice.
//! - **Only a repeat of the same request.** An entry is keyed by
//!   (namespace, collection, key) and carries the fingerprint the caller
//!   computed, so one key under two *different* requests does not answer one
//!   with the other's token — it is a second write, which is what a client
//!   reusing a key wrongly has done.
//!
//! ## Two parts, and why
//!
//! [`Ledger::gate`] is a per-key lock and [`Ledger::replay`] /
//! [`Ledger::remember`] are the answers. The lock is what makes the guarantee
//! hold for a retry that arrives *while* the first call is still running, which
//! is the case that matters: without it, two concurrent requests under one key
//! both miss the ledger and both write. It is one `Mutex` per key rather than
//! one for the whole ledger, so unrelated keyed writes do not queue behind each
//! other — and a key nobody is holding any more drops its entry, so the map is
//! bounded by the calls in flight rather than by the keys ever seen.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use loams_proto::loams::document::v1::WriteDocumentsResponse;
use loams_query::Backlog;

/// How long a keyed write's answer can be replayed. Long enough for the retry
/// of a lost answer (a client retries in seconds) and short enough that the
/// ledger is not a durable record of writes that a caller may have forgotten.
pub(super) const WINDOW: Duration = Duration::from_secs(300);

/// The most keyed writes whose answers this process keeps at once. It bounds
/// the ledger's memory: each entry is a token, one enum per op, one position
/// per op and two counters, so the whole ledger is bounded by this times the
/// largest write a caller sent.
pub(super) const ENTRIES: usize = 1024;

/// One keyed write's answer, replayed verbatim by a retry.
#[derive(Debug)]
pub(super) struct Answer {
    /// The answer body, which carries the token in `token` as well.
    pub(super) response: WriteDocumentsResponse,
    /// The backlog the answer was admitted at, which the replayed headers must
    /// report: they are the same numbers the first answer reported.
    pub(super) backlog: Backlog,
}

/// One remembered write.
#[derive(Debug)]
struct Replay {
    namespace: String,
    collection: String,
    key: String,
    fingerprint: u64,
    answer: Answer,
    at: Instant,
}

/// What a gate is keyed by: the collection the write names and the caller's
/// key. Two writes under one key in one collection are one write; the same key
/// in another collection is another.
type GateId = (String, String, String);

/// The keyed writes this process remembers, and the locks it holds for them.
#[derive(Debug, Default)]
pub(super) struct Ledger {
    answers: Mutex<VecDeque<Replay>>,
    gates: Mutex<HashMap<GateId, Weak<tokio::sync::Mutex<()>>>>,
}

impl Ledger {
    /// The lock for one `(namespace, collection, key)`, creating it if this is
    /// the first caller under that key.
    ///
    /// An **empty** key gets a fresh, unlocked gate: an unkeyed write is never
    /// deduplicated, so making every unkeyed write in the process wait on one
    /// mutex would serialise the whole write path for nothing.
    pub(super) fn gate(
        &self,
        namespace: &str,
        collection: &str,
        key: &str,
    ) -> Arc<tokio::sync::Mutex<()>> {
        if key.is_empty() {
            return Arc::new(tokio::sync::Mutex::new(()));
        }
        let id = (namespace.to_owned(), collection.to_owned(), key.to_owned());
        let mut gates = lock(&self.gates);
        if let Some(held) = gates.get(&id).and_then(Weak::upgrade) {
            return held;
        }
        let gate = Arc::new(tokio::sync::Mutex::new(()));
        gates.insert(id, Arc::downgrade(&gate));
        // A key whose gate nobody holds any more is dead weight, and there is
        // one entry per key ever used since the last sweep, so sweep whenever
        // the map has grown past the number of live calls plus one. Retaining
        // only the held gates leaves the map bounded by the calls in flight.
        if gates.len() > lock(&self.answers).len() + 1 {
            gates.retain(|_, held| held.strong_count() > 0);
        }
        gate
    }

    /// The answer a retry of this exact request replays, or `None` when this
    /// key has not been written inside the window.
    ///
    /// A key whose entry has expired, or whose fingerprint is a different
    /// request, is *not* a replay. Expired entries are dropped as they are met
    /// rather than by a sweep, so an idle ledger costs nothing and a busy one
    /// costs one pass.
    pub(super) fn replay(
        &self,
        namespace: &str,
        collection: &str,
        key: &str,
        fingerprint: u64,
    ) -> Option<Answer> {
        let mut answers = lock(&self.answers);
        purge_expired(&mut answers);
        let entry = answers.iter_mut().find(|entry| {
            entry.namespace == namespace
                && entry.collection == collection
                && entry.key == key
                && entry.fingerprint == fingerprint
        })?;
        // Replayed, so its window starts again: a client that keeps retrying a
        // key that keeps failing to arrive should not find the window closed
        // underneath it.
        entry.at = Instant::now();
        Some(clone_answer(&entry.answer))
    }

    /// The answer to remember for a keyed write, evicting the oldest if this
    /// ledger is full.
    pub(super) fn remember(
        &self,
        namespace: &str,
        collection: &str,
        key: &str,
        fingerprint: u64,
        response: WriteDocumentsResponse,
        backlog: Backlog,
    ) {
        let mut answers = lock(&self.answers);
        purge_expired(&mut answers);
        while answers.len() >= ENTRIES {
            answers.pop_front();
        }
        answers.push_back(Replay {
            namespace: namespace.to_owned(),
            collection: collection.to_owned(),
            key: key.to_owned(),
            fingerprint,
            answer: Answer { response, backlog },
            at: Instant::now(),
        });
    }
}

/// Drops the entries past their window, oldest first.
fn purge_expired(answers: &mut VecDeque<Replay>) {
    while answers
        .front()
        .is_some_and(|entry| entry.at.elapsed() >= WINDOW)
    {
        answers.pop_front();
    }
}

/// The answer to replay.
///
/// It is cloned rather than handed out because the entry stays in the ledger
/// for the rest of the window: a caller cannot be handed a reference into a
/// mutex, and a replay that mutated the entry would make the *second* retry
/// differ from the first, which is the one thing a replay must not do.
fn clone_answer(answer: &Answer) -> Answer {
    Answer {
        response: answer.response.clone(),
        backlog: answer.backlog,
    }
}

/// The map or queue mutex, or a poison error that cannot happen.
///
/// A panic while one of these is held would leave it poisoned, and the state
/// behind it — answers and gates — is plain data with no invariant a panic
/// could have broken: the ledger stays usable and simply stops remembering
/// until the next call. Refusing every write because one thread panicked while
/// counting them would be a worse answer than carrying on.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}
