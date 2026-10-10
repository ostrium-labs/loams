//! Generates the `loams.live.v1` messages (and the internal
//! `loams.live.worker.v1` ones), the `LiveService` trait and its
//! client from `proto/loams/live/v1/*.proto` at the workspace root (R1 plan
//! Task 7), with connect-rust's code generator and the system `protoc`.

fn main() {
    // Read at run time, not with `env!`: the shared target directory reuses
    // a compiled build script across worktrees, and a path baked in at
    // compile time would point into whichever worktree compiled it first.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .expect("cargo sets CARGO_MANIFEST_DIR for build scripts");
    let root = format!("{manifest_dir}/../../proto");
    let mut files: Vec<String> = ["value", "live", "journal", "catalog", "idempotency"]
        .iter()
        .map(|name| format!("{root}/loams/live/v1/{name}.proto"))
        .collect();
    // The internal worker protocol (LV1 plan Ruling 5, Task 5).
    files.push(format!("{root}/loams/live/worker/v1/worker.proto"));
    println!("cargo::rerun-if-changed=build.rs");
    for file in &files {
        println!("cargo::rerun-if-changed={file}");
    }
    if let Err(err) = connectrpc_build::Config::new()
        .files(&files)
        .includes(&[root])
        .include_file("_connectrpc.rs")
        .gate_client_feature(true)
        .compile()
    {
        panic!("generating the loams.live.v1 code failed (is protoc installed?): {err:#}");
    }
}
