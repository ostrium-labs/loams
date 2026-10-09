//! Per-database connection limits (1040) and activity accounting
//! (`ReportActivity`, idle detection in Task 5).

use std::collections::HashMap;
use std::hash::Hash;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

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

/// A token bucket: `rate` tokens per second up to `burst`.
#[derive(Debug, Clone)]
pub struct TokenBucket {
    rate: f64,
    burst: f64,
    tokens: f64,
    refilled: Instant,
}

impl TokenBucket {
    /// A full bucket.
    pub fn new(rate_per_sec: u32, burst: u32) -> Self {
        Self {
            rate: f64::from(rate_per_sec),
            burst: f64::from(burst),
            tokens: f64::from(burst),
            refilled: Instant::now(),
        }
    }

    fn refill(&mut self) {
        let now = Instant::now();
        self.tokens = (self.tokens + now.duration_since(self.refilled).as_secs_f64() * self.rate)
            .min(self.burst);
        self.refilled = now;
    }

    /// Whether a token is left, without taking it.
    pub fn has_token(&mut self) -> bool {
        self.refill();
        self.tokens >= 1.0
    }

    /// Takes one token, if any.
    pub fn take(&mut self) -> bool {
        self.refill();
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }

    /// A full bucket of `burst` tokens, refilled one per `every`.
    pub fn with_refill(burst: u32, every: Duration) -> Self {
        Self {
            rate: 1.0 / every.as_secs_f64().max(f64::MIN_POSITIVE),
            burst: f64::from(burst),
            tokens: f64::from(burst),
            refilled: Instant::now(),
        }
    }

    /// Takes one token if any is left (a charge after the fact; the
    /// balance never goes below zero).
    pub fn charge(&mut self) {
        self.refill();
        self.tokens = (self.tokens - 1.0).max(0.0);
    }

    fn tokens(&mut self) -> f64 {
        self.refill();
        self.tokens
    }

    fn is_full(&mut self) -> bool {
        self.refill();
        self.tokens >= self.burst
    }
}

#[derive(Debug)]
struct Db {
    open: u32,
    bucket: TokenBucket,
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
        let (rate, burst) = (self.config.connect_rate_per_sec, self.config.connect_burst);
        dbs.entry(db.to_owned()).or_insert_with(|| Db {
            open: 0,
            bucket: TokenBucket::new(rate, burst),
        })
    }

    /// Takes one token from `db`'s connection rate (token bucket).
    pub fn admit(&self, db: &str) -> Result<(), LimitError> {
        let mut dbs = self.lock();
        if self.db(&mut dbs, db).bucket.take() {
            Ok(())
        } else {
            Err(LimitError::Rate)
        }
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

/// The key a client source is limited by (R4.2, fix round 2 N2): its IPv4
/// address, with a v4-mapped IPv6 address canonicalised to IPv4, or its
/// IPv6 /64 (one host usually owns a whole /64).
pub fn source_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => IpAddr::V4(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let mut octets = v6.octets();
                octets[8..].fill(0);
                IpAddr::V6(octets.into())
            }
        },
    }
}

/// Pre-authentication limits per client source ([`source_key`]; plan SQ1
/// Task 4 fix rounds 1 and 2): connections still in their handshake, and
/// new connections per second.
#[derive(Debug, Clone, PartialEq)]
pub struct PreAuthConfig {
    /// Connections from one IP in their handshake at once.
    pub per_ip_concurrent: u32,
    /// New connections per second from one IP, sustained.
    pub per_ip_rate_per_sec: u32,
    /// New connections from one IP allowed in a burst.
    pub per_ip_burst: u32,
    /// IPs tracked at once; idle ones are forgotten first, and a new IP is
    /// refused while every tracked one is busy.
    pub max_tracked_ips: usize,
}

impl Default for PreAuthConfig {
    fn default() -> Self {
        Self {
            per_ip_concurrent: 32,
            per_ip_rate_per_sec: 20,
            per_ip_burst: 200,
            max_tracked_ips: 100_000,
        }
    }
}

#[derive(Debug)]
struct Ip {
    open: u32,
    bucket: TokenBucket,
}

/// The per-IP pre-authentication limiter.
#[derive(Debug)]
pub struct PreAuth {
    config: PreAuthConfig,
    ips: Mutex<HashMap<IpAddr, Ip>>,
}

