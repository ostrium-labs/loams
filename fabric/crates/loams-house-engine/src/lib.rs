//! The Loams House Rust engine (design §75 §8; plan HE1 Tasks 23 and 39).
//!
//! It plans and executes ClickHouse statements and writes ClickHouse
//! formats. In HE1 Task 23 it runs over an in-memory catalog seeded from the
//! oracle ([`catalog`], [`seed`]) behind `loams-house-engine-dev`
//! ([`server`]), the oracle's `house-rust` target (D1804). Task 39 runs it in
//! `loams-house-rworker`, a worker process that links no libchdb (D1578),
//! over the House's pinned lake tables. It never depends on the front
//! (`loams-house`), and the front never depends on DataFusion
//! (`engine_crates_stay_apart`).

pub mod catalog;
pub mod engine;
pub mod lake;
pub mod registry;
pub mod seed;
pub mod server;
pub mod settings;

pub use engine::{Answer, Engine};
pub use settings::Settings;
