# OP2-A — Loams Operator v2 Core (design 86 phase OP2-0) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Where this plan gives exact names, paths, fields or tests, use them verbatim. Record any deviation in "Rulings made during execution" at the end of this file.
>
> **Status: Plan** (2026-11-02). Track OP2, design [§86](../design/86-open-platform-operator-v2.md) phase **OP2-0** (D2601–D2604, D2623–D2626, D2630, and §4.3 Loams Access D2632–D2633), amending [§65](../design/65-loams-operator.md) (D1172–D1201). This plan **supersedes the stub** [OP1](2026-10-19-op1-loams-operator.md): it keeps OP1's crate split, kube version rule, HA model, Review Focus items 1–3 and 7, and LOC Core, and drops the v1 component kinds (`LoamsEngine`, `LoamsPD`, `LoamsTiKV`, `LoamsGateway`, `ObjectStore`, `LoamsNamespace`), which §86 replaces with the home's platform base and `engine` listings. Sibling plans written in parallel: **OP2-B** (marketplace, `listing/v2`, owns `crates/loams-agentd-marketplace`) and **OP2-C** (one-click deploy, consumes this plan's chart and home YAML). The `loams.access.v1` API, the console Access area and the `loams access` CLI (D2634) are **not** in this plan; they are follow-up plan **OP2-D**, which builds on this plan's Access provisioner and audit events. **Starts after UP1 merges** (§86 §8).

**Goal:** `helm install loams-operator` plus one home `LoamsCluster` brings up the platform base (gateway, agentd, and **Loams Access**: the identity provider (official Zitadel via `loams-zitadel`/`loams-identity-sync` by default, `loams-identity` selectable), official OpenFGA and the Meter) on kind or k3s, and a `LoamsInstall` of a signed `oci` or `helm` listing (with its `depends_on` installed first and an `object` `StorageBinding` wired in) runs in its own namespace under the D2626 tier limits, with its machine identity, OpenFGA tuples and Meter subject created and removed together in one journaled sequence and recorded as `io.loams.dev.access.*` CloudEvents (D2633); the operator is HA (leader Lease, journal in status, fences) and passes LOC Core on kind in CI.

**Architecture:** Three crates, none in the `loams` binary's default build: `loams-operator-api` (CRD types for `LoamsCluster`, `LoamsInstall`, `StorageBinding`, `LoamsDevice`, `LoamsFleet`; `crdgen`), `loams-kube` (leader election, fence token, disruption Lease, operation journal, server-side apply, drift, conditions, events, metrics) and `loams-operator` (controllers, the client-side Helm renderer and manifest policy, the OCI renderer, the listing source over OP2-B's verifier, the TiKV `bridge` engine driver over official tidb-operator v2, the object-store probe, the Access provisioner). LOC lives under `conformance/operator/` (`loc-spec`, `loc-harness`, the Loams adapter). One Helm chart `deploy/helm/loams-operator` and one multi-arch image `ghcr.io/ostrium-labs/loams-operator`.

**Tech Stack:** Rust 1.97; `kube` =3.1.0 / `k8s-openapi` =0.27.1 (workspace pins) with crate-level features `runtime` and `derive`; `schemars` 1.x (what `kube-core` 3.1.0 requires); `object_store` 0.14 (workspace) for the probe; `loams-zitadel`, `loams-authz` (OpenFGA client) and `loams-cloudevents` (workspace crates) for Access; the official `helm` CLI (pinned) inside the operator image for client-side rendering; kopium (dev tool, output checked in) for tidb-operator v2 types; kind on GitHub-hosted runners.

**Spec:** [§86](../design/86-open-platform-operator-v2.md) §4, §4.1, §4.2, §4.3 (D2632–D2634), §5 (D2609), §8 OP2-0; [§65](../design/65-loams-operator.md) §5–§7, §10–§12, §17 (as amended by §86); [§76](../design/76-identity-and-authorization.md) §12 (Zitadel/OpenFGA deployment, pins); [§73](../design/73-agent-marketplace.md) §10 (tiers).

---

## Interfaces for sibling plans

OP2-C and OP2-B may rely on exactly these names. A change to any of them is a ruling recorded here and announced to both plans.

