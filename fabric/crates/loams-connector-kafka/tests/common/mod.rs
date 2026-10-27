//! What the Kafka family's suites share: the systems under test (OR-2), the
//! pipelines that drive the connector, and raw clients to look at the topic.
//!
//! A real system is used only when its stack is up (Global Constraints,
//! "Tests skip without services"):
//!
//! | System | Gate | Address (default) |
//! |---|---|---|
//! | Apache Kafka 4.x (`deploy/fabric/compose.yaml` service `kafka`, profile `connectors-p2`) | `LOAMS_CONNECTORS_STACK` | `LOAMS_KAFKA_BOOTSTRAP` (`127.0.0.1:9092`) |
//! | Apache Kafka 4.x with SASL_SSL, PLAIN and a test CA (service `kafka-sasl`, profile `connectors-p2`), through the `confluent-cloud` profile | `LOAMS_CONNECTORS_STACK` | `LOAMS_KAFKA_SASL_BOOTSTRAP` (`SASL_SSL://localhost:9094`), CA `LOAMS_KAFKA_SASL_CA` (`deploy/fabric/kafka-sasl/ca.pem`) |
//! | Redpanda (service `redpanda`, profile `connectors-ci-only`) | `LOAMS_CONNECTORS_CI_ONLY_STACK` | `LOAMS_REDPANDA_BOOTSTRAP` (`127.0.0.1:19092`) |
//! | Event Hubs emulator (service `eventhubs`, profile `connectors-ci-only`) | `LOAMS_CONNECTORS_CI_ONLY_STACK` | `LOAMS_EVENTHUBS_BOOTSTRAP` (`127.0.0.1:9092`) |
//! | a real Event Hubs namespace (nightly) | `LOAMS_EVENTHUBS_NAMESPACE` | `<namespace>.servicebus.windows.net:9093` |
//!
//! Without its gate a suite prints why it skipped and passes; a skipped suite
//! never counts toward `preview` (OR-23). librdkafka's in-process mock
//! cluster serves the `mock_*` tests, which always run: they prove the
//! connector's own logic, never a system's behaviour.
#![allow(dead_code)]

use std::collections::BTreeSet;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;

use loams_connector_kafka::Kafka;
use loams_connector_kafka::config::Connection;
use loams_connector_sdk::pipeline::FABRIC_SPLIT;
use loams_connector_sdk::testing::memory::{MemoryCheckpointStore, MemoryFabric};
use loams_connector_sdk::{
    Checkpoint, CloudEvent, ConnectorContext, ConnectorFactory, EventBatch, InstanceKey, Pipeline,
    PipelineConfig, PipelineHandle, Position, Profile, Scrubber, SinkHandle, SplitId,
    StaticSecrets, TransactionalSink,
};
use rdkafka::admin::{AdminClient, AdminOptions, NewPartitions, NewTopic, TopicReplication};
use rdkafka::client::DefaultClientContext;
use rdkafka::consumer::{BaseConsumer, Consumer};
use rdkafka::message::OwnedMessage;
use rdkafka::mocking::MockCluster;
use rdkafka::producer::DefaultProducerContext;
use rdkafka::{ClientConfig, Message, Offset, TopicPartitionList};
use serde_json::{Value, json};

pub const GROUP: &str = "flow-kafka-suite";

/// The repository root (for profiles and manifests).
pub fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

pub fn load_profile(id: &str) -> Profile {
    let path = repo_root().join(format!("connectors/profiles/{id}.yaml"));
    let text = std::fs::read_to_string(&path).expect("profile file");
    serde_norway::from_str(&text).expect("profile parses")
}

/// A system under test.
pub struct Target {
    pub name: &'static str,
    pub bootstrap: String,
    /// Config every instance on this system carries (security, SASL).
    pub base: Value,
    pub secrets: Arc<StaticSecrets>,
    pub profile: Option<Profile>,
    /// Whether the suite may create topics (Event Hubs' are configured).
    pub can_admin: bool,
    /// A topic that already exists, and its partition count.
    pub fixed_topic: Option<(String, i32)>,
    /// The consumer group a source must use (an Event Hubs consumer group).
    pub group: Option<String>,
    /// The replication factor of the topics the suite creates (`-1`: the
    /// broker's default; Confluent Cloud refuses anything but 3 or the default).
    pub replication: i32,
    mock: Option<MockCluster<'static, DefaultProducerContext>>,
}

