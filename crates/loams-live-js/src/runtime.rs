//! [`Bundle`]: a deployed ES module run in QuickJS (LV1 plan Task 3; R1
//! Task 13 items 1–3; design §20 §6, §45 §3).
//!
//! **Slots.** A bundle owns `contexts` slots. A slot is an OS thread with
//! its own QuickJS runtime (a runtime is single-threaded), its memory limit
//! and its interrupt handler. A slot keeps one context ready: the prelude
//! ran, the globals are frozen and the bundle is evaluated. A call takes
//! the ready context, runs exactly one invocation in it and drops it, on
//! success or failure; the slot then prepares the next one before it takes
//! another call. After a timeout, an out-of-memory error, an aborted call
//! or a call that left promise jobs queued (a floating promise, which
//! would otherwise run in the next call), the slot also replaces its
//! runtime. The jobs a bundle's top level leaves run while it loads,
//! within the load's limits.
//!
//! **Host calls.** `ctx.db.*` reaches the host through `natives.host`: the
//! slot sends the operation to the caller's future, which runs it on the
//! call's [`LiveTxn`] (so reads land in the read set) and sends the answer
//! back. The slot blocks meanwhile, and the CPU meter is paused. A storage
//! error (a conflict, a deadline) aborts the call: the JavaScript side
//! cannot catch it, and the caller returns it to the runner, which reruns
//! a mutation from scratch. A dropped caller aborts the call the same way.
//!
//! **Values.** `bigint` is `I64` (out of range is an error), number is
//! `F64`, `ArrayBuffer` is `Bytes`, arrays and plain objects (prototype
//! `Object.prototype` or `null`) map element-wise; `undefined` is `Null`,
//! except as an object field, which is left out. Anything else (functions,
//! symbols, class instances, `Date`, typed arrays, proxies) is refused.
//! Converting from JavaScript has a byte budget, charged before anything
//! is allocated and for every reference to a shared value:
//! `Limits::max_result_bytes` for a result (`RESOURCE_EXHAUSTED`), four
//! times `Limits::max_document_bytes` for a `ctx.db` call's arguments (a
//! catchable error).

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use futures::future::BoxFuture;
use loams_live::catalog::{BY_CREATION_TIME, BY_ID, CREATION_TIME_FIELD, ID_FIELD};
use loams_live::query::{QueryArgs, doc_value};
use loams_live::{CallOutput, FnKind, Function, LiveError, LiveTxn, LiveValue, LogLevel, system};
use rand::{RngCore, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rquickjs::context::intrinsic;
use rquickjs::loader::{BuiltinLoader, BuiltinResolver};
use rquickjs::object::Property;
use rquickjs::{
    Array, ArrayBuffer, BigInt, Coerced, Context, Ctx, Exception, Module, Object, Persistent,
    Promise, Runtime, Type, Value,
};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc as tmpsc;

use loams_live_js_alloc::MemoryMeter;

use crate::limits::{Console, CpuMeter};

/// The largest bundle (16 MiB, D682).
pub const MAX_BUNDLE_BYTES: usize = 16 * 1024 * 1024;

/// The most functions one bundle exports (2 048, D682).
pub const MAX_EXPORTS: usize = 2048;

/// The globals a function sees; the prelude deletes every other one. No
/// timers, `fetch`, `WebAssembly`, `performance`, `WeakRef`,
/// `FinalizationRegistry`, `Atomics` or `SharedArrayBuffer`.
pub const GLOBALS: &[&str] = &[
    "AggregateError",
    "Array",
    "ArrayBuffer",
    "BigInt",
    "BigInt64Array",
    "BigUint64Array",
    "Boolean",
    "DataView",
    "Date",
    "Error",
    "EvalError",
    "Float16Array",
    "Float32Array",
    "Float64Array",
    "Function",
    "Infinity",
    "Int16Array",
    "Int32Array",
    "Int8Array",
    "InternalError",
    "Iterator",
    "JSON",
    "Map",
    "Math",
    "NaN",
    "Number",
    "Object",
    "Promise",
    "Proxy",
    "RangeError",
    "ReferenceError",
    "Reflect",
    "RegExp",
    "Set",
    "String",
    "SuppressedError",
    "Symbol",
    "SyntaxError",
    "TypeError",
    "URIError",
    "Uint16Array",
    "Uint32Array",
    "Uint8Array",
    "Uint8ClampedArray",
    "WeakMap",
    "WeakSet",
    "console",
    "crypto",
    "decodeURI",
    "decodeURIComponent",
    "encodeURI",
    "encodeURIComponent",
    "escape",
    "eval",
    "globalThis",
    "isFinite",
    "isNaN",
    "parseFloat",
    "parseInt",
    "queueMicrotask",
    "undefined",
    "unescape",
];

const PRELUDE: &str = include_str!("prelude.js");
const SERVER_MODULE: &str = "loams:server";
const BUNDLE_MODULE: &str = "loams:bundle";
const SERVER_GLOBAL: &str = "__loams_server__";
const SERVER_SOURCE: &str = "const s = globalThis.__loams_server__;\n\
     export const query = s.query;\n\
     export const mutation = s.mutation;\n\
     export const internalQuery = s.internalQuery;\n\
     export const internalMutation = s.internalMutation;\n";
/// The name errors of bundle evaluation carry.
const BUNDLE: &str = "<bundle>";
/// A slot thread's stack; QuickJS's own stack limit is well inside it.
const SLOT_STACK_BYTES: usize = 8 * 1024 * 1024;
/// QuickJS's stack limit: deeper recursion is a `RangeError`.
const JS_STACK_BYTES: usize = 1024 * 1024;
/// What may be allocated past the memory limit after the interrupt handler
/// stops a call: enough for QuickJS to build the error that stops it.
const MEMORY_GRACE_BYTES: usize = 64 * 1024;
/// The deepest value converted between JavaScript and Rust; deeper (or
/// cyclic) values are refused. Documents allow 16 (`Limits::max_depth`).
const MAX_VALUE_DEPTH: usize = 64;

/// The longest array-like the Array methods QuickJS runs without interrupt
/// checks accept (`join`, `sort`, `slice`, …): a dense array takes at
/// least 16 bytes an element, so a longer one is sparse.
fn max_array_like(config: &JsConfig) -> f64 {
    let elements = (config.memory_limit / 16).max(1 << 16);
    u32::try_from(elements).map_or(f64::from(u32::MAX), f64::from)
}

/// The configuration of a bundle's runtime. `Default` is R1's and D682's
/// defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsConfig {
    /// The memory limit of each slot's runtime (64 MiB).
    pub memory_limit: usize,
    /// JavaScript CPU per call, host calls excluded (1 s).
    pub cpu_limit: Duration,
    /// Slots: calls that run at once, each with a context ready (4).
    pub contexts: usize,
    /// `console.*` lines kept per call (64).
    pub console_lines: usize,
    /// Bytes kept per `console.*` line (4 KiB).
    pub console_line_bytes: usize,
}

