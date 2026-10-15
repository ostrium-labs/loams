//! The stack directory's own files in an upgrade (design §54 §7.2's S13,
//! §10.3 step 7): a private copy of the bundle, `.env` and `secrets/` taken
//! with the snapshot, and the target bundle installed over the stack
//! directory with the user's `.env` and `secrets/` kept, the release's
//! `.env` changes applied (the user's values kept) and `LOAMS_VERSION` set.
//!
//! The copy lives in `<stack>/.loams-upgrade/snapshots/<id>/host/` on the
//! host (0700, files 0600): the snapshot volume is the containers', and the
//! host files are the CLI's (UP1 Task 13 ruling).

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use crate::errors::{Result, UpgradeError};
use crate::history;
use crate::path::Release;

/// The included Postgres stack's directory, beside the stack directory.
pub const POSTGRES_DIR: &str = "loams-postgres-dev";

fn io(path: &Path, e: std::io::Error) -> UpgradeError {
    UpgradeError::new("internal", format!("{}: {e}", path.display()))
}

/// Never copied over, and never removed, by an install or a restore.
fn kept(name: &str) -> bool {
    name == history::DIR || name == ".env" || name == "secrets" || name.starts_with(".env.bak-")
}

fn copy_tree(from: &Path, to: &Path, private: bool, skip: &dyn Fn(&str) -> bool) -> Result<()> {
    std::fs::create_dir_all(to).map_err(|e| io(to, e))?;
    if private {
        std::fs::set_permissions(to, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| io(to, e))?;
    }
    for entry in std::fs::read_dir(from).map_err(|e| io(from, e))? {
        let entry = entry.map_err(|e| io(from, e))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if skip(&name) {
            continue;
        }
        let source = entry.path();
        let target = to.join(&name);
        let kind = entry.file_type().map_err(|e| io(&source, e))?;
        if kind.is_dir() {
            copy_tree(&source, &target, private, &|_| false)?;
        } else if kind.is_file() {
            std::fs::copy(&source, &target).map_err(|e| io(&target, e))?;
            if private {
                std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600))
                    .map_err(|e| io(&target, e))?;
            }
        }
    }
    Ok(())
}

/// Removes what `dir` has and `reference` does not (the kept names aside).
fn remove_extra(dir: &Path, reference: &Path) -> Result<()> {
    for entry in std::fs::read_dir(dir).map_err(|e| io(dir, e))? {
        let entry = entry.map_err(|e| io(dir, e))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if kept(&name) || reference.join(&name).exists() {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            std::fs::remove_dir_all(&path).map_err(|e| io(&path, e))?;
        } else {
            std::fs::remove_file(&path).map_err(|e| io(&path, e))?;
        }
    }
    Ok(())
}

/// `<stack>/.loams-upgrade/snapshots/<id>/host`.
pub fn host_copy_dir(stack_dir: &Path, id: &str) -> PathBuf {
    stack_dir
        .join(history::DIR)
        .join("snapshots")
        .join(id)
        .join("host")
}

/// The stack directory's sibling Postgres directory, if the stack has one.
fn postgres_dir(stack_dir: &Path) -> Option<PathBuf> {
    let dir = stack_dir.parent()?.join(POSTGRES_DIR);
    dir.join("compose.yaml").exists().then_some(dir)
}

/// The modes of every file and directory under `dir` (relative paths), but
/// the history's.
fn modes(dir: &Path) -> Result<std::collections::BTreeMap<String, u32>> {
    let mut out = std::collections::BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(at) = stack.pop() {
        for entry in std::fs::read_dir(&at).map_err(|e| io(&at, e))? {
            let entry = entry.map_err(|e| io(&at, e))?;
            let path = entry.path();
            let rel = path
                .strip_prefix(dir)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            if rel == history::DIR {
                continue;
            }
            let meta = std::fs::symlink_metadata(&path).map_err(|e| io(&path, e))?;
            if meta.is_dir() {
                stack.push(path);
            } else if !meta.is_file() {
                continue;
            }
            out.insert(rel, meta.permissions().mode() & 0o7777);
        }
    }
    Ok(out)
}

/// Puts back the modes `saved` recorded for files under `dir`.
fn apply_modes(dir: &Path, saved: &std::collections::BTreeMap<String, u32>) -> Result<()> {
    for (rel, mode) in saved {
        let path = dir.join(rel);
        if path.exists() {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(*mode))
                .map_err(|e| io(&path, e))?;
        }
    }
    Ok(())
}

