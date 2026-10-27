//! The Kafka source (plan Task 9, CN1 Task 6).
//!
//! * **Splits are topic-partitions** (`<topic>/<partition>`). The enumerator
//!   reads the cluster's metadata for the configured topics on every
//!   discovery, so a partition added to a topic becomes a new split without a
//!   restart (`partition_added_is_discovered`). A topic that does not exist
//!   yet has no splits until it does.
//! * **One consumer per split**, assigned (not subscribed) to its partition,
//!   so the SDK, not the group protocol, decides who reads what (OR-16). The
//!   consumer reads `read_committed`, so records of an aborted transaction are
//!   never ingested.
//! * **Position** = the next offset to read (big-endian `u64`; display
//!   `<topic>/<partition>@<offset>`). A split opened at a checkpoint resumes
//!   there, with librdkafka's offset reset set to `error`: an offset the
//!   broker no longer has is `PositionLost`, never a silent skip. A split
//!   without a checkpoint starts at the group's committed offset, else at
//!   `auto_offset_reset`.
//! * **Commit after the Fabric acknowledges** (CN1 Ruling 4): `committed`
//!   commits the consumer group's offset for the partition, synchronously.
//! * **Backpressure**: `next` returns at most the budget and holds at most
//!   one record back (the one that did not fit), so the consumer is never
//!   drained ahead of the pipeline.
//! * **Debezium** (`format: debezium-json | debezium-avro`, CC1 Task 3):
//!   each record is decoded by [`DebeziumDecoder`]. A record that is not a
//!   change event is `Poison` (the pipeline dead-letters it and the source
//!   moves past it); a record whose decoding failed for another reason (the
//!   registry was unreachable) is kept and decoded again on the next call;
//!   a dropped tombstone only moves the position.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use loams_connector_sdk::{
    Budget, CloudEvent, ConnectorError, ErrorClass, EventBatch, Position, Source, SourceBatch,
    SourceFactory, Split, SplitEnumerator, SplitState,
};
use rdkafka::consumer::{BaseConsumer, CommitMode, Consumer, StreamConsumer};
use rdkafka::error::RDKafkaErrorCode;
use rdkafka::{Message, Offset, TopicPartitionList};
use serde_json::json;

use crate::config::{CALL_TIMEOUT, Connection, OffsetReset, RecordFormat};
use crate::debezium::{DebeziumDecoder, RawRecord};
use crate::error::{AuthWatch, class_of, class_of_code, join_error, kafka_error};
use crate::record::event_from_message;

/// A split's id: `<topic>/<partition>`.
pub fn split_id(topic: &str, partition: i32) -> String {
    format!("{topic}/{partition}")
}

fn split_parts(split: &Split) -> Result<(String, i32), ConnectorError> {
    let topic = split.spec.get("topic").and_then(|v| v.as_str());
    let partition = split.spec.get("partition").and_then(|v| v.as_i64());
    match (topic, partition) {
        (Some(t), Some(p)) if p >= 0 && p <= i64::from(i32::MAX) => Ok((t.to_owned(), p as i32)),
        _ => Err(ConnectorError::fatal(format!(
            "split {} is not a Kafka topic-partition",
            split.id
        ))),
    }
}

/// The position just before `offset`'s record.
pub fn position(topic: &str, partition: i32, next_offset: i64) -> Position {
    let n = next_offset.max(0) as u64;
    Position::new(n.to_be_bytes().to_vec(), format!("{topic}/{partition}@{n}"))
}

fn offset_of(pos: &Position) -> Result<i64, ConnectorError> {
    pos.as_u64()
        .and_then(|n| i64::try_from(n).ok())
        .ok_or_else(|| ConnectorError::fatal(format!("{pos} is not a Kafka offset")))
}

/// Finds the topic-partitions of the configured topics.
pub struct KafkaEnumerator {
    conn: Arc<Connection>,
    client: Option<Arc<BaseConsumer<AuthWatch>>>,
}

impl std::fmt::Debug for KafkaEnumerator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KafkaEnumerator")
            .field("topics", &self.conn.cfg.topics)
            .finish_non_exhaustive()
    }
}

