//! Commits on the embedded store (LV1 plan Task 21): the group committer,
//! pessimistic locks and the transaction runner.
//!
//! **The committer.** One thread per store file applies commits in order.
//! It takes every commit waiting, checks each against the versions table
//! (including the versions of the commits before it in the group), gives
//! each that passes a commit timestamp and writes its versions, all in one
//! redb write transaction: one fsync for the group. A commit conflicts when
//! a key it writes or locks has a version newer than its start timestamp
//! (or than its pessimistic lock), or is locked pessimistically by another
//! transaction; an insert fails when its key is present.
//!
//! **The runner** is `loams_tikv`'s, on the embedded transaction: retries
//! with a jittered backoff, the deadline, the fault points of
//! [`faults`](crate::FaultPlan) with the same effects, and commit tokens at
//! `root ‖ t/<16 bytes>` resolved by a read at a fresh timestamp (a resolver
//! that finds a token absent fences it, so the lost commit can no longer
//! apply).

use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use redb::ReadableTable;
use tokio::sync::{Notify, oneshot};

use super::mvcc::{already_exists, encode, newest, version_key};
use super::oracle::Oracle;
use super::{Core, HIGH_WATER, Handle, ORACLE, VERSIONS};
use crate::{Committed, Fault, FaultPoint, Ts, Txn, TxnError, TxnOptions};

/// What a commit does to one key.
#[derive(Debug, Clone)]
pub(crate) enum Op {
    Put(Vec<u8>),
    /// A put that fails the commit when the key is present.
    Insert(Vec<u8>),
    Del,
    /// The key must be absent; nothing is written.
    CheckNotExist,
    /// Conflict detection only; nothing is written.
    Lock,
}

/// One key of a commit.
#[derive(Debug, Clone)]
pub(crate) struct Mutation {
    /// The key's version prefix.
    pub(crate) prefix: Vec<u8>,
    /// The key relative to the root (for error texts).
    pub(crate) key: Vec<u8>,
    pub(crate) op: Op,
    /// A version newer than this conflicts: the start timestamp, or the
    /// timestamp of the transaction's pessimistic lock on the key.
    pub(crate) check_ts: Ts,
}

/// Why the committer refused a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refused {
    Conflict,
    AlreadyExists(String),
    /// The write transaction failed: the outcome is unknown to the caller.
    Storage(String),
}

impl From<Refused> for TxnError {
    fn from(r: Refused) -> Self {
        match r {
            Refused::Conflict => TxnError::Conflict,
            Refused::AlreadyExists(m) => TxnError::AlreadyExists(m),
            Refused::Storage(m) => {
                tracing::warn!(error = %m, "an embedded commit failed to write");
                TxnError::Undetermined { token: None }
            }
        }
    }
}

/// A commit waiting for the committer.
#[derive(Debug)]
pub(crate) struct Request {
    pub(crate) owner: u64,
    pub(crate) mutations: Vec<Mutation>,
    pub(crate) reply: oneshot::Sender<Result<Ts, Refused>>,
}

/// The most commits one group takes.
const MAX_GROUP: usize = 1_024;

/// The committer thread: applies groups until every sender is gone.
pub(crate) fn committer(core: Arc<Core>, requests: Receiver<Request>) {
    while let Ok(first) = requests.recv() {
        let mut group = vec![first];
        while group.len() < MAX_GROUP {
            match requests.try_recv() {
                Ok(next) => group.push(next),
                Err(_) => break,
            }
        }
        core.commit_group(group);
    }
}

impl Core {
    fn commit_group(&self, group: Vec<Request>) {
        let mut mark = None;
        let outcomes = self.apply_group(&group, &mut mark);
        self.oracle.finish_group(mark);
        match outcomes {
            Ok(outcomes) => {
                for (req, outcome) in group.into_iter().zip(outcomes) {
                    let _ = req.reply.send(outcome);
                }
            }
            Err(e) => {
                let text = e.to_string();
                for req in group {
                    let _ = req.reply.send(Err(Refused::Storage(text.clone())));
                }
            }
        }
    }

