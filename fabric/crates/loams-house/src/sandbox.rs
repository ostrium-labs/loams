//! The front's half of L3, the worker sandbox (HS1 Task 6, design §49 §12,
//! §13.2): the forwarder's far end and the workers' cgroups.
//!
//! The worker seals itself (`loams_house_worker::sandbox`): its own user and
//! network namespace, Landlock, seccomp. What only the front can do is here.
//!
//! # The forwarder
//!
//! A `netns` worker's only way out is the socket it inherits on fd 4. Its
//! forwarder thread sends one byte per connection chDB makes; [`serve_forwarder`]
//! answers each with one end of a fresh socket pair (`SCM_RIGHTS`) and serves the
//! other: today it relays to the configured upstream; from HS1 Task 9 it is
//! `house-cache`, which knows the worker by this socket. At most
//! [`MAX_FORWARDED_CONNECTIONS`] are open per worker; past that a request is
//! answered with a stream that is already closed. Nothing on this path carries a
//! credential to the worker.
//!
//! # Cgroups
//!
//! When the front owns a delegated cgroup v2 directory ([`WorkerCgroups`]), each
//! worker gets a child with `memory.max`, `memory.swap.max`, `cpu.max` and
//! `pids.max` written before it starts; the worker joins it first thing
//! (`--cgroup`), and the waiter removes it after reaping the worker.

use std::fmt;
use std::io::{self, IoSlice, Read};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use nix::sys::socket::{ControlMessage, MsgFlags, UnixAddr, sendmsg};

use crate::errors::{ChError, HouseError};

pub use loams_house_ipc::{FORWARDER_PORT, FORWARDER_SOCKET_FD, SandboxMode};

/// The most connections one worker's forwarder holds open at once.
pub const MAX_FORWARDED_CONNECTIONS: usize = 64;

/// How long the front waits to reach the upstream for a forwarded connection.
pub const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// The sandbox a process launcher gives its workers when nothing says otherwise:
/// `netns` on Linux, and nothing on other systems (macOS development builds,
/// which have no L3 and say so at start).
pub fn default_mode() -> SandboxMode {
    if cfg!(target_os = "linux") {
        SandboxMode::Netns
    } else {
        SandboxMode::None
    }
}

/// The warning the front logs when workers run without L3.
pub fn unsandboxed_warning(why: &str) -> String {
    format!(
        "loams-house: WARNING: House workers run WITHOUT the OS sandbox (L3, --sandbox=none; {why}). \
         A query that gets past the deny list can read and write this machine's files and \
         reach its network. Never serve untrusted users like this."
    )
}

/// Serves one worker's forwarder channel on a thread until the worker's end
/// closes. `worker` names it in thread names.
pub fn serve_forwarder(
    channel: UnixStream,
    upstream: SocketAddr,
    worker: &str,
) -> io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name(format!("house-forwarder-{worker}"))
        .spawn(move || forwarder_loop(&channel, upstream))
}

fn forwarder_loop(channel: &UnixStream, upstream: SocketAddr) {
    let open = Arc::new(AtomicUsize::new(0));
    let mut reader = channel;
    let mut request = [0u8; 1];
    loop {
        match reader.read(&mut request) {
            Ok(0) => return,
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return,
        }
        let Ok((ours, theirs)) = UnixStream::pair() else {
            return;
        };
        if send_socket(channel, &theirs).is_err() {
            return;
        }
        drop(theirs);
        if open.fetch_add(1, Ordering::SeqCst) >= MAX_FORWARDED_CONNECTIONS {
            // Over the cap: the worker's end closes at once.
            open.fetch_sub(1, Ordering::SeqCst);
            continue;
        }
        let open = Arc::clone(&open);
        let spawned = std::thread::Builder::new()
            .name("house-forwarded".to_string())
            .spawn(move || {
                if let Ok(conn) = TcpStream::connect_timeout(&upstream, UPSTREAM_CONNECT_TIMEOUT) {
                    relay(ours, conn);
                }
                open.fetch_sub(1, Ordering::SeqCst);
            });
        if spawned.is_err() {
            return;
        }
    }
}

