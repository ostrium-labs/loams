//! A stub A2A 1.0 client for Loams Bot (design 39 section 5, D465-D467).
//!
//! Loams Bot delegates to the platform agents (Plane, Zulip, Forgejo,
//! GlitchTip, analytics) over A2A. The desktop never speaks A2A to those
//! agents itself (D465: clients speak Connect to Loams Bot); this client is
//! the stand-in that lets the chat entry point run end to end before
//! `loams.bot.v1` exists (SF3), and it is what an external A2A agent URL
//! (`LOAMS_BOT_URL`) is reached with.
//!
//! The wire shapes follow A2A 1.0's JSON-RPC binding as documented in design
//! 39 section 5.3 (PascalCase methods, ProtoJSON bodies). They have to be
//! re-verified against `a2a.proto` in SF2 Task 0 (Q466); parsing is lenient
//! about enum spelling for that reason.

mod client;
mod types;

pub use client::{A2aClient, A2aError, JsonRpcA2aClient, MockA2aClient};
pub use types::{
    Artifact, Message, Part, Reply, Role, SendMessageResponse, Task, TaskState, TaskStatus,
};
