//! What the proxy reads and writes on the wire (SP-a D1994).
//!
//! Spark Connect messages are forwarded as bytes with [`RawCodec`], so a
//! client newer than the vendored protos (PySpark 4.2 against Sail, say)
//! loses nothing. Every Spark Connect request message carries
//! `string session_id = 1` and `UserContext user_context = 2`
//! (`tests/proto.rs` checks this against the descriptor set); the proxy
//! decodes only those two fields with [`SessionHeader`] and never parses a
//! plan.
//!
//! To pin the caller's identity the proxy **appends** a `user_context`
//! sub-message carrying only `user_id` ([`with_user_id`]). Protobuf merges
//! repeated occurrences of a singular message field and keeps the last value
//! of a singular scalar, so the backend sees the client's `user_context`
//! (`user_name`, `extensions`) with `user_id` replaced, and every other byte
//! of the request is unchanged.

use bytes::{Buf, BufMut, Bytes, BytesMut};
use prost::Message;
use tonic::Status;
use tonic::codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};

/// A pass-through codec: a gRPC message is its bytes.
#[derive(Debug, Clone, Copy, Default)]
pub struct RawCodec;

/// [`RawCodec`]'s encoder.
#[derive(Debug, Clone, Copy, Default)]
pub struct RawEncoder;

/// [`RawCodec`]'s decoder.
#[derive(Debug, Clone, Copy, Default)]
pub struct RawDecoder;

impl Codec for RawCodec {
    type Encode = Bytes;
    type Decode = Bytes;
    type Encoder = RawEncoder;
    type Decoder = RawDecoder;

    fn encoder(&mut self) -> Self::Encoder {
        RawEncoder
    }

    fn decoder(&mut self) -> Self::Decoder {
        RawDecoder
    }
}

impl Encoder for RawEncoder {
    type Item = Bytes;
    type Error = Status;

    fn encode(&mut self, item: Bytes, dst: &mut EncodeBuf<'_>) -> Result<(), Status> {
        dst.put(item);
        Ok(())
    }
}

impl Decoder for RawDecoder {
    type Item = Bytes;
    type Error = Status;

    fn decode(&mut self, src: &mut DecodeBuf<'_>) -> Result<Option<Bytes>, Status> {
        let n = src.remaining();
        Ok(Some(src.copy_to_bytes(n)))
    }
}

/// `spark.connect.UserContext`, the fields the proxy touches.
#[derive(Clone, PartialEq, Eq, Message)]
pub struct UserContextHeader {
    /// `string user_id = 1`.
    #[prost(string, tag = "1")]
    pub user_id: String,
    /// `string user_name = 2`.
    #[prost(string, tag = "2")]
    pub user_name: String,
}

/// The two fields every Spark Connect request starts with. Decoding a full
/// request as this message skips every other field.
#[derive(Clone, PartialEq, Eq, Message)]
pub struct SessionHeader {
    /// `string session_id = 1`.
    #[prost(string, tag = "1")]
    pub session_id: String,
    /// `UserContext user_context = 2`.
    #[prost(message, optional, tag = "2")]
    pub user_context: Option<UserContextHeader>,
}

/// `CloneSessionResponse`'s `new_session_id = 3`, for counting the session a
/// clone creates.
#[derive(Clone, PartialEq, Eq, Message)]
pub struct CloneSessionHeader {
    /// `string session_id = 1`.
    #[prost(string, tag = "1")]
    pub session_id: String,
    /// `string new_session_id = 3`.
    #[prost(string, tag = "3")]
    pub new_session_id: String,
}

/// `CloneSessionRequest`'s `optional string new_session_id = 4`.
#[derive(Clone, PartialEq, Eq, Message)]
pub struct CloneSessionRequestHeader {
    /// `optional string new_session_id = 4`.
    #[prost(string, optional, tag = "4")]
    pub new_session_id: Option<String>,
}

/// Reads the session header of a request.
///
/// # Errors
///
/// `INVALID_ARGUMENT` when the bytes are not a protobuf message, or the
/// session id is not a UUID (Spark Connect requires one).
pub fn session_header(message: &[u8]) -> Result<SessionHeader, Status> {
    let header = SessionHeader::decode(message)
        .map_err(|e| Status::invalid_argument(format!("not a Spark Connect request: {e}")))?;
    check_session_id(&header.session_id)?;
    Ok(header)
}

