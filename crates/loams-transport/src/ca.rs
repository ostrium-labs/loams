//! The deployment's internal CA (design §83 §6.2–§6.3, D2309, D2310).
//!
//! - [`Ca::init`] makes a root (10 years) and an intermediate (1 year), both
//!   ECDSA P-256 (D2306), and hands the PEM material back as [`CaMaterial`].
//!   The caller keeps the root in the deployment's secret store and seals the
//!   intermediate's key at rest (the CA service, a later part of Task 3).
//! - [`Ca::load`] opens a CA from that material; only the intermediate's key
//!   is needed to issue.
//! - [`Ca::issue`] signs a leaf for a CSR (D2311): the requester generated
//!   its key itself, so no private key ever leaves the process that uses it.
//!   The leaf carries only the SPIFFE IDs the [`Grant`] allows, plus the DNS
//!   names and IP addresses the CSR asked for; nothing else from the CSR is
//!   copied (no CA bit, no key usages, no validity). It lives
//!   [`LEAF_LIFETIME`].

use std::fmt;
use std::time::{Duration, SystemTime};

use rcgen::{
    BasicConstraints, CertificateParams, CertificateSigningRequestParams, DistinguishedName,
    DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose, PublicKeyData,
    SanType, SerialNumber,
};
use rustls_pki_types::{CertificateDer, CertificateSigningRequestDer};

use crate::spiffe::SpiffeId;

/// A leaf's life (D2309).
pub const LEAF_LIFETIME: Duration = Duration::from_secs(24 * 3600);
/// The intermediate's life (D2309).
pub const INTERMEDIATE_LIFETIME: Duration = Duration::from_secs(365 * 24 * 3600);
/// The root's life (D2309).
pub const ROOT_LIFETIME: Duration = Duration::from_secs(10 * 365 * 24 * 3600);
/// How far a certificate's `notBefore` is set back, for clock skew between
/// nodes (§83 §15 tests ±5 minutes).
pub const BACKDATE: Duration = Duration::from_secs(5 * 60);

/// The identities a requester may receive: a join token's grant (§83 §6.4),
/// or, on renewal, the identities its current certificate already holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    pub ids: Vec<SpiffeId>,
}

/// A CA's PEM material. `Debug` redacts both keys.
#[derive(Clone)]
pub struct CaMaterial {
    pub root_cert_pem: String,
    pub root_key_pem: String,
    pub intermediate_cert_pem: String,
    pub intermediate_key_pem: String,
}

impl fmt::Debug for CaMaterial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CaMaterial")
            .field("root_cert_pem", &self.root_cert_pem)
            .field("root_key_pem", &"<redacted>")
            .field("intermediate_cert_pem", &self.intermediate_cert_pem)
            .field("intermediate_key_pem", &"<redacted>")
            .finish()
    }
}

/// A leaf the CA issued.
#[derive(Clone, Debug)]
pub struct Issued {
    /// Leaf first, then the intermediate.
    pub chain: Vec<CertificateDer<'static>>,
    /// The leaf's serial number (the deny list's key, §83 §6.5).
    pub serial: Vec<u8>,
    pub not_after: SystemTime,
}

