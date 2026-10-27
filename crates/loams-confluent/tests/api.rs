//! The clusters, metrics and costs calls against a local fake of the API
//! shapes they use (paging, request bodies). Ids are placeholders.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use loams_confluent::costs::{self, Day};
use loams_confluent::metrics::{self, Metric};
use loams_confluent::{Client, CloudKey, Secret, clusters};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Default)]
struct Fake {
    base: String,
    calls: Vec<String>,
    bodies: Vec<Value>,
}

type Shared = Arc<Mutex<Fake>>;

fn cluster(id: &str, kind: &str) -> Value {
    json!({"id": id, "spec": {"display_name": id, "cloud": "AWS", "region": "us-east-1",
           "config": {"kind": kind}, "kafka_bootstrap_endpoint": format!("SASL_SSL://{id}.example:9092")}})
}

async fn list_clusters(
    State(f): State<Shared>,
    Query(q): Query<BTreeMap<String, String>>,
) -> (StatusCode, String) {
    let mut f = f.lock().unwrap();
    f.calls.push(format!(
        "GET clusters env={} token={}",
        q.get("environment").cloned().unwrap_or_default(),
        q.get("page_token").cloned().unwrap_or_default()
    ));
    if q.contains_key("page_token") {
        return (
            StatusCode::OK,
            json!({"data": [cluster("lkc-two02", "Standard")], "metadata": {}}).to_string(),
        );
    }
    let next = format!(
        "{}/cmk/v2/clusters?environment=env-test01&page_size=100&page_token=p2",
        f.base
    );
    (
        StatusCode::OK,
        json!({"data": [cluster("lkc-one01", "Basic")], "metadata": {"next": next}}).to_string(),
    )
}

async fn get_cluster(State(f): State<Shared>, Path(id): Path<String>) -> (StatusCode, String) {
    f.lock().unwrap().calls.push(format!("GET cluster {id}"));
    (StatusCode::OK, cluster(&id, "Basic").to_string())
}

async fn patch_cluster(
    State(f): State<Shared>,
    Path(id): Path<String>,
    axum::Json(body): axum::Json<Value>,
) -> StatusCode {
    let mut f = f.lock().unwrap();
    f.calls.push(format!("PATCH cluster {id}"));
    f.bodies.push(body);
    StatusCode::ACCEPTED
}

async fn costs_route(
    State(f): State<Shared>,
    Query(q): Query<BTreeMap<String, String>>,
) -> (StatusCode, String) {
    let mut f = f.lock().unwrap();
    let start = q.get("start_date").cloned().unwrap_or_default();
    let end = q.get("end_date").cloned().unwrap_or_default();
    f.calls.push(format!(
        "GET costs {start} {end} {}",
        q.get("page_token").cloned().unwrap_or_default()
    ));
    let line = |amount: f64| {
        json!({"start_date": start, "end_date": end, "product": "KAFKA", "line_type": "KAFKA_NETWORK_READ",
               "resource": {"id": "lkc-one01", "display_name": "one", "environment": {"id": "env-test01"}},
               "quantity": 1.5, "unit": "GB", "price": 0.05, "original_amount": amount,
               "discount_amount": 0, "amount": amount})
    };
    if q.contains_key("page_token") {
        return (
            StatusCode::OK,
            json!({"data": [line(2.0), {"start_date": start, "end_date": end, "line_type": "PROMO_CREDIT",
                    "original_amount": 0, "amount": -3.0}], "metadata": {}})
            .to_string(),
        );
    }
    let next = format!(
        "{}/billing/v1/costs?start_date={start}&end_date={end}&page_size=10000&page_token=p2",
        f.base
    );
    (
        StatusCode::OK,
        json!({"data": [line(1.0)], "metadata": {"next": next}}).to_string(),
    )
}

async fn metrics_route(
    State(f): State<Shared>,
    Query(q): Query<BTreeMap<String, String>>,
    axum::Json(body): axum::Json<Value>,
) -> (StatusCode, String) {
    let mut f = f.lock().unwrap();
    let token = q.get("page_token").cloned().unwrap_or_default();
    f.calls.push(format!("POST metrics token={token}"));
    f.bodies.push(body);
    if token.is_empty() {
        return (
            StatusCode::OK,
            json!({"data": [{"timestamp": "2027-01-15T08:00:00Z", "value": 10.0, "metric.principal_id": "sa-a"}],
                   "meta": {"pagination": {"page_size": 1, "next_page_token": "t/2=="}}})
            .to_string(),
        );
    }
    (
        StatusCode::OK,
        json!({"data": [{"timestamp": "2027-01-15T08:00:00Z", "value": 5.0, "metric.principal_id": "sa-b"},
                         {"timestamp": "2027-01-15T09:00:00Z", "value": 1.0, "metric.principal_id": "sa-a"}],
               "meta": {"pagination": {"page_size": 2}}})
        .to_string(),
    )
}

