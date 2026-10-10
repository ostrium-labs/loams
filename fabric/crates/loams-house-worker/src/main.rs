//! `loams-house-worker --tmp-dir <dir> --worker-id <id> [--memory-limit <bytes>]
//! [--s3-endpoint <url>] [--sandbox netns|pods|none] [--forwarder-port <p>]
//! [--cgroup <dir>]`, with its `hsw1` socket on fd 3 and, with a forwarder, the
//! forwarder's socket on fd 4. See the library docs.

use std::os::unix::net::UnixStream;
use std::time::Instant;

use loams_chdb_sys::process::exit_now;
use loams_house_ipc::{EngineError, Frame, FrameCodec};
use loams_house_worker::sandbox::{Sandbox, SandboxConfig, SandboxMode};
use loams_house_worker::{Hosting, Worker, WorkerArgs};

/// `EX_USAGE`: bad arguments.
const EXIT_USAGE: i32 = 64;
/// `EX_OSERR`: no socket on fd 3.
const EXIT_NO_SOCKET: i32 = 71;
/// The engine did not boot.
const EXIT_BOOT: i32 = 1;
/// The sandbox could not be put in force (`EX_NOPERM`): the worker never runs
/// unsealed when it was asked to be sealed.
const EXIT_SANDBOX: i32 = 77;

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

    // L3 (HS1 Task 6): sealed before libchdb boots.
    let _sandbox = match seal(&args) {
        Ok(sandbox) => sandbox,
        Err(error) => refuse(&args, &mut writer, error, EXIT_SANDBOX),
    };

    let worker = match Worker::boot(&args, started) {
        Ok(worker) => worker,
        Err(error) => refuse(&args, &mut writer, error, EXIT_BOOT),
    };

    let label = args.worker_id.clone();
    worker
        .serve(socket, writer, Hosting::Process(label.clone()))
        .exit_process(&label)
}

/// Puts the sandbox `args` ask for in force, taking the forwarder's socket (fd 4)
/// when there is one.
fn seal(args: &WorkerArgs) -> Result<Sandbox, EngineError> {
    let fail = |detail: String| EngineError {
        code: 0,
        name: "LOAMS_SANDBOX".to_string(),
        message: detail,
    };
    let forwarder = match args.forwarder_port {
        Some(port) => {
            let socket = loams_chdb_sys::inherited::take_forwarder_socket()
                .map_err(|err| fail(format!("no forwarder socket: {err}")))?;
            Some((port, socket))
        }
        None => None,
    };
    let connect_ports = match args.sandbox {
        SandboxMode::Pods => args.s3_endpoint_port().into_iter().collect(),
        SandboxMode::Netns | SandboxMode::None => Vec::new(),
    };
    Sandbox::apply(SandboxConfig {
        mode: args.sandbox,
        tmp_dir: args.tmp_dir.clone(),
        cgroup: args.cgroup.clone(),
        forwarder,
        connect_ports,
    })
    .map_err(|err| fail(err.to_string()))
}

/// The front is waiting for `Ready`; an `Error` instead says why, and the worker
/// exits.
fn refuse(args: &WorkerArgs, writer: &mut UnixStream, error: EngineError, code: i32) -> ! {
    eprintln!(
        "loams-house-worker {}: boot failed: {error}",
        args.worker_id
    );
    let _ = FrameCodec::write(
        writer,
        &Frame::Error {
            error,
            poisoned: true,
        },
    );
    exit_now(code);
}
