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

mod limits;
mod runtime;

pub use runtime::{
    Bundle, FunctionMeta, GLOBALS, JsConfig, MAX_BUNDLE_BYTES, MAX_EXPORTS, Validator, Visibility,
};
