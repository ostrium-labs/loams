//! Loams House: the declared ClickHouse surface `chsurface-1.0` of
//! [§32](../../../design/32-loams-flow-fabric-house.md) §8, served over the
//! ClickHouse HTTP interface.
//!
//! # The HTTP interface (HS1 Task 3)
//!
//! [`http::serve`] answers ClickHouse's HTTP protocol on loopback (FL2 Task 2's
//! contract) and runs every statement on the [`WorkerPool`]: [`auth`] for the
//! development users, [`compress`] for request and response bodies, [`config`]
//! for the knobs.
//!
//! # The front links no libchdb (HS1 Task 2, D761)
//!
//! chDB runs in `loams-house-worker` processes. This crate supervises them —
//! [`watchdog`] starts, kills and watches one; [`admission`] keeps the
//! [`WorkerPool`] — and talks to them in `hsw1` frames (`loams-house-ipc`). Engine
//! errors arrive already taken apart, as `loams_house_ipc::EngineError`. Only the
//! `inproc-worker` feature (development, the desktop) brings libchdb into this
//! crate's graph; `no_libchdb_in_front` checks the default one.
//!
//! **FL2 Task 3 owned errors only.** [`errors`] was the whole crate then: the error
//! a client sees — ClickHouse's code, its name, its HTTP status and its own
//! rendering of the exception text — plus the two rules that make an error look
//! like a ClickHouse error to a driver. The HTTP layer (`http.rs`, `auth.rs`,
//! `compress.rs`, `config.rs`) is Task 2's, the classifier and sessions Task 4's,
//! and nothing in this module reaches for them: an error is raised, rendered and
//! given a status, and where it goes from there is the transport's business.
//!
//! # What a client sees
//!
//! ```text
//! Code: 60. DB::Exception: Unknown table expression identifier 'nope'. (UNKNOWN_TABLE) (version 26.9.2.1)
//! ```
//!
//! with the header `X-ClickHouse-Exception-Code: 60` and the HTTP status
//! ClickHouse's own `HTTPHandler` gives code 60 — 404. That last part is the one
//! that cannot be guessed: §32 §8.8 asks for it to be verified against 26.9, so
//! [`errors::CODES`] carries a status per code read out of ClickHouse's source at
//! the pinned tag rather than from the design's one-line summary.
//!
//! # Errors that come from chDB
//!
//! A statement chDB ran fails with **chDB's** code and name, not one of
//! [`errors::CODES`]'s: §32 §8.8's "errors from chDB pass through unchanged". The
//! House parses nothing twice — `loams_chdb::ChdbError` already takes the pinned
//! library's exception text apart in the worker (that parser is measured, with the
//! real captured strings, in `loams-chdb/src/error.rs`), the `hsw1` `Error` frame
//! carries the parts, and this crate only renders them.
//! [`errors::HouseError`] is the union of the two origins, so the transport answers
//! one type and never has to know which raised it.
//!
//! # An error after the first byte
//!
//! A failure that arrives while the body is still being streamed does **not**
//! replace what the client already read: the exception text is appended to the
//! bytes already sent and the connection is closed without the terminating chunk
//! (Ruling 9). [`errors::MidStreamBody`] is that rule in code, because the
//! alternative — a truncated body the client reads as a complete result — is the
//! one way a streaming surface can lie.
//!
//! Names: Loams, `loams-*` (owner rulings, 2026-10-01).

pub mod admission;
pub mod auth;
pub mod classify;
pub mod compress;
pub mod config;
pub mod errors;
#[cfg(feature = "test-hooks")]
pub mod fuzz;
pub mod http;
pub mod request;
pub mod watchdog;

pub use admission::{Collected, Event, Outcome, PoolConfig, PoolStats, WorkerLease, WorkerPool};
pub use config::{HouseConfig, UserMap};
pub use errors::{CODES, ChError, HouseError, MidStreamBody};
pub use http::{HouseHandle, serve};
#[cfg(feature = "inproc-worker")]
pub use watchdog::InprocWorker;
pub use watchdog::{ExitReason, KillHandle, Launcher, ProcessLauncher};
