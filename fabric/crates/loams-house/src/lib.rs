//! Loams House: the declared ClickHouse surface `chsurface-1.0` of
//! [§32](../../../design/32-loams-flow-fabric-house.md) §8, served over the
//! ClickHouse HTTP interface.
//!
//! **Task 3 owns errors only.** [`errors`] is the whole crate so far: the error
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
//! library's exception text apart (that parser is measured, with the real captured
//! strings, in `loams-chdb/src/error.rs`) and this crate only renders the parts.
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

pub mod errors;

pub use errors::{CODES, ChError, HouseError, MidStreamBody};
