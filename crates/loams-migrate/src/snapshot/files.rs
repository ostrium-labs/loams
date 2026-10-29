//! File helpers of snapshots: private directories (0700) and files (0600),
//! recursive copies, digests and whole-directory replacement.

use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::manifest::Component;
use super::{SnapshotError, io};

/// Sets `path`'s mode bits. Windows has none (a new file inherits its
/// directory's ACL), so there this does nothing.
#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

pub(crate) fn private_dir(dir: &Path) -> Result<(), SnapshotError> {
    fs::create_dir_all(dir).map_err(|e| io(dir.display(), e))?;
    set_mode(dir, 0o700).map_err(|e| io(dir.display(), e))
}

/// `dir` and its missing parents, each 0700 when this creates it.
pub(crate) fn private_dir_all(dir: &Path) -> Result<(), SnapshotError> {
    if dir.exists() {
        return Ok(());
    }
    if let Some(parent) = dir.parent() {
        private_dir_all(parent)?;
    }
    private_dir(dir)
}

pub(crate) fn private_file(path: &Path) -> Result<(), SnapshotError> {
    set_mode(path, 0o600).map_err(|e| io(path.display(), e))
}

pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<(), SnapshotError> {
    fs::write(path, bytes).map_err(|e| io(path.display(), e))?;
    private_file(path)
}

pub(crate) fn non_empty(dir: &Path) -> bool {
    fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some())
}

/// Every regular file under `root` as (path relative to `root` with `/`,
/// absolute path), sorted.
pub(crate) fn walk(root: &Path) -> Result<Vec<(String, PathBuf)>, SnapshotError> {
    let mut out = Vec::new();
    walk_into(root, root, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk_into(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(String, PathBuf)>,
) -> Result<(), SnapshotError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(io(dir.display(), e)),
    };
    for entry in entries {
        let entry = entry.map_err(|e| io(dir.display(), e))?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|e| io(path.display(), e))?;
        if kind.is_dir() {
            walk_into(root, &path, out)?;
        } else if kind.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|e| io(path.display(), e))?
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            out.push((relative, path));
        }
    }
    Ok(())
}

/// The bytes of every file under `path` (a file or a directory).
pub(crate) fn size_of(path: &Path) -> u64 {
    match fs::metadata(path) {
        Ok(meta) if meta.is_file() => meta.len(),
        Ok(_) => walk(path)
            .unwrap_or_default()
            .iter()
            .filter_map(|(_, p)| fs::metadata(p).ok())
            .map(|m| m.len())
            .sum(),
        Err(_) => 0,
    }
}

