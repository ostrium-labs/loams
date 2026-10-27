//! loams-agentd: Loams Desktop's per-user agent daemon (design §50).
//!
//! The library holds the process model the binary runs: the daemon directory,
//! `config.toml`, the single-instance lock, the token, discovery and `Hello`
//! (plan DD1 Task 6), the RPC's access control (Task 7), and the per-user
//! service, the runtime directory and linger (Task 8), and the remote
//! Connect listener's settings (plan SD1 Task 7).

pub mod config;
pub mod discovery;
pub mod hello;
pub mod lock;
pub mod marketplace;
pub mod owner_only;
pub mod paths;
pub mod remote;
pub mod roles;
pub mod runtime_install;
pub mod server;
pub mod service;
pub mod token;

/// Exit code: the daemon stopped (§50 §5.1).
pub const EXIT_STOPPED: u8 = 0;
/// Exit code: another daemon already runs on this directory.
pub const EXIT_ALREADY_RUNNING: u8 = 3;
/// Exit code: `config.toml` is missing or invalid.
pub const EXIT_INVALID_CONFIG: u8 = 4;
/// Exit code: an internal error. Service managers restart only on this and on signals.
pub const EXIT_INTERNAL: u8 = 70;
