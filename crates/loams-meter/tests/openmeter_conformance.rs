//! OpenMeter conformance (ME1 Task 15): the OpenMeter-compatible routes
//! answer what OpenMeter's `api/openapi.yaml` (pinned commit, subset in
//! `tests/openmeter/openapi.subset.json`, extracted by
//! `scripts/meter/openmeter-subset.py`) says they answer.
//!
//! Each case calls the real router in process and validates the body
//! against the operation's response schema with a JSON Schema validator;
//! request bodies built from OpenMeter's own create-input schema are
//! accepted.

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use loams_authz::{BuiltinAuthorizer, Subject, Tuple};
use loams_meter::audit::MemAudit;
use loams_meter::authz::{Caller, MeterAuthz};
use loams_meter::customers::{Clock, Customers, NewCustomer};
use loams_meter::entitlements::{Entitlements, ValueSource};
use loams_meter::error::Result;
use loams_meter::store::MemStore;
use loams_meter_core::{
    Amount, AsOf, Balance, Entitlement, EntitlementKind, Grant, Interval, Mode, Period,
    UsagePeriod, UtcNanos,
};
use serde_json::{Value, json};
use tower::ServiceExt;

const SUBSET: &str = include_str!("openmeter/openapi.subset.json");

fn spec() -> Value {
    serde_json::from_str(SUBSET).expect("subset json")
}

/// The commit the subset was taken from must stay pinned (re-extract with
/// the script to move it).
#[test]
fn the_subset_is_pinned() {
    let s = spec();
    let src = &s["x-loams-source"];
    assert_eq!(src["repository"], "openmeterio/openmeter");
    assert_eq!(src["commit"].as_str().map(str::len), Some(40));
    assert_eq!(src["sha256"].as_str().map(str::len), Some(64));
    assert_eq!(src["license"], "Apache-2.0");
}

/// Validates `instance` against `schema` (a fragment of the subset that
/// may `$ref` its components).
fn check(schema: &Value, instance: &Value) {
    let mut root = schema.clone();
    root["components"] = spec()["components"].clone();
    let v = jsonschema::validator_for(&root).expect("schema compiles");
    let errors: Vec<String> = v.iter_errors(instance).map(|e| e.to_string()).collect();
    assert!(errors.is_empty(), "{instance}\n{errors:#?}");
}

fn response_schema(path: &str, method: &str, status: &str) -> Value {
    let s = spec();
    let op = &s["paths"][path][method];
    assert!(op.is_object(), "{method} {path} is not in OpenMeter's API");
    let content = &op["responses"][status]["content"];
    let media = if content["application/json"].is_object() {
        "application/json"
    } else {
        "application/problem+json"
    };
    let schema = content[media]["schema"].clone();
    assert!(
        schema.is_object(),
        "{method} {path} has no {status} response"
    );
    schema
}

const VALUE: &str =
    "/api/v1/subjects/{subjectIdOrKey}/entitlements/{entitlementIdOrFeatureKey}/value";
const GRANTS: &str =
    "/api/v1/subjects/{subjectIdOrKey}/entitlements/{entitlementIdOrFeatureKey}/grants";

#[derive(Debug)]
struct Fixed;

#[async_trait]
impl ValueSource for Fixed {
    async fn value(&self, e: &Entitlement, grants: &[Grant], at: UtcNanos) -> Result<Balance> {
        let total: Amount = grants
            .iter()
            .filter(|g| g.is_live(at))
            .map(|g| g.amount)
            .sum();
        let usage = Amount::parse("12.5").expect("usage");
        Ok(Balance {
            entitlement: e.id.clone(),
            period: Period {
                start: UtcNanos(0),
                end: UtcNanos(30 * UtcNanos::DAY),
            },
            has_access: total > usage,
            balance: total.saturating_sub(usage).clamp_non_negative(),
            usage,
            overage: usage.saturating_sub(total).clamp_non_negative(),
            total_available: total,
            grants: Vec::new(),
            prepaid_only: false,
            as_of: AsOf::Replay,
        })
    }
}