impl Default for JsConfig {
    fn default() -> Self {
        JsConfig {
            memory_limit: 64 * 1024 * 1024,
            cpu_limit: Duration::from_secs(1),
            contexts: 4,
            console_lines: 64,
            console_line_bytes: 4096,
        }
    }
}

/// Who may call a function (D699): end users (`query`, `mutation`) or
/// platform principals only (`internalQuery`, `internalMutation`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Internal,
}

/// An argument validator. Validators arrive with LV1 Task 4
/// (`loams_live::validate::Validator`); until then a bundle that declares
/// `args` is refused, so this type has no values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Validator {}

/// One function of a bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionMeta {
    /// `module:export`.
    pub path: String,
    pub kind: FnKind,
    pub visibility: Visibility,
    pub args: Option<Validator>,
}

/// A loaded bundle: one ES module whose exported objects hold functions
/// built with `loams:server`'s `query`, `mutation`, `internalQuery` and
/// `internalMutation`. Cheap to share; its slots stop when the bundle and
/// every function taken from it are dropped.
pub struct Bundle {
    metas: Arc<[FunctionMeta]>,
    pool: Arc<Pool>,
}

impl fmt::Debug for Bundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bundle")
            .field("functions", &self.metas.len())
            .field("config", &self.pool.config)
            .finish_non_exhaustive()
    }
}

impl Bundle {
    /// Validates `source` (size, evaluation within the limits, exports)
    /// and starts its slots. Evaluation runs on a thread of its own, off
    /// the async executor, for at most about `config.cpu_limit`.
    pub async fn load(source: &str, config: JsConfig) -> Result<Self, LiveError> {
        if source.len() > MAX_BUNDLE_BYTES {
            return Err(LiveError::LimitExceeded {
                limit: "max_bundle_bytes",
                message: format!(
                    "the bundle has {} bytes, more than {MAX_BUNDLE_BYTES}",
                    source.len()
                ),
            });
        }
        if config.contexts == 0 {
            return Err(LiveError::InvalidArgument(
                "a bundle needs at least one context".into(),
            ));
        }
        let source: Arc<str> = Arc::from(source);
        let metas = validate(source.clone(), config.clone()).await?;
        if metas.len() > MAX_EXPORTS {
            return Err(LiveError::LimitExceeded {
                limit: "max_exports",
                message: format!(
                    "the bundle exports {} functions, more than {MAX_EXPORTS}",
                    metas.len()
                ),
            });
        }
        let pool = Pool::start(source, config)?;
        Ok(Bundle {
            metas: metas.into(),
            pool,
        })
    }

    /// The bundle's functions, in export order.
    pub fn functions(&self) -> Vec<FunctionMeta> {
        self.metas.to_vec()
    }

    /// The function `path` (`module:export`), or `None`.
    pub fn function(&self, path: &str) -> Option<Arc<dyn Function>> {
        let meta = self.metas.iter().find(|m| m.path == path)?;
        Some(Arc::new(JsFunction {
            meta: meta.clone(),
            pool: self.pool.clone(),
        }))
    }
}

/// Evaluates the bundle once on a scratch runtime and returns its
/// functions. The evaluation runs on a thread of its own (the slots' stack
/// size), and the caller awaits it without blocking its executor.
async fn validate(source: Arc<str>, config: JsConfig) -> Result<Vec<FunctionMeta>, LiveError> {
    let (done, result) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("loams-js-load".into())
        .stack_size(SLOT_STACK_BYTES)
        .spawn(move || {
            let checked = catch_unwind(AssertUnwindSafe(|| {
                let engine = Engine::new(&config)?;
                let prepared = engine.prepare(&source)?;
                Ok(prepared.metas.clone())
            }))
            .unwrap_or_else(|_| Err(LiveError::Internal("the bundle check panicked".into())));
            // The caller may have stopped waiting.
            let _ = done.send(checked);
        })
        .map_err(|e| LiveError::Internal(format!("starting the bundle check: {e}")))?;
    result
        .await
        .map_err(|_| LiveError::Internal("the bundle check stopped".into()))?
}

/// A function of a bundle.
struct JsFunction {
    meta: FunctionMeta,
    pool: Arc<Pool>,
}

impl Function for JsFunction {
    fn name(&self) -> &str {
        &self.meta.path
    }

    fn kind(&self) -> FnKind {
        self.meta.kind
    }

    fn call<'a>(
        &'a self,
        txn: &'a mut LiveTxn<'_>,
        args: LiveValue,
    ) -> BoxFuture<'a, Result<LiveValue, LiveError>> {
        Box::pin(async move {
            let (events, mut rx) = tmpsc::unbounded_channel();
            let (reply, replies) = mpsc::channel();
            let ts = txn.start_ts();
            let limits = txn.limits();
            let job = Job {
                path: self.meta.path.clone(),
                args,
                start_ms: ts.physical_ms(),
                seed: seed(ts.0, txn.request_id()),
                result_bytes: limits.max_result_bytes,
                host_bytes: limits.max_document_bytes.saturating_mul(HOST_BUDGET_FACTOR),
                events,
                replies,
            };
            self.pool
                .jobs
                .send(job)
                .map_err(|_| LiveError::Internal("the JavaScript slots have stopped".into()))?;
            let mut storage: Option<LiveError> = None;
            while let Some(event) = rx.recv().await {
                match event {
                    Event::Host { op, args } => {
                        let answer = match &storage {
                            Some(e) => Err(e.clone()),
                            None => host(txn, op, args).await,
                        };
                        if let Err(e @ LiveError::Txn(_)) = &answer {
                            storage = Some(e.clone());
                        }
                        // The slot has gone only if the call was aborted.
                        let _ = reply.send(answer);
                    }
                    Event::Done { result, output } => {
                        let out = txn.output_mut();
                        out.logs.extend(output.logs);
                        out.dropped = out.dropped.saturating_add(output.dropped);
                        return match storage {
                            Some(e) => Err(e),
                            None => result,
                        };
                    }
                }
            }
            Err(LiveError::Internal(
                "the JavaScript slot stopped during the call".into(),
            ))
        })
    }
}

