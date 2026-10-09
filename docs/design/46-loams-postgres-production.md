# 46 — Loams Postgres in Production: Serverless Postgres on the Bucket, Routed by PgDog

Status: **Proposed** · 2026-10-08. Source: the owner's goal "Loams Postgres (serverless, Neon-based) production ready", and the owner's clarification of the same day, which is binding: Loams Postgres is **serverless Postgres on object storage (Neon) with a PgDog router** ("same for postgres add pgdog router"). Decisions **D700–D719** and questions **Q640–Q654** are recorded in the [decision log](13-decision-log.md). Plan: [PG2](../plans/2026-10-08-pg2-postgres-production.md).

This is an addendum to [§28](28-loams-postgres.md). It keeps §28's approved decisions D230–D232 and D234–D236 and its proposals D237–D241 and D263–D272. Under the owner's directive of 2026-10-08 ("remove safekeepers"), it **reverses D233's gating**: `loams-wal` is the only WAL at launch, and §28 §7's relative gate becomes the absolute launch targets of §9.3 (D714). It also builds on [§23](23-neon-and-wesql.md) §6 (N1–N6), [§31](31-loams-router-and-verification.md) (D304–D307, PgDog sharding), [§41](41-multitenant-byoc-control-plane.md) (D540–D559, the open control plane), [§19](19-console-identity-and-agents.md) and [§38](38-knative-authentik-gitops.md) §4 (identity), [§44](44-unified-api-and-sdks.md) (the API rules) and [§37](37-desktop-and-mobile-apps.md) §19.10 (D667, D668: the desktop as a local control plane). It **answers Q113**: the scale-to-zero design is PgDog plus a Loams waker, with Neon's proxy kept as the fallback (D709). It **amends D668**: the desktop's direct pageserver calls become the fallback, used only when a server does not offer this API (D719).

**Amended by [§51](51-neon-fork.md) (2026-10-09).** Ostrium Labs now owns and maintains the Neon fork (D800). Fork releases follow D810–D811, not D241. The majors are D813's, which answers Q649. Major upgrades go through `fast_import` and `ImportPgdata` first (D815, amending D716's dump and restore into a fallback). The extension allow-list is D812's catalogue (Q651).

**Out of scope:** the engine's `--pg-listen` listener (feature `pgwire`, PG1). It serves read-only SQL over a namespace's collections. It is **not** Loams Postgres, it never routes to a compute, and nothing here changes it.

Markers: **(verify)** means not checked against a primary source; the plan task that depends on the fact checks it first. **(estimate)** means computed, not measured. **(target)** is a number this document sets as a gate.

---

## 1. Summary

