//! ClickHouse's scalar functions on DataFusion (plan HE1 Tasks 30–33), one
//! module per family, each with a `register`.

pub mod arrays;
pub mod chudf;
pub mod cond;
pub mod convert;
pub mod datetime;
pub mod hash;
pub mod json;
pub mod lambda;
pub mod maps;
pub mod math;
pub mod misc;
pub mod nested;
pub mod strings;
pub mod tuples;
pub mod typed;

/// Registers every Task 33 family.
pub fn register_t33(r: &mut loams_house_sql::Registry) {
    convert::register(r);
    cond::register(r);
    math::register(r);
    json::register(r);
    misc::register(r);
}
