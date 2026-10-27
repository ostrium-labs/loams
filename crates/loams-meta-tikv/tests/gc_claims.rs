//! Garbage collection against a writer that would make the same object
//! reachable (design §20 §11.3): the orphan passes claim what they return
//! under the object's `o/` row lock, and a WAL commit or a segment swap
//! locks that row before it references the object, so an object is never
//! both returned for deletion and referenced.
//!
//! This is the TiKV crash gate's `cache: object not found: …seg` (nightly,
//! 2026-10-20): a restarted server's swap waited on a crashed process's lock
//! after its checks, garbage collection read the segment as unreferenced and
//! deleted it, and the swap then committed a reference to it.
//!
//! Runs when the standard TiKV playground is named by `LOAMS_TEST_PD`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use loams_common::NamespaceId;
use loams_common::StreamId;
use loams_common::meta::{
    ApplyError, Consistency, Freshness, MetaError, MetaStore, Retention, SegmentSwap, WalChunk,
    WalClass, WalCommit,
};
use loams_meta_tikv::TikvMeta;
use loams_tikv::testing::{self, TEST_META};
use loams_tikv::{Fault, FaultPlan, FaultPoint, Tikv, TikvConfig};

const L: Consistency = Consistency::Linearizable;

/// One test at a time: they connect and open handles on one playground.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Holds the first attempt of `op` just before its prewrite (its pessimistic
/// locks taken, its checks done) for `hold`, and says when it got there.
struct HoldBeforePrewrite {
    op: &'static str,
    hold: Duration,
    reached: AtomicBool,
}

impl FaultPlan for HoldBeforePrewrite {
    fn at(&self, op: &str, point: FaultPoint, attempt: u32) -> Option<Fault> {
        (op == self.op && point == FaultPoint::BeforePrewrite && attempt == 1).then(|| {
            self.reached.store(true, Ordering::SeqCst);
            Fault::Delay(self.hold)
        })
    }
}

async fn open(config: &TikvConfig, plan: Option<Arc<dyn FaultPlan>>) -> TikvMeta {
    let mut tikv = Tikv::connect(config.clone())
        .await
        .expect("connect to the test keyspace");
    if let Some(plan) = plan {
        tikv = tikv.with_faults(plan);
    }
    TikvMeta::open_on(
        tikv,
        loams_meta_tikv::DEFAULT_ID_BLOCK,
        loams_meta_tikv::DEFAULT_POLL,
    )
    .await
    .expect("open the metastore")
}

/// A namespace with one single-partition stream holding one WAL record.
async fn setup(meta: &TikvMeta) -> (NamespaceId, StreamId) {
    let ns = meta.create_namespace("gc").await.expect("namespace");
    let s = meta
        .create_stream(ns, "events", 1, WalClass::Standard, Retention::default())
        .await
        .expect("stream");
    meta.commit_wal(wal(meta, "gc/wal-a", s))
        .await
        .into_result()
        .expect("a WAL commit");
    (ns, s)
}

fn wal(meta: &TikvMeta, object: &str, stream: StreamId) -> WalCommit {
    WalCommit {
        object: object.to_string(),
        created_at_ms: meta.now_ms(),
        chunks: vec![WalChunk {
            stream,
            partition: 0,
            records: 1,
            byte_range: 0..10,
            max_timestamp_ms: 0,
        }],
        fences: Vec::new(),
    }
}

fn swap(stream: StreamId, segment: &str, created_at_ms: u64, max_age_ms: u64) -> SegmentSwap {
    SegmentSwap {
        stream,
        partition: 0,
        replaces: vec![(0, "gc/wal-a".to_string())],
        segment: segment.to_string(),
        byte_range: 0..10,
        max_timestamp_ms: 0,
        fence: None,
        fresh: Freshness {
            created_at_ms,
            max_age_ms,
        },
    }
}

