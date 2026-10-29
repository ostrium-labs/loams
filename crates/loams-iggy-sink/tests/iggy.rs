//! The sink against a real Iggy server: the image the `kafka` compose profile
//! pins (ostrium-labs/iggy). `deploy/release/ci/iggy-sink-it.sh` starts it and
//! runs these with:
//!
//! - `LOAMS_IGGY_SINK_IT`: Iggy's TCP address (`127.0.0.1:<port>`);
//! - `LOAMS_IGGY_SINK_IT_USER`, `LOAMS_IGGY_SINK_IT_PASSWORD`: the root user;
//! - `LOAMS_IGGY_SINK_IT_RESTART` (optional): a shell command that restarts
//!   the server, for the restart test;
//! - `LOAMS_IGGY_SINK_IT_KAFKA_TOPIC` (optional): a topic of the `kafka`
//!   stream the script produced to with kcat through the gateway, holding
//!   `k1:v1` (header `trace=abc`) and `k2:{"n":2}`.
//!
//! Without `LOAMS_IGGY_SINK_IT` each test prints a skip line and passes.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use iggy::prelude::{
    Client, HeaderKey, HeaderValue, Identifier, IggyClient, IggyClientBuilder, IggyMessage,
    MessageClient, Partitioning, StreamClient, StreamCreateOptions, TopicClient,
    TopicCreateOptions, UserClient,
};
use loams_iggy_sink::memory::MemoryTarget;
use loams_iggy_sink::{
    EVENT_TYPE_IGGY, EVENT_TYPE_KAFKA, IggySource, IggySourceConfig, Sink, SinkConfig, Source,
};

struct Server {
    address: String,
    user: String,
    password: String,
}

fn server(test: &str) -> Option<Server> {
    let Ok(address) = std::env::var("LOAMS_IGGY_SINK_IT") else {
        println!("skipped: {test} needs LOAMS_IGGY_SINK_IT (deploy/release/ci/iggy-sink-it.sh)");
        return None;
    };
    Some(Server {
        address,
        user: std::env::var("LOAMS_IGGY_SINK_IT_USER").unwrap_or_else(|_| "iggy".into()),
        password: std::env::var("LOAMS_IGGY_SINK_IT_PASSWORD")
            .expect("LOAMS_IGGY_SINK_IT_PASSWORD"),
    })
}

fn unique(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{prefix}-{}", nanos % 1_000_000_000_000)
}

async fn admin(server: &Server) -> IggyClient {
    let client = IggyClientBuilder::new()
        .with_tcp()
        .with_server_address(server.address.clone())
        .build()
        .expect("client");
    client.connect().await.expect("connect");
    client
        .login_user(&server.user, &server.password)
        .await
        .expect("login");
    client
}

async fn create(client: &IggyClient, stream: &str, topic: &str, partitions: u32) {
    client
        .create_stream(stream, &StreamCreateOptions::default())
        .await
        .expect("create stream");
    client
        .create_topic(
            &Identifier::named(stream).expect("name"),
            topic,
            &TopicCreateOptions {
                partitions_count: Some(partitions),
                ..TopicCreateOptions::default()
            },
        )
        .await
        .expect("create topic");
}

async fn send(client: &IggyClient, stream: &str, topic: &str, partition: u32, payloads: &[String]) {
    let mut headers = BTreeMap::new();
    headers.insert(
        HeaderKey::try_from("trace").expect("key"),
        HeaderValue::try_from("t-1").expect("value"),
    );
    let mut messages: Vec<IggyMessage> = payloads
        .iter()
        .map(|p| {
            IggyMessage::builder()
                .payload(Bytes::from(p.clone()))
                .user_headers(headers.clone())
                .build()
                .expect("message")
        })
        .collect();
    client
        .send_messages(
            &Identifier::named(stream).expect("name"),
            &Identifier::named(topic).expect("name"),
            &Partitioning::partition_id(partition),
            &mut messages,
        )
        .await
        .expect("send");
}

fn source(server: &Server, consumer: &str) -> Arc<IggySource> {
    Arc::new(
        IggySource::new(IggySourceConfig {
            address: server.address.clone(),
            username: server.user.clone(),
            password: server.password.clone(),
            consumer: consumer.into(),
            request_timeout: Duration::from_secs(10),
        })
        .expect("source"),
    )
}

fn config(stream: &str) -> SinkConfig {
    SinkConfig {
        iggy_stream: stream.into(),
        namespace: "it".into(),
        batch: 50,
        poll_interval: Duration::from_millis(20),
        discover_interval: Duration::from_millis(200),
        min_backoff: Duration::from_millis(20),
        max_backoff: Duration::from_millis(500),
        ..SinkConfig::default()
    }
}