/// Why the CA could not do what was asked.
#[derive(Debug, thiserror::Error)]
pub enum CaError {
    #[error("certificate generation: {0}")]
    Rcgen(#[from] rcgen::Error),
    #[error("the CSR does not parse or its signature does not verify: {0}")]
    Csr(rcgen::Error),
    #[error("the CSR asks for no SPIFFE ID")]
    NoIdentity,
    #[error("the CSR asks for {0}, which the grant does not allow")]
    NotGranted(String),
    #[error("the CSR asks for an identity in cluster {0:?}, not this CA's")]
    WrongCluster(String),
    #[error("the CSR's URI SAN {0:?} is not a Loams SPIFFE ID")]
    BadUri(String),
    #[error("the key is not the root certificate's")]
    KeyMismatch,
}

fn name(cluster: &str, what: &str) -> DistinguishedName {
    let mut dn = DistinguishedName::new();
    dn.push(DnType::OrganizationName, "Loams");
    dn.push(DnType::CommonName, format!("Loams {cluster} {what}"));
    dn
}

fn ca_params(
    cluster: &str,
    what: &str,
    now: SystemTime,
    life: Duration,
    path_len: Option<u8>,
) -> Result<CertificateParams, CaError> {
    let mut p = CertificateParams::new(Vec::<String>::new())?;
    p.distinguished_name = name(cluster, what);
    p.is_ca = IsCa::Ca(match path_len {
        Some(n) => BasicConstraints::Constrained(n),
        None => BasicConstraints::Unconstrained,
    });
    p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    p.not_before = (now - BACKDATE).into();
    p.not_after = (now + life).into();
    p.use_authority_key_identifier_extension = true;
    Ok(p)
}

/// The DER of a PEM certificate.
fn pem_der(pem: &str) -> Result<CertificateDer<'static>, CaError> {
    use rustls_pki_types::pem::PemObject;
    CertificateDer::from_pem_slice(pem.as_bytes())
        .map_err(|e| CaError::Rcgen(rcgen::Error::PemError(e.to_string())))
}

/// An open CA: the root certificate, and the intermediate with its key.
pub struct Ca {
    cluster: String,
    root: CertificateDer<'static>,
    intermediate: CertificateDer<'static>,
    issuer: Issuer<'static, KeyPair>,
}

impl fmt::Debug for Ca {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ca")
            .field("cluster", &self.cluster)
            .finish_non_exhaustive()
    }
}

impl Ca {
    /// A new root and intermediate for `cluster` (a SPIFFE trust domain).
    pub fn init(cluster: &str, now: SystemTime) -> Result<CaMaterial, CaError> {
        // The cluster must be usable as a trust domain.
        SpiffeId::new(cluster, crate::spiffe::Kind::Node, "0")
            .map_err(|_| CaError::WrongCluster(cluster.to_string()))?;
        let root_key = KeyPair::generate()?;
        let root_params = ca_params(cluster, "root", now, ROOT_LIFETIME, None)?;
        let root = root_params.self_signed(&root_key)?;
        let root_issuer = Issuer::new(root_params, &root_key);
        let int_key = KeyPair::generate()?;
        let int_params = ca_params(cluster, "intermediate", now, INTERMEDIATE_LIFETIME, Some(0))?;
        let intermediate = int_params.signed_by(&int_key, &root_issuer)?;
        Ok(CaMaterial {
            root_cert_pem: root.pem(),
            root_key_pem: root_key.serialize_pem(),
            intermediate_cert_pem: intermediate.pem(),
            intermediate_key_pem: int_key.serialize_pem(),
        })
    }

    /// A new intermediate under the root `root_cert_pem`, whose key is
    /// `root_key_pem` (a rotation, §83 §6.6): returns the intermediate's
    /// certificate and key as PEM. The key must be the root certificate's.
    pub fn new_intermediate(
        cluster: &str,
        root_cert_pem: &str,
        root_key_pem: &str,
        now: SystemTime,
    ) -> Result<(String, String), CaError> {
        let root_key = KeyPair::from_pem(root_key_pem)?;
        let root_der = pem_der(root_cert_pem)?;
        let (_, root) = x509_parser::parse_x509_certificate(root_der.as_ref())
            .map_err(|_| CaError::Rcgen(rcgen::Error::CouldNotParseCertificate))?;
        if root.public_key().raw != root_key.subject_public_key_info().as_slice() {
            return Err(CaError::KeyMismatch);
        }
        let root_issuer = Issuer::from_ca_cert_der(&root_der, &root_key)?;
        let int_key = KeyPair::generate()?;
        let int_params = ca_params(cluster, "intermediate", now, INTERMEDIATE_LIFETIME, Some(0))?;
        let intermediate = int_params.signed_by(&int_key, &root_issuer)?;
        Ok((intermediate.pem(), int_key.serialize_pem()))
    }

    /// Opens the CA that `material` describes. The root's key is not used.
    pub fn load(cluster: &str, material: &CaMaterial) -> Result<Ca, CaError> {
        let key = KeyPair::from_pem(&material.intermediate_key_pem)?;
        let issuer = Issuer::from_ca_cert_pem(&material.intermediate_cert_pem, key)?;
        Ok(Ca {
            cluster: cluster.to_string(),
            root: pem_der(&material.root_cert_pem)?,
            intermediate: pem_der(&material.intermediate_cert_pem)?,
            issuer,
        })
    }

