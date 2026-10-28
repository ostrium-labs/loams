//! What products link to meter and enforce usage (design [§63] §5.2).
//!
//! - [`event`]: the usage event (§63 §6), built by a producer and encoded once
//!   as a structured-mode CloudEvent in the Loams usage profile.
//! - [`sink`]: [`MeterSink`](sink::MeterSink), the emitter. Best-effort mode is
//!   D365's batched plain produce (bounded queue, 50 ms or 1 MiB flush, oldest
//!   dropped and counted); spooled mode appends to the node agent's spool before
//!   a caller advances its baselines (§63 §7.2).
//! - [`gate`]: [`Gate`](gate::Gate) (`check`, `reserve`, `settle`), the
//!   leasing [`LeaseGate`](gate::LeaseGate) and [`NoGate`](gate::NoGate)
//!   (§63 §10, §11).
//!
//! **The dependency runs one way** (D202, D551): products depend on this crate's
//! traits and nothing here names a product. With [`sink::NoSink`] a product
//! emits nothing and behaves exactly as before.
//!
//! **Amounts are integers** in each feature's base unit (D2180): bytes,
//! milliseconds, tokens, requests. The counters and leases in this crate are
//! atomics of `u64`/`i64`; `loams-meter-core`'s decimal `Amount` converts at
//! the lease boundary, never on the request path.
//!
//! [§63]: ../../../docs/design/63-loams-meter.md

pub mod adapters;
pub mod entitle;
pub mod event;
pub mod gate;
pub mod policy;
pub mod sink;

pub use entitle::{Entitlements, features};
pub use event::{EncodedEvent, UsageEvent};
pub use gate::{Decision, Gate, NoGate, Refusal, RefusalReason};
pub use sink::{MeterSink, NoSink};
