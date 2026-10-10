//! The worker's Landlock rules (L3, §49 §13.2; HS1 R1.10, R1.11).
//!
//! What the worker may touch on the file system, once the rules are in force:
//!
//! * **read and write** its private directory (chDB's `--path`, its config, its
//!   working directory and `user_files_path`, R1.10) and nothing else;
//! * **read** the code it runs: every file mapped into the process when the
//!   rules are made (the worker binary, `libchdb.so` wherever the build put it,
//!   the C library), the system library directories, and the time zone data;
//! * **read** the few kernel files chDB sizes itself from (`/proc/self`, CPU and
//!   memory counts, its cgroup's limits) and `/dev/null`, `/dev/urandom`.
//!
//! Not `/etc` (only `/etc/localtime` and the loader's `/etc/ld.so.cache`), not
//! `/home`, not another process's
//! `/proc/<pid>`. TCP: no `bind` at all, and `connect` only to the ports given
//! (the forwarder's). From ABI 6 on, the worker also cannot signal or reach the
//! abstract Unix sockets of processes outside its domain, and from ABI 9 on it
//! cannot connect to any pathname Unix socket.
//!
//! Landlock ABI 4 (Linux 6.7) is required, for the TCP rules (R1.11); newer
//! rights are added where the kernel has them and reported in the status.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use landlock::{
    ABI, Access, AccessFs, AccessNet, BitFlags, CompatLevel, Compatible, NetPort, PathBeneath,
    PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetStatus, Scope,
};

use super::SandboxError;

/// The ABI the rules are written for: the newest the `landlock` crate knows.
const WRITTEN_FOR: ABI = ABI::V9;

/// The ABI the worker refuses to start without: TCP rules arrived in 4.
const REQUIRED: ABI = ABI::V4;

/// Directories whose files the worker may read (and map as code).
pub const READ_DIRS: &[&str] = &[
    "/usr/lib",
    "/usr/lib64",
    "/lib",
    "/lib64",
    "/usr/share/zoneinfo",
    "/sys/fs/cgroup",
    "/sys/devices/system/cpu",
    "/sys/devices/system/node",
];

/// Single files the worker may read.
pub const READ_FILES: &[&str] = &[
    "/etc/localtime",
    // The loader's cache of library paths, for a later `dlopen` (NSS).
    "/etc/ld.so.cache",
    "/proc/meminfo",
    "/proc/cpuinfo",
    "/proc/stat",
    "/proc/loadavg",
    "/proc/sys/kernel/pid_max",
    "/proc/sys/kernel/threads-max",
    "/proc/sys/vm/overcommit_memory",
    "/proc/sys/vm/max_map_count",
    "/proc/sys/kernel/random/boot_id",
    "/dev/urandom",
    "/dev/random",
];

/// What the rules allow, for the log line and the tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rules {
    /// Read and write.
    pub private_dir: PathBuf,
    /// Read only: directories and files, as given or found in `/proc/self/maps`.
    pub readable: Vec<PathBuf>,
    /// TCP ports the worker may connect to.
    pub connect_ports: Vec<u16>,
}

impl Rules {
    /// The rules for a worker whose private directory is `private_dir` and which
    /// may connect to `connect_ports`.
    pub fn new(private_dir: &Path, connect_ports: &[u16]) -> Self {
        let mut readable: BTreeSet<PathBuf> = READ_DIRS
            .iter()
            .chain(READ_FILES)
            .map(PathBuf::from)
            .collect();
        readable.extend(mapped_files());
        Self {
            private_dir: private_dir.to_path_buf(),
            readable: readable.into_iter().collect(),
            connect_ports: connect_ports.to_vec(),
        }
    }
}

/// Every file mapped into this process (`/proc/self/maps`): the binary, its
/// libraries, and `libchdb.so` wherever the build or the image put it.
fn mapped_files() -> BTreeSet<PathBuf> {
    let Ok(maps) = std::fs::read_to_string("/proc/self/maps") else {
        return BTreeSet::new();
    };
    maps.lines()
        .filter_map(|line| line.split_whitespace().nth(5))
        .filter(|path| path.starts_with('/'))
        .map(PathBuf::from)
        .collect()
}

