//! The project RPCs of `pg-control` (PG2 Task 5) on the local store and a
//! fake Neon.

mod common;

use std::time::Duration;

use common::{T0_MS, admin, harness, harness_on, user};
use loams_pg_control::ids::{ProjectId, tenant_id};
use loams_pg_control::model::{
    BranchKey, BranchRec, BranchState, OperationRec, ProjectNameKey, ProjectNameRec, ProjectRec,
    ProjectState, WalService,
};
use loams_pg_control::service::branches::UpdateBranch;
use loams_pg_control::service::operations::{OperationKind, OperationState};
use loams_pg_control::service::projects::{CreateProject, DeleteProject, UpdateProject};
use loams_pg_control::service::{LEDGER_TTL, Reason};
use loams_pg_control::{PgControlStore, Versioned};

fn create(name: &str, key: &str) -> CreateProject {
    CreateProject {
        namespace: "acme".into(),
        name: name.into(),
        idempotency_key: key.into(),
        ..CreateProject::default()
    }
}

async fn all_projects(h: &common::Harness) -> Vec<Versioned<ProjectRec>> {
    let (page, next) = h
        .service
        .list_projects("acme", 0, "")
        .await
        .expect("list projects");
    assert!(next.is_empty());
    page
}

#[tokio::test]
async fn create_project_writes_creating_records_and_an_operation() {
    let h = harness().await;
    let out = h
        .service
        .create_project(&user(), create("shop", "k1"))
        .await
        .expect("create");
    let p = &out.project.record;
    let id: ProjectId = p.id.parse().expect("a project id");
    assert_eq!(p.namespace, "acme");
    assert_eq!(p.name, "shop");
    assert_eq!(p.state, ProjectState::Creating);
    assert_eq!(p.tenant_id, tenant_id(&id));
    assert_eq!(p.pg_version, 17);
    assert_eq!(p.history_retention_s, 7 * 24 * 3600);
    assert_eq!(
        p.wal,
        WalService::LoamsWal {
            pool: "default".into()
        }
    );
    assert_eq!(p.created_at_ms, T0_MS);

    // The default branch `main` is written with it, in `creating`.
    let main_id = p.default_branch_id.clone().expect("a default branch");
    let main = h
        .store
        .get::<BranchRec>(&BranchKey {
            project_id: p.id.clone(),
            id: main_id.clone(),
        })
        .await
        .expect("get")
        .expect("main");
    assert_eq!(main.record.name, "main");
    assert_eq!(main.record.state, BranchState::Creating);
    assert_eq!(main.record.parent_id, None);

    // The name index, and the operation the reconciler finishes (Task 7).
    let name = h
        .store
        .get::<ProjectNameRec>(&ProjectNameKey {
            namespace: "acme".into(),
            name: "shop".into(),
        })
        .await
        .expect("get")
        .expect("the name index");
    assert_eq!(name.record.project_id, p.id);
    let op: &OperationRec = &out.operation;
    assert!(
        op.id.starts_with("op-") && op.id.len() == 3 + 26,
        "{}",
        op.id
    );
    assert_eq!(op.kind, OperationKind::ProjectCreate);
    assert_eq!(op.kind.as_str(), "postgres.project.create");
    assert_eq!(op.state, OperationState::Pending);
    assert_eq!(op.project_id, p.id);
    let stored = h
        .service
        .get_operation("acme", &p.id, &op.id)
        .await
        .expect("the operation");
    assert_eq!(&stored, op);
}

#[tokio::test]
async fn create_project_replay_returns_same_operation() {
    let h = harness().await;
    let first = h
        .service
        .create_project(&user(), create("shop", "k1"))
        .await
        .expect("create");
    h.clock.advance_ms(60_000);
    let again = h
        .service
        .create_project(&user(), create("shop", "k1"))
        .await
        .expect("replay");
    assert_eq!(again, first, "a replay answers the first response");
    assert_eq!(all_projects(&h).await.len(), 1);
}

#[tokio::test]
async fn create_project_same_name_other_key_is_already_exists() {
    let h = harness().await;
    h.service
        .create_project(&user(), create("shop", "k1"))
        .await
        .expect("create");
    let e = h
        .service
        .create_project(&user(), create("shop", "k2"))
        .await
        .expect_err("a taken name");
    assert_eq!(e.reason, Reason::AlreadyExists);
    assert_eq!(e.reason.as_str(), "already_exists");
    assert_eq!(all_projects(&h).await.len(), 1);
}

