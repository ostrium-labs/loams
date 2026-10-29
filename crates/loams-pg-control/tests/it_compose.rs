//! The compose runtime end to end (PG2 Task 10): a compute started by
//! [`ComposeRuntime`] on `deploy/loams-postgres-dev`'s network reads its spec
//! from the upcall listener, starts against the stack's pageserver and
//! safekeeper, and answers `SELECT 1` on its endpoint's address. Ignored by
//! default; `pg2-e2e.yml` runs it:
//!
//! ```text
//! (cd deploy/loams-postgres-dev && docker compose up -d rustfs create-bucket storage_broker pageserver safekeeper1)
//! cargo test -p loams-pg-control --test it_compose -- --ignored
//! ```
//!
//! The stack's `safekeeper1` takes the compute's WAL: the computes are on
//! the stack's network, and `loams-wal` replaces it there in Task 43.
//!
//! Settings (environment): `NEON_PAGESERVER` (the management API from the
//! host, `http://127.0.0.1:9898`); `IT_COMPOSE_NETWORK` (the stack's
//! network, `loams-neon_default`); `IT_COMPOSE_PAGESERVER` (the pageserver
//! from the network, `postgresql://no_user@pageserver:6400`) and
//! `IT_PAGESERVER_ID` (`1234`, `identity.toml`'s); `IT_COMPOSE_SAFEKEEPERS`
//! (`safekeeper1:5454`); `IT_UPCALL_HOST` (the host as the computes reach
//! it, `host.containers.internal` under Podman; `host.docker.internal` with
//! `IT_EXTRA_HOSTS=host.docker.internal:host-gateway` under Docker);
//! `COMPUTE_IMAGE` (default: the pin file's `compute-v<default>`).
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use loams_pg_control::StoreOptions;
use loams_pg_control::ids::{self, BranchId, ComputeId, EndpointId, ProjectId};
use loams_pg_control::model::{
    BranchRec, BranchState, ComputeBinding, ComputeRec, ComputeStatus, Cu, DesiredState,
    EndpointRec, EndpointState, EndpointType, PoolMode, ProjectRec, ProjectState, WalService,
};
use loams_pg_control::runtime::compose::{
    ComposeBin, ComposeConfig, ComposeRuntime, TokioCommandRunner,
};
use loams_pg_control::runtime::{ComputeLaunch, ComputeRuntime, RuntimeStatus};
use loams_pg_control::service::Clock;
use loams_pg_control::store::conformance::local_factory;
use loams_pg_control::upcall::{
    COMPUTE_TOKEN_TTL, ComputeCtlConfigurer, ComputeKeys, UpcallConfig, UpcallServer,
};
use loams_postgres::compute_ctl::{ComputeCtlClient, ComputeStatus as CtlStatus};
use loams_postgres::pageserver::{NeonClient, NeonEndpoints, TenantConfig, TimelineCreate};
use loams_postgres::{TenantId, TimelineId};

const NS: &str = "it";

fn env(var: &str, default: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| default.into())
}

fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

/// The pin file's compute image for its default major.
fn pinned_compute_image() -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let text =
        std::fs::read_to_string(root.join("deploy/pins/loams-postgres-images.toml")).unwrap();
    let pins: toml::Value = toml::from_str(&text).unwrap();
    let major = pins["majors"]["default"].as_integer().unwrap();
    let image = &pins["images"][format!("compute-v{major}")];
    format!(
        "{}@{}",
        image["repository"].as_str().unwrap(),
        image["digest"].as_str().unwrap()
    )
}

/// Removes the namespace's compose project when the test ends, however it
/// ends.
struct Down(ComposeBin, PathBuf);

impl Drop for Down {
    fn drop(&mut self) {
        let _ = std::process::Command::new(&self.0.program)
            .args(&self.0.args)
            .args(["-p", &format!("loams-pg-{NS}"), "down", "--timeout", "5"])
            .current_dir(&self.1)
            .status();
    }
}

