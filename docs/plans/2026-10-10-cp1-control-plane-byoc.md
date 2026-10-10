# CP1 — The Multitenant BYOC Control Plane in Production Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Each task lists the interfaces it must produce and the tests that must exist and pass before it is done. Where this plan gives exact names, paths, ports or defaults, use them verbatim. Where it gives a contract and named tests, write the code to that contract, and record any deviation in "Rulings made during execution" at the end of this file.
>
> **Status: Planned** (2026-10-10). Track CP, design [§41](../design/41-multitenant-byoc-control-plane.md) (D540–D559, Q540–Q559) and the control-plane parts of [§43](../design/43-private-networking.md) (§6, §8.2, §8.3, §11: D582, D585–D591, D598; Q582, Q588–Q593, Q596).
>
> **Supersedes, when accepted:**
> - [MT4](2026-10-02-mt4-byoc-control-plane.md) Tasks 0–7 and 9. MT4 Task 8 (the no-metering guard, merged under #258) stays as built; this plan's Task 27 extends it.
> - [NET1](2026-10-02-net1-private-networking.md) Task 5 (`loams-net` and BYOC join automation) and Task 6 (inventory diff). Its renderer, Task 2, is ported to Rust here (Ruling CP-R9); NET1 keeps Tasks 0, 1, 3, 4 and 7 (the policy as code, roster sync, templates, guide and Headscale e2e).
> - Neither plan is edited by this file. The integrator adds a pointer note to each when this plan is accepted.
>
> **Why a new plan.** MT4 was written on 2026-10-02 against an M2 `ControlStore`, an MT1 OpenFGA model and a `loams-operator`, and none of them exists yet. Since then five product control planes have started on their own: `pg-control` (PG2, `crates/loams-pg-control`, built through Task 5), `loams-sqldb` and `loams-sqlgate` (SQ1, TiDB on per-branch TiKV keyspaces), `loams-graph` (GR1), `loams-house` (HS1, in the `fabric/` workspace) and Live's app directory (LV1). Each one reads "the §41 limits record" and "the tenants repository" as if they existed, and each has an interim stand-in for them. CP1 builds the hub. It also builds **the one tenancy contract the five product planes plug into**: namespace identity, the directory, limits, authorization, provisioning, erasure and enforcement state. With that contract, onboarding, BYOC, rings and quotas work the same way for every product.

**Goal:** the open Multitenant BYOC Control Plane, in production, with every Loams product plugged into it:
- **The tenancy contract** (`loams-tenancy`): namespace identity, `Directory`, `LimitsSource`, `Authorizer`, `TenancyParticipant` and `ErasureReceipt`. Every product control plane implements or consumes it, with a static single-node implementation for `loams dev` and the desktop.
- **The hub** (`loams control`): `loams.control.v1` (orgs, namespaces, clusters, enrolments, limits, releases and rollouts) on a fenced control store extracted from `pg-control`'s, with authorization, audit, tenant caps and a signed directory.
- **Tenant onboarding as a Git commit**: a deterministic renderer covering every product's Kubernetes footprint, the Git writer, an `ApplicationSet`, and the operator's namespace reconcile, which provisions through the products and flips the namespace to `active`.
- **Offboarding with cross-product erasure**: no Kubernetes namespace, keyspace or directory entry is removed without every enabled product's erasure receipt.
- **BYOC**: the outbound-only cluster agent, enrolment, a closed command set, observe-only and air-gapped modes, and the directory mirrored into each cluster so that the data path never depends on the hub.
- **Private networking for BYOC** (§43): `loams-net` with `NetProvider` (Tailscale BYOK default, Headscale alternative), the Rust renderer for the tenant policy section, single-use tagged join keys, time-boxed support grants, and a nightly inventory diff that becomes audit events.
- **Release channels and upgrade rings**: a release manifest that carries every product's image pins, `compatibleWith` and `noDowngrade`, product smoke gates per ring, promotion, pause and rollback.
- **Enforcement without metering**: every product's limit lives in one record, and approximate enforcement state is visible. Nothing is billing-grade; the guard is extended to field names.
- **Operations**: the console's operator view, the CLI, Helm charts, metrics and alerts, a threat model, and the D558 exit gate extended to products.

The exit is "Exit criteria for production" at the end of this plan, with the tasks that own each item.

**Architecture** (§41 §1.1, §4–§9; §43 §6):
- **Two layers, one contract.**
  - **The hub, `loams control`.** A role of the `loams` binary behind the feature `control`, like `pg-control`. It owns orgs, namespaces, clusters, limits and releases. It never talks to a product's data path and never holds a Kubernetes credential.
  - **The product control planes** (`pg-control`, `loams-sqldb`, `loams-graph`, `loams-house`, Live) run **in each data-plane cluster**. Each keeps its own records in its own store, and reads tenancy only through `loams-tenancy`: is this namespace `active`, what are its limits, may this principal do this. Each answers four internal calls (`Describe`, `Provision`, `Erase`, `GetEnforcement`) on `loams.tenancy.v1`, cluster-local only.
- **Git is the write path for desired configuration** (§41 §6). The hub writes records and an outbox row in one transaction; the Git writer renders `tenants/<org>/…`; Argo CD (or Flux) applies it; the operator reconciles `LoamsNamespace` objects and calls the products' `Provision`. Runtime objects (compute Pods and Services) are the one scoped exception (D707, D736), and they are confined to the product's own Kubernetes namespaces by RBAC this plan renders.
- **Every cluster runs the agent, and the agent is the only hub client in a cluster** (Ruling CP-R5). Over outbound mTLS it sends heartbeats (versions, Argo CD conditions, `LoamsNamespace` conditions, erasure receipts, approximate enforcement summaries, gate results) and receives a signed, cluster-scoped directory. It mirrors that directory into the cluster's local control store. Gateways and product planes read it there, so a hub outage changes nothing on the data path, and in BYOC no product holds a hub credential.
- **Enforcement is not metering** (§41 §9.2). Limits flow hub → directory → product. Enforcement state flows product → agent → hub as approximate samples that are never persisted beyond the latest one. The products' private lifecycle observers (`ComputeLifecycleObserver`, the gate's activity accounting) are untouched, and nothing in CP1 consumes them.
- **The tailnet is a network path, not an identity** (§43 §6.2). In tailnet mode the agent reaches `tag:control:443` with the same client certificate. `loams-net` issues keys and applies policy only after the checker passes.

**Tech Stack:**
- Rust 1.97.1 (workspace `rust-version`), edition 2024, workspace lints.
- `connectrpc` 0.9 and `buffa` through `loams-proto` (D362, §44).
- `tokio`, `reqwest` 0.13 (rustls), `rustls`, `rcgen` (the cluster CA, ECDSA P-256), `ed25519-dalek` (directory and command signatures; Task 0 pins the release), `sha2`, `ulid`, `postcard`, `serde`.
- `loams-kv` (the store seam under `loams-ctlstore`), `loams-tikv` and `loams-meta-tikv`.
- `kube` 4.2.0 with `k8s-openapi` 0.28.0 (feature `v1_36`), the versions SQ1 pinned. Only in the operator and the agent, never in the hub.
- Git: `gix` or a `git` subprocess behind the `GitRemote` trait (Task 0 measures and decides).
- YAML: a maintained serializer chosen in Task 0 (`serde_yaml` is archived), behind a canonical emitter that the golden tests pin.
- Test tools:
  - k3d and kind (CI only), Argo CD at the version MT3 or Task 0 pins, Flux for the small profile, Forgejo as the CI Git host, `kubeconform`, `helm`;
  - `wiremock` for the Tailscale and Headscale clients, `trybuild` for compile-fail tests, `proptest`;
  - Headscale v0.29.4 and `tailscale/tailscale` v1.102.5, pinned by the digests in §43 §3 (Task 0 re-checks them).
- `buf` for protos.

**Spec:**
- [§41](../design/41-multitenant-byoc-control-plane.md) (all), D540–D559, Q540–Q559 in the [decision log](../design/13-decision-log.md).
- [§43](../design/43-private-networking.md) §5 (tags and join keys), §6 (BYOC over a tailnet), §8.1–§8.3 (policy, keys, audit), §11 (tooling).
- [§18](../design/18-metastore-backends-and-router.md) §5.2 (the directory), §6 (orgs, `ControlStore`, quotas), §8 (BYOC modes, D64), §9 (erasure, D68, D69).
- [§38](../design/38-knative-authentik-gitops.md) §3.3 (per-namespace tenancy, D443), §4 (Authentik), §6 (waves).
- The product designs, for their tenancy hooks:
  - [§46](../design/46-loams-postgres-production.md) §6, §14, §15 (D703, D707, D717);
  - [§47](../design/47-loams-sql-production.md) §16, §20 (D734, D736, D738);
  - [§48](../design/48-loams-graph-production.md) §11.1, §13.2 (D752);
  - [§49](../design/49-loams-house-production.md) §14, §15 (D776);
  - [§45](../design/45-loams-live-production.md) §3.2, §7, §12 (D682, D690, D696).
- The product plans, at their current rulings: [PG2](2026-10-08-pg2-postgres-production.md) (Task 0 rulings 3–5, R1.7, R5.x; Tasks 9, 47, 48), [SQ1](2026-10-08-sq1-loams-sql-tidb.md) (Tasks 11, 14, 25), [GR1](2026-10-08-gr1-graph-production.md) (Tasks 24, 25), [HS1](2026-10-08-hs1-house-production.md) (Tasks 20, 22, 26), [LV1](2026-10-08-lv1-live-production.md) (Task 28).
- [§44](../design/44-unified-api-and-sdks.md) §4–§7 (API rules); [open-core.md](../open-core.md); MT1, MT2, MT3 and NET1 as planned.

## Global Constraints

- **Worktree and branches.** Work in `~/Documents/Ostriumlabs/loams-wt/cp1-control-plane`. Use one branch per milestone, `feat/cp1a-tenancy`, `feat/cp1b-hub`, `feat/cp1c-product-fit`, `feat/cp1d-gitops`, `feat/cp1e-byoc`, `feat/cp1f-rings` and `feat/cp1g-ops`, each based on `dev`, with stacked PRs targeting `dev`. A product-fit task (8–12) may instead land on the product's own branch if that product's coordinator asks; record which. Use `git commit -s` (DCO). Commit areas: `control`, `tenancy`, `ctlstore`, `gitops`, `operator`, `byoc`, `net`, `pg`, `sqldb`, `graph`, `house`, `live`, `console`, `cli`, `deploy`, `ci`, `docs`.
- **Rust builds** use the shared target directory (`~/Documents/.cargo/config.toml`). Never set `CARGO_TARGET_DIR` and never build in `/tmp`. Run one cargo build at a time on the build machine (jobs and linker from `~/.cargo/config.toml`), and build the touched crates (`cargo test -p loams-control`), not the workspace. `fabric/` is a separate workspace: build it separately, never at the same time. k3d, kind, Forgejo, Argo CD and Headscale containers run in CI jobs, or locally only when no cargo build is running.
- **The default build changes by one small crate only.** `loams-tenancy` (types and traits; no I/O dependency beyond `futures`) may enter the default build, because Live is in the default set. `loams-control`, `loams-ctlstore`, `loams-gitops`, `loams-net`, `kube` and `rcgen` must not. `cargo tree -p loams -e normal` on default features must not list them (Task 5 test).
- **The hub holds no cluster credential.** `loams-control` has no `kube` dependency (`cargo tree` test, Task 5). The hub changes a cluster only by committing to Git or by sending a signed command from the closed set to the agent.
- **AP0 API rules** (§44): every mutation has `string idempotency_key = 15`; reads are `NO_SIDE_EFFECTS`; errors are `loams.errors.v1` with a reason registered in `docs/api/reasons.md`; pagination is `page_size`/`page_token`; long work returns a `loams.operations.v1.Operation`; watch streams send a snapshot, then changes, then a heartbeat every 15 s. This replaces MT4's `client_token` (Ruling CP-R2).
- **Deterministic rendering.** Tenants-repository files, Kubernetes objects, the directory snapshot's signed bytes and the tailnet policy's tenant section are byte-identical for identical records (golden tests). Rebuilding the tenants repository from the store yields identical bytes.
- **Namespace identity.** Every product record, Kubernetes namespace, keyspace mapping, PgDog database and bucket prefix that belongs to a namespace is keyed by the directory's `NamespaceId` (u64), never by the namespace's name alone. Names are unique only within an org (§18 §6) (Ruling CP-R7; CPQ1).
- **Secrets.** Enrolment tokens, CA keys, signing keys, Git credentials, Tailscale OAuth secrets, Headscale API keys and join keys are typed `Secret<T>`, whose `Debug` and `Display` print `[redacted]`. No secret appears in a log line, an error, an `Operation.result`, a metric label, an audit event or a Git file. An enrolment token and a join key are returned once. Only a SHA-256 hash of an enrolment token is stored.
- **No billing** (D541, D548, D550). No field, metric, table, endpoint or RPC whose snake_case name has a segment equal to `plan`, `price`, `invoice`, `credit`, `billable`, `meter` or `usage`, in any CP1 code or in a product's tenancy adapter. Task 27's guard enforces it by segment, so `explain` and `placement` pass. The approximate `used_approx` is enforcement state, and the API and the console say so. **This plan names none of the guard's protocol literals**, so it needs no allowlist entry.
- **The dependency runs one way** (D551). Nothing names `loams-platform`. The extension points the platform uses are the operations API, the limits API (through the `limits_writer` relation), the tenants repository and the products' observers. CP1 adds no other.
- **Fail closed on tenancy, open on the data path.** If the hub's store is unavailable, `CreateOrg` and `CreateNamespace` are refused. Gateways and products keep enforcing the last directory they hold (`LimitsOrigin::LastKnown`).
- **Agents are not operators.** An `agent` principal can hold neither `deployment#operator` nor `deployment#limits_writer`, nor `org#admin` on an org other than its own (tests in Tasks 3 and 5).
- **Pins.** Exact versions for every new crate and image, and digests for every image. A new dependency must be at least 14 days old (`cargo info`, the registry date) and pass `cargo deny`. Record each pin in the task's commit message.
- **External claims marked (verify)** in §41 and §43 (ApplicationSet `RollingSync`, Argo CD signature verification, Tailscale tagged-device expiry, OpenFGA modular models) are checked in Task 0 before the task that depends on them.