fn gate(var: &str, what: &str) -> bool {
    if std::env::var_os(var).is_some() {
        true
    } else {
        eprintln!("skipped: {what} needs {var} (its compose stack); see tests/common/mod.rs");
        false
    }
}

/// `var`, or `default` when it is unset or empty (the nightly job sets its
/// account variables from secrets that may be empty).
fn env_or(var: &str, default: &str) -> String {
    std::env::var(var)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_owned())
}

/// A unique suffix, so suites can share a broker.
pub fn unique(prefix: &str) -> String {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{prefix}-{n}")
}

impl Target {
    /// librdkafka's in-process mock cluster (no system; connector logic only).
    pub fn mock() -> Self {
        let mock = MockCluster::new(1).expect("mock cluster");
        Self {
            name: "kafka",
            bootstrap: mock.bootstrap_servers(),
            base: json!({ "security_protocol": "plaintext" }),
            secrets: Arc::new(StaticSecrets::new()),
            profile: None,
            can_admin: true,
            fixed_topic: None,
            group: None,
            replication: 1,
            mock: Some(mock),
        }
    }

    /// Apache Kafka, when the `connectors` stack is up.
    pub fn kafka() -> Option<Self> {
        if !gate("LOAMS_CONNECTORS_STACK", "the Apache Kafka suite") {
            return None;
        }
        Some(Self {
            name: "kafka",
            bootstrap: env_or("LOAMS_KAFKA_BOOTSTRAP", "127.0.0.1:9092"),
            base: json!({ "security_protocol": "plaintext" }),
            secrets: Arc::new(StaticSecrets::new()),
            profile: None,
            can_admin: true,
            fixed_topic: None,
            group: None,
            replication: 1,
            mock: None,
        })
    }

    /// Redpanda, when the `connectors-ci-only` stack is up.
    pub fn redpanda() -> Option<Self> {
        if !gate("LOAMS_CONNECTORS_CI_ONLY_STACK", "the Redpanda suite") {
            return None;
        }
        Some(Self {
            name: "redpanda",
            bootstrap: env_or("LOAMS_REDPANDA_BOOTSTRAP", "127.0.0.1:19092"),
            base: json!({ "security_protocol": "plaintext" }),
            secrets: Arc::new(StaticSecrets::new()),
            profile: Some(load_profile("redpanda")),
            can_admin: true,
            fixed_topic: None,
            group: None,
            replication: 1,
            mock: None,
        })
    }

    /// Azure Event Hubs' Kafka endpoint: a real namespace when
    /// `LOAMS_EVENTHUBS_NAMESPACE` is set (nightly), else the emulator when
    /// the `connectors-ci-only` stack is up. The profile's defaults
    /// (`sasl_ssl`, PLAIN, `$ConnectionString`) apply; the emulator listens
    /// without TLS, so its instances override `security_protocol`.
    pub fn eventhubs() -> Option<Self> {
        let profile = load_profile("azure-event-hubs");
        let hub = env_or("LOAMS_EVENTHUBS_HUB", "eh1");
        let partitions = env_or("LOAMS_EVENTHUBS_PARTITIONS", "2")
            .parse()
            .expect("LOAMS_EVENTHUBS_PARTITIONS");
        let group = env_or("LOAMS_EVENTHUBS_GROUP", "cg1");
        // The nightly job sets the variable from a secret that may be empty:
        // an empty namespace means no account, not a namespace named "".
        if let Some(ns) = std::env::var("LOAMS_EVENTHUBS_NAMESPACE")
            .ok()
            .filter(|v| !v.is_empty())
        {
            let cs = std::env::var("LOAMS_EVENTHUBS_CONNECTION_STRING")
                .expect("LOAMS_EVENTHUBS_CONNECTION_STRING with LOAMS_EVENTHUBS_NAMESPACE");
            return Some(Self {
                name: "azure-event-hubs",
                bootstrap: format!("{ns}.servicebus.windows.net:9093"),
                base: json!({ "sasl": { "password": "eventhubs-connection-string" } }),
                secrets: Arc::new(StaticSecrets::new().with("eventhubs-connection-string", cs)),
                profile: Some(profile),
                can_admin: false,
                fixed_topic: Some((hub, partitions)),
                group: Some(group),
                replication: 1,
                mock: None,
            });
        }
        if !gate("LOAMS_CONNECTORS_CI_ONLY_STACK", "the Event Hubs suite") {
            return None;
        }
        // The emulator's documented connection string (no secret: it is the
        // same for every emulator).
        let cs = env_or(
            "LOAMS_EVENTHUBS_CONNECTION_STRING",
            "Endpoint=sb://localhost;SharedAccessKeyName=RootManageSharedAccessKey;SharedAccessKey=SAS_KEY_VALUE;UseDevelopmentEmulator=true;",
        );
        Some(Self {
            name: "azure-event-hubs",
            bootstrap: env_or("LOAMS_EVENTHUBS_BOOTSTRAP", "127.0.0.1:9092"),
            base: json!({
                "security_protocol": "sasl_plaintext",
                "sasl": { "password": "eventhubs-connection-string" },
            }),
            secrets: Arc::new(StaticSecrets::new().with("eventhubs-connection-string", cs)),
            profile: Some(profile),
            can_admin: false,
            fixed_topic: Some((hub, partitions)),
            group: Some(group),
            replication: 1,
            mock: None,
        })
    }

