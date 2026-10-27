//! Rate limiting (CN2 Task 4): one token bucket per instance, for requests and
//! for bytes, shared by its splits and adjusted by `Throttled`.
//!
//! A request larger than the burst waits for a full bucket and then goes,
//! leaving the bucket in debt by the rest, so the long-run rate holds for
//! any request size (CC1 Task 1: a source's batch is charged after it is
//! read, and may be larger than one second's worth). An operator may change
//! the limits of a running instance ([`RateLimiter::set_limit`]).

use std::sync::Mutex;
use std::time::Duration;

use tokio::time::Instant;

/// A rate: `amount` per `per`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rate {
    pub amount: f64,
    pub per: Duration,
}

impl Rate {
    pub fn per_second(amount: f64) -> Self {
        Self {
            amount,
            per: Duration::from_secs(1),
        }
    }

    fn per_sec(&self) -> f64 {
        self.amount / self.per.as_secs_f64().max(f64::MIN_POSITIVE)
    }
}

/// An instance's configured limits; `None` is unlimited.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RateLimit {
    pub requests: Option<Rate>,
    pub bytes: Option<Rate>,
}

impl RateLimit {
    /// The `rate_limit` object of an instance config (CC1 Task 1 and 2):
    /// `{"bytes_per_sec": N, "requests_per_sec": M}`, both optional and
    /// positive. Absent or `null` is unlimited.
    pub fn from_config(config: &serde_json::Value) -> Result<Self, crate::ConnectorError> {
        match config.get("rate_limit") {
            None | Some(serde_json::Value::Null) => Ok(Self::default()),
            Some(v) => Self::from_json(v),
        }
    }

    /// The `{"bytes_per_sec", "requests_per_sec"}` object itself.
    pub fn from_json(v: &serde_json::Value) -> Result<Self, crate::ConnectorError> {
        let o = v
            .as_object()
            .ok_or_else(|| crate::ConnectorError::fatal("rate_limit must be an object"))?;
        let mut out = Self::default();
        for (k, v) in o {
            let rate = match v {
                serde_json::Value::Null => None,
                v => Some(
                    v.as_f64()
                        .filter(|n| n.is_finite() && *n > 0.0)
                        .map(Rate::per_second)
                        .ok_or_else(|| {
                            crate::ConnectorError::fatal(format!(
                                "rate_limit.{k} must be a positive number"
                            ))
                        })?,
                ),
            };
            match k.as_str() {
                "bytes_per_sec" => out.bytes = rate,
                "requests_per_sec" => out.requests = rate,
                other => {
                    return Err(crate::ConnectorError::fatal(format!(
                        "unknown setting rate_limit.{other}"
                    )));
                }
            }
        }
        Ok(out)
    }

    /// The bytes per second, when limited.
    pub fn bytes_per_sec(&self) -> Option<f64> {
        self.bytes.map(|r| r.per_sec())
    }

    /// The requests per second, when limited.
    pub fn requests_per_sec(&self) -> Option<f64> {
        self.requests.map(|r| r.per_sec())
    }
}

/// The factor a `Throttled` multiplies the rate by, its floor, and how fast a
/// success brings it back.
const THROTTLE_FACTOR: f64 = 0.5;
const MIN_FACTOR: f64 = 1.0 / 16.0;
const RECOVERY: f64 = 1.05;

#[derive(Debug)]
struct Bucket {
    rate_per_sec: f64,
    capacity: f64,
    tokens: f64,
}

impl Bucket {
    fn new(rate: Rate) -> Self {
        let per_sec = rate.per_sec();
        // Burst: one period's worth, at least one unit.
        let capacity = rate.amount.max(1.0);
        Self {
            rate_per_sec: per_sec,
            capacity,
            tokens: capacity,
        }
    }

    fn refill(&mut self, elapsed: Duration, factor: f64) {
        self.tokens =
            (self.tokens + elapsed.as_secs_f64() * self.rate_per_sec * factor).min(self.capacity);
    }

    /// Time until `n` tokens are there (capped at the capacity, so a request
    /// bigger than the burst waits for a full bucket and then goes). The
    /// bucket may be in debt from such a request.
    fn wait_for(&self, n: f64, factor: f64) -> Duration {
        let n = n.min(self.capacity);
        if self.tokens >= n {
            Duration::ZERO
        } else {
            Duration::from_secs_f64((n - self.tokens) / (self.rate_per_sec * factor))
        }
    }

    /// Take `n` tokens, all of them: a request bigger than the burst leaves
    /// the bucket in debt, which later requests wait out.
    fn take(&mut self, n: f64) {
        self.tokens -= n;
    }

    /// A bucket for `rate` that keeps this one's debt or, at most, a full
    /// new burst (a lowered limit applies at once, with no fresh burst).
    fn replaced(&self, rate: Rate) -> Self {
        let mut b = Self::new(rate);
        b.tokens = self.tokens.min(b.capacity);
        b
    }
}

#[derive(Debug)]
struct State {
    requests: Option<Bucket>,
    bytes: Option<Bucket>,
    limit: RateLimit,
    factor: f64,
    blocked_until: Option<Instant>,
    last: Instant,
}

/// The token bucket of one instance.
#[derive(Debug)]
pub struct RateLimiter {
    state: Mutex<State>,
}

/// Which buckets an acquire draws on.
#[derive(Clone, Copy)]
struct Draw {
    request: bool,
    bytes: u64,
}

