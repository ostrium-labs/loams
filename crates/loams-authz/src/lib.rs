//! Loams' one authorizer (design §76, D1610).
//!
//! Zitadel answers "who"; this crate answers "what can they do". Application code asks one
//! question, through [`Authorizer`]: may this [`Subject`] hold this `can_*` relation on this
//! object? The model that answers it is `deploy/openfga/core.fga` (D1603).
//!
//! | Implementation | Use |
//! |---|---|
//! | [`OpenFgaAuthorizer`] | OpenFGA over gRPC (`openfga-client`), profile `authz` |
//! | [`CachedAuthorizer`] | wraps any authorizer: allow decisions only, TTL ≤ 5 s (D66, D1611) |
//! | [`AllowAllAuthorizer`] | developer mode, refused unless every listener is on loopback |
//! | [`BuiltinAuthorizer`] | single-org mode: model v1 in process, no OpenFGA (D1613) |
//! | [`RecordingAuthorizer`] | records every request and decision (tests, decision log) |
//!
//! [`http`] enforces it on HTTP routes, deny by default (D1611).

mod allow_all;
mod builtin;
mod cached;
pub mod http;
mod openfga;
mod recording;

use std::fmt;
use std::str::FromStr;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub use allow_all::{AllowAllAuthorizer, NotLoopback};
pub use builtin::BuiltinAuthorizer;
pub use cached::{CachedAuthorizer, MAX_CACHE_TTL};
pub use openfga::{MAX_CHECKS_PER_BATCH, OpenFgaAuthorizer, Tuple};
pub use recording::{Record, RecordingAuthorizer};

/// The three principal kinds of §19 and their OpenFGA types (D1605).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubjectKind {
    /// A person: `user:<zitadel user id>`.
    User,
    /// A machine that is not an AI agent (CI, an app's backend, an `lk_` key): `service_account:<id>`.
    ServiceAccount,
    /// An AI agent, a Zitadel machine user: `agent:<id>`. It reaches only what it is granted.
    Agent,
}

impl SubjectKind {
    /// The OpenFGA type name.
    pub fn fga_type(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::ServiceAccount => "service_account",
            Self::Agent => "agent",
        }
    }
}

/// Who is asking. `id` is the identity provider's subject (Zitadel's user id), never an email.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Subject {
    pub kind: SubjectKind,
    pub id: String,
    /// The org the token was issued in (`urn:zitadel:iam:user:resourceowner:id`).
    pub org: Option<String>,
    /// The RFC 8693 actor chain, outermost first: an agent acting for a person is the person
    /// with the agent in `act`. Every decision record carries it.
    pub act: Vec<Subject>,
    /// The agent session a token belongs to, for `agent with agent_session` grants (Q1204).
    pub session: Option<String>,
}

impl Subject {
    pub fn new(kind: SubjectKind, id: impl Into<String>) -> Self {
        Self {
            kind,
            id: id.into(),
            org: None,
            act: Vec::new(),
            session: None,
        }
    }

    pub fn user(id: impl Into<String>) -> Self {
        Self::new(SubjectKind::User, id)
    }

    pub fn agent(id: impl Into<String>) -> Self {
        Self::new(SubjectKind::Agent, id)
    }

    pub fn service_account(id: impl Into<String>) -> Self {
        Self::new(SubjectKind::ServiceAccount, id)
    }

    #[must_use]
    pub fn with_org(mut self, org: impl Into<String>) -> Self {
        self.org = Some(org.into());
        self
    }

    #[must_use]
    pub fn with_session(mut self, session: impl Into<String>) -> Self {
        self.session = Some(session.into());
        self
    }

    /// The subject as an OpenFGA user: `agent:<id>`.
    pub fn fga_user(&self) -> String {
        format!("{}:{}", self.kind.fga_type(), self.id)
    }
}

/// An OpenFGA object, `type:id` (`deployment:web`, `environment:<namespace id>`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ObjectRef {
    pub ty: String,
    pub id: String,
}

