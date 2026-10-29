//! A marketplace world for tests (feature `testkit`): a signed official
//! index served by a fake fetcher, a verifier that checks stand-in bundles
//! the way Sigstore checks real ones, npm tarballs with matching integrity
//! and provenance, and an npm that lays the package out. agentd's sessions
//! and Connect tests use it, so they need neither the network nor Sigstore.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use serde_json::{Value, json};
use sha2::{Digest, Sha256, Sha512};

use crate::install::Npm;
use crate::market::Fetcher;
use crate::provenance::{SLSA_V1, npm_integrity};
use crate::service::{Deps, MemoryVault, NPM_REGISTRY, attestations_url, tarball_url};
use crate::sources::OFFICIAL_IDENTITY;
use crate::verify::{GITHUB_ACTIONS_ISSUER, Identity, SignatureError, SignatureVerifier};

pub const OFFICIAL_BASE: &str = "https://ostrium-labs.github.io/loams-marketplace";
/// The ACP harness listing [`World`] serves.
pub const HARNESS_ID: &str = "testkit/echo-acp";
/// The local MCP server listing [`World`] serves (one secret, `token`).
pub const MCP_ID: &str = "testkit/notes-mcp";
pub const REPOSITORY: &str = "https://github.com/ostrium-labs/testkit";

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/index")
            .join(name),
    )
    .unwrap()
}

/// A stand-in Sigstore bundle over `bytes`, signed by `identity`.
pub fn sign(bytes: &[u8], identity: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "sha256": hex::encode(Sha256::digest(bytes)),
        "identity": identity,
        "issuer": GITHUB_ACTIONS_ISSUER,
    }))
    .unwrap()
}

/// Checks stand-in index bundles by digest and signer, and stand-in
/// provenance bundles (DSSE envelopes) by their `sha512` subject.
#[derive(Debug, Default, Clone, Copy)]
pub struct FakeVerifier;

impl SignatureVerifier for FakeVerifier {
    fn verify_blob(
        &self,
        artifact: &[u8],
        bundle: &[u8],
        signer: &Identity,
    ) -> Result<(), SignatureError> {
        let bundle: Value =
            serde_json::from_slice(bundle).map_err(|e| SignatureError::Malformed(e.to_string()))?;
        if let Some(signed) = bundle["testkitSigner"].as_str() {
            if signed != signer.identity {
                return Err(SignatureError::Invalid(format!("signed by {signed}")));
            }
            return Ok(());
        }
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

/// Serves bytes by URL and counts requests.
#[derive(Clone, Default)]
pub struct FakeFetcher {
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    calls: Arc<Mutex<HashMap<String, usize>>>,
}

impl std::fmt::Debug for FakeFetcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeFetcher").finish_non_exhaustive()
    }
}

impl FakeFetcher {
    pub fn put(&self, url: &str, bytes: Vec<u8>) {
        self.files.lock().unwrap().insert(url.into(), bytes);
    }

    /// How many times `url` was requested.
    pub fn calls(&self, url: &str) -> usize {
        self.calls.lock().unwrap().get(url).copied().unwrap_or(0)
    }

    /// Serves an index and revocations at `base`, signed by `identity`.
    pub fn serve(&self, base: &str, index: &[u8], revocations: &[u8], identity: &str) {
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
        *self.calls.lock().unwrap().entry(url.into()).or_default() += 1;
        self.files
            .lock()
            .unwrap()
            .get(url)
            .cloned()
            .ok_or_else(|| format!("HTTP 404 for {url}"))
    }
}

/// Lays out `node_modules/<name>/` with a `package.json` and the entrypoint,
/// as npm would for a package with no dependencies.
#[derive(Debug, Default)]
pub struct FakeNpm {
    /// npm package name → entrypoint.
    pub packages: HashMap<String, String>,
}

impl Npm for FakeNpm {
    fn install(&self, tarball: &Path, prefix: &Path) -> Result<(), String> {
        let bytes = std::fs::read(tarball).map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&bytes);
        let name = text
            .strip_prefix("testkit tarball ")
            .ok_or("not a testkit tarball")?
            .trim()
            .to_owned();
        let entry = self.packages.get(&name).ok_or("unknown package")?;
        let dir = prefix.join("node_modules").join(&name);
        let file = dir.join(entry);
        std::fs::create_dir_all(file.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(
            dir.join("package.json"),
            json!({ "name": name, "version": "1.0.0" }).to_string(),
        )
        .map_err(|e| e.to_string())?;
        std::fs::write(&file, "// testkit\n").map_err(|e| e.to_string())
    }
}

/// The tarball bytes of npm package `name`.
pub fn tarball(name: &str) -> Vec<u8> {
    format!("testkit tarball {name}\n").into_bytes()
}

/// A stand-in SLSA provenance bundle for `bytes`, built from `repository`.
pub fn provenance_bundle(bytes: &[u8], repository: &str) -> Value {
    let statement = json!({
        "_type": "https://in-toto.io/Statement/v1",
        "subject": [{ "name": "pkg", "digest": { "sha512": hex::encode(Sha512::digest(bytes)) } }],
        "predicateType": SLSA_V1,
        "predicate": { "buildDefinition": { "externalParameters": { "workflow": {
            "repository": repository, "path": ".github/workflows/release.yml", "ref": "refs/heads/main"
        } } } }
    });
    json!({
        "testkitSigner": format!("{repository}/.github/workflows/release.yml@refs/heads/main"),
        "dsseEnvelope": {
            "payload": base64::engine::general_purpose::STANDARD.encode(statement.to_string()),
            "payloadType": "application/vnd.in-toto+json",
            "signatures": []
        }
    })
}

