//! `loams-confluent` against a local fake of the Confluent Cloud API shapes it
//! uses (`/iam/v2/api-keys`, `/cmk/v2/clusters`), a memory secret store, a
//! fake cloud CLI and a fake connect admin API. Ids are placeholders.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use base64::Engine as _;
use loams_confluent::cli::{CheckOpts, Cloud, KeyOpts, RotateOpts, check, key_create, key_rotate};
use loams_confluent::store::{CliStore, MemoryStore, SecretSink, Target};
use loams_confluent::{Client, CloudKey, Error, Secret, keys};
use serde_json::{Value, json};

const CLOUD_ID: &str = "CLOUDKEY01";
const CLOUD_SECRET: &str = "cloud-secret-PLANTED";

#[derive(Default)]
struct Fake {
    /// method path, in order (shared with the admin API's calls).
    calls: Vec<String>,
    /// id -> (owner, resource)
    keys: BTreeMap<String, (String, String)>,
    next: u32,
    /// answers to send before the real one, by path prefix
    scripted: Vec<(String, u16, Option<u64>)>,
    created_at: BTreeMap<String, Instant>,
    deleted_at: BTreeMap<String, Instant>,
    restart_status: u16,
    base: String,
}

type Shared = Arc<Mutex<Fake>>;

fn authorized(h: &HeaderMap) -> bool {
    let want = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{CLOUD_ID}:{CLOUD_SECRET}"))
    );
    h.get("authorization").and_then(|v| v.to_str().ok()) == Some(want.as_str())
}

/// A scripted failure for `path`, if one is queued.
fn scripted(f: &mut Fake, path: &str) -> Option<(StatusCode, HeaderMap, String)> {
    let i = f
        .scripted
        .iter()
        .position(|(p, _, _)| path.starts_with(p.as_str()))?;
    let (_, status, retry) = f.scripted.remove(i);
    let mut h = HeaderMap::new();
    if let Some(r) = retry {
        h.insert("retry-after", r.to_string().parse().unwrap());
    }
    Some((
        StatusCode::from_u16(status).unwrap(),
        h,
        json!({"errors":[{"status": status.to_string(), "detail": "scripted"}]}).to_string(),
    ))
}

async fn create_key(
    State(f): State<Shared>,
    h: HeaderMap,
    body: String,
) -> (StatusCode, HeaderMap, String) {
    let mut f = f.lock().unwrap();
    f.calls.push("POST /iam/v2/api-keys".into());
    if let Some(r) = scripted(&mut f, "/iam/v2/api-keys") {
        return r;
    }
    if !authorized(&h) {
        return (StatusCode::UNAUTHORIZED, HeaderMap::new(), "{}".into());
    }
    let b: Value = serde_json::from_str(&body).unwrap();
    let owner = b["spec"]["owner"]["id"].as_str().unwrap().to_string();
    let resource = b["spec"]["resource"]["id"].as_str().unwrap().to_string();
    assert!(
        b["spec"]["resource"]["environment"]
            .as_str()
            .unwrap()
            .starts_with("env-")
    );
    f.next += 1;
    let id = format!("KEYTEST{:02}", f.next);
    f.keys.insert(id.clone(), (owner.clone(), resource.clone()));
    f.created_at.insert(id.clone(), Instant::now());
    (
        StatusCode::ACCEPTED,
        HeaderMap::new(),
        json!({"id": id, "spec": {"secret": format!("key-secret-PLANTED-{}", f.next), "owner": {"id": owner}, "resource": {"id": resource}}}).to_string(),
    )
}

async fn list_keys(
    State(f): State<Shared>,
    Query(q): Query<BTreeMap<String, String>>,
) -> (StatusCode, HeaderMap, String) {
    let mut f = f.lock().unwrap();
    f.calls.push("GET /iam/v2/api-keys".into());
    if let Some(r) = scripted(&mut f, "/iam/v2/api-keys?") {
        return r;
    }
    let owner = q.get("spec.owner").cloned().unwrap_or_default();
    let resource = q.get("spec.resource").cloned().unwrap_or_default();
    let all: Vec<Value> = f
        .keys
        .iter()
        .filter(|(_, (o, r))| *o == owner && *r == resource)
        .map(|(id, (o, r))| json!({"id": id, "spec": {"display_name": "k", "owner": {"id": o}, "resource": {"id": r}}}))
        .collect();
    // one key per page, to exercise paging
    let page: usize = q.get("page_token").map_or(0, |t| t.parse().unwrap());
    let mut body =
        json!({"data": all.get(page).cloned().into_iter().collect::<Vec<_>>(), "metadata": {}});
    if page + 1 < all.len() {
        body["metadata"]["next"] = json!(format!(
            "{}/iam/v2/api-keys?spec.owner={owner}&spec.resource={resource}&page_size=1&page_token={}",
            f.base,
            page + 1
        ));
    }
    (StatusCode::OK, HeaderMap::new(), body.to_string())
}

