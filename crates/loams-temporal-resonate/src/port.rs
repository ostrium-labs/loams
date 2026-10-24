//! The `Durability` port (D1591): what the gateway asks of Resonate, named
//! after FW3's `resonate-client` methods so the final adapter is thin.

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Tag: the dispatch target that makes a promise carry a task.
pub const TAG_TARGET: &str = "resonate:target";
/// Tag: a sleep. At its deadline a timer **resolves** (not
/// `rejected_timedout`; 31ffb201 `types.rs::is_timer`).
pub const TAG_TIMER: &str = "resonate:timer";
/// Tag: the absolute time (Unix ms) before which a new task is not
/// dispatched. Needs `resonate:target` and must be below `timeoutAt`.
pub const TAG_DELAY: &str = "resonate:delay";
/// Tag: makes a promise awaitable (a listener may be registered on it) and
/// armed (it times out eagerly, at its deadline, rather than on next read).
pub const TAG_SCOPE: &str = "resonate:scope";

/// The poll address of a group: `poll://any@<group>`.
pub fn poll_address(group: &str) -> String {
    format!("poll://any@{group}")
}

/// A poll group is the host of a `poll://` URL and a path segment of
/// `GET /poll/{group}/{id}`, so it is kept to what both read the same way.
pub fn valid_group(group: &str) -> bool {
    !group.is_empty()
        && group.len() <= 253
        && group
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
        && !group.starts_with(['.', '-'])
        && !group.ends_with(['.', '-'])
        && !group.contains("..")
}

/// A process ID is a path segment of `GET /poll/{group}/{id}`.
pub fn valid_pid(pid: &str) -> bool {
    !pid.is_empty()
        && pid.len() <= 128
        && pid
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.' || b == b'_')
}

/// A `promise.create` the gateway asks for: also the outbox row's payload.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CreatePromise {
    /// The promise ID ([`crate::ids::PromiseId::encode`]).
    pub id: String,
    /// The deadline, Unix ms.
    pub timeout_at_ms: i64,
    /// Tags (the `loams:*` tags of the ID, plus any of the caller's). The
    /// adapter adds the `resonate:*` tags the fields below ask for.
    #[serde(default)]
    pub tags: BTreeMap<String, String>,
    /// The parameter, opaque. Sent base64 in `param.data`.
    #[serde(default, with = "b64_opt", skip_serializing_if = "Option::is_none")]
    pub param: Option<Vec<u8>>,
    /// Make the promise a task dispatched to `poll://any@<group>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_group: Option<String>,
    /// `resonate:timer`: the promise **resolves** at its deadline.
    #[serde(default, skip_serializing_if = "is_false")]
    pub timer: bool,
    /// `resonate:scope = global`: awaitable and armed without being a timer
    /// or a task, so it **rejects as timed out** at its deadline and takes a
    /// listener. What a `to` promise needs for §74 §5.3's `rejectedTimedout`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub awaitable: bool,
    /// `resonate:delay`: do not dispatch the task before this time (Unix ms,
    /// absolute). Needs `target_group`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay_until_ms: Option<i64>,
    /// Register a listener for `poll://any@<group>` after the create. The
    /// promise must be awaitable: a timer, a task, or `awaitable`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listener_group: Option<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl CreatePromise {
    /// The tags sent on the wire: the caller's plus the `resonate:*` ones.
    pub fn wire_tags(&self) -> BTreeMap<String, String> {
        let mut tags = self.tags.clone();
        if let Some(g) = &self.target_group {
            tags.insert(TAG_TARGET.into(), poll_address(g));
        }
        if self.timer {
            tags.insert(TAG_TIMER.into(), "true".into());
        }
        if self.awaitable {
            tags.insert(TAG_SCOPE.into(), "global".into());
        }
        if let Some(d) = self.delay_until_ms {
            tags.insert(TAG_DELAY.into(), d.to_string());
        }
        tags
    }

    /// The checks the server makes on the fields this port sets (31ffb201
    /// `validate_promise_create_data`), made before anything is sent.
    pub fn validate(&self) -> Result<(), DurabilityError> {
        let bad = |m: &str| {
            Err(DurabilityError::Rejected {
                status: 400,
                message: m.to_string(),
            })
        };
        if self.id.is_empty() || self.id.contains('\0') {
            return bad("promise ID is empty or holds a null byte");
        }
        if self.timeout_at_ms < 0 {
            return bad("timeoutAt must be non-negative");
        }
        for g in self.target_group.iter().chain(self.listener_group.iter()) {
            if !valid_group(g) {
                return bad("invalid poll group");
            }
        }
        if let Some(d) = self.delay_until_ms {
            if self.target_group.is_none() {
                return bad("resonate:delay requires a resonate:target tag");
            }
            if d < 0 || d >= self.timeout_at_ms {
                return bad("resonate:delay must be non-negative and less than timeoutAt");
            }
        }
        if self.listener_group.is_some()
            && !(self.timer || self.awaitable || self.target_group.is_some())
        {
            return bad("a listener needs an awaitable promise (timer, task or awaitable)");
        }
        Ok(())
    }
}

