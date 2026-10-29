//! `loams-temporal-resonate`: how the Temporal gateway reaches Resonate
//! (design 74 §5.3–§5.5 and §6, plan DX1 Task 6, D1503 and D1591).
//!
//! - [`ids`]: the deterministic promise IDs of §74 §5.4, as amended by
//!   X1.R13 (the run is Resonate's origin).
//! - [`Durability`]: the port the gateway calls, named after FW3's
//!   `resonate-client` methods, with [`FakeDurability`] (in process, manual
//!   clock, fault injection) for every test above the port.
//! - [`HttpDurability`]: the adapter onto a real server, on FW3's
//!   `resonate-client` (D2040), its poll subscription included (D2043):
//!   nothing interim is left of D1591's adapter.
//! - [`Relay`]: drains the store's outbox into the port.
//!
//! # How §74 §5.3 maps onto the protocol (31ffb201; the fork pinned at 53d38ab3)
//!
//! - A workflow or activity task is a [`CreatePromise`] with `target_group`
//!   (tag `resonate:target = poll://any@<group>`). The task's ID is the
//!   promise's ID. It is pending at version 0, and [`Delivery::TaskReady`]
//!   names the version to acquire; [`Claim::version`] is one more.
//! - A lease that expires makes the task pending again at the acquired
//!   version and redispatches it; a pending task is also redispatched every
//!   30 s, so `TaskReady` duplicates are normal and an acquire that loses
//!   answers [`DurabilityError::Conflict`].
//! - Timers: a promise tagged `resonate:timer` **resolves** at its deadline.
//!   For §5.3's `rejectedTimedout` (the `to` promises) create it with
//!   `awaitable` instead (`resonate:scope = global`), which times out as
//!   `rejected_timedout` and takes a listener just the same.
//! - `resonate:delay` is an absolute time in Unix ms (below `timeoutAt`, and
//!   only with a target). The SQL stores ignored it before the fork's
//!   f29c04e1 ([`DelaySupport`]); on such a server the relay keeps a delayed
//!   create in the outbox until its time.
//! - A listener's `unblock` is sent once; a message for a group with no
//!   subscriber is lost ([`Delivery::Gap`] says when that may have happened).

pub mod ids;

mod client;
mod fake;
mod port;
mod relay;

pub use client::{HttpDurability, RetryPolicy, TokenFn};
pub use fake::{
    DEFAULT_START_MS, DeliveryFault, FakeDurability, FakeTask, FakeTaskState, PENDING_RETRY_TTL_MS,
};
pub use port::{
    Claim, CreatePromise, DelaySupport, Delivery, Durability, DurabilityError, OutboxRequest,
    PromiseStatus, PromiseView, SettleAs, TAG_DELAY, TAG_SCOPE, TAG_TARGET, TAG_TIMER,
    poll_address, valid_group, valid_pid,
};
pub use relay::{
    OUTBOX_CONFLICT_METRIC, OUTBOX_DISPATCHED_METRIC, OUTBOX_REJECTED_METRIC,
    OUTBOX_UNAVAILABLE_METRIC, OutboxSource, Relay, RelayConfig, RelayMetrics, RelayPass,
};
