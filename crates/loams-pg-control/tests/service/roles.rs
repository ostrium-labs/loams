//! The role and database RPCs of `pg-control` (PG2 Task 6) on the store of
//! the test binary, a fake Neon and an in-memory secret store.

use std::sync::Arc;

use crate::common::{
    FakeNeon, FirstBatch, Harness, captured_logs, fresh_store, harness_sharing, once_before,
    plain_service_with, user,
};
use loams_pg_control::ids::BranchId;
use loams_pg_control::model::{
    AllIdempotency, BranchState, DatabaseRec, IdempotencyRec, PoolMode, ProjectRec, RoleKey,
    RoleRec,
};
use loams_pg_control::secrets::SecretRef;
use loams_pg_control::service::Reason;
use loams_pg_control::service::branches::{CreateBranch, DeleteBranch};
use loams_pg_control::service::databases::{CreateDatabase, DeleteDatabase};
use loams_pg_control::service::projects::CreateProject;
use loams_pg_control::service::roles::{CreateRole, DeleteRole, ResetRolePassword};
use loams_pg_control::{KvControlStore, Page, PgControlStore, StoreOptions};

/// A project `shop` in `acme`, and its default branch's id.
async fn project(h: &Harness) -> (ProjectRec, String) {
    let out = h
        .service
        .create_project(
            &user(),
            CreateProject {
                namespace: "acme".into(),
                name: "shop".into(),
                idempotency_key: "p".into(),
                ..CreateProject::default()
            },
        )
        .await
        .expect("create project");
    let main = out.project.record.default_branch_id.clone().expect("main");
    (out.project.record, main)
}

fn role(project_id: &str, branch_id: &str, name: &str, key: &str) -> CreateRole {
    CreateRole {
        namespace: "acme".into(),
        project_id: project_id.into(),
        branch_id: branch_id.into(),
        name: name.into(),
        idempotency_key: key.into(),
    }
}

fn reset(project_id: &str, branch_id: &str, name: &str, key: &str) -> ResetRolePassword {
    ResetRolePassword {
        namespace: "acme".into(),
        project_id: project_id.into(),
        branch_id: branch_id.into(),
        name: name.into(),
        idempotency_key: key.into(),
    }
}

fn drop_role(project_id: &str, branch_id: &str, name: &str, key: &str) -> DeleteRole {
    DeleteRole {
        namespace: "acme".into(),
        project_id: project_id.into(),
        branch_id: branch_id.into(),
        name: name.into(),
        idempotency_key: key.into(),
    }
}

fn database(
    project_id: &str,
    branch_id: &str,
    name: &str,
    owner: &str,
    key: &str,
) -> CreateDatabase {
    CreateDatabase {
        namespace: "acme".into(),
        project_id: project_id.into(),
        branch_id: branch_id.into(),
        name: name.into(),
        owner_role: owner.into(),
        idempotency_key: key.into(),
    }
}

fn drop_database(project_id: &str, branch_id: &str, name: &str, key: &str) -> DeleteDatabase {
    DeleteDatabase {
        namespace: "acme".into(),
        project_id: project_id.into(),
        branch_id: branch_id.into(),
        name: name.into(),
        idempotency_key: key.into(),
    }
}

/// A child of `main` named `name` (at its head; `main` is still creating).
async fn child(h: &Harness, p: &ProjectRec, name: &str) -> String {
    h.service
        .create_branch(
            &user(),
            CreateBranch {
                namespace: "acme".into(),
                project_id: p.id.clone(),
                name: name.into(),
                idempotency_key: format!("branch-{name}"),
                ..CreateBranch::default()
            },
        )
        .await
        .expect("create branch")
        .branch
        .branch
        .record
        .id
}

/// The roles of a branch, every page.
async fn roles(h: &Harness, p: &ProjectRec, branch_id: &str) -> Vec<RoleRec> {
    let (page, next) = h
        .service
        .list_roles("acme", &p.id, branch_id, 0, "")
        .await
        .expect("list roles");
    assert!(next.is_empty(), "one page");
    page.into_iter().map(|r| r.record).collect()
}

