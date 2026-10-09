//! The edge is gone (plan DD1 Task 2, D781): no `loams-agentd*` source may
//! still name the edge sync, the device-room relay or preview signaling.
//!
//! The patterns follow ruling T0-12: `chat2` alone names fields of the stored
//! Loro doc schema, so only the edge client names are matched; `/blob/` also
//! appears in GitHub URLs the harness crate installs from, so it is checked
//! outside `loams-agentd-harness` only.

use std::path::{Path, PathBuf};

const PATTERNS: &[&str] = &[
    "edge_url",
    "EdgeConfig",
    "device_room",
    "chat2_host",
    "ChatClient",
    "chat_client",
    "chat2_live",
    "HostRelay",
    "LinkCache",
    "/blob/",
    "edge.loams.invalid",
];

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let in_tests = path
                .parent()
                .and_then(|p| p.file_name())
                .is_some_and(|p| p == "tests");
            if name == "target" || (name == "fixtures" && in_tests) {
                continue;
            }
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs")
            || path.file_name().is_some_and(|n| n == "Cargo.toml")
        {
            out.push(path);
        }
    }
}

#[test]
fn no_edge_symbols() {
    let root = crates_dir();
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&root).expect("crates dir").flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with("loams-agentd") {
            sources(&entry.path(), &mut files);
        }
    }
    assert!(!files.is_empty(), "no loams-agentd* sources under {root:?}");
    let this_file = Path::new(file!())
        .file_name()
        .expect("test file name")
        .to_owned();

    let mut hits = Vec::new();
    for file in files {
        let rel = file
            .strip_prefix(&root)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        if rel.starts_with("loams-agentd/tests/") && file.file_name() == Some(&this_file) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for (n, line) in text.lines().enumerate() {
            for pattern in PATTERNS {
                if *pattern == "/blob/" && rel.starts_with("loams-agentd-harness/") {
                    continue;
                }
                if line.contains(pattern) {
                    hits.push(format!("{rel}:{}: {pattern}", n + 1));
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "{} edge symbol(s) remain:\n{}",
        hits.len(),
        hits.join("\n")
    );
}
