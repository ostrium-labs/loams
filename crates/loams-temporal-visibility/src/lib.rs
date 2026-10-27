//! `loams-temporal-visibility`: visibility for the Loams Temporal gateway
//! (design 74 Q1162: the full list-filter grammar on Postgres, no
//! Elasticsearch; D2074-D2076).
//!
//! - [`query`]: Temporal's SQL-like visibility query language, parsed.
//! - [`filter`]: the parsed query checked against the system and custom
//!   search attributes and typed ([`compile`]).
//! - [`sql`]: a typed filter as parameterised SQL on the `visibility` table.
//! - [`eval`]: the same filter in memory, with the same semantics.
//! - [`store`]: the backends ([`PgVisibility`], [`MemVisibility`]) and the
//!   per-namespace search-attribute registry.

pub mod eval;
pub mod filter;
pub mod query;
pub mod sql;
pub mod store;

pub use eval::Record;
pub use filter::{QueryError, Resolved, ValueType, compile};
pub use store::{MemVisibility, PgVisibility, VisibilityError, VisibilityStore};
