//! MVCC on redb (LV1 plan Task 21, Ruling 4; row T21-1): the key layout,
//! reads at a timestamp, and the embedded [`Txn`] and [`Snap`].
//!
//! The table `versions` maps `keyspace_id:u32 BE ‖ esc(root ‖ key) ‖
//! (u64::MAX − commit_ts) BE` to `0x00 ‖ value`, or `0x01` for a tombstone.
//! `esc` escapes each `0x00` as `0x00 0xFF` and ends with `0x00 0x00`: it
//! keeps the order of keys and makes them prefix-free, so the versions of a
//! key are contiguous, newest first, and a range of keys maps to exactly
//! one range of the table.
//!
//! A transaction reads at its start timestamp through a buffer with the
//! semantics of `tikv-client`'s: its own writes overlay what it reads,
//! reads are cached (an `insert` of a key it read as present fails at
//! once), `lock_keys` adds a lock-only mutation, and an insert or a delete
//! of an inserted key leaves a not-exists check for the commit.

use std::collections::{BTreeMap, HashMap};
use std::ops::Bound;
use std::time::Instant;

use redb::{ReadableDatabase, ReadableTable, StorageError};

use super::commit::{Mutation, Op};
use super::gc::OpenGuard;
use super::{Handle, VERSIONS};
use crate::{MAX_VALUE_BYTES, Mode, Pair, Ts, TxnError};

/// The refusal of a read below the GC safe window (the TiKV backend's text).
pub(crate) const BELOW_SAFE_POINT: &str = "read below the GC safe point";

const TOMBSTONE: u8 = 0x01;
const VALUE: u8 = 0x00;

/// Appends `bytes`, each `0x00` escaped as `0x00 0xFF`.
fn escape_into(out: &mut Vec<u8>, bytes: &[u8]) {
    for &b in bytes {
        out.push(b);
        if b == 0 {
            out.push(0xFF);
        }
    }
}

/// The inverse of [`escape_into`].
fn unescape(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len());
    let mut i = 0;
    while i < body.len() {
        out.push(body[i]);
        i += if body[i] == 0 { 2 } else { 1 };
    }
    out
}

/// The version prefix of `key` under `root` in keyspace `ks`:
/// `ks ‖ esc(root ‖ key)`.
pub(crate) fn prefix(ks: u32, root: &[u8], key: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + root.len() + key.len() + 4);
    out.extend_from_slice(&ks.to_be_bytes());
    escape_into(&mut out, root);
    escape_into(&mut out, key);
    out.extend_from_slice(&[0, 0]);
    out
}

/// The table key of the version of `prefix` committed at `ts`.
pub(crate) fn version_key(prefix: &[u8], ts: Ts) -> Vec<u8> {
    let mut out = Vec::with_capacity(prefix.len() + 8);
    out.extend_from_slice(prefix);
    out.extend_from_slice(&(u64::MAX - ts.0).to_be_bytes());
    out
}

/// A table key's version prefix and commit timestamp.
pub(crate) fn split(composite: &[u8]) -> Option<(&[u8], Ts)> {
    if composite.len() < 4 + 2 + 8 {
        return None;
    }
    let (prefix, inv) = composite.split_at(composite.len() - 8);
    let inv = u64::from_be_bytes(inv.try_into().ok()?);
    Some((prefix, Ts(u64::MAX - inv)))
}

/// The key, relative to a root of `root_len` bytes, of a version prefix.
pub(crate) fn relative(prefix: &[u8], root_len: usize) -> Vec<u8> {
    let body = &prefix[4..prefix.len().saturating_sub(2).max(4)];
    let mut full = unescape(body);
    full.drain(..root_len.min(full.len()));
    full
}

/// A stored value: `Some(value)`, or `None` for a tombstone.
pub(crate) fn decode(stored: &[u8]) -> Option<Vec<u8>> {
    match stored.split_first() {
        Some((&VALUE, value)) => Some(value.to_vec()),
        _ => None,
    }
}

