//! `loams-postgres` against recorded fixtures (PG2 Task 2): the request bodies
//! are the ones the pinned pageserver and `loams-wal` accepted, byte for byte
//! after canonical JSON, and the responses are theirs
//! (`tests/fixtures/README.md`, `capture.sh`).
#![allow(clippy::unwrap_used)]

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use loams_postgres::compute_ctl::{
    ComputeCtlClient, ComputeCtlConfig, ComputeStatus, TerminateMode,
};
use loams_postgres::pageserver::{NeonClient, NeonEndpoints, TenantConfig, TimelineCreate};
use loams_postgres::spec::{ComputeMode, ComputeSpecBuilder, DatabaseRec, RoleRec, Setting};
use loams_postgres::wal::{WalClient, WalTimelineCreate};
use loams_postgres::{Component, Lsn, Op, Secret, TenantId, TimelineId};
use serde_json::Value;

const T: &str = "4c6f616d734e656f6e54656e616e7431";
const TL: &str = "4c6f616d734e656f6e54696d656c6e31";
const BR: &str = "4c6f616d734e656f6e4272616e636831";

fn fixture(name: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// JSON with every object's keys sorted, compact: two documents are the
/// same request when their canonical forms are equal byte for byte.
fn canonical(text: &str) -> String {
    fn sort(v: Value) -> Value {
        match v {
            Value::Object(m) => {
                let mut entries: Vec<(String, Value)> = m.into_iter().collect();
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                Value::Object(entries.into_iter().map(|(k, v)| (k, sort(v))).collect())
            }
            Value::Array(a) => Value::Array(a.into_iter().map(sort).collect()),
            other => other,
        }
    }
    serde_json::to_string(&sort(serde_json::from_str(text).unwrap())).unwrap()
}

/// One request the stand-in server saw.
#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path_and_query: String,
    auth: Option<String>,
    body: String,
}

/// A stand-in HTTP server: records each request and answers the next queued
/// response.
#[derive(Clone, Default)]
struct Fake {
    seen: Arc<Mutex<Vec<Seen>>>,
    answers: Arc<Mutex<VecDeque<(u16, String)>>>,
}

impl Fake {
    fn answer(&self, status: u16, body: &str) -> &Self {
        self.answers
            .lock()
            .unwrap()
            .push_back((status, body.to_string()));
        self
    }

    fn last(&self) -> Seen {
        self.seen
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("a request")
    }
}

async fn handle(State(fake): State<Fake>, headers: HeaderMap, req: Request) -> Response {
    let method = req.method().to_string();
    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|p| p.to_string())
        .unwrap_or_default();
    let body: Bytes = axum::body::to_bytes(req.into_body(), 1 << 20)
        .await
        .unwrap();
    fake.seen.lock().unwrap().push(Seen {
        method,
        path_and_query,
        auth: headers
            .get("authorization")
            .map(|v| v.to_str().unwrap().to_string()),
        body: String::from_utf8(body.to_vec()).unwrap(),
    });
    let (status, body) = fake
        .answers
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or((200, "null".into()));
    (
        StatusCode::from_u16(status).unwrap(),
        [("content-type", "application/json")],
        body,
    )
        .into_response()
}

async fn serve() -> (Fake, String) {
    let fake = Fake::default();
    let app = axum::Router::new()
        .fallback(handle)
        .with_state(fake.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await });
    (fake, format!("http://{addr}"))
}

fn pageserver(url: &str) -> NeonClient {
    NeonClient::new(
        NeonEndpoints {
            pageserver: url.parse().unwrap(),
            storcon: None,
        },
        None,
    )
    .unwrap()
}

fn t() -> TenantId {
    T.parse().unwrap()
}
fn tl() -> TimelineId {
    TL.parse().unwrap()
}
fn br() -> TimelineId {
    BR.parse().unwrap()
}

#[tokio::test]
async fn create_timeline_body_matches_fixture() {
    let (fake, url) = serve().await;
    fake.answer(201, &fixture("create_timeline.response.json"));
    let info = pageserver(&url)
        .create_timeline(t(), &TimelineCreate::bootstrap(tl(), 17))
        .await
        .unwrap();
    let seen = fake.last();
    assert_eq!(seen.method, "POST");
    assert_eq!(seen.path_and_query, format!("/v1/tenant/{T}/timeline"));
    assert_eq!(
        canonical(&seen.body),
        canonical(&fixture("create_timeline.request.json"))
    );
    assert_eq!(info.timeline_id, tl());
}