    /// Checks and writes a group in one write transaction; `mark` is set to
    /// the oracle mark written with it.
    fn apply_group(
        &self,
        group: &[Request],
        mark: &mut Option<Ts>,
    ) -> Result<Vec<Result<Ts, Refused>>, redb::Error> {
        let write = self.db.begin_write()?;
        let mut outcomes = Vec::with_capacity(group.len());
        let mut last = None;
        {
            let mut table = write.open_table(VERSIONS)?;
            for req in group {
                match self.check(&table, req)? {
                    Err(refused) => {
                        if refused == Refused::Conflict {
                            self.counters.conflicts.inc();
                        }
                        outcomes.push(Err(refused));
                    }
                    Ok(()) => {
                        let ts = self.oracle.allocate_commit();
                        for m in &req.mutations {
                            let stored = match &m.op {
                                Op::Put(v) | Op::Insert(v) => encode(Some(v)),
                                Op::Del => encode(None),
                                Op::CheckNotExist | Op::Lock => continue,
                            };
                            table
                                .insert(version_key(&m.prefix, ts).as_slice(), stored.as_slice())?;
                        }
                        last = Some(ts);
                        outcomes.push(Ok(ts));
                    }
                }
            }
        }
        let Some(last) = last else {
            write.abort()?;
            return Ok(outcomes);
        };
        if last >= self.oracle.durable() {
            let mut oracle = write.open_table(ORACLE)?;
            let stored = oracle.get(HIGH_WATER)?.map_or(0, |g| g.value());
            let next = stored.max(Oracle::mark_for(last).0);
            oracle.insert(HIGH_WATER, next)?;
            *mark = Some(Ts(next));
        }
        write.commit()?;
        self.counters.write_transactions.inc();
        self.counters
            .commits
            .add(outcomes.iter().filter(|o| o.is_ok()).count());
        Ok(outcomes)
    }

    fn check<T>(&self, table: &T, req: &Request) -> Result<Result<(), Refused>, redb::Error>
    where
        T: ReadableTable<&'static [u8], &'static [u8]>,
    {
        let mut present = Vec::with_capacity(req.mutations.len());
        for m in &req.mutations {
            if self.locks.held_by_other(&m.prefix, req.owner) {
                return Ok(Err(Refused::Conflict));
            }
            let latest = newest(table, &m.prefix, Ts(u64::MAX))?;
            if latest.as_ref().is_some_and(|(ts, _)| *ts > m.check_ts) {
                return Ok(Err(Refused::Conflict));
            }
            present.push(latest.is_some_and(|(_, v)| v.is_some()));
        }
        for (m, present) in req.mutations.iter().zip(present) {
            if present && matches!(m.op, Op::Insert(_) | Op::CheckNotExist) {
                return Ok(Err(Refused::AlreadyExists(already_exists(&m.key))));
            }
        }
        Ok(Ok(()))
    }
}

/// How long a pessimistic lock request waits for another holder before it
/// gives up with a conflict.
const LOCK_WAIT: Duration = Duration::from_secs(1);

/// The pessimistic locks of a store file: version prefix → owner.
#[derive(Debug, Default)]
pub(crate) struct LockTable {
    held: Mutex<HashMap<Vec<u8>, u64>>,
    released: Notify,
}

impl LockTable {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Vec<u8>, u64>> {
        self.held.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Takes the locks of `prefixes` for `owner`, waiting up to
    /// [`LOCK_WAIT`] for each other holder.
    pub(crate) async fn acquire(&self, prefixes: &[Vec<u8>], owner: u64) -> Result<(), TxnError> {
        for prefix in prefixes {
            let give_up = Instant::now() + LOCK_WAIT;
            loop {
                let released = self.released.notified();
                tokio::pin!(released);
                released.as_mut().enable();
                {
                    let mut held = self.lock();
                    match held.get(prefix) {
                        None => {
                            held.insert(prefix.clone(), owner);
                            break;
                        }
                        Some(&o) if o == owner => break,
                        Some(_) => {}
                    }
                }
                let left = give_up.saturating_duration_since(Instant::now());
                if left.is_zero() || tokio::time::timeout(left, released).await.is_err() {
                    return Err(TxnError::Conflict);
                }
            }
        }
        Ok(())
    }

    /// Whether another transaction than `owner` holds the lock of `prefix`.
    pub(crate) fn held_by_other(&self, prefix: &[u8], owner: u64) -> bool {
        self.lock().get(prefix).is_some_and(|&o| o != owner)
    }