impl RateLimiter {
    pub fn new(limit: RateLimit) -> Self {
        Self {
            state: Mutex::new(State {
                requests: limit.requests.map(Bucket::new),
                bytes: limit.bytes.map(Bucket::new),
                limit,
                factor: 1.0,
                blocked_until: None,
                last: Instant::now(),
            }),
        }
    }

    pub fn unlimited() -> Self {
        Self::new(RateLimit::default())
    }

    /// Wait until one request of `bytes` may go, then take its tokens.
    pub async fn acquire(&self, bytes: u64) {
        self.draw(Draw {
            request: true,
            bytes,
        })
        .await;
    }

    /// Wait until `bytes` may go, then take them from the byte bucket only
    /// (a source's batch, charged after it was read: its request was
    /// [`RateLimiter::acquire`]d before the read).
    pub async fn acquire_bytes(&self, bytes: u64) {
        if bytes > 0 {
            self.draw(Draw {
                request: false,
                bytes,
            })
            .await;
        }
    }

    /// Replace the limits of a running instance (an operator, or the
    /// Confluent budget guard). A lowered limit applies at once.
    pub fn set_limit(&self, limit: RateLimit) {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        s.requests = limit.requests.map(|r| match &s.requests {
            Some(old) => old.replaced(r),
            None => Bucket::new(r),
        });
        s.bytes = limit.bytes.map(|r| match &s.bytes {
            Some(old) => old.replaced(r),
            None => Bucket::new(r),
        });
        s.limit = limit;
    }

    /// The configured limits.
    pub fn limit(&self) -> RateLimit {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).limit
    }

    async fn draw(&self, d: Draw) {
        let Draw { request, bytes } = d;
        loop {
            let wait = {
                let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
                let now = Instant::now();
                let elapsed = now.saturating_duration_since(s.last);
                s.last = now;
                let factor = s.factor;
                if let Some(b) = s.requests.as_mut() {
                    b.refill(elapsed, factor);
                }
                if let Some(b) = s.bytes.as_mut() {
                    b.refill(elapsed, factor);
                }
                let blocked = s
                    .blocked_until
                    .map_or(Duration::ZERO, |u| u.saturating_duration_since(now));
                let req = s
                    .requests
                    .as_ref()
                    .filter(|_| request)
                    .map_or(Duration::ZERO, |b| b.wait_for(1.0, factor));
                let byt = s
                    .bytes
                    .as_ref()
                    .map_or(Duration::ZERO, |b| b.wait_for(bytes as f64, factor));
                let wait = blocked.max(req).max(byt);
                if wait.is_zero() {
                    if let Some(b) = s.requests.as_mut().filter(|_| request) {
                        b.take(1.0);
                    }
                    if let Some(b) = s.bytes.as_mut() {
                        b.take(bytes as f64);
                    }
                    return;
                }
                wait
            };
            tokio::time::sleep(wait).await;
        }
    }

    /// The system said `Throttled { after }`: nobody on this instance sends
    /// before `after` has passed, and the rate halves (down to 1/16 of the
    /// configured rate).
    pub fn throttled(&self, after: Duration) {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let until = Instant::now() + after;
        s.blocked_until = Some(s.blocked_until.map_or(until, |u| u.max(until)));
        s.factor = (s.factor * THROTTLE_FACTOR).max(MIN_FACTOR);
    }

    /// A request succeeded: the rate creeps back towards the configured one.
    pub fn succeeded(&self) {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        s.factor = (s.factor * RECOVERY).min(1.0);
    }

    /// The current fraction of the configured rate.
    pub fn factor(&self) -> f64 {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).factor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_config_parses() {
        let r = RateLimit::from_config(&serde_json::json!({
            "rate_limit": {"bytes_per_sec": 65536, "requests_per_sec": 10}
        }))
        .unwrap_or_default();
        assert_eq!(r.bytes_per_sec(), Some(65536.0));
        assert_eq!(r.requests_per_sec(), Some(10.0));
        assert_eq!(
            RateLimit::from_config(&serde_json::json!({})).ok(),
            Some(RateLimit::default())
        );
        for bad in [
            serde_json::json!({"rate_limit": {"bytes_per_sec": 0}}),
            serde_json::json!({"rate_limit": {"bytes_per_sec": -1}}),
            serde_json::json!({"rate_limit": {"bogus": 1}}),
            serde_json::json!({"rate_limit": 5}),
        ] {
            assert!(RateLimit::from_config(&bad).is_err(), "{bad}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn large_requests_hold_the_long_run_rate() {
        // 1 000 B/s, requests of 4 000 B: the first goes at once (full
        // bucket), each later one waits for the debt: 10 requests take ~36 s.
        let l = RateLimiter::new(RateLimit {
            requests: None,
            bytes: Some(Rate::per_second(1_000.0)),
        });
        let t0 = Instant::now();
        for _ in 0..10 {
            l.acquire_bytes(4_000).await;
        }
        let secs = t0.elapsed().as_secs_f64();
        assert!((35.0..=37.5).contains(&secs), "{secs}");
    }

    #[tokio::test(start_paused = true)]
    async fn set_limit_applies_at_once() {
        let l = RateLimiter::new(RateLimit::default());
        l.acquire(1_000_000).await;
        l.set_limit(RateLimit {
            requests: None,
            bytes: Some(Rate::per_second(100.0)),
        });
        assert_eq!(l.limit().bytes_per_sec(), Some(100.0));
        let t0 = Instant::now();
        l.acquire(100).await;
        l.acquire(100).await;
        assert!(
            t0.elapsed() >= Duration::from_millis(900),
            "{:?}",
            t0.elapsed()
        );
    }
}
