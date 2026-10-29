//! One Iggy message to one CloudEvent.
//!
//! The Kafka gateway of Ostrium's Iggy fork stores a Kafka record as one Iggy
//! message (the fork's `gateways/kafka/docs/BRIDGE_MAPPING.md`, mapping
//! version 1): the value is the payload, `kafka.v` says the gateway wrote it,
//! `kafka.key` holds the key, `kafka.h.<name>` each record header, `kafka.ts`
//! a timestamp the origin timestamp could not hold, `kafka.value` marks a
//! null (`null`) or empty (`empty`) value stored as one placeholder byte, and
//! `kafka.envelope` marks a record whose key, value and headers are packed in
//! the payload instead. [`decode`] reverses all of that; a message without
//! `kafka.v` is an Iggy client's own and is taken as it is.
//!
//! The event:
//!
//! | Attribute | Value |
//! |---|---|
//! | `id` | the Iggy offset, in decimal |
//! | `source` | `/iggy/<stream>/<topic>/<partition>` (percent-encoded names) |
//! | `type` | [`EVENT_TYPE_KAFKA`], [`EVENT_TYPE_IGGY`] or [`EVENT_TYPE_UNDECODABLE`] |
//! | `time` | the record's timestamp (Kafka's, else the producer's, else the server's) |
//! | `datacontenttype` | `application/json` when the value parses as JSON, `text/plain; charset=utf-8` when it is UTF-8, else `application/octet-stream` |
//! | `partitionkey` | the record key, when it is UTF-8 |
//! | `kafkakey` | the record key, base64 (Binary), when it is not |
//! | `kafkaheaders` | the record headers as a JSON object (a UTF-8 value as a string, any other as `{"base64": …}`, a null one as `null`) |
//! | `iggymessageid` | the Iggy message id, when the producer set one |
//! | `loamserror` | why an undecodable message could not be read (its raw payload is the data) |
//!
//! `source` + `id` is unique per Iggy message, so a replay of the same
//! offsets is deduplicated by the engine's ledger (D270). A null value (a
//! Kafka tombstone) is an event without data.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use bytes::Bytes;
use loams_cloudevents::{Attr, AttrType, CloudEvent, SPEC_VERSION, format_time_ms};
use serde_json::{Map, Value};

use crate::source::{Header, RawMessage};

/// The type of an event made from a record the Kafka gateway wrote.
pub const EVENT_TYPE_KAFKA: &str = "io.loams.dev.kafka.record.v1";
/// The type of an event made from a message an Iggy client wrote.
pub const EVENT_TYPE_IGGY: &str = "io.loams.dev.iggy.message.v1";
/// The type of an event made from a message the mapping could not read: an
/// unknown `kafka.v`, a malformed envelope. Kept, with its raw payload, so the
/// partition does not stall and nothing is lost.
pub const EVENT_TYPE_UNDECODABLE: &str = "io.loams.dev.iggy.undecodable.v1";

const VERSION_HEADER: &[u8] = b"kafka.v";
const MAPPING_VERSION: u8 = 1;
const KEY_HEADER: &[u8] = b"kafka.key";
const VALUE_MARKER_HEADER: &[u8] = b"kafka.value";
const TIMESTAMP_HEADER: &[u8] = b"kafka.ts";
const HEADER_PREFIX: &[u8] = b"kafka.h.";
const ENVELOPE_HEADER: &[u8] = b"kafka.envelope";
const ENVELOPE_VERSION: u8 = 1;
const FLAG_KEY: u8 = 0b01;
const FLAG_VALUE: u8 = 0b10;

/// A message read back as a record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// Whether the Kafka gateway wrote it.
    pub kafka: bool,
    pub key: Option<Bytes>,
    /// `None`: a null value (a tombstone).
    pub value: Option<Bytes>,
    /// In stored order; a `None` value is a null header value.
    pub headers: Vec<(String, Option<Bytes>)>,
    /// Milliseconds since the epoch, if the message has a time.
    pub timestamp_ms: Option<i64>,
}

/// Why a message could not be read as a record.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("the user header block ({0} bytes) cannot be parsed")]
    Headers(u32),
    #[error("kafka.v is {0}; this build reads mapping version {MAPPING_VERSION}")]
    MappingVersion(u8),
    #[error("kafka.envelope is {0}; this build reads envelope version {ENVELOPE_VERSION}")]
    EnvelopeVersion(u8),
    #[error("the envelope is malformed: {0}")]
    Envelope(&'static str),
    #[error("kafka.value is {0:?}, not null or empty")]
    ValueMarker(String),
    #[error("kafka.ts is not a Kafka timestamp")]
    Timestamp,
}

