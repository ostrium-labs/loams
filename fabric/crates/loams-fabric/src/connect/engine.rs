//! The engine as `connect` sees it (D1821, CC1 ruling R1.1): `loams.stream.v1`
//! over Connect JSON on the engine's connect socket (`unix:///…`) or, for
//! development, its main port (`http://host:port`), with a Loams API key.
//!
//! - [`EngineWriter`] is the SDK's `FabricWriter` over `ProduceCloudEvents`.
//!   The engine appends each event once per `source` + `id` within its dedup
//!   window, so a source that re-reads after a crash writes no duplicate.
//! - [`EngineReader`] is the SDK's `FabricReader` over `FetchCloudEvents`,
//!   one cursor per partition, its committed position kept in the bucket.
//! - [`EngineDeadLetters`] sends dead letters to a stream of their own.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use http_body_util::{BodyExt as _, Full};
use hyper_util::rt::TokioIo;
use loams_connector_sdk::{
    Budget, CloudEvent, DeadLetter, DeadLetterQueue, ErrorClass, EventBatch, FabricAck,
    FabricError, FabricReader, FabricWriter, Position, Secret, SourceBatch,
};
use serde_json::{Value, json};

use super::state::PositionDoc;

/// The engine's address: its connect socket or a TCP port.
#[derive(Clone, PartialEq, Eq)]
pub enum EngineUrl {
    Unix(PathBuf),
    Tcp { authority: String },
}

impl EngineUrl {
    /// `unix:///run/loams-connect/engine.sock` or `http://host:port`.
    pub fn parse(text: &str) -> Result<Self, String> {
        if let Some(path) = text.strip_prefix("unix://") {
            if !path.starts_with('/') {
                return Err(format!(
                    "--engine {text:?}: a unix URL names an absolute path (unix:///run/…)"
                ));
            }
            return Ok(Self::Unix(PathBuf::from(path)));
        }
        if let Some(rest) = text.strip_prefix("http://") {
            let authority = rest.trim_end_matches('/');
            if authority.is_empty() || authority.contains('/') {
                return Err(format!("--engine {text:?}: expected http://host:port"));
            }
            return Ok(Self::Tcp {
                authority: authority.to_owned(),
            });
        }
        Err(format!(
            "--engine {text:?}: expected unix:///path/to/engine.sock or http://host:port"
        ))
    }
}

impl fmt::Display for EngineUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unix(p) => write!(f, "unix://{}", p.display()),
            Self::Tcp { authority } => write!(f, "http://{authority}"),
        }
    }
}

impl fmt::Debug for EngineUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// How long one engine call may take beyond its long poll.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// The engine's Connect API with the role's API key. Cheap to clone.
#[derive(Clone)]
pub struct EngineClient {
    url: EngineUrl,
    key: Option<Arc<Secret>>,
}

impl fmt::Debug for EngineClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EngineClient")
            .field("url", &self.url)
            .field("key", &self.key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// A refused or failed engine call. Never carries the key.
#[derive(Debug, Clone)]
pub struct EngineError {
    /// The Connect error code (`unavailable`, `not_found`, …) or `transport`.
    pub code: String,
    pub message: String,
    pub retry_after: Option<Duration>,
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "engine {}: {}", self.code, self.message)
    }
}

impl EngineError {
    fn transport(message: impl Into<String>) -> Self {
        Self {
            code: "transport".to_owned(),
            message: message.into(),
            retry_after: None,
        }
    }

    /// The SDK's error class for this failure: the engine down or busy is
    /// retryable, a refused credential stops the instance, a bad request is
    /// the batch's fault.
    pub fn class(&self) -> ErrorClass {
        match self.code.as_str() {
            "unauthenticated" | "permission_denied" => ErrorClass::Auth,
            "resource_exhausted" => ErrorClass::Throttled {
                after: self.retry_after.unwrap_or(Duration::from_secs(1)),
            },
            "invalid_argument" | "failed_precondition" | "out_of_range" => ErrorClass::Poison,
            "unimplemented" => ErrorClass::Fatal,
            _ => ErrorClass::Retryable {
                after: self.retry_after,
            },
        }
    }