## Rulings made while writing this plan

| # | Ruling | Why | Cost if wrong |
|---|---|---|---|
| CP-R1 | **Protos go into `loams-proto`'s `FILES` list** (`proto/loams/control/v1/*.proto`, `proto/loams/tenancy/v1/tenancy.proto`); there is no `loams-control-proto` crate (amends MT4 Ruling 1) | The as-built convention (PG2 Task 0 ruling 2) | A later split is mechanical |
| CP-R2 | **AP0's `idempotency_key = 15`**, not MT4's `client_token` | One rule across the API (§44) | None; nothing is built on `client_token` |
| CP-R3 | **The hub is a role, `loams control`, of the `loams` binary behind the feature `control`**; `loams-byoc-agent` is its own small binary and image | The same shape as `pg-control` (PG2 Task 9); the agent must be small and hold no engine | Splitting the hub out later is a packaging change (CPQ10) |
| CP-R4 | **The control store is `pg-control`'s fenced store, extracted** into `crates/loams-ctlstore` (generic over `Record`, local redb and TiKV backends, one conformance suite, `IdempotencyLedger`). `pg-control` re-exports it under its existing names, so PG2's code and tests do not change | It is the only built, conformance-tested, fenced control store in the tree; the M2 `ControlStore` does not exist. SQ1's unbuilt `store/` can adopt it | A conflict with PG2's in-flight branches; fallback is a copy (CPQ2) |
| CP-R5 | **Every cluster runs the agent**, managed or BYOC. It is the only hub client in a cluster: it mirrors the signed, cluster-scoped directory into the cluster's control store, and gateways and products read it locally. `SetLimits` (§41 §7.2) becomes `ApplyDirectory`, which carries limits under the same signature rule | One path for status and limits; the hub never reads Kubernetes; the data path survives a hub outage; no product holds a hub credential | Mirror latency of one heartbeat interval (CPQ3) |
| CP-R6 | **Product Kubernetes namespaces are `loams-<p>-<nsid>`**, where `<p>` is `ns` (functions and Knative, D443), `pg`, `sql` or `house`, and `<nsid>` is the `NamespaceId` as 13 lowercase Crockford base32 characters. Labels carry `loams.dev/org`, `loams.dev/namespace` (the id), `loams.dev/namespace-name` and `loams.dev/product` | §41's `loams-ns-<namespace>` and §46's `loams-pg-<ns>` collide across orgs (names are unique only per org) and break on rename | Renames in PG2 Task 48 and SQ1 Task 14 before they ship (CPQ1) |
| CP-R7 | **Products key namespace-owned records by `NamespaceId`.** Product APIs keep a `namespace` string field, which is a name within the caller's org (or the id in decimal). The product resolves it through `Directory::resolve` before any store access | Cross-org isolation; rename safety | PG2 re-keys `x/<ns>/…` and `Q/n/<ns>/…` before GA (Task 8) |
| CP-R8 | **Chart paths:** `deploy/helm/loams-control/` (the hub) and `deploy/helm/loams-byoc-agent/` (the agent, its tailnet sidecar and the operator's tenancy controller). Both become subcharts of `deploy/helm/loams-stack` when MT3 creates it | Matches PG2's `deploy/helm/loams-postgres/`; replaces MT4's `charts/control` and NET1's `charts/loams-byoc-agent` | A path move |
| CP-R9 | **The tenant section of the tailnet policy is rendered in Rust** (`loams-net::policy::render_tenant_section`), because the hub is its only automated caller. NET1 Task 2's goldens and test names are kept, and `loams net render-policy` is the CLI over the same function | One renderer; the hub cannot shell out to Python in its image | NET1 keeps a Python renderer if the owner prefers it; then both must match the shared goldens (CPQ8) |
| CP-R10 | **The operator's namespace controller is created here if MT2 has not created `loams-operator`.** It is `crates/loams-operator` with only the `LoamsNamespace` controller, on `kube` 4.2.0; MT2 adds Knative tenancy to the same crate later | Onboarding cannot wait for the Clever fork (D185) | Reconciling with the fork's layout later (CPQ5) |

## Review Focus

1. **Anything billing-grade enters the open code.** Expected: never. Tests: Task 27 `guard_rejects_billing_segments`, `guard_allows_explain_and_placement`, `no_rpc_spends_or_reports_money`; Task 6 `enforcement_state_never_persisted_beyond_latest`, `over_quota_writes_no_record`; Task 11 `house_tenancy_adapter_emits_no_events`.
2. **The hub writes to a cluster other than through Git or a signed command.** Expected: never. Tests: Task 5 `hub_has_no_kube_dependency`; Task 15 `namespace_appears_only_through_argo`; Task 19 `unknown_command_refused`, `unsigned_directory_refused`.
3. **Two orgs with the same namespace name share anything**: a product record, a Kubernetes namespace, a keyspace, a PgDog database, a bucket prefix or a tailnet tag. Expected: never. Tests: Task 8 `same_namespace_name_two_orgs_isolated_pg`; Task 9 `same_namespace_name_two_orgs_isolated_sql`; Task 13 `kube_namespace_names_never_collide`; Task 21 `tenant_tags_unique_per_org`.
4. **A product serves a namespace that is not `active`** (still provisioning, deleting or erased). Expected: refused with `namespace_not_active`. Tests: Tasks 8–12 `inactive_namespace_refused_<product>`.
5. **A namespace's data, Kubernetes namespace or directory entry is removed before every enabled product has sent its erasure receipt.** Expected: never. Tests: Task 16 `kube_namespace_kept_until_all_receipts`, `directory_entry_kept_until_all_receipts`, `missing_product_receipt_blocks_offboarding`.
6. **A stolen agent certificate, or an enrolment token, reaches more than one cluster or lives longer than its window.** Expected: bounded. Tests: Task 18 `enrolment_token_single_use`, `cert_scoped_to_cluster`, `revoked_cert_refused_within_one_heartbeat`; Task 19 `agent_has_no_inbound_listener`.
7. **An agent principal raises a limit, or enforcement stops when the hub is down.** Expected: no, and no. Tests: Task 6 `agent_cannot_write_limits`; Task 7 `limits_remain_when_hub_down`, `restart_without_hub_uses_mirrored_directory`.
8. **One tenant's tailnet node reaches another tenant or an operator tag, or a failing policy is applied.** Expected: never. Tests: Task 21 `apply_refuses_failing_policy`, `tenant_cannot_reach_other_tenant`; Task 22 `byoc_node_reaches_control_only`.
9. **A release promotes past a failed gate, or downgrades a `noDowngrade` component.** Expected: never. Tests: Task 25 `failed_gate_blocks_promotion`, `pause_stops_ring`; Task 24 `no_downgrade_refused`.
10. **Extracting the store changes `pg-control`.** Expected: no behaviour change. Test: Task 2 `pg_control_suite_unchanged`.

---

## File structure

```
proto/loams/control/v1/{control.proto,limits.proto,directory.proto,release.proto,agent.proto}   Task 1
proto/loams/tenancy/v1/tenancy.proto                                                            Task 1
crates/loams-proto/build.rs                                (FILES gains the packages)           Task 1
crates/loams-ctlstore/                                                                           Task 2
  src/{lib.rs,record.rs,kv.rs,local.rs,tikv.rs,lease.rs,batch.rs,idempotency.rs,secret.rs,prefixes.rs}
  src/conformance.rs   tests/{local.rs,tikv.rs,prefixes.rs}
crates/loams-pg-control/src/store/mod.rs                   (re-exports; Task 2)
crates/loams-tenancy/                                                                            Task 3
  src/{lib.rs,ids.rs,principal.rs,directory.rs,limits.rs,authz.rs,participant.rs,erasure.rs,enforcement.rs,audit.rs,static_dir.rs}
crates/loams-control/                                                                            Tasks 4–7, 16, 18, 24–25
  src/{lib.rs,model.rs,keys.rs,outbox.rs,audit.rs,authz.rs,caps.rs}
  src/service/{mod.rs,orgs.rs,namespaces.rs,clusters.rs,limits.rs,releases.rs,directory.rs,agent.rs}
  src/directory/{build.rs,sign.rs,scope.rs}
  src/{ca.rs,enrol.rs,status.rs,erasure.rs,rings.rs,gates.rs,net.rs}
  tests/
crates/loams-tenancy-client/                               (StoreDirectory, mirror reader; Task 7)
crates/loams-gitops/src/{lib.rs,layout.rs,render.rs,emit.rs,writer.rs,remote.rs,sign.rs}         Tasks 13–14
crates/loams-operator/src/{lib.rs,namespace.rs,participants.rs,erasure.rs,rbac.rs}               Tasks 15–16 (CP-R10)
crates/loams-byoc-agent/src/{main.rs,enrol.rs,heartbeat.rs,commands.rs,mirror.rs,observe_only.rs,bundle.rs,tailnet.rs}   Tasks 19–20, 22
crates/loams-net/src/{lib.rs,provider.rs,tailscale.rs,headscale.rs,checker.rs,policy.rs,inventory.rs}  Tasks 21–23
crates/loams-pg-control/src/tenancy.rs                                                           Task 8
crates/loams-sqldb/src/tenancy.rs  crates/loams-sqlgate/src/tenancy.rs                           Task 9
crates/loams-graph/src/tenancy.rs                                                                Task 10
fabric/crates/loams-house/src/tenancy.rs                                                         Task 11
crates/loams-live/src/tenancy.rs                                                                 Task 12
crates/loams/src/{main.rs,server.rs,api/connect_control.rs,cli/control.rs}                       Tasks 5, 26
deploy/gitops/{appset-tenants.yaml,appset-rings/,gates/}                                         Tasks 15, 25
deploy/openfga/{core.fga,postgres.fga,sqldb.fga,graph.fga,house.fga,live.fga,fga.mod}            Task 3
deploy/helm/loams-control/  deploy/helm/loams-byoc-agent/                                        Task 28
deploy/observability/loams-control/{dashboards/,alerts.yaml}                                    Task 28
release/manifest.toml                                                                            Task 24
web/apps/console/src/pages/operator/                                                             Task 26
scripts/ci/no-metering.py   (field-segment rule)                                                 Task 27
docs/security/loams-control-threat-model.md   docs/runbooks/loams-control/                       Tasks 29–30
.github/workflows/{cp1.yml,cp1-e2e.yml}
```

## Shared contracts (all tasks use these names)

### Proto (Task 1 writes it; this is the contract, not the full file)

```proto
syntax = "proto3";
package loams.control.v1;

service OrgService {
  rpc CreateOrg(CreateOrgRequest) returns (CreateOrgResponse);
  rpc GetOrg(GetOrgRequest) returns (GetOrgResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc ListOrgs(ListOrgsRequest) returns (ListOrgsResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc UpdateOrg(UpdateOrgRequest) returns (UpdateOrgResponse);
  rpc DeleteOrg(DeleteOrgRequest) returns (DeleteOrgResponse);                       // -> Operation
}
service NamespaceService {
  rpc CreateNamespace(CreateNamespaceRequest) returns (CreateNamespaceResponse);     // -> Operation
  rpc GetNamespace(GetNamespaceRequest) returns (GetNamespaceResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc ListNamespaces(ListNamespacesRequest) returns (ListNamespacesResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc UpdateNamespace(UpdateNamespaceRequest) returns (UpdateNamespaceResponse);     // products, placement, egress
  rpc DeleteNamespace(DeleteNamespaceRequest) returns (DeleteNamespaceResponse);     // -> Operation (erasure)
  rpc GetNamespaceStatus(GetNamespaceStatusRequest) returns (GetNamespaceStatusResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
}
service ClusterService {
  rpc CreateCluster(CreateClusterRequest) returns (CreateClusterResponse);           // kind: MANAGED | BYOC
  rpc GetCluster(GetClusterRequest) returns (GetClusterResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc ListClusters(ListClustersRequest) returns (ListClustersResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc UpdateCluster(UpdateClusterRequest) returns (UpdateClusterResponse);           // channel, ring, window, observe_only, net
  rpc CreateEnrolment(CreateEnrolmentRequest) returns (CreateEnrolmentResponse);     // token returned once
  rpc RevokeCluster(RevokeClusterRequest) returns (RevokeClusterResponse);
}
service LimitsService {
  rpc PutLimits(PutLimitsRequest) returns (PutLimitsResponse);                       // org or namespace scope
  rpc GetLimits(GetLimitsRequest) returns (GetLimitsResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc GetEnforcementState(GetEnforcementStateRequest) returns (GetEnforcementStateResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
}
service ReleaseService {
  rpc PublishRelease(PublishReleaseRequest) returns (PublishReleaseResponse);
  rpc GetRelease(GetReleaseRequest) returns (GetReleaseResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc ListReleases(ListReleasesRequest) returns (ListReleasesResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc Promote(PromoteRequest) returns (PromoteResponse);                             // -> Operation
  rpc PauseRollout(PauseRolloutRequest) returns (PauseRolloutResponse);
  rpc ResumeRollout(ResumeRolloutRequest) returns (ResumeRolloutResponse);
  rpc Rollback(RollbackRequest) returns (RollbackResponse);                          // -> Operation
  rpc GetRollout(GetRolloutRequest) returns (GetRolloutResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc WatchRollout(WatchRolloutRequest) returns (stream WatchRolloutResponse);
}
// Cluster-facing. mTLS with the cluster certificate; every call is scoped to the certificate's cluster_id.
service AgentService {
  rpc Enrol(EnrolRequest) returns (EnrolResponse);                                   // token + CSR -> 24 h certificate
  rpc Renew(RenewRequest) returns (RenewResponse);
  rpc Report(stream AgentReport) returns (ReportAck);                                // heartbeats
  rpc Commands(CommandsRequest) returns (stream SignedCommand);                      // closed set, below
  rpc AckCommand(AckCommandRequest) returns (AckCommandResponse);
}

// Every mutating request has `string idempotency_key = 15;`.
// Ids: "org-", "cl-", "enr-", "rel-", "ro-" + 26-char Crockford ULID. A namespace id is the directory's u64.
// Namespace.state: NAMESPACE_STATE_{PROVISIONING,ACTIVE,DELETING,ERASED}.
// Product: PRODUCT_{COLLECTIONS,FUNCTIONS,POSTGRES,SQL,GRAPH,HOUSE,LIVE}.

message Limits {                     // every field optional; unset = the deployment default
  CoreLimits core = 1;               // requests_per_s, ingest_bytes_per_s, concurrent_queries, storage_bytes,
                                     // unapplied_records, unapplied_bytes, metadata_ops_per_s, namespaces_per_org
  KubeLimits kube = 2;               // cpu_millis, memory_bytes, pods, knative_max_scale (D443)
  PostgresLimits postgres = 3;       // §46 §14: projects, branches_per_project, endpoints_per_project, max_cu_per_endpoint,
                                     // total_cu, storage_bytes, history_retention_s, connections_per_endpoint, new_connections_per_s
  SqlLimits sql = 4;                 // §47 §20: databases, branches_per_database, storage_bytes_per_database,
                                     // connections_per_database, new_connections_per_s, max_class, txn_duration_s
  GraphLimits graph = 5;             // §48 §13.2: graphs, elements, stored_bytes, concurrent_statements,
                                     // statements_per_s, memory_bytes, import_bytes_per_day
  HouseLimits house = 6;             // §49 §15: concurrent_queries, queries_per_min, scan_bytes_per_day,
                                     // cpu_seconds_per_day, lake_storage_bytes, insert_bytes_per_day, pipes
  LiveLimits live = 7;               // §45 §12: apps, requests_per_s_app, requests_per_s_principal, mutations_per_s,
                                     // sessions_per_node, function_cpu_s_per_min, storage_bytes_per_app, documents_per_app
}
message EnforcementSample {          // "enforcement state, not billing-grade" (proto comment and console label)
  Product product = 1; string limit = 2; optional uint64 limit_value = 3;
  uint64 used_approx = 4; uint32 window_s = 5; int64 as_of_ms = 6;
}
message DirEntry {
  uint64 namespace_id = 1; string org_id = 2; string name = 3; NamespaceState state = 4;
  uint32 shard = 5; SizeClass size_class = 6; repeated Product products = 7;
  Limits limits = 8;                 // org limits merged under namespace limits, defaults NOT filled in
  uint64 limits_generation = 9; repeated string cluster_ids = 10;
}
message DirectorySnapshot {          // cluster-scoped; signed over the canonical bytes of fields 1–4
  string cluster_id = 1; uint64 generation = 2; repeated DirEntry entries = 3; int64 issued_at_ms = 4;
  string key_id = 5; bytes signature = 6;   // Ed25519
}
message SignedCommand {
  string command_id = 1; string cluster_id = 2; int64 not_after_ms = 3;
  oneof command { Pause pause = 10; ResumeRollout resume = 11; ReportBundle report = 12;
                  RotateCertificate rotate = 13; ApplyDirectory apply_directory = 14; }   // closed; CP-R5
  string key_id = 20; bytes signature = 21;
}
```

```proto
syntax = "proto3";
package loams.tenancy.v1;           // internal: cluster-local listener, mTLS, never on a public route

service TenancyParticipant {         // served by each product role
  rpc Describe(DescribeRequest) returns (DescribeResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
                                     // product, version, footprint, the limit names it enforces
  rpc Provision(ProvisionRequest) returns (ProvisionResponse);        // idempotent by (namespace_id, generation)
  rpc Erase(EraseRequest) returns (EraseResponse);                    // idempotent by request_id; IN_PROGRESS | DONE(receipt)
  rpc GetEnforcement(GetEnforcementRequest) returns (GetEnforcementResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
}
message ErasureReceipt {             // no data: counts and a digest only
  uint64 namespace_id = 1; string product = 2; string request_id = 3; int64 completed_at_ms = 4;
  uint64 objects_removed = 5; bytes key_hashes_digest = 6;   // SHA-256 over the sorted HMAC key hashes (§18 §9)
}
```

### Rust traits (`loams-tenancy`)

```rust
pub struct OrgId(Ulid);                        // "org-…"; Display/FromStr
pub struct NamespaceId(pub u64);               // From<loams_common::NamespaceId> behind feature `common`
pub struct NamespaceRef { pub id: NamespaceId, pub org: OrgId, pub name: String }
pub enum Product { Collections, Functions, Postgres, Sql, Graph, House, Live }
pub enum NamespaceState { Provisioning, Active, Deleting, Erased }
pub fn kube_namespace(p: Product, id: NamespaceId) -> Option<String>; // CP-R6; None for directory-only products

pub trait Directory: Send + Sync + 'static {
    fn get(&self, id: NamespaceId) -> Option<Arc<DirEntry>>;
    fn resolve(&self, org: &OrgId, name: &str) -> Option<Arc<DirEntry>>;
    fn generation(&self) -> u64;
    fn changes(&self) -> BoxStream<'static, DirectoryChange>;          // Upserted(entry) | Removed(id)
    /// The one admission check every product makes before touching its store.
    fn require_active(&self, org: &OrgId, ns: &str) -> Result<Arc<DirEntry>, TenancyError> {  /* default */ }
}
pub trait LimitsSource: Send + Sync + 'static {
    /// Never fails: the record, else the last known, else the deployment default.
    fn effective(&self, ns: NamespaceId) -> EffectiveLimits;
}
pub struct EffectiveLimits { pub limits: Arc<Limits>, pub generation: u64, pub origin: LimitsOrigin }
pub enum LimitsOrigin { Record, LastKnown { since_ms: u64 }, DeploymentDefault }

pub enum PrincipalKind { Person, Agent, Service }
pub struct Principal { pub kind: PrincipalKind, pub id: String, pub org: Option<OrgId>, pub act: Vec<String> }
pub struct ObjectRef { pub kind: &'static str, pub id: String }       // "deployment", "org", "namespace", "pg_project", …
#[async_trait]
pub trait Authorizer: Send + Sync + 'static {
    async fn check(&self, who: &Principal, relation: &str, object: &ObjectRef) -> Result<bool, AuthzError>;
}
// Implementations: DevAuthorizer (allow-all; refuses to start unless every listener is loopback),
// RbacAuthorizer (built in; bindings from the control store), OpenFgaAuthorizer (when MT1 lands).

#[async_trait]
pub trait TenancyParticipant: Send + Sync + 'static {
    fn describe(&self) -> Participation;          // product, footprint (DirectoryOnly | KubeNamespace), limit names
    async fn provision(&self, ns: &NamespaceRef, generation: u64) -> Result<ProvisionStatus, ParticipantError>;
    async fn erase(&self, ns: &NamespaceRef, request_id: Ulid) -> Result<ErasureProgress, ParticipantError>;
    fn enforcement(&self, ns: NamespaceId) -> Vec<EnforcementSample>;
}
pub enum ErasureProgress { InProgress { done: u64, total: Option<u64> }, Done(ErasureReceipt) }

pub trait AuditSink: Send + Sync + 'static { fn emit(&self, e: AuditEvent); } // principal, act chain, action, object, outcome
```

### Product fit (the contract each product meets; Tasks 8–12)

| | Postgres (`pg-control`) | SQL (`loams-sqldb`, `loams-sqlgate`) | Graph (`loams-graph`) | House (`loams-house`, `fabric/`) | Live (`loams-live`) |
|---|---|---|---|---|---|
| Runs as | role `pg-control`, feature `postgres` | `loams` features `sqldb`; gate role | in `loams`, feature `graph` | `loams-fabric` role `house` | in `loams`, feature `live` |
| Footprint | `loams-pg-<nsid>`: PgDog ×2, NetworkPolicy, ResourceQuota from Git; Pods and Services by `pg-control` (D707) | `loams-sql-<nsid>`: per-branch Deployment, Service, Secret and NetworkPolicy by the sqldb controller (D736); PD, TiKV and the gate pool are shared | Directory-only | `loams-house-<nsid>` only for the worker fallback mode; otherwise directory-only | Directory-only; one keyspace per app |
| Records keyed by | `NamespaceId` (re-key from the name, CP-R7) | `NamespaceId` (already `m/<ns u64 BE>`) | the metastore namespace (already the id) | the namespace id | the app directory gains `namespace_id` |
| Limits section | `postgres` | `sql` | `graph` | `house` | `live` |
| Replaces | PG2 Task 47's `PgLimits` fallback | SQ1 Task 25's config quotas | GR1 Task 25's config source | HS1 Task 22's `QuotaSource` config impl | LV1 Task 28's app-directory limits (D65 note) |
| Erasure | delete every project (Neon tenant, timelines, `loams-wal` timelines, the bucket prefix) | delete every keyspace (PD archive, then TiKV range delete) and `sqlbackup/<ns>/` | delete graphs, their log streams and bucket prefixes | drop the namespace's lake tables and `_house.query_log` partitions | delete apps and their keyspaces |
| Authz types | `pg_project` | `sql_database` | `graph` | `house_database`, `house_table` | `live_app` |
| Private observer (untouched) | `ComputeLifecycleObserver` | the gate's activity accounting | — | §49 §15 events (CPQ4) | — |

### Naming grammar

- `NamespaceId` in Kubernetes names: 13 lowercase Crockford base32 characters, zero-padded (`kube_namespace` is the only implementation). Kubernetes namespace: `loams-<p>-<nsid>`, at most 25 characters.
- Tailnet tag of a BYOC tenant: `tag:byoc-<org id lowercased>` (`org-` + 26 = 30 characters, within §43's `^[a-z][a-z0-9-]{1,30}[a-z0-9]$`). Node hostname `byoc-<cluster id lowercased>`.
- Control-store keys live under the prefix `cp/`; lease scopes are `e/cp/<scope>`. `loams-ctlstore::prefixes::REGISTERED` lists every registered prefix (metastore, `pg-control`, sqldb, `cp/`) and a test proves that none is a prefix of another.

---

## Execution order

1. Task 0.
2. **CP1a** (Tasks 1–4). Task 2 is scheduled with PG2's coordinator, because it moves code out of `loams-pg-control`.
3. **CP1b** (Tasks 5–7).
4. **CP1c** (Tasks 8–12) after Tasks 3 and 7. Each runs beside its product's plan, as soon as that product has a store and an admission path: PG2 is ready now, SQ1 after its Task 11, GR1 now, HS1 after its Task 20, LV1 after its Task 21.
5. **CP1d** (Tasks 13–17) after CP1b. Task 15 needs `loams-operator` (CP-R10).
6. **CP1e** (Tasks 18–23) after Task 7. Tasks 21–23 need NET1 Task 1 (the policy checker) and may start beside Tasks 18–20.
7. **CP1f** (Tasks 24–25) after Task 19.
8. **CP1g** (Tasks 26–30) last; Task 27 may run at any time.

---

### Task 0: Reconcile with the code as built

**Files:** this plan's "Rulings made during execution" only.

Steps:
1. Answer each of the following and record the answer, with file paths, as a ruling:
   - What is merged of MT1 (OpenFGA model, Authentik verifier), MT2 (`loams-operator`), MT3 (waves, Argo CD version, `deploy/helm/loams-stack`) and RN1 Task 3 (`InvocationObserver`)? Each absent item keeps this plan's fallback (Task 3's `RbacAuthorizer`, CP-R10, Task 28's standalone charts).
   - `pg-control`'s store as of `dev`: is `crates/loams-pg-control/src/store` still generic over `Record`, and which PG2 branches touch it? Agree the Task 2 window with PG2's coordinator, or record the fallback (copy, CPQ2).
   - The namespace identity of each product as built: `pg-control` keys (`x/<ns>/`, `Q/n/<ns>/`), `loams-sqldb` (`m/<ns u64 BE>`), `loams-graph` (the metastore namespace), `loams-house` (`Bind`'s namespace id), Live (`loams_live_system` `apps/<id>`). List every place a bare name is used as a key.
   - The free key prefixes in `loams-meta-tikv/src/keys.rs` and `loams-pg-control/src/model.rs`; confirm `cp/` and `e/cp/` are free.
   - Argo CD at the pinned version: `ApplicationSet` progressive sync (`RollingSync`) status (Q559, **(verify)**); commit signature verification (GPG, SSH, gitsign) and `signatureKeys` (Q556).
   - `gix` push against Forgejo versus a `git` subprocess: measure a commit and push of 1,000 files on the build machine. Pick one for `GitRemote`.
   - Forgejo's API for opening pull requests, branch protection and OIDC with Authentik.
   - OpenFGA modular models (`fga.mod`), and the minimum OpenFGA version (**(verify)**).
   - A maintained YAML serializer (`serde_yaml` is archived), at least 14 days old, or the canonical emitter alone.
   - `ed25519-dalek`, `rcgen` and `biscuit-auth` (Q542) releases at least 14 days old, and `cargo deny`.
   - The §43 pins: Headscale v0.29.4 and `tailscale/tailscale` v1.102.5 digests, and whether a tagged Tailscale device has key expiry disabled by default (**(verify)**, §43 §8.2).
   - Whether `fabric/` can take a path dependency on `crates/loams-tenancy` without pulling `loams-common` (CPQ9).
   - The CLI's home: `crates/loams/src/main.rs` subcommands, or a CLI crate if one now exists.
2. Record defaults for Q540–Q559 and the carried §43 questions where no owner answer exists, as MT4 Task 0 would have.
3. Commit `docs(cp1): task 0 rulings`.

## CP1a — The tenancy contract and the control store (Tasks 1–4)

### Task 1: `loams.control.v1`, `loams.tenancy.v1`, reasons and route map

**Files:** create `proto/loams/control/v1/{control.proto,limits.proto,directory.proto,release.proto,agent.proto}` and `proto/loams/tenancy/v1/tenancy.proto`. Modify `crates/loams-proto/build.rs` (`FILES`), `buf.yaml` (`breaking.ignore` lists both packages until GA, as `loams.postgres.v1` does), `docs/api/reasons.md` and `docs/api/route-map.md`. Tests in `crates/loams-proto/tests/control.rs`.

**Interfaces:** the services and messages of the shared contract, complete. `ModuleOptions` with `unstable: true`. The `Operation` kinds are listed in a proto comment. `loams.tenancy.v1` carries a file option comment "internal: never routed by the gateway", and the route map lists it under internal routes only.

Reasons (registered with codes):
- `org_not_found`, `namespace_not_found`, `cluster_not_found` (not_found);
- `namespace_not_active`, `erasure_pending`, `enrolment_used`, `enrolment_expired`, `release_incompatible`, `downgrade_refused`, `gate_failed` (failed_precondition);
- `tenant_cap_reached`, `tailnet_capacity` (resource_exhausted);
- `cluster_revoked` (permission_denied);
- `quota_exceeded` is reused (PG2 R1.5); its metadata gains `product`.

Tests:
- `buf lint` (STANDARD) and `buf breaking` (FILE, against `dev`) pass in `cp1.yml`.
- `every_mutation_has_idempotency_key`: as PG2 Task 1, over both packages.
- `no_secret_field_in_responses`: no response field of `ClusterService`, except `CreateEnrolmentResponse.token`, and no `AgentService` response field is named `*secret*`, `*password*` or `*private_key*`.
- `signed_messages_have_key_id_and_signature`: `DirectorySnapshot` and `SignedCommand`.
- `command_set_is_closed`: the `SignedCommand.command` oneof has exactly the five cases of the contract.
- `limits_sections_cover_products`: one `Limits` field per `Product` that enforces limits.
- `reasons_registered`.

Commit `feat(control): loams.control.v1 and loams.tenancy.v1 protos`.

### Task 2: `loams-ctlstore`, the fenced control store extracted from `pg-control`

**Files:** create `crates/loams-ctlstore/` (as in the file structure). Move the generic parts of `crates/loams-pg-control/src/store/{kv.rs,local.rs,tikv.rs,conformance.rs}` and `service/idempotency.rs` into it. `crates/loams-pg-control/src/store/mod.rs` keeps `PgControlStore`, `LEASE_SCOPE = "e/pg/"` and its `Record` impls, and re-exports the rest. Tests: `tests/{local.rs,tikv.rs,prefixes.rs}` (TiKV skipped when `LOAMS_TIKV_PD` is unset).

**Interfaces:**
- `ControlStore` (the trait `PgControlStore` is today, renamed; `PgControlStore` becomes a trait alias by blanket impl), `KvControlStore`, `ApiWriter`, `Batch`, `Fence`, `StoreError`, `Versioned`, `Page`, `StoreEvent`, `StoreOptions`.
- `Record { const PREFIX: &'static [u8]; type Key; … }`, as `pg-control`'s model has it.
- `control_store_conformance!($factory, $prefixes)`: the suite, parameterized by the caller's prefixes and lease scope.
- `IdempotencyLedger` (24 h TTL, fingerprint-checked; PG2 Task 0 ruling 3).
- `Secret<T>`, with `[redacted]` `Debug` and `Display`, and `SecretFile` (a read-only, mounted secret directory). The hub reads its CA and signing keys from it and never writes Kubernetes Secrets.
- `prefixes::REGISTERED: &[(&str, &[u8])]` and `prefixes::assert_disjoint()`.

Tests:
- `pg_control_suite_unchanged`: `cargo test -p loams-pg-control` passes with no test file changed. The PR diff under `crates/loams-pg-control/tests/` is empty, and CI checks it.
- `conformance_on_cp_prefix`: the suite on `cp/` and `e/cp/`, local and TiKV.
- `registered_prefixes_disjoint`: metastore (`N/ S/ L/ K/ w/ r/ q/ W/ e/m/ e/cluster/`), `pg-control` (`x/ X/ C/ E/ R/ D/ I/ O/ Q/ t/ e/pg/`), sqldb (`m/ mn/ M/ mr/`) and `cp/ e/cp/`.
- `secret_debug_is_redacted`, `secret_file_reads_mounted_key`.

Steps: write `pg_control_suite_unchanged` as the CI diff check first → move → PASS on both backends → commit `refactor(ctlstore): extract the fenced control store from pg-control`.

### Task 3: `loams-tenancy`, the contract every product plugs into

**Files:** create `crates/loams-tenancy/` (as in the file structure), and `deploy/openfga/{core.fga,postgres.fga,sqldb.fga,graph.fga,house.fga,live.fga,fga.mod}`. Tests are inline, plus `tests/{authz.rs,limits.rs,static_dir.rs}`.

**Interfaces:**
- The traits and types of the shared contract.
- `Limits` as a plain Rust struct generated from (or converted to) the proto, without depending on `loams-proto`. The conversion lives in `loams-control` (feature `proto` of `loams-tenancy`, off by default), so `fabric/` can use the crate without the proto stack.
- `merge(org: &Limits, ns: &Limits) -> Limits`: field-wise; a namespace field narrows (never widens) an org field.
- `DeploymentDefaults`: the defaults table, generated into `docs/api/limits.md` (D88). Every product's defaults come from its design: §46 §14, §47 §20, §48 §13.2, §49 §15, §45 §12.
- `StaticDirectory`: from a TOML file (`[[namespace]] id, org, name, products, [limits…]`). It is the single-node, desktop and air-gapped source. With no file, it holds org `local` and namespace `dev` (id 1), all products, and default limits.
- `RbacAuthorizer` with the relations of the OpenFGA model:
  - `deployment`: `operator`, `limits_writer`;
  - `org`: `admin`, `member`, `viewer`;
  - `namespace`: `admin`, `editor`, `viewer`, `connect`, inheriting from `org`;
  - `cluster`: `operator`, from `deployment`;
  - `rollout`: `operator`;
  - each product type: `parent: namespace` and `viewer`/`editor`/`admin`/`connect` inherited.
- `DevAuthorizer`, which refuses to start unless every listener is loopback (PG2 Task 0 ruling 4).
- The OpenFGA files: `core.fga` owns `deployment`, `org`, `namespace`, `cluster`, `rollout`; each product file adds its types with `parent: namespace`. If Task 0 finds that modular models are unsupported, one generated `model.fga` concatenates them, and a test checks that it is current.

Tests:
- `agent_cannot_be_operator`: binding `deployment#operator` or `deployment#limits_writer` to an `Agent` principal is refused by `RbacAuthorizer`'s binding writer, and `check` is false even if a binding is forced into the store.
- `namespace_limit_narrows_org_limit`, `unset_falls_back_to_default`, `last_known_survives_source_loss`.
- `kube_namespace_is_25_chars_and_unique` (proptest over ids).
- `static_directory_default_is_dev`, `static_directory_rejects_duplicate_name_in_org`, `same_name_two_orgs_allowed`.
- `require_active_refuses_provisioning_deleting_erased`.
- `defaults_table_rendered` (the generated `docs/api/limits.md` is current).
- `fga_model_validates` (`fga model validate` in CI, skipped locally without the binary).
- `enforcement_sample_field_names_pass_guard` (the field list, checked against Task 27's segment rule).

Commit `feat(tenancy): the tenancy contract for product control planes`.

### Task 4: Control records, keys, the outbox and the audit log

**Files:** `crates/loams-control/src/{model.rs,keys.rs,outbox.rs,audit.rs}`. Tests: `tests/model.rs`, `tests/outbox.rs`.

**Interfaces:**
- Records, postcard-encoded with a format byte, under `cp/`:
  - `OrgRec` `cp/o/<org>`, with the name index `cp/on/<name>`;
  - `NamespaceRec` `cp/n/<nsid BE>`, with the index `cp/nn/<org>/<name>`. It holds products, placement (cluster ids), egress policy, state, generation and the per-product `ProvisionStatus`;
  - `ClusterRec` `cp/c/<cl>`, `EnrolmentRec` `cp/ce/<enr>` (token hash only), `RevocationRec` `cp/cr/<serial>`;
  - `LimitsRec` `cp/l/o/<org>` and `cp/l/n/<nsid>`;
  - `ReleaseRec` `cp/r/<rel>`, `RolloutRec` `cp/ro/<ro>`, `GateResultRec` `cp/g/<ro>/<ring>/<gate>`;
  - `ErasureRec` `cp/x/<nsid>/<product>` (the receipt);
  - `EnforcementRec` `cp/es/<nsid>/<product>/<limit>` (latest sample only, overwritten);
  - `NamespaceIdSeq` `cp/seq/ns`.
- `NamespaceId` allocation: a fenced counter, never reused, starting above the metastore's highest existing id (Task 0 records it).
- `OutboxRow { seq: u64, kind, key, generation }` at `cp/ob/<seq BE>`, written in the same batch as the record; `processed` is a separate marker. The consumers are the Git writer, the directory builder and the OpenFGA tuple writer (D66).
- `AuditEvent { at, principal, act, action, object, outcome, request_id }`, emitted for every mutation through `AuditSink` (OTel logs; D221).

Tests:
- `record_roundtrip_and_format_byte`
- `names_unique_within_parent`, `same_namespace_name_two_orgs_distinct_ids`
- `namespace_ids_never_reused`
- `outbox_row_in_same_batch_as_record`: a fault after the batch leaves either both or neither.
- `outbox_is_ordered_and_idempotent`
- `enforcement_rec_keeps_only_latest`
- `audit_event_has_no_secret`

Commit `feat(control): control records, outbox and audit`.

## CP1b — The hub (Tasks 5–7)

### Task 5: The operations API, authorization, caps and the `loams control` role

**Files:** `crates/loams-control/src/{authz.rs,caps.rs,service/{mod.rs,orgs.rs,namespaces.rs,clusters.rs}}`. In `crates/loams/`: `Cargo.toml` (feature `control` = `dep:loams-control`), `src/main.rs` (subcommand `control`), `src/api/connect_control.rs`. Add `.github/workflows/cp1.yml`. Tests: `crates/loams-control/tests/{orgs.rs,namespaces.rs,clusters.rs}` and `crates/loams/tests/control_role.rs`.

**Interfaces:**
- `loams control --store tikv|local --listen <addr> --agent-listen <addr> --secrets <dir> [--authz openfga|rbac|dev]`. `--agent-listen` is the mTLS listener for `AgentService`, separate from the API listener.
- `OrgService`, `NamespaceService` and `ClusterService` with `IdempotencyLedger`.
- `CreateNamespace` writes `NamespaceRec{state: Provisioning}` and the outbox row, and returns an `Operation` that completes when Task 15's status shows every enabled product `Ready` on every placed cluster.
- Caps: per-principal token buckets on `CreateOrg` and `CreateNamespace`, a per-deployment org cap and `namespaces_per_org`. All are refused with `tenant_cap_reached`. If the store is unavailable, creation is refused (fail closed).
- Authorization through `Authorizer` on every call, against the object named. The operator view's calls need `deployment#operator`.
- Service credentials: mTLS plus an attenuated Biscuit when Q542 is answered that way; mTLS alone until then (Task 0 records it).
- gRPC reflection lists only served services (as PG2 R1.4).

Tests:
- `create_org_replay_returns_same_answer`, `create_namespace_replay_returns_same_operation`
- `unauthorized_is_denied_per_object`
- `agent_cannot_hold_operator` (through the API)
- `tenant_cap_and_rate_limit_enforced`
- `creation_fails_closed_when_store_unavailable`
- `every_mutation_emits_audit_event`, `audit_event_names_act_chain`
- `delete_namespace_needs_admin`
- `hub_has_no_kube_dependency`: `cargo tree -p loams-control -e normal` lists no `kube`, `k8s-openapi` or `kube-client`.
- `default_features_exclude_control`: `cargo tree -p loams -e normal` on default features lists none of `loams-control`, `loams-ctlstore`, `loams-gitops`, `loams-net`, `kube` or `rcgen`.
- `reflection_lists_only_served_services`

Commit `feat(control): the operations API and the control role`.

### Task 6: The limits API and enforcement state

**Files:** `crates/loams-control/src/service/limits.rs`. Tests: `tests/limits.rs`.

**Interfaces:**
- `PutLimits` (org or namespace scope) needs `deployment#limits_writer`. That relation is held by operator credentials, and on the hosted cloud by the private plan mapping's service credential (D546). Validation per section, with the product's own maximum where its design has one (`postgres.max_cu_per_endpoint ≤ 16`, `graph.concurrent_statements ≤ 64` by default, and so on). An out-of-range value is `invalid_argument` naming the field.
- A change bumps `limits_generation` and writes an outbox row, which the directory builder consumes.
- `GetLimits` returns the record and, with `effective: true`, the merged view with defaults filled in, marked per field `record | org | default`.
- `GetEnforcementState` returns the latest `EnforcementSample`s that agents reported, with `as_of_ms`. A sample older than 3 heartbeat intervals is returned with `stale: true`. The response's proto comment and the console say "enforcement state, not billing-grade".
- `EnforcementRec` keeps only the latest sample per `(namespace, product, limit)`. It is overwritten, never appended, and deleted with the namespace.

Tests:
- `limits_roundtrip`, `validation_names_field`
- `agent_cannot_write_limits`
- `namespace_cannot_widen_org_limit`
- `limits_change_reaches_directory_outbox`
- `enforcement_state_never_persisted_beyond_latest`: 100 reports store one record per key.
- `over_quota_writes_no_record`: a product's `quota_exceeded` adds no control-store key besides `cp/es/`.
- `stale_sample_flagged`

Commit `feat(control): limits API and enforcement state`.

### Task 7: The signed directory, its mirror and the `LimitsSource`

**Files:** `crates/loams-control/src/directory/{build.rs,sign.rs,scope.rs}` and `src/service/directory.rs` (internal reads for operators), plus `crates/loams-tenancy-client/` (`StoreDirectory`: reads the mirrored snapshot from the cluster's control store; `FileDirectory`: the air-gapped and test form). Tests: `crates/loams-control/tests/directory.rs` and `crates/loams-tenancy-client/tests/*.rs`.

**Interfaces:**
- `DirectoryBuilder` consumes the outbox and keeps, per cluster, the entries placed on it. It emits a full `DirectorySnapshot` on the first request and every 1,024 generations, and deltas (`ApplyDirectory{base_generation, upserts, removals}`) otherwise. Signing: Ed25519 over canonical bytes; `key_id` names the key in `SecretFile`. Two keys are valid during a rotation.
- The mirror key layout in a cluster's store: `cp/m/snap` (the last full snapshot) and `cp/m/delta/<gen BE>` (deltas since then), compacted on every full snapshot.
- `StoreDirectory::open(store, verifying_keys) -> impl Directory + LimitsSource`:
  - it verifies every snapshot and delta before applying it, and refuses a gap in generations (it asks for a full snapshot);
  - it watches the prefix and keeps an in-memory index by id and by `(org, name)`;
  - when the mirror is older than `directory.stale_after` (default 10 minutes), limits are `LimitsOrigin::LastKnown`, and requests still pass.
- Gateways' directory cache (§18 §5.2) uses the same reader.

Tests:
- `snapshot_is_deterministic_and_signed`
- `delta_gap_requests_full_snapshot`
- `unsigned_directory_refused`, `wrong_key_refused`, `rotation_accepts_both_keys`
- `cluster_scope_excludes_unplaced_namespaces`
- `limits_remain_when_hub_down`: stop the hub; products keep the last limits; the origin becomes `LastKnown`.
- `restart_without_hub_uses_mirrored_directory`: restart a reader with the hub down; it serves from `cp/m/`.
- `one_million_entries_snapshot_under_budget`: a size and time budget recorded in the rulings; the `bench` profile only.

Commit `feat(control): signed cluster-scoped directory and its mirror`.

## CP1c — The product control planes plug in (Tasks 8–12)

Each task here is small and has the same shape: admission through `Directory::require_active`, limits through `LimitsSource`, a `TenancyParticipant` implementation served on the product's cluster-local tenancy listener (`--tenancy-listen`, default `127.0.0.1:7695` in single-node mode), authorization through the shared `Authorizer` with the product's OpenFGA type, and one named test per row of the product-fit table. The product's existing interim (config limits, a local `Authorizer` trait) stays as the `StaticDirectory` path, so `loams dev` and the desktop are unchanged.

### Task 8: Postgres (`pg-control`)

**Files:** `crates/loams-pg-control/src/tenancy.rs`; modify `src/service/mod.rs` (admission), `src/model.rs` (keys), and PG2's `quota.rs` when it exists. Tests: `crates/loams-pg-control/tests/tenancy.rs`.

**Interfaces:**
- **Re-key (CP-R7).** `x/<ns>/…`, `X/…/n/…` and `Q/n/<ns>/…` take the `NamespaceId` in place of the name. A one-shot migration, `pg-control migrate-namespace-keys`, runs before PG2's GA and is idempotent. `GetConnectionInfo` and PgDog's routed names keep project and branch names (§46 §8.1), and the PgDog instance is per `NamespaceId`.
- `Caller` gains `principal: Principal` and `ns: Arc<DirEntry>`. `admin` is computed through `Authorizer::check(…, "admin", pg_project:<id>)`. This replaces PG2 Task 9's local trait; `--pg-authz` maps to `DevAuthorizer`, `RbacAuthorizer` or `OpenFgaAuthorizer`.
- Limits: the `postgres` section replaces PG2 Task 47's `PgLimits` record. `pg_total_cu` and the autoscaler's cap read `EffectiveLimits`.
- `TenancyParticipant`:
  - `provision` checks that `loams-pg-<nsid>` exists and that PgDog's Deployment there is available;
  - `erase` runs `DeleteProject` for every project of the namespace, waits for each erasure receipt (§46 §6), and returns one combined receipt;
  - `enforcement` reports projects, total CU and connections.
- `kube_namespace(Postgres, id)` replaces `KubeRuntime::namespace_for` (PG2 Task 13).

Tests:
- `inactive_namespace_refused_pg`
- `same_namespace_name_two_orgs_isolated_pg`
- `migrate_namespace_keys_is_idempotent`
- `limits_from_directory_cap_total_cu`
- `erase_deletes_every_project_and_returns_receipt`
- `enforcement_reports_projects_and_cu`
- `pg_runtime_uses_kube_namespace_helper`

Commit `feat(pg): plug pg-control into the tenancy contract`.

### Task 9: SQL (`loams-sqldb` and `loams-sqlgate`)

**Files:** `crates/loams-sqldb/src/tenancy.rs`, `crates/loams-sqlgate/src/tenancy.rs`; modify the gate's `ResolveUser`/`EnsureRunning` path and SQ1 Task 25's `quota.rs`. If SQ1's `store/` is not built yet, it is built on `loams-ctlstore` (record the ruling in both plans). Tests: `crates/loams-sqldb/tests/tenancy.rs`.

**Interfaces:**
- The gate refuses a connection to a non-`active` namespace with MySQL error 1045 and the message `namespace not active`, before any compute wake.
- Limits: the `sql` section replaces config quotas. `storage_bytes_per_database` turns the database read-only (§47 §20), and `max_class` caps `UpdateDatabase`.
- The sqldb Kubernetes driver (SQ1 Task 14) puts per-branch objects in `loams-sql-<nsid>` (CPQ6).
- `TenancyParticipant`:
  - `provision` checks the namespace exists and has its default-deny NetworkPolicy;
  - `erase` deletes every database through the delete saga (keyspace archived in PD, TiKV range deleted, BR prefix `sqlbackup/<ns>/` removed) and returns a receipt;
  - `enforcement` reports databases, branches, connections and storage per database.

Tests:
- `inactive_namespace_refused_sql` (the gate, before wake)
- `same_namespace_name_two_orgs_isolated_sql`
- `storage_limit_from_directory_turns_read_only`
- `erase_deletes_keyspaces_and_backups`
- `kube_driver_uses_namespace_helper`

Commit `feat(sqldb): plug loams-sqldb and the gate into the tenancy contract`.

### Task 10: Graph (`loams-graph`)

**Files:** `crates/loams-graph/src/tenancy.rs`; modify `src/limits.rs` (`NamespaceSlots` reads `concurrent_statements`) and GR1 Task 25's `quota.rs`. Tests: `crates/loams-graph/tests/tenancy.rs`.

**Interfaces:** admission on every `loams.graph.v1` call; the `graph` limits section; authorization type `graph`; `TenancyParticipant` (directory-only footprint; `erase` deletes graphs, their log streams and bucket prefixes; `enforcement` reports graphs, elements and stored bytes from the change-set counts GR1 Task 25 keeps).

Tests: `inactive_namespace_refused_graph`, `namespace_slots_follow_directory_limit`, `erase_removes_graphs_and_prefixes`, `enforcement_counts_from_change_sets`.

Commit `feat(graph): plug loams-graph into the tenancy contract`.

### Task 11: House (`loams-house`, `fabric/`)

**Files:** `fabric/crates/loams-house/src/tenancy.rs`; `fabric/crates/loams-house/Cargo.toml` (a path dependency on `../../crates/loams-tenancy`, CPQ9); HS1 Task 22's `QuotaSource` gains a `LimitsSource` implementation. Tests: `fabric/crates/loams-house/tests/tenancy.rs`.

**Interfaces:**
- Admission at the front, before a worker is bound.
- The `house` limits section; `202` for rate and concurrency, `house_quota_exceeded` for volume (§49 §15).
- Authorization types `house_database` and `house_table`.
- `TenancyParticipant`: `erase` drops the namespace's lake tables, `_house.query_log` partitions and snapshots (past retention is not kept for an erased namespace); `enforcement` reports concurrent queries and scanned bytes per day.
- **The adapter emits no events.** §49 §15's per-query events stay where HS1 puts them, pending CPQ4. The adapter never reads or forwards them.

Tests: `inactive_namespace_refused_house`, `quota_source_from_limits`, `erase_drops_lake_tables_and_query_log`, `house_tenancy_adapter_emits_no_events` (a compile-time field list and a search of the module for the event names).

Commit `feat(house): plug loams-house into the tenancy contract`.

### Task 12: Live (`loams-live`)

**Files:** `crates/loams-live/src/tenancy.rs`; modify the app directory (`apps/<id>` gains `namespace_id`) and LV1 Task 28's limits source. Tests: `crates/loams-live/tests/tenancy.rs`.

**Interfaces:** `CreateApp` requires an `active` namespace. The `live` limits section feeds the per-node token buckets (§45 §12; still per node, documented). Authorization type `live_app`. `TenancyParticipant` (directory-only; `erase` deletes apps and their keyspaces; `enforcement` reports apps, documents and storage).

Tests: `inactive_namespace_refused_live`, `app_directory_carries_namespace_id`, `limits_from_directory_feed_buckets`, `erase_deletes_app_keyspaces`.

Commit `feat(live): plug Live into the tenancy contract`.

## CP1d — Tenants as Git commits (Tasks 13–17)

### Task 13: The tenants-repository renderer

**Files:** `crates/loams-gitops/src/{lib.rs,layout.rs,render.rs,emit.rs}`. Tests: `tests/golden/`, `tests/render.rs`.

**Interfaces:**
- `render_org(org, namespaces, clusters, limits) -> BTreeMap<PathBuf, Vec<u8>>`, with the layout of §41 §6:
  - `tenants/<org>/{org.yaml,clusters.yaml,kustomization.yaml}`;
  - `namespaces/<nsid>.yaml` (file names use the id, not the name).
- A namespace gets a file only if it has a Kubernetes footprint (D544): an enabled product whose `Participation.footprint` is `KubeNamespace`, or functions.
- The file holds a `LoamsNamespace` custom resource: `spec.{namespaceId, org, name, products[], limits.kube, egress, generation}`.
- Per product, the operator renders (Task 15), not the hub:
  - the Kubernetes namespace;
  - NetworkPolicy default-deny, plus the product's allowed flows;
  - `ResourceQuota` and `LimitRange` from `limits.kube`;
  - for `pg` and `sql`, the **runtime-object Role**: Pods, Services and Secrets in that namespace only, bound to the product controller's ServiceAccount (D707, D736).
- `emit.rs`: a canonical YAML emitter (sorted keys, fixed quoting, LF, trailing newline).

Tests:
- `render_is_deterministic` (golden files)
- `render_golden_org_with_pg_sql_graph`
- `directory_only_namespace_has_no_file`
- `kube_namespace_names_never_collide` (two orgs, same namespace name)
- `file_names_use_namespace_id`
- `runtime_role_scoped_to_product_namespace` (golden Role)
- `no_secret_in_rendered_files` (gitleaks over the goldens)

Commit `feat(gitops): render the tenants repository`.

### Task 14: The Git writer

**Files:** `crates/loams-gitops/src/{writer.rs,remote.rs,sign.rs}`, and `crates/loams-control/src/outbox.rs` (the consumer). Tests: `tests/writer.rs` (a bare repository) and `tests/it_forgejo.rs` (ignored by default; runs in `cp1-e2e.yml`).

**Interfaces:**
- `GitRemote` trait (`gix` or a subprocess, per Task 0): `fetch`, `commit(tree, message, signer)`, `push(branch)`, `open_pull_request(branch, title, body)`, `merged(pr)`.
- `GitWriter::run(store, remote, policy)`:
  - it drains the outbox in order, renders the touched orgs, and commits to `tenants/<org>` changes on a work branch;
  - it merges directly (`policy = self_service`) or opens a pull request (`policy = reviewed`);
  - it signs commits with the bot key (SSH by default, Q556), and marks rows processed only after the push or the pull request is recorded.
- Drift: on every run it re-renders from the store and compares. A difference that no outbox row explains is overwritten and reported as a `Drift` audit event naming the paths.
- `rebuild(store) -> tree`: the whole repository from the store.

Tests:
- `commit_per_outbox_batch`
- `rebuild_from_store_matches_repo`
- `manual_edit_overwritten_and_reported_as_drift`
- `reviewed_policy_opens_pr_never_pushes_main`
- `commits_are_signed`
- `push_failure_leaves_rows_unprocessed`
- `it_forgejo_pr_flow` (ignored by default)

Commit `feat(gitops): the tenants-repository writer`.

### Task 15: The `ApplicationSet` and the operator's namespace reconcile

**Files:** `deploy/gitops/appset-tenants.yaml`; `crates/loams-operator/src/{lib.rs,namespace.rs,participants.rs,rbac.rs}` (CP-R10). Tests: `crates/loams-operator/tests/{namespace.rs,golden/}` and `tests/it_k3d.rs` (ignored by default).

**Interfaces:**
- `ApplicationSet`: a Git directory generator over `tenants/*` and a cluster generator matrixed with `clusters.yaml`. Sync is automated with `prune: true` and `selfHeal: true`, `PrunePropagationPolicy=foreground`, and the resources finalizer (MT4 Ruling 4).
- The `LoamsNamespace` controller:
  - it renders and applies the per-product Kubernetes objects of Task 13;
  - it calls `TenancyParticipant.Provision` on each enabled product's cluster-local Service (`loams-<product>-tenancy.loams-system:7695`, mTLS from the cluster CA);
  - it writes the status conditions `Ready`, `Degraded`, `Drift`, `ErasurePending` and `ProductReady/<product>`, with the observed generation.
- The agent (Task 19) reports these conditions. The hub's `status.rs` flips the namespace to `active`, through the outbox and the directory, only when every enabled product is `Ready` on every placed cluster.

Tests:
- `objects_golden_per_product`
- `provision_called_once_per_generation`
- `not_ready_until_all_products_ready`
- `unowned_object_drift_reported_not_fixed`
- `namespace_appears_only_through_argo` (it_k3d: the hub process holds no kubeconfig; the namespace exists only after Argo CD syncs)

Commit `feat(operator): reconcile LoamsNamespace through the products`.

### Task 16: Offboarding and cross-product erasure

**Files:** `crates/loams-control/src/erasure.rs`, `crates/loams-operator/src/erasure.rs`. Tests: `crates/loams-control/tests/erasure.rs`, `crates/loams-operator/tests/erasure.rs`.

**Interfaces:**
- `DeleteNamespace` sets `Deleting`. The directory removes the namespace from routing, so gateways and products refuse it with `namespace_not_active`.
- The operator calls `Erase` on every enabled product until each returns `Done(receipt)`. The agent forwards the receipts, and the hub stores them as `ErasureRec`.
- Only when every enabled product's receipt is stored does the hub remove the namespace file from Git. Argo CD prunes, and the operator deletes the Kubernetes namespaces only if the `LoamsNamespace` carries every receipt (`ErasurePending` until then). The directory entry becomes `Erased`, and is removed after 30 days.
- A directory-only product's receipt is required the same way.
- An accidental removal of `tenants/<org>/` without receipts deletes nothing: the operator refuses, and reports `ErasurePending`.

Tests:
- `kube_namespace_kept_until_all_receipts`
- `directory_entry_kept_until_all_receipts`
- `missing_product_receipt_blocks_offboarding`
- `manual_git_removal_deletes_nothing`
- `erase_is_idempotent_by_request_id`
- `receipt_has_no_data` (field list)

Commit `feat(control): offboarding with cross-product erasure`.

### Task 17: The onboarding e2e

**Files:** `.github/workflows/cp1-e2e.yml` (job `onboard`), `scripts/cp1/onboard-e2e.sh`. Tests: `crates/loams-control/tests/e2e_onboard.rs` (ignored by default).

**Contract:** k3d, Forgejo, Argo CD, the operator, the agent, `loams control` and `pg-control`, with the sqldb controller where SQ1 has merged. Create an org and a namespace with `products = [postgres, sql, graph]` through the API, then see:
- the commit;
- the `Application`;
- `loams-pg-<nsid>` and `loams-sql-<nsid>`, each with NetworkPolicy and ResourceQuota;
- the products `Ready`, and the directory entry `active`;
- `CreateProject` succeeds.

Delete the namespace and see receipts from all three products before the Kubernetes namespaces go.

Tests: `e2e_onboard_org_with_products`, `e2e_offboard_with_receipts`.

Commit `test(control): onboarding and offboarding end to end`. **CP1d exit:** both e2e tests pass in CI.

## CP1e — BYOC and private networking (Tasks 18–23)

### Task 18: The cluster CA, enrolment and revocation (hub side)

**Files:** `crates/loams-control/src/{ca.rs,enrol.rs,service/agent.rs}`. Tests: `tests/enrol.rs`.

**Interfaces:**
- `CreateEnrolment` returns a token once: 32 random bytes, base64url, single use, valid 1 hour. Only its SHA-256 is stored.
- `Enrol(token, csr)` issues a client certificate for `cluster_id`: ECDSA P-256 from the CA key in `SecretFile`, valid 24 hours, with the SAN URI `loams://cluster/<cl>`.
- `Renew` renews at 12 hours remaining. `RevokeCluster` writes `RevocationRec`; the revocation list goes out on the next command stream and is checked on every `AgentService` call.
- Every `AgentService` call is scoped to the certificate's `cluster_id`: reports for orgs not placed on it are dropped and logged.

Tests:
- `enrolment_token_single_use`, `enrolment_token_expires_after_one_hour`, `token_never_stored_or_logged`
- `cert_scoped_to_cluster`: a cluster cannot report for, or read, another cluster's namespaces.
- `cert_renews_before_expiry`
- `revoked_cert_refused_within_one_heartbeat`

Commit `feat(byoc): cluster CA, enrolment and revocation`.

### Task 19: The agent

**Files:** `crates/loams-byoc-agent/src/{main.rs,enrol.rs,heartbeat.rs,commands.rs,mirror.rs,observe_only.rs}`. Tests: `crates/loams-byoc-agent/tests/*.rs`.

**Interfaces:**
- Outbound only: `Enrol`, then `Report` (every 15 s) and `Commands` over mTLS HTTP/2 (D361, Q549).
- `AgentReport` holds:
  - versions and Argo CD application conditions;
  - `LoamsNamespace` conditions;
  - erasure receipts;
  - `GetEnforcement` samples from each product (approximate);
  - gate Job results (Task 25);
  - product health.
  It never holds data (§41 §7.3).
- Commands: `Pause`, `ResumeRollout`, `ReportBundle` (customer-allowed scopes only), `RotateCertificate`, and `ApplyDirectory`, which verifies the signature and generation and writes to the mirror (Task 7). Anything else is refused and logged.
- Observe-only mode refuses all commands except `ApplyDirectory`, which a cluster needs to serve, and `RotateCertificate`. The customer applies rollouts by merging pull requests.
- The agent opens no listening socket.

Tests:
- `agent_has_no_inbound_listener` (inspects the process's sockets in the test)
- `unknown_command_refused`, `unsigned_directory_refused`, `expired_command_refused`
- `observe_only_refuses_rollout_commands`
- `report_has_no_data_fields` (field list)
- `heartbeat_survives_hub_restart`

Commit `feat(byoc): the outbound-only cluster agent`.

### Task 20: Air-gapped mode and the bundle

**Files:** `crates/loams-byoc-agent/src/bundle.rs`, `crates/loams/src/cli/control.rs` (`loams cluster bundle`). Tests: `tests/bundle.rs`.

**Interfaces:**
- `loams cluster bundle --cluster <cl> --out <file>` writes a tarball holding the release repository at the channel head, the tenants files placed on the cluster, a signed `DirectorySnapshot` and a manifest with digests.
- `loams-byoc-agent import-bundle <file>` (run by the customer, no network) verifies the bundle, writes the mirror and leaves the Git trees for the customer's mirror. With no agent connection, the cluster shows as `disconnected` (§41 §7.1).

Tests: `airgap_bundle_roundtrip`, `tampered_bundle_refused`, `import_writes_mirror`, `disconnected_cluster_status`.

Commit `feat(byoc): air-gapped bundles`.

### Task 21: `loams-net`, the provider seam, the checker and the tenant-section renderer

**Files:** `crates/loams-net/src/{lib.rs,provider.rs,tailscale.rs,headscale.rs,checker.rs,policy.rs}`. Tests: `crates/loams-net/tests/*.rs` (wiremock), and `tests/golden/` (NET1 Task 2's goldens, moved or shared from `deploy/tailnet/policy/golden/`).

**Interfaces** (§43 §6.3, §11):
- The trait:
  ```rust
  #[async_trait]
  pub trait NetProvider: Send + Sync {
      async fn issue_join_key(&self, tenant: &TenantTag, kind: KeyKind) -> Result<Secret<JoinKey>>; // single use, tagged, ≤1 h
      async fn revoke_tenant(&self, tenant: &TenantTag) -> Result<u32>;                                 // nodes deleted
      async fn list_nodes(&self, tenant: Option<&TenantTag>) -> Result<Vec<NodeInfo>>;
      async fn apply_policy(&self, rendered: &str, checker: &dyn PolicyChecker) -> Result<PolicyApplied>;
  }
  ```
- `TailscaleClient`: API v2, an OAuth client with the scopes `auth_keys`, `devices` and `policy_file` only; `POST …/keys` with tags, `reusable: false` and a short expiry; ACL with ETag, `acl/validate` first.
- `HeadscaleClient`: `/api/v1`, a bearer API key from `SecretFile`, reachable only on the tailnet or loopback (Q591). In `file` mode `apply_policy` commits through `GitRemote` to the policy repository; in `database` mode it calls the API's policy write (Q589).
- `PolicyChecker`: `TailscaleApiChecker` (validate), and `HeadscaleCliChecker`, which runs NET1 Task 1's `check.sh` (seeded scratch database) as a subprocess with argument arrays.
- `policy::render_tenant_section(base, tenants, support_grants, now) -> String`, between NET1's markers:
  - per tenant, its tag, a grant to `tag:control` tcp 443, and unexpired support grants;
  - `deny` tests for every ordered tenant pair, and for each operator tag;
  - deterministic output.
- The provider is chosen by `tailnet.provider` (`tailscale` by default) and `tailnet.login_server` (D582).

Tests:
- `join_key_is_single_use_tagged_and_short` (both providers)
- `reusable_requires_explicit_kind_and_max_24h`
- `apply_refuses_failing_policy`: nothing is committed or posted.
- `tailscale_apply_validates_then_posts_with_etag`, `tailscale_oauth_scopes_are_minimal`
- `headscale_file_mode_commits_never_puts`
- `secrets_never_logged`
- `golden_two_tenants`, `golden_no_tenants`, `expired_support_grant_dropped`, `render_is_idempotent` (NET1 Task 2's names)
- `tenant_cannot_reach_other_tenant` (through `check.sh` with 3 tenants; CI `net` job)
- `tenant_tags_unique_per_org`
- `no_billing_names` (the Task 27 guard over the crate)

Commit `feat(net): NetProvider, both clients, checker and the tenant renderer`.

### Task 22: BYOC tailnet mode

**Files:** `crates/loams-control/src/net.rs`, `crates/loams-byoc-agent/src/tailnet.rs`, `deploy/helm/loams-byoc-agent/{values.yaml,templates/tailscaled.yaml}`. Tests: `crates/loams-control/tests/net.rs`, `crates/loams-byoc-agent/tests/tailnet.rs`, and `helm template` goldens.

**Interfaces:**
- `UpdateCluster{net.mode = TAILNET}` re-renders the tenant section and applies it through `NetProvider`. `CreateEnrolment` on such a cluster also returns a single-use join key (`tag:byoc-<org>`, 1 hour), once.
- Capacity: past `tailnet.max_tagged_nodes` (default 45, under the Personal plan's 50; Q593) enrolment in tailnet mode is refused with `tailnet_capacity`.
- The chart's values follow §43 §6.4: `net.mode`, `net.provider`, `net.loginServer` and `net.authKeySecret`. There is a userspace `tailscaled` sidecar, with no `NET_ADMIN`. `--login-server` is set only when `loginServer` is set, and `--advertise-tags` only when it is not (§43 §6.5).
- The agent prefers the hub's tailnet name and falls back to the public name (Q590 default: both).
- Support grant (Q588): the customer sets `net.supportGrant.enabled` and a TTL (default 4 hours) in their values. The agent reports the request. The hub renders the grant with its expiry, and a reconciler removes it at expiry.
- Offboarding the last cluster of an org revokes the tag's nodes and removes the section. Revoking a cluster deletes its node.

Tests:
- `tailnet_enrolment_returns_key_once`
- `capacity_refused_past_max`
- `byoc_chart_tailscale_has_no_login_server_flag`, `byoc_chart_headscale_sets_login_server`, `chart_without_tailnet_has_no_sidecar`
- `support_grant_expires_and_is_removed`
- `revoke_cluster_deletes_node`
- `byoc_node_reaches_control_only` (CI `net` job: Headscale and two `tailscale/tailscale` userspace nodes of two tenants; each reaches `tag:control:443` and not the other)

Commit `feat(byoc): tailnet connectivity mode`.

### Task 23: Inventory diff and audit events

**Files:** `crates/loams-net/src/inventory.rs`; a nightly job in `crates/loams-control`. Tests: `crates/loams-net/tests/inventory.rs`.

**Interfaces:** `list_nodes` compared with the last snapshot (`cp/ni/snap`). Findings:
- a new node;
- a new tag on an existing node;
- a server without tags;
- a tagged node unseen for 30 days;
- a node whose expiry is later than policy allows;
- a `tag:byoc-*` node with no matching cluster.

Each finding is an `AuditEvent` (§43 §8.3; D221). The snapshot is replaced atomically.

Tests (NET1 Task 6's names): `new_untagged_server_alerts`, `new_tag_alerts`, `stale_tagged_node_alerts`, `no_change_is_silent`, `snapshot_is_atomic`, `audit_event_has_no_key_material`, plus `orphan_byoc_node_alerts`.

Commit `feat(net): nightly inventory diff`. **CP1e exit:** a second k3d cluster enrols as BYOC (outbound mode, then tailnet mode on Headscale), mirrors the directory, onboards a namespace and survives a hub outage with limits enforced.

## CP1f — Releases and rings (Tasks 24–25)

### Task 24: Releases that carry every product

**Files:** `release/manifest.toml`, `crates/loams-control/src/service/releases.rs`. Tests: `tests/releases.rs`.

**Interfaces:**
- `release/manifest.toml`:
  - `version`, `chart_version`, `compatible_with` (the oldest version it runs beside);
  - `[components.<name>]` with `image@sha256`, read from each product's pin file (`release/sqldb-images.toml`, `deploy/pins/loams-postgres-images.toml` after NF1 Task 1b, and the House, Graph and Live entries as their plans add them);
  - `no_downgrade = [...]`: TiKV, PD, the metastore's on-disk format, the pageserver and `loams-wal` formats, and Live's journal;
  - `[migrations.<product>]` with expand and contract step ids.
- `PublishRelease` validates the manifest. It refuses a contract step whose expand step has not been in `stable` for one full ring cycle, and refuses an image without a digest.
- `Promote` and `Rollback` return `Operation`s. A rollback is a revert of the channel branch; for a `no_downgrade` component it is refused with `downgrade_refused`.

Tests:
- `release_manifest_collects_product_pins`
- `image_without_digest_refused`
- `contract_step_requires_expand_in_stable_for_one_cycle`
- `no_downgrade_refused`
- `skew_beyond_compatible_with_refused`

Commit `feat(control): releases carry every product`.

### Task 25: Channels, rings, gates and promotion

**Files:** `crates/loams-control/src/{rings.rs,gates.rs}`, `deploy/gitops/appset-rings/`, `deploy/gitops/gates/*.yaml`. Tests: `tests/rings.rs`.

**Interfaces:**
- Channels `canary`, `early` and `stable`, one release-repository branch each.
- Rings 0–2 and the BYOC window ring (§41 §8.1). Clusters are labelled `loams.dev/ring`. There is one `ApplicationSet` per ring (the fallback, Q559), unless Task 0 found `RollingSync` stable.
- Gates are Argo CD PostSync Jobs in the release. Their results reach the hub through the agent as `GateResultRec`s:
  - `argo-healthy`, `e2e-smoke`, `conformance-collections` (D60, D63);
  - `pg-smoke` (`CreateProject` → `StartEndpoint` → `SELECT 1` through PgDog);
  - `sql-smoke` (`CreateDatabase` → a query through the gate);
  - `graph-smoke`, `house-smoke`, `live-smoke`;
  - `error-budget` (the cluster's Prometheus).
  A product gate runs only on clusters where the product is enabled.
- Promotion fast-forwards the next channel only after the previous ring's gates pass and its soak time has run. A gate failure stops promotion and opens an incident audit event.
- An agent's automatic `Pause` on a failed health check stops its ring (Q548). Ring 2 goes in batches of at most 10%. A BYOC cluster syncs only inside its window.

Tests:
- `failed_gate_blocks_promotion`, `product_gate_skipped_where_disabled`
- `soak_time_enforced`
- `pause_stops_ring`
- `rollback_is_a_revert`
- `ring_batches_at_most_ten_percent`
- `byoc_ring_syncs_only_in_window`

Commit `feat(control): channels, rings and product gates`.

## CP1g — Operator view, guard, deployment, security and exit (Tasks 26–30)

### Task 26: The operator view and the CLI

**Files:** `web/apps/console/src/pages/operator/*`, `crates/loams/src/cli/control.rs` (or the home Task 0 recorded). Tests: console component tests and CLI golden tests.

**Interfaces:**
- Console pages: orgs and namespaces (state, products, per-product readiness), clusters and enrolments (the token and the join key are shown once), rollouts and rings, enforcement state labelled "enforcement state, not billing-grade", and erasure receipts.
- The pages require `deployment#operator`. Q546's resource dashboards are not in money.
- CLI:
  - `loams org create|list|delete`;
  - `loams namespace create|list|delete|status`;
  - `loams cluster create|enrol|list|revoke|bundle`;
  - `loams limits get|put`;
  - `loams rollout publish|promote|pause|resume|rollback|status`;
  - `loams net render-policy` (CP-R9).

Tests:
- component tests (empty, loading, error, token shown once)
- `operator_view_has_no_money_strings`: a snapshot test that no string matches `/\b(plan|price|invoice|billing|credit)\b/i`
- `operator_view_requires_operator_relation`
- CLI golden outputs

Commit `feat(console): operator view and control-plane CLI`.

### Task 27: The no-metering guard, extended to field names

**Files:** `scripts/ci/no-metering.py`, `scripts/ci/test-no-metering.py`, `CONTRIBUTING.md` (a short section). Tests: the script's self-tests.

**Contract:** keep T8-1 to T8-5 as built. Add a field-segment rule:
- **Scope:** `.proto` files under `proto/loams/{control,tenancy}`, and Rust `pub` items and serde field names in `crates/loams-{control,ctlstore,tenancy,tenancy-client,gitops,operator,byoc-agent,net}`, plus each product's `tenancy.rs`.
- **Rule:** fail when a snake_case or camelCase identifier has a segment equal to `plan`, `price`, `invoice`, `credit`, `billable`, `meter` or `usage`.
- The guard prints only the path and the identifier, never file contents (T8 rules).

Tests:
- `guard_rejects_billing_segments` (fixture `billable_seconds`, `usageBytes`)
- `guard_allows_explain_and_placement`
- `guard_scope_covers_product_tenancy_adapters`
- `no_rpc_spends_or_reports_money`: a descriptor walk over `loams.control.v1` and `loams.tenancy.v1`.

Commit `ci: the no-metering guard checks field names`.

### Task 28: Helm charts, observability and alerts

**Files:** `deploy/helm/loams-control/` (hub Deployment ×2, the agent listener Service, `SecretFile` mounts, NetworkPolicy) and `deploy/helm/loams-byoc-agent/` (the agent, the operator's `LoamsNamespace` controller, the CRD, RBAC, and the tailnet sidecar of Task 22); `deploy/observability/loams-control/{dashboards/control.json,alerts.yaml}`; `crates/loams-control/src/metrics.rs`. Tests: `helm template` goldens, `kubeconform`, `promtool test rules`.

**Interfaces:**
- Metrics:
  - `loams_control_{requests_total,outbox_lag_seconds,git_push_seconds,directory_generation,clusters_connected,agent_heartbeat_age_seconds,gate_results_total,erasure_pending}`;
  - `loams_agent_{mirror_generation,mirror_age_seconds,commands_total}`.
  None of them is billing-grade (D547).
- Alerts:
  - outbox lag above 5 minutes;
  - a heartbeat older than 3 intervals;
  - mirror age above `directory.stale_after`;
  - `erasure_pending` older than 24 hours;
  - a gate failure;
  - a CA, or a signing key, expiring within 14 days;
  - an inventory diff finding.

Tests:
- `helm_template_golden_hub`, `helm_template_golden_agent_outbound`, `helm_template_golden_agent_tailnet`
- `kubeconform_passes`
- `agent_rbac_is_namespace_scoped` (the operator's ClusterRole manages only `loams-*` namespaces and the CRD)
- `promtool_rules_pass`
- `it_k3d_install_from_empty_argocd`, `it_k3d_install_from_empty_flux`

Commit `deploy(control): charts, dashboards and alerts`.

### Task 29: Security review and chaos

**Files:** `docs/security/loams-control-threat-model.md`, `scripts/cp1/chaos.sh`. Tests: the chaos scenarios in `cp1-e2e.yml`.

**Contract:** a threat model covering §41 §11 and §43 §8.6, with a test for each mitigation it claims. Chaos scenarios:
- the hub killed during 100 `CreateNamespace` calls: no orphan record, Git file or Kubernetes namespace;
- the hub down for 30 minutes: the data path serves, and limits are enforced;
- a stolen agent certificate, used after revocation;
- a forged `ApplyDirectory`;
- a malicious commit to the tenants repository: an unsigned commit is refused by Argo CD, and a signed commit removing a namespace without receipts deletes nothing;
- Headscale down: established tailnet connections keep working.

An external review is the owner's action (Q557).

Tests: `kill_hub_during_100_creates_leaves_no_orphans`, `hub_outage_data_path_unaffected`, `revoked_cert_replay_refused`, `forged_apply_directory_refused`, `unsigned_commit_not_synced`, `signed_removal_without_receipts_deletes_nothing`, `headscale_down_keeps_connections`.

Commit `docs(security): control-plane threat model and chaos`.

### Task 30: The exit gate, docs, runbooks and status

**Files:**
- `.github/workflows/cp1-e2e.yml` (job `exit`);
- `docs/runbooks/loams-control/{enrol.md,offboard.md,rollout.md,hub-outage.md,tailnet.md,rotate-keys.md}`;
- user docs (orgs, namespaces, limits, BYOC);
- `docs/plans/README.md` (a CP1 row; MT4 and NET1 pointer notes);
- §41 §14 and §43 §12 as built.

**Contract:** D558, extended to products, as one CI job:
- create an org and a namespace with Postgres and SQL, and see the commit, the `Application`, the product namespaces and `active`;
- enrol a second k3d cluster as BYOC, outbound and then tailnet, and onboard a namespace there;
- promote a release through ring 0 to ring 1, fail `pg-smoke` on purpose, and see promotion stop;
- exceed a Postgres and a Graph limit, receive `quota_exceeded`, and see no new key outside `cp/es/`;
- offboard with receipts from every product;
- run the guard.

Each runbook step is run once on k3d and marked verified.

Tests: `exit_gate` green; `cargo deny check`; `buf breaking`; `docs_links_resolve`.

Commit `docs(control): CP1 exit gate, runbooks and status`.

---

## Exit criteria for production (§41 §14 and D558, with the owning tasks)

- [ ] **API** served, linted and breaking-checked, with idempotency replay tests, reasons and route-map rows: Tasks 1, 5, 6.
- [ ] **Control store** shared with `pg-control`, conformance-tested on both backends, prefixes disjoint: Task 2.
- [ ] **Tenancy contract** implemented by Postgres, SQL, Graph, House and Live; inactive namespaces refused; cross-org isolation tested: Tasks 3, 8–12.
- [ ] **Directory** signed, cluster-scoped and mirrored; the data path survives a hub outage: Tasks 7, 19, 29.
- [ ] **Onboarding** through Git only, with every product's footprint, `active` only when every product is ready: Tasks 13–15, 17.
- [ ] **Offboarding** blocked until every product's erasure receipt: Task 16.
- [ ] **BYOC:** enrolment, the agent, observe-only and air-gapped modes, revocation within one heartbeat: Tasks 18–20.
- [ ] **Private networking:** both providers behind `NetProvider`; tenant isolation tested; join keys single-use; support grants expire; inventory audited: Tasks 21–23.
- [ ] **Releases and rings:** product pins, `compatibleWith`, `noDowngrade`, product gates, pause and rollback: Tasks 24, 25.
- [ ] **Limits and enforcement state:** every product section enforced; agents cannot write; nothing billing-grade: Tasks 6, 8–12, 27.
- [ ] **Operations:** operator view, CLI, charts from empty through Argo CD and Flux, dashboards and alerts: Tasks 26, 28.
- [ ] **Security:** threat model with tests, chaos green, external review closed for high and critical (Q557): Task 29.
- [ ] **Docs:** runbooks verified, user docs, §41 and §43 as built: Task 30.

## Self-review

- **Spec coverage.**

  | Section | Task(s) |
  |---|---|
  | §41 §4 The control plane | 1, 4, 5 |
  | §41 §5 Tenancy | 3, 8–13, 15 |
  | §41 §6 Onboarding through Git | 13–17 |
  | §41 §7 BYOC (§7.1–§7.3) | 7, 18–20 |
  | §41 §7.4 and §43 §6 Tailnet mode | 21, 22 |
  | §41 §8 Upgrade waves | 24, 25 |
  | §41 §9 Quotas without metering | 6, 7, 8–12 |
  | §41 §10 Observability boundary | 11, 27, 28 |
  | §41 §11 Security | 5, 18, 19, 29 |
  | §41 §12 Console and dependency direction | 26, 27 |
  | §43 §5 Tags and join keys, §8.2 | 21, 22 |
  | §43 §6.3 `NetProvider`, §11 tooling | 21 |
  | §43 §8.3 Audit | 23 |
  | §18 §5.2 Directory, §9 Erasure | 7, 16 |
  | §46 §14–§15, §47 §16/§20, §48 §13.2, §49 §15, §45 §12 | 8, 9, 10, 11, 12 |

- **Types.** `ControlStore`, `Directory`, `LimitsSource`, `Authorizer`, `TenancyParticipant`, `ErasureReceipt`, `NetProvider` and the naming grammar are defined once, in the shared contracts.
- **Review Focus.** Items 1–10 each name owning tests (Tasks 2, 5–12, 13, 15, 16, 18, 19, 21, 22, 24, 25, 27).
- **Not in this plan:** NET1 Tasks 0, 1, 3, 4 and 7 (the policy as code, roster sync, templates, guide, Headscale e2e); MT1's OpenFGA server and Authentik wiring; MT2's Knative tenancy; `loams-meta-remote` (BYOC-managed-meta, Q551); anything in `loams-platform`.

## Open questions

New questions are labelled CPQ1 to CPQ10 until the integrator numbers them in the decision log. Carried questions keep their numbers.

| # | Question | Default | Owner | Needed by |
|---|---|---|---|---|
| CPQ1 | **Namespace identity.** Key every product's namespace-owned records, and name Kubernetes namespaces, by `NamespaceId` (`loams-<p>-<nsid>`), amending §41's `loams-ns-<namespace>` and §46's `loams-pg-<ns>` (CP-R6, CP-R7)? This re-keys `pg-control`'s `x/<ns>/` and `Q/n/<ns>/` before GA | Yes | Owner + PG2, SQ1 | Before Task 8; before PG2 Task 13 ships |
| CPQ2 | Extract `pg-control`'s store into `loams-ctlstore` and share it (CP-R4), or copy it | Extract, timed with PG2 | Eng + PG2 | Task 2 |
| CPQ3 | Every cluster runs the agent, and the agent is the only hub client, mirroring the directory locally (CP-R5); `SetLimits` becomes `ApplyDirectory` | Yes | Eng | Task 7 |
| CPQ4 | **§49 §15 and D776 have the House emit per-query events "through §27's hook interface"** (HS1 Task 22), but D548 moved that contract to `loams-platform`. Should the House use a private link-time observer seam like `pg-control`'s `ComputeLifecycleObserver`? | Yes; HS1 Task 22 keeps quotas and drops the open event path | Owner | Before HS1 Task 22 |
| CPQ5 | No `loams-operator` (the D185 fork) exists. Create the crate with only the namespace controller (CP-R10), or wait for MT2 | Create now | Eng | Task 15 |
| CPQ6 | SQL runtime objects in a per-namespace `loams-sql-<nsid>` (default), or one shared `loams-sql` namespace with labels (§47 §16 does not say) | Per namespace | Eng + SQ1 | Before SQ1 Task 14 |
| CPQ7 | Tailnet tag per org (`tag:byoc-<org>`, as §43) with per-cluster hostnames, and `tailnet.max_tagged_nodes = 45` on the Personal plan | Yes | Founder | Task 22 |
| CPQ8 | One renderer for the tenant policy section, in Rust (CP-R9), with NET1 Task 2's Python dropped; or both, held to shared goldens | Rust only | Eng | Task 21 |
| CPQ9 | The `fabric/` workspace takes a path dependency on `crates/loams-tenancy` (types only, no proto) | Yes | Eng + HS1 | Task 11 |
| CPQ10 | The hub as a role of `loams` (CP-R3), or its own binary and image per D542's crate list | Role | Eng | Task 5 |
| Q544 | Licence of the control plane: Apache-2.0 or source-available. **Owner decision** | Apache-2.0 | Founder | Before Task 5 merges |
| Q556 | Commit signing for the tenants and release repositories | SSH, verified by Argo CD | Eng | Task 14 |
| Q559 | Rings through `RollingSync` or one `ApplicationSet` per ring | One per ring | Eng | Task 0 / 25 |
| Q542 | Service credentials: mTLS plus an attenuated Biscuit, or mTLS only | mTLS + Biscuit | Eng | Task 5 |
| Q557 | External security review before GA. **Owner action** (budget) | — | Founder | Task 29 |
| Q588 | Who enables a support grant, its TTL, and session recording | Customer, 4 h, no recording | Founder | Task 22 |
| Q590 | In tailnet mode, does the agent use only the tailnet or both paths | Both, tailnet preferred | Eng | Task 22 |
| Q591 | Headscale's unscoped API keys: accept 90-day keys reachable only on the tailnet or loopback | Accept | Eng | Task 21 |
| Q592 | `loams-net` in this repository | Yes | Founder | Task 21 |
| Q593 | Tailscale plan: Personal (non-commercial; 50 tagged resources) until customers depend on it | Personal, then paid or Headscale | Founder | Before the first paying BYOC tenant |

**Decisions the owner must make before the tasks that need them:** CPQ1 and CPQ4 (they change PG2 and HS1 now), Q544 (before Task 5 merges), Q557 (before GA), and CPQ7 with Q593 (before tailnet-mode BYOC carries a paying tenant).

## Rulings made during execution

(Task 0 and later tasks append here.)