    /// Releases every lock of `owner`.
    pub(crate) fn release(&self, owner: u64) {
        self.lock().retain(|_, o| *o != owner);
        self.released.notify_waiters();
    }
}

// ---- commit tokens (the layout of `loams_tikv::token`) ----

const TOKEN_PREFIX: &[u8] = b"t/";
const TOKEN_TTL: Duration = Duration::from_secs(30 * 60);
type Token = [u8; 16];

pub(crate) fn token_key(token: &Token) -> Vec<u8> {
    [TOKEN_PREFIX, token.as_slice()].concat()
}

/// The prefix of every commit token, under a root.
pub(crate) fn token_prefix() -> &'static [u8] {
    TOKEN_PREFIX
}

fn token_value(now_ms: u64) -> Vec<u8> {
    let ttl = u64::try_from(TOKEN_TTL.as_millis()).unwrap_or(u64::MAX);
    now_ms.saturating_add(ttl).to_be_bytes().to_vec()
}

fn fence_value(now_ms: u64) -> Vec<u8> {
    let mut v = token_value(now_ms);
    v.push(b'F');
    v
}

fn is_fence(value: &[u8]) -> bool {
    value.len() == 9 && value[8] == b'F'
}

/// The `expires_ms` of a token or fence value.
pub(crate) fn token_expiry(value: &[u8]) -> Option<u64> {
    value
        .get(..8)
        .filter(|_| value.len() == 8 || is_fence(value))
        .and_then(|b| b.try_into().ok())
        .map(u64::from_be_bytes)
}

// ---- the runner ----

/// The first backoff pause; doubles per attempt up to [`BACKOFF_MAX`], each
/// pause drawn from its upper half; the pauses of a run take at most half
/// its deadline (`loams_tikv`'s runner, row T11-1).
const BACKOFF_BASE: Duration = Duration::from_millis(10);
const BACKOFF_MAX: Duration = Duration::from_secs(1);
const BACKOFF_SHARE: u32 = 2;
const BACKOFF_FLOOR: Duration = Duration::from_millis(2);

fn backoff_ceiling(attempt: u32) -> Duration {
    let exp = attempt.saturating_sub(2).min(10);
    BACKOFF_BASE.saturating_mul(1 << exp).min(BACKOFF_MAX)
}

fn backoff(attempt: u32, max_attempts: u32, deadline: Duration) -> Duration {
    let ceiling = backoff_ceiling(attempt);
    let total: Duration = (2..=max_attempts.max(2)).map(backoff_ceiling).sum();
    let budget = deadline / BACKOFF_SHARE;
    let ceiling = if total > budget {
        ceiling.mul_f64(budget.as_secs_f64() / total.as_secs_f64())
    } else {
        ceiling
    }
    .max(BACKOFF_FLOOR);
    let us = u64::try_from(ceiling.as_micros()).unwrap_or(u64::MAX);
    Duration::from_micros(rand::random_range(us / 2..=us))
}

fn retryable(e: &TxnError) -> bool {
    matches!(e, TxnError::Conflict | TxnError::NotApplied(_))
}

enum Attempt<T> {
    Committed {
        value: T,
        commit_ts: Ts,
        resolved: bool,
    },
    Retry(TxnError),
    Fail(TxnError),
}

impl<T> Attempt<T> {
    fn from_error(e: TxnError) -> Self {
        if retryable(&e) {
            Attempt::Retry(e)
        } else {
            Attempt::Fail(e)
        }
    }
}

/// The embedded transaction inside a body's [`Txn`].
fn inner(txn: &mut Txn) -> &mut super::Txn {
    match txn {
        Txn::Embedded(t) => t,
        #[cfg(feature = "tikv")]
        Txn::Tikv(_) => unreachable!("the embedded runner wraps embedded transactions"),
    }
}

fn into_inner(txn: Txn) -> super::Txn {
    match txn {
        Txn::Embedded(t) => t,
        #[cfg(feature = "tikv")]
        Txn::Tikv(_) => unreachable!("the embedded runner wraps embedded transactions"),
    }
}

