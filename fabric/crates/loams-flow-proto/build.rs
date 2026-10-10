//! Generates the `loams.flow.v1` messages from `fabric/proto/loams/flow/v1/`
//! with connect-rust's code generator and the system `protoc`, as
//! `crates/loams-live-proto/build.rs` does for `proto/loams/live/v1/`
//! (R1 plan Task 7).

fn main() {
    // The proto root is the workspace's `fabric/proto`, a sibling of the engine
    // workspace's root `proto/` rather than a subtree of it: the `fabric/`
    // workspace is a separate cargo workspace (FL1 plan Task 1), and this crate
    // sits at `fabric/crates/loams-flow-proto`, so its root is two levels up.
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../proto");
    // The manifest types of CN1 Task 1. CN1 Task 3 adds `flow`, which owns
    // `FlowService`; the list grows there.
    let files: Vec<String> = ["connector", "instance"]
        .iter()
        .map(|name| format!("{root}/loams/flow/v1/{name}.proto"))
        .collect();
    if let Err(err) = connectrpc_build::Config::new()
        .files(&files)
        .includes(&[root])
        .include_file("_connectrpc.rs")
        .gate_client_feature(true)
        .compile()
    {
        panic!("generating the loams.flow.v1 code failed (is protoc installed?): {err:#}");
    }
}
