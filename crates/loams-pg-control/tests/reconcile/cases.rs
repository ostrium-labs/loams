//! The reconcile cases (PG2 Task 7): what the project and branch
//! reconcilers do with Neon and the records, under the project's fenced
//! lease, run by `reconcile` (local store) and `reconcile_tikv`.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use loams_pg_control::ids::ProjectId;
use loams_pg_control::model::{
    BranchGuardKey, BranchGuardRec, BranchNameKey, BranchNameRec, BranchRec, BranchScope,
    BranchState, Cu, DatabaseRec, DesiredState, EndpointKey, EndpointRec, EndpointState,
    EndpointType, OperationState, PoolMode, ProjectNameKey, ProjectNameRec, ProjectState, RoleRec,
    project_lease,
};
use loams_pg_control::neon::{TenantId, TimelineId};
use loams_pg_control::reconcile::{
    BeforeWrite, Pass, ReconcileError, Reconciler, ReconcilerConfig,
};
use loams_pg_control::secrets::{Secret, SecretRef, SecretStore};
use loams_pg_control::service::branches::{BranchPoint, CreateBranch, DeleteBranch};
use loams_pg_control::service::databases::CreateDatabase;
use loams_pg_control::service::projects::{CreateProject, DeleteProject, UpdateProject};
use loams_pg_control::service::roles::CreateRole;
use loams_pg_control::service::{Clock, Reason};
use loams_pg_control::{KvControlStore, Page, PgControlStore, StoreError};
use tokio::sync::{Notify, oneshot};

use crate::common::{FakeNeon, Harness, INITDB_LSN, admin, user};

const NS: &str = "acme";

/// A reconciler's settings for a test: quick retries and resyncs, the
/// harness's clock.
fn config(h: &Harness, holder: &str) -> ReconcilerConfig {
    let at = h.clock.0.clone();
    ReconcilerConfig {
        resync: Duration::from_millis(500),
        retry_initial: Duration::from_millis(50),
        retry_max: Duration::from_millis(500),
        clock: Clock::from_fn(move || at.load(Ordering::SeqCst)),
        ..ReconcilerConfig::new(holder)
    }
}

fn reconciler(h: &Harness, config: ReconcilerConfig) -> Reconciler<KvControlStore, FakeNeon> {
    Reconciler::new(h.store.clone(), h.neon.clone(), h.secrets.clone(), config)
}

/// A reconciler's run loop, stopped when dropped.
struct Running {
    stop: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<Result<(), ReconcileError>>,
}

impl Running {
    async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let out = (&mut self.task).await.expect("the run loop");
        out.expect("the run loop ends cleanly");
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn start(r: Reconciler<KvControlStore, FakeNeon>) -> Running {
    let (stop, stopped) = oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        r.run(async move {
            let _ = stopped.await;
        })
        .await
    });
    Running {
        stop: Some(stop),
        task,
    }
}

