//! The compose stack an upgrade drives (design §54 §10.1): a directory with
//! `compose.yaml` whose project is `loams`, and its `.env`. Found from
//! `--dir`, else `LOAMS_STACK_DIR`, else the current directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::errors::{Result, UpgradeError};

/// The compose project name of a Loams stack.
pub const PROJECT: &str = "loams";

/// The line that puts PD, TiKV and TiDB behind the `tikv` profile in a
/// compose file of the light default (0.1.1 and later).
pub const TIKV_PROFILE_LINE: &str = "profiles: [tikv]";

/// A Loams compose stack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackDir {
    pub dir: PathBuf,
    /// The compose project: `COMPOSE_PROJECT_NAME` (the environment, then
    /// `.env`), else the compose file's `loams`. Its volumes and network are
    /// `<project>_<name>`.
    pub project: String,
    /// `.env`'s settings (values are never printed).
    pub env: BTreeMap<String, String>,
}

/// Whether `dir` holds a Loams stack: a `compose.yaml` whose top-level
/// `name:` is `loams`.
pub fn is_loams_stack(dir: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(dir.join("compose.yaml")) else {
        return false;
    };
    text.lines().any(|line| {
        let line = line.trim_end();
        line.strip_prefix("name:")
            .is_some_and(|name| name.trim().trim_matches(['"', '\'']) == PROJECT)
    })
}

/// Whether a stack with compose file `compose` and `profiles` runs PD and
/// TiKV: always before the `tikv` profile existed, else with it.
pub fn runs_tikv(compose: &str, profiles: &[String]) -> bool {
    !compose.lines().any(|l| l.trim() == TIKV_PROFILE_LINE)
        || profiles.iter().any(|p| p == "tikv" || p == "*")
}

/// `.env` as name → value (comments and blank lines skipped, quotes kept).
pub fn read_env(path: &Path) -> BTreeMap<String, String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

/// The compose project of a stack whose `.env` is `env`.
pub fn project_of(env: &BTreeMap<String, String>) -> String {
    std::env::var("COMPOSE_PROJECT_NAME")
        .ok()
        .or_else(|| env.get("COMPOSE_PROJECT_NAME").cloned())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| PROJECT.to_string())
}

impl StackDir {
    /// The stack: `flag` (`--dir`), else `env_dir` (`LOAMS_STACK_DIR`), else
    /// `cwd` when it holds one. `stack_not_found` otherwise.
    pub fn discover(flag: Option<&Path>, env_dir: Option<&Path>, cwd: &Path) -> Result<Self> {
        let (dir, source) = match (flag, env_dir) {
            (Some(dir), _) => (dir.to_path_buf(), "--dir"),
            (None, Some(dir)) => (dir.to_path_buf(), "LOAMS_STACK_DIR"),
            (None, None) => (cwd.to_path_buf(), "the current directory"),
        };
        if !is_loams_stack(&dir) {
            return Err(UpgradeError::new(
                "stack_not_found",
                format!(
                    "{} ({source}) has no compose.yaml of project `{PROJECT}`; pass --dir with the \
                     stack's directory (deploy/release in the bundle)",
                    dir.display()
                ),
            ));
        }
        let env = read_env(&dir.join(".env"));
        let project = project_of(&env);
        Ok(StackDir { dir, project, env })
    }

    /// The release the stack runs: `.env`'s `LOAMS_VERSION`, else the
    /// compose file's default.
    pub fn release(&self) -> Option<String> {
        if let Some(v) = self.env.get("LOAMS_VERSION").filter(|v| !v.is_empty()) {
            return Some(v.clone());
        }
        let text = std::fs::read_to_string(self.dir.join("compose.yaml")).ok()?;
        let start = text.find("${LOAMS_VERSION:-")? + "${LOAMS_VERSION:-".len();
        let end = text[start..].find('}')?;
        Some(text[start..start + end].to_string())
    }

    /// The compose profiles the stack runs with: `COMPOSE_PROFILES` (the
    /// environment, then `.env`), comma-separated, as compose reads it.
    pub fn profiles(&self) -> Vec<String> {
        std::env::var("COMPOSE_PROFILES")
            .ok()
            .or_else(|| self.env.get("COMPOSE_PROFILES").cloned())
            .unwrap_or_default()
            .split(',')
            .map(|p| p.trim().trim_matches(['"', '\'']).to_string())
            .filter(|p| !p.is_empty())
            .collect()
    }

    /// Whether the stack keeps its metastore and Live on TiKV (PD and TiKV
    /// run). A compose file from before the light default (0.1.0 and
    /// earlier: no `tikv` profile) always runs them; a later one with the
    /// `tikv` profile only (design §54, D1340).
    pub fn tikv(&self) -> bool {
        let text = std::fs::read_to_string(self.dir.join("compose.yaml")).unwrap_or_default();
        runs_tikv(&text, &self.profiles())
    }

    /// The snapshot directory on the host, when `.env` names one
    /// (`LOAMS_UPGRADE_SNAPSHOT_DIR`); else the `upgrade-snapshots` volume.
    pub fn snapshot_host_dir(&self) -> Option<PathBuf> {
        self.env
            .get("LOAMS_UPGRADE_SNAPSHOT_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profiles(list: &[&str]) -> Vec<String> {
        list.iter().map(|p| (*p).to_string()).collect()
    }

    #[test]
    fn a_compose_file_before_the_tikv_profile_always_runs_tikv() {
        let old = "name: loams\nservices:\n  pd:\n    image: pd\n  tikv:\n    image: tikv\n";
        assert!(runs_tikv(old, &[]));
        assert!(runs_tikv(old, &profiles(&["house"])));
    }

    #[test]
    fn the_light_default_runs_tikv_only_with_its_profile() {
        let light = "name: loams\nservices:\n  pd:\n    profiles: [tikv]\n    image: pd\n";
        assert!(!runs_tikv(light, &[]));
        assert!(!runs_tikv(light, &profiles(&["rustfs", "house-tailnet"])));
        assert!(runs_tikv(light, &profiles(&["rustfs", "tikv"])));
        assert!(runs_tikv(light, &profiles(&["*"])));
    }

    #[test]
    fn the_release_compose_file_puts_tikv_behind_its_profile() {
        let text = include_str!("../../../deploy/release/compose.yaml");
        assert!(!runs_tikv(text, &[]));
        assert!(runs_tikv(text, &profiles(&["tikv"])));
    }
}
