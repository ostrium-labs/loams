//! The meter's gate on the produce path (design §63 §10.2; ME1 Task 22):
//! [`MeteredProducer`] wraps a [`StreamProducer`] and checks
//! `stream_ingest_bytes` for each batch's logical bytes (keys, values and
//! header values) before appending. Every produce path that goes through a
//! `StreamProducer` (HTTP, Flight, the native gRPC service) is covered once
//! the role wraps its producer.
//!
//! Namespaces whose name starts with `_` (`_system`, which holds the meter's
//! own `_meter` stream) are never gated, so metering cannot stop itself.

use std::sync::Arc;

use loams_log::{AppendAck, Record};
use loams_meter_gate::entitle::{Entitlements, features};
use loams_meter_gate::gate::{Refusal, RefusalReason};

use crate::error::ServiceError;
use crate::flight_ingest::StreamProducer;

/// A [`StreamProducer`] that asks the gate first.
#[derive(Debug)]
pub struct MeteredProducer {
    inner: Arc<dyn StreamProducer>,
    entitlements: Entitlements,
}

impl MeteredProducer {
    pub fn new(inner: Arc<dyn StreamProducer>, entitlements: Entitlements) -> Self {
        Self {
            inner,
            entitlements,
        }
    }
}

/// The logical bytes of a batch (D103): keys, values and header values.
pub fn logical_bytes(records: &[(u32, Vec<Record>)]) -> u64 {
    let mut n: u64 = 0;
    for (_, batch) in records {
        for r in batch {
            n = n
                .saturating_add(r.key.as_ref().map_or(0, |b| b.len() as u64))
                .saturating_add(r.value.as_ref().map_or(0, |b| b.len() as u64));
            for (k, v) in &r.headers {
                n = n
                    .saturating_add(k.len() as u64)
                    .saturating_add(v.as_ref().map_or(0, |b| b.len() as u64));
            }
        }
    }
    n
}

/// A refusal on the stream surfaces (§63 §17.4): `resource_exhausted` (429
/// on HTTP, `RESOURCE_EXHAUSTED` on gRPC) naming the entitlement and the
/// reason, or `unavailable` when the meter is.
pub fn refusal_error(r: &Refusal) -> ServiceError {
    let message = format!("entitlement {} is refused: {}", r.feature, r.reason);
    let retry_after_ms = r
        .retry_after
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
    match r.reason {
        RefusalReason::MeterUnavailable => ServiceError::Unavailable(message),
        _ => ServiceError::ResourceExhausted {
            message,
            retry_after_ms,
        },
    }
}

#[async_trait::async_trait]
impl StreamProducer for MeteredProducer {
    async fn partitions(&self, ns: &str, stream: &str) -> Result<u32, ServiceError> {
        self.inner.partitions(ns, stream).await
    }

    async fn produce(
        &self,
        ns: &str,
        stream: &str,
        records: Vec<(u32, Vec<Record>)>,
    ) -> Result<Vec<AppendAck>, ServiceError> {
        if !ns.starts_with('_') {
            let bytes = logical_bytes(&records);
            self.entitlements
                .check(None, ns, features::STREAM_INGEST_BYTES, bytes)
                .map_err(|r| refusal_error(&r))?;
        }
        self.inner.produce(ns, stream, records).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use loams_common::StreamId;
    use loams_meter_gate::adapters::DeploymentOrg;
    use loams_meter_gate::entitle::DenyGate;
    use std::sync::Mutex;

    #[derive(Debug, Default)]
    struct Count(Mutex<u32>);

    #[async_trait::async_trait]
    impl StreamProducer for Count {
        async fn partitions(&self, _ns: &str, _stream: &str) -> Result<u32, ServiceError> {
            Ok(1)
        }

        async fn produce(
            &self,
            _ns: &str,
            _stream: &str,
            records: Vec<(u32, Vec<Record>)>,
        ) -> Result<Vec<AppendAck>, ServiceError> {
            *self.0.lock().unwrap() += 1;
            Ok(records
                .iter()
                .map(|(p, _)| AppendAck {
                    stream: StreamId(1),
                    partition: *p,
                    base_offset: 0,
                    last_offset: 0,
                })
                .collect())
        }
    }

    fn record(v: &'static [u8]) -> Record {
        Record {
            key: Some(Bytes::from_static(b"k")),
            value: Some(Bytes::from_static(v)),
            headers: vec![("h".into(), Some(Bytes::from_static(b"xy")))],
            timestamp_ms: 0,
        }
    }

    #[test]
    fn logical_bytes_counts_keys_values_and_headers() {
        assert_eq!(logical_bytes(&[(0, vec![record(b"abcd")])]), 1 + 4 + 1 + 2);
    }

    #[tokio::test]
    async fn refused_org_cannot_produce_but_system_can() {
        let inner = Arc::new(Count::default());
        let p = MeteredProducer::new(
            inner.clone(),
            Entitlements::new(
                Arc::new(DenyGate::new(
                    "org_42",
                    features::STREAM_INGEST_BYTES,
                    RefusalReason::EntitlementExhausted,
                )),
                Arc::new(DeploymentOrg::new("42")),
            ),
        );
        let err = p
            .produce("7", "s", vec![(0, vec![record(b"a")])])
            .await
            .unwrap_err();
        assert!(
            matches!(err, ServiceError::ResourceExhausted { .. }),
            "{err}"
        );
        assert!(err.to_string().contains("stream_ingest_bytes"));
        assert_eq!(*inner.0.lock().unwrap(), 0);
        p.produce("_system", "_meter", vec![(0, vec![record(b"a")])])
            .await
            .unwrap();
        assert_eq!(*inner.0.lock().unwrap(), 1);
    }

    #[test]
    fn meter_unavailable_is_unavailable() {
        let e = refusal_error(&Refusal::new(
            RefusalReason::MeterUnavailable,
            features::STREAM_INGEST_BYTES,
        ));
        assert!(matches!(e, ServiceError::Unavailable(_)));
    }
}