/// Polls `f` until it holds, for up to 30 s.
async fn eventually<F, Fut>(what: &str, f: F)
where
    F: Fn() -> Fut,
    Fut: Future<Output = bool>,
{
    for _ in 0..600 {
        if f().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

fn create(name: &str, key: &str) -> CreateProject {
    CreateProject {
        namespace: NS.into(),
        name: name.into(),
        idempotency_key: key.into(),
        ..CreateProject::default()
    }
}

fn child(project_id: &str, name: &str, key: &str) -> CreateBranch {
    CreateBranch {
        namespace: NS.into(),
        project_id: project_id.into(),
        name: name.into(),
        point: BranchPoint::Head,
        idempotency_key: key.into(),
        ..CreateBranch::default()
    }
}

async fn project_state(h: &Harness, id: &str) -> Option<ProjectState> {
    h.service
        .get_project(NS, id)
        .await
        .ok()
        .map(|p| p.record.state)
}

async fn branch_state(h: &Harness, project_id: &str, branch_id: &str) -> Option<BranchState> {
    h.service
        .get_branch(NS, project_id, branch_id)
        .await
        .ok()
        .map(|b| b.branch.record.state)
}

async fn op_state(h: &Harness, op_id: &str) -> OperationState {
    h.service.get_operation(op_id).await.expect("the op").state
}

/// Creates a project and waits, with `running`, for it to be ready;
/// answers its id, its `main` branch id and its tenant.
async fn ready_project(h: &Harness, name: &str) -> (String, String, [u8; 16]) {
    let created = h
        .service
        .create_project(&user(), create(name, &format!("create-{name}")))
        .await
        .expect("create project");
    let id = created.project.record.id.clone();
    let main = created
        .project
        .record
        .default_branch_id
        .clone()
        .expect("main");
    eventually("the project to be ready", || async {
        project_state(h, &id).await == Some(ProjectState::Ready)
    })
    .await;
    (id, main, created.project.record.tenant_id)
}

async fn ready_branch(h: &Harness, project_id: &str, name: &str) -> BranchRec {
    let created = h
        .service
        .create_branch(&user(), child(project_id, name, &format!("branch-{name}")))
        .await
        .expect("create branch");
    let id = created.branch.branch.record.id.clone();
    eventually("the branch to be ready", || async {
        branch_state(h, project_id, &id).await == Some(BranchState::Ready)
    })
    .await;
    created.branch.branch.record
}

async fn roles_of(store: &KvControlStore, branch_id: &str) -> Vec<RoleRec> {
    let scope = BranchScope {
        branch_id: branch_id.into(),
    };
    let (roles, _) = store
        .list::<RoleRec>(&scope, Page::first(1000))
        .await
        .expect("list roles");
    roles.into_iter().map(|r| r.record).collect()
}

async fn databases_of(store: &KvControlStore, branch_id: &str) -> Vec<DatabaseRec> {
    let scope = BranchScope {
        branch_id: branch_id.into(),
    };
    let (dbs, _) = store
        .list::<DatabaseRec>(&scope, Page::first(1000))
        .await
        .expect("list databases");
    dbs.into_iter().map(|r| r.record).collect()
}

async fn guard(
    store: &KvControlStore,
    project_id: &str,
    branch_id: &str,
) -> Option<BranchGuardRec> {
    store
        .get::<BranchGuardRec>(&BranchGuardKey {
            project_id: project_id.into(),
            branch_id: branch_id.into(),
        })
        .await
        .expect("get guard")
        .map(|g| g.record)
}

fn endpoint(project_id: &str, branch_id: &str) -> EndpointRec {
    EndpointRec {
        project_id: project_id.into(),
        id: "ep-01K7ZZZZZZZZZZZZZZZZZZZZZZ".into(),
        branch_id: branch_id.into(),
        kind: EndpointType::ReadWrite,
        min_cu: Cu(1),
        max_cu: Cu(4),
        suspend_timeout_s: 300,
        pool_mode: PoolMode::Transaction,
        pg_settings: Default::default(),
        desired: DesiredState::Suspended,
        state: EndpointState::Suspended,
        compute_id: None,
        backend_addr: None,
        failure: None,
    }
}

fn tl(id: [u8; 16]) -> String {
    TimelineId(id).to_string()
}

/// A hook that, the first time `step` is about to be written, signals
/// `reached` and then waits for `release` (forever if it never comes).
fn pause_at(step: &'static str, reached: Arc<Notify>, release: Arc<Notify>) -> BeforeWrite {
    let fired = Arc::new(AtomicBool::new(false));
    Arc::new(move |at: &'static str| {
        let run = at == step && !fired.swap(true, Ordering::SeqCst);
        let (reached, release) = (reached.clone(), release.clone());
        Box::pin(async move {
            if run {
                reached.notify_one();
                release.notified().await;
            }
        })
    })
}

/// Runs passes of `r` until one reaches the hook that notifies `reached`,
/// and answers that pass, still paused there. A pass that ends before (a
/// transient store error under load) is run again; one that ends
/// otherwise fails the test.
async fn pass_to_hook(
    r: Arc<Reconciler<KvControlStore, FakeNeon>>,
    project: &str,
    reached: &Notify,
) -> tokio::task::JoinHandle<Result<Pass, ReconcileError>> {
    for _ in 0..100 {
        let mut pass = {
            let (r, id) = (r.clone(), project.to_string());
            tokio::spawn(async move { r.reconcile_project(NS, &id).await })
        };
        tokio::select! {
            () = reached.notified() => return pass,
            out = &mut pass => match out.expect("the pass") {
                Err(ReconcileError::Store(_) | ReconcileError::Moved) => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                other => panic!("the pass ended before the hook: {other:?}"),
            },
        }
    }
    panic!("no pass reached the hook");
}

/// Takes `project`'s lease for `holder` as soon as the current holder's
/// lapses.
async fn take_lease(store: &KvControlStore, project: &str, holder: &str) {
    for _ in 0..600 {
        match store
            .acquire_lease(&project_lease(project), holder, Duration::from_secs(60))
            .await
        {
            Ok(_) => return,
            Err(StoreError::Held { .. }) => tokio::time::sleep(Duration::from_millis(50)).await,
            Err(e) => panic!("acquire: {e}"),
        }
    }
    panic!("the lease never lapsed");
}

/// Runs passes of `r` until one is not `Held` (the earlier holder's lease
/// lapses).
async fn pass_when_free(r: &Reconciler<KvControlStore, FakeNeon>, project: &str) -> Pass {
    for _ in 0..600 {
        match r.reconcile_project(NS, project).await {
            Ok(Pass::Held { .. }) | Err(ReconcileError::Store(_) | ReconcileError::Moved) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Ok(p) => return p,
            Err(e) => panic!("pass: {e}"),
        }
    }
    panic!("the lease never lapsed");
}

/// `CreateProject` → the reconciler attaches the tenant with its
/// `pitr_interval`, creates `main`'s timeline on the pageserver and on
/// `loams-wal`, and marks the project and `main` ready and the operation
/// succeeded, with its progress complete.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_reaches_ready() {
    let h = harness!();
    let run = start(reconciler(&h, config(&h, "pg-control-a")));
    let created = h
        .service
        .create_project(&user(), create("shop", "k1"))
        .await
        .expect("create");
    let p = created.project.record.clone();
    eventually("ready", || async {
        project_state(&h, &p.id).await == Some(ProjectState::Ready)
    })
    .await;
    let main = p.default_branch_id.clone().expect("main");
    assert_eq!(
        branch_state(&h, &p.id, &main).await,
        Some(BranchState::Ready)
    );
    let op = h
        .service
        .get_operation(&created.operation.id)
        .await
        .expect("the op");
    assert_eq!(op.state, OperationState::Succeeded);
    let progress = op.progress.expect("progress");
    assert_eq!((progress.done, progress.total), (3, 3), "{progress:?}");
    assert_eq!(progress.unit, "steps");

    let t = TenantId(p.tenant_id);
    let main_tl = h
        .service
        .get_branch(NS, &p.id, &main)
        .await
        .expect("main")
        .branch
        .record
        .timeline_id;
    assert_eq!(h.neon.count(&format!("attach {t}")), 1);
    assert_eq!(
        h.neon
            .count(&format!("tenant_config {t} pitr={}", 7 * 24 * 3600)),
        1
    );
    assert_eq!(
        h.neon.count(&format!(
            "create_timeline {} ancestor=- at=- pg=17",
            tl(main_tl)
        )),
        1,
        "{:?}",
        h.neon.calls()
    );
    assert_eq!(
        h.neon.count(&format!(
            "wal_create_timeline {} start={INITDB_LSN} pg=17",
            tl(main_tl)
        )),
        1
    );
    run.stop().await;
}

