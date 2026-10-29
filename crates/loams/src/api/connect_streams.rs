//! `loams.stream.v1` on the main port (design §44 §5.1; API2 Task 2; SDKP
//! P0c).
//!
//! Every RPC calls the code its native REST route calls
//! (`/v1/namespaces/{ns}/streams/...` in [`super`] and [`super::events`]):
//! the same metastore commands, the same `LogWriter` and `LogReader`, the same
//! CloudEvents ingest with its idempotency ledger. What the RPCs add:
//!
//! - **`CreateStream` is idempotent by name**: the same name and partition
//!   count answers the existing stream with `created = false`.
//! - **`ListStreams` pages** (AIP-158, an offset token, as `ListCollections`).
//! - **Idempotent `Produce`**: `producer_id` + `producer_seq` is a key in the
//!   stream's ledger, so a repeat answers the first append's offsets with
//!   `duplicate` set and appends nothing (`produce_with_producer_seq_dedupes`).
//!   Without them a produce is a plain append, and two identical ones append
//!   twice.
//! - **`Fetch.max_records`**, on top of the reader's byte budget, and the
//!   long poll `max_wait_ms` the reader already serves (capped at 30 s here).
//!
//! The tonic `StreamService` on `--stream-grpc-listen` (feature
//! `stream-grpc`) and the Dapr edge keep their copy of this package's proto,
//! which this one is wire-compatible with (see `stream.proto`'s header).

// The generated service traits return `impl Encodable<_>`; these impls name
// the concrete message type (`connect.rs` does the same).
#![allow(refining_impl_trait)]

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use connectrpc::{
    ConnectError, ErrorCode, RequestContext, Response, ServiceRequest, ServiceResult,
};
use loams_cloudevents::json as ce_json;
use loams_cloudevents::record::{RecordContext, from_record};
use loams_collection::ConsistencyToken;
use loams_common::meta::{
    ApplyError, Consistency, IdempotencyClaim, IdempotencyCompletion, IdempotencyKey,
    IdempotencyState, MetaError, Retention as NativeRetention, StreamState, WalClass,
};
use loams_common::{StreamId, meta::Stream as NativeStream};
use loams_log::{FetchRequest as LogFetch, Record as LogRecord};
use loams_proto::loams::stream::v1::{
    CreateStreamRequest, CreateStreamResponse, DescribeStreamRequest, DescribeStreamResponse,
    EventResult, EventStatus as PbEventStatus, FetchCloudEventsRequest, FetchCloudEventsResponse,
    FetchRequest, FetchResponse, FetchedRecord, Header, ListStreamsRequest, ListStreamsResponse,
    PartitionOffsets, ProduceCloudEventsRequest, ProduceCloudEventsResponse, ProduceRequest,
    ProduceResponse, Retention, StreamInfo, StreamService, StreamServiceExt,
};
use sha2::{Digest, Sha256};

use super::connect::refuse;
use super::connect_errors::{invalid, refused, refused_meta};
use super::errors::ApiError;
use super::events::{CLAIM_TTL_MS, DetachedIngest, EventStatus, new_owner};
use super::streams::{produce_records, refuse_system_stream};
use super::{AppState, DEFAULT_MAX_BYTES, MAX_FETCH_BYTES, namespace_id, stream_id};

/// The longest long poll a fetch may ask for.
const MAX_WAIT_MS: u32 = 30_000;

/// The largest page `ListStreams` answers.
const MAX_PAGE: usize = 1000;

#[derive(Debug)]
struct Streams {
    state: AppState,
}

impl Streams {
    fn new(state: AppState) -> Arc<Self> {
        Arc::new(Self { state })
    }
}

fn required<'a>(field: &str, value: &'a str) -> Result<&'a str, ConnectError> {
    if value.is_empty() {
        Err(invalid(field, format!("{field} is required")))
    } else {
        Ok(value)
    }
}

fn retention_of(retention: &NativeRetention) -> Retention {
    Retention {
        max_age_ms: retention.max_age_ms,
        max_bytes: retention.max_bytes,
        ..Default::default()
    }
}