impl ObjectRef {
    pub fn new(ty: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            ty: ty.into(),
            id: id.into(),
        }
    }
}

impl fmt::Display for ObjectRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.ty, self.id)
    }
}

impl FromStr for ObjectRef {
    type Err = AuthzError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.split_once(':') {
            Some((ty, id)) if !ty.is_empty() && !id.is_empty() && !ty.contains('#') => {
                Ok(Self::new(ty, id))
            }
            _ => Err(AuthzError::Invalid(format!("{s:?} is not type:id"))),
        }
    }
}

/// OpenFGA's consistency preference (§3.2). `HigherConsistency` is for destructive, admin and
/// credential-issuing calls, and bypasses every cache (D1611).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Consistency {
    #[default]
    MinimizeLatency,
    HigherConsistency,
}

/// One question: may `subject` hold `relation` on `object`?
#[derive(Clone, Debug, PartialEq)]
pub struct CheckRequest {
    pub subject: Subject,
    /// A `can_*` relation in application code (D1603); other relations only in sync and admin
    /// code that manages tuples.
    pub relation: String,
    pub object: ObjectRef,
    /// Condition context (for example `{"session_id": …, "now": …}` for `agent_session`).
    pub context: serde_json::Map<String, serde_json::Value>,
    /// Tuples that hold for this request only.
    pub contextual_tuples: Vec<Tuple>,
    pub consistency: Consistency,
}

impl CheckRequest {
    pub fn new(subject: Subject, relation: impl Into<String>, object: ObjectRef) -> Self {
        Self {
            subject,
            relation: relation.into(),
            object,
            context: serde_json::Map::new(),
            contextual_tuples: Vec::new(),
            consistency: Consistency::MinimizeLatency,
        }
    }

    #[must_use]
    pub fn consistency(mut self, consistency: Consistency) -> Self {
        self.consistency = consistency;
        self
    }

    #[must_use]
    pub fn context(mut self, key: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        self.context.insert(key.into(), value.into());
        self
    }

    /// Whether a cached answer may serve this request: only a plain, low-latency check by a
    /// subject without an agent session. The cache key does not carry the session or the
    /// time, so a session's answer must never serve another session or outlive `expires`.
    pub fn cacheable(&self) -> bool {
        self.consistency == Consistency::MinimizeLatency
            && self.context.is_empty()
            && self.contextual_tuples.is_empty()
            && self.subject.session.is_none()
    }

    /// The condition context every backend evaluates (the same for OpenFGA and the built-in
    /// authorizer): the request's context, with `session_id` set to the authenticated
    /// subject's session when it has one (overriding the caller's), and `now` set to the
    /// server's clock when the caller gave none. `agent_session` grants (Q1204) then need no
    /// help from the caller.
    pub fn effective_context(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut ctx = self.context.clone();
        if let Some(session) = &self.subject.session {
            ctx.insert("session_id".into(), session.clone().into());
        }
        if !ctx.contains_key("now")
            && let Ok(now) = time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
        {
            ctx.insert("now".into(), now.into());
        }
        ctx
    }
}

/// The answer, with what the decision log needs (§11).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision {
    pub allowed: bool,
    /// The authorization model that decided, when the backend has one.
    pub model_id: Option<String>,
    /// Served from [`CachedAuthorizer`].
    pub cached: bool,
}

impl Decision {
    pub fn allow(model_id: Option<String>) -> Self {
        Self {
            allowed: true,
            model_id,
            cached: false,
        }
    }

