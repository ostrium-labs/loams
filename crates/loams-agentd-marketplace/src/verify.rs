//! Signature checks (design §73 §6, D1459, D1462; MK1 Task 0 rulings 3–4).
//! An index and its revocation list are trusted only when a Sigstore bundle
//! beside each verifies and was signed by the source's pinned identity.

use sigstore_verify::trust_root::{SIGSTORE_PRODUCTION_TRUSTED_ROOT, TrustedRoot};
use sigstore_verify::types::{Artifact, Bundle, Sha256Hash};
use sigstore_verify::{IdentityMatcher, VerificationPolicy};

/// Who must have signed: a certificate identity (the SAN, for GitHub Actions
/// the workflow URL with its ref) and its OIDC issuer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub identity: String,
    pub issuer: String,
}

pub const GITHUB_ACTIONS_ISSUER: &str = "https://token.actions.githubusercontent.com";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SignatureError {
    #[error("no signature: {0}")]
    Missing(String),
    #[error("the signature bundle is malformed: {0}")]
    Malformed(String),
    #[error("the signature does not verify: {0}")]
    Invalid(String),
}

/// What signed what. The production implementation is [`SigstoreVerifier`];
/// tests substitute their own.
pub trait SignatureVerifier: Send + Sync {
    /// Checks `bundle` (Sigstore bundle JSON) over `artifact`, signed by `signer`.
    fn verify_blob(
        &self,
        artifact: &[u8],
        bundle: &[u8],
        signer: &Identity,
    ) -> Result<(), SignatureError>;
}

/// Sigstore's public-good instance, with the trusted root embedded in
/// `sigstore-trust-root` (no network at verification time).
pub struct SigstoreVerifier {
    root: TrustedRoot,
}

impl std::fmt::Debug for SigstoreVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SigstoreVerifier").finish_non_exhaustive()
    }
}

impl SigstoreVerifier {
    pub fn production() -> Result<Self, SignatureError> {
        let root = TrustedRoot::from_json(SIGSTORE_PRODUCTION_TRUSTED_ROOT)
            .map_err(|e| SignatureError::Malformed(format!("embedded trusted root: {e}")))?;
        Ok(Self { root })
    }

    /// Verifies against a SHA-256 digest when the bytes are not at hand.
    pub fn verify_digest(
        &self,
        sha256_hex: &str,
        bundle: &[u8],
        signer: &Identity,
    ) -> Result<(), SignatureError> {
        let digest = Sha256Hash::from_hex(sha256_hex)
            .map_err(|e| SignatureError::Malformed(format!("digest: {e}")))?;
        self.check(Artifact::from(digest), bundle, signer)
    }

    fn check(
        &self,
        artifact: Artifact<'_>,
        bundle: &[u8],
        signer: &Identity,
    ) -> Result<(), SignatureError> {
        let text = std::str::from_utf8(bundle)
            .map_err(|_| SignatureError::Malformed("not UTF-8".into()))?;
        let bundle =
            Bundle::from_json(text).map_err(|e| SignatureError::Malformed(e.to_string()))?;
        let policy = VerificationPolicy::new(
            IdentityMatcher::Exact(signer.identity.clone()),
            signer.issuer.clone(),
        );
        sigstore_verify::verify(artifact, &bundle, &policy, &self.root)
            .map(|_| ())
            .map_err(|e| SignatureError::Invalid(e.to_string()))
    }
}

impl SignatureVerifier for SigstoreVerifier {
    fn verify_blob(
        &self,
        artifact: &[u8],
        bundle: &[u8],
        signer: &Identity,
    ) -> Result<(), SignatureError> {
        self.check(Artifact::from(artifact), bundle, signer)
    }
}
