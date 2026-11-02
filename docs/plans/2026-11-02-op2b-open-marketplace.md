# OP2-B: The Open Marketplace (design 86, phase MK1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Where this plan gives exact names, paths, fields or tests, use them verbatim. Record any deviation under "Rulings made during execution" at the end of this file.
>
> **Status: Draft** (2026-11-02). Track OP2, part B of three: OP2-A is operator v2 (it owns the CRDs and the chart `deploy/helm/loams-operator`), OP2-B is this plan, and OP2-C is one-click deploy. It starts **after UP1 merges**: no forks are left in the tree (D2450, D2600).

**Goal:** anyone with a GitHub account can publish an `agent`, `harness`, `mcp-server`, `app` or `engine` listing by opening a pull request to `ostrium-labs/loams-marketplace`. The pull request is checked automatically, merged by a maintainer and shown as Community, and can earn Verified. Operator v2, one-click deploy and agentd then install it from one signed index through one Rust API.

**Architecture:**
- **Listings.** `listing/v2` is a JSON Schema (published in the index repo, compiled into `crates/loams-agentd-marketplace`) plus typed Rust (`ListingV2`). Old `listing/v1` documents parse as v2 agents.
- **Index.** The index build in `agents/loams-listing` writes two signed files: `index.json`, which stays v1-only so 0.1.x agentd keeps working, and `index.v2.json`, which holds every listing with a tier per version.
- **Checks.** The index repo's workflows check each pull request: schema, namespace ownership, digest, provenance, and a kind dry-run of app and engine listings through OP2-A's operator. That dry-run is LIC's install profile. LIC 0.1 is a spec plus a Rust runner built on LOC's kind harness.

**Tech Stack:**
- Rust 1.97 with workspace dependencies: `jsonschema` 0.58, `sigstore-verify` =0.14.0, `ring` =0.17.14, `semver` 1, `kube` =3.1.0 (LIC only).
- TypeScript on Node 24 with `ajv` 8.20.0 and `sigstore` 5.0.0 (`agents/loams-listing`).
- GitHub Actions on GitHub-hosted runners in `loams-marketplace`. The workflows pin cosign v3.0.6, plus kind and Helm releases at least 14 days old.

**Spec:**
- [§86](../design/86-open-platform-operator-v2.md) §4.1 and §5: D2605–D2610, D2624–D2626, and Q1605 (default: CODEOWNERS).
- [§73](../design/73-agent-marketplace.md) §3.6 (manifest), §5 (index), §6 (signing), §9 (LAPC) and §10 (tiers, review, revocation).
- The house style and the MK0 rulings in [MK1](2026-10-21-mk1-agent-marketplace.md).
- [§65](../design/65-loams-operator.md) and [OP1](2026-10-19-op1-loams-operator.md) for LOC.

## Interfaces for sibling plans

OP2-A (operator) and OP2-C (one-click deploy) build against these. They live in `crates/loams-agentd-marketplace`. **Depend on it with `default-features = false, features = ["fetch"]`** (dev: add `"testkit"`): the listing core then builds without agentd's harness, its installers or the conformance runner (Task 1).

```rust
// listing_v2.rs (Task 2)
#[serde(rename_all = "kebab-case")] pub enum ListingKind { Agent, Harness, McpServer, App, Engine }
#[serde(rename_all = "kebab-case")] pub enum Target { Cluster, NodeLaptop, NodeDevice }
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Package {
    Oci { image: String, digest: String },                 // image has no tag; digest "sha256:<64 hex>"
    Helm { chart: String, version: String, digest: String }, // chart "oci://…" (manifest digest) or https …/.tgz (file sha256)
    Kustomize { url: String, digest: String },             // https tarball of the kustomization dir
    Wasm { url: String, digest: String },                  // node-laptop / node-device only
    Binary { targets: Vec<(String /*triple*/, String /*url*/, String /*digest*/)> }, // JSON: [{triple,url,digest}]
}
pub enum StorageKind { Object, SqlMysql, SqlPostgres, Kafka, Kv }   // D2603 names, kebab-case
pub struct Requires { pub storage: Vec<StorageKind>, pub cpu_millis: Option<u32>, pub memory_mib: Option<u32>, pub gpu: Option<Gpu> }
pub struct Gpu { pub count: u32, pub vendor: GpuVendor /* nvidia | amd | any */ }
pub struct CostHint { pub cpu_millis: u32, pub memory_mib: u32, pub storage_gib: u32, pub egress_gib_per_month: u32, pub managed: Vec<StorageKind> }
pub struct DependsOn { pub id: String, pub version: semver::VersionReq }
pub struct Run { pub ports: Vec<Port>, pub health: Option<Health>, pub args: Vec<String> } // oci app/engine only
pub struct Port { pub name: String, pub port: u16, pub protocol: PortProtocol /* tcp | udp */ }
pub struct Health { pub http_path: String, pub port: u16 }
pub enum SchemaVersion { V1, V2 }
pub struct SourceRef { pub repository: String, pub commit: String, pub path: Option<String> }
pub struct ProvenanceRef { pub kind: ProvenanceType /* serde "type": npm-slsa | pypi-pep740 | cosign | sigstore-bundle | github-attestation */, pub reference: String /* serde "ref" */ }
pub struct ListingV2 {
    pub schema: SchemaVersion, pub id: String, pub kind: ListingKind, pub version: semver::Version,
    pub title: String, pub summary: String, pub licence: String, pub source: SourceRef,
    pub security_contact: Option<String>, pub package: Option<Package>, pub provenance: Option<ProvenanceRef>,
    pub requires: Requires, pub targets: Vec<Target>, pub depends_on: Vec<DependsOn>,
    pub values_schema: Option<serde_json::Value>, pub cost_hint: Option<CostHint>, pub run: Option<Run>,
    pub raw: serde_json::Value, // the signed document; §73 agent fields (artifact, runtime, tools, place…) stay here
}
impl ListingV2 {
    pub fn from_json(doc: &serde_json::Value) -> Result<ListingV2, ListingError>;
    pub fn namespace(&self) -> &str;
    pub fn validate_values(&self, values: &serde_json::Value) -> Result<(), Vec<listing::Issue>>;
}

// index::Tier, unchanged and re-exported: FirstParty, Verified, Community
pub use index::Tier;

// signed.rs (Task 3)
pub struct IndexFiles { pub index: Vec<u8>, pub index_bundle: Vec<u8>, pub revocations: Vec<u8>, pub revocations_bundle: Vec<u8> }
pub enum TrustAnchor { Keyless(verify::Identity), PublicKey { pem: String } }
impl TrustAnchor { pub fn official() -> TrustAnchor; }
pub struct SignedIndex { /* private */ }
impl SignedIndex {
    pub fn verify(files: &IndexFiles, anchor: &TrustAnchor) -> Result<SignedIndex, VerifyError>;
    pub fn revocations(&self) -> &RevocationList;
    pub fn source_commit(&self) -> Option<&str>;
}
pub struct VerifiedListing { pub listing: ListingV2, pub tier: Tier, pub listing_sha256: String, pub warning: Option<Revocation> }
impl VerifiedListing { pub fn listing(&self) -> &ListingV2; pub fn tier(&self) -> Tier; } // accessors OP2-A Task 11 uses
pub enum VerifyError {
    Signature { file: &'static str, error: SignatureError }, Index(IndexError),
    NoListing(String), NoVersion { id: String, version: String },
    Yanked { id: String, version: String, reason: String }, Revoked { id: String, version: String, reason: String },
    PackageNotAllowed { id: String, tier: Tier, package: &'static str },
    Dependency { id: String, reason: String },
}
pub fn verify_listing(index: &SignedIndex, id: &str, version: &semver::Version) -> Result<VerifiedListing, VerifyError>;
pub fn resolve_depends_on(index: &SignedIndex, root: &VerifiedListing) -> Result<Vec<VerifiedListing>, VerifyError>; // install order, deps first, root excluded, depth ≤ 8 (OP2-A's limit)

// fetch.rs (Task 1): the `Fetcher` trait moves here from market.rs; HttpsFetcher behind feature `fetch` (not `agentd`)
// testkit.rs (Task 3), feature `testkit` without `agentd`:
pub fn signed_v2_index(listings: &[serde_json::Value], revocations: &serde_json::Value) -> (IndexFiles, TrustAnchor); // ring P-256 key, for OP2-A's tests

// revocations.rs (Task 3)
pub type RevocationList = revocations::Revocations;
pub fn is_revoked(list: &RevocationList, id: &str, version: &semver::Version) -> bool; // true only for a `disable` revocation
```

