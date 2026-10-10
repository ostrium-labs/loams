//! The pageserver end to end on `deploy/loams-pg-bench`, with no safekeeper
//! of any kind (PG2 Tasks 31 and 32): `scripts/pg2/it-pageserver-loams-wal.sh`
//! against the `loams-wal-interpreted` this workspace builds. It needs
//! podman (or docker) with compose and the bench's images, and takes the
//! benchmark's host ports, so it is ignored by default:
//!
//! ```text
//! POSTGRES_INSTALL_DIR=$PWD/pg_install cargo test --test it_pageserver -- --ignored
//! ```

use std::path::Path;
use std::process::Command;

fn run_it() {
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/pg2/it-pageserver-loams-wal.sh");
    let status = Command::new(script)
        .env("LOAMS_WAL", env!("CARGO_BIN_EXE_loams-wal-interpreted"))
        .status()
        .expect("run the script");
    assert!(status.success(), "it-pageserver-loams-wal.sh: {status}");
}

/// The pageserver finds `loams-wal` through the storage broker (publication
/// or discovery), ingests through the interpreted sender to the commit, and
/// a compute started afresh reads the data back.
#[test]
#[ignore = "needs compose and the bench images"]
fn it_pageserver_discovers_loams_wal_via_broker() {
    run_it();
}
