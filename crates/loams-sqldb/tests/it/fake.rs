use std::time::Duration;

use loams_sqldb::images::Images;
use loams_sqldb::model::{BranchId, Class};
use loams_sqldb::runtime::fake::{Call, FakeRuntime, Op};
use loams_sqldb::runtime::{JobSpec, MemberState, RuntimeError, SqlRuntime};

fn br(n: u8) -> BranchId {
    BranchId::parse(&format!("br_000000000000000{n}")).expect("id")
}

#[tokio::test]
async fn fake_runtime_follows_the_contract() {
    let rt = FakeRuntime::new();
    let b = br(1);
    assert!(rt.pool_status(&b).await.expect("status").is_none());

    let st = rt.ensure_pool(&b, Class::Xs, 1).await.expect("ensure");
    assert_eq!((st.class, st.replicas, st.ready()), (Class::Xs, 1, 1));
    assert!(st.members.iter().all(|m| m.state == MemberState::Ready));

    // Idempotent; a class change keeps the replica count asked for.
    let st = rt.ensure_pool(&b, Class::S, 1).await.expect("ensure again");
    assert_eq!((st.class, st.members.len()), (Class::S, 1));

    let st = rt.scale(&b, 3).await.expect("scale up");
    assert_eq!(
        st.members.iter().map(|m| m.index).collect::<Vec<_>>(),
        [0, 1, 2]
    );
    let st = rt.scale(&b, 0).await.expect("scale to zero");
    assert_eq!((st.replicas, st.members.len()), (0, 0));
    // A suspended pool still exists, with its class.
    assert_eq!(
        rt.pool_status(&b).await.expect("status").map(|s| s.class),
        Some(Class::S)
    );

    assert!(matches!(
        rt.scale(&br(2), 1).await,
        Err(RuntimeError::NoPool(_))
    ));

    rt.delete_pool(&b).await.expect("delete");
    rt.delete_pool(&b).await.expect("delete is idempotent");
    assert!(rt.pool_status(&b).await.expect("status").is_none());

    assert_eq!(
        rt.calls(),
        vec![
            Call::PoolStatus(b.clone()),
            Call::EnsurePool(b.clone(), Class::Xs, 1),
            Call::EnsurePool(b.clone(), Class::S, 1),
            Call::Scale(b.clone(), 3),
            Call::Scale(b.clone(), 0),
            Call::PoolStatus(b.clone()),
            Call::Scale(br(2), 1),
            Call::DeletePool(b.clone()),
            Call::DeletePool(b.clone()),
            Call::PoolStatus(b),
        ]
    );
}

#[tokio::test]
async fn fake_runtime_injects_failures_once() {
    let rt = FakeRuntime::new();
    let b = br(3);
    rt.fail_next(Op::EnsurePool);
    assert!(matches!(
        rt.ensure_pool(&b, Class::M, 1).await,
        Err(RuntimeError::Unavailable(_))
    ));
    // The failed call changed nothing; the retry succeeds.
    assert!(rt.pool_status(&b).await.expect("status").is_none());
    rt.ensure_pool(&b, Class::M, 1).await.expect("retry");

    rt.hold_starting(true);
    let st = rt.scale(&b, 2).await.expect("scale");
    assert_eq!(st.ready(), 1, "the new member is still starting");
    rt.hold_starting(false);
    assert_eq!(
        rt.pool_status(&b).await.expect("s").expect("pool").ready(),
        2
    );
}

#[tokio::test]
async fn fake_runtime_runs_jobs() {
    let rt = FakeRuntime::new();
    let spec = JobSpec {
        name: "backup-br_0000000000000001".into(),
        image: Images::load().expect("pins").br().clone(),
        args: vec!["backup".into(), "full".into()],
        env: vec![],
        timeout: Duration::from_secs(60),
    };
    assert_eq!(rt.run_job(&spec).await.expect("job").exit_code, 0);
    rt.set_job_exit_code(2);
    assert_eq!(rt.run_job(&spec).await.expect("job").exit_code, 2);
    rt.fail_next(Op::RunJob);
    assert!(rt.run_job(&spec).await.is_err());
    assert_eq!(
        rt.calls()
            .iter()
            .filter(|c| matches!(c, Call::RunJob(n) if n == &spec.name))
            .count(),
        3
    );
}

#[tokio::test]
async fn fake_runtime_restarts_members_on_class_change() {
    let rt = FakeRuntime::new();
    let b = br(4);
    let names = |st: &loams_sqldb::runtime::PoolStatus| {
        st.members
            .iter()
            .map(|m| m.name.clone())
            .collect::<Vec<_>>()
    };
    let xs = names(&rt.ensure_pool(&b, Class::Xs, 2).await.expect("xs"));
    let same = names(&rt.ensure_pool(&b, Class::Xs, 2).await.expect("xs again"));
    assert_eq!(xs, same, "an unchanged pool keeps its members");
    let s = names(&rt.ensure_pool(&b, Class::S, 2).await.expect("s"));
    assert_eq!(s.len(), 2);
    assert!(
        xs.iter().zip(&s).all(|(a, b)| a != b),
        "class change replaces every member: {xs:?} {s:?}"
    );
}

#[tokio::test]
async fn fake_runtime_reports_and_replaces_exited_members() {
    let rt = FakeRuntime::new();
    let b = br(5);
    let before = rt
        .ensure_pool(&b, Class::Xs, 1)
        .await
        .expect("ensure")
        .members[0]
        .name
        .clone();
    rt.crash_member(&b, 0, Some(137));
    let st = rt.pool_status(&b).await.expect("status").expect("pool");
    assert_eq!(st.members[0].state, MemberState::Exited { code: Some(137) });
    assert_eq!(st.ready(), 0);
    let st = rt.scale(&b, 1).await.expect("scale replaces it");
    assert_eq!(st.members[0].state, MemberState::Ready);
    assert_ne!(st.members[0].name, before);
}

#[tokio::test]
async fn fake_runtime_applies_then_errors() {
    let rt = FakeRuntime::new();
    let b = br(6);
    rt.fail_after_next(Op::EnsurePool);
    assert!(matches!(
        rt.ensure_pool(&b, Class::Xs, 1).await,
        Err(RuntimeError::Unavailable(_))
    ));
    assert_eq!(
        rt.pool_status(&b)
            .await
            .expect("s")
            .expect("applied")
            .members
            .len(),
        1
    );
    rt.fail_after_next(Op::Scale);
    assert!(rt.scale(&b, 0).await.is_err());
    assert_eq!(
        rt.pool_status(&b).await.expect("s").expect("pool").replicas,
        0
    );
    rt.fail_after_next(Op::DeletePool);
    assert!(rt.delete_pool(&b).await.is_err());
    assert!(
        rt.pool_status(&b).await.expect("s").is_none(),
        "deleted despite the error"
    );
}
