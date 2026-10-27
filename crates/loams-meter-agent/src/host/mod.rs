//! The T1 source: per-invocation host reports from wasmtime hosts (design
//! §63 §21, ME1 Task 29; platform doc 06 PD61–PD64).
//!
//! T1 runs many tenants in one shared host process, so the host's cgroup
//! (`loams.slice/wasm-host.scope`) says how much CPU they used together and
//! the host's reports say how that splits. This module has both ends of the
//! node-local socket and the reconciliation:
//!
//! * [`proto`]: `loams.meter.v1.host`, the messages and their framing;
//! * [`auth`]: `SO_PEERCRED`-bound sessions, per-boot epochs and HMAC keys,
//!   and the ack rules that never cover a `Gap` silently;
//! * [`server`]: the agent's end of the socket;
//! * [`reporter`]: the host's end, which a wasmtime host links;
//! * [`reconcile`]: the per-window split of the cgroup total, which the
//!   tenants' sum never exceeds.
//!
//! Status: preview, not yet run end to end against a wasmtime host.

pub mod auth;
pub mod proto;
pub mod reconcile;
pub mod reporter;
pub mod server;

pub use auth::Sessions;
pub use reconcile::{HostLedger, Split, SplitKind, TenantKey};
pub use reporter::{Reporter, ReporterConfig, ReporterStats};
pub use server::{HostEvent, HostServerConfig};
