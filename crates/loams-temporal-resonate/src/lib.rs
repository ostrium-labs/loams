//! `loams-temporal-resonate`: how the Temporal gateway reaches Resonate
//! (design 74 §5.3–§5.5 and §6, plan DX1 Task 6, D1503 and D1591).
//!
//! - [`ids`]: the deterministic promise IDs of §74 §5.4, as amended by
//!   X1.R13 (the run is Resonate's origin).
//! - [`Durability`]: the port the gateway calls, with [`FakeDurability`] (in
//!   process, manual clock, fault injection), today its only implementation.
//! - [`Relay`]: drains the store's outbox into the port.
//!
//! There is no adapter onto a real Resonate server any more. `HttpDurability`
//! was built on `resonate-client` and its poll consumer, which only the
//! ostrium-labs/resonate fork had (FW3, D2040, D2043); D2450 dropped the fork,
//! and the official resonatehq/resonate has no Rust protocol client. Until an
//! adapter is written on the official protocol, the Temporal gateway runs on
//! the in-process fake only (development).
//!
//! # How §74 §5.3 maps onto the protocol (31ffb201)
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
//!   only with a target). Upstream's SQL stores dispatch at create and ignore
//!   it ([`DelaySupport`]); on such a server the relay keeps a delayed create
//!   in the outbox until its time.
//! - A listener's `unblock` is sent once; a message for a group with no
//!   subscriber is lost ([`Delivery::Gap`] says when that may have happened).

pub mod ids;

mod fake;
mod port;
mod relay;

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