async fn app() -> axum::Router {
    let fga = BuiltinAuthorizer::new();
    fga.write(&[
        Tuple::new("platform:default", "platform", "org:acme"),
        Tuple::new("user:mia", "meter_admin", "platform:default"),
    ])
    .expect("tuples");
    let authz = MeterAuthz::new(Arc::new(fga), "meter-export");
    let store = Arc::new(MemStore::new());
    let audit = Arc::new(MemAudit::new());
    let clock: Clock = Arc::new(|| UtcNanos(1_700_000_000_000_000_000));
    let mia = Caller::Principal(Subject::user("mia"));
    Customers::new(store.clone(), authz.clone(), audit.clone(), clock.clone())
        .create(
            &mia,
            NewCustomer {
                org: "acme".into(),
                subjects: vec!["acme".into()],
                ..NewCustomer::default()
            },
        )
        .await
        .expect("customer");
    let ents = Entitlements::new(store, authz, audit, Arc::new(Fixed), clock);
    ents.create(
        &mia,
        "acme",
        "house_cpu",
        EntitlementKind::Metered {
            usage_period: UsagePeriod {
                interval: Interval::Month,
                anchor: UtcNanos(0),
            },
            mode: Mode::HardLimit,
            issue_after_reset: None,
            preserve_overage_at_reset: false,
        },
        "test",
        None,
    )
    .await
    .expect("metered");
    ents.create(
        &mia,
        "acme",
        "pg_branches",
        EntitlementKind::Static {
            config: json!({ "max": 10 }),
        },
        "test",
        None,
    )
    .await
    .expect("static");
    loams_meter::openmeter::router(ents).layer(axum::Extension(mia))
}

async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .expect("request");
    let res = app.clone().oneshot(req).await.expect("response");
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .expect("body");
    let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, v)
}

#[tokio::test]
async fn openmeter_entitlement_routes_conform() {
    let app = app().await;

    // createGrant: a body built from OpenMeter's own example shape.
    let input = json!({
        "amount": 100,
        "priority": 1,
        "effectiveAt": "2023-01-01T00:00:00Z",
        "expiration": { "duration": "MONTH", "count": 12 },
        "maxRolloverAmount": 50,
        "recurrence": { "interval": "MONTH", "anchor": "2023-01-01T00:00:00Z" },
        "metadata": { "reason": "conformance" }
    });
    let s = spec();
    check(
        &s["paths"][GRANTS]["post"]["requestBody"]["content"]["application/json"]["schema"],
        &input,
    );
    let (status, created) = call(
        &app,
        "POST",
        "/api/v1/subjects/acme/entitlements/house_cpu/grants",
        Some(input),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    check(&response_schema(GRANTS, "post", "201"), &created);
    assert_eq!(created["amount"], 100.0);

    // listEntitlementGrants, by feature key and by entitlement id.
    let (status, list) = call(
        &app,
        "GET",
        "/api/v1/subjects/acme/entitlements/house_cpu/grants",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    check(&response_schema(GRANTS, "get", "200"), &list);
    assert_eq!(list.as_array().map(Vec::len), Some(1));
    let ent_id = created["entitlementId"].as_str().expect("id").to_string();
    let (status, by_id) = call(
        &app,
        "GET",
        &format!("/api/v1/subjects/acme/entitlements/{ent_id}/grants"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(by_id, list);

    // getEntitlementValue: metered and static.
    let (status, v) = call(
        &app,
        "GET",
        "/api/v1/subjects/acme/entitlements/house_cpu/value",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    check(&response_schema(VALUE, "get", "200"), &v);
    assert_eq!(v["hasAccess"], true);
    assert_eq!(v["usage"], 12.5);
    let (status, v) = call(
        &app,
        "GET",
        "/api/v1/subjects/acme/entitlements/pg_branches/value",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    check(&response_schema(VALUE, "get", "200"), &v);
    assert_eq!(v["config"], "{\"max\":10}");

    // Errors are OpenMeter's problem shape with the status's schema.
    let (status, p) = call(
        &app,
        "GET",
        "/api/v1/subjects/acme/entitlements/nope/value",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    check(&response_schema(VALUE, "get", "404"), &p);
    let (status, p) = call(
        &app,
        "POST",
        "/api/v1/subjects/acme/entitlements/house_cpu/grants",
        Some(json!({ "amount": 1, "effectiveAt": "2023-01-01T00:00:00Z", "expiration": { "duration": "FORTNIGHT", "count": 1 } })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    check(&response_schema(GRANTS, "post", "400"), &p);
}

#[tokio::test]
async fn without_a_caller_the_routes_answer_401() {
    // The router without the auth layer's extension.
    let fga = BuiltinAuthorizer::new();
    let authz = MeterAuthz::new(Arc::new(fga), "meter-export");
    let store = Arc::new(MemStore::new());
    let audit = Arc::new(MemAudit::new());
    let clock: Clock = Arc::new(|| UtcNanos(0));
    let app = loams_meter::openmeter::router(Entitlements::new(
        store,
        authz,
        audit,
        Arc::new(Fixed),
        clock,
    ));
    let (status, p) = call(
        &app,
        "GET",
        "/api/v1/subjects/acme/entitlements/house_cpu/value",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    check(&response_schema(VALUE, "get", "401"), &p);
}
