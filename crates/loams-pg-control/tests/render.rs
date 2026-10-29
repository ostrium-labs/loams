//! PgDog's config for a namespace (PG2 Task 20): `pgdog.toml` and
//! `users.toml` from the records, checked against goldens in
//! `tests/golden/pgdog/` (`LOAMS_BLESS=1` rewrites them).
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::Path;

use loams_pg_control::autoscale::max_connections;
use loams_pg_control::ids::{BranchId, ProjectId};
use loams_pg_control::model::{
    BranchRec, BranchState, Cu, DatabaseRec, EndpointRec, EndpointType, PoolMode, ProjectRec,
    ProjectState, RoleRec, ShardAttachment, WalService,
};
use loams_pg_control::pgdog::render::{
    ClientTls, NamespaceRecords, RenderSettings, RoleCredential, RoleKey, render_namespace,
};
use loams_pg_control::secrets::{Secret, SecretRef};

const NS: &str = "acme";

/// Fixed ids, so the goldens do not move.
fn pid(n: u8) -> String {
    format!("prj-01JZ00000000000000000000{n:02}")
}
fn bid(n: u8) -> String {
    format!("br-01JZ00000000000000000000{n:02}")
}

fn project(n: u8, name: &str, default_branch: u8) -> ProjectRec {
    ProjectRec {
        namespace: NS.into(),
        id: pid(n),
        name: name.into(),
        tenant_id: [n; 16],
        pg_version: 17,
        wal: WalService::LoamsWal {
            pool: "default".into(),
        },
        history_retention_s: 86_400,
        region: "local".into(),
        default_branch_id: Some(bid(default_branch)),
        settings: BTreeMap::new(),
        state: ProjectState::Ready,
        created_at_ms: 1,
        updated_at_ms: 1,
    }
}

fn branch(project: u8, n: u8, name: &str) -> BranchRec {
    BranchRec {
        project_id: pid(project),
        id: bid(n),
        name: name.into(),
        timeline_id: [n; 16],
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
    }
}

fn endpoint(
    project: u8,
    branch: u8,
    id: &str,
    kind: EndpointType,
    addr: &str,
    cu: u16,
) -> EndpointRec {
    let mut e = EndpointRec::new(pid(project), id, bid(branch), kind);
    e.backend_addr = Some(addr.into());
    e.min_cu = Cu(1);
    e.max_cu = Cu(cu);
    e
}

fn role(project: u8, branch: u8, name: &str) -> RoleRec {
    RoleRec {
        project_id: pid(project),
        branch_id: bid(branch),
        name: name.into(),
        secret_ref: SecretRef::new_role(&pid(project).parse::<ProjectId>().unwrap()),
        login: true,
        pool_mode: None,
        system: false,
        created_at_ms: 1,
        updated_at_ms: 1,
    }
}

fn database(project: u8, branch: u8, name: &str) -> DatabaseRec {
    DatabaseRec {
        project_id: pid(project),
        branch_id: bid(branch),
        name: name.into(),
        owner: "app".into(),
        created_at_ms: 1,
    }
}

fn credential(seed: &str) -> RoleCredential {
    RoleCredential {
        verifier: format!("SCRAM-SHA-256$4096:c2FsdC0{seed}$c3RvcmVk:c2VydmVy"),
        backend_password: Secret::new(format!("backend-{seed}")),
    }
}

/// Two projects (`shop`, `blog`), three branches (`shop`'s `main` and
/// `dev`, `blog`'s `main`), read-write endpoints on each, a read-only one
/// on `shop/main`, roles and databases.
fn records() -> NamespaceRecords {
    let _ = BranchId::new(); // ids parse as the model's
    let roles = vec![
        role(1, 1, "app"),
        {
            let mut r = role(1, 1, "migrator");
            r.pool_mode = Some(PoolMode::Session);
            r
        },
        role(1, 2, "app"),
        role(2, 3, "writer"),
    ];
    let credentials = roles
        .iter()
        .map(|r| {
            (
                RoleKey {
                    branch_id: r.branch_id.clone(),
                    name: r.name.clone(),
                },
                credential(&format!("{}-{}", &r.branch_id[16..], r.name)),
            )
        })
        .collect();
    NamespaceRecords {
        namespace: NS.into(),
        projects: vec![project(1, "shop", 1), project(2, "blog", 3)],
        branches: vec![
            branch(1, 1, "main"),
            branch(1, 2, "dev"),
            branch(2, 3, "main"),
        ],
        endpoints: vec![
            endpoint(
                1,
                1,
                "ep-01JZ0000000000000000000001",
                EndpointType::ReadWrite,
                "ep-01jz0000000000000000000001.loams-pg-acme.svc.cluster.local:5432",
                4,
            ),
            endpoint(
                1,
                1,
                "ep-01JZ0000000000000000000002",
                EndpointType::ReadOnly,
                "ep-01jz0000000000000000000002.loams-pg-acme.svc.cluster.local:5432",
                2,
            ),
            endpoint(
                1,
                2,
                "ep-01JZ0000000000000000000003",
                EndpointType::ReadWrite,
                "ep-01jz0000000000000000000003.loams-pg-acme.svc.cluster.local:5432",
                1,
            ),
            endpoint(
                2,
                3,
                "ep-01JZ0000000000000000000004",
                EndpointType::ReadWrite,
                "ep-01jz0000000000000000000004.loams-pg-acme.svc.cluster.local:5432",
                8,
            ),
        ],
        roles,
        databases: vec![
            database(1, 1, "orders"),
            database(1, 1, "catalog"),
            database(2, 3, "posts"),
        ],
        credentials,
    }
}

