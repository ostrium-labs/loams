# LV1 — Loams Live Production Readiness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, **test first**: write the named tests, run them and watch them fail, then implement until they pass. Each task lists the interfaces it must produce and the tests that must exist and pass before it is done. Where this plan gives exact values (names, paths, reasons, metric names, defaults), use them verbatim. The code is not pre-written in this plan (M0.3 Ruling 1). Record every deviation in "Rulings made during execution" at the end of this file.
>
> **Status: Planned** (2026-10-08). **Track LV**, beside track R (design [§45](../design/45-loams-live-production.md), D680–D699, Q625–Q639). LV1 finishes R1's unbuilt Tasks 13, 14, 16 and 17 (Task 15 is moot under D260) and pulls in the production-critical parts of R2 and R3 (§45 §2.3). It has five milestones, LV1a–LV1e, which are **executed in the order LV1c, LV1a, LV1b, LV1d, LV1e** (see "Milestones and order"). Branches `lv1<m>-t<N>` (for example `lv1c-t20`), stacked per milestone, based on `dev`; PRs target `dev`.

**Goal:** Loams Live passes the production exit checklist at the end of this plan:
- it runs highly available on TiKV, served on the main Connect port behind TLS and auth, with many isolated apps per cluster;
- it deploys versioned TypeScript functions and schemas with rollback, validators and online index changes;
- it backs up every app to object storage with point-in-time restore;
- it is observable and protected by quotas;
- it ships a TypeScript reactive client, React hooks and a deploy CLI;
- it runs in `loams dev` and Loams Desktop with no TiKV;
- its correctness, performance and security gates are green.

