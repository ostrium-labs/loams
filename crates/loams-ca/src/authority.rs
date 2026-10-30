//! The CA's operations over its document (design §83 §6.2–§6.6).
//!
//! Each write loads the document, changes it and stores it with a version
//! compare-and-swap, retrying from a fresh load when another writer moved it
//! first. A refusal changes nothing: a CSR that is refused does not spend the
//! join token it came with. A leaf signed in an attempt that lost the race is
//! discarded, never returned, and its serial is not reused because the next
//! attempt reads the winner's `next_serial`.

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use loams_transport::ca::{BACKDATE, INTERMEDIATE_LIFETIME, LEAF_LIFETIME, ROOT_LIFETIME};
use loams_transport::{
    Ca, CaMaterial, DenyList, Grant, JoinToken, Kind, SpiffeId, TokenError, VerifiedPeer,
};
use rustls_pki_types::CertificateDer;
use rustls_pki_types::pem::PemObject;
use serde::Serialize;

use crate::doc::{CaDoc, DeniedSerial, FORMAT, IntermediateRec, IssuedRec, TokenRec, secs};
use crate::error::CaServiceError;
use crate::seal::KeySealer;
use crate::store::CaDocStore;

/// How long a rotated-out intermediate stays in the trust bundle (§83 §6.6).
pub const RETIRING_INTERMEDIATE_KEPT: Duration = Duration::from_secs(30 * 24 * 3600);
/// How many compare-and-swap attempts a write makes before it gives up.
const ATTEMPTS: usize = 16;
/// How far ahead `status` warns of an expiring root or intermediate (§83
/// §6.6, D2329's alarm).
pub const EXPIRY_WARNING: Duration = Duration::from_secs(30 * 24 * 3600);

/// The root's private key, handed to the operator once by `init` for the
/// deployment's secret store. `Debug` redacts it.
pub struct RootKey(String);

impl RootKey {
    /// The PEM, for writing to the secret store.
    pub fn expose_pem(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RootKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RootKey(<redacted>)")
    }
}

/// A leaf issued by `Issue` or `Renew`.
#[derive(Clone, Debug)]
pub struct CertificateIssued {
    /// Leaf first, then the intermediate.
    pub chain: Vec<CertificateDer<'static>>,
    pub serial: Vec<u8>,
    pub not_after: SystemTime,
    /// Two-thirds of the leaf's life, backdate included (D2309, R3.3).
    pub renew_at: SystemTime,
}

/// The trust bundle (§83 §6.2).
#[derive(Clone, Debug)]
pub struct Bundle {
    pub roots: Vec<CertificateDer<'static>>,
    /// The active intermediate first, then the retiring ones.
    pub intermediates: Vec<CertificateDer<'static>>,
}

/// What to put on the deny list (D2312).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Revoke {
    /// A leaf, by its serial as the certificate encodes it.
    Serial(Vec<u8>),
    /// Every certificate holding this identity.
    Identity(SpiffeId),
    /// A cluster node: `spiffe://<cluster>/node/<id>`.
    Node(String),
}

/// `loams ca status`.
#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub cluster: String,
    pub root_not_after: u64,
    pub intermediates: Vec<IntermediateStatus>,
    pub next_serial: u64,
    pub tokens_outstanding: usize,
    pub tokens_spent: usize,
    pub issued_live: usize,
    pub denied_serials: Vec<String>,
    pub denied_identities: Vec<String>,
    /// What expires within [`EXPIRY_WARNING`].
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct IntermediateStatus {
    pub created_at: u64,
    pub not_after: u64,
    pub active: bool,
    pub retire_at: Option<u64>,
}

/// The CA over its document.
pub struct Authority {
    store: Arc<dyn CaDocStore>,
    sealer: Result<KeySealer, String>,
    /// The opened active intermediate, by its certificate.
    open: Mutex<Option<(String, Arc<Ca>)>>,
}

impl fmt::Debug for Authority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Authority")
            .field("store", &self.store)
            .finish_non_exhaustive()
    }
}

fn unavailable(e: String) -> CaServiceError {
    CaServiceError::Unavailable(e)
}

fn der(pem: &str) -> Result<CertificateDer<'static>, CaServiceError> {
    CertificateDer::from_pem_slice(pem.as_bytes())
        .map_err(|e| CaServiceError::Internal(format!("a certificate in the CA document: {e}")))
}

impl Authority {
    /// The CA over `store`. `sealer` opens the intermediate's key; a node
    /// without its key file (`Err`) still answers `TrustBundle`, and
    /// refuses to issue with `ca_not_initialized`.
    pub fn new(store: Arc<dyn CaDocStore>, sealer: Result<KeySealer, String>) -> Self {
        Authority {
            store,
            sealer,
            open: Mutex::new(None),
        }
    }