/// The seed of `Math.random` for a call: SHA-256 of the start timestamp
/// and the request id.
fn seed(ts: u64, request_id: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"loams.live.random.v1\0");
    h.update(ts.to_be_bytes());
    h.update(request_id.as_bytes());
    let digest = h.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// A `ctx.db` operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostOp {
    Get,
    Query,
    Insert,
    Patch,
    Replace,
    Delete,
}

impl HostOp {
    fn parse(op: &str) -> Option<Self> {
        Some(match op {
            "get" => HostOp::Get,
            "query" => HostOp::Query,
            "insert" => HostOp::Insert,
            "patch" => HostOp::Patch,
            "replace" => HostOp::Replace,
            "delete" => HostOp::Delete,
            _ => return None,
        })
    }
}

/// Runs a host operation on the call's transaction. The arguments have the
/// `_system:*` shapes; a query also names the index fields its range
/// uses, which are checked against the index.
async fn host(txn: &mut LiveTxn<'_>, op: HostOp, args: LiveValue) -> Result<LiveValue, LiveError> {
    let name = match op {
        HostOp::Get => system::GET,
        HostOp::Query => return host_query(txn, args).await,
        HostOp::Insert => system::INSERT,
        HostOp::Patch => system::PATCH,
        HostOp::Replace => system::REPLACE,
        HostOp::Delete => system::DELETE,
    };
    let f = system::lookup(name)
        .ok_or_else(|| LiveError::Internal(format!("no system function {name}")))?;
    f.call(txn, args).await
}

async fn host_query(txn: &mut LiveTxn<'_>, args: LiveValue) -> Result<LiveValue, LiveError> {
    let LiveValue::Object(mut args) = args else {
        return Err(LiveError::Internal(
            "a query's arguments are an object".into(),
        ));
    };
    let named: Vec<String> = match args.remove("fields") {
        Some(LiveValue::Array(fields)) => fields
            .into_iter()
            .map(|f| match f {
                LiveValue::Str(s) => Ok(s),
                other => Err(LiveError::InvalidArgument(format!(
                    "withIndex: a field name is a string, not {}",
                    other.type_name()
                ))),
            })
            .collect::<Result<_, _>>()?,
        _ => Vec::new(),
    };
    let query = QueryArgs::parse("db.query", LiveValue::Object(args))?;
    let Some(table) = txn.table(&query.table).await? else {
        return Ok(LiveValue::Array(Vec::new()));
    };
    let fields: Vec<String> = match query.index.as_str() {
        BY_CREATION_TIME => vec![CREATION_TIME_FIELD.to_string()],
        BY_ID => vec![ID_FIELD.to_string()],
        name => table
            .indexes
            .iter()
            .find(|i| i.name == name)
            .map(|i| i.fields.clone())
            .unwrap_or_default(),
    };
    if table.index_id(&query.index).is_some()
        && (named.len() > fields.len() || named.iter().zip(&fields).any(|(a, b)| a != b))
    {
        return Err(LiveError::InvalidArgument(format!(
            "withIndex: index '{}' of table '{}' has the fields [{}], in that order; \
             the range names [{}]",
            query.index,
            table.name,
            fields.join(", "),
            named.join(", ")
        )));
    }
    let docs = txn.query(query.range(&table)?).await?;
    Ok(LiveValue::Array(docs.iter().map(doc_value).collect()))
}

// ---- the slots ----

/// What a slot sends the caller.
enum Event {
    Host {
        op: HostOp,
        args: LiveValue,
    },
    Done {
        result: Result<LiveValue, LiveError>,
        output: CallOutput,
    },
}

/// One call, queued for a slot.
struct Job {
    path: String,
    args: LiveValue,
    start_ms: u64,
    seed: [u8; 32],
    /// The conversion budget of the result (`Limits::max_result_bytes`).
    result_bytes: usize,
    /// The conversion budget of each `ctx.db` call's arguments.
    host_bytes: usize,
    events: tmpsc::UnboundedSender<Event>,
    replies: mpsc::Receiver<Result<LiveValue, LiveError>>,
}

/// A bundle's slots. Dropping it closes the queue; each slot ends after
/// its current call.
struct Pool {
    jobs: mpsc::Sender<Job>,
    config: JsConfig,
}

impl Pool {
    fn start(source: Arc<str>, config: JsConfig) -> Result<Arc<Self>, LiveError> {
        let (jobs, queue) = mpsc::channel();
        let queue = Arc::new(Mutex::new(queue));
        for n in 0..config.contexts {
            let (source, config, queue) = (source.clone(), config.clone(), queue.clone());
            std::thread::Builder::new()
                .name(format!("loams-js-{n}"))
                .stack_size(SLOT_STACK_BYTES)
                .spawn(move || slot_main(&source, &config, &queue))
                .map_err(|e| LiveError::Internal(format!("starting a JavaScript slot: {e}")))?;
        }
        Ok(Arc::new(Pool { jobs, config }))
    }
}

/// How long a slot waits before restarting after its `restarts`-th panic
/// in a row: 10 ms, doubling, at most 5 s.
fn restart_delay(restarts: u32) -> Duration {
    let ms = 10u64.saturating_mul(1u64 << restarts.saturating_sub(1).min(16));
    Duration::from_millis(ms.min(5_000))
}

/// A slot that panicked after this long counts as healthy again.
const SLOT_HEALTHY_AFTER: Duration = Duration::from_secs(60);

fn slot_main(source: &str, config: &JsConfig, queue: &Mutex<mpsc::Receiver<Job>>) {
    let mut restarts = 0u32;
    loop {
        let started = std::time::Instant::now();
        match catch_unwind(AssertUnwindSafe(|| slot_loop(source, config, queue))) {
            Ok(()) => return,
            Err(_) => {
                restarts = if started.elapsed() > SLOT_HEALTHY_AFTER {
                    1
                } else {
                    restarts.saturating_add(1)
                };
                let wait = restart_delay(restarts);
                tracing::error!(
                    restarts,
                    ?wait,
                    "a Loams Live JavaScript slot panicked; restarting it"
                );
                // A slot that panics at once must not spin.
                std::thread::sleep(wait);
            }
        }
    }
}