/// `compose <args>` on the namespace's project; its stdout, or a panic with
/// its stderr.
fn compose(bin: &ComposeBin, dir: &Path, args: &[&str]) -> String {
    let file = dir.join(format!("{NS}.yaml"));
    let out = std::process::Command::new(&bin.program)
        .args(&bin.args)
        .args([
            "-f",
            &file.display().to_string(),
            "-p",
            &format!("loams-pg-{NS}"),
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "compose {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs deploy/loams-postgres-dev running, and docker or podman"]
async fn it_compose_compute_answers_select_1() {
    let bin = ComposeBin::from_path().expect("docker, podman or docker-compose on PATH");
    let project = ProjectId::new();
    let main = BranchId::new();
    let endpoint = EndpointId::new();
    let compute = ComputeId::new();
    let tenant = TenantId(ids::tenant_id(&project));
    let timeline = TimelineId(ids::timeline_id(&main));

    // Storage: the tenant and its first timeline, as the reconciler makes
    // them (Task 7).
    let neon = NeonClient::new(
        NeonEndpoints {
            pageserver: env("NEON_PAGESERVER", "http://127.0.0.1:9898")
                .parse()
                .unwrap(),
            storcon: None,
        },
        None,
    )
    .unwrap();
    neon.attach_tenant(tenant, 1, &TenantConfig::default())
        .await
        .unwrap();
    neon.create_timeline(
        tenant,
        &TimelineCreate {
            new_timeline_id: timeline,
            ancestor: None,
            ancestor_start_lsn: None,
            pg_version: Some(17),
        },
    )
    .await
    .unwrap();

    // The records, and the upcall listener on every address of the host.
    let dir = tempfile::Builder::new()
        .prefix("it-compose-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .unwrap();
    let store = local_factory(dir.path())
        .store(StoreOptions::default())
        .await
        .unwrap();
    let keys = Arc::new(ComputeKeys::generate().unwrap());
    let pageserver_id: u64 = env("IT_PAGESERVER_ID", "1234").parse().unwrap();
    let upcall = UpcallServer::bind(
        store.clone(),
        keys.clone(),
        Arc::new(ComputeCtlConfigurer::new(keys.clone(), Clock::system())),
        UpcallConfig {
            pageservers: BTreeMap::from([(
                pageserver_id,
                env(
                    "IT_COMPOSE_PAGESERVER",
                    "postgresql://no_user@pageserver:6400",
                ),
            )]),
            default_pageserver: Some(pageserver_id),
            wal_pools: BTreeMap::from([(
                "default".to_owned(),
                env("IT_COMPOSE_SAFEKEEPERS", "safekeeper1:5454")
                    .split(',')
                    .map(str::to_owned)
                    .collect(),
            )]),
            ..UpcallConfig::defaults("0.0.0.0:0".parse().unwrap())
        },
    )
    .await
    .unwrap();
    let upcall_port = upcall.local_addr().port();
    let (stop_upcall, stopped) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(upcall.serve(async move {
        let _ = stopped.await;
    }));

    let now = now_ms();
    let api = store.api_writer();
    api.put(
        &ProjectRec {
            namespace: NS.into(),
            id: project.to_string(),
            name: "it".into(),
            tenant_id: tenant.0,
            pg_version: 17,
            wal: WalService::LoamsWal {
                pool: "default".into(),
            },
            history_retention_s: 24 * 3600,
            region: "local".into(),
            default_branch_id: Some(main.to_string()),
            settings: BTreeMap::new(),
            state: ProjectState::Ready,
            created_at_ms: now,
            updated_at_ms: now,
        },
        None,
    )
    .await
    .unwrap();
    api.put(
        &BranchRec {
            project_id: project.to_string(),
            id: main.to_string(),
            name: "main".into(),
            timeline_id: timeline.0,
            parent_id: None,
            ancestor_lsn: None,
            expires_at_ms: None,
            protected: false,
            stripe_size: None,
            shards: Vec::new(),
            state: BranchState::Ready,
            created_at_ms: now,
            parent_time_ms: None,
            updated_at_ms: now,
            pg_version: 17,
        },
        None,
    )
    .await
    .unwrap();
    let mut ep = EndpointRec {
        project_id: project.to_string(),
        id: endpoint.to_string(),
        branch_id: main.to_string(),
        kind: EndpointType::ReadWrite,
        min_cu: Cu(4),
        max_cu: Cu(4),
        suspend_timeout_s: 0,
        pool_mode: PoolMode::Transaction,
        pg_settings: BTreeMap::new(),
        desired: DesiredState::Running,
        state: EndpointState::Starting,
        compute_id: Some(compute.to_string()),
        backend_addr: None,
        failure: None,
        ..EndpointRec::new("", "", "", EndpointType::ReadWrite)
    };

    // The runtime: the address, then the compute.
    let compose_dir = dir.path().join("pg").join("compose");
    let host = env("IT_UPCALL_HOST", "host.containers.internal");
    let rt = ComposeRuntime::open(
        ComposeConfig {
            network: Some(env("IT_COMPOSE_NETWORK", "loams-neon_default")),
            extra_hosts: std::env::var("IT_EXTRA_HOSTS")
                .map(|h| h.split(',').map(str::to_owned).collect())
                .unwrap_or_default(),
            ..ComposeConfig::new(
                bin.clone(),
                compose_dir.clone(),
                format!("http://{host}:{upcall_port}"),
            )
        },
        Arc::new(TokioCommandRunner),
    )
    .unwrap();
    let _down = Down(bin.clone(), dir.path().to_path_buf());
    let addr = rt.ensure_address(NS, &ep).await.unwrap();
    ep.backend_addr = Some(addr.to_string());
    api.put(&ep, None).await.unwrap();
    api.put(
        &ComputeRec {
            id: compute.to_string(),
            binding: Some(ComputeBinding {
                namespace: NS.into(),
                project_id: project.to_string(),
                endpoint_id: endpoint.to_string(),
            }),
            pool: None,
            bound_at_ms: None,
            spec_version: 0,
            status: ComputeStatus::Pending,
            jwt_key_id: Some(keys.kid().to_owned()),
            ctl_url: None,
            cu: Cu(4),
            created_at_ms: now,
            last_active_ms: None,
        },
        None,
    )
    .await
    .unwrap();
    let image = std::env::var("COMPUTE_IMAGE").unwrap_or_else(|_| pinned_compute_image());
    let launch = ComputeLaunch {
        compute_id: compute,
        namespace: NS.into(),
        binding: Some(ComputeBinding {
            namespace: NS.into(),
            project_id: project.to_string(),
            endpoint_id: endpoint.to_string(),
        }),
        address: Some(addr.clone()),
        image,
        cu: Cu(4),
        upcall_token: keys
            .issue_upcall_token(&compute, now, COMPUTE_TOKEN_TTL)
            .unwrap(),
    };
    let handle = rt.start(&launch).await.unwrap();
    rt.bind_address(NS, &ep, &handle).await.unwrap();
    assert_eq!(
        rt.start(&launch).await.unwrap(),
        handle,
        "start is idempotent"
    );

    // compute_ctl reads its spec through the upcall and starts Postgres.
    let service = compute.to_string().to_lowercase();
    let ctl = ComputeCtlClient::new(
        handle.ctl_url.parse().unwrap(),
        keys.issue_compute_ctl_token(&compute, now_ms(), Duration::from_secs(600))
            .unwrap(),
    )
    .unwrap();
    let mut last = String::new();
    let mut running = false;
    for _ in 0..360 {
        match ctl.status().await {
            Ok(s) if s.status == CtlStatus::Running => {
                running = true;
                break;
            }
            Ok(s) if s.status == CtlStatus::Failed => {
                last = format!("{s:?}");
                break;
            }
            Ok(s) => last = format!("{s:?}"),
            Err(e) => last = e.to_string(),
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    if !running {
        let logs = compose(&bin, &compose_dir, &["logs", "--no-color", &service]);
        panic!("compute_ctl never ran: {last}\n{logs}");
    }

    // A login role, made inside the compute (role passwords in the spec
    // are Task 24's), then SELECT 1 through the endpoint's address.
    let password = format!("it-{}", now_ms());
    compose(
        &bin,
        &compose_dir,
        &[
            "exec",
            "-T",
            &service,
            "/usr/local/bin/psql",
            "-h",
            "127.0.0.1",
            "-p",
            "55433",
            "-U",
            "cloud_admin",
            "-d",
            "postgres",
            "-v",
            "ON_ERROR_STOP=1",
            "-c",
            &format!("CREATE ROLE it_select LOGIN PASSWORD '{password}'"),
        ],
    );
    let (client, conn) = tokio_postgres::connect(
        &format!(
            "host={} port={} user=it_select password={password} dbname=postgres \
             connect_timeout=10",
            addr.host, addr.port
        ),
        tokio_postgres::NoTls,
    )
    .await
    .unwrap();
    tokio::spawn(conn);
    let one: i32 = client.query_one("SELECT 1", &[]).await.unwrap().get(0);
    assert_eq!(one, 1);
    drop(client);

    assert_eq!(
        rt.status(NS, &compute).await.unwrap(),
        RuntimeStatus::Running
    );
    rt.stop(NS, &compute).await.unwrap();
    assert_eq!(
        rt.status(NS, &compute).await.unwrap(),
        RuntimeStatus::Missing
    );
    rt.remove_address(NS, &ep).await.unwrap();
    let _ = stop_upcall.send(());
}
