//! The project RPCs of `pg-control` (PG2 Task 5) on the local store and a
//! fake Neon.

use std::time::Duration;

use crate::common::{FirstBatch, T0_MS, admin, harness_on, user};
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

async fn all_projects(h: &crate::common::Harness) -> Vec<Versioned<ProjectRec>> {
    let (page, next) = h
        .service
        .list_projects("acme", 0, "")
        .await
        .expect("list projects");
    assert!(next.is_empty());
    page
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_project_writes_creating_records_and_an_operation() {
    let h = harness!();
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
        .get_operation(&op.id)
        .await
        .expect("the operation");
    assert_eq!(&stored, op);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_project_replay_returns_same_operation() {
    let h = harness!();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_project_same_name_other_key_is_already_exists() {
    let h = harness!();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_key_for_another_request_is_invalid_argument() {
    let h = harness!();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_ledger_forgets_after_its_ttl() {
    let h = harness!();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_empty_key_is_never_replayed() {
    let h = harness!();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_names_and_settings_are_invalid_argument() {
    let h = harness!();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn history_retention_is_clamped() {
    let h = harness!();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_project_in_another_namespace_is_project_not_found() {
    let h = harness!();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_projects_paginates() {
    let h = harness!();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_project_renames_and_frees_the_old_name() {
    let h = harness!();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_project_marks_it_deleting_and_returns_an_operation() {
    let h = harness!();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_project_with_a_protected_branch_needs_admin() {
    let h = harness!();
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
/// `already_exists`, and makes one project. After the commit the first call
/// itself finds its ledger entry and answers; before it, the first call is
/// `unavailable` and the retry creates.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_project_undetermined_then_retry_returns_same_operation() {
    use std::sync::Arc;

    use loams_kv::FaultPoint;
    use loams_pg_control::KvControlStore;

    for point in [FaultPoint::AfterCommit, FaultPoint::BeforeCommit] {
        let Some(kv) = crate::factory().kv().await else {
            eprintln!("skipped: no store");
            return;
        };
        // No commit tokens, so the lost acknowledgement stays unresolved.
        let untokened = loams_pg_control::StoreOptions {
            commit_tokens: false,
            ..crate::common::options()
        };
        let faulty = KvControlStore::new(
            kv.clone().with_faults(Arc::new(FirstBatch { point })),
            untokened,
        );
        let first = harness_on(faulty)
            .service
            .create_project(&user(), create("shop", "k1"))
            .await;
        let h = harness_on(KvControlStore::new(kv, crate::common::options()));
        let retry = h
            .service
            .create_project(&user(), create("shop", "k1"))
            .await
            .unwrap_or_else(|e| panic!("{point:?}: the retry: {e}"));
        match point {
            FaultPoint::AfterCommit => {
                let first = first.expect("resolved through the ledger");
                assert_eq!(retry, first, "the retry answers the first response");
            }
            _ => {
                let e = first.expect_err("nothing applied: unknown to the caller");
                assert_eq!(e.reason, Reason::Unavailable, "{e}");
            }
        }
        let all = all_projects(&h).await;
        assert_eq!(all.len(), 1, "{point:?}");
        assert_eq!(all[0].record.id, retry.project.record.id);
    }
}

/// The literal R3.14 path, without timing: a hook commits a second call
/// under the same key after the first has read the ledger (empty) and
/// before its batch commits. The first's batch then conflicts at the store,
/// and it answers the second's operation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_conflict_on_the_ledger_answers_the_winner() {
    use std::sync::{Arc, Mutex};

    use crate::common::{FakeNeon, fresh_store, harness_with, once_before, plain_service};

    let Some(store) = fresh_store().await else {
        eprintln!("skipped: no store");
        return;
    };
    let winner = Arc::new(Mutex::new(None));
    let hook = {
        let (winner, store) = (winner.clone(), store.clone());
        once_before("CreateProject", move || {
            let (winner, other) = (winner.clone(), plain_service(&store, &FakeNeon::default()));
            async move {
                let won = other
                    .create_project(&user(), create("shop", "k1"))
                    .await
                    .expect("the second call");
                *winner.lock().expect("lock") = Some(won);
            }
        })
    };
    let h = harness_with(store, Some(hook));
    let first = h
        .service
        .create_project(&user(), create("shop", "k1"))
        .await
        .expect("the first call");
    let won = winner.lock().expect("lock").clone().expect("the hook ran");
    assert_eq!(first, won, "the loser answers the winner's response");
    assert_eq!(all_projects(&h).await.len(), 1);
}

/// Records carry when they were created and last written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn records_carry_their_update_time() {
    let h = harness!();
    let out = h
        .service
        .create_project(&user(), create("shop", "k1"))
        .await
        .expect("create");
    assert_eq!(out.project.record.created_at_ms, T0_MS);
    assert_eq!(out.project.record.updated_at_ms, T0_MS);
    h.clock.advance_ms(5_000);
    let updated = h
        .service
        .update_project(
            &user(),
            UpdateProject {
                namespace: "acme".into(),
                project_id: out.project.record.id.clone(),
                name: Some("store".into()),
                idempotency_key: "u1".into(),
                ..UpdateProject::default()
            },
        )
        .await
        .expect("update");
    assert_eq!(updated.record.created_at_ms, T0_MS);
    assert_eq!(updated.record.updated_at_ms, T0_MS + 5_000);
}

/// The ledger's entry for `(user, rpc, key)`, as stored.
async fn ledger_entry(
    h: &crate::common::Harness,
) -> Versioned<loams_pg_control::model::IdempotencyRec> {
    use loams_pg_control::Page;
    use loams_pg_control::model::{AllIdempotency, IdempotencyRec};
    let (all, _) = h
        .store
        .list::<IdempotencyRec>(&AllIdempotency, Page::default())
        .await
        .expect("the ledger");
    assert_eq!(all.len(), 1);
    all.into_iter().next().expect("an entry")
}

/// A replay survives a record gaining a field: the answer is JSON, and the
/// new field (here `updated_at_ms`, as if recorded before it existed)
/// takes its default.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replay_survives_a_record_gaining_a_field() {
    use loams_pg_control::service::ANSWER_JSON;

    let h = harness!();
    let first = h
        .service
        .create_project(&user(), create("shop", "k1"))
        .await
        .expect("create");
    let entry = ledger_entry(&h).await;
    assert_eq!(entry.record.answer[0], ANSWER_JSON);
    let mut json: serde_json::Value =
        serde_json::from_slice(&entry.record.answer[1..]).expect("JSON");
    let record = json["project"]["record"]
        .as_object_mut()
        .expect("the project");
    assert!(record.remove("updated_at_ms").is_some());
    let mut older = entry.record.clone();
    older.answer = vec![ANSWER_JSON];
    older
        .answer
        .extend(serde_json::to_vec(&json).expect("JSON"));
    h.store
        .api_writer()
        .put(&older, Some(entry.version))
        .await
        .expect("rewrite the entry");
    let replay = h
        .service
        .create_project(&user(), create("shop", "k1"))
        .await
        .expect("replay");
    assert_eq!(replay.operation, first.operation);
    assert_eq!(replay.project.record.updated_at_ms, 0);
    assert_eq!(replay.project.record.id, first.project.record.id);
}

/// An answer under a format tag this build does not know is refused
/// plainly, never misread.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_answer_in_an_unknown_format_is_failed_precondition() {
    let h = harness!();
    h.service
        .create_project(&user(), create("shop", "k1"))
        .await
        .expect("create");
    let entry = ledger_entry(&h).await;
    let mut other = entry.record.clone();
    other.answer[0] = 9;
    h.store
        .api_writer()
        .put(&other, Some(entry.version))
        .await
        .expect("rewrite the entry");
    let e = h
        .service
        .create_project(&user(), create("shop", "k1"))
        .await
        .expect_err("an unknown format");
    assert_eq!(e.reason, Reason::FailedPrecondition);
}

/// Operations are found by id alone, and listed per namespace in creation
/// order, through indexes written with them; a refused create writes none.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn operations_are_indexed_by_id_and_namespace() {
    use loams_pg_control::model::OperationState;

    let h = harness!();
    let mut ids = Vec::new();
    for i in 0..3 {
        let out = h
            .service
            .create_project(&user(), create(&format!("p{i}"), &format!("k{i}")))
            .await
            .expect("create");
        ids.push(out.operation.id);
    }
    h.service
        .create_project(
            &user(),
            CreateProject {
                namespace: "other".into(),
                ..create("q", "k9")
            },
        )
        .await
        .expect("another namespace");
    h.service
        .create_project(&user(), create("p0", "k-dup"))
        .await
        .expect_err("a taken name writes no operation");
    let op = h.service.get_operation(&ids[1]).await.expect("by id");
    assert_eq!(op.id, ids[1]);
    assert_eq!(op.namespace, "acme");
    let e = h
        .service
        .get_operation("op-00000000000000000000000000")
        .await
        .expect_err("absent");
    assert_eq!(e.reason, Reason::NotFound);

    let mut listed = Vec::new();
    let mut token = String::new();
    loop {
        let (page, next) = h
            .service
            .list_operations("acme", 2, &token)
            .await
            .expect("a page");
        listed.extend(page.into_iter().map(|o| o.id));
        if next.is_empty() {
            break;
        }
        token = next;
    }
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(listed, sorted, "acme's three, in id order");
    // The approval state Task 9 sets exists.
    assert_ne!(OperationState::AwaitingApproval, OperationState::Pending);
}

/// A mutation without a principal is refused before anything is read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_empty_principal_is_refused() {
    let h = harness!();
    let nobody = loams_pg_control::service::Caller {
        principal: String::new(),
        admin: true,
    };
    let e = h
        .service
        .create_project(&nobody, create("shop", "k1"))
        .await
        .expect_err("no principal");
    assert_eq!(e.reason, Reason::Unauthenticated);
    assert!(all_projects(&h).await.is_empty());
}

/// After its entry expired, a key's next call succeeds as a fresh one,
/// replacing the entry, and is replayed from then on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_expired_entry_is_replaced_by_a_fresh_call() {
    let h = harness!();
    let first = h
        .service
        .create_project(&user(), create("shop", "k1"))
        .await
        .expect("create");
    h.clock.advance_ms(24 * 3600 * 1000 + 1);
    let second = h
        .service
        .create_project(&user(), create("store", "k1"))
        .await
        .expect("a fresh call under the expired key");
    assert_ne!(second.operation.id, first.operation.id);
    let replay = h
        .service
        .create_project(&user(), create("store", "k1"))
        .await
        .expect("a replay of the new entry");
    assert_eq!(replay, second);
    let entry = ledger_entry(&h).await;
    assert_eq!(entry.record.created_at_ms, h.clock.now_ms());
    assert_eq!(h.service.ledger().prune(h.clock.now_ms()).await, Ok(0));
}

/// A branch protected between a non-admin's project delete reading the
/// branches and its commit fails that delete (the branches it read are
/// checked in its batch); its second try sees the protection.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_branch_protected_during_a_project_delete_conflicts() {
    use crate::common::{FakeNeon, fresh_store, harness_with, once_before, plain_service};

    let Some(store) = fresh_store().await else {
        eprintln!("skipped: no store");
        return;
    };
    let setup = plain_service(&store, &FakeNeon::default());
    let p = setup
        .create_project(&user(), create("shop", "k1"))
        .await
        .expect("create")
        .project
        .record;
    let main = p.default_branch_id.clone().expect("main");
    let hook = {
        let (store, pid) = (store.clone(), p.id.clone());
        once_before("DeleteProject", move || {
            let other = plain_service(&store, &FakeNeon::default());
            let req = UpdateBranch {
                namespace: "acme".into(),
                project_id: pid.clone(),
                branch_id: main.clone(),
                protected: Some(true),
                idempotency_key: "p1".into(),
                ..UpdateBranch::default()
            };
            async move {
                other.update_branch(&user(), req).await.expect("protect");
            }
        })
    };
    let h = harness_with(store, Some(hook));
    let e = h
        .service
        .delete_project(
            &user(),
            DeleteProject {
                namespace: "acme".into(),
                project_id: p.id.clone(),
                expected_version: None,
                idempotency_key: "d1".into(),
            },
        )
        .await
        .expect_err("protected meanwhile");
    assert_eq!(e.reason, Reason::BranchProtected, "{e}");
}
