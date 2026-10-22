//! Traces (HS1 Task 23, design §49 §15): one trace per statement, with spans
//! for its admission, each worker execution, the `house-cache` reads its worker
//! made and the commit of what it wrote. The ClickHouse `query_id` is an
//! attribute of the root span.
//!
//! * The front opens the root span ([`Tracer::root`]) when a statement arrives,
//!   and runs the statement inside [`scope`], so whatever runs for it — the
//!   queues, a worker's lease, an `INSERT`'s commit — opens its span under the
//!   current one with [`child`], without the context passed by hand.
//! * A worker's execution is a span from `Execute` to its terminal frame
//!   (`crate::admission::WorkerLease`). While it is open, the worker's id is
//!   bound to it ([`Tracer::bind_worker`]): `house-cache` serves a worker's
//!   requests on its own tasks, knows only the worker, and finds the span its
//!   reads belong under with [`Tracer::worker_span`].
//! * Spans go to a [`SpanSink`] when they end. [`OtlpHttp`] sends them to an
//!   OpenTelemetry collector over OTLP/HTTP with the JSON encoding
//!   (`POST <endpoint>/v1/traces`), in batches; [`Recorder`] keeps them for
//!   tests. A [`Tracer`] without a sink opens no span at all.
//!
//! Span names: `house.query` (the root), `house.admission`, `house.worker`,
//! `house.cache.read` and `house.commit`. The statement's text is never an
//! attribute: literals can be secrets (the operator log's rule, [`crate::oplog`]).

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A span's place in its trace: what a child names as its parent.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpanContext {
    /// The trace (16 bytes, never all zero).
    pub trace_id: [u8; 16],
    /// The span (8 bytes, never all zero).
    pub span_id: [u8; 8],
}

impl fmt::Debug for SpanContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", hex(&self.trace_id), hex(&self.span_id))
    }
}

impl SpanContext {
    /// A W3C `traceparent` header's trace and parent span
    /// (`00-<32 hex>-<16 hex>-<2 hex>`); `None` when it is not one.
    pub fn from_traceparent(header: &str) -> Option<Self> {
        let mut parts = header.trim().split('-');
        let (version, trace, span, flags) =
            (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
        if version != "00" || parts.next().is_some() || flags.len() != 2 {
            return None;
        }
        let trace_id: [u8; 16] = unhex(trace)?.try_into().ok()?;
        let span_id: [u8; 8] = unhex(span)?.try_into().ok()?;
        (trace_id != [0; 16] && span_id != [0; 8]).then_some(Self { trace_id, span_id })
    }
}

/// An attribute's value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Text(String),
    Int(i64),
    Bool(bool),
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Self::Text(v.to_string())
    }
}

impl From<String> for Value {
    fn from(v: String) -> Self {
        Self::Text(v)
    }
}

impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Self::Int(v)
    }
}

impl From<u64> for Value {
    fn from(v: u64) -> Self {
        Self::Int(i64::try_from(v).unwrap_or(i64::MAX))
    }
}

impl From<i32> for Value {
    fn from(v: i32) -> Self {
        Self::Int(i64::from(v))
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}

/// A span that ended.
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub context: SpanContext,
    /// The parent span, in the same trace; `None` for a root.
    pub parent: Option<[u8; 8]>,
    pub name: &'static str,
    pub start: SystemTime,
    pub end: SystemTime,
    pub attributes: Vec<(&'static str, Value)>,
    /// Whether it ended in an error.
    pub error: bool,
}

impl Span {
    /// An attribute's value.
    pub fn attribute(&self, key: &str) -> Option<&Value> {
        self.attributes
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v)
    }
}

/// Where ended spans go.
pub trait SpanSink: Send + Sync + fmt::Debug {
    fn export(&self, span: Span);
}

/// Keeps every span (tests).
#[derive(Debug, Default)]
pub struct Recorder(Mutex<Vec<Span>>);

impl Recorder {
    /// Every span so far, in the order they ended.
    pub fn spans(&self) -> Vec<Span> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl SpanSink for Recorder {
    fn export(&self, span: Span) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).push(span);
    }
}

/// The front's tracer: the sink, and which span each worker's work belongs
/// under now.
#[derive(Debug, Default)]
pub struct Tracer {
    sink: Option<Arc<dyn SpanSink>>,
    workers: Mutex<HashMap<String, SpanContext>>,
}

impl Tracer {
    /// A tracer that records nothing.
    pub fn off() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// A tracer whose spans go to `sink`.
    pub fn new(sink: Arc<dyn SpanSink>) -> Arc<Self> {
        Arc::new(Self {
            sink: Some(sink),
            workers: Mutex::new(HashMap::new()),
        })
    }