#[tokio::test]
async fn branch_body_has_ancestor_and_lsn() {
    let (fake, url) = serve().await;
    fake.answer(201, &fixture("branch.response.json"));
    let lsn: Lsn = "0/14E8F98".parse().unwrap();
    let info = pageserver(&url)
        .create_timeline(t(), &TimelineCreate::branch(br(), tl(), Some(lsn)))
        .await
        .unwrap();
    assert_eq!(
        canonical(&fake.last().body),
        canonical(&fixture("branch.request.json"))
    );
    assert_eq!(info.ancestor_timeline_id, Some(tl()));
    assert_eq!(info.ancestor_lsn, Some(lsn));
}

#[tokio::test]
async fn attach_body_matches_fixture() {
    let (fake, url) = serve().await;
    fake.answer(200, &fixture("attach.response.json"));
    pageserver(&url)
        .attach_tenant(t(), 1, &TenantConfig::default())
        .await
        .unwrap();
    let seen = fake.last();
    assert_eq!(seen.method, "PUT");
    assert_eq!(
        seen.path_and_query,
        format!("/v1/tenant/{T}/location_config")
    );
    assert_eq!(
        canonical(&seen.body),
        canonical(&fixture("attach.request.json"))
    );
}

#[tokio::test]
async fn tenant_config_body_matches_fixture() {
    let (fake, url) = serve().await;
    fake.answer(200, &fixture("tenant_config.response.json"));
    let conf = TenantConfig {
        pitr_interval: Some(Duration::from_secs(7 * 24 * 3600)),
        ..TenantConfig::default()
    };
    pageserver(&url).tenant_config(t(), &conf).await.unwrap();
    let seen = fake.last();
    // A patch: the settings sent are set, the others kept.
    assert_eq!(seen.method, "PATCH");
    assert_eq!(seen.path_and_query, "/v1/tenant/config");
    assert_eq!(
        canonical(&seen.body),
        canonical(&fixture("tenant_config.request.json"))
    );
}

/// The pinned pageserver's own answers parse, with the fields Loams reads.
#[tokio::test]
async fn timeline_info_parses_fixture() {
    let (fake, url) = serve().await;
    let client = pageserver(&url);
    fake.answer(200, &fixture("timeline_get.response.json"));
    let info = client.timeline(t(), tl()).await.unwrap();
    assert_eq!(
        fake.last().path_and_query,
        format!("/v1/tenant/{T}/timeline/{TL}")
    );
    assert_eq!(info.tenant_id, t());
    assert_eq!(info.pg_version, 17);
    assert_eq!(info.last_record_lsn, "0/14E8F98".parse().unwrap());
    assert_eq!(info.initdb_lsn, "0/14E8F20".parse().unwrap());
    assert!(info.current_logical_size > 0);
    assert_eq!(info.state, "Active");
    assert_eq!(info.ancestor_timeline_id, None);

    fake.answer(200, &fixture("timeline_list.response.json"));
    let all = client.list_timelines(t()).await.unwrap();
    assert_eq!(
        fake.last().path_and_query,
        format!("/v1/tenant/{T}/timeline")
    );
    let mut ids: Vec<TimelineId> = all.iter().map(|i| i.timeline_id).collect();
    ids.sort();
    let mut want = vec![tl(), br()];
    want.sort();
    assert_eq!(ids, want);

    fake.answer(200, &fixture("lsn_by_timestamp.response.json"));
    let at = time::OffsetDateTime::from_unix_timestamp(1_791_513_706).unwrap();
    let found = client.lsn_by_timestamp(t(), tl(), at, false).await.unwrap();
    assert_eq!(
        fake.last().path_and_query,
        format!(
            "/v1/tenant/{T}/timeline/{TL}/get_lsn_by_timestamp?timestamp=2026-10-09T02%3A41%3A46Z"
        )
    );
    assert_eq!(found.lsn, "0/14E8F20".parse().unwrap());
    assert_eq!(found.kind, "nodata");
    assert_eq!(found.valid_until, None);

    // With a lease. The body is a stand-in from the fork's handler
    // (`get_lsn_by_timestamp_handler`: `LsnLease` flattened, `valid_until`
    // in RFC 3339 ms); `deploy/loams-postgres-dev` has not been captured
    // with a lease yet.
    fake.answer(
        200,
        r#"{"lsn":"0/169AD58","kind":"present","valid_until":"2026-10-09T03:41:46.000Z"}"#,
    );
    let leased = client.lsn_by_timestamp(t(), tl(), at, true).await.unwrap();
    assert_eq!(
        fake.last().path_and_query,
        format!(
            "/v1/tenant/{T}/timeline/{TL}/get_lsn_by_timestamp?timestamp=2026-10-09T02%3A41%3A46Z&with_lease=true"
        )
    );
    assert_eq!(leased.kind, "present");
    assert_eq!(
        leased.valid_until.as_deref(),
        Some("2026-10-09T03:41:46.000Z")
    );

    fake.answer(202, &fixture("delete_branch.response.json"));
    client.delete_timeline(t(), br()).await.unwrap();
    let seen = fake.last();
    assert_eq!(seen.method, "DELETE");
    assert_eq!(seen.path_and_query, format!("/v1/tenant/{T}/timeline/{BR}"));
}

