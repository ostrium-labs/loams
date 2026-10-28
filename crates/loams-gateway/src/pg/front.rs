//! The Postgres front door (design §57 §7): sniff, refuse or route, then
//! splice (`SSLRequest` clients) or terminate and re-encrypt (direct TLS).

use std::sync::Arc;
use std::time::{Duration, Instant};

use loams_meter_gate::entitle::{Entitlements, features};
use loams_meter_gate::gate::Refusal;
use rustls::pki_types::ServerName;
use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::{TlsAcceptor, TlsConnector};

use super::Entry;
use super::upstream::{self, Ejections, UpstreamError};
use crate::config::TlsMode;
use crate::io::Prefixed;
use crate::observe::{ConnectionObserver, Path};
use crate::reload::RouteRx;
use crate::risingwave::{RwEndpoint, RwFrontDoor};
use crate::routes::{PgNamespace, RouteTable};
use crate::sniff::SniffError;
use crate::sniff::pg::{Opening, fatal_error, sniff_opening};
use crate::sniff::tls::{ClientHello, MAX_CLIENT_HELLO, alert, read_client_hello};
use crate::tls::{ALPN_POSTGRESQL, ServerCerts};

/// The message a plaintext client gets (SQLSTATE 28000, D1015).
pub const PLAINTEXT_REFUSED: &str =
    "Loams Postgres requires TLS: connect with sslmode=require or stronger";