/// Whether a stored value is a tombstone.
pub(crate) fn is_tombstone(stored: &[u8]) -> bool {
    stored.first() == Some(&TOMBSTONE)
}

/// The stored form of `value` (`None`: a tombstone).
pub(crate) fn encode(value: Option<&[u8]>) -> Vec<u8> {
    match value {
        Some(v) => {
            let mut out = Vec::with_capacity(v.len() + 1);
            out.push(VALUE);
            out.extend_from_slice(v);
            out
        }
        None => vec![TOMBSTONE],
    }
}

/// A version: its commit timestamp and value (`None` for a tombstone).
pub(crate) type Version = (Ts, Option<Vec<u8>>);

/// The newest version of `prefix` at or below `at`.
pub(crate) fn newest<T>(table: &T, prefix: &[u8], at: Ts) -> Result<Option<Version>, StorageError>
where
    T: ReadableTable<&'static [u8], &'static [u8]>,
{
    let lo = version_key(prefix, at);
    let hi = version_key(prefix, Ts(0));
    let mut range = table.range(lo.as_slice()..=hi.as_slice())?;
    match range.next() {
        Some(entry) => {
            let (k, v) = entry?;
            let ts = split(k.value()).map_or(Ts(0), |(_, ts)| ts);
            Ok(Some((ts, decode(v.value()))))
        }
        None => Ok(None),
    }
}

/// The table range of the keys `start..end` under `root` in keyspace `ks`
/// (`end` `None`: every key under the root).
fn bounds(ks: u32, root: &[u8], start: &[u8], end: Option<&[u8]>) -> (Vec<u8>, Option<Vec<u8>>) {
    let lo = prefix(ks, root, start);
    let hi = match end {
        Some(end) => Some(prefix(ks, root, end)),
        None => {
            let mut under = ks.to_be_bytes().to_vec();
            escape_into(&mut under, root);
            let succ = crate::tuple::successor(&under);
            (!succ.is_empty()).then_some(succ)
        }
    };
    (lo, hi)
}

/// Up to `limit` live pairs at `at` in a table range, in key order or its
/// reverse; keys relative to a root of `root_len` bytes.
fn scan_at<T>(
    table: &T,
    (lo, hi): (Vec<u8>, Option<Vec<u8>>),
    at: Ts,
    limit: usize,
    reverse: bool,
    root_len: usize,
) -> Result<Vec<Pair>, StorageError>
where
    T: ReadableTable<&'static [u8], &'static [u8]>,
{
    let mut out = Vec::new();
    if limit == 0 || hi.as_ref().is_some_and(|hi| lo >= *hi) {
        return Ok(out);
    }
    let upper = match &hi {
        Some(hi) => Bound::Excluded(hi.as_slice()),
        None => Bound::Unbounded,
    };
    let range = table.range::<&[u8]>((Bound::Included(lo.as_slice()), upper))?;
    if reverse {
        // Oldest version first within a key: the last one at or below `at`
        // is the newest visible.
        // The key's prefix, and the newest value at or below `at` so far.
        type Candidate = (Vec<u8>, Option<Option<Vec<u8>>>);
        let mut current: Option<Candidate> = None;
        for entry in range.rev() {
            let (k, v) = entry?;
            let Some((p, ts)) = split(k.value()) else {
                continue;
            };
            if current.as_ref().is_none_or(|(cp, _)| cp.as_slice() != p) {
                if let Some((cp, Some(Some(value)))) = current.take() {
                    out.push((relative(&cp, root_len), value));
                    if out.len() >= limit {
                        return Ok(out);
                    }
                }
                current = Some((p.to_vec(), None));
            }
            if ts <= at
                && let Some((_, seen)) = current.as_mut()
            {
                *seen = Some(decode(v.value()));
            }
        }
        if let Some((cp, Some(Some(value)))) = current {
            out.push((relative(&cp, root_len), value));
        }
    } else {
        // Newest version first within a key: the first one at or below `at`.
        let mut current: Option<Vec<u8>> = None;
        let mut decided = false;
        for entry in range {
            let (k, v) = entry?;
            let Some((p, ts)) = split(k.value()) else {
                continue;
            };
            if current.as_deref() != Some(p) {
                current = Some(p.to_vec());
                decided = false;
            }
            if decided || ts > at {
                continue;
            }
            decided = true;
            if let Some(value) = decode(v.value()) {
                out.push((relative(p, root_len), value));
                if out.len() >= limit {
                    break;
                }
            }
        }
    }
    Ok(out)
}

