//! Progress for the desktop (§85 §12, D2436): bytes done and total,
//! throughput over a 5 s window, ETA, the transport, chunks in flight,
//! retries and the resume state.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::net::Transport;

/// What a transfer is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Connecting,
    Transferring,
    /// Hashing the whole file or the downloaded file.
    Verifying,
    Done,
}

/// One progress report.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    pub tid: String,
    pub direction: Direction,
    pub phase: Phase,
    /// Committed (upload) or verified and durable (download) bytes.
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// The bytes already done when this session started: 0 for a fresh
    /// transfer, more for a resume.
    pub resumed_from: u64,
    /// Bytes per second over the last 5 s.
    pub rate_bps: f64,
    /// Seconds left at that rate, when there is a rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eta_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<Transport>,
    pub inflight: usize,
    pub retries: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Upload,
    Download,
}

const WINDOW: Duration = Duration::from_secs(5);

#[derive(Debug)]
struct Inner {
    p: Progress,
    samples: VecDeque<(Instant, u64)>,
}

/// A shared progress tracker; the pipeline updates it, a reporter samples it.
#[derive(Clone, Debug)]
pub struct Tracker(Arc<Mutex<Inner>>);

impl Tracker {
    pub fn new(tid: &str, direction: Direction, total: u64, done: u64) -> Self {
        Tracker(Arc::new(Mutex::new(Inner {
            p: Progress {
                tid: tid.to_string(),
                direction,
                phase: Phase::Connecting,
                bytes_done: done,
                bytes_total: total,
                resumed_from: done,
                rate_bps: 0.0,
                eta_s: None,
                transport: None,
                inflight: 0,
                retries: 0,
            },
            samples: VecDeque::from([(Instant::now(), done)]),
        })))
    }

    fn with<R>(&self, f: impl FnOnce(&mut Inner) -> R) -> R {
        let mut g = self.0.lock().unwrap_or_else(|p| p.into_inner());
        f(&mut g)
    }

    pub fn set_phase(&self, phase: Phase) {
        self.with(|i| i.p.phase = phase);
    }

    pub fn set_transport(&self, t: Transport) {
        self.with(|i| i.p.transport = Some(t));
    }

    pub fn set_done(&self, bytes: u64) {
        self.with(|i| {
            i.p.bytes_done = bytes;
            let now = Instant::now();
            i.samples.push_back((now, bytes));
            while i.samples.len() > 2 && i.samples.front().is_some_and(|(t, _)| now - *t > WINDOW) {
                i.samples.pop_front();
            }
        });
    }

    /// Sets the done bytes if `bytes` is more (replies can arrive out of
    /// order).
    pub fn raise_done(&self, bytes: u64) {
        let cur = self.with(|i| i.p.bytes_done);
        if bytes > cur {
            self.set_done(bytes);
        }
    }

    pub fn add_done(&self, bytes: u64) {
        let total = self.with(|i| i.p.bytes_done + bytes);
        self.set_done(total);
    }

    pub fn inflight(&self, delta: isize) {
        self.with(|i| i.p.inflight = i.p.inflight.saturating_add_signed(delta));
    }

    pub fn retry(&self) {
        self.with(|i| i.p.retries += 1);
    }

    /// The current report, with rate and ETA computed now.
    pub fn snapshot(&self) -> Progress {
        self.with(|i| {
            let mut p = i.p.clone();
            if let (Some((t0, b0)), Some((t1, b1))) = (i.samples.front(), i.samples.back()) {
                let secs = (*t1 - *t0).as_secs_f64();
                if secs > 0.2 && b1 > b0 {
                    p.rate_bps = (b1 - b0) as f64 / secs;
                    let left = p.bytes_total.saturating_sub(p.bytes_done) as f64;
                    p.eta_s = Some(left / p.rate_bps);
                }
            }
            p
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_and_eta_follow_the_samples() {
        let t = Tracker::new("T", Direction::Upload, 1000, 100);
        {
            let mut g = t.0.lock().expect("lock");
            let start = Instant::now() - Duration::from_secs(2);
            g.samples = VecDeque::from([(start, 100), (start + Duration::from_secs(2), 500)]);
            g.p.bytes_done = 500;
        }
        let p = t.snapshot();
        assert!((p.rate_bps - 200.0).abs() < 1.0, "{p:?}");
        assert!((p.eta_s.expect("eta") - 2.5).abs() < 0.1, "{p:?}");
        assert_eq!(p.resumed_from, 100);
    }
}
