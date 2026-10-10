//! The worker's seccomp filter (L3, §49 §13.2).
//!
//! Allow by default, deny by list: chDB is a large program whose system calls
//! are not ours to enumerate, while what a sealed worker must never do is short.
//! Denied calls fail with `EPERM` rather than killing the process, so chDB turns
//! them into an ordinary query error (`executable()`, a `url()` that slipped
//! through) instead of a crash.
//!
//! * **No new programs or processes**: `execve`, `execveat`, `fork`, `vfork`, and
//!   `clone` without `CLONE_THREAD`. Threads still start: `clone3` answers
//!   `ENOSYS`, which makes glibc's `pthread_create` fall back to `clone`, whose
//!   flags a filter can read (`clone3` keeps them in memory, where it cannot).
//! * **No way out of the namespaces or the rules**: `unshare`, `setns`, the mount
//!   family, `pivot_root`, `chroot`.
//! * **No reaching into other processes or the kernel**: `ptrace`,
//!   `process_vm_readv`/`writev`, `pidfd_getfd`, `kcmp`, `bpf`,
//!   `perf_event_open`, `userfaultfd`, the key ring, module loading, `kexec`,
//!   `reboot`, swap, clocks, `syslog`, `acct`, `quotactl`, file handles,
//!   `iopl`/`ioperm`.
//! * **Only IP sockets**: `socket` with any family but `AF_INET`/`AF_INET6` is
//!   refused, so no pathname Unix socket on the host can be reached (`socketpair`
//!   stays, for the forwarder), and no netlink after `lo` is up.
//! * **No io_uring** (`ENOSYS`): its operations do not pass through this filter.

use std::collections::BTreeMap;

use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule, TargetArch,
};

use super::SandboxError;

/// `CLONE_THREAD`.
const CLONE_THREAD: u64 = 0x0001_0000;

/// System calls refused outright with `EPERM`.
pub fn denied() -> Vec<(&'static str, i64)> {
    let mut calls = vec![
        ("execve", libc::SYS_execve),
        ("execveat", libc::SYS_execveat),
        ("unshare", libc::SYS_unshare),
        ("setns", libc::SYS_setns),
        ("mount", libc::SYS_mount),
        ("umount2", libc::SYS_umount2),
        ("pivot_root", libc::SYS_pivot_root),
        ("chroot", libc::SYS_chroot),
        ("open_tree", libc::SYS_open_tree),
        ("move_mount", libc::SYS_move_mount),
        ("fsopen", libc::SYS_fsopen),
        ("fsconfig", libc::SYS_fsconfig),
        ("fsmount", libc::SYS_fsmount),
        ("fspick", libc::SYS_fspick),
        ("mount_setattr", libc::SYS_mount_setattr),
        ("ptrace", libc::SYS_ptrace),
        ("process_vm_readv", libc::SYS_process_vm_readv),
        ("process_vm_writev", libc::SYS_process_vm_writev),
        ("pidfd_getfd", libc::SYS_pidfd_getfd),
        ("kcmp", libc::SYS_kcmp),
        ("bpf", libc::SYS_bpf),
        ("perf_event_open", libc::SYS_perf_event_open),
        ("userfaultfd", libc::SYS_userfaultfd),
        ("keyctl", libc::SYS_keyctl),
        ("add_key", libc::SYS_add_key),
        ("request_key", libc::SYS_request_key),
        ("init_module", libc::SYS_init_module),
        ("finit_module", libc::SYS_finit_module),
        ("delete_module", libc::SYS_delete_module),
        ("kexec_load", libc::SYS_kexec_load),
        ("kexec_file_load", libc::SYS_kexec_file_load),
        ("reboot", libc::SYS_reboot),
        ("swapon", libc::SYS_swapon),
        ("swapoff", libc::SYS_swapoff),
        ("syslog", libc::SYS_syslog),
        ("acct", libc::SYS_acct),
        ("quotactl", libc::SYS_quotactl),
        ("settimeofday", libc::SYS_settimeofday),
        ("clock_settime", libc::SYS_clock_settime),
        ("clock_adjtime", libc::SYS_clock_adjtime),
        ("adjtimex", libc::SYS_adjtimex),
        ("sethostname", libc::SYS_sethostname),
        ("setdomainname", libc::SYS_setdomainname),
        ("name_to_handle_at", libc::SYS_name_to_handle_at),
        ("open_by_handle_at", libc::SYS_open_by_handle_at),
        ("fanotify_init", libc::SYS_fanotify_init),
        ("vhangup", libc::SYS_vhangup),
    ];
    #[cfg(target_arch = "x86_64")]
    calls.extend([
        ("fork", libc::SYS_fork),
        ("vfork", libc::SYS_vfork),
        ("iopl", libc::SYS_iopl),
        ("ioperm", libc::SYS_ioperm),
        ("uselib", libc::SYS_uselib),
    ]);
    calls
}

