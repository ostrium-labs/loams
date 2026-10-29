//! Who may do what with Loams Graph on the main port (GR1 Task 24, design §48 §11, D750): the
//! caller of a graph RPC, and its standing on a graph, asked through `loams-authz` as the
//! `graph` type's `can_read`, `can_write` and `can_admin` (design 76 §9, IA2 Task 4;
//! [`crate::auth::authz`] says who the caller is and where the tuples come from).
//!
//! In built-in single-org mode, a caller's relation on a namespace's graphs (every graph of a
//! namespace alike) is:
//!
//! - **The operator** (the developer on loopback with `LOAMS_AUTH_MODE=off`, a key from
//!   `LOAMS_API_KEYS_FILE`, any API key before first-run setup): `admin`, and project admin.
//! - **An org `owner` or `admin`**: `admin` on every namespace, and project admin (they
//!   administer every project).
//! - **A `member`**, in a namespace that is a project's environment: the project role their
//!   grants give them, directly or through a team (`viewer` → `reader`, `developer` → `writer`,
//!   `admin` → `admin` and project admin); none without a grant. A service account's key reaches
//!   its own project's environments as a `writer`. An agent reaches its granted projects'.
//! - **A `member`** (or a service account's key), in a namespace that is no project's
//!   environment: `writer` (as `member` writes Loams Git repositories, GT1 Task 16).
//! - **Anyone else** (not in the org, no principal): nothing.
//!
//! A credential bound to an environment (`<project>/<environment>`) reaches only that
//! environment's namespace, and its scopes cap the relation (`graph:read|write|admin`); both are
//! [`loams_graph::authz::decide`]'s.

use std::sync::Arc;

use async_trait::async_trait;
use connectrpc::{ConnectError, ErrorCode};
use loams_graph::authz::{Caller, CallerKind, GraphAuthorizer, PrincipalReads};
use loams_graph::model_authz::{GraphDirectory, ModelGraphAuthorizer, Place, Who as Model};
use loams_proto::loams::admin::v1 as admin_pb;

use crate::auth::authorizer::Denied;
use crate::auth::authz::{self, Standing as Who};
use crate::auth::directory::Doc;
use crate::auth::{Grant, Principal, PrincipalKind, Principals};

/// The graph caller of an admitted principal (or of none: [`Caller::anonymous`]).
pub(crate) fn caller_of(principal: Option<&Principal>) -> Caller {
    let Some(principal) = principal else {
        return Caller::anonymous();
    };
    let kind = match principal.kind {
        PrincipalKind::User => CallerKind::User,
        PrincipalKind::Session => CallerKind::Session,
        PrincipalKind::ApiKey => CallerKind::ApiKey,
        PrincipalKind::Tailscale => CallerKind::Tailscale,
        PrincipalKind::DevLoopback => CallerKind::DevLoopback,
    };
    Caller {
        id: principal.subject.clone(),
        kind,
        issuer: principal.issuer.clone(),
        name: principal.display_name.clone(),
        email: principal.email.clone(),
        environment: principal.grant.environment.clone(),
        scopes: principal.grant.scopes.clone(),
    }
}

/// The principal a caller came from, for the org authorizer.
fn principal_of(caller: &Caller) -> Option<Principal> {
    let kind = match caller.kind {
        CallerKind::User => PrincipalKind::User,
        CallerKind::Session => PrincipalKind::Session,
        CallerKind::ApiKey => PrincipalKind::ApiKey,
        CallerKind::Tailscale => PrincipalKind::Tailscale,
        CallerKind::DevLoopback => PrincipalKind::DevLoopback,
        CallerKind::Anonymous => return None,
    };
    Some(Principal {
        subject: caller.id.clone(),
        kind,
        issuer: caller.issuer.clone(),
        display_name: caller.name.clone(),
        email: caller.email.clone(),
        grant: Grant {
            environment: caller.environment.clone(),
            scopes: caller.scopes.clone(),
            ..Grant::default()
        },
    })
}

/// The built-in RBAC over the principal store (module docs).
#[derive(Clone)]
pub(crate) struct DirectoryGraphAuthz {
    principals: Principals,
}

impl std::fmt::Debug for DirectoryGraphAuthz {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirectoryGraphAuthz")
            .finish_non_exhaustive()
    }
}

impl DirectoryGraphAuthz {
    pub(crate) fn new(principals: Principals) -> Self {
        Self { principals }
    }