async fn until(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    for _ in 0..secs * 20 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

/// Every (partition, offset) once, or a panic naming the duplicate.
fn offsets(target: &MemoryTarget, topic: &str) -> BTreeSet<(u32, u64)> {
    let mut seen = BTreeSet::new();
    for (partition, event) in target.events("it", topic) {
        let offset: u64 = event.id().parse().expect("offset");
        assert!(
            seen.insert((partition, offset)),
            "{partition}/{offset} twice"
        );
    }
    seen
}

fn payloads(range: std::ops::Range<u32>) -> Vec<String> {
    range.map(|i| format!("{{\"n\":{i}}}")).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_source_polls_and_commits_against_iggy() {
    let Some(server) = server("the_source_polls_and_commits_against_iggy") else {
        return;
    };
    let client = admin(&server).await;
    let stream = unique("src");
    create(&client, &stream, "t", 2).await;
    send(&client, &stream, "t", 1, &payloads(0..5)).await;

    let source = source(&server, "loams_sink_it");
    let topics = source.topics(&stream).await.expect("topics");
    assert_eq!(topics.len(), 1);
    assert_eq!((topics[0].name.as_str(), topics[0].partitions), ("t", 2));
    // Partitions are numbered from 0, as the mapping and Loams assume.
    assert!(
        source
            .poll(&stream, "t", 0, 0, 10)
            .await
            .expect("poll 0")
            .is_empty()
    );
    let polled = source.poll(&stream, "t", 1, 0, 10).await.expect("poll 1");
    assert_eq!(
        polled.iter().map(|m| m.offset).collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4]
    );
    assert_eq!(polled[2].payload.as_ref(), b"{\"n\":2}");
    let headers = polled[0].headers.as_ref().expect("headers");
    assert!(
        headers
            .iter()
            .any(|h| h.name == b"trace" && h.value.as_ref() == b"t-1")
    );
    // From an offset in the middle.
    let tail = source
        .poll(&stream, "t", 1, 3, 10)
        .await
        .expect("poll from 3");
    assert_eq!(
        tail.iter().map(|m| m.offset).collect::<Vec<_>>(),
        vec![3, 4]
    );

    assert_eq!(
        source.committed(&stream, "t", 1).await.expect("committed"),
        None
    );
    source.commit(&stream, "t", 1, 2).await.expect("commit");
    assert_eq!(
        source.committed(&stream, "t", 1).await.expect("committed"),
        Some(2)
    );
    // Another consumer has its own offsets.
    let other = self::source(&server, "someone_else");
    assert_eq!(
        other.committed(&stream, "t", 1).await.expect("committed"),
        None
    );
    // A missing stream has no topics (this server answers an empty list
    // rather than an error), so the runner simply waits for it.
    assert!(
        source
            .topics("no-such-stream")
            .await
            .map_or(true, |t| t.is_empty())
    );
    source.close().await;
    other.close().await;
    let _ = client
        .delete_stream(&Identifier::named(&stream).expect("name"))
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_sink_carries_every_message_and_resumes_after_a_kill() {
    let Some(server) = server("the_sink_carries_every_message_and_resumes_after_a_kill") else {
        return;
    };
    let client = admin(&server).await;
    let stream = unique("sink");
    create(&client, &stream, "orders", 3).await;
    for p in 0..3 {
        send(&client, &stream, "orders", p, &payloads(0..40)).await;
    }
    let target = Arc::new(MemoryTarget::default());
    let consumer = unique("loams_sink");
    let sink = Sink::new(config(&stream), source(&server, &consumer), target.clone()).start();
    until("the first 120", 30, || {
        target.events("it", "orders").len() == 120
    })
    .await;
    let first = sink.metrics().total();
    assert_eq!(first.appended, 120);
    assert!(
        target
            .events("it", "orders")
            .iter()
            .all(|(_, e)| e.ty() == EVENT_TYPE_IGGY)
    );

    // A kill while new messages arrive: nothing is lost, nothing lands twice.
    for p in 0..3 {
        send(&client, &stream, "orders", p, &payloads(40..60)).await;
    }
    sink.abort().await;
    for p in 0..3 {
        send(&client, &stream, "orders", p, &payloads(60..80)).await;
    }
    let src = source(&server, &consumer);
    let sink = Sink::new(config(&stream), src.clone(), target.clone()).start();
    until("all 240", 30, || target.events("it", "orders").len() == 240).await;
    let expect: BTreeSet<(u32, u64)> = (0..3).flat_map(|p| (0..80).map(move |o| (p, o))).collect();
    assert_eq!(offsets(&target, "orders"), expect);
    // Every partition's offset reaches Iggy.
    for p in 0..3 {
        for _ in 0..100 {
            if src.committed(&stream, "orders", p).await.ok().flatten() == Some(79) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(
            src.committed(&stream, "orders", p)
                .await
                .expect("committed"),
            Some(79)
        );
    }
    sink.stop().await;
    src.close().await;
    let _ = client
        .delete_stream(&Identifier::named(&stream).expect("name"))
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_sink_survives_an_iggy_restart() {
    let Some(server) = server("the_sink_survives_an_iggy_restart") else {
        return;
    };
    let Ok(restart) = std::env::var("LOAMS_IGGY_SINK_IT_RESTART") else {
        println!("skipped: the_sink_survives_an_iggy_restart needs LOAMS_IGGY_SINK_IT_RESTART");
        return;
    };
    let client = admin(&server).await;
    let stream = unique("restart");
    create(&client, &stream, "t", 1).await;
    send(&client, &stream, "t", 0, &payloads(0..30)).await;
    let target = Arc::new(MemoryTarget::default());
    let src = source(&server, "loams_sink");
    let sink = Sink::new(config(&stream), src.clone(), target.clone()).start();
    until("the first 30", 30, || target.events("it", "t").len() == 30).await;
    drop(client);

    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(&restart)
        .status()
        .expect("run the restart command");
    assert!(status.success(), "the restart command failed");
    // Iggy is back (with its data volume): new messages flow again.
    let client = {
        let mut connected = None;
        for _ in 0..60 {
            let c = IggyClientBuilder::new()
                .with_tcp()
                .with_server_address(server.address.clone())
                .build()
                .expect("client");
            if c.connect().await.is_ok()
                && c.login_user(&server.user, &server.password).await.is_ok()
            {
                connected = Some(c);
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        connected.expect("Iggy came back")
    };
    send(&client, &stream, "t", 0, &payloads(30..50)).await;
    until("all 50 after the restart", 60, || {
        target.events("it", "t").len() == 50
    })
    .await;
    // Every message once. The offsets after the restart need not follow on
    // from 29: the fork's server (0.9.1-edge.2) was seen to resume at 65537
    // after a restart, which the sink takes as a gap and carries on from.
    let got = offsets(&target, "t");
    assert_eq!(got.len(), 50);
    assert!((0..30).all(|o| got.contains(&(0, o))));
    let mut payloads: Vec<String> = target
        .events("it", "t")
        .iter()
        .map(|(_, e)| String::from_utf8(e.data().expect("data").to_vec()).expect("utf-8"))
        .collect();
    payloads.sort();
    let mut want = self::payloads(0..50);
    want.sort();
    assert_eq!(payloads, want);
    sink.stop().await;
    src.close().await;
    let _ = client
        .delete_stream(&Identifier::named(&stream).expect("name"))
        .await;
}

/// Records the Kafka gateway wrote (kcat, through the `kafka` profile) map to
/// Kafka record events: key, headers, value.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn records_from_the_kafka_gateway_map_to_kafka_events() {
    let Some(server) = server("records_from_the_kafka_gateway_map_to_kafka_events") else {
        return;
    };
    let Ok(topic) = std::env::var("LOAMS_IGGY_SINK_IT_KAFKA_TOPIC") else {
        println!(
            "skipped: records_from_the_kafka_gateway_map_to_kafka_events needs LOAMS_IGGY_SINK_IT_KAFKA_TOPIC"
        );
        return;
    };
    let target = Arc::new(MemoryTarget::default());
    let src = source(&server, &unique("loams_sink_gw"));
    let sink = Sink::new(
        SinkConfig {
            topics: Some(vec![topic.clone()]),
            ..config("kafka")
        },
        src.clone(),
        target.clone(),
    )
    .start();
    until("the two kcat records", 30, || {
        target.events("it", &topic).len() == 2
    })
    .await;
    sink.stop().await;
    src.close().await;
    let events = target.events("it", &topic);
    let (first, second) = (&events[0].1, &events[1].1);
    assert_eq!(first.ty(), EVENT_TYPE_KAFKA);
    assert_eq!(first.source(), format!("/iggy/kafka/{topic}/0"));
    assert_eq!(first.attr("partitionkey"), Some("k1"));
    assert_eq!(first.attr("kafkaheaders"), Some("{\"trace\":\"abc\"}"));
    assert_eq!(first.data().map(|d| d.as_ref()), Some(&b"v1"[..]));
    assert_eq!(first.datacontenttype(), Some("text/plain; charset=utf-8"));
    assert!(first.attr("time").is_some());
    assert_eq!(second.attr("partitionkey"), Some("k2"));
    assert_eq!(second.datacontenttype(), Some("application/json"));
}