/// The databases of a branch, every page.
async fn databases(h: &Harness, p: &ProjectRec, branch_id: &str) -> Vec<DatabaseRec> {
    let (page, next) = h
        .service
        .list_databases("acme", &p.id, branch_id, 0, "")
        .await
        .expect("list databases");
    assert!(next.is_empty(), "one page");
    page.into_iter().map(|d| d.record).collect()
}

async fn the_role(h: &Harness, branch_id: &str, name: &str) -> Option<RoleRec> {
    h.store
        .get::<RoleRec>(&RoleKey {
            branch_id: branch_id.into(),
            name: name.into(),
        })
        .await
        .expect("get role")
        .map(|v| v.record)
}

fn text(bytes: Option<Vec<u8>>) -> Option<String> {
    bytes.map(|b| String::from_utf8(b).expect("utf-8"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_role_returns_password_once() {
    let h = harness!();
    let (p, main) = project(&h).await;
    let out = h
        .service
        .create_role(&user(), role(&p.id, "", "app", "r1"))
        .await
        .expect("create");
    let r = &out.role.record;
    assert_eq!(
        (r.project_id.as_str(), r.branch_id.as_str(), r.name.as_str()),
        (p.id.as_str(), main.as_str(), "app"),
        "an empty branch_id takes the default branch"
    );
    assert!(r.login && !r.system && r.pool_mode.is_none());
    let password = out.password.expose().clone();
    assert_eq!(password.len(), 43, "32 bytes, base64url");
    assert_eq!(
        text(h.secrets.value(&r.secret_ref)).as_deref(),
        Some(password.as_str()),
        "the secret store holds it under the record's reference"
    );

    let e = h
        .service
        .create_role(&user(), role(&p.id, "", "app", "r1"))
        .await
        .expect_err("a replay");
    assert_eq!(e.reason, Reason::SecretAlreadyIssued, "{e}");
    assert_eq!(e.reason.code(), "failed_precondition");
    assert_eq!(e.metadata.get("role").map(String::as_str), Some("app"));
    assert!(e.metadata.contains_key("hint"), "{e:?}");
    assert!(!format!("{e:?} {e}").contains(&password));
    assert_eq!(h.secrets.len(), 1, "a replay stores no secret");

    let listed = roles(&h, &p, &main).await;
    assert_eq!(listed, vec![r.clone()]);
    // An explicit branch_id names the same branch.
    assert_eq!(roles(&h, &p, "").await, listed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reset_password_rotates_and_invalidates_old() {
    let h = harness!();
    let (p, main) = project(&h).await;
    let first = h
        .service
        .create_role(&user(), role(&p.id, &main, "app", "r1"))
        .await
        .expect("create");
    let old_ref = first.role.record.secret_ref.clone();
    let reset1 = h
        .service
        .reset_role_password(&user(), reset(&p.id, &main, "app", "x1"))
        .await
        .expect("reset");
    let new_ref = reset1.role.record.secret_ref.clone();
    assert_ne!(new_ref, old_ref, "a new reference");
    assert_ne!(reset1.password.expose(), first.password.expose());
    assert!(reset1.role.version > first.role.version);
    assert_eq!(
        text(h.secrets.value(&new_ref)).as_deref(),
        Some(reset1.password.expose().as_str())
    );
    assert_eq!(h.secrets.value(&old_ref), None, "the old secret is gone");
    assert_eq!(h.secrets.len(), 1);
    assert_eq!(
        the_role(&h, &main, "app")
            .await
            .expect("the role")
            .secret_ref,
        new_ref
    );

    let e = h
        .service
        .reset_role_password(&user(), reset(&p.id, &main, "app", "x1"))
        .await
        .expect_err("a replay");
    assert_eq!(e.reason, Reason::SecretAlreadyIssued, "{e}");
    assert_eq!(h.secrets.len(), 1, "a replay rotates nothing");

    let e = h
        .service
        .reset_role_password(&user(), reset(&p.id, &main, "ghost", "x2"))
        .await
        .expect_err("no such role");
    assert_eq!(e.reason, Reason::NotFound, "{e}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn role_secret_never_logged() {
    let logs = captured_logs();
    let h = harness!();
    let (p, main) = project(&h).await;
    let mut seen = Vec::new();
    let created = h
        .service
        .create_role(&user(), role(&p.id, &main, "app", "r1"))
        .await
        .expect("create");
    seen.push(format!("{created:?}"));
    let replay = h
        .service
        .create_role(&user(), role(&p.id, &main, "app", "r1"))
        .await
        .expect_err("replay");
    seen.push(format!("{replay:?} {replay}"));
    let reset1 = h
        .service
        .reset_role_password(&user(), reset(&p.id, &main, "app", "x1"))
        .await
        .expect("reset");
    seen.push(format!("{reset1:?}"));
    let replay = h
        .service
        .reset_role_password(&user(), reset(&p.id, &main, "app", "x1"))
        .await
        .expect_err("replay");
    seen.push(format!("{replay:?} {replay}"));
    seen.push(format!(
        "{:?}",
        h.service
            .create_database(&user(), database(&p.id, &main, "db", "app", "d1"))
            .await
            .expect("database")
    ));
    seen.push(format!(
        "{:?}",
        h.service.list_roles("acme", &p.id, &main, 0, "").await
    ));
    // A child branch copies the secret.
    let dev = child(&h, &p, "dev").await;
    seen.push(format!(
        "{:?}",
        h.service.list_roles("acme", &p.id, &dev, 0, "").await
    ));
    seen.push(format!(
        "{:?}",
        h.service
            .delete_database(&user(), drop_database(&p.id, &main, "db", "dd1"))
            .await
    ));
    seen.push(format!(
        "{:?}",
        h.service
            .delete_role(&user(), drop_role(&p.id, &main, "app", "dr1"))
            .await
    ));
    seen.push(format!("{:?}", h.service));

    let passwords = [
        created.password.expose().clone(),
        reset1.password.expose().clone(),
    ];
    let logged = String::from_utf8(logs.lock().expect("logs").clone()).expect("utf-8 logs");
    assert!(
        logged.contains(created.role.record.secret_ref.as_str()),
        "the capture works: the issue is logged, by reference"
    );
    // Every record and ledger entry the calls wrote.
    let (entries, _) = h
        .store
        .list::<IdempotencyRec>(&AllIdempotency, Page::first(1000))
        .await
        .expect("the ledger");
    assert!(!entries.is_empty());
    for e in &entries {
        seen.push(String::from_utf8_lossy(&e.record.answer).into_owned());
    }
    for r in roles(&h, &p, &dev).await {
        seen.push(format!("{r:?}"));
    }
    for password in &passwords {
        assert!(!logged.contains(password.as_str()), "a password was logged");
        for s in &seen {
            assert!(!s.contains(password.as_str()), "a password in {s}");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn database_owner_must_exist_on_branch() {
    let h = harness!();
    let (p, main) = project(&h).await;
    let dev = child(&h, &p, "dev").await;
    h.service
        .create_role(&user(), role(&p.id, &dev, "devonly", "r1"))
        .await
        .expect("a role on dev");
    for owner in ["ghost", "devonly"] {
        let e = h
            .service
            .create_database(&user(), database(&p.id, &main, "db", owner, owner))
            .await
            .expect_err("no such owner on main");
        assert_eq!(e.reason, Reason::FailedPrecondition, "{owner}: {e}");
        assert!(e.message.contains(owner), "{e}");
        assert!(
            e.metadata.is_empty(),
            "failed_precondition registers no metadata"
        );
    }
    let e = h
        .service
        .create_database(&user(), database(&p.id, &main, "db", "", "k0"))
        .await
        .expect_err("an owner is required");
    assert_eq!(e.reason, Reason::InvalidArgument, "{e}");

    h.service
        .create_role(&user(), role(&p.id, &main, "owner", "r2"))
        .await
        .expect("the owner");
    let db = h
        .service
        .create_database(&user(), database(&p.id, &main, "db", "owner", "k1"))
        .await
        .expect("database");
    assert_eq!(
        (
            db.record.name.as_str(),
            db.record.owner.as_str(),
            db.record.branch_id.as_str()
        ),
        ("db", "owner", main.as_str())
    );
    let replay = h
        .service
        .create_database(&user(), database(&p.id, &main, "db", "owner", "k1"))
        .await
        .expect("a replay");
    assert_eq!(replay, db);
    let e = h
        .service
        .create_database(&user(), database(&p.id, &main, "db", "owner", "k2"))
        .await
        .expect_err("a taken name");
    assert_eq!(e.reason, Reason::AlreadyExists, "{e}");
    assert_eq!(databases(&h, &p, &main).await, vec![db.record]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_branch_inherits_roles_and_databases() {
    let h = harness!();
    let (p, main) = project(&h).await;
    let app = h
        .service
        .create_role(&user(), role(&p.id, &main, "app", "r1"))
        .await
        .expect("role");
    h.service
        .create_role(&user(), role(&p.id, &main, "ro", "r2"))
        .await
        .expect("role");
    h.service
        .create_database(&user(), database(&p.id, &main, "shop", "app", "d1"))
        .await
        .expect("database");
    let dev = child(&h, &p, "dev").await;

    let copied = roles(&h, &p, &dev).await;
    assert_eq!(
        copied.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        ["app", "ro"]
    );
    let dev_app = copied[0].clone();
    assert_eq!(dev_app.branch_id, dev);
    assert_ne!(
        dev_app.secret_ref, app.role.record.secret_ref,
        "its own copy"
    );
    assert_eq!(
        text(h.secrets.value(&dev_app.secret_ref)).as_deref(),
        Some(app.password.expose().as_str()),
        "the same password, as the timeline's catalog has it"
    );
    let dbs = databases(&h, &p, &dev).await;
    assert_eq!(dbs.len(), 1);
    assert_eq!(
        (
            dbs[0].name.as_str(),
            dbs[0].owner.as_str(),
            dbs[0].branch_id.as_str()
        ),
        ("shop", "app", dev.as_str())
    );

    // The branches' roles are independent from here on.
    let rotated = h
        .service
        .reset_role_password(&user(), reset(&p.id, &main, "app", "x1"))
        .await
        .expect("reset main's");
    assert_ne!(rotated.password.expose(), app.password.expose());
    assert_eq!(
        text(h.secrets.value(&dev_app.secret_ref)).as_deref(),
        Some(app.password.expose().as_str()),
        "dev keeps the password it branched with"
    );
    h.service
        .delete_database(&user(), drop_database(&p.id, &dev, "shop", "dd1"))
        .await
        .expect("drop dev's database");
    h.service
        .delete_role(&user(), drop_role(&p.id, &dev, "app", "dr1"))
        .await
        .expect("drop dev's role");
    assert_eq!(h.secrets.value(&dev_app.secret_ref), None);
    assert_eq!(
        text(h.secrets.value(&rotated.role.record.secret_ref)).as_deref(),
        Some(rotated.password.expose().as_str()),
        "main's is untouched"
    );
    assert_eq!(databases(&h, &p, &main).await.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_role_same_name_other_key_is_already_exists() {
    let h = harness!();
    let (p, main) = project(&h).await;
    h.service
        .create_role(&user(), role(&p.id, &main, "app", "r1"))
        .await
        .expect("create");
    let e = h
        .service
        .create_role(&user(), role(&p.id, &main, "app", "r2"))
        .await
        .expect_err("taken");
    assert_eq!(e.reason, Reason::AlreadyExists, "{e}");
    assert_eq!(h.secrets.len(), 1);
}

/// A create that loses the name to a concurrent create, between its reads
/// and its commit, answers `already_exists` and leaves no secret behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_create_that_loses_the_race_discards_its_secret() {
    let Some(store) = fresh_store().await else {
        eprintln!("skipped: no store");
        return;
    };
    let secrets = Arc::new(crate::common::MemorySecrets::default());
    let setup = plain_service_with(&store, &FakeNeon::default(), secrets.clone());
    let p = setup
        .create_project(
            &user(),
            CreateProject {
                namespace: "acme".into(),
                name: "shop".into(),
                idempotency_key: "p".into(),
                ..CreateProject::default()
            },
        )
        .await
        .expect("project")
        .project
        .record;
    let hook = {
        let other = plain_service_with(&store, &FakeNeon::default(), secrets.clone());
        let pid = p.id.clone();
        once_before("CreateRole", move || {
            let (other, pid) = (other.clone(), pid.clone());
            async move {
                other
                    .create_role(&user(), role(&pid, "", "app", "winner"))
                    .await
                    .expect("the winner");
            }
        })
    };
    let h = harness_sharing(store, secrets.clone(), Some(hook));
    let e = h
        .service
        .create_role(&user(), role(&p.id, "", "app", "loser"))
        .await
        .expect_err("the name was taken meanwhile");
    assert_eq!(e.reason, Reason::AlreadyExists, "{e}");
    assert_eq!(secrets.len(), 1, "only the winner's secret");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_role_deletes_its_secret_and_refuses_an_owner() {
    let h = harness!();
    let (p, main) = project(&h).await;
    let app = h
        .service
        .create_role(&user(), role(&p.id, &main, "app", "r1"))
        .await
        .expect("role");
    h.service
        .create_database(&user(), database(&p.id, &main, "shop", "app", "d1"))
        .await
        .expect("database");
    let e = h
        .service
        .delete_role(&user(), drop_role(&p.id, &main, "app", "dr1"))
        .await
        .expect_err("it owns shop");
    assert_eq!(e.reason, Reason::FailedPrecondition, "{e}");
    assert!(e.message.contains("shop"), "{e}");
    h.service
        .delete_database(&user(), drop_database(&p.id, &main, "shop", "dd1"))
        .await
        .expect("drop the database");
    h.service
        .delete_role(&user(), drop_role(&p.id, &main, "app", "dr2"))
        .await
        .expect("drop the role");
    assert_eq!(h.secrets.value(&app.role.record.secret_ref), None);
    assert!(roles(&h, &p, &main).await.is_empty());
    h.service
        .delete_role(&user(), drop_role(&p.id, &main, "app", "dr2"))
        .await
        .expect("a replay answers the first response");
    let e = h
        .service
        .delete_role(&user(), drop_role(&p.id, &main, "app", "dr3"))
        .await
        .expect_err("gone");
    assert_eq!(e.reason, Reason::NotFound, "{e}");
    let e = h
        .service
        .delete_database(&user(), drop_database(&p.id, &main, "shop", "dd2"))
        .await
        .expect_err("gone");
    assert_eq!(e.reason, Reason::NotFound, "{e}");
}

/// A database created for a role while the role is being deleted: the
/// delete, about to commit, conflicts on the role record the create
/// rewrote, reads again and refuses.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_role_races_create_database() {
    let Some(store) = fresh_store().await else {
        eprintln!("skipped: no store");
        return;
    };
    let secrets = Arc::new(crate::common::MemorySecrets::default());
    let setup = plain_service_with(&store, &FakeNeon::default(), secrets.clone());
    let p = setup
        .create_project(
            &user(),
            CreateProject {
                namespace: "acme".into(),
                name: "shop".into(),
                idempotency_key: "p".into(),
                ..CreateProject::default()
            },
        )
        .await
        .expect("project")
        .project
        .record;
    let app = setup
        .create_role(&user(), role(&p.id, "", "app", "r1"))
        .await
        .expect("role");
    let hook = {
        let other = plain_service_with(&store, &FakeNeon::default(), secrets.clone());
        let pid = p.id.clone();
        once_before("DeleteRole", move || {
            let (other, pid) = (other.clone(), pid.clone());
            async move {
                other
                    .create_database(&user(), database(&pid, "", "shop", "app", "d1"))
                    .await
                    .expect("the database");
            }
        })
    };
    let h = harness_sharing(store, secrets.clone(), Some(hook));
    let e = h
        .service
        .delete_role(&user(), drop_role(&p.id, "", "app", "dr1"))
        .await
        .expect_err("the role owns a database now");
    assert_eq!(e.reason, Reason::FailedPrecondition, "{e}");
    let main = app.role.record.branch_id.clone();
    assert!(the_role(&h, &main, "app").await.is_some());
    assert!(secrets.value(&app.role.record.secret_ref).is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reserved_names_are_refused() {
    let h = harness!();
    let (p, main) = project(&h).await;
    let long = "a".repeat(64);
    for name in [
        "",
        "pg_monitor",
        "public",
        "none",
        "cloud_admin",
        "neon_superuser",
        "zenith_admin",
        "loams_ro",
        "tok_abc",
        "a\0b",
        long.as_str(),
    ] {
        let e = h
            .service
            .create_role(&user(), role(&p.id, &main, name, ""))
            .await
            .expect_err("reserved");
        assert_eq!(e.reason, Reason::InvalidArgument, "{name:?}: {e}");
    }
    h.service
        .create_role(&user(), role(&p.id, &main, "app", ""))
        .await
        .expect("an owner");
    for name in ["postgres", "template0", "template1", ""] {
        let e = h
            .service
            .create_database(&user(), database(&p.id, &main, name, "app", ""))
            .await
            .expect_err("reserved");
        assert_eq!(e.reason, Reason::InvalidArgument, "{name:?}: {e}");
    }
    assert_eq!(h.secrets.len(), 1, "a refused create stores no secret");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deleting_branch_takes_no_roles() {
    let h = harness!();
    let (p, _main) = project(&h).await;
    let dev = child(&h, &p, "dev").await;
    h.service
        .delete_branch(
            &user(),
            DeleteBranch {
                namespace: "acme".into(),
                project_id: p.id.clone(),
                branch_id: dev.clone(),
                ..DeleteBranch::default()
            },
        )
        .await
        .expect("delete dev");
    let e = h
        .service
        .create_role(&user(), role(&p.id, &dev, "app", ""))
        .await
        .expect_err("dev is deleting");
    assert_eq!(e.reason, Reason::FailedPrecondition, "{e}");
    let e = h
        .service
        .create_role(
            &user(),
            role(&p.id, &BranchId::new().to_string(), "app", ""),
        )
        .await
        .expect_err("no such branch");
    assert_eq!(e.reason, Reason::NotFound, "{e}");
    assert_eq!(h.secrets.len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_secret_write_writes_no_role() {
    let h = harness!();
    let (p, main) = project(&h).await;
    h.secrets.fail_puts(true);
    let e = h
        .service
        .create_role(&user(), role(&p.id, &main, "app", "r1"))
        .await
        .expect_err("the credential store is down");
    assert_eq!(e.reason, Reason::Unavailable, "{e}");
    assert!(roles(&h, &p, &main).await.is_empty());
    h.secrets.fail_puts(false);
    let out = h
        .service
        .create_role(&user(), role(&p.id, &main, "app", "r1"))
        .await
        .expect("the same key, once the store is back");
    assert_eq!(h.secrets.len(), 1);
    assert!(h.secrets.value(&out.role.record.secret_ref).is_some());
}

/// A create whose batch outcome is lost: resolved through the ledger when it
/// applied (the caller gets its password), and otherwise answered
/// `unavailable` with the secret kept (its use is unknown).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_role_undetermined_resolves_through_the_ledger() {
    use loams_kv::FaultPoint;

    for point in [FaultPoint::AfterCommit, FaultPoint::BeforeCommit] {
        let Some(kv) = crate::factory().kv().await else {
            eprintln!("skipped: no store");
            return;
        };
        let secrets = Arc::new(crate::common::MemorySecrets::default());
        let good = KvControlStore::new(kv.clone(), crate::common::options());
        let setup = plain_service_with(&good, &FakeNeon::default(), secrets.clone());
        let p = setup
            .create_project(
                &user(),
                CreateProject {
                    namespace: "acme".into(),
                    name: "shop".into(),
                    idempotency_key: "p".into(),
                    ..CreateProject::default()
                },
            )
            .await
            .expect("project")
            .project
            .record;
        let untokened = StoreOptions {
            commit_tokens: false,
            ..crate::common::options()
        };
        let faulty = KvControlStore::new(kv.with_faults(Arc::new(FirstBatch { point })), untokened);
        let h = harness_sharing(faulty, secrets.clone(), None);
        let first = h
            .service
            .create_role(&user(), role(&p.id, "", "app", "r1"))
            .await;
        let main = p.default_branch_id.clone().expect("main");
        match point {
            FaultPoint::AfterCommit => {
                let first = first.expect("resolved through the ledger: the caller's own");
                assert_eq!(
                    text(secrets.value(&first.role.record.secret_ref)).as_deref(),
                    Some(first.password.expose().as_str())
                );
                let e = setup
                    .create_role(&user(), role(&p.id, "", "app", "r1"))
                    .await
                    .expect_err("a retry");
                assert_eq!(e.reason, Reason::SecretAlreadyIssued, "{e}");
            }
            _ => {
                let e = first.expect_err("nothing applied: unknown to the caller");
                assert_eq!(e.reason, Reason::Unavailable, "{e}");
                assert_eq!(secrets.len(), 1, "kept: the write's outcome was unknown");
                let retry = setup
                    .create_role(&user(), role(&p.id, "", "app", "r1"))
                    .await
                    .expect("the retry creates it");
                assert_eq!(
                    text(secrets.value(&retry.role.record.secret_ref)).as_deref(),
                    Some(retry.password.expose().as_str())
                );
            }
        }
        assert!(the_role(&h, &main, "app").await.is_some(), "{point:?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_roles_paginates() {
    let h = harness!();
    let (p, main) = project(&h).await;
    for name in ["a", "b", "c"] {
        h.service
            .create_role(&user(), role(&p.id, &main, name, ""))
            .await
            .expect("role");
    }
    let (first, token) = h
        .service
        .list_roles("acme", &p.id, &main, 2, "")
        .await
        .expect("page 1");
    assert_eq!(first.len(), 2);
    assert!(!token.is_empty());
    let (second, end) = h
        .service
        .list_roles("acme", &p.id, &main, 2, &token)
        .await
        .expect("page 2");
    assert_eq!(second.len(), 1);
    assert!(end.is_empty());
    assert_eq!(second[0].record.name, "c");
    let e = h
        .service
        .list_roles("acme", &p.id, &main, -1, "")
        .await
        .expect_err("a negative size");
    assert_eq!(e.reason, Reason::InvalidArgument, "{e}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_system_role_is_neither_reset_nor_deleted() {
    let h = harness!();
    let (p, main) = project(&h).await;
    let system = RoleRec {
        project_id: p.id.clone(),
        branch_id: main.clone(),
        name: "agent_ro".into(),
        secret_ref: SecretRef::parse("pg-role-system").expect("a ref"),
        login: true,
        pool_mode: Some(PoolMode::Transaction),
        system: true,
        created_at_ms: 0,
        updated_at_ms: 0,
    };
    h.store
        .api_writer()
        .put(&system, None)
        .await
        .expect("a system role");
    let e = h
        .service
        .reset_role_password(&user(), reset(&p.id, &main, "agent_ro", ""))
        .await
        .expect_err("system");
    assert_eq!(e.reason, Reason::FailedPrecondition, "{e}");
    let e = h
        .service
        .delete_role(&user(), drop_role(&p.id, &main, "agent_ro", ""))
        .await
        .expect_err("system");
    assert_eq!(e.reason, Reason::FailedPrecondition, "{e}");
    assert_eq!(h.secrets.len(), 0, "a refused reset stores no secret");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn roles_of_a_branch_that_is_not_ready_are_kept() {
    // Roles are records, applied through the compute spec: a creating
    // branch takes them, a failed one does not.
    let h = harness!();
    let (p, main) = project(&h).await;
    h.service
        .create_role(&user(), role(&p.id, &main, "app", ""))
        .await
        .expect("main is creating");
    crate::common::set_branch_state(&h.store, &p.id, &main, BranchState::Failed).await;
    let e = h
        .service
        .create_role(&user(), role(&p.id, &main, "app2", ""))
        .await
        .expect_err("failed");
    assert_eq!(e.reason, Reason::FailedPrecondition, "{e}");
    assert_eq!(roles(&h, &p, &main).await.len(), 1, "listing still works");
}
