//! MK1 Task 4: sources, signed index verification, search and revocations.
//! The index fixture (`testdata/index`) was built by `loams-listing index
//! build`, so parsing it also checks that Rust and TypeScript agree on the
//! canonical listing digest.
#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use loams_agentd_marketplace::index::{Kind, Level, Tier};
use loams_agentd_marketplace::revocations::{Installed, Severity};
use loams_agentd_marketplace::sources::{OFFICIAL_IDENTITY, OFFICIAL_NAME, OFFICIAL_URL};
use loams_agentd_marketplace::verify::{GITHUB_ACTIONS_ISSUER, Identity, SignatureError};
use loams_agentd_marketplace::{
    Fetcher, MarketError, Marketplace, Query, SignatureVerifier, Source,
};
use sha2::{Digest, Sha256};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/index")
            .join(name),
    )
    .unwrap()
}

/// A stand-in for a Sigstore bundle: the SHA-256 it covers and who signed it.
fn sign(bytes: &[u8], identity: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "sha256": hex::encode(Sha256::digest(bytes)),
        "identity": identity,
        "issuer": GITHUB_ACTIONS_ISSUER,
    }))
    .unwrap()
}

/// Checks the stand-in bundles the way Sigstore checks real ones: the digest
/// must match the bytes and the signer must be the pinned identity.
struct FakeVerifier;

