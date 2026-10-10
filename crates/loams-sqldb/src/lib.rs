//! The Loams SQL control plane (design §47; plan SQ1).
//!
//! Loams SQL serves MySQL from one stateless `tidb-server` pool per branch,
//! each pool in keyspace mode over the shared Loams TiKV. This crate holds
//! the image pins ([`images`]), the shared model ([`model`]), the rendered
//! `tidb.toml` ([`render`]), the compute runtimes ([`runtime`]), and the
//! serverless lifecycle: suspend and resume sagas and wake on connect
//! ([`sagas`]), driven by the idle detector ([`idle`]).
//!
//! TiDB, TiKV, PD and BR run as images; nothing Go is linked (D11, D126,
//! `docs/sqldb/licensing.md`).

pub mod idle;
pub mod images;
pub mod model;
pub mod render;
pub mod runtime;
pub mod sagas;
