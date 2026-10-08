//! The order-preserving tuple codec (design §20 §4.3), the one `loams-tikv`
//! re-exports too: both come from `loams-tuple` (LV1 plan Ruling 2, row
//! T20-1).

pub use loams_tuple::{CodecError, tuple};
