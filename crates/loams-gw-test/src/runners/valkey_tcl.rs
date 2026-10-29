//! Valkey's TCL suite against `loams-resp` (design §77 §12.1, D1661).
//!
//! Valkey `9.1.2` (tag commit `7f1dffedff6de73058b2c2a389422b6ecd56c8fb`,
//! BSD-3-Clause) is the conformance suite: Redis 7.4 and later are not
//! open source (RSALv2/SSPL, and AGPL from Redis 8), so their tests are not
//! used. CI fetches `tests/` at the tag unmodified, runs
//! `./runtest --host <gateway> --port <port>` with the subset and the
//! tags of the research report §1.4, and compares the pass, fail and skip
//! counts with a control run against `valkey/valkey:9.1.2`
//! (`sha256:418652cf…`, KV0 Task 10). Built in KV2.

use super::{Runner, Status};

pub const RUNNER: Runner = Runner {
    name: "valkey-tcl",
    about: "Valkey 9.1.2's TCL suite (the report §1.4 subset) against loams-resp",
    status: Status::Planned { milestone: "KV2" },
};
