//! The interim adapter (D1591): [`HttpDurability`] speaks Resonate's
//! protocol `2026-04-01` over HTTP itself, until FW3's `resonate-client`
//! publishes a pinnable revision. Then a pin-bump PR replaces this module
//! with that client and deletes it; `tests/profile.rs` must pass before and
//! after. Nothing in this crate but the root re-export names this module.
//!
//! - Requests: `POST {base}/` of one envelope
//!   `{"kind", "head": {"corrId", "version"}, "data"}`; the answer is an
//!   envelope whose `head.status` is also the HTTP status. A connection
//!   failure, a timeout, a `503` (the server's `Unavailable`) or a proxy's
//!   `502`/`504` is retried with full-jitter exponential backoff, bounded by
//!   attempts and a deadline ([`RetryPolicy`]); after that the call is
//!   [`DurabilityError::Unavailable`] and may have been applied.
//! - Deliveries: `GET {base}/poll/{group}/{pid}`, server-sent events, one
//!   JSON message per event (`execute` or `unblock`). A dropped stream is
//!   reopened with backoff and a [`Delivery::Gap`] is sent when it is back.
//! - Auth: the token hook's value goes in `Authorization: Bearer …` on both
//!   (the listener's auth, D111).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::port::{
    Claim, CreatePromise, DelaySupport, Delivery, Durability, DurabilityError, PromiseStatus,
    PromiseView, SettleAs, b64, poll_address, unb64, valid_group, valid_pid,
};

/// The protocol version this adapter speaks.
pub const PROTOCOL_VERSION: &str = "2026-04-01";

/// Where a bearer token comes from, read before every request.
pub type TokenFn = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// Retries for a call that got no answer (mirrors `resonate-client`'s).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Attempts in total, including the first.
    pub max_attempts: u32,
    /// The first backoff's upper bound.
    pub base: Duration,
    /// No backoff is longer than this.
    pub cap: Duration,
    /// The whole call ends by this.
    pub deadline: Duration,
    /// Each attempt's own timeout.
    pub attempt_timeout: Duration,
}

impl Default for RetryPolicy {
    /// Six attempts, 50 ms doubling to 2 s, within 30 s; 10 s per attempt.
    fn default() -> Self {
        Self {
            max_attempts: 6,
            base: Duration::from_millis(50),
            cap: Duration::from_secs(2),
            deadline: Duration::from_secs(30),
            attempt_timeout: Duration::from_secs(10),
        }
    }
}

/// xorshift: only has to keep clients that failed together apart.
#[derive(Debug)]
struct Jitter(AtomicU64);

impl Jitter {
    fn new() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9e37_79b9_7f4a_7c15);
        Self(AtomicU64::new(seed | 1))
    }

    fn next(&self) -> u64 {
        let mut x = self.0.load(Ordering::Relaxed);
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0.store(x, Ordering::Relaxed);
        x
    }

    /// Full jitter: uniform in `[0, min(cap, base * 2^(attempt-1))]`.
    fn backoff(&self, base: Duration, cap: Duration, attempt: u32) -> Duration {
        let exp = base.saturating_mul(
            1u32.checked_shl(attempt.saturating_sub(1))
                .unwrap_or(u32::MAX),
        );
        let ceiling = exp.min(cap).as_millis() as u64;
        if ceiling == 0 {
            return Duration::ZERO;
        }
        Duration::from_millis(self.next() % (ceiling + 1))
    }
}

type Subs = Arc<Mutex<BTreeMap<String, mpsc::Sender<Delivery>>>>;

/// The interim HTTP adapter (module docs).
pub struct HttpDurability {
    base: String,
    pid: String,
    token: Option<TokenFn>,
    http: reqwest::Client,
    retry: RetryPolicy,
    corr: AtomicU64,
    jitter: Arc<Jitter>,
    subs: Subs,
    delay: DelaySupport,
}

impl std::fmt::Debug for HttpDurability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpDurability")
            .field("base", &self.base)
            .field("pid", &self.pid)
            .field("token", &self.token.as_ref().map(|_| "<hook>"))
            .field("retry", &self.retry)
            .finish_non_exhaustive()
    }
}

enum Attempt {
    Answer(Value),
    Retry(String),
    Fail(DurabilityError),
}

fn lock(subs: &Subs) -> std::sync::MutexGuard<'_, BTreeMap<String, mpsc::Sender<Delivery>>> {
    subs.lock().unwrap_or_else(|e| e.into_inner())
}

