//! Generates the `loams.graph.v1` types from the workspace's own proto tree.
//!
//! Modelled on `loams-flow-proto`'s build script, and deliberately separate from it: Loams's Flow
//! and Graph control planes are different contracts and a bump to one should not regenerate the
//! other. The proto root is the `fabric/proto/` directory this workspace owns (D634), not the
//! engine's root `proto/` tree, because `loams.graph.v1` describes an engine Loams links into its
//! own process rather than a service the engine exposes.

use std::path::PathBuf;

fn main() {
    let root: PathBuf = [env!("CARGO_MANIFEST_DIR"), "..", "..", "proto"]
        .iter()
        .collect();
    let file = root.join("loams/graph/v1/graph.proto");
    println!("cargo:rerun-if-changed={}", file.display());
    println!("cargo:rerun-if-env-changed=PROTOC");

    if let Err(err) = connectrpc_build::Config::new()
        .files(&[file])
        .includes(&[root])
        // Gating the client behind a feature, as `crates/loams-live-proto` does for `LiveService`:
        // the server trait and the message types are always generated, the connect-rust client only
        // when something asks for it.
        .gate_client_feature(true)
        .compile()
    {
        panic!("{err:#}");
    }
}
