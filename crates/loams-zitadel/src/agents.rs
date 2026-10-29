//! Loams agents as Zitadel machine users (design 76 §10, D1607, D1624, D1628; plan IA4 Task 1).
//!
//! - **Create** ([`Agents::create`]): a machine user with JWT access tokens and a private-key
//!   JWT key, the principal link `(issuer, sub) → agent` (the link is the source of the kind,
//!   D1628), and the agent's `owner` and `project` tuples. **No grant**: the owner's rights
//!   are never the agent's (D1607).
//! - **Grant** ([`Agents::grant`]): one tuple on one environment, standing or for one agent
//!   session (`agent with agent_session`, Q1204).
//! - **Revoke** ([`Agents::revoke`]): the link is disabled first, so the engine refuses the
//!   agent's tokens that are already issued on the next request (D1624); then its grants are
//!   deleted and the machine user is deactivated in Zitadel, so it gets no new token. Every
//!   step runs even when an earlier one fails, and the first failure is returned.
//!
//! The tuples go to a [`TupleStore`] (OpenFGA here; the built-in authorizer reads the engine's
//! principal store instead), and the links to a
//! [`LinkStore`] (the engine's principal store).

use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use loams_authz::{OpenFgaAuthorizer, Tuple};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::{AddedKey, Error, Result, Zitadel};

/// How long an agent's key is valid by default: 90 days.
pub const DEFAULT_KEY_TTL: Duration = Duration::from_secs(90 * 24 * 3600);

/// Where relationship tuples are written.
#[async_trait]
pub trait TupleStore: Send + Sync {
    /// Writes and deletes idempotently (a duplicate write and a missing delete succeed).
    async fn write(&self, writes: &[Tuple], deletes: &[Tuple]) -> loams_authz::Result<()>;
}

#[async_trait]
impl TupleStore for OpenFgaAuthorizer {
    async fn write(&self, writes: &[Tuple], deletes: &[Tuple]) -> loams_authz::Result<()> {
        OpenFgaAuthorizer::write(self, writes, deletes).await
    }
}

#[async_trait]
impl<T: TupleStore + ?Sized> TupleStore for std::sync::Arc<T> {
    async fn write(&self, writes: &[Tuple], deletes: &[Tuple]) -> loams_authz::Result<()> {
        (**self).write(writes, deletes).await
    }
}

/// A principal link for an agent: the identity provider's `(issuer, subject)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentLink {
    pub issuer: String,
    /// The machine user's id, its tokens' `sub`; also the agent's id (`agent:<id>`).
    pub subject: String,
    /// A disabled link's tokens are refused on every request (D1624).
    pub disabled: bool,
}

/// Where principal links are kept (the engine's principal store).
#[async_trait]
pub trait LinkStore: Send + Sync {
    async fn put(&self, link: AgentLink) -> Result<()>;
    /// Sets the link's `disabled`; a missing link is an error.
    async fn set_disabled(&self, issuer: &str, subject: &str, disabled: bool) -> Result<()>;
}

#[async_trait]
impl<L: LinkStore + ?Sized> LinkStore for std::sync::Arc<L> {
    async fn put(&self, link: AgentLink) -> Result<()> {
        (**self).put(link).await
    }

    async fn set_disabled(&self, issuer: &str, subject: &str, disabled: bool) -> Result<()> {
        (**self).set_disabled(issuer, subject, disabled).await
    }
}

/// Links in memory (tests, and a process that keeps none of its own).
#[derive(Debug, Default)]
pub struct MemoryLinks(Mutex<Vec<AgentLink>>);

impl MemoryLinks {
    pub fn get(&self, issuer: &str, subject: &str) -> Option<AgentLink> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|l| l.issuer == issuer && l.subject == subject)
            .cloned()
    }
}

#[async_trait]
impl LinkStore for MemoryLinks {
    async fn put(&self, link: AgentLink) -> Result<()> {
        let mut links = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        links.retain(|l| !(l.issuer == link.issuer && l.subject == link.subject));
        links.push(link);
        Ok(())
    }

    async fn set_disabled(&self, issuer: &str, subject: &str, disabled: bool) -> Result<()> {
        let mut links = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let link = links
            .iter_mut()
            .find(|l| l.issuer == issuer && l.subject == subject)
            .ok_or_else(|| Error::Invalid(format!("no link for {issuer} {subject}")))?;
        link.disabled = disabled;
        Ok(())
    }
}

/// What a new agent is.
#[derive(Clone, Debug)]
pub struct NewAgent<'a> {
    /// The machine user's username, unique in the org.
    pub username: &'a str,
    pub name: &'a str,
    pub description: Option<&'a str>,
    /// The person accountable for it: their OpenFGA user id (`user:<id>`'s id).
    pub owner: &'a str,
    /// The project it is registered in: the OpenFGA project id.
    pub project: &'a str,
}

/// An agent [`Agents::create`] made.
#[derive(Clone, Debug)]
pub struct Agent {
    /// The machine user's id: its tokens' `sub` and `agent:<id>`.
    pub id: String,
    /// Its key; the key file is in it only now.
    pub key: AddedKey,
}

/// A grant for one agent session, which ends at `expires` (the `agent_session` condition).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionGrant {
    pub session: String,
    pub expires: OffsetDateTime,
}