**The published files** sit at `https://ostrium-labs.github.io/loams-marketplace/`:
- `index.v2.json` and `index.v2.json.sigstore.json`
- `revocations.json` and `revocations.json.sigstore.json`
- `index.json` and `index.json.sigstore.json` (v1 only, for 0.1.x agentd)

`TrustAnchor::official()` is the publish workflow's identity: `…/loams-marketplace/.github/workflows/publish.yml@refs/heads/main`, with the GitHub Actions issuer. The JSON Schema is `schemas/listing-v2.schema.json` in the index repo, with `$id` `https://loams.dev/schemas/listing/v2.json`.

**What this plan consumes from OP2-A** (its draft `2026-11-02-op2a-operator-v2-core.md`). Task 0 records the merged names; if they differ, only the Task 10 job adapts, not the contract above.
- The chart `deploy/helm/loams-operator`.
- `LoamsInstall` at `loams.dev/v1alpha1`, with `spec.cluster`, `spec.listing.{id, version}`, `spec.values` and the condition `Ready`.
- `LoamsCluster.spec.marketplace.{index, refresh}`.
- LOC's kind harness in `conformance/operator/loc-harness`.

**What it still needs from OP2-A,** for the PR dry-run, which cannot sign keylessly from a fork:
- `spec.marketplace.publicKey` (optional PEM), which selects `TrustAnchor::PublicKey`;
- an index URL the job can serve inside kind: `file://` from a mounted ConfigMap, or OP2-A's `FixtureSource` exposed as a chart value.

Until OP2-A has one of these, the `dry-run` job stays skipped with its message.

## Decisions this plan makes

Each is recorded here because the spec leaves it open:

1. **Two indexes.** `index.json` stays index v1 with v1 listings only, because 0.1.x agentd refuses a whole index that holds one listing it cannot validate. `index.v2.json` (`index_version: 2`) holds everything, with a `tier` on each version.
2. **How v1 and v2 are told apart.** A v2 document carries `"$schema": "https://loams.dev/schemas/listing/v2.json"`; no `$schema`, or the v1 URL, means v1. v1 `targets` map as follows:
   - `agentd` → `[cluster, node-laptop]`
   - `byoc-server` → `[cluster]`
   - `app-byoc` → `[cluster]`
3. **Two copies of the schema.** The published `schemas/listing-v2.schema.json` (index repo) and the compiled `schemas/marketplace/listing-v2.schema.json` (loams) are byte-identical, checked by the index self-test at the pinned `.github/loams-ref`. Changes land in loams first.
4. **Trust anchors.** An index is trusted either keyless (identity and issuer) or by an ECDSA P-256 public key (`ring`, from a cosign `sign-blob --key` bundle). The key form is for private indexes and for the dry-run's job-local key. Fork pull requests cannot sign keylessly.
5. **Tier is per version, set by the index build from evidence** (D2608):
   - `first-party`: reserved namespaces.
   - `verified`: `versions/<v>/verified.json`, owned by maintainers through CODEOWNERS, plus `security_contact`, plus provenance, plus a passing LAPC Core run (agent kinds) or a passing full LIC run (app and engine).
   - `community`: everything else.
   - **Packages by tier** (D2626): Community may use `oci`, or no `package` (agent kinds keep §73 artifacts). `helm`, `kustomize`, `wasm` and `binary` need Verified. `verify_listing` enforces this again on the consumer's side.
6. **`is_revoked` means a `disable` revocation.** A `warn` reaches the caller as `VerifiedListing.warning`. Namespace suspension (`publishers/<ns>.json` `suspended`) is expanded by the index build into one `disable` entry (`versions: "*"`) per listing, so 0.1.x clients honour it too.
7. **Namespace ownership.**
   - A namespace is the lowercased GitHub login. `publishers/<ns>.json` pins the account's numeric id, so a renamed or re-registered login cannot take it over.
   - A **user** namespace needs that user as the pull request's author or an approver.
   - An **org** namespace needs its author or an approver to be a *public* member of the org, or a maintainer listed in the publishers file.
   - **Reserved:** `ostrium`, `loams`, and the prefixes `ostrium-` and `loams-`, all mapped to the `ostrium-labs` org.
8. **What gets the kind dry-run.**
   - `app` and `engine` listings that target `cluster`.
   - Agent kinds get no kind dry-run in MK1, because handing an install to agentd is OP2-A's work. LAPC Core runs for Verified.
   - Node-only listings (`wasm`, `binary`) skip it with a message.
   - Until OP2-A's chart exists at the pinned loams ref, the job is skipped with a message.
9. **LIC 0.1 is the cluster profile only.** The QEMU arm64 device profile is reserved for EDGE1/EDGE2, so node-target listings cannot be Verified yet. That is harmless: no devices exist before EDGE.
10. **Tools in the index CI.** `loams-lic` and `loams-agentd` are built with `cargo install --git … --rev <loams-ref> --locked`, cached by ref. `loams-agentd` is built only in the Verified-evidence job.
11. **Fork pull requests.**
    - Every check runs on `pull_request`, with read-only permissions. There is no `pull_request_target` and no `id-token` in any job that touches submitted content.
    - The report comment comes from a `workflow_run` workflow that reads the report as data.
12. **No proto change.** Connect keeps `loams.marketplace.v1`; v2-only fields travel in the ndjson listing JSON. agentd lists `app` and `engine` but refuses to install them ("install it with a LoamsInstall or `loams deploy`").
13. **Schema details.**
    - `requires` is numeric (`cpu_millis`, `memory_mib`).
    - `cost_hint` is required for `app` and `engine` (D2615).
    - `values_schema` is ≤ 64 KiB, with local (`#…`) `$ref`s only.
    - `run` exists for `oci` app and engine packages, so OP2-A can render them and LIC can probe health.
14. **Provenance of v2 packages.**
    - `cosign`: OCI only, checked with the cosign v3.0.6 CLI. The signer must be a workflow of `source.repository`.
    - `github-attestation`: any digest.
    - `sigstore-bundle`: single-file packages.

## Global Constraints

- **Repositories and worktrees.**
  - **loams tasks:** worktree `~/Documents/Ostriumlabs/loams-wt/op2b-<task>`, one PR per task into `dev`, `git commit -s`. Label Tasks 3, 6, 7, 8 and 9 `review:greptile` (security, new API). Greptile does not review `loams-marketplace`; Tasks 11 and 12 get a maintainer review there.
  - **loams-marketplace tasks:** **never clone it.** Branch and edit through `gh api` (contents and git refs), and open the PR with `gh pr create --repo ostrium-labs/loams-marketplace`. Its `self-test` and `validate` workflows are the test runner.
- **Light Rust builds.**
  - Run `cargo test -p <crate> -j 4`, one heavy build at a time.
  - Never `--workspace`, never set `CARGO_TARGET_DIR`, never build in `/tmp`. The global target dir is configured already.
- **No forks** (D2450, D2600): official crates, actions and CLIs only. New dependencies must be at least 14 days old.
- **Metadata only:** the index never contains package bytes (§73 D1460).
- **Free only:** `pricing` stays the one-value enum `["free"]` (D2610).
- **Published versions are immutable:** a change or deletion of a `listing.json` fails (the MK0 rule). Fixes are a new version, `yanked.json` or a revocation.
- **Digests** are always `sha256:<64 lowercase hex>`. URLs are `https://` only.
- **No secrets or OIDC tokens** in any job that installs, pulls or runs submitted content.

## Review Focus

1. **A 0.1.x agentd after the first v2 listing merges.** Expected: it keeps reading `index.json` with no error. Tests: `index_v1_output_excludes_v2_listings` (Task 7) and `v1_parser_reads_built_index_json` (Task 3).
2. **A namespace taken over** by a renamed or recreated account, a case variant (`Acme` vs `acme`), a non-member of an org, or a look-alike reserved prefix (`loams-official`). Expected: refused. Tests: `owner_id_mismatch_refused`, `namespace_is_case_insensitive`, `org_non_member_refused` and `reserved_prefix_refused` (Task 8).
3. **A fork pull request gets secrets, OIDC or a write token** where submitted images run or submitted files are read. Expected: never. Test: `workflow_policy` in `tests/self-test.sh` (Task 11).
4. **A `values_schema` with a remote `$ref`, or a huge one,** which would make the operator fetch URLs or spend memory at install. Expected: refused at lint and at parse. Tests: `values_schema_remote_ref_refused` and `values_schema_over_64k_refused` (Task 2).
5. **A dependency that resolves only to yanked or revoked versions, or a dependency cycle.** Expected: `VerifyError::Dependency`, and nothing installs. Tests: `dependency_only_revoked_versions_refused` and `dependency_cycle_refused` (Task 3).

---

### Task 0: Rulings (both repos)