/// Sends `socket` over `channel` with one byte (`SCM_RIGHTS`).
fn send_socket(channel: &UnixStream, socket: &UnixStream) -> io::Result<()> {
    let fds = [socket.as_raw_fd()];
    let control = [ControlMessage::ScmRights(&fds)];
    let byte = [1u8];
    sendmsg::<UnixAddr>(
        channel.as_raw_fd(),
        &[IoSlice::new(&byte)],
        &control,
        MsgFlags::empty(),
        None,
    )
    .map(drop)
    .map_err(io::Error::from)
}

/// Copies both ways until both directions end. Runs on the calling thread and
/// one more.
fn relay(worker: UnixStream, upstream: TcpStream) {
    let (Ok(worker_back), Ok(upstream_back)) = (worker.try_clone(), upstream.try_clone()) else {
        return;
    };
    let out = std::thread::Builder::new()
        .name("house-forwarded-out".to_string())
        .spawn(move || {
            let mut from = worker;
            let mut to = upstream;
            let _ = io::copy(&mut from, &mut to);
            let _ = to.shutdown(Shutdown::Write);
        });
    let mut from = upstream_back;
    let mut to = worker_back;
    let _ = io::copy(&mut from, &mut to);
    let _ = to.shutdown(Shutdown::Write);
    if let Ok(out) = out {
        let _ = out.join();
    }
}

/// Per-worker cgroup limits (§49 §12).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CgroupLimits {
    /// `memory.max`, in bytes; `memory.swap.max` is 0 when set.
    pub memory_max: Option<u64>,
    /// `cpu.max`: CPUs' worth of time per period (`cpus × 100 000` µs per
    /// 100 000 µs).
    pub cpus: Option<u32>,
    /// `pids.max`: processes and threads.
    pub pids_max: Option<u64>,
}

impl CgroupLimits {
    /// §49 §12's shared class: the worker's memory (the query cap plus 512 MiB
    /// headroom), 8 CPUs, and 2 048 threads (chDB's pools at 8 threads per query
    /// stay far below it; measured idle: see HS1 R6.5).
    pub fn for_worker(memory_limit: u64) -> Self {
        Self {
            memory_max: Some(memory_limit),
            cpus: Some(8),
            pids_max: Some(2048),
        }
    }

    /// The controllers these limits need.
    fn controllers(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.memory_max.is_some() {
            out.push("memory");
        }
        if self.cpus.is_some() {
            out.push("cpu");
        }
        if self.pids_max.is_some() {
            out.push("pids");
        }
        out
    }
}

/// A delegated cgroup v2 directory the front owns, under which each worker
/// gets a child.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerCgroups {
    root: PathBuf,
    limits: CgroupLimits,
}

fn cgroup_error(what: impl fmt::Display, err: impl fmt::Display) -> HouseError {
    HouseError::from(ChError::network_error(format!(
        "House worker cgroup: {what}: {err}"
    )))
}

impl WorkerCgroups {
    /// Takes `root`, a cgroup v2 directory this process may write and which
    /// holds no process of its own (cgroup v2's "no internal processes"), and
    /// enables the controllers `limits` need for its children.
    pub fn prepare(root: impl Into<PathBuf>, limits: CgroupLimits) -> Result<Self, HouseError> {
        let root = root.into();
        let available = std::fs::read_to_string(root.join("cgroup.controllers"))
            .map_err(|err| cgroup_error(root.display(), err))?;
        let wanted = limits.controllers();
        if let Some(missing) = wanted
            .iter()
            .find(|name| !available.split_whitespace().any(|have| have == **name))
        {
            return Err(cgroup_error(
                root.display(),
                format!("the {missing} controller is not delegated here ({available:?})"),
            ));
        }
        let enable: Vec<String> = wanted.iter().map(|name| format!("+{name}")).collect();
        if !enable.is_empty() {
            std::fs::write(root.join("cgroup.subtree_control"), enable.join(" "))
                .map_err(|err| cgroup_error(root.join("cgroup.subtree_control").display(), err))?;
        }
        Ok(Self { root, limits })
    }

