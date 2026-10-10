//! The source walk shared by the strip tests (`no_edge_symbols`,
//! `no_remote_feature_symbols`): every file of the `loams-agentd*` crates
//! with one of the given extensions (plus `Cargo.toml`), and the code part
//! of a line.

// Each test binary uses part of this module.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// One scanned file: its path relative to `crates/`, with `/` separators.
pub struct Source {
    pub rel: String,
    pub path: PathBuf,
}

impl Source {
    pub fn is_manifest(&self) -> bool {
        self.rel.ends_with("Cargo.toml")
    }
}

/// The code part of a line: everything before a `//` comment that is not
/// inside a string literal (so `"ws://127.0.0.1"` stays code). Raw strings
/// (`r"…"`, `r#"…"#`, `br#"…"#`) end only at their own delimiter, and a
/// `'"'` character literal opens no string.
pub fn code(line: &str) -> &str {
    let bytes = line.as_bytes();
    let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'/' if bytes.get(i + 1) == Some(&b'/') => return &line[..i],
            b'r' if i == 0
                || !ident(bytes[i - 1])
                || (bytes[i - 1] == b'b' && (i < 2 || !ident(bytes[i - 2]))) =>
            {
                let hashes = bytes[i + 1..].iter().take_while(|&&b| b == b'#').count();
                if bytes.get(i + 1 + hashes) == Some(&b'"') {
                    let close: Vec<u8> = std::iter::once(b'"')
                        .chain(std::iter::repeat_n(b'#', hashes))
                        .collect();
                    let body = i + 2 + hashes;
                    match bytes[body..].windows(close.len()).position(|w| w == close) {
                        Some(end) => i = body + end + close.len(),
                        // The raw string continues on the next line.
                        None => return line,
                    }
                    continue;
                }
                i += 1;
            }
            b'\'' if bytes.get(i + 1) == Some(&b'"') && bytes.get(i + 2) == Some(&b'\'') => i += 3,
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    line
}

/// True when `pattern` occurs in `line` as a whole identifier: the
/// characters around it are not alphanumeric or `_`.
pub fn contains_word(line: &str, pattern: &str) -> bool {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    line.match_indices(pattern).any(|(at, _)| {
        let before = line[..at].chars().next_back();
        let after = line[at + pattern.len()..].chars().next();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn walk(dir: &Path, extensions: &[&str], skip_fixtures: bool, out: &mut Vec<PathBuf>) {
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
            if name == "target" || (skip_fixtures && name == "fixtures" && in_tests) {
                continue;
            }
            walk(&path, extensions, skip_fixtures, out);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| extensions.contains(&e))
            || path.file_name().is_some_and(|n| n == "Cargo.toml")
        {
            out.push(path);
        }
    }
}

/// The strip tests, which name every pattern they forbid.
const STRIP_TESTS: &[&str] = &[
    "loams-agentd/tests/no_edge_symbols.rs",
    "loams-agentd/tests/no_remote_feature_symbols.rs",
];

/// Every `loams-agentd*` file with one of `extensions`, and every manifest,
/// except the strip tests themselves.
pub fn agentd_sources(extensions: &[&str], skip_fixtures: bool) -> Vec<Source> {
    let root = crates_dir();
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(&root).expect("crates dir").flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with("loams-agentd")
        {
            walk(&entry.path(), extensions, skip_fixtures, &mut paths);
        }
    }
    assert!(!paths.is_empty(), "no loams-agentd* sources under {root:?}");
    paths
        .into_iter()
        .map(|path| Source {
            rel: path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/"),
            path,
        })
        .filter(|source| !STRIP_TESTS.contains(&source.rel.as_str()))
        .collect()
}
