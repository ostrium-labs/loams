//! The embedded store's pessimistic locks (D851; KV1 plan KV0 Task 5).
//!
//! One table per store file maps a key's version prefix (keyspace, root and
//! key) to the pessimistic transaction holding it. A pessimistic
//! transaction takes a key's lock at `get_for_update`, `put`, `insert`,
//! `delete` and `lock_keys`, and holds it until it commits or rolls back.
//! A second taker waits, up to its run's lock-wait limit
//! ([`lock_wait_until`]); a wait that would close a cycle of waiters (a
//! deadlock) and a wait that passes the limit both fail with
//! `Conflict`, as TiKV's `Deadlock` and lock-wait timeout do. The committer
//! refuses (with `Conflict`) any commit that writes or locks a key another
//! transaction holds, so an optimistic writer cannot slip under a lock.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tokio::sync::Notify;

use crate::TxnError;

/// The id of a pessimistic transaction (unique per store file).
pub(crate) type Owner = u64;

#[derive(Debug, Default)]
struct State {
    /// Key prefix → its holder.
    holders: HashMap<Vec<u8>, Owner>,
    /// Waiter → the holder it waits for.
    waits_for: HashMap<Owner, Owner>,
    next: Owner,
}

/// The lock table of a store file.
#[derive(Debug, Default)]
pub(crate) struct LockTable {
    state: Mutex<State>,
    released: Notify,
}

/// The share of a run's deadline that lock waits may use: the rest is left
/// for the body to see the `Conflict` and the runner to return it before
/// its own deadline (at most a second).
pub(crate) fn lock_wait_until(start: Instant, deadline: Duration) -> Instant {
    start + deadline.saturating_sub((deadline / 10).min(Duration::from_secs(1)))
}

impl LockTable {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A fresh owner id.
    pub(crate) fn owner(&self) -> Owner {
        let mut state = self.state();
        state.next += 1;
        state.next
    }

    /// The holder of `prefix`, if any.
    pub(crate) fn holder(&self, prefix: &[u8]) -> Option<Owner> {
        self.state().holders.get(prefix).copied()
    }

    /// Takes the lock of `prefix` for `owner`, waiting until `until` while
    /// another transaction holds it. `Ok(true)`: newly taken; `Ok(false)`:
    /// `owner` held it already. `Err(Conflict)` on a deadlock or when
    /// `until` passes.
    pub(crate) async fn acquire(
        &self,
        owner: Owner,
        prefix: &[u8],
        until: Instant,
    ) -> Result<bool, TxnError> {
        loop {
            let released = self.released.notified();
            tokio::pin!(released);
            // Registered before the check, so a release between the check
            // and the wait is not missed.
            released.as_mut().enable();
            {
                let mut state = self.state();
                match state.holders.get(prefix).copied() {
                    None => {
                        state.holders.insert(prefix.to_vec(), owner);
                        state.waits_for.remove(&owner);
                        return Ok(true);
                    }
                    Some(holder) if holder == owner => {
                        state.waits_for.remove(&owner);
                        return Ok(false);
                    }
                    Some(holder) => {
                        if closes_cycle(&state.waits_for, owner, holder) {
                            state.waits_for.remove(&owner);
                            return Err(TxnError::Conflict);
                        }
                        state.waits_for.insert(owner, holder);
                    }
                }
            }
            if tokio::time::timeout_at(until.into(), released)
                .await
                .is_err()
            {
                self.state().waits_for.remove(&owner);
                return Err(TxnError::Conflict);
            }
        }
    }

    /// Releases every lock `owner` holds among `prefixes`, and wakes the
    /// waiters.
    pub(crate) fn release<'a>(&self, owner: Owner, prefixes: impl IntoIterator<Item = &'a [u8]>) {
        let mut state = self.state();
        let mut any = false;
        for prefix in prefixes {
            if state.holders.get(prefix) == Some(&owner) {
                state.holders.remove(prefix);
                any = true;
            }
        }
        state.waits_for.remove(&owner);
        drop(state);
        if any {
            self.released.notify_waiters();
        }
    }
}

/// Whether `waiter` waiting for `holder` closes a cycle: `holder` waits,
/// directly or through others, for `waiter`.
fn closes_cycle(waits_for: &HashMap<Owner, Owner>, waiter: Owner, holder: Owner) -> bool {
    let mut at = holder;
    for _ in 0..=waits_for.len() {
        if at == waiter {
            return true;
        }
        match waits_for.get(&at) {
            Some(&next) => at = next,
            None => return false,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn soon(ms: u64) -> Instant {
        Instant::now() + Duration::from_millis(ms)
    }

    #[tokio::test]
    async fn a_second_taker_waits_for_the_release() {
        let table = Arc::new(LockTable::default());
        let (a, b) = (table.owner(), table.owner());
        assert!(table.acquire(a, b"k", soon(10)).await.unwrap());
        assert!(
            !table.acquire(a, b"k", soon(10)).await.unwrap(),
            "re-entrant"
        );
        let waiter = tokio::spawn({
            let table = table.clone();
            async move { table.acquire(b, b"k", soon(5_000)).await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiter.is_finished(), "b waits while a holds");
        table.release(a, [b"k".as_slice()]);
        assert!(waiter.await.unwrap().unwrap());
        assert_eq!(table.holder(b"k"), Some(b));
    }

    #[tokio::test]
    async fn a_wait_past_its_limit_is_a_conflict() {
        let table = LockTable::default();
        let (a, b) = (table.owner(), table.owner());
        table.acquire(a, b"k", soon(10)).await.unwrap();
        let started = Instant::now();
        assert_eq!(
            table.acquire(b, b"k", soon(100)).await,
            Err(TxnError::Conflict)
        );
        assert!(started.elapsed() >= Duration::from_millis(90));
        assert_eq!(table.holder(b"k"), Some(a));
    }

    #[tokio::test]
    async fn a_deadlock_is_a_conflict_at_once() {
        let table = Arc::new(LockTable::default());
        let (a, b) = (table.owner(), table.owner());
        table.acquire(a, b"x", soon(10)).await.unwrap();
        table.acquire(b, b"y", soon(10)).await.unwrap();
        let a_waits = tokio::spawn({
            let table = table.clone();
            async move { table.acquire(a, b"y", soon(5_000)).await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        let started = Instant::now();
        assert_eq!(
            table.acquire(b, b"x", soon(5_000)).await,
            Err(TxnError::Conflict)
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "detected, not timed out"
        );
        table.release(b, [b"y".as_slice()]);
        assert!(a_waits.await.unwrap().unwrap());
    }

    #[test]
    fn lock_waits_leave_part_of_the_deadline() {
        let start = Instant::now();
        assert_eq!(
            lock_wait_until(start, Duration::from_secs(10)),
            start + Duration::from_secs(9)
        );
        assert_eq!(
            lock_wait_until(start, Duration::from_secs(60)),
            start + Duration::from_secs(59)
        );
        assert_eq!(
            lock_wait_until(start, Duration::from_millis(500)),
            start + Duration::from_millis(450)
        );
    }
}
