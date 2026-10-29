//! A cgroup v2 reader (design 01 §5.1, §5.2).
//!
//! The reader walks configured subtrees of the cgroup root and samples every
//! **target** cgroup: a directory whose name matches one of the target
//! patterns (a pod's cgroup, a workerd scope). It never descends into a
//! target, because a parent's `cpu.stat` already includes its children and
//! sampling both would count twice.
//!
//! A cgroup can vanish at any moment (a pod exits between `readdir` and
//! `read`); such a cgroup is skipped, never an error. [`CgroupReader::scan_lenient`]
//! also skips a target cgroup that cannot be read or parsed, and reports it,
//! so one bad cgroup does not stop metering for every other tenant.

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::error::MeterError;

/// One reading of one cgroup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CgroupSample {
    /// The path relative to the cgroup root, `/`-separated, no leading `/`.
    pub path: String,
    /// The directory's inode. A cgroup recreated at the same path has a new
    /// one, so `(path, inode)` names one cgroup's lifetime.
    pub inode: u64,
    /// `cpu.stat` `usage_usec`: cumulative CPU microseconds.
    pub cpu_usec: u64,
    /// `memory.current`, when the memory controller is enabled.
    pub memory_current: Option<u64>,
    /// `memory.peak`, when the kernel has it (5.19+).
    pub memory_peak: Option<u64>,
    /// `cgroup.events` `populated`: whether any process is left. `false`
    /// means the last process exited and this is the final reading before
    /// the runtime removes the directory (design 01 §7.2).
    pub populated: bool,
}

impl CgroupSample {
    /// The resource id used in events and baselines: `<path>#ino=<inode>`.
    pub fn resource_id(&self) -> String {
        format!("{}#ino={}", self.path, self.inode)
    }
}

/// The default target patterns: Kubernetes pod cgroups under the systemd
/// driver (`kubepods-burstable-pod<uid>.slice`, `kubepods-pod<uid>.slice`)
/// and the cgroupfs driver (`pod<uid>`), and the layout of operon's usage
/// hooks (operon §27 §3.2, D201): the per-tenant T0 slice
/// `loams.slice/tenant-<org>.slice` and the shared T1 host
/// `loams.slice/wasm-host.scope`.
pub const DEFAULT_TARGETS: &[&str] = &[
    "kubepods-*pod*.slice",
    "pod*",
    "tenant-*.slice",
    "wasm-host.scope",
];

/// A lenient scan: the samples, and the target cgroups that could not be
/// read or parsed this time (their paths, relative to the root).
#[derive(Debug, Default)]
pub struct Scan {
    /// The cgroups read, sorted by path.
    pub samples: Vec<CgroupSample>,
    /// The target cgroups skipped, with why.
    pub skipped: Vec<(String, MeterError)>,
}

/// Reads cgroup v2 files under a root (normally `/sys/fs/cgroup`).
#[derive(Debug, Clone)]
pub struct CgroupReader {
    root: PathBuf,
    scopes: Vec<String>,
    targets: Vec<String>,
}

impl CgroupReader {
    /// A reader over `root`, walking `scopes` (paths relative to `root`;
    /// empty means the whole root) and sampling cgroups whose directory name
    /// matches one of `targets` (`*` matches any run of characters).
    pub fn new(root: impl Into<PathBuf>, scopes: Vec<String>, targets: Vec<String>) -> Self {
        let scopes = if scopes.is_empty() {
            vec![String::new()]
        } else {
            scopes
                .into_iter()
                .map(|s| s.trim_matches('/').to_string())
                .collect()
        };
        Self {
            root: root.into(),
            scopes,
            targets,
        }
    }

    /// A reader with [`DEFAULT_TARGETS`].
    pub fn with_default_targets(root: impl Into<PathBuf>, scopes: Vec<String>) -> Self {
        let targets = DEFAULT_TARGETS.iter().map(|t| t.to_string()).collect();
        Self::new(root, scopes, targets)
    }