/// `CreateBranch` at the head of a ready `main` → the timeline is a branch
/// of `main`'s at the recorded LSN, on the pageserver and on `loams-wal`;
/// the branch and its operation finish.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn branch_reaches_ready() {
    let h = harness!();
    let run = start(reconciler(&h, config(&h, "pg-control-a")));
    let (project, main, _) = ready_project(&h, "shop").await;
    let created = h
        .service
        .create_branch(&user(), child(&project, "dev", "b1"))
        .await
        .expect("create branch");
    let b = created.branch.branch.record.clone();
    assert_eq!(b.ancestor_lsn, Some(INITDB_LSN.0));
    eventually("the branch ready", || async {
        branch_state(&h, &project, &b.id).await == Some(BranchState::Ready)
    })
    .await;
    assert_eq!(
        op_state(&h, &created.operation.id).await,
        OperationState::Succeeded
    );
    let main_tl = h
        .service
        .get_branch(NS, &project, &main)
        .await
        .expect("main")
        .branch
        .record
        .timeline_id;
    assert_eq!(
        h.neon.count(&format!(
            "create_timeline {} ancestor={} at={INITDB_LSN} pg=-",
            tl(b.timeline_id),
            tl(main_tl)
        )),
        1,
        "{:?}",
        h.neon.calls()
    );
    assert_eq!(
        h.neon
            .count(&format!("wal_create_timeline {}", tl(b.timeline_id))),
        1
    );
    run.stop().await;
}

