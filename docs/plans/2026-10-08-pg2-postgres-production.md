# PG2 — Loams Postgres in Production Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Each task lists the interfaces it must produce and the tests that must exist and pass before it is done. Where this plan gives exact names, paths, ports or defaults, use them verbatim. Where it gives a contract and named tests, write the code to that contract, and record any deviation in "Rulings made during execution" at the end of this file.
>
> **Status: Planned** (2026-10-08). Track PG2, design [§46](../design/46-loams-postgres-production.md) (D700–D719, Q640–Q654), building on [§28](../design/28-loams-postgres.md) (D230–D241, D263–D272). PG2a–PG2c implement §28's P2b and P3. **Owner directive, 2026-10-08: "remove safekeepers".** `loams-wal` is the only WAL, and PG2d (making it meet its launch targets) is launch-critical. PG2 is unrelated to PG1: the engine's read-only `--pg-listen` is not Loams Postgres and is out of scope.
>
> **Amended by [NF1](2026-10-09-nf1-neon-fork.md) (2026-10-09, design [§51](../design/51-loams-postgres-fork.md), owner decision "Mirror and fully fork neon postgres all repos we will maintain").** Ostrium Labs builds and maintains the fork's images. NF1 supersedes D241's cadence (D810–D813) and changes four things here:
> - **Task 0 ruling 7 (Q649):** D813 answers it. Postgres 17 at GA; 18 when NF1e's gate passes, without delaying PG2; 19 beta.
> - **Task 2 ruling R2.1:** the image pins move to `ghcr.io/ostrium-labs`, first as a digest-preserving mirror, then as our own builds, through one pin file, `deploy/pins/neon-images.toml` (NF1 Tasks 1, 2, 14).
> - **Task 51:** builds `UpgradeProject` on NF1 Task 37 (`fast_import` and `ImportPgdata`), with dump and restore as the fallback (D815).
> - **Task 55:** takes its extension allow-list from NF1's catalogue (D812, NF1 Task 21).
>
> **Amended again by NF1 (2026-10-09, hard fork, D820–D824).** The fork is a hard fork named Loams Postgres: `ostrium-labs/neon` is now `ostrium-labs/loams-postgres` and nothing is merged from `neondatabase/*`. NF1 Task 1b renames this plan's Loams-side names before Task 2's pin file. **Names after the hard fork (D822, D823; NF1 Task 1b).** This document predates the rename and keeps the old names in its text. Read them as: `ostrium-labs/neon` → `ostrium-labs/loams-postgres`; `crates/loams-neon` (package `loams-neon`) → `crates/loams-postgres` (package `loams-postgres`); `deploy/neon` → `deploy/loams-postgres-dev` (the compose project `loams-neon` and the desktop's copied `stacks/neon` keep their names); our images `neon` → `loams-postgres-storage` and `compute-node-v<N>` → `loams-postgres-compute-v<N>` (Neon's own `77e22e4b` images stay `ghcr.io/neondatabase/...` until NF1 Task 14, with private copies under `ghcr.io/ostrium-labs/neon-archive/`); `deploy/pins/neon-images.toml` → `deploy/pins/loams-postgres-images.toml`. Compatibility names (the `neon` extension, `neon.*` GUCs, `neon_superuser`, `NEON_*`, wire and on-disk formats) do not change. Tasks of this plan that run after NF1 Task 1b use the new names; its rulings keep the names they used.

**Goal:** Loams Postgres GA. Serverless Postgres on the bucket (the `ostrium-labs/neon` fork), with:
- Loams' control plane: `loams.postgres.v1`, `pg-control`, compute lifecycle and scale-to-zero;
- PgDog as the front door: routing, pooling, TLS, SCRAM, credential exchange for OIDC and agent tokens, wake-on-connect, read routing, health;
- `loams-wal` as the only WAL service (TiKV hot tier or Arm A, chosen by bake-off), meeting absolute launch targets, with HA, fencing, offload, an archive restore path and nemesis-tested durability;
- PITR, HA, upgrades, observability, quotas and a Helm chart;
- a single-node mode the desktop and small self-hosts use.

The exit is §46 §17's checklist, repeated at the end of this plan with the owning tasks.

**Architecture** (§46 §6, §8):
- **`pg-control` is a role of the `loams` binary** (feature `postgres`, off by default). It serves `loams.postgres.v1` and the upcalls Neon's components call. It runs level-triggered reconcilers under a fenced lease per project, and drives `ComputeRuntime` (compose or Kubernetes), PgDog (render, push, `RELOAD`) and the waker.
- **`loams-neon`** is a plain HTTP client of the fork's storage controller, pageservers, `loams-wal` and `compute_ctl`, with no Neon code dependency.
- **`loams-wal` is the WAL** (§46 §9). The compute's walproposer speaks the safekeeper protocol to it, and the pageserver reads from it through the in-process interpreted sender. No stock safekeeper runs anywhere except the never-deployed bench reference profile.
- **PgDog is unmodified** and runs one Deployment per Loams namespace. Loams only renders its config, pushes it through a Loams sidecar, sends `RELOAD` and reads its admin database.
- **State** sits behind `PgControlStore`: TiKV in production, redb in single-node mode. The same conformance suite covers both.

