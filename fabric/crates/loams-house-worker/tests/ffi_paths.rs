//! `no_hanging_ffi_paths_called` (HS1 Task 2, design §49 §13.3): the two Arrow
//! paths FL2 measured to hang at chDB 26.9.0 — `chdb_arrow_scan` (Arrow input)
//! and `chdb_stream_query_arrow` (streaming Arrow output), and the `loams-chdb`
//! API over them — are never called in production code.
//!
//! They stay defined in `loams-chdb-sys` (the generated bindings and their thin
//! wrappers) and `loams-chdb` (the API, with `arrow.rs` and `session.rs` its only
//! definers), whose own tests run them in a child process to keep the defect
//! measured. Every other source file in the `fabric/` workspace, the worker
//! included, must not name them. `Arrow` and `ArrowStream` as **output format
//! names** are fine: those go through the byte path, which works (FL2 Ruling 14).

use std::path::{Path, PathBuf};

/// The names, assembled so this file does not match itself.
fn forbidden() -> Vec<String> {
    [
        ("chdb_arrow", "_scan"),
        ("chdb_stream_query", "_arrow"),
        ("chdb_query", "_arrow"),
        ("execute", "_arrow"),
        ("register", "_arrow"),
        ("scan", "_arrow"),
        ("arrow", "_stream("),
        ("Arrow", "Handle"),
        ("loams_chdb::", "arrow"),
        ("loams_chdb::Arrow", "Stream"),
    ]
    .iter()
    .map(|(a, b)| format!("{a}{b}"))
    .collect()
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_hanging_ffi_paths_called() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("fabric/crates")
        .to_path_buf();
    let mut files = Vec::new();
    rust_files(&crates, &mut files);
    assert!(
        files.len() > 20,
        "the walk found the workspace: {}",
        files.len()
    );

    let names = forbidden();
    let mut offenders = Vec::new();
    for file in &files {
        let relative = file.strip_prefix(&crates).unwrap_or(file);
        let krate = relative
            .components()
            .next()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .unwrap_or_default();
        let allowed = match krate.as_str() {
            // The bindings and their wrappers.
            "loams-chdb-sys" => true,
            // The API's definers and re-export, and the crate's own tests.
            "loams-chdb" => {
                let rest = relative.strip_prefix("loams-chdb").unwrap_or(relative);
                rest.starts_with("tests")
                    || [
                        Path::new("src/arrow.rs"),
                        Path::new("src/session.rs"),
                        Path::new("src/lib.rs"),
                    ]
                    .contains(&rest)
            }
            _ => false,
        };
        // This file names them in its documentation.
        if allowed || relative == Path::new("loams-house-worker/tests/ffi_paths.rs") {
            continue;
        }
        let text = std::fs::read_to_string(file).unwrap_or_default();
        for (n, line) in text.lines().enumerate() {
            for name in &names {
                if line.contains(name.as_str()) {
                    offenders.push(format!("{}:{}: {name}", relative.display(), n + 1));
                }
            }
        }
    }
    // Not vacuous: the names are where they are defined.
    let arrow =
        std::fs::read_to_string(crates.join("loams-chdb/src/session.rs")).unwrap_or_default();
    assert!(
        names.iter().any(|name| arrow.contains(name.as_str())),
        "the scan no longer finds the API it guards; update `forbidden`"
    );
    assert!(
        offenders.is_empty(),
        "the hanging Arrow FFI paths are referenced outside loams-chdb(-sys):\n{}",
        offenders.join("\n")
    );
}
