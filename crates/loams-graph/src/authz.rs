//! Per-graph authorization (GR1 Task 24, design §48 §11, D750).
//!
//! The object is `graph:<ns>/<name>`, and a caller holds one of three relations on it, least to
//! most: `reader`, `writer`, `admin` (§48 §11.1's OpenFGA type, `docs/authz/graph.fga`). Each RPC
//! needs one ([`Need`]):
//!
//! | RPC | Needs |
//! |---|---|
//! | `Execute`, `ExecuteStream`, `Explain` | the statement's [`Access`]: `reader` for a read, `writer` for a write, `admin` for DDL |
//! | `ExecuteBatch` | the strongest of its statements' |
//! | `GetGraph`, `GetSchema` | `reader` |
//! | `ListGraphs` | nothing on the namespace; each graph is answered only to a `reader` of it ([`filter_visible`]) |
//! | `CreateGraph` | `admin` on the namespace (which creates the namespace when it does not exist) |
//! | `UpdateGraph`, `DeleteGraph` (and `Restore`/`Export`/`Import`, Task 29) | `admin`; in a protected environment also the project `admin` role ([`Need::Destroy`]) |
//!
//! A decision has three inputs, applied in this order ([`decide`]):
//!
//! 1. **The environment fence.** A credential bound to an environment reaches only that
//!    environment's namespace (an environment is one namespace, §19 §4); any other is refused,
//!    whatever its roles. The same rule as Live's app fence (LV1 T15-1).
//! 2. **The relation** a [`GraphAuthorizer`] answers ([`Standing`]): the caller's strongest
//!    relation on the graph, whether the namespace is a protected environment, and whether the
//!    caller holds the project `admin` role there. The server's authorizer is
//!    [`ModelGraphAuthorizer`](crate::model_authz::ModelGraphAuthorizer), which asks model v1
//!    through `loams-authz` (built in or OpenFGA); [`StaticRbac`] is a fixed table for tests and
//!    embedders.
//! 3. **The credential's scopes** (§48 §11.2 item 3): `graph:read`, `graph:write`, `graph:admin`
//!    cap the relation; a credential that names no scopes is not capped, and one that names
//!    scopes but none of these reaches no graph.
//!
//! A refusal is `PERMISSION_DENIED`, reason `permission_denied`, with `need` (the relation it
//! needed) in the `ErrorInfo`'s metadata, and is audited: one `audit = true` line and a count
//! ([`denials`]). The authorization is checked before the graph is looked up, so a caller who may
//! not read a graph cannot tell whether it exists.
//!
//! **Loams' own reads for a principal** (GR1 R24.12, amending R24.4): a search's `expand` and the
//! SQL graph and algorithm table functions are a principal's requests too. `loams-query` passes
//! each one's [`GraphCaller`](loams_query::graph::GraphCaller), and a principal's
//! ([`PrincipalReads`]) needs `reader` on every graph it touches, decided and audited here as an
//! RPC's is; a refusal fails the request (`PERMISSION_DENIED`, `need` = `reader`). Only Loams'
//! own calls on no principal's behalf (links, a write-back, a forwarded call, maintenance) are
//! `GraphCaller::Internal` and unchecked.
//!
//! The engine session that runs a statement has the Grafeo role of the statement's own access,
//! which is never above the caller's relation (a statement needing more is refused here first),
//! so the engine still refuses a write that both classifiers called a read (§48 §11.2 item 1).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use connectrpc::{ConnectError, ErrorCode};

use crate::classify::Access;
use crate::service::errors::refuse_with;

/// The reason of every refusal here (registered in `docs/api/reasons.md`).
pub const DENIED: &str = "permission_denied";

/// The scope that caps a credential at `reader`.
pub const SCOPE_READ: &str = "graph:read";
/// The scope that caps a credential at `writer`.
pub const SCOPE_WRITE: &str = "graph:write";
/// The scope that allows `admin`.
pub const SCOPE_ADMIN: &str = "graph:admin";

/// A caller's relation on a graph (§48 §11.1), least to most. Ordered: a stronger relation
/// includes the weaker ones (`can_read: reader or writer or admin`, `can_write: writer or
/// admin`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Relation {
    /// Reads the graph and its schema.
    Reader,
    /// Also writes its data.
    Writer,
    /// Also changes its schema, settings and lifecycle.
    Admin,
}

impl Relation {
    /// The relation a statement of `access` needs.
    #[must_use]
    pub fn for_access(access: Access) -> Self {
        match access {
            Access::Read => Self::Reader,
            Access::Write => Self::Writer,
            Access::Admin => Self::Admin,
        }
    }

