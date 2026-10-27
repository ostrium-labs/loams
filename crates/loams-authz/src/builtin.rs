//! [`BuiltinAuthorizer`]: model v1 evaluated in process, for built-in single-org mode (D1613).
//!
//! A single-user install runs no OpenFGA. This authorizer answers the same `can_*` questions
//! from the same tuples, with the same rewrite rules as `deploy/openfga/core.fga` and
//! `graph.fga`, transcribed in [`MODEL`]. `builtin_matches_openfga` (tests/builtin.rs) runs
//! every case of `deploy/openfga/core.fga.yaml` against this and against OpenFGA, so the two
//! cannot drift apart unnoticed.
//!
//! Writes enforce the model's type restrictions, so `agent` can never hold `billing_admin`,
//! `meter_admin` or `operator` here either (D1607).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::RwLock;

use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::{Authorizer, AuthzError, CheckRequest, Decision, ObjectRef, Result, Subject, Tuple};

/// What a direct tuple may name as its user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Allowed {
    /// `[user]`: `user:<id>`.
    Type(&'static str),
    /// `[team#member]`: the members of a team.
    Userset(&'static str, &'static str),
    /// `[agent with agent_session]`: an agent, under a condition.
    WithCondition(&'static str, &'static str),
}

/// One branch of a relation's `or`.
#[derive(Debug, Clone, Copy)]
enum Rule {
    Direct(&'static [Allowed]),
    /// `or editor`: another relation on the same object.
    Computed(&'static str),
    /// `or admin from org`: `relation` on the object(s) the tupleset relation points at.
    From(&'static str, &'static str),
}

use Allowed::{Type, Userset, WithCondition};
use Rule::{Computed, Direct, From};

const PEOPLE_AND_TEAMS: &[Allowed] = &[Type("user"), Userset("team", "member")];
const PROJECT_GRANTEES: &[Allowed] = &[
    Type("user"),
    Userset("team", "member"),
    Type("agent"),
    Type("service_account"),
];
const ENV_GRANTEES: &[Allowed] = &[
    Type("user"),
    Userset("team", "member"),
    Type("agent"),
    WithCondition("agent", "agent_session"),
    Type("service_account"),
];

/// Model v1: `(type, relation, rules)`. Keep in step with `deploy/openfga/core.fga` and
/// `graph.fga`; the differential test fails when they differ on any tested case.
const MODEL: &[(&str, &str, &[Rule])] = &[
    ("platform", "operator", &[Direct(&[Type("user")])]),
    (
        "platform",
        "limits_writer",
        &[Direct(&[Type("user"), Type("service_account")])],
    ),
    ("platform", "meter_admin", &[Direct(&[Type("user")])]),
    ("platform", "cloud_admin", &[Direct(&[Type("user")])]),
    ("org", "platform", &[Direct(&[Type("platform")])]),
    ("org", "owner", &[Direct(&[Type("user")])]),
    (
        "org",
        "admin",
        &[Direct(PEOPLE_AND_TEAMS), Computed("owner")],
    ),
    (
        "org",
        "member",
        &[Direct(PEOPLE_AND_TEAMS), Computed("admin")],
    ),
    (
        "org",
        "billing_admin",
        &[Direct(&[Type("user")]), Computed("owner")],
    ),
    (
        "org",
        "billing_viewer",
        &[Direct(PEOPLE_AND_TEAMS), Computed("billing_admin")],
    ),
    (
        "org",
        "namespace_editor",
        &[Direct(&[Type("service_account")]), Computed("member")],
    ),
    ("org", "can_view", &[Computed("namespace_editor")]),
    ("org", "can_admin", &[Computed("admin")]),
    ("org", "can_run_durable", &[Computed("namespace_editor")]),
    ("team", "org", &[Direct(&[Type("org")])]),
    ("team", "member", &[Direct(PEOPLE_AND_TEAMS)]),
    ("agent", "owner", &[Direct(&[Type("user")])]),
    ("agent", "project", &[Direct(&[Type("project")])]),
    ("service_account", "project", &[Direct(&[Type("project")])]),
    ("project", "org", &[Direct(&[Type("org")])]),
    (
        "project",
        "admin",
        &[Direct(PEOPLE_AND_TEAMS), From("org", "admin")],
    ),
    (
        "project",
        "editor",
        &[Direct(PROJECT_GRANTEES), Computed("admin")],
    ),
    (
        "project",
        "viewer",
        &[Direct(PROJECT_GRANTEES), Computed("editor")],
    ),
    ("project", "can_admin", &[Computed("admin")]),
    ("environment", "project", &[Direct(&[Type("project")])]),
    ("environment", "org", &[Direct(&[Type("org")])]),
    (
        "environment",
        "deployer",
        &[
            Direct(ENV_GRANTEES),
            From("project", "admin"),
            From("org", "admin"),
        ],
    ),
    (
        "environment",
        "editor",
        &[
            Direct(ENV_GRANTEES),
            Computed("deployer"),
            From("project", "editor"),
            From("org", "namespace_editor"),
        ],
    ),
    (
        "environment",
        "viewer",
        &[
            Direct(ENV_GRANTEES),
            Computed("editor"),
            From("project", "viewer"),
        ],
    ),
    ("environment", "can_read", &[Computed("viewer")]),
    ("environment", "can_write", &[Computed("editor")]),
    ("environment", "can_admin", &[Computed("deployer")]),
    (
        "deployment",
        "environment",
        &[Direct(&[Type("environment")])],
    ),
    ("deployment", "can_view", &[From("environment", "viewer")]),
    (
        "deployment",
        "can_operate",
        &[From("environment", "editor")],
    ),
    (
        "deployment",
        "can_destroy",
        &[From("environment", "deployer")],
    ),
    ("database", "environment", &[Direct(&[Type("environment")])]),
    ("database", "can_read", &[From("environment", "viewer")]),
    ("database", "can_write", &[From("environment", "editor")]),
    ("database", "can_admin", &[From("environment", "deployer")]),
    ("database", "can_connect", &[From("environment", "editor")]),
    ("engine", "environment", &[Direct(&[Type("environment")])]),
    ("engine", "can_view", &[From("environment", "viewer")]),
    ("engine", "can_operate", &[From("environment", "editor")]),
    ("engine", "can_admin", &[From("environment", "deployer")]),
    ("engine", "can_connect", &[From("environment", "editor")]),
    ("graph", "environment", &[Direct(&[Type("environment")])]),
    (
        "graph",
        "reader",
        &[
            Direct(&[Type("user"), Type("service_account"), Type("agent")]),
            From("environment", "viewer"),
        ],
    ),
    (
        "graph",
        "writer",
        &[
            Direct(&[Type("user"), Type("service_account"), Type("agent")]),
            From("environment", "editor"),
        ],
    ),
    (
        "graph",
        "admin",
        &[
            Direct(&[Type("user"), Type("service_account")]),
            From("environment", "deployer"),
        ],
    ),
    (
        "graph",
        "can_read",
        &[Computed("reader"), Computed("writer"), Computed("admin")],
    ),
    (
        "graph",
        "can_write",
        &[Computed("writer"), Computed("admin")],
    ),
    ("graph", "can_admin", &[Computed("admin")]),
];

/// OpenFGA's default resolution depth.
const MAX_DEPTH: u32 = 25;

fn rules_of(ty: &str, relation: &str) -> Option<&'static [Rule]> {
    MODEL
        .iter()
        .find(|(t, r, _)| *t == ty && *r == relation)
        .map(|(_, _, rules)| *rules)
}

fn type_of(s: &str) -> &str {
    s.split(':').next().unwrap_or_default()
}

/// A stored tuple: its user (`agent:a1` or `team:eng#member`) and condition.
#[derive(Debug, Clone, PartialEq)]
struct Stored {
    user: String,
    condition: Option<(String, Map<String, Value>)>,
}

#[derive(Debug, Default)]
struct Store {
    /// `(object, relation)` → tuples.
    by_object: HashMap<(String, String), Vec<Stored>>,
}

#[derive(Debug, Default)]
pub struct BuiltinAuthorizer {
    store: RwLock<Store>,
}

impl BuiltinAuthorizer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Writes tuples, idempotently, after checking each against the model's type restrictions.
    /// Nothing is written if any tuple is refused.
    pub fn write(&self, tuples: &[Tuple]) -> Result<()> {
        for t in tuples {
            validate(t)?;
        }
        let mut store = self
            .store
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for t in tuples {
            let slot = store
                .by_object
                .entry((t.object.clone(), t.relation.clone()))
                .or_default();
            slot.retain(|s| s.user != t.user);
            slot.push(Stored {
                user: t.user.clone(),
                condition: t.condition.clone(),
            });
        }
        Ok(())
    }