fn header<'a>(headers: &'a [Header], name: &[u8]) -> Option<&'a Header> {
    headers.iter().find(|h| h.name == name)
}

fn origin_ms(message: &RawMessage) -> Option<i64> {
    let micros = match message.origin_timestamp_us {
        0 => message.timestamp_us,
        micros => micros,
    };
    (micros > 0)
        .then(|| i64::try_from(micros / 1000).ok())
        .flatten()
}

/// Reads `message` the way the gateway's Fetch does.
pub fn decode(message: &RawMessage) -> Result<Record, DecodeError> {
    let headers = message
        .headers
        .as_ref()
        .map_err(|len| DecodeError::Headers(*len))?;
    let Some(version) = header(headers, VERSION_HEADER) else {
        // An Iggy client's own message: every header passes through.
        return Ok(Record {
            kafka: false,
            key: None,
            value: Some(message.payload.clone()),
            headers: headers
                .iter()
                .filter_map(|h| {
                    let name = String::from_utf8(h.name.clone()).ok()?;
                    Some((name, Some(h.value.clone())))
                })
                .collect(),
            timestamp_ms: origin_ms(message),
        });
    };
    if version.value.as_ref() != [MAPPING_VERSION] {
        return Err(DecodeError::MappingVersion(
            version.value.first().copied().unwrap_or_default(),
        ));
    }
    let timestamp_ms = match header(headers, TIMESTAMP_HEADER) {
        None => origin_ms(message),
        Some(h) => match h.int64 {
            Some(-1) => (message.timestamp_us > 0)
                .then(|| i64::try_from(message.timestamp_us / 1000).ok())
                .flatten(),
            Some(ms) if ms >= 0 => Some(ms),
            _ => return Err(DecodeError::Timestamp),
        },
    };
    if let Some(envelope) = header(headers, ENVELOPE_HEADER) {
        let (key, value, headers) = decode_envelope(&envelope.value, &message.payload)?;
        return Ok(Record {
            kafka: true,
            key,
            value,
            headers,
            timestamp_ms,
        });
    }
    let value = match header(headers, VALUE_MARKER_HEADER) {
        None => Some(message.payload.clone()),
        Some(marker) => match marker.value.as_ref() {
            b"null" => None,
            b"empty" => Some(Bytes::new()),
            other => {
                return Err(DecodeError::ValueMarker(
                    String::from_utf8_lossy(other).into_owned(),
                ));
            }
        },
    };
    let record_headers = headers
        .iter()
        .filter_map(|h| {
            let name = h.name.strip_prefix(HEADER_PREFIX)?;
            let name = String::from_utf8(name.to_vec()).ok()?;
            Some((name, Some(h.value.clone())))
        })
        .collect();
    Ok(Record {
        kafka: true,
        key: header(headers, KEY_HEADER).map(|h| h.value.clone()),
        value,
        headers: record_headers,
        timestamp_ms,
    })
}

type Fields = (Option<Bytes>, Option<Bytes>, Vec<(String, Option<Bytes>)>);

fn take(buf: &mut Bytes, n: usize) -> Result<Bytes, DecodeError> {
    if buf.len() < n {
        return Err(DecodeError::Envelope("truncated"));
    }
    Ok(buf.split_to(n))
}

fn take_u32(buf: &mut Bytes) -> Result<usize, DecodeError> {
    let bytes = take(buf, 4)?;
    let mut word = [0u8; 4];
    word.copy_from_slice(&bytes);
    usize::try_from(u32::from_le_bytes(word)).map_err(|_| DecodeError::Envelope("length"))
}

fn take_field(buf: &mut Bytes) -> Result<Bytes, DecodeError> {
    let len = take_u32(buf)?;
    take(buf, len)
}

