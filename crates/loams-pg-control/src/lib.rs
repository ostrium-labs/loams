//! Loams Postgres's control plane, `pg-control` (design §46 §6; PG2).
//!
//! So far: the records ([`model`]) and the store they live in
//! ([`store::PgControlStore`]), on the embedded store in single-node mode
//! and on TiKV in production (Task 3). Reached only through `loams`'s
//! feature `postgres` (Task 9).

pub mod model;
pub mod store;

pub use store::{
    ApiWriter, Fence, KvControlStore, Page, PgControlStore, StoreError, StoreEvent, StoreOptions,
    Versioned,
};
