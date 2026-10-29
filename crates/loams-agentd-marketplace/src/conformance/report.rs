//! The conformance report (`schemas/marketplace/conformance-report.v1.json`):
//! what the runner checked, how each requirement came out and what it
//! measured. The index CI signs it with Sigstore and the listing links it
//! (`conformance.report`, `conformance.report_sha256`).

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The schema's text.
pub const REPORT_SCHEMA_JSON: &str =
    include_str!("../../../../schemas/marketplace/conformance-report.v1.json");

/// The schema's `$id`, which every report names in `$schema`.
pub const REPORT_SCHEMA_ID: &str = "https://loams.dev/schemas/conformance-report/v1.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Requirement {
    Must,
    Should,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Pass,
    Fail,
    /// Not run: an earlier requirement failed, or the MK0 runner cannot
    /// check what the listing declares. A skipped MUST is not a pass.
    Skip,
}

/// One LAPC requirement's outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    /// `LAPC-C1` … `LAPC-C9`.
    pub id: String,
    pub level: String,
    pub requirement: Requirement,
    pub title: String,
    pub status: CheckStatus,
    /// Why, in plain words: each finding or observation.
    pub details: Vec<String>,
}

/// The runner and how it isolated the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Runner {
    pub name: String,
    pub version: String,
    pub os: String,
    pub arch: String,
    /// `process` in MK0 (a cleared environment, a private process group and
    /// data directory); a sandbox adapter from SB1.
    pub isolation: String,
    /// How egress was recorded: `proxy` (the contract's egress proxy) and,
    /// on Linux, `procfs` (the agent's sockets, sampled).
    pub egress_recording: Vec<String>,
    pub node: String,
    pub limits: Limits,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub ready_ms: u64,
    pub turn_ms: u64,
    pub stop_ms: u64,
}

/// What was tested.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subject {
    pub id: String,
    pub version: String,
    pub kind: String,
    /// SHA-256 of the listing's canonical JSON, as the index records it.
    pub listing_sha256: String,
    pub package: PackageInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageInfo {
    /// `dir` (a local package directory), `tarball` or `npm` (the registry).
    pub origin: String,
    pub name: String,
    pub version: String,
    /// Whether the bytes were checked against the listing's integrity
    /// (always for `tarball` and `npm`; a directory has no tarball).
    pub integrity_checked: bool,
}

/// What the runner measured during the run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Measured {
    /// Requests to the scripted model during the scripted turn.
    pub model_requests: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub tool_calls: Vec<ToolCall>,
    pub egress: Vec<Egress>,
    /// The tail of the agent's stderr.
    pub stderr_tail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    /// The name the agent used (ACP title, else kind; or the MCP tool).
    pub name: String,
    /// `acp` (a tool call or permission request) or `mcp` (a call to the
    /// fake MCP server).
    pub via: String,
    pub declared: bool,
    /// Whether the agent asked permission (ACP) for it.
    pub asked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Egress {
    pub host: String,
    pub port: u16,
    /// `proxy` or `direct` (seen on a socket, around the proxy).
    pub via: String,
    pub allowed: bool,
}

/// One LAPC Core run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    #[serde(rename = "$schema")]
    pub schema: String,
    /// `LAPC`.
    pub suite: String,
    /// The level this run tests: `core` in MK0.
    pub tested_level: String,
    /// The highest level passed: `core`, or `none`.
    pub level: String,
    pub runner: Runner,
    pub subject: Subject,
    pub started_unix_ms: u64,
    pub duration_ms: u64,
    pub checks: Vec<Check>,
    pub measured: Measured,
}

impl Report {
    /// Every MUST of the tested level passed.
    pub fn passed(&self) -> bool {
        self.level == self.tested_level
    }

    /// The report as written to disk: pretty JSON and a final newline.
    pub fn to_json(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).unwrap_or_default();
        text.push('\n');
        text
    }
}

/// The level a set of checks reaches: Core when every Core MUST passed.
pub fn level_of(checks: &[Check]) -> &'static str {
    let core = checks
        .iter()
        .filter(|c| c.level == "core" && c.requirement == Requirement::Must)
        .all(|c| c.status == CheckStatus::Pass);
    if core && !checks.is_empty() {
        "core"
    } else {
        "none"
    }
}

fn validator() -> &'static jsonschema::Validator {
    static VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
    VALIDATOR.get_or_init(|| {
        let schema: Value =
            serde_json::from_str(REPORT_SCHEMA_JSON).expect("conformance-report.v1.json is JSON");
        jsonschema::draft202012::new(&schema)
            .expect("conformance-report.v1.json is a valid 2020-12 schema")
    })
}

/// Checks a report's JSON against the schema; `Err` lists every issue.
pub fn validate_report(report: &Value) -> Result<(), Vec<String>> {
    let issues: Vec<String> = validator()
        .iter_errors(report)
        .map(|e| format!("{}: {e}", e.instance_path()))
        .collect();
    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}
