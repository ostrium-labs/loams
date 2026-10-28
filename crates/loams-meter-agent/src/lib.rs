//! `loams-meter-agent`: Loams' node metering agent (design §63 §21, from
//! platform design 01, MB0).
//!
//! The agent runs once per node (a DaemonSet), outside every tenant sandbox.
//! Each tick it reads cgroup v2 counters ([`cgroup`]), turns them into
//! per-window deltas ([`delta`]), resolves the tenant ([`resolver`]), writes
//! CloudEvents meter events ([`event`]) to its on-disk spool ([`spool`]) and
//! only then advances its baselines. A forwarder drains the spool into the
//! Loams stream `_system/_meter` ([`stream`], through the [`sink::Sink`]
//! trait) as usage events in the Loams usage profile ([`usage`], §63 §6). Every event carries the idempotency key `<node>/<source>/<seq>`,
//! so retries and replays never double-count.
//!
//! The T1 source ([`host`]): wasmtime hosts report each invocation on a
//! node-local socket, and the agent splits the shared host cgroup's CPU by
//! those reports, never past the cgroup total.
//!
//! On Kubernetes, [`pods`] resolves pod cgroups from the node's pods'
//! `loams.dev/*` labels, and [`final_read`] scans as soon as a cgroup's
//! `cgroup.events` says it is empty, before the kubelet removes it.

pub mod agent;
pub mod cgroup;
pub mod delta;
pub mod error;
pub mod event;
pub mod final_read;
pub mod host;
pub mod pods;
pub mod resolver;
pub mod sink;
pub mod spool;
pub mod stream;
pub mod usage;

pub use agent::{Agent, TickReport};
pub use cgroup::{CgroupReader, CgroupSample};
pub use delta::DeltaTracker;
pub use error::MeterError;
pub use event::{Attribution, Measurement, Meter, MeterEvent, SourceKind, Tier};
pub use resolver::{StaticResolver, TenantResolver};
pub use sink::{MemorySink, Sink, forward};
pub use spool::{Spool, SpoolConfig};
pub use stream::{StreamSink, StreamSinkConfig};
