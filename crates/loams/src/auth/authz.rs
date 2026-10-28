//! The engine's authorization through `loams-authz` (design 76 §9, D1610, D1613; plan IA1,
//! IA2 Task 4).
//!
//! Application code asks one question, through [`loams_authz::Authorizer`]: may this
//! [`Subject`] hold this `can_*` relation on this object? This module answers the two halves
//! the engine owns:
//!
//! - **Who is asking** ([`standing`]): an admitted [`Principal`] becomes a `Subject`, or the
//!   operator, or nobody in the org.
//!   - A JWT whose `(iss, sub)` has a principal link ([`super::directory::PrincipalLink`]) is
//!     the link's kind: `user`, `agent` or `service_account` (D1628). A disabled link never
//!     gets here: [`super::Auth::authenticate`] refuses it on every request (D1624).
//!   - A JWT with no link, and a tailnet user, is the member with their verified email (as
//!     today; an invitation is accepted on the first call), or nobody (D1605's re-link by
//!     email until the identity profile writes links).
//!   - A console session is its person: `user:<user id>`.
//!   - A `KeyService` key acts as its principal: a service account's key is
//!     `service_account:<id>`, a key acting as its creator is `user:<id>`.
//!   - The operator (the developer on loopback with `LOAMS_AUTH_MODE=off`, a key from
//!     `LOAMS_API_KEYS_FILE`, any API key before first-run setup) is not a subject: it reaches
//!     everything, as it does today.
//! - **What the principal store grants** ([`tuples_of`]): in built-in single-org mode
//!   (`LOAMS_AUTHZ=builtin`, the default, D1613) the store is the tuple source, and
//!   [`DirectoryAuthorizer`] evaluates model v1 over it in process. With `LOAMS_AUTHZ=openfga`
//!   the same questions go to OpenFGA, behind a 5 s allow cache (D1625); its tuples are written
//!   by the sync bridge (IA3), not from here.
//!
//! A namespace that is no project's environment belongs to the org (`environment#org`, model
//! v1 as amended by IA2 Task 4). The store does not list namespaces, so a check on one adds
//! that tuple as a contextual tuple ([`environment_check`]).

use std::fmt;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use async_trait::async_trait;
use loams_authz::{
    Authorizer, AuthzError, BuiltinAuthorizer, CachedAuthorizer, CheckRequest, Decision, ObjectRef,
    OpenFgaAuthorizer, Subject, Tuple,
};
use loams_proto::loams::admin::v1 as pb;

use super::authorizer::Denied;
use super::directory::{Directory, Doc, LinkKind, ProjectRec, Role};
use super::{Principal, PrincipalKind, Principals};

/// The environment variable that picks the authorizer.
pub const AUTHZ_ENV: &str = "LOAMS_AUTHZ";

/// How long an OpenFGA allow is cached (D66, D1625: one cache, at most 5 s).
pub const OPENFGA_CACHE_TTL: Duration = Duration::from_secs(5);

/// Which authorizer answers (`LOAMS_AUTHZ`).
#[derive(Clone, Default, PartialEq, Eq)]
pub enum AuthzConfig {
    /// `builtin` (the default): model v1 in process over the principal store (D1613).
    #[default]
    Builtin,
    /// `openfga`: OpenFGA over gRPC (`LOAMS_AUTHZ_OPENFGA_GRPC`, `_STORE`, `_MODEL`, and
    /// `_KEY` when it requires a pre-shared key), with a 5 s allow cache.
    OpenFga {
        grpc: String,
        store: String,
        model: String,
        key: Option<String>,
    },
    /// `off`: every check allows. Accepted only with `LOAMS_AUTH_MODE=off`, whose listeners
    /// are all loopback.
    Off,
}

impl fmt::Debug for AuthzConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Builtin => f.write_str("Builtin"),
            Self::OpenFga {
                grpc,
                store,
                model,
                key,
            } => f
                .debug_struct("OpenFga")
                .field("grpc", grpc)
                .field("store", store)
                .field("model", model)
                .field("key", &key.as_ref().map(|_| "[redacted]"))
                .finish(),
            Self::Off => f.write_str("Off"),
        }
    }
}