fn record_view(v: &Value) -> Result<PromiseView, DurabilityError> {
    let bad = |m: &str| DurabilityError::Protocol(format!("promise record: {m}: {v}"));
    let id = v
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("no id"))?;
    let status = v
        .get("state")
        .and_then(Value::as_str)
        .and_then(PromiseStatus::parse)
        .ok_or_else(|| bad("no state"))?;
    let param = match v.pointer("/param/data").and_then(Value::as_str) {
        Some(s) => Some(unb64(s)?),
        None => None,
    };
    let tags = v
        .get("tags")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();
    Ok(PromiseView {
        id: id.to_string(),
        status,
        param,
        tags,
        timeout_at_ms: v.get("timeoutAt").and_then(Value::as_i64).unwrap_or(0),
        settled_at_ms: v.get("settledAt").and_then(Value::as_i64),
    })
}

fn promise_of(data: &Value) -> Result<PromiseView, DurabilityError> {
    record_view(
        data.get("promise")
            .ok_or_else(|| DurabilityError::Protocol(format!("no promise in {data}")))?,
    )
}

/// The origin of an ID, as Resonate computes it: everything before the
/// first `:`. A heartbeat names tasks of one origin only.
fn origin(id: &str) -> &str {
    id.split_once(':').map(|(o, _)| o).unwrap_or(id)
}

/// One poll message as a delivery; `None` for a kind this port ignores.
pub(crate) fn parse_message(msg: &str) -> Option<Delivery> {
    let v: Value = serde_json::from_str(msg).ok()?;
    match v.get("kind")?.as_str()? {
        "execute" => {
            let task = v.pointer("/data/task")?;
            let id = task.get("id")?.as_str()?.to_string();
            Some(Delivery::TaskReady {
                task_id: id.clone(),
                version: task.get("version")?.as_i64()?,
                promise_id: id,
            })
        }
        "unblock" => {
            let p = v.pointer("/data/promise")?;
            Some(Delivery::Settled {
                promise_id: p.get("id")?.as_str()?.to_string(),
                status: PromiseStatus::parse(p.get("state")?.as_str()?)?,
            })
        }
        _ => None,
    }
}

/// Server-sent events, fed bytes, yielding each event's data.
#[derive(Debug, Default)]
struct SseParser {
    buf: String,
    data: Vec<String>,
}

impl SseParser {
    fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buf.push_str(&String::from_utf8_lossy(bytes));
        let mut out = Vec::new();
        while let Some(nl) = self.buf.find('\n') {
            let line: String = self.buf.drain(..=nl).collect();
            let line = line.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                if !self.data.is_empty() {
                    out.push(self.data.join("\n"));
                    self.data.clear();
                }
            } else if let Some(rest) = line.strip_prefix("data:") {
                self.data
                    .push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
            }
            // Comments (`: ping`, the keepalive) and other fields are ignored.
        }
        out
    }
}