/// The envelope layout of BRIDGE_MAPPING.md, version 1 (little-endian).
fn decode_envelope(version: &[u8], payload: &Bytes) -> Result<Fields, DecodeError> {
    match version.first() {
        Some(&ENVELOPE_VERSION) => {}
        other => {
            return Err(DecodeError::EnvelopeVersion(
                other.copied().unwrap_or_default(),
            ));
        }
    }
    let mut buf = payload.clone();
    let flags = take(&mut buf, 1)?[0];
    let key = take_field(&mut buf)?;
    let value = take_field(&mut buf)?;
    let count = take_u32(&mut buf)?;
    // Every header costs at least nine bytes: bound the count by the input.
    if buf.len() < count.saturating_mul(9) {
        return Err(DecodeError::Envelope("truncated"));
    }
    let mut headers = Vec::with_capacity(count);
    for _ in 0..count {
        let name = String::from_utf8(take_field(&mut buf)?.to_vec())
            .map_err(|_| DecodeError::Envelope("a header name is not UTF-8"))?;
        let present = take(&mut buf, 1)?[0] != 0;
        let value = take_field(&mut buf)?;
        headers.push((name, present.then_some(value)));
    }
    if !buf.is_empty() {
        return Err(DecodeError::Envelope("trailing bytes"));
    }
    Ok((
        (flags & FLAG_KEY != 0).then_some(key),
        (flags & FLAG_VALUE != 0).then_some(value),
        headers,
    ))
}

/// `source` for a partition: `/iggy/<stream>/<topic>/<partition>`.
pub fn source_of(stream: &str, topic: &str, partition: u32) -> String {
    format!("/iggy/{}/{}/{partition}", encode(stream), encode(topic))
}

/// Percent-encodes everything but RFC 3986's unreserved characters.
fn encode(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for b in segment.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn content_type(value: &[u8]) -> &'static str {
    if serde_json::from_slice::<serde::de::IgnoredAny>(value).is_ok() {
        "application/json"
    } else if std::str::from_utf8(value).is_ok() {
        "text/plain; charset=utf-8"
    } else {
        "application/octet-stream"
    }
}

fn headers_json(headers: &[(String, Option<Bytes>)]) -> String {
    let mut map = Map::new();
    for (name, value) in headers {
        let value = match value {
            None => Value::Null,
            Some(bytes) => match std::str::from_utf8(bytes) {
                Ok(text) => Value::String(text.to_string()),
                Err(_) => {
                    let mut inner = Map::new();
                    inner.insert("base64".into(), Value::String(BASE64.encode(bytes)));
                    Value::Object(inner)
                }
            },
        };
        map.insert(name.clone(), value);
    }
    Value::Object(map).to_string()
}

fn base_attrs(
    stream: &str,
    topic: &str,
    partition: u32,
    message: &RawMessage,
    ty: &str,
) -> Vec<Attr> {
    vec![
        Attr::string("specversion", SPEC_VERSION),
        Attr::string("id", message.offset.to_string()),
        Attr {
            name: "source".into(),
            value: source_of(stream, topic, partition),
            kind: AttrType::UriRef,
        },
        Attr::string("type", ty),
    ]
}

/// The CloudEvent for `message` on `stream/topic/partition`. Never fails: a
/// message the mapping cannot read becomes an [`EVENT_TYPE_UNDECODABLE`]
/// event with its raw payload.
pub fn to_event(stream: &str, topic: &str, partition: u32, message: &RawMessage) -> CloudEvent {
    match decode(message).and_then(|record| {
        record_event(stream, topic, partition, message, &record)
            .map_err(|_| DecodeError::Envelope("the event is invalid"))
    }) {
        Ok(event) => event,
        Err(err) => undecodable(stream, topic, partition, message, &err.to_string()),
    }
}

fn record_event(
    stream: &str,
    topic: &str,
    partition: u32,
    message: &RawMessage,
    record: &Record,
) -> Result<CloudEvent, loams_cloudevents::Error> {
    let ty = if record.kafka {
        EVENT_TYPE_KAFKA
    } else {
        EVENT_TYPE_IGGY
    };
    let mut attrs = base_attrs(stream, topic, partition, message, ty);
    if let Some(value) = &record.value {
        attrs.push(Attr::string("datacontenttype", content_type(value)));
    }
    if let Some(ms) = record.timestamp_ms {
        attrs.push(Attr {
            name: "time".into(),
            value: format_time_ms(ms),
            kind: AttrType::Timestamp,
        });
    }
    if let Some(key) = &record.key {
        match std::str::from_utf8(key) {
            Ok(text) if !text.is_empty() => attrs.push(Attr::string("partitionkey", text)),
            _ => attrs.push(Attr {
                name: "kafkakey".into(),
                value: BASE64.encode(key),
                kind: AttrType::Binary,
            }),
        }
    }
    if !record.headers.is_empty() {
        attrs.push(Attr::string("kafkaheaders", headers_json(&record.headers)));
    }
    if message.id != 0 {
        attrs.push(Attr::string("iggymessageid", message.id.to_string()));
    }
    CloudEvent::new(attrs, record.value.clone())
}