    /// Whether spans are recorded at all.
    pub fn is_on(&self) -> bool {
        self.sink.is_some()
    }

    /// Opens a statement's root span: a new trace, or the caller's when its
    /// request carried a `traceparent`.
    pub fn root(self: &Arc<Self>, parent: Option<SpanContext>) -> Option<OpenSpan> {
        self.sink.as_ref()?;
        let (trace_id, parent) = match parent {
            Some(p) => (p.trace_id, Some(p.span_id)),
            None => (random_id::<16>(), None),
        };
        Some(OpenSpan::new(
            Arc::clone(self),
            trace_id,
            parent,
            "house.query",
        ))
    }

    /// Opens a span under `parent`.
    pub fn child(self: &Arc<Self>, parent: SpanContext, name: &'static str) -> Option<OpenSpan> {
        self.sink.as_ref()?;
        Some(OpenSpan::new(
            Arc::clone(self),
            parent.trace_id,
            Some(parent.span_id),
            name,
        ))
    }

    /// Records a span that already ended (`house-cache`'s reads, measured on
    /// their own task).
    pub fn record(
        &self,
        parent: SpanContext,
        name: &'static str,
        start: SystemTime,
        end: SystemTime,
        attributes: Vec<(&'static str, Value)>,
        error: bool,
    ) {
        if let Some(sink) = &self.sink {
            sink.export(Span {
                context: SpanContext {
                    trace_id: parent.trace_id,
                    span_id: random_id::<8>(),
                },
                parent: Some(parent.span_id),
                name,
                start,
                end,
                attributes,
                error,
            });
        }
    }

    /// From now until [`Tracer::unbind_worker`], `worker`'s requests belong
    /// under `span`.
    pub fn bind_worker(&self, worker: &str, span: SpanContext) {
        if self.sink.is_some() {
            self.lock().insert(worker.to_string(), span);
        }
    }

    /// `worker` runs nothing traced any more.
    pub fn unbind_worker(&self, worker: &str) {
        if self.sink.is_some() {
            self.lock().remove(worker);
        }
    }

    /// The span `worker`'s requests belong under now, if any.
    pub fn worker_span(&self, worker: &str) -> Option<SpanContext> {
        self.sink.as_ref()?;
        self.lock().get(worker).copied()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, SpanContext>> {
        self.workers.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn export(&self, span: Span) {
        if let Some(sink) = &self.sink {
            sink.export(span);
        }
    }
}

/// A span not yet ended. It ends when [`OpenSpan::end`] is called or it is
/// dropped.
#[derive(Debug)]
pub struct OpenSpan {
    tracer: Arc<Tracer>,
    span: Option<Span>,
}

impl OpenSpan {
    fn new(
        tracer: Arc<Tracer>,
        trace_id: [u8; 16],
        parent: Option<[u8; 8]>,
        name: &'static str,
    ) -> Self {
        let now = SystemTime::now();
        Self {
            tracer,
            span: Some(Span {
                context: SpanContext {
                    trace_id,
                    span_id: random_id::<8>(),
                },
                parent,
                name,
                start: now,
                end: now,
                attributes: Vec::new(),
                error: false,
            }),
        }
    }

    /// Its context, for children.
    pub fn context(&self) -> SpanContext {
        self.span
            .as_ref()
            .map(|s| s.context)
            .unwrap_or(SpanContext {
                trace_id: [0; 16],
                span_id: [0; 8],
            })
    }

    /// Its tracer.
    pub fn tracer(&self) -> &Arc<Tracer> {
        &self.tracer
    }

    /// Sets an attribute (a later value replaces an earlier one).
    pub fn set(&mut self, key: &'static str, value: impl Into<Value>) {
        if let Some(span) = &mut self.span {
            let value = value.into();
            match span.attributes.iter_mut().find(|(k, _)| *k == key) {
                Some(found) => found.1 = value,
                None => span.attributes.push((key, value)),
            }
        }
    }

    /// Marks it failed.
    pub fn fail(&mut self) {
        if let Some(span) = &mut self.span {
            span.error = true;
        }
    }

    /// Ends it now.
    pub fn end(mut self) {
        self.finish();
    }

    fn finish(&mut self) {
        if let Some(mut span) = self.span.take() {
            span.end = SystemTime::now();
            self.tracer.export(span);
        }
    }
}

impl Drop for OpenSpan {
    fn drop(&mut self) {
        self.finish();
    }
}

/// The span a task's work belongs under: its tracer and context.
#[derive(Clone, Debug)]
pub struct Current {
    pub tracer: Arc<Tracer>,
    pub context: SpanContext,
}

tokio::task_local! {
    static CURRENT: Current;
}

/// Runs `future` with `current` as the span its work belongs under; `None`
/// runs it untraced.
pub async fn scope<F: std::future::Future>(current: Option<Current>, future: F) -> F::Output {
    match current {
        Some(current) => CURRENT.scope(current, future).await,
        None => future.await,
    }
}

/// The span the running task's work belongs under, if it is traced.
pub fn current() -> Option<Current> {
    CURRENT.try_with(Clone::clone).ok()
}

/// Opens a span under the running task's current one.
pub fn child(name: &'static str) -> Option<OpenSpan> {
    let current = current()?;
    current.tracer.child(current.context, name)
}

fn random_id<const N: usize>() -> [u8; N] {
    loop {
        let mut out = [0u8; N];
        let mut filled = 0;
        while filled < N {
            let bytes = uuid::Uuid::new_v4().into_bytes();
            let take = (N - filled).min(bytes.len());
            out[filled..filled + take].copy_from_slice(&bytes[..take]);
            filled += take;
        }
        if out.iter().any(|b| *b != 0) {
            return out;
        }
    }
}

/// Lower-case hex.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

fn unix_nanos(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Spans as an OTLP/HTTP JSON `ExportTraceServiceRequest`, from `service`.
pub fn otlp_json(service: &str, spans: &[Span]) -> serde_json::Value {
    use serde_json::json;
    let value = |v: &Value| match v {
        Value::Text(s) => json!({ "stringValue": s }),
        // OTLP's JSON encodes 64-bit integers as strings.
        Value::Int(i) => json!({ "intValue": i.to_string() }),
        Value::Bool(b) => json!({ "boolValue": b }),
    };
    let spans: Vec<serde_json::Value> = spans
        .iter()
        .map(|s| {
            let mut out = json!({
                "traceId": hex(&s.context.trace_id),
                "spanId": hex(&s.context.span_id),
                "name": s.name,
                // SPAN_KIND_SERVER for the root, SPAN_KIND_INTERNAL otherwise.
                "kind": if s.parent.is_none() { 2 } else { 1 },
                "startTimeUnixNano": unix_nanos(s.start).to_string(),
                "endTimeUnixNano": unix_nanos(s.end).to_string(),
                "attributes": s.attributes.iter().map(|(k, v)| json!({
                    "key": k,
                    "value": value(v),
                })).collect::<Vec<_>>(),
                // STATUS_CODE_ERROR or STATUS_CODE_UNSET.
                "status": { "code": if s.error { 2 } else { 0 } },
            });
            if let Some(parent) = s.parent {
                out["parentSpanId"] = json!(hex(&parent));
            }
            out
        })
        .collect();
    json!({
        "resourceSpans": [{
            "resource": { "attributes": [
                { "key": "service.name", "value": { "stringValue": service } }
            ]},
            "scopeSpans": [{
                "scope": { "name": "loams-house" },
                "spans": spans,
            }],
        }],
    })
}

/// The most spans one [`OtlpHttp`] batch carries.
pub const OTLP_BATCH: usize = 512;
/// The most spans [`OtlpHttp`] holds while its collector is slow or away;
/// more are dropped (and counted).
pub const OTLP_QUEUE: usize = 8192;
/// How often [`OtlpHttp`] sends what it holds.
pub const OTLP_INTERVAL: Duration = Duration::from_secs(2);

/// Sends spans to an OpenTelemetry collector: OTLP/HTTP, JSON encoding,
/// `POST <endpoint>/v1/traces`, batched every [`OTLP_INTERVAL`] or
/// [`OTLP_BATCH`] spans. A batch the collector refuses or does not take in
/// time is dropped: tracing never holds a statement up.
#[derive(Debug)]
pub struct OtlpHttp {
    queue: Arc<Mutex<Vec<Span>>>,
    wake: Arc<tokio::sync::Notify>,
    dropped: Arc<std::sync::atomic::AtomicU64>,
}

impl OtlpHttp {
    /// Starts the sender on the current runtime. `endpoint` is the collector's
    /// base URL (`http://otel-collector:4318`).
    pub fn start(endpoint: &str, service: &str) -> Result<Arc<Self>, String> {
        let url = format!("{}/v1/traces", endpoint.trim_end_matches('/'));
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(format!(
                "the OTLP endpoint {endpoint:?} is not an http:// or https:// URL"
            ));
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|err| format!("the OTLP client: {err}"))?;
        let queue: Arc<Mutex<Vec<Span>>> = Arc::default();
        let wake = Arc::new(tokio::sync::Notify::new());
        let dropped = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let this = Arc::new(Self {
            queue: Arc::clone(&queue),
            wake: Arc::clone(&wake),
            dropped,
        });
        let service = service.to_string();
        tokio::spawn(async move {
            loop {
                let _ = tokio::time::timeout(OTLP_INTERVAL, wake.notified()).await;
                loop {
                    let batch: Vec<Span> = {
                        let mut queue = queue.lock().unwrap_or_else(|e| e.into_inner());
                        let n = queue.len().min(OTLP_BATCH);
                        queue.drain(..n).collect()
                    };
                    if batch.is_empty() {
                        break;
                    }
                    let body = otlp_json(&service, &batch).to_string();
                    let sent = client
                        .post(&url)
                        .header("Content-Type", "application/json")
                        .body(body)
                        .send()
                        .await;
                    if let Err(err) = sent {
                        eprintln!(
                            "loams-house: {} spans not sent to {url}: {err}",
                            batch.len()
                        );
                        break;
                    }
                }
            }
        });
        Ok(this)
    }

    /// Spans dropped because the queue was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl SpanSink for OtlpHttp {
    fn export(&self, span: Span) {
        let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        if queue.len() >= OTLP_QUEUE {
            self.dropped
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return;
        }
        queue.push(span);
        if queue.len() >= OTLP_BATCH {
            self.wake.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traceparent_is_read() {
        let ctx = SpanContext::from_traceparent(
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        )
        .expect("a traceparent");
        assert_eq!(hex(&ctx.trace_id), "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(hex(&ctx.span_id), "00f067aa0ba902b7");
        for bad in [
            "",
            "01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            "00-00000000000000000000000000000000-00f067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01",
            "00-4bf92f3577b34da6a3ce929d0e0e473-00f067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-x",
        ] {
            assert!(SpanContext::from_traceparent(bad).is_none(), "{bad}");
        }
    }

    #[tokio::test]
    async fn spans_nest_through_the_task_scope() {
        let recorder = Arc::new(Recorder::default());
        let tracer = Tracer::new(recorder.clone());
        let mut root = tracer.root(None).expect("on");
        root.set("loams.query_id", "q1");
        let current = Current {
            tracer: Arc::clone(&tracer),
            context: root.context(),
        };
        scope(Some(current), async {
            let mut admission = child("house.admission").expect("a child");
            admission.set("loams.namespace", 2u64);
            admission.end();
        })
        .await;
        assert!(child("house.admission").is_none(), "outside the scope");
        let root_ctx = root.context();
        root.end();
        let spans = recorder.spans();
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].name, "house.admission");
        assert_eq!(spans[0].parent, Some(root_ctx.span_id));
        assert_eq!(spans[0].context.trace_id, root_ctx.trace_id);
        assert_eq!(
            spans[1].attribute("loams.query_id"),
            Some(&Value::from("q1"))
        );
        assert!(spans[1].parent.is_none());
    }

    #[test]
    fn workers_bind_to_spans() {
        let tracer = Tracer::new(Arc::new(Recorder::default()));
        let span = SpanContext {
            trace_id: [1; 16],
            span_id: [2; 8],
        };
        tracer.bind_worker("w1", span);
        assert_eq!(tracer.worker_span("w1"), Some(span));
        tracer.unbind_worker("w1");
        assert_eq!(tracer.worker_span("w1"), None);
        // A tracer that records nothing binds nothing.
        let off = Tracer::off();
        off.bind_worker("w1", span);
        assert_eq!(off.worker_span("w1"), None);
        assert!(off.root(None).is_none());
    }

    #[test]
    fn otlp_json_shape() {
        let span = Span {
            context: SpanContext {
                trace_id: [0xab; 16],
                span_id: [0xcd; 8],
            },
            parent: Some([0xef; 8]),
            name: "house.worker",
            start: UNIX_EPOCH + Duration::from_secs(1),
            end: UNIX_EPOCH + Duration::from_secs(2),
            attributes: vec![("loams.rows", Value::Int(7))],
            error: true,
        };
        let json = otlp_json("loams-house", &[span]);
        let s = &json["resourceSpans"][0]["scopeSpans"][0]["spans"][0];
        assert_eq!(s["traceId"], "abababababababababababababababab");
        assert_eq!(s["parentSpanId"], "efefefefefefefef");
        assert_eq!(s["startTimeUnixNano"], "1000000000");
        assert_eq!(s["attributes"][0]["value"]["intValue"], "7");
        assert_eq!(s["status"]["code"], 2);
    }
}