/// Serves calls until the queue closes.
fn slot_loop(source: &str, config: &JsConfig, queue: &Mutex<mpsc::Receiver<Job>>) {
    let mut engine: Option<Engine> = None;
    // Declared after `engine`, so it is dropped first: a context must not
    // outlive its runtime.
    let mut ready: Option<Result<Prepared, LiveError>> = None;
    loop {
        if engine.is_none() {
            ready = None;
            engine = match Engine::new(config) {
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
            ready = Some(engine.prepare(source));
        }
        let job = {
            let queue = queue.lock().unwrap_or_else(|e| e.into_inner());
            match queue.recv() {
                Ok(job) => job,
                Err(_) => return,
            }
        };
        match (ready.take(), &engine) {
            (Some(Ok(prepared)), Some(e)) => {
                if e.run(prepared, job) {
                    engine = None;
                }
            }
            (Some(Err(err)), _) => {
                let _ = job.events.send(Event::Done {
                    result: Err(err),
                    output: CallOutput::default(),
                });
                // A context that failed to prepare (the bundle loaded
                // before) gets a fresh runtime next time.
                engine = None;
            }
            _ => {
                let _ = job.events.send(Event::Done {
                    result: Err(LiveError::Internal("no JavaScript context is ready".into())),
                    output: CallOutput::default(),
                });
                engine = None;
            }
        }
    }
}

/// The host's link to the call a slot runs.
struct Link {
    events: tmpsc::UnboundedSender<Event>,
    replies: mpsc::Receiver<Result<LiveValue, LiveError>>,
}

/// The per-call state the natives and the interrupt handler share.
struct SlotState {
    meter: CpuMeter,
    /// The runtime's memory account (its allocator counts against it).
    memory: MemoryMeter,
    abort: Cell<bool>,
    now_ms: Cell<f64>,
    rng: RefCell<ChaCha8Rng>,
    console: RefCell<Console>,
    link: RefCell<Option<Link>>,
    host_errors: RefCell<Vec<LiveError>>,
    /// The conversion budget of each `ctx.db` call's arguments.
    host_bytes: Cell<usize>,
    config: JsConfig,
}

impl SlotState {
    fn new(config: &JsConfig, memory: MemoryMeter) -> Self {
        SlotState {
            meter: CpuMeter::new(),
            memory,
            abort: Cell::new(false),
            now_ms: Cell::new(0.0),
            rng: RefCell::new(ChaCha8Rng::from_seed([0; 32])),
            console: RefCell::new(Console::new(
                config.console_lines,
                config.console_line_bytes,
            )),
            link: RefCell::new(None),
            host_errors: RefCell::new(Vec::new()),
            host_bytes: Cell::new(0),
            config: config.clone(),
        }
    }

    /// Resets the state for a call (or, with `None`, for evaluating the
    /// bundle: the clock at the epoch, a fixed seed, no host).
    fn begin(&self, call: Option<(u64, [u8; 32], Link)>) {
        self.abort.set(false);
        self.host_errors.borrow_mut().clear();
        *self.console.borrow_mut() =
            Console::new(self.config.console_lines, self.config.console_line_bytes);
        let (now, seed, link) = match call {
            Some((now, seed, link)) => (now, seed, Some(link)),
            None => (0, [0; 32], None),
        };
        // Milliseconds since the epoch are exact in an f64 until 2^53.
        self.now_ms.set(now as f64);
        *self.rng.borrow_mut() = ChaCha8Rng::from_seed(seed);
        *self.link.borrow_mut() = link;
        self.meter.start(self.config.cpu_limit);
    }

    /// Whether the call has been stopped (aborted, out of memory or past
    /// its CPU limit): no more of its jobs may run.
    fn halted(&self) -> bool {
        self.abort.get() || self.memory.exceeded() || self.meter.timed_out()
    }

    /// Whether the interrupt handler stops the running code.
    fn interrupt(&self) -> bool {
        if self.abort.get() {
            return true;
        }
        if self.memory.exceeded() {
            // QuickJS allocates the error that stops the call right after
            // this returns, past the limit.
            self.memory.top_up_grace(MEMORY_GRACE_BYTES);
            return true;
        }
        let gone = self
            .link
            .try_borrow()
            .is_ok_and(|l| l.as_ref().is_some_and(|l| l.events.is_closed()));
        if gone {
            self.abort.set(true);
            return true;
        }
        self.meter.check()
    }

    fn random(&self) -> f64 {
        let bits = self.rng.borrow_mut().next_u64() >> 11;
        bits as f64 / (1u64 << 53) as f64
    }
}

/// A slot's runtime.
struct Engine {
    rt: Runtime,
    state: Rc<SlotState>,
}

/// A context with the prelude run and the bundle evaluated. `internals`
/// is declared first so it is dropped before `ctx`.
struct Prepared {
    internals: Persistent<Object<'static>>,
    metas: Vec<FunctionMeta>,
    ctx: Context,
}

/// How a failed call is classified.
#[derive(Clone, Copy)]
enum Phase<'a> {
    Load,
    Call(&'a str),
}

impl Phase<'_> {
    fn function(&self) -> String {
        match self {
            Phase::Load => BUNDLE.to_string(),
            Phase::Call(path) => path.to_string(),
        }
    }
}

impl Engine {
    fn new(config: &JsConfig) -> Result<Self, LiveError> {
        // The memory limit is the allocator's, not QuickJS's: QuickJS's own
        // out-of-memory error is catchable, the allocator's flag is not
        // (row T3-6).
        let memory = MemoryMeter::new(config.memory_limit);
        let rt = Runtime::new_with_alloc(memory.allocator())
            .map_err(|e| LiveError::Internal(format!("creating a JavaScript runtime: {e}")))?;
        rt.set_max_stack_size(JS_STACK_BYTES);
        let state = Rc::new(SlotState::new(config, memory));
        let handler_state = state.clone();
        rt.set_interrupt_handler(Some(Box::new(move || handler_state.interrupt())));
        rt.set_loader(
            BuiltinResolver::default().with_module(SERVER_MODULE),
            BuiltinLoader::default(),
        );
        Ok(Engine { rt, state })
    }

