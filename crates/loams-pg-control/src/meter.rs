//! Usage events from pg-control (design §63 §6.3, ME1 Task 3): a
//! [`ComputeLifecycleObserver`] that feeds a
//! [`ComputeTicker`](loams_meter_gate::adapters::ComputeTicker) and emits one
//! `io.loams.dev.usage.postgres.compute.v1` event per running compute per UTC
//! minute and at its end, with `cu_ms` (compute units × ms).
//!
//! Ids are `<endpoint>/<compute>/<minute>` (§63 §6.1, D2182): a minute
//! re-emitted after a restart has the id it had before, so the stream's
//! dedupe drops it. Call [`MeterLifecycle::run_ticker`] on the runtime to
//! close minutes while computes run; lifecycle events close them too.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use loams_meter_gate::adapters::{ComputeTick, ComputeTicker, Emitter};
use loams_meter_gate::event::{UsageEvent, types};

use crate::observer::{
    ComputeLifecycleEvent, ComputeLifecycleObserver, DefaultObserver, LifecycleKind,
};

/// How often the ticker closes minutes by default.
pub const TICK_INTERVAL: Duration = Duration::from_secs(60);

/// Emits compute time; passes every event on to `inner`.
pub struct MeterLifecycle {
    emitter: Arc<Emitter>,
    ticker: Mutex<ComputeTicker>,
    inner: Arc<dyn ComputeLifecycleObserver>,
}

impl MeterLifecycle {
    /// Events through `emitter`, counting into a [`DefaultObserver`] as well.
    pub fn new(emitter: Arc<Emitter>) -> Self {
        Self::wrapping(emitter, Arc::new(DefaultObserver::default()))
    }

    pub fn wrapping(emitter: Arc<Emitter>, inner: Arc<dyn ComputeLifecycleObserver>) -> Self {
        Self {
            emitter,
            ticker: Mutex::new(ComputeTicker::new()),
            inner,
        }
    }

    /// Closes every minute that ended by `now` and emits it.
    pub fn tick(&self, now: SystemTime) {
        let ticks = self.ticker().tick(ms(now));
        self.send(ticks);
    }

    /// Ticks every `interval` until the task is dropped.
    pub async fn run_ticker(self: Arc<Self>, interval: Duration) {
        let mut every = tokio::time::interval(interval);
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            every.tick().await;
            self.tick(SystemTime::now());
        }
    }

    fn ticker(&self) -> MutexGuard<'_, ComputeTicker> {
        self.ticker.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn send(&self, ticks: Vec<ComputeTick>) {
        for t in ticks {
            let mut ev = UsageEvent::new(types::POSTGRES_COMPUTE, t.id())
                .at_ns(t.end_ms.saturating_mul(1_000_000))
                .num("cu_ms", t.cu_ms)
                .text("endpoint", &t.endpoint)
                .text("compute", &t.compute);
            for (k, v) in &t.dims {
                ev = ev.text(k, v);
            }
            self.emitter.emit_attributed(&t.attribution, ev);
        }
    }
}

impl std::fmt::Debug for MeterLifecycle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeterLifecycle")
            .field("source", &self.emitter.source())
            .finish_non_exhaustive()
    }
}

impl ComputeLifecycleObserver for MeterLifecycle {
    fn on_event(&self, e: &ComputeLifecycleEvent) {
        let endpoint = e.endpoint.to_string();
        let compute = e.compute.to_string();
        let at = ms(e.at);
        let ticks = match &e.event {
            LifecycleKind::Started { cu } => {
                let org = (!e.org.is_empty()).then_some(e.org.as_str());
                // `resolve` counts a compute it cannot attribute.
                if let Some(attribution) = self.emitter.resolve(org, &e.namespace) {
                    let dims = vec![
                        ("namespace".to_owned(), e.namespace.clone()),
                        ("project".to_owned(), e.project.to_string()),
                    ];
                    self.ticker()
                        .start(&endpoint, &compute, attribution, dims, cu.0, at);
                }
                Vec::new()
            }
            LifecycleKind::Resized { cu } => self.ticker().resize(&endpoint, &compute, cu.0, at),
            LifecycleKind::Suspended | LifecycleKind::Failed { .. } => {
                self.ticker().stop(&endpoint, &compute, at)
            }
        };
        self.send(ticks);
        self.inner.on_event(e);
    }
}

fn ms(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{ComputeId, EndpointId, ProjectId};
    use crate::model::Cu;
    use loams_meter_gate::adapters::{DeploymentOrg, source_uri};
    use loams_meter_gate::sink::MemorySink;

    fn event(kind: LifecycleKind, at_ms: u64) -> ComputeLifecycleEvent {
        ComputeLifecycleEvent {
            org: "42".into(),
            namespace: "7".into(),
            project: ProjectId::new(),
            endpoint: EndpointId::new(),
            compute: ComputeId::new(),
            event: kind,
            at: UNIX_EPOCH + Duration::from_millis(at_ms),
        }
    }

    #[test]
    fn compute_minutes_become_usage_events() {
        let sink = Arc::new(MemorySink::default());
        let emitter = Arc::new(Emitter::new(
            sink.clone(),
            Arc::new(DeploymentOrg::default()),
            source_uri("local", "pg-control", "n1"),
        ));
        let m = MeterLifecycle::new(emitter);
        let started = event(LifecycleKind::Started { cu: Cu(4) }, 60_000);
        m.on_event(&started);
        m.tick(UNIX_EPOCH + Duration::from_millis(180_000));
        let suspended = ComputeLifecycleEvent {
            event: LifecycleKind::Suspended,
            at: UNIX_EPOCH + Duration::from_millis(190_000),
            ..started.clone()
        };
        m.on_event(&suspended);
        let events = sink.events();
        let cu: Vec<u64> = events
            .iter()
            .map(|e| e.data["cu_ms"].as_u64().unwrap())
            .collect();
        assert_eq!(cu, [60_000, 60_000, 10_000]);
        let endpoint = started.endpoint.to_string();
        let compute = started.compute.to_string();
        assert_eq!(events[0].id, format!("{endpoint}/{compute}/1"));
        assert_eq!(events[2].id, format!("{endpoint}/{compute}/3"));
        assert!(events.iter().all(|e| e.subject == "org_42"));
        events[0].validate().unwrap();
    }

    #[test]
    fn hard_stop_refuses_wake_with_resource_exhausted() {
        use crate::service::{Reason, entitlement_error};
        use loams_meter_gate::gate::{Refusal, RefusalReason};
        use std::time::Duration;

        let e = entitlement_error(&Refusal::new(
            RefusalReason::EntitlementExhausted,
            "pg_compute",
        ));
        assert_eq!(e.reason, Reason::ResourceExhausted);
        assert_eq!(e.metadata["entitlement"], "pg_compute");
        assert_eq!(e.metadata["reason"], "entitlement_exhausted");
        let e = entitlement_error(&Refusal::new(
            RefusalReason::PaymentMethodRequired,
            "pg_compute",
        ));
        assert_eq!(e.reason, Reason::FailedPrecondition);
        let e = entitlement_error(
            &Refusal::new(RefusalReason::MeterUnavailable, "pg_compute")
                .retry_after(Duration::from_secs(1)),
        );
        assert_eq!(e.reason, Reason::Unavailable);
        assert_eq!(e.metadata["retry_after_s"], "1");
    }
}
