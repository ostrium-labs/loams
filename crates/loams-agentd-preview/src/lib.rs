//! Project-scoped HTTP discovery and stable local routing on this device.
//! Application bytes use bounded multiplexed streams over a local socket pair.

pub mod catalog;
pub mod discovery;
pub mod mux;
pub mod proxy;

pub mod service;
pub use service::PreviewService;