impl HttpDurability {
    /// An adapter for the server at `base_url` (its protocol endpoint, e.g.
    /// `http://127.0.0.1:8001` or the engine's `…/durable`), acting as `pid`
    /// (also the poll connection's ID). https uses the process's rustls
    /// provider, installing ring's if there is none.
    pub fn new(
        base_url: impl Into<String>,
        pid: impl Into<String>,
        token: Option<TokenFn>,
    ) -> Result<Self, DurabilityError> {
        let pid = pid.into();
        if !valid_pid(&pid) {
            return Err(DurabilityError::Rejected {
                status: 400,
                message: format!("invalid pid {pid:?}"),
            });
        }
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            // Another thread may install one first; either way one is set.
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| DurabilityError::Protocol(format!("cannot build the HTTP client: {e}")))?;
        Ok(Self {
            base: base_url.into().trim_end_matches('/').to_string(),
            pid,
            token,
            http,
            retry: RetryPolicy::default(),
            corr: AtomicU64::new(1),
            jitter: Arc::new(Jitter::new()),
            subs: Arc::default(),
            delay: DelaySupport::Defer,
        })
    }

    /// Use `policy` for every call and for opening a subscription.
    pub fn with_retry(mut self, policy: RetryPolicy) -> Self {
        self.retry = policy;
        self
    }

    /// Whether the server honours `resonate:delay`. [`DelaySupport::Defer`]
    /// by default: the SQL stores (the light compose's SQLite) ignore it at
    /// 31ffb201, and deferring in the relay is right on every store.
    pub fn with_delay_support(mut self, delay: DelaySupport) -> Self {
        self.delay = delay;
        self
    }

    /// The process ID.
    pub fn pid(&self) -> &str {
        &self.pid
    }

    fn bearer(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self.token.as_ref().and_then(|t| t()) {
            Some(t) => rb.bearer_auth(t),
            None => rb,
        }
    }

    async fn attempt(&self, kind: &str, corr_id: &str, data: &Value) -> Attempt {
        let body = json!({
            "kind": kind,
            "head": { "corrId": corr_id, "version": PROTOCOL_VERSION },
            "data": data,
        });
        let rb = self
            .http
            .post(format!("{}/", self.base))
            .timeout(self.retry.attempt_timeout)
            .json(&body);
        let resp = match self.bearer(rb).send().await {
            Ok(r) => r,
            Err(e) => return Attempt::Retry(e.to_string()),
        };
        let http_status = resp.status().as_u16();
        let bytes = match resp.bytes().await {
            Ok(b) => b,
            Err(e) => return Attempt::Retry(e.to_string()),
        };
        if matches!(http_status, 502..=504) {
            return Attempt::Retry(format!(
                "HTTP {http_status}: {}",
                String::from_utf8_lossy(&bytes)
            ));
        }
        let env: Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(e) => {
                return Attempt::Fail(DurabilityError::Protocol(format!(
                    "{kind}: HTTP {http_status}, not an envelope: {e}"
                )));
            }
        };
        let status = env
            .pointer("/head/status")
            .and_then(Value::as_i64)
            .unwrap_or(i64::from(http_status));
        if env.get("kind").and_then(Value::as_str) != Some(kind)
            || env.pointer("/head/corrId").and_then(Value::as_str) != Some(corr_id)
        {
            return Attempt::Fail(DurabilityError::Protocol(format!(
                "{kind} ({corr_id}): answer for another request: {env}"
            )));
        }
        let data = env.get("data").cloned().unwrap_or(Value::Null);
        let message = || {
            data.as_str()
                .map(str::to_string)
                .unwrap_or_else(|| data.to_string())
        };
        match status {
            200 => Attempt::Answer(data),
            503 => Attempt::Retry(message()),
            404 => Attempt::Fail(DurabilityError::NotFound),
            409 => Attempt::Fail(DurabilityError::Conflict(message())),
            s => Attempt::Fail(DurabilityError::Rejected {
                status: u16::try_from(s).unwrap_or(500),
                message: message(),
            }),
        }
    }

    /// One request, retried per the policy.
    async fn call(&self, kind: &str, data: Value) -> Result<Value, DurabilityError> {
        let corr_id = format!("{}-{}", self.pid, self.corr.fetch_add(1, Ordering::Relaxed));
        let start = Instant::now();
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let last = match self.attempt(kind, &corr_id, &data).await {
                Attempt::Answer(v) => return Ok(v),
                Attempt::Fail(e) => return Err(e),
                Attempt::Retry(m) => m,
            };
            let wait = self
                .jitter
                .backoff(self.retry.base, self.retry.cap, attempt);
            if attempt >= self.retry.max_attempts || start.elapsed() + wait > self.retry.deadline {
                return Err(DurabilityError::Unavailable(format!(
                    "{kind} ({corr_id}) after {attempt} attempt(s): {last}"
                )));
            }
            tracing::debug!(kind, corr_id, attempt, error = %last, "resonate call retried");
            tokio::time::sleep(wait).await;
        }
    }

    /// Deliver `Settled` for a promise found settled to this process's
    /// subscription of `group` (Resonate registers no listener on it).
    fn deliver_local(&self, group: &str, view: &PromiseView) {
        if view.status.is_settled()
            && let Some(tx) = lock(&self.subs).get(group)
        {
            let _ = tx.try_send(Delivery::Settled {
                promise_id: view.id.clone(),
                status: view.status,
            });
        }
    }

    async fn open(
        http: &reqwest::Client,
        url: &str,
        token: Option<&TokenFn>,
    ) -> Result<reqwest::Response, String> {
        let mut rb = http.get(url).header("accept", "text/event-stream");
        if let Some(t) = token.and_then(|t| t()) {
            rb = rb.bearer_auth(t);
        }
        let resp = rb.send().await.map_err(|e| e.to_string())?;
        if resp.status().is_success() {
            Ok(resp)
        } else {
            Err(format!("HTTP {}", resp.status()))
        }
    }
}

