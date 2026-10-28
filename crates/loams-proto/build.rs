//! Generates the public `loams.*.v1` packages of the unified Connect API
//! (design §44 §7–§8) from `proto/` at the workspace root, with
//! connect-rust's code generator and the system `protoc`, and writes their
//! descriptor set for `grpc.reflection.v1`.
//!
//! API1 Task 1 starts the list with the three packages the server needs
//! before any application service exists: the facade options the SDK
//! generator reads (`loams.options.v1`), the shared error detail and the
//! instance discovery service. Task 2 adds `loams.collection.v1` (the
//! namespaces, collections, aliases, versions, scan plans and hot tier), and
//! Task 3 adds the document half of the *same* package — the write, the three
//! reads and the two filter writes — as a second file, which is where §44 §7.2
//! and §8 put it. Task 4 adds the third — the search IR and its answer, i.e.
//! §8's `Query` — and `page_token` on `ScrollDocumentsRequest`. Tasks 5–8
//! append `loams.sql.v1`, `loams.link.v1`, `loams.admin.v1`, `loams.auth.v1`
//! and `loams.internal.v1`. GR1 Task 1 adds `loams.graph.v1` (design §48),
//! PG2 Task 1 `loams.postgres.v1` (design §46 §4), GT1 Task 12
//! `loams.repos.v1` (design §36 §19.10), and MM1 Task 1 `loams.memory.v1`
//! (design §52 §9.1).
//!
//! `loams.live.v1` is **not** here: R1's `loams-live-proto` already
//! generates it, and a proto package's Rust types are generated exactly
//! once in this workspace. The server reaches the `LiveService` trait through
//! that crate; `loams-proto` is the crate the SDKs of Task 2 onwards build
//! on.

/// The proto files, as paths under the workspace's `proto/` directory.
const FILES: &[&str] = &[
    "loams/options/v1/options.proto",
    "loams/errors/v1/errors.proto",
    "loams/instance/v1/instance.proto",
    "loams/collection/v1/collection.proto",
    "loams/collection/v1/document.proto",
    "loams/collection/v1/query.proto",
    // `loams.graph.v1`'s slow RPCs answer an `Operation` (GR1 Task 2).
    // `loams-apps-mock` generates this package too, for its own server, as
    // both crates already do for `loams.errors.v1`.
    "loams/operations/v1/operations.proto",
    // Loams Graph (design §48, D741, D746): moved from `fabric/proto/` by
    // GR1 Task 1 and reworked by Task 2. Its Rust types used to come from
    // `fabric/`'s `loams-graph-proto`.
    "loams/graph/v1/graph.proto",
    // Loams Postgres (design §46 §4, PG2 Task 1). The types only: the
    // service is served by `pg-control` behind the `loams` feature
    // `postgres` (PG2 Task 9), so the default server does not register it.
    "loams/postgres/v1/postgres.proto",
    // Loams Git (design §36 §19.10, GT1 Task 12, ruling T0-10): the types are
    // always generated; `RepoService` is served by the `loams` feature `git`.
    // `workspaces`, `objects` and `env` are empty stubs that GT1 Tasks 25, 18
    // and 61 fill in, so the package is registered here once.
    "loams/repos/v1/repos.proto",
    "loams/repos/v1/workspaces.proto",
    "loams/repos/v1/objects.proto",
    "loams/repos/v1/env.proto",
    // Loams Memory (design §52 §9.1, MM1 Task 1): the types are always
    // generated; the services are served by the `loams` feature `memory`
    // from MM1 Task 11.
    "loams/memory/v1/memory.proto",
    // The console API (API2 Tasks 6 and 7): the types and clients the Rust
    // SDK's facade binds. The server registers neither package yet; its
    // `connect_admin.rs` and `connect_auth.rs` are the tasks' server half.
    "loams/admin/v1/common.proto",
    "loams/admin/v1/org.proto",
    "loams/admin/v1/project.proto",
    "loams/admin/v1/agent.proto",
    "loams/admin/v1/key.proto",
    "loams/admin/v1/audit.proto",
    "loams/auth/v1/auth.proto",
    "loams/sql/v1/sql.proto",
    "loams/stream/v1/stream.proto",
    // Loams Meter (design §63 §17.1, ME1 Tasks 10 and 11): the customer,
    // entitlement and plan services, served by the `meter` role (`loams-meter`).
    "loams/meter/v1/customer.proto",
    "loams/meter/v1/entitlement.proto",
    "loams/meter/v1/plan.proto",
    // ME1 Task 8: MeterService.QueryMeter over the House windows.
    "loams/meter/v1/meter.proto",
    // ME1 Task 14: webhooks, rules and deliveries.
    "loams/meter/v1/notification.proto",
    // ME1 Task 17: internal, between gates and counter owners.
    "loams/meter/v1/lease.proto",
];

fn main() {
    // Read at run time, not with `env!`: the shared target directory reuses
    // a compiled build script across worktrees, and a path baked in at
    // compile time would point into whichever worktree compiled it first.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .expect("cargo sets CARGO_MANIFEST_DIR for build scripts");
    // `proto/` inside the crate: a symlink to the workspace's `proto/` in this
    // repository, real files in the published crate. A checkout without symlink
    // support (Windows with `core.symlinks=false`) falls back to the workspace.
    let packaged = format!("{manifest_dir}/proto");
    let root = if std::path::Path::new(&packaged).join("loams").is_dir() {
        packaged
    } else {
        format!("{manifest_dir}/../../proto")
    };
    let files: Vec<String> = FILES.iter().map(|f| format!("{root}/{f}")).collect();
    for file in &files {
        println!("cargo:rerun-if-changed={file}");
    }
    if let Err(err) = connectrpc_build::Config::new()
        .files(&files)
        .includes(&[root])
        .include_file("_connectrpc.rs")
        .emit_descriptor_set("loams_api_descriptor.bin")
        .gate_client_feature(true)
        .compile()
    {
        panic!("generating the loams API code failed (is protoc installed?): {err:#}");
    }
}