    pub fn into_fabric(self) -> FabricError {
        FabricError::new(self.class(), self.to_string())
    }
}

impl EngineClient {
    pub fn new(url: EngineUrl, key: Option<Secret>) -> Self {
        Self {
            url,
            key: key.map(Arc::new),
        }
    }

    pub fn url(&self) -> &EngineUrl {
        &self.url
    }

    /// One `loams.stream.v1.StreamService` call, Connect JSON in and out.
    pub async fn call(&self, method: &str, body: &Value) -> Result<Value, EngineError> {
        let path = format!("/loams.stream.v1.StreamService/{method}");
        tokio::time::timeout(
            CALL_TIMEOUT + Duration::from_secs(30),
            self.post(&path, body),
        )
        .await
        .map_err(|_| EngineError::transport(format!("{method} timed out")))?
    }

    async fn post(&self, path: &str, body: &Value) -> Result<Value, EngineError> {
        let mut req = http::Request::post(path)
            .header(http::header::CONTENT_TYPE, "application/json")
            .header("connect-protocol-version", "1");
        req = match &self.url {
            EngineUrl::Unix(_) => req.header(http::header::HOST, "loams"),
            EngineUrl::Tcp { authority } => req.header(http::header::HOST, authority.as_str()),
        };
        if let Some(key) = &self.key {
            let mut value = Vec::with_capacity(7 + key.expose().len());
            value.extend_from_slice(b"Bearer ");
            value.extend_from_slice(key.expose());
            let mut header = http::HeaderValue::from_bytes(&value)
                .map_err(|_| EngineError::transport("the engine key is not a valid header"))?;
            header.set_sensitive(true);
            req = req.header(http::header::AUTHORIZATION, header);
        }
        let req = req
            .body(Full::new(Bytes::from(body.to_string())))
            .map_err(|e| EngineError::transport(e.to_string()))?;
        let resp = match &self.url {
            EngineUrl::Unix(path) => {
                let io = tokio::net::UnixStream::connect(path).await.map_err(|e| {
                    EngineError::transport(format!("connecting to {}: {e}", path.display()))
                })?;
                send(TokioIo::new(io), req).await?
            }
            EngineUrl::Tcp { authority } => {
                let io = tokio::net::TcpStream::connect(authority.as_str())
                    .await
                    .map_err(|e| {
                        EngineError::transport(format!("connecting to {authority}: {e}"))
                    })?;
                send(TokioIo::new(io), req).await?
            }
        };
        let (parts, body) = resp.into_parts();
        let bytes = body
            .collect()
            .await
            .map_err(|e| EngineError::transport(format!("reading the answer: {e}")))?
            .to_bytes();
        let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        if parts.status.is_success() {
            return Ok(value);
        }
        let retry_after = parts
            .headers
            .get(http::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        let code = value
            .get("code")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| match parts.status.as_u16() {
                401 => "unauthenticated".to_owned(),
                403 => "permission_denied".to_owned(),
                404 => "unimplemented".to_owned(),
                429 => "resource_exhausted".to_owned(),
                s => format!("http_{s}"),
            });
        let message = value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("no message")
            .to_owned();
        Err(EngineError {
            code,
            message,
            retry_after,
        })
    }

    /// The stream, created with `partitions` when missing (the RPC is
    /// idempotent by name); its partition count.
    pub async fn ensure_stream(
        &self,
        namespace: &str,
        stream: &str,
        partitions: u32,
    ) -> Result<u32, EngineError> {
        let v = self
            .call(
                "CreateStream",
                &json!({"namespace": namespace, "name": stream, "partitions": partitions}),
            )
            .await;
        match v {
            Ok(v) => Ok(partitions_of(&v["stream"]).unwrap_or(partitions)),
            Err(e) if e.code == "already_exists" => self.partitions(namespace, stream).await,
            Err(e) => Err(e),
        }
    }

    /// The stream's partition count.
    pub async fn partitions(&self, namespace: &str, stream: &str) -> Result<u32, EngineError> {
        let v = self
            .call(
                "DescribeStream",
                &json!({"namespace": namespace, "stream": stream}),
            )
            .await?;
        partitions_of(&v["stream"])
            .ok_or_else(|| EngineError::transport("DescribeStream answered no partition count"))
    }
}

