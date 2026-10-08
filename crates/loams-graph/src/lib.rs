//! Loams's embedded graph engine (design §33 D634).
//!
//! The engine is **Grafeo**, a graph database written in Rust, linked into this process and called
//! directly. The surface is **GQL**, the ISO standard (ISO/IEC 39075), served over **Connect-RPC** —
//! the protocol every other Loams service already speaks.
//!
//! Three consequences of D634 are visible in this module rather than merely documented:
//!
//! * **No Bolt.** Bolt's Rust ecosystem has been frozen since December 2022 and the one maintained
//!   client, `neo4rs`, is Neo4j-specific rather than portable. Reaching an engine Loams embeds over
//!   Bolt would add a second RPC stack to avoid the one Loams already has.
//! * **No network, so no credential.** [`Graph::open`] takes a path, not a URL. A remote engine
//!   would need a secret; an embedded one has none, which is why the Grafeo manifest's `auth` is
//!   `none` and its `secrets` list is empty.
//! * **No query rewriting.** A statement is passed to the engine unchanged. Classification and
//!   projection belong to the connector layer (design §33 §4); a graph statement Loams edited
//!   would no longer be the statement the user wrote.
//!
//! What this crate is *not*: it is not a connector. It is the engine behind one, and the connector
//! that exposes it to a route is CN1's `loams_flow::connectors::graph`, whose manifest is
//! `connectors/registry/grafeo.yaml`.

#![deny(missing_docs)]

pub mod engine;
pub mod service;
pub mod value;

pub use engine::{BatchStatement, Engine, Graph, GraphError, GraphRow, OpenSpec};
pub use service::GraphServiceImpl;

/// The GQL standard this crate's surface targets: ISO/IEC 39075.
pub const GQL_STANDARD: &str = "ISO/IEC 39075";

/// The engine's own version, as the `grafeo` crate reports it.
///
/// Taken from the dependency rather than written down, so the number here cannot drift from the one
/// linked into the binary; `EngineInfo` reports it over the wire as well.
pub const ENGINE_VERSION: &str = grafeo::VERSION;
