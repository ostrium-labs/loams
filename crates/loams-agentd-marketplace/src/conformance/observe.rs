//! What the agent's processes do, seen from outside (Linux, through
//! `/proc`): its descendants, the files they hold open and the remote ends of
//! their TCP sockets. It samples every 50 ms, so it sees long-lived sockets
//! and open files, not every short one; the egress proxy is the main record
//! and SB1's sandbox makes both complete. Elsewhere it records nothing and
//! says so.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// What sampling saw over the run.
#[derive(Debug, Clone, Default)]
pub(super) struct Observed {
    /// Whether this platform samples at all (Linux only).
    pub available: bool,
    /// `ip:port` of TCP peers that are not loopback.
    pub remote_peers: BTreeSet<String>,
    /// Regular files held open for writing.
    pub writes: BTreeSet<PathBuf>,
    /// Regular files held open for reading.
    pub reads: BTreeSet<PathBuf>,
    /// Every process of the agent seen: pid → start time (clock ticks).
    pub processes: BTreeMap<i32, u64>,
}

#[cfg(target_os = "linux")]
mod linux {
    use std::collections::{BTreeMap, BTreeSet};
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use super::Observed;

    const INTERVAL: Duration = Duration::from_millis(50);

    #[derive(Debug)]
    pub(in super::super) struct Observer {
        stop: Arc<AtomicBool>,
        handle: Option<std::thread::JoinHandle<Observed>>,
    }

    impl Observer {
        /// Samples `root` (which leads its own process group) and its
        /// descendants until [`Observer::finish`].
        pub(in super::super) fn start(root: i32) -> Self {
            let stop = Arc::new(AtomicBool::new(false));
            let flag = Arc::clone(&stop);
            let handle = std::thread::Builder::new()
                .name("lapc-observe".into())
                .spawn(move || {
                    let mut observed = Observed {
                        available: true,
                        ..Observed::default()
                    };
                    let inherited = own_fds();
                    while !flag.load(Ordering::SeqCst) {
                        sample(root, &inherited, &mut observed);
                        std::thread::sleep(INTERVAL);
                    }
                    sample(root, &inherited, &mut observed);
                    observed
                })
                .ok();
            Self { stop, handle }
        }

        pub(in super::super) fn finish(mut self) -> Observed {
            self.stop.store(true, Ordering::SeqCst);
            self.handle
                .take()
                .and_then(|h| h.join().ok())
                .unwrap_or_default()
        }
    }

