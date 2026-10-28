//! TLS for the SQL API and `loams risingwave` (plan RW1, lane RW-b; design
//! 81 R8: loams-gateway terminates TLS in front of the frontend).
//!
//! The rule: **a connection to any host that is not loopback uses TLS and
//! verifies the server's certificate**; a plain connection is allowed only to
//! loopback (`localhost`, `127.0.0.0/8`, `::1`) or a Unix socket, which is
//! how the compose `engine-gateway` is reached. `sslmode=disable` in the URL
//! of a remote host is refused, and `sslmode=prefer` (tokio-postgres's
//! default, which falls back to plain text) is upgraded to require.
//!
//! The stack is the workspace's own rustls (ring provider) over tokio-rustls,
//! adapted to tokio-postgres's `MakeTlsConnect`, as in
//! `fabric/crates/loams-connector-postgres`. Trust roots are the Mozilla set
//! (webpki-roots) plus an optional PEM bundle (`--ca-file`,
//! `LOAMS_RISINGWAVE_CA_FILE`) for a gateway with a private CA. Certificates
//! are always verified; there is no "accept any certificate" mode.

use std::future::Future;
use std::io;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use rustls::ClientConfig;
use rustls::pki_types::{CertificateDer, ServerName};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_postgres::Socket;
use tokio_postgres::config::{Host, SslMode};
use tokio_postgres::tls::{ChannelBinding, MakeTlsConnect, TlsConnect};

/// TLS settings a caller can give.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TlsOptions {
    /// A PEM bundle of extra trust anchors (a private CA).
    pub ca_file: Option<PathBuf>,
    /// Use TLS even to a loopback host.
    pub force_tls: bool,
}

/// Why a connection's transport could not be chosen.
#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    #[error(
        "{host} is not a loopback host, so the connection needs TLS; remove sslmode=disable from the URL"
    )]
    PlainToRemote { host: String },
    #[error("the URL names no host")]
    NoHost,
    #[error("reading the CA file {path}: {message}")]
    CaFile { path: String, message: String },
    #[error("TLS configuration: {0}")]
    Config(String),
}

/// How to connect.
#[derive(Debug, Clone)]
pub enum Transport {
    /// Plain TCP or a Unix socket: loopback only.
    Plain,
    /// TLS with certificate verification.
    Tls(MakeRustls),
}

/// Whether `host` is loopback: `localhost` (and `*.localhost`), an address
/// in `127.0.0.0/8`, or `::1` (bracketed or not).
pub fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") {
        return true;
    }
    host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// The hosts that are not loopback (Unix sockets count as local).
fn remote_hosts(config: &tokio_postgres::Config) -> Result<Vec<String>, TlsError> {
    let hosts = config.get_hosts();
    if hosts.is_empty() {
        return Err(TlsError::NoHost);
    }
    let mut remote = Vec::new();
    for host in hosts {
        match host {
            Host::Tcp(name) if !is_loopback_host(name) => remote.push(name.clone()),
            _ => {}
        }
    }
    Ok(remote)
}

/// Picks the transport for `config` and sets its `sslmode` to match: TLS
/// (`require`, verified) for any remote host or when forced, plain
/// (`disable`) otherwise.
pub fn choose(
    config: &mut tokio_postgres::Config,
    options: &TlsOptions,
) -> Result<Transport, TlsError> {
    let remote = remote_hosts(config)?;
    let disabled = config.get_ssl_mode() == SslMode::Disable;
    if let Some(host) = remote.first()
        && disabled
    {
        return Err(TlsError::PlainToRemote { host: host.clone() });
    }
    let wants_tls =
        !remote.is_empty() || options.force_tls || config.get_ssl_mode() == SslMode::Require;
    if !wants_tls {
        config.ssl_mode(SslMode::Disable);
        return Ok(Transport::Plain);
    }
    if disabled {
        return Err(TlsError::Config(
            "TLS was asked for but the URL says sslmode=disable".to_owned(),
        ));
    }
    config.ssl_mode(SslMode::Require);
    Ok(Transport::Tls(MakeRustls::new(client_config(
        options.ca_file.as_deref(),
    )?)))
}

/// A verifying rustls client configuration: webpki roots plus `ca_file`.
pub fn client_config(ca_file: Option<&Path>) -> Result<Arc<ClientConfig>, TlsError> {
    let mut roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    if let Some(path) = ca_file {
        let shown = path.display().to_string();
        let pem = std::fs::read(path).map_err(|err| TlsError::CaFile {
            path: shown.clone(),
            message: err.to_string(),
        })?;
        add_pem(&mut roots, &pem).map_err(|message| TlsError::CaFile {
            path: shown,
            message,
        })?;
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|err| TlsError::Config(err.to_string()))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// Adds every certificate of a PEM bundle; an empty bundle is an error.
fn add_pem(roots: &mut rustls::RootCertStore, pem: &[u8]) -> Result<(), String> {
    use rustls::pki_types::pem::PemObject;
    let mut added = 0usize;
    for cert in CertificateDer::pem_slice_iter(pem) {
        let cert = cert.map_err(|err| err.to_string())?;
        roots.add(cert).map_err(|err| err.to_string())?;
        added += 1;
    }
    if added == 0 {
        return Err("no certificate in the file".to_owned());
    }
    Ok(())
}

/// tokio-postgres's TLS factory over rustls.
#[derive(Clone)]
pub struct MakeRustls {
    config: Arc<ClientConfig>,
}

impl std::fmt::Debug for MakeRustls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MakeRustls").finish_non_exhaustive()
    }
}