/// [`Store::run`](crate::Store::run) on the embedded store.
pub(crate) async fn run<T: Send, F>(
    handle: &Handle,
    opts: TxnOptions,
    mut body: F,
) -> Result<Committed<T>, TxnError>
where
    F: for<'t> FnMut(&'t mut Txn) -> BoxFuture<'t, Result<T, TxnError>>,
{
    let deadline = Instant::now() + opts.deadline;
    let mut last = TxnError::Deadline;
    for attempt in 1..=opts.max_attempts.max(1) {
        if attempt > 1 {
            handle.shared.core.counters.restarts.inc();
            let pause = backoff(attempt, opts.max_attempts, opts.deadline);
            if Instant::now() + pause >= deadline {
                return Err(TxnError::Deadline);
            }
            tokio::time::sleep(pause).await;
        }
        if Instant::now() >= deadline {
            return Err(TxnError::Deadline);
        }
        match run_attempt(handle, &opts, attempt, deadline, &mut body).await {
            Attempt::Committed {
                value,
                commit_ts,
                resolved,
            } => {
                return Ok(Committed {
                    value,
                    commit_ts,
                    attempts: attempt,
                    earlier_unknown: resolved,
                });
            }
            Attempt::Retry(e) => {
                tracing::debug!(op = opts.op, attempt, error = %e, "restarting a transaction");
                last = e;
            }
            Attempt::Fail(e) => return Err(e),
        }
    }
    Err(last)
}

async fn run_attempt<T: Send, F>(
    handle: &Handle,
    opts: &TxnOptions,
    attempt: u32,
    deadline: Instant,
    body: &mut F,
) -> Attempt<T>
where
    F: for<'t> FnMut(&'t mut Txn) -> BoxFuture<'t, Result<T, TxnError>>,
{
    let fault = |point| handle.fault(opts.op, point, attempt);
    match fault(FaultPoint::BeforeBegin) {
        Some(Fault::Refuse | Fault::LoseAck) => {
            return Attempt::Retry(TxnError::NotApplied("fault: refused before begin".into()));
        }
        Some(Fault::Conflict) => return Attempt::Retry(TxnError::Conflict),
        Some(Fault::Delay(d)) => tokio::time::sleep(d).await,
        None => {}
    }

    let mut wrapped = match handle.begin_attempt(opts.mode, attempt).await {
        Ok(txn) => Txn::Embedded(txn),
        Err(e) => return Attempt::from_error(e),
    };
    let remaining = deadline.saturating_duration_since(Instant::now());
    let value = match tokio::time::timeout(remaining, body(&mut wrapped)).await {
        Ok(Ok(value)) => value,
        Ok(Err(e)) => return Attempt::from_error(e),
        Err(_) => return Attempt::Fail(TxnError::Deadline),
    };

    let mut lost = false;
    if let Some(f) = fault(FaultPoint::BeforePrewrite) {
        match pre_commit_fault(f, "prewrite").await {
            Some(outcome) => return outcome,
            None => lost = f == Fault::LoseAck,
        }
    }
    let token: Option<Token> = opts.commit_token.then(rand::random);
    if !lost && let Some(token) = &token {
        let txn = inner(&mut wrapped);
        let start_ms = txn.start_ts().physical_ms();
        if let Err(e) = txn.put(&token_key(token), token_value(start_ms)).await {
            return Attempt::Fail(e);
        }
    }
    if !lost && let Some(f) = fault(FaultPoint::BeforeCommit) {
        match pre_commit_fault(f, "commit").await {
            Some(outcome) => return outcome,
            None => lost = f == Fault::LoseAck,
        }
    }
    if lost {
        drop(wrapped);
        return unknown_outcome(handle, token, value).await;
    }

    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Attempt::Fail(TxnError::Deadline);
    }
    let txn = into_inner(wrapped);
    let commit_ts = match tokio::time::timeout(remaining, txn.commit()).await {
        // The commit request stays queued and may still apply.
        Err(_) => return unknown_outcome(handle, token, value).await,
        Ok(Ok(ts)) => ts,
        Ok(Err(TxnError::Undetermined { .. })) => {
            return unknown_outcome(handle, token, value).await;
        }
        Ok(Err(e)) => return Attempt::from_error(e),
    };

    match fault(FaultPoint::AfterCommit) {
        Some(Fault::Delay(d)) => tokio::time::sleep(d).await,
        Some(_) => return unknown_outcome(handle, token, value).await,
        None => {}
    }
    Attempt::Committed {
        value,
        commit_ts,
        resolved: false,
    }
}

