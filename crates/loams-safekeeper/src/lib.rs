//! Loams's WAL service for Neon computes (§28 of the design docs, phase P4a).
//!
//! Loams replaces Neon's safekeepers with one logical acceptor per timeline,
//! backed by TiKV as the quorum hot tier and group-committed to the bucket.
//! This crate speaks Neon's safekeeper protocol, so walproposer (the compute's
//! Postgres extension) and the pageserver stay unmodified:
//!
//! - [`proto`]: the v3 wire messages and the replication commands.
//! - [`acceptor`]: the proposer-facing state machine, ported from Neon's
//!   `safekeeper.rs`.
//! - [`store`]: the [`WalStore`] trait (the fenced, durable head and WAL
//!   chunks), the shared state-transition rules, and an in-memory store.
//! - `tikv` (feature `tikv`): the TiKV store, one 1PC transaction per call.
//! - [`tikv_raw`]: the raw TiKV store, blind pipelined appends fenced once
//!   per election (`TikvRawKv` with feature `tikv`).
//! - `journal`, `meta`, `nvme` (feature `nvme`): Arm A's local-NVMe store
//!   (§28 §7.2): a shared, preallocated journal with tiered durable writes,
//!   pipelined appends, and acceptor metadata off the commit path.
//! - [`pgwire`]: the slice of the Postgres protocol a safekeeper speaks.
//! - `service`, `http` (feature `server`): the listener walproposer connects
//!   to, the control plane's HTTP API, and the `loams-wal` binary; `send`,
//!   the pageserver's interpreted replication, served in process through a
//!   `WalInterpreter` (Neon's `wal_decoder`, Q112); and `broker`, the
//!   storage broker publication through which the pageserver finds it.
//!
//! Neon's code is Apache-2.0; the ported parts keep its structure and name
//! their sources.

pub mod acceptor;
#[cfg(feature = "server")]
pub mod broker;
#[cfg(feature = "server")]
pub mod cli;
#[cfg(feature = "server")]
pub mod http;
#[cfg(feature = "nvme")]
pub mod journal;
#[cfg(feature = "nvme")]
pub mod meta;
#[cfg(feature = "nvme")]
pub mod nvme;
pub mod pgwire;
#[cfg(feature = "server")]
pub mod propose;
pub mod proto;
#[cfg(feature = "server")]
pub mod send;
#[cfg(feature = "server")]
pub mod service;
#[cfg(feature = "compio")]
pub mod shard;
pub mod store;
#[cfg(feature = "tikv")]
pub mod tikv;
pub mod tikv_raw;
pub mod types;

pub use acceptor::Acceptor;
pub use store::{AppendBatch, Deposed, MemWalStore, WalStore};
pub use types::{AcceptorState, Lsn, Term, TimelineId};

/// Errors of the WAL service.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A malformed or out-of-order message; the connection is dropped and
    /// walproposer reconnects.
    #[error("protocol: {0}")]
    Protocol(String),
    /// The timeline does not exist (and creation was not allowed).
    #[error("timeline {0} not found")]
    NotFound(TimelineId),
    /// The requested WAL was trimmed from the hot tier.
    #[error("WAL from {from} was trimmed (the hot tier starts at {trimmed})")]
    Trimmed { from: Lsn, trimmed: Lsn },
    /// The store failed; the operation may be retried.
    #[error("store: {0}")]
    Store(String),
    /// A connection failed.
    #[error("io: {0}")]
    Io(String),
}