/// The digest of a component: every file's relative path and contents, in
/// path order.
pub(crate) fn digest(root: &Path) -> Result<(u64, String), SnapshotError> {
    let mut hash = Sha256::new();
    let mut bytes = 0;
    for (relative, path) in walk(root)? {
        let data = fs::read(&path).map_err(|e| io(path.display(), e))?;
        bytes += data.len() as u64;
        hash.update(relative.as_bytes());
        hash.update([0]);
        hash.update((data.len() as u64).to_be_bytes());
        hash.update(&data);
    }
    Ok((bytes, hex(&hash.finalize())))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// Describes the component at `<dir>/<path>`.
pub(crate) fn describe(
    store: &str,
    method: &str,
    dir: &Path,
    path: &str,
) -> Result<Component, SnapshotError> {
    let (bytes, sha256) = digest(&dir.join(path))?;
    Ok(Component {
        store: store.to_string(),
        method: method.to_string(),
        path: path.to_string(),
        bytes,
        sha256,
        version: None,
    })
}

/// Copies directory `from` to `to` (private), and describes it.
pub(crate) fn copy_component(
    store: &str,
    from: &Path,
    to: &Path,
) -> Result<Component, SnapshotError> {
    private_dir(to)?;
    for (relative, path) in walk(from)? {
        let target = to.join(&relative);
        if let Some(parent) = target.parent() {
            private_dir_all(parent)?;
        }
        fs::copy(&path, &target).map_err(|e| io(path.display(), e))?;
        private_file(&target)?;
    }
    let dir = to.parent().unwrap_or(to);
    let name = to
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    describe(store, "copy", dir, &name)
}

/// Staging directories `replace_dir` keeps inside the target while it works.
const RESTORE_NEW: &str = ".restore-new";
const RESTORE_OLD: &str = ".restore-old";

/// Replaces the contents of directory `target` with a copy of `source`.
///
/// Everything happens inside `target`: the copy is built in
/// `target/.restore-new`, the current entries are moved into
/// `target/.restore-old`, the copy's entries take their place, and only then is
/// the old content removed. `target` itself is never renamed, so it may be a
/// volume mount point, and nothing is written beside it: `loams upgrade` runs
/// restores in a container with a read-only root file system, where a sibling
/// such as `/var/lib/loams.restore-new` cannot be created (upgrade gate,
/// snapshot run 40686997353). Every rename stays on the target's file system.
pub(crate) fn replace_dir(source: &Path, target: &Path) -> Result<(), SnapshotError> {
    fs::create_dir_all(target).map_err(|e| io(target.display(), e))?;
    let staged = target.join(RESTORE_NEW);
    let old = target.join(RESTORE_OLD);
    let _ = fs::remove_dir_all(&staged);
    let _ = fs::remove_dir_all(&old);
    fs::create_dir_all(&staged).map_err(|e| io(staged.display(), e))?;
    for (relative, path) in walk(source)? {
        let to = staged.join(&relative);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(|e| io(parent.display(), e))?;
        }
        fs::copy(&path, &to).map_err(|e| io(path.display(), e))?;
    }
    fs::create_dir_all(&old).map_err(|e| io(old.display(), e))?;
    for name in entries_except_staging(target)? {
        fs::rename(target.join(&name), old.join(&name))
            .map_err(|e| io(target.join(&name).display(), e))?;
    }
    for name in entries_except_staging(&staged)? {
        fs::rename(staged.join(&name), target.join(&name))
            .map_err(|e| io(staged.join(&name).display(), e))?;
    }
    let _ = fs::remove_dir_all(&staged);
    let _ = fs::remove_dir_all(&old);
    Ok(())
}

/// The names in `dir`, without the two staging directories.
fn entries_except_staging(dir: &Path) -> Result<Vec<std::ffi::OsString>, SnapshotError> {
    let mut names = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| io(dir.display(), e))? {
        let name = entry.map_err(|e| io(dir.display(), e))?.file_name();
        if name != RESTORE_NEW && name != RESTORE_OLD {
            names.push(name);
        }
    }
    Ok(names)
}

#[cfg(test)]
mod replace_dir_tests {
    #![allow(clippy::unwrap_used)]

    use std::fs;

    use super::replace_dir;

    #[test]
    fn the_target_holds_exactly_the_source_and_no_staging_is_left() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("snap");
        let target = tmp.path().join("data");
        fs::create_dir_all(source.join("a/b")).unwrap();
        fs::write(source.join("a/b/new.txt"), "new").unwrap();
        fs::write(source.join("top.txt"), "top").unwrap();
        fs::create_dir_all(target.join("gone")).unwrap();
        fs::write(target.join("gone/old.txt"), "old").unwrap();
        fs::write(target.join("top.txt"), "stale").unwrap();

        replace_dir(&source, &target).unwrap();

        assert_eq!(
            fs::read_to_string(target.join("a/b/new.txt")).unwrap(),
            "new"
        );
        assert_eq!(fs::read_to_string(target.join("top.txt")).unwrap(), "top");
        assert!(!target.join("gone").exists());
        assert!(!target.join(super::RESTORE_NEW).exists());
        assert!(!target.join(super::RESTORE_OLD).exists());
    }

    #[test]
    fn nothing_is_written_beside_the_target() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("snap");
        let parent = tmp.path().join("parent");
        let target = parent.join("data");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("f"), "x").unwrap();
        fs::create_dir_all(&target).unwrap();

        replace_dir(&source, &target).unwrap();

        let siblings: Vec<_> = fs::read_dir(&parent)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(siblings, vec![std::ffi::OsString::from("data")]);
    }

    #[test]
    fn a_leftover_from_an_interrupted_restore_is_cleared() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("snap");
        let target = tmp.path().join("data");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("f"), "x").unwrap();
        fs::create_dir_all(target.join(super::RESTORE_OLD)).unwrap();
        fs::write(target.join(super::RESTORE_OLD).join("junk"), "j").unwrap();

        replace_dir(&source, &target).unwrap();

        assert_eq!(fs::read_to_string(target.join("f")).unwrap(), "x");
        assert!(!target.join(super::RESTORE_OLD).exists());
    }
}