fn settings() -> RenderSettings {
    let mut s = RenderSettings::new(Secret::new("admin-secret".into()));
    s.client_tls = Some(ClientTls {
        certificate: "/etc/pgdog/tls/tls.crt".into(),
        private_key: "/etc/pgdog/tls/tls.key".into(),
    });
    s.backend_ca = Some("/etc/pgdog/compute-ca/ca.crt".into());
    s
}

fn golden(name: &str, text: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/pgdog")
        .join(name);
    if std::env::var_os("LOAMS_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
    }
    assert_eq!(text, std::fs::read_to_string(&path).unwrap(), "{name}");
}

/// The same records give byte-identical files and generation, whatever
/// order the records come in; a change moves the generation.
#[test]
fn render_is_deterministic() {
    let a = render_namespace(&records(), &settings()).unwrap();
    let mut shuffled = records();
    shuffled.projects.reverse();
    shuffled.branches.reverse();
    shuffled.endpoints.reverse();
    shuffled.roles.reverse();
    shuffled.databases.reverse();
    let b = render_namespace(&shuffled, &settings()).unwrap();
    assert_eq!(a.pgdog_toml.expose(), b.pgdog_toml.expose());
    assert_eq!(a.users_toml.expose(), b.users_toml.expose());
    assert_eq!(a.generation, b.generation);
    let mut changed = records();
    changed.databases.push(database(1, 1, "billing"));
    let c = render_namespace(&changed, &settings()).unwrap();
    assert_ne!(a.generation, c.generation);
    // Neither file shows in Debug.
    let shown = format!("{a:?}");
    assert!(
        !shown.contains("admin-secret") && !shown.contains("backend-"),
        "{shown}"
    );
}

/// Two projects, three branches: the routed names of §46 §8.1, primaries
/// and a replica, the users and their grants.
#[test]
fn render_golden_two_projects_three_branches() {
    let r = render_namespace(&records(), &settings()).unwrap();
    golden(
        "two_projects_three_branches.pgdog.toml",
        r.pgdog_toml.expose(),
    );
    golden(
        "two_projects_three_branches.users.toml",
        r.users_toml.expose(),
    );
    let toml = r.pgdog_toml.expose();
    for name in [
        "\"shop\"",
        "\"shop__main\"",
        "\"shop__main.orders\"",
        "\"shop__main.catalog\"",
        "\"shop__main__ro\"",
        "\"shop__dev\"",
        "\"blog\"",
        "\"blog__main.posts\"",
    ] {
        assert!(toml.contains(&format!("name = {name}")), "{name}");
    }
    assert!(
        !toml.contains("name = \"shop__dev.orders\""),
        "databases are per branch"
    );
    assert!(r.left_out.is_empty());
}

/// A role is rendered only for its own branch's routed names; a role that
/// cannot log in, a system role, and a role with no credentials are absent.
#[test]
fn ungranted_role_absent_from_users() {
    let mut recs = records();
    let mut nologin = role(1, 1, "reader");
    nologin.login = false;
    let mut system = role(1, 1, "neon_superuser");
    system.system = true;
    recs.roles.extend([nologin, system, role(1, 1, "nocred")]);
    let r = render_namespace(&recs, &settings()).unwrap();
    let users = r.users_toml.expose();
    for absent in ["\"reader\"", "\"neon_superuser\"", "\"nocred\""] {
        assert!(!users.contains(&format!("name = {absent}")), "{absent}");
    }
    // blog's `writer` reaches blog's names only, shop/dev's `app` only dev's.
    let entry = |name: &str, needle: &str| {
        users
            .split("[[users]]")
            .filter(|e| e.contains(&format!("name = \"{name}\"")))
            .any(|e| e.contains(needle))
    };
    assert!(entry("writer", "\"blog__main.posts\""));
    assert!(!entry("writer", "\"shop"));
    assert!(entry("app", "databases = [\"shop__dev\"]"));
}

