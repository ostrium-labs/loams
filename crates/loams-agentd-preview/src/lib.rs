//! Project-scoped HTTP discovery and stable local routing on this device.
//! Application bytes use bounded multiplexed streams over a local socket pair.

// Lints the zeron fork never ran clippy against; plan DD1 rulings T1-12 and T1-13. ci.yml's
// workspace clippy already runs with -D warnings, so this list keeps it green until
// Tasks 2-4 delete or fix the code and drop it.
#![allow(
    clippy::large_enum_variant,
    clippy::field_reassign_with_default,
    clippy::unwrap_used,
    missing_debug_implementations
)]
pub mod catalog;
pub mod discovery;
pub mod mux;
pub mod proxy;

pub mod service;
pub use service::PreviewService;
