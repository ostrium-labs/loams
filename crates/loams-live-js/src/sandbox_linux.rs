//! The isolated worker's Linux sandbox (LV1 plan Task 5; design §45 §3.1,
//! D681), applied once the worker has read `Load` and created its runtime,
//! before it evaluates any of the bundle:
//!
//! 1. a landlock ruleset that handles every filesystem and network access
//!    the kernel knows and grants none, and scopes signals and abstract
//!    UNIX sockets to the sandbox (best effort: a kernel without landlock
//!    still gets the rest);
//! 2. `PR_SET_NO_NEW_PRIVS`;
//! 3. `RLIMIT_NOFILE` = 3 (stdin, stdout and stderr) and `RLIMIT_AS` = the
//!    address space already mapped plus the memory limit plus 64 MiB;
//! 4. a seccomp filter that allows only [`ALLOWED`] (`read` and `write` on
//!    fds 0–2 only) and kills the whole process with `SIGSYS` on anything
//!    else.
//!
//! The sandbox refuses a process with more than one thread: landlock and
//! `no_new_privs` bind the calling thread only, and an unsandboxed thread
//! beside a compromised one would be an escape. The seccomp filter is
//! synchronised to every thread all the same.

use std::collections::BTreeMap;

use landlock::{ABI, Access, AccessFs, AccessNet, Ruleset, RulesetAttr, RulesetStatus, Scope};
use rustix::process::{Resource, Rlimit, setrlimit};
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule, TargetArch,
};

/// The address space a worker may map beyond its runtime's memory limit:
/// its frames, its Rust-side values and the stack (LV1 plan Task 5).
pub(crate) const AS_HEADROOM: u64 = 64 * 1024 * 1024;

/// The system calls a sandboxed worker may make. `read` and `write` are
/// further limited to fds 0–2. `mremap` is beyond the plan's list: the C
/// allocator grows a large block (a QuickJS string or array) in place with
/// it (LV1 row T5-3).
const ALLOWED: &[(&str, i64)] = &[
    ("read", libc::SYS_read),
    ("write", libc::SYS_write),
    ("futex", libc::SYS_futex),
    ("mmap", libc::SYS_mmap),
    ("munmap", libc::SYS_munmap),
    ("mremap", libc::SYS_mremap),
    ("mprotect", libc::SYS_mprotect),
    ("brk", libc::SYS_brk),
    ("madvise", libc::SYS_madvise),
    ("clock_gettime", libc::SYS_clock_gettime),
    ("sched_yield", libc::SYS_sched_yield),
    ("rt_sigreturn", libc::SYS_rt_sigreturn),
    ("exit_group", libc::SYS_exit_group),
];

/// The system calls limited to fds 0–2 (stdin, stdout, stderr).
const STDIO_ONLY: &[i64] = &[libc::SYS_read, libc::SYS_write];

/// Applies the sandbox. `memory_limit` is the runtime's limit, which
/// `RLIMIT_AS` adds to the address space mapped so far.
pub(crate) fn apply(memory_limit: usize) -> Result<(), String> {
    // Read before landlock, which forbids it.
    let status = std::fs::read_to_string("/proc/self/status")
        .map_err(|e| format!("reading /proc/self/status: {e}"))?;
    let threads = status_field(&status, "Threads:")?;
    if threads != 1 {
        return Err(format!(
            "the worker has {threads} threads; it sandboxes itself only single-threaded"
        ));
    }
    let mapped = status_field(&status, "VmSize:")?.saturating_mul(1024);
    let filter = filter()?;
    restrict_filesystem_and_network()?;
    rustix::thread::set_no_new_privs(true).map_err(|e| format!("no_new_privs: {e}"))?;
    limit(Resource::Nofile, 3)?;
    let address_space = mapped
        .saturating_add(memory_limit as u64)
        .saturating_add(AS_HEADROOM);
    limit(Resource::As, address_space)?;
    seccompiler::apply_filter_all_threads(&filter).map_err(|e| format!("seccomp: {e}"))
}

/// The number in a `/proc/self/status` line (`Threads:`, or `VmSize:` in
/// kB).
fn status_field(status: &str, name: &str) -> Result<u64, String> {
    status
        .lines()
        .find_map(|l| l.strip_prefix(name))
        .map(|v| v.trim().trim_end_matches("kB").trim())
        .and_then(|v| v.parse::<u64>().ok())
        .ok_or_else(|| format!("no {name} in /proc/self/status"))
}

/// Sets both the soft and the hard limit of `resource` (the hard one, so
/// the worker cannot raise it again).
fn limit(resource: Resource, value: u64) -> Result<(), String> {
    setrlimit(
        resource,
        Rlimit {
            current: Some(value),
            maximum: Some(value),
        },
    )
    .map_err(|e| format!("setrlimit {resource:?}: {e}"))
}

fn restrict_filesystem_and_network() -> Result<(), String> {
    let abi = ABI::V6;
    let status = Ruleset::default()
        .handle_access(AccessFs::from_all(abi))
        .and_then(|r| r.handle_access(AccessNet::from_all(abi)))
        .and_then(|r| r.scope(Scope::from_all(abi)))
        .and_then(|r| r.create())
        .and_then(|r| r.restrict_self())
        .map_err(|e| format!("landlock: {e}"))?;
    if status.ruleset == RulesetStatus::NotEnforced {
        // seccomp still forbids every open, socket and exec.
        eprintln!(
            "loams live-worker: this kernel has no landlock; the seccomp filter still applies"
        );
    }
    Ok(())
}

fn filter() -> Result<BpfProgram, String> {
    let stdio = || -> Result<Vec<SeccompRule>, String> {
        let fd_at_most_2 = SeccompCondition::new(0, SeccompCmpArgLen::Qword, SeccompCmpOp::Le, 2)
            .map_err(|e| format!("seccomp condition: {e}"))?;
        Ok(vec![
            SeccompRule::new(vec![fd_at_most_2]).map_err(|e| format!("seccomp rule: {e}"))?,
        ])
    };
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
    for (_, nr) in ALLOWED {
        let chain = if STDIO_ONLY.contains(nr) {
            stdio()?
        } else {
            Vec::new()
        };
        rules.insert(*nr, chain);
    }
    let arch: TargetArch = std::env::consts::ARCH
        .try_into()
        .map_err(|e| format!("seccomp on {}: {e:?}", std::env::consts::ARCH))?;
    SeccompFilter::new(
        rules,
        SeccompAction::KillProcess,
        SeccompAction::Allow,
        arch,
    )
    .and_then(BpfProgram::try_from)
    .map_err(|e| format!("seccomp filter: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filter_compiles_and_the_address_space_is_read() {
        assert!(filter().is_ok());
        let status = std::fs::read_to_string("/proc/self/status").expect("status");
        assert!(status_field(&status, "VmSize:").expect("VmSize") > 0);
        assert!(status_field(&status, "Threads:").expect("Threads") >= 1);
        assert!(status_field(&status, "Nothing:").is_err());
        let names: Vec<&str> = ALLOWED.iter().map(|(n, _)| *n).collect();
        for forbidden in ["open", "openat", "socket", "execve", "clone", "ptrace"] {
            assert!(!names.contains(&forbidden), "{forbidden} is not allowed");
        }
    }
}