fn storage(e: impl std::fmt::Display) -> TxnError {
    TxnError::Fatal(format!("embedded store: {e}"))
}

/// Reads of a store at one timestamp, through a fresh redb read
/// transaction each.
#[derive(Debug, Clone)]
struct Reader {
    handle: Handle,
    at: Ts,
}

impl Reader {
    fn get_many(&self, keys: &[Vec<u8>]) -> Result<Vec<Option<Vec<u8>>>, TxnError> {
        let core = &self.handle.shared.core;
        let read = core.db.begin_read().map_err(storage)?;
        let table = read.open_table(VERSIONS).map_err(storage)?;
        keys.iter()
            .map(|k| {
                let p = self.handle.prefix(k);
                newest(&table, &p, self.at)
                    .map(|v| v.and_then(|(_, value)| value))
                    .map_err(storage)
            })
            .collect()
    }

    fn scan(
        &self,
        start: &[u8],
        end: Option<&[u8]>,
        limit: usize,
        reverse: bool,
    ) -> Result<Vec<Pair>, TxnError> {
        let h = &self.handle;
        let read = h.shared.core.db.begin_read().map_err(storage)?;
        let table = read.open_table(VERSIONS).map_err(storage)?;
        let range = bounds(h.ks_id, &h.root, start, end);
        scan_at(&table, range, self.at, limit, reverse, h.root.len()).map_err(storage)
    }
}

/// A key's entry in a transaction's buffer (`tikv-client`'s `BufferEntry`).
#[derive(Debug, Clone)]
enum Entry {
    /// Read from the store (`None`: absent).
    Cached(Option<Vec<u8>>),
    /// Locked by `lock_keys`, with the value read before, if any.
    Locked(Option<Option<Vec<u8>>>),
    Put(Vec<u8>),
    Del,
    Insert(Vec<u8>),
    /// The key must be absent at commit (an inserted key deleted again).
    CheckNotExist,
}

impl Entry {
    /// The value the transaction sees, when the buffer decides it.
    fn value(&self) -> Option<Option<Vec<u8>>> {
        match self {
            Entry::Cached(v) | Entry::Locked(Some(v)) => Some(v.clone()),
            Entry::Put(v) | Entry::Insert(v) => Some(Some(v.clone())),
            Entry::Del | Entry::CheckNotExist => Some(None),
            Entry::Locked(None) => None,
        }
    }
}

/// An embedded transaction: reads at its start timestamp, writes buffered
/// until [`commit`](Self::commit). Dropping it rolls it back. Every key is
/// relative to the store's root.
#[derive(Debug)]
pub struct Txn {
    reader: Reader,
    attempt: u32,
    mode: Mode,
    buffer: BTreeMap<Vec<u8>, Entry>,
    /// Pessimistic locks held: key → the timestamp they were taken at.
    for_update: HashMap<Vec<u8>, Ts>,
    owner: u64,
    refuse_reads_after: Instant,
    _open: OpenGuard,
}

impl Drop for Txn {
    fn drop(&mut self) {
        if !self.for_update.is_empty() {
            self.reader.handle.shared.core.locks.release(self.owner);
        }
    }
}

impl Txn {
    pub(crate) fn new(
        handle: Handle,
        start: Ts,
        attempt: u32,
        mode: Mode,
        refuse_reads_after: Instant,
        open: OpenGuard,
    ) -> Self {
        let owner = handle.shared.core.next_owner();
        Txn {
            reader: Reader { handle, at: start },
            attempt,
            mode,
            buffer: BTreeMap::new(),
            for_update: HashMap::new(),
            owner,
            refuse_reads_after,
            _open: open,
        }
    }

