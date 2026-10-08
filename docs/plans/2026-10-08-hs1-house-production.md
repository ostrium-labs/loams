# HS1 — Loams House to Production: Serverless ClickHouse-Compatible Analytics on the Bucket — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Each task lists the interfaces it must produce and the tests that must exist and pass before it is done. Where this plan gives exact values (names, ports, codes, settings, paths), use them verbatim. Where it gives a contract and named tests, write the code to that contract, and record any deviation in "Rulings made during execution" at the end of this file. The code is not pre-written in this plan.
>
> **Status: Planned** (2026-10-08). **Track HS** (design [§49](../design/49-loams-house-production.md), D760–D779, Q685–Q699; addendum to [§32](../design/32-loams-flow-fabric-house.md)). Branches `hs1-t<N>`, stacked per milestone; PRs target `dev`. HS1 **continues** [FL2](2026-10-01-fl2-house-sql.md) from where it stands (Tasks 0, 1 and 3 merged; Task 2 onward unbuilt) and absorbs FL2's remaining tasks as mapped by Task 0; it does not wait for [FL1](2026-10-01-fl1-fabric-foundation.md), whose Fabric is needed only for `realtime` tables (Task 14) and Iggy pipes (Task 18). **Owner gates:** Q685 (lake tables, amends D346), Q686 (front/worker split) and Q687 (native protocol now) block HS1b, HS1a Task 2 and HS1e Tasks 30–31 respectively; until answered, those tasks follow §49's proposal on a branch and do not merge.

**Goal:** Loams House generally available: ClickHouse SQL over Iceberg tables in the customer's bucket, executed by chDB in sealed worker processes behind a front that speaks ClickHouse HTTP and the native protocol, authenticates with Loams identities, commits Iceberg snapshots and meters per query — with the conformance, performance and security evidence of §49 §13–§16. Milestones:

| Milestone | Scope | Tasks | Exit |
|---|---|---|---|
| **HS1a** | Reconcile, spike, the worker process and its supervisor, the HTTP surface, classifier and sessions, the deny list and the OS sandbox, mounting and `loams.house.v1` | 0–8 (9) | `loams-fabric house --single-node` answers ClickHouse HTTP and `loams.house.v1` queries over `numbers()`/`values()` in sealed workers, and the engine's main port proxies `loams.house.v1` |
| **HS1b** | `house-cache`, the catalog, lake reads and writes, Replacing/Summing with compaction, realtime tables (when FL1 exists), retention and restore | 9–15 (7) | Lake tables of all Tier 1 engines are created, written, read, compacted, time-travelled and restored through HTTP, with every byte read through `house-cache` |
| **HS1c** | Ingest: `LoamsStream` pipes, materialized views, Iggy and `S3Queue` pipes, ingest faults | 16–19 (4) | A Loams stream lands exactly once in a lake table through an MV under kill tests |
| **HS1d** | Auth and TLS, governance, quotas and usage, observability and query log, autoscaling and scale-to-zero, HA/DR, packaging, upgrades, the desktop Analytics page | 20–28 (9) | Every §49 §10–§15, §17–§19 behaviour is tested on a kind cluster and on the single-node mode; the AP1e Analytics page runs on `loams.house.v1` |
| **HS1e** | `chsurface-2.0`, the native protocol, upstream stateless/ClickBench/TPC-H, the driver and BI matrix, performance gates, faults, security review, docs, GA | 29–38 (10) | "Exit criteria for production" all ticked |

**Architecture** (§49 §3):
- **The front** is `loams-fabric` (role `house`) in the `fabric/` workspace. It links no libchdb. It owns listeners, the MT1 verifier, the classifier, sessions, admission, the catalog, Iceberg commits (iceberg-rust 0.10 in an isolated crate), pipes, compaction scheduling, the query log and `house-cache`.
- **The worker** is a new binary `loams-house-worker` (crate `loams-house-worker`), the only thing that links `loams-chdb`. It speaks the `hsw1` frame protocol over a Unix socket, runs in a sealed sandbox and holds no credentials.
- **Storage** is Iceberg v2 via Lakekeeper (production) or the local SQLite catalog (single-node), files in the bucket under `ns/<id>/house/`. No MergeTree inside chDB, ever.

**Tech Stack:** Rust 1.97.1, edition 2024, the `fabric/` workspace's lints (`unsafe_code = "forbid"` except `loams-chdb-sys`, FL2 Ruling 8). Existing: `loams-chdb-sys`, `loams-chdb`, `loams-house` (errors), arrow 59, `axum` 0.8, `tokio`, `bytes`, `flate2`, `uuid`, connect-rust 0.9 and `connectrpc-build` 0.9. New (Task 0 checks each is at least 14 days old and passes `fabric/deny.toml`): `sqlparser` 0.63 (FL2 Ruling 4), `zstd`, `lz4_flex`, `cityhash-rs` (or the `clickhouse` crate's CityHash128 v1.0.2; Task 0 picks), `iceberg` 0.10.1 with its arrow 58 (isolated in `loams-house-lake`), `rusqlite` (local catalog), `landlock`, `seccompiler`, `nix` (namespaces, signals), `postcard`, `rustls` (TLS listeners), path dependencies on `loams-cache` and `loams-store` (engine workspace), MT1's verifier crate when it exists. Test tools: the reference `clickhouse/clickhouse-server` at chDB's ClickHouse version (digest-pinned, CI only), Lakekeeper 0.13.6 with Postgres 17 and RustFS in compose, toxiproxy 2.x, kind, Grafana and Metabase containers (CI only), Python 3.13 with `uv`, Go and JDK toolchains for driver suites.

