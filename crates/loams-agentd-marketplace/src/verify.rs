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

/// The DER prefix of a P-256 `SubjectPublicKeyInfo`: SEQUENCE {
/// AlgorithmIdentifier { id-ecPublicKey, prime256v1 }, BIT STRING (66 bytes) }.
/// The uncompressed point (`04 || X || Y`, 65 bytes) follows it.
const P256_SPKI_PREFIX: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];
const P256_SPKI_LEN: usize = P256_SPKI_PREFIX.len() + 65;

/// Checks a cosign `sign-blob --key` bundle (Sigstore bundle v0.3, a
/// `messageSignature` and a public-key hint; OP2-B Task 0 ruling) against
/// one ECDSA P-256 public key (Decision 4, D2609): the key form of a trust
/// anchor, for private indexes and the dry-run's job-local key.
///
/// The signature (`messageSignature.signature`, base64 ASN.1 DER) is
/// verified over the artifact bytes; that is the check. The bundle is parsed
/// leniently (unknown fields are ignored, bytes are never compared). The
/// public-key hint and the message digest, when present, only reject a
/// bundle early. The signer [`Identity`] does not apply to a key and is
/// ignored.
#[derive(Clone)]
pub struct KeyVerifier {
    /// The uncompressed point.
    point: Vec<u8>,
    /// base64 of the SHA-256 of the SPKI DER: cosign's `publicKey.hint`.
    hint: String,
}

impl std::fmt::Debug for KeyVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyVerifier")
            .field("hint", &self.hint)
            .finish_non_exhaustive()
    }
}

impl KeyVerifier {
    /// Reads a PEM `PUBLIC KEY` (SPKI) holding an uncompressed P-256 key,
    /// as `cosign generate-key-pair` writes `cosign.pub`. Anything else is
    /// [`SignatureError::Malformed`].
    pub fn from_pem(pem: &str) -> Result<KeyVerifier, SignatureError> {
        use base64::Engine as _;
        use sha2::{Digest, Sha256};

        let malformed = |why: &str| SignatureError::Malformed(format!("public key: {why}"));
        let body = pem
            .trim()
            .strip_prefix("-----BEGIN PUBLIC KEY-----")
            .and_then(|rest| rest.strip_suffix("-----END PUBLIC KEY-----"))
            .ok_or_else(|| malformed("not a PEM PUBLIC KEY"))?;
        let body: String = body.chars().filter(|c| !c.is_ascii_whitespace()).collect();
        let der = base64::engine::general_purpose::STANDARD
            .decode(body)
            .map_err(|_| malformed("the PEM body is not base64"))?;
        if der.len() != P256_SPKI_LEN || der[..P256_SPKI_PREFIX.len()] != P256_SPKI_PREFIX {
            return Err(malformed("not an uncompressed ECDSA P-256 key"));
        }
        let point = der[P256_SPKI_PREFIX.len()..].to_vec();
        if point[0] != 0x04 {
            return Err(malformed("not an uncompressed ECDSA P-256 key"));
        }
        Ok(KeyVerifier {
            point,
            hint: base64::engine::general_purpose::STANDARD.encode(Sha256::digest(&der)),
        })
    }
}

impl SignatureVerifier for KeyVerifier {
    fn verify_blob(
        &self,
        artifact: &[u8],
        bundle: &[u8],
        _signer: &Identity,
    ) -> Result<(), SignatureError> {
        use base64::Engine as _;
        use sha2::{Digest, Sha256};

        if bundle.iter().all(u8::is_ascii_whitespace) {
            return Err(SignatureError::Missing("the bundle is empty".into()));
        }
        let bundle: serde_json::Value = serde_json::from_slice(bundle)
            .map_err(|e| SignatureError::Malformed(format!("not JSON: {e}")))?;
        let b64 = base64::engine::general_purpose::STANDARD;

        // Early rejects only; the signature check below decides.
        if let Some(hint) = bundle
            .pointer("/verificationMaterial/publicKey/hint")
            .and_then(serde_json::Value::as_str)
            && hint != self.hint
        {
            return Err(SignatureError::Invalid(
                "the bundle names another public key".into(),
            ));
        }
        if let Some(digest) = bundle
            .pointer("/messageSignature/messageDigest/digest")
            .and_then(serde_json::Value::as_str)
            && b64.decode(digest).ok().as_deref() != Some(Sha256::digest(artifact).as_slice())
        {
            return Err(SignatureError::Invalid(
                "the bundle signs other bytes".into(),
            ));
        }

        let signature = bundle
            .pointer("/messageSignature/signature")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| SignatureError::Malformed("no messageSignature.signature".into()))?;
        let signature = b64
            .decode(signature)
            .map_err(|_| SignatureError::Malformed("the signature is not base64".into()))?;
        ring::signature::UnparsedPublicKey::new(
            &ring::signature::ECDSA_P256_SHA256_ASN1,
            &self.point,
        )
        .verify(artifact, &signature)
        .map_err(|_| SignatureError::Invalid("the ECDSA P-256 signature does not verify".into()))
    }
}
