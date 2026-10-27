//! Turns cgroup readings into per-window deltas (design 01 §7).
//!
//! The rules:
//!
//! - **Counters, not gauges.** CPU is a cumulative counter, so a missed
//!   sample folds into the next delta and nothing is lost.
//! - **Resets.** A counter that decreases under the same resource id is a
//!   reset: the delta is the new reading, never negative.
//! - **Churn.** Resources are keyed by path and inode, so a cgroup recreated
//!   at the same path is a new resource starting at zero. A resource missing
//!   from a scan is dropped.
//! - **First sight.** While the tracker is *warm* (it has continuous state),
//!   a new resource's whole counter is usage. A *cold* tracker (an agent
//!   that lost its state) baselines unknown resources at their current
//!   reading: an undercount, never a double bill.
//! - **Two-phase.** [`DeltaTracker::observe`] computes deltas without
//!   changing anything; [`DeltaTracker::commit`] advances the baselines only
//!   after the events are safely spooled. A refused append leaves the usage
//!   in the counters, to be carried by the next delta.
//! - **Memory** is a gauge, integrated as a trapezoid over the window while
//!   the cgroup is populated, in byte-seconds.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::cgroup::CgroupSample;
use crate::event::{Meter, MeterEvent};

/// The last committed reading of one resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Baseline {
    /// Cumulative CPU microseconds.
    pub cpu_usec: u64,
    /// Resident bytes, when known.
    pub memory_bytes: Option<u64>,
    /// Whether any process was in the cgroup.
    pub populated: bool,
    /// When it was read, in Unix milliseconds.
    pub at_ms: i64,
}

/// Everything the tracker needs to resume after a restart.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackerState {
    /// Whether the tracker has continuous state (see the module docs).
    pub warm: bool,
    /// When the last committed scan ran, in Unix milliseconds.
    pub last_scan_ms: Option<i64>,
    /// Baselines by resource id.
    pub baselines: BTreeMap<String, Baseline>,
}

/// One resource's usage over one window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CgroupDelta {
    /// The reading at the window's end.
    pub sample: CgroupSample,
    /// The window's start, in Unix milliseconds.
    pub window_start_ms: i64,
    /// The window's end, in Unix milliseconds.
    pub window_end_ms: i64,
    /// CPU microseconds used in the window.
    pub cpu_usec: u64,
    /// Byte-seconds of resident memory in the window.
    pub memory_byte_seconds: u64,
    /// Whether the CPU counter went backwards (a reset).
    pub reset: bool,
}

/// The result of [`DeltaTracker::observe`]: deltas to emit, and the
/// baselines to adopt once they are spooled.
#[derive(Debug, Clone)]
pub struct Observation {
    /// Non-zero deltas, in resource order.
    pub deltas: Vec<CgroupDelta>,
    next: BTreeMap<String, Baseline>,
    at_ms: i64,
}

/// Computes deltas from successive scans.
#[derive(Debug, Clone, Default)]
pub struct DeltaTracker {
    state: TrackerState,
}

impl DeltaTracker {
    /// A cold tracker: the first scan only sets baselines.
    pub fn new() -> Self {
        Self::default()
    }

    /// A tracker resumed from saved state.
    pub fn from_state(state: TrackerState) -> Self {
        Self { state }
    }

    /// The state to save.
    pub fn state(&self) -> &TrackerState {
        &self.state
    }

    /// Keeps the committed baselines of `paths` (cgroups skipped this scan
    /// because they could not be read) in `observation`, so their usage is
    /// counted from the old baseline at the next good reading instead of
    /// being dropped or counted twice.
    pub fn carry(&self, observation: &mut Observation, paths: &[String]) {
        if paths.is_empty() {
            return;
        }
        for (id, base) in &self.state.baselines {
            let path = id.rsplit_once("#ino=").map_or(id.as_str(), |(p, _)| p);
            if paths.iter().any(|p| p == path) {
                observation
                    .next
                    .entry(id.clone())
                    .or_insert_with(|| base.clone());
            }
        }
    }

