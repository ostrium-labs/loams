//! A sink that produces events to a Loams stream over the engine's HTTP
//! produce route (design 01 §6.1):
//!
//! `POST {base}/v1/namespaces/{ns}/streams/{stream}/partitions/{p}/records`
//! with `{"records": [{"key", "value", "headers", "timestamp_ms"}]}`, keys,
//! values and header values in base64.
//!
//! Each record's key is the org and its value the CloudEvent in JSON; the
//! partition is `xxh3(org) % partitions`, so one org's events stay in order.
//! Headers carry `ce-id` and the content type.
//!
//! The engine has no idempotent producers yet (D72 lands them in M2). Until
//! then a retried batch may be appended twice, and the aggregator drops the
//! copies by `id`.
// TODO(D72): send a producer id `(node, source)` and the spool offset as the
// sequence once the stream API takes them, so the stream dedupes itself.

use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Value, json};
use xxhash_rust::xxh3::xxh3_64;

use crate::error::MeterError;
use crate::event::MeterEvent;
use crate::sink::Sink;

/// Where the stream is.
#[derive(Debug, Clone)]
pub struct StreamSinkConfig {
    /// The engine's HTTP base URL, e.g. `http://loams:8080`.
    pub base_url: String,
    /// The system namespace. Default `_system`.
    pub namespace: String,
    /// The stream. Default `_meter`.
    pub stream: String,
    /// The stream's partition count.
    pub partitions: u32,
    /// Per-request timeout.
    pub timeout: Duration,
}

impl StreamSinkConfig {
    /// `_system/_meter` at `base_url` with `partitions` partitions.
    pub fn new(base_url: impl Into<String>, partitions: u32) -> Self {
        Self {
            base_url: base_url.into(),
            namespace: "_system".into(),
            stream: "_meter".into(),
            partitions,
            timeout: Duration::from_secs(10),
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

    /// The partition of `org`'s events.
    pub fn partition_of(&self, org: &str) -> u32 {
        (xxh3_64(org.as_bytes()) % u64::from(self.config.partitions)) as u32
    }

    fn url(&self, partition: u32) -> String {
        let c = &self.config;
        format!(
            "{}/v1/namespaces/{}/streams/{}/partitions/{partition}/records",
            c.base_url.trim_end_matches('/'),
            c.namespace,
            c.stream
        )
    }
}

/// The produce route's record for `event`.
pub fn record(event: &MeterEvent) -> Result<Value, MeterError> {
    let value = serde_json::to_vec(event).map_err(|e| MeterError::SinkRejected(e.to_string()))?;
    let window_end_ms = event.data.window_end.unix_timestamp_nanos() / 1_000_000;
    Ok(json!({
        "key": BASE64.encode(event.data.org.as_bytes()),
        "value": BASE64.encode(value),
        "headers": [
            { "key": "ce-id", "value": BASE64.encode(event.id.as_bytes()) },
            { "key": "content-type", "value": BASE64.encode("application/cloudevents+json") },
        ],
        "timestamp_ms": i64::try_from(window_end_ms).unwrap_or(-1),
    }))
}

#[async_trait]
impl Sink for StreamSink {
    async fn send(&mut self, events: &[MeterEvent]) -> Result<(), MeterError> {
        // Group by partition, keeping each partition's order.
        let mut batches: Vec<(u32, Vec<Value>)> = Vec::new();
        for event in events {
            let partition = self.partition_of(&event.data.org);
            let record = record(event)?;
            match batches.iter_mut().find(|(p, _)| *p == partition) {
                Some((_, records)) => records.push(record),
                None => batches.push((partition, vec![record])),
            }
        }
        for (partition, records) in batches {
            let response = self
                .client
                .post(self.url(partition))
                .json(&json!({ "records": records }))
                .send()
                .await
                .map_err(|e| MeterError::SinkUnavailable(e.to_string()))?;
            let status = response.status();
            if status.is_success() {
                continue;
            }
            let body = response.text().await.unwrap_or_default();
            let message = format!("partition {partition}: HTTP {status}: {body}");
            return Err(
                if status.is_server_error() || status.as_u16() == 429 || status.as_u16() == 408 {
                    MeterError::SinkUnavailable(message)
                } else {
                    MeterError::SinkRejected(message)
                },
            );
        }
        Ok(())
    }
}
