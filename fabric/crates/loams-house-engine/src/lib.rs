//! The Loams House Rust engine (design §75 §8; plan HE1 Tasks 23 and 39).
//!
//! It plans and executes ClickHouse statements over pinned lake tables and
//! writes ClickHouse formats. The House front runs it in `loams-house-rworker`,
//! a worker process that links no libchdb (D1578), and the oracle runs it in
//! `loams-house-engine-dev` (D1804). It never depends on the front
//! (`loams-house`), and the front never depends on DataFusion
//! (`engine_crates_stay_apart`).
