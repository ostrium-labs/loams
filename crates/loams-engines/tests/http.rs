//! The HTTP facade end to end over fake adapters, with and without IA2's enforcer.

#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::middleware::{Next, from_fn};
use axum::response::Response;
use loams_authz::{Authorizer, CheckRequest, Decision, ObjectRef, Subject};
use loams_engines::http::{router, router_with_authz};
use loams_engines::testing::FakeEngine;
use loams_engines::{EngineKind, EngineRegistry, EngineService};
use serde_json::{Value, json};
use tower::ServiceExt;

fn service() -> Arc<EngineService> {
    let registry = EngineRegistry::new()
        .with(Arc::new(FakeEngine::new(EngineKind::Risingwave)))
        .with(Arc::new(FakeEngine::new(EngineKind::Flink)))
        .with(Arc::new(FakeEngine::new(EngineKind::Spark)));
    Arc::new(EngineService::new(registry))
}

async fn call(
    app: &Router,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    let v = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, v)
}

fn create_body(name: &str, kind: &str) -> Value {
    json!({ "name": name, "kind": kind, "size": { "tier": "xs" } })
}

#[tokio::test]
async fn lifecycle_over_http() {
    let app = router(service());
    let base = "/v1/namespaces/p1/engines";

    let (s, cat) = call(&app, Method::GET, "/v1/engines", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(cat["engines"].as_array().unwrap().len(), 3);
    assert!(
        cat["engines"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["stability"] == "preview" && e["enabled"] == true)
    );

    let (s, rw) = call(
        &app,
        Method::POST,
        base,
        Some(create_body("rw", "risingwave")),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{rw}");
    assert_eq!(rw["kind"], "risingwave");
    assert_eq!(rw["status"]["phase"], "running");
    assert_eq!(rw["size"]["vcpu_milli"], 500);
    let id = rw["id"].as_str().unwrap().to_owned();
    let one = format!("{base}/{id}");

    let (s, _) = call(&app, Method::POST, base, Some(create_body("sp", "spark"))).await;
    assert_eq!(s, StatusCode::ACCEPTED);

    let (s, list) = call(&app, Method::GET, &format!("{base}?kind=risingwave"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(list["instances"].as_array().unwrap().len(), 1);
    let (s, all) = call(&app, Method::GET, base, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(all["instances"].as_array().unwrap().len(), 2);
    let (s, e) = call(&app, Method::GET, &format!("{base}?kind=kafka"), None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(e["error"], "invalid_argument");

    let (s, sql) = call(
        &app,
        Method::POST,
        &format!("{one}/sql"),
        Some(json!({ "sql": "select 1" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{sql}");
    assert_eq!(sql["columns"][0]["type"], "varchar");
    assert_eq!(sql["rows"][0][0], "select 1");

    let (s, scaled) = call(
        &app,
        Method::PATCH,
        &one,
        Some(json!({ "size": { "tier": "m", "replicas": 3 } })),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{scaled}");
    assert_eq!(scaled["size"]["replicas"], 3);

    let (s, sus) = call(&app, Method::POST, &format!("{one}/suspend"), None).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    assert_eq!(sus["status"]["phase"], "suspended");
    let (s, e) = call(&app, Method::POST, &format!("{one}/suspend"), None).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(e["error"], "failed_precondition");
    let (s, _) = call(&app, Method::POST, &format!("{one}/resume"), None).await;
    assert_eq!(s, StatusCode::ACCEPTED);

    let (s, usage) = call(
        &app,
        Method::GET,
        &format!("{one}/usage?from=2026-01-01T00%3A00%3A00Z"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert!(usage["samples"].is_array());
    assert!(usage["totals"]["cpu_ms"].is_u64());

    let (s, gone) = call(&app, Method::DELETE, &one, None).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    assert_eq!(gone["status"]["phase"], "deleted");
    let (s, e) = call(&app, Method::GET, &one, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(e["error"], "not_found");
}

#[tokio::test]
async fn bad_bodies_answer_the_error_shape() {
    let app = router(service());
    let base = "/v1/namespaces/p1/engines";
    for body in [
        json!({ "name": "x" }),
        json!({ "name": "x", "kind": "risingwave", "size": { "tier": "xxl" } }),
        json!({ "name": "x", "kind": "risingwave", "size": { "tier": "xs" }, "extra": 1 }),
        json!({ "name": "x", "kind": "risingwave", "size": { "tier": "xs", "replicas": 0 } }),
    ] {
        let (s, e) = call(&app, Method::POST, base, Some(body.clone())).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(e["error"], "invalid_argument", "{body}");
        assert!(e["message"].is_string());
    }
    let (s, _) = call(&app, Method::POST, base, Some(create_body("dup", "flink"))).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let (s, e) = call(&app, Method::POST, base, Some(create_body("dup", "flink"))).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(e["error"], "already_exists");
}

#[tokio::test]
async fn spark_sessions_and_jobs_over_http() {
    let app = router(service());
    let base = "/v1/namespaces/p1/engines";
    let (_, sp) = call(&app, Method::POST, base, Some(create_body("sp", "spark"))).await;
    let one = format!("{base}/{}", sp["id"].as_str().unwrap());

    let (s, ses) = call(
        &app,
        Method::POST,
        &format!("{one}/sessions"),
        Some(json!({ "kind": "sql" })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{ses}");
    let sid = ses["id"].as_str().unwrap().to_owned();
    let (_, list) = call(&app, Method::GET, &format!("{one}/sessions"), None).await;
    assert_eq!(list["sessions"].as_array().unwrap().len(), 1);
    let (s, body) = call(&app, Method::DELETE, &format!("{one}/sessions/{sid}"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert!(body.is_null());

    let (s, job) = call(
        &app,
        Method::POST,
        &format!("{one}/jobs"),
        Some(json!({ "name": "q", "kind": "sql", "entrypoint": "select 1" })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{job}");
    let jid = job["id"].as_str().unwrap().to_owned();
    let (s, c) = call(
        &app,
        Method::POST,
        &format!("{one}/jobs/{jid}/cancel"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(c["state"], "cancelled");
    let (_, jobs) = call(&app, Method::GET, &format!("{one}/jobs"), None).await;
    assert_eq!(jobs["jobs"][0]["state"], "cancelled");

    // One job with its full status, and its log as text.
    let (s, one_job) = call(&app, Method::GET, &format!("{one}/jobs/{jid}"), None).await;
    assert_eq!(s, StatusCode::OK, "{one_job}");
    assert_eq!(one_job["id"], jid.as_str());
    assert!(one_job["detail"]["attempts"].is_array());
    let (s, _) = call(&app, Method::GET, &format!("{one}/jobs/nope"), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let req = Request::builder()
        .uri(format!("{one}/jobs/{jid}/logs?attempt=2"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(
        res.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/plain")
    );
    let text = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    assert!(String::from_utf8_lossy(&text).contains("attempt 2"));
    let (s, e) = call(
        &app,
        Method::GET,
        &format!("{one}/jobs/{jid}/logs?attempt=x"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(e["error"], "invalid_argument");

    // Schedules: create (a retry answers the same), list, refuse a different body, delete.
    let schedule = json!({
        "id": "nightly",
        "cron": "0 3 * * *",
        "job": { "name": "etl", "kind": "python", "entrypoint": "s3://b/etl.py", "retries": 2 }
    });
    for _ in 0..2 {
        let (s, created) = call(
            &app,
            Method::POST,
            &format!("{one}/schedules"),
            Some(schedule.clone()),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "{created}");
        assert_eq!(
            created["job"]["retries"], 2,
            "the engine's own fields pass through"
        );
    }
    let mut other = schedule.clone();
    other["cron"] = json!("0 4 * * *");
    let (s, _) = call(&app, Method::POST, &format!("{one}/schedules"), Some(other)).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, bad) = call(
        &app,
        Method::POST,
        &format!("{one}/schedules"),
        Some(json!({ "id": "x", "cron": "daily", "job": schedule["job"] })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{bad}");
    let (_, list) = call(&app, Method::GET, &format!("{one}/schedules"), None).await;
    assert_eq!(list["schedules"].as_array().unwrap().len(), 1);
    let (s, _) = call(
        &app,
        Method::DELETE,
        &format!("{one}/schedules/nightly"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = call(
        &app,
        Method::DELETE,
        &format!("{one}/schedules/nightly"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    let (s, e) = call(
        &app,
        Method::POST,
        &format!("{one}/sql"),
        Some(json!({ "sql": "select 1" })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(e["error"], "unsupported");
}

/// `user:viewer` may view namespace p1, `user:operator` operate it and `user:admin` administer
/// it, each relation implying the weaker ones. Nothing on any other namespace.
struct Model;

#[async_trait]
impl Authorizer for Model {
    async fn check(&self, req: CheckRequest) -> loams_authz::Result<Decision> {
        let rank = |r: &str| match r {
            "can_read" => 1,
            "can_write" => 2,
            "can_admin" => 3,
            _ => 99,
        };
        let held = match req.subject.id.as_str() {
            "viewer" => 1,
            "operator" => 2,
            "admin" => 3,
            _ => 0,
        };
        let ok = req.object == ObjectRef::new("environment", "p1") && rank(&req.relation) <= held;
        Ok(if ok {
            Decision::allow(None)
        } else {
            Decision::deny(None)
        })
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

/// A stand-in authentication layer: `x-user` names the subject, org `acme`.
async fn authn(mut req: Request, next: Next) -> Response {
    if let Some(user) = req
        .headers()
        .get("x-user")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
    {
        req.extensions_mut()
            .insert(Subject::user(user).with_org("acme"));
    }
    next.run(req).await
}

async fn as_user(
    app: &Router,
    user: &str,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> StatusCode {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("x-user", user)
        .header("content-type", "application/json")
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn the_enforcer_checks_namespace_relations() {
    let app = router_with_authz(service(), Arc::new(Model)).layer(from_fn(authn));
    let base = "/v1/namespaces/p1/engines";
    let create = Some(create_body("rw", "risingwave"));

    assert_eq!(
        as_user(&app, "operator", Method::POST, base, create.clone()).await,
        StatusCode::FORBIDDEN
    );
    let req = Request::builder()
        .method(Method::POST)
        .uri(base)
        .header("x-user", "admin")
        .header("content-type", "application/json")
        .body(Body::from(create.clone().unwrap().to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    let inst: Value =
        serde_json::from_slice(&to_bytes(res.into_body(), 1 << 20).await.unwrap()).unwrap();
    assert_eq!(
        inst["org"], "acme",
        "the billing org comes from the subject"
    );
    let one = format!("{base}/{}", inst["id"].as_str().unwrap());

    assert_eq!(
        as_user(&app, "viewer", Method::GET, base, None).await,
        StatusCode::OK
    );
    assert_eq!(
        as_user(&app, "viewer", Method::GET, &one, None).await,
        StatusCode::OK
    );
    assert_eq!(
        as_user(
            &app,
            "viewer",
            Method::POST,
            &format!("{one}/suspend"),
            None
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        as_user(
            &app,
            "operator",
            Method::POST,
            &format!("{one}/sql"),
            Some(json!({"sql": "select 1"}))
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(
        as_user(&app, "operator", Method::DELETE, &one, None).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        as_user(&app, "stranger", Method::GET, base, None).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        as_user(
            &app,
            "admin",
            Method::GET,
            "/v1/namespaces/p2/engines",
            None
        )
        .await,
        StatusCode::FORBIDDEN
    );
    // No subject at all.
    let anon = Request::builder().uri(base).body(Body::empty()).unwrap();
    assert_eq!(
        app.clone().oneshot(anon).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        as_user(&app, "admin", Method::DELETE, &one, None).await,
        StatusCode::ACCEPTED
    );
}