fn info(namespace: &str, stream: &NativeStream) -> StreamInfo {
    StreamInfo {
        namespace: namespace.to_owned(),
        name: stream.name.clone(),
        id: stream.id.0,
        partitions: stream.partitions,
        retention: buffa::MessageField::some(retention_of(&stream.retention)),
        ..Default::default()
    }
}

fn byte_budget(max_bytes: Option<u32>) -> usize {
    max_bytes
        .map_or(DEFAULT_MAX_BYTES, |n| n as usize)
        .min(MAX_FETCH_BYTES)
}

fn wait_of(max_wait_ms: Option<u32>) -> Duration {
    Duration::from_millis(u64::from(max_wait_ms.unwrap_or(0).min(MAX_WAIT_MS)))
}

/// The ledger key of an idempotent produce: the producer and its sequence.
fn produce_key(producer_id: &str, seq: u64) -> IdempotencyKey {
    let mut hash = Sha256::new();
    hash.update(b"loams.stream.v1.Produce");
    hash.update([0u8]);
    hash.update(producer_id.as_bytes());
    hash.update([0u8]);
    hash.update(seq.to_be_bytes());
    hash.finalize().into()
}

impl Streams {
    async fn stream(&self, ns: &str, name: &str) -> Result<NativeStream, ConnectError> {
        let namespace = namespace_id(&*self.state.meta, ns).await.map_err(refused)?;
        self.state
            .meta
            .stream_by_name(Consistency::Local, namespace, name)
            .await
            .map_err(refused_meta)?
            .ok_or_else(|| refused(ApiError::not_found(format!("stream {ns}/{name} not found"))))
    }

    /// One append, and its token: the partition's next offset.
    async fn append(
        &self,
        ns: &str,
        stream: &str,
        partition: u32,
        records: Vec<LogRecord>,
    ) -> Result<(StreamId, u64, u64), ApiError> {
        let acks = produce_records(
            &self.state.meta,
            &self.state.writer,
            ns,
            stream,
            vec![(partition, records)],
        )
        .await?;
        let ack = acks.into_iter().next().ok_or_else(|| {
            ApiError::from(loams_log::LogError::CommitUnknown(
                "no acknowledgement for an append".to_string(),
            ))
        })?;
        Ok((ack.stream, ack.base_offset, ack.last_offset))
    }
}

fn produced(
    id: StreamId,
    partition: u32,
    base: u64,
    last: u64,
    duplicate: bool,
) -> ProduceResponse {
    ProduceResponse {
        stream_id: id.0,
        base_offset: base,
        last_offset: last,
        consistency_token: ConsistencyToken(vec![(id, partition, last + 1)]).to_string(),
        duplicate,
        ..Default::default()
    }
}

impl StreamService for Streams {
    async fn create_stream(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, CreateStreamRequest>,
    ) -> ServiceResult<CreateStreamResponse> {
        let ns = required("namespace", request.namespace)?;
        let name = required("name", request.name)?;
        if request.partitions == 0 {
            return Err(invalid("partitions", "a stream has at least one partition"));
        }
        let namespace = namespace_id(&*self.state.meta, ns).await.map_err(refused)?;
        let retention = request
            .retention
            .as_option()
            .map_or(NativeRetention::default(), |r| NativeRetention {
                max_age_ms: r.max_age_ms,
                max_bytes: r.max_bytes,
            });
        let created = match self
            .state
            .meta
            .create_stream(
                namespace,
                name,
                request.partitions,
                WalClass::Standard,
                retention,
            )
            .await
        {
            Ok(_) => true,
            Err(MetaError::Rejected(ApplyError::StreamExists(_))) => false,
            Err(err) => return Err(refused_meta(err)),
        };
        let stream = self.stream(ns, name).await?;
        if !created && stream.partitions != request.partitions {
            return Err(refuse(
                ErrorCode::AlreadyExists,
                "already_exists",
                format!(
                    "stream {ns}/{name} exists with {} partitions, not {}",
                    stream.partitions, request.partitions
                ),
                &[("kind", "stream".to_owned()), ("name", name.to_owned())],
            ));
        }
        Response::ok(CreateStreamResponse {
            stream: buffa::MessageField::some(info(ns, &stream)),
            created,
            ..Default::default()
        })
    }

