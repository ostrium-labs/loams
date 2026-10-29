//! The `loams` binary's client of the local agent daemon (design §50 §5.4,
//! §73 §7.1–§7.2; plan MK1 Task 10).
//!
//! - [`connect`]: find the daemon through its discovery file, dial its
//!   loopback WebSocket with its owner token and say `Hello`.
//! - [`marketplace`]: `loams marketplace search|info|install|uninstall|list|update|source`.
//! - [`term`]: the terminal a command writes to and asks on.
//!
//! It links the daemon's wire types and RPC client only, never the daemon:
//! the `loams` server binary gains no agentd engine, harness or store crate.

pub mod connect;
pub mod marketplace;
pub mod term;