    /// The limits each child gets.
    pub fn limits(&self) -> CgroupLimits {
        self.limits
    }

    /// The child directory of worker `id`.
    pub fn dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    /// Makes worker `id`'s child and writes its limits.
    pub fn create(&self, id: &str) -> Result<PathBuf, HouseError> {
        let dir = self.dir(id);
        std::fs::create_dir(&dir).map_err(|err| cgroup_error(dir.display(), err))?;
        let written = self.write_limits(&dir);
        if let Err(err) = written {
            remove(&dir);
            return Err(err);
        }
        Ok(dir)
    }

    fn write_limits(&self, dir: &Path) -> Result<(), HouseError> {
        let write = |file: &str, value: String| {
            std::fs::write(dir.join(file), value)
                .map_err(|err| cgroup_error(dir.join(file).display(), err))
        };
        if let Some(bytes) = self.limits.memory_max {
            write("memory.max", bytes.to_string())?;
            // No swap: a worker over its memory is killed (241), not slowed.
            if dir.join("memory.swap.max").exists() {
                write("memory.swap.max", "0".to_string())?;
            }
        }
        if let Some(cpus) = self.limits.cpus {
            write("cpu.max", format!("{} 100000", u64::from(cpus) * 100_000))?;
        }
        if let Some(pids) = self.limits.pids_max {
            write("pids.max", pids.to_string())?;
        }
        Ok(())
    }
}

/// Removes a worker's cgroup child once its process is reaped (best effort: a
/// child that still has a process cannot be removed, and a later sweep or the
/// operator's own cleanup takes it).
pub fn remove(dir: &Path) {
    let _ = std::fs::remove_dir(dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_name_their_controllers() {
        let limits = CgroupLimits::for_worker(1 << 30);
        assert_eq!(limits.controllers(), vec!["memory", "cpu", "pids"]);
        let none = CgroupLimits {
            memory_max: None,
            cpus: None,
            pids_max: Some(10),
        };
        assert_eq!(none.controllers(), vec!["pids"]);
    }

    #[test]
    fn a_directory_that_is_not_a_cgroup_is_refused() {
        let dir = std::env::temp_dir().join(format!("loams-not-a-cgroup-{}", std::process::id()));
        assert!(WorkerCgroups::prepare(&dir, CgroupLimits::for_worker(1 << 30)).is_err());
    }

    #[test]
    fn the_default_is_sealed_on_linux() {
        if cfg!(target_os = "linux") {
            assert_eq!(default_mode(), SandboxMode::Netns);
        }
        assert!(unsandboxed_warning("forced").contains("WITHOUT the OS sandbox"));
    }

    #[test]
    fn the_forwarder_answers_a_request_with_one_socket() {
        use std::io::Write as _;
        let upstream = std::net::TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = upstream.local_addr().expect("address");
        let (front, worker) = UnixStream::pair().expect("pair");
        let _serving = serve_forwarder(front, address, "test").expect("serves");
        let mut ask = &worker;
        ask.write_all(b"c").expect("ask");
        // Adopting the received descriptor needs `unsafe` (the worker does it in
        // `loams-chdb-sys`); here the message is checked: one byte, one socket.
        // The worker tests carry bytes through it end to end.
        let mut byte = [0u8; 1];
        let mut space = nix::cmsg_space!([std::os::fd::RawFd; 1]);
        let mut iov = [std::io::IoSliceMut::new(&mut byte)];
        let message = nix::sys::socket::recvmsg::<()>(
            worker.as_raw_fd(),
            &mut iov,
            Some(&mut space),
            MsgFlags::MSG_CMSG_CLOEXEC,
        )
        .expect("a message");
        let fds: Vec<_> = message
            .cmsgs()
            .expect("control messages")
            .filter_map(|cmsg| match cmsg {
                nix::sys::socket::ControlMessageOwned::ScmRights(fds) => Some(fds),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(fds.len(), 1);
        // The front connects to the upstream for it.
        upstream.set_nonblocking(false).expect("blocking accept");
        let (_conn, _) = upstream.accept().expect("the forwarded connection");
    }
}
