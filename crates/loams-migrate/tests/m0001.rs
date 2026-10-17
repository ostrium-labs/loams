//! `0001-safekeeper-volume` (UP1 Task 10).

#[path = "support/neon.rs"]
mod neon;
mod support;

use std::time::Duration;

use loams_migrate::cx::HostChange;
use loams_migrate::migrations::{MIGRATIONS, SAFEKEEPER_VOLUME, SafekeeperVolume};
use loams_migrate::{Class, Migration, Step};
use neon::{Fake, fakes};
use support::Stack;

fn quick() -> SafekeeperVolume {
    SafekeeperVolume {
        timeout: Duration::from_millis(200),
        poll: Duration::from_millis(10),
    }
}

fn recreate() -> HostChange {
    HostChange::Recreate {
        service: "safekeeper1".to_string(),
        reason: "give it the safekeeper-data volume; its WAL is drained".to_string(),
    }
}

#[tokio::test]
async fn m0001_requires_drain_record() {
    let stack = Stack::new().await;
    // A timeline whose WAL the pageserver has not uploaded: the migration
    // fails, and asks for no recreate.
    let (_shared, pageserver, safekeeper) = fakes(Fake {
        remote: 0x100,
        step: 0,
        flush: 0x400,
        ..Fake::default()
    })
    .await;
    let mut cx = stack.cx();
    cx.postgres = Some((pageserver, safekeeper));
    let err = quick().apply(&mut cx, None).await.unwrap_err();
    assert_eq!(err.code, "postgres_not_drained", "{err}");
    assert!(cx.host.changes().is_empty());
    // Drained: it records that the CLI may recreate the safekeeper.
    let (_shared, pageserver, safekeeper) = fakes(Fake {
        remote: 0x400,
        flush: 0x400,
        ..Fake::default()
    })
    .await;
    let mut cx = stack.cx();
    cx.postgres = Some((pageserver, safekeeper));
    assert_eq!(
        quick().apply(&mut cx, None).await.expect("drained"),
        Step::Done
    );
    assert_eq!(cx.host.changes(), [recreate()]);
}

#[tokio::test]
async fn m0001_is_reversible_and_idempotent() {
    assert_eq!(SAFEKEEPER_VOLUME.class(), Class::Reversible);
    assert_eq!(SAFEKEEPER_VOLUME.id(), "0001-safekeeper-volume");
    assert!(
        MIGRATIONS
            .iter()
            .any(|m| m.id() == "0001-safekeeper-volume")
    );
    let path = loams_migrate::UpgradePath::compiled();
    let listed = path
        .release("0.1.0")
        .expect("0.1.0")
        .migrations
        .iter()
        .find(|m| m.id == "0001-safekeeper-volume")
        .expect("listed under 0.1.0");
    assert_eq!(listed.class, Class::Reversible);
    let stack = Stack::new().await;
    let (_shared, pageserver, safekeeper) = fakes(Fake {
        remote: 0x400,
        flush: 0x400,
        ..Fake::default()
    })
    .await;
    let mut cx = stack.cx();
    cx.postgres = Some((pageserver, safekeeper));
    // Twice: the same outcome, the recreate asked once.
    assert_eq!(
        quick().apply(&mut cx, None).await.expect("first"),
        Step::Done
    );
    assert_eq!(
        quick().apply(&mut cx, None).await.expect("again"),
        Step::Done
    );
    assert_eq!(cx.host.changes(), [recreate()]);
    // A stack without Postgres has nothing to drain.
    let mut bare = stack.cx();
    assert_eq!(
        quick().apply(&mut bare, None).await.expect("no postgres"),
        Step::Done
    );
    assert!(bare.host.changes().is_empty());
    let mut absent = stack.cx();
    absent.postgres = Some((
        "http://no-such-pageserver.invalid:9898".to_string(),
        "http://no-such-safekeeper.invalid:7676".to_string(),
    ));
    assert_eq!(
        quick()
            .apply(&mut absent, None)
            .await
            .expect("name does not resolve"),
        Step::Done
    );
    assert!(absent.host.changes().is_empty());
}
