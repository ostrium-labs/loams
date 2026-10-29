//! `MeterSink`, the usage emitter (design §63 §7.2, ME1 Task 2).
//!
//! Two modes:
//! - **Best-effort** ([`BestEffortSink`]), D365's high-rate path for in-process
//!   producers: a bounded queue (65,536 events or 64 MiB), flushed every 50 ms
//!   or at 1 MiB as one [`Producer::produce`] call, oldest events dropped when
//!   full and counted as `loams_events_dropped_total{reason="queue_full"}`.
//!   [`MeterSink::emit`] never waits on the producer: it encodes the event,
//!   takes a short lock and returns.
//! - **Spooled** ([`SpooledSink`]), the node agent's path: an event is appended
//!   (and fsynced, by the [`Spool`]) before the caller advances its baseline, so
//!   a full spool delays usage instead of losing it ([`Baseline`]).
//!   [`forward_once`] drains the spool into the stream.
//!
//! A failed produce is retried with the **same encoded bytes**, so the same
//! `id`s (dedup layer 1, §63 §7.3).

use std::collections::VecDeque;
use std::fmt;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::event::{EncodedEvent, EventError, UsageEvent};

/// The metric a role exports from [`SinkStats`]' drop counters (§63 §14).
pub const DROPPED_METRIC: &str = "loams_events_dropped_total";

/// Where a product sends usage events. Implementations never block the caller.
pub trait MeterSink: Send + Sync + fmt::Debug {
    fn emit(&self, event: UsageEvent);

    /// Emits many events at once (pipelining, BQ1); the default emits them
    /// one by one. Never blocks the caller.
    fn emit_batch(&self, events: Vec<UsageEvent>) {
        for event in events {
            self.emit(event);
        }
    }
}

impl<T: MeterSink + ?Sized> MeterSink for Arc<T> {
    fn emit(&self, event: UsageEvent) {
        (**self).emit(event);
    }

    fn emit_batch(&self, events: Vec<UsageEvent>) {
        (**self).emit_batch(events);
    }
}

/// Emits nothing: a product without the meter behaves as before (D202, D551).
#[derive(Debug, Default, Clone, Copy)]
pub struct NoSink;

impl MeterSink for NoSink {
    fn emit(&self, _event: UsageEvent) {}
}

/// Keeps every event in memory (tests and `loams dev` diagnostics).
#[derive(Debug, Default)]
pub struct MemorySink(Mutex<Vec<UsageEvent>>);

impl MemorySink {
    pub fn events(&self) -> Vec<UsageEvent> {
        lock(&self.0).clone()
    }
}

impl MeterSink for MemorySink {
    fn emit(&self, event: UsageEvent) {
        lock(&self.0).push(event);
    }
}

/// A produce that failed; the batch is retried as it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProduceError(pub String);

impl fmt::Display for ProduceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "produce to the meter stream failed: {}", self.0)
    }
}

impl std::error::Error for ProduceError {}

/// Appends a batch to `_system/_meter` (§63 §7.1), keyed by each event's
/// `key`, in D270's record layout. Called on the sink's own thread, so it may
/// block (an async producer bridges with its runtime handle's `block_on`).
pub trait Producer: Send + Sync + fmt::Debug + 'static {
    fn produce(&self, batch: &[EncodedEvent]) -> Result<(), ProduceError>;
}

/// The best-effort sink's limits (D365, §63 §7.2).
#[derive(Clone, Debug)]
pub struct BestEffortConfig {
    /// Events queued at most; the oldest is dropped beyond it.
    pub max_events: usize,
    /// Bytes queued at most; the oldest is dropped beyond it.
    pub max_bytes: usize,
    /// A flush happens at least this often while events wait.
    pub flush_interval: Duration,
    /// A flush happens as soon as this many bytes wait; also a batch's size.
    pub flush_bytes: usize,
    /// The first wait after a failed produce; doubles up to `retry_backoff_max`.
    pub retry_backoff_min: Duration,
    pub retry_backoff_max: Duration,
}

