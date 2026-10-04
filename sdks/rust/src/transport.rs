//! The transport (design §44 §4; D600, D128).
//!
//! One port serves the Connect protocol, gRPC and gRPC-Web (D600), and the SDK
//! speaks all three through **connect-rust** — the server's own stack (D128,
//! D362). `tonic` is deliberately absent: the SDK2 plan rules it out for this
//! SDK, and choosing a second gRPC stack would mean two codegen pipelines and
//! two error shapes for the same protos.
//!
//! [`HttpClient`] is the default: the Connect protocol over HTTP/1.1, which
//! works for every endpoint and is what `loams dev` serves. Two narrower
//! transports are available for callers that want them:
//!
//! * [`grpc_transport`] — gRPC over HTTP/2 with prior knowledge (h2c), for a
//!   loopback `loams dev` where HTTP/2 is on the table and half-duplex streams
//!   are worth the prior-knowledge requirement;
//! * a caller-supplied `connectrpc::client::ClientTransport`, which is what
//!   [`crate::Loams::with_transport`] takes. A TLS-terminating sidecar, a
//!   pinned-certificate connector and a mock are all reachable that way, and
//!   none of them needs a hook in this crate.
//!
//! Rust has no browser target to worry about the way R10 does for TypeScript, so
//! there is no subpath split here: `reqwest` and every other Node-shaped concern
//! is simply not involved. The one thing R10's spirit asks for — *the default
//! entry point reaches for no unnecessary stack* — is why the TLS roots are
//! behind the `tls` feature rather than always linked.

use std::sync::Arc;
use std::time::Duration;

use connectrpc::Protocol;
use connectrpc::client::{ClientConfig, HttpClient};
use http::Uri;

use crate::error::LoamsError;

/// Which wire protocol the transport speaks. All three are served on one port
/// (design §44 §4), so this is a choice, not a constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WireProtocol {
    /// The Connect protocol over HTTP/1.1 or ALPN: a unary call is an HTTP
    /// `POST` with a JSON or protobuf body, and a server stream is the Connect
    /// streaming envelope. This is the default, because it is the one that works
    /// through every proxy and is what `curl` sends.
    #[default]
    Connect,
    /// gRPC over HTTP/2. Needs either ALPN (TLS) or prior knowledge (h2c).
    Grpc,
    /// gRPC-Web: gRPC's framing over HTTP/1.1, which is what a browser sends.
    GrpcWeb,
}

impl WireProtocol {
    fn as_protocol(self) -> Protocol {
        match self {
            WireProtocol::Connect => Protocol::Connect,
            WireProtocol::Grpc => Protocol::Grpc,
            WireProtocol::GrpcWeb => Protocol::GrpcWeb,
        }
    }
}

/// How to build a [`crate::Loams`].
#[derive(Debug, Clone)]
pub struct TransportOptions {
    /// The instance's base URL, `https://acme.loams.dev` or
    /// `http://127.0.0.1:8080` for a loopback stack.
    pub endpoint: String,
    /// Which wire protocol to speak. All three are served on the one port.
    pub protocol: WireProtocol,
    /// The client's default deadline, applied to calls that set none.
    pub timeout: Option<Duration>,
    /// Whether the response body is the proto3 JSON mapping rather than
    /// protobuf bytes. `false` (protobuf) is the default, and is what an SDK
    /// sends; `true` is what a fixture recorded from `curl` carries.
    pub json: bool,
}

impl TransportOptions {
    /// The defaults for an endpoint: Connect, protobuf, no deadline.
    #[must_use]
    pub fn new(endpoint: impl Into<String>) -> Self {
        TransportOptions {
            endpoint: endpoint.into(),
            protocol: WireProtocol::default(),
            timeout: None,
            json: false,
        }
    }

    /// Speaks `protocol` instead of Connect.
    #[must_use]
    pub fn protocol(mut self, protocol: WireProtocol) -> Self {
        self.protocol = protocol;
        self
    }

    /// Sets the client's default deadline.
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Asks for the proto3 JSON mapping rather than protobuf bytes.
    #[must_use]
    pub fn json(mut self, json: bool) -> Self {
        self.json = json;
        self
    }

    /// The base URI, parsed.
    ///
    /// # Errors
    ///
    /// Returns a [`LoamsError`] when the endpoint is not an absolute URI with a
    /// scheme and an authority, which is the one thing a base URL must have.
    pub fn uri(&self) -> Result<Uri, LoamsError> {
        let bad = || {
            LoamsError::internal(format!(
                "`{}` has no scheme or authority; an endpoint looks like https://acme.loams.dev",
                self.endpoint
            ))
        };
        let trimmed = self.endpoint.trim().trim_end_matches('/');
        let uri: Uri = trimmed.parse().map_err(|error| {
            LoamsError::internal(format!("`{}` is not a base URL: {error}", self.endpoint))
        })?;
        // `http::Uri` happily parses a bare path-and-query, so parsing alone does
        // not prove there is an endpoint here: `://x` arrives as a relative
        // reference. Require the scheme Connect actually speaks, and a host.
        match uri.scheme_str() {
            Some("http") | Some("https") => {}
            _ => return Err(bad()),
        }
        if uri.authority().map(|authority| authority.host()).unwrap_or("").is_empty() {
            return Err(bad());
        }
        Ok(uri)
    }

