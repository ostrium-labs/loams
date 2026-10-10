//! `KubeSecretStore` (PG2 Task 6, feature `kubernetes`) against a stand-in
//! of the Kubernetes API's Secret routes: what it sends, and how it reads
//! the answers.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use base64::Engine;
use loams_pg_control::ids::ProjectId;
use loams_pg_control::secrets::{KubeSecretStore, Secret, SecretError, SecretRef, SecretStore};
use serde_json::{Value, json};

#[derive(Default)]
struct Api {
    objects: HashMap<(String, String), Value>,
    /// `METHOD query content-type` of every call.
    calls: Vec<String>,
    /// Answer 500 to everything.
    broken: bool,
}

type Shared = Arc<Mutex<Api>>;

fn status(code: u16, reason: &str, message: &str) -> Response {
    let body = json!({
        "kind": "Status", "apiVersion": "v1", "metadata": {},
        "status": if code < 300 { "Success" } else { "Failure" },
        "message": message, "reason": reason, "code": code,
    });
    (
        StatusCode::from_u16(code).expect("a status"),
        [("content-type", "application/json")],
        body.to_string(),
    )
        .into_response()
}

fn record(api: &mut Api, method: &str, query: Option<&str>, headers: &HeaderMap) -> bool {
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    api.calls
        .push(format!("{method} {} {ct}", query.unwrap_or("")));
    api.broken
}

async fn patch(
    State(s): State<Shared>,
    Path((ns, name)): Path<(String, String)>,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut api = s.lock().expect("lock");
    if record(&mut api, "PATCH", q.as_deref(), &headers) {
        return status(500, "InternalError", "etcd is down");
    }
    let mut object: Value = serde_json::from_slice(&body).expect("a JSON object");
    object["metadata"]["resourceVersion"] = json!("1");
    api.objects.insert((ns, name), object.clone());
    (StatusCode::OK, axum::Json(object)).into_response()
}

