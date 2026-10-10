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
//! **Trusted code only.** The slots run in this process, and some of
//! QuickJS's C built-ins loop without polling the interrupt handler, so the
//! CPU limit cannot be guaranteed against hostile code (LV1 rows T3-7 and
//! T3-10). In-process mode serves desktop, `loams dev` and single-tenant
//! deployments; multi-tenant serving needs LV1 Task 5's isolated worker and
//! its wall-clock kill.

mod host;
mod limits;
mod runtime;
mod validators;

/// Who may call a function (D699).
pub use loams_live::Visibility;
/// A function's argument validator (`loams:server`'s `v`), shared with
/// schema validators.
pub use loams_live::validate::Validator;
pub use runtime::{Bundle, FunctionMeta, GLOBALS, JsConfig, MAX_BUNDLE_BYTES, MAX_EXPORTS};