impl Default for BestEffortConfig {
    fn default() -> Self {
        Self {
            max_events: 65_536,
            max_bytes: 64 << 20,
            flush_interval: Duration::from_millis(50),
            flush_bytes: 1 << 20,
            retry_backoff_min: Duration::from_millis(10),
            retry_backoff_max: Duration::from_secs(1),
        }
    }
}

/// Counters a role exports as metrics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SinkStats {
    /// Events accepted into the queue.
    pub enqueued: u64,
    /// Events the producer acknowledged.
    pub produced: u64,
    /// `loams_events_dropped_total{reason="queue_full"}`.
    pub dropped_queue_full: u64,
    /// `loams_events_dropped_total{reason="invalid"}`: not in the usage profile.
    pub dropped_invalid: u64,
    /// `loams_events_dropped_total{reason="shutdown"}`: still queued when the
    /// sink closed and the last produce failed.
    pub dropped_shutdown: u64,
    /// Failed produce calls (each is retried).
    pub produce_errors: u64,
}

#[derive(Debug, Default)]
struct Counters {
    enqueued: AtomicU64,
    produced: AtomicU64,
    dropped_queue_full: AtomicU64,
    dropped_invalid: AtomicU64,
    dropped_shutdown: AtomicU64,
    produce_errors: AtomicU64,
}

impl Counters {
    fn add(c: &AtomicU64, n: u64) {
        if n > 0 {
            c.fetch_add(n, Ordering::Relaxed);
        }
    }

    fn snapshot(&self) -> SinkStats {
        SinkStats {
            enqueued: self.enqueued.load(Ordering::Relaxed),
            produced: self.produced.load(Ordering::Relaxed),
            dropped_queue_full: self.dropped_queue_full.load(Ordering::Relaxed),
            dropped_invalid: self.dropped_invalid.load(Ordering::Relaxed),
            dropped_shutdown: self.dropped_shutdown.load(Ordering::Relaxed),
            produce_errors: self.produce_errors.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Default)]
struct Queue {
    events: VecDeque<EncodedEvent>,
    bytes: usize,
    closed: bool,
}

impl Queue {
    /// Drops the oldest events until the queue is within its limits.
    fn enforce(&mut self, max_events: usize, max_bytes: usize) -> u64 {
        let mut dropped = 0;
        while self.events.len() > max_events || self.bytes > max_bytes {
            let Some(old) = self.events.pop_front() else {
                break;
            };
            self.bytes -= old.size();
            dropped += 1;
        }
        dropped
    }

    /// The oldest events up to `max_bytes` (at least one).
    fn take_batch(&mut self, max_bytes: usize) -> Vec<EncodedEvent> {
        let mut batch = Vec::new();
        let mut size = 0;
        while let Some(front) = self.events.front() {
            if !batch.is_empty() && size + front.size() > max_bytes {
                break;
            }
            let Some(ev) = self.events.pop_front() else {
                break;
            };
            size += ev.size();
            batch.push(ev);
        }
        self.bytes -= size;
        batch
    }

    /// Puts a failed batch back at the front, in order.
    fn requeue(&mut self, batch: Vec<EncodedEvent>) {
        for ev in batch.into_iter().rev() {
            self.bytes += ev.size();
            self.events.push_front(ev);
        }
    }
}

#[derive(Debug)]
struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
    config: BestEffortConfig,
    counters: Counters,
}

/// D365's best-effort emitter (§63 §7.2 path 1).
#[derive(Debug)]
pub struct BestEffortSink {
    shared: Arc<Shared>,
    flusher: Mutex<Option<JoinHandle<()>>>,
}

