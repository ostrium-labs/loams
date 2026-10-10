//! The worker process (LV1 plan Task 5): `loams live-worker`, which the
//! host starts with piped stdio. It reads `Load`, creates its QuickJS
//! runtime, applies the sandbox (`sandbox_linux.rs`), evaluates the bundle
//! and answers `Loaded`; then it serves one `Invoke` at a time on its main
//! thread with the same [`Engine`] the in-process slots use, its host
//! calls written to the host as `HostCall` frames.
//!
//! A worker never reuses a runtime that ran out of memory: it answers that
//! call with `retire` set and exits (LV1 row T3-10). After a timeout or a
//! call that left jobs queued it replaces its runtime, as a slot does. It
//! exits when the host closes its stdin.

use std::cell::{Cell, RefCell};
use std::io::{self, StdinLock, StdoutLock};
use std::process::ExitCode;
use std::rc::Rc;
use std::time::Duration;

use loams_kv::Ts;
use loams_live::{LiveError, LiveValue};

use crate::host::HostOp;
use crate::ipc::{
    self, Done, DoneOutcome, FrameError, HostCall, HostMessage, Invoke, Load, Loaded, ProbeDone,
    ProbeKind, ReplyAnswer, ToHost, ToWorker,
};
use crate::runtime::{
    Call, Engine, Failure, HostAnswer, HostLink, JsConfig, Outcome, Prepared, check_exports,
};

/// The worker's stdio, shared by its loop and its host link.
struct Stdio {
    input: RefCell<StdinLock<'static>>,
    output: RefCell<StdoutLock<'static>>,
}

impl Stdio {
    fn read(&self) -> Result<HostMessage, FrameError> {
        ipc::read_frame(&mut *self.input.borrow_mut())
    }

    fn send(&self, message: ToHost) -> io::Result<()> {
        ipc::write_frame(&mut *self.output.borrow_mut(), &ipc::to_host(message))
    }
}

/// A running call's link to the host: each `ctx.db` operation is a
/// `HostCall` frame, answered by the `HostReply` with its id.
struct StdioLink {
    io: Rc<Stdio>,
    next: Cell<u64>,
}

impl HostLink for StdioLink {
    fn call(&self, op: HostOp, args: LiveValue) -> Option<HostAnswer> {
        let id = self.next.get();
        self.next.set(id.wrapping_add(1));
        self.io
            .send(ToHost::HostCall(Box::new(HostCall {
                id,
                op: op.name().to_string(),
                args: buffa::MessageField::some(args.to_proto()),
                ..Default::default()
            })))
            .ok()?;
        // Anything but this call's reply means the host has gone wrong;
        // the call aborts, and the next read ends the worker.
        let Some(ToWorker::HostReply(reply)) = self.io.read().ok()?.message else {
            return None;
        };
        if reply.id != id {
            return None;
        }
        match reply.answer? {
            ReplyAnswer::Ok(value) => LiveValue::from_proto(*value).ok().map(HostAnswer::Ok),
            ReplyAnswer::Error(e) => Some(HostAnswer::Error {
                message: e.message,
                index: e.index as usize,
            }),
            ReplyAnswer::Abort(_) => Some(HostAnswer::Abort),
        }
    }

    fn gone(&self) -> bool {
        // A host that has gone closes the pipes; the next read sees it.
        false
    }
}

/// The worker's entry point: `loams live-worker` (hidden) and the
/// `loams-live-worker` binary call it, with nothing else running in the
/// process. Exits non-zero only when it cannot serve at all.
pub fn worker_main() -> ExitCode {
    match serve() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("loams live-worker: {e}");
            ExitCode::FAILURE
        }
    }
}

