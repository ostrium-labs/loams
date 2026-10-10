//! `loams.house.v1` on the main port (HS1 Task 7, design §49 §18.1, D778).
//!
//! Loams House is served by a separate process, `loams-fabric house`, which
//! answers `loams.house.v1` on its own HTTP listener. The console, the desktop
//! and the SDKs see one origin (§44 §4), so this port **forwards** every
//! `/loams.house.v1.*` request to the configured front (`[house] endpoint`,
//! `--house-endpoint`) instead of serving the package itself:
//!
//! - the request goes through as it came: method, path and query, every
//!   end-to-end header **including the caller's `Authorization`** (the front
//!   authenticates; this port does not), and the body, streamed as it arrives;
//! - the answer comes back the same way, streamed as the front produces it, so
//!   a server-streaming `ExecuteQuery` is never buffered whole here;
//! - hop-by-hop headers (RFC 9110 §7.6.1) stay on their hop.
//!
//! A background check (`GET <endpoint>/ping`, which a House front answers
//! `Ok.`) decides whether the catalogue lists `loams.house.v1` as available
//! ([`HouseProxy::healthy`]); the forwarding itself does not consult it, so a
//! front that came back is reachable before the next check.
//!
//! When no endpoint is configured every `loams.house.v1` RPC answers
//! `unimplemented` with the reason `house_not_configured`, and a front that
//! cannot be reached answers `unavailable` with `unavailable`. Both are written
//! in the protocol the caller speaks — a Connect unary error, a Connect
//! end-of-stream message, or gRPC/gRPC-Web trailers — because there is no
//! connect-rust handler behind these paths to render them: the package's protos
//! are HS1 Task 8's, and this port never decodes its messages.
//!
//! The front speaks HTTP/1.1 (HS1 R3.8), so the Connect protocol and gRPC-Web
//! go through; gRPC proper needs HTTP/2 end to end and does not.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use bytes::{BufMut as _, Bytes, BytesMut};
use connectrpc::envelope::Envelope;
use connectrpc::protocol::{Protocol, RequestProtocol};
use connectrpc::{ConnectError, ErrorCode};
use http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use sync_wrapper::SyncWrapper;
use url::Url;

use super::connect::refuse;

/// The package this module forwards.
pub const PACKAGE: &str = "loams.house.v1";

/// Every path of the package starts with this: `/loams.house.v1.<Service>/<Method>`.
pub const PATH_PREFIX: &str = "/loams.house.v1.";

/// How long a connection to the front may take before the call is `unavailable`.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long one health check may take.
pub const HEALTH_TIMEOUT: Duration = Duration::from_secs(2);

/// `[house]`: where the House front is (design §49 §18.1).
#[derive(Clone, Debug)]
pub struct HouseProxyConfig {
    /// The front's base URL, such as `http://127.0.0.1:8123`. `None` (the
    /// default): `loams.house.v1` answers `house_not_configured`.
    pub endpoint: Option<Url>,
    /// How often the front's health is checked. Default 5 s.
    pub health_interval: Duration,
}

impl Default for HouseProxyConfig {
    fn default() -> Self {
        Self {
            endpoint: None,
            health_interval: Duration::from_secs(5),
        }
    }
}

/// The forwarding half: the endpoint, a client, and the last health answer.
#[derive(Debug)]
pub struct HouseProxy {
    endpoint: Url,
    client: reqwest::Client,
    healthy: AtomicBool,
}