async fn delete_key(State(f): State<Shared>, Path(id): Path<String>) -> StatusCode {
    let mut f = f.lock().unwrap();
    f.calls.push(format!("DELETE /iam/v2/api-keys/{id}"));
    f.deleted_at.insert(id.clone(), Instant::now());
    if f.keys.remove(&id).is_some() {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

async fn get_cluster(
    State(f): State<Shared>,
    Path(id): Path<String>,
) -> (StatusCode, HeaderMap, String) {
    let mut f = f.lock().unwrap();
    f.calls.push(format!("GET /cmk/v2/clusters/{id}"));
    if let Some(r) = scripted(&mut f, "/cmk/v2/clusters") {
        return r;
    }
    if id != "lkc-test01" {
        return (StatusCode::NOT_FOUND, HeaderMap::new(), "{}".into());
    }
    let bootstrap = f
        .calls
        .iter()
        .find_map(|c| c.strip_prefix("bootstrap ").map(str::to_string))
        .unwrap_or_else(|| "127.0.0.1:9".into());
    (
        StatusCode::OK,
        HeaderMap::new(),
        json!({"id": id, "spec": {"display_name": "loams-prod", "cloud": "AWS", "region": "us-east-1",
               "config": {"kind": "Basic"}, "kafka_bootstrap_endpoint": format!("SASL_SSL://{bootstrap}")}})
        .to_string(),
    )
}

async fn restart(State(f): State<Shared>, h: HeaderMap, Path(id): Path<String>) -> StatusCode {
    let mut f = f.lock().unwrap();
    assert_eq!(
        h.get("authorization").and_then(|v| v.to_str().ok()),
        Some("Bearer admin-token-PLANTED")
    );
    f.calls.push(format!("RESTART {id}"));
    StatusCode::from_u16(f.restart_status).unwrap()
}

async fn serve(f: Shared) -> String {
    let app = Router::new()
        .route("/iam/v2/api-keys", post(create_key).get(list_keys))
        .route("/iam/v2/api-keys/{id}", axum::routing::delete(delete_key))
        .route("/cmk/v2/clusters/{id}", get(get_cluster))
        .route("/v1/connect/instances/{id}", post(restart))
        .with_state(f.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    f.lock().unwrap().base = format!("http://{addr}");
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    format!("http://{addr}")
}

fn http() -> reqwest::Client {
    loams_confluent::cli::install_crypto();
    reqwest::Client::new()
}

fn client(base: &str) -> Client {
    Client::new(
        base,
        CloudKey::new(CLOUD_ID, Secret::new(CLOUD_SECRET)),
        http(),
    )
    .with_backoff(Duration::from_millis(5))
}

fn opts(base: &str) -> KeyOpts {
    KeyOpts {
        integration: "confluent".into(),
        service_account: "sa-test01".into(),
        cluster: "lkc-test01".into(),
        environment: "env-test01".into(),
        schema_registry: None,
        cloud: Cloud::Aws,
        secret_prefix: "loams/prod".into(),
        region: Some("us-east-1".into()),
        project: None,
        vault: None,
        api_url: base.into(),
        secret_cli: None,
    }
}

#[tokio::test]
async fn key_create_never_prints_secret() {
    let f = Shared::default();
    let base = serve(f.clone()).await;
    let store = MemoryStore::default();
    let mut out = Vec::new();
    let mut o = opts(&base);
    o.schema_registry = Some("lsrc-test01".into());
    let created = key_create(&client(&base), &store, &o, &mut out)
        .await
        .unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains(&created.kafka_key) && !out.contains("PLANTED"),
        "{out}"
    );
    assert_eq!(
        store.versions("loams/prod/confluent-confluent-api-key"),
        vec![created.kafka_key.clone()]
    );
    assert_eq!(
        store.versions("loams/prod/confluent-confluent-api-secret"),
        vec!["key-secret-PLANTED-1".to_string()]
    );
    assert_eq!(
        store.versions("loams/prod/confluent-confluent-sr-secret"),
        vec!["key-secret-PLANTED-2".to_string()]
    );
    let keys = &f.lock().unwrap().keys;
    assert_eq!(
        keys[&created.kafka_key],
        ("sa-test01".into(), "lkc-test01".into())
    );
    assert_eq!(
        keys[created.sr_key.as_ref().unwrap()],
        ("sa-test01".into(), "lsrc-test01".into())
    );
}

#[tokio::test]
async fn key_create_undoes_the_key_when_the_store_refuses() {
    let f = Shared::default();
    let base = serve(f.clone()).await;
    let store = MemoryStore::default();
    store.fail_on("api-secret");
    let err = key_create(&client(&base), &store, &opts(&base), &mut Vec::new())
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("deleted the new key KEYTEST01") && !msg.contains("PLANTED"),
        "{msg}"
    );
    assert!(f.lock().unwrap().keys.is_empty(), "the useless key is gone");
}

#[tokio::test]
async fn key_create_refuses_bad_ids() {
    let base = "http://127.0.0.1:9";
    let mut o = opts(base);
    o.cluster = "lkc-x;rm".into();
    assert!(matches!(
        key_create(&client(base), &MemoryStore::default(), &o, &mut Vec::new()).await,
        Err(Error::Usage(_))
    ));
    let mut o = opts(base);
    o.integration = "Bad_Name".into();
    assert!(matches!(
        key_create(&client(base), &MemoryStore::default(), &o, &mut Vec::new()).await,
        Err(Error::Usage(_))
    ));
    let mut o = opts(base);
    o.cloud = Cloud::Azure;
    assert!(matches!(
        key_create(&client(base), &MemoryStore::default(), &o, &mut Vec::new()).await,
        Err(Error::Usage(_))
    ));
}

fn rotate_opts(base: &str, token: &std::path::Path, grace: Duration) -> RotateOpts {
    RotateOpts {
        key: opts(base),
        grace,
        connect_admin: Some(base.into()),
        admin_token_file: Some(token.to_path_buf()),
        restart_instances: vec!["orders-out".into(), "orders-in".into()],
        keep_old: false,
        refresh_wait: Duration::ZERO,
    }
}

fn token_file() -> tempfile_path::TempPath {
    tempfile_path::TempPath::with("admin-token-PLANTED\n")
}

#[tokio::test]
async fn rotate_deletes_old_after_grace() {
    let f = Shared::default();
    f.lock().unwrap().restart_status = 200;
    let base = serve(f.clone()).await;
    let c = client(&base);
    let store = MemoryStore::default();
    // two old keys, so the list pages
    let old1 = keys::create(&c, "sa-test01", "lkc-test01", "env-test01", "old", "old")
        .await
        .unwrap()
        .id;
    let old2 = keys::create(&c, "sa-test01", "lkc-test01", "env-test01", "old", "old")
        .await
        .unwrap()
        .id;
    let token = token_file();
    let grace = Duration::from_millis(300);
    let mut out = Vec::new();
    let created = key_rotate(
        &c,
        &store,
        &http(),
        &rotate_opts(&base, token.path(), grace),
        &mut out,
    )
    .await
    .unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(!out.contains("PLANTED"), "{out}");
    let f = f.lock().unwrap();
    // order: new key, restarts, then the deletes of exactly the old keys
    let calls: Vec<&String> = f.calls.iter().filter(|c| !c.starts_with("GET")).collect();
    let pos = |s: &str| {
        calls
            .iter()
            .position(|c| *c == s)
            .unwrap_or_else(|| panic!("{s} missing: {calls:?}"))
    };
    assert!(pos("RESTART orders-out:restart") < pos(&format!("DELETE /iam/v2/api-keys/{old1}")));
    assert!(pos("RESTART orders-in:restart") < pos(&format!("DELETE /iam/v2/api-keys/{old2}")));
    assert!(
        !calls
            .iter()
            .any(|c| **c == format!("DELETE /iam/v2/api-keys/{}", created.kafka_key))
    );
    assert!(f.deleted_at[&old1].duration_since(f.created_at[&created.kafka_key]) >= grace);
    assert_eq!(
        f.keys.keys().cloned().collect::<Vec<_>>(),
        vec![created.kafka_key.clone()]
    );
    assert_eq!(
        store.versions("loams/prod/confluent-confluent-api-key"),
        vec![created.kafka_key]
    );
}

#[tokio::test]
async fn rotate_keeps_old_keys_when_a_restart_fails() {
    let f = Shared::default();
    f.lock().unwrap().restart_status = 503;
    let base = serve(f.clone()).await;
    let c = client(&base);
    let old = keys::create(&c, "sa-test01", "lkc-test01", "env-test01", "old", "old")
        .await
        .unwrap()
        .id;
    let token = token_file();
    let mut out = Vec::new();
    key_rotate(
        &c,
        &MemoryStore::default(),
        &http(),
        &rotate_opts(&base, token.path(), Duration::ZERO),
        &mut out,
    )
    .await
    .unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains("kept the old keys (an instance did not restart)") && out.contains(&old),
        "{out}"
    );
    assert!(f.lock().unwrap().keys.contains_key(&old));
}

