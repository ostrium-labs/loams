//! Control-plane protocols as sans-I/O [`crate::Machine`]s, each the code
//! side of a TLA+ spec in `spec/tla/router/` whose actions it emits as
//! [`crate::SpecEvent`]s.
//!
//! - [`lifecycle`]: a Loams SQL branch's serverless lifecycle (design §47
//!   §14; `Lifecycle.tla`; plan SQ1 Task 5).

pub mod lifecycle;
