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
use crate::event::{
    Attribution, Measurement, Meter, MeterEvent, PLATFORM_ORG, SourceKind, Tier, time_from_unix_ms,
};
use crate::host::proto::Invocation;
use crate::host::reconcile::{DEFAULT_TOLERANCE_BP, HostLedger, SplitKind, TenantKey};
use crate::resolver::TenantResolver;
use crate::spool::{Spool, sync_dir, write_synced};

#[derive(Debug, Default, Serialize, Deserialize)]
struct AgentState {
    next_seq: u64,
    tracker: TrackerState,
    /// Host-reported CPU not yet reconciled (T1, crate::host).
    #[serde(default)]
    host: HostLedger,
}

/// The last path component of the shared T1 host cgroup.
pub const WASM_HOST_SCOPE: &str = "wasm-host.scope";

/// Whether the cgroup at `path` is the shared T1 host.
pub fn is_wasm_host(path: &str) -> bool {
    path.rsplit('/').next() == Some(WASM_HOST_SCOPE)
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
    host_ledger: HostLedger,
    host_reports: bool,
    tolerance_bp: u64,
    scanned: Vec<String>,
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
        let (tracker, next_seq, host_ledger) = match load_state(&state_path)? {
            Some(state) => {
                let mut tracker = DeltaTracker::from_state(state.tracker);
                let mut host = state.host;
                for event in &spooled {
                    if event.data.node == node && event.data.seq >= state.next_seq {
                        tracker.replay(event);
                        // A host report spooled after the state was saved.
                        if event.data.meter == Meter::WasmFuel {
                            host.add_cpu(
                                TenantKey {
                                    org: event.data.org.clone(),
                                    namespace: event.data.namespace.clone(),
                                    function: event.data.function.clone().unwrap_or_default(),
                                },
                                event.data.cpu_usec.unwrap_or(0),
                            );
                        }
                    }
                }
                (tracker, state.next_seq.max(max_spooled), host)
            }
            None => {
                let fresh = u64::try_from(now_ms).unwrap_or(0).saturating_mul(1_000);
                (
                    DeltaTracker::new(),
                    fresh.max(max_spooled),
                    HostLedger::default(),
                )
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
            host_ledger,
            host_reports: false,
            tolerance_bp: DEFAULT_TOLERANCE_BP,
            scanned: Vec::new(),
        })
    }

    /// Turns on the T1 source: the CPU of the shared wasmtime host cgroup is
    /// split by the host reports ([`Agent::record_host_report`]) instead of
    /// being attributed to the cgroup's resolved tenant.
    pub fn set_host_reports(&mut self, enabled: bool) {
        self.host_reports = enabled;
    }

    /// Spools one host report's invocations as
    /// `io.loams.dev.usage.function.fuel.v1` events and adds their CPU to
    /// the window being reconciled. `Ok` means durable: the caller may ack
    /// the report. The event ids derive from the work
    /// (`<node>/wasm/<host_id>:<seq>:<index>`), so a report the host resends
    /// after an agent crash deduplicates downstream.
    pub fn record_host_report(
        &mut self,
        host_id: &str,
        report_seq: u64,
        invocations: &[Invocation],
    ) -> Result<usize, MeterError> {
        let mut events = Vec::with_capacity(invocations.len());
        let mut cleaned = Vec::with_capacity(invocations.len());
        for (index, inv) in invocations.iter().enumerate() {
            let mut inv = inv.clone();
            if inv.org.is_empty() || inv.namespace.is_empty() {
                inv.org = PLATFORM_ORG.to_string();
                inv.namespace = PLATFORM_ORG.to_string();
            }
            let measurement = Measurement {
                attribution: Attribution {
                    org: inv.org.clone(),
                    namespace: inv.namespace.clone(),
                    function: (!inv.function.is_empty()).then(|| inv.function.clone()),
                    tier: Some(Tier::T1),
                },
                source: SourceKind::Wasm,
                meter: Meter::WasmFuel,
                value: inv.fuel,
                window_start: time_from_unix_ms(inv.start_unix_ms),
                window_end: time_from_unix_ms(inv.end_unix_ms),
                resource: format!("{host_id}/{}", inv.invocation_id),
                counter: None,
                peak_bytes: None,
                cpu_usec: Some(inv.cpu_usec),
                estimated: inv.cpu_estimated,
            };
            let mut event = measurement.into_event(&self.node, self.next_seq + index as u64);
            event.id = format!("{}/wasm/{host_id}:{report_seq}:{index}", self.node);
            events.push(event);
            cleaned.push(inv);
        }
        self.spool.append(&events)?;
        self.next_seq += events.len() as u64;
        self.host_ledger.add(&cleaned);
        self.save_state()?;
        Ok(events.len())
    }

    /// Records a host's gap: the window being reconciled is incomplete and
    /// is split by the last complete window's shares. `Ok` means durable.
    pub fn record_gap(&mut self) -> Result<(), MeterError> {
        self.host_ledger.gap();
        self.save_state()
    }

    /// Scans once at `now_ms` and spools the resulting events.
    pub fn tick(&mut self, now_ms: i64) -> Result<TickReport, MeterError> {
        let scan = self.reader.scan_lenient()?;
        for (path, e) in &scan.skipped {
            warn!(cgroup = %path, error = %e, "cgroup unreadable; skipped this tick");
        }
        let samples = scan.samples;
        self.scanned = samples.iter().map(|s| s.path.clone()).collect();
        let skipped: Vec<String> = scan.skipped.into_iter().map(|(p, _)| p).collect();
        let mut observation = self.tracker.observe(&samples, now_ms);
        self.tracker.carry(&mut observation, &skipped);
        let node = self.node.as_str();
        let base_seq = self.next_seq;
        let split_host = self.host_reports;
        let tolerance_bp = self.tolerance_bp;
        // Committed with the baselines, only once the events are spooled.
        let mut ledger = self.host_ledger.clone();
        let mut events: Vec<MeterEvent> = Vec::new();
        for delta in &observation.deltas {
            let attribution = self
                .resolver
                .resolve(&delta.sample)
                .unwrap_or_else(Attribution::platform);
            let resource = delta.sample.resource_id();
            let window_start = time_from_unix_ms(delta.window_start_ms);
            let window_end = time_from_unix_ms(delta.window_end_ms);
            let push = |events: &mut Vec<MeterEvent>,
                        attribution: Attribution,
                        meter: Meter,
                        value: u64,
                        counter: Option<u64>,
                        peak_bytes: Option<u64>,
                        estimated: bool| {
                let measurement = Measurement {
                    attribution,
                    source: SourceKind::Cgroup,
                    meter,
                    value,
                    window_start,
                    window_end,
                    resource: resource.clone(),
                    counter,
                    peak_bytes,
                    cpu_usec: None,
                    estimated,
                };
                let seq = base_seq + events.len() as u64;
                events.push(measurement.into_event(node, seq));
            };
            let counter = Some(delta.sample.cpu_usec);
            if delta.cpu_usec > 0 && split_host && is_wasm_host(&delta.sample.path) {
                // T1: the host cgroup's CPU, split by the host reports; the
                // tenants' sum never exceeds it (crate::host::reconcile).
                let split = ledger.reconcile(delta.cpu_usec, tolerance_bp);
                if matches!(split.kind, SplitKind::Scaled | SplitKind::PreviousShares) {
                    warn!(
                        total = delta.cpu_usec,
                        kind = ?split.kind,
                        "host reports disagree with the host cgroup; total apportioned"
                    );
                }
                let estimated = split.kind.estimated();
                for (tenant, cpu) in &split.tenants {
                    let tenant_attribution = Attribution {
                        org: tenant.org.clone(),
                        namespace: tenant.namespace.clone(),
                        function: (!tenant.function.is_empty()).then(|| tenant.function.clone()),
                        tier: Some(Tier::T1),
                    };
                    push(
                        &mut events,
                        tenant_attribution,
                        Meter::CpuUsec,
                        *cpu,
                        counter,
                        None,
                        estimated,
                    );
                }
                if split.overhead > 0 {
                    push(
                        &mut events,
                        Attribution::platform(),
                        Meter::CpuUsec,
                        split.overhead,
                        counter,
                        None,
                        false,
                    );
                }
            } else if delta.cpu_usec > 0 {
                push(
                    &mut events,
                    attribution.clone(),
                    Meter::CpuUsec,
                    delta.cpu_usec,
                    counter,
                    None,
                    false,
                );
            }
            if delta.memory_byte_seconds > 0 {
                push(
                    &mut events,
                    attribution.clone(),
                    Meter::MemoryByteSeconds,
                    delta.memory_byte_seconds,
                    delta.sample.memory_current,
                    delta.sample.memory_peak,
                    false,
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
        self.host_ledger = ledger;
        self.save_state()?;
        Ok(report)
    }

    /// The target cgroups the last scan read (paths relative to the cgroup
    /// root), for the final-read watches ([`crate::final_read`]).
    pub fn scanned(&self) -> &[String] {
        &self.scanned
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
            host: self.host_ledger.clone(),
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
