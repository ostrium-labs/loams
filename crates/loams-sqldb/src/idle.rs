//! The idle detector (plan SQ1 Task 5; §47 §14): a branch is idle after no
//! command except `COM_PING` for `suspend_after` (default 5 min; zero
//! disables suspend). The gate's `ReportActivity` feeds
//! [`IdleDetector::activity`]; the lifecycle host asks [`IdleDetector::due`]
//! once a second and sends the branch's machine `Idle`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use tokio::time::Instant;

use crate::model::BranchId;

#[derive(Debug, Clone, Copy)]
struct Entry {
    /// The last command, the branch's last start, or the last `due`.
    last: Instant,
    /// This branch's `suspend_after`, if not the default.
    after: Option<Duration>,
}

/// See the module docs.
#[derive(Debug)]
pub struct IdleDetector {
    default_after: Duration,
    branches: Mutex<HashMap<BranchId, Entry>>,
}

impl IdleDetector {
    /// Branches suspend after `default_after` without a command, unless
    /// [`IdleDetector::set_suspend_after`] says otherwise.
    pub fn new(default_after: Duration) -> Self {
        Self {
            default_after,
            branches: Mutex::new(HashMap::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<BranchId, Entry>> {
        self.branches
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A command ran on `branch` (or it started running): its idle time
    /// starts again.
    pub fn activity(&self, branch: &BranchId, now: Instant) {
        self.lock()
            .entry(branch.clone())
            .and_modify(|e| e.last = now)
            .or_insert(Entry {
                last: now,
                after: None,
            });
    }

    /// `branch`'s own `suspend_after`; zero never suspends it.
    pub fn set_suspend_after(&self, branch: &BranchId, after: Duration, now: Instant) {
        self.lock()
            .entry(branch.clone())
            .and_modify(|e| e.after = Some(after))
            .or_insert(Entry {
                last: now,
                after: Some(after),
            });
    }

    /// Branches idle for their `suspend_after` at `now`. Each is reported
    /// again only after another `suspend_after` without activity (a
    /// suspend that was lost to a crash is asked for again).
    pub fn due(&self, now: Instant) -> Vec<BranchId> {
        let mut out = Vec::new();
        for (b, e) in self.lock().iter_mut() {
            let after = e.after.unwrap_or(self.default_after);
            if !after.is_zero() && now.saturating_duration_since(e.last) >= after {
                e.last = now;
                out.push(b.clone());
            }
        }
        out.sort();
        out
    }
}