/// Through the storage controller: tenants are created with `POST
/// /v1/tenant` (it chooses the generation), everything else goes to the
/// same routes on the controller. Its create answers `TimelineInfo` with
/// `safekeepers`; its delete waits and answers 200, or 409 when the deletion
/// is still going after 25 s (`storage_controller/src/http.rs`).
#[tokio::test]
async fn storcon_routes() {
    let (ps, ps_url) = serve().await;
    let (sc, sc_url) = serve().await;
    let client = NeonClient::new(
        NeonEndpoints {
            pageserver: ps_url.parse().unwrap(),
            storcon: Some(sc_url.parse().unwrap()),
        },
        None,
    )
    .unwrap();
    sc.answer(
        201,
        r#"{"shards":[{"shard_id":"4c6f616d734e656f6e54656e616e7431","node_id":1,"generation":1}]}"#,
    );
    let conf = TenantConfig {
        pitr_interval: Some(Duration::from_secs(7 * 24 * 3600)),
        ..TenantConfig::default()
    };
    client.attach_tenant(t(), 1, &conf).await.unwrap();
    let seen = sc.last();
    assert_eq!(
        (seen.method.as_str(), seen.path_and_query.as_str()),
        ("POST", "/v1/tenant")
    );
    // No generation: the controller owns them.
    assert_eq!(
        canonical(&seen.body),
        canonical(&format!(
            r#"{{"new_tenant_id":"{T}","pitr_interval":"7days"}}"#
        ))
    );

    sc.answer(201, &fixture("src_storcon_create_timeline.response.json"));
    let info = client
        .create_timeline(t(), &TimelineCreate::bootstrap(tl(), 17))
        .await
        .unwrap();
    assert_eq!(info.timeline_id, tl());
    assert_eq!(sc.last().path_and_query, format!("/v1/tenant/{T}/timeline"));

    sc.answer(200, &fixture("timeline_get.response.json"));
    client.timeline(t(), tl()).await.unwrap();

    sc.answer(200, "null");
    client.delete_timeline(t(), br()).await.unwrap();
    assert_eq!(sc.last().method, "DELETE");
    sc.answer(409, &fixture("src_storcon_delete_timeout.response.json"));
    let e = client.delete_timeline(t(), br()).await.unwrap_err();
    assert_eq!((e.status, e.reason()), (409, "aborted"));
    assert_eq!(e.component, Component::StorageController);

    assert!(
        ps.seen.lock().unwrap().is_empty(),
        "the pageserver is not called directly"
    );
}

