//! Loams Meter's `meter` role (design §63 §5.2): the part of the meter that
//! runs as a service. The sans-I/O types, the balance engine and the lease
//! allocator are in `loams-meter-core`; what products link is
//! `loams-meter-gate`.
//!
//! | Module | Task | Does |
//! |---|---|---|
//! | [`owner`] | ME1 Task 12 | Counter owners: who owns which `_system/_meter` partition (fenced leases), per-partition counters with offset buckets and recent-keys dedup, the rebuild from the House and the stream tail |
//!
//! The House tables and the close job (ME1 Tasks 5–8) and the exporters
//! (Tasks 24–26) live beside these in their own modules.

pub mod owner;
