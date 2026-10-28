//! Cassandra gateway components on Loams KV (design 77, milestone KV3a).
//!
//! This initial slice implements partition token calculation only. It does
//! not yet expose a listener or claim CQL driver compatibility.

mod token;

pub use token::cassandra_token;
