//! The generated `loams.graph.v1` contract (design §33 D634).
//!
//! Loams's graph surface is **GQL**, the ISO standard (ISO/IEC 39075), reached over **Connect-RPC**
//! — the protocol every other Loams service already speaks. The engine is Grafeo, embedded in the
//! Fabric process rather than reached over a network, so there is **no Bolt on this path** and no
//! second deployment unit.
//!
//! D634 (a) explains why GQL rather than Cypher, and D634 (b) why embedding rather than running a
//! server. The short version: Cypher is one vendor's dialect, and Bolt's Rust ecosystem has been
//! frozen since 2022, so targeting the standard over Loams's own existing RPC is what keeps this
//! portable and removes the second protocol.
//!
//! Unlike `loams-flow-proto`, this crate's proto **does** declare a service — `GraphService`,
//! D634 — so the generated `__connect.rs` carries the server trait and, behind the `client`
//! feature gated in `build.rs`, the connect-rust client.

// The generated connect-rust service types carry no `Debug`, which this workspace's
// `missing_debug_implementations` lint warns about. They are code buffa emits, not code Loams
// writes, so the lint is silenced for them rather than worked around by hand-implementing `Debug`
// on a foreign type -- the same narrow allow `crates/loams-live-proto` uses for `LiveService`.
#![allow(missing_debug_implementations)]
// buffa writes its own allow list onto the `__buffa` module it generates — and that list already
// contains `derivable_impls` and `match_single_binding` — but it `include!`s two of its files at the
// top level instead, outside the module the list is attached to: `loams.graph.v1.graph.rs` and, on
// account of the service, `loams.graph.v1.graph.__connect.rs`. Two lints therefore land on generated
// code with nothing to silence them, and CI's `-D warnings` turns both into build failures:
// `derivable_impls` on the `QueryLanguage` default impl (which the generator emits next to a
// `#[derive(Default)]` on the same enum), and `match_single_binding` on the router's method dispatch.
// Both belong to the generator rather than to this crate, so they are allowed here. Editing `OUT_DIR`
// instead would be undone by the next `cargo clean`.
#![allow(clippy::derivable_impls, clippy::match_single_binding)]

// `connectrpc-build` 0.9 is buffa-based, and it emits one file per proto plus a `<package>.mod.rs`
// that roots them -- not the `_connectrpc.rs` bundle `connectrpc::include_generated!()` looks for. So
// the entry point is the generated mod file. This is the same generator `loams-flow-proto` uses;
// only the service half is new here, in `loams.graph.v1.graph.__connect.rs`.
mod generated {
    include!(concat!(env!("OUT_DIR"), "/loams.graph.v1.mod.rs"));
}

pub use generated::*;
