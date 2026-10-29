//! DX1 Task 1's tests: the vendored protos are the pinned tag's, the
//! generated service is the one the API declares, and the licence travels
//! with them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use loams_temporal_proto::temporal::api::history::v1::History;
use prost::Message as _;
use sha2::{Digest, Sha256};

fn crate_dir() -> PathBuf {
    // At run time: the shared target reuses test binaries across worktrees.
    PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"))
}

fn walk(dir: &Path, base: &Path, out: &mut BTreeMap<String, String>) {
    for entry in std::fs::read_dir(dir).expect("readable proto dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            walk(&path, base, out);
        } else {
            let rel = path.strip_prefix(base).expect("under base");
            let bytes = std::fs::read(&path).expect("readable file");
            out.insert(
                rel.to_string_lossy().into_owned(),
                hex::encode(Sha256::digest(&bytes)),
            );
        }
    }
}

#[test]
fn vendored_files_match_manifest() {
    let dir = crate_dir();
    let manifest = std::fs::read_to_string(dir.join("VENDORED.md")).expect("VENDORED.md");
    let listed: BTreeMap<String, String> = manifest
        .split("```")
        .nth(1)
        .expect("a fenced file list")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let (hash, path) = l.split_once("  ").expect("`<sha256>  <path>` lines");
            (path.to_string(), hash.to_string())
        })
        .collect();
    let proto = dir.join("proto");
    let mut actual = BTreeMap::new();
    for sub in ["temporal", "google", "nexusannotations"] {
        walk(&proto.join(sub), &proto, &mut actual);
    }
    assert_eq!(
        listed, actual,
        "proto/ differs from VENDORED.md: vendored files are never edited (see its Updating section)"
    );
}

#[test]
fn workflow_service_method_count() {
    let methods = loams_temporal_proto::workflow_service_methods();
    // temporalio/api v1.63.5 (plan DX1 X1.R1).
    assert_eq!(methods.len(), 123, "{methods:?}");
    for m in [
        "GetSystemInfo",
        "StartWorkflowExecution",
        "PollWorkflowTaskQueue",
        "RespondWorkflowTaskCompleted",
        "PollActivityTaskQueue",
        "SignalWorkflowExecution",
        "QueryWorkflow",
    ] {
        assert!(methods.iter().any(|x| x == m), "{m} missing");
    }
    assert_eq!(
        loams_temporal_proto::service_methods("temporal.api.operatorservice.v1.OperatorService")
            .len(),
        12
    );
}

#[test]
fn licence_and_notice_present() {
    let dir = crate_dir();
    let licence = std::fs::read_to_string(dir.join("LICENSE-temporalio-api")).expect("licence");
    assert!(licence.starts_with("The MIT License"));
    assert!(licence.contains("Temporal Technologies Inc."));
    let notice = std::fs::read_to_string(dir.join("../../NOTICE")).expect("NOTICE");
    assert!(
        notice.contains("crates/loams-temporal-proto/") && notice.contains("temporalio/api"),
        "the root NOTICE names the vendored Temporal API"
    );
}

/// Every binary history of DX0 Task 4's corpus decodes, and what it
/// re-encodes to decodes to the same message: the generated types lose
/// nothing the server wrote. (Bytes may differ only in map entry order, which
/// Go's encoder does not fix.)
#[test]
fn corpus_binpb_roundtrip() {
    let corpus = crate_dir().join("../../conformance/compute/temporal/corpus/histories");
    if !corpus.is_dir() {
        eprintln!("skipped: no committed corpus at {}", corpus.display());
        return;
    }
    let mut seen = 0;
    for lang in ["go", "ts"] {
        let Ok(dir) = std::fs::read_dir(corpus.join(lang)) else {
            continue;
        };
        for entry in dir {
            let path = entry.expect("entry").path();
            if path.extension().is_some_and(|x| x == "binpb") {
                let bytes = std::fs::read(&path).expect("readable");
                let history = History::decode(bytes.as_slice())
                    .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                assert!(!history.events.is_empty(), "{}", path.display());
                let again =
                    History::decode(history.encode_to_vec().as_slice()).expect("re-decodes");
                assert_eq!(again, history, "{} loses data", path.display());
                assert_eq!(
                    again.encoded_len(),
                    bytes.len(),
                    "{}: unknown fields were dropped",
                    path.display()
                );
                seen += 1;
            }
        }
    }
    eprintln!("{seen} corpus histories round-tripped");
}