    /// The start timestamp: every read sees the data committed before it.
    pub fn start_ts(&self) -> Ts {
        self.reader.at
    }

    /// Which attempt of [`Store::run`](crate::Store::run) this is, from 1.
    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    fn check_window(&self) -> Result<(), TxnError> {
        if Instant::now() > self.refuse_reads_after {
            return Err(TxnError::Fatal(BELOW_SAFE_POINT.to_string()));
        }
        Ok(())
    }

    fn cache(&mut self, key: Vec<u8>, value: Option<Vec<u8>>) {
        match self.buffer.get_mut(&key) {
            None => {
                self.buffer.insert(key, Entry::Cached(value));
            }
            Some(e @ Entry::Locked(None)) => *e = Entry::Locked(Some(value)),
            Some(_) => {}
        }
    }

    /// Reads `key` at the start timestamp.
    pub async fn get(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, TxnError> {
        self.check_window()?;
        if let Some(v) = self.buffer.get(key).and_then(Entry::value) {
            return Ok(v);
        }
        let value = self
            .reader
            .get_many(&[key.to_vec()])?
            .pop()
            .unwrap_or_default();
        self.cache(key.to_vec(), value.clone());
        Ok(value)
    }

    /// Reads `keys` at the start timestamp; absent keys are left out. The
    /// result is sorted by key.
    pub async fn batch_get<K: AsRef<[u8]>>(
        &mut self,
        keys: impl IntoIterator<Item = K>,
    ) -> Result<Vec<Pair>, TxnError> {
        self.check_window()?;
        let mut found = BTreeMap::new();
        let mut fetch = Vec::new();
        for key in keys {
            let key = key.as_ref();
            match self.buffer.get(key).and_then(Entry::value) {
                Some(v) => {
                    found.insert(key.to_vec(), v);
                }
                None => fetch.push(key.to_vec()),
            }
        }
        let values = self.reader.get_many(&fetch)?;
        for (key, value) in fetch.into_iter().zip(values) {
            self.cache(key.clone(), value.clone());
            found.insert(key, value);
        }
        Ok(found
            .into_iter()
            .filter_map(|(k, v)| v.map(|v| (k, v)))
            .collect())
    }

    async fn scan_inner(
        &mut self,
        start: &[u8],
        end: Option<&[u8]>,
        limit: usize,
        reverse: bool,
    ) -> Result<Vec<Pair>, TxnError> {
        self.check_window()?;
        let upper = end.map_or(Bound::Unbounded, Bound::Excluded);
        let in_range = || self.buffer.range::<[u8], _>((Bound::Included(start), upper));
        // Fetch enough to cover the keys the buffer hides.
        let hidden = in_range()
            .filter(|(_, e)| matches!(e, Entry::Del | Entry::CheckNotExist))
            .count();
        let fetched = self
            .reader
            .scan(start, end, limit.saturating_add(hidden), reverse)?;
        let mut merged: BTreeMap<Vec<u8>, Vec<u8>> = fetched.into_iter().collect();
        for (k, e) in in_range() {
            match e {
                Entry::Put(v) | Entry::Insert(v) => {
                    merged.insert(k.clone(), v.clone());
                }
                Entry::Del | Entry::CheckNotExist => {
                    merged.remove(k);
                }
                Entry::Cached(_) | Entry::Locked(_) => {}
            }
        }
        for (k, v) in &merged {
            self.cache(k.clone(), Some(v.clone()));
        }
        let pairs: Vec<Pair> = if reverse {
            merged.into_iter().rev().take(limit).collect()
        } else {
            merged.into_iter().take(limit).collect()
        };
        Ok(pairs)
    }

    /// Up to `limit` pairs with keys in `start..end` in key order (`end`
    /// `None`: to the end of the root).
    pub async fn scan(
        &mut self,
        start: &[u8],
        end: Option<&[u8]>,
        limit: usize,
    ) -> Result<Vec<Pair>, TxnError> {
        self.scan_inner(start, end, limit, false).await
    }

    /// Like [`scan`](Self::scan), in descending key order.
    pub async fn scan_reverse(
        &mut self,
        start: &[u8],
        end: Option<&[u8]>,
        limit: usize,
    ) -> Result<Vec<Pair>, TxnError> {
        self.scan_inner(start, end, limit, true).await
    }

    /// Takes the pessimistic locks of `keys` (in a pessimistic transaction),
    /// waiting for another holder; with `not_exists`, then fails with
    /// `AlreadyExists` when a key is present.
    async fn lock_now(&mut self, keys: &[Vec<u8>], not_exists: bool) -> Result<(), TxnError> {
        if self.mode != Mode::Pessimistic {
            return Ok(());
        }
        let handle = self.reader.handle.clone();
        let core = &handle.shared.core;
        let wanted: Vec<Vec<u8>> = keys
            .iter()
            .filter(|k| !self.for_update.contains_key(*k))
            .cloned()
            .collect();
        if wanted.is_empty() && !not_exists {
            return Ok(());
        }
        let prefixes: Vec<Vec<u8>> = wanted.iter().map(|k| handle.prefix(k)).collect();
        core.locks.acquire(&prefixes, self.owner).await?;
        let at = handle.now().await.map_err(|e| TxnError::NotApplied(e.to_string()))?;
        core.oracle.wait_visible(at).await;
        for k in wanted {
            self.for_update.insert(k, at);
        }
        if not_exists {
            let latest = Reader {
                handle: handle.clone(),
                at,
            }
            .get_many(keys)?;
            if let Some((k, _)) = keys.iter().zip(latest).find(|(_, v)| v.is_some()) {
                return Err(TxnError::AlreadyExists(already_exists(k)));
            }
        }
        Ok(())
    }

    /// Writes `value` at `key`. Refuses a value over 2 MiB
    /// ([`MAX_VALUE_BYTES`]) with `TxnError::Fatal("value over 2 MiB")`.
    pub async fn put(&mut self, key: &[u8], value: impl Into<Vec<u8>>) -> Result<(), TxnError> {
        let value = value.into();
        if value.len() > MAX_VALUE_BYTES {
            return Err(TxnError::Fatal("value over 2 MiB".to_string()));
        }
        self.lock_now(&[key.to_vec()], false).await?;
        let entry = match self.buffer.get(key) {
            Some(Entry::Insert(_) | Entry::CheckNotExist) => Entry::Insert(value),
            _ => Entry::Put(value),
        };
        self.buffer.insert(key.to_vec(), entry);
        Ok(())
    }

    /// Writes `value` at `key` if the key is absent; the transaction fails
    /// with [`TxnError::AlreadyExists`] if it is not (at once when it read
    /// the key as present, else at commit). The 2 MiB bound applies.
    pub async fn insert(&mut self, key: &[u8], value: impl Into<Vec<u8>>) -> Result<(), TxnError> {
        let value = value.into();
        if value.len() > MAX_VALUE_BYTES {
            return Err(TxnError::Fatal("value over 2 MiB".to_string()));
        }
        if self
            .buffer
            .get(key)
            .and_then(Entry::value)
            .is_some_and(|v| v.is_some())
        {
            return Err(TxnError::AlreadyExists(already_exists(key)));
        }
        self.lock_now(&[key.to_vec()], true).await?;
        let entry = match self.buffer.get(key) {
            Some(Entry::Del) => Entry::Put(value),
            _ => Entry::Insert(value),
        };
        self.buffer.insert(key.to_vec(), entry);
        Ok(())
    }

    /// Deletes `key`.
    pub async fn delete(&mut self, key: &[u8]) -> Result<(), TxnError> {
        self.lock_now(&[key.to_vec()], false).await?;
        let entry = match self.buffer.get(key) {
            Some(Entry::Insert(_) | Entry::CheckNotExist) if self.mode == Mode::Optimistic => {
                Entry::CheckNotExist
            }
            _ => Entry::Del,
        };
        self.buffer.insert(key.to_vec(), entry);
        Ok(())
    }

    /// Locks `keys` without writing them: a concurrent write to one of them
    /// is a write-write conflict. In a pessimistic transaction the locks are
    /// taken at once, so a second holder waits.
    pub async fn lock_keys<K: AsRef<[u8]>>(
        &mut self,
        keys: impl IntoIterator<Item = K>,
    ) -> Result<(), TxnError> {
        let keys: Vec<Vec<u8>> = keys.into_iter().map(|k| k.as_ref().to_vec()).collect();
        self.lock_now(&keys, false).await?;
        for key in keys {
            match self.buffer.get_mut(&key) {
                None => {
                    self.buffer.insert(key, Entry::Locked(None));
                }
                Some(e @ Entry::Cached(_)) => {
                    let Entry::Cached(v) = std::mem::replace(e, Entry::Locked(None)) else {
                        unreachable!("matched as cached")
                    };
                    *e = Entry::Locked(Some(v));
                }
                Some(_) => {}
            }
        }
        Ok(())
    }

    /// The mutations the commit applies and checks.
    pub(crate) fn mutations(&self) -> Vec<Mutation> {
        let h = &self.reader.handle;
        self.buffer
            .iter()
            .filter_map(|(key, entry)| {
                let op = match entry {
                    Entry::Cached(_) => return None,
                    Entry::Locked(_) => Op::Lock,
                    Entry::Put(v) => Op::Put(v.clone()),
                    Entry::Insert(v) => Op::Insert(v.clone()),
                    Entry::Del => Op::Del,
                    Entry::CheckNotExist => Op::CheckNotExist,
                };
                Some(Mutation {
                    prefix: h.prefix(key),
                    key: key.clone(),
                    op,
                    check_ts: self.for_update.get(key).copied().unwrap_or(self.reader.at),
                })
            })
            .collect()
    }

    /// Commits the transaction: its timestamp, or the start timestamp when
    /// it wrote and locked nothing. Fails with `Conflict` when a key it
    /// wrote or locked has a newer version than its start (or than its
    /// pessimistic lock), with `AlreadyExists` when an inserted key is
    /// present, and with `Undetermined` when the store failed to write.
    /// This is the backend's raw commit: [`Store::run`](crate::Store::run)
    /// adds the retries, faults and commit tokens.
    pub async fn commit(self) -> Result<Ts, TxnError> {
        let mutations = self.mutations();
        if mutations.is_empty() {
            return Ok(self.reader.at);
        }
        let handle = self.reader.handle.clone();
        let ts = handle.commit(self.owner, mutations).await;
        drop(self);
        ts
    }

    /// Rolls the transaction back (as dropping it does).
    pub fn rollback(self) {}
}

/// The `AlreadyExists` text of `key` (relative), as the TiKV backend words
/// it: at most 64 bytes of the key, escaped.
pub(crate) fn already_exists(key: &[u8]) -> String {
    const SHOWN: usize = 64;
    let shown = &key[..key.len().min(SHOWN)];
    let mut out = format!("key already exists: \"{}\"", shown.escape_ascii());
    if key.len() > SHOWN {
        out.push_str(&format!("… ({} bytes)", key.len()));
    }
    out
}

/// A read-only view at one timestamp, from
/// [`Handle::snapshot`](super::Handle::snapshot). Every key is relative to
/// the store's root. Its reads are refused once its timestamp leaves the GC
/// safe window, unless a [`GcBarrier`](crate::GcBarrier) of the store covers
/// it.
#[derive(Debug)]
pub struct Snap {
    reader: Reader,
    refuse_reads_after: Instant,
    _open: OpenGuard,
}

impl Snap {
    pub(crate) fn new(handle: Handle, at: Ts, refuse_reads_after: Instant, open: OpenGuard) -> Self {
        Snap {
            reader: Reader { handle, at },
            refuse_reads_after,
            _open: open,
        }
    }