/// Two reconcilers on one store: each project's lease lets one act, so the
/// fake sees exactly one attach per tenant and one create per timeline.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_instances_one_actor() {
    let h = harness!();
    let a = start(reconciler(&h, config(&h, "pg-control-a")));
    let b = start(reconciler(&h, config(&h, "pg-control-b")));
    let mut projects = Vec::new();
    for i in 0..3 {
        let created = h
            .service
            .create_project(&user(), create(&format!("shop{i}"), &format!("k{i}")))
            .await
            .expect("create");
        projects.push(created.project.record);
    }
    for p in &projects {
        eventually("every project ready", || async {
            project_state(&h, &p.id).await == Some(ProjectState::Ready)
        })
        .await;
    }
    let mut branches = Vec::new();
    for p in &projects {
        let created = h
            .service
            .create_branch(&user(), child(&p.id, "dev", &format!("b-{}", p.id)))
            .await
            .expect("create branch");
        branches.push(created.branch.branch.record);
    }
    for br in &branches {
        eventually("every branch ready", || async {
            branch_state(&h, &br.project_id, &br.id).await == Some(BranchState::Ready)
        })
        .await;
    }
    for p in &projects {
        let t = TenantId(p.tenant_id);
        assert_eq!(
            h.neon.count(&format!("attach {t}")),
            1,
            "{:?}",
            h.neon.calls()
        );
    }
    for br in &branches {
        assert_eq!(
            h.neon
                .count(&format!("create_timeline {}", tl(br.timeline_id))),
            1
        );
    }
    a.stop().await;
    b.stop().await;
}

/// The first holder pauses before its write, its lease lapses and a second
/// holder takes it: the first one's write is fenced, and changes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_lease_write_is_fenced() {
    let h = harness!();
    let created = h
        .service
        .create_project(&user(), create("shop", "k1"))
        .await
        .expect("create");
    let id = created.project.record.id.clone();
    let (reached, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let a = reconciler(
        &h,
        ReconcilerConfig {
            lease_ttl: Duration::from_millis(300),
            before_write: Some(pause_at("project.ready", reached.clone(), release.clone())),
            ..config(&h, "pg-control-a")
        },
    );
    let pass = pass_to_hook(Arc::new(a), &id, &reached).await;
    take_lease(&h.store, &id, "pg-control-b").await;
    release.notify_one();
    let out = pass.await.expect("a's pass");
    assert!(matches!(out, Err(ReconcileError::Fenced)), "{out:?}");
    assert_eq!(project_state(&h, &id).await, Some(ProjectState::Creating));

    // The new holder finishes the work.
    let b = reconciler(&h, config(&h, "pg-control-b"));
    let out = pass_when_free(&b, &id).await;
    assert!(matches!(out, Pass::Done), "{out:?}");
    assert_eq!(project_state(&h, &id).await, Some(ProjectState::Ready));
    assert_eq!(
        op_state(&h, &created.operation.id).await,
        OperationState::Succeeded
    );
}

/// A pass dies after the pageserver created `main`'s timeline and before
/// the record says so: the next pass gets 409 for the create, reads it as
/// done, and reaches ready.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crash_between_neon_call_and_record_write_converges() {
    let h = harness!();
    let created = h
        .service
        .create_project(&user(), create("shop", "k1"))
        .await
        .expect("create");
    let id = created.project.record.id.clone();
    let main = created
        .project
        .record
        .default_branch_id
        .clone()
        .expect("main");
    let (reached, never) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let a = reconciler(
        &h,
        ReconcilerConfig {
            lease_ttl: Duration::from_millis(300),
            before_write: Some(pause_at("project.ready", reached.clone(), never)),
            ..config(&h, "pg-control-a")
        },
    );
    let pass = pass_to_hook(Arc::new(a), &id, &reached).await;
    pass.abort();
    let _ = pass.await;
    assert_eq!(h.neon.count("create_timeline"), 1);
    assert_eq!(project_state(&h, &id).await, Some(ProjectState::Creating));

    let b = reconciler(&h, config(&h, "pg-control-b"));
    let out = pass_when_free(&b, &id).await;
    assert!(matches!(out, Pass::Done), "{out:?}");
    assert_eq!(project_state(&h, &id).await, Some(ProjectState::Ready));
    assert_eq!(branch_state(&h, &id, &main).await, Some(BranchState::Ready));
    assert_eq!(h.neon.count("create_timeline"), 2);
    assert_eq!(
        h.neon
            .calls()
            .iter()
            .filter(|c| c.starts_with("create_timeline") && c.ends_with("-> already_exists"))
            .count(),
        1,
        "{:?}",
        h.neon.calls()
    );
}