    /// The strongest statement this relation may run, the Grafeo session role's access.
    #[must_use]
    pub fn access(self) -> Access {
        match self {
            Self::Reader => Access::Read,
            Self::Writer => Access::Write,
            Self::Admin => Access::Admin,
        }
    }

    /// Its OpenFGA name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reader => "reader",
            Self::Writer => "writer",
            Self::Admin => "admin",
        }
    }

    /// The scope that allows it.
    #[must_use]
    pub fn scope(self) -> &'static str {
        match self {
            Self::Reader => SCOPE_READ,
            Self::Writer => SCOPE_WRITE,
            Self::Admin => SCOPE_ADMIN,
        }
    }
}

impl fmt::Display for Relation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What an RPC needs (§48 §11.1's table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// At least this relation.
    At(Relation),
    /// `admin`, and in a protected environment the project `admin` role as well (§19 §4:
    /// destructive actions in a protected environment).
    Destroy,
}

impl Need {
    /// What a statement of `access` needs.
    #[must_use]
    pub fn statement(access: Access) -> Self {
        Self::At(Relation::for_access(access))
    }

    /// The relation it needs.
    #[must_use]
    pub fn relation(self) -> Relation {
        match self {
            Self::At(relation) => relation,
            Self::Destroy => Relation::Admin,
        }
    }
}

/// How the caller authenticated, as the engine's authentication layer admitted them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CallerKind {
    /// An OIDC user (a JWT from the issuer).
    User,
    /// A person signed in to the console.
    Session,
    /// An API key.
    ApiKey,
    /// A tailnet user.
    Tailscale,
    /// The developer on loopback with authentication off: the operator.
    DevLoopback,
    /// No principal was admitted (a router served without the authentication layer).
    Anonymous,
}

impl CallerKind {
    /// Its name in audit lines.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Session => "session",
            Self::ApiKey => "api_key",
            Self::Tailscale => "tailscale",
            Self::DevLoopback => "dev_loopback",
            Self::Anonymous => "anonymous",
        }
    }
}

/// Who is calling: the principal the engine's authentication admitted and what its credential
/// reaches (the engine's `Principal` and its `Grant`, LV1 T14-2). Never a credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    /// The principal's id: the OIDC `sub`, `apikey:<id>`, a user id, or `dev-loopback`.
    pub id: String,
    /// How it authenticated.
    pub kind: CallerKind,
    /// The issuer of its OIDC token (the exact `iss`), which with `id` names its principal
    /// link (design 76 §5, D1628); `None` for any other credential.
    pub issuer: Option<String>,
    /// A name to show.
    pub name: String,
    /// A verified email, for people.
    pub email: Option<String>,
    /// The environment the credential is bound to (`<project>/<environment>`, or a namespace);
    /// `None` reaches every namespace its roles allow.
    pub environment: Option<String>,
    /// The scopes it carries; `None`: every scope.
    pub scopes: Option<BTreeSet<String>>,
}

impl Caller {
    /// A caller with no environment and every scope.
    #[must_use]
    pub fn new(id: impl Into<String>, kind: CallerKind) -> Self {
        let id = id.into();
        Self {
            name: id.clone(),
            id,
            kind,
            issuer: None,
            email: None,
            environment: None,
            scopes: None,
        }
    }

    /// No principal: reaches nothing an authorizer does not give to anyone.
    #[must_use]
    pub fn anonymous() -> Self {
        Self::new("anonymous", CallerKind::Anonymous)
    }

    /// The strongest relation the credential's scopes allow: `None` for a credential that names
    /// scopes but no `graph:*` one.
    #[must_use]
    pub fn scope_cap(&self) -> Option<Relation> {
        let Some(scopes) = &self.scopes else {
            return Some(Relation::Admin);
        };
        [Relation::Admin, Relation::Writer, Relation::Reader]
            .into_iter()
            .find(|relation| scopes.contains(relation.scope()))
    }
}

/// What a [`GraphAuthorizer`] knows of a caller on one graph (or namespace).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Standing {
    /// The caller's strongest relation, before scopes; `None`: none.
    pub relation: Option<Relation>,
    /// Whether the namespace is a protected environment (§19 §4).
    pub protected: bool,
    /// Whether the caller holds the project `admin` role over the namespace.
    pub project_admin: bool,
    /// Whether the namespace is the one the caller's credential is bound to, or the credential is
    /// bound to none (the environment fence).
    pub in_environment: bool,
}

