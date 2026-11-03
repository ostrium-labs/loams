#![allow(clippy::unwrap_used)]

//! OP2-B Task 3: the signed index v2, `verify_listing`, `is_revoked` and
//! `resolve_depends_on`, as the operator consumes them.

use std::path::{Path, PathBuf};

use loams_agentd_marketplace::testkit::{TestKey, index_v2, signed_v2_index};
use loams_agentd_marketplace::verify::SignatureError;
use loams_agentd_marketplace::{
    IndexFiles, KeyVerifier, ListingKind, SignatureVerifier, SignedIndex, Tier, TrustAnchor,
    VerifyError, index, is_revoked, resolve_depends_on, verify_listing,
};
use semver::Version;
use serde_json::{Value, json};

fn crate_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn read(path: &str) -> Vec<u8> {
    std::fs::read(crate_dir().join(path)).unwrap()
}

fn v(text: &str) -> Version {
    text.parse().unwrap()
}

fn no_revocations() -> Value {
    json!({ "revocations": [] })
}

fn revocation(id: &str, versions: &str, severity: &str) -> Value {
    json!({ "id": id, "versions": versions, "severity": severity, "reason": "bad", "date": "2026-11-02" })
}

/// A v2 app at `id@version` with an `oci` package and no dependencies.
fn app(id: &str, version: &str) -> Value {
    let mut doc: Value = serde_json::from_slice(&read(
        "../../schemas/marketplace/testdata/v2/valid/app-oci.json",
    ))
    .unwrap();
    doc["id"] = json!(id);
    doc["version"] = json!(version);
    doc["depends_on"] = json!([]);
    doc
}

fn app_needing(id: &str, version: &str, deps: &[(&str, &str)]) -> Value {
    let mut doc = app(id, version);
    doc["depends_on"] = deps
        .iter()
        .map(|(id, req)| json!({ "id": id, "version": req }))
        .collect();
    doc
}

fn helm_app(id: &str) -> Value {
    let mut doc: Value = serde_json::from_slice(&read(
        "../../schemas/marketplace/testdata/v2/valid/app-helm.json",
    ))
    .unwrap();
    doc["id"] = json!(id);
    doc
}

fn signed(listings: &[Value], revocations: &Value) -> SignedIndex {
    let (files, anchor) = signed_v2_index(listings, revocations);
    SignedIndex::verify(&files, &anchor).unwrap()
}

/// The hand-written fixture (`testdata/index-v2`), signed with a fresh key.
fn fixture() -> SignedIndex {
    let key = TestKey::generate();
    let files = key.sign_files(
        &read("testdata/index-v2/index.v2.json"),
        &read("testdata/index-v2/revocations.json"),
    );
    SignedIndex::verify(&files, &key.anchor()).unwrap()
}

// Signatures.

#[test]
fn index_signature_required() {
    let (mut files, anchor) = signed_v2_index(&[app("acme/a", "1.0.0")], &no_revocations());
    files.index_bundle.clear();
    assert!(matches!(
        SignedIndex::verify(&files, &anchor),
        Err(VerifyError::Signature {
            file: "index.v2.json",
            error: SignatureError::Missing(_)
        })
    ));
}

#[test]
fn revocations_signature_required() {
    let (mut files, anchor) = signed_v2_index(&[app("acme/a", "1.0.0")], &no_revocations());
    files.revocations_bundle.clear();
    assert!(matches!(
        SignedIndex::verify(&files, &anchor),
        Err(VerifyError::Signature {
            file: "revocations.json",
            ..
        })
    ));
}

#[test]
fn wrong_key_refused() {
    let (files, _) = signed_v2_index(&[app("acme/a", "1.0.0")], &no_revocations());
    let other = TestKey::generate();
    assert!(matches!(
        SignedIndex::verify(&files, &other.anchor()),
        Err(VerifyError::Signature {
            file: "index.v2.json",
            error: SignatureError::Invalid(_)
        })
    ));
}

#[test]
fn tampered_index_refused() {
    let (mut files, anchor) = signed_v2_index(&[app("acme/a", "1.0.0")], &no_revocations());
    files.index.push(b' ');
    assert!(matches!(
        SignedIndex::verify(&files, &anchor),
        Err(VerifyError::Signature {
            file: "index.v2.json",
            ..
        })
    ));
}

