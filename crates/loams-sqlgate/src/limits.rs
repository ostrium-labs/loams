//! Per-database connection limits (1040) and activity accounting
//! (`ReportActivity`, idle detection in Task 5).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::time::Instant;

/// Limits per database (here: per branch).
#[derive(Debug, Clone, PartialEq)]
pub struct LimitsConfig {
    /// Open client connections at once (§47 §15's class value, set by the
    /// control plane; a uniform default until then).
    pub max_connections_per_db: u32,
    /// New connections per second, sustained.
    pub connect_rate_per_sec: u32,
    /// New connections allowed in a burst.
    pub connect_burst: u32,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            max_connections_per_db: 100,
            connect_rate_per_sec: 50,
            connect_burst: 100,
        }
    }
}

#[derive(Debug)]
struct Db {
    open: u32,
    tokens: f64,
    refilled: Instant,
}

/// Why a connection was refused (both answer 1040).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LimitError {
    /// Too many new connections per second.
    #[error("connection rate exceeded")]
    Rate,
    /// Too many open connections.
    #[error("too many connections")]
    Cap,
}

/// The per-database counters.
#[derive(Debug)]
pub struct Limiter {
    config: LimitsConfig,
    dbs: Mutex<HashMap<String, Db>>,
}

impl Limiter {
    /// Counters for `config`.
    pub fn new(config: LimitsConfig) -> Arc<Self> {
        Arc::new(Self {
            config,
            dbs: Mutex::new(HashMap::new()),
        })
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Db>> {
        self.dbs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn db<'a>(&self, dbs: &'a mut HashMap<String, Db>, db: &str) -> &'a mut Db {
        let burst = f64::from(self.config.connect_burst);
        dbs.entry(db.to_owned()).or_insert(Db {
            open: 0,
            tokens: burst,
            refilled: Instant::now(),
        })
    }

    /// Takes one token from `db`'s connection rate (token bucket).
    pub fn admit(&self, db: &str) -> Result<(), LimitError> {
        let mut dbs = self.lock();
        let (rate, burst) = (
            f64::from(self.config.connect_rate_per_sec),
            f64::from(self.config.connect_burst),
        );
        let d = self.db(&mut dbs, db);
        let now = Instant::now();
        d.tokens = (d.tokens + now.duration_since(d.refilled).as_secs_f64() * rate).min(burst);
        d.refilled = now;
        if d.tokens < 1.0 {
            return Err(LimitError::Rate);
        }
        d.tokens -= 1.0;
        Ok(())
    }

    /// Takes a connection slot for `db`, released when the slot drops.
    pub fn acquire(self: &Arc<Self>, db: &str) -> Result<Slot, LimitError> {
        let mut dbs = self.lock();
        let max = self.config.max_connections_per_db;
        let d = self.db(&mut dbs, db);
        if d.open >= max {
            return Err(LimitError::Cap);
        }
        d.open += 1;
        Ok(Slot {
            limiter: self.clone(),
            db: db.to_owned(),
        })
    }

    /// Open connections of `db`.
    pub fn open(&self, db: &str) -> u32 {
        self.lock().get(db).map_or(0, |d| d.open)
    }
}

/// An open connection's slot.
#[derive(Debug)]
pub struct Slot {
    limiter: Arc<Limiter>,
    db: String,
}

impl Drop for Slot {
    fn drop(&mut self) {
        if let Some(d) = self.limiter.lock().get_mut(&self.db) {
            d.open = d.open.saturating_sub(1);
        }
    }
}

/// Where command activity goes (`ReportActivity`, batched by Task 5).
pub trait ActivitySink: Send + Sync {
    /// One command (never `COM_PING`) on `branch`.
    fn command(&self, branch: &str);
}

/// Counts commands per branch (tests, and the batch `ReportActivity` sends).
#[derive(Debug, Default)]
pub struct ActivityCounter(Mutex<HashMap<String, u64>>);

impl ActivityCounter {
    /// Commands counted for `branch`.
    pub fn count(&self, branch: &str) -> u64 {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(branch)
            .copied()
            .unwrap_or(0)
    }
}

impl ActivitySink for ActivityCounter {
    fn command(&self, branch: &str) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(branch.to_owned())
            .or_default() += 1;
    }
}
