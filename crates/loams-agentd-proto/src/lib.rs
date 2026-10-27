//! loams-agentd-proto — wire types shared by engine, UI, and RPC.
//!
//! Ported from loams-desktop's `packages/control/src/wire.ts` + `packages/harness/src/types.ts`.
//! Context occupancy is replicated per chat. The harness's `Usage` events are kept per
//! turn for the thread header (`usage`, SD1 Task 14), informational and never billing.

pub mod agent;
pub mod approvals;
pub mod brand;
pub mod chat;
pub mod checkpoints;
pub mod daemon;
pub mod entities;
pub mod factory;
pub mod factory_loop;
pub mod file_mentions;
pub mod git_write;
pub mod groups;
pub mod invocation;
pub mod marketplace;
pub mod motion;
pub mod notify;
pub mod params;
pub mod preview;
pub mod review;
pub mod rpc;
pub mod sidebar_pins;
pub mod transcript;
pub mod usage;
pub mod view;
pub mod workspace;

pub use agent::*;
pub use chat::*;
pub use entities::*;
pub use git_write::*;
pub use params::*;
pub use preview::*;
pub use review::*;
pub use sidebar_pins::*;
pub use workspace::*;

/// Parse "0.2.12" (tolerating a `-suffix`/`+build` tail on the last part)
/// into a comparable triple — the fleet feature-gate primitive (device rows
/// stamp `Device::version` at boot). `None` for anything that doesn't lead
/// with three dotted integers, and gates treat `None` as "too old".
pub fn version_triple(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.trim().splitn(3, '.');
    let major: u64 = parts.next()?.parse().ok()?;
    let minor: u64 = parts.next()?.parse().ok()?;
    let patch = parts.next()?;
    let patch: u64 = patch
        .split(['-', '+'])
        .next()
        .unwrap_or(patch)
        .parse()
        .ok()?;
    Some((major, minor, patch))
}