    async fn load(&self) -> Result<(u64, CaDoc), CaServiceError> {
        let (version, bytes) = self
            .store
            .load()
            .await
            .map_err(unavailable)?
            .ok_or(CaServiceError::NotInitialized)?;
        let doc = CaDoc::decode(&bytes).map_err(CaServiceError::Internal)?;
        Ok((version, doc))
    }

    /// Loads, applies `f`, prunes and stores, retrying on a lost race.
    async fn update<T>(
        &self,
        now: SystemTime,
        mut f: impl FnMut(&mut CaDoc) -> Result<T, CaServiceError>,
    ) -> Result<T, CaServiceError> {
        for _ in 0..ATTEMPTS {
            let (version, mut doc) = self.load().await?;
            let out = f(&mut doc)?;
            doc.prune(now);
            if self
                .store
                .save(Some(version), doc.encode())
                .await
                .map_err(unavailable)?
                .is_some()
            {
                return Ok(out);
            }
        }
        Err(CaServiceError::Unavailable(
            "the CA document kept changing under this write".to_string(),
        ))
    }

    fn sealer(&self) -> Result<&KeySealer, CaServiceError> {
        self.sealer
            .as_ref()
            .map_err(|e| CaServiceError::Sealed(e.clone()))
    }

    /// The active intermediate, opened.
    fn ca(&self, doc: &CaDoc) -> Result<Arc<Ca>, CaServiceError> {
        let active = doc.active();
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((cert, ca)) = open.as_ref()
            && *cert == active.cert_pem
        {
            return Ok(ca.clone());
        }
        let key = self
            .sealer()?
            .open(&active.key_sealed)
            .map_err(CaServiceError::Sealed)?;
        let material = CaMaterial {
            root_cert_pem: doc.root_cert_pem.clone(),
            root_key_pem: String::new(),
            intermediate_cert_pem: active.cert_pem.clone(),
            intermediate_key_pem: key,
        };
        let ca = Arc::new(
            Ca::load(&doc.cluster, &material)
                .map_err(|e| CaServiceError::Internal(format!("opening the CA: {e}")))?,
        );
        *open = Some((active.cert_pem.clone(), ca.clone()));
        Ok(ca)
    }

    /// Makes the root and the first intermediate for `cluster` and stores
    /// the document (§83 §6.4 step 1). Refused when the deployment already
    /// has a CA. The root's key is returned for the secret store and is not
    /// kept anywhere else.
    pub async fn init(&self, cluster: &str, now: SystemTime) -> Result<RootKey, CaServiceError> {
        if self.store.load().await.map_err(unavailable)?.is_some() {
            return Err(CaServiceError::AlreadyInitialized);
        }
        let material = Ca::init(cluster, now).map_err(|e| match e {
            loams_transport::CaError::WrongCluster(c) => CaServiceError::Invalid {
                field: "cluster",
                message: format!("{c:?} is not a SPIFFE trust domain"),
            },
            other => CaServiceError::from(other),
        })?;
        let key_sealed = self
            .sealer()?
            .seal(&material.intermediate_key_pem)
            .map_err(CaServiceError::Internal)?;
        let doc = CaDoc {
            format: FORMAT,
            cluster: cluster.to_string(),
            root_cert_pem: material.root_cert_pem.clone(),
            root_not_after: secs(now + ROOT_LIFETIME),
            intermediates: vec![IntermediateRec {
                cert_pem: material.intermediate_cert_pem.clone(),
                key_sealed,
                created_at: secs(now),
                not_after: secs(now + INTERMEDIATE_LIFETIME),
                retire_at: None,
            }],
            next_serial: 1,
            tokens: Vec::new(),
            issued: Vec::new(),
            deny: Default::default(),
        };
        match self
            .store
            .save(None, doc.encode())
            .await
            .map_err(unavailable)?
        {
            Some(_) => Ok(RootKey(material.root_key_pem)),
            None => Err(CaServiceError::AlreadyInitialized),
        }
    }

    /// The deployment's cluster id (the SPIFFE trust domain).
    pub async fn cluster(&self) -> Result<String, CaServiceError> {
        Ok(self.load().await?.1.cluster)
    }

