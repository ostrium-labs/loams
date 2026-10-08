//! The embedded timestamp oracle (LV1 plan Task 21, Ruling 3).
//!
//! Timestamps are in TSO layout: the wall clock's milliseconds, never going
//! back, and 18 logical bits. Every timestamp issued is below the persisted
//! high-water mark (`oracle.high_water`): before one would reach it, the
//! mark is advanced to one second past it and persisted, so it moves at most
//! once a second under steady use (a second holds 2^18 allocations per
//! millisecond, far more than 1 000). A restart starts above the mark.
//!
//! The oracle also tracks the commit group in flight: a commit timestamp is
//! allocated before its versions are written, so a read at or above it
//! waits ([`Oracle::wait_visible`]) until the group is applied.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::Notify;

use crate::Ts;

/// How far past an issued timestamp the persisted mark is set (ms).
const RESERVE_MS: u64 = 1_000;

#[derive(Debug)]
struct State {
    /// The last timestamp issued (or observed).
    last: Ts,
    /// The persisted high-water mark: every issued timestamp is below it.
    durable: Ts,
    /// The first commit timestamp of the group being applied.
    inflight: Option<Ts>,
}

/// The oracle of one store file.
#[derive(Debug)]
pub(crate) struct Oracle {
    state: Mutex<State>,
    /// Serializes the persisting of the mark outside commit groups.
    pub(crate) persist: Mutex<()>,
    applied: Notify,
}

/// Milliseconds since the Unix epoch by the wall clock.
pub(crate) fn wall_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

impl Oracle {
    /// An oracle whose persisted mark is `high_water`: it issues timestamps
    /// above it.
    pub(crate) fn new(high_water: Ts) -> Self {
        Oracle {
            state: Mutex::new(State {
                last: high_water,
                durable: high_water,
                inflight: None,
            }),
            persist: Mutex::new(()),
            applied: Notify::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn next(last: Ts) -> Ts {
        Ts::from_parts(wall_ms(), 0).max(Ts(last.0.saturating_add(1)))
    }

    /// The mark to persist before issuing `ts` (the largest timestamp when
    /// `ts` is within a second of it).
    pub(crate) fn mark_for(ts: Ts) -> Ts {
        ts.physical_ms()
            .checked_add(RESERVE_MS)
            .and_then(|ms| Ts::checked_from_parts(ms, 0))
            .unwrap_or(Ts(u64::MAX))
    }

    /// The latest timestamp a read may name: one reservation (1 s) past
    /// the later of the last timestamp issued and the wall clock. A read
    /// above it is refused, so a caller's timestamp cannot move the oracle
    /// (and its persisted mark) far ahead.
    pub(crate) fn read_limit(&self) -> Ts {
        let last = self.lock().last;
        Self::mark_for(last.max(Ts::from_parts(wall_ms(), 0)))
    }

    /// The last timestamp issued (or observed).
    pub(crate) fn last(&self) -> Ts {
        self.lock().last
    }

    /// A fresh timestamp, or `Err(next)` when `next` would reach the
    /// persisted mark, which must first be moved past it.
    pub(crate) fn try_now(&self) -> Result<Ts, Ts> {
        let mut s = self.lock();
        let next = Self::next(s.last);
        if next < s.durable {
            s.last = next;
            Ok(next)
        } else {
            Err(next)
        }
    }

    /// The persisted mark.
    pub(crate) fn durable(&self) -> Ts {
        self.lock().durable
    }

    /// The mark `mark` is persisted.
    pub(crate) fn persisted(&self, mark: Ts) {
        let mut s = self.lock();
        s.durable = s.durable.max(mark);
    }

    /// A read at `at` (at most [`read_limit`](Self::read_limit)) was
    /// taken: later timestamps (commits included) are above it.
    pub(crate) fn observe(&self, at: Ts) {
        let mut s = self.lock();
        s.last = s.last.max(at);
    }

    /// A commit timestamp for the group being applied (by the committer,
    /// which persists the mark in the group's own write transaction).
    pub(crate) fn allocate_commit(&self) -> Ts {
        let mut s = self.lock();
        let next = Self::next(s.last);
        s.last = next;
        s.inflight.get_or_insert(next);
        next
    }

    /// The group is applied (or failed); `mark` was persisted with it.
    pub(crate) fn finish_group(&self, mark: Option<Ts>) {
        {
            let mut s = self.lock();
            s.inflight = None;
            if let Some(mark) = mark {
                s.durable = s.durable.max(mark);
            }
        }
        self.applied.notify_waiters();
    }

    /// Waits until every commit with a timestamp at or below `at` is
    /// applied.
    pub(crate) async fn wait_visible(&self, at: Ts) {
        loop {
            let applied = self.applied.notified();
            tokio::pin!(applied);
            applied.as_mut().enable();
            if self.lock().inflight.is_none_or(|first| first > at) {
                return;
            }
            applied.await;
        }
    }

    /// [`wait_visible`](Self::wait_visible) for a thread outside the
    /// runtime (GC).
    pub(crate) fn wait_visible_blocking(&self, at: Ts) {
        while self.lock().inflight.is_some_and(|first| first <= at) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issues_increasing_timestamps_below_the_mark() {
        let o = Oracle::new(Ts(0));
        let next = o.try_now().expect_err("the mark is 0");
        let mark = Oracle::mark_for(next);
        assert!(mark > next);
        o.persisted(mark);
        let mut last = Ts(0);
        for _ in 0..10_000 {
            let ts = o.try_now().expect("below the mark");
            assert!(ts > last && ts < mark);
            last = ts;
        }
        let c = o.allocate_commit();
        assert!(c > last);
        o.finish_group(None);
    }

    #[test]
    fn marks_never_wrap() {
        assert_eq!(Oracle::mark_for(Ts(u64::MAX)), Ts(u64::MAX));
        let near = Ts::from_parts(Ts::MAX_PHYSICAL_MS - 10, 0);
        assert_eq!(Oracle::mark_for(near), Ts(u64::MAX));
        let o = Oracle::new(Ts(0));
        let limit = o.read_limit();
        let wall = wall_ms();
        assert!(limit.physical_ms() >= wall + RESERVE_MS - 5);
        assert!(limit.physical_ms() <= wall + RESERVE_MS + 5_000);
    }

    #[test]
    fn a_restart_starts_above_the_mark() {
        let ahead = Ts::from_parts(wall_ms() + 60_000, 5);
        let o = Oracle::new(ahead);
        let next = o.try_now().expect_err("at the mark");
        assert!(next > ahead);
    }

    #[test]
    fn observe_moves_the_oracle_past_a_read() {
        let o = Oracle::new(Ts(0));
        o.persisted(Ts::from_parts(wall_ms() + 120_000, 0));
        let far = Ts::from_parts(wall_ms() + 60_000, 3);
        o.observe(far);
        assert!(o.try_now().expect("below the mark") > far);
    }
}