impl HouseProxy {
    /// A proxy to `config.endpoint` with its health check running every
    /// `config.health_interval`, or `None` when no endpoint is configured. The
    /// check stops when the last reference is dropped. Must be called inside a
    /// tokio runtime.
    pub fn start(config: &HouseProxyConfig) -> Option<Arc<Self>> {
        let endpoint = config.endpoint.clone()?;
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .unwrap_or_default();
        let proxy = Arc::new(Self {
            endpoint,
            client,
            healthy: AtomicBool::new(false),
        });
        let weak = Arc::downgrade(&proxy);
        let every = config.health_interval.max(Duration::from_millis(10));
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                let Some(proxy) = weak.upgrade() else { return };
                let healthy = proxy.check().await;
                proxy.healthy.store(healthy, Ordering::Relaxed);
            }
        });
        Some(proxy)
    }

    /// Whether the front answered its last health check.
    pub fn healthy(&self) -> bool {
        self.healthy.load(Ordering::Relaxed)
    }

    /// `GET <endpoint>/ping` answers `200`.
    async fn check(&self) -> bool {
        let Ok(url) = self.endpoint.join("ping") else {
            return false;
        };
        matches!(
            self.client.get(url).timeout(HEALTH_TIMEOUT).send().await,
            Ok(response) if response.status() == reqwest::StatusCode::OK
        )
    }

    /// The front's URL for `path_and_query`, under the endpoint's own path.
    fn target(&self, path_and_query: &str) -> String {
        let base = self.endpoint.as_str().trim_end_matches('/');
        format!("{base}{path_and_query}")
    }

    /// Forwards one request and streams the answer back.
    async fn forward(&self, request: Request) -> Response {
        let (parts, body) = request.into_parts();
        let path_and_query = parts
            .uri
            .path_and_query()
            .map_or(parts.uri.path(), |p| p.as_str());
        let protocol = Protocol::detect(&parts.headers);
        let mut upstream = self
            .client
            .request(parts.method.clone(), self.target(path_and_query))
            .body(reqwest::Body::wrap(SyncBody(SyncWrapper::new(body))));
        for (name, value) in end_to_end(&parts.headers) {
            upstream = upstream.header(name, value);
        }
        match upstream.send().await {
            Ok(answer) => {
                let answer = http::Response::from(answer);
                let (mut parts, body) = answer.into_parts();
                strip_hop_by_hop(&mut parts.headers);
                Response::from_parts(parts, Body::new(body))
            }
            Err(err) => {
                tracing::warn!(%err, endpoint = %self.endpoint, "loams.house.v1: the House front is unreachable");
                let error = refuse(
                    ErrorCode::Unavailable,
                    "unavailable",
                    format!("the House front at {} is unreachable", self.endpoint),
                    &[],
                );
                refusal(protocol, &error)
            }
        }
    }
}

/// `routes` with every `/loams.house.v1.*` request taken off to the front, or
/// answered `house_not_configured` when there is none. Applied outermost, so
/// these paths never reach the REST fallback, its body limit or the hot layer.
pub(crate) fn layer(routes: Router, proxy: Option<Arc<HouseProxy>>) -> Router {
    routes.layer(axum::middleware::from_fn(
        move |request: Request, next: Next| {
            let proxy = proxy.clone();
            async move {
                if !request.uri().path().starts_with(PATH_PREFIX) {
                    return next.run(request).await;
                }
                match proxy {
                    Some(proxy) => proxy.forward(request).await,
                    None => {
                        let protocol = Protocol::detect(request.headers());
                        refusal(protocol, &not_configured())
                    }
                }
            }
        },
    ))
}

/// What a `loams.house.v1` RPC answers on a server with no House endpoint.
fn not_configured() -> ConnectError {
    refuse(
        ErrorCode::Unimplemented,
        "house_not_configured",
        "loams.house.v1 is not configured on this server: start it with --house-endpoint \
         <url of a loams-fabric house front>",
        &[],
    )
}

/// The hop-by-hop headers (RFC 9110 §7.6.1), plus `host` (the client sets the
/// front's) and `content-length` on the way back (the body is re-framed).
const HOP_BY_HOP: [HeaderName; 8] = [
    header::CONNECTION,
    HeaderName::from_static("keep-alive"),
    header::PROXY_AUTHENTICATE,
    header::PROXY_AUTHORIZATION,
    header::TE,
    header::TRAILER,
    header::TRANSFER_ENCODING,
    header::UPGRADE,
];