    /// A join token for `grant` (§83 §6.4). Every identity must be in this
    /// CA's cluster and off the deny list.
    pub async fn create_token(
        &self,
        grant: Vec<SpiffeId>,
        now: SystemTime,
    ) -> Result<JoinToken, CaServiceError> {
        if grant.is_empty() {
            return Err(CaServiceError::Invalid {
                field: "grant",
                message: "a token grants at least one identity".to_string(),
            });
        }
        let token = JoinToken::generate().map_err(CaServiceError::Token)?;
        let record = token.record(Grant { ids: grant.clone() }, now);
        self.update(now, |doc| {
            for id in &grant {
                if id.cluster() != doc.cluster {
                    return Err(CaServiceError::NotGranted(id.to_string()));
                }
                if doc.deny.ids.iter().any(|d| *d == id.to_string()) {
                    return Err(CaServiceError::Revoked(id.to_string()));
                }
            }
            doc.tokens.push(TokenRec::from_record(&record, now));
            Ok(())
        })
        .await?;
        Ok(token)
    }

    /// Adds `target` to the deny list (D2312).
    pub async fn revoke(&self, target: Revoke, now: SystemTime) -> Result<(), CaServiceError> {
        self.update(now, |doc| {
            match &target {
                Revoke::Serial(serial) => {
                    let serial = hex::encode(serial);
                    let until = doc
                        .issued
                        .iter()
                        .find(|i| i.serial == serial)
                        .map_or(secs(now + LEAF_LIFETIME + BACKDATE), |i| i.not_after);
                    if !doc.deny.serials.iter().any(|d| d.serial == serial) {
                        doc.deny.serials.push(DeniedSerial { serial, until });
                    }
                }
                Revoke::Identity(id) => {
                    if id.cluster() != doc.cluster {
                        return Err(CaServiceError::Invalid {
                            field: "identity",
                            message: format!("{id} is not in cluster {}", doc.cluster),
                        });
                    }
                    let id = id.to_string();
                    if !doc.deny.ids.contains(&id) {
                        doc.deny.ids.push(id);
                    }
                }
                Revoke::Node(node) => {
                    let id = SpiffeId::new(&doc.cluster, Kind::Node, node).map_err(|e| {
                        CaServiceError::Invalid {
                            field: "node",
                            message: e.to_string(),
                        }
                    })?;
                    let id = id.to_string();
                    if !doc.deny.ids.contains(&id) {
                        doc.deny.ids.push(id);
                    }
                }
            }
            Ok(())
        })
        .await
    }

    /// A new intermediate signed by the root (§83 §6.6). The old one stops
    /// issuing now and stays in the trust bundle for
    /// [`RETIRING_INTERMEDIATE_KEPT`].
    pub async fn rotate_intermediate(
        &self,
        root_key_pem: &str,
        now: SystemTime,
    ) -> Result<(), CaServiceError> {
        let (_, doc) = self.load().await?;
        let (cert_pem, key_pem) =
            Ca::new_intermediate(&doc.cluster, &doc.root_cert_pem, root_key_pem, now)?;
        let key_sealed = self
            .sealer()?
            .seal(&key_pem)
            .map_err(CaServiceError::Internal)?;
        let retire_at = secs(now + RETIRING_INTERMEDIATE_KEPT);
        self.update(now, |doc| {
            for rec in &mut doc.intermediates {
                if rec.retire_at.is_none() {
                    rec.retire_at = Some(retire_at);
                }
            }
            doc.intermediates.push(IntermediateRec {
                cert_pem: cert_pem.clone(),
                key_sealed: key_sealed.clone(),
                created_at: secs(now),
                not_after: secs(now + INTERMEDIATE_LIFETIME),
                retire_at: None,
            });
            Ok(())
        })
        .await
    }

    fn signed(
        &self,
        doc: &mut CaDoc,
        csr: &[u8],
        grant: &Grant,
        now: SystemTime,
    ) -> Result<CertificateIssued, CaServiceError> {
        let ca = self.ca(doc)?;
        let serial = doc.next_serial;
        let issued = ca.issue(csr, grant, serial, now)?;
        doc.next_serial = serial.saturating_add(1);
        let ids = loams_transport::peer_identity(&issued.chain)
            .map_err(|e| CaServiceError::Internal(format!("the issued leaf: {e}")))?;
        doc.issued.push(IssuedRec {
            serial: hex::encode(&issued.serial),
            ids: ids.ids().iter().map(ToString::to_string).collect(),
            not_after: secs(issued.not_after),
        });
        let not_before = now - BACKDATE;
        let life = issued
            .not_after
            .duration_since(not_before)
            .unwrap_or_default();
        Ok(CertificateIssued {
            renew_at: not_before + life * 2 / 3,
            chain: issued.chain,
            serial: issued.serial,
            not_after: issued.not_after,
        })
    }

