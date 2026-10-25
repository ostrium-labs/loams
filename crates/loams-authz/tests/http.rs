//! IA2 Task 2: the deny-by-default middleware.

#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::middleware::{Next, from_fn, from_fn_with_state};
use axum::response::Response;
use axum::routing::{delete, get};
use loams_authz::http::{DecisionLog, DecisionRecord, Declared, Enforcer, Need, enforce, segments};
use loams_authz::{
    Authorizer, AuthzError, CheckRequest, Consistency, Decision, ObjectRef, RecordingAuthorizer,
    Result, Subject,
};
use tower::ServiceExt;

/// Grants `agent:a1` `can_view` and `can_operate` on `deployment:dev-web`; fails for `deployment:broken`.
struct Model;

#[async_trait]
impl Authorizer for Model {
    async fn check(&self, req: CheckRequest) -> Result<Decision> {
        if req.object.id == "broken" {
            return Err(AuthzError::Unavailable("down".into()));
        }
        let ok = req.subject.fga_user() == "agent:a1"
            && req.object == ObjectRef::new("deployment", "dev-web")
            && matches!(req.relation.as_str(), "can_view" | "can_operate");
        Ok(if ok {
            Decision::allow(Some("m1".into()))
        } else {
            Decision::deny(Some("m1".into()))
        })
    }

    async fn list_objects(&self, _: &Subject, _: &str, _: &str) -> Result<Vec<ObjectRef>> {
        Ok(Vec::new())
    }
}

fn routes(m: &Method, path: &str) -> Option<Declared> {
    let seg = segments(path)?;
    let seg: Vec<&str> = seg.iter().map(String::as_str).collect();
    let dep = |id: &str| ObjectRef::new("deployment", id);
    match (m, seg.as_slice()) {
        (&Method::GET, ["healthz"]) => Some(Declared::Public),
        (&Method::GET, ["whoami"]) => Some(Declared::Authenticated),
        (&Method::GET, ["deployments", id]) => {
            Need::new("can_view", dep(id)).ok().map(Declared::Check)
        }
        (&Method::DELETE, ["deployments", id]) => Need::new("can_destroy", dep(id))
            .ok()
            .map(|n| Declared::Check(n.strict())),
        _ => None,
    }
}

#[derive(Default)]
struct Log(Mutex<Vec<DecisionRecord>>);

impl DecisionLog for Log {
    fn record(&self, r: &DecisionRecord) {
        self.0.lock().unwrap().push(r.clone());
    }
}

/// Stands in for the authentication layer: `x-test-subject: agent:a1` becomes a `Subject`.
async fn fake_authn(mut req: Request, next: Next) -> Response {
    if let Some(s) = req
        .headers()
        .get("x-test-subject")
        .and_then(|v| v.to_str().ok())
    {
        let (kind, id) = s.split_once(':').unwrap();
        let subject = match kind {
            "agent" => Subject::agent(id),
            "user" => Subject::user(id),
            _ => Subject::service_account(id),
        };
        req.extensions_mut().insert(subject);
    }
    next.run(req).await
}

struct App {
    router: Router,
    log: Arc<Log>,
    authz: Arc<RecordingAuthorizer<Model>>,
}

fn app() -> App {
    let log = Arc::new(Log::default());
    let authz = Arc::new(RecordingAuthorizer::new(Model));
    let enforcer = Enforcer::new(authz.clone(), routes).with_log(log.clone());
    let router = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/whoami", get(|| async { "me" }))
        .route(
            "/deployments/{id}",
            get(|| async { "view" }).delete(|| async { "gone" }),
        )
        .route("/undeclared", get(|| async { "should never run" }))
        .route("/also-undeclared", delete(|| async { "should never run" }))
        .layer(from_fn_with_state(enforcer, enforce))
        .layer(from_fn(fake_authn));
    App { router, log, authz }
}

async fn call(app: &App, method: Method, path: &str, subject: Option<&str>) -> StatusCode {
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header("x-request-id", "req-1");
    if let Some(s) = subject {
        b = b.header("x-test-subject", s);
    }
    app.router
        .clone()
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn undeclared_route_is_403() {
    let a = app();
    assert_eq!(
        call(&a, Method::GET, "/undeclared", Some("agent:a1")).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&a, Method::DELETE, "/also-undeclared", None).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&a, Method::POST, "/deployments/dev-web", Some("agent:a1")).await,
        StatusCode::FORBIDDEN,
        "a declared path with an undeclared method"
    );
    assert!(
        a.authz.records().is_empty(),
        "an undeclared route never reaches the authorizer"
    );
}

