//! Operators and functions of the Loams House Rust engine (design §75 §6.3;
//! plan HE1 Tasks 27–35).
//!
//! ClickHouse's scalar functions, aggregate functions and combinators, the
//! extension nodes' physical operators (`FINAL`, totals, `LIMIT BY`, `ARRAY
//! JOIN`, `WITH FILL`), registered with DataFusion.

pub mod func;
pub mod ops;
