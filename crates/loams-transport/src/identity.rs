//! This process's identity: its certificate chain, its private key, and the
//! cluster's trust bundle (design §83 §6, D2309–D2314).
//!
//! An [`Identity`] is what every internal listener and client is built from:
//! [`Identity::server_config`] and [`Identity::client_config`] wrap
//! [`crate::tls`]'s builders with this process's material. Its leaf lives 24
//! hours (D2309) and is renewed at two-thirds of its life
//! ([`Identity::renew_at`]); whoever holds the identity (the CA client in a
//! later part of CT1 Task 3, or a file watcher for an external CA, D2314)
//! swaps in the renewed one.
//!
//! The private key is never printed: `Debug` shows the identities and the
//! validity window only.

use std::fmt;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rustls::{ClientConfig, ServerConfig};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};

use crate::peer::{PeerError, PeerIdentity, peer_identity};
use crate::tls::{self, Alpn, TlsError};

/// A process's certificate chain, key and trust bundle.
pub struct Identity {
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
    bundle: Vec<CertificateDer<'static>>,
    ids: PeerIdentity,
    not_before: SystemTime,
    not_after: SystemTime,
    serial: Vec<u8>,
}

/// Why an identity could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("{what}: {source}")]
    Pem {
        what: &'static str,
        source: rustls_pki_types::pem::Error,
    },
    #[error("the certificate chain is empty")]
    EmptyChain,
    #[error("the trust bundle is empty")]
    EmptyBundle,
    #[error("the leaf certificate does not parse: {0}")]
    Parse(String),
    #[error(transparent)]
    Peer(#[from] PeerError),
}

fn system_time(t: x509_parser::time::ASN1Time) -> SystemTime {
    let secs = t.timestamp();
    if secs >= 0 {
        UNIX_EPOCH + Duration::from_secs(secs.unsigned_abs())
    } else {
        UNIX_EPOCH - Duration::from_secs(secs.unsigned_abs())
    }
}

impl Identity {
    /// An identity from DER material. The leaf (the chain's first
    /// certificate) must carry at least one SPIFFE ID of one cluster.
    pub fn new(
        chain: Vec<CertificateDer<'static>>,
        key: PrivateKeyDer<'static>,
        bundle: Vec<CertificateDer<'static>>,
    ) -> Result<Self, IdentityError> {
        let leaf = chain.first().ok_or(IdentityError::EmptyChain)?;
        if bundle.is_empty() {
            return Err(IdentityError::EmptyBundle);
        }
        let (_, cert) = x509_parser::parse_x509_certificate(leaf.as_ref())
            .map_err(|e| IdentityError::Parse(e.to_string()))?;
        let validity = cert.validity();
        let not_before = system_time(validity.not_before);
        let not_after = system_time(validity.not_after);
        let serial = cert.raw_serial().to_vec();
        let ids = peer_identity(&chain)?;
        Ok(Identity {
            chain,
            key,
            bundle,
            ids,
            not_before,
            not_after,
            serial,
        })
    }

    /// An identity from PEM files: the chain (leaf first), its key, and the
    /// trust bundle. This is the layout of cert-manager's Secrets and of
    /// SPIRE's helper (`tls.crt`, `tls.key`, `ca.crt`; D2314).
    pub fn from_pem_files(chain: &Path, key: &Path, bundle: &Path) -> Result<Self, IdentityError> {
        let read_certs = |path: &Path, what| {
            CertificateDer::pem_file_iter(path)
                .and_then(|it| it.collect::<Result<Vec<_>, _>>())
                .map_err(|source| IdentityError::Pem { what, source })
        };
        let chain = read_certs(chain, "certificate chain")?;
        let bundle = read_certs(bundle, "trust bundle")?;
        let key = PrivateKeyDer::from_pem_file(key).map_err(|source| IdentityError::Pem {
            what: "private key",
            source,
        })?;
        Identity::new(chain, key, bundle)
    }

    /// The SPIFFE IDs this process holds.
    pub fn ids(&self) -> &PeerIdentity {
        &self.ids
    }

    /// The leaf's serial number, as the CA issued it (the deny list's key).
    pub fn serial(&self) -> &[u8] {
        &self.serial
    }

    pub fn not_before(&self) -> SystemTime {
        self.not_before
    }

    pub fn not_after(&self) -> SystemTime {
        self.not_after
    }

    /// When to renew: two-thirds of the way through the leaf's life (D2309).
    pub fn renew_at(&self) -> SystemTime {
        let life = self
            .not_after
            .duration_since(self.not_before)
            .unwrap_or_default();
        self.not_before + life * 2 / 3
    }

    /// Whether `now` is at or past [`Identity::renew_at`].
    pub fn needs_renewal(&self, now: SystemTime) -> bool {
        now >= self.renew_at()
    }

    /// Whether the leaf is valid at `now`.
    pub fn is_valid_at(&self, now: SystemTime) -> bool {
        now >= self.not_before && now < self.not_after
    }

    /// The trust bundle this identity verifies peers against.
    pub fn bundle(&self) -> &[CertificateDer<'static>] {
        &self.bundle
    }

    /// The certificate chain (leaf first) this process presents.
    pub fn chain(&self) -> &[CertificateDer<'static>] {
        &self.chain
    }

    /// An internal listener's TLS configuration (client certificates
    /// required) for `alpn`.
    pub fn server_config(&self, alpn: Alpn) -> Result<Arc<ServerConfig>, TlsError> {
        tls::server_config(self.chain.clone(), self.key.clone_key(), &self.bundle, alpn)
    }

    /// An internal client's TLS configuration for `alpn`.
    pub fn client_config(&self, alpn: Alpn) -> Result<Arc<ClientConfig>, TlsError> {
        tls::client_config(self.chain.clone(), self.key.clone_key(), &self.bundle, alpn)
    }
}

impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Identity")
            .field("ids", &self.ids.ids())
            .field("not_before", &self.not_before)
            .field("not_after", &self.not_after)
            .field("key", &"<redacted>")
            .finish()
    }
}