#[tokio::test]
async fn retries_429_with_retry_after_and_5xx() {
    let f = Shared::default();
    f.lock().unwrap().scripted = vec![
        ("/iam/v2/api-keys".into(), 429, Some(0)),
        ("/iam/v2/api-keys".into(), 503, None),
    ];
    let base = serve(f.clone()).await;
    let k = keys::create(
        &client(&base),
        "sa-test01",
        "lkc-test01",
        "env-test01",
        "n",
        "d",
    )
    .await
    .unwrap();
    assert_eq!(k.id, "KEYTEST01");
    assert_eq!(f.lock().unwrap().calls.len(), 3);
}

#[tokio::test]
async fn gives_up_after_repeated_5xx_and_maps_errors() {
    let f = Shared::default();
    f.lock().unwrap().scripted = (0..4)
        .map(|_| ("/iam/v2/api-keys".to_string(), 500, None))
        .collect();
    let base = serve(f.clone()).await;
    let c = client(&base);
    let e = keys::create(&c, "sa-test01", "lkc-test01", "env-test01", "n", "d")
        .await
        .unwrap_err();
    assert!(matches!(e, Error::Api { status: 500, .. }), "{e:?}");
    f.lock().unwrap().scripted = vec![("/iam/v2/api-keys".into(), 403, None)];
    let e = keys::create(&c, "sa-test01", "lkc-test01", "env-test01", "n", "d")
        .await
        .unwrap_err();
    assert!(matches!(e, Error::Forbidden(_)), "{e:?}");
    let e = Client::new(
        &base,
        CloudKey::new("WRONG", Secret::new("wrong-PLANTED")),
        http(),
    )
    .with_backoff(Duration::from_millis(1));
    let e = keys::create(&e, "sa-test01", "lkc-test01", "env-test01", "n", "d")
        .await
        .unwrap_err();
    assert!(matches!(e, Error::Unauthorized));
    let e = loams_confluent::clusters::get(&c, "env-test01", "lkc-none")
        .await
        .unwrap_err();
    assert!(matches!(e, Error::NotFound(_)));
}

