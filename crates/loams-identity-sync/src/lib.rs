//! `identity-sync`: keeps OpenFGA's tuples and Loams' principal links consistent with
//! Zitadel's identity state (design 76 §8.1; D1608 as amended by D1622, D1623, D1624 and
//! D1628; plan IA1, IA3).
//!
//! - **The feed** ([`IdentitySync::step`]) is Zitadel's event list (`ListEvents`), read in ascending
//!   order from a [`Position`] kept with the sync's [`State`]: the last creation date read
//!   and the `(aggregate, sequence)` pairs at that date, so a re-read skips what was applied
//!   (D1622). It reads every few seconds, and at once when [`webhook`] is woken.
//! - **Membership** comes from user grants on the Loams project (D1623): role keys
//!   `member`, `admin` and `owner` become `org:O#member|admin|owner@user:U`. Zitadel's own
//!   org members (`org.member.*`) are its managers and map to nothing.
//! - **Deactivation** (`user.deactivated`, `user.locked`, `user.removed`) disables the
//!   principal's link first, so the engine refuses its live tokens on the next request
//!   (D1624), then deletes every tuple whose user is that principal.
//! - **Machine users** (`user.machine.added`) get a principal link of kind
//!   `service_account` unless one exists (an agent's, written by `loams-zitadel`'s
//!   `Agents::create`, D1628).
//! - **The webhook** ([`webhook`]) is a wake-up only: it verifies `ZITADEL-Signature` and
//!   never applies the body (Actions v2 delivers at most once).
//! - **The reconciliation** ([`reconcile`]) compares the grants Zitadel lists with OpenFGA,
//!   repairs what both Zitadel and the event state agree on, and reports the drift.
//!
//! Writes are idempotent (a duplicate write and a missing delete succeed), and a batch of
//! events is committed to the [`StateStore`] only after its writes succeeded, so a failure
//! is retried on the next step.

pub mod memory;
pub mod reconcile;
mod state;
pub mod webhook;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use loams_authz::Tuple;
use loams_zitadel::Zitadel;
use loams_zitadel::events::Event;
use serde_json::Value;
use time::OffsetDateTime;
use tokio::sync::Notify;

pub use state::{GrantRec, Position, State};

