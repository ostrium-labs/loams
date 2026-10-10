//! The project, branch, role and database RPCs (PG2 Tasks 5 and 6) on TiKV and a fake Neon: the
//! cases of `service_local`. Each prints `skipped:` when `LOAMS_TEST_PD` is
//! unset.

#[macro_use]
mod common;

use loams_pg_control::store::conformance::{Factory, tikv_factory};

/// The backend the cases run on.
fn factory() -> Factory {
    tikv_factory()
}

#[path = "service/branches.rs"]
mod branches;
#[path = "service/projects.rs"]
mod projects;
#[path = "service/roles.rs"]
mod roles;