/// A promise's state (Resonate's five).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromiseStatus {
    Pending,
    Resolved,
    Rejected,
    RejectedCanceled,
    RejectedTimedout,
}

impl PromiseStatus {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            PromiseStatus::Pending => "pending",
            PromiseStatus::Resolved => "resolved",
            PromiseStatus::Rejected => "rejected",
            PromiseStatus::RejectedCanceled => "rejected_canceled",
            PromiseStatus::RejectedTimedout => "rejected_timedout",
        }
    }

    /// Parse the wire spelling.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "pending" => PromiseStatus::Pending,
            "resolved" => PromiseStatus::Resolved,
            "rejected" => PromiseStatus::Rejected,
            "rejected_canceled" => PromiseStatus::RejectedCanceled,
            "rejected_timedout" => PromiseStatus::RejectedTimedout,
            _ => return None,
        })
    }

    /// Anything but pending.
    pub fn is_settled(self) -> bool {
        self != PromiseStatus::Pending
    }
}

/// How the gateway settles a promise (`promise.settle`, `task.fulfill`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettleAs {
    Resolved,
    Rejected,
    RejectedCanceled,
}

impl SettleAs {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            SettleAs::Resolved => "resolved",
            SettleAs::Rejected => "rejected",
            SettleAs::RejectedCanceled => "rejected_canceled",
        }
    }

    /// The state a promise settled this way is in.
    pub fn status(self) -> PromiseStatus {
        match self {
            SettleAs::Resolved => PromiseStatus::Resolved,
            SettleAs::Rejected => PromiseStatus::Rejected,
            SettleAs::RejectedCanceled => PromiseStatus::RejectedCanceled,
        }
    }
}

/// A promise as Resonate answers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromiseView {
    pub id: String,
    pub status: PromiseStatus,
    /// The parameter (the existing one, on a create that found the ID).
    pub param: Option<Vec<u8>>,
    /// The promise's tags, `resonate:*` included.
    pub tags: BTreeMap<String, String>,
    pub timeout_at_ms: i64,
    pub settled_at_ms: Option<i64>,
}

/// A task this process holds the lease on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    /// The task ID. In Resonate a task's ID **is** its promise's ID.
    pub task_id: String,
    /// The version after the acquire: what heartbeat, fulfill and release
    /// name. One more than the version acquired.
    pub version: i64,
    pub promise_id: String,
    pub param: Option<Vec<u8>>,
}

/// What Resonate sends a subscribed group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    /// An `execute` message: the task is pending at `version`; acquire it
    /// with that version. Sent at creation (or when a delay ends), after a
    /// release or a lease expiry, and again every 30 s while it stays
    /// pending, so duplicates are normal.
    TaskReady {
        task_id: String,
        version: i64,
        promise_id: String,
    },
    /// An `unblock` message to a listener: the promise settled (by a call or
    /// at its deadline). Sent once and not retried by the server.
    Settled {
        promise_id: String,
        status: PromiseStatus,
    },
    /// The subscription was interrupted and is back: anything sent to the
    /// group meanwhile is lost (an `execute` comes back with the 30 s
    /// redispatch, an `unblock` never does), so the caller re-reads the
    /// promises it waits on.
    Gap { group: String },
}

/// Why a port call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DurabilityError {
    /// `404`: no such promise or task.
    #[error("not found")]
    NotFound,
    /// `409`: the task is not in the state or version the call assumed.
    #[error("conflict: {0}")]
    Conflict(String),
    /// The ID exists with a different parameter (a bug: deterministic IDs
    /// name one request).
    #[error("promise {id} exists with a different parameter")]
    IdConflict { id: String },
    /// No answer within the retry budget. The call **may have been applied**.
    #[error("unavailable (may have been applied): {0}")]
    Unavailable(String),
    /// Any other answer: `400` (refused by validation, before or after
    /// sending), `401`/`403`, `422` (a listener on a promise that is not
    /// awaitable), `500`.
    #[error("rejected with {status}: {message}")]
    Rejected { status: u16, message: String },
    /// The peer's answer was not the protocol.
    #[error("protocol: {0}")]
    Protocol(String),
}

