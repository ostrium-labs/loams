//! The Loams House worker (HS1 Task 2, design §49 §4): the only binary that links
//! libchdb.
//!
//! The front (`loams-fabric house`) starts it with `posix_spawn`, an empty
//! environment, the arguments of [`config::WorkerArgs`] and one end of a Unix
//! socket pair on fd 3. The worker writes its own chDB configuration (explicit
//! grants, no disk cache), boots the engine, sends `Ready`, waits for `Bind`, and
//! then serves `Execute`s for that one namespace until the front kills it. It has no
//! shutdown path: the front ends it with `SIGKILL` (§49 §10.1), and a worker whose
//! socket closes exits at once.
//!
//! The library half is the same serve loop, so `loams-house`'s `InprocWorker`
//! (feature `inproc-worker`) can run it on a thread of the front for development.

pub mod config;
pub mod serve;
pub mod settings;

pub use config::WorkerArgs;
/// The engine configuration type [`engine_config`] returns.
pub use loams_chdb::EngineConfig;
pub use serve::{End, Hosting, Worker, engine_config, engine_error};
