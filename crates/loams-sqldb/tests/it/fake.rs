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
