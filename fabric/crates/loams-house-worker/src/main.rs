//! `loams-house-worker --tmp-dir <dir> --worker-id <id> [--memory-limit <bytes>]
//! [--s3-endpoint <url>]`, with its `hsw1` socket on fd 3. See the library docs.

use std::os::unix::net::UnixStream;
use std::time::Instant;

use loams_chdb_sys::process::exit_now;
use loams_house_ipc::{Frame, FrameCodec};
use loams_house_worker::{Hosting, Worker, WorkerArgs};

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
            exit_now(EXIT_USAGE);
        }
    };
    let socket = match loams_chdb_sys::inherited::take_worker_socket() {
        Ok(fd) => UnixStream::from(fd),
        Err(err) => {
            eprintln!(
                "loams-house-worker {}: no hsw1 socket: {err}",
                args.worker_id
            );
            exit_now(EXIT_NO_SOCKET);
        }
    };
    let mut writer = match socket.try_clone() {
        Ok(writer) => writer,
        Err(err) => {
            eprintln!("loams-house-worker {}: {err}", args.worker_id);
            exit_now(EXIT_NO_SOCKET);
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
            exit_now(EXIT_BOOT);
        }
    };

    let label = args.worker_id.clone();
    worker
        .serve(socket, writer, Hosting::Process(label.clone()))
        .exit_process(&label)
}
