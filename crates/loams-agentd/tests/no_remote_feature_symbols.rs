//! WorkOS, the Cursor shim, self-update and push are gone (plan DD1 Task 3,
//! D781): no `loams-agentd*` source, manifest or script may still name them.
//!
//! Matching is case-sensitive, so a comment may still say "WorkOS" when it
//! records what was removed; `Updater` and `UpdateStatus` match whole words
//! only (ruling T0-12), so `HarnessUpdateStatus` stays. Fixtures are scanned
//! too: the Cursor shim's fakes go with it.

mod source_scan;

use source_scan::contains_word;

/// Matched anywhere in a line.
const PATTERNS: &[&str] = &[
    // WorkOS sign-in (T0-2).
    "workos",
    "WORKOS",
    // The Cursor shim and its SDK pin.
    "cursor_sdk",
    "@cursor/sdk",
    "CursorHarness",
    "HarnessId::Cursor",
    // Self-update and the harness-update policies (T0-1).
    "ApplyUpdate",
    "SetHarnessUpdatePolicy",
    "HarnessUpdatePolicy",
    // Push: the nudge, the command relay and liveness (T0-3).
    "Nudge",
    "RelayCommand",
    "FocusChat",
    "ProbeSync",
    "WatchConnectivity",
    "WatchTransfers",
];

/// Matched as whole words.
const WORDS: &[&str] = &["Updater", "UpdateStatus"];

/// `(pattern, file)` pairs that may match: the test that checks the removed
/// methods are unknown.
const ALLOWED: &[(&str, &str)] = &[
    ("RelayCommand", "loams-agentd-sessions/tests/local_first.rs"),
    ("FocusChat", "loams-agentd-sessions/tests/local_first.rs"),
    ("ProbeSync", "loams-agentd-sessions/tests/local_first.rs"),
    (
        "WatchConnectivity",
        "loams-agentd-sessions/tests/local_first.rs",
    ),
    (
        "WatchTransfers",
        "loams-agentd-sessions/tests/local_first.rs",
    ),
    ("ApplyUpdate", "loams-agentd-sessions/tests/local_first.rs"),
    ("UpdateStatus", "loams-agentd-sessions/tests/local_first.rs"),
    (
        "HarnessUpdatePolicy",
        "loams-agentd-sessions/tests/local_first.rs",
    ),
    (
        "SetHarnessUpdatePolicy",
        "loams-agentd-sessions/tests/local_first.rs",
    ),
];

#[test]
fn no_remote_feature_symbols() {
    let extensions = ["rs", "mjs", "js", "ts", "py", "sh"];
    let mut hits = Vec::new();
    for source in source_scan::agentd_sources(&extensions, false) {
        let rel = source.rel.as_str();
        let Ok(text) = std::fs::read_to_string(&source.path) else {
            continue;
        };
        for (n, line) in text.lines().enumerate() {
            let found = PATTERNS
                .iter()
                .filter(|pattern| line.contains(**pattern))
                .chain(WORDS.iter().filter(|word| contains_word(line, word)));
            for pattern in found {
                if !ALLOWED.contains(&(pattern, rel)) {
                    hits.push(format!("{rel}:{}: {pattern}", n + 1));
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "{} remote-feature symbol(s) remain:\n{}",
        hits.len(),
        hits.join("\n")
    );
}

#[test]
fn whole_word_matching() {
    assert!(contains_word("let u = Updater::new();", "Updater"));
    assert!(contains_word("methods::UpdateStatus", "UpdateStatus"));
    assert!(!contains_word("HarnessUpdateStatus", "UpdateStatus"));
    assert!(!contains_word("UpdateStatusRow", "UpdateStatus"));
    assert!(!contains_word("update_status", "UpdateStatus"));
}