impl AuthzConfig {
    /// Reads `LOAMS_AUTHZ` and `LOAMS_AUTHZ_OPENFGA_*` through `var`. `auth_off` is whether
    /// `LOAMS_AUTH_MODE` is off. Errors name the variable.
    pub fn from_vars(var: impl Fn(&str) -> Option<String>, auth_off: bool) -> Result<Self, String> {
        let get = |name: &str| {
            var(name)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        match get(AUTHZ_ENV)
            .as_deref()
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            None | Some("builtin") => Ok(Self::Builtin),
            Some("openfga") => {
                let required = |name: &str| {
                    get(name).ok_or_else(|| format!("{AUTHZ_ENV}=openfga needs {name}"))
                };
                Ok(Self::OpenFga {
                    grpc: required("LOAMS_AUTHZ_OPENFGA_GRPC")?,
                    store: required("LOAMS_AUTHZ_OPENFGA_STORE")?,
                    model: required("LOAMS_AUTHZ_OPENFGA_MODEL")?,
                    key: get("LOAMS_AUTHZ_OPENFGA_KEY"),
                })
            }
            Some("off") if auth_off => Ok(Self::Off),
            Some("off") => Err(format!(
                "{AUTHZ_ENV}=off allows every request; it is accepted only with \
                 LOAMS_AUTH_MODE=off, whose listeners are all loopback"
            )),
            Some(other) => Err(format!(
                "{AUTHZ_ENV}={other:?} is not one of builtin, openfga, off"
            )),
        }
    }

    /// The authorizer this config names, over `directory` for `builtin`.
    ///
    /// # Errors
    ///
    /// A malformed OpenFGA endpoint or key.
    pub fn build(&self, directory: &Arc<Directory>) -> Result<Arc<dyn Authorizer>, String> {
        Ok(match self {
            Self::Builtin => Arc::new(DirectoryAuthorizer::new(directory.clone())),
            Self::OpenFga {
                grpc,
                store,
                model,
                key,
            } => {
                let fga = OpenFgaAuthorizer::connect_lazy(grpc, store, model, key.as_deref())
                    .map_err(|e| format!("LOAMS_AUTHZ_OPENFGA_GRPC: {e}"))?;
                Arc::new(CachedAuthorizer::new(fga, OPENFGA_CACHE_TTL, 100_000))
            }
            Self::Off => Arc::new(AllowAll),
        })
    }
}

/// `LOAMS_AUTHZ=off`, behind `LOAMS_AUTH_MODE=off` (loopback only, which
/// [`super::AuthConfig::check_listeners`] enforces before anything listens).
#[derive(Debug)]
struct AllowAll;

#[async_trait]
impl Authorizer for AllowAll {
    async fn check(&self, _req: CheckRequest) -> loams_authz::Result<Decision> {
        Ok(Decision::allow(None))
    }

    async fn list_objects(
        &self,
        _: &Subject,
        _: &str,
        _: &str,
    ) -> loams_authz::Result<Vec<ObjectRef>> {
        Ok(Vec::new())
    }
}

/// Who is asking, as far as authorization goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Standing {
    /// The operator's own credentials: everything.
    Operator,
    /// A principal of the org, checked through the authorizer.
    Subject(Subject),
    /// Nobody the org knows (outside it, a disabled agent, or before first-run setup).
    Outside,
}