    impl Drop for Observer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
        }
    }

    /// `(ppid, pgrp, starttime)` from `/proc/<pid>/stat`.
    pub(in super::super) fn stat(pid: i32) -> Option<(i32, i32, u64)> {
        let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // The command name is in parentheses and may hold anything.
        let after = &text[text.rfind(')')? + 1..];
        let fields: Vec<&str> = after.split_whitespace().collect();
        // fields[0] is the state (field 3); starttime is field 22.
        Some((
            fields.get(1)?.parse().ok()?,
            fields.get(2)?.parse().ok()?,
            fields.get(19)?.parse().ok()?,
        ))
    }

    /// Whether the process `pid` that started at `start` still runs.
    pub(in super::super) fn alive(pid: i32, start: u64) -> bool {
        stat(pid).is_some_and(|(_, _, s)| s == start)
    }

    fn members(root: i32) -> BTreeMap<i32, u64> {
        let mut all = Vec::new();
        if let Ok(entries) = std::fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let Some(pid) = entry
                    .file_name()
                    .to_str()
                    .and_then(|n| n.parse::<i32>().ok())
                else {
                    continue;
                };
                if let Some((ppid, pgrp, start)) = stat(pid) {
                    all.push((pid, ppid, pgrp, start));
                }
            }
        }
        let mut members: BTreeMap<i32, u64> = all
            .iter()
            .filter(|(pid, _, pgrp, _)| *pid == root || *pgrp == root)
            .map(|(pid, _, _, start)| (*pid, *start))
            .collect();
        // Descendants that left the group (setsid) still count.
        loop {
            let before = members.len();
            for (pid, ppid, _, start) in &all {
                if members.contains_key(ppid) {
                    members.entry(*pid).or_insert(*start);
                }
            }
            if members.len() == before {
                return members;
            }
        }
    }

    /// The runner's own descriptors: number → target. A child inherits any
    /// that lack close-on-exec (a build tool's lock file, say) at the same
    /// number; those are the runner's, not the agent's.
    fn own_fds() -> BTreeMap<String, PathBuf> {
        let mut fds = BTreeMap::new();
        if let Ok(entries) = std::fs::read_dir("/proc/self/fd") {
            for fd in entries.flatten() {
                if let Ok(target) = std::fs::read_link(fd.path()) {
                    fds.insert(fd.file_name().to_string_lossy().into_owned(), target);
                }
            }
        }
        fds
    }

    fn sample(root: i32, inherited: &BTreeMap<String, PathBuf>, observed: &mut Observed) {
        let members = members(root);
        let mut inodes = BTreeSet::new();
        for (pid, start) in &members {
            observed.processes.entry(*pid).or_insert(*start);
            let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
                continue;
            };
            for fd in fds.flatten() {
                let Ok(target) = std::fs::read_link(fd.path()) else {
                    continue;
                };
                let number = fd.file_name().to_string_lossy().into_owned();
                if inherited.get(&number) == Some(&target) {
                    continue;
                }
                let text = target.to_string_lossy();
                if let Some(inode) = text
                    .strip_prefix("socket:[")
                    .and_then(|t| t.strip_suffix(']'))
                {
                    inodes.insert(inode.to_owned());
                    continue;
                }
                if !text.starts_with('/')
                    || ["/dev/", "/proc/", "/sys/"]
                        .iter()
                        .any(|p| text.starts_with(p))
                    || !target.is_file()
                {
                    continue;
                }
                let flags = std::fs::read_to_string(format!(
                    "/proc/{pid}/fdinfo/{}",
                    fd.file_name().to_string_lossy()
                ))
                .ok()
                .and_then(|info| {
                    info.lines()
                        .find_map(|l| l.strip_prefix("flags:"))
                        .and_then(|f| u32::from_str_radix(f.trim(), 8).ok())
                })
                .unwrap_or(0);
                // O_ACCMODE: 0 read only, 1 write only, 2 read and write.
                if flags & 3 != 0 {
                    observed.writes.insert(target);
                } else {
                    observed.reads.insert(target);
                }
            }
        }
        if inodes.is_empty() {
            return;
        }
        // Every member shares the root's network namespace.
        let Some(pid) = members.keys().next() else {
            return;
        };
        for table in ["tcp", "tcp6"] {
            let Ok(text) = std::fs::read_to_string(format!("/proc/{pid}/net/{table}")) else {
                continue;
            };
            for (inode, peer) in parse_tcp_table(&text) {
                if inodes.contains(&inode) {
                    observed.remote_peers.insert(peer);
                }
            }
        }
    }

    /// `(inode, "ip:port")` for every socket in a `/proc/net/tcp{,6}` table
    /// with a peer that is not loopback or unspecified.
    pub(in super::super) fn parse_tcp_table(text: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for line in text.lines().skip(1) {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let (Some(remote), Some(inode)) = (fields.get(2), fields.get(9)) else {
                continue;
            };
            let Some((addr, port)) = remote.split_once(':') else {
                continue;
            };
            let Ok(port) = u16::from_str_radix(port, 16) else {
                continue;
            };
            let Some(ip) = parse_hex_ip(addr) else {
                continue;
            };
            let loopback = match ip {
                IpAddr::V4(v4) => v4.is_loopback() || v4.is_unspecified(),
                IpAddr::V6(v6) => {
                    v6.is_loopback()
                        || v6.is_unspecified()
                        || v6
                            .to_ipv4_mapped()
                            .is_some_and(|v4| v4.is_loopback() || v4.is_unspecified())
                }
            };
            if loopback {
                continue;
            }
            let peer = match ip {
                IpAddr::V4(v4) => format!("{v4}:{port}"),
                IpAddr::V6(v6) => format!("[{v6}]:{port}"),
            };
            out.push(((*inode).to_owned(), peer));
        }
        out
    }

    /// The kernel prints each 32-bit word in host (little-endian) order.
    fn parse_hex_ip(hex: &str) -> Option<IpAddr> {
        let word = |s: &str| u32::from_str_radix(s, 16).ok().map(u32::to_le_bytes);
        match hex.len() {
            8 => Some(IpAddr::V4(Ipv4Addr::from(word(hex)?))),
            32 => {
                let mut bytes = [0u8; 16];
                for i in 0..4 {
                    bytes[i * 4..i * 4 + 4].copy_from_slice(&word(hex.get(i * 8..i * 8 + 8)?)?);
                }
                Some(IpAddr::V6(Ipv6Addr::from(bytes)))
            }
            _ => None,
        }
    }

    /// Whether `path` is inside `root`.
    pub(in super::super) fn under(path: &Path, root: &Path) -> bool {
        path.starts_with(root)
    }

    #[cfg(test)]
    #[allow(clippy::unwrap_used)]
    mod tests {
        use super::*;

        const TCP: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 111 1 0000000000000000 100 0 0 10 0
   1: 0100007F:A1B2 0100007F:1F90 01 00000000:00000000 00:00000000 00000000  1000        0 222 1 0000000000000000 20 4 30 10 -1
   2: 0A00020F:C350 010200C0:01BB 02 00000001:00000000 01:00000064 00000002  1000        0 333 1 0000000000000000 200 0 0 10 -1
";

        #[test]
        fn tcp_tables_keep_remote_peers_only() {
            assert_eq!(
                parse_tcp_table(TCP),
                vec![("333".to_owned(), "192.0.2.1:443".to_owned())]
            );
        }

        #[test]
        fn tcp6_loopback_is_ignored() {
            let table = "header
   0: 00000000000000000000000001000000:1F90 00000000000000000000000001000000:A1B2 01 0:0 0:0 0 1000 0 444 1
   1: 0000000000000000FFFF00000100007F:1F90 0000000000000000FFFF00000100007F:A1B2 01 0:0 0:0 0 1000 0 555 1
";
            assert!(parse_tcp_table(table).is_empty());
        }

        #[test]
        fn this_process_is_alive_with_its_start_time() {
            let pid = std::process::id() as i32;
            let (_, _, start) = stat(pid).unwrap();
            assert!(alive(pid, start));
            assert!(!alive(pid, start + 1));
        }
    }
}

#[cfg(target_os = "linux")]
pub(super) use linux::{Observer, alive, under};

/// Records nothing outside Linux.
#[cfg(not(target_os = "linux"))]
#[derive(Debug)]
pub(super) struct Observer;

#[cfg(not(target_os = "linux"))]
impl Observer {
    pub(super) fn start(_root: i32) -> Self {
        Self
    }

    pub(super) fn finish(self) -> Observed {
        Observed::default()
    }
}

#[cfg(not(target_os = "linux"))]
pub(super) fn alive(_pid: i32, _start: u64) -> bool {
    false
}

#[cfg(not(target_os = "linux"))]
pub(super) fn under(path: &std::path::Path, root: &std::path::Path) -> bool {
    path.starts_with(root)
}
