//! The CA's state document (design §83 §6.2–§6.6), JSON.
//!
//! It never holds a private key in plaintext: the root's key is not in it at
//! all (it stays in the deployment's secret store), and each intermediate's
//! key is sealed with age ([`crate::seal`]). Join tokens are kept as their
//! secrets' SHA-256 digests.
//!
//! Every write prunes what can no longer matter ([`CaDoc::prune`]): tokens a
//! day past their expiry, issued serials past their `notAfter`, deny-list
//! serials past the `notAfter` of the leaf they revoke, and intermediates
//! past their retirement.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use loams_transport::{Grant, SpiffeId, TokenRecord};
use serde::{Deserialize, Serialize};

/// The document's format version.
pub const FORMAT: u32 = 1;
/// How long a spent or expired token's record is kept, so a second use
/// answers `join_token_spent` or `join_token_expired` rather than
/// `join_token_unknown`.
pub const TOKEN_RECORD_KEPT: Duration = Duration::from_secs(24 * 3600);

/// Seconds since the epoch.
pub fn secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The time `s` seconds after the epoch.
pub fn time(s: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(s)
}

/// The CA's state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaDoc {
    pub format: u32,
    /// The SPIFFE trust domain: the deployment's cluster id (D2308).
    pub cluster: String,
    pub root_cert_pem: String,
    pub root_not_after: u64,
    /// Oldest first; the last one issues (the active one).
    pub intermediates: Vec<IntermediateRec>,
    /// The next leaf serial (R3.4).
    pub next_serial: u64,
    #[serde(default)]
    pub tokens: Vec<TokenRec>,
    #[serde(default)]
    pub issued: Vec<IssuedRec>,
    #[serde(default)]
    pub deny: DenyRec,
}

/// An intermediate and its sealed key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntermediateRec {
    pub cert_pem: String,
    /// `age:<base64>` ([`crate::seal::KeySealer`]).
    pub key_sealed: String,
    pub created_at: u64,
    pub not_after: u64,
    /// When a rotation retires it: until then it stays in the trust bundle
    /// (§83 §6.6, 30 days). `None` for the active one.
    #[serde(default)]
    pub retire_at: Option<u64>,
}

/// A join token's record: the secret's digest, never the secret.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenRec {
    pub id: String,
    /// Hex SHA-256 of the secret.
    pub secret_sha256: String,
    pub grant: Vec<String>,
    pub created_at: u64,
    pub expires_at: u64,
    pub spent: bool,
}

/// A leaf the CA issued that has not expired yet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssuedRec {
    /// Hex, as the certificate encodes it (R3.4).
    pub serial: String,
    pub ids: Vec<String>,
    pub not_after: u64,
}

/// The deny list (D2312).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DenyRec {
    #[serde(default)]
    pub serials: Vec<DeniedSerial>,
    /// SPIFFE IDs, refused until removed by hand.
    #[serde(default)]
    pub ids: Vec<String>,
}

/// A revoked serial, kept until the leaf it names has expired.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeniedSerial {
    pub serial: String,
    pub until: u64,
}

impl CaDoc {
    pub fn decode(bytes: &[u8]) -> Result<CaDoc, String> {
        let doc: CaDoc =
            serde_json::from_slice(bytes).map_err(|e| format!("the CA document: {e}"))?;
        if doc.format != FORMAT {
            return Err(format!(
                "the CA document has format {}, this binary reads {FORMAT}",
                doc.format
            ));
        }
        if doc.intermediates.is_empty() {
            return Err("the CA document has no intermediate".to_string());
        }
        Ok(doc)
    }

    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// The intermediate that issues.
    pub fn active(&self) -> &IntermediateRec {
        // `decode` refuses a document without one.
        &self.intermediates[self.intermediates.len() - 1]
    }

    /// The intermediates in the trust bundle at `now`: the active one first,
    /// then the retiring ones that have not reached their retirement.
    pub fn in_service(&self, now: SystemTime) -> impl Iterator<Item = &IntermediateRec> {
        let now = secs(now);
        self.intermediates
            .iter()
            .rev()
            .enumerate()
            .filter(move |(i, rec)| *i == 0 || rec.retire_at.is_none_or(|r| r > now))
            .map(|(_, rec)| rec)
    }

    /// Whether a peer with `serial` (hex) or any of `ids` is denied.
    pub fn denies(&self, serial_hex: &str, ids: &[SpiffeId]) -> Option<String> {
        if self.deny.serials.iter().any(|d| d.serial == serial_hex) {
            return Some(format!("certificate serial {serial_hex}"));
        }
        ids.iter()
            .find(|id| self.deny.ids.iter().any(|d| *d == id.to_string()))
            .map(ToString::to_string)
    }

    /// Drops what can no longer matter at `now`.
    pub fn prune(&mut self, now: SystemTime) {
        let n = secs(now);
        let kept = TOKEN_RECORD_KEPT.as_secs();
        self.tokens
            .retain(|t| t.expires_at.saturating_add(kept) > n);
        self.issued.retain(|i| i.not_after > n);
        self.deny.serials.retain(|d| d.until > n);
        let last = self.intermediates.len().saturating_sub(1);
        let mut i = 0;
        self.intermediates.retain(|rec| {
            let keep = i == last || rec.retire_at.is_none_or(|r| r > n);
            i += 1;
            keep
        });
    }
}

impl TokenRec {
    /// The record as `loams_transport` redeems it.
    pub fn to_record(&self) -> Result<TokenRecord, String> {
        let digest = hex::decode(&self.secret_sha256)
            .ok()
            .and_then(|d| <[u8; 32]>::try_from(d).ok())
            .ok_or_else(|| format!("token {}: a bad digest", self.id))?;
        let ids = self
            .grant
            .iter()
            .map(|s| {
                s.parse::<SpiffeId>()
                    .map_err(|e| format!("token {}: {e}", self.id))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(TokenRecord {
            id: self.id.clone(),
            secret_sha256: digest,
            grant: Grant { ids },
            expires_at: time(self.expires_at),
            spent: self.spent,
        })
    }

    pub fn from_record(record: &TokenRecord, created_at: SystemTime) -> TokenRec {
        TokenRec {
            id: record.id.clone(),
            secret_sha256: hex::encode(record.secret_sha256),
            grant: record.grant.ids.iter().map(ToString::to_string).collect(),
            created_at: secs(created_at),
            expires_at: secs(record.expires_at),
            spent: record.spent,
        }
    }
}
