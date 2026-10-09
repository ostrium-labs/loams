//! `loams-neon` against recorded fixtures (PG2 Task 2): the request bodies
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
use loams_neon::compute_ctl::{ComputeCtlClient, ComputeCtlConfig, ComputeStatus, TerminateMode};
use loams_neon::pageserver::{NeonClient, NeonEndpoints, TenantConfig, TimelineCreate};
use loams_neon::spec::{ComputeMode, ComputeSpecBuilder, DatabaseRec, RoleRec, Setting};
use loams_neon::wal::{WalClient, WalTimelineCreate};
use loams_neon::{Component, Lsn, NeonError, Secret, TenantId, TimelineId};
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
    assert_eq!(seen.method, "PUT");
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
    let found = client.lsn_by_timestamp(t(), tl(), at).await.unwrap();
    assert_eq!(
        fake.last().path_and_query,
        format!(
            "/v1/tenant/{T}/timeline/{TL}/get_lsn_by_timestamp?timestamp=2026-10-09T02%3A41%3A46Z"
        )
    );
    assert_eq!(found.lsn, "0/14E8F20".parse().unwrap());
    assert_eq!(found.kind, "nodata");

    fake.answer(202, &fixture("delete_branch.response.json"));
    client.delete_timeline(t(), br()).await.unwrap();
    let seen = fake.last();
    assert_eq!(seen.method, "DELETE");
    assert_eq!(seen.path_and_query, format!("/v1/tenant/{T}/timeline/{BR}"));
}

