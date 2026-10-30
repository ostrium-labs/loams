//! Join tokens and the deny list (design §83 §6.4–§6.5, D2311, D2312).
//!
//! A [`JoinToken`] is what an operator hands a new process: an id and a
//! 32-byte secret, written `loamsjt1.<id>.<secret>` (URL-safe base64). The CA
//! keeps only a [`TokenRecord`]: the secret's SHA-256, the [`Grant`] and the
//! expiry. [`TokenStore::redeem`] checks the secret in constant time and
//! marks the token spent in the same step, so a token works once.
//!
//! The [`DenyList`] holds revoked serials and identities; every listener
//! checks it at the handshake and on new streams, and the CA refuses to renew
//! anything on it.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Mutex, PoisonError, RwLock};
use std::time::{Duration, SystemTime};

use aws_lc_rs::digest::{SHA256, digest};
use aws_lc_rs::rand::{SecureRandom, SystemRandom};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use crate::ca::Grant;
use crate::peer::PeerIdentity;
use crate::spiffe::SpiffeId;

/// A join token's life (D2311).
pub const TOKEN_LIFETIME: Duration = Duration::from_secs(3600);
const PREFIX: &str = "loamsjt1";

/// A join token as handed to a new process. `Debug` hides the secret.
#[derive(Clone, PartialEq, Eq)]
pub struct JoinToken {
    pub id: String,
    secret: [u8; 32],
}

impl fmt::Debug for JoinToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JoinToken")
            .field("id", &self.id)
            .field("secret", &"<redacted>")
            .finish()
    }
}

/// Why a token was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TokenError {
    #[error("not a join token")]
    Malformed,
    #[error("unknown join token")]
    Unknown,
    #[error("the join token was already used")]
    Spent,
    #[error("the join token expired")]
    Expired,
    #[error("the system random source failed")]
    Random,
}

impl JoinToken {
    /// A fresh token with a random id and secret.
    pub fn generate() -> Result<JoinToken, TokenError> {
        let rng = SystemRandom::new();
        let mut id = [0u8; 9];
        let mut secret = [0u8; 32];
        rng.fill(&mut id).map_err(|_| TokenError::Random)?;
        rng.fill(&mut secret).map_err(|_| TokenError::Random)?;
        Ok(JoinToken {
            id: URL_SAFE_NO_PAD.encode(id),
            secret,
        })
    }

    /// The string an operator hands over (`loamsjt1.<id>.<secret>`).
    pub fn expose(&self) -> String {
        format!(
            "{PREFIX}.{}.{}",
            self.id,
            URL_SAFE_NO_PAD.encode(self.secret)
        )
    }

    pub fn parse(s: &str) -> Result<JoinToken, TokenError> {
        let mut parts = s.trim().split('.');
        let (Some(PREFIX), Some(id), Some(secret), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(TokenError::Malformed);
        };
        if id.is_empty() || id.len() > 64 {
            return Err(TokenError::Malformed);
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(secret)
            .map_err(|_| TokenError::Malformed)?;
        let secret: [u8; 32] = bytes.try_into().map_err(|_| TokenError::Malformed)?;
        Ok(JoinToken {
            id: id.to_string(),
            secret,
        })
    }

    fn digest(&self) -> [u8; 32] {
        let d = digest(&SHA256, &self.secret);
        let mut out = [0u8; 32];
        out.copy_from_slice(d.as_ref());
        out
    }

    /// The record the CA keeps for this token: no secret, only its digest.
    pub fn record(&self, grant: Grant, now: SystemTime) -> TokenRecord {
        TokenRecord {
            id: self.id.clone(),
            secret_sha256: self.digest(),
            grant,
            expires_at: now + TOKEN_LIFETIME,
            spent: false,
        }
    }
}

/// What the CA stores per token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenRecord {
    pub id: String,
    pub secret_sha256: [u8; 32],
    pub grant: Grant,
    pub expires_at: SystemTime,
    pub spent: bool,
}

/// Where token records live: the metastore in the CA service, memory in
/// tests. `redeem` must check and spend atomically.
pub trait TokenStore: Send + Sync {
    fn put(&self, record: TokenRecord);
    fn redeem(&self, token: &JoinToken, now: SystemTime) -> Result<Grant, TokenError>;
}

/// An in-memory [`TokenStore`].
#[derive(Debug, Default)]
pub struct MemoryTokens {
    records: Mutex<HashMap<String, TokenRecord>>,
}

impl TokenStore for MemoryTokens {
    fn put(&self, record: TokenRecord) {
        self.records
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(record.id.clone(), record);
    }

    fn redeem(&self, token: &JoinToken, now: SystemTime) -> Result<Grant, TokenError> {
        let mut records = self.records.lock().unwrap_or_else(PoisonError::into_inner);
        records
            .get_mut(&token.id)
            .ok_or(TokenError::Unknown)?
            .redeem(token, now)
    }
}

impl TokenRecord {
    /// Checks `token`'s secret against this record in constant time and, if
    /// it matches and the record is unspent and unexpired, spends it and
    /// returns its grant. A store that keeps records elsewhere (the CA
    /// service's metastore document) calls this inside its own atomic
    /// update, so the check and the spend stay one step.
    pub fn redeem(&mut self, token: &JoinToken, now: SystemTime) -> Result<Grant, TokenError> {
        if self.id != token.id
            || aws_lc_rs::constant_time::verify_slices_are_equal(
                &self.secret_sha256,
                &token.digest(),
            )
            .is_err()
        {
            return Err(TokenError::Unknown);
        }
        if self.spent {
            return Err(TokenError::Spent);
        }
        if now >= self.expires_at {
            return Err(TokenError::Expired);
        }
        self.spent = true;
        Ok(self.grant.clone())
    }
}

/// Revoked serials and identities (D2312).
#[derive(Debug, Default)]
pub struct DenyList {
    serials: RwLock<HashSet<Vec<u8>>>,
    ids: RwLock<HashSet<SpiffeId>>,
}

impl DenyList {
    pub fn revoke_serial(&self, serial: &[u8]) {
        self.serials
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(serial.to_vec());
    }

    pub fn revoke_identity(&self, id: SpiffeId) {
        self.ids
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id);
    }

    /// Replaces every entry with `serials` and `ids`: what a node does when
    /// the metastore's copy of the list changes (§83 §6.5).
    pub fn replace(
        &self,
        serials: impl IntoIterator<Item = Vec<u8>>,
        ids: impl IntoIterator<Item = SpiffeId>,
    ) {
        let serials: HashSet<Vec<u8>> = serials.into_iter().collect();
        let ids: HashSet<SpiffeId> = ids.into_iter().collect();
        *self.serials.write().unwrap_or_else(PoisonError::into_inner) = serials;
        *self.ids.write().unwrap_or_else(PoisonError::into_inner) = ids;
    }

    /// Whether a peer with this leaf serial and these identities is refused.
    pub fn denies(&self, serial: &[u8], peer: &PeerIdentity) -> bool {
        if self
            .serials
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(serial)
        {
            return true;
        }
        let ids = self.ids.read().unwrap_or_else(PoisonError::into_inner);
        peer.ids().iter().any(|id| ids.contains(id))
    }
}
