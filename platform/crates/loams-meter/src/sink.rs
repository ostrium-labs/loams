//! Where meter events go (design 01 §6).
//!
//! A [`Sink`] durably takes events in order. The agent's own sink is the
//! [`Spool`](crate::spool::Spool); [`forward`] drains the spool into a
//! downstream sink such as [`StreamSink`](crate::stream::StreamSink). The
//! same event may reach a downstream sink more than once (a crash between
//! its acknowledgement and the spool's), always with the same `id`, and the
//! consumer dedupes on it.

use async_trait::async_trait;

use crate::error::MeterError;
use crate::event::MeterEvent;
use crate::spool::Spool;

/// Takes meter events.
#[async_trait]
pub trait Sink: Send {
    /// Takes `events`, in order. `Ok` means the sink holds them durably.
    /// [`MeterError::SinkUnavailable`] asks for a retry later.
    async fn send(&mut self, events: &[MeterEvent]) -> Result<(), MeterError>;
}

/// Sends up to `max` of the spool's unacknowledged events to `sink`, then
/// acknowledges them. Returns how many were forwarded; `0` when the spool
/// is drained.
pub async fn forward(
    spool: &mut Spool,
    sink: &mut dyn Sink,
    max: usize,
) -> Result<usize, MeterError> {
    let pending = spool.pending(max)?;
    let Some(&(last, _)) = pending.last() else {
        return Ok(0);
    };
    let events: Vec<MeterEvent> = pending.into_iter().map(|(_, e)| e).collect();
    sink.send(&events).await?;
    spool.ack(last)?;
    Ok(events.len())
}

/// A sink that keeps events in memory, for tests and dry runs.
#[derive(Debug, Default)]
pub struct MemorySink {
    /// Everything sent, in order.
    pub events: Vec<MeterEvent>,
    /// When set, the next `send` fails with [`MeterError::SinkUnavailable`]
    /// and clears it.
    pub fail_next: bool,
}

#[async_trait]
impl Sink for MemorySink {
    async fn send(&mut self, events: &[MeterEvent]) -> Result<(), MeterError> {
        if std::mem::take(&mut self.fail_next) {
            return Err(MeterError::SinkUnavailable("injected".into()));
        }
        self.events.extend_from_slice(events);
        Ok(())
    }
}