/// Read one opened stream into `tx` until it ends or the receiver goes.
/// `true` if the receiver is gone.
async fn pump(resp: reqwest::Response, tx: &mpsc::Sender<Delivery>, group: &str) -> bool {
    let mut stream = resp.bytes_stream();
    let mut sse = SseParser::default();
    loop {
        let chunk = tokio::select! {
            _ = tx.closed() => return true,
            c = stream.next() => c,
        };
        match chunk {
            Some(Ok(bytes)) => {
                for msg in sse.feed(&bytes) {
                    match parse_message(&msg) {
                        Some(d) => {
                            if tx.send(d).await.is_err() {
                                return true;
                            }
                        }
                        None => tracing::debug!(group, msg, "poll message ignored"),
                    }
                }
            }
            Some(Err(e)) => {
                tracing::warn!(group, error = %e, "poll stream failed");
                return false;
            }
            None => {
                tracing::warn!(group, "poll stream ended");
                return false;
            }
        }
    }
}

#[async_trait]
impl Durability for HttpDurability {
    async fn promise_create(&self, req: &CreatePromise) -> Result<PromiseView, DurabilityError> {
        req.validate()?;
        let param = match &req.param {
            Some(p) => json!({ "data": b64(p) }),
            None => json!({}),
        };
        let data = self
            .call(
                "promise.create",
                json!({
                    "id": req.id,
                    "timeoutAt": req.timeout_at_ms,
                    "param": param,
                    "tags": req.wire_tags(),
                }),
            )
            .await?;
        let view = promise_of(&data)?;
        if view.param != req.param {
            return Err(DurabilityError::IdConflict { id: req.id.clone() });
        }
        match &req.listener_group {
            Some(g) => self.promise_register_listener(&req.id, g).await,
            None => Ok(view),
        }
    }

    async fn promise_settle(
        &self,
        id: &str,
        state: SettleAs,
    ) -> Result<PromiseView, DurabilityError> {
        let data = self
            .call(
                "promise.settle",
                json!({ "id": id, "state": state.as_str(), "value": {} }),
            )
            .await?;
        promise_of(&data)
    }

    async fn promise_register_listener(
        &self,
        id: &str,
        group: &str,
    ) -> Result<PromiseView, DurabilityError> {
        if !valid_group(group) {
            return Err(DurabilityError::Rejected {
                status: 400,
                message: format!("invalid poll group {group:?}"),
            });
        }
        let data = self
            .call(
                "promise.register_listener",
                json!({ "awaited": id, "address": poll_address(group) }),
            )
            .await?;
        let view = promise_of(&data)?;
        self.deliver_local(group, &view);
        Ok(view)
    }

    async fn promise_get(&self, id: &str) -> Result<PromiseView, DurabilityError> {
        let data = self.call("promise.get", json!({ "id": id })).await?;
        promise_of(&data)
    }

    async fn task_acquire(
        &self,
        task_id: &str,
        version: i64,
        pid: &str,
        ttl_ms: i64,
    ) -> Result<Claim, DurabilityError> {
        let data = self
            .call(
                "task.acquire",
                json!({ "id": task_id, "version": version, "pid": pid, "ttl": ttl_ms }),
            )
            .await?;
        let promise = promise_of(&data)?;
        let version = data
            .pointer("/task/version")
            .and_then(Value::as_i64)
            .ok_or_else(|| DurabilityError::Protocol(format!("no task version in {data}")))?;
        let task_id = data
            .pointer("/task/id")
            .and_then(Value::as_str)
            .unwrap_or(task_id)
            .to_string();
        Ok(Claim {
            task_id,
            version,
            promise_id: promise.id,
            param: promise.param,
        })
    }

    async fn task_heartbeat(
        &self,
        pid: &str,
        tasks: &[(&str, i64)],
    ) -> Result<(), DurabilityError> {
        // One request per origin: the server refuses a mixed one (400).
        let mut by_origin: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
        for (id, version) in tasks {
            by_origin
                .entry(origin(id))
                .or_default()
                .push(json!({ "id": id, "version": version }));
        }
        for (_, tasks) in by_origin {
            self.call("task.heartbeat", json!({ "pid": pid, "tasks": tasks }))
                .await?;
        }
        Ok(())
    }

    async fn task_fulfill(
        &self,
        task_id: &str,
        version: i64,
        state: SettleAs,
    ) -> Result<(), DurabilityError> {
        self.call(
            "task.fulfill",
            json!({
                "id": task_id,
                "version": version,
                "action": {
                    "kind": "promise.settle",
                    "head": {},
                    "data": { "id": task_id, "state": state.as_str(), "value": {} },
                },
            }),
        )
        .await
        .map(|_| ())
    }

