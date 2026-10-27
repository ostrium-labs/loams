//! The system objects `migrate` makes (ME1 Task 4, design §63 §7.1):
//! Loams Meter's `_system/_meter` stream.

mod support;

use loams_common::meta::{
    Consistency, METER_STREAM, METER_STREAM_PARTITIONS, METER_STREAM_RETENTION_MS, Retention,
    SYSTEM_NAMESPACE, WalClass,
};
use loams_migrate::system::{ensure_meter_stream, ensure_system_streams, meter_stream_retention};
use loams_migrate::{Migration, RunOptions, Runner};
use support::{FileMigration, Stack, path};

const P2: &str = "0.1.0-preview.2";
const P3: &str = "0.1.0-preview.3";

/// The `_system/_meter` streams of the stack.
async fn meter_streams(stack: &Stack) -> Vec<loams_common::meta::Stream> {
    let system = stack
        .meta
        .namespace_by_name(Consistency::Linearizable, SYSTEM_NAMESPACE)
        .await
        .expect("read")
        .expect("the _system namespace");
    stack
        .meta
        .streams(Consistency::Linearizable, Some(system.id))
        .await
        .expect("streams")
        .into_iter()
        .filter(|s| s.name == METER_STREAM)
        .collect()
}

/// `meter_stream_created_once` (ME1 plan, Task 4): every `migrate run`, on a
/// new stack and on an upgrade, makes sure of `_system/_meter`, and only the
/// first one creates it: 32 partitions, class `Standard`, 14 days.
#[tokio::test]
async fn meter_stream_created_once() {
    let stack = Stack::new().await;
    // Before any run there is no meter stream (and no `_system` streams).
    assert!(
        stack
            .meta
            .namespace_by_name(Consistency::Linearizable, SYSTEM_NAMESPACE)
            .await
            .expect("read")
            .is_none()
    );

    // A new stack: the run applies nothing and still makes the stream.
    let none: [&dyn Migration; 0] = [];
    let train = path("prerelease");
    let mut opts = RunOptions::new("migrate-test");
    opts.new_stack = true;
    let report = Runner::new(&none, &train)
        .run(&mut stack.cx(), P2, &opts)
        .await
        .expect("first run");
    assert_eq!(report.system_streams, ["_system/_meter"]);
    let first = meter_streams(&stack).await;
    assert_eq!(first.len(), 1, "{first:?}");
    let stream = &first[0];
    assert_eq!(stream.partitions, METER_STREAM_PARTITIONS);
    assert_eq!(stream.partitions, 32);
    assert_eq!(stream.class, WalClass::Standard);
    assert_eq!(
        stream.retention,
        Retention {
            max_age_ms: Some(METER_STREAM_RETENTION_MS),
            max_bytes: None,
        }
    );
    assert_eq!(stream.retention, meter_stream_retention());
    assert_eq!(METER_STREAM_RETENTION_MS, 14 * 24 * 3_600_000);

    // An upgrade that applies a migration: the same stream, not a second one.
    let a = FileMigration::new("0001-a", 2);
    a.seed(stack.data.path());
    let migrations: [&dyn Migration; 1] = [&a];
    let report = Runner::new(&migrations, &train)
        .run(&mut stack.cx(), P3, &RunOptions::new("migrate-test"))
        .await
        .expect("upgrade");
    assert_eq!(report.applied, ["0001-a"]);
    assert_eq!(report.system_streams, ["_system/_meter"]);
    // A run with nothing to do, and the helpers called directly (the `meter`
    // role calls `ensure_meter_stream` when it starts without `migrate`).
    Runner::new(&migrations, &train)
        .run(&mut stack.cx(), P3, &RunOptions::new("migrate-test"))
        .await
        .expect("rerun");
    let id = ensure_meter_stream(&*stack.meta).await.expect("ensure");
    let again = ensure_meter_stream(&*stack.meta)
        .await
        .expect("ensure again");
    assert_eq!(id, again);
    ensure_system_streams(&*stack.meta)
        .await
        .expect("ensure all");
    assert_eq!(meter_streams(&stack).await, first);
    assert_eq!(id, stream.id);
}

/// The stream is reserved: nobody creates `_meter` as an ordinary stream,
/// in `_system` or anywhere else.
#[tokio::test]
async fn meter_stream_is_reserved() {
    let stack = Stack::new().await;
    let id = ensure_meter_stream(&*stack.meta).await.expect("ensure");
    let ns = stack
        .meta
        .create_namespace("tenant")
        .await
        .expect("namespace");
    let refused = stack
        .meta
        .create_stream(
            ns,
            METER_STREAM,
            METER_STREAM_PARTITIONS,
            WalClass::Standard,
            Retention::default(),
        )
        .await;
    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(meter_streams(&stack).await[0].id, id);
}