impl SignatureVerifier for FakeVerifier {
    fn verify_blob(
        &self,
        artifact: &[u8],
        bundle: &[u8],
        signer: &Identity,
    ) -> Result<(), SignatureError> {
        let bundle: serde_json::Value =
            serde_json::from_slice(bundle).map_err(|e| SignatureError::Malformed(e.to_string()))?;
        if bundle["sha256"] != hex::encode(Sha256::digest(artifact)) {
            return Err(SignatureError::Invalid("digest mismatch".into()));
        }
        if bundle["identity"] != signer.identity.as_str()
            || bundle["issuer"] != signer.issuer.as_str()
        {
            return Err(SignatureError::Invalid(format!(
                "signed by {}, not {}",
                bundle["identity"], signer.identity
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Default)]
struct FakeFetcher {
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
}

impl FakeFetcher {
    fn put(&self, url: &str, bytes: Vec<u8>) {
        self.files.lock().unwrap().insert(url.into(), bytes);
    }
    fn remove(&self, url: &str) {
        self.files.lock().unwrap().remove(url);
    }
    /// Serves an index and revocations at `base`, signed by `identity`.
    fn serve(&self, base: &str, index: &[u8], revocations: &[u8], identity: &str) {
        self.put(&format!("{base}/index.json"), index.to_vec());
        self.put(
            &format!("{base}/index.json.sigstore.json"),
            sign(index, identity),
        );
        self.put(&format!("{base}/revocations.json"), revocations.to_vec());
        self.put(
            &format!("{base}/revocations.json.sigstore.json"),
            sign(revocations, identity),
        );
    }
}

#[async_trait::async_trait]
impl Fetcher for FakeFetcher {
    async fn get(&self, url: &str) -> Result<Vec<u8>, String> {
        self.files
            .lock()
            .unwrap()
            .get(url)
            .cloned()
            .ok_or_else(|| format!("HTTP 404 for {url}"))
    }
}

const OFFICIAL_BASE: &str = "https://ostrium-labs.github.io/loams-marketplace";

fn official_world() -> FakeFetcher {
    let fetcher = FakeFetcher::default();
    fetcher.serve(
        OFFICIAL_BASE,
        &fixture("index.json"),
        &fixture("revocations.json"),
        OFFICIAL_IDENTITY,
    );
    fetcher
}

#[tokio::test]
async fn the_official_source_is_built_in_and_pinned() {
    let dir = tempfile::tempdir().unwrap();
    let market = Marketplace::open(dir.path(), FakeFetcher::default(), FakeVerifier).unwrap();
    let sources = market.list_sources();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].source.name, OFFICIAL_NAME);
    assert_eq!(sources[0].source.url, OFFICIAL_URL);
    assert_eq!(sources[0].source.identity, OFFICIAL_IDENTITY);
    assert!(sources[0].source.official);
}

#[tokio::test]
async fn a_verified_index_is_loaded() {
    let dir = tempfile::tempdir().unwrap();
    let mut market = Marketplace::open(dir.path(), official_world(), FakeVerifier).unwrap();
    market.refresh(OFFICIAL_NAME).await.unwrap();
    let info = &market.list_sources()[0];
    assert_eq!(info.listings, 6);
    assert!(info.error.is_none());
    assert!(info.refreshed_ms > 0);
}

#[tokio::test]
async fn index_signature_required() {
    let dir = tempfile::tempdir().unwrap();
    let fetcher = official_world();
    fetcher.remove(&format!("{OFFICIAL_BASE}/index.json.sigstore.json"));
    let mut market = Marketplace::open(dir.path(), fetcher.clone(), FakeVerifier).unwrap();
    let error = market.refresh(OFFICIAL_NAME).await.unwrap_err();
    assert!(
        matches!(
            error,
            MarketError::Signature {
                error: SignatureError::Missing(_),
                ..
            }
        ),
        "{error}"
    );
    assert!(market.verified(OFFICIAL_NAME).is_none());
    assert!(market.search(&Query::default()).is_empty());
    assert!(market.list_sources()[0].error.is_some());

    // An unsigned revocation list is refused the same way.
    let fetcher = official_world();
    fetcher.remove(&format!("{OFFICIAL_BASE}/revocations.json.sigstore.json"));
    let mut market = Marketplace::open(dir.path(), fetcher, FakeVerifier).unwrap();
    assert!(market.refresh(OFFICIAL_NAME).await.is_err());

    // A tampered index fails its signature.
    let fetcher = official_world();
    let mut index = fixture("index.json");
    index.extend_from_slice(b" ");
    fetcher.put(&format!("{OFFICIAL_BASE}/index.json"), index);
    let mut market = Marketplace::open(dir.path(), fetcher, FakeVerifier).unwrap();
    let error = market.refresh(OFFICIAL_NAME).await.unwrap_err();
    assert!(
        matches!(
            error,
            MarketError::Signature {
                error: SignatureError::Invalid(_),
                ..
            }
        ),
        "{error}"
    );
}

#[tokio::test]
async fn index_identity_pinned() {
    let dir = tempfile::tempdir().unwrap();
    let fetcher = FakeFetcher::default();
    // Validly signed, but by another workflow.
    fetcher.serve(
        OFFICIAL_BASE,
        &fixture("index.json"),
        &fixture("revocations.json"),
        "https://github.com/attacker/fork/.github/workflows/publish.yml@refs/heads/main",
    );
    let mut market = Marketplace::open(dir.path(), fetcher.clone(), FakeVerifier).unwrap();
    assert!(market.refresh(OFFICIAL_NAME).await.is_err());
    assert!(market.verified(OFFICIAL_NAME).is_none());

    // A private source is pinned to its own identity, and only verifies with it.
    let base = "https://index.example.org/agents";
    let identity =
        "https://github.com/example-org/agents-index/.github/workflows/publish.yml@refs/heads/main";
    fetcher.serve(
        base,
        &fixture("index.json"),
        &fixture("revocations.json"),
        identity,
    );
    let wrong = Source {
        name: "example".into(),
        url: format!("{base}/index.json"),
        identity: OFFICIAL_IDENTITY.into(),
        issuer: GITHUB_ACTIONS_ISSUER.into(),
        official: false,
    };
    assert!(market.add_source(wrong.clone()).await.is_err());
    assert_eq!(
        market.list_sources().len(),
        1,
        "a failed source is not kept"
    );
    let right = Source {
        identity: identity.into(),
        ..wrong
    };
    let info = market.add_source(right).await.unwrap();
    assert_eq!(info.listings, 6);
    assert_eq!(market.list_sources().len(), 2);
}

#[tokio::test]
async fn a_cache_edited_on_disk_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut market = Marketplace::open(dir.path(), official_world(), FakeVerifier).unwrap();
    market.refresh(OFFICIAL_NAME).await.unwrap();
    drop(market);
    let cached = dir.path().join("marketplace/cache/official/index.json");
    let mut bytes = std::fs::read(&cached).unwrap();
    bytes.extend_from_slice(b"\n");
    std::fs::write(&cached, bytes).unwrap();
    let market = Marketplace::open(dir.path(), FakeFetcher::default(), FakeVerifier).unwrap();
    assert!(market.verified(OFFICIAL_NAME).is_none());
    assert!(market.list_sources()[0].error.is_some());
}

#[tokio::test]
async fn a_failed_refresh_keeps_the_last_verified_index() {
    let dir = tempfile::tempdir().unwrap();
    let fetcher = official_world();
    let mut market = Marketplace::open(dir.path(), fetcher.clone(), FakeVerifier).unwrap();
    market.refresh(OFFICIAL_NAME).await.unwrap();
    fetcher.remove(&format!("{OFFICIAL_BASE}/index.json.sigstore.json"));
    assert!(market.refresh(OFFICIAL_NAME).await.is_err());
    assert_eq!(market.search(&Query::default()).len(), 6);
    assert!(market.list_sources()[0].error.is_some());
}

#[tokio::test]
async fn search_filters_by_kind_framework_level() {
    let dir = tempfile::tempdir().unwrap();
    let mut market = Marketplace::open(dir.path(), official_world(), FakeVerifier).unwrap();
    market.refresh(OFFICIAL_NAME).await.unwrap();
    let ids =
        |query: Query| -> Vec<String> { market.search(&query).into_iter().map(|s| s.id).collect() };

    assert_eq!(ids(Query::default()).len(), 6);
    assert_eq!(
        ids(Query {
            kinds: vec![Kind::McpServer],
            ..Query::default()
        }),
        ["ostrium/github-mcp-server", "ostrium/plane-mcp-server"]
    );
    assert_eq!(
        ids(Query {
            kinds: vec![Kind::Harness],
            ..Query::default()
        }),
        ["ostrium/claude-code", "vercel-labs/fx"]
    );
    assert_eq!(
        ids(Query {
            frameworks: vec!["mastra".into()],
            ..Query::default()
        }),
        ["ostrium/loams-starter"]
    );
    assert_eq!(
        ids(Query {
            frameworks: vec!["pydantic-ai".into()],
            languages: vec!["python".into()],
            ..Query::default()
        }),
        ["example-org/research-assistant"]
    );
    // The starter's latest (0.1.1) passed Compatible; nothing reached Native.
    assert_eq!(
        ids(Query {
            min_level: Some(Level::Compatible),
            ..Query::default()
        }),
        ["ostrium/loams-starter"]
    );
    assert!(
        ids(Query {
            min_level: Some(Level::Native),
            ..Query::default()
        })
        .is_empty()
    );
    assert_eq!(
        ids(Query {
            tiers: vec![Tier::Community],
            ..Query::default()
        }),
        ["example-org/research-assistant", "vercel-labs/fx"]
    );
    // Text matches the id, title, summary and tool names.
    assert_eq!(
        ids(Query {
            text: "merge_pull_request".into(),
            ..Query::default()
        }),
        ["ostrium/github-mcp-server"]
    );
    assert_eq!(
        ids(Query {
            text: "PLANE hosted".into(),
            ..Query::default()
        }),
        ["ostrium/plane-mcp-server"]
    );
    assert!(
        ids(Query {
            source: Some("elsewhere".into()),
            ..Query::default()
        })
        .is_empty()
    );
}

#[tokio::test]
async fn get_listing_reads_versions_and_their_revocations() {
    let dir = tempfile::tempdir().unwrap();
    let mut market = Marketplace::open(dir.path(), official_world(), FakeVerifier).unwrap();
    market.refresh(OFFICIAL_NAME).await.unwrap();
    let view = market
        .get_listing("ostrium/loams-starter", None, None)
        .unwrap();
    assert_eq!(view.version.version, "0.1.1");
    assert!(view.summary.revocation.is_none(), "0.1.1 is not revoked");
    let versions: Vec<(&str, Option<Severity>)> = view
        .versions
        .iter()
        .map(|(v, r)| (v.version.as_str(), r.as_ref().map(|r| r.severity)))
        .collect();
    assert_eq!(
        versions,
        [("0.1.0", Some(Severity::Disable)), ("0.1.1", None)]
    );
    let old = market
        .get_listing("ostrium/loams-starter", Some("0.1.0"), Some(OFFICIAL_NAME))
        .unwrap();
    assert_eq!(old.version.listing["version"], "0.1.0");
    assert!(matches!(
        market.get_listing("ostrium/loams-starter", Some("9.9.9"), None),
        Err(MarketError::NoVersion { .. })
    ));
    assert!(matches!(
        market.get_listing("ostrium/nothing", None, None),
        Err(MarketError::NoListing(_))
    ));
    let github = market.search(&Query {
        text: "github".into(),
        kinds: vec![Kind::McpServer],
        ..Query::default()
    });
    assert_eq!(
        github[0].revocation.as_ref().unwrap().severity,
        Severity::Warn
    );
}

#[tokio::test]
async fn revocations_checked_on_start() {
    let dir = tempfile::tempdir().unwrap();
    let mut market = Marketplace::open(dir.path(), official_world(), FakeVerifier).unwrap();
    market.refresh(OFFICIAL_NAME).await.unwrap();
    drop(market);

    // agentd restarts with no network: the verified cache is enough.
    let market = Marketplace::open(dir.path(), FakeFetcher::default(), FakeVerifier).unwrap();
    let installed = [
        Installed {
            source: OFFICIAL_NAME.into(),
            id: "ostrium/loams-starter".into(),
            version: "0.1.0".into(),
        },
        Installed {
            source: OFFICIAL_NAME.into(),
            id: "ostrium/loams-starter".into(),
            version: "0.1.1".into(),
        },
        Installed {
            source: OFFICIAL_NAME.into(),
            id: "ostrium/github-mcp-server".into(),
            version: "2.0.3".into(),
        },
    ];
    let findings = market.check_revocations(&installed);
    let summary: Vec<(&str, &str, bool)> = findings
        .iter()
        .map(|f| {
            (
                f.installed.id.as_str(),
                f.installed.version.as_str(),
                f.disables(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("ostrium/loams-starter", "0.1.0", true),
            ("ostrium/github-mcp-server", "2.0.3", false)
        ]
    );
}

#[tokio::test]
async fn sources_persist_and_the_official_one_stays() {
    let dir = tempfile::tempdir().unwrap();
    let fetcher = official_world();
    let base = "https://index.example.org/agents";
    let identity =
        "https://github.com/example-org/agents-index/.github/workflows/publish.yml@refs/heads/main";
    fetcher.serve(
        base,
        &fixture("index.json"),
        &fixture("revocations.json"),
        identity,
    );
    let mut market = Marketplace::open(dir.path(), fetcher.clone(), FakeVerifier).unwrap();
    market
        .add_source(Source {
            name: "example".into(),
            url: format!("{base}/index.json"),
            identity: identity.into(),
            issuer: GITHUB_ACTIONS_ISSUER.into(),
            official: false,
        })
        .await
        .unwrap();
    assert!(matches!(
        market.remove_source(OFFICIAL_NAME),
        Err(MarketError::Source(_))
    ));
    drop(market);
    let mut market = Marketplace::open(dir.path(), fetcher, FakeVerifier).unwrap();
    let names: Vec<String> = market
        .list_sources()
        .into_iter()
        .map(|s| s.source.name)
        .collect();
    assert_eq!(names, [OFFICIAL_NAME, "example"]);
    assert_eq!(
        market.list_sources()[1].listings,
        6,
        "verified from the cache"
    );
    market.remove_source("example").unwrap();
    assert!(!dir.path().join("marketplace/cache/example").exists());
    assert_eq!(market.list_sources().len(), 1);
}

#[tokio::test]
async fn bad_sources_are_refused_before_any_fetch() {
    let dir = tempfile::tempdir().unwrap();
    let mut market = Marketplace::open(dir.path(), FakeFetcher::default(), FakeVerifier).unwrap();
    for source in [
        Source {
            name: "Bad Name".into(),
            url: "https://x.example/index.json".into(),
            identity: "i".into(),
            issuer: "j".into(),
            official: false,
        },
        Source {
            name: "plain".into(),
            url: "http://x.example/index.json".into(),
            identity: "i".into(),
            issuer: "j".into(),
            official: false,
        },
        Source {
            name: "anon".into(),
            url: "https://x.example/index.json".into(),
            identity: String::new(),
            issuer: "j".into(),
            official: false,
        },
    ] {
        assert!(matches!(
            market.add_source(source).await,
            Err(MarketError::Source(_))
        ));
    }
}
