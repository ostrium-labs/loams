//! The background workers (plan KV2 Task 10): the expiry sweep and the GC,
//! each run only while this process holds its lease (`lease`), so one of
//! each runs per keyspace however many gateways share it.

use std::sync::atomic::Ordering;
use std::time::Duration;

use loams_kv::Store;
use tokio::sync::watch;

use crate::metrics::metrics;
use crate::types::now_ms;
use crate::{expire, gc, lease};

/// Index entries (expiry) and data keys (GC) per transaction.
pub const BATCH: usize = 256;

/// How the workers run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerOptions {
    pub databases: u16,
    pub expire_every: Duration,
    pub gc_every: Duration,
}

impl WorkerOptions {
    /// A lease outlives several missed ticks before another holder takes it.
    fn lease_ms(every: Duration) -> u64 {
        u64::try_from(every.as_millis().saturating_mul(10))
            .unwrap_or(u64::MAX)
            .max(5_000)
    }
}

/// This process's lease holder id.
fn holder_id() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let pid = u64::from(std::process::id());
    (u64::try_from(nanos & u128::from(u64::MAX)).unwrap_or(0) ^ pid.rotate_left(32)) | 1
}

/// Runs both workers until `stop` changes.
pub async fn run(store: Store, opts: WorkerOptions, mut stop: watch::Receiver<bool>) {
    let holder = holder_id();
    let mut expire_tick = tokio::time::interval(opts.expire_every);
    let mut gc_tick = tokio::time::interval(opts.gc_every);
    expire_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    gc_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = stop.changed() => return,
            _ = expire_tick.tick() => {
                let lease_ms = WorkerOptions::lease_ms(opts.expire_every);
                match lease::hold(&store, "expire", holder, lease_ms).await {
                    Ok(true) => match expire::sweep(&store, opts.databases, now_ms(), BATCH).await {
                        Ok(n) => {
                            metrics().expired.fetch_add(n, Ordering::Relaxed);
                        }
                        Err(e) => tracing::warn!(error = %e, "resp: the expiry sweep failed"),
                    },
                    Ok(false) => {}
                    Err(e) => tracing::warn!(error = %e, "resp: the expiry lease failed"),
                }
            }
            _ = gc_tick.tick() => {
                let lease_ms = WorkerOptions::lease_ms(opts.gc_every);
                match lease::hold(&store, "gc", holder, lease_ms).await {
                    Ok(true) => match gc::collect(&store, BATCH).await {
                        Ok(n) => {
                            metrics().gc_keys.fetch_add(n, Ordering::Relaxed);
                        }
                        Err(e) => tracing::warn!(error = %e, "resp: the GC failed"),
                    },
                    Ok(false) => {}
                    Err(e) => tracing::warn!(error = %e, "resp: the GC lease failed"),
                }
            }
        }
    }
}