/// The standing of `principal` (module docs).
///
/// # Errors
///
/// [`Denied::Unavailable`] when the principal store cannot be read.
pub async fn standing(principals: &Principals, principal: &Principal) -> Result<Standing, Denied> {
    if principal.kind == PrincipalKind::DevLoopback {
        return Ok(Standing::Operator);
    }
    let doc = principals.directory.read().await?;
    let Some(org_id) = doc.org.as_ref().map(|o| o.id.clone()) else {
        // Before first-run setup the only keys are the operator's own.
        return Ok(if principal.kind == PrincipalKind::ApiKey {
            Standing::Operator
        } else {
            Standing::Outside
        });
    };
    let in_org = |s: Subject| Standing::Subject(s.with_org(org_id.clone()));
    if let Some(issuer) = principal.issuer.as_deref()
        && let Some(link) = doc.link(issuer, &principal.subject)
    {
        if link.disabled {
            return Ok(Standing::Outside);
        }
        return Ok(match link.kind {
            LinkKind::User => in_org(Subject::user(&link.principal)),
            LinkKind::ServiceAccount => in_org(Subject::service_account(&link.principal)),
            LinkKind::Agent => match doc.agent(&link.principal) {
                Some(rec)
                    if rec.agent.status.as_known()
                        != Some(pb::AgentStatus::AGENT_STATUS_SUSPENDED) =>
                {
                    in_org(Subject::agent(&link.principal))
                }
                _ => Standing::Outside,
            },
        });
    }
    Ok(match principal.kind {
        PrincipalKind::DevLoopback => Standing::Operator,
        PrincipalKind::Session => match doc.member(&principal.subject) {
            Some(member) => in_org(Subject::user(&member.user.id)),
            None => Standing::Outside,
        },
        PrincipalKind::ApiKey => {
            let id = principal
                .subject
                .strip_prefix("apikey:")
                .unwrap_or(&principal.subject);
            match doc.key(id) {
                None => Standing::Operator,
                Some(key) => {
                    let who = key.key.principal.as_option().cloned().unwrap_or_default();
                    match who.kind.as_known() {
                        Some(pb::SubjectKind::SUBJECT_KIND_USER) => in_org(Subject::user(who.id)),
                        Some(pb::SubjectKind::SUBJECT_KIND_SERVICE_ACCOUNT) | None => {
                            in_org(Subject::service_account(who.id))
                        }
                        Some(_) => Standing::Outside,
                    }
                }
            }
        }
        PrincipalKind::User | PrincipalKind::Tailscale => {
            let sub_is_a_member_id = doc.member(&principal.subject).is_some();
            drop(doc);
            let unlinked = || {
                // D1628: a JWT subject with no link and no member is `user:<sub>` (in OpenFGA
                // mode the sync writes Zitadel's user ids), unless that id is a Loams member's
                // own, which another issuer's `sub` must never stand for.
                match principal.issuer.as_deref() {
                    Some(_) if sub_is_a_member_id => Standing::Outside,
                    Some(_) => in_org(Subject::user(&principal.subject)),
                    None => Standing::Outside,
                }
            };
            match principals
                .authorizer
                .authorize(principal, Role::Member)
                .await
            {
                Ok(actor) => match actor.user {
                    Some(user) => in_org(Subject::user(user)),
                    None => unlinked(),
                },
                Err(Denied::SetupRequired) => Standing::Outside,
                Err(Denied::RoleRequired { .. }) => unlinked(),
                Err(err @ Denied::Unavailable(_)) => return Err(err),
            }
        }
    })
}

/// A project's OpenFGA object id: its id, or its slug when it has none.
pub fn project_id(project: &ProjectRec) -> &str {
    if project.project.id.is_empty() {
        &project.project.slug
    } else {
        &project.project.id
    }
}

/// The project whose environment `namespace` is.
pub fn project_of<'d>(doc: &'d Doc, namespace: &str) -> Option<&'d ProjectRec> {
    doc.projects
        .iter()
        .find(|p| p.environments.iter().any(|e| e.namespace == namespace))
}

/// A check of `relation` on `environment:<namespace>`, with the contextual tuple that roots a
/// namespace that is no project's environment in the org.
pub fn environment_check(
    doc: &Doc,
    subject: &Subject,
    relation: &str,
    namespace: &str,
) -> CheckRequest {
    let mut req = CheckRequest::new(
        subject.clone(),
        relation,
        ObjectRef::new("environment", namespace),
    );
    req.contextual_tuples = namespace_roots(doc, namespace);
    req
}

/// The contextual tuple that roots a namespace that is no project's environment in the org (the
/// engine writes no tuple per namespace).
pub fn namespace_roots(doc: &Doc, namespace: &str) -> Vec<Tuple> {
    match (&doc.org, project_of(doc, namespace)) {
        (Some(org), None) => vec![Tuple::new(
            format!("org:{}", org.id),
            "org",
            format!("environment:{namespace}"),
        )],
        _ => Vec::new(),
    }
}

/// The relation a project role grants (`PROJECT_ROLE_DEVELOPER` is model v1's `editor`).
fn project_relation(role: pb::ProjectRole) -> Option<&'static str> {
    match role {
        pb::ProjectRole::PROJECT_ROLE_ADMIN => Some("admin"),
        pb::ProjectRole::PROJECT_ROLE_DEVELOPER => Some("editor"),
        pb::ProjectRole::PROJECT_ROLE_VIEWER => Some("viewer"),
        _ => None,
    }
}

