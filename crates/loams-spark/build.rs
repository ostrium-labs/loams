//! Generates the Spark Connect API (`spark.connect`, vendored from
//! `apache/spark` v4.1.3 under `proto/`, see `VENDORED.md`) with prost and
//! tonic, using the system `protoc`, and writes the descriptor set the crate
//! exposes. The loams-temporal-proto pattern.
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
    protos(&root.join("spark"), &mut files);
    println!("cargo:rerun-if-changed={}", root.display());

    let out_dir = PathBuf::from(std::env::var("OUT_DIR")?);
    tonic_prost_build::configure()
        // Arrow batches and artifacts can be megabytes: share, don't copy.
        .bytes(".")
        // The server is only the tests' fake backend; the proxy forwards
        // bytes and never implements the service itself (D1994).
        .build_server(true)
        .build_client(true)
        .generate_default_stubs(true)
        .file_descriptor_set_path(out_dir.join("spark_connect_descriptor.bin"))
        .include_file("_includes.rs")
        .compile_protos(&files, &[root])?;
    Ok(())
}