#[test]
fn nothing_is_parsed_before_both_signatures_verify() {
    // Garbage index bytes, correctly signed, but an unsigned revocation list:
    // the signature error wins over the parse error.
    let key = TestKey::generate();
    let mut files = key.sign_files(b"not json", b"{}");
    files.revocations_bundle = TestKey::generate().sign_bundle(b"{}");
    assert!(matches!(
        SignedIndex::verify(&files, &key.anchor()),
        Err(VerifyError::Signature {
            file: "revocations.json",
            ..
        })
    ));
}

#[test]
fn keyless_identity_pinned() {
    // MK0's real cosign release bundle (keyless, signed by Sigstore's own
    // identity) is no signature of the official index.
    let bundle = read("testdata/sigstore/cosign-linux-amd64.sigstore.json");
    let files = IndexFiles {
        index: read("testdata/index-v2/index.v2.json"),
        index_bundle: bundle.clone(),
        revocations: read("testdata/index-v2/revocations.json"),
        revocations_bundle: bundle,
    };
    assert!(matches!(
        SignedIndex::verify(&files, &TrustAnchor::official()),
        Err(VerifyError::Signature {
            file: "index.v2.json",
            ..
        })
    ));
    // Nor does a key anchor accept a keyless bundle.
    assert!(matches!(
        SignedIndex::verify(&files, &TestKey::generate().anchor()),
        Err(VerifyError::Signature {
            file: "index.v2.json",
            ..
        })
    ));
}

#[test]
fn official_anchor_is_the_publish_workflow() {
    let TrustAnchor::Keyless(identity) = TrustAnchor::official() else {
        panic!("the official anchor is keyless");
    };
    assert_eq!(
        identity.identity,
        "https://github.com/ostrium-labs/loams-marketplace/.github/workflows/publish.yml@refs/heads/main"
    );
    assert_eq!(
        identity.issuer,
        "https://token.actions.githubusercontent.com"
    );
}

// KeyVerifier.

#[test]
fn real_cosign_key_bundle_verifies() {
    // Made by the official cosign v3.0.6 linux-amd64 binary:
    // `cosign sign-blob --key cosign.key --bundle blob.json.sigstore.json
    //  --tlog-upload=false --use-signing-config=false blob.json`.
    let pem = String::from_utf8(read("testdata/sigstore/cosign-key/cosign.pub")).unwrap();
    let verifier = KeyVerifier::from_pem(&pem).unwrap();
    let blob = read("testdata/sigstore/cosign-key/blob.json");
    let bundle = read("testdata/sigstore/cosign-key/blob.json.sigstore.json");
    let nobody = loams_agentd_marketplace::Identity {
        identity: String::new(),
        issuer: String::new(),
    };
    verifier.verify_blob(&blob, &bundle, &nobody).unwrap();

    let mut other = blob.clone();
    other.push(b'\n');
    assert!(matches!(
        verifier.verify_blob(&other, &bundle, &nobody),
        Err(SignatureError::Invalid(_))
    ));

    // The digest and hint are early rejects only: without them the
    // signature alone still decides.
    let mut bare: Value = serde_json::from_slice(&bundle).unwrap();
    bare["verificationMaterial"] = json!({});
    bare["messageSignature"]
        .as_object_mut()
        .unwrap()
        .remove("messageDigest");
    let bare = serde_json::to_vec(&bare).unwrap();
    verifier.verify_blob(&blob, &bare, &nobody).unwrap();
    assert!(verifier.verify_blob(&other, &bare, &nobody).is_err());

    // The real bundle under a different key fails.
    let stranger = KeyVerifier::from_pem(&TestKey::generate().pem()).unwrap();
    assert!(stranger.verify_blob(&blob, &bare, &nobody).is_err());
}

#[test]
fn key_must_be_p256_spki() {
    for pem in [
        "",
        "-----BEGIN PUBLIC KEY-----\nAAAA\n-----END PUBLIC KEY-----\n",
        "-----BEGIN EC PRIVATE KEY-----\nAAAA\n-----END EC PRIVATE KEY-----\n",
        // An Ed25519 SPKI.
        "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEAGb9ECWmEzf6FQbrBZ9w7lshQhqowtrbLDFw4rXAxZuE=\n-----END PUBLIC KEY-----\n",
    ] {
        assert!(matches!(
            KeyVerifier::from_pem(pem),
            Err(SignatureError::Malformed(_))
        ));
    }
}

// Lookup.

