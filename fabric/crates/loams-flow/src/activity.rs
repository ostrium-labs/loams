//! The connector activity observer (CN2 plan Task 24, design §33 Q359).
//!
//! Once per window (default 60 s), every registered instance and direction is
//! handed to a [`ConnectorActivityObserver`] as a [`ConnectorActivity`]: how
//! many events, bytes and external calls it handled in that window. This is
//! open observability, not metering (D548–D552): the record is a plain struct
//! passed to a method call. It has no wire format (no serialiser is derived or
//! linked here), and this module opens no socket or file, keeps no buffer of
//! records and persists nothing; `activity_observer_has_no_wire_format` holds
//! that. This repository ships [`NoActivityObserver`] only; a billing observer
//! comes with the metering code moving into the open repo now (§63, D1119,
//! which supersedes the open-core reason for D548–D552).
//!
//! The counters come from [`ConnectorMetrics`]: the SDK's
//! `ConnectorObserver` hook feeds them for native connectors, and
//! `InstanceMetrics::apply_scrape` for the fallback runtimes.

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};

use crate::manifest::RuntimeKind;
use crate::metrics::{ConnectorMetrics, InstanceMetrics};
use crate::validate::Direction;

/// The activity window's default length.
pub const DEFAULT_ACTIVITY_WINDOW: Duration = Duration::from_secs(60);

/// One instance and direction's activity in one window. A plain struct with
/// no wire format (D548).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorActivity {
    pub namespace: u64,
    pub instance: String,
    pub connector: String,
    pub runtime: RuntimeKind,
    /// The instance runs on its manifest's fallback runtime (OR-4).
    pub fallback: bool,
    pub direction: Direction,
    /// Events the Fabric (source) or the target (sink) acknowledged.
    pub events: u64,
    /// Bytes read from the external system.
    pub bytes_in: u64,
    /// Bytes written to the external system.
    pub bytes_out: u64,
    /// Calls to the external system.
    pub api_calls: u64,
    pub window_end: SystemTime,
}

/// Receives each instance's activity once per window. Implementations must
/// return quickly: they are called on the activity timer's task.
pub trait ConnectorActivityObserver: Send + Sync + 'static {
    fn on_activity(&self, a: &ConnectorActivity);
}

/// Observes nothing: the open-source default.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoActivityObserver;

impl ConnectorActivityObserver for NoActivityObserver {
    fn on_activity(&self, _a: &ConnectorActivity) {}
}

/// Hands every instance's window to the observer, on a timer
/// ([`ActivityPump::spawn`]) or when called ([`ActivityPump::tick`]).
#[derive(Clone)]
pub struct ActivityPump {
    metrics: ConnectorMetrics,
    observer: Arc<dyn ConnectorActivityObserver>,
}

impl fmt::Debug for ActivityPump {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ActivityPump").finish_non_exhaustive()
    }
}

impl ActivityPump {
    pub fn new(metrics: ConnectorMetrics, observer: Arc<dyn ConnectorActivityObserver>) -> Self {
        Self { metrics, observer }
    }

    /// End the current window at `window_end`: one call per registered
    /// instance and direction (with zeros when it was idle), and one last
    /// call for each instance deregistered since the previous window.
    pub fn tick(&self, window_end: SystemTime) {
        for m in self.metrics.take_retired() {
            self.report(&m, window_end);
        }
        for m in self.metrics.instances() {
            self.report(&m, window_end);
        }
    }

    fn report(&self, m: &InstanceMetrics, window_end: SystemTime) {
        let t = m.take_window();
        let l = m.labels();
        self.observer.on_activity(&ConnectorActivity {
            namespace: l.namespace(),
            instance: l.instance().to_owned(),
            connector: l.connector().to_owned(),
            runtime: l.runtime(),
            fallback: l.fallback(),
            direction: l.direction(),
            events: t.events,
            bytes_in: t.bytes_in,
            bytes_out: t.bytes_out,
            api_calls: t.api_calls,
            window_end,
        });
    }

    /// Tick every `window` on the current tokio runtime, the first time one
    /// window from now. Abort the handle to stop.
    pub fn spawn(self, window: Duration) -> JoinHandle<()> {
        let window = window.max(Duration::from_millis(1));
        tokio::spawn(async move {
            let mut timer = tokio::time::interval_at(Instant::now() + window, window);
            timer.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                timer.tick().await;
                self.tick(SystemTime::now());
            }
        })
    }
}