    pub fn cluster(&self) -> &str {
        &self.cluster
    }

    /// The trust bundle peers verify against: the root (§83 §6.2).
    pub fn bundle(&self) -> Vec<CertificateDer<'static>> {
        vec![self.root.clone()]
    }

    /// Signs a leaf for `csr` with the identities `grant` allows (D2311).
    /// Every SPIFFE ID the CSR asks for must be in the grant and in this
    /// CA's cluster; DNS names and IP addresses are copied as asked.
    pub fn issue(
        &self,
        csr: &[u8],
        grant: &Grant,
        serial: u64,
        now: SystemTime,
    ) -> Result<Issued, CaError> {
        let req = CertificateSigningRequestParams::from_der(&CertificateSigningRequestDer::from(
            csr.to_vec(),
        ))
        .map_err(CaError::Csr)?;
        let mut sans = Vec::new();
        let mut ids = 0usize;
        for san in &req.params.subject_alt_names {
            match san {
                SanType::URI(uri) => {
                    let id: SpiffeId = uri
                        .as_str()
                        .parse()
                        .map_err(|_| CaError::BadUri(uri.as_str().to_string()))?;
                    if id.cluster() != self.cluster {
                        return Err(CaError::WrongCluster(id.cluster().to_string()));
                    }
                    if !grant.ids.contains(&id) {
                        return Err(CaError::NotGranted(id.to_string()));
                    }
                    ids += 1;
                    sans.push(san.clone());
                }
                SanType::DnsName(_) | SanType::IpAddress(_) => sans.push(san.clone()),
                // Email, other names: not part of a workload identity.
                _ => {}
            }
        }
        if ids == 0 {
            return Err(CaError::NoIdentity);
        }
        let mut p = CertificateParams::new(Vec::<String>::new())?;
        p.subject_alt_names = sans;
        p.distinguished_name = DistinguishedName::new();
        p.is_ca = IsCa::ExplicitNoCa;
        p.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        p.extended_key_usages = vec![
            ExtendedKeyUsagePurpose::ServerAuth,
            ExtendedKeyUsagePurpose::ClientAuth,
        ];
        let not_after = now + LEAF_LIFETIME;
        p.not_before = (now - BACKDATE).into();
        p.not_after = not_after.into();
        let serial_bytes = serial.to_be_bytes().to_vec();
        p.serial_number = Some(SerialNumber::from_slice(&serial_bytes));
        p.use_authority_key_identifier_extension = true;
        let leaf = p.signed_by(&req.public_key, &self.issuer)?;
        // The serial as the certificate encodes it (DER drops leading zero
        // bytes), so it matches what `Identity::serial` reads back.
        let serial = x509_parser::parse_x509_certificate(leaf.der().as_ref())
            .map(|(_, c)| c.raw_serial().to_vec())
            .unwrap_or(serial_bytes);
        Ok(Issued {
            chain: vec![leaf.der().clone(), self.intermediate.clone()],
            serial,
            not_after,
        })
    }
}

/// Makes this process's key and a CSR for `ids` plus `dns_names`. The key
/// stays with the caller (in memory); only the CSR goes to the CA.
pub fn new_key_and_csr(
    ids: &[SpiffeId],
    dns_names: &[String],
) -> Result<(rustls_pki_types::PrivateKeyDer<'static>, Vec<u8>), CaError> {
    let key = KeyPair::generate()?;
    let mut p = CertificateParams::new(dns_names.to_vec())?;
    for id in ids {
        p.subject_alt_names.push(SanType::URI(
            id.to_string()
                .try_into()
                .map_err(|_| CaError::BadUri(id.to_string()))?,
        ));
    }
    let csr = p.serialize_request(&key)?;
    let der = csr.der().to_vec();
    let key_der = rustls_pki_types::PrivateKeyDer::Pkcs8(
        rustls_pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()),
    );
    Ok((key_der, der))
}