#[tokio::test]
async fn secret_never_in_debug_or_error() {
    let f = Shared::default();
    let base = serve(f.clone()).await;
    let c = client(&base);
    let k = keys::create(&c, "sa-test01", "lkc-test01", "env-test01", "n", "d")
        .await
        .unwrap();
    let mut seen = vec![format!("{k:?}"), format!("{c:?}")];
    for status in [400, 401, 403, 404, 409, 500] {
        f.lock().unwrap().scripted = (0..4)
            .map(|_| ("/iam/v2/api-keys".to_string(), status, None))
            .collect();
        let e = keys::create(&c, "sa-test01", "lkc-test01", "env-test01", "n", "d")
            .await
            .unwrap_err();
        seen.push(format!("{e} {e:?}"));
    }
    for s in seen {
        assert!(!s.contains("PLANTED"), "{s}");
    }
}

#[tokio::test]
async fn check_reports_region_and_reachability() {
    let f = Shared::default();
    let kafka = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let kafka_addr = kafka.local_addr().unwrap().to_string();
    f.lock()
        .unwrap()
        .calls
        .push(format!("bootstrap {kafka_addr}"));
    let base = serve(f.clone()).await;
    let c = client(&base);
    let mut o = CheckOpts {
        cloud: Cloud::Aws,
        region: "us-east-1".into(),
        cluster: "lkc-test01".into(),
        environment: "env-test01".into(),
        schema_registry_url: None,
        timeout: Duration::from_secs(2),
        api_url: base.clone(),
    };
    let mut out = Vec::new();
    assert!(check(&c, &o, &mut out).await.unwrap());
    let s = String::from_utf8(out).unwrap();
    assert!(
        s.contains("ok   the cluster is in the VM's cloud and region")
            && s.contains(&format!("ok   bootstrap {kafka_addr}")),
        "{s}"
    );
    o.region = "us-west-2".into();
    let mut out = Vec::new();
    assert!(
        check(&c, &o, &mut out).await.unwrap(),
        "cross-region is a warning"
    );
    assert!(
        String::from_utf8(out)
            .unwrap()
            .contains("warn the cluster is in aws us-east-1 and the VM in aws us-west-2")
    );
    drop(kafka);
    let mut out = Vec::new();
    assert!(
        !check(&c, &o, &mut out).await.unwrap(),
        "an unreachable bootstrap fails"
    );
}

