//! Loams Graph's relations through `loams-authz` (design 76 §9, D1610; plan IA1, ruling IA2.R5).
//!
//! [`ModelGraphAuthorizer`] is the [`GraphAuthorizer`] of any host that has a
//! [`loams_authz::Authorizer`]: the engine's built-in model over its principal store, OpenFGA, or a
//! test's [`loams_authz::BuiltinAuthorizer`]. It asks model v1's `graph` type (`deploy/openfga/
//! graph.fga`) for `can_admin`, `can_write` and `can_read` on `graph:<ns>/<name>`, or on
//! `environment:<ns>` when no graph is named (`CreateGraph`), and `can_admin` on the namespace's
//! project for the protected-environment rule. The strongest relation allowed is the caller's.
//!
//! What only the host knows comes from a [`GraphDirectory`]:
//!
//! - **who** the caller is in the model ([`Who`]): nobody, the operator, or a subject;
//! - **the place** of a namespace ([`Place`]): whether it is a protected environment, whether the
//!   caller's credential reaches it (the environment fence), the project whose environment it is,
//!   and the contextual tuples that root it (a host that writes no tuple per namespace or graph
//!   passes them with each check).
//!
//! `ListGraphs`' filter ([`GraphAuthorizer::visible`]) asks `can_read` on each graph in one batch,
//! so a graph granted to the caller directly (a `graph:<ns>/<name>#reader` tuple in OpenFGA) is
//! listed even when the caller has no role on its namespace.
//!
//! An authorizer that cannot answer is `UNAVAILABLE`, never an allow.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use connectrpc::{ConnectError, ErrorCode};
use loams_authz::{Authorizer, CheckRequest, Consistency, ObjectRef, Subject, Tuple};

use crate::authz::{Caller, GraphAuthorizer, Need, Relation, Standing, decide};

/// Who a caller is, as far as the model goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Who {
    /// No principal, or one outside the org: no relation.
    Outside,
    /// The operator (loopback dev mode, an operator key): `admin` everywhere, no model check.
    Operator,
    /// A subject the model is asked about.
    Subject(Subject),
}

/// What the host knows of a namespace, for one caller.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Place {
    /// The namespace is a protected environment (§19 §4).
    pub protected: bool,
    /// The caller's credential is bound to no environment, or to this namespace's.
    pub in_environment: bool,
    /// The OpenFGA id of the project whose environment the namespace is, if any.
    pub project: Option<String>,
    /// Contextual tuples that root `environment:<ns>` (for example `org:<id>#org` for a namespace
    /// that is no project's environment), sent with every check on the namespace or its graphs.
    pub roots: Vec<Tuple>,
}

/// The host's side of [`ModelGraphAuthorizer`].
#[async_trait]
pub trait GraphDirectory: Send + Sync + fmt::Debug {
    /// Who `caller` is.
    ///
    /// # Errors
    ///
    /// `UNAVAILABLE` when the host cannot tell.
    async fn who(&self, caller: &Caller) -> Result<Who, ConnectError>;

    /// The place of `namespace` for `caller`.
    ///
    /// # Errors
    ///
    /// `UNAVAILABLE` when the host cannot tell.
    async fn place(&self, caller: &Caller, namespace: &str) -> Result<Place, ConnectError>;
}

/// The graph relations, strongest first, and the `can_*` that grants each.
const RELATIONS: [(Relation, &str); 3] = [
    (Relation::Admin, "can_admin"),
    (Relation::Writer, "can_write"),
    (Relation::Reader, "can_read"),
];

/// The OpenFGA object of graph `graph` in `namespace`.
#[must_use]
pub fn graph_object(namespace: &str, graph: &str) -> ObjectRef {
    ObjectRef::new("graph", format!("{namespace}/{graph}"))
}

/// A check of `relation` on `environment:<namespace>`, rooted by `place`.
#[must_use]
pub fn environment_check(
    subject: &Subject,
    relation: &str,
    namespace: &str,
    place: &Place,
) -> CheckRequest {
    let mut req = CheckRequest::new(
        subject.clone(),
        relation,
        ObjectRef::new("environment", namespace),
    );
    req.contextual_tuples.clone_from(&place.roots);
    req
}

/// A check of `relation` on `graph:<namespace>/<graph>`, with the tuple that hangs the graph off
/// its namespace (no host writes one per graph) and `place`'s roots.
#[must_use]
pub fn graph_check(
    subject: &Subject,
    relation: &str,
    namespace: &str,
    graph: &str,
    place: &Place,
) -> CheckRequest {
    let object = graph_object(namespace, graph);
    let mut req = CheckRequest::new(subject.clone(), relation, object.clone());
    req.contextual_tuples.clone_from(&place.roots);
    req.contextual_tuples.push(Tuple::new(
        format!("environment:{namespace}"),
        "environment",
        object.to_string(),
    ));
    req
}

/// [`GraphAuthorizer`] over `loams-authz` (module docs).
pub struct ModelGraphAuthorizer<D> {
    directory: D,
    authz: Arc<dyn Authorizer>,
}