/// A branch with an endpoint is not deleted: its operation waits, and no
/// timeline is deleted, until the endpoint is gone. Then one fenced batch
/// removes the branch, its name index, its guard, its roles and databases,
/// and decrements the parent's guard; the roles' secrets go after it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_waits_for_endpoints() {
    let h = harness!();
    let run = start(reconciler(&h, config(&h, "pg-control-a")));
    let (project, main, _) = ready_project(&h, "shop").await;
    let dev = ready_branch(&h, &project, "dev").await;
    let role = h
        .service
        .create_role(
            &user(),
            CreateRole {
                namespace: NS.into(),
                project_id: project.clone(),
                branch_id: dev.id.clone(),
                name: "app".into(),
                idempotency_key: "r1".into(),
            },
        )
        .await
        .expect("a role");
    h.service
        .create_database(
            &user(),
            CreateDatabase {
                namespace: NS.into(),
                project_id: project.clone(),
                branch_id: dev.id.clone(),
                name: "appdb".into(),
                owner_role: "app".into(),
                idempotency_key: "d1".into(),
            },
        )
        .await
        .expect("a database");
    assert!(h.secrets.value(&role.role.record.secret_ref).is_some());
    let ep = endpoint(&project, &dev.id);
    let ep_version = h
        .store
        .api_writer()
        .put(&ep, None)
        .await
        .expect("an endpoint");
    assert_eq!(
        guard(&h.store, &project, &main).await.map(|g| g.children),
        Some(1)
    );

    let op = h
        .service
        .delete_branch(
            &user(),
            DeleteBranch {
                namespace: NS.into(),
                project_id: project.clone(),
                branch_id: dev.id.clone(),
                idempotency_key: "del".into(),
                ..DeleteBranch::default()
            },
        )
        .await
        .expect("delete");
    eventually("the delete to wait for the endpoint", || async {
        h.service
            .get_operation(&op.id)
            .await
            .ok()
            .and_then(|o| o.progress)
            .is_some_and(|p| p.phase == "waiting_for_endpoints")
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(h.neon.count("delete_timeline"), 0);
    assert_eq!(
        branch_state(&h, &project, &dev.id).await,
        Some(BranchState::Deleting)
    );
    assert_eq!(op_state(&h, &op.id).await, OperationState::Running);

    h.store
        .api_writer()
        .delete::<EndpointRec>(
            &EndpointKey {
                project_id: project.clone(),
                id: ep.id.clone(),
            },
            ep_version,
        )
        .await
        .expect("the endpoint goes");
    eventually("the branch removed", || async {
        branch_state(&h, &project, &dev.id).await.is_none()
    })
    .await;
    assert_eq!(op_state(&h, &op.id).await, OperationState::Succeeded);
    assert_eq!(
        h.neon
            .count(&format!("delete_timeline {}", tl(dev.timeline_id))),
        1
    );
    assert!(guard(&h.store, &project, &dev.id).await.is_none());
    assert_eq!(
        guard(&h.store, &project, &main).await.map(|g| g.children),
        Some(0)
    );
    let name = h
        .store
        .get::<BranchNameRec>(&BranchNameKey {
            project_id: project.clone(),
            name: "dev".into(),
        })
        .await
        .expect("get");
    assert!(name.is_none(), "the name index goes with the branch");
    assert!(roles_of(&h.store, &dev.id).await.is_empty());
    assert!(databases_of(&h.store, &dev.id).await.is_empty());
    eventually("the role's secret deleted", || async {
        h.secrets.value(&role.role.record.secret_ref).is_none()
    })
    .await;
    run.stop().await;
}

/// `DeleteProject` removes every branch (children before their parent),
/// their roles, databases and secrets, then the project and its name
/// index; the operation succeeds and stays readable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_delete_removes_every_record() {
    let h = harness!();
    let run = start(reconciler(&h, config(&h, "pg-control-a")));
    let (project, main, _) = ready_project(&h, "shop").await;
    let role = h
        .service
        .create_role(
            &user(),
            CreateRole {
                namespace: NS.into(),
                project_id: project.clone(),
                branch_id: String::new(),
                name: "app".into(),
                idempotency_key: "r1".into(),
            },
        )
        .await
        .expect("a role on main");
    let dev = ready_branch(&h, &project, "dev").await;
    let copied = roles_of(&h.store, &dev.id).await;
    assert_eq!(copied.len(), 1, "dev inherits main's role");

    let op = h
        .service
        .delete_project(
            &admin(),
            DeleteProject {
                namespace: NS.into(),
                project_id: project.clone(),
                idempotency_key: "del".into(),
                ..DeleteProject::default()
            },
        )
        .await
        .expect("delete project");
    eventually("the project removed", || async {
        project_state(&h, &project).await.is_none()
    })
    .await;
    assert_eq!(op_state(&h, &op.id).await, OperationState::Succeeded);
    let deletes: Vec<String> = h
        .neon
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("delete_timeline"))
        .collect();
    let main_tl = deletes
        .iter()
        .position(|c| !c.contains(&tl(dev.timeline_id)))
        .expect("main's delete");
    let dev_at = deletes
        .iter()
        .position(|c| c.contains(&tl(dev.timeline_id)))
        .expect("dev's delete");
    assert!(dev_at < main_tl, "the child first: {deletes:?}");
    for b in [&main, &dev.id] {
        assert!(branch_state(&h, &project, b).await.is_none());
        assert!(guard(&h.store, &project, b).await.is_none());
        assert!(roles_of(&h.store, b).await.is_empty());
    }
    let name = h
        .store
        .get::<ProjectNameRec>(&ProjectNameKey {
            namespace: NS.into(),
            name: "shop".into(),
        })
        .await
        .expect("get");
    assert!(name.is_none(), "the name is free again");
    eventually("every secret deleted", || async {
        h.secrets.value(&role.role.record.secret_ref).is_none()
            && h.secrets.value(&copied[0].secret_ref).is_none()
    })
    .await;
    // The name can be taken again, by a new project.
    let again = h
        .service
        .create_project(&user(), create("shop", "k-again"))
        .await
        .expect("the name is free");
    assert_ne!(again.project.record.id, project);
    run.stop().await;
}