#[tokio::test]
async fn same_key_for_another_request_is_invalid_argument() {
    let h = harness().await;
    h.service
        .create_project(&user(), create("shop", "k1"))
        .await
        .expect("create");
    let e = h
        .service
        .create_project(&user(), create("other", "k1"))
        .await
        .expect_err("another request under the key");
    assert_eq!(e.reason, Reason::InvalidArgument);
    assert_eq!(
        e.metadata.get("field").map(String::as_str),
        Some("idempotency_key")
    );
    // Another principal's key is its own.
    let other = loams_pg_control::service::Caller {
        principal: "user:bob".into(),
        admin: false,
    };
    h.service
        .create_project(&other, create("other", "k1"))
        .await
        .expect("bob's own key");
}

#[tokio::test]
async fn the_ledger_forgets_after_its_ttl() {
    let h = harness().await;
    let first = h
        .service
        .create_project(&user(), create("shop", "k1"))
        .await
        .expect("create");
    assert_eq!(LEDGER_TTL, Duration::from_secs(24 * 3600));
    h.clock.advance_ms(24 * 3600 * 1000 - 1);
    let replay = h
        .service
        .create_project(&user(), create("shop", "k1"))
        .await
        .expect("replay inside 24 h");
    assert_eq!(replay, first);
    h.clock.advance_ms(2);
    // Past 24 h the key is fresh, and the name is taken.
    let e = h
        .service
        .create_project(&user(), create("shop", "k1"))
        .await
        .expect_err("a fresh create");
    assert_eq!(e.reason, Reason::AlreadyExists);
    // Pruning removes the expired entry.
    assert_eq!(h.service.ledger().prune(h.clock.now_ms()).await, Ok(1));
    assert_eq!(h.service.ledger().prune(h.clock.now_ms()).await, Ok(0));
}

#[tokio::test]
async fn an_empty_key_is_never_replayed() {
    let h = harness().await;
    h.service
        .create_project(&user(), create("shop", ""))
        .await
        .expect("create");
    let e = h
        .service
        .create_project(&user(), create("shop", ""))
        .await
        .expect_err("not a replay");
    assert_eq!(e.reason, Reason::AlreadyExists);
}

#[tokio::test]
async fn bad_names_and_settings_are_invalid_argument() {
    let h = harness().await;
    for (req, field) in [
        (create("Shop", "k1"), "name"),
        (create("a__b", "k2"), "name"),
        (create("", "k3"), "name"),
        (
            CreateProject {
                namespace: "".into(),
                ..create("shop", "k4")
            },
            "namespace",
        ),
        (
            CreateProject {
                pg_version: 15,
                ..create("shop", "k5")
            },
            "pg_version",
        ),
    ] {
        let e = h
            .service
            .create_project(&user(), req)
            .await
            .expect_err(field);
        assert_eq!(e.reason, Reason::InvalidArgument, "{field}");
        assert_eq!(e.metadata.get("field").map(String::as_str), Some(field));
    }
    assert!(all_projects(&h).await.is_empty());
}

#[tokio::test]
async fn history_retention_is_clamped() {
    let h = harness().await;
    let out = h
        .service
        .create_project(
            &user(),
            CreateProject {
                history_retention: Some(Duration::from_secs(365 * 24 * 3600)),
                ..create("shop", "k1")
            },
        )
        .await
        .expect("create");
    assert_eq!(out.project.record.history_retention_s, 30 * 24 * 3600);
}

#[tokio::test]
async fn get_project_in_another_namespace_is_project_not_found() {
    let h = harness().await;
    let out = h
        .service
        .create_project(&user(), create("shop", "k1"))
        .await
        .expect("create");
    let id = out.project.record.id.clone();
    assert_eq!(
        h.service.get_project("acme", &id).await.expect("get"),
        out.project
    );
    let e = h
        .service
        .get_project("other", &id)
        .await
        .expect_err("another namespace");
    assert_eq!(e.reason, Reason::ProjectNotFound);
    assert_eq!(e.reason.as_str(), "project_not_found");
    assert_eq!(e.metadata.get("project"), Some(&id));
    let e = h
        .service
        .get_project("acme", "nope")
        .await
        .expect_err("a bad id");
    assert_eq!(e.reason, Reason::InvalidArgument);
}

