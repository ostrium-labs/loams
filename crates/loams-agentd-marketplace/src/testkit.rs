//! A marketplace world for tests (feature `testkit`): a signed official
//! index served by a fake fetcher, a verifier that checks stand-in bundles
//! the way Sigstore checks real ones, npm tarballs with matching integrity
//! and provenance, and an npm that lays the package out. agentd's sessions
//! and Connect tests use it, so they need neither the network nor Sigstore.
//!
//! Without `agentd` only the signing half is here: [`sign`] and
//! [`FakeVerifier`], the ring-signed index v2 of [`signed_v2_index`] and its
//! [`TestKey`] (OP2-B Task 3), and with `fetch` the [`FakeFetcher`] (OP2-B
//! Task 1).
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[cfg(feature = "fetch")]
use std::collections::HashMap;
#[cfg(feature = "agentd")]
use std::path::{Path, PathBuf};
#[cfg(feature = "fetch")]
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use serde_json::{Value, json};
#[cfg(feature = "agentd")]
use sha2::Sha512;
use sha2::{Digest, Sha256};

#[cfg(feature = "fetch")]
use crate::fetch::Fetcher;
#[cfg(feature = "agentd")]
use crate::install::Npm;
#[cfg(feature = "agentd")]
use crate::provenance::{SLSA_V1, npm_integrity};
#[cfg(feature = "agentd")]
use crate::service::{Deps, MemoryVault, NPM_REGISTRY, attestations_url, tarball_url};
use crate::signed::{IndexFiles, TrustAnchor};
#[cfg(feature = "agentd")]
use crate::sources::OFFICIAL_IDENTITY;
use crate::verify::{GITHUB_ACTIONS_ISSUER, Identity, SignatureError, SignatureVerifier};

pub const OFFICIAL_BASE: &str = "https://ostrium-labs.github.io/loams-marketplace";
#[cfg(feature = "agentd")]
/// The ACP harness listing [`World`] serves.
pub const HARNESS_ID: &str = "testkit/echo-acp";
#[cfg(feature = "agentd")]
/// The local MCP server listing [`World`] serves (one secret, `token`).
pub const MCP_ID: &str = "testkit/notes-mcp";
pub const REPOSITORY: &str = "https://github.com/ostrium-labs/testkit";

#[cfg(feature = "agentd")]
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

#[cfg(feature = "fetch")]
/// Serves bytes by URL and counts requests.
#[derive(Clone, Default)]
pub struct FakeFetcher {
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    calls: Arc<Mutex<HashMap<String, usize>>>,
}

#[cfg(feature = "fetch")]
impl std::fmt::Debug for FakeFetcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeFetcher").finish_non_exhaustive()
    }
}

#[cfg(feature = "fetch")]
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

#[cfg(feature = "fetch")]
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

#[cfg(feature = "agentd")]
/// Lays out `node_modules/<name>/` with a `package.json` and the entrypoint,
/// as npm would for a package with no dependencies.
#[derive(Debug, Default)]
pub struct FakeNpm {
    /// npm package name → entrypoint.
    pub packages: HashMap<String, String>,
}

#[cfg(feature = "agentd")]
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

#[cfg(feature = "agentd")]
/// The tarball bytes of npm package `name`.
pub fn tarball(name: &str) -> Vec<u8> {
    format!("testkit tarball {name}\n").into_bytes()
}

#[cfg(feature = "agentd")]
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

#[cfg(feature = "agentd")]
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

#[cfg(feature = "agentd")]
/// An ACP harness on the node runtime, version 1.0.0.
pub fn harness_listing() -> Value {
    harness_listing_at("1.0.0")
}

#[cfg(feature = "agentd")]
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

#[cfg(feature = "agentd")]
/// Every version of the harness, ascending.
pub const HARNESS_VERSIONS: &[&str] = &["1.0.0", "1.1.0"];

#[cfg(feature = "agentd")]
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

#[cfg(feature = "agentd")]
fn sha256_hex(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

#[cfg(feature = "agentd")]
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

#[cfg(feature = "agentd")]
/// A data directory, the official source served and signed, and the
/// testkit packages on the fake registry.
#[derive(Debug, Clone)]
pub struct World {
    pub data_dir: PathBuf,
    pub fetcher: FakeFetcher,
}

#[cfg(feature = "agentd")]
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

/// An ECDSA P-256 key made with `ring`, which signs the cosign
/// `sign-blob --key` bundle shape (OP2-B Task 0 ruling) that
/// [`crate::verify::KeyVerifier`] checks.
pub struct TestKey {
    pair: ring::signature::EcdsaKeyPair,
}

impl std::fmt::Debug for TestKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestKey").finish_non_exhaustive()
    }
}

/// `SubjectPublicKeyInfo` DER of a P-256 key, before its 65-byte point.
const P256_SPKI_PREFIX: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];

