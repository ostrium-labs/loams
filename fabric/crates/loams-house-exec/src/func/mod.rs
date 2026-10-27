//! ClickHouse's scalar functions, one module per family (plan HE1 Tasks
//! 30–33), each with a `register(&mut Registry)`.

pub mod cond;
pub mod convert;
pub mod json;
pub mod math;
pub mod misc;
pub mod typed;

/// Registers every Task 33 family.
pub fn register_t33(r: &mut loams_house_sql::Registry) {
    convert::register(r);
    cond::register(r);
    math::register(r);
    json::register(r);
    misc::register(r);
}
