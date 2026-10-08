//! `no_hanging_ffi_paths_called` (HS1 Task 2, design §49 §13.3): the two Arrow
//! paths FL2 measured to hang at chDB 26.9.0 — `chdb_arrow_scan` (Arrow input)
//! and `chdb_stream_query_arrow` (streaming Arrow output), and the `loams-chdb`
//! API over them — are never called in production code.
//!
//! They stay defined in `loams-chdb-sys` (the generated bindings and their thin
//! wrappers) and `loams-chdb` (the API), whose own tests run them in a child
//! process to keep the defect measured. Inside those two crates only the Arrow
//! items themselves may name them (review M8); every other line of the `fabric/`
//! workspace, the worker included, must not. `Arrow` and `ArrowStream` as **output format
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

/// Which lines of a defining crate's source may name the paths (review M8): the
/// lines of an item — `fn`, `impl`, `struct`, `mod` — whose header names Arrow
/// (the definitions), comments, the `arrow` module's re-export, and an inline
/// `#[cfg(test)] mod tests`. A call from any other item is an offender, in these
/// crates as anywhere else.
fn arrow_definition_lines(text: &str) -> Vec<bool> {
    let mut allowed = Vec::new();
    let mut depth: i64 = 0;
    // The depth an allowed item started at, while inside it.
    let mut inside: Option<i64> = None;
    let mut cfg_test = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        let is_item = [
            "fn ",
            "impl",
            "struct ",
            "mod ",
            "pub ",
            "pub(crate) ",
            "unsafe ",
        ]
        .iter()
        .any(|p| trimmed.starts_with(p))
            && [" fn ", "fn ", "impl", "struct ", "mod "]
                .iter()
                .any(|k| trimmed.contains(k));
        let names_arrow = trimmed.to_lowercase().contains("arrow");
        let tests_mod = cfg_test && trimmed.starts_with("mod tests");
        if inside.is_none() && ((is_item && names_arrow) || tests_mod) {
            inside = Some(depth);
        }
        cfg_test = trimmed.starts_with("#[cfg(test)]");
        let comment = trimmed.starts_with("//");
        let reexport = trimmed.starts_with("pub use arrow::")
            || trimmed.starts_with("pub mod arrow")
            || (trimmed.starts_with("use ") && trimmed.contains("arrow::"));
        allowed.push(inside.is_some() || comment || reexport);
        if !comment {
            depth += line.matches('{').count() as i64 - line.matches('}').count() as i64;
        }
        if let Some(start) = inside {
            // The item ends when its braces close, or at its `;` if it had none.
            if depth <= start && (line.contains('}') || line.trim_end().ends_with(';')) {
                inside = None;
            }
        }
    }
    allowed
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
        // This file names them in its documentation.
        if relative == Path::new("loams-house-worker/tests/ffi_paths.rs") {
            continue;
        }
        let krate = relative
            .components()
            .next()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .unwrap_or_default();
        let defining = krate == "loams-chdb" || krate == "loams-chdb-sys";
        // The defining crates' own tests run the paths in a child process to keep
        // the defect measured.
        if defining
            && relative
                .components()
                .nth(1)
                .is_some_and(|c| c.as_os_str() == "tests")
        {
            continue;
        }
        let text = std::fs::read_to_string(file).unwrap_or_default();
        let allowed = if relative == Path::new("loams-chdb/src/arrow.rs") {
            // The Arrow module itself: every item in it is an Arrow definition.
            vec![true; text.lines().count()]
        } else if defining {
            arrow_definition_lines(&text)
        } else {
            vec![false; text.lines().count()]
        };
        for (n, line) in text.lines().enumerate() {
            if allowed[n] {
                continue;
            }
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

/// Review M8: in a defining crate, only the Arrow items may name the paths. A call
/// from any other method of the same file is an offender.
#[test]
fn only_arrow_items_are_allowed_in_the_defining_crates() {
    let call = forbidden()
        .into_iter()
        .find(|n| n.starts_with("execute"))
        .expect("the method name");
    let source = format!(
        "impl Session {{\n    \
             pub fn {call}(&self, sql: &str) -> R {{\n        \
                 self.{call}_with_id(sql)\n    \
             }}\n\n    \
             pub fn run(&self) {{\n        \
                 let _ = self.{call}(\"SELECT 1\");\n    \
             }}\n\
         }}\n"
    );
    let allowed = arrow_definition_lines(&source);
    let lines: Vec<&str> = source.lines().collect();
    let flagged: Vec<&str> = lines
        .iter()
        .zip(&allowed)
        .filter(|(line, ok)| !**ok && line.contains(call.as_str()))
        .map(|(line, _)| *line)
        .collect();
    assert_eq!(
        flagged,
        vec![format!("        let _ = self.{call}(\"SELECT 1\");").as_str()],
        "the definition's own body is allowed, the call from `run` is not"
    );
}