    /// Deletes tuples; a missing one is not an error.
    pub fn delete(&self, tuples: &[Tuple]) {
        let mut store = self
            .store
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for t in tuples {
            if let Some(slot) = store
                .by_object
                .get_mut(&(t.object.clone(), t.relation.clone()))
            {
                slot.retain(|s| s.user != t.user);
            }
        }
    }

    fn eval(
        &self,
        store: &Store,
        user: &str,
        relation: &str,
        object: &str,
        ctx: &Map<String, Value>,
        depth: u32,
    ) -> Result<bool> {
        if depth > MAX_DEPTH {
            return Err(AuthzError::Invalid("resolution depth exceeded".into()));
        }
        let ty = type_of(object);
        let Some(rules) = rules_of(ty, relation) else {
            return Err(AuthzError::Invalid(format!(
                "type {ty} has no relation {relation}"
            )));
        };
        for rule in rules {
            let hit = match *rule {
                Direct(allowed) => {
                    self.direct(store, allowed, user, relation, object, ctx, depth)?
                }
                Computed(rel) => self.eval(store, user, rel, object, ctx, depth + 1)?,
                From(tupleset, rel) => {
                    let parents: Vec<String> = store
                        .by_object
                        .get(&(object.to_owned(), tupleset.to_owned()))
                        .map(|v| v.iter().map(|s| s.user.clone()).collect())
                        .unwrap_or_default();
                    let mut any = false;
                    for p in parents {
                        // A parent whose type lacks the relation contributes nothing (OpenFGA too).
                        if rules_of(type_of(&p), rel).is_some()
                            && self.eval(store, user, rel, &p, ctx, depth + 1)?
                        {
                            any = true;
                            break;
                        }
                    }
                    any
                }
            };
            if hit {
                return Ok(true);
            }
        }
        Ok(false)
    }

