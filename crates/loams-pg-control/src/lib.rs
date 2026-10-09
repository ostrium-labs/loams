//! Loams Postgres's control plane, `pg-control` (design §46 §6; PG2).
//!
//! So far: the records ([`model`]) and the store they live in
//! ([`store::PgControlStore`]), on the embedded store in single-node mode
//! and on TiKV in production (Task 3); resource ids and the derived Neon
//! ids ([`ids`]), and names and PgDog's routed-name grammar ([`names`])
//! (Task 4); the project and branch RPCs ([`service`]) with their
//! operations and idempotency ledger, and the seam to Neon's components
//! they read through ([`neon::NeonRead`]) (Task 5). Reached only through `loams`'s
//! feature `postgres` (Task 9).

pub mod ids;
pub mod model;
pub mod names;
pub mod neon;
pub mod service;
pub mod store;

pub use store::{
    ApiWriter, Batch, BatchError, Fence, KvControlStore, Page, PgControlStore, StoreError,
    StoreEvent, StoreOptions, Versioned,
};