    /// The connect-rust client configuration this endpoint implies.
    ///
    /// # Errors
    ///
    /// Returns a [`LoamsError`] when the endpoint is not a usable base URL.
    pub fn config(&self) -> Result<ClientConfig, LoamsError> {
        let uri = self.uri()?;
        let mut config = ClientConfig::new(uri).with_protocol(self.protocol.as_protocol());
        config = config.with_codec_format(if self.json {
            connectrpc::codec::CodecFormat::Json
        } else {
            connectrpc::codec::CodecFormat::Proto
        });
        if let Some(timeout) = self.timeout {
            config = config.with_default_timeout(timeout);
        }
        Ok(config)
    }
}

/// The default transport: Connect over HTTP/1.1 (or ALPN), plaintext or TLS.
///
/// # Errors
///
/// Returns a [`LoamsError`] when the endpoint is not a usable base URL, or when
/// it is `https://` and the `tls` feature is off — an SDK that silently fell
/// back to plaintext on an `https://` endpoint would send a bearer in the clear,
/// so that is refused rather than guessed at.
pub fn transport(options: &TransportOptions) -> Result<HttpClient, LoamsError> {
    let uri = options.uri()?;
    if uri.scheme_str() == Some("https") {
        #[cfg(feature = "tls")]
        {
            return Ok(HttpClient::builder().with_tls(tls_config()?));
        }
        #[cfg(not(feature = "tls"))]
        {
            return Err(LoamsError::internal(
                "`https://` needs this crate's `tls` feature, which is off: rebuild with \
                 `--features tls`, or point the endpoint at a plaintext sidecar",
            ));
        }
    }
    Ok(HttpClient::plaintext())
}

/// The h2c transport: gRPC over HTTP/2 with prior knowledge.
///
/// For a loopback `loams dev` on plain HTTP/2, where a caller wants the gRPC
/// framing rather than the Connect envelope. It does **not** work through a proxy
/// that speaks HTTP/1.1 only, which is why it is not the default.
///
/// # Errors
///
/// Returns a [`LoamsError`] when the endpoint is not a usable base URL, or is
/// `https://` — prior knowledge is a plaintext technique, and TLS negotiates
/// HTTP/2 through ALPN instead, which [`transport`] already does.
pub fn grpc_transport(options: &TransportOptions) -> Result<HttpClient, LoamsError> {
    let uri = options.uri()?;
    if uri.scheme_str() == Some("https") {
        return Err(LoamsError::internal(
            "prior-knowledge h2c is a plaintext technique; for an `https://` endpoint use \
             `transport`, which negotiates HTTP/2 through ALPN",
        ));
    }
    Ok(HttpClient::plaintext_http2_only())
}

/// A rustls client configuration for an `https://` endpoint: the platform's
/// trust anchors, no client certificate, HTTP/2 and HTTP/1.1 both offered.
///
/// # Errors
///
/// Returns a [`LoamsError`] when the platform offers no TLS backend or no
/// protocol versions, which on a supported host does not happen — a failure here
/// means the host has no TLS at all, and reporting beats connecting without it.
#[cfg(feature = "tls")]
pub fn tls_config() -> Result<Arc<rustls::ClientConfig>, LoamsError> {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|error| LoamsError::internal(format!("no usable TLS protocol version: {error}")))?
        // Empty ALPN: connect-rust's `with_tls` sets `[h2, http/1.1]` itself,
        // and hyper-rustls rejects a non-empty list on input.
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_endpoint_becomes_a_config_with_the_chosen_protocol_and_encoding() {
        let config = TransportOptions::new("https://acme.loams.dev/")
            .protocol(WireProtocol::Grpc)
            .json(true)
            .timeout(Duration::from_secs(5))
            .config()
            .expect("a base URL");
        // `http::Uri` normalises an empty path to "/", so compare the parts the
        // endpoint actually names rather than its re-serialised string.
        let base = config.base_uri();
        assert_eq!(base.scheme_str(), Some("https"));
        assert_eq!(base.authority().map(|a| a.as_str()), Some("acme.loams.dev"));
        assert_eq!(config.protocol(), Protocol::Grpc);
        assert_eq!(config.codec_format(), connectrpc::codec::CodecFormat::Json);
        assert_eq!(config.default_timeout(), Some(Duration::from_secs(5)));
    }

    #[test]
    fn the_default_is_the_connect_protocol_with_protobuf_bytes() {
        let config = TransportOptions::new("http://127.0.0.1:8080")
            .config()
            .expect("a base URL");
        assert_eq!(config.protocol(), Protocol::Connect);
        assert_eq!(config.codec_format(), connectrpc::codec::CodecFormat::Proto);
        assert_eq!(config.default_timeout(), None);
    }

    #[test]
    fn an_endpoint_without_a_scheme_or_authority_is_refused_by_name() {
        for bad in ["acme.loams.dev", "/v1", "://x"] {
            let error = TransportOptions::new(bad).config().unwrap_err();
            assert!(
                error.to_string().contains(bad) || error.to_string().contains("base URL"),
                "{bad}: {error}"
            );
        }
    }

    #[test]
    fn prior_knowledge_h2c_refuses_https_rather_than_guessing() {
        let error = grpc_transport(&TransportOptions::new("https://acme.loams.dev"))
            .expect_err("https is not h2c");
        assert!(error.to_string().contains("ALPN"), "{error}");
    }

    #[cfg(feature = "tls")]
    #[test]
    fn the_tls_configuration_offers_h2_and_http11() {
        let config = tls_config().expect("a rustls configuration");
        assert!(
            config.alpn_protocols.is_empty(),
            "connect-rust sets ALPN itself"
        );
    }
}