    #[allow(clippy::too_many_arguments)]
    fn direct(
        &self,
        store: &Store,
        allowed: &[Allowed],
        user: &str,
        relation: &str,
        object: &str,
        ctx: &Map<String, Value>,
        depth: u32,
    ) -> Result<bool> {
        let Some(tuples) = store
            .by_object
            .get(&(object.to_owned(), relation.to_owned()))
        else {
            return Ok(false);
        };
        for t in tuples {
            if let Some((set, rel)) = t.user.split_once('#') {
                if allows_userset(allowed, type_of(set), rel)
                    && self.eval(store, user, rel, set, ctx, depth + 1)?
                {
                    return Ok(true);
                }
                continue;
            }
            if t.user != user {
                continue;
            }
            let ok = match &t.condition {
                None => allows_type(allowed, type_of(user)),
                Some((name, stored)) => {
                    allows_condition(allowed, type_of(user), name) && condition(name, stored, ctx)?
                }
            };
            if ok {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

fn allows_type(allowed: &[Allowed], ty: &str) -> bool {
    allowed.iter().any(|a| matches!(a, Type(t) if *t == ty))
}

fn allows_userset(allowed: &[Allowed], ty: &str, rel: &str) -> bool {
    allowed
        .iter()
        .any(|a| matches!(a, Userset(t, r) if *t == ty && *r == rel))
}

fn allows_condition(allowed: &[Allowed], ty: &str, condition: &str) -> bool {
    allowed
        .iter()
        .any(|a| matches!(a, WithCondition(t, c) if *t == ty && *c == condition))
}

/// The model's one condition, `agent_session`:
/// `session_id == allowed_session && now < expires`. A missing parameter does not satisfy it.
fn condition(
    name: &str,
    stored: &Map<String, Value>,
    request: &Map<String, Value>,
) -> Result<bool> {
    if name != "agent_session" {
        return Err(AuthzError::Invalid(format!("unknown condition {name}")));
    }
    let get = |k: &str| {
        stored
            .get(k)
            .or_else(|| request.get(k))
            .and_then(Value::as_str)
    };
    let (Some(session), Some(allowed), Some(now), Some(expires)) = (
        get("session_id"),
        get("allowed_session"),
        get("now"),
        get("expires"),
    ) else {
        return Ok(false);
    };
    let ts = |s: &str| {
        time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_err(|e| AuthzError::Invalid(format!("timestamp {s:?}: {e}")))
    };
    Ok(session == allowed && ts(now)? < ts(expires)?)
}

/// Whether `s` is `type:id` with a non-empty type and id, no `#`, no whitespace, and no
/// wildcard (model v1 declares no `type:*`).
fn well_formed(s: &str) -> bool {
    s.split_once(':').is_some_and(|(ty, id)| {
        !ty.is_empty()
            && !id.is_empty()
            && id != "*"
            && !s.contains('#')
            && !s.chars().any(char::is_whitespace)
    })
}

/// The model's type restrictions on a written tuple.
fn validate(t: &Tuple) -> Result<()> {
    let ty = type_of(&t.object);
    let invalid = |why: String| {
        AuthzError::Invalid(format!(
            "Invalid tuple '{}#{}@{}'. Reason: {why}",
            t.object, t.relation, t.user
        ))
    };
    let user_object = t
        .user
        .split_once('#')
        .map_or(t.user.as_str(), |(set, _)| set);
    if !well_formed(&t.object) || !well_formed(user_object) {
        return Err(invalid("the object and user must be type:id".into()));
    }
    t.check_condition()?;
    let Some(rules) = rules_of(ty, &t.relation) else {
        return Err(invalid(format!(
            "type '{ty}' has no relation '{}'",
            t.relation
        )));
    };
    let allowed: Vec<Allowed> = rules
        .iter()
        .filter_map(|r| if let Direct(a) = r { Some(*a) } else { None })
        .flatten()
        .copied()
        .collect();
    let user_ty = type_of(&t.user);
    let ok = match (t.user.split_once('#'), &t.condition) {
        (Some((set, rel)), None) => allows_userset(&allowed, type_of(set), rel),
        (Some(_), Some(_)) => false,
        (None, None) => allows_type(&allowed, user_ty),
        (None, Some((c, _))) => allows_condition(&allowed, user_ty, c),
    };
    if ok {
        Ok(())
    } else {
        Err(invalid(format!(
            "type '{user_ty}' is not an allowed type restriction for '{ty}#{}'",
            t.relation
        )))
    }
}

#[async_trait]
impl Authorizer for BuiltinAuthorizer {
    async fn check(&self, req: CheckRequest) -> Result<Decision> {
        let store = self
            .store
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let ctx = req.effective_context();
        let allowed = if req.contextual_tuples.is_empty() {
            self.eval(
                &store,
                &req.subject.fga_user(),
                &req.relation,
                &req.object.to_string(),
                &ctx,
                0,
            )?
        } else {
            drop(store);
            let overlay = BuiltinAuthorizer::new();
            {
                let base = self
                    .store
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                overlay
                    .store
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .by_object
                    .clone_from(&base.by_object);
            }
            overlay.write(&req.contextual_tuples)?;
            let s = overlay
                .store
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            overlay.eval(
                &s,
                &req.subject.fga_user(),
                &req.relation,
                &req.object.to_string(),
                &ctx,
                0,
            )?
        };
        Ok(if allowed {
            Decision::allow(None)
        } else {
            Decision::deny(None)
        })
    }

    async fn list_objects(
        &self,
        subject: &Subject,
        relation: &str,
        ty: &str,
    ) -> Result<Vec<ObjectRef>> {
        let candidates: BTreeSet<String> = {
            let store = self
                .store
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut seen = HashSet::new();
            for ((object, _), tuples) in &store.by_object {
                seen.insert(object.clone());
                for t in tuples {
                    seen.insert(t.user.split('#').next().unwrap_or_default().to_owned());
                }
            }
            seen.into_iter().filter(|o| type_of(o) == ty).collect()
        };
        let mut out = Vec::new();
        for o in candidates {
            let object: ObjectRef = o.parse()?;
            if self
                .check(CheckRequest::new(subject.clone(), relation, object.clone()))
                .await?
                .allowed
            {
                out.push(object);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_rule_names_a_relation_the_model_has() {
        for (ty, rel, rules) in MODEL {
            for r in *rules {
                match r {
                    Computed(c) => assert!(super::rules_of(ty, c).is_some(), "{ty}#{rel}: {c}"),
                    From(ts, _) => assert!(super::rules_of(ty, ts).is_some(), "{ty}#{rel}: {ts}"),
                    Direct(_) => {}
                }
            }
        }
    }

    #[test]
    fn builtin_refuses_agent_billing_admin() {
        let b = BuiltinAuthorizer::new();
        for (rel, obj) in [
            ("billing_admin", "org:o"),
            ("meter_admin", "platform:default"),
            ("operator", "platform:default"),
        ] {
            let e = b.write(&[Tuple::new("agent:a", rel, obj)]).unwrap_err();
            assert!(
                e.to_string().contains("not an allowed type restriction"),
                "{e}"
            );
        }
        assert!(
            b.write(&[Tuple::new("agent:a", "editor", "environment:e")])
                .is_ok()
        );
        assert!(
            b.write(&[Tuple::new("user:u", "editor", "environment:e")
                .with_condition("agent_session", Map::new())])
                .is_err()
        );
    }

    #[test]
    fn builtin_refuses_malformed_and_wildcard_tuples() {
        let b = BuiltinAuthorizer::new();
        for t in [
            Tuple::new("user:*", "viewer", "environment:e"),
            Tuple::new("user:", "viewer", "environment:e"),
            Tuple::new("user:u", "viewer", "environment"),
            Tuple::new("user:u", "viewer", "environment:e#x"),
            Tuple::new("user:u v", "viewer", "environment:e"),
            Tuple::new("agent:a", "viewer", "environment:e").with_condition(
                "agent_session",
                serde_json::json!({"allowed_session": "s", "expires": "2030-01-01T00:00:00Z",
                    "now": "2026-01-01T00:00:00Z"})
                .as_object()
                .unwrap()
                .clone(),
            ),
            Tuple::new("agent:a", "viewer", "environment:e").with_condition(
                "agent_session",
                serde_json::json!({"allowed_session": "s", "session_id": "s"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        ] {
            assert!(b.write(std::slice::from_ref(&t)).is_err(), "{t:?}");
        }
    }

    #[tokio::test]
    async fn session_grant_uses_the_subjects_session_and_the_clock() {
        let b = BuiltinAuthorizer::new();
        let grant = |expires: &str| {
            Tuple::new("agent:a", "viewer", "environment:e").with_condition(
                "agent_session",
                serde_json::json!({"allowed_session": "s1", "expires": expires})
                    .as_object()
                    .unwrap()
                    .clone(),
            )
        };
        b.write(&[grant("2999-01-01T00:00:00Z")]).unwrap();
        let ask = |s: Subject| CheckRequest::new(s, "viewer", "environment:e".parse().unwrap());
        assert!(
            b.check(ask(Subject::agent("a").with_session("s1")))
                .await
                .unwrap()
                .allowed
        );
        assert!(
            !b.check(ask(Subject::agent("a").with_session("s2")).context("session_id", "s1"))
                .await
                .unwrap()
                .allowed,
            "the caller cannot claim another session"
        );
        b.write(&[grant("2020-01-01T00:00:00Z")]).unwrap();
        assert!(
            !b.check(ask(Subject::agent("a").with_session("s1")))
                .await
                .unwrap()
                .allowed,
            "expired by the server's clock"
        );
    }

    #[tokio::test]
    async fn agent_reaches_only_its_grant_and_not_its_owner() {
        let b = BuiltinAuthorizer::new();
        b.write(&[
            Tuple::new("org:o", "org", "project:p"),
            Tuple::new("project:p", "project", "environment:dev"),
            Tuple::new("project:p", "project", "environment:prod"),
            Tuple::new("environment:dev", "environment", "deployment:d"),
            Tuple::new("environment:prod", "environment", "deployment:x"),
            Tuple::new("user:pia", "admin", "project:p"),
            Tuple::new("user:pia", "owner", "agent:a"),
            Tuple::new("agent:a", "editor", "environment:dev"),
        ])
        .unwrap();
        let can = |rel: &str, obj: &str| {
            CheckRequest::new(Subject::agent("a"), rel, obj.parse().unwrap())
        };
        assert!(
            b.check(can("can_operate", "deployment:d"))
                .await
                .unwrap()
                .allowed
        );
        assert!(
            !b.check(can("can_destroy", "deployment:d"))
                .await
                .unwrap()
                .allowed
        );
        assert!(
            !b.check(can("can_view", "deployment:x"))
                .await
                .unwrap()
                .allowed,
            "owner pia's rights are not the agent's"
        );
        assert_eq!(
            b.list_objects(&Subject::agent("a"), "can_view", "deployment")
                .await
                .unwrap(),
            vec![ObjectRef::new("deployment", "d")]
        );
        b.delete(&[Tuple::new("agent:a", "editor", "environment:dev")]);
        assert!(
            !b.check(can("can_view", "deployment:d"))
                .await
                .unwrap()
                .allowed,
            "deny after revoke"
        );
    }
}
