//! Loams Graph's use of the log (GR1 Task 12, D744): commits fenced by a
//! graph owner's lease, and a graph's reserved implicit stream.

use std::time::Duration;

use loams_common::meta::{
    ApplyError, Consistency, Fence, MetaStore, WalCommit, graph_stream_name,
    legacy_graph_stream_name,
};
use loams_common::{NamespaceId, StreamId};

use super::{chunk, commit, high_watermark, namespace, rejected, retired, stream};
use crate::Backend;

const L: Consistency = Consistency::Linearizable;
const TTL: Duration = Duration::from_secs(30);
const GRAPH: &str = "gr_01J9ZQ3V7K8X2M4N6P8R0T2V4W";
const OTHER_GRAPH: &str = "gr_01J9ZQ3V7K8X2M4N6P8R0T2V4X";

fn fenced(meta: &dyn MetaStore, object: &str, s: StreamId, fence: &Fence) -> WalCommit {
    WalCommit {
        object: object.to_string(),
        created_at_ms: meta.now_ms(),
        chunks: vec![chunk(s, 0, 2, 0..20)],
        fences: vec![fence.clone()],
    }
}

pub async fn a_fenced_commit_wal_is_refused_once_its_lease_moves(backend: &dyn Backend) {
    let db = backend.start().await;
    let meta = db.first();
    let ns = namespace(meta, "graph-fenced").await;
    let s = stream(meta, ns, "events", 1).await;
    let lease = "graph/gr_01J9ZQ3V7K8X2M4N6P8R0T2V4W/owner";
    let grant = meta
        .acquire_lease(lease, "node-1", TTL)
        .await
        .expect("acquire");
    let fence = Fence {
        lease: lease.to_string(),
        epoch: grant.epoch,
    };
    // While the lease is held at its epoch, the commit applies.
    let first = meta
        .commit_wal(fenced(meta, "graph-fenced/wal-1", s, &fence))
        .await
        .into_result()
        .expect("fenced commit");
    assert_eq!(first, vec![0]);
    // A fence at another epoch is broken.
    let wrong = db
        .last()
        .commit_wal(fenced(
            meta,
            "graph-fenced/wal-2",
            s,
            &Fence {
                epoch: grant.epoch + 1,
                ..fence.clone()
            },
        ))
        .await;
    assert!(!wrong.earlier_unknown);
    let refusal = ApplyError::Fenced {
        lease: lease.to_string(),
    };
    assert_eq!(rejected(wrong.result), refusal);
    // The owner gives the lease up: its fence breaks, and nothing of a commit
    // under it lands.
    meta.release_lease(lease, "node-1", grant.epoch)
        .await
        .expect("release");
    let stale = meta
        .commit_wal(fenced(meta, "graph-fenced/wal-3", s, &fence))
        .await;
    assert_eq!(rejected(stale.result), refusal);
    assert_eq!(high_watermark(db.last(), s, 0).await, 2);
    // A retry of the object committed under the fence still answers its
    // offsets: it was made while the fence held.
    assert_eq!(
        db.last()
            .commit_wal(fenced(meta, "graph-fenced/wal-1", s, &fence))
            .await
            .into_result()
            .expect("retry"),
        vec![0]
    );
    // The next owner's epoch fences again.
    let next = db
        .last()
        .acquire_lease(lease, "node-2", TTL)
        .await
        .expect("take over");
    assert!(next.epoch > grant.epoch);
    let renewed = Fence {
        lease: lease.to_string(),
        epoch: next.epoch,
    };
    assert_eq!(
        meta.commit_wal(fenced(meta, "graph-fenced/wal-4", s, &renewed))
            .await
            .into_result()
            .expect("the new owner commits"),
        vec![2]
    );
    // Every fence of a commit must hold.
    let mixed = WalCommit {
        fences: vec![renewed, fence],
        ..fenced(
            meta,
            "graph-fenced/wal-5",
            s,
            &Fence {
                lease: lease.to_string(),
                epoch: next.epoch,
            },
        )
    };
    assert_eq!(rejected(meta.commit_wal(mixed).await.result), refusal);
    assert_eq!(high_watermark(db.last(), s, 0).await, 4);
}