async fn send<I>(
    io: TokioIo<I>,
    req: http::Request<Full<Bytes>>,
) -> Result<http::Response<hyper::body::Incoming>, EngineError>
where
    I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static,
{
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .map_err(|e| EngineError::transport(format!("HTTP handshake: {e}")))?;
    tokio::spawn(async move {
        let _ = conn.await;
    });
    sender
        .send_request(req)
        .await
        .map_err(|e| EngineError::transport(format!("sending the request: {e}")))
}

fn partitions_of(stream: &Value) -> Option<u32> {
    stream
        .get("partitions")
        .and_then(|p| {
            p.as_u64()
                .or_else(|| p.as_str().and_then(|s| s.parse().ok()))
        })
        .and_then(|p| u32::try_from(p).ok())
}

/// A proto3 JSON `uint64` (a decimal string or a number), absent as 0.
fn u64_of(v: &Value) -> u64 {
    match v {
        Value::String(s) => s.parse().unwrap_or(0),
        Value::Number(n) => n.as_u64().unwrap_or(0),
        _ => 0,
    }
}

// ---------------------------------------------------------- CloudEvents JSON

/// A JSON media type: the data goes in `data` as JSON.
fn is_json(content_type: Option<&str>) -> bool {
    content_type.is_none_or(|ct| {
        let ct = ct
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        ct == "application/json" || ct.ends_with("+json") || ct == "text/json"
    })
}

/// One event in the CloudEvents JSON format.
pub fn event_to_json(ev: &CloudEvent) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("specversion".into(), json!(ev.specversion));
    m.insert("id".into(), json!(ev.id));
    m.insert("source".into(), json!(ev.source));
    m.insert("type".into(), json!(ev.ty));
    if let Some(s) = &ev.subject {
        m.insert("subject".into(), json!(s));
    }
    if let Some(t) = &ev.time {
        m.insert("time".into(), json!(t));
    }
    if let Some(ct) = &ev.datacontenttype {
        m.insert("datacontenttype".into(), json!(ct));
    }
    for (k, v) in &ev.extensions {
        m.entry(k.clone()).or_insert_with(|| json!(v));
    }
    if !ev.data.is_empty() {
        let parsed = is_json(ev.datacontenttype.as_deref())
            .then(|| serde_json::from_slice::<Value>(&ev.data).ok())
            .flatten();
        match parsed {
            Some(v) => {
                m.insert("data".into(), v);
            }
            None => {
                m.insert("data_base64".into(), json!(STANDARD.encode(&ev.data)));
            }
        }
    }
    Value::Object(m)
}

const CORE: &[&str] = &[
    "specversion",
    "id",
    "source",
    "type",
    "subject",
    "time",
    "datacontenttype",
    "data",
    "data_base64",
    "dataschema",
];

/// One event from the CloudEvents JSON format; `None` if it is not one.
pub fn event_from_json(v: &Value) -> Option<CloudEvent> {
    let m = v.as_object()?;
    let s = |k: &str| m.get(k).and_then(Value::as_str).map(str::to_owned);
    let mut ev = CloudEvent::new(s("id")?, s("source")?, s("type")?);
    ev.specversion = s("specversion").unwrap_or_else(|| "1.0".to_owned());
    ev.subject = s("subject");
    ev.time = s("time");
    ev.datacontenttype = s("datacontenttype");
    if let Some(b64) = m.get("data_base64").and_then(Value::as_str) {
        ev.data = Bytes::from(STANDARD.decode(b64).ok()?);
    } else if let Some(d) = m.get("data") {
        ev.data = match d {
            Value::String(text) if !is_json(ev.datacontenttype.as_deref()) => {
                Bytes::from(text.clone())
            }
            other => Bytes::from(serde_json::to_vec(other).ok()?),
        };
        if ev.datacontenttype.is_none() {
            ev.datacontenttype = Some("application/json".to_owned());
        }
    }
    for (k, v) in m {
        if CORE.contains(&k.as_str()) {
            continue;
        }
        let text = match v {
            Value::String(s) => s.clone(),
            Value::Null => continue,
            other => other.to_string(),
        };
        ev.extensions.insert(k.clone(), text);
    }
    if let Some(ds) = s("dataschema") {
        ev.extensions.insert("dataschema".to_owned(), ds);
    }
    Some(ev)
}