**Spec:**
- [§49](../design/49-loams-house-production.md) (all of it) and D760–D779, Q685–Q699 in the [decision log](../design/13-decision-log.md).
- [§32](../design/32-loams-flow-fabric-house.md) §7–§10 and §12–§14; D342–D351, D413, D414.
- [FL2](2026-10-01-fl2-house-sql.md) (its Tasks, Rulings 1–10 while writing and 1–14 during execution) and [`fl2-dependency-spike.md`](fl2-dependency-spike.md); [FL1](2026-10-01-fl1-fabric-foundation.md) for the Fabric pieces.
- [§19](../design/19-console-identity-and-agents.md) §4–§5, [§38](../design/38-knative-authentik-gitops.md) and [MT1](2026-10-02-mt1-authentik-identity.md) (the verifier); [§44](../design/44-unified-api-and-sdks.md) §4–§8 (Connect conventions, catalogue); [§27](../design/27-usage-hooks.md); [§37](../design/37-desktop-and-mobile-apps.md) §19 and [AP1e](2026-10-08-ap1e-electron-desktop.md) (the desktop's plugin and sidecar conventions).
- As built: `fabric/Cargo.toml`, `fabric/crates/{loams-chdb-sys,loams-chdb,loams-house,loams-flow}`, `crates/loams-cache`, `crates/loams-store`, `crates/loams/src/api/connect.rs`, `proto/loams/{instance,operations,errors}/v1`.

## Global Constraints

- **Worktree and branch.** Work in `~/Documents/Ostriumlabs/loams-wt/hs1-<milestone>`, branch `feat/hs1-<milestone>` based on `dev`. `git commit -s` (DCO). Commit areas: `house`, `chdb`, `lake`, `native`, `conformance`, `proto`, `deploy`, `ci`, `docs`, `desktop`, `plugins`.
- **The build machine.** One cargo build at a time, with the shared target directory from `~/Documents/.cargo/config.toml`; never set `CARGO_TARGET_DIR`, never build in `/tmp`. libchdb is linked dynamically only (FL2 Ruling 2); stop the compose stacks during builds.
- **Never ClickHouse storage** (D762). No `MergeTree`-family table is created inside chDB; only session-local `Memory`/`Null` temporary tables and views. `no_mergetree_in_chdb` runs after every integration test file.
- **libchdb only in the worker** (D761). `cargo tree -p loams-fabric` must not contain `loams-chdb-sys`; a CI check asserts it. The `inproc` mode is a feature `inproc-worker` of `loams-fabric`, off in release images.
- **No credentials in the worker.** The worker's environment, arguments and frames never carry bucket keys or tokens. Rust types for secrets implement `Debug` as `"[redacted]"`.
- **Deny by default** (§49 §13). Every disabled function, engine, clause and setting has a test answering `344`, and every L3 sandbox rule has a test that bypasses L1 (a test-only `--unsafe-disable-deny-list` flag, compiled only under `cfg(test)`) and still fails.
- **Pinned versions move together**: chdb-core, the reference server and the corpus expected outputs, in `fabric/crates/loams-house/src/versions.rs` (FL2 Global Constraint). Stable chdb-core releases only, at least 14 days old.
- **Licences (D11).** Every new crate passes `cargo deny`. Grafana and Metabase (AGPL) only as unmodified CI containers. ClickBench query text is never committed (D414).
- **Loopback until TLS.** Plaintext listeners (8123, 9000) bind loopback only; non-loopback needs TLS (8443, 9440) and MT1's verifier (Task 20).

## Review Focus

1. **A query reads another namespace's data or the host** — by a qualified name, a crafted `icebergS3`/`s3`/`url`/`file`, a dictionary source, `INTO OUTFILE`, `format_schema`, or a key outside the worker's prefix at `house-cache`. Expected: `344` or `497`, and the L3 sandbox stops it even with L1 off. Tests: Task 5 `denied_everywhere_in_the_tree`, Task 6 `sandbox_blocks_with_deny_list_off`, Task 9 `proxy_refuses_foreign_prefix`, Task 11 `qualified_name_outside_namespace_is_60`.
2. **An acknowledged insert is lost or applied twice** — on a front crash mid-commit, a retried request, a commit conflict, or a pipe restart. Tests: Task 12 `ack_only_after_commit`, `retry_with_same_token_writes_once`, `concurrent_commits_all_land`; Task 16 `pipe_restart_is_exactly_once`; Task 19 `kill_front_during_commit_matrix`.
3. **A runaway query is not stopped** — timeout, cancel, `KILL QUERY`, memory blow-up. Expected: the worker is killed and the client gets `159`/`394`/`241` within 2 s of the limit. Tests: Task 2 `kill_is_the_cancel`, Task 21 `timeout_kills_worker`, `oom_is_241_not_a_crash_of_the_front`.
4. **A secret leaks** into a worker, a frame, a log line, an error, or the query log. Tests: Task 2 `worker_env_has_no_secret`, Task 23 `operator_logs_redact_literals`, Task 20 `api_key_never_logged`.
5. **The House surface lies about compatibility** — an allowlisted test that now passes, a surface entry with no test, or a result that differs from the reference without an entry. Tests: Task 29 `unexpected_pass_fails`, `every_supported_entry_has_tests`, the `house-conformance` CI job.

---

## File structure

```
fabric/crates/loams-house/            the front's library (FL2's crate, extended)
  src/{config,versions,http,auth,compress,classify,session,settings,admission,watchdog,limits,deny,surface}.rs
  src/{catalog/{mod,rest,local},types,ddl,read,views,insert,dedup,compact,retention,pipes/{mod,loams_stream,iggy,s3queue},mv}.rs
  src/{querylog,system,usage,metrics,api}.rs           api.rs: loams.house.v1 handlers
  tests/
fabric/crates/loams-house-proto/      generated loams.house.v1 (connectrpc-build)
fabric/crates/loams-house-lake/       Iceberg commits with iceberg 0.10 (arrow 58), Parquet footer stats; no other crate sees its Arrow types
fabric/crates/loams-house-cache/      the S3 read proxy over loams-cache, prefix scoping, signing
fabric/crates/loams-house-native/     sans-I/O native protocol codec + tokio server adapter; fuzz/
fabric/crates/loams-house-worker/     the worker binary: hsw1 frames, chDB execution, sandbox setup, forwarder
fabric/crates/loams-house-ipc/        hsw1 frame types and codec (shared by front and worker; no libchdb)
fabric/crates/loams-fabric/           the binary: loams-fabric {house|ingest|flow} …  (created here if FL1 has not)
fabric/crates/loams-house-conformance/  FL2 Task 10's harness, extended to native and lake/realtime classes
proto/loams/house/v1/house.proto
crates/loams/src/api/house_proxy.rs   the main port's proxy of /loams.house.v1.* and the catalogue row
conformance/clickhouse/{allowlist.toml,surface.toml,corpus/**,stateless/manifest.toml,clickbench/,tpch/,drivers/,bi/}
deploy/house/{compose.yaml,kind/,helm/loams-house/}
scripts/house/{reference.sh,clickbench.sh,tpch.sh,drivers_*.sh,bench/*}
docs/house/{index,quickstart,surface (generated),tables,ingest,security,operations,limits,performance,desktop}.md
web/plugins/analytics/                 the desktop Analytics page (Task 28; AP1e conventions)
```

## Shared contracts (all tasks use these names)

- **Ports**: HTTP and Connect `8123` (plaintext, loopback) / `8443` (TLS); native `9000` (plaintext, loopback) / `9440` (TLS); admin and metrics `8125` (loopback by default); `house-cache` on an ephemeral loopback port reached only through forwarders.
- **Flags**: `--house-listen`, `--house-tls-listen`, `--native-listen`, `--native-tls-listen`, `--admin-listen`, `--single-node`, `--workers=process|inproc`, `--catalog=rest:<url>|local:<path>`, `--store=<url>`.
- **Settings** (Loams): `loams_table_class` (`lake`|`realtime`), `loams_snapshot_id`, `loams_as_of`, `loams_lake_commit_interval_ms` (1000), `loams_lake_commit_bytes` (268435456), `loams_lake_target_file_bytes` (268435456), `loams_dedup_window` (1000), `loams_pipe_max_rows` (1000000), `loams_pipe_max_delay_ms` (2000), `loams_wake_timeout_ms` (30000), `loams_consistency_wait_ms` (5000, realtime), `loams_consistency_token`, `loams_bucket_key`, `loams_tiering_freshness`.
- **Response headers** (in addition to FL2 Task 2's): `X-Loams-Snapshot-Id` (comma-separated `table=id` after a lake write), `X-Loams-Consistency-Token` (realtime), `X-Loams-Worker` (opaque id, debugging).
- **Frames `hsw1`** (`loams-house-ipc`): `Bind`, `Execute`, `Input`, `InputEnd`, `Chunk`, `Progress`, `Stats`, `Error`, `Done`, `Ready` (§49 §4.2); `u32` length prefix, postcard bodies, a version byte; unknown version → the worker exits with code 70.
- **Worker exit reasons** (`loams_house_worker_kills_total{reason}`): `budget`, `idle`, `cancel`, `timeout`, `rss`, `oom`, `poisoned`, `crash`, `drain`.
- **Catalog trait** (`loams-house::catalog`): `HouseCatalog { list_databases, create_database, drop_database, list_tables, load_table, create_table, drop_table, rename_table, update_schema, commit(table, requirements, updates), register_table }`; implementations `RestCatalog` (Lakekeeper), `LocalCatalog` (SQLite CAS), `MemCatalog` (tests); one conformance macro `house_catalog_conformance!`.
- **Table class**: `TableClass { Lake, Realtime, External }`.
- **`reason` values** (appended to `docs/api/reasons.md`): those of §49 §18.2.
- **Scopes**: `house:read`, `house:write`, `house:ddl`, `house:admin`.

---

## HS1a — The front, the worker and the HTTP surface

### Task 0: Reconcile with FL1, FL2 and the code as built

**Files:** this plan's "Rulings made during execution" only.

Answer and record, with paths and commits, as rulings R0.N:
1. FL2 as built: which tasks merged (expected 0, 1, 3), what is open on `fl2-*` branches (`fl2-arrow-ruling`, `fl2-t0-chdb-spike`, `fl2-t3-errors`), and the final state of FL2 Rulings 10–14. Map every remaining FL2 task to its HS1 task: FL2 2 → HS1 3; 4 → 4; 5 → 10 (lake) and 14 (realtime); 6 → 12 and 14; 7 → 11 and 14; 8 → 5 and 11; 9 → 29; 10 → 29; 11 → 32; 12 → 33; 13 → 35. Record that FL2's plan should be marked "Absorbed into HS1" from its Task 2 (the controller edits FL2's status and README row).
2. FL1 as built: does `fabric/crates/loams-fabric` exist? `deploy/fabric/`? `loams-fabric-envelope`? If not, HS1 Task 7 creates the `loams-fabric` binary crate with only the `house` role, and Tasks 14 and 18 stay blocked on FL1.
3. `loams-cache` and `loams-store` as path dependencies of the `fabric/` workspace: their dependency graphs (`object_store`, `foyer`, `tokio`, any `arrow`) unify with the Fabric's; a one-crate probe builds.
4. MT1's state: does a verifier crate exist? If not, Task 20 defines a `TokenVerifier` trait with a static-key test implementation and wires MT1's later.
5. The engine's Connect catalogue (`crates/loams/src/api/connect.rs` `CATALOGUE`, `route_map.rs`'s check against §44) — what adding `loams.house.v1` requires (a §44 catalogue line; a proxy route rather than a local service).
6. AP1e as built: the plugin layout under `web/plugins/`, the sidecar supervisor's API in `apps/desktop-electron/src/main/`, the write-confirm dialog and the agent tool registry Task 28 must reuse; record the names.
7. Pins for every new dependency of the Tech Stack (at least 14 days old, `deny.toml`), and the chdb-core stable release to pin (v26.9.0 or newer stable).