#[test]
fn verify_listing_returns_tier_and_sha() {
    let index = fixture();
    let found = verify_listing(&index, "acme/notes-app", &v("1.2.0")).unwrap();
    assert_eq!(found.tier(), Tier::Community);
    assert_eq!(found.listing().kind, ListingKind::App);
    let raw: Value = serde_json::from_slice(&read("testdata/index-v2/index.v2.json")).unwrap();
    let entry = raw["listings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "acme/notes-app")
        .unwrap();
    assert_eq!(
        found.listing_sha256,
        entry["versions"][0]["listing_sha256"].as_str().unwrap()
    );
    assert!(found.warning.is_none());
    assert_eq!(
        index.source_commit(),
        Some("0123456789abcdef0123456789abcdef01234567")
    );
}

#[test]
fn no_listing() {
    assert_eq!(
        verify_listing(&fixture(), "acme/nothing", &v("1.0.0")).unwrap_err(),
        VerifyError::NoListing("acme/nothing".into())
    );
}

#[test]
fn no_version() {
    assert!(matches!(
        verify_listing(&fixture(), "acme/notes-app", &v("9.9.9")),
        Err(VerifyError::NoVersion { .. })
    ));
}

// Yanks and revocations.

#[test]
fn yanked_refused() {
    let index = signed(
        &[json!({ "listing": app("acme/a", "1.0.0"), "yanked": "broken migration" })],
        &no_revocations(),
    );
    assert!(matches!(
        verify_listing(&index, "acme/a", &v("1.0.0")),
        Err(VerifyError::Yanked { reason, .. }) if reason == "broken migration"
    ));
}

#[test]
fn disable_revoked_refused() {
    let index = signed(
        &[app("acme/a", "1.0.0")],
        &json!({ "revocations": [revocation("acme/a", "=1.0.0", "disable")] }),
    );
    assert!(matches!(
        verify_listing(&index, "acme/a", &v("1.0.0")),
        Err(VerifyError::Revoked { .. })
    ));
}

#[test]
fn warn_revocation_returned_as_warning() {
    let index = signed(
        &[app("acme/a", "1.0.0")],
        &json!({ "revocations": [revocation("acme/a", "*", "warn")] }),
    );
    let found = verify_listing(&index, "acme/a", &v("1.0.0")).unwrap();
    assert!(found.warning.is_some());
}

#[test]
fn is_revoked_true_only_for_disable() {
    let index = signed(
        &[app("acme/a", "1.0.0"), app("acme/b", "1.0.0")],
        &json!({ "revocations": [
            revocation("acme/a", "*", "warn"),
            revocation("acme/b", "*", "disable"),
        ] }),
    );
    assert!(!is_revoked(index.revocations(), "acme/a", &v("1.0.0")));
    assert!(is_revoked(index.revocations(), "acme/b", &v("1.0.0")));
    assert!(!is_revoked(index.revocations(), "acme/c", &v("1.0.0")));
}

#[test]
fn is_revoked_covers_prerelease() {
    let index = signed(
        &[app("acme/a", "1.0.0")],
        &json!({ "revocations": [revocation("acme/a", ">=1.0.0, <2.0.0", "disable")] }),
    );
    assert!(is_revoked(index.revocations(), "acme/a", &v("1.5.0-rc.1")));
    assert!(!is_revoked(index.revocations(), "acme/a", &v("2.0.0")));
}

#[test]
fn suspended_namespace_entries_revoke_all() {
    // The index build expands a suspended namespace into one `disable`
    // entry per listing with `versions: "*"` (Decision 6).
    let index = signed(
        &[
            app("evil/a", "1.0.0"),
            app("evil/a", "2.0.0-rc.1"),
            app("evil/b", "0.1.0"),
        ],
        &json!({ "revocations": [
            revocation("evil/a", "*", "disable"),
            revocation("evil/b", "*", "disable"),
        ] }),
    );
    for (id, version) in [
        ("evil/a", "1.0.0"),
        ("evil/a", "2.0.0-rc.1"),
        ("evil/b", "0.1.0"),
    ] {
        assert!(is_revoked(index.revocations(), id, &v(version)));
        assert!(matches!(
            verify_listing(&index, id, &v(version)),
            Err(VerifyError::Revoked { .. })
        ));
    }
}

// D2626: packages by tier.

#[test]
fn community_helm_refused() {
    let index = signed(&[helm_app("acme/wiki")], &no_revocations());
    assert_eq!(
        verify_listing(&index, "acme/wiki", &v("3.1.0")).unwrap_err(),
        VerifyError::PackageNotAllowed {
            id: "acme/wiki".into(),
            tier: Tier::Community,
            package: "helm",
        }
    );
}

#[test]
fn verified_helm_allowed() {
    let index = fixture();
    let found = verify_listing(&index, "acme/wiki", &v("3.1.0")).unwrap();
    assert_eq!(found.tier, Tier::Verified);

    let index = signed(
        &[json!({ "listing": helm_app("acme/wiki"), "tier": "verified" })],
        &no_revocations(),
    );
    verify_listing(&index, "acme/wiki", &v("3.1.0")).unwrap();
    // A reserved namespace is first-party, which may use helm too.
    let index = signed(&[helm_app("loams-labs/wiki")], &no_revocations());
    assert_eq!(
        verify_listing(&index, "loams-labs/wiki", &v("3.1.0"))
            .unwrap()
            .tier,
        Tier::FirstParty
    );
}

#[test]
fn community_oci_allowed() {
    let index = signed(&[app("acme/a", "1.0.0")], &no_revocations());
    assert_eq!(
        verify_listing(&index, "acme/a", &v("1.0.0")).unwrap().tier,
        Tier::Community
    );
}

#[test]
fn tier_is_per_version() {
    let index = signed(
        &[
            json!({ "listing": helm_app("acme/wiki"), "tier": "verified" }),
            {
                let mut next = helm_app("acme/wiki");
                next["version"] = json!("3.2.0");
                next
            },
        ],
        &no_revocations(),
    );
    verify_listing(&index, "acme/wiki", &v("3.1.0")).unwrap();
    assert!(matches!(
        verify_listing(&index, "acme/wiki", &v("3.2.0")),
        Err(VerifyError::PackageNotAllowed { .. })
    ));
}

// Dependencies.

fn ids(list: &[loams_agentd_marketplace::VerifiedListing]) -> Vec<String> {
    list.iter()
        .map(|l| format!("{}@{}", l.listing.id, l.listing.version))
        .collect()
}

#[test]
fn dependencies_resolve_in_install_order() {
    let index = signed(
        &[
            app_needing("acme/a", "1.0.0", &[("acme/b", "^1")]),
            app_needing("acme/b", "1.0.0", &[("acme/c", "^2")]),
            app_needing("acme/b", "1.4.0", &[("acme/c", "^2")]),
            json!({ "listing": app_needing("acme/b", "1.9.0", &[]), "yanked": "bad" }),
            app_needing("acme/b", "1.10.0-rc.1", &[]),
            app_needing("acme/b", "2.0.0", &[]),
            app("acme/c", "2.1.0"),
            app("acme/c", "2.3.0"),
            app("acme/c", "2.5.0"),
        ],
        &json!({ "revocations": [revocation("acme/c", "=2.5.0", "disable")] }),
    );
    let root = verify_listing(&index, "acme/a", &v("1.0.0")).unwrap();
    assert_eq!(
        ids(&resolve_depends_on(&index, &root).unwrap()),
        ["acme/c@2.3.0", "acme/b@1.4.0"]
    );
}

#[test]
fn shared_dependencies_appear_once() {
    let index = signed(
        &[
            app_needing("acme/a", "1.0.0", &[("acme/b", "^1"), ("acme/c", "^1")]),
            app_needing("acme/b", "1.0.0", &[("acme/c", "^1")]),
            app("acme/c", "1.0.0"),
        ],
        &no_revocations(),
    );
    let root = verify_listing(&index, "acme/a", &v("1.0.0")).unwrap();
    assert_eq!(
        ids(&resolve_depends_on(&index, &root).unwrap()),
        ["acme/c@1.0.0", "acme/b@1.0.0"]
    );
}

#[test]
fn dependency_only_revoked_versions_refused() {
    let index = signed(
        &[
            app_needing("acme/a", "1.0.0", &[("acme/b", "^1")]),
            app("acme/b", "1.0.0"),
            json!({ "listing": app("acme/b", "1.1.0"), "yanked": "bad" }),
            app("acme/b", "1.2.0-rc.1"),
        ],
        &json!({ "revocations": [revocation("acme/b", "=1.0.0", "disable")] }),
    );
    let root = verify_listing(&index, "acme/a", &v("1.0.0")).unwrap();
    assert!(matches!(
        resolve_depends_on(&index, &root),
        Err(VerifyError::Dependency { id, .. }) if id == "acme/b"
    ));
}

#[test]
fn dependency_cycle_refused() {
    let index = signed(
        &[
            app_needing("acme/a", "1.0.0", &[("acme/b", "^1")]),
            app_needing("acme/b", "1.0.0", &[("acme/c", "^1")]),
            app_needing("acme/c", "1.0.0", &[("acme/a", "^1")]),
        ],
        &no_revocations(),
    );
    let root = verify_listing(&index, "acme/a", &v("1.0.0")).unwrap();
    assert!(matches!(
        resolve_depends_on(&index, &root),
        Err(VerifyError::Dependency { reason, .. }) if reason.contains("cycle")
    ));
}

#[test]
fn missing_dependency_refused() {
    let index = signed(
        &[app_needing("acme/a", "1.0.0", &[("acme/pg", "^16")])],
        &no_revocations(),
    );
    let root = verify_listing(&index, "acme/a", &v("1.0.0")).unwrap();
    assert!(matches!(
        resolve_depends_on(&index, &root),
        Err(VerifyError::Dependency { id, .. }) if id == "acme/pg"
    ));
}

/// `acme/d0` → `acme/d1` → … → `acme/d<len>`.
fn chain(len: usize) -> Vec<Value> {
    (0..=len)
        .map(|i| {
            let next = format!("acme/d{}", i + 1);
            if i < len {
                app_needing(&format!("acme/d{i}"), "1.0.0", &[(&next, "^1")])
            } else {
                app(&format!("acme/d{i}"), "1.0.0")
            }
        })
        .collect()
}

#[test]
fn dependency_depth_over_8_refused() {
    let index = signed(&chain(8), &no_revocations());
    let root = verify_listing(&index, "acme/d0", &v("1.0.0")).unwrap();
    assert_eq!(resolve_depends_on(&index, &root).unwrap().len(), 8);

    let index = signed(&chain(9), &no_revocations());
    let root = verify_listing(&index, "acme/d0", &v("1.0.0")).unwrap();
    assert!(matches!(
        resolve_depends_on(&index, &root),
        Err(VerifyError::Dependency { .. })
    ));
}

// Compatibility.

#[test]
fn index_v1_still_verifies() {
    let key = TestKey::generate();
    let files = key.sign_files(
        &read("testdata/index-v2/index.json"),
        &read("testdata/index/revocations.json"),
    );
    let index = SignedIndex::verify(&files, &key.anchor()).unwrap();
    // Tiers come from the entry; kinds are v1's.
    let found = verify_listing(&index, "ostrium/claude-code", &v("1.0.0")).unwrap();
    let community = verify_listing(&index, "vercel-labs/fx", &v("0.0.14")).unwrap();
    assert_eq!(community.tier, Tier::Community);
    assert_eq!(found.tier, Tier::FirstParty);
    assert!(found.listing.package.is_none());
    assert!(matches!(
        found.listing.kind,
        ListingKind::Agent | ListingKind::Harness | ListingKind::McpServer
    ));
    // MK0's revocations apply.
    assert!(is_revoked(
        index.revocations(),
        "ostrium/loams-starter",
        &v("0.1.0")
    ));
    assert!(matches!(
        verify_listing(&index, "ostrium/loams-starter", &v("0.1.0")),
        Err(VerifyError::Revoked { .. })
    ));
}

#[test]
fn v1_parser_reads_built_index_json() {
    let parsed = index::parse(&read("testdata/index-v2/index.json")).unwrap();
    assert!(!parsed.listings.is_empty());
}

#[test]
fn index_v1_refuses_a_v2_listing() {
    let v2: Value = serde_json::from_slice(&index_v2(&[app("acme/a", "1.0.0")])).unwrap();
    let mut v1 = v2.clone();
    v1["index_version"] = json!(1);
    assert!(matches!(
        index::parse(&serde_json::to_vec(&v1).unwrap()),
        Err(index::IndexError::Listing { .. })
    ));
    // And index v2 needs a tier on every version.
    let mut untiered = v2;
    untiered["listings"][0]["versions"][0]
        .as_object_mut()
        .unwrap()
        .remove("tier");
    assert!(matches!(
        index::parse(&serde_json::to_vec(&untiered).unwrap()),
        Err(index::IndexError::Shape(_))
    ));
}