/// A fault before the commit. `None`: carry on (after a delay, or towards
/// an unknown outcome for `LoseAck`).
async fn pre_commit_fault<T>(fault: Fault, stage: &str) -> Option<Attempt<T>> {
    match fault {
        Fault::Delay(d) => {
            tokio::time::sleep(d).await;
            None
        }
        Fault::Refuse => Some(Attempt::Retry(TxnError::NotApplied(format!(
            "fault: refused before {stage}"
        )))),
        Fault::Conflict => Some(Attempt::Retry(TxnError::Conflict)),
        Fault::LoseAck => None,
    }
}

/// How long the token resolver keeps trying.
const RESOLVE_FOR: Duration = Duration::from_secs(10);

/// Resolves an unknown commit outcome through its token: present, the
/// commit applied; absent, the resolver fences it and the commit certainly
/// did not apply (the runner retries).
async fn unknown_outcome<T>(handle: &Handle, token: Option<Token>, value: T) -> Attempt<T> {
    handle.shared.core.counters.unknown_outcomes.inc();
    let Some(token) = token else {
        return Attempt::Fail(TxnError::Undetermined { token: None });
    };
    let give_up = Instant::now() + RESOLVE_FOR;
    while Instant::now() < give_up {
        match resolve_token(handle, &token).await {
            Some(Resolved::Committed(ts)) => {
                return Attempt::Committed {
                    value,
                    commit_ts: ts,
                    resolved: true,
                };
            }
            Some(Resolved::NotApplied) => {
                return Attempt::Retry(TxnError::NotApplied(
                    "the commit's outcome was unknown and its token is absent".into(),
                ));
            }
            None => tokio::time::sleep(Duration::from_millis(10)).await,
        }
    }
    Attempt::Fail(TxnError::Undetermined { token: Some(token) })
}

enum Resolved {
    Committed(Ts),
    NotApplied,
}

async fn resolve_token(handle: &Handle, token: &Token) -> Option<Resolved> {
    let mut txn = handle
        .begin_attempt(crate::Mode::Optimistic, 1)
        .await
        .ok()?;
    let start = txn.start_ts();
    let key = token_key(token);
    match txn.get(&key).await.ok()? {
        Some(v) => Some(if is_fence(&v) {
            Resolved::NotApplied
        } else {
            Resolved::Committed(start)
        }),
        None => {
            txn.put(&key, fence_value(start.physical_ms())).await.ok()?;
            txn.commit().await.ok().map(|_| Resolved::NotApplied)
        }
    }
}

/// Deletes the expired tokens and fences among `keys` (relative to the
/// handle's root) as one transaction; returns how many went.
pub(crate) fn sweep_tokens_blocking(
    handle: &Handle,
    now_ms: u64,
    keys: Vec<(Vec<u8>, Vec<u8>)>,
) -> u64 {
    let expired: Vec<Vec<u8>> = keys
        .into_iter()
        .filter(|(_, v)| token_expiry(v).is_some_and(|e| e < now_ms))
        .map(|(k, _)| k)
        .collect();
    if expired.is_empty() {
        return 0;
    }
    let core = &handle.shared.core;
    let Ok(start) = core.now_blocking() else {
        return 0;
    };
    core.oracle.wait_visible_blocking(start);
    let mutations: Vec<Mutation> = expired
        .iter()
        .map(|k| Mutation {
            prefix: handle.prefix(k),
            key: k.clone(),
            op: Op::Del,
            check_ts: start,
        })
        .collect();
    let n = mutations.len() as u64;
    let (reply, outcome) = oneshot::channel();
    if !handle.send(Request {
        owner: core.next_owner(),
        mutations,
        reply,
    }) {
        return 0;
    }
    match outcome.blocking_recv() {
        Ok(Ok(_)) => n,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_stays_bounded() {
        let deadline = Duration::from_secs(10);
        for attempt in 2..40 {
            let pause = backoff(attempt, 40, deadline);
            assert!(pause >= Duration::from_millis(1), "{pause:?}");
            assert!(pause <= BACKOFF_MAX, "{pause:?}");
        }
    }

    #[test]
    fn token_values_carry_their_expiry() {
        let v = token_value(1_000);
        assert_eq!(token_expiry(&v), Some(1_000 + 30 * 60 * 1000));
        let f = fence_value(1_000);
        assert!(is_fence(&f) && !is_fence(&v));
        assert_eq!(token_expiry(&f), token_expiry(&v));
        assert_eq!(token_expiry(b"short"), None);
        assert!(token_key(&[7; 16]).starts_with(token_prefix()));
    }
}