#[tokio::test]
async fn cli_store_passes_the_value_on_stdin_and_creates_when_missing() {
    let dir = tempfile_path::TempDir::new();
    let log = dir.path().join("log");
    let script = dir.path().join("fake-aws");
    // Records argv and stdin; the first put fails as a missing secret does.
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf 'ARGS %s\\n' \"$*\" >> '{log}'\nprintf 'STDIN ' >> '{log}'\ncat >> '{log}'\nprintf '\\n' >> '{log}'\ncase \"$*\" in *put-secret-value*) echo 'An error occurred (ResourceNotFoundException)' >&2; exit 254;; esac\nexit 0\n",
            log = log.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let store = CliStore::new(Target::Aws {
        region: Some("us-east-1".into()),
    })
    .with_program(&script);
    store
        .put(
            "loams/prod/confluent-confluent-api-secret",
            &Secret::new("value-PLANTED"),
        )
        .await
        .unwrap();
    let log = std::fs::read_to_string(&log).unwrap();
    let args: Vec<&str> = log.lines().filter(|l| l.starts_with("ARGS")).collect();
    assert_eq!(args.len(), 2, "{log}");
    assert!(
        args[0].contains("put-secret-value") && args[1].contains("create-secret"),
        "{log}"
    );
    assert!(args.iter().all(|a| !a.contains("PLANTED")
        && a.contains("file:///dev/stdin")
        && a.contains("--region us-east-1")));
    assert_eq!(
        log.lines().filter(|l| *l == "STDIN value-PLANTED").count(),
        2,
        "{log}"
    );
}

/// Small temporary paths without another dependency.
mod tempfile_path {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);

    fn fresh() -> PathBuf {
        let base =
            std::env::var_os("CARGO_TARGET_TMPDIR").map_or_else(std::env::temp_dir, PathBuf::from);
        base.join(format!(
            "loams-confluent-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ))
    }

    pub struct TempPath(PathBuf);
    impl TempPath {
        pub fn with(content: &str) -> Self {
            let p = fresh();
            std::fs::write(&p, content).unwrap();
            Self(p)
        }
        pub fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempPath {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    pub struct TempDir(PathBuf);
    impl TempDir {
        pub fn new() -> Self {
            let p = fresh();
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        pub fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[tokio::test]
async fn rotate_without_instances_keeps_old_keys() {
    let f = Shared::default();
    f.lock().unwrap().restart_status = 200;
    let base = serve(f.clone()).await;
    let c = client(&base);
    let old = keys::create(&c, "sa-test01", "lkc-test01", "env-test01", "old", "old")
        .await
        .unwrap()
        .id;
    let token = token_file();
    let mut o = rotate_opts(&base, token.path(), Duration::ZERO);
    o.restart_instances.clear();
    let mut out = Vec::new();
    key_rotate(&c, &MemoryStore::default(), &http(), &o, &mut out)
        .await
        .unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains("restarts nothing") && out.contains(&old),
        "{out}"
    );
    let f = f.lock().unwrap();
    assert!(f.keys.contains_key(&old));
    assert!(
        !f.calls.iter().any(|c| c.starts_with("RESTART")),
        "{:?}",
        f.calls
    );
}
