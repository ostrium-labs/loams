//! The runners that drive a gateway with an outside suite. **None runs
//! yet**: the gateways do not exist until KV2 (`loams-resp`) and KV3
//! (`loams-cql`). Each module records what it will run, against what, and
//! where its inputs come from, so the later tasks fill in a known shape.
//! Suites are fetched by CI at pinned revisions at run time, never copied
//! into this repository (KV1 plan Global Constraints).

pub mod cql_drivers;
pub mod differential;
pub mod tidis_py;
pub mod titan_go;
pub mod valkey_tcl;

/// Where a runner stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Not built yet; the milestone that builds it.
    Planned { milestone: &'static str },
}

/// A runner's description, for `loams-gw-test list`.
#[derive(Debug, Clone, Copy)]
pub struct Runner {
    pub name: &'static str,
    pub about: &'static str,
    pub status: Status,
}

/// Every runner.
pub fn all() -> [Runner; 5] {
    [
        valkey_tcl::RUNNER,
        tidis_py::RUNNER,
        titan_go::RUNNER,
        cql_drivers::RUNNER,
        differential::RUNNER,
    ]
}
