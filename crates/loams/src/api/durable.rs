//! The durable API under `/durable/` on the main port (B12), behind the
//! port's authentication.
//!
//! The embedded Resonate server (D1) listens on its own loopback port
//! (`--durable-listen`, 127.0.0.1:8001 by default) with no authentication
//! (D111), so a browser could not reach it and a desktop could only reach it
//! on "This computer". Here its one protocol endpoint, `POST /` of a request
//! envelope, is also `POST /durable/` on the main port, the prefix the
//! console's durable client (`@loams/durable-client`) and the desktop's
//! protocol proxy already use. It answers what the loopback listener answers
//! (`loams_durable::DurableGateway`), and that listener keeps working.
//!
//! ## Who may call it
//!
//! Durable promises, schedules and tasks belong to the instance, not to a
//! namespace, so the check is the org's: the authentication layer admits the
//! caller, and then
//!
//! - the operator (sign-in off, a key from `LOAMS_API_KEYS_FILE`, any API
//!   key before first-run setup) may call it;
//! - anyone else needs both the org role `member` and, through
//!   `loams-authz`, `can_run_durable` on `org:<id>` (model v1: any member and
//!   the org's service accounts, never an agent; design 76 §9, plan IA1
//!   ruling T4.R4). The model only narrows what the role gives: a key acting
//!   as a person is checked with that person's live membership;
//! - a credential bound to one environment may not (it reaches one
//!   namespace, and this is no namespace's), nor may anyone outside the org.
//!
//! A refusal is the `{code, message}` JSON body the console's envelope
//! client reads (`403 permission_denied`); no durable server here is
//! `404 durable_not_served`.
//!
//! ## Where it is served
//!
//! Like Loams Live (LV1 Task 18), only on a main port that is loopback, or
//! that has TLS and authentication on (`Server::assemble`), and never on a
//! cluster node (whose main port is not guarded).

use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use serde_json::json;

use super::AppState;
use crate::auth::authorizer::Denied;
use crate::auth::authz::{self, Standing};
use crate::auth::directory::Role;
use crate::auth::{Principal, PrincipalKind, Principals};

/// The path prefix the durable API is served under.
pub const PREFIX: &str = "/durable/";

/// The durable server's protocol endpoint: the HTTP status and JSON body a
/// `POST /` of `body` gets (`loams_durable::DurableGateway`).
#[async_trait]
pub trait DurableEndpoint: Send + Sync + std::fmt::Debug + 'static {
    async fn handle(&self, body: &[u8]) -> (u16, Vec<u8>);
}

#[cfg(feature = "durable")]
#[async_trait]
impl DurableEndpoint for loams_durable::DurableGateway {
    async fn handle(&self, body: &[u8]) -> (u16, Vec<u8>) {
        loams_durable::DurableGateway::handle(self, body).await
    }
}

/// The durable endpoint the main port serves, when it serves one.
pub type DurableHandle = Arc<dyn DurableEndpoint>;

fn refusal(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        axum::Json(json!({ "code": code, "message": message })),
    )
        .into_response()
}

/// Whether `principal` may call the durable API (module docs).
async fn allowed(principals: &Principals, principal: Option<&Principal>) -> Result<(), Response> {
    let denied = || {
        refusal(
            StatusCode::FORBIDDEN,
            "permission_denied",
            "durable execution needs a member of the org, with a credential not bound to one environment",
        )
    };
    let Some(principal) = principal else {
        return Err(denied());
    };
    if principal.grant.environment.is_some() {
        return Err(denied());
    }
    if principal.kind == PrincipalKind::DevLoopback {
        return Ok(());
    }
    let unavailable = |err: &dyn std::fmt::Display| {
        tracing::warn!(error = %err, "authorizing a durable call failed");
        refusal(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "authorization could not be answered; retry",
        )
    };
    match principals
        .authorizer
        .authorize(principal, Role::Member)
        .await
    {
        Ok(_) => {}
        // Before first-run setup the only keys are the operator's own.
        Err(Denied::SetupRequired) if principal.kind == PrincipalKind::ApiKey => return Ok(()),
        Err(Denied::SetupRequired | Denied::RoleRequired { .. }) => return Err(denied()),
        Err(Denied::Unavailable(err)) => return Err(unavailable(&err)),
    }
    let subject = match authz::standing(principals, principal).await {
        Ok(Standing::Operator) => return Ok(()),
        Ok(Standing::Outside) => return Err(denied()),
        Ok(Standing::Subject(subject)) => subject,
        Err(err) => return Err(unavailable(&format!("{err:?}"))),
    };
    let org = match principals.directory.read().await {
        Ok(doc) => match doc.org.as_ref() {
            Some(org) => org.id.clone(),
            None => return Err(denied()),
        },
        Err(err) => return Err(unavailable(&err.0)),
    };
    let check = loams_authz::CheckRequest::new(
        subject,
        "can_run_durable",
        loams_authz::ObjectRef::new("org", org),
    );
    match principals.authz.check(check).await {
        Ok(decision) if decision.allowed => Ok(()),
        Ok(_) => Err(denied()),
        Err(err) => Err(unavailable(&err)),
    }
}

/// `POST /durable/`: one protocol envelope, answered by the embedded server.
pub(super) async fn envelope(
    State(state): State<AppState>,
    principal: Option<axum::Extension<Principal>>,
    body: Bytes,
) -> Response {
    serve(
        state.durable.as_ref(),
        &state.principals,
        principal.as_deref(),
        &body,
    )
    .await
}

