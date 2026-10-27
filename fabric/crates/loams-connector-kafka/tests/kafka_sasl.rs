//! The Kafka family over SASL_SSL (CC1 Task 2, D1822): Apache Kafka 4.3.1
//! with a SASL_SSL listener, PLAIN users and a test CA (the `kafka-sasl`
//! service), driven through the `confluent-cloud` target profile, which is
//! the shape a Confluent Cloud cluster has. Skipped without
//! `LOAMS_CONNECTORS_STACK`; in CI the `connectors-p2` kafka shard runs:
//!
//! ```text
//! docker compose -f deploy/fabric/compose.yaml --profile connectors-p2 up -d --wait kafka-sasl
//! LOAMS_CONNECTORS_STACK=1 cargo test -p loams-connector-kafka --test kafka_sasl -j 2
//! ```
//!
//! A real Confluent Cloud cluster runs the same cases nightly (CC1 Task 10).

mod common;

use common::*;
use std::sync::Arc;

use loams_connector_kafka::config::Connection;
use loams_connector_kafka::{CHECKPOINT_TOPIC, KafkaEnumerator};
use loams_connector_sdk::{
    ConnectorContext, ErrorClass, InstanceKey, Scrubber, SplitEnumerator, StaticSecrets,
};
use rdkafka::admin::{AdminClient, AdminOptions, NewTopic, TopicReplication};
use rdkafka::client::DefaultClientContext;
use rdkafka::types::RDKafkaErrorCode;
use serde_json::{Value, json};

/// The profile does not create the checkpoint topic (`create_checkpoint_topic:
/// false`; on Confluent the module or the user does), so the suite creates
/// it as they would, once.
async fn ensure_checkpoint_topic(target: &Target) {
    let admin: AdminClient<DefaultClientContext> =
        target.raw().await.create().expect("admin client");
    let t = NewTopic::new(CHECKPOINT_TOPIC, 1, TopicReplication::Fixed(1))
        .set("cleanup.policy", "compact");
    for r in admin
        .create_topics([&t], &AdminOptions::new())
        .await
        .expect("create topics")
    {
        match r {
            Ok(_) | Err((_, RDKafkaErrorCode::TopicAlreadyExists)) => {}
            Err((name, code)) => panic!("creating {name}: {code}"),
        }
    }
}

/// A metadata call with `config` (through the profile) and `secrets`; its
/// error class, if any.
async fn discover_with(
    target: &Target,
    config: Value,
    secrets: Arc<StaticSecrets>,
) -> Result<(), ErrorClass> {
    let inst = InstanceKey::new(5, unique("sasl-refused"));
    let mut cfg = config;
    if let Some(p) = &target.profile {
        cfg = p.config_with_defaults(&cfg);
    }
    let ctx = ConnectorContext::new(inst, cfg.clone(), secrets, Scrubber::new());
    let conn = Connection::new("kafka", &ctx, &cfg).expect("config");
    let mut enumerator = KafkaEnumerator::new(conn);
    enumerator
        .discover(&[])
        .await
        .map(|_| ())
        .map_err(|e| e.class)
}

/// Roundtrip, kill-restart and dup-check through the profile over SASL_SSL.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn kafka_sasl_suite() {
    let Some(target) = Target::kafka_sasl() else {
        return;
    };
    roundtrip_suite(&target).await;
}

/// The exactly-once sink over SASL_SSL, with the checkpoint topic created
/// beforehand (the profile's `create_checkpoint_topic: false`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn kafka_sasl_txn_exactly_once() {
    let Some(target) = Target::kafka_sasl() else {
        return;
    };
    ensure_checkpoint_topic(&target).await;
    recover_reads_checkpoint_case(&target).await;
    exactly_once_under_crash_case(&target).await;
}

/// A wrong password is `Auth`, and without the test CA (the profile's
/// probed system roots) the broker's certificate is not trusted: the
/// listener really authenticates and really verifies.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn kafka_sasl_refuses_bad_password_and_unknown_ca() {
    let Some(target) = Target::kafka_sasl() else {
        return;
    };
    let (topic, _) = target.topic("kafka-sasl-refused", 1).await;
    assert_eq!(
        discover_with(
            &target,
            target.config(&[&topic], json!({})),
            target.secrets.clone()
        )
        .await,
        Ok(()),
        "the right credentials and CA connect"
    );

    let wrong = StaticSecrets::new()
        .with("kafka-sasl/username", "loams-test")
        .with("kafka-sasl/password", "not-the-password");
    let err = discover_with(
        &target,
        target.config(&[&topic], json!({})),
        Arc::new(wrong),
    )
    .await
    .expect_err("a wrong password is refused");
    assert_eq!(err, ErrorClass::Auth, "a refused login is Auth");

    let mut no_ca = target.config(&[&topic], json!({}));
    no_ca["tls"].as_object_mut().expect("tls").remove("ca_pem");
    discover_with(&target, no_ca, target.secrets.clone())
        .await
        .expect_err("a certificate the system roots do not know is refused");
}
