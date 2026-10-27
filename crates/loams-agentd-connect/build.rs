//! Generates `loams.agentd.v1` (design §53 §6.1, plan SD1 Task 4) from
//! `proto/loams/agentd/v1` at the workspace root, with the `loams.errors.v1`
//! detail its errors carry and the `loams.options.v1` annotations it imports,
//! through connect-rust's code generator and `protoc`, as `loams-apps-mock`
//! does. It also writes the descriptor set, which the descriptor tests and the
//! renderer allowlist read.
//!
//! `PROTOC_INCLUDE`, when set, is added to the include path for the
//! well-known types (`google/protobuf/*.proto`); a system `protoc` finds them
//! itself.

/// The proto files, as paths under the workspace's `proto/` directory.
const FILES: &[&str] = &[
    "loams/options/v1/options.proto",
    "loams/errors/v1/errors.proto",
    "loams/agentd/v1/studio.proto",
    "loams/agentd/v1/terminals.proto",
    "loams/agentd/v1/projects.proto",
    "loams/agentd/v1/threads.proto",
    "loams/agentd/v1/queue.proto",
    "loams/agentd/v1/harness.proto",
    "loams/agentd/v1/git.proto",
    "loams/agentd/v1/diff.proto",
    "loams/agentd/v1/files.proto",
    "loams/agentd/v1/preview.proto",
    "loams/agentd/v1/attachments.proto",
    "loams/agentd/v1/review.proto",
    "loams/agentd/v1/approvals.proto",
    "loams/agentd/v1/groups.proto",
    "loams/agentd/v1/notifications.proto",
    "loams/agentd/v1/factory.proto",
    // MK1 Task 4b: the marketplace service and the listing it serves.
    "loams/marketplace/v1/listing.proto",
    "loams/agentd/v1/marketplace.proto",
];

fn main() {
    // Read at run time, not with `env!`: a shared target directory reuses a
    // compiled build script across worktrees, and a path baked in at compile
    // time would point into whichever worktree compiled it first.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .expect("cargo sets CARGO_MANIFEST_DIR for build scripts");
    let root = format!("{manifest_dir}/../../proto");
    let files: Vec<String> = FILES.iter().map(|file| format!("{root}/{file}")).collect();
    for file in &files {
        println!("cargo:rerun-if-changed={file}");
    }
    println!("cargo:rerun-if-env-changed=PROTOC_INCLUDE");
    let mut includes = vec![root];
    if let Ok(include) = std::env::var("PROTOC_INCLUDE")
        && !include.is_empty()
    {
        includes.push(include);
    }
    if let Err(err) = connectrpc_build::Config::new()
        .files(&files)
        .includes(&includes)
        .include_file("_connectrpc.rs")
        .emit_descriptor_set("agentd_descriptor.bin")
        .gate_client_feature(false)
        .compile()
    {
        panic!("generating loams.agentd.v1 failed (is protoc installed?): {err:#}");
    }
}
