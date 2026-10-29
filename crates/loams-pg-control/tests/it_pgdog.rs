//! The config push against the pinned PgDog (PG2 Task 21), the compose way:
//! [`DirSink`] writes the namespace's files into the directory PgDog's
//! container mounts, and [`PgDogAdminClient`] sends `RELOAD` through its
//! admin database. A change to the records then shows in `SHOW POOLS`
//! without a restart. Ignored by default (it starts a container):
//!
//! ```text
//! IT_CONTAINER_ENGINE=podman cargo test -p loams-pg-control --test it_pgdog -- --ignored
//! ```
//!
//! `IT_CONTAINER_ENGINE` is `docker` by default. The image is the pin in
//! `deploy/pgdog/compose.yaml` (Task 18), unmodified.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use loams_pg_control::PgControlStore;
use loams_pg_control::ids::ProjectId;
use loams_pg_control::model::{
    BranchRec, BranchState, Cu, EndpointRec, EndpointType, ProjectRec, ProjectState, RoleRec,
    WalService, project_lease,
};
use loams_pg_control::pgdog::admin::{AdminLogin, PgDogAdminClient, StaticDirectory};
use loams_pg_control::pgdog::push::{CredentialSource, DirSink, PgDogPusher, PushError};
use loams_pg_control::pgdog::render::{NamespaceRecords, RenderSettings, RoleCredential, RoleKey};
use loams_pg_control::pgdog::{InstanceId, PgDogAdmin};
use loams_pg_control::secrets::{Secret, SecretRef};
use loams_pg_control::store::conformance::local_factory;
use loams_pg_control::store::{KvControlStore, StoreOptions};

const NS: &str = "itpgdog";
const PID: &str = "prj-01JZ0000000000000000000001";
const BID: &str = "br-01JZ0000000000000000000001";
const ADMIN_PASSWORD: &str = "it-admin-password";

fn engine() -> String {
    std::env::var("IT_CONTAINER_ENGINE").unwrap_or_else(|_| "docker".into())
}

/// The pinned image: the one reference pinned by digest in Task 18's
/// scratch stack (its Postgres backends are pinned by tag). The image is
/// named only in `deploy/pgdog` (Task 19's licence boundary).
fn image() -> String {
    let compose = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../deploy/pgdog/compose.yaml"
    ))
    .unwrap();
    compose
        .lines()
        .filter_map(|l| l.trim().strip_prefix("image: "))
        .find(|image| image.contains("@sha256:"))
        .expect("the pinned PgDog image")
        .to_owned()
}

/// Removes the container when the test ends, however it ends.
struct Container(String);

impl Drop for Container {
    fn drop(&mut self) {
        let _ = Command::new(engine()).args(["rm", "-f", &self.0]).output();
    }
}

