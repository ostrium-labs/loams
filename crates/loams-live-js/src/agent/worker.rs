//! The agent worker process (SPIKE): `loams-agent-worker worker`, which
//! Task 2 makes `loams-agentd agent-worker`. The host starts it with piped
//! stdio and an empty environment.
//!
//! It reads `AgentLoad`, creates its QuickJS runtime, seals itself with
//! the function worker's sandbox (`sandbox_linux.rs`, unchanged), evaluates
//! the bundle and answers `AgentLoaded`. Then it is an event loop on its
//! one thread: it runs the job queue dry, writes `AgentIdle`, and blocks
//! reading the next frame. Everything the agent awaits arrives as a frame,
//! so promises resolved from frames are enough: rquickjs's `futures`
//! feature is not needed.
//!
//! Limits: the allocator's memory limit and a CPU *slice*: one dispatch
//! (a frame and the jobs it unblocks) may run at most `cpu_slice_ns` of
//! interpreter time, checked by the interrupt handler. A worker stopped by
//! either writes `AgentStopped` and exits. QuickJS C built-ins that do not
//! poll the interrupt handler are stopped from outside: the host kills a
//! worker that has not written `AgentIdle` within the slice plus a grace.

use std::cell::{Cell, RefCell};
use std::io::{self, StdinLock, StdoutLock};
use std::process::ExitCode;
use std::rc::Rc;
use std::time::{Duration, Instant};

use loams_live_js_alloc::MemoryMeter;
use loams_live_proto::loams::live::worker::v1 as wire;
use rquickjs::context::intrinsic;
use rquickjs::loader::{BuiltinLoader, BuiltinResolver};
use rquickjs::{ArrayBuffer, Context, Ctx, Function, Module, Object, Persistent, Runtime};

use super::globals::Entropy;
use crate::ipc::{self, FrameError, ProbeKind, ToHost, ToWorker};

const PRELUDE: &str = include_str!("prelude.js");
const AGENT_MODULE: &str = "loams:agent";
const AGENT_GLOBAL: &str = "__loams_agent__";
const AGENT_SOURCE: &str = "const a = globalThis.__loams_agent__;\n\
     export const acp = a.acp;\n\
     export const model = a.model;\n\
     export const mcp = a.mcp;\n\
     export const loams = a.loams;\n";
const JS_STACK_BYTES: usize = 1024 * 1024;
const MEMORY_GRACE_BYTES: usize = 64 * 1024;
/// The largest `BodyChunk` either side sends.
pub(crate) const MAX_CHUNK_BYTES: usize = 1 << 20;

struct Limits {
    slice: Duration,
    since: Cell<Option<Instant>>,
    memory: MemoryMeter,
    stopped: Cell<Option<&'static str>>,
}

impl Limits {
    fn interrupt(&self) -> bool {
        if self.stopped.get().is_some() {
            return true;
        }
        if self.memory.exceeded() {
            self.memory.top_up_grace(MEMORY_GRACE_BYTES);
            self.stopped.set(Some("memory"));
            return true;
        }
        if self.since.get().is_some_and(|s| s.elapsed() > self.slice) {
            self.stopped.set(Some("cpu"));
            return true;
        }
        false
    }
}

/// The worker's stdout, shared by its loop and its natives.
struct Out(RefCell<StdoutLock<'static>>);

impl Out {
    fn send(&self, message: ToHost) -> io::Result<()> {
        ipc::write_frame(&mut *self.0.borrow_mut(), &ipc::to_host(message))
    }
}

fn io_error(e: io::Error) -> rquickjs::Error {
    rquickjs::Error::Io(e)
}

/// The agent worker's entry point.
pub fn agent_worker_main() -> ExitCode {
    match serve() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("loams agent-worker: {e}");
            ExitCode::FAILURE
        }
    }
}

fn read(input: &mut StdinLock<'static>) -> Result<Option<ToWorker>, String> {
    match ipc::read_frame::<wire::HostMessage>(input) {
        Ok(m) => Ok(m.message),
        Err(FrameError::Closed) => Ok(None),
        Err(e) => Err(format!("the host {e}")),
    }
}

fn loaded(out: &Out, name: String, version: String, error: String) -> Result<(), String> {
    out.send(ToHost::AgentLoaded(Box::new(wire::AgentLoaded {
        name,
        version,
        error,
        ..Default::default()
    })))
    .map_err(|e| format!("answering AgentLoad: {e}"))
}