/// The tuples the principal store grants, in model v1 (built-in single-org mode, D1613).
///
/// - Members are `owner`, `admin` or `member` of `org:<id>`; teams are rooted in the org, and
///   their people are `member`s.
/// - Every project is rooted in the org (`project#org`), so the org's owners and admins are
///   its `admin`s (`admin from org`). An org member is a viewer of only the projects they are
///   granted: model v1's `project.viewer` has no `member from org` (plan IA1, ruling T4.R2,
///   option 2).
/// - A grant is its subject's relation on the project (`admin`, `editor`, `viewer`). An agent
///   or a service account is never a project `admin` (the model's type restriction); such a
///   grant is skipped.
/// - Environments hang off their project; agents and service accounts are rooted in theirs.
///   A service account (and a `KeyService` key acting as one) is its project's `editor`, and
///   edits the org's own namespaces (`org#namespace_editor`).
/// - An agent's `owner` is written and confers nothing (D1607).
pub fn tuples_of(doc: &Doc) -> Vec<Tuple> {
    let Some(org) = &doc.org else {
        return Vec::new();
    };
    let org_obj = format!("org:{}", org.id);
    let mut out = Vec::new();
    for m in &doc.members {
        let relation = match m.role {
            Role::Owner => "owner",
            Role::Admin => "admin",
            Role::Member => "member",
        };
        out.push(Tuple::new(
            format!("user:{}", m.user.id),
            relation,
            &org_obj,
        ));
    }
    for t in &doc.teams {
        let team = format!("team:{}", t.team.id);
        out.push(Tuple::new(&org_obj, "org", &team));
        for m in &t.members {
            out.push(Tuple::new(format!("user:{m}"), "member", &team));
        }
    }
    let project_obj = |slug: &str| {
        doc.project(slug)
            .map(|p| format!("project:{}", project_id(p)))
    };
    for p in &doc.projects {
        let project = format!("project:{}", project_id(p));
        out.push(Tuple::new(&org_obj, "org", &project));
        for g in &p.grants {
            let (Some(subject), Some(relation)) = (
                g.subject.as_option(),
                g.role.as_known().and_then(project_relation),
            ) else {
                continue;
            };
            let user = match subject.kind.as_known() {
                Some(pb::SubjectKind::SUBJECT_KIND_USER) => format!("user:{}", subject.id),
                Some(pb::SubjectKind::SUBJECT_KIND_TEAM) => match doc.team(&subject.id) {
                    Some(team) => format!("team:{}#member", team.team.id),
                    None => continue,
                },
                Some(pb::SubjectKind::SUBJECT_KIND_AGENT) if relation != "admin" => {
                    format!("agent:{}", subject.id)
                }
                Some(pb::SubjectKind::SUBJECT_KIND_SERVICE_ACCOUNT) if relation != "admin" => {
                    format!("service_account:{}", subject.id)
                }
                _ => continue,
            };
            out.push(Tuple::new(user, relation, &project));
        }
        for e in &p.environments {
            out.push(Tuple::new(
                &project,
                "project",
                format!("environment:{}", e.namespace),
            ));
        }
    }
    for a in &doc.agents {
        let agent = format!("agent:{}", a.agent.id);
        if let Some(project) = project_obj(&a.agent.project) {
            out.push(Tuple::new(project, "project", &agent));
        }
        if let Some(owner) = a.agent.owner.as_option()
            && owner.kind.as_known() == Some(pb::SubjectKind::SUBJECT_KIND_USER)
        {
            out.push(Tuple::new(format!("user:{}", owner.id), "owner", &agent));
        }
    }
    let mut service_account = |id: &str, project_slug: &str| {
        let sa = format!("service_account:{id}");
        if let Some(project) = project_obj(project_slug) {
            out.push(Tuple::new(&project, "project", &sa));
            out.push(Tuple::new(&sa, "editor", project));
        }
        out.push(Tuple::new(&sa, "namespace_editor", &org_obj));
    };
    for sa in &doc.service_accounts {
        service_account(&sa.id, &sa.project);
    }
    for key in doc.keys.iter().filter(|k| !k.revoked) {
        if let Some(who) = key.key.principal.as_option()
            && who.kind.as_known() == Some(pb::SubjectKind::SUBJECT_KIND_SERVICE_ACCOUNT)
        {
            service_account(&who.id, &key.project);
        }
    }
    out
}

/// Model v1 in process over the principal store (built-in single-org mode, D1613): a
/// [`BuiltinAuthorizer`] loaded with [`tuples_of`] the current document, rebuilt when the
/// document changes. Not cached: it answers in process, so a revoked grant is refused on the
/// next request.
pub struct DirectoryAuthorizer {
    directory: Arc<Directory>,
    built: Mutex<Option<(Weak<Doc>, Arc<BuiltinAuthorizer>)>>,
}

impl fmt::Debug for DirectoryAuthorizer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DirectoryAuthorizer")
            .finish_non_exhaustive()
    }
}

impl DirectoryAuthorizer {
    pub fn new(directory: Arc<Directory>) -> Self {
        Self {
            directory,
            built: Mutex::new(None),
        }
    }