/// [`envelope`], over what it reads from the request and the state.
async fn serve(
    durable: Option<&DurableHandle>,
    principals: &Principals,
    principal: Option<&Principal>,
    body: &[u8],
) -> Response {
    let Some(durable) = durable else {
        return refusal(
            StatusCode::NOT_FOUND,
            "durable_not_served",
            "durable execution is not served on this port: the engine runs without its \
             durable server (--no-durable, a build without `durable`, or a cluster node), or \
             this main port is neither loopback nor behind TLS and authentication",
        );
    };
    if let Err(refused) = allowed(principals, principal).await {
        return refused;
    }
    let (status, body) = durable.handle(body).await;
    (
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        [(header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::directory::{Directory, MemberRec};
    use crate::auth::{Grant, SetupGate};
    use loams_proto::loams::admin::v1 as admin_pb;

    async fn principals() -> Principals {
        let directory = Directory::in_memory();
        directory
            .update(|doc| {
                doc.org = Some(admin_pb::Org {
                    id: "org".into(),
                    slug: "acme".into(),
                    ..Default::default()
                });
                doc.members = ["own", "mem"]
                    .into_iter()
                    .zip([Role::Owner, Role::Member])
                    .map(|(id, role)| MemberRec {
                        user: admin_pb::User {
                            id: id.into(),
                            name: id.into(),
                            ..Default::default()
                        },
                        role,
                        joined_at_ms: 0,
                        password: String::new(),
                    })
                    .collect();
                Ok::<_, Denied>(())
            })
            .await
            .expect("seed");
        Principals::new(directory, SetupGate::Closed)
    }

    fn session(id: &str, environment: Option<&str>) -> Principal {
        Principal {
            subject: id.into(),
            kind: PrincipalKind::Session,
            issuer: None,
            display_name: id.into(),
            email: None,
            grant: Grant {
                environment: environment.map(str::to_owned),
                ..Grant::default()
            },
        }
    }

    /// A key that acts as `gone`, made when they were an owner, left behind by a removal: the
    /// role stored on it says owner, the model (their live membership) says nobody.
    async fn stale_key(p: &Principals) -> Principal {
        use crate::auth::directory::KeyRec;
        p.directory
            .update(|doc| {
                doc.keys.push(KeyRec {
                    key: admin_pb::ApiKey {
                        id: "k_gone".into(),
                        principal: admin_pb::SubjectRef {
                            kind: admin_pb::SubjectKind::SUBJECT_KIND_USER.into(),
                            id: "gone".into(),
                            ..Default::default()
                        }
                        .into(),
                        ..Default::default()
                    },
                    project: String::new(),
                    hash: String::new(),
                    role: Role::Owner,
                    idempotency_key: "k".into(),
                    created_by: "gone".into(),
                    revoked: false,
                });
                Ok::<_, Denied>(())
            })
            .await
            .expect("seed the key");
        Principal {
            kind: PrincipalKind::ApiKey,
            issuer: None,
            ..session("apikey:k_gone", None)
        }
    }

    #[tokio::test]
    async fn who_may_call_the_durable_api() {
        let p = principals().await;
        let key = Principal {
            kind: PrincipalKind::ApiKey,
            issuer: None,
            ..session("apikey:file", None)
        };
        let cases = [
            ("the owner", Some(session("own", None)), true),
            ("a member", Some(session("mem", None)), true),
            ("sign-in off", Some(crate::auth::Auth::dev_loopback()), true),
            ("a key from the keys file", Some(key), true),
            (
                "the owner bound to one environment",
                Some(session("own", Some("shop/dev"))),
                false,
            ),
            (
                "someone outside the org",
                Some(session("nobody", None)),
                false,
            ),
            ("anonymous", None, false),
            (
                "a key acting as someone who left the org",
                Some(stale_key(&p).await),
                false,
            ),
        ];
        for (who, principal, expected) in cases {
            assert_eq!(
                allowed(&p, principal.as_ref()).await.is_ok(),
                expected,
                "{who}"
            );
        }
    }

    /// An endpoint that echoes the body with status 201, as a server would.
    #[derive(Debug)]
    struct Echo;

    #[async_trait]
    impl DurableEndpoint for Echo {
        async fn handle(&self, body: &[u8]) -> (u16, Vec<u8>) {
            (201, body.to_vec())
        }
    }

    async fn read(response: Response) -> (StatusCode, serde_json::Value) {
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("a body");
        (status, serde_json::from_slice(&bytes).expect("json"))
    }

    #[tokio::test]
    async fn the_route_answers_the_server_or_says_why_not() {
        let p = principals().await;
        let echo: DurableHandle = Arc::new(Echo);
        let body = br#"{"kind":"promise.get"}"#;
        let (status, json) =
            read(serve(Some(&echo), &p, Some(&session("mem", None)), body).await).await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(json["kind"], "promise.get");

        let (status, json) = read(serve(None, &p, Some(&session("own", None)), body).await).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(json["code"], "durable_not_served");

        let (status, json) =
            read(serve(Some(&echo), &p, Some(&session("nobody", None)), body).await).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(json["code"], "permission_denied");
        assert!(
            json["message"].is_string(),
            "the envelope client shows the message"
        );
    }
}
