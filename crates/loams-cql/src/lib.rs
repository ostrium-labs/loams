//! Cassandra gateway components on Loams KV (design 77, milestone KV3a).
//!
//! Work in progress (preview): partition tokens, the v4 frame codec and
//! handshake, a parsed single-partition statement subset, transactional
//! scalar-row storage and an executor. It does not claim CQL driver
//! compatibility yet.

pub mod frame;
pub mod handshake;
pub mod query;
pub mod query_protocol;
pub mod server;
mod token;

pub use token::cassandra_token;

/// Transactional CQL scalar-row storage.
pub mod storage;

/// Parsed single-partition CQL statement subset.
pub mod parser;


/// Tenant-scoped CQL execution over KV.
pub mod executor;