fn serve() -> Result<(), String> {
    // A hash map's keys come from the OS on first use, which the sandbox
    // forbids: take them now.
    let _ = std::collections::hash_map::RandomState::new();
    let io = Rc::new(Stdio {
        input: RefCell::new(io::stdin().lock()),
        output: RefCell::new(io::stdout().lock()),
    });
    let load = match io.read() {
        Ok(HostMessage {
            message: Some(ToWorker::Load(load)),
            ..
        }) => *load,
        Err(FrameError::Closed) => return Ok(()),
        Err(e) => return Err(format!("the host {e}")),
        Ok(_) => return Err("the host's first frame is not Load".into()),
    };
    let (config, source) = match loaded_config(load) {
        Ok(c) => c,
        Err(e) => return refuse_load(&io, &e),
    };
    let mut engine = match Engine::new(&config) {
        Ok(e) => Some(e),
        Err(e) => return refuse_load(&io, &e),
    };
    if let Err(e) = sandbox(&config) {
        let error = LiveError::Internal(format!("the worker sandbox: {e}"));
        refuse_load(&io, &error)?;
        return Err(e);
    }
    let mut ready: Option<Result<Prepared, LiveError>> = engine.as_ref().map(|e| {
        e.prepare(&source)
            .and_then(|p| check_exports(p.metas.len()).map(|()| p))
    });
    match &ready {
        Some(Ok(prepared)) => io
            .send(ToHost::Loaded(Box::new(Loaded {
                functions: prepared.metas.iter().map(ipc::meta_to_wire).collect(),
                ..Default::default()
            })))
            .map_err(|e| format!("answering Load: {e}"))?,
        Some(Err(e)) => return refuse_load(&io, e),
        None => return Err("no runtime".into()),
    }
    loop {
        if engine.is_none() {
            ready = None;
            engine = match Engine::new(&config) {
                Ok(e) => Some(e),
                Err(e) => {
                    ready = Some(Err(e));
                    None
                }
            };
        }
        if ready.is_none()
            && let Some(engine) = &engine
        {
            ready = Some(engine.prepare(&source));
        }
        let message = match io.read() {
            Ok(m) => m.message,
            Err(FrameError::Closed) => return Ok(()),
            Err(e) => return Err(format!("the host {e}")),
        };
        match message {
            Some(ToWorker::Invoke(invoke)) => {
                let (outcome, poisoned, retire) = match (ready.take(), &engine) {
                    (Some(Ok(prepared)), Some(e)) => {
                        let call = call_of(*invoke)?;
                        let link = Rc::new(StdioLink {
                            io: io.clone(),
                            next: Cell::new(0),
                        });
                        let (outcome, poisoned) = e.run(prepared, call, link);
                        (outcome, poisoned, e.out_of_memory_seen())
                    }
                    (Some(Err(e)), _) => (Outcome::failed(e), true, false),
                    _ => (
                        Outcome::failed(LiveError::Internal(
                            "no JavaScript context is ready".into(),
                        )),
                        true,
                        false,
                    ),
                };
                if poisoned {
                    engine = None;
                }
                io.send(ToHost::Done(Box::new(done(&outcome, retire))))
                    .map_err(|e| format!("answering Invoke: {e}"))?;
                if retire {
                    return Ok(());
                }
            }
            Some(ToWorker::Probe(probe)) => {
                let error = run_probe(probe.kind.as_known());
                io.send(ToHost::ProbeDone(Box::new(ProbeDone {
                    error,
                    ..Default::default()
                })))
                .map_err(|e| format!("answering Probe: {e}"))?;
            }
            _ => return Err("the host sent a frame out of turn".into()),
        }
    }
}

/// The runtime configuration and source of a `Load`.
fn loaded_config(load: Load) -> Result<(JsConfig, String), LiveError> {
    let config = JsConfig {
        memory_limit: usize::try_from(load.memory_limit).unwrap_or(usize::MAX),
        cpu_limit: Duration::from_nanos(load.cpu_limit_ns),
        contexts: 1,
        console_lines: load.console_lines as usize,
        console_line_bytes: load.console_line_bytes as usize,
        isolation: loams_live::Isolation::Isolated,
    };
    let source = String::from_utf8(load.bundle)
        .map_err(|_| LiveError::InvalidArgument("the bundle is not UTF-8".into()))?;
    Ok((config, source))
}

/// Answers `Load` with `error`; the worker then exits.
fn refuse_load(io: &Stdio, error: &LiveError) -> Result<(), String> {
    io.send(ToHost::Loaded(Box::new(Loaded {
        error: buffa::MessageField::some(ipc::failure_to_wire(&Failure::Live(error.clone()))),
        ..Default::default()
    })))
    .map_err(|e| format!("answering Load: {e}"))
}

#[cfg(target_os = "linux")]
fn sandbox(config: &JsConfig) -> Result<(), String> {
    crate::sandbox_linux::apply(config.memory_limit)
}

#[cfg(not(target_os = "linux"))]
fn sandbox(_config: &JsConfig) -> Result<(), String> {
    Err("isolated workers need Linux (seccomp and landlock)".into())
}

/// The call an `Invoke` asks for.
fn call_of(invoke: Invoke) -> Result<Call, String> {
    let ctx = invoke.ctx.into_option().unwrap_or_default();
    let args = invoke
        .args
        .into_option()
        .map(LiveValue::from_proto)
        .transpose()
        .map_err(|e| format!("the host sent bad arguments: {e}"))?
        .unwrap_or(LiveValue::Null);
    Ok(Call::new(
        invoke.path,
        args,
        Ts(ctx.start_ts),
        &ctx.request_id,
        usize::try_from(ctx.result_bytes).unwrap_or(usize::MAX),
        usize::try_from(ctx.host_bytes).unwrap_or(usize::MAX),
    ))
}

/// The `Done` frame of an outcome.
fn done(outcome: &Outcome, retire: bool) -> Done {
    let result = match &outcome.result {
        Ok(value) => DoneOutcome::Result(Box::new(value.to_proto())),
        Err(failure) => DoneOutcome::Error(Box::new(ipc::failure_to_wire(failure))),
    };
    Done {
        outcome: Some(result),
        logs: ipc::logs_to_wire(&outcome.output),
        dropped: outcome.output.dropped,
        cpu_ns: u64::try_from(outcome.cpu.as_nanos()).unwrap_or(u64::MAX),
        retire,
        ..Default::default()
    }
}

/// Makes the system call a test probe names. Inside the sandbox every one
/// but `Ping` kills the process with `SIGSYS`; outside it, the call's own
/// error, if any, is the answer.
fn run_probe(kind: Option<ProbeKind>) -> String {
    let result = match kind {
        Some(ProbeKind::PROBE_KIND_OPEN_FILE) => std::fs::File::open("/etc/passwd").map(drop),
        Some(ProbeKind::PROBE_KIND_OPEN_SOCKET) => {
            std::net::TcpStream::connect(("127.0.0.1", 9)).map(drop)
        }
        Some(ProbeKind::PROBE_KIND_EXEC) => {
            std::process::Command::new("/bin/true").status().map(drop)
        }
        _ => Ok(()),
    };
    result.err().map(|e| e.to_string()).unwrap_or_default()
}
