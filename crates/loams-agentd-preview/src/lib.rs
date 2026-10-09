//! Project-scoped HTTP discovery, stable local routing, and authenticated peers.
//! Application bytes use bounded multiplexed streams; edge signaling never
//! transports preview requests or response bodies.

// Lints the zeron fork never ran clippy against; plan DD1 ruling T1-12. Tasks 2-4
// delete or fix the code and then drop this list (Task 4 makes the agentd job -D warnings).
#![allow(clippy::large_enum_variant, clippy::field_reassign_with_default)]
pub mod catalog;
pub mod discovery;
pub mod login;
pub mod mux;
pub mod peer;
pub mod proxy;
pub mod signaling;

pub mod service;
pub use service::PreviewService;