    /// A Confluent Cloud cluster (CC0 Task 0 of plan CC1), when
    /// `LOAMS_CONFLUENT_BOOTSTRAP` is set: `sasl_ssl`, PLAIN with a Kafka API
    /// key (`LOAMS_CONFLUENT_API_KEY`, `LOAMS_CONFLUENT_API_SECRET`), full
    /// verification against the probed system roots. Never runs in CI. The
    /// key needs CREATE on the `loams-cc0-` topic prefix and the checkpoint
    /// topic (`LOAMS_CONFLUENT_CHECKPOINT_TOPIC`, default
    /// `loams-cc0-checkpoints`), READ on the `grp-` and `suite-reader-` group
    /// prefixes, WRITE on the `loams-` transactional-id prefix and
    /// IDEMPOTENT_WRITE.
    pub fn confluent() -> Option<Self> {
        let bootstrap = std::env::var("LOAMS_CONFLUENT_BOOTSTRAP")
            .ok()
            .filter(|v| !v.is_empty());
        let Some(bootstrap) = bootstrap else {
            eprintln!("skipped: the Confluent Cloud suite needs LOAMS_CONFLUENT_BOOTSTRAP");
            return None;
        };
        let key = std::env::var("LOAMS_CONFLUENT_API_KEY").expect("LOAMS_CONFLUENT_API_KEY");
        let secret =
            std::env::var("LOAMS_CONFLUENT_API_SECRET").expect("LOAMS_CONFLUENT_API_SECRET");
        Some(Self {
            name: "loams-cc0",
            bootstrap,
            base: json!({
                "security_protocol": "sasl_ssl",
                "sasl": { "mechanism": "plain", "username": key, "password": "confluent-api-secret" },
                "tls": { "enabled": true, "verify_full": true },
                "checkpoint_topic": env_or("LOAMS_CONFLUENT_CHECKPOINT_TOPIC", "loams-cc0-checkpoints"),
            }),
            secrets: Arc::new(StaticSecrets::new().with("confluent-api-secret", secret)),
            profile: None,
            can_admin: true,
            fixed_topic: None,
            group: None,
            replication: -1,
            mock: None,
        })
    }