    /// A fresh context: the prelude, the frozen globals, `loams:server`
    /// and the evaluated bundle.
    fn prepare(&self, source: &str) -> Result<Prepared, LiveError> {
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
        )>(&self.rt)
        .map_err(|e| LiveError::Internal(format!("creating a JavaScript context: {e}")))?;
        self.state.begin(None);
        let prepared = ctx.with(|ctx| {
            let steps = || -> rquickjs::Result<(Object<'_>, Vec<Vec<String>>)> {
                let setup: rquickjs::Function = ctx.eval(PRELUDE)?;
                let natives = self.natives(&ctx)?;
                let config = Object::new(ctx.clone())?;
                config.set("lineBytes", self.state.config.console_line_bytes)?;
                config.set("maxLength", max_array_like(&self.state.config))?;
                config.set("globals", GLOBALS.to_vec())?;
                let internals: Object = setup.call((natives, config))?;
                let globals = ctx.globals();
                globals.set(SERVER_GLOBAL, internals.get::<_, Object>("server")?)?;
                let (_, done) =
                    Module::declare(ctx.clone(), SERVER_MODULE, SERVER_SOURCE)?.eval()?;
                self.settle::<()>(&ctx, &done)?;
                globals.remove(SERVER_GLOBAL)?;
                let (module, done) = Module::declare(ctx.clone(), BUNDLE_MODULE, source)?.eval()?;
                self.settle::<()>(&ctx, &done)?;
                let namespace = module.namespace()?;
                let collect: rquickjs::Function = internals.get("collect")?;
                let metas: Vec<Vec<String>> = collect.call((namespace,))?;
                Ok((internals, metas))
            };
            match steps() {
                Ok((internals, metas)) => {
                    // The jobs the top level left (a floating promise) run
                    // now, within the load's limits, so none is left for
                    // the first call (C1). A top level that never stops
                    // queueing jobs is stopped by the CPU limit.
                    while !self.state.halted() && ctx.execute_pending_job() {}
                    if let Some(stopped) = self.stopped(Phase::Load) {
                        return Err(stopped);
                    }
                    let metas = metas.into_iter().map(meta).collect::<Result<Vec<_>, _>>()?;
                    Ok((Persistent::save(&ctx, internals), metas))
                }
                Err(e) => Err(self.classify(&ctx, e, Phase::Load, None)),
            }
        });
        self.state.meter.pause();
        let (internals, metas) = prepared.map_err(|e| match e {
            LiveError::FunctionError(m) => {
                LiveError::InvalidArgument(format!("the bundle failed to load: {m}"))
            }
            other => other,
        })?;
        Ok(Prepared {
            internals,
            metas,
            ctx,
        })
    }

    /// The host functions the prelude receives.
    fn natives<'js>(&self, ctx: &Ctx<'js>) -> rquickjs::Result<Object<'js>> {
        let natives = Object::new(ctx.clone())?;
        let s = self.state.clone();
        natives.set(
            "now",
            rquickjs::Function::new(ctx.clone(), move || s.now_ms.get())?,
        )?;
        let s = self.state.clone();
        natives.set(
            "random",
            rquickjs::Function::new(ctx.clone(), move || s.random())?,
        )?;
        natives.set(
            "isProxy",
            rquickjs::Function::new(ctx.clone(), |value: Value<'js>| value.is_proxy())?,
        )?;
        let s = self.state.clone();
        natives.set(
            "log",
            rquickjs::Function::new(ctx.clone(), move |level: String, line: String| {
                let level = match level.as_str() {
                    "debug" => LogLevel::Debug,
                    "info" => LogLevel::Info,
                    "warn" => LogLevel::Warn,
                    "error" => LogLevel::Error,
                    _ => LogLevel::Log,
                };
                s.console.borrow_mut().push(level, &line);
            })?,
        )?;
        let s = self.state.clone();
        natives.set(
            "host",
            rquickjs::Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>,
                      op: String,
                      args: Value<'js>,
                      values: Object<'js>|
                      -> rquickjs::Result<Object<'js>> {
                    let helpers = Helpers::from(&values)?;
                    host_call(&s, &ctx, &op, args, &helpers)
                },
            )?,
        )?;
        Ok(natives)
    }

    /// Runs `job` in `prepared`, sends its outcome and drops the context.
    /// Returns whether the runtime must be replaced.
    fn run(&self, prepared: Prepared, job: Job) -> bool {
        let Job {
            path,
            args,
            start_ms,
            seed,
            result_bytes,
            host_bytes,
            events,
            replies,
        } = job;
        let link = Link {
            events: events.clone(),
            replies,
        };
        self.state.begin(Some((start_ms, seed, link)));
        self.state.host_bytes.set(host_bytes);
        let Prepared { internals, ctx, .. } = prepared;
        let result = ctx.with(|ctx| {
            let internals = match internals.restore(&ctx) {
                Ok(i) => i,
                Err(e) => return Err(LiveError::Internal(format!("restoring a context: {e}"))),
            };
            let call = || -> rquickjs::Result<Value<'_>> {
                let invoke: rquickjs::Function = internals.get("invoke")?;
                let args = to_js(&ctx, &args)?;
                let done: Promise = invoke.call((path.as_str(), args))?;
                self.settle::<Value>(&ctx, &done)
            };
            let phase = Phase::Call(&path);
            match call() {
                Ok(value) => {
                    let helpers = match internals
                        .get::<_, Object>("values")
                        .and_then(|v| Helpers::from(&v))
                    {
                        Ok(h) => h,
                        Err(e) => return Err(self.classify(&ctx, e, phase, Some(&internals))),
                    };
                    let mut budget = Budget::new(result_bytes, &self.state.meter);
                    match to_live(&ctx, value, &helpers, &mut budget, 0, false) {
                        Ok(v) => Ok(v),
                        Err(Conv::Js(e)) => Err(self.classify(&ctx, e, phase, Some(&internals))),
                        Err(Conv::Invalid(m)) => Err(self.stopped(phase).unwrap_or_else(|| {
                            LiveError::FunctionError(format!("{path} returned {m}"))
                        })),
                        Err(Conv::Stopped) => Err(self.stopped(phase).unwrap_or_else(|| {
                            LiveError::Internal("a conversion stopped".into())
                        })),
                        Err(Conv::Over) => Err(self.stopped(phase).unwrap_or_else(|| {
                            LiveError::LimitExceeded {
                                limit: "max_result_bytes",
                                message: format!(
                                    "{path} returned a value larger than {result_bytes} bytes \
                                     (every reference to a shared value counts)"
                                ),
                            }
                        })),
                    }
                }
                Err(rquickjs::Error::WouldBlock) => Err(self.stopped(phase).unwrap_or_else(|| {
                    LiveError::FunctionError(format!(
                        "{path}: the handler's promise never settled (it awaits something \
                         that never happens)"
                    ))
                })),
                Err(e) => Err(self.classify(&ctx, e, phase, Some(&internals))),
            }
        });
        self.state.meter.pause();
        let output = self.state.console.borrow_mut().take();
        self.state.link.borrow_mut().take();
        // Jobs still queued (a floating promise) would run in the next
        // call's `finish`, with its host link: its transaction, its
        // console and its CPU. They belong to this call's context, so the
        // runtime is replaced with them (C1).
        // Out of memory is the allocator's flag, whatever the call did
        // after it: a handler that caught QuickJS's error and returned still
        // ran out (row T3-6).
        let result = match result {
            Ok(_) if self.state.memory.exceeded() => Err(self.out_of_memory(Phase::Call(&path))),
            other => other,
        };
        let poisoned = self.state.halted()
            || matches!(result, Err(LiveError::FunctionOutOfMemory { .. }))
            || self.rt.is_job_pending();
        drop(ctx);
        if !poisoned {
            // Cycles of the dropped context, so the next call starts with
            // the memory it had.
            self.rt.run_gc();
        }
        let _ = events.send(Event::Done { result, output });
        poisoned
    }

    /// Runs the runtime's jobs until `promise` settles, as
    /// `Promise::finish` does, but runs none once the call is halted: a
    /// stopped call's jobs are dropped with its runtime.
    fn settle<'js, T: rquickjs::FromJs<'js>>(
        &self,
        ctx: &Ctx<'js>,
        promise: &Promise<'js>,
    ) -> rquickjs::Result<T> {
        loop {
            if let Some(settled) = promise.result() {
                return settled;
            }
            if self.state.halted() || !ctx.execute_pending_job() {
                return Err(rquickjs::Error::WouldBlock);
            }
        }
    }

    fn out_of_memory(&self, phase: Phase<'_>) -> LiveError {
        LiveError::FunctionOutOfMemory {
            function: phase.function(),
            limit: self.state.config.memory_limit,
        }
    }

    /// The error of a call the host stopped: aborted, out of memory (the
    /// allocator's flag) or past its CPU limit.
    fn stopped(&self, phase: Phase<'_>) -> Option<LiveError> {
        if self.state.abort.get() {
            return Some(LiveError::Internal("the call was aborted".into()));
        }
        if self.state.memory.exceeded() {
            return Some(self.out_of_memory(phase));
        }
        if self.state.meter.timed_out() {
            return Some(LiveError::FunctionTimeout {
                function: phase.function(),
                limit: self.state.config.cpu_limit,
            });
        }
        None
    }

    /// The error of a failed evaluation or call.
    fn classify<'js>(
        &self,
        ctx: &Ctx<'js>,
        error: rquickjs::Error,
        phase: Phase<'_>,
        internals: Option<&Object<'js>>,
    ) -> LiveError {
        if let Some(stopped) = self.stopped(phase) {
            if error.is_exception() {
                ctx.catch();
            }
            return stopped;
        }
        let thrown = match error {
            e if e.is_exception() => ctx.catch(),
            other => return LiveError::FunctionError(other.to_string()),
        };
        if let (Some(internals), Some(_)) = (internals, thrown.as_object()) {
            let index = internals
                .get::<_, rquickjs::Function>("hostError")
                .and_then(|f| f.call::<_, i32>((thrown.clone(),)));
            match index {
                Ok(i) if i >= 0 => {
                    if let Some(e) = usize::try_from(i)
                        .ok()
                        .and_then(|i| self.state.host_errors.borrow().get(i).cloned())
                    {
                        return e;
                    }
                }
                Ok(_) => {}
                Err(_) => {
                    ctx.catch();
                }
            }
        }
        let text = describe(ctx, &thrown);
        if let Some(stopped) = self.stopped(phase) {
            return stopped;
        }
        LiveError::FunctionError(text)
    }
}

