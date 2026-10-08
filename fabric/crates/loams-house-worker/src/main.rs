//! `loams-house-worker --tmp-dir <dir> --worker-id <id> [--memory-limit <bytes>]
//! [--s3-endpoint <url>]`, with its `hsw1` socket on fd 3. See the library docs.

use std::os::unix::net::UnixStream;
use std::time::Instant;

use loams_house_ipc::{EXIT_PROTOCOL, Frame, FrameCodec, WORKER_SOCKET_FD};
use loams_house_worker::{End, Worker, WorkerArgs};

/// `EX_USAGE`: bad arguments.
const EXIT_USAGE: i32 = 64;
/// `EX_OSERR`: no socket on fd 3.
const EXIT_NO_SOCKET: i32 = 71;
/// The engine did not boot.
const EXIT_BOOT: i32 = 1;

fn main() {
    let started = Instant::now();
    let args = match WorkerArgs::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("loams-house-worker: {err}");
            std::process::exit(EXIT_USAGE);
        }
    };
    let socket = match loams_chdb_sys::inherited::take_socket(WORKER_SOCKET_FD) {
        Ok(fd) => UnixStream::from(fd),
        Err(err) => {
            eprintln!(
                "loams-house-worker {}: no hsw1 socket: {err}",
                args.worker_id
            );
            std::process::exit(EXIT_NO_SOCKET);
        }
    };
    let mut writer = match socket.try_clone() {
        Ok(writer) => writer,
        Err(err) => {
            eprintln!("loams-house-worker {}: {err}", args.worker_id);
            std::process::exit(EXIT_NO_SOCKET);
        }
    };

    let worker = match Worker::boot(&args, started) {
        Ok(worker) => worker,
        Err(error) => {
            // The front is waiting for `Ready`; an `Error` instead says why.
            eprintln!(
                "loams-house-worker {}: boot failed: {error}",
                args.worker_id
            );
            let _ = FrameCodec::write(
                &mut writer,
                &Frame::Error {
                    error,
                    poisoned: true,
                },
            );
            std::process::exit(EXIT_BOOT);
        }
    };

    let code = match worker.serve(socket, writer) {
        End::Closed => 0,
        End::Protocol(why) => {
            eprintln!("loams-house-worker {}: hsw1: {why}", args.worker_id);
            EXIT_PROTOCOL
        }
        End::Io(why) => {
            eprintln!("loams-house-worker {}: socket: {why}", args.worker_id);
            1
        }
    };
    std::process::exit(code);
}