/// Read-only endpoints are replicas of the branch's `__ro` name, read-only;
/// the primary names never list them.
#[test]
fn ro_endpoint_rendered_as_replica() {
    let r = render_namespace(&records(), &settings()).unwrap();
    let toml = r.pgdog_toml.expose();
    let entries: Vec<&str> = toml.split("[[databases]]").skip(1).collect();
    let ro: Vec<&&str> = entries
        .iter()
        .filter(|e| e.contains("name = \"shop__main__ro\""))
        .collect();
    assert_eq!(ro.len(), 1);
    assert!(ro[0].contains("role = \"replica\"") && ro[0].contains("read_only = true"));
    assert!(ro[0].contains("ep-01jz0000000000000000000002"));
    for e in entries.iter().filter(|e| !e.contains("__ro\"")) {
        assert!(e.contains("role = \"primary\""), "{e}");
        assert!(!e.contains("ep-01jz0000000000000000000002"), "{e}");
    }
}

/// Pool sizes are 80 % of the compute's `max_connections` at the
/// endpoint's largest size.
#[test]
fn pool_size_follows_cu() {
    assert_eq!(max_connections(Cu(1)), 112);
    assert_eq!(max_connections(Cu(4)), 450);
    assert_eq!(max_connections(Cu(16)), 1802);
    assert_eq!(max_connections(Cu(64)), 4000);
    let r = render_namespace(&records(), &settings()).unwrap();
    let toml = r.pgdog_toml.expose();
    let size_of = |name: &str| -> u32 {
        let e = toml
            .split("[[databases]]")
            .find(|e| e.contains(&format!("name = \"{name}\"\n")))
            .unwrap();
        e.lines()
            .find_map(|l| l.strip_prefix("pool_size = "))
            .unwrap()
            .parse()
            .unwrap()
    };
    assert_eq!(size_of("shop"), max_connections(Cu(4)) * 4 / 5); // 1 CU
    assert_eq!(size_of("shop__dev"), max_connections(Cu(1)) * 4 / 5); // 0.25 CU
    assert_eq!(size_of("blog"), max_connections(Cu(8)) * 4 / 5); // 2 CU
}

/// Sharded projects wait for storage HA (R2.14): left out, and said so.
#[test]
fn sharded_project_is_left_out() {
    let mut recs = records();
    recs.branches[2].stripe_size = Some(2048);
    recs.branches[2].shards = vec![
        ShardAttachment {
            shard_number: 0,
            node_id: 1,
        },
        ShardAttachment {
            shard_number: 1,
            node_id: 2,
        },
    ];
    let r = render_namespace(&recs, &settings()).unwrap();
    assert!(!r.pgdog_toml.expose().contains("\"blog"));
    assert!(!r.users_toml.expose().contains("\"blog"));
    assert_eq!(r.left_out.len(), 1);
    assert_eq!(r.left_out[0].0, pid(2));
}

/// Without TLS settings the files say so; an endpoint without an address
/// is not rendered.
#[test]
fn plain_settings_and_addressless_endpoints() {
    let mut recs = records();
    recs.endpoints[2].backend_addr = None;
    let r = render_namespace(&recs, &RenderSettings::new(Secret::new("x".into()))).unwrap();
    let toml = r.pgdog_toml.expose();
    assert!(toml.contains("tls_client_required = false"));
    assert!(toml.contains("tls_verify = \"disabled\""));
    assert!(!toml.contains("\"shop__dev\""));
    assert!(toml.contains("idle_healthcheck_interval = 0"));
    assert!(toml.contains("min_pool_size = 0"));
}

/// The namespace's records read back from the store render the same
/// `pgdog.toml` (credentials come with Task 24).
#[tokio::test]
async fn records_are_read_from_the_store() {
    use loams_pg_control::StoreOptions;
    use loams_pg_control::store::conformance::local_factory;
    let store = local_factory(env!("CARGO_TARGET_TMPDIR"))
        .store(StoreOptions::default())
        .await
        .unwrap();
    let recs = records();
    let api = store.api_writer();
    for p in &recs.projects {
        api.put(p, None).await.unwrap();
    }
    for b in &recs.branches {
        api.put(b, None).await.unwrap();
    }
    for e in &recs.endpoints {
        api.put(e, None).await.unwrap();
    }
    for r in &recs.roles {
        api.put(r, None).await.unwrap();
    }
    for d in &recs.databases {
        api.put(d, None).await.unwrap();
    }
    let read = NamespaceRecords::read(&store, NS).await.unwrap();
    assert!(read.credentials.is_empty());
    let a = render_namespace(&read, &settings()).unwrap();
    let b = render_namespace(&recs, &settings()).unwrap();
    assert_eq!(a.pgdog_toml.expose(), b.pgdog_toml.expose());
    let other = NamespaceRecords::read(&store, "other").await.unwrap();
    assert!(other.projects.is_empty());
}