/// Puts the rules in force for this thread and every thread it starts. Returns
/// what was enforced: `full` or `partial (ABI n)`.
pub fn restrict(rules: &Rules) -> Result<String, SandboxError> {
    let step = "landlock";
    let fail = |err: landlock::RulesetError| SandboxError::new(step, err);
    let read = AccessFs::from_read(WRITTEN_FOR);
    let all = AccessFs::from_all(WRITTEN_FOR);
    let mut ruleset = Ruleset::default()
        .set_compatibility(CompatLevel::HardRequirement)
        .handle_access(AccessFs::from_all(REQUIRED))
        .and_then(|r| r.handle_access(AccessNet::from_all(REQUIRED)))
        .map_err(fail)?
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(all)
        .and_then(|r| r.scope(Scope::from_all(WRITTEN_FOR)))
        .map_err(fail)?
        .create()
        .map_err(fail)?
        .add_rule(PathBeneath::new(
            PathFd::new(&rules.private_dir).map_err(|err| SandboxError::new(step, err))?,
            all,
        ))
        .map_err(fail)?;
    // `/dev/null` is written to as well as read.
    if let Ok(null) = PathFd::new("/dev/null") {
        ruleset = ruleset
            .add_rule(PathBeneath::new(
                null,
                AccessFs::ReadFile | AccessFs::WriteFile,
            ))
            .map_err(fail)?;
    }
    for path in &rules.readable {
        // A path this machine does not have is not an error (`/lib64` on some
        // distributions, `/sys/devices/system/node` without NUMA).
        let Ok(fd) = PathFd::new(path) else {
            continue;
        };
        let access: BitFlags<AccessFs> = if path.is_dir() {
            read
        } else {
            AccessFs::from_file(WRITTEN_FOR) & read
        };
        ruleset = ruleset
            .add_rule(PathBeneath::new(fd, access))
            .map_err(fail)?;
    }
    // `/proc/self` resolves to this process's own directory when the rule is
    // made, so the rule covers it and none of another process's.
    if let Ok(fd) = PathFd::new("/proc/self") {
        ruleset = ruleset.add_rule(PathBeneath::new(fd, read)).map_err(fail)?;
    }
    for port in &rules.connect_ports {
        ruleset = ruleset
            .add_rule(NetPort::new(*port, AccessNet::ConnectTcp))
            .map_err(fail)?;
    }
    let status = ruleset.restrict_self().map_err(fail)?;
    match status.ruleset {
        RulesetStatus::FullyEnforced => Ok("full".to_string()),
        RulesetStatus::PartiallyEnforced => Ok(format!("partial ({:?})", status.landlock)),
        RulesetStatus::NotEnforced => Err(SandboxError::new(
            step,
            "the kernel enforced none of the rules",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_read_the_code_and_write_only_the_private_dir() {
        let rules = Rules::new(Path::new("/w/7"), &[9180]);
        assert_eq!(rules.private_dir, PathBuf::from("/w/7"));
        assert_eq!(rules.connect_ports, vec![9180]);
        // The test binary itself is mapped, so it is readable.
        let exe = std::env::current_exe().expect("the test binary");
        assert!(
            rules.readable.iter().any(|p| p == &exe),
            "{exe:?} in {:?}",
            rules.readable
        );
        for denied in [
            "/etc",
            "/etc/passwd",
            "/etc/hostname",
            "/home",
            "/proc",
            "/tmp",
        ] {
            assert!(
                !rules.readable.iter().any(|p| p == Path::new(denied)),
                "{denied} must not be readable"
            );
        }
        assert!(
            !rules.readable.iter().any(|p| p.starts_with("/etc")
                && p != Path::new("/etc/localtime")
                && p != Path::new("/etc/ld.so.cache")),
            "only /etc/localtime and /etc/ld.so.cache under /etc: {:?}",
            rules.readable
        );
    }
}