fn run(args: &[&str]) -> String {
    let out = Command::new(engine()).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "{} {args:?}: {}",
        engine(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// One credential for every role (the backends are never reached: pools
/// need no server connection to be listed).
struct OneCredential;

#[async_trait]
impl CredentialSource for OneCredential {
    async fn credentials(
        &self,
        records: &NamespaceRecords,
    ) -> Result<BTreeMap<RoleKey, RoleCredential>, PushError> {
        Ok(records
            .roles
            .iter()
            .map(|r| {
                (
                    RoleKey {
                        branch_id: r.branch_id.clone(),
                        name: r.name.clone(),
                    },
                    RoleCredential {
                        verifier: "SCRAM-SHA-256$4096:c2FsdA==$c3RvcmVkS2V5:c2VydmVyS2V5".into(),
                        backend_password: Secret::new("backend".into()),
                    },
                )
            })
            .collect())
    }
}

async fn seed(store: &KvControlStore) {
    let fence = store
        .acquire_lease(&project_lease(PID), "it", Duration::from_secs(60))
        .await
        .unwrap();
    let project = ProjectRec {
        namespace: NS.into(),
        id: PID.into(),
        name: "shop".into(),
        tenant_id: [1; 16],
        pg_version: 17,
        wal: WalService::LoamsWal {
            pool: "default".into(),
        },
        history_retention_s: 86_400,
        region: "local".into(),
        default_branch_id: Some(BID.into()),
        settings: BTreeMap::new(),
        state: ProjectState::Ready,
        created_at_ms: 1,
        updated_at_ms: 1,
    };
    let branch = BranchRec {
        project_id: PID.into(),
        id: BID.into(),
        name: "main".into(),
        timeline_id: [1; 16],
        parent_id: None,
        ancestor_lsn: None,
        expires_at_ms: None,
        protected: false,
        stripe_size: None,
        shards: Vec::new(),
        state: BranchState::Ready,
        created_at_ms: 1,
        parent_time_ms: None,
        updated_at_ms: 1,
        pg_version: 17,
    };
    let role = RoleRec {
        project_id: PID.into(),
        branch_id: BID.into(),
        name: "app".into(),
        secret_ref: SecretRef::new_role(&PID.parse::<ProjectId>().unwrap()),
        login: true,
        pool_mode: None,
        system: false,
        created_at_ms: 1,
        updated_at_ms: 1,
    };
    store.put(&project, None, &fence).await.unwrap();
    store.put(&branch, None, &fence).await.unwrap();
    store.put(&role, None, &fence).await.unwrap();
    store
        .put(
            &endpoint("ep-01JZ0000000000000000000001", EndpointType::ReadWrite),
            None,
            &fence,
        )
        .await
        .unwrap();
}

fn endpoint(id: &str, kind: EndpointType) -> EndpointRec {
    let mut e = EndpointRec::new(PID, id, BID, kind);
    // Never connected to: nothing listens there.
    e.backend_addr = Some("127.0.0.1:1".into());
    e.min_cu = Cu(1);
    e.max_cu = Cu(1);
    e
}

#[tokio::test]
#[ignore = "starts the pinned PgDog in a container"]
async fn it_reload_picks_up_a_pushed_change() {
    let store = local_factory(env!("CARGO_TARGET_TMPDIR"))
        .store(StoreOptions::default())
        .await
        .unwrap();
    seed(&store).await;
    let root = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let mut settings = RenderSettings::new(Secret::new(ADMIN_PASSWORD.into()));
    settings.port = 6432;

    // The files exist before PgDog starts, as the sidecar's persisted copy
    // makes them on Kubernetes.
    let first = {
        let p = PgDogPusher::new(
            store.clone(),
            settings.clone(),
            Arc::new(OneCredential),
            Arc::new(DirSink {
                root: root.path().to_path_buf(),
            }),
            Arc::new(loams_pg_control::pgdog::NoPgDog),
        );
        let config = p.render(NS).await.unwrap();
        loams_pgdog_sync::write_config(
            &root.path().join(NS),
            &loams_pgdog_sync::Config {
                generation: config.generation,
                pgdog_toml: config.pgdog_toml.expose().clone(),
                users_toml: config.users_toml.expose().clone(),
            },
        )
        .unwrap();
        config.generation
    };

    let name = format!("pg2-it-pgdog-{}", std::process::id());
    let mount = format!("{}:/etc/pgdog:ro", root.path().join(NS).display());
    run(&[
        "run",
        "-d",
        "--name",
        &name,
        "-p",
        "127.0.0.1::6432",
        "-v",
        &mount,
        &image(),
        "pgdog",
        "--config",
        "/etc/pgdog/pgdog.toml",
        "--users",
        "/etc/pgdog/users.toml",
    ]);
    let container = Container(name);
    let port: u16 = run(&["port", &container.0, "6432/tcp"])
        .lines()
        .next()
        .and_then(|l| l.rsplit(':').next())
        .unwrap()
        .parse()
        .unwrap();

    let instance = InstanceId("127.0.0.1".into());
    let admin = PgDogAdminClient {
        login: AdminLogin {
            port,
            user: settings.admin_user.clone(),
            password: Secret::new(ADMIN_PASSWORD.into()),
            connect_timeout: Duration::from_secs(2),
        },
        directory: Arc::new(StaticDirectory(BTreeMap::from([(
            NS.to_owned(),
            vec![instance.clone()],
        )]))),
    };
    let mut before = Vec::new();
    for _ in 0..60 {
        if let Ok(names) = admin.show_databases(&instance).await {
            before = names;
            if !before.is_empty() {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(before.contains(&"shop".to_owned()), "{before:?}");
    assert!(!before.contains(&"shop__main__ro".to_owned()), "{before:?}");

    // A read-only endpoint appears in the records; the push writes the
    // files and reloads PgDog, which then serves its routed name.
    let fence = store
        .acquire_lease(&project_lease(PID), "it", Duration::from_secs(60))
        .await
        .unwrap();
    store
        .put(
            &endpoint("ep-01JZ0000000000000000000002", EndpointType::ReadOnly),
            None,
            &fence,
        )
        .await
        .unwrap();
    let admin: Arc<dyn PgDogAdmin> = Arc::new(admin.clone());
    let p = PgDogPusher::new(
        store,
        settings,
        Arc::new(OneCredential),
        Arc::new(DirSink {
            root: root.path().to_path_buf(),
        }),
        admin.clone(),
    );
    let pushed = p.push(NS).await.unwrap();
    assert_ne!(pushed.generation, first);
    assert_eq!(pushed.applied, vec![instance.clone()], "{pushed:?}");
    let after = admin.show_databases(&instance).await.unwrap();
    assert!(after.contains(&"shop__main__ro".to_owned()), "{after:?}");
    assert!(!p.resync(NS).await.unwrap());
    drop(container);
}