impl BestEffortSink {
    /// Starts the flush thread, which hands batches to `producer`.
    pub fn new(producer: Arc<dyn Producer>, config: BestEffortConfig) -> io::Result<Self> {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue::default()),
            wake: Condvar::new(),
            config,
            counters: Counters::default(),
        });
        let worker = Arc::clone(&shared);
        let flusher = std::thread::Builder::new()
            .name("loams-meter-sink".into())
            .spawn(move || flush_loop(&worker, producer.as_ref()))?;
        Ok(Self {
            shared,
            flusher: Mutex::new(Some(flusher)),
        })
    }

    pub fn stats(&self) -> SinkStats {
        self.shared.counters.snapshot()
    }

    /// Events waiting in the queue (not counting a batch being produced).
    pub fn queued(&self) -> usize {
        lock(&self.shared.queue).events.len()
    }

    /// Flushes what is queued (one more produce attempt) and stops the thread.
    /// Called by `Drop`; idempotent.
    pub fn close(&self) {
        lock(&self.shared.queue).closed = true;
        self.shared.wake.notify_all();
        if let Some(handle) = lock(&self.flusher).take() {
            let _ = handle.join();
        }
    }

    fn push_many(&self, events: Vec<EncodedEvent>) {
        if events.is_empty() {
            return;
        }
        let cfg = &self.shared.config;
        let c = &self.shared.counters;
        let n = events.len() as u64;
        let mut q = lock(&self.shared.queue);
        if q.closed {
            drop(q);
            Counters::add(&c.dropped_shutdown, n);
            return;
        }
        for ev in events {
            q.bytes += ev.size();
            q.events.push_back(ev);
        }
        let dropped = q.enforce(cfg.max_events, cfg.max_bytes);
        let full_batch = q.bytes >= cfg.flush_bytes;
        drop(q);
        Counters::add(&c.enqueued, n);
        Counters::add(&c.dropped_queue_full, dropped);
        if full_batch {
            self.shared.wake.notify_one();
        }
    }

    fn push(&self, ev: EncodedEvent) {
        let cfg = &self.shared.config;
        let c = &self.shared.counters;
        let mut q = lock(&self.shared.queue);
        if q.closed {
            drop(q);
            Counters::add(&c.dropped_shutdown, 1);
            return;
        }
        q.bytes += ev.size();
        q.events.push_back(ev);
        let dropped = q.enforce(cfg.max_events, cfg.max_bytes);
        let full_batch = q.bytes >= cfg.flush_bytes;
        drop(q);
        Counters::add(&c.enqueued, 1);
        Counters::add(&c.dropped_queue_full, dropped);
        if full_batch {
            self.shared.wake.notify_one();
        }
    }
}

impl MeterSink for BestEffortSink {
    fn emit(&self, event: UsageEvent) {
        if event.validate().is_err() {
            Counters::add(&self.shared.counters.dropped_invalid, 1);
            return;
        }
        self.push(event.encode());
    }

    /// Validates and encodes outside the lock, then takes the queue's lock
    /// once for the whole batch, enforces the limits once and wakes the
    /// flusher at most once (BQ1 pipelining).
    fn emit_batch(&self, events: Vec<UsageEvent>) {
        let total = events.len();
        let encoded: Vec<EncodedEvent> = events
            .iter()
            .filter(|e| e.validate().is_ok())
            .map(UsageEvent::encode)
            .collect();
        Counters::add(
            &self.shared.counters.dropped_invalid,
            (total - encoded.len()) as u64,
        );
        self.push_many(encoded);
    }
}

impl Drop for BestEffortSink {
    fn drop(&mut self) {
        self.close();
    }
}