    async fn describe_stream(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, DescribeStreamRequest>,
    ) -> ServiceResult<DescribeStreamResponse> {
        let ns = required("namespace", request.namespace)?;
        let name = required("stream", request.stream)?;
        let stream = self.stream(ns, name).await?;
        let mut out = info(ns, &stream);
        if let Some(StreamState { partitions, .. }) = self
            .state
            .meta
            .stream_state(Consistency::Local, stream.id)
            .await
            .map_err(refused_meta)?
        {
            out.offsets = (0u32..)
                .zip(partitions)
                .filter_map(|(partition, bounds)| {
                    let bounds = bounds?;
                    Some(PartitionOffsets {
                        partition,
                        log_start_offset: bounds.log_start_offset,
                        high_watermark: bounds.high_watermark,
                        ..Default::default()
                    })
                })
                .collect();
        }
        Response::ok(DescribeStreamResponse {
            stream: buffa::MessageField::some(out),
            ..Default::default()
        })
    }

    async fn list_streams(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, ListStreamsRequest>,
    ) -> ServiceResult<ListStreamsResponse> {
        let ns = required("namespace", request.namespace)?;
        if request.page_size < 0 {
            return Err(invalid("page_size", "page_size cannot be negative"));
        }
        let start = if request.page_token.is_empty() {
            0
        } else {
            request
                .page_token
                .parse::<usize>()
                .map_err(|_| invalid("page_token", "page_token is not one this server issued"))?
        };
        let size = match request.page_size {
            0 => MAX_PAGE,
            n => (n as usize).min(MAX_PAGE),
        };
        let namespace = namespace_id(&*self.state.meta, ns).await.map_err(refused)?;
        let mut streams = self
            .state
            .meta
            .streams(Consistency::Local, Some(namespace))
            .await
            .map_err(refused_meta)?;
        streams.sort_by(|a, b| a.name.cmp(&b.name));
        let end = (start + size).min(streams.len());
        let page = streams.get(start..end).unwrap_or_default();
        Response::ok(ListStreamsResponse {
            streams: page.iter().map(|stream| info(ns, stream)).collect(),
            next_page_token: if end < streams.len() {
                end.to_string()
            } else {
                String::new()
            },
            ..Default::default()
        })
    }