/// A batch as `application/cloudevents-batch+json`, base64 for proto3 JSON.
fn json_batch(events: &[CloudEvent]) -> String {
    let arr: Vec<Value> = events.iter().map(event_to_json).collect();
    STANDARD.encode(Value::Array(arr).to_string())
}

// ------------------------------------------------------------------- writer

/// The source side: appends batches to one stream.
#[derive(Debug, Clone)]
pub struct EngineWriter {
    client: EngineClient,
    namespace: String,
    stream: String,
}

impl EngineWriter {
    pub fn new(client: EngineClient, namespace: String, stream: String) -> Self {
        Self {
            client,
            namespace,
            stream,
        }
    }

    async fn produce(&self, events: &[CloudEvent]) -> Result<FabricAck, FabricError> {
        let v = self
            .client
            .call(
                "ProduceCloudEvents",
                &json!({
                    "namespace": self.namespace,
                    "stream": self.stream,
                    "jsonBatch": json_batch(events),
                }),
            )
            .await
            .map_err(EngineError::into_fabric)?;
        let results = v["results"].as_array().cloned().unwrap_or_default();
        if results.len() != events.len() {
            return Err(FabricError::retryable(format!(
                "ProduceCloudEvents answered {} results for {} events",
                results.len(),
                events.len()
            )));
        }
        let mut ack = FabricAck::default();
        let mut in_flight: Option<u64> = None;
        for r in &results {
            match r["status"].as_str().unwrap_or_default() {
                "EVENT_STATUS_APPENDED" => ack.accepted += 1,
                "EVENT_STATUS_DUPLICATE" => ack.duplicates += 1,
                "EVENT_STATUS_IN_FLIGHT" => {
                    let ms = u64_of(&r["retryAfterMs"]).max(50);
                    in_flight = Some(in_flight.map_or(ms, |m| m.max(ms)));
                }
                other => {
                    return Err(FabricError::retryable(format!(
                        "ProduceCloudEvents answered status {other:?}"
                    )));
                }
            }
        }
        if let Some(ms) = in_flight {
            // Another request is appending some of these ids; sending the
            // batch again answers them as duplicates once it is done.
            return Err(FabricError::new(
                ErrorClass::Retryable {
                    after: Some(Duration::from_millis(ms)),
                },
                "events in flight in another request",
            ));
        }
        Ok(ack)
    }
}

#[async_trait]
impl FabricWriter for EngineWriter {
    async fn append(&self, b: &SourceBatch) -> Result<FabricAck, FabricError> {
        match &b.events {
            EventBatch::Events(events) if events.is_empty() => Ok(FabricAck::default()),
            EventBatch::Events(events) => self.produce(events).await,
            EventBatch::Arrow { .. } => Err(FabricError::new(
                ErrorClass::Fatal,
                "connect writes CloudEvents; this source sent an Arrow batch, which the engine's \
                 ProduceCloudEvents does not take",
            )),
        }
    }
}

// ------------------------------------------------------------------- reader

/// Where a sink's reader is: the next offset of each partition.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cursor {
    pub next: BTreeMap<u32, u64>,
}

impl Cursor {
    pub fn to_position(&self) -> Position {
        let opaque = serde_json::to_vec(&self.next).unwrap_or_default();
        let display = self
            .next
            .iter()
            .map(|(p, o)| format!("{p}:{o}"))
            .collect::<Vec<_>>()
            .join(",");
        Position::new(opaque, display)
    }

    pub fn from_position(p: &Position) -> Option<Self> {
        let next: BTreeMap<u32, u64> = serde_json::from_slice(&p.opaque).ok()?;
        Some(Self { next })
    }
}

