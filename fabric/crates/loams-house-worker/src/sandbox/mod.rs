//! L3, the worker's OS sandbox (HS1 Task 6, design §49 §13.2).
//!
//! [`Sandbox::apply`] runs in `main`, after the arguments and the inherited
//! sockets are taken and **before the engine boots**: libchdb is linked, not
//! `dlopen`ed, so its code is already mapped, and the Landlock rules cover the
//! file it was mapped from (HS1 R1.11). Nothing chDB does at boot or after (its
//! config, its first query, every user statement) runs outside the sandbox.
//!
//! In the `netns` mode (the default), in this order:
//!
//! 1. the process is alone (one thread), or nothing below is sound;
//! 2. it joins its cgroup when the front made one (`--cgroup`, [`cgroup`]);
//! 3. no core dumps (`RLIMIT_CORE` 0): a core holds query data;
//! 4. a user and network namespace of its own, `lo` up ([`netns`]);
//! 5. the forwarder's listener on `127.0.0.1:<port>` ([`crate::forwarder`]);
//! 6. every capability dropped (they were only ever the new namespace's);
//! 7. Landlock: read the code, write only the private directory, TCP only to the
//!    forwarder ([`landlock`]);
//! 8. seccomp: no `execve`, no new processes, no namespaces, mounts, `ptrace`,
//!    `bpf`, no socket families but IP ([`seccomp`]); `no_new_privs` is set by
//!    7 and 8;
//! 9. the forwarder's thread starts, inside all of it.
//!
//! `pods` skips 4 and 5: the pod's NetworkPolicy is the network boundary, and
//! Landlock lets the worker connect only to the port of its `--s3-endpoint`.
//! `none` does only 2, and the front logs that the worker is not sealed.
//!
//! What L3 does not do, and why (HS1 R6.6): no PID namespace (`/proc` stays the
//! host's, but Landlock lets the worker read only its own `/proc/self`, and the
//! user namespace already keeps it from other processes' `environ` and memory);
//! no distinct UID (an unprivileged front can map only its own; the deployment's
//! `runAsUser` is the distinct UID).

pub mod cgroup;
pub mod landlock;
pub mod netns;
pub mod seccomp;

use std::fmt;
use std::net::{Ipv4Addr, TcpListener};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use nix::sys::resource::{Resource, setrlimit};

pub use loams_house_ipc::SandboxMode;

use crate::forwarder::Forwarder;

/// A step of the sandbox that failed, and why. The worker does not boot.
#[derive(Debug)]
pub struct SandboxError {
    /// The step: `unshare(…)`, `landlock`, `seccomp`, …
    pub step: String,
    /// The OS's or the library's words.
    pub detail: String,
}

impl SandboxError {
    /// A failure of `step`.
    pub fn new(step: impl Into<String>, detail: impl fmt::Display) -> Self {
        Self {
            step: step.into(),
            detail: detail.to_string(),
        }
    }
}

impl fmt::Display for SandboxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sandbox: {}: {}", self.step, self.detail)
    }
}

impl std::error::Error for SandboxError {}

/// What [`Sandbox::apply`] is given.
#[derive(Debug)]
pub struct SandboxConfig {
    /// The mode (`--sandbox`).
    pub mode: SandboxMode,
    /// The worker's private directory: the only place it may write.
    pub tmp_dir: PathBuf,
    /// The cgroup the front made for this worker (`--cgroup`).
    pub cgroup: Option<PathBuf>,
    /// `netns`: the forwarder's port and its socket to the front (fd 4).
    pub forwarder: Option<(u16, OwnedFd)>,
    /// `pods`: the TCP ports the worker may connect to.
    pub connect_ports: Vec<u16>,
}

/// A sandbox in force.
#[derive(Debug)]
pub struct Sandbox {
    /// The mode.
    pub mode: SandboxMode,
    /// What Landlock enforced (`full`, `partial (…)`), when it ran.
    pub landlock: Option<String>,
    /// The forwarder, in the `netns` mode with one.
    pub forwarder: Option<Forwarder>,
}

impl Sandbox {
    /// Seals this process (see the module docs for the order). Must be called
    /// while the process has one thread.
    pub fn apply(config: SandboxConfig) -> Result<Self, SandboxError> {
        if config.mode != SandboxMode::None {
            let threads = threads()?;
            if threads != 1 {
                return Err(SandboxError::new(
                    "start",
                    format!("the process has {threads} threads; the sandbox needs one"),
                ));
            }
        }
        if let Some(dir) = &config.cgroup {
            cgroup::join(dir)?;
        }
        if config.mode == SandboxMode::None {
            return Ok(Self {
                mode: config.mode,
                landlock: None,
                forwarder: None,
            });
        }
        // Landlock's rule needs the directory to exist; the front makes it, and a
        // worker started by hand gets it here, before the namespaces.
        std::fs::create_dir_all(&config.tmp_dir)
            .map_err(|err| SandboxError::new("private directory", err))?;
        setrlimit(Resource::RLIMIT_CORE, 0, 0)
            .map_err(|err| SandboxError::new("RLIMIT_CORE", err))?;

        let mut connect_ports = config.connect_ports.clone();
        let mut listener = None;
        if config.mode == SandboxMode::Netns {
            netns::enter()?;
            netns::raise_loopback()?;
            if let Some((port, socket)) = config.forwarder {
                let bound = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
                    .map_err(|err| SandboxError::new("forwarder listener", err))?;
                connect_ports = vec![port];
                listener = Some((bound, UnixStream::from(socket)));
            } else {
                connect_ports.clear();
            }
        }
        loams_chdb_sys::capabilities::drop_all()
            .map_err(|err| SandboxError::new("capabilities", err))?;
        let rules = landlock::Rules::new(&config.tmp_dir, &connect_ports);
        let enforced = landlock::restrict(&rules)?;
        seccomp::install()?;
        let forwarder = match listener {
            Some((listener, channel)) => Some(
                Forwarder::start(listener, channel)
                    .map_err(|err| SandboxError::new("forwarder", err))?,
            ),
            None => None,
        };
        Ok(Self {
            mode: config.mode,
            landlock: Some(enforced),
            forwarder,
        })
    }
}

/// The process's thread count (`/proc/self/status`).
fn threads() -> Result<usize, SandboxError> {
    let status = std::fs::read_to_string("/proc/self/status")
        .map_err(|err| SandboxError::new("start", err))?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("Threads:"))
        .and_then(|count| count.trim().parse().ok())
        .ok_or_else(|| SandboxError::new("start", "no thread count in /proc/self/status"))
}
