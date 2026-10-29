//! Reading: every manifest verified, every entry verified by SHA-256.

#![allow(clippy::unwrap_used)]

mod support;

use std::sync::Arc;

use loams_knowledge::{
    KnowledgeError, KnowledgeReader, Layout, PublishRequest, ReaderConfig, TrustPolicy, Visibility,
};
use loams_walrus::StoreOpts;
use support::{COMMIT, COMMIT2, FakeVerifier, REPO, World, fixture};

async fn published(w: &World, commit: &str) {
    w.publisher
        .publish(
            &fixture(),
            &PublishRequest {
                repo: REPO.into(),
                commit: commit.into(),
                visibility: Visibility::Public,
                store: StoreOpts::default(),
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn knowledge_get_verifies_entry() {
    let w = World::new().await;
    published(&w, COMMIT).await;
    let f = w
        .reader
        .get(REPO, Some(COMMIT), "docs/design/walrus.md")
        .await
        .unwrap();
    assert_eq!(f.text(), "# Walrus\n\nHedged reads, verified by SHA-256.\n");
    assert_eq!(f.commit, COMMIT);
    assert_eq!(f.sha256, loams_walrus::ContentDigest::of(&f.data));
    let err = w
        .reader
        .get(REPO, Some(COMMIT), "src/main.rs")
        .await
        .unwrap_err();
    assert!(matches!(err, KnowledgeError::NotInManifest { .. }), "{err}");
    let err = w
        .reader
        .get(REPO, Some(COMMIT), "../etc/passwd")
        .await
        .unwrap_err();
    assert!(matches!(err, KnowledgeError::Invalid(_)), "{err}");
}

/// Review focus 4: a Walrus read is never used before its SHA-256 matches.
#[tokio::test]
async fn knowledge_get_digest_mismatch_refused() {
    let w = World::new().await;
    published(&w, COMMIT).await;
    let m = w.reader.manifest(REPO, Some(COMMIT)).await.unwrap();
    let entry = m.entry("README.md").unwrap();
    w.walrus.tamper(entry.quilt_patch_id.as_str());
    let err = w
        .reader
        .get(REPO, Some(COMMIT), "README.md")
        .await
        .unwrap_err();
    match err {
        KnowledgeError::Digest { path, expected, .. } => {
            assert_eq!(path, "README.md");
            assert_eq!(expected, entry.sha256.to_hex());
        }
        other => panic!("expected a digest error, got {other}"),
    }
}

#[tokio::test]
async fn tampered_manifest_in_bucket_refused() {
    let w = World::new().await;
    published(&w, COMMIT).await;
    let key = Layout::default().manifest(REPO, COMMIT);
    let (bytes, _) = w.store.get(&key).await.unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    let evil = text.replacen("\"bytes\": ", "\"bytes\": 1", 1);
    w.store.put(&key, evil.into()).await.unwrap();
    // A fresh reader: nothing memoised from before the tampering.
    let reader = KnowledgeReader::new(
        w.store.clone(),
        w.walrus.client().await,
        Arc::new(FakeVerifier),
        TrustPolicy::new().pin(REPO, support::signer()),
        Layout::default(),
    );
    let err = reader
        .get(REPO, Some(COMMIT), "README.md")
        .await
        .unwrap_err();
    assert!(matches!(err, KnowledgeError::Signature { .. }), "{err}");
    // A missing bundle is refused the same way.
    w.store
        .delete(&Layout::default().bundle(REPO, COMMIT))
        .await
        .unwrap();
    let err = reader
        .get(REPO, Some(COMMIT), "README.md")
        .await
        .unwrap_err();
    assert!(matches!(err, KnowledgeError::Signature { .. }), "{err}");
}

#[tokio::test]
async fn knowledge_get_latest_commit() {
    let w = World::new().await;
    published(&w, COMMIT).await;
    published(&w, COMMIT2).await;
    let f = w.reader.get(REPO, None, "README.md").await.unwrap();
    assert_eq!(f.commit, COMMIT2);
    let m = w.reader.manifest(REPO, Some(COMMIT)).await.unwrap();
    assert_eq!(m.commit, COMMIT);
    let err = w.reader.get("ostrium-labs/none", None, "README.md").await;
    assert!(err.is_err());
}

#[tokio::test]
async fn knowledge_get_unknown_repo_refused() {
    let w = World::new().await;
    published(&w, COMMIT).await;
    // Same bucket, but no signer pinned for the repository.
    let reader = KnowledgeReader::new(
        w.store.clone(),
        w.walrus.client().await,
        Arc::new(FakeVerifier),
        TrustPolicy::new(),
        Layout::default(),
    );
    let reads = w.walrus.reads();
    let err = reader
        .get(REPO, Some(COMMIT), "README.md")
        .await
        .unwrap_err();
    assert!(matches!(err, KnowledgeError::UnknownRepo(_)), "{err}");
    assert_eq!(
        w.walrus.reads(),
        reads,
        "nothing fetched for an untrusted repo"
    );
}

#[tokio::test]
async fn knowledge_search_paths_and_text() {
    let w = World::new().await;
    published(&w, COMMIT).await;
    let r = w
        .reader
        .search(REPO, Some(COMMIT), "walrus", 10)
        .await
        .unwrap();
    assert_eq!(r.hits[0].path, "docs/design/walrus.md");
    assert_eq!(r.unindexed, 0);
    assert_eq!(r.indexed, 7);
    // A text-only match, with its line as the snippet.
    let r = w
        .reader
        .search(REPO, None, "marketplace", 10)
        .await
        .unwrap();
    let hit = r
        .hits
        .iter()
        .find(|h| h.path == "docs/plans/ms1.md")
        .unwrap();
    assert_eq!(hit.snippet.as_deref(), Some("MS1 seeds the marketplace."));
    // Tracked paths outside the set are found through the file listing.
    let r = w.reader.search(REPO, None, "main.rs", 10).await.unwrap();
    assert_eq!(r.hits[0].path, loams_knowledge::FILES_ENTRY);
    assert_eq!(r.hits[0].snippet.as_deref(), Some("src/main.rs"));
    // The limit holds, and an empty query finds nothing.
    let r = w.reader.search(REPO, None, "md", 2).await.unwrap();
    assert_eq!(r.hits.len(), 2);
    let r = w.reader.search(REPO, None, "   ", 10).await.unwrap();
    assert!(r.hits.is_empty());
}

#[tokio::test]
async fn search_skips_entries_that_fail_verification() {
    let w = World::new().await;
    published(&w, COMMIT).await;
    let m = w.reader.manifest(REPO, Some(COMMIT)).await.unwrap();
    w.walrus
        .tamper(m.entry("AGENTS.md").unwrap().quilt_patch_id.as_str());
    let r = w
        .reader
        .search(REPO, Some(COMMIT), "cargo", 10)
        .await
        .unwrap();
    assert_eq!(r.unindexed, 1);
    assert!(r.hits.iter().all(|h| h.path != "AGENTS.md"), "{r:?}");
}

#[tokio::test]
async fn reader_config_parses_and_needs_walrus() {
    let cfg = ReaderConfig::parse(
        r#"
[walrus]
enabled = false

[knowledge]
bucket = "memory:///"

[[knowledge.signers]]
repo = "ostrium-labs/fixture"
identity = "https://github.com/ostrium-labs/fixture/.github/workflows/knowledge.yml@refs/heads/main"
"#,
    )
    .unwrap();
    assert_eq!(cfg.knowledge.prefix, "knowledge");
    assert_eq!(
        cfg.knowledge.signers[0].issuer,
        loams_knowledge::GITHUB_ACTIONS_ISSUER
    );
    let trust = cfg.trust().unwrap();
    assert!(trust.signer(REPO).is_some());
    let err = cfg.reader_with(Arc::new(FakeVerifier)).await.unwrap_err();
    assert!(matches!(err, KnowledgeError::Invalid(_)), "{err}");
    assert!(ReaderConfig::parse("[knowledge]\nbucket = \"memory:///\"\nwhat = 1\n").is_err());
}
