//! Publishing: public only, the knowledge set, the quilt and the signed
//! manifest in the bucket.

#![allow(clippy::unwrap_used)]

mod support;

use loams_knowledge::{
    FILES_ENTRY, GitCheckout, KnowledgeError, KnowledgeManifest, Layout, MemorySource,
    PublishRequest, Visibility,
};
use loams_walrus::StoreOpts;
use support::{COMMIT, FakeVerifier, REPO, World, fixture, signer};

fn request(visibility: Visibility) -> PublishRequest {
    PublishRequest {
        repo: REPO.into(),
        commit: COMMIT.into(),
        visibility,
        store: StoreOpts::default(),
    }
}

fn paths(m: &KnowledgeManifest) -> Vec<&str> {
    m.entries.iter().map(|e| e.path.as_str()).collect()
}

/// Review focus 5: private data never reaches a publisher.
#[tokio::test]
async fn publish_refuses_private_repo() {
    let w = World::new().await;
    for visibility in [Visibility::Private, Visibility::Unknown] {
        let err = w
            .publisher
            .publish(&fixture(), &request(visibility))
            .await
            .unwrap_err();
        assert!(matches!(err, KnowledgeError::NotPublic { .. }), "{err}");
    }
    assert_eq!(w.walrus.puts(), 0, "nothing may reach the publisher");
    assert!(w.store.list("knowledge/").await.unwrap().is_empty());
}

#[tokio::test]
async fn publish_refuses_unmarked_repo() {
    let w = World::new().await;
    let unmarked = MemorySource::new().file("README.md", "hi\n");
    let err = w
        .publisher
        .publish(&unmarked, &request(Visibility::Public))
        .await
        .unwrap_err();
    assert!(matches!(err, KnowledgeError::NotPublic { .. }), "{err}");
    let marked_false = MemorySource::new()
        .file(".loams/knowledge.toml", "public = false\n")
        .file("README.md", "hi\n");
    let err = w
        .publisher
        .publish(&marked_false, &request(Visibility::Public))
        .await
        .unwrap_err();
    assert!(matches!(err, KnowledgeError::NotPublic { .. }), "{err}");
    assert_eq!(w.walrus.puts(), 0);
}

#[tokio::test]
async fn publish_selects_knowledge_set() {
    let w = World::new().await;
    let out = w
        .publisher
        .publish(&fixture(), &request(Visibility::Public))
        .await
        .unwrap();
    assert_eq!(
        paths(&out.manifest),
        [
            FILES_ENTRY,
            "AGENTS.md",
            "README.md",
            "docs/design/walrus.md",
            "docs/plans/ms1.md",
            "proto/a/v1/a.proto",
            "schemas/a.json",
        ]
    );
    // The file listing has every tracked path, the excluded ones too.
    let listing = w.reader.get(REPO, Some(COMMIT), FILES_ENTRY).await.unwrap();
    let listing = listing.text();
    for p in ["src/main.rs", "docs/logo.png", "docs/big.md", "README.md"] {
        assert!(
            listing.lines().any(|l| l == p),
            "{p} missing from {listing}"
        );
    }
}

#[tokio::test]
async fn knowledge_toml_only_narrows() {
    let w = World::new().await;
    let source = fixture().file(
        ".loams/knowledge.toml",
        "public = true\ninclude = [\"docs/\", \"README.md\", \"src/\"]\nexclude = [\"docs/plans\"]\n",
    );
    let out = w
        .publisher
        .publish(&source, &request(Visibility::Public))
        .await
        .unwrap();
    // `src/` is outside the default set: including it does not widen it.
    assert_eq!(
        paths(&out.manifest),
        [FILES_ENTRY, "README.md", "docs/design/walrus.md"]
    );
}