/// The text of a thrown value. Never the basis of a classification: a
/// function can throw any text (out of memory is the allocator's flag).
fn describe<'js>(ctx: &Ctx<'js>, thrown: &Value<'js>) -> String {
    if let Some(e) = thrown
        .as_object()
        .and_then(|o| Exception::from_object(o.clone()))
    {
        let name = e
            .get::<_, Coerced<String>>("name")
            .map(|n| n.0)
            .unwrap_or_else(|_| {
                ctx.catch();
                "Error".to_string()
            });
        let message = e
            .get::<_, Coerced<String>>("message")
            .map(|m| m.0)
            .unwrap_or_else(|_| {
                ctx.catch();
                String::new()
            });
        return format!("{name}: {message}");
    }
    match thrown.get::<Coerced<String>>() {
        Ok(s) => format!("Uncaught {}", s.0),
        Err(_) => {
            ctx.catch();
            "Uncaught exception".to_string()
        }
    }
}

/// `natives.host(op, args, values)`: one `ctx.db` operation. The reply is
/// `{ ok }`, `{ error, index }` (a catchable error the host keeps as
/// `index`) or `{ abort: true }`.
fn host_call<'js>(
    s: &SlotState,
    ctx: &Ctx<'js>,
    op: &str,
    args: Value<'js>,
    helpers: &Helpers<'js>,
) -> rquickjs::Result<Object<'js>> {
    let reply = Object::new(ctx.clone())?;
    if s.halted() {
        reply.set("abort", true)?;
        return Ok(reply);
    }
    let refuse = |reply: &Object<'js>, e: LiveError| -> rquickjs::Result<()> {
        let mut errors = s.host_errors.borrow_mut();
        reply.set("error", js_message(&e))?;
        reply.set("index", errors.len())?;
        errors.push(e);
        Ok(())
    };
    let Some(host_op) = HostOp::parse(op) else {
        refuse(
            &reply,
            LiveError::Internal(format!("no host operation {op}")),
        )?;
        return Ok(reply);
    };
    let max = s.host_bytes.get();
    let mut budget = Budget::new(max, &s.meter);
    let args = match to_live(ctx, args, helpers, &mut budget, 0, false) {
        Ok(v) => v,
        Err(Conv::Js(e)) => return Err(e),
        Err(Conv::Invalid(m)) => {
            refuse(&reply, LiveError::InvalidArgument(format!("db: {m}")))?;
            return Ok(reply);
        }
        Err(Conv::Stopped) => {
            // Past the CPU limit: the interrupt handler stops the call at
            // its next poll, uncatchably.
            reply.set("abort", true)?;
            return Ok(reply);
        }
        Err(Conv::Over) => {
            let e = LiveError::LimitExceeded {
                limit: "max_document_bytes",
                message: format!(
                    "ctx.db.{op}: the arguments are larger than {max} bytes, four times \
                     max_document_bytes (every reference to a shared value counts)"
                ),
            };
            refuse(&reply, e)?;
            return Ok(reply);
        }
    };
    let answer = {
        let link = s.link.borrow();
        let Some(link) = link.as_ref() else {
            refuse(
                &reply,
                LiveError::InvalidArgument("ctx.db is available only inside a handler".into()),
            )?;
            return Ok(reply);
        };
        s.meter.pause();
        let answer = match link.events.send(Event::Host { op: host_op, args }) {
            Ok(()) => link.replies.recv().ok(),
            Err(_) => None,
        };
        s.meter.resume();
        answer
    };
    match answer {
        Some(Ok(value)) => reply.set("ok", to_js(ctx, &value)?)?,
        Some(Err(LiveError::Txn(_))) | None => {
            s.abort.set(true);
            reply.set("abort", true)?;
        }
        Some(Err(e)) => refuse(&reply, e)?,
    }
    Ok(reply)
}

