//! The CQL drivers' integration suites against `loams-cql` (design §77
//! §12.2).
//!
//! The Python driver's, gocql's and the Rust driver's selected subsets
//! (research report §2.2), each first run against `cassandra:5.0.9`
//! (`sha256:3ea93337…`) as the control. No code from `scylladb/scylladb`
//! (D1649). Built in KV3a, completed in KV3b.

use super::{Runner, Status};

pub const RUNNER: Runner = Runner {
    name: "cql-drivers",
    about: "the Python, Go and Rust CQL drivers' subsets against loams-cql",
    status: Status::Planned { milestone: "KV3a" },
};
