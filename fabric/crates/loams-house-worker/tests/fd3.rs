//! The worker adopts fd 3 only when it is an inherited socket (HS1 Task 2 review
//! I4): `loams_chdb_sys::inherited::take_worker_socket` checks close-on-exec and
//! `S_ISSOCK`, and the worker exits with `EX_OSERR` (71) otherwise.

mod common;

use std::ffi::CString;
use std::os::fd::AsRawFd;

use nix::spawn::{PosixSpawnAttr, PosixSpawnFileActions, posix_spawn};
use nix::sys::wait::{WaitStatus, waitpid};

const EXIT_NO_SOCKET: i32 = 71;

fn run_worker(fd3: Option<&std::fs::File>, name: &str) -> WaitStatus {
    let dir = common::tmp_root(name);
    let mut actions = PosixSpawnFileActions::init().expect("actions");
    match fd3 {
        Some(file) => actions.add_dup2(file.as_raw_fd(), 3).expect("dup2"),
        None => actions.add_close(3).expect("close"),
    }
    let attr = PosixSpawnAttr::init().expect("attr");
    let argv: Vec<CString> = [
        common::WORKER.to_string(),
        "--tmp-dir".to_string(),
        dir.display().to_string(),
        "--worker-id".to_string(),
        name.to_string(),
    ]
    .into_iter()
    .map(|a| CString::new(a).expect("argument"))
    .collect();
    let envp: Vec<CString> = Vec::new();
    let pid = posix_spawn(common::WORKER, &actions, &attr, &argv, &envp).expect("spawn");
    waitpid(pid, None).expect("wait")
}

#[test]
fn fd3_must_be_an_inherited_socket() {
    let file_path = common::tmp_root("fd3-file").join("not-a-socket");
    std::fs::write(&file_path, b"x").expect("file");
    let file = std::fs::File::open(&file_path).expect("open");
    assert!(
        matches!(
            run_worker(Some(&file), "fd3-file"),
            WaitStatus::Exited(_, EXIT_NO_SOCKET)
        ),
        "a regular file on fd 3 is refused"
    );
    assert!(
        matches!(
            run_worker(None, "fd3-none"),
            WaitStatus::Exited(_, EXIT_NO_SOCKET)
        ),
        "no fd 3 is refused"
    );
}

#[test]
fn the_two_fd_constants_agree() {
    assert_eq!(
        loams_chdb_sys::inherited::WORKER_SOCKET_FD,
        loams_house_ipc::WORKER_SOCKET_FD
    );
}