impl<D: fmt::Debug> fmt::Debug for ModelGraphAuthorizer<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelGraphAuthorizer")
            .field("directory", &self.directory)
            .finish_non_exhaustive()
    }
}

impl<D: GraphDirectory> ModelGraphAuthorizer<D> {
    /// Creates a graph authorizer backed by the given directory and authorization service.
    pub fn new(directory: D, authz: Arc<dyn Authorizer>) -> Self {
        Self { directory, authz }
    }

    async fn batch(&self, checks: Vec<CheckRequest>) -> Result<Vec<bool>, ConnectError> {
        let decisions = self.authz.batch_check(checks).await.map_err(|err| {
            tracing::warn!(error = %err, "the authorizer could not answer a graph check");
            ConnectError::new(
                ErrorCode::Unavailable,
                "authorization is unavailable; retry",
            )
        })?;
        Ok(decisions.into_iter().map(|d| d.allowed).collect())
    }
}

#[async_trait]
impl<D: GraphDirectory> GraphAuthorizer for ModelGraphAuthorizer<D> {
    async fn standing(
        &self,
        caller: &Caller,
        namespace: &str,
        graph: Option<&str>,
    ) -> Result<Standing, ConnectError> {
        let who = self.directory.who(caller).await?;
        let place = self.directory.place(caller, namespace).await?;
        let (relation, project_admin) = match who {
            Who::Outside => (None, false),
            Who::Operator => (Some(Relation::Admin), true),
            Who::Subject(subject) => {
                let mut checks: Vec<CheckRequest> = RELATIONS
                    .iter()
                    .map(|(_, can)| match graph {
                        Some(graph) => graph_check(&subject, can, namespace, graph, &place),
                        None => environment_check(&subject, can, namespace, &place),
                    })
                    .collect();
                // Project admin: `can_admin` on the project, or, for a namespace that is no
                // project's, on the namespace (an org admin's). Destroying data asks the
                // freshest answer (D1611).
                let admin = match &place.project {
                    Some(project) => CheckRequest::new(
                        subject.clone(),
                        "can_admin",
                        ObjectRef::new("project", project),
                    ),
                    None => environment_check(&subject, "can_admin", namespace, &place),
                };
                checks.push(admin.consistency(Consistency::HigherConsistency));
                let allowed = self.batch(checks).await?;
                let relation = RELATIONS
                    .iter()
                    .zip(&allowed)
                    .find(|(_, allowed)| **allowed)
                    .map(|((relation, _), _)| *relation);
                (relation, allowed.last().copied().unwrap_or(false))
            }
        };
        Ok(Standing {
            relation,
            protected: place.protected,
            project_admin,
            in_environment: place.in_environment,
        })
    }