/// Through the storage controller: tenants are created with `POST
/// /v1/tenant` (it chooses the generation), everything else goes to the
/// same routes on the controller.
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
    );
    sc.answer(
        201,
        r#"{"shards":[{"shard_id":"4c6f616d734e656f6e54656e616e7431","node_id":1}]}"#,
    );
    client
        .attach_tenant(t(), 1, &TenantConfig::default())
        .await
        .unwrap();
    let seen = sc.last();
    assert_eq!(
        (seen.method.as_str(), seen.path_and_query.as_str()),
        ("POST", "/v1/tenant")
    );
    assert_eq!(
        canonical(&seen.body),
        canonical(&format!(r#"{{"new_tenant_id":"{T}"}}"#))
    );
    sc.answer(201, &fixture("create_timeline.response.json"));
    client
        .create_timeline(t(), &TimelineCreate::bootstrap(tl(), 17))
        .await
        .unwrap();
    assert_eq!(sc.last().path_and_query, format!("/v1/tenant/{T}/timeline"));
    sc.answer(200, &fixture("timeline_get.response.json"));
    client.timeline(t(), tl()).await.unwrap();
    assert!(
        ps.seen.lock().unwrap().is_empty(),
        "the pageserver is not called directly"
    );
}

/// Errors carry the status and the component's `msg`, and map to
/// `loams.errors.v1` reasons.
#[tokio::test]
async fn neon_error_maps_to_reason() {
    let (fake, url) = serve().await;
    let client = pageserver(&url);
    fake.answer(409, &fixture("conflict.response.json"));
    let e = client
        .create_timeline(t(), &TimelineCreate::bootstrap(br(), 17))
        .await
        .unwrap_err();
    assert_eq!(e.status, 409);
    assert_eq!(e.msg, "timeline already exists with different parameters");
    assert_eq!(e.reason(), "already_exists");

    fake.answer(404, &fixture("not_found.response.json"));
    let e = client.timeline(t(), tl()).await.unwrap_err();
    assert_eq!((e.status, e.reason()), (404, "not_found"));
    assert!(e.msg.starts_with("NotFound: Timeline"));

    for status in [500, 502, 503] {
        fake.answer(status, r#"{"msg":"down"}"#);
        let e = client.timeline(t(), tl()).await.unwrap_err();
        assert_eq!(e.reason(), "storage_unavailable", "{status}");
    }
    // A component that cannot be reached is unavailable too.
    let gone = pageserver("http://127.0.0.1:1");
    let e = gone.timeline(t(), tl()).await.unwrap_err();
    assert_eq!((e.status, e.reason()), (0, "storage_unavailable"));

    // The component refusing Loams's own storage token is a misconfiguration,
    // not the caller's: never `unauthenticated`, which tells a client to sign
    // in again.
    for status in [401, 403] {
        fake.answer(status, r#"{"msg":"Unauthorized: malformed jwt token"}"#);
        let e = client.timeline(t(), tl()).await.unwrap_err();
        assert_eq!(e.reason(), "internal", "{status}");
    }
    // Any other refusal is a request Loams should not have sent.
    fake.answer(405, "");
    let e = client.timeline(t(), tl()).await.unwrap_err();
    assert_eq!(e.reason(), "internal");

    let info = NeonError {
        status: 503,
        msg: "x".into(),
        component: Component::Pageserver,
    }
    .error_info();
    assert_eq!(info.reason, "storage_unavailable");
    let meta = |k: &str| info.metadata.get(k).map(String::as_str);
    assert_eq!(meta("component"), Some("pageserver"));
    assert_eq!(meta("status"), Some("503"));
    assert_eq!(e.component, Component::Pageserver);
}

/// Each client names its component, and only the storage components'
/// outages are `storage_unavailable`; a compute that cannot be reached is
/// plain `unavailable`. compute_ctl's errors are `{"error": ...}`.
#[tokio::test]
async fn errors_name_their_component() {
    let wal = WalClient::new("http://127.0.0.1:1".parse().unwrap(), None);
    let e = wal.timeline_status(t(), tl()).await.unwrap_err();
    assert_eq!(
        (e.component, e.reason()),
        (Component::Wal, "storage_unavailable")
    );

    let sc = NeonClient::new(
        NeonEndpoints {
            pageserver: "http://127.0.0.1:1".parse().unwrap(),
            storcon: Some("http://127.0.0.1:1".parse().unwrap()),
        },
        None,
    );
    let e = sc.timeline(t(), tl()).await.unwrap_err();
    assert_eq!(e.component, Component::StorageController);
    assert_eq!(
        e.error_info().metadata.get("component").map(String::as_str),
        Some("storage_controller")
    );

    let (fake, url) = serve().await;
    let ctl = ComputeCtlClient::new(url.parse().unwrap(), Secret::new("jwt".into()));
    fake.answer(412, r#"{"error":"invalid compute status: running"}"#);
    let e = ctl.status().await.unwrap_err();
    assert_eq!(e.component, Component::ComputeCtl);
    assert_eq!(e.msg, "invalid compute status: running");
    assert_eq!(e.reason(), "failed_precondition");
    let gone = ComputeCtlClient::new(
        "http://127.0.0.1:1".parse().unwrap(),
        Secret::new("jwt".into()),
    );
    assert_eq!(gone.status().await.unwrap_err().reason(), "unavailable");
}

/// loams-wal's timeline API, as a stock safekeeper's.
#[tokio::test]
async fn wal_client_matches_fixtures() {
    let (fake, url) = serve().await;
    let wal = WalClient::new(url.parse().unwrap(), Some(Secret::new("wal-token".into())));
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
    let ctl = ComputeCtlClient::new(url.parse().unwrap(), Secret::new("compute-jwt".into()));
    fake.answer(
        200,
        r#"{"start_time":"2026-10-09T02:41:46Z","tenant":null,"timeline":null,"status":"empty","last_active":null,"error":null}"#,
    );
    let st = ctl.status().await.unwrap();
    assert_eq!(st.status, ComputeStatus::Empty);
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

    // A failed promotion is a 500 whose `PromoteState` names the error.
    fake.answer(
        500,
        r#"{"status":"failed","error":"compute mode \"primary\" is not replica"}"#,
    );
    let e = ctl
        .promote(&spec, "0/14E8F98".parse().unwrap())
        .await
        .unwrap_err();
    assert_eq!(e.status, 500);
    assert_eq!(e.msg, r#"compute mode "primary" is not replica"#);
    // A 200 that is not `completed` is not a promotion either.
    fake.answer(200, r#"{"status":"not_promoted"}"#);
    let e = ctl
        .promote(&spec, "0/14E8F98".parse().unwrap())
        .await
        .unwrap_err();
    assert!(e.msg.contains("not_promoted"), "{e}");
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

/// Secrets never show in `Debug` or `Display`.
#[test]
fn secrets_are_redacted() {
    let s = Secret::new("hunter2".to_string());
    assert_eq!(format!("{s:?}"), "[redacted]");
    assert_eq!(format!("{s}"), "[redacted]");
    let ctl = ComputeCtlClient::new(
        "http://x:1".parse().unwrap(),
        Secret::new("jwt-value".into()),
    );
    assert!(!format!("{ctl:?}").contains("jwt-value"));
    let wal = WalClient::new(
        "http://x:1".parse().unwrap(),
        Some(Secret::new("tok".into())),
    );
    assert!(!format!("{wal:?}").contains("tok\""));
    let spec = golden_builder().build();
    assert!(!format!("{spec:?}").contains("storage-jwt"));
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