fn undecodable(
    stream: &str,
    topic: &str,
    partition: u32,
    message: &RawMessage,
    error: &str,
) -> CloudEvent {
    let mut attrs = base_attrs(stream, topic, partition, message, EVENT_TYPE_UNDECODABLE);
    attrs.push(Attr::string("datacontenttype", "application/octet-stream"));
    if let Some(ms) = origin_ms(message) {
        attrs.push(Attr {
            name: "time".into(),
            value: format_time_ms(ms),
            kind: AttrType::Timestamp,
        });
    }
    attrs.push(Attr::string("loamserror", error));
    // Every attribute above is valid by construction.
    CloudEvent::new(attrs, Some(message.payload.clone()))
        .unwrap_or_else(|err| unreachable!("the undecodable event is valid: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(name: &str, value: &[u8]) -> Header {
        Header {
            name: name.as_bytes().to_vec(),
            value: Bytes::copy_from_slice(value),
            int64: None,
        }
    }

    fn message(offset: u64, payload: &[u8], headers: Vec<Header>) -> RawMessage {
        RawMessage {
            offset,
            id: 0,
            timestamp_us: 1_700_000_000_123_000,
            origin_timestamp_us: 1_700_000_000_456_000,
            payload: Bytes::copy_from_slice(payload),
            headers: Ok(headers),
        }
    }

    #[test]
    fn a_plain_kafka_record() {
        let m = message(
            7,
            b"{\"a\":1}",
            vec![
                h("kafka.v", &[1]),
                h("kafka.key", b"user-1"),
                h("kafka.h.trace", b"abc"),
            ],
        );
        let e = to_event("kafka", "orders", 2, &m);
        assert_eq!(e.id(), "7");
        assert_eq!(e.source(), "/iggy/kafka/orders/2");
        assert_eq!(e.ty(), EVENT_TYPE_KAFKA);
        assert_eq!(e.datacontenttype(), Some("application/json"));
        assert_eq!(e.attr("partitionkey"), Some("user-1"));
        assert_eq!(e.attr("kafkaheaders"), Some("{\"trace\":\"abc\"}"));
        assert_eq!(e.attr("time"), Some("2023-11-14T22:13:20.456Z"));
        assert_eq!(e.data().map(|d| d.as_ref()), Some(&b"{\"a\":1}"[..]));
    }

    #[test]
    fn the_dedup_key_is_unique_per_offset_and_partition() {
        let a = to_event("kafka", "t", 0, &message(1, b"x", vec![]));
        let b = to_event("kafka", "t", 0, &message(1, b"y", vec![]));
        let c = to_event("kafka", "t", 0, &message(2, b"x", vec![]));
        let d = to_event("kafka", "t", 1, &message(1, b"x", vec![]));
        assert_eq!(
            a.dedup_key(),
            b.dedup_key(),
            "a replay of one offset is one event"
        );
        assert_ne!(a.dedup_key(), c.dedup_key());
        assert_ne!(a.dedup_key(), d.dedup_key());
    }

    #[test]
    fn tombstones_and_empty_values() {
        let tomb = to_event(
            "kafka",
            "t",
            0,
            &message(1, &[0], vec![h("kafka.v", &[1]), h("kafka.value", b"null")]),
        );
        assert_eq!(tomb.data(), None);
        assert_eq!(tomb.datacontenttype(), None);
        let empty = to_event(
            "kafka",
            "t",
            0,
            &message(
                2,
                &[0],
                vec![h("kafka.v", &[1]), h("kafka.value", b"empty")],
            ),
        );
        assert_eq!(empty.data().map(|d| d.len()), Some(0));
    }

    #[test]
    fn kafka_ts_overrides_the_origin_time() {
        let mut ts = h("kafka.ts", &[]);
        ts.int64 = Some(0);
        let e = to_event(
            "kafka",
            "t",
            0,
            &message(1, b"v", vec![h("kafka.v", &[1]), ts.clone()]),
        );
        assert_eq!(e.attr("time"), Some("1970-01-01T00:00:00.000Z"));
        ts.int64 = Some(-1);
        let e = to_event(
            "kafka",
            "t",
            0,
            &message(1, b"v", vec![h("kafka.v", &[1]), ts]),
        );
        assert_eq!(
            e.attr("time"),
            Some("2023-11-14T22:13:20.123Z"),
            "-1 is the server's time"
        );
    }

    fn envelope(
        key: Option<&[u8]>,
        value: Option<&[u8]>,
        headers: &[(&str, Option<&[u8]>)],
    ) -> Vec<u8> {
        let mut out = vec![u8::from(key.is_some()) | (u8::from(value.is_some()) << 1)];
        for field in [key, value] {
            let field = field.unwrap_or_default();
            out.extend_from_slice(&u32::try_from(field.len()).unwrap_or_default().to_le_bytes());
            out.extend_from_slice(field);
        }
        out.extend_from_slice(
            &u32::try_from(headers.len())
                .unwrap_or_default()
                .to_le_bytes(),
        );
        for (name, value) in headers {
            out.extend_from_slice(&u32::try_from(name.len()).unwrap_or_default().to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.push(u8::from(value.is_some()));
            let v = value.unwrap_or_default();
            out.extend_from_slice(&u32::try_from(v.len()).unwrap_or_default().to_le_bytes());
            out.extend_from_slice(v);
        }
        out
    }

    #[test]
    fn an_enveloped_record() {
        let long_key = vec![0xffu8; 300];
        let payload = envelope(
            Some(&long_key),
            Some(b"hello"),
            &[("a", Some(b"1")), ("b", None)],
        );
        let m = message(
            3,
            &payload,
            vec![h("kafka.v", &[1]), h("kafka.envelope", &[1])],
        );
        let e = to_event("kafka", "t", 0, &m);
        assert_eq!(e.ty(), EVENT_TYPE_KAFKA);
        assert_eq!(e.data().map(|d| d.as_ref()), Some(&b"hello"[..]));
        assert_eq!(e.attr("kafkakey"), Some(BASE64.encode(&long_key).as_str()));
        assert_eq!(e.attr("kafkaheaders"), Some("{\"a\":\"1\",\"b\":null}"));
        assert_eq!(e.datacontenttype(), Some("text/plain; charset=utf-8"));
    }

    #[test]
    fn an_empty_key_is_kept_as_binary() {
        let payload = envelope(Some(b""), Some(b"v"), &[]);
        let m = message(
            3,
            &payload,
            vec![h("kafka.v", &[1]), h("kafka.envelope", &[1])],
        );
        assert_eq!(to_event("kafka", "t", 0, &m).attr("kafkakey"), Some(""));
    }

    #[test]
    fn unreadable_messages_are_kept_not_dropped() {
        for m in [
            message(1, b"x", vec![h("kafka.v", &[9])]),
            message(1, b"x", vec![h("kafka.v", &[1]), h("kafka.envelope", &[1])]),
            message(1, b"x", vec![h("kafka.v", &[1]), h("kafka.value", b"what")]),
            RawMessage {
                headers: Err(12),
                ..message(1, b"x", vec![])
            },
        ] {
            let e = to_event("kafka", "t", 0, &m);
            assert_eq!(e.ty(), EVENT_TYPE_UNDECODABLE);
            assert_eq!(e.id(), "1");
            assert_eq!(e.data().map(|d| d.as_ref()), Some(&b"x"[..]));
            assert!(e.attr("loamserror").is_some_and(|s| !s.is_empty()));
        }
    }

    #[test]
    fn an_iggy_clients_message_passes_its_headers_through() {
        let mut m = message(
            4,
            &[0xff, 0x00],
            vec![h("kafka.key", b"not-a-key"), h("trace", b"t1")],
        );
        m.id = 42;
        let e = to_event("app", "raw events", 0, &m);
        assert_eq!(e.ty(), EVENT_TYPE_IGGY);
        assert_eq!(e.source(), "/iggy/app/raw%20events/0");
        assert_eq!(
            e.attr("partitionkey"),
            None,
            "kafka.* means nothing without kafka.v"
        );
        assert_eq!(
            e.attr("kafkaheaders"),
            Some("{\"kafka.key\":\"not-a-key\",\"trace\":\"t1\"}")
        );
        assert_eq!(e.datacontenttype(), Some("application/octet-stream"));
        assert_eq!(e.attr("iggymessageid"), Some("42"));
    }
}
