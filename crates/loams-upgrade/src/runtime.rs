//! The container runtime under a stack: `docker compose` or `podman
//! compose`, detected (`platform_unsupported` without either). [`Runtime`]
//! is the seam the tests replace with a recording fake.

use std::path::Path;
use std::process::Command;

use crate::errors::{Result, UpgradeError};

/// A command's outcome.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Output {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.status == 0
    }
}

/// What an upgrade asks of the container runtime.
pub trait Runtime: Send + Sync {
    /// `<engine> compose --project-directory <dir> <args>`.
    fn compose(&self, dir: &Path, args: &[String]) -> Result<Output>;
    /// `<engine> <args>` (`run`, `pull`, `volume`, `info`).
    fn engine(&self, args: &[String]) -> Result<Output>;
}

/// The real runtime.
#[derive(Clone, Debug)]
pub struct Cli {
    program: String,
}

impl Cli {
    /// `docker` when `docker compose version` works, else `podman`.
    pub fn detect() -> Result<Self> {
        for program in ["docker", "podman"] {
            let works = Command::new(program)
                .args(["compose", "version"])
                .output()
                .is_ok_and(|o| o.status.success());
            if works {
                return Ok(Cli {
                    program: program.to_string(),
                });
            }
        }
        Err(UpgradeError::new(
            "platform_unsupported",
            "neither `docker compose` nor `podman compose` works here",
        ))
    }

    fn run(&self, args: &[String]) -> Result<Output> {
        // LOAMS_UPGRADE_TRACE=1 prints every command (the gate keeps it in
        // its logs). Arguments hold paths and image names, never `.env` values.
        if std::env::var_os("LOAMS_UPGRADE_TRACE").is_some() {
            eprintln!("+ {} {}", self.program, args.join(" "));
        }
        let out = Command::new(&self.program)
            .args(args)
            .output()
            .map_err(|e| UpgradeError::new("internal", format!("{}: {e}", self.program)))?;
        Ok(Output {
            status: out.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

impl Runtime for Cli {
    fn compose(&self, dir: &Path, args: &[String]) -> Result<Output> {
        let mut all = vec![
            "compose".to_string(),
            "--project-directory".to_string(),
            dir.display().to_string(),
        ];
        all.extend(args.iter().cloned());
        self.run(&all)
    }

    fn engine(&self, args: &[String]) -> Result<Output> {
        self.run(args)
    }
}

/// `args` as owned strings.
pub fn args<const N: usize>(items: [&str; N]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}