**Files:** Modify `docs/plans/2026-11-02-op2b-open-marketplace.md` ("Rulings made during execution").

- [ ] **Step 1:** Record OP2-A's real names: the chart path, the `marketplace.sources[]` values, `file://` source support, `LoamsInstall`'s fields and condition, and the LOC harness's crate and kind-cluster type (`conformance/operator/loc-harness` per OP1 Task 5). Say whether they exist on `dev` yet.
- [ ] **Step 2:** Record:
  - the pinned versions of kind, Helm and `kindest/node`, each at least 14 days old;
  - the shape of cosign v3.0.6's `sign-blob --key --bundle --tlog-upload=false` bundle (where `messageSignature.signature` sits);
  - that `GET /orgs/{org}/public_members/{user}` answers 204 or 404 with the workflow token;
  - whether the maintainers team for CODEOWNERS exists. Creating it is an owner step, as is the branch protection that requires code-owner review and the checks.
- [ ] **Step 3: Commit** `git commit -s -m "plan: OP2-B Task 0 rulings"`.

### Task 1 (loams): `listing/v2` schema, fixtures and the feature-free core

**Files:**
- Create: `schemas/marketplace/listing-v2.schema.json`, `schemas/marketplace/testdata/v2/valid/{app-oci,app-helm,engine-kustomize,agent-v2,mcp-server-v2,device-wasm,laptop-binary}.json`, `schemas/marketplace/testdata/v2/invalid.json`, `crates/loams-agentd-marketplace/tests/schema_v2.rs`
- Modify: `crates/loams-agentd-marketplace/Cargo.toml`, `src/lib.rs`, `src/listing.rs`

**Interfaces:**
- Produces: `listing::SCHEMA_V2_JSON`, `listing::SCHEMA_V2_ID = "https://loams.dev/schemas/listing/v2.json"`, and `listing::validate_v2(&Value) -> Result<(), Vec<Issue>>`. Also three Cargo features:
  - `agentd` (on by default, implies `fetch`): gates `conformance`, `contract`, `engine`, `identity`, `install`, `market`, `plan`, `provenance`, `service`, `artifact` and the agentd half of `testkit`, and makes their dependencies optional.
  - `fetch`: `fetch.rs` with the `Fetcher` trait (moved from `market.rs` and re-exported there) and `HttpsFetcher`, on reqwest.
  - `testkit`: stays its own feature.

- [ ] **Step 1: Write the failing tests** in `tests/schema_v2.rs`. `invalid.json` follows MK0's form: each case is a valid fixture plus a JSON-pointer patch.
  - `v2_schema_accepts_fixtures`: every file in `testdata/v2/valid` passes `validate_v2`.
  - `v2_schema_rejects_each_invalid_fixture`. Each case must fail with an issue at the named pointer:
    - `wasm-with-cluster` (`/targets`)
    - `binary-with-cluster`
    - `app-without-package`
    - `engine-with-wasm-on-cluster`
    - `oci-image-with-tag` (`image` matching `:[^/]+$` or containing `@`)
    - `digest-not-sha256`
    - `package-without-provenance`
    - `app-without-cost-hint`
    - `bad-storage-kind`
    - `depends-on-bad-range`
    - `pricing-not-free`
    - `run-on-helm`
    - `http-url`
  - `v1_fixtures_still_pass_v1`: the existing `testdata/valid` files are unchanged and still pass `validate`.
  - `v2_schema_id_matches`: `schema_v2()["$id"] == SCHEMA_V2_ID`.
- [ ] **Step 2: Run** `cargo test -p loams-agentd-marketplace -j 4 --test schema_v2`. Expected: FAIL to compile (`validate_v2` not found).
- [ ] **Step 3: Write `listing-v2.schema.json`** (Draft 2020-12). Start from v1's `$defs` by `$ref` to the v1 schema's `$id`. The validator gets both resources: `jsonschema::options().with_resource(SCHEMA_ID, …)` in Rust, and `ajv.addSchema` in Task 5.
  - **Added:** `$schema` (const, the v2 URL; required), `kind` (5 values), `package`, `requires`, `targets` (the v2 enum), `depends_on`, `values_schema` (type `object`), `cost_hint`, `run`, and `provenance.type` widened with `cosign` for OCI.
  - **Rules in `allOf`:**
    - `app` and `engine` require `package` and `cost_hint`;
    - `wasm` and `binary` forbid `cluster` in `targets`;
    - `package` requires `provenance`;
    - `run` only with `package.type == "oci"` and kind `app` or `engine`;
    - agent kinds keep v1's per-kind rules.
- [ ] **Step 4: Add `validate_v2`** beside `validate` in `listing.rs`, with `SCHEMA_V2_JSON = include_str!("../../../schemas/marketplace/listing-v2.schema.json")`.
- [ ] **Step 5: Make agentd a feature.** In `Cargo.toml`, set `[features] default = ["agentd"]`, `agentd = ["fetch", "dep:tokio", "dep:rustix", "dep:loams-walrus", "dep:loams-zitadel", "dep:loams-agentd-harness"]` and `fetch = ["dep:reqwest"]`. Gate the modules in `lib.rs` with `#[cfg(feature = "agentd")]` or `#[cfg(feature = "fetch")]`.
- [ ] **Step 6: Run** `cargo test -p loams-agentd-marketplace -j 4`, then `cargo check -p loams-agentd-marketplace --no-default-features -j 4`, then `cargo tree -p loams-agentd-marketplace --no-default-features -e normal | grep -c -E 'loams-agentd-harness|reqwest|tokio '`. Expected: all tests PASS, check OK, count `0`. Also run `cargo check -p loams-agentd-marketplace --no-default-features --features fetch,testkit -j 4`; expected OK.
- [ ] **Step 7: Commit** `git commit -s -m "marketplace: listing/v2 schema and a feature-free listing core (OP2-B Task 1, D2605)"`.

### Task 2 (loams): `ListingV2` types, the v1 upgrade and values validation

**Files:**
- Create: `crates/loams-agentd-marketplace/src/listing_v2.rs`, `tests/listing_v2.rs`
- Modify: `src/lib.rs` (re-exports per "Interfaces for sibling plans")

**Interfaces:**
- Consumes: `listing::validate`, `listing::validate_v2` (Task 1).
- Produces: every type in `listing_v2.rs` above; `ListingKind::as_str(&self) -> &'static str` (`"mcp-server"` etc., replacing `index::Kind::as_str`); `ListingError { Schema(Vec<Issue>), ValuesSchema(String), Shape(String) }`.

- [ ] **Step 1: Write the failing tests:**
  ```rust
  #[test] fn v2_app_oci_parses() { let l = ListingV2::from_json(&fixture("v2/valid/app-oci.json")).unwrap();
      assert_eq!(l.kind, ListingKind::App); assert_eq!(l.schema, SchemaVersion::V2);
      assert!(matches!(l.package, Some(Package::Oci { ref digest, .. }) if digest.starts_with("sha256:"))); }
  #[test] fn binary_targets_are_triples() { /* laptop-binary.json → targets[0].0 == "x86_64-unknown-linux-gnu" */ }
  #[test] fn v1_listing_parses_as_agent() { /* testdata/valid/agent-mastra-acp.json → schema V1, kind Agent,
      targets == [Cluster, NodeLaptop], package None, raw["artifact"] present */ }
  #[test] fn v1_byoc_server_maps_to_cluster() {}
  #[test] fn values_validated_against_values_schema() { /* app-helm: {"replicas":2} Ok; {"replicas":"x"} Err */ }
  #[test] fn values_schema_remote_ref_refused() { /* {"$ref":"https://evil/x.json"} → ListingError::ValuesSchema */ }
  #[test] fn values_schema_over_64k_refused() { /* 65 537-byte canonical schema → ValuesSchema */ }
  #[test] fn no_values_schema_accepts_only_empty_object() {}
  #[test] fn package_round_trips_json() { /* serialize(Package::Binary{..}) == fixture's package object */ }
  ```
- [ ] **Step 2: Run** `cargo test -p loams-agentd-marketplace -j 4 --test listing_v2`. Expected: FAIL (module missing).
- [ ] **Step 3: Implement `ListingV2::from_json(doc: &Value) -> Result<ListingV2, ListingError>`** in `listing_v2.rs`:
  - choose v1 or v2 by `$schema` (Decision 2), validate with that schema, then deserialize;
  - map v1 `targets` per Decision 2;
  - `Binary.targets` goes through a `#[serde(with = "binary_targets")]` helper (objects ⇄ tuples);
  - the values-schema checks: walk every `$ref` and refuse any that does not start with `#`; refuse a canonical size over 65 536 bytes; compile with `jsonschema::draft202012`.