impl Standing {
    /// The operator's standing: `admin` everywhere, project admin, no fence.
    #[must_use]
    pub fn operator() -> Self {
        Self {
            relation: Some(Relation::Admin),
            protected: false,
            project_admin: true,
            in_environment: true,
        }
    }
}

/// Answers a caller's standing on a graph (§48 §11.1; the D66 `Authorizer` shape for Loams
/// Graph). The server's implementation is
/// [`ModelGraphAuthorizer`](crate::model_authz::ModelGraphAuthorizer), over `loams-authz`.
#[async_trait]
pub trait GraphAuthorizer: Send + Sync + fmt::Debug {
    /// `caller`'s standing on graph `graph` of `namespace`, or on the namespace itself when
    /// `graph` is `None` (`CreateGraph`).
    ///
    /// # Errors
    ///
    /// `UNAVAILABLE` when what it reads cannot be read.
    async fn standing(
        &self,
        caller: &Caller,
        namespace: &str,
        graph: Option<&str>,
    ) -> Result<Standing, ConnectError>;

    /// Which of `graphs` (names in `namespace`) the caller may read, in order. The default asks
    /// [`GraphAuthorizer::standing`] once per graph; an authorizer whose relations are
    /// namespace-wide answers them all from one read.
    ///
    /// # Errors
    ///
    /// As [`GraphAuthorizer::standing`].
    async fn visible(
        &self,
        caller: &Caller,
        namespace: &str,
        graphs: &[String],
    ) -> Result<Vec<bool>, ConnectError> {
        let mut out = Vec::with_capacity(graphs.len());
        for graph in graphs {
            let standing = self.standing(caller, namespace, Some(graph)).await?;
            out.push(decide(caller, &standing, Need::At(Relation::Reader)).is_ok());
        }
        Ok(out)
    }
}

/// Why a decision refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The credential is bound to another environment.
    OutsideEnvironment,
    /// The caller holds no relation, or a weaker one than needed.
    Relation {
        /// What it holds.
        have: Option<Relation>,
    },
    /// The credential's scopes cap it below what is needed.
    Scope {
        /// The cap; `None`: no graph scope at all.
        cap: Option<Relation>,
    },
    /// A destructive action in a protected environment needs the project `admin` role.
    ProjectAdmin,
}

/// Decides one check: the fence, the relation, the scopes (module docs). Answers the caller's
/// effective relation (its relation capped by its scopes).
///
/// # Errors
///
/// The first [`Refusal`] that applies.
pub fn decide(caller: &Caller, standing: &Standing, need: Need) -> Result<Relation, Refusal> {
    if !standing.in_environment {
        return Err(Refusal::OutsideEnvironment);
    }
    let needed = need.relation();
    let held = standing.relation.filter(|held| *held >= needed);
    let Some(held) = held else {
        return Err(Refusal::Relation {
            have: standing.relation,
        });
    };
    let cap = caller.scope_cap();
    let effective = match cap {
        Some(cap) if cap >= needed => held.min(cap),
        _ => return Err(Refusal::Scope { cap }),
    };
    if need == Need::Destroy && standing.protected && !standing.project_admin {
        return Err(Refusal::ProjectAdmin);
    }
    Ok(effective)
}

/// Checks that `caller` may make `rpc` on graph `graph` of `namespace` (or on the namespace),
/// auditing a refusal.
///
/// # Errors
///
/// `PERMISSION_DENIED`/`permission_denied`, or the authorizer's own error.
pub async fn authorize(
    authorizer: &dyn GraphAuthorizer,
    caller: &Caller,
    rpc: &str,
    namespace: &str,
    graph: Option<&str>,
    need: Need,
) -> Result<Relation, ConnectError> {
    let standing = authorizer.standing(caller, namespace, graph).await?;
    decide(caller, &standing, need)
        .map_err(|refusal| denied(caller, rpc, namespace, graph, need, refusal))
}

