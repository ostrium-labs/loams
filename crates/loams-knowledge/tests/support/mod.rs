//! A stateful mock Walrus (publisher and aggregator on one loopback
//! server, after `loams-walrus`'s test support), and stand-in Sigstore
//! signing that checks what Sigstore checks: the digest and the signer.

#![allow(dead_code, clippy::unwrap_used)]

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use loams_knowledge::{
    GITHUB_ACTIONS_ISSUER, Identity, KnowledgeError, KnowledgeReader, Layout, ManifestSigner,
    Publisher, SignatureError, SignatureVerifier, TrustPolicy,
};
use loams_store::Store;
use loams_walrus::{BlobId, Walrus, WalrusConfig};
use sha2::{Digest, Sha256};
use url::Url;

pub const REPO: &str = "ostrium-labs/fixture";
pub const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
pub const COMMIT2: &str = "89abcdef0123456789abcdef0123456789abcdef";
pub const SIGNER: &str =
    "https://github.com/ostrium-labs/fixture/.github/workflows/knowledge.yml@refs/heads/main";

#[derive(Default)]
struct MockState {
    /// Patch id to bytes.
    patches: Mutex<HashMap<String, Bytes>>,
    /// Patch ids served with wrong bytes after they are stored.
    tampered: Mutex<HashSet<String>>,
    puts: AtomicUsize,
    reads: AtomicUsize,
    quilts: AtomicUsize,
}

/// A mock Walrus publisher and aggregator.
#[derive(Clone)]
pub struct MockWalrus {
    pub url: Url,
    state: Arc<MockState>,
}

impl MockWalrus {
    pub async fn start() -> Self {
        let state = Arc::new(MockState::default());
        let app = Router::new()
            .route("/v1/quilts", put(store_quilt))
            .route("/v1/blobs/by-quilt-patch-id/{patch}", get(read_patch))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            url: Url::parse(&format!("http://{addr}/")).unwrap(),
            state,
        }
    }

    /// Requests that reached the publisher.
    pub fn puts(&self) -> usize {
        self.state.puts.load(Ordering::SeqCst)
    }

    /// Patch reads that reached the aggregator.
    pub fn reads(&self) -> usize {
        self.state.reads.load(Ordering::SeqCst)
    }

    /// From now on, serve wrong bytes for this patch.
    pub fn tamper(&self, patch_id: &str) {
        self.state.tampered.lock().unwrap().insert(patch_id.into());
    }

    pub fn config(&self) -> WalrusConfig {
        WalrusConfig {
            enabled: true,
            network: loams_walrus::Network::Mainnet,
            aggregators: vec![self.url.clone()],
            publisher: Some(self.url.clone()),
            hedge_after_ms: 50,
            request_timeout_ms: 10_000,
            read_after_write_ms: 1_000,
            ..WalrusConfig::default()
        }
    }

    pub async fn client(&self) -> Walrus {
        Walrus::new(&self.config()).await.unwrap()
    }
}

async fn store_quilt(
    State(state): State<Arc<MockState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    state.puts.fetch_add(1, Ordering::SeqCst);
    let n = state.quilts.fetch_add(1, Ordering::SeqCst) as u8;
    let ct = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let boundary = ct.split("boundary=").nth(1).unwrap().to_string();
    let mut stored = Vec::new();
    for (id, data) in parse_multipart(&body, &boundary) {
        let patch = format!("p{}", hex::encode(Sha256::digest(format!("{n}/{id}"))));
        state.patches.lock().unwrap().insert(patch.clone(), data);
        stored.push(serde_json::json!({ "identifier": id, "quiltPatchId": patch }));
    }
    let blob = BlobId::from_bytes([n.wrapping_add(1); 32]).to_string();
    let answer = serde_json::json!({
        "blobStoreResult": { "newlyCreated": { "blobObject": {
            "id": "0x01", "blobId": blob, "size": body.len(),
            "storage": { "endEpoch": 42 } } } },
        "storedQuiltBlobs": stored,
    });
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        answer.to_string(),
    )
        .into_response()
}