fn serve() -> Result<(), String> {
    // A hash map's keys come from the OS on first use, which the seal
    // forbids: take them now (as Live's worker does).
    let _ = std::collections::hash_map::RandomState::new();
    let mut input = io::stdin().lock();
    let out = Rc::new(Out(RefCell::new(io::stdout().lock())));
    let load = match read(&mut input)? {
        Some(ToWorker::AgentLoad(load)) => *load,
        None => return Ok(()),
        Some(_) => return Err("the host's first frame is not AgentLoad".into()),
    };
    let memory_limit = usize::try_from(load.memory_limit).unwrap_or(usize::MAX);
    let slice = Duration::from_nanos(load.cpu_slice_ns);
    let source = String::from_utf8(load.bundle).map_err(|_| "the bundle is not UTF-8")?;
    let entropy = Rc::new(Entropy::new(&load.entropy));

    let memory = MemoryMeter::new(memory_limit);
    let rt = Runtime::new_with_alloc(memory.allocator()).map_err(|e| format!("runtime: {e}"))?;
    rt.set_max_stack_size(JS_STACK_BYTES);
    let limits = Rc::new(Limits {
        slice,
        since: Cell::new(None),
        memory,
        stopped: Cell::new(None),
    });
    let handler = limits.clone();
    rt.set_interrupt_handler(Some(Box::new(move || handler.interrupt())));
    rt.set_loader(
        BuiltinResolver::default().with_module(AGENT_MODULE),
        BuiltinLoader::default(),
    );
    // `Eval` is needed to compile modules; the prelude removes `eval` and
    // the Function constructors before the bundle runs.
    let ctx = Context::custom::<(
        intrinsic::Date,
        intrinsic::Eval,
        intrinsic::RegExpCompiler,
        intrinsic::RegExp,
        intrinsic::Json,
        intrinsic::Proxy,
        intrinsic::MapSet,
        intrinsic::TypedArrays,
        intrinsic::Promise,
        intrinsic::Performance,
        intrinsic::WeakRef,
    )>(&rt)
    .map_err(|e| format!("context: {e}"))?;

    if let Err(e) = seal(memory_limit) {
        loaded(
            &out,
            String::new(),
            String::new(),
            format!("the worker sandbox: {e}"),
        )?;
        return Err(e);
    }

    limits.since.set(Some(Instant::now()));
    let prepared = ctx.with(
        |ctx| -> Result<(Persistent<Object<'static>>, String, String), String> {
            let js = |e: rquickjs::Error| describe(&ctx, e);
            let setup: Function = ctx.eval(PRELUDE).map_err(js)?;
            let natives = natives(&ctx, &out, &entropy).map_err(js)?;
            let config = Object::new(ctx.clone()).map_err(js)?;
            for (k, v) in &load.config {
                config.set(k.as_str(), v.as_str()).map_err(js)?;
            }
            let internals: Object = setup.call((natives, config)).map_err(js)?;
            let globals = ctx.globals();
            globals
                .set(
                    AGENT_GLOBAL,
                    internals.get::<_, Object>("agent").map_err(js)?,
                )
                .map_err(js)?;
            let (_, done) = Module::declare(ctx.clone(), AGENT_MODULE, AGENT_SOURCE)
                .and_then(|m| m.eval())
                .map_err(js)?;
            done.finish::<()>().map_err(js)?;
            globals.remove(AGENT_GLOBAL).map_err(js)?;
            let (module, done) = Module::declare(ctx.clone(), "agent", source)
                .and_then(|m| m.eval())
                .map_err(js)?;
            done.finish::<()>().map_err(js)?;
            while limits.stopped.get().is_none() && ctx.execute_pending_job() {}
            let bind: Function = internals.get("bind").map_err(js)?;
            let info: Object = bind.call((module.namespace().map_err(js)?,)).map_err(js)?;
            let name: String = info.get("name").map_err(js)?;
            let version: String = info.get("version").map_err(js)?;
            Ok((Persistent::save(&ctx, internals), name, version))
        },
    );
    limits.since.set(None);
    let internals = match prepared {
        Ok((internals, name, version)) if limits.stopped.get().is_none() => {
            loaded(&out, name, version, String::new())?;
            internals
        }
        Ok(_) => {
            let reason = limits.stopped.get().unwrap_or("a limit");
            return loaded(
                &out,
                String::new(),
                String::new(),
                format!("stopped ({reason}) while loading"),
            );
        }
        Err(e) => return loaded(&out, String::new(), String::new(), e),
    };

    loop {
        out.send(ToHost::Idle(Box::default()))
            .map_err(|e| format!("writing: {e}"))?;
        let Some(message) = read(&mut input)? else {
            return Ok(());
        };
        if let ToWorker::Probe(probe) = &message {
            let error = run_probe(probe.kind.as_known());
            out.send(ToHost::ProbeDone(Box::new(wire::ProbeDone {
                error,
                ..Default::default()
            })))
            .map_err(|e| format!("writing: {e}"))?;
            continue;
        }
        if let ToWorker::Entropy(e) = &message {
            entropy.reseed(&e.data);
            continue;
        }
        limits.since.set(Some(Instant::now()));
        let result = ctx.with(|ctx| {
            let r = internals
                .clone()
                .restore(&ctx)
                .and_then(|internals| dispatch(&ctx, &internals, message));
            if r.is_err() {
                let _ = ctx.catch();
            }
            while limits.stopped.get().is_none() && ctx.execute_pending_job() {}
            r
        });
        // An allocation refused past the limit raises a catchable error
        // when the interrupt handler has not run since: the flag decides.
        if limits.memory.exceeded() && limits.stopped.get().is_none() {
            limits.stopped.set(Some("memory"));
        }
        limits.since.set(None);
        if let Some(reason) = limits.stopped.get() {
            let _ = out.send(ToHost::Stopped(Box::new(wire::AgentStopped {
                reason: reason.to_string(),
                ..Default::default()
            })));
            return Err(format!("stopped: over its {reason} limit"));
        }
        if let Err(e) = result {
            log(&out, "error", &format!("dispatch: {e}"));
        }
    }
}