/// What the gateway needs from Resonate (§74 §6.2). Every call is
/// idempotent or fenced by a version, so a retried call is harmless.
#[async_trait]
pub trait Durability: Send + Sync + 'static {
    /// Create a promise, or answer the existing one. An existing ID with a
    /// different parameter is [`DurabilityError::IdConflict`]. Registers
    /// `listener_group`'s listener after the create; if the promise is
    /// already settled, a [`Delivery::Settled`] goes to this process's
    /// subscription of that group, if any (Resonate registers no listener
    /// on a settled promise).
    async fn promise_create(&self, req: &CreatePromise) -> Result<PromiseView, DurabilityError>;
    /// Settle a pending promise; a settled one is answered unchanged. Settling
    /// a task's promise fulfils the task.
    async fn promise_settle(
        &self,
        id: &str,
        state: SettleAs,
    ) -> Result<PromiseView, DurabilityError>;
    /// Send `poll://any@<group>` a [`Delivery::Settled`] when `id` settles.
    /// On a settled promise the server registers nothing; the port delivers
    /// to this process's subscription at once instead (as `promise_create`).
    async fn promise_register_listener(
        &self,
        id: &str,
        group: &str,
    ) -> Result<PromiseView, DurabilityError>;
    /// The promise as it is now (a pending one past its deadline reads as
    /// timed out). Added for reconciliation after a [`Delivery::Gap`].
    async fn promise_get(&self, id: &str) -> Result<PromiseView, DurabilityError>;
    /// Take the pending task at `version` as `pid` for `ttl_ms`.
    /// [`DurabilityError::Conflict`] if it is not pending at that version.
    async fn task_acquire(
        &self,
        task_id: &str,
        version: i64,
        pid: &str,
        ttl_ms: i64,
    ) -> Result<Claim, DurabilityError>;
    /// Extend the leases `pid` holds on `tasks` (`(task_id, version)`), by
    /// each lease's own TTL. Leases not held are skipped silently.
    async fn task_heartbeat(&self, pid: &str, tasks: &[(&str, i64)])
    -> Result<(), DurabilityError>;
    /// Settle the task's promise and finish the task.
    async fn task_fulfill(
        &self,
        task_id: &str,
        version: i64,
        state: SettleAs,
    ) -> Result<(), DurabilityError>;
    /// Give the task back: it is pending again and redispatched.
    async fn task_release(&self, task_id: &str, version: i64) -> Result<(), DurabilityError>;
    /// Receive what Resonate sends `poll://any@<group>`. One subscription per
    /// group per process: a second while the first's receiver is alive is
    /// [`DurabilityError::Conflict`].
    async fn subscribe(
        &self,
        group: &str,
    ) -> Result<tokio::sync::mpsc::Receiver<Delivery>, DurabilityError>;

    /// Whether the server behind the port honours `resonate:delay`. The
    /// relay holds back a delayed create itself when it does not.
    fn delay_support(&self) -> DelaySupport {
        DelaySupport::Native
    }

    /// The clock deadlines are judged by, Unix ms (the fake's own clock;
    /// the wall clock for a real server).
    fn now_ms(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

/// Whether a server honours `resonate:delay` (31ffb201: the oracle and the
/// blob server, which backs TiKV, do; the SQL stores, SQLite, Postgres and
/// MySQL, ignore it and dispatch at create).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelaySupport {
    /// The server holds the first dispatch until the delay ends.
    Native,
    /// It does not: the [`crate::Relay`] keeps a delayed create in the
    /// outbox until its time, then sends it.
    Defer,
}

/// The outbox row's JSON (the store keeps it opaque).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum OutboxRequest {
    CreatePromise(CreatePromise),
    Settle { id: String, state: SettleAs },
}

impl OutboxRequest {
    /// The promise the row is about.
    pub fn promise_id(&self) -> &str {
        match self {
            OutboxRequest::CreatePromise(c) => &c.id,
            OutboxRequest::Settle { id, .. } => id,
        }
    }
}

pub(crate) fn b64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub(crate) fn unb64(s: &str) -> Result<Vec<u8>, DurabilityError> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| DurabilityError::Protocol(format!("param is not base64: {e}")))
}

mod b64_opt {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(b) => s.serialize_str(&super::b64(b)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        let o: Option<String> = Option::deserialize(d)?;
        o.map(|s| super::unb64(&s).map_err(serde::de::Error::custom))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outbox_request_json_is_stable() {
        let row = OutboxRequest::CreatePromise(CreatePromise {
            id: "tmr:n:w:r:5".into(),
            timeout_at_ms: 10,
            param: Some(b"hi".to_vec()),
            timer: true,
            listener_group: Some("lgw.0".into()),
            ..Default::default()
        });
        let json = serde_json::to_string(&row).unwrap();
        assert_eq!(
            json,
            r#"{"op":"create_promise","id":"tmr:n:w:r:5","timeout_at_ms":10,"tags":{},"param":"aGk=","timer":true,"listener_group":"lgw.0"}"#
        );
        assert_eq!(serde_json::from_str::<OutboxRequest>(&json).unwrap(), row);
        let settle = OutboxRequest::Settle {
            id: "x".into(),
            state: SettleAs::RejectedCanceled,
        };
        let json = serde_json::to_string(&settle).unwrap();
        assert_eq!(
            json,
            r#"{"op":"settle","id":"x","state":"rejected_canceled"}"#
        );
        assert_eq!(
            serde_json::from_str::<OutboxRequest>(&json).unwrap(),
            settle
        );
    }

    #[test]
    fn groups_and_pids() {
        assert!(valid_group("ltq.3.ab12"));
        assert!(!valid_group("LTQ"));
        assert!(!valid_group("a/b"));
        assert!(valid_pid("gw-1_a.b"));
        assert!(!valid_pid("a/b"));
    }
}
