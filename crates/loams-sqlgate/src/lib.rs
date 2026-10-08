//! `loams-sqlgate`, the Loams SQL gate (design §47 §12; plan SQ1 Tasks 3–5).
//!
//! The gate terminates TLS, authenticates, wakes the branch's `tidb-server`
//! pool and relays packets; it never parses SQL (D731). This crate starts
//! with [`codec`], the sans-I/O MySQL protocol pieces the server (Task 4)
//! drives.

pub mod codec;

#[doc(hidden)]
pub mod fuzz;
