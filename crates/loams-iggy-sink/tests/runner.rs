//! The runner against the in-memory source and target: at least once,
//! deduplicated, backpressure, restart from the committed offset.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use loams_iggy_sink::memory::{Fault, MemorySource, MemoryTarget};
use loams_iggy_sink::{Sink, SinkConfig, SinkHandle};

fn config() -> SinkConfig {
    SinkConfig {
        iggy_stream: "kafka".into(),
        namespace: "kafka".into(),
        batch: 7,
        poll_interval: Duration::from_millis(5),
        discover_interval: Duration::from_millis(20),
        min_backoff: Duration::from_millis(1),
        max_backoff: Duration::from_millis(20),
        ..SinkConfig::default()
    }
}

fn start(source: &Arc<MemorySource>, target: &Arc<MemoryTarget>, config: SinkConfig) -> SinkHandle {
    Sink::new(config, source.clone(), target.clone()).start()
}

/// Waits until `f` holds, for up to 10 s.
async fn until(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..1000 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}");
}

/// The offsets in `namespace/stream` per partition, asserting none twice.
fn offsets(target: &MemoryTarget, stream: &str) -> Vec<(u32, u64)> {
    let mut seen = BTreeSet::new();
    for (partition, event) in target.events("kafka", stream) {
        let offset: u64 = event.id().parse().expect("the id is the offset");
        assert!(
            event.source().ends_with(&format!("/{partition}")),
            "the source names the partition"
        );
        assert!(
            seen.insert((partition, offset)),
            "offset {partition}/{offset} appended twice"
        );
    }
    seen.into_iter().collect()
}

fn all(partitions: u32, per: u64) -> Vec<(u32, u64)> {
    (0..partitions)
        .flat_map(|p| (0..per).map(move |o| (p, o)))
        .collect()
}

#[tokio::test]
async fn every_message_lands_once_and_offsets_are_committed() {
    let (source, target) = (
        Arc::new(MemorySource::default()),
        Arc::new(MemoryTarget::default()),
    );
    source.create_topic("kafka", "orders", 3);
    for p in 0..3 {
        for i in 0..25 {
            source.send("kafka", "orders", p, format!("{{\"n\":{i}}}").as_bytes());
        }
    }
    let sink = start(&source, &target, config());
    until("all 75 events", || {
        target.events("kafka", "orders").len() == 75
    })
    .await;
    until("every partition committed", || {
        (0..3).all(|p| source.committed_offset("kafka", "orders", p) == Some(24))
    })
    .await;
    assert_eq!(offsets(&target, "orders"), all(3, 25));
    assert_eq!(target.stream_partitions("kafka", "orders"), Some(3));
    let metrics = sink.metrics();
    sink.stop().await;
    let total = metrics.total();
    assert_eq!((total.appended, total.duplicates), (75, 0));
    assert_eq!(
        metrics.partition("orders", 1).and_then(|m| m.committed),
        Some(24)
    );
}

#[tokio::test]
async fn new_messages_and_topics_are_picked_up() {
    let (source, target) = (
        Arc::new(MemorySource::default()),
        Arc::new(MemoryTarget::default()),
    );
    source.create_topic("kafka", "a", 1);
    let sink = start(&source, &target, config());
    source.send("kafka", "a", 0, b"1");
    until("a/0", || target.events("kafka", "a").len() == 1).await;
    source.create_topic("kafka", "b", 2);
    source.send("kafka", "b", 1, b"x");
    source.send("kafka", "a", 0, b"2");
    until("b/1 and a/1", || {
        target.events("kafka", "b").len() == 1 && target.events("kafka", "a").len() == 2
    })
    .await;
    sink.stop().await;
    assert_eq!(offsets(&target, "b"), vec![(1, 0)]);
}

