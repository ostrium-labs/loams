//! Durable execution for Loams (D1, design §21): the Resonate server
//! embedded in the `loams` process.
//!
//! [`DurableServer`] builds Resonate from its plugins ([`registry`]) with a
//! configuration made from Loams's flags alone ([`DurableConfig`]), serves the
//! durable API on a loopback listener (127.0.0.1:8001 by default, D138), and
//! answers in-process protocol calls ([`DurableServer::process`]). The store
//! is the local one for `loams dev` and `standalone` (upstream's blob server
//! on a redb file, [`local`]; D2450 replaced SQLite), a MySQL-protocol
//! database with the `mysql` feature, or Postgres with the `postgres` feature.
//!
//! [`DurableRuntime`] runs Loams's own durable functions on the Resonate Rust
//! SDK over [`InProcNetwork`], which reaches the server through its
//! `worker_inproc` plugin with no socket (D141).
//!
//! [`DurableGateway`] is the listener's protocol endpoint as a function, for a
//! host that serves the durable API on a port of its own behind its own
//! authentication (B12: `loams` under `/durable/` on its main port).
//!
//! The embed leaves its host alone: no tracing subscriber, no signal handler,
//! no panic hook, and a handler panic answers 500.

mod config;
mod embed;
mod error;
mod gateway;
pub mod inproc;
mod listen;
pub mod local;
#[cfg(feature = "mysql")]
mod mysql;
mod registry;
mod runtime;

pub use config::{
    DEFAULT_DATABASE, DEFAULT_LISTEN, DEFAULT_RETRY_TIMEOUT, DEFAULT_SHUTDOWN_TIMEOUT,
    DurableConfig, DurableStore, MysqlTls, PROTECTED, redact_url,
};
pub use embed::{DurableServer, LOCK_FILE, PROTOCOL_VERSION};
pub use error::DurableError;
pub use gateway::DurableGateway;
pub use inproc::{InProcNetwork, InProcWorker};
pub use listen::{is_loopback, parse_listen};
pub use registry::registry;
pub use resonate_plugin::ResonateServer;
pub use runtime::{DurableRuntime, RuntimeOptions};