    /// Computes the deltas of `samples`, read at `now_ms`, against the
    /// committed baselines. Changes nothing.
    pub fn observe(&self, samples: &[CgroupSample], now_ms: i64) -> Observation {
        let mut deltas = Vec::new();
        let mut next = BTreeMap::new();
        for sample in samples {
            let id = sample.resource_id();
            let delta = match self.state.baselines.get(&id) {
                Some(base) => {
                    let reset = sample.cpu_usec < base.cpu_usec;
                    let cpu_usec = if reset {
                        sample.cpu_usec
                    } else {
                        sample.cpu_usec - base.cpu_usec
                    };
                    let memory_byte_seconds = if base.populated || sample.populated {
                        trapezoid(base.memory_bytes, sample.memory_current, base.at_ms, now_ms)
                    } else {
                        0
                    };
                    Some(CgroupDelta {
                        sample: sample.clone(),
                        window_start_ms: base.at_ms,
                        window_end_ms: now_ms,
                        cpu_usec,
                        memory_byte_seconds,
                        reset,
                    })
                }
                // Warm: the resource appeared since the last scan, and its
                // counters started at zero.
                None if self.state.warm => Some(CgroupDelta {
                    sample: sample.clone(),
                    window_start_ms: self.state.last_scan_ms.unwrap_or(now_ms).min(now_ms),
                    window_end_ms: now_ms,
                    cpu_usec: sample.cpu_usec,
                    memory_byte_seconds: 0,
                    reset: false,
                }),
                // Cold: its usage before now is unknown to us; skip it.
                None => None,
            };
            if let Some(delta) = delta
                && (delta.cpu_usec > 0 || delta.memory_byte_seconds > 0)
            {
                deltas.push(delta);
            }
            next.insert(
                id,
                Baseline {
                    cpu_usec: sample.cpu_usec,
                    memory_bytes: sample.memory_current,
                    populated: sample.populated,
                    at_ms: now_ms,
                },
            );
        }
        Observation {
            deltas,
            next,
            at_ms: now_ms,
        }
    }

    /// Adopts `observation`'s baselines. Resources missing from its scan are
    /// dropped. The tracker is warm from here on.
    pub fn commit(&mut self, observation: Observation) {
        self.state.baselines = observation.next;
        self.state.last_scan_ms = Some(observation.at_ms);
        self.state.warm = true;
    }

    /// Advances baselines from an event that reached the spool after the
    /// saved state was written (crash recovery, design 01 §6.2), so it is
    /// not emitted a second time under a new id.
    pub fn replay(&mut self, event: &MeterEvent) {
        let data = &event.data;
        let Some(counter) = data.counter else {
            return;
        };
        let at_ms = millis(data.window_end);
        let base = self
            .state
            .baselines
            .entry(data.resource.clone())
            .or_insert(Baseline {
                cpu_usec: 0,
                memory_bytes: None,
                populated: true,
                at_ms,
            });
        match data.meter {
            Meter::CpuUsec => base.cpu_usec = counter,
            Meter::MemoryByteSeconds => base.memory_bytes = Some(counter),
            _ => return,
        }
        base.at_ms = base.at_ms.max(at_ms);
        self.state.warm = true;
        self.state.last_scan_ms = Some(self.state.last_scan_ms.map_or(at_ms, |t| t.max(at_ms)));
    }
}

/// Byte-seconds between two gauge readings, as a trapezoid. A missing end
/// counts as the other end; both missing is zero.
fn trapezoid(start: Option<u64>, end: Option<u64>, start_ms: i64, end_ms: i64) -> u64 {
    let (a, b) = match (start, end) {
        (Some(a), Some(b)) => (a, b),
        (Some(a), None) => (a, a),
        (None, Some(b)) => (b, b),
        (None, None) => return 0,
    };
    let dt_ms = u128::try_from(end_ms.saturating_sub(start_ms)).unwrap_or(0);
    let area = (u128::from(a) + u128::from(b)) * dt_ms / 2000;
    u64::try_from(area).unwrap_or(u64::MAX)
}