/// Checks that a session id is a UUID, as Spark Connect servers require.
///
/// # Errors
///
/// `INVALID_ARGUMENT` otherwise.
pub fn check_session_id(id: &str) -> Result<(), Status> {
    uuid::Uuid::try_parse(id)
        .map(|_| ())
        .map_err(|_| Status::invalid_argument("session_id must be a UUID"))
}

/// The request with `user_context.user_id` set to `user_id`: the original
/// bytes followed by a `user_context` field holding only `user_id`.
pub fn with_user_id(message: &Bytes, user_id: &str) -> Bytes {
    let tail = SessionHeader {
        session_id: String::new(),
        user_context: Some(UserContextHeader {
            user_id: user_id.to_owned(),
            user_name: String::new(),
        }),
    };
    let mut out = BytesMut::with_capacity(message.len() + tail.encoded_len());
    out.extend_from_slice(message);
    tail.encode(&mut out)
        .expect("BytesMut grows to fit the encoded tail");
    out.freeze()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::connect;

    const SID: &str = "8b0b7a8e-6d4c-4f7e-9f63-1c2d3e4f5a6b";

    fn config_request() -> connect::ConfigRequest {
        connect::ConfigRequest {
            session_id: SID.into(),
            user_context: Some(connect::UserContext {
                user_id: "alice-claims-to-be-root".into(),
                user_name: "alice".into(),
                extensions: Vec::new(),
            }),
            client_type: Some("pyspark/4.1.3".into()),
            ..Default::default()
        }
    }

    #[test]
    fn reads_the_session_header_of_any_request() {
        let bytes = config_request().encode_to_vec();
        let header = session_header(&bytes).unwrap();
        assert_eq!(header.session_id, SID);
        assert_eq!(header.user_context.unwrap().user_name, "alice");

        let exec = connect::ExecutePlanRequest {
            session_id: SID.into(),
            operation_id: Some("op-1".into()),
            ..Default::default()
        };
        assert_eq!(
            session_header(&exec.encode_to_vec()).unwrap().session_id,
            SID
        );
    }

    #[test]
    fn rejects_non_uuid_sessions() {
        let mut req = config_request();
        req.session_id = "../other-tenant".into();
        let err = session_header(&req.encode_to_vec()).unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn user_id_override_keeps_every_other_field() {
        let original = config_request();
        let bytes = Bytes::from(original.encode_to_vec());
        let rewritten = with_user_id(&bytes, "ns_42/user:u-1");
        let decoded = connect::ConfigRequest::decode(rewritten.as_ref()).unwrap();
        let ctx = decoded.user_context.clone().unwrap();
        assert_eq!(ctx.user_id, "ns_42/user:u-1");
        assert_eq!(ctx.user_name, "alice");
        assert_eq!(decoded.client_type.as_deref(), Some("pyspark/4.1.3"));
        assert_eq!(decoded.session_id, SID);
    }

    #[test]
    fn user_id_override_adds_a_missing_user_context() {
        let req = connect::InterruptRequest {
            session_id: SID.into(),
            ..Default::default()
        };
        let rewritten = with_user_id(&Bytes::from(req.encode_to_vec()), "u");
        let decoded = connect::InterruptRequest::decode(rewritten.as_ref()).unwrap();
        assert_eq!(decoded.user_context.unwrap().user_id, "u");
    }

    #[test]
    fn unknown_fields_of_newer_clients_survive() {
        // Field 999 (varint 7) is unknown to these protos; it must reach the
        // backend unchanged.
        let mut bytes = config_request().encode_to_vec();
        prost::encoding::encode_key(999, prost::encoding::WireType::Varint, &mut bytes);
        prost::encoding::encode_varint(7, &mut bytes);
        let original = Bytes::from(bytes);
        let rewritten = with_user_id(&original, "u");
        assert!(rewritten.starts_with(&original));
    }

    #[test]
    fn clone_response_header() {
        let resp = connect::CloneSessionResponse {
            session_id: SID.into(),
            new_session_id: "11111111-2222-4333-8444-555555555555".into(),
            ..Default::default()
        };
        let h = CloneSessionHeader::decode(resp.encode_to_vec().as_slice()).unwrap();
        assert_eq!(h.new_session_id, "11111111-2222-4333-8444-555555555555");
    }
}
