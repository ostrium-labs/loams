//! `loams-sqlgate`, the Loams SQL gate (design §47 §12; plan SQ1 Tasks 3–5).
//!
//! The gate terminates TLS, authenticates, wakes the branch's `tidb-server`
//! pool and relays packets; it never parses SQL (D731).
//! - [`codec`]: the sans-I/O MySQL protocol (Task 3).
//! - [`server`], [`auth`], [`upstream`], [`limits`]: the gate server
//!   (Task 4, feature `server`).

pub mod codec;

#[cfg(feature = "server")]
pub mod auth;
#[cfg(feature = "server")]
pub mod limits;
#[cfg(feature = "server")]
pub mod server;
#[cfg(feature = "server")]
pub mod upstream;
#[cfg(feature = "server")]
pub mod wire;

#[doc(hidden)]
pub mod fuzz;
