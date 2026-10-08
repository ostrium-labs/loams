//! The store seam of Loams Live (design §45 §8; LV1 plan Tasks 20–21).
//!
//! [`Store`] is the one transaction surface `loams-live` uses. It is an enum
//! over two backends (LV1 plan Ruling 1): **embedded**, MVCC on redb (Task
//! 21, [`embedded`]), and **tikv** (feature `tikv`), which wraps
//! `loams-tikv`'s handle. [`Store::run`] runs a body in a transaction with
//! the runner's retries; [`Store::snapshot`] reads at a [`Ts`]. [`Txn`] and
//! [`Snap`] are the transaction and the read-only view, every key relative
//! to the store's root. [`Ts`] is a timestamp in TSO layout on both backends
//! (Ruling 3). [`TxnOptions`], [`TxnError`] and [`Committed`] are the
//! runner's options, error classes and result; [`FaultPlan`] its fault hooks;
//! [`GcBarrier`] holds GC below a timestamp. [`tuple`] is the
//! order-preserving tuple codec. [`testing`] yields the stores a test runs
//! on, and [`conformance`] holds every backend to the same semantics
//! ([`kv_conformance!`]).

mod codec;
pub mod conformance;
pub mod embedded;
mod faults;
mod gc;
mod runner;
mod store;
pub mod testing;
#[cfg(feature = "tikv")]
mod tikv;
mod ts;
mod txn;

pub use codec::{CodecError, tuple};
pub use faults::{Fault, FaultPlan, FaultPoint};
pub use gc::GcBarrier;
pub use runner::{CommitMode, Committed, Mode, TxnError, TxnOptions};
pub use store::{Backend, EmbeddedConfig, KvError, PESSIMISTIC_REFUSED, Store, StoreConfig};
pub use ts::Ts;
pub use txn::{MAX_VALUE_BYTES, Pair, Snap, Txn};

/// The TiKV handle configuration of [`StoreConfig::Tikv`].
#[cfg(feature = "tikv")]
pub use loams_tikv::TikvConfig;
