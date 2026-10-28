//! The agent profile (SPIKE, MS1 Task 1, D1807; feature `agent`).
//!
//! One long-lived worker per running agent: [`AgentWorker::start`] spawns
//! `loams-agent-worker worker` (MS1 Task 2: `loams-agentd agent-worker`),
//! which seals itself with Live's function-worker sandbox, unchanged, and
//! evaluates the agent's bundle (one ES module). The agent imports
//! `loams:agent` (`acp`, `model`, `mcp`, `loams`) and speaks ACP through
//! `acp.connect(handler)` or `acp.stream()`; every effect is a frame to
//! agentd ([`AgentHosts`] under the install's [`Manifest`]), and
//! [`acp::serve_stdio`] presents the agent to agentd's `AcpHarness` as an
//! ordinary ACP agent on stdio.
//!
//! See `docs/plans/ms1-quickjs-agent-spike.md`.

pub mod acp;
pub mod fake;
mod globals;
mod host;
mod worker;

pub use host::{
    AgentConfig, AgentHosts, AgentWorker, FetchRequest, FetchResponse, HostFuture, Manifest,
};
#[doc(hidden)]
pub use loams_live_proto::loams::live::worker::v1::ProbeKind;
pub use worker::agent_worker_main;
