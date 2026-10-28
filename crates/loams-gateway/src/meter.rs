//! Usage events from the gateway (design §63 §6.3, ME1 Task 3): a
//! [`ConnectionObserver`] that turns each closed Postgres connection into an
//! `io.loams.dev.usage.gateway.connection.v1` event, then hands the call on
//! to the observer it wraps.
//!
//! The gateway knows a connection's namespace only; the org comes from the
//! [`SubjectResolver`](loams_meter_gate::adapters::SubjectResolver) the
//! [`Emitter`] was built with. Ids are `<boot_ns>/<seq>` per gateway process
//! ([`Sequence`]), assigned once when the event is built, so the sink's
//! retries keep them.

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use loams_meter_gate::adapters::{Emitter, Sequence};
use loams_meter_gate::event::{UsageEvent, types};

use crate::observe::{ConnectionObserver, NoopObserver, Path};

/// Emits a usage event per closed connection.
pub struct MeterConnectionObserver {
    emitter: Arc<Emitter>,
    seq: Sequence,
    inner: Arc<dyn ConnectionObserver>,
}

impl MeterConnectionObserver {
    /// Events through `emitter`; no other observer.
    pub fn new(emitter: Arc<Emitter>) -> Self {
        Self::wrapping(emitter, Arc::new(NoopObserver))
    }

    /// Events through `emitter`, then every call to `inner`.
    pub fn wrapping(emitter: Arc<Emitter>, inner: Arc<dyn ConnectionObserver>) -> Self {
        Self {
            emitter,
            seq: Sequence::starting_now(),
            inner,
        }
    }

    /// The event for one closed connection (also used by tests).
    pub fn event(&self, bytes_in: u64, bytes_out: u64, duration: Duration) -> UsageEvent {
        UsageEvent::new(types::GATEWAY_CONNECTION, self.seq.next_id())
            .at(SystemTime::now())
            .num("bytes_in", bytes_in)
            .num("bytes_out", bytes_out)
            .num(
                "duration_ms",
                u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
            )
    }
}

impl fmt::Debug for MeterConnectionObserver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MeterConnectionObserver")
            .field("source", &self.emitter.source())
            .finish_non_exhaustive()
    }
}

impl ConnectionObserver for MeterConnectionObserver {
    fn on_open(&self, namespace: &str, path: Path) {
        self.inner.on_open(namespace, path);
    }

    fn on_close(&self, namespace: &str, bytes_in: u64, bytes_out: u64, duration: Duration) {
        let event = self
            .event(bytes_in, bytes_out, duration)
            .text("namespace", namespace);
        self.emitter.emit(None, namespace, event);
        self.inner
            .on_close(namespace, bytes_in, bytes_out, duration);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loams_meter_gate::adapters::{DeploymentOrg, source_uri};
    use loams_meter_gate::sink::MemorySink;

    #[test]
    fn closed_connection_becomes_a_usage_event() {
        let sink = Arc::new(MemorySink::default());
        let emitter = Arc::new(Emitter::new(
            sink.clone(),
            Arc::new(DeploymentOrg::new("42")),
            source_uri("local", "gateway", "n1"),
        ));
        let obs = MeterConnectionObserver::new(emitter);
        obs.on_open("7", Path::Spliced);
        obs.on_close("7", 100, 2_000, Duration::from_millis(1_500));
        obs.on_close("7", 1, 1, Duration::from_millis(1));
        let events = sink.events();
        assert_eq!(events.len(), 2);
        let e = &events[0];
        assert_eq!(e.r#type, types::GATEWAY_CONNECTION);
        assert_eq!(e.subject, "org_42");
        assert_eq!(e.tenant, "42/7");
        assert_eq!(e.data["bytes_in"], 100);
        assert_eq!(e.data["bytes_out"], 2_000);
        assert_eq!(e.data["duration_ms"], 1_500);
        assert_ne!(events[0].id, events[1].id);
        e.validate().unwrap();
    }
}
