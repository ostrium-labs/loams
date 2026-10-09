//! The branch RPCs of `pg-control` (PG2 Task 5) on the local store and a
//! fake Neon.

use std::time::Duration;

use crate::common::{Harness, T0_MS, admin, mark_branch_ready, user};
use loams_pg_control::ids::{BranchId, ProjectId, tenant_id, timeline_id};
use loams_pg_control::model::{BranchState, ProjectRec};
use loams_pg_control::neon::{Lsn, LsnAtTime, TimelineView, WalHeads};
use loams_pg_control::service::Reason;
use loams_pg_control::service::branches::{
    BranchPoint, CreateBranch, DeleteBranch, SetDefaultBranch, UpdateBranch,
};
use loams_pg_control::service::operations::{OperationKind, OperationState};
use loams_pg_control::service::projects::CreateProject;

/// A project `shop` in `acme`; its id and its default branch's.
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

fn branch(project_id: &str, name: &str, key: &str) -> CreateBranch {
    CreateBranch {
        namespace: "acme".into(),
        project_id: project_id.into(),
        name: name.into(),
        idempotency_key: key.into(),
        ..CreateBranch::default()
    }
}

fn ids(p: &ProjectRec, branch_id: &str) -> ([u8; 16], [u8; 16]) {
    let pid: ProjectId = p.id.parse().expect("a project id");
    let bid: BranchId = branch_id.parse().expect("a branch id");
    (tenant_id(&pid), timeline_id(&bid))
}