    /// Samples every target cgroup, sorted by path. Any target that cannot
    /// be read or parsed fails the scan.
    pub fn scan(&self) -> Result<Vec<CgroupSample>, MeterError> {
        self.scan_inner(None).map(|scan| scan.samples)
    }

    /// Samples every target cgroup, sorted by path, skipping (and
    /// reporting) the targets that cannot be read or parsed. Errors walking
    /// the tree above the targets still fail the scan.
    pub fn scan_lenient(&self) -> Result<Scan, MeterError> {
        let mut skipped = Vec::new();
        let mut scan = self.scan_inner(Some(&mut skipped))?;
        scan.skipped = skipped;
        Ok(scan)
    }

    fn scan_inner(
        &self,
        mut skipped: Option<&mut Vec<(String, MeterError)>>,
    ) -> Result<Scan, MeterError> {
        let mut out = Vec::new();
        for scope in &self.scopes {
            let dir = if scope.is_empty() {
                self.root.clone()
            } else {
                self.root.join(scope)
            };
            self.walk(&dir, scope, &mut out, skipped.as_deref_mut())?;
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out.dedup_by(|a, b| a.path == b.path);
        Ok(Scan {
            samples: out,
            skipped: Vec::new(),
        })
    }

    fn walk(
        &self,
        dir: &Path,
        rel: &str,
        out: &mut Vec<CgroupSample>,
        mut skipped: Option<&mut Vec<(String, MeterError)>>,
    ) -> Result<(), MeterError> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if vanished(&e) => return Ok(()),
            Err(e) => return Err(MeterError::io(dir, e)),
        };
        let mut children = Vec::new();
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) if vanished(&e) => continue,
                Err(e) => return Err(MeterError::io(dir, e)),
            };
            match entry.file_type() {
                Ok(t) if t.is_dir() => {}
                Ok(_) => continue,
                Err(e) if vanished(&e) => continue,
                Err(e) => return Err(MeterError::io(entry.path(), e)),
            }
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            children.push(name);
        }
        children.sort();
        for name in children {
            let child_rel = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            let child = dir.join(&name);
            if self.targets.iter().any(|t| glob_match(t, &name)) {
                match read_cgroup(&child, child_rel.clone()) {
                    Ok(Some(sample)) => out.push(sample),
                    Ok(None) => {}
                    Err(e) => match skipped.as_deref_mut() {
                        Some(skipped) => skipped.push((child_rel, e)),
                        None => return Err(e),
                    },
                }
            } else {
                self.walk(&child, &child_rel, out, skipped.as_deref_mut())?;
            }
        }
        Ok(())
    }
}

/// Reads one cgroup directory. `None` when it vanished while being read.
pub fn read_cgroup(dir: &Path, rel: String) -> Result<Option<CgroupSample>, MeterError> {
    let inode = match fs::metadata(dir) {
        Ok(meta) => meta.ino(),
        Err(e) if vanished(&e) => return Ok(None),
        Err(e) => return Err(MeterError::io(dir, e)),
    };
    let Some(cpu_stat) = read_optional(&dir.join("cpu.stat"))? else {
        return Ok(None);
    };
    let cpu_usec = parse_keyed(&cpu_stat, "usage_usec")
        .ok_or_else(|| MeterError::parse(dir.join("cpu.stat"), "no usage_usec"))?;
    let memory_current = read_u64(&dir.join("memory.current"))?;
    let memory_peak = read_u64(&dir.join("memory.peak"))?;
    // No cgroup.events means the root cgroup, which is always populated.
    let populated = match read_optional(&dir.join("cgroup.events"))? {
        Some(events) => parse_keyed(&events, "populated") != Some(0),
        None => true,
    };
    Ok(Some(CgroupSample {
        path: rel,
        inode,
        cpu_usec,
        memory_current,
        memory_peak,
        populated,
    }))
}

/// A file's contents, or `None` if it does not exist (or vanished).
fn read_optional(path: &Path) -> Result<Option<String>, MeterError> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if vanished(&e) => Ok(None),
        Err(e) => Err(MeterError::io(path, e)),
    }
}