    async fn current(&self) -> loams_authz::Result<Arc<BuiltinAuthorizer>> {
        let doc = self
            .directory
            .read()
            .await
            .map_err(|e| AuthzError::Unavailable(e.0))?;
        let mut built = self
            .built
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((seen, authz)) = built.as_ref()
            && seen.upgrade().is_some_and(|d| Arc::ptr_eq(&d, &doc))
        {
            return Ok(authz.clone());
        }
        let authz = Arc::new(load(&tuples_of(&doc)));
        *built = Some((Arc::downgrade(&doc), authz.clone()));
        Ok(authz)
    }
}

/// A built-in authorizer holding `tuples`. A tuple the model refuses is skipped with a
/// warning, so one bad record cannot take every grant down.
fn load(tuples: &[Tuple]) -> BuiltinAuthorizer {
    let authz = BuiltinAuthorizer::new();
    if authz.write(tuples).is_err() {
        for t in tuples {
            if let Err(err) = authz.write(std::slice::from_ref(t)) {
                tracing::warn!(%err, "a principal store record is not a valid model v1 tuple; skipped");
            }
        }
    }
    authz
}

#[async_trait]
impl Authorizer for DirectoryAuthorizer {
    async fn check(&self, req: CheckRequest) -> loams_authz::Result<Decision> {
        self.current().await?.check(req).await
    }

    async fn batch_check(&self, reqs: Vec<CheckRequest>) -> loams_authz::Result<Vec<Decision>> {
        self.current().await?.batch_check(reqs).await
    }

    async fn list_objects(
        &self,
        subject: &Subject,
        relation: &str,
        ty: &str,
    ) -> loams_authz::Result<Vec<ObjectRef>> {
        self.current()
            .await?
            .list_objects(subject, relation, ty)
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::auth::directory::{AgentRec, KeyRec, MemberRec, PrincipalLink};
    use crate::auth::{
        Auth, AuthConfig, AuthErrorKind, Authenticator, Credential, Grant, SetupGate, reason,
    };

    const ISS: &str = "https://auth.example.com";

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |name| map.get(name).cloned()
    }

    fn project(id: &str, slug: &str, envs: &[&str], grants: Vec<pb::Grant>) -> ProjectRec {
        ProjectRec {
            project: pb::Project {
                id: id.into(),
                slug: slug.into(),
                ..Default::default()
            },
            grants,
            environments: envs
                .iter()
                .map(|ns| pb::Environment {
                    slug: ns.rsplit('-').next().unwrap_or(ns).into(),
                    project: slug.into(),
                    namespace: (*ns).into(),
                    ..Default::default()
                })
                .collect(),
        }
    }

    fn member(id: &str, role: Role) -> MemberRec {
        MemberRec {
            user: pb::User {
                id: id.into(),
                name: id.into(),
                email: format!("{id}@example.com"),
                ..Default::default()
            },
            role,
            joined_at_ms: 0,
            password: String::new(),
        }
    }

    fn grant(kind: pb::SubjectKind, id: &str, role: pb::ProjectRole) -> pb::Grant {
        pb::Grant {
            subject: pb::SubjectRef {
                kind: kind.into(),
                id: id.into(),
                ..Default::default()
            }
            .into(),
            role: role.into(),
            ..Default::default()
        }
    }

