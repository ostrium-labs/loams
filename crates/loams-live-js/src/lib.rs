//! `loams-live-js`: Loams Live's function runtime (LV1 plan Task 3; R1
//! Task 13 items 1–3; design §20 §6, §45 §3).
//!
//! A [`Bundle`] is one ES module whose exported objects hold query and
//! mutation functions built with `loams:server`. Its functions implement
//! [`loams_live::Function`], so the [`loams_live::Runner`] runs them like
//! the built-in `_system:*` functions: `ctx.db` calls the call's
//! [`loams_live::LiveTxn`], so reads land in the read set, and a conflict
//! reruns a mutation from scratch.
//!
//! Every call runs in a fresh context with frozen built-ins and the
//! determinism rules of §20 §6.2: `Date` is the call's start timestamp,
//! `Math.random` a stream seeded by the start timestamp and the request id,
//! `crypto.*` throws `DeterminismError`, and there are no timers, `fetch`
//! or `WebAssembly` ([`GLOBALS`] is the allowlist). The interrupt handler
//! enforces [`JsConfig::cpu_limit`] (`FUNCTION_TIMEOUT`) and each runtime
//! [`JsConfig::memory_limit`] (`FUNCTION_OUT_OF_MEMORY`). `console.*` lines
//! are collected per call into [`loams_live::CallOutput`], truncated at
//! [`JsConfig::console_lines`] × [`JsConfig::console_line_bytes`] (D682).
//!
//! The host API (LV1 Task 4): `ctx.db.get/query/insert/patch/replace/
//! delete`, `withIndex`, `order`, `take`, `first`, `collect` and
//! `paginate({ cursor, numItems })`; a function's `args`, built with
//! `loams:server`'s `v`, is a [`Validator`] checked before the handler
//! runs; `internalQuery` and `internalMutation` are
//! [`Visibility::Internal`].
//!
//! **Two isolation modes** (design §45 §3.1, D681; LV1 plan Task 5).
//! [`Isolation::InProcess`] runs the slots on threads of this process. Some
//! of QuickJS's C built-ins loop without polling the interrupt handler, so
//! there the CPU limit cannot be guaranteed against hostile code (LV1 rows
//! T3-7 and T3-10): it serves desktop, `loams dev` and single-tenant
//! deployments, trusted code only. [`Isolation::Isolated`] runs each slot
//! in a sandboxed worker process ([`WorkerPool`], [`WorkerHandle`]; Linux
//! only): seccomp allows only memory, clock and stdio system calls, landlock
//! grants no filesystem or network access, resource limits cap the address
//! space and the open files, and the host kills a worker whose call runs
//! past its CPU limit plus [`KILL_GRACE`]. A worker that crashes fails its
//! call with `live_worker_crashed`. [`worker_main`] is the worker process's
//! entry point (`loams live-worker`).
//!
//! [`JsLoader`] is the runtime of `loams-live`'s deployments (LV1 plan
//! Task 6): it loads a deployment's bundle in either mode, and a bundle on
//! isolated workers reports when they degrade.

//! **The agent profile** (SPIKE, MS1 Task 1, D1807; feature `agent`, off
//! by default): one long-lived isolated worker per running agent, sealed
//! by the same sandbox, with ACP and every capability of the agent
//! environment contract (D1453) as frames to agentd. See the `agent` module.
//!
//! Features: `live` (default) is everything above; `agent` adds the agent
//! profile and compiles without `live`.

// The crate docs above link Live's items, which an agent-only build lacks.
#![cfg_attr(not(feature = "live"), allow(rustdoc::broken_intra_doc_links))]

#[cfg(feature = "agent")]
pub mod agent;
#[cfg(feature = "live")]
mod child;
#[cfg(feature = "live")]
mod host;
#[cfg(any(feature = "live", feature = "agent"))]
mod ipc;
#[cfg(feature = "live")]
mod limits;
#[cfg(feature = "live")]
mod loader;
#[cfg(feature = "live")]
mod runtime;
#[cfg(all(target_os = "linux", any(feature = "live", feature = "agent")))]
mod sandbox_linux;
#[cfg(feature = "live")]
mod validators;
#[cfg(feature = "live")]
mod worker;

#[cfg(feature = "live")]
pub use child::worker_main;
#[cfg(feature = "live")]
pub use loader::JsLoader;
/// Who may call a function (D699).
#[cfg(feature = "live")]
pub use loams_live::Visibility;
/// A function's argument validator (`loams:server`'s `v`), shared with
/// schema validators.
#[cfg(feature = "live")]
pub use loams_live::validate::Validator;
/// Where a bundle's functions run, and whose code a node serves (design
/// §45 §3.1, D681).
#[cfg(feature = "live")]
pub use loams_live::{Isolation, Tenancy};
#[cfg(feature = "live")]
pub use runtime::{Bundle, FunctionMeta, GLOBALS, JsConfig, MAX_BUNDLE_BYTES, MAX_EXPORTS};
#[cfg(feature = "live")]
#[doc(hidden)]
pub use worker::Probe;
#[cfg(feature = "live")]
pub use worker::{KILL_GRACE, WorkerCommand, WorkerHandle, WorkerPool};