impl TestKey {
    pub fn generate() -> TestKey {
        use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair};
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng)
            .unwrap();
        TestKey { pair }
    }

    fn spki_der(&self) -> Vec<u8> {
        use ring::signature::KeyPair as _;
        let mut der = P256_SPKI_PREFIX.to_vec();
        der.extend_from_slice(self.pair.public_key().as_ref());
        der
    }

    /// The public key as `cosign.pub` holds it.
    pub fn pem(&self) -> String {
        let body = base64::engine::general_purpose::STANDARD.encode(self.spki_der());
        let mut pem = String::from("-----BEGIN PUBLIC KEY-----\n");
        for line in body.as_bytes().chunks(64) {
            pem.push_str(std::str::from_utf8(line).unwrap());
            pem.push('\n');
        }
        pem.push_str("-----END PUBLIC KEY-----\n");
        pem
    }

    /// The trust anchor for this key.
    pub fn anchor(&self) -> TrustAnchor {
        TrustAnchor::PublicKey { pem: self.pem() }
    }

    /// A cosign v3.0.6 `sign-blob --key` bundle over `bytes`.
    pub fn sign_bundle(&self, bytes: &[u8]) -> Vec<u8> {
        let b64 = base64::engine::general_purpose::STANDARD;
        let rng = ring::rand::SystemRandom::new();
        let signature = self.pair.sign(&rng, bytes).unwrap();
        serde_json::to_vec(&json!({
            "mediaType": "application/vnd.dev.sigstore.bundle.v0.3+json",
            "verificationMaterial": {
                "publicKey": { "hint": b64.encode(Sha256::digest(self.spki_der())) }
            },
            "messageSignature": {
                "messageDigest": { "algorithm": "SHA2_256", "digest": b64.encode(Sha256::digest(bytes)) },
                "signature": b64.encode(signature.as_ref()),
            }
        }))
        .unwrap()
    }

    /// The index and revocations, both signed with this key.
    pub fn sign_files(&self, index: &[u8], revocations: &[u8]) -> IndexFiles {
        IndexFiles {
            index: index.to_vec(),
            index_bundle: self.sign_bundle(index),
            revocations: revocations.to_vec(),
            revocations_bundle: self.sign_bundle(revocations),
        }
    }
}

/// Whether `namespace` is reserved for Ostrium (Decision 7).
fn reserved_namespace(namespace: &str) -> bool {
    matches!(namespace, "ostrium" | "loams")
        || namespace.starts_with("ostrium-")
        || namespace.starts_with("loams-")
}

/// Builds index v2 bytes. Each item is a listing document, or
/// `{"listing": <doc>, "tier"?: "first-party"|"verified"|"community",
/// "yanked"?: "<reason>"}`. Without a `tier`, a reserved namespace is
/// `first-party` and any other `community`. Versions are sorted ascending;
/// `latest` is the highest release (or the highest version when all are
/// prereleases) and the entry's tier is `latest`'s.
pub fn index_v2(listings: &[Value]) -> Vec<u8> {
    let mut entries: Vec<(String, Vec<(semver::Version, Value)>)> = Vec::new();
    for item in listings {
        let (listing, tier, yanked) = match item.get("listing") {
            Some(listing) => (
                listing.clone(),
                item.get("tier").cloned(),
                item.get("yanked").cloned(),
            ),
            None => (item.clone(), None, None),
        };
        let id = listing["id"].as_str().unwrap().to_owned();
        let version: semver::Version = listing["version"].as_str().unwrap().parse().unwrap();
        let namespace = id.split('/').next().unwrap();
        let tier = tier.unwrap_or_else(|| {
            json!(if reserved_namespace(namespace) {
                "first-party"
            } else {
                "community"
            })
        });
        let mut row = json!({
            "version": listing["version"],
            "tier": tier,
            "listing_sha256": hex::encode(Sha256::digest(crate::index::canonical(&listing).as_bytes())),
            "listing": listing,
        });
        if let Some(reason) = yanked {
            row["yanked"] = reason;
        }
        match entries.iter_mut().find(|(e, _)| *e == id) {
            Some((_, rows)) => rows.push((version, row)),
            None => entries.push((id, vec![(version, row)])),
        }
    }
    let listings: Vec<Value> = entries
        .into_iter()
        .map(|(id, mut rows)| {
            rows.sort_by(|a, b| a.0.cmp(&b.0));
            let latest = rows
                .iter()
                .rev()
                .find(|(v, _)| v.pre.is_empty())
                .unwrap_or_else(|| rows.last().unwrap());
            let doc = &latest.1["listing"];
            json!({
                "id": id,
                "kind": doc["kind"],
                "title": doc["title"],
                "summary": doc["summary"],
                "tier": latest.1["tier"],
                "latest": latest.1["version"],
                "versions": rows.iter().map(|(_, row)| row.clone()).collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::to_vec_pretty(&json!({
        "$schema": "https://loams.dev/schemas/index/v2.json",
        "index_version": 2,
        "source_commit": "0".repeat(40),
        "listings": listings,
    }))
    .unwrap()
}

/// An index v2 of `listings` (see [`index_v2`]) and `revocations`
/// (revocations v1 JSON), both signed with a fresh `ring` P-256 key in the
/// cosign key-bundle shape, and the key's trust anchor. For OP2-A's tests.
pub fn signed_v2_index(listings: &[Value], revocations: &Value) -> (IndexFiles, TrustAnchor) {
    let key = TestKey::generate();
    let files = key.sign_files(
        &index_v2(listings),
        &serde_json::to_vec(revocations).unwrap(),
    );
    (files, key.anchor())
}
