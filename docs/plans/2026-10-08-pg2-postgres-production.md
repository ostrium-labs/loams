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
- **No billing** (D548, D550): no field, metric, table or endpoint named `plan`, `price`, `invoice`, `credit`, `meter`, `billable` or `usage` in any PG2 code. `scripts/ci/no-metering.sh` covers `crates/loams-pg-control` and `deploy/helm/loams-postgres` (Task 9). Lifecycle events go out only through `ComputeLifecycleObserver`.
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
scripts/ci/{pgdog-licence.sh,no-metering.sh (extended)}
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

**Files:** `src/{authz.rs,audit.rs}`. In `crates/loams/`: `Cargo.toml` (feature `postgres` = `dep:loams-pg-control`), `src/main.rs` (subcommand `pg-control` and flags), `src/server.rs` (mounting `PostgresService`). Extend `scripts/ci/no-metering.sh`. Add `.github/workflows/pg2.yml` and `pg2-e2e.yml`. Tests: `crates/loams/tests/pg_control.rs` and `crates/loams-pg-control/tests/e2e_deploy_neon.rs` (ignored by default; runs in `pg2-e2e.yml`).

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
- `deploy/helm/loams-postgres/templates/sni-listener.yaml`: an Envoy (or Kourier/Traefik, per MT2's choice) TCP route per namespace hostname `<ns>.<region>.pg.<base-domain>` with TLS passthrough, plus a non-SNI port per namespace.
- `deploy/loams-postgres-single/` gets one PgDog service.

Tests:
- `pgdog_manifests_golden`
- `sni_route_per_namespace_golden`
- `kubeconform_passes`
- `it_kind_two_namespaces_isolated`: a client of namespace A's hostname cannot reach B's databases.

Commit `deploy(pgdog): per-namespace deployments and SNI routing`.

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

Commit `feat(pg): scram through pgdog`.

### Task 25: Credential exchange for OIDC and agent tokens (`IssueConnectCredential`)

**Files:** `src/service/connect.rs`. Tests: `tests/connect.rs` and `it_compose_token_login`.

**Interfaces:**
- `IssueConnectCredential{project, branch, role?, ttl_s}` checks the gateway-verified principal (an Authentik-backed Loams token, D449) and OpenFGA `connect`.
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
4. **MT1 / OpenFGA.** MT1 is **Planned**, not merged (`docs/plans/2026-10-02-mt1-authentik-identity.md`); no OpenFGA model or client exists in `crates/` (only design mentions and a buy-vs-build line for `openfga-client`). Ruling: Task 9 ships the `Authorizer` trait with an allow-all dev implementation behind `--pg-authz dev`, which refuses to start unless the listener is loopback, and adds the OpenFGA implementation when MT1 lands.
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