impl MakeRustls {
    pub fn new(config: Arc<ClientConfig>) -> Self {
        Self { config }
    }
}

impl MakeTlsConnect<Socket> for MakeRustls {
    type Stream = RustlsStream;
    type TlsConnect = RustlsConnect;
    type Error = io::Error;

    fn make_tls_connect(&mut self, domain: &str) -> Result<RustlsConnect, io::Error> {
        let domain = domain.trim_start_matches('[').trim_end_matches(']');
        let name = ServerName::try_from(domain.to_owned())
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))?;
        Ok(RustlsConnect {
            config: self.config.clone(),
            name,
        })
    }
}

/// One TLS handshake.
pub struct RustlsConnect {
    config: Arc<ClientConfig>,
    name: ServerName<'static>,
}

impl std::fmt::Debug for RustlsConnect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RustlsConnect")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl TlsConnect<Socket> for RustlsConnect {
    type Stream = RustlsStream;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = io::Result<RustlsStream>> + Send>>;

    fn connect(self, stream: Socket) -> Self::Future {
        Box::pin(async move {
            let tls = tokio_rustls::TlsConnector::from(self.config)
                .connect(self.name, stream)
                .await?;
            Ok(RustlsStream(Box::new(tls)))
        })
    }
}

/// A TLS session over the connection's socket.
pub struct RustlsStream(Box<tokio_rustls::client::TlsStream<Socket>>);

impl std::fmt::Debug for RustlsStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RustlsStream").finish_non_exhaustive()
    }
}

impl AsyncRead for RustlsStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(self.get_mut().0.as_mut()).poll_read(cx, buf)
    }
}

impl AsyncWrite for RustlsStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(self.get_mut().0.as_mut()).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(self.get_mut().0.as_mut()).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(self.get_mut().0.as_mut()).poll_shutdown(cx)
    }
}

impl tokio_postgres::tls::TlsStream for RustlsStream {
    fn channel_binding(&self) -> ChannelBinding {
        ChannelBinding::none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(url: &str) -> tokio_postgres::Config {
        url.parse().unwrap()
    }

    #[test]
    fn loopback_hosts() {
        for host in [
            "localhost",
            "LOCALHOST",
            "db.localhost",
            "127.0.0.1",
            "127.1.2.3",
            "::1",
            "[::1]",
        ] {
            assert!(is_loopback_host(host), "{host}");
        }
        for host in [
            "risingwave",
            "10.0.0.1",
            "100.64.0.7",
            "fd7a:115c::1",
            "localhost.example.com",
            "rw.loams.dev",
        ] {
            assert!(!is_loopback_host(host), "{host}");
        }
    }

    #[test]
    fn loopback_is_plain_by_default() {
        let mut cfg = config("postgres://loams@127.0.0.1:4566/dev");
        assert!(matches!(
            choose(&mut cfg, &TlsOptions::default()),
            Ok(Transport::Plain)
        ));
        assert_eq!(cfg.get_ssl_mode(), SslMode::Disable);
    }

    #[test]
    fn remote_hosts_need_verified_tls() {
        let mut cfg = config("postgres://loams@rw.ns1.eu1.rw.loams.dev:4566/dev");
        assert!(matches!(
            choose(&mut cfg, &TlsOptions::default()),
            Ok(Transport::Tls(_))
        ));
        assert_eq!(
            cfg.get_ssl_mode(),
            SslMode::Require,
            "prefer is upgraded to require"
        );
        let mut plain = config("postgres://loams@rw.loams.dev:4566/dev?sslmode=disable");
        assert!(matches!(
            choose(&mut plain, &TlsOptions::default()),
            Err(TlsError::PlainToRemote { .. })
        ));
    }

    #[test]
    fn a_mix_of_hosts_needs_tls() {
        let mut cfg = config("host=127.0.0.1,rw.loams.dev port=4566 user=loams dbname=dev");
        assert!(matches!(
            choose(&mut cfg, &TlsOptions::default()),
            Ok(Transport::Tls(_))
        ));
    }

    #[test]
    fn loopback_tls_when_asked() {
        let mut forced = config("postgres://loams@localhost:4566/dev");
        let options = TlsOptions {
            force_tls: true,
            ..TlsOptions::default()
        };
        assert!(matches!(
            choose(&mut forced, &options),
            Ok(Transport::Tls(_))
        ));
        let mut required = config("postgres://loams@localhost:4566/dev?sslmode=require");
        assert!(matches!(
            choose(&mut required, &TlsOptions::default()),
            Ok(Transport::Tls(_))
        ));
        let mut contradictory = config("postgres://loams@localhost:4566/dev?sslmode=disable");
        assert!(choose(&mut contradictory, &options).is_err());
    }

    #[test]
    fn ca_files_must_hold_certificates() {
        let mut roots = rustls::RootCertStore::empty();
        assert!(add_pem(&mut roots, b"").is_err());
        assert!(add_pem(&mut roots, b"not a pem").is_err());
        let missing = client_config(Some(Path::new("/nonexistent/loams-ca.pem")));
        assert!(matches!(missing, Err(TlsError::CaFile { .. })));
        assert!(client_config(None).is_ok());
    }

    #[test]
    fn server_names_strip_ipv6_brackets() {
        let mut make = MakeRustls::new(client_config(None).unwrap());
        assert!(make.make_tls_connect("rw.loams.dev").is_ok());
        assert!(make.make_tls_connect("[fd7a:115c::1]").is_ok());
        assert!(make.make_tls_connect("not a name!").is_err());
    }
}