#[tokio::test]
async fn list_projects_paginates() {
    let h = harness().await;
    for i in 0..5 {
        h.service
            .create_project(&user(), create(&format!("p{i}"), &format!("k{i}")))
            .await
            .expect("create");
    }
    let mut seen = Vec::new();
    let mut token = String::new();
    loop {
        let (page, next) = h
            .service
            .list_projects("acme", 2, &token)
            .await
            .expect("a page");
        assert!(page.len() <= 2);
        seen.extend(page.into_iter().map(|p| p.record.name));
        if next.is_empty() {
            break;
        }
        token = next;
    }
    seen.sort();
    assert_eq!(seen, ["p0", "p1", "p2", "p3", "p4"]);
    let e = h
        .service
        .list_projects("acme", -1, "")
        .await
        .expect_err("a negative page size");
    assert_eq!(e.reason, Reason::InvalidArgument);
}

#[tokio::test]
async fn update_project_renames_and_frees_the_old_name() {
    let h = harness().await;
    let out = h
        .service
        .create_project(&user(), create("shop", "k1"))
        .await
        .expect("create");
    let id = out.project.record.id.clone();
    let e = h
        .service
        .update_project(
            &user(),
            UpdateProject {
                namespace: "acme".into(),
                project_id: id.clone(),
                idempotency_key: "u0".into(),
                ..UpdateProject::default()
            },
        )
        .await
        .expect_err("an empty mask");
    assert_eq!(e.reason, Reason::InvalidArgument);
    assert_eq!(
        e.metadata.get("field").map(String::as_str),
        Some("update_mask")
    );
    let e = h
        .service
        .update_project(
            &user(),
            UpdateProject {
                namespace: "acme".into(),
                project_id: id.clone(),
                name: Some("store".into()),
                expected_version: Some(out.project.version + 1),
                idempotency_key: "u1".into(),
                ..UpdateProject::default()
            },
        )
        .await
        .expect_err("a stale version");
    assert_eq!(e.reason, Reason::Aborted);
    let updated = h
        .service
        .update_project(
            &user(),
            UpdateProject {
                namespace: "acme".into(),
                project_id: id.clone(),
                name: Some("store".into()),
                history_retention: Some(Duration::from_secs(3600)),
                expected_version: Some(out.project.version),
                idempotency_key: "u2".into(),
            },
        )
        .await
        .expect("rename");
    assert_eq!(updated.record.name, "store");
    assert_eq!(updated.record.history_retention_s, 3600);
    assert!(updated.version > out.project.version);
    // The old name is free again, the new one taken.
    h.service
        .create_project(&user(), create("shop", "k2"))
        .await
        .expect("the old name is free");
    let e = h
        .service
        .create_project(&user(), create("store", "k3"))
        .await
        .expect_err("the new name is taken");
    assert_eq!(e.reason, Reason::AlreadyExists);
}

#[tokio::test]
async fn delete_project_marks_it_deleting_and_returns_an_operation() {
    let h = harness().await;
    let out = h
        .service
        .create_project(&user(), create("shop", "k1"))
        .await
        .expect("create");
    let id = out.project.record.id.clone();
    let req = DeleteProject {
        namespace: "acme".into(),
        project_id: id.clone(),
        expected_version: None,
        idempotency_key: "d1".into(),
    };
    let op = h
        .service
        .delete_project(&user(), req.clone())
        .await
        .expect("delete");
    assert_eq!(op.kind, OperationKind::ProjectDelete);
    assert_eq!(
        h.service
            .delete_project(&user(), req)
            .await
            .expect("replay"),
        op
    );
    let p = h.service.get_project("acme", &id).await.expect("get");
    assert_eq!(p.record.state, ProjectState::Deleting);
    let e = h
        .service
        .create_branch(
            &user(),
            loams_pg_control::service::branches::CreateBranch {
                namespace: "acme".into(),
                project_id: id.clone(),
                name: "dev".into(),
                idempotency_key: "b1".into(),
                ..Default::default()
            },
        )
        .await
        .expect_err("a deleting project takes no branch");
    assert_eq!(e.reason, Reason::FailedPrecondition);
}

#[tokio::test]
async fn delete_project_with_a_protected_branch_needs_admin() {
    let h = harness().await;
    let out = h
        .service
        .create_project(&user(), create("shop", "k1"))
        .await
        .expect("create");
    let id = out.project.record.id.clone();
    let main = out.project.record.default_branch_id.clone().expect("main");
    h.service
        .update_branch(
            &user(),
            UpdateBranch {
                namespace: "acme".into(),
                project_id: id.clone(),
                branch_id: main,
                protected: Some(true),
                idempotency_key: "p1".into(),
                ..UpdateBranch::default()
            },
        )
        .await
        .expect("protect");
    let req = DeleteProject {
        namespace: "acme".into(),
        project_id: id.clone(),
        expected_version: None,
        idempotency_key: "d1".into(),
    };
    let e = h
        .service
        .delete_project(&user(), req.clone())
        .await
        .expect_err("protected");
    assert_eq!(e.reason, Reason::BranchProtected);
    h.service
        .delete_project(&admin(), req)
        .await
        .expect("an admin may");
}

