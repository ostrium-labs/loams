//! Where role passwords live: the [`SecretStore`] seam (design §46 §8.6,
//! D711; PG2 Task 6), until the Q30 credential store replaces its
//! implementations behind the same trait.
//!
//! - [`FileSecretStore`] (single-node): one age-encrypted file, its key (an
//!   age X25519 identity) in a 0600 file or, with the feature `keyring`, the
//!   OS keyring.
//! - [`KubeSecretStore`] (feature `kubernetes`): one Kubernetes Secret per
//!   reference. Encryption at rest is the cluster's: an `EncryptionConfiguration`
//!   with a KMS provider for `secrets` (PG2 Task 48's chart documents it).
//!
//! **One secret, one reference, one record.** A reference
//! ([`SecretRef::new_role`]) is fresh for every password issued, and names
//! no role: a [`RoleRec`](crate::model::RoleRec) holds the reference, never
//! the secret. A reset writes the new password under a new reference, moves
//! the record to it, and only then deletes the old secret; a child branch's
//! roles get copies under their own references. So deleting a secret is
//! safe as soon as the record that held it has moved on, and a secret whose
//! record never committed is unreferenced (the service deletes it, or leaves
//! it when the write's outcome is unknown).
//!
//! Nothing here logs, or puts in an error, a secret's bytes. References are
//! not secrets and may be logged.

mod file;
#[cfg(feature = "kubernetes")]
mod kubernetes;

use async_trait::async_trait;
use base64::Engine;
use rand::TryRngCore;
use serde::{Deserialize, Serialize};
use ulid::Ulid;

pub use file::{FileSecretStore, KeySource};
#[cfg(feature = "kubernetes")]
pub use kubernetes::KubeSecretStore;
/// The redacting wrapper (`loams-postgres`'s, R2.8): its `Debug` and
/// `Display` print `[redacted]`.
pub use loams_postgres::Secret;

use crate::ids::ProjectId;

/// How many random bytes a role password holds (§46 §8.6).
pub const PASSWORD_BYTES: usize = 32;

/// The longest reference: a Kubernetes object name (a DNS subdomain).
pub const MAX_REF_LEN: usize = 253;

/// A secret's name in its store. Lower-case letters, digits, `-` and `.`,
/// starting and ending with a letter or digit, at most [`MAX_REF_LEN`]
/// bytes: valid as a Kubernetes Secret's name as is.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SecretRef(String);

impl SecretRef {
    /// A fresh reference for a role password of `project`:
    /// `pg-role-<project ulid>-<ulid>`, lower case.
    pub fn new_role(project: &ProjectId) -> Self {
        SecretRef(format!(
            "pg-role-{}-{}",
            project.ulid().to_string().to_ascii_lowercase(),
            Ulid::generate().to_string().to_ascii_lowercase()
        ))
    }

    /// The reference `s`, checked.
    ///
    /// # Errors
    ///
    /// [`SecretError::InvalidRef`] for a name outside the grammar.
    pub fn parse(s: &str) -> Result<Self, SecretError> {
        let bytes = s.as_bytes();
        let edge = |b: &u8| b.is_ascii_lowercase() || b.is_ascii_digit();
        let ok = !bytes.is_empty()
            && bytes.len() <= MAX_REF_LEN
            && bytes.first().is_some_and(edge)
            && bytes.last().is_some_and(edge)
            && bytes.iter().all(|b| edge(b) || *b == b'-' || *b == b'.');
        if ok {
            Ok(SecretRef(s.to_string()))
        } else {
            Err(SecretError::InvalidRef(s.to_string()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SecretRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for SecretRef {
    type Error = SecretError;
    fn try_from(s: String) -> Result<Self, SecretError> {
        SecretRef::parse(&s)
    }
}

impl From<SecretRef> for String {
    fn from(r: SecretRef) -> String {
        r.0
    }
}

/// Why a secret store call failed. Never carries a secret's bytes.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    /// No secret under the reference.
    #[error("no secret {0}")]
    NotFound(String),
    /// A reference outside [`SecretRef`]'s grammar.
    #[error("invalid secret reference {0:?}")]
    InvalidRef(String),
    /// The store did not answer, or its file could not be read or written.
    #[error("secret store unavailable: {0}")]
    Unavailable(String),
    /// The store's content does not decrypt or decode.
    #[error("secret store corrupt: {0}")]
    Corrupt(String),
    /// The store is set up unsafely or wrongly (for example a key file
    /// others can read).
    #[error("secret store misconfigured: {0}")]
    Config(String),
}

/// The credential store's seam (PG2's shared contract).
#[async_trait]
pub trait SecretStore: Send + Sync + 'static {
    /// Stores `s` under `r`, replacing what was there.
    async fn put(&self, r: &SecretRef, s: Secret<Vec<u8>>) -> Result<(), SecretError>;
    /// The secret under `r`.
    async fn get(&self, r: &SecretRef) -> Result<Secret<Vec<u8>>, SecretError>;
    /// Deletes the secret under `r`; an absent one is not an error.
    async fn delete(&self, r: &SecretRef) -> Result<(), SecretError>;
}

/// A new role password: [`PASSWORD_BYTES`] bytes from the operating
/// system's generator, base64url without padding (43 characters).
///
/// # Errors
///
/// [`SecretError::Unavailable`] when the OS generator fails.
pub fn generate_password() -> Result<Secret<String>, SecretError> {
    let mut bytes = zeroize::Zeroizing::new([0u8; PASSWORD_BYTES]);
    rand::rngs::OsRng
        .try_fill_bytes(bytes.as_mut())
        .map_err(|e| SecretError::Unavailable(format!("the OS random generator: {e}")))?;
    Ok(Secret::new(
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes.as_ref()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_password_is_32_bytes_of_base64url() {
        let a = generate_password().expect("a password");
        let b = generate_password().expect("a password");
        assert_eq!(a.expose().len(), 43);
        assert_ne!(a.expose(), b.expose());
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(a.expose())
            .expect("base64url");
        assert_eq!(decoded.len(), PASSWORD_BYTES);
        assert_eq!(format!("{a:?} {a}"), "[redacted] [redacted]");
    }

    #[test]
    fn a_role_ref_is_a_kubernetes_name_and_fresh() {
        let p = ProjectId::new();
        let (a, b) = (SecretRef::new_role(&p), SecretRef::new_role(&p));
        assert_ne!(a, b);
        assert_eq!(SecretRef::parse(a.as_str()), Ok(a.clone()));
        assert!(a.as_str().starts_with("pg-role-"));
        for bad in ["", "-a", "a-", "A", "a/b", "a_b", &"a".repeat(254)] {
            assert!(SecretRef::parse(bad).is_err(), "{bad:?}");
        }
        let json = serde_json::to_string(&a).expect("json");
        assert_eq!(serde_json::from_str::<SecretRef>(&json).expect("back"), a);
        assert!(serde_json::from_str::<SecretRef>("\"A/b\"").is_err());
    }
}