async fn serve(f: Shared) -> String {
    let app = Router::new()
        .route("/cmk/v2/clusters", get(list_clusters))
        .route(
            "/cmk/v2/clusters/{id}",
            get(get_cluster).patch(patch_cluster),
        )
        .route("/billing/v1/costs", get(costs_route))
        .route("/v2/metrics/cloud/query", post(metrics_route))
        .with_state(f.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    f.lock().unwrap().base = format!("http://{addr}");
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    format!("http://{addr}")
}

fn client(base: &str) -> Client {
    loams_confluent::cli::install_crypto();
    Client::new(
        base,
        CloudKey::new("CLOUDKEY01", Secret::new("cloud-secret-PLANTED")),
        reqwest::Client::new(),
    )
    .with_backoff(Duration::from_millis(5))
}

#[tokio::test]
async fn clusters_list_pages_and_strips_sasl_ssl() {
    let f = Shared::default();
    let c = client(&serve(f.clone()).await);
    let all = clusters::list(&c, "env-test01").await.unwrap();
    let ids: Vec<&str> = all.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["lkc-one01", "lkc-two02"]);
    assert_eq!(all[0].bootstrap, "lkc-one01.example:9092");
    assert_eq!(all[1].kind, "Standard");
    assert_eq!(f.lock().unwrap().calls.len(), 2);
}

#[tokio::test]
async fn set_max_ecku_reads_the_kind_then_patches() {
    let f = Shared::default();
    let c = client(&serve(f.clone()).await);
    clusters::set_max_ecku(&c, "env-test01", "lkc-one01", 2)
        .await
        .unwrap();
    let f = f.lock().unwrap();
    assert_eq!(
        f.calls,
        ["GET cluster lkc-one01", "PATCH cluster lkc-one01"]
    );
    assert_eq!(
        f.bodies[0],
        json!({"spec": {"environment": {"id": "env-test01"}, "config": {"kind": "Basic", "max_ecku": 2}}})
    );
    drop(f);
    assert!(
        clusters::set_max_ecku(&c, "env-test01", "lkc-one01", 0)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn costs_split_windows_and_page() {
    let f = Shared::default();
    let c = client(&serve(f.clone()).await);
    let start: Day = "2026-09-01".parse().unwrap();
    let end: Day = "2026-10-15".parse().unwrap();
    let lines = costs::list(&c, start, end).await.unwrap();
    let calls = f.lock().unwrap().calls.clone();
    // two windows (30 + 14 days), two pages each
    assert_eq!(
        calls,
        [
            "GET costs 2026-09-01 2026-10-01 ",
            "GET costs 2026-09-01 2026-10-01 p2",
            "GET costs 2026-10-01 2026-10-15 ",
            "GET costs 2026-10-01 2026-10-15 p2",
        ]
    );
    assert_eq!(lines.len(), 6);
    let by = costs::total_by_resource(&lines);
    assert!((by["lkc-one01"] - 6.0).abs() < 1e-9);
    assert!((by[""] + 6.0).abs() < 1e-9, "credits under the empty id");
    assert!(costs::list(&c, end, start).await.is_err());
}

#[tokio::test]
async fn metrics_query_pages_with_the_token_and_groups() {
    let f = Shared::default();
    let c = client(&serve(f.clone()).await);
    let q = metrics::Query::hourly(
        Metric::RequestBytes,
        "lkc-one01",
        1_800_000_000,
        1_800_007_200,
    )
    .group_by("metric.principal_id");
    let points = metrics::query(&c, &q).await.unwrap();
    assert_eq!(points.len(), 3);
    let sums = metrics::sum_by(&points, "metric.principal_id");
    assert!((sums["sa-a"] - 11.0).abs() < 1e-9);
    assert!((sums["sa-b"] - 5.0).abs() < 1e-9);
    let f = f.lock().unwrap();
    assert_eq!(f.calls, ["POST metrics token=", "POST metrics token=t/2=="]);
    assert_eq!(f.bodies[0], f.bodies[1], "the same body on every page");
    assert_eq!(f.bodies[0]["group_by"][0], "metric.principal_id");
}
