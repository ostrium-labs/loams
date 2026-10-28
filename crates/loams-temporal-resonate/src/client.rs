//! [`HttpDurability`]: the `Durability` port on FW3's `resonate-client`
//! (D2040, replacing D1591's interim request path).
//!
//! - Requests: every `promise.*` and `task.*` call goes through
//!   [`resonate_client::Client`] (protocol `2026-04-01`, `POST {base}/`),
//!   with its retries ([`RetryPolicy`]: connection failures, timeouts, `503`
//!   and a proxy's `502`/`504`), correlation IDs and per-attempt auth. After
//!   the policy the call is [`DurabilityError::Unavailable`] and may have
//!   been applied.
//! - Deliveries: [`Durability::subscribe`] is the client's poll consumer
//!   (`Client::poll`, `GET {base}/poll/{group}/{pid}`, D2043), which
//!   replaced D1591's interim reader. Its `Reconnected` event is
//!   [`Delivery::Gap`] (X1.R19).
//! - Auth: the token hook's value goes in `Authorization: Bearer …` on both
//!   (the listener's auth, D111).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use resonate_client::{
    Client, PollEvent, PollMessage, PollOptions, PromiseCreate as RcCreate, PromiseRecord,
    PromiseSettle, PromiseState, PromiseValue, SettleState,
};
use tokio::sync::mpsc;

pub use resonate_client::RetryPolicy;

use crate::port::{
    Claim, CreatePromise, DelaySupport, Delivery, Durability, DurabilityError, PromiseStatus,
    PromiseView, SettleAs, b64, poll_address, unb64, valid_group, valid_pid,
};

/// Where a bearer token comes from, read before every request and every poll
/// connection.
pub type TokenFn = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// This process's subscriptions, one per group.
type Subs = Arc<Mutex<BTreeMap<String, mpsc::Sender<Delivery>>>>;

fn lock(subs: &Subs) -> MutexGuard<'_, BTreeMap<String, mpsc::Sender<Delivery>>> {
    subs.lock().unwrap_or_else(|e| e.into_inner())
}

/// Deliveries buffered per subscription.
const SUBSCRIPTION_BUFFER: usize = 1024;

/// Each attempt's own timeout.
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(10);

/// The adapter onto a Resonate server's protocol endpoint (module docs).
pub struct HttpDurability {
    client: Client,
    subs: Subs,
    pid: String,
    retry: RetryPolicy,
    base: String,
    token: Option<TokenFn>,
    delay: DelaySupport,
}

impl std::fmt::Debug for HttpDurability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpDurability")
            .field("base", &self.base)
            .field("pid", &self.pid)
            .field("token", &self.token.as_ref().map(|_| "<hook>"))
            .field("retry", &self.retry)
            .field("delay", &self.delay)
            .finish_non_exhaustive()
    }
}

fn build_client(
    base: &str,
    token: Option<&TokenFn>,
    retry: RetryPolicy,
) -> Result<Client, DurabilityError> {
    let mut b = Client::http(format!("{base}/"))
        .timeout(ATTEMPT_TIMEOUT)
        .connect_timeout(Duration::from_secs(5))
        .retry(retry);
    if let Some(t) = token {
        let t = t.clone();
        b = b.auth(move || t());
    }
    b.build().map_err(map_err)
}

/// `resonate-client`'s error as the port's.
fn map_err(e: resonate_client::Error) -> DurabilityError {
    use resonate_client::Error as E;
    match e {
        E::Rejected { status: 404, .. } => DurabilityError::NotFound,
        E::Rejected {
            status: 409,
            message,
            ..
        } => DurabilityError::Conflict(message),
        E::Rejected {
            status, message, ..
        } => DurabilityError::Rejected { status, message },
        E::Unavailable { .. } => DurabilityError::Unavailable(e.to_string()),
        // The token source failed: nothing was sent.
        E::Auth(m) => DurabilityError::Rejected {
            status: 401,
            message: format!("token source failed: {m}"),
        },
        other => DurabilityError::Protocol(other.to_string()),
    }
}

