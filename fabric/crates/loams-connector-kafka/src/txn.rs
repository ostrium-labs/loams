//! The transactional Kafka sink (`exactly_once`, OR-7, plan Task 9).
//!
//! * `transactional.id = <prefix><instance>-<split>` (`prefix` is the
//!   config's `transactional_id_prefix`, default `loams-<ns>-`), where the split is
//!   the SDK's Fabric split. `recover()` calls `init_transactions`, which
//!   fences any older producer with the same id (a zombie after a crash or a
//!   reschedule) and aborts the transaction it left open, then reads the
//!   checkpoint back.
//! * Per batch: `write` begins a transaction (if none is open) and produces
//!   the batch's records; `prepare(cp)` produces the checkpoint to the
//!   compacted topic `__loams_checkpoints` (key `<ns>/<instance>/<split>`)
//!   **inside the same transaction**; `commit` commits it. The data and the
//!   checkpoint become visible together or not at all.
//! * `recover()` reads the checkpoint topic with
//!   `isolation.level=read_committed`, so a checkpoint whose transaction was
//!   aborted (or never committed) is never seen; the last committed one wins,
//!   and the pipeline seeks the Fabric reader there, skipping every batch
//!   Kafka already holds.
//!
//! The checkpoint record goes to one partition chosen from its key (FNV-1a,
//! so every run picks the same one), and `recover` reads only that partition,
//! up to the high watermark it saw after `init_transactions`.
//!
//! Event Hubs has no Kafka transactions on its emulator and only a preview
//! in the cloud (R0.19): its profile narrows `sink.transactional`, and the
//! factory refuses `delivery: exactly_once` under it.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use bytes::Bytes;
use loams_connector_sdk::pipeline::FABRIC_SPLIT;
use loams_connector_sdk::{
    Checkpoint, ConnectorError, EventBatch, InstanceKey, Position, Sink, SinkAck, SplitId,
    TransactionalSink, TxnToken,
};
use rdkafka::admin::{AdminClient, AdminOptions, NewTopic, TopicReplication};
use rdkafka::client::DefaultClientContext;
use rdkafka::consumer::{BaseConsumer, Consumer};
use rdkafka::error::{KafkaError, RDKafkaErrorCode};
use rdkafka::producer::{FutureProducer, Producer};
use rdkafka::{Message, Offset, TopicPartitionList};
use serde::{Deserialize, Serialize};

use crate::config::{CALL_TIMEOUT, Connection};
use crate::error::{join_error, kafka_error};
use crate::record::OutRecord;
use crate::sink::{producer_config, records, send_all};

/// `<prefix><instance>-<split>`, the prefix defaulting to `loams-<ns>-`.
pub fn transactional_id(instance: &InstanceKey, prefix: Option<&str>, split: &str) -> String {
    match prefix {
        Some(p) => format!("{p}{}-{split}", instance.name),
        None => format!("loams-{}-{}-{split}", instance.namespace, instance.name),
    }
}

/// The checkpoint record's key: `<ns>/<instance>/<split>`.
pub fn checkpoint_key(instance: &InstanceKey, split: &str) -> String {
    format!("{}/{}/{split}", instance.namespace, instance.name)
}

/// The partition of the checkpoint topic a key lives on (FNV-1a 32).
pub fn checkpoint_partition(key: &str, partitions: i32) -> i32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in key.as_bytes() {
        h ^= u32::from(*b);
        h = h.wrapping_mul(0x0100_0193);
    }
    (h % partitions.max(1) as u32) as i32
}

/// The checkpoint as stored: the lease epoch, each split's position, and the
/// transaction it was committed with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredCheckpoint {
    pub namespace: u64,
    pub instance: String,
    pub epoch: u64,
    pub txn: u64,
    pub splits: Vec<StoredPosition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredPosition {
    pub split: String,
    /// The position's opaque bytes, base64.
    pub opaque: String,
    pub display: String,
}

impl StoredCheckpoint {
    pub fn from_checkpoint(cp: &Checkpoint, txn: u64) -> Self {
        Self {
            namespace: cp.instance.namespace,
            instance: cp.instance.name.clone(),
            epoch: cp.epoch,
            txn,
            splits: cp
                .splits
                .iter()
                .map(|(id, pos)| StoredPosition {
                    split: id.as_str().to_owned(),
                    opaque: base64::engine::general_purpose::STANDARD.encode(&pos.opaque),
                    display: pos.display.clone(),
                })
                .collect(),
        }
    }