fn is_hop_by_hop(name: &HeaderName, headers: &HeaderMap) -> bool {
    HOP_BY_HOP.contains(name)
        || name == "proxy-connection"
        // Headers the request's own `Connection` names are hop-by-hop too.
        || headers
            .get_all(header::CONNECTION)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .any(|token| token.trim().eq_ignore_ascii_case(name.as_str()))
}

/// The request headers that go to the front: all but hop-by-hop ones and `host`.
fn end_to_end(headers: &HeaderMap) -> Vec<(HeaderName, HeaderValue)> {
    headers
        .iter()
        .filter(|(name, _)| *name != header::HOST && !is_hop_by_hop(name, headers))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

fn strip_hop_by_hop(headers: &mut HeaderMap) {
    let names: Vec<HeaderName> = headers
        .keys()
        .filter(|name| is_hop_by_hop(name, headers) || *name == header::CONTENT_LENGTH)
        .cloned()
        .collect();
    for name in names {
        headers.remove(&name);
    }
}

/// The caller's body for `reqwest`, which needs a `Sync` body; axum's is only
/// `Send`. Polling needs `&mut` alone, which `SyncWrapper` hands out.
struct SyncBody(SyncWrapper<Body>);

impl http_body::Body for SyncBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Bytes>, axum::Error>>> {
        Pin::new(self.get_mut().0.get_mut()).poll_frame(cx)
    }
}

/// `error` in the shape of the caller's protocol (see the module docs).
fn refusal(protocol: Option<RequestProtocol>, error: &ConnectError) -> Response {
    let status = StatusCode::from_u16(error.http_status().as_u16())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let Some(protocol) = protocol else {
        // Not an RPC content type: the Connect unary shape, which any HTTP
        // client can read.
        return connect_unary(status, error);
    };
    match protocol.protocol {
        Protocol::Connect if protocol.is_streaming => {
            let mut end = BytesMut::from(&b"{\"error\":"[..]);
            end.extend_from_slice(&error.to_json());
            end.put_u8(b'}');
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, content_type(protocol))],
                Envelope::end_stream(end.freeze()).encode(),
            )
                .into_response()
        }
        Protocol::Connect => connect_unary(status, error),
        Protocol::Grpc | Protocol::GrpcWeb => grpc(protocol, error),
        _ => connect_unary(status, error),
    }
}

fn connect_unary(status: StatusCode, error: &ConnectError) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        error.to_json(),
    )
        .into_response()
}

fn content_type(protocol: RequestProtocol) -> &'static str {
    protocol
        .protocol
        .response_content_type(protocol.codec_format, true)
}

/// A trailers-only gRPC answer, or gRPC-Web's trailers frame (`0x80`).
fn grpc(protocol: RequestProtocol, error: &ConnectError) -> Response {
    let mut trailers = vec![
        ("grpc-status".to_owned(), error.code.grpc_code().to_string()),
        (
            "grpc-status-details-bin".to_owned(),
            base64::engine::general_purpose::STANDARD_NO_PAD.encode(status_details(error)),
        ),
    ];
    if let Some(message) = &error.message {
        trailers.push(("grpc-message".to_owned(), percent_encode(message)));
    }
    let mut response = if protocol.protocol == Protocol::GrpcWeb {
        let mut block = String::new();
        for (name, value) in &trailers {
            block.push_str(&format!("{name}: {value}\r\n"));
        }
        let mut frame = BytesMut::with_capacity(5 + block.len());
        frame.put_u8(0x80);
        frame.put_u32(block.len() as u32);
        frame.extend_from_slice(block.as_bytes());
        let body = frame.freeze();
        let body = if protocol.is_text_mode {
            Bytes::from(base64::engine::general_purpose::STANDARD.encode(&body))
        } else {
            body
        };
        Response::new(Body::from(body))
    } else {
        // Trailers-only: the status rides in the headers of an empty answer.
        Response::new(Body::empty())
    };
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(content_type(protocol)) {
        headers.insert(header::CONTENT_TYPE, value);
    }
    if protocol.protocol == Protocol::Grpc {
        for (name, value) in &trailers {
            if let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(value),
            ) {
                headers.insert(name, value);
            }
        }
    }
    response
}