Commit: `docs(hs1): task 0 rulings`.

### Task 1: Spike and measurements (no product code)

**Files:** `docs/house/performance.md` (measured numbers only), `docs/plans/hs1-spike.md`, this plan's rulings.

Measure and record, with commands and outputs:
- Worker boot: process spawn + libchdb load + `chdb_connect` + first `SELECT 1`, p50/p95 over 50 runs; idle RSS (private and shared) of one, four and sixteen workers.
- chDB reading Iceberg through a loopback HTTP endpoint (a stub S3 server): `icebergS3` with a custom `http://127.0.0.1:<p>/` endpoint and dummy credentials; `iceberg_metadata_file_path` pinning; partition and min/max pruning; v2 position and equality deletes; schema evolution (added, renamed, widened columns); `timestamptz` and `timestamp_ns`.
- That chDB caches nothing on disk with no `filesystem_caches_path`; which in-memory metadata caches exist and their settings.
- chDB's Iceberg writer at the pinned version (does `INSERT INTO icebergS3` work; is it experimental) — evidence for Q688.
- chDB access control: can the worker's user have source privileges revoked (`REVOKE READ ON URL` etc.)? Does `INTO OUTFILE` write a server-side file in chDB? `user_files_path` behaviour when empty.
- `FORMAT Native` output framing, and whether `chdb_stream_insert` accepts a `Native` and a `Parquet` body without a temp file (decides `Input` handling).
- Unprivileged user and network namespaces, Landlock ABI version and seccomp on the build machine, in a kind node, and in a stock GKE/EKS node image (record "unknown" where not reachable).
- `iceberg` 0.10.1 + arrow 58 inside the `fabric/` workspace: build time and binary size delta.

*Exit:* rulings that confirm or change §49 §4, §6.1, §7 and §13.2. **If chDB cannot read Iceberg through a custom endpoint, stop and raise it to the owner** (it changes D766).

### Task 2: The worker binary, the `hsw1` protocol and the supervisor

**Files:** `fabric/crates/loams-house-ipc/**`, `fabric/crates/loams-house-worker/**`, `fabric/crates/loams-house/src/{watchdog,admission}.rs` (supervisor half), tests.

**Produces:** `loams-house-ipc::{Frame, FrameCodec, PROTOCOL_VERSION}`; the worker's `main` (connects to the socket passed as fd 3, sends `Ready`, waits for `Bind`, then serves `Execute`); `WorkerPool { acquire(ns) -> WorkerLease, release(lease, outcome), kill(lease, reason), stats() }` with warm idle workers, namespace binding and the recycle rules of §49 §10.1; an `InprocWorker` behind feature `inproc-worker`.