fn millis(t: time::OffsetDateTime) -> i64 {
    i64::try_from(t.unix_timestamp_nanos() / 1_000_000).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(path: &str, inode: u64, cpu: u64, mem: u64, populated: bool) -> CgroupSample {
        CgroupSample {
            path: path.into(),
            inode,
            cpu_usec: cpu,
            memory_current: Some(mem),
            memory_peak: None,
            populated,
        }
    }

    fn step(t: &mut DeltaTracker, samples: &[CgroupSample], now: i64) -> Vec<CgroupDelta> {
        let obs = t.observe(samples, now);
        let deltas = obs.deltas.clone();
        t.commit(obs);
        deltas
    }

    #[test]
    fn cold_start_baselines_without_emitting() {
        let mut t = DeltaTracker::new();
        assert!(step(&mut t, &[sample("a", 1, 5_000, 0, true)], 1_000).is_empty());
        let d = step(&mut t, &[sample("a", 1, 5_700, 0, true)], 2_000);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].cpu_usec, 700);
        assert_eq!((d[0].window_start_ms, d[0].window_end_ms), (1_000, 2_000));
    }

    #[test]
    fn warm_tracker_counts_a_new_cgroup_from_zero() {
        let mut t = DeltaTracker::new();
        step(&mut t, &[], 1_000);
        let d = step(&mut t, &[sample("a", 1, 300, 0, true)], 2_000);
        assert_eq!(d[0].cpu_usec, 300);
        assert_eq!(d[0].window_start_ms, 1_000);
    }

    #[test]
    fn missed_samples_lose_nothing() {
        let mut t = DeltaTracker::new();
        step(&mut t, &[sample("a", 1, 100, 0, true)], 1_000);
        // Two scans skipped; the counter carries the usage.
        let d = step(&mut t, &[sample("a", 1, 1_100, 0, true)], 4_000);
        assert_eq!(d[0].cpu_usec, 1_000);
        assert_eq!(d[0].window_start_ms, 1_000);
    }

    #[test]
    fn a_decrease_is_a_reset() {
        let mut t = DeltaTracker::new();
        step(&mut t, &[sample("a", 1, 10_000, 0, true)], 1_000);
        let d = step(&mut t, &[sample("a", 1, 400, 0, true)], 2_000);
        assert_eq!(d[0].cpu_usec, 400);
        assert!(d[0].reset);
    }

    #[test]
    fn recreated_cgroup_is_a_new_resource() {
        let mut t = DeltaTracker::new();
        step(&mut t, &[sample("a", 1, 10_000, 0, true)], 1_000);
        // Same path, new inode: a new lifetime starting at zero.
        let d = step(&mut t, &[sample("a", 2, 250, 0, true)], 2_000);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].cpu_usec, 250);
        assert!(!d[0].reset);
        assert_eq!(t.state().baselines.len(), 1);
    }

    #[test]
    fn removed_cgroups_are_dropped() {
        let mut t = DeltaTracker::new();
        step(
            &mut t,
            &[sample("a", 1, 1, 0, true), sample("b", 2, 1, 0, true)],
            1_000,
        );
        step(&mut t, &[sample("a", 1, 1, 0, true)], 2_000);
        assert_eq!(t.state().baselines.len(), 1);
    }

    #[test]
    fn uncommitted_observations_carry_usage_forward() {
        let mut t = DeltaTracker::new();
        step(&mut t, &[sample("a", 1, 100, 0, true)], 1_000);
        // The spool refused this one: no commit.
        let refused = t.observe(&[sample("a", 1, 300, 0, true)], 2_000);
        assert_eq!(refused.deltas[0].cpu_usec, 200);
        let d = step(&mut t, &[sample("a", 1, 450, 0, true)], 3_000);
        assert_eq!(d[0].cpu_usec, 350);
        assert_eq!(d[0].window_start_ms, 1_000);
    }

    #[test]
    fn memory_integrates_only_while_populated() {
        let mut t = DeltaTracker::new();
        step(&mut t, &[sample("a", 1, 0, 1_000, true)], 0);
        // 1 000 → 3 000 bytes over 2 s: 4 000 byte-seconds.
        let d = step(&mut t, &[sample("a", 1, 0, 3_000, true)], 2_000);
        assert_eq!(d[0].memory_byte_seconds, 4_000);
        // The last process exits during the window: still counted once.
        let d = step(&mut t, &[sample("a", 1, 0, 3_000, false)], 3_000);
        assert_eq!(d[0].memory_byte_seconds, 3_000);
        // Empty cgroup holding page cache: not billed.
        assert!(step(&mut t, &[sample("a", 1, 0, 3_000, false)], 4_000).is_empty());
    }

    #[test]
    fn zero_deltas_are_not_emitted() {
        let mut t = DeltaTracker::new();
        step(&mut t, &[sample("a", 1, 7, 0, false)], 1_000);
        assert!(step(&mut t, &[sample("a", 1, 7, 0, false)], 2_000).is_empty());
    }

    #[test]
    fn clock_going_backwards_bills_no_memory() {
        let mut t = DeltaTracker::new();
        step(&mut t, &[sample("a", 1, 0, 1_000, true)], 5_000);
        assert!(step(&mut t, &[sample("a", 1, 0, 1_000, true)], 4_000).is_empty());
    }
}
