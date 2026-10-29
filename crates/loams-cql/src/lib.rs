//! Cassandra gateway components on Loams KV (design 77, milestone KV3a).
//!
//! This initial slice implements partition token calculation only. It does
//! not yet expose a listener or claim CQL driver compatibility.

mod token;

pub use token::cassandra_token;

/// Transactional CQL scalar-row storage.
pub mod storage;

/// Parsed single-partition CQL statement subset.
pub mod parser;

/// Query execution contract.
pub mod query;

/// Tenant-scoped CQL execution over KV.
pub mod executor;
