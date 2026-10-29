//! MK1 Task 4: the production verifier checks a real Sigstore bundle
//! offline against the embedded public-good trusted root, and holds it to
//! the pinned identity (testdata/sigstore/README.md).
#![allow(clippy::unwrap_used)]

use std::path::Path;

use loams_agentd_marketplace::SignatureVerifier;
use loams_agentd_marketplace::verify::{Identity, SignatureError, SigstoreVerifier};

const COSIGN_SHA256: &str = "c956e5dfcac53d52bcf058360d579472f0c1d2d9b69f55209e256fe7783f4c74";

fn bundle() -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/sigstore/cosign-linux-amd64.sigstore.json"),
    )
    .unwrap()
}

fn cosign_release() -> Identity {
    Identity {
        identity: "keyless@projectsigstore.iam.gserviceaccount.com".into(),
        issuer: "https://accounts.google.com".into(),
    }
}

#[test]
fn a_real_bundle_verifies_for_its_signer() {
    let verifier = SigstoreVerifier::production().unwrap();
    verifier
        .verify_digest(COSIGN_SHA256, &bundle(), &cosign_release())
        .unwrap();
}

#[test]
fn index_identity_pinned_with_real_signatures() {
    let verifier = SigstoreVerifier::production().unwrap();
    let pinned = Identity {
        identity: "https://github.com/ostrium-labs/loams-marketplace/.github/workflows/publish.yml@refs/heads/main".into(),
        issuer: "https://token.actions.githubusercontent.com".into(),
    };
    let error = verifier
        .verify_digest(COSIGN_SHA256, &bundle(), &pinned)
        .unwrap_err();
    assert!(matches!(error, SignatureError::Invalid(_)), "{error}");
    // The right identity from the wrong issuer fails too.
    let wrong_issuer = Identity {
        issuer: "https://token.actions.githubusercontent.com".into(),
        ..cosign_release()
    };
    assert!(
        verifier
            .verify_digest(COSIGN_SHA256, &bundle(), &wrong_issuer)
            .is_err()
    );
}

#[test]
fn other_bytes_do_not_verify() {
    let verifier = SigstoreVerifier::production().unwrap();
    let other = "0".repeat(64);
    assert!(
        verifier
            .verify_digest(&other, &bundle(), &cosign_release())
            .is_err()
    );
    assert!(
        verifier
            .verify_blob(
                b"an index that was never signed",
                &bundle(),
                &cosign_release()
            )
            .is_err()
    );
}

#[test]
fn a_malformed_bundle_is_refused() {
    let verifier = SigstoreVerifier::production().unwrap();
    for junk in [&b""[..], b"{}", b"not json", &[0xff, 0xfe]] {
        assert!(verifier.verify_blob(b"x", junk, &cosign_release()).is_err());
    }
}
