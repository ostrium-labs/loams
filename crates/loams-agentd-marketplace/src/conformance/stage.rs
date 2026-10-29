//! Installing the package for a run, as a user's install would: npm with
//! scripts off into `<work>/install`, then LAPC-C8's check for code that an
//! install would have run. A package directory without dependencies is
//! copied instead (no npm needed); one with dependencies is packed first.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde_json::Value;
use sha2::{Digest, Sha512};

use super::{ConformanceError, Package};
use crate::install;

/// The package, installed.
#[derive(Debug)]
pub(super) struct Staged {
    /// `<work>/install/node_modules/<name>`.
    pub package_dir: PathBuf,
    pub origin: &'static str,
    pub name: String,
    pub version: String,
    pub integrity_checked: bool,
    /// LAPC-C8: what an install would have run, if anything.
    pub install_code: Option<String>,
}

fn io(path: &Path) -> impl Fn(std::io::Error) -> ConformanceError + '_ {
    move |e| ConformanceError::Io {
        path: path.display().to_string(),
        error: e.to_string(),
    }
}

fn stage_error(message: impl Into<String>) -> ConformanceError {
    ConformanceError::Stage(message.into())
}

/// What npm needs to run: the runtime directories on PATH, a private HOME
/// and cache, and the runner's own proxy and registry settings.
#[derive(Debug, Clone)]
pub(super) struct NpmTool {
    pub npm: PathBuf,
    pub node_dir: PathBuf,
}