#[tokio::test]
async fn backpressure_pauses_without_loss_or_early_commit() {
    let (source, target) = (
        Arc::new(MemorySource::default()),
        Arc::new(MemoryTarget::default()),
    );
    source.create_topic("kafka", "t", 1);
    for i in 0..20 {
        source.send("kafka", "t", 0, format!("{i}").as_bytes());
    }
    target.inject([
        Fault::Backpressure(Some(Duration::from_millis(30))),
        Fault::Backpressure(None),
        Fault::Unavailable,
        Fault::Backpressure(Some(Duration::from_millis(30))),
    ]);
    let sink = start(&source, &target, config());
    // While the target refuses, nothing is committed.
    tokio::time::sleep(Duration::from_millis(15)).await;
    assert_eq!(source.committed_offset("kafka", "t", 0), None);
    until("all 20", || target.events("kafka", "t").len() == 20).await;
    until("committed", || {
        source.committed_offset("kafka", "t", 0) == Some(19)
    })
    .await;
    let metrics = sink.metrics();
    sink.stop().await;
    assert_eq!(offsets(&target, "t"), all(1, 20));
    let total = metrics.total();
    assert_eq!(total.backpressure, 3);
    assert_eq!(total.errors, 1);
}

#[tokio::test]
async fn in_flight_answers_are_written_again() {
    let (source, target) = (
        Arc::new(MemorySource::default()),
        Arc::new(MemoryTarget::default()),
    );
    source.create_topic("kafka", "t", 1);
    for i in 0..7 {
        source.send("kafka", "t", 0, format!("{i}").as_bytes());
    }
    target.inject([Fault::PartialInFlight(3)]);
    let sink = start(&source, &target, config());
    until("committed", || {
        source.committed_offset("kafka", "t", 0) == Some(6)
    })
    .await;
    let metrics = sink.metrics();
    sink.stop().await;
    assert_eq!(offsets(&target, "t"), all(1, 7));
    let total = metrics.total();
    assert_eq!(
        (total.appended, total.duplicates, total.in_flight),
        (7, 3, 1)
    );
}

#[tokio::test]
async fn a_restart_resumes_from_the_committed_offset() {
    let (source, target) = (
        Arc::new(MemorySource::default()),
        Arc::new(MemoryTarget::default()),
    );
    source.create_topic("kafka", "t", 1);
    for i in 0..10 {
        source.send("kafka", "t", 0, format!("{i}").as_bytes());
    }
    let sink = start(&source, &target, config());
    until("committed", || {
        source.committed_offset("kafka", "t", 0) == Some(9)
    })
    .await;
    sink.stop().await;
    let writes = target.writes();
    for i in 10..15 {
        source.send("kafka", "t", 0, format!("{i}").as_bytes());
    }
    let sink = start(&source, &target, config());
    until("committed", || {
        source.committed_offset("kafka", "t", 0) == Some(14)
    })
    .await;
    let metrics = sink.metrics();
    sink.stop().await;
    assert_eq!(offsets(&target, "t"), all(1, 15));
    // The second run read only what was new.
    assert_eq!(metrics.total().duplicates, 0);
    assert!(target.writes() > writes);
}

/// A crash between the write and the commit: the restarted sink writes the
/// uncommitted batch again and the target's dedup answers duplicates, so the
/// stream holds every offset exactly once.
#[tokio::test]
async fn a_kill_between_write_and_commit_replays_as_duplicates() {
    let (source, target) = (
        Arc::new(MemorySource::default()),
        Arc::new(MemoryTarget::default()),
    );
    source.create_topic("kafka", "t", 2);
    for p in 0..2 {
        for i in 0..30 {
            source.send("kafka", "t", p, format!("{i}").as_bytes());
        }
    }
    // Every commit fails: the sink keeps writing, and commits nothing.
    source.fail_next_commits(u32::MAX);
    let sink = start(&source, &target, config());
    until("all 60 written", || target.events("kafka", "t").len() == 60).await;
    sink.abort().await;
    assert_eq!(source.committed_offset("kafka", "t", 0), None);
    source.fail_next_commits(0);

    let sink = start(&source, &target, config());
    until("both committed", || {
        (0..2).all(|p| source.committed_offset("kafka", "t", p) == Some(29))
    })
    .await;
    let metrics = sink.metrics();
    sink.stop().await;
    assert_eq!(offsets(&target, "t"), all(2, 30));
    assert_eq!(metrics.total().appended, 0, "the replay appended nothing");
    assert_eq!(metrics.total().duplicates, 60);
}