fn timeline(min_readable: u64, last_record: u64) -> TimelineView {
    TimelineView {
        last_record_lsn: Lsn(last_record),
        min_readable_lsn: Lsn(min_readable),
        logical_size_bytes: 8 << 20,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_branch_defaults_to_the_default_branch_at_its_head() {
    let h = harness!();
    let (p, main) = project(&h).await;
    let out = h
        .service
        .create_branch(&user(), branch(&p.id, "dev", "b1"))
        .await
        .expect("create");
    let b = &out.branch.branch.record;
    let bid: BranchId = b.id.parse().expect("a branch id");
    assert_eq!(b.project_id, p.id);
    assert_eq!(b.name, "dev");
    assert_eq!(b.parent_id.as_deref(), Some(main.as_str()));
    assert_eq!(
        b.ancestor_lsn, None,
        "the head: the reconciler branches there"
    );
    assert_eq!(b.timeline_id, timeline_id(&bid));
    assert_eq!(b.state, BranchState::Creating);
    assert_eq!(b.created_at_ms, T0_MS);
    assert!(!out.branch.is_default);
    assert_eq!(out.operation.kind, OperationKind::BranchCreate);
    assert_eq!(out.operation.state, OperationState::Pending);
    assert_eq!(out.operation.branch_id.as_deref(), Some(b.id.as_str()));
    // No point, so Neon is not asked.
    assert!(h.neon.calls().is_empty(), "{:?}", h.neon.calls());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_branch_replay_returns_same_operation() {
    let h = harness!();
    let (p, _) = project(&h).await;
    let first = h
        .service
        .create_branch(&user(), branch(&p.id, "dev", "b1"))
        .await
        .expect("create");
    let again = h
        .service
        .create_branch(&user(), branch(&p.id, "dev", "b1"))
        .await
        .expect("replay");
    assert_eq!(again, first);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_branch_same_name_other_key_is_already_exists() {
    let h = harness!();
    let (p, _) = project(&h).await;
    h.service
        .create_branch(&user(), branch(&p.id, "dev", "b1"))
        .await
        .expect("create");
    let e = h
        .service
        .create_branch(&user(), branch(&p.id, "dev", "b2"))
        .await
        .expect_err("a taken name");
    assert_eq!(e.reason, Reason::AlreadyExists);
    // `main` is taken too.
    let e = h
        .service
        .create_branch(&user(), branch(&p.id, "main", "b3"))
        .await
        .expect_err("main is taken");
    assert_eq!(e.reason, Reason::AlreadyExists);
    let (all, _) = h
        .service
        .list_branches("acme", &p.id, 0, "")
        .await
        .expect("list");
    assert_eq!(all.len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_branch_at_timestamp_resolves_lsn() {
    let h = harness!();
    let (p, main) = project(&h).await;
    mark_branch_ready(&h.store, &p.id, &main).await;
    let (t, tl) = ids(&p, &main);
    h.neon
        .set_lsn_at_time(t, tl, LsnAtTime::Present(Lsn(0x0169_AD58)));
    h.neon
        .set_timeline(t, tl, timeline(0x0100_0000, 0x0200_0000));
    let at = T0_MS - 3_600_000;
    let out = h
        .service
        .create_branch(
            &user(),
            CreateBranch {
                point: BranchPoint::Time(at),
                ..branch(&p.id, "an-hour-ago", "b1")
            },
        )
        .await
        .expect("create at a time");
    assert_eq!(out.branch.branch.record.ancestor_lsn, Some(0x0169_AD58));
    assert!(
        h.neon.calls().contains(&format!("lsn_by_timestamp {at}")),
        "{:?}",
        h.neon.calls()
    );
    assert_eq!(Lsn(0x0169_AD58).to_string(), "0/169AD58");

    // At an LSN, in "X/Y" form.
    let out = h
        .service
        .create_branch(
            &user(),
            CreateBranch {
                point: BranchPoint::Lsn("0/1800000".into()),
                ..branch(&p.id, "at-lsn", "b2")
            },
        )
        .await
        .expect("create at an LSN");
    assert_eq!(out.branch.branch.record.ancestor_lsn, Some(0x0180_0000));

    // A time Neon has no WAL for yet is the head.
    h.neon
        .set_lsn_at_time(t, tl, LsnAtTime::Future(Lsn(0x0200_0000)));
    let out = h
        .service
        .create_branch(
            &user(),
            CreateBranch {
                point: BranchPoint::Time(T0_MS),
                ..branch(&p.id, "now", "b3")
            },
        )
        .await
        .expect("create at now");
    assert_eq!(out.branch.branch.record.ancestor_lsn, Some(0x0200_0000));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lsn_out_of_retention_refused() {
    let h = harness!();
    let (p, main) = project(&h).await;
    mark_branch_ready(&h.store, &p.id, &main).await;
    let (t, tl) = ids(&p, &main);
    h.neon
        .set_timeline(t, tl, timeline(0x0200_0000, 0x0300_0000));

    // An LSN below what the pageserver keeps.
    let e = h
        .service
        .create_branch(
            &user(),
            CreateBranch {
                point: BranchPoint::Lsn("0/1000000".into()),
                ..branch(&p.id, "old", "b1")
            },
        )
        .await
        .expect_err("out of retention");
    assert_eq!(e.reason, Reason::LsnOutOfRetention);
    assert_eq!(e.reason.as_str(), "lsn_out_of_retention");
    assert_eq!(
        e.metadata.get("oldest_lsn").map(String::as_str),
        Some("0/2000000")
    );
    assert_eq!(
        e.metadata.get("history_retention").map(String::as_str),
        Some("604800s")
    );

    // A time before the project's history retention.
    let e = h
        .service
        .create_branch(
            &user(),
            CreateBranch {
                point: BranchPoint::Time(T0_MS - 8 * 24 * 3_600_000),
                ..branch(&p.id, "older", "b2")
            },
        )
        .await
        .expect_err("out of retention");
    assert_eq!(e.reason, Reason::LsnOutOfRetention);

    // A time older than any WAL Neon has.
    h.neon.set_lsn_at_time(t, tl, LsnAtTime::Past);
    let e = h
        .service
        .create_branch(
            &user(),
            CreateBranch {
                point: BranchPoint::Time(T0_MS - 60_000),
                ..branch(&p.id, "before-data", "b3")
            },
        )
        .await
        .expect_err("past");
    assert_eq!(e.reason, Reason::LsnOutOfRetention);

    // Past the parent's head is not a point in its history.
    let e = h
        .service
        .create_branch(
            &user(),
            CreateBranch {
                point: BranchPoint::Lsn("0/4000000".into()),
                ..branch(&p.id, "future", "b4")
            },
        )
        .await
        .expect_err("past the head");
    assert_eq!(e.reason, Reason::InvalidArgument);
    let e = h
        .service
        .create_branch(
            &user(),
            CreateBranch {
                point: BranchPoint::Lsn("nonsense".into()),
                ..branch(&p.id, "bad", "b5")
            },
        )
        .await
        .expect_err("unparseable");
    assert_eq!(e.reason, Reason::InvalidArgument);
    assert_eq!(e.metadata.get("field").map(String::as_str), Some("lsn"));
    let (all, _) = h
        .service
        .list_branches("acme", &p.id, 0, "")
        .await
        .expect("list");
    assert_eq!(all.len(), 1, "nothing was created");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_point_needs_a_ready_parent() {
    let h = harness!();
    let (p, _) = project(&h).await;
    let e = h
        .service
        .create_branch(
            &user(),
            CreateBranch {
                point: BranchPoint::Lsn("0/1000000".into()),
                ..branch(&p.id, "early", "b1")
            },
        )
        .await
        .expect_err("main is still creating");
    assert_eq!(e.reason, Reason::FailedPrecondition);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_branch_with_children_refused() {
    let h = harness!();
    let (p, _) = project(&h).await;
    let dev = h
        .service
        .create_branch(&user(), branch(&p.id, "dev", "b1"))
        .await
        .expect("dev");
    let dev_id = dev.branch.branch.record.id.clone();
    let child = h
        .service
        .create_branch(
            &user(),
            CreateBranch {
                parent_id: dev_id.clone(),
                ..branch(&p.id, "feature", "b2")
            },
        )
        .await
        .expect("a child of dev");
    assert_eq!(
        child.branch.branch.record.parent_id.as_deref(),
        Some(dev_id.as_str())
    );
    let e = h
        .service
        .delete_branch(
            &user(),
            DeleteBranch {
                namespace: "acme".into(),
                project_id: p.id.clone(),
                branch_id: dev_id.clone(),
                expected_version: None,
                idempotency_key: "d1".into(),
            },
        )
        .await
        .expect_err("dev has a child");
    assert_eq!(e.reason, Reason::BranchHasChildren);
    assert_eq!(e.reason.as_str(), "branch_has_children");
    assert_eq!(e.metadata.get("children").map(String::as_str), Some("1"));

    // The child goes first; then dev may.
    let op = h
        .service
        .delete_branch(
            &user(),
            DeleteBranch {
                namespace: "acme".into(),
                project_id: p.id.clone(),
                branch_id: child.branch.branch.record.id.clone(),
                expected_version: None,
                idempotency_key: "d2".into(),
            },
        )
        .await
        .expect("delete the child");
    assert_eq!(op.kind, OperationKind::BranchDelete);
    let got = h
        .service
        .get_branch("acme", &p.id, &child.branch.branch.record.id)
        .await
        .expect("get");
    assert_eq!(got.branch.record.state, BranchState::Deleting);
    // Still a child until the reconciler removes it (Task 7).
    let e = h
        .service
        .delete_branch(
            &user(),
            DeleteBranch {
                namespace: "acme".into(),
                project_id: p.id.clone(),
                branch_id: dev_id.clone(),
                expected_version: None,
                idempotency_key: "d3".into(),
            },
        )
        .await
        .expect_err("the child is not gone yet");
    assert_eq!(e.reason, Reason::BranchHasChildren);
    // A deleting branch takes no children.
    let e = h
        .service
        .create_branch(
            &user(),
            CreateBranch {
                parent_id: child.branch.branch.record.id.clone(),
                ..branch(&p.id, "grandchild", "b3")
            },
        )
        .await
        .expect_err("a deleting parent");
    assert_eq!(e.reason, Reason::FailedPrecondition);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn protected_branch_delete_needs_admin() {
    let h = harness!();
    let (p, _) = project(&h).await;
    let out = h
        .service
        .create_branch(
            &user(),
            CreateBranch {
                protected: true,
                ..branch(&p.id, "prod", "b1")
            },
        )
        .await
        .expect("create");
    let id = out.branch.branch.record.id.clone();
    assert!(out.branch.branch.record.protected);
    let req = DeleteBranch {
        namespace: "acme".into(),
        project_id: p.id.clone(),
        branch_id: id.clone(),
        expected_version: None,
        idempotency_key: "d1".into(),
    };
    let e = h
        .service
        .delete_branch(&user(), req.clone())
        .await
        .expect_err("protected");
    assert_eq!(e.reason, Reason::BranchProtected);
    assert_eq!(e.reason.as_str(), "branch_protected");
    assert_eq!(e.metadata.get("branch"), Some(&id));
    // Nor may a non-admin lift the protection.
    let e = h
        .service
        .update_branch(
            &user(),
            UpdateBranch {
                namespace: "acme".into(),
                project_id: p.id.clone(),
                branch_id: id.clone(),
                protected: Some(false),
                idempotency_key: "u1".into(),
                ..UpdateBranch::default()
            },
        )
        .await
        .expect_err("unprotect");
    assert_eq!(e.reason, Reason::PermissionDenied);
    h.service
        .delete_branch(&admin(), req)
        .await
        .expect("an admin may");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_default_branch_cannot_be_deleted() {
    let h = harness!();
    let (p, main) = project(&h).await;
    let e = h
        .service
        .delete_branch(
            &admin(),
            DeleteBranch {
                namespace: "acme".into(),
                project_id: p.id.clone(),
                branch_id: main,
                expected_version: None,
                idempotency_key: "d1".into(),
            },
        )
        .await
        .expect_err("the default branch");
    assert_eq!(e.reason, Reason::FailedPrecondition);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_default_branch_moves_the_default() {
    let h = harness!();
    let (p, main) = project(&h).await;
    let dev = h
        .service
        .create_branch(&user(), branch(&p.id, "dev", "b1"))
        .await
        .expect("dev");
    let dev_id = dev.branch.branch.record.id.clone();
    let updated = h
        .service
        .set_default_branch(
            &user(),
            SetDefaultBranch {
                namespace: "acme".into(),
                project_id: p.id.clone(),
                branch_id: dev_id.clone(),
                expected_version: None,
                idempotency_key: "s1".into(),
            },
        )
        .await
        .expect("set default");
    assert_eq!(
        updated.record.default_branch_id.as_deref(),
        Some(dev_id.as_str())
    );
    let got = h
        .service
        .get_branch("acme", &p.id, &dev_id)
        .await
        .expect("get");
    assert!(got.is_default);
    // An empty parent now means dev.
    let child = h
        .service
        .create_branch(&user(), branch(&p.id, "feature", "b2"))
        .await
        .expect("child");
    assert_eq!(
        child.branch.branch.record.parent_id.as_deref(),
        Some(dev_id.as_str())
    );
    // main is an ordinary branch now; dev is not deletable.
    let e = h
        .service
        .delete_branch(
            &admin(),
            DeleteBranch {
                namespace: "acme".into(),
                project_id: p.id.clone(),
                branch_id: dev_id,
                expected_version: None,
                idempotency_key: "d1".into(),
            },
        )
        .await
        .expect_err("the new default");
    assert_eq!(e.reason, Reason::FailedPrecondition);
    let _ = main;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_branches_paginates() {
    let h = harness!();
    let (p, _) = project(&h).await;
    for i in 0..6 {
        h.service
            .create_branch(&user(), branch(&p.id, &format!("b{i}"), &format!("k{i}")))
            .await
            .expect("create");
    }
    let mut names = Vec::new();
    let mut token = String::new();
    let mut pages = 0;
    loop {
        let (page, next) = h
            .service
            .list_branches("acme", &p.id, 3, &token)
            .await
            .expect("a page");
        assert!(page.len() <= 3);
        pages += 1;
        names.extend(page.into_iter().map(|b| b.branch.record.name));
        if next.is_empty() {
            break;
        }
        token = next;
    }
    assert_eq!(pages, 3, "7 branches in pages of 3");
    names.sort();
    assert_eq!(names, ["b0", "b1", "b2", "b3", "b4", "b5", "main"]);
    let e = h
        .service
        .list_branches("acme", &p.id, 3, "zz")
        .await
        .expect_err("a bad token");
    assert_eq!(e.reason, Reason::InvalidArgument);
    let e = h
        .service
        .list_branches("other", &p.id, 3, "")
        .await
        .expect_err("another namespace");
    assert_eq!(e.reason, Reason::ProjectNotFound);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_branch_reports_wal_heads() {
    let h = harness!();
    let (p, main) = project(&h).await;
    // While creating there is no timeline to ask about.
    let got = h
        .service
        .get_branch("acme", &p.id, &main)
        .await
        .expect("get");
    assert!(got.is_default);
    assert_eq!(got.wal, None);
    assert!(h.neon.calls().is_empty());

    mark_branch_ready(&h.store, &p.id, &main).await;
    let (t, tl) = ids(&p, &main);
    let heads = WalHeads {
        commit_lsn: Lsn(0x0300_0010),
        flush_lsn: Lsn(0x0300_0020),
        remote_consistent_lsn: Lsn(0x0200_0000),
        backup_lsn: Lsn(0x0100_0000),
    };
    h.neon.set_wal(t, tl, heads.clone());
    h.neon.set_timeline(t, tl, timeline(0, 0x0300_0000));
    let got = h
        .service
        .get_branch("acme", &p.id, &main)
        .await
        .expect("get");
    assert_eq!(got.wal, Some(heads));
    assert_eq!(got.logical_size_bytes, Some(8 << 20));
    assert!(h.neon.calls().contains(&"wal_heads".to_string()));
    assert_eq!(Lsn(0x0300_0010).to_string(), "0/3000010");

    // loams-wal does not know the timeline: the record still answers,
    // without heads.
    let dev = h
        .service
        .create_branch(&user(), branch(&p.id, "dev", "b1"))
        .await
        .expect("dev");
    let dev_id = dev.branch.branch.record.id.clone();
    mark_branch_ready(&h.store, &p.id, &dev_id).await;
    let got = h
        .service
        .get_branch("acme", &p.id, &dev_id)
        .await
        .expect("get without heads");
    assert_eq!(got.wal, None);
    let e = h
        .service
        .get_branch("acme", &p.id, &BranchId::new().to_string())
        .await
        .expect_err("no such branch");
    assert_eq!(e.reason, Reason::NotFound);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_branch_renames_and_sets_a_ttl() {
    let h = harness!();
    let (p, _) = project(&h).await;
    let out = h
        .service
        .create_branch(
            &user(),
            CreateBranch {
                ttl: Some(Duration::from_secs(3600)),
                ..branch(&p.id, "dev", "b1")
            },
        )
        .await
        .expect("create");
    let b = &out.branch.branch;
    assert_eq!(b.record.expires_at_ms, Some(T0_MS + 3_600_000));
    let updated = h
        .service
        .update_branch(
            &user(),
            UpdateBranch {
                namespace: "acme".into(),
                project_id: p.id.clone(),
                branch_id: b.record.id.clone(),
                name: Some("staging".into()),
                expire_at_ms: Some(None),
                expected_version: Some(b.version),
                idempotency_key: "u1".into(),
                ..UpdateBranch::default()
            },
        )
        .await
        .expect("update");
    assert_eq!(updated.branch.record.name, "staging");
    assert_eq!(updated.branch.record.expires_at_ms, None);
    // The old name is free; the new one taken.
    h.service
        .create_branch(&user(), branch(&p.id, "dev", "b2"))
        .await
        .expect("dev is free");
    let e = h
        .service
        .create_branch(&user(), branch(&p.id, "staging", "b3"))
        .await
        .expect_err("staging is taken");
    assert_eq!(e.reason, Reason::AlreadyExists);
}

/// A child's create leaves its parent's record, and version, as they were:
/// the count lives in the parent's guard.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_child_create_leaves_the_parent_version() {
    let h = harness!();
    let (p, main) = project(&h).await;
    let before = h
        .service
        .get_branch("acme", &p.id, &main)
        .await
        .expect("get");
    h.service
        .create_branch(&user(), branch(&p.id, "dev", "b1"))
        .await
        .expect("a child");
    let after = h
        .service
        .get_branch("acme", &p.id, &main)
        .await
        .expect("get");
    assert_eq!(after.branch, before.branch);
}

/// A child created between a delete's reads and its commit makes the
/// delete conflict; on its second try the delete sees the child.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_child_created_during_its_parents_delete_conflicts() {
    use crate::common::{FakeNeon, fresh_store, harness_with, once_before, plain_service};

    let Some(store) = fresh_store().await else {
        eprintln!("skipped: no store");
        return;
    };
    let setup = plain_service(&store, &FakeNeon::default());
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
    let dev = setup
        .create_branch(&user(), branch(&p.id, "dev", "b1"))
        .await
        .expect("dev")
        .branch
        .branch
        .record
        .id;
    let hook = {
        let (store, pid, dev) = (store.clone(), p.id.clone(), dev.clone());
        once_before("DeleteBranch", move || {
            let other = plain_service(&store, &FakeNeon::default());
            let req = CreateBranch {
                parent_id: dev.clone(),
                ..branch(&pid, "feature", "b2")
            };
            async move {
                other.create_branch(&user(), req).await.expect("the child");
            }
        })
    };
    let h = harness_with(store, Some(hook));
    let e = h
        .service
        .delete_branch(
            &user(),
            DeleteBranch {
                namespace: "acme".into(),
                project_id: p.id.clone(),
                branch_id: dev.clone(),
                expected_version: None,
                idempotency_key: "d1".into(),
            },
        )
        .await
        .expect_err("the child won");
    assert_eq!(e.reason, Reason::BranchHasChildren, "{e}");
    let got = h
        .service
        .get_branch("acme", &p.id, &dev)
        .await
        .expect("dev");
    assert_eq!(got.branch.record.state, BranchState::Creating);
}

/// A parent deleted between a child create's reads and its commit makes
/// the create conflict; on its second try it sees the parent deleting.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_parent_deleted_during_a_child_create_conflicts() {
    use crate::common::{FakeNeon, fresh_store, harness_with, once_before, plain_service};

    let Some(store) = fresh_store().await else {
        eprintln!("skipped: no store");
        return;
    };
    let setup = plain_service(&store, &FakeNeon::default());
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
    let dev = setup
        .create_branch(&user(), branch(&p.id, "dev", "b1"))
        .await
        .expect("dev")
        .branch
        .branch
        .record
        .id;
    let hook = {
        let (store, pid, dev) = (store.clone(), p.id.clone(), dev.clone());
        once_before("CreateBranch", move || {
            let other = plain_service(&store, &FakeNeon::default());
            let req = DeleteBranch {
                namespace: "acme".into(),
                project_id: pid.clone(),
                branch_id: dev.clone(),
                expected_version: None,
                idempotency_key: "d1".into(),
            };
            async move {
                other.delete_branch(&user(), req).await.expect("the delete");
            }
        })
    };
    let h = harness_with(store, Some(hook));
    let e = h
        .service
        .create_branch(
            &user(),
            CreateBranch {
                parent_id: dev.clone(),
                ..branch(&p.id, "feature", "b2")
            },
        )
        .await
        .expect_err("the delete won");
    assert_eq!(e.reason, Reason::FailedPrecondition, "{e}");
    let (all, _) = h
        .service
        .list_branches("acme", &p.id, 0, "")
        .await
        .expect("list");
    assert_eq!(all.len(), 2, "main and dev only");
}