    /// The snapshot's timestamp.
    pub fn ts(&self) -> Ts {
        self.reader.at
    }

    fn check_window(&self) -> Result<(), TxnError> {
        if Instant::now() > self.refuse_reads_after
            && !self.reader.handle.shared.core.covered(self.reader.at)
        {
            return Err(TxnError::Fatal(BELOW_SAFE_POINT.to_string()));
        }
        Ok(())
    }

    /// Reads `key`.
    pub async fn get(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, TxnError> {
        self.check_window()?;
        Ok(self
            .reader
            .get_many(&[key.to_vec()])?
            .pop()
            .unwrap_or_default())
    }

    /// Reads `keys`; absent keys are left out. Sorted by key.
    pub async fn batch_get<K: AsRef<[u8]>>(
        &mut self,
        keys: impl IntoIterator<Item = K>,
    ) -> Result<Vec<Pair>, TxnError> {
        self.check_window()?;
        let keys: Vec<Vec<u8>> = keys.into_iter().map(|k| k.as_ref().to_vec()).collect();
        let values = self.reader.get_many(&keys)?;
        let found: BTreeMap<Vec<u8>, Vec<u8>> = keys
            .into_iter()
            .zip(values)
            .filter_map(|(k, v)| v.map(|v| (k, v)))
            .collect();
        Ok(found.into_iter().collect())
    }

    /// Up to `limit` pairs with keys in `start..end` in key order (`end`
    /// `None`: to the end of the root).
    pub async fn scan(
        &mut self,
        start: &[u8],
        end: Option<&[u8]>,
        limit: usize,
    ) -> Result<Vec<Pair>, TxnError> {
        self.check_window()?;
        self.reader.scan(start, end, limit, false)
    }

    /// Like [`scan`](Self::scan), in descending key order.
    pub async fn scan_reverse(
        &mut self,
        start: &[u8],
        end: Option<&[u8]>,
        limit: usize,
    ) -> Result<Vec<Pair>, TxnError> {
        self.check_window()?;
        self.reader.scan(start, end, limit, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaping_keeps_order_and_is_prefix_free() {
        let keys: [&[u8]; 8] = [
            b"", b"\x00", b"\x00\x00", b"\x00\x01", b"\x00\xff", b"a", b"a\x00", b"a\x01",
        ];
        let prefixes: Vec<Vec<u8>> = keys.iter().map(|k| prefix(7, b"r\x00", k)).collect();
        for (i, a) in prefixes.iter().enumerate() {
            for (j, b) in prefixes.iter().enumerate() {
                assert_eq!(a.cmp(b), keys[i].cmp(keys[j]), "{:?} vs {:?}", keys[i], keys[j]);
                if i != j {
                    assert!(!b.starts_with(a), "{:?} is a prefix of {:?}", keys[i], keys[j]);
                }
            }
            assert_eq!(relative(a, 2), keys[i]);
        }
    }

    #[test]
    fn versions_sort_newest_first_and_split_back() {
        let p = prefix(1, b"", b"k");
        let old = version_key(&p, Ts(5));
        let new = version_key(&p, Ts(9));
        assert!(new < old);
        assert_eq!(split(&old), Some((p.as_slice(), Ts(5))));
        assert_eq!(decode(&encode(Some(b"v"))), Some(b"v".to_vec()));
        assert_eq!(decode(&encode(None)), None);
        assert!(is_tombstone(&encode(None)));
        assert!(!is_tombstone(&encode(Some(b""))));
    }

    #[test]
    fn already_exists_shows_at_most_64_bytes() {
        assert_eq!(already_exists(b"k\x00"), "key already exists: \"k\\x00\"");
        let long = already_exists(&[b'x'; 70]);
        assert!(long.ends_with("… (70 bytes)"), "{long}");
    }
}