/// A single-value file such as `memory.current`. `max` reads as `None`.
fn read_u64(path: &Path) -> Result<Option<u64>, MeterError> {
    let Some(s) = read_optional(path)? else {
        return Ok(None);
    };
    let s = s.trim();
    if s == "max" {
        return Ok(None);
    }
    s.parse()
        .map(Some)
        .map_err(|_| MeterError::parse(path, format!("not a number: {s:?}")))
}

/// The value of `key` in a flat-keyed file (`cpu.stat`, `cgroup.events`).
pub fn parse_keyed(contents: &str, key: &str) -> Option<u64> {
    contents.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        (parts.next()? == key).then(|| parts.next()?.parse().ok())?
    })
}

fn vanished(e: &io::Error) -> bool {
    // ENODEV: reading a file of a cgroup that is being removed.
    matches!(e.kind(), io::ErrorKind::NotFound) || e.raw_os_error() == Some(19)
}

/// Matches `name` against `pattern`, where `*` matches any run of characters
/// (including none) and every other character matches itself.
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let p = pattern.as_bytes();
    let n = name.as_bytes();
    let (mut pi, mut ni) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ni < n.len() {
        if pi < p.len() && p[pi] == b'*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if pi < p.len() && p[pi] == n[ni] {
            pi += 1;
            ni += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == b'*')
}

/// The pod UID in a Kubernetes cgroup path, for the kube resolver (MB2).
/// The systemd driver writes `kubepods-<qos>-pod<uid>.slice` with the UID's
/// `-` replaced by `_`; the cgroupfs driver writes `pod<uid>`.
pub fn pod_uid_from_path(path: &str) -> Option<String> {
    path.split('/').rev().find_map(|component| {
        let rest = component.strip_suffix(".slice").unwrap_or(component);
        let idx = rest.rfind("pod")?;
        let uid = &rest[idx + 3..];
        let looks_like_uid = uid.len() >= 32
            && uid
                .chars()
                .all(|c| c.is_ascii_hexdigit() || c == '-' || c == '_');
        looks_like_uid.then(|| uid.replace('_', "-"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob() {
        assert!(glob_match("pod*", "pod1234"));
        assert!(glob_match(
            "kubepods-*pod*.slice",
            "kubepods-burstable-podab_cd.slice"
        ));
        assert!(glob_match("kubepods-*pod*.slice", "kubepods-podab.slice"));
        assert!(!glob_match(
            "kubepods-*pod*.slice",
            "kubepods-burstable.slice"
        ));
        assert!(glob_match("loams-*", "loams-"));
        assert!(!glob_match("loams-*", "xloam-a"));
        assert!(glob_match("*", ""));
        assert!(glob_match("a*b*c", "aXXbYYc"));
        assert!(!glob_match("a*b*c", "aXXbYY"));
    }

    #[test]
    fn keyed_files() {
        let stat = "usage_usec 1234\nuser_usec 1000\nsystem_usec 234\n";
        assert_eq!(parse_keyed(stat, "usage_usec"), Some(1234));
        assert_eq!(parse_keyed(stat, "system_usec"), Some(234));
        assert_eq!(parse_keyed(stat, "nr_periods"), None);
        assert_eq!(parse_keyed("populated 0\nfrozen 0\n", "populated"), Some(0));
    }

    #[test]
    fn pod_uids() {
        let systemd = "kubepods.slice/kubepods-burstable.slice/\
                       kubepods-burstable-pod0e8f2a1c_3b4d_4e5f_8a9b_0c1d2e3f4a5b.slice";
        assert_eq!(
            pod_uid_from_path(systemd).as_deref(),
            Some("0e8f2a1c-3b4d-4e5f-8a9b-0c1d2e3f4a5b")
        );
        let cgroupfs = "kubepods/besteffort/pod0e8f2a1c-3b4d-4e5f-8a9b-0c1d2e3f4a5b";
        assert_eq!(
            pod_uid_from_path(cgroupfs).as_deref(),
            Some("0e8f2a1c-3b4d-4e5f-8a9b-0c1d2e3f4a5b")
        );
        assert_eq!(pod_uid_from_path("system.slice/loams-org_a.scope"), None);
    }
}
