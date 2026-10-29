//! TLS for the transfer transport (§85 §10, D2438): TLS 1.3 only, on an
//! explicit aws-lc-rs provider (D2306), 0-RTT off (D2318).
//!
//! The server presents its certificate chain. The client trusts, per server,
//! a CA bundle, the system roots, or (for a dev engine) one pinned
//! certificate by SHA-256. Client authentication is a bearer token in v1
//! (Q1512), not a client certificate.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, RootCertStore, SignatureScheme};
use sha2::{Digest, Sha256};

/// ALPN for HTTP/3.
pub const ALPN_H3: &[u8] = b"h3";
/// ALPN for HTTP/2.
pub const ALPN_H2: &[u8] = b"h2";
/// ALPN for HTTP/1.1 (accepted by the TCP listener for plain tools).
pub const ALPN_H1: &[u8] = b"http/1.1";

/// The crypto provider every config here uses, independent of any process
/// default another crate may have installed.
pub fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

fn tls_err(e: impl fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, e.to_string())
}

/// A server's certificate chain and key.
#[derive(Debug)]
pub struct ServerIdentity {
    pub chain: Vec<CertificateDer<'static>>,
    pub key: PrivateKeyDer<'static>,
}

impl Clone for ServerIdentity {
    fn clone(&self) -> Self {
        ServerIdentity {
            chain: self.chain.clone(),
            key: self.key.clone_key(),
        }
    }
}

impl ServerIdentity {
    /// Reads a PEM chain and key.
    pub fn from_pem_files(cert: &Path, key: &Path) -> io::Result<Self> {
        let chain = CertificateDer::pem_file_iter(cert)
            .map_err(tls_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(tls_err)?;
        if chain.is_empty() {
            return Err(tls_err(format!("{} holds no certificate", cert.display())));
        }
        let key = PrivateKeyDer::from_pem_file(key).map_err(tls_err)?;
        Ok(ServerIdentity { chain, key })
    }

    /// A self-signed certificate for `names` (DNS names or IP addresses),
    /// for dev servers; clients pin it by [`ServerIdentity::fingerprint`].
    /// Also returns the certificate's and the key's PEM.
    pub fn self_signed(names: &[String]) -> io::Result<(Self, String, String)> {
        let ck = rcgen::generate_simple_self_signed(names.to_vec()).map_err(tls_err)?;
        let id = ServerIdentity {
            chain: vec![ck.cert.der().clone()],
            key: PrivateKeyDer::try_from(ck.signing_key.serialize_der()).map_err(tls_err)?,
        };
        Ok((id, ck.cert.pem(), ck.signing_key.serialize_pem()))
    }

    /// The leaf certificate's SHA-256, lowercase hex: what a client pins.
    pub fn fingerprint(&self) -> String {
        self.chain
            .first()
            .map(|c| hex::encode(Sha256::digest(c.as_ref())))
            .unwrap_or_default()
    }
}

/// The server's TLS config for one ALPN set.
pub fn server_config(id: &ServerIdentity, alpn: &[&[u8]]) -> io::Result<rustls::ServerConfig> {
    let id = id.clone();
    let mut cfg = rustls::ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(tls_err)?
        .with_no_client_auth()
        .with_single_cert(id.chain, id.key)
        .map_err(tls_err)?;
    cfg.alpn_protocols = alpn.iter().map(|a| a.to_vec()).collect();
    cfg.max_early_data_size = 0;
    Ok(cfg)
}

/// What a client trusts for one server (D2438).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trust {
    /// The operating system's roots.
    System,
    /// A PEM bundle of roots (the server's or the deployment's CA).
    CaFile(PathBuf),
    /// Exactly the certificate with this SHA-256 (lowercase hex). Dev only.
    Pin(String),
}

/// The client's TLS config for one ALPN.
pub fn client_config(trust: &Trust, alpn: &[u8]) -> io::Result<rustls::ClientConfig> {
    let builder = rustls::ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(tls_err)?;
    let mut cfg = match trust {
        Trust::System => {
            let mut roots = RootCertStore::empty();
            let found = rustls_native_certs::load_native_certs();
            for c in found.certs {
                let _ = roots.add(c);
            }
            if roots.is_empty() {
                return Err(tls_err("no system root certificates found"));
            }
            builder.with_root_certificates(roots).with_no_client_auth()
        }
        Trust::CaFile(path) => {
            let mut roots = RootCertStore::empty();
            for c in CertificateDer::pem_file_iter(path).map_err(tls_err)? {
                roots.add(c.map_err(tls_err)?).map_err(tls_err)?;
            }
            builder.with_root_certificates(roots).with_no_client_auth()
        }
        Trust::Pin(sha) => {
            let pin = sha.to_ascii_lowercase();
            if pin.len() != 64 || !pin.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(tls_err("a certificate pin is a SHA-256 in hex"));
            }
            builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(PinVerifier {
                    pin,
                    algs: provider().signature_verification_algorithms,
                }))
                .with_no_client_auth()
        }
    };
    cfg.alpn_protocols = vec![alpn.to_vec()];
    cfg.enable_early_data = false;
    Ok(cfg)
}

/// Accepts exactly one certificate, by its SHA-256, and still checks that
/// the peer holds its key (the handshake signature).
#[derive(Debug)]
struct PinVerifier {
    pin: String,
    algs: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if hex::encode(Sha256::digest(end_entity.as_ref())) == self.pin {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(
                "the server certificate does not match the pinned SHA-256".into(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algs)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

/// The TLS server name for a host from a URL.
pub fn server_name(host: &str) -> io::Result<ServerName<'static>> {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    ServerName::try_from(h.to_string()).map_err(tls_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_signed_roundtrips_through_pem() {
        let (id, cert, key) = ServerIdentity::self_signed(&["localhost".into()]).expect("cert");
        let dir = tempfile::tempdir().expect("dir");
        let (c, k) = (dir.path().join("c.pem"), dir.path().join("k.pem"));
        std::fs::write(&c, cert).expect("write");
        std::fs::write(&k, key).expect("write");
        let back = ServerIdentity::from_pem_files(&c, &k).expect("read");
        assert_eq!(back.fingerprint(), id.fingerprint());
        server_config(&back, &[ALPN_H2]).expect("server config");
    }

    #[test]
    fn bad_pin_refused() {
        assert!(client_config(&Trust::Pin("xyz".into()), ALPN_H3).is_err());
    }
}
