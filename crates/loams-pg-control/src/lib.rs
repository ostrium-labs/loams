//! Loams Postgres's control plane, `pg-control` (design §46 §6; PG2).
//!
//! So far: the records ([`model`]) and the store they live in
//! ([`store::PgControlStore`]), on the embedded store in single-node mode
//! and on TiKV in production (Task 3); resource ids and the derived Neon
//! ids ([`ids`]), and names and PgDog's routed-name grammar ([`names`])
//! (Task 4); the project and branch RPCs ([`service`]) with their
//! operations and idempotency ledger, and the seam to Neon's components
//! they read through ([`neon::NeonRead`]) (Task 5); the role and database
//! RPCs and the [`secrets::SecretStore`] role passwords live in (Task 6);
//! the project and branch reconcilers, under each project's fenced lease
//! ([`reconcile`]) (Task 7); the upcall listener Neon's components call,
//! with the compute tokens it checks ([`upcall`]) (Task 8); who may call
//! ([`authz`]) and the audit events every mutation emits ([`audit`])
//! (Task 9); where computes run ([`runtime`]: the `ComputeRuntime` seam,
//! the compose runtime and the computes' certificates) (Task 10); the
//! endpoint RPCs, `WatchProject`, and the endpoint reconciler that runs
//! the compute state machine ([`reconcile::endpoint`]) (Task 11); idle
//! detection and the suspend ([`idle`], with PgDog's pools read through
//! [`pgdog::PgDogAdmin`]) and the compute lifecycle events
//! ([`observer::ComputeLifecycleObserver`]) (Task 12); the Kubernetes
//! runtime (Task 13); the warm pool of unbound computes ([`pool`])
//! (Task 14). Reached
//! only through `loams`'s feature `postgres` (Task 9), whose `pg-control`
//! role serves this crate.

pub mod audit;
pub mod authz;
pub mod autoscale;
pub mod idle;
pub mod ids;
pub mod meter;
pub mod model;
pub mod names;
pub mod neon;
pub mod observer;
pub mod pgdog;
pub mod pool;
pub mod reconcile;
pub mod runtime;
pub mod scram;
pub mod secrets;
pub mod service;
pub mod store;
pub mod upcall;

pub use store::{
    ApiWriter, Batch, BatchError, Fence, KvControlStore, Page, PgControlStore, StoreError,
    StoreEvent, StoreOptions, Versioned,
};
