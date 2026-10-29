//! Token budgets: per store per UTC day, per run and per call, with
//! reserve-then-settle accounting (§52 §10.3).
//!
//! A call reserves its estimate (input estimate plus `max_output_tokens`)
//! before anything is sent. If any cap would be passed, the call is refused
//! with [`LlmError::BudgetExhausted`] and nothing reaches the provider. When
//! the provider answers, the reservation is settled with the tokens it
//! reports; a call that fails releases its reservation.
//!
//! The ledgers are in memory, per process (ruling R2.5). A cluster-wide
//! daily budget backed by the metastore is Task 52's.

use std::fmt;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::error::LlmError;

const DAY_SECS: i64 = 86_400;

/// Wall-clock time for the daily reset.
pub trait Clock: Send + Sync {
    /// Seconds since the Unix epoch (UTC).
    fn now_unix_secs(&self) -> i64;
}

/// The system clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix_secs(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
            .unwrap_or(0)
    }
}

/// A clock tests set by hand.
#[derive(Debug, Default)]
pub struct ManualClock(AtomicI64);

impl ManualClock {
    /// A clock at `unix_secs`.
    #[must_use]
    pub fn new(unix_secs: i64) -> Self {
        Self(AtomicI64::new(unix_secs))
    }
    /// Sets the time.
    pub fn set(&self, unix_secs: i64) {
        self.0.store(unix_secs, Ordering::SeqCst);
    }
    /// Moves the time forward by `secs`.
    pub fn advance(&self, secs: i64) {
        self.0.fetch_add(secs, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_unix_secs(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Debug, Default, Clone, Copy)]
struct Tally {
    used: u64,
    reserved: u64,
}

impl Tally {
    fn fits(&self, limit: u64, tokens: u64) -> bool {
        self.used
            .saturating_add(self.reserved)
            .saturating_add(tokens)
            <= limit
    }
}

#[derive(Debug, Default)]
struct DayTally {
    day: i64,
    tally: Tally,
}

/// One store's daily budget (extraction plus consolidation). It resets at
/// 00:00 UTC.
pub struct DailyBudget {
    limit: u64,
    clock: Arc<dyn Clock>,
    state: Mutex<DayTally>,
}

impl fmt::Debug for DailyBudget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DailyBudget")
            .field("limit", &self.limit)
            .field("used", &self.used())
            .finish_non_exhaustive()
    }
}

impl DailyBudget {
    /// A daily budget of `limit` tokens on the system clock.
    #[must_use]
    pub fn new(limit: u64) -> Self {
        Self::with_clock(limit, Arc::new(SystemClock))
    }

    /// A daily budget of `limit` tokens on `clock`.
    #[must_use]
    pub fn with_clock(limit: u64, clock: Arc<dyn Clock>) -> Self {
        let day = clock.now_unix_secs().div_euclid(DAY_SECS);
        Self {
            limit,
            clock,
            state: Mutex::new(DayTally {
                day,
                tally: Tally::default(),
            }),
        }
    }

    fn today(&self) -> i64 {
        self.clock.now_unix_secs().div_euclid(DAY_SECS)
    }

    /// The current day's state, rolled over at 00:00 UTC.
    fn current(&self) -> MutexGuard<'_, DayTally> {
        let today = self.today();
        let mut s = lock(&self.state);
        if s.day != today {
            *s = DayTally {
                day: today,
                tally: Tally::default(),
            };
        }
        s
    }

    /// Tokens settled today.
    #[must_use]
    pub fn used(&self) -> u64 {
        self.current().tally.used
    }

    /// The daily cap in tokens.
    #[must_use]
    pub fn limit(&self) -> u64 {
        self.limit
    }
}

/// One run's budget (a dream run, one extraction job).
#[derive(Debug)]
pub struct RunBudget {
    limit: u64,
    state: Mutex<Tally>,
}