/// Hands one frame to the prelude.
fn dispatch<'js>(
    ctx: &Ctx<'js>,
    internals: &Object<'js>,
    message: ToWorker,
) -> rquickjs::Result<()> {
    match message {
        ToWorker::Acp(m) => internals.get::<_, Function>("onAcp")?.call((m.json,)),
        ToWorker::AgentReply(reply) => {
            use wire::__buffa::oneof::agent_reply::Answer;
            let (kind, payload) = match reply.answer {
                Some(Answer::Error(e)) => ("error", e),
                Some(Answer::Json(j)) => ("json", j),
                Some(Answer::Fetch(head)) => {
                    let headers: Vec<(String, String)> = head
                        .headers
                        .iter()
                        .map(|h| (h.name.clone(), h.value.clone()))
                        .collect();
                    let head = serde_json::json!({
                        "status": head.status,
                        "statusText": head.status_text,
                        "headers": headers,
                        "url": head.url,
                    });
                    ("fetch", head.to_string())
                }
                None => ("error", "an empty reply".to_string()),
            };
            internals
                .get::<_, Function>("onReply")?
                .call((reply.id as f64, kind, payload))
        }
        ToWorker::BodyChunk(chunk) => {
            let data = ArrayBuffer::new(ctx.clone(), chunk.data)?;
            internals
                .get::<_, Function>("onChunk")?
                .call((chunk.id as f64, data, chunk.last))
        }
        ToWorker::TimerFired(t) => internals
            .get::<_, Function>("onTimer")?
            .call((t.id as f64,)),
        _ => Ok(()),
    }
}

fn log(out: &Out, level: &str, line: &str) {
    let level = match level {
        "debug" => wire::LogLevel::LOG_LEVEL_DEBUG,
        "info" => wire::LogLevel::LOG_LEVEL_INFO,
        "warn" => wire::LogLevel::LOG_LEVEL_WARN,
        "error" => wire::LogLevel::LOG_LEVEL_ERROR,
        _ => wire::LogLevel::LOG_LEVEL_LOG,
    };
    let _ = out.send(ToHost::Log(Box::new(wire::AgentLog {
        level: level.into(),
        line: line.to_string(),
        ..Default::default()
    })));
}

fn call(out: &Out, id: f64, kind: wire::__buffa::oneof::agent_call::Kind) -> rquickjs::Result<()> {
    out.send(ToHost::AgentCall(Box::new(wire::AgentCall {
        id: id as u64,
        kind: Some(kind),
        ..Default::default()
    })))
    .map_err(io_error)
}

