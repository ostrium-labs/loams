//! The upcall listener (PG2 Task 8): `compute_ctl`'s spec upcall and the
//! storage controller's `notify-attach`, on the local (embedded) store, a
//! fake `compute_ctl` and real HTTP on a loopback port.
#![allow(clippy::unwrap_used)]

#[allow(unused_macros)]
mod common;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use loams_pg_control::ids::{self, BranchId, ComputeId, EndpointId, ProjectId};
use loams_pg_control::model::{
    BranchKey, BranchRec, BranchState, ComputeBinding, ComputeKey, ComputeRec, ComputeStatus, Cu,
    DatabaseRec, DesiredState, EndpointKey, EndpointRec, EndpointState, EndpointType, PoolMode,
    ProjectRec, ProjectState, RoleRec, ShardAttachment, WalService,
};
use loams_pg_control::secrets::{Secret, SecretRef};
use loams_pg_control::service::Clock;
use loams_pg_control::store::conformance::{Factory, local_factory};
use loams_pg_control::upcall::{
    ComputeConfigurer, ComputeKeys, ConfigureError, UpcallConfig, UpcallError, UpcallServer,
};
use loams_pg_control::{KvControlStore, PgControlStore};
use loams_postgres::compute_ctl::ComputeCtlConfig;
use loams_postgres::spec::ComputeSpec;
use serde_json::Value;
use tokio::sync::oneshot;

use common::T0_MS;

/// The backend the cases run on.
fn factory() -> Factory {
    local_factory(env!("CARGO_TARGET_TMPDIR"))
}

const NOTIFY_TOKEN: &str = "notify-token-0123456789abcdef-sentinel";
const PAGESERVER_1: &str = "postgresql://no_user@pageserver-1:6400";
const PAGESERVER_7: &str = "postgresql://no_user@pageserver-7:6400";

/// A fake `compute_ctl` behind the configurer seam: records each
/// `/configure`, fails when told to.
#[derive(Default)]
struct FakeCtl {
    seen: Mutex<Vec<(String, Value)>>,
    fail: Mutex<bool>,
}

#[async_trait]
impl ComputeConfigurer for FakeCtl {
    async fn configure(
        &self,
        compute: &ComputeRec,
        spec: &ComputeSpec,
        config: &ComputeCtlConfig,
    ) -> Result<(), ConfigureError> {
        let body = serde_json::json!({ "spec": spec, "compute_ctl_config": config });
        self.seen.lock().unwrap().push((compute.id.clone(), body));
        if *self.fail.lock().unwrap() {
            return Err(ConfigureError("compute_ctl answered 500".into()));
        }
        Ok(())
    }
}

struct Ids {
    ns: String,
    project: ProjectId,
    main: BranchId,
    dev: BranchId,
    endpoint: EndpointId,
    compute: ComputeId,
}

struct Rig {
    store: KvControlStore,
    keys: Arc<ComputeKeys>,
    /// A key the server still trusts (a rotation), which no record names.
    older: ComputeKeys,
    ctl: Arc<FakeCtl>,
    base: String,
    http: reqwest::Client,
    ids: Ids,
    stop: Option<oneshot::Sender<()>>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

fn config(listen: SocketAddr) -> UpcallConfig {
    UpcallConfig {
        listen,
        api_listen: None,
        notify_token: Some(Secret::new(NOTIFY_TOKEN.into())),
        pageservers: BTreeMap::from([(1, PAGESERVER_1.into()), (7, PAGESERVER_7.into())]),
        default_pageserver: Some(1),
        wal_pools: BTreeMap::from([("default".into(), vec!["loams-wal.pg.svc:5454".into()])]),
        clock: Clock::from_fn(|| T0_MS),
        ..UpcallConfig::defaults(listen)
    }
}

async fn rig_with(tweak: impl FnOnce(&mut UpcallConfig)) -> Rig {
    rig_on(factory().store(common::options()).await.unwrap(), tweak).await
}

async fn rig_on(store: KvControlStore, tweak: impl FnOnce(&mut UpcallConfig)) -> Rig {
    let older = ComputeKeys::generate().unwrap();
    let keys = Arc::new(ComputeKeys::generate().unwrap().trust(&older));
    let ctl = Arc::new(FakeCtl::default());
    let mut cfg = config("127.0.0.1:0".parse().unwrap());
    tweak(&mut cfg);
    let server = UpcallServer::bind(store.clone(), keys.clone(), ctl.clone(), cfg)
        .await
        .unwrap();
    let base = format!("http://{}", server.local_addr());
    let (stop, stopped) = oneshot::channel();
    tokio::spawn(server.serve(async move {
        let _ = stopped.await;
    }));
    let ids = Ids {
        ns: "acme".into(),
        project: ProjectId::new(),
        main: BranchId::new(),
        dev: BranchId::new(),
        endpoint: EndpointId::new(),
        compute: ComputeId::new(),
    };
    let rig = Rig {
        store,
        keys,
        older,
        ctl,
        base,
        http: reqwest::Client::new(),
        ids,
        stop: Some(stop),
    };
    rig.seed().await;
    rig
}

async fn rig() -> Rig {
    rig_with(|_| {}).await
}

impl Rig {
    /// A ready project with branches `main` and `dev`, a role and a
    /// database on `main`, and an endpoint on `main` whose current compute
    /// is `ids.compute`, bound and running.
    async fn seed(&self) {
        let api = self.store.api_writer();
        let i = &self.ids;
        api.put(
            &ProjectRec {
                namespace: i.ns.clone(),
                id: i.project.to_string(),
                name: "shop".into(),
                tenant_id: ids::tenant_id(&i.project),
                pg_version: 17,
                wal: WalService::LoamsWal {
                    pool: "default".into(),
                },
                history_retention_s: 7 * 24 * 3600,
                region: "local".into(),
                default_branch_id: Some(i.main.to_string()),
                settings: BTreeMap::new(),
                state: ProjectState::Ready,
                created_at_ms: T0_MS,
                updated_at_ms: T0_MS,
            },
            None,
        )
        .await
        .unwrap();
        for (b, name, parent) in [(i.main, "main", None), (i.dev, "dev", Some(i.main))] {
            api.put(
                &BranchRec {
                    project_id: i.project.to_string(),
                    id: b.to_string(),
                    name: name.into(),
                    timeline_id: ids::timeline_id(&b),
                    parent_id: parent.map(|p: BranchId| p.to_string()),
                    ancestor_lsn: None,
                    expires_at_ms: None,
                    protected: false,
                    stripe_size: None,
                    shards: Vec::new(),
                    state: BranchState::Ready,
                    created_at_ms: T0_MS,
                    parent_time_ms: None,
                    updated_at_ms: T0_MS,
                    pg_version: 17,
                },
                None,
            )
            .await
            .unwrap();
        }
        api.put(
            &RoleRec {
                project_id: i.project.to_string(),
                branch_id: i.main.to_string(),
                name: "app".into(),
                secret_ref: SecretRef::new_role(&i.project),
                login: true,
                pool_mode: None,
                system: false,
                created_at_ms: T0_MS,
                updated_at_ms: T0_MS,
            },
            None,
        )
        .await
        .unwrap();
        api.put(
            &DatabaseRec {
                project_id: i.project.to_string(),
                branch_id: i.main.to_string(),
                name: "appdb".into(),
                owner: "app".into(),
                created_at_ms: T0_MS,
            },
            None,
        )
        .await
        .unwrap();
        api.put(
            &EndpointRec {
                project_id: i.project.to_string(),
                id: i.endpoint.to_string(),
                branch_id: i.main.to_string(),
                kind: EndpointType::ReadWrite,
                min_cu: Cu(1),
                max_cu: Cu(4),
                suspend_timeout_s: 300,
                pool_mode: PoolMode::Transaction,
                pg_settings: BTreeMap::from([("work_mem".into(), "8MB".into())]),
                desired: DesiredState::Running,
                state: EndpointState::Running,
                compute_id: Some(i.compute.to_string()),
                backend_addr: Some("ep.local:55500".into()),
                failure: None,
                ..EndpointRec::new("", "", "", EndpointType::ReadWrite)
            },
            None,
        )
        .await
        .unwrap();
        self.put_compute(i.compute, self.bound(), ComputeStatus::Running)
            .await;
    }