    /// `Issue` (§83 §6.3): a first certificate for a joining process, in
    /// exchange for `token`. The CSR may ask only for identities the token
    /// grants, and none that is revoked. A refused request leaves the token
    /// unspent.
    pub async fn issue(
        &self,
        token: &str,
        csr: &[u8],
        now: SystemTime,
    ) -> Result<CertificateIssued, CaServiceError> {
        let token = JoinToken::parse(token).map_err(CaServiceError::Token)?;
        self.update(now, |doc| {
            let rec = doc
                .tokens
                .iter_mut()
                .find(|t| t.id == token.id)
                .ok_or(CaServiceError::Token(TokenError::Unknown))?;
            let mut record = rec.to_record().map_err(CaServiceError::Internal)?;
            let grant = record.redeem(&token, now).map_err(CaServiceError::Token)?;
            rec.spent = record.spent;
            if let Some(what) = doc.denies("", &grant.ids) {
                return Err(CaServiceError::Revoked(what));
            }
            self.signed(doc, csr, &grant, now)
        })
        .await
    }

    /// `Renew` (§83 §6.3): a fresh certificate for `peer`, the caller the
    /// mTLS handshake verified. The CSR may ask only for identities the
    /// caller's certificate holds. Refused for a revoked serial or identity.
    pub async fn renew(
        &self,
        peer: &VerifiedPeer,
        csr: &[u8],
        now: SystemTime,
    ) -> Result<CertificateIssued, CaServiceError> {
        let grant = Grant {
            ids: peer.identity.ids().to_vec(),
        };
        let serial = hex::encode(&peer.serial);
        self.update(now, |doc| {
            if let Some(what) = doc.denies(&serial, &grant.ids) {
                return Err(CaServiceError::Revoked(what));
            }
            self.signed(doc, csr, &grant, now)
        })
        .await
    }

    /// `TrustBundle` (§83 §6.2): the root and the intermediates in service.
    pub async fn bundle(&self, now: SystemTime) -> Result<Bundle, CaServiceError> {
        let (_, doc) = self.load().await?;
        Ok(Bundle {
            roots: vec![der(&doc.root_cert_pem)?],
            intermediates: doc
                .in_service(now)
                .map(|rec| der(&rec.cert_pem))
                .collect::<Result<_, _>>()?,
        })
    }

    /// Replaces `list` with the document's deny list: what a node does when
    /// the document changes (§83 §6.5).
    pub async fn refresh_deny_list(&self, list: &DenyList) -> Result<(), CaServiceError> {
        let (_, doc) = self.load().await?;
        let serials = doc
            .deny
            .serials
            .iter()
            .filter_map(|d| hex::decode(&d.serial).ok())
            .collect::<Vec<_>>();
        let ids = doc
            .deny
            .ids
            .iter()
            .filter_map(|s| s.parse::<SpiffeId>().ok())
            .collect::<Vec<_>>();
        list.replace(serials, ids);
        Ok(())
    }

    /// `loams ca status`.
    pub async fn status(&self, now: SystemTime) -> Result<Status, CaServiceError> {
        let (_, doc) = self.load().await?;
        let n = secs(now);
        let warn_by = secs(now + EXPIRY_WARNING);
        let mut warnings = Vec::new();
        if doc.root_not_after <= warn_by {
            warnings.push(format!(
                "the root expires at {}: rotate it",
                doc.root_not_after
            ));
        }
        if doc.active().not_after <= warn_by {
            warnings.push(format!(
                "the active intermediate expires at {}: run `loams ca rotate intermediate`",
                doc.active().not_after
            ));
        }
        let last = doc.intermediates.len() - 1;
        Ok(Status {
            cluster: doc.cluster.clone(),
            root_not_after: doc.root_not_after,
            intermediates: doc
                .intermediates
                .iter()
                .enumerate()
                .map(|(i, rec)| IntermediateStatus {
                    created_at: rec.created_at,
                    not_after: rec.not_after,
                    active: i == last,
                    retire_at: rec.retire_at,
                })
                .collect(),
            next_serial: doc.next_serial,
            tokens_outstanding: doc
                .tokens
                .iter()
                .filter(|t| !t.spent && t.expires_at > n)
                .count(),
            tokens_spent: doc.tokens.iter().filter(|t| t.spent).count(),
            issued_live: doc.issued.iter().filter(|i| i.not_after > n).count(),
            denied_serials: doc.deny.serials.iter().map(|d| d.serial.clone()).collect(),
            denied_identities: doc.deny.ids.clone(),
            warnings,
        })
    }
}