#[tokio::test]
async fn kills_at_random_points_never_lose_or_duplicate() {
    let (source, target) = (
        Arc::new(MemorySource::default()),
        Arc::new(MemoryTarget::default()),
    );
    source.create_topic("kafka", "t", 3);
    let mut sent = 0u64;
    for round in 0u64..8 {
        for p in 0..3 {
            for _ in 0..(5 + round) {
                source.send("kafka", "t", p, b"v");
                sent += 1;
            }
        }
        if round % 3 == 1 {
            source.fail_next_polls(2);
            source.fail_next_commits(1);
        }
        let sink = start(&source, &target, config());
        tokio::time::sleep(Duration::from_millis(3 + round * 4)).await;
        sink.abort().await;
    }
    let sink = start(&source, &target, config());
    until("everything", || {
        target.events("kafka", "t").len() as u64 == sent
    })
    .await;
    sink.stop().await;
    let got = offsets(&target, "t");
    assert_eq!(got.len() as u64, sent);
}

#[tokio::test]
async fn names_loams_refuses_are_skipped_and_reported() {
    let (source, target) = (
        Arc::new(MemorySource::default()),
        Arc::new(MemoryTarget::default()),
    );
    source.create_topic("kafka", "__consumer_offsets", 1);
    source.create_topic("kafka", "has space", 1);
    source.create_topic("kafka", "ok.topic-1", 1);
    source.send("kafka", "__consumer_offsets", 0, b"x");
    source.send("kafka", "ok.topic-1", 0, b"x");
    let sink = start(&source, &target, config());
    until("ok.topic-1", || {
        target.events("kafka", "ok.topic-1").len() == 1
    })
    .await;
    let metrics = sink.metrics();
    sink.stop().await;
    let json = metrics.to_json();
    let skipped = json["skipped_topics"].as_object().expect("an object");
    assert!(skipped.contains_key("__consumer_offsets") && skipped.contains_key("has space"));
    assert_eq!(
        target.stream_partitions("kafka", "__consumer_offsets"),
        None
    );
}

#[tokio::test]
async fn a_stream_that_cannot_be_created_is_retried() {
    let (source, target) = (
        Arc::new(MemorySource::default()),
        Arc::new(MemoryTarget::default()),
    );
    source.create_topic("kafka", "t", 1);
    source.send("kafka", "t", 0, b"x");
    target.refuse_streams(Some("the engine is starting"));
    let sink = start(&source, &target, config());
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(target.events("kafka", "t").is_empty());
    assert!(
        sink.metrics().to_json()["last_discovery_error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("starting"))
    );
    target.refuse_streams(None);
    until("t", || target.events("kafka", "t").len() == 1).await;
    sink.stop().await;
}

#[tokio::test]
async fn the_topic_filter_and_byte_budget_apply() {
    let (source, target) = (
        Arc::new(MemorySource::default()),
        Arc::new(MemoryTarget::default()),
    );
    source.create_topic("kafka", "in", 1);
    source.create_topic("kafka", "out", 1);
    for _ in 0..6 {
        source.send("kafka", "in", 0, &[b'x'; 100]);
    }
    source.send("kafka", "out", 0, b"x");
    let sink = start(
        &source,
        &target,
        SinkConfig {
            topics: Some(vec!["in".into()]),
            max_batch_bytes: 250,
            ..config()
        },
    );
    until("in", || {
        source.committed_offset("kafka", "in", 0) == Some(5)
    })
    .await;
    sink.stop().await;
    assert_eq!(offsets(&target, "in"), all(1, 6));
    assert!(target.events("kafka", "out").is_empty());
    // 100-byte messages under a 250-byte budget: two per write.
    assert_eq!(target.writes(), 3);
}
