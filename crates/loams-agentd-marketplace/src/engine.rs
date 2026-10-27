//! The engine of the `node` runtime (design §73 §3.2, D1808; plan MS1 Task 6):
//! `node`, the default, or `bun`, at the Bun version D1791 pins. Only the
//! program that runs the entrypoint changes. The npm artifact, its
//! provenance, the scripts-off install and the environment contract (D1453)
//! are the same for both engines.
//!
//! agentd resolves the engine before install (MK1 Task 0 ruling 9 for Node),
//! so a Bun listing on a machine without Bun is refused with nothing written.

use std::path::{Path, PathBuf};

use semver::{Version, VersionReq};
use serde_json::Value;

use crate::install::Installed;

/// The Bun version agentd runs (D1791; VV1's pin, measured in VV0 and the
/// MS1 spike). A Bun listing's range must admit it.
pub const BUN_PINNED: &str = "1.4.4";

/// The oldest Node the `node` runtime accepts when a listing names no range.
pub const NODE_MINIMUM: &str = "22.13.0";

/// Bun's runtime flags for a marketplace agent: no package auto-install at
/// run time (the code is exactly what was installed and verified) and no
/// automatic `.env` loading (the environment is exactly the contract).
pub const BUN_FLAGS: &[&str] = &["--no-install", "--no-env-file"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Engine {
    Node,
    Bun,
}

impl Engine {
    /// The listing's engine; absent (every listing before D1808) is `Node`.
    /// `None` when the listing is not on the `node` runtime.
    pub fn of(listing: &Value) -> Option<Engine> {
        if listing["runtime"]["kind"].as_str() != Some("node") {
            return None;
        }
        Some(match listing["runtime"]["engine"].as_str() {
            Some("bun") => Engine::Bun,
            _ => Engine::Node,
        })
    }

    /// The name of the engine's program, as in the listing.
    pub fn as_str(self) -> &'static str {
        match self {
            Engine::Node => "node",
            Engine::Bun => "bun",
        }
    }

    fn display_name(self) -> &'static str {
        match self {
            Engine::Node => "Node",
            Engine::Bun => "Bun",
        }
    }

    /// The range used when a listing names none.
    pub fn default_range(self) -> String {
        match self {
            Engine::Node => format!(">={NODE_MINIMUM}"),
            Engine::Bun => format!(">={BUN_PINNED}"),
        }
    }
}

impl std::fmt::Display for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    #[error("the listing is not on the node runtime, so it has no engine")]
    NotNode,
    #[error("the listing's runtime.version {0:?} is not a version range")]
    BadRange(String),
    /// The "Bun not installed" state (and its Node equal).
    #[error(
        "this agent runs on {} {required}, and {} is not installed",
        .engine.display_name(),
        .engine.display_name()
    )]
    NotInstalled { engine: Engine, required: String },
    #[error(
        "this agent runs on {} {required}, and the installed {} is {found}",
        .engine.display_name(),
        .engine.display_name()
    )]
    TooOld {
        engine: Engine,
        found: String,
        required: String,
    },
}

/// The listing's range for its engine: `runtime.version`, else the engine's
/// default.
pub fn required(listing: &Value) -> Result<(Engine, String, VersionReq), EngineError> {
    let engine = Engine::of(listing).ok_or(EngineError::NotNode)?;
    let range = listing["runtime"]["version"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| engine.default_range());
    let req = VersionReq::parse(&range).map_err(|_| EngineError::BadRange(range.clone()))?;
    Ok((engine, range, req))
}

/// Finds an engine's program and reads its version. agentd's probe searches
/// `PATH` and the node-version-manager directories it already scans for
/// harness adapters; tests use a fake.
pub trait EngineProbe {
    fn find(&self, program: &str) -> Option<PathBuf>;
    fn version(&self, program: &Path) -> Option<Version>;
}

/// Searches `dirs` in order and runs `<program> --version`.
#[derive(Debug, Clone, Default)]
pub struct PathProbe {
    pub dirs: Vec<PathBuf>,
}

impl PathProbe {
    /// The directories of the `PATH` variable.
    pub fn from_env() -> Self {
        Self {
            dirs: std::env::var_os("PATH")
                .map(|path| std::env::split_paths(&path).collect())
                .unwrap_or_default(),
        }
    }
}

impl EngineProbe for PathProbe {
    fn find(&self, program: &str) -> Option<PathBuf> {
        let name = if cfg!(windows) {
            format!("{program}.exe")
        } else {
            program.to_owned()
        };
        self.dirs
            .iter()
            .map(|dir| dir.join(&name))
            .find(|path| path.is_file())
    }

    fn version(&self, program: &Path) -> Option<Version> {
        let output = std::process::Command::new(program)
            .arg("--version")
            .env_clear()
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        parse_version(&String::from_utf8_lossy(&output.stdout))
    }
}

/// `v24.21.0` (node) and `1.4.4` (bun), with any trailing text.
pub fn parse_version(text: &str) -> Option<Version> {
    let word = text.split_whitespace().next()?;
    Version::parse(word.strip_prefix('v').unwrap_or(word)).ok()
}

/// The engine agentd will start the agent with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedEngine {
    pub engine: Engine,
    pub program: PathBuf,
    pub version: Version,
}

impl ResolvedEngine {
    /// The directory holding the engine's program: the contract's
    /// `runtime_bin`, the agent's only `PATH` entry.
    pub fn bin_dir(&self) -> Option<PathBuf> {
        self.program.parent().map(Path::to_path_buf)
    }
}

/// Finds the listing's engine at a version in its range.
pub fn resolve(listing: &Value, probe: &dyn EngineProbe) -> Result<ResolvedEngine, EngineError> {
    let (engine, range, req) = required(listing)?;
    let program = probe
        .find(engine.as_str())
        .ok_or_else(|| EngineError::NotInstalled {
            engine,
            required: range.clone(),
        })?;
    let version = probe
        .version(&program)
        .ok_or_else(|| EngineError::NotInstalled {
            engine,
            required: range.clone(),
        })?;
    if !req.matches(&version) {
        return Err(EngineError::TooOld {
            engine,
            found: version.to_string(),
            required: range,
        });
    }
    Ok(ResolvedEngine {
        engine,
        program,
        version,
    })
}

/// A process line: the program and its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: String,
    pub args: Vec<String>,
}

/// `args` for running `entry` on `engine`: `node <entry>` or
/// `bun --no-install --no-env-file <entry>`.
pub fn engine_args(engine: Engine, entry: &str) -> Vec<String> {
    let mut args: Vec<String> = match engine {
        Engine::Node => Vec::new(),
        Engine::Bun => BUN_FLAGS.iter().map(|f| (*f).to_owned()).collect(),
    };
    args.push(entry.to_owned());
    args
}

/// How agentd starts an installed `node` agent, with the contract's
/// environment (unchanged by the engine).
pub fn launch(installed: &Installed, resolved: &ResolvedEngine) -> Launch {
    Launch {
        program: resolved.program.display().to_string(),
        args: engine_args(resolved.engine, &installed.entry),
    }
}