/// The graphs of `graphs` (in `namespace`) `caller` may read, in order (`ListGraphs`, controller
/// ruling M5). A namespace outside the credential's environment is refused, not filtered to
/// nothing, so the caller can tell the two apart.
///
/// # Errors
///
/// `PERMISSION_DENIED` outside the credential's environment, or the authorizer's own error.
pub async fn filter_visible<T>(
    authorizer: &dyn GraphAuthorizer,
    caller: &Caller,
    namespace: &str,
    graphs: Vec<T>,
    name: impl Fn(&T) -> &str,
) -> Result<Vec<T>, ConnectError> {
    let standing = authorizer.standing(caller, namespace, None).await?;
    if !standing.in_environment {
        return Err(denied(
            caller,
            "ListGraphs",
            namespace,
            None,
            Need::At(Relation::Reader),
            Refusal::OutsideEnvironment,
        ));
    }
    let names: Vec<String> = graphs.iter().map(|g| name(g).to_string()).collect();
    let visible = authorizer.visible(caller, namespace, &names).await?;
    Ok(graphs
        .into_iter()
        .zip(visible)
        .filter_map(|(graph, visible)| visible.then_some(graph))
        .collect())
}

static DENIALS: AtomicU64 = AtomicU64::new(0);
static DISAGREEMENTS: AtomicU64 = AtomicU64::new(0);

/// Authorization refusals in this process so far (Task 27's `loams_graph_authz_denials_total`).
#[must_use]
pub fn denials() -> u64 {
    DENIALS.load(Ordering::Relaxed)
}

/// Statements the keyword guard and the engine classified differently in this process so far
/// (§48 §11.2; Task 27's `loams_graph_classifier_disagreements_total`).
#[must_use]
pub fn classifier_disagreements() -> u64 {
    DISAGREEMENTS.load(Ordering::Relaxed)
}

/// Logs and counts a statement the two classifiers read differently (`graph_classifier_
/// disagreement`): each one is a guard bug or an engine bug, though the guard's conservative
/// answers (a read it calls a write) are expected and listed in the classify corpus. The
/// statement is logged with its literals redacted.
pub(crate) fn note_disagreement(statement: &str, guard: Access, engine: Access) {
    DISAGREEMENTS.fetch_add(1, Ordering::Relaxed);
    tracing::info!(
        reason = "graph_classifier_disagreement",
        guard = ?guard,
        engine = ?engine,
        statement = %crate::redact::redact_literals(statement),
        "the keyword guard and the engine classified a statement differently"
    );
}

/// A principal's graph reads through `loams-query` (a search's `expand`, the SQL graph and
/// algorithm table functions; GR1 R24.12): its [`Caller`] and the authorizer that decides it,
/// as `loams_query::graph::GraphCaller::Principal`.
#[derive(Debug, Clone)]
pub struct PrincipalReads {
    authorizer: Arc<dyn GraphAuthorizer>,
    caller: Caller,
}

impl PrincipalReads {
    /// `caller`'s reads, decided by `authorizer`.
    #[must_use]
    pub fn new(authorizer: Arc<dyn GraphAuthorizer>, caller: Caller) -> Self {
        Self { authorizer, caller }
    }

    /// As the graph caller `loams-query` passes down.
    #[must_use]
    pub fn into_graph_caller(self) -> loams_query::graph::GraphCaller {
        loams_query::graph::GraphCaller::Principal(Arc::new(self))
    }
}

#[async_trait]
impl loams_query::graph::GraphReadGate for PrincipalReads {
    /// `reader` on the graph: the fence, the relation, the scopes ([`decide`]); a refusal is
    /// audited and counted as an RPC's, with `what` as its rpc.
    async fn authorize_read(
        &self,
        what: &str,
        namespace: &str,
        graph: &str,
    ) -> Result<(), loams_query::ServiceError> {
        let need = Need::At(Relation::Reader);
        let standing = self
            .authorizer
            .standing(&self.caller, namespace, Some(graph))
            .await
            .map_err(|err| {
                loams_query::ServiceError::Unavailable(
                    err.message.unwrap_or_else(|| err.code.as_str().to_string()),
                )
            })?;
        decide(&self.caller, &standing, need)
            .map(|_| ())
            .map_err(|refusal| {
                let denial =
                    record_denial(&self.caller, what, namespace, Some(graph), need, refusal);
                loams_query::ServiceError::PermissionDenied {
                    message: denial.message,
                    need: denial.need.to_string(),
                    object: denial.object,
                }
            })
    }
}

/// A refusal, as audited: its message, what it needed, and the object.
struct Denial {
    message: String,
    need: &'static str,
    object: String,
}

/// The refusal for `refusal`, audited.
fn denied(
    caller: &Caller,
    rpc: &str,
    namespace: &str,
    graph: Option<&str>,
    need: Need,
    refusal: Refusal,
) -> ConnectError {
    let denial = record_denial(caller, rpc, namespace, graph, need, refusal);
    refuse_with(
        ErrorCode::PermissionDenied,
        DENIED,
        denial.message,
        &[("need", denial.need)],
    )
}