    fn bound(&self) -> Option<ComputeBinding> {
        Some(ComputeBinding {
            namespace: self.ids.ns.clone(),
            project_id: self.ids.project.to_string(),
            endpoint_id: self.ids.endpoint.to_string(),
        })
    }

    async fn put_compute(
        &self,
        id: ComputeId,
        binding: Option<ComputeBinding>,
        status: ComputeStatus,
    ) {
        let key = ComputeKey { id: id.to_string() };
        let current = self.store.get::<ComputeRec>(&key).await.unwrap();
        self.store
            .api_writer()
            .put(
                &ComputeRec {
                    id: id.to_string(),
                    binding,
                    pool: None,
                    bound_at_ms: None,
                    spec_version: 0,
                    status,
                    jwt_key_id: Some(self.keys.kid().to_string()),
                    ctl_url: Some(format!("http://{id}.computes:3080").to_lowercase()),
                    cu: Cu(4),
                    created_at_ms: T0_MS,
                    last_active_ms: None,
                },
                current.map(|c| c.version),
            )
            .await
            .unwrap();
    }

    fn token(&self, id: ComputeId) -> String {
        self.keys
            .issue_upcall_token(&id, T0_MS, Duration::from_secs(3600))
            .unwrap()
            .expose()
            .clone()
    }

    async fn spec(&self, id: ComputeId, bearer: Option<&str>) -> (u16, String) {
        let mut req = self
            .http
            .get(format!("{}/compute/api/v2/computes/{id}/spec", self.base));
        if let Some(t) = bearer {
            req = req.bearer_auth(t);
        }
        let resp = req.send().await.unwrap();
        (resp.status().as_u16(), resp.text().await.unwrap())
    }

    async fn notify(&self, bearer: Option<&str>, body: &Value) -> (u16, String) {
        let mut req = self
            .http
            .put(format!("{}/notify-attach", self.base))
            .json(body);
        if let Some(t) = bearer {
            req = req.bearer_auth(t);
        }
        let resp = req.send().await.unwrap();
        (resp.status().as_u16(), resp.text().await.unwrap())
    }

    fn attach_body(&self, node: u64) -> Value {
        serde_json::json!({
            "tenant_id": hex::encode(ids::tenant_id(&self.ids.project)),
            "preferred_az": null,
            "stripe_size": null,
            "shards": [{ "node_id": node, "shard_number": 0 }],
        })
    }