impl PreAuth {
    /// Counters for `config`.
    pub fn new(config: PreAuthConfig) -> Arc<Self> {
        Arc::new(Self {
            config,
            ips: Mutex::new(HashMap::new()),
        })
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<IpAddr, Ip>> {
        self.ips
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Admits a new connection from `ip` into its handshake: one rate
    /// token and one of the IP's handshake slots, released on drop.
    pub fn admit(self: &Arc<Self>, ip: IpAddr) -> Result<PreAuthSlot, LimitError> {
        let ip = source_key(ip);
        let mut ips = self.lock();
        if !ips.contains_key(&ip) && ips.len() >= self.config.max_tracked_ips {
            ips.retain(|_, s| s.open > 0 || !s.bucket.is_full());
            if ips.len() >= self.config.max_tracked_ips {
                return Err(LimitError::Cap);
            }
        }
        let (rate, burst) = (self.config.per_ip_rate_per_sec, self.config.per_ip_burst);
        let s = ips.entry(ip).or_insert_with(|| Ip {
            open: 0,
            bucket: TokenBucket::new(rate, burst),
        });
        if s.open >= self.config.per_ip_concurrent {
            return Err(LimitError::Cap);
        }
        if !s.bucket.take() {
            return Err(LimitError::Rate);
        }
        s.open += 1;
        Ok(PreAuthSlot {
            limiter: self.clone(),
            ip,
        })
    }

    /// Connections from `ip`'s source ([`source_key`]) in their handshake.
    pub fn open(&self, ip: IpAddr) -> u32 {
        self.lock().get(&source_key(ip)).map_or(0, |s| s.open)
    }
}

/// A connection's place in its IP's handshake count.
#[derive(Debug)]
pub struct PreAuthSlot {
    limiter: Arc<PreAuth>,
    ip: IpAddr,
}

impl Drop for PreAuthSlot {
    fn drop(&mut self) {
        if let Some(s) = self.limiter.lock().get_mut(&self.ip) {
            s.open = s.open.saturating_sub(1);
        }
    }
}

/// A failure budget: `burst` failures, refilled one per `refill_every`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FailureLimit {
    /// Failures allowed in a burst.
    pub burst: u32,
    /// One more failure allowed per this interval.
    pub refill_every: Duration,
}

/// Failed-login budgets by key (R4.2): by client source
/// ([`source_key`]) and by user name as sent. A key with an empty bucket
/// is refused before the user lookup and Argon2id. Only failures create
/// entries; the map holds at most `cap` keys, and on overflow forgets
/// full buckets first, then the tenth with the most tokens left.
#[derive(Debug)]
pub struct FailureBuckets<K> {
    limit: FailureLimit,
    cap: usize,
    map: Mutex<HashMap<K, TokenBucket>>,
}

impl<K: Hash + Eq + Clone> FailureBuckets<K> {
    /// Empty, with `limit` per key and at most `cap` keys.
    pub fn new(limit: FailureLimit, cap: usize) -> Self {
        Self {
            limit,
            cap: cap.max(1),
            map: Mutex::new(HashMap::new()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<K, TokenBucket>> {
        self.map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Whether `key` may try again.
    pub fn allows(&self, key: &K) -> bool {
        self.lock().get_mut(key).is_none_or(TokenBucket::has_token)
    }

    /// Charges one failure to `key`.
    pub fn charge(&self, key: K) {
        let mut map = self.lock();
        if !map.contains_key(&key) && map.len() >= self.cap {
            map.retain(|_, b| !b.is_full());
            if map.len() >= self.cap {
                let mut tokens: Vec<f64> = map.values_mut().map(TokenBucket::tokens).collect();
                let drop = (tokens.len() / 10).max(1);
                let at = tokens.len() - drop;
                tokens.select_nth_unstable_by(at, f64::total_cmp);
                let cutoff = tokens[at];
                let mut left = drop;
                map.retain(|_, b| {
                    if left > 0 && b.tokens() >= cutoff {
                        left -= 1;
                        false
                    } else {
                        true
                    }
                });
            }
        }
        let limit = self.limit;
        map.entry(key)
            .or_insert_with(|| TokenBucket::with_refill(limit.burst, limit.refill_every))
            .charge();
    }

    /// Keys held.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether no key is held.
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
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
