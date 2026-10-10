//! Generates the app protos' messages, service traits and clients from
//! `proto/loams/{instance,devices,approvals,operations,notifications,errors}/v1`
//! and `loams.graph.v1` (with the `loams.options.v1` it imports; GR1 Task 7)
//! at the workspace root (design §37 §8.2), with connect-rust's code
//! generator and the system `protoc`.

fn main() {
    // Read at run time, not with `env!`: the shared target directory reuses
    // a compiled build script across worktrees, and a path baked in at
    // compile time would point into whichever worktree compiled it first.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .expect("cargo sets CARGO_MANIFEST_DIR for build scripts");
    let root = format!("{manifest_dir}/../../proto");
    let files: Vec<String> = [
        "errors/v1/errors",
        "instance/v1/instance",
        "devices/v1/devices",
        "approvals/v1/approvals",
        "operations/v1/operations",
        "notifications/v1/notifications",
        "options/v1/options",
        "graph/v1/graph",
    ]
    .iter()
    .map(|name| format!("{root}/loams/{name}.proto"))
    .collect();
    for file in &files {
        println!("cargo:rerun-if-changed={file}");
    }
    if let Err(err) = connectrpc_build::Config::new()
        .files(&files)
        .includes(&[root])
        .include_file("_connectrpc.rs")
        .gate_client_feature(false)
        .compile()
    {
        panic!("generating the app proto code failed (is protoc installed?): {err:#}");
    }
}
