//! `loams_sink` (FL1 Task 10, design §32 §5.10, D1796–D1798): the Iggy to
//! Loams hop.
//!
//! The `kafka` compose profile runs Ostrium's fork of Apache Iggy with its
//! Kafka gateway, which stores each Kafka record as one Iggy message (the
//! fork's `gateways/kafka/docs/BRIDGE_MAPPING.md`). This crate reads those
//! messages, one task per Iggy partition, and appends each one to a Loams
//! stream as a [`CloudEvent`](loams_cloudevents::CloudEvent):
//!
//! - **At least once.** A partition's offset is committed to Iggy (a named
//!   consumer, [`SinkConfig::consumer`]) only after the [`Target`] has
//!   acknowledged every message up to it, so a crash replays from the last
//!   committed offset and never skips one.
//! - **Idempotent.** Each event's `source` names the Iggy stream, topic and
//!   partition and its `id` is the message's offset, so `source` + `id` is
//!   unique per message and the engine's CloudEvents ledger (D270) answers a
//!   replay inside its dedup window as `duplicate` instead of appending it
//!   again ([`mapping`]).
//! - **Backpressure.** Polling is pull-based with one batch in flight per
//!   partition: while the target refuses a batch ([`TargetError`]) the
//!   partition waits (the advertised `Retry-After`, else an exponential
//!   backoff) and polls nothing more.
//! - **Restart.** A restarted sink resumes each partition from its committed
//!   offset; a topic created later is picked up by the next discovery pass.
//! - **Metrics.** [`SinkMetrics`] counts per partition and in total, and
//!   renders as JSON for the engine's status route.
//!
//! The Iggy side is the [`Source`] trait, implemented over the Iggy Rust SDK
//! by [`IggySource`]; the Loams side is [`Target`], which the engine
//! implements over its own CloudEvents ingest. [`memory`] has in-memory
//! versions of both for tests.

pub mod iggy_source;
pub mod mapping;
pub mod memory;
pub mod metrics;
pub mod runner;
pub mod source;
pub mod target;

pub use iggy_source::{IggySource, IggySourceConfig};
pub use mapping::{EVENT_TYPE_IGGY, EVENT_TYPE_KAFKA, EVENT_TYPE_UNDECODABLE, to_event};
pub use metrics::{PartitionMetrics, SinkMetrics};
pub use runner::{Sink, SinkConfig, SinkHandle};
pub use source::{Header, RawMessage, Source, SourceError, TopicInfo};
pub use target::{Target, TargetError, WriteOutcome};
