//! The Temporal gateway's frontend (design 74 §5, plan DX1 Task 8): the
//! `WorkflowService` gRPC surface of the DX1 method set over the engine
//! (state machine + compare-and-set store), the matcher, and Resonate.
//!
//! - [`Engine`]: load a run, apply one input, commit events, state and
//!   outbox rows in one compare-and-set; retry on conflict.
//! - [`service`]: the methods; tasks handed out only after Resonate grants
//!   the claim; task tokens HMAC-signed ([`token`]); callers authenticated
//!   ([`auth`]).
//! - [`effects`]: the state machine's effects as outbox rows with
//!   deterministic promise IDs; the relay drains them into Resonate.
//! - [`callbacks`]: timers and timeouts come back in through the engine.
//! - [`server`]: wiring, the background loops, and `UNIMPLEMENTED` with the
//!   matrix link for methods outside the DX1 set.
//! - [`operator`] and [`ui`]: what the Temporal Web UI and the `temporal`
//!   CLI read (`OperatorService`'s search attributes, `GetClusterInfo`, the
//!   reverse history) and the UI's loopback listener.
//!
//! Nothing here depends on Loams-internal crates (D1539): `loams-gateway`
//! and the standalone `loams-temporal` binary mount it.

pub mod auth;
pub mod callbacks;
pub mod capabilities;
pub mod describe;
pub mod effects;
pub mod engine;
pub mod errors;
pub mod fastpath;
pub mod followups;
pub mod lifecycle;
pub mod namespaces;
pub mod operator;
pub mod outbox;
pub mod queries;
pub mod schedules;
pub mod server;
pub mod service;
pub mod standalone;
pub mod token;
pub mod ui;
pub mod update;
pub mod visibility;

pub use auth::{Authenticator, DevAllowAll, Principal, StaticKeys};
pub use engine::Engine;
pub use server::{Gateway, ImplementedOnly, Parts, PartsExt};
pub use service::{Frontend, FrontendConfig, WorkflowServiceImpl};
pub use token::TokenKeys;