*Tests:* `frames_roundtrip_property` (proptest over every frame); `worker_answers_select_one_in_every_format` (TSV, CSV, JSONEachRow, RowBinary, Native, Parquet, Arrow, ArrowStream; FL2 Task 1's list); `kill_is_the_cancel` (`SELECT count() FROM numbers(1e12)`, cancel → client answer 394 within 1 s, worker gone, pool replenished); `crash_does_not_reach_the_front` (a test-only frame that makes the worker `abort()`; the query answers 210, other workers keep serving); `binding_is_exclusive` (a worker bound to ns A never receives ns B); `recycle_after_budget`; `worker_env_has_no_secret` (inspects `/proc/<pid>/environ` and argv); `no_libchdb_in_front` (`cargo tree` check in CI); `no_hanging_ffi_paths_called` (grep-level test that `chdb_arrow_scan` and streaming Arrow are referenced only in `loams-chdb` tests).

### Task 3: The ClickHouse HTTP interface (FL2 Task 2, on the pool)

**Files:** `fabric/crates/loams-house/src/{lib,config,http,auth,compress}.rs`, `tests/http.rs`.

FL2 Task 2's contract verbatim (parameters, GET is read-only, header order, `X-ClickHouse-*` response headers, compression), executed through `WorkerPool` instead of an in-process session, plus: progress headers emitted during the query when `send_progress_in_http_headers = 1` (from `Progress` frames, at most every 100 ms); `wait_end_of_query = 1` buffers to a temp file capped at 1 GiB; mid-stream errors per FL2 Ruling 9 (`MidStreamBody`, built). Auth in this task is FL2's dev `UserMap`; Task 20 replaces it.

*Tests:* FL2 Task 2's eleven tests, plus `progress_headers_stream`, `wait_end_of_query_buffers`, `mid_stream_error_matches_reference`.

### Task 4: Classifier, sessions and settings (FL2 Task 4)

**Files:** `fabric/crates/loams-house/src/{classify,session,settings}.rs`, `tests/classify.rs`.

FL2 Task 4's contract with FL2 Ruling 6 (sqlparser for the statements Loams owns, `chdb_classify_query_n` for the negative half — the classify call runs in a worker, since the front has no libchdb: Task 0 confirms a cheap `Execute` of kind `Classify` or moves the call into a pre-warmed classifier worker). Additions: `Stmt` gains `CreatePipe`, `CreateMaterializedView`, `KillQuery`, `Undrop`, `Rename`; sessions per §49 §10.3, including `TemporaryTable` pinning a worker (`max_pinned_workers_per_namespace`, default 2, else 202); session affinity key exported for the load balancer (`X-Loams-Session-Affinity`).

*Tests:* FL2 Task 4's ten tests, plus `temporary_table_pins_a_worker`, `third_pinned_worker_is_202`, `session_survives_worker_recycle_without_temp_tables`.

### Task 5: The deny list (L1) and chDB's controls (L2)

**Files:** `fabric/crates/loams-house/src/deny.rs`, `fabric/crates/loams-house-worker/src/settings.rs`, `tests/deny.rs`, `conformance/clickhouse/corpus/deny/**`.

**Produces:** `DENIED_TABLE_FUNCTIONS`, `DENIED_ENGINES`, `DENIED_FUNCTIONS`, `DENIED_CLAUSES` (`INTO OUTFILE`, `FROM INFILE`), `DENIED_SETTINGS` (`format_schema*`, `user_files_path`, path-valued settings), `HOST_FUNCTIONS` (rewritten or 344); `check(tree: &QueryTree) -> Result<(), ChError>` over the worker's `EXPLAIN QUERY TREE` output; the worker's fixed L2 settings of §49 §13.2.

*Tests:* `denied_everywhere_in_the_tree` (each denied item in a subquery, view, CTE, `JOIN`, `IN`, `INSERT … SELECT`, `CREATE … AS SELECT`, `WITH`, and through `merge()`); `into_outfile_is_344`; `format_schema_setting_is_164`; `create_function_executable_is_344`; `dictionary_http_source_is_344`; `allowed_table_functions_work` (`numbers`, `numbers_mt`, `zeros`, `values`, `generateRandom`, `format`, `null`, `view`, `merge` over own tables); `host_functions_return_loams_values`.

### Task 6: The worker sandbox (L3)

**Files:** `fabric/crates/loams-house-worker/src/sandbox/{mod,netns,landlock,seccomp,cgroup}.rs`, `fabric/crates/loams-house-worker/src/forwarder.rs`, `tests/sandbox.rs`, `deploy/house/kind/worker-pods.yaml` (fallback).

**Produces:** `Sandbox::apply(cfg)` run by the worker before loading libchdb (`dlopen` after sandboxing, or before with Landlock rules covering its path — Task 1 decides); the forwarder process (no credentials) relaying `127.0.0.1:<p>` inside the netns to an inherited Unix socket; cgroup v2 child per worker (`memory.max`, `cpu.max`, `pids.max`) when the front owns a delegated cgroup; the worker-pods fallback selected by `--sandbox=netns|pods|none` (`none` only with `--single-node` on macOS or when explicitly forced, logged at start).

*Tests* (Linux; skipped with a reason elsewhere): `sandbox_blocks_with_deny_list_off` (with L1 disabled under `cfg(test)`: `url('http://<test server on host>')`, `s3(…)`, `remote(…)`, `file('/etc/passwd')`, `INTO OUTFILE '/tmp/x'`, `executable(…)` all fail); `worker_reaches_only_its_forwarder`; `worker_cannot_exec`; `worker_cannot_write_outside_tmp`; `cgroup_limits_applied`; `pods_fallback_network_policy` (kind: a worker pod's egress to anything but the front's forwarder port times out).

### Task 7: The `loams-fabric` binary, the `house` role and the engine's proxy

**Files:** `fabric/crates/loams-fabric/**` (create if FL1 has not), `fabric/crates/loams-house/src/config.rs`, `crates/loams/src/api/{house_proxy.rs,connect.rs}`, `crates/loams/src/config.rs` (`[house] endpoint`), `docs/design/44-unified-api-and-sdks.md` catalogue line (one row), tests.

**Produces:** `loams-fabric house` with the flags of the Shared contracts and a TOML config (`[house]`, `[house.limits]`, `[house.pool]`, `[house.catalog]`, `[house.store]`, `[house.tls]`); `--single-node` defaults (§49 §17); the engine's main port forwards `/loams.house.v1.*` to `[house] endpoint` with the caller's `Authorization` header, streaming both ways, and the catalogue row `loams.house.v1` (`available` when the endpoint's health check passes; `unstable: true`).

*Tests:* `house_role_starts_and_serves_ping`; `single_node_defaults`; `non_loopback_plaintext_refused` (FL2's message); engine side `house_proxy_streams_and_preserves_auth`, `catalogue_lists_house_only_when_healthy`, `house_absent_answers_feature_not_configured` (`house_not_configured`).

### Task 8: `loams.house.v1` protos and `HouseService`

**Files:** `proto/loams/house/v1/house.proto`, `fabric/crates/loams-house-proto/**`, `fabric/crates/loams-house/src/api.rs`, `fabric/crates/loams-house/tests/api.rs`, `docs/api/reasons.md`.

**Produces:** every RPC of §49 §18.2 with AP0 conventions (`NO_SIDE_EFFECTS` on reads, `idempotency_key` on `RestoreTable`, AIP-158 pages); `ExecuteQuery` runs through the same classifier, deny list, admission and pool as HTTP, emitting `header`, `batch`, `progress`, `summary`; `ARROW_IPC` batches are chDB's `ArrowStream` bytes split at message boundaries; `JSON_ROWS` is `JSONCompactEachRow`. Catalog-backed RPCs (`ListTables`, `DescribeTable`, `ListQueryHistory`) answer from fixtures until Tasks 10 and 23 land, with an `unimplemented` reason where no data exists yet.

*Tests:* `execute_query_arrow_and_json_agree`; `max_rows_truncates_and_says_so`; `read_only_refuses_writes_with_reason`; `cancel_query_kills_worker`; `errors_carry_clickhouse_code_metadata`; `buf_lint_and_breaking` (CI); `api_and_http_share_the_deny_list`.

---

## HS1b — Storage on the bucket

### Task 9: `house-cache`, the S3 read proxy

**Files:** `fabric/crates/loams-house-cache/**`, tests.

**Produces:** `HouseCache::serve(listener, store_resolver, cache: RangeCache)`; S3 `GetObject` (ranges), `HeadObject`, `ListObjectsV2` (path style), each request bound to a worker identity from its forwarder socket and a `Grant { ns, prefixes: Vec<String> }` set by `Bind`; reads through `loams-cache`'s `RangeCache`; upstream signing with the namespace's credentials from `[house.store]` (or vended STS credentials where configured).

*Tests:* `get_range_head_list_match_upstream` (RustFS); `proxy_refuses_foreign_prefix` (403 and a metric); `cached_bytes_are_verified` (corrupt a cached block → re-fetch, never served); `cache_survives_worker_recycle`; `no_credentials_reach_the_worker` (inspect requests the worker side sends: no `Authorization` with real keys); `chdb_reads_through_proxy_only` (RustFS with toxiproxy cut for everything but the front: queries still succeed through the proxy).

### Task 10: The catalog and DDL for lake tables

**Files:** `fabric/crates/loams-house/src/{catalog/{mod,rest,local},types,ddl}.rs`, `fabric/crates/loams-house-lake/**` (schema and table metadata parts), tests.

**Produces:** the `HouseCatalog` trait (Shared contracts) with `RestCatalog` (Lakekeeper REST, nested namespaces per §49 §5.3 or the flat fallback Task 1 recorded), `LocalCatalog` (SQLite, CAS on the metadata pointer) and `MemCatalog`; DDL for lake tables (§49 §6.3) with FL2 Task 5's type table restated for Iceberg; `loams.ch.*` table properties; `external` tables discovered from the catalog; `SHOW CREATE`, `DESCRIBE`, `EXISTS`, `RENAME`, `UNDROP`.

*Tests:* `house_catalog_conformance!` for all three implementations; FL2 Task 5's tests on the lake class (`create_each_tier1_engine`, `type_mapping_roundtrip`, `partition_by_supported_transforms`, `partition_by_other_is_36`, `unsupported_type_is_48`, `alter_add_column`, `drop_database_cascades`); `external_tables_listed_read_only` (a table created by another writer is listed, `INSERT` is 497); `undrop_within_retention`; `local_catalog_tables_register_into_lakekeeper`.

### Task 11: Lake reads and system tables

**Files:** `fabric/crates/loams-house/src/{read,views,system}.rs`, tests.

**Produces:** per-query pinning (`ReadPin { table, snapshot_id, metadata_file }`), the view DDL of §49 §7 (through `house-cache`, ClickHouse types restored, `FINAL` merge views for Replacing and Summing), `loams_snapshot_id` and `loams_as_of` time travel; system tables (FL2 Task 8's list and columns, from the catalog, namespace-scoped; `system.parts` one row per data file; `system.loams_pipes` placeholder for Task 16).

*Tests:* `pin_is_stable_while_commits_land`; `time_travel_by_snapshot_and_timestamp`; `qualified_name_outside_namespace_is_60`; `external_table_reads`; `system_tables_list_namespace_tables_only`; `system_tables_engine_is_clickhouse_engine`; `system_columns_types_are_clickhouse_types`; `differential_select_corpus_on_lake` (the FL2 `select/` corpus against the reference loaded with the same rows).

### Task 12: Lake writes

**Files:** `fabric/crates/loams-house/src/{insert,dedup}.rs`, `fabric/crates/loams-house-lake/src/{append,stats}.rs`, tests.

**Produces:** §49 §6.1: body → worker → Parquet files (cast, defaults, sort, split by size and partition) → upload through `loams-store` → `DataFile` stats from footers → `fast_append` with `assert-ref-snapshot-id` and rebase-retry; group commit per table (`loams_lake_commit_interval_ms`, `loams_lake_commit_bytes`); `async_insert`/`wait_for_async_insert` semantics; deduplication by token or block hash with summaries `loams.dedup.<n>`; `X-ClickHouse-Summary` and `X-Loams-Snapshot-Id`; `INSERT … SELECT` and `CREATE TABLE … AS SELECT`.

*Tests:* `insert_every_declared_format_lands_identical_rows` (FL2 Task 6's list); `ack_only_after_commit` (a fault injected between upload and commit: the client gets an error, the files are orphans for Task 15, no rows visible); `retry_with_same_token_writes_once`; `block_hash_dedup_window`; `insert_deduplicate_0_writes_twice`; `concurrent_commits_all_land` (16 writers × 100 inserts, all rows present once); `group_commit_coalesces` (≤ 1 commit per interval per table); `wait_for_async_insert_0_acks_early`; `large_insert_is_streamed_with_bounded_rss` (5 GiB body); `partition_split_files`.

### Task 13: Replacing and Summing on lake tables; compaction

**Files:** `fabric/crates/loams-house/src/compact.rs`, `fabric/crates/loams-house-lake/src/rewrite.rs`, `conformance/clickhouse/corpus/engines/lake/**`, tests.

**Produces:** `FINAL` merge views (Task 11) verified; the compaction scheduler (thresholds: ≥ 16 files under 32 MiB in a partition, or unmerged-row ratio ≥ 20 % for Replacing/Summing), compaction runs in the namespace's worker pool, `replace` commits with `assert-ref-snapshot-id`, abandoned on conflict; `OPTIMIZE TABLE t [PARTITION p] FINAL [DEDUPLICATE]` runs it synchronously.

*Tests:* `replacing_final_equals_reference_final`; `replacing_without_final_may_show_duplicates_like_reference` (compared in FL2's `Merged` mode and in unmerged multiset mode); `summing_final_equals_reference`; `is_deleted_dropped_by_final_and_compaction`; `optimize_final_compacts_and_answers_after_commit`; `compaction_conflict_is_abandoned_not_lost`; `compaction_preserves_rows_property` (proptest: any insert sequence, any compaction schedule → `FINAL` result unchanged).

### Task 14: Realtime tables (FL2 Tasks 5–7, Fluss half) — blocked on FL1

**Files:** `fabric/crates/loams-house/src/{catalog/fluss.rs,realtime.rs,token.rs}`, tests.

FL2 Tasks 5–7 for `loams_table_class = 'realtime'`, unchanged in contract (DDL to Fluss, `INSERT` with `fluss-rs`, union reads, consistency tokens per FL2 Ruling 8, the `FL-R5` staleness fallback). Without FL1's stack, the task delivers only `realtime_without_fabric_is_36` and the surface marks `realtime` as `Planned("FL1")`.

*Tests:* FL2 Task 7's list when FL1 exists; `realtime_without_fabric_is_36` always.

### Task 15: Retention, orphan files and restore

**Files:** `fabric/crates/loams-house/src/retention.rs`, `fabric/crates/loams-house-lake/src/{expire,orphans}.rs`, `fabric/crates/loams-house/src/api.rs` (`RestoreTable`), tests.

**Produces:** snapshot expiry after `loams_snapshot_retention` (default 7 d, minimum 24 h), orphan-file deletion (files under the table location not referenced by any retained snapshot and older than 3 d), `DROP` purge after retention, `RestoreTable` as a `loams.operations.v1` operation (`set-ref` to a snapshot), `register_table` reconciliation for DR.

*Tests:* `expired_snapshots_free_files_only_when_unreferenced`; `orphan_gc_never_deletes_referenced_files` (property test over random commit/compact/expire schedules); `restore_table_to_snapshot_and_timestamp`; `restore_is_idempotent_by_key`; `dr_register_reconciles_newer_metadata`.

---

## HS1c — Ingest

### Task 16: `LoamsStream` pipes

**Files:** `fabric/crates/loams-house/src/pipes/{mod,loams_stream}.rs`, tests.

**Produces:** `CREATE TABLE q (…) ENGINE = LoamsStream('<stream>', '<format>'[, '<consumer>'])` (formats: `JSONEachRow`, `CSV`, `CloudEvents`, `Avro` if chDB supports it, else 48); a pipe runner per `(pipe, MV)` in the front that fetches batches from the engine's stream API (M0.3 fetch; `loams.stream.v1` when API1 lands, Task 0 records which), runs the MV's `SELECT` over each batch in a worker, and commits rows plus `loams.pipe.<mv>.<partition>` offsets in one snapshot; `system.loams_pipes`; `ListPipes`.

*Tests:* `pipe_lands_every_record_once`; `pipe_restart_is_exactly_once` (kill the front at every step of a batch); `two_runners_one_wins` (the loser's commit fails its requirement and it re-reads offsets); `cloudevents_columns_layout` (§32 §5.4); `pipe_lag_metric`.

### Task 17: Materialized views

**Files:** `fabric/crates/loams-house/src/mv.rs`, `conformance/clickhouse/corpus/mv/**`, tests.

**Produces:** `CREATE MATERIALIZED VIEW mv TO t AS SELECT … FROM src` for `src` a pipe or a lake table; on every committed insert into `src`, each MV's `SELECT` runs over the inserted batch and commits into `t`, recording `loams.mv.<mv>.src_snapshot`; `DROP VIEW`; `POPULATE` answers 48 (as ClickHouse recommends against it; documented).

*Tests:* `mv_per_block_matches_reference`; `mv_retry_does_not_double_apply`; `mv_chain_two_levels`; `mv_failure_does_not_fail_source_insert` (ClickHouse's `materialized_views_ignore_errors = 0` default makes it fail; Task 0 records the 26.9 default and the test follows it).

### Task 18: Iggy and `S3Queue` pipes — Iggy half blocked on FL1

**Files:** `fabric/crates/loams-house/src/pipes/{iggy,s3queue}.rs`, tests.

**Produces:** `ENGINE = IggyTopic('<stream>', '<topic>', '<format>')` through the Iggy SDK with offsets in snapshots (when FL1 exists); `ENGINE = S3Queue('<path under ns/<id>/>', '<format>')` with processed object keys in snapshot summaries, refusing paths outside the namespace prefix (344).

*Tests:* `s3queue_processes_each_object_once`; `s3queue_outside_prefix_is_344`; `iggy_pipe_exactly_once` (FL1 only).

### Task 19: Ingest fault evidence

**Files:** `fabric/crates/loams-house/tests/ingest_faults.rs`, `deploy/house/compose.yaml` (toxiproxy on the bucket and the catalog).

*Tests:* `kill_front_during_commit_matrix` (kill at upload, before commit, after commit before ack, after ack: rows exactly once after client retry with token); `catalog_outage_during_pipe` (pipe stalls, resumes, no loss); `bucket_503_bursts_during_insert`; `slow_catalog_commit_timeouts_are_retried`.

---

## HS1d — Auth, governance and operations

### Task 20: Authentication, authorization and TLS

**Files:** `fabric/crates/loams-house/src/auth.rs` (replacing the dev `UserMap` outside `--single-node`), TLS in `http.rs` and the native server, tests.

**Produces:** MT1's verifier (or Task 0's `TokenVerifier`) on HTTP, Connect and native; credential mapping of §49 §14; scopes; `Authorizer` checks for `house_database`/`house_table`; protected-environment rules; `system.tables` filtered by visibility; TLS listeners 8443 and 9440 with `rustls`; ClickHouse access DDL → 48; audit events.

*Tests:* `bearer_basic_header_and_param_forms`; `native_hello_password_carries_key`; `token_for_other_env_is_497`; `read_scope_is_readonly`; `table_grant_hides_table`; `access_ddl_is_48`; `plaintext_off_loopback_refused`; `api_key_never_logged`; `audit_on_ddl_and_kill`.

### Task 21: Resource governance

**Files:** `fabric/crates/loams-house/src/{limits,admission,watchdog}.rs`, tests.

**Produces:** the table of §49 §12: per-query caps applied as chDB settings and cgroup limits, the watchdog (kill at limit + 2 s), per-namespace queues, node memory admission, decompressed-body caps, the `164`-on-over-cap rule, `KILL QUERY` semantics.

*Tests:* `timeout_kills_worker` (159 within limit + 2 s); `oom_is_241_not_a_crash_of_the_front`; `queue_then_202`; `node_admission_by_memory`; `setting_over_cap_is_164`; `zip_bomb_is_36`; `kill_query_own_and_admin`.

### Task 22: Quotas and usage events

**Files:** `fabric/crates/loams-house/src/{usage,quota}.rs`, tests.

**Produces:** per-namespace quotas (§49 §15) from config now and from the control plane later (a `QuotaSource` trait); `house.query`, `house.insert`, `house.storage`, `house.compaction` events through §27's hook interface.

*Tests:* `quota_rate_and_concurrency_202`; `quota_scan_bytes_per_day`; `usage_event_per_query_has_cpu_and_bytes`; `storage_event_daily`.

### Task 23: Observability and the query log

**Files:** `fabric/crates/loams-house/src/{metrics,querylog}.rs`, `deploy/house/dashboards/*.json`, `deploy/house/alerts.yaml`, tests.

**Produces:** the metrics of §49 §15; OTLP spans; operator logs with literal redaction; the `_house.query_log` lake table written in 5 s group commits; `system.query_log` and `system.processes` over it; `ListQueryHistory` and `GetQuery` from it.

*Tests:* `metrics_exposed_with_names`; `trace_spans_link_front_worker_cache`; `operator_logs_redact_literals`; `query_log_row_per_query`; `history_scope_mine_vs_namespace`; `query_log_retention`.

### Task 24: Autoscaling and scale to zero

**Files:** `deploy/house/helm/loams-house/**` (Deployment, HPA or KEDA ScaledObject, PDB, NetworkPolicies, ServiceMonitor), `fabric/crates/loams-house/src/pool.rs` (wake and dedicated pools), kind tests.

**Produces:** §49 §10.2: shared pool min 1 per zone; dedicated pools at zero pods with the front's wake queue; `idle_unbind_after`; `loams_house_queue_depth` for the autoscaler.

*Tests (kind):* `namespace_idle_has_no_workers`; `dedicated_pool_wakes_from_zero` (records p95); `wake_timeout_is_202`; `scale_out_on_queue_depth`; `drain_finishes_running_queries`.

### Task 25: HA and disaster recovery

**Files:** `docs/house/operations.md` (runbooks), `scripts/house/dr_drill.sh`, kind tests.

*Tests (kind):* `front_replica_loss_only_fails_its_queries`; `session_affinity_survives_scaling`; `catalog_failover_cnpg`; `dr_drill_restores_catalog_and_reconciles` (CNPG PITR, then `register_table`); nightly job `house-dr-drill`.

### Task 26: Packaging and platforms

**Files:** `release/house-images.toml`, `deploy/house/Containerfile.{front,worker}`, `.github/workflows/house-release.yml`, `NOTICE`, `fabric/crates/loams-chdb-sys/build.rs` (macOS tarballs and digests), `docs/house/licensing.md`.

**Produces:** images `loams-house-front` and `loams-house-worker` (linux/amd64, linux/arm64) with SBOMs; libchdb digests for linux-x86_64, linux-aarch64, macos-arm64, macos-x86_64; the desktop bundle (`loams-fabric`, `loams-house-worker`, `libchdb`) per Linux and macOS platform with a signed manifest (the desktop key, Q494).

*Tests:* `images_pinned_by_digest`; `front_image_has_no_libchdb`; `macos_builds_link` (CI on macOS runners if Q620 adds them, else recorded manual); `no_agpl_in_dependency_graph` (`cargo deny` over the Fabric).

### Task 27: Upgrades

**Files:** `docs/house/operations.md` (upgrade section), `scripts/house/bump_chdb.sh`, CI.

**Produces:** the one-PR bump procedure (chdb-core stable ≥ 14 days, reference image, corpus outputs, ABI test, full conformance and perf), rolling worker replacement by drain.

*Tests:* `abi_symbols_match_header` (the 52-symbol check of the spike, run against any new `.so`); `rolling_upgrade_without_failed_queries` (kind: queries during a worker image change see no error but retries).

### Task 28: The desktop Analytics page (AP1e plugin)

**Files:** `web/plugins/analytics/**` (AP1e conventions recorded in Task 0), `apps/desktop-electron/src/main/house/**` (on-demand install and sidecar), the agent tool `house_query`, Vitest and Playwright tests. Runs after AP1e Part 2 has merged; coordinates file ownership with AP1e's controller.

**Produces:** the page of §49 §18.3 on `loams.house.v1` (SQL console with ClickHouse keywords and `system.functions`, results grid with ClickHouse types, progress, cancel, write-confirm, table browser with class badges, history, CSV/Parquet export); "Install the analytics engine" (download by digest, verify the manifest signature, supervise `loams-fabric house --single-node` beside `loams dev`); Windows shows remote-only.

*Tests:* `page_hidden_without_house_service`; `console_runs_read_only_by_default`; `write_requires_confirmation`; `history_loads_into_editor`; `browser_tree_from_list_tables`; `agent_tool_is_read_only`; `install_verifies_digest_and_signature`; Playwright `_electron` smoke: install on Linux, create a lake table, insert, query, see it in history.

---

## HS1e — Conformance, drivers, performance and GA

### Task 29: `chsurface-2.0` in code and the harness (FL2 Tasks 9–10)

**Files:** `fabric/crates/loams-house/src/surface.rs`, `conformance/clickhouse/{surface.toml,allowlist.toml,corpus/**}`, `fabric/crates/loams-house-conformance/**`, `docs/house/surface.md` (generated).

FL2 Tasks 9 and 10's contracts, with `SURFACE_VERSION = "2.0"`, an `Interface` axis (`Http`, `Native`, `Connect`) and a `TableClass` axis on every entry; the ~400-case seed corpus plus `lake/`, `mv/`, `pipes/`, `time_travel/`, `native/` areas; every case runs over HTTP and native (once Task 31 lands) against the reference.

*Tests:* FL2 Task 9 and 10's tests (`page_matches_code`, `every_supported_entry_has_tests`, `allowlist_validation`, `unexpected_pass_fails`, …) and the blocking CI job `house-conformance`.

### Task 30: The native protocol codec

**Files:** `fabric/crates/loams-house-native/src/{codec,packets,block,compress,varint}.rs`, `fuzz/`, tests.

**Produces:** a sans-I/O codec for client and server packets at the pinned revision, `Native` block (de)framing, LZ4/ZSTD compressed frames with CityHash128 checksums; `opensrv-clickhouse` consulted as a reference only.

*Tests:* `packets_roundtrip_property`; `captured_sessions_decode` (captures of `clickhouse-client`, `clickhouse-go` native and `clickhouse-driver` against the reference, under `conformance/clickhouse/drivers/native/`); `checksum_mismatch_is_rejected`; 1 h fuzz in CI nightly, 24 h before GA.

### Task 31: The native server

**Files:** `fabric/crates/loams-house-native/src/server.rs`, `fabric/crates/loams-house/src/native.rs`, tests.

**Produces:** listeners 9000/9440 on the codec; handshake, `Query` (settings, parameters, external tables), `Data` in for `INSERT`, `Data`/`Progress`/`ProfileInfo`/`Totals`/`Extremes`/`Exception`/`EndOfStream` out, `Cancel` (kills the worker), `Ping`; same auth, classifier, deny list, limits and pool as HTTP.

*Tests:* `clickhouse_client_interactive_and_query_modes`; `native_insert_formats`; `native_cancel_kills_worker`; `native_errors_match_reference`; `corpus_over_native_equals_http`.

### Task 32: Upstream stateless subset, ClickBench and TPC-H (FL2 Task 11)

**Files:** FL2 Task 11's files; `scripts/house/{clickbench.sh,tpch.sh}` for the full `hits` set and SF100.

FL2 Task 11's contract with the gate raised: the stateless manifest's selected tests at ≥ 90 % pass (Q693), gated in CI from this task; ClickBench on the full `hits` set correct against the reference (D414 rules); TPC-H SF1 on PRs and SF100 nightly, correct.

*Tests:* `stateless_manifest_is_reproducible`; nightly `house-nightly` publishing `house-compat-<sha>.json`.

### Task 33: The driver and BI matrix

**Files:** `scripts/house/drivers_{py,go,java,client}.sh`, `fabric/crates/loams-house/tests/drivers_rs.rs`, `conformance/clickhouse/bi/{grafana,metabase,superset}/**`, `.github/workflows/fabric.yml`.

**Produces:** §49 §8.2's Tier 1 suites in CI and Tier 2 smoke scripts; Grafana's and Metabase's generated SQL recorded and replayed as corpus cases so a plugin upgrade that changes SQL is noticed.

*Tests:* one CI job per Tier 1 client, all green; `bi_recorded_queries_replay`.

### Task 34: Performance gates

**Files:** `scripts/house/bench/**`, `docs/house/performance.md`.

**Produces:** every §49 §16 gate measured on the reference hardware against both baselines, with the report generator; the build-machine numbers recorded beside them.

*Exit:* each gate met, or the measured number accepted by the owner (Q693) and recorded.

### Task 35: Fault evidence (FL2 Task 13, extended)

**Files:** `fabric/crates/loams-house/tests/faults.rs`, `deploy/house/compose.yaml`.

*Tests:* `corpus_under_s3_latency`, `corpus_under_s3_resets`, `corpus_under_catalog_latency`, `cache_disk_loss_is_a_miss_not_an_error`, `kill_worker_mid_query`, `kill_front_mid_query`, `node_memory_pressure`; a 24 h soak with the ClickBench mix and ingest running, no leaked workers, RSS flat.

### Task 36: Security review and fuzzing

**Files:** `docs/house/security.md` (the review), `fuzz/` targets for the HTTP parameter parser, the classifier, the native codec and the `hsw1` codec.

**Produces:** a written review against §49 §13.1's threat table, with an external-style attempt per row (SSRF, file read/write, exec, cross-tenant, credential theft, fingerprinting, exhaustion, FFI); findings filed and fixed; 24 h fuzzing per target clean.

*Exit:* no open high or critical finding.

### Task 37: Docs

**Files:** `docs/house/**`, `docs/guides/clickhouse-surface.md` (generated, linked), the SDK module `loams.house` notes for SDK2.

**Produces:** quickstart (`clickhouse-client` and curl against `--single-node`), table classes, ingest, security model, limits (D88 pattern, generated from `limits.rs`), operations and DR, performance, desktop. `docs_examples_run` executes every code block against a test House.

### Task 38: The GA flip

**Files:** `fabric/crates/loams-house/src/surface.rs` (`unstable: false`), `crates/loams/src/api/connect.rs` (`loams.house.v1` stable), `docs/house/index.md`, `CHANGELOG.md`, this plan's status.

*Exit:* the checklist below is all ticked; the owner signs off.

---

## Exit criteria for production (all must be ticked for Task 38)

Engine and storage
- [ ] libchdb is only in the worker (`no_libchdb_in_front`), and no MergeTree is ever created in chDB (`no_mergetree_in_chdb`).
- [ ] Lake tables: every Task 10–13 and 15 test green; `compaction_preserves_rows_property` and `orphan_gc_never_deletes_referenced_files` green over 10 000 cases.
- [ ] Writes: `ack_only_after_commit`, `retry_with_same_token_writes_once`, `concurrent_commits_all_land`, `kill_front_during_commit_matrix` green.
- [ ] Ingest: `pipe_restart_is_exactly_once` and `mv_retry_does_not_double_apply` green; Iggy pipes green or labelled `Planned("FL1")`.
- [ ] Realtime tables: FL2 Task 7's tests green, or `realtime` labelled `Planned("FL1")` on the surface page.

Security and auth
- [ ] Security review written with no open high or critical finding; 24 h fuzzing clean for all four targets (Task 36).
- [ ] Every deny-list entry has a test, and `sandbox_blocks_with_deny_list_off` is green on the production sandbox mode (netns or pods) (Tasks 5–6).
- [ ] MT1 verification, scopes, table grants, TLS on non-loopback listeners and audit (Task 20).
- [ ] No AGPL/BSL/SSPL/ELv2 crate in the Fabric's graph; ClickBench query text not in the repository.

Operations
- [ ] Governance limits, quotas and usage events green (Tasks 21–22).
- [ ] Metrics, traces, redacted logs, query log, dashboards and alerts (Task 23).
- [ ] Scale-to-zero and wake measured and within §49 §16 or accepted (Task 24).
- [ ] HA and DR drills green for 14 consecutive nights (Task 25).
- [ ] Images by digest with SBOMs; rolling chDB upgrade without failed queries (Tasks 26–27).

Evidence and docs
- [ ] `chsurface-2.0` at 100 % of the declared corpus minus approved allowlist entries, over HTTP and native (Tasks 29–31).
- [ ] Stateless subset ≥ 90 % (or the owner's Q693 number); ClickBench full and TPC-H SF100 correct (Task 32).
- [ ] Tier 1 drivers and tools green; Tier 2 smoke recorded (Task 33).
- [ ] §49 §16 gates met or accepted (Task 34); fault suite and 24 h soak clean (Task 35).
- [ ] `docs/house/` complete, `docs_examples_run` green (Task 37).
- [ ] The desktop Analytics page runs on `loams.house.v1`, with install, console, browser and history (Task 28).

## Self-review

- Every §49 decision has a task: D760 (checklist, 38), D761 (2, 6, 7), D762 (10–12, global constraint), D763 (10, 14), D764 (12), D765 (13), D766 (9, 11), D767 (3, 29–31), D768 (30–31), D769 (16–18), D770 (2, 4, 24), D771 (10, 9), D772 (20), D773 (21), D774 (5, 6, 36), D775 (15, 25), D776 (22, 23), D777 (29, 32–35), D778 (7, 8, 28), D779 (26, 27, 28).
- Every Review Focus row names its tests.
- Task 0 reconciles with FL1 and FL2 as built and maps FL2's remaining tasks; nothing in HS1a–HS1e waits for FL1 except Tasks 14 and 18's Iggy half.
- Owner gates (Q685–Q687) are named where they block merges, not where they block work on a branch.

## Rulings made during execution

| # | Ruling | Why | Cost if wrong |
|---|---|---|---|
| R0.1 | **FL2 as built (checked 2026-10-08 at `6079e4d7`): Tasks 0, 1 and 3 are merged** (`1e756537` reconcile and spike record; `0c78e964` `loams-chdb-sys` and `loams-chdb`; `0c221bf1` Arrow and cancellation findings; `4f35958c` `loams-house` errors). `origin/fl2-arrow-ruling`, `origin/fl2-t0-chdb-spike` and `origin/fl2-t3-errors` are all ancestors of `dev`: nothing is open on them, and they can be deleted. FL2 Rulings 1-14 stand as written in FL2's table; the final state of 10-14 is: 10 (Arrow unusable) is **superseded by 14** for output (buffered `Arrow`, `ArrowStream`, `Parquet` work; `chdb_arrow_scan` and `chdb_stream_query_arrow` hang and are never called, HS1 Task 2's `no_hanging_ffi_paths_called`); 11 (cancellation is Loams' own: kill the worker, HS1 Task 2) stands; 12 (one connection shape per process, per-session settings via `SET`) stands and is why sessions are per worker; 13 (`filesystem_caches_path`, no `filesystem_cache_size_limit` through the C ABI) stands, so HS1 Task 9's cache size is `house-cache`'s, not chDB's. **Map of FL2's unbuilt tasks:** 2 -> HS1 3; 4 -> 4; 5 -> 10 (lake) and 14 (realtime); 6 -> 12 and 14; 7 -> 11 and 14; 8 -> 5 and 11; 9 -> 29; 10 -> 29; 11 -> 32; 12 -> 33; 13 -> 35. FL2's status is "Absorbed into HS1" from its Task 2 (FL2 plan header and plans README row edited in the same commit) | The plan's expectation held; no stray branch has unmerged work | Low |
| R0.2 | **FL1 is not built.** `fabric/crates/` holds `loams-chdb`, `loams-chdb-sys`, `loams-flow`, `loams-flow-proto`, `loams-graph`, `loams-graph-proto`, `loams-house` only: no `loams-fabric` binary, no `loams-fabric-envelope`; no `deploy/fabric/` (deploy/ has dapr, loams-pg-bench, neon, tikv, wesql). FL1's plan is still "Planned". **Task 7 creates the `loams-fabric` binary crate with only the `house` role**; Tasks 14 (realtime) and 18's Iggy half stay blocked on FL1 | `ls` and `git log` on `fabric/` and `deploy/` | Low; if FL1 lands first, Task 7 adds the role to its binary instead |
| R0.3 | **`loams-cache` and `loams-store` work as path dependencies of `fabric/`.** A probe crate in the `fabric/` workspace (`loams-cache = { path = "../../../crates/loams-cache" }`, `loams-store` likewise, plus the workspace's `arrow` 59 and `tokio`) ran `cargo check` clean in 36 s and was then removed (lock file restored). Their workspace-inherited dependencies resolve from the engine's `Cargo.toml`: `object_store` 0.14.2 (one version in the probe graph), `foyer` 0.22.6, `tokio` 1.53.2 (unified with the Fabric's), `moka`, `crc32c`. **Neither crate depends on arrow**, so the engine's arrow 58 does not enter; the Fabric stays on arrow 59. `cargo tree -d` shows only the usual duplicate families (rand, getrandom, digest, syn), none from these crates. `cargo deny check bans` is ok; licences fail only for the probe crate's own missing licence field (not a finding). Consequence: `iceberg` 0.10.1 brings arrow 58, so it stays isolated in `loams-house-lake`, which must exchange data with the rest as Arrow IPC bytes or Parquet, never as `arrow` 59 types | Measured (`cargo tree`, `cargo deny`) | Medium if `loams-house-lake` leaks arrow 58 types into a 59 API; Task 10 adds a `cargo tree` CI check |
| R0.4 | **MT1 has no verifier crate.** `crates/` has no auth or verifier crate and no `TokenVerifier` anywhere; MT1's plan is "Planned". **Task 20 defines `TokenVerifier` (a trait in `loams-house`) with a static-key test implementation and wires MT1's verifier when it exists**; non-loopback listeners require TLS plus a verifier regardless | grep over `crates/` | Low |
| R0.5 | **Adding `loams.house.v1` to the engine.** `CATALOGUE` in `crates/loams/src/api/connect.rs` has three packages today (`loams.instance.v1`, `loams.collection.v1`, `loams.live.v1`; `Package { package, services, available, unstable }`) and feeds `GetInstance.services[]`. §44's module table (`docs/design/44-unified-api-and-sdks.md`, around lines 42 and 180-183) lists planned packages by name (`loams.flow.v1` "when FL lands"). Adding the House needs: (a) a §44 catalogue line for `loams.house.v1` (`HouseService`, §49 §18); (b) a `CATALOGUE` entry with `unstable: true` whose `available` is dynamic (an endpoint configured **and** healthy), unlike the other entries' constants, so the catalogue needs a health-aware flag; (c) a **proxy route** for `/loams.house.v1.*` to `[house] endpoint` (headers and auth preserved, streaming bodies), not a local service impl; (d) the tests in `crates/loams/tests/connect_api.rs` and `route_map.rs`, which check the catalogue against the route map. (`route_map.rs` is a test file at `crates/loams/tests/route_map.rs`, not in `src/api/`.) | Read the code | Medium: `available` being a constant today means Task 7 must introduce the dynamic flag first |
| R0.6 | **AP1e as built (names Task 28 reuses).** Plugins are workspace packages under `web/plugins/<name>/` with `package.json` (`"loams": {"plugin": {"kind":"console","entry":".","tier":"first-party","inject":["desktop","router","slots"],"slots":["console.page","shell.nav.section"]}}`), `src/index.tsx` (a `PluginModule` with `apply(ctx)` calling `router.page({id, path, title, plugin}, …)` and `slots.register({name:'shell.nav.section', order, meta:{id,label,href,icon,group}}, …)`), `src/page.tsx`, `test/*.test.tsx` and `vitest.config.ts`; the model is `web/plugins/postgres` and `web/plugins/wesql`. Shared UI: `@loams/desktop-ui` (`web/packages/desktop-ui`: `SqlConsole`, `PageHead`, `Tabs`, `StackCard`, `ConnectPanel`, `useStack`; `isWrite`/`stripSql` re-exported from `@loams/desktop/sql-lex`). The **write-confirm dialog** is inside `SqlConsole` (`sql-console.tsx`: `isWrite(text, dialect)` sets `confirm`, a `Dialog` titled "Run a statement that changes data?" with Cancel and a danger Run button); the Analytics page reuses that pattern but decides writes by the server's `house_read_only` answer (§49 §18.3), so it needs a `SqlConsole` variant, not the client-side `isWrite`. Main process (`apps/desktop-electron/src/main/`): the **supervisor** is `engine/supervisor.ts` `EngineSupervisor` (`SupervisorDeps` injects `spawn`, `binary()`, `dataDir`, `logFile`, `fetch`; `state()`, `start()`, `stop()`; emits `"state"`; backoff `[1,2,4,8,16,30]` s, 5 restarts per 10 min; `scrubEnv` strips `LOAMS_*_TOKEN`, `*_SECRET`, `*_API_KEY`), with `engine/ipc.electron.ts`, `engine/binary.ts`, `engine/ports.ts`, `engine/log-rotate.ts`; `stacks/` supervises compose stacks (`stacks.ts`, `runtime.ts` `detectRuntime`); `servers/registry.ts` is the server registry; `update/` holds the signed-manifest pieces (`manifest.ts`, `pubkey.ts`, `pin.ts`) that Task 28's digest and signature check reuses. The **agent tool registry** is `agent/tools.ts` (`ToolDef {name, description, risk: 'read'|'write', schema, run(ctx,args)}`, `ToolRegistry`, `TOOLS`, `registerTools`, `resultText`, `truncate`, `MAX_RESULT_CHARS` 20 000), registered from `index.ts` (`registerTools(sqlAgentTools(sql))`); SQL helpers live in `sql/` (`caps.ts` `runCapped`, `DEFAULT_CAPS`, `redact`, `isPlainRead`; `tools.ts` `sqlAgentTools`; `agent/sql-guard.ts`). Task 28's `house_query` is a `ToolDef` with `risk: 'read'`, registered the same way; its main-process code goes in `main/house/` beside these | Read the code at `6079e4d7` | Low; names may move while AP1e Part 2 is open, and Task 28 re-checks them |
| R0.7 | **Pins** (crates.io, today 2026-10-08, newest stable at least 14 days old; all licences pass `fabric/deny.toml`'s allow list): `sqlparser` 0.63.0 (2026-09-13, Apache-2.0); `zstd` 0.14.0 (2026-09-04, BSD-3-Clause) or 0.13.3 if its `zstd-sys` is already in the graph (it is not yet: Task 12 decides); `lz4_flex` 0.14.0 (2026-07-14, MIT; already in the lock at an older line, so unify first); **`cityhash-rs` 1.0.1** (2022-07-29, MIT OR Apache-2.0; **1.0.0 is GPL-3.0-or-later and must not be used**, so pin `=1.0.1`) rather than the `clickhouse` crate 0.15.2 (2026-08-28, MIT OR Apache-2.0), which would pull a whole client for one hash; `iceberg` and `iceberg-catalog-rest` 0.10.1 (2026-08-01, Apache-2.0); `rusqlite` 0.40.2 (2026-08-08, MIT; `bundled`); `landlock` 0.4.7 (2026-07-27, MIT OR Apache-2.0); `seccompiler` 0.5.0 (2025-03-07, Apache-2.0 OR BSD-3-Clause); `nix` 0.31.3 (2026-05-11, MIT); `postcard` 1.1.3 (2025-07-24, MIT OR Apache-2.0); `rustls` 0.23.45 (2026-09-14, the version already locked by the graph), `tokio-rustls` 0.26.5 (2026-09-04), `rcgen` 0.14.10 (2026-08-28, test certificates only). **chdb-core: keep v26.9.0** (released 2026-09-28, so only 10 days old on 2026-10-08: it is already pinned and digest-verified in `loams-chdb-sys/build.rs` from FL2, and the 14-day rule is read as "do not adopt a *new* pin younger than 14 days"; it turns 14 days old on 2026-10-12). v26.9.1-rc.1 (2026-10-08) is a pre-release and is not eligible; the next stable bump is made only when a stable newer than v26.9.0 is at least 14 days old, then in `versions.rs` together with the reference server digest | crates.io API and `gh release list -R chdb-io/chdb-core` | Medium for the chdb-core exception: the owner may prefer v26.7.3 (2026-09-10), but the build already depends on v26.9.0 |
| R0.8 | **The desktop Analytics page contract (§49 §18), recorded for Task 28.** Gate: `GetInstance.services[]` lists `loams.house.v1.HouseService` or the page shows "not configured" (install action for the local engine; the server's message for a remote one; Windows is remote-only). RPCs used: `ExecuteQuery` (server-streaming; `format = ARROW_IPC` for the grid, `JSON_ROWS` for CSV export; `read_only = true` and `max_rows = 1 000` by default; frames `header`, `batch`, `progress`, `summary`; truncation reported), `CancelQuery`, `ExplainQuery`, `ListDatabases`, `ListTables` (class badges `LAKE`, `REALTIME`, `EXTERNAL`), `DescribeTable` (columns, DDL, 20 snapshots, 100 partitions), `PreviewTable` (server writes the `LIMIT` SQL; `limit` at most 1 000), `ListQueryHistory(scope = MINE)` and `GetQuery` (server-side, shared across devices; saved queries stay local to the desktop in v0.1), `ListPipes`; `RestoreTable` is not on the page in v0.1. Writes: off until "Allow writes" is on for the tab; a `house_read_only` answer to a read-only run triggers the confirm dialog, then re-runs with `read_only = false`. Agent tool `house_query`: `read_only = true`, `max_rows = 200`, never writable. Export: CSV or Parquet through `ExecuteQuery` (`JSON_ROWS`) or an HTTP `FORMAT Parquet` download through the main-process proxy, capped at 1 GiB. Editor: ClickHouse keywords plus the function list from `system.functions` fetched once through `ExecuteQuery`. Error reasons the page maps: `house_syntax_error`, `house_unknown_table`, `house_access_denied`, `house_read_only`, `house_timeout`, `house_memory_limit`, `house_too_many_queries`, `house_quota_exceeded`, `house_disabled_function`, `house_cancelled`, `house_not_configured`, with `ErrorInfo.metadata` `clickhouse_code` and `clickhouse_name`. Plugin id `@loams/plugin-analytics`, path `/analytics`, group "Data". §49 is already in `docs/design/README.md` (row 49, "Proposed"), so no README change was needed there | §49 §18 | Low |

