# 65 — Loams Operator: One Rust Operator for the Whole Platform, and Loams Operator Conformance

Status: **Approved** · 2026-10-19 (owner approved all decisions and questions with their defaults, 2026-10-19). Source: the owner's request of 2026-10-19, **"There is a TiDB Rust Kubernetes operator. Fork it and write a plan for the Loams operator: a single operator for the entire Loams platform, with HA. Only design it; we can build it slowly. It should have a conformance suite that inspires all other operators."** Decisions **D1172–D1201** and questions **Q984–Q999** are in the [decision log](13-decision-log.md), all approved by the owner on 2026-10-19 with their recommended defaults. The numbers follow §64 (D1150–D1171, Q970–Q983). Plan stub: [OP1](../plans/2026-10-19-op1-loams-operator.md). **This document writes no code.**

Builds on:
- [§10](10-operations.md) §1 and §7: deployment modes, the `loams` binary's roles, the M2 operator that was never built (D46).
- [§20](20-reactive-database-on-tikv.md) and [§25](25-clever-cloud-stack.md) §5–§6: PD and TiKV unmodified (D126), TiKV through tidb-operator v2 (D179), RustFS and the `ObjectStoreProvider` trait (D178), the planned fork of `clever-kubernetes-operator` (D185), Argo CD sync waves (D186).
- [§38](38-knative-authentik-gitops.md) and [§41](41-multitenant-byoc-control-plane.md), with plan [CP1](../plans/2026-10-10-cp1-control-plane-byoc.md): one Kubernetes namespace per Loams namespace (D443), GitOps as the only write path (D544), upgrade rings and `compatibleWith` (D545), the BYOC agent's closed command set (D543), the `LoamsNamespace` controller (CP-R10).
- [§46](46-loams-postgres-production.md) §9.5 and §11–§15, [§51](51-loams-postgres-fork.md): Loams Postgres HA, `loams-wal` in place of safekeepers, upgrades and the Kubernetes layout (D715–D718).
- [§47](47-loams-sql-production.md) §16: Loams SQL's `kube-rs` controller for per-branch TiDB computes (D736, Q664).
- [§49](49-loams-house-production.md) §11, [§45](45-loams-live-production.md) §9–§10, [§48](48-loams-graph-production.md) §14: House, Live and Graph HA.
- [§54](54-upgrades.md): data-keeping upgrades, the `migrate` service, N/N−1 formats, the Kubernetes part (D943).
- [§57](57-loams-gateway.md) §9: the gateway on Kubernetes (D1019).
- [§43](43-private-networking.md) and the [networking map](networking-map.md): the tailnet, and the rule that the engine is never on the public internet (amended 2026-10-21 by D1311: off-tailnet callers reach Loams only through the gateway's edge listener, §69).
- [§55](55-commit-path-determinism.md), [§56](56-loams-sandbox.md): `loams-detsim`, the sandbox.
- [§64](64-loams-infra-stack.md): Loams Secrets, Config and Catalog (D1155–D1161).

Markers, as in §62 and §64:
- **(verify)** means the claim has not been checked against a primary source. The task that depends on it checks it first.
- **(estimate)** means the number was judged, not measured.
- **(unverified)** means a search did not settle it.

---

## 1. Summary

### 1.1 The request and what it means

Three things were asked for:

1. **Fork the Rust TiDB operator.** A search on 2026-10-19 found no Kubernetes operator for TiDB, TiKV or PD written in Rust that Loams could fork (§2). The owner's conditions (Rust, operates TiDB/TiKV/PD, Apache-2.0 or MIT) are not met by any repository, so **nothing was forked**. The one Rust project that touches TiDB on Kubernetes is an AGPL-3.0 SurrealDB controller that creates PingCAP's own `TidbCluster` objects and leaves the operating to PingCAP's Go operator. The design therefore takes PingCAP's **Go tidb-operator v2** (Apache-2.0) as the behavioural reference for PD and TiKV, and **kube-rs** (Apache-2.0) as the Rust foundation.
2. **One operator for the whole platform, with HA.** Today the Kubernetes story is a set of plans that each bring their own piece: a fork of Clever Cloud's operator that was never made (D185), tidb-operator v2 for PD and TiKV (D179), a `LoamsNamespace` controller (CP-R10), a `LoamsPostgres` CRD "after GA" (D718), Loams SQL's own controller (D736) and Argo CD waves for ordering (D186). This design replaces them with **one Rust operator, `loams-operator`**, that installs, upgrades, scales, backs up and heals every Loams component from one `LoamsCluster` object, and is highly available itself.
3. **A conformance suite that inspires other operators.** **Loams Operator Conformance (LOC)** is a versioned spec of required operator behaviours with IDs and MUST/SHOULD levels, a harness on kind and k3d with fault injection, upgrade and skew matrices, backup round trips, a small adapter trait so it can test any operator, a machine-readable report and a badge (§12).

### 1.2 The recommendation in one paragraph

Build **`loams-operator`** in Rust on kube-rs as one binary with one Helm chart and one API group, `loams.dev`. A **`LoamsCluster`** object describes the whole platform and owns typed component objects (engine, PD, TiKV, Loams Postgres, Loams SQL, gateway, House, Durable, agentd and Studio, object storage); quorum members get one `LoamsInstance` each, as tidb-operator v2 does, so the operator can drain, replace or remove a specific member. The operator runs two or three replicas with a Lease, keeps every multi-step change in a **journal in the object's status**, makes every step idempotent and checked against observed reality, and **fences** disruptive actions with a per-group disruption Lease taken by compare-and-swap, so a paused or stale leader cannot act twice. It is never on the data path: if it stops, the platform keeps serving. PD and TiKV are first reached through the existing tidb-operator v2 (a **bridge driver**), then through **native Rust reconcilers** ported from tidb-operator v2's logic, which become the default only when they pass the same LOC HA profile as the bridge. Per-tenant objects (Postgres computes, per-branch TiDB computes) stay with `pg-control` and `loams-sqldb`, which share the operator's runtime crate. LOC ships with the operator, runs in Loams' CI at three levels (Core, HA, Day-2), and is published so that any operator can run it through an adapter.

### 1.3 What changes

| Before | After |
|---|---|
| Fork `CleverCloud/clever-kubernetes-operator` as `loams-operator` (D185, never done) | No fork. A new crate `loams-operator` on kube-rs; Clever's operator and kube-rs' `controller-rs` template are references only (D1173) |
| tidb-operator v2 deploys PD and TiKV (D179, D736) | A `bridge` driver keeps it for OP1; a `native` driver replaces it from OP2 after LOC parity (D1182) |
| Argo CD sync waves order components (D186, D455) | The operator orders components inside one cluster; Argo CD applies only CRDs, the operator and the `LoamsCluster` (D1184) |
| `LoamsNamespace` controller in a small `loams-operator` crate (CP-R10) | The same controller, one of the operator's controllers (D1177) |
| `LoamsPostgres` CRD "after GA" (D718) | A component kind owned by `LoamsCluster` (D1176) |
| Operator API group unresolved (Q-RT-8) | `loams.dev`, starting at `v1alpha1` (D1174) |

## 2. The upstream search (2026-10-19)

### 2.1 What was searched

- GitHub repository search for "tidb operator", "tikv operator", "tidb-operator", "tidb kubernetes", "tikv kubernetes operator", with and without `language:rust`.
- GitHub code search for Rust files naming `TidbCluster`, `TikvCluster` or `pingcap.com` CRD groups next to `kube::`.
- crates.io search for "tidb operator" and "tikv kubernetes".
- Two web searches for a Rust or kube-rs rewrite of tidb-operator.

### 2.2 What exists

| Repository | Language | Licence | Activity | What it is | Verdict |
|---|---|---|---|---|---|
| [pingcap/tidb-operator](https://github.com/pingcap/tidb-operator) | Go (controller-runtime) | Apache-2.0 | Pushed 2026-10-15; v2.0.0 GA 2025-12-18, v2.0.1 2026-03-26 (latest stable v2), v2.2.0-alpha.11 2026-09-16; v1.6.6 2026-08-12 | PingCAP's operator. `main` is v2; `release-1.x` is v1 | **The behavioural reference and porting source** for PD and TiKV (§13). Not forked: it is Go, and the owner asked for Rust |
| [rustfs/operator](https://github.com/rustfs/operator) | Rust (kube-rs) | Apache-2.0 | 0.0.8 on 2026-10-03, "under active development" | RustFS's own operator: one `Tenant` kind (`rustfs.com/v1alpha1`) rendering StatefulSets | Not a TiDB operator. A candidate for the RustFS part only (Q999) |
| [tikv/tikv-operator](https://github.com/tikv/tikv-operator) | Go | Apache-2.0 | **Archived**, last push 2022-06-28 | An early TiKV-only operator | Not used |
| [aexol-studio/sdb-controller](https://github.com/aexol-studio/sdb-controller) | Rust (kube-rs, from the `controller-rs` template) | **AGPL-3.0** | Created 2025-08-07, last commit 2025-08-21, 0 stars | A SurrealDB controller. It carries a `tidb-api` crate of PingCAP's v1 types and creates `TidbCluster` objects for PingCAP's Go operator to run; it does not operate PD or TiKV itself | **Excluded twice:** AGPL (D1023's rule) and not a TiKV operator |
| [kube-rs/kopium](https://github.com/kube-rs/kopium) | Rust | Apache-2.0 | Active | Generates Rust types from CRD schemas; the code-search hit for `TidbCluster` is a test in its analyzer | A tool: OP1's bridge driver generates its `core.pingcap.com` types with it |
| crates.io | — | — | — | No crate operates TiDB, TiKV or PD on Kubernetes. Hits were SQL clients and `tikv-client` | — |

Nothing meets all three of the owner's conditions, so **no repository was forked or cloned**, as the owner's conditions require. The belief that a Rust TiDB operator exists may come from TiKV itself (Rust), from SurrealDB's TiKV-backed controllers, or from RustFS's new Rust operator (unverified guess); none of them operates TiKV or PD.

### 2.3 The fork policy instead (D1173)

The repository has two fork precedents. Neon became a **hard fork** with no upstream sync (§51), because upstream went quiet. Grafeo became a **fork with a patch queue** (`LG-NNNN` rows, each upstreamable), because upstream is alive and Loams only needs fixes. Neither fits here: there is no Rust code to fork, and tidb-operator is a Go program whose value to Loams is the **knowledge in its PD and TiKV controllers**, not its code base.

So the policy is a **port with provenance and upstream tracking by review**:
- `loams-operator` is new Rust code. Where a PD or TiKV procedure is ported from tidb-operator v2 (Apache-2.0), the Rust module says so in a header (`Ported from pingcap/tidb-operator@<sha>:<path>, Apache-2.0`), the repository's `NOTICE` names PingCAP, and the port is a translation of behaviour, not a line-for-line copy.
- **Upstream is tracked, not merged.** PD and TiKV keep changing, so a quarterly "upstream watch" task diffs tidb-operator's PD and TiKV controllers and its release notes since the last recorded sha, and files an issue per behaviour change (a new PD API, a changed eviction rule). This is closer to Grafeo's live upstream than to Neon's frozen one, without a fork to maintain.
- **`clever-kubernetes-operator` is not forked** (supersedes D185). Its reusable core is about 1,500 lines (§25 §7), the fork was never created, and keeping its MIT notice for a skeleton the team will rewrite buys nothing. Its ideas, and those of kube-rs' `controller-rs` template (Apache-2.0), are references.

## 3. Principles

1. **One operator, one chart, one API group.** Every Loams component on Kubernetes is installed and changed by `loams-operator`. No component needs a second operator from Loams; third-party operators are allowed only where Loams uses an unmodified third-party database (CloudNativePG for the storage controller's and Lakekeeper's Postgres, D413, D718), and §16 lists them.
2. **The operator is never on the data path.** It writes Kubernetes objects and calls admin APIs. If every replica stops, the platform keeps serving; only changes wait.
3. **Reality wins over records.** Status is a cache of what the operator last saw and did. Each reconcile re-reads the cluster (Pods, PD's store states, `pg-control`'s view) before acting, so a lost or stale status can delay work but never cause a wrong action.
4. **One disruptive step at a time per quorum group,** fenced, journaled and idempotent (§7).
5. **Platform objects here, tenant objects in the product control planes.** The operator runs `pg-control`, `loams-sqldb` and the rest; those control planes create per-tenant computes on their own fast paths (cold start in seconds), using the operator's runtime crate and its conventions (D1178).
6. **Nothing public by default, the engine never public.** Exposure is `tailnet` or `internal`; the operator refuses to render a public Service, `NodePort` or Ingress for any engine, admin or data port (§11.4).
7. **Conformance is part of the product.** Every behaviour this document promises has a LOC requirement ID; a promise without a test is not made.

## 4. Scope: what the operator manages

| Component | Workload | State | HA shape | Tenant objects owned by |
|---|---|---|---|---|
| **Engine** (`loams`, roles `meta`, `log`, `query`, `worker`, `gateway` per §10 §1) | Deployment per role; `meta` as `LoamsInstance`s only with the `raft` metastore | Bucket; metastore (TiKV or Raft) | Stateless roles ≥ 2 across zones; `meta` 3 or 5 voters | — |
| **`migrate`** (§54) | Job before each engine upgrade | Ledger in the metastore | Runs once per upgrade, retried idempotently | — |
| **PD** | `LoamsInstance` per member | PVC (etcd data) | 3 or 5 members across zones | — |
| **TiKV** (API v2, `enable-ttl`, D122) | `LoamsInstance` per store | PVC (RocksDB, Raft log) | ≥ 3 stores, 3 replicas per region across zones | Keyspaces: the engine, Live, `loams-sqldb` |
| **Loams Postgres** (§46) | `pg-control` Deployment ×2; storage controller ×2; storage broker ×2; pageservers and `loams-wal` acceptors as `LoamsInstance`s; endpoint storage | Bucket; pageserver NVMe; `loams-wal` journals | As §46 §11 | Computes, PgDog, endpoint Services: `pg-control` |
| **Loams SQL** (§47) | `loams-sqlgate` pool Deployment; `loams-sqldb` Deployment ×2 | TiKV keyspaces | Gate ≥ 2; control plane ×2 with leases | Per-branch TiDB computes, BR tasks: `loams-sqldb` (D736) |
| **Gateway** (`loams-gateway`, §57) | Deployment ×2 | None | Two replicas across zones, PDB `minAvailable: 1` (D1019) | Routes: `pg-control` |
| **House** (`loams-fabric` house, §49) | Front Deployment ≥ 2; workers; Lakekeeper ≥ 2 | Bucket; Lakekeeper's Postgres | As §49 §11 | — |
| **Durable** (Resonate on TiKV) | Deployment + HPA | TiKV | ≥ 2 | — |
| **agentd** (remote, one principal each, D921) | One `LoamsAgentd` → single-replica StatefulSet | PVC + checkpoints in the bucket | Not HA by design (D921); restart and reattach | — |
| **Studio console** (static) | Deployment ×2 (optional; the hosted console is the default) | None | Two replicas | — |
| **Object storage** | `ObjectStore`: BYO S3 (reference + probe) or RustFS (operator-rendered) | The bucket | The provider's; RustFS distributed mode (verify) | Buckets and keys: `ObjectStoreProvider` (D178) |
| **Tenancy** | `LoamsNamespace` (CP1 Task 15) | — | — | Namespace, NetworkPolicy, ResourceQuota, LimitRange |

Out of scope: TiFlash, TiCDC, TiProxy, DM and the TiDB dashboard (Loams runs none of them); Knative's own components (the Knative Operator stays, D446); CloudNativePG itself.

## 5. The API (D1174–D1177)

### 5.1 Group and versions

- **Group `loams.dev`**, the project's domain. It answers Q-RT-8. All kinds are **namespaced**; the operator lives in `loams-system`.
- **Versions** move `v1alpha1` → `v1beta1` → `v1`. From `v1beta1`, no field is removed or changes meaning; new fields are optional with defaults. A version bump ships with a conversion webhook served by every operator replica, and with round-trip tests (`v1beta1 → v1 → v1beta1` is lossless) in LOC's API area.
- **Schemas come from the Rust types** (`#[derive(CustomResource, JsonSchema)]`). A `crdgen` binary writes them to `deploy/helm/loams-operator/crds/`, and a drift test fails CI when the checked-in CRDs differ. Validation is in the schema: CEL rules (`x-kubernetes-validations`) for immutable fields (storage class, TiKV `api-version`, the metastore backend), minimum members (PD ≥ 1, and odd), and exposure (§11.4).

### 5.2 Kinds

| Kind | Purpose | Created by |
|---|---|---|
| `LoamsCluster` | The whole platform: release, object store, zones, exposure, secrets provider, TLS, and one sub-spec per component (`enabled`, replicas, resources, storage, `config` overlay, `podTemplate` overlay) | The user or GitOps |
| `ObjectStore` | A bucket and credentials from a provider (`rustfs`, `s3`, `cellar`; D178), with a capability probe | `LoamsCluster` or the user |
| `LoamsEngine` | The `loams` roles, the metastore backend and the `migrate` Job | `LoamsCluster` |
| `LoamsPD`, `LoamsTiKV` | The PD and TiKV groups; `spec.driver: native \| bridge \| external` (§9) | `LoamsCluster` |
| `LoamsPostgres` | Storage controller, broker, pageservers, `loams-wal`, endpoint storage, `pg-control` (D718's CRD) | `LoamsCluster` |
| `LoamsSQL` | `loams-sqlgate`, `loams-sqldb`, the TiDB image pin for computes | `LoamsCluster` |
| `LoamsGateway`, `LoamsHouse`, `LoamsDurable`, `LoamsStudio` | One each | `LoamsCluster` |
| `LoamsAgentd` | One remote agentd for one principal | The user or Studio |
| `LoamsInstance` | One quorum member (PD, TiKV, `meta`, `loams-wal` acceptor, pageserver): its Pod, PVCs, identity in the group and member state | The group's controller |
| `LoamsNamespace` | A Loams namespace's Kubernetes footprint (CP1) | GitOps (D544) |
| `LoamsBackup`, `LoamsBackupSchedule`, `LoamsRestore` | Backups and restores (§8.9) | The user or a schedule |
| `LoamsOperation` | A day-2 request: restart, evict leaders, transfer the PD leader, replace an instance, rotate certificates, pause, upgrade plan (§10.4) | The user, the CLI or the BYOC agent |

Component objects are named `<cluster>-<component>`, carry an owner reference to the `LoamsCluster`, and are **written only by the operator** when owned (a validating webhook refuses other writers to owned objects' `spec`, except through `LoamsCluster`). Each component kind can also be created on its own, for users who want only TiKV or only Loams Postgres.

### 5.3 A sketch

```yaml
apiVersion: loams.dev/v1alpha1
kind: LoamsCluster
metadata: { name: prod, namespace: loams-system }
spec:
  release: 0.3.0                  # resolves to a pinned component set (§8.8)
  channel: stable                 # D545
  zones: [eu-west-1a, eu-west-1b, eu-west-1c]
  objectStore: { ref: prod-bucket }
  exposure: { mode: tailnet }     # tailnet | internal; never public (§11.4)
  secrets: { provider: kubernetes }  # kubernetes | loams (§11.3)
  tls: { issuer: operator-ca }    # operator-ca | cert-manager
  components:
    pd:       { members: 3, storage: { size: 20Gi, class: nvme } }
    tikv:     { members: 3, driver: bridge, storage: { size: 1Ti, class: nvme } }
    engine:   { metastore: tikv, roles: { query: 3, log: 3, worker: 2 } }
    postgres: { enabled: true, wal: { mode: acceptors, members: 3 }, pageservers: 3 }
    sql:      { enabled: true, gate: 2 }
    gateway:  { replicas: 2 }
    house:    { enabled: false }
    durable:  { enabled: true }
  maintenance: { window: "Sun 02:00-04:00", timezone: UTC }
```

### 5.4 The per-member model (D1175)

tidb-operator v2 manages each PD, TiKV and TiDB member as its own object and Pod rather than through a StatefulSet (§13). Loams takes the same model for quorum members, because a StatefulSet can only remove its highest ordinal, while a safe scale-in removes the member PD chose to drain, and a failed store must be replaced by identity. Stateless roles stay Deployments.

`LoamsInstance` holds: the group, the member's stable identity (PD name, TiKV store ID, acceptor ID), its zone, the Pod's template hash, its PVCs, and its member state (`Up`, `Offline`, `Tombstone`, `Joining`, `Removed`). PVCs carry an owner reference to the instance, not the Pod, so deleting a Pod never deletes data; a PVC is deleted only after the member is `Tombstone` or `Removed` and the group's `pvcRetention` allows it.

### 5.5 Who may write what (D1177)

- The **operator** writes component objects, instances, workloads and status.
- **Users and GitOps** write `LoamsCluster`, standalone component objects, `LoamsNamespace`, backups, restores and operations.
- **The BYOC agent** (D543) may create only `LoamsOperation` objects of the closed types its command set already has (`Pause`, `ResumeRollout`, `Report`, `RotateCertificate`, `SetLimits`), never edit specs.
- **Tenants** have no Kubernetes RBAC on any Loams kind; they use Loams' APIs.

## 6. Inside the operator (D1178, D1179)

### 6.1 Crates

| Crate | Content | Users |
|---|---|---|
| `loams-operator-api` | CRD types, defaults, validation, conversion; no runtime | The operator, the `loams` CLI, LOC's Loams adapter, generated SDK types |
| `loams-kube` | The shared runtime: leader election and fencing (§7), the operation journal, server-side apply helpers, conditions and reasons, events, metrics, the drift detector | `loams-operator`, `pg-control`'s `ComputeRuntime` (Kubernetes driver), `loams-sqldb`'s controller |
| `loams-operator` | Controllers, drivers (PD/TiKV `native`, `bridge`, `external`), webhooks, the `crdgen` binary | — |
| `loc-spec`, `loc-harness`, `loc-adapter` | Loams Operator Conformance (§12) | Anyone |

None of them enters the `loams` binary's default build. `kube` and `k8s-openapi` are already workspace dependencies: `kube` 3.1.0 is pinned on `dev`, CP1 names 4.2.0, and 4.2.0 (2026-07-22, Apache-2.0) is the newest release on crates.io. OP0 moves the workspace to one version (D1179).

**Leader election is Loams' own code in `loams-kube`.** kube-rs has none built in (issue [#485](https://github.com/kube-rs/kube/issues/485) is open). Two MIT crates exist, `kube-leader-election` 0.44.0 and `kube-lease-manager` 0.12.0, but the fences of §7.2 need the Lease's transition count and the renewal clock exposed to every write, so a small implementation (estimate: 300–500 lines) that follows client-go's algorithm is simpler than wrapping either. They are references.

### 6.2 Controllers

One kube-rs `Controller` per kind, each watching its owned objects. The runtime already guarantees one reconcile per object at a time; across objects, a **disruption Lease per quorum group** (§7.3) serialises anything that takes a member down.

Each reconcile follows the same shape:
1. **Observe.** Read the object, its owned objects, and the component's own view (PD's members and stores, `pg-control`'s storage view, the engine's `GetInstance`).
2. **Decide.** Compute the desired objects and the next step of any running operation from the journal (§6.3) and the observation. When the two disagree, observation wins.
3. **Act once.** Apply owned objects with server-side apply under the field manager `loams-operator`, or take **at most one disruptive step**, then requeue.
4. **Report.** Write status and conditions with `observedGeneration`, and emit events.

### 6.3 The operation journal

Every multi-step change (a rolling upgrade, a scale-in, a restore, a certificate rotation) is an **operation** in the component's `status.operation`:

```yaml
status:
  operation:
    id: op-7f3c                # also the idempotency key sent to admin APIs
    kind: RollingUpgrade
    target: { version: v8.5.9 }
    plan: [tikv-2, tikv-0, tikv-1]   # fixed when the operation starts
    step: { member: tikv-0, phase: EvictingLeaders, since: "2026-10-19T08:10:00Z" }
    done: [tikv-2]
    fence: 14                  # the Lease transition count that wrote this step (§7.2)
```

- **Steps are idempotent and checked.** "Add the evict-leader scheduler for store 5" first reads PD's schedulers; "delete the Pod" first checks that its image is not already the target. Re-running any step after a crash is safe.
- **Phases are derived, not trusted.** If the journal says `EvictingLeaders` but PD reports store 5 with no leaders and the Pod already runs the target image, the reconcile records the later phase and moves on.
- **The plan is fixed at the start** and only shrinks, so a new leader finishes the same plan rather than inventing a new order mid-upgrade.
- **A changed spec mid-operation** (a new target version) does not restart the plan: the current member finishes its step, the operation ends `Superseded`, and a new one starts from the observed state.

### 6.4 Drift

Owned objects are watched. A change made by someone else to a field the operator owns is **reverted** at the next reconcile with a `Drift` event and the metric `loams_operator_drift_corrections_total{kind}`. Fields the operator does not own (an annotation another controller adds, a label from a policy engine) are left alone, which server-side apply's field ownership gives for free. The annotation `loams.dev/paused: "true"` on any Loams object stops all writes to it and its children and sets the `Paused` condition; it is the documented way to hand-edit in an emergency.

## 7. HA of the operator (D1179–D1181)

### 7.1 Replicas and leader election

- **Two replicas by default, three in multi-zone clusters,** with a PDB of `minAvailable: 1` and anti-affinity across nodes and zones.
- **Leader election on a `coordination.k8s.io/v1` Lease** `loams-operator-leader` in `loams-system`, with client-go's timings: lease 15 s, renew deadline 10 s, retry 2 s. Only the leader runs reconcilers. All replicas keep their caches warm, serve `/metrics`, and serve the webhooks (conversion, validation, the eviction gate of §8.2), which are stateless.
- **Losing the lease stops work.** A leader that cannot renew by its renew deadline (measured on a monotonic clock) stops issuing writes at once, cancels in-flight reconciles, and exits so that a clean process takes its place. This is client-go's `ReleaseOnCancel` behaviour plus a self-fence.

### 7.2 Why a lease is not enough, and the fences

A leader can be paused (GC, a stopped node, `SIGSTOP`) past its lease, wake up and act on stale beliefs. Kubernetes leases do not fence writes by themselves. Loams adds three fences:

1. **Optimistic concurrency on the journal.** A step is advanced only by an update to the component's status that carries the `resourceVersion` the leader read. A stale leader's write fails with `409 Conflict`, and it must re-observe.
2. **A fencing token.** The Lease's `leaseTransitions` is a counter that rises with each new holder. The leader records it in `status.operation.fence` when it advances a step, and refuses to act on an operation whose fence is higher than its own. Calls to Loams' own control planes (`pg-control`, `loams-sqldb`, the engine's admin API) carry the operation ID and the fence; they reject a lower fence for the same operation, as MetaStore leases already do with epochs.
3. **A disruption Lease per quorum group.** Before taking any member of a group down (deleting a TiKV Pod, removing a PD member, restarting an acceptor), the leader takes a second Lease, `loams-<cluster>-<group>-disruption`, by a create or an update with `resourceVersion`, naming the operation and the member. Two leaders cannot both hold it, whatever their clocks say, so **two members of one group are never taken down at once** even in a split brain. It is released when the member is back and healthy, and expires after a long timeout (default 30 minutes) only with a `Warning` event.

PD's HTTP API has no fencing. Its calls are made idempotent (read the scheduler or member list first, act only if needed) and are always preceded by the disruption Lease.

### 7.3 Crash safety mid-upgrade

A crash or leader change at any point of an upgrade leaves one of these states, each of which the next leader handles from observation:

| Crash point | What the next leader sees | What it does |
|---|---|---|
| After adding the evict-leader scheduler | Scheduler present, leaders draining | Continues waiting for the drain |
| After deleting the Pod | Pod absent or starting with the target image | Waits for the store to be `Up` and caught up |
| After the store came back, before removing the scheduler | Store `Up`, scheduler present, `done` lacks the member | Removes the scheduler, records the member done |
| Holding the disruption Lease, leader dead | Lease held by the old operation | Takes it over for the same operation ID (same plan), never a new one |
| Between two members | Disruption Lease free | Takes it for the next member in the plan |

LOC's `HA-UPG` requirements kill the leader at each of these boundaries (§12.4).

### 7.4 API server unavailable

The operator does nothing without the API server: it cannot renew, so it stops, and the managed platform keeps serving because nothing in its data path calls the API server (pods are already running; TiKV and PD elect among themselves). When the API server returns, a leader is elected, caches resync, and running operations resume from their journals. Rate limits and exponential backoff keep a recovering operator from storming the API server (`kube`'s default backoff, with a cap of 5 minutes).

### 7.5 Upgrading the operator itself

- An operator upgrade **restarts no managed Pod** unless its release notes say why. Pod templates are rendered with stable defaults; a change in rendering that would roll Pods ships behind the next `release` change, not the operator's. LOC checks this (`LOC-UPG-003`).
- The operator supports the managed release it ships with and the one before (N and N−1, §8.8), and refuses to manage a `LoamsCluster` on a newer release than it knows (`Ready=False`, reason `ReleaseUnknown`).
- CRDs are upgraded before the operator (Argo CD wave −2, or `helm upgrade` with the chart's `crds/` hook), and the storage version only moves after every stored object has been rewritten (`kube-storage-version-migrator` or the operator's own migrator).

## 8. HA of the managed platform (D1182–D1190)

### 8.1 Topology

- `LoamsCluster.spec.zones` lists the zones. Every quorum group uses `topologySpreadConstraints` over `topology.kubernetes.io/zone` with `maxSkew: 1` and `whenUnsatisfiable: DoNotSchedule`, plus anti-affinity on `kubernetes.io/hostname`. Stateless roles use `ScheduleAnyway`.
- **PD and TiKV learn the topology.** TiKV stores get `server.labels` `{zone, host}` from the node, and PD gets `replication.location-labels = ["zone", "host"]` and `isolation-level = "zone"` when there are at least three zones, so PD never puts two replicas of a region in one zone.
- With fewer than three zones the cluster is `Degraded` with reason `ZonesBelowQuorum`, not refused: a single-zone dev or BYOC cluster is legitimate.

### 8.2 PDBs and node drains

- **PDBs** for every component (tidb-operator v2 appears to leave PDBs to the user; Loams creates them): quorum groups `maxUnavailable: 1`; stateless roles `maxUnavailable: 1` or `minAvailable: 1` for two-replica sets; PgDog and the gate keep §46's `maxUnavailable: 0` during their own rollouts and `1` otherwise.
- **The eviction gate** (D1185). A PDB cannot say "only after the store's leaders have moved". The operator serves a validating webhook on `pods/eviction` for PD, TiKV, `meta` and acceptor Pods: an eviction is denied with a retryable message ("draining leaders from store 5") until the operator has evicted the store's leaders (or transferred the PD leader) and taken the disruption Lease, then allowed. `kubectl drain` and cluster autoscalers retry denied evictions (verify the retry behaviour for webhook denials, which differs from PDB's `429`). The webhook's `failurePolicy` is `Ignore`, so a dead operator falls back to the PDB alone, which is still safe for Raft, just slower for clients. Cordoned nodes are also watched, and leader eviction starts as soon as a node with a TiKV store is cordoned.

### 8.3 PD

- **Members:** 3 or 5, odd only (CEL rule). PD microservice mode (TSO and scheduling groups) is not used.
- **Rolling upgrade:** followers first, one at a time; before the PD leader's turn, transfer leadership to an upgraded follower through PD's API and wait for the new leader to serve; then restart the old leader. After each member: the member list is healthy and the cluster has a leader for 30 s.
- **Scale-out:** a new instance joins the existing members (`--join`). **Scale-in:** remove the member through PD's API, then delete the Pod; never below the quorum of the current membership. A member is never removed while another is unhealthy.
- **Failure:** a PD Pod that does not come back is replaced with the same identity on its PVC; if the PVC is lost, the member is removed and a new one joins. With a majority lost, the operator stops and reports `QuorumLost`: PD's own recovery (`pd-recover`) is a documented manual procedure, never automatic.

### 8.4 TiKV

- **Rolling upgrade, per store,** following tidb-operator v2's `TaskPod` and `TaskEvictLeader` (§13): take the disruption Lease → add PD's evict-leader scheduler for the store → wait until three things hold: the store has no leaders, no region has a down peer, and the TiDB and `tikv-client` region-cache TTL has passed since the eviction started (so clients stop routing to the store) → delete the Pod with a grace period that grows with the store's region count (v2: 200 regions per second, at least 30 s) → the new Pod starts with the target image on the same PVC → wait until the store is `Up` and has been ready for a minimum time before it takes leaders back → remove the scheduler → release the Lease. If the three conditions do not hold within 10 minutes (estimate), the upgrade stops with `UpgradeBlocked` rather than restarting a store that still leads regions. Stores are upgraded one zone at a time.
- **Scale-out:** new instances join; PD rebalances. **Scale-in** follows v2's two-step store deletion: first a reversible `offline: true` on the instance (leaders evicted with a 5-minute timeout, then PD moves the regions), then, once the store is `Removed` or `Tombstone`, the instance, its Pod and (by policy) its PVC are deleted. Scale-in is refused when the stores still serving would be at or below PD's `max-replicas`, or above 80 % of their capacity (estimate). A store is never taken offline while another store is down.
- **Failure:** a store `Down` longer than PD's `max-store-down-time` (default 30 minutes) is re-replicated by PD itself onto the remaining stores. tidb-operator v1 added replacement stores automatically; v2 has no automatic failover and replaces a store when a user deletes its instance (no failover logic was found on `main`; unverified). Loams follows v2 by default: replacement is a `ReplaceInstance` operation, and an opt-in `autoFailover` (capped at one extra store per group) adds a replacement after `max-store-down-time` (Q993).
- **Invariants the operator refuses to change:** `storage.api-version = 2` and `enable-ttl = true` (D122), the keyspace settings PD needs, and the data directory layout.

### 8.5 Loams Postgres

Safekeepers are gone: `loams-wal` is the only WAL (owner directive 2026-10-08, §46 PG2d). The operator applies §46's rules:
- **`loams-wal` acceptors (Arm A):** 3 across zones, quorum 2 of 3. One acceptor at a time, waiting between steps until every timeline's `commit_lsn` on the restarted acceptor has caught up (D716). Replacing an acceptor is a membership change made by `pg-control` (§46 §9.5); the operator asks for it through `pg-control`'s API with the operation ID and fence, then creates the new instance. **The TiKV store arm** is a Deployment pool (two per zone) that rolls with `maxUnavailable: 1`; its durability is TiKV's (§8.4).
- **Pageservers:** before a restart, drain the pageserver's tenants through the storage controller and wait for the fill elsewhere (§46 §12; the API path is (verify)); one at a time.
- **Storage controller, broker, `pg-control`:** Deployments ×2; `pg-control`'s project leases move within one lease period on loss (§46 §11).
- **Computes and PgDog** belong to `pg-control` (D1178). The operator only publishes the compute image pins from the release, and `pg-control` restarts computes in their maintenance windows (§46 §12).
- **If a deployment still runs stock safekeepers** (dev data migration, §46 §9.8), the same quorum rule holds: one at a time, never below a majority.

### 8.6 Loams SQL

- `loams-sqlgate` rolls with `maxSurge: 1, maxUnavailable: 0` and its drain; `loams-sqldb` ×2 with leases.
- Per-branch TiDB computes belong to `loams-sqldb` (D736). The operator publishes the TiDB image pin; `loams-sqldb` moves computes to it at their next resume or in the maintenance window.
- **Skew:** TiDB computes never run newer than TiKV and PD (TiDB's own upgrade order is PD → TiKV → TiDB).

### 8.7 Engine, House, Durable, gateway, agentd

- **Engine:** before rolling a new engine release, run the `migrate` Job (§54, D943): snapshot (the bucket's object versioning plus a TiKV BR backup when the metastore is TiKV), migrate, record in the ledger; then roll the roles. With the `raft` metastore, `meta` instances roll one at a time, waiting until the restarted voter has caught up with the leader's log.
- **House:** the front drains for up to 60 s (§49 §11); workers are disposable; Lakekeeper ≥ 2.
- **Durable:** a Deployment with an HPA; its state is in TiKV.
- **Gateway:** two replicas, 60 s drain (D1019).
- **agentd:** single replica by design (D921); the operator restarts it and keeps its PVC.

### 8.8 Releases, upgrade order and version skew (D1186, D1187)

**A release is a pinned set.** `LoamsCluster.spec.release` names a Loams release, and the operator resolves it from the release's pin files (`release/upgrade-path.toml` and `deploy/pins/*`, §54 §11) to one image digest per component. A per-component `version` override is accepted only with `allowSkew: true`, and the skew rules below still apply.

**One step at a time.** Upgrades go from release N−1 to N only (§54's rule); skipping is refused with `UpgradePathInvalid`. A release's `compatibleWith` (D545) is enforced here.

**Order inside one cluster:**

| Order | Component | Why here |
|---|---|---|
| 0 | CRDs, then the operator | The operator must understand the new release |
| 1 | Object store (RustFS) | Everything depends on it; S3 compatibility is stable |
| 2 | PD | TiDB's documented order: PD before TiKV |
| 3 | TiKV | Before every TiKV client |
| 4 | Loams Postgres storage: broker, storage controller, pageservers, `loams-wal` | Servers before `pg-control` and computes |
| 5 | `pg-control`, `loams-sqldb`, `loams-sqlgate` | Control planes after their storage |
| 6 | `migrate` Job, then the engine roles | §54: migrate before the engine starts |
| 7 | Durable, House | Clients of TiKV and the bucket |
| 8 | Gateway | Speaks to both old and new backends |
| 9 | Studio console, agentd | Clients last; the console supports N−1 (D938) |
| 10 | Computes (Postgres, TiDB) | By their control planes, in maintenance windows |

Each step waits for the component's `Ready` and its LOC health probe before the next starts; a failure stops the upgrade with `UpgradeBlocked` and the reason, and never rolls back on its own.

**Skew rules:**

| Pair | Rule |
|---|---|
| Operator ↔ release | The operator manages releases N and N−1 |
| PD ↔ TiKV ↔ TiDB | One version in steady state, as tidb-operator v2 requires; during an upgrade PD ≥ TiKV ≥ TiDB compute (v2's upgrader holds TiKV until PD is at the new version and healthy, and TiDB until TiKV is) |
| Engine ↔ formats | N reads N and N−1 formats (D923) |
| Console ↔ engine | N and N−1 (D938) |
| `pg-control` ↔ pageserver and `loams-wal` | Within one Loams release |
| CRD storage version ↔ operator | The operator reads the storage version and the one before |

**Downgrades** are refused by default. A release may declare a downgrade edge (a patch release with no `noDowngrade` marker, D545); any other way back is a restore from backup (§8.9). LOC requires that a refused downgrade is refused cleanly, with no Pod restarted (`LOC-UPG-006`).

### 8.9 Backups and restore (D1188, D1189)

Most Loams data already lives in the bucket, so a Loams backup is mostly **pinning a consistent point**:

| Component | Backup | Restore |
|---|---|---|
| Engine | Bucket object versioning + a metastore snapshot (Raft snapshot in the bucket, or a TiKV BR backup of the engine's keyspace) | Point the new cluster's metastore at the snapshot; objects by version |
| TiKV keyspaces (engine, Live, SQL) | BR snapshot backup and log backup to the bucket, per keyspace where API v2 keyspace backup is supported (verify; D131, Q37); otherwise per cluster | BR restore into a fresh TiKV group before any client starts |
| Loams Postgres | Layers and WAL already in the bucket; the storage controller's database by CloudNativePG's backup; bucket versioning (D715) | A new cluster attaches to the same prefixes at a point; PITR stays `pg-control`'s per branch |
| House | Iceberg in the bucket; Lakekeeper's Postgres by CloudNativePG PITR (§49 §11) | Catalog PITR then `RegisterTables` |
| Kubernetes objects | The `LoamsCluster` and component specs, `ObjectStore`s and Secret **references** (never values), exported as YAML into the backup | Re-applied by `LoamsRestore` |

- **`LoamsBackup`** runs these in a fixed order and records each part's position (BR `backup-ts`, metastore index, bucket version marker). A backup is `Complete` only when every part is.
- **`LoamsRestore` always restores into a new cluster** (a new `LoamsCluster` in the `Restoring` phase), never over a live one. The order is: object store and prefixes → PD and TiKV (BR restore) → metastore → Postgres storage → control planes → engine → the rest. Clients start only after the restore is `Complete`.
- **The restore drill** (D715's monthly drill) is a `LoamsBackupSchedule` option: restore into a scratch namespace, run checksums, delete, and alert on failure. It is also LOC's `LOC-BKP` round trip.

### 8.10 Object storage

- **BYO S3:** an `ObjectStore` with `provider: s3` references a Secret or uses workload identity. At creation and after every credential change, the operator runs a **capability probe** Job: conditional `PUT` (`If-None-Match`, `If-Match`), range reads, list-after-write, multipart, and object versioning. A store without conditional writes is refused (D178); without versioning it is `Degraded` (backups need it).
- **RustFS:** `provider: rustfs` renders RustFS from the release's pinned image (Apache-2.0; 1.0.0 on 2026-09-16, 1.0.3 on 2026-10-17) in its distributed mode across zones, with versioning on (verify RustFS's erasure-set requirements and its admin API, §25 §5). RustFS now has its own Rust operator ([rustfs/operator](https://github.com/rustfs/operator), Apache-2.0, kube-rs, one `Tenant` kind, 0.0.8). It is too young to depend on; OP1 renders RustFS the way its upstream Helm chart does, and Q999 asks whether to port from or embed that operator's reconciler once it reaches a stable release.

## 9. PD and TiKV drivers: bridge first, native next (D1182)

`LoamsPD.spec.driver` and `LoamsTiKV.spec.driver` choose who runs the members:

| Driver | What the operator does | When |
|---|---|---|
| `external` | Nothing but connect: takes PD endpoints and TLS material, probes health, reports status | Users with their own TiKV; managed TiKV offerings |
| `bridge` | Renders tidb-operator v2's `Cluster`, `PDGroup` and `TiKVGroup` (`core.pingcap.com/v1alpha1`, no `TiDBGroup`, as D179) and maps their status into Loams conditions. The pinned tidb-operator is installed by the Loams chart as a dependency | OP1 default |
| `native` | Loams' own reconcilers on `LoamsInstance`s (§8.3, §8.4), ported from tidb-operator v2's behaviour (§13) | From OP2; the default once it passes every LOC HA requirement the bridge passes |

- The owner asked for a single operator. The bridge keeps a second operator running only during OP1, behind the same Loams API, so users never touch `core.pingcap.com` objects, and LOC's results decide when it goes.
- **No in-place move from `bridge` to `native`.** The two drivers label and own Pods differently, and adopting another operator's Pods under load is exactly the kind of untested path this design avoids. A cluster moves by backup and restore into a `native` cluster, or stays on `bridge` until a later release (Q994).

## 10. Status, events, metrics and day-2 operations (D1191, D1192)

### 10.1 Conditions

Every Loams kind has `status.observedGeneration` and `status.conditions` as `metav1.Condition` (type, status, reason, message, `lastTransitionTime`, `observedGeneration`). There is no `status.phase` field (the Kubernetes API conventions advise against it); a `status.summary` string feeds the printer columns.

| Type | Meaning | On |
|---|---|---|
| `Ready` | The object serves as specified | All kinds |
| `Progressing` | An operation is running (the reason names it) | All kinds with workloads |
| `Degraded` | Serving, but below the spec (a member down, zones below quorum, no bucket versioning) | All kinds with workloads |
| `Paused` | `loams.dev/paused` is set | All kinds |
| `Drift` | A drift was corrected in the last hour | All kinds with owned objects |
| `QuorumHealthy` | The group has a healthy majority and a leader | PD, TiKV, `meta`, acceptors |
| `UpgradeBlocked` | An upgrade stopped; the reason says why | Groups, `LoamsCluster` |
| `BackupHealthy` | The last scheduled backup and drill succeeded | `LoamsCluster`, `LoamsBackupSchedule` |
| `ProductReady/<product>`, `ErasurePending` | CP1's tenancy conditions | `LoamsNamespace` |

Reasons are CamelCase from a closed registry, `docs/api/operator-reasons.md`, the way `docs/api/reasons.md` registers API reasons. A reason not in the registry fails a unit test.

`LoamsCluster` rolls up its components: `Ready` only when every enabled component is `Ready`; `Degraded` if any is.

### 10.2 Events

`events.k8s.io/v1` events on the object they concern, with the operation ID in the note. Every disruptive step emits a `Normal` event before (`EvictingLeaders store=5 op=op-7f3c`) and after it; every refusal, fence conflict and timeout is a `Warning`. Events are aggregated by the API server's rules and never carry secrets.

### 10.3 Metrics and traces

`/metrics` on every replica; families prefixed `loams_operator_`; **no tenant labels** (as D1021):

| Metric | Labels |
|---|---|
| `reconcile_total`, `reconcile_duration_seconds` | `controller`, `result` |
| `leader` (gauge, 1 on the leader), `lease_transitions_total` | `replica` |
| `operations` (gauge), `operation_step_duration_seconds` | `kind`, `state` |
| `disruption_lease_held` (gauge) | `group` |
| `drift_corrections_total` | `kind` |
| `component_ready` (gauge) | `cluster`, `component` |
| `webhook_requests_total` | `webhook`, `result` |
| `fence_conflicts_total` | `controller` |

OTel spans cover each reconcile and each admin-API call, with the operation ID as an attribute. Dashboards and alerts go to `deploy/observability/loams-operator/` (an operation stuck in one step past its timeout, a fence conflict, no leader for 60 s, `QuorumHealthy=False`, `BackupHealthy=False`).

### 10.4 Day-2 operations

Day-2 actions are objects, so they are audited by the API server, replayable from Git and usable by the BYOC agent:

| `LoamsOperation.spec.type` | Effect |
|---|---|
| `Restart` | Rolling restart of a component, with the same safety as an upgrade |
| `EvictLeaders` / `ClearEviction` | Move region leaders off a TiKV store (for maintenance outside Kubernetes) |
| `TransferPDLeader` | Move PD leadership to a named member |
| `ReplaceInstance` | Replace a member by identity (new PVC), with re-replication |
| `RotateCertificates` | Issue new certificates and roll every component that uses them |
| `Pause` / `Resume` | Set or clear `loams.dev/paused` on a tree of objects |
| `UpgradePlan` | Compute, without acting, the steps an upgrade to a release would take, written to the operation's status; with `approval: required` on the `LoamsCluster`, the upgrade waits for `loams.dev/approved-by` on this object |
| `Diagnose` | Collect status, events, PD and `pg-control` views and recent logs into a bundle in the bucket (no secrets) |

The `loams` CLI gains `loams operator status | plan | upgrade | pause | resume | evict-leaders | backup | restore | diagnose`, which create and watch these objects; the console's operator view (D555) reads the same status.

## 11. Security and multi-tenancy (D1193–D1196)

### 11.1 Least-privilege RBAC

| Identity | May | May not |
|---|---|---|
| Operator (leader and followers) | In managed namespaces: Pods, PVCs, Services, Deployments, StatefulSets, Jobs, ConfigMaps, PDBs, NetworkPolicies, Leases, Events; Secrets `get`, `create`, `update` by name; its own kinds and their status. Cluster-wide: read Nodes (topology), read CRDs; `create`/`delete` Namespaces (tenancy, §11.2) | `pods/exec`, `pods/attach`, `escalate`, `bind`, `impersonate`, cluster-wide Secret `list`/`watch`, any `*` verb |
| `pg-control`, `loams-sqldb` | In their tenant namespaces only: Pods, Services, Secrets, NetworkPolicies (D718, D736) | Anything cluster-scoped |
| Every other workload | Nothing (`automountServiceAccountToken: false`) | — |

- **No exec, ever.** Every PD, TiKV and Loams operation goes through an HTTP or gRPC admin API over mTLS. `pods/exec` is the widest privilege an operator usually holds; this one does not need it.
- RBAC cannot restrict namespace creation to a name prefix, so a **ValidatingAdmissionPolicy** (CEL, GA since Kubernetes 1.30) allows the operator's ServiceAccount to create, label and delete only namespaces named `loams-*` that carry `loams.dev/managed-by` (CP1's `agent_rbac_is_namespace_scoped`).

### 11.2 Scope

- **Cluster scope** (default): one operator per Kubernetes cluster, watching `LoamsCluster`s in any namespace and managing tenant namespaces through `LoamsNamespace`.
- **Namespace scope**, for a BYOC customer who grants Loams one namespace: the operator watches only its namespace, gets a `Role` rather than a `ClusterRole`, and the tenancy controller is off (tenancy lives inside the Loams namespace model, not Kubernetes namespaces). Topology then comes from the PD and Pod labels the customer sets, since Nodes cannot be read.
- One operator per Kubernetes cluster in either scope; two operators watching the same objects is refused by the leader Lease name and a startup check.

### 11.3 Secrets

- **Specs never contain secret values,** only references. A webhook refuses inline credentials in any Loams kind.
- **`secrets.provider: kubernetes`** (default): the operator creates Secrets it needs (TLS, internal tokens, bucket keys from `ObjectStoreProvider.issue_credentials`) in the component's namespace.
- **`secrets.provider: loams`**: values are issued and rotated by Loams Secrets (§64 §5) and delivered as Kubernetes Secrets by the operator (or by ESO through §64's webhook provider); leases are renewed by the operator and revoked when the component is deleted.
- **TLS everywhere internal:** PD, TiKV, the engine, `pg-control`, `loams-wal` and the gateway's upstreams use mTLS from the operator's CA (CP1's cluster CA, rcgen) or cert-manager. Rotation is `RotateCertificates`.
- Logs, events, metrics and `Diagnose` bundles carry no secret value; a test captures them through LOC's runs and greps for every issued value.

### 11.4 Exposure: the engine is never public (D1195)

The standing rule (networking map, amended 2026-10-21 by D1311): the engine, admin and data endpoints are never on the public internet. They are reached on loopback, over the tailnet, or by `loams-gateway` on an internal network; callers off the tailnet reach Loams only through the gateway's edge listener, over a private link or a brokered tunnel that authenticates every request first ([§69](69-edge-reach.md) §9.1).

- `spec.exposure.mode` is `tailnet` (default; the Tailscale Kubernetes operator or a sidecar, as `net.mode: tailnet` in D587) or `internal` (`ClusterIP`, or a `LoadBalancer` carrying the provider's internal annotation from a fixed list).
- The operator **never renders** a public `LoadBalancer`, a `NodePort`, an `Ingress` or a Gateway API route for any Loams component, and a CEL rule refuses a spec that asks for one. This amends D1019's `type: LoadBalancer` for the gateway to an internal load balancer (approved by the owner on 2026-10-19 as Q992's default).
- A ValidatingAdmissionPolicy shipped with the chart also refuses Services of those types in managed namespaces from anyone, so a hand-made Service cannot open a hole the operator would not.
- `NetworkPolicy` is default-deny in every managed namespace, with per-component allow rules rendered from the component graph (PD ↔ TiKV, engine → TiKV and the bucket, gateway → PgDog, and so on).

### 11.5 Multi-tenancy

- The operator is platform infrastructure. Tenants never hold Kubernetes RBAC on Loams kinds; they reach Loams through its APIs, and their isolation comes from `LoamsNamespace` (D443): namespace, default-deny NetworkPolicy, ResourceQuota, LimitRange, sandboxed RuntimeClass where §56 requires it.
- Pods run the `restricted` Pod Security Standard: non-root, read-only root filesystem except data directories, `seccompProfile: RuntimeDefault`, all capabilities dropped. The managed namespaces carry `pod-security.kubernetes.io/enforce: restricted`.
- Images are pinned by digest from the release; the operator refuses an override that is not a digest unless `allowUnpinned: true` (dev only).

## 12. Loams Operator Conformance (LOC) (D1197–D1200)

### 12.1 Why another suite

Operators are how stateful software runs on Kubernetes, and users choose them mostly on claims. The existing tools each cover a part:

| Prior art | What it does | What it does not do |
|---|---|---|
| [Kubernetes conformance](https://github.com/cncf/k8s-conformance) run by [Sonobuoy](https://github.com/vmware-tanzu/sonobuoy) (Apache-2.0; v0.57.5, 2026-07-01) | Proves a **cluster** implements the Kubernetes API; submissions by pull request with results, and a logo programme | Says nothing about operators or the workloads they run |
| [Operator Capability Levels](https://operatorframework.io/operator-capabilities/) (Basic Install, Seamless Upgrades, Full Lifecycle, Deep Insights, Auto Pilot) | A shared vocabulary for operator maturity | **Self-declared**: CloudNativePG, for example, states level V in its docs; no test proves a level |
| [Operator SDK scorecard](https://sdk.operatorframework.io/docs/testing-operators/scorecard/) (Apache-2.0; Operator Framework is CNCF Incubating) | Static and basic checks of an OLM bundle (CRD descriptions, status present) | No faults, no upgrades, no data checks |
| [KUTTL](https://github.com/kudobuilder/kuttl) (Apache-2.0; v0.27.0, 2026-09-23), [Chainsaw](https://github.com/kyverno/chainsaw) (Apache-2.0, a Kyverno subproject; v0.2.15, 2026-05-06) | Declarative "apply, then assert" test frameworks | Frameworks, not a spec: every project writes its own tests, and there is no shared list of required behaviours |
| [Chaos Mesh](https://github.com/chaos-mesh/chaos-mesh) (Apache-2.0, CNCF Incubating; v2.8.5, 2026-10-16), [LitmusChaos](https://github.com/litmuschaos/litmus) (Apache-2.0, CNCF Incubating; 3.33.0, 2026-10-15) | Inject faults into Pods, networks, nodes and clocks | Inject; do not define what a correct operator must do under the fault |
| [Jepsen](https://jepsen.io/) and Elle | Check database histories for consistency anomalies under faults | Test databases, not operators; a bespoke harness per system |
| Each operator's own e2e suite (tidb-operator's, CloudNativePG's, RustFS operator's kind harness) | Deep tests of one operator | Not comparable across operators; results are not published in a common form |

**What is new in LOC:**
1. **A spec of operator behaviour with IDs and levels,** testable and versioned, so "HA" means a list of passed requirements rather than a claim.
2. **Correctness of the workload under operator actions:** every chaos and upgrade test runs a workload probe that checks acknowledged writes survive, not only that Pods become ready.
3. **Operator-side faults:** a paused (stale) leader, API-server loss, a leader killed at every step boundary of an upgrade. Chaos tools fault the workload; LOC also faults the operator.
4. **Recorded-history invariants:** the harness records every write the operator made (from the API server's audit log) and checks properties after the run, such as "never two members of one quorum group down by the operator's hand at once".
5. **Operator-agnostic by an adapter** (§12.6), with a language-neutral protocol, so a Go or Java operator can run it.
6. **A machine-readable, signed report and a badge** that name the suite version, the operator version and the exact requirements passed.

### 12.2 Structure of the spec

- Requirements have IDs `LOC-<AREA>-<NNN>` and an RFC 2119 level: **MUST** (failing fails the level), **SHOULD** (failing is reported and lowers nothing, but the report lists it), **MAY** (informational).
- Each requirement belongs to one **level** (Core, HA, Day-2) and applies to one or more **workload classes**: `stateless`, `quorum` (Raft or Paxos members: PD, TiKV, etcd, ZooKeeper), `primary-replica` (Postgres, MySQL), `bucket-backed` (state in object storage).
- The spec is a Markdown document plus a machine-readable `loc-spec.toml` (ID, level, classes, text, test name), so the harness and the report cannot drift from the spec.
- Areas: `REC` reconcile and idempotency, `DRF` drift, `DEL` finalizers and deletion, `STS` status and events, `API` CRDs and versions, `SEC` security, `OBS` observability, `HA` operator HA and fencing, `CHA` workload under chaos, `UPG` upgrades and skew, `BKP` backup and restore, `SCL` scale.

### 12.3 Levels

Levels are cumulative: HA requires Core, and Day-2 requires HA.

| Level | Proves | Cluster | Faults | Typical run (estimate) |
|---|---|---|---|---|
| **Core** | Correct reconciliation, idempotency, drift correction, finalizers and safe deletion, status and events, CRD hygiene, least-privilege RBAC, metrics | kind or k3d, 1 control plane + 3 workers | Operator Pod killed | 15–30 min |
| **HA** | The operator survives its own failures and keeps the workload's quorum under faults | 3 zones simulated by node labels, operator ×2 or ×3 | Pod kill, node drain, node stop, network partition, leader kill, stale leader (`SIGSTOP`), API-server unavailability | 1–3 h |
| **Day-2** | Upgrades, downgrades and skew matrices, backup and restore round trips, scale-out, scale-in and scale to zero, day-2 operations, certificate rotation | As HA | Faults at upgrade step boundaries | 3–8 h |

### 12.4 Requirement catalogue (first draft)

Core:

| ID | Level | Requirement |
|---|---|---|
| LOC-REC-001 | MUST | Applying the same spec twice causes no write to any owned object the second time (idempotency, checked by resourceVersion and audit) |
| LOC-REC-002 | MUST | Restarting the operator with no spec change causes no Pod restart and no write other than status heartbeats |
| LOC-REC-003 | MUST | `status.observedGeneration` reaches `metadata.generation` within the adapter's declared bound after each change |
| LOC-REC-004 | SHOULD | A reconcile performs at most one disruptive action per group |
| LOC-DRF-001 | MUST | A manual edit to an operator-owned field of an owned object is reverted, with an event |
| LOC-DRF-002 | MUST | A deleted owned object (Service, ConfigMap, PDB) is recreated |
| LOC-DRF-003 | SHOULD | Fields the operator does not own are left alone |
| LOC-DEL-001 | MUST | Deleting the custom resource removes every owned object it created, and nothing else |
| LOC-DEL-002 | MUST | Data volumes are kept or deleted exactly as the declared retention policy says |
| LOC-DEL-003 | MUST | A finalizer never blocks deletion forever: with the operator gone, a documented annotation lets an admin finish deletion |
| LOC-DEL-004 | MUST | Deleting a member that holds data (a quorum member) drains or replicates it first, or is refused |
| LOC-STS-001 | MUST | Status uses `metav1.Condition` with a `Ready` type, reasons from a published registry, and `observedGeneration` |
| LOC-STS-002 | MUST | Every disruptive action emits an event naming the object and the action |
| LOC-API-001 | MUST | CRDs have structural schemas; immutable fields are enforced by the API server |
| LOC-API-002 | MUST | Conversion between served versions round-trips without loss |
| LOC-SEC-001 | MUST | The operator holds no `*` verbs, no `pods/exec`, no `escalate`, `bind` or `impersonate`, and no cluster-wide Secret `list` |
| LOC-SEC-002 | MUST | No secret value appears in specs, status, events, logs or metrics |
| LOC-SEC-003 | SHOULD | Workloads run the `restricted` Pod Security Standard |
| LOC-SEC-004 | MAY | Nothing is exposed outside the cluster unless the spec asks (Loams makes this MUST for itself) |
| LOC-OBS-001 | MUST | Reconcile counts, errors and durations are exported as metrics |

HA:

| ID | Level | Requirement |
|---|---|---|
| LOC-HA-001 | MUST | With N ≥ 2 replicas, killing the leader elects another within lease duration + 2 × retry, and work resumes |
| LOC-HA-002 | MUST | A leader paused past its lease (`SIGSTOP`) and resumed makes no write that conflicts with the new leader (audit-checked) |
| LOC-HA-003 | MUST | Killing the leader at each step boundary of an operation leads to the operation's completion with the same plan, and no step done twice in a harmful way |
| LOC-HA-004 | MUST | With the API server unavailable for 5 minutes, the workload keeps serving and the operator makes no action from stale state on recovery |
| LOC-HA-005 | MUST | Two operator instances never take two members of one quorum group down at once (audit-checked) |
| LOC-CHA-001 | MUST | Killing any single workload Pod loses no acknowledged write and the workload recovers without human action |
| LOC-CHA-002 | MUST | A node drain completes without losing quorum or acknowledged writes, and leaders are moved before their Pods are evicted (quorum class) |
| LOC-CHA-003 | MUST | A network partition isolating one zone loses no acknowledged write; the majority side keeps serving |
| LOC-CHA-004 | SHOULD | Members are spread across the declared zones, so losing one zone keeps quorum |
| LOC-CHA-005 | MUST | PDBs exist for every component and match its quorum |

Day-2:

| ID | Level | Requirement |
|---|---|---|
| LOC-UPG-001 | MUST | Every upgrade edge the adapter declares completes under load with no acknowledged write lost and availability above the declared target |
| LOC-UPG-002 | MUST | Upgrades proceed one member at a time per quorum group, leaders moved first |
| LOC-UPG-003 | MUST | Upgrading the operator alone restarts no workload Pod |
| LOC-UPG-004 | MUST | An unsupported skip or skew is refused before any change, with a reason |
| LOC-UPG-005 | MUST | A failed member stops the upgrade with a condition; nothing rolls back on its own |
| LOC-UPG-006 | MUST | A downgrade either completes on a declared edge or is refused cleanly |
| LOC-BKP-001 | MUST | A backup taken under load, restored into a new instance, matches the source's checksum at the backup point |
| LOC-BKP-002 | MUST | A restore never writes into a live instance |
| LOC-BKP-003 | SHOULD | Point-in-time restore lands within the declared granularity |
| LOC-SCL-001 | MUST | Scale-out adds members that join and take load without disruption |
| LOC-SCL-002 | MUST | Scale-in removes members only after their data has moved, and is refused below quorum or capacity |
| LOC-SCL-003 | SHOULD | Scale to zero (where the class allows it) keeps data and wakes back to `Ready` within the declared bound |
| LOC-SCL-004 | MUST | Replacing a member by identity re-replicates its data |

The catalogue is a draft: OP0 turns it into `loc-spec.toml` 0.1, and LOC 1.0 freezes it after two external operators have run it (Q998).

### 12.5 The harness

- **`loc` CLI and `loc-harness` crate** (Rust). `loc run --adapter <path or command> --level ha --class quorum --cluster kind` creates the cluster, installs the operator through the adapter, runs the selected requirements and writes the report.
- **Clusters:** kind (default) and k3d, three or more worker nodes, zones simulated by node labels; the API server's **audit log** is turned on with a fixed policy so every write by the operator is recorded with its user agent (the adapter sets one per replica).
- **Fault injectors**, built in and small, with Chaos Mesh as an optional backend and RustFS's [`s3chaos`](https://github.com/rustfs/s3chaos) (Apache-2.0) as an optional object-storage fault proxy for the `bucket-backed` class:

| Fault | How (kind/k3d) |
|---|---|
| Pod kill | API `delete` with grace 0 |
| Node drain | The Eviction API, as `kubectl drain` does |
| Node stop | `docker pause` of the node container |
| Network partition | `iptables` rules inside node containers between zone groups |
| Leader kill | Find the Lease holder, delete its Pod |
| Stale leader | `SIGSTOP` the leader's process past the lease, then `SIGCONT` |
| API-server unavailable | `docker pause` of the control-plane container, or drop its port from the operator's node |
| Step-boundary crash | Watch the operator's events and kill the leader on the Nth disruptive event |

- **Workload probes** generate load during every chaos and upgrade test and check results afterwards: a register/bank workload for stores, list-append for transactional systems (Elle-style), and a checksum for backup tests. The Loams adapter uses `loams-nemesis` checkers where they exist (D315).
- **History checkers** read the audit log and the operator's events after the run and evaluate invariants (`LOC-HA-002`, `LOC-HA-005`, `LOC-REC-001`).
- **Deterministic mode (MAY, Rust operators):** a reconciler built on kube-rs can be run against an in-memory API server under `loams-detsim` (§55) with seeded faults, so many HA requirements run in seconds per seed in unit CI. It complements, never replaces, the kind run.
- **Seeds and artifacts:** every run records its seed, cluster config, versions, audit log, events and probe histories, so any failure replays.

### 12.6 The adapter

The harness knows nothing about the operator under test except what an adapter says. In Rust:

```rust
/// What an operator must expose to be tested by LOC. Methods take the harness's
/// context (kube client, namespace, artifacts dir) and return when the request is
/// accepted; the harness waits on `readiness`.
#[async_trait]
pub trait OperatorAdapter: Send + Sync {
    fn describe(&self) -> AdapterInfo;                       // name, version, classes, levels claimed, kinds
    async fn install(&self, cx: &Cx, opts: &InstallOpts) -> Result<()>;   // replicas, scope
    async fn uninstall(&self, cx: &Cx) -> Result<()>;
    async fn create(&self, cx: &Cx, w: &WorkloadSpec) -> Result<ObjectRef>; // members, version, zones
    async fn scale(&self, cx: &Cx, o: &ObjectRef, members: u32) -> Result<()>;
    async fn upgrade(&self, cx: &Cx, o: &ObjectRef, version: &str) -> Result<()>;
    async fn backup(&self, cx: &Cx, o: &ObjectRef) -> Result<BackupRef>;
    async fn restore(&self, cx: &Cx, b: &BackupRef, into: &str) -> Result<ObjectRef>;
    async fn delete(&self, cx: &Cx, o: &ObjectRef) -> Result<()>;
    fn readiness(&self, obj: &DynamicObject) -> Readiness;   // default: the Ready condition
    fn members(&self, cx: &Cx, o: &ObjectRef) -> BoxFuture<'_, Result<Vec<Member>>>; // pod, role, zone
    fn probe(&self, cx: &Cx, o: &ObjectRef) -> Box<dyn WorkloadProbe>;
    fn versions(&self) -> VersionGraph;                      // supported upgrade and downgrade edges
    fn bounds(&self) -> Bounds;                              // ready-within, availability target
}
```

- **Language-neutral form:** the same methods as JSON-RPC 2.0 over stdio (`loc-adapter` speaks it), so an adapter can be a 200-line Go or Python program. Workload probes can also be a container image implementing three HTTP endpoints (`/write`, `/read`, `/checksum`).
- **Unsupported is honest:** an adapter may return `unsupported` for `backup`, `restore` or `scale`; the requirements that need them are reported `not-applicable`, and the level reached is computed from what applies to the declared classes. An adapter cannot declare away a MUST of a class it claims.

### 12.7 Matrices

| Matrix | Cells |
|---|---|
| Upgrade | Every edge in `versions()`, under load, plus a leader kill at a random step boundary per edge |
| Downgrade | Every declared downgrade edge completes; one undeclared edge per release is refused cleanly |
| Skew | Operator N with workload N−1; operator N−1 (rolled back) with CRDs at N; mixed members during an upgrade held for 10 minutes |
| Kubernetes | The three newest Kubernetes minors (Q997) |
| Topology | One zone (dev) and three zones |

### 12.8 The report and the badge

- **Report** `loc-report.json` (schema `loc.dev/report/v1`; the domain is a placeholder until Q991) with: suite version and spec hash; adapter info; cluster (provider, Kubernetes version, nodes, zones); for each requirement its ID, level, class, result (`pass`, `fail`, `skip`, `not-applicable`), duration, seed, and links to evidence; and a summary of the highest level passed per class. JUnit XML and a Markdown summary are rendered from it.
- **Attestation:** the report can be signed as an in-toto attestation with cosign (keyless in CI), binding it to the operator's image digest.
- **Badge:** an SVG (`LOC HA · quorum · 1.0`) rendered from a verified report. Badges are listed through pull requests to a public registry repository with the report and its attestation, the way CNCF's `k8s-conformance` repository takes submissions. A badge names the suite version and the operator version it was earned on, and expires when the next suite minor has been out for six months (estimate).

### 12.9 Home and governance

- LOC is developed in this repository under `conformance/operator/` (crates `loc-spec`, `loc-harness`, `loc-adapter`), Apache-2.0, and mirrored by a release workflow to its own repository so other projects can depend on it without the Loams workspace (D619's mirror pattern).
- Spec changes go by pull request with two maintainers' review; a requirement is deprecated for one minor before removal; MUSTs are only added in minor releases.
- A neutral home (for example a CNCF working group) is a later question (Q991), once outside operators use it.

### 12.10 How Loams runs it

| CI tier | Level | When | Runner |
|---|---|---|---|
| Every operator pull request | Core, plus deterministic-mode HA tests | Path-filtered on `crates/loams-operator*`, `crates/loams-kube`, `conformance/operator/` | GitHub-hosted (the runner rule) |
| Nightly | HA, quorum class, one PD/TiKV driver per night | `dev` | GitHub-hosted, small TiKV configs (estimate: 3 PD + 3 TiKV fit in 16 GB with reduced block cache, verify) |
| Weekly and release gate | Day-2 | Release branches | GitHub-hosted, or the `loam-bench` runner for the heavy cells |

The `native` PD/TiKV driver becomes the default only after it passes every MUST in HA and Day-2 that `bridge` passes on the same run (D1182).

## 13. What Loams takes from tidb-operator v2, and what it drops (D1183)

tidb-operator v2 has three layers (`docs/arch/README.md` on `main`): a `Cluster` that holds feature gates, a group per component (`PDGroup`, `TiKVGroup`, …, with replicas, update, scale and schedule policies and a Pod template), and an instance per member (`PD`, `TiKV`, …: one Pod, its PVCs, config and state). Its API has three groups: `core.pingcap.com/v1alpha1` (`Cluster`, the group and instance pairs for PD, TiKV, TiDB, TiFlash, TiProxy, TiCDC, TSO, Scheduling, Scheduler, Router, ResourceManager, TiKVWorker and DM, and `PlacementPolicy`), `br.pingcap.com/v1alpha1` (`Backup`, `BackupSchedule`, `CompactBackup`, `Restore`, `TiBR`, `TiBRGC`) and shared meta types. Paths below are on `main` as read on 2026-10-19; OP2 Task 0 pins a sha.

| From tidb-operator v2 | Where | Take | Loams |
|---|---|---|---|
| A group per component and an instance per member; Pods managed directly, no StatefulSets ([v2 vs v1](https://docs.pingcap.com/tidb-in-kubernetes/v2.0/v2-vs-v1/)) | `api/core/v1alpha1/`, `docs/why.md` | **Yes** | `LoamsPD`/`LoamsTiKV` + `LoamsInstance` (§5.4) |
| Group updates with `maxSurge 0, maxUnavailable 1`, a minimum ready time and topology hooks | `pkg/updater/`, `pkg/controllers/{pdgroup,tikvgroup}/tasks/updater.go` | **Yes, ported** | §8.3, §8.4, plus the disruption Lease (§7.2) |
| Upgrade preconditions across groups (TiKV waits for PD, TiDB waits for TiKV; 30 s retry) | `pkg/action/upgrader.go` | **Yes, extended** | §8.8 adds the Loams components |
| PD leader transfer before recreating the leader's Pod; non-leaders first | `pkg/controllers/pd/tasks/pod.go`, `pkg/controllers/pdgroup/tasks/util.go`, `pkg/pdapi/v1/client.go` | **Yes, ported** | §8.3 |
| TiKV leader eviction with a `LeadersEvicted` condition; no down peers and the region-cache TTL before deleting the Pod; grace by region count | `pkg/controllers/tikv/tasks/{evict_leader.go,pod.go}` | **Yes, ported** | §8.4 |
| Two-step store deletion (reversible `offline`, then removal after `Removed`/tombstone); scale-in blocked at `max-replicas` | `docs/rfcs/250725-two-step-store-deletion.md`, `pkg/controllers/tikv/tasks/offline.go`, `pkg/controllers/common/store_offline.go` | **Yes, ported** | §8.4 |
| `EvenlySpread` schedule policy over topologies with weights; scale-in victim choice | `pkg/updater/policy/topology.go`, `pkg/utils/topology/scheduler.go` | **Yes, ported** | §8.1 |
| Pod and PVC overlays that cannot overwrite system labels | `pkg/overlay/` | **Yes, the idea** | `podTemplate` overlay in each sub-spec |
| Feature gates on the cluster object; per-feature version constraints | `api/meta/v1alpha1/feature.go`, `pkg/compatibility/feat.go` | **Yes, the idea** | `LoamsCluster.spec.featureGates`; constraints in the release pins |
| `PlacementPolicy`: key ranges (keyspaces) bound to TiKV groups as PD placement rules | `docs/rfcs/260707-placement-policy.md` | **Later** | A keyspace-placement field for tenants that need dedicated stores (not in OP0–OP3) |
| BR `Backup` (snapshot and log modes), `Restore`, `CompactBackup` | `api/br/` | **Yes, the flow** | §8.9; note that v2.0's docs mark `BackupSchedule` unsupported (verify on the pinned sha) |
| No automatic failover in v2 (v1 had it) | v2.0 docs | **Default kept** | `ReplaceInstance`; `autoFailover` opt-in (Q993) |
| No operator-created PDBs (inferred) | — | **Changed** | Loams creates PDBs (§8.2) |
| TiDB, TiFlash, TiProxy, TiCDC, TSO and scheduling microservices, Router, ResourceManager, TiKVWorker, DM, `TiBR` | — | **No** | Loams runs none of them (TiDB computes are `loams-sqldb`'s) |
| Go code | — | **No** | Rust; ported behaviour with provenance (§2.3) |

From other projects, as references only:
- **CloudNativePG** (Apache-2.0, v1.30.1 on 2026-09-23, CNCF Sandbox): the `cnpg.io/fencedInstances` annotation, the per-Pod instance manager, two-step failover, and v1.30's per-cluster primary Lease (15 s / 10 s / 2 s) that gates promotion. Loams' fences in §7.2 apply the same idea to the operator itself.
- **Stackable `operator-rs`** (Apache-2.0; its product operators are OSL-3.0 and are not used): cluster-resource apply and prune, condition builders and role-group config merging, as kube-rs conventions.
- **kube-rs `controller-rs`** (Apache-2.0): telemetry and project layout.
- **`neondatabase/helm-charts`** (Apache-2.0, "beta", charts for the storage broker, storage controller, proxy, SNI router, endpoint storage and scrubber; no pageserver, safekeeper or compute charts): a cross-check for `LoamsPostgres` rendering.
- **Clever's operator** (MIT): the `ObjectStoreProvider` idea that D178 already took.

## 14. Minimum viable slice, phases and effort (D1201)

### 14.1 Why this slice

The smallest operator worth running replaces the most hand work with the least new risk: **`LoamsCluster` with the engine, an `ObjectStore`, PD and TiKV through the bridge, the gateway and tenancy**, on an HA operator with the journal and fences, passing LOC Core and the operator half of LOC HA. It reuses tidb-operator v2 where the risk is highest (quorum moves), and builds the parts every later phase needs (the runtime crate, conditions, the harness).

### 14.2 Phases (estimate, engineer-weeks)

| Phase | Content | Effort | Gate to start |
|---|---|---|---|
| **OP0** | Spike: `loams-operator-api` types for `LoamsCluster`, `ObjectStore`, `LoamsInstance`; `loams-kube` leader election and the disruption Lease; `loc-spec.toml` 0.1 and the harness skeleton on kind with pod kill and stale-leader injection | 3–4 | v0.1.0 shipped (Q996) |
| **OP1** | The MVS: engine (all metastore backends), `ObjectStore` (S3 probe, RustFS), PD/TiKV `external` and `bridge`, gateway, `LoamsNamespace` (CP1 Task 15 moves here if not built), journal, fences, drift, conditions, events, metrics, exposure rules, RBAC and VAPs, the Helm chart, `loams operator status/pause/resume`; LOC Core passing; LOC HA's `HA-*` passing | 12–16 | OP0 |
| **OP2** | Native PD and TiKV: `LoamsInstance`, upgrades, scale, failover, the eviction gate; BR backups and `LoamsRestore` for TiKV; LOC HA (quorum class) on `native` and `bridge`; `native` becomes the default at parity | 14–18 | OP1; a 3-zone test cluster |
| **OP3** | `LoamsPostgres`, `LoamsSQL`, `LoamsHouse`, `LoamsDurable`, `LoamsStudio`, `LoamsAgentd`; the full upgrade order and skew rules; the `migrate` Job; `LoamsBackup` across components; `LoamsOperation`; the CLI; LOC Day-2 | 16–22 | OP2; PG2e and SQ1b merged |
| **OP4** | LOC 1.0: the stdio adapter protocol, report signing, the badge registry, adapters for tidb-operator v2 and CloudNativePG as the first external users, public docs | 6–8 | OP2 |
| **Total** | | **51–68** (two engineers, roughly 7–9 months, built slowly as the owner asked) | |

## 15. Risks

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R1 | **The native PD/TiKV port misses edge cases** tidb-operator learned over years | High | High | Bridge first; port with v2's tests as a checklist; LOC parity gate before `native` is default |
| R2 | **A fencing bug lets two leaders act** | Medium | Very high | Three fences (§7.2); `LOC-HA-002` and `LOC-HA-005` with audit checks; deterministic-mode seeds in every PR |
| R3 | **The eviction webhook blocks drains** | Medium | Medium | `failurePolicy: Ignore`; a timeout after which the webhook allows; a metric and alert on denied evictions |
| R4 | **Scope:** one operator for every component becomes a large program | High | Medium | Phases with gates; tenant objects stay in the product control planes (D1178); components join one per release |
| R5 | **CRD churn** before `v1beta1` | Medium | Medium | `v1alpha1` labelled preview; conversion tested from the first bump |
| R6 | **GitHub-hosted runners too small** for HA and Day-2 runs | Medium | Medium | Small configs; deterministic mode; heavy cells on `loam-bench` |
| R7 | **LOC is seen as Loams-specific** and not adopted | Medium | Medium | Adapter protocol, external adapters in OP4, a neutral home later (Q991) |
| R8 | **BR keyspace backup gaps** for API v2 (D131, Q37) | Medium | High | Whole-cluster BR until verified; the restore drill catches gaps |
| R9 | **tidb-operator v2's CRDs change** (`v1alpha1`) under the bridge | Medium | Low | Pinned per release; bridge types generated with kopium per pin |

## 16. What not to build

- **No fork** of tidb-operator, `clever-kubernetes-operator` or any operator.
- **No operator on the data path,** no proxying of client traffic through it.
- **No per-tenant compute reconciliation in the operator;** `pg-control` and `loams-sqldb` keep it.
- **No `pods/exec`,** no shelling into members.
- **No automatic rollback** of an upgrade and **no automatic quorum recovery** (`pd-recover`, unsafe TiKV recovery) — both are documented manual procedures with `LoamsOperation` support at most.
- **No in-place backend switch** (metastore, PD/TiKV driver) on a live cluster.
- **No multi-cluster control** in the operator: rings and fleets are §41's control plane and Argo CD (D545).
- **No public exposure,** no Ingress controller, no service mesh.
- **No own Raft or database** for operator state; the API server is the store.
- **No OLM bundle or OperatorHub listing** before `v1beta1` (Q996 covers the timing).
- **No TiFlash, TiCDC, TiProxy, DM or TiDB dashboard** support.
- **No second Loams operator** for any component, now or later.
- Third-party operators that stay: CloudNativePG (storage controller and Lakekeeper databases), the Knative Operator (D446), the Tailscale Kubernetes operator when `exposure.mode: tailnet` uses it, cert-manager when chosen, and tidb-operator v2 only under the `bridge` driver.

## 17. Amendments to earlier decisions

| Decision | Change |
|---|---|
| D185 (fork `clever-kubernetes-operator`) | **Superseded** by D1173: no fork; a new kube-rs crate |
| D178 (`ObjectStoreProvider` in the forked operator) | Kept; the trait lives in `loams-operator` |
| D179, D736, Q664 (tidb-operator v2 for PD/TiKV) | Kept as the `bridge` driver for OP1; the `native` driver replaces it at LOC parity (D1182) |
| D186, D455 (sync waves) | Narrowed: Argo CD applies CRDs (−2), the operators (−1) and the `LoamsCluster` (0); component order is the operator's (D1184) |
| D443, D544, CP-R10, CPQ5 | The `LoamsNamespace` controller is one of `loams-operator`'s controllers; behaviour unchanged |
| D545 (`compatibleWith`, `noDowngrade`) | Enforced by the operator (D1186, D1187) |
| D718 (`LoamsPostgres` CRD after GA) | `LoamsPostgres` is a component kind of `LoamsCluster`, built in OP3 |
| D943 (`migrate` in a wave) | The operator runs the `migrate` Job before the engine (D1186) |
| D1019 (gateway `type: LoadBalancer`) | **Amended (approved 2026-10-19, Q992's default):** internal load balancer or tailnet only, never a public `LoadBalancer` (D1195); recorded on D1019's row in the decision log |
| §10 §1, D46 (the M2 operator) | This design is that operator |
| Q-RT-8 | Answered by D1174 (`loams.dev`, in this repository) |
| Q-RT-14 | Moot (no fork) |

## 18. Decisions (D1172–D1201, approved by the owner on 2026-10-19)

| ID | Decision |
|---|---|
| D1172 | **One operator, `loams-operator`,** in Rust on kube-rs, installs, upgrades, scales, backs up and heals every Loams component on Kubernetes from one `LoamsCluster`; one binary, one Helm chart, one API group |
| D1173 | **No fork.** No Rust operator for TiDB, TiKV or PD exists (search of 2026-10-19); `clever-kubernetes-operator` is not forked either (supersedes D185). PD and TiKV behaviour is ported from tidb-operator v2 (Apache-2.0) with per-module provenance and a `NOTICE` line, and upstream is tracked by a quarterly review, not merged |
| D1174 | **API group `loams.dev`,** namespaced kinds, versions `v1alpha1` → `v1beta1` → `v1`, schemas generated from Rust types with a drift test, CEL validation for immutable fields and exposure, conversion webhooks with round-trip tests (answers Q-RT-8) |
| D1175 | **Kinds:** `LoamsCluster`; `ObjectStore`; `LoamsEngine`, `LoamsPD`, `LoamsTiKV`, `LoamsPostgres`, `LoamsSQL`, `LoamsGateway`, `LoamsHouse`, `LoamsDurable`, `LoamsStudio`, `LoamsAgentd`; `LoamsInstance` per quorum member with PVCs owned by the instance; `LoamsNamespace`; `LoamsBackup`, `LoamsBackupSchedule`, `LoamsRestore`; `LoamsOperation` |
| D1176 | **Component objects are owned by `LoamsCluster`** and written only by the operator; each can also be created on its own. `LoamsPostgres` is D718's CRD |
| D1177 | **Write rights:** the operator writes components, instances, workloads and status; users and GitOps write clusters, standalone components, namespaces, backups and operations; the BYOC agent creates only `LoamsOperation`s of its closed types; tenants have no Kubernetes RBAC on Loams kinds. The `LoamsNamespace` controller of CP1 is one of the operator's controllers |
| D1178 | **Platform objects in the operator, tenant objects in the product control planes:** Postgres computes, PgDog and per-branch TiDB computes stay with `pg-control` and `loams-sqldb`, which use the shared `loams-kube` runtime and its conventions |
| D1179 | **Crates:** `loams-operator-api` (types, no runtime), `loams-kube` (leader election, fencing, journal, apply, conditions, events, metrics, drift), `loams-operator` (controllers, drivers, webhooks, `crdgen`); none in the `loams` binary's default build; one `kube` version for the workspace settled at OP0 |
| D1180 | **Operator HA:** 2–3 replicas, a `coordination.k8s.io` Lease (15 s / 10 s / 2 s), reconcilers on the leader only, warm caches and webhooks on all replicas, a self-fence on missed renewal; the operator is never on the data path |
| D1181 | **Fencing and crash safety:** an operation journal in status with a fixed plan and derived phases; three fences (resourceVersion on the journal, the Lease transition count as a token checked by Loams control planes, a per-group disruption Lease by compare-and-swap); idempotent, read-first admin calls; no `pods/exec` |
| D1182 | **PD and TiKV drivers `external`, `bridge` (tidb-operator v2, OP1 default) and `native` (OP2);** `native` becomes the default only after it passes every LOC HA and Day-2 MUST that `bridge` passes; no in-place move between drivers |
| D1183 | **What is taken from tidb-operator v2:** per-member objects without StatefulSets, leader eviction, PD leader transfer, store deletion through tombstones, topology labels, overlays, feature gates, BR flows; TiDB, TiFlash, TiCDC, TiProxy, DM and dashboard controllers are dropped |
| D1184 | **The operator orders components inside a cluster;** Argo CD waves shrink to CRDs (−2), operators (−1) and the `LoamsCluster` (0) (narrows D186, D455) |
| D1185 | **Managed HA:** zone spread with `maxSkew: 1` for quorum groups, PD location labels and zone isolation, PDBs per component, an eviction-gate webhook (`failurePolicy: Ignore`) that moves leaders before a quorum member's eviction |
| D1186 | **Releases are pinned sets** resolved from the release's pin files; upgrades only N−1 → N; the order of §8.8 (operator, object store, PD, TiKV, Postgres storage, control planes, `migrate` then engine, Durable and House, gateway, console and agentd, then computes by their control planes); a failed step stops with `UpgradeBlocked` and never rolls back on its own |
| D1187 | **Skew and downgrade rules of §8.8:** operator manages N and N−1; PD ≥ TiKV ≥ TiDB compute; downgrades only on edges a release declares, otherwise restore |
| D1188 | **Backups pin a consistent point** across the bucket, the metastore, TiKV BR (keyspace or cluster), the CloudNativePG databases and the exported specs (no secret values) |
| D1189 | **Restores always go into a new cluster,** in a fixed order, with clients started only after `Complete`; the monthly restore drill is a schedule option and a LOC test |
| D1190 | **`ObjectStore` capability probe** (conditional writes, versioning, range reads, multipart) at creation and on credential change; no conditional writes → refused (D178); no versioning → `Degraded` |
| D1191 | **Status conventions:** `metav1.Condition` with `Ready`, `Progressing`, `Degraded`, `Paused`, `Drift`, `QuorumHealthy`, `UpgradeBlocked`, `BackupHealthy`; no `phase` field; reasons from `docs/api/operator-reasons.md`; events before and after each disruptive step; `loams_operator_*` metrics without tenant labels |
| D1192 | **Day-2 operations are objects** (`LoamsOperation`: restart, evict leaders, PD leader transfer, replace instance, rotate certificates, pause/resume, upgrade plan with optional approval, diagnose) driven by `loams operator …` and the console's operator view |
| D1193 | **Least-privilege RBAC:** no `pods/exec`, `escalate`, `bind`, `impersonate`, `*` verbs or cluster-wide Secret listing; namespace creation limited to `loams-*` by a ValidatingAdmissionPolicy; workloads without API tokens except `pg-control` and `loams-sqldb` in their tenant namespaces |
| D1194 | **Scope:** cluster scope by default; a namespace-scoped install for BYOC without the tenancy controller; one operator per Kubernetes cluster |
| D1195 | **Exposure:** `tailnet` (default) or `internal` only; the operator never renders a public `LoadBalancer`, `NodePort`, Ingress or route for any component, and a chart-shipped VAP refuses them in managed namespaces (narrows D1019 to an internal load balancer) |
| D1196 | **Secrets:** references only in specs; `kubernetes` (default) or `loams` (§64 Loams Secrets) as the provider; internal mTLS from the operator CA or cert-manager; restricted Pod Security; digests only |
| D1197 | **Loams Operator Conformance (LOC)** is a first-class deliverable: a versioned spec with `LOC-<AREA>-<NNN>` IDs and MUST/SHOULD/MAY, cumulative levels Core, HA and Day-2, and workload classes `stateless`, `quorum`, `primary-replica`, `bucket-backed` |
| D1198 | **The LOC harness** runs on kind and k3d with built-in fault injectors (pod kill, drain, node stop, partition, leader kill, stale leader, API-server loss, step-boundary crash), workload probes, audit-log history checks, seeds and artifacts; Chaos Mesh optional; a deterministic mode under `loams-detsim` for Rust operators |
| D1199 | **The LOC adapter:** the `OperatorAdapter` trait in Rust and the same methods as JSON-RPC over stdio, so operators in any language can run LOC; requirements that need an unsupported method are `not-applicable`, never waived for a claimed class |
| D1200 | **LOC reports and badges:** `loc-report.json` with JUnit and Markdown renderings, optional cosign attestation, an SVG badge naming suite and operator versions, a public registry by pull request; developed here under `conformance/operator/` and mirrored to its own repository |
| D1201 | **Phases OP0–OP4** as §14; plan OP1 (stub) covers OP0 and OP1; later phases get plans when their gates are met |

## 19. Open questions (Q984–Q999)

| ID | Question | Default (all answered by the owner on 2026-10-19: the default) |
|---|---|---|
| Q984 | API group `loams.dev`, with the kinds of §5.2, starting at `v1alpha1`? | Yes |
| Q985 | PD and TiKV: bridge to tidb-operator v2 in OP1, native Rust reconcilers in OP2, native default at LOC parity? Or native from the start? | Bridge first, then native at parity |
| Q986 | Do Postgres computes, PgDog and per-branch TiDB computes stay with `pg-control` and `loams-sqldb` rather than the operator? | Yes, sharing `loams-kube` |
| Q987 | A `LoamsInstance` object per quorum member (tidb-operator v2's model), or Pods only with member state in the group's status? | `LoamsInstance` |
| Q988 | Ship the eviction-gate webhook for quorum members? | Yes, `failurePolicy: Ignore`, with a 30-minute allow-after timeout |
| Q989 | The storage controller's and Lakekeeper's Postgres: keep CloudNativePG, or allow an external Postgres too? | Keep CNPG as the default; an external Postgres by connection Secret is also accepted |
| Q990 | Default install scope: cluster, with namespace scope for BYOC on request? | Yes |
| Q991 | LOC's name and home: keep "Loams Operator Conformance" in this repository with a mirror repository (`ostrium-labs/loams-operator-conformance`), or a neutral name from the start? And offer it to a neutral home (for example a CNCF working group) later? | Keep the name for 0.x; decide a neutral name before 1.0 if outside operators adopt it; offer a neutral home only after at least two outside operators publish reports |
| Q992 | May the gateway's Postgres data listener ever be public (D1019's `LoadBalancer`), given the "never public" rule? | No; internal load balancer or tailnet only, revisited only by an owner ruling |
| Q993 | TiKV store failover: manual `ReplaceInstance` with an opt-in `autoFailover` (tidb-operator v2's model), or automatic by default (v1's)? | Manual by default, `autoFailover` opt-in and capped at one extra store per group; PD re-replicates either way |
| Q994 | Moving a `bridge` cluster to `native`: in-place adoption, or backup and restore? | Backup and restore, or stay on `bridge`; no in-place adoption |
| Q995 | Downgrades: only on edges a release declares, otherwise restore from backup? | Yes |
| Q996 | When does OP0 start? | After v0.1.0 ships, like IS1 (§64 §12) |
| Q997 | The Kubernetes support window? | The three newest minors at each Loams release (verify the floor at OP0; VAP needs ≥ 1.30) |
| Q998 | Run LOC against tidb-operator v2 and CloudNativePG as its first external adapters before freezing LOC 1.0? | Yes, in OP4 |
| Q999 | RustFS: render it in `loams-operator` (as OP1 does), or later embed or port the reconciler of RustFS's own Rust operator (`rustfs/operator`, Apache-2.0, 0.0.8)? | Render it ourselves; re-evaluate when `rustfs/operator` has a stable release, porting rather than running a second operator |

## 20. Sources

All read on 2026-10-19 unless stated.

- The search: [pingcap/tidb-operator](https://github.com/pingcap/tidb-operator) (Go, Apache-2.0; releases [list](https://github.com/pingcap/tidb-operator/releases)); [tikv/tikv-operator](https://github.com/tikv/tikv-operator) (archived); [aexol-studio/sdb-controller](https://github.com/aexol-studio/sdb-controller) (Rust, AGPL-3.0; `src/cluster/crd.rs` imports `tidb_api::tidbclusters::TidbCluster`); [kube-rs/kopium](https://github.com/kube-rs/kopium); [crates.io API search](https://crates.io/api/v1/crates?q=tidb%20operator).
- TiDB Operator docs: [overview](https://docs.pingcap.com/tidb-in-kubernetes/stable/tidb-operator-overview/); [release notes](https://docs.pingcap.com/releases/tidb-operator/).
- kube-rs: [repository](https://github.com/kube-rs/kube); [controller-rs](https://github.com/kube-rs/controller-rs); [guides](https://kube.rs/guides/).
- Other operators: [CleverCloud/clever-kubernetes-operator](https://github.com/CleverCloud/clever-kubernetes-operator) (Rust, MIT); [CloudNativePG](https://github.com/cloudnative-pg/cloudnative-pg) (Go, Apache-2.0); [Stackable operator-rs](https://github.com/stackabletech/operator-rs); [RustFS](https://github.com/rustfs/rustfs) (Apache-2.0); [Tailscale](https://github.com/tailscale/tailscale) (BSD-3-Clause; Kubernetes operator in-tree).
- Kubernetes: [API conventions (conditions)](https://github.com/kubernetes/community/blob/master/contributors/devel/sig-architecture/api-conventions.md#typical-status-properties); [leader election in client-go](https://pkg.go.dev/k8s.io/client-go/tools/leaderelection); [ValidatingAdmissionPolicy](https://kubernetes.io/docs/reference/access-authn-authz/validating-admission-policy/); [the Eviction API](https://kubernetes.io/docs/concepts/scheduling-eviction/api-eviction/); [Pod Security Standards](https://kubernetes.io/docs/concepts/security/pod-security-standards/).
- Conformance prior art: [k8s-conformance](https://github.com/cncf/k8s-conformance); [Sonobuoy](https://github.com/vmware-tanzu/sonobuoy); [Operator Capability Levels](https://sdk.operatorframework.io/docs/overview/operator-capabilities/); [scorecard](https://sdk.operatorframework.io/docs/testing-operators/scorecard/); [KUTTL](https://github.com/kudobuilder/kuttl); [Chainsaw](https://github.com/kyverno/chainsaw); [Chaos Mesh](https://github.com/chaos-mesh/chaos-mesh); [LitmusChaos](https://github.com/litmuschaos/litmus); [Jepsen](https://jepsen.io/).
- Loams: the designs and plans linked at the top; `Cargo.toml` (`kube` 3.1.0, `k8s-openapi` 0.27.1 on `dev` at `49e169e8`); `crates/loams-pg-control/Cargo.toml` (`kubernetes` feature); `deploy/release/compose.yaml`.

> **See also (2026-10-21):** [§69](69-edge-reach.md) (Edge Reach, approved by the owner 2026-10-21) adds `exposure.mode: tunnel` (a `cloudflared` Deployment with no Service, so D1195 holds) and renders the gateway's edge listeners in `internal` mode (D1316). There is no `edge-public` mode: D1312 was deferred, so Q992's answer stands.