/// The sink side: reads one stream's partitions round robin.
pub struct EngineReader {
    client: EngineClient,
    namespace: String,
    stream: String,
    partitions: u32,
    cursor: Cursor,
    turn: u32,
    store: Arc<PositionDoc>,
}

impl fmt::Debug for EngineReader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EngineReader")
            .field("stream", &format!("{}/{}", self.namespace, self.stream))
            .field("cursor", &self.cursor)
            .finish_non_exhaustive()
    }
}

/// The largest fetch the engine answers.
const MAX_FETCH_BYTES: usize = 16 << 20;

impl EngineReader {
    /// A reader from the committed position in `store` (or the start of
    /// each partition).
    pub async fn open(
        client: EngineClient,
        namespace: String,
        stream: String,
        store: Arc<PositionDoc>,
    ) -> Result<Self, FabricError> {
        let partitions = client
            .partitions(&namespace, &stream)
            .await
            .map_err(EngineError::into_fabric)?;
        let mut cursor = store
            .load()
            .await
            .map_err(|e| FabricError::retryable(e.to_string()))?
            .and_then(|p| Cursor::from_position(&p))
            .unwrap_or_default();
        for p in 0..partitions {
            cursor.next.entry(p).or_insert(0);
        }
        Ok(Self {
            client,
            namespace,
            stream,
            partitions,
            cursor,
            turn: 0,
            store,
        })
    }

    async fn fetch(
        &self,
        partition: u32,
        offset: u64,
        budget: Budget,
        wait_ms: u32,
    ) -> Result<(Vec<CloudEvent>, u64), FabricError> {
        let max_bytes = budget.max_bytes.clamp(1, MAX_FETCH_BYTES);
        let v = self
            .client
            .call(
                "FetchCloudEvents",
                &json!({
                    "namespace": self.namespace,
                    "stream": self.stream,
                    "partition": partition,
                    "offset": offset.to_string(),
                    "maxBytes": max_bytes,
                    "maxWaitMs": wait_ms,
                }),
            )
            .await
            .map_err(EngineError::into_fabric)?;
        let next = u64_of(&v["nextOffset"]);
        let count = u64_of(&v["count"]);
        if count == 0 {
            return Ok((Vec::new(), next.max(offset)));
        }
        let raw = STANDARD
            .decode(v["jsonBatch"].as_str().unwrap_or_default())
            .map_err(|e| FabricError::retryable(format!("FetchCloudEvents batch: {e}")))?;
        let arr: Vec<Value> = serde_json::from_slice(&raw)
            .map_err(|e| FabricError::retryable(format!("FetchCloudEvents batch: {e}")))?;
        let mut events = Vec::with_capacity(arr.len());
        for v in &arr {
            match event_from_json(v) {
                Some(ev) => events.push(ev),
                None => {
                    return Err(FabricError::new(
                        ErrorClass::Fatal,
                        "FetchCloudEvents answered something that is not a CloudEvent",
                    ));
                }
            }
        }
        // Within the budget: keep a prefix. Offsets within a partition are
        // one per record, so the prefix's end is `offset + kept` (the engine
        // answers records from `offset` on, in order).
        let mut kept = events.len().min(budget.max_rows.max(1));
        let mut bytes: usize = events[..kept].iter().map(CloudEvent::size_bytes).sum();
        while kept > 1 && bytes > budget.max_bytes {
            kept -= 1;
            bytes -= events[kept].size_bytes();
        }
        if kept < events.len() {
            events.truncate(kept);
            return Ok((events, offset + kept as u64));
        }
        Ok((events, next))
    }

    /// Persist the committed position now (the supervisor calls it at stop).
    pub async fn flush(&self) {
        self.store.flush().await;
    }
}

#[async_trait]
impl FabricReader for EngineReader {
    async fn poll(
        &mut self,
        budget: Budget,
    ) -> Result<Option<(EventBatch, Position)>, FabricError> {
        let n = self.partitions.max(1);
        // One partition: a short long poll instead of the pipeline's linger.
        let wait_ms = if n == 1 { 500 } else { 0 };
        for i in 0..n {
            let p = (self.turn + i) % n;
            let offset = self.cursor.next.get(&p).copied().unwrap_or(0);
            let (events, next) = self.fetch(p, offset, budget, wait_ms).await?;
            if events.is_empty() {
                if next > offset {
                    // Records trimmed by retention: skip to the log's start.
                    self.cursor.next.insert(p, next);
                }
                continue;
            }
            self.cursor.next.insert(p, next);
            self.turn = (p + 1) % n;
            return Ok(Some((
                EventBatch::Events(events),
                self.cursor.to_position(),
            )));
        }
        Ok(None)
    }

