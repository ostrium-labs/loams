//! The knowledge tools in the MCP shim (MS1 Task 7, D1813).

#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use axum::Router;
use axum::routing::get;
use loams_agentd_mcp::{Endpoint, LoamsDesktop, Origin, Tools};
use loams_knowledge::{
    Identity, KnowledgeManifest, KnowledgeReader, Layout, MANIFEST_SCHEMA, ManifestEntry,
    SignatureError, SignatureVerifier, TrustPolicy,
};
use loams_store::Store;
use loams_walrus::{BlobId, ContentDigest, QuiltPatchId, Walrus, WalrusConfig};
use serde_json::json;

const REPO: &str = "ostrium-labs/fixture";
const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
const README: &str = "# Fixture\n\nKnowledge for agents.\n";

/// A stand-in bundle: the artifact's sha256 and the signer's identity.
#[derive(Debug)]
struct StandIn;

impl SignatureVerifier for StandIn {
    fn verify_blob(
        &self,
        artifact: &[u8],
        bundle: &[u8],
        signer: &Identity,
    ) -> Result<(), SignatureError> {
        let want = format!(
            "{}|{}",
            ContentDigest::of(artifact).to_hex(),
            signer.identity
        );
        if bundle == want.as_bytes() {
            Ok(())
        } else {
            Err(SignatureError::Invalid("stand-in mismatch".into()))
        }
    }
}

fn signer() -> Identity {
    Identity {
        identity: "https://github.com/ostrium-labs/fixture/.github/workflows/k.yml@refs/heads/main"
            .into(),
        issuer: loams_knowledge::GITHUB_ACTIONS_ISSUER.into(),
    }
}

fn desktop() -> Arc<LoamsDesktop> {
    Arc::new(LoamsDesktop::new(
        Endpoint {
            port: None,
            token: None,
        },
        Origin::default(),
    ))
}

/// A bucket holding one signed manifest, and an aggregator serving its one
/// patch with `served` as the bytes.
async fn reader(served: &'static str) -> Arc<KnowledgeReader> {
    let app = Router::new().route(
        "/v1/blobs/by-quilt-patch-id/patch1",
        get(move || async move { served }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let walrus = Walrus::new(&WalrusConfig {
        enabled: true,
        network: loams_walrus::Network::Mainnet,
        aggregators: vec![url::Url::parse(&format!("http://{addr}/")).unwrap()],
        hedge_after_ms: 50,
        ..WalrusConfig::default()
    })
    .await
    .unwrap();

    let manifest = KnowledgeManifest {
        schema: MANIFEST_SCHEMA.into(),
        repo: REPO.into(),
        commit: COMMIT.into(),
        quilt_id: BlobId::from_bytes([1; 32]),
        created: "2026-10-26T00:00:00Z".into(),
        end_epoch: 42,
        entries: vec![ManifestEntry {
            path: "README.md".into(),
            quilt_patch_id: QuiltPatchId::parse("patch1").unwrap(),
            sha256: ContentDigest::of(README.as_bytes()),
            bytes: README.len() as u64,
        }],
    };
    let bytes = manifest.to_bytes().unwrap();
    let bundle = format!(
        "{}|{}",
        ContentDigest::of(&bytes).to_hex(),
        signer().identity
    );
    let store = Store::in_memory();
    let layout = Layout::default();
    store
        .put(&layout.bundle(REPO, COMMIT), bundle.into())
        .await
        .unwrap();
    store
        .put(&layout.manifest(REPO, COMMIT), bytes.into())
        .await
        .unwrap();
    store
        .put(&layout.latest(REPO), COMMIT.into())
        .await
        .unwrap();
    Arc::new(KnowledgeReader::new(
        store,
        walrus,
        Arc::new(StandIn),
        TrustPolicy::new().pin(REPO, signer()),
        layout,
    ))
}

#[tokio::test]
async fn knowledge_tools_absent_without_config() {
    let tools = Tools::new(desktop());
    assert!(!tools.list().iter().any(|t| t.name.starts_with("knowledge")));
    assert!(!tools.has("knowledge_get"));
    let err = tools
        .call(
            "knowledge_get",
            json!({ "repo": REPO, "path": "README.md" }),
        )
        .await
        .unwrap_err();
    assert!(err.contains("unknown tool"), "{err}");
}

#[tokio::test]
async fn knowledge_tools_are_read_only() {
    let tools = Tools::new(desktop()).with_knowledge(reader(README).await);
    let listed = tools.list();
    let knowledge: Vec<_> = listed
        .iter()
        .filter(|t| t.name.starts_with("knowledge"))
        .collect();
    let names: Vec<&str> = knowledge.iter().map(|t| t.name).collect();
    assert_eq!(
        names,
        ["knowledge_get", "knowledge_search"],
        "no write tool"
    );
    for t in knowledge {
        assert_eq!(t.annotations["risk"], "read", "{}", t.name);
        assert_eq!(t.annotations["readOnlyHint"], true, "{}", t.name);
        assert_eq!(t.annotations["destructiveHint"], false, "{}", t.name);
        assert_eq!(t.input_schema["type"], "object");
        assert_eq!(loams_agentd_mcp::risk_of(t.name), "read");
    }
    // Names fit the Anthropic and OpenAI tool-name rules (ruling 2).
    for t in &listed {
        assert!(
            t.name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
            "{}",
            t.name
        );
    }
    let mut unique: Vec<&str> = listed.iter().map(|t| t.name).collect();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), listed.len());
}

#[tokio::test]
async fn knowledge_get_through_shim() {
    let tools = Tools::new(desktop()).with_knowledge(reader(README).await);
    let got = tools
        .call(
            "knowledge_get",
            json!({ "repo": REPO, "path": "README.md" }),
        )
        .await
        .unwrap();
    assert_eq!(got["text"], README);
    assert_eq!(got["commit"], COMMIT);
    assert_eq!(got["sha256"], ContentDigest::of(README.as_bytes()).to_hex());
    let found = tools
        .call(
            "knowledge_search",
            json!({ "repo": REPO, "commit": COMMIT, "query": "agents" }),
        )
        .await
        .unwrap();
    assert_eq!(found["hits"][0]["path"], "README.md");
    assert_eq!(found["hits"][0]["snippet"], "Knowledge for agents.");
    let err = tools
        .call(
            "knowledge_get",
            json!({ "repo": REPO, "path": "README.md", "write": true }),
        )
        .await
        .unwrap_err();
    assert!(err.contains("invalid arguments"), "{err}");
}

#[tokio::test]
async fn knowledge_get_through_shim_refuses_wrong_bytes() {
    let tools = Tools::new(desktop()).with_knowledge(reader("# Not the fixture\n").await);
    let err = tools
        .call(
            "knowledge_get",
            json!({ "repo": REPO, "path": "README.md" }),
        )
        .await
        .unwrap_err();
    assert!(err.contains("expected sha256"), "{err}");
}
