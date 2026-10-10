//! The edge is gone (plan DD1 Task 2, D781): no `loams-agentd*` source may
//! still name the edge sync, the device-room relay or preview signaling.
//!
//! The patterns follow ruling T0-12: `chat2` alone names fields of the stored
//! Loro doc schema, so only the edge client names are matched; `/blob/` also
//! appears in GitHub URLs the harness crate installs from, so it is checked
//! outside `loams-agentd-harness` only.
//!
//! Fix round 1 (M4) adds: `webrtc` in the manifests; the removed methods
//! `RelayCommand` and `WatchDevices` (named only by the test that checks they
//! are unknown); WorkOS in code (comments may still say what was removed);
//! and no WebSocket client that dials anything but loopback outside the
//! crates that talk to providers and git hosts.

mod source_scan;

use source_scan::code;

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
    "RelayCommand",
    "WatchDevices",
];

/// Patterns matched in `Cargo.toml` files only.
const MANIFEST_PATTERNS: &[&str] = &["webrtc"];

/// `(pattern, file)` pairs that may match: the test that checks the removed
/// methods are unknown names them.
const ALLOWED: &[(&str, &str)] = &[
    ("RelayCommand", "loams-agentd-sessions/tests/local_first.rs"),
    ("WatchDevices", "loams-agentd-sessions/tests/local_first.rs"),
];

/// WebSocket client calls. `loams_agentd_rpc::connect_ws` refuses any host
/// but loopback, so its callers are not listed.
const WS_CLIENT_CALLS: &[&str] = &["connect_async(", "client_async("];

/// Markers that show a WebSocket client dials loopback: one must appear in
/// the code (not the comments) of the call's line or the eight lines before.
const LOOPBACK_MARKERS: &[&str] = &["127.0.0.1", "[::1]", "localhost", "loopback"];

/// Code that may dial remote WebSockets: agent-account sign-ins, harness
/// release checks, source control and the link crate.
const WS_ALLOWED: &[&str] = &[
    "loams-agentd-sessions/src/agent_accounts",
    "loams-agentd-sessions/src/harness_updates",
    "loams-agentd-sessions/src/repos",
    "loams-agentd-sessions/src/source_control",
    "loams-agentd-link/",
];

#[test]
fn no_edge_symbols() {
    let mut hits = Vec::new();
    for source in source_scan::agentd_sources(&["rs"], true) {
        let rel = source.rel.as_str();
        let Ok(text) = std::fs::read_to_string(&source.path) else {
            continue;
        };
        let manifest = source.is_manifest();
        let lines: Vec<&str> = text.lines().collect();
        for (n, line) in lines.iter().enumerate() {
            for pattern in PATTERNS {
                if *pattern == "/blob/" && rel.starts_with("loams-agentd-harness/") {
                    continue;
                }
                if ALLOWED.contains(&(pattern, rel)) {
                    continue;
                }
                if line.contains(pattern) {
                    hits.push(format!("{rel}:{}: {pattern}", n + 1));
                }
            }
            if manifest {
                for pattern in MANIFEST_PATTERNS {
                    if line.contains(pattern) {
                        hits.push(format!("{rel}:{}: {pattern}", n + 1));
                    }
                }
                continue;
            }
            let line_code = code(line);
            if line_code.to_ascii_lowercase().contains("workos") {
                hits.push(format!("{rel}:{}: WorkOS in code", n + 1));
            }
            if WS_CLIENT_CALLS.iter().any(|call| line_code.contains(call))
                && !line_code.contains("fn ")
                && !WS_ALLOWED.iter().any(|allowed| rel.starts_with(allowed))
            {
                let window = lines[n.saturating_sub(8)..=n]
                    .iter()
                    .map(|line| code(line).to_ascii_lowercase())
                    .collect::<Vec<_>>()
                    .join("\n");
                if !LOOPBACK_MARKERS.iter().any(|m| window.contains(m)) {
                    hits.push(format!("{rel}:{}: WebSocket client off loopback", n + 1));
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