    async fn task_release(&self, task_id: &str, version: i64) -> Result<(), DurabilityError> {
        self.call("task.release", json!({ "id": task_id, "version": version }))
            .await
            .map(|_| ())
    }

    fn delay_support(&self) -> DelaySupport {
        self.delay
    }

    async fn subscribe(&self, group: &str) -> Result<mpsc::Receiver<Delivery>, DurabilityError> {
        if !valid_group(group) {
            return Err(DurabilityError::Rejected {
                status: 400,
                message: format!("invalid poll group {group:?}"),
            });
        }
        if lock(&self.subs)
            .get(group)
            .is_some_and(|tx| !tx.is_closed())
        {
            return Err(DurabilityError::Conflict(
                "group already subscribed in this process".into(),
            ));
        }
        let url = format!("{}/poll/{group}/{}", self.base, self.pid);
        // The server registers the connection before it answers, so once
        // the headers are here every later message reaches this stream.
        let start = Instant::now();
        let mut attempt = 0u32;
        let resp = loop {
            attempt += 1;
            match Self::open(&self.http, &url, self.token.as_ref()).await {
                Ok(r) => break r,
                Err(e) => {
                    let wait = self
                        .jitter
                        .backoff(self.retry.base, self.retry.cap, attempt);
                    if attempt >= self.retry.max_attempts
                        || start.elapsed() + wait > self.retry.deadline
                    {
                        return Err(DurabilityError::Unavailable(format!(
                            "poll {group} after {attempt} attempt(s): {e}"
                        )));
                    }
                    tokio::time::sleep(wait).await;
                }
            }
        };
        let (tx, rx) = mpsc::channel(1024);
        lock(&self.subs).insert(group.to_string(), tx.clone());
        let http = self.http.clone();
        let token = self.token.clone();
        let jitter = Arc::clone(&self.jitter);
        let group = group.to_string();
        tokio::spawn(async move {
            let mut resp = resp;
            loop {
                if pump(resp, &tx, &group).await {
                    return;
                }
                let mut attempt = 0u32;
                resp = loop {
                    attempt += 1;
                    let wait = jitter.backoff(
                        Duration::from_millis(100),
                        Duration::from_secs(5),
                        attempt.min(16),
                    );
                    tokio::select! {
                        _ = tx.closed() => return,
                        _ = tokio::time::sleep(wait) => {}
                    }
                    match Self::open(&http, &url, token.as_ref()).await {
                        Ok(r) => break r,
                        Err(e) => tracing::warn!(group, error = %e, "poll reconnect failed"),
                    }
                };
                if tx
                    .send(Delivery::Gap {
                        group: group.clone(),
                    })
                    .await
                    .is_err()
                {
                    return;
                }
            }
        });
        Ok(rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_events_split_across_chunks() {
        let mut p = SseParser::default();
        assert!(p.feed(b": ping\n\ndata: {\"a\"").is_empty());
        assert_eq!(p.feed(b":1}\r\n\r\ndata: x\n\n"), vec!["{\"a\":1}", "x"]);
    }

    #[test]
    fn messages_from_the_pinned_server() {
        // Bytes as the fork's server sent them (31ffb201, sqlite).
        let exec = r#"{"data":{"task":{"id":"wft:a.b:wf:r:1","version":1}},"head":{"serverUrl":""},"kind":"execute"}"#;
        assert_eq!(
            parse_message(exec),
            Some(Delivery::TaskReady {
                task_id: "wft:a.b:wf:r:1".into(),
                version: 1,
                promise_id: "wft:a.b:wf:r:1".into()
            })
        );
        let unblock = r#"{"data":{"promise":{"createdAt":1,"id":"tmr:a","param":{},"settledAt":2,"state":"resolved","tags":{"resonate:timer":"true"},"timeoutAt":2,"value":{}}},"head":{},"kind":"unblock"}"#;
        assert_eq!(
            parse_message(unblock),
            Some(Delivery::Settled {
                promise_id: "tmr:a".into(),
                status: PromiseStatus::Resolved
            })
        );
    }

    #[test]
    fn heartbeat_origin_is_the_prefix() {
        // X1.R13: the origin is the run, so one heartbeat covers a run.
        assert_eq!(origin("n.w.r:act.5.1"), "n.w.r");
        assert_eq!(origin("plain"), "plain");
    }
}