fn status_of(s: PromiseState) -> PromiseStatus {
    match s {
        PromiseState::Pending => PromiseStatus::Pending,
        PromiseState::Resolved => PromiseStatus::Resolved,
        PromiseState::Rejected => PromiseStatus::Rejected,
        PromiseState::RejectedCanceled => PromiseStatus::RejectedCanceled,
        PromiseState::RejectedTimedout => PromiseStatus::RejectedTimedout,
    }
}

fn settle_state(s: SettleAs) -> SettleState {
    match s {
        SettleAs::Resolved => SettleState::Resolved,
        SettleAs::Rejected => SettleState::Rejected,
        SettleAs::RejectedCanceled => SettleState::RejectedCanceled,
    }
}

fn view(r: PromiseRecord) -> Result<PromiseView, DurabilityError> {
    let param = r.param.data.as_deref().map(unb64).transpose()?;
    Ok(PromiseView {
        id: r.id,
        status: status_of(r.state),
        param,
        tags: r.tags.into_iter().collect::<BTreeMap<_, _>>(),
        timeout_at_ms: r.timeout_at,
        settled_at_ms: r.settled_at,
    })
}

/// A poll event as a delivery; `None` for a message kind the port ignores.
fn delivery_of(group: &str, event: PollEvent) -> Option<Delivery> {
    match event {
        PollEvent::Message(PollMessage::Execute { id, version, .. }) => Some(Delivery::TaskReady {
            task_id: id.clone(),
            version,
            promise_id: id,
        }),
        PollEvent::Message(PollMessage::Unblock { promise, .. }) => Some(Delivery::Settled {
            promise_id: promise.id,
            status: status_of(promise.state),
        }),
        PollEvent::Message(PollMessage::Unknown { kind, .. }) => {
            tracing::debug!(group, kind = %kind, "poll message ignored");
            None
        }
        // Anything sent to the group while the connection was down is lost.
        PollEvent::Reconnected { .. } => Some(Delivery::Gap {
            group: group.to_string(),
        }),
    }
}

/// The origin of an ID, as Resonate computes it: everything before the
/// first `:`. A heartbeat names tasks of one origin only.
fn origin(id: &str) -> &str {
    id.split_once(':').map(|(o, _)| o).unwrap_or(id)
}

impl HttpDurability {
    /// An adapter for the server at `base_url` (its protocol endpoint, e.g.
    /// `http://127.0.0.1:8001` or the engine's `…/durable`), acting as `pid`
    /// (also the poll connection's ID).
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
        let base = base_url.into().trim_end_matches('/').to_string();
        let retry = RetryPolicy::default();
        let client = build_client(&base, token.as_ref(), retry)?;
        Ok(Self {
            client,
            subs: Arc::default(),
            pid,
            retry,
            base,
            token,
            delay: DelaySupport::Defer,
        })
    }

    /// Use `policy` for every call and for opening a subscription.
    pub fn with_retry(mut self, policy: RetryPolicy) -> Result<Self, DurabilityError> {
        self.client = build_client(&self.base, self.token.as_ref(), policy)?;
        self.retry = policy;
        Ok(self)
    }

    /// Whether the server honours `resonate:delay`. [`DelaySupport::Defer`]
    /// by default, which is right on every server. [`DelaySupport::Native`]
    /// is true for the fork's server from `loams/main` f29c04e1 on, on every
    /// store (the SQL stores' fix, fork #8 and upstream #1196), including
    /// `loams-durable`'s embedded one from the 53d38ab3 pin (D2043), but not
    /// for an older server (X1.R14, D2042).
    pub fn with_delay_support(mut self, delay: DelaySupport) -> Self {
        self.delay = delay;
        self
    }

    /// The process ID.
    pub fn pid(&self) -> &str {
        &self.pid
    }
}