    async fn branch(&self, id: BranchId) -> BranchRec {
        self.store
            .get::<BranchRec>(&BranchKey {
                project_id: self.ids.project.to_string(),
                id: id.to_string(),
            })
            .await
            .unwrap()
            .unwrap()
            .record
    }
}

/// The spec upcall answers only to the compute's own token: none, a token
/// for another compute, one signed by another key, an expired one, one for
/// `compute_ctl`'s audience and garbage are all refused.
#[tokio::test]
async fn spec_requires_compute_jwt() {
    let r = rig().await;
    let id = r.ids.compute;
    let (status, body) = r.spec(id, Some(&r.token(id))).await;
    assert_eq!(status, 200, "{body}");

    assert_eq!(r.spec(id, None).await.0, 401);
    assert_eq!(r.spec(id, Some("not-a-jwt")).await.0, 401);
    // Another compute's token: authenticated, but not for this compute.
    let other = ComputeId::new();
    assert_eq!(r.spec(id, Some(&r.token(other))).await.0, 403);
    // Another key.
    let stranger = ComputeKeys::generate().unwrap();
    let forged = stranger
        .issue_upcall_token(&id, T0_MS, Duration::from_secs(3600))
        .unwrap();
    assert_eq!(r.spec(id, Some(forged.expose())).await.0, 401);
    // Expired (issued two hours before the server's clock, for one hour).
    let old = r
        .keys
        .issue_upcall_token(&id, T0_MS - 7_200_000, Duration::from_secs(3600))
        .unwrap();
    assert_eq!(r.spec(id, Some(old.expose())).await.0, 401);
    // A token pg-control signs for compute_ctl's own API is not an upcall
    // token.
    let ctl = r
        .keys
        .issue_compute_ctl_token(&id, T0_MS, Duration::from_secs(3600))
        .unwrap();
    assert_eq!(r.spec(id, Some(ctl.expose())).await.0, 401);
    // A key the server trusts, but not the one the compute's record names.
    let token = r
        .older
        .issue_upcall_token(&id, T0_MS, Duration::from_secs(3600))
        .unwrap();
    assert_eq!(r.spec(id, Some(token.expose())).await.0, 401);
    // A compute the store does not know.
    let unknown = ComputeId::new();
    assert_eq!(r.spec(unknown, Some(&r.token(unknown))).await.0, 404);
}

/// A compute bound to no endpoint (a warm-pool compute) gets `empty` and
/// the JWKS, so `compute_ctl` waits for `/configure`.
#[tokio::test]
async fn spec_for_unbound_compute_is_empty() {
    let r = rig().await;
    let pool = ComputeId::new();
    r.put_compute(pool, None, ComputeStatus::Pending).await;
    let (status, body) = r.spec(pool, Some(&r.token(pool))).await;
    assert_eq!(status, 200, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["status"], "empty");
    assert!(v["spec"].is_null());
    assert_eq!(v["compute_ctl_config"]["jwks"], r.keys.jwks());
    let jwk = &v["compute_ctl_config"]["jwks"]["keys"][0];
    assert_eq!(jwk["kty"], "OKP");
    assert_eq!(jwk["crv"], "Ed25519");
    assert_eq!(jwk["alg"], "EdDSA");
    assert_eq!(jwk["kid"], r.keys.kid());
}

/// The attached spec is built from the records, and is the same bytes for
/// the same compute until a record it reads changes (§28 §5.3).
#[tokio::test]
async fn spec_is_stable_until_record_changes() {
    let r = rig().await;
    let id = r.ids.compute;
    let token = r.token(id);
    let (status, first) = r.spec(id, Some(&token)).await;
    assert_eq!(status, 200, "{first}");
    let v: Value = serde_json::from_str(&first).unwrap();
    assert_eq!(v["status"], "attached");
    let spec = &v["spec"];
    assert_eq!(
        spec["tenant_id"],
        hex::encode(ids::tenant_id(&r.ids.project))
    );
    assert_eq!(
        spec["timeline_id"],
        hex::encode(ids::timeline_id(&r.ids.main))
    );
    assert_eq!(spec["operation_uuid"], id.to_string());
    assert_eq!(spec["endpoint_id"], r.ids.endpoint.to_string());
    assert_eq!(spec["mode"], "Primary");
    assert_eq!(spec["pageserver_connstring"], PAGESERVER_1);
    assert_eq!(spec["safekeeper_connstrings"][0], "loams-wal.pg.svc:5454");
    assert_eq!(spec["cluster"]["roles"][0]["name"], "app");
    assert!(spec["cluster"]["roles"][0]["encrypted_password"].is_null());
    assert_eq!(spec["cluster"]["databases"][0]["name"], "appdb");
    assert!(first.contains("\"work_mem\""), "{first}");
    assert_eq!(v["compute_ctl_config"]["jwks"], r.keys.jwks());

    // Same records, same bytes; a write the spec does not read changes
    // nothing.
    assert_eq!(r.spec(id, Some(&token)).await.1, first);
    r.put_compute(id, r.bound(), ComputeStatus::Running).await;
    assert_eq!(r.spec(id, Some(&token)).await.1, first);

    // The endpoint's settings change: so does the spec.
    let key = EndpointKey {
        project_id: r.ids.project.to_string(),
        id: r.ids.endpoint.to_string(),
    };
    let ep = r.store.get::<EndpointRec>(&key).await.unwrap().unwrap();
    let mut rec = ep.record.clone();
    rec.pg_settings.insert("work_mem".into(), "16MB".into());
    r.store
        .api_writer()
        .put(&rec, Some(ep.version))
        .await
        .unwrap();
    let (_, changed) = r.spec(id, Some(&token)).await;
    assert_ne!(changed, first);
    assert!(changed.contains("16MB"), "{changed}");
}

/// Postgres listens on every address of the compute, on the port its
/// runtime publishes (Task 10), whatever the endpoint's settings say.
#[tokio::test]
async fn spec_listens_on_the_compute_port() {
    let r = rig().await;
    let key = EndpointKey {
        project_id: r.ids.project.to_string(),
        id: r.ids.endpoint.to_string(),
    };
    let ep = r.store.get::<EndpointRec>(&key).await.unwrap().unwrap();
    let mut rec = ep.record.clone();
    rec.pg_settings.insert("port".into(), "5432".into());
    rec.pg_settings
        .insert("listen_addresses".into(), "localhost".into());
    r.store
        .api_writer()
        .put(&rec, Some(ep.version))
        .await
        .unwrap();
    let id = r.ids.compute;
    let (code, body) = r.spec(id, Some(&r.token(id))).await;
    assert_eq!(code, 200, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    let settings = v["spec"]["cluster"]["settings"].as_array().unwrap();
    let get = |name: &str| {
        let found: Vec<_> = settings.iter().filter(|s| s["name"] == name).collect();
        assert_eq!(found.len(), 1, "{name}: {found:?}");
        found[0]["value"].as_str().unwrap().to_owned()
    };
    assert_eq!(get("listen_addresses"), "*");
    assert_eq!(
        get("port"),
        loams_pg_control::runtime::COMPUTE_PG_PORT.to_string()
    );
}

/// A compute that is not its endpoint's current one, or has stopped, gets
/// no spec (it would be a second primary on the timeline).
#[tokio::test]
async fn spec_refused_for_replaced_or_stopped_compute() {
    let r = rig().await;
    let old = ComputeId::new();
    r.put_compute(old, r.bound(), ComputeStatus::Running).await;
    assert_eq!(r.spec(old, Some(&r.token(old))).await.0, 409);
    let id = r.ids.compute;
    r.put_compute(id, r.bound(), ComputeStatus::Stopped).await;
    assert_eq!(r.spec(id, Some(&r.token(id))).await.0, 409);
}

/// `notify-attach` records the pageserver on every branch of the tenant
/// and pushes the new spec to the tenant's running computes.
#[tokio::test]
async fn notify_attach_updates_shard_map_and_reconfigures() {
    let r = rig().await;
    let body = r.attach_body(7);
    assert_eq!(r.notify(None, &body).await.0, 401);
    assert_eq!(r.notify(Some("wrong-token"), &body).await.0, 401);
    assert!(r.ctl.seen.lock().unwrap().is_empty());

    let (status, text) = r.notify(Some(NOTIFY_TOKEN), &body).await;
    assert_eq!(status, 200, "{text}");
    for b in [r.ids.main, r.ids.dev] {
        let rec = r.branch(b).await;
        assert_eq!(
            rec.shards,
            vec![ShardAttachment {
                shard_number: 0,
                node_id: 7
            }]
        );
        assert_eq!(rec.stripe_size, None);
    }
    {
        let seen = r.ctl.seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "one running compute");
        assert_eq!(seen[0].0, r.ids.compute.to_string());
        assert_eq!(seen[0].1["spec"]["pageserver_connstring"], PAGESERVER_7);
        assert_eq!(seen[0].1["compute_ctl_config"]["jwks"], r.keys.jwks());
    }
    // The compute's next spec read names the new pageserver.
    let (_, spec) = r.spec(r.ids.compute, Some(&r.token(r.ids.compute))).await;
    assert!(spec.contains(PAGESERVER_7), "{spec}");

    // A compute that does not take it: recorded but not applied (423),
    // which the controller keeps and sends again.
    *r.ctl.fail.lock().unwrap() = true;
    let (status, _) = r.notify(Some(NOTIFY_TOKEN), &r.attach_body(1)).await;
    assert_eq!(status, 423);
    assert_eq!(r.branch(r.ids.main).await.shards[0].node_id, 1);

    // An unknown tenant, and a malformed body.
    let mut unknown = r.attach_body(1);
    unknown["tenant_id"] = Value::String(hex::encode([9u8; 16]));
    assert_eq!(r.notify(Some(NOTIFY_TOKEN), &unknown).await.0, 404);
    let mut bad = r.attach_body(1);
    bad["shards"] = serde_json::json!([]);
    assert_eq!(r.notify(Some(NOTIFY_TOKEN), &bad).await.0, 400);
}

/// The storage controller manages no safekeepers (§46 §9.1), so
/// `notify-safekeepers` is not a route.
#[tokio::test]
async fn notify_safekeepers_not_served() {
    let r = rig().await;
    let body = serde_json::json!({
        "tenant_id": hex::encode(ids::tenant_id(&r.ids.project)),
        "timeline_id": hex::encode(ids::timeline_id(&r.ids.main)),
        "generation": 1,
        "safekeepers": [{ "id": 1, "hostname": "sk1" }],
    });
    let resp = r
        .http
        .put(format!("{}/notify-safekeepers", r.base))
        .bearer_auth(NOTIFY_TOKEN)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 404);
}

