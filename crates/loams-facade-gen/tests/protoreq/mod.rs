//! The `CodeGeneratorRequest` `buf generate` would hand the plugin, built from
//! the repository's real `proto/` tree.
//!
//! The generator's unit tests are hermetic (`tests/support` writes descriptors
//! by hand), but the golden tests must run over the protos that ship, or a
//! golden file only pins the fixture. So this module runs `protoc` over
//! `proto/`, exactly as the buf template's single input directory does, and
//! re-tags the resulting descriptor set as a request.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// The repository root, from the crate's manifest directory.
pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The request, built once: `protoc` runs for every golden test and the
/// descriptor set does not change within a run.
pub fn code_generator_request() -> &'static [u8] {
    static REQUEST: OnceLock<Vec<u8>> = OnceLock::new();
    REQUEST.get_or_init(build_request)
}

fn build_request() -> Vec<u8> {
    let root = root();
    let proto = root.join("proto");
    let out = std::env::temp_dir().join(format!("loams-facade-golden-{}.bin", std::process::id()));
    let status = Command::new("protoc")
        .current_dir(&root)
        .arg(format!("--proto_path={}", proto.display()))
        .arg("--include_imports")
        .arg(format!("--descriptor_set_out={}", out.display()))
        .args(proto_files(&proto.join("loams")))
        .status()
        .expect("protoc is on PATH (the crate's own build.rs needs it too)");
    assert!(status.success(), "protoc failed with {status}");
    let bytes = std::fs::read(&out).expect("the descriptor set");
    let _ = std::fs::remove_file(&out);
    wrap_in_request(&bytes)
}

/// Every `.proto` under a directory, as paths relative to `proto/`, sorted so
/// protoc's input order does not depend on the filesystem.
fn proto_files(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in std::fs::read_dir(&current).expect("read the proto tree") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "proto")
            {
                out.push(
                    path.strip_prefix(dir.parent().expect("proto/"))
                        .expect("under proto/")
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }
    out.sort();
    assert!(!out.is_empty(), "no protos under {}", dir.display());
    out
}

/// `FileDescriptorSet` (field 1 per file) to `CodeGeneratorRequest` (field 15
/// per file). The two differ only in the field number, which is why this is a
/// re-tag rather than a re-parse.
fn wrap_in_request(descriptor_set: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < descriptor_set.len() {
        let (tag, next) = read_varint(descriptor_set, at);
        at = next;
        let number = tag >> 3;
        let wire = tag & 0x07;
        assert_eq!(wire, 2, "a descriptor set is all length-delimited");
        let (len, next) = read_varint(descriptor_set, at);
        at = next;
        let payload = &descriptor_set[at..at + len as usize];
        at += len as usize;
        if number == 1 {
            push_bytes(&mut out, 15, payload);
        }
    }
    out
}

fn read_varint(bytes: &[u8], mut at: usize) -> (u64, usize) {
    let mut value = 0u64;
    let mut shift = 0;
    loop {
        let byte = bytes[at];
        at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return (value, at);
        }
        shift += 7;
    }
}

fn push_bytes(out: &mut Vec<u8>, number: u32, payload: &[u8]) {
    push_varint(out, (u64::from(number) << 3) | 2);
    push_varint(out, payload.len() as u64);
    out.extend_from_slice(payload);
}

fn push_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}