/// Errors carry the status and the component's `msg`, and map to
/// `loams.errors.v1` reasons by status and call. The bodies are the pinned
/// pageserver's (`*.response.json`) or, where the capture cannot provoke
/// them, the fork's text (`src_*.response.json`, README.md).
#[tokio::test]
async fn neon_error_maps_to_reason() {
    let (fake, url) = serve().await;
    let client = pageserver(&url);
    let create = TimelineCreate::bootstrap(br(), 17);
    let branch = TimelineCreate::branch(br(), tl(), Some("0/14E8F90".parse().unwrap()));

    // 409 on a create: the id is taken with other parameters.
    fake.answer(409, &fixture("conflict.response.json"));
    let e = client.create_timeline(t(), &create).await.unwrap_err();
    assert_eq!(e.msg, "timeline already exists with different parameters");
    assert_eq!(
        (e.status, e.op, e.reason()),
        (409, Op::CreateTimeline, "already_exists")
    );

    // 406 on a branch: below the ancestor's own start, or its GC cutoff.
    fake.answer(406, &fixture("branch_below_ancestor.response.json"));
    let e = client.create_timeline(t(), &branch).await.unwrap_err();
    assert_eq!(e.reason(), "failed_precondition", "{e}");
    fake.answer(406, &fixture("src_branch_gc_cutoff.response.json"));
    let e = client.create_timeline(t(), &branch).await.unwrap_err();
    assert_eq!(e.reason(), "lsn_out_of_retention");
    let info = e.error_info();
    assert_eq!(
        info.metadata.get("oldest_lsn").map(String::as_str),
        Some("0/14E8F98")
    );

    // 429: the same create is already running.
    fake.answer(429, &fixture("src_create_in_progress.response.json"));
    let e = client.create_timeline(t(), &create).await.unwrap_err();
    assert_eq!(e.reason(), "aborted");

    // 404 on a read.
    fake.answer(404, &fixture("not_found.response.json"));
    let e = client.timeline(t(), tl()).await.unwrap_err();
    assert_eq!((e.status, e.reason()), (404, "not_found"));
    assert!(e.msg.starts_with("NotFound: Timeline"));
    assert_eq!(
        e.error_info().metadata.get("kind").map(String::as_str),
        Some("timeline")
    );

    // 412 on a delete: children, or a missing tenant.
    fake.answer(412, &fixture("delete_with_children.response.json"));
    let e = client.delete_timeline(t(), tl()).await.unwrap_err();
    assert_eq!(e.reason(), "branch_has_children");
    assert_eq!(
        e.error_info().metadata.get("children").map(String::as_str),
        Some("1")
    );
    fake.answer(412, &fixture("delete_tenant_missing.response.json"));
    let e = client.delete_timeline(t(), tl()).await.unwrap_err();
    assert_eq!(e.reason(), "not_found");
    assert_eq!(
        e.error_info().metadata.get("kind").map(String::as_str),
        Some("tenant")
    );

    // 408 is the component's own timeout.
    fake.answer(408, r#"{"msg":"Timeout"}"#);
    assert_eq!(
        client.timeline(t(), tl()).await.unwrap_err().reason(),
        "unavailable"
    );

    for status in [500, 502, 503] {
        fake.answer(
            status,
            r#"{"msg":"Resource temporarily unavailable: down"}"#,
        );
        let e = client.timeline(t(), tl()).await.unwrap_err();
        assert_eq!(e.reason(), "storage_unavailable", "{status}");
        let info = e.error_info();
        assert_eq!(
            info.metadata.get("component").map(String::as_str),
            Some("pageserver")
        );
        // Only registered metadata: no `status`.
        assert_eq!(info.metadata.len(), 1, "{:?}", info.metadata);
    }
    // A component that cannot be reached is unavailable too.
    let gone = pageserver("http://127.0.0.1:1");
    let e = gone.timeline(t(), tl()).await.unwrap_err();
    assert_eq!((e.status, e.reason()), (0, "storage_unavailable"));

    // The component refusing Loams's own token is a misconfiguration, not
    // the caller's: never `unauthenticated`, which tells a client to sign in
    // again.
    for status in [401, 403] {
        fake.answer(status, r#"{"msg":"Unauthorized: malformed jwt token"}"#);
        let e = client.timeline(t(), tl()).await.unwrap_err();
        assert_eq!(e.reason(), "internal", "{status}");
    }
    // Any other refusal is a request Loams should not have sent.
    fake.answer(405, "");
    let e = client.timeline(t(), tl()).await.unwrap_err();
    assert_eq!(e.reason(), "internal");
}

/// Each client names its component, and only the storage components'
/// outages are `storage_unavailable`; a compute that cannot be reached is
/// plain `unavailable`. compute_ctl's errors are `{"error": ...}`.
#[tokio::test]
async fn errors_name_their_component() {
    let wal = WalClient::new("http://127.0.0.1:1".parse().unwrap(), None).unwrap();
    let e = wal.timeline_status(t(), tl()).await.unwrap_err();
    assert_eq!(
        (e.component, e.reason()),
        (Component::Wal, "storage_unavailable")
    );
    assert_eq!(
        e.error_info().metadata.get("component").map(String::as_str),
        Some("loams_wal")
    );

    let (fake, url) = serve().await;
    let ctl = ComputeCtlClient::new(url.parse().unwrap(), Secret::new("jwt".into())).unwrap();
    fake.answer(401, &fixture("compute_unauthorized.response.json"));
    let e = ctl.status().await.unwrap_err();
    assert_eq!(e.component, Component::ComputeCtl);
    assert_eq!(e.msg, "failed to verify authorization token");
    assert_eq!(e.reason(), "internal");
    let gone = ComputeCtlClient::new(
        "http://127.0.0.1:1".parse().unwrap(),
        Secret::new("jwt".into()),
    )
    .unwrap();
    assert_eq!(gone.status().await.unwrap_err().reason(), "unavailable");
}

/// A base URL keeps its path prefix, and one with credentials, a query or
/// another scheme is refused before any call.
#[tokio::test]
async fn base_urls() {
    let (fake, url) = serve().await;
    let client = pageserver(&format!("{url}/gateway/ps/"));
    fake.answer(200, &fixture("timeline_get.response.json"));
    client.timeline(t(), tl()).await.unwrap();
    assert_eq!(
        fake.last().path_and_query,
        format!("/gateway/ps/v1/tenant/{T}/timeline/{TL}")
    );
    for bad in [
        "http://user:hunter2@127.0.0.1:9898",
        "http://user@127.0.0.1:9898",
        "http://127.0.0.1:9898/?token=x",
        "file:///etc/passwd",
    ] {
        let e = NeonClient::new(
            NeonEndpoints {
                pageserver: bad.parse().unwrap(),
                storcon: None,
            },
            None,
        )
        .unwrap_err();
        assert_eq!((e.op, e.reason()), (Op::Setup, "internal"), "{bad}");
        assert!(!e.msg.contains("hunter2"), "{e}");
        assert!(WalClient::new(bad.parse().unwrap(), None).is_err(), "{bad}");
    }
}

/// loams-wal's timeline API.
#[tokio::test]
async fn wal_client_matches_fixtures() {
    let (fake, url) = serve().await;
    let wal = WalClient::new(url.parse().unwrap(), Some(Secret::new("wal-token".into()))).unwrap();
    fake.answer(200, &fixture("wal_create_timeline.response.json"));
    let st = wal
        .create_timeline(&WalTimelineCreate {
            tenant_id: t(),
            timeline_id: tl(),
            pg_version: 17,
            start_lsn: "0/14E8F98".parse().unwrap(),
            system_id: None,
            wal_seg_size: None,
            commit_lsn: None,
        })
        .await
        .unwrap();
    let seen = fake.last();
    assert_eq!(
        (seen.method.as_str(), seen.path_and_query.as_str()),
        ("POST", "/v1/tenant/timeline")
    );
    assert_eq!(seen.auth.as_deref(), Some("Bearer wal-token"));
    assert_eq!(
        canonical(&seen.body),
        canonical(&fixture("wal_create_timeline.request.json"))
    );
    assert_eq!(st.commit_lsn, "0/14E8F98".parse().unwrap());
    assert_eq!(st.pg_version, 170_000);

    fake.answer(200, &fixture("wal_timeline_status.response.json"));
    let st = wal.timeline_status(t(), tl()).await.unwrap();
    assert_eq!(
        fake.last().path_and_query,
        format!("/v1/tenant/{T}/timeline/{TL}")
    );
    assert_eq!(st.flush_lsn, "0/14E8F98".parse().unwrap());
    assert_eq!(st.term, 0);

    // A create of an existing timeline answers its head unchanged (200),
    // even with other parameters (`conflict_wal.request.json` asks for 16).
    fake.answer(200, &fixture("wal_create_conflict.response.json"));
    let again = wal
        .create_timeline(&WalTimelineCreate {
            tenant_id: t(),
            timeline_id: tl(),
            pg_version: 16,
            start_lsn: "0/1000000".parse().unwrap(),
            system_id: None,
            wal_seg_size: None,
            commit_lsn: None,
        })
        .await
        .unwrap();
    assert_eq!(
        canonical(&fake.last().body),
        canonical(&fixture("conflict_wal.request.json"))
    );
    assert_eq!(again, st);

    fake.answer(404, &fixture("wal_not_found.response.json"));
    let e = wal.timeline_status(t(), tl()).await.unwrap_err();
    assert_eq!(e.reason(), "not_found");
}

/// compute_ctl: the per-compute JWT on every call, and its request shapes
/// (`compute_api::requests::ConfigurationRequest`, `responses::PromoteConfig`,
/// `/terminate?mode=`).
#[tokio::test]
async fn compute_ctl_calls() {
    let (fake, url) = serve().await;
    let ctl =
        ComputeCtlClient::new(url.parse().unwrap(), Secret::new("compute-jwt".into())).unwrap();
    // compute1's own answer (recorded).
    fake.answer(200, &fixture("compute_status.response.json"));
    let st = ctl.status().await.unwrap();
    assert_eq!(st.status, ComputeStatus::Running);
    assert_eq!(st.tenant.as_deref(), Some(T));
    let seen = fake.last();
    assert_eq!(
        (seen.method.as_str(), seen.path_and_query.as_str()),
        ("GET", "/status")
    );
    assert_eq!(seen.auth.as_deref(), Some("Bearer compute-jwt"));

    let spec = golden_builder().build();
    fake.answer(200, r#"{"start_time":"2026-10-09T02:41:46Z","tenant":null,"timeline":null,"status":"configuration_pending","last_active":null,"error":null}"#);
    ctl.configure(&spec, &ComputeCtlConfig::default())
        .await
        .unwrap();
    let seen = fake.last();
    assert_eq!(
        (seen.method.as_str(), seen.path_and_query.as_str()),
        ("POST", "/configure")
    );
    let body: Value = serde_json::from_str(&seen.body).unwrap();
    assert_eq!(body["spec"], serde_json::to_value(&spec).unwrap());
    assert_eq!(
        body["compute_ctl_config"],
        serde_json::json!({"jwks": {"keys": []}, "tls": null})
    );

    fake.answer(200, r#"{"lsn":"0/14E8F98"}"#);
    let lsn = ctl.terminate(TerminateMode::Fast).await.unwrap();
    assert_eq!(fake.last().path_and_query, "/terminate?mode=fast");
    assert_eq!(lsn, Some("0/14E8F98".parse().unwrap()));

    fake.answer(
        200,
        r#"{"status":"completed","lsn_wait_time_ms":3,"pg_promote_time_ms":40,"reconfigure_time_ms":12}"#,
    );
    ctl.promote(&spec, "0/14E8F98".parse().unwrap())
        .await
        .unwrap();
    let seen = fake.last();
    assert_eq!(
        (seen.method.as_str(), seen.path_and_query.as_str()),
        ("POST", "/promote")
    );
    let body: Value = serde_json::from_str(&seen.body).unwrap();
    assert_eq!(body["wal_flush_lsn"], "0/14E8F98");
    assert_eq!(body["spec"]["mode"], "Primary");

    // compute_ctl refuses to promote a primary (recorded from compute1),
    // and a replica whose cache is not prewarmed (the fork's text): both are
    // `failed_precondition`.
    fake.answer(500, &fixture("promote_primary.response.json"));
    let e = ctl
        .promote(&spec, "0/14E8F98".parse().unwrap())
        .await
        .unwrap_err();
    assert_eq!(e.msg, r#"compute mode "primary" is not replica"#);
    assert_eq!((e.status, e.reason()), (500, "failed_precondition"));
    fake.answer(500, &fixture("src_promote_not_prewarmed.response.json"));
    let e = ctl
        .promote(&spec, "0/14E8F98".parse().unwrap())
        .await
        .unwrap_err();
    assert_eq!(e.msg, "compute NotPrewarmed");
    assert_eq!(e.reason(), "failed_precondition");
    // A 200 that is not `completed` is not a promotion either.
    fake.answer(200, r#"{"status":"not_promoted"}"#);
    let e = ctl
        .promote(&spec, "0/14E8F98".parse().unwrap())
        .await
        .unwrap_err();
    assert!(e.msg.contains("not_promoted"), "{e}");
    assert_eq!(e.reason(), "failed_precondition");

    // Prewarming: start it (202), read its state (recorded from compute1).
    fake.answer(202, "");
    ctl.prewarm(Some("ep-01J9Z8Q4XWQ3T8F2D4G6H8J0KP"))
        .await
        .unwrap();
    let seen = fake.last();
    assert_eq!(
        (seen.method.as_str(), seen.path_and_query.as_str()),
        (
            "POST",
            "/lfc/prewarm?from_endpoint=ep-01J9Z8Q4XWQ3T8F2D4G6H8J0KP"
        )
    );
    fake.answer(200, &fixture("prewarm_state.response.json"));
    let st = ctl.prewarm_state().await.unwrap();
    assert_eq!(st.status, "not_prewarmed");
    fake.answer(
        429,
        r#"{"error":"Multiple requests for prewarm are not allowed"}"#,
    );
    assert_eq!(ctl.prewarm(None).await.unwrap_err().reason(), "aborted");
}

/// A failover target prewarms from endpoint storage: the spec carries its
/// address, token and `autoprewarm` (left out when unset, as in the golden).
#[test]
fn spec_prewarm_fields() {
    let spec = golden_builder()
        .mode(ComputeMode::Replica)
        .endpoint_storage("endpoint-storage:9993", Secret::new("es-token".into()))
        .autoprewarm(true)
        .build();
    let v = serde_json::to_value(&spec).unwrap();
    assert_eq!(v["endpoint_storage_addr"], "endpoint-storage:9993");
    assert_eq!(v["endpoint_storage_token"], "es-token");
    assert_eq!(v["autoprewarm"], true);
    assert_eq!(v["mode"], "Replica");
    let plain = serde_json::to_value(golden_builder().build()).unwrap();
    for k in [
        "endpoint_storage_addr",
        "endpoint_storage_token",
        "autoprewarm",
    ] {
        assert!(plain.get(k).is_none(), "{k}");
    }
}

fn golden_builder() -> ComputeSpecBuilder {
    ComputeSpecBuilder::new(t(), tl())
        .ids(
            "prj-01J9Z8Q4XWQ3T8F2D4G6H8J0KM",
            "br-01J9Z8Q4XWQ3T8F2D4G6H8J0KN",
            "ep-01J9Z8Q4XWQ3T8F2D4G6H8J0KP",
        )
        .compute_id("cmp-01J9Z8Q4XWQ3T8F2D4G6H8J0KQ")
        .safekeepers(["loams-wal.pg.svc:5454"])
        .pageserver(1234, "postgresql://no_user@pageserver:6400")
        .storage_auth_token(Secret::new("storage-jwt".into()))
        .role(RoleRec {
            name: "app".into(),
            encrypted_password: Some("SCRAM-SHA-256$4096:c2FsdA==$c3RvcmVk:c2VydmVy".into()),
        })
        .database(DatabaseRec {
            name: "app".into(),
            owner: "app".into(),
        })
        .setting(Setting::new("max_connections", "100", "integer"))
        .max_cluster_size_mb(10_240)
        .suspend_timeout_seconds(300)
        .mode(ComputeMode::Primary)
}

/// One record set produces `tests/fixtures/spec_main.json` (the fork's
/// `compute_api::spec::ComputeSpec`).
#[test]
fn spec_builder_golden() {
    let spec = golden_builder().build();
    assert_eq!(
        canonical(&serde_json::to_string(&spec).unwrap()),
        canonical(&fixture("spec_main.json"))
    );
}

/// A caller's setting replaces a default of the same name in place, and the
/// quota's `neon.max_cluster_size` wins over a caller's: every name appears
/// once, so `postgresql.conf` never holds two lines for one GUC.
#[test]
fn spec_settings_are_unique() {
    let spec = golden_builder()
        .setting(Setting::new("max_replication_write_lag", "1GB", "integer"))
        .setting(Setting::new("neon.max_cluster_size", "1", "integer"))
        .setting(Setting::new("max_connections", "200", "integer"))
        .build();
    let settings = spec.cluster.settings.unwrap();
    let names: Vec<&str> = settings.iter().map(|s| s.name.as_str()).collect();
    let mut unique = names.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), names.len(), "{names:?}");
    let value = |n: &str| {
        settings
            .iter()
            .find(|s| s.name == n)
            .and_then(|s| s.value.clone())
            .unwrap()
    };
    assert_eq!(value("max_replication_write_lag"), "1GB");
    assert_eq!(names[3], "max_replication_write_lag", "{names:?}");
    assert_eq!(value("max_connections"), "200");
    assert_eq!(value("neon.max_cluster_size"), "10240");
}

/// Secrets never show in `Debug`, `Display` or an error: not the storage
/// token, the `loams-wal` token, the compute JWT, the endpoint-storage token
/// nor a role's SCRAM verifier.
#[tokio::test]
async fn secrets_are_redacted() {
    const SECRETS: [&str; 5] = [
        "sentinel-storage-token",
        "sentinel-wal-token",
        "sentinel-compute-jwt",
        "sentinel-es-token",
        "SCRAM-SHA-256$4096:c2FsdA==$c3RvcmVk:c2VydmVy",
    ];
    let s = Secret::new(SECRETS[0].to_string());
    assert_eq!(format!("{s:?}"), "[redacted]");
    assert_eq!(format!("{s}"), "[redacted]");
    let (fake, url) = serve().await;
    let ps = NeonClient::new(
        NeonEndpoints {
            pageserver: url.parse().unwrap(),
            storcon: None,
        },
        Some(Secret::new(SECRETS[0].into())),
    )
    .unwrap();
    let wal = WalClient::new(url.parse().unwrap(), Some(Secret::new(SECRETS[1].into()))).unwrap();
    let ctl = ComputeCtlClient::new(url.parse().unwrap(), Secret::new(SECRETS[2].into())).unwrap();
    let builder = golden_builder()
        .endpoint_storage("es:1", Secret::new(SECRETS[3].into()))
        .storage_auth_token(Secret::new(SECRETS[0].into()));
    let spec = builder.clone().build();
    // Errors from each client, with the token on the wire.
    fake.answer(401, r#"{"msg":"Unauthorized: malformed jwt token"}"#);
    let e1 = ps.timeline(t(), tl()).await.unwrap_err();
    assert_eq!(fake.last().auth, Some(format!("Bearer {}", SECRETS[0])));
    fake.answer(401, r#"{"msg":"Unauthorized"}"#);
    let e2 = wal.timeline_status(t(), tl()).await.unwrap_err();
    fake.answer(401, &fixture("compute_unauthorized.response.json"));
    let e3 = ctl.status().await.unwrap_err();
    let shown = [
        format!("{ps:?}"),
        format!("{wal:?}"),
        format!("{ctl:?}"),
        format!("{builder:?}"),
        format!("{spec:?}"),
        format!("{e1:?} {e1}"),
        format!("{e2:?} {e2}"),
        format!("{e3:?} {e3} {:?}", e3.error_info()),
    ];
    for text in &shown {
        for secret in SECRETS {
            assert!(!text.contains(secret), "{secret} in {text}");
        }
        // The SCRAM verifier's pieces too.
        assert!(!text.contains("c3RvcmVk"), "{text}");
    }
}

/// The id and LSN text forms Neon uses.
#[test]
fn ids_and_lsns_round_trip() {
    assert_eq!(t().to_string(), T);
    assert!("4c6f".parse::<TenantId>().is_err());
    let lsn: Lsn = "1/6B59C00".parse().unwrap();
    assert_eq!(lsn.0, 0x1_06B5_9C00);
    assert_eq!(lsn.to_string(), "1/6B59C00");
    assert!("16B59C00".parse::<Lsn>().is_err());
}
