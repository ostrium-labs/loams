//! The reconcilers and the fenced project lease (PG2 Task 7) on TiKV and a
//! fake Neon: the cases of `reconcile`. Each prints `skipped:` when
//! `LOAMS_TEST_PD` is unset.

#[macro_use]
mod common;

use loams_pg_control::store::conformance::{Factory, tikv_factory};

/// The backend the cases run on.
fn factory() -> Factory {
    tikv_factory()
}

#[path = "reconcile/cases.rs"]
mod cases;