    async fn commit(&mut self, upto: &Position) -> Result<(), FabricError> {
        self.store
            .commit(upto.clone())
            .await
            .map_err(|e| FabricError::retryable(e.to_string()))
    }

    async fn seek(&mut self, to: &Position) -> Result<(), FabricError> {
        let cursor = Cursor::from_position(to).ok_or_else(|| {
            FabricError::new(ErrorClass::Fatal, "not a position this reader returned")
        })?;
        self.cursor = cursor;
        for p in 0..self.partitions {
            self.cursor.next.entry(p).or_insert(0);
        }
        Ok(())
    }
}

// ------------------------------------------------------------- dead letters

/// Dead letters as CloudEvents on a stream of their own.
#[derive(Debug, Clone)]
pub struct EngineDeadLetters {
    writer: EngineWriter,
}

impl EngineDeadLetters {
    pub fn new(writer: EngineWriter) -> Self {
        Self { writer }
    }
}

#[async_trait]
impl DeadLetterQueue for EngineDeadLetters {
    async fn send(&self, dead: DeadLetter) -> Result<(), FabricError> {
        let event = dead.event.as_ref().map(event_to_json);
        let id = match &dead.event {
            Some(ev) => format!("{}#{}", ev.source, ev.id),
            None => format!(
                "{}/{}/{}",
                dead.split.as_ref().map_or("-", |s| s.as_str()),
                dead.error.len(),
                uuid::Uuid::new_v4()
            ),
        };
        let ev = CloudEvent::new(
            id,
            format!("/loams/connect/{}", dead.instance.name),
            "io.loams.connect.dead_letter",
        )
        .with_data(
            "application/json",
            json!({
                "instance": dead.instance.name,
                "split": dead.split.as_ref().map(|s| s.as_str()),
                "class": format!("{:?}", dead.class),
                "error": dead.error,
                "event": event,
            })
            .to_string(),
        );
        self.writer.produce(&[ev]).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_parse() {
        assert_eq!(
            EngineUrl::parse("unix:///run/loams-connect/engine.sock").ok(),
            Some(EngineUrl::Unix("/run/loams-connect/engine.sock".into()))
        );
        assert_eq!(
            EngineUrl::parse("http://127.0.0.1:8080/").ok(),
            Some(EngineUrl::Tcp {
                authority: "127.0.0.1:8080".into()
            })
        );
        for bad in ["unix://rel.sock", "https://x:1", "http://", "x"] {
            assert!(EngineUrl::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn events_round_trip_through_json() {
        let ev = CloudEvent::new("o/0/1", "/kafka/orders", "t")
            .with_data("application/json", r#"{"n":1}"#)
            .with_extension("loamsop", "c");
        let back = event_from_json(&event_to_json(&ev)).expect("an event");
        assert_eq!(back, ev);
        let bin =
            CloudEvent::new("b", "/s", "t").with_data("application/octet-stream", vec![0u8, 255]);
        let back = event_from_json(&event_to_json(&bin)).expect("an event");
        assert_eq!(back.data, bin.data);
    }

    #[test]
    fn cursor_round_trips() {
        let c = Cursor {
            next: [(0, 12), (1, 5)].into(),
        };
        let p = c.to_position();
        assert_eq!(p.display, "0:12,1:5");
        assert_eq!(Cursor::from_position(&p), Some(c));
    }

    #[test]
    fn debug_never_shows_the_key() {
        let c = EngineClient::new(
            EngineUrl::parse("http://h:1").unwrap_or(EngineUrl::Unix("/x".into())),
            Some(Secret::new("lk_secret_value")),
        );
        assert!(!format!("{c:?}").contains("lk_secret_value"));
    }
}