impl KafkaEnumerator {
    pub fn new(conn: Arc<Connection>) -> Self {
        Self { conn, client: None }
    }

    async fn client(&mut self) -> Result<Arc<BaseConsumer<AuthWatch>>, ConnectorError> {
        if let Some(c) = &self.client {
            return Ok(c.clone());
        }
        let cfg = self.conn.base().await?;
        let c: BaseConsumer<AuthWatch> = cfg
            .create_with_context(AuthWatch::default())
            .map_err(|e| kafka_error("creating the metadata client", &e))?;
        let c = Arc::new(c);
        self.client = Some(c.clone());
        Ok(c)
    }
}

#[async_trait]
impl SplitEnumerator for KafkaEnumerator {
    async fn discover(&mut self, _known: &[SplitState]) -> Result<Vec<Split>, ConnectorError> {
        let client = self.client().await?;
        let watch = client.clone();
        let topics = self.conn.cfg.topics.clone();
        let (found, brokers) = tokio::task::spawn_blocking(move || {
            let mut found = Vec::new();
            let mut brokers = Vec::new();
            for topic in &topics {
                let md = match client.fetch_metadata(Some(topic), CALL_TIMEOUT) {
                    Ok(md) => md,
                    Err(e) => {
                        let what = format!("metadata for {topic}");
                        return Err(match refused_login(&client) {
                            Some(reason) => {
                                ConnectorError::new(ErrorClass::Auth, format!("{what}: {reason}"))
                            }
                            None => kafka_error(&what, &e),
                        });
                    }
                };
                for b in md.brokers() {
                    brokers.push((b.host().to_owned(), b.port()));
                }
                for t in md.topics() {
                    if let Some(err) = t.error() {
                        let code = RDKafkaErrorCode::from(err);
                        if matches!(
                            code,
                            RDKafkaErrorCode::UnknownTopicOrPartition | RDKafkaErrorCode::UnknownTopic
                        ) {
                            tracing::warn!(topic = %t.name(), "topic does not exist yet; no splits");
                            continue;
                        }
                        return Err(ConnectorError::new(
                            class_of_code(code),
                            format!("metadata for {}: {code}", t.name()),
                        ));
                    }
                    for p in t.partitions() {
                        found.push((t.name().to_owned(), p.id()));
                    }
                }
            }
            Ok((found, brokers))
        })
        .await
        .map_err(|e| join_error("metadata", e))?
        .map_err(|e| watch.context().classify(e))?;

        // librdkafka dials the brokers the metadata names: each one passes the
        // egress check too.
        let mut seen = std::collections::BTreeSet::new();
        for (host, port) in brokers {
            if seen.insert((host.clone(), port)) {
                let port = u16::try_from(port)
                    .map_err(|_| ConnectorError::fatal(format!("broker {host} has port {port}")))?;
                self.conn.check_broker(&host, port).await?;
            }
        }

        let mut splits: Vec<Split> = found
            .into_iter()
            .map(|(topic, partition)| {
                Split::new(
                    split_id(&topic, partition),
                    json!({ "topic": topic, "partition": partition }),
                )
            })
            .collect();
        splits.sort_by(|a, b| a.id.cmp(&b.id));
        splits.dedup_by(|a, b| a.id == b.id);
        Ok(splits)
    }
}

/// The reason librdkafka refused this client's login, if it did. librdkafka
/// reports a refused SASL login as an event on the client's queue, not as
/// the failed call's error (that is a timeout or a transport failure): serve
/// the queued events so [`AuthWatch`] sees it, or take it from the event.
fn refused_login(client: &BaseConsumer<AuthWatch>) -> Option<String> {
    // `poll` also answers `None` for an event it served itself (a log
    // line), so a `None` does not mean the queue is empty: poll a bounded
    // number of times.
    for _ in 0..256 {
        if let Some(Err(e)) = client.poll(Duration::ZERO)
            && class_of(&e) == ErrorClass::Auth
        {
            return Some(e.to_string());
        }
        if let Some(reason) = client.context().refused() {
            return Some(reason);
        }
    }
    client.context().refused()
}

