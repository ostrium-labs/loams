//! GC of the embedded store (LV1 plan Task 21): barriers, open snapshots,
//! the safe point and the GC thread.
//!
//! A GC round computes `safe_point = min(now − gc_life_time, the last
//! timestamp issued, oldest open snapshot or transaction, live barriers)`,
//! never moving it back, persists
//! it (`oracle.gc_safe_point`), and deletes every version older than the
//! newest version at or below it (that version too when it is a
//! tombstone). It also deletes expired commit tokens and fences, as the
//! cluster GC loop does on TiKV. A thread per store file runs a round every
//! `gc_interval`.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use redb::{ReadableDatabase, ReadableTable};

use super::mvcc::{decode, is_tombstone, relative, split};
use super::oracle::wall_ms;
use super::{Core, GC_SAFE_POINT, Handle, ORACLE, Shared, VERSIONS, commit};
use crate::{KvError, Pair, Ts};

/// The GC bookkeeping of a store file.
#[derive(Debug, Default)]
pub(crate) struct GcState {
    /// GC has dropped what only reads below this saw.
    pub(crate) safe_point: Ts,
    /// Service id → (timestamp, expiry).
    barriers: HashMap<String, (Ts, Instant)>,
    /// Open snapshots and transactions by timestamp.
    open: BTreeMap<Ts, usize>,
}

impl GcState {
    pub(crate) fn new(safe_point: Ts) -> Self {
        GcState {
            safe_point,
            ..GcState::default()
        }
    }

    fn live_barriers(&mut self) -> impl Iterator<Item = Ts> + '_ {
        let now = Instant::now();
        self.barriers.retain(|_, (_, expires)| *expires > now);
        self.barriers.values().map(|(ts, _)| *ts)
    }

    /// Whether a live barrier holds GC at or below `at`.
    pub(crate) fn covers(&mut self, at: Ts) -> bool {
        self.live_barriers().any(|ts| ts <= at)
    }

    /// Sets (or moves) a barrier; `Err(safe_point)` when GC is past `at`.
    pub(crate) fn set_barrier(&mut self, id: &str, at: Ts, expires: Instant) -> Result<(), Ts> {
        if self.safe_point > at {
            self.barriers.remove(id);
            return Err(self.safe_point);
        }
        self.barriers.insert(id.to_string(), (at, expires));
        Ok(())
    }

    pub(crate) fn remove_barrier(&mut self, id: &str) {
        self.barriers.remove(id);
    }

    fn open(&mut self, at: Ts) {
        *self.open.entry(at).or_default() += 1;
    }

    fn close(&mut self, at: Ts) {
        if let Some(n) = self.open.get_mut(&at) {
            *n -= 1;
            if *n == 0 {
                self.open.remove(&at);
            }
        }
    }

    /// The next safe point for a GC at `candidate` (`now − gc_life_time`).
    fn advance(&mut self, candidate: Ts) -> Ts {
        let mut sp = candidate;
        if let Some((&oldest, _)) = self.open.first_key_value() {
            sp = sp.min(oldest);
        }
        if let Some(barrier) = self.live_barriers().min() {
            sp = sp.min(barrier);
        }
        self.safe_point = self.safe_point.max(sp);
        self.safe_point
    }
}

/// An open snapshot or transaction: GC keeps what it reads until dropped.
#[derive(Debug)]
pub(crate) struct OpenGuard {
    core: Arc<Core>,
    at: Ts,
}

impl OpenGuard {
    /// Registers a read at `at`, unless GC is past it (`Err(safe_point)`).
    /// `allow` decides, under the GC lock, whether the read may open (the
    /// window check of a snapshot).
    pub(crate) fn open(
        core: &Arc<Core>,
        at: Ts,
        allow: impl FnOnce(&mut GcState) -> Result<(), KvError>,
    ) -> Result<Self, KvError> {
        let mut gc = core.gc_state();
        allow(&mut gc)?;
        if gc.safe_point > at {
            return Err(KvError::GcSafePoint {
                at: at.0,
                safe_point: gc.safe_point.0,
            });
        }
        gc.open(at);
        Ok(OpenGuard {
            core: core.clone(),
            at,
        })
    }
}

impl Drop for OpenGuard {
    fn drop(&mut self) {
        self.core.gc_state().close(self.at);
    }
}

/// What a GC round did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GcReport {
    /// The safe point after the round.
    pub safe_point: Ts,
    /// Versions deleted.
    pub versions_deleted: u64,
    /// Expired commit tokens and fences deleted.
    pub tokens_swept: u64,
}

