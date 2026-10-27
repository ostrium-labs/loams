//! `loams-meter-agent`: Loams Cloud's node metering agent (design 01, MB0).
//!
//! The agent runs once per node (a DaemonSet), outside every tenant sandbox.
//! Each tick it reads cgroup v2 counters ([`cgroup`]), turns them into
//! per-window deltas ([`delta`]), resolves the tenant ([`resolver`]), writes
//! CloudEvents meter events ([`event`]) to its on-disk spool ([`spool`]) and
//! only then advances its baselines. A forwarder drains the spool into the
//! Loams stream `_system/_meter` ([`stream`], through the [`sink::Sink`]
//! trait). Every event carries the idempotency key `<node>/<source>/<seq>`,
//! so retries and replays never double-count.
//!
//! Not here yet: the kube resolver and inotify final reads (MB2), Envoy ALS
//! (MB3), the wasmtime host socket (MB4) and the aggregator (MB1).

pub mod agent;
pub mod cgroup;
pub mod delta;
pub mod error;
pub mod event;
pub mod resolver;
pub mod sink;
pub mod spool;
pub mod stream;

pub use agent::{Agent, TickReport};
pub use cgroup::{CgroupReader, CgroupSample};
pub use delta::DeltaTracker;
pub use error::MeterError;
pub use event::{Attribution, Measurement, Meter, MeterEvent, SourceKind, Tier};
pub use resolver::{StaticResolver, TenantResolver};
pub use sink::{MemorySink, Sink, forward};
pub use spool::{Spool, SpoolConfig};
pub use stream::{StreamSink, StreamSinkConfig};