    /// Who the caller is (`Who::Outside` with no principal).
    async fn resolve(&self, caller: &Caller) -> Result<Who, ConnectError> {
        let Some(principal) = principal_of(caller) else {
            return Ok(Who::Outside);
        };
        match authz::standing(&self.principals, &principal).await {
            Ok(who) => Ok(who),
            Err(Denied::SetupRequired | Denied::RoleRequired { .. }) => Ok(Who::Outside),
            Err(Denied::Unavailable(err)) => {
                tracing::warn!(error = %err, "reading the principal store for a graph check failed");
                Err(unavailable())
            }
        }
    }
}

fn unavailable() -> ConnectError {
    ConnectError::new(
        ErrorCode::Unavailable,
        "the principal store could not be read; retry",
    )
}

/// The project and environment whose namespace is `namespace`, if any.
fn environment_of<'d>(
    doc: &'d Doc,
    namespace: &str,
) -> Option<(
    &'d crate::auth::directory::ProjectRec,
    &'d admin_pb::Environment,
)> {
    doc.projects.iter().find_map(|project| {
        project
            .environments
            .iter()
            .find(|env| env.namespace == namespace)
            .map(|env| (project, env))
    })
}

/// Whether a credential bound to `environment` (`<project>/<environment>`, or a namespace's
/// name) reaches `namespace`.
fn reaches(doc: &Doc, environment: &str, namespace: &str) -> bool {
    if environment == namespace {
        return true;
    }
    let Some((project, env)) = environment.split_once('/') else {
        return false;
    };
    doc.project(project)
        .and_then(|p| p.environments.iter().find(|e| e.slug == env))
        .is_some_and(|e| e.namespace == namespace)
}

#[async_trait]
impl GraphDirectory for DirectoryGraphAuthz {
    async fn who(&self, caller: &Caller) -> Result<Model, ConnectError> {
        Ok(match self.resolve(caller).await? {
            Who::Outside => Model::Outside,
            Who::Operator => Model::Operator,
            Who::Subject(subject) => Model::Subject(subject),
        })
    }

    async fn place(&self, caller: &Caller, namespace: &str) -> Result<Place, ConnectError> {
        let doc = self.principals.directory.read().await.map_err(|err| {
            tracing::warn!(error = %err.0, "reading the principal store for a graph check failed");
            unavailable()
        })?;
        let place = environment_of(&doc, namespace);
        Ok(Place {
            protected: place.is_some_and(|(_, env)| env.protected),
            in_environment: caller
                .environment
                .as_deref()
                .is_none_or(|env| reaches(&doc, env, namespace)),
            project: place.map(|(project, _)| authz::project_id(project).to_owned()),
            roots: authz::namespace_roots(&doc, namespace),
        })
    }
}

/// The authorizer the server's graph services use.
pub(crate) fn authorizer(principals: &Principals) -> Arc<dyn GraphAuthorizer> {
    Arc::new(ModelGraphAuthorizer::new(
        DirectoryGraphAuthz::new(principals.clone()),
        principals.authz.clone(),
    ))
}