**Tech Stack:**
- Rust 1.97 (workspace `rust-version`), edition 2024, workspace lints.
- `connectrpc` 0.9 and `buffa` through `loams-proto` (D362, §44).
- `tokio`, `reqwest` 0.13 (rustls), `serde`, `redb` 4 (workspace), `loams-tikv` and `loams-meta-tikv`.
- `tokio-postgres` (PgDog's admin database and the tests), `rcgen` (the compute CA, as MT4), and `kube` at the version `loams-operator` or MT4 uses (Task 0 records it).
- `buf` for protos.
- Test tools: compose (docker or podman) for `deploy/neon` and single-node; kind for Kubernetes e2e (CI only); pgbench; and the drivers and ORMs of Task 29.
- External, pinned by digest: the fork's `neon` and `compute-node-v17` images, `ghcr.io/pgdogdev/pgdog` (version from Task 18), CloudNativePG 1.30.x, cert-manager.

**Spec:**
- [§46](../design/46-loams-postgres-production.md) (all), and D700–D719, Q640–Q654 in the [decision log](../design/13-decision-log.md).
- [§28](../design/28-loams-postgres.md) §5 (control-plane endpoints), §6 (the WAL service), §6.10 (the quiesced move), §7 (the bench protocol), §8 (PgDog rules), §10 (fork upkeep).
- [§23](../design/23-neon-and-wesql.md) §3.2, §6.1–§6.5.
- [§31](../design/31-loams-router-and-verification.md) §6–§8 and [RT1](2026-10-01-rt1-postgres-slice-and-sim.md) (PgDog renderer, admin adapter, `ConfigPush`).
- [§41](../design/41-multitenant-byoc-control-plane.md) §4–§9 and [MT4](2026-10-02-mt4-byoc-control-plane.md) (limits record, tenants repository, no-metering guard).
- [§44](../design/44-unified-api-and-sdks.md) §4–§7 (API rules).
- [§37](../design/37-desktop-and-mobile-apps.md) §19.10 (D667, D668) and [AP1e](2026-10-08-ap1e-electron-desktop.md) Tasks 21–23.

## Global Constraints

- **Worktree and branches.** Work in `~/Documents/Ostriumlabs/loams-wt/pg2-postgres-production`. Use one branch per milestone, `feat/pg2a-control-plane`, `feat/pg2b-compute`, `feat/pg2c-pgdog`, `feat/pg2d-wal-gate`, `feat/pg2e-ops` and `feat/pg2f-single-node`, each based on `dev`, with stacked PRs targeting `dev`. Use `git commit -s` (DCO). Commit areas: `pg`, `neon`, `pgdog`, `wal`, `deploy`, `ci`, `desktop`, `docs`.
- **Rust builds** use the shared target directory (`~/Documents/.cargo/config.toml`). Never set `CARGO_TARGET_DIR` and never build in `/tmp`. Run one cargo build at a time on the build machine (jobs and linker from `~/.cargo/config.toml`). Build the touched crates (`cargo test -p loams-pg-control`), not the workspace. kind, k3d and compose stacks run in CI jobs, or locally only when no cargo build is running.
- **The default build does not change.** `loams-neon` and `loams-pg-control` are only reachable through the `loams` feature `postgres`. `cargo tree -p loams -e normal` on default features must not list them (Task 9 test).
- **AP0 API rules** (§44): every mutation has `idempotency_key`; reads are `NO_SIDE_EFFECTS`; errors are `loams.errors.v1` with a reason registered in `docs/api/reasons.md`; pagination is `page_size`/`page_token`; watch streams send a snapshot, then changes, then a heartbeat every 15 s.
- **Deterministic rendering.** PgDog config, compute specs and Kubernetes objects are byte-identical for identical records (golden tests).
- **Secrets.** Role passwords, compute JWTs, storage tokens and TLS keys are typed `Secret<T>`, whose `Debug` and `Display` print `[redacted]`. No secret appears in a log line, an error message, an `Operation.result` or a metric label. `GetConnectionInfo` has no password field.
- **The PgDog licence boundary** (§46 §8.3): no PgDog crate, source, patch, plugin or text in this repository. Images are pinned by digest. Task 19's CI check enforces it.
- **No billing** (D548, D550): no field, metric, table or endpoint named `plan`, `price`, `invoice`, `credit`, `meter`, `billable` or `usage` in any PG2 code. Lifecycle events go out only through `ComputeLifecycleObserver`.
- **Pins.** Exact versions for every new crate and image. Images are pinned by digest. A new dependency must be at least 14 days old (`cargo info`, the registry date). Record each pin in the task's commit message.
- **Neon's API is the fork's code.** Every Neon request or response shape Loams copies is checked against `ostrium-labs/neon` at the pinned commit and recorded as a fixture under `crates/loams-neon/tests/fixtures/`, with the source path in a comment.

## Review Focus

1. **A retried or concurrent mutation creates a second tenant, timeline, compute, Service or role.** Expected: never; the deterministic ids and the fenced lease make every step idempotent. Tests: Task 5 `create_project_replay_returns_same_operation`, `create_branch_same_name_other_key_is_already_exists`; Task 7 `two_instances_one_actor`, `stale_lease_write_is_fenced`; Task 52 `kill_pg_control_during_100_creates_leaves_no_orphans`.
2. **A role reaches a database it is not granted, through PgDog.** Expected: PgDog refuses it before any backend connection. Tests: Task 24 `unauthorized_database_refused_before_backend`, `revoked_grant_removed_after_push`.
3. **A secret leaks** into a log, an error, an operation result, a metric or `GetConnectionInfo`. Expected: never. Tests: Task 6 `role_secret_never_logged`, `create_role_returns_password_once`; Task 1 `connection_info_has_no_password_field`; Task 25 `issued_credential_not_in_operation_result`.
4. **A connection carries plaintext**, client to PgDog or PgDog to compute. Expected: refused. Test: Task 23 `plaintext_refused_both_hops`.
5. **Wake races.** Two PgDog replicas see the same waiter; a suspend starts while a client waits; a wake starts during a suspend. Expected: one compute start, no client error, no suspend under a waiting client. Tests: Task 26 `concurrent_waiters_one_start`, `suspend_aborted_when_client_waiting`, `wake_during_suspending_restarts_cleanly`.
6. **A compute reads another tenant's pages.** Expected: the pageserver refuses (tenant-scoped JWT). Test: Task 53 `compute_token_cannot_read_other_tenant`.
7. **PgDog code or a patch enters the repository.** Expected: CI fails. Tests: Task 19 `no_pgdog_in_lockfiles`, `pgdog_image_pinned_by_digest`, `licence_bundle_present`.
8. **The default `loams` build gains Postgres code.** Expected: no. Test: Task 9 `default_features_exclude_postgres`.
9. **`loams-wal` acknowledges a commit that is later lost**, through a deposed proposer, a pipelined append that crosses a term bump, a vote not durable before the term rises, or a torn journal unit. Expected: never. Tests: Task 34 `deposed_writer_fenced_with_pipelining`; Task 39 `deposed_compute_gets_no_ack`, `vote_durable_before_term_raise`; Task 41 `nemesis_catalog_no_lost_commit`, `detsim_10k_seeds_safe`.
10. **A stock safekeeper survives in a shipped artifact, or the dev migration loses data.** Expected: no. Tests: Task 43 `no_safekeeper_guard_rejects_fixture`, `migrate_wal_preserves_checksums`; Task 40 `archive_restore_matches_checksums`.

---

## File structure

```
proto/loams/postgres/v1/postgres.proto                     Task 1
crates/loams-proto/                                         (generated; build.rs gains the package)
crates/loams-neon/                                          Task 2
  src/{lib.rs,error.rs,ids.rs,pageserver.rs,storcon.rs,wal.rs,compute_ctl.rs,spec.rs}
  tests/{fixtures/,client.rs,it_deploy_neon.rs}
crates/loams-pg-control/                                    Tasks 3–17, 20–27, 36, 44, 46–47, 50–51, 56
  src/lib.rs  model.rs  ids.rs  names.rs  secrets.rs  authz.rs  audit.rs  observer.rs  metrics.rs
  src/store/{mod.rs,local.rs,tikv.rs,conformance.rs}
  src/service/{mod.rs,projects.rs,branches.rs,endpoints.rs,roles.rs,databases.rs,connect.rs,watch.rs,operations.rs}
  src/reconcile/{mod.rs,lease.rs,project.rs,branch.rs,endpoint.rs,resync.rs}
  src/runtime/{mod.rs,compose.rs,kubernetes.rs,certs.rs}
  src/{pool.rs,autoscale.rs,idle.rs,waker.rs,upcall.rs,quota.rs,restore.rs,upgrade.rs,wal_pool.rs,archive.rs}
  src/pgdog/{mod.rs,render.rs,push.rs,admin.rs,credentials.rs}
  tests/
crates/loams-pgdog-sync/                                    Task 21 (sidecar binary)
crates/loams-sqlrouter-io/src/pgdog/unsharded.rs            Task 20 (with RT1)
crates/loams-safekeeper/src/{send.rs,broker.rs,metrics.rs,tls.rs,offload.rs,membership.rs}   Tasks 30–41
crates/loams-wal-decoder/                                   Task 31 (wraps the fork's wal_decoder)
scripts/loams-pg-bench/{compare.py,targets.toml}            Task 30 (absolute gate)
scripts/pg2/nemesis/                                        Task 41
scripts/ci/no-safekeeper.sh                                 Task 43
crates/loams/src/{main.rs,server.rs}                        Tasks 9, 56, 57 (role and flags)
deploy/neon/compose.yaml                                    Task 2 (digests)
deploy/pgdog/{README.md,LICENSE-NOTICE.md,compose.yaml}     Task 19
deploy/helm/loams-postgres/                                 Task 48
deploy/loams-postgres-single/compose.yaml                   Task 56
deploy/observability/loams-postgres/{dashboards/,alerts.yaml}   Tasks 28, 46
scripts/pg2/{cold-start.sh,wake-bench.sh,restore-drill.sh,chaos.sh,pg-regress.sh,bank-check.py}
scripts/ci/pgdog-licence.sh
conformance/pg-regress/allowlist.tsv                        Task 55
conformance/drivers/                                        Task 29
conformance/router/pgdog-loampg-*.tsv                       Task 29 (re-blessed)
docs/security/loams-postgres-threat-model.md                Task 53
docs/runbooks/loams-postgres/                               Task 60
.github/workflows/{pg2.yml,pg2-e2e.yml,loams-pg-bench.yml}
apps/desktop-electron/src/main/sql/backend/{types.ts,control-plane.ts,local-stack.ts}   Task 58 (after AP1e 21–23)
```

## Shared contracts (all tasks use these names)

### Proto (Task 1 writes it; this is the contract, not the full file)

```proto
syntax = "proto3";
package loams.postgres.v1;

service PostgresService {
  rpc CreateProject(CreateProjectRequest) returns (CreateProjectResponse);           // -> Operation
  rpc GetProject(GetProjectRequest) returns (GetProjectResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc ListProjects(ListProjectsRequest) returns (ListProjectsResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc UpdateProject(UpdateProjectRequest) returns (UpdateProjectResponse);
  rpc DeleteProject(DeleteProjectRequest) returns (DeleteProjectResponse);           // -> Operation
  rpc CreateBranch(CreateBranchRequest) returns (CreateBranchResponse);              // -> Operation
  rpc GetBranch(GetBranchRequest) returns (GetBranchResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc ListBranches(ListBranchesRequest) returns (ListBranchesResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc UpdateBranch(UpdateBranchRequest) returns (UpdateBranchResponse);
  rpc DeleteBranch(DeleteBranchRequest) returns (DeleteBranchResponse);              // -> Operation
  rpc RestoreBranch(RestoreBranchRequest) returns (RestoreBranchResponse);           // -> Operation
  rpc SetDefaultBranch(SetDefaultBranchRequest) returns (SetDefaultBranchResponse);
  rpc CreateEndpoint(CreateEndpointRequest) returns (CreateEndpointResponse);
  rpc GetEndpoint(GetEndpointRequest) returns (GetEndpointResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc ListEndpoints(ListEndpointsRequest) returns (ListEndpointsResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc UpdateEndpoint(UpdateEndpointRequest) returns (UpdateEndpointResponse);
  rpc StartEndpoint(StartEndpointRequest) returns (StartEndpointResponse);           // -> Operation
  rpc SuspendEndpoint(SuspendEndpointRequest) returns (SuspendEndpointResponse);     // -> Operation
  rpc RestartEndpoint(RestartEndpointRequest) returns (RestartEndpointResponse);     // -> Operation
  rpc DeleteEndpoint(DeleteEndpointRequest) returns (DeleteEndpointResponse);        // -> Operation
  rpc CreateRole(CreateRoleRequest) returns (CreateRoleResponse);                    // password once
  rpc ListRoles(ListRolesRequest) returns (ListRolesResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc ResetRolePassword(ResetRolePasswordRequest) returns (ResetRolePasswordResponse);
  rpc DeleteRole(DeleteRoleRequest) returns (DeleteRoleResponse);
  rpc CreateDatabase(CreateDatabaseRequest) returns (CreateDatabaseResponse);
  rpc ListDatabases(ListDatabasesRequest) returns (ListDatabasesResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc DeleteDatabase(DeleteDatabaseRequest) returns (DeleteDatabaseResponse);
  rpc GetConnectionInfo(GetConnectionInfoRequest) returns (GetConnectionInfoResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc IssueConnectCredential(IssueConnectCredentialRequest) returns (IssueConnectCredentialResponse);
  rpc UpgradeProject(UpgradeProjectRequest) returns (UpgradeProjectResponse);        // -> Operation (Task 51)
  rpc WatchProject(WatchProjectRequest) returns (stream WatchProjectResponse);
}
// Every mutating request has `string idempotency_key = 15;`.
// Ids: "prj-", "br-", "ep-", "cmp-" + 26-char Crockford ULID.
// Endpoint.state: ENDPOINT_STATE_{IDLE,STARTING,RUNNING,SUSPENDING,SUSPENDED,FAILED,DELETING}.
// Branch.wal: {commit_lsn, flush_lsn, remote_consistent_lsn, backup_lsn} as "X/Y" strings.
// GetConnectionInfoResponse: host, port, database (PgDog grammar), user, sslmode, ca_pem. No password.
```

### Rust traits (`loams-pg-control`)

```rust
pub trait PgControlStore: Send + Sync + 'static {
    async fn get<R: Record>(&self, key: &R::Key) -> Result<Option<Versioned<R>>, StoreError>;
    async fn put<R: Record>(&self, rec: &R, expected: Option<u64>, fence: &Fence) -> Result<u64, StoreError>; // CAS
    async fn delete<R: Record>(&self, key: &R::Key, expected: u64, fence: &Fence) -> Result<(), StoreError>;
    async fn list<R: Record>(&self, prefix: &R::Prefix, page: Page) -> Result<(Vec<Versioned<R>>, Option<String>), StoreError>;
    async fn acquire_lease(&self, scope: &str, holder: &str, ttl: Duration) -> Result<Fence, StoreError>;
    fn watch(&self, prefix: &[u8]) -> BoxStream<'static, StoreEvent>;
}
pub enum StoreError { Conflict { current: Option<u64> }, Fenced, NotFound, Unavailable, Undetermined, Corrupt }

#[async_trait]
pub trait ComputeRuntime: Send + Sync + 'static {
    async fn ensure_address(&self, ep: &EndpointRec) -> Result<BackendAddr>;      // Service or host port; idempotent
    async fn start(&self, c: &ComputeLaunch) -> Result<ComputeHandle>;            // idempotent by compute_id
    async fn bind_address(&self, ep: &EndpointRec, c: &ComputeHandle) -> Result<()>;
    async fn stop(&self, compute_id: &ComputeId) -> Result<()>;                    // idempotent
    async fn status(&self, compute_id: &ComputeId) -> Result<RuntimeStatus>;
    async fn resize(&self, compute_id: &ComputeId, cu: Cu) -> Result<ResizeOutcome>; // Applied | Refused(reason) | Unsupported
}

pub trait ComputeLifecycleObserver: Send + Sync + 'static {
    fn on_event(&self, e: &ComputeLifecycleEvent); // plain struct, no wire format (D703)
}
pub struct ComputeLifecycleEvent { pub org: String, pub namespace: String, pub project: ProjectId,
    pub endpoint: EndpointId, pub compute: ComputeId, pub event: LifecycleKind, pub at: SystemTime }
pub enum LifecycleKind { Started { cu: Cu }, Resized { cu: Cu }, Suspended, Failed { reason: String } }

#[async_trait]
pub trait SecretStore: Send + Sync + 'static {   // the Q30 credential store's seam
    async fn put(&self, r: &SecretRef, s: Secret<Vec<u8>>) -> Result<()>;
    async fn get(&self, r: &SecretRef) -> Result<Secret<Vec<u8>>>;
    async fn delete(&self, r: &SecretRef) -> Result<()>;
}

#[async_trait]
pub trait PgDogAdmin: Send + Sync {             // RT1's admin adapter, extended
    async fn reload(&self, instance: &InstanceId) -> Result<()>;
    async fn show_pools(&self, instance: &InstanceId) -> Result<Vec<PoolRow>>; // database, user, cl_active, cl_waiting, sv_active, …
    async fn show_databases(&self, instance: &InstanceId) -> Result<Vec<String>>;
}
```

### PgDog routed-name grammar (§46 §8.1)

`name := project [ "__" branch [ ( "." database ) | "__ro" ] ]`. Here `project` and `branch` are names (`[a-z0-9][a-z0-9-]{0,62}`, with no `__`), and `database` is a Postgres identifier. `names::parse_routed` and `names::format_routed` are the only implementations. They round-trip, and Task 4 property-tests them.

---

## Execution order

1. Task 0.
2. **PG2a** (Tasks 1–9), then **PG2b** (10–17), then **PG2c** (19–29).
3. Task 18 (the PgDog verification spike) runs at once, beside PG2a, because its answers shape Tasks 20–27.
4. **PG2d** (30–43) is **launch-critical** and starts at once, beside PG2a, on the bench runner and in CI. Task 31 (the interpreted sender) comes first because it removes the feeder from every measurement. Task 42 needs Tasks 30–41. Task 43 needs Task 31 and Task 39's pool move.
5. **PG2e** (44–55) after PG2c.
6. **PG2f** (56–60): Tasks 56–57 after Task 10 (PG2b), Task 22 (PG2c) and Task 43 (PG2d). Tasks 58–59 after AP1e's Tasks 21–23 have merged.

---

### Task 0: Reconcile with the code as built

**Files:** this plan's "Rulings made during execution" only.

Steps:
1. Answer each of the following and record the answer, with file paths, as a ruling:
   - Is §46 §19's as-built table still true? Check `crates/loams-safekeeper`, `deploy/neon`, `bench/results/`, `crates/loams-meta-tikv/src/keys.rs` (are `x/ X/ C/ E/ R/ D/` still free?), and whether `loams-control`, `loams-operator` and `loams-sqlrouter-io` exist yet.
   - How does `loams-proto`'s `build.rs` pick up a new package, and how is a Connect service mounted in `crates/loams/src/server.rs`? Name the existing example to copy (`InstanceService` or the AP0 services).
   - Which `idempotency_key` replay store do the AP0 services use, if any? If none exists, Task 5 adds `IdempotencyLedger` on `PgControlStore` with a 24 h TTL.
   - What do the OpenFGA model and client of MT1 look like? If MT1 has not merged, Task 9 uses an `Authorizer` trait with an allow-all dev implementation behind `--pg-authz dev` (loopback only) and an OpenFGA implementation when MT1 lands.
   - Which `kube` version and which client setup does the repository use (MT2/MT4 or `loams-operator`)? If neither exists, pick the newest `kube` release that is at least 14 days old.
   - Which `ostrium-labs/neon` commit and image digests to pin (`neon`, `compute-node-v17`; `compute-node-v16` only if Q649 keeps 16)? Record them for Task 2.
   - Q649: Postgres 17 only at GA, or 17 and 18? Record the owner's answer or the default (17 at GA, 18 when §28 §10 step 2 lands).
2. Commit `docs(pg2): task 0 rulings`.

## PG2a — Control plane core and API (Tasks 1–9)

### Task 1: `loams.postgres.v1` protos, reasons and route map

**Files:** create `proto/loams/postgres/v1/postgres.proto`. Modify `crates/loams-proto/build.rs` (or its package list), `docs/api/reasons.md` and `docs/api/route-map.md`. Tests in `crates/loams-proto/tests/postgres.rs`.

**Interfaces:** the service and messages of the shared contract, complete. Every mutating request has `idempotency_key = 15`. The `Operation` kinds are listed in a proto comment.

Tests:
- `buf lint` (STANDARD) and `buf breaking` (FILE, against `dev`) pass in `pg2.yml`.
- `every_mutation_has_idempotency_key`: walks the descriptor set and asserts that every non-`NO_SIDE_EFFECTS` RPC's request has field 15 named `idempotency_key` of type string.
- `connection_info_has_no_password_field`: `GetConnectionInfoResponse` has no field whose name contains `password` or `secret`.
- `reasons_registered`: every reason in §46 §4.1 appears in `docs/api/reasons.md`.

Steps: write the tests (FAIL: package missing) → write the proto → generate → PASS → add the route-map rows (§46 §4.2) → commit `feat(pg): loams.postgres.v1 protos`.

### Task 2: `loams-neon`, the client of Neon's components

**Files:** create `crates/loams-neon/` (as in the file structure). Modify `deploy/neon/compose.yaml` to pin the Task 0 digests, and its README to list them. Tests: `tests/client.rs` (fixtures) and `tests/it_deploy_neon.rs` (`#[ignore]`, run in `pg2-e2e.yml`).

*(Amended by Task 32's review, R32.2.)* Also pin the Neon images of `deploy/loams-pg-bench/compose.yaml` (`neon`, `compute-node-v17`; today `${NEON_TAG:-latest}`) by digest, move `pg2.yml`'s `PG_HEADERS_IMAGE` and `loams-pg-bench.yml`'s to the fork's pinned `neon` image, and add a `pg2-e2e.yml` job that builds `loams-wal-interpreted` and runs `scripts/pg2/it-pageserver-loams-wal.sh` (`it_pageserver_discovers_loams_wal_via_broker`, which is also Task 31's `it_pageserver_ingests_from_loams_wal_without_safekeeper`).

**Interfaces:**
- `NeonClient::new(endpoints: NeonEndpoints, auth: Option<Secret<String>>)`, with methods:
  - `attach_tenant(t, generation, conf)` (`PUT /v1/tenant/{t}/location_config`);
  - `create_timeline(t, TimelineCreate{new_timeline_id, ancestor?, ancestor_start_lsn?, pg_version})` (`POST /v1/tenant/{t}/timeline/`);
  - `list_timelines(t)`, `timeline(t, tl)`, `delete_timeline(t, tl)`;
  - `lsn_by_timestamp(t, tl, ts)`;
  - `tenant_config(t, TenantConfig{pitr_interval, …})`.
- The same methods through the storage controller when `NeonEndpoints.storcon` is set (Task 0 records the routes).
- `WalClient::timeline_status(t, tl)` and `WalClient::create_timeline(...)`: against `loams-wal`'s `GET /v1/tenant/{t}/timeline/{tl}` and `POST /v1/tenant/timeline`. *(Corrected by R2.11: the contract is `loams-wal`'s only; no stock safekeeper fleet exists to migrate, and Task 43 does not reuse this client against one.)*
- `ComputeCtlClient::{status, configure(spec), terminate, promote}`, with a per-compute JWT.
- `spec::ComputeSpecBuilder`, which builds `ComputeSpec` from records (roles, databases, settings, `safekeeper_connstrings` (the `loams-wal` acceptors or pool Service), `pageserver_connection_info`, `storage_auth_token`, `neon.max_cluster_size`).
- `NeonError { status, msg }` maps to `loams.errors.v1` reasons.

Tests:
- `create_timeline_body_matches_fixture`, `branch_body_has_ancestor_and_lsn`, `attach_body_matches_fixture`: the request bodies match the recorded fixtures byte for byte after canonical JSON.
- `timeline_info_parses_fixture`: real responses captured from the pinned image.
- `spec_builder_golden`: one record set produces `tests/fixtures/spec_main.json`.
- `neon_error_maps_to_reason`: 404 → `not_found`, 409 → `already_exists`, 5xx → `storage_unavailable`.
- `it_deploy_neon_tenant_timeline_branch` (ignored by default): attach, timeline, branch, `list_timelines` shows both, then delete.

Steps: capture the fixtures from `deploy/neon` at the pinned digests (the commands go in `tests/fixtures/README.md`) → write the tests (FAIL) → implement → PASS → `cargo deny check` → commit `feat(neon): loams-neon client`.

### Task 3: `PgControlStore`: local and TiKV backends with one conformance suite

**Files:** `crates/loams-pg-control/src/store/{mod.rs,local.rs,tikv.rs,conformance.rs}` and `src/model.rs` (the records of §46 §6.2, postcard-encoded with a format byte). Tests: `tests/store_local.rs` and `tests/store_tikv.rs` (TiKV playground, skipped when `LOAMS_TIKV_PD` is unset).

**Interfaces:** `PgControlStore` as in the shared contract. `pg_control_store_conformance!($factory)`. The keys are §46 §6.2's, under the `loams-meta-tikv` root. The lease scope is `e/pg/<project_id>`.

Tests (inside the macro):
- `put_absent_then_conflict`
- `cas_on_version`
- `delete_requires_version`
- `list_pages_in_key_order`
- `lease_fences_old_holder`: a write with an old `Fence` returns `Fenced`.
- `watch_sees_put_and_delete`
- `undetermined_is_surfaced`: through an injected fault.

Each passes on both backends.

Steps: tests first → local (redb) → TiKV (reuse `loams-meta-tikv`'s `check_fence`) → PASS on both → commit `feat(pg): control store with local and tikv backends`.

### Task 4: Ids, names and the routed-name grammar

**Files:** `src/ids.rs` and `src/names.rs`. Tests are inline, plus `tests/names_prop.rs`.

**Interfaces:**
- `ProjectId`, `BranchId`, `EndpointId`, `ComputeId` are newtypes over a ULID, with prefixes.
- `tenant_id(&ProjectId) -> [u8; 16]` and `timeline_id(&BranchId) -> [u8; 16]` follow §46 §3.
- `validate_name`, `parse_routed` and `format_routed` follow the shared grammar.

Tests:
- `tenant_id_is_deterministic_and_matches_vector`: fixed vectors checked in.
- `routed_roundtrip_prop`: proptest, 10,000 cases.
- `rejects_double_underscore_in_names`
- `ro_suffix_parses`
- `database_with_dot_parses`

Commit `feat(pg): ids and routed names`.

### Task 5: Projects and branches

**Files:** `src/service/{projects.rs,branches.rs,operations.rs,mod.rs}`. Tests: `tests/projects.rs` and `tests/branches.rs`, against the local store and a fake `NeonClient` (a trait `NeonApi` that `loams-neon` implements).

**Interfaces:**
- The project and branch RPCs. Create writes the record in state `creating` and returns an `Operation`. The reconciler (Task 7) does the Neon calls.
- `IdempotencyLedger`, unless Task 0 found an existing one: key `(principal, rpc, idempotency_key)`, value the first response, TTL 24 h.
- *(Amended by R3.14.)* A create whose store write was `Undetermined` and is then retried gets `Conflict` from the store when the first write applied. The ledger, or a writer id kept in the record, must turn that into the first call's answer, not `already_exists`. Test: `create_project_undetermined_then_retry_returns_same_operation`.

Tests:
- `create_project_replay_returns_same_operation`
- `create_branch_same_name_other_key_is_already_exists`
- `create_branch_at_timestamp_resolves_lsn` (the fake returns an LSN)
- `lsn_out_of_retention_refused`
- `delete_branch_with_children_refused`: reason `branch_has_children`
- `protected_branch_delete_needs_admin`
- `list_branches_paginates`
- `get_branch_reports_wal_heads`: from `WalClient` through the fake

Commit `feat(pg): projects and branches`.

### Task 6: Roles, databases and the secret store

**Files:** `src/service/{roles.rs,databases.rs}` and `src/secrets.rs`. Tests: `tests/roles.rs`.

**Interfaces:**
- `SecretStore` as in the shared contract, with `FileSecretStore` (single-node: an age-encrypted file with its key in the OS keyring or a 0600 file) and `KubeSecretStore` (Kubernetes Secrets with KMS encryption at rest). The Q30 store replaces these behind the same trait when it lands.
- `CreateRole` generates 32 random bytes (base64url) and stores the secret. It returns the password once. A replay returns `secret_already_issued`.

Tests:
- `create_role_returns_password_once`
- `reset_password_rotates_and_invalidates_old` (checked against the compute in Task 24)
- `role_secret_never_logged`: captures `tracing` output and the `Debug` of every response, and asserts the password bytes are absent.
- `database_owner_must_exist_on_branch`
- `child_branch_inherits_roles_and_databases`: the records are copied at branch creation.

Commit `feat(pg): roles, databases, secret store`.

### Task 7: Reconcilers and the fenced project lease

**Files:** `src/reconcile/{mod.rs,lease.rs,project.rs,branch.rs,resync.rs}`. Tests: `tests/reconcile.rs`.

**Interfaces:**
- `Reconciler::run(store, neon, runtime, shutdown)`.
- It takes the lease per project. It works on record watch events, plus a 30 s resync, plus explicit kicks (`Kick::Project(id)`).
- Project: attach the tenant, set `pitr_interval`, create the `main` timeline, set `state = ready`.
- Branch: create the timeline, then `ready`. Delete: delete the timeline after its endpoints are gone.
- Each step is idempotent: a 409 or an existing timeline counts as done.
- *(Task 5 fix round 1, R5.10–R5.12.)*
  - The reconciler talks to Neon through `NeonWrite` (and `NeonRead`) over `neon::NeonClients`.
  - It moves each operation from `Pending` through `Running` to `Succeeded` or `Failed`, and sets the operation's progress.
  - When it removes a branch, one fenced batch does all of this:
    - deletes the `BranchRec`, its name index and its guard;
    - decrements the parent's guard (`children - 1`);
    - stamps `updated_at_ms`.
  - It removes a project's records and indexes the same way.
  - *(Task 6, R6.8.)* Removing a branch also removes its roles' and databases' records (`R/<branch>/`, `D/<branch>/`) and then deletes their secrets; the resync sweeps secrets no record names.
  - This needs a fenced `Batch` on `PgControlStore`.

Tests:
- `project_reaches_ready`
- `branch_reaches_ready`
- `two_instances_one_actor`: two reconcilers on one store; the fake Neon records exactly one attach per project.
- `stale_lease_write_is_fenced`: the first holder pauses, the second takes the lease, and the first one's write fails with `Fenced`.
- `crash_between_neon_call_and_record_write_converges`: a fault injected after `create_timeline` leads to the next pass treating 409 as done and reaching `ready`.
- `delete_waits_for_endpoints`

Commit `feat(pg): reconcilers with fenced leases`.

### Task 8: The upcall listener

**Files:** `src/upcall.rs`. Tests: `tests/upcall.rs`.

**Interfaces:**
- `UpcallServer` on its own listener (`--pg-upcall-listen`, default `127.0.0.1:7690` in single-node mode).
- `GET /compute/api/v2/computes/{id}/spec`: Bearer JWT (Ed25519) per compute, `kid` in the `C/` record. It answers `{status:"empty"}` for an unbound pool compute and `{status:"attached", spec, compute_ctl_config}` otherwise.
- `PUT /notify-attach`: a static token or mTLS. It updates the branch shard map, pushes `/configure` to the tenant's computes, and answers 2xx, 423 or 429 as §28 §5.2 says. `notify-safekeepers` is not served: the storage controller manages no safekeepers (§46 §9.1). Task 0 checks the controller's flags for running without them.
- `compute_ctl_config.jwks` carries `pg-control`'s public key.

Tests:
- `spec_requires_compute_jwt`
- `spec_for_unbound_compute_is_empty`
- `spec_is_stable_until_record_changes` (§28 §5.3: same spec for the same compute id)
- `notify_attach_updates_shard_map_and_reconfigures`
- `notify_safekeepers_not_served` (404)
- `upcall_not_served_on_api_listener`

Commit `feat(pg): neon upcall API`.

### Task 9: Authorization, audit, binary wiring and the PG2a end-to-end

**Files:** `src/{authz.rs,audit.rs}`. In `crates/loams/`: `Cargo.toml` (feature `postgres` = `dep:loams-pg-control`), `src/main.rs` (subcommand `pg-control` and flags), `src/server.rs` (mounting `PostgresService`). Add `.github/workflows/pg2.yml` and `pg2-e2e.yml`. Tests: `crates/loams/tests/pg_control.rs` and `crates/loams-pg-control/tests/e2e_deploy_neon.rs` (ignored by default; runs in `pg2-e2e.yml`).

**Interfaces:**
- `loams pg-control --store tikv|local --runtime compose|kubernetes --listen <addr> --pg-upcall-listen <addr> [--pg-authz openfga|dev]`.
- The OpenFGA relations on `pg_project:<id>` are `viewer`, `editor`, `admin` and `connect`, inherited from the namespace. Agents cannot hold `admin`.
- Each mutation emits an audit event (D221) with the principal and the `act` chain.
- *(Amended by Task 1's review, R1.4.)* gRPC reflection lists only the services the process serves: `connect.rs::reflector` builds `Reflector::with_services(served_services())`. Today it advertises every service in `loams-proto`'s descriptor set, including the unserved `PostgresService` and `OperationsService`.

Tests:
- `reflection_lists_only_served_services`: `grpc.reflection.v1` `ListServices` equals the catalogue's available services (`served_services()`, plus reflection and health themselves), with and without the feature `postgres`.
- `default_features_exclude_postgres`: `cargo tree -p loams -e normal --no-default-features --features default` does not contain `loams-pg-control` or `loams-neon`.
- `viewer_cannot_create_branch`
- `agent_cannot_hold_admin`
- `every_mutation_emits_audit_event`
- `no_metering_guard_covers_pg`: the guard script fails on a fixture field named `billable_seconds` under `crates/loams-pg-control`.
- `e2e_create_project_and_branch`: against `deploy/neon` with pinned images, `CreateProject` → `ready`, `CreateBranch` → `ready`, and the pageserver lists both timelines.
- *(Amended by R8.7 and R8.9.)* `api_listener_serves_no_upcall`: the API listener answers 404 for `/compute/api/v2/computes/{id}/spec` and `/notify-attach`. The binary starts `UpcallServer` on `--pg-upcall-listen`, with the API address as `api_listen`. It keeps one signing key shared by every instance across restarts.

Commit `feat(pg): pg-control role, authz, audit`. **PG2a exit:** a project and a branch are created through the API and exist in the pageserver. Every Task 1–9 test passes.

## PG2b — Compute lifecycle (Tasks 10–17)

### Task 10: `ComputeRuntime` and the compose runtime

**Files:** `src/runtime/{mod.rs,compose.rs,certs.rs}`. Tests: `tests/runtime_compose.rs` (unit, with a fake command runner) and `tests/it_compose.rs` (ignored by default).

**Interfaces:**
- `ComputeRuntime` as in the shared contract.
- `ComposeRuntime { bin: ComposeBin, project_dir, port_range: 55500..56000 }` writes one service per compute into `<data>/pg/compose/<ns>.yaml`, which is generated deterministically. It runs `up -d <service>` and `rm -sf <service>` with argument arrays (never a shell string), and a 5-minute timeout.
- `ensure_address` allocates a stable host port per endpoint, stored in `E/`.
- `certs.rs` issues compute certificates from a local CA (rcgen, ECDSA P-256). They last 30 days and renew at 15.

Tests:
- `compose_file_golden`
- `start_is_idempotent_by_compute_id`
- `stop_missing_is_ok`
- `port_is_stable_across_restarts`
- `compose_bin_detection_order` (docker compose, podman compose, docker-compose, as AP1e Task 21)
- `it_compose_compute_answers_select_1`

Commit `feat(pg): compute runtime, compose backend`.

### Task 11: Endpoints and the compute state machine

**Files:** `src/service/endpoints.rs` and `src/reconcile/endpoint.rs`. Tests: `tests/endpoints.rs`.

**Interfaces:**
- The §46 §7.1 states. `StartEndpoint` sets desired `running`. The reconciler then creates a `C/` record, builds the spec (Task 2), calls `runtime.start`, waits for `compute_ctl /status = running` (60 s timeout), calls `bind_address` and sets `running`.
- `SuspendEndpoint` drives `suspending` → `/terminate` → `stop` → `suspended`.
- `RestartEndpoint` replaces the compute.
- At most one `read_write` endpoint per branch: `endpoint_exists_for_branch`.
- Three crashes in 10 minutes lead to `failed`, with the reason.
- *(Amended by R8.9.)* The `C/` record is written with:
  - `binding`, `jwt_key_id` and `ctl_url`;
  - the compute's upcall token (`ComputeKeys::issue_upcall_token`), passed as `NEON_CONTROL_PLANE_TOKEN`.

  A compute older than `COMPUTE_TOKEN_TTL` is replaced, and a spec whose inputs changed is pushed again. Endpoint `pg_settings` are checked against an allow-list. Removing a deleting branch's or project's endpoints is R7.1's cascade.

Tests:
- `start_reaches_running`
- `suspend_reaches_suspended_and_stops_compute`
- `second_rw_endpoint_refused`
- `crash_loop_goes_failed_after_three`
- `start_clears_failed`
- `restart_replaces_compute_keeps_address`
- `delete_endpoint_stops_and_removes_address`

Commit `feat(pg): endpoint lifecycle`.

### Task 12: Idle detection, suspend and the lifecycle observer

**Files:** `src/idle.rs` and `src/observer.rs`. Tests: `tests/idle.rs`.

**Interfaces:**
- `IdleMonitor` samples `compute_ctl /status` (`last_active`; Task 0 records the field) and PgDog's `cl_active`/`cl_waiting` for the endpoint (through `PgDogAdmin`; a fake until PG2c).
- It suspends after `suspend_timeout_s`: default 300, minimum 60, 0 = never.
- An active logical-replication subscriber blocks the suspend.
- `ComputeLifecycleObserver` receives `Started`, `Resized`, `Suspended` and `Failed`. The default implementation is a no-op plus metrics.

Tests:
- `suspends_after_timeout_when_idle`
- `zero_timeout_never_suspends`
- `active_client_blocks_suspend`
- `logical_subscriber_blocks_suspend`
- `observer_sees_start_and_suspend`
- `observer_event_has_no_billing_fields`: a compile-time list of fields, checked against the guard's names.

Commit `feat(pg): idle suspend and lifecycle observer`.

### Task 13: The Kubernetes runtime

**Files:** `src/runtime/kubernetes.rs`. Tests: `tests/runtime_kube.rs` (golden objects) and `tests/it_kind.rs` (runs in `pg2-e2e.yml` on kind).

**Interfaces:**
- `KubeRuntime { client, namespace_for: fn(&ns) -> String /* "loams-pg-<ns>" */ }`.
- `start` creates a Pod named `cmp-<id>` with:
  - the labels `loams.dev/{org,namespace,project,endpoint,compute}` and `argocd.argoproj.io/compare-options: IgnoreExtraneous`;
  - CPU and memory requests and limits from the CU;
  - the compute certificate as a Secret volume;
  - `--control-plane-uri` pointing at the upcall Service;
  - `securityContext`: non-root, read-only root filesystem with an `emptyDir` for data, `seccompProfile: RuntimeDefault`, and an optional `runtimeClassName`.
- `ensure_address` creates a Service `ep-<id>`. `bind_address` sets its selector `loams.dev/compute=<id>`.
- `resize` patches the Pod's `resize` subresource.
- Its RBAC is a Role in each `loams-pg-*` namespace for Pods, Services and Secrets only, and Task 48 ships it.

Tests:
- `pod_golden`
- `service_golden`
- `bind_moves_selector`
- `resize_unsupported_maps_to_outcome`: when the API server lacks the subresource.
- `rbac_allows_only_pods_services_secrets`: a golden Role.
- `it_kind_start_select_1_and_stop`

Commit `feat(pg): kubernetes compute runtime`.

### Task 14: The warm pool

**Files:** `src/pool.rs`. Tests: `tests/pool.rs`.

**Interfaces:**
- `WarmPool { size_per_key: u32 (default 2), key: (region, pg_version, image digest) }` keeps unbound computes. Their spec answer is `empty` (Task 8).
- `take(endpoint)` binds one through `/configure` (or the next spec poll) and refills in the background.
- An image change drains the pool.

Tests:
- `pool_refills_to_size`
- `take_binds_and_refills`
- `image_change_drains_pool`
- `pool_size_zero_disables`
- `bound_compute_gets_attached_spec`

Commit `feat(pg): warm compute pool`.

### Task 15: The autoscaler

**Files:** `src/autoscale.rs`. Tests: `tests/autoscale.rs` (a simulated metrics feed).

**Interfaces:**
- `Autoscaler::tick(now, metrics) -> Vec<Decision>` follows §46 §7.4's policy (pure).
- The applier calls `runtime.resize`, then `ComputeCtlClient::configure` with the new `neon.file_cache_size_limit`.
- `min_cu`, `max_cu`, steps of 0.25 and the limits caps all apply.
- Q644's answer is recorded in the rulings.

Tests:
- `scales_up_after_15s_high_cpu`
- `scales_down_after_2min_low_cpu`
- `never_outside_min_max`
- `step_limits_respected`
- `refused_resize_retries_and_logs`
- `quota_caps_total_cu`

Commit `feat(pg): autoscaler`.

### Task 16: Read-only endpoints and promotion

**Files:** `src/service/endpoints.rs` (extended) and `src/reconcile/endpoint.rs`. Tests: `tests/replicas.rs`.

**Interfaces:** `read_only` endpoints start computes with `mode = Replica` (Task 2's spec builder). Any number per branch, each up to the limit. `RestartEndpoint{promote: true}` on a read-only endpoint whose read-write endpoint is down calls `compute_ctl /promote` and swaps the endpoint types.

*(Amended by Task 32's review, M4.)* Replicas read WAL from `loams-wal`, which publishes `standby_horizon = 0` today. With read-only endpoints, `loams-wal` must take the replicas' hot-standby `apply_lsn` (as the fork's `walsenders.get_hotstandby()`) and publish it as `standby_horizon` in `SafekeeperTimelineInfo` and in discovery responses, so that the pageserver keeps what the replicas still need. Test: `standby_horizon_published_from_replica_feedback`.

Tests:
- `ro_endpoint_starts_replica_spec`
- `ro_rejects_writes` (`it_compose`)
- `promote_swaps_types`
- `promote_refused_while_rw_running`

Commit `feat(pg): read-only endpoints`.
- *(Task 2, R2.13.)* The pinned `compute_ctl` refuses to promote a replica whose local file cache is not prewarmed. A promotion target runs with endpoint storage and `autoprewarm` (`ComputeSpecBuilder::endpoint_storage`, `autoprewarm`), or is prewarmed (`ComputeCtlClient::prewarm`, then `prewarm_state` until `completed`) before `promote`. This task deploys endpoint storage for the stacks that promote, and tests `promote_refuses_an_unprewarmed_replica`.

### Task 17: The cold-start harness and the PG2b gate

**Files:** `scripts/pg2/cold-start.sh` and `scripts/pg2/wake-bench.sh`. Results go to `bench/results/<date>-<sha>-coldstart.json`. Add a `coldstart` job to `.github/workflows/loams-pg-bench.yml` (`workflow_dispatch`, `loam-bench` runner).

**Contract:** measure suspended → `StartEndpoint` → first `SELECT 1`, 50 times, with the pool on and off, on compose and on kind. Record p50, p90 and p99. After PG2c, the same scripts measure through PgDog (Task 26).

Tests: `stats_from_samples` (a unit test of the script's Python helper), plus one recorded run in the PR.

Commit `bench(pg): cold start harness`. **PG2b exit:** endpoints start, suspend when idle, restart and autoscale on compose and on kind. The cold-start numbers are recorded, without gating yet.

## PG2c — PgDog: routing, pooling, TLS, auth and wake-on-connect (Tasks 18–29)

### Task 18: PgDog verification (Q640–Q643) and the version pin

**Files:** this plan's rulings, and `deploy/pgdog/compose.yaml` (a scratch stack with PgDog, two `postgres:17.11` backends and one stopped backend).

Steps. Each answer is a ruling, with the PgDog version, the config used and the observed output:
1. **Version and licence.** The newest PgDog release at least 14 days old: its digest, `LICENSE` (still AGPL-3.0?) and `gh api repos/pgdogdev/pgdog`.
2. **Q640, queueing on a refused backend.** Stop a backend, connect a client and start the backend after 3 s. Does the client succeed, wait or fail? Measure with `checkout_timeout`, `connect_timeout`, `ban_timeout` and the retry settings. Record the exact field names.
3. **Q643, `users.toml`.** Does it accept a SCRAM verifier, or does it need the plaintext? Is a separate server-side user and password possible?
4. **Q642.** Is there native JWT/OIDC client auth? Is there SNI-based routing?
5. Per-user pool mode; replica `role` and read/write-split settings; allowed-address settings.
6. **`RELOAD`.** Does it re-read both files? Do existing client connections survive? How long does it take with 1,000 databases?
7. **The admin database.** The exact `SHOW POOLS`, `SHOW DATABASES`, `SHOW USERS` and `SHOW CLIENTS` columns.
8. **Metrics.** The OpenMetrics port and the series names.
9. **Decide §46 §8.4's path** (queue, retry config, wake shim or Neon proxy) and record it as the binding input for Task 26.

Commit `docs(pg2): task 18 pgdog verification rulings`.

### Task 19: `deploy/pgdog` and the licence rules in CI

**Files:** `deploy/pgdog/{README.md,LICENSE-NOTICE.md}` and `LICENSES.md` (a PgDog row: AGPL-3.0, unmodified, the source link for the pinned tag). Add `scripts/ci/pgdog-licence.sh` and a step in `pg2.yml`.

**Contract:** the README states §46 §8.3's rules verbatim. The script fails when:
- any `Cargo.lock`, `pnpm-lock.yaml` or vendored directory names `pgdog` as a package;
- any file outside `deploy/pgdog`, `deploy/helm/loams-postgres` and `deploy/loams-postgres-single` references a `pgdogdev/pgdog` image;
- a reference lacks an `@sha256:` digest;
- a bundle that references the image lacks `LICENSE-NOTICE.md`.

It also runs `scripts/spec/provenance.sh` on `conformance/router`.

Tests: `no_pgdog_in_lockfiles`, `pgdog_image_pinned_by_digest`, `licence_bundle_present`, as script self-tests with fixtures (`--self-test`).

Commit `ci(pgdog): licence boundary checks`.

### Task 20: Rendering unsharded, per-namespace PgDog config

**Files:** `crates/loams-sqlrouter-io/src/pgdog/unsharded.rs` (if RT1 has not created the crate yet, create it with RT1's module layout and record the ruling) and `crates/loams-pg-control/src/pgdog/render.rs` (records → renderer input). Tests: `tests/golden/pgdog/*.toml` and `tests/render.rs`.

**Interfaces:**
- `render_namespace(ns, projects, branches, endpoints, roles, grants, settings) -> RenderedConfig { pgdog_toml: String, users_toml: Secret<String>, generation: u64 }`.
- It uses `names::format_routed` for every `[[databases]]` name. Hosts are the endpoints' backend addresses.
- Read-only endpoints become `role = "replica"`. Pool sizes come from the CU (§46 §8.1).
- Users are rendered only for granted names, with the pool mode per role.
- `[general]` holds the TLS paths, the Task 18 timeouts and the metrics port.

Tests:
- `render_is_deterministic`
- `render_golden_two_projects_three_branches`
- `ungranted_role_absent_from_users`
- `ro_endpoint_rendered_as_replica`
- `pool_size_follows_cu`
- `sharded_project_delegates_to_rt1_renderer`: a project with a shard map yields RT1's output unchanged.

Commit `feat(pgdog): per-namespace config rendering`.

### Task 21: Config push: the `pgdog-config-sync` sidecar, `RELOAD` and drift

**Files:** `crates/loams-pgdog-sync/` (a binary: mTLS server, atomic file writes into a shared directory, a `GET /generation` route) and `crates/loams-pg-control/src/pgdog/{push.rs,admin.rs}`. Tests: `tests/push.rs` and `crates/loams-pgdog-sync/tests/sync.rs`.

**Interfaces:**
- `PgDogPusher` debounces to 500 ms per namespace. It renders, sends the files to each replica's sidecar, waits for the generation, then sends `RELOAD` through `PgDogAdmin`, and records the generation in `ConfigPush` (RT1).
- On compose, it writes to a bind mount and sends `RELOAD`.
- The resync compares `SHOW DATABASES` and `SHOW USERS` with the rendered generation.

Tests:
- `sync_writes_atomically` (no partial file visible to a concurrent reader)
- `sync_requires_mtls`
- `push_debounces`
- `reload_after_generation_ack`
- `drift_triggers_repush`
- `pgdog_restart_loads_persisted_copy`

Commit `feat(pgdog): config push with sidecar`.

### Task 22: PgDog deployments and the L4 SNI listener

**Files:**
- MT4's tenant renderer hook: a `postgres` addon in `tenants/<org>/namespaces/<ns>.yaml` that renders a PgDog Deployment (×2, anti-affinity across AZs) with its sidecar, Service, NetworkPolicy and PodDisruptionBudget into `loams-pg-<ns>`. If MT4 has not landed, ship these as Helm templates in Task 48's chart, keyed by namespace values, and record the ruling.
- ~~`deploy/helm/loams-postgres/templates/sni-listener.yaml`: an Envoy (or Kourier/Traefik, per MT2's choice) TCP route per namespace hostname `<ns>.<region>.pg.<base-domain>` with TLS passthrough, plus a non-SNI port per namespace.~~ **Replaced by ruling R22.0:** the SNI listener is `loams-gateway` ([GW1](2026-10-16-gw1-loams-gateway.md) Tasks 1–11), and `sni_route_per_namespace_golden` checks the gateway's rendered routes.
- `deploy/loams-postgres-single/` gets one PgDog service.

Tests:
- `pgdog_manifests_golden`
- `sni_route_per_namespace_golden`
- `kubeconform_passes`
- `it_kind_two_namespaces_isolated`: a client of namespace A's hostname cannot reach B's databases.

Commit `deploy(pgdog): per-namespace deployments and SNI routing`.

**Amended 2026-10-16 (R22.0):** the SNI listener is built by GW1; this task deploys PgDog and consumes GW1's gateway.

### Task 23: TLS on both hops

**Files:** `src/runtime/certs.rs` (compute certificates for `ep-<id>.loams-pg-<ns>.svc`), the PgDog certificate through cert-manager (a `Certificate` per namespace hostname in Task 22's templates; the local CA in single-node mode), and the compute spec's `pg_hba` (`hostssl` only). Tests: `tests/tls.rs` and `it_kind_tls`.

Tests:
- `plaintext_refused_both_hops`: a `sslmode=disable` client is refused by PgDog, and a plaintext backend connection is refused by the compute.
- `verify_full_succeeds_with_namespace_hostname`
- `pgdog_verifies_compute_cert`: a compute with a certificate from another CA is refused.
- `cert_renewal_before_expiry`

Commit `feat(pg): tls on both hops`.

### Task 24: SCRAM roles end to end, and authorization before routing

**Files:** `src/pgdog/credentials.rs`, plus wiring of Task 6's roles into the spec (Task 2) and `users.toml` (Task 20). Tests: `it_compose_scram.rs` and `it_kind_scram`.

Tests:
- `scram_login_through_pgdog`: psql with `sslmode=verify-full`, through PgDog.
- `md5_and_plain_refused`
- `unauthorized_database_refused_before_backend`: the compute's `pg_stat_activity` shows no connection attempt.
- `revoked_grant_removed_after_push`
- `reset_password_rotates_at_pgdog_and_compute`

*(0.1.0 blocker, 2026-10-18: the compute half, before PgDog.)* `src/scram.rs` derives each role's SCRAM-SHA-256 verifier from its stored password, salted from its `SecretRef` (fresh per password) so the spec stays a pure function of the records; `RoleVerifiers` caches it per reference and is shared by the upcall listener and the reconciler (`Sources.verifiers`). The spec's `encrypted_password` is the verifier, never the password. Role and database writes kick the reconciler, whose `push_spec` sends `/configure` to the running compute; a compute started later reads it in its spec. Tests: `scram::tests` (a reference vector), `upcall::spec_carries_scram_verifier_never_password`, `it_endpoint::it_role_password_reaches_compute_and_rotates` (in `pg2-e2e.yml`). `GetConnectionInfo` answers the endpoint's `backend_addr` directly until Task 20's PgDog fronts it (`service/connect.rs`). The PgDog half (`users.toml`, `scram_login_through_pgdog`, `md5_and_plain_refused`) stays with this task.

Commit `feat(pg): scram through pgdog`.

### Task 25: Credential exchange for OIDC and agent tokens (`IssueConnectCredential`)

**Files:** `src/service/connect.rs`. Tests: `tests/connect.rs` and `it_compose_token_login`.

**Interfaces:**
- `IssueConnectCredential{project, branch, role?, ttl_s}` checks the gateway-verified principal (an IdP-backed Loams token, D449; the IdP is Zitadel, §76 D1600) and OpenFGA `connect`.
- It creates or reuses `tok_<hex(sha256(sub))[0..16]>` with `VALID UNTIL`, a fresh password and membership in the role. It pushes PgDog synchronously, waiting for the generation, and returns `{connection, password, expire_time}` (the proto's `IssueConnectCredentialResponse`; `connection.user` is the login). TTL defaults to 900 s and is at most 3,600 s. A replay with the same `idempotency_key` answers `secret_already_issued`: the ledger keeps only that a credential was issued, never the password (R1.10).
- The reconciler drops expired roles within 60 s.
- `GetConnectionInfo` returns host, port, database, user and the CA. It stays `NO_SIDE_EFFECTS` (R1.9). For a caller with the `connect` relation, and never one with only `viewer`, it also starts a suspended endpoint as a best-effort hint (D709 step 5). The answer never depends on the start, and the start's failure is not the RPC's.

Tests:
- `issue_requires_connect_relation`
- `ttl_capped_at_one_hour`
- `expired_credential_refused_and_dropped`
- `issued_credential_not_in_operation_result`
- `agent_token_gets_scoped_role_only`
- `get_connection_info_starts_suspended_endpoint` (with `connect`)
- `get_connection_info_viewer_does_not_wake`

Commit `feat(pg): credential exchange for oidc and agents`.

### Task 26: The waker: wake-on-connect

**Files:** `src/waker.rs`. If Task 18 chose the shim, also `src/waker/shim.rs`. Tests: `tests/waker.rs` (fake admin) and `it_compose_wake`, `it_kind_wake`.

**Interfaces:**
- `Waker` polls `show_pools` on each PgDog instance: every 100 ms while any endpoint in the namespace is `suspended`, and every 1 s otherwise.
- When `cl_waiting > 0` for a routed name whose endpoint is suspended or suspending, it calls `wake(endpoint)`. The wake is idempotent through the endpoint record's CAS, and uses the warm pool.
- A suspend in progress is aborted when a waiter appears before `/terminate`, and restarted cleanly after it.
- The fallback path follows Task 18's ruling.

Tests:
- `waiting_client_wakes_suspended_endpoint`
- `concurrent_waiters_one_start`: two PgDog instances report the same waiter, and there is exactly one `runtime.start`.
- `suspend_aborted_when_client_waiting`
- `wake_during_suspending_restarts_cleanly`
- `it_wake_client_sees_no_error`: psql through PgDog to a suspended endpoint gets the result of `SELECT 1`, and records the latency.

Then run Task 17's scripts through PgDog and record the results.

Commit `feat(pg): wake on connect`.

### Task 27: Health, failover, read routing and pool modes

**Files:** `src/pgdog/render.rs` (health and ban settings from Task 18) and `src/reconcile/endpoint.rs` (replace a failed compute behind the same address). Tests: `it_kind_failover` and `tests/render.rs` (extended).

Tests:
- `rw_compute_kill_recovers_under_60s`: pgbench `-c 4` through PgDog. Kill the compute Pod. Transactions resume within 60 s and the bank checker finds no lost commit.
- `ro_name_routes_only_to_replicas`
- `rw_split_off_by_default`
- `session_role_gets_session_pool`
- `prepared_statements_in_transaction_mode`: the pgjdbc and asyncpg cases.

Commit `feat(pg): health, failover and read routing through pgdog`.

### Task 28: PgDog observability

**Files:** `deploy/observability/loams-postgres/dashboards/pgdog.json` and `alerts.yaml` (PgDog rules), and a ServiceMonitor or scrape annotations in Task 22's templates. Tests: `promtool test rules` fixtures under `deploy/observability/loams-postgres/tests/`.

Tests:
- `alert_waiting_clients_on_running_endpoint` (`cl_waiting > 0` for 10 s)
- `alert_pgdog_down`
- `alert_backend_banned_for_60s`
- `dashboard_json_valid`

Commit `deploy(pgdog): metrics, dashboards, alerts`.

### Task 29: Conformance through PgDog: router inventories, drivers and ORMs

**Files:**
- Re-bless `conformance/router/pgdog-loampg-{statements,suites}.tsv` against a Loams Postgres compute through PgDog, and update `conformance/router/README.md` (the target column).
- `conformance/drivers/` with one directory per driver and ORM of §46 §16.1: a small program or script and an `expected.txt`.
- `.github/workflows/pg2-e2e.yml` jobs `router-inventory` and `drivers` on the single-node stack.

Tests:
- `router_inventory_classified`: no row is `pending-target`, and no `error` or `differs` appears in `pool`, `health`, `query` or `copy` (the components Loams uses), per D302's gate.
- `drivers_matrix_green`: each driver and ORM passes connect `verify-full`, transactions, prepared statements in transaction pooling, `COPY`, cancel, and `LISTEN`/`NOTIFY` with a session-mode role.

Commit `test(pg): router inventory and driver matrix through pgdog`. **PG2c exit:** §46 §17's PgDog line is checked off.

## PG2d — `loams-wal` to launch (Tasks 30–43): launch-critical

**The owner's directive (2026-10-08): "remove safekeepers".** `loams-wal` is the only WAL (§46 §9, D714), and PG2d is on the critical path to GA.
- It starts at once and runs beside PG2a–PG2c on the `loam-bench` runner. Its correctness tasks run in CI.
- Every task is a PR to `crates/loams-safekeeper` (or the bench harness) with tests, and each is measured with `scripts/loams-pg-bench/gate.sh` in its absolute mode (Task 30) on the same runner before and after.
- **Diagnose before fixing.** Each performance task states its hypothesis and the experiment that confirms or refutes it, as a ruling, *before* changing code.
- The stores are `tikv`, `tikv-raw` and `nvme` (Arm A). The TiKV hot tier is the default launch candidate (the owner's directive). Arm A is developed alongside, and Task 42 picks the store (Q652).

### Task 30: Reconcile, instrument the commit path, and switch the gate to absolute mode

**Files:**
- `crates/loams-safekeeper/src/{service.rs,nvme.rs,tikv.rs,tikv_raw.rs,journal/mod.rs}`: per-stage timing spans and histograms behind the feature `stage-timing`, off by default.
- `scripts/loams-pg-bench/compare.py`: an `--absolute targets.toml` mode, with §46 §9.3's targets in `scripts/loams-pg-bench/targets.toml`.
- §28 §7.3 and the rulings.

**Interfaces:**
- The stages of an append are `recv`, `queue`, `encode`, `submit`, `durable` and `ack` for the `nvme` store, and `recv`, `tso`, `fence_read`, `prewrite`, `commit` and `ack` for the TiKV stores. Each stage is a histogram, dumped to the run's JSON as `stage_ms{store,stage,p50,p99}`.
- `compare.py --absolute` fails a workload that misses a target and prints the stock-safekeeper reference, when present, for information only (Q654).

**Tests:**
- `stage_timings_sum_to_end_to_end` (in process, mem store; within 5 %);
- `compare_absolute_fails_on_miss` and `compare_absolute_passes_on_meet` (Python, with fixtures).

**Steps:**
1. Re-read `bench/results/gate-*.md` and `2026-10-01-raw/`, and record the table of §46 §9.2 as ruling R30.1.
2. Run one instrumented `bulk` and `commit-16` per store on the laptop, and record where the time goes (R30.2).
3. Calibrate §46 §9.3 on the reference runner (Q653) and record the calibrated targets (R30.3).

Commit `bench(wal): stage timing and absolute gate`.

### Task 31: The in-process interpreted sender (Q112); the feeder removed

**Files:**
- A new crate `crates/loams-wal-decoder`, a thin wrapper that depends on the fork's `wal_decoder`, `postgres_ffi` and `utils` as git dependencies pinned to the Task 0 fork commit;
- `crates/loams-safekeeper/src/send.rs` (the interpreted `START_REPLICATION`: protobuf, zstd level 1, per-shard filtering, as the fork's `send_interpreted_wal.rs`);
- CI: `pg2.yml` extracts the Postgres 17 server headers from the pinned `compute-node-v17` image (`/usr/local/v17/include/postgresql/server`), caches them per digest, and sets `POSTGRES_INSTALL_DIR`.
- `service.rs` stops refusing the interpreted protocol.
- `feeder.rs` and `--feed-safekeeper` are deleted.

**Interfaces:**
- `loams-wal` built with the feature `interpreted` serves the pageserver directly.
- The crate boundary keeps Neon's workspace pins out of `loams-safekeeper`'s default build.
- `cargo deny check` stays clean (Apache-2.0 sources; ported code keeps Neon's `NOTICE`).

**Tests:**
- `interpreted_stream_matches_fork_sender`: a golden comparison of the bytes for a recorded WAL range, against the fork's sender run once to produce the fixture;
- `shard_filter_routes_records`;
- `it_pageserver_ingests_from_loams_wal_without_safekeeper` (`deploy/loams-pg-bench`, no `sk` profile): a compute writes, the pageserver's `last_record_lsn` reaches the commit, and a `SELECT` after a compute restart sees the data;
- `no_feeder_flag_exists`.

Commit `feat(wal): in-process interpreted sender; drop the feeder`.

### Task 32: Storage broker publication and discovery

**Files:** `crates/loams-safekeeper/src/broker.rs` (a gRPC client built from the fork's `storage_broker/proto/broker.proto`, vendored with attribution).

**Interfaces:**
- Publish `SafekeeperTimelineInfo` for each served timeline every 1 s: `safekeeper_id` (the acceptor id, or the TiKV pool's logical id), `safekeeper_connstr`, `commit_lsn`, `flush_lsn`, `remote_consistent_lsn`, `backup_lsn` and `availability_zone`.
- Answer `SafekeeperDiscoveryRequest`.

**Tests:** `publishes_every_second`, `discovery_answers_for_known_timeline`, and `it_pageserver_discovers_loams_wal_via_broker`.

Commit `feat(wal): broker publication`.

### Task 33: `bulk` throughput, diagnosed and fixed

**Files:** decided by the diagnosis.

**Steps.** With Task 30's stage timings and Task 31 merged, run these experiments in order, and record each result as a ruling before any code change:
1. **The feeder hypothesis.** Re-run `bulk` with the in-process sender. If `bulk` rises to target, close the task.
2. **walproposer's in-flight window.** Read `walproposer.c` and record how far it sends ahead of acks, and whether `loams-wal` delays `AppendResponse` beyond the durable point of the bytes it acknowledges (`service.rs`, `nvme.rs` `settle`). Experiment: acknowledge each request as soon as its unit is durable, not at the end of the batch.
3. **Journal shape.** Sweep the unit size (256 KiB–4 MiB), `--io-depth` (1–32) and `--commit-flush-ms`. Record MB/s against WAL CPU, and the stage histograms.
4. **Device contention.** Put the journal on its own filesystem and device, apart from the pageserver and RustFS, and record the difference.
5. **TiKV stores.** The `bulk` stage breakdown (`tso`, `fence_read`, `prewrite`, `commit`), which feeds Tasks 34–35.

Then fix the confirmed cause. Add a regression test where the cause can be shown in process, for example `ack_not_delayed_past_durable_unit`, and a `bulk` row to `targets.toml`.

Commit `perf(wal): <cause>`.

### Task 34: Group commit, batching and pipelining in the TiKV store

**Files:** `crates/loams-safekeeper/src/{tikv.rs,tikv_raw.rs,store.rs}` and the `client-rust` fork (`begin_at(ts)`, and `BatchCommands`-style batching if Task 33 shows RPC overhead, Q118).

**Interfaces:**
- **Pipelined appends** for `tikv` (D267, Q115): up to `--pipeline-depth` fenced transactions in flight per timeline, acknowledged in LSN order.
- **TSO reuse:** the next `start_ts` is the previous `commit_ts`, with a fallback to PD on conflict.
- **Batching:** a linger of at most 200 µs, or 1 MiB.

**Tests:**
- `pipelined_txn_appends_ack_in_order`;
- `deposed_writer_fenced_with_pipelining` (a term bump during in-flight appends: none of the deposed writer's appends is acknowledged after the bump commits);
- `tso_reuse_conflict_falls_back`;
- `batch_respects_linger_and_size`.

Commit `perf(wal): pipelined, batched tikv appends`.

### Task 35: TiKV raw against txn, leader placement and Raft ticks

**Files:** `deploy/loams-pg-bench/tikv.toml` (the WAL-store profile), `scripts/loams-pg-bench/place-leaders.sh` and the rulings.

**Steps:**
1. On three PLP nodes with netem AZ delays (Q114), measure `tikv` (with Task 34) and `tikv-raw` at depths 1, 8 and 32. Record p50, p99, TPS, `bulk` and the stage breakdown.
2. Explain the `txn-placed` regression of 2026-10-01 (30 TPS) before relying on leader placement.
3. Set shorter ticks for the WAL stores (Q117: 200 ms) and measure the commit stall through a leader kill.
4. Rule which TiKV mode is the candidate.

**Tests:** `raw_store_fences_a_deposed_writer` (existing; it must still pass), `leader_kill_stall_under_2s` (manual workflow, results attached).

Commit `bench(wal): tikv modes, placement and ticks`.

### Task 36: The io_uring path, and SQPOLL CPU

**Files:** `crates/loams-safekeeper/src/{shard.rs,bin/loams-wal.rs}`.

**Interfaces:**
- `--uring-sqpoll-shared`: the first shard's ring is created with SQPOLL, and the others attach to its kernel thread (`IORING_SETUP_ATTACH_WQ`, through the raw `io-uring` crate where compio does not expose it; record the approach).
- `--uring-sqpoll-idle-ms` defaults to 2.
- **SQPOLL is off by default**, and `--io auto` picks `uring` without SQPOLL unless Task 42 shows that shared SQPOLL meets §46 §9.3's CPU target.

**Tests:**
- `shared_sqpoll_single_kernel_thread` (counts `iou-sqp-*` threads, on Linux CI);
- `idle_shard_cpu_below_two_percent` (a 30 s idle sample);
- `auto_selects_uring_without_sqpoll`.

Then re-run the gate's `commit-1` and `commit-16` with CPU per transaction.

Commit `perf(wal): shared sqpoll, off by default`.

### Task 37: Metrics and health

**Files:** `crates/loams-safekeeper/src/{metrics.rs,http.rs}`.

**Interfaces:**
- `GET /metrics` with `loams_wal_append_seconds{store}`, `loams_wal_stage_seconds{stage}`, `loams_wal_durable_lag_bytes`, `loams_wal_offload_lag_bytes`, `loams_wal_offload_lag_seconds`, `loams_wal_journal_segments`, `loams_wal_timelines`, `loams_wal_connections{role=proposer|reader}`, `loams_wal_term_changes_total` and `loams_wal_cpu_seconds_total{shard}`.
- `GET /healthz` (unauthenticated) and `GET /readyz` (ready once the journal has replayed and the metadata store is reachable).

**Tests:** `metrics_route_lists_families`, `append_histogram_counts`, `healthz_unauthenticated` and `readyz_false_during_replay`.

Commit `feat(wal): metrics and health`.

### Task 38: TLS and auth

**Files:** `crates/loams-safekeeper/src/tls.rs` and the flags `--tls-cert`, `--tls-key` and `--tls-client-ca`.

**Contract:**
- rustls on the HTTP listener, and Postgres `SSLRequest` on the pg listener for walproposer, the pageserver and the broker. Record whether walproposer supports TLS to safekeepers (verify). If it does not, the compute's hop runs in-cluster under mTLS from the mesh or a sidecar, and this is recorded as a ruling.
- The pageserver and `pg-control` authenticate with `NeonJWT`-compatible tokens scoped to the tenant.
- With TLS configured, plaintext is refused.

**Tests:** `http_tls_required_when_configured`, `pg_sslrequest_negotiated`, `plaintext_refused_with_tls` and `jwt_scoped_to_tenant`.

*(Amended by Task 32's review, R32.6.)* Also the storage broker client: `--broker-endpoint https://...` with `--broker-ca` (the fork's `make_tls_config`: the CA certificates for the broker's TLS), replacing today's refusal of `https://`. Test: `broker_client_tls_with_ca`.

Commit `feat(wal): tls and tenant tokens`.

### Task 39: HA, failover and fencing

**Files:** for Arm A, `crates/loams-safekeeper/src/{acceptor.rs,meta.rs}` and a `membership.rs` (Q261: `pg-control` writes the new member set and generation to TiKV, the new acceptor resyncs from the bucket and a peer, and walproposer gets the new `mconf` through a compute `/configure`). For the TiKV store, a timeout on the reconnect path. In `pg-control`, `wal_pool.rs` maps projects to pools and runs the quiesced move as a saga (§46 §9.5).

*(Amended by Task 32's review, R32.5 and M1.)*
- **The broker peer pull.** Arm A acceptors learn each other's `remote_consistent_lsn`, `commit_lsn` and `backup_lsn` from the broker, as the fork's safekeeper `pull_loop` does (`SubscribeSafekeeperInfo`, `record_safekeeper_info`). An acceptor the pageserver does not stream from then trims and stops publishing on the real pageserver progress, not on Task 32's staleness timeout (R32.5), which stays as the fallback. Test: `peer_remote_consistent_lsn_from_broker`.
- **Publishing after a restart.** At start, an instance loads the timelines its store holds whose pageserver lags (`remote_consistent_lsn < commit_lsn`) into the published set, so a lagging timeline is published again without waiting for a pageserver's discovery request. The TiKV store needs a scan of the heads, and the NVMe store's metadata a listing. Test: `lagging_timelines_published_after_restart`.

**Tests:**
- `acceptor_loss_no_stall` (Arm A);
- `replace_acceptor_resyncs_from_bucket_and_peer`;
- `deposed_compute_gets_no_ack` (both stores);
- `vote_durable_before_term_raise` (crash between the vote write and the response);
- `quiesced_move_between_pools_preserves_data` (checksums);
- `move_crash_at_each_step_resumes_or_compensates`;
- `tikv_instance_kill_is_reconnect`.

Commit `feat(wal): membership, failover and pool moves`.
- *(Task 2, R2.13.)* A failover that promotes a compute replica needs that replica prewarmed (Task 16's endpoint storage and `autoprewarm`); a refused promotion is `failed_precondition`, and the failover replaces the replica instead.

### Task 40: Offload, trim, recover-from-bucket and the WAL archive restore

**Files:**
- `crates/loams-safekeeper/src/offload.rs`: D269 for both stores, with the TiKV lease `B/<tl>` and `.lwal` objects;
- `crates/loams-pg-control/src/archive.rs` and the CLI `loams pg archive-restore`: a weekly pageserver `basebackup` to `pgbase/<tenant>/<timeline>/<lsn>.tar.zst`, then `.lwal` replay into a vanilla Postgres 17 container.

**Tests:**
- `offload_advances_backup_lsn_within_500ms`;
- `trim_waits_for_backup_and_remote_consistent`;
- `bucket_outage_bounds_backlog_and_alerts` (the offload lag metric rises, appends continue up to a disk cap, then backpressure);
- `recover_from_bucket_drill` (lose every hot-tier copy, recover up to `backup_lsn`, restart the compute, data up to `backup_lsn` present);
- `archive_restore_matches_checksums` (vanilla Postgres from basebackup plus `.lwal` equals the pageserver branch at the same LSN).

> **Pointer (2026-10-16, [§58](../design/58-data-access-and-htap.md) §8.5, D1060, Q924):** the offload's create-only `.lwal` writes, unknown-outcome resolution, framing and GC go into the shared bucket-segment library `loams-seglog`, extracted from `loams-git`'s `StoreWalStore` in this task ([DA1](2026-10-16-da1-data-access.md) Task F1). The `.lwal` format is unchanged.

Commit `feat(wal): offload, recover and archive restore`.

### Task 41: Chaos and nemesis

**Files:**
- `scripts/pg2/nemesis/`: a minimal harness, used until RT5's `loams-nemesis` exists: a fault driver over compose and kind (kill, `SIGSTOP`, `tc netem` partitions, `dm-delay` disk stalls, an `fsync`-error hook behind the feature `fault-io`, TiKV and PD leader kills, a RustFS outage) and the bank and list-append checkers;
- `crates/loams-safekeeper/tests/detsim.rs`: the acceptor state machine under `loams-detsim` (RT1) with message loss, reordering and crashes;
- `tests/tla_traces.rs`: Neon's `safekeeper/spec` traces replayed.

**Tests:**
- `nemesis_catalog_no_lost_commit` (each §46 §9.7 fault, 10 minutes each in the nightly job; a list-append history checked for lost or duplicated acknowledged writes);
- `detsim_10k_seeds_safe`;
- `tla_traces_accepted`;
- a manual 72 h soak, with results attached.

Commit `test(wal): nemesis and simulation`.

### Task 42: The store bake-off and the launch gate run

**Files:** `bench/results/` and §28 §7.3 (the table).

**Steps:**
1. On §46 §16.3's topology, run the full protocol: 5-minute runs, `-s 50`, `tpcb-64`, `bulk`, the aggregate test (32 timelines), the CPU measurements and the failover run, for the TiKV candidate (Task 35's mode) and for Arm A (Task 36's default). Run each 3 times.
2. Write the absolute-mode result and the stock-safekeeper reference column.
3. Open the owner-decision issue for Q652 with the table, the risks and a recommendation.
4. The chosen store becomes `pg-control`'s default pool. The other stays behind the setting `postgres.wal.store`.

**Gate:** every §46 §9.3 row green for the chosen store, or an owner-approved revision recorded in §46.

> **Pointer (2026-10-16, [§58](../design/58-data-access-and-htap.md) §8.3, D1058):** the proposed default going into this bake-off (a lean, not a ruling) is Arm A for production, with the TiKV store behind `WalStore` for small and single-node deployments. This task's results and Q652 decide.

Commit `bench(wal): launch gate run <date>`.

### Task 43: Dev data migration, and removing safekeepers from dev

**Files:**
- `loams pg migrate-wal` in the CLI and `crates/loams-pg-control/src/wal_pool.rs` (the quiesced move from a stock safekeeper); *(R2.11.)* the only stock safekeeper is a dev stack's `safekeeper1`, so the command reads it with a client of its own, scoped to the migration, and writes through `loams_neon::WalClient`, which is `loams-wal`'s contract only;
- `deploy/neon/compose.yaml` and its README (`safekeeper1` replaced by `loams-wal --store nvme --features interpreted`, one acceptor, D268's local metadata backend);
- `deploy/loams-pg-bench/compose.yaml` (stock safekeepers only behind the `sk` reference profile, per Q654);
- `scripts/ci/no-safekeeper.sh` and a `pg2.yml` step;
- a note to AP1e's maintainers: the desktop's `stacks/neon` copy follows `deploy/neon`, and Task 58's local-stack backend uses `loams-wal`'s `GET /v1/tenant/{t}/timeline/{tl}`, which serves the same route.

**Tests:**
- `migrate_wal_dry_run_lists_timelines`;
- `migrate_wal_preserves_checksums` (`deploy/neon` with seeded data, migrate, compare);
- `migrate_wal_is_resumable`;
- `no_safekeeper_guard_rejects_fixture` (a fixture compose running Neon's `safekeeper` fails the guard);
- `deploy_neon_smoke_on_loams_wal` (tenant, timeline, branch, compute, `SELECT 1`).

Commit `feat(wal): migrate dev data and remove safekeepers from dev`. **PG2d exit:** Task 42's gate is green on the chosen store; Tasks 31–41 are merged; no artifact runs a stock safekeeper outside the `sk` reference profile.

## PG2e — Operations: PITR, backups, observability, quotas, Kubernetes, upgrades, security and gates (Tasks 44–55)

### Task 44: PITR, restore, TTL and protection

**Files:** `src/restore.rs` (a saga), `src/service/branches.rs` and the branch reconciler (TTL GC). Tests: `tests/restore.rs` and `it_compose_pitr`.

Tests:
- `branch_at_timestamp_sees_state_at_that_time`: write A, note t, write B, branch at t, and the branch has A and not B.
- `restore_keeps_backup_branch_with_ttl`
- `restore_moves_endpoints`
- `restore_crash_at_each_step_converges`
- `ttl_deletes_expired_branch`
- `retention_clamped_to_limit`

Commit `feat(pg): pitr and restore`.

### Task 45: Bucket versioning, replication and the restore drill

**Files:** `deploy/helm/loams-postgres/templates/bucket-policy.yaml` (where the provider supports it; documented otherwise), `scripts/pg2/restore-drill.sh`, and a CronJob template plus a `pg2-e2e.yml` scheduled job.

**Contract:** the drill restores a random timestamp from the last 7 days into a scratch project, checksums a known table set, checks the optional logical export with `pg_restore --list`, deletes the scratch project, and exits non-zero on any mismatch.

Tests: `drill_detects_corruption` (a fixture with a deliberately altered row) and `drill_cleans_up`. Commit `ops(pg): restore drill`.

### Task 46: `loams_pg_*` metrics, traces, dashboards and alerts

**Files:** `src/metrics.rs`, spans in `waker.rs`, `reconcile/endpoint.rs` and `upcall.rs`, plus `deploy/observability/loams-postgres/{dashboards/pg-control.json,dashboards/storage.json,alerts.yaml}`.

Tests:
- `metrics_families_present`: every §46 §13 `pg-control` series.
- `wake_trace_has_all_spans`: an in-memory exporter.
- `promtool test rules` for each §46 §13 alert.
- `traceparent_in_spec`

Commit `feat(pg): observability`.

### Task 47: Quotas

**Files:** the limits record (MT4's `Limits` gains the §46 §14 fields; if MT4 has not landed, add a `PgLimits` record read from the deployment config and record the ruling), and `src/quota.rs`.

*(Task 5 fix round 1, R5.13.)* Also limits branches per project (`pg_branches_per_project`) and branch nesting depth: `CreateBranch` refuses past either with `quota_exceeded`. Nesting depth is read from the parent's chain, or kept in the branch guard.

Tests:
- `projects_quota_refuses_with_reason`
- `total_cu_quota_blocks_start_and_caps_autoscale`
- `storage_limit_sets_max_cluster_size`: the compute refuses writes past it, and reads continue.
- `connections_limit_rendered_into_pgdog`
- `limits_remain_when_writer_absent`

Commit `feat(pg): quota enforcement`.

### Task 48: The Helm chart and the GitOps waves

**Files:** `deploy/helm/loams-postgres/` with:
- the storage controller and its CNPG `Cluster`;
- the storage broker, pageservers (StatefulSet) and the `loams-wal` pool (the chosen store's shape from Task 42: Arm A as a StatefulSet ×3 with zone spread on PLP NVMe, or the TiKV store as a Deployment ×2 per AZ with the `loams_pgwal` keyspace and its placement rules);
- `pg-control` (Deployment ×2) and its RBAC;
- the SNI listener;
- the upcall Service;
- cert-manager `Issuer`s;
- NetworkPolicies;
- values for single-AZ and three-AZ.

Argo CD `Application`s and Flux `Kustomization`s go in MT3's wave layout.

Tests:
- `helm template` golden for both value sets
- `kubeconform`
- `networkpolicy_default_deny_golden`
- `it_kind_install_from_empty_argocd`
- `it_kind_install_from_empty_flux`: the chart reaches Healthy, and `CreateProject` → `StartEndpoint` → psql through PgDog works.

Commit `deploy(pg): helm chart and waves`.

### Task 49: Storage HA

**Files:** chart values and templates, plus storage controller settings. Q646's answer goes in the rulings.

Tests (kind, three zones simulated with node labels):
- `pageserver_loss_fails_over_with_secondaries`: reads resume, and `notify-attach` re-specs the compute.
- `wal_acceptor_or_tikv_leader_loss_within_target` (the §46 §9.3 failover row)
- `storage_controller_leader_loss_recovers`
- `broker_loss_tolerated`

Commit `feat(pg): storage ha`.
- *(Task 2, R2.14.)* `NeonClient::attach_tenant` through the storage controller sends no placement policy and no shard parameters (unsharded, attached, no secondary). This task adds the placement (`Attached(1)` for a secondary) and records its fixture from a deployed controller.

### Task 50: Upgrades

**Files:** `src/upgrade.rs` (rolling computes to a new image in maintenance windows), `scripts/pg2/roll-*.sh` and runbook stubs.

Tests (kind, under pgbench `-c 8` with the bank checker):
- `compute_image_roll_in_window`
- `pageserver_drain_fill_roll`
- `loams_wal_roll_one_at_a_time`
- `pgdog_roll_no_failed_tx_beyond_reconnect`
- `pg_control_roll_no_api_errors_beyond_retry`

Each asserts no lost commit and no failed transaction except connection resets.

Commit `feat(pg): rolling upgrades`.

### Task 51: Major-version upgrade (`UpgradeProject`)

**Files:** `src/upgrade.rs` (a saga: create a project at the target version, `pg_dump | pg_restore` per database, verify row counts and checksums, swap the endpoints, keep the old project for 7 days). Q648 goes in the rulings.

Tests: `upgrade_16_to_17_preserves_data` (or 17 to 18 per Q649), `upgrade_failure_keeps_source_serving` and `upgrade_crash_resumes`. Commit `feat(pg): major version upgrade by dump and restore`.

*(Amended by NF1, D815.)* The saga's first path is NF1 Task 37's:
1. `fast_import pgdata` from the target major's compute into the bucket;
2. `ImportPgdata` into a new root timeline of the same tenant, plus a `loams-wal` timeline at the import's end LSN;
3. a new branch, then the endpoint swap on the caller's request.

`pg_dump | pg_restore` into a new project stays the fallback when the import refuses. Test `upgrade_17_to_18_preserves_checksums` replaces the 16 → 17 case.

### Task 52: The failure table, and the chaos soak

**Files:** `scripts/pg2/chaos.sh`, `scripts/pg2/bank-check.py`, and a `pg2-e2e.yml` nightly job (2 h) plus a manual 72 h workflow.

Tests:
- `kill_pg_control_during_100_creates_leaves_no_orphans`
- each §46 §11 row as a kind test (compute, PgDog, `pg-control`, pageserver, `loams-wal`, storage controller)
- `soak_72h_no_lost_commit`: a manual run, with the results attached to the PR.

Commit `test(pg): failure table and chaos soak`.

### Task 53: The security review

**Files:** `docs/security/loams-postgres-threat-model.md` (§46 §15.2's items, each with its test), image signing (cosign) and SBOMs in the release workflow, and the compute `RuntimeClass` measurement (Q645).

Tests:
- `compute_token_cannot_read_other_tenant`
- `compute_egress_denied_by_default`
- `forged_upcall_refused`
- `neon_superuser_cannot_copy_program`
- `extension_outside_allowlist_refused`
- `agent_quota_bypass_refused`
- `images_signed_and_verified`

Then the external review is booked, and its findings are tracked as issues. GA requires the high and critical ones closed. Commit `security(pg): threat model and tests`.

### Task 54: The performance gate run

**Files:** `bench/results/<date>-pg2-gate.json` and the §46 §16.2 table, with measured columns, in the PR description.

Steps: calibrate on §46 §16.3's topology (Q653), run every §16.2 row 3 times, and write the results. GA needs all of them green, or an owner-approved revision of a target, recorded in §46.

Commit `bench(pg): ga performance gate`.

### Task 55: `pg_regress` and the extensions

**Files:** `scripts/pg2/pg-regress.sh` (runs the compute major version's `parallel_schedule` through PgDog in session mode) and `conformance/pg-regress/allowlist.tsv` (test, reason, issue). Extension smoke tests per §46 §16.1. Q651's allow-list goes in the compute image config.

Tests: `pg_regress_matches_allowlist` (any unlisted failure fails the job, and so does a listed test that now passes, until the list is updated) and `extensions_survive_branch_and_restore`.

*(Amended by NF1, D812.)* Q651's allow-list is NF1's extension catalogue (`compute/loams-extensions.toml` in the fork, NF1 Tasks 20–21): `core` extensions are tested here on every supported major, and `included` ones by `CREATE EXTENSION`.

Commit `test(pg): pg_regress and extensions`.

## PG2f — Single-node mode and the desktop (Tasks 56–60)

### Task 56: `loams dev --postgres`

**Files:** `deploy/loams-postgres-single/compose.yaml` (pinned digests: broker, pageserver with `remote_storage` on a local path, one `loams-wal --store nvme` acceptor with the interpreted sender and the local metadata backend, PgDog with the sidecar) and `crates/loams/src/main.rs` (`--postgres`, `--postgres-stack <dir>`, `--postgres-wal safekeeper|loams-wal`). `pg-control` runs with `--store local --runtime compose`, plus the local CA.

Tests:
- `single_node_compose_golden`
- `it_single_node_project_branch_connect`: one command, then `CreateProject`, `CreateBranch`, `StartEndpoint`, psql through PgDog with `verify-full` against the local CA, suspend, and wake on connect.
- `restart_engine_keeps_state` (the redb store)

Commit `feat(pg): single-node mode`.

### Task 57: Advertising the API, and the CLI

**Files:** `crates/loams/src/server.rs` (`GetInstance.api_versions` gains `loams.postgres.v1`, and the features gain `postgres.control` when the role runs) and the CLI (`loams pg up|down|projects|branches|connect`, track CLI conventions).

Tests: `instance_advertises_postgres_when_enabled`, `instance_omits_postgres_by_default`, and `cli_connect_issues_credential_and_execs_psql` (with a fake exec).

Commit `feat(pg): advertise api and cli`.

### Task 58: The desktop `PostgresBackend` contract

**Precondition:** AP1e Tasks 21–23 have merged. Do not touch `apps/desktop-electron` or `web/` before that.

**Files:** `apps/desktop-electron/src/main/sql/backend/{types.ts,control-plane.ts,local-stack.ts}` and `test/pg-backend.test.ts`.

**Interfaces:**
- `PostgresBackend`, exactly as §46 §18.2 gives it.
- `LocalStackBackend` wraps AP1e's `neon.ts`, and fixes the deviations §46 §19 lists (the `location_config` step, the trailing slash, passing `TENANT_ID` and `TIMELINE_ID` to the stack).
- `ControlPlaneBackend` uses `@loams/proto`'s generated `loams.postgres.v1` client over the active server's transport.
- `selectBackend(flags)` returns the control plane when `flags.has('loams.postgres.v1')`.
- Passwords from `CreateRole` and `ResetRolePassword` go into the desktop's `safeStorage` vault (AP1e Task 10's), never over IPC except through the existing reveal action.

Tests:
- `select_backend_by_flag`
- `control_plane_maps_each_d668_op` (the mapping table of §46 §18.2, against a fake transport)
- `local_stack_unsupported_start_returns_code`
- `password_never_crosses_ipc_without_reveal`

Commit `feat(desktop): postgres backend contract`.
- *(Task 2, R2.2; done in Task 2's fix round 1, R2.17.)* `deploy/neon` now runs Postgres 17. A desktop local stack whose timelines were created at 16 needs a fresh volume: the stacks manager reports `pg_major_mismatch` and offers "Reset local Postgres data" (`compose down -v` after a native confirmation). The local-stack backend keeps that behaviour.

### Task 59: The desktop switch end to end

**Files:** `web/plugins/postgres/` (the header shows "Control plane" or "Local stack"; the Endpoints tab with Start and Suspend when supported) and `apps/desktop-electron/test/e2e/postgres.spec.ts`.

Tests: `e2e_local_engine_with_postgres_uses_control_plane` (the desktop starts `loams dev --postgres`, creates a branch, starts an endpoint, and runs SQL) and `e2e_without_postgres_falls_back_to_local_stack`.

Commit `feat(desktop): postgres page on the control plane`.

### Task 60: Docs, runbooks and plan status

**Files:**
- `docs/runbooks/loams-postgres/{failover.md,restore.md,upgrade.md,wal-incident.md,pgdog.md}`;
- user docs (connect, branch, restore, limits);
- `docs/plans/README.md` (status);
- *(Task 1's review, R1.6.)* At GA, remove `unstable: true` from `loams.postgres.v1`'s `ModuleOptions` and `proto/loams/postgres` from `buf.yaml`'s `breaking.ignore`, so that `buf breaking` protects the package from then on.
- §46 §19 (as built). *(Task 32's review, M9.)* That includes §46 §9 and §28 §6.7, §7.2 and D271, which still describe the feeder and its `--no-sync` safekeeper as the pageserver's path. It was deleted in Task 31/32 (commit `3a91c0dc`; the pageserver finds `loams-wal` through the broker, R32.1–R32.3).
- `docs/design/12-roadmap-testing-risks.md` (a PG2 row).

Steps: each runbook step is executed once on kind and marked verified. Commit `docs(pg): runbooks and status`.

---

## Exit criteria for production (§46 §17, with the owning tasks)

- [ ] **API** served, linted and breaking-checked, with idempotency replay tests and route-map rows: Tasks 1, 5, 6, 9.
- [ ] **Control plane** ×2 on TiKV; no orphans under kills: Tasks 3, 7, 52.
- [ ] **Lifecycle:** start, suspend, wake on connect, restart and delete on compose and Kubernetes; the warm pool; autoscaling inside bounds: Tasks 10–16, 26.
- [ ] **PgDog:** per-namespace deployments, rendering and push, TLS on both hops, SCRAM, credential exchange, authorization before routing, read routing, health and failover, licence CI: Tasks 18–28.
- [ ] **Branching and PITR:** Tasks 5, 44.
- [ ] **HA and DR:** the failure table on kind; the restore drill green twice: Tasks 45, 49, 52.
- [ ] **Upgrades:** compute, storage, PgDog and `pg-control` rolls under load; major version by dump and restore: Tasks 50, 51.
- [ ] **Observability:** endpoints scraped, dashboards, every alert tested: Tasks 28, 37, 46.
- [ ] **Quotas:** every limit refused in a test: Task 47.
- [ ] **Security:** threat model, tests, external review closed for high and critical, signed images: Task 53.
- [ ] **Conformance:** router inventory classified, drivers and ORMs, `pg_regress` and extensions: Tasks 29, 55.
- [ ] **Performance:** §46 §16.2 green on §16.3's topology: Tasks 17, 26, 54.
- [ ] **Deployment:** the Helm chart from empty through Argo CD and Flux; single-node in one command; the desktop on the API: Tasks 48, 56–59.
- [ ] **Docs:** runbooks verified, user docs: Task 60.
- [ ] **The WAL (launch-critical):** `loams-wal` only.
  - Task 42's §46 §9.3 targets are green on the chosen store, with the owner's Q652 ruling.
  - The interpreted sender is in process (Task 31), and broker publication (32), metrics (37), TLS (38), HA and fencing (39) are done.
  - The recover and archive-restore drills pass (40), and the nemesis catalog is clean (41).
  - Dev data is migrated and `no-safekeeper.sh` is green (43).

## Self-review

- **Spec coverage.**

  | §46 section | Task(s) |
  |---|---|
  | §3 Resource model | 4, 5 |
  | §4 API | 1, 5, 6, 11, 25 |
  | §5 Open and private | 9 (guard), 12 (observer) |
  | §6 Control plane | 2, 3, 7, 8, 9 |
  | §7 Lifecycle and autoscaling | 10–17 |
  | §8 PgDog | 18–29 |
  | §9 Storage and the WAL (`loams-wal` only) | 30–43, 49 |
  | §10 Branching and PITR | 5, 40, 44 |
  | §11 HA, backups and DR | 39, 40, 45, 49, 52 |
  | §12 Upgrades | 50, 51 |
  | §13 Observability | 28, 37, 46 |
  | §14 Quotas | 47 |
  | §15 Kubernetes and security | 13, 22, 38, 48, 53 |
  | §16 Gates | 17, 29, 42, 54, 55 |
  | §18 Single-node and desktop | 56–59 |

- **Types.** `PgControlStore`, `ComputeRuntime`, `ComputeLifecycleObserver`, `SecretStore`, `PgDogAdmin` and the routed-name grammar are defined once, in the shared contracts.
- **Review Focus.** Items 1–10 each name an owning test (Tasks 5, 7, 52, 24, 6, 1, 25, 23, 26, 53, 19, 9, 34, 39, 41, 40, 43).
- **Decisions the owner must make before the tasks that need them:** Q640–Q643 (Task 18 measures, then the owner rules if PgDog cannot queue), Q647 and Q649 (before GA and before Task 0 respectively), Q652 (the launch `loams-wal` store, at Task 42), Q654 (the bench reference profile, at Task 30), Q653 (the reference hardware, before Tasks 30 and 54).

## Rulings made during execution

(Task 0 and later tasks append here.)

### Task 0 rulings (2026-10-08, reconciled with the code at `6079e4d7`)

1. **§46 §19 is still true, with two additions.**
   - `crates/loams-safekeeper` (binary `loams-wal`) is unchanged in kind. `deploy/neon/compose.yaml` still uses `${NEON_TAG:-latest}`, `PG_VERSION` defaults to 16, and there is still no storage controller, proxy or TLS.
   - `bench/results/` has two more gate files (`gate-rf3-20260930T174137Z.md`, `gate-rf3-20260930T185121Z.md`). The latest (`nvme-pwritev2`) is still **FAIL**: `commit-1` p99 and TPS are at 29% and 33% of target, and `bulk` is at 50%. Nothing contradicts §19.
   - `crates/loams-meta-tikv/src/keys.rs` prefixes in use are `N/ S/ L/ K/ w/ r/ q/` and `e/m/` (lease scope). `x/ X/ C/ E/ R/ D/` are free (the `E/` and `D/` spellings are free too; only lower-case `e/m/` exists). Task 4 must keep a unit test that no other prefix collides.
   - `loams-control`, `loams-operator` and `loams-sqlrouter-io` do **not** exist in `crates/`.
2. **Proto and Connect wiring (template to copy).** `crates/loams-proto/build.rs` has a hand-kept `FILES` list of paths under `proto/` (for example `loams/instance/v1/instance.proto`); a new package is one more entry, and `cargo:rerun-if-changed` and the descriptor set follow from the list. Note the list currently holds only options, errors, instance and collection (document, query); the AP0 packages (`approvals`, `devices`, `notifications`, `operations`, `live`) exist under `proto/loams/` but are not in `FILES`, so Task 1 adds `loams/postgres/v1/*.proto` itself and nothing more. Mounting: `crates/loams/src/api/connect.rs::routes` builds a `connectrpc::Router` and calls `Arc::new(Instance).register(rpc)` then `connect_collections::register`, `connect_documents::register`, `connect_query::register`; `served_services()` feeds the health and reflection installers. **Copy `InstanceService` (`connect.rs`) for the shape and `connect_documents.rs` for a service holding `AppState`**; the Postgres service goes in a new `api/connect_postgres.rs` behind feature `postgres`, registered in `routes`.
3. **Idempotency.** AP0's `approvals.rs` (in `loams-apps-mock`) replays from an in-memory `decided` map keyed by `idempotency_key`. `crates/loams/src/api/connect_idempotency.rs` is a per-process window ledger for `WriteDocuments` and says so (a retry on another node is a fresh write). `loams-common/src/meta` has `claim/complete/release/prune_idempotency_keys` on the metastore, with `q/` in `keys.rs`, but those are for engine writes. **No store fits `pg-control` (durable, 24 h, fingerprint-checked).** Task 5 adds `IdempotencyLedger` on `PgControlStore` with a 24 h TTL as planned, and borrows the "same key + different fingerprint is an error" rule from `connect_idempotency.rs`.
4. **MT1 / OpenFGA.** MT1 is **Planned**, not merged (`docs/plans/2026-10-02-mt1-authentik-identity.md`); no OpenFGA model or client exists in `crates/` (only design mentions and a buy-vs-build line for `openfga-client`). Ruling: Task 9 ships the `Authorizer` trait with an allow-all dev implementation behind `--pg-authz dev`, which refuses to start unless the listener is loopback, and adds the OpenFGA implementation when MT1 lands (now IA2's `OpenFgaAuthorizer`, §76 D1610).
5. **`kube`.** No workspace `Cargo.toml` depends on `kube`, and MT2/MT4/`loams-operator` are unbuilt (MT4's plan says only that the operator will use `kube`). Ruling: Task 8 picks the newest `kube` release that is at least 14 days old on the day it runs, pins it exactly, and records the version here; `cargo deny` must pass.
6. **Neon fork pins.** `ostrium-labs/neon` `HEAD` is `fa504217c61bbcaf5c512d75830564541f917f8f` (read with `git ls-remote`, 2026-10-08). No registry client (`skopeo`, `crane`, `docker`) is available in this environment, so **image digests are not recorded**; Task 2 must resolve the digests of `neon` and `compute-node-v17` for a fork-built tag and pin them (`@sha256:...`), replacing `latest`. `compute-node-v16` is not kept (ruling 7). The commit above is a starting point; Task 2 confirms it builds and records the final one.
7. **Q649.** No owner answer on record. Default applies: **Postgres 17 only at GA**; 18 when §28 §10 step 2 lands. `deploy/neon` moves from 16 to 17 in Task 2. *(Superseded 2026-10-09 by D813, §51 §8.1: 17 at GA; 18 once NF1e's gate passes, without delaying PG2's GA; 19 beta.)*
8. **Desktop control plane (AP1e Tasks 21-23, built; changes §46 §19's "Desktop" row).** `apps/desktop-electron/src/main/sql/` (`neon.ts`, `pg.ts`, `wesql.ts`, `caps.ts`, `ipc.electron.ts`, `tools.ts`) now drives `deploy/neon` through the pageserver API on `127.0.0.1:9898` and the safekeeper API on `7676` as a local control plane. Rulings on how PG2 relates:
   - `pg.ts`'s `PostgresBackend` is the seam. `loams.postgres.v1` becomes a second implementation (`ControlPlanePostgresBackend`) chosen when the server advertises the API. Task 58 keeps the desktop's `neon.ts` path as the **single-node/dev fallback** and does not delete it, and the RPC shapes must be expressible by `PostgresBackend` (branches, timelines, WAL status, read-only role) without changing the UI contracts in `shared/contracts`.
   - `neon.ts`'s gaps from §19 (tenant `location_config`, trailing slash, `TENANT_ID` and `TIMELINE_ID`) were not re-verified here; Task 58 still checks them.
   - **Roles.** The agent's reads connect as `loams_ro` (`PG_RO_USER`): `LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS`, with per-schema `USAGE` and `SELECT` grants and no role membership (commit `e79413ce`; the `pg.ts` doc comment on line 40 still says `pg_read_all_data` and is stale). PG2's `loams_ro`-equivalent for agent tokens (credential exchange, §46) **uses the same name and the same grant model**, and PgDog's agent-token login maps to it. Tasks that create roles (credential exchange, Task 20 onward) must copy that role attribute list and the per-schema grant approach, and must not grant `pg_read_all_data`. The `wesql.ts` page uses the same `loams_ro` name for MySQL, which is unrelated to PG2.
   - The desktop remains a client of its own local stack; it is not a `pg-control` client until the server advertises `loams.postgres.v1`.
9. **PG2d is launch-critical and starts first.** Every gate file in `bench/results/` is FAIL (latest: `gate-rf3-20260930T185121Z.md`), and there is no safekeeper fallback (owner directive, "remove safekeepers"). Ruling: PG2d (Tasks 30 onward, loams-wal performance) is scheduled **before** PG2a-PG2c in execution order; PG2a-PG2c may proceed in parallel only where they do not touch `loams-safekeeper`. Diagnose bottlenecks before fixing them (§9.4). Launch slips rather than shipping safekeepers.

### Task 31 rulings (2026-10-08, before any code change)

- **R31.1 Hypothesis (to be confirmed by measurement).** The feeder costs the candidate a second WAL pipeline on the same box: the feeder safekeeper's process CPU is 60–75 % of `loams-wal`'s own (`bench/results/20260930T181115Z-71074de-nvme-uring-rf3-r1.json`: `commit-16` 23.3 s against 33.2 s, `tpcb-16` 23.3 s against 32.8 s, `bulk` 1.85 s against 1.81 s), plus its journal writes and its `pread`s on the journal device. *(Corrected in fix round 1.)* Backpressure was **not** part of the mechanism: until fix round 1, `loams-wal` relayed no pageserver feedback to walproposer (`AppendResponse.pageserver_feedback` was always `None`; the feeder's acks were only written to the head as `remote_consistent_lsn`), so walproposer's `max_replication_write_lag`, `flush_lag` and `apply_lag` never engaged. The feeder's cost reached the compute only through the shared CPU, disk and journal reads. Since fix round 1 the feedback is relayed, so backpressure now acts on the candidate, and a pageserver that lags slows the compute; the before and after runs must record the pageserver's lag next to `bulk`. Prediction: with the in-process interpreted sender, `feeder_cpu_s` disappears, WAL CPU per transaction rises by less than the feeder's share (one decode instead of a second safekeeper's write, decode and send), and `bulk` MB/s rises. **Experiment:** `scripts/loams-pg-bench/gate.sh` in absolute mode (Task 30) on the `loam-bench` runner, same store and tier, before (feeder) and after (in-process sender); compare `bulk` MB/s, `wal_cpu_us_per_tx`, `wal_cpu_ms_per_mb` and the stage histograms. It is refuted for `bulk` if `bulk` moves less than run-to-run noise (three runs), and Task 33 then goes to its step 2. **Status: pending** (needs the decoder linked and the runner).
- **R31.2 Blocker: the fork's code is not reachable from this worktree.** `loams-wal-decoder` needs the fork's `wal_decoder`, `postgres_ffi`, `utils` (and, through them, `pageserver_api` and the fork's own git dependencies) at `fa504217c61bbcaf5c512d75830564541f917f8f`. No checkout of `ostrium-labs/neon` exists on this machine and fetching it was not authorized for this task, so the crate cannot be resolved or built here. Consequences:
  - `crates/loams-wal-decoder` is written against the fork's API as recalled, marked unverified, and listed in the workspace's `exclude` so that no `cargo` command in the workspace tries to fetch it. Adding it as a workspace member (or as an optional dependency of `loams-safekeeper`) puts the fork's whole graph in `Cargo.lock`; optional dependencies are still resolved.
  - `cargo deny` sets `unknown-git = "deny"`; the fork, and every git source the fork's workspace uses, must be added to `allow-git` when the crate is wired in.
  - The fork's `postgres_ffi/build.rs` may generate bindings for every supported major version (v14–v17 upstream), not only 17. The CI step therefore extracts every `/usr/local/v*/include/postgresql/server` the pinned image holds, not only v17.
  - The image digest for `compute-node-v17` is Task 2's (ruling 6); the CI job reads it from `deploy/neon/images.env` and fails if it is missing.
- **R31.3 The sender sits behind a trait.** `loams-safekeeper` defines `send::WalInterpreter` and `send::ShardDecoder` with plain types, and `loams-wal-decoder` exposes a plain API (bytes, LSNs as `u64`, shard numbers), so that either wiring works without touching the sender: the plan's feature `interpreted` (loams-safekeeper depends on the wrapper), or Q112's alternative of a binary crate in its own workspace (its own `Cargo.lock`) if the fork's pins conflict with the workspace's. Which one is chosen depends on the lockfile conflicts, which cannot be seen without the fork (question for the owner).
- **R31.4 One decoder per pageserver connection.** Each `START_REPLICATION` with the interpreted protocol gets its own decoder for its own shard. The fork's later design decodes once per timeline and fans out to the attached shards; that saves CPU only for sharded tenants, and is a follow-up if Task 33's numbers ask for it. Reads are `MAX_SEND_SIZE` (128 KiB) chunks up to `commit_lsn`, one batch per chunk with `streaming_lsn` = the chunk's end and `commit_lsn` = the readable end, as the fork's sender frames them (CopyData tag `'0'`, two `u64`s, then the encoded batch).
- **R31.5 (superseded by R31.11: the decoder is linked, and the feeder now waits for Task 32's end-to-end test.) The feeder stays until the decoder is linked.** §28 §6.7 confirmed that a compute on `loams-wal` with no interpreted sender stalls on its first read of a page it wrote. Deleting `feeder.rs` and `--feed-safekeeper` before `loams-wal-decoder` builds would leave the pageserver with no ingest path and break Task 30's laptop runs on `deploy/loams-pg-bench`. The deletion (`feeder.rs`, `--feed-safekeeper`, `WalServiceConfig::feeder`, `ensure_feeder` and its two call sites, the two feeder unit tests, `feeder_copies_committed_wal_to_a_safekeeper`, and the `feeder-safekeeper` compose service) lands in the commit that links the decoder, with `no_feeder_flag_exists`.
- **R31.6 Deviation: the headers come from the `neon` image, not `compute-node-v17`.** Checked on the locally cached stock images, run with `--pull=never`: `ghcr.io/neondatabase/neon` (digest `sha256:ead56a7b…`) has `/usr/local/v14`…`v17/include/postgresql/server`, and `compute-node-v16` has no server headers anywhere (`find / -name postgres.h` is empty). Q112's note agrees. `scripts/pg2/pg-headers.sh` therefore extracts every `/usr/local/vNN/include/postgresql/server` from the pinned `neon` image, which is what the fork's `postgres_ffi` needs if it still binds every major version. *(Updated in R31.10.)* `pg2.yml` takes the image from its `PG_HEADERS_IMAGE` env, pinned to `ghcr.io/neondatabase/neon@sha256:ead56a7b…` (Neon 77e22e4b, the image the golden fixtures' safekeeper came from), and caches the headers per image. Task 2 replaces it with the fork's own image when it pins one; recheck both points then.
- **R31.7 Owner rulings (2026-10-08) resolve R31.2 and R31.3.** The fork is `ostrium-labs/neon` (public; `fa504217` exists there), and every `dina-kar/neon` and `dina-kar/postgres` reference now reads `ostrium-labs/...`. `loams-wal-decoder` takes the fork as a git dependency at that rev, so CI reproduces it. It is **a workspace of its own** with its own `Cargo.lock` and a `loams-wal-interpreted` binary. That binary is `loams_safekeeper::cli::main` with `NeonInterpreter`; the CLI moved from the binary into `loams-safekeeper/src/cli.rs` so that both binaries share it. Neon's tree stays out of the main lockfile and licence check; `deny.toml` lists the fork in `allow-git`, and nothing in the main graph uses it. The binary is not named `loams-wal`, because both workspaces build into the one shared target dir and `run.sh` takes `<target>/release/loams-wal`.
- **R31.8 How the decoder workspace builds.**
  - Its lock is seeded from the fork's own `Cargo.lock`, so Neon's crates build against the versions they were tested with. That includes zstd 1.5.5 (`zstd-sys 2.0.9`) and `async-compression 0.4.5`, which keeps compressed batches byte-identical. Two deviations: `xattr` is bumped from 1.0.0 to 1.6.1 (1.0.0 does not build with the newer `libc` that loams-safekeeper's tree brings), and `tokio` is 1.53.1.
  - Neon's `workspace_hack` (cargo-hakari) is replaced by a stub through `[patch]`. The stub enables only what the linked crates rely on without declaring it (`bytes/serde`), which keeps hakari's whole-workspace set (parquet, jemalloc and more) out.
  - The fork's Postgres submodules (`ostrium-labs/postgres`) are fetched by cargo with `CARGO_NET_GIT_FETCH_WITH_CLI`. Locally, the four submodule commits were seeded into cargo's git database with `git fetch --depth=1` (65 MiB, not the full history), so the cargo package lock was held only briefly.
- **R31.9 Corrections found against the fork's code.**
  - Neon's `Compression` is externally tagged: the option is `"compression":{"zstd":{"level":1}}`, not `{"algo":"zstd",...}`. The stock safekeeper refused the latter, and `protocol_json_matches_the_fork` now pins the shape against Neon's own serde.
  - The fork sends no batch for a chunk that completes no record; `ShardDecoder::decode` returns `Option`.
  - Chunks end at `MAX_SEND_SIZE` cut back to a page boundary, unless they reach the readable end, and never cross a segment (`wal_reader_stream.rs`).
  - Empty records go only to shard zero.
  - Shard identity is `ShardIdentity::from_params`, as the fork's `handler.rs` builds it.
- **R31.10 The golden fixtures and their sender.**
  - `tests/fixtures/wal-17.bin` is 3.7 MB of Postgres 17.11 WAL (about 22k records).
  - The fork's-sender outputs (unsharded, and shard 1 of 2) were recorded from Neon's safekeeper in the locally cached `ghcr.io/neondatabase/neon@sha256:ead56a7b…` (`git-env:77e22e4b`). That commit is an ancestor of `fa504217`, and none of the 11 commits in between touch the decoder, sender, framing, protocol or shard code, so it is the fork's sender. Re-recording gives identical bytes.
  - Results: `interpreted_stream_matches_fork_sender` is byte-identical for both shards (framing, LSNs, batches), and so is the built binary's stream. `pageserver_decodes_the_stream_to_the_commit` shows the pageserver's `from_wire` reading every batch to `next_record_lsn` = commit, with shard 1's records a strict subset.
  - CI pins the headers' image to the same digest until Task 2 pins the fork's.
- **R31.11 The compose end-to-end test needs Task 32.** The pageserver finds safekeepers only through the storage broker (`walreceiver/connection_manager.rs` subscribes to it). `loams-wal` does not publish yet (Task 32), so `it_pageserver_ingests_from_loams_wal_without_safekeeper` cannot pass on `deploy/loams-pg-bench` until then. By the owner's condition the feeder is still not deleted, and `no_feeder_flag_exists` comes with its deletion after Task 32. R31.1's before and after measurement is pending for the same reason (the candidate needs a pageserver that ingests from `loams-wal`) and needs the `loam-bench` runner.
- **R31.12 Fix round 1 (review of 2026-10-08).**
  - *Feedback.* A reader far behind that writes feedback while it waits (the pageserver's walreceiver) deadlocked both streams: feedback went through a 16-deep mpsc that was drained only when idle. It now goes to `watch` channels (the latest wins, nothing waits), both stream loops take it after every chunk, and the latest `PageserverFeedback` is carried by every `AppendResponse` (and pushed on change on the tokio path, as the fork's `network_write` does). Interpreted keepalives set `request_reply`. Shard options follow the fork's `handler.rs` (all three for interpreted, none for vanilla).
  - *The fork trim (owner ruling: never ship CDDL).* `inferno` (CDDL-1.0) came in through `pprof` and `jemalloc_pprof`, from `postgres_ffi` (unused normal dependency) and from `http-utils` via `storage_broker` via `pageserver_api`. Commit `1218fb7a6a37e1b4c268bad5c2952c238d81d368` on branch `loams/decoder-trim` of `ostrium-labs/neon` (on `fa504217`; pushed to `ostrium-labs/neon`, verified with `git ls-remote` on 2026-10-09):
    - `pageserver_api` gets a default-on feature `storage-clients` gating `storage_broker` and `remote_storage` (only `ConfigToml`'s broker and remote-storage fields use them);
    - `wal_decoder` takes `pageserver_api` without default features;
    - `postgres_ffi`'s `pprof` becomes a dev-dependency.

    The fork's default build of the three crates still checks (`cargo check --locked`, its own lockfile unchanged), so the pageserver is unaffected. The decoder's graph drops from 631 to 473 packages: no `inferno`, `pprof`, `storage_broker`, `http-utils`, `remote_storage`, Azure SDK or AWS SDK. The Azure fork the owner provided is therefore not needed.
  - *Sources.* Neon's rust-postgres comes from `ostrium-labs/rust-postgres` at `f3cf448f` through `[patch]`, the `crates-io` `tokio-postgres` patch included. The decoder's git sources are now only `ostrium-labs` forks, and its own `deny.toml` (the root policy plus those sources, kept a superset by `scripts/pg2/check-decoder-deny.py`) passes bans, licences and sources with no exceptions. The root `deny.toml` no longer lists the fork, and the same script fails if the root lockfile names any Neon source.
  - *CI.* `pg2.yml` also runs on pushes to `dev` and includes the root manifests and `crates/loams-tikv` in its paths. It seeds the fork's Postgres submodules with one commit each (`scripts/pg2/seed-neon-submodules.sh`), runs `cargo deny check` (advisories included) on the decoder, and builds the binary with `nvme` and with `tikv`.
  - *Fixtures.* `wal-17.bin.zst` (zstd -19, 687 KB). The fork's sender outputs are now per-body digest lists (`streaming_lsn`, `commit_lsn`, length, sha256), and the tests still require exact equality. Byte identity holds for a static commit: the fixtures were recorded with all WAL committed before the reader started.
- **R31.13 Fix round 2 (re-review of 2026-10-09).**
  - Pageserver feedback goes out only as it arrives, as in the fork. Normal replies leave `pageserver_feedback` unset. Each walproposer connection sends a copy of its last `AppendResponse` per feedback event (tokio and compio paths). The per-timeline channel is a `broadcast` (capacity 64), so several shards' feedback is not collapsed. Nothing is kept, so feedback stops when the pageserver's connection closes. This replaces round 1's "latest feedback in every response", which kept replaying stale feedback.
  - A sharded interpreted request with `shard_stripe_size` 0 is refused again: the decoder would divide by zero.
  - Vanilla keepalives set `request_reply`, as the fork's `send_wal.rs` does (controller ruling).


### Task 32 rulings (2026-10-09)

- **R32.1 What is published.**
  - A timeline is published every second while a proposer streams to this instance, and afterwards while its pageserver still has WAL to persist (`remote_consistent_lsn < commit_lsn`). This mirrors the fork's broker-active set.
  - The values come from the proposer's live view when there is one, and from the store's head otherwise. `commit_lsn` is never above `flush_lsn`.
  - Any instance answers a `SafekeeperDiscoveryRequest` for a timeline its store holds, and publishes that timeline from then on. This covers a pageserver whose timeline this instance has not seen, for example after a restart, or in a pool where the WAL was written through another instance.
  - `safekeeper_id` is `--id`: the acceptor's id for Arm A, and the pool's logical id for a TiKV pool, all of whose instances advertise the pool's address (`--advertise-pg`, `--advertise-http`).
  - The client is tonic, generated from the vendored `broker.proto` (unchanged, from the fork at `1218fb7a`, tag `loams-decoder-trim-1`) under the existing `server` feature. Every CI job that builds it already installs `protoc`.
- **R32.2 The pageserver end-to-end test passes; it is not in CI yet.**
  - `scripts/pg2/it-pageserver-loams-wal.sh` and the ignored wrapper `it_pageserver_discovers_loams_wal_via_broker` (`crates/loams-wal-decoder/tests/it_pageserver.rs`) use `deploy/loams-pg-bench` with no safekeeper and no feeder. A compute writes 400k rows (more than its `shared_buffers`) through `loams-wal-interpreted`; the pageserver's `last_record_lsn` passes the flush LSN within about 3 s; a fresh compute (basebackup from the pageserver) reads the rows back.
  - It passed three times on 2026-10-09: before and after the feeder's deletion, and with a `tikv`-feature binary. In the first run the pageserver found `loams-wal` through discovery before the first publication arrived, so both paths ran.
  - It is not in CI, because the bench's Neon images are still `latest`, not pinned by digest. *(Updated in Task 32's review.)* Task 2's text now owns both: pinning `deploy/loams-pg-bench`'s Neon images by digest, and the `pg2-e2e.yml` job that runs the script. *(Done in Task 2, R2.1 and R2.9.)*
- **R32.3 Task 31's open items.**
  - The compose end-to-end test passes (R32.2), so the feeder is deleted (owner's condition, R31.11), with `no_feeder_flag_exists`.
  - `run.sh` and the benchmark workflow now run `loams-wal-interpreted` with `--broker-endpoint`, and the result JSON no longer has `feeder_cpu_s`.
  - Task 31 now waits only on R31.1's before/after measurement on the `loam-bench` runner. The "before" side has to be an older build (dev before `3a91c0dc`), since the feeder no longer exists at head.
- **R32.4 Startup checks (review I4).** `--broker-endpoint` must be `http://host:port`: a missing scheme is refused, and so is `https://` until Task 38 (R32.6). `--advertise-pg` and `--advertise-http` must be `host:port`, with an IPv6 host in brackets. When the listener is not on loopback, an advertised address that is unspecified or loopback is refused. All of this is checked before the service starts.
- **R32.5 A bounded published set; Arm A staleness (review I2; controller ruling I3).**
  - The broker owns its published set. A timeline is published:
    - while a proposer or a replication reader (pageserver or replica, not the compute's recovery reader) is on this instance;
    - for `--broker-staleness-secs` (default 300) after that, while its pageserver lags;
    - after a discovery request, for 30 s.
  - A timeline leaves the set once it is inactive and stale.
  - So an Arm A acceptor the pageserver does not stream from goes inactive after the staleness timeout, even though no peer tells it the pageserver's `remote_consistent_lsn`. The real peer pull is Task 39's (its text is amended).
  - Each round reads heads 16 at a time within 500 ms, starting where the last round stopped.
- **R32.6 Broker TLS belongs to Task 38 (review I6).** Today `https://` endpoints are refused with a pointer to Task 38. Task 38's text now includes `https://` with `--broker-ca`, as in the fork's `make_tls_config`.
- **R32.7 Notes moved to other tasks (review M1, M4, M9).**
  - Publishing lagging timelines after a restart (loading them from the store at start): Task 39.
  - `standby_horizon` from replica feedback: Task 16.
  - Replacing the feeder text in §46 and §28's as-built descriptions: Task 60.


### Task 1 rulings (2026-10-09)

- **R1.1 `WatchProject` is `NO_SIDE_EFFECTS`.** The contract snippet left it unmarked. It is a read (AP0: reads are `NO_SIDE_EFFECTS`), and `every_mutation_has_idempotency_key` would otherwise require a key on a watch. *Note (review):* the SDKs treat `NO_SIDE_EFFECTS` calls as retry-safe. For a watch, retrying means resuming from the last `cursor`, which the request's `resume_cursor` supports, so a retried watch neither repeats nor loses events. Connect sends server streams as POST, so the level does not turn the watch into an HTTP GET.
- **R1.2 Console routes.** `docs/api/route-map.md` gains a "Planned: `loams.postgres.v1`" section that puts the route first in each row. `route_map_covers_every_route` holds every method-first row to what the binary serves, so these rows only become method-first rows in the main tables when PG2 Task 9 serves them. Until then, `every_rpc_has_a_planned_console_route` checks the section both ways: every row names one of the service's RPCs, and every RPC has a row.
  - §46 §4.2's pattern does not route `UpgradeProject`, `WatchProject`, `RestoreBranch`, `SetDefaultBranch`, `ResetRolePassword`, `GetConnectionInfo` or `IssueConnectCredential`. They get AIP-136 custom methods (`:upgrade`, `:watch`, `:restore`, `:set_default`, `:reset_password`, `:issue_credential`) or a `connection_info` sub-resource.
  - Roles and databases are per branch (§46 §3) but sit under the project, as §4.2 writes them, with the branch in the body or the query (default: the project's default branch).
- **R1.3 SDK exposure waits.**
  - `PostgresService` carries `ModuleOptions` (`postgres`). *(Corrected by R1.6: it is `unstable: true` until GA, and `buf breaking` skips the package; the first version said "stable, because `buf breaking` covers it".)* No method carries `FacadeOptions` yet, so the generated SDK facades expose no `postgres` module.
  - A module with calls needs every SDK template's package map to name the package. The Go SDK's committed stubs (an explicit path list) and `@loams/proto` would need generating, which belongs with serving the API (Task 9) and the API advertisement (Task 57).
  - The facades change only in their package list and the new reasons: the Python, Rust and TypeScript facades are regenerated, and the Go SDK's hand-written fallback `reason.go` is updated by hand (its `TestReasonRegistryMatchesDocs` passes).
- **R1.4 The types are in `loams-proto`'s default build; the service is not served.** As the plan's file list says, `loams/postgres/v1/postgres.proto` joins `loams-proto`'s `FILES`. The default `loams` registers no `PostgresService`; that is Task 9's, behind the feature `postgres`. gRPC reflection advertises every service of the descriptor set (`Reflector` is not curated), as it already does for the unserved `loams.operations.v1.OperationsService`. Task 9 should curate reflection with `Reflector::with_services(served_services())`, so that only served services are listed.
- **R1.5 Reasons.** The nine §46 §4.1 reasons are registered with these codes:
  - `project_not_found` (not_found);
  - `branch_has_children`, `branch_protected` and `secret_already_issued` (failed_precondition);
  - `lsn_out_of_retention` (out_of_range at first; failed_precondition since R1.8);
  - `endpoint_exists_for_branch` (already_exists);
  - `compute_start_failed` and `storage_unavailable` (unavailable);
  - `quota_exceeded` (resource_exhausted).

  A create under a taken name with another key uses the generic `already_exists`. `loams.postgres.v1` is added to §44 §7.2's module catalogue, which `no_proto_declares_a_package_the_design_does_not_enumerate` requires.
- **R1.6 `loams.postgres.v1` is unstable until GA (controller ruling, review I2).** The module carries `unstable: true`, and `buf.yaml`'s `breaking.ignore` lists `proto/loams/postgres`, as for `loams.graph.v1`. Task 60's text now removes both at GA. Until then fields may be renumbered, which fix round 1 used. `pg2.yml`'s `buf breaking` against dev passes for the package by construction until then; `buf lint` still covers it.
- **R1.7 One addressing style (review minor 4).**
  - Every request has a top-level `namespace = 1`, then `project_id` and the child's id. An update carries the resource, whose `id` names it.
  - `project_id` and the console's `{project}` (likewise `{branch}`, `{endpoint}`) are always the ids, never names, because names can change.
  - An empty `update_mask` is `invalid_argument`, not "every field".
  - `DeleteProject`, `DeleteBranch`, `DeleteEndpoint` and `SetDefaultBranch` take `optional uint64 expected_version`. Roles and databases have no version, so their deletes have none.
  - Every `branch_id` that may be empty says it takes the project's default branch.
- **R1.8 Reason codes (review minors 1, 2).**
  - `lsn_out_of_retention` is `failed_precondition`: the point exists, but not in what the project keeps.
  - `endpoint_exists_for_branch` is `CreateEndpoint`'s only. A promote while the branch's read-write endpoint runs answers the generic `failed_precondition`.
  - `secret_already_issued` carries `role` in its metadata and a hint (`ResetRolePassword`, or `IssueConnectCredential` with a new key).
  - `reasons_have_their_connect_codes` pins all nine.
- **R1.9 `GetConnectionInfo` stays `NO_SIDE_EFFECTS` (controller ruling, review I3).** Its wake of a suspended endpoint is a best-effort hint, made only for a caller with the `connect` relation and never for one with only `viewer`. The proto comment and Task 25's text say so, and Task 25 gains `get_connection_info_viewer_does_not_wake`.
- **R1.10 Secrets and the idempotency ledger (review I1).** `CreateRole`, `ResetRolePassword` and `IssueConnectCredential` answer their secret once. The ledger records that a secret was issued, never the secret, so a replay answers `secret_already_issued`. This is stated in the proto header and in `docs/api/reasons.md`.
- **R1.11 `ConnectionInfo` and the credential's shape (review minor 6).**
  - `ConnectionInfo` (host, port, routed database, user, sslmode, CA, URI) is shared by `GetConnectionInfoResponse.connection` and `IssueConnectCredentialResponse.connection`.
  - The credential is `{connection, password, expire_time}`. §46 §8.6 and Task 25 are amended from `expires_at` to the package's `*_time` naming.
  - The TTL defaults to 15 minutes and is capped at 1 hour (a longer request is cut, not refused). This is documented on `ttl`.
  - The no-secret test also refuses `credential`, `token` and `private_key`.
- **R1.12 Branch and worktree names (review minor 9).** This plan runs on branch `backend/pg2` in worktree `~/Documents/Ostriumlabs/loams-wt/pg2`, one branch for all milestones, merged to `dev` task by task. That replaces the Global Constraints' `feat/pg2a-…`…`feat/pg2f-…` branches and the `loams-wt/pg2-postgres-production` worktree.


### Task 2 rulings (2026-10-09)

- **R2.1 The pinned images are Neon's last public build; the fork publishes none.** `ostrium-labs/neon` has no image registry yet, so `deploy/neon` and `deploy/loams-pg-bench` pin Neon's build of 2025-08-26 (git `77e22e4b`; the pinned `pageserver --version` says so) by multi-arch index digest: `ghcr.io/neondatabase/neon@sha256:7a4f124917bb929964b2d696d710f19584f80bb9bd51b2af4a6e2425434c761f` (amd64 manifest `sha256:ead56a7b…`) and `ghcr.io/neondatabase/compute-node-v17@sha256:13ab146d3e7bbabb25a8532f315ac443e7512351d1ede0bab586def5c70e26c3` (amd64 `sha256:9b86e3ec…`). For the APIs `loams-neon` copies, that build is the fork's code: `git log 77e22e4b..loams-decoder-trim-1` is empty for `libs/pageserver_api/src/{models,controller_api}.rs`, `pageserver/src/http/routes.rs`, `storage_controller/src/http.rs`, `libs/compute_api`, `compute_tools/src/http`, `libs/http-utils/src/error.rs` and `safekeeper/src/http`. `NEON_IMAGE` and `COMPUTE_IMAGE` override the pins; `NEON_TAG`, `NEON_REPOSITORY` and `PG_VERSION` no longer pick images. `PG_HEADERS_IMAGE` (`pg2.yml`, `pg2-e2e.yml`, `loams-pg-bench.yml`) moves to the same index digest (same build as R31.6's manifest, so the headers are unchanged; the cache key changes once). When the fork publishes images, the four places to change are the two compose files and those workflow envs. *(Review I5: mirroring the pins to a registry Loams controls is pending the owner's answer.)*
- **R2.2 Postgres 17 (Task 0 ruling 7).** Both stacks run `compute-node-v17`, and the README, `run.sh` and `it-pageserver-loams-wal.sh` create timelines at 17. Checked on 2026-10-09: `deploy/neon`'s `compute1` answers `PostgreSQL 17.5`, and `it-pageserver-loams-wal.sh` passes on the pinned v17 images. A desktop local stack created with 16 needs a fresh volume (`down -v`); Task 58 notes it.
- **R2.3 `NeonError { status, msg, component }`.** The contract's error gains the component it came from (`pageserver`, `storage_controller`, `loams_wal`, `compute_ctl`), because `storage_unavailable`'s registered metadata is `component` (`docs/api/reasons.md`). `error_info()` carries `component` and `status`, never `msg` (it can name internal hosts). *(Amended by R2.15: `status` is dropped, and `component` is sent with `storage_unavailable` only.)*
- **R2.4 The reason mapping.** *(Refined by R2.12: the reason depends on the call too.)* As the task says, 404 is `not_found`, 409 `already_exists`, and 5xx (and an unreachable or unreadable component, status 0) `storage_unavailable`; for `compute_ctl`, which is not storage, the same is the generic `unavailable`. Also: 400 `invalid_argument`, 412 `failed_precondition`, 429 `resource_exhausted`. 401 and 403 are `internal`: the refused credential is Loams's own storage token or compute JWT, and `unauthenticated` would tell the caller to sign in again. Any other status is `internal` (Loams sent a request it should not have).
- **R2.5 Through the storage controller.** `attach_tenant` is `POST /v1/tenant` (`TenantCreateRequest`, unsharded; the controller picks the generation, so the argument is ignored); every other method uses the pageserver's route on the controller, whose timeline create answers `TimelineInfo` flattened with `safekeepers` (ignored) and whose timeline delete waits for the deletion and answers 200. `deploy/neon` runs no controller, so this path is tested against a stand-in server (`storcon_routes`) with the shapes from the fork's source; recorded fixtures wait for the first task that deploys a controller.
- **R2.6 `compute_ctl`.** Its errors are `{"error": ...}` (`GenericAPIError`), read into `msg`. `promote` is `Ok` only when `PromoteState` is `completed`; a failed promotion is its 500 with the error. The pinned `compute_ctl` reads `{spec, compute_ctl_config}` from its config file (both deploy configs already do), and `configure` sends the same pair.
- **R2.7 The spec has one line per setting.** The builder's defaults are `shared_preload_libraries = neon`, `restart_after_crash = off`, `password_encryption = scram-sha-256`, `max_replication_write_lag = 500MB`, `max_replication_flush_lag = 10GB` and, for a primary, `synchronous_standby_names = walproposer`. A caller's setting of the same name replaces a default in place, and the quota's `neon.max_cluster_size` replaces a caller's (`spec_settings_are_unique`). `spec_main.json` was checked by running the pinned `compute-node-v17` with it (basebackup from the pageserver, role and database `app` created, `neon.max_cluster_size` set).
- **R2.8 `Secret<T>` lives in `loams-neon`.** `loams_neon::Secret` is the redacting type the Global Constraints ask for; Task 6's `secrets.rs` re-exports it rather than defining a second one.
- **R2.9 CI.** `pg2.yml` gains `neon-client` (the fixture tests and clippy). `pg2-e2e.yml` runs `it_deploy_neon_tenant_timeline_branch` on `deploy/neon`, and builds `loams-wal-interpreted` (debug, the default store) to run `scripts/pg2/it-pageserver-loams-wal.sh` on `deploy/loams-pg-bench` (R32.2 is done).
- **R2.10 `loams-wal`'s create is idempotent by id.** *(Corrected by R2.11.)* A `POST /v1/tenant/timeline` for an existing timeline answers its head (200) even with other parameters; the pageserver answers a conflicting timeline create with 409. A caller that cares (`pg-control`'s branch reconciler, Task 7) compares the answered head with what it asked for.

#### Task 2 fix round 1 (review of 2026-10-09)

- **R2.11 `WalClient` is `loams-wal`'s contract only (controller ruling, review I1).** The owner removed stock safekeepers, and no fleet of them exists to migrate. `WalClient` promises nothing about a stock safekeeper; R2.10, the task's interface text and Task 43's text are corrected. Task 43's `migrate-wal` reads a dev stack's `safekeeper1` with a client of its own.
- **R2.12 The reason depends on the call and the status (review I2).** `NeonError` carries `op` (`Op`), and `reason()` reads it with the status and, where the fork's text tells two cases apart, the message:

  | Status | Call | Reason |
  |---|---|---|
  | 400 | any | `invalid_argument` |
  | 404 | any | `not_found` (`kind`: `timeline` or `tenant`) |
  | 406 | timeline create | `lsn_out_of_retention` below the ancestor's GC cutoff (`oldest_lsn`); otherwise `failed_precondition` (before the ancestor's own branch point, an archived ancestor, a wait that timed out) |
  | 408 | any | `unavailable` |
  | 409 | timeline delete | `aborted` (a deletion in progress; the controller's 25 s wait ran out) |
  | 409 | other | `already_exists` |
  | 412 | timeline delete | `branch_has_children` ("child timelines", with `children`), `not_found` ("tenant is missing") |
  | 412 | other | `failed_precondition` |
  | 429 | any | `aborted` (the same create, or a second prewarm, already running) |
  | 200 or 500 | promote | `failed_precondition` (R2.13) |
  | 401, 403 | any | `internal` (R2.4) |
  | 0, 5xx | storage | `storage_unavailable` (`component`) |
  | 0, 5xx | `compute_ctl` | `unavailable` |
  | other | any | `internal` |

  The bodies the tests use are the pinned pageserver's own where the capture can provoke them (406 below the ancestor's start, both 412s) and the fork's text otherwise (`src_*`: the GC cutoff, a create in progress, the controller's delete timeout).
- **R2.13 Promotion needs a prewarmed replica (review I3).** The pinned `compute_ctl` refuses `/promote` on a replica whose local file cache is not prewarmed ("compute NotPrewarmed", a 500 `PromoteState::Failed`), and on a primary. `promote` is `Ok` only on `completed`; a refusal, a failure or a 200 that is not `completed` is `failed_precondition`, documented as: replace the replica, or prewarm it and retry. `ComputeCtlClient` gains `prewarm(from_endpoint)` (`POST /lfc/prewarm`) and `prewarm_state()`, and the spec builder `endpoint_storage(addr, token)` and `autoprewarm(bool)` (left out of the spec when unset, so `spec_main.json` is unchanged). Neither stack runs endpoint storage, so **Task 16 (promotion) and Task 39 (HA and failover) own the requirement**: a failover target runs with endpoint storage and `autoprewarm`, or is prewarmed before `promote`; their texts are amended.
- **R2.14 Attach through the storage controller (review M5).** `attach_tenant` sends `TenantCreateRequest` with no `generation` (the controller owns generations) and no `placement_policy` (the controller's default: attached, no secondary), for an unsharded tenant. Placement policies and shards are not supported until Task 49 (storage HA), whose text is amended.
- **R2.15 Client hygiene (review M1–M4, M9, M10).**
  - Timeouts are per call: `configure` and `promote` 300 s, `terminate` 120 s (its `fast` mode holds the answer 30 s), every other call 60 s, and 5 s to connect. A timeout does not mean the call failed: callers poll `/status` and repeat (each of the three is safe to repeat).
  - The constructors return `Result`: a client whose HTTP client cannot be built is an error (`Op::Setup`, `internal`), not a client without timeouts. This changes the contract's `NeonClient::new(endpoints, auth)` to return `Result<NeonClient, NeonError>`.
  - A base URL keeps its path prefix (`http://gw/ps` calls `http://gw/ps/v1/...`). A URL with credentials, a query or fragment, or a scheme other than http(s) is refused; the token goes in `auth`.
  - `tenant_config` is `PATCH /v1/tenant/config` (`TenantConfigPatchRequest`): it sets the settings it names and keeps the others. A typed setting wins over the same name in `extra`, so each name is sent once.
  - `ErrorInfo` carries only the metadata `docs/api/reasons.md` registers for its reason (`component`, `children`, `oldest_lsn`, `kind`); `status` is dropped.
  - Fixtures: the 406, both 412s, `compute_ctl`'s `/status` and 401, `/lfc/prewarm` and a refused promote are recorded from `deploy/neon` (`compute-jwt.py` adds a capture key to `compute1`'s JWKS and signs a per-compute token; `compose.capture.yaml` publishes port 3080). Answers the capture cannot provoke are `src_*`, hand-written from the fork's source, which `tests/fixtures/README.md` lists with the source lines. The redaction test puts sentinels in every token and a SCRAM verifier, and checks the `Debug` and `Display` of every client, builder, spec and error.
- **R2.16 Scripts and CI (review M6–M8).** `run.sh` and `it-pageserver-loams-wal.sh` refuse a `PG_VERSION` other than 17 and a `COMPUTE_IMAGE` that is not `compute-node-v17`. When `NEON_IMAGE` overrides the pin with a tag, `run.sh` records the tag with the digest it resolved to. `pg2-e2e.yml`'s path filters cover every crate and file its builds use; its jobs read the rust caches `pg2.yml` saves on `dev` (`pg2-loams-neon`, `pg2-wal-decoder`) and never save their own; the pinned images are pulled in their own step, with retries.
- **R2.17 The desktop refuses data of another Postgres major (controller ruling, review I4; it overrides the constraint not to edit `apps/desktop-electron` for these files).** The postgres stack's readiness probe compares every timeline's `pg_version` (`GET /v1/tenant`, then `GET /v1/tenant/{t}/timeline`) with 17. A mismatch is the stack error `pg_major_mismatch` with a plain message. "Reset local Postgres data" (`stacks.reset`) runs `compose down -v` on the stack's own project after a native confirmation whose default is Cancel; a declined reset answers `cancelled` and shows no error. It is offered in Settings > Local stacks and on the stack card; the console's fake bridge implements it (`?pgmismatch` previews the state). Desktop branch names in `branches.json` are keyed by timeline id, so names left by a reset never match a new timeline; they are not deleted.
- **R2.18 Mirroring the pinned images (controller ruling, review I5): pending.** Whether the pinned Neon images are mirrored to a registry Loams controls waits for the owner's answer; the pins are unchanged until then (R2.1).

### Task 3 rulings (2026-10-09)

- **R3.1 `PgControlStore` sits on `loams-kv`, with one implementation for both backends.** `loams-kv` (LV1) already gives the embedded store (MVCC on redb) and TiKV the same optimistic transactions, `lock_keys`, timestamps in TSO layout, fault hooks and commit tokens, and holds both to `kv_conformance!`. A redb backend of our own would copy that MVCC and conflict logic. So `KvControlStore` (`src/store/kv.rs`, a file the task's list did not name) implements the trait once over `loams_kv::Store`. `store::local::open(path, options)` opens the embedded store in one redb file, keyspace `loams_pg`, root empty. `store::tikv::open(config, options)` (feature `tikv`) opens a `loams-tikv` handle with the metastore's `TikvConfig` (keyspace `loams_meta` and its root), so the keys sit beside `loams-meta-tikv`'s. The backend still reaches TiKV only through `loams-tikv`, now via `loams-kv`. The features are `tikv` (`loams-kv/tikv`) and `faults` (`loams-kv/faults`, for the injected cases). The default build of the crate has no TiKV code.
- **R3.2 `check_fence` is reused as a rule, not as a function.** `loams-meta-tikv`'s `check_fence` is `pub(crate)`, takes a `loams_tikv::Txn`, and reads the lease at `e/m/<key>`, which is the metastore's own scope. `pg-control`'s leases live at `e/pg/<project_id>`. The fence check in `kv.rs` does the same thing inside each write's transaction. It reads the lease and locks it (`get` plus `lock_keys` is an optimistic `get_for_update`, which is what `check_fence` runs in an optimistic transaction). The write goes through only if the epoch is equal and the lease has an owner. The lease record is the metastore's `Lease` field for field, and its encoding (`FORMAT ‖ postcard`) is byte for byte the same. A test pins that encoding. Acquire follows `loams-meta-tikv`'s rules:
  - Deadlines are judged against the transaction's start timestamp, which is the TSO on TiKV.
  - A free, expired or released lease goes to the next epoch.
  - The current holder renews at the same epoch.
  - Any other holder gets `Held`.
  - The TTL is capped at 10 minutes, as `MAX_LEASE_TTL_MS` is.
  - Only scopes `e/pg/<id>` are accepted, so `pg-control` can never touch `e/m/` or `e/cluster/gc`.
- **R3.3 The contract's `StoreError` gains three variants, and two carry a message.**
  - New variants:
    - `Held { holder, deadline_ms }`: a lease another holder has. Task 7's second reconciler needs to tell this apart from a failure.
    - `InvalidArgument(String)`: a bad key part, page token, lease scope or TTL.
    - `Corrupt(String)`: a stored value that does not decode.
  - `Unavailable(String)` carries the message of the runner's error. It covers a conflict that outlasts the attempts, not-applied, the deadline and fatal errors. Nothing was written in those cases.
  - `Undetermined` stays a bare variant.
  - `Conflict { current }` names the current version, and `None` means the record is absent. `delete` of an absent record is `NotFound`.
- **R3.4 `Fence::Unfenced`.** *(Superseded by R3.10.)* The API service's writes hold no project lease (Task 5 writes the `creating` record and the reconciler acts on it), so `Fence` is `Unfenced | Lease { scope, epoch }`. Unfenced writes are still compare-and-set. Reconcilers always write with their lease's fence.
- **R3.5 Versions are start timestamps.** A write's version is its transaction's start timestamp, or the previous version plus 1 if that is larger. Under snapshot isolation that number grows with every committed write of a key, including across a delete and a re-create. So a version is never reused, and a watch that diffs versions cannot miss a re-create. A stored value is `FORMAT (1) ‖ varint(version) ‖ postcard(record)`.
- **R3.6 Keys and the model.**
  - The records of §46 §6.2, plus the project name index `x/<ns>/n/<name>` as its own record (`ProjectNameRec`).
  - A project listing's prefix is `x/<ns>/prj-`, so it never meets the name index. A project id must start with `prj-`.
  - Every key part except the last must not contain `/` or be empty, and every part is at most 255 bytes. The last part is a role or database name, and it may contain `/`.
  - Ids are the prefixed strings for now. Task 4's newtypes must serialize as the same strings, so the format byte stays 1.
  - `tags_are_disjoint_from_the_metastore` pins the tags `x X E C R D` against `loams-meta-tikv`'s tags, the `e/m/` and `e/cluster/` scopes and the commit tokens' `t/`. Task 4 keeps this test, as Task 0 ruling 1 asks.
- **R3.7 The watch is a poll over its prefix.**
  - `watch(prefix)` scans the prefix in one snapshot. It sends a `Put` for each record, then `Synced`. After that it rescans every `poll` (default 250 ms), and at once after the handle's own writes, and sends a `Put` or `Delete` for each change.
  - Changes between two scans collapse to the latest version.
  - A scan that fails is logged and retried. The stream never ends.
  - This follows `loams-meta-tikv`'s change watch, which uses no change counters (row T5-4).
  - If a reconciler's full-prefix rescans cost too much on TiKV, a scoped change feed (D63) replaces this behind the same `StoreEvent`s.
- **R3.8 Unknown outcomes.**
  - Writes carry a commit token by default (`StoreOptions::commit_tokens`), so `loams-kv`'s runner resolves a lost acknowledgement. `lost_ack_is_resolved_by_its_token`, a ninth case, checks this.
  - `undetermined_is_surfaced` injects `LoseAck` after the commit and before it into a store without tokens. Both surface `Undetermined`, never success or a conflict, and the write applied only in the first case.
  - With tokens, `Undetermined` appears only when the token cannot be resolved for two request timeouts.
- **R3.9 The TiKV tests use `LOAMS_TEST_PD`, not `LOAMS_TIKV_PD`.** Every TiKV suite in the repository, and CI's TiKV job, use `loams_tikv::testing::cluster()` and `LOAMS_TEST_PD`, so `tests/store_tikv.rs` does too. Each case prints `skipped:` when the variable is unset. It runs on the test keyspace `loams_test_meta` under a fresh random root. CI changes:
  - `ci.yml`'s TiKV suites job builds and runs `cargo test -p loams-pg-control --features tikv --test store_tikv`, and its `tikv` filter covers the crate.
  - `pg2.yml` gains `pg-control`, which runs the tests on the local store and clippy on the default and `tikv` builds.

#### Task 3 fix round 1 (2026-10-09)

- **R3.10 Reconcilers cannot write unfenced (review I1).**
  - `Fence` is now an opaque struct (`scope`, `holder`, `epoch`, with private fields), and `Fence::Unfenced` is gone. Only `acquire_lease` and `renew_lease` make a fence.
  - Every write of `PgControlStore` takes a fence.
  - The API service's unfenced, compare-and-set writes exist only on `ApiWriter`. It comes from `KvControlStore::api_writer()`, an inherent method that is not on the trait.
  - Code generic over `S: PgControlStore`, as Task 7's reconcilers must be, has no unfenced path.
  - Doc tests on `PgControlStore` and `Fence` prove it:
    - a `compile_fail` reconciler that reaches for `api_writer()`;
    - a `compile_fail` hand-built `Fence`;
    - a `no_run` fenced write that compiles.
  - Task 5 holds the `ApiWriter`. Task 7 takes the store generically.

- **R3.11 A fence covers only its own project (review).**
  - `Record::project()` names a record's project for `x/` (the project and its name index), `X/`, `E/` and `C/` (the compute's `project_id`).
  - A fenced write needs the lease `e/pg/<that project>`. This is checked against the new record before the transaction, and against the stored record inside it, so a write under prj-A can neither take over nor delete a record of prj-B.
  - A mismatch is `InvalidArgument`: a reconciler bug, not a retryable condition.
  - Role and database records name only a branch, so any project's fence may write them. If Task 6 needs the same guard there, it adds `project_id` to those records, with a new format byte.
  - Test: `fence_covers_only_its_project`.
- **R3.12 `renew_lease(fence, ttl)` (review).**
  - A holder keeps its fence with `renew_lease`, which follows the metastore's renew rule. If the lease is still at the fence's epoch, held by the fence's holder and not expired against the store's clock, it extends the lease and returns the same fence. Otherwise it returns the new error `StoreError::LeaseLost`, and the holder must stop acting.
  - `acquire_lease` keeps the metastore's acquire rule: it extends a lease its holder still holds, at the same epoch, and gives an expired or released lease the next epoch, even to the same holder. A holder that lost its lease therefore never gets its old fence back.
  - Task 7's reconciler renews with `renew_lease` and acquires only at start, or after `LeaseLost`.
  - `lease_fences_old_holder` is rewritten so that no deadline can pass early. `Held` and the renewal run under a 60 s TTL. Then holder A renews with 200 ms, sleeps 600 ms and gets `LeaseLost`. B takes the lease at epoch 2, and A's put and delete are `Fenced`.
- **R3.13 A watch scans only `pg-control`'s keys (review).**
  - `watch(prefix)` now returns `Result<BoxStream, StoreError>`. It refuses, with `InvalidArgument`, a prefix that does not start with `<tag>/` for a tag of `TAGS` (`x X E C R D`). So it never scans the metastore's records under the same root.
  - An undecodable value under a valid prefix is skipped, and reported once per key. The report repeats only if the key decoded or disappeared in between.
  - Scans no longer log from inside the transaction body, which can rerun.
- **R3.14 An undetermined create, retried, is a conflict at the store (review; a note for Task 5).**
  - Commit tokens resolve most lost acknowledgements (R3.8), but not all. When `put(rec, None, …)` answers `Undetermined`, the write may have applied.
  - If it applied, a plain retry gets `Conflict { current: Some(v) }`, which the service would report as `already_exists`, although the record is the caller's own.
  - Task 5 must resolve this, through one of two ways:
    - the `IdempotencyLedger`, written in the same transaction as the record, or claimed before it. A retry with the same `idempotency_key` then finds the operation and answers it.
    - a writer id kept in the record: the operation id, or the idempotency key's hash. On `Conflict`, the service reads the record and treats it as its own when the id matches.
  - Task 5's text gains the test `create_project_undetermined_then_retry_returns_same_operation`, using the conformance suite's injected lost ack.


### Task 5 rulings (2026-10-09)

- **R5.1 One transaction per mutation: `Batch` on `ApiWriter`.**
  - `ApiWriter::commit(Batch)` applies puts, deletes and checks in one `loams-kv` transaction (op `pg.batch`), or none of them.
  - Every expectation is checked, and every outcome and derived body is computed, before the first write. A refusal names the index of the operation that failed (`BatchError { index, error }`).
  - `Batch::put_derived` makes a record inside the transaction from the versions the earlier operations get. The idempotency entry uses it, because its answer has to name versions that only the transaction assigns.
  - `Batch::check` locks a key and fails if the key moved, but it commits no version of its own. On the embedded store a `Lock` mutation is not stored, so a later writer of the key does not notice the check. Where both sides must see each other, both write the key.
  - The conformance case `batch_applies_all_or_nothing` passes on both backends.
  - The batch is on `ApiWriter` only. If a reconciler needs one (Task 7: deleting a branch with its name index), it adds a fenced variant.
- **R5.2 R3.14 is resolved by the ledger, written in the same transaction.**
  - Ledger key: `IdempotencyRec` at `I/<hex(SHA-256(principal ‖ 0 ‖ rpc ‖ 0 ‖ key))>`.
  - The entry holds the principal, rpc, key, the request's SHA-256 fingerprint, the postcard-encoded first answer, and a 24 h expiry.
  - An entry exists exactly when the call's writes applied. A replay answers the entry. The same key on another request is `invalid_argument` (field `idempotency_key`), the rule Task 0 took from `connect_idempotency.rs`. An empty key is never recorded.
  - After `Undetermined`, the service reads the entry: if it is there, the call answers it. Otherwise the call answers `unavailable` ("retry with the same idempotency_key").
  - After a `Conflict` at any index, the service also reads the entry first, and answers it if a concurrent call under the same key won. Only then does the conflict mean `already_exists` (a name index) or another try (up to 5, then `aborted`).
  - Tests:
    - `create_project_undetermined_then_retry_returns_same_operation` (LoseAck after and before the commit, with no commit tokens);
    - `a_conflict_on_the_ledger_answers_the_winner` (a delayed commit loses to a second call under the same key; a probe confirmed it takes the store-conflict path).
  - `IdempotencyLedger::prune(now)` deletes expired entries. Task 9 runs it on a timer.
- **R5.3 New records and tags.**
  - New records:
    - `BranchNameRec` at `X/<project_id>/n/<name>` (a branch name is unique in its project, race-free);
    - `OperationRec` at `O/<project_id>/<op_id>`;
    - `IdempotencyRec` at `I/…`.
  - `TAGS` is now `xXECRDOI`, and `tags_are_disjoint_from_the_metastore` covers `O` and `I`.
  - A branch id must start with `br-`, and a branch listing's prefix is `X/<project_id>/br-`, so a listing never meets the name index (as projects do).
  - No existing record changed, so `FORMAT` stays 1.
  - Operation ids are `op-` and 26 lower-case hex characters (D146): the first 104 bits of a ULID.
  - The reconciler (Task 7) finds a project's pending operations by listing `O/<project_id>/`, and moves them on under its lease (`OperationRec::project()` is the project's).
- **R5.4 Races, handled without a read-write conflict check (snapshot isolation sees only write-write conflicts).**
  - `CreateBranch` rewrites its parent unchanged (`put` at its version). `DeleteBranch` writes the branch. So a child created while its parent is being deleted conflicts whichever commits first. The cost: creating a child moves the parent's `version`, so an `expected_version` the client read before that is answered `aborted`.
  - `DeleteBranch` writes the branch and checks the project. `SetDefaultBranch` writes the project and checks the branch.
  - `CreateBranch` checks the project. A branch created while the project is being deleted is still removed by the project's reconciler.
- **R5.5 `NeonApi` lives in `pg-control` (`src/neon.rs`).**
  - The trait has three calls: `timeline` (head, `min_readable_lsn`, logical size), `lsn_by_timestamp` (`Present | Future | Past | NoData`) and `wal_heads`. It uses its own `Lsn` (`X/Y` text), and an error already mapped to a reason (`NeonApiError { reason, component, message }`).
  - `loams-postgres` (the renamed `loams-neon`) does not implement it yet, because Task 5 ran during NF1's rename. Task 7 adds the implementation, over `NeonClient` and `WalClient`, together with the storage-changing calls.
- **R5.6 Branch points and retention.**
  - A point needs a `ready` parent (`failed_precondition` otherwise).
  - These are `lsn_out_of_retention` (metadata `oldest_lsn` when known, and `history_retention` as `"<s>s"`): a time older than the project's `history_retention`, a time Neon answers `Past` for, or an LSN below the parent's `min_readable_lsn`.
  - An LSN past the parent's head is `invalid_argument`. `Future` resolves to the head LSN Neon returns. `NoData` is `failed_precondition`.
  - The head (no point) leaves `ancestor_lsn` unset, and the reconciler branches at the head.
  - `parent_time` is not kept: `BranchRec` has no field for it, and adding one needs a new format byte. A later task that needs it adds the field.
- **R5.7 The service's shape.**
  - `PgService<N: NeonApi>` takes and answers plain Rust types (`service::{projects, branches}`). Task 9 maps them to the proto.
  - `Caller { principal, admin }`: the principal scopes the ledger, and `admin` (from Task 9's `Authorizer`) gates deleting a protected branch, lifting a branch's protection, and deleting a project that has a protected branch.
  - Defaults (`ServiceConfig`): Postgres 17 (the only major offered), region `local`, WAL pool `default`, retention 7 days, clamped to 30 days (Task 47 takes the limit from the namespace's limits record).
  - `GetBranch` reads WAL heads and logical size only for a `ready` branch. A component that does not answer leaves its part empty, and is logged. `ListBranches` reads neither.
  - The default branch cannot be deleted (`failed_precondition`). A branch that is already deleting is `failed_precondition`, and so is deleting a project that is already deleting.
- **R5.8 Not in Task 5.**
  - `RestoreBranch` belongs to Task 44.
  - Agents' approvals for protected branches belong to Task 9.
  - The reconciler's own steps (`ready`, the operation's end, removing the records and the name indexes on delete) belong to Task 7.

#### Task 5 fix round 1 (2026-10-09, controller ruling: change the records now, while none exist outside tests)

- **R5.9 Branch points are bounded by `loams-wal`'s `commit_lsn`.** This overrides R5.6's upper bound.
  - The pageserver's `last_record_lsn` lags what `loams-wal` has acknowledged. So a point must lie in `[parent.ancestor_lsn, commit_lsn]` and must not be below the pageserver's `min_readable_lsn`.
  - A point below the parent's start is `invalid_argument`, with `oldest_lsn` set to the start. A point past `commit_lsn` is `invalid_argument`.
  - A head branch of a `ready` parent records the parent's `commit_lsn` as `ancestor_lsn`. A head branch of a parent that is still `creating` records none, and the reconciler branches at the head.
  - A time the pageserver answers `Future` for is the head.
  - Tests: `a_branch_right_after_a_commit_is_accepted`, `a_head_branch_records_an_lsn`, `a_point_before_the_parent_starts_is_refused`.
- **R5.10 The branch guard replaces R5.4's rewrite of the parent.**
  - `BranchGuardRec` at `X/<prj>/g/<branch_id>` holds `{children, deleting}`. It is created with the branch, and `main`'s with the project.
  - `CreateBranch` CAS-puts the parent's guard with `children + 1`, and is refused while the guard says `deleting`.
  - `DeleteBranch` CAS-puts its own guard with `deleting`, and is refused while `children > 0`. `branch_has_children` is now an O(1) read.
  - Both write the guard, so the races conflict on both backends.
  - The parent's `BranchRec` and its version no longer change when a child is created.
  - Task 7 decrements `children` in the fenced batch that removes a child. Its text is amended.
  - `DeleteBranch` also accepts a `Failed` branch.
- **R5.11 Timestamps, and replays that survive record changes.**
  - `BranchRec` gains `parent_time_ms` and `updated_at_ms`, and `ProjectRec` gains `updated_at_ms`. `FORMAT` stays 1, by the controller's ruling.
  - Ledger answers are a tag (`ANSWER_JSON` = 1) followed by JSON, no longer postcard. A record that gains a field marked `serde(default)` still replays (`a_replay_survives_a_record_gaining_a_field`).
  - A change JSON cannot absorb takes a new tag, and the old tag's decoder is kept for 24 h. An unknown tag answers `failed_precondition` ("retry with a new key").
- **R5.12 Operations are indexed, and gain `AwaitingApproval`.**
  - `OperationIdRec` (`Q/i/<op_id>`) serves `GetOperation`, which names only the id. `NamespaceOperationRec` (`Q/n/<ns>/<op_id>`) serves `ListOperations` and `WatchOperations`, in creation order.
  - Both are written in the operation's batch.
  - The tag is `Q`, because `o` is the metastore's. `TAGS` is now `xXECRDOQI`.
  - The `AwaitingApproval` state is for Task 9's approvals.
- **R5.5 superseded: `NeonRead` and `NeonWrite`.**
  - `NeonApi` splits into `NeonRead` (the service's reads) and `NeonWrite` (Task 7's changes). Both use `loams-postgres`'s typed `TenantId`, `TimelineId` and `Lsn`, and a typed `Reason` with a `Component`.
  - `neon::NeonClients` implements both over `NeonClient` and `WalClient`. It lives in `pg-control`, so the dependency runs one way: `loams-pg-control` depends on `loams-postgres`.
  - `NeonClient::lsn_by_timestamp` gains `with_lease`, which sends `&with_lease=true` (the fork's `get_lsn_by_timestamp_handler`), and `LsnByTimestamp` gains `valid_until`. The adapter always asks for the lease.
  - The lease response body in the test is a stand-in from the fork's source. A capture from `deploy/loams-postgres-dev` is still to do (Task 7's end-to-end).
- **Other review fixes.**
  - Every try of a mutation asks the ledger again, so an expired entry is replaced, and then replayed.
  - A non-admin's `DeleteProject` checks, in its batch, every branch it read.
  - `UpdateBranch` refuses while the project is deleting, and refuses an expiry that is not in the future. An expiry on a protected branch or the default branch needs `admin`.
  - A mutation with an empty principal is `unauthenticated`.
- **Tests.**
  - The service cases are in `tests/service/` and run in `service_local`, and in `service_tikv` (feature `tikv`, `LOAMS_TEST_PD`; CI's TiKV suites job runs it).
  - The races are deterministic, through `ServiceConfig::before_commit`, a hook that commits a competing write between a mutation's reads and its commit.
  - The R3.14 test asserts each fault point's exact outcome.
  - The conformance suite gains `concurrent_batches_lose_no_update`.
- **R5.13 Recorded for later tasks.**
  - Limits on branches per project and on nesting depth: Task 47, whose text is amended.
  - Operations: Task 7 sets progress and the end states. Collecting finished operations and their indexes (and the ledger's prune timer) belongs to Task 9's wiring, and their retention to Task 46's runbook.

### Task 6 rulings (2026-10-10)

- **R6.1 One secret, one reference, one record.**
  - Every password issued (create, reset, a child branch's copy) is stored under a fresh `SecretRef` (`pg-role-<project ulid>-<ulid>`, lower case, a valid Kubernetes object name). Only one `RoleRec` ever names a reference.
  - Order: the secret is stored, then the record that names it commits, in one batch with the ledger entry. A reset deletes the old secret after its new record commits.
  - A secret whose record did not commit is deleted again, but only after the service reads that no record names it. After `Undetermined` it is kept, because the record may name it.
  - So a crash, a failed delete or an unknown outcome can leave an unused secret, never a record without its secret. Sweeping unused secrets needs a `list` on `SecretStore`; it belongs with Task 7's resync.
- **R6.2 Records.**
  - `RoleRec` gains `project_id`, `system`, `created_at_ms` and `updated_at_ms`. `secret_ref` is typed `SecretRef`, which encodes as the same string.
  - `DatabaseRec` gains `project_id` and `created_at_ms`.
  - Both now name their project (`Record::project`), so a fenced write of either needs that project's lease (R3.11).
  - `FORMAT` stays 1: no role or database record exists outside tests (R5.11's precedent).
- **R6.3 The password is answered once (R1.10).**
  - The ledger records the role (with its reference, never the password). A replay answers `secret_already_issued`, with `role` and `hint`.
  - A call that loses its batch to a concurrent call under the same key also answers `secret_already_issued`: the ledger's answer names the winner's reference, not its own.
  - After `Undetermined`, a call whose write applied finds its own entry and answers its password. If the outcome stays unknown, it answers `unavailable`. A retry under the same key then creates the role (nothing applied) or answers `secret_already_issued` (it applied, and the password is lost: reset it).
- **R6.4 Inheritance.**
  - `CreateBranch` copies the parent's roles, each with a copy of its secret under the child's own reference, and the databases whose owner it copied, all in the batch that creates the branch. The batch checks each parent role's version, so a reset or delete meanwhile makes it copy again.
  - It copies the records as they are now, not as of the branch point: `pg-control` keeps no history of them. The compute spec makes the records authoritative, so the child's compute gets what its records say.
  - A taken branch name is refused before anything is copied.
- **R6.5 Names and preconditions.**
  - A role name is a Postgres identifier (1–63 bytes, no NUL). These are reserved, compared lower-cased:
    - `public` and `none` (Postgres);
    - `cloud_admin`, `neon_superuser`, `databricks_superuser` and `zenith_admin` (the fork's `compute_ctl`);
    - the prefixes `pg_`, `tok_` (§46 §8.6's login roles) and `loams_` (roles Loams manages, such as `loams_ro`).
  - `postgres`, `template0` and `template1` are reserved database names.
  - A database's owner must be a role of the same branch (`failed_precondition`, which registers no metadata, so the role is named in the message).
  - `DeleteRole` refuses a role that owns a database, as `DROP ROLE` would. `CreateDatabase` rewrites its owner's record unchanged, so a create and a delete of the owner conflict either way.
  - A `system` role is neither reset nor deleted through the API.
  - An empty `branch_id` takes the default branch. Changes need the project and the branch `creating` or `ready`; listings work in any state.
- **R6.6 The stores.**
  - `FileSecretStore`:
    - one file holding a format byte and a postcard map, age-encrypted (age 0.11.5, X25519) and replaced atomically (0600 temporary file, fsync, rename, directory fsync);
    - the key is a 0600 file, created on first use, and a key file others can read is refused. With the feature `keyring`, the key can live in an OS keyring entry instead;
    - one process owns the file.
  - `KubeSecretStore` (feature `kubernetes`):
    - one `Opaque` Secret per reference (data key `secret`, labels `app.kubernetes.io/managed-by=loams-pg-control`), written by server-side apply with the field manager `loams-pg-control`;
    - it needs `get`, `patch` and `delete` on `secrets` in its namespace;
    - KMS encryption at rest is the cluster's `EncryptionConfiguration`, which Task 48's chart documents.
  - **kube is pinned at 3.1.0, not 4.2.0** (a deviation from Task 0 ruling 5). kube-client 4.x needs serde-saphyr 0.0.29, which caps `smallvec` below 1.16 and would downgrade it workspace-wide. 3.1.0 (2026-03-17, k8s-openapi 0.27.1) reads kubeconfig with `serde_yaml`, which the lockfile already has. Task 13 moves to 4.x once serde-saphyr lifts the cap.
- **R6.7 Tests.** The role and database cases are in `tests/service/roles.rs`, run by `service_local` and `service_tikv`, rather than in `tests/roles.rs`, so they run on both backends as Task 5's do. The stores' cases are in `tests/secrets.rs` and `tests/secrets_kube.rs` (feature `kubernetes`, against a stand-in API server). `pg2.yml`'s `pg-control` job runs the latter, and clippy with `kubernetes,keyring`.
- **R6.8 For later tasks.**
  - **Task 7.** Removing a branch or a project also removes its `R/` and `D/` records and deletes their secrets. The orphan sweep (R6.1) also belongs here.
  - **Tasks 11 and 24.** The fork's `compute_ctl` creates a spec role as `CREATE ROLE … INHERIT CREATEROLE CREATEDB BYPASSRLS REPLICATION IN ROLE neon_superuser` (`compute_tools/src/spec_apply.rs:819`). Only a role in `jwks_roles` is created as a plain role. Least privilege for API roles, and `loams_ro`'s attribute list (Task 0 ruling 8), are decided there.
  - **Task 24.** `reset_password_rotates_and_invalidates_old` checks the store's side. That the old password stops working at the compute and at PgDog is Task 24's check.
  - **Task 9.** Map `RoleRec` to `Role` (`system`, `create_time`). Wire `PgService::new`'s `SecretStore`: the file store in single-node mode, the Kubernetes store under `--runtime kubernetes`.
  - **Task 47.** Limits on roles and databases per branch.

### Task 7 rulings (2026-10-10)

- **R7.1 The reconciler's shape.**
  - `Reconciler::new(store, neon, secrets, ReconcilerConfig)` and `Reconciler::run(&self, shutdown)`, with `kicker()` (`Kick::Project(id)`), `reconcile_project(namespace, id)` (one pass; the run loop's and the tests') and `sweep_secrets()`. It is generic over `S: PgControlStore` and `N: NeonWrite` (R3.10).
  - It takes the `SecretStore` (removing a branch deletes its roles' secrets; the sweep) and no `runtime`: `ComputeRuntime` is Task 10's, and endpoints are reconciled in Task 11. Until then a branch or project delete **waits** until the endpoints on it are gone. Task 11's endpoint reconciler removes the endpoints of a `deleting` branch or project (the cascade); its text should say so.
- **R7.2 Triggers and scheduling.**
  - Watches on `x/` (projects; the name index is skipped), `X/` (branches, name index, guards) and `E/` (endpoints) mark a project; so do the resync (30 s) and kicks. A kick for a project the watch has not reported is dropped (the watch reports it).
  - One pass per project at a time, at most 8 at once. A failed pass is retried after 1 s, doubling to 30 s; a pass that waits on something outside (endpoints, a parent, a timeline being deleted) comes back after 1 s. `Held` (another instance's lease) is retried at the resync, so a dead holder's projects move within `lease_ttl` plus one resync.
  - The run loop's `namespace` comes from the `x/<ns>/<project_id>` key. It is used only to build `ProjectKey`, so CP-R7's re-key (`NamespaceId` in place of the name) changes no reconciler code. Task 7 adds no record keyed by a name.
- **R7.3 The lease.** TTL 60 s (configurable). A pass reads without the lease, and takes it only when there is something to do, then reads again under it. The held fence is renewed (same epoch) before every Neon call and every write; it is acquired again only after `LeaseLost` (a new epoch, R3.12). A write that a lapse beat to it fails `Fenced` at the store. The test seam `ReconcilerConfig::before_write` runs between the renewal and the write.
- **R7.4 `PgControlStore::commit(batch, fence)`: the fenced batch.** As `ApiWriter::commit`, inside one transaction that first checks and locks the lease (`Fenced`, with no index, when it moved). Each batch operation now carries its record's project, and a stored value's project is decoded inside the transaction, so a fenced batch, like a fenced write, cannot write, delete or check another project's record (`InvalidArgument` at the operation's index, R3.11). Conformance case: `fenced_batch_is_fenced_and_scoped`.
- **R7.5 Neon steps.**
  - Project: `attach_tenant` with `pitr_interval` (generation 1 against a pageserver; the storage controller picks its own, R2.14), then `tenant_config` with it again (the controller's create answers 409 for an attached tenant), then `main`'s timeline, then one batch (project and `main` `ready`, operation succeeded).
  - Timeline: the pageserver's create (bootstrap at the project's major, or a branch of the parent's timeline at `ancestor_lsn`, or its head when none is recorded); a 409 is done, and the timeline is read instead. Then `loams-wal`'s create with `start_lsn` = the pageserver's `last_record_lsn`, as Neon's control plane does; a 409 or an existing timeline is done (R2.10).
  - Delete: `not_found` is done; `aborted` (a deletion in progress) and `branch_has_children` wait. The pageserver deletes in the background, so the records go only once a read of the timeline answers `not_found`.
  - A refusal is **transient** (`storage_unavailable`, `unavailable`, `aborted`, `internal`: retried with backoff) or **final** (any other reason: the operation fails with it). An operation's error message names the step and the reason, never the component's text (it can name hosts).
- **R7.6 Operations and progress.**
  - `OperationRec` gains `progress: Option<OperationProgress { done, total, unit, phase }>` (`loams.operations.v1.Progress`), `serde(default)`. `FORMAT` stays 1, by R5.11's precedent (no operation exists outside tests).
  - Steps: project create 3 (`attaching_tenant`, `creating_timeline`, `done`); branch create 2 (`creating_timeline`, `done`); branch delete 3 (`waiting_for_endpoints` or `waiting_for_children`, `deleting_timeline`, `removing_records`, `done`); project delete one per branch plus one (`waiting_for_endpoints`, `deleting_branches`, `done`).
  - An operation is `Running` from its first step, and ends in the batch that finishes its work. The open create operation of a branch or project removed before it was ready ends `Failed` with `aborted`.
  - R5.10's "stamps `updated_at_ms`": the batch stamps every record it writes (the operations, the parent's guard has no such field). The parent's `BranchRec` is not written (R5.10).
- **R7.7 Final refusals.**
  - Project create: the project, its `creating` branches and their operations fail in one batch; `DeleteProject` takes a failed project.
  - Branch create: the branch and its operation fail.
  - Branch delete: the branch fails, its guard's `deleting` is cleared and the operation fails, so `DeleteBranch` can be asked again.
  - During a project delete, a final refusal of a timeline delete is retried instead (the project is going regardless).
- **R7.8 The sweep (R6.1).**
  - `SecretStore` gains `list() -> Vec<SecretRef>` (a change to the shared contract's trait). `FileSecretStore` lists its map; `KubeSecretStore` lists metadata only (`list_metadata`), selected by its two labels, page by page (500). **The Kubernetes store's Role needs `list` on `secrets`**, which Task 48's chart grants.
  - The sweep runs from the resync every 10 minutes. It deletes a role reference (`pg-role-<project>-<ulid>`) older than 15 minutes (the second ULID's time) that no role of its project's branches names; a project that is gone has no branches. Other names are never touched. It needs no lease: a record only names a reference made for its own call. *(PR #395 review.)* The service enforces how soon: `apply` refuses (`unavailable`, nothing committed, the secrets deleted again) a batch that names a reference issued more than `ISSUE_WINDOW` (5 minutes) ago, and the sweep's grace is never below `MIN_SECRET_GRACE` (10 minutes, twice the window). So an old unnamed reference is never named later. A delete the credential store refuses does not stop the sweep, which then answers the first such error.
- **R7.9 A changed retention.** The reconciler keeps, in memory, the `pitr_interval` it last set per project, and sets it again when `history_retention` differs. So `UpdateProject`'s retention reaches the tenant, and each instance sets it once per project after a restart.
- **R7.10 Gaps for later tasks (owner to place).**
  - **No tenant delete.** `loams-postgres` has no tenant delete (or detach), so a deleted project's tenant stays attached, with its remote data. Adding it needs a fork fixture (Global Constraints). Candidates: Task 44 (PITR and protection) or Task 52 (`kill_pg_control_during_100_creates_leaves_no_orphans`).
  - **No `loams-wal` timeline delete.** `WalClient` has none (R2.11), so a removed branch's WAL stays on `loams-wal`. Same placement.
  - **Operations pile up.** Each pass under the lease lists the project's operations, which nothing collects until Task 9 (R5.13).
  - **A child's roles are copied as of now** (R6.4), not as of the branch point; unchanged, and now documented where the copy is made (`service::branches`, `PgService::inherit`).
- **R7.11 Tests.** `tests/reconcile.rs` (local store) and `tests/reconcile_tikv.rs` (feature `tikv`; CI's TiKV suites job runs it) share `tests/reconcile/cases.rs`. The named cases are there, plus `project_delete_removes_every_record`, `neon_refusals_fail_or_retry`, `failed_project_create_can_be_deleted`, `retention_change_reaches_the_tenant` and `resync_sweeps_orphan_secrets`. The test fake's `NeonWrite` keeps tenants and timelines and answers 409 for an existing timeline.

### Task 8 rulings (2026-10-11)

- **R8.1 Files.** `src/upcall.rs` holds the server, its config and the two handlers. Its parts sit beside it in `src/upcall/`, one job each: `keys.rs` (the key and the tokens), `spec.rs` (the spec from the records), `tenants.rs` (tenant id → project) and `limit.rs` (per-peer buckets). The Neon shapes are in `loams-postgres`, with fixtures: `ControlPlaneConfigResponse` and `ControlPlaneComputeStatus` (`compute_ctl.rs`), and `NotifyAttachRequest` (`storcon.rs`).
- **R8.2 One Ed25519 key, two token audiences.** `ComputeKeys` holds `pg-control`'s key. The `kid` is `pgc-` plus 8 bytes of the public key's SHA-256. It signs JWTs with the claims `{compute_id, aud, iat, exp}`:
  - **upcall token**: `aud` `loams-pg-control`, lifetime `COMPUTE_TOKEN_TTL` (90 days). It is the compute's own (`NEON_CONTROL_PLANE_TOKEN`). The compute keeps it, and `compute_ctl` reads its spec again whenever it reconfigures itself, so the token must outlive the compute.
  - **`compute_ctl` token**: `aud` `compute`, lifetime `CTL_TOKEN_TTL` (10 minutes). `pg-control` uses it for its own calls to that compute. `compute_ctl` checks only its `compute_id`, against the JWKS in `compute_ctl_config`.

  The upcall refuses any token that is not an upcall token. A compute record's `jwt_key_id` must equal the token's `kid`. `trust(older)` keeps an older key trusted, and published in the JWKS, during a rotation. `jsonwebtoken` =9.3.1 and `ring` =0.17.14 were already in the lockfile, so no package is new.
- **R8.3 `ComputeRec` changes** (`FORMAT` stays 1; no compute record exists outside tests, R5.11's precedent):
  - `project_id` and `endpoint_id` become `binding: Option<ComputeBinding { namespace, project_id, endpoint_id }>`. `None` is an unbound warm-pool compute, whose spec answer is `empty`. `namespace` is the project key's, so the upcall can read the project. `Record::project()` is the binding's project, and an unbound compute names none.
  - `ctl_url` is new: `compute_ctl`'s external API, where `/configure` is pushed. The runtime reports it (Task 10).
- **R8.4 The spec upcall.**
  - Status codes:
    - **401**: a missing or invalid token, an unknown `kid`, an expired token (60 s leeway, checked against the listener's clock), the wrong audience, or a `kid` other than the record's.
    - **403**: the token is for another compute.
    - **404**: unknown compute.
    - **409**: the compute is stopped or failed, is not its endpoint's current compute, or its endpoint, branch or project is gone. A replaced compute must not run as a second primary.
    - **503**: the project or branch is not ready, the pageserver or `loams-wal` pool has no configured address, the store is unavailable, or the peer is over its rate. `compute_ctl` retries only 502 and 503.
  - The spec is a pure function of the project, endpoint, branch, role and database records. So the same records give the same bytes (§28 §5.3).
  - The spec's inputs:
    - **Pageserver.** It comes from the branch's `notify-attach` shard map. A branch with no map uses `default_pageserver` (single node). Node URLs come from `UpcallConfig::pageservers`. A sharded map is refused until Task 49 (R2.14).
    - **Safekeepers.** `safekeeper_connstrings` come from `wal_pools[project.wal.pool]`.
    - **Endpoint.** It gives the mode (read-write is `Primary`, read-only is `Replica`), the settings (its `pg_settings`) and the suspend timeout (0 becomes -1).
    - **Not set yet.** Roles have no `encrypted_password`, which is Task 24's wiring of SCRAM verifiers. There is no `storage_auth_token` (Tasks 38 and 53) and no `neon.max_cluster_size` (Task 47).
- **R8.5 `notify-attach`.**
  - **Auth.** It is a static bearer token, the controller's `--control-plane-jwt-token`, compared in constant time over SHA-256 digests. mTLS is not built; that belongs with the TLS of Tasks 38 and 53. Without a configured token the route is not served (single node).
  - **Finding the project.** The tenant id is a one-way hash, so a watch on `x/` keys a map from tenant id to project key. It adds no record (CP1). Until the first scan, the answer is 503. An unknown tenant is looked up once more after two polls, then answered 404.
  - **Recording.** One API batch writes the new `shards` and `stripe_size` to every branch, compare-and-set, redone on conflict.
  - **Pushing.** `/configure` goes to every `Running` compute that is its endpoint's current compute, through `ComputeConfigurer` (`ComputeCtlConfigurer` in production). At most 16 run at once, within `apply_timeout` (8 s; the controller's request timeout is 10 s).
  - **Answers.** 200 means recorded and applied. 423 means recorded but not applied: the controller keeps `applied: false` and sends it again. 429 means too many notifications at once (32, the controller's own concurrency), or too many refused credentials from the peer. 400 is a malformed shard map, and 413 a body over 64 KiB.
- **R8.6 Bounds.** Requests are bounded per connection, per peer and globally:
  - **Connections.** At most 256 at once; one more is closed. Headers must arrive within 10 s (hyper's `header_read_timeout`).
  - **Requests.** `Authorization` is at most 4 KiB. A body is read only after authentication.
  - **Per peer IP** (a map of at most 4,096 peers):
    - spec requests: 5/s, burst 20; over it is 503;
    - refused credentials: 1/s, burst 10; over it is 429.
  - **Timeouts.** A spec request, or a notification's store work, has 5 s.
  - **Logs.** No token, header or spec is logged. `upcall_logs_no_token` checks this.
- **R8.7 `upcall_not_served_on_api_listener`.** The router is private (a `compile_fail` doctest), so `UpcallServer` is the only way to serve it. `bind` refuses a `listen` that overlaps `api_listen`: the same port with the same IP, or a wildcard IP. **Task 9 adds the HTTP half** to `crates/loams/tests/pg_control.rs`: the API listener answers 404 for `/compute/api/v2/computes/{id}/spec` and `/notify-attach`.
- **R8.8 Checked against the pinned `compute_ctl`.** The ignored `it_compute_ctl_reads_the_spec_upcall` (Podman, host network, `compute-node-v17@sha256:13ab146d…`) passed on 2026-10-11:
  - `compute_ctl` read the `empty` answer through the upcall with its token, and then answered `/status` = `empty`;
  - it accepted `pg-control`'s `compute_ctl` token against the JWKS, and refused another key's token;
  - it accepted the attached spec at `/configure`. No storage runs in the test, so the start itself never finished.

  Fixtures: `spec_upcall_empty.response.json` (verified this way), and `src_notify_attach*.request.json` from the fork's source, because no storage controller runs in `deploy/loams-postgres-dev`.
- **R8.9 For later tasks.**
  - **Task 9:**
    - wire `UpcallServer` with `--pg-upcall-listen` (default `DEFAULT_UPCALL_LISTEN`, `127.0.0.1:7690`) and the API address as `api_listen`;
    - keep the signing key across restarts, shared by every `pg-control` instance: a Secret, or the file store in single-node mode. Two instances must never each generate a key;
    - pass the pageserver and pool addresses and the controller's token.
  - **Tasks 10 and 11:**
    - starting a compute writes its `C/` record with `binding`, `jwt_key_id = keys.kid()` and `ctl_url`, and hands the compute its upcall token (`issue_upcall_token`, `COMPUTE_TOKEN_TTL`) and `--control-plane-uri`;
    - a compute older than the token TTL is replaced;
    - the endpoint reconciler re-pushes a spec when its inputs change, because a compute that read its spec just before a `notify-attach` committed still has the old pageserver;
    - endpoint `pg_settings` need an allow-list, because the builder lets a caller's setting replace `shared_preload_libraries` or `synchronous_standby_names`.
  - **Task 14:** an unbound compute's record has `binding: None`, and binding it is a write under the project's lease.
  - **Task 24:** role verifiers in the spec.
  - **Task 46:** `loams_pg_upcall_requests_total{route,code}`.
  - **Task 48:**
    - the NetworkPolicy lets only computes and the storage controller reach the upcall port;
    - the controller's token and the signing key live in Secrets.
  - **Task 53:**
    - TLS (or mTLS for the controller) on the upcall listener;
    - a tenant-scoped `storage_auth_token`.

### Task 9 rulings (2026-10-11)

- **R9.1 The role is its own process and listener.** `loams pg-control` (feature `postgres`) serves `loams.postgres.v1.PostgresService` and `loams.operations.v1.OperationsService`'s `GetOperation` and `ListOperations` (its operations only; `WatchOperations` and `CancelOperation` answer `not_implemented`) on `--listen` (default `127.0.0.1:7680`), with `grpc.health.v1` and, with `--reflection`, `grpc.reflection.v1`. The upcalls are on `--pg-upcall-listen` (`127.0.0.1:7690`), bound with the API's bound address as `api_listen`. The engine's main port (`loams dev`, `standalone`, `cluster`) does not serve the package: Task 56 runs the role inside `loams dev --postgres`, and Task 57 advertises it in `GetInstance`. *Deviation from the file list:* the mount is `crates/loams/src/pg_control.rs` (the role) and `src/api/connect_postgres.rs` (the handlers, as Task 0 ruling 2 named it), with `src/api/postgres_messages.rs` (record ↔ proto) and `src/pg_control_cli.rs` (flags); `server.rs` is unchanged. The other flags: `--store local|tikv` (`--meta tikv://…` for TiKV, needing the `tikv` feature too), `--runtime compose|kubernetes` (until Task 10 it picks the secret store; `--kube-namespace`, feature `postgres-kubernetes`), `--data-dir`, `--pageserver`, `--storage-controller`, `--wal`, `--pageserver-pg <id>=<libpq URL>`, `--default-pageserver`, `--wal-pool <pool>=<connstr>,…`, `--storage-controller-token-file`. The RPCs later tasks build answer `unimplemented`/`not_implemented` after authentication and authorization, naming the task: endpoints (Task 11), `RestoreBranch` (44), `UpgradeProject` (51), `GetConnectionInfo` (20 and 25), `IssueConnectCredential` (25). **`WatchProject` has no owning task in this plan**; the owner should place it (Task 11 is the natural one). *(Placed in Task 11: R11.7.)*
- **R9.2 Authentication and authorization before MT1 (Task 0 ruling 4, API2-Q1).** Two seams in `src/authz.rs`, each a trait MT1 replaces without touching a handler: `Authenticator` (headers → `Principal { subject, kind, act, credential }`) and `Authorizer` (OpenFGA's `Check`). `--pg-authz`:
  - `dev` (default): `DevLoopbackAuthenticator` and `DevAuthorizer`; refuses to start unless `--listen` is loopback.
  - `api-key` (`--pg-api-keys <file>`): `ApiKeyAuthenticator` (`authorization: Bearer lk_<ULID>_<32 base62>`, API2's format) and `GrantsAuthorizer` over the same JSON keys file, which holds per key its id, the **SHA-256** of the key (deviation from API2's argon2id: a key carries 190 random bits, so a slow hash adds nothing, and it is compared in constant time), the subject, its kind, its `act` chain and its grants (`{"relation": "editor", "namespace": "acme"}` or `{"project": "prj-…"}`). `loams pg-api-key --subject … --kind … [--act agent:bot] --grant editor:namespace:acme` prints the key once with the entry to add. The file is read at start.
  - `openfga`: refused at start until MT1's model exists.
  - The relations are §46 §4.1's on `pg_project:<id>`, inherited from the namespace; `admin` ⊇ `editor` ⊇ `viewer`, `editor` ⊇ `connect`, and `viewer` does not imply `connect` (R1.9). `authz::required` is the table of the relation each RPC needs, on its namespace (`CreateProject` editor, `ListProjects` viewer) or its project.
  - **Agents never hold `admin`:** `authorize` refuses `admin` to a principal that is an agent or acts through one (its `act` chain) whatever the authorizer says, and a keys file granting an agent `admin` does not load. `Caller::admin` is that check on the project.
  - Agents' approvals (§46 §4.1, R5.8): `loams.approvals.v1` is served by no Loams binary yet, so an agent's delete of a protected branch, or lifting of a protection, is refused (`permission_denied`, or `branch_protected` for a delete) rather than parked `AWAITING_APPROVAL`. The approval flow waits for the approvals service.
  - The ledger's principal is `<kind>:<subject>`.
- **R9.3 Audit (D221, §41).** `src/audit.rs`: every mutation, applied or refused, and every refusal before the service (unauthenticated, permission denied), is one `AuditEvent` naming the principal, its kind, the `act` chain, the API key's id, the RPC, namespace, project, the ids and names the request gave, the idempotency key, the operation and the outcome (a reason, never a message). It never holds a request or answer body. `LogAuditSink` writes each as one JSON line on the `loams_audit` log target, which the OTel log pipeline carries (D221); the audit query API and a Loams stream as its store are later work. Successful reads are not audited.
- **R9.4 No console REST.** API2 (D602) removes application REST, so the role serves the RPCs only; `docs/api/route-map.md`'s rows stay planned (R1.2 is amended accordingly).
- **R9.5 Reflection lists what a process answers (R1.4).** `api::connect::install_reflection` advertises the services with a route on the router (served, or stubs that refuse with a typed reason, as GR1's `reflection_lists_only_served_or_stubbed_services` requires), the House layer's, health and reflection. A package only in the descriptor set (`loams.operations.v1` and `loams.postgres.v1` on the engine's port) is never listed. The plan's "the catalogue's available services" is read as "the services the catalogue's rows answer", since unavailable rows are stubbed.
- **R9.6 One signing key (R8.9).** `SecretStore` gains `create` (store only if absent; the file store under its lock, the Kubernetes store by a plain create that answers 409), a change to the shared contract's trait. `load_or_create_signing_key` reads `pg-control-signing-key` from the secret store, and when it is absent stores a new key with `create` and reads back whatever is stored, so concurrent first starts end on one key. A stored key that does not parse, or a store that does not answer, fails the start; nothing signs with a key the store does not hold. The Kubernetes store's ServiceAccount needs `create` on `secrets` (Task 48).
- **R9.7 Upkeep.** Every 10 minutes the role prunes the idempotency ledger and removes operations that finished more than `OPERATION_RETENTION` (7 days) ago, with their indexes (`PgService::collect_operations`; R5.13). Task 46's runbook owns the retention's value.
- **R9.8 The no-metering guard covers PG2** (the guard was removed 2026-10-15). It also read, under `crates/loams-pg-control/` and `deploy/helm/loams-postgres/`, every name in code and strings (comments removed) and fails on a part that is `plan`, `price`, `invoice`, `credit`, `meter`, `billable` or `usage` (or their plurals). `FaultPlan` (`loams-kv`'s) is allowed by name. `no_metering_guard_covers_pg` was in the guard's self-tests.
- **R9.9 The end-to-end needs `loams-wal`.** `e2e_create_project_and_branch` passed on 2026-10-11 against `deploy/loams-postgres-dev`'s pinned pageserver and a `loams-wal --store mem`. The stack's stock `safekeeper1` refuses `loams-wal`'s timeline create (it requires `mconf`), as R2.11 says; `pg2-e2e.yml`'s `pg-control-deploy-neon` builds and runs `loams-wal` for it.

### Task 10 rulings (2026-10-11)

- **R10.1 The seam.** `src/runtime/mod.rs` holds `ComputeRuntime` and its types:
  - `BackendAddr { host, port }` (`E/`'s `backend_addr` as `host:port`), `ComputeLaunch { compute_id, binding, address, image, cu, upcall_token }`, `ComputeHandle { compute_id, ctl_url, address }`, `RuntimeStatus` (`Missing`, `Pending`, `Running`, `Exited { code }`: the container's state, not `compute_ctl`'s), `ResizeOutcome` (`Applied`, `Refused(reason)`, `Unsupported`) and `RuntimeError` (`InvalidArgument`, `PortsExhausted`, `Unavailable`, `Timeout`; never a token).
  - **The trait gains `remove_address(ep)`**, a change to the shared contract: Task 11's `delete_endpoint_stops_and_removes_address` needs a way to give the address up (the Kubernetes runtime deletes the Service, Task 13).
  - The constants every runtime shares: Postgres on `COMPUTE_PG_PORT` (55433, as `deploy/loams-postgres-dev`'s computes), `compute_ctl` on 3080 (external) and 3081 (internal), and `COMMAND_TIMEOUT` (5 minutes).
  - `is_pinned(image)`: `<name>@sha256:<64 lower-case hex>`. Runtimes refuse any other image (Global Constraints).
- **R10.2 The compose files (deviation: the file list named only `<ns>.yaml`).** Under `<data>/pg/compose` (`ComposeConfig::project_dir`), directories 0700 and files 0600, written atomically:
  - `<ns>.yaml`, the compose file of project `loams-pg-<ns>`, rendered deterministically from `state/<ns>.json` (the namespace's computes in key order). The state is written first, and `ComposeRuntime::open` renders every compose file again, so a crash between the two never leaves the file ahead of the state. A namespace whose last compute stops loses both files.
  - `state/ports.json`, each endpoint's host port (R10.3).
  - `env/<service>.env`, the compute's `NEON_CONTROL_PLANE_TOKEN`: the only place the upcall token is written. It is not in the compose file, a state file or a command line.
  - A namespace must be a DNS label to name a project and files (the Kubernetes runtime's `loams-pg-<ns>` needs the same); anything else is `InvalidArgument`.
- **R10.3 One service per compute.** The service is the compute id in lower case.
  - It runs `compute_ctl` with `--pgdata /var/db/postgres/compute`, `--pgbin`, `--connstr postgresql://cloud_admin@127.0.0.1:55433/postgres`, `--compute-id`, `--control-plane-uri` (`ComposeConfig::control_plane_uri`) and the two HTTP ports, as Task 8's `it_compute_ctl_reads_the_spec_upcall` ran it.
  - `restart: "no"`: a compute is replaced, never restarted (§46 §7.1).
  - Labels `loams.dev/{compute,endpoint,namespace,project}`. `cpus` and `mem_limit` from the CU (0.25 vCPU and 1 GiB per quarter).
  - It joins `ComposeConfig::network`, the storage stack's network as an external network (`loams-neon_default` for `deploy/loams-postgres-dev`, `loams_default` for the preview release, which includes it), with the alias `ep-<endpoint ulid>` in lower case; and `extra_hosts` (`host.docker.internal:host-gateway` under Docker when the upcall listener is on the host).
  - Commands are argument arrays with `-f <ns>.yaml -p loams-pg-<ns>`: `up -d --no-build`, `rm -s -f`, `ps -a --format json` (both the line and the array shapes) and `port <service> 3080`, each killed after 5 minutes. Commands of one namespace run one at a time.
  - `ComposeBin::detect` takes `docker compose`, then `podman compose`, then `docker-compose`, as AP1e Task 21. `podman compose` delegating to `docker-compose` was tested; the Python `podman-compose` provider was not, and its `ps --format json` may differ.
- **R10.4 Addresses on compose.**
  - `ensure_address` gives each endpoint the lowest free port of `55500..56000`, and keeps it in `state/ports.json` across calls and restarts. An endpoint whose record already names a port (`E/`, which the reconciler writes) keeps it, unless the port is outside the range or another endpoint's (`InvalidArgument`). `remove_address` frees the port, and refuses while one of the endpoint's computes still runs.
  - The compute publishes Postgres on that port at `ComposeConfig::publish_ip` (loopback by default). Compose fixes a container's ports when it starts, so a compute is bound at `start`. `start` refuses an address that is not the endpoint's, and **removes the endpoint's previous compute first**: two computes cannot publish one port, and a replaced read-write compute must not keep running. `bind_address` only checks that the compute was started with the endpoint's address.
  - The compose runtime starts bound computes only. The warm pool on compose (unbound computes have no port to publish) is Task 14's to decide.
  - `resize` is `Unsupported`: changing a container's limits recreates it, which is a restart.
- **R10.5 Reaching `compute_ctl` (`CtlReach`).** `Host` (the default; single-node mode and the desktop, where `pg-control` runs on the host): the container publishes 3080 on an ephemeral loopback port, and `compose port` names it, so `ctl_url` is `http://127.0.0.1:<port>` for the container's life. `Network` (`pg-control` itself on the compose network, as the preview release's `loams` container would be): `http://<service>:3080`, nothing published.
- **R10.6 The spec listens where the runtime publishes (a change to Task 8's spec).** The upcall's spec sets `listen_addresses = '*'` and `port = 55433` after the endpoint's settings, so an endpoint cannot move Postgres off the port its address points at. Neon's own control plane sets both (`control_plane/src/endpoint.rs`); without them Postgres listened on the container's loopback only. Test: `spec_listens_on_the_compute_port` (`tests/upcall.rs`). Task 11's allow-list for the other settings is unchanged.
- **R10.7 Compute certificates (`src/runtime/certs.rs`).**
  - The CA: ECDSA P-256, valid 10 years, stored in the `SecretStore` under `pg-compute-ca` (certificate and key, JSON) and shared by every instance as the signing key is (R9.6): `load_or_create_ca` reads it, and `create`s one only when absent. A stored CA that does not parse fails, as the signing key does.
  - A compute certificate: ECDSA P-256, signed by the CA, the given DNS names or IPs as SANs, `serverAuth`, not a CA, `notBefore` 5 minutes back, valid 30 days (`CERT_LIFETIME`), renewal due 15 days after issue (`RENEW_AFTER`, `needs_renewal`).
  - `rcgen` 0.14.10 (`crypto`, `pem`, `ring`) was already in the lockfile through `loams-sqlgate`; `x509-parser` 0.18.1 (`verify`) is a dev-dependency, already in the lockfile through `rcgen`. No new package.
  - Nothing mounts the certificate into a compute yet: **Task 23** (TLS on both hops) wires it into the compose service and the compute's `ssl` settings, and Task 13 into the Pod's Secret volume. Renewing a running compute's certificate is Task 23's too.
- **R10.8 What Task 11 wires.** The `loams pg-control` binary does not build a runtime yet: the endpoint reconciler (Task 11) is the first caller, and it brings the flags (the compose network, the control-plane URI as computes see it, `CtlReach`, the compute image per Postgres major from `deploy/pins/loams-postgres-images.toml`, never a `neon-archive` image). `--runtime compose` still picks only the secret store until then. Task 11 also records the handle's `ctl_url` and the address in `C/` and `E/`.
- **R10.9 The end-to-end test.** `it_compose_compute_answers_select_1` (`tests/it_compose.rs`, ignored; `pg2-e2e.yml`'s `pg-control-compose-runtime`).
  - What it does: it creates the tenant and timeline on `deploy/loams-postgres-dev`'s pageserver and writes ready records. It starts the upcall listener on the host. The runtime then starts the pin file's `compute-v17` on the stack's network. `compute_ctl` reads its spec through the upcall and reaches `running`. A role made inside the compute then runs `SELECT 1` through the endpoint's host port, and `stop` and `remove_address` clean up.
  - The compute's WAL goes to the stack's `safekeeper1`. A `loams-wal` on the host is not reachable from the compose network; moving the dev stack to `loams-wal` is Task 43's.
  - It passed three times on 2026-10-11 (Podman 6.1.1 with `docker-compose` 5.2.0; the storage stack as compose project `loams-pg2-it`, torn down with `down -v`; the computes in the test's own project, `loams-pg-it`), in 4 to 17 s with the images cached. CI runs it under Docker with `host.docker.internal:host-gateway`.
- **R10.10 For later tasks.**
  - **Task 13:** the Kubernetes runtime implements the same trait, `remove_address` included (delete the Service).
  - **Task 14:** the warm pool on compose (R10.4).
  - **Task 22:** on compose, PgDog on the storage network reaches an endpoint at `ep-<endpoint ulid>:55433` (the alias), and from the host at the endpoint's port.
  - **Task 23:** the certificate in the compute (R10.7).
  - **Task 56:** `loams dev --postgres` runs the compose runtime with `<data>/pg/compose`, `deploy/loams-postgres-single`'s network and `CtlReach::Host`. Running `pg-control` inside the preview release's `loams` container needs the compose CLI and the engine's socket in that container, with `CtlReach::Network`.

### Task 11 rulings (2026-10-12)

- **R11.1 Records** (`FORMAT` stays 1: no endpoint, compute or operation record exists outside tests, R5.11's precedent).
  - `EndpointRec` gains `created_at_ms`, `updated_at_ms`, `crashes` (crash times within 10 minutes), `replace_compute` (`RestartEndpoint`), `settings_version` (bumped by every `UpdateEndpoint` that changes `pg_settings` or the suspend timeout) and `open_ops` (the operations the reconciler has not finished, at most 16; one more is `aborted`), all `serde(default)`. `EndpointRec::new` builds one at rest.
  - `OperationRec` gains `endpoint_id`; `OperationKind` gains `EndpointStart`, `EndpointSuspend`, `EndpointRestart`, `EndpointDelete` (appended, so the postcard indexes do not move). An endpoint operation's target is `{project, branch, endpoint}`.
  - `PoolMode` gains `Statement` (the proto's `POOL_MODE_STATEMENT`), appended.
  - `ComputeRec.spec_version` is the version of the spec's inputs (R11.5).
- **R11.2 The service** (`src/service/endpoints.rs`).
  - Sizes: 0.25 to 16 CU in 0.25 steps, `min_cu ≤ max_cu`; unset `min_cu` is 0.25 and unset `max_cu` is `min_cu`. Suspend timeout: unset 300 s, 0 never, otherwise at least 60 s. Pool mode: unset is transaction.
  - One read-write endpoint per branch: a create rewrites the branch's `BranchGuardRec` unchanged, so two creates on one branch conflict (the retry sees the first) and so does a create against `DeleteBranch`; no endpoint is created on a branch or project being deleted. `endpoint_exists_for_branch` carries `endpoint`. A deleting read-write endpoint still counts until it is gone.
  - Start, suspend, restart and delete set `desired` (restart also `replace_compute` when a compute exists or is starting; delete sets state `deleting`) and append an operation to `open_ops` in one batch. Start and restart clear a `failed` endpoint's failure and crash history (`start_clears_failed`).
  - `RestartEndpoint{promote: true}` answers `not_implemented` (Task 16).
  - `ListEndpoints` with a `branch_id` filters within each page (a page may hold fewer than `page_size`).
- **R11.3 The `pg_settings` allow-list (Task 8 carry).** `ALLOWED_PG_SETTINGS`: session and planner settings, timeouts, logging, autovacuum tuning, locale formats. Never: `shared_preload_libraries`, `synchronous_standby_names`, `listen_addresses`, `port`, any `neon.*`, `wal_level` and the replication lags, `max_connections`, `shared_buffers` and other CU-tied sizes, `restart_after_crash`, `password_encryption`, anything that loads code or reads files. Names are exact and lower case; values are 1–256 bytes of printable ASCII without `'` or `\`, so they cannot leave their line in `postgresql.conf`. The API refuses others (`invalid_argument`, field `pg_settings`), and the spec builder leaves out any that a record still holds.
- **R11.4 The endpoint reconciler** (`src/reconcile/endpoint.rs`). `Reconciler::with_computes(Computes)` (runtime, a `ComputeCtl` seam over `compute_ctl`, the signing key, the image per major, the spec sources); without it endpoints are not reconciled and deletes wait (Task 7's behaviour). Each pass reconciles the project's endpoints first, then the project and branches.
  - **Start** (once project and branch are ready): `ensure_address` (recorded in `E/`), one batch writing `C/` (bound, `Pending`, the key's `kid`, the inputs' version) and naming it on the endpoint, `runtime.start` with a fresh upcall token (`COMPUTE_TOKEN_TTL`), `ctl_url` into `C/`, then polls (`Computes::poll`, 500 ms) until `compute_ctl /status` is running: `bind_address`, compute and endpoint `running`. Not running 60 s after the compute's creation fails the endpoint (the diagram's timeout goes straight to `failed`).
  - **Crashes** (container exited or missing, `compute_ctl` failed or terminated), while starting or running: the compute is stopped and its record removed, and a new compute starts; the third crash within 10 minutes fails the endpoint until `StartEndpoint`.
  - **Suspend**: `suspending` (the compute's record `Stopping`, so the upcall refuses it a spec), `/terminate?mode=immediate`, `runtime.stop`, then `suspended` with no compute and no `C/` record (stopped computes' records are removed, not kept). A start asked meanwhile finishes the suspend and then starts a new compute.
  - **Replacement** (`replace_compute`, or a compute within `REPLACE_MARGIN` = 1 day of its upcall token's expiry; R8.9 read as "replaced before the token expires") stops the compute and starts a new one behind the same address; it is not a crash.
  - **Operations** end in the batch that settles them: start and restart when the endpoint runs (`compute_start_failed` when it fails); suspend once no compute runs; delete with the removal. An operation the endpoint's later `desired` contradicts is `aborted` ("superseded"). Open operations move to `Running` while they wait.
  - **Work without the lease**: an endpoint needs a pass unless it is at rest (suspended and asked to be, or failed and not asked again) with no compute and no open operation. So a project with a running endpoint takes its lease every resync (30 s) to check the compute; Task 12's idle monitor samples the same computes.
- **R11.5 Re-pushing the spec (Task 8 carry).** The spec builder answers the version of the spec's inputs: a SHA-256 of the spec's bytes, the project's and branch's record versions, the endpoint's `settings_version` and each role's and database's name and version (versions are never reused, R3.5, so inputs that change and change back still differ). `C/` records it at creation, before the compute can read anything. Whenever a running compute's current version differs, the reconciler pushes `/configure` and records it. That covers a `notify-attach` landing after a starting compute read its spec, and role, database and setting changes (those reach a running compute within one resync: the `R/` and `D/` keys are not watched).
- **R11.6 Removal and R7.1's cascade (Task 7 carry).** An endpoint `deleting`, or any endpoint of a branch or project being deleted, has its compute terminated and stopped, its address given up (`remove_address`), and its `E/` and `C/` records removed in one batch (the delete operation succeeds, others are `aborted`); the pass then reads the project again, so the branch or project goes in the same or the next pass.
- **R11.7 `WatchProject` (R9.1's unowned RPC).** `src/service/watch.rs`: a snapshot (project, branches, endpoints, the endpoints' current computes), then changes found by reading the records every second and comparing versions (changes between reads collapse, as R3.7's watch), removals after changes (computes, endpoints, branches), a heartbeat after 15 s without an answer. The cursor (`w1.` and 32 hex) is a digest of every `(kind, id, version)` the client holds after that answer. A resume whose cursor names the current state sends no snapshot; any other cursor gets a snapshot with `snapshot_reset`. Nothing is kept on the server. A deleted project ends the stream with `project_not_found`. Each watch reads the project's records once a second; a change feed (D63) can replace this behind the same events.
- **R11.8 The binary (Task 10 carry, R10.8).** `--runtime compose` builds the compose runtime: `--compose-network`, `--compose-dir` (default `<data-dir>/compose`), `--compute-control-plane-uri` (default `http://host.docker.internal:<upcall port>` with `host.docker.internal:host-gateway`, or `http://host.containers.internal:<port>` under Podman), `--compose-extra-host`, `--compose-publish-ip` (127.0.0.1), `--compute-ctl-reach host|network`, `--compute-image <major>=<repo>@sha256:<digest>`. A missing compose binary fails the start. The images come from `deploy/pins/loams-postgres-images.toml`, compiled in (`runtime::images`), each pinned by digest and never a `neon-archive` copy; the pg2 workflows' path filters include the pin file. A loopback upcall listener is warned about: computes in containers cannot reach it. `--runtime kubernetes` records endpoints but starts none until Task 13. `PgControlConfig.computes = None` runs the API alone (the binary's tests).
- **R11.9 Least privilege for API roles (Task 6 carry) is not an endpoint or spec setting: Task 24 and NF1.** The fork's `compute_ctl` (`compute_tools/src/spec_apply.rs:817–828`) creates every spec role it does not find as `CREATE ROLE … INHERIT CREATEROLE CREATEDB BYPASSRLS REPLICATION IN ROLE <privileged_role_name>`; only roles named in `spec.local_proxy_config.jwks[].role_names` are created plain. The spec has no field for role attributes, and naming roles in `local_proxy_config.jwks` would turn on a JWT login path to them, so the spec cannot fix it. The fix is a fork change (a per-role attribute list in `ComputeSpec.cluster.roles`, or a spec flag for plain roles, applied in `spec_apply.rs`'s `CreateAndAlterRoles` phase), with Task 24 putting `loams_ro`'s attribute list (Task 0 ruling 8) and the API roles' least privilege in the spec. Until then every role `CreateRole` makes is a member of `neon_superuser` with those attributes inside its own project's Postgres.
- **R11.10 Tests.** `tests/endpoints.rs` (local) and `tests/endpoints_tikv.rs` (CI's TiKV suites job) share `tests/endpoints/cases.rs`: the seven named cases on a fake runtime and a fake `compute_ctl`, plus the start timeout, crashes outside the window, the spec re-push, the allow-list, sizes, the cascade, token-age replacement, superseded operations, and deleting things. `tests/watch.rs` covers the watch. `tests/it_endpoint.rs` (ignored; `pg2-e2e.yml`'s compose job) runs start, `SELECT 1`, suspend, a second start and delete on `deploy/loams-postgres-dev` with real computes. `crates/loams/tests/pg_control.rs` gains `endpoint_rpcs_round_trip` and `watch_project_streams_a_snapshot`.

### Task 12 rulings (2026-10-12)

- **R12.1 The field (Task 0 left it unrecorded).** `compute_ctl`'s `/status` answers `last_active` (`compute_api/src/responses.rs` `ComputeStatusResponse`, RFC 3339 as chrono's `to_rfc3339` writes it, with an offset rather than `Z`; `null` before any activity). The fork's monitor (`compute_tools/src/monitor.rs`, every 500 ms) moves it to now while any client backend other than `cloud_admin` is not `idle` (a query running, however long; a transaction left open), while a walsender other than walproposer streams, while a logical subscription's worker runs (`pg_stat_subscription.pid` set), and while autovacuum works; otherwise to the latest `idle` backend's `state_change`. It only moves forward. `loams-postgres`'s `ComputeStatusResponse` gains `last_active` and `last_active_ms()`; `it_idle_long_query_is_activity_then_suspends` read it from the pinned `compute-node-v17` during a 15 s query (it moved on every read).
- **R12.2 The idle monitor lives in the endpoint reconciler, under the project's lease.** `IdleMonitor` (`src/idle.rs`) is `Computes::idle`. Each pass over a running endpoint already reads `compute_ctl /status` (Task 11); the same answer and PgDog's clients of the endpoint are its sample, so only the lease holder samples and suspends, and a suspend is the reconciler's own `suspending` → `/terminate` → `stop` path. Idle means: no PgDog client (`cl_active + cl_waiting`) at the sample, and `last_active` unchanged since the last sample. Activity is dated by `last_active`, clamped to the interval since the previous sample (a skewed compute clock moves it by at most one interval). The first sample of a compute on this instance counts as activity, so a restart or a lease move delays a suspend by up to one timeout and never hastens it. The state is in memory (`ComputeRec.last_active_ms` stays unused: writing it per sample would be a write per compute per pass). A sample that could not be taken (no answer from `compute_ctl` or a PgDog replica, an unreadable `last_active`), open operations, a replacement asked, or `suspend_timeout_s = 0` never suspend; an operation the same pass settled (a start or restart of a running endpoint) is activity in that sample (PR #463 review). The idle suspend sets `desired = suspended` and writes no operation; a later `StartEndpoint` or wake (Task 26) starts a new compute. Sampling is at the pass cadence (the 30 s resync); a pass returns `Again` when the suspend falls due before the next resync, so a compute suspends at most one resync after `suspend_timeout_s` of real idleness and no extra pass is scheduled otherwise.
- **R12.3 `PgDogAdmin` (`src/pgdog/mod.rs`) gains `instances(namespace)`**, a change to the shared contract: the monitor (and Task 26's waker) must know a namespace's replicas before reading their pools. `PoolRow` is `{database, user, cl_active, cl_waiting, sv_active}`. `NoPgDog` (no replicas anywhere) is what `pg-control` runs with until PG2c; Task 21/22's admin adapter implements the trait. `pool_serves` maps a pool to its endpoint by the routed name (§46 §8.1): `<project>` (the default branch), `<project>__<branch>` and `<project>__<branch>.<db>` to the branch's read-write endpoint, `<project>__<branch>__ro` to each of its read-only endpoints. Task 18 checks PgDog's `cl_active` counts a client for as long as its query or transaction runs.
- **R12.4 Logical replication blocks the suspend through `compute_ctl`.** No other `compute_ctl` route reports subscribers, and `pg-control` holds no credential for the compute's Postgres; the fork's monitor already holds `last_active` at now while one streams (R12.1). `logical_subscriber_blocks_suspend` fakes exactly that.
- **R12.5 Lifecycle events** (`src/observer.rs`): `ComputeLifecycleObserver` and `ComputeLifecycleEvent` as the shared contract; `Computes::lifecycle` (`Lifecycle { observer, org }`) sends each after the batch that records it commits, so at most once (none is kept: a process dying in between loses it, per D703). `Started { cu }` when a compute's endpoint goes `running`; `Suspended` when a compute is stopped on purpose (suspend asked or idle, a restart's or a token-age replacement's old compute, removal); `Failed { reason }` when it crashed or did not start (the reason names what failed, never the compute's text). Every compute ends with exactly one `Suspended` or `Failed`, which may come without a `Started` for a compute that never ran. `Resized` is Task 15's. `DefaultObserver` counts per kind for Task 46's metrics. `observer::field_names()` reads the event's names off the types (whole-struct destructuring, exhaustive matches), and `observer_event_has_no_billing_fields` held them against the guard's word list (both removed 2026-10-15).
- **R12.6 `org` is one string per deployment, empty by default.** No `pg-control` record names an org (namespaces carry none until MT's tenants repository). The private observer can map the namespace itself; when MT4 lands, `Lifecycle::org` becomes a lookup. Task 13's `loams.dev/org` Pod label has the same gap.
- **R12.7 Tests.** `tests/idle.rs` (local store): the six named cases plus `unanswered_samples_never_suspend` and `every_compute_ends_once`; the fakes and harness of `tests/endpoints/cases.rs` moved to `tests/endpoints/env.rs`, shared by `endpoints`, `endpoints_tikv` and `idle`. Unit tests in `idle.rs` (dating, the first sample, a long query, clients alone, forgetting), `pgdog` (`pool_serves`) and `observer`. `tests/it_endpoint.rs` (ignored; `pg2-e2e.yml`'s compose job) gains `it_idle_long_query_is_activity_then_suspends`; its setup is shared (`stack(ns)`, one compose project per case). No `idle_tikv`: the idle state is in memory, and the suspend's writes are `endpoints_tikv`'s.

### pg-control in the release stack (rulings 2026-10-13, burst brief: "the smallest safe option")

No task of this plan put `pg-control` in the preview release stack: R10.10 noted only that it would need "the compose CLI and the engine's socket in that container", which is why it was held from preview.2. These rulings take the smallest option that gives no container socket to the engine or to `pg-control`.

- **R-REL.1 A compute runner holds the socket; `pg-control` does not.** `loams pg-runner` (feature `postgres`, `src/runtime/runner.rs`) runs the compose runtime of Task 10 and serves `ComputeRuntime` over HTTP: `POST /v1/{ensure-address,remove-address,start,bind-address,stop,status,resize}` with JSON bodies, errors as `{"error": RuntimeError}` (400, 409, 503, 504), `GET /health`. `pg-control --compute-runner <url> --compute-runner-token-file <path>` drives it through `runtime::remote::RemoteRuntime`, so the reconciler is unchanged; with a runner, the compose flags (`--compose-network`, `--compose-dir`, `--compute-control-plane-uri`, `--compose-extra-host`) are refused on `pg-control` and given to the runner. A socket proxy that filters Docker API calls (as `docker-socket-proxy` does) was not taken: it filters by endpoint, so a proxy that lets compose create containers lets anything create a privileged one, and an image check would have to parse every create body of every compose version. The runner's API is the seven typed calls instead, and the compose files are its own rendering.
- **R-REL.2 What the runner lets through (`RunnerPolicy`).** A start: the image exactly one of the runner's list (the pin file's compute image per supported major, compiled in, or the runner's own `--compute-image` overrides; `check_image` refuses unpinned tags and `neon-archive` copies, and an empty list); bound to an endpoint; namespace a DNS label; project and endpoint ids canonical; 0.25 to 16 CU; the upcall token 1 to 8192 bytes of base64url, base64 and dots (`runtime::check_upcall_token`, which `ComposeRuntime::start` also applies: a newline in the token would have set another variable in the compute's env file). An endpoint: canonical project, branch and endpoint ids; the runner rebuilds the record from those, its kind and its address alone. A resize: 0.25 to 16 CU. Nothing refused reaches the runtime.
- **R-REL.3 The runner's token.** 32 random bytes as base64url (`read_or_create_token`): `pg-runner --create-token` writes it (0600, directory 0700) when missing, into a volume `pg-control` mounts read-only (both run as uid 10001). Every call but `/health` needs it as a bearer token, compared as SHA-256 digests in constant time; 401 otherwise. The upcall token crosses the runner's network in plain text; that network has two members and no route out (R-REL.4), and TLS between them waits for Task 23's certificates.
- **R-REL.4 The release stack (`deploy/release/compose.yaml`, `--profile postgres-control`).** `pg-control`: `--store local` in its own volume (one node; the TiKV store stays production's), `--pg-authz api-key` with `secrets/pg-api-keys.json` (bound alone, `create_host_path: false`), API on `127.0.0.1:${PG_CONTROL_PORT:-7680}`, upcalls on the compose network at `pg-control:7690`, the included storage stack's pageserver (node 1234) and `safekeeper1`; networks `default` and `pg-runner`; no socket. `pg-runner`: the socket from `CONTAINER_SOCKET` (rootless Podman's recommended; rootful Docker's documented as root-equivalent) with its group from `CONTAINER_SOCKET_GID`, `cap_drop: [ALL]`, `no-new-privileges`, network `pg-runner` only (`internal: true`), no ports; computes on `PG_COMPUTE_NETWORK` (`loams_default`) as compose projects `loams-pg-<ns>`, endpoints published on `PG_ENDPOINT_BIND` (127.0.0.1), `compute_ctl` reached by service name (`CtlReach::Network`). `release-compose.yml` checks, on the resolved config, that `pg-runner` is the only service mounting a `.sock`, that it is on its internal network alone with nothing published and no privilege, and that `pg-control` is its only neighbour.
- **R-REL.5 The image.** `deploy/release/Containerfile.loams-pg-control` builds `loams` with `postgres,tikv` (the engine image is unchanged) and adds the standalone `docker-compose` v5.5.1 (2026-09-03; v5.6.0 is under 14 days old), checked against the release's SHA-256 per architecture (amd64 `db188918…d576`, arm64 `732e3a84…2ef7`); one image for both roles. `build.sh`, `release-images.yml` and `release-github.yml` build, publish and attest it as `loams-pg-control` from the next release tag; preview.2 ships without it.
- **R-REL.6 Tests.** `tests/runtime_runner.rs` (the HTTP server and client over loopback on a recording fake runtime): `remote_round_trips_every_call`, `remote_errors_keep_their_kind`, `runner_refuses_unlisted_images`, `runner_refuses_a_wrong_or_missing_token`, `runner_refuses_malformed_launches_and_endpoints`, `health_needs_no_token_and_says_nothing_else`, `remote_needs_an_http_url`, `policy_takes_only_runnable_images`, `the_token_file_is_created_once_and_private`; `runtime_compose.rs`'s `bad_launches_are_refused` gains the token newline. `crates/loams/tests/pg_control.rs`: `pg_runner_holds_the_runtime_behind_its_token`, `pg_runner_needs_a_control_plane_uri`, `pg_control_starts_with_a_compute_runner`; the CLI's `a_compute_runner_takes_the_compose_flags` and `the_runner_creates_its_token`. `tests/it_endpoint.rs` with `IT_COMPUTE_RUNNER=1` runs the endpoint lifecycle on real computes through a runner (a `pg2-e2e.yml` step). The images were not built locally (a full release build); the compose file was checked with `docker-compose config` and the jq checks.

### Task 13 rulings (2026-10-13)

- **R13.1 Every runtime call but `start` names the Loams namespace (a change to the shared contract).** `ensure_address(ns, ep)`, `remove_address(ns, ep)`, `bind_address(ns, ep, c)`, `stop(ns, id)`, `status(ns, id)`, `resize(ns, id, cu)`; `start` reads it from the launch's binding. Pods and Services are namespaced and the Role reaches only `loams-pg-*` namespaces, so the Kubernetes runtime cannot find an endpoint's Service or a compute's Pod from its id alone (an `EndpointRec` holds no namespace). The reconciler passes its project's namespace; the compose runtime ignores it (host ports are one range for every namespace, and it keeps each compute's namespace in its own state); the compute runner's wire carries it, and the runner refuses one that is not a DNS label.
- **R13.2 The objects** (`src/runtime/kubernetes.rs`, feature `kubernetes`, rendered by pure functions with goldens in `tests/fixtures/kube/`). In `loams-pg-<ns>` (`namespace_for`; the Loams namespace is a DNS label of at most 54 characters):
  - Pod `cmp-<ulid>` (lower case): `compute_ctl` with the compose runtime's arguments (now one function, `runtime::compute_ctl_args`), `restartPolicy: Never`, labels `app.kubernetes.io/{managed-by,component}` and `loams.dev/{org,namespace,project,endpoint,compute}`. The plan's `argocd.argoproj.io/compare-options: IgnoreExtraneous` is an annotation in Argo CD, not a label, so it is set as one.
  - Its Secret (same name): `token` (`NEON_CONTROL_PLANE_TOKEN` comes from it through `secretKeyRef`, never from the Pod's spec), and with the compute CA `tls.crt`, `tls.key` and `ca.crt` for the endpoint Service's names, mounted read-only at `/etc/loams-compute/tls` (mode 0440, group 1000). Postgres's `ssl` settings stay Task 23's. Written by server-side apply before the Pod, so a retry after a crash between the two replaces the token.
  - Service `ep-<ulid>`: ClusterIP, port 5432 to 55433, selector `app.kubernetes.io/component: postgres-compute` plus `loams.dev/compute`, `none` until `bind_address` sets the compute (a JSON merge patch). The address is `ep-<ulid>.loams-pg-<ns>.svc.<cluster domain>:5432`; a recorded address of another shape is refused.
  - Headless Service `computes` (`publishNotReadyAddresses`): each Pod sets `hostname` and `subdomain`, so `compute_ctl` is `http://cmp-<ulid>.computes.loams-pg-<ns>.svc.<domain>:3080` from the moment the Pod exists. The plan did not say how `pg-control` reaches a Pod's `compute_ctl` before the endpoint's Service points at it; a Pod IP is not known when `start` answers.
- **R13.3 The Pod's security.** Pod: `runAsNonRoot`, uid, gid and fsGroup 1000 (the image's `postgres` is a name, which the kubelet cannot check for `runAsNonRoot`), `seccompProfile: RuntimeDefault`, no ServiceAccount token, no service links, optional `runtimeClassName` (`--kube-runtime-class`). Container: read-only root filesystem with `emptyDir` at `/var/db/postgres` and `/tmp`, `allowPrivilegeEscalation: false`, every capability dropped. Requests equal limits: 250m CPU and 1 GiB per quarter CU (1 CU = 1 vCPU and 4 GiB), with `resizePolicy` `NotRequired` for both.
- **R13.4 Lifecycle.** `start` is idempotent by compute id: it creates nothing when the Pod exists, and treats a 409 as done. `stop` deletes the Pod (grace 10 s, after the reconciler's `/terminate`) and its Secret; a 404 is done. `status`: no Pod is `Missing`; one being deleted, `Succeeded` or `Failed`, or whose container terminated, is `Exited` with the container's code; `Running` once it runs; otherwise `Pending`. `resize` patches the `resize` subresource (strategic merge). 2xx is `Applied` (the kubelet applies it). 404 or 405 is `Unsupported`, which is unambiguous because the Pod was read first. 409 or 422 is `Refused`. A compute with no Pod is `InvalidArgument`. API errors become `InvalidArgument` (400, 422) or `Unavailable`, with the API's code, reason and message, never an object's data.
- **R13.5 RBAC and verification.** The Role (`runtime::kubernetes::role`, golden `role.json`) covers `pods` (get, list, watch, create, delete), `pods/resize` (patch), `services` (get, list, create, patch, delete) and `secrets` (get, create, patch, delete), in the core group only. Task 48's chart ships it with its RoleBinding. The Pod's lockdown was run end to end: compose computes with a read-only root, uid 1000, no capabilities, `no-new-privileges` and empty `/var/db/postgres` and `/tmp` (as tmpfs, mode 0777, like `emptyDir`) passed `it_compose_compute_answers_select_1` on `compute-node-v17` (a local experiment, not committed).
- **R13.6 The binary.** `--runtime kubernetes` (feature `postgres-kubernetes`) now runs computes. It requires `--compute-control-plane-uri` (the upcall Service as Pods reach it) and takes `--kube-runtime-class` and `--kube-cluster-domain`. It loads or creates the compute CA in the Kubernetes secret store (R10.7) and uses the in-cluster ServiceAccount or `KUBECONFIG`.
- **R13.7 Tests.** `tests/runtime_kube.rs` covers:
  - goldens: `pod_golden`, `service_golden` and `rbac_allows_only_pods_services_secrets`;
  - against a stand-in API server: `start_is_idempotent_by_compute_id`, `bad_launches_are_refused`, `bind_moves_selector`, `resize_unsupported_maps_to_outcome`, `status_stop_and_remove`, `certificate_mounted_with_a_ca` and `secret_holds_the_token`.

  **Deviation:** `tests/it_kind.rs` is `it_kind_start_bind_and_stop`, not `it_kind_start_select_1_and_stop`. No storage runs in kind, so the compute gets no spec and `SELECT 1` through the Service waits for Task 48's chart. It checks that the kubelet admits and starts the Pod as rendered, that `bind_address` moves the selector, and the cleanup. It runs in `pg2-e2e.yml`'s new kind job. It was not run on this host: rootless kind needs systemd `Delegate=yes`.

### Task 14 rulings (2026-10-14)

- **R14.1 The key is per Loams namespace.** `PoolKey { namespace, pg_version, image }` (`model.rs`). The plan's key was (region, `pg_version`, image digest). A Kubernetes compute Pod lives in `loams-pg-<ns>` (§46 §15.1), and only that namespace's endpoint Services can select it, so a warm Pod serves one namespace. The region is the deployment's: one `pg-control` serves one region. A namespace's pool is kept while one of its projects has an endpoint. Every pass over such a project (`WarmPool::want`, before the lease) keeps its (namespace, major) wanted for `want_ttl`, 10 minutes. Every instance passes over every project at each resync, so each instance knows the same keys. An instance drains nothing for want of demand until it has been up for `want_ttl`, so a restart never empties the pools.
- **R14.2 Compose has no warm pool.** Compose fixes a container's published ports when it starts, and an unbound compute has no endpoint port to publish (R10.4).
  - The compose runtime, and the compute runner that wraps it, still refuse an unbound start.
  - `--warm-pool-size` defaults to 2 on Kubernetes. With `--runtime compose`, any size but 0 is refused, both on the CLI and in `PgControlConfig`.
  - Kubernetes keeps a pool of size 0 too, so the computes left from a larger size drain.
- **R14.3 Records.**
  - `ComputeRec` gains `pool: Option<PoolKey>` (the pool it was started for, kept once bound) and `bound_at_ms`.
  - `FORMAT` stays 1. No release has shipped endpoints: preview.1 predates Task 11, and the `loams-pg-control` image first builds at the next tag. This follows the precedent of R5.11, R8.3 and R11.1.
  - A warm compute's record is written unbound before the runtime starts it, so a crash never leaves a compute without a record. Its status is `Pending` until `compute_ctl` reports `empty`, then `Running` (warm), and `Stopping` while it drains.
  - `ComputeLaunch` gains `namespace`, which a binding must match (`ComputeLaunch::namespace`). This changes the R13.1 contract: an unbound launch has no other place to name its namespace. The runner's wire carries the new field.
- **R14.4 One maintainer.**
  - The instance holding the lease `e/pg/warm-pool` ticks every 10 s, or every 1 s while computes start, and at once after a take (`WarmPool::kick`).
  - Each tick it starts up to `size_per_key` computes per wanted key, at most 16 per tick. It marks computes `Running` once their `compute_ctl` reports `empty`.
  - It drains a compute that died, did not report within the start timeout (60 s), is older than `max_age` (24 h, so a taken compute has a fresh image and token), is signed with another key, or whose key is unwanted or whose image is no longer the major's. A changed image drains the pool.
  - A drain first writes `Stopping` by compare-and-set, and a take binds by compare-and-set. One of the two writes fails, so a taken compute is never stopped. A write that finds the record bound fails the project check (`InvalidArgument`) and is treated as lost.
  - Every instance's tick reads the unbound computes and keeps the warm ones in memory for takes. The read is a full `C/` scan, per instance per tick, made only while a pool is wanted or exists. An index (a new tag) can replace the scan when compute counts make it costly (Task 46 measures it).
- **R14.5 Taking (the endpoint reconciler, under the project's lease).**
  - An endpoint whose `min_cu` equals the pool's size, 0.25 CU (the default `min_cu`, R11.2), takes a warm compute of its key. Before binding, it checks that the compute's `compute_ctl` still reports `empty`, so a dead compute does not count as the endpoint's crash.
  - One batch binds the compute (`binding`, `Pending`, `bound_at_ms`, the inputs' `spec_version`) and names it on the endpoint.
  - It then pushes `/configure`, waiting at most `push_wait` (2 s). `compute_ctl` takes the spec at once but answers only when Postgres runs, and the passes poll for that. A warm compute never reads its spec again, so a bound pool compute that still reports `empty` is pushed again (`CtlStatus::Empty`, new).
  - The start timeout counts from `bound_at_ms`. Token-age replacement still counts from creation, when the token was issued.
  - An endpoint of another size starts its own compute. Resizing a warm compute on take is Task 15's to consider.
  - On Kubernetes, `bind_address` also labels the Pod `loams.dev/{project,endpoint}`, because a warm Pod starts with them empty. The Role gains `patch` on `pods`. This adds no reach: `pg-control` already creates Pods with any spec in these namespaces.
  - A warm Pod's certificate names its own Pod (`cmp-<ulid>.computes.loams-pg-<ns>.svc…`), because its endpoint is not known yet. **Carry to Task 23:** PgDog's hop to a pool-born compute must verify the compute's own name or the CA (`verify-ca`), not the endpoint Service's name, or Task 23 issues a bound certificate on take.
- **R14.6 Lifecycle and metrics.** A warm compute serves no endpoint, so it sends no lifecycle event until it is bound. From then on it is an ordinary compute (`Started` when it runs, then `Suspended` or `Failed`). `loams_pg_pool_size{state}` and `compute_starts_total{cause=pool}` belong to Task 46, which can read `ComputeRec.pool`.
- **R14.7 Tests.**
  - `tests/pool.rs` (local store, `endpoints/env.rs`'s fakes, with the fake `compute_ctl` answering `empty` for a warm compute until it is configured) covers the five named cases: `pool_refills_to_size`, `take_binds_and_refills`, `image_change_drains_pool`, `pool_size_zero_disables` and `bound_compute_gets_attached_spec`. The last runs through the upcall listener: `empty`, then `attached` with the pushed spec.
  - It adds `other_sizes_start_their_own`, `one_instance_maintains` and `unwanted_pool_drains_once_settled`.
  - `tests/runtime_kube.rs` adds `unbound_pod_starts_and_bind_labels_it` and `unbound_certificate_names_the_pod`. `role.json` is re-blessed for `patch`. The CLI adds `the_warm_pool_is_kubernetes_only`.
  - No TiKV variant: the pool's writes are plain fenced compare-and-sets, which the conformance suite covers.
  - No kind run: the pool on kind needs storage, which waits for Task 48. Task 17's cold-start harness measures it with the pool on and off.
- **R14.8 Also changed.** `WatchEvent::Endpoint` holds a `Box<EndpointView>`, because `ComputeRec` grew past clippy's `large_enum_variant`.

### Task 15 rulings (2026-10-15)

- **R15.1 Q644, answered.**
  - Autoscaling uses Kubernetes in-place Pod resize (D712, the `resize` subresource of Task 13). NeonVM and Neon's autoscaler agent are not used: they need KVM nodes, which BYOC and Autopilot-class clusters lack.
  - 0.25 CU stays the smallest size.
  - Compose cannot resize a running container (R10.4), so `pg-control` builds the autoscaler only with `--runtime kubernetes`. A runtime that answers `Unsupported` is never asked again for that compute.
- **R15.2 Samples (`ComputeMetrics`, `src/autoscale.rs`).**
  - **CPU** comes from the Kubernetes metrics API (metrics-server): the `compute` container's `usage.cpu` in the Pod's `PodMetrics`, as a fraction of the compute's size. `KubeRuntime` implements it. The Role gains `get` on `metrics.k8s.io` `pods`, and nothing else in that group.
  - **The file cache's size and working set** come from `compute_ctl`'s unauthenticated `/autoscaling_metrics`. That is the `neon` communicator's exposition (`pgxn/neon/communicator/src/worker_process/lfc_metrics.rs`): `lfc_cache_size_limit` and `lfc_approximate_working_set_size_windows{duration_seconds="300"}` in 8 KiB pages. It is read through `ComputeCtlClient::autoscaling_metrics` (fixture `src_autoscaling_metrics.txt`, built from the source) and `ComputeCtl::file_cache`.
  - `Combined` joins the two sources. A source that fails leaves its fields unknown, and an unknown field triggers nothing.
  - **The no-metering guard (R9.8).** It flagged the API's own name for a container's figures (`usage`), so the code reads the one object in the container's entry that holds `cpu` and does not name it. The test reads Kubernetes' document from `tests/fixtures/kube/pod_metrics.json`. The guard exempts that one file, by its exact path, from the name check (`FOREIGN_DOCUMENTS`), but still checks it for markers. `test_pg_guard_skips_only_listed_foreign_documents` covers this.
- **R15.3 The file cache is in the spec.** Neon leaves the local file cache off by default (`neon.max_file_cache_size = 0`), so the spec now sets two values after the endpoint's settings:
  - `neon.max_file_cache_size` is 16 CU's cache. It is a start-time setting, and only sizes a shared-memory table.
  - `neon.file_cache_size_limit` is the compute's own size. It is reloadable, and the autoscaler moves it.
  - The cache is 768 MiB per 0.25 CU (75 % of the memory), at the extension's default path `file.cache` in the data directory, on the Pod's `emptyDir`. `spec::build` therefore takes the compute's CU. A changed size changes the spec's version, so a pass that sees a cache size the compute did not get pushes it.
- **R15.4 The policy (`Autoscaler::tick`, pure).**
  - It runs every 5 s. It scales up after 15 s above 70 % CPU, or at once when the working set passes 75 % of the cache. It scales down after 2 minutes below 30 % CPU, and only if the working set would fit 75 % of the smaller cache.
  - A step is at most ×2 up and −25 % down, and always at least one 0.25 CU step. Below 1 CU, 25 % is less than a step, so the step is one quarter. A compute outside its endpoint's `[min_cu, max_cu]` moves inside at once.
  - **Windows.** A decision's window holds until the decision is applied, so a refused resize is asked again at the next tick, not after a new window.
  - **Caps.** A namespace cap on its computes' total CU (`Scaling::set_cap`, which Task 47's limits record feeds) cuts each scale-up to what is left, taken in observation order.
- **R15.5 The applier (`src/reconcile/scale.rs`, under the project's lease).**
  - The reconciler's passes follow each running compute (`Scaling::track`, refreshed with the endpoint's range on every pass) and stop when it ends.
  - Each tick samples the followed computes, decides, and applies:
    - Up: resize, then `/configure` the larger cache.
    - Down: `/configure` the smaller cache first, then resize. A resize that does not apply puts the cache back.
  - An applied resize writes `C/`'s `cu` and `spec_version` by compare-and-set, and the observer receives `Resized { cu }`.
  - A compute that is no longer its endpoint's running one, or whose lease is held by another instance, is no longer followed on this instance.
- **R15.6 Tests.**
  - `tests/autoscale.rs` holds the six named cases, plus `unsupported_resize_stops_deciding` and `scale_down_shrinks_the_cache_first`. The applier runs on `endpoints/env.rs`'s fakes: the runtime takes a queue of resize answers, and resizes and `/configure`s are logged in order.
  - `tests/runtime_kube.rs` has `pod_cpu_from_the_metrics_api`, and `role.json` is re-blessed.
  - `loams-postgres`'s client test parses the exposition.
  - No kind run: metrics-server and storage in kind wait for Task 48. That the cache reloads without a restart (§46 §7.4's "verify") is Task 17's to measure on a real compute.

### Task 16 rulings (2026-10-15)

- **R16.1 Read-only endpoints.**
  - A branch may have any number of read-only endpoints (Task 11 already allowed them). Their computes get a `Replica` spec, a hot standby following the branch's head.
  - The warm pool binds to them too: the spec decides the mode.
  - A `CreateEndpoint` of a read-write endpoint now also counts a read-only endpoint that is being promoted as the branch's read-write endpoint.
- **R16.2 `standby_horizon` from `loams-wal` (Task 32's review M4).**
  - The replication reader parses each replica's `'r'` standby status. Each connection keeps its last applied LSN; an applied LSN of 0 means none, and the entry goes when the connection closes.
  - `WalService::standby_horizon(tl)` is the least of those LSNs on this instance, as the fork's `walsenders.get_hotstandby()` computes it. It is published as `standby_horizon` in `SafekeeperTimelineInfo` and in discovery responses.
  - Hot-standby `'h'` feedback is not relayed to the proposer. Replica queries can then be cancelled by conflicts, which does not affect correctness. Relaying it is left for Task 39 (HA).
  - Test: `standby_horizon_published_from_replica_feedback`.
- **R16.3 Promotion (`RestartEndpoint{promote}`).**
  - **The service** (`promote_endpoint`). The endpoint must be read-only, running, and not already being promoted. The branch's read-write endpoint must be down: no compute, and not asked to run. Otherwise the answer is `failed_precondition`.
  - The batch sets `EndpointRec.promote`. It also checks the read-write endpoint's version and rewrites the branch's guard unchanged, so a racing start or create conflicts with it.
  - The operation is `postgres.endpoint.restart` on the wire, as the proto says. Internally it is a new `OperationKind::EndpointPromote`, appended.
  - **The reconciler**, on the running replica, under the project's lease:
    1. It checks again that the read-write endpoint is down. If it is not, the operation fails.
    2. It prewarms: `/lfc/prewarm`, from the read-write endpoint's saved cache state. It waits up to `PREWARM_WAIT` (2 min) for it.
    3. It calls `/promote` with a primary's spec, at the flushed LSN from `loams-wal`.
    4. One batch swaps the two endpoints' types and records the primary spec's version.
  - A refusal (R2.13: an unprewarmed replica) fails the operation with `failed_precondition`, and nothing changes.
  - The same compute keeps serving behind the same address. The old read-write endpoint starts as a replica afterwards.
- **R16.4 Endpoint storage (R2.13: "this task deploys endpoint storage for the stacks that promote").**
  - **Tokens and key.** `ComputeKeys` signs endpoint storage's EdDSA tokens, with claims `{tenant_id, timeline_id, endpoint_id, exp}`. It also writes its public key as an SPKI PEM, the `pemfile` endpoint storage reads.
    - The upcall and `compute_ctl` tokens have other claims, so no verifier takes another's token, and one key serves all three.
    - A token's expiry is the compute's creation time (from its ULID id) plus `COMPUTE_TOKEN_TTL`. The same records therefore still give the same spec bytes.
  - **Verified against the pinned image's `endpoint_storage`.** It loaded our PEM. Our token could PUT and GET its own endpoint's state. Another key's token got 400. A PUT to another endpoint's path got 401. A GET of another endpoint's state is allowed, which is Neon's design so a promoted replica can read the primary's state.
  - **The spec.** With `--endpoint-storage <host:port>`, every compute gets endpoint storage's address and token and `autoprewarm`. A primary also gets `offload_lfc_interval_seconds = 300`, a new spec field in `loams-postgres`.
  - **Deployment.** The release stack's `postgres-control` profile runs `endpoint-storage` from the included storage stack's pinned `neon` image. That image is not a neon-archive copy, and the service is not one of the barred proxies.
    - It uses the stack's RustFS bucket `neon` under `endpoint-storage/`, and publishes no ports.
    - `pg-control` writes the key into a shared volume (`--endpoint-storage-pem-out`). Endpoint storage mounts it read-only and waits for it.
    - The `loams-pg-control` image creates `/run/endpoint-storage`.
    - `release-compose.yml`'s checks pass locally.
  - The Helm chart's deployment is Task 48's.
  - **Without endpoint storage** (`deploy/loams-postgres-dev`, single node), `/lfc/prewarm` ends `failed` ("endpoint_storage_addr missing"). The pinned `compute_ctl` promotes after a failed prewarm (it refuses only `not_prewarmed` and `prewarming`), so the promoted compute starts with a cold cache.
- **R16.5 Tests.**
  - `tests/replicas.rs` covers the four named cases: `ro_endpoint_starts_replica_spec`, `promote_swaps_types`, `promote_refused_while_rw_running` and `promote_refuses_an_unprewarmed_replica`. Specs are read through a real upcall listener (`Env::upcall_spec`).
  - Unit test `endpoint_storage_takes_our_tokens_and_only_them`.
  - `loams-postgres`'s spec test covers the offload field.
  - The plan's `ro_rejects_writes` (`it_compose`) is `it_ro_endpoint_rejects_writes_then_promotes` in `tests/it_endpoint.rs`, run in `pg2-e2e.yml`'s compose job. On the dev stack, with real computes, it checks:
    - the replica replays a row;
    - it refuses an INSERT with `read_only_sql_transaction`;
    - the promotion is refused while the read-write endpoint runs;
    - once that endpoint is suspended, the replica is promoted behind the same address and takes writes.
- **R16.6 The pinned `compute_ctl` panics in `/promote` (found by the real-compute test).**
  - After `pg_promote()` succeeds, `promote_impl` runs on a tokio worker and calls the synchronous `reconfigure`. That calls `apply_spec_sql`, which does `Handle::current().block_on` ("Cannot start a runtime from within a runtime", `compute_tools/src/spec_apply.rs:50`). The task dies, and the answer is 500 "promote sender dropped". The fork's source at the pinned commit has the same code.
  - Neon's own test escapes it because `neon_local`'s specs set `skip_pg_catalog_updates`.
  - **Workaround here:** the promotion's spec sets `skip_pg_catalog_updates: true`, a new field in `loams-postgres`'s spec, sent only with `/promote`. The replica already has the primary's catalog. The compute's spec version is recorded as unknown, so the next pass pushes the full primary spec through `/configure`, which applies the catalog on a safe path.
  - **Carry to NF1's patch queue:** run the catalog step of `promote_impl` off the async worker (`spawn_blocking`), or make it async.
- **R16.7 Replicas never streamed WAL (found by the real-compute test; a gap since Task 8's spec).**
  - Neon's control plane gives a hot standby `primary_conninfo` naming the safekeepers (`host=h1,h2 port=p1,p2 options='-c timeline_id=… tenant_id=…' application_name=replica replication=true`), `primary_slot_name = repl_<timeline>_`, `hot_standby = on` and `recovery_prefetch = off` (`control_plane/src/endpoint.rs`). Our spec gave none of these, so a read-only compute stayed at its base backup.
  - The spec now sets them for `Replica` mode, after the endpoint's settings, which cannot name them (R11.3).
  - `it_ro_endpoint_rejects_writes_then_promotes` checks that a row inserted after the replica started reaches it.
- **R16.8 Real-compute runs (2026-10-15).** On `deploy/loams-postgres-dev`, as compose project `pg2-b2-it` under rootless Podman:
  - `it_ro_endpoint_rejects_writes_then_promotes` passed twice. The promotion target is safekeeper1's flushed LSN after the read-write endpoint's suspend; the test reads it from safekeeper1's HTTP API (`IT_SAFEKEEPER_HTTP`), because its Neon is a fake.
  - `it_endpoint_starts_suspends_and_deletes`, `it_idle_long_query_is_activity_then_suspends` and `it_compose_compute_answers_select_1` also pass. They now carry Task 15's file cache settings, so real computes start with the LFC on (R15.3).
- **R16.9 No second primary (from CodeRabbit's review of #635).**
  - **Ambiguous `/promote` errors.** Only `compute_ctl`'s refusals before `pg_promote()` keep the compute as the replica: not a replica, not prewarmed or prewarming, not caught up, or a 4xx. Any other error (a lost answer, "promote sender dropped", a failure after `pg_promote()`, or no answer within `PROMOTE_WAIT`, 45 s, which is less than the 60 s lease) means the outcome is unknown. In that case the promotion fails and the compute is replaced by a fresh replica (`replace_compute` in the same batch).
  - **Starts during a promotion.** While a read-only endpoint of a branch is being promoted, the branch's read-write endpoint is not started or restarted (`failed_precondition`). Both requests rewrite the branch's guard unchanged, so a start and a promotion conflict whichever commits first.
  - **A replica that stops.** A replica that is suspended or fails ends its pending promotion as `aborted`, and the flag is cleared in the same write.
  - The PEM is written to a temporary file and renamed into place.
  - Tests: `suspend_ends_a_pending_promotion`, `rw_start_refused_while_promoting`, `an_unknown_promotion_outcome_replaces_the_compute` and `promote_errors_are_refusals_only_before_pg_promote`.

### Task 17 rulings (2026-10-15)

- **R17.1 What is measured.** The time from `StartEndpoint` on a suspended endpoint to the first `SELECT 1` that answers through the endpoint's address, as a login role, tried every 25 ms.
  - The reconciler is driven as its run loop paces it: a pass, then the wait the pass asks for, at most 500 ms (`Computes::poll` while a compute starts).
  - The first start is not counted: it pulls the images and makes the role.
  - The measurement is `it_cold_start_samples` in `tests/it_endpoint.rs`. It uses the API service, the endpoint reconciler and the compose runtime, with real computes on `deploy/loams-postgres-dev`.
  - `scripts/pg2/cold-start.sh` brings up the storage as its own compose project (`pg2-coldstart`), runs it, and writes `bench/results/<date>-<sha>-coldstart.json` through `scripts/pg2/coldstart_stats.py`. That helper takes p50, p90 and p99 by nearest rank (as the WAL benchmark's `stats.py`), min, max and mean, and §46 §16's target per variant.
  - `stats_from_samples` is in the helper's `--self-test`, which CI's light job runs.
  - `loams-pg-bench.yml` gains `suite: coldstart` (a `workflow_dispatch` on the `loam-bench` runner) with `runs`.
- **R17.2 Variants.**
  - Recorded now: compose with the pool off. Compose has no warm pool (R14.2).
  - Kind with the pool on and off needs storage in the cluster, which Task 48's chart brings. The kind variants are added to the script then.
  - **`scripts/pg2/wake-bench.sh` is not written here.** It measures from the first client connection through PgDog and the waker, so it belongs to Task 26, which builds the waker. The plan's PG2b exit records the numbers without gating.
- **R17.3 The recorded run** (2026-10-15, at `b8da2481` plus this task's test; Linux x86_64, 14 CPUs, rootless Podman, images cached), 50 cold starts on compose with the pool off:
  - p50 1417 ms, p90 2271 ms, p99 6594 ms; min 1026 ms, max 6594 ms, mean 1760 ms.
  - This meets §46 §16's compose target (p99 ≤ 20 s). The samples are in `bench/results/20261015-b8da248180df-coldstart.json`.
  - The few slow samples (3.3 s to 6.6 s) were not diagnosed: the shared build machine was busy.
- **R17.4 PG2b exit.**
  - Done: endpoints start, suspend when idle, restart and promote on compose (real computes: `it_endpoint`). The Kubernetes runtime's objects and lifecycle are tested against a stand-in API, and kind starts its Pods.
  - The autoscaler is built (Task 15). It resizes only on Kubernetes, and real resizes wait for Task 48's cluster.
  - Not yet run: Kubernetes end to end (storage in kind, Task 48).

### Task 18 rulings (2026-10-15): PgDog, checked

The checks ran on the scratch stack `deploy/pgdog/compose.yaml`: the pinned PgDog, two `postgres:17.11` backends (`pg1`, `pg2`), and `pg3`, which starts later. Configs are in `deploy/pgdog/scratch/`. They were run locally under rootless Podman with `psql` 17 as the client. Nothing of PgDog's is in the repository: these are field names and observed behaviour.

- **R18.1 The pin.**
  - PgDog **v0.1.60** (2026-09-24, git `0d040f9`), the newest release at least 14 days old. v0.1.61 (2026-10-09) is six days old.
  - Image `ghcr.io/pgdogdev/pgdog@sha256:25d1908886f595a2e3b00ec59783326712e3c0f75f91d29f032ff712b30f2266`, the multi-arch index (linux/amd64 is `sha256:1bf5346d…`). This is the same pin as `conformance/router` and RT1's inventory.
  - Still **AGPL-3.0** (`gh api repos/pgdogdev/pgdog`: `license: AGPL-3.0`, not archived).
- **R18.2 Q640: PgDog queues a client while its backend is down.**
  - A client of a database whose server cannot be reached waits in the pool's queue. `SHOW POOLS` shows `cl_waiting = 1`, and `SHOW CLIENTS` shows the client's `state` as `waiting`.
  - Meanwhile PgDog asks for a new server connection about every `connect_attempt_delay` (500 ms here; reason `client_waiting`), each with `connect_timeout` (1 s).
  - With `pg3` started 3 s after the client connected, the client's query answered **5.1 s** after it connected, once Postgres was up.
  - A backend that never comes back fails the client with `FATAL: checkout timeout` after **`checkout_timeout`** (15 s set; it took 16.1 s).
  - The pool of a lone primary is **not banned** (`banned = f`), so the next client waits the full `checkout_timeout` again (15.0 s).
  - The settings, in ms in `[general]`: `checkout_timeout`, `connect_timeout`, `connect_attempts`, `connect_attempt_delay`, `ban_timeout`, `healthcheck_interval`, `idle_healthcheck_interval`, `idle_healthcheck_delay`, `healthcheck_timeout`, `min_pool_size`, `idle_timeout`.
- **R18.3 Q643: `users.toml` takes a SCRAM verifier.**
  - `password_hash` holds Postgres's own verifier (`SCRAM-SHA-256$4096:…`, as `pg_authid.rolpassword` shows it). The right password logs in over SCRAM, and a wrong one is refused ("password … is wrong").
  - The backend login is separate: `server_user` and `server_password`. `current_user` on the backend was the server user.
  - `passwords` (several) and `server_auth` (`rds_iam`, `azure_workload_identity`, `vault_dynamic`, `vault_static`) also exist.
  - **Ruling for Task 24:** `users.toml` holds each role's SCRAM verifier, so the plaintext is never stored there. It also holds a backend-only password that `pg-control` generates per role and that the user never sees, with `server_user` as the same role. §46's assumption that PgDog needs the user's plaintext is withdrawn.
- **R18.4 Q642: no JWT or OIDC client auth, and no SNI routing in PgDog.**
  - Client `auth_type` is one of `scram`, `md5`, `plain` or `trust`. `jwt` appears only as Vault's Kubernetes login path. Nothing reads the TLS server name.
  - So the credential exchange (Task 25) and the L4 SNI listener in front (Task 22) stay as designed.
- **R18.5 Other settings.**
  - **Pool mode.** Per database (`[[databases]].pooler_mode`) and per user (`users.pooler_mode`): `transaction`, `session` or `statement`.
  - **Replicas.** A `[[databases]]` entry has `role = "primary" | "replica" | "auto"`. Replicas get `read_only = true`. Load balancing is `load_balancing_strategy` (default `random`). The read/write split is `read_write_split` (default `include_primary`) and `read_write_strategy` (default `conservative`). A user can be `read_only`.
  - **No allowed-address or `hba`-style setting** exists, so NetworkPolicy alone decides who reaches PgDog (Tasks 22 and 48).
  - **TLS.** Clients: `tls_certificate`, `tls_private_key`, `tls_client_required`, and `tls_client_ca_certificate` for mTLS. Backends: `tls_verify` (`disabled`, `prefer` (default), `verify_ca`, `verify_full`), `tls_server_ca_certificate`, and client certificates per `[[databases]]` entry (Task 23).
- **R18.6 `RELOAD` re-reads both files and keeps connections.**
  - Users added to `users.toml` and 1,000 databases added to `pgdog.toml` were all served after one `RELOAD` (1,004 pools; `db999` answered).
  - The admin command answered in **1.85 s** with 1,000 databases.
  - A client connected across the reload ran its next query on the **same backend** (pid unchanged).
- **R18.7 The admin database** (`[admin]` `name`, `user`, `password`).
  - `SHOW POOLS`: `id, database, user, addr, port, shard, role, cl_waiting, sv_idle, sv_active, sv_idle_xact, sv_total, maxwait, maxwait_us, pool_mode, paused, banned, healthy, errors, re_synced, out_of_sync, force_closed, online, schema_admin`. **There is no `cl_active`.**
  - `SHOW CLIENTS`: `id, user, database, addr, port, state (active|idle|waiting), replication, connect_time, last_request, queries, transactions, transactions_2pc, wait_time, query_time, transaction_time, bytes_received, bytes_sent, errors, application_name, locked, prepared_statements`.
  - `SHOW SERVERS`: `pool_id, database, user, addr, port, state, connect_time, request_time, remote_pid, client_id, transactions, queries, rollbacks, prepared_statements, healthchecks, errors, bytes_received, bytes_sent, last_sent, last_received, age, application_name`.
  - `SHOW DATABASES` and `SHOW USERS` answer one empty column in this version. `SHOW CONFIG` lists the effective settings.
  - **Change to R12.3's `PoolRow`, carried to Task 21's admin adapter:** `cl_active` is not a column. The idle monitor's clients of an endpoint are `sv_active + cl_waiting` from `SHOW POOLS`, which counts a client holding a server in a query or transaction. Alternatively they are the `active` and `waiting` rows of `SHOW CLIENTS` for the pool's database.
- **R18.8 Metrics.** OpenMetrics is on `openmetrics_port` and `openmetrics_host`, with no prefix unless `openmetrics_namespace` is set. The series include `cl_waiting`, `clients`, `sv_active`, `sv_idle`, `sv_idle_xact`, `maxwait`, `banned`, `errors`, `max_connections`, and the `avg_*` family (`avg_query_time`, `avg_wait_time`, `avg_xact_time`, `avg_checkout_timeouts`, …). Task 28 maps them.
- **R18.9 §46 §8.4's path: PgDog's own queue, with the waker.** No Neon proxy and no shim.
  - **What Tasks 20, 26 and 27 render:**
    - `checkout_timeout` **20 s** above the compose cold-start p99 (6.6 s, R17.3) and §46's Kubernetes no-pool target of 15 s. §46 §8.4's 15 s is amended.
    - `connect_timeout` 1 s, `connect_attempts` 1 and `connect_attempt_delay` 500 ms, so a waiting client sees its compute within about 0.5 s of it listening.
    - `ban_timeout` short (1 s) for replica pools.
  - **What the waker does.** It reads `cl_waiting > 0` on a suspended endpoint's pool, as R12.3 and §46 §8.4 have it.
  - **Health checks must not keep computes awake** (the carry from Task 12). An idle-connection health check runs a query on the compute, which moves `compute_ctl`'s `last_active` (R12.1). The rendering therefore uses:
    - `idle_healthcheck_interval = 0` (off);
    - `min_pool_size = 0` (no connection is kept open, so none is checked and a suspended endpoint is not dialled for `below_min`);
    - `idle_timeout` below the shortest suspend timeout (30 s, under the 60 s minimum), so PgDog lets go of an idle compute.
  - Checking out a connection still health-checks it when it is older than `healthcheck_interval`, but only on behalf of a client, which is activity anyway.

### Task 19 rulings (2026-10-15)

- **R19.1 The bundle files.**
  - `deploy/pgdog/LICENSE-NOTICE.md` gives the program, the version, the image digest, the exact source tag (`https://github.com/pgdogdev/pgdog/tree/v0.1.60`), the licence and "no modifications".
  - `deploy/pgdog/LICENSE.AGPL-3.0` is PgDog's `LICENSE` at that tag: the FSF's AGPL v3 text, unchanged, with no PgDog-specific content.
  - `deploy/pgdog/README.md` states §46 §8.3 verbatim.
  - Bundles that reference the image carry both files: the single-node compose (Task 56), the Helm chart (Task 48) and the desktop's stack.
- **R19.2 `LICENSES.md` is new, at the root.** None existed. `NOTICE` lists code derived into Loams, while `LICENSES.md` lists programs Loams runs as separate, unmodified processes. Its first row is PgDog.
- **R19.3 The check** (`scripts/ci/pgdog-licence.sh`, over tracked files) fails in four cases:
  - **A PgDog package:** a `pgdog`-named package in a `Cargo.lock`, `pnpm-lock.yaml`, `package-lock.json` or `yarn.lock`, or a vendored `pgdog*` directory under `vendor/`, `vendored/` or `third_party/`.
  - **A reference in the wrong place:** a reference to `ghcr.io/pgdogdev/pgdog` outside the three bundles.
    - The router conformance harness (`scripts/router/inventory`, RT1's, never distributed) is allowed.
    - Prose (`*.md`) may name the image.
  - **An unpinned reference:** one with no full `sha256` digest. An `@$var` reference counts as pinned when its file pins a full digest, as the harness's `digest="${PGDOG_DIGEST:-sha256:…}"` does.
  - **A bundle without its notice:** a bundle that references the image but has no `LICENSE-NOTICE.md`.
  - It then runs `scripts/spec/provenance.sh`.
  - Its `--self-test` covers `no_pgdog_in_lockfiles`, `pgdog_image_pinned_by_digest` and `licence_bundle_present` (eleven fixture repositories).
- **R19.4 Where it runs.** It runs in `ci.yml`'s unconditional light job, not in `pg2.yml` as the plan said. `pg2.yml` runs only when Postgres paths change, and a PgDog package could arrive through any lockfile or deploy file. It is part of `CI required`.

### Task 20 rulings (2026-10-15)

- **R20.1 Where the renderer lives.** It is `crates/loams-pg-control/src/pgdog/render.rs`: pure and deterministic, from the records.
  - RT1's renderer was planned as `loams-sqlrouter/src/render/pgdog.rs`, and RT1 has not built it or `loams-sqlrouter-io`. This task does not create another track's crates.
  - When RT1's sharded renderer exists, a sharded project delegates to it. Until then, sharded projects (a shard map, R2.14) are left out and listed in `RenderedConfig::left_out` (`sharded_project_is_left_out`, in place of `sharded_project_delegates_to_rt1_renderer`).
- **R20.2 Routed names.** They follow §46 §8.1 through `format_routed` only.
  - `<project>` (the default branch) and `<project>__<branch>` reach the branch's `postgres` database. No record names a default database, and every compute has `postgres`.
  - `<project>__<branch>.<db>` reaches each database record of the branch.
  - `<project>__<branch>__ro` reaches each read-only endpoint as a `replica` (`read_only = true`) of one cluster, so PgDog balances them. Primary names never list replicas, so no read/write split happens on them (§46 §8.1's default).
  - Hosts are the endpoints' recorded backend addresses. An endpoint without an address is left out. Compose's network alias (R10.10) is Task 22's mapping.
- **R20.3 Pool sizes and `max_connections`.**
  - `max_connections(cu)` follows Neon's table: about 450 per CU, 112 at 0.25 CU, at most 4,000. It is computed at the endpoint's largest size (`max_cu`), because it is a start-time setting.
  - The spec now sets it, after the endpoint's settings (R11.3 already reserves it).
  - Each `[[databases]]` entry's `pool_size` is 80 % of it (§46 §8.1). The role limit is absent: no role record has one yet.
  - The pool mode is the endpoint's, and a role's `pool_mode` overrides it in `users.toml`.
- **R20.4 Users and grants.**
  - A login, non-system role is granted every routed name of its own branch, and only those. That is until Task 24's explicit grants.
  - One `[[users]]` entry per role lists its names, with `password_hash` (the role's SCRAM verifier), `server_user` (the same role) and `server_password` (the backend-only secret, R18.3).
  - A role without credentials is left out. `NamespaceRecords::read` reads the records without credentials, and Task 24 supplies them.
- **R20.5 Both files are secrets.** `RenderedConfig` returns `pgdog_toml: Secret<String>`, not a plain string, because `[admin] password` lives in `pgdog.toml` (PgDog takes it from nowhere else). Both go to a Secret, not a ConfigMap (Task 21).
- **R20.6 `[general]`.**
  - Listener 6432, OpenMetrics 9090, `auth_type = "scram"`, transaction pooling, and R18.9's timeouts:
    - `checkout_timeout` 20 s, `connect_timeout` 1 s, `connect_attempts` 1, `connect_attempt_delay` 500 ms, `ban_timeout` 1 s, `healthcheck_interval` 30 s;
    - `idle_healthcheck_interval` 0, `min_pool_size` 0, `idle_timeout` 30 s, so health checks never keep a compute awake.
  - TLS toward clients (`tls_certificate`, `tls_private_key`, `tls_client_required = true`) and toward computes (`tls_verify = "verify_ca"` with the compute CA) when configured; otherwise off (Task 23).
- **R20.7 Checked against the pinned PgDog.** v0.1.60 loaded the rendered golden (TLS paths dropped) and built one pool per (routed name, role) as rendered: `migrator`'s pools in session mode, and `shop__main__ro` as a replica. Tests in `tests/render.rs`:
  - `render_is_deterministic`, `render_golden_two_projects_three_branches` (goldens in `tests/golden/pgdog/`), `ungranted_role_absent_from_users`, `ro_endpoint_rendered_as_replica`, `pool_size_follows_cu`;
  - `sharded_project_is_left_out`, `plain_settings_and_addressless_endpoints` and `records_are_read_from_the_store`.

### Task 21 rulings (2026-10-16)

- **R21.1 The sidecar.** `crates/loams-pgdog-sync` builds `pgdog-config-sync` (Apache-2.0, a separate process; PgDog stays unmodified).
  - It listens on 7695 with mutual TLS only: a client certificate must chain to the configured CA (`pg-control`'s), and anything else fails the handshake, because the files hold secrets (R20.5).
  - Routes: `PUT /v1/config` (JSON `{generation, pgdog_toml, users_toml}`, at most 16 MiB) and `GET /v1/generation` (the plan's `GET /generation`, versioned).
  - Each file goes to a temporary file in the same directory, is fsynced, and is renamed over the old one, so a reader sees a whole old or a whole new file; the generation is written last.
  - At start, `--persisted` (the mounted Secret) is copied in unless the directory already holds a generation, so a restarted PgDog serves the last pushed config before `pg-control` pushes again.
- **R21.2 The generation is Task 20's digest of both files**, not a counter. Identical records give the same generation on every `pg-control` instance and after a restart, so the acknowledgement is "the sink answers the rendered generation".
- **R21.3 Where the applied generation lives.** RT1's `ConfigPush` machine is not built, so `PgDogPusher` keeps the generation each replica applied (sent, acknowledged, reloaded) in memory. After a restart it knows none, so the first drift check pushes every replica once; that is harmless, because `RELOAD` keeps live connections (R18.6).
- **R21.4 Drift.** The pinned PgDog's `SHOW DATABASES` answers one empty column and `SHOW USERS` gives nothing to compare with a render (R18.7). So a replica has drifted when its sink holds another generation than the records render, or when this pusher has not reloaded it at that generation (a failed `RELOAD`). `PgDogAdmin::show_databases` answers the distinct `database` of `SHOW POOLS`.
- **R21.5 Order per replica:** send, poll the sink's generation (every 100 ms, up to 5 s), then `RELOAD`. A replica that does not acknowledge is never reloaded and is reported in `Pushed::failed`; the other replicas still get the push.
- **R21.6 Compose and single node.** `DirSink` writes `<root>/<namespace>/`, which PgDog's container mounts as a directory (not as single files, so the renames are visible inside it), then sends `RELOAD`. `it_pgdog` checks this against the pinned PgDog: a pushed read-only endpoint shows in `SHOW POOLS` without a restart.
- **R21.7 The idle monitor** counts `sv_active + cl_waiting` (R18.7's carry); `PoolRow.cl_active` is gone.
- **R21.8 What waits.**
  - Task 22, which deploys PgDog: the binary's flags, the kicks (a store watch on the namespace's records into `PgDogPusher::run`), a periodic `resync`, and the sidecar's image and Helm templates. Until then nothing runs PgDog, and `pg-control` keeps `NoPgDog`.
  - Task 23: the admin connection is plaintext until then.
  - Task 24: credentials. `NoCredentials` renders no users.
  - `run` pushes due namespaces one after another, so one slow replica (up to the 5 s acknowledgement timeout) delays the others. Task 22 decides whether to push namespaces concurrently.
- Tests: `crates/loams-pgdog-sync/tests/sync.rs` (`sync_writes_atomically`, `sync_requires_mtls`, `pgdog_restart_loads_persisted_copy`); `tests/push.rs` (`push_debounces`, `reload_after_generation_ack`, `drift_triggers_repush`, `failed_reload_is_retried_by_resync`, `sidecar_sink_pushes_over_mtls`, `dir_sink_writes_the_namespace_directory`); `tests/it_pgdog.rs` (`it_reload_picks_up_a_pushed_change`, ignored by default, run locally with Podman).

### Task 22 rulings (2026-10-16, before any code)

- **R22.0 The SNI listener is `loams-gateway`, not Envoy, Kourier or Traefik** (owner, 2026-10-16: "build a crate"; design [§57](../design/57-loams-gateway.md), D1011, amending D708).
  - **Why a generic L4 router cannot do it:** a Postgres client sends a plaintext `SSLRequest` before its ClientHello, so the first bytes carry no SNI; PgDog does TLS only after `SSLRequest` and reads no SNI (R18.4); and PgDog negotiates no ALPN, which libpq 17's `sslnegotiation=direct` requires (§57 §3).
  - **What the gateway does** (D1014–D1017): it answers `SSLRequest` with `S`, reads SNI, sends its own `SSLRequest` to the namespace's PgDog and splices the client's TLS bytes, so PgDog still terminates that TLS (D710). It terminates direct-TLS clients with ALPN `postgresql` and re-encrypts to PgDog, verifying PgDog's certificate. Plaintext is refused with `FATAL 28000`. Dedicated ports are allocated on request (`dedicated_port`), and upstream choice is ordered, so cancels reach the issuing replica.
  - **Split of the work:** GW1 Tasks 1–8 build the gateway and its image. GW1 Task 9 adds `pg-control`'s routes renderer and dedicated-port records. GW1 Tasks 10–11 add the compose service and the Helm templates. This task keeps the PgDog Deployments (×2, sidecar, Service, NetworkPolicy, PDB), and its tests become:
    - `pgdog_manifests_golden`;
    - `sni_route_per_namespace_golden` (the rendered gateway routes, GW1 Task 9);
    - `kubeconform_passes`;
    - `it_kind_two_namespaces_isolated` (through the gateway, GW1 Task 11).
  - **PgDog's Service** is headless, so the gateway resolves every replica and tries them in order (D1017).
  - **PgDog's certificate** is one per namespace hostname (Task 23), mounted by that namespace's PgDog only, and the gateway reads a copy of each: on the terminate path it must present PgDog's exact certificate, because libpq's SCRAM-SHA-256-PLUS binds to the certificate the client saw (§57 §7.3, Q905).
  - **NetworkPolicy:** PgDog's port 6432 admits only the gateway's pods and `pg-control`.