async fn read_patch(State(state): State<Arc<MockState>>, Path(patch): Path<String>) -> Response {
    state.reads.fetch_add(1, Ordering::SeqCst);
    let data = state.patches.lock().unwrap().get(&patch).cloned();
    match data {
        Some(_) if state.tampered.lock().unwrap().contains(&patch) => {
            (StatusCode::OK, Bytes::from_static(b"tampered bytes")).into_response()
        }
        Some(d) => (StatusCode::OK, d).into_response(),
        None => (StatusCode::NOT_FOUND, "no such patch").into_response(),
    }
}

/// The parts of the body `loams-walrus` writes: one file field per
/// identifier.
fn parse_multipart(body: &[u8], boundary: &str) -> Vec<(String, Bytes)> {
    let delim = format!("--{boundary}");
    let text = body;
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = find(rest, delim.as_bytes()) {
        rest = &rest[at + delim.len()..];
        if rest.starts_with(b"--") {
            break;
        }
        let head_end = find(rest, b"\r\n\r\n").unwrap();
        let head = std::str::from_utf8(&rest[..head_end]).unwrap();
        let name = head
            .split("name=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let data_start = head_end + 4;
        let next = find(&rest[data_start..], delim.as_bytes()).unwrap();
        let data = &rest[data_start..data_start + next - 2]; // drop the \r\n
        out.push((name.to_string(), Bytes::copy_from_slice(data)));
        rest = &rest[data_start + next..];
    }
    out
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// A stand-in Sigstore bundle: the digest and the claimed signer.
#[derive(Debug, Clone)]
pub struct FakeSigner {
    pub identity: String,
}

impl Default for FakeSigner {
    fn default() -> Self {
        Self {
            identity: SIGNER.into(),
        }
    }
}

pub fn bundle_for(bytes: &[u8], identity: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "sha256": hex::encode(Sha256::digest(bytes)),
        "identity": identity,
        "issuer": GITHUB_ACTIONS_ISSUER,
    }))
    .unwrap()
}

#[async_trait]
impl ManifestSigner for FakeSigner {
    async fn sign(&self, bytes: &[u8]) -> Result<Vec<u8>, KnowledgeError> {
        Ok(bundle_for(bytes, &self.identity))
    }
}

/// Checks the stand-in bundles the way Sigstore checks real ones.
#[derive(Debug)]
pub struct FakeVerifier;

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

pub fn signer() -> Identity {
    Identity {
        identity: SIGNER.into(),
        issuer: GITHUB_ACTIONS_ISSUER.into(),
    }
}

/// A mock Walrus, an in-memory bucket, a publisher and a reader over both.
pub struct World {
    pub walrus: MockWalrus,
    pub store: Store,
    pub publisher: Publisher,
    pub reader: KnowledgeReader,
}

impl World {
    pub async fn new() -> Self {
        let walrus = MockWalrus::start().await;
        let store = Store::in_memory();
        let client = walrus.client().await;
        let publisher = Publisher::new(
            store.clone(),
            client.clone(),
            Arc::new(FakeSigner::default()),
            Layout::default(),
        );
        let reader = KnowledgeReader::new(
            store.clone(),
            client,
            Arc::new(FakeVerifier),
            TrustPolicy::new().pin(REPO, signer()),
            Layout::default(),
        );
        Self {
            walrus,
            store,
            publisher,
            reader,
        }
    }
}

/// A public fixture repository.
pub fn fixture() -> loams_knowledge::MemorySource {
    loams_knowledge::MemorySource::new()
        .file(".loams/knowledge.toml", "public = true\n")
        .file("README.md", "# Fixture\n\nThe fixture repository.\n")
        .file("AGENTS.md", "Agents: run `cargo test`.\n")
        .file(
            "docs/design/walrus.md",
            "# Walrus\n\nHedged reads, verified by SHA-256.\n",
        )
        .file("docs/plans/ms1.md", "MS1 seeds the marketplace.\n")
        .file("docs/logo.png", vec![0x89u8, b'P', b'N', b'G', 0, 1, 2])
        .file("proto/a/v1/a.proto", "syntax = \"proto3\";\n")
        .file("schemas/a.json", "{\"type\": \"object\"}\n")
        .file("src/main.rs", "fn main() {}\n")
        .file("docs/vendor/x.md", "vendored\n")
        .file("docs/gen.md", "<!-- @generated -->\nx\n")
        .file("docs/big.md", "a".repeat(1024 * 1024 + 1))
        .symlink("docs/link.md", "README.md")
}
