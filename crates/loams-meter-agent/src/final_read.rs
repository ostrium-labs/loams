//! Final reads (platform doc 06 §3.3, design 01 MB2; ME1 Task 30): the
//! agent watches every target cgroup's `cgroup.events` with inotify and, when
//! `populated` becomes `0`, scans at once, before the runtime or the kubelet
//! removes the cgroup. The periodic scan stays as the floor: if a cgroup
//! vanishes before its final read, at most one interval is lost.
//!
//! [`FinalReads`] owns the inotify watches; [`FinalReads::sync`] follows the
//! set of target cgroups after each scan, and finished cgroups arrive on the
//! channel given to [`FinalReads::new`] as paths relative to the cgroup root.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tracing::debug;

use crate::cgroup::parse_keyed;
use crate::error::MeterError;

/// The file whose `populated` key the kernel updates (and notifies) when a
/// cgroup's last process exits.
pub const EVENTS_FILE: &str = "cgroup.events";

/// Whether `cgroup.events` at `path` says the cgroup is empty.
pub fn finished(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| parse_keyed(&s, "populated"))
        == Some(0)
}

/// inotify watches on the target cgroups' `cgroup.events`.
pub struct FinalReads {
    root: PathBuf,
    watcher: RecommendedWatcher,
    watched: BTreeSet<String>,
}

impl std::fmt::Debug for FinalReads {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FinalReads")
            .field("root", &self.root)
            .field("watched", &self.watched.len())
            .finish()
    }
}

impl FinalReads {
    /// Watches under the cgroup root `root`; finished cgroups (relative
    /// paths) are sent on `tx`.
    pub fn new(
        root: impl Into<PathBuf>,
        tx: mpsc::UnboundedSender<String>,
    ) -> Result<Self, MeterError> {
        let root: PathBuf = root.into();
        let base = root.clone();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else {
                return;
            };
            if !matches!(
                event.kind,
                EventKind::Modify(_) | EventKind::Any | EventKind::Other
            ) {
                return;
            }
            for path in &event.paths {
                if path.file_name().is_none_or(|n| n != EVENTS_FILE) || !finished(path) {
                    continue;
                }
                let Some(rel) = path
                    .parent()
                    .and_then(|dir| dir.strip_prefix(&base).ok())
                    .and_then(|rel| rel.to_str())
                else {
                    continue;
                };
                let _ = tx.send(rel.to_string());
            }
        })
        .map_err(|e| MeterError::Config(format!("inotify: {e}")))?;
        Ok(Self {
            root,
            watcher,
            watched: BTreeSet::new(),
        })
    }

    /// Watches exactly the cgroups in `paths` (relative to the root): adds
    /// the new ones and drops the ones gone. A cgroup that vanished before
    /// its watch was added is skipped; the periodic scan covers it.
    pub fn sync(&mut self, paths: &[String]) {
        let want: BTreeSet<String> = paths.iter().cloned().collect();
        let gone: Vec<String> = self.watched.difference(&want).cloned().collect();
        for rel in gone {
            // The directory is usually gone already, which removed the watch.
            let _ = self.watcher.unwatch(&self.events_path(&rel));
            self.watched.remove(&rel);
        }
        for rel in want {
            if self.watched.contains(&rel) {
                continue;
            }
            match self
                .watcher
                .watch(&self.events_path(&rel), RecursiveMode::NonRecursive)
            {
                Ok(()) => {
                    self.watched.insert(rel);
                }
                Err(e) => debug!(cgroup = %rel, error = %e, "no inotify watch on cgroup.events"),
            }
        }
    }

    /// Cgroups watched.
    pub fn len(&self) -> usize {
        self.watched.len()
    }

    /// Whether nothing is watched.
    pub fn is_empty(&self) -> bool {
        self.watched.is_empty()
    }

    fn events_path(&self, rel: &str) -> PathBuf {
        self.root.join(rel).join(EVENTS_FILE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finished_reads_populated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(EVENTS_FILE);
        std::fs::write(&path, "populated 1\nfrozen 0\n").unwrap();
        assert!(!finished(&path));
        std::fs::write(&path, "populated 0\nfrozen 0\n").unwrap();
        assert!(finished(&path));
        assert!(!finished(&dir.path().join("missing")));
    }

    #[tokio::test]
    async fn a_finished_cgroup_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let rel = "kubepods.slice/kubepods-pod1.slice";
        let cg = dir.path().join(rel);
        std::fs::create_dir_all(&cg).unwrap();
        std::fs::write(cg.join(EVENTS_FILE), "populated 1\nfrozen 0\n").unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut reads = FinalReads::new(dir.path(), tx).unwrap();
        reads.sync(&[rel.to_string()]);
        assert_eq!(reads.len(), 1);
        std::fs::write(cg.join(EVENTS_FILE), "populated 0\nfrozen 0\n").unwrap();
        let got = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got, rel);
        reads.sync(&[]);
        assert!(reads.is_empty());
    }
}