/// `google.rpc.Status { code, message, details: [Any(ErrorInfo)] }`, encoded by
/// hand: three fields, and this port has no `google.rpc` types to encode it with.
fn status_details(error: &ConnectError) -> Vec<u8> {
    let mut status = Vec::new();
    status.push(0x08); // field 1, varint
    varint(&mut status, u64::from(error.code.grpc_code()));
    if let Some(message) = &error.message {
        status.push(0x12); // field 2, bytes
        varint(&mut status, message.len() as u64);
        status.extend_from_slice(message.as_bytes());
    }
    for detail in &error.details {
        let value = detail_bytes(detail);
        let mut any = Vec::new();
        let type_url = format!("type.googleapis.com/{}", detail.type_url);
        any.push(0x0a); // Any.type_url
        varint(&mut any, type_url.len() as u64);
        any.extend_from_slice(type_url.as_bytes());
        any.push(0x12); // Any.value
        varint(&mut any, value.len() as u64);
        any.extend_from_slice(&value);
        status.push(0x1a); // field 3, bytes
        varint(&mut status, any.len() as u64);
        status.extend_from_slice(&any);
    }
    status
}

/// A detail's message bytes: connect-rust keeps them base64 (unpadded, but a
/// padded value is read too).
fn detail_bytes(detail: &connectrpc::ErrorDetail) -> Vec<u8> {
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
    let text = detail.value.as_deref().unwrap_or_default();
    STANDARD_NO_PAD
        .decode(text)
        .or_else(|_| STANDARD.decode(text))
        .unwrap_or_default()
}

fn varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// gRPC's `grpc-message` encoding: printable ASCII but `%` as is, the rest
/// percent-encoded.
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if (0x20..=0x7e).contains(&byte) && byte != b'%' {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Only for the unit test below: an `ErrorInfo` back out of a refusal.
#[cfg(test)]
fn reason_in(error: &ConnectError) -> Option<String> {
    use buffa::Message as _;
    use loams_proto::loams::errors::v1::ErrorInfo;
    error
        .details
        .first()
        .and_then(|detail| ErrorInfo::decode_from_slice(&detail_bytes(detail)).ok())
        .map(|info| info.reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hop_by_hop_headers_stay_on_their_hop() {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer x"));
        headers.insert(
            header::CONNECTION,
            HeaderValue::from_static("close, x-private"),
        );
        headers.insert("x-private", HeaderValue::from_static("1"));
        headers.insert("keep-alive", HeaderValue::from_static("timeout=5"));
        headers.insert(header::HOST, HeaderValue::from_static("engine"));
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        let kept: Vec<String> = end_to_end(&headers)
            .into_iter()
            .map(|(name, _)| name.as_str().to_owned())
            .collect();
        assert_eq!(kept, ["authorization", "content-type"]);
    }

    #[test]
    fn grpc_status_details_carry_the_reason() {
        let error = not_configured();
        assert_eq!(reason_in(&error).as_deref(), Some("house_not_configured"));
        let bytes = status_details(&error);
        assert_eq!(bytes[0..2], [0x08, 12], "code 12, UNIMPLEMENTED");
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("type.googleapis.com/loams.errors.v1.ErrorInfo"));
        assert!(text.contains("house_not_configured"));
        assert_eq!(percent_encode("50% ok\n"), "50%25 ok%0A");
    }

    #[test]
    fn targets_keep_the_endpoint_path() {
        let proxy = |endpoint: &str| HouseProxy {
            endpoint: endpoint.parse().expect("url"),
            client: reqwest::Client::new(),
            healthy: AtomicBool::new(false),
        };
        assert_eq!(
            proxy("http://127.0.0.1:8123").target("/loams.house.v1.HouseService/X?a=1"),
            "http://127.0.0.1:8123/loams.house.v1.HouseService/X?a=1"
        );
        assert_eq!(
            proxy("http://h:1/house/").target("/loams.house.v1.HouseService/X"),
            "http://h:1/house/loams.house.v1.HouseService/X"
        );
    }
}