/// A permanent refusal (the pageserver's `lsn_out_of_retention`) fails the
/// branch and its operation with the reason; a transient one
/// (`storage_unavailable`) is retried until it passes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn neon_refusals_fail_or_retry() {
    let h = harness!();
    let run = start(reconciler(&h, config(&h, "pg-control-a")));
    let (project, _, _) = ready_project(&h, "shop").await;

    h.neon
        .fail_next("create_timeline", Reason::LsnOutOfRetention);
    let created = h
        .service
        .create_branch(&user(), child(&project, "old", "b1"))
        .await
        .expect("create");
    let id = created.branch.branch.record.id.clone();
    eventually("the branch failed", || async {
        branch_state(&h, &project, &id).await == Some(BranchState::Failed)
    })
    .await;
    let op = h
        .service
        .get_operation(&created.operation.id)
        .await
        .expect("op");
    assert_eq!(op.state, OperationState::Failed);
    assert_eq!(
        op.error.expect("an error").reason,
        Reason::LsnOutOfRetention.as_str()
    );

    h.neon
        .fail_next("create_timeline", Reason::StorageUnavailable);
    h.neon
        .fail_next("create_timeline", Reason::StorageUnavailable);
    let created = h
        .service
        .create_branch(&user(), child(&project, "dev", "b2"))
        .await
        .expect("create");
    let id = created.branch.branch.record.id.clone();
    eventually("the branch ready after retries", || async {
        branch_state(&h, &project, &id).await == Some(BranchState::Ready)
    })
    .await;
    assert_eq!(
        op_state(&h, &created.operation.id).await,
        OperationState::Succeeded
    );
    assert_eq!(h.neon.count("create_timeline -> storage_unavailable"), 2);
    run.stop().await;
}

