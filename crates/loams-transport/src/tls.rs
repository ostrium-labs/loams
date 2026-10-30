//! `rustls` configurations for internal hops (design §83 §5, §6, §10.1).
//!
//! Both sides use TLS 1.3 only, aws-lc-rs as the crypto provider (D2306),
//! and no 0-RTT (D2318). A listener requires a client certificate that chains
//! to the cluster's trust bundle (D2303), and so does a client for the server.
//! The same configuration serves QUIC (`quinn` wraps it) and TCP
//! (`tokio-rustls`); only the ALPN differs (D2304).

use std::sync::Arc;

use rustls::client::WebPkiServerVerifier;
use rustls::crypto::aws_lc_rs;
use rustls::server::WebPkiClientVerifier;
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use rustls_pki_types::{CertificateDer, PrivateKeyDer};

/// The application protocol of an internal connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alpn {
    /// HTTP/3 over QUIC (the default transport).
    H3,
    /// HTTP/2 over TLS (the fallback, and the default where measured better).
    H2,
}

impl Alpn {
    pub fn id(self) -> &'static [u8] {
        match self {
            Alpn::H3 => b"h3",
            Alpn::H2 => b"h2",
        }
    }
}

/// A configuration that could not be built.
#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    #[error("the trust bundle is empty")]
    EmptyBundle,
    #[error("a trust-bundle certificate was refused: {0}")]
    Bundle(rustls::Error),
    #[error("verifier: {0}")]
    Verifier(String),
    #[error("certificate or key: {0}")]
    Rustls(#[from] rustls::Error),
}

fn roots(bundle: &[CertificateDer<'static>]) -> Result<Arc<RootCertStore>, TlsError> {
    if bundle.is_empty() {
        return Err(TlsError::EmptyBundle);
    }
    let mut store = RootCertStore::empty();
    for cert in bundle {
        store.add(cert.clone()).map_err(TlsError::Bundle)?;
    }
    Ok(Arc::new(store))
}

/// An internal listener's configuration: this process's chain and key, and
/// client certificates required from `bundle`.
pub fn server_config(
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
    bundle: &[CertificateDer<'static>],
    alpn: Alpn,
) -> Result<Arc<ServerConfig>, TlsError> {
    let provider = Arc::new(aws_lc_rs::default_provider());
    let verifier = WebPkiClientVerifier::builder_with_provider(roots(bundle)?, provider.clone())
        .build()
        .map_err(|e| TlsError::Verifier(e.to_string()))?;
    let mut cfg = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(verifier)
        .with_single_cert(chain, key)?;
    cfg.alpn_protocols = vec![alpn.id().to_vec()];
    cfg.max_early_data_size = 0;
    Ok(Arc::new(cfg))
}

/// An internal client's configuration: this process's chain and key as its
/// client certificate, and servers verified against `bundle`.
pub fn client_config(
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
    bundle: &[CertificateDer<'static>],
    alpn: Alpn,
) -> Result<Arc<ClientConfig>, TlsError> {
    let provider = Arc::new(aws_lc_rs::default_provider());
    let verifier = WebPkiServerVerifier::builder_with_provider(roots(bundle)?, provider.clone())
        .build()
        .map_err(|e| TlsError::Verifier(e.to_string()))?;
    let mut cfg = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_webpki_verifier(verifier)
        .with_client_auth_cert(chain, key)?;
    cfg.alpn_protocols = vec![alpn.id().to_vec()];
    cfg.enable_early_data = false;
    Ok(Arc::new(cfg))
}