    pub fn deny(model_id: Option<String>) -> Self {
        Self {
            allowed: false,
            model_id,
            cached: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AuthzError {
    /// The backend could not answer. Callers treat this as a deny, never an allow.
    #[error("authorizer unavailable: {0}")]
    Unavailable(String),
    /// The request or a tuple is malformed, or the model refuses it (a type restriction).
    #[error("invalid authorization request: {0}")]
    Invalid(String),
}

pub type Result<T, E = AuthzError> = std::result::Result<T, E>;

/// The one authorizer trait (D1610). Implementations must deny anything the model does not grant.
#[async_trait]
pub trait Authorizer: Send + Sync + 'static {
    async fn check(&self, req: CheckRequest) -> Result<Decision>;

    /// One decision per request, in order. The default runs the checks concurrently; OpenFGA
    /// overrides it with `BatchCheck`.
    async fn batch_check(&self, reqs: Vec<CheckRequest>) -> Result<Vec<Decision>> {
        futures::future::try_join_all(reqs.into_iter().map(|r| self.check(r))).await
    }

    /// The objects of type `ty` on which `subject` holds `relation` (D66's `filter_visible`).
    /// Small result sets only (OpenFGA stops at 1000 objects or 3 s); large listings filter by
    /// org in the database and then `batch_check` (§9.3).
    async fn list_objects(
        &self,
        subject: &Subject,
        relation: &str,
        ty: &str,
    ) -> Result<Vec<ObjectRef>>;
}

#[async_trait]
impl<A: Authorizer + ?Sized> Authorizer for std::sync::Arc<A> {
    async fn check(&self, req: CheckRequest) -> Result<Decision> {
        (**self).check(req).await
    }

    async fn batch_check(&self, reqs: Vec<CheckRequest>) -> Result<Vec<Decision>> {
        (**self).batch_check(reqs).await
    }

    async fn list_objects(
        &self,
        subject: &Subject,
        relation: &str,
        ty: &str,
    ) -> Result<Vec<ObjectRef>> {
        (**self).list_objects(subject, relation, ty).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subject_fga_user() {
        assert_eq!(Subject::user("u1").fga_user(), "user:u1");
        assert_eq!(Subject::agent("a1").fga_user(), "agent:a1");
        assert_eq!(
            Subject::service_account("s1").fga_user(),
            "service_account:s1"
        );
    }

    #[test]
    fn object_ref_round_trip() {
        let o: ObjectRef = "deployment:web-1".parse().unwrap();
        assert_eq!(o, ObjectRef::new("deployment", "web-1"));
        assert_eq!(o.to_string(), "deployment:web-1");
        let g: ObjectRef = "graph:ns/kg".parse().unwrap();
        assert_eq!(g.id, "ns/kg");
        for bad in ["deployment", ":x", "deployment:", "team#member:x"] {
            assert!(bad.parse::<ObjectRef>().is_err(), "{bad}");
        }
    }

    #[test]
    fn only_plain_low_latency_checks_are_cacheable() {
        let base = CheckRequest::new(
            Subject::user("u"),
            "can_view",
            ObjectRef::new("deployment", "d"),
        );
        assert!(base.cacheable());
        assert!(
            !base
                .clone()
                .consistency(Consistency::HigherConsistency)
                .cacheable()
        );
        assert!(!base.clone().context("session_id", "s").cacheable());
        let mut with_session = base;
        with_session.subject = Subject::agent("a").with_session("s1");
        assert!(
            !with_session.cacheable(),
            "a session's allow must not serve another session"
        );
    }

    #[test]
    fn effective_context_takes_the_session_from_the_subject() {
        let req = CheckRequest::new(
            Subject::agent("a").with_session("s-real"),
            "can_view",
            ObjectRef::new("deployment", "d"),
        )
        .context("session_id", "s-claimed");
        let ctx = req.effective_context();
        assert_eq!(ctx["session_id"], "s-real");
        assert!(ctx["now"].as_str().unwrap().ends_with('Z'));
        let fixed = CheckRequest::new(Subject::user("u"), "can_view", ObjectRef::new("d", "x"))
            .context("now", "2026-10-21T12:00:00Z");
        assert_eq!(fixed.effective_context()["now"], "2026-10-21T12:00:00Z");
        assert!(!fixed.effective_context().contains_key("session_id"));
    }
}
