//! Which namespaces a caller can reach, for `NamespaceService/ListNamespaces`
//! (the 0.1.0 console's Namespaces page, B7).
//!
//! A namespace is listed exactly when its data can be read: `can_read` on
//! `environment:<namespace>`, asked through `loams-authz` (design 76 §9, IA2
//! Task 4; [`crate::auth::authz`] says who the caller is and where the tuples
//! come from). In built-in single-org mode that gives:
//!
//! - **The operator** (the developer on loopback with `LOAMS_AUTH_MODE=off`,
//!   a key from `LOAMS_API_KEYS_FILE`, any API key before first-run setup) and
//!   an org **`owner` or `admin`**: every namespace.
//! - **A `member`**: every namespace that is no project's environment, and
//!   the environments of the projects their grants (direct or through a team)
//!   give them any role in. A service account's key: its own project's
//!   environments, and the namespaces that are no project's.
//! - **An agent** (a JWT linked to one): the environments of the projects it
//!   is granted, and nothing else.
//! - **Anyone else** (outside the org, or no principal): none.
//!
//! A credential bound to an environment (`<project>/<environment>`, or a
//! namespace's name) reaches only that environment's namespace, whatever its
//! role.
//!
//! This module is not behind the `graph` feature: listing namespaces is part
//! of every variant, and `graph_auth` stays the graph's own.

use connectrpc::{ConnectError, ErrorCode};

use crate::auth::authorizer::Denied;
use crate::auth::authz::{self, Standing};
use crate::auth::directory::Doc;
use crate::auth::{Principal, Principals};

/// The namespaces of `names` that `principal` reaches, in their order
/// (module docs).
pub(crate) async fn reachable(
    principals: &Principals,
    principal: Option<&Principal>,
    names: Vec<String>,
) -> Result<Vec<String>, ConnectError> {
    let Some(principal) = principal else {
        return Ok(Vec::new());
    };
    let standing = match authz::standing(principals, principal).await {
        Ok(standing) => standing,
        Err(Denied::Unavailable(err)) => {
            tracing::warn!(error = %err, "reading the principal store to list namespaces failed");
            return Err(unavailable());
        }
        Err(Denied::SetupRequired | Denied::RoleRequired { .. }) => Standing::Outside,
    };
    if matches!(standing, Standing::Outside) {
        return Ok(Vec::new());
    }
    if matches!(standing, Standing::Operator) && principal.grant.environment.is_none() {
        return Ok(names);
    }
    let doc = principals.directory.read().await.map_err(|err| {
        tracing::warn!(error = %err.0, "reading the principal store to list namespaces failed");
        unavailable()
    })?;
    let fenced: Vec<String> = names
        .into_iter()
        .filter(|ns| {
            principal
                .grant
                .environment
                .as_deref()
                .is_none_or(|bound| bound_reaches(&doc, bound, ns))
        })
        .collect();
    let Standing::Subject(subject) = standing else {
        return Ok(fenced);
    };
    let checks = fenced
        .iter()
        .map(|ns| authz::environment_check(&doc, &subject, "can_read", ns))
        .collect();
    let decisions = principals.authz.batch_check(checks).await.map_err(|err| {
        tracing::warn!(error = %err, "the authorizer could not answer a namespace listing");
        unavailable()
    })?;
    Ok(fenced
        .into_iter()
        .zip(decisions)
        .filter(|(_, d)| d.allowed)
        .map(|(ns, _)| ns)
        .collect())
}

/// Whether a credential bound to `environment` (`<project>/<environment>`,
/// or a namespace's name) reaches `ns`.
fn bound_reaches(doc: &Doc, environment: &str, ns: &str) -> bool {
    if environment == ns {
        return true;
    }
    let Some((project, env)) = environment.split_once('/') else {
        return false;
    };
    doc.project(project)
        .and_then(|p| p.environments.iter().find(|e| e.slug == env))
        .is_some_and(|e| e.namespace == ns)
}