    /// Apache Kafka behind SASL_SSL (service `kafka-sasl`), when the
    /// `connectors` stack is up, through the `confluent-cloud` target profile
    /// (CC1 Task 2, D1822): the profile's `sasl_ssl`, PLAIN and `verify_full`
    /// defaults, the username and password as secret names (as
    /// `loams-byoc-secrets` writes them for Confluent, D1827), a bootstrap
    /// string with Confluent's `SASL_SSL://` scheme, and the service's test
    /// CA in place of the system roots. The user and password are the
    /// service's fixed test values.
    pub fn kafka_sasl() -> Option<Self> {
        if !gate("LOAMS_CONNECTORS_STACK", "the Apache Kafka SASL_SSL suite") {
            return None;
        }
        let ca_path = std::env::var("LOAMS_KAFKA_SASL_CA")
            .ok()
            .filter(|v| !v.is_empty())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| repo_root().join("deploy/fabric/kafka-sasl/ca.pem"));
        let ca = std::fs::read_to_string(&ca_path).unwrap_or_else(|e| {
            panic!(
                "{}: {e}; the kafka-sasl service writes its test CA there when it starts",
                ca_path.display()
            )
        });
        Some(Self {
            name: "kafka-sasl",
            bootstrap: env_or("LOAMS_KAFKA_SASL_BOOTSTRAP", "SASL_SSL://localhost:9094"),
            base: json!({
                "sasl": {
                    "username_secret": "kafka-sasl/username",
                    "password": "kafka-sasl/password",
                },
                "tls": { "ca_pem": ca },
            }),
            secrets: Arc::new(
                StaticSecrets::new()
                    .with("kafka-sasl/username", "loams-test")
                    .with("kafka-sasl/password", "loams-test-password"),
            ),
            profile: Some(load_profile("confluent-cloud")),
            can_admin: true,
            fixed_topic: None,
            group: None,
            mock: None,
        })
    }

    pub fn mock_cluster(&self) -> &MockCluster<'static, DefaultProducerContext> {
        self.mock.as_ref().expect("a mock target")
    }

    pub fn factory(&self) -> Kafka {
        match &self.profile {
            Some(p) => Kafka::for_profile(p).expect("a kafka profile"),
            None => Kafka::new(),
        }
    }

    /// An instance config: the target's base, then `extra`.
    pub fn config(&self, topics: &[&str], extra: Value) -> Value {
        let mut cfg = json!({ "brokers": self.bootstrap, "topics": topics });
        merge(&mut cfg, &self.base);
        if let Some(g) = &self.group {
            merge(&mut cfg, &json!({ "group_id": g }));
        }
        merge(&mut cfg, &extra);
        cfg
    }

    pub fn ctx(&self, instance: &InstanceKey, config: Value) -> ConnectorContext {
        let mut ctx = ConnectorContext::new(
            instance.clone(),
            config,
            self.secrets.clone(),
            Scrubber::new(),
        );
        ctx.profile = self.profile.clone();
        ctx
    }

    /// A raw client config for this target (security and credentials as an
    /// instance would have them), for the suite's own admin and checks.
    pub async fn raw(&self) -> ClientConfig {
        let inst = InstanceKey::new(0, "suite");
        let mut cfg = self.config(&["unused"], json!({}));
        if let Some(p) = &self.profile {
            cfg = p.config_with_defaults(&cfg);
        }
        let ctx = self.ctx(&inst, cfg.clone());
        Connection::new("suite", &ctx, &cfg)
            .expect("config")
            .base()
            .await
            .expect("client config")
    }

    /// A topic for one test: a new one where the suite may create topics,
    /// else the configured hub.
    pub async fn topic(&self, prefix: &str, partitions: i32) -> (String, i32) {
        if let Some((t, n)) = &self.fixed_topic {
            return (t.clone(), *n);
        }
        let name = unique(prefix);
        self.create_topic(&name, partitions, false).await;
        (name, partitions)
    }

    pub async fn create_topic(&self, name: &str, partitions: i32, compacted: bool) {
        if let Some(m) = &self.mock {
            m.create_topic(name, partitions, 1).expect("mock topic");
            return;
        }
        let admin: AdminClient<DefaultClientContext> =
            self.raw().await.create().expect("admin client");
        let mut t = NewTopic::new(name, partitions, TopicReplication::Fixed(self.replication));
        if compacted {
            t = t.set("cleanup.policy", "compact");
        }
        let res = admin
            .create_topics([&t], &AdminOptions::new())
            .await
            .expect("create topics");
        for r in res {
            r.expect("topic created");
        }
    }

    pub async fn add_partitions(&self, name: &str, total: usize) {
        let admin: AdminClient<DefaultClientContext> =
            self.raw().await.create().expect("admin client");
        let res = admin
            .create_partitions([&NewPartitions::new(name, total)], &AdminOptions::new())
            .await
            .expect("create partitions");
        for r in res {
            r.expect("partitions added");
        }
    }

    /// Every record of `topic` up to its high watermarks, read with the
    /// given isolation level (`read_committed` or `read_uncommitted`).
    pub async fn read_all(&self, topic: &str, isolation: &str) -> Vec<OwnedMessage> {
        let mut cfg = self.raw().await;
        cfg.set("group.id", unique("suite-reader"))
            .set("enable.auto.commit", "false")
            .set("isolation.level", isolation);
        let topic = topic.to_owned();
        tokio::task::spawn_blocking(move || {
            let c: BaseConsumer = cfg.create().expect("reader");
            let md = c
                .fetch_metadata(Some(&topic), Duration::from_secs(30))
                .expect("metadata");
            let parts: Vec<i32> = md.topics()[0].partitions().iter().map(|p| p.id()).collect();
            let mut tpl = TopicPartitionList::new();
            let mut highs = Vec::new();
            for p in &parts {
                let (_, high) = c
                    .fetch_watermarks(&topic, *p, Duration::from_secs(30))
                    .expect("watermarks");
                highs.push((*p, high));
                tpl.add_partition_offset(&topic, *p, Offset::Beginning)
                    .expect("tpl");
            }
            c.assign(&tpl).expect("assign");
            let mut out = Vec::new();
            let deadline = std::time::Instant::now() + Duration::from_secs(60);
            loop {
                if let Some(m) = c.poll(Duration::from_millis(100)) {
                    out.push(m.expect("message").detach());
                }
                let pos = c.position().expect("position");
                let done = highs.iter().all(|(p, high)| {
                    *high == 0
                        || matches!(
                            pos.find_partition(&topic, *p).map(|e| e.offset()),
                            Some(Offset::Offset(o)) if o >= *high
                        )
                });
                if done || std::time::Instant::now() > deadline {
                    break;
                }
            }
            out
        })
        .await
        .expect("reader task")
    }
}