/// The upcall listener is never the API listener: binding it on the API
/// address (or a wildcard that covers it) is refused, and without a
/// storage-controller token `notify-attach` is not served at all.
#[tokio::test]
async fn upcall_not_served_on_api_listener() {
    let store = factory().store(common::options()).await.unwrap();
    let keys = Arc::new(ComputeKeys::generate().unwrap());
    let ctl: Arc<FakeCtl> = Arc::new(FakeCtl::default());
    let api = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let api_addr = api.local_addr().unwrap();
    for listen in [
        api_addr,
        SocketAddr::new("0.0.0.0".parse().unwrap(), api_addr.port()),
    ] {
        let mut cfg = config(listen);
        cfg.api_listen = Some(api_addr);
        let refused = UpcallServer::bind(store.clone(), keys.clone(), ctl.clone(), cfg).await;
        assert!(
            matches!(refused, Err(UpcallError::SharesApiListener { .. })),
            "{listen}: {:?}",
            refused.err()
        );
    }

    let r = rig_with(|c| c.notify_token = None).await;
    let (status, _) = r.notify(Some(NOTIFY_TOKEN), &r.attach_body(7)).await;
    assert_eq!(status, 404);
}

/// Requests are bounded: a body over the limit, a spec flood from one peer
/// and repeated bad tokens are refused before any work.
#[tokio::test]
async fn upcall_bounds_sizes_and_rates() {
    let r = rig_with(|c| {
        c.limits.spec_rate.burst = 3;
        c.limits.spec_rate.per_second = 0.01;
        c.limits.auth_failure_rate.burst = 2;
        c.limits.auth_failure_rate.per_second = 0.01;
    })
    .await;
    let big = serde_json::json!({ "pad": "x".repeat(200 * 1024) });
    assert_eq!(r.notify(Some(NOTIFY_TOKEN), &big).await.0, 413);

    let id = r.ids.compute;
    let token = r.token(id);
    for _ in 0..3 {
        assert_eq!(r.spec(id, Some(&token)).await.0, 200);
    }
    // compute_ctl retries a 503, not a 429.
    assert_eq!(r.spec(id, Some(&token)).await.0, 503);

    for _ in 0..2 {
        assert_eq!(r.notify(Some("bad"), &r.attach_body(1)).await.0, 401);
    }
    assert_eq!(r.notify(Some("bad"), &r.attach_body(1)).await.0, 429);
}

