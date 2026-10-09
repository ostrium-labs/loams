//! Loams Postgres's client of Neon's components (design §46 §6, PG2 Task 2):
//! the pageserver and the storage controller ([`pageserver`]), `loams-wal`
//! ([`wal`]), `compute_ctl` ([`compute_ctl`]) and the compute spec
//! ([`spec`]). A plain HTTP client with no Neon code dependency: every
//! shape it copies is the fork's (`ostrium-labs/neon`, tag
//! `loams-decoder-trim-1`), and the module docs name the source file.
//! `tests/fixtures/` holds requests the pinned images accepted and the
//! answers they gave.
//!
//! Reached only through `loams`'s feature `postgres` (PG2 Task 9).

pub mod compute_ctl;
pub mod error;
mod http;
pub mod ids;
pub mod pageserver;
pub mod secret;
pub mod spec;
pub mod storcon;
pub mod wal;

pub use error::{Component, NeonError, Op};
pub use ids::{Lsn, TenantId, TimelineId};
pub use secret::Secret;
