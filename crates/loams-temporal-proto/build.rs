//! Generates the Temporal API (`temporal.api.*.v1`, vendored from
//! `temporalio/api` v1.63.5 under `proto/`, see `VENDORED.md`) and the
//! gateway's own `loams.temporal.internal.v1` with prost and tonic, using the
//! system `protoc`, and writes the descriptor set the crate exposes.
//!
//! The proto root is read from `CARGO_MANIFEST_DIR` at run time, not baked in
//! at compile time: the shared target directory reuses a compiled build
//! script across worktrees.

use std::path::{Path, PathBuf};

fn protos(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .map(|e| e.expect("directory entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            protos(&path, out);
        } else if path.extension().is_some_and(|x| x == "proto") {
            out.push(path);
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")?;
    let root = PathBuf::from(&manifest_dir).join("proto");
    let mut files = Vec::new();
    protos(&root.join("temporal"), &mut files);
    protos(&root.join("loams"), &mut files);
    println!("cargo:rerun-if-changed={}", root.display());

    let out_dir = PathBuf::from(std::env::var("OUT_DIR")?);
    tonic_prost_build::configure()
        // Payloads are opaque and can be megabytes: share, don't copy.
        .bytes(".")
        .build_server(true)
        .build_client(true)
        // Methods outside the gateway's set answer UNIMPLEMENTED by default
        // (the frontend also rewrites the message to link the matrix).
        .generate_default_stubs(true)
        .file_descriptor_set_path(out_dir.join("temporal_descriptor.bin"))
        .include_file("_includes.rs")
        .compile_protos(&files, &[root])?;
    Ok(())
}