/// Neither the compute's token nor the controller's appears in a log line,
/// and the keys print no key material.
#[tokio::test]
async fn upcall_logs_no_token() {
    let logs = common::captured_logs();
    let r = rig().await;
    let id = r.ids.compute;
    let token = r.token(id);
    r.spec(id, Some(&token)).await;
    r.spec(ComputeId::new(), Some(&token)).await;
    r.notify(Some(NOTIFY_TOKEN), &r.attach_body(7)).await;
    r.notify(Some("bad-token-sentinel"), &r.attach_body(7))
        .await;
    let text = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
    for secret in [token.as_str(), NOTIFY_TOKEN, "bad-token-sentinel"] {
        assert!(!text.contains(secret), "a token was logged");
    }
    let shown = format!("{:?}", r.keys);
    let der = hex::encode(r.keys.pkcs8().expose());
    assert!(!shown.contains(&der), "{shown}");
    assert!(shown.contains(r.keys.kid()));
}

/// The wall clock, in ms: `compute_ctl` checks `exp` against it.
fn wall_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

/// A free loopback port.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// The pinned `compute_ctl` (`compute-node-v17`, Podman or Docker, host
/// network) reads its spec through the upcall with its token, takes the
/// `empty` answer and the JWKS in it, and then accepts `pg-control`'s
/// `compute_ctl` token: `/status` answers `empty`. Then `/configure` with an
/// attached spec built from the records is parsed (no storage runs, so the
/// start itself fails, but not with a 400).
///
/// `COMPUTE_IMAGE` overrides the image; `CONTAINER_BIN` the runtime.
#[tokio::test]
#[ignore = "needs podman or docker and the pinned compute-node-v17 image"]
async fn it_compute_ctl_reads_the_spec_upcall() {
    let image = std::env::var("COMPUTE_IMAGE").unwrap_or_else(|_| {
        "ghcr.io/neondatabase/compute-node-v17@sha256:13ab146d3e7bbabb25a8532f315ac443e7512351d1ede0bab586def5c70e26c3".into()
    });
    let bin = std::env::var("CONTAINER_BIN").unwrap_or_else(|_| "podman".into());
    let r = rig().await;
    let pool = ComputeId::new();
    r.put_compute(pool, None, ComputeStatus::Pending).await;
    let (ext, int, pg) = (free_port(), free_port(), free_port());
    let name = format!("loams-upcall-it-{}", pool.to_string().to_lowercase());
    let started = std::process::Command::new(&bin)
        .args([
            "run",
            "-d",
            "--rm",
            "--pull=never",
            "--network=host",
            "--name",
            &name,
        ])
        .args(["-e", &format!("NEON_CONTROL_PLANE_TOKEN={}", r.token(pool))])
        .args(["--entrypoint", "/usr/local/bin/compute_ctl", &image])
        .args([
            "--pgdata",
            "/var/db/postgres/compute",
            "-b",
            "/usr/local/bin/postgres",
        ])
        .args([
            "-C",
            &format!("postgresql://cloud_admin@127.0.0.1:{pg}/postgres"),
        ])
        .args(["--compute-id", &pool.to_string()])
        .args(["-p", &r.base])
        .args(["--external-http-port", &ext.to_string()])
        .args(["--internal-http-port", &int.to_string()])
        .status()
        .unwrap();
    assert!(started.success(), "{bin} run");
    struct Remove(String, String);
    impl Drop for Remove {
        fn drop(&mut self) {
            let _ = std::process::Command::new(&self.0)
                .args(["rm", "-f", &self.1])
                .status();
        }
    }
    let _remove = Remove(bin.clone(), name.clone());

    let ctl_token = r
        .keys
        .issue_compute_ctl_token(&pool, wall_ms(), Duration::from_secs(600))
        .unwrap();
    let status_url = format!("http://127.0.0.1:{ext}/status");
    let mut last = String::new();
    for _ in 0..120 {
        if let Ok(resp) = r
            .http
            .get(&status_url)
            .bearer_auth(ctl_token.expose())
            .send()
            .await
        {
            let code = resp.status().as_u16();
            last = format!("{code} {}", resp.text().await.unwrap_or_default());
            if code == 200 && last.contains("\"empty\"") {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(
        last.starts_with("200") && last.contains("\"empty\""),
        "{last}"
    );
    // Another key's token is refused by compute_ctl: the JWKS is ours.
    let stranger = ComputeKeys::generate()
        .unwrap()
        .issue_compute_ctl_token(&pool, wall_ms(), Duration::from_secs(600))
        .unwrap();
    let refused = r
        .http
        .get(&status_url)
        .bearer_auth(stranger.expose())
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status().as_u16(), 401);

    // Bind it, and hand it the attached spec through /configure.
    r.put_compute(pool, r.bound(), ComputeStatus::Pending).await;
    let ep_key = EndpointKey {
        project_id: r.ids.project.to_string(),
        id: r.ids.endpoint.to_string(),
    };
    let ep = r.store.get::<EndpointRec>(&ep_key).await.unwrap().unwrap();
    let mut rec = ep.record.clone();
    rec.compute_id = Some(pool.to_string());
    r.store
        .api_writer()
        .put(&rec, Some(ep.version))
        .await
        .unwrap();
    let (code, body) = r.spec(pool, Some(&r.token(pool))).await;
    assert_eq!(code, 200, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    let configure = r
        .http
        .post(format!("http://127.0.0.1:{ext}/configure"))
        .bearer_auth(ctl_token.expose())
        .json(&serde_json::json!({
            "spec": v["spec"],
            "compute_ctl_config": v["compute_ctl_config"],
        }))
        .timeout(Duration::from_secs(20))
        .send()
        .await;
    match configure {
        Ok(resp) => {
            let code = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            assert_ne!(code, 400, "compute_ctl refused the spec: {text}");
            assert_ne!(code, 401, "{text}");
        }
        // Still starting (and failing) without storage: accepted.
        Err(e) => assert!(e.is_timeout(), "{e}"),
    }
}

/// Injects `fault` at `point` of the first `pg.batch` transaction only, and
/// signals `reached` when it does.
struct FirstBatchOnce {
    point: loams_kv::FaultPoint,
    fault: loams_kv::Fault,
    fired: std::sync::atomic::AtomicBool,
    reached: Arc<tokio::sync::Notify>,
}

impl loams_kv::FaultPlan for FirstBatchOnce {
    fn at(&self, op: &str, point: loams_kv::FaultPoint, _: u32) -> Option<loams_kv::Fault> {
        if op != "pg.batch" || point != self.point {
            return None;
        }
        if self.fired.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return None;
        }
        self.reached.notify_one();
        Some(self.fault)
    }
}

/// A rig whose store faults its first batch: the shard-map write.
async fn faulty_rig(
    point: loams_kv::FaultPoint,
    fault: loams_kv::Fault,
    commit_tokens: bool,
) -> (Rig, Arc<tokio::sync::Notify>) {
    let kv = factory().kv().await.unwrap();
    let reached = Arc::new(tokio::sync::Notify::new());
    let faults = FirstBatchOnce {
        point,
        fault,
        fired: std::sync::atomic::AtomicBool::new(false),
        reached: reached.clone(),
    };
    let options = loams_pg_control::StoreOptions {
        commit_tokens,
        ..common::options()
    };
    let store = KvControlStore::new(kv.with_faults(Arc::new(faults)), options);
    (rig_on(store, |_| {}).await, reached)
}

/// The shard-map write under faults (CodeRabbit, PR #417):
/// - a transaction conflict at commit is redone by the store's runner: 200;
/// - a branch written between the notification's read and its batch makes
///   the batch's compare-and-set fail; `record` reads again and writes
///   both changes: 200, and the concurrent change is kept;
/// - a lost acknowledgement (no commit tokens) is 503; the controller's
///   retry finds the map already written, writes nothing and answers 200.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notify_attach_shard_map_write_survives_faults() {
    use loams_kv::{Fault, FaultPoint};

    // A transaction conflict at commit.
    let (r, _) = faulty_rig(FaultPoint::BeforeCommit, Fault::Conflict, true).await;
    let (status, text) = r.notify(Some(NOTIFY_TOKEN), &r.attach_body(7)).await;
    assert_eq!(status, 200, "{text}");
    assert_eq!(r.branch(r.ids.main).await.shards[0].node_id, 7);

    // A concurrent write between the read and the batch.
    let (r, reached) = faulty_rig(
        FaultPoint::BeforeBegin,
        Fault::Delay(Duration::from_secs(2)),
        true,
    )
    .await;
    let r = Arc::new(r);
    let notify = {
        let r = r.clone();
        tokio::spawn(async move { r.notify(Some(NOTIFY_TOKEN), &r.attach_body(7)).await })
    };
    reached.notified().await;
    let key = BranchKey {
        project_id: r.ids.project.to_string(),
        id: r.ids.dev.to_string(),
    };
    let dev = r.store.get::<BranchRec>(&key).await.unwrap().unwrap();
    let mut rec = dev.record.clone();
    rec.protected = true;
    r.store
        .api_writer()
        .put(&rec, Some(dev.version))
        .await
        .unwrap();
    let (status, text) = notify.await.unwrap();
    assert_eq!(status, 200, "{text}");
    let dev = r.branch(r.ids.dev).await;
    assert!(dev.protected, "the concurrent change is kept");
    assert_eq!(dev.shards[0].node_id, 7);
    assert_eq!(r.branch(r.ids.main).await.shards[0].node_id, 7);

    // A lost acknowledgement after the commit.
    let (r, _) = faulty_rig(FaultPoint::AfterCommit, Fault::LoseAck, false).await;
    let (status, text) = r.notify(Some(NOTIFY_TOKEN), &r.attach_body(7)).await;
    assert_eq!(status, 503, "{text}");
    let main_key = BranchKey {
        project_id: r.ids.project.to_string(),
        id: r.ids.main.to_string(),
    };
    let written = r.store.get::<BranchRec>(&main_key).await.unwrap().unwrap();
    assert_eq!(written.record.shards[0].node_id, 7, "the batch applied");
    let (status, text) = r.notify(Some(NOTIFY_TOKEN), &r.attach_body(7)).await;
    assert_eq!(status, 200, "{text}");
    let again = r.store.get::<BranchRec>(&main_key).await.unwrap().unwrap();
    assert_eq!(again.version, written.version, "the retry wrote nothing");
}

/// Task 24: a role carries its password's SCRAM-SHA-256 verifier, never
/// the password; the same verifier for the same records; a reset (a new
/// reference and secret) gives a new one, which a compute takes on its next
/// spec or `/configure`; a role whose secret is lost gets none.
#[tokio::test]
async fn spec_carries_scram_verifier_never_password() {
    use loams_pg_control::model::RoleKey;
    use loams_pg_control::scram::{self, RoleVerifiers};
    use loams_pg_control::secrets::SecretStore;

    let secrets = Arc::new(common::MemorySecrets::default());
    let verifiers = Arc::new(RoleVerifiers::new(secrets.clone()));
    let r = rig_with(|c| c.verifiers = Some(verifiers.clone())).await;
    let key = RoleKey {
        branch_id: r.ids.main.to_string(),
        name: "app".into(),
    };
    let role = r.store.get::<RoleRec>(&key).await.unwrap().unwrap();
    let password = "pw-sentinel-0123456789abcdefghijklmnopqrstu";
    secrets
        .put(
            &role.record.secret_ref,
            Secret::new(password.as_bytes().to_vec()),
        )
        .await
        .unwrap();

    let id = r.ids.compute;
    let token = r.token(id);
    let (status, first) = r.spec(id, Some(&token)).await;
    assert_eq!(status, 200, "{first}");
    assert!(!first.contains(password), "the password is in the spec");
    let v: Value = serde_json::from_str(&first).unwrap();
    let got = v["spec"]["cluster"]["roles"][0]["encrypted_password"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(got.starts_with("SCRAM-SHA-256$4096:"), "{got}");
    assert_eq!(
        got,
        scram::role_verifier(
            password.as_bytes(),
            verifiers.salt_key().await.unwrap(),
            &role.record.secret_ref
        )
    );
    // Stable: the same records give the same bytes.
    assert_eq!(r.spec(id, Some(&token)).await.1, first);

    // A reset: a new reference and password; the old secret is deleted.
    let new_password = "pw-sentinel-rotated-0123456789abcdefghijklm";
    let mut rec = role.record.clone();
    rec.secret_ref = SecretRef::new_role(&r.ids.project);
    secrets
        .put(
            &rec.secret_ref,
            Secret::new(new_password.as_bytes().to_vec()),
        )
        .await
        .unwrap();
    r.store
        .api_writer()
        .put(&rec, Some(role.version))
        .await
        .unwrap();
    secrets.delete(&role.record.secret_ref).await.unwrap();
    let (status, second) = r.spec(id, Some(&token)).await;
    assert_eq!(status, 200, "{second}");
    assert!(!second.contains(password) && !second.contains(new_password));
    let v: Value = serde_json::from_str(&second).unwrap();
    assert_eq!(
        v["spec"]["cluster"]["roles"][0]["encrypted_password"],
        scram::role_verifier(
            new_password.as_bytes(),
            verifiers.salt_key().await.unwrap(),
            &rec.secret_ref
        )
    );
    assert_ne!(second, first);

    // A role whose secret is lost (its record still names it) gets no
    // password, and the spec is still served.
    let r2 = rig_with(|c| c.verifiers = Some(verifiers.clone())).await;
    let id = r2.ids.compute;
    let (status, third) = r2.spec(id, Some(&r2.token(id))).await;
    assert_eq!(status, 200, "{third}");
    let v: Value = serde_json::from_str(&third).unwrap();
    assert!(v["spec"]["cluster"]["roles"][0]["encrypted_password"].is_null());
}

// ---- the pageserver's timeline import status (NF2 Task 13) ----

const PAGESERVER_TOKEN: &str = "pageserver-upcall-token-for-tests";

impl Rig {
    async fn import_status(
        &self,
        method: reqwest::Method,
        bearer: Option<&str>,
        body: &Value,
    ) -> (u16, String) {
        let mut req = self
            .http
            .request(
                method,
                format!("{}/upcall/v1/timeline_import_status", self.base),
            )
            .json(body);
        if let Some(t) = bearer {
            req = req.bearer_auth(t);
        }
        let resp = req.send().await.unwrap();
        (resp.status().as_u16(), resp.text().await.unwrap())
    }
}

async fn import_rig() -> Rig {
    rig_with(|c| c.pageserver_token = Some(Secret::new(PAGESERVER_TOKEN.into()))).await
}

fn shard_body(status: Option<Value>) -> Value {
    let mut b = serde_json::json!({
        "tenant_shard_id": "0f3a3b1c2d4e5f60718293a4b5c6d7e8",
        "timeline_id": "a1b2c3d4e5f60718293a4b5c6d7e8f90",
        "generation": 1,
    });
    if let Some(s) = status {
        b["status"] = s;
    }
    b
}

/// A pageserver's import reports are recorded and answered back: an import
/// not reported yet is in progress, a reported one is what was put, and a
/// repeated put (at-least-once delivery) is fine.
#[tokio::test]
async fn timeline_import_status_round_trip() {
    let r = import_rig().await;
    let get = reqwest::Method::GET;
    let post = reqwest::Method::POST;
    let (code, body) = r
        .import_status(get.clone(), Some(PAGESERVER_TOKEN), &shard_body(None))
        .await;
    assert_eq!(code, 200, "{body}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap(),
        serde_json::json!({"InProgress": null})
    );

    let progress = serde_json::json!({"InProgress": {"V1": {"jobs": 4, "completed": 1, "import_source_hash": 7}}});
    for status in [
        progress.clone(),
        progress.clone(),
        Value::String("Done".into()),
        Value::String("Done".into()),
    ] {
        let (code, body) = r
            .import_status(
                post.clone(),
                Some(PAGESERVER_TOKEN),
                &shard_body(Some(status.clone())),
            )
            .await;
        assert_eq!(code, 200, "{body}");
        let (_, got) = r
            .import_status(get.clone(), Some(PAGESERVER_TOKEN), &shard_body(None))
            .await;
        assert_eq!(serde_json::from_str::<Value>(&got).unwrap(), status);
    }
    let rec = r
        .store
        .get::<loams_pg_control::model::ImportStatusRec>(
            &loams_pg_control::model::ImportStatusKey {
                tenant_shard_id: "0f3a3b1c2d4e5f60718293a4b5c6d7e8".into(),
                timeline_id: "a1b2c3d4e5f60718293a4b5c6d7e8f90".into(),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rec.record.state, loams_pg_control::model::ImportState::Done);
}

/// The routes need the pageservers' token, and are not served without one.
#[tokio::test]
async fn timeline_import_status_needs_the_pageserver_token() {
    let r = import_rig().await;
    let body = shard_body(Some(Value::String("Done".into())));
    let (code, _) = r
        .import_status(reqwest::Method::POST, Some("wrong"), &body)
        .await;
    assert_eq!(code, 401);
    let (code, _) = r.import_status(reqwest::Method::POST, None, &body).await;
    assert_eq!(code, 401);
    // The notify-attach token is not the pageservers'.
    let (code, _) = r
        .import_status(reqwest::Method::POST, Some(NOTIFY_TOKEN), &body)
        .await;
    assert_eq!(code, 401);

    let r = rig().await;
    let (code, _) = r
        .import_status(reqwest::Method::POST, Some(PAGESERVER_TOKEN), &body)
        .await;
    assert_eq!(
        code, 404,
        "no pageserver token configured: the route is not served"
    );
}

/// An error report is kept with its message; a status that is not a
/// ShardImportStatus is refused.
#[tokio::test]
async fn timeline_import_status_error_and_garbage() {
    let r = import_rig().await;
    let err = serde_json::json!({"Error": "the bucket is unreadable"});
    let (code, body) = r
        .import_status(
            reqwest::Method::POST,
            Some(PAGESERVER_TOKEN),
            &shard_body(Some(err)),
        )
        .await;
    assert_eq!(code, 200, "{body}");
    let rec = r
        .store
        .get::<loams_pg_control::model::ImportStatusRec>(
            &loams_pg_control::model::ImportStatusKey {
                tenant_shard_id: "0f3a3b1c2d4e5f60718293a4b5c6d7e8".into(),
                timeline_id: "a1b2c3d4e5f60718293a4b5c6d7e8f90".into(),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        rec.record.state,
        loams_pg_control::model::ImportState::Error("the bucket is unreadable".into())
    );
    let (code, _) = r
        .import_status(
            reqwest::Method::POST,
            Some(PAGESERVER_TOKEN),
            &shard_body(Some(serde_json::json!({"Nope": 1}))),
        )
        .await;
    assert_eq!(code, 400);
}

// ---- the pageserver's generation validation (NF2 Task 13, part 2) ----

impl Rig {
    async fn validate(&self, bearer: Option<&str>, body: &Value) -> (u16, String) {
        let mut req = self
            .http
            .post(format!("{}/upcall/v1/validate", self.base))
            .json(body);
        if let Some(t) = bearer {
            req = req.bearer_auth(t);
        }
        let resp = req.send().await.unwrap();
        (resp.status().as_u16(), resp.text().await.unwrap())
    }
}

/// The pageserver's deletion queue validates each tenant's generation
/// before it deletes layers or reports a remote consistent LSN. Loams runs
/// one pageserver per tenant shard, attached at its last known generation
/// (emergency mode), so every generation it asks about is current; before
/// this route its control_plane_api was a junk address and validation
/// retried forever.
#[tokio::test]
async fn pageserver_generations_validate() {
    let r = import_rig().await;
    let body = serde_json::json!({"tenants": [
        {"id": "0f3a3b1c2d4e5f60718293a4b5c6d7e8", "gen": 1},
        {"id": "1f3a3b1c2d4e5f60718293a4b5c6d7e8-0004", "gen": 7},
    ]});
    let (code, text) = r.validate(Some(PAGESERVER_TOKEN), &body).await;
    assert_eq!(code, 200, "{text}");
    assert_eq!(
        serde_json::from_str::<Value>(&text).unwrap(),
        serde_json::json!({"tenants": [
            {"id": "0f3a3b1c2d4e5f60718293a4b5c6d7e8", "valid": true},
            {"id": "1f3a3b1c2d4e5f60718293a4b5c6d7e8-0004", "valid": true},
        ]})
    );
    let (code, _) = r.validate(Some("wrong"), &body).await;
    assert_eq!(code, 401);
    let (code, _) = r
        .validate(Some(PAGESERVER_TOKEN), &serde_json::json!({"nope": 1}))
        .await;
    assert_eq!(code, 400);
    let r = rig().await;
    let (code, _) = r.validate(Some(PAGESERVER_TOKEN), &body).await;
    assert_eq!(code, 404, "no pageserver token: not served");
}
