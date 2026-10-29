//! The signed manifest: shape, signing and verification.

#![allow(clippy::unwrap_used)]

mod support;

use loams_knowledge::{
    Identity, KnowledgeError, KnowledgeManifest, MANIFEST_SCHEMA, ManifestEntry, ManifestSigner,
    SignatureError, VerifiedManifest,
};
use loams_walrus::{BlobId, ContentDigest, QuiltPatchId};
use support::{COMMIT, COMMIT2, FakeSigner, FakeVerifier, REPO, signer};

fn manifest() -> KnowledgeManifest {
    let entry = |path: &str, body: &str| ManifestEntry {
        path: path.into(),
        quilt_patch_id: QuiltPatchId::parse(&format!("patch{}", path.len())).unwrap(),
        sha256: ContentDigest::of(body.as_bytes()),
        bytes: body.len() as u64,
    };
    KnowledgeManifest {
        schema: MANIFEST_SCHEMA.into(),
        repo: REPO.into(),
        commit: COMMIT.into(),
        quilt_id: BlobId::from_bytes([7; 32]),
        created: "2026-10-26T00:00:00Z".into(),
        end_epoch: 42,
        entries: vec![entry("README.md", "hi"), entry("docs/a.md", "a")],
    }
}

async fn signed() -> (Vec<u8>, Vec<u8>) {
    let bytes = manifest().to_bytes().unwrap();
    let bundle = FakeSigner::default().sign(&bytes).await.unwrap();
    (bytes, bundle)
}

#[test]
fn manifest_round_trips() {
    let m = manifest();
    let bytes = m.to_bytes().unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(text.contains("\"quiltId\""), "{text}");
    assert!(text.contains("\"quiltPatchId\""), "{text}");
    assert!(text.contains("\"endEpoch\": 42"), "{text}");
    assert_eq!(KnowledgeManifest::parse(&bytes).unwrap(), m);
    assert_eq!(m.entry("docs/a.md").unwrap().bytes, 1);
    assert!(m.entry("docs/b.md").is_none());
}

#[test]
fn manifest_rejects_bad_paths() {
    for bad in ["", "/abs", "a//b", "a/../b", "./a", "a\"b", "a\\b", "a\nb"] {
        let mut m = manifest();
        m.entries[1].path = bad.into();
        assert!(m.to_bytes().is_err(), "{bad:?} accepted");
    }
    let mut unsorted = manifest();
    unsorted.entries.reverse();
    assert!(unsorted.validate().is_err());
    let mut dup = manifest();
    dup.entries[1].path = "README.md".into();
    assert!(dup.validate().is_err());
    let mut commit = manifest();
    commit.commit = "HEAD".into();
    assert!(commit.validate().is_err());
}

#[tokio::test]
async fn manifest_sign_verify() {
    let (bytes, bundle) = signed().await;
    let v = VerifiedManifest::verify(
        &bytes,
        Some(&bundle),
        &FakeVerifier,
        &signer(),
        REPO,
        COMMIT,
    )
    .unwrap();
    assert_eq!(*v.manifest(), manifest());
}

#[tokio::test]
async fn manifest_signature_required() {
    let (bytes, _) = signed().await;
    let err =
        VerifiedManifest::verify(&bytes, None, &FakeVerifier, &signer(), REPO, COMMIT).unwrap_err();
    assert!(
        matches!(
            err,
            KnowledgeError::Signature {
                error: SignatureError::Missing(_),
                ..
            }
        ),
        "{err}"
    );
}

#[tokio::test]
async fn tampered_manifest_refused() {
    let (bytes, bundle) = signed().await;
    // Point README.md at other bytes, keeping the old signature.
    let mut m = manifest();
    m.entries[0].sha256 = ContentDigest::of(b"evil");
    let tampered = m.to_bytes().unwrap();
    assert_ne!(tampered, bytes);
    let err = VerifiedManifest::verify(
        &tampered,
        Some(&bundle),
        &FakeVerifier,
        &signer(),
        REPO,
        COMMIT,
    )
    .unwrap_err();
    assert!(
        matches!(
            err,
            KnowledgeError::Signature {
                error: SignatureError::Invalid(_),
                ..
            }
        ),
        "{err}"
    );
}

#[tokio::test]
async fn manifest_wrong_signer_refused() {
    let bytes = manifest().to_bytes().unwrap();
    let bundle = FakeSigner {
        identity: "https://github.com/mallory/fork/.github/workflows/x.yml@refs/heads/main".into(),
    }
    .sign(&bytes)
    .await
    .unwrap();
    let err = VerifiedManifest::verify(
        &bytes,
        Some(&bundle),
        &FakeVerifier,
        &signer(),
        REPO,
        COMMIT,
    )
    .unwrap_err();
    assert!(matches!(err, KnowledgeError::Signature { .. }), "{err}");
    // And a signer pinned for another issuer.
    let other = Identity {
        issuer: "https://accounts.example.com".into(),
        ..signer()
    };
    let (bytes, bundle) = signed().await;
    assert!(
        VerifiedManifest::verify(&bytes, Some(&bundle), &FakeVerifier, &other, REPO, COMMIT)
            .is_err()
    );
}

#[tokio::test]
async fn manifest_for_other_commit_refused() {
    // A validly signed manifest for one commit, served as another's.
    let (bytes, bundle) = signed().await;
    let err = VerifiedManifest::verify(
        &bytes,
        Some(&bundle),
        &FakeVerifier,
        &signer(),
        REPO,
        COMMIT2,
    )
    .unwrap_err();
    assert!(matches!(err, KnowledgeError::InvalidManifest(_)), "{err}");
    let err = VerifiedManifest::verify(
        &bytes,
        Some(&bundle),
        &FakeVerifier,
        &signer(),
        "ostrium-labs/other",
        COMMIT,
    )
    .unwrap_err();
    assert!(matches!(err, KnowledgeError::InvalidManifest(_)), "{err}");
}