async fn read(
    State(s): State<Shared>,
    Path((ns, name)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let mut api = s.lock().expect("lock");
    if record(&mut api, "GET", None, &headers) {
        return status(500, "InternalError", "etcd is down");
    }
    match api.objects.get(&(ns, name.clone())) {
        Some(o) => (StatusCode::OK, axum::Json(o.clone())).into_response(),
        None => status(404, "NotFound", &format!("secrets \"{name}\" not found")),
    }
}

async fn delete(
    State(s): State<Shared>,
    Path((ns, name)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let mut api = s.lock().expect("lock");
    if record(&mut api, "DELETE", None, &headers) {
        return status(500, "InternalError", "etcd is down");
    }
    match api.objects.remove(&(ns, name.clone())) {
        Some(_) => status(200, "", ""),
        None => status(404, "NotFound", &format!("secrets \"{name}\" not found")),
    }
}

/// The namespace's Secrets' metadata, two per page whatever the `limit`,
/// so `list` has to follow `continue`.
async fn list(
    State(s): State<Shared>,
    Path(ns): Path<String>,
    RawQuery(q): RawQuery,
    headers: HeaderMap,
) -> Response {
    let mut api = s.lock().expect("lock");
    if record(&mut api, "LIST", q.as_deref(), &headers) {
        return status(500, "InternalError", "etcd is down");
    }
    let mut names: Vec<String> = api
        .objects
        .keys()
        .filter(|(n, _)| *n == ns)
        .map(|(_, name)| name.clone())
        .collect();
    names.sort();
    let from: usize = q
        .as_deref()
        .unwrap_or("")
        .split('&')
        .find_map(|kv| kv.strip_prefix("continue="))
        .map_or(0, |c| c.parse().expect("a continue token"));
    let page: Vec<Value> = names
        .iter()
        .skip(from)
        .take(2)
        .map(|n| {
            json!({
                "apiVersion": "meta.k8s.io/v1", "kind": "PartialObjectMetadata",
                "metadata": {"name": n, "namespace": ns},
            })
        })
        .collect();
    let next = if from + 2 < names.len() {
        (from + 2).to_string()
    } else {
        String::new()
    };
    let body = json!({
        "apiVersion": "meta.k8s.io/v1", "kind": "PartialObjectMetadataList",
        "metadata": {"continue": next, "resourceVersion": "1"},
        "items": page,
    });
    (StatusCode::OK, axum::Json(body)).into_response()
}

/// A stand-in API server, and a store in namespace `loams-pg-acme` on it.
async fn stand_in() -> (Shared, KubeSecretStore) {
    let shared: Shared = Arc::default();
    let app = Router::new()
        .route(
            "/api/v1/namespaces/{ns}/secrets/{name}",
            get(read).patch(patch).delete(delete),
        )
        .route("/api/v1/namespaces/{ns}/secrets", get(list))
        .with_state(shared.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { axum::serve(listener, app).await });
    let config = kube::Config::new(format!("http://{addr}").parse().expect("a URL"));
    let client = kube::Client::try_from(config).expect("a client");
    (shared, KubeSecretStore::new(client, "loams-pg-acme"))
}

fn new_ref() -> SecretRef {
    SecretRef::new_role(&ProjectId::new())
}

#[tokio::test]
async fn kube_store_applies_reads_and_deletes_secrets() {
    let (api, store) = stand_in().await;
    let r = new_ref();
    store
        .put(&r, Secret::new(b"hunter2-hunter2".to_vec()))
        .await
        .expect("put");
    {
        let api = api.lock().expect("lock");
        let object = &api.objects[&("loams-pg-acme".to_string(), r.to_string())];
        assert_eq!(object["kind"], "Secret");
        assert_eq!(object["type"], "Opaque");
        assert_eq!(object["metadata"]["name"], r.as_str());
        assert_eq!(
            object["metadata"]["labels"]["app.kubernetes.io/managed-by"],
            "loams-pg-control"
        );
        let data = object["data"]["secret"].as_str().expect("base64 data");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(data)
                .expect("base64"),
            b"hunter2-hunter2"
        );
        let call = &api.calls[0];
        assert!(call.starts_with("PATCH "), "{call}");
        assert!(call.contains("fieldManager=loams-pg-control"), "{call}");
        assert!(call.contains("force=true"), "{call}");
        assert!(call.ends_with("application/apply-patch+yaml"), "{call}");
    }
    assert_eq!(
        store.get(&r).await.expect("get").expose(),
        b"hunter2-hunter2"
    );
    // A second put replaces it.
    store
        .put(&r, Secret::new(b"rotated".to_vec()))
        .await
        .expect("put again");
    assert_eq!(store.get(&r).await.expect("get").expose(), b"rotated");

    store.delete(&r).await.expect("delete");
    assert!(matches!(store.get(&r).await, Err(SecretError::NotFound(_))));
    store.delete(&r).await.expect("an absent secret is deleted");
}

#[tokio::test]
async fn kube_store_failures_are_unavailable_and_carry_no_secret() {
    let (api, store) = stand_in().await;
    api.lock().expect("lock").broken = true;
    let r = new_ref();
    let e = store
        .put(&r, Secret::new(b"do-not-leak-me".to_vec()))
        .await
        .expect_err("500");
    assert!(matches!(e, SecretError::Unavailable(_)), "{e}");
    assert!(!format!("{e} {e:?}").contains("do-not-leak-me"));
    assert!(matches!(
        store.get(&r).await,
        Err(SecretError::Unavailable(_))
    ));
    assert!(matches!(
        store.delete(&r).await,
        Err(SecretError::Unavailable(_))
    ));
    assert!(matches!(
        store.list().await,
        Err(SecretError::Unavailable(_))
    ));
    assert!(!format!("{store:?}").is_empty());
}

#[tokio::test]
async fn kube_store_lists_role_secrets_by_label_page_by_page() {
    let (api, store) = stand_in().await;
    let mut refs: Vec<SecretRef> = (0..5).map(|_| new_ref()).collect();
    for r in &refs {
        store.put(r, Secret::new(b"x".to_vec())).await.expect("put");
    }
    let mut listed = store.list().await.expect("list");
    listed.sort();
    refs.sort();
    assert_eq!(listed, refs);
    let api = api.lock().expect("lock");
    let lists: Vec<&String> = api
        .calls
        .iter()
        .filter(|c| c.starts_with("LIST "))
        .collect();
    assert_eq!(lists.len(), 3, "five secrets, two a page: {lists:?}");
    for call in &lists {
        assert!(
            call.contains("labelSelector=app.kubernetes.io%2Fmanaged-by%3Dloams-pg-control%2Capp.kubernetes.io%2Fcomponent%3Dpostgres-role-secret"),
            "{call}"
        );
    }
    assert!(lists[1].contains("continue=2"), "{}", lists[1]);
}