    /// An org with `own` (owner) and `mem` (member); projects `shop` (`shop-dev`,
    /// `shop-prod`) and `ops` (`ops-prod`); agent `ag_1` in `shop` with a developer grant there,
    /// linked from `ISS`/`machine-1`; agent `ag_2`, suspended, linked from `machine-2`; a
    /// service account key `k_sa` of `shop`; and a key `k_own` acting as `own`.
    async fn principals() -> Principals {
        use pb::ProjectRole as R;
        use pb::SubjectKind as K;
        let directory = Directory::in_memory();
        directory
            .update(|doc| {
                doc.org = Some(pb::Org {
                    id: "org_1".into(),
                    slug: "acme".into(),
                    ..Default::default()
                });
                doc.members = [("own", Role::Owner), ("mem", Role::Member)]
                    .into_iter()
                    .map(|(id, role)| MemberRec {
                        user: pb::User {
                            id: id.into(),
                            name: id.into(),
                            email: format!("{id}@example.com"),
                            ..Default::default()
                        },
                        role,
                        joined_at_ms: 0,
                        password: String::new(),
                    })
                    .collect();
                doc.projects = vec![
                    project(
                        "pr_shop",
                        "shop",
                        &["shop-dev", "shop-prod"],
                        vec![grant(
                            K::SUBJECT_KIND_AGENT,
                            "ag_1",
                            R::PROJECT_ROLE_DEVELOPER,
                        )],
                    ),
                    project("pr_ops", "ops", &["ops-prod"], vec![]),
                ];
                doc.agents = [
                    ("ag_1", pb::AgentStatus::AGENT_STATUS_ACTIVE),
                    ("ag_2", pb::AgentStatus::AGENT_STATUS_SUSPENDED),
                ]
                .into_iter()
                .map(|(id, status)| AgentRec {
                    agent: pb::Agent {
                        id: id.into(),
                        project: "shop".into(),
                        status: status.into(),
                        owner: pb::SubjectRef {
                            kind: K::SUBJECT_KIND_USER.into(),
                            id: "own".into(),
                            ..Default::default()
                        }
                        .into(),
                        ..Default::default()
                    },
                    trust_policies: Vec::new(),
                })
                .collect();
                doc.links = vec![
                    link("machine-1", LinkKind::Agent, "ag_1", false),
                    link("machine-2", LinkKind::Agent, "ag_2", false),
                    link("machine-3", LinkKind::Agent, "ag_1", true),
                    link("person-1", LinkKind::User, "mem", false),
                ];
                doc.keys = vec![
                    key("k_sa", K::SUBJECT_KIND_SERVICE_ACCOUNT, "sa_1"),
                    key("k_own", K::SUBJECT_KIND_USER, "own"),
                ];
                Ok::<_, Denied>(())
            })
            .await
            .expect("seed the directory");
        Principals::new(directory, SetupGate::Closed)
    }

    fn link(sub: &str, kind: LinkKind, principal: &str, disabled: bool) -> PrincipalLink {
        PrincipalLink {
            issuer: ISS.into(),
            subject: sub.into(),
            kind,
            principal: principal.into(),
            disabled,
        }
    }

    fn key(id: &str, kind: pb::SubjectKind, principal: &str) -> KeyRec {
        KeyRec {
            key: pb::ApiKey {
                id: id.into(),
                principal: pb::SubjectRef {
                    kind: kind.into(),
                    id: principal.into(),
                    ..Default::default()
                }
                .into(),
                ..Default::default()
            },
            project: "shop".into(),
            hash: String::new(),
            role: Role::Member,
            idempotency_key: String::new(),
            created_by: "own".into(),
            revoked: false,
        }
    }

    fn principal(subject: &str, kind: PrincipalKind, issuer: Option<&str>) -> Principal {
        Principal {
            subject: subject.into(),
            kind,
            issuer: issuer.map(str::to_owned),
            display_name: subject.into(),
            email: None,
            grant: Grant::default(),
        }
    }

    fn subject_of(standing: Standing) -> Subject {
        match standing {
            Standing::Subject(s) => s,
            other => panic!("expected a subject, got {other:?}"),
        }
    }

    async fn can(p: &Principals, who: &Principal, relation: &str, ns: &str) -> bool {
        let Standing::Subject(subject) = standing(p, who).await.expect("standing") else {
            return false;
        };
        let doc = p.directory.read().await.expect("doc");
        p.authz
            .check(environment_check(&doc, &subject, relation, ns))
            .await
            .expect("check")
            .allowed
    }

    #[tokio::test]
    async fn builtin_is_the_default() {
        assert_eq!(
            AuthzConfig::from_vars(vars(&[]), false).unwrap(),
            AuthzConfig::Builtin
        );
        assert_eq!(
            AuthzConfig::from_vars(vars(&[(AUTHZ_ENV, " BuiltIn ")]), false).unwrap(),
            AuthzConfig::Builtin
        );
        assert_eq!(AuthConfig::off().authz, AuthzConfig::Builtin);
        // The default answers from the principal store: a member edits the org's own
        // namespace, which only its tuples say.
        let p = principals().await;
        let member = principal("mem", PrincipalKind::Session, None);
        assert!(can(&p, &member, "can_write", "scratch").await);
        assert!(!can(&p, &member, "can_read", "shop-dev").await);
        let fga = AuthzConfig::from_vars(
            vars(&[
                (AUTHZ_ENV, "openfga"),
                ("LOAMS_AUTHZ_OPENFGA_GRPC", "http://openfga:8081"),
                ("LOAMS_AUTHZ_OPENFGA_STORE", "s1"),
                ("LOAMS_AUTHZ_OPENFGA_MODEL", "m1"),
                ("LOAMS_AUTHZ_OPENFGA_KEY", "sekrit"),
            ]),
            false,
        )
        .unwrap();
        assert!(
            !format!("{fga:?}").contains("sekrit"),
            "the key is redacted"
        );
        let err = AuthzConfig::from_vars(vars(&[(AUTHZ_ENV, "openfga")]), false).unwrap_err();
        assert!(err.contains("LOAMS_AUTHZ_OPENFGA_GRPC"), "{err}");
        assert!(AuthzConfig::from_vars(vars(&[(AUTHZ_ENV, "opa")]), false).is_err());
    }