fn flush_loop(shared: &Shared, producer: &dyn Producer) {
    let cfg = &shared.config;
    let c = &shared.counters;
    let mut backoff = cfg.retry_backoff_min;
    loop {
        let (batch, closed) = {
            let mut q = lock(&shared.queue);
            let deadline = Instant::now() + cfg.flush_interval;
            while !q.closed && q.bytes < cfg.flush_bytes {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                q = match shared.wake.wait_timeout(q, deadline - now) {
                    Ok((g, _)) => g,
                    Err(poisoned) => poisoned.into_inner().0,
                };
            }
            if q.events.is_empty() {
                if q.closed {
                    return;
                }
                continue;
            }
            (q.take_batch(cfg.flush_bytes), q.closed)
        };
        let n = batch.len() as u64;
        match producer.produce(&batch) {
            Ok(()) => {
                Counters::add(&c.produced, n);
                backoff = cfg.retry_backoff_min;
            }
            Err(_) => {
                Counters::add(&c.produce_errors, 1);
                if closed {
                    // One last attempt failed while closing: what is left is
                    // dropped and counted, so a dead stream cannot hang a
                    // shutdown.
                    let mut q = lock(&shared.queue);
                    let rest = q.events.len() as u64;
                    q.events.clear();
                    q.bytes = 0;
                    drop(q);
                    Counters::add(&c.dropped_shutdown, n + rest);
                    return;
                }
                let mut q = lock(&shared.queue);
                q.requeue(batch);
                let dropped = q.enforce(cfg.max_events, cfg.max_bytes);
                Counters::add(&c.dropped_queue_full, dropped);
                // Wait out the backoff, but wake at once on close.
                let until = Instant::now() + backoff;
                while !q.closed {
                    let now = Instant::now();
                    if now >= until {
                        break;
                    }
                    q = match shared.wake.wait_timeout(q, until - now) {
                        Ok((g, _)) => g,
                        Err(poisoned) => poisoned.into_inner().0,
                    };
                }
                backoff = (backoff * 2).min(cfg.retry_backoff_max);
            }
        }
    }
}

/// Why the spool refused an event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpoolError {
    /// The spool is at its size bound: the caller keeps its baseline.
    Full,
    /// The append or fsync failed.
    Io(String),
    /// The event is not in the usage profile.
    Invalid(EventError),
}

impl fmt::Display for SpoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full => f.write_str("usage spool is full"),
            Self::Io(e) => write!(f, "usage spool: {e}"),
            Self::Invalid(e) => write!(f, "usage spool: {e}"),
        }
    }
}

impl std::error::Error for SpoolError {}

/// A size-bounded, durable, ordered spool (the node agent's, platform PD4).
/// `loams-meter-agent` implements it on disk; [`MemSpool`] is the in-memory one.
pub trait Spool: Send + Sync + fmt::Debug {
    /// Appends durably (fsynced) or not at all.
    fn append(&self, event: &EncodedEvent) -> Result<(), SpoolError>;
    /// Up to `max` of the oldest unacknowledged events with their positions.
    fn peek(&self, max: usize) -> Result<Vec<(u64, EncodedEvent)>, SpoolError>;
    /// Forgets every event up to and including position `upto`.
    fn ack(&self, upto: u64) -> Result<(), SpoolError>;
}

/// Spooled mode (§63 §7.2 path 2): [`SpooledSink::emit`] returns only after
/// the spool holds the event, and says when it does not.
#[derive(Debug)]
pub struct SpooledSink<S: Spool> {
    spool: S,
}

impl<S: Spool> SpooledSink<S> {
    pub fn new(spool: S) -> Self {
        Self { spool }
    }

    pub fn spool(&self) -> &S {
        &self.spool
    }

    /// Appends `event`; `Err` means it is not spooled and the caller must not
    /// advance its baseline ([`Baseline::advance`]).
    pub fn emit(&self, event: &UsageEvent) -> Result<(), SpoolError> {
        event.validate().map_err(SpoolError::Invalid)?;
        self.spool.append(&event.encode())
    }
}

/// A cumulative counter's last reading that reached the spool. Usage is the
/// delta from it, and the baseline moves only after the append succeeds, so a
/// full spool keeps the usage in the counter for a later read (PD4, PD8).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Baseline {
    last: u64,
}

impl Baseline {
    pub fn new(start: u64) -> Self {
        Self { last: start }
    }

    pub fn value(&self) -> u64 {
        self.last
    }

    /// The delta since the baseline for a `cumulative` reading, handed to
    /// `append`; the baseline advances to `cumulative` only if it succeeds. A
    /// reading below the baseline is a reset counter: the delta is the reading.
    /// A zero delta appends nothing.
    pub fn advance<E>(
        &mut self,
        cumulative: u64,
        append: impl FnOnce(u64) -> Result<(), E>,
    ) -> Result<u64, E> {
        let delta = if cumulative >= self.last {
            cumulative - self.last
        } else {
            cumulative
        };
        if delta > 0 {
            append(delta)?;
        }
        self.last = cumulative;
        Ok(delta)
    }
}