pub async fn graph_streams_are_reserved_idempotent_and_migrated(backend: &dyn Backend) {
    let db = backend.start().await;
    let meta = db.first();
    let ns = namespace(meta, "graph-streams").await;
    let created = meta
        .ensure_graph_stream(ns, GRAPH)
        .await
        .expect("ensure graph stream");
    // Again, through another handle: the same stream.
    assert_eq!(
        db.last()
            .ensure_graph_stream(ns, GRAPH)
            .await
            .expect("again"),
        created
    );
    let found = db
        .last()
        .stream_by_name(L, ns, &graph_stream_name(GRAPH))
        .await
        .expect("read")
        .expect("the graph's stream");
    assert_eq!(found.id, created);
    assert_eq!(found.partitions, 1);
    // The name is reserved: nobody creates it as an ordinary stream.
    assert!(matches!(
        rejected(
            meta.create_stream(
                ns,
                &graph_stream_name(OTHER_GRAPH),
                1,
                loams_common::meta::WalClass::Standard,
                loams_common::meta::Retention::default(),
            )
            .await
        ),
        ApplyError::InvalidArgument(_)
    ));
    // A stream of GR1 Task 11's name moves to the reserved name, records and all.
    let legacy = stream(meta, ns, &legacy_graph_stream_name(OTHER_GRAPH), 1).await;
    commit(
        meta,
        "graph-streams/wal-1",
        vec![chunk(legacy, 0, 3, 0..30)],
    )
    .await;
    assert_eq!(
        meta.ensure_graph_stream(ns, OTHER_GRAPH)
            .await
            .expect("migrate"),
        legacy
    );
    assert_eq!(
        db.last()
            .ensure_graph_stream(ns, OTHER_GRAPH)
            .await
            .expect("migrate again"),
        legacy
    );
    let reader = db.last();
    assert_eq!(
        reader
            .stream_by_name(L, ns, &legacy_graph_stream_name(OTHER_GRAPH))
            .await
            .expect("read"),
        None
    );
    assert_eq!(
        reader
            .stream(L, legacy)
            .await
            .expect("read")
            .expect("the stream")
            .name,
        graph_stream_name(OTHER_GRAPH)
    );
    assert_eq!(high_watermark(reader, legacy, 0).await, 3);
    // A stream of the legacy name in another shape (anyone could have taken the name) does not
    // move: it would carry its retention or partitions under the reserved name.
    const THIRD_GRAPH: &str = "gr_01J9ZQ3V7K8X2M4N6P8R0T2V4Y";
    let odd = stream(meta, ns, &legacy_graph_stream_name(THIRD_GRAPH), 2).await;
    assert!(matches!(
        rejected(meta.ensure_graph_stream(ns, THIRD_GRAPH).await),
        ApplyError::InvalidArgument(_)
    ));
    assert_eq!(
        reader.stream(L, odd).await.expect("read").map(|s| s.name),
        Some(legacy_graph_stream_name(THIRD_GRAPH))
    );
    // Only graph ids name graph streams, in a namespace that exists.
    for bad in [
        "",
        "gr_",
        "graph",
        "gr_01j9zq3v7k8x2m4n6p8r0t2v4w",
        "gr_81J9ZQ3V7K8X2M4N6P8R0T2V4W",
    ] {
        assert!(
            matches!(
                rejected(meta.ensure_graph_stream(ns, bad).await),
                ApplyError::InvalidArgument(_)
            ),
            "{bad:?}"
        );
    }
    let missing = NamespaceId(ns.0 + 1000);
    assert_eq!(
        rejected(meta.ensure_graph_stream(missing, GRAPH).await),
        ApplyError::NamespaceNotFound(missing)
    );
}

