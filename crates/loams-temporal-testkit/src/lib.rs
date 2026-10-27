//! The Temporal gateway's test kit (design 74 §13.1, plan DX1 Task 2).
//!
//! - [`corpus`]: DX0 Task 4's corpus of real Temporal histories, loaded and
//!   checked against its manifest.
//! - [`json`]: a `History` as protobuf JSON, the shape the Temporal CLI
//!   writes and the corpus's `normalize.py` reads.
//! - [`normalize`]: the same masking as `normalize.py`, plus the gateway's
//!   one extra equivalence (a sticky task queue reads as its normal queue:
//!   the gateway has no sticky queues in DX1, Q1156).
//! - [`diff`]: the differences between two normalised histories, event by
//!   event and field by field.

pub mod corpus;
pub mod diff;
pub mod json;
pub mod normalize;

pub use corpus::{Corpus, CorpusError, CorpusHistory};
pub use diff::{Difference, diff};
pub use normalize::{Options, compare, normalize, normalized_history};