/// The relations an agent may be granted on an environment.
pub const GRANTABLE: &[&str] = &["viewer", "editor", "deployer"];

/// Agents in one Zitadel org, with their tuples and links.
#[derive(Debug)]
pub struct Agents<T, L> {
    zitadel: Zitadel,
    /// The issuer whose tokens the links name (the instance's issuer URL, byte for byte).
    issuer: String,
    org_id: String,
    tuples: T,
    links: L,
    key_ttl: Duration,
}

impl<T: TupleStore, L: LinkStore> Agents<T, L> {
    pub fn new(
        zitadel: Zitadel,
        issuer: impl Into<String>,
        org_id: impl Into<String>,
        tuples: T,
        links: L,
    ) -> Self {
        Self {
            zitadel,
            issuer: issuer.into(),
            org_id: org_id.into(),
            tuples,
            links,
            key_ttl: DEFAULT_KEY_TTL,
        }
    }

    /// Keys valid for `ttl` instead of [`DEFAULT_KEY_TTL`].
    #[must_use]
    pub fn with_key_ttl(mut self, ttl: Duration) -> Self {
        self.key_ttl = ttl;
        self
    }

    pub fn tuples(&self) -> &T {
        &self.tuples
    }

    pub fn links(&self) -> &L {
        &self.links
    }

    pub fn zitadel(&self) -> &Zitadel {
        &self.zitadel
    }

    /// The structural tuples of an agent: its owner and its project. They grant nothing.
    pub fn roots(agent: &str, owner: &str, project: &str) -> [Tuple; 2] {
        let a = format!("agent:{agent}");
        [
            Tuple::new(format!("user:{owner}"), "owner", &a),
            Tuple::new(format!("project:{project}"), "project", a),
        ]
    }

    /// Creates the agent (module docs). If a later step fails, the machine user is deleted
    /// again, so no half-made agent is left that holds a key.
    ///
    /// # Errors
    ///
    /// The first step that failed.
    pub async fn create(&self, new: &NewAgent<'_>) -> Result<Agent> {
        let id = self
            .zitadel
            .create_machine_user(&self.org_id, new.username, new.name, new.description)
            .await?;
        match self.finish_create(&id, new).await {
            Ok(key) => Ok(Agent { id, key }),
            Err(err) => {
                if let Err(undo) = self.zitadel.delete_user(&id).await {
                    tracing::warn!(user = %id, error = %undo, "removing a half-made agent failed");
                }
                // The link (if written) names a user that no longer exists; disable it.
                let _ = self.links.set_disabled(&self.issuer, &id, true).await;
                Err(err)
            }
        }
    }

    async fn finish_create(&self, id: &str, new: &NewAgent<'_>) -> Result<AddedKey> {
        let expires = OffsetDateTime::now_utc()
            + time::Duration::try_from(self.key_ttl)
                .map_err(|e| Error::Invalid(format!("key ttl: {e}")))?;
        let key = self.zitadel.add_key(id, expires, None).await?;
        self.links
            .put(AgentLink {
                issuer: self.issuer.clone(),
                subject: id.to_owned(),
                disabled: false,
            })
            .await?;
        self.tuples
            .write(&Self::roots(id, new.owner, new.project), &[])
            .await?;
        Ok(key)
    }

    /// Grants `agent` `relation` (`viewer`, `editor` or `deployer`) on `environment`, for good
    /// or for one session, and answers the tuple (what [`Agents::revoke`] deletes).
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] for another relation; the store's refusal.
    pub async fn grant(
        &self,
        agent: &str,
        relation: &str,
        environment: &str,
        session: Option<&SessionGrant>,
    ) -> Result<Tuple> {
        if !GRANTABLE.contains(&relation) {
            return Err(Error::Invalid(format!(
                "an agent is granted one of {GRANTABLE:?} on an environment, not {relation:?}"
            )));
        }
        let mut tuple = Tuple::new(
            format!("agent:{agent}"),
            relation,
            format!("environment:{environment}"),
        );
        if let Some(s) = session {
            let expires = s
                .expires
                .format(&Rfc3339)
                .map_err(|e| Error::Invalid(format!("expiry: {e}")))?;
            let mut ctx = serde_json::Map::new();
            ctx.insert("allowed_session".into(), s.session.clone().into());
            ctx.insert("expires".into(), expires.into());
            tuple = tuple.with_condition("agent_session", ctx);
        }
        self.tuples.write(std::slice::from_ref(&tuple), &[]).await?;
        Ok(tuple)
    }

    /// Revokes the agent (module docs): its link disabled, `grants` deleted, its machine user
    /// deactivated.
    ///
    /// # Errors
    ///
    /// The first step that failed; the others still ran.
    pub async fn revoke(&self, agent: &str, grants: &[Tuple]) -> Result<()> {
        let link = self.links.set_disabled(&self.issuer, agent, true).await;
        let tuples = self.tuples.write(&[], grants).await.map_err(Error::from);
        let user = self.zitadel.deactivate(agent).await;
        link.and(tuples).and(user)
    }

    /// Undoes [`Agents::revoke`]'s link and machine user; grants must be made again.
    ///
    /// # Errors
    ///
    /// The first step that failed.
    pub async fn reinstate(&self, agent: &str) -> Result<()> {
        self.zitadel.reactivate(agent).await?;
        self.links.set_disabled(&self.issuer, agent, false).await
    }
}