    #[test]
    fn authz_off_refused_off_loopback() {
        let err = AuthzConfig::from_vars(vars(&[(AUTHZ_ENV, "off")]), false).unwrap_err();
        assert!(err.contains("LOAMS_AUTH_MODE=off"), "{err}");
        assert_eq!(
            AuthzConfig::from_vars(vars(&[(AUTHZ_ENV, "off")]), true).unwrap(),
            AuthzConfig::Off
        );
        // Through the engine's own config: authentication on refuses it...
        assert!(
            AuthConfig::from_vars(vars(&[
                ("LOAMS_AUTH_MODE", "apikey"),
                ("LOAMS_API_KEYS_FILE", "/keys"),
                (AUTHZ_ENV, "off"),
            ]))
            .is_err()
        );
        // ...and with it off, every listener must be loopback.
        let off = AuthConfig::from_vars(vars(&[(AUTHZ_ENV, "off")])).unwrap();
        assert_eq!(off.authz, AuthzConfig::Off);
        let lan: std::net::SocketAddr = "192.168.1.10:7780".parse().unwrap();
        assert!(off.check_listeners(&[("--listen", lan)]).is_err());
    }

    #[tokio::test]
    async fn session_maps_to_user() {
        let p = principals().await;
        let s = subject_of(
            standing(&p, &principal("mem", PrincipalKind::Session, None))
                .await
                .unwrap(),
        );
        assert_eq!(s.fga_user(), "user:mem");
        assert_eq!(s.org.as_deref(), Some("org_1"));
        assert_eq!(
            standing(&p, &principal("gone", PrincipalKind::Session, None))
                .await
                .unwrap(),
            Standing::Outside
        );
    }

    #[tokio::test]
    async fn lk_key_maps_to_service_account() {
        let p = principals().await;
        let sa = subject_of(
            standing(&p, &principal("apikey:k_sa", PrincipalKind::ApiKey, None))
                .await
                .unwrap(),
        );
        assert_eq!(sa.fga_user(), "service_account:sa_1");
        let own = subject_of(
            standing(&p, &principal("apikey:k_own", PrincipalKind::ApiKey, None))
                .await
                .unwrap(),
        );
        assert_eq!(own.fga_user(), "user:own", "a key acting as its creator");
        assert_eq!(
            standing(&p, &principal("apikey:file", PrincipalKind::ApiKey, None))
                .await
                .unwrap(),
            Standing::Operator,
            "a key from the keys file is the operator's"
        );
        let key = principal("apikey:k_sa", PrincipalKind::ApiKey, None);
        assert!(can(&p, &key, "can_write", "shop-dev").await);
        assert!(!can(&p, &key, "can_admin", "shop-dev").await);
        assert!(!can(&p, &key, "can_read", "ops-prod").await);
        assert!(
            can(&p, &key, "can_write", "scratch").await,
            "the org's own namespace"
        );
    }

    #[tokio::test]
    async fn agent_jwt_reaches_only_granted_namespace() {
        let p = principals().await;
        let agent = principal("machine-1", PrincipalKind::User, Some(ISS));
        assert_eq!(
            subject_of(standing(&p, &agent).await.unwrap()).fga_user(),
            "agent:ag_1"
        );
        assert!(can(&p, &agent, "can_write", "shop-dev").await);
        assert!(can(&p, &agent, "can_read", "shop-prod").await);
        assert!(!can(&p, &agent, "can_admin", "shop-dev").await);
        assert!(!can(&p, &agent, "can_read", "ops-prod").await);
        assert!(
            !can(&p, &agent, "can_read", "scratch").await,
            "an agent is no org member: the org's own namespaces are not its"
        );
        // The same `sub` from another issuer is not the agent.
        let other = principal(
            "machine-1",
            PrincipalKind::User,
            Some("https://evil.example"),
        );
        assert_eq!(
            subject_of(standing(&p, &other).await.unwrap()).fga_user(),
            "user:machine-1"
        );
        assert!(!can(&p, &other, "can_read", "shop-dev").await);
        // Its owner's rights are not its own.
        let owner = principal("own", PrincipalKind::Session, None);
        assert!(can(&p, &owner, "can_admin", "ops-prod").await);
    }