#[tokio::test]
async fn missing_subject_is_401() {
    let a = app();
    assert_eq!(
        call(&a, Method::GET, "/deployments/dev-web", None).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&a, Method::GET, "/whoami", None).await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn public_and_authenticated_routes() {
    let a = app();
    assert_eq!(
        call(&a, Method::GET, "/healthz", None).await,
        StatusCode::OK
    );
    assert_eq!(
        call(&a, Method::GET, "/whoami", Some("user:u1")).await,
        StatusCode::OK
    );
    assert!(a.authz.records().is_empty());
}

#[tokio::test]
async fn allow_passes_through() {
    let a = app();
    assert_eq!(
        call(&a, Method::GET, "/deployments/dev-web", Some("agent:a1")).await,
        StatusCode::OK
    );
    let r = a.authz.records();
    assert_eq!(r.len(), 1);
    assert!(r[0].decision.as_ref().unwrap().allowed);
}

#[tokio::test]
async fn deny_is_403() {
    let a = app();
    assert_eq!(
        call(&a, Method::GET, "/deployments/prod-web", Some("agent:a1")).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&a, Method::GET, "/deployments/dev-web", Some("user:a1")).await,
        StatusCode::FORBIDDEN,
        "same id, other kind"
    );
}

#[tokio::test]
async fn authorizer_error_is_503() {
    let a = app();
    assert_eq!(
        call(&a, Method::GET, "/deployments/broken", Some("agent:a1")).await,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let log = a.log.0.lock().unwrap();
    assert_eq!(log.len(), 1);
    assert!(log[0].decision.is_none());
}

#[tokio::test]
async fn destructive_route_uses_higher_consistency() {
    let a = app();
    assert_eq!(
        call(&a, Method::DELETE, "/deployments/dev-web", Some("agent:a1")).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&a, Method::GET, "/deployments/dev-web", Some("agent:a1")).await,
        StatusCode::OK
    );
    let r = a.authz.records();
    assert_eq!(
        (r[0].request.relation.as_str(), r[0].request.consistency),
        ("can_destroy", Consistency::HigherConsistency)
    );
    assert_eq!(
        (r[1].request.relation.as_str(), r[1].request.consistency),
        ("can_view", Consistency::MinimizeLatency)
    );
}

#[test]
fn need_rejects_non_can_relation() {
    let o = ObjectRef::new("deployment", "d");
    assert!(Need::new("can_view", o.clone()).is_ok());
    for bad in ["editor", "admin", "viewer", "can_", "billing_admin"] {
        assert!(Need::new(bad, o.clone()).is_err(), "{bad}");
    }
}

#[tokio::test]
async fn decision_log_records_denials() {
    let a = app();
    call(&a, Method::GET, "/deployments/dev-web", Some("agent:a1")).await;
    call(&a, Method::GET, "/deployments/prod-web", Some("agent:a1")).await;
    let log = a.log.0.lock().unwrap();
    let got: Vec<(String, bool, Option<&str>)> = log
        .iter()
        .map(|r| {
            (
                r.object.to_string(),
                r.decision.as_ref().unwrap().allowed,
                r.request_id.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("deployment:dev-web".into(), true, Some("req-1")),
            ("deployment:prod-web".into(), false, Some("req-1"))
        ]
    );
    assert_eq!(
        log[1].decision.as_ref().unwrap().model_id.as_deref(),
        Some("m1")
    );
    assert_eq!(log[1].subject, Subject::agent("a1"));
}

#[tokio::test]
async fn percent_encoded_id_is_checked_decoded() {
    let a = app();
    // `%2D` is `-`: axum's `Path` hands the handler `dev-web`, so the check must name it too.
    assert_eq!(
        call(&a, Method::GET, "/deployments/dev%2Dweb", Some("agent:a1")).await,
        StatusCode::OK
    );
    let r = a.authz.records();
    assert_eq!(r[0].request.object, ObjectRef::new("deployment", "dev-web"));
    for path in [
        "/deployments/%2E%2E",
        "/deployments/..",
        "/deployments/%FF",
        "/deployments/dev-web/",
        "/deployments/",
        "/deployments/team%3Ax%23member",
        "/deployments/a%20b",
    ] {
        assert_eq!(
            call(&a, Method::GET, path, Some("agent:a1")).await,
            StatusCode::FORBIDDEN,
            "{path}"
        );
    }
    assert_eq!(
        a.authz.records().len(),
        1,
        "malformed ids never reach the authorizer"
    );
}

#[test]
fn segments_decode_like_axum_path() {
    assert_eq!(segments("/a/b%2Fc").unwrap(), ["a", "b/c"]);
    assert_eq!(segments("/a//b").unwrap(), ["a", "", "b"]);
    assert_eq!(segments("/").unwrap(), [""]);
    assert!(segments("a/b").is_none(), "not absolute");
    assert!(segments("/a/%C3").is_none(), "not UTF-8");
    assert!(segments("/a/.").is_none());
}

#[test]
fn need_rejects_malformed_object() {
    for (ty, id) in [
        ("deployment", ""),
        ("", "d"),
        ("deployment", "x#member"),
        ("team:x", "member"),
        ("deployment", "a b"),
        ("deployment", "a\nb"),
    ] {
        assert!(
            Need::new("can_view", ObjectRef::new(ty, id)).is_err(),
            "{ty}:{id}"
        );
    }
    assert!(Need::new("can_view", ObjectRef::new("graph", "ns/kg")).is_ok());
    assert!(Need::new("can_View", ObjectRef::new("graph", "g")).is_err());
}