| # | Decision | Status |
|---|---|---|
| D700 | **"Production ready" means the exit checklist in §17** is met on the reference topology. It covers the API, the compute lifecycle, PgDog routing, HA, PITR, upgrades, observability, quotas, a security review, conformance and the §16 performance gates. **`loams-wal` is the only WAL** (owner, 2026-10-08: "remove safekeepers"), and its launch targets (§9.3) are GA gates | Proposed |
| D701 | **Resource model:** org → namespace → **project** (one Neon tenant) → **branch** (one timeline) → **endpoint** (a stable address and its settings) → **compute** (a disposable Postgres process); **roles** and **databases** are per branch and are inherited when branching. Ids are ULIDs with type prefixes; Neon tenant and timeline ids are derived from them deterministically (§3) | Proposed |
| D702 | **API:** `loams.postgres.v1.PostgresService` (Connect, AP0 rules: an `idempotency_key` on every mutation, `NO_SIDE_EFFECTS` reads, `loams.errors.v1` reasons, AIP-158 pagination, watch streams with a snapshot, changes and a 15 s heartbeat). Slow mutations return a `loams.operations.v1.Operation`. The console REST routes map 1:1 under `/v1/namespaces/{ns}/postgres/…` (§4) | Proposed |
| D703 | **Open source and private, stated explicitly:** the control plane, the Neon-facing upcalls, the compute runtimes, the autoscaler, PgDog config management, the waker, quota *enforcement*, the Helm chart and single-node mode are Apache-2.0 in this repository. `loams-platform` keeps the plan → limits mapping, billing-grade compute and storage usage, the commercial create and upgrade APIs and hosted fleet operations. The seam is the limits API and a `ComputeLifecycleObserver` trait with no wire format (§5) | Proposed |
| D704 | **Crates and processes:** `loams-neon` (the client of Neon's components, §23 N1), `loams-pg-control` (service, reconcilers, upcalls, waker, store) and the generated types in `loams-proto`. They run as the `pg-control` role of the `loams` binary behind the feature `postgres`, off by default (§6) | Proposed |
| D705 | **State:** a `PgControlStore` trait with a TiKV backend (the `x/`, `X/`, `C/` records of §28 §5.3, plus `E/`, `R/` and `D/`) and a local redb backend for single-node mode, sharing one conformance suite. Writes are compare-and-set on a version (§6.2) | Proposed |
| D706 | **Level-triggered reconcilers** compare desired and observed state for each resource. They hold a fenced lease per project (`check_fence`, §28 §6.4), so exactly one `pg-control` instance acts on a project at a time. Multi-step flows that need compensation (branch restore, the WAL switch) are Loams Durable sagas (§23 §6.5) | Proposed |
| D707 | **Compute lifecycle:** `ComputeRuntime` with `compose` (dev, desktop, small self-hosts) and `kubernetes` backends. On Kubernetes, computes are Pods created by `pg-control` directly, not through Git: they are runtime state with minute-scale churn, as Knative's Pods are. The endpoint states are `idle → starting → running → suspending → suspended`, plus `failed` and `deleting`. `suspend_timeout` defaults to 300 s. A **warm pool** of spec-less computes cuts cold starts (§7) | Proposed |
| D708 | **PgDog is the front door** (owner, 2026-10-08), unmodified and AGPL-3.0, as a separate service: it terminates TLS, authenticates with SCRAM, pools (transaction mode by default), routes by database name to the branch's compute, balances reads across replica computes, health-checks backends and carries §31's sharding. There is **one PgDog Deployment per Loams namespace**, reached through an L4 SNI-passthrough listener on `<ns>.<region>.pg.<base-domain>`. `pg-control` renders and reloads its config through RT1's renderer and `ConfigPush` machine (§8) | Proposed (implements the owner's clarification; amends D236's single-PgDog layout) |
| D709 | **Wake-on-connect without modifying PgDog.** Each endpoint has a stable backend address (a Kubernetes Service, or a compose port). PgDog queues a client while the backend is down. `pg-control`'s **waker** watches PgDog's admin database (`SHOW POOLS`) and metrics for clients waiting on a suspended endpoint, then starts its compute. If PG2 Task 18 shows PgDog cannot hold a client through a wake, the fallback is Neon's proxy (fork, Apache-2.0) in front of PgDog with Loams' `wake_compute`. **Answers Q113** (§8.4) | Proposed; PgDog's queueing is (verify) |
| D710 | **TLS on both hops, and plaintext refused.** Client → PgDog uses TLS 1.2+ with a certificate per namespace hostname. PgDog → compute uses TLS verified against the cluster CA, or mesh mTLS. The Neon storage components use JWT auth (`NeonJWT`) scoped to the tenant. `loams-wal` gains TLS on both listeners before launch (§8.5) | Proposed |
| D711 | **Auth:** SCRAM-SHA-256 only, with MD5 and plain refused. Role secrets are generated by Loams, shown once and kept in the credential store (Q30). **JWT and OIDC** go through credential exchange: a caller with an Authentik-backed Loams token (D449) calls `IssueConnectCredential` and receives a short-lived role and password (15 min by default, at most 1 h) after an OpenFGA `connect` check. If upstream PgDog gains native JWT auth, it is adopted by configuration (§8.6) | Proposed |
| D712 | **Autoscaling** between `min_cu` and `max_cu`, where 1 CU = 1 vCPU and 4 GiB and the step is 0.25 CU. It uses Kubernetes in-place Pod resize and resizes Neon's local file cache through `/configure`, driven by `pg-control` from compute metrics. NeonVM is not used (§7.4) | Proposed |
| D713 | **Branching and PITR:** copy-on-write timelines; a branch at an LSN or a timestamp inside the history retention window (default 7 days); a restore is branch-and-swap that keeps a backup branch; branches can have a TTL and protection; a branch with children cannot be deleted (§10) | Proposed |
| D714 | **`loams-wal` is the only WAL service; stock safekeepers are removed** (owner directive, 2026-10-08). Durability is a quorum hot tier plus object storage (250 ms group commit). The TiKV hot tier (D234) is the default launch candidate and Arm A (D264) is developed alongside; a bake-off against absolute launch targets (§9.3) picks the store, and the owner rules (Q652). The in-process interpreted sender, broker publication, TLS, metrics, membership changes and the offload are launch-critical. Safekeepers remain only in the dev compose until its data is migrated (§9.8), and as a never-deployed benchmark reference. Pageservers run on object storage with at least two per AZ and secondaries (§9) | Proposed; reverses D233's gating and the earlier D714 text |
| D715 | **HA and DR targets:** RPO 0 for an AZ loss; RTO ≤ 60 s for a compute failure and ≤ 5 min for an AZ loss; bucket versioning and cross-region replication; a monthly automated restore drill (§11) | Proposed |
| D716 | **Upgrades:** a new compute image applies at the next start, and running computes are rolled within 7 days in a maintenance window. Storage components roll through the storage controller's drain and fill. PgDog rolls with connection draining. Fork releases follow D241. API changes are checked by `buf breaking`. A major Postgres upgrade at GA is dump and restore into a new branch, run by Loams (§12) | Proposed |
| D717 | **Observability and quotas:** `/metrics` on every component (`loams-wal` gains one), the `loams_pg_*` families, PgDog's OpenMetrics, OTel traces across the wake path, `pg_stat_statements` on by default, dashboards and alerts in `deploy/`; none of it billing-grade (D548). Quotas extend the §41 limits record and are enforced at `pg-control` admission, in PgDog's pool and connection limits, and through `neon.max_cluster_size` in the compute spec (§13, §14) | Proposed |
| D718 | **Kubernetes deployment:** a Helm chart `deploy/helm/loams-postgres` applied through MT3's GitOps waves (storage controller DB on CNPG, storage broker, pageservers, the `loams-wal` pool, `pg-control`, per-namespace PgDog). Computes go in `loams-pg-<ns>` namespaces with default-deny NetworkPolicy and an optional sandboxed RuntimeClass. A `LoamsPostgres` CRD in `loams-operator` (D185) comes later and does not block GA. An **external security review** is a GA gate (§15) | Proposed |
| D719 | **Single-node mode and the desktop contract:** `loams dev --postgres` runs `pg-control` with the local store, the compose runtime, one pageserver, one `loams-wal` acceptor, a storage broker, PgDog and local-filesystem remote storage. It advertises `loams.postgres.v1` in `GetInstance`. Loams Desktop uses the API when the active server advertises it, and D668's direct pageserver path otherwise (§18) | Proposed; amends D668 |

## 2. What "production ready" means

Loams Postgres is production ready when a team can do the following on a self-hosted or BYOC Kubernetes cluster, or on Loams Cloud, and the guarantees hold under faults:

1. Create a project, branch it, and connect with any mainstream driver or ORM through one hostname.
2. Leave it idle and pay only for bytes in the bucket: computes suspend and wake on connect.
3. Restore it to any second in the retention window.
4. Lose a pod, a node or an AZ without losing an acknowledged commit.
5. Upgrade every component without downtime beyond a reconnect.
6. See what it is doing, and have limits enforced.

§17 turns this into a checklist, and §16 sets the numbers. Today none of it holds: there is no control plane, no `loams-neon` crate, no compute start or stop, no routing, no metrics endpoint, and `conformance/router` marks Loams Postgres `pending-target` (§19).

## 3. Resource model (D701)

```
org ─► namespace ─► project ─────────────► Neon tenant (one Postgres cluster, its own branches, quotas, erasure)
                      ├─ branch ──────────► timeline (ancestor + LSN); default branch "main"
                      │    ├─ role, database (inherited by child branches; applied through the compute spec)
                      │    └─ endpoint ───► stable address + settings (rw: one per branch; ro: any number)
                      │          └─ compute ► one compute_ctl + Postgres process (disposable, replaced, never patched)
                      └─ settings: pg_version, history_retention, region, WAL pool
```

| Resource | Id | Neon object | Notes |
|---|---|---|---|
| Project | `prj-<ULID>`; the name is unique in the namespace | tenant: `tenant_id = SHA-256("loams/pg/tenant/" ‖ project_id)[0..16]` | §23 §3.2's one-tenant-per-database rule. Deriving the id from the immutable project id, not from the name, allows renames. A retried create maps to the same tenant |
| Branch | `br-<ULID>`; the name is unique in the project | timeline: `SHA-256("loams/pg/timeline/" ‖ branch_id)[0..16]`, `ancestor_timeline_id`, `ancestor_start_lsn` | `main` is created with the project |
| Endpoint | `ep-<ULID>` | none (Loams-only) | `type: read_write \| read_only`, `min_cu`, `max_cu`, `suspend_timeout_s`, `pool_mode`, `pg_settings`. At most one `read_write` endpoint per branch |
| Compute | `cmp-<ULID>` | `compute_id` in `compute_ctl`'s spec request | Created per start; the endpoint keeps the address |
| Role | the name, per branch | spec `cluster.roles` | The secret is never stored in clear (§8.6) |
| Database | the name, per branch | spec `cluster.databases` | The owner is a role of the same branch |

Child branches inherit roles and databases as they were at the branch point, because they are catalog state inside the timeline. `pg-control` copies the parent's records so that its own view matches.

## 4. The API (D702)

### 4.1 `loams.postgres.v1`

`proto/loams/postgres/v1/postgres.proto`, package `loams.postgres.v1`. The RPCs follow §44's naming (`<Verb><Noun>`):

| Group | RPCs | Long-running (returns `Operation`) |
|---|---|---|
| Projects | `CreateProject`, `GetProject`, `ListProjects`, `UpdateProject`, `DeleteProject` | Create, Delete |
| Branches | `CreateBranch` (parent, and `lsn` or `timestamp`, or the head), `GetBranch` (with WAL heads: `commit_lsn`, `flush_lsn`, `remote_consistent_lsn`, `backup_lsn`; logical size), `ListBranches`, `UpdateBranch` (name, TTL, `protected`), `DeleteBranch`, `RestoreBranch` (§10), `SetDefaultBranch` | Create, Delete, Restore |
| Endpoints | `CreateEndpoint`, `GetEndpoint`, `ListEndpoints`, `UpdateEndpoint`, `StartEndpoint`, `SuspendEndpoint`, `RestartEndpoint`, `DeleteEndpoint` | Start, Suspend, Restart, Delete |
| Roles | `CreateRole` (returns the password once), `ListRoles`, `ResetRolePassword` (returns it once), `DeleteRole` | — |
| Databases | `CreateDatabase`, `ListDatabases`, `DeleteDatabase` | — |
| Connect | `GetConnectionInfo` (host, port, database name in PgDog's grammar, role, `sslmode=verify-full`, CA bundle; **never a password**), `IssueConnectCredential` (D711) | — |
| Watch | `WatchProject` (snapshot, then changes to branches, endpoints and computes, then a heartbeat every 15 s; resumes from a cursor) | — |

- **Idempotency.** Every mutation takes `idempotency_key` (a client ULID, AP0 Ruling 5). A replay within 24 h returns the first response, including the same `Operation` id and, for `CreateRole` and `ResetRolePassword`, a reason `secret_already_issued` instead of the password a second time. Creates are also idempotent by name: a second `CreateBranch` with the same name and a different key returns `already_exists`.
- **Operations.** Operation kinds are `postgres.project.create`, `postgres.branch.create`, `postgres.branch.restore`, `postgres.endpoint.start`, `postgres.endpoint.suspend` and so on, with `target = {project, branch?, endpoint?}`. `WatchOperations` from `loams.operations.v1` works unchanged.
- **Errors.** Reasons are added to `docs/api/reasons.md`: `project_not_found`, `branch_has_children`, `branch_protected`, `lsn_out_of_retention`, `endpoint_exists_for_branch`, `compute_start_failed`, `quota_exceeded` (`RESOURCE_EXHAUSTED`), `storage_unavailable`.
- **Authorization.** Every RPC is checked in OpenFGA (D66, D67) against `pg_project:<id>` with the relations `viewer`, `editor`, `admin` and `connect`, inherited from the namespace. Agents may hold `viewer`, `editor` and `connect` but not `admin`. `DeleteProject` and `RestoreBranch` on a protected branch go through `loams.approvals.v1` when the caller is an agent.

### 4.2 Console REST

`/v1/namespaces/{ns}/postgres/projects[/{project}[/branches[/{branch}]|/endpoints[/{endpoint}][:start|:suspend|:restart]|/roles|/databases]]`. Each route maps to one RPC in `docs/api/route-map.md`. The `Idempotency-Key` header carries `idempotency_key`. This follows §44 §5.2.

### 4.3 The upcall API Neon's components call (internal)

Served on a separate listener, never exposed to tenants. These are §28 §5.2's endpoints, unchanged:

| Caller | Endpoint | Auth |
|---|---|---|
| `compute_ctl` | `GET /compute/api/v2/computes/{compute_id}/spec` (`status: empty` keeps a warm-pool compute waiting, §7.3) | Bearer JWT, one per compute, signed by `pg-control`'s key, JWKS published to `compute_ctl_config` |
| Storage controller | `PUT /notify-attach` (`notify-safekeepers` is unused: the storage controller manages no safekeepers, §9.1) | mTLS, or a static token from a Secret |
| Neon proxy (only in the D709 fallback) | `GET /get_endpoint_access_control`, `GET /wake_compute`, `GET /endpoints/{e}/jwks` | mTLS |

## 5. Where it lives: open source and private (D703)

D220 put "the control plane" in `loams-platform`. D540 then opened the multi-tenant control plane, and D557 defined the hosted cloud as the open control plane plus a private commercial layer. Loams Postgres follows D540 and D557:

| Open, Apache-2.0, this repository | Private, `loams-platform` |
|---|---|
| `loams.postgres.v1` and all its handlers | The commercial API a hosted tenant calls first: plan, entitlement and payment checks, which then call `loams.postgres.v1` with a service credential (D557) |
| `pg-control`: reconcilers, upcalls, waker, autoscaler, compute runtimes, store | The plan → limits mapping that writes the limits API (D546) |
| PgDog config rendering and reload; `deploy/pgdog/` | Billing-grade compute-seconds, CU-hours, storage-byte-hours and data transfer: the record, its delivery and its reconciliation (D548) |
| Quota **enforcement** (§14) and the limits fields | Pricing pages, invoices, credits |
| Generic metrics, traces and logs (§13) | Hosted-only fleet operations: capacity planning across regions, the hosted on-call automation |
| `ComputeLifecycleObserver`: a Rust trait called with a plain struct (`org`, `namespace`, `project`, `endpoint`, `compute`, `event: Started \| Resized{cu} \| Suspended \| Failed`, `at`), no wire format, no buffer, no persistence, as D549's `InvocationObserver` | The observer implementation that turns those events into billing records, linked in at build time |
| Helm chart, single-node mode, desktop integration | — |

The dependency runs one way (D551): nothing here names `loams-platform`, and `scripts/ci/no-metering.sh` (MT4 Task 8) also scans `loams-pg-control`.

## 6. The control plane (D704–D706)

### 6.1 Processes

```
                         console / CLI / SDKs / desktop / agents
                                        │ Connect + REST (gateway auth, OpenFGA)
                                        ▼
 ┌───────────────────────────── pg-control (role of `loams`, ×2+, stateless) ─────────────────────────────┐
 │ PostgresService │ reconcilers (project, branch, endpoint, compute) │ waker │ autoscaler │ PgDog push   │
 │ upcall listener (spec, notify-attach)                               │ PgControlStore (TiKV | local)    │
 └──────┬──────────────────────┬──────────────────────┬─────────────────────┬────────────────────────────┘
        │ loams-neon           │ ComputeRuntime        │ PgDog admin + config │ storage controller API
        ▼                      ▼                       ▼                      ▼
   pageservers,          compute Pods /          PgDog (per namespace)   storage controller ──► pageservers
   loams-wal pool        compose services        ──► compute Services     (its DB on CNPG)
```

- **`loams-neon`** (N1) speaks HTTP and JSON to the storage controller (or a pageserver directly in single-node mode), `loams-wal`, and `compute_ctl`. It keeps its own copies of the few Neon request and response structs (`TimelineCreateRequest`, `ComputeSpec`, `ControlPlaneConfigResponse`), tested against recorded fixtures from the pinned fork image. It has no Neon code dependency.
- **`loams-pg-control`** holds the service, the reconcilers, the waker, the autoscaler, the PgDog integration and the store. It links `loams-sqlrouter-io`'s PgDog renderer and admin adapter (RT1) so that sharded and unsharded databases share one config path.
- **The `pg-control` role** of the `loams` binary (`loams pg-control --store tikv --runtime kubernetes`) runs it. Single-node mode runs the same code inside `loams dev --postgres` (§18).

### 6.2 State (D705)

| Record | Key (`loams-meta-tikv`) | Contents |
|---|---|---|
| Project | `x/<ns>/<project_id>`, name index `x/<ns>/n/<name>` | §28 §5.3's database record: `engine = loams-pg`, tenant id, `pg_version`, `wal`, `history_retention_s`, region, settings, `state`, version |
| Branch | `X/<project_id>/<branch_id>` | timeline id, parent, `ancestor_lsn`, name, `ttl`, `protected`, pageserver shard map from `notify-attach`, `state`, version |
| Endpoint | `E/<project_id>/<endpoint_id>` | branch, type, CU range, suspend timeout, pool mode, desired state (`running` or `suspended`), current `compute_id`, backend address, version |
| Compute | `C/<compute_id>` | §28 §5.3: spec version, status, endpoint, JWT key id, CU, created and last-active times |
| Role | `R/<branch_id>/<role>` | credential-store reference (never the secret), `login`, `pool_mode`, version |
| Database | `D/<branch_id>/<db>` | owner role, version |

The prefixes `x/`, `X/` and `C/` are unused in `crates/loams-meta-tikv/src/keys.rs` today (checked 2026-10-08), and so are `E/`, `R/` and `D/`. PG2 Task 0 checks again. The local backend (redb under the engine's data directory) implements the same trait. `pg_control_store_conformance!` runs the CAS, absence, listing and fencing cases against both, the same way `metastore_conformance!` does.

### 6.3 Reconcilers (D706)

- **Level-triggered.** Each reconciler reads the desired record, observes the world (the storage controller's tenant and timeline, the runtime's status, PgDog's `SHOW DATABASES`), and makes one idempotent step toward the desired state. It is triggered by record changes, by the waker, and by a 30 s resync.
- **One actor per project.** A lease `e/pg/<project_id>`, with the epoch checked inside every write transaction (`check_fence`), so a paused instance cannot act on stale state.
- **Sagas** for flows that must compensate (branch restore, a quiesced WAL move, project delete with its erasure receipt, §18 §9 of the metastore design) run on Loams Durable, as §23 §6.5's branch-per-workspace saga does.

## 7. Compute lifecycle (D707, D712)

### 7.1 States

```
          StartEndpoint / wake                compute_ctl /status = running
 idle ────────────────────────► starting ──────────────────────────────────► running
   ▲                               │ timeout 60 s or crash ×3                │  idle ≥ suspend_timeout
   │                               ▼                                         ▼  or SuspendEndpoint
   │                             failed ◄───────────── crash ─────────── suspending ── /terminate ──► suspended
   │                                                                                                      │
   └──────────────────────────────────── wake (waker, API, console) ─────────────────────────────────────┘
```

- **Idle detection.** The compute reports activity through `compute_ctl`'s `/status` (`last_active` (verify)). `pg-control` also counts PgDog's active and waiting clients for the endpoint. A compute suspends when both have been idle for `suspend_timeout_s`: 300 s by default, 60 s at minimum, and 0 to never suspend. Logical replication slots with an active subscriber keep a compute running.
- **Replace, never restart.** A new compute gets a new `compute_id` and spec, as §23 §3.2 says. The endpoint's address does not change.
- **Failure.** Three crashes within 10 minutes put the endpoint into `failed`, with the reason and the log location in `GetEndpoint`. A `StartEndpoint` call clears it.

### 7.2 Runtimes

| Runtime | Used by | What a compute is | Address |
|---|---|---|---|
| `compose` | single-node mode, the desktop, CI | A service in a compose project per Loams namespace (`docker compose` or `podman compose`, as AP1e Task 21 detects them), image pinned by digest | A host port per endpoint, recorded in `E/` |
| `kubernetes` | production | A Pod (not a Deployment) in `loams-pg-<ns>`, with labels `loams.dev/{org,namespace,project,endpoint,compute}` (§27 §3.2), resource requests from the CU, the compute TLS certificate mounted, and `--control-plane-uri` pointing at the upcall listener | A ClusterIP Service `ep-<id>` per endpoint, whose selector names the current compute; PgDog connects to the Service |

**Why Pods and not Git.** §41's rule that Git is the only write path to a cluster is about desired configuration. Computes start and stop every few minutes per endpoint, much as Knative's autoscaler creates Pods without a commit. `pg-control` uses a ServiceAccount whose Role allows only Pods, Services and Secrets in `loams-pg-*` namespaces. The namespaces, their NetworkPolicies, quotas and PgDog Deployments still come through Git (MT4).

### 7.3 Cold starts and the warm pool

- **The warm pool.** `pg-control` keeps N unbound compute Pods per (region, `pg_version`, image) running `compute_ctl` with no spec. Its spec endpoint answers `status: "empty"` (§28 §5.2), so they wait. A wake binds one: it records the compute against the endpoint, answers its next spec poll (or pushes `/configure`), and points the endpoint's Service at it. N is sized from the wake rate. The default is 2, and 0 turns the pool off.
- **Targets (§16):** from the first client connection on a suspended endpoint to the first query result through PgDog, p50 ≤ 2 s and p99 ≤ 5 s with the warm pool, and p99 ≤ 15 s without it, on Kubernetes. On compose, p99 ≤ 20 s (target).

### 7.4 Autoscaling (D712)

- **The unit:** 1 CU = 1 vCPU and 4 GiB of RAM, in steps of 0.25 CU. The ranges are `0.25 ≤ min_cu ≤ max_cu ≤ 16` by default, and the limits record (§14) caps them.
- **The mechanism:** Kubernetes in-place Pod resize (the `resize` subresource, GA in Kubernetes 1.35 (verify)) for CPU and memory, plus `/configure` with a new `neon.file_cache_size_limit` for Neon's local file cache (verify that it reloads without a restart). `shared_buffers` stays fixed at the start CU, as in Neon, and the file cache absorbs the growth.
- **The policy:** in `pg-control`, every 5 s. It scales up when CPU is above 70 % for 15 s or the file cache's working set exceeds 75 % of its size, and down when CPU is below 30 % for 2 minutes. The step is at most ×2 up and −25 % down per decision. A refused resize (node capacity) is logged and retried. A move to another node only happens when the endpoint restarts.
- **Not used:** NeonVM and Neon's autoscaler agent. They need KVM nodes, which BYOC clusters and Autopilot-class clusters lack (Q644).

## 8. PgDog: routing, pooling, auth and wake-on-connect (D708–D711)

### 8.1 Shape

```
 client ──TLS, SNI = <ns>.<region>.pg.<base-domain>──► L4 SNI-passthrough listener (Envoy/Kourier/Traefik TCP route)
                                                          │ TLS untouched
                                                          ▼
                     PgDog Deployment in loams-pg-<ns> (×2, unmodified image pinned by digest)
                     TLS termination ─ SCRAM ─ pools ─ route by database name ─ read/write split ─ health checks
                                                          │ TLS (cluster CA) or mesh mTLS
                     ┌────────────────────────────────────┼────────────────────────────────────┐
                     ▼                                    ▼                                    ▼
          Service ep-<rw endpoint>             Service ep-<ro endpoint>              (§31: shard computes)
          ──► the compute Pod                  ──► replica compute Pods
                     ▲
     pg-control: renders pgdog.toml + users.toml ─► ConfigMap + Secret ─► RELOAD; waker reads SHOW POOLS
```

- **One PgDog per Loams namespace** (D708). Database names are then only unique within a namespace, which is where users choose them. A config error, a reload or a noisy tenant stays in one namespace. Idle namespaces still cost two small PgDog Pods. A shared regional PgDog with globally unique names is Q641.
- **Hostname routing.** The hostname picks the namespace, through SNI at an L4 listener that passes TLS through. **The database name picks the project, branch and database** inside PgDog:

  | Client `dbname` | Routes to |
  |---|---|
  | `<project>` | the project's default branch, its default database, its read-write endpoint |
  | `<project>__<branch>` | that branch's read-write endpoint, default database |
  | `<project>__<branch>.<database>` | that branch, that database |
  | `<project>__<branch>__ro` | the branch's read-only endpoints, load-balanced |

  This extends §28 §8's `<db>__<branch>` (Q49). Clients that cannot send SNI (old libpq, some JDBC setups) connect to the namespace's dedicated port on the same listener instead, which PG2 Task 22 defines. Per-endpoint hostnames in the Neon style are not offered. If PgDog gains SNI-based routing upstream (verify), they could be added by configuration.
- **Pooling.** Transaction mode by default. A role whose `pool_mode` is `session` gets session pooling, for migrations, `LISTEN`/`NOTIFY`, advisory locks and session-level prepared statements beyond PgDog's support (PgDog sets pool modes per user (verify), with a per-database alias as the fallback). Pool size per (role, database) is `min(max_connections of the compute's CU × 0.8, the role's limit)`, rendered from the CU.
- **Reads.** Read-only endpoints are PgDog `role = replica` hosts for the branch. `__ro` names go only to replicas. On the primary name, PgDog's read/write split is **off by default**, because it changes read-your-writes semantics for applications that did not ask for it. A project setting turns it on.
- **Health and failover.** PgDog's health checks and bans mark a backend down. Failover of a read-write endpoint is `pg-control` replacing the compute behind the same Service (§11), so PgDog's config does not change. PgDog's `ban_timeout` is rendered short (1 s, verify the field name) so a replaced compute is used at once.
- **Sharding.** A project created with a shard map uses §31's design: the shard map in `xs/<ns>/<db>` (D304), PgDog's sharding, `ConfigPush` and Loams' cutover (D305), and 2PC only under D306. PG2 renders unsharded projects only and accepts RT1's renderer for sharded ones. Sharded GA follows track RT.

### 8.2 Config management

- **Rendering.** It is deterministic: the same records produce byte-identical files, under golden tests. `pgdog.toml` holds `[[databases]]` per routed name, with host, port, `database_name`, `role` and pool settings, plus `[general]` (TLS paths, `checkout_timeout`, `ban_timeout`, `healthcheck_interval`, OpenMetrics port). `users.toml` holds one entry per (role, routed name) that the role is granted.
- **Authorization before routing.** A role appears only for the routed names its grants allow (§23 §6.3), so PgDog refuses an unauthorized database before it opens a backend connection.
- **Push.** The kubelet can take a minute or more to refresh a mounted ConfigMap, which is too slow for credential exchange (§8.6). So, on Kubernetes:
  1. A small Loams sidecar in the PgDog Pod, `pgdog-config-sync` (Apache-2.0, a separate container, so PgDog stays unmodified), receives the rendered files from `pg-control` over mTLS.
  2. It writes them atomically to an `emptyDir` that PgDog reads, and reports the applied generation.
  3. `pg-control` then sends `RELOAD` to that replica through the admin database.

  The ConfigMap and Secret stay the persisted copy, which the sidecar loads at Pod start. On compose, `pg-control` writes the files to a bind mount and sends `RELOAD`. Pushes are debounced to at most one per 500 ms per namespace. `ConfigPush` (RT1) records the generation each instance has applied. PG2 Task 18 checks that PgDog re-reads both files on `RELOAD` (verify).
- **Drift.** The resync compares `SHOW DATABASES` and `SHOW USERS` on each instance with the rendered generation and re-pushes on a mismatch.

### 8.3 The licence boundary (D236, restated)

PgDog is **AGPL-3.0** (`LICENSE` is the GNU AGPL v3 text, no CLA; verified 2026-09-29, §28 §8; PG2 Task 18 checks again and records the pinned version). Loams uses it **as an unmodified, separate process**:

- No PgDog crate in any `Cargo.lock`; `deny.toml` already rejects AGPL. No PgDog source in any Loams repository. `conformance/router` stores no PgDog text (D318).
- No patches. A needed fix goes upstream (with the owner's approval) or is worked around in configuration. If neither works, the fallback is used (§28 §8: Neon's proxy, PgBouncer).
- No PgDog plugins: a plugin loaded into the PgDog process would form one program with it.
- Images are pinned by digest from `ghcr.io/pgdogdev/pgdog`. Wherever Loams **distributes** a bundle that includes the image reference (BYOC chart, the single-node compose file, the desktop's stack), it ships PgDog's licence text and a link to the exact upstream source tag, recorded in `LICENSES.md` and `deploy/pgdog/README.md`. The licence-check job (D-SC-6) checks deploy images.
- PgDog's closed Enterprise Edition is not used.

### 8.4 Wake-on-connect (D709, answers Q113)

PgDog cannot wake a compute (§28 §8). Loams does it from outside the data path:

1. A suspended endpoint keeps its Service (or compose port). With no compute behind it, a connection from PgDog is refused at once.
2. PgDog holds the client in its pool's wait queue for up to `checkout_timeout`, rendered as 15 s, while it retries the backend (verify: Task 0 measures this behaviour on the pinned version).
3. **The waker** in `pg-control` polls `SHOW POOLS` on each PgDog instance every 100 ms while any endpoint in the namespace is suspended, and every 1 s otherwise. It also reads the backend error counters. When `cl_waiting > 0` on a pool whose endpoint is `suspended`, it wakes the endpoint (§7.3). The wake is idempotent per endpoint, so several PgDog instances seeing the same waiter cause one start.
4. Once the compute is `running`, the Service selects it and PgDog's next retry succeeds. The waiting client never saw an error.
5. **Proactive wake.** `GetConnectionInfo`, the console's SQL tab, and the `loams` CLI's `pg connect` start the endpoint while returning, which hides most of the cold start.

**If step 2 does not hold** (PgDog fails the client at once on a refused backend), the fallback is chosen in PG2 Task 18 in this order:

1. PgDog configuration that keeps the client queued (a longer `connect_timeout` with retries).
2. A Loams **wake shim**: a small TCP listener that the suspended endpoint's Service points at. It accepts PgDog's backend connection, wakes the compute, then splices the bytes unchanged to the compute. TLS stays end to end between PgDog and the compute.
3. Neon's proxy in front of PgDog, with Loams' `wake_compute` (§28 §5.2).

### 8.5 TLS (D710)

- **Client → PgDog:** TLS 1.2 or later is required. The non-TLS listener is not exposed. The certificate is `*.<region>.pg.<base-domain>`, from cert-manager, held by PgDog. `sslmode=verify-full` works because the namespace hostname matches. Self-hosts may bring their own CA, and single-node mode uses a local CA (§18).
- **PgDog → compute:** TLS with full verification of the compute's certificate (`ep-<id>.loams-pg-<ns>.svc`, issued per compute from a cluster CA by `pg-control`), or mesh mTLS where a mesh is installed. The compute's `pg_hba` accepts `hostssl` only.
- **Storage plane:** pageserver `auth_type = NeonJWT` and `loams-wal` JWT auth (Task 38); the compute's `storage_auth_token` is scoped to its tenant; `compute_ctl`'s API takes a per-compute JWT (§4.3).
- **`loams-wal`:** today it has a bearer token and `--trusted-network`, and no TLS (§19). It must gain rustls on both listeners before launch; a mesh with mTLS is acceptable in addition, not instead (Task 38).

### 8.6 Authentication (D711)

- **SCRAM-SHA-256** at PgDog, from `users.toml`. Passwords are 32 random bytes in base64url, generated by `pg-control` and shown once (`CreateRole`, `ResetRolePassword`). The credential store (Q30) keeps them, encrypted, because PgDog needs a usable secret for its own server-side login (verify whether PgDog accepts a SCRAM verifier in `users.toml`, Q643). The Secret that carries `users.toml` is readable only by that namespace's PgDog ServiceAccount, and KMS-encrypted at rest where the cluster supports it.
- **JWT and OIDC through credential exchange.** A person signs in with Authentik (OIDC, D447–D449) or an agent holds a Loams token (§19 §5). Either calls `IssueConnectCredential{project, branch, role?, ttl}`. After the OpenFGA `connect` check, `pg-control` creates (or reuses) a login role `tok_<hash(sub)>` with `VALID UNTIL now() + ttl`, a fresh password, and membership in the requested role. It renders it into `users.toml`, pushes, and returns `{connection, password, expire_time}` (`connection.user` is the login; PG2 Task 1 ruling R1.11). The password is answered once; the idempotency ledger keeps only that it was issued. The default TTL is 15 minutes and the maximum 1 hour. Expired roles are dropped by the reconciler. This is the RDS IAM authentication pattern, without changing PgDog. Native JWT in PgDog, if it exists upstream (verify, Q642), replaces the exchange by configuration.
- **Agents** are principals with `connect` grants on specific projects or branches. They never receive a long-lived role password (§19 P5, D295).
- **Network controls.** Per-project IP allow-lists are rendered as PgDog's allowed-address settings where PgDog supports them (verify), and otherwise enforced at the L4 listener. `block_public_access` hides the project from the public listener and leaves it reachable only through private networking (§43).

## 9. Storage and the WAL: `loams-wal` only (D714)

### 9.1 What the owner's directive changes

On 2026-10-08 the owner ruled "remove safekeepers". Loams Postgres launches on **`loams-wal`** (`crates/loams-safekeeper`): a quorum hot tier with object storage behind it.

- **`loams-wal` is the only WAL service** in every production, BYOC and single-node artifact, and, after PG2 Task 43, in the dev stacks too.
- **Stock Neon safekeepers are removed from the design.** That means:
  - no safekeeper StatefulSet;
  - no storage-controller safekeeper scheduling: `notify-safekeepers` is unused, and the storage controller runs with no safekeepers registered (verify that it does not require them);
  - no per-project `wal` setting;
  - no safekeeper fallback.
- **They survive in two places only:**
  - `deploy/neon`, until Task 43 migrates its data and replaces them;
  - the `sk` reference profile of `deploy/loams-pg-bench`, which is never deployed, so that each result can still be compared with Neon's own WAL (Q654).
- **This reverses D233's "default only if the gate passes".** The gate becomes a launch gate with absolute targets (§9.3), and D714 records the reversal.
- **The feeder goes too, because it needs a stock safekeeper** (§28 §6.7, D271). So the in-process interpreted sender (Q112) is no longer optional. Broker publication, TLS, metrics, membership changes and the offload are all launch-critical as well (§19 lists what `loams-wal` lacks today).

### 9.2 Architecture at launch

```
 compute (walproposer, unmodified) ──safekeeper protocol v3──► loams-wal pool ──► hot tier (quorum)
                                                                   │  group commit, 250 ms
                                                                   ├──► bucket: pgwal/<tenant>/<timeline>/<begin>-<end>.lwal
 pageserver ◄──START_REPLICATION (interpreted, in process)─────────┤
 storage broker ◄── SafekeeperTimelineInfo every 1 s ──────────────┘
```

- **Durability.** A commit is acknowledged once the hot tier's quorum holds it. The WAL is group-committed to the bucket every 250 ms. Trimming waits for `min(backup_lsn, remote_consistent_lsn, commit_lsn)` (D240, D269).
- **Two hot-tier stores sit behind one `WalStore` trait:**

| Store | Quorum and fence | Laptop results (rf3, three runs; §28 §7.1–§7.3, `bench/results/2026-10-01-raw/`) |
|---|---|---|
| **TiKV hot tier**: `tikv` (TxnKV 1PC) and `tikv-raw` (fenced, pipelined) (D234, D237–D240) | TiKV Raft, 3 replicas across AZs. One logical acceptor per timeline in a stateless pool (D239). The fence is a write-write conflict on the head key | Behind the safekeeper reference: `commit-1` 58–87 TPS against 109–117; `commit-16` 474–712 against 814–983; `bulk` 8–18 MB/s. Leader placement (`txn-placed`) was worse, at 30 TPS (unexplained, Task 35) |
| **Arm A**: `nvme` (D263–D272) | walproposer's own Paxos over 3 acceptors on local NVMe. TiKV holds only metadata, votes and the offload lease (D268, D269) | Passes `commit-16` and `tpcb-16` against the reference in every tier, and `commit-1` with SQPOLL. **`bulk` is 8–32 MB/s against 45–228.** SQPOLL costs 19–30 ms of CPU per `commit-1` transaction |

- **D714's choice.** The owner's directive names the TiKV hot tier, so it is the **default launch candidate**. Arm A is developed alongside it, because it leads on latency today. Task 42's bake-off on the reference topology measures both against §9.3. The owner then picks the launch store (Q652). Whichever ships, the other stays behind the trait, and a project can move between pools by the quiesced move of §9.5.

### 9.3 Launch targets (absolute; D714)

The targets apply on the reference topology (§16.3). Task 30 calibrates them, and the owner confirms them (Q653).

| Measure | Target |
|---|---|
| `commit-1` | p50 ≤ 3 ms, p99 ≤ 10 ms |
| `commit-16` | ≥ 2,500 TPS, p99 ≤ 15 ms |
| `tpcb-64` (`-s 100`), 4 CU | ≥ 3,000 TPS, p99 ≤ 60 ms |
| `bulk` (one 1 GB transaction) | ≥ 100 MB/s sustained per timeline |
| Aggregate | ≥ 400 MB/s per WAL node across 32 active timelines |
| WAL CPU | ≤ 300 µs per transaction at `commit-16`; ≤ 10 ms per MB on `bulk`; ≤ 2 % of a core per idle shard |
| Failover | Longest commit stall ≤ 2 s when an acceptor (Arm A) or a TiKV region leader is killed |
| Durability | No acknowledged commit lost under §9.7's fault catalog |
| Reference | Each run also reports the stock-safekeeper reference profile, for information only (Q654) |

### 9.4 The bottlenecks, and how each is attacked (PG2d)

**`bulk` throughput (every store, every tier).** Arm A spends only 7–9 ms of WAL CPU per MB and the TiKV stores little more, so the WAL process is mostly *waiting*. Task 33 tests the suspects in this order, and records each result before changing code:

1. **The feeder path.** The pageserver ingests through a second stream to a `--no-sync` stock safekeeper. walproposer's backpressure (`max_replication_write_lag = 500MB`, `max_replication_flush_lag = 10GB` in `deploy/loams-pg-bench/compute/config.json`) is driven by the pageserver feedback that `loams-wal` relays, so a slow feeder slows the compute. The in-process interpreted sender (Task 31) removes this path.
2. **Ack granularity and walproposer's in-flight window:** how far walproposer sends ahead of `AppendResponse`s (verify in the fork's `walproposer.c`), and whether `loams-wal` acknowledges per flush unit or per request.
3. **Journal shape:** units of at most 1 MiB, `--io-depth 4`, `O_DSYNC` per unit, and the 4 KiB padding per unit.
4. **Reads competing with writes:** the feeder's `pread`s on the same journal device.
5. **TiKV stores:** one append in flight per timeline (`tikv`); a TSO round trip, a fenced head read and a prewrite per append; the 1 MiB `raft-max-size-per-msg`.

**SQPOLL CPU.** Each shard's ring has its own kernel polling thread that spins. The fixes, in Task 36:
- share one SQPOLL thread across the shards' rings (`IORING_SETUP_ATTACH_WQ`);
- shorten `--uring-sqpoll-idle-ms`;
- keep SQPOLL **off by default** unless it meets the CPU target. Plain `uring` already meets `commit-16` and `tpcb-16`.

**TiKV levers, raw versus txn (Tasks 34–35):**
- reuse the previous commit timestamp as the next `start_ts` (`begin_at(ts)` in the `client-rust` fork);
- several fenced transactions in flight per timeline (Q115);
- `BatchCommands`-style RPC batching (Q118);
- shorter Raft ticks for WAL stores (Q117);
- leader placement in the compute's AZ, explaining the `txn-placed` regression first;
- raw pipelined (d8, d32) against TxnKV 1PC, measured on three PLP nodes (Q114).

**Group commit.** Arm A already group-commits per shard across timelines (D265). The TiKV store batches a timeline's queue into one transaction of up to 1 MiB. Task 34 adds pipelining (D267) to the TiKV store and measures the batch size and the linger time.

### 9.5 HA, failover and fencing

- **Arm A.**
  - Losing one of the three acceptors stalls nothing.
  - Replacing one is a membership change (Q261): `pg-control` writes the new member set to TiKV, the new acceptor resyncs from the bucket and a peer, and walproposer is given the new configuration and generation (verify the fork's walproposer `mconf` support).
  - Terms are fenced by a durable vote before the in-memory term rises (D264, D268).
- **The TiKV store.**
  - Losing a pool instance is a reconnect (`neon.safekeeper_reconnect_timeout = 100`).
  - Losing a TiKV region leader stalls the timeline until the election. Shorter ticks bound that (Q117; the target is ≤ 2 s, §9.3).
  - The fence is the head-key write-write conflict (§28 §6.4).
- **A deposed compute** (an old compute still running after a new one started on the same branch) gets no further acknowledgement from either store. Task 39 tests this.
- **The quiesced move** is §28 §6.10's procedure, kept for three uses: between `loams-wal` pools or stores, from a stock safekeeper into `loams-wal` (dev migration only, §9.8), and for rollback. The steps:
  1. Stop the compute and wait until `commit_lsn = flush_lsn`.
  2. Create the timeline in the target at that LSN, with the term history.
  3. Re-spec the compute and start it.
  4. Delete the source once `remote_consistent_lsn` has passed the switch point.

### 9.6 PITR and recovery from `loams-wal` and object storage

- **PITR within retention** is a pageserver branch at an LSN or a timestamp (§10). The pageserver keeps layers covering `pitr_interval`.
- **The `.lwal` objects are the WAL archive.** They are kept for the history retention plus 7 days, and they serve three purposes:
  1. **Hot-tier loss.** `recover_from_bucket` rebuilds the timeline up to `backup_lsn`, and the compute restarts there. The RPO is the 250 ms group-commit interval.
  2. **Pageserver rebuild.** When a timeline's layers are lost or corrupt, the pageserver re-ingests from `loams-wal`, which serves trimmed ranges from the bucket (§28 §6.7, source 3).
  3. **An independent restore path:** a weekly `basebackup` from the pageserver into the bucket, plus `.lwal` replay into a vanilla Postgres of the same major version (`loams pg archive-restore`). This is the second line of defence, in place of logical dumps, and does not depend on Neon's layer format.
- **The monthly restore drill** (§11) exercises (1) and (3).

### 9.7 The chaos fault catalog (nemesis)

**Faults:**
- killing one acceptor, then two (availability may be lost; data must not be);
- `SIGSTOP` pauses;
- partitions between compute and acceptor, acceptor and TiKV, and acceptor and bucket;
- disk latency and stalls (`dm-delay`), `fsync` errors (a test I/O hook in the journal), and torn writes (the journal's recovery tests);
- killing a TiKV region leader, and the PD leader;
- a bucket outage, during which the offload backlog is bounded and alerts;
- killing a pageserver, to exercise backpressure;
- clock jumps, for the leases.

**Checkers:**
- bank and list-append (Elle-style) checkers, run through `loams-nemesis` (RT5) when it exists, or Task 41's minimal harness until then;
- the acceptor state machine under `loams-detsim` (RT1) with the same faults;
- Neon's TLA+ model replayed as trace tests (§28 §12 row 2).

### 9.8 Dev data migration, and removing safekeepers from dev

- **What exists today.** Data sits on stock `safekeeper1` timelines in `deploy/neon` stacks and in the desktop's local stacks (D667).
- **The migration tool.** `loams pg migrate-wal --pageserver <url> --safekeeper <url> [--tenant t --timeline tl] [--dry-run]` runs §9.5's quiesced move into a `loams-wal`, then checks it with table checksums. The fallback is `pg_dump`/`pg_restore` into a new project.
- **Then the stacks change.** `deploy/neon` replaces `safekeeper1` with `loams-wal --store nvme` (one acceptor, D268's local metadata backend, the in-process interpreted sender), and the desktop's stack follows. In `deploy/loams-pg-bench`, stock safekeepers stay only behind the `sk` reference profile (Q654).
- **A CI guard.** `scripts/ci/no-safekeeper.sh` fails when a production artifact (the Helm chart, the single-node compose, the desktop stack, or `deploy/neon` after Task 43) runs Neon's `safekeeper` binary.

### 9.9 The pageservers

- **Pageservers on object storage.** Layers go to the bucket (RustFS for self-hosts, S3, GCS or Azure Blob through Neon's `remote_storage`). The local NVMe is a cache. The storage controller places tenants, issues generations and fails over. There are at least 2 pageservers per AZ, with **secondary locations** keeping a warm cache for fast failover (verify the fork's support and settings). A tenant above 64 GiB of logical size (target) is split into shards by the storage controller.
- **The storage controller** runs from the fork, with its database on a two-instance CNPG Cluster (Q48, §28 §5.2). It runs 2 replicas, with leadership through that database (verify, Q646). It moves to TiKV in P5 only.

## 10. Branching and PITR (D713)

- **Branches** are copy-on-write timelines: `CreateBranch` with `parent` and either `lsn`, `timestamp` (resolved with the pageserver's `get_lsn_by_timestamp` (verify the route)) or the parent's head. Creation does not depend on the database's size: target p99 ≤ 2 s to `ready`, without a compute.
- **History retention** is the tenant's `pitr_interval`: 7 days by default, settable per project up to the limit (§14). `lsn_out_of_retention` is returned for older targets.
- **Restore** (`RestoreBranch{branch, lsn | timestamp | source_branch}`) is a saga: create a new timeline at the target, rename the old one to `<branch>_old_<ts>` (a backup branch with a 7-day TTL), give the new timeline the branch's id and name, and move the branch's endpoints onto it. Connections drop once. The backup branch makes the restore itself reversible.
- **TTL and protection.** `ttl` deletes a branch when it expires (§23 §6.5's GC schedule). A `protected` branch cannot be deleted or restored without `admin`, and an agent needs an approval for it. `DeleteBranch` on a branch with children returns `branch_has_children`.
- **Not supported:** merging branches (D155), and schema-only branches.

## 11. HA, backups and DR (D715)

| Component | Replicas | Failure | Recovery |
|---|---|---|---|
| Compute (read-write) | 1 | Pod or node loss | `pg-control` replaces it behind the same Service. RTO ≤ 60 s (target), most of it the cold start. Optional hot standby: a read-only endpoint promoted through `compute_ctl /promote` (§28 §5.1) |
| PgDog | 2 per namespace, spread across AZs | Pod loss | The L4 listener routes to the other replica; clients reconnect |
| `pg-control` | 2+ | Instance loss | The project lease moves within one lease period (10 s) |
| `loams-wal` (Arm A) | 3 acceptors across AZs | One lost | No stall (quorum 2 of 3); replacement by membership change (§9.5) |
| `loams-wal` (TiKV store) | pool ×2 per AZ; TiKV ×3 across AZs | Instance or TiKV leader lost | Reconnect in 100 ms; leader election stall ≤ 2 s (target, Q117) |
| Pageservers | ≥ 2 per AZ, with secondaries | One lost | The storage controller fails tenants over to the secondaries and pushes `notify-attach` |
| Storage controller | 2 | Leader loss | Leadership through its database (verify) |
| Storage broker | 2 | Loss | Stateless |
| Object storage | the provider's | Region loss | Cross-region replication of the `neon` bucket. RPO = `loams-wal`'s group-commit interval (250 ms) plus the bucket's replication lag (Q647) |

- **Backups.** The data is already in the bucket, as layers and WAL. Bucket **versioning** with a 7-day noncurrent retention guards against a bad delete or a corrupt layer upload. Cross-region replication covers region loss.
- **Logical exports** are optional, per project: a scheduled `pg_dump` to a separate bucket, as a second line of defence that does not depend on Neon's formats (Q653).
- **Restore drill.** Monthly and automated: restore a branch at a random timestamp in a scratch project, run checksums on a known table set, and check a logical export with `pg_restore --list`. A failure pages someone.

## 12. Upgrades (D716)

- **Compute images** (Postgres minors, extensions, `compute_ctl`) apply to the next start. Running computes older than 7 days on an outdated image are restarted in the project's maintenance window (default Sunday 02:00–04:00 in the region's time zone). Security releases may shorten that to 24 h, with a notice.
- **Pageservers** roll one at a time through the storage controller's drain and fill (`/control/v1/node/{id}/drain` (verify)). **`loams-wal`** rolls one acceptor (Arm A, keeping two of three) or one pool instance (TiKV store) at a time, waiting for the timelines to catch up between steps.
- **PgDog** rolls with `maxUnavailable: 0`. Clients on a terminating Pod see a reconnect.
- **`pg-control`** rolls freely. The API is versioned under `buf breaking`, and a breaking change needs `v2` beside `v1` for one minor release (D616).
- **Fork releases** follow D241: quarterly rebases, minors within a week of upstream. *(Superseded by §51 D810–D811: releases `nf-YYYY.MM.N`, weekly and nightly syncs by merge, Postgres minors within 7 days.)*
- **Major Postgres versions.** At GA, `UpgradeProject{target_version}` creates a branch on a new project at the new version and copies the data with `pg_dump | pg_restore`, then swaps endpoints. Downtime equals the copy time and is announced. In-place `pg_upgrade` support on the fork is later work (Q648).

## 13. Observability (D717)

| Source | Endpoint | Key series |
|---|---|---|
| `pg-control` | `/metrics` | `loams_pg_endpoints{state}`, `loams_pg_compute_starts_total{cause=api\|wake\|pool\|replace}`, `loams_pg_cold_start_seconds` (histogram, connect-to-ready), `loams_pg_wake_detect_seconds`, `loams_pg_reconcile_errors_total{kind}`, `loams_pg_pool_size{state}`, `loams_pg_autoscale_decisions_total{dir}`, `loams_pg_pgdog_push_seconds`, `loams_pg_upcall_requests_total{route,code}` |
| PgDog | OpenMetrics port | clients waiting and active, server connections, pool saturation, errors, query and transaction time, bans |
| `compute_ctl`, Postgres | `compute_ctl`'s metrics, and `pg_stat_statements` (on by default) | start phases, LFC hit ratio, replication lag, connections |
| Pageserver, storage controller, broker | their `/metrics` | Neon's own series |
| `loams-wal` | **new** `/metrics` and `/healthz` | append latency, durable lag, journal segments, offload lag |

- **Traces.** OTel spans across a wake: PgDog wait → waker detection → compute bind or start → `compute_ctl` phases → first query. `traceparent` goes into the compute spec, so `compute_ctl` spans join the trace (verify the fork's OTel support).
- **Logs.** Structured JSON to stdout, collected by the cluster's pipeline. Postgres logs carry the endpoint and compute ids.
- **Dashboards and alerts** are in `deploy/observability/loams-postgres/`: Grafana JSON and Prometheus rules. Alerts fire on `cl_waiting > 0` for more than 10 s on a running endpoint, cold-start p99 above target for 15 minutes, `loams-wal` durable and offload lag, TiKV leader elections on `loams_pgwal`, pageserver `remote_consistent_lsn` lag, reconcile errors, certificate expiry within 14 days, and a failed restore drill.
- **Not billing-grade.** None of these is a usage record (D548). Billing goes through `ComputeLifecycleObserver` to `loams-platform`.

## 14. Quotas and limits (D717)

The limits record of §41 §9 / MT4 Ruling 9 gains: `pg_projects`, `pg_branches_per_project`, `pg_endpoints_per_project`, `pg_max_cu_per_endpoint`, `pg_total_cu`, `pg_storage_bytes`, `pg_history_retention_s`, `pg_connections_per_endpoint`, `pg_new_connections_per_s`.

| Limit | Enforced at | When it is exceeded |
|---|---|---|
| Projects, branches, endpoints | `pg-control` admission | `RESOURCE_EXHAUSTED`, reason `quota_exceeded` |
| CU per endpoint, total running CU | `pg-control` (start, resize) | The start or resize is refused; the autoscaler caps at the limit |
| Storage bytes (the logical size from the pageserver) | `neon.max_cluster_size` in the compute spec | Writes fail with Postgres' own error once the size is reached; reads continue |
| History retention | the tenant's `pitr_interval` | Clamped |
| Connections, new connections per second | PgDog pool sizes and client limits | PgDog refuses the connection |
| Kubernetes resources | `ResourceQuota` and `LimitRange` on `loams-pg-<ns>` | The scheduler refuses the Pod; the endpoint goes to `failed` with the reason |

Enforcement state is approximate and labelled "not billing-grade" (D547).

## 15. Kubernetes deployment and security (D718)

### 15.1 What runs where

| Namespace | Workloads | Installed by |
|---|---|---|
| `loams-postgres` | `pg-control` (Deployment ×2), storage controller (Deployment ×2) and its CNPG Cluster, storage broker (×2), pageservers (StatefulSet, NVMe local PVs), the `loams-wal` pool (Arm A: StatefulSet ×3, one per AZ, on PLP NVMe; TiKV store: Deployment ×2 per AZ, plus the `loams_pgwal` keyspace on the TiKV cluster), the L4 SNI listener | Helm chart `deploy/helm/loams-postgres`, applied as Argo CD Applications in MT3's waves (CNPG and cert-manager first, then storage, then `pg-control`) |
| `loams-pg-<ns>` (one per Loams namespace with Postgres) | PgDog (Deployment ×2), compute Pods, endpoint Services, warm-pool Pods | The namespace, NetworkPolicy, quota and PgDog come through the tenants repository (MT4). Pods and Services come from `pg-control` (§7.2) |

- **NetworkPolicy (default deny).** Computes may reach the pageservers, `loams-wal`, `pg-control`'s upcall listener and DNS, and nothing else unless the project enables outbound access (for `postgres_fdw` and `dblink`). PgDog may reach its namespace's computes. Only the L4 listener may reach PgDog.
- **Isolation.** Computes run untrusted SQL. Users get `neon_superuser`, never `superuser`, so `COPY … PROGRAM`, untrusted languages and `pg_read_server_files` are not available. The extension allow-list is in the compute image (Q651). A sandboxed RuntimeClass (gVisor or Kata) is optional per node pool, and its cost is measured in PG2e (Q645). Pods run non-root, with a read-only root filesystem except the data directory, `seccompProfile: RuntimeDefault` and no host paths.
- **Operator.** `loams-operator` (D185) is not in the repository yet. Its `LoamsPostgres` CRD (object store, AZs, versions, sizes) comes after GA and renders the same objects as the chart. GA does not depend on it.

### 15.2 Security review (a GA gate)

1. A threat model in `docs/security/loams-postgres-threat-model.md`. It covers tenant escape from a compute, a cross-tenant route through PgDog, credential theft (`users.toml`, the credential store, compute JWTs), a forged upcall to `pg-control`, a malicious extension, a branch read across tenants through a forged tenant id, quota bypass by agents, and supply chain (fork images, PgDog digest).
2. Tests for each item, in the plan's tasks.
3. An **external review** of the chart, the NetworkPolicies, `pg-control`'s upcall and auth paths, and PgDog's rendered config, with every high and critical finding fixed.
4. Images signed (cosign), SBOMs published, `cargo deny` on this repository and the fork.

## 16. Conformance and performance gates (D700)

### 16.1 Conformance

| Suite | What passes |
|---|---|
| Postgres `pg_regress` (the parallel schedule of the compute's major version, run through PgDog in session mode) | Everything except a checked-in allow-list with a reason per test (tablespaces, and tests that need superuser or file access) |
| Neon's `test_runner` subset for compute and branching in the fork's CI, plus Neon's safekeeper protocol tests run against `loams-wal` | As upstream |
| `conformance/router` PgDog inventories (`pgdog-loampg-*.tsv`) | Replayed against a Loams Postgres compute through PgDog. The 548 statement rows move from `pending-target` to classified, with no `error` or `differs` in the components Loams uses (the RT3-style gate, D302). The suites (pgbench and the others) are `pass` |
| Drivers | psql/libpq 17, psycopg 3, asyncpg, node-postgres, postgres.js, pgjdbc, Npgsql, pgx, tokio-postgres, sqlx: connect with `verify-full`, transactions, prepared statements in transaction pooling, `COPY`, cancel, `LISTEN`/`NOTIFY` in session mode |
| ORMs and migration tools | Prisma (migrate and client), Drizzle, SQLAlchemy with Alembic, Django, Rails ActiveRecord, Hibernate, TypeORM, Flyway: their own migration and CRUD smoke suites |
| Extensions | `pgvector`, `postgis`, `pg_trgm`, `pg_stat_statements`, `pgcrypto`, `uuid-ossp`, `hstore`, `citext`, `btree_gin`, `btree_gist`: create, use and survive a branch and a restore |

### 16.2 Performance gates (targets)

Measured on the reference topology (§16.3) with 3 runs, as §28 §7 measures.

| Gate | Target |
|---|---|
| Cold start, suspended → first result through PgDog, warm pool | p50 ≤ 2 s, p99 ≤ 5 s |
| Cold start without the pool | p99 ≤ 15 s |
| 100 concurrent wakes of distinct endpoints | p99 ≤ 8 s, no failed connection |
| Connection setup through PgDog (TLS + SCRAM, warm compute) | p50 ≤ 15 ms, p99 ≤ 50 ms, in-region |
| PgDog overhead on `SELECT 1` (transaction mode, versus direct) | ≤ 0.3 ms at p50 |
| `loams-wal` launch targets (`commit-1`, `commit-16`, `tpcb-64`, `bulk`, aggregate, CPU, failover) | §9.3 |
| `tpcb-64` (`-s 100`), 4 CU compute through PgDog | ≥ 3,000 TPS, p99 ≤ 60 ms (estimate from §28 §6.1's figures; PG2 Task 54 calibrates and the owner confirms, Q653) |
| Branch create to `ready` (no compute), any size up to 1 TiB | p99 ≤ 2 s |
| PITR restore to `ready` | p99 ≤ 30 s |
| Control-plane API | reads p99 ≤ 200 ms; synchronous mutations p99 ≤ 500 ms |
| Scale | 1,000 projects, 10,000 suspended endpoints and 200 running computes on the test cluster with every gate above still holding |
| Chaos soak, 72 h | Kills of computes, PgDog, `pg-control`, pageservers, `loams-wal` acceptors and TiKV leaders, and the storage controller, plus §9.7's catalog; a bank checker (and `loams-nemesis` from RT5 when it exists) finds no lost acknowledged commit; availability of read-write endpoints ≥ 99.95 % outside cold starts |

### 16.3 Reference topology

Three nodes in three AZs (or one host with `tc netem` at 0.5 ms one way, as §28 §7), NVMe with power-loss protection, Kubernetes 1.35, the `loam-bench` self-hosted runner of `loams-pg-bench.yml`. Shared CI runners do not produce gate numbers.

## 17. Exit checklist for production (D700)

Loams Postgres is GA when each item is checked off with a link to its evidence (a CI run, a results file or a report):

- [ ] **API:** every `loams.postgres.v1` RPC is served, passes `buf lint` and `buf breaking`, has idempotency replay tests, and is in `docs/api/route-map.md`. SDK fixtures exist (SDK1).
- [ ] **Control plane:** `pg-control` runs ×2 on TiKV; killing either instance during 100 concurrent creates leaves no duplicate or orphaned tenant, timeline, compute or Service.
- [ ] **Lifecycle:** start, suspend, wake on connect, restart and delete work on both runtimes; idle computes suspend; the warm pool works; autoscaling stays inside `[min_cu, max_cu]`.
- [ ] **PgDog:** per-namespace deployments rendered and reloaded from records; TLS on both hops, with plaintext refused; SCRAM; credential exchange for OIDC and agent tokens; an unauthorized database refused before any backend connection; read-only routing; health and failover; licence rules in CI.
- [ ] **Branching and PITR:** branch at LSN and at timestamp; restore with a backup branch; TTL and protection; retention enforced.
- [ ] **HA and DR:** the §11 failure table exercised in CI on kind (compute, PgDog, `pg-control`, pageserver, `loams-wal`); the restore drill green twice in a row.
- [ ] **Upgrades:** a compute image roll, a pageserver and `loams-wal` roll, a PgDog roll and a `pg-control` roll under pgbench load, with no failed transaction beyond reconnects and no lost commit.
- [ ] **Observability:** every §13 endpoint scraped, the dashboards load, every alert has a firing test.
- [ ] **Quotas:** every §14 limit has a test that sees the refusal.
- [ ] **Security:** the threat model, its tests, and the external review with high and critical findings closed; images signed; NetworkPolicy tests.
- [ ] **Conformance:** §16.1 green, and `conformance/router`'s Loams Postgres rows classified.
- [ ] **Performance:** §16.2 green on §16.3's topology, with the results in `bench/results/`.
- [ ] **Deployment:** the Helm chart installs from an empty kind cluster through Argo CD and through Flux; single-node mode starts with one command; the desktop switches to the API (§18).
- [ ] **Docs:** operator runbooks (failover, restore, upgrade, WAL incidents), user docs (connect, branch, restore, limits), and `deploy/pgdog/README.md`.
- [ ] **The WAL:** `loams-wal` only. The §9.3 targets are green on the chosen store, the owner has ruled on the store (Q652), the interpreted sender is in process, broker publication, TLS, metrics and membership changes are done, the recover-from-bucket and archive-restore drills pass, the §9.7 nemesis run is clean, and `scripts/ci/no-safekeeper.sh` passes, with dev data migrated (§9.8).

## 18. Single-node mode and the desktop contract (D719)

### 18.1 Single-node mode

`loams dev --postgres` (or `loams pg up` from the CLI) starts, in one engine process:

- `pg-control` with the local store (redb) and the `compose` runtime;
- a compose project `loams-pg` from `deploy/loams-postgres-single/` with **pinned digests**: the storage broker, one pageserver with `remote_storage = local_fs` under the engine's data directory (or RustFS when the user configures one), one `loams-wal --store nvme` acceptor with D268's local metadata backend and the in-process interpreted sender, and one PgDog;
- computes as compose services, with a local CA for TLS (the certificate is shown in `GetConnectionInfo`).

The engine advertises `loams.postgres.v1` in `InstanceService/GetInstance` (`api_versions`) and the feature `postgres.control`. Small self-hosts use the same mode on a single VM. It runs with the same guarantees as one AZ: no HA, and durability as good as the one disk and the local backups.

### 18.2 The desktop contract

AP1e's Postgres page (D668) calls the pageserver directly because no control plane exists. With this API the page works the same way against a local engine, a self-hosted cluster or Loams Cloud:

- **Selection.** If `flags.has('loams.postgres.v1')` on the active server, the desktop uses the API. Otherwise it uses D668's local path against `deploy/neon`. The page's header shows which: "Control plane" or "Local stack".
- **One interface, two implementations** in the desktop's main process (`apps/desktop-electron/src/main/sql/`), so the renderer and the `pg` IPC surface do not change:

```ts
export interface PostgresBackend {
  kind: 'control-plane' | 'local-stack';
  listProjects(): Promise<PgProject[]>;                  // local: GET /v1/tenant (one project per tenant)
  listBranches(project: string): Promise<PgBranch[]>;    // local: GET /v1/tenant/{t}/timeline
  createBranch(project: string, b: { name: string; parentBranchId: string; lsn?: string; timestamp?: string }): Promise<PgOperation>;
  branchWal(project: string, branch: string): Promise<{ commitLsn: string; flushLsn: string; remoteConsistentLsn?: string; backupLsn?: string }>;
  listEndpoints(project: string): Promise<PgEndpoint[]>; // local: one synthetic endpoint per compose compute
  startEndpoint(id: string): Promise<PgOperation>;       // local: unsupported -> { ok: false, code: 'unsupported_local' }
  suspendEndpoint(id: string): Promise<PgOperation>;     // local: unsupported
  connectionInfo(endpoint: string, database?: string): Promise<{ host: string; port: number; database: string; user: string; sslmode: 'verify-full' | 'disable'; caPem?: string; passwordRef: string }>;
  watch(project: string, cb: (e: PgWatchEvent) => void): () => void; // local: 5 s polling
}
```

| D668 operation | `control-plane` | `local-stack` (unchanged) |
|---|---|---|
| `tenants()` | `ListProjects` | `GET :9898/v1/tenant` |
| `timelines(t)` | `ListBranches` | `GET :9898/v1/tenant/{t}/timeline` |
| `createBranch(t, …)` | `CreateBranch`, then `WatchOperations` | `POST :9898/v1/tenant/{t}/timeline/` (trailing slash, as `deploy/neon/README.md`) |
| `walStatus(t, tl)` | `GetBranch.wal` | `GET :7676/v1/tenant/{t}/timeline/{tl}` |
| `connection()` | `GetConnectionInfo` (through PgDog) | compose constants (compute port 55433) |
| `revealPassword()` | The password from `CreateRole` or `ResetRolePassword`, kept in the desktop's encrypted store (`safeStorage`); never fetched from the server | compose default |
| `query(sql)` | `pg` in main against `GetConnectionInfo`, with `IssueConnectCredential` when the user signed in with OIDC | as D668 |
| start, stop compute | `StartEndpoint`, `SuspendEndpoint` | not offered (D668) |

Nothing else in the desktop changes. The work is PG2 Task 58, scheduled after AP1e's Tasks 21–23 merge, so it does not collide with AP1e.

## 19. As built on 2026-10-08 (reconciled with the code)

| Item | State |
|---|---|
| `crates/loams-safekeeper` | The binary `loams-wal` (feature `server`): `--listen-pg 127.0.0.1:5454`, `--listen-http 127.0.0.1:7676` with `GET /v1/status`, `POST /v1/tenant/timeline` and `GET /v1/tenant/{t}/timeline/{tl}`; `--store mem\|tikv\|tikv-raw\|nvme`; `--io auto\|uring\|pwritev2\|buffered`; `--runtime tokio\|compio`; the feeder; a bearer token and `--trusted-network`. **No `/metrics`, no health route, no TLS, no broker publishing, and no interpreted protocol** (`service.rs` refuses it, Q112) |
| Bench gates | Every gate file in `bench/results/` is FAIL. Arm A passes `commit-16` and `tpcb-16` for every tier, and `commit-1` for SQPOLL. It fails `bulk` for every tier. §28 §7.3, which §7.2 refers to, was missing; this addendum adds it |
| `deploy/neon` | Stock Neon, driven by `curl` from its README: pageserver `127.0.0.1:9898`, safekeeper `7676`, compute1 `55433`, compute2 `55434` (profile `branch`), RustFS `9000`. The images are `latest` and **not pinned**. There is no storage controller, no proxy and no TLS |
| Control plane | None: no `loams-neon`, no `loams-pg-control`, no `x/`/`X/`/`C/` keys in `keys.rs`, no `loams-control` (MT4 is planned), no `loams-operator` |
| PgDog | `conformance/router` holds the PgDog → Postgres 17.11 inventory (548 statement rows, all `pending-target`). RT1 (in progress) plans the renderer, the admin adapter and `ConfigPush` in `loams-sqlrouter-io`, which does not exist yet |
| Desktop | AP1e Tasks 21–23 (stacks, `neon.ts`, the Postgres page) are not built. Task 22's `neon.ts` omits the tenant `location_config` step and the trailing slash that `deploy/neon/README.md` uses, and Task 21 does not pass `TENANT_ID` and `TIMELINE_ID`; PG2 Task 58 fixes them |

## 20. Risks

| # | Risk | Mitigation |
|---|---|---|
| 1 | PgDog fails a client at once on a refused backend, so wake-on-connect does not work | PG2 Task 18 measures it first; §8.4's fallbacks in order; Neon's proxy (in the fork) as the last one |
| 2 | Per-namespace PgDog costs too much at 10,000 namespaces | The Pods are small; a shared regional PgDog (Q641) is the escape, with globally unique routed names |
| 3 | PgDog needs plaintext role passwords in `users.toml` | Q643; the Secret's access is narrowed to PgDog's ServiceAccount and encrypted at rest; credential exchange keeps most logins short-lived |
| 4 | In-place Pod resize is not available, or not stable, on a customer's cluster | Autoscaling degrades to restart-to-resize at the next suspend; `min_cu = max_cu` is always valid |
| 5 | Cold start misses the target on BYOC clusters without image caches | The warm pool; pre-pulled images through a DaemonSet; the gate is measured on the reference topology and reported on others |
| 6 | Fork upkeep (§28 §10) slows security releases | D241's cadence; minor-only upgrades flow through image rolls (§12) |
| 7 | Running `pg-control` Pods outside Git surprises GitOps operators | Only in `loams-pg-*` namespaces, with labels, and Argo CD told to ignore them (`argocd.argoproj.io/compare-options: IgnoreExtraneous`) |
| 8 | AGPL exposure through a well-meant PgDog patch | §8.3; CI guards; reviewers reject any PgDog fork reference |
| 9 | A cross-tenant read through a forged tenant id at the pageserver | Tenant-scoped JWTs (`NeonJWT`); computes cannot reach another tenant's storage token; a negative test in PG2e |
| 10 | **`loams-wal` misses its launch targets** (every gate file in `bench/results/` is FAIL today; `bulk` is 3–10× short), and there is no safekeeper fallback | PG2d is launch-critical and starts at once; bottlenecks are diagnosed before they are fixed (§9.4); two stores are pursued; the owner rules on the store and on any target revision (Q652, Q653). Launch slips rather than shipping safekeepers |
| 11 | **The in-process interpreted sender** pulls Neon's `wal_decoder`, `postgres_ffi` (bindgen against Postgres headers) and workspace pins into Loams (D271's reason for deferring it) | A separate feature and crate boundary; headers extracted from the pinned compute image and cached in CI (Q112); the dependency set reviewed with `cargo deny` before Task 31 merges |
| 12 | **Protocol and durability bugs** in a young WAL service now carry every production commit | Neon's safekeeper tests and TLA+ traces against `loams-wal`; `loams-detsim`; the §9.7 nemesis catalog; the archive-restore path as an independent second line (§9.6) |

## 21. Open questions (Q640–Q654)

| # | Question | Needed by |
|---|---|---|
| Q640 | Does PgDog (the pinned version) hold a client in its wait queue while a backend refuses connections, and for how long? If not, which §8.4 fallback: retry configuration, a Loams wake shim, or Neon's proxy in front? | PG2 Task 18 |
| Q641 | One PgDog per Loams namespace (D708), or a shared regional PgDog with globally unique routed names for small tenants on Loams Cloud? | PG2 Task 20 |
| Q642 | Does upstream PgDog support JWT/OIDC client auth or SNI-based routing? If it does, adopt it by configuration in place of the credential exchange or alongside it | PG2 Task 18 |
| Q643 | Does PgDog accept SCRAM verifiers in `users.toml`, or does it need the plaintext password for its server-side login? | PG2 Task 18 |
| Q644 | Autoscaling: Kubernetes in-place Pod resize (D712), or NeonVM on KVM node pools where they exist? Is 0.25 CU the right minimum? | PG2 Task 15 |
| Q645 | Compute isolation: runc with the §15.1 hardening, or gVisor or Kata by default? What does gVisor cost Postgres on pgbench? | PG2 Task 53 |
| Q646 | The storage controller's HA: does the fork support two replicas with leadership through its database, and is a two-instance CNPG Cluster enough? | PG2 Task 49 |
| Q647 | Region-loss RPO now depends on `loams-wal`: is "250 ms group-commit interval plus bucket replication lag" acceptable at GA, and must the archive (`.lwal`) bucket replicate synchronously or asynchronously? | Founder, before GA |
| Q648 | Major-version upgrades: dump and restore at GA (D716), or in-place `pg_upgrade` support in the fork first? | PG2 Task 51 |
| Q649 | ~~Postgres 18 at GA (D241 says 17 and 18; §28 §10 estimates 3–6 engineer-weeks of fork work), or 17 only at GA?~~ Answered by §51 D813: 17 at GA; 18 when NF1e's gate passes; 19 beta | Answered (proposed) |
| Q650 | The fields `loams-platform` needs from `ComputeLifecycleObserver` to bill compute and storage; confirm that storage-byte-hours are read from the pageserver's logical size by the platform | `loams-platform`, PG2 Task 12 |
| Q651 | The extension allow-list at GA, and which extensions are excluded for licence reasons (for example TimescaleDB's TSL parts) | PG2 Task 55 |
| Q652 | The launch `loams-wal` store: the TiKV hot tier (the owner's directive; D234) or Arm A (local NVMe acceptors; D264), on Task 42's bake-off results against §9.3; and whether the other store ships behind a setting | Founder, PG2 Task 42 |
| Q653 | The reference hardware for §16.3 and who provides it; the owner confirms the §9.3 and §16.2 targets after calibration (PG2 Task 30 for the WAL, Task 54 for the rest); and whether optional logical exports (§11) are offered at GA | Founder, PG2 Tasks 30 and 54 |
| Q654 | Keep stock safekeepers as a never-deployed reference profile in `deploy/loams-pg-bench` (§9.1), or drop them from the benchmark too and compare only with §9.3's absolute targets; and how long `deploy/neon` may keep them before Task 43 | Founder, PG2 Task 30 |

## 22. Contradictions with earlier decisions, and how they are resolved

| Earlier | Conflict | Resolution |
|---|---|---|
| D236: PgDog routes, as a single Deployment in the Loams Postgres namespace | D708: one PgDog per Loams namespace | **Amended layout**, same rule: unmodified, separate, config only |
| §28 §8: "PgDog cannot wake a stopped compute, so there is no scale-to-zero"; Q113 | D709: wake from outside PgDog | **Q113 answered.** Neon's proxy becomes the fallback, not the plan |
| D220: the control plane is private | D703 | Already amended by D540 and D557. Only the commercial layer and billing are private |
| D668: the desktop is the local control plane | D719 | **Amended:** the fallback when no server offers `loams.postgres.v1` |
| §41 §2.1: Git is the only write path to a cluster | D707: `pg-control` creates compute Pods | **A scoped exception:** runtime Pods and Services in `loams-pg-*` only; everything declarative still comes through Git |
| §28 §2.2 non-goals: "autoscaling VMs are out of scope" | D712 | Consistent: autoscaling without VMs |
| §28 §11's phases P2b and P3 | PG2a–PG2c | PG2a–PG2c **implement** P2b and P3; PG2d replaces P4b–P4c as a launch milestone; P5 is unchanged |
| D233 (§28): `loams-wal` becomes the default only if the pgbench gate passes; safekeepers stay the default until then | Owner directive 2026-10-08, "remove safekeepers"; D714 | **Reversed:** `loams-wal` is the only WAL; the gate becomes absolute launch targets (§9.3); safekeepers are never a fallback |
| D271 (§28 §7.2): the feeder stays; in-process `wal_decoder` deferred | The feeder needs a stock safekeeper | **Superseded at launch:** the in-process interpreted sender is required (Task 31) |
| §28 §6.10: the per-database switch from safekeepers, with rollback to safekeepers | No safekeepers in production | Kept only as the quiesced move between `loams-wal` pools and for dev migration (§9.5, §9.8) |

## 23. Sources

- Loams: §19, §23, §28 (all), §31 §6–§8, §37 §19.10, §38 §4, §41, §44; `crates/loams-safekeeper` (`src/bin/loams-wal.rs`, `src/http.rs`, `src/service.rs`); `crates/loams-meta-tikv/src/keys.rs`; `deploy/neon/{compose.yaml,README.md}`; `deploy/loams-pg-bench/`; `scripts/loams-pg-bench/`; `bench/results/gate-*.md`; `conformance/router/README.md`; `proto/loams/operations/v1/operations.proto`; `proto/loams/devices/v1/devices.proto`; `.github/workflows/loams-pg-bench.yml`; `docs/plans/2026-10-01-rt1-postgres-slice-and-sim.md`; `docs/plans/2026-10-02-mt4-byoc-control-plane.md`; `docs/plans/2026-10-08-ap1e-electron-desktop.md`. Read on 2026-10-08.
- Neon (fork `ostrium-labs/neon` at `fa504217c`, as §28 §15): `libs/compute_api`, `compute_tools`, `storage_controller`, `proxy/src/control_plane`. The items marked (verify) were not re-read for this addendum.
- PgDog: §28 §8's verification of 2026-09-29 (licence, features, configuration). Queueing, ban, admin and auth behaviour are (verify) and are checked in PG2 Task 18.
