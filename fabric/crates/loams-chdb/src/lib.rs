//! Loams's chDB engine: one engine per process, a session per connection,
//! results as byte blocks or Arrow record batches, and a cancellation a client
//! can see at once.
//!
//! FL2 Task 1 built the safe half of the FFI that `loams-chdb-sys` generates
//! (Ruling 1), and Task 2's House will sit on top of it. What that means in use:
//!
//! ```no_run
//! use loams_chdb::{Engine, EngineConfig, SessionId};
//!
//! # fn main() -> Result<(), loams_chdb::ChdbError> {
//! // One engine per process: a second call with the same configuration returns
//! // the same engine, and one with a different configuration is refused.
//! let engine = Engine::start(EngineConfig::default())?;
//! println!("chDB {} is ClickHouse {}", engine.chdb_version(), engine.version());
//!
//! // A session is a connection, which is where a House `session_id`'s
//! // isolation comes from.
//! let session = engine.session(SessionId::new("loams-test"), &Default::default())?;
//!
//! // Results stream: one block at a time, released when the stream drops.
//! let mut stream = session.execute("SELECT 1 AS one", "JSONEachRow", &[])?;
//! while let Some(chunk) = stream.next_chunk()? {
//!     println!("{}", String::from_utf8_lossy(&chunk));
//! }
//! println!("{:?}", stream.stats());
//! # Ok(())
//! # }
//! ```
//!
//! Everything an async caller wants has an `_async` form that runs the FFI on a
//! blocking thread, because a query can keep a fetch blocked for as long as it
//! takes to run: [`QueryStream::next_chunk_async`], [`ArrowStream::next_batch_async`],
//! [`Session::execute_async`], [`Session::execute_arrow_async`],
//! [`Session::cancel_async`] and [`Engine::version_async`]. The synchronous methods
//! are the primitives those are built on and are meant for a caller that is
//! already on a blocking thread.
//!
//! # What the pinned library actually does
//!
//! Task 1 measured all of this against `libchdb.so` v26.9.0, whose
//! `SELECT version()` answers `26.9.2.1`. The parts that shape this crate:
//!
//! * `chdb_connect` returns a *cell* and every query takes the connection inside
//!   it, so both halves are kept ([`engine::Connection`]).
//! * **One connection shape per process.** A second `chdb_connect` whose
//!   *server-level* arguments differ from the first one's returns null, so a
//!   session takes the engine's arguments and applies its settings per statement.
//!   HS1 R1.9 later measured that differing *query-level* arguments are accepted,
//!   which is what [`Engine::session_with_args`] is for.
//! * A result carrying an error has a **null buffer**, which `slice::from_raw_parts`
//!   rejects outright rather than reading as an empty slice.
//! * `chdb_stream_query` returns in about a millisecond — the embedded engine runs
//!   the statement on its own thread — so starting a query and cancelling it are
//!   different threads' work.
//! * **A cancellation spends the connection it happened on**: the next streaming
//!   statement on that connection answers `"No active streaming query"`, so a
//!   cancelled [`Session`] opens a new one. And a second `chdb_connect` is only
//!   accepted when its arguments match the first one's exactly.
//! * **Neither Arrow direction works at v26.9.0.** `chdb_arrow_scan` never returns,
//!   and the stream `chdb_stream_query_arrow` produces hangs in its `get_schema` —
//!   and either of them **terminates the process with SIGSEGV** once
//!   `chdb_set_signal_handlers_enabled(0)` has been called, which is what
//!   [`EngineConfig::install_signal_handlers`] being false means. The API is
//!   implemented as the header describes and `tests/arrow.rs` runs it in a child
//!   process, so the defect is measured rather than asserted away.
//! * `chdb_stream_cancel_query` does **not** interrupt a statement: it blocks
//!   until the statement finishes on its own (275 s for
//!   `SELECT count() FROM numbers(1e12)` on this machine) and then tears the stream
//!   down. chDB 26.9.0 has no `query_id` setting either, so a statement cannot be
//!   named for a cross-connection `KILL QUERY`. [`Session::cancel`] is therefore
//!   Loams' cancellation, answered at once with `394 CANCELLED`, with the engine's
//!   own blocking cancel left in the background.
//!
//! Names: Loams, `loams-*` (owner rulings, 2026-10-01).

pub mod arrow;
pub mod engine;
pub mod error;
pub mod query;
pub mod session;

pub use arrow::{ArrowHandle, ArrowStream, RecordBatchReader};
pub use engine::{Engine, EngineConfig, SessionId, Settings};
pub use error::ChdbError;
pub use query::{QueryStats, QueryStream};
pub use session::{Analysis, InsertStream, InsertSummary, QueryClass, Session};