    pub fn to_checkpoint(&self) -> Result<Checkpoint, ConnectorError> {
        let mut cp = Checkpoint::new(
            InstanceKey::new(self.namespace, self.instance.clone()),
            self.epoch,
        );
        for s in &self.splits {
            let opaque = base64::engine::general_purpose::STANDARD
                .decode(&s.opaque)
                .map_err(|e| {
                    ConnectorError::fatal(format!("checkpoint of split {}: {e}", s.split))
                })?;
            cp.splits.insert(
                SplitId::new(s.split.clone()),
                Position::new(opaque, s.display.clone()),
            );
        }
        Ok(cp)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TxnState {
    /// No transaction open.
    Idle,
    /// Records of transaction `n` are being produced.
    Open(u64),
    /// Transaction `n` holds its checkpoint and waits for `commit`.
    Prepared(u64),
}

/// The exactly-once sink.
pub struct TransactionalKafkaSink {
    conn: Arc<Connection>,
    txn_id: String,
    key: String,
    producer: Option<Arc<FutureProducer>>,
    /// The checkpoint topic's partition count, once known.
    partitions: Option<i32>,
    state: TxnState,
    /// The number of the last transaction begun.
    seq: u64,
    /// The number of the last transaction committed.
    committed: u64,
}

impl std::fmt::Debug for TransactionalKafkaSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransactionalKafkaSink")
            .field("transactional_id", &self.txn_id)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

fn token_seq(t: &TxnToken) -> Result<u64, ConnectorError> {
    let b: [u8; 8] =
        t.opaque.as_ref().try_into().map_err(|_| {
            ConnectorError::fatal(format!("{} is not a Kafka transaction token", t.id))
        })?;
    Ok(u64::from_be_bytes(b))
}

impl TransactionalKafkaSink {
    pub fn new(conn: Arc<Connection>) -> Self {
        let txn_id = transactional_id(
            &conn.instance,
            conn.cfg.transactional_id_prefix.as_deref(),
            FABRIC_SPLIT,
        );
        let key = checkpoint_key(&conn.instance, FABRIC_SPLIT);
        Self {
            conn,
            txn_id,
            key,
            producer: None,
            partitions: None,
            state: TxnState::Idle,
            seq: 0,
            committed: 0,
        }
    }

    pub fn transactional_id(&self) -> &str {
        &self.txn_id
    }

    /// The producer, created and initialised (`init_transactions`) once.
    async fn producer(&mut self) -> Result<Arc<FutureProducer>, ConnectorError> {
        if let Some(p) = &self.producer {
            return Ok(p.clone());
        }
        let mut cfg = producer_config(&self.conn).await?;
        cfg.set("transactional.id", &self.txn_id).set(
            "transaction.timeout.ms",
            self.conn.cfg.transaction_timeout_ms.to_string(),
        );
        let p: FutureProducer = cfg
            .create()
            .map_err(|e| kafka_error("creating the transactional producer", &e))?;
        let p = Arc::new(p);
        let init = p.clone();
        tokio::task::spawn_blocking(move || init.init_transactions(CALL_TIMEOUT))
            .await
            .map_err(|e| join_error("init_transactions", e))?
            .map_err(|e| kafka_error("init_transactions", &e))?;
        self.producer = Some(p.clone());
        self.state = TxnState::Idle;
        Ok(p)
    }

    /// Drop the producer after an error that leaves it unusable; the next
    /// call starts over with `init_transactions`.
    fn reset(&mut self) {
        self.producer = None;
        self.state = TxnState::Idle;
    }

    async fn begin(&mut self) -> Result<Arc<FutureProducer>, ConnectorError> {
        let p = self.producer().await?;
        if self.state == TxnState::Idle {
            p.begin_transaction()
                .map_err(|e| kafka_error("begin_transaction", &e))?;
            self.seq += 1;
            self.state = TxnState::Open(self.seq);
        }
        Ok(p)
    }

    async fn abort_open(&mut self) {
        if self.state == TxnState::Idle {
            return;
        }
        let Some(p) = self.producer.clone() else {
            self.state = TxnState::Idle;
            return;
        };
        let res = tokio::task::spawn_blocking(move || p.abort_transaction(CALL_TIMEOUT)).await;
        match res {
            Ok(Ok(())) => self.state = TxnState::Idle,
            // A producer that cannot abort is replaced; the broker aborts its
            // transaction when the next `init_transactions` fences it.
            _ => self.reset(),
        }
    }

    /// The checkpoint topic's partition count, creating the topic
    /// (compacted, one partition) if it is missing and allowed to.
    async fn checkpoint_partitions(&mut self) -> Result<i32, ConnectorError> {
        if let Some(n) = self.partitions {
            return Ok(n);
        }
        let topic = self.conn.cfg.checkpoint_topic.clone();
        let base = self.conn.base().await?;
        let client: BaseConsumer = base
            .create()
            .map_err(|e| kafka_error("creating the checkpoint reader", &e))?;
        let client = Arc::new(client);
        let n = partitions_of(client.clone(), &topic).await?;
        let n = match n {
            Some(n) => n,
            None if self.conn.cfg.create_checkpoint_topic => {
                let admin: AdminClient<DefaultClientContext> = base
                    .create()
                    .map_err(|e| kafka_error("creating the admin client", &e))?;
                let new = NewTopic::new(&topic, 1, TopicReplication::Fixed(-1))
                    .set("cleanup.policy", "compact");
                let res = admin
                    .create_topics(
                        [&new],
                        &AdminOptions::new().operation_timeout(Some(CALL_TIMEOUT)),
                    )
                    .await
                    .map_err(|e| kafka_error("creating the checkpoint topic", &e))?;
                for r in res {
                    if let Err((t, code)) = r
                        && code != RDKafkaErrorCode::TopicAlreadyExists
                    {
                        return Err(kafka_error(
                            &format!("creating the checkpoint topic {t}"),
                            &KafkaError::AdminOp(code),
                        ));
                    }
                }
                // Metadata may lag the creation briefly.
                let mut n = None;
                for _ in 0..50 {
                    n = partitions_of(client.clone(), &topic).await?;
                    if n.is_some() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                n.ok_or_else(|| {
                    ConnectorError::retryable(format!(
                        "the checkpoint topic {topic} is not visible yet"
                    ))
                })?
            }
            None => {
                return Err(ConnectorError::fatal(format!(
                    "the checkpoint topic {topic} does not exist (create it compacted, or set create_checkpoint_topic)"
                )));
            }
        };
        self.partitions = Some(n);
        Ok(n)
    }

    /// The last committed checkpoint, read `read_committed`.
    async fn read_checkpoint(&mut self) -> Result<Option<StoredCheckpoint>, ConnectorError> {
        let n = self.checkpoint_partitions().await?;
        let topic = self.conn.cfg.checkpoint_topic.clone();
        let partition = checkpoint_partition(&self.key, n);
        let mut cfg = self.conn.base().await?;
        cfg.set("isolation.level", "read_committed")
            .set("enable.auto.commit", "false")
            .set("enable.partition.eof", "false")
            .set("group.id", format!("{}-recover", self.txn_id));
        let consumer: BaseConsumer = cfg
            .create()
            .map_err(|e| kafka_error("creating the checkpoint reader", &e))?;
        let key = self.key.clone();
        let deadline = Duration::from_millis(self.conn.cfg.recover_timeout_ms);
        let found = tokio::task::spawn_blocking(move || -> Result<Option<Vec<u8>>, ConnectorError> {
            let (low, high) = consumer
                .fetch_watermarks(&topic, partition, CALL_TIMEOUT)
                .map_err(|e| kafka_error("checkpoint topic watermarks", &e))?;
            if high <= low {
                return Ok(None);
            }
            let mut tpl = TopicPartitionList::new();
            tpl.add_partition_offset(&topic, partition, Offset::Beginning)
                .map_err(|e| kafka_error("reading the checkpoint topic", &e))?;
            consumer
                .assign(&tpl)
                .map_err(|e| kafka_error("reading the checkpoint topic", &e))?;
            let started = std::time::Instant::now();
            let mut last: Option<Vec<u8>> = None;
            loop {
                if let Some(m) = consumer.poll(Duration::from_millis(100)) {
                    let m = m.map_err(|e| kafka_error("reading the checkpoint topic", &e))?;
                    if m.key() == Some(key.as_bytes()) {
                        // A tombstone clears the checkpoint.
                        last = m.payload().map(<[u8]>::to_vec);
                    }
                }
                // Done once the read position passes the high watermark seen
                // at the start (control records and aborted records included).
                let pos = consumer
                    .position()
                    .ok()
                    .and_then(|l| l.find_partition(&topic, partition).map(|e| e.offset()));
                if let Some(Offset::Offset(o)) = pos
                    && o >= high
                {
                    return Ok(last);
                }
                if started.elapsed() > deadline {
                    return Err(ConnectorError::retryable(format!(
                        "reading {topic}/{partition} up to offset {high} took over {} ms (an open transaction holds it?)",
                        deadline.as_millis()
                    )));
                }
            }
        })
        .await
        .map_err(|e| join_error("reading the checkpoint topic", e))??;
        found
            .map(|bytes| {
                serde_json::from_slice::<StoredCheckpoint>(&bytes)
                    .map_err(|e| ConnectorError::fatal(format!("the stored checkpoint: {e}")))
            })
            .transpose()
    }
}

async fn partitions_of(
    client: Arc<BaseConsumer>,
    topic: &str,
) -> Result<Option<i32>, ConnectorError> {
    let topic = topic.to_owned();
    tokio::task::spawn_blocking(move || {
        let md = client
            .fetch_metadata(Some(&topic), CALL_TIMEOUT)
            .map_err(|e| kafka_error("checkpoint topic metadata", &e))?;
        Ok(md.topics().first().and_then(|t| {
            if t.error().is_some() || t.partitions().is_empty() {
                None
            } else {
                Some(t.partitions().len() as i32)
            }
        }))
    })
    .await
    .map_err(|e| join_error("checkpoint topic metadata", e))?
}

#[async_trait]
impl Sink for TransactionalKafkaSink {
    async fn write(&mut self, batch: EventBatch) -> Result<SinkAck, ConnectorError> {
        // Refuse a poison batch before the transaction sees any of it, so
        // isolating its events one by one never aborts good ones.
        let records = records(&self.conn, &batch)?;
        let p = self.begin().await?;
        let topic = self.conn.cfg.sink_topic()?.to_owned();
        match send_all(&p, &topic, &records, None).await {
            Ok(ack) => Ok(ack),
            Err(e) => {
                // The transaction may hold part of the batch: abort it, so a
                // retry starts clean.
                self.abort_open().await;
                Err(e)
            }
        }
    }

    async fn flush(&mut self) -> Result<SinkAck, ConnectorError> {
        Ok(SinkAck::default())
    }
}

#[async_trait]
impl TransactionalSink for TransactionalKafkaSink {
    async fn recover(&mut self) -> Result<Option<Checkpoint>, ConnectorError> {
        // `init_transactions` fences older producers and aborts what they
        // left open, before the checkpoint is read.
        self.reset();
        self.producer().await?;
        let stored = self.read_checkpoint().await?;
        match stored {
            Some(s) => {
                if s.namespace != self.conn.instance.namespace
                    || s.instance != self.conn.instance.name
                {
                    return Err(ConnectorError::fatal(format!(
                        "the checkpoint under {} belongs to {}/{}",
                        self.key, s.namespace, s.instance
                    )));
                }
                Ok(Some(s.to_checkpoint()?))
            }
            None => Ok(None),
        }
    }

    async fn prepare(&mut self, cp: &Checkpoint) -> Result<TxnToken, ConnectorError> {
        let n = self.checkpoint_partitions().await?;
        let p = self.begin().await?;
        let TxnState::Open(seq) = self.state else {
            return Err(ConnectorError::fatal("prepare without an open transaction"));
        };
        let value = serde_json::to_vec(&StoredCheckpoint::from_checkpoint(cp, seq))
            .map_err(|e| ConnectorError::fatal(format!("encoding the checkpoint: {e}")))?;
        let record = OutRecord {
            key: Some(self.key.clone().into_bytes()),
            payload: Bytes::from(value),
            headers: Vec::new(),
        };
        let topic = self.conn.cfg.checkpoint_topic.clone();
        let partition = checkpoint_partition(&self.key, n);
        if let Err(e) = send_all(&p, &topic, std::slice::from_ref(&record), Some(partition)).await {
            self.abort_open().await;
            return Err(e);
        }
        self.state = TxnState::Prepared(seq);
        Ok(TxnToken {
            id: self.txn_id.clone(),
            opaque: Bytes::copy_from_slice(&seq.to_be_bytes()),
        })
    }

    async fn commit(&mut self, t: TxnToken) -> Result<(), ConnectorError> {
        let seq = token_seq(&t)?;
        match self.state {
            TxnState::Prepared(s) if s == seq => {}
            _ if seq <= self.committed => return Ok(()),
            _ => {
                return Err(ConnectorError::fatal(format!(
                    "transaction {seq} of {} is not prepared",
                    self.txn_id
                )));
            }
        }
        let p = self
            .producer
            .clone()
            .ok_or_else(|| ConnectorError::fatal("no transactional producer"))?;
        let res = tokio::task::spawn_blocking(move || p.commit_transaction(CALL_TIMEOUT))
            .await
            .map_err(|e| join_error("commit_transaction", e))?;
        match res {
            Ok(()) => {
                self.state = TxnState::Idle;
                self.committed = seq;
                Ok(())
            }
            Err(KafkaError::Transaction(rd)) if rd.txn_requires_abort() => {
                self.abort_open().await;
                Err(ConnectorError::fatal(format!(
                    "transaction {seq} of {} was aborted ({}); the batch is written again after recovery",
                    self.txn_id,
                    rd.code()
                )))
            }
            Err(e) => {
                let err = kafka_error("commit_transaction", &e);
                if !err.class.is_transient() {
                    self.reset();
                }
                // A retriable failure leaves the transaction prepared, and the
                // pipeline calls `commit` again.
                Err(err)
            }
        }
    }

    async fn abort(&mut self, t: TxnToken) -> Result<(), ConnectorError> {
        let seq = token_seq(&t)?;
        match self.state {
            TxnState::Open(s) | TxnState::Prepared(s) if s == seq => {
                self.abort_open().await;
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        let i = InstanceKey::new(4, "orders-out");
        assert_eq!(
            transactional_id(&i, None, FABRIC_SPLIT),
            "loams-4-orders-out-fabric"
        );
        assert_eq!(
            transactional_id(&i, Some("loams-prod-"), FABRIC_SPLIT),
            "loams-prod-orders-out-fabric",
            "under the Confluent module's PREFIXED ACL"
        );
        assert_eq!(checkpoint_key(&i, FABRIC_SPLIT), "4/orders-out/fabric");
        let p = checkpoint_partition("4/orders-out/fabric", 7);
        assert!((0..7).contains(&p));
        assert_eq!(p, checkpoint_partition("4/orders-out/fabric", 7));
        assert_eq!(checkpoint_partition("anything", 1), 0);
        assert_eq!(checkpoint_partition("anything", 0), 0);
    }

    #[test]
    fn checkpoint_round_trips() {
        let mut cp = Checkpoint::new(InstanceKey::new(4, "orders-out"), 9);
        cp.splits.insert(
            SplitId::new(FABRIC_SPLIT),
            Position::new(vec![0, 1, 2, 255], "fabric@3"),
        );
        let stored = StoredCheckpoint::from_checkpoint(&cp, 12);
        let json = serde_json::to_vec(&stored).expect("encode");
        let back: StoredCheckpoint = serde_json::from_slice(&json).expect("decode");
        assert_eq!(back.txn, 12);
        assert_eq!(back.to_checkpoint().expect("checkpoint"), cp);
    }

    #[test]
    fn tokens() {
        let t = TxnToken {
            id: "x".to_owned(),
            opaque: Bytes::copy_from_slice(&5u64.to_be_bytes()),
        };
        assert_eq!(token_seq(&t).expect("seq"), 5);
        assert!(
            token_seq(&TxnToken {
                id: "x".to_owned(),
                opaque: Bytes::new()
            })
            .is_err()
        );
    }
}
