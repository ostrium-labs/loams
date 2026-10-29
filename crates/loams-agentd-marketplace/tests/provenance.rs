//! MK1 Task 6: integrity and provenance, on a real npm package with real
//! Sigstore provenance (`testdata/npm`: sigstore@3.1.0, Apache-2.0, built by
//! sigstore/sigstore-js's release workflow), verified offline.
#![allow(clippy::unwrap_used)]

use std::path::Path;

use loams_agentd_marketplace::SigstoreVerifier;
use loams_agentd_marketplace::provenance::{self, ProvenanceError};

fn read(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/npm")
            .join(name),
    )
    .unwrap()
}

fn integrity() -> String {
    String::from_utf8(read("sigstore-3.1.0.integrity"))
        .unwrap()
        .trim()
        .to_owned()
}

const SOURCE: &str = "https://github.com/sigstore/sigstore-js";

#[test]
fn a_real_package_verifies() {
    let verifier = SigstoreVerifier::production().unwrap();
    let built = provenance::verify_npm(
        &read("sigstore-3.1.0.tgz"),
        &integrity(),
        &[read("sigstore-3.1.0.slsa.sigstore.json")],
        SOURCE,
        &verifier,
    )
    .unwrap();
    assert_eq!(built.repository, SOURCE);
    assert!(
        built
            .identity
            .starts_with("https://github.com/sigstore/sigstore-js/.github/workflows/")
    );
}

#[test]
fn integrity_mismatch_refused() {
    let verifier = SigstoreVerifier::production().unwrap();
    let mut tarball = read("sigstore-3.1.0.tgz");
    tarball.push(0);
    let error = provenance::verify_npm(
        &tarball,
        &integrity(),
        &[read("sigstore-3.1.0.slsa.sigstore.json")],
        SOURCE,
        &verifier,
    )
    .unwrap_err();
    assert!(
        matches!(error, ProvenanceError::IntegrityMismatch { .. }),
        "{error}"
    );
}

#[test]
fn integrity_mismatch_refused_even_when_the_listing_matches_other_bytes() {
    // The listing pins other bytes than the ones fetched: refused before provenance.
    let verifier = SigstoreVerifier::production().unwrap();
    let other = provenance::npm_integrity(b"other bytes");
    let error = provenance::verify_npm(
        &read("sigstore-3.1.0.tgz"),
        &other,
        &[read("sigstore-3.1.0.slsa.sigstore.json")],
        SOURCE,
        &verifier,
    )
    .unwrap_err();
    assert!(matches!(error, ProvenanceError::IntegrityMismatch { .. }));
}

#[test]
fn provenance_repo_mismatch_refused() {
    let verifier = SigstoreVerifier::production().unwrap();
    let error = provenance::verify_npm(
        &read("sigstore-3.1.0.tgz"),
        &integrity(),
        &[read("sigstore-3.1.0.slsa.sigstore.json")],
        "https://github.com/ostrium-labs/loams-mastra",
        &verifier,
    )
    .unwrap_err();
    assert!(
        matches!(error, ProvenanceError::RepoMismatch { .. }),
        "{error}"
    );
}

#[test]
fn a_package_without_provenance_is_refused() {
    let verifier = SigstoreVerifier::production().unwrap();
    let error = provenance::verify_npm(
        &read("sigstore-3.1.0.tgz"),
        &integrity(),
        &[],
        SOURCE,
        &verifier,
    )
    .unwrap_err();
    assert_eq!(error, ProvenanceError::Missing);
}

#[test]
fn a_forged_statement_does_not_verify() {
    // Rewrite the statement to name another repository: the DSSE signature no
    // longer covers it.
    let verifier = SigstoreVerifier::production().unwrap();
    let mut bundle: serde_json::Value =
        serde_json::from_slice(&read("sigstore-3.1.0.slsa.sigstore.json")).unwrap();
    use base64::Engine;
    let engine = base64::engine::general_purpose::STANDARD;
    let payload = engine
        .decode(bundle["dsseEnvelope"]["payload"].as_str().unwrap())
        .unwrap();
    let forged = String::from_utf8(payload)
        .unwrap()
        .replace("sigstore/sigstore-js", "attacker/sigstore-js");
    bundle["dsseEnvelope"]["payload"] = engine.encode(forged).into();
    let error = provenance::verify_npm(
        &read("sigstore-3.1.0.tgz"),
        &integrity(),
        &[serde_json::to_vec(&bundle).unwrap()],
        "https://github.com/attacker/sigstore-js",
        &verifier,
    )
    .unwrap_err();
    assert!(matches!(error, ProvenanceError::Signature(_)), "{error}");
}