/// Why a connection ended before it was relayed. Logged, never sent.
#[derive(Debug, thiserror::Error)]
pub enum FrontError {
    /// No route table is loaded yet.
    #[error("no route table loaded yet")]
    NoRoutes,
    /// The opening or ClientHello could not be read.
    #[error(transparent)]
    Sniff(#[from] SniffError),
    /// A plaintext `StartupMessage` (refused, D1015).
    #[error("plaintext startup refused")]
    Plaintext,
    /// Bytes arrived before the gateway answered the request.
    #[error("data sent before the server's answer to {0}")]
    Pipelined(&'static str),
    /// A second `GSSENCRequest`.
    #[error("a second GSSENCRequest")]
    GssTwice,
    /// An opening that is not a Postgres packet or TLS.
    #[error("unrecognized opening")]
    Unknown,
    /// No namespace for the SNI (or no SNI and no default namespace).
    #[error("no namespace for SNI {0:?}")]
    UnknownName(Option<String>),
    /// Direct TLS without ALPN `postgresql`.
    #[error("direct TLS without ALPN postgresql")]
    NoAlpn,
    /// Direct TLS while `tls_mode = "passthrough"`.
    #[error("direct TLS refused in passthrough mode")]
    DirectInPassthrough,
    /// No configured certificate covers the name.
    #[error("no certificate for {0}")]
    NoCertificate(String),
    /// The gateway holds no copy of PgDog's certificate (channel binding).
    #[error("namespace {0}: PgDog's certificate is not among the gateway's")]
    CertificateMismatch(String),
    /// No PgDog of the namespace could be used.
    #[error("upstream: {0}")]
    Upstream(#[from] UpstreamError),
    /// A TLS handshake failed.
    #[error("tls: {0}")]
    Tls(std::io::Error),
    /// The handshake deadline passed (D1020).
    #[error("handshake timed out")]
    Timeout,
    /// A RisingWave client was not let in (design 81, D1957).
    #[error("risingwave: {0}")]
    RisingWave(String),
    /// The meter's gate refused the namespace's org (design §63 §10.2):
    /// new connections are refused at the TLS alert (`access_denied`); the
    /// client is in TLS by then, so no Postgres error can be sent in splice
    /// mode.
    #[error("{0}")]
    Entitlement(Refusal),
    /// A socket error.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// How the client asked for TLS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Negotiation {
    SslRequest,
    Direct,
}

/// A connection ready to relay.
enum Ready {
    Splice {
        client: TcpStream,
        upstream: TcpStream,
        ns: Arc<PgNamespace>,
    },
    Terminate {
        client: Box<tokio_rustls::server::TlsStream<Prefixed<TcpStream>>>,
        upstream: Box<tokio_rustls::client::TlsStream<TcpStream>>,
        ns: Arc<PgNamespace>,
    },
    /// A RisingWave instance: TLS ended at the gateway, the client
    /// checked against Loams, the gateway logged in upstream.
    RisingWave {
        client: Box<tokio_rustls::server::TlsStream<Prefixed<TcpStream>>>,
        upstream: TcpStream,
        ep: Arc<RwEndpoint>,
    },
    /// Nothing to relay (a cancel request was handled).
    Done,
}

/// The Postgres module's per-process state.
#[derive(Debug)]
pub struct PgFrontDoor {
    routes: RouteRx,
    mode: TlsMode,
    handshake_timeout: Duration,
    connect_timeout: Duration,
    server_certs: ServerCerts,
    upstream_tls: Option<Arc<rustls::ClientConfig>>,
    ejections: Ejections,
    observer: Arc<dyn ConnectionObserver>,
    risingwave: Option<Arc<RwFrontDoor>>,
    entitlements: Entitlements,
}

/// What [`PgFrontDoor::new`] takes.
#[derive(Debug)]
pub struct FrontDoorOptions {
    pub mode: TlsMode,
    pub handshake_timeout: Duration,
    pub connect_timeout: Duration,
    pub server_certs: ServerCerts,
    pub upstream_tls: Option<Arc<rustls::ClientConfig>>,
    pub observer: Arc<dyn ConnectionObserver>,
    /// The RisingWave module; `None` refuses RisingWave routes.
    pub risingwave: Option<Arc<RwFrontDoor>>,
    /// The meter's gate, asked on route resolution (`NoGate` by default).
    pub entitlements: Entitlements,
}

impl PgFrontDoor {
    pub fn new(routes: RouteRx, options: FrontDoorOptions) -> Self {
        Self {
            routes,
            mode: options.mode,
            handshake_timeout: options.handshake_timeout,
            connect_timeout: options.connect_timeout,
            server_certs: options.server_certs,
            upstream_tls: options.upstream_tls,
            ejections: Ejections::default(),
            observer: options.observer,
            entitlements: options.entitlements,
            risingwave: options.risingwave,
        }
    }

    /// Serves one client connection to its end.
    pub async fn serve(self: Arc<Self>, client: TcpStream, entry: Entry) {
        let peer = client.peer_addr().ok();
        let started = Instant::now();
        let ready = match tokio::time::timeout(
            self.handshake_timeout,
            self.handshake(client, entry),
        )
        .await
        {
            Ok(Ok(ready)) => ready,
            Ok(Err(e)) => {
                tracing::debug!(?peer, ?entry, error = %e, "postgres connection refused");
                return;
            }
            Err(_) => {
                tracing::debug!(?peer, ?entry, error = %FrontError::Timeout, "postgres connection refused");
                return;
            }
        };
        let (name, path, result) = match ready {
            Ready::Done => return,
            Ready::Splice {
                mut client,
                mut upstream,
                ns,
            } => {
                keepalive(&client);
                keepalive(&upstream);
                self.observer.on_open(&ns.name, Path::Spliced);
                let r = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
                (ns.name.clone(), Path::Spliced, r)
            }
            Ready::Terminate {
                mut client,
                mut upstream,
                ns,
            } => {
                keepalive(client.get_ref().0.get_ref());
                keepalive(upstream.get_ref().0);
                self.observer.on_open(&ns.name, Path::Terminated);
                let r = tokio::io::copy_bidirectional(&mut *client, &mut *upstream).await;
                (ns.name.clone(), Path::Terminated, r)
            }
            Ready::RisingWave {
                mut client,
                mut upstream,
                ep,
            } => {
                keepalive(client.get_ref().0.get_ref());
                keepalive(&upstream);
                self.observer.on_open(&ep.name, Path::Terminated);
                let r = tokio::io::copy_bidirectional(&mut *client, &mut upstream).await;
                (ep.name.clone(), Path::Terminated, r)
            }
        };
        let (bytes_in, bytes_out) = match result {
            Ok(counts) => counts,
            Err(e) => {
                tracing::debug!(?peer, namespace = %name, error = %e, "relay ended with an error");
                (0, 0)
            }
        };
        self.observer
            .on_close(&name, bytes_in, bytes_out, started.elapsed());
        tracing::debug!(?peer, namespace = %name, ?path, bytes_in, bytes_out, "postgres connection closed");
    }

    async fn handshake(&self, mut client: TcpStream, entry: Entry) -> Result<Ready, FrontError> {
        let table = self.routes.borrow().clone().ok_or(FrontError::NoRoutes)?;
        let mut buf = Vec::new();
        let mut gss_answered = false;
        loop {
            match sniff_opening(&mut client, &mut buf).await? {
                Opening::SslRequest => {
                    if buf.len() > 8 {
                        return Err(FrontError::Pipelined("SSLRequest"));
                    }
                    client.write_all(b"S").await?;
                    buf.clear();
                    let hello = read_client_hello(&mut client, &mut buf, MAX_CLIENT_HELLO).await?;
                    return self
                        .route(client, buf, hello, entry, &table, Negotiation::SslRequest)
                        .await;
                }
                Opening::Tls => {
                    let hello = read_client_hello(&mut client, &mut buf, MAX_CLIENT_HELLO).await?;
                    return self
                        .route(client, buf, hello, entry, &table, Negotiation::Direct)
                        .await;
                }
                Opening::GssEncRequest => {
                    if gss_answered {
                        return Err(FrontError::GssTwice);
                    }
                    if buf.len() > 8 {
                        return Err(FrontError::Pipelined("GSSENCRequest"));
                    }
                    gss_answered = true;
                    client.write_all(b"N").await?;
                    buf.clear();
                }
                Opening::Startup => {
                    client
                        .write_all(&fatal_error("28000", PLAINTEXT_REFUSED))
                        .await?;
                    let _ = client.shutdown().await;
                    return Err(FrontError::Plaintext);
                }
                Opening::Cancel(packet) => {
                    self.cancel(&packet, entry, &table).await;
                    return Ok(Ready::Done);
                }
                Opening::Unknown => return Err(FrontError::Unknown),
            }
        }
    }

    /// Picks the namespace and the path once the ClientHello is read.
    async fn route(
        &self,
        mut client: TcpStream,
        hello_bytes: Vec<u8>,
        hello: ClientHello,
        entry: Entry,
        table: &RouteTable,
        negotiation: Negotiation,
    ) -> Result<Ready, FrontError> {
        let rw = match entry {
            Entry::Dedicated(port) => table.risingwave.by_port(port),
            Entry::Shared => hello
                .server_name
                .as_deref()
                .and_then(|n| table.risingwave.by_hostname(n)),
        };
        if let Some(ep) = rw.cloned() {
            return self.risingwave(client, hello_bytes, hello, ep).await;
        }
        let ns = match entry {
            Entry::Dedicated(port) => table.by_port(port),
            Entry::Shared => match &hello.server_name {
                Some(name) => table.by_hostname(name),
                None => table.pg.default_namespace.as_ref(),
            },
        };
        let Some(ns) = ns.cloned() else {
            send_alert(&mut client, alert::UNRECOGNIZED_NAME).await;
            return Err(FrontError::UnknownName(hello.server_name));
        };
        let terminate = match (negotiation, self.mode) {
            (Negotiation::Direct, TlsMode::Passthrough) => {
                send_alert(&mut client, alert::NO_APPLICATION_PROTOCOL).await;
                return Err(FrontError::DirectInPassthrough);
            }
            (Negotiation::Direct, _) => true,
            (Negotiation::SslRequest, TlsMode::Terminate) => true,
            (Negotiation::SslRequest, _) => false,
        };
        if negotiation == Negotiation::Direct && !hello.offers_alpn(ALPN_POSTGRESQL) {
            send_alert(&mut client, alert::NO_APPLICATION_PROTOCOL).await;
            return Err(FrontError::NoAlpn);
        }
        // Has the namespace's org access to Postgres at all (§63 §10.2,
        // D2186)? A hard-stopped org's new connections end here; open ones
        // run on. Spliced (passthrough), the client is in TLS with PgDog's
        // certificate the gateway cannot speak for: the TLS alert
        // `access_denied`. Terminated, the gateway ends TLS itself and
        // answers the startup with SQLSTATE 53400.
        if let Err(r) = self
            .entitlements
            .check(None, &ns.name, features::PG_COMPUTE, 0)
        {
            if terminate {
                return Err(self
                    .refuse_terminated(client, hello_bytes, &hello, &ns, r)
                    .await);
            }
            send_alert(&mut client, alert::ACCESS_DENIED).await;
            return Err(FrontError::Entitlement(r));
        }
        if terminate {
            self.terminate(client, hello_bytes, hello, ns).await
        } else {
            self.splice(client, hello_bytes, ns).await
        }
    }

    /// A RisingWave instance (design 81, D1957): TLS ends here with a
    /// certificate of `[risingwave]`, then [`RwFrontDoor::login`] checks the
    /// client's Loams credential and logs in upstream.
    async fn risingwave(
        &self,
        mut client: TcpStream,
        hello_bytes: Vec<u8>,
        hello: ClientHello,
        ep: Arc<RwEndpoint>,
    ) -> Result<Ready, FrontError> {
        let Some(rw) = self.risingwave.clone() else {
            send_alert(&mut client, alert::UNRECOGNIZED_NAME).await;
            return Err(FrontError::UnknownName(hello.server_name));
        };
        let name = hello
            .server_name
            .clone()
            .or_else(|| ep.hostnames.first().cloned())
            .unwrap_or_default();
        let Some(config) = rw.server_certs.config_for_name(&name) else {
            send_alert(&mut client, alert::UNRECOGNIZED_NAME).await;
            return Err(FrontError::NoCertificate(name));
        };
        let mut tls = TlsAcceptor::from(config)
            .accept(Prefixed::new(hello_bytes, client))
            .await
            .map_err(FrontError::Tls)?;
        let upstream = rw
            .login(&mut tls, &ep)
            .await
            .map_err(|e| FrontError::RisingWave(e.to_string()))?;
        Ok(Ready::RisingWave {
            client: Box::new(tls),
            upstream,
            ep,
        })
    }

    /// TLS stays between the client and PgDog: the gateway sends its own
    /// `SSLRequest` and forwards the client's TLS bytes unchanged.
    async fn splice(
        &self,
        mut client: TcpStream,
        hello_bytes: Vec<u8>,
        ns: Arc<PgNamespace>,
    ) -> Result<Ready, FrontError> {
        let (mut upstream, _addr) =
            match upstream::connect_pgdog(&ns, &self.ejections, self.connect_timeout).await {
                Ok(u) => u,
                Err(e) => {
                    send_alert(&mut client, alert::INTERNAL_ERROR).await;
                    return Err(e.into());
                }
            };
        upstream.write_all(&hello_bytes).await?;
        Ok(Ready::Splice {
            client,
            upstream,
            ns,
        })
    }

    /// TLS ends at the gateway (ALPN `postgresql`), and a second TLS
    /// session to PgDog verifies PgDog's certificate. PgDog is reached first,
    /// so the gateway can present the very certificate PgDog presents:
    /// SCRAM channel binding hashes it (see [`ServerCerts`]).
    async fn terminate(
        &self,
        mut client: TcpStream,
        hello_bytes: Vec<u8>,
        hello: ClientHello,
        ns: Arc<PgNamespace>,
    ) -> Result<Ready, FrontError> {
        let name = hello
            .server_name
            .clone()
            .or_else(|| ns.canonical_hostname().map(str::to_owned))
            .unwrap_or_default();
        let Some(connector) = self.upstream_tls.clone().map(TlsConnector::from) else {
            send_alert(&mut client, alert::INTERNAL_ERROR).await;
            return Err(FrontError::NoCertificate(name));
        };
        if !self.server_certs.covers(&name) {
            send_alert(&mut client, alert::UNRECOGNIZED_NAME).await;
            return Err(FrontError::NoCertificate(name));
        }
        let upstream_name = ServerName::try_from(ns.upstream_server_name().to_owned())
            .map_err(|e| FrontError::Tls(std::io::Error::other(e)))?;
        let upstream = async {
            let (tcp, _addr) =
                upstream::connect_pgdog(&ns, &self.ejections, self.connect_timeout).await?;
            connector
                .connect(upstream_name, tcp)
                .await
                .map_err(FrontError::Tls)
        };
        let tls_upstream = match upstream.await {
            Ok(u) => u,
            Err(e) => {
                send_alert(&mut client, alert::INTERNAL_ERROR).await;
                return Err(e);
            }
        };
        let pgdog_leaf = tls_upstream
            .get_ref()
            .1
            .peer_certificates()
            .and_then(|c| c.first())
            .map(|c| c.as_ref().to_vec())
            .unwrap_or_default();
        let Some(server_config) = self.server_certs.config_for_leaf(&pgdog_leaf) else {
            tracing::error!(
                namespace = %ns.name,
                "the gateway holds no copy of PgDog's certificate: direct-TLS clients would fail \
                 SCRAM channel binding, so they are refused (configure [[pg.certs]] with PgDog's certificate)"
            );
            send_alert(&mut client, alert::INTERNAL_ERROR).await;
            return Err(FrontError::CertificateMismatch(ns.name.clone()));
        };
        let tls_client = TlsAcceptor::from(server_config)
            .accept(Prefixed::new(hello_bytes, client))
            .await
            .map_err(FrontError::Tls)?;
        Ok(Ready::Terminate {
            client: Box::new(tls_client),
            upstream: Box::new(tls_upstream),
            ns,
        })
    }

    /// Refuses a terminate-mode connection the gate refused: TLS with the
    /// gateway's certificate for the name, the client's startup read (at
    /// most 2 s), then a Postgres `ErrorResponse` naming the entitlement:
    /// SQLSTATE `53400` (`configuration_limit_exceeded`), or `53300` when
    /// the meter is unavailable (§63 §17.4). Without a certificate for the
    /// name it falls back to the TLS alert `access_denied`.
    async fn refuse_terminated(
        &self,
        mut client: TcpStream,
        hello_bytes: Vec<u8>,
        hello: &ClientHello,
        ns: &PgNamespace,
        r: Refusal,
    ) -> FrontError {
        let name = hello
            .server_name
            .clone()
            .or_else(|| ns.canonical_hostname().map(str::to_owned))
            .unwrap_or_default();
        let Some(config) = self.server_certs.config_for_name(&name) else {
            send_alert(&mut client, alert::ACCESS_DENIED).await;
            return FrontError::Entitlement(r);
        };
        if let Ok(mut tls) = TlsAcceptor::from(config)
            .accept(Prefixed::new(hello_bytes, client))
            .await
        {
            let mut startup = [0u8; 1024];
            let _ = tokio::time::timeout(Duration::from_secs(2), tls.read(&mut startup)).await;
            let _ = tls
                .write_all(&fatal_error(
                    entitlement_sqlstate(&r),
                    &entitlement_message(&r),
                ))
                .await;
            let _ = tls.shutdown().await;
        }
        FrontError::Entitlement(r)
    }

    /// A plaintext cancel: every replica of the namespace on a dedicated
    /// port; dropped on the shared port, which cannot route it (Q895).
    async fn cancel(&self, packet: &[u8; 16], entry: Entry, table: &RouteTable) {
        match entry {
            Entry::Dedicated(port) => match table.by_port(port) {
                Some(ns) => {
                    let sent = upstream::broadcast_cancel(ns, packet, self.connect_timeout).await;
                    tracing::debug!(namespace = %ns.name, sent, "cancel request forwarded");
                }
                None => tracing::debug!(port, "cancel on a port with no namespace dropped"),
            },
            Entry::Shared => {
                tracing::debug!("plaintext cancel on the shared port dropped (no SNI)");
            }
        }
    }
}

async fn send_alert<W: AsyncWrite + Unpin>(w: &mut W, description: u8) {
    let _ = w.write_all(&alert::record(description)).await;
    let _ = w.shutdown().await;
}

fn keepalive(stream: &TcpStream) {
    let ka = socket2::TcpKeepalive::new()
        .with_time(Duration::from_secs(60))
        .with_interval(Duration::from_secs(15));
    if let Err(e) = socket2::SockRef::from(stream).set_tcp_keepalive(&ka) {
        tracing::debug!(error = %e, "tcp keepalive not set");
    }
}

/// The SQLSTATE of a gate refusal (§63 §17.4): `53400`
/// (`configuration_limit_exceeded`) for an exhausted allowance, the spend
/// cap, a missing payment method or entitlement; `53300`
/// (`too_many_connections`, retryable) when the meter is unavailable.
pub fn entitlement_sqlstate(r: &Refusal) -> &'static str {
    match r.reason {
        loams_meter_gate::gate::RefusalReason::MeterUnavailable => "53300",
        _ => "53400",
    }
}

/// The message of a gate refusal: names the entitlement and the reason.
pub fn entitlement_message(r: &Refusal) -> String {
    format!(
        "Loams: entitlement {} is refused ({}); new connections are not accepted",
        r.feature, r.reason
    )
}

#[cfg(test)]
mod entitlement_tests {
    use super::*;
    use loams_meter_gate::gate::RefusalReason;

    #[test]
    fn hard_stop_is_53400_and_names_the_feature() {
        let r = Refusal::new(RefusalReason::EntitlementExhausted, "pg_compute");
        assert_eq!(entitlement_sqlstate(&r), "53400");
        let msg = entitlement_message(&r);
        assert!(msg.contains("pg_compute") && msg.contains("entitlement_exhausted"));
        let frame = fatal_error(entitlement_sqlstate(&r), &msg);
        assert_eq!(frame[0], b'E');
        assert!(frame.windows(6).any(|w| w == b"C53400"));
    }

    #[test]
    fn meter_unavailable_is_retryable_53300() {
        let r = Refusal::new(RefusalReason::MeterUnavailable, "pg_compute");
        assert_eq!(entitlement_sqlstate(&r), "53300");
        for reason in [
            RefusalReason::SpendCapReached,
            RefusalReason::PaymentMethodRequired,
            RefusalReason::NotEntitled,
        ] {
            assert_eq!(
                entitlement_sqlstate(&Refusal::new(reason, "pg_compute")),
                "53400"
            );
        }
    }
}
