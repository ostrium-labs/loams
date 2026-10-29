//! A sink that appends events to `_system/_meter` through the meter's
//! service ingest (D2206):
//!
//! `POST {base}/internal/v1/meter/partitions/{p}/records`
//! with `{"records": [{"key", "value", "headers", "timestamp_ms"}]}` (the
//! engine's produce body), keys, values and header values in base64. Outside
//! HTTP produces to the reserved `_system` namespace are refused, so the
//! agent cannot use the native produce route. On a cluster the base is a
//! gateway node's cluster listener and needs no key; on a single node it is
//! the main port, and the agent signs in with an operator API key
//! ([`StreamSinkConfig::api_key`]). The engine appends each event to its
//! subject's partition whatever `{p}` says; the agent still sends each
//! partition's events together, in order.
//!
//! Each record is the usage event of design §63 §6 ([`crate::usage`]) in
//! D270's binary-mode layout (`loams_cloudevents::record::to_record`): the
//! attributes as `ce_` headers, `content-type`, the JSON data as the value
//! and `subject` as the key. The partition is the stream's default
//! partitioner over that key (Kafka's murmur2), so one subject's events stay
//! in order in the partition its counter owner holds (§63 §11.1), and the
//! House pipe reads the `_ce_*` columns.
//!
//! The engine has no idempotent producers yet (D72 lands them in M2). Until
//! then a retried batch may be appended twice, and the meter's counter
//! owners and the House drop the copies by `source` + `id` (§63 §7.3).
// TODO(D72): send a producer id `(node, source)` and the spool offset as the
// sequence once the stream API takes them, so the stream dedupes itself.

use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Value, json};

use crate::error::MeterError;
use crate::event::MeterEvent;
use crate::sink::Sink;
use crate::usage::{self, DEFAULT_CLUSTER};

/// Where the stream is.
#[derive(Debug, Clone)]
pub struct StreamSinkConfig {
    /// The engine's HTTP base URL, e.g. `http://loams:8080`.
    pub base_url: String,
    /// An operator API key for a single node's main port (`Bearer`); none
    /// on a cluster listener.
    pub api_key: Option<String>,
    /// The stream's partition count.
    pub partitions: u32,
    /// Per-request timeout.
    pub timeout: Duration,
    /// The cluster named in each event's `source`
    /// (`loams://<cluster>/meter-agent/<node>`). Default `local`.
    pub cluster: String,
}

impl StreamSinkConfig {
    /// `_system/_meter` at `base_url` with `partitions` partitions.
    pub fn new(base_url: impl Into<String>, partitions: u32) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: None,
            partitions,
            timeout: Duration::from_secs(10),
            cluster: DEFAULT_CLUSTER.into(),
        }
    }
}

/// Produces meter events to a Loams stream.
#[derive(Debug, Clone)]
pub struct StreamSink {
    client: reqwest::Client,
    config: StreamSinkConfig,
}

impl StreamSink {
    /// A sink for `config`.
    pub fn new(config: StreamSinkConfig) -> Result<Self, MeterError> {
        if config.partitions == 0 {
            return Err(MeterError::Config("partitions must be at least 1".into()));
        }
        let client = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|e| MeterError::Config(e.to_string()))?;
        Ok(Self { client, config })
    }

    /// The partition of the billing subject `subject`'s events.
    pub fn partition_of(&self, subject: &str) -> u32 {
        usage::subject_partition(subject, self.config.partitions)
    }

    fn url(&self, partition: u32) -> String {
        format!(
            "{}/internal/v1/meter/partitions/{partition}/records",
            self.config.base_url.trim_end_matches('/'),
        )
    }
}

/// The produce route's record for `event`, sent from `cluster`, and its
/// partition among `partitions`.
pub fn record(
    event: &MeterEvent,
    cluster: &str,
    partitions: u32,
) -> Result<(u32, Value), MeterError> {
    let ce = usage::to_cloudevent(event, cluster)?;
    let partition = usage::partition(&ce, partitions);
    let record = usage::to_record(&ce);
    let b64 = |raw: &Option<bytes::Bytes>| raw.as_ref().map(|b| BASE64.encode(b));
    let headers: Vec<Value> = record
        .headers
        .iter()
        .map(|(key, value)| json!({ "key": key, "value": b64(value) }))
        .collect();
    Ok((
        partition,
        json!({
            "key": b64(&record.key),
            "value": b64(&record.value),
            "headers": headers,
            "timestamp_ms": record.timestamp_ms,
        }),
    ))
}

#[async_trait]
impl Sink for StreamSink {
    async fn send(&mut self, events: &[MeterEvent]) -> Result<(), MeterError> {
        // Group by partition, keeping each partition's order.
        let mut batches: Vec<(u32, Vec<Value>)> = Vec::new();
        for event in events {
            let (partition, record) = record(event, &self.config.cluster, self.config.partitions)?;
            match batches.iter_mut().find(|(p, _)| *p == partition) {
                Some((_, records)) => records.push(record),
                None => batches.push((partition, vec![record])),
            }
        }
        // One produce per partition, sent concurrently over the client's
        // connection pool: sequential POSTs made a batch's latency the sum of
        // up to `partitions` round trips (BQ1-meter). A failure fails the
        // whole send and the forwarder retries every event, as before; the
        // consumer dedupes on `id`.
        let sends = batches.into_iter().map(|(partition, records)| {
            let mut request = self.client.post(self.url(partition));
            if let Some(key) = &self.config.api_key {
                request = request.bearer_auth(key);
            }
            let request = request.json(&json!({ "records": records }));
            async move {
                let response = request
                    .send()
                    .await
                    .map_err(|e| MeterError::SinkUnavailable(e.to_string()))?;
                let status = response.status();
                if status.is_success() {
                    return Ok(());
                }
                let body = response.text().await.unwrap_or_default();
                let message = format!("partition {partition}: HTTP {status}: {body}");
                Err(
                    if status.is_server_error() || status.as_u16() == 429 || status.as_u16() == 408
                    {
                        MeterError::SinkUnavailable(message)
                    } else {
                        MeterError::SinkRejected(message)
                    },
                )
            }
        });
        for result in futures::future::join_all(sends).await {
            result?;
        }
        Ok(())
    }
}
