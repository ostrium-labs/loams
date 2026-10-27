//! MS1 Task 5 (D1812): a listing's walrus artifact location. The tarball and
//! its provenance are the real sigstore@3.1.0 ones (testdata/npm), so the
//! integrity and Sigstore checks after a Walrus read are the production
//! ones. Walrus is a mock aggregator on loopback, in the pattern of
//! crates/loams-walrus/tests/support; the registry is an in-memory fetcher.
#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use loams_agentd_marketplace::artifact::{
    ArtifactFetch, ArtifactSources, FetchError, NPM_REGISTRY, Origin, npm_tarball_url,
};
use loams_agentd_marketplace::install::{self, Context, InstallError, Installed, Layout, Npm};
use loams_agentd_marketplace::provenance::ProvenanceError;
use loams_agentd_marketplace::{Fetcher, SigstoreVerifier, listing};
use loams_walrus::{BlobId, Walrus, WalrusConfig};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

// --- fixtures -----------------------------------------------------------------

fn read(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/npm")
            .join(name),
    )
    .unwrap()
}

fn tarball() -> Vec<u8> {
    read("sigstore-3.1.0.tgz")
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn blob_id() -> BlobId {
    BlobId::from_bytes([7; 32])
}

const REGISTRY: &str = "https://registry.example";

fn registry_url() -> String {
    npm_tarball_url(REGISTRY, "sigstore", "3.1.0")
}

/// The sigstore@3.1.0 agent listing of tests/install.rs, with a walrus
/// location whose SHA-256 is `sha256`.
fn listing_with_walrus(sha256: &str) -> Value {
    let integrity = String::from_utf8(read("sigstore-3.1.0.integrity"))
        .unwrap()
        .trim()
        .to_owned();
    json!({
        "id": "ostrium/sigstore-agent",
        "kind": "agent",
        "version": "3.1.0",
        "title": "Sigstore agent",
        "source": { "repository": "https://github.com/sigstore/sigstore-js", "commit": "0".repeat(40) },
        "artifact": {
            "type": "npm", "name": "sigstore", "version": "3.1.0", "integrity": integrity,
            "locations": [{ "kind": "walrus", "blob_id": blob_id().as_str(), "sha256": sha256 }]
        },
        "provenance": { "type": "npm-slsa", "ref": "https://registry.npmjs.org/-/npm/v1/attestations/sigstore@3.1.0" },
        "runtime": { "kind": "node", "version": ">=22.13" },
        "entrypoint": "dist/index.js",
        "pricing": "free"
    })
}

fn listing() -> Value {
    listing_with_walrus(&sha256_hex(&tarball()))
}

// --- the mock aggregator ------------------------------------------------------

struct Aggregator {
    status: StatusCode,
    body: Bytes,
    hits: AtomicUsize,
    paths: Mutex<Vec<String>>,
}

async fn aggregate(State(state): State<Arc<Aggregator>>, req: axum::extract::Request) -> Response {
    state.hits.fetch_add(1, Ordering::SeqCst);
    state
        .paths
        .lock()
        .unwrap()
        .push(req.uri().path().to_owned());
    (state.status, state.body.clone()).into_response()
}

/// An aggregator answering every request with `status` and `body`, and a
/// Walrus client (enabled) that reads only from it.
async fn walrus_serving(status: StatusCode, body: Vec<u8>) -> (Walrus, Arc<Aggregator>) {
    let state = Arc::new(Aggregator {
        status,
        body: body.into(),
        hits: AtomicUsize::new(0),
        paths: Mutex::new(Vec::new()),
    });
    let app = Router::new()
        .fallback(any(aggregate))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let config: WalrusConfig = serde_json::from_value(json!({
        "enabled": true,
        "aggregators": [format!("http://{addr}")],
        "hedge_after_ms": 50,
        "request_timeout_ms": 5000,
    }))
    .unwrap();
    (Walrus::new(&config).await.unwrap(), state)
}

async fn walrus_disabled() -> Walrus {
    Walrus::new(&WalrusConfig::default()).await.unwrap()
}

// --- the registry and npm -----------------------------------------------------

#[derive(Default)]
struct Registry {
    files: HashMap<String, Vec<u8>>,
    calls: Mutex<Vec<String>>,
}

impl Registry {
    fn serving_tarball() -> Self {
        Self {
            files: HashMap::from([(registry_url(), tarball())]),
            ..Self::default()
        }
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl Fetcher for Registry {
    async fn get(&self, url: &str) -> Result<Vec<u8>, String> {
        self.calls.lock().unwrap().push(url.to_owned());
        self.files
            .get(url)
            .cloned()
            .ok_or_else(|| "404 Not Found".to_owned())
    }
}

struct FakeNpm;

impl Npm for FakeNpm {
    fn install(&self, tarball: &Path, prefix: &Path) -> Result<(), String> {
        assert!(tarball.is_file());
        for (path, text) in [
            (
                "node_modules/sigstore/package.json",
                r#"{"name":"sigstore","version":"3.1.0"}"#,
            ),
            (
                "node_modules/sigstore/dist/index.js",
                "module.exports = {};",
            ),
        ] {
            let path = prefix.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        Ok(())
    }
}

async fn install_with(
    layout: &Layout,
    listing: &Value,
    walrus: &Walrus,
    registry: &Registry,
    npm_registry: Option<&str>,
) -> Result<(Installed, Origin), InstallError> {
    install_with_bundles(
        layout,
        listing,
        walrus,
        registry,
        npm_registry,
        &[read("sigstore-3.1.0.slsa.sigstore.json")],
    )
    .await
}

async fn install_with_bundles(
    layout: &Layout,
    listing: &Value,
    walrus: &Walrus,
    registry: &Registry,
    npm_registry: Option<&str>,
    bundles: &[Vec<u8>],
) -> Result<(Installed, Origin), InstallError> {
    let verifier = SigstoreVerifier::production().unwrap();
    let fetch = ArtifactFetch {
        walrus,
        https: registry,
        npm_registry,
    };
    install::install_node_fetched(
        layout,
        listing,
        &fetch,
        bundles,
        &verifier,
        &FakeNpm,
        &Context {
            source: "official".into(),
            plan_digest: "ab".repeat(32),
            now_ms: 1_800_000_000_000,
        },
    )
    .await
}

fn new_layout() -> (tempfile::TempDir, Layout) {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path().join("agents"));
    (dir, layout)
}

// --- tests --------------------------------------------------------------------

#[test]
fn walrus_source_validates() {
    // The schema fixture with a walrus location (schemas/marketplace/testdata).
    let fixture: Value = serde_json::from_slice(
        &std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../schemas/marketplace/testdata/valid/agent-mastra-acp.json"),
        )
        .unwrap(),
    )
    .unwrap();
    listing::validate(&fixture).unwrap();
    let sources = ArtifactSources::package(&fixture, Some(NPM_REGISTRY)).unwrap();
    assert_eq!(sources.walrus.len(), 1);
    assert_eq!(
        sources.https.as_deref(),
        Some("https://registry.npmjs.org/@loams/starter-agent/-/starter-agent-0.1.0.tgz")
    );
    let blob = fixture["artifact"]["locations"][0]["blob_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(sources.walrus[0].blob_id.as_str(), blob);

    let sources = ArtifactSources::package(&listing(), Some(REGISTRY)).unwrap();
    assert_eq!(sources.walrus[0].blob_id, blob_id());
    assert_eq!(sources.https.as_deref(), Some(registry_url().as_str()));

    for (pointer, value) in [
        ("/artifact/locations/0/kind", json!("ipfs")),
        ("/artifact/locations/0/blob_id", json!("not a blob id")),
        // 43 characters, but the last carries bits past 32 bytes.
        (
            "/artifact/locations/0/blob_id",
            json!(format!("{}B", &blob[..42])),
        ),
        ("/artifact/locations/0/sha256", json!("ABC")),
        ("/artifact/locations", json!([])),
    ] {
        let mut doc = fixture.clone();
        *doc.pointer_mut(pointer).unwrap() = value.clone();
        assert!(
            listing::validate(&doc).is_err(),
            "{pointer} = {value} was accepted"
        );
    }
}

#[test]
fn npm_tarball_url_handles_scopes() {
    assert_eq!(
        npm_tarball_url(NPM_REGISTRY, "sigstore", "3.1.0"),
        "https://registry.npmjs.org/sigstore/-/sigstore-3.1.0.tgz"
    );
    assert_eq!(
        npm_tarball_url("https://r.example/", "@loams/starter-agent", "0.1.0"),
        "https://r.example/@loams/starter-agent/-/starter-agent-0.1.0.tgz"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn walrus_install_verifies_sha256_then_sigstore() {
    // Verified Walrus bytes install, and the registry is never asked.
    let (walrus, aggregator) = walrus_serving(StatusCode::OK, tarball()).await;
    let registry = Registry::serving_tarball();
    let (_dir, layout) = new_layout();
    let (installed, origin) = install_with(&layout, &listing(), &walrus, &registry, Some(REGISTRY))
        .await
        .unwrap();
    assert_eq!(
        origin,
        Origin::Walrus {
            blob_id: blob_id().to_string()
        }
    );
    assert!(registry.calls().is_empty());
    assert_eq!(
        aggregator.paths.lock().unwrap()[0],
        format!("/v1/blobs/{}", blob_id())
    );
    assert!(
        installed
            .built_by
            .starts_with("https://github.com/sigstore/sigstore-js/")
    );

    // Verified Walrus bytes with no provenance are refused: Sigstore is
    // never skipped because the bytes came from Walrus (D1459).
    let (_dir, layout) = new_layout();
    let error = install_with_bundles(&layout, &listing(), &walrus, &registry, Some(REGISTRY), &[])
        .await
        .unwrap_err();
    assert_eq!(error, InstallError::Provenance(ProvenanceError::Missing));
    assert!(layout.installed("ostrium/sigstore-agent").is_none());

    // Bytes that match the Walrus SHA-256 are still checked against the
    // artifact's integrity and provenance: a listing whose walrus digest
    // names a tampered tarball is refused after the Walrus read succeeds.
    let mut tampered = tarball();
    tampered[100] ^= 1;
    let (walrus, aggregator) = walrus_serving(StatusCode::OK, tampered.clone()).await;
    let (_dir, layout) = new_layout();
    let error = install_with(
        &layout,
        &listing_with_walrus(&sha256_hex(&tampered)),
        &walrus,
        &registry,
        Some(REGISTRY),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(
            error,
            InstallError::Provenance(ProvenanceError::IntegrityMismatch { .. })
        ),
        "{error:?}"
    );
    assert_eq!(aggregator.hits.load(Ordering::SeqCst), 1);
    assert!(registry.calls().is_empty());
    assert!(layout.installed("ostrium/sigstore-agent").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn walrus_digest_mismatch_refused() {
    // The aggregator sends other bytes; with no registry there is no fallback.
    let (walrus, aggregator) = walrus_serving(StatusCode::OK, b"evil".to_vec()).await;
    let registry = Registry::serving_tarball();
    let (_dir, layout) = new_layout();
    let error = install_with(&layout, &listing(), &walrus, &registry, None)
        .await
        .unwrap_err();
    assert!(
        matches!(&error, InstallError::Fetch(FetchError::WalrusIntegrity { blob_id: b, .. }) if *b == blob_id().to_string()),
        "{error:?}"
    );
    assert!(aggregator.hits.load(Ordering::SeqCst) >= 1);
    assert!(registry.calls().is_empty());
    assert!(layout.installed("ostrium/sigstore-agent").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn walrus_failure_falls_back_to_https() {
    for (status, body) in [
        (StatusCode::OK, b"evil".to_vec()),
        (StatusCode::NOT_FOUND, b"no such blob".to_vec()),
        (StatusCode::INTERNAL_SERVER_ERROR, b"down".to_vec()),
    ] {
        let (walrus, aggregator) = walrus_serving(status, body).await;
        let registry = Registry::serving_tarball();
        let (_dir, layout) = new_layout();
        let (installed, origin) =
            install_with(&layout, &listing(), &walrus, &registry, Some(REGISTRY))
                .await
                .unwrap_or_else(|e| panic!("{status}: {e}"));
        assert!(aggregator.hits.load(Ordering::SeqCst) >= 1, "{status}");
        assert_eq!(
            origin,
            Origin::Https {
                url: registry_url()
            },
            "{status}"
        );
        assert_eq!(registry.calls(), [registry_url()], "{status}");
        assert_eq!(installed.version, "3.1.0");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn walrus_disabled_uses_https() {
    let walrus = walrus_disabled().await;
    let registry = Registry::serving_tarball();
    let (_dir, layout) = new_layout();
    let (_, origin) = install_with(&layout, &listing(), &walrus, &registry, Some(REGISTRY))
        .await
        .unwrap();
    assert_eq!(
        origin,
        Origin::Https {
            url: registry_url()
        }
    );
    assert_eq!(registry.calls(), [registry_url()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn walrus_disabled_without_https_refused() {
    let walrus = walrus_disabled().await;
    let registry = Registry::serving_tarball();
    let (_dir, layout) = new_layout();
    let error = install_with(&layout, &listing(), &walrus, &registry, None)
        .await
        .unwrap_err();
    assert_eq!(
        error,
        InstallError::Fetch(FetchError::WalrusDisabled {
            blob_id: blob_id().to_string()
        })
    );
    assert!(registry.calls().is_empty());
    assert!(layout.installed("ostrium/sigstore-agent").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn https_failure_reports_walrus_cause() {
    let (walrus, _aggregator) = walrus_serving(StatusCode::OK, b"evil".to_vec()).await;
    let registry = Registry::default();
    let (_dir, layout) = new_layout();
    let error = install_with(&layout, &listing(), &walrus, &registry, Some(REGISTRY))
        .await
        .unwrap_err();
    let InstallError::Fetch(FetchError::Https { url, walrus, .. }) = &error else {
        panic!("{error:?}");
    };
    assert_eq!(*url, registry_url());
    assert!(walrus.as_deref().unwrap().contains("sha256"), "{error}");
}

#[test]
fn archive_file_sources_are_its_locations_then_its_url() {
    let sha = "213840ab495023fa2d584d2f93c121559fcacde0fb4793c6c20f7f751021a333";
    let listing = json!({
        "artifact": { "type": "archive", "name": "fx", "version": "0.0.14", "files": [
            { "platform": "linux-x86_64", "url": "https://example.com/fx.tar.gz", "sha256": sha,
              "locations": [{ "kind": "walrus", "blob_id": blob_id().as_str(), "sha256": sha }] }
        ]}
    });
    let sources = ArtifactSources::archive_file(&listing, "linux-x86_64").unwrap();
    assert_eq!(sources.walrus[0].sha256.to_hex(), sha);
    assert_eq!(
        sources.https.as_deref(),
        Some("https://example.com/fx.tar.gz")
    );
    assert_eq!(
        ArtifactSources::archive_file(&listing, "darwin-aarch64"),
        Err(FetchError::NoLocation)
    );
}
