//! The gateway module end to end: auth, tenant binding of sessions, the
//! option checks, and forwarding to a mock SQL Gateway.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use httpmock::Method::{GET, POST};
use httpmock::MockServer;
use loams_flink::TenantId;
use loams_flink::gateway::auth::{GrantsAuthorizer, KeyDigestAuthenticator, digest_hex};
use loams_flink::gateway::{GatewayState, StaticUpstreams, router};
use loams_flink::storage::ObjectStore;
use serde_json::{Value, json};
use tower::ServiceExt;

const KEY_T1: &str = "lk_01JABCDEFGHJKMNPQRSTVWXYZ0_abcdefghijklmnopqrstuvwxyz012345";
const KEY_T2: &str = "lk_01JABCDEFGHJKMNPQRSTVWXYZ1_abcdefghijklmnopqrstuvwxyz012345";
const S: &str = "2b0b1c7e-5d3c-4a1e-9f53-6f2b4c8a9d10";

fn state(upstream: &str) -> GatewayState {
    let keys = format!(
        "01JABCDEFGHJKMNPQRSTVWXYZ0 {} t1\n01JABCDEFGHJKMNPQRSTVWXYZ1 {} t2\n",
        digest_hex(KEY_T1),
        digest_hex(KEY_T2)
    );
    let upstreams = StaticUpstreams(HashMap::from([
        (TenantId::new("t1").unwrap(), upstream.to_owned()),
        (TenantId::new("t2").unwrap(), upstream.to_owned()),
    ]));
    GatewayState::new(
        Arc::new(KeyDigestAuthenticator::parse(&keys).unwrap()),
        Arc::new(GrantsAuthorizer),
        Arc::new(upstreams),
        ObjectStore::default(),
    )
}

async fn call(
    state: &GatewayState,
    method: &str,
    uri: &str,
    key: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(key) = key {
        req = req.header("authorization", format!("Bearer {key}"));
    }
    let body = body.map_or_else(Body::empty, |b| Body::from(b.to_string()));
    let resp = router(state.clone())
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

#[tokio::test]
async fn sessions_are_bound_to_the_tenant_that_opened_them() {
    let server = MockServer::start_async().await;
    let open = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/v3/sessions")
                .header_missing("authorization");
            then.status(200).json_body(json!({"sessionHandle": S}));
        })
        .await;
    let exec = server
        .mock_async(|when, then| {
            when.method(POST)
                .path(format!("/v3/sessions/{S}/statements"));
            then.status(200)
                .json_body(json!({"operationHandle": "op-1"}));
        })
        .await;
    let st = state(&server.base_url());

    let (status, body) = call(
        &st,
        "POST",
        "/flink/t1/v3/sessions",
        Some(KEY_T1),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["sessionHandle"], S);
    open.assert_async().await;

    // The owner may use it.
    let stmt = json!({"statement": "INSERT INTO a SELECT * FROM b"});
    let (status, _) = call(
        &st,
        "POST",
        &format!("/flink/t1/v3/sessions/{S}/statements"),
        Some(KEY_T1),
        Some(stmt.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    exec.assert_calls_async(1).await;

    // Another tenant cannot, even with its own valid key.
    let (status, _) = call(
        &st,
        "POST",
        &format!("/flink/t2/v3/sessions/{S}/statements"),
        Some(KEY_T2),
        Some(stmt.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Nor can t2's key reach t1's route.
    let (status, _) = call(
        &st,
        "POST",
        &format!("/flink/t1/v3/sessions/{S}/statements"),
        Some(KEY_T2),
        Some(stmt),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    exec.assert_calls_async(1).await;
}

#[tokio::test]
async fn unauthenticated_and_unlisted_requests_never_reach_flink() {
    let server = MockServer::start_async().await;
    let any = server
        .mock_async(|when, then| {
            when.method(GET);
            then.status(200).json_body(json!({}));
        })
        .await;
    let st = state(&server.base_url());
    let (status, _) = call(&st, "GET", "/flink/t1/v3/info", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = call(&st, "GET", "/flink/t1/jobs/overview", Some(KEY_T1), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(&st, "GET", "/flink/T1/v3/info", Some(KEY_T1), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    any.assert_calls_async(0).await;
}

#[tokio::test]
async fn protected_options_are_refused() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(POST).path("/v3/sessions");
            then.status(200).json_body(json!({"sessionHandle": S}));
        })
        .await;
    let st = state(&server.base_url());
    let (status, _) = call(
        &st,
        "POST",
        "/flink/t1/v3/sessions",
        Some(KEY_T1),
        Some(json!({"properties": {"state.checkpoints.dir": "s3://flink/flink/t2/checkpoints"}})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &st,
        "POST",
        "/flink/t1/v3/sessions",
        Some(KEY_T1),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = call(
        &st,
        "POST",
        &format!("/flink/t1/v3/sessions/{S}/configure-session"),
        Some(KEY_T1),
        Some(json!({"statement": "SET 'high-availability.type' = 'NONE'"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, _) = call(
        &st,
        "POST",
        &format!("/flink/t1/v3/sessions/{S}/statements"),
        Some(KEY_T1),
        Some(json!({
            "statement": "INSERT INTO a SELECT * FROM b",
            "executionConfig": {"execution.state-recovery.path": "s3://flink/flink/t2/savepoints/sp"}
        })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