#[async_trait]
impl Durability for HttpDurability {
    async fn promise_create(&self, req: &CreatePromise) -> Result<PromiseView, DurabilityError> {
        req.validate()?;
        let mut create = RcCreate::new(req.id.clone(), req.timeout_at_ms);
        if let Some(p) = &req.param {
            create = create.data(b64(p));
        }
        create.tags = req.wire_tags().into_iter().collect();
        let got = view(
            self.client
                .promise_create(&create)
                .await
                .map_err(map_err)?
                .data,
        )?;
        // 200 with the stored record either way (X1.R11 (4)).
        if got.param != req.param {
            return Err(DurabilityError::IdConflict { id: req.id.clone() });
        }
        match &req.listener_group {
            Some(g) => self.promise_register_listener(&req.id, g).await,
            None => Ok(got),
        }
    }

    async fn promise_settle(
        &self,
        id: &str,
        state: SettleAs,
    ) -> Result<PromiseView, DurabilityError> {
        let r = self
            .client
            .promise_settle(&PromiseSettle::new(id, settle_state(state)))
            .await
            .map_err(map_err)?;
        view(r.data)
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
        let r = self
            .client
            .promise_register_listener(id, &poll_address(group))
            .await
            .map_err(map_err)?;
        let v = view(r.data)?;
        // Resonate registers no listener on a settled promise: tell this
        // process's subscription of the group, if any.
        if v.status.is_settled()
            && let Some(tx) = lock(&self.subs).get(group)
        {
            let _ = tx.try_send(Delivery::Settled {
                promise_id: v.id.clone(),
                status: v.status,
            });
        }
        Ok(v)
    }

    async fn promise_get(&self, id: &str) -> Result<PromiseView, DurabilityError> {
        view(self.client.promise_get(id).await.map_err(map_err)?.data)
    }

