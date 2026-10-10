//! The order-preserving tuple codec (design §20 §4.3; R1 plan Task 2).
//!
//! The codec lives in `loams-tuple` since LV1 Task 20, shared with
//! `loams-kv` (LV1 plan Ruling 2, row T20-1); this module re-exports it so
//! `loams_tikv::tuple` and `loams_tikv::codec::tuple` keep their paths.

pub use loams_tuple::{CodecError, tuple};