| Interface | Value |
|---|---|
| API group / version | `loams.dev` / `v1alpha1`; all kinds namespaced |
| Kinds (plural, short) | `LoamsCluster` (`loamsclusters`, `lc`), `LoamsInstall` (`loamsinstalls`, `li`), `StorageBinding` (`storagebindings`, `sb`), `LoamsDevice` (`loamsdevices`, `ldev`), `LoamsFleet` (`loamsfleets`, `lfleet`) |
| Operator namespace | `loams-system` (the chart refuses any other release namespace) |
| Home object | `LoamsCluster` named `home` in `loams-system`, `spec.role: home` |
| Home example (consumed by OP2-C) | `deploy/operator/home.yaml` (k3s/kind single-node home, `exposure.mode: internal`), with the Secrets it references named in comments |
| Install example | `deploy/operator/install.example.yaml` |
| Chart path | `deploy/helm/loams-operator` (chart `version` = workspace version) |
| Chart registry | `oci://ghcr.io/ostrium-labs/charts/loams-operator` |
| Chart values | `image.repository` (default `ghcr.io/ostrium-labs/loams-operator`), `image.digest` (required; the release fills it), `replicas` (default `2`), `home.create` (bool, default `false`), `home.spec` (object, the `LoamsCluster` spec; rendered as `home` when `home.create`), `marketplace.index` (default `https://ostrium-labs.github.io/loams-marketplace/index.v2.json`; `file:///etc/loams/marketplace/index.v2.json` for kind and the PR dry-run), `marketplace.publicKey` (PEM; empty = OP2-B's `TrustAnchor::official()` keyless anchor), `marketplace.configMap` (optional ConfigMap holding the four index files, mounted read-only at `/etc/loams/marketplace/`). With `home.create`, `home.spec.marketplace` defaults to these values |
| Image | `ghcr.io/ostrium-labs/loams-operator:<version>`, manifest list for `linux/amd64` and `linux/arm64`, signed like the other release images |
| Readiness | `kubectl -n loams-system wait loamscluster/home --for=condition=Ready --timeout=15m`; per install `loamsinstall/<name> --for=condition=Ready` |
| Cluster-scoped confirmation | The install shows `AwaitingConfirmation=True`; `status.clusterScoped.objects` lists `{apiVersion, kind, name}`, `status.clusterScoped.digest` is `sha256:<hex>`, ConfigMap `cluster-scoped` in the install namespace holds the manifest under key `manifest.yaml`. The confirming client applies that manifest with **its own** credentials (server-side apply, field manager `loams-approver`) and sets `spec.approvedClusterScoped` to the digest |
| Install namespace | `loams-i-<install name>`; if longer than 63 characters, `loams-i-<first 46>-<8 hex of sha256(name)>` |
| Workload-identity ServiceAccounts | `storage-<binding>` in the binding's namespace (probe), `install` in each install namespace. IAM trust for IRSA / GKE WI / Azure WI may use `system:serviceaccount:loams-i-*:install` |
| Home Access fields | `spec.base.access.org` (the tenant tree's org id), `spec.base.access.identity.provider: zitadel \| loams-identity` (default `zitadel` in OP2-0), `spec.base.access.audit.sink` (optional URL for CloudEvents, HTTP binary mode) |
| Install Access fields | `spec.project` (default `default`); `status.access { identityId, tuplesWritten, meterSubject }`; condition `AccessReady` |
| Access subject id | `org/<org>/project/<project>/install/<install uid>` (EDGE1 adds `.../device/<uid>`); Zitadel machine username `install-<uid>`; OpenFGA object `install:<uid>`; Meter subject = the subject id |
| Access audit events | CloudEvents 1.0, `source` `loams-operator/<home uid>`, `subject` = the subject id, deterministic `id` `<operation id>/<step>`; types `io.loams.dev.access.identity.created`, `.identity.deleted`, `.permission.granted`, `.permission.revoked`, `.meter.subject.created`, `.meter.subject.deleted` |
| Pod labels (Meter, §84) | `loams.dev/install`, `loams.dev/listing` (id with `/` as `.`), `loams.dev/listing-version`, `loams.dev/tier` |
| Consumed from OP2-B (`crates/loams-agentd-marketplace`, with `default-features = false, features = ["fetch"]`; dev adds `"testkit"`) | Exactly OP2-B's "Interfaces for sibling plans" ([plan](2026-11-02-op2b-open-marketplace.md)): `ListingV2` (`id`, `kind`, `version`, `package: Option<Package>`, `requires: Requires { storage: Vec<StorageKind>, cpu_millis, memory_mib, gpu }`, `targets`, `depends_on: Vec<DependsOn>`, `values_schema`, `run: Option<Run { ports, health, args }>`, `validate_values`), `ListingKind`, `Package` (`Helm.chart` is `oci://…` with a manifest digest or an `https://…/.tgz` with a file sha256), `StorageKind`, `Tier`, `IndexFiles`, `TrustAnchor { Keyless, PublicKey { pem } }`, `SignedIndex::verify(&IndexFiles, &TrustAnchor)`, `SignedIndex::revocations()`, `VerifiedListing { listing, tier, listing_sha256, warning }`, `VerifyError`, `verify_listing`, `resolve_depends_on` (install order, deps first, root excluded, depth ≤ 8), `RevocationList`, `is_revoked` (true only for `disable`), `testkit::signed_v2_index`. Index files: `index.v2.json`, `index.v2.json.sigstore.json`, `revocations.json`, `revocations.json.sigstore.json` from one base URL |

## Decisions made in this plan (spec gaps, decided)

Each is the author's call under the owner's "do the best"; record changes in "Rulings made during execution".

- **R1 — No component kinds.** §65's `LoamsEngine`/`LoamsPD`/`LoamsTiKV`/`LoamsGateway`/`ObjectStore`/`LoamsNamespace` are not built. The home's platform base is rendered from `LoamsCluster` directly; engines are `LoamsInstall`s (D2623); `StorageBinding` `object` replaces `ObjectStore` and keeps its capability probe (D1190).
- **R2 — Cluster-scoped objects are applied by the confirming user, not the operator.** The operator holds no `escalate`/`bind` (§65 §11.1), so it cannot create ClusterRoles it does not hold. It renders them, lists them with a digest, and waits; it then checks that each listed object exists with the rendered content hash before applying the namespaced rest. This is D2626's "separate user confirmation listing them".
- **R3 — Operator RBAC.** One ClusterRole with the allow-listed namespaced kinds (Secrets `get/create/update/patch/delete`, never `list/watch`), bound cluster-wide and narrowed by the chart's ValidatingAdmissionPolicy `loams-operator-namespaces` to `loams-system` and `loams-i-*`. A chart Role that grants more than the operator holds fails to apply (`ApplyForbidden`), so no listing gets more than the operator.
- **R4 — Helm rendering shells out to the pinned official `helm` CLI** (`helm template --no-hooks --include-crds`), chart bytes checked against `Package::Helm.digest` before rendering. Listings: hooks always dropped, and any object still carrying `helm.sh/hook` is refused. Platform-base pins (first-party, from `pins/releases.toml`) may set `hooks = "ordered-jobs"`: pre-install/pre-upgrade hook Jobs are kept and run, in weight order, before the rest (needed if Zitadel's chart uses hooks; Task 0 checks).
- **R5 — Kind policy per tier.** Community: `oci` only; Verified and FirstParty: `oci` and `helm`; `kustomize` refused in OP2-0 (`PackageNotSupportedYet`); `wasm`/`binary` refused for cluster installs (`PackageNotForCluster`). Agent, harness and MCP-server kinds are refused in OP2-0 (`ListingKindNotSupportedYet`); the agentd handoff (D2625) is a later plan. Validating/mutating webhooks: FirstParty only, as cluster-scoped objects under R2.
- **R6 — gVisor detection.** A RuntimeClass is gVisor if its `handler` is `runsc` or `gvisor` (GKE Sandbox) or `LoamsCluster.spec.sandbox.runtimeClassName` names it. Without one, Community installs are refused (`SandboxUnavailable`) unless `LoamsInstall.spec.sandbox.allowWithoutGvisor: true`, which sets `Sandboxed=False` and emits a `Warning`.
- **R7 — OCI app contract.** One container; ports, readiness and args from `ListingV2.run` (`Run { ports, health, args }`); when `run` is absent, port `8080` named `http` and readiness `GET /healthz` on it. Restricted Pod Security, read-only root with an `emptyDir` at `/tmp`, requests from `requires.cpu_millis`/`memory_mib`; a `requires.gpu` listing is refused in OP2-0 (`GpuNotSupportedYet`).
- **R8 — Access databases.** The identity provider and OpenFGA take a Postgres DSN from a user-created Secret (`database.secretRef`), the D2603 "Secret the user created" path; DEP2 moves it to a `sql-postgres` binding. `identity.provider: loams-identity` is accepted by the schema but renders only when `pins/releases.toml` carries a `loams_identity` image digest; otherwise `AccessReady=False`, reason `IdentityProviderUnavailable` (D2632: Zitadel until `loams-identity` passes OIDC conformance).
- **R16 — Access provisioning (D2633).** Every `LoamsInstall` gets, in one journaled sequence before its workloads are applied: (1) a machine identity (Zitadel machine user `install-<uid>`, **no key** in OP2-0; keys and the token file come with OP2-D/§73's contract), (2) OpenFGA tuples `install:<uid>#project@project:<project>` and `install:<uid>#operator@service_account:<identity id>`, so the project's `owner`/`operator`/`viewer` flow to the install, (3) a Meter subject under the project with the project's budgets as entitlements. Delete runs the reverse (Meter, tuples, identity). Every step is read-first and keyed by the deterministic names above, so a crash between a remote write and the status write never leaves an orphan. The OpenFGA model gains type `install` (relations `project`, `owner`, `operator`, `viewer`, each also `from project`), added to `deploy/openfga/core.fga` with a model test; the project-level `owner`/`operator`/`viewer` names come from the D2633 tree (Task 0 maps them onto the model's `admin`/`editor`/`viewer` if OP2-D has not renamed them yet). The Access steps are generic over `AccessSubject`, so EDGE1 reuses them for `LoamsDevice`.
- **R17 — Access audit delivery.** Each Access step's CloudEvent is a journal step of its own (`audit`), delivered at least once to `spec.base.access.audit.sink` with the deterministic id (receivers dedupe on it); with no sink, events go to the operator's structured log and a Kubernetes Event only. The `_system` stream sink for the audit log is OP2-D's choice.
- **R9 — Dependencies.** Resolution (versions, revocations, cycles, depth ≤ 8) is OP2-B's `resolve_depends_on`; its `VerifyError::Dependency` maps to reason `DependencyUnresolved`. The operator only decides placement: platform-base ids are skipped; an existing `LoamsInstall` of that listing in the same home namespace whose version satisfies the range is reused; otherwise the operator creates `dep-<listing namespace>-<listing name>` labelled `loams.dev/dependency=true`, with a non-controller ownerReference from each dependent, so Kubernetes GC removes it with its last dependent.
- **R10 — Revocation and the index source.** The four index files are fetched from `spec.marketplace.index`'s base (`https://` through OP2-B's `HttpsFetcher`; `file://` only under `/etc/loams/marketplace/`) and verified with `SignedIndex::verify` against `spec.marketplace.publicKey` (`TrustAnchor::PublicKey`) or `TrustAnchor::official()`, every `spec.marketplace.refresh` (default `5m`). A `warn` revocation (`VerifiedListing.warning`) keeps the install running with a `Warning` event and `Degraded=True`, reason `ListingRevocationWarning`. A `disable` revocation (`is_revoked`) on a running install gets `Revoked=True`, `Ready=False`, and its Deployments/StatefulSets are scaled to 0 (PVCs kept). An index outage keeps running installs as they are and holds new ones at `IndexUnavailable`.
- **R11 — TiKV through the bridge.** First-party listing `loams/tikv` (kind `engine`) is installed by the compiled-in `TikvBridgeDriver`, which renders tidb-operator v2 `Cluster`, `PDGroup`, `TiKVGroup` (no `TiDBGroup`, D179) and maps their status. It `depends_on` `loams/tidb-operator` (kind `engine`, `helm`, the official chart by digest; its CRDs and ClusterRoles go through R2). Drivers apply only to `Tier::FirstParty` listings named in `DriverRegistry`.
- **R12 — Meter in OP2-0** is the `loams` image in role `meter` (part of Access) plus the Meter pod labels on every install. The meter node agent keeps its own chart (`deploy/helm/loams-meter-agent`, privileged), installed outside the operator.
- **R13 — No admission webhooks in OP2-0.** Validation is CEL in the CRDs plus the chart's ValidatingAdmissionPolicies; no conversion webhook (one served version).
- **R14 — `crdgen` lives in `loams-operator-api`** (`src/bin/crdgen.rs`), not `loams-operator` as §65 §6.1 says, so CRDs regenerate without building the controllers (light builds).
- **R15 — LOC scope.** OP2-0 ships `loc-spec` 0.1 (Core + HA IDs), `loc-harness` (kind, audit log, pod-kill and stale-leader injectors, report JSON) and the Rust Loams adapter. The stdio `loc-adapter`, drain/partition/node-stop injectors and Day-2 are OP4/later. LOC Core runs per PR on kind; LOC HA (`LOC-HA-001`, `-002`) runs locally (`scripts/operator/loc-ha.sh`), since nightlies are local only.

## Global Constraints

- **Worktrees and branches.** `~/Documents/Ostriumlabs/loams-wt/op2a-<task>`; one branch and PR per task into `dev`; `git commit -s`. Label PRs of Tasks 1, 8, 10, 13, 14 and 15 `review:greptile` (new API, security).
- **Builds.** Do not set `CARGO_TARGET_DIR`; never build in `/tmp`; one heavy build at a time; `-j 4`; `cargo test -p <crate> -j 4`, never `--workspace`. kind clusters run only between builds.
- **The default build does not change.** Test `default_build_excludes_operator` (`scripts/operator/check-default-build.sh`): `cargo tree -p loams -e normal` on default features lists none of `loams-operator`, `loams-kube`, `loams-operator-api`, `loc-harness`, `loc-spec`.
- **No forks (D2450/D2600).** Pins reference only official upstream images and charts; test `pins_reference_no_fork_images` refuses any `ghcr.io/ostrium-labs/` reference other than Loams' own images (`loams`, `loams-gateway`, `loams-agentd`, `loams-operator`, `loams-identity-sync`, `loams-identity`).
- **Versions.** `kube` and `k8s-openapi` from the workspace (=3.1.0, =0.27.1); `schemars = "1"` crate-local in `loams-operator-api`; Kubernetes floor **1.30** (VAP GA); images and charts by digest, releases at least 14 days old except security fixes.
- **Licences.** Zitadel (AGPL-3.0) only as its unmodified official image through its upstream chart (D1602); nothing AGPL/GPL/BSL/SSPL/OSL linked; tidb-operator types generated from Apache-2.0 CRDs carry `Generated by kopium from pingcap/tidb-operator@<sha>, Apache-2.0`; `cargo deny check` passes with no new allow entry.
- **No exec.** No code path calls `pods/exec` or `pods/attach`; test `rbac_has_no_exec`.
- **Never public (D1195).** No `LoadBalancer` without an internal annotation from the fixed list, no `NodePort`, `Ingress` or Gateway API route, from any renderer or listing; test `no_public_service_rendered`.
- **Secrets.** Specs carry references only; no secret value in status, events, logs or metrics (LOC `LOC-SEC-002`).
- **Least privilege.** No `*` verbs, `escalate`, `bind`, `impersonate`, or cluster-wide Secret `list/watch` (LOC `LOC-SEC-001`).
- **Runners.** GitHub-hosted only; kind jobs path-filtered on `crates/loams-operator*`, `crates/loams-kube`, `conformance/operator/`, `deploy/helm/loams-operator/`; no scheduled workflows.
- **Status labels.** Docs and chart say **preview**.

## Review Focus

1. **A chart re-renders differently after approval** (values edited, or a new chart version) while the old approval digest is set. Expected: `AwaitingConfirmation=True` again and nothing new applied. Test `approval_digest_changes_with_values` (Task 14).
2. **A chart templates an object into another namespace** (`kube-system`, `loams-system`, the home namespace). Expected: the whole render refused with `ForeignNamespace`, nothing applied. Test `foreign_namespace_object_refused` (Task 8).
3. **The index is unreachable** (GitHub Pages outage). Expected: running installs keep running, revocations from the last good list still apply, new installs wait at `IndexUnavailable`, not `Failed`. Test `index_outage_keeps_running_installs` (Task 11).
4. **A listing's digest changes under the same version** (re-pushed tag or re-signed index). Expected: refused with `ListingDigestChanged`, the running version untouched. Test `listing_digest_change_for_same_version_refused` (Task 14).
5. **A dependency is deleted while a dependent still runs.** Expected: an auto-created dependency stays until its last dependent goes; a user-created one gets a finalizer-held `Warning` and the dependent goes `Degraded` with `DependencyMissing`. Test `dependency_kept_while_dependent_exists` (Task 16).

OP1's items carried over: two leaders acting (Task 4 `stale_leader_cannot_take_disruption_lease`), a paused leader writing (Task 3 `self_fence_after_missed_renewal`, LOC `LOC-HA-002`), a crash repeating a step (Task 4 `journal_resume_each_phase`), restarts restarting Pods (LOC `LOC-REC-002`), and anything public/exec/secret (Global Constraints).

---

## File structure

```
crates/loams-operator-api/          types only
  src/lib.rs, common.rs (Condition re-export, OperationStatus, SecretRef, Exposure)
  src/cluster.rs  src/install.rs  src/storage.rs  src/device.rs  src/bin/crdgen.rs
  tests/crds.rs  tests/it_kind_cel.rs
crates/loams-kube/                  shared runtime
  src/lease.rs  src/fence.rs  src/journal.rs  src/conditions.rs  src/events.rs
  src/apply.rs  src/drift.rs  src/metrics.rs
crates/loams-operator/
  src/main.rs (run | probe-object)  src/manager.rs
  pins/releases.toml  src/pins.rs
  src/render/{mod.rs,helm.rs,policy.rs,oci.rs,base.rs}
  src/listing.rs  src/admit.rs  src/storage/{mod.rs,probe.rs}
  src/cluster.rs  src/install/{mod.rs,deps.rs,revoke.rs}
  src/access/{mod.rs,identity.rs,permissions.rs,meter.rs,audit.rs}
  src/drivers/{mod.rs,tikv_bridge.rs,tidb_operator_types.rs}
  tests/*.rs, tests/fixtures/
deploy/helm/loams-operator/         Chart.yaml values.yaml crds/ templates/ tests/
deploy/operator/home.yaml  install.example.yaml
deploy/release/Containerfile.loams-operator
conformance/operator/loc-spec/  loc-harness/  adapters/loams/
docs/api/operator-reasons.md  docs/operator/README.md
.github/workflows/operator.yml  scripts/operator/{check-default-build.sh,loc-ha.sh}
```

---

### Task 0: Rulings

**Files:**
- Modify: `docs/plans/2026-11-02-op2a-operator-v2-core.md` ("Rulings made during execution")

**Interfaces:**
- Consumes: OP2-B's merged or drafted `crates/loams-agentd-marketplace` API.
- Produces: recorded rulings that later tasks cite.

- [ ] **Step 1: Check and record each item**
  1. UP1 merged: no fork image or crate left in `deploy/` or `Cargo.toml` that this plan's pins would pull.
  2. OP2-B's merged API still matches its "Interfaces for sibling plans" (names above); whether `resolve_depends_on` treats platform-base ids (`PLATFORM_BASE_IDS`, Task 16) as implicit or expects the operator to strip them first.
  3. `kube` 3.1.0 with features `runtime` and `derive` builds against `schemars` 1.x; `kube-runtime`'s leader election is still absent (kube-rs#485).
  4. Pins at the newest release ≥ 14 days old: `helm` CLI, tidb-operator v2 chart and its CRD sha, PD/TiKV images, `zitadel/zitadel-charts`, OpenFGA chart 0.3.16 (or newer), RustFS image for tests, kind node image for Kubernetes 1.30–latest.
  5. Whether the Zitadel and tidb-operator charts use Helm hooks (R4), run under the `restricted` Pod Security level, and publish OCI charts (else `--repo`).
  6. Which `loams-gateway` modules and config a home needs (edge listener, routes to agentd and Zitadel); the `meter` role's command and store (R12); whether a `loams-identity-sync` image exists (else build it in Task 19's image matrix).
  7. Access backends: `loams-zitadel`'s `create_machine_user`/`deactivate` and a lookup by username; whether `loams-authz`'s `OpenFgaAuthorizer::write` fails on an existing tuple (else read first); the Meter call that creates a subject with entitlements (`Grant`) and deletes it (role `meter`'s API, §63), and where a project's budgets are read from; the D2633 project role names versus `deploy/openfga/core.fga`'s `admin`/`editor`/`viewer` (R16); whether OP2-D has already fixed a `loams-identity` image pin.
  8. A tiny official image that serves `GET /healthz` on 8080 for the OCI fixture (candidate: `registry.k8s.io/e2e-test-images/agnhost` `netexec`).

- [ ] **Step 2: Commit**

```bash
git add docs/plans/2026-11-02-op2a-operator-v2-core.md
git commit -s -m "plan(op2a): Task 0 rulings"
```

### Task 1: `loams-operator-api` core kinds and `crdgen`

**Files:**
- Create: `crates/loams-operator-api/{Cargo.toml,src/lib.rs,src/common.rs,src/cluster.rs,src/install.rs,src/storage.rs,src/bin/crdgen.rs}`
- Create: `deploy/helm/loams-operator/crds/*.yaml` (generated), `scripts/operator/check-default-build.sh`
- Test: `crates/loams-operator-api/tests/crds.rs`, `crates/loams-operator-api/tests/it_kind_cel.rs`

**Interfaces:**
- Produces (all `#[derive(CustomResource, JsonSchema, Serialize, Deserialize, Clone, Debug)]`, group `loams.dev`, version `v1alpha1`, namespaced, `status` subresource):
  - `LoamsClusterSpec { role: ClusterRole /* Home only */, release: String, exposure: Exposure, tls: Tls, storage: Option<ClusterStorage { object: String }>, sandbox: Sandbox { runtime_class_name: Option<String> }, marketplace: Marketplace { index: Url /* https:// or file:///etc/loams/marketplace/… */, public_key: Option<String> /* PEM */, refresh: Duration /* default 5m */ }, base: PlatformBase }`
  - `PlatformBase { gateway: Replicas, agentd: Replicas, access: Access }`; `Access { org: String, identity: AccessIdentity { provider: IdentityProvider { Zitadel, LoamsIdentity } /* default Zitadel */, replicas: u32, external_domain: String, database: DbRef }, openfga: OpenFga { replicas: u32, database: DbRef }, meter: Replicas, audit: Audit { sink: Option<Url> } }`; `DbRef { secret_ref: SecretRef /* key "dsn" */ }`
  - `Exposure { mode: ExposureMode { Tailnet, Internal } }` (default `Tailnet`); `InstallExposure { mode: InstallExposureMode { None, Tailnet, Internal, GatewayEdge } }` (default `None`; `GatewayEdge` refused in OP2-0)
  - `LoamsInstallSpec { cluster: String, listing: ListingRef { id: String, version: String }, values: serde_json::Value, storage: Vec<StorageRef { name: String, binding: String }>, exposure: InstallExposure, project: String /* default "default" */, sandbox: InstallSandbox { allow_without_gvisor: bool }, approved_cluster_scoped: Option<String> }`
  - `StorageBindingSpec { kind: StorageKind { Object, SqlMysql, SqlPostgres, Kafka, Kv }, object: Option<ObjectSpec { provider: ObjectProvider { S3, Gcs, Azure, R2 }, bucket, region: Option, endpoint: Option<Url>, prefix: Option }>, sql_mysql / sql_postgres / kafka / kv: Option<..> /* types only */, credentials: Credentials { workload_identity: Option<WorkloadIdentity { aws_role_arn, gcp_service_account, azure_client_id }>, secret_ref: Option<SecretRef> } }`
  - `common::OperationStatus { id: String, kind: String, plan: Vec<String>, step: Option<StepStatus { item: String, phase: String, since: Time }>, done: Vec<String>, fence: i32 }`; every status has `observed_generation: Option<i64>`, `conditions: Vec<metav1::Condition>`, `summary: Option<String>`, `operation: Option<OperationStatus>`
  - `LoamsInstallStatus` adds `access: Option<AccessStatus { identity_id: Option<String>, tuples_written: bool, meter_subject: Option<String> }>`, `namespace`, `listing: Option<ResolvedListing { tier, digest }>`, `cluster_scoped: Option<ClusterScoped { objects: Vec<ObjectId>, digest: String }>`, `dependencies: Vec<ResolvedDependency { id, version, install }>`
  - `StorageBindingStatus` adds `capabilities: Option<ObjectCapabilities { conditional_put: bool, range_read: bool, multipart: bool }>`
  - `fn all_crds() -> Vec<CustomResourceDefinition>`; binary `crdgen [--check]`

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn crds_match_checked_in() { /* all_crds() serialised == deploy/helm/loams-operator/crds/*.yaml */ }
#[test] fn every_kind_is_namespaced_v1alpha1_loams_dev() { for c in all_crds() { assert_eq!(c.spec.group, "loams.dev"); assert_eq!(c.spec.scope, "Namespaced"); assert_eq!(c.spec.versions[0].name, "v1alpha1"); } }
#[test] fn install_defaults() { let s: LoamsInstallSpec = from_yaml("cluster: home\nlisting: {id: a/b, version: 1.0.0}"); assert_eq!(s.exposure.mode, InstallExposureMode::None); assert!(!s.sandbox.allow_without_gvisor); assert_eq!(s.project, "default"); }
#[test] fn cluster_defaults() { let s = minimal_home(); assert_eq!(s.exposure.mode, ExposureMode::Tailnet); assert_eq!(s.marketplace.refresh, Duration::from_secs(300)); assert_eq!(s.base.access.identity.provider, IdentityProvider::Zitadel); }
#[test] fn no_secret_value_fields() { /* walk every CRD schema; no property named password|token|key|dsn except inside SecretRef */ }
// it_kind_cel.rs, #[ignore], run by operator.yml's kind job:
async fn crd_cel_refuses_public_exposure()        // LoamsCluster exposure.mode "public" → 422
async fn crd_cel_listing_ref_immutable()          // patch spec.listing.id → 422 "listing.id is immutable"
async fn crd_cel_storage_kind_immutable()          // patch StorageBinding spec.kind → 422
async fn crd_cel_object_kind_needs_object_block()  // kind: object without object → 422
async fn crd_cel_one_credential_source()           // both workloadIdentity and secretRef → 422
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p loams-operator-api -j 4`
Expected: FAIL (crate does not exist / unresolved types)

- [ ] **Step 3: Implement the types, `all_crds()` and `crdgen`** in the files above; CEL via `#[schemars(extend("x-kubernetes-validations" = ...))]` (`self == oldSelf` for immutability). Printer columns: `Ready` condition and `summary`. Add `scripts/operator/check-default-build.sh` (the Global Constraints test). Generate CRDs: `cargo run -p loams-operator-api --bin crdgen -j 4`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p loams-operator-api -j 4 && scripts/operator/check-default-build.sh`
Expected: PASS; the script prints `ok: default build excludes operator crates`

- [ ] **Step 5: Commit**

```bash
git add crates/loams-operator-api deploy/helm/loams-operator/crds scripts/operator/check-default-build.sh Cargo.lock
git commit -s -m "operator: loams-operator-api core kinds and crdgen (OP2-A Task 1, D2602, D2603, D2623)"
```

### Task 2: `LoamsDevice` and `LoamsFleet` types

**Files:**
- Create: `crates/loams-operator-api/src/device.rs`
- Modify: `crates/loams-operator-api/src/lib.rs` (`all_crds()`), `deploy/helm/loams-operator/crds/`
- Test: `crates/loams-operator-api/tests/crds.rs`

**Interfaces:**
- Produces: `LoamsDeviceSpec { cluster: String, fleet: Option<String>, identity: DeviceIdentity { zitadel_machine_user: Option<String>, key_fingerprint: String }, class: DeviceClass { Laptop, Gateway, Sensor }, desired: Vec<ListingRef> }`, `LoamsDeviceStatus { last_seen: Option<Time>, reported: Vec<ListingRef>, conditions }`; `LoamsFleetSpec { cluster: String, selector: LabelSelector, desired: Vec<ListingRef> }`. No controller (EDGE1 builds it; D2604).

- [ ] **Step 1: Write the failing tests** — `device_kinds_in_all_crds` (`all_crds()` has `loamsdevices.loams.dev` and `loamsfleets.loams.dev`), `device_key_fingerprint_required` (schema `required` contains `keyFingerprint`), and `crds_match_checked_in` now fails until regenerated.
- [ ] **Step 2: Run** `cargo test -p loams-operator-api -j 4` — Expected: FAIL
- [ ] **Step 3: Implement** the two kinds; regenerate CRDs with `crdgen`.
- [ ] **Step 4: Run** `cargo test -p loams-operator-api -j 4` — Expected: PASS
- [ ] **Step 5: Commit** — `git commit -s -m "operator: LoamsDevice and LoamsFleet types (OP2-A Task 2, D2604)"`

### Task 3: `loams-kube` leader election with self-fence

**Files:**
- Create: `crates/loams-kube/{Cargo.toml,src/lib.rs,src/lease.rs}`
- Test: `crates/loams-kube/tests/lease.rs`, `crates/loams-kube/tests/it_kind_lease.rs`

**Interfaces:**
- Produces:
  - `pub struct LeaseConfig { name: String /* "loams-operator-leader" */, namespace: String, identity: String, lease: Duration /* 15s */, renew_deadline: Duration /* 10s */, retry: Duration /* 2s */ }`
  - `#[async_trait] pub trait LeaseStore { async fn get(&self, name: &str) -> Result<Option<Lease>>; async fn create(&self, l: &Lease) -> Result<Lease>; async fn replace(&self, l: &Lease /* carries resourceVersion */) -> Result<Lease, LeaseError /* Conflict | Api */>; }` with `KubeLeaseStore` and `MemLeaseStore` (tests)
  - `pub trait Clock { fn now(&self) -> Instant; fn wall(&self) -> SystemTime; }` (monotonic for deadlines)
  - `pub async fn run_leader<F>(cfg: LeaseConfig, store: impl LeaseStore, clock: impl Clock, work: impl FnOnce(LeaderToken) -> F) -> LeaderExit` where `LeaderToken { transitions: i32, lost: CancellationToken }`; `LeaderExit { LostLease, Shutdown }` — the binary exits non-zero on `LostLease`

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test(start_paused = true)] async fn single_leader_under_contention() { /* 3 candidates on one MemLeaseStore; at every tick at most one LeaderToken alive */ }
#[tokio::test(start_paused = true)] async fn self_fence_after_missed_renewal() { /* store stalls 11s; token.lost cancelled by renew_deadline 10s on the monotonic clock; exit == LostLease */ }
#[tokio::test(start_paused = true)] async fn lease_transitions_increase() { /* holder A then B: B.transitions == A.transitions + 1 */ }
#[tokio::test(start_paused = true)] async fn expired_lease_taken_only_after_lease_duration() { /* holder silent: B acquires at >= 15s, not before */ }
// it_kind_lease.rs #[ignore]: it_kind_leader_failover — two processes; delete holder; other leads within 15s + 2×2s
```

- [ ] **Step 2: Run** `cargo test -p loams-kube -j 4 --test lease` — Expected: FAIL
- [ ] **Step 3: Implement** client-go's algorithm (acquire if empty or `renewTime + leaseDuration` past; renew every `retry`; give up at `renew_deadline`); every write uses `replace` with the read `resourceVersion`.
- [ ] **Step 4: Run** `cargo test -p loams-kube -j 4 --test lease` — Expected: PASS
- [ ] **Step 5: Commit** — `git commit -s -m "loams-kube: leader election with self-fence (OP2-A Task 3, D1180)"`

### Task 4: Fence token, disruption Lease and the operation journal

**Files:**
- Create: `crates/loams-kube/src/{fence.rs,journal.rs}`
- Test: `crates/loams-kube/tests/{fence.rs,journal.rs}`

**Interfaces:**
- Consumes: `LeaseStore`, `LeaderToken` (Task 3); `OperationStatus`, `StepStatus` (Task 1).
- Produces:
  - `pub struct Fence(i32)`; `impl Fence { fn from_token(t: &LeaderToken) -> Self; fn admits(&self, op: &OperationStatus) -> bool /* op.fence <= self */ }`
  - `pub async fn take_disruption(store: &impl LeaseStore, group: &str /* "loams-<owner>-disruption" */, op_id: &str, item: &str, holder: &str) -> Result<DisruptionGuard, DisruptionError { HeldBy { op_id, item }, Api }>`; same `op_id` takes over a dead holder's Lease; `DisruptionGuard::release()`; expiry 30 min with a `Warning`
  - `pub struct Journal { op: OperationStatus }` with `fn start(kind, plan: Vec<String>, fence: Fence) -> Self`, `fn next(&self) -> Option<&str>`, `fn advance(&mut self, item: &str, phase: &str, fence: Fence, now: Time) -> Result<(), JournalError { StaleFence, NotInPlan, PlanChanged }>`, `fn complete_item(&mut self, item: &str)`, `fn supersede(self) -> OperationStatus`
  - `pub async fn write_status<K>(api: &Api<K>, obj: &K /* carries resourceVersion */, status: serde_json::Value) -> Result<K, WriteError { Conflict, Api }>` (status replace, never merge without resourceVersion)
  - `pub trait PhaseObserver { fn observed_phase(&self, item: &str) -> Option<String>; }` and `Journal::reconcile_with(&mut self, obs: &impl PhaseObserver)` (observation wins)

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test] async fn disruption_lease_is_exclusive() { /* op-a holds; op-b → Err(HeldBy{op_id:"op-a",..}) */ }
#[tokio::test] async fn stale_leader_cannot_take_disruption_lease() { /* two leaders race CAS on one MemLeaseStore; exactly one Ok */ }
#[tokio::test] async fn same_operation_takes_over_dead_holder() { /* op-a holder gone; new leader with op-a gets it; op-b does not */ }
#[test] fn plan_fixed_at_start_and_only_shrinks() { /* advance("x") not in plan → NotInPlan */ }
#[test] fn stale_fence_refused() { /* op.fence 7, Fence(6).advance → StaleFence */ }
#[test] fn journal_resume_each_phase() { /* for each crash point, a new Journal from the stored status + PhaseObserver yields the same next() and never re-runs a done item */ }
#[test] fn phase_derived_from_observation() { /* journal says Applying, observer says Healthy → phase Healthy */ }
#[tokio::test] async fn journal_write_with_old_resource_version_conflicts() { /* mock API returns 409 → WriteError::Conflict */ }
```

- [ ] **Step 2: Run** `cargo test -p loams-kube -j 4 --test fence --test journal` — Expected: FAIL
- [ ] **Step 3: Implement** per the signatures; `write_status` uses `Api::replace_status` with the object's `resourceVersion`.
- [ ] **Step 4: Run** the same command — Expected: PASS
- [ ] **Step 5: Commit** — `git commit -s -m "loams-kube: fences, disruption Lease and the operation journal (OP2-A Task 4, D1181)"`

### Task 5: Conditions, reasons registry, events and metrics

**Files:**
- Create: `crates/loams-kube/src/{conditions.rs,events.rs,metrics.rs}`, `docs/api/operator-reasons.md`
- Test: `crates/loams-kube/tests/{conditions.rs,metrics.rs}`

**Interfaces:**
- Produces:
  - `pub enum CondType { Ready, Progressing, Degraded, Paused, Drift, UpgradeBlocked, AwaitingConfirmation, Sandboxed, Revoked, AccessReady }`
  - `pub fn set_condition(conds: &mut Vec<Condition>, t: CondType, status: bool, reason: Reason, msg: &str, generation: i64, now: Time)` (keeps `lastTransitionTime` when status unchanged)
  - `pub struct Reason(&'static str)` with consts for every reason this plan names (`ReleaseUnknown`, `UpgradeSkipRefused`, `ComponentNotReady`, `IndexUnavailable`, `SignatureInvalid`, `ListingRevoked`, `ListingDigestChanged`, `ListingKindNotSupportedYet`, `PackageNotSupportedYet`, `PackageNotForCluster`, `TierPackageRefused`, `KindNotAllowed`, `ForeignNamespace`, `PublicExposureRefused`, `HookRefused`, `SandboxUnavailable`, `GpuNotSupportedYet`, `ListingRevocationWarning`, `AwaitingConfirmation`, `ClusterScopedMismatch`, `ApplyForbidden`, `DependencyUnresolved`, `DependencyMissing`, `StorageMissing`, `StorageKindNotSupportedYet`, `ConditionalWritesMissing`, `IdentityProviderUnavailable`, `AccessStepFailed`, `AccessCleanupPending`, `DriverNotFound`, `Healthy`, `Applied`, `Paused`, `DriftCorrected`, `Superseded`); `pub fn registry() -> &'static [&'static str]`
  - `pub fn rollup(children: &[(String, &[Condition])]) -> (bool /*ready*/, bool /*degraded*/)`
  - `pub struct Recorder` wrapping `kube::runtime::events::Recorder`: `async fn disruptive(&self, obj, action: &str, op_id: &str, before: bool)`, `async fn warn(&self, obj, reason: Reason, note: &str)`
  - `pub struct Metrics` (prometheus registry) with families of §65 §10.3 prefixed `loams_operator_`; `fn render(&self) -> String`

- [ ] **Step 1: Write the failing tests** — `reason_in_registry` (every `Reason` const appears in `docs/api/operator-reasons.md` and vice versa), `transition_time_kept_when_status_unchanged`, `ready_rollup_from_components` (one `Ready=False` → not ready; one `Degraded=True` → degraded), `metrics_have_no_tenant_labels` (rendered families' label names ⊆ {controller, result, replica, kind, state, group, cluster, component}), `event_note_has_op_id` (before/after events carry `op=<id>`).
- [ ] **Step 2: Run** `cargo test -p loams-kube -j 4 --test conditions --test metrics` — Expected: FAIL
- [ ] **Step 3: Implement**; write `docs/api/operator-reasons.md` as a table (reason, kinds, meaning).
- [ ] **Step 4: Run** the same command — Expected: PASS
- [ ] **Step 5: Commit** — `git commit -s -m "loams-kube: conditions, reasons, events and metrics (OP2-A Task 5, D1191)"`

### Task 6: Server-side apply, drift and pause

**Files:**
- Create: `crates/loams-kube/src/{apply.rs,drift.rs}`
- Test: `crates/loams-kube/tests/{apply.rs,it_kind_apply.rs}`

**Interfaces:**
- Produces:
  - `pub const FIELD_MANAGER: &str = "loams-operator";` and `pub const MANAGED_BY: (&str, &str) = ("loams.dev/managed-by", "loams-operator");`
  - `pub async fn apply_all(client: &Client, objs: &[DynamicObject], owner: &OwnerReference) -> Result<ApplyReport { created: usize, changed: usize, unchanged: usize }, ApplyError { Forbidden(ObjectId), Api }>` (SSA, `force: true`, adds `MANAGED_BY` label and owner ref; skips the PATCH when the stored `loams.dev/applied-hash` equals the object's hash, so a re-apply writes nothing — LOC-REC-001)
  - `pub async fn prune(client: &Client, owner_uid: &str, keep: &[ObjectId]) -> Result<usize>`
  - `pub fn is_paused(meta: &ObjectMeta) -> bool` (`loams.dev/paused: "true"`)
  - `pub fn drifted(desired: &DynamicObject, live: &DynamicObject) -> bool` (compares only fields owned by `FIELD_MANAGER` in `managedFields`)

- [ ] **Step 1: Write the failing tests** — `second_apply_writes_nothing` (hash annotation equal → no PATCH on a mock client), `owned_field_drift_reverted` and `foreign_annotation_kept` (kind, `#[ignore]`: edit replicas → reverted + `DriftCorrected` event; add a foreign annotation → kept), `paused_stops_writes` (paused owner → `apply_all` returns without calls), `prune_removes_only_owned`.
- [ ] **Step 2: Run** `cargo test -p loams-kube -j 4 --test apply` — Expected: FAIL
- [ ] **Step 3: Implement** per the signatures.
- [ ] **Step 4: Run** `cargo test -p loams-kube -j 4 --test apply` — Expected: PASS
- [ ] **Step 5: Commit** — `git commit -s -m "loams-kube: server-side apply, drift and pause (OP2-A Task 6)"`

### Task 7: Operator binary, manager and the Helm chart

**Files:**
- Create: `crates/loams-operator/{Cargo.toml,src/main.rs,src/manager.rs}`
- Create: `deploy/helm/loams-operator/{Chart.yaml,values.yaml,values.schema.json,templates/{deployment,serviceaccount,clusterrole,clusterrolebinding,role,rolebinding,pdb,networkpolicy,vap-namespaces,vap-public-service,home,_helpers.tpl}.yaml,tests/}`
- Test: `crates/loams-operator/tests/chart.rs`

**Interfaces:**
- Consumes: `run_leader`, `LeaderToken` (Task 3); `Metrics` (Task 5).
- Produces:
  - Binary `loams-operator` with subcommands `run` (default) and `probe-object` (Task 13); flags `--namespace loams-system`, `--metrics-addr 0.0.0.0:9090`, `--health-addr 0.0.0.0:8081`
  - `pub async fn run_controllers(client: Client, token: LeaderToken, ctx: Arc<Ctx>) -> anyhow::Result<()>` — starts one `kube::runtime::Controller` per kind (registered by later tasks through `pub fn controllers() -> Vec<ControllerSpec>`); cancels all on `token.lost`
  - `pub struct Ctx { client: Client, recorder: Recorder, metrics: Metrics, source: Arc<dyn ListingSource>, pins: Pins, fence: Fence }` (fields filled by later tasks)
  - Chart values exactly as "Interfaces for sibling plans"; `templates/home.yaml` renders `LoamsCluster home` from `home.spec` when `home.create`

- [ ] **Step 1: Write the failing tests** (`tests/chart.rs` runs `helm template` on the chart)

```rust
#[test] fn chart_refuses_other_namespace() { /* helm template -n default → error "loams-operator must be installed in loams-system" */ }
#[test] fn chart_requires_image_digest() { /* no image.digest → error */ }
#[test] fn marketplace_values_flow_to_home_and_mount() { /* marketplace.configMap=idx → volume at /etc/loams/marketplace read-only; home.create → home.spec.marketplace.index/publicKey from values */ }
#[test] fn home_rendered_only_when_create() { /* home.create=false → no LoamsCluster; true → kind LoamsCluster, name home, spec == home.spec */ }
#[test] fn rbac_has_no_exec() { /* no rule names pods/exec or pods/attach */ }
#[test] fn rbac_least_privilege() { /* no "*" verb/resource; no escalate|bind|impersonate; secrets verbs ⊆ {get,create,update,patch,delete} */ }
#[test] fn vap_limits_namespaces() { /* vap-namespaces CEL allows loams-system and loams-i-* only, for the operator SA */ }
#[test] fn vap_refuses_public_service() { /* vap-public-service refuses NodePort and LoadBalancer without an internal annotation in loams-* namespaces */ }
#[test] fn leader_lease_rbac_by_name() { /* leases: get/update on resourceNames [loams-operator-leader]; create unrestricted (RBAC cannot name on create) */ }
#[test] fn deployment_hardened() { /* replicas from values (default 2), anti-affinity, runAsNonRoot, readOnlyRootFilesystem, drop ALL, RuntimeDefault, image "<repo>@<digest>" */ }
```

- [ ] **Step 2: Run** `cargo test -p loams-operator -j 4 --test chart` — Expected: FAIL
- [ ] **Step 3: Implement** the binary, manager and chart (R3 for the ClusterRole; PDB `minAvailable: 1`; `kubeVersion: ">=1.30.0-0"`; `annotations.licenses: Apache-2.0`; README marks it preview).
- [ ] **Step 4: Run** `cargo test -p loams-operator -j 4 --test chart && helm lint deploy/helm/loams-operator --set image.digest=sha256:$(printf 0%.0s {1..64})` — Expected: PASS, `1 chart(s) linted, 0 chart(s) failed`
- [ ] **Step 5: Commit** — `git commit -s -m "operator: binary, leader-gated manager and Helm chart (OP2-A Task 7, D1180, D1193)"`

### Task 8: Client-side Helm renderer and manifest policy

**Files:**
- Create: `crates/loams-operator/src/render/{mod.rs,helm.rs,policy.rs}`
- Test: `crates/loams-operator/tests/{render_helm.rs,policy.rs}`, fixtures `crates/loams-operator/tests/fixtures/charts/{hello,hooks,foreign-ns,crds,nodeport}/`

**Interfaces:**
- Produces:
  - `#[async_trait] pub trait ChartFetcher { async fn fetch(&self, chart: &str, version: &str) -> Result<Vec<u8>, RenderError>; }` — `HelmCliFetcher` (`oci://` charts pulled as `<chart>@<digest>`, so the registry manifest digest is checked; `https://…/.tgz` downloaded and its sha256 compared), `DirFetcher` (tests)
  - `pub async fn render_helm(f: &dyn ChartFetcher, chart: &str, version: &str, digest: &str, release: &str, namespace: &str, values: &serde_json::Value, kube_version: &str, hooks: HookMode { Drop, OrderedJobs }) -> Result<Rendered, RenderError { DigestMismatch, HelmFailed(String), Parse }>`; `Rendered { namespaced: Vec<DynamicObject>, cluster_scoped: Vec<DynamicObject>, hook_jobs: Vec<DynamicObject> }`
  - `pub fn check(r: &Rendered, tier: Tier, namespace: &str) -> Result<(), PolicyError { KindNotAllowed(ObjectId), ForeignNamespace(ObjectId), PublicExposure(ObjectId), HookRefused(ObjectId), ClusterScopedNotAllowed(ObjectId) }>`
  - `pub const NAMESPACED_ALLOW: &[(&str, &str)]` = Deployment, StatefulSet, DaemonSet, Job, CronJob, Service, ConfigMap, Secret, ServiceAccount, Role, RoleBinding, PersistentVolumeClaim, PodDisruptionBudget, NetworkPolicy, HorizontalPodAutoscaler; `CLUSTER_ALLOW_CONFIRMED` = CustomResourceDefinition, ClusterRole, ClusterRoleBinding (+ webhook configurations for `FirstParty` only, R5); everything else refused (Namespace, Ingress, Gateway API kinds, APIService, PodSecurityPolicy, ValidatingAdmissionPolicy*)
  - `pub fn cluster_scoped_digest(objs: &[DynamicObject]) -> String` (`sha256:` over the sorted canonical JSON)

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test] async fn digest_mismatch_refused() { assert!(matches!(render(hello, wrong_digest), Err(RenderError::DigestMismatch))); }
#[tokio::test] async fn hooks_dropped_for_listings() { let r = render(hooks, HookMode::Drop).unwrap(); assert!(r.hook_jobs.is_empty()); assert!(all_objs(&r).iter().all(|o| !o.annotations().contains_key("helm.sh/hook"))); }
#[tokio::test] async fn ordered_hook_jobs_kept_for_base() { let r = render(hooks, HookMode::OrderedJobs).unwrap(); assert_eq!(names(&r.hook_jobs), ["init", "setup"]); /* by hook weight */ }
#[test] fn foreign_namespace_object_refused() { /* object with metadata.namespace kube-system → ForeignNamespace */ }
#[test] fn community_refuses_helm_output_kinds_beyond_oci() { /* check(.., Tier::Community, ..) on a chart with a Role → KindNotAllowed */ }
#[test] fn crds_split_to_cluster_scoped() { let r = render(crds); assert_eq!(r.cluster_scoped.len(), 1); }
#[test] fn verified_refuses_webhooks() { /* ValidatingWebhookConfiguration, Tier::Verified → ClusterScopedNotAllowed */ }
#[test] fn no_public_service_rendered() { /* nodeport fixture and LoadBalancer without internal annotation → PublicExposure; Ingress → KindNotAllowed */ }
#[test] fn cluster_scoped_digest_is_order_independent() { /* shuffled input → same digest */ }
```

- [ ] **Step 2: Run** `cargo test -p loams-operator -j 4 --test render_helm --test policy` — Expected: FAIL
- [ ] **Step 3: Implement** — `helm template <release> <tgz> --namespace <ns> --no-hooks|(hooks kept) --include-crds --kube-version <v> -f <values.json>`, run with `HELM_CACHE_HOME`, `HELM_CONFIG_HOME`, `HELM_DATA_HOME` under the Pod's `emptyDir`; the internal-LB annotation list is a `const` (AWS, GCP, Azure, and Tailscale's `tailscale.com/expose`).
- [ ] **Step 4: Run** the same command (needs `helm` on PATH; CI installs the pinned version) — Expected: PASS
- [ ] **Step 5: Commit** — `git commit -s -m "operator: client-side Helm rendering and the manifest policy (OP2-A Task 8, D2626)"`

### Task 9: Release pins and the platform-base renderers

**Files:**
- Create: `crates/loams-operator/pins/releases.toml`, `crates/loams-operator/src/pins.rs`, `crates/loams-operator/src/render/base.rs`
- Test: `crates/loams-operator/tests/{pins.rs,render_base.rs}`, goldens `crates/loams-operator/tests/goldens/base/*.yaml`

**Interfaces:**
- Consumes: `render_helm`, `HookMode::OrderedJobs` (Task 8); `PlatformBase` (Task 1).
- Produces:
  - `pins/releases.toml`: per Loams release `[release."<x.y.z>"]` with `gateway`, `agentd`, `loams` (meter), `identity_sync`, optional `loams_identity` image digests; `[..zitadel] chart, version, digest, hooks`; `[..openfga] chart, version, digest`; `[..tidb_operator] chart, version, digest, crd_sha`; `[..tikv] pd, tikv` image digests; `previous = "<x.y.z>"`
  - `pub struct Pins` with `fn load() -> Pins` (`include_str!`), `fn release(&self, v: &str) -> Result<&Release, PinError { Unknown, Skip { from, to } }>`, `fn supports_upgrade(&self, from: &str, to: &str) -> bool` (N−1 → N only)
  - `pub enum BaseComponent { Identity, Permissions, Meter, Gateway, Agentd }` with `pub const BASE_ORDER: [BaseComponent; 5] = [Identity, Permissions, Meter, Gateway, Agentd]` (the first three are Loams Access; `LoamsCluster` reports them together as `AccessReady`)
  - `pub async fn render_component(c: BaseComponent, home: &LoamsCluster, rel: &Release, f: &dyn ChartFetcher) -> Result<Rendered, RenderError>` — Identity = for `Zitadel`, the Zitadel chart (DSN and a generated masterkey Secret by reference) + the `loams-identity-sync` Deployment; for `LoamsIdentity`, a `loams-identity` Deployment from the pinned digest (R8); Permissions = the OpenFGA chart (values from `deploy/openfga/helm/values.yaml`) and a model-write Job for `deploy/openfga/core.fga`; Gateway = Deployment ×`replicas`, ConfigMap, PDB `minAvailable: 1`, Service (`ClusterIP`, or the tailnet annotation for `Tailnet`); Agentd = Deployment + Service; Meter = `loams` image in role `meter` (R12); all in `loams-system`, restricted Pod Security, `automountServiceAccountToken: false`

- [ ] **Step 1: Write the failing tests** — `release_resolves_to_digests` (every image is `@sha256:`), `upgrade_skip_refused` (`0.1→0.3` false), `pins_reference_no_fork_images` (Global Constraints), `base_goldens_match` (each component on a fixture home equals its golden), `no_public_service_rendered_base` (all components × both exposure modes), `restart_safe_rendering` (rendering twice from the same inputs is byte-identical, LOC-REC-002).
- [ ] **Step 2: Run** `cargo test -p loams-operator -j 4 --test pins --test render_base` — Expected: FAIL
- [ ] **Step 3: Implement** per the signatures with Task 0's pins; `UPDATE_GOLDENS=1` regenerates goldens.
- [ ] **Step 4: Run** the same command — Expected: PASS
- [ ] **Step 5: Commit** — `git commit -s -m "operator: release pins and platform-base rendering (OP2-A Task 9, D2623, D2630)"`

### Task 10: The `LoamsCluster` (home) controller

**Files:**
- Create: `crates/loams-operator/src/cluster.rs`
- Create: `deploy/operator/home.yaml`
- Test: `crates/loams-operator/tests/{cluster.rs,it_kind_home.rs}`

**Interfaces:**
- Consumes: `Journal`, `Fence`, `write_status` (Task 4); `set_condition`, `rollup`, `Recorder` (Task 5); `apply_all`, `prune`, `is_paused` (Task 6); `Pins`, `BASE_ORDER`, `render_component` (Task 9).
- Produces: `pub async fn reconcile_cluster(obj: Arc<LoamsCluster>, ctx: Arc<Ctx>) -> Result<Action, ReconcileError>`; operation kinds `Rollout` (plan = `BASE_ORDER` names) and `Upgrade { from, to }`; `pub fn sandbox_runtime_class(home: &LoamsCluster, classes: &[RuntimeClass]) -> Option<String>` (R6, used by Task 11); finalizer `loams.dev/cluster` with the escape annotation `loams.dev/force-delete: "true"` (LOC-DEL-003).

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test] async fn order_waits_for_ready() { /* identity not Ready → permissions not applied; status.operation.step.item == "identity" */ }
#[tokio::test] async fn access_ready_rolls_up_three_parts() { /* identity+permissions Ready, meter not → AccessReady=False, Ready=False */ }
#[tokio::test] async fn loams_identity_without_pin_unavailable() { /* provider loams-identity, no loams_identity pin → AccessReady=False IdentityProviderUnavailable, nothing applied */ }
#[tokio::test] async fn failed_step_blocks_without_rollback() { /* gateway apply fails → UpgradeBlocked=True, reason ComponentNotReady, identity untouched */ }
#[tokio::test] async fn release_unknown_refused() { /* release "9.9.9" → Ready=False ReleaseUnknown, nothing applied */ }
#[tokio::test] async fn upgrade_skip_refused_before_any_change() { /* N-2 → N → UpgradeBlocked UpgradeSkipRefused, zero writes */ }
#[tokio::test] async fn changed_spec_supersedes_operation() { /* release changed mid-rollout → op ends Superseded, new op plan from observation */ }
#[tokio::test] async fn only_role_home_accepted() { /* second LoamsCluster in the namespace → Ready=False "one home per operator" */ }
#[test] fn gvisor_detected_by_handler() { /* handler runsc → Some(name); handler gvisor → Some; none → None; spec override wins */ }
// it_kind_home.rs #[ignore]: it_kind_home_ready — install chart + deploy/operator/home.yaml (fixture Postgres for identity) → Ready within 15 min; observedGeneration == generation
```

- [ ] **Step 2: Run** `cargo test -p loams-operator -j 4 --test cluster` — Expected: FAIL
- [ ] **Step 3: Implement** observe → decide → act once → report (§65 §6.2): at most one component applied per reconcile; ordered hook Jobs run first and are awaited; status written with `write_status`; `Ready` = rollup of the five components.
- [ ] **Step 4: Run** `cargo test -p loams-operator -j 4 --test cluster` — Expected: PASS
- [ ] **Step 5: Commit** — `git commit -s -m "operator: the home LoamsCluster controller (OP2-A Task 10, D2623, D1186)"`

### Task 11: Listing source, revocation and tier admission

**Files:**
- Create: `crates/loams-operator/src/{listing.rs,admit.rs}`
- Test: `crates/loams-operator/tests/{listing.rs,admit.rs}`

**Interfaces:**
- Consumes (OP2-B): `IndexFiles`, `TrustAnchor`, `SignedIndex::verify`, `verify_listing`, `VerifiedListing`, `VerifyError`, `RevocationList`, `is_revoked`, `ListingV2`, `ListingKind`, `Package`, `Tier`, `HttpsFetcher`; dev: `testkit::signed_v2_index`.
- Produces:
  - `#[async_trait] pub trait ListingSource: Send + Sync { async fn verified(&self, id: &str, v: &semver::Version) -> Result<VerifiedListing, SourceError>; async fn versions(&self, id: &str) -> Result<Vec<semver::Version>, SourceError>; async fn revoked(&self, id: &str, v: &semver::Version) -> bool; async fn dependencies(&self, root: &VerifiedListing) -> Result<Vec<VerifiedListing>, SourceError> /* OP2-B's resolve_depends_on */; }`; `SourceError { Unavailable, Verify(VerifyError), NotFound }`
  - `pub struct IndexSource` with `fn new(index: Url, anchor: TrustAnchor, refresh: Duration, fetcher: Arc<dyn Fetcher>)` and `fn anchor_from_spec(public_key: Option<&str>) -> Result<TrustAnchor, SourceError>`; refresh loop keeps the last good `SignedIndex`; `pub fn index_files_urls(index: &Url) -> [Url; 4]`; `pub struct FixtureSource` (controller unit tests)
  - `pub fn admit(l: &VerifiedListing, cluster_has_gvisor: bool, spec: &LoamsInstallSpec) -> Result<Admission { tier: Tier, runtime_class: Option<String>, sandboxed: bool }, AdmitError>` with reasons `ListingKindNotSupportedYet`, `PackageNotSupportedYet`, `PackageNotForCluster`, `TierPackageRefused` (Community + helm), `SandboxUnavailable`, `GpuNotSupportedYet`

- [ ] **Step 1: Write the failing tests** (signing through OP2-B's `testkit` feature as a dev-dependency)

```rust
#[tokio::test] async fn unsigned_index_refused() { assert!(matches!(src.verified("acme/hello", &v1).await, Err(SourceError::Verify(_)))); }
#[tokio::test] async fn public_key_anchor_verifies_testkit_index() { /* signed_v2_index → PublicKey anchor → verified() Ok; another key → Verify */ }
#[tokio::test] async fn file_index_only_under_marketplace_dir() { /* file:///etc/loams/marketplace/index.v2.json ok; file:///etc/passwd → Err(SourceError::Unavailable) with message "index path outside /etc/loams/marketplace" */ }
#[test] fn index_file_urls_are_v2() { /* …/index.v2.json → [index.v2.json, index.v2.json.sigstore.json, revocations.json, revocations.json.sigstore.json] */ }
#[tokio::test] async fn warn_revocation_keeps_running() { /* warn → verified() Ok with warning, revoked() false; disable → revoked() true */ }
#[tokio::test] async fn revoked_version_reported_within_one_refresh() { /* revoke; advance time by refresh; revoked() == true */ }
#[tokio::test] async fn index_outage_keeps_running_installs() { /* fetch fails after one good load → verified() still Ok from cache for known versions, revoked() uses last list; first-ever load failing → Unavailable */ }
#[test] fn community_helm_refused() { assert_eq!(admit(community_helm, true, &s).unwrap_err().reason(), "TierPackageRefused"); }
#[test] fn community_without_gvisor_refused_unless_override() { /* false,false → SandboxUnavailable; override → Ok{sandboxed:false} */ }
#[test] fn verified_helm_admitted_without_gvisor() { /* Ok{runtime_class: None} */ }
#[test] fn agent_kinds_refused_in_op2_0() { /* Agent, Harness, McpServer → ListingKindNotSupportedYet */ }
#[test] fn wasm_and_binary_refused_for_cluster() { /* PackageNotForCluster */ }
#[test] fn kustomize_refused_in_op2_0() { /* PackageNotSupportedYet */ }
```

- [ ] **Step 2: Run** `cargo test -p loams-operator -j 4 --test listing --test admit` — Expected: FAIL
- [ ] **Step 3: Implement** per the signatures (R5, R6, R10).
- [ ] **Step 4: Run** the same command — Expected: PASS
- [ ] **Step 5: Commit** — `git commit -s -m "operator: verified listing source, revocations and tier admission (OP2-A Task 11, D2608, D2609, D2626)"`

### Task 12: OCI package renderer

**Files:**
- Create: `crates/loams-operator/src/render/oci.rs`
- Test: `crates/loams-operator/tests/render_oci.rs`, goldens `tests/goldens/oci/*.yaml`

**Interfaces:**
- Consumes: `Admission` (Task 11); `Package::Oci { image, digest }`.
- Produces: `pub fn render_oci(install: &LoamsInstall, l: &ListingV2, adm: &Admission, ns: &str, env: &[EnvVar], sa_annotations: &BTreeMap<String, String>) -> Result<Rendered, RenderError>` — ServiceAccount `install`, Deployment (replicas from `values.replicas`, default 1; ports, probe and args per R7; image `<image>@<digest>`; `runtimeClassName` from `adm`), Service `ClusterIP` with one port per `run.ports` entry (else 80→8080) (tailnet annotation for `Tailnet`), default-deny NetworkPolicy plus ingress from `loams-system` gateway pods; pod labels of the Meter interface.

- [ ] **Step 1: Write the failing tests** — `oci_goldens_match`, `run_ports_and_health_used` (`run.ports` [9000 tcp], `health {/ready, 9000}` → container port 9000, readiness `/ready`; no `run` → 8080 and `/healthz`), `image_pinned_by_digest` (container image ends `@<digest>` and never uses a tag), `runtime_class_set_when_sandboxed`, `restricted_pod_security` (non-root, read-only root, drop ALL, RuntimeDefault, no hostNetwork/hostPID/privileged), `meter_labels_present`, `no_public_service_rendered_oci` (every exposure mode).
- [ ] **Step 2: Run** `cargo test -p loams-operator -j 4 --test render_oci` — Expected: FAIL
- [ ] **Step 3: Implement** per the signature.
- [ ] **Step 4: Run** the same command — Expected: PASS
- [ ] **Step 5: Commit** — `git commit -s -m "operator: OCI package rendering (OP2-A Task 12, D2602)"`

### Task 13: `StorageBinding` controller, `object` kind and the probe

**Files:**
- Create: `crates/loams-operator/src/storage/{mod.rs,probe.rs}`
- Test: `crates/loams-operator/tests/{storage.rs,probe.rs,it_kind_storage.rs}`

**Interfaces:**
- Consumes: `StorageBindingSpec` (Task 1); `apply_all`, conditions (Tasks 5–6).
- Produces:
  - `pub async fn reconcile_binding(obj: Arc<StorageBinding>, ctx: Arc<Ctx>) -> Result<Action, ReconcileError>` — non-`object` kinds: `Ready=False StorageKindNotSupportedYet`; `object`: ServiceAccount `storage-<binding>` with workload-identity annotations (`eks.amazonaws.com/role-arn`, `iam.gke.io/gcp-service-account`, `azure.workload.identity/client-id` plus pod label `azure.workload.identity/use: "true"`), then a probe Job running `loams-operator probe-object`; reads the result from the Pod's termination message into `status.capabilities`
  - `pub async fn probe(store: &dyn object_store::ObjectStore) -> ObjectCapabilities` (conditional put via `PutMode::Create` twice → second fails with `AlreadyExists`; range read; multipart of 2 parts; deletes its own `.loams-probe/<uuid>` objects)
  - `pub fn binding_env(name: &str, b: &StorageBinding) -> Vec<EnvVar>` → `LOAMS_STORAGE_<NAME>_URL` (`s3://bucket/prefix`, `gs://`, `az://`), `_REGION`, `_ENDPOINT`; secret-backed credentials as `valueFrom.secretKeyRef` to a copy `storage-<binding>` in the target namespace (keys `access_key_id`, `secret_access_key` or provider equivalents)
  - `pub async fn copy_secret(client: &Client, from: (&str, &str), to_ns: &str, name: &str) -> Result<()>` (get by name, create/update; never logs data)

- [ ] **Step 1: Write the failing tests** — `probe_refuses_no_conditional_put` (an `ObjectStore` wrapper that ignores `PutMode::Create` → `Ready=False ConditionalWritesMissing`), `probe_reports_capabilities` (in-memory store → all true), `workload_identity_annotations_per_provider`, `binding_env_urls` (`s3://b/p`, `gs://b`, `az://b`), `non_object_kinds_not_supported_yet`, `secret_copy_never_logged` (capture `tracing` output; no secret bytes); kind `#[ignore]` `it_kind_binding_probe_rustfs` (official RustFS image + secretRef → `Ready=True`, capabilities all true).
- [ ] **Step 2: Run** `cargo test -p loams-operator -j 4 --test storage --test probe` — Expected: FAIL
- [ ] **Step 3: Implement** per the signatures; the probe subcommand builds the store with `object_store`'s `AmazonS3Builder`/`GoogleCloudStorageBuilder`/`MicrosoftAzureBuilder` from env (workload identity is picked up by those builders' default credential chains).
- [ ] **Step 4: Run** the same command — Expected: PASS
- [ ] **Step 5: Commit** — `git commit -s -m "operator: StorageBinding object kind and capability probe (OP2-A Task 13, D2603, D1190)"`

### Task 14: The `LoamsInstall` controller

**Files:**
- Create: `crates/loams-operator/src/install/{mod.rs,revoke.rs}`, `crates/loams-operator/src/access/mod.rs` (trait and types only), `deploy/operator/install.example.yaml`
- Test: `crates/loams-operator/tests/{install.rs,it_kind_install.rs}`, fixtures `tests/fixtures/listings/{hello-oci,hello-helm,crd-helm}.json`

**Interfaces:**
- Consumes: `ListingSource`, `admit` (Task 11); `render_helm`, `check`, `cluster_scoped_digest` (Task 8); `render_oci` (Task 12); `binding_env`, `copy_secret` (Task 13); `Journal`, `take_disruption` (Task 4); `apply_all`, `prune` (Task 6); `DriverRegistry` (Task 17, `None` until then).
- Produces:
  - `pub async fn reconcile_install(obj: Arc<LoamsInstall>, ctx: Arc<Ctx>) -> Result<Action, ReconcileError>`
  - `pub fn install_namespace(name: &str) -> String` (the interface rule)
  - Operation kinds `Install` and `Upgrade`, plan = `["dependencies", "verify", "access.identity", "access.permissions", "access.meter", "render", "confirm", "apply", "healthy"]` (the `access.*` steps call `ctx.access.step(..)`); `#[async_trait] pub trait AccessSteps: Send + Sync { async fn step(&self, step: &str, s: &AccessSubject, st: &mut AccessStatus, op_id: &str) -> Result<(), AccessError>; async fn deprovision(&self, s: &AccessSubject, st: &mut AccessStatus, op_id: &str) -> Result<(), AccessError>; }` with `NoAccess` (no-op) until Task 15; `AccessSubject` and `AccessError` are defined here in `src/access/mod.rs` and filled out by Task 15; any `apply` that changes a pod template takes `take_disruption("loams-<install>-disruption", ..)`
  - Namespace labels: `loams.dev/managed-by=loams-operator`, `loams.dev/install=<name>`, `pod-security.kubernetes.io/enforce=restricted`
  - Health: Deployments/StatefulSets/DaemonSets available, Jobs complete (or the driver's `health`)
  - Finalizer `loams.dev/install`: run `ctx.access.deprovision(..)` (Task 15) first, then prune namespaced objects and delete the namespace; cluster-scoped objects are left and listed in a `Warning` (the user applied them, R2); `loams.dev/force-delete: "true"` escape
  - `revoke::enforce(install, ctx)`: R10

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test] async fn oci_install_reaches_ready() { /* FixtureSource hello-oci → namespace loams-i-hello, Deployment applied, Ready after health */ }
#[tokio::test] async fn signature_checked_before_anything_runs() { /* Verify error → zero objects created, Ready=False SignatureInvalid */ }
#[tokio::test] async fn cluster_scoped_waits_for_confirmation() { /* crd-helm: AwaitingConfirmation=True, status.clusterScoped.digest set, ConfigMap cluster-scoped has manifest.yaml, no namespaced object applied */ }
#[tokio::test] async fn confirmation_requires_matching_objects() { /* approvedClusterScoped=digest but CRD absent → ClusterScopedMismatch; CRD present with rendered hash → apply proceeds */ }
#[tokio::test] async fn approval_digest_changes_with_values() { /* approve, then change values that alter the CRD → AwaitingConfirmation=True again, nothing new applied */ }
#[tokio::test] async fn listing_digest_change_for_same_version_refused() { /* status.listing.digest X, source now says Y for same version → ListingDigestChanged, running objects untouched */ }
#[tokio::test] async fn storage_binding_wired() { /* storage [{name: data, binding: b}] → env LOAMS_STORAGE_DATA_URL; SA install has binding's WI annotations; missing binding → StorageMissing */ }
#[tokio::test] async fn required_storage_kind_enforced() { /* requires.storage [StorageKind::Object], no binding → StorageMissing; [StorageKind::Kafka] → StorageKindNotSupportedYet; binding kind must equal the required StorageKind */ }
#[tokio::test] async fn revoked_install_scaled_to_zero() { /* revoke → Revoked=True, Deployment replicas 0, PVCs kept */ }
#[tokio::test] async fn upgrade_takes_disruption_lease() { /* version bump with pod-template change → Lease loams-<name>-disruption held during apply, released after healthy */ }
#[test] fn install_namespace_name_fits_63() { assert!(install_namespace(&"x".repeat(80)).len() <= 63); assert_eq!(install_namespace("hello"), "loams-i-hello"); }
// it_kind_install.rs #[ignore]: it_kind_oci_install_and_delete (Ready, then delete → namespace gone, nothing else removed — LOC-DEL-001);
//                               it_kind_helm_install_confirmed (kubectl-apply the cluster-scoped manifest as the test user, set the digest → Ready)
```

- [ ] **Step 2: Run** `cargo test -p loams-operator -j 4 --test install` — Expected: FAIL
- [ ] **Step 3: Implement** observe → decide → act once → report; `status.listing.digest` is `VerifiedListing.listing_sha256` (plus the package digest), recorded at first verify and compared thereafter.
- [ ] **Step 4: Run** `cargo test -p loams-operator -j 4 --test install` — Expected: PASS
- [ ] **Step 5: Commit** — `git commit -s -m "operator: LoamsInstall for oci and helm packages (OP2-A Task 14, D2602, D2626, D2609)"`

### Task 15: Loams Access provisioning for installs

**Files:**
- Create: `crates/loams-operator/src/access/{mod.rs,identity.rs,permissions.rs,meter.rs,audit.rs}`
- Modify: `deploy/openfga/core.fga` (type `install`, R16), `crates/loams-operator/src/install/mod.rs` (wire `AccessProvisioner` into `Ctx`)
- Test: `crates/loams-operator/tests/{access.rs,it_kind_access.rs}`, the model test beside `deploy/openfga/core.fga.yaml`

**Interfaces:**
- Consumes: `Journal`, `write_status` (Task 4); `Recorder` (Task 5); `reconcile_install`'s `access.*` steps and finalizer (Task 14); `AccessStatus`, `Access` (Task 1); `loams_zitadel` client, `loams_authz::OpenFgaAuthorizer::write` and `Tuple`, `loams_cloudevents` (workspace crates).
- Produces:
  - `pub struct AccessSubject { kind: SubjectKind { Install, Device }, org: String, project: String, uid: String }` with `fn id(&self) -> String` (`org/<org>/project/<project>/install/<uid>`), `fn username(&self) -> String` (`install-<uid>`), `fn fga_object(&self) -> String` (`install:<uid>`)
  - `#[async_trait] pub trait IdentityPort { async fn find(&self, username: &str) -> Result<Option<String>>; async fn create(&self, username: &str) -> Result<String>; async fn delete(&self, id: &str) -> Result<()>; }` — `ZitadelIdentity` (OP2-0), `LoamsIdentity` (when R8's pin exists)
  - `#[async_trait] pub trait PermissionPort { async fn has(&self, t: &Tuple) -> Result<bool>; async fn write(&self, add: &[Tuple], del: &[Tuple]) -> Result<()>; }` — `OpenFgaPermissions`
  - `#[async_trait] pub trait MeterPort { async fn find(&self, subject: &str) -> Result<bool>; async fn create(&self, subject: &str, project: &str) -> Result<()> /* entitlements = the project's budgets */; async fn delete(&self, subject: &str) -> Result<()>; }` — `MeterApi`
  - `#[async_trait] pub trait AuditSink { async fn send(&self, ev: &CloudEvent) -> Result<()>; }` — `HttpSink` (binary mode), `LogSink`
  - `pub struct AccessProvisioner { identity, permissions, meter, audit }` implementing Task 14's `AccessSteps` (`step` takes `access.identity | access.permissions | access.meter`); each step is read-first (find/has), records its result in `AccessStatus` before the next step, then sends its event with id `<op_id>/<step>`
  - `pub fn event(kind: AccessEvent, s: &AccessSubject, op_id: &str, step: &str) -> CloudEvent` (types per the Interfaces table)

- [ ] **Step 1: Write the failing tests** (fakes for the four ports with injectable failures and call logs)

```rust
#[tokio::test] async fn workloads_wait_for_access() { /* identity port failing → no Deployment applied; step.item == "access.identity" */ }
#[tokio::test] async fn provision_creates_all_three_in_order() { /* steps run → identity created, 2 tuples written, meter subject created; status.access all set; AccessReady=True; 3 events */ }
#[tokio::test] async fn fga_failure_after_identity_resumes_without_orphan() { /* permissions.write fails once → AccessReady=False AccessStepFailed, step stays access.permissions; next reconcile: identity.create not called again (find hits), tuples written, meter created; exactly one identity exists */ }
#[tokio::test] async fn crash_after_remote_create_before_status_write_no_duplicate() { /* identity created, status write lost → resume: find("install-<uid>") reuses it; identity.create calls == 1 */ }
#[tokio::test] async fn meter_failure_leaves_no_orphan_on_delete() { /* meter.create fails, then install deleted → deprovision removes tuples and identity; meter.delete not required; no port holds a record */ }
#[tokio::test] async fn deprovision_reverse_order_and_idempotent() { /* order meter, permissions, identity; second deprovision makes no remote write */ }
#[tokio::test] async fn deprovision_failure_holds_finalizer() { /* identity.delete fails → finalizer kept, AccessCleanupPending; retry succeeds → namespace deleted */ }
#[tokio::test] async fn audit_event_ids_deterministic_and_resent() { /* sink fails then succeeds → same event id sent twice, never two ids for one step */ }
#[test] fn events_have_access_types() { /* types ∈ io.loams.dev.access.{identity.created,identity.deleted,permission.granted,permission.revoked,meter.subject.created,meter.subject.deleted}; subject == AccessSubject::id */ }
#[test] fn no_secret_in_events() { /* event data has ids only; no key or token fields */ }
// model test (fga CLI): project viewer can view install:<uid>; install's service_account is operator of install:<uid> only, not of the project
// it_kind_access.rs #[ignore]: it_kind_install_access_lifecycle — on the kind home: install fixture → Zitadel user install-<uid> exists, OpenFGA check passes, Meter subject exists; delete → all three gone
```

- [ ] **Step 2: Run** `cargo test -p loams-operator -j 4 --test access` — Expected: FAIL
- [ ] **Step 3: Implement** per the signatures (R16, R17); add type `install` to `deploy/openfga/core.fga` and its model test cases.
- [ ] **Step 4: Run** `cargo test -p loams-operator -j 4 --test access` and the model test (`fga model test --tests deploy/openfga/core.fga.yaml`) — Expected: PASS
- [ ] **Step 5: Commit** — `git commit -s -m "operator: Loams Access provisioning for installs (OP2-A Task 15, D2632, D2633)"`

### Task 16: `depends_on` ordering

**Files:**
- Create: `crates/loams-operator/src/install/deps.rs`
- Test: `crates/loams-operator/tests/deps.rs`

**Interfaces:**
- Consumes: OP2-B's `resolve_depends_on(index, root) -> Result<Vec<VerifiedListing>, VerifyError>` (through `ListingSource::dependencies`, Task 11), `DependsOn`; `reconcile_install`'s `dependencies` step (Task 14).
- Produces:
  - `pub fn place(resolved: &[VerifiedListing], ranges: &[DependsOn], existing: &[LoamsInstall]) -> Vec<DepAction>`; `DepAction { Reuse { install: String }, Create { name: String /* dep-<ns>-<name> */, id, version } }`, keeping `resolved`'s order (dependencies first); an existing install is reused when its version satisfies every range that names its id
  - `pub const PLATFORM_BASE_IDS: &[&str]` (`loams/gateway`, `loams/access`, `loams/identity`, `loams/openfga`, `loams/meter`, `loams/agentd`, `loams/operator`) — implicit, skipped (D2624)

- [ ] **Step 1: Write the failing tests** — `order_kept_from_resolver` (resolved [c, b] → actions [c, b]), `existing_install_reused` (an install of `b` at 1.3 satisfies `^1`), `existing_install_outside_range_not_reused`, `resolver_error_maps_to_reason` (`VerifyError::Dependency` → `Ready=False DependencyUnresolved`, nothing created), `platform_base_ids_skipped`, `dependent_waits_until_dependency_ready` (controller test: `step.item == "dependencies"` while `b` is not Ready), `dependency_kept_while_dependent_exists` (Review Focus 5: two dependents own `dep-x-b`; delete one → `dep-x-b` stays; user-created dependency deleted → dependent `Degraded` `DependencyMissing`).
- [ ] **Step 2: Run** `cargo test -p loams-operator -j 4 --test deps` — Expected: FAIL
- [ ] **Step 3: Implement** per R9.
- [ ] **Step 4: Run** `cargo test -p loams-operator -j 4 --test deps` — Expected: PASS
- [ ] **Step 5: Commit** — `git commit -s -m "operator: depends_on resolution and ordering (OP2-A Task 16, D2624)"`

### Task 17: TiKV engine listing through the tidb-operator v2 bridge

**Files:**
- Create: `crates/loams-operator/src/drivers/{mod.rs,tikv_bridge.rs,tidb_operator_types.rs}` (kopium output, checked in)
- Test: `crates/loams-operator/tests/{tikv_bridge.rs,it_kind_tikv.rs}`

**Interfaces:**
- Consumes: `Pins` `tidb_operator`, `tikv` (Task 9); `reconcile_install` (Task 14), `resolve` (Task 16).
- Produces:
  - `#[async_trait] pub trait EngineDriver: Send + Sync { fn listing_id(&self) -> &'static str; fn render(&self, install: &LoamsInstall, l: &VerifiedListing, rel: &Release, ns: &str) -> Result<Rendered, DriverError>; fn health(&self, observed: &[DynamicObject]) -> Health { Ready, Progressing(String), Degraded(String) }; }`
  - `pub struct DriverRegistry` with `fn for_listing(&self, l: &VerifiedListing) -> Option<&dyn EngineDriver>` (only `Tier::FirstParty`)
  - `TikvBridgeDriver` (`listing_id() == "loams/tikv"`): renders `core.pingcap.com/v1alpha1` `Cluster`, `PDGroup` (replicas from `values.pd.replicas`, default 3, odd), `TiKVGroup` (`values.tikv.replicas`, default 3) with TiKV config `storage.api-version = 2`, `storage.enable-ttl = true` and zone location labels; no `TiDBGroup`; images from pins; maps group conditions to `Health`

- [ ] **Step 1: Write the failing tests** — `bridge_renders_no_tidbgroup`, `bridge_invariants` (api-version 2 and enable-ttl present; even PD replicas → `DriverError::EvenPd`), `bridge_status_mapped` (fixture PDGroup/TiKVGroup statuses → `Ready`/`Progressing`/`Degraded`), `driver_only_for_first_party` (a Verified listing with id `loams/tikv`… cannot exist, but a Community `x/tikv` gets `None`), `tikv_depends_on_tidb_operator` (fixture `loams/tikv` listing resolves `loams/tidb-operator` first); kind `#[ignore]` `it_kind_tikv_bridge` (install `loams/tikv` with 1 PD + 1 TiKV via `values`, confirm tidb-operator's cluster-scoped objects as the test user → `Ready` within 15 min).
- [ ] **Step 2: Run** `cargo test -p loams-operator -j 4 --test tikv_bridge` — Expected: FAIL
- [ ] **Step 3: Implement**; generate types with `kopium` from the pinned tidb-operator CRDs (Task 0 sha), header per Global Constraints; wire `DriverRegistry` into `Ctx`.
- [ ] **Step 4: Run** `cargo test -p loams-operator -j 4 --test tikv_bridge` — Expected: PASS
- [ ] **Step 5: Commit** — `git commit -s -m "operator: TiKV engine listing via the tidb-operator v2 bridge (OP2-A Task 17, D2601, D2623, D1182)"`

### Task 18: LOC spec 0.1 and the harness

**Files:**
- Create: `conformance/operator/loc-spec/{Cargo.toml,loc-spec.toml,src/lib.rs}`, `conformance/operator/loc-harness/{Cargo.toml,src/{lib.rs,bin/loc.rs,cluster.rs,inject.rs,audit.rs,report.rs,adapter.rs}}`, `conformance/operator/loc-harness/kind/{cluster.yaml,audit-policy.yaml}`
- Modify: `Cargo.toml` (`members` += `conformance/operator/*`)
- Test: `conformance/operator/loc-spec/tests/spec.rs`, `conformance/operator/loc-harness/tests/{audit.rs,report.rs}`

**Interfaces:**
- Produces:
  - `loc-spec.toml` 0.1: §65 §12.4's Core and HA rows (`id`, `level`, `strength`, `class`, `text`, `test`)
  - `pub fn spec() -> Spec`; `Spec::render_markdown(&self) -> String`
  - The `OperatorAdapter` trait exactly as §65 §12.6 (in `loc-harness::adapter`), with `Unsupported` results mapping to `not-applicable`
  - `pub async fn run(adapter: &dyn OperatorAdapter, level: Level, class: Class, cluster: ClusterKind::Kind) -> Report`; `Report` serialises to schema `loc.dev/report/v1` (§65 §12.8) plus JUnit XML
  - Injectors `pod_kill(name)`, `leader_kill(lease)`, `stale_leader(lease, pause: Duration)` (`docker exec <node> kill -STOP/-CONT <pid>`)
  - History checks over the audit log: `no_second_write(..)` (REC-001), `no_write_from_stale_holder(..)` (HA-002)

- [ ] **Step 1: Write the failing tests** — `spec_ids_unique`, `spec_renders_markdown`, `report_schema_valid` (against `conformance/operator/loc-harness/report.schema.json`), `history_check_detects_double_write` and `history_check_detects_stale_write` (fixture audit logs, one good, one bad each), `unsupported_is_not_applicable`.
- [ ] **Step 2: Run** `cargo test -p loc-spec -p loc-harness -j 4` — Expected: FAIL
- [ ] **Step 3: Implement**; kind config: 1 control plane + 3 workers labelled `topology.kubernetes.io/zone=z1..z3`, API-server audit log on with `audit-policy.yaml` (writes at `RequestResponse` for `loams.dev`, `apps`, core and `coordination.k8s.io`).
- [ ] **Step 4: Run** the same command — Expected: PASS
- [ ] **Step 5: Commit** — `git commit -s -m "conformance: LOC spec 0.1 and the kind harness (OP2-A Task 18, D1197, D1198)"`

### Task 19: Loams adapter, CI, image and chart publication

**Files:**
- Create: `conformance/operator/adapters/loams/{Cargo.toml,src/lib.rs}`, `.github/workflows/operator.yml`, `scripts/operator/loc-ha.sh`, `deploy/release/Containerfile.loams-operator`
- Modify: `deploy/release/build.sh` (image `loams-operator`), `.github/workflows/release-images.yml` (matrix += `loams-operator`), the chart release job (push `oci://ghcr.io/ostrium-labs/charts/loams-operator`)
- Test: `conformance/operator/adapters/loams/tests/core.rs`

**Interfaces:**
- Consumes: `OperatorAdapter`, `run` (Task 18); chart (Task 7); `FixtureSource` (Task 11).
- Produces: `pub struct LoamsAdapter` (`describe()` claims level Core, class `stateless`; workload = the home plus fixture `acme/hello` OCI install; `scale`/`backup`/`restore` → `Unsupported`); image `loams-operator` (Rust build stage as `Containerfile.loams-gateway`, runtime adds the pinned `helm` binary checked by sha256; non-root 65532); workflow `operator.yml` jobs `unit` (tests of the three crates, `-j 4`), `crd-drift` (`crdgen --check`), `chart` (lint + `tests/chart.rs`), `kind` (all `it_kind_*` with `--ignored`, then `loc run --level core`, report uploaded as an artifact).

- [ ] **Step 1: Write the failing test** — `loc_core_passes` (`#[ignore]`, kind): every Core MUST applicable to `stateless` is `pass`; `LOC-API-002`, `LOC-DEL-002`, `LOC-DEL-004` are `not-applicable`.
- [ ] **Step 2: Run** `cargo test -p loams-loc-adapter -j 4 --test core -- --ignored` on a local kind cluster — Expected: FAIL (adapter missing)
- [ ] **Step 3: Implement** the adapter, Containerfile, build-script entry, release matrix entry (amd64 and arm64 legs plus manifest, as the other images), chart push job, and `operator.yml`; `scripts/operator/loc-ha.sh` runs `loc run --level ha` with 3 operator replicas locally (LOC-HA-001, -002).
- [ ] **Step 4: Run** `cargo test -p loams-loc-adapter -j 4 --test core -- --ignored` and `deploy/release/build.sh loams-operator --platforms linux/amd64` — Expected: PASS; image builds and `docker run --rm <img> --help` lists `run` and `probe-object`
- [ ] **Step 5: Commit** — `git commit -s -m "operator: LOC Core in CI, the Loams adapter, image and chart publication (OP2-A Task 19, D1200)"`

### Task 20: Docs and amendments

**Files:**
- Create: `docs/operator/README.md` (preview; install with the chart; the home; installs and confirmation; tiers and sandbox; StorageBinding `object`; Loams Access (what each install gets, the audit events, OP2-D for the API/console/CLI); pause and force-delete annotations; LOC Core result link), `conformance/operator/README.md`
- Modify: `docs/design/65-loams-operator.md` §17 (rows for R1, R2, R13, R14), `docs/plans/README.md` (OP2-A row; OP1 marked superseded), `docs/plans/2026-10-19-op1-loams-operator.md` (status line: superseded by OP2-A)
- Test: the repository's link check

**Interfaces:**
- Consumes: everything above. Produces: nothing new.

- [ ] **Step 1: Write the docs** listed above, quoting the "Interfaces for sibling plans" table in `docs/operator/README.md`.
- [ ] **Step 2: Run** the repo's markdown link check on the changed files — Expected: no broken links
- [ ] **Step 3: Commit** — `git commit -s -m "docs: operator v2 core docs and §65 amendments (OP2-A Task 20)"`

---

## Owner actions

- None to start beyond UP1 merging. A k3s VM is not needed: kind on GitHub-hosted runners covers OP2-0; DEP1 (OP2-C) brings the first real cloud home.

## Rulings made during execution

(none yet)
