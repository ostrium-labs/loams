//! ClickHouse's scalar functions on DataFusion (plan HE1 Tasks 30–33), one
//! module per family, each with a `register`.

pub mod arrays;
pub mod lambda;
pub mod maps;
pub mod nested;
pub mod tuples;