impl NpmTool {
    async fn run(
        &self,
        args: &[String],
        cwd: &Path,
        work: &Path,
    ) -> Result<String, ConformanceError> {
        let mut path = vec![self.node_dir.clone()];
        if let Some(dir) = self.npm.parent() {
            path.push(dir.to_owned());
        }
        let mut command = tokio::process::Command::new(&self.npm);
        command
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .env("PATH", std::env::join_paths(path).unwrap_or_default())
            .env("HOME", work.join("npm-home"))
            .env("npm_config_cache", work.join("npm-cache"))
            .env("npm_config_update_notifier", "false")
            .env("npm_config_fund", "false")
            .env("npm_config_audit", "false")
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        for name in [
            "HTTPS_PROXY",
            "HTTP_PROXY",
            "NO_PROXY",
            "https_proxy",
            "http_proxy",
            "no_proxy",
            "npm_config_registry",
        ] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        let output = command
            .output()
            .await
            .map_err(|e| stage_error(format!("npm: {e}")))?;
        if !output.status.success() {
            return Err(stage_error(format!(
                "npm {} failed: {}",
                args.first().map(String::as_str).unwrap_or_default(),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// `npm pack <spec> --ignore-scripts`: the tarball's path.
    async fn pack(&self, spec: &str, work: &Path) -> Result<PathBuf, ConformanceError> {
        let dest = work.join("pack");
        std::fs::create_dir_all(&dest).map_err(io(&dest))?;
        let out = self
            .run(
                &[
                    "pack".into(),
                    spec.into(),
                    "--ignore-scripts".into(),
                    "--json".into(),
                    "--pack-destination".into(),
                    dest.display().to_string(),
                ],
                work,
                work,
            )
            .await?;
        let packed: Value = serde_json::from_str(&out)
            .map_err(|e| stage_error(format!("npm pack printed no JSON: {e}")))?;
        let file = packed[0]["filename"]
            .as_str()
            .ok_or_else(|| stage_error("npm pack named no tarball"))?;
        // npm names scoped tarballs `scope-name-version.tgz`.
        Ok(dest.join(file.trim_start_matches('@').replace('/', "-")))
    }
}

fn str_at<'a>(value: &'a Value, path: &[&str]) -> &'a str {
    path.iter()
        .try_fold(value, |v, key| v.get(*key))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// `sha512-<base64>` of a file, npm's integrity form.
fn integrity_of(bytes: &[u8]) -> String {
    format!(
        "sha512-{}",
        base64::engine::general_purpose::STANDARD.encode(Sha512::digest(bytes))
    )
}

/// Copies a package directory, leaving out `node_modules`, `.git` and
/// symbolic links (a package must not point outside itself).
fn copy_package(from: &Path, to: &Path) -> Result<(), ConformanceError> {
    std::fs::create_dir_all(to).map_err(io(to))?;
    for entry in std::fs::read_dir(from).map_err(io(from))?.flatten() {
        let name = entry.file_name();
        if name == "node_modules" || name == ".git" {
            continue;
        }
        let path = entry.path();
        let kind = entry.file_type().map_err(io(&path))?;
        if kind.is_dir() {
            copy_package(&path, &to.join(&name))?;
        } else if kind.is_file() {
            std::fs::copy(&path, to.join(&name)).map_err(io(&path))?;
        }
    }
    Ok(())
}

/// Installs the package under `<work>/install` and checks it for install-time code.
pub(super) async fn stage(
    listing: &Value,
    package: &Package,
    work: &Path,
    npm: Option<&NpmTool>,
) -> Result<Staged, ConformanceError> {
    let name = str_at(listing, &["artifact", "name"]).to_owned();
    let version = str_at(listing, &["artifact", "version"]).to_owned();
    let integrity = str_at(listing, &["artifact", "integrity"]);
    if str_at(listing, &["artifact", "type"]) != "npm" || name.is_empty() {
        return Err(ConformanceError::Unsupported(
            "the MK0 runner installs npm artifacts only".into(),
        ));
    }
    let prefix = work.join("install");
    let package_dir = prefix.join("node_modules").join(&name);
    let need_npm = || {
        npm.ok_or_else(|| {
            stage_error("this package needs npm to install, and no npm was found (pass --npm)")
        })
    };

    let (origin, tarball, integrity_checked) = match package {
        Package::Dir(dir) => {
            let manifest: Value = std::fs::read(dir.join("package.json"))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .ok_or_else(|| stage_error(format!("{} has no package.json", dir.display())))?;
            let found = manifest["name"].as_str().unwrap_or_default();
            if found != name {
                return Err(stage_error(format!(
                    "the package is {found:?}, but the listing's artifact is {name:?}"
                )));
            }
            let has_deps = manifest["dependencies"]
                .as_object()
                .is_some_and(|d| !d.is_empty());
            if has_deps {
                let tarball = need_npm()?.pack(&dir.display().to_string(), work).await?;
                ("dir", Some(tarball), false)
            } else {
                copy_package(dir, &package_dir)?;
                ("dir", None, false)
            }
        }
        Package::Tarball(file) => ("tarball", Some(file.clone()), true),
        Package::Registry => {
            let tarball = need_npm()?.pack(&format!("{name}@{version}"), work).await?;
            ("npm", Some(tarball), true)
        }
    };

    if let Some(tarball) = tarball {
        let bytes = std::fs::read(&tarball).map_err(io(&tarball))?;
        if integrity_checked && integrity_of(&bytes) != integrity {
            return Err(stage_error(format!(
                "{} does not match the listing's integrity",
                tarball.display()
            )));
        }
        std::fs::create_dir_all(&prefix).map_err(io(&prefix))?;
        need_npm()?
            .run(&install::npm_args(&tarball, &prefix), work, work)
            .await?;
    }
    if !package_dir.join("package.json").is_file() {
        return Err(stage_error(format!(
            "the install has no {name}/package.json"
        )));
    }

    let mut install_code = install::refuse_install_scripts(&prefix.join("node_modules"))
        .err()
        .map(|e| e.to_string());
    if install_code.is_none()
        && let Some(lock) = std::fs::read(package_dir.join("npm-shrinkwrap.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        && let Some(path) = install::shrinkwrap_declares_scripts(&lock)
    {
        install_code = Some(format!(
            "{path} has an install script (hasInstallScript in npm-shrinkwrap.json)"
        ));
    }

    Ok(Staged {
        package_dir,
        origin,
        name,
        version,
        integrity_checked,
        install_code,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn integrity_is_npm_sha512() {
        assert_eq!(
            integrity_of(b""),
            "sha512-z4PhNX7vuL3xVChQ1m2AB9Yg5AULVxXcg/SpIdNs6c5H0NE8XYXysP+DGNKHfuwvY7kxvUdBeoGlODJ6+SfaPg=="
        );
    }
}