    async fn produce(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, ProduceRequest>,
    ) -> ServiceResult<ProduceResponse> {
        let ns = required("namespace", request.namespace)?;
        let name = required("stream", request.stream)?;
        refuse_system_stream(name).map_err(refused)?;
        if request.records.is_empty() {
            return Err(invalid("records", "a produce carries at least one record"));
        }
        let partition = request.partition;
        let records: Vec<LogRecord> = request
            .records
            .iter()
            .map(|record| LogRecord {
                key: record.key.map(Bytes::copy_from_slice),
                value: record.value.map(Bytes::copy_from_slice),
                headers: record
                    .headers
                    .iter()
                    .map(|h| (h.key.to_owned(), h.value.map(Bytes::copy_from_slice)))
                    .collect(),
                // Zero or negative takes the writer's clock.
                timestamp_ms: if record.timestamp_ms > 0 {
                    record.timestamp_ms
                } else {
                    -1
                },
            })
            .collect();
        let count = records.len() as u64;

        let (Some(producer), Some(seq)) = (request.producer_id, request.producer_seq) else {
            if request.producer_id.is_some() != request.producer_seq.is_some() {
                return Err(invalid(
                    "producer_seq",
                    "producer_id and producer_seq go together: set both for an idempotent produce",
                ));
            }
            let (id, base, last) = self
                .append(ns, name, partition, records)
                .await
                .map_err(refused)?;
            return Response::ok(produced(id, partition, base, last, false));
        };

        // The idempotent path: claim the key, append, complete the claim.
        let stream = self.stream(ns, name).await?;
        let key = produce_key(producer, seq);
        let owner = new_owner(self.state.node_id);
        let states = self
            .state
            .meta
            .claim_idempotency_keys(IdempotencyClaim {
                stream: stream.id,
                owner: owner.clone(),
                keys: vec![key],
                ttl_ms: CLAIM_TTL_MS,
            })
            .await
            .map_err(refused_meta)?;
        match states.first() {
            Some(IdempotencyState::Done {
                partition: at,
                offset,
            }) => {
                return Response::ok(produced(stream.id, *at, *offset, offset + count - 1, true));
            }
            Some(IdempotencyState::InFlight { until_ms }) => {
                let wait = until_ms.saturating_sub(self.state.meta.now_ms()).max(1000);
                return Err(refuse(
                    ErrorCode::Unavailable,
                    "unavailable",
                    format!(
                        "producer {producer} sequence {seq} is being appended by another request; \
                         retry after {wait} ms"
                    ),
                    &[("retry_after_ms", wait.to_string())],
                ));
            }
            Some(IdempotencyState::Claimed) => {}
            None => {
                return Err(refused(super::errors::internal(
                    "the ledger answered no state for a claimed key",
                )));
            }
        }
        let appended = self.append(ns, name, partition, records).await;
        let (id, base, last) = match appended {
            Ok(done) => done,
            Err(err) => {
                // A refused append releases the claim so a retry need not wait
                // for it to lapse; an append whose outcome is unknown keeps it.
                if err.code() != "unavailable"
                    && let Err(release) = self
                        .state
                        .meta
                        .release_idempotency_keys(stream.id, &owner, vec![key])
                        .await
                {
                    tracing::warn!(%release, "releasing a produce claim failed");
                }
                return Err(refused(err));
            }
        };
        if let Err(err) = self
            .state
            .meta
            .complete_idempotency_keys(IdempotencyCompletion {
                stream: stream.id,
                owner,
                done: vec![(key, partition, base)],
                window_ms: self.state.cloudevents.window_ms(),
            })
            .await
        {
            tracing::warn!(%err, "completing a produce claim failed");
        }
        Response::ok(produced(id, partition, base, last, false))
    }

    async fn fetch(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, FetchRequest>,
    ) -> ServiceResult<FetchResponse> {
        let ns = required("namespace", request.namespace)?;
        let name = required("stream", request.stream)?;
        let id = stream_id(&*self.state.meta, ns, name)
            .await
            .map_err(refused)?;
        let mut response = self
            .state
            .reader
            .fetch(LogFetch {
                stream: id,
                partition: request.partition,
                offset: request.offset,
                max_bytes: byte_budget(request.max_bytes),
                max_wait: wait_of(request.max_wait_ms),
            })
            .await
            .map_err(|err| refused(err.into()))?;
        let mut next_offset = response.next_offset;
        if let Some(max) = request.max_records
            && (max as usize) < response.records.len()
        {
            response.records.truncate(max.max(1) as usize);
            next_offset = response
                .records
                .last()
                .map_or(request.offset, |r| r.offset + 1);
        }
        Response::ok(FetchResponse {
            records: response
                .records
                .iter()
                .map(|r| FetchedRecord {
                    offset: r.offset,
                    key: r.record.key.as_ref().map(|k| k.to_vec()),
                    value: r.record.value.as_ref().map(|v| v.to_vec()),
                    headers: r
                        .record
                        .headers
                        .iter()
                        .map(|(key, value)| Header {
                            key: key.clone(),
                            value: value.as_ref().map(|v| v.to_vec()),
                            ..Default::default()
                        })
                        .collect(),
                    timestamp_ms: r.record.timestamp_ms,
                    ..Default::default()
                })
                .collect(),
            next_offset,
            high_watermark: response.high_watermark,
            log_start_offset: response.log_start_offset,
            ..Default::default()
        })
    }