fn npm_listing(id: &str, kind: &str, package: &str, entry: &str, version: &str) -> Value {
    let bytes = tarball(package);
    json!({
        "id": id,
        "kind": kind,
        "version": version,
        "title": format!("Testkit {id}"),
        "summary": "A listing for agentd's tests.",
        "licence": "MIT",
        "pricing": "free",
        "source": { "repository": REPOSITORY, "commit": "0".repeat(40) },
        "publisher": { "name": "Testkit" },
        "artifact": { "type": "npm", "name": package, "version": version, "integrity": npm_integrity(&bytes) },
        "provenance": { "type": "npm-slsa", "ref": attestations_url(NPM_REGISTRY, package, version) },
        "runtime": { "kind": "node", "version": ">=22.13" },
        "entrypoint": entry,
    })
}

/// An ACP harness on the node runtime, version 1.0.0.
pub fn harness_listing() -> Value {
    harness_listing_at("1.0.0")
}

/// The harness at `version`; 1.1.0 also reaches `api.example.com`, so an
/// update to it widens the plan.
pub fn harness_listing_at(version: &str) -> Value {
    let mut listing = npm_listing(HARNESS_ID, "harness", "echo-acp", "bin/echo.js", version);
    listing["harness"] = json!({
        "auth": ["api-key"],
        "launch": { "command": "echo-acp", "protocol": "acp", "args": ["--acp"] }
    });
    if version != "1.0.0" {
        listing["place"] = json!({ "egress": ["api.example.com"] });
    }
    listing
}

/// Every version of the harness, ascending.
pub const HARNESS_VERSIONS: &[&str] = &["1.0.0", "1.1.0"];

/// A local MCP server on the node runtime, with a required secret.
pub fn mcp_listing() -> Value {
    let mut listing = npm_listing(MCP_ID, "mcp-server", "notes-mcp", "bin/notes.js", "1.0.0");
    listing["mcp_server"] = json!({
        "mode": "local", "transport": "stdio", "args": ["--stdio"],
        "server": { "name": "dev.loams.testkit/notes-mcp", "description": "Notes.", "version": "1.0.0" }
    });
    listing["tools"] =
        json!([{ "name": "read_note", "description": "Reads a note.", "risk": "read" }]);
    listing["config"] = json!([{
        "name": "token", "description": "The notes token.", "type": "string",
        "env": "NOTES_TOKEN", "required": true, "secret": true
    }]);
    listing
}

fn sha256_hex(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// The fixture index (`testdata/index`) with the testkit listings added.
pub fn index_bytes() -> Vec<u8> {
    let mut index: Value = serde_json::from_slice(&fixture("index.json")).unwrap();
    let harness: Vec<Value> = HARNESS_VERSIONS
        .iter()
        .map(|v| harness_listing_at(v))
        .collect();
    for versions in [harness, vec![mcp_listing()]] {
        let latest = versions.last().unwrap();
        let entry = json!({
            "id": latest["id"],
            "kind": latest["kind"],
            "title": latest["title"],
            "summary": latest["summary"],
            "tier": "community",
            "latest": latest["version"],
            "versions": versions.iter().map(|listing| json!({
                "version": listing["version"],
                "listing_sha256": sha256_hex(&crate::index::canonical(listing)),
                "listing": listing,
            })).collect::<Vec<_>>(),
        });
        index["listings"].as_array_mut().unwrap().push(entry);
    }
    serde_json::to_vec_pretty(&index).unwrap()
}

/// A data directory, the official source served and signed, and the
/// testkit packages on the fake registry.
#[derive(Debug, Clone)]
pub struct World {
    pub data_dir: PathBuf,
    pub fetcher: FakeFetcher,
}

impl World {
    pub fn new(data_dir: &Path) -> Self {
        let fetcher = FakeFetcher::default();
        fetcher.serve(
            OFFICIAL_BASE,
            &index_bytes(),
            &fixture("revocations.json"),
            OFFICIAL_IDENTITY,
        );
        for (package, version) in [
            ("echo-acp", "1.0.0"),
            ("echo-acp", "1.1.0"),
            ("notes-mcp", "1.0.0"),
        ] {
            let bytes = tarball(package);
            fetcher.put(
                &attestations_url(NPM_REGISTRY, package, version),
                serde_json::to_vec(&json!({ "attestations": [
                    { "predicateType": SLSA_V1, "bundle": provenance_bundle(&bytes, REPOSITORY) }
                ] }))
                .unwrap(),
            );
            fetcher.put(&tarball_url(NPM_REGISTRY, package, version), bytes);
        }
        Self {
            data_dir: data_dir.to_owned(),
            fetcher,
        }
    }

    /// Serves `revocations` (revocations v1 JSON) as the official list.
    pub fn revoke(&self, revocations: &Value) {
        let bytes = serde_json::to_vec(revocations).unwrap();
        self.fetcher.put(
            &format!("{OFFICIAL_BASE}/revocations.json.sigstore.json"),
            sign(&bytes, OFFICIAL_IDENTITY),
        );
        self.fetcher
            .put(&format!("{OFFICIAL_BASE}/revocations.json"), bytes);
    }

    pub fn deps(&self) -> Deps {
        let mut npm = FakeNpm::default();
        npm.packages.insert("echo-acp".into(), "bin/echo.js".into());
        npm.packages
            .insert("notes-mcp".into(), "bin/notes.js".into());
        Deps {
            data_dir: self.data_dir.clone(),
            fetcher: Arc::new(self.fetcher.clone()),
            verifier: Arc::new(FakeVerifier),
            npm: Arc::new(npm),
            vault: Arc::new(MemoryVault::default()),
            node: Some(PathBuf::from("/usr/bin/node")),
            registry: NPM_REGISTRY.into(),
            identity: None,
            machine_users: None,
        }
    }
}
