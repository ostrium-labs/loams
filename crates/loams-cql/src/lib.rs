//! Cassandra gateway components on Loams KV (design 77, milestone KV3a).
//!
//! This initial slice implements partition token calculation only. It does
//! not yet expose a listener or claim CQL driver compatibility.

pub mod frame;
pub mod handshake;
pub mod query;
mod token;

pub use token::cassandra_token;
