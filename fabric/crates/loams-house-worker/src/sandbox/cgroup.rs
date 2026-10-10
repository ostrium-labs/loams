//! Joining the cgroup v2 child the front made for this worker (L3, §49 §12,
//! §13.2; HS1 Task 6).
//!
//! When the front owns a delegated cgroup, it makes one child per worker, writes
//! its `memory.max`, `cpu.max` and `pids.max` (`loams_house::sandbox`), and names
//! it with `--cgroup`. The worker moves itself in before it does anything else,
//! so every byte and thread libchdb ever takes is counted against the limits. A
//! worker that cannot join refuses to boot.

use std::path::Path;

use super::SandboxError;

/// Moves this process into the cgroup at `dir` (writes `0`, "the writer", to its
/// `cgroup.procs`) and checks that it is there.
pub fn join(dir: &Path) -> Result<(), SandboxError> {
    let step = "cgroup";
    std::fs::write(dir.join("cgroup.procs"), "0")
        .map_err(|err| SandboxError::new(step, format!("{}: {err}", dir.display())))?;
    let ours =
        std::fs::read_to_string("/proc/self/cgroup").map_err(|err| SandboxError::new(step, err))?;
    let joined = ours
        .lines()
        .filter_map(|line| line.strip_prefix("0::"))
        .map(|path| path.trim_start_matches('/'))
        .any(|path| !path.is_empty() && dir.ends_with(path));
    if joined {
        Ok(())
    } else {
        Err(SandboxError::new(
            step,
            format!("still in {:?} after joining {}", ours.trim(), dir.display()),
        ))
    }
}
