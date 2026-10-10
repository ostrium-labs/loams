//! The project, branch, role and database RPCs (PG2 Tasks 5 and 6) on the local (embedded) store
//! and a fake Neon. `service_tikv` runs the same cases on TiKV.

#[macro_use]
mod common;

use loams_pg_control::store::conformance::{Factory, local_factory};

/// The backend the cases run on.
fn factory() -> Factory {
    local_factory(env!("CARGO_TARGET_TMPDIR"))
}

#[path = "service/branches.rs"]
mod branches;
#[path = "service/projects.rs"]
mod projects;
#[path = "service/roles.rs"]
mod roles;