    async fn produce_cloud_events(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, ProduceCloudEventsRequest>,
    ) -> ServiceResult<ProduceCloudEventsResponse> {
        use loams_proto::loams::stream::v1::__buffa::view::oneof::produce_cloud_events_request::Events;
        let ns = required("namespace", request.namespace)?;
        let name = required("stream", request.stream)?;
        refuse_system_stream(name).map_err(refused)?;
        let events = match request.events.as_ref() {
            Some(Events::Batch(bytes)) => loams_cloudevents::protobuf::decode_batch(bytes)
                .map_err(|err| invalid("batch", format!("invalid CloudEvent batch: {err}")))?,
            Some(Events::JsonBatch(bytes)) => ce_json::parse_batch(bytes)
                .map_err(|err| invalid("json_batch", format!("invalid CloudEvent batch: {err}")))?,
            None => return Err(invalid("events", "send a batch or a json_batch")),
        };
        // On its own task: a client that goes away mid-request must not leave
        // appended events with open claims (see `DetachedIngest`).
        let report = DetachedIngest {
            meta: self.state.meta.clone(),
            writer: self.state.writer.clone(),
            config: self.state.cloudevents,
            node_id: self.state.node_id,
        }
        .run(ns.to_owned(), name.to_owned(), request.partition, events)
        .await
        .map_err(refused)?;
        Response::ok(ProduceCloudEventsResponse {
            results: report
                .outcomes
                .iter()
                .map(|outcome| {
                    let (partition, offset) = outcome.location.unwrap_or_default();
                    EventResult {
                        status: match outcome.status {
                            EventStatus::Appended => PbEventStatus::EVENT_STATUS_APPENDED,
                            EventStatus::Duplicate => PbEventStatus::EVENT_STATUS_DUPLICATE,
                            EventStatus::InFlight { .. } => PbEventStatus::EVENT_STATUS_IN_FLIGHT,
                        }
                        .into(),
                        partition,
                        offset,
                        retry_after_ms: match outcome.status {
                            EventStatus::InFlight { retry_after_ms } => retry_after_ms,
                            _ => 0,
                        },
                        ..Default::default()
                    }
                })
                .collect(),
            consistency_token: if report.token.0.is_empty() {
                String::new()
            } else {
                report.token.to_string()
            },
            ..Default::default()
        })
    }

    async fn fetch_cloud_events(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, FetchCloudEventsRequest>,
    ) -> ServiceResult<FetchCloudEventsResponse> {
        let ns = required("namespace", request.namespace)?;
        let name = required("stream", request.stream)?;
        let id = stream_id(&*self.state.meta, ns, name)
            .await
            .map_err(refused)?;
        let response = self
            .state
            .reader
            .fetch(LogFetch {
                stream: id,
                partition: request.partition,
                offset: request.offset,
                max_bytes: byte_budget(request.max_bytes),
                max_wait: wait_of(request.max_wait_ms),
            })
            .await
            .map_err(|err| refused(err.into()))?;
        let events: Vec<_> = response
            .records
            .iter()
            .map(|r| {
                from_record(
                    &r.record,
                    RecordContext {
                        namespace: ns,
                        stream: name,
                        stream_id: id.0,
                        partition: request.partition,
                        offset: r.offset,
                    },
                )
            })
            .collect();
        Response::ok(FetchCloudEventsResponse {
            json_batch: ce_json::write_batch(&events),
            count: events.len() as u32,
            next_offset: response.next_offset,
            high_watermark: response.high_watermark,
            ..Default::default()
        })
    }
}

pub(super) fn register(router: connectrpc::Router, state: &AppState) -> connectrpc::Router {
    StreamServiceExt::register(Streams::new(state.clone()), router)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn produce_keys_differ_by_producer_and_sequence() {
        assert_eq!(produce_key("p", 1), produce_key("p", 1));
        assert_ne!(produce_key("p", 1), produce_key("p", 2));
        assert_ne!(produce_key("p", 1), produce_key("q", 1));
        // The separator keeps "ab"+"c" apart from "a"+"bc".
        assert_ne!(produce_key("ab", 1), produce_key("a", 1));
    }

    #[test]
    fn the_long_poll_is_capped() {
        assert_eq!(wait_of(None), Duration::ZERO);
        assert_eq!(wait_of(Some(60_000)), Duration::from_millis(30_000));
        assert_eq!(byte_budget(None), DEFAULT_MAX_BYTES);
        assert_eq!(byte_budget(Some(u32::MAX)), MAX_FETCH_BYTES);
    }
}