pub async fn drop_graph_stream_is_fenced_and_retires_its_objects(backend: &dyn Backend) {
    let db = backend.start().await;
    let meta = db.first();
    let ns = namespace(meta, "graph-drop").await;
    let s = meta.ensure_graph_stream(ns, GRAPH).await.expect("ensure");
    let object = "graph-drop/wal-1";
    commit(meta, object, vec![chunk(s, 0, 2, 0..20)]).await;
    let lease = "graph/maintenance";
    let grant = meta
        .acquire_lease(lease, "node-1", TTL)
        .await
        .expect("acquire");
    let fence = Fence {
        lease: lease.to_string(),
        epoch: grant.epoch,
    };
    let broken = Fence {
        epoch: grant.epoch + 1,
        ..fence.clone()
    };
    assert_eq!(
        rejected(meta.drop_graph_stream(ns, GRAPH, Some(broken)).await),
        ApplyError::Fenced {
            lease: lease.to_string()
        }
    );
    assert_eq!(
        db.last().stream(L, s).await.expect("read").map(|st| st.id),
        Some(s)
    );
    assert_eq!(
        db.last()
            .drop_graph_stream(ns, GRAPH, Some(fence.clone()))
            .await
            .expect("drop"),
        Some(s)
    );
    let reader = db.last();
    assert_eq!(reader.stream(L, s).await.expect("read"), None);
    assert_eq!(
        reader
            .stream_by_name(L, ns, &graph_stream_name(GRAPH))
            .await
            .expect("read"),
        None
    );
    assert!(retired(reader).await.contains(&object.to_string()));
    // A retry finds nothing to drop, and a stream of the legacy name drops too.
    assert_eq!(
        meta.drop_graph_stream(ns, GRAPH, Some(fence))
            .await
            .expect("retry"),
        None
    );
    let legacy = stream(meta, ns, &legacy_graph_stream_name(OTHER_GRAPH), 1).await;
    assert_eq!(
        meta.drop_graph_stream(ns, OTHER_GRAPH, None)
            .await
            .expect("drop legacy"),
        Some(legacy)
    );
    // The name is free for a graph made again under the same id.
    assert_ne!(meta.ensure_graph_stream(ns, GRAPH).await.expect("again"), s);
}

/// GT1 Task 15: reserved system streams (`_git`) are created only by
/// `ensure_system_stream`, with their own partition count, idempotently.
pub async fn system_streams_are_reserved_and_idempotent(backend: &dyn Backend) {
    use loams_common::meta::{GIT_STREAM, system_stream_partitions};

    let db = backend.start().await;
    let meta = db.first();
    let ns = namespace(meta, "system-streams").await;
    let created = meta
        .ensure_system_stream(ns, GIT_STREAM)
        .await
        .expect("ensure _git");
    assert_eq!(
        db.last()
            .ensure_system_stream(ns, GIT_STREAM)
            .await
            .expect("again"),
        created
    );
    let found = db
        .last()
        .stream_by_name(L, ns, GIT_STREAM)
        .await
        .expect("read")
        .expect("the _git stream");
    assert_eq!(found.id, created);
    assert_eq!(Some(found.partitions), system_stream_partitions(GIT_STREAM));
    // Nobody creates it as an ordinary stream, and no other name is a system stream.
    assert!(matches!(
        rejected(
            meta.create_stream(
                ns,
                GIT_STREAM,
                1,
                loams_common::meta::WalClass::Standard,
                loams_common::meta::Retention::default(),
            )
            .await
        ),
        ApplyError::InvalidArgument(_)
    ));
    assert!(matches!(
        rejected(meta.ensure_system_stream(ns, "_other").await),
        ApplyError::InvalidArgument(_)
    ));
    // ME1 Task 4: Loams Meter's usage stream, with its 32 partitions and 14-day retention.
    let meter = meta
        .ensure_system_stream(ns, loams_common::meta::METER_STREAM)
        .await
        .expect("ensure _meter");
    assert_ne!(meter, created);
    assert_eq!(
        db.last()
            .ensure_system_stream(ns, loams_common::meta::METER_STREAM)
            .await
            .expect("again"),
        meter
    );
    let found = db
        .last()
        .stream_by_name(L, ns, loams_common::meta::METER_STREAM)
        .await
        .expect("read")
        .expect("the _meter stream");
    assert_eq!(found.id, meter);
    assert_eq!(
        found.partitions,
        loams_common::meta::METER_STREAM_PARTITIONS
    );
    assert_eq!(
        found.retention,
        loams_common::meta::Retention {
            max_age_ms: Some(loams_common::meta::METER_STREAM_RETENTION_MS),
            max_bytes: None,
        }
    );
    assert_eq!(found.class, loams_common::meta::WalClass::Standard);
    assert!(matches!(
        rejected(
            meta.ensure_system_stream(NamespaceId(u64::MAX - 7), GIT_STREAM)
                .await
        ),
        ApplyError::NamespaceNotFound(_)
    ));
}