**Architecture:**
- **One store seam.** A new crate `loams-kv` holds the transaction surface `loams-live` uses (`Store`, `Txn`, `Snap`, `Ts`, the runner, the tuple codec, error classes, fault hooks). It has two backends: **embedded** (MVCC on redb, always built) and **tikv** (feature, wrapping `loams-tikv`). `loams-live` depends on `loams-kv` only (§45 §8).
- **Functions.** A new crate `loams-live-js` runs bundles in QuickJS (`rquickjs`), either in process or in a sandboxed per-app worker subprocess (`loams live-worker`) that talks to the host over framed protobuf on stdio (§45 §3.1).
- **Deployments and schema** are catalog records. Activation is a CAS of the catalog pointer. Index and validator changes are two-phase with a lifetime wait and a leased backfill job. Every catalog change also writes a `catalog` journal entry so all nodes switch at one tick (§45 §3.3, §4, §9).
- **Exposure.** `LiveService` and the new `LiveAdminService` are registered on the main Connect router (`crates/loams/src/api/connect.rs`), behind the port's TLS, auth and CORS layers. Apps are addressed by the `loams-live-app` header and resolved through an app directory in the keyspace `loams_live_system`. Per-app engines (runner, subscriptions, tailer, runtime) start lazily (§45 §6, §7).
- **Auth.** The `loams-auth` verifier (MT1's, or the verification core LV1b builds if MT1 has not merged) resolves Loams tokens and API keys. Per-app trusted issuers resolve end-user OIDC tokens. The `Authorizer` (D66) decides per app and per table (§45 §5).
- **Operations.** Multi-node session forwarding over the cluster listener, Prometheus metrics, OTLP traces, quotas with token buckets, a per-app logical backup (snapshot plus journal export) with restore into a new app, and a load harness with stored baselines (§45 §9–§12, §15).

**Tech Stack:**
- **Rust:** 1.97.1, edition 2024, workspace lints.
- **New Rust dependencies** (Task 0 checks versions, licences and that they build together):
  - `rquickjs` 0.14 (MIT), features `futures`, `loader`, `macro`, `array-buffer` (R1 row R13);
  - `seccompiler` (Apache-2.0 OR BSD-3-Clause) and `landlock` (MIT OR Apache-2.0), Linux only, for the worker sandbox;
  - `prometheus-client` (Apache-2.0), only if the engine has no metrics registry by Task 29 (Task 0 checks);
  - `opentelemetry` and `opentelemetry-otlp` (Apache-2.0), only if M2 has not added them (Task 0 checks).
- **Reused Rust dependencies:** `redb` 4, `tikv-client` (the fork pin), `connectrpc` 0.9, `buffa` 0.9, `axum` 0.8, `rustls` 0.23, `jsonwebtoken`, `openidconnect` 4 (with MT1), `sha2`, `hmac`, `ulid`, `proptest`, `tokio`, `tracing`.
- **TypeScript:** the workspace's toolchain (TypeScript 5.9.3, Node ≥ 22, pnpm, Biome, Vitest), `@bufbuild/protobuf` 2, `@connectrpc/connect` 2, `@connectrpc/connect-web` 2, React 19 (peer) with `@testing-library/react`, and `esbuild` (MIT) for `@loams/live-cli`. Exact pins only; every new npm dependency at least 14 days old.
- **Cluster:** `tiup playground v8.5.8` as in R1 (`scripts/tikv/playground.sh`, `--tag loams-<purpose> --port-offset 17000`), `toxiproxy` for the nemesis, and the BR binary of v8.5.8 for Task 30.

**Spec:**
- [§45](../design/45-loams-live-production.md): all of it.
- [§20](../design/20-reactive-database-on-tikv.md): §4–§9, §12–§14.
- [§44](../design/44-unified-api-and-sdks.md): §4, §7.2–§7.4, §10.3–§10.4.
- [§19](../design/19-console-identity-and-agents.md) §5.
- [§38](../design/38-knative-authentik-gitops.md) §4.
- [§10](../design/10-operations.md) §3–§5.
- [Decision log](../design/13-decision-log.md): D66, D88, D111, D116–D131, D260, D600–D611, D670, D680–D699; Q31, Q35–Q37, Q625–Q639.
- As built: [R1](2026-09-27-r1-reactive-core.md) (Tasks 1–12 and their rulings rows), [MT1](2026-10-02-mt1-authentik-identity.md) (Tasks 2, 3, 7), [AP1e](2026-10-08-ap1e-electron-desktop.md) (Tasks 20, 24), [API1](2026-10-02-api1-unified-connect.md).

## Global Constraints

- **Worktree.** `~/Documents/Ostriumlabs/loams-wt/lv1-<milestone>` (for example `lv1c-embedded-store`). Never commit in the main checkout. `git commit -s` (DCO).
- **Commit areas:** `kv`, `live`, `tikv`, `auth`, `api`, `sdk`, `deploy`, `ci`, `docs`.
- **The build machine.** One cargo build at a time, the shared target directory from `~/Documents/.cargo/config.toml`. Never set `CARGO_TARGET_DIR`, never build in `/tmp`. Build with `-p <crate>` where possible. The TiKV playground (about 3.2 GB RSS) runs only when no cargo build runs.
- **Cluster tests skip without a cluster.** As in R1: `loams_tikv::testing::cluster()` returns `None` and prints `skipped: <test> needs LOAMS_TEST_PD` when the variable is unset. **Every Live test that does not exercise a TiKV-only behaviour runs on both backends**, through `loams_kv::testing::stores()`, which yields the embedded store always and the TiKV store when `LOAMS_TEST_PD` is set. So a plain `cargo test` covers the embedded backend on every PR.
- **No forks of PD or TiKV** (D126). `tikv-client` stays the pinned fork (R1 row F1).
- **Loopback until Task 18.** Until Task 18 merges, every Live entry point keeps R1's refusal of non-loopback addresses. Task 18 lifts it only together with TLS and auth.
- **No secrets and no user data in logs.** Tokens, API keys, document values, arguments and results never reach a log line, a span attribute, a metric label or an error message. Task 27's canary test enforces this.
- **Proto changes** go to `proto/loams/live/v1/` and keep `unstable: true` until Task 40. `buf lint` passes on every change. Regenerate the TypeScript stubs with `pnpm --filter @loams/live generate`.
- **Do not edit** `apps/desktop-electron/` or `web/` (owned by AP1e). Where Live changes what the desktop does, the task records the follow-up in "Rulings made during execution" for the desktop owner.
- **Determinism in functions** (§20 §6.2) is never relaxed.

## Rulings made while writing this plan

| # | Ruling | Why | Cost if wrong |
|---|---|---|---|
| 1 | **The seam is an enum, not a trait object.** `loams_kv::Store` is `enum { Embedded, Tikv }` with inherent async methods, and `Txn` and `Snap` the same | No `async-trait` boxing on every read; `loams-live`'s code changes only in imports | Adding a third backend touches three enums |
| 2 | **`loams-tikv` keeps its public API.** `loams-kv`'s tikv backend wraps it, and the tuple codec and `Ts` move to `loams-kv`, re-exported from `loams-tikv` for `loams-meta-tikv` | The metastore crate stays untouched | Two names for one codec until a cleanup |
| 3 | **`Ts` is a `u64` in TSO layout** (physical ms << 18 \| logical) for both backends; `tikv_client::Timestamp` appears only inside the tikv backend | Version numbers stay comparable and identical on the wire (`StateVersion.ts`, `commit_ts`) | None |
| 4 | **One redb file per data directory** at `<data_dir>/live/store.redb`, keyspaces as a `u32` id prefix inside it, the id map in a `keyspaces` table | One file lock, one fsync stream, one GC | One app's write load delays another's commits in dev; irrelevant there |
| 5 | **The worker protocol is `loams.live.worker.v1`** (internal, not in the public API catalogue), length-prefixed protobuf frames over the child's stdin and stdout; stderr carries the worker's own logs | No sockets in the sandbox at all | A new internal proto to version; it never leaves one binary |
| 6 | **The admin service is a separate proto file** `proto/loams/live/v1/admin.proto` in the same package | The facade maps it to `loams.live.admin`; authorization can be applied per service | None |
| 7 | **Backups use protobuf segments** (`loams.live.v1.BackupSegment`), zstd-compressed, not Parquet | Lossless `LiveValue` round trip, no schema per table | External tools cannot read backups directly; an export to Parquet is later work |
| 8 | **Quota buckets are per node** (§45 §12) | No coordination on the hot path | The effective limit scales with node count; documented |
| 9 | **The lifetime check uses a fresh TSO before prewrite**, not the local clock | Both backends have a timestamp oracle; clocks drift | One more TSO fetch per mutation (batched by the client, about 0.1 ms) |

## Milestones and order

| Milestone | Tasks | Scope | Runs |
|---|---|---|---|
| — | 0 | Reconcile with the code as built | First |
| **LV1c** — embedded store | 20–23 (4) | `loams-kv`, the embedded MVCC backend, parity suites, `--live-store` and the feature split | **Second**: every later test runs on embedded per PR, and the refactor is cheapest before more code is written against `loams_tikv` types |
| **LV1a** — gates, functions, deploy, schema | 1–11 (11) | The reactive and transaction checkers, `loams-live-js`, the isolated worker, the function API, deployments and rollback, validators, online index changes, catalog entries, `LiveAdminService` (deploy half), function logs | Third |
| **LV1b** — apps, exposure, auth | 12–19 (8) | The main port, the app directory and lifecycle, the verifier, authorization, end-user OIDC, session identity, TLS and CORS, error scrubbing | Fourth; Task 14 depends on `loams-auth` (see Task 0) |
| **LV1d** — HA and operations | 24–33 (10) | Multi-node sessions, the nemesis, metrics, traces and logs, quotas, backup export, restore, the BR spike, the reference deployment, the performance harness | Fifth; Tasks 27–28 may start after Task 13 |
| **LV1e** — SDKs, docs, security, release | 34–40 (7) | `@loams/live`, the fixtures, `@loams/live-react`, `@loams/live-cli`, docs, the threat model and fuzzing, the exit report and the release gate | Last; Tasks 34–36 may start after Task 17 |

41 tasks in all (0–40).

## Review Focus

1. **No missed update across catalog changes.** Deploys, rollbacks, index backfills and identity changes in the middle of a workload never produce a pushed result that differs from a fresh snapshot evaluation. Tests: Task 1 (`reactive_checker_*`), Task 9 (`concurrent_inserts_during_backfill_are_indexed`, `checker_passes_with_index_changes_mid_workload`), Task 7 (`activation_reruns_every_subscription_once`).
2. **Tenant isolation.** An environment-scoped credential never reaches another app; a cursor never crosses apps; a cached result never crosses identities. Tests: Task 15 (`authz_matrix` and its blessed table), Task 4 (`forged_cursor_rejected`, `cursor_from_other_app_rejected`), Task 17 (`results_never_shared_across_identities`).
3. **The sandbox.** `isolated` workers cannot open files, sockets or processes, and a crash is contained. Tests: Task 5 (`worker_cannot_open_files`, `worker_cannot_open_sockets`, `worker_cannot_exec`, `worker_crash_is_function_error_and_respawns`).
4. **Backend parity.** The embedded store gives TiKV's transaction semantics. Tests: Task 21 (`kv_conformance!`), Task 22 (`every_live_suite_runs_on_embedded`), Tasks 1–2 (both checkers run on both backends through `stores()`).
5. **Backup consistency.** A restore equals the source at the restore tick. Tests: Task 30 (`restore_matches_source_snapshot_at_tick`, `pitr_to_middle_tick`).
6. **No leakage.** No token, document value or raw TiKV key appears in logs, spans, metrics or errors. Tests: Task 19 (`tikv_errors_never_leak_raw_keys`), Task 27 (`canary_never_logged`).

## File structure

```
Cargo.toml                                    # + members loams-kv, loams-live-js; rquickjs, seccompiler, landlock (+ metrics/otel if absent)
deny.toml                                     # licences for the new crates, if needed (Task 0)
proto/loams/live/v1/
  live.proto                                  # Authenticate; Deploy removed (Task 10)
  admin.proto                                 # LiveAdminService (Tasks 10, 13, 29, 30)
  catalog.proto                               # DeploymentRecord, IndexState, Validator, AppRecord
  journal.proto                               # JournalEntry.catalog (Task 11)
  backup.proto                                # BackupSegment, BackupManifest (Task 29)
proto/loams/live/worker/v1/worker.proto       # internal worker protocol (Task 5)
crates/loams-kv/                              # new (LV1c)
  src/{lib.rs,ts.rs,store.rs,txn.rs,runner.rs,faults.rs,codec.rs,gc.rs,testing.rs,conformance.rs}
  src/embedded/{mod.rs,oracle.rs,mvcc.rs,commit.rs,gc.rs}
  src/tikv.rs                                 # feature `tikv`
  tests/{conformance.rs,embedded.rs,crash.rs}
crates/loams-tikv/src/{codec.rs,lib.rs}       # codec re-exported from loams-kv (Ruling 2)
crates/loams-live/
  src/{deploy.rs,schema.rs,validate.rs,backfill.rs,apps.rs,engines.rs,auth.rs,authz.rs,
       quota.rs,metrics.rs,backup.rs,restore.rs,forward.rs,admin.rs,cursor.rs,logs.rs}
  src/testing/{mod.rs,workload.rs,checker.rs}
  tests/{reactive_checker.rs,txn_checker.rs,deploy.rs,schema.rs,backfill.rs,apps.rs,authz.rs,
         authz_matrix.expected.md,identity.rs,quota.rs,backup.rs,multinode.rs,observability.rs}
crates/loams-live-js/                         # new (LV1a)
  src/{lib.rs,runtime.rs,prelude.js,host.rs,limits.rs,validators.rs,worker.rs,sandbox_linux.rs,ipc.rs}
  tests/{functions.rs,limits.rs,determinism.rs,validators.rs,worker.rs,sandbox.rs}
crates/loams-sim/src/elle.rs                  # list-append cycle checker (Task 2)
crates/loams-auth/src/{lib.rs,verify.rs,keys.rs,jwks.rs,revocation.rs}   # only if absent at Task 0 (Task 14)
crates/loams/
  Cargo.toml                                  # features live (embedded), live-tikv
  src/{main.rs,server.rs}                     # --live-store, live-worker subcommand, live deploy
  src/api/{connect.rs,live.rs}                # LiveService and LiveAdminService on the main port
scripts/tikv/{nemesis.sh,playground.sh}       # 3 PD / 3 TiKV / 2 Live topology (Task 25)
deploy/live/{compose.yaml,alerts.yaml,helm-values.yaml,README.md}
bench/live/{Cargo.toml,src/main.rs,scenarios/*.toml,baseline.json,compare.py}
sdks/fixtures/live/*.json                     # session fixtures (Task 34)
sdks/typescript/packages/live/src/{index.ts,client.ts,session.ts,optimistic.ts,values.ts,auth.ts,paginate.ts}
sdks/typescript/packages/live/test/{session.test.ts,values.test.ts,auth.test.ts,fixtures.test.ts,live.test.ts}
sdks/typescript/packages/live-react/          # new (Task 35)
sdks/typescript/packages/live-cli/            # new (Task 36)
docs/guides/live/{quickstart,functions,schema,auth,deploy,operations,backup,limits}.md
docs/security/live-threat-model.md
docs/plans/lv1-exit-report.md
docs/design/{20-reactive-database-on-tikv.md,45-loams-live-production.md,13-decision-log.md}  CHANGELOG.md  docs/plans/README.md
.github/workflows/{ci.yml,live-nightly.yml}
```

---

### Task 0: Reconcile with the code as built

**Files:** read:
- `crates/loams-live/`, `crates/loams-tikv/`, `crates/loams-live-proto/`, `proto/loams/live/v1/`;
- `crates/loams/src/{main.rs,server.rs,api/connect.rs}`, `crates/loams/Cargo.toml`;
- `sdks/typescript/packages/live/`;
- `crates/loams-auth/` (if present);
- the admin listener and metrics code (if present).

Write the reconciliation into this plan's "Rulings made during execution" (`T0-1`…).

**Consumes** (each checked against `dev`; every difference is listed with its resolution):
- `loams_live::{Runner, LiveTxn, Function, FnKind, SubKey, Subscriptions, Sessions, LiveServer, LiveConfig, Limits, AppKeys}` as R1 Tasks 8–12 built them;
- `Runner::try_quiesce` (row T9-1), which Task 9 retires;
- `loams_tikv::{Tikv, Txn, Snap, TxnOptions, TxnError, CommitMode, FaultPlan, GcBarrier, tuple}`;
- `LiveAbsent` and `CATALOGUE` in `connect.rs`;
- `LiveArgs` (`--live-listen`, `--live-pd`, `--live-keyspace`, `--live-app`, `--live-tick-read-lag-ms`, `--no-live`).

**Produces:** a short findings note covering:
1. **R1 Tasks 13, 14, 16 and 17 are unbuilt.** Check that `deploy.rs` answers `UNIMPLEMENTED`, there is no `loams-live-js`, and the TypeScript package is generated-only.
2. **Whether AP1e Task 20 (`_system:tables`) has merged.** If not, Task 12 adds it with AP1e's contract.
3. **Whether `loams-auth` exists.** That means MT1 Task 2 or the M2 identity work. Record the as-built `Verifier` and `KeyStore` surface. If absent, Task 14 builds the verification core with the interfaces in Task 14.
4. **Whether the engine has a metrics registry, an admin listener, and OTLP export** (§10 §5). If not, Tasks 27–28 add the minimum.
5. **Whether the main port has TLS and an auth layer** (MT1 Task 7). If not, Task 18 adds TLS config for the main port, gated exactly as MT1 Task 7 specifies.
6. **Naming drift.** Doc comments and log lines say "Loam Live", and `--live-keyspace`'s help says `loam_live_<app>` while the prefix is `loams_live_`. Fix them in Task 23.
7. **Versions and licences** of the new dependencies (Tech Stack); a throwaway build of `rquickjs`, `seccompiler` and `landlock` together.
8. **The owner's answers to Q625, Q626, Q629, Q631 and Q639,** if given. Otherwise the §45 defaults stand.

**Tests:** none (a reading task). **Commit:** `docs: LV1 task 0 findings`.

---

## LV1c — The embedded store (runs second)

### Task 20: The `loams-kv` seam

**Files:**
- new: `crates/loams-kv/{Cargo.toml,src/{lib.rs,ts.rs,store.rs,txn.rs,runner.rs,faults.rs,codec.rs,gc.rs,testing.rs,tikv.rs}}`;
- changed: `crates/loams-tikv/src/{codec.rs,lib.rs}`; every `crates/loams-live/src/*.rs` that names a `loams_tikv` type; `Cargo.toml`.

**Consumes:** `loams_tikv::{Tikv, TikvConfig, Txn, Snap, TxnOptions, TxnError, Committed, CommitMode, FaultPlan, FaultPoint, GcBarrier, tuple}`.

**Produces:**

```rust
// loams-kv
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ts(pub u64);                                   // Ruling 3
impl Ts { pub fn physical_ms(self) -> u64; pub fn from_parts(ms: u64, logical: u32) -> Self; }
pub enum StoreConfig { Embedded(EmbeddedConfig), #[cfg(feature = "tikv")] Tikv(loams_tikv::TikvConfig) }
pub struct EmbeddedConfig { pub path: PathBuf, pub keyspace: String, pub root: Vec<u8>, pub gc_life_time: Duration }
#[derive(Clone)] pub enum Store { Embedded(embedded::Handle), #[cfg(feature = "tikv")] Tikv(loams_tikv::Tikv) }
impl Store {
    pub async fn open(config: StoreConfig) -> Result<Self, KvError>;
    pub async fn now(&self) -> Result<Ts, KvError>;
    pub async fn snapshot(&self, at: Ts) -> Result<Snap, KvError>;
    pub async fn run<T: Send, F>(&self, opts: TxnOptions, body: F) -> Result<Committed<T>, TxnError>
        where F: for<'t> FnMut(&'t mut Txn) -> BoxFuture<'t, Result<T, TxnError>>;
    pub fn keyspace(&self) -> &str;  pub fn root(&self) -> &[u8];  pub fn key(&self, suffix: &[u8]) -> Vec<u8>;
    pub fn with_faults(self, plan: Arc<dyn FaultPlan>) -> Self;
    pub async fn barrier(&self, name: &str, at: Ts, ttl: Duration) -> Result<GcBarrier, KvError>;
    pub fn backend(&self) -> Backend;                     // Embedded | Tikv
}
pub enum Txn { .. }   // get, batch_get, scan, scan_reverse, put, insert, delete, lock_keys, start_ts() -> Ts, attempt()
pub enum Snap { .. }  // ts() -> Ts, get, batch_get, scan, scan_reverse
pub use codec::tuple;                                       // moved from loams-tikv (Ruling 2)
pub mod testing { pub async fn stores(name: &str) -> Vec<Store>; }  // embedded always, tikv with LOAMS_TEST_PD
```

The embedded variant is a stub in this task: `open` returns `KvError::Unsupported("embedded backend arrives in Task 21")`, and `testing::stores()` yields only TiKV until Task 21.

**Tests:**
- `loams-kv/tests/conformance.rs::tikv_store_roundtrip`: the tikv variant's put, get and scan behave as `loams_tikv::Tikv` (skips without PD).
- `ts_parts_roundtrip`: `from_parts` and `physical_ms` invert.
- `tuple_codec_reexport_is_identical`: bytes from `loams_tikv::tuple` and `loams_kv::tuple` are equal for a proptest of values.
- **Every existing `loams-live` test passes unchanged in behaviour** on the tikv variant. This is the refactor's gate.

**Steps:**
1. Write the three new tests and run them (they fail: no crate).
2. Create `loams-kv`, move the codec and `Ts`, and wrap `loams_tikv`.
3. Port `loams-live` imports from `loams_tikv` to `loams_kv`; `Runner::open` takes `Store`.
4. Run `cargo test -p loams-kv -p loams-live`, then the TiKV job with PD.
5. Commit.

**Commit:** `kv: add the loams-kv store seam over loams-tikv`; `live: depend on loams-kv instead of loams-tikv`.

### Task 21: The embedded MVCC backend on redb

**Files:** `crates/loams-kv/src/embedded/{mod.rs,oracle.rs,mvcc.rs,commit.rs,gc.rs}`, `src/conformance.rs`, `tests/{embedded.rs,crash.rs,conformance.rs}`; and, moved from Task 23 by ruling T20-9, `crates/loams-live/src/{config.rs,service.rs}`, `crates/loams-live/tests/{service.rs,session.rs}`, `crates/loams/src/{main.rs,server.rs}` and `crates/loams/tests/live.rs` (every builder of a `LiveConfig`).

**Consumes:** Task 20's enums; `redb` 4.

**Produces:**
- `embedded::Handle`, a store at `EmbeddedConfig.path` with Ruling 4's layout:
  - table `versions`: `keyspace_id:u32 BE ‖ root ‖ key ‖ (u64::MAX − commit_ts) BE → [0x00 ‖ value] | [0x01]` (a tombstone);
  - table `keyspaces`: `name → id`;
  - table `oracle`: `high_water`.
- **Oracle:** physical ms from the wall clock, never going back; 18 logical bits; the persisted high-water mark advanced by 1 000 allocations or 1 s; a restart starts above it.
- **Commit:** buffered writes and lock keys. Under the store's commit mutex, conflict if any written or locked key has a version with `commit_ts > start_ts`. Group commit drains the waiting commits into one redb write transaction (one fsync).
- **GC:** deletes versions older than the newest version ≤ `safe_point`, where `safe_point = min(now − gc_life_time, oldest open snapshot, barriers)`. It runs every `gc_interval` (60 s).
- `loams_kv::conformance::kv_conformance!`, a macro that expands one `#[tokio::test]` per case for a `Store` factory.
- **(T20-9, from Task 23)** `LiveConfig.store: StoreConfig` replaces `LiveConfig.tikv`; `LiveConfig::with_store(app, StoreConfig)` beside `LiveConfig::with_tikv(app, TikvConfig)` (kept, it wraps `StoreConfig::Tikv`). `LiveServer::start` opens whatever store the config names, and `crates/loams` starts its cluster GC loop only for a TiKV store (`Store::as_tikv`). The CLI flags stay as they are until Task 23.

**Tests** (`kv_conformance!` cases run on both backends):
- `snapshot_ignores_later_commits`: a write committed after a snapshot's ts is invisible to it.
- `first_committer_wins`: two transactions writing one key, started together; exactly one commits, and the other gets `TxnError::Conflict`.
- `locked_read_conflicts_with_write`: `lock_keys(k)` in T1 and a write of `k` in T2 that commits first make T1 conflict.
- `scan_bounds_and_order`: forward and reverse scans over `[lo, hi)` return keys in order, respect the limit, and skip tombstones.
- `insert_existing_key_fails`: `insert` of a key visible at start fails with the same `TxnError` class as TiKV's `AlreadyExist`.
- `roots_are_isolated`: two roots in one keyspace never see each other's keys.

Embedded-only tests:
- `ts_monotonic_across_restart`: reopen the file; the first `now()` exceeds every timestamp issued before.
- `crash_mid_commit_is_atomic` (`tests/crash.rs`): a child process commits 1 000 multi-key transactions and is `SIGKILL`ed at a random point; after reopening, every transaction is either fully visible or absent.
- `gc_respects_barrier`: a barrier at `t` keeps a snapshot at `t` readable after GC.
- `model_check_against_reference` (proptest, 256 cases): random concurrent transactions against an in-memory reference SI model give identical commit and conflict outcomes and reads.
- `group_commit_batches_fsyncs`: 100 concurrent commits take fewer than 100 redb write transactions.

**Steps:** tests first (they fail against the stub), then implement the oracle, then MVCC reads, then commit, then GC. Run `cargo test -p loams-kv`.

**Commit:** `kv: an embedded MVCC store on redb with snapshot isolation`.

### Task 22: Parity suites on both backends

**Files:** `crates/loams-live/tests/*.rs` (parameterized over `loams_kv::testing::stores()`), `crates/loams-live/src/testing/mod.rs`, `.github/workflows/ci.yml`.

**Consumes:** Tasks 20–21.

**Produces:** a `live_test!(name, |store| async { … })` macro that runs one body per backend and names the case `<name>::embedded` or `<name>::tikv`. The existing suites (`docs`, `journal`, `txn`, `subs`, `session`, `service`, `value`) are moved onto it. A CI job `live-embedded` runs `cargo test -p loams-kv -p loams-live` with no PD on every PR that touches `crates/loams-kv`, `crates/loams-live*` or `proto/loams/live`.

**Tests:**
- `every_live_suite_runs_on_embedded`: a meta-test lists the `#[test]` names and asserts each has an `::embedded` variant, except an allowlist of TiKV-only tests (TSO-stream loss, region errors, PD stall), each with a comment saying why.
- The full existing suites, on embedded.

**Steps:** write the macro and the meta-test (it fails), migrate the suites, fix any parity gap by changing **the embedded backend**, never the test, unless the test asserted a TiKV-only detail (record a ruling).

**Commit:** `live: run the Live suites on the embedded and TiKV backends`.

### Task 23: `--live-store`, the feature split and dev defaults

**Files:** `crates/loams/{Cargo.toml,src/main.rs,src/server.rs}`, `crates/loams/tests/live_dev.rs`, `crates/loams-live/src/config.rs` (only the default store and the naming fixes: `LiveConfig.store` arrives in Task 21, ruling T20-9), `docs/build-from-source/` (the feature list), `CHANGELOG.md`.

**Consumes:** `LiveArgs`, `LiveRuntime`, `ServerError::Live*`.

**Prerequisites** (Task 21 review, minor items 3 and 4, to land before the embedded store becomes the default):
- **Incremental embedded GC.** A GC round works in bounded key ranges, one redb write transaction each, with a resume cursor between them. It never collects an unbounded list of doomed versions, and it never holds the write lock for a whole-table scan.
- **Scans skip older versions.** An embedded scan re-seeks past a key's older versions instead of stepping through them. A large scan runs in `spawn_blocking` (row T21-12) rather than inline on a runtime worker.
- Tests: GC of a store with more versions than one range takes several write transactions and finishes; a scan over keys with many versions reads each key's newest visible one; neither blocks a concurrent commit for a whole-table pass.

**Produces:**
- Cargo features: `live = ["dep:loams-live", "loams-kv/embedded"]`, **in `default`**; `live-tikv = ["live", "tikv", "loams-kv/tikv"]`. The `full` variant enables `live-tikv`.
- The flag `--live-store <embedded | tikv://<pd>[,<pd>]/<keyspace>>`, default `embedded`, stored under `<data_dir>/live/`. `--live-pd` and `--live-keyspace` stay one release as aliases that imply `tikv://` and print a deprecation line.
- `--no-live` unchanged.
- `LiveConfig.store` (from Task 21, T20-9) defaults to `StoreConfig::Embedded` under `<data_dir>/live/store.redb`.
- The naming fixes of Task 0 item 6.
- **Desktop follow-up** (recorded, not done here): AP1e's engine args can drop the TiKV-gated `--no-live` (D692 amends D670).

**Tests:**
- `dev_starts_live_without_pd`: spawn `loams dev --listen 127.0.0.1:0` with a temp data dir; `GetInstance` and a `_system:insert` then `_system:get` over the Live listener succeed with no PD running.
- `live_store_tikv_requires_feature`: built without `live-tikv`, `--live-store tikv://…` fails with `--live-store tikv:// needs a build with the live-tikv feature`.
- `live_pd_alias_maps_to_tikv_store`.
- `data_survives_restart_on_embedded`: insert, stop, restart, get.
- `default_build_has_no_tikv_client`: `cargo tree -p loams -e normal` with default features contains no `tikv-client` (a CI script assertion).

**Steps:** tests first; then the features, the flag and the defaults; then run `cargo test -p loams --test live_dev`.

**Commit:** `live: run Live on the embedded store by default; --live-store selects TiKV`.

---

## LV1a — Gates, functions, deploy and schema

### Task 1: The reactive correctness checker

**Files:** `crates/loams-live/src/testing/{workload.rs,checker.rs}`, `crates/loams-live/tests/reactive_checker.rs`.

**Consumes:** `Runner`, `Subscriptions`, `Sessions`, `ClientState`, `Subscriptions::drop_next_batch` (the existing test hook), `loams_kv::testing::stores()`.

**Produces:**

```rust
pub struct Workload { pub seed: u64, pub sessions: usize, pub tables: usize, pub ops: usize, pub disturb: Vec<Disturbance> }
pub enum Disturbance { Deploy { at_op: usize }, Rollback { at_op: usize }, AddIndex { at_op: usize }, DropIndex { at_op: usize },
                       IdentityChange { at_op: usize }, NodeKill { at_op: usize } }   // variants wired by Tasks 7, 9, 17, 24
pub struct Report { pub transitions: usize, pub checked: usize, pub violations: Vec<Violation> }
pub async fn run_reactive_checker(store: Store, w: Workload) -> Report;
```

Checks (§20 §14 item 3):
- every Transition's updated query equals a fresh snapshot evaluation at the Transition's `end.ts`;
- versions strictly increase per session;
- every committed mutation that touches a subscribed range is reflected by the first tick at or after its commit;
- a resumed session converges.

**Tests:**
- `reactive_checker_passes_seeded_workload` (10 seeds × 2 000 ops, both backends).
- `reactive_checker_detects_missed_invalidation`: with `drop_next_batch` armed, the checker reports at least one violation (the checker is not vacuous).
- `reactive_checker_resume_converges`: sessions disconnected mid-run resume and match.

**Steps:** write the tests against a stub `run_reactive_checker` (fail), implement the workload and the checker, run on embedded, then on TiKV.

**Commit:** `live: the reactive correctness checker`.

### Task 2: The transaction checker

**Files:** `crates/loams-sim/src/elle.rs`, `crates/loams-live/tests/txn_checker.rs`.

**Consumes:** `Runner::mutate` with `_system:*` functions; `RunnerOptions::serializable_ranges`.

**Produces:**
- `loams_sim::elle::{History, Op, check_list_append(&History) -> Result<(), Anomaly>}`: dependency-graph cycle search for G0, G1c and G-single, plus G2 reported separately;
- a list-append workload over Live documents.

**Tests:**
- `elle_detects_known_g_single` and `elle_detects_known_g2_write_skew`: fixed bad histories are flagged.
- `elle_accepts_known_si_history`.
- `live_histories_are_snapshot_isolated` (both backends, 5 seeds).
- `point_read_promotion_prevents_write_skew`: the `db.get`-based write-skew workload shows no G2.
- `range_write_skew_is_possible_without_serializable_ranges`: documents Q31. The test asserts that the anomaly **can** occur with the option off (so the docs stay honest) and never occurs with it on.

**Steps:** tests first with the fixed histories, then the checker, then the workload.

**Commit:** `sim: an Elle-style list-append checker`; `live: the transaction checker`.

### Task 3: `loams-live-js`: the QuickJS runtime (R1 Task 13, part 1)

**Files:** `crates/loams-live-js/{Cargo.toml,src/{lib.rs,runtime.rs,prelude.js,limits.rs}}`, `tests/{functions.rs,limits.rs,determinism.rs}`.

**Consumes:** `loams_live::{Function, FnKind, LiveTxn, LiveValue, LiveError, Limits}`.

**Produces** (R1 Task 13's contract, verbatim where it exists):

```rust
pub struct JsConfig { pub memory_limit: usize /* 64 MiB */, pub cpu_limit: Duration /* 1 s */, pub contexts: usize /* 4 */,
                      pub console_lines: usize /* 64 */, pub console_line_bytes: usize /* 4 KiB */ }
pub struct Bundle;   // one ES module; exports from "loams:server"
impl Bundle { pub fn load(source: &str, config: JsConfig) -> Result<Self, LiveError>;
              pub fn functions(&self) -> Vec<FunctionMeta>;          // path, kind, visibility, args validator
              pub fn function(&self, path: &str) -> Option<Arc<dyn Function>>; }
pub struct FunctionMeta { pub path: String, pub kind: FnKind, pub visibility: Visibility, pub args: Option<Validator> }
pub enum Visibility { Public, Internal }
```

Semantics as R1 Task 13 items 1–3:
- the determinism rules (`Date.now` is the start ts; `Math.random` is a seeded PRNG; `crypto.*` throws `DeterminismError`; no `setTimeout`, `fetch` or `WebAssembly`);
- one context per invocation;
- frozen globals;
- the CPU interrupt (`FUNCTION_TIMEOUT`) and the memory limit (`FUNCTION_OUT_OF_MEMORY`).

`console.*` output is collected per call into `CallOutput.logs` (truncated per D682).

**Tests** (R1's names):
- `functions.rs`: `query_and_mutation_run_and_record_read_sets`, `mutation_rerun_on_conflict_is_invisible_to_the_caller`, `unknown_function_is_not_found`.
- `limits.rs`: `busy_loop_times_out`, `allocation_bomb_hits_memory_limit`, `context_recovers_after_timeout`, `console_output_truncated_at_limits`.
- `determinism.rs`: `module_state_does_not_leak_between_calls`, `crypto_random_throws_in_queries_and_mutations`, `date_now_is_start_ts`, `random_is_repeatable_for_same_ts_and_request`, `no_fetch_no_timers`.

**Steps:** tests first; add `rquickjs` to the workspace; implement the runtime, then the prelude, then the limits.

**Commit:** `live: run query and mutation functions in QuickJS`.

### Task 4: The host API: `ctx.db`, pagination, argument validators, visibility

**Files:** `crates/loams-live-js/src/{host.rs,validators.rs}`, `crates/loams-live/src/{cursor.rs,validate.rs}`, `tests/validators.rs`, `crates/loams-live/tests/cursor.rs`.

**Consumes:** Task 3; `LiveTxn::{get, query, insert, patch, replace, delete}`.

**Produces:**
- `ctx.db.get/query/insert/patch/replace/delete`, `withIndex(name, q => q.eq().gt().gte().lt().lte())`, `order`, `take`, `first`, `collect`, `paginate({ cursor, numItems })` returning `{ page, continueCursor, isDone }`;
- `loams_live::cursor::{encode(app_key, range_end) -> String, decode(app_key, &str) -> Result<KeyBound, LiveError>}`: base64url of `version:u8 ‖ index_id ‖ last_key ‖ HMAC-SHA256(app_key)[..16]`; the per-app key is a catalog record created with the app;
- `loams_live::validate::{Validator, check(&Validator, &LiveValue) -> Result<(), Vec<FieldViolation>>}`, shared by argument and schema validators (`v.null|int64|float64|boolean|string|bytes|array|object|literal|union|optional|any|id(table)`);
- `Function` gains `fn visibility(&self) -> Visibility` and receives a `CallCtx { identity: Option<Identity>, request_id, deployment }`. `Identity` is defined in Task 16; until then `None`.

**Tests:**
- `paginate_returns_pages_and_is_done`;
- `paginate_read_set_ends_at_last_key` (an insert after the page's last key does not invalidate the page subscription);
- `forged_cursor_rejected` (one bit flipped → `INVALID_ARGUMENT`, reason `live_bad_cursor`);
- `cursor_from_other_app_rejected`;
- `args_validator_rejects_before_handler_runs` (the handler's side counter is unchanged);
- `validator_vocabulary_roundtrip` (proptest: values generated from a validator pass it; mutated values fail with the right path);
- `internal_function_metadata_is_internal`.

**Steps:** tests first; the validator, then the cursor, then the host bindings.

**Commit:** `live: the function host API with pagination and argument validators`.

### Task 5: The isolated worker

**Files:** `proto/loams/live/worker/v1/worker.proto`, `crates/loams-live-js/src/{worker.rs,ipc.rs,sandbox_linux.rs}`, `crates/loams/src/main.rs` (hidden subcommand `live-worker`), `crates/loams-live-js/tests/{worker.rs,sandbox.rs}`.

**Consumes:** Tasks 3–4.

**Produces:**
- `enum Isolation { InProcess, Isolated }` in `JsConfig`;
- `WorkerPool::spawn(app, bundle_bytes, config) -> WorkerHandle` (one worker per (node, app, deployment));
- the protocol (Ruling 5): `Load{bundle}`, `Invoke{path, args, ctx}` → zero or more `HostCall{id, op, …}` / `HostReply{id, …}` → `Done{result | error, logs, cpu_ns}`.
- **Linux sandbox**, applied after `Load`:
  - `PR_SET_NO_NEW_PRIVS`;
  - a landlock ruleset with no filesystem access;
  - a seccomp allowlist (read and write on fds 0–2, `futex`, `mmap`, `munmap`, `mprotect`, `brk`, `clock_gettime`, `exit_group`, `rt_sigreturn`, `sched_yield`, `madvise`), anything else `SECCOMP_RET_KILL_PROCESS`;
  - `RLIMIT_AS` = memory limit + 64 MiB;
  - `RLIMIT_NOFILE` = 3.
- `[live] isolation = "in_process" | "isolated"` and `[live] tenancy = "single" | "multi"`. `multi` with `in_process`, or `multi` on a non-Linux OS, refuses to start: `live: tenancy = "multi" needs isolation = "isolated" (Linux only)`.

**Tests:**
- `functions_suite_runs_isolated`: every Task 3 and 4 test, parameterized, also passes with `Isolated`.
- `worker_cannot_open_files`: a test-only host hook asks the worker to `open("/etc/passwd")`; the worker dies with `SIGSYS` and the call returns `FUNCTION_ERROR`, reason `live_worker_crashed`.
- `worker_cannot_open_sockets`.
- `worker_cannot_exec`.
- `worker_crash_is_function_error_and_respawns`: the next call succeeds on a fresh worker.
- `five_crashes_in_a_minute_degrade_the_deployment`.
- `multi_tenancy_requires_isolation`: config validation.
- `host_call_latency_recorded`: a bench-style test that records the p50 IPC round trip into the test output; not a gate, but it feeds P8.

**Steps:** tests first (the Linux-only tests are `#[cfg(target_os = "linux")]`), then the protocol, then the worker loop, then the sandbox.

**Commit:** `live: run functions in a sandboxed worker process`.

### Task 6: Deployments: records, storage and activation

**Files:** `crates/loams-live/src/{deploy.rs,schema.rs}`, `proto/loams/live/v1/catalog.proto`, `crates/loams-live/tests/deploy.rs`.

**Consumes:** `loams-store` (bundles at `live/<app>/deployments/<id>.js`, R1 Ruling 10); `catalog.rs`; Tasks 3–5.

**Produces:**

```rust
pub struct DeploymentRecord { pub id: Ulid, pub bundle_sha256: [u8; 32], pub schema: Schema, pub created_at_ms: u64,
                              pub created_by: String, pub message: String, pub parent: Option<Ulid>, pub state: DeploymentState }
pub enum DeploymentState { Pending, Validating, Backfilling, Active, Superseded, Failed { reason: String }, Degraded }
pub struct DeployRequest { pub bundle: Vec<u8>, pub schema: Schema, pub expected_active: Option<Ulid>, pub message: String, pub dry_run: bool }
pub async fn deploy(app: &AppEngine, req: DeployRequest, by: &Principal) -> Result<DeployOutcome, LiveError>;
pub async fn list_deployments(app: &AppEngine, page: PageRequest) -> Result<Page<DeploymentRecord>, LiveError>;
pub fn resolve(app: &AppEngine, name: &str) -> Result<Arc<dyn Function>, LiveError>;  // system, else active deployment
```

Semantics are §45 §3.3:
- validate in a scratch runtime;
- store the bundle and check its hash on read;
- write `pending`;
- apply the schema (Tasks 8–9; in this task only additive table creation);
- CAS the pointer against `expected_active` (on mismatch, `FAILED_PRECONDITION`, reason `live_deploy_conflict`);
- keep 50, and let the janitor delete older bundles.

`SubKey` gains `deployment: Ulid` (the nil ULID for system functions).

**Tests:**
- `deploy_swaps_functions_for_new_calls` (R1's name);
- `concurrent_deploys_one_wins`;
- `dry_run_writes_nothing`;
- `bundle_hash_mismatch_refused_and_previous_keeps_serving` (corrupt the stored object);
- `invalid_bundle_fails_without_pointer_change`;
- `retention_keeps_fifty_and_the_active_parent`;
- `system_functions_survive_any_deploy`.

**Steps:** tests first; records and protos, then storage, then the CAS, then retention.

**Commit:** `live: versioned deployments with compare-and-swap activation`.

### Task 7: Rollback and activation semantics

**Files:** `crates/loams-live/src/deploy.rs`, `crates/loams-live/tests/deploy.rs`, `crates/loams-live/tests/reactive_checker.rs`.

**Consumes:** Task 6; `Subscriptions`.

**Produces:**
- `pub async fn activate(app: &AppEngine, id: Ulid, expected_active: Option<Ulid>, by: &Principal) -> Result<DeployOutcome, LiveError>`, which re-runs the deploy path with the stored bundle and schema (§45 §3.3: no special path);
- at the activation tick, every subscription of the app reruns once on the new deployment.

**Tests:**
- `rollback_restores_previous_functions`;
- `rollback_reapplies_previous_schema` (an index added by the later deployment is dropped; checked after Task 9 lands, with an `#[ignore]` reason until then that Task 9 removes);
- `activation_reruns_every_subscription_once` (the rerun counter rises by exactly the number of distinct `SubKey`s);
- `no_session_sees_mixed_deployments` (each Transition's results all carry one deployment id, via a test hook);
- `reactive_checker_with_deploys_and_rollbacks` (the Task 1 checker with `Disturbance::{Deploy, Rollback}`).

**Steps:** tests first; then activation and the rerun-all at the tick.

**Commit:** `live: rollback by activating an earlier deployment`.

### Task 8: Schema validators

**Files:** `crates/loams-live/src/{schema.rs,validate.rs,backfill.rs}`, `proto/loams/live/v1/catalog.proto` (`Validator`, `TableSchema.validator`, `schema_validation`), `crates/loams-live/tests/schema.rs`.

**Consumes:** Task 4's `Validator`; `docs.rs` write paths; the worker lease (`loams-meta` leases in cluster mode, an in-process lease in dev).

**Produces:**
- `backfill::TableJob`, a leased, checkpointed, rate-limited scan of one table's documents in batches. The validation scan here and the index backfill of Task 9 both use it;
- validator enforcement on every write (including `_system:*`): `INVALID_ARGUMENT`, reason `live_schema_violation`, with `field_violations`;
- tightening a validator: state `enforcing`, then the validation scan (a backfill-style job with deployment state `validating`); a violation fails the deploy, reverts the validator, and reports up to 100 ids;
- loosening applies at once;
- `schemaValidation: false` skips the scan.

**Tests:**
- `write_violating_validator_refused`;
- `system_insert_is_validated_too`;
- `tightening_with_valid_data_succeeds`;
- `tightening_with_invalid_data_fails_and_reverts` (the deploy is `Failed`, the old validator is active, and the error lists the offending ids);
- `loosening_skips_scan`;
- `schema_validation_false_skips_scan_but_checks_writes`;
- `writes_during_validation_scan_are_checked`.

**Steps:** tests first; enforcement, then the scan, then the deploy states.

**Commit:** `live: schema validators enforced on writes and checked on deploy`.

### Task 9: Online index changes

**Files:** `crates/loams-live/src/{backfill.rs,catalog.rs,txn.rs}`, `crates/loams-live/tests/backfill.rs`.

**Consumes:** `catalog.rs` index definitions; `Runner::mutate`; `loams_kv::Store::run`; Task 8's `backfill::TableJob`.

**Produces:**
- `IndexState { WriteOnly { since: Ts }, Backfilling { checkpoint: Vec<u8> }, Ready, Dropping { since: Ts } }` in `IndexDef`;
- `RunnerOptions.max_mutation_lifetime` (default = `Limits::mutation_deadline`). Before prewrite, a mutation fetches `store.now()` and returns `TxnError::NotApplied("mutation lifetime exceeded")` (a retry) if `now.physical_ms() − start.physical_ms() > max_mutation_lifetime` (Ruling 9);
- the backfill job: wait until `since + 2 × lifetime`; then batches of 256 documents, each batch one transaction that reads, calls `lock_keys` on the document keys and puts the index entries; a checkpoint in the catalog; rate `backfill_docs_per_s` (5 000);
- `GetIndexStatus { state, done, total_estimate }`;
- queries on a non-ready index fail with `FAILED_PRECONDITION`, reason `live_index_not_ready`;
- drops: `Dropping`, then the wait, then a batched range delete, then the definition is removed;
- **retires R1 Ruling 5 and the row T9-1 quiesce gate** (`Runner::try_quiesce` is deleted).

**Tests:**
- `mutation_older_than_lifetime_refuses_to_prewrite` (a fault plan delays one mutation past the lifetime; it retries with a new start ts and commits);
- `index_add_on_non_empty_table_backfills` (10 000 docs; afterwards `withIndex` returns every doc in order);
- `concurrent_inserts_during_backfill_are_indexed` (an insert and patch loop runs throughout; the final index equals a rebuild from documents);
- `query_on_backfilling_index_is_failed_precondition`;
- `backfill_resumes_after_crash` (the job is killed mid-way; a new job resumes from the checkpoint; no duplicates and no gaps);
- `drop_index_deletes_entries_after_wait`;
- `checker_passes_with_index_changes_mid_workload` (Task 1 with `AddIndex` and `DropIndex`);
- `try_quiesce_is_gone` (a compile-level check through the public API surface snapshot).

**Steps:** tests first; the lifetime rule, then the states, then the job, then drops; then remove the quiesce gate.

**Commit:** `live: online index changes with a lifetime wait and a leased backfill`.

### Task 10: `LiveAdminService` (deploy half) and function logs

**Files:** `proto/loams/live/v1/{admin.proto,live.proto}`, `crates/loams-live/src/{admin.rs,logs.rs,service.rs}`, `crates/loams-live/tests/service.rs`, `sdks/typescript/packages/live/src/gen/` (regenerated).

**Consumes:** Tasks 6–9.

**Produces:**
- `service LiveAdminService { Deploy; ListDeployments; GetDeployment; ActivateDeployment; GetIndexStatus; TailLogs (server stream) }` with `loams.options.v1.module = { name: "live.admin", unstable: true }`;
- `LiveService.Deploy` **removed** (Ruling 6, D689);
- `logs::Ring` per app per node (10 000 lines), `TailLogs{follow, since_ms, function?}`, fed by `CallOutput.logs` with `{ts, request_id, function, level, line}`;
- `LiveService.Deploy`'s facade entry in `loams.tables` moves to `loams.live.admin.deploy` (the facade regenerates).

**Tests:**
- `deploy_over_admin_service_roundtrip` (Connect JSON and binary);
- `live_service_has_no_deploy` (the reflection service lists no `Deploy` on `LiveService`);
- `tail_logs_streams_console_output_with_request_ids`;
- `tail_logs_filters_by_function`;
- `logs_ring_drops_oldest_and_counts`.

**Steps:** proto first, `buf lint`, regenerate; tests; handlers.

**Commit:** `live: LiveAdminService with deploy, rollback, index status and logs`.

### Task 11: `catalog` journal entries

**Files:** `proto/loams/live/v1/journal.proto` (`JournalEntry.catalog: CatalogChange`), `crates/loams-live/src/{journal.rs,subs.rs,catalog.rs}`, `crates/loams-live/tests/journal.rs`.

**Consumes:** the journal and tailer (R1 Task 9).

**Produces:**
- every catalog write (table create, index state change, validator change, deployment activation, app settings) appends a journal entry with `catalog: { kind, table_id?, deployment? }` in the same transaction;
- the subscription manager reloads the catalog at the tick that contains it, before matching data writes of the same tick;
- the backup exporter (Task 29) consumes the same entries.

**Tests:**
- `catalog_entry_written_with_each_catalog_change`;
- `tailer_reloads_catalog_at_the_tick`;
- `data_write_after_index_change_in_same_tick_uses_new_catalog`.

**Steps:** tests first; proto; writers; tailer reload.

**Commit:** `live: record catalog changes in the commit journal`.

---

## LV1b — Apps, exposure and auth

### Task 12: Live on the main Connect port

**Files:** `crates/loams/src/api/{connect.rs,live.rs}`, `crates/loams/src/server.rs`, `crates/loams-live/src/{service.rs,system.rs}`, `crates/loams/tests/live_connect.rs`.

**Consumes:** `CATALOGUE`, `LiveAbsent`, the main router's `register`; `LiveServer`.

**Produces:**
- with the `live` feature and Live configured, `LiveService` and `LiveAdminService` are registered on the main port (replacing `LiveAbsent`) and `CATALOGUE`'s `loams.live.v1` row reports `available: true` (still `unstable: true`); without the feature, `LiveAbsent` stays;
- `--live-listen` keeps serving the same services, loopback only, and prints `--live-listen is deprecated; Live is served on --listen (D688)`;
- `_system:tables` if Task 0 found AP1e Task 20 unmerged, with AP1e's contract.

**Tests:**
- `live_on_main_port_over_connect_grpc_and_grpc_web`;
- `get_instance_reports_live_available`;
- `live_absent_without_feature`;
- `live_listen_flag_warns_and_serves`;
- `system_tables_lists_created_tables` and `system_tables_via_mutate_refused` (only if added here).

**Steps:** tests first; registration; flag handling.

**Commit:** `api: serve Loams Live on the main Connect port`.

### Task 13: The app directory and app lifecycle

**Files:** `crates/loams-live/src/{apps.rs,engines.rs,admin.rs}`, `proto/loams/live/v1/{admin.proto,catalog.proto}` (`AppRecord`, `CreateApp`, `ListApps`, `GetApp`, `UpdateApp`, `DeleteApp`), `crates/loams-live/tests/apps.rs`.

**Consumes:** `loams_kv::Store` (the system keyspace `loams_live_system`); `loams_tikv::ensure_keyspace` (tikv backend) or the embedded keyspace table.

**Produces:**
- `AppRecord { id: u32, name, namespace, keyspace, state: Active | Disabled | Deleting { purge_after_ms }, limits, issuers, allowed_origins, allow_anonymous, cursor_key_ref, created_at_ms }`;
- `Apps::{create, get_by_name, list, update, delete}`. `create` ensures the keyspace `loams_live_a<id>` **before** writing the record;
- `Engines`: lazily started `AppEngine { runner, subs, sessions, runtime, tailer }` per app, stopped after 10 minutes idle;
- the header `loams-live-app` resolves the app; the default app is the server's `--live-app`;
- `DeleteApp`: `Disabled` plus the keyspace disabled (TiKV), a backup first (once Task 29 lands; until then a recorded TODO ruling), and a purge after 7 days.

**Tests:**
- `create_list_get_update_app`;
- `create_app_creates_keyspace_first` (a fault after `ensure_keyspace` leaves no record and an orphan the janitor reports);
- `engine_starts_lazily_and_stops_when_idle` (paused time);
- `two_apps_never_see_each_others_documents`;
- `disabled_app_refuses_calls` (`FAILED_PRECONDITION`, reason `live_app_disabled`);
- `deleted_app_purged_after_retention`;
- `max_apps_per_cluster_enforced`.

**Steps:** tests first; the directory, then the engines, then the header routing.

**Commit:** `live: an app directory and many apps per node`.

### Task 14: The verifier

**Files:** if `loams-auth` exists, adapt to it in `crates/loams-live/src/auth.rs` only. Otherwise also `crates/loams-auth/{Cargo.toml,src/{lib.rs,verify.rs,keys.rs,jwks.rs,revocation.rs}}` and `crates/loams-auth/tests/verify.rs`.

**Consumes:** the instance signing keys and JWKS (§19 §5.3) or a config `[auth] jwks = "<path or url>"`; API key records (`KeyStore`).

**Produces** (the verification core only, named as MT1 names it so that MT1 and M2 extend it):

```rust
pub struct Verifier { /* instance issuer + JWKS cache + KeyStore + revocation set */ }
pub enum Principal { User { id, org, env }, ServiceAccount { id, org, env }, Agent { id, org, env, act: Vec<String> } }
pub struct Verified { pub principal: Principal, pub scopes: BTreeSet<String>, pub expires_at_ms: u64, pub token_id: String }
impl Verifier { pub async fn verify_bearer(&self, header: &str) -> Result<Verified, AuthError>; }
#[async_trait] pub trait KeyStore: Send + Sync { async fn lookup(&self, key_id: &str) -> Result<Option<KeyRecord>, AuthError>; }
pub struct KeyRecord { pub hash: [u8; 32] /* argon2id or SHA-256 per §18 §6 */, pub org, pub env, pub scopes, pub expires_at_ms, pub revoked: bool }
pub trait Revocations: Send + Sync { fn is_revoked(&self, token_id: &str) -> bool; fn subscribe(&self) -> broadcast::Receiver<String>; }
```

Rules:
- EdDSA only for Loams tokens; 60 s leeway; `env` required;
- API keys compared in constant time;
- a key cache of 30 s (§10 §4);
- **no issuance** (no token endpoint, no key creation).

In standalone without the `ControlStore`, a file-backed `KeyStore` (`[auth] keys_file`, hashed entries) lets operators run Live with API keys before M2. It is documented as the interim.

**Tests:**
- `valid_loams_token_verifies`;
- `expired_rejected_with_60s_leeway`;
- `wrong_issuer_rejected`;
- `alg_none_and_hs256_rejected`;
- `api_key_verifies_and_hash_compare_is_constant_time` (a code-level check that `subtle::ConstantTimeEq` is used);
- `revoked_key_rejected_within_cache_ttl`;
- `tampered_payload_rejected` (proptest).

**Steps:** tests first; the verifier; the file `KeyStore`.

**Commit:** `auth: the token and API key verification core` (only if built); `live: verify Live credentials`.

### Task 15: Authorization: scopes, `Authorizer` actions and the matrix

**Files:** `crates/loams-live/src/authz.rs`, `crates/loams-live/tests/{authz.rs,authz_matrix.expected.md}`.

**Consumes:** Task 14's `Verified`; the `Authorizer` trait (D66) if it exists, else `authz::Authorizer` with the same `check(principal, action, resource)` signature and an `AllowAll` plus a static role-binding implementation from config (the interim).

**Produces:**
- the app fence: an env-scoped credential's app must equal the addressed app, else `PERMISSION_DENIED`, reason `live_app_mismatch`;
- scope and action checks per §45 §5.2 (`live:call|read|write|deploy|admin`; `live_app:<app>`, `live_table:<app>/<table>`);
- `_system:*` reads and writes check the table;
- internal functions need `live:call` **and** a platform principal;
- end users (Task 16) may call only public functions;
- audit records (`audit=true` log lines until `_audit` exists).

**Tests:**
- `authz_matrix`: generates every combination of credential kind {none, end user, user token, service account, API key, agent} × scope set × app match {same, other} × target {public fn, internal fn, `_system:get`, `_system:insert`, admin Deploy, admin UpdateApp} × RPC {Query, Mutate, Watch, admin}, runs each against a server, and compares the outcomes (`allowed` or the exact code and reason) with the blessed `authz_matrix.expected.md`. `LOAMS_BLESS=1` rewrites the file, and a reviewer reads the diff.
- `other_app_credential_denied`;
- `table_level_read_grant_limits_system_query`;
- `audit_line_for_deploy_and_system_write`.

**Steps:** write the matrix test and an empty expected file (fail); implement; bless; review the table by hand against §45 §5.

**Commit:** `live: per-app and per-table authorization with a blessed matrix`.

### Task 16: App end users through OIDC

**Files:** `crates/loams-live/src/auth.rs` (issuers per app), `crates/loams-live-js/src/host.rs` (`ctx.auth`), `crates/loams-live/tests/identity.rs`, `.github/workflows/identity.yml` (an Authentik case, if MT1's job exists).

**Consumes:** `AppRecord.issuers`; MT1 Task 2's `TrustedIssuers` if merged (else the same rules implemented here: discovery, a 10-minute JWKS cache, a refresh on unknown `kid` at most every 30 s).

**Produces:**
- `Identity { issuer, subject, token_identifier /* issuer|subject */, email, name, claims: LiveValue }`;
- bearer classification per §45 §5.3: the instance issuer means a platform principal; an issuer configured on the app means an end user; the `loams_` prefix means an API key; else `UNAUTHENTICATED`;
- `allow_anonymous`;
- `ctx.auth.getUserIdentity()`.

**Tests:**
- `end_user_token_from_configured_issuer_calls_public_function` (a local test issuer with a generated key serving discovery and JWKS);
- `end_user_cannot_call_internal_or_system_or_admin`;
- `token_from_unconfigured_issuer_rejected`;
- `ctx_auth_returns_identity_claims`;
- `anonymous_allowed_only_when_configured`;
- `jwks_refresh_rate_limited`;
- `authentik_end_user_e2e` (in the `identity` job, against MT1's compose stack: an Authentik-issued token calls a public function; skipped where the stack is absent).

**Steps:** tests first; classification; issuers; `ctx.auth`.

**Commit:** `live: app end users authenticate with OIDC tokens`.

### Task 17: Session identity

**Files:** `proto/loams/live/v1/live.proto` (`rpc Authenticate(AuthenticateRequest) returns (AuthenticateResponse)`), `crates/loams-live/src/{session.rs,subs.rs,service.rs}`, `crates/loams-live/tests/identity.rs`.

**Consumes:** Tasks 14–16; `StateVersion.identity`; `SubKey`.

**Produces:**
- `Watch` reads its identity from `Authorization`;
- `Authenticate{session_id}`: the same subject extends the expiry; a different subject increments `identity`, reruns all of the session's queries at the next tick, and sends a Transition with the new identity version;
- expiry plus 60 s closes the stream with `UNAUTHENTICATED`, reason `token_expired`;
- revocation closes the sessions of a revoked token or key within the feed latency;
- 24 h rotation closes the stream with `UNAVAILABLE`, reason `session_rotate`;
- `SubKey { deployment, function, args_digest, identity }` with `identity = token_identifier | principal id | "anonymous"`.

**Tests:**
- `authenticate_same_subject_extends_without_rerun`;
- `authenticate_new_subject_bumps_identity_and_reruns`;
- `expired_session_closed_with_token_expired` (paused time);
- `revoked_key_closes_sessions`;
- `stream_rotates_after_24h`;
- `results_never_shared_across_identities` (two users watch one `ctx.auth`-dependent query; each sees only their own result; the cache holds two entries);
- `reactive_checker_with_identity_changes` (Task 1 with `IdentityChange`).

**Steps:** proto; tests; the session state; `SubKey`.

**Commit:** `live: authenticated sessions with identity versions, expiry and rotation`.

### Task 18: TLS, the non-loopback rule and CORS

**Files:** `crates/loams/src/{server.rs,api/connect.rs}`, `crates/loams-live/src/config.rs`, `crates/loams/tests/live_exposure.rs`.

**Consumes:** the main port's TLS config if MT1 Task 7 merged; else `[tls] cert_file, key_file, client_ca?` with rustls, implemented exactly to MT1 Task 7's rule and message.

**Produces:**
- the non-loopback rule (`<listener> listen on <addr>: non-loopback addresses need [tls] and [auth] (MT1)`);
- `[tls] terminated_by_proxy = true` with `trusted_proxies` (the port's setting);
- a CORS layer: preflight answered for the union of active apps' `allowed_origins`; actual requests re-checked against the addressed app (`PERMISSION_DENIED`, reason `live_origin_not_allowed`); never `Allow-Credentials`;
- R1's `check_listen` refusal remains only on the deprecated `--live-listen`.

**Tests:**
- `non_loopback_without_tls_and_auth_refused`;
- `non_loopback_with_tls_and_auth_starts` (a self-signed cert generated in the test);
- `cors_preflight_allows_union_of_origins`;
- `cors_actual_request_checked_per_app`;
- `no_allow_credentials_header`.

**Steps:** tests first; then TLS (if absent), the rule and the CORS layer.

**Commit:** `api: TLS and CORS for Live beyond loopback`.

### Task 19: Error scrubbing and edge limits

**Files:** `crates/loams-live/src/{error.rs,service.rs}`, `crates/loams-live/tests/service.rs`.

**Consumes:** `LiveError`, `connect_error`; §20 §11.4 (raw keys in `tikv-client` errors).

**Produces:**
- every `LiveError::Internal` and `Txn` message reaching a client is scrubbed of byte strings that look like keys, and replaced by a request id the operator can find in the logs;
- `ErrorInfo.reason` set on every Live error (registered in `docs/api/reasons.md`);
- request message size limits (arguments 4 MiB, bundle 16 MiB on the admin service) enforced before decoding;
- **`QueryRequest.ts` (and any client-supplied read timestamp) validated at the service edge on both backends** (Task 21 review, row T21-14): a timestamp ahead of the store's clock is `INVALID_ARGUMENT`, before it reaches `Store::snapshot`. The embedded store already refuses one more than 1 s ahead (`KvError::TsAhead`); TiKV does not refuse it, and a far-future read there pushes the max-ts that commits must exceed.

**Tests:**
- `future_read_ts_refused_at_the_edge` (a `Query` at `u64::MAX` and one years ahead → `INVALID_ARGUMENT` on both backends; the store's clock unchanged);
- `tikv_errors_never_leak_raw_keys` (an injected TiKV error containing a key → the client message has none, and the server log has the request id);
- `every_live_reason_is_registered` (a scan of the code's reason constants against `docs/api/reasons.md`);
- `oversized_arguments_refused_before_decode`.

**Steps:** tests first; then scrubbing, reasons and limits.

**Commit:** `live: scrub internal errors and register Live error reasons`.

---

## LV1d — HA and operations

### Task 24: Multi-node sessions

**Files:** `crates/loams-live/src/forward.rs`, `crates/loams/src/api/internal.rs` (or `loams.internal.v1`, as API1 built it), `crates/loams-live/tests/multinode.rs`.

**Consumes:** M1.3's request forwarding over the cluster listener; `session_id` node prefixes; Task 11.

**Produces:**
- `ModifyQuerySet` and `Authenticate` for a session owned by another node are forwarded over the cluster listener (internal port only, cluster-token or mTLS authenticated as the port already is);
- an unknown or dead owner gives `NOT_FOUND`, reason `session_gone`;
- draining: on `SIGTERM`, readiness goes false, streams close with `UNAVAILABLE`, reason `node_draining`, in-flight mutations finish within their deadline, and the process exits.

**Tests** (two `LiveServer`s in process on one store; both backends):
- `modify_on_other_node_is_forwarded`;
- `authenticate_on_other_node_is_forwarded`;
- `dead_owner_gives_session_gone`;
- `deploy_on_one_node_reaches_the_other_at_the_same_tick`;
- `drain_closes_streams_with_node_draining`;
- `reactive_checker_with_node_kill` (Task 1 with `NodeKill`: clients resume on the survivor).

**Steps:** tests first; forwarding; draining.

**Commit:** `live: serve sessions across several Live nodes`.

### Task 25: The nemesis

**Files:** `scripts/tikv/{nemesis.sh,playground.sh}`, `crates/loams-live/tests/nemesis.rs` (`#[ignore]`, run by the job), `.github/workflows/live-nightly.yml`.

**Consumes:** Tasks 1, 2 and 24; `toxiproxy`.

**Produces:**
- `playground.sh start --topology ha` (3 PD, 3 TiKV, 2 Live nodes behind toxiproxy);
- `nemesis.sh` with faults:
  - kill and restart a TiKV store;
  - kill the PD leader;
  - stall PD for 3 s (R1 Task 2 rulings);
  - kill a Live node;
  - partition a Live node from TiKV;
  - `SIGSTOP` and `SIGCONT` a TiKV store;
- a nightly job (30 min) and a release job (24 h) that run both checkers throughout.

**Tests:**
- `nemesis_reactive_and_txn_checkers_pass` (the job's single test; the report lists the faults applied and the violations, which must be empty);
- `nemesis_script_selftest` (each fault applies and heals within 30 s, run once with no workload).

**Steps:** the script and self-test first; then the job; run a 30-minute local run between builds.

**Commit:** `ci: a nightly nemesis for Loams Live`.

### Task 26: `gc_blocked_seconds` and GC alerts

**Files:** `crates/loams-tikv/src/gc.rs`, `crates/loams-kv/src/embedded/gc.rs`, `crates/loams-tikv/tests/gc.rs`.

**Consumes:** the R1 Task 3 GC loop; R1 row T3-12.

**Produces:**
- the gauge `loams_live_gc_blocked_seconds{keyspace}` (registered through Task 27's registry);
- `loams_live_gc_safe_point_lag_seconds`;
- a per-keyspace failure counter;
- a log line naming the barrier that holds the safe point.

**Tests:**
- `blocked_keyspace_reports_seconds`;
- `barrier_holding_safe_point_is_named`;
- `embedded_gc_reports_same_metrics`.

**Commit:** `tikv: report blocked GC per keyspace`.

### Task 27: Metrics, traces and logs

**Files:** `crates/loams-live/src/metrics.rs`, `crates/loams/src/server.rs` (registry and admin route if absent), `crates/loams-live/tests/observability.rs`, `deploy/live/alerts.yaml`.

**Consumes:** Task 0 item 4.

**Produces:**
- the metric set of §45 §11, named exactly as there, with only `app` as a high-cardinality label;
- per-function series behind `metrics.per_function`, capped at 50;
- OTLP spans: `live.rpc`, then `live.function` (attempt, kind, deployment), then `live.host_batch`, then `kv.txn` (begin, prewrite, commit events), and `live.tick` with links to the commits it served;
- structured JSON log fields `request_id`, `trace_id`, `app`, `function`, `deployment`, `principal`;
- `/debug/dump` Live section;
- Prometheus alert rules for §45 §11's alerts.

**Tests:**
- `metrics_exposed_with_expected_names` (scrape `/metrics`, assert every name in §45 §11);
- `metric_labels_bounded` (1 000 functions → at most 50 per-function series);
- `trace_links_rpc_to_txn` (an in-memory exporter);
- `canary_never_logged` (a document, argument and token containing `CANARY-7f3a` are written, read, failed and authenticated; no log line, span attribute or metric label contains it);
- `alerts_yaml_is_valid` (`promtool check rules` in CI).

**Steps:** tests first; registry (if absent); instrumentation; alerts.

**Commit:** `live: metrics, traces and structured logs`.

### Task 28: Quotas and rate limits

**Files:** `crates/loams-live/src/{quota.rs,limits.rs}`, `crates/loams-live/tests/quota.rs`, the limits table source (D88).

**Consumes:** `AppRecord.limits`; Task 27 metrics.

**Produces:**
- token buckets per node for every quota in §45 §12;
- over a limit: `RESOURCE_EXHAUSTED`, reason `live_quota_exceeded`, `metadata.quota`, `RetryInfo`;
- soft storage and document limits: an alert at 90%, refusing inserts at 100%;
- the limits page entries.

**Tests:**
- one pair per quota: `<quota>_at_limit_admitted` and `<quota>_past_limit_refused_with_retry_info`;
- `per_principal_rate_independent_of_app_rate`;
- `storage_soft_limit_refuses_inserts_not_deletes`;
- `limits_page_lists_every_live_limit` (the D88 render test).

**Commit:** `live: per-app and per-principal quotas`.

### Task 29: Backup: snapshot and continuous export

**Files:** `proto/loams/live/v1/{backup.proto,admin.proto}`, `crates/loams-live/src/backup.rs`, `crates/loams-live/tests/backup.rs`.

**Consumes:** `loams-store`; the journal consumer checkpoints (R1 Task 9); `Store::barrier`; Task 11's catalog entries.

**Produces:**
- `BackupManifest { backup_id, app, snapshot_ts, segments, changes_through_ts }` at `live/<app>/backups/<backup_id>/manifest.pb`;
- the snapshot: segments of up to 64 MiB (zstd) read at `snapshot_ts` under a barrier;
- the exporter: a journal consumer named `backup`. For each tick `T` it reads the documents named by new entries at `T` and appends `ChangeBatch { ts: T, upserts, deletes, catalog }` to `changes/<seq>.pb`. It flushes every 10 s or 8 MiB, and holds the janitor through its checkpoint;
- `CreateBackup` (a new snapshot) and `ListBackups`;
- daily snapshots, and pruning past the PITR window (7 days, Q636);
- the metric `loams_live_backup_export_lag_seconds{app}`.

**Tests:**
- `snapshot_is_consistent_at_ts` (concurrent writes during the snapshot; the snapshot equals a fresh read at `snapshot_ts`);
- `export_covers_every_committed_change` (a replay of the export from the snapshot equals the app at the last exported tick);
- `exporter_holds_the_journal_janitor`;
- `exporter_resumes_after_crash_without_gaps_or_duplicates`;
- `catalog_changes_exported_in_order`;
- both backends.

**Commit:** `live: per-app snapshot and continuous change export to the bucket`.

### Task 30: Restore and point-in-time recovery

**Files:** `crates/loams-live/src/restore.rs`, `crates/loams-live/tests/backup.rs`, `scripts/live/restore-drill.sh`.

**Consumes:** Task 29; Task 13 (`create`); Task 9 (index rebuild through backfill).

**Produces:**
- `Restore { backup_id, to_ts?, target_app, replace: bool }`: create the target app; load the snapshot; replay changes ≤ `to_ts`; rebuild indexes; restore the catalog and the active deployment; with `replace`, swap the directory names atomically (the old app becomes `<name>-replaced-<ts>`, disabled);
- the drill script (back up, write more, restore to a middle tick, compare).

**Tests:**
- `restore_matches_source_snapshot_at_tick` (the reactive checker's state comparison: every table and index equal);
- `pitr_to_middle_tick`;
- `restore_with_replace_swaps_atomically` (clients addressing the name see the old or the new app, never a mix);
- `restore_drill_script_passes` (in the nightly job, on TiKV).

**Commit:** `live: restore an app to a point in time`.

### Task 31: The BR spike and the cluster DR runbook

**Files:** `docs/plans/lv1-br-spike.md`, `scripts/tikv/br-drill.sh`, `docs/guides/live/backup.md`.

**Consumes:** Q37, Q627; the v8.5.8 `br` binary.

**Produces:**
- a spike report: whether `br backup txn` or `raw` and `br restore` cover an API v2 txn keyspace (Live and metastore keyspaces), per keyspace or only cluster-wide, and whether log backup (`br log start`) and PITR restore work for them, with the commands and outputs;
- if BR works, a nightly drill; if not, the runbook says that cluster DR is per-app logical restore of every app, in parallel, plus a metastore restore.

**Tests:** `br_drill` (nightly, only if the spike is positive; otherwise this task records the negative result as a ruling and the test is not created).

**Commit:** `docs: BR on API v2 txn keyspaces (spike) and the cluster DR runbook`.

### Task 32: Reference deployment

**Files:** `deploy/live/{compose.yaml,helm-values.yaml,README.md}`, `crates/loams/tests/live_probes.rs`.

**Consumes:** Tasks 24, 27.

**Produces:**
- a compose topology (3 PD, 3 TiKV with location labels, 2 Live nodes with the `live-tikv` build, an L7 proxy with a 60 s idle timeout, MinIO or RustFS for backups);
- the Live role's chart values: probes on `/health/live` and `/health/ready`, `terminationGracePeriodSeconds` at least the mutation deadline plus 10 s, a pod anti-affinity rule, resources;
- a readiness check that is false until the app directory and the TiKV connection are healthy.

**Tests:**
- `readiness_false_until_store_connected`;
- `readiness_false_while_draining`;
- `compose_smoke` (in the nightly job: bring the stack up, run a 2-minute checker workload through the proxy, kill one Live node, check that the clients resume).

**Commit:** `deploy: a reference HA topology for Loams Live`.

### Task 33: The performance harness and gates

**Files:** `bench/live/{Cargo.toml,src/main.rs,scenarios/p01.toml … p15.toml,baseline.json,compare.py}`, `.github/workflows/live-nightly.yml`.

**Consumes:** §45 §15; the generated Rust client; Task 32's topology.

**Produces:**
- one scenario per gate P1–P15 with its measurement;
- JSON results `{gate, value, unit, p50, p99, rig}`;
- `compare.py` failing on a regression past each gate's threshold;
- the first run's numbers written to `baseline.json` and into the exit report for Q633.

**Tests:**
- `harness_selftest` (each scenario runs for 10 s against `loams dev` on the embedded store and emits a valid result);
- `compare_flags_regression` (a synthetic result past its threshold fails).

**Steps:** self-test first; scenarios; the baseline run on the reference rig (an owner action if the rig is not available: record it).

**Commit:** `bench: Loams Live performance gates`.

---

## LV1e — SDKs, docs, security and release

### Task 34: `@loams/live`, the reactive client (R1 Task 14)

**Files:** `sdks/typescript/packages/live/{package.json,src/{index.ts,client.ts,session.ts,optimistic.ts,values.ts,auth.ts,paginate.ts},test/{session.test.ts,values.test.ts,auth.test.ts,live.test.ts}}`.

**Consumes:** the generated stubs; `TokenSource` from `@loams/client` (D608) if exported, else a local interface with the same shape.

**Produces** (R1 Task 14's API, extended):

```ts
export class LiveClient {
  constructor(opts: { baseUrl: string; app?: string; auth?: TokenSource; transport?: "connect" | "grpc-web"; fetch?: typeof fetch });
  watch<T = LiveValue>(fn: string, args?: LiveValue): Subscription<T>;
  query<T = LiveValue>(fn: string, args?: LiveValue): Promise<T>;
  mutate<T = LiveValue>(fn: string, args?: LiveValue, opts?: { idempotencyKey?: string; optimistic?: (local: LocalStore) => void }): Promise<{ result: T; commitTs: bigint }>;
  paginate<T>(fn: string, args: LiveValue, opts: { numItems: number }): PaginatedSubscription<T>;
  setAuth(auth: TokenSource | null): Promise<void>;     // Authenticate on the open session
  close(): void;
}
```

Behaviour:
- reconnect with jitter (250 ms to 10 s);
- resume on a version gap, `session_rotate`, `node_draining` or `session_gone`;
- refresh and `Authenticate` before expiry, and on `token_expired` a refresh then a resume;
- the `loams-live-app` header;
- a UUIDv7 idempotency key once per logical mutation;
- `RetryInfo` honoured;
- lossless `int64`;
- no Node built-ins under `src/`.

**Tests:**
- `session.test.ts` (a fake transport): `applies_transitions_in_version_order`, `resumes_on_version_gap`, `drops_optimistic_update_at_commit_ts`, `resumes_on_session_rotate_and_draining`, `identity_change_discards_old_results`.
- `auth.test.ts`: `authenticates_before_expiry`, `token_expired_refreshes_and_resumes`.
- `values.test.ts`: `int64_roundtrip_is_lossless`.
- `live.test.ts` (against `loams dev`, embedded): `watch_sees_insert_from_another_client`, `mutate_returns_commit_ts_and_watch_catches_up`, `reconnect_after_server_restart_converges`, `paginate_walks_all_pages`.

**Commit:** `sdk: the Loams Live reactive TypeScript client`.

### Task 35: Session conformance fixtures

**Files:** `sdks/fixtures/live/*.json`, `sdks/fixtures/manifest.json`, `crates/loams-live/tests/fixtures.rs`, `sdks/typescript/packages/live/test/fixtures.test.ts`.

**Consumes:** §44 §10.4's fixture runner conventions; Task 34.

**Produces:** fixtures for:
- initial set, add and remove;
- a gap and its resume;
- chunked Transitions (`more`);
- merged Transitions under backpressure;
- an identity change;
- rotate, draining and gone;
- `token_expired`;
- a quota error with `RetryInfo`.

Each fixture is replayed against the real server (Rust) and through the client's fake transport (TypeScript).

**Tests:** `every_live_fixture_passes_on_server`, `every_live_fixture_passes_in_client`, and `results/typescript-live.json` written (generated, not committed, D640).

**Commit:** `sdk: Live session conformance fixtures`.

### Task 36: `@loams/live-react`

**Files:** `sdks/typescript/packages/live-react/{package.json,src/{index.ts,provider.tsx,hooks.ts},test/hooks.test.tsx}`.

**Consumes:** Task 34.

**Produces:** `LiveProvider`, `useQuery(fn, args | "skip")`, `useMutation(fn)` with `.withOptimisticUpdate(...)`, `usePaginatedQuery(fn, args, { initialNumItems })` with `loadMore`, and `useLiveAuth()`, built on `useSyncExternalStore`. React 19 is a peer dependency.

**Tests:**
- `use_query_renders_and_updates`;
- `skip_does_not_subscribe`;
- `unmount_unsubscribes`;
- `optimistic_update_renders_then_reconciles`;
- `paginated_load_more_appends`;
- `strict_mode_double_mount_subscribes_once`.

**Commit:** `sdk: React hooks for Loams Live`.

### Task 37: `@loams/live-cli` and `loams live deploy`

**Files:** `sdks/typescript/packages/live-cli/{package.json,src/{cli.ts,bundle.ts,deploy.ts,codegen.ts,migrate.ts,logs.ts},test/*.test.ts}`, `crates/loams/src/main.rs` (`live deploy --bundle --schema`).

**Consumes:** `LiveAdminService`; esbuild.

**Produces:**
- `npx loams-live init | dev | deploy | deployments | rollback [--to <id>] | logs [--follow] | migrate run <fn> | codegen`;
- `dev` watches `loams/`, bundles, and deploys to the dev app with `expected_active`;
- `codegen` writes `loams/_generated/api.ts` (typed function references from the export metadata and argument validators);
- the Rust `loams live deploy --bundle <file.js> --schema <schema.json> [--app] [--message]` for CI without Node.

**Tests:**
- `bundle_produces_single_esm_module`;
- `deploy_sends_expected_active_and_reports_conflict`;
- `rollback_lists_and_activates`;
- `migrate_runs_batches_until_done_and_resumes_with_cursor`;
- `codegen_types_match_validators` (a `tsc` compile of a sample);
- the Rust `live_deploy_cli_roundtrip`.

**Commit:** `sdk: the Loams Live CLI`; `cli: loams live deploy`.

### Task 38: Documentation

**Files:** `docs/guides/live/{quickstart,functions,schema,auth,deploy,operations,backup,limits}.md`, `docs/design/20-reactive-database-on-tikv.md` (as built), `docs/design/45-loams-live-production.md` (as built), `docs/plans/2026-09-27-r1-reactive-core.md` (the status of Tasks 13, 14, 16 and 17: "done in LV1"), `CHANGELOG.md`.

**Produces:** §45 §17's list. Every command in the guides is run by a doc test (`scripts/docs/check-live-guides.sh` runs the quickstart against `loams dev` in CI).

**Tests:** `live_quickstart_runs` (CI); `limits_page_generated` (from Task 28); link check.

**Commit:** `docs: Loams Live developer and operator guides`.

### Task 39: Security review and fuzzing

**Files:** `docs/security/live-threat-model.md`, `crates/loams-live/fuzz/` (cargo-fuzz targets), `crates/loams-live-js/fuzz/`, `.github/workflows/live-nightly.yml`.

**Consumes:** §45 §16.

**Produces:**
- the threat model covering every §45 §16 item, each mapped to its mitigation and to the test that proves it (or marked as an accepted risk with the owner);
- fuzz targets `value_decode`, `tuple_decode`, `cursor_decode`, `worker_frame`, `jwt_parse`, `api_key_parse`, run nightly for 10 minutes each;
- `cargo deny check` clean;
- the external review scheduled (Q635, an owner action).

**Tests:** `fuzz_targets_build`; the nightly fuzz job; `threat_model_items_have_tests` (a script checks that every row names a test that exists).

**Commit:** `docs: the Loams Live threat model`; `live: fuzz targets for Live decoders`.

### Task 40: Stabilize the protocol, the exit report and the release gate

**Files:** `proto/loams/live/v1/*.proto` (drop `unstable`), `buf.yaml` (stop excluding the package from `buf breaking`), `crates/loams/src/api/connect.rs` (`unstable: false` when the gate passes), `docs/plans/lv1-exit-report.md`, `docs/plans/README.md`, this plan's status.

**Consumes:** every earlier task.

**Produces:**
- the exit report: every checklist row below with its evidence (CI run links, the bench JSON, the restore drill log, the nemesis report, the review summary);
- the gate decision. If every row passes, `loams.live.v1` is stable and `GetInstance` reports `unstable: false`. Otherwise Live ships labelled preview (D698), and the report lists the failing rows.

**Tests:** `buf_breaking_covers_live` (CI); `get_instance_live_not_unstable` (only when the gate passed).

**Commit:** `live: stabilize loams.live.v1`; `docs: LV1 exit report`.

---

## Self-review

- Every §45 section maps to tasks:

  | §45 | Tasks |
  |---|---|
  | §3 | 3–7, 10 |
  | §4 | 8–9, 37 |
  | §5 | 14–17 |
  | §6 | 12, 18 |
  | §7 | 13 |
  | §8 | 20–23 |
  | §9 | 24, 26, 32 |
  | §10 | 29–31 |
  | §11 | 27 |
  | §12 | 28 |
  | §13 | 34–37 |
  | §14 | 1, 2, 5, 15, 22, 25, 35, 39 |
  | §15 | 33 |
  | §16 | 39 |
  | §17 | 38 |
  | §18 | 40 |

- R1's unbuilt tasks:

  | R1 task | LV1 tasks |
  |---|---|
  | 13 | 3–7 |
  | 14 | 34 |
  | 16 | 1, 2, 25 |
  | 17 | 38 |

- Every task names its tests, and each test states what it asserts.
- External dependencies:
  - `loams-auth` (Task 14 builds the core if absent);
  - the `Authorizer` (Task 15 has an interim);
  - main-port TLS (Task 18 builds it if absent);
  - the metrics registry (Task 27 builds it if absent);
  - the reference rig (Task 33, an owner action).
- No task edits `apps/desktop-electron/` or `web/`.

## Exit criteria for production

Live is labelled **production** in a release only when every box is checked on the release candidate (D698). Otherwise it ships as **preview**.

**Correctness**
- [ ] `kv_conformance!` passes on embedded and TiKV.
- [ ] The reactive checker passes on both backends with every disturbance: deploy, rollback, index add and drop, identity change, node kill.
- [ ] The transaction checker passes. Point-read promotion shows no write skew. The range write-skew behaviour is documented (Q31).
- [ ] The Live mutation fault matrix passes. No acknowledged mutation is lost, and no idempotent mutation is applied twice.
- [ ] The nemesis ran **24 hours green** on the HA topology for this release.
- [ ] `missed_invalidation_total` stayed at 0 throughout the nemesis and the benchmarks.

**Functions and schema**
- [ ] Deploy, rollback, dry run and deploy conflicts work over `LiveAdminService` and `@loams/live-cli`.
- [ ] Validators are enforced. Tightening scans and fails safely.
- [ ] Online index add and drop work on a 1 M-document table under load (P11).
- [ ] Multi-tenant mode refuses to start without `isolated`. The sandbox tests pass on Linux.

**Security and exposure**
- [ ] Live is served on the main port. Non-loopback requires TLS and auth.
- [ ] `authz_matrix` is blessed and reviewed. The tenant fence (`live_app_mismatch`) is tested.
- [ ] End-user OIDC works with Authentik (e2e) and with a generic issuer.
- [ ] Session expiry, revocation and rotation are tested. Results are never shared across identities.
- [ ] No canary appears in logs, spans or metrics. No raw keys appear in errors.
- [ ] The threat model is complete. The fuzz targets have run nightly for 14 days with no open crash.
- [ ] The external review is done with no open high or critical finding (Q635).

**Operations**
- [ ] The reference HA topology passes its compose smoke test. Draining works.
- [ ] Metrics, traces, logs and alert rules are shipped and validated.
- [ ] Every quota has an at-limit and a past-limit test, and the limits page is generated.
- [ ] Every app's backup export is running, with lag ≤ 15 s at P2 load (P12).
- [ ] The restore drill passed on this release candidate (TiKV), including PITR to a middle tick.
- [ ] The cluster DR path is documented (BR, if the spike is positive).
- [ ] `gc_blocked_seconds` is exported and alerting.

**Performance**
- [ ] P1–P15 are measured on the reference rig and recorded in the exit report. None regressed past its threshold, and the owner has confirmed the baselines (Q633).

**Developer experience**
- [ ] `loams dev` and Loams Desktop run Live with no TiKV (embedded default).
- [ ] `@loams/live`, `@loams/live-react` and `@loams/live-cli` are published at 1.0 and pass 100% of the Live fixtures.
- [ ] The guides are published and the quickstart doc test is green.

**Contract**
- [ ] `loams.live.v1` has `unstable` dropped, `buf breaking` covers it, and `GetInstance` reports `unstable: false`.
- [ ] Every Live error reason is registered in `docs/api/reasons.md`.

## Rulings made during execution

Task 0 (reconciliation against `dev` at 6079e4d7, 2026-10-08).

| # | Ruling | Why | Cost if wrong |
|---|---|---|---|
| T0-1 | **R1 Tasks 13, 14, 16, 17 are unbuilt, as the plan assumed.** `crates/loams-live/src/deploy.rs` exports `DEPLOY_UNIMPLEMENTED` and `Deploy` answers `UNIMPLEMENTED` (its header says Task 13 follows). There is no `loams-live-js` crate and no `loams-auth` crate. `sdks/typescript/packages/live` holds only `src/{gen,index.ts}`, generated code with no hand-written client. `crates/loams-live/src` has no `js`, `auth` or `admin` module. | Directory listings and `deploy.rs` | None |
| T0-2 | **AP1e Task 20 (`_system:tables`) has merged.** `crates/loams-live/src/system.rs` defines `TABLES = "_system:tables"`, a query with args `{}` returning `[{ name, id, indexes: [{ name, fields }] }]`, built-ins included, with a unit test. Task 12 adds nothing; it only keeps `_system:tables` in the "internal, never public" set (§45 §3.4) and in the admin-only function list. | `system.rs` lines 10, 32 and 159 | None |
| T0-3 | **`loams-auth` does not exist** (no `Verifier`, no `KeyStore`, no `jsonwebtoken` or `openidconnect` in the workspace). Task 14 builds the verification core with the interfaces its text gives and records the as-built surface here when done. MT1 Task 2 has not landed on `dev`. LV1b's order stays as planned, with Task 14 free of MT1. | `ls crates`, `Cargo.toml` | If MT1 lands first, Task 14 reuses its crate and this ruling is superseded |
| T0-4 | **The engine has no metrics registry, admin listener or OTLP export.** Only `loams-qdrant` routes `GET /metrics` (a compatibility stub). `crates/loams/Cargo.toml` has no prometheus or opentelemetry dependency. Tasks 27 and 28 add the minimum: `prometheus-client` and the admin listener in 27, OTLP in 28. Live's only counters today are the subscription counters exposed through the server handle (`missed_invalidations`). | grep of the workspace | Task 27 grows if M2's observability lands first |
| T0-5 | **The main port has no TLS and no auth layer.** `crates/loams/src` has no rustls use and no bearer handling; `rustls` is only a workspace pin with `default-features = false`. MT1 Task 7 is not merged, so Task 18 adds `[tls] cert_file, key_file, client_ca?` itself, gated exactly as MT1 Task 7 specifies (non-loopback requires TLS and auth). | `crates/loams/Cargo.toml`, `crates/loams/src` | None |
| T0-6 | **Naming drift, to fix in Task 23.** (a) Doc comments and log lines say "Loam Live" (`service.rs` lines 1, 33, 120; `main.rs` 636, 640, 648, 658; `server.rs` 270, 353, 460, 462, 637, 682, 709, 727, 1038, 1927, 1933, 1940; `Cargo.toml` 91). (b) `--live-keyspace` help says `loam_live_<app>`; the constant `KEYSPACE_PREFIX` is `loams_live_`. (c) The doc comments of `service.rs` say the package is `loam.live.v1`; the proto package and the wire path are `loams.live.v1`. The wire is correct; only text changes. | Greps | Cosmetic; a user-facing error message would be wrong |
| T0-7 | **Consumed surface is as built.** `LiveArgs` has exactly `--live-listen` (default `127.0.0.1:7710`, loopback-only check at startup), `--live-pd` (default `127.0.0.1:19379`), `--live-keyspace`, `--live-app` (default `dev`), `--live-tick-read-lag-ms` (default 50) and `--no-live`. `LiveAbsent` and `CATALOGUE` are in `crates/loams/src/api/connect.rs`; the `loams.live.v1` row is currently `available: false`-style via `LiveAbsent` and `unstable` is a field of the row. `Runner::try_quiesce` exists at `txn.rs:672` (returns `Option<Quiesced>`, with a test at `tests/txn.rs:1404`) and Task 9 retires it as planned. `loams-tikv` modules present: `classify, codec, config, faults, gc, keyspace, pd, regions, runner, testing, token, tso, txn`. | Reading | None |
| T0-8 | **Dependency versions and licences.** `rquickjs` 0.14.0 (MIT, MSRV 1.87), `seccompiler` 0.5.0 (Apache-2.0 OR BSD-3-Clause), `landlock` 0.4.7 (MIT OR Apache-2.0), `prometheus-client` 0.25.1 (Apache-2.0 OR MIT), `opentelemetry` and `opentelemetry-otlp` 0.33.0 (Apache-2.0). All are allowed by the licence policy as far as the plan's list goes (check `deny.toml` in Task 23). The Tech Stack licence for `seccompiler` is corrected to "Apache-2.0 OR BSD-3-Clause". A throwaway crate with the three sandbox and JS dependencies, with the Task 21 feature set for `rquickjs`, **resolved** with a lockfile of 40 packages on Rust 1.97.1. It was **not compiled**, to keep the shared target dir cold; Task 21's first step compiles it. | `cargo info`, `cargo generate-lockfile` | A compile failure (for example `rquickjs-sys` needing a C toolchain) shows in Task 21 |
| T0-9 | **Owner answers to Q625, Q626, Q629, Q631, Q639: none given.** The §45 defaults stand: Q625 actions and scheduled functions stay in R2; Q626 embedded is dev and Desktop only; Q629 `live-tikv` is binary-only; Q631 function code only, no declarative rules; Q639 a Live app is 1:1 with an environment. | Plan and spec text carry no answers | Any later answer reopens the affected task |
| T0-10 | **Desktop contract: Live has its own port today.** `apps/desktop-electron` (not edited here) reserves a fifth port (`ports.live`) and starts the engine with `--live-listen 127.0.0.1:<live> --live-pd <pd>`, or `--no-live` when the binary lacks the flag (`engine/binary.ts`: `helpSupportsLive` matches `--no-live` or `--live-listen`). The supervisor sets `liveUrl = http://127.0.0.1:<live>`. The protocol handler (`protocol/route.ts`) forwards `/loams.live.v1.*` from `loams-app://console` to `engine.liveUrl`, and the agent tools (`agent-tools/live.ts`) post Connect JSON to `<liveUrl>/loams.live.v1.LiveService/{Query,Mutate}` with no `Authorization` header. | Reading the desktop code | See T0-11 to T0-13 |
| T0-11 | **Main-port move is non-breaking for the desktop if `--live-listen` keeps working** (Task 12 already keeps it, deprecated, loopback only, same services; D688). The deprecation line is printed to the engine log, which the desktop captures but does not parse. **Follow-up for the desktop owner** (not in LV1): drop the fifth port and `--live-listen`, set `liveUrl` to the main `http` URL, and make `helpSupportsLive` accept the new flag set. Until `--live-listen` is removed in a later release, LV1 must not remove it. The removal needs a release note. | Keeps AP1e working while LV1 lands | None if the flag stays |
| T0-12 | **Auth on loopback must stay off by default.** The desktop sends no credentials and runs on loopback. Task 15's rule is therefore the one in §45 §6: loopback listeners with no `[auth]` config accept unauthenticated requests (dev mode, one warning in the log); non-loopback requires TLS and auth. Task 15 must not make auth mandatory for loopback. **Follow-up for the desktop owner:** none now; once `[auth]` is configured on a Desktop-run engine, the protocol handler must add `Authorization: Bearer`. | Desktop sends no credentials | The Live page and agent tools fail with 401 if auth is forced |
| T0-13 | **Value JSON and function-name contract the desktop relies on.** (a) Proto3 JSON of `loams.live.v1.Value` (`nullValue`, `boolValue`, `int64Value` as a decimal string, `doubleValue`, `stringValue`, `bytesValue` as base64, `arrayValue.values`, `objectValue.fields`), plus the desktop-side forms `{"$int64":"<decimal>"}` and `{"$bytes":"<base64>"}` shared with the Live page. These are client-side conventions, not wire changes. (b) `Query` and `Mutate` request and response field names, including the response `result` and `ts` (or `commitTs`). (c) `_system:tables` (T0-2) and the other `_system:*` read functions the Live page calls. **LV1 must keep all of these byte-compatible.** Task 3 (`unstable` dropped) and the `ctx.auth`, `paginate` and public/internal changes of Tasks 15, 17 and 26 are additive. Rule for every task: `buf breaking` must stay green and no existing field number or JSON name changes. Public and internal functions (D699): an existing function with no declared visibility must default to **public**, so desktop-deployed apps and the agent tools keep working. **Follow-up for the desktop owner:** `_system:*` becomes admin-only on the end-user path (§45 §3.4); when auth is on, the Live page needs an admin credential. | Reading `agent-tools/live.ts`, `route.ts`, `supervisor.ts` | A visibility default of internal would break every deployed app |
| T0-14 | **Embedded store and the desktop stack.** `deploy/tikv/compose.yaml` runs PD and one TiKV (API v2, v8.5.0 image, host networking, PD on 19379 as the `--live-pd` default). LV1c's embedded default means the desktop no longer needs that stack to run Live. **Follow-up for the desktop owner:** when the embedded store is the default, `--live-pd` becomes optional and `wantLive` in the supervisor should no longer depend on `livePd`; keep the TiKV stack as an opt-in. Until then Task 25's flag set must keep `--live-pd` meaning "use TiKV" and an absent `--live-pd` meaning embedded, **but only after** Task 25 lands; the desktop passes `--no-live` when there is no PD, which stays valid. | Reading `supervisor.ts`, `compose.yaml` | The desktop keeps showing "Live needs a TiKV stack" until updated |
| T0-15 | **The plan's version of the engine's TiKV image and playground differ.** The compose file pins `v8.5.0` (overridable by `TIKV_TAG`); the plan's cluster tests use `tiup playground v8.5.8`. Not a conflict (different uses). Task 30's BR binary and the test cluster use 8.5.8; the desktop stack is not upgraded by LV1. | Compose file, plan Tech Stack | None |

Task 20 (the `loams-kv` seam, 2026-10-08).

| # | Ruling | Why | Cost if wrong |
|---|---|---|---|
| T20-1 | **The tuple codec moved to a new leaf crate, `loams-tuple`**, not into `loams-kv`; `loams-kv` and `loams-tikv` both re-export it (`loams_kv::tuple`, `loams_tikv::{tuple, codec::tuple, CodecError}` keep their paths). `Ts` lives in `loams-kv` only and is not re-exported from `loams-tikv`. Amends Ruling 2. | `loams-kv`'s tikv backend depends on `loams-tikv`, so `loams-tikv` cannot re-export from `loams-kv` without a package cycle. `loams-tikv` and `loams-meta-tikv` use tikv-client's `Timestamp` and never need `Ts`. | One more crate in the workspace |
| T20-2 | **`loams-tikv` gains `Tikv::run_as::<W, T, E, F>` and the trait `RunTxn`** (additive; `Tikv::run` keeps its signature and calls `run_as::<Txn, _, TxnError, _>`). The body works on `W: RunTxn`, which owns the attempt's `Txn`, and fails with any `E: Into<TxnError>`. `loams_kv::Txn` implements `RunTxn`, so `Store::run` hands its body straight to the TiKV runner. | `loams_kv::Txn` has no lifetime parameter (the plan's `pub enum Txn`), so it must own the attempt's `loams_tikv::Txn`; converting the body's error by rewrapping its future would force `T: 'static` | None: retries, tokens, faults and error classes are `run`'s own code |
| T20-3 | **Features of `loams-kv`:** `tikv = ["dep:loams-tikv"]` and `faults = ["loams-tikv?/faults"]`. `Store::with_faults` exists only with `faults` (as `Tikv::with_faults` does). No `embedded` feature yet (Task 21 or 23 adds it if the embedded backend's dependencies must be optional). `loams-live` depends on `loams-kv` with `tikv` until Task 23; its dev-dependency adds `faults`. | Mirrors `loams-tikv`'s fault gating; keeps fault hooks out of release builds | Task 23 reshapes the feature list anyway |
| T20-4 | **The embedded stub is uninhabited**: `embedded::{Handle, Txn, Snap}` each hold a `std::convert::Infallible`, so every embedded match arm is statically unreachable (no panics). `Store::open(StoreConfig::Embedded(_))` returns `KvError::Unsupported("embedded backend arrives in Task 21")`. `testing::stores(name)` yields only the TiKV store until Task 21. `loams_kv::testing` also re-exports `loams_tikv::testing`'s cluster harness (`cluster`, `TestCluster`, `TEST_LIVE`, …) and adds `testing::tikv()` (a TiKV store on `TEST_LIVE` under a fresh root), so `loams-live`'s tests need no `loams-tikv` dependency. | No placeholder behaviour to remove later; Task 21 replaces the three types | None |
| T20-5 | **`LiveConfig.tikv: TikvConfig` stays** (re-exported as `loams_kv::TikvConfig`); Task 21 replaces it with `store: StoreConfig` (T20-9; Task 23 in the original plan). `LiveServer::start` opens `Store::open(StoreConfig::Tikv(config.tikv))`. `Runner::tikv()` and `LiveHandle::tikv()` became `store()`. `crates/loams/src/server.rs` (outside the task's file list) reaches the TiKV handle for its cluster GC loop and sweep list through the new `Store::as_tikv() -> Option<&Tikv>` (feature `tikv`); an embedded store needs no cluster GC. | Keeps `crates/loams/src/main.rs`, `crates/loams/tests/live.rs` and the CLI unchanged until Task 23 | None |
| T20-6 | **`Store::barrier(name, at, ttl)` uses the service id `loams/<name>`**, `name` being `<purpose>/<id>`. `loams_kv::GcBarrier` has `service_id()`, `ts()` and `delete(self)`. | `loams_tikv::GcBarrier` requires the `loams/` prefix; one name argument as the plan's signature has | None until Task 29 uses it |
| T20-7 | **`Ts` extras:** `Ts::logical()`, `Display` (the decimal version) and `Default`; `from_parts` masks `logical` to 18 bits. tikv-client converts versions through `i64`, so `Ts` and `Timestamp` agree for physical parts below 2^45 ms (year 3084). `Snap::ts()` returns `Ts` by value. `KvError` is `Unsupported(&'static str)` or `Tikv(TikvError)` with the TiKV text unchanged (`#[error(transparent)]`), so Live's error messages are byte-identical. | Needed by `lagged` and its test; keeps error texts stable | None |
| T20-8 | **Test environment:** the machine's test cluster runs at another port offset than the plan's 17000, so `loams-tikv`'s `gc::safe_point_advances_cluster_wide` needs `LOAMS_TEST_TIKV_STATUS=127.0.0.1:20280` beside `LOAMS_TEST_PD=127.0.0.1:23790`. Not a code change. | It reads TiKV's status port, default `127.0.0.1:37180` | None |
| T20-9 | **`LiveConfig.store: StoreConfig` (replacing `LiveConfig.tikv`) and a `LiveServer` that starts on any `Store` move from Task 23 into Task 21** (controller ruling, review of Task 20). Task 21's and Task 23's file lists are amended; Task 23 keeps the CLI flag, the feature split and the embedded default. Not implemented in Task 20. | Task 22 can then run the `service` and `session` suites, which start a `LiveServer`, on the embedded backend | Task 21 grows by the config change and its callers in `crates/loams` |

Task 21 (the embedded MVCC backend on redb, and T20-9, 2026-10-08).

| # | Ruling | Why | Cost if wrong |
|---|---|---|---|
| T21-1 | **The `versions` key escapes the key: `keyspace_id:u32 BE ‖ esc(root ‖ key) ‖ (u64::MAX − commit_ts) BE`.** `esc` writes each `0x00` as `0x00 0xFF` and ends with `0x00 0x00`. Values are as Ruling 4 says (`0x00 ‖ value`, or `0x01` for a tombstone). Amends Ruling 4's layout. | Raw `root ‖ key ‖ ts` is not prefix-free. Key `a`'s versions sort among the versions of `a\x00…`, and a scan bounded by `a\x05` misses `a`, whose versions sort above the bound. Escaping keeps key order, makes each key's versions contiguous (newest first), and maps a key range to exactly one table range. | About 2 bytes more per version |
| T21-2 | **The oracle persists its high-water mark 1 s ahead.** Every timestamp issued is below the persisted `oracle.high_water`. Before one would reach the mark, the mark moves to the timestamp's millisecond + 1 000 and is persisted (by the committer inside the group's own write transaction, or by a blocking write). Under steady use that is about one write a second, and 2^18 logical values per millisecond far exceed "1 000 allocations". A restart starts above the mark. A `snapshot(at)` above every issued timestamp moves the oracle past `at` and persists a mark above it (TiKV's `max_ts`), so no later commit lands at or below a read already taken. | Reserving ahead is crash-safe. A lagging mark ("persist every 1 000 allocations") could reissue timestamps after a crash. | One extra write transaction per second of activity |
| T21-3 | **The `oracle` table also holds `gc_safe_point`.** GC persists it in the same write transaction that drops the versions, and it never moves back. Snapshots, transactions and barriers below it are refused after a restart too. | Otherwise a restart would forget how far GC went, and a new barrier could let a snapshot read versions that are already gone | None |
| T21-4 | **`EmbeddedConfig` changes:** it gains `gc_interval` (default 60 s; the first handle on a file sets it) and `EmbeddedConfig::new(path, keyspace)` (empty root, `gc_life_time` 10 min, `gc_interval` 60 s). `gc_life_time` must exceed one minute, `gc_interval` must be positive and the keyspace must be non-empty. Reads have TiKV's window, `gc_life_time − 1 min`. Past it, a `snapshot(at)` is refused with `KvError::GcSafePoint` and an open `Snap`'s reads with `Fatal("read below the GC safe point")`, unless a barrier covers `at`. Any read below the persisted GC safe point is refused. | Parity with TiKV's window (row R7) and its config check. The conformance barrier case runs unchanged on both backends. | None |
| T21-5 | **`KvError` gains three variants and loses one.** New: `Embedded(String)`, plus `GcSafePoint { at, safe_point }` and `BarrierBelowSafePoint { service_id, ts, min_safe_point }` with the TiKV texts unchanged. `Unsupported` is removed, since the stub is gone and nothing constructed it. | Live's error texts stay the same on both backends | Code that matches `KvError::Tikv(GcSafePoint)` must also match the embedded variant (the conformance helper does) |
| T21-6 | **There is one database per file per process.** A registry keyed by the canonical path gives every `Handle` on a file the same redb `Database`, oracle, committer thread and GC thread, and the file closes with the last handle. A reopen waits up to 2 s for this process's previous handle to close the file. Another process holding the file gets `KvError::Embedded("the store file … is open in another process")`. `embedded::is_open(path)` is public. Barriers and open snapshots are per file, across keyspaces (as PD's service safe points are cluster-wide). The file's GC life time is the longest any handle asked for. Keyspace ids are assigned from 1 on first use. | redb locks its file (Task 20 review) | None |
| T21-7 | **Embedded transaction semantics follow `tikv-client`'s buffer.** Reads are cached. An `insert` of a key the transaction read as present fails at once; any other insert fails at commit. An insert-then-delete leaves a not-exists check. `lock_keys` is a lock-only mutation. The committer checks conflicts before inserts. Its checks: a written or locked key has a version newer than the start timestamp (or than its pessimistic lock), or a pessimistic lock of another transaction holds it, giving `Conflict`; an inserted key is present, giving `AlreadyExists("key already exists: \"…\"")`. A read-only transaction commits at its start timestamp. A failed write transaction is `Undetermined`, which the runner resolves through the commit token. The backend's raw API is public: `Handle::begin(mode)`, `embedded::Txn::commit()` and `Handle::{snapshot, now, barrier, gc_once, gc_once_at, stats}`. The model check uses it, and later checkers can too. | The seam's behaviour must not depend on the backend (Review Focus 4). The runner's retries would hide interleavings from the model check. | None |
| T21-8 | **(Superseded by T21-15.)** **Pessimistic mode on embedded uses an in-process lock table.** `put`, `insert`, `delete` and `lock_keys` lock at once, wait up to 1 s for another holder, and then fail with `Conflict` (TiKV's lock wait and deadlock both map to `Conflict`). The lock timestamp is the key's conflict threshold. Reads stay at the start timestamp, as on TiKV, since the seam has no `get_for_update`, so a pessimistic read-modify-write can lose an update on either backend. The test `pessimistic_writers_queue` asserts only that writers queue. | Parity; `loams-live` does not use pessimistic mode today | A future pessimistic caller needs `get_for_update` on the seam |
| T21-9 | **Embedded GC also sweeps expired commit tokens and fences** under every root opened on the file, as one transaction per root. When the newest version at or below the safe point is a tombstone, GC drops it along with everything older. | The embedded store has no cluster GC loop to sweep tokens | None |
| T21-10 | **The conformance harness.** `kv_conformance!(backend, factory)` expands 13 `#[tokio::test]` cases (multi-thread) for a `testing::Factory`, which yields a store per `testing::Spec { root, gc_life_time }` or `None` to skip. The 13 cases are: the brief's six; `read_only_commit_is_at_start_ts`; four runner cases (retry on `Conflict`/`NotApplied`, `max_attempts`, no retry of `Fatal`/`AlreadyExists`, deadline); `fault_points_and_commit_tokens` (Task 20's fault table, moved here); and `barrier_holds_old_snapshots_and_refuses_below_safe_point` (Task 20's barrier test, moved here). `tikv_store_roundtrip` stays TiKV-only. `testing` gains `embedded()`, `embedded_factory(dir)`, `tikv_factory()`, `TempDir`, `TMPDIR_ENV` and its own `random_root`, which is no longer re-exported from `loams_tikv` but behaves the same. `stores()` now yields the embedded store first. Embedded test stores live under `LOAMS_TEST_TMPDIR` (default: the system temporary directory). `loams-kv`'s own tests use `CARGO_TARGET_TMPDIR`. | The Task 20 review asked for runner-level cases held by both runners | None |
| T21-11 | **There is no `embedded` cargo feature yet.** `redb` is a plain dependency of `loams-kv`, so T20-3 stands. Task 23 decides whether `live = [..., "loams-kv/embedded"]` gates it. | The plan's Architecture says embedded is always built | Task 23 adds the feature if the binary split needs it |
| T21-12 | **Reads run inline in the async methods.** They are short redb read transactions served from redb's cache. Commits and fsyncs run on the committer thread; oracle marks and GC run on blocking threads. A commit group is capped at 1 024 transactions. | Avoids a thread hop per read | A very large scan blocks a runtime worker for its duration |
| T21-13 | **T20-9 as built.** `LiveConfig.store: StoreConfig` and `LiveConfig::with_store` exist; `with_tikv` wraps `StoreConfig::Tikv`, and `LiveConfig::new` still builds TiKV until Task 23. `loams-live` re-exports `StoreConfig` and `EmbeddedConfig`. `LiveServer::start` opens whatever store the config names. Its error text is now "opening the Live store (keyspace …)" instead of "connecting to the Live keyspace …". `crates/loams` checks "same cluster" only for a TiKV store and sweeps only TiKV handles. `Server::live_swept_by_metastore_gc()` is `false` on an embedded store, which runs its own GC. | Ruling T20-9 | None |
| T21-14 | **Fix round 1: a read timestamp may run at most 1 s ahead of the embedded store's clock.** `Store::snapshot(at)` on embedded refuses `at` above `max(last issued, wall clock) + 1 s` with the new `KvError::TsAhead { at, limit }`, before it moves the oracle or persists a mark. `Ts::from_parts` saturates (no wrap) above `Ts::MAX_PHYSICAL_MS`, and `Ts::checked_from_parts` is new; the oracle's mark arithmetic is checked. Task 19 validates `QueryRequest.ts` at the service edge on both backends. | `QueryRequest.ts` reaches `Store::snapshot`: a client could push the persisted mark to `u64::MAX` and wedge the oracle | A caller that reads at a timestamp it got from another, faster clock more than 1 s ahead is refused |
| T21-15 | **Fix round 1: `Store::run` refuses `Mode::Pessimistic` on both backends** with `TxnError::Fatal(PESSIMISTIC_REFUSED)` before the body runs. The conformance case `pessimistic_mode_is_refused` holds both backends to it. The embedded lock table is removed, and `embedded::Handle::begin()` takes no mode. Supersedes T21-8. | Nothing uses pessimistic mode. Reads stay at the start timestamp, and the seam has no `get_for_update`, so a pessimistic read-modify-write could lose an update on either backend. | The first pessimistic caller adds `get_for_update` to the seam (TiKV: `loams_tikv::Txn::get_for_update`; embedded: a lock table again) |
| T21-16 | **Fix round 1: GC counts only open reads inside the read window, and never passes the oracle.** An open snapshot or transaction older than `now − (gc_life_time − 1 min)` no longer holds GC: it cannot read any more, and a barrier that covers it is counted on its own. The candidate safe point is capped at the last timestamp issued. `gc_once_at` is `#[doc(hidden)]` and test-only. Embedded `Snap` and `Txn` reads refuse a timestamp GC has passed, with `Fatal("read below the GC safe point")`, instead of answering from what is left. Since the read floor is a minute above `now − gc_life_time`, the open-read term now never lowers the safe point by itself. It stays as the plan's formula, and as a guard should the window change. | A leaked `Snap` pinned GC forever. A GC run ahead of the clock refused every read at the clock. | None |
| T21-17 | **Fix round 1: the registry keys an existing file by its full canonical path**, so a symlinked file is the file it names. The registry also remembers each file's database (`Weak<Core>`) beside its handles. A reopen while this process's previous database is still closing waits up to 30 s for it, then fails with "the store file … is still closing in this process". A file locked when no database of this process is closing fails at once with "… is open in another process", instead of after a blind 2 s retry. | Two paths to one file tripped redb's file lock; a reopen racing the close reported another process | None |

Task 22 (parity suites on both backends, 2026-10-09).

| # | Ruling | Why | Cost if wrong |
|---|---|---|---|
| T22-1 | **`live_test!` gives its body a `loams_live::testing::TestStore`, not a bare `Store`.** `TestStore` derefs to the `Store` and adds `store()`, `store_config()`, `live_config(app)`, `fresh_root()`, `backend()` and `dir()`. Besides `live_test!(name, \|store\| async { … })`, the shorthand `live_test!(name)` runs an `async fn name(store: TestStore)` in scope, so the migrated suites stay ordinary, rustfmt-formatted functions. The cases are `name::embedded` (always, on a fresh file under `LOAMS_TEST_TMPDIR`, else `CARGO_TARGET_TMPDIR`) and `name::tikv` (with `LOAMS_TEST_PD`). Both run on a multi-thread runtime with 4 workers; some tests had used 2. | A service test starts a `LiveServer`, which opens its store from a `LiveConfig`, so the test needs the store's config as well as the store. A runner needs a `LiveConfig` for the same store. | None |
| T22-2 | **`every_live_suite_runs_on_embedded` reads the test sources rather than `--list`.** A `#[test]` or `#[tokio::test]` that reaches a store is a failure unless it is on the `TIKV_ONLY` allowlist with a reason. "Reaches a store" means a store marker such as `Runner::open`, `LiveServer::start` or `testing::cluster`, directly or through a local helper (followed to a fixed point). The meta-test also requires a `live_test!` case in each of `docs`, `journal`, `service`, `session`, `subs` and `txn`, and at least 60 `::embedded` cases (there are 64). The allowlist is empty: the TiKV-only behaviours the plan names (TSO-stream loss, region errors, a PD stall) are tested in `loams-tikv` and `loams-kv`, not in Live's suites. `value.rs` opens no store, so nothing moved. | Each test binary lists only its own tests, so `--list` from one binary cannot see the others. A probe test reaching a store two helpers deep fails the meta-test. | A store reached through a helper in another file, or through a macro, is not followed |
| T22-3 | **No parity gap: no change to the embedded backend.** Every existing Live test passes on embedded unchanged in its assertions. The changes are of form only. `server_runs_on_an_embedded_store` (Task 21) became `server_runs_on_the_configured_store`, on both backends. The `index_scan_order_matches_value_order` proptest runs its synchronous runner on `spawn_blocking`, driving each case on the test's runtime. `idempotent_mutate_applies_once_across_lost_ack` draws each case's root with `fresh_root()`. In `non_loopback_bind_is_refused`, the "starts" half uses the test's store. Locals named `tikv` were renamed `kv`. | The brief: fix gaps in the backend, never the test | None |
| T22-4 | **CI job `live-embedded`.** It runs `cargo test -p loams-kv -p loams-live --locked` with `LOAMS_TEST_PD` empty and `LOAMS_TEST_TMPDIR=$RUNNER_TEMP`. It runs on a pull request when the `live-embedded` filter matches, and always on pushes and the nightly. The filter is the brief's paths plus `crates/loams-tuple/**` (a dependency of `loams-kv`) and the toolchain anchor. The job is in the `CI required` summary. | The brief, plus the seam's own dependency | None |