fn unavailable() -> ConnectError {
    ConnectError::new(
        ErrorCode::Unavailable,
        "the principal store could not be read; retry",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::directory::{Directory, KeyRec, MemberRec, ProjectRec, Role, TeamRec};
    use crate::auth::{Grant, PrincipalKind, SetupGate};
    use loams_proto::loams::admin::v1 as admin_pb;

    fn member(id: &str, role: Role) -> MemberRec {
        MemberRec {
            user: admin_pb::User {
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

    fn env(project: &str, slug: &str, namespace: &str) -> admin_pb::Environment {
        admin_pb::Environment {
            slug: slug.into(),
            project: project.into(),
            namespace: namespace.into(),
            ..Default::default()
        }
    }

    /// An org with projects `shop` (environments `shop-dev`, `shop-prod`) and
    /// `ops` (`ops-prod`), grants in `shop` only, and a `shop` service key.
    async fn principals(setup: bool) -> Principals {
        use admin_pb::ProjectRole as R;
        use admin_pb::SubjectKind as K;
        let directory = Directory::in_memory();
        if setup {
            directory
                .update(|doc| {
                    doc.org = Some(admin_pb::Org {
                        id: "org".into(),
                        slug: "acme".into(),
                        ..Default::default()
                    });
                    doc.members = vec![
                        member("own", Role::Owner),
                        member("adm", Role::Admin),
                        member("viewer", Role::Member),
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
                    doc.projects = vec![
                        ProjectRec {
                            project: admin_pb::Project {
                                id: "pr_1".into(),
                                slug: "shop".into(),
                                ..Default::default()
                            },
                            grants: vec![
                                grant(K::SUBJECT_KIND_USER, "viewer", R::PROJECT_ROLE_VIEWER),
                                grant(K::SUBJECT_KIND_TEAM, "tm_1", R::PROJECT_ROLE_DEVELOPER),
                            ],
                            environments: vec![
                                env("shop", "dev", "shop-dev"),
                                env("shop", "prod", "shop-prod"),
                            ],
                        },
                        ProjectRec {
                            project: admin_pb::Project {
                                id: "pr_2".into(),
                                slug: "ops".into(),
                                ..Default::default()
                            },
                            grants: vec![],
                            environments: vec![env("ops", "prod", "ops-prod")],
                        },
                    ];
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
        }
        Principals::new(directory, SetupGate::Closed)
    }

    fn all() -> Vec<String> {
        ["default", "ops-prod", "scratch", "shop-dev", "shop-prod"]
            .map(String::from)
            .to_vec()
    }

    fn principal(subject: &str, kind: PrincipalKind, environment: Option<&str>) -> Principal {
        Principal {
            subject: subject.into(),
            kind,
            issuer: None,
            display_name: subject.into(),
            email: None,
            grant: Grant {
                environment: environment.map(str::to_owned),
                ..Grant::default()
            },
        }
    }

    fn session(id: &str, environment: Option<&str>) -> Principal {
        principal(id, PrincipalKind::Session, environment)
    }

    async fn list(principals: &Principals, who: Option<&Principal>) -> Vec<&'static str> {
        let reached = reachable(principals, who, all()).await.expect("reachable");
        all()
            .into_iter()
            .zip(["default", "ops-prod", "scratch", "shop-dev", "shop-prod"])
            .filter(|(ns, _)| reached.contains(ns))
            .map(|(_, name)| name)
            .collect()
    }

    #[tokio::test]
    async fn the_authz_matrix() {
        let p = principals(true).await;
        let every = vec!["default", "ops-prod", "scratch", "shop-dev", "shop-prod"];
        let unowned = vec!["default", "scratch"];
        let cases: Vec<(&str, Option<Principal>, Vec<&str>)> = vec![
            ("the owner", Some(session("own", None)), every.clone()),
            ("an org admin", Some(session("adm", None)), every.clone()),
            (
                "sign-in off (dev-loopback)",
                Some(crate::auth::Auth::dev_loopback()),
                every.clone(),
            ),
            (
                "a key from the keys file",
                Some(principal("apikey:file", PrincipalKind::ApiKey, None)),
                every.clone(),
            ),
            (
                "a project viewer",
                Some(session("viewer", None)),
                vec!["default", "scratch", "shop-dev", "shop-prod"],
            ),
            (
                "a viewer bound to one environment",
                Some(session("viewer", Some("shop/dev"))),
                vec!["shop-dev"],
            ),
            (
                "a viewer bound to an environment of a project they hold no role in",
                Some(session("viewer", Some("ops/prod"))),
                vec![],
            ),
            (
                "the owner bound to a namespace by name",
                Some(session("own", Some("ops-prod"))),
                vec!["ops-prod"],
            ),
            (
                "a team member",
                Some(session("teamed", None)),
                vec!["default", "scratch", "shop-dev", "shop-prod"],
            ),
            (
                "a member with no grant",
                Some(session("stranger", None)),
                unowned.clone(),
            ),
            (
                "the project's service key",
                Some(principal("apikey:k_sa", PrincipalKind::ApiKey, None)),
                vec!["default", "scratch", "shop-dev", "shop-prod"],
            ),
            (
                "someone outside the org",
                Some(session("nobody", None)),
                vec![],
            ),
            ("anonymous", None, vec![]),
        ];
        for (who, principal, expected) in cases {
            assert_eq!(list(&p, principal.as_ref()).await, expected, "{who}");
        }
    }

    #[tokio::test]
    async fn before_setup_only_the_operator_lists() {
        let p = principals(false).await;
        let every = vec!["default", "ops-prod", "scratch", "shop-dev", "shop-prod"];
        assert_eq!(
            list(&p, Some(&crate::auth::Auth::dev_loopback())).await,
            every
        );
        assert_eq!(
            list(
                &p,
                Some(&principal("apikey:file", PrincipalKind::ApiKey, None))
            )
            .await,
            every
        );
        assert!(list(&p, Some(&session("own", None))).await.is_empty());
        assert!(list(&p, None).await.is_empty());
    }
}
