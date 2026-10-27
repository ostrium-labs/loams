//! The agent's tick: scan, resolve, delta, spool, commit (design 01 §6.1).
//!
//! State (the baselines and the next sequence number) is saved by rename
//! after each spooled batch. After a crash between the spool append and the
//! state write, [`Agent::open`] replays the spooled events past the saved
//! sequence into the baselines, so nothing is emitted twice under a new id.
//! An agent that finds no state starts cold (baselines at current readings:
//! an undercount, never a double bill) and starts its sequence at the
//! current Unix time in microseconds, above any id it could have used
//! before.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::cgroup::CgroupReader;
use crate::delta::{DeltaTracker, TrackerState};
use crate::error::MeterError;
use crate::event::{Attribution, Measurement, Meter, MeterEvent, SourceKind, time_from_unix_ms};
use crate::resolver::TenantResolver;
use crate::spool::{Spool, sync_dir, write_synced};

#[derive(Debug, Default, Serialize, Deserialize)]
struct AgentState {
    next_seq: u64,
    tracker: TrackerState,
}

/// What one tick did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TickReport {
    /// Cgroups sampled.
    pub resources: usize,
    /// Events spooled.
    pub events: usize,
    /// Whether the spool was full; the usage waits in the counters.
    pub spool_full: bool,
    /// Target cgroups that could not be read or parsed and were skipped;
    /// their usage is counted at their next good reading.
    pub skipped: usize,
}

/// The node metering agent over cgroup v2.
#[derive(Debug)]
pub struct Agent<R> {
    node: String,
    state_path: PathBuf,
    reader: CgroupReader,
    resolver: R,
    tracker: DeltaTracker,
    spool: Spool,
    next_seq: u64,
}

impl<R: TenantResolver> Agent<R> {
    /// Opens the agent for `node`, resuming from `state_path` and `spool`.
    /// `now_ms` seeds the sequence when there is no state.
    pub fn open(
        node: impl Into<String>,
        state_path: impl Into<PathBuf>,
        reader: CgroupReader,
        resolver: R,
        spool: Spool,
        now_ms: i64,
    ) -> Result<Self, MeterError> {
        let node = node.into();
        let state_path = state_path.into();
        let spooled = spool.events()?;
        let max_spooled = spooled
            .iter()
            .filter(|e| e.data.node == node)
            .map(|e| e.data.seq + 1)
            .max()
            .unwrap_or(0);
        let (tracker, next_seq) = match load_state(&state_path)? {
            Some(state) => {
                let mut tracker = DeltaTracker::from_state(state.tracker);
                for event in &spooled {
                    if event.data.node == node && event.data.seq >= state.next_seq {
                        tracker.replay(event);
                    }
                }
                (tracker, state.next_seq.max(max_spooled))
            }
            None => {
                let fresh = u64::try_from(now_ms).unwrap_or(0).saturating_mul(1_000);
                (DeltaTracker::new(), fresh.max(max_spooled))
            }
        };
        Ok(Self {
            node,
            state_path,
            reader,
            resolver,
            tracker,
            spool,
            next_seq,
        })
    }

    /// Scans once at `now_ms` and spools the resulting events.
    pub fn tick(&mut self, now_ms: i64) -> Result<TickReport, MeterError> {
        let scan = self.reader.scan_lenient()?;
        for (path, e) in &scan.skipped {
            warn!(cgroup = %path, error = %e, "cgroup unreadable; skipped this tick");
        }
        let samples = scan.samples;
        let skipped: Vec<String> = scan.skipped.into_iter().map(|(p, _)| p).collect();
        let mut observation = self.tracker.observe(&samples, now_ms);
        self.tracker.carry(&mut observation, &skipped);
        let mut events = Vec::new();
        for delta in &observation.deltas {
            let attribution = self
                .resolver
                .resolve(&delta.sample)
                .unwrap_or_else(Attribution::platform);
            let resource = delta.sample.resource_id();
            let window_start = time_from_unix_ms(delta.window_start_ms);
            let window_end = time_from_unix_ms(delta.window_end_ms);
            let mut push = |meter, value, counter, peak_bytes| {
                let measurement = Measurement {
                    attribution: attribution.clone(),
                    source: SourceKind::Cgroup,
                    meter,
                    value,
                    window_start,
                    window_end,
                    resource: resource.clone(),
                    counter,
                    peak_bytes,
                    cpu_usec: None,
                    estimated: false,
                };
                let seq = self.next_seq + events.len() as u64;
                events.push(measurement.into_event(&self.node, seq));
            };
            if delta.cpu_usec > 0 {
                push(
                    Meter::CpuUsec,
                    delta.cpu_usec,
                    Some(delta.sample.cpu_usec),
                    None,
                );
            }
            if delta.memory_byte_seconds > 0 {
                push(
                    Meter::MemoryByteSeconds,
                    delta.memory_byte_seconds,
                    delta.sample.memory_current,
                    delta.sample.memory_peak,
                );
            }
        }
        let mut report = TickReport {
            resources: samples.len(),
            events: events.len(),
            spool_full: false,
            skipped: skipped.len(),
        };
        match self.spool.append(&events) {
            Ok(_) => {}
            Err(MeterError::SpoolFull { bytes, limit }) => {
                warn!(bytes, limit, "spool full; usage carried to the next tick");
                report.events = 0;
                report.spool_full = true;
                return Ok(report);
            }
            Err(e) => return Err(e),
        }
        self.next_seq += events.len() as u64;
        self.tracker.commit(observation);
        self.save_state()?;
        Ok(report)
    }

    /// The spool, for the forwarder.
    pub fn spool_mut(&mut self) -> &mut Spool {
        &mut self.spool
    }

    /// The node name.
    pub fn node(&self) -> &str {
        &self.node
    }

    /// The sequence number the next event gets.
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    fn save_state(&self) -> Result<(), MeterError> {
        let state = AgentState {
            next_seq: self.next_seq,
            tracker: self.tracker.state().clone(),
        };
        let json = serde_json::to_vec(&state)
            .map_err(|e| MeterError::parse(&self.state_path, e.to_string()))?;
        let tmp = self.state_path.with_extension("tmp");
        write_synced(&tmp, &json)?;
        std::fs::rename(&tmp, &self.state_path).map_err(|e| MeterError::io(&self.state_path, e))?;
        if let Some(dir) = self
            .state_path
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
        {
            sync_dir(dir)?;
        }
        Ok(())
    }
}

/// Loads saved state. A missing file is `None`; so is a corrupt one, with a
/// warning, because starting cold never double-bills.
fn load_state(path: &Path) -> Result<Option<AgentState>, MeterError> {
    match std::fs::read(path) {
        Ok(data) => match serde_json::from_slice(&data) {
            Ok(state) => Ok(Some(state)),
            Err(e) => {
                warn!(path = %path.display(), error = %e, "corrupt state; starting cold");
                Ok(None)
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(MeterError::io(path, e)),
    }
}

/// Events for `meter` in `events`, for tests and dry runs.
pub fn of_meter(events: &[MeterEvent], meter: Meter) -> impl Iterator<Item = &MeterEvent> {
    events.iter().filter(move |e| e.data.meter == meter)
}