/// Audits and counts a refusal, and words it.
fn record_denial(
    caller: &Caller,
    rpc: &str,
    namespace: &str,
    graph: Option<&str>,
    need: Need,
    refusal: Refusal,
) -> Denial {
    DENIALS.fetch_add(1, Ordering::Relaxed);
    let object = match graph {
        Some(graph) => format!("graph:{namespace}/{graph}"),
        None => format!("namespace:{namespace}"),
    };
    let needed = need.relation();
    let (why, need_name) = match refusal {
        Refusal::OutsideEnvironment => (
            "the credential is bound to another environment".to_string(),
            needed.as_str(),
        ),
        Refusal::Relation { have: None } => (
            format!("the caller holds no relation on {object}"),
            needed.as_str(),
        ),
        Refusal::Relation { have: Some(have) } => (
            format!("the caller is a {have} of {object}"),
            needed.as_str(),
        ),
        Refusal::Scope { cap: None } => (
            format!(
                "the credential carries none of the scopes {SCOPE_READ}, {SCOPE_WRITE}, {SCOPE_ADMIN}"
            ),
            needed.as_str(),
        ),
        Refusal::Scope { cap: Some(cap) } => (
            format!(
                "the credential's scopes allow at most {cap} ({})",
                cap.scope()
            ),
            needed.as_str(),
        ),
        Refusal::ProjectAdmin => (
            format!("{namespace} is a protected environment: this needs the project admin role"),
            "project_admin",
        ),
    };
    tracing::info!(
        audit = true,
        outcome = "denied",
        caller = %caller.id,
        kind = caller.kind.as_str(),
        rpc,
        object = %object,
        need = need_name,
        refusal = ?refusal,
        "Loams Graph authorization refused"
    );
    Denial {
        message: format!("{rpc} on {object} needs {need_name}: {why}"),
        need: need_name,
        object,
    }
}

/// Audits an allowed call that changes a graph's lifecycle (create, update, delete), as Live
/// audits deploys, until the engine has an audit log.
pub(crate) fn audit_allowed(caller: &Caller, rpc: &str, namespace: &str, graph: &str) {
    tracing::info!(
        audit = true,
        outcome = "allowed",
        caller = %caller.id,
        kind = caller.kind.as_str(),
        rpc,
        object = %format!("graph:{namespace}/{graph}"),
        "Loams Graph administration"
    );
}

/// A fixed table of relations: the built-in RBAC for tests and embedders (the server expands its
/// principal store's roles instead). Relations are granted per caller id, on a namespace (every
/// graph in it) or on one graph; the strongest applies.
#[derive(Debug, Clone, Default)]
pub struct StaticRbac {
    namespaces: BTreeMap<(String, String), Relation>,
    graphs: BTreeMap<(String, String, String), Relation>,
    protected: BTreeSet<String>,
    project_admins: BTreeSet<(String, String)>,
    environments: BTreeMap<String, String>,
}

impl StaticRbac {
    /// An empty table: nobody reaches anything.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Grants `relation` on every graph of `namespace` to caller `id`.
    #[must_use]
    pub fn grant(mut self, id: &str, namespace: &str, relation: Relation) -> Self {
        self.namespaces
            .insert((id.to_string(), namespace.to_string()), relation);
        self
    }

    /// Grants `relation` on graph `graph` of `namespace` to caller `id`.
    #[must_use]
    pub fn grant_graph(
        mut self,
        id: &str,
        namespace: &str,
        graph: &str,
        relation: Relation,
    ) -> Self {
        self.graphs.insert(
            (id.to_string(), namespace.to_string(), graph.to_string()),
            relation,
        );
        self
    }

    /// Marks `namespace` a protected environment.
    #[must_use]
    pub fn protect(mut self, namespace: &str) -> Self {
        self.protected.insert(namespace.to_string());
        self
    }

    /// Gives caller `id` the project `admin` role over `namespace`.
    #[must_use]
    pub fn project_admin(mut self, id: &str, namespace: &str) -> Self {
        self.project_admins
            .insert((id.to_string(), namespace.to_string()));
        self
    }

    /// Names the namespace of environment `environment` (`<project>/<environment>`); an
    /// environment not named here is its own namespace's name.
    #[must_use]
    pub fn environment(mut self, environment: &str, namespace: &str) -> Self {
        self.environments
            .insert(environment.to_string(), namespace.to_string());
        self
    }