/// The text of a host error as JavaScript sees it. An internal or
/// corrupt-data error's detail (keys, paths, the store's own text) stays
/// on the host: JavaScript gets the kind only, and the detail is logged.
fn js_message(e: &LiveError) -> String {
    match e {
        LiveError::Internal(_) | LiveError::Corrupt(_) => {
            tracing::warn!(error = %e, "a host call failed inside the store");
            "internal error (the details are in the host's log)".to_string()
        }
        other => other.to_string(),
    }
}

fn meta(row: Vec<String>) -> Result<FunctionMeta, LiveError> {
    let [path, kind, visibility] = <[String; 3]>::try_from(row)
        .map_err(|_| LiveError::Internal("a function row has three fields".into()))?;
    let kind = match kind.as_str() {
        "query" => FnKind::Query,
        "mutation" => FnKind::Mutation,
        other => return Err(LiveError::Internal(format!("a function kind {other}"))),
    };
    let visibility = match visibility.as_str() {
        "public" => Visibility::Public,
        "internal" => Visibility::Internal,
        other => return Err(LiveError::Internal(format!("a visibility {other}"))),
    };
    Ok(FunctionMeta {
        path,
        kind,
        visibility,
        args: None,
    })
}

// ---- values ----

/// What converting a JavaScript value charges per array element, object
/// field and scalar, besides the bytes of strings, keys and buffers.
const NODE_BYTES: usize = 8;

/// A `ctx.db` call's arguments may cost this many times
/// `Limits::max_document_bytes`: the conversion charges at most four times
/// a value's encoded size, so every document the store would accept fits,
/// and the store checks the exact size.
const HOST_BUDGET_FACTOR: usize = 4;

/// Why a JavaScript value did not convert.
enum Conv {
    /// JavaScript threw (an interrupt, out of memory, a throwing getter).
    Js(rquickjs::Error),
    /// The value is not a Loams value.
    Invalid(String),
    /// The value is larger than the conversion's budget.
    Over,
    /// The call ran past its CPU limit while converting.
    Stopped,
}

impl From<rquickjs::Error> for Conv {
    fn from(e: rquickjs::Error) -> Self {
        Conv::Js(e)
    }
}

/// The bytes one conversion may still charge (C2). Every node is charged
/// before anything is allocated for it, and every reference to a shared
/// value is charged again, so a sparse array, a string shared many times
/// or a DAG of shared arrays runs out of budget instead of memory.
/// The conversion also checks the call's CPU meter every so many nodes:
/// it runs in Rust, where the interrupt handler is not polled.
struct Budget<'m> {
    left: usize,
    nodes: u32,
    meter: &'m CpuMeter,
}

impl<'m> Budget<'m> {
    fn new(bytes: usize, meter: &'m CpuMeter) -> Self {
        Budget {
            left: bytes,
            nodes: 0,
            meter,
        }
    }

    fn charge(&mut self, bytes: usize) -> Result<(), Conv> {
        self.left = self.left.checked_sub(bytes).ok_or(Conv::Over)?;
        Ok(())
    }

    /// Counts a node; every 1 024 nodes, stops a call past its CPU limit.
    fn tick(&mut self) -> Result<(), Conv> {
        self.nodes = self.nodes.wrapping_add(1);
        if self.nodes.is_multiple_of(1024) && self.meter.check() {
            return Err(Conv::Stopped);
        }
        Ok(())
    }
}

/// The prelude's value helpers: `bytes(buffer)` (an `ArrayBuffer` as a
/// Latin-1 string) and `size(value)` (a string's length in UTF-16 units,
/// an `ArrayBuffer`'s byte length).
struct Helpers<'js> {
    bytes: rquickjs::Function<'js>,
    size: rquickjs::Function<'js>,
}

impl<'js> Helpers<'js> {
    fn from(values: &Object<'js>) -> rquickjs::Result<Self> {
        Ok(Helpers {
            bytes: values.get("bytes")?,
            size: values.get("size")?,
        })
    }

    fn size(&self, value: &Value<'js>) -> Result<usize, Conv> {
        let n: f64 = self.size.call((value.clone(),))?;
        // A length is an integer below 2^53; anything else is refused.
        if !(0.0..9_007_199_254_740_992.0).contains(&n) {
            return Err(Conv::Over);
        }
        Ok(n as usize)
    }
}