/// R3.14: a create whose write came back `Undetermined` is answered, on a
/// retry with the same key, with the first call's operation, never
/// `already_exists`, and makes one project.
#[tokio::test]
async fn create_project_undetermined_then_retry_returns_same_operation() {
    use std::sync::Arc;

    use loams_kv::FaultPoint;
    use loams_pg_control::KvControlStore;
    use loams_pg_control::store::conformance::local_factory;

    for point in [FaultPoint::AfterCommit, FaultPoint::BeforeCommit] {
        let kv = local_factory(env!("CARGO_TARGET_TMPDIR"))
            .kv()
            .await
            .expect("a local store");
        // No commit tokens, so the lost acknowledgement stays unresolved.
        let untokened = loams_pg_control::StoreOptions {
            commit_tokens: false,
            ..common::options()
        };
        let faulty = KvControlStore::new(
            kv.clone().with_faults(Arc::new(FirstBatch { point })),
            untokened,
        );
        let first = harness_on(faulty)
            .service
            .create_project(&user(), create("shop", "k1"))
            .await;
        let h = harness_on(KvControlStore::new(kv, common::options()));
        let retry = h
            .service
            .create_project(&user(), create("shop", "k1"))
            .await
            .unwrap_or_else(|e| panic!("{point:?}: the retry: {e}"));
        match first {
            Ok(first) => assert_eq!(retry.operation.id, first.operation.id, "{point:?}"),
            Err(e) => assert_eq!(e.reason, Reason::Unavailable, "{point:?}: {e}"),
        }
        if point == FaultPoint::AfterCommit {
            // The write applied, so the first call's answer is the record's.
            let names = all_projects(&h).await;
            assert_eq!(names[0].record.id, retry.project.record.id);
        }
        assert_eq!(all_projects(&h).await.len(), 1, "{point:?}");
    }
}

/// The literal R3.14 path: two calls under one key both miss the ledger, the
/// second's write meets the first's at the store (`Conflict` on the ledger
/// entry), and it answers the first's operation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_conflict_on_the_ledger_answers_the_winner() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use loams_kv::{Fault, FaultPlan, FaultPoint};
    use loams_pg_control::KvControlStore;
    use loams_pg_control::store::conformance::local_factory;

    /// Delays the first batch's first commit, so the second call commits
    /// first.
    #[derive(Debug, Default)]
    struct SlowFirst(AtomicU32);
    impl FaultPlan for SlowFirst {
        fn at(&self, op: &str, point: FaultPoint, attempt: u32) -> Option<Fault> {
            (op == "pg.batch"
                && point == FaultPoint::BeforeCommit
                && attempt == 1
                && self.0.fetch_add(1, Ordering::SeqCst) == 0)
                .then_some(Fault::Delay(Duration::from_millis(400)))
        }
    }

    let kv = local_factory(env!("CARGO_TARGET_TMPDIR"))
        .kv()
        .await
        .expect("a local store");
    let store = KvControlStore::new(
        kv.with_faults(Arc::new(SlowFirst::default())),
        common::options(),
    );
    let h = Arc::new(harness_on(store));
    let slow = {
        let h = h.clone();
        tokio::spawn(async move {
            h.service
                .create_project(&user(), create("shop", "k1"))
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    let fast = h
        .service
        .create_project(&user(), create("shop", "k1"))
        .await
        .expect("the fast call");
    let slow = slow.await.expect("join").expect("the slow call");
    assert_eq!(slow.operation.id, fast.operation.id);
    assert_eq!(slow.project, fast.project);
    assert_eq!(all_projects(&h).await.len(), 1);
}

/// `LoseAck` at `point` of the first attempt of every `pg.batch`.
#[derive(Debug)]
struct FirstBatch {
    point: loams_kv::FaultPoint,
}

impl loams_kv::FaultPlan for FirstBatch {
    fn at(&self, op: &str, point: loams_kv::FaultPoint, attempt: u32) -> Option<loams_kv::Fault> {
        (op == "pg.batch" && point == self.point && attempt == 1)
            .then_some(loams_kv::Fault::LoseAck)
    }
}