    /// `can_read` on each graph, in one batch; the fence and the credential's scopes apply as in
    /// [`decide`].
    async fn visible(
        &self,
        caller: &Caller,
        namespace: &str,
        graphs: &[String],
    ) -> Result<Vec<bool>, ConnectError> {
        if graphs.is_empty() {
            return Ok(Vec::new());
        }
        let who = self.directory.who(caller).await?;
        let place = self.directory.place(caller, namespace).await?;
        let readable: Vec<bool> = match who {
            Who::Outside => vec![false; graphs.len()],
            Who::Operator => vec![true; graphs.len()],
            Who::Subject(subject) => {
                let checks = graphs
                    .iter()
                    .map(|graph| graph_check(&subject, "can_read", namespace, graph, &place))
                    .collect();
                self.batch(checks).await?
            }
        };
        Ok(readable
            .into_iter()
            .map(|readable| {
                let standing = Standing {
                    relation: readable.then_some(Relation::Reader),
                    protected: place.protected,
                    project_admin: false,
                    in_environment: place.in_environment,
                };
                decide(caller, &standing, Need::At(Relation::Reader)).is_ok()
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use loams_authz::BuiltinAuthorizer;

    use super::*;
    use crate::authz::CallerKind;

    /// A fixed directory: callers by id, namespaces by name.
    #[derive(Debug, Default)]
    struct Fixed {
        who: BTreeMap<String, Who>,
        places: BTreeMap<String, Place>,
    }

    #[async_trait]
    impl GraphDirectory for Fixed {
        async fn who(&self, caller: &Caller) -> Result<Who, ConnectError> {
            Ok(self.who.get(&caller.id).cloned().unwrap_or(Who::Outside))
        }

        async fn place(&self, caller: &Caller, namespace: &str) -> Result<Place, ConnectError> {
            let mut place = self.places.get(namespace).cloned().unwrap_or_default();
            place.in_environment = caller.environment.as_deref().is_none_or(|e| e == namespace);
            Ok(place)
        }
    }

    fn t(user: &str, relation: &str, object: &str) -> Tuple {
        Tuple::new(user, relation, object)
    }

    /// `shop-dev` is project `shop`'s environment, `shop-prod` its protected one; `scratch` is
    /// the org's own. `dev` is a project editor, `own` the org admin, `guest` reads one graph of
    /// `shop-dev` only, and `bot` is an agent granted `shop-dev` as an editor.
    fn authorizer() -> ModelGraphAuthorizer<Fixed> {
        let model = BuiltinAuthorizer::new();
        model
            .write(&[
                t("org:acme", "org", "project:shop"),
                t("project:shop", "project", "environment:shop-dev"),
                t("project:shop", "project", "environment:shop-prod"),
                t("user:own", "admin", "org:acme"),
                t("user:dev", "member", "org:acme"),
                t("user:dev", "editor", "project:shop"),
                t("user:guest", "member", "org:acme"),
                t("user:guest", "reader", "graph:shop-dev/kg"),
                t("agent:bot", "editor", "environment:shop-dev"),
            ])
            .expect("model v1 tuples");
        let mut fixed = Fixed::default();
        for id in ["own", "dev", "guest"] {
            fixed.who.insert(id.into(), Who::Subject(Subject::user(id)));
        }
        fixed
            .who
            .insert("bot".into(), Who::Subject(Subject::agent("bot")));
        fixed.who.insert("root".into(), Who::Operator);
        for (ns, protected) in [("shop-dev", false), ("shop-prod", true)] {
            fixed.places.insert(
                ns.into(),
                Place {
                    protected,
                    project: Some("shop".into()),
                    ..Place::default()
                },
            );
        }
        fixed.places.insert(
            "scratch".into(),
            Place {
                roots: vec![t("org:acme", "org", "environment:scratch")],
                ..Place::default()
            },
        );
        ModelGraphAuthorizer::new(fixed, Arc::new(model))
    }

    fn caller(id: &str) -> Caller {
        Caller::new(id, CallerKind::User)
    }

    async fn relation(a: &ModelGraphAuthorizer<Fixed>, id: &str, ns: &str) -> Option<Relation> {
        a.standing(&caller(id), ns, Some("kg"))
            .await
            .expect("standing")
            .relation
    }

    #[tokio::test]
    async fn relations_come_from_the_model() {
        let a = authorizer();
        assert_eq!(relation(&a, "own", "shop-dev").await, Some(Relation::Admin));
        assert_eq!(
            relation(&a, "dev", "shop-dev").await,
            Some(Relation::Writer)
        );
        assert_eq!(
            relation(&a, "bot", "shop-dev").await,
            Some(Relation::Writer)
        );
        assert_eq!(
            relation(&a, "guest", "shop-dev").await,
            Some(Relation::Reader)
        );
        assert_eq!(relation(&a, "guest", "shop-prod").await, None);
        assert_eq!(
            relation(&a, "root", "shop-prod").await,
            Some(Relation::Admin)
        );
        assert_eq!(relation(&a, "nobody", "shop-dev").await, None);
        // `scratch` belongs to the org through the contextual root: a member writes it.
        assert_eq!(relation(&a, "dev", "scratch").await, Some(Relation::Writer));
        assert_eq!(relation(&a, "own", "scratch").await, Some(Relation::Admin));
    }

    #[tokio::test]
    async fn project_admin_and_protection() {
        let a = authorizer();
        let own = a.standing(&caller("own"), "shop-prod", None).await.unwrap();
        assert!(own.protected && own.project_admin);
        let dev = a.standing(&caller("dev"), "shop-prod", None).await.unwrap();
        assert!(dev.protected && !dev.project_admin);
        assert_eq!(dev.relation, Some(Relation::Writer));
        assert!(decide(&caller("dev"), &dev, Need::Destroy).is_err());
        assert!(decide(&caller("own"), &own, Need::Destroy).is_ok());
    }

    #[tokio::test]
    async fn a_graph_granted_directly_is_listed() {
        let a = authorizer();
        let graphs = vec!["kg".to_owned(), "other".to_owned()];
        assert_eq!(
            a.visible(&caller("guest"), "shop-dev", &graphs)
                .await
                .unwrap(),
            vec![true, false],
            "guest reads kg only, by a direct tuple"
        );
        assert_eq!(
            a.visible(&caller("dev"), "shop-dev", &graphs)
                .await
                .unwrap(),
            vec![true, true]
        );
        assert_eq!(
            a.visible(&caller("nobody"), "shop-dev", &graphs)
                .await
                .unwrap(),
            vec![false, false]
        );
    }

    #[tokio::test]
    async fn the_environment_fence_and_scopes_still_apply() {
        let a = authorizer();
        let graphs = vec!["kg".to_owned()];
        let mut fenced = caller("dev");
        fenced.environment = Some("shop-prod".into());
        assert_eq!(
            a.visible(&fenced, "shop-dev", &graphs).await.unwrap(),
            vec![false],
            "a credential bound to shop-prod does not reach shop-dev"
        );
        let mut scoped = caller("dev");
        scoped.scopes = Some(["graph:read".to_owned()].into_iter().collect());
        let standing = a.standing(&scoped, "shop-dev", Some("kg")).await.unwrap();
        assert!(decide(&scoped, &standing, Need::At(Relation::Reader)).is_ok());
        assert!(decide(&scoped, &standing, Need::At(Relation::Writer)).is_err());
    }
}