/// The graph caller of a request's reads through `loams-query` (GR1 R24.12): `principal`'s,
/// checked for `reader` by this authorizer ([`super::graph_caller`]).
pub(crate) fn graph_caller(
    principals: &Principals,
    principal: Option<&Principal>,
) -> loams_query::graph::GraphCaller {
    PrincipalReads::new(authorizer(principals), caller_of(principal)).into_graph_caller()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::SetupGate;
    use crate::auth::directory::{Directory, KeyRec, MemberRec, ProjectRec, Role, TeamRec};
    use loams_graph::authz::{Need, Relation, decide};

    fn user(id: &str) -> admin_pb::User {
        admin_pb::User {
            id: id.into(),
            name: id.into(),
            email: format!("{id}@example.com"),
            ..Default::default()
        }
    }

    fn member(id: &str, role: Role) -> MemberRec {
        MemberRec {
            user: user(id),
            role,
            joined_at_ms: 0,
            password: String::new(),
        }
    }

    fn grant(
        kind: admin_pb::SubjectKind,
        id: &str,
        role: admin_pb::ProjectRole,
    ) -> admin_pb::Grant {
        admin_pb::Grant {
            subject: admin_pb::SubjectRef {
                kind: kind.into(),
                id: id.into(),
                ..Default::default()
            }
            .into(),
            role: role.into(),
            ..Default::default()
        }
    }

    fn env(project: &str, slug: &str, namespace: &str, protected: bool) -> admin_pb::Environment {
        admin_pb::Environment {
            slug: slug.into(),
            project: project.into(),
            namespace: namespace.into(),
            protected,
            ..Default::default()
        }
    }

    /// An org with a project `shop` (environments `shop-dev`, and `shop-prod`, protected), its
    /// grants, a team, and a service account key of the project.
    async fn principals() -> Principals {
        use admin_pb::ProjectRole as R;
        use admin_pb::SubjectKind as K;
        let directory = Directory::in_memory();
        directory
            .update(|doc| {
                doc.org = Some(admin_pb::Org {
                    id: "org".into(),
                    slug: "acme".into(),
                    ..Default::default()
                });
                doc.members = vec![
                    member("own", Role::Owner),
                    member("viewer", Role::Member),
                    member("dev", Role::Member),
                    member("padmin", Role::Member),
                    member("teamed", Role::Member),
                    member("stranger", Role::Member),
                ];
                doc.teams = vec![TeamRec {
                    team: admin_pb::Team {
                        id: "tm_1".into(),
                        slug: "core".into(),
                        ..Default::default()
                    },
                    members: vec!["teamed".into()],
                }];
                doc.projects = vec![ProjectRec {
                    project: admin_pb::Project {
                        id: "pr_1".into(),
                        slug: "shop".into(),
                        ..Default::default()
                    },
                    grants: vec![
                        grant(K::SUBJECT_KIND_USER, "viewer", R::PROJECT_ROLE_VIEWER),
                        grant(K::SUBJECT_KIND_USER, "dev", R::PROJECT_ROLE_DEVELOPER),
                        grant(K::SUBJECT_KIND_USER, "padmin", R::PROJECT_ROLE_ADMIN),
                        grant(K::SUBJECT_KIND_TEAM, "tm_1", R::PROJECT_ROLE_DEVELOPER),
                    ],
                    environments: vec![
                        env("shop", "dev", "shop-dev", false),
                        env("shop", "prod", "shop-prod", true),
                    ],
                }];
                doc.keys = vec![KeyRec {
                    key: admin_pb::ApiKey {
                        id: "k_sa".into(),
                        principal: admin_pb::SubjectRef {
                            kind: K::SUBJECT_KIND_SERVICE_ACCOUNT.into(),
                            id: "sa_1".into(),
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
                }];
                Ok::<_, Denied>(())
            })
            .await
            .expect("seed the directory");
        Principals::new(directory, SetupGate::Closed)
    }

    fn session(id: &str) -> Caller {
        Caller::new(id, CallerKind::Session)
    }

    async fn relation(
        authz: &Arc<dyn GraphAuthorizer>,
        caller: &Caller,
        ns: &str,
    ) -> Option<Relation> {
        let standing = authz
            .standing(caller, ns, Some("g"))
            .await
            .expect("standing");
        decide(caller, &standing, Need::At(Relation::Reader))
            .ok()
            .and(standing.relation)
            .filter(|_| standing.in_environment)
    }

    #[tokio::test]
    async fn org_and_project_roles_expand_to_graph_relations() {
        let authz = authorizer(&principals().await);
        let cases = [
            ("own", "shop-prod", Some(Relation::Admin)),
            ("own", "elsewhere", Some(Relation::Admin)),
            ("viewer", "shop-dev", Some(Relation::Reader)),
            ("dev", "shop-dev", Some(Relation::Writer)),
            ("padmin", "shop-prod", Some(Relation::Admin)),
            ("teamed", "shop-dev", Some(Relation::Writer)),
            // A member with no grant in the project reaches none of its environments...
            ("stranger", "shop-dev", None),
            // ...and writes a namespace that is no project's.
            ("stranger", "elsewhere", Some(Relation::Writer)),
            ("nobody", "elsewhere", None),
        ];
        for (who, ns, expected) in cases {
            assert_eq!(
                relation(&authz, &session(who), ns).await,
                expected,
                "{who} on {ns}"
            );
        }
        assert_eq!(
            relation(&authz, &Caller::anonymous(), "elsewhere").await,
            None
        );
        let loopback = Caller::new("dev-loopback", CallerKind::DevLoopback);
        assert_eq!(
            relation(&authz, &loopback, "shop-prod").await,
            Some(Relation::Admin)
        );
        // The service account's key: its project's environments, and no other project's.
        let key = Caller::new("apikey:k_sa", CallerKind::ApiKey);
        assert_eq!(
            relation(&authz, &key, "shop-dev").await,
            Some(Relation::Writer)
        );
    }

    #[tokio::test]
    async fn protected_environments_and_project_admins() {
        let authz = authorizer(&principals().await);
        let prod = authz
            .standing(&session("padmin"), "shop-prod", Some("g"))
            .await
            .expect("standing");
        assert!(prod.protected && prod.project_admin);
        let dev = authz
            .standing(&session("dev"), "shop-dev", Some("g"))
            .await
            .expect("standing");
        assert!(!dev.protected && !dev.project_admin);
        // An org owner administers every project.
        let owner = authz
            .standing(&session("own"), "shop-prod", None)
            .await
            .expect("standing");
        assert!(owner.project_admin);
    }

    #[tokio::test]
    async fn an_environment_bound_credential_reaches_only_its_namespace() {
        let authz = authorizer(&principals().await);
        let bound = Caller {
            environment: Some("shop/dev".into()),
            ..session("own")
        };
        let at = |ns: &'static str| {
            let (authz, bound) = (authz.clone(), bound.clone());
            async move {
                authz
                    .standing(&bound, ns, Some("g"))
                    .await
                    .expect("standing")
                    .in_environment
            }
        };
        assert!(at("shop-dev").await);
        assert!(!at("shop-prod").await);
        assert!(!at("elsewhere").await);
        // A credential bound to a namespace by name reaches it (the LV1 T14-2 form).
        let named = Caller {
            environment: Some("elsewhere".into()),
            ..session("own")
        };
        assert!(
            authz
                .standing(&named, "elsewhere", None)
                .await
                .expect("standing")
                .in_environment
        );
    }

    /// GR1 R24.12: a search's `expand` and the SQL graph functions read as the request's
    /// principal, through the same RBAC: no relation (or another environment, or no principal)
    /// is `permission_denied` with need `reader`; a `viewer` and the operator read.
    #[tokio::test]
    async fn graph_reads_through_loams_query_need_reader() {
        use loams_query::ServiceError;
        let principals = principals().await;
        let person = |subject: &str, environment: Option<&str>| Principal {
            subject: subject.into(),
            kind: PrincipalKind::Session,
            issuer: None,
            display_name: subject.into(),
            email: None,
            grant: Grant {
                environment: environment.map(str::to_string),
                ..Grant::default()
            },
        };
        let read = |principal: Option<Principal>, ns: &'static str| {
            let principals = principals.clone();
            async move {
                graph_caller(&principals, principal.as_ref())
                    .authorize_read("graph_neighbors", ns, "kg")
                    .await
            }
        };
        for (who, principal, ns) in [
            (
                "a member with no grant",
                Some(person("stranger", None)),
                "shop-dev",
            ),
            (
                "a viewer bound to another environment",
                Some(person("viewer", Some("shop/prod"))),
                "shop-dev",
            ),
            ("no principal", None, "elsewhere"),
        ] {
            match read(principal, ns).await {
                Err(ServiceError::PermissionDenied { need, object, .. }) => {
                    assert_eq!(need, "reader", "{who}");
                    assert_eq!(object, format!("graph:{ns}/kg"), "{who}");
                }
                other => panic!("{who}: expected permission_denied, got {other:?}"),
            }
        }
        read(Some(person("viewer", None)), "shop-dev")
            .await
            .expect("a viewer reads");
        read(Some(crate::auth::Auth::dev_loopback()), "shop-prod")
            .await
            .expect("the operator reads");
    }

    #[test]
    fn a_principal_becomes_a_caller_with_its_grant() {
        let principal = Principal {
            subject: "apikey:k1".into(),
            kind: PrincipalKind::ApiKey,
            issuer: None,
            display_name: "ci".into(),
            email: None,
            grant: Grant {
                environment: Some("shop/dev".into()),
                scopes: Grant::scopes_of(["graph:read"]),
                ..Grant::default()
            },
        };
        let caller = caller_of(Some(&principal));
        assert_eq!(caller.kind, CallerKind::ApiKey);
        assert_eq!(caller.environment.as_deref(), Some("shop/dev"));
        assert_eq!(caller.scope_cap(), Some(Relation::Reader));
        assert_eq!(caller_of(None).kind, CallerKind::Anonymous);
        assert_eq!(
            principal_of(&caller).map(|p| p.subject),
            Some("apikey:k1".into())
        );
    }
}