#[tokio::test]
async fn publish_stores_quilt_and_signed_manifest() {
    let w = World::new().await;
    let out = w
        .publisher
        .publish(&fixture(), &request(Visibility::Public))
        .await
        .unwrap();
    assert_eq!(w.walrus.puts(), 1, "one quilt");
    assert_eq!(out.manifest.end_epoch, 42);
    assert_eq!(out.manifest.repo, REPO);
    assert_eq!(out.manifest.commit, COMMIT);
    let layout = Layout::default();
    assert_eq!(out.manifest_key, layout.manifest(REPO, COMMIT));
    assert_eq!(
        out.manifest_key,
        format!("knowledge/{REPO}/{COMMIT}/manifest.json")
    );
    let (stored, _) = w.store.get(&out.manifest_key).await.unwrap();
    let (bundle, _) = w.store.get(&out.bundle_key).await.unwrap();
    assert_eq!(stored, out.manifest_bytes);
    let verified = loams_knowledge::VerifiedManifest::verify(
        &stored,
        Some(&bundle),
        &FakeVerifier,
        &signer(),
        REPO,
        COMMIT,
    )
    .unwrap();
    assert_eq!(*verified.manifest(), out.manifest);
    let (latest, _) = w.store.get(&layout.latest(REPO)).await.unwrap();
    assert_eq!(std::str::from_utf8(&latest).unwrap().trim(), COMMIT);
    // Each entry's digest and size are those of the source's bytes.
    let readme = out.manifest.entry("README.md").unwrap();
    assert_eq!(
        readme.sha256,
        loams_walrus::ContentDigest::of(b"# Fixture\n\nThe fixture repository.\n")
    );
}

#[tokio::test]
async fn publish_too_large_set_refused() {
    let w = World::new().await;
    let mut source = fixture();
    // Twelve files just under 1 MiB each: over the 10 MiB quilt limit.
    for i in 0..12 {
        source = source.file(&format!("docs/big{i}.md"), "b".repeat(1000 * 1024));
    }
    let err = w
        .publisher
        .publish(&source, &request(Visibility::Public))
        .await
        .unwrap_err();
    assert!(matches!(err, KnowledgeError::TooLarge { .. }), "{err}");
    assert_eq!(w.walrus.puts(), 0);
}

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[tokio::test]
async fn publish_from_git_checkout() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    git(d, &["init", "-q"]);
    std::fs::create_dir_all(d.join(".loams")).unwrap();
    std::fs::create_dir_all(d.join("docs")).unwrap();
    std::fs::write(d.join(".loams/knowledge.toml"), "public = true\n").unwrap();
    std::fs::write(d.join("README.md"), "committed\n").unwrap();
    std::fs::write(d.join("docs/a.md"), "doc a\n").unwrap();
    std::fs::write(d.join("main.rs"), "fn main() {}\n").unwrap();
    git(d, &["add", "-A"]);
    git(d, &["commit", "-q", "-m", "one"]);
    let commit = git(d, &["rev-parse", "HEAD"]).trim().to_string();
    // Uncommitted changes are never read.
    std::fs::write(d.join("README.md"), "dirty\n").unwrap();
    std::fs::write(d.join("docs/untracked.md"), "untracked\n").unwrap();

    let w = World::new().await;
    let req = PublishRequest {
        commit: commit.clone(),
        ..request(Visibility::Public)
    };
    let out = w
        .publisher
        .publish(&GitCheckout::new(d), &req)
        .await
        .unwrap();
    assert_eq!(
        paths(&out.manifest),
        [FILES_ENTRY, "README.md", "docs/a.md"]
    );
    let readme = w
        .reader
        .get(REPO, Some(&commit), "README.md")
        .await
        .unwrap();
    assert_eq!(readme.text(), "committed\n");
    let files = w.reader.get(REPO, None, FILES_ENTRY).await.unwrap().text();
    assert_eq!(
        files,
        ".loams/knowledge.toml\nREADME.md\ndocs/a.md\nmain.rs\n"
    );
}