/// A failed project create (the attach refused for good) fails the project,
/// `main` and the operation; deleting the failed project then removes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_project_create_can_be_deleted() {
    let h = harness!();
    let run = start(reconciler(&h, config(&h, "pg-control-a")));
    h.neon.fail_next("attach", Reason::InvalidArgument);
    let created = h
        .service
        .create_project(&user(), create("shop", "k1"))
        .await
        .expect("create");
    let id = created.project.record.id.clone();
    eventually("the project failed", || async {
        project_state(&h, &id).await == Some(ProjectState::Failed)
    })
    .await;
    let op = h
        .service
        .get_operation(&created.operation.id)
        .await
        .expect("op");
    assert_eq!(op.state, OperationState::Failed);
    let main = created.project.record.default_branch_id.expect("main");
    assert_eq!(
        branch_state(&h, &id, &main).await,
        Some(BranchState::Failed)
    );

    h.service
        .delete_project(
            &user(),
            DeleteProject {
                namespace: NS.into(),
                project_id: id.clone(),
                idempotency_key: "del".into(),
                ..DeleteProject::default()
            },
        )
        .await
        .expect("delete");
    eventually("the project removed", || async {
        project_state(&h, &id).await.is_none()
    })
    .await;
    run.stop().await;
}

/// `UpdateProject`'s `history_retention` reaches the tenant's
/// `pitr_interval`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retention_change_reaches_the_tenant() {
    let h = harness!();
    let run = start(reconciler(&h, config(&h, "pg-control-a")));
    let (project, _, tenant) = ready_project(&h, "shop").await;
    h.service
        .update_project(
            &user(),
            UpdateProject {
                namespace: NS.into(),
                project_id: project.clone(),
                history_retention: Some(Duration::from_secs(86_400)),
                idempotency_key: "u1".into(),
                ..UpdateProject::default()
            },
        )
        .await
        .expect("update");
    let t = TenantId(tenant);
    eventually("the new pitr_interval", || async {
        h.neon.count(&format!("tenant_config {t} pitr=86400")) == 1
    })
    .await;
    run.stop().await;
}

/// The resync's sweep deletes secrets no role names once they are older
/// than the grace period, including those of projects that are gone, and
/// keeps named, young and foreign ones.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resync_sweeps_orphan_secrets() {
    let h = harness!();
    let r = reconciler(&h, config(&h, "pg-control-a"));
    let created = h
        .service
        .create_project(&user(), create("shop", "k1"))
        .await
        .expect("create");
    let id = created.project.record.id.clone();
    assert!(matches!(r.reconcile_project(NS, &id).await, Ok(Pass::Done)));
    let role = h
        .service
        .create_role(
            &user(),
            CreateRole {
                namespace: NS.into(),
                project_id: id.clone(),
                branch_id: String::new(),
                name: "app".into(),
                idempotency_key: "r1".into(),
            },
        )
        .await
        .expect("a role");
    let project: ProjectId = id.parse().expect("an id");
    let orphan = SecretRef::new_role(&project);
    let gone = SecretRef::new_role(&ProjectId::new());
    let foreign = SecretRef::parse("not-a-role-secret").expect("a ref");
    for r in [&orphan, &gone, &foreign] {
        h.secrets
            .put(r, Secret::new(b"x".to_vec()))
            .await
            .expect("put");
    }
    // Young: nothing goes. (The test clock starts before today.)
    assert_eq!(r.sweep_secrets().await.expect("sweep"), 0);
    assert_eq!(h.secrets.len(), 4);

    // A day later, the two orphans go.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_millis();
    h.clock.0.store(
        u64::try_from(now).expect("ms") + 86_400_000,
        Ordering::SeqCst,
    );
    assert_eq!(r.sweep_secrets().await.expect("sweep"), 2);
    assert!(h.secrets.value(&role.role.record.secret_ref).is_some());
    assert!(h.secrets.value(&foreign).is_some());
    assert!(h.secrets.value(&orphan).is_none());
    assert!(h.secrets.value(&gone).is_none());
}