/// System calls that answer `ENOSYS`, so callers fall back to what the filter
/// can see (`clone3` → `clone`) or do without (io_uring).
pub fn unavailable() -> Vec<(&'static str, i64)> {
    vec![
        ("clone3", libc::SYS_clone3),
        ("io_uring_setup", libc::SYS_io_uring_setup),
        ("io_uring_enter", libc::SYS_io_uring_enter),
        ("io_uring_register", libc::SYS_io_uring_register),
    ]
}

fn arch() -> Result<TargetArch, SandboxError> {
    TargetArch::try_from(std::env::consts::ARCH).map_err(|err| SandboxError::new("seccomp", err))
}

fn condition(arg: u8, op: SeccompCmpOp, value: u64) -> Result<SeccompCondition, SandboxError> {
    SeccompCondition::new(arg, SeccompCmpArgLen::Qword, op, value)
        .map_err(|err| SandboxError::new("seccomp", err))
}

fn rule(conditions: Vec<SeccompCondition>) -> Result<SeccompRule, SandboxError> {
    SeccompRule::new(conditions).map_err(|err| SandboxError::new("seccomp", err))
}

fn compile(rules: BTreeMap<i64, Vec<SeccompRule>>, errno: i32) -> Result<BpfProgram, SandboxError> {
    let errno = u32::try_from(errno).map_err(|err| SandboxError::new("seccomp", err))?;
    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(errno),
        arch()?,
    )
    .map_err(|err| SandboxError::new("seccomp", err))?;
    BpfProgram::try_from(filter).map_err(|err| SandboxError::new("seccomp", err))
}

/// The two filters: `EPERM` for [`denied`] and the argument rules, `ENOSYS` for
/// [`unavailable`].
pub fn filters() -> Result<[BpfProgram; 2], SandboxError> {
    let mut refused: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
    for (_, number) in denied() {
        refused.insert(number, Vec::new());
    }
    // `clone` that makes a process rather than a thread.
    refused.insert(
        libc::SYS_clone,
        vec![rule(vec![condition(
            0,
            SeccompCmpOp::MaskedEq(CLONE_THREAD),
            0,
        )?])?],
    );
    // `socket` of any family but IPv4 and IPv6 (the conditions of one rule all
    // have to hold).
    refused.insert(
        libc::SYS_socket,
        vec![rule(vec![
            condition(0, SeccompCmpOp::Ne, libc::AF_INET as u64)?,
            condition(0, SeccompCmpOp::Ne, libc::AF_INET6 as u64)?,
        ])?],
    );
    let mut missing: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
    for (_, number) in unavailable() {
        missing.insert(number, Vec::new());
    }
    Ok([
        compile(refused, libc::EPERM)?,
        compile(missing, libc::ENOSYS)?,
    ])
}

/// Installs the filters on every thread of the process (`TSYNC`); threads
/// started later inherit them.
pub fn install() -> Result<(), SandboxError> {
    for filter in filters()? {
        seccompiler::apply_filter_all_threads(&filter)
            .map_err(|err| SandboxError::new("seccomp", err))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_compile_for_this_machine() {
        let [refused, missing] = filters().expect("the filters compile");
        assert!(!refused.is_empty());
        assert!(!missing.is_empty());
        let names: Vec<&str> = denied().iter().map(|(name, _)| *name).collect();
        for name in [
            "execve", "execveat", "ptrace", "mount", "unshare", "setns", "bpf",
        ] {
            assert!(names.contains(&name), "{name} must be denied");
        }
    }
}
