//! Tidis's Python suite as a reference corpus (design §77 §8.2, §12.3).
//!
//! `test/test_helper.py --ip --port` from `tidb-incubator/tidis` at
//! `555856206f248737b12d3f148b10f08a604bc821` (269 `unittest` cases on
//! `redis==3.5.3`, 98 of them Lua, which v1 leaves out, Q1223), fetched at
//! run time. KV0 Task 10 records which cases fail on real Valkey (Tidis's
//! own deviations); KV2 triages the rest against `loams-resp`.

use super::{Runner, Status};

pub const RUNNER: Runner = Runner {
    name: "tidis-py",
    about: "Tidis's Python unittest corpus (Lua excluded) against loams-resp",
    status: Status::Planned { milestone: "KV2" },
};