    async fn task_acquire(
        &self,
        task_id: &str,
        version: i64,
        pid: &str,
        ttl_ms: i64,
    ) -> Result<Claim, DurabilityError> {
        let claim = self
            .client
            .task_acquire(task_id, version, pid, ttl_ms)
            .await
            .map_err(map_err)?
            .data;
        let promise = view(claim.promise)?;
        Ok(Claim {
            task_id: claim.task.id,
            version: claim.task.version,
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
        let mut by_origin: BTreeMap<&str, Vec<(&str, i64)>> = BTreeMap::new();
        for &(id, version) in tasks {
            by_origin.entry(origin(id)).or_default().push((id, version));
        }
        for (_, tasks) in by_origin {
            self.client
                .task_heartbeat(pid, &tasks)
                .await
                .map_err(map_err)?;
        }
        Ok(())
    }

    async fn task_fulfill(
        &self,
        task_id: &str,
        version: i64,
        state: SettleAs,
    ) -> Result<(), DurabilityError> {
        self.client
            .task_fulfill(
                task_id,
                version,
                settle_state(state),
                PromiseValue::default(),
            )
            .await
            .map(|_| ())
            .map_err(map_err)
    }

    async fn task_release(&self, task_id: &str, version: i64) -> Result<(), DurabilityError> {
        self.client
            .task_release(task_id, version)
            .await
            .map(|_| ())
            .map_err(map_err)
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
        // Returns once the server has registered the connection, so every
        // later message reaches it.
        let options = PollOptions {
            open: Some(self.retry),
            ..PollOptions::default()
        };
        let mut sub = self
            .client
            .poll_with(group, &self.pid, options)
            .await
            .map_err(map_err)?;
        let (tx, rx) = mpsc::channel(SUBSCRIPTION_BUFFER);
        lock(&self.subs).insert(group.to_string(), tx.clone());
        let group = group.to_string();
        tokio::spawn(async move {
            loop {
                let event = tokio::select! {
                    _ = tx.closed() => return,
                    e = sub.recv() => e,
                };
                let Some(event) = event else { return };
                let Some(delivery) = delivery_of(&group, event) else {
                    continue;
                };
                if tx.send(delivery).await.is_err() {
                    return;
                }
            }
            // `sub` drops here, which closes the connection.
        });
        Ok(rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_origin_is_the_prefix() {
        // X1.R13: the origin is the run, so one heartbeat covers a run.
        assert_eq!(origin("n.w.r:act.5.1"), "n.w.r");
        assert_eq!(origin("plain"), "plain");
    }

    #[test]
    fn answers_map_onto_the_port() {
        let rejected = |status| resonate_client::Error::Rejected {
            kind: "task.acquire".into(),
            corr_id: "c".into(),
            status,
            message: "m".into(),
            attempts: 1,
        };
        assert_eq!(map_err(rejected(404)), DurabilityError::NotFound);
        assert_eq!(
            map_err(rejected(409)),
            DurabilityError::Conflict("m".into())
        );
        assert_eq!(
            map_err(rejected(422)),
            DurabilityError::Rejected {
                status: 422,
                message: "m".into()
            }
        );
        assert!(matches!(
            map_err(resonate_client::Error::Unavailable {
                kind: "promise.create".into(),
                corr_id: "c".into(),
                attempts: 6,
                message: "refused".into(),
            }),
            DurabilityError::Unavailable(_)
        ));
    }

    #[test]
    fn records_map_onto_views() {
        let r: PromiseRecord = serde_json::from_str(
            r#"{"id":"n.w.r:tmr.5","state":"rejected_timedout","param":{"data":"aGk="},"value":{},"tags":{"resonate:scope":"global"},"timeoutAt":7,"createdAt":1,"settledAt":7}"#,
        )
        .expect("record");
        let v = view(r).expect("view");
        assert_eq!(v.status, PromiseStatus::RejectedTimedout);
        assert_eq!(v.param.as_deref(), Some(&b"hi"[..]));
        assert_eq!(v.settled_at_ms, Some(7));
        assert_eq!(
            v.tags.get("resonate:scope").map(String::as_str),
            Some("global")
        );
    }

    #[test]
    fn poll_events_map_onto_deliveries() {
        let exec = PollEvent::Message(PollMessage::Execute {
            id: "n.w.r:wft.1".into(),
            version: 3,
            server_url: String::new(),
            trace: resonate_client::TraceContext::default(),
        });
        assert_eq!(
            delivery_of("g", exec),
            Some(Delivery::TaskReady {
                task_id: "n.w.r:wft.1".into(),
                version: 3,
                promise_id: "n.w.r:wft.1".into()
            })
        );
        let promise: PromiseRecord = serde_json::from_str(
            r#"{"id":"n.w.r:to.4","state":"rejected_timedout","param":{},"value":{},"tags":{},"timeoutAt":7,"createdAt":1,"settledAt":7}"#,
        )
        .expect("record");
        let unblock = PollEvent::Message(PollMessage::Unblock {
            promise,
            trace: resonate_client::TraceContext::default(),
        });
        assert_eq!(
            delivery_of("g", unblock),
            Some(Delivery::Settled {
                promise_id: "n.w.r:to.4".into(),
                status: PromiseStatus::RejectedTimedout
            })
        );
        assert_eq!(
            delivery_of("g", PollEvent::Reconnected { attempts: 2 }),
            Some(Delivery::Gap { group: "g".into() })
        );
        let unknown = PollEvent::Message(PollMessage::Unknown {
            kind: "resume".into(),
            message: serde_json::json!({}),
        });
        assert_eq!(delivery_of("g", unknown), None);
    }

    #[test]
    fn a_bad_pid_or_url_is_refused() {
        assert!(HttpDurability::new("http://127.0.0.1:1", "a/b", None).is_err());
        assert!(HttpDurability::new("not a url", "p", None).is_err());
    }
}