/// Copies the stack directory (bundle, `.env`, `secrets/`) and its sibling
/// Postgres directory into the snapshot's host copy, privately, with their
/// modes in `modes.json` (a restore puts them back: a service bind-mounts
/// some of these files and reads them as another user).
pub fn save_host(stack_dir: &Path, id: &str) -> Result<PathBuf> {
    history::private_dir(stack_dir)?;
    let to = host_copy_dir(stack_dir, id);
    copy_tree(stack_dir, &to.join("release"), true, &|n| n == history::DIR)?;
    let mut recorded = serde_json::Map::new();
    recorded.insert(
        "release".into(),
        serde_json::to_value(modes(stack_dir)?).unwrap_or_default(),
    );
    if let Some(pg) = postgres_dir(stack_dir) {
        copy_tree(&pg, &to.join(POSTGRES_DIR), true, &|_| false)?;
        recorded.insert(
            POSTGRES_DIR.into(),
            serde_json::to_value(modes(&pg)?).unwrap_or_default(),
        );
    }
    write_private(
        &to.join("modes.json"),
        &serde_json::Value::Object(recorded).to_string(),
    )?;
    Ok(to)
}

/// The modes `save_host` recorded for `part` (`release` or the Postgres
/// directory); empty for a copy without them.
fn saved_modes(from: &Path, part: &str) -> std::collections::BTreeMap<String, u32> {
    std::fs::read_to_string(from.join("modes.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| serde_json::from_value(v[part].clone()).ok())
        .unwrap_or_default()
}

/// Puts the host copy back: the bundle files, `.env` and `secrets/` exactly
/// as they were (files the upgrade added are removed).
pub fn restore_host(stack_dir: &Path, id: &str) -> Result<()> {
    let from = host_copy_dir(stack_dir, id);
    let release = from.join("release");
    if !release.join("compose.yaml").exists() {
        return Err(UpgradeError::new(
            "snapshot_incomplete",
            format!("{} has no copy of the stack directory", from.display()),
        ));
    }
    remove_extra(stack_dir, &release)?;
    copy_tree(&release, stack_dir, false, &|n| n == history::DIR)?;
    // `.env` and `secrets/` are the user's: put back exactly, privately.
    for name in [".env", "secrets"] {
        let saved = release.join(name);
        if saved.is_dir() {
            copy_tree(&saved, &stack_dir.join(name), true, &|_| false)?;
        } else if saved.is_file() {
            std::fs::copy(&saved, stack_dir.join(name)).map_err(|e| io(&saved, e))?;
        }
    }
    apply_modes(stack_dir, &saved_modes(&from, "release"))?;
    let saved_pg = from.join(POSTGRES_DIR);
    if saved_pg.exists()
        && let Some(parent) = stack_dir.parent()
    {
        let pg = parent.join(POSTGRES_DIR);
        if pg.exists() {
            remove_extra(&pg, &saved_pg)?;
        }
        copy_tree(&saved_pg, &pg, false, &|_| false)?;
        apply_modes(&pg, &saved_modes(&from, POSTGRES_DIR))?;
    }
    Ok(())
}

/// Installs the target bundle (`release_dir`, with its sibling Postgres
/// directory) over the stack directory: its files replace the old ones;
/// `.env`, `secrets/`, the history and anything else the user keeps there
/// (a `compose.override.yaml`) stay.
pub fn install_bundle(stack_dir: &Path, release_dir: &Path) -> Result<()> {
    if release_dir == stack_dir {
        return Ok(());
    }
    copy_tree(release_dir, stack_dir, false, &kept)?;
    if let (Some(from), Some(parent)) = (
        release_dir.parent().map(|p| p.join(POSTGRES_DIR)),
        stack_dir.parent(),
    ) && from.join("compose.yaml").exists()
    {
        copy_tree(&from, &parent.join(POSTGRES_DIR), false, &|_| false)?;
    }
    Ok(())
}

/// `.env`'s text with the release's changes: `LOAMS_VERSION=<to>`, and each
/// new variable added with its default unless the user already set it.
pub fn env_with_changes(text: &str, to: &str, release: &Release) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut version_set = false;
    for line in text.lines() {
        let name = line.split_once('=').map(|(k, _)| k.trim());
        if name == Some("LOAMS_VERSION") && !line.trim_start().starts_with('#') {
            lines.push(format!("LOAMS_VERSION={to}"));
            version_set = true;
        } else {
            lines.push(line.to_string());
        }
    }
    if !version_set {
        lines.push(format!("LOAMS_VERSION={to}"));
    }
    for change in &release.env {
        let present = lines.iter().any(|l| {
            !l.trim_start().starts_with('#')
                && l.split_once('=').map(|(k, _)| k.trim()) == Some(change.add.as_str())
        });
        if !present {
            lines.push(format!("{}={}", change.add, change.default));
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Writes `.env.bak-<from>` (0600) and then `.env` with the release's
/// changes.
pub fn apply_env(stack_dir: &Path, from: &str, to: &str, release: &Release) -> Result<()> {
    let env = stack_dir.join(".env");
    let text = std::fs::read_to_string(&env).unwrap_or_default();
    let backup = stack_dir.join(format!(".env.bak-{from}"));
    write_private(&backup, &text)?;
    write_private(&env, &env_with_changes(&text, to, release))
}

/// Writes `text` to `path`, 0600.
pub fn write_private(path: &Path, text: &str) -> Result<()> {
    std::fs::write(path, text).map_err(|e| io(path, e))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|e| io(path, e))
}
