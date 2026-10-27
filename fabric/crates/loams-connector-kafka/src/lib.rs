//! `loams-connector-kafka`: the Kafka-protocol connector family (CN2 Task 9,
//! OR-6), on `loams-connector-sdk`.
//!
//! One implementation serves three manifests (OR-2): Apache Kafka (★), and
//! as target profiles Redpanda (`connectors/profiles/redpanda.yaml`) and
//! Azure Event Hubs' Kafka endpoint (`connectors/profiles/azure-event-hubs.yaml`,
//! OR-22). Each target's suite runs against that target itself.
//!
//! * [`kafka::Kafka`] — the [`loams_connector_sdk::ConnectorFactory`]
//!   (`runtime.ref: loams_connector_kafka::kafka`).
//! * [`source`] — topic-partition splits, the enumerator following partition
//!   additions, consumer-group offsets committed after the Fabric
//!   acknowledges (`at_least_once` into `ingest`, whose dedup by the stable
//!   `<topic>/<partition>/<offset>` ids makes it effectively once).
//! * [`sink`] — the idempotent producer (`effectively_once`), and
//!   [`txn`] — Kafka transactions with the checkpoint in
//!   `__loams_checkpoints` (`exactly_once`).
//! * [`record`] — CloudEvents in Kafka records (D270's binary mode, with
//!   structured-mode and plain records read too).
//! * [`registry`] — a Confluent Schema Registry client (R5.4).
//! * [`debezium`] — Debezium change events (JSON or Avro) as Loams `change`
//!   events, with `format: debezium-json | debezium-avro` (CC1 Task 3).

pub mod config;
pub mod debezium;
pub mod error;
pub mod kafka;
pub mod record;
pub mod registry;
pub mod sink;
pub mod source;
pub mod txn;

pub use config::{CHECKPOINT_TOPIC, KafkaConfig, RecordFormat, SinkDelivery};
pub use debezium::{DebeziumDecoder, RawRecord};
pub use kafka::Kafka;
pub use registry::ConfluentRegistry;
pub use sink::{KafkaSink, KafkaSinkFactory};
pub use source::{KafkaEnumerator, KafkaSource, KafkaSourceFactory};
pub use txn::TransactionalKafkaSink;