    fn standing_of(&self, caller: &Caller, namespace: &str, graph: Option<&str>) -> Standing {
        let key = |ns: &str| (caller.id.clone(), ns.to_string());
        let on_namespace = self.namespaces.get(&key(namespace)).copied();
        let on_graph = graph.and_then(|graph| {
            self.graphs
                .get(&(caller.id.clone(), namespace.to_string(), graph.to_string()))
                .copied()
        });
        let in_environment = caller
            .environment
            .as_deref()
            .is_none_or(|env| self.environments.get(env).map_or(env, String::as_str) == namespace);
        Standing {
            relation: on_namespace.max(on_graph),
            protected: self.protected.contains(namespace),
            project_admin: self.project_admins.contains(&key(namespace)),
            in_environment,
        }
    }
}

#[async_trait]
impl GraphAuthorizer for StaticRbac {
    async fn standing(
        &self,
        caller: &Caller,
        namespace: &str,
        graph: Option<&str>,
    ) -> Result<Standing, ConnectError> {
        Ok(self.standing_of(caller, namespace, graph))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_scopes(scopes: &[&str]) -> Caller {
        Caller {
            scopes: Some(scopes.iter().map(ToString::to_string).collect()),
            ..Caller::new("k", CallerKind::ApiKey)
        }
    }

    fn standing(relation: Option<Relation>) -> Standing {
        Standing {
            relation,
            in_environment: true,
            ..Standing::default()
        }
    }

    #[test]
    fn relations_order_and_include_the_weaker() {
        let all = Caller::new("u", CallerKind::User);
        for (held, need, ok) in [
            (Relation::Reader, Relation::Reader, true),
            (Relation::Reader, Relation::Writer, false),
            (Relation::Writer, Relation::Reader, true),
            (Relation::Writer, Relation::Admin, false),
            (Relation::Admin, Relation::Writer, true),
        ] {
            assert_eq!(
                decide(&all, &standing(Some(held)), Need::At(need)).is_ok(),
                ok,
                "{held} for {need}"
            );
        }
        assert_eq!(
            decide(&all, &standing(None), Need::At(Relation::Reader)),
            Err(Refusal::Relation { have: None })
        );
    }

    #[test]
    fn scopes_cap_the_relation() {
        let admin = standing(Some(Relation::Admin));
        assert_eq!(
            decide(
                &with_scopes(&[SCOPE_READ]),
                &admin,
                Need::At(Relation::Reader)
            ),
            Ok(Relation::Reader)
        );
        assert_eq!(
            decide(
                &with_scopes(&[SCOPE_READ]),
                &admin,
                Need::At(Relation::Writer)
            ),
            Err(Refusal::Scope {
                cap: Some(Relation::Reader)
            })
        );
        assert_eq!(
            decide(
                &with_scopes(&["live:call"]),
                &admin,
                Need::At(Relation::Reader)
            ),
            Err(Refusal::Scope { cap: None })
        );
        assert_eq!(
            decide(
                &with_scopes(&[SCOPE_WRITE, SCOPE_READ]),
                &admin,
                Need::At(Relation::Writer)
            ),
            Ok(Relation::Writer)
        );
        // A scope never raises a relation.
        assert_eq!(
            decide(
                &with_scopes(&[SCOPE_ADMIN]),
                &standing(Some(Relation::Reader)),
                Need::At(Relation::Writer)
            ),
            Err(Refusal::Relation {
                have: Some(Relation::Reader)
            })
        );
    }

    #[test]
    fn destroy_in_a_protected_environment_needs_project_admin() {
        let caller = Caller::new("u", CallerKind::User);
        let mut s = standing(Some(Relation::Admin));
        assert!(decide(&caller, &s, Need::Destroy).is_ok());
        s.protected = true;
        assert_eq!(
            decide(&caller, &s, Need::Destroy),
            Err(Refusal::ProjectAdmin)
        );
        // Other admin work in a protected environment needs no more than admin.
        assert!(decide(&caller, &s, Need::At(Relation::Admin)).is_ok());
        s.project_admin = true;
        assert!(decide(&caller, &s, Need::Destroy).is_ok());
    }

    #[test]
    fn the_fence_comes_first() {
        let caller = Caller::new("u", CallerKind::User);
        let s = Standing {
            in_environment: false,
            ..Standing::operator()
        };
        assert_eq!(
            decide(&caller, &s, Need::At(Relation::Reader)),
            Err(Refusal::OutsideEnvironment)
        );
    }
}