/// What can go wrong.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Zitadel(#[from] loams_zitadel::Error),
    #[error("authorization store: {0}")]
    Authz(#[from] loams_authz::AuthzError),
    #[error("principal store: {0}")]
    Principals(String),
    #[error("sync state: {0}")]
    State(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// The org roles a user grant's role keys stand for (D1623). Any other role key is ignored.
pub const ROLE_KEYS: [&str; 3] = ["member", "admin", "owner"];

/// The object types of model v1 that a person, an agent or a service account can be written
/// on directly. A deactivated principal's tuples are read and deleted on each of them.
pub const DIRECT_TYPES: [&str; 9] = [
    "platform",
    "org",
    "team",
    "agent",
    "project",
    "environment",
    "deployment",
    "database",
    "graph",
];

/// The most events one step asks for at once, when more than a batch share one creation date.
pub const MAX_BATCH: u32 = 10_000;

/// The event types the sync reads (at most 30, `ListEvents`' limit).
pub const EVENT_TYPES: [&str; 12] = [
    "user.grant.added",
    "user.grant.changed",
    "user.grant.removed",
    "user.grant.cascade.removed",
    "user.grant.deactivated",
    "user.grant.reactivated",
    "user.machine.added",
    "user.deactivated",
    "user.reactivated",
    "user.locked",
    "user.unlocked",
    "user.removed",
];

/// Where tuples live: OpenFGA (`loams_authz::OpenFgaAuthorizer`), or [`memory::MemoryTuples`].
#[async_trait]
pub trait Tuples: Send + Sync {
    /// Writes and deletes idempotently.
    async fn write(&self, writes: &[Tuple], deletes: &[Tuple]) -> loams_authz::Result<()>;
    /// The tuples on `object` (`org:o1`, or a type alone, `org:`, with `user`), narrowed by
    /// `user` and `relation` when given.
    async fn read(
        &self,
        user: Option<&str>,
        relation: Option<&str>,
        object: &str,
    ) -> loams_authz::Result<Vec<Tuple>>;
}

#[async_trait]
impl Tuples for loams_authz::OpenFgaAuthorizer {
    async fn write(&self, writes: &[Tuple], deletes: &[Tuple]) -> loams_authz::Result<()> {
        loams_authz::OpenFgaAuthorizer::write(self, writes, deletes).await
    }

    async fn read(
        &self,
        user: Option<&str>,
        relation: Option<&str>,
        object: &str,
    ) -> loams_authz::Result<Vec<Tuple>> {
        loams_authz::OpenFgaAuthorizer::read(self, user, relation, object).await
    }
}

/// A Loams principal's kind, as its link records it (D1628).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    User,
    Agent,
    ServiceAccount,
}

/// Loams' principal links `(issuer, subject) → (kind, disabled)`: the engine's principal
/// store, or [`memory::MemoryPrincipals`].
#[async_trait]
pub trait Principals: Send + Sync {
    /// Links `subject` as `kind` unless it is linked already (an agent's link is never
    /// turned into a service account's). Atomic in the store.
    async fn link_if_absent(&self, issuer: &str, subject: &str, kind: Kind) -> Result<()>;
    /// Sets the link's `disabled`, linking `subject` as `kind_if_absent` first when it has no
    /// link (so a person with no link is refused too, D1624).
    async fn set_disabled(
        &self,
        issuer: &str,
        subject: &str,
        kind_if_absent: Kind,
        disabled: bool,
    ) -> Result<()>;
}

/// Where the sync's [`State`] is kept (the metastore in the engine).
#[async_trait]
pub trait StateStore: Send + Sync {
    async fn load(&self) -> Result<Option<State>>;
    async fn save(&self, state: &State) -> Result<()>;
}

/// The sync's settings.
#[derive(Clone, Debug)]
pub struct Config {
    /// The Loams project's id in Zitadel: only grants on it are memberships.
    pub project_id: String,
    /// The issuer of the tokens whose subjects the links name.
    pub issuer: String,
    /// How often the event list is read without a wake-up.
    pub poll: Duration,
    /// Events per `ListEvents` call.
    pub batch: u32,
    /// How often the reconciliation runs (nightly by default).
    pub reconcile_every: Duration,
}

impl Config {
    pub fn new(project_id: impl Into<String>, issuer: impl Into<String>) -> Self {
        Self {
            project_id: project_id.into(),
            issuer: issuer.into(),
            poll: Duration::from_secs(3),
            batch: 200,
            reconcile_every: Duration::from_secs(24 * 3600),
        }
    }
}

/// The bridge.
pub struct IdentitySync {
    zitadel: Zitadel,
    tuples: Arc<dyn Tuples>,
    principals: Arc<dyn Principals>,
    store: Arc<dyn StateStore>,
    config: Config,
    wake: Arc<Notify>,
}

impl std::fmt::Debug for IdentitySync {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdentitySync")
            .field("zitadel", &self.zitadel)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// What one [`IdentitySync::step`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stepped {
    /// Events read and applied (or skipped as already applied, or not the sync's concern).
    pub events: usize,
    pub writes: usize,
    pub deletes: usize,
}

impl IdentitySync {
    pub fn new(
        zitadel: Zitadel,
        tuples: Arc<dyn Tuples>,
        principals: Arc<dyn Principals>,
        store: Arc<dyn StateStore>,
        config: Config,
    ) -> Self {
        Self {
            zitadel,
            tuples,
            principals,
            store,
            config,
            wake: Arc::new(Notify::new()),
        }
    }

    /// What the webhook notifies to make the sync read now.
    pub fn waker(&self) -> Arc<Notify> {
        self.wake.clone()
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Reads and applies every event after the stored position, a batch at a time, each
    /// batch committed after its writes succeeded.
    ///
    /// # Errors
    ///
    /// Zitadel, OpenFGA, the principal store or the state store failing; nothing of the
    /// failed batch is committed, and the next step retries it.
    pub async fn step(&self) -> Result<Stepped> {
        let mut state = self.store.load().await?.unwrap_or_default();
        let mut done = Stepped::default();
        let mut limit = self.config.batch.max(1);
        loop {
            // `from` is strict in Zitadel: ask from just before the stored date, so the
            // events at that date come again and the stored pairs skip the ones applied.
            let from = state
                .position
                .at
                .map(|at| at - time::Duration::microseconds(1));
            let events = self.zitadel.list_events(&EVENT_TYPES, from, limit).await?;
            let full = events.len() >= limit as usize;
            let mut next = state.clone();
            let mut fresh = 0;
            for event in &events {
                if !next.position.advance(event) {
                    continue;
                }
                fresh += 1;
                let (w, d) = self.apply(&mut next, event).await?;
                done.writes += w;
                done.deletes += d;
            }
            done.events += fresh;
            if fresh > 0 || next.position != state.position {
                self.store.save(&next).await?;
                state = next;
            }
            if !full {
                return Ok(done);
            }
            if fresh == 0 {
                // A full batch of events at one creation date, all applied: ListEvents cannot
                // page within one date, so ask for more at once.
                if limit >= MAX_BATCH {
                    tracing::warn!(
                        target: "loams_identity_sync",
                        limit,
                        "more events at one creation date than one ListEvents call returns"
                    );
                    return Ok(done);
                }
                limit = limit.saturating_mul(2).min(MAX_BATCH);
            } else {
                limit = self.config.batch.max(1);
            }
        }
    }

    /// Applies one event to `state` and to the stores; answers the writes and deletes made.
    async fn apply(&self, state: &mut State, event: &Event) -> Result<(usize, usize)> {
        if !state.newer(event) {
            return Ok((0, 0));
        }
        let user_id = |payload: &Value| {
            payload
                .get("userId")
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        match event.kind.as_str() {
            "user.grant.added" => {
                let project = event.payload.get("projectId").and_then(Value::as_str);
                let Some(user) = user_id(&event.payload) else {
                    return Ok((0, 0));
                };
                if project != Some(self.config.project_id.as_str()) {
                    return Ok((0, 0));
                }
                let before = state.roles(&user, &event.resource_owner);
                state.grants.insert(
                    event.aggregate_id.clone(),
                    GrantRec {
                        user: user.clone(),
                        org: event.resource_owner.clone(),
                        roles: role_keys(&event.payload),
                        active: true,
                    },
                );
                self.membership(state, &user, &event.resource_owner, before)
                    .await
            }
            "user.grant.changed"
            | "user.grant.removed"
            | "user.grant.cascade.removed"
            | "user.grant.deactivated"
            | "user.grant.reactivated" => {
                let Some(grant) = state.grants.get(&event.aggregate_id).cloned() else {
                    // Not a grant on the Loams project.
                    return Ok((0, 0));
                };
                let before = state.roles(&grant.user, &grant.org);
                match event.kind.as_str() {
                    "user.grant.changed" => {
                        if let Some(g) = state.grants.get_mut(&event.aggregate_id) {
                            g.roles = role_keys(&event.payload);
                        }
                    }
                    "user.grant.deactivated" | "user.grant.reactivated" => {
                        if let Some(g) = state.grants.get_mut(&event.aggregate_id) {
                            g.active = event.kind == "user.grant.reactivated";
                        }
                    }
                    _ => {
                        state.grants.remove(&event.aggregate_id);
                    }
                }
                self.membership(state, &grant.user, &grant.org, before)
                    .await
            }
            "user.machine.added" => {
                self.principals
                    .link_if_absent(
                        &self.config.issuer,
                        &event.aggregate_id,
                        Kind::ServiceAccount,
                    )
                    .await?;
                Ok((0, 0))
            }
            "user.deactivated" | "user.locked" | "user.removed" => {
                let reason = event.kind.trim_start_matches("user.").to_owned();
                state
                    .disabled
                    .entry(event.aggregate_id.clone())
                    .or_default()
                    .insert(reason);
                // The link first: the engine refuses live tokens before the tuples go.
                self.principals
                    .set_disabled(&self.config.issuer, &event.aggregate_id, Kind::User, true)
                    .await?;
                let deletes = self.principal_tuples(&event.aggregate_id).await?;
                self.tuples.write(&[], &deletes).await?;
                Ok((0, deletes.len()))
            }
            "user.reactivated" | "user.unlocked" => {
                let reason = if event.kind == "user.reactivated" {
                    "deactivated"
                } else {
                    "locked"
                };
                let Some(reasons) = state.disabled.get_mut(&event.aggregate_id) else {
                    return Ok((0, 0));
                };
                reasons.remove(reason);
                if !reasons.is_empty() {
                    return Ok((0, 0));
                }
                state.disabled.remove(&event.aggregate_id);
                self.principals
                    .set_disabled(&self.config.issuer, &event.aggregate_id, Kind::User, false)
                    .await?;
                // Its memberships come back from its grants; anything else deleted at
                // deactivation (an agent's grants, a team) is granted again by hand.
                let writes: Vec<Tuple> = state
                    .orgs_of(&event.aggregate_id)
                    .into_iter()
                    .flat_map(|org| {
                        role_tuples(
                            &event.aggregate_id,
                            &org,
                            &state.roles(&event.aggregate_id, &org),
                        )
                    })
                    .collect();
                self.tuples.write(&writes, &[]).await?;
                Ok((writes.len(), 0))
            }
            _ => Ok((0, 0)),
        }
    }

    /// Writes and deletes `user`'s roles in `org` from `before` to what the state now gives.
    async fn membership(
        &self,
        state: &State,
        user: &str,
        org: &str,
        before: BTreeSet<String>,
    ) -> Result<(usize, usize)> {
        let after = state.roles(user, org);
        let writes = role_tuples(user, org, &after.difference(&before).cloned().collect());
        let deletes = role_tuples(user, org, &before.difference(&after).cloned().collect());
        self.tuples.write(&writes, &deletes).await?;
        Ok((writes.len(), deletes.len()))
    }

    /// Every stored tuple whose user is `id` as a person, an agent or a service account.
    async fn principal_tuples(&self, id: &str) -> Result<Vec<Tuple>> {
        let mut out = Vec::new();
        for kind in ["user", "agent", "service_account"] {
            let user = format!("{kind}:{id}");
            for ty in DIRECT_TYPES {
                out.extend(
                    self.tuples
                        .read(Some(&user), None, &format!("{ty}:"))
                        .await?,
                );
            }
        }
        Ok(out)
    }

    /// Runs until `shutdown` is notified: a step every [`Config::poll`] or on a wake-up, and
    /// the reconciliation every [`Config::reconcile_every`]. Failures are logged and retried.
    pub async fn run(self: Arc<Self>, shutdown: Arc<Notify>) {
        let mut next_reconcile = tokio::time::Instant::now() + self.config.reconcile_every;
        loop {
            match self.step().await {
                Ok(s) if s.events > 0 => tracing::info!(
                    target: "loams_identity_sync",
                    events = s.events, writes = s.writes, deletes = s.deletes,
                    "applied Zitadel events"
                ),
                Ok(_) => {}
                Err(err) => {
                    tracing::warn!(target: "loams_identity_sync", %err, "sync step failed; retrying")
                }
            }
            if tokio::time::Instant::now() >= next_reconcile {
                match reconcile::reconcile(&self).await {
                    Ok(report) => report.log(),
                    Err(err) => {
                        tracing::warn!(target: "loams_identity_sync", %err, "reconciliation failed")
                    }
                }
                next_reconcile = tokio::time::Instant::now() + self.config.reconcile_every;
            }
            tokio::select! {
                () = shutdown.notified() => return,
                () = self.wake.notified() => {}
                () = tokio::time::sleep(self.config.poll) => {}
            }
        }
    }
}

/// The org role keys of a grant's payload.
fn role_keys(payload: &Value) -> BTreeSet<String> {
    payload
        .get("roleKeys")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|k| ROLE_KEYS.contains(k))
        .map(str::to_owned)
        .collect()
}

/// `org:<org>#<role>@user:<user>` for each role.
pub(crate) fn role_tuples(user: &str, org: &str, roles: &BTreeSet<String>) -> Vec<Tuple> {
    roles
        .iter()
        .map(|r| Tuple::new(format!("user:{user}"), r, format!("org:{org}")))
        .collect()
}

/// `now`, for the webhook's timestamp check.
pub(crate) fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}