pub fn merge(base: &mut Value, over: &Value) {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            for (k, v) in o {
                match b.get_mut(k) {
                    Some(slot) => merge(slot, v),
                    None => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (b, o) => *b = o.clone(),
    }
}

/// Small batches, a checkpoint on every ack, a short linger.
pub fn small_config() -> PipelineConfig {
    PipelineConfig {
        max_batch_rows: 25,
        max_batch_bytes: 256 << 10,
        linger: Duration::from_millis(20),
        max_in_flight_batches: 4,
        memory_budget_bytes: 4 << 20,
        checkpoint_interval: Duration::ZERO,
        ..PipelineConfig::default()
    }
}

/// Wait (up to `secs`) until `cond` holds.
pub async fn eventually(what: &str, secs: u64, mut cond: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    while !cond() {
        if tokio::time::Instant::now() > deadline {
            panic!("timed out waiting for: {what}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// `n` CloudEvents `<prefix>/<i>` on `fabric`, from `from`.
pub fn push_events(fabric: &MemoryFabric, prefix: &str, from: u64, n: u64) -> Vec<String> {
    (from..from + n)
        .map(|i| {
            let id = format!("{prefix}/{i}");
            fabric.push(
                CloudEvent::new(
                    id.clone(),
                    "/upstream/orders",
                    "com.example.order.created.v1",
                )
                .with_data("application/json", format!("{{\"i\":{i}}}"))
                .with_extension("partitionkey", format!("k{}", i % 7)),
            );
            id
        })
        .collect()
}

/// A sink pipeline from `fabric`'s group into the target.
pub async fn sink_pipeline(
    target: &Target,
    instance: &InstanceKey,
    config: Value,
    fabric: &MemoryFabric,
    epoch: u64,
) -> PipelineHandle {
    let factory = target
        .factory()
        .sink(&target.ctx(instance, config))
        .expect("sink factory")
        .expect("sink offered");
    Pipeline::sink(instance.clone(), small_config())
        .reader(Box::new(fabric.reader(GROUP)))
        .sink(Arc::from(factory))
        .dead_letters(Arc::new(fabric.clone()))
        .epoch(epoch)
        .start()
        .await
        .expect("sink pipeline starts")
}

/// A source pipeline from the target into `fabric`.
pub async fn source_pipeline(
    target: &Target,
    instance: &InstanceKey,
    config: Value,
    fabric: &MemoryFabric,
    store: &MemoryCheckpointStore,
    discover_every: Duration,
) -> PipelineHandle {
    let (enumerator, sources) = target
        .factory()
        .source(&target.ctx(instance, config))
        .expect("source factory")
        .expect("source offered");
    Pipeline::source(instance.clone(), small_config())
        .enumerator(enumerator)
        .sources(Arc::from(sources))
        .writer(Arc::new(fabric.clone()))
        .checkpoints(Arc::new(store.clone()))
        .dead_letters(Arc::new(fabric.clone()))
        .discover_every(discover_every)
        .epoch(1)
        .start()
        .await
        .expect("source pipeline starts")
}

/// The ids of `fabric`'s events.
pub fn id_set(fabric: &MemoryFabric) -> BTreeSet<String> {
    fabric.ids().into_iter().collect()
}

/// The contract, roundtrip, kill-restart and dup-check suites in one pass
/// (§33 §4 rule 3), against `target`:
///
/// 1. the sink writes `n` events from the Fabric to a topic;
/// 2. the source reads them back into a second Fabric, with their CloudEvents
///    identity (`type`, `source`, `id`) intact;
/// 3. the source is stopped, `n` more are written, and a new source pipeline
///    on the same checkpoint store resumes: nothing is lost, and anything
///    read twice is dropped by `ingest`'s dedup (the second Fabric).
pub async fn roundtrip_suite(target: &Target) {
    let (topic, partitions) = target.topic(&format!("{}-suite", target.name), 2).await;
    let n = 60u64;
    let run = unique("run");

    let sink_inst = InstanceKey::new(1, unique("sink"));
    let upstream = MemoryFabric::new();
    let mut sent: BTreeSet<String> = push_events(&upstream, &run, 0, n).into_iter().collect();
    let sink = sink_pipeline(
        target,
        &sink_inst,
        target.config(&[&topic], json!({})),
        &upstream,
        1,
    )
    .await;
    eventually("the sink writes the first batch", 120, || {
        upstream.committed(GROUP) == n
    })
    .await;

    let src_inst = InstanceKey::new(1, unique("source"));
    let src_cfg = target.config(
        &[&topic],
        json!({ "auto_offset_reset": "earliest", "group_id_prefix": unique("grp") }),
    );
    let ingest = MemoryFabric::new();
    let store = MemoryCheckpointStore::new();
    let source = source_pipeline(
        target,
        &src_inst,
        src_cfg.clone(),
        &ingest,
        &store,
        Duration::from_secs(3600),
    )
    .await;
    eventually("the source reads the first batch back", 120, || {
        sent.iter().all(|id| ingest.ids().contains(id))
    })
    .await;
    assert_eq!(
        source.status().splits.len(),
        partitions as usize,
        "one split per partition"
    );
    for ev in ingest.events() {
        if ev.id.starts_with(&run) {
            assert_eq!(ev.ty, "com.example.order.created.v1", "type kept");
            assert_eq!(ev.source, "/upstream/orders", "source kept");
        }
    }
    source.stop().await;

    // Kill-restart: more data while the source is down, then a new pipeline
    // on the same checkpoints.
    sent.extend(push_events(&upstream, &run, n, n));
    eventually("the sink writes the second batch", 120, || {
        upstream.committed(GROUP) == 2 * n
    })
    .await;
    sink.stop().await;
    let source = source_pipeline(
        target,
        &src_inst,
        src_cfg,
        &ingest,
        &store,
        Duration::from_secs(3600),
    )
    .await;
    eventually("the restarted source catches up", 120, || {
        let got = id_set(&ingest);
        sent.iter().all(|id| got.contains(id))
    })
    .await;
    source.stop().await;
    let ours: Vec<_> = ingest
        .ids()
        .into_iter()
        .filter(|id| id.starts_with(&run))
        .collect();
    assert_eq!(ours.len() as u64, 2 * n, "each event ingested once");
}

/// A raw record: key, payload and headers.
pub type RawRecord = (Option<String>, Vec<u8>, Vec<(String, String)>);

/// Produce raw records (key, payload, headers) to `topic`, optionally to one
/// partition, as an upstream producer would.
pub async fn produce_raw(
    target: &Target,
    topic: &str,
    partition: Option<i32>,
    records: Vec<RawRecord>,
) {
    use rdkafka::message::{Header, OwnedHeaders};
    use rdkafka::producer::{FutureProducer, FutureRecord};
    let p: FutureProducer = target.raw().await.create().expect("producer");
    for (key, payload, headers) in &records {
        let mut h = OwnedHeaders::new();
        for (k, v) in headers {
            h = h.insert(Header {
                key: k,
                value: Some(v.as_bytes()),
            });
        }
        let mut r: FutureRecord<'_, str, [u8]> = FutureRecord::to(topic)
            .payload(payload.as_slice())
            .headers(h);
        if let Some(k) = key {
            r = r.key(k.as_str());
        }
        if let Some(p) = partition {
            r = r.partition(p);
        }
        p.send(r, Duration::from_secs(30))
            .await
            .map_err(|(e, _)| e)
            .expect("record produced");
    }
}

/// A batch of `n` Fabric events `<prefix>/<i>`, from `from`.
pub fn batch(prefix: &str, from: u64, n: u64) -> EventBatch {
    EventBatch::Events(
        (from..from + n)
            .map(|i| {
                CloudEvent::new(
                    format!("{prefix}/{i}"),
                    "/upstream/orders",
                    "com.example.order.v1",
                )
                .with_data("application/json", format!("{{\"i\":{i}}}"))
            })
            .collect(),
    )
}

/// The checkpoint the pipeline would prepare after the Fabric offset `upto`.
pub fn fabric_checkpoint(instance: &InstanceKey, epoch: u64, upto: u64) -> Checkpoint {
    let mut cp = Checkpoint::new(instance.clone(), epoch);
    cp.splits
        .insert(SplitId::new(FABRIC_SPLIT), Position::from_u64(upto));
    cp
}

fn fabric_offset(cp: Option<Checkpoint>) -> Option<u64> {
    cp.and_then(|c| {
        c.splits
            .get(&SplitId::new(FABRIC_SPLIT))
            .and_then(Position::as_u64)
    })
}

/// The instance's sink, which must be transactional.
pub fn txn_sink(
    target: &Target,
    instance: &InstanceKey,
    config: Value,
) -> Box<dyn TransactionalSink> {
    let factory = target
        .factory()
        .sink(&target.ctx(instance, config))
        .expect("sink factory")
        .expect("sink offered");
    match factory.create().expect("sink") {
        SinkHandle::Transactional(t) => t,
        other => panic!("expected a transactional sink, got {other:?}"),
    }
}

/// Drop a sink the way a crash would (no abort, no commit), off the async
/// runtime: librdkafka's destroy blocks.
pub async fn crash(sink: Box<dyn TransactionalSink>) {
    tokio::task::spawn_blocking(move || drop(sink))
        .await
        .expect("drop");
}

/// `kafka_txn_recover_reads_checkpoint`: what a committed transaction stored
/// is what the next sink's `recover()` returns; a prepared but uncommitted
/// one is not (checked only where the broker really aborts, so not on the
/// mock cluster); another instance's checkpoint on the same topic is not ours.
pub async fn recover_reads_checkpoint_case(target: &Target) {
    let aborts = target.mock.is_none();
    let (topic, _) = target
        .topic(&format!("{}-txn-recover", target.name), 1)
        .await;
    let inst = InstanceKey::new(3, unique("txn-recover"));
    let cfg = target.config(&[&topic], json!({ "delivery": "exactly_once" }));

    let mut s1 = txn_sink(target, &inst, cfg.clone());
    assert_eq!(
        s1.recover().await.expect("recover"),
        None,
        "a new instance has no checkpoint"
    );
    s1.write(batch(&inst.name, 0, 5)).await.expect("write");
    let cp5 = fabric_checkpoint(&inst, 2, 5);
    let t = s1.prepare(&cp5).await.expect("prepare");
    s1.commit(t.clone()).await.expect("commit");
    s1.commit(t).await.expect("commit is idempotent");
    if aborts {
        // A second transaction, prepared and never committed.
        s1.write(batch(&inst.name, 5, 5)).await.expect("write");
        s1.prepare(&fabric_checkpoint(&inst, 2, 10))
            .await
            .expect("prepare");
    }
    crash(s1).await;

    let mut s2 = txn_sink(target, &inst, cfg.clone());
    assert_eq!(s2.recover().await.expect("recover"), Some(cp5));
    crash(s2).await;

    let other = InstanceKey::new(3, unique("txn-recover-other"));
    let mut o = txn_sink(target, &other, cfg);
    assert_eq!(o.recover().await.expect("recover"), None);
    crash(o).await;
}

/// `kafka_txn_sink_exactly_once_under_crash`: a crash after `write`, after
/// `prepare`, and after `commit` but before the Fabric commit; each restart
/// recovers, and the topic read `read_committed` holds every event exactly
/// once. Needs a broker that really aborts (not the mock cluster).
pub async fn exactly_once_under_crash_case(target: &Target) {
    let (topic, _) = target.topic(&format!("{}-txn-crash", target.name), 2).await;
    let inst = InstanceKey::new(3, unique("txn-crash"));
    let cfg = target.config(&[&topic], json!({ "delivery": "exactly_once" }));
    let p = inst.name.clone();

    // Batch 1 commits.
    let mut s = txn_sink(target, &inst, cfg.clone());
    assert_eq!(fabric_offset(s.recover().await.expect("recover")), None);
    s.write(batch(&p, 0, 10)).await.expect("write");
    let t = s
        .prepare(&fabric_checkpoint(&inst, 1, 10))
        .await
        .expect("prepare");
    s.commit(t).await.expect("commit");
    // Crash after write (batch 2).
    s.write(batch(&p, 10, 10)).await.expect("write");
    crash(s).await;

    let mut s = txn_sink(target, &inst, cfg.clone());
    assert_eq!(
        fabric_offset(s.recover().await.expect("recover")),
        Some(10),
        "after a crash in write"
    );
    // Crash after prepare (batch 2 again).
    s.write(batch(&p, 10, 10)).await.expect("write");
    s.prepare(&fabric_checkpoint(&inst, 1, 20))
        .await
        .expect("prepare");
    crash(s).await;

    let mut s = txn_sink(target, &inst, cfg.clone());
    assert_eq!(
        fabric_offset(s.recover().await.expect("recover")),
        Some(10),
        "after a crash in prepare"
    );
    // Crash after commit, before the Fabric offset is committed.
    s.write(batch(&p, 10, 10)).await.expect("write");
    let t = s
        .prepare(&fabric_checkpoint(&inst, 1, 20))
        .await
        .expect("prepare");
    s.commit(t).await.expect("commit");
    crash(s).await;

    let mut s = txn_sink(target, &inst, cfg.clone());
    assert_eq!(
        fabric_offset(s.recover().await.expect("recover")),
        Some(20),
        "after a crash between commit and the Fabric commit, batch 2 is skipped"
    );
    s.write(batch(&p, 20, 10)).await.expect("write");
    let t = s
        .prepare(&fabric_checkpoint(&inst, 1, 30))
        .await
        .expect("prepare");
    s.commit(t).await.expect("commit");
    crash(s).await;

    let committed: Vec<String> = target
        .read_all(&topic, "read_committed")
        .await
        .iter()
        .filter_map(ce_id)
        .collect();
    let mut sorted = committed.clone();
    sorted.sort();
    let mut expected: Vec<String> = (0..30).map(|i| format!("{p}/{i}")).collect();
    expected.sort();
    assert_eq!(sorted, expected, "every event exactly once, read_committed");
    let uncommitted = target.read_all(&topic, "read_uncommitted").await.len();
    assert!(
        uncommitted > committed.len(),
        "the crashed transactions left records that read_committed hides ({uncommitted} vs {})",
        committed.len()
    );
}

/// A record's `ce_id` header.
pub fn ce_id(m: &OwnedMessage) -> Option<String> {
    header(m, "ce_id")
}

pub fn header(m: &OwnedMessage, name: &str) -> Option<String> {
    use rdkafka::message::Headers;
    m.headers().and_then(|h| {
        h.iter()
            .find(|x| x.key == name)
            .and_then(|x| x.value)
            .map(|v| String::from_utf8_lossy(v).into_owned())
    })
}

/// A request the fake registry saw.
#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub auth: Option<String>,
    pub body: Value,
}

/// A fake Confluent Schema Registry on loopback that answers `routes`
/// (method, path, status, JSON body) and 404 (`40403`) otherwise, as
/// Confluent's REST API documents; it records every request.
pub async fn fake_registry_with(
    routes: Vec<(&'static str, &'static str, u16, Value)>,
) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let routes = Arc::new(routes);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let log = seen.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let log = log.clone();
            let routes = routes.clone();
            tokio::spawn(async move {
                let svc = hyper::service::service_fn(move |req: Request<hyper::body::Incoming>| {
                    let log = log.clone();
                    let routes = routes.clone();
                    async move {
                        let (parts, body) = req.into_parts();
                        let body = body.collect().await.expect("body").to_bytes();
                        let s = Seen {
                            method: parts.method.to_string(),
                            path: parts.uri.path().to_owned(),
                            auth: parts
                                .headers
                                .get("authorization")
                                .and_then(|v| v.to_str().ok())
                                .map(str::to_owned),
                            body: serde_json::from_slice(&body).unwrap_or(Value::Null),
                        };
                        log.lock().expect("log").push(s.clone());
                        let (status, body) = routes
                            .iter()
                            .find(|(m, p, _, _)| *m == s.method && *p == s.path)
                            .map(|(_, _, status, body)| (*status, body.clone()))
                            .unwrap_or_else(|| {
                                (
                                    404,
                                    json!({ "error_code": 40403, "message": "Schema not found" }),
                                )
                            });
                        Ok::<_, Infallible>(
                            Response::builder()
                                .status(StatusCode::from_u16(status).expect("status"))
                                .header("content-type", "application/vnd.schemaregistry.v1+json")
                                .body(Full::new(Bytes::from(body.to_string())))
                                .expect("response"),
                        )
                    }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), svc)
                    .await;
            });
        }
    });
    (base, seen)
}