- [ ] **Step 4: Implement `validate_values(&self, values: &Value) -> Result<(), Vec<Issue>>`.** The compiled validator is cached in a `OnceLock` inside `ListingV2` (skipped by serde). With no `values_schema`, it accepts only `{}`.
- [ ] **Step 5: Run** `cargo test -p loams-agentd-marketplace -j 4 --test listing_v2 --test schema_v2`, then `cargo check -p loams-agentd-marketplace --no-default-features -j 4`. Expected: PASS.
- [ ] **Step 6: Commit** `git commit -s -m "marketplace: ListingV2, the v1 upgrade and values validation (OP2-B Task 2, D2605, D2624)"`.

### Task 3 (loams): `SignedIndex`, `verify_listing`, `is_revoked`, `resolve_depends_on`

**Files:**
- Create: `crates/loams-agentd-marketplace/src/signed.rs`, `tests/signed_index.rs`, `testdata/index-v2/` (fixture tree built in Task 7; until then hand-written and replaced in Task 7)
- Modify: `src/verify.rs` (add `KeyVerifier`), `src/index.rs` (index v2, per-version `tier`, `Kind` becomes `pub type Kind = ListingKind`), `src/revocations.rs` (`RevocationList`, `is_revoked`), `src/lib.rs`

**Interfaces:**
- Consumes: `ListingV2::from_json` (Task 2); `SignatureVerifier`, `SigstoreVerifier` and `Identity` (existing `verify.rs`); `Revocations::parse` and `Revocation::covers` (existing).
- Produces: everything under "signed.rs", "revocations.rs" and "testkit.rs" in the sibling interface block; also `KeyVerifier::from_pem(&str) -> Result<KeyVerifier, SignatureError>`, which implements `SignatureVerifier`. The in-test `sign_bundle` helper becomes `testkit::signed_v2_index`.

