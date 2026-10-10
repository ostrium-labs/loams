//! The reconcilers and the fenced project lease (PG2 Task 7) on the local
//! (embedded) store and a fake Neon. `reconcile_tikv` runs the same cases
//! on TiKV.

#[macro_use]
mod common;

use loams_pg_control::store::conformance::{Factory, local_factory};

/// The backend the cases run on.
fn factory() -> Factory {
    local_factory(env!("CARGO_TARGET_TMPDIR"))
}

#[path = "reconcile/cases.rs"]
mod cases;