    #[tokio::test]
    async fn unlinked_subject_is_a_user() {
        let p = principals().await;
        let s = subject_of(
            standing(&p, &principal("zitadel-42", PrincipalKind::User, Some(ISS)))
                .await
                .unwrap(),
        );
        assert_eq!(s.fga_user(), "user:zitadel-42");
        assert!(
            !can(
                &p,
                &principal("zitadel-42", PrincipalKind::User, Some(ISS)),
                "can_read",
                "scratch"
            )
            .await
        );
        // A `sub` equal to a Loams member's own id is never that member.
        assert_eq!(
            standing(&p, &principal("own", PrincipalKind::User, Some(ISS)))
                .await
                .unwrap(),
            Standing::Outside
        );
        // A linked person is their member.
        let linked = principal("person-1", PrincipalKind::User, Some(ISS));
        assert_eq!(
            subject_of(standing(&p, &linked).await.unwrap()).fga_user(),
            "user:mem"
        );
        assert!(can(&p, &linked, "can_write", "scratch").await);
    }

    /// Admits any bearer as a JWT principal from `ISS` with the bearer as its `sub`.
    struct FakeJwt;

    #[async_trait]
    impl Authenticator for FakeJwt {
        async fn authenticate(
            &self,
            credential: &Credential,
        ) -> Option<Result<Principal, crate::auth::AuthError>> {
            let Credential::Jwt(sub) = credential else {
                return None;
            };
            Some(Ok(principal(sub, PrincipalKind::User, Some(ISS))))
        }
    }

    #[tokio::test]
    async fn disabled_link_refuses_live_jwt() {
        let p = principals().await;
        let auth = Auth::with_authenticators(vec![Arc::new(FakeJwt)], Default::default());
        auth.attach_directory(p.directory.clone());
        let jwt = |sub: &str| Credential::Jwt(sub.into());
        assert_eq!(
            auth.authenticate(&jwt("machine-1")).await.unwrap().subject,
            "machine-1"
        );
        for (sub, why) in [
            ("machine-3", "a disabled link"),
            ("machine-2", "a suspended agent"),
        ] {
            let err = auth.authenticate(&jwt(sub)).await.unwrap_err();
            assert_eq!(err.reason, reason::TOKEN_REVOKED, "{why}");
            assert_eq!(err.kind, AuthErrorKind::Unauthenticated, "{why}");
        }
        // Disabling takes effect on the next request.
        p.directory
            .update(|doc| {
                doc.links[0].disabled = true;
                Ok::<_, Denied>(())
            })
            .await
            .unwrap();
        assert_eq!(
            auth.authenticate(&jwt("machine-1"))
                .await
                .unwrap_err()
                .reason,
            reason::TOKEN_REVOKED
        );
        // An unlinked subject is admitted (and checked as `user:<sub>`).
        assert!(auth.authenticate(&jwt("someone")).await.is_ok());
    }

    #[tokio::test]
    async fn tuples_of_keeps_members_out_of_ungranted_projects() {
        let doc = Doc {
            org: Some(pb::Org {
                id: "o".into(),
                ..Default::default()
            }),
            members: vec![
                member("u-admin", Role::Admin),
                member("u-member", Role::Member),
            ],
            projects: vec![project("p", "p", &["p-dev"], vec![])],
            ..Doc::default()
        };
        let tuples = tuples_of(&doc);
        assert!(
            tuples
                .iter()
                .any(|t| t.relation == "org" && t.object == "project:p" && t.user == "org:o"),
            "every project is rooted in the org"
        );
        let authz = load(&tuples);
        let can = |user: &str, relation: &str| {
            let req = CheckRequest::new(
                Subject::user(user).with_org("o"),
                relation,
                ObjectRef::new("environment", "p-dev"),
            );
            let authz = &authz;
            async move { authz.check(req).await.unwrap().allowed }
        };
        assert!(
            can("u-admin", "can_admin").await,
            "an org admin is every project's admin"
        );
        assert!(
            !can("u-member", "can_read").await,
            "an org member with no grant reads none of a project's environments (T4.R2)"
        );
        assert!(tuples_of(&Doc::default()).is_empty(), "no org, no tuples");
    }
}