- [ ] **Step 1: Write the failing tests.** They are signed in-test with a `ring`-generated P-256 key through a test helper `sign_bundle(bytes, &key) -> Vec<u8>`, which builds the cosign key-bundle shape recorded in Task 0. Keyless cases reuse MK0's real cosign release bundle.
  - **Signatures:** `index_signature_required` (empty `index_bundle` → `VerifyError::Signature { file: "index.v2.json", .. }`), `revocations_signature_required`, and `wrong_key_refused`. Also `keyless_identity_pinned`: the cosign bundle under `TrustAnchor::official()` gives `Signature`.
  - **Lookup:** `verify_listing_returns_tier_and_sha` (the community app gives `Tier::Community` and `listing_sha256` equal to the index's), `no_listing` and `no_version`.
  - **Yanks and revocations:**
    - `yanked_refused`;
    - `disable_revoked_refused`;
    - `warn_revocation_returned_as_warning` (`warning.is_some()`, `Ok`);
    - `is_revoked_true_only_for_disable`;
    - `is_revoked_covers_prerelease` (`>=1.0.0, <2.0.0` covers `1.5.0-rc.1`);
    - `suspended_namespace_entries_revoke_all` (`versions: "*"`).
  - **D2626:** `community_helm_refused` (`PackageNotAllowed { package: "helm", .. }`), `verified_helm_allowed` and `community_oci_allowed`.
  - **Dependencies:**
    - `dependencies_resolve_in_install_order` (A→B→C gives `[C, B]`, with the highest matching version that is not yanked, revoked or a prerelease);
    - `dependency_only_revoked_versions_refused`;
    - `dependency_cycle_refused`;
    - `missing_dependency_refused`;
    - `dependency_depth_over_8_refused`.
  - **Compatibility:** `index_v1_still_verifies` (an index v1 file under a key anchor: tiers per entry, kinds v1). Also `v1_parser_reads_built_index_json`: the existing `index::parse` reads `testdata/index-v2/index.json`.
- [ ] **Step 2: Run** `cargo test -p loams-agentd-marketplace -j 4 --test signed_index`. Expected: FAIL (module missing).
- [ ] **Step 3: Add `KeyVerifier`** in `verify.rs`. It reads `messageSignature.signature` (base64 DER) from the bundle and verifies it over the artifact bytes with `ring::signature::ECDSA_P256_SHA256_ASN1`. The PEM must be an SPKI P-256 key; anything else is `Malformed`.
- [ ] **Step 4: Extend `index.rs`.** It accepts `index_version` 1 or 2. In v2, each version holds `tier`, and each listing is validated by `ListingV2::from_json`, so both v1 and v2 documents are accepted. `listing_sha256` is checked with the existing `canonical`.
- [ ] **Step 5: Implement `signed.rs`:**
  - `SignedIndex::verify` checks both bundles under the anchor before parsing anything (MK0 Task 4 ruling 2).
  - `verify_listing` finds the entry and version, refuses `yanked` and `disable` revocations, and applies Decision 5's tier and package table.
  - `resolve_depends_on` is a DFS with an on-stack set for cycles. Each dependency gets the highest version matching its `VersionReq` that `verify_listing` accepts; the result is in post-order with duplicates removed.
- [ ] **Step 6: Add `RevocationList` and `is_revoked`** in `revocations.rs`: `list.for_version(id, &version.to_string()).is_some_and(|r| r.severity == Severity::Disable)`.
- [ ] **Step 7: Run** `cargo test -p loams-agentd-marketplace -j 4`, then `cargo check -p loams-agentd-marketplace --no-default-features -j 4`. Expected: PASS, including every existing MK0 test.
- [ ] **Step 8: Commit** `git commit -s -m "marketplace: signed index v2, verify_listing and is_revoked for the operator (OP2-B Task 3, D2609, D2624, D2626)"`.

### Task 4 (loams): agentd and `loams marketplace` read index v2

**Files:**
- Modify: `crates/loams-agentd-marketplace/src/{sources.rs,market.rs,service.rs}`, `crates/loams-agentd-proto/src/marketplace.rs` (summary gains `targets: Vec<String>` and `tier` per version; additive and serde-defaulted), `crates/loams-agentd-cli/src/marketplace/{mod.rs,render.rs}`
- Test: `crates/loams-agentd-marketplace/tests/market.rs`, `tests/host.rs`, `crates/loams-agentd-cli/src/marketplace/render.rs` (unit tests)

**Interfaces:**
- Consumes: `SignedIndex`, `verify_listing` and `TrustAnchor` (Task 3).
- Produces: `Source::index_v2_url(&self) -> String` (sibling `index.v2.json`); `MarketError::KindNotForAgentd { id: String, kind: &'static str }`.

- [ ] **Step 1: Write the failing tests:**
  - `market_reads_index_v2_first`: the fake world serves both files, and search finds the v2-only `acme/notes-app`.
  - `market_falls_back_to_index_v1`: `index.v2.json` answers 404, so the v1 listings load and the source has no error.
  - `app_listing_install_refused`: `plan_install` for kind `app` gives `KindNotForAgentd { kind: "app" }`.
  - `v2_agent_installs_like_v1`: a v2 `agent` with a `node` artifact plans the same as its v1 twin.
  - In the CLI: `info_renders_v2_fields` (the text shows "Targets: cluster", "Package: oci ghcr.io/acme/notes@sha256:…", "Needs: object storage, 500m CPU, 512 MiB", "Depends on: acme/pg ^16" and "Tier: Community") and `search_kind_engine_filter`.
- [ ] **Step 2: Run** `cargo test -p loams-agentd-marketplace -j 4 --test market --test host`, then `cargo test -p loams-agentd-cli -j 4 marketplace`. Expected: FAIL.
- [ ] **Step 3: Implement.**
  - `Marketplace::refresh` fetches `index.v2.json` and its bundle first, and on 404 only falls back to `index.json`. The verified cache stores whichever verified.
  - `plan_install` and `install` return `KindNotForAgentd` for `App` and `Engine`, with the message "install it with a LoamsInstall or `loams deploy`".
  - The renderer prints the v2 fields in that order, only when present.
- [ ] **Step 4: Run** the same commands. Expected: PASS.
- [ ] **Step 5: Commit** `git commit -s -m "agentd: read the v2 index; list apps and engines, install only agent kinds (OP2-B Task 4, D2625)"`.

### Task 5 (loams): `loams-listing lint` for v2 and open namespaces

**Files:**
- Modify: `agents/loams-listing/src/{schema.ts,lint.ts,cli.ts,types.ts}`
- Test: `agents/loams-listing/test/lint.test.ts`

**Interfaces:**
- Consumes: `schemas/marketplace/listing-v2.schema.json` (Task 1).
- Produces:
  - `lint(doc, { index?: IndexSnapshot, firstPartyOnly?: boolean })`, where `IndexSnapshot = { versions(id: string): string[] }` comes from the built index plus the pull request's added listings;
  - the new rule ids `reserved-namespace`, `depends-on-resolves`, `depends-on-self`, `values-schema-local-ref`, `values-schema-size`, `package-needs-verified` (a warning; the error is the PR check in Task 11) and `binary-triple-unique`;
  - CLI `lint --index <dir>`. `--first-party-only` stays, but CI stops passing it.

- [ ] **Step 1: Write the failing tests** (vitest):
  - `lint_picks_schema_by_dollar_schema`: a v2 app passes; the same document without `$schema` fails v1.
  - `reserved_prefix_refused`: `loams-official/x` and `ostrium-tools/x` give `reserved-namespace` unless the publisher is `ostrium-labs`.
  - `depends_on_must_resolve`: `acme/pg ^17` against `{acme/pg: [16.4.0]}` gives `depends-on-resolves`.
  - `values_schema_remote_ref_refused_in_lint`.
  - `helm_package_warns_needs_verified`.
  - `v1_rules_unchanged`: every existing v1 lint test still passes.
- [ ] **Step 2: Run** `pnpm --filter @loams/listing test -- lint`. Expected: FAIL on the new tests.
- [ ] **Step 3: Implement.**
  - `schema.ts` loads both schemas and picks one by `$schema`.
  - The rules are in `lint.ts`. The reserved set is `["ostrium", "loams"]` plus the prefixes `ostrium-` and `loams-`.
  - `depends-on-resolves` uses `semver.ts`'s Cargo-range matcher.
- [ ] **Step 4: Run** `pnpm --filter @loams/listing test && pnpm --filter @loams/listing typecheck`. Expected: PASS.
- [ ] **Step 5: Commit** `git commit -s -m "listing: lint listing/v2 and open namespaces (OP2-B Task 5, D2605, D2607)"`.

### Task 6 (loams): `loams-listing verify` for v2 packages (digest resolves, provenance present)

**Files:**
- Modify: `agents/loams-listing/src/verify.ts`
- Create: `agents/loams-listing/src/oci.ts`
- Test: `agents/loams-listing/test/verify.test.ts`, `agents/loams-listing/test/oci.test.ts`

**Interfaces:**
- Consumes: the existing `Fetcher`, `verifyGithubAttestation` and `verifySigstoreBundle` in `verify.ts`.
- Produces:
  - `resolveOciDigest(ref: {registry, repository, digest}, fetch): Promise<{mediaType, platforms: string[]}>`, using the anonymous Bearer token flow and `HEAD /v2/<repo>/manifests/<digest>`;
  - `verifyPackage(listing, deps: { fetch, runCosign }): Promise<PackageReport>`, where `PackageReport = { digest: 'ok'|'fail', provenance: 'ok'|'fail'|'missing', detail: string }`;
  - CLI `verify --cosign <path>`.

- [ ] **Step 1: Write the failing tests**, against fake fetchers and a fake `runCosign`:
  - `oci_digest_resolves`;
  - `oci_digest_missing_fails` (404 gives `digest: 'fail'`);
  - `oci_index_must_include_linux_amd64_and_arm64` (D2626 images: amd64 and arm64);
  - `helm_oci_chart_digest_resolves`;
  - `helm_https_tgz_sha256_checked`;
  - `kustomize_tarball_sha256_checked`;
  - `binary_each_triple_checked` (one bad file fails the whole package);
  - `cosign_signer_must_be_source_repo_workflow`: the identity regexp passed to cosign is `^https://github\.com/acme/notes/\.github/workflows/`, and the issuer is `https://token.actions.githubusercontent.com`;
  - `github_attestation_by_package_digest`;
  - `provenance_missing_reported`.
- [ ] **Step 2: Run** `pnpm --filter @loams/listing test -- verify oci`. Expected: FAIL.
- [ ] **Step 3: Implement** `oci.ts` and the v2 branch of `verify` (Decision 14):
  - non-OCI files are downloaded under the existing 512 MiB cap and hashed;
  - `cosign verify <image>@<digest> --certificate-identity-regexp … --certificate-oidc-issuer … --output json` runs through `runCosign` (`child_process.execFile`, no shell);
  - the existing pypi and oci "not yet verified" failure is removed for v2 `package`s only.
- [ ] **Step 4: Run** `pnpm --filter @loams/listing test`. Expected: PASS.
- [ ] **Step 5: Commit** `git commit -s -m "listing: verify v2 package digests and provenance (OP2-B Task 6, D2606)"`.

### Task 7 (loams): index build v2: two indexes, tiers from evidence, suspension

**Files:**
- Modify: `agents/loams-listing/src/{index-build.ts,cli.ts}`, `schemas/marketplace/index.v1.json` (unchanged; a test pins it)
- Create: `schemas/marketplace/index.v2.json`, `schemas/marketplace/verified.v1.json`, `schemas/marketplace/publisher.v1.json`, `crates/loams-agentd-marketplace/testdata/index-v2/` (built by this CLI; replaces Task 3's hand-written fixture)
- Test: `agents/loams-listing/test/index-build.test.ts`

**Interfaces:**
- Consumes: `lint` (Task 5).
- Produces:
  - **Index-repo files:** `listings/<ns>/<name>/versions/<v>/verified.json` (`verified.v1.json`: `{suite: "lapc-core"|"lic-0.1", report_sha256, run_url, reviewed_by: {login, id}, date}`) and `publishers/<ns>.json` (`publisher.v1.json`: `{namespace, github: {login, id, type: "User"|"Organization"}, maintainers: [{login, id}], security_contact?, suspended?: {reason, date}}`).
  - **Output:** `index.json` (v1), `index.v2.json` and `revocations.json`.

- [ ] **Step 1: Write the failing tests:**
  - `index_v1_output_excludes_v2_listings`: the v1 output holds only documents without a v2 `$schema`, and stays byte-identical to MK0's builder on the MK0 fixture tree.
  - `index_v2_has_tier_per_version`.
  - `verified_needs_all_evidence`: with `verified.json` but no `security_contact`, the tier is `community`; with both, plus provenance in the listing, it is `verified`.
  - `reserved_namespace_is_first_party`.
  - `suspended_namespace_expands_to_revocations`: one entry per listing with `versions: "*"` and `severity: "disable"`, merged with the root `revocations.json` in sorted, canonical order.
  - `index_build_is_deterministic` (extended to v2).
- [ ] **Step 2: Run** `pnpm --filter @loams/listing test -- index-build`. Expected: FAIL.
- [ ] **Step 3: Implement** in `index-build.ts`:
  - The tier is computed per version from Decision 5.
  - Entry-level `tier` equals `latest`'s tier, for v1 compatibility.
  - Suspension is expanded before writing.
  - `--first-party-only` is still honoured for v1 output callers.
  - **Rebuild the Rust fixture** by running `node agents/loams-listing/bin/loams-listing.mjs index build crates/loams-agentd-marketplace/testdata/index-v2/src --out crates/loams-agentd-marketplace/testdata/index-v2`.
- [ ] **Step 4: Run** `pnpm --filter @loams/listing test`, then `cargo test -p loams-agentd-marketplace -j 4 --test signed_index`. Expected: PASS on the built fixture.
- [ ] **Step 5: Commit** `git commit -s -m "listing: index v2 with per-version tiers, kept-v1 index, namespace suspension (OP2-B Task 7, D2608, D2609)"`.

### Task 8 (loams): `loams-listing namespace-check`

**Files:**
- Create: `agents/loams-listing/src/namespace.ts`, `agents/loams-listing/test/namespace.test.ts`
- Modify: `agents/loams-listing/src/cli.ts`

**Interfaces:**
- Consumes: the `publisher.v1.json` schema (Task 7).
- Produces:
  - `checkNamespace(input: { namespace: string; publisherFile?: Publisher; addsPublisherFile: boolean; author: {login,id}; approvers: {login,id}[] }, gh: { user(login): Promise<{id,type}>; isPublicMember(org, login): Promise<boolean> }): Promise<{ ok: boolean; reason: string }>`;
  - CLI `namespace-check --pr <event.json> --reviews <reviews.json> <listing.json>…`, which exits 1 with one `::error` line per refusal.

- [ ] **Step 1: Write the failing tests:**
  - `user_namespace_author_ok`;
  - `user_namespace_approver_ok`;
  - `user_namespace_stranger_refused`;
  - `org_public_member_ok`;
  - `org_non_member_refused`;
  - `listed_maintainer_ok`;
  - `owner_id_mismatch_refused`: `publishers/acme.json` pins id 42, and GitHub now says `acme` is id 99;
  - `namespace_is_case_insensitive`: `Acme` the author owns `acme`, and a listing under `Acme/x` is refused by lint as non-lowercase;
  - `reserved_prefix_refused` (non-`ostrium-labs` member);
  - `first_listing_must_add_publisher_file`;
  - `publisher_file_change_needs_owner`;
  - `suspended_namespace_refused`.
- [ ] **Step 2: Run** `pnpm --filter @loams/listing test -- namespace`. Expected: FAIL.
- [ ] **Step 3: Implement** Decision 7 in `namespace.ts`. In production, the `gh` adapter calls `GET /users/{login}` and `GET /orgs/{org}/public_members/{login}` with `GITHUB_TOKEN`, read-only.
- [ ] **Step 4: Run** `pnpm --filter @loams/listing test`. Expected: PASS.
- [ ] **Step 5: Commit** `git commit -s -m "listing: namespace ownership check for open namespaces (OP2-B Task 8, D2606, D2607)"`.

### Task 9 (loams): LIC 0.1: the spec and the runner core

**Files:**
- Create:
  - `conformance/listing/lic-spec.toml`
  - `conformance/listing/README.md` (rendered from the spec)
  - `conformance/listing/loams-lic/{Cargo.toml,src/lib.rs,src/spec.rs,src/checks.rs,src/report.rs,src/main.rs}`
  - `schemas/marketplace/lic-report.v1.json`
  - `conformance/listing/loams-lic/tests/core.rs`
- Modify: root `Cargo.toml` (`members` gains `"conformance/listing/loams-lic"`)

**Interfaces:**
- Consumes: `ListingV2`, `SignedIndex` and `verify_listing` (Tasks 2–3), with `default-features = false`.
- Produces:
  - `#[async_trait] trait LicCluster { async fn snapshot_cluster_scoped(&self) -> Result<BTreeSet<ObjRef>>; async fn apply_install(&self, id: &str, version: &semver::Version, values: &Value) -> Result<()>; async fn wait_ready(&self, timeout: Duration) -> Result<ReadyState>; async fn install_pods(&self) -> Result<Vec<PodFacts>>; async fn probe(&self, health: &Health) -> Result<u16>; async fn delete_install(&self) -> Result<()>; async fn namespace_gone(&self, timeout: Duration) -> Result<bool>; }`
  - `pub enum Profile { DryRun, Full }`
  - `pub async fn run(cluster: &dyn LicCluster, index: &SignedIndex, id: &str, version: &semver::Version, profile: Profile) -> LicReport`
  - the CLI `loams-lic run --index <dir> --public-key <pem> --listing <id>@<version> --profile dry-run|full --out <report.json>`, with exit codes 0 pass, 1 fail, 2 could not run (as LAPC).

- [ ] **Step 1: Write `lic-spec.toml`**, LIC 0.1, profile `cluster`:

  | ID | Level | Profile | Requirement |
  |---|---|---|---|
  | LIC-I1 | MUST | dry-run, full | `LoamsInstall` reaches `Ready` within 600 s |
  | LIC-H1 | MUST | dry-run, full | every workload in the install namespace is available; `run.health` answers 2xx when declared |
  | LIC-S1 | MUST | dry-run, full | no pod is privileged or uses `hostNetwork`, `hostPID` or `hostPath` (D2626) |
  | LIC-U1 | MUST | full | the previous indexed version upgrades to this one and passes I1 and H1 again; `n/a` (counts as pass) when there is no previous version |
  | LIC-D1 | MUST | dry-run, full | deleting the install removes its namespace within 300 s |
  | LIC-D2 | MUST | dry-run, full | the set of cluster-scoped objects after uninstall equals the set before install |
  | LIC-R1 | SHOULD | full | the sum of pod requests ≤ `requires.cpu_millis` and `memory_mib` when declared |

  The `node-device` profile (QEMU arm64) is listed as reserved, with no IDs (Decision 9).
- [ ] **Step 2: Write the failing tests** against an in-memory `FakeCluster`:
  - `spec_ids_unique`;
  - `spec_renders_markdown` (README equals the rendering);
  - `dry_run_passes_good_fake`;
  - `leftover_cluster_role_fails_d2`;
  - `never_ready_fails_i1`;
  - `privileged_pod_fails_s1`;
  - `first_version_u1_not_applicable`;
  - `skipped_must_is_not_pass`;
  - `report_matches_schema`;
  - `revoked_listing_cannot_run` (exit 2, `VerifyError::Revoked`).
- [ ] **Step 3: Run** `cargo test -p loams-lic -j 4`. Expected: FAIL (crate empty).
- [ ] **Step 4: Implement** `spec.rs` (parse, render), `checks.rs` (one fn per ID over `LicCluster`), `report.rs` and `main.rs`. `main.rs` builds no real cluster yet: `--cluster kind` answers exit 2 with "the kind adapter arrives with Task 10".
- [ ] **Step 5: Run** `cargo test -p loams-lic -j 4`. Expected: PASS.
- [ ] **Step 6: Commit** `git commit -s -m "conformance: Listing Install Conformance 0.1 spec and runner core (OP2-B Task 9, D2608)"`.

### Task 10 (loams): LIC's kind adapter over LOC's harness and OP2-A's operator

**Depends on OP2-A:** the chart `deploy/helm/loams-operator`, `LoamsInstall`, and `loc-harness` on `dev`.

**Files:**
- Create: `conformance/listing/loams-lic/src/kind.rs`, `conformance/listing/loams-lic/tests/kind.rs`, `conformance/listing/fixtures/{hello-app,leaky-app}/` (a tiny OCI app listing and one whose chart leaves a ClusterRole; both use public, digest-pinned images)
- Modify: `conformance/listing/loams-lic/Cargo.toml` (adds `loc-harness` and `kube`), `src/main.rs`

**Interfaces:**
- Consumes: OP2-A's names as recorded in Task 0. `loc-harness`'s kind bring-up with audit logging; `LoamsCluster.spec.marketplace.index` plus the key anchor and in-kind index URL listed under "What it still needs from OP2-A"; and `LoamsInstall` `spec.cluster`, `spec.listing.{id,version}`, `spec.values` and condition `Ready`.
- Produces:
  - `KindCluster: LicCluster`;
  - `loams-lic run --cluster kind …`, which creates the cluster, loads the index into a ConfigMap, installs the chart with `helm install --wait`, runs the profile and deletes the cluster;
  - `loams-lic prepare-index --listings <dir> --out <dir> --key-out <pem>`, which builds the PR-local index with `loams-listing index build` and signs it with a fresh job-local key (Decision 4).

- [ ] **Step 1: Write the failing tests**, `#[ignore]` unless `LOAMS_KIND=1`:
  - `kind_dry_run_hello_app_passes`;
  - `kind_leaky_app_fails_d2`;
  - `kind_full_upgrade_from_previous` (hello-app 0.1.0 → 0.2.0).
  - Also `prepare_index_verifies_under_its_key`, which is not ignored: the output verifies with `SignedIndex::verify(.., &TrustAnchor::PublicKey{pem})`.
- [ ] **Step 2: Run** `cargo test -p loams-lic -j 4 prepare_index`. Expected: FAIL. On a machine with Docker, also run `LOAMS_KIND=1 cargo test -p loams-lic -j 4 --test kind -- --ignored --test-threads 1`. Expected: FAIL.
- [ ] **Step 3: Implement `kind.rs`.**
  - `snapshot_cluster_scoped` lists every cluster-scoped resource from API discovery. It leaves out `Event`, `Lease`, `TokenReview` and `SubjectAccessReview`, and objects that existed before install by uid.
  - `install_pods` reads the install namespace.
  - `probe` port-forwards to `run.health.port`.
- [ ] **Step 4: Run** the same commands. Expected: PASS. The kind run is done once locally on the owner's machine, or in the index CI's dry-run job (Task 11). Record which in the rulings.
- [ ] **Step 5: Commit** `git commit -s -m "conformance: LIC kind adapter on the LOC harness and operator v2 (OP2-B Task 10)"`.

### Task 11 (loams-marketplace): open submission: schema, CODEOWNERS, PR checks, dry-run job

**Files** (in `ostrium-labs/loams-marketplace`, through `gh api`):
- Create:
  - `schemas/listing-v2.schema.json` (copy of loams' `schemas/marketplace/listing-v2.schema.json` at the new pin)
  - `.github/CODEOWNERS`
  - `.github/workflows/report-comment.yml`
  - `scripts/namespace-check.sh`
  - `scripts/dry-run.sh`
  - `publishers/ostrium.json` and `publishers/loams.json`
  - `CONTRIBUTING.md`
  - `tests/fixtures/{community-app,bad-owner,v2-agent,node-only,depends-missing}/`
- Modify:
  - `.github/loams-ref` (to the merge commit of Tasks 1–10, or Tasks 1–9 if OP2-A has not merged)
  - `.github/workflows/validate.yml`
  - `scripts/setup-cli.sh` (sparse paths gain `/conformance/listing/` and `/schemas/marketplace/`)
  - `tests/self-test.sh`
  - `README.md`

**Interfaces:**
- Consumes: `loams-listing lint|verify|namespace-check|index build` (Tasks 5–8) and `loams-lic prepare-index|run` (Tasks 9–10).
- Produces: these required check names for branch protection: `listings`, `namespace`, `packages`, `dry-run`.

- [ ] **Step 1: Write the failing self-tests** in `tests/self-test.sh`. Push them to branch `op2b-t11` and confirm with `gh run watch` that `self-test` fails on each:
  - `schema_mirror_matches`: `cmp schemas/listing-v2.schema.json "$LOAMS/schemas/marketplace/listing-v2.schema.json"`;
  - `community_app_lints_and_builds`;
  - `bad_owner_refused`: `namespace-check` with a fixture event whose author is not the owner;
  - `node_only_dry_run_skips_with_message` (stdout contains `dry-run skipped: node-only package`);
  - `depends_missing_refused`;
  - `workflow_policy`: `grep` fails if any workflow has `pull_request_target`, or if `validate.yml` has `id-token` or `secrets.` in any job.
- [ ] **Step 2: Rewrite `validate.yml`.** Triggers: `pull_request`, `pull_request_review` (types `submitted`, `dismissed`) and `merge_group`. Permissions are `contents: read`. Jobs:
  - `listings`: changed files; lint without `--first-party-only`, with `--index`; the dry index build (v1 and v2); revocations schema check. A `package-needs-verified` warning becomes an error unless the PR adds `verified.json`.
  - `namespace`: `scripts/namespace-check.sh` gathers the PR event and `gh api …/pulls/<n>/reviews` (approved, latest per reviewer) and runs `loams-listing namespace-check`.
  - `packages`: `loams-listing verify --cosign "$(command -v cosign)"`, with cosign v3.0.6.
  - `dry-run`: for each added `app` or `engine` with a `cluster` target, `scripts/dry-run.sh`. If `$LOAMS/deploy/helm/loams-operator/Chart.yaml` is absent at the pin, it prints `dry-run skipped: operator v2 chart (plan OP2-A) is not at the pinned loams ref` and exits 0. Otherwise it caches `cargo install --git https://github.com/ostrium-labs/loams --rev "$(cat .github/loams-ref)" --locked loams-lic` by ref, then runs `prepare-index` and `run --cluster kind --profile dry-run`. Timeout 30 minutes.
  - `report`: unchanged in substance, plus a row per check, uploaded as `listing-report`.

  Delete the old `comment` job. `report-comment.yml` runs `on: workflow_run: workflows: [validate], types: [completed]` with `pull-requests: write` only. It downloads `listing-report` and posts `report.md` as the body with `gh pr comment --edit-last --create-if-none`. It never checks out or runs PR content.
- [ ] **Step 3: Write `.github/CODEOWNERS`.** Every path goes to the maintainers team recorded in Task 0 (Q1605). The rules that matter: `/listings/ostrium/`, `/listings/loams/`, `/publishers/`, `/revocations.json`, `/**/verified.json`, `/.github/` and `/schemas/`. Write `CONTRIBUTING.md` as the submission guide:
  - fork the repo and add `listings/<ns>/<name>/versions/<v>/listing.json`;
  - add `publishers/<ns>.json` on the first submission;
  - expect these checks;
  - Community and Verified.
- [ ] **Step 4: Push and run.** Run `gh pr create --repo ostrium-labs/loams-marketplace --draft`, then `gh run watch`. Expected: `self-test` PASS. `validate` PASS on the PR itself, which only adds publishers and the schema. `dry-run` is either skipped with its message or PASS.
- [ ] **Step 5: Test with a real fork pull request.** Open it from a personal fork with `tests/fixtures/community-app`'s listing under the author's namespace. Expected: all four checks green (or `dry-run` skipped), and the comment posted by `report-comment`. Then close it without merging.
- [ ] **Step 6: Commit** (squash-merge title) `index: open namespaces, v2 checks, kind dry-run and CODEOWNERS review (OP2-B Task 11, D2606, D2607, Q1605)`.

### Task 12 (loams-marketplace): Verified evidence and publishing index v2

**Files** (through `gh api`):
- Create: `.github/workflows/verified.yml`, `scripts/verified-evidence.sh`, `tests/fixtures/verified-app/`
- Modify: `.github/workflows/publish.yml`, `scripts/check-signed.sh`, `tests/self-test.sh`, `README.md` (the tiers and revocation runbook: who may edit `revocations.json`, `warn` vs `disable`, suspension through `publishers/<ns>.json`)

**Interfaces:**
- Consumes: `verified.v1.json` and the tier rules (Task 7); `loams-lic run --profile full` (Tasks 9–10); `loams-agentd agent conformance` (MK0 Task 8).
- Produces: the published `index.v2.json` and `index.v2.json.sigstore.json` under `TrustAnchor::official()`, and the required check `verified` (only when a PR adds `verified.json`).

- [ ] **Step 1: Write the failing self-tests:**
  - `verified_needs_suite_report`: `verified.json` whose `report_sha256` does not equal the job's report fails;
  - `verified_needs_security_contact`;
  - `publish_signs_index_v2` (with `SELF_TEST_SIGN=1`, `check-signed.sh` verifies all three bundles);
  - `unsigned_index_v2_fails`.

  Push them and watch `self-test` fail.
- [ ] **Step 2: Write `verified.yml`.** It runs on `pull_request` paths `**/verified.json`, with read-only permissions and no OIDC. Per added `verified.json`:
  - kinds `agent`, `harness` and `mcp-server` run `loams-agentd agent conformance <listing.json> --out report.json`, with `loams-agentd` built by the cached `cargo install … --rev <ref> --locked loams-agentd`;
  - kinds `app` and `engine` run `loams-lic run --cluster kind --profile full --out report.json`. While the operator chart is absent it fails with "Verified for apps and engines needs operator v2 (OP2-A)";
  - it then requires `sha256(report.json) == verified.json.report_sha256`, a passing report (LAPC level ≥ core, or every LIC MUST passed), and a listing `security_contact`.
- [ ] **Step 3: Update `publish.yml`:**
  - build both indexes (`index build . --source-commit $GITHUB_SHA`, no `--first-party-only`);
  - `cosign sign-blob --bundle` for `index.json`, `index.v2.json` and `revocations.json`;
  - `check-signed.sh` gains `index.v2.json`;
  - deploy Pages.
- [ ] **Step 4: Push and run** `gh run watch` on the PR. Expected: `self-test` PASS. After merge, `curl -fsS https://ostrium-labs.github.io/loams-marketplace/index.v2.json | jq .index_version` prints `2`. Also `cargo test -p loams-agentd-marketplace -j 4 --test signed_index -- --ignored live_official_index_v2_verifies` passes: it is an ignored live test added here, with the official anchor.
- [ ] **Step 5: Commit** (squash-merge title) `index: Verified evidence, publish and sign index v2 (OP2-B Task 12, D2608, D2609)`.

### Task 13 (loams): docs

**Files:**
- Modify: `docs/marketplace/publishing.md` (rewritten for v2: kinds, `package`, `targets`, `requires`, `depends_on`, `values_schema`, `cost_hint`, `run`, namespaces, the checks, tiers, revocation), `docs/marketplace/README.md` (status **preview**; open namespaces), `docs/plans/README.md` (an OP2-B row)

- [ ] **Step 1:** Write the docs. Every field name and check name must match the schema and `validate.yml` exactly.
- [ ] **Step 2: Run** the repository's Markdown link check (`pnpm -w run docs:links`, or the command `docs/plans/README.md` names). Expected: PASS.
- [ ] **Step 3: Commit** `git commit -s -m "docs: publishing to the open marketplace (OP2-B Task 13)"`.

## Not in this plan

- These are design 73 MK1 items not named in design 86's MK1, and follow later:
  - scanning (§73 §10.2: OSV, GuardDog, tool-poisoning beyond lint, the 3-day age rule);
  - domain namespaces;
  - the `python` runtime;
  - LAPC Compatible.
- The LIC device profile (EDGE1/EDGE2), `loams-registry` (MK2) and paid listings (D2610).
- Operator-side enforcement of D2626 (gVisor RuntimeClass, allow-listed kinds, the cluster-scoped confirmation) and the sync interval of D2609. These are OP2-A's; `verify_listing` and `is_revoked` are the inputs they call.

## Rulings made during execution

Recorded by Task 0 on 2026-11-02. Later tasks consume these.

### OP2-A names (as its draft plan states them)

- Status: none of this is on `main` as of 2026-11-02, and this lane uses no `dev` branch (P1). `deploy/helm/` holds only `loams-connectors`, `loams-meter-agent` and `loams-risingwave`; there is no `conformance/operator/`. OP2-A has not started. Source: [OP2-A plan](2026-11-02-op2a-operator-v2-core.md), "Interfaces for sibling plans" and Task 18.
- Chart: `deploy/helm/loams-operator` (OCI `oci://ghcr.io/ostrium-labs/charts/loams-operator`), installs only into `loams-system`.
- Marketplace values: `marketplace.index` (default `https://ostrium-labs.github.io/loams-marketplace/index.v2.json`), `marketplace.publicKey` (PEM; empty means `TrustAnchor::official()`), `marketplace.configMap` (optional ConfigMap with the four index files, mounted read-only at `/etc/loams/marketplace/`). With `home.create`, `home.spec.marketplace` takes these values.
- `file://` sources: yes, but only under `/etc/loams/marketplace/` (for example `file:///etc/loams/marketplace/index.v2.json`). Any other path fails with `SourceError::Unavailable`.
- What OP2-B still needs from OP2-A, both yes:
  - `spec.marketplace.publicKey` (PEM, becomes `TrustAnchor::PublicKey`);
  - an in-kind index URL: the `file://` ConfigMap mount above, plus `FixtureSource` for controller unit tests.
- `LoamsInstall`: `spec { cluster, listing { id, version }, values, storage[{name, binding}], exposure, project (default "default"), sandbox { allowWithoutGvisor }, approvedClusterScoped }`. Status adds `access { identityId, tuplesWritten, meterSubject }`, `listing { tier, digest }`, `clusterScoped { objects, digest }`, `dependencies`. Conditions: `Ready` (`kubectl wait loamsinstall/<name> --for=condition=Ready`), `AwaitingConfirmation`, `AccessReady`, `Revoked`, `Degraded`, `Sandboxed`. Namespace `loams-i-<install name>`. Home: `LoamsCluster home` in `loams-system`, `kubectl wait loamscluster/home --for=condition=Ready`.
- LOC harness: crates `conformance/operator/loc-spec` and `conformance/operator/loc-harness` (plus `adapters/loams`). The kind cluster type is `ClusterKind::Kind`, used as `run(adapter, level, class, ClusterKind::Kind) -> Report`; its kind config is `conformance/operator/loc-harness/kind/cluster.yaml`.

### Pinned tool versions (each at least 14 days old, so published on or before 2026-10-19)

- kind `v0.33.0`, released 2026-08-26 (newest stable; `kubernetes-sigs/kind` releases).
- `kindest/node` `v1.37.0@sha256:a1ed56cfb0e7b93589bdf97c8cd566405a265939e3620fc4f5de89adff580ae5`, the default node image of kind v0.33.0 (its release notes; always use the digest). Fallback lines in the same notes: v1.36.4 `sha256:099e049362a1526b2db71494e1947aae99bd16290d7c895f2b7ea312e3cbfaed`.
- Helm `v3.22.1`, released 2026-10-15 (18 days before 2026-11-02). Helm `v4.3.1` was released the same day. Ruling: use the 3.x line, as it is the conservative one for `helm lint` and `helm template`. OP2-A's R4 also uses the `helm` CLI; a bump to 4.x is a separate change. Verify the downloaded binary by the checksum file Helm publishes.
- cosign `v3.0.6` (released 2026-04-06) stays pinned for signing. Newer 3.x releases exist; this plan does not move.

### Cosign v3.0.6 `sign-blob` bundle shape

Source: `cmd/cosign/cli/sign/sign_blob.go`, `pkg/cosign/bundle/protobundle.go` and `cmd/cosign/cli/signcommon/common.go` at tag `v3.0.6`.

- Flags: the documented command line fails on v3.0.6. `--use-signing-config` defaults to `true`, and `LoadTrustedMaterialAndSigningConfig` rejects it with `--tlog-upload=false` ("--tlog-upload=false is not supported with --signing-config or --use-signing-config"). Use `cosign sign-blob --key <k> --bundle <f> --tlog-upload=false --use-signing-config=false <file>`. `--new-bundle-format` defaults to `true`.
- With a plain key (no certificate), the code path writes a Sigstore protobuf bundle as protojson. The shape:

```json
{
  "mediaType": "application/vnd.dev.sigstore.bundle.v0.3+json",
  "verificationMaterial": {
    "publicKey": { "hint": "<base64 of sha256(PKIX DER of the public key)>" }
  },
  "messageSignature": {
    "messageDigest": { "algorithm": "SHA2_256" /* string unconfirmed */, "digest": "<base64 of the raw sha256 of the file>" },
    "signature": "<base64 of the signature bytes>"
  }
}
```

- Task 3's `KeyVerifier` reads `messageSignature.signature` (base64 standard) as the signature over the exact file bytes. For an ECDSA P-256 key (cosign's default `cosign generate-key-pair`) it is ASN.1 DER ECDSA over the SHA-256 of the file. `verificationMaterial.publicKey.hint` is only a hint, not the key. The key comes from `TrustAnchor::PublicKey { pem }`. The hint may be used to reject a bundle early when it differs from sha256(DER of the configured key), but a mismatch must not be the only check.
- With `--tlog-upload=false` and no `--rfc3161-timestamp`, no `tlogEntries` or timestamp data are expected.
- Not confirmed from source alone: the exact whitespace and key order of the protojson output (protojson is deliberately not stable, so parse it; never compare bytes), and the exact `algorithm` string for the digest. Task 3 must parse leniently (ignore unknown fields) and Task 2/3 tests must use a bundle generated by a real cosign v3.0.6, committed as a fixture.

### Org membership check for the namespace gate

- `GET /orgs/{org}/public_members/{username}` answers `204 No Content` when the user is a public member and `404 Not Found` when not (GitHub REST docs, "Check public organization membership for a user"). It lists only public members, so a private member also gets 404.
- Verified read-only on 2026-11-02: `gh api -i /orgs/ostrium-labs/public_members` returns `[]` (nobody is public), and `/orgs/ostrium-labs/public_members/dina-kar` returns 404 while `/orgs/ostrium-labs/members/dina-kar` returns 204 for the authenticated owner.
- Ruling (implemented by Task 8, tests `org_non_member_refused` and friends): for a non-reserved org namespace, refuse unless the PR author or an approver gets 204 from `public_members` or is listed in `publishers/<ns>.json` `maintainers`. For reserved namespaces (mapped to ostrium-labs, which has zero public members), use the `publishers/<ns>.json` `maintainers` list only. A 404 means "not a public member" and is not by itself a refusal when the publishers-file list covers the user. The `workflow_policy` self-test stays the fork-safety proof.

### CODEOWNERS and the maintainers team

- `GET /orgs/ostrium-labs/teams` works for the owner's token. Teams: `maintainers` (slug `maintainers`, "Trusted maintainers: merge dev into main, manage teams and releases") and `committers`.
- Ruling: CODEOWNERS in `loams-marketplace` lists `@ostrium-labs/maintainers` for the registry-owned paths, with `@dina-kar` as a second owner until team membership is confirmed to cover reviewers. The team must have write access to `ostrium-labs/loams-marketplace`; its repository permission is not checked here. Owner follow-ups, not done by this lane: grant the team access to the repo, set branch protection on `main` that requires code-owner review and the validate checks, and drop the `@dina-kar` second CODEOWNER once team membership and access are confirmed.

### Lane rulings (controller)

- P1: loams PRs target `main`, not `dev` (owner brief; the OP2 plan/design PRs landed on main).
- P2: loams-marketplace is cloned locally at `~/Documents/Ostriumlabs/loams-marketplace` (owner approved) and changed through branches and PRs, superseding "never clone it".
- P3: one loams worktree `loams-wt/op2b` with a branch per task, since tasks run sequentially.
- P4: `ring` and `KeyVerifier` stay in the feature-free core; `testkit` without `agentd` needs only ring and serde_json.
- P5: Task 2 adds `ListingKind` beside `index::Kind`; Task 3 turns `index::Kind` into `pub type Kind = ListingKind`.
- P6: Task 11 Step 5 (a real fork pull request from a personal fork) is an owner follow-up; the lane covers fork safety with the `workflow_policy` self-test.
- P7: Task 12's ignored live test `live_official_index_v2_verifies` ships in a loams PR after the publish lands.
- P8: `review:greptile` labels are applied, but merges do not wait on Greptile.
