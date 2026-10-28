//! The provider conformance suite and the Obscura provider's own tests.
//!
//! Everything here runs against the crate's deterministic drivers, so the whole
//! suite is seconds long and needs no browser. The same scenarios against a
//! real Obscura binary and the real service image are lane BR-b's parity suite
//! and CI job.

mod config;
mod contract;
mod egress;
mod obscura;
mod secrets;
mod webmcp;