impl RunBudget {
    /// A run budget of `limit` tokens.
    #[must_use]
    pub fn new(limit: u64) -> Self {
        Self {
            limit,
            state: Mutex::new(Tally::default()),
        }
    }

    /// Tokens settled by this run.
    #[must_use]
    pub fn used(&self) -> u64 {
        lock(&self.state).used
    }

    /// The run's cap in tokens.
    #[must_use]
    pub fn limit(&self) -> u64 {
        self.limit
    }
}

/// The caps one call is held to. Cheap to clone; clones share ledgers.
#[derive(Debug, Clone, Default)]
pub struct Budget {
    daily: Option<Arc<DailyBudget>>,
    run: Option<Arc<RunBudget>>,
    per_call: Option<u64>,
}

impl Budget {
    /// No caps.
    #[must_use]
    pub fn unlimited() -> Self {
        Self::default()
    }

    /// Adds the store's daily budget.
    #[must_use]
    pub fn with_daily(mut self, daily: Arc<DailyBudget>) -> Self {
        self.daily = Some(daily);
        self
    }

    /// Adds a run budget.
    #[must_use]
    pub fn with_run(mut self, run: Arc<RunBudget>) -> Self {
        self.run = Some(run);
        self
    }

    /// Adds a per-call cap: a call whose estimate is larger is refused.
    #[must_use]
    pub fn with_per_call(mut self, tokens: u64) -> Self {
        self.per_call = Some(tokens);
        self
    }

    /// Reserves `estimate` tokens against every cap, or refuses without
    /// reserving anything.
    pub fn reserve(&self, estimate: u64) -> Result<Reservation, LlmError> {
        if self.per_call.is_some_and(|cap| estimate > cap) {
            return Err(LlmError::BudgetExhausted);
        }
        // Lock order: daily, then run.
        let mut day = self.daily.as_ref().map(|d| (d, d.current()));
        let mut run = self.run.as_ref().map(|r| (r, lock(&r.state)));
        if let Some((d, s)) = &day
            && !s.tally.fits(d.limit, estimate)
        {
            return Err(LlmError::BudgetExhausted);
        }
        if let Some((r, s)) = &run
            && !s.fits(r.limit, estimate)
        {
            return Err(LlmError::BudgetExhausted);
        }
        let mut day_of_reservation = None;
        if let Some((_, s)) = &mut day {
            s.tally.reserved += estimate;
            day_of_reservation = Some(s.day);
        }
        if let Some((_, s)) = &mut run {
            s.reserved += estimate;
        }
        Ok(Reservation {
            budget: self.clone(),
            estimate,
            day: day_of_reservation,
            open: true,
        })
    }
}

/// Tokens held for one call until it settles. Dropping it unsettled
/// releases the hold.
#[derive(Debug)]
pub struct Reservation {
    budget: Budget,
    estimate: u64,
    day: Option<i64>,
    open: bool,
}

impl Reservation {
    /// The tokens held.
    #[must_use]
    pub fn estimate(&self) -> u64 {
        self.estimate
    }

    /// Replaces the hold with the tokens the provider reported.
    pub fn settle(mut self, actual: u64) {
        self.finish(actual);
    }

    fn finish(&mut self, actual: u64) {
        if !self.open {
            return;
        }
        self.open = false;
        if let Some(d) = &self.budget.daily {
            let mut s = d.current();
            if Some(s.day) == self.day {
                s.tally.reserved = s.tally.reserved.saturating_sub(self.estimate);
            }
            // A call that straddles midnight is charged to the day it ends.
            s.tally.used = s.tally.used.saturating_add(actual);
        }
        if let Some(r) = &self.budget.run {
            let mut s = lock(&r.state);
            s.reserved = s.reserved.saturating_sub(self.estimate);
            s.used = s.used.saturating_add(actual);
        }
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.finish(0);
    }
}

/// A rough token estimate for text: a quarter of its bytes, rounded up.
#[must_use]
pub fn estimate_tokens(text: &str) -> u64 {
    (text.len() as u64).div_ceil(4)
}