/// Waits until `cond` holds, re-checking whenever the metastore changes or
/// the clock moves (no fixed sleep decides the outcome).
async fn until(what: &str, mut cond: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !cond().await {
        assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// A swap that has checked everything and holds its locks, but has not
/// committed, while garbage collection looks at its segment: collection
/// waits for the swap and sees the reference, so it returns nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pending_swap_keeps_its_segment_from_garbage_collection() {
    let Some(cluster) = testing::cluster().await else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let config = cluster.config(TEST_META);
    let plan = Arc::new(HoldBeforePrewrite {
        op: "meta.swap_segment",
        hold: Duration::from_secs(3),
        reached: AtomicBool::new(false),
    });
    let held = open(&config, Some(plan.clone())).await;
    let gc = open(&config, None).await;
    let (ns, s) = setup(&gc).await;

    // Fresh enough for its own 1 s deadline when it checks; 200 ms of grace
    // has passed when garbage collection looks.
    let written = gc.now_ms();
    let segment = "gc/seg-pending";
    let swapping = tokio::spawn({
        let held = held.clone();
        let swap = swap(s, segment, written, 1_000);
        async move { held.swap_segment(swap).await.into_result() }
    });
    until("the swap holds its locks", async || {
        plan.reached.load(Ordering::SeqCst)
    })
    .await;
    until("200 ms of the metastore clock", async || {
        gc.clock_ms(L).await.expect("clock") >= written + 200
    })
    .await;
    let orphans = gc
        .orphan_segments(ns, vec![(segment.to_string(), written)], 200, 10)
        .await
        .expect("the orphan pass");
    let swapped = swapping.await.expect("the swap task");
    assert!(
        !(orphans.contains(&segment.to_string()) && swapped.is_ok()),
        "the segment was returned for deletion and then referenced: {orphans:?}, {swapped:?}"
    );
    assert_eq!(orphans, Vec::<String>::new());
    swapped.expect("the swap commits");
    let index = gc.partition_index(L, s, 0, 0, None).await.expect("read");
    assert!(
        index.is_some_and(|i| i.entries().any(|e| e.object == segment)),
        "the swap's entry is there"
    );
}

/// An object garbage collection has returned (and so claimed) cannot become
/// reachable afterwards: a later swap of that segment, or a WAL commit of
/// that object, is refused as stale however fresh it says it is.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_claimed_object_is_never_referenced() {
    let Some(cluster) = testing::cluster().await else {
        return;
    };
    let _serial = SERIAL.lock().await;
    let config = cluster.config(TEST_META);
    let meta = open(&config, None).await;
    let (ns, s) = setup(&meta).await;
    // Uploaded a while ago (the clock estimate can run ahead of TSO).
    let written = meta.now_ms().saturating_sub(60_000);

    let segment = "gc/seg-claimed";
    let orphans = meta
        .orphan_segments(ns, vec![(segment.to_string(), written)], 0, 10)
        .await
        .expect("the orphan pass");
    assert_eq!(orphans, vec![segment.to_string()]);
    let refused = meta
        .swap_segment(swap(s, segment, meta.now_ms(), 600_000))
        .await
        .into_result();
    assert!(
        matches!(
            refused,
            Err(MetaError::Rejected(ApplyError::StaleObject { .. }))
        ),
        "{refused:?}"
    );

    let object = "gc/wal-claimed";
    let orphans = meta
        .orphan_wal_objects(vec![(object.to_string(), written)], 0, 10)
        .await
        .expect("the orphan pass");
    assert_eq!(orphans, vec![object.to_string()]);
    let refused = meta.commit_wal(wal(&meta, object, s)).await.into_result();
    assert!(
        matches!(
            refused,
            Err(MetaError::Rejected(ApplyError::StaleCommit { .. }))
        ),
        "{refused:?}"
    );

    // A pass over a referenced object claims nothing: it stays usable.
    let orphans = meta
        .orphan_wal_objects(vec![("gc/wal-a".to_string(), written)], 0, 10)
        .await
        .expect("the orphan pass");
    assert_eq!(orphans, Vec::<String>::new());
    meta.swap_segment(swap(s, "gc/seg-ok", meta.now_ms(), 600_000))
        .await
        .into_result()
        .expect("a swap of an unclaimed segment");
}