impl Shared {
    /// One GC round as if the wall clock read `now_ms` (blocking).
    pub(crate) fn gc_blocking(self: &Arc<Self>, now_ms: u64) -> Result<GcReport, KvError> {
        let core = &self.core;
        let tokens_swept = self.sweep_tokens(now_ms)?;
        let life = u64::try_from(self.gc_life_time().as_millis()).unwrap_or(u64::MAX);
        // Never past a timestamp the oracle issued, whatever `now_ms` says
        // (review fix 5): reads at the clock stay possible.
        let candidate = Ts::from_parts(now_ms.saturating_sub(life), 0).min(core.oracle.last());
        let safe_point = core.gc_state().advance(candidate);
        let versions_deleted = collect(core, safe_point).map_err(super::storage)?;
        core.counters.gc_runs.inc();
        core.counters.versions_deleted.add(versions_deleted);
        core.counters.tokens_swept.add(tokens_swept);
        Ok(GcReport {
            safe_point,
            versions_deleted,
            tokens_swept,
        })
    }

    /// Deletes the expired tokens and fences under every root opened on
    /// this file.
    fn sweep_tokens(self: &Arc<Self>, now_ms: u64) -> Result<u64, KvError> {
        let mut swept = 0;
        for (ks, root) in self.roots() {
            let scoped = Handle::scoped(self.clone(), ks, root);
            let keys = {
                let read = self.core.db.begin_read().map_err(super::storage)?;
                let table = read.open_table(VERSIONS).map_err(super::storage)?;
                scoped.latest_with_prefix(&table, commit::token_prefix())?
            };
            swept += commit::sweep_tokens_blocking(&scoped, now_ms, keys);
        }
        Ok(swept)
    }
}

/// Deletes the versions no read at or above `safe_point` sees, and persists
/// the safe point; returns how many versions went.
fn collect(core: &Core, safe_point: Ts) -> Result<u64, redb::Error> {
    let write = core.db.begin_write()?;
    let mut deleted = 0;
    {
        let mut table = write.open_table(VERSIONS)?;
        let mut doomed: Vec<Vec<u8>> = Vec::new();
        {
            let mut current: Option<Vec<u8>> = None;
            // Whether the current key's newest version at or below the safe
            // point was seen (everything older goes).
            let mut kept = false;
            for entry in table.iter()? {
                let (k, v) = entry?;
                let composite = k.value();
                let Some((prefix, ts)) = split(composite) else {
                    continue;
                };
                if current.as_deref() != Some(prefix) {
                    current = Some(prefix.to_vec());
                    kept = false;
                }
                if ts > safe_point {
                    continue;
                }
                if kept {
                    doomed.push(composite.to_vec());
                } else {
                    kept = true;
                    if is_tombstone(v.value()) {
                        doomed.push(composite.to_vec());
                    }
                }
            }
        }
        for k in &doomed {
            table.remove(k.as_slice())?;
            deleted += 1;
        }
        let mut oracle = write.open_table(ORACLE)?;
        oracle.insert(GC_SAFE_POINT, safe_point.0)?;
    }
    write.commit()?;
    core.counters.write_transactions.inc();
    Ok(deleted)
}

impl Handle {
    /// The latest live value of every key under `prefix` (relative to this
    /// handle's root).
    fn latest_with_prefix<T>(&self, table: &T, prefix: &[u8]) -> Result<Vec<Pair>, KvError>
    where
        T: ReadableTable<&'static [u8], &'static [u8]>,
    {
        let mut out = Vec::new();
        let lo = self.prefix(prefix);
        let lo = &lo[..lo.len() - 2];
        let range = table.range(lo..).map_err(super::storage)?;
        let mut current: Option<Vec<u8>> = None;
        for entry in range {
            let (k, v) = entry.map_err(super::storage)?;
            let Some((p, _)) = split(k.value()) else {
                continue;
            };
            if !p.starts_with(lo) {
                break;
            }
            if current.as_deref() == Some(p) {
                continue;
            }
            current = Some(p.to_vec());
            let key = relative(p, self.root().len());
            if let Some(value) = decode(v.value()) {
                out.push((key, value));
            }
        }
        Ok(out)
    }
}

/// Starts the GC thread of a store file: a round every `interval` while the
/// file is open.
pub(crate) fn spawn(shared: Weak<Shared>, interval: Duration) {
    let started = std::thread::Builder::new()
        .name("loams-kv-gc".into())
        .spawn(move || {
            loop {
                let next = Instant::now() + interval;
                while Instant::now() < next {
                    if shared.strong_count() == 0 {
                        return;
                    }
                    std::thread::sleep(
                        next.saturating_duration_since(Instant::now())
                            .min(Duration::from_millis(500)),
                    );
                }
                let Some(open) = shared.upgrade() else {
                    return;
                };
                if let Err(e) = open.gc_blocking(wall_ms()) {
                    tracing::warn!(error = %e, "embedded store GC failed");
                }
            }
        });
    if let Err(e) = started {
        tracing::warn!(error = %e, "the embedded store's GC thread did not start");
    }
}
