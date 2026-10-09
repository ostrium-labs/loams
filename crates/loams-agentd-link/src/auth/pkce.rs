//! PKCE (RFC 7636), S256 only.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest as _, Sha256};

/// A code verifier and its S256 challenge.
#[derive(Clone)]
pub struct Pkce {
    /// The secret kept by the client and sent at the token endpoint.
    pub verifier: String,
    /// `BASE64URL(SHA256(verifier))`, sent in the authorization request.
    pub challenge: String,
}

impl std::fmt::Debug for Pkce {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pkce")
            .field("verifier", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl Pkce {
    /// A fresh verifier of 32 random bytes (43 characters).
    ///
    /// # Panics
    ///
    /// Panics if the operating system has no randomness source.
    #[must_use]
    pub fn generate() -> Self {
        Self::from_verifier(random_token(32))
    }

    /// The challenge for a given verifier (RFC 7636 appendix B).
    #[must_use]
    pub fn from_verifier(verifier: String) -> Self {
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Self {
            verifier,
            challenge,
        }
    }
}

/// `n` random bytes as unpadded base64url, for verifiers, `state` and `nonce`.
///
/// # Panics
///
/// Panics if the operating system has no randomness source.
#[must_use]
pub fn random_token(n: usize) -> String {
    let mut bytes = vec![0_u8; n];
    getrandom::fill(&mut bytes).expect("operating system randomness is available");
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_rfc_7636_vector() {
        let p = Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into());
        assert_eq!(p.challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn generated_verifiers_are_unique_and_in_range() {
        let (a, b) = (Pkce::generate(), Pkce::generate());
        assert_ne!(a.verifier, b.verifier);
        assert!((43..=128).contains(&a.verifier.len()));
        assert!(!format!("{a:?}").contains(&a.verifier));
    }
}
