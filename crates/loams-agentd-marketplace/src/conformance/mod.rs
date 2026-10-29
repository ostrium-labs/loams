//! The agent package conformance runner, LAPC Core (design §73 §9, D1469;
//! plan MK1 Task 8): `loams-agentd agent conformance <package-dir|listing.json>`.
//!
//! It installs the package as a user's install would (scripts off), starts
//! it in its own process with exactly the environment contract (§73 §3.3)
//! and a private data directory, and plays the ACP client. The agent's world
//! is one loopback server: a scripted OpenAI-compatible model, a fake MCP
//! server per declared MCP dependency, and the egress proxy that records
//! every host. Then it checks LAPC-C1 to C9 and writes a JSON report
//! (`schemas/marketplace/conformance-report.v1.json`).
//!
//! MK0 isolates by process only: no sandbox until SB1. Egress is recorded by
//! the contract's proxy and, on Linux, by sampling the agent's sockets; the
//! report names which. The runner runs on Linux and macOS.

#[cfg(unix)]
mod acp;
#[cfg(unix)]
mod checks;
#[cfg(unix)]
mod observe;
pub mod report;
#[cfg(unix)]
mod runner;
#[cfg(unix)]
mod stage;
#[cfg(unix)]
mod world;

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

pub use report::{Check, CheckStatus, Report, Requirement, validate_report};

/// Where the package's bytes come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Package {
    /// A package directory with `package.json` at its root.
    Dir(PathBuf),
    /// An npm tarball, checked against the listing's integrity.
    Tarball(PathBuf),
    /// The npm registry, by the listing's artifact (needs npm).
    Registry,
}

/// A listing and its package.
#[derive(Debug, Clone)]
pub struct Target {
    pub listing: Value,
    pub package: Package,
}

impl Target {
    /// Reads the command line's `<package-dir|listing.json>` and optional
    /// `--package`. A directory holds `listing.json` and is the package; a
    /// listing file's package is `--package` (a directory or a tarball), else
    /// the registry.
    pub fn resolve(path: &Path, package: Option<&Path>) -> Result<Self, ConformanceError> {
        let (listing_path, default) = if path.is_dir() {
            (path.join("listing.json"), Package::Dir(path.to_owned()))
        } else {
            (path.to_owned(), Package::Registry)
        };
        let bytes = std::fs::read(&listing_path).map_err(|e| ConformanceError::Io {
            path: listing_path.display().to_string(),
            error: e.to_string(),
        })?;
        let listing: Value = serde_json::from_slice(&bytes).map_err(|e| {
            ConformanceError::Input(format!("{}: not JSON: {e}", listing_path.display()))
        })?;
        let package = match package {
            Some(p) if p.is_dir() => Package::Dir(p.to_owned()),
            Some(p) => Package::Tarball(p.to_owned()),
            None => default,
        };
        Ok(Self { listing, package })
    }
}

/// How long the runner waits. The defaults are the spec's (§73 §9): ready
/// within 30 s, stopped within 10 s; a scripted turn gets two minutes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub ready: Duration,
    pub turn: Duration,
    pub stop: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            ready: Duration::from_secs(30),
            turn: Duration::from_secs(120),
            stop: Duration::from_secs(10),
        }
    }
}

/// Loams services for declared capabilities. A Core run provides none by
/// default, so their variables are absent; CI with a local Loams passes them.
#[derive(Debug, Clone, Default)]
pub struct Services {
    pub pg_url: Option<String>,
    pub qdrant: Option<(String, String)>,
    pub elasticsearch_url: Option<String>,
    pub durable_url: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    /// `node`; found on PATH when unset.
    pub node: Option<PathBuf>,
    /// `npm`, for tarballs, the registry and packages with dependencies;
    /// found on PATH when unset.
    pub npm: Option<PathBuf>,
    /// Where to install and run; a fresh temporary directory, removed
    /// afterwards, when unset.
    pub work_dir: Option<PathBuf>,
    pub limits: Limits,
    pub services: Services,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConformanceError {
    #[error("{0}")]
    Input(String),
    #[error("the runner cannot run this listing: {0}")]
    Unsupported(String),
    #[error("installing the package failed: {0}")]
    Stage(String),
    #[error("{path}: {error}")]
    Io { path: String, error: String },
}

/// The declared tool an ACP tool call or permission request names (MK1
/// Task 0 ruling 6): ACP has no stable tool name, so the declared `name` is
/// matched against the call's `title` first, then its `kind`.
pub fn declared_tool<'a>(declared: &'a [String], title: &str, kind: &str) -> Option<&'a str> {
    [title, kind]
        .iter()
        .filter(|candidate| !candidate.is_empty())
        .find_map(|candidate| declared.iter().find(|name| name == candidate))
        .map(String::as_str)
}

/// The tool names a listing declares, in order.
pub fn declared_tools(listing: &Value) -> Vec<String> {
    listing["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t["name"].as_str().map(str::to_owned))
        .collect()
}

/// Runs LAPC Core against `target`. `Err` means the run could not happen
/// (an invalid listing, no Node, a package that would not install); a run
/// that happened returns its report, passed or not.
pub async fn run(target: &Target, options: &Options) -> Result<Report, ConformanceError> {
    #[cfg(unix)]
    {
        runner::run(target, options).await
    }
    #[cfg(not(unix))]
    {
        let _ = (target, options);
        Err(ConformanceError::Unsupported(
            "the MK0 conformance runner runs on Linux and macOS".into(),
        ))
    }
}

/// Signs a written report with Sigstore keyless signing, as the index CI's
/// identity: `cosign sign-blob --yes --bundle <report>.sigstore.json <report>`.
/// Returns the bundle's path.
pub fn sign_report(report: &Path, cosign: Option<&Path>) -> Result<PathBuf, ConformanceError> {
    let cosign = cosign
        .map(Path::to_owned)
        .unwrap_or_else(|| "cosign".into());
    let mut bundle = report.as_os_str().to_owned();
    bundle.push(".sigstore.json");
    let bundle = PathBuf::from(bundle);
    let output = std::process::Command::new(&cosign)
        .arg("sign-blob")
        .arg("--yes")
        .arg("--bundle")
        .arg(&bundle)
        .arg(report)
        .output()
        .map_err(|e| ConformanceError::Input(format!("{}: {e}", cosign.display())))?;
    if !output.status.success() {
        return Err(ConformanceError::Input(format!(
            "cosign sign-blob failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(bundle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_tools_match_title_then_kind() {
        let declared = vec!["save_note".to_owned(), "read".to_owned()];
        assert_eq!(
            declared_tool(&declared, "save_note", "edit"),
            Some("save_note")
        );
        assert_eq!(declared_tool(&declared, "Read file", "read"), Some("read"));
        assert_eq!(declared_tool(&declared, "shell_exec", "execute"), None);
        assert_eq!(declared_tool(&declared, "", ""), None);
    }
}