/// Makes one [`KafkaSource`] per split.
#[derive(Debug)]
pub struct KafkaSourceFactory {
    conn: Arc<Connection>,
}

impl KafkaSourceFactory {
    pub fn new(conn: Arc<Connection>) -> Self {
        Self { conn }
    }
}

impl SourceFactory for KafkaSourceFactory {
    fn create(&self, split: &Split) -> Result<Box<dyn Source>, ConnectorError> {
        split_parts(split)?;
        Ok(Box::new(KafkaSource::new(self.conn.clone())))
    }
}

/// Reads one topic-partition.
pub struct KafkaSource {
    conn: Arc<Connection>,
    topic: String,
    partition: i32,
    consumer: Option<Arc<StreamConsumer>>,
    /// The offset after the last record handed out.
    next_offset: Option<i64>,
    /// The one record that did not fit the last budget, and its offset.
    pending: Option<(CloudEvent, i64)>,
    /// A consumer error that arrived after some records; reported next.
    deferred: Option<ConnectorError>,
    /// The Debezium decoder, with `format: debezium-*`.
    decoder: Option<Arc<DebeziumDecoder>>,
    /// A record to decode (again) before reading on: its decoding failed
    /// after some records were taken, or with a transient error.
    pending_raw: Option<RawRecord>,
}

/// What one consumed record became.
enum Read {
    Event(CloudEvent, i64),
    Raw(RawRecord),
}

impl std::fmt::Debug for KafkaSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KafkaSource")
            .field("topic", &self.topic)
            .field("partition", &self.partition)
            .field("next_offset", &self.next_offset)
            .finish_non_exhaustive()
    }
}

impl KafkaSource {
    pub fn new(conn: Arc<Connection>) -> Self {
        Self {
            conn,
            topic: String::new(),
            partition: 0,
            consumer: None,
            next_offset: None,
            pending: None,
            deferred: None,
            decoder: None,
            pending_raw: None,
        }
    }

    fn consumer(&self) -> Result<Arc<StreamConsumer>, ConnectorError> {
        self.consumer
            .clone()
            .ok_or_else(|| ConnectorError::fatal("the Kafka source is not open"))
    }

    /// R4.7: a record larger than a whole batch can never be read; it is
    /// `Poison` (a DLQ record with the split), and the source moves past it.
    fn too_large(&self, offset: i64, size: usize, budget: Budget) -> ConnectorError {
        ConnectorError::poison(format!(
            "{}/{}@{offset}: a record of {size} bytes is larger than a batch ({} bytes)",
            self.topic, self.partition, budget.max_bytes
        ))
    }

    fn position(&self) -> Position {
        position(&self.topic, self.partition, self.next_offset.unwrap_or(0))
    }
}