/// Converts `value`. `prepaid` says the caller already charged the node
/// itself (an array element or an object field).
fn to_live<'js>(
    ctx: &Ctx<'js>,
    value: Value<'js>,
    helpers: &Helpers<'js>,
    budget: &mut Budget<'_>,
    depth: usize,
    prepaid: bool,
) -> Result<LiveValue, Conv> {
    budget.tick()?;
    if depth > MAX_VALUE_DEPTH {
        return Err(Conv::Invalid(format!(
            "a value nested deeper than {MAX_VALUE_DEPTH} (or a cycle)"
        )));
    }
    if value.is_proxy() {
        return Err(Conv::Invalid("a Proxy, which is not a Loams value".into()));
    }
    if !prepaid {
        budget.charge(NODE_BYTES)?;
    }
    Ok(match value.type_of() {
        Type::Uninitialized | Type::Undefined | Type::Null => LiveValue::Null,
        Type::Bool => LiveValue::Bool(value.as_bool().unwrap_or_default()),
        Type::Int => LiveValue::F64(f64::from(value.as_int().unwrap_or_default())),
        Type::Float => LiveValue::F64(value.as_float().unwrap_or_default()),
        Type::String => {
            // UTF-8 takes at least one byte per UTF-16 unit: charge that
            // before converting, and the rest after.
            let units = helpers.size(&value)?;
            budget.charge(units)?;
            let s = value
                .as_string()
                .ok_or_else(|| Conv::Invalid("a string".into()))?
                .to_string()?;
            budget.charge(s.len().saturating_sub(units))?;
            LiveValue::Str(s)
        }
        Type::BigInt => {
            let text: Coerced<String> = value.get()?;
            LiveValue::I64(
                text.0
                    .parse()
                    .map_err(|_| Conv::Invalid(format!("the bigint {}n, outside int64", text.0)))?,
            )
        }
        Type::Array => {
            let array = value
                .as_array()
                .ok_or_else(|| Conv::Invalid("an array".into()))?;
            // Read as a number: rquickjs's `Array::len` panics past 2^31.
            let length: f64 = array.as_object().get("length")?;
            if !(0.0..=f64::from(u32::MAX)).contains(&length) {
                return Err(Conv::Invalid(format!("an array of length {length}")));
            }
            let length = length as u32;
            budget.charge((length as usize).saturating_mul(NODE_BYTES))?;
            let mut items = Vec::new();
            for i in 0..length {
                let item = array.get::<Value>(i as usize)?;
                items.push(to_live(ctx, item, helpers, budget, depth + 1, true)?);
            }
            LiveValue::Array(items)
        }
        Type::Object => {
            let object = value
                .as_object()
                .ok_or_else(|| Conv::Invalid("an object".into()))?
                .clone();
            if ArrayBuffer::from_object(object.clone()).is_some() {
                budget.charge(helpers.size(&value)?)?;
                let latin1: rquickjs::String = helpers.bytes.call((object,))?;
                let bytes = latin1
                    .to_string()?
                    .chars()
                    .map(|c| u8::try_from(u32::from(c)))
                    .collect::<Result<Vec<u8>, _>>()
                    .map_err(|_| Conv::Invalid("an unreadable ArrayBuffer".into()))?;
                return Ok(LiveValue::Bytes(bytes));
            }
            let plain = match object.get_prototype() {
                None => true,
                Some(proto) => {
                    let object_proto: Object =
                        ctx.globals().get::<_, Object>("Object")?.get("prototype")?;
                    proto == object_proto
                }
            };
            if !plain {
                return Err(Conv::Invalid(
                    "an object that is not a plain object (a class instance, Date, Map, \
                     typed array…); use plain objects, arrays and ArrayBuffer"
                        .into(),
                ));
            }
            let mut fields = BTreeMap::new();
            for entry in object.props::<String, Value>() {
                let (key, item) = entry?;
                if item.is_undefined() {
                    continue;
                }
                budget.charge(NODE_BYTES.saturating_add(key.len()))?;
                fields.insert(key, to_live(ctx, item, helpers, budget, depth + 1, true)?);
            }
            LiveValue::Object(fields)
        }
        other => {
            return Err(Conv::Invalid(format!(
                "a {}, which is not a Loams value",
                other.as_str()
            )));
        }
    })
}

fn to_js<'js>(ctx: &Ctx<'js>, value: &LiveValue) -> rquickjs::Result<Value<'js>> {
    Ok(match value {
        LiveValue::Null => Value::new_null(ctx.clone()),
        LiveValue::I64(i) => BigInt::from_i64(ctx.clone(), *i)?.into_value(),
        LiveValue::F64(f) => Value::new_number(ctx.clone(), *f),
        LiveValue::Bool(b) => Value::new_bool(ctx.clone(), *b),
        LiveValue::Str(s) => rquickjs::String::from_str(ctx.clone(), s)?.into_value(),
        LiveValue::Bytes(b) => ArrayBuffer::new(ctx.clone(), b.clone())?.into_value(),
        LiveValue::Array(items) => {
            let array = Array::new(ctx.clone())?;
            for (i, item) in items.iter().enumerate() {
                array.set(i, to_js(ctx, item)?)?;
            }
            array.into_value()
        }
        LiveValue::Object(fields) => {
            let object = Object::new(ctx.clone())?;
            for (key, item) in fields {
                // Defined, not assigned: a field named `__proto__` is a
                // field, not the prototype.
                object.prop(
                    key.as_str(),
                    Property::from(to_js(ctx, item)?)
                        .writable()
                        .enumerable()
                        .configurable(),
                )?;
            }
            object.into_value()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_differ_by_timestamp_and_request() {
        assert_eq!(seed(1, "a"), seed(1, "a"));
        assert_ne!(seed(1, "a"), seed(2, "a"));
        assert_ne!(seed(1, "a"), seed(1, "b"));
    }

    #[test]
    fn javascript_never_sees_internal_detail() {
        for e in [
            LiveError::Internal("key 0xdeadbeef in /var/lib/loams".into()),
            LiveError::Corrupt("record 0xdeadbeef".into()),
        ] {
            let m = js_message(&e);
            assert!(!m.contains("deadbeef"), "{m}");
            assert!(m.starts_with("internal error"), "{m}");
        }
        let m = js_message(&LiveError::NotFound("table t".into()));
        assert!(m.contains("table t"), "{m}");
    }

    #[test]
    fn slot_restarts_back_off() {
        assert_eq!(restart_delay(1), Duration::from_millis(10));
        assert_eq!(restart_delay(2), Duration::from_millis(20));
        assert_eq!(restart_delay(5), Duration::from_millis(160));
        assert_eq!(restart_delay(20), Duration::from_secs(5));
        assert_eq!(restart_delay(u32::MAX), Duration::from_secs(5));
    }

    #[test]
    fn host_ops_parse() {
        assert_eq!(HostOp::parse("get"), Some(HostOp::Get));
        assert_eq!(HostOp::parse("tables"), None);
    }
}
