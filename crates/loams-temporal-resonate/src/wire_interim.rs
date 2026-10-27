//! What is still interim (D1591, D2041): the **poll subscription**.
//! FW3's `resonate-client` sends every protocol request (the requests of
//! [`crate::HttpDurability`] go through it), but it has no consumer for the
//! poll transport, so this module still reads `GET {base}/poll/{group}/{pid}`
//! itself. When the fork's client grows one, this module goes.
//!
//! - Deliveries: server-sent events, one JSON message per event (`execute`
//!   or `unblock`). A dropped stream is reopened with backoff and a
//!   [`Delivery::Gap`] is sent when it is back (X1.R19).
//! - Auth: the token hook's value goes in `Authorization: Bearer …` (the
//!   listener's auth, D111), as on the requests.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt;
use serde_json::Value;
use tokio::sync::mpsc;

use crate::port::{Delivery, DurabilityError, PromiseStatus, PromiseView, valid_group};

/// Where a bearer token comes from, read before every request and every
/// poll connection.
pub type TokenFn = Arc<dyn Fn() -> Option<String> + Send + Sync>;

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

fn lock(subs: &Subs) -> std::sync::MutexGuard<'_, BTreeMap<String, mpsc::Sender<Delivery>>> {
    subs.lock().unwrap_or_else(|e| e.into_inner())
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

/// How opening a subscription is retried: the request client's policy.
#[derive(Debug, Clone, Copy)]
pub(crate) struct OpenRetry {
    pub max_attempts: u32,
    pub base: Duration,
    pub cap: Duration,
    pub deadline: Duration,
}

/// This process's poll subscriptions, one per group.
pub(crate) struct Poller {
    base: String,
    pid: String,
    token: Option<TokenFn>,
    http: reqwest::Client,
    retry: OpenRetry,
    jitter: Arc<Jitter>,
    subs: Subs,
}

impl std::fmt::Debug for Poller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Poller")
            .field("base", &self.base)
            .field("pid", &self.pid)
            .finish_non_exhaustive()
    }
}

impl Poller {
    /// `base` without a trailing `/`. https uses the process's rustls
    /// provider, installing ring's if there is none.
    pub(crate) fn new(
        base: String,
        pid: String,
        token: Option<TokenFn>,
        retry: OpenRetry,
    ) -> Result<Self, DurabilityError> {
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            // Another thread may install one first; either way one is set.
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| DurabilityError::Protocol(format!("cannot build the HTTP client: {e}")))?;
        Ok(Self {
            base,
            pid,
            token,
            http,
            retry,
            jitter: Arc::new(Jitter::new()),
            subs: Arc::default(),
        })
    }

    pub(crate) fn set_retry(&mut self, retry: OpenRetry) {
        self.retry = retry;
    }

    /// Deliver `Settled` for a promise found settled to this process's
    /// subscription of `group` (Resonate registers no listener on it).
    pub(crate) fn deliver_local(&self, group: &str, view: &PromiseView) {
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

    pub(crate) async fn subscribe(
        &self,
        group: &str,
    ) -> Result<mpsc::Receiver<Delivery>, DurabilityError> {
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
}