/// Where a split without a checkpoint starts: the group's committed offset,
/// read with an explicit `OffsetFetch`, else `auto_offset_reset`.
///
/// librdkafka's own path (`Offset::Stored` on an assigned partition) asks the
/// group coordinator once. On a cluster whose `__consumer_offsets` does not
/// exist yet (the first consumer group it ever sees, as on a fresh broker)
/// that request fails while the topic is created, and the assignment then
/// never starts fetching: the split reads nothing, with no error. Here a
/// transient failure is retried until [`CALL_TIMEOUT`], then reported.
async fn group_start(
    consumer: Arc<StreamConsumer>,
    topic: &str,
    partition: i32,
    reset: OffsetReset,
) -> Result<Offset, ConnectorError> {
    let mut tpl = TopicPartitionList::new();
    tpl.add_partition(topic, partition);
    let deadline = tokio::time::Instant::now() + CALL_TIMEOUT;
    loop {
        let (c, t) = (consumer.clone(), tpl.clone());
        let got = tokio::task::spawn_blocking(move || c.committed_offsets(t, OFFSET_FETCH_TIMEOUT))
            .await
            .map_err(|e| join_error("reading the committed offset", e))?;
        match got {
            Ok(list) => {
                let committed = list
                    .find_partition(topic, partition)
                    .map_or(Offset::Invalid, |e| e.offset());
                return Ok(match (committed, reset) {
                    (Offset::Offset(n), _) if n >= 0 => Offset::Offset(n),
                    (_, OffsetReset::Earliest) => Offset::Beginning,
                    (_, OffsetReset::Latest) => Offset::End,
                    // Nothing committed and no reset: librdkafka reports
                    // `_AUTO_OFFSET_RESET`, which `next` maps to PositionLost.
                    (_, OffsetReset::None) => Offset::Stored,
                });
            }
            Err(e) => {
                let err = kafka_error("reading the consumer group's committed offset", &e);
                if !err.class.is_transient() || tokio::time::Instant::now() >= deadline {
                    return Err(err);
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
}

/// One `OffsetFetch` attempt of [`group_start`].
const OFFSET_FETCH_TIMEOUT: Duration = Duration::from_secs(5);

#[async_trait]
impl Source for KafkaSource {
    async fn open(&mut self, split: &Split, pos: Option<Position>) -> Result<(), ConnectorError> {
        let (topic, partition) = split_parts(split)?;
        let resume = pos.as_ref().map(offset_of).transpose()?;
        let mut cfg = self.conn.base().await?;
        cfg.set("group.id", self.conn.cfg.group(&self.conn.instance))
            .set("enable.auto.commit", "false")
            .set("enable.auto.offset.store", "false")
            .set("enable.partition.eof", "false")
            .set("isolation.level", "read_committed")
            .set(
                "auto.offset.reset",
                // A resumed split must not jump: a lost offset is an error.
                if resume.is_some() {
                    "error"
                } else {
                    self.conn.cfg.auto_offset_reset.as_str()
                },
            );
        let consumer: Arc<StreamConsumer> = Arc::new(
            cfg.create()
                .map_err(|e| kafka_error("creating the consumer", &e))?,
        );
        let start = match resume {
            Some(o) => Offset::Offset(o),
            None => {
                group_start(
                    consumer.clone(),
                    &topic,
                    partition,
                    self.conn.cfg.auto_offset_reset,
                )
                .await?
            }
        };
        let mut tpl = TopicPartitionList::new();
        tpl.add_partition_offset(&topic, partition, start)
            .map_err(|e| kafka_error("assigning the partition", &e))?;
        consumer
            .assign(&tpl)
            .map_err(|e| kafka_error("assigning the partition", &e))?;
        self.topic = topic;
        self.partition = partition;
        self.next_offset = resume;
        self.pending = None;
        self.deferred = None;
        self.pending_raw = None;
        self.decoder = match self.conn.cfg.format {
            RecordFormat::Plain => None,
            format => {
                let registry = self.conn.registry().await?.map(|r| {
                    Arc::new(r) as Arc<dyn loams_connector_sdk::schema::registry::SchemaRegistry>
                });
                Some(Arc::new(DebeziumDecoder::new(
                    self.conn.envelope.clone(),
                    format,
                    self.conn.cfg.debezium.clone(),
                    registry,
                )?))
            }
        };
        self.consumer = Some(consumer);
        Ok(())
    }

    async fn next(&mut self, budget: Budget) -> Result<Option<SourceBatch>, ConnectorError> {
        if let Some(e) = self.deferred.take() {
            return Err(e);
        }
        let consumer = self.consumer()?;
        let max_rows = self
            .conn
            .cfg
            .max_poll_records
            .map_or(budget.max_rows, |m| m.min(budget.max_rows))
            .max(1);
        let mut events: Vec<CloudEvent> = Vec::new();
        let mut bytes = 0usize;

        if let Some((ev, offset)) = self.pending.take() {
            let size = ev.size_bytes();
            if !budget.admits(1, size) {
                self.next_offset = Some(offset + 1);
                return Err(self.too_large(offset, size, budget));
            }
            bytes += size;
            events.push(ev);
            self.next_offset = Some(offset + 1);
        }

        let wait = Duration::from_millis(self.conn.cfg.poll_wait_ms);
        while events.len() < max_rows {
            let read = match self.pending_raw.take() {
                Some(raw) => Read::Raw(raw),
                None => {
                    // Wait for the first record; after that take only what
                    // is ready.
                    let limit = if events.is_empty() {
                        wait
                    } else {
                        Duration::ZERO
                    };
                    let msg = match tokio::time::timeout(limit, consumer.recv()).await {
                        Err(_) => break,
                        Ok(m) => m,
                    };
                    match msg {
                        Ok(m) => match &self.decoder {
                            None => {
                                Read::Event(event_from_message(&self.conn.envelope, &m), m.offset())
                            }
                            Some(_) => Read::Raw(RawRecord::from_message(&m)),
                        },
                        Err(e) => {
                            let err = kafka_error(
                                &format!("reading {}/{}", self.topic, self.partition),
                                &e,
                            );
                            if events.is_empty() && self.pending.is_none() {
                                return Err(err);
                            }
                            self.deferred = Some(err);
                            break;
                        }
                    }
                }
            };
            let (ev, offset) = match read {
                Read::Event(ev, offset) => (ev, offset),
                Read::Raw(raw) => {
                    let Some(decoder) = self.decoder.clone() else {
                        return Err(ConnectorError::fatal("a raw record without a decoder"));
                    };
                    match decoder.decode(&raw).await {
                        Ok(Some(ev)) => (ev, raw.offset),
                        // A dropped tombstone: only the position moves.
                        Ok(None) => {
                            self.next_offset = Some(raw.offset + 1);
                            continue;
                        }
                        Err(e) => {
                            if events.is_empty() {
                                if e.class == loams_connector_sdk::ErrorClass::Poison {
                                    // Dead-lettered by the pipeline: move past it.
                                    self.next_offset = Some(raw.offset + 1);
                                } else {
                                    self.pending_raw = Some(raw);
                                }
                                return Err(e);
                            }
                            // Report it on the next call, after this batch.
                            self.pending_raw = Some(raw);
                            break;
                        }
                    }
                }
            };
            let size = ev.size_bytes();
            if !budget.admits(events.len() + 1, bytes + size) {
                if events.is_empty() {
                    self.next_offset = Some(offset + 1);
                    return Err(self.too_large(offset, size, budget));
                }
                self.pending = Some((ev, offset));
                break;
            }
            bytes += size;
            events.push(ev);
            self.next_offset = Some(offset + 1);
        }
        if events.is_empty() {
            return Ok(None);
        }
        Ok(Some(SourceBatch {
            events: EventBatch::Events(events),
            position: self.position(),
        }))
    }

    async fn committed(&mut self, upto: &Position) -> Result<(), ConnectorError> {
        let consumer = self.consumer()?;
        let offset = offset_of(upto)?;
        let mut tpl = TopicPartitionList::new();
        tpl.add_partition_offset(&self.topic, self.partition, Offset::Offset(offset))
            .map_err(|e| kafka_error("committing the offset", &e))?;
        tokio::task::spawn_blocking(move || consumer.commit(&tpl, CommitMode::Sync))
            .await
            .map_err(|e| join_error("commit", e))?
            .map_err(|e| kafka_error("committing the consumer group offset", &e))
    }

    async fn close(&mut self) -> Result<(), ConnectorError> {
        if let Some(c) = self.consumer.take() {
            let _ = tokio::task::spawn_blocking(move || {
                let _ = c.unassign();
                drop(c);
            })
            .await;
        }
        self.pending = None;
        self.pending_raw = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_round_trip() {
        let p = position("orders", 3, 42);
        assert_eq!(p.display, "orders/3@42");
        assert_eq!(offset_of(&p).expect("offset"), 42);
        let split = Split::new(
            split_id("orders", 3),
            json!({"topic": "orders", "partition": 3}),
        );
        assert_eq!(split.id.as_str(), "orders/3");
        assert_eq!(
            split_parts(&split).expect("parts"),
            ("orders".to_owned(), 3)
        );
        assert!(split_parts(&Split::new("x", json!({}))).is_err());
    }
}
