//! The Go tests of Titan (`distributedio/titan`) as a reference corpus
//! (design §77 §8.2, §12.3).
//!
//! Titan at `97868f5d24e0c13621658fbcb712064fec59e8be`: its
//! `tools/autotest` drives a server over redigo. A harness-side patch here
//! (not in Titan) skips `integration.Start()` and passes the gateway's
//! address. KV0 Task 10 runs it against Valkey first; KV2 against
//! `loams-resp`, and, if Titan builds and serves on `mocktikv://`, against
//! an unmodified Titan as a differential oracle.

use super::{Runner, Status};

pub const RUNNER: Runner = Runner {
    name: "titan-go",
    about: "distributedio/titan's autotest corpus against loams-resp",
    status: Status::Planned { milestone: "KV2" },
};
