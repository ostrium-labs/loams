//! The worker's own user and network namespaces (L3, §49 §13.2; HS1 R1.11).
//!
//! An unprivileged process may make a user namespace, and inside it a network
//! namespace, whose only interface is a loopback that starts down. The worker
//! maps its own uid and gid onto themselves (so the files it writes in its
//! private directory stay the front's user's, and nothing runs as root inside),
//! raises `lo` (HS1 R1.11: chDB's users file accepts `127.0.0.1` only, and connect
//! fails while `lo` is down), and from then on reaches nothing but its own
//! loopback: the forwarder listens there, and nothing else does.
//!
//! `unshare(CLONE_NEWUSER)` refuses a process with more than one thread, so this
//! runs first, before anything starts a thread (libchdb's constructors start
//! none, measured).

use std::io::{self, Read};
use std::os::fd::AsRawFd;

use nix::sched::{CloneFlags, unshare};
use nix::sys::socket::{
    AddressFamily, MsgFlags, NetlinkAddr, SockFlag, SockProtocol, SockType, bind, recv, send,
    socket,
};
use nix::unistd::{getgid, getuid};

use super::SandboxError;

/// Enters a new user namespace and a new network namespace, mapping the
/// worker's uid and gid onto themselves.
pub fn enter() -> Result<(), SandboxError> {
    let uid = getuid();
    let gid = getgid();
    unshare(CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNET)
        .map_err(|err| SandboxError::new("unshare(CLONE_NEWUSER | CLONE_NEWNET)", err))?;
    // An unprivileged process may write one line mapping its own ids, and only
    // after giving up `setgroups` (user_namespaces(7)).
    write_proc("/proc/self/setgroups", "deny")?;
    write_proc("/proc/self/uid_map", &format!("{uid} {uid} 1"))?;
    write_proc("/proc/self/gid_map", &format!("{gid} {gid} 1"))?;
    Ok(())
}

fn write_proc(path: &str, line: &str) -> Result<(), SandboxError> {
    std::fs::write(path, line).map_err(|err| SandboxError::new(path, err))
}

/// The loopback's index: 1 in every network namespace (`LOOPBACK_IFINDEX`).
const LOOPBACK_INDEX: i32 = 1;

/// `RTM_NEWLINK`.
const RTM_NEWLINK: u16 = 16;
/// `NLM_F_REQUEST | NLM_F_ACK`.
const REQUEST_WITH_ACK: u16 = 0x1 | 0x4;
/// `NLMSG_ERROR`, which also carries the acknowledgement (error 0).
const NLMSG_ERROR: u16 = 2;
/// `IFF_UP`.
const IFF_UP: u32 = 0x1;

/// Raises `lo` in the current network namespace over rtnetlink: one
/// `RTM_NEWLINK` that sets `IFF_UP` on index 1, and its acknowledgement.
pub fn raise_loopback() -> Result<(), SandboxError> {
    let step = "raise lo";
    let fd = socket(
        AddressFamily::Netlink,
        SockType::Raw,
        SockFlag::SOCK_CLOEXEC,
        SockProtocol::NetlinkRoute,
    )
    .map_err(|err| SandboxError::new(step, err))?;
    bind(fd.as_raw_fd(), &NetlinkAddr::new(0, 0)).map_err(|err| SandboxError::new(step, err))?;
    let request = new_link_up(LOOPBACK_INDEX);
    send(fd.as_raw_fd(), &request, MsgFlags::empty())
        .map_err(|err| SandboxError::new(step, err))?;
    let mut answer = [0u8; 512];
    let n = recv(fd.as_raw_fd(), &mut answer, MsgFlags::empty())
        .map_err(|err| SandboxError::new(step, err))?;
    acknowledged(&answer[..n]).map_err(|err| SandboxError::new(step, err))
}

/// `nlmsghdr` (16 bytes) then `ifinfomsg` (16 bytes), native-endian.
fn new_link_up(index: i32) -> Vec<u8> {
    let mut message = Vec::with_capacity(32);
    message.extend_from_slice(&32u32.to_ne_bytes()); // nlmsg_len
    message.extend_from_slice(&RTM_NEWLINK.to_ne_bytes()); // nlmsg_type
    message.extend_from_slice(&REQUEST_WITH_ACK.to_ne_bytes()); // nlmsg_flags
    message.extend_from_slice(&1u32.to_ne_bytes()); // nlmsg_seq
    message.extend_from_slice(&0u32.to_ne_bytes()); // nlmsg_pid: the kernel
    message.push(0); // ifi_family: AF_UNSPEC
    message.push(0); // padding
    message.extend_from_slice(&0u16.to_ne_bytes()); // ifi_type
    message.extend_from_slice(&index.to_ne_bytes()); // ifi_index
    message.extend_from_slice(&IFF_UP.to_ne_bytes()); // ifi_flags
    message.extend_from_slice(&IFF_UP.to_ne_bytes()); // ifi_change
    message
}

/// Reads the kernel's answer: an `NLMSG_ERROR` whose error is 0 is the
/// acknowledgement; any other error is the errno it names.
fn acknowledged(answer: &[u8]) -> io::Result<()> {
    let mut cursor = answer;
    let mut header = [0u8; 16];
    cursor.read_exact(&mut header)?;
    let kind = u16::from_ne_bytes([header[4], header[5]]);
    if kind != NLMSG_ERROR {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("rtnetlink answered message type {kind}, not an acknowledgement"),
        ));
    }
    let mut error = [0u8; 4];
    cursor.read_exact(&mut error)?;
    match i32::from_ne_bytes(error) {
        0 => Ok(()),
        negative => Err(io::Error::from_raw_os_error(-negative)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_is_one_new_link_with_up() {
        let message = new_link_up(1);
        assert_eq!(message.len(), 32);
        assert_eq!(
            u32::from_ne_bytes(message[0..4].try_into().expect("4 bytes")),
            32
        );
        assert_eq!(u16::from_ne_bytes([message[4], message[5]]), RTM_NEWLINK);
        assert_eq!(
            i32::from_ne_bytes(message[20..24].try_into().expect("4 bytes")),
            1
        );
        assert_eq!(
            u32::from_ne_bytes(message[24..28].try_into().expect("4 bytes")),
            IFF_UP
        );
        assert_eq!(
            u32::from_ne_bytes(message[28..32].try_into().expect("4 bytes")),
            IFF_UP
        );
    }

    #[test]
    fn an_error_answer_is_its_errno() {
        let mut answer = vec![0u8; 36];
        answer[4..6].copy_from_slice(&NLMSG_ERROR.to_ne_bytes());
        assert!(acknowledged(&answer).is_ok());
        answer[16..20].copy_from_slice(&(-1i32).to_ne_bytes());
        let err = acknowledged(&answer).expect_err("EPERM");
        assert_eq!(err.raw_os_error(), Some(1));
        answer[4..6].copy_from_slice(&16u16.to_ne_bytes());
        assert!(acknowledged(&answer).is_err());
        assert!(acknowledged(&answer[..8]).is_err());
    }
}
