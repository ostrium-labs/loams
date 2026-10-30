//! The verified peer's identities (design §83 §7, D2315).
//!
//! The TLS layer has already verified the chain against the cluster's trust
//! bundle when these functions run: on QUIC from
//! `quinn::Connection::peer_identity()`, on TCP from
//! `rustls::ServerConnection::peer_certificates()`. They only read the
//! leaf's URI SANs. A node certificate carries one `node/` ID and one
//! `role/` ID per role it runs (D2308), so a peer can have several.

use rustls_pki_types::CertificateDer;
use x509_parser::extensions::GeneralName;

use crate::spiffe::{Kind, SpiffeId};

/// The SPIFFE IDs in a verified peer's leaf certificate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerIdentity {
    ids: Vec<SpiffeId>,
}

/// Why a verified certificate yields no usable identity.
#[derive(Debug, thiserror::Error)]
pub enum PeerError {
    #[error("the peer presented no certificate")]
    NoCertificate,
    #[error("the peer's certificate does not parse: {0}")]
    Parse(String),
    #[error("the peer's certificate has no spiffe:// URI SAN")]
    NoSpiffeId,
    #[error("the peer's certificate names more than one cluster")]
    MixedClusters,
}

impl PeerIdentity {
    /// Every ID, in certificate order.
    pub fn ids(&self) -> &[SpiffeId] {
        &self.ids
    }

    /// The cluster the IDs belong to (all of them share it).
    pub fn cluster(&self) -> &str {
        self.ids[0].cluster()
    }

    /// The `node/` ID, if the peer is a cluster node.
    pub fn node(&self) -> Option<&SpiffeId> {
        self.ids.iter().find(|id| id.kind() == Kind::Node)
    }

    /// Whether the peer holds `short` (`role/gateway`, `svc/meter-agent`,
    /// `node/3`), the form route policies name.
    pub fn has(&self, short: &str) -> bool {
        self.ids.iter().any(|id| id.short() == short)
    }
}

/// The identities of a verified chain's leaf (the first certificate).
pub fn peer_identity(chain: &[CertificateDer<'_>]) -> Result<PeerIdentity, PeerError> {
    let leaf = chain.first().ok_or(PeerError::NoCertificate)?;
    let (_, cert) = x509_parser::parse_x509_certificate(leaf.as_ref())
        .map_err(|e| PeerError::Parse(e.to_string()))?;
    let san = cert
        .subject_alternative_name()
        .map_err(|e| PeerError::Parse(e.to_string()))?
        .ok_or(PeerError::NoSpiffeId)?;
    let ids: Vec<SpiffeId> = san
        .value
        .general_names
        .iter()
        .filter_map(|name| match name {
            GeneralName::URI(uri) if uri.starts_with("spiffe://") => uri.parse().ok(),
            _ => None,
        })
        .collect();
    let first = ids.first().ok_or(PeerError::NoSpiffeId)?;
    if ids.iter().any(|id| id.cluster() != first.cluster()) {
        return Err(PeerError::MixedClusters);
    }
    Ok(PeerIdentity { ids })
}

/// A verified peer as a listener hands it to the routes it serves: the
/// leaf's identities and its serial, the two things the deny list and the CA
/// service's `Renew` check (§83 §6.3, §6.5). The TLS layer inserts it into
/// each request's extensions after the handshake verified the chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedPeer {
    pub identity: PeerIdentity,
    /// The leaf's serial as the certificate encodes it (R3.4).
    pub serial: Vec<u8>,
}

/// The [`VerifiedPeer`] of a verified chain's leaf.
pub fn verified_peer(chain: &[CertificateDer<'_>]) -> Result<VerifiedPeer, PeerError> {
    let identity = peer_identity(chain)?;
    let leaf = chain.first().ok_or(PeerError::NoCertificate)?;
    let (_, cert) = x509_parser::parse_x509_certificate(leaf.as_ref())
        .map_err(|e| PeerError::Parse(e.to_string()))?;
    Ok(VerifiedPeer {
        identity,
        serial: cert.raw_serial().to_vec(),
    })
}