/// Why a forward pass stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForwardError {
    Spool(SpoolError),
    Produce(ProduceError),
}

impl fmt::Display for ForwardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spool(e) => e.fmt(f),
            Self::Produce(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for ForwardError {}

/// Moves up to `max` spooled events to the stream; acknowledges them only
/// after the produce succeeded, so a failure resends the same bytes.
pub fn forward_once(
    spool: &dyn Spool,
    producer: &dyn Producer,
    max: usize,
) -> Result<usize, ForwardError> {
    let items = spool.peek(max).map_err(ForwardError::Spool)?;
    let Some(&(last, _)) = items.last() else {
        return Ok(0);
    };
    let batch: Vec<EncodedEvent> = items.into_iter().map(|(_, ev)| ev).collect();
    producer.produce(&batch).map_err(ForwardError::Produce)?;
    spool.ack(last).map_err(ForwardError::Spool)?;
    Ok(batch.len())
}

#[derive(Debug, Default)]
struct MemSpoolState {
    next: u64,
    items: VecDeque<(u64, EncodedEvent)>,
    bytes: usize,
}

/// An in-memory [`Spool`] bounded by bytes (tests, single-host dev). Not
/// durable: the agent's on-disk spool is the production one.
#[derive(Debug)]
pub struct MemSpool {
    max_bytes: usize,
    state: Mutex<MemSpoolState>,
}

impl MemSpool {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            state: Mutex::new(MemSpoolState::default()),
        }
    }

    pub fn len(&self) -> usize {
        lock(&self.state).items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Spool for MemSpool {
    fn append(&self, event: &EncodedEvent) -> Result<(), SpoolError> {
        let mut s = lock(&self.state);
        if s.bytes + event.size() > self.max_bytes {
            return Err(SpoolError::Full);
        }
        let pos = s.next;
        s.next += 1;
        s.bytes += event.size();
        s.items.push_back((pos, event.clone()));
        Ok(())
    }

    fn peek(&self, max: usize) -> Result<Vec<(u64, EncodedEvent)>, SpoolError> {
        Ok(lock(&self.state).items.iter().take(max).cloned().collect())
    }

    fn ack(&self, upto: u64) -> Result<(), SpoolError> {
        let mut s = lock(&self.state);
        while s.items.front().is_some_and(|(pos, _)| *pos <= upto) {
            if let Some((_, ev)) = s.items.pop_front() {
                s.bytes -= ev.size();
            }
        }
        Ok(())
    }
}

/// Locks, ignoring poisoning: the guarded state stays consistent between
/// statements, and a panicking producer must not stop metering.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::types;
    use std::sync::atomic::AtomicUsize;

    fn ev(i: u64) -> UsageEvent {
        UsageEvent::new(types::GATEWAY_REQUEST, format!("req-{i}"))
            .source("loams://test/gateway/n1")
            .subject("org_1")
            .tenant("1/1")
            .at_ns(1)
            .num("bytes_in", i)
    }

    fn id_of(e: &EncodedEvent) -> String {
        let v: serde_json::Value = serde_json::from_slice(&e.payload).unwrap();
        v["id"].as_str().unwrap().to_owned()
    }

    fn wait_until(mut f: impl FnMut() -> bool) -> bool {
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        false
    }

    /// Records every batch it is handed.
    #[derive(Debug, Default)]
    struct Recording {
        attempts: Mutex<Vec<Vec<EncodedEvent>>>,
        fail_first: AtomicUsize,
    }

    impl Producer for Recording {
        fn produce(&self, batch: &[EncodedEvent]) -> Result<(), ProduceError> {
            lock(&self.attempts).push(batch.to_vec());
            if self
                .fail_first
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
            {
                return Err(ProduceError("unavailable".into()));
            }
            Ok(())
        }
    }

    /// Blocks every produce until released.
    #[derive(Debug, Default)]
    struct Blocking {
        released: Mutex<bool>,
        cv: Condvar,
    }

    impl Blocking {
        fn release(&self) {
            *lock(&self.released) = true;
            self.cv.notify_all();
        }
    }

    impl Producer for Blocking {
        fn produce(&self, _batch: &[EncodedEvent]) -> Result<(), ProduceError> {
            let mut r = lock(&self.released);
            while !*r {
                r = self.cv.wait(r).unwrap_or_else(|e| e.into_inner());
            }
            Ok(())
        }
    }

    #[test]
    fn sink_never_blocks() {
        let producer = Arc::new(Blocking::default());
        let sink = BestEffortSink::new(
            producer.clone(),
            BestEffortConfig {
                max_events: 1_000,
                flush_interval: Duration::from_millis(1),
                ..BestEffortConfig::default()
            },
        )
        .unwrap();
        let start = Instant::now();
        for i in 0..200_000 {
            sink.emit(ev(i));
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(5),
            "emit blocked: {elapsed:?}"
        );
        let stats = sink.stats();
        assert!(stats.dropped_queue_full > 0);
        assert!(sink.queued() <= 1_000);
        producer.release();
        sink.close();
    }

    #[test]
    fn emit_batch_enqueues_once_and_counts_invalid() {
        let producer = Arc::new(Recording::default());
        let sink = BestEffortSink::new(producer.clone(), BestEffortConfig::default()).unwrap();
        let mut batch: Vec<UsageEvent> = (0..10).map(ev).collect();
        batch.push(UsageEvent::new(
            "io.loams.dev.usage.house.query.v1",
            "no-subject",
        ));
        sink.emit_batch(batch);
        sink.close();
        let stats = sink.stats();
        assert_eq!(stats.enqueued, 10);
        assert_eq!(stats.dropped_invalid, 1);
        assert_eq!(stats.produced, 10);
        let sent: Vec<EncodedEvent> = lock(&producer.attempts).concat();
        let expected: Vec<EncodedEvent> = (0..10).map(|i| ev(i).encode()).collect();
        assert_eq!(sent, expected, "order kept");
    }

    #[test]
    fn sink_drops_oldest_when_full() {
        let producer = Arc::new(Recording::default());
        let sink = BestEffortSink::new(
            producer.clone(),
            BestEffortConfig {
                max_events: 4,
                flush_interval: Duration::from_secs(3600),
                flush_bytes: usize::MAX,
                ..BestEffortConfig::default()
            },
        )
        .unwrap();
        for i in 0..10 {
            sink.emit(ev(i));
        }
        assert_eq!(sink.stats().dropped_queue_full, 6);
        sink.close();
        let attempts = lock(&producer.attempts).clone();
        let ids: Vec<String> = attempts.iter().flatten().map(id_of).collect();
        assert_eq!(ids, ["req-6", "req-7", "req-8", "req-9"]);
        assert_eq!(sink.stats().produced, 4);
    }

    #[test]
    fn sink_drops_oldest_by_bytes() {
        let size = ev(0).encode().size();
        let producer = Arc::new(Recording::default());
        let sink = BestEffortSink::new(
            producer,
            BestEffortConfig {
                max_bytes: size * 3,
                flush_interval: Duration::from_secs(3600),
                flush_bytes: usize::MAX,
                ..BestEffortConfig::default()
            },
        )
        .unwrap();
        for i in 0..5 {
            sink.emit(ev(i));
        }
        assert_eq!(sink.queued(), 3);
        assert_eq!(sink.stats().dropped_queue_full, 2);
    }

    #[test]
    fn retry_keeps_ids() {
        let producer = Arc::new(Recording {
            fail_first: AtomicUsize::new(2),
            ..Recording::default()
        });
        let sink = BestEffortSink::new(
            producer.clone(),
            BestEffortConfig {
                flush_interval: Duration::from_millis(5),
                retry_backoff_min: Duration::from_millis(1),
                retry_backoff_max: Duration::from_millis(4),
                ..BestEffortConfig::default()
            },
        )
        .unwrap();
        for i in 0..3 {
            sink.emit(ev(i));
        }
        assert!(wait_until(|| sink.stats().produced == 3));
        let attempts = lock(&producer.attempts).clone();
        assert_eq!(attempts.len(), 3, "two failures then one success");
        assert!(attempts.iter().all(|a| a == &attempts[0]));
        let ids: Vec<String> = attempts[0].iter().map(id_of).collect();
        assert_eq!(ids, ["req-0", "req-1", "req-2"]);
        assert_eq!(sink.stats().produce_errors, 2);
    }

    #[test]
    fn invalid_events_are_counted_not_sent() {
        let producer = Arc::new(Recording::default());
        let sink = BestEffortSink::new(producer.clone(), BestEffortConfig::default()).unwrap();
        sink.emit(UsageEvent::new(types::JOBS_RUN, "x"));
        sink.close();
        assert_eq!(sink.stats().dropped_invalid, 1);
        assert!(lock(&producer.attempts).is_empty());
    }

    #[test]
    fn close_with_dead_stream_drops_and_returns() {
        let producer = Arc::new(Recording {
            fail_first: AtomicUsize::new(usize::MAX),
            ..Recording::default()
        });
        let sink = BestEffortSink::new(
            producer,
            BestEffortConfig {
                flush_interval: Duration::from_secs(3600),
                flush_bytes: usize::MAX,
                ..BestEffortConfig::default()
            },
        )
        .unwrap();
        sink.emit(ev(1));
        sink.emit(ev(2));
        sink.close();
        assert_eq!(sink.stats().dropped_shutdown, 2);
    }

    #[test]
    fn spooled_full_keeps_usage_in_counters() {
        let one = ev(0).encode().size();
        let sink = SpooledSink::new(MemSpool::new(one + one / 2));
        let producer = Recording::default();
        let mut base = Baseline::new(0);
        let emit = |delta: u64| sink.emit(&ev(delta));

        assert_eq!(base.advance(100, emit), Ok(100));
        // The spool holds one event: the next append is refused and the
        // baseline stays where it was.
        assert_eq!(base.advance(250, emit), Err(SpoolError::Full));
        assert_eq!(base.value(), 100);

        assert_eq!(forward_once(sink.spool(), &producer, 16), Ok(1));
        assert!(sink.spool().is_empty());

        // The delta now covers everything since the last spooled reading.
        assert_eq!(base.advance(300, emit), Ok(200));
        assert_eq!(forward_once(sink.spool(), &producer, 16), Ok(1));

        let total: u64 = lock(&producer.attempts)
            .iter()
            .flatten()
            .map(|e| {
                let v: serde_json::Value = serde_json::from_slice(&e.payload).unwrap();
                v["data"]["bytes_in"].as_u64().unwrap()
            })
            .sum();
        assert_eq!(total, 300, "no usage lost to the full spool");
    }

    #[test]
    fn forward_failure_keeps_the_spool() {
        let spool = MemSpool::new(1 << 20);
        spool.append(&ev(1).encode()).unwrap();
        let producer = Recording {
            fail_first: AtomicUsize::new(1),
            ..Recording::default()
        };
        assert!(matches!(
            forward_once(&spool, &producer, 16),
            Err(ForwardError::Produce(_))
        ));
        assert_eq!(spool.len(), 1);
        assert_eq!(forward_once(&spool, &producer, 16), Ok(1));
        let attempts = lock(&producer.attempts).clone();
        assert_eq!(attempts[0], attempts[1], "the retry resends the same bytes");
    }

    #[test]
    fn baseline_reset_counter_counts_from_zero() {
        let mut b = Baseline::new(500);
        assert_eq!(b.advance(20, |_| Ok::<(), ()>(())), Ok(20));
        assert_eq!(b.value(), 20);
    }

    #[test]
    fn no_sink_emits_nothing() {
        NoSink.emit(ev(1));
        let m = MemorySink::default();
        m.emit(ev(1));
        assert_eq!(m.events().len(), 1);
    }
}