/// The host functions the prelude receives.
fn natives<'js>(
    ctx: &Ctx<'js>,
    out: &Rc<Out>,
    entropy: &Rc<Entropy>,
) -> rquickjs::Result<Object<'js>> {
    use wire::__buffa::oneof::agent_call::Kind;
    let n = Object::new(ctx.clone())?;
    let o = out.clone();
    n.set(
        "acp",
        Function::new(ctx.clone(), move |json: String| {
            o.send(ToHost::Acp(Box::new(wire::AcpMessage {
                json,
                ..Default::default()
            })))
            .map_err(io_error)
        })?,
    )?;
    let o = out.clone();
    n.set(
        "fetch",
        Function::new(
            ctx.clone(),
            move |id: f64, url: String, method: String, headers: String, body_follows: bool| {
                let pairs: Vec<(String, String)> =
                    serde_json::from_str(&headers).unwrap_or_default();
                let headers = pairs
                    .into_iter()
                    .map(|(name, value)| wire::Header {
                        name,
                        value,
                        ..Default::default()
                    })
                    .collect();
                call(
                    &o,
                    id,
                    Kind::Fetch(Box::new(wire::FetchCall {
                        url,
                        method,
                        headers,
                        body_follows,
                        ..Default::default()
                    })),
                )
            },
        )?,
    )?;
    let o = out.clone();
    n.set(
        "bodyChunk",
        Function::new(ctx.clone(), move |id: f64, latin1: String, last: bool| {
            // One byte per char (the prelude's `latin1`).
            let data: Vec<u8> = latin1.chars().map(|c| u32::from(c) as u8).collect();
            o.send(ToHost::BodyChunk(Box::new(wire::BodyChunk {
                id: id as u64,
                data,
                last,
                ..Default::default()
            })))
            .map_err(io_error)
        })?,
    )?;
    let o = out.clone();
    n.set(
        "model",
        Function::new(
            ctx.clone(),
            move |id: f64, api: String, json: String, stream: bool| {
                call(
                    &o,
                    id,
                    Kind::Model(Box::new(wire::ModelCall {
                        api,
                        json,
                        stream,
                        ..Default::default()
                    })),
                )
            },
        )?,
    )?;
    let o = out.clone();
    n.set(
        "mcp",
        Function::new(
            ctx.clone(),
            move |id: f64, method: String, server: String, tool: String, json: String| {
                call(
                    &o,
                    id,
                    Kind::Mcp(Box::new(wire::McpCall {
                        method,
                        server,
                        tool,
                        json,
                        ..Default::default()
                    })),
                )
            },
        )?,
    )?;
    let o = out.clone();
    n.set(
        "token",
        Function::new(ctx.clone(), move |id: f64| {
            call(&o, id, Kind::Token(Box::default()))
        })?,
    )?;
    let o = out.clone();
    n.set(
        "timerSet",
        Function::new(ctx.clone(), move |id: f64, ms: f64| {
            o.send(ToHost::TimerSet(Box::new(wire::TimerSet {
                id: id as u64,
                ms: ms.max(0.0) as u64,
                ..Default::default()
            })))
            .map_err(io_error)
        })?,
    )?;
    let o = out.clone();
    n.set(
        "timerClear",
        Function::new(ctx.clone(), move |id: f64| {
            o.send(ToHost::TimerClear(Box::new(wire::TimerClear {
                id: id as u64,
                ..Default::default()
            })))
            .map_err(io_error)
        })?,
    )?;
    let o = out.clone();
    n.set(
        "log",
        Function::new(ctx.clone(), move |level: String, line: String| {
            log(&o, &level, &line)
        })?,
    )?;
    n.set(
        "utf8",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, s: String| {
            ArrayBuffer::new(ctx, s.into_bytes())
        })?,
    )?;
    let e = entropy.clone();
    n.set("random", Function::new(ctx.clone(), move || e.random())?)?;
    let e = entropy.clone();
    let o = out.clone();
    n.set(
        "randomBytes",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, len: usize| {
            let (bytes, refill) = e.bytes(len.min(65536));
            if refill {
                o.send(ToHost::EntropyRequest(Box::default()))
                    .map_err(io_error)?;
            }
            ArrayBuffer::new(ctx, bytes)
        })?,
    )?;
    Ok(n)
}

fn describe(ctx: &Ctx<'_>, e: rquickjs::Error) -> String {
    let thrown = ctx.catch();
    match thrown.as_exception() {
        Some(x) => format!(
            "{}\n{}",
            x.message().unwrap_or_default(),
            x.stack().unwrap_or_default()
        ),
        None if thrown.is_undefined() => e.to_string(),
        None => format!("{e}: {thrown:?}"),
    }
}

#[cfg(target_os = "linux")]
fn seal(memory_limit: usize) -> Result<(), String> {
    crate::sandbox_linux::apply(memory_limit)
}

#[cfg(not(target_os = "linux"))]
fn seal(_memory_limit: usize) -> Result<(), String> {
    Err("agent workers need Linux (seccomp and landlock)".into())
}

/// Makes the system call a test probe names (as Live's worker does). Inside
/// the seal every one but `Ping` kills the process with `SIGSYS`.
fn run_probe(kind: Option<ProbeKind>) -> String {
    let result = match kind {
        Some(ProbeKind::PROBE_KIND_OPEN_FILE) => std::fs::File::open("/etc/passwd").map(drop),
        Some(ProbeKind::PROBE_KIND_OPEN_SOCKET) => {
            std::net::TcpStream::connect(("127.0.0.1", 9)).map(drop)
        }
        Some(ProbeKind::PROBE_KIND_EXEC) => {
            std::process::Command::new("/bin/true").status().map(drop)
        }
        Some(ProbeKind::PROBE_KIND_SPAWN_THREAD) => {
            std::thread::Builder::new().spawn(|| {}).and_then(|h| {
                h.join()
                    .map_err(|_| io::Error::other("the thread panicked"))
            })
        }
        _ => Ok(()),
    };
    result.err().map(|e| e.to_string()).unwrap_or_default()
}
