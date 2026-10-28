//! TLS configs (design §57 §5, §7.3): server configs for the terminate
//! path, one per certificate, chosen by name; the client config that
//! verifies PgDog's certificate.

use std::sync::Arc;

use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ClientConfig, RootCertStore, ServerConfig};

use crate::config::CertConfig;

/// The ALPN protocol of Postgres's direct TLS.
pub const ALPN_POSTGRESQL: &[u8] = b"postgresql";

#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    #[error("tls: reading {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("tls: {0}")]
    Pem(String),
    #[error("tls: {0}")]
    Rustls(#[from] rustls::Error),
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Server configs for the terminate path, each with the names it serves
/// and its leaf certificate.
///
/// The terminate path presents **the certificate PgDog presents**: libpq
/// uses SCRAM-SHA-256-PLUS over TLS, whose `tls-server-end-point` binding is
/// a hash of the server's certificate, so PgDog refuses the password when
/// the client saw another certificate (§57 §7.3).
#[derive(Debug, Clone, Default)]
pub struct ServerCerts {
    entries: Vec<ServerCert>,
}

#[derive(Debug, Clone)]
struct ServerCert {
    names: Vec<String>,
    leaf: CertificateDer<'static>,
    config: Arc<ServerConfig>,
}

impl ServerCerts {
    /// Loads every configured certificate from its PEM files.
    pub fn load(certs: &[CertConfig]) -> Result<Self, TlsError> {
        let mut out = Self::default();
        for c in certs {
            let cert = read(&c.cert)?;
            let key = read(&c.key)?;
            out.add(&c.names, &cert, &key)?;
        }
        Ok(out)
    }

    /// Adds one certificate chain and key (PEM) for `names`.
    pub fn add(
        &mut self,
        names: &[String],
        cert_pem: &[u8],
        key_pem: &[u8],
    ) -> Result<(), TlsError> {
        let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cert_pem)
            .collect::<Result<_, _>>()
            .map_err(|e| TlsError::Pem(e.to_string()))?;
        if chain.is_empty() {
            return Err(TlsError::Pem("no certificate in the PEM".into()));
        }
        let leaf = chain[0].clone();
        let key =
            PrivateKeyDer::from_pem_slice(key_pem).map_err(|e| TlsError::Pem(e.to_string()))?;
        let mut config = ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()?
            .with_no_client_auth()
            .with_single_cert(chain, key)?;
        // Direct TLS requires ALPN `postgresql` (Postgres 17); a client that
        // offers other protocols only is refused by rustls, and one that
        // offers none (an `SSLRequest` client of an old libpq) is served.
        config.alpn_protocols = vec![ALPN_POSTGRESQL.to_vec()];
        let names = names.iter().map(|n| n.to_ascii_lowercase()).collect();
        self.entries.push(ServerCert {
            names,
            leaf,
            config: Arc::new(config),
        });
        Ok(())
    }

    /// Whether no certificate is configured (the terminate path is off).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether some certificate's names cover `host`: an exact name or a
    /// one-label wildcard (`*.eu1.pg.example.com`).
    pub fn covers(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        self.entries.iter().any(|e| {
            e.names
                .iter()
                .any(|n| *n == host || wildcard_matches(n, &host))
        })
    }

    /// The config of the first certificate whose names cover `host`; with
    /// an empty `host` (a dedicated port, no SNI), the first certificate.
    pub fn config_for_name(&self, host: &str) -> Option<Arc<ServerConfig>> {
        if host.is_empty() {
            return self.entries.first().map(|e| e.config.clone());
        }
        let host = host.to_ascii_lowercase();
        self.entries
            .iter()
            .find(|e| {
                e.names
                    .iter()
                    .any(|n| *n == host || wildcard_matches(n, &host))
            })
            .map(|e| e.config.clone())
    }

    /// The config whose leaf certificate is `leaf` (PgDog's, as PgDog
    /// presented it).
    pub fn config_for_leaf(&self, leaf: &[u8]) -> Option<Arc<ServerConfig>> {
        self.entries
            .iter()
            .find(|e| e.leaf.as_ref() == leaf)
            .map(|e| e.config.clone())
    }
}

/// `*.a.b` matches `x.a.b` and nothing with more or fewer labels.
fn wildcard_matches(pattern: &str, host: &str) -> bool {
    let Some(suffix) = pattern.strip_prefix("*.") else {
        return false;
    };
    host.split_once('.')
        .is_some_and(|(label, rest)| !label.is_empty() && rest == suffix)
}

/// The client config that verifies PgDog's certificate against `ca_pem`.
pub fn upstream_client_config(ca_pem: &[u8]) -> Result<Arc<ClientConfig>, TlsError> {
    let mut roots = RootCertStore::empty();
    for ca in CertificateDer::pem_slice_iter(ca_pem) {
        roots.add(ca.map_err(|e| TlsError::Pem(e.to_string()))?)?;
    }
    if roots.is_empty() {
        return Err(TlsError::Pem(
            "no CA certificate in the upstream CA PEM".into(),
        ));
    }
    let config = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// Reads a PEM file.
pub fn read(path: &std::path::Path) -> Result<Vec<u8>, TlsError> {
    std::fs::read(path).map_err(|source| TlsError::Read {
        path: path.display().to_string(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::wildcard_matches;

    #[test]
    fn wildcard_covers_one_label() {
        assert!(wildcard_matches("*.eu1.pg.x", "acme.eu1.pg.x"));
        assert!(!wildcard_matches("*.eu1.pg.x", "a.b.eu1.pg.x"));
        assert!(!wildcard_matches("*.eu1.pg.x", "eu1.pg.x"));
        assert!(!wildcard_matches("acme.eu1.pg.x", "acme.eu1.pg.x"));
    }
}
