# DX1 — Durable Execution Compatibility: Temporal Protocol, DBOS and Apache Airflow (plan stub)

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Where this plan gives exact names, paths, branches or tests, use them verbatim. Record any deviation in "Rulings made during execution" at the end of this file.
>
> **Status: DX1 Tasks 0–11 done; DX1's exit is met on loopback, with a conditional go for DX2** ([`dx1-exit-report.md`](dx1-exit-report.md), 2026-10-27: 26 of 26 SDK-leg cases; two defects to fix first, X11.R4). DX0 stopped after Task 3 (S.1). DX0 and DX1 were specified on 2026-10-21 and 2026-10-24; DX1 started in parallel with DX0 (D1590). Track DX, design [§74](../design/74-durable-execution-compat.md) (D1500–D1539, Q1150–Q1165, **approved by the owner on 2026-10-21 with every default**, Q1165 open; it amends §68's Temporal and Airflow decisions). This plan fully specifies **DX0**, the baselines. They are the upstream Temporal server on Loams Postgres (compat mode), DBOS conformance, a corpus of real Temporal histories, and Airflow 3.3 with the Edge executor on Loams Postgres. DX0 writes no engine code. **DX1**, the Temporal gateway MVP, is specified task by task below; the owner started it on 2026-10-24 in parallel with DX0 and the fork work (D1590, amending D1534). DX1A–DX5 and the fork tasks FW0–FW10 stay at milestone level and get full specifications when DX0 exits (D1534).

**Goal (DX0):**
- the stock Temporal server runs on Loams Postgres 16 through the endpoint mode Task 2 picks, passes Temporal's SQL persistence tests and a soak, and is available as the compose profile `temporal` (compat mode, D1509). Postgres 17 runs as a leg whose failures are expected failures;
- a versioned corpus of real Temporal histories, from pinned Go and TypeScript SDKs against that server, is committed with a manifest, and each SDK's replayer accepts it. This is DX1's differential oracle;
- DBOS Transact's own suites (Python, TypeScript, Go, Java) pass against Loams Postgres, with a crash-recovery case and a polling leg, and the PgDog transaction-mode leg is recorded as an expected failure;
- Airflow 3.3 runs on Loams Postgres with two schedulers and the Edge executor (`edge3`) and passes §68 §4.3's gate tests plus the Edge cases;
- every result is in a strict expected-failure list and in the generated compatibility matrix, and the DX0 exit report gives a go or no-go for DX1 and DX1A.

**Architecture:** no crate changes. DX0 shares §68's OC0 harness (`conformance/compute/`, `deploy/release/compose.compute.yaml`, `scripts/compute/`, `.github/workflows/compute-compat.yml`). It extends OC0 Task 6 (Airflow) and Task 7 (the Temporal spike), and adds `conformance/compute/{dbos-py,dbos-ts,dbos-go,dbos-java}/` and `conformance/compute/temporal/corpus/`. **Shared tasks are done once.** If OC0's task has merged, the DX0 task adds only its delta. If it has not, the DX0 task lands OC0's task as OC1 specifies it, plus the delta, and marks the OC0 task done in OC1's rulings.

**Tech stack:** Docker Compose v2; bash with `set -euo pipefail` and shellcheck; Python 3.12 (stdlib only for scripts); `temporal-sql-tool` and the server images at the pinned 1.32.x tag; Go 1.24+ and Node 22 for the corpus workers; the DBOS SDKs' own test runners (pytest, the TypeScript SDK's test runner, `go test`, Gradle); `psql`.

**Effort (estimate):** 3–3.5 engineer-weeks (14–18 days), or about 2 if OC0 Tasks 1, 2, 6 and 7 have landed first. The owner's draft said 1–2 weeks; the difference is the corpus (Task 4) and four DBOS suites. Per task: 0 (1 d), 1 (2–3 d, or 0), 2 (1 d), 3 (2–3 d), 4 (3–4 d), 5 (2–3 d), 6 (2 d), 7 (1 d). Tasks 3, 5 and 6 can run in parallel after Task 2; Task 4 needs Task 3 (or its `start-dev` fallback).

## Global Constraints

- **Worktrees and branches.** `~/Documents/Ostriumlabs/loams-wt/dx0-<task>`, one branch and one PR per task into `dev` (CodeRabbit reviews; merge on green). `git commit -s`. Never `git stash`.
- **No Rust build, no crate change.** If a task finds a Loams Postgres or gateway defect, it opens an issue against the owning track (§51 for the fork, GW1 for the gateway), records a ruling, and keeps the product's row red or expected-failure.
- **No patches to upstream products.** Temporal, DBOS and Airflow run exactly as released. Only configuration differs. Upstream tests disabled for a reason go in `xfail.toml` with the reason and an issue.
- **Pins (D1274).** Every image by digest and every package by exact version, at a release at least 14 days old when the PR opens (security fixes excepted), in `conformance/compute/pins.env`. On 2026-10-21 that rules out Temporal Go SDK v1.50.0, .NET 1.21.0, Ruby v1.10.0, Rust 1.1.0, `edge3` 5.0.0 and the DBOS releases of 2026-10-15. Task 0 picks the newest eligible ones.
- **Licences (D1271, D1536).** Temporal (MIT), DBOS Transact (MIT) and Airflow (Apache-2.0) only. No DBOS Conductor image or licence key. No BUSL Resonate server. Task 1's `licence_gate_compute_pins` covers the new pins.
- **Never on Loams SQL.** No Temporal or DBOS database is ever created on Loams SQL (TiDB). Test `no_loams_sql_in_dx0` greps the DX0 files for the Loams SQL host and the `mysql8` plugin.
- **Never public.** No new host ports. Products reach Loams on the compose network only.
- **GitHub-hosted runners only.** Suites run nightly and on `workflow_dispatch`. Per-PR jobs only lint and validate files, and run the corpus manifest check (Task 4).
- **Wording (D1512, D1275).** DX0's docs say "runs the Temporal server on Loams Postgres" (compat mode). They never call the gateway "Temporal-compatible", because it does not exist yet. `docs_compute_wording` covers `docs/guides/durable/`.

## Review Focus

1. **A pass that proves nothing.** Each product's own suite runs, and the Loams cases check effects (one execution per step, no double run, histories read back), not exit codes. Tests: `temporal_persistence_suite`, `dbos_py_crash_recovery_exactly_once`, `airflow_edge_worker_kill_retries`.
2. **A corpus that is not reproducible or not real.** Every history in the corpus comes from the pinned real server, with a manifest of SDK, server and scenario versions, and the replayers accept it. Tests: `corpus_manifest_complete`, `corpus_replays_go`, `corpus_replays_ts`.
3. **The wrong endpoint mode.** Each suite runs on exactly the mode its recipe states. Test: `recipe_endpoint_mode_matches_suite`.
4. **A hidden workaround.** No patched product, and no disabled upstream test without an `xfail.toml` entry. Tests: `xfail_entries_have_reason_and_issue`, `xfail_strict_unexpected_pass`.
5. **Loams SQL or a licence slip.** Tests: `no_loams_sql_in_dx0`, `licence_gate_compute_pins`, `no_licence_key_in_compute_files`.

## DX0 — The baselines (Tasks 0–7)

### Task 0 — Rulings

Check and record, in "Rulings made during execution":
- **Pins:**
  - the Temporal server and `admin-tools` images (newest 1.32.x eligible under D1274) and `temporal-sql-tool` from the same tag; the Temporal CLI (v1.9.1);
  - the Go SDK (newest eligible 1.4x) and TypeScript SDK (1.24.0 or newer eligible), plus the `samples-go` and `samples-typescript` commits the corpus builds on;
  - DBOS: `dbos-transact-py`, `-ts`, `-golang` and `-java`, each at the newest release at least 14 days old, with the commit whose test suite runs;
  - Airflow 3.3.2 with its constraints file and the `apache/airflow` image digest, `apache-airflow-providers-edge3` (newest eligible) and `apache-airflow-task-sdk`.
- **Temporal's persistence tests:** the entry point for the SQL persistence tests at the pinned tag (`common/persistence/tests` with plugin `postgres12`, or `postgres12_pgx`), and whether they run from source in a Go container (expected) or from the image.
- **Temporal's default limits** (blob size, gRPC message size, history size and count) and its default namespace retention, read from the pinned tag's `dynamicconfig` defaults. They feed §74 D1508 and Q1163, recorded with file and line.
- **Endpoint modes:** which Loams Postgres endpoints the release compose and Kubernetes offer: PgDog transaction mode through `loams-gateway` `pg`, a session-pooled mode, and a direct compute endpoint. If neither a session nor a direct endpoint exists, Task 2 uses the compute's address on the compose network and files the gap against GW1.
- **Loams Postgres majors** available in the release compose (16; 17 if selectable).
- **DBOS polling:** whether the TypeScript, Go and Java SDKs have an equivalent of Python's `use_listen_notify`. Read their configuration code at the pinned tag.
- **Each DBOS SDK's suite:** the command that runs it against an external Postgres URL, and the tests that need anything beyond Postgres (Kafka, plv8). Those become `xfail.toml` entries with the reason "needs X", not failures.
- **OC0's state:** which of OC0 Tasks 1, 2, 6 and 7 have merged. That decides whether Tasks 1, 3 and 6 below land OC0's part.

Files: this plan. Tests: none. Gate: rulings recorded.

### Task 1 — The harness (shared with OC0 Tasks 1 and 2)

If OC0 Tasks 1 and 2 have merged, this task only adds the new profiles and pins. If not, it lands them exactly as OC1 specifies, plus the following.

Files:
- `conformance/compute/pins.env`: adds `TEMPORAL_*`, `TEMPORAL_SDK_GO`, `TEMPORAL_SDK_TS`, `DBOS_PY`, `DBOS_TS`, `DBOS_GO`, `DBOS_JAVA`, `AIRFLOW_EDGE3`
- `deploy/release/compose.compute.yaml`: profiles `temporal` (the stock server, four services or the auto-setup image per Task 0, on Loams Postgres) and `dbos` (a test-runner container only); the `airflow` profile gains `edge-worker` services (two)
- `deploy/release/compute/temporal/development-sql.yaml`: the server's dynamic config, with `numHistoryShards: 16` fixed
- `conformance/compute/lib.sh`: adds `oc_temporal_wait` (the frontend answers `GetSystemInfo` through `temporal operator cluster health`) and `oc_pg_mode <mode>` (prints the URL for `txn`, `session` or `direct`)

Tests (in `conformance/compute/selftest.sh`): `compose_compute_config_valid` for `temporal`, `dbos` and `airflow`; `compose_compute_requires_profile`; `compose_compute_no_new_host_ports`; `compose_compute_images_pinned_by_digest`; `no_loams_sql_in_dx0`; `licence_gate_compute_pins` (with the new pins).

### Task 2 — Loams Postgres endpoints and databases

Files: `conformance/compute/durable-init.sh` (a one-shot `compute-init` step for the `temporal`, `dbos` and `airflow` profiles), `conformance/compute/endpoint-modes.md`.

- Creates, idempotently, on Loams Postgres:
  - `temporal` and `temporal_visibility` (through `temporal-sql-tool create-database`, `setup-schema` and `update-schema` at the pinned tag);
  - `dbos_py`, `dbos_ts`, `dbos_go` and `dbos_java` (one per SDK, each its own system database, D1520);
  - `airflow`.
- Each in its own database, never a shared schema (D1529).
- `endpoint-modes.md` records, per product, the mode used and why: Temporal session or direct (multi-statement transactions with `FOR UPDATE` and `FOR SHARE`); DBOS session or direct (`LISTEN`/`NOTIFY`); Airflow's scheduler and triggerer session or direct, unless Task 6's transaction-mode leg passes.

Tests: `durable_init_idempotent` (run twice, same result), `durable_init_separate_databases` (`\l` shows one database per product and no shared schema), `pg_modes_reachable` (each mode's URL answers `SELECT 1`; a missing mode fails with the GW1 issue in the message).

### Task 3 — Compat mode: the Temporal server on Loams Postgres (extends OC0 Task 7)

Files: `conformance/compute/temporal/{run.sh,persistence.sh,soak.sh,xfail.toml,README.md,SPIKE.md}`, `conformance/compute/temporal/pg17/xfail.toml`.

Steps:
1. **Persistence suite.** Temporal's SQL persistence tests at the pinned tag, in a Go container, against Loams Postgres 16 on Task 2's mode, then the same against 17.
2. **Soak.** The stock server (frontend, history, matching, worker; `numHistoryShards = 16`) runs the `samples-go` load workflow (Task 0 picks it) for 30 minutes at 50 starts per second (**estimate**, adjusted in Task 0). Halfway through, the history service is restarted. At 20 minutes, a Loams Postgres compute is suspended and resumed.
3. **Transaction-mode leg.** The same smoke runs through PgDog transaction mode and is expected to fail. If it passes, the run fails (strict), and the recipe can recommend transaction mode.
4. **SPIKE.md** records the results, the endpoint mode, the 17 result, and the recommendation for compat mode (D1509).

Tests: `temporal_schema_setup`, `temporal_persistence_suite`, `temporal_persistence_suite_pg17` (expected failures allowed with entries), `temporal_soak_no_failed_workflows`, `temporal_history_restart_recovers`, `temporal_compute_resume_recovers`, `temporal_txn_mode_xfail`.

Gate: if `temporal_persistence_suite` or the soak is red on 16, DX0 exits **no-go** for compat mode. Task 4 then runs its corpus against `temporal server start-dev` (SQLite) as the oracle instead, and the matrix shows Temporal-on-Loams-Postgres red with the reasons.

### Task 4 — The history corpus (Go and TypeScript)

Files:
- `conformance/compute/temporal/corpus/README.md`: the scenario list, the normalisation rules, and how to regenerate
- `conformance/compute/temporal/corpus/workers/go/` (a Go module pinned to Task 0's SDK) and `.../workers/ts/` (an npm package pinned the same way), each with one workflow per scenario
- `conformance/compute/temporal/corpus/generate.sh`: starts each scenario with a **fixed workflow ID** (`corpus-<lang>-<scenario>`), waits for close, and exports with `temporal workflow show --output json` and as binary protobuf (`GetWorkflowExecutionHistory` through a 30-line helper in the Go module)
- `conformance/compute/temporal/corpus/histories/<lang>/<scenario>.{json,binpb}`
- `conformance/compute/temporal/corpus/manifest.json`: per history, the scenario, language, SDK version, server version and image digest, Loams Postgres major, generation date, SHA-256 of both files, and the phase it belongs to (DX1, DX2, DX3, DX4)
- `conformance/compute/temporal/corpus/replay/go/replay_test.go` and `.../replay/ts/replay.test.ts`: replay every history of that language with the SDK's replayer (`worker.NewWorkflowReplayer` with `ReplayWorkflowHistoryFromJSONFile`; `Worker.runReplayHistories`)

Scenarios (each in both languages unless marked):
- **DX1:**
  - `hello` (one activity)
  - `two_activities_sequential`
  - `activity_parallel_fanout_10`
  - `signal_then_complete`
  - `query_while_running`
  - `cancel_requested`
  - `terminated`
  - `workflow_failed_application_error`
  - `activity_heartbeat_details`
- **DX2:**
  - `timer_short`, and `timer_cancelled`
  - `activity_retry_backoff_3_attempts`
  - `activity_start_to_close_timeout`, `activity_schedule_to_start_timeout`, `activity_heartbeat_timeout`
  - `workflow_task_failure_then_fix` (the replay of a nondeterminism fix, Go only)
  - `signal_with_start`
  - `child_workflow`, and `child_workflow_cancelled`
  - `continue_as_new_3`
  - `id_reuse_allow_duplicate_failed_only`, and `id_conflict_fail`
  - `local_activity`
  - `side_effect_and_version_marker`
- **DX3:**
  - `search_attributes_upsert`
  - `memo`
  - `large_payload_1mb`
- **DX4:**
  - `update_accepted_completed`, and `update_rejected` (validator)
  - `schedule_triggered` (Go only)
  - `reset_to_first_workflow_task` (Go only)

Tests:
- `corpus_manifest_complete`: every file is listed, every hash matches, and every scenario exists in both languages unless marked;
- `corpus_replays_go` and `corpus_replays_ts`: every history replays with zero nondeterminism errors;
- `corpus_regenerates_stably`: generating twice produces histories identical after Task 4's normalisation (`normalize.py`: masks timestamps, run IDs, identities, task tokens, request IDs and the worker's binary checksum; keeps event IDs, types, order, batch boundaries and attributes);
- `normalize_selftest`, on fixtures: two histories that differ only in masked fields compare equal, and a reordered pair does not.

Per PR: only `corpus_manifest_complete` and `normalize_selftest` run (no server). The replays and the regeneration run nightly.

### Task 5 — DBOS conformance

Files: `conformance/compute/{dbos-py,dbos-ts,dbos-go,dbos-java}/{run.sh,xfail.toml,README.md}`, `conformance/compute/dbos-common/crash/` (a small app per language for the Loams cases).

Each `run.sh`:
- checks out the SDK's repository at the pinned tag (a read-only `git archive` of the tag through the GitHub API, not a clone kept in the tree);
- points its suite at the Loams Postgres URL for Task 2's mode, writes JUnit and runs `oc_xfail_check`.

Loams cases, per language:
- `dbos_<l>_crash_recovery_exactly_once`: a five-step workflow writes a row per step. `kill -9` of the app after step 3, then a restart. Steps 1–3 are not re-run, and steps 4–5 run once (row counts).
- `dbos_<l>_queue_concurrency`: 1,000 enqueued workflows on a queue with concurrency 10 and two app processes. Each runs once.
- `dbos_<l>_notify_latency`: an event set by one process is received by another in under 1 s on the session or direct endpoint.
- `dbos_<l>_notify_after_compute_resume`: suspend and resume the Loams Postgres compute. After the listener reconnects, a new notification arrives.
- `dbos_py_polling_mode`: a fresh system database created with `use_listen_notify = false`. The suite subset and crash recovery pass. Other languages only if Task 0 found an equivalent switch.
- `dbos_<l>_txn_mode_xfail`: the crash-recovery app through PgDog transaction mode, recorded as an expected failure with the reason (`LISTEN` across transactions).

Tests: `dbos_py_suite`, `dbos_ts_suite`, `dbos_go_suite`, `dbos_java_suite`, and the Loams cases above.

### Task 6 — Airflow 3.3 with the Edge executor on Loams Postgres (extends OC0 Task 6)

Files: `conformance/compute/airflow/{run.sh,dags/,xfail.toml,README.md}` (OC0's), plus `conformance/compute/airflow/edge/{run.sh,dags/}`.

Setup: OC0 Task 6's configuration, plus `[core] executor = airflow.providers.edge3.executors.EdgeExecutor` (the class path is checked in Task 0), the Edge API enabled on the API server, and two `edge-worker` containers with the queue `default`. Then one run with `LocalExecutor` for comparison.

Tests:
- OC0's: `airflow_db_migrate_empty`, `airflow_ha_no_double_run`, `airflow_scheduler_kill`, `airflow_remote_logs_after_worker_gone`, `airflow_xcom_object_storage`, `airflow_git_bundle_version`, `airflow_rest_api_v2_smoke`;
- `airflow_edge_workers_register`: both workers show as online through the Edge API;
- `airflow_edge_ha_no_double_run`: two schedulers and two edge workers run 500 mapped task instances, each with exactly one try;
- `airflow_edge_worker_kill_retries`: `kill -9` of an edge worker mid-task. The task is retried on the other worker within the configured heartbeat timeout, and the try count is 2;
- `airflow_edge_maintenance_mode`: a worker in maintenance takes no new tasks and finishes its running one;
- `airflow_txn_mode_leg`: the scheduler and triggerer through PgDog transaction mode. If it passes, Task 2's `endpoint-modes.md` records that transaction mode is allowed. If it fails, it is an expected failure with the reason.

### Task 7 — Matrix, docs and the DX0 exit report

Files:
- `scripts/compute/matrix.py` (OC0's): a "Durable execution" section with rows for Temporal compat mode (16, 17), the corpus (the scenario count per phase), DBOS per SDK, and Airflow with Edge
- `docs/guides/durable/README.md`: §74 §10's which-engine table and links
- `docs/guides/durable/temporal-compat-mode.md`: running the Temporal server on Loams Postgres. It names the endpoint mode, the shard count fixed at creation, and never Loams SQL
- `docs/guides/durable/dbos.md`: the endpoint, one database per app, polling and scale-to-zero, the expected failures
- `docs/guides/durable/airflow.md`: a link to `docs/guides/compute/airflow.md`, plus the Edge executor section
- `docs/plans/dx0-exit-report.md`: the results; the go or no-go for compat mode; the corpus size; the facts DX1 needs (Temporal's limits and default retention, the endpoint modes, the DBOS polling switches); the recommendations for DX1 and DX1A; the questions for the owner

Tests: `docs_compute_snippets_match_files` (covers `docs/guides/durable/snippets/`), `docs_compute_wording`, `recipe_endpoint_mode_matches_suite`, `matrix_from_fixture` (with durable rows).

**DX0 exit:**
- every product has a nightly run with a committed `xfail.toml`;
- the corpus has at least the DX1 and DX2 scenarios in both languages and replays clean;
- `SPIKE.md` and `dx0-exit-report.md` give the go or no-go;
- the owner reviews the report before DX1 starts.

## DX1 — The Temporal gateway MVP (Tasks 0–11)

**Started 2026-10-24, in parallel with DX0 and the fork work** (owner: "start DX1 temporal gateway too, run in parallel"; D1590, amending D1534's gate). DX1 does not wait for DX0's exit report, for FW3 or for FW7. How it works without them is in "Building before FW3 and FW7" below, and every place the plan leans on an unfinished dependency names the test that catches a mismatch later.

**Goal (DX1, §74 §13.3):** the official Go and TypeScript SDKs, at the R0.3 pins, run `hello` (one activity) and a signal against `loams-gateway`'s `temporal` module with only a host change and an API key; the gateway's histories for every DX1 corpus scenario match the corpus after normalisation; each SDK's replayer accepts them; `GetSystemInfo` advertises exactly what is built (`capabilities_match_impl`).

**Architecture.** Eight crates (D1504) and one gateway module. The workflow logic is a pure function `apply(state, input) -> transition` (`loams-temporal-statemachine`). One `Engine` per process (`loams-temporal-frontend`) loads a run, applies one input, and commits the events, the new mutable state and the outbox rows in one compare-and-set transaction (`loams-temporal-store`, D1502). A relay drains the outbox into Resonate through a `Durability` port (`loams-temporal-resonate`, D1503). Resonate's deliveries (a task ready, a promise settled or timed out) come back as inputs through the same `Engine`. Polls wait in `loams-temporal-matching`, which claims every task from Resonate before it answers (D1505).

```
SDK ─gRPC─▶ frontend (WorkflowService) ─▶ Engine ─▶ statemachine::apply (pure)
                 ▲  │ polls                 │ CAS commit: events + state + outbox
                 │  ▼                       ▼
            matching ◀── TaskReady ──── relay ─▶ Durability ─▶ Resonate
                 └── acquire ──────────────────────▶ (task, lease, timers)
```

**Tech stack:** Rust (workspace edition and MSRV), tonic 0.14 and `tonic-prost-build` with the system `protoc` (as `loams-stream-grpc`), sqlx 0.9 (`postgres`, `runtime-tokio`), `hmac` and `sha2` for task tokens, proptest for the property tests. Go 1.26.8 and Node 22 images (R0.3) for the SDK legs. No new dependency outside the workspace's list without a line in the PR body.

**Effort (estimate):** 7–9 engineer-weeks (36–45 days), up from §74's 6–8, because the interim Resonate adapter and the corpus-driven golden tests are new work. Per task: 0 (0.5 d), 1 (2 d), 2 (2 d), 3 (3–4 d), 4 (5–7 d), 5 (4–5 d), 6 (4–5 d), 7 (3 d), 8 (6–8 d), 9 (2 d), 10 (3–4 d), 11 (1 d).

### Order and independence

| Task | Needs | Can run beside |
|---|---|---|
| 0 Rulings | — | everything |
| 1 `loams-temporal-proto` | 0 | 5, 6 |
| 2 `loams-temporal-testkit` (corpus, normaliser, differ) | 1; DX0 Task 4's committed corpus | 3, 5, 6 |
| 3 `loams-temporal-history` | 1 | 2, 5, 6 |
| 4 `loams-temporal-statemachine` | 2, 3 | 5, 6, 7 |
| 5 `loams-temporal-store` | 0 (stores bytes) | 1–4, 6, 7 |
| 6 `loams-temporal-resonate` (port, fake, interim adapter, IDs, relay) | 0 | 1–5 |
| 7 `loams-temporal-matching` | 6 (the port) | 4, 5 |
| 8 `loams-temporal-frontend` (Engine, `WorkflowService`, tokens, auth) | 4, 5, 6, 7 | — |
| 9 `loams-gateway` `temporal` module, `loams-temporal` binary | 8 | — |
| 10 Compose profile, Go and TypeScript legs, replay, differential | 9 | — |
| 11 Matrix, docs, DX1 exit report, bench hand-off | 10 | — |

The first runnable workflow is the end of Task 9 (an SDK worker against the standalone binary on the fake or the real Resonate). Task 10 makes it a conformance leg.

### Building before FW3 and FW7 (D1591)

- **The port.** `loams-temporal-resonate` defines its own `Durability` trait (Task 6), shaped on the requests the gateway needs from §74 §6.2 and named after `resonate-client`'s methods as the fork lead's FW3 work names them (`task_create`, `task_acquire`, `task_heartbeat`, `task_fulfill`, `task_release`, `promise_create`, `promise_settle`, `promise_register_listener`), so the final adapter is thin. Everything above the port is tested against `FakeDurability`, an in-process, deterministic implementation with a manual clock.
- **Until FW3 publishes a pinnable revision:** an interim adapter `HttpDurability` speaks the protocol `2026-04-01` envelope (`POST /`) and the poll transport (`GET /poll/{group}/{id}`, server-sent events) to a real Resonate server: the engine's durable listener in compose (network-namespace path, §74 §6.1) or the fork's `resonate` binary. It lives in one module, `wire_interim`, that nothing else imports. When FW3 lands, a pin-bump PR replaces it with `resonate-client` (D1516) and deletes the module; the same profile tests must pass before and after.
- **Until FW7 publishes the gateway-facing profile:** §74 §5.3's mapping is implemented as written, and each assumption it makes about Resonate is a named test in `crates/loams-temporal-resonate/tests/profile.rs` that runs against a real server (`LOAMS_TEMPORAL_TEST_RESONATE_URL`): `profile_timer_times_out_and_notifies`, `profile_task_lease_expiry_redelivers`, `profile_idempotent_create_same_id`, `profile_idempotent_create_conflicting_param`, `profile_delayed_create`, `profile_listener_delivery`, `profile_settle_canceled`. FW7's scenario tests in the fork should mirror them. Where FW7 rules differently, the gateway follows FW7 and the change is a ruling here. The promise-ID length limit is taken as 255 bytes until FW7 states it (the hash fallback of D1503 makes the exact value a constant).
- **Until the durable listener has auth (D111):** DX1 runs only on the compose network-namespace path (`network_mode: service:loams`, loopback). The cluster leg is DX3's, gated on D111.
- **Asked of the fork lead** in `loams-wt/.he0-shared/resonate-bench.md` ("DX1" section): a pushed, pinnable FW3 revision; FW7's answer codes and the ID limit; whether one `poll://any@<group>` with one subscriber receives every message (single replica in DX1; FW8 covers many).

### Global constraints (DX1)

- **Worktrees and branches.** `~/Documents/Ostriumlabs/loams-wt/dx1-t<N>-<topic>`, one branch and one PR per task into `dev`. `git commit -s`. Never `git stash`. PRs that add a public interface or storage carry the label `review:greptile`.
- **Builds.** Heavy builds through `loams-wt/.he0-shared/buildslot.sh` with `-j 4`. Per PR: `cargo fmt --check`, `cargo clippy -p <touched crates> --all-targets -- -D warnings`, `cargo test -p <touched crates>`; delete the PR's own test binaries afterwards.
- **No Temporal server code** (D1500-series, D1536). The protos are vendored (MIT, with licence and NOTICE). The server may be read to settle a question of behaviour; its code is never copied, translated or paraphrased line by line. Behaviour is taken from the corpus, the SDKs' replayers and the API's own comments.
- **Pure core.** `loams-temporal-history` and `loams-temporal-statemachine` do no I/O, read no clock and make no random numbers: time, IDs and request IDs come in through the input. A test (`statemachine_has_no_io_deps`) checks their `Cargo.toml` dependency lists.
- **No Loams-internal dependencies** in `loams-temporal-*` (D1539). `loams-gateway` depends on them, not the reverse.
- **Never on Loams SQL; never history in Resonate** (D1502). The store's tests run on stock Postgres 17 in CI and on Loams Postgres `compute1` (R0.6, R0.7) locally.
- **Metrics** are `loams_temporal_*`, with no tenant or namespace labels (D1021, D1532).
- **Wording** (D1512): "speaks the Temporal API" or "works with the Temporal SDKs", with the matrix linked; never "Temporal-compatible" without it.

### Task 0 — Rulings

Recorded in "Rulings made during execution" (X1.R1–X1.R9): the proto pin, what the SDKs need from `GetSystemInfo`, the long-poll deadlines, the task-token format, auth in the MVP, the shard count, the gateway's limits, the MVP's replica count, and Q1150's namespace mapping for compose. Files: this plan. Gate: rulings recorded.

### Task 1 — `loams-temporal-proto`

Files:
- `crates/loams-temporal-proto/proto/`: `temporalio/api` at **v1.63.5** (X1.R1), the subtrees `temporal/api/**`, and the `google/api/{annotations,http}.proto` and `nexus/**` files they import, unchanged
- `crates/loams-temporal-proto/proto/loams/temporal/internal/v1/state.proto`: the gateway's own messages (mutable state, outbox payloads, task tokens), package `loams.temporal.internal.v1`
- `crates/loams-temporal-proto/{Cargo.toml,build.rs,src/lib.rs}`; `LICENSE-temporalio-api` (the MIT text of the tag); `VENDORED.md` (tag, commit `3ebdff42a9f07ac484b415fe8ff0b483b4ce3340`, the tarball's SHA-256, the file list with hashes, and how to update)
- `NOTICE`: a "Temporal API" entry; `LICENSES.md` row

Interfaces: `pub mod temporal::api::*::v1` (prost + tonic server and client), `pub mod loams::temporal::internal::v1`, `pub const API_TAG: &str`, `pub const FILE_DESCRIPTOR_SET: &[u8]`, `pub fn workflow_service_methods() -> Vec<&'static str>` (read from the descriptor). `build.rs` reads `CARGO_MANIFEST_DIR` at run time (the shared target reuses build scripts across worktrees).

Tests: `vendored_files_match_manifest` (every file's SHA-256 equals `VENDORED.md`), `workflow_service_method_count` (the count at v1.63.5, recorded in X1.R1), `corpus_binpb_roundtrip` (every `.binpb` in the corpus decodes and re-encodes to the same bytes; skips with a message while the corpus is absent), `licence_and_notice_present`.

Exit: the crate builds and its tests pass; the licence and NOTICE are in place.

### Task 2 — `loams-temporal-testkit`: corpus, normaliser, differ

Files: `crates/loams-temporal-testkit/src/{lib.rs,corpus.rs,normalize.rs,diff.rs}`, `tests/corpus.rs`.

Interfaces:
- `Corpus::load(root) -> Corpus` (reads `manifest.json`, checks hashes) and `Corpus::histories(phase, lang) -> impl Iterator<Item = &CorpusHistory>`; `CorpusHistory { scenario, lang, run, phase, history: History }`
- `normalize(&History, &Rules) -> History` with the rules of `conformance/compute/temporal/corpus/normalize.py` (times, run IDs and other UUIDs, identities, task tokens, request IDs, checksums and build IDs, the namespace, task IDs, `historySizeBytes`, stack traces, sticky queue names, a continued run's first-task backoff, sorted SDK flags), including the free order of parallel activity started and completed events for `activity_parallel_fanout_10`
- `diff(expected, actual) -> Vec<Difference>`, each with the event ID, the field path and both values, printed as a table

Tests: `corpus_loads_and_hashes_match`, `normalize_masks_only_masked_fields` (fixtures: two histories differing only in masked fields compare equal; a reordered pair and a changed attribute do not), `normalize_agrees_with_python` (every corpus pair `normalize.py` calls equal, the Rust normaliser calls equal, and the reverse, on the committed corpus and the fixtures; skipped without `python3`), `diff_reports_path`.

Exit: the DX1 scenarios load in both languages; the normalisers agree.

### Task 3 — `loams-temporal-history`

Files: `crates/loams-temporal-history/src/{lib.rs,ids.rs,builder.rs,state.rs,fold.rs,limits.rs}`.

Interfaces:
- `EventIds` (the next event ID; contiguous allocation; the batch's first event ID)
- `HistoryBuilder`: one constructor per event type the MVP emits (`WorkflowExecutionStarted`, `WorkflowTaskScheduled/Started/Completed/Failed/TimedOut`, `ActivityTaskScheduled/Started/Completed/Failed/TimedOut/CancelRequested/Canceled`, `WorkflowExecutionSignaled`, `WorkflowExecutionCancelRequested`, `WorkflowExecutionCompleted/Failed/Canceled/Terminated`, `MarkerRecorded` for side effects and versions), each taking every value it records (time, identity, request ID) as an argument
- `MutableState` (the `loams.temporal.internal.v1.MutableState` message): execution info, `next_event_id`, the pending workflow task, pending activities by scheduled event ID, buffered events, the cancel request, the close status, the last completed workflow task's started event ID
- `fold(&[HistoryEvent]) -> Result<MutableState, FoldError>` (rebuild from history; §74 §5.5 rule 7)
- `Limits` (R0.9's defaults: 2 MiB payloads, 4 MiB messages, 50 MiB and 51,200 events per run), checked by the builder's caller

Tests: `fold_corpus_dx1` (every DX1 history folds, and the result is closed with the right status), `invariant_contiguous_ids`, `invariant_one_outstanding_wft`, `invariant_activity_one_terminal` (§74 §13.2 invariants 1–3, as debug assertions and over the corpus), `builder_matches_corpus_shapes` (for each event type in the DX1 corpus, the builder's event with the corpus's values equals the corpus's event).

### Task 4 — `loams-temporal-statemachine`

Files: `crates/loams-temporal-statemachine/src/{lib.rs,input.rs,workflow_task.rs,activity.rs,signal.rs,cancel.rs,close.rs,query.rs}`, `tests/{table.rs,props.rs,golden.rs}`.

Interfaces:
- `Input`: `Start`, `WorkflowTaskStarted { identity, request_id, now }`, `WorkflowTaskCompleted { commands, identity, messages, sdk_metadata, now }`, `WorkflowTaskFailed`, `WorkflowTaskTimedOut { scheduled_event_id, attempt }`, `ActivityStarted`, `ActivityCompleted`, `ActivityFailed`, `ActivityCanceled`, `ActivityHeartbeat`, `Signal`, `CancelRequested`, `Terminate`, `Timeout { kind, event_id, attempt }`
- `apply(&MutableState, Input) -> Result<Transition, Rejection>`; `Transition { events, state, effects, reply }`; `Effect`: `ScheduleWorkflowTask { scheduled_event_id, attempt, start_to_close }`, `ScheduleActivity { scheduled_event_id, attempt, task_queue, timeouts }`, `ArmTimeout { kind, event_id, at }`, `CancelPromises { run }`, `RunClosed`
- `Rejection` maps one to one to the gRPC status the SDKs expect (`NotFound` for a stale token, `InvalidArgument` for a bad command, `FailedPrecondition` for a closed run)
- The rules the corpus fixes: signals and cancel requests that arrive while a workflow task is started are buffered and written after `WorkflowTaskCompleted`; a completed workflow task with new buffered events schedules the next one; a close command with buffered events is refused with `UnhandledCommand`, as the SDKs expect; only the first failed attempt of a workflow task is written (the transient attempts are DX2's)

Tests:
- `table.rs`: one case per command and event pair of the MVP set
- `props.rs` (proptest): random input sequences never break §74 §13.2 invariants 1–4
- `golden.rs`: `golden_dx1_<scenario>` for each DX1 scenario and language: the inputs are read from the corpus history (the commands in each `WorkflowTaskCompleted` batch, the activity outcomes, signals, cancels), driven through `apply`, and the result equals the corpus history after normalisation
- `statemachine_has_no_io_deps`

Exit: every DX1 golden test passes in both languages.

### Task 5 — `loams-temporal-store` (label `review:greptile`)

Files: `crates/loams-temporal-store/src/{lib.rs,mem.rs,pg.rs,shard.rs}`, `migrations/0001_init.sql`, `tests/store.rs`.

Schema: §74 §5.5's tables (`namespaces`, `executions`, `current_runs`, `history_events`, `outbox`, `task_queues`, `visibility`, `schema_version`), with `shard_id` and the 64-shard default (X1.R6). Visibility gets only the columns `DescribeWorkflowExecution` reads; the list grammar is DX3.

Interfaces:
- `Store` (async trait): `create_namespace`, `get_namespace`, `list_namespaces`; `start(StartExecution) -> Result<StartOutcome>` (the current-run check and the first commit in one transaction); `load(&RunKey) -> Option<Loaded { state, version }>`; `commit(&RunKey, expected_version, Commit { events, state, outbox, visibility }) -> Result<u64, StoreError::Conflict>`; `history(&RunKey, from_event_id, page_size) -> HistoryPage`; `claim_outbox(shards, limit) -> Vec<OutboxRow>` (`FOR UPDATE SKIP LOCKED`); `mark_dispatched(&[OutboxId])`; `outbox_notify() -> impl Stream` (`LISTEN`/`NOTIFY` on Postgres)
- `MemStore` (tests, the dev mode) and `PgStore` (sqlx; the session or direct endpoint only, §74 §5.5 rule 5)

Tests (each against `MemStore` and, with `LOAMS_TEMPORAL_TEST_PG_URL`, `PgStore`): `cas_rejects_stale_version`, `commit_is_atomic` (a fault injected between statements leaves nothing visible), `outbox_two_relays_no_double_claim`, `history_pages_in_order`, `start_rejects_running_duplicate`, `migrations_idempotent`. CI: a path-filtered job with a stock `postgres:17` service by digest; locally on Loams Postgres `compute1`.

### Task 6 — `loams-temporal-resonate` (label `review:greptile`)

Files: `crates/loams-temporal-resonate/src/{lib.rs,ids.rs,port.rs,fake.rs,wire_interim.rs,relay.rs}`, `tests/{ids.rs,relay.rs,profile.rs}`.

Interfaces:
- `ids`: `PromiseId::{Wft, Act, Tmr, To, Start}` with `encode() -> (String, Tags)` and `parse(&str) -> Result<PromiseId>`, exactly §74 §5.4's grammar (percent-encoding, the `h` + 32-hex fallback past the limit with `loams:wf`, the `loams:ns`, `loams:run` and `loams:kind` tags)
- `Durability` (async trait, D1591): `promise_create(id, timeout_at, tags, param)`, `promise_settle(id, state)`, `promise_register_listener(id, group)`, `task_acquire(task_id, version, pid, ttl_ms) -> Claim`, `task_heartbeat(pid)`, `task_fulfill(task_id, version, state)`, `task_release(task_id, version)`, and `deliveries() -> Stream<Delivery>` with `Delivery::{TaskReady { task_id, version, promise_id }, Settled { promise_id, state }}`. A workflow or activity task is a `promise_create` with `resonate:target = poll://any@<group>` (§74 §5.3)
- `FakeDurability` (deterministic, `advance(ms)`, injectable faults: drop, duplicate and delay a delivery, lose all state)
- `HttpDurability` over `wire_interim` (D1591), with a bearer-token hook for D111
- `Relay::run(store, durability, shards)`: claims outbox rows, calls the port, marks them dispatched; counts `loams_temporal_outbox_conflict_total` when an existing ID answers with a different parameter

Tests: `ids_roundtrip` (proptest), `ids_hash_fallback_past_limit`, `ids_reject_raw_colon`, `relay_crash_between_call_and_mark_is_idempotent`, `relay_conflict_counts_and_alerts`, `fake_lease_expiry_redelivers`, and the `profile_*` tests of D1591 against a real server (skipped without `LOAMS_TEMPORAL_TEST_RESONATE_URL`; run locally on the engine's listener or the fork's binary, and in the Task 10 leg).

### Task 7 — `loams-temporal-matching`

Files: `crates/loams-temporal-matching/src/{lib.rs,registry.rs,backlog.rs}`, `tests/matching.rs`.

Interfaces: `Matcher::poll(queue: QueueKey, kind, deadline) -> Option<Dispatch>`; `Matcher::offer(TaskReady)`; `Matcher::cancel_poll`. A FIFO of waiting polls per (namespace, task queue, kind); a backlog of ready tasks; every hand-out is preceded by `Durability::task_acquire`, and a refused claim moves on to the next task (D1505: Resonate is the only authority). Polls end at 60 s with an empty answer (X1.R3). The MVP runs one replica that owns every partition (X1.R8); partition leases and forwarding are DX3's.

Tests: `fifo_oldest_poll_first`, `claim_refused_skips_task`, `poll_deadline_returns_empty`, `poll_cancelled_task_not_lost` (a poll whose client goes away after the claim releases the task), `no_double_handout_under_race` (loom-free: 1,000 tasks, 16 pollers, the fake counts acquisitions).

### Task 8 — `loams-temporal-frontend` (label `review:greptile`)

Files: `crates/loams-temporal-frontend/src/{lib.rs,engine.rs,service.rs,token.rs,auth.rs,namespaces.rs,query.rs,capabilities.rs}`, `tests/{methods.rs,scenarios.rs,capabilities.rs}`.

Interfaces:
- `Engine`: `execute(run, input) -> Reply` (load, `apply`, commit with compare-and-set, retry on conflict up to a bound, then `Unavailable`); the callback path for `Delivery` (a stale callback is acknowledged and dropped, invariant 4); a per-run watch for `GetWorkflowExecutionHistory`'s wait for close
- `WorkflowServiceImpl`: the DX1 methods of §74 §5.8, each its own function; every other method answers `UNIMPLEMENTED` with the matrix's URL in the message
- `TaskToken`: the `loams.temporal.internal.v1.TaskToken` message (namespace ID, workflow ID, run ID, scheduled event ID, attempt, kind), HMAC-SHA256 with a key ring of current and previous keys (X1.R4)
- `Authenticator` (trait): `StaticKeys` (SHA-256 of `lk_` keys from the config, each bound to a Loams namespace ID); `LoopbackDev` only when the listener is loopback and `insecure_dev = true` (X1.R5). The Zitadel and `loams-authz` path (D1600, §76) is a later adapter in the gateway
- Queries: `QueryWorkflow` hands a query to a waiting workflow-task poll (the legacy `query` field), and `RespondQueryTaskCompleted` answers it; nothing durable
- `capabilities()`: what `GetSystemInfo` advertises

Tests: `methods.rs` (each DX1 method's happy path and its rejections, through an in-process tonic server on `MemStore` and `FakeDurability`); `scenarios.rs`: `gateway_dx1_<scenario>_matches_corpus` (a scripted worker on the generated tonic client replays the corpus's commands against the gateway; the gateway's history equals the corpus after normalisation); `capabilities_match_impl` (the methods with an implementation, the advertised capabilities and the method set of §74 §5.8 for DX1 are the same set); `token_tamper_rejected`, `token_previous_key_accepted`; `stale_timeout_dropped`.

### Task 9 — The `temporal` module in `loams-gateway`, and `loams-temporal`

Files: `crates/loams-gateway/src/temporal/{mod.rs,config.rs,listener.rs,metrics.rs}` behind the cargo feature `temporal`; `crates/loams-temporal-frontend/src/bin/loams-temporal.rs` (the standalone binary of Q1151 and D1539, which needs nothing from Loams); `deploy/release/temporal-gateway.toml.example`.

Config (`[temporal]`): `listen` (default `127.0.0.1:7233`; h2c on loopback or the compose network, TLS otherwise), `database_url` or `store = "memory"` (dev only), `resonate_url` or `durability = "fake"` (dev only), `shards`, `task_token_keys`, `keys` (the `StaticKeys` file), `bootstrap_namespaces` (`default`, 72 h retention, R0.9). The dev values refuse to start unless `insecure_dev = true` and the listener is loopback.

Tests: `config_rejects_dev_store_on_public_listener`, `module_serves_get_system_info` (an in-process gateway answers), `metrics_have_no_tenant_labels`, `standalone_binary_has_no_loams_deps` (`cargo tree` of `loams-temporal` names no `loams-*` crate outside `loams-temporal-*`).

Exit: **the first runnable workflow.** The Go SDK's `hello` from the corpus workers completes against `loams-temporal` with `store = "memory"` and `durability = "fake"`, and then with `PgStore` and `HttpDurability` against the engine's Resonate. The bench agent is told in `resonate-bench.md` ("DX1: gateway ready").

### Task 10 — The `temporal-gateway` compose profile and the SDK legs

Files: `deploy/release/compose.compute.yaml` (profile `temporal-gateway`: the gateway in the engine's network namespace, `network_mode: service:loams`, the `loams_temporal` database created by `durable-init.sh`, no host port); `conformance/compute/temporal/gateway/{run.sh,xfail.toml,README.md}`.

`run.sh` brings up the profile, runs the corpus workers (DX0 Task 4's Go and TypeScript generators, unchanged, with the gateway's address and an API key) for the DX1 scenarios into a scratch directory, replays the result with each SDK's replayer, and compares it with the committed corpus through `manifest.py stable` and the Rust differ.

Tests: `gateway_hello_go`, `gateway_hello_ts`, `gateway_signal_go`, `gateway_signal_ts`, `gateway_dx1_scenarios_{go,ts}`, `gateway_histories_replay_{go,ts}`, `gateway_dx1_differential`, `gateway_profile_no_host_ports`, `resonate_profile_on_engine` (Task 6's `profile_*` tests against the engine's listener). Expected failures go in `xfail.toml` with a reason and an issue.

### Task 11 — Matrix, docs, the DX1 exit report

Files: `scripts/compute/matrix.py` (a "Temporal API (gateway)" section: one row per method and SDK, from the Task 10 JUnit); `docs/guides/durable/temporal-gateway.md` (D1512's wording, the matrix link, the dev mode's limits); `docs/plans/dx1-exit-report.md` (results; the FW3 and FW7 state and which interim pieces remain; the go or no-go for DX2).

**DX1 exit (§74 §13.3):** hello, an activity and a signal pass on Go and TypeScript; the replayers accept the gateway's histories; the DX1 differential is green; `capabilities_match_impl` is green; the interim adapter is either replaced by `resonate-client` or listed in the exit report with FW3's state.

## Fork work: `ostrium-labs/resonate` (FW0–FW10; §74 §7.4)

These tasks run in the fork, not in this repository. Each is one branch on the fork's `main` and one upstream PR to `resonatehq/resonate`, with DCO sign-off. §74 §7.4 holds each task's acceptance criteria. The Loams side of each is one pin-bump PR here (`Cargo.toml` `rev`, plus the code that needs it).

| Task | When | Loams-side follow-up |
|---|---|---|
| FW0: shepherd #1162–#1168, `CARRIES.md`, fork CI | With DX0 | none |
| FW1: the TiKV store in the fork, keeping the 0.1.0 format | With DX0 | Pin bump; delete `crates/loams-durable/src/tikv.rs`; `durable-tikv` uses the fork's plugin |
| FW2: TiDB upstream | With DX0 | none |
| FW3: `resonate-client` and the public router entry | Before DX1 | `loams-temporal-resonate` and `loams-worker` depend on it |
| FW7: the gateway-facing protocol profile | Before DX1 Task 0 | DX1 Task 0 reads it |
| FW8: poll groups with many consumers | Before DX1A | none |
| FW5: settled-promise retention | DX2 | Gateway retention (Q1163) |
| FW6: tag-indexed search | DX3 | Console Durable page |
| FW9: several instances on one TiKV keyspace | DX3 | Cluster HA tests |
| FW10: benchmarks and tracing | DX3 | DX3's load comparison |
| FW4: batch request (only if DX3 measures the need) | DX3 | The relay uses it |

## DX1–DX5 (milestone level; DX1's tasks are above, the rest after DX0's exit)

| Milestone | Content (§74) | Gate |
|---|---|---|
| **DX1** | Gateway MVP. Vendored protos; `loams-temporal-{proto,history,statemachine,store,matching,resonate,frontend,testkit}`; the `temporal` module in `loams-gateway`; the store with compare-and-set and outbox on Loams Postgres; the relay; the matching fast path; the MVP method set; Go and TypeScript hello-world; the replayers accept gateway histories; `capabilities_match_impl` | Started 2026-10-24 in parallel with DX0 (D1590); builds against its own `Durability` port until FW3 and FW7 land (D1591); its exit needs DX0 Task 4's corpus and the compose network-namespace path (cluster leg: D111) |
| **DX1A** | `LoamsResonateExecutor` in `ostrium-labs/loams-airflow`; `crates/loams-worker`; provider v0; chaos (worker kill, lease expiry, scheduler failover); the Airflow version matrix | DX0 Airflow; FW3, FW8 |
| **DX2** | Timers, timeouts, retries, signals, queries, cancel and terminate, child workflows, continue-as-new, ID policies, local activities; the differential harness in CI; Java and Python; DBOS console preset and views | DX1 exit; FW5 |
| **DX3** | Visibility and the list-filter grammar; sticky queues; shards and partitions; outbox batching; load tests against Temporal; Resonate throughput on TiKV | DX2 exit; FW6, FW9, FW10 (FW4 if measured) |
| **DX4** | Updates, versioning, Schedules, reset, `OperatorService`; Airflow asset watchers, auth manager, state store; the console's Durable section; an AIP-108 Rust prototype | DX3; AIP-108 out of experimental for the Rust part |
| **DX5** | TLA+ specs `spec/tla/temporal/{WorkflowTask,Activity,Timer,OutboxCas}.tla`; deterministic simulation (1,000 seeds nightly); porcupine; SDK-suite soak | Parallel from DX1 |

## Owner actions

- §74 and its defaults (D1500–D1539, Q1150–Q1164), including the amendments to §68, were approved on 2026-10-21; nothing further for the design.
- Q1165: the design-partner terms with ResonateHQ. The fork-work list (§74 §7.4) is the engineering input.

## Rulings made during execution

Checked on 2026-10-23 through the GitHub API, Docker Hub and PyPI. The 14-day rule (D1274) admits releases published on or before 2026-10-09.

| # | Ruling | Task |
|---|---|---|
| R0.1 | **OC0's state:** none of OC0 Tasks 0–11 has merged (`conformance/compute/` does not exist on `dev` at `710f47078`). OC0's own gate (v0.1.1 shipped, Q1030) is met: v0.1.1 was released on 2026-10-22, which also meets DX0's gate (§74 §14). So Task 1 lands OC0 Task 1 in full and OC0 Task 2 for the `airflow`, `temporal` and `dbos` profiles; `risingwave`, `sail` and `lakekeeper` stay with OC0 Task 2. Task 3 lands OC0 Task 7 and Task 6 lands OC0 Task 6. Each records the OC0 task done (or partly done) in OC1's rulings | 0, 1, 3, 6 |
| R0.2 | **Temporal pins:** server v1.32.1 (2026-10-08, tag commit `af08a5e5f2dd40f545f2afac7780daaa6987001e`): `temporalio/server:1.32.1@sha256:acecbab5b0b8ae9cf4012b508866ca97feda3709577bf157b537239f40029049` and `temporalio/admin-tools:1.32.1@sha256:9ef0dc5b063bf4ffd5c1512de72e0e73e225bd2c7006a18ce69bd592a8862a83` (index digests), with `temporal-sql-tool` from that admin-tools image. `temporalio/auto-setup` has no 1.32 tag (last pushed 2026-02-05), so the profile runs the server image as four services (`SERVICES=frontend`, `history`, `matching`, `worker`) plus a one-shot schema step. CLI v1.9.1 (2026-09-14) | 1, 2, 3 |
| R0.3 | **SDK pins:** Go SDK v1.49.0 (2026-09-14; v1.50.0 of 2026-10-15 is too new) and TypeScript SDK v1.24.0 (2026-09-15). The corpus builds on `samples-go` `d3784428122e0cd4b28e91a0cd86b589749abf28` and `samples-typescript` `07a4b3a991ef0b17eed46ecf92a774169297e345` (the last commits before 2026-10-09, both 2026-10-06; they already require SDK v1.49.0 and `^1.24.0`). Go toolchain: the server's `go.mod` says `go 1.26.8`, so the persistence suite runs in `golang:1.26.8@sha256:eb36c1664dd974cde625f736e02c204383deebe03977365caaec5bf49f794348` (2026-10-06, eligible); the corpus workers use the same image | 3, 4 |
| R0.4 | **The soak's load workflow:** `samples-go` has no load generator. The soak uses Temporal's own `temporalio/omes` (MIT) at `dae0843208ef65fd2f357145c00c770be7934331` (2026-10-08), scenario `workflow_with_single_noop_activity`, with its Go worker. The rate is **20 starts per second** (an estimate for a 4-vCPU GitHub-hosted runner that also runs the Loams Postgres stack and four Temporal services; the plan's 50 was an estimate). `SPIKE.md` records the rate actually sustained | 3 |
| R0.5 | **Temporal's persistence tests:** they run **from source** at v1.32.1 in the Go container (the server image has no test binaries): `go test ./common/persistence/tests/ -run 'TestPQ\|TestPGX\|TestPGXSP' -count=1` with `POSTGRES_SEEDS` and `POSTGRES_PORT` (`temporal/environment/env.go:36-37`). Three entry points (`postgresql_test.go:736-770`): `TestPQ` (plugin `postgres12`), `TestPGX` (`postgres12_pgx`) and `TestPGXSP` (`postgres12_pgx` with `default_query_exec_mode=simple_protocol`, the PgBouncer transaction-pooling configuration, temporalio/temporal#9804). The suite logs in as `temporal`/`temporal` (hard-coded, `postgresql_test_util.go:27-28`) and creates and drops `test_*` databases, so Task 2 creates a role `temporal` with password `temporal` and `CREATEDB` on the test stack's `compute1` only. The transaction-mode leg of Task 3 is `TestPGXSP` through PgDog | 2, 3 |
| R0.6 | **Loams Postgres majors:** `pg-control` offers **only 17** (17.11): `deploy/pins/loams-postgres-images.toml` has `supported = [17]` and `maintenance = [16]`, and `ServiceConfig::pg_versions` is `vec![17]` with no flag (`runtime/images.rs::from_pins` maps only `supported`); filed as #1130. The release stack's included static compute `compute1` (`deploy/loams-postgres-dev`) takes any pinned major, though: `COMPUTE_IMAGE` set to the pinned `compute-v16` digest and `COMPUTE_PG_MAJOR=16` give a 16 timeline on fresh volumes (Task 2 ran `conformance/compute/durable/run.sh` on both on 2026-10-24: `pg_modes_reachable_direct` reported major 16 and 17, and every other case passed on each). So DX0 runs **16 on `compute1`** (the primary leg, as planned) and **17 on `compute1`** with its default image (the expected-failure leg), each on its own fresh stack. #1130 only blocks running the suites on a `pg-control`-provisioned 16 endpoint, which DX0 does not need | 2, 3, 7 |
| R0.7 | **Endpoint modes:** the 0.1.1 release compose offers only **direct** endpoints: the static `compute1` (`compute1:55433` on `loams_default`, superuser `cloud_admin`, published on `127.0.0.1:55433`) and, with `--profile postgres-control`, each `pg-control` endpoint's compute (alias `ep-<endpoint ulid>`, port 55433, published on 55500–55999). No PgDog fronts the computes yet (PG2c), there is no session-pooled mode, and `deploy/helm/` has no Loams Postgres chart. Filed as #1129 (GW1/PG2c). So `oc_pg_mode direct` prints `compute1`'s compose-network URL (or `LOAMS_PG_DIRECT_URL` when set, for a `pg-control` endpoint); `oc_pg_mode session` fails with "no session-pooled endpoint (#1129)"; `oc_pg_mode txn` prints a PgDog started from the conformance overlay (R0.8). Every Temporal, DBOS and Airflow suite runs on `direct`. The suspend-and-resume cases stop and start `compute1` (a compute is stateless; its timeline stays on the pageserver), which is what a `pg-control` suspend does to an endpoint's compute | 1, 2 |
| R0.8 | **PgDog for the transaction-mode legs:** `scripts/ci/pgdog-licence.sh` fails on a PgDog image reference outside `deploy/pgdog`, `deploy/helm/loams-postgres`, `deploy/loams-postgres-single` and `scripts/router/inventory`. `deploy/release/compose.compute.yaml` is distributed, so it never names PgDog. The `txn` legs use a separate, never-distributed overlay `conformance/compute/pgdog-txn.yaml` with the pinned PgDog v0.1.60 digest from `deploy/pgdog/README.md`, and Task 1 adds `conformance/compute/` to the script's allow-list as a non-distributed test harness (like `scripts/router/inventory`), with the notice file linked from `conformance/compute/README.md` | 1, 3, 5, 6 |
| R0.9 | **Temporal's default limits** (v1.32.1, `common/dynamicconfig/constants.go`; they feed D1508 and Q1163): `limit.blobSize.error` 2 MiB (line 387) and `.warn` 512 KiB (392); `limit.memoSize.error` 2 MiB (397) and `.warn` 2 KiB (402); `limit.numPendingChildExecutions`, `numPendingActivities`, `numPendingSignals` and `numPendingCancelRequests` `.error` 2,000 each (407–430); `limit.historySize.error` 50 MiB (431), `.warn` 10 MiB (436), `.suggestContinueAsNew` 4 MiB (441); `limit.historyCount.error` 51,200 events (447), `.warn` 10,240 (452), `.suggestContinueAsNew` 4,096 (483); `limit.mutableStateSize.error` 8 MiB (468), `.warn` 1 MiB (473); search attributes: 100 keys (889), 2 KiB per value (894), 40 KiB in total (899). **gRPC:** the maximum request is 4 MiB (`common/rpc/grpc.go:30-32`, `MaxHTTPAPIRequestBytes`, "set to the max gRPC request size", the grpc-go default) and inter-node receives are 128 MiB (`grpc.go:43`). **Retention:** the server sets no default; the minimum is 1 h for local namespaces (`system.namespaceMinRetentionLocal`, line 218) and 24 h for global ones (213), and the CLI's `operator namespace create --retention` defaults to **72 h** (`temporalio/cli` v1.9.1 `internal/temporalcli/commands.yaml:2629-2632`). Task 3 creates the `default` namespace with `--retention 72h` because nothing else creates it once `auto-setup` is gone | 3, DX1 |
| R0.10 | **DBOS pins** (the newest release at least 14 days old; every 2026-10-15 release is too new): `dbos-transact-py` 3.2.0 (2026-09-29, commit `d324156616281d7d4923bc60851617bdbb70ea5e`), `dbos-transact-ts` v5.2 (2026-09-29, `3f36908f58fd8b3079cbf5372203c7a2d0ba06cc`), `dbos-transact-golang` v1.6.0 (2026-10-07, `04337c15667adcdf5cc057be448ed9545d6f06d9`), `dbos-transact-java` 1.2.0 (2026-10-01, `0f4fe187b20bd6f5f735d8f99d810aeb90b8314d`); all MIT | 1, 5 |
| R0.11 | **DBOS polling switches:** Python has `use_listen_notify` (`dbos/_dbos_config.py:44`, fixed when the system database is created) and `notification_listener_polling_interval_sec`. TypeScript has `useListenNotify` in its config (`src/dbos-executor.ts:180`, default true, also fixed at migration time, `src/system_database.ts:558`). Java has `useListenNotify` in `DBOSConfig` (`transact/.../config/DBOSConfig.java:61`). **Go has no switch**: it polls only when the dialect lacks `LISTEN`/`NOTIFY` (CockroachDB, SQLite; `dbos/internal/sysdb/dialect.go:225,334,375`). So the polling-mode case runs for Python, TypeScript and Java, not Go | 5 |
| R0.12 | **DBOS suite commands:** every suite hard-codes `localhost:5432`, user `postgres` and the password from `PGPASSWORD` (Python `tests/conftest.py:94-97`, databases `dbostestpy` and `dbostestpy_dbos_sys`). Each runner container therefore runs a `socat` forwarder from `localhost:5432` to the direct endpoint, and Task 2 creates a `postgres` role with `CREATEDB` (and the password the runner passes as `PGPASSWORD`) on `compute1`. The suites keep their own database names; Task 2's `dbos_py`, `dbos_ts`, `dbos_go` and `dbos_java` hold the Loams cases' apps (D1520 still holds: one system database per app). Commands: Python `pdm install && pdm run pytest tests` (`DBOS_DATABASE=POSTGRES`); TypeScript `npm ci && npm run build && npm run test:unit` (jest; `test:packages` is out of scope); Go `mockery && gotestsum -- -race -count=1 -timeout 30m ./...` (toolchain go1.26.6 per its `go.mod`, run in the R0.3 image); Java `./gradlew clean build` on Temurin 21 (its matrix is 17, 21 and 25). Java's build derives its version from git history (the workflow fetches with `fetch-depth: 0`), so its `run.sh` makes a throw-away repository with the pinned tag inside the runner's work directory; nothing is kept in the tree | 5 |
| R0.13 | **DBOS tests that need more than Postgres** (expected failures with the reason "needs X"): Python `tests/test_kafka.py` (Kafka), and the CockroachDB cases in Python and TypeScript (`tests/cockroachdb.test.ts`; Python's `DBOS_COCKROACHDB_URL` cases), "needs CockroachDB". The `sibedge/postgres-plv8` image in Python's CI is not needed: no test refers to plv8 (code search, 2026-10-23; Task 5 re-checks at the pin). Go and Java need only Postgres | 5 |
| R0.14 | **Airflow pins:** Airflow 3.3.2 (2026-09-17), `apache/airflow:3.3.2-python3.12@sha256:9df9c8be4096b9cc626bd7cb1f2b8c712eef66c59c615f8c7e6200871ca10bd1`, constraints `https://raw.githubusercontent.com/apache/airflow/constraints-3.3.2/constraints-3.12.txt`, `apache-airflow-task-sdk` 1.3.2 (pinned by `airflow-core`), `apache-airflow-providers-edge3` **4.3.2** (2026-09-14; 5.0.0 of 2026-10-13 is too new, and the 3.3.2 constraints pin 4.3.2). The image's default extras do not include `edge3`, so Task 6 adds a two-line `conformance/compute/airflow/edge/Containerfile` (`FROM` the pinned digest, `pip install` the provider with the constraints file): an install, not a patch. The executor class is `airflow.providers.edge3.executors.EdgeExecutor` (re-exported from `executors/edge_executor.py` at `providers-edge3/4.3.2`), and the worker command is `airflow edge worker` | 1, 6 |
| R0.15 | **Wording:** this plan's "CodeRabbit reviews" in the global constraints is superseded by the repository's rule (Greptile, on PRs labelled `review:greptile` only); DX0's PRs are labelled when they add a new interface or touch storage | all |
| T1.R1 | The `dbos` profile is five services, not one runner: `dbos-pg` (socat, forwarding `127.0.0.1:5432` to `DBOS_PG_TARGET`, default `compute1:55433`) and `dbos-py`, `dbos-ts`, `dbos-go`, `dbos-java`, idle toolchain containers in `dbos-pg`'s network namespace (`network_mode: service:dbos-pg`), so the suites' hard-coded `localhost:5432` reaches Loams Postgres (R0.12) and the transaction-mode leg only changes the forwarder's target | 1, 5 |
| T1.R2 | `edge3` is installed at container start through `_PIP_ADDITIONAL_REQUIREMENTS` with `PIP_CONSTRAINT` set to the 3.3.2 constraints file, instead of R0.14's Containerfile: every image stays pinned by digest and nothing is built (`compose_compute_images_pinned_by_digest`). If Task 6 finds the start-up install too slow or flaky, it switches to a built image on the pinned base and records that | 1, 6 |
| T1.R3 | The licence gate reads `conformance/compute/licences.toml` (each pin's licence, the value it was checked for and its source) on every pull request, offline, and `--online` re-reads GitHub's licence API, PyPI and npm for the nightly. The official images carry no `org.opencontainers.image.licenses` label, so a label lookup would resolve nothing. A pin bump without a re-check fails (`licence_gate_refuses_bsl_fixture` covers both) | 1 |
| T1.R4 | `conformance/compute/selftest.sh` runs as a step of `ci.yml`'s `repo-checks` job (part of `CI required`) instead of a `pull_request` job of `compute-compat.yml`, which lands with OC0 Task 11 and DX0 Task 7 for the nightly suites | 1 |
| T1.R5 | Task 1's `airflow` profile has the processes only (API server, two schedulers, DAG processor, triggerer, two edge workers). The database migration, remote logging and XCom object storage on RustFS and the `GitDagBundle` (§68 §4.3) land with Task 6, and the one-shot `compute-init` (databases and roles) with Task 2 | 1, 2, 6 |
| T1.R6 | The Temporal services have no healthcheck: the server image carries no client. `oc_temporal_wait` asks the frontend with the admin-tools image's `temporal operator cluster health` from the stack's network. `oc_pg_mode` takes `[database] [user:password]`. `oc_down` removes only the profile's own services and their volumes, never the stack's or Loams Postgres's data. The self-test adds `xfail_stale_entry_fails`, `xfail_empty_run_fails`, `junit_case_roundtrip`, `pg_mode_urls`, `pins_are_literal` and `compose_compute_uses_direct_endpoint` to the plan's list | 1 |
| T1.R7 | `scripts/ci/pgdog-licence.sh` allows `conformance/compute/` as a never-distributed test harness, beside `scripts/router/inventory` (R0.8). `deploy/release/compose.compute.yaml` names no PgDog image | 1 |
| T2.R1 | The init script is `deploy/release/compute/durable-init.sh`, not `conformance/compute/durable-init.sh`: `compose.compute.yaml` mounts it, and the opt-in compose file must work from `deploy/release/` alone. It has two steps, as two one-shot services: `compute-init` (`roles`, in the pinned psql image: roles, databases, owners, `REVOKE … FROM PUBLIC`) and `temporal-schema` (`temporal-schema`, in the admin-tools image: `temporal-sql-tool setup-schema -v 0.0` then `update-schema -d …/postgresql/v12/{temporal,visibility}/versioned`). The databases are created with SQL rather than `temporal-sql-tool create-database`, so one step owns every database and its owner. It creates every product's databases whichever profile is on (compose does not tell a service which profiles are active; seven empty databases cost nothing) | 2 |
| T2.R2 | The Task 2 tests are `conformance/compute/durable/run.sh` (suite `durable`, with `durable/xfail.toml`), run against the Postgres stack alone (`compose run` starts `compute1` and its dependencies, not the engine). `pg_modes_reachable` is three cases: `_direct` (also checks the major, `COMPUTE_PG_MAJOR`, default 17), `_txn` (PgDog of the overlay) and `_session` (expected failure, #1129). Added: `durable_init_roles_isolated` (each role reaches its own databases only). `lib.sh` gains `OC_PROJECT` and `OC_COMPOSE_EXTRA` so a second stack can run beside another one (local runs on a shared machine) | 2 |
| T3.R1 | The soak's load is `conformance/compute/temporal/soak/`, a 300-line Go program on the pinned SDK, instead of `temporalio/omes` (R0.4): omes needs its `workers/proto/api_upstream` git submodule, which a source archive does not carry (no clones), and its verdict is an exit code. `soak/` starts workflows at a fixed rate with fixed IDs (a retried start never makes a second workflow), runs their one activity in the same process, and passes only when every started workflow completed with its result (`temporal_soak_no_failed_workflows`). R0.4's rate (20 per second) and the plan's 30 minutes are its defaults (`SOAK_RATE`, `SOAK_DURATION`) | 3 |
| T3.R2 | Temporal's persistence suite and `soak/` build in the pinned Go image with `GOMAXPROCS=4` and `-p 4`, and local runs go through the shared build-slot limiter: the machines are shared. The server's source is a codeload archive of the pinned commit in a named volume | 3 |
| T3.R3 | The transaction-mode leg (`temporal_txn_mode_xfail`) runs the four services through PgDog and a 10-workflow smoke, not the persistence suite: that suite creates `test_*` databases at run time, and PgDog serves only the databases in its config. `TestPGXSP` (simple protocol, the PgBouncer configuration) still runs on the direct endpoint as part of the suite | 3 |
| T3.R4 | `COMPUTE_PG_MAJOR` (16 or 17) selects the leg; `run.sh` reads that major's `compute1` image from `deploy/pins/loams-postgres-images.toml`. The legs' expected failures are `temporal/xfail.toml` and `temporal/pg17/xfail.toml`. `oc_up` now starts only a profile's services and their dependencies (Loams Postgres), not the engine, the House or agentd. Added cases: `temporal_frontend_serving`, `temporal_smoke_hello`. Inherent expected failures (a product that needs a session failing through transaction pooling) cite the DX0 tracking issue #1149 | 3 |
| T3.R5 | Temporal's persistence suite runs its three plugin variants one after another (`-parallel 1`, `PERSISTENCE_PARALLEL`). Run in parallel on the shared machine on 2026-10-24, three tests failed on their own deadlines or timing assumptions (`TestClusterMembershipUpsertCanPageRead` in all three, `TestScanAllTrees` in two, `TestInsertSelect_*` in `TestPGXSP`); run serially, all 1,236 cases passed (6 skipped by the suite itself). `SPIKE.md` records both runs | 3 |
| T3.R6 | `temporal_txn_mode_xfail` passed: the four services ran through PgDog transaction pooling and the smoke completed. Per the plan's strict rule its entry is removed and the case must pass, but transaction mode is **not** recommended until the soak passes through PgDog as well (nightly). The Postgres 17 leg has no entries until its first run | 3 |
| S.1 | **Status on 2026-10-24, when the owner stopped DX0 work after Task 3.** Merged: Task 0 (#1131), Task 1 (#1135), Task 2 (#1168). Task 3: its PR, with the 16 leg run twice locally (SPIKE.md): Temporal's persistence suite green (1,236 cases), soak green once and red once because a stopped compute did not start again (#1185), so compat mode is **not yet go**. Left for Task 3's follow-up: #1185, the 17 leg, the 30-minute soak and a soak through PgDog, all in the nightly that Task 7 adds. Task 4 (corpus) is on branch `dx0-corpus` (not merged, no PR): the Go and TypeScript workers for all 31 scenarios (28 in TypeScript), the exporter, `normalize.py`, `manifest.py`, `generate.sh` and both replay tests are written; against `temporal server start-dev` every Go (37 files) and TypeScript (32 files) history replayed clean and two generations compared equal after normalisation; left: generate the committed corpus against compat mode, commit `histories/` and `manifest.json`, open the PR. Task 5 (DBOS) is on branch `dx0-dbos` (not merged, no PR): `run.sh` per SDK, the shared cases (`dbos-common/cases.sh`) and the crash apps in Python, TypeScript, Go and Java are written but have **not been run**; left: run each, fill each `xfail.toml` from the results, open the PR. Tasks 6 (Airflow with Edge) and 7 (matrix, docs, nightly workflow, exit report) have not started. The fork work (FW0-FW10) has not started | all |
| X1.R0 | **DX1 starts now, in parallel** (owner, 2026-10-24: "start DX1 temporal gateway too, run in parallel"). DX1 starts without DX0's exit report, without the owner's review of it, and without FW3 or FW7, in parallel with the fork work and the benchmarks. This overrides D1534's ordering and the DX1 gate in this plan; recorded as **D1590**. What DX1 still takes from DX0 is the committed corpus (Task 4), which its golden and differential tests read. How DX1 builds without FW3 and FW7 is **D1591** ("Building before FW3 and FW7") | DX1 |
| X1.R1 | **Proto pin:** `temporalio/api` **v1.63.5** (tag commit `3ebdff42a9f07ac484b415fe8ff0b483b4ce3340`, 2026-08-04, codeload tarball SHA-256 `359fd496ee2d97ed9776bae473e3d65c9c037b5370af8c1f2e9008043c1136a8`), the version both the server v1.32.1 (R0.2) and the Go SDK v1.49.0 (R0.3) pin as `go.temporal.io/api` in their `go.mod`. MIT, "Copyright (c) 2025 Temporal Technologies Inc.". At that tag `WorkflowService` has **123** RPCs (§74 §4.1 counted 121 on 2026-10-20 at a later state of `master`; the test pins 123) and `OperatorService` 12. The tag carries its own copies of `google/api/{annotations,http}.proto` and `nexusannotations/v1/options.proto`, the only non-Temporal imports besides the well-known types, so nothing else is fetched | 1 |
| X1.R2 | **`GetSystemInfo`** advertises only behaviour that is built. The Go SDK v1.49.0 reads `sdk_metadata`, `eager_workflow_start`, `temporal_failure_responses`, `workflow_task_completion_pagination`, `worker_heartbeats`, `worker_commands`, `worker_poll_complete_on_shutdown` and `poller_autoscaling(_auto_enroll)` (`internal/*.go`). DX1 sets `sdk_metadata` (the corpus's `WorkflowTaskCompleted` events carry the SDK's flags, so the gateway must accept and record them) and `signal_and_query_header`, `internal_error_differentiation`, `activity_failure_include_heartbeat` and `encoded_failure_attributes` (pass-through behaviour); every other flag is false, so the SDK does not call `RecordWorkerHeartbeat` or eager start. A test reads the flags from the same table as the method list | 8 |
| X1.R3 | **Long polls:** the Go SDK sets a 70 s deadline on each poll (`pollTaskServiceTimeOut`, `internal/internal_task_pollers.go:40`, "the matching long-poll interval plus some delta"), so the gateway answers an empty poll at **60 s**, or 5 s before the request's own deadline if that is earlier. The TypeScript SDK (sdk-core) is checked in Task 10 (`gateway_hello_ts` runs over a poll that times out) | 7 |
| X1.R4 | **Task tokens** are the gateway's own `loams.temporal.internal.v1.TaskToken` message (version byte, namespace ID, workflow ID, run ID, scheduled event ID, attempt, kind, key ID) followed by HMAC-SHA256 over it. The key ring is the config's `task_token_keys` (current first); a token signed with a previous key is accepted until that key is removed. Tokens are opaque to the SDKs, which only echo them | 8 |
| X1.R5 | **Auth in the MVP** (D1507, as amended by D1600): the SDKs send their `api_key` as `authorization: Bearer lk_…`. `StaticKeys` checks its SHA-256 against a file of `{hash, loams_namespace_id, role}` rows, the same shape a later `loams-authz` adapter answers. Zitadel tokens and `durable:temporal:*` checks through `loams-authz` come with the cluster leg (DX3, behind D111). `insecure_dev` (no key) is refused unless the listener is loopback | 8, 9 |
| X1.R6 | **Shards:** 64 per `loams_temporal` database (Q1152), fixed at creation and stored in `schema_version`; a different count on an existing database refuses to start. `shard_id` = the first 8 bytes of SHA-256(namespace_id ‖ 0x00 ‖ workflow_id), big-endian, mod 64, written down once in `loams-temporal-store::shard` with a fixed test vector | 5 |
| X1.R7 | **Limits** are R0.9's: 2 MiB per payload blob, 4 MiB per gRPC message (tonic's `max_decoding_message_size`), 50 MiB and 51,200 events per run, 2,000 pending activities and signals; each answered with the status and message shape the SDKs expect (`InvalidArgument` for a blob, a terminated run for the history limit). Per-namespace lowering is DX2's | 3, 8 |
| X1.R8 | **One replica in DX1.** The MVP runs one gateway process, which owns every task-queue partition. Partition leases in `task_queues`, forwarding between replicas, and the ≥2-replica Deployment (§74 §12) are DX3's, with FW8. Resonate stays the authority for each claim from the first day (D1505), so a second replica later adds no new correctness argument | 7, 9 |
| X1.R9 | **Namespaces in compose (Q1150):** the key file binds each key to one Loams namespace ID; the gateway bootstraps the Temporal namespace `default` in it (72 h retention, R0.9) and `RegisterNamespace` is admin-only (role `admin` in the key file). Promise IDs use `<loams-ns-id>.<temporal-ns-id>` (D1503) | 6, 8, 9 |
| T4.R1 | The committed corpus (#1207) is generated against `temporal server start-dev` (CLI 1.9.1 from the pinned admin-tools 1.32.1 image, which embeds server 1.32.0; the manifest records 1.32.0 and `loams_postgres_major: start-dev`), per Task 3's gate, because compat mode is not yet go (#1185). When compat mode is go, the nightly regenerates the corpus with `CORPUS_SERVER=compat-mode` against the pinned 1.32.1 server on Loams Postgres, and that corpus replaces this one in a pull request. (On a loaded podman host the generator lost name resolution to the dev server after a restart; it worked by IP) | 4 |
| T5.R1 | **The DBOS harness** is `conformance/compute/dbos-{py,ts,go,java}/run.sh`, with the shared Loams cases in `dbos-common/cases.sh` and one small app per language in `dbos-common/crash/<l>/`. Each app has the same commands (`setup`, `start`, `resume`, `count`, `enqueue`, `serve`, `queued`, `set-event`, `get-event`), and each step writes one row, so the cases judge rows rather than exit codes. The crash case kills the process when step 4 starts: Python, TypeScript and Go send themselves `SIGKILL`, and Java calls `Runtime.halt`. Tool pins re-checked on 2026-10-27, all at least 14 days old: `pdm` 2.29.2 (2026-09-17), `jest-junit` 17.0.0 (2026-04-24), `mockery` 3.8.0 (2026-09-09), `gotestsum` 1.13.0 (2025-09-11), and the TypeScript app's `pg` 8.23.1 (2026-09-30) and `@dbos-inc/dbos-sdk` 5.2.11 (2026-09-29) | 5 |
| T5.R2 | The polling-mode case runs on a fresh system database, `dbos_<l>_poll_<epoch>`, because `use_listen_notify` is fixed when DBOS creates its system tables. `cases.sh` creates the database as the admin, owned by `postgres`, before the app's `setup` connects to it. Without that, `setup` would fail on a database that does not exist yet | 5 |
| T5.R3 | **No "needs X" entries for Kafka or CockroachDB.** At the pinned commits these tests skip themselves when their service is absent: Python's `tests/test_kafka.py` calls `pytest.skip("Kafka not available")`, and TypeScript's `tests/cockroachdb.test.ts` uses `describe.skip` without `DBOS_COCKROACHDB_URL`. A skipped test is neither a pass nor a failure in `xfail_check.py`, so listing them would only make stale entries. R0.13's plan is superseded | 5 |
| T5.R4 | **Status, 2026-10-27: written, not yet run.** No suite or Loams case has run against a stack (the owner's no-build rule). Each SDK's `xfail.toml` lists only `dbos_<l>_txn_mode_xfail`, as the plan expects. Temporal passed through the same PgDog unexpectedly (T3.R6), so the first nightly may turn that entry into an unexpected pass, which the strict check reports. The nightly's `dbos` leg (Task 7) runs all four SDKs. The self-test gains `dbos_files_parse` and `dbos_xfail_and_pins` | 5 |
| X1.R10 | **Timers are in DX1.** The DX1 corpus scenario `cancel_requested` waits on a one-hour timer in both SDKs (`TimerStarted`, then `TimerCanceled` after the cancel request), so the DX1 state machine handles `StartTimer`, `CancelTimer` and `TimerFired`. DX2 keeps `timer_short`, `timer_cancelled` and the timer edge cases | 4, 6, 8 |
| X1.R11 | **FW7's first findings** (fork lead, 2026-10-24, `resonate-bench.md` "FW7 protocol profile"), and what DX1 does about each: (1) a promise tagged `resonate:timer` *resolves* at its deadline, and `resonate:scope=global` gives `rejected_timedout`: the gateway treats a promise the server settles at its deadline as fired whatever the final state, and tells user timers (`tmr:`) from timeouts (`to:`) by the ID's kind (D1503), not by the state; (2) the Postgres store answers 500 for promise IDs with more than one `:` until a fork fix lands: D1503's grammar stays, and DX1's real-server tests run on SQLite (the single-node store, D1518) or TiKV; (3) the ID limit is 255 bytes, confirming the plan's assumption and the hash fallback; (4) re-creating an ID with a different parameter answers 200 with the stored record, so the gateway detects conflicts itself by comparing the returned parameter (`loams_temporal_outbox_conflict_total`); (5) the SQL stores ignore `resonate:delay` until a fork fix: activity retry backoff (DX2) must not depend on it, and DX1's corpus has no retries | 6, 8 |
| X1.R12 | **The test kit's normaliser works on protobuf JSON** made from the vendored descriptors with `prost-reflect` (feature `serde`, which adds `serde-value`, MIT), so it follows `normalize.py` key by key. It adds one equivalence for the gateway, `sticky_as_normal` (a sticky task queue reads as its normal queue; the gateway has no sticky queues in DX1, Q1156). `normalize_agrees_with_python` compares verdicts, not bytes: the two JSON encoders differ in number formatting | 2 |
| X1.R13 | **Promise-ID grammar amended** (amends D1503's §5.4 grammar; coordinator decision 2026-10-25, owner may veto). FW7 found that Resonate's *origin* is the text before an ID's first `:`, that the blob and TiKV stores keep one compare-and-set document per origin, that `task.heartbeat` takes tasks of one origin only, and that the Postgres store refuses IDs with more than one `:` (500). §5.4's `wft:{ns}:…` IDs made the kind the origin (every promise of a kind, across every namespace, in one document) and carried up to five `:`. The grammar is now FW7's §8.0 form, with the run as the origin: `<ns>.<wf>.<run>:wft.<scheduled_event_id>`, `<ns>.<wf>.<run>:act.<scheduled_event_id>.<attempt>`, `<ns>.<wf>.<run>:tmr.<started_event_id>`, `<ns>.<wf>.<run>:to.<event_id>.<s2s\|s2c\|sc2c\|hb\|wft>.<attempt>` and `<ns>.<schedule_id>.<nominal_time_unix_ms>` (no `:`, Resonate's schedule template). Exactly one `:` in every run-scoped ID. `<ns>` stays `<loams-ns-id>.<temporal-ns-id>` (X1.R9), first; `<wf>`, `<run>` and `<schedule_id>` escape every byte but ASCII letters, digits, `-`, `_` and `~` as `%XX`, and `<ns>` the same but keeps its `.`, so the origin splits exactly at its last two `.`. A `to` ID now carries the attempt, so attempt 2's timeouts are different promises from attempt 1's. The 255-byte limit and the `h` + 32 hex fallback (with `loams:wf`) and the `loams:ns`, `loams:run` and `loams:kind` tags are unchanged. Implemented in `loams-temporal-resonate::ids`; proposed to the fork lead in `resonate-bench.md` (DX1 section) | 6, 7, 8 |
| X1.R14 | **Retry backoff on the SQL stores** (coordinator decision 2026-10-25): the SQLite, Postgres and MySQL stores ignore `resonate:delay` until the fork's fix lands (X1.R11 (5)), so the relay holds a delayed create, and every later outbox row for the same promise, until its time (`DelaySupport::Defer`, the interim adapter's default), and sends it then. No timer-promise workaround. When the fork's fix is pinned, the adapter may report `DelaySupport::Native` and `profile_delayed_create` drops its expected failure on the SQL stores | 6, DX2 |
| X1.R15 | **Matching, as built** (Task 7): the queues are keyed by their Resonate poll group (`ltq-<w|a>-<hash>`), so a task offered before its queue's first poll is kept, not dropped. The fast path (D1505) is a `Durability` decorator on the relay's creates: a new task is offered to the matcher at version 0 right after Resonate accepts it; Resonate's own redelivery of pending tasks (every 30 s) is the backstop, for example after a gateway restart. Queries go through the same queues as in-memory items and are never durable | 7, 8 |
| X1.R16 | **A closed run's outstanding promises are settled lazily** until FW5's retention: a task handed out for a closed or moved-on run is fulfilled and skipped, a timer or timeout that fires for one is acknowledged and dropped (§13.2 invariant 4), and `to:` timeout promises are never cancelled, so they fire stale. Nothing is written to history for them | 8 |
| X1.R17 | **Workflow-task failures and terminate in DX1:** every failed or timed-out workflow-task attempt is written (Temporal's transient attempts, not written until one succeeds, are DX2's); terminate writes any buffered events and then `WorkflowExecutionTerminated`, without a `WorkflowTaskFailed` for a started task (DX2 checks Temporal's behaviour against the corpus) | 4 |
| X1.R18 | **`UNIMPLEMENTED` with the matrix link:** the proto crate generates default method stubs, and the frontend's `ImplementedOnly` layer answers every `WorkflowService` method outside `capabilities::IMPLEMENTED` with `UNIMPLEMENTED` and the matrix URL before the stub is reached. `capabilities_match_impl` calls every one of the 123 methods and checks the answer, and checks `IMPLEMENTED` against §5.8's DX1 list | 8 |
| X1.R19 | **Callback gaps:** Resonate sends a listener's `unblock` once; one sent while the gateway's poll connection is down is lost (`Delivery::Gap` reports the reconnect). Re-reading the awaited timers and timeouts after a gap is DX3's reconciliation; until then a lost firing waits for the run's next input. The interim adapter reconnects at once, so the window is a reconnect's length | 8 |
| X1.R20 | **The SDK legs' differential** (Task 10, `conformance/compute/temporal/gateway/compare.py`) adds two equivalences to the corpus's `normalize.py`: a sticky task queue reads as its normal queue (X1.R12), and a later `WorkflowTaskCompleted`'s `sdkMetadata` keeps only its flag sets, because an SDK without a sticky cache replays from the start and names itself (`sdkName`, `sdkVersion`) again. The parallel fan-out compares the multiset of its events other than workflow tasks, because how many completions share a workflow task is timing (#1225). The legs run on loopback with host-networked SDK containers, not yet in the compose profile `temporal-gateway`, which needs the gateway image (Task 10's compose half, next) | 10 |
| X1.R21 | **The first runnable workflow** (2026-10-25): `loams-temporal` on the interim adapter against the fork's Resonate server (`loams/main` f419b8be, SQLite): the Go SDK v1.49.0 and the TypeScript SDK 1.24.0 ran all nine DX1 corpus scenarios each (hello, a signal, queries, cancel, terminate, failure, heartbeats, fan-out), and each SDK's replayer accepted all eighteen histories. sdk-core names its build in the deprecated `binary_checksum` of workflow polls and completions, and its activities inherit the workflow's build (`use_workflow_build_id`); the gateway records both as Temporal does | 9, 10 |
| T6.R1 | **Pins re-checked on 2026-10-27** (PyPI and Docker Hub; the 14-day rule admits releases on or before 2026-10-13): Airflow 3.3.2 (2026-09-17) is still the newest eligible release (3.4.0b1 of 2026-10-13 and 3.4.0b2 of 2026-10-20 are betas), the `apache/airflow:3.3.2-python3.12` index digest is unchanged, and `apache-airflow-task-sdk` stays 1.3.2. `edge3` stays **4.3.2**: 5.0.0 (2026-10-13) is now old enough, but the 3.3.2 constraints file pins 4.3.2 and the profile installs with `PIP_CONSTRAINT`, so 5.0.0 would not resolve. The constraints also pin the `git` (0.5.0), `amazon` (9.36.0), `common-io` (1.9.0) and `standard` (1.19.0) providers the cases use, all in the stock image | 6 |
| T6.R2 | `edge3` stays a start-up install (T1.R2) for now. Every one of the seven Airflow containers installs it, so a cold start is slow; the nightly's first run decides whether Task 6 switches to a built image on the pinned base | 1, 6 |
| T6.R3 | **The `GitDagBundle` reads a plain `git daemon`** (`git-server` in `conformance/compute/airflow/compose.test.yaml`, from the pinned Airflow image, which carries git) instead of Loams Git, which is not in the release stack's light profile. The connection is `git://git-server/dags.git` (`AIRFLOW_CONN_LOAMS_GIT`). The Edge cases' DAGs are a second bundle, a `LocalDagBundle` (`oc-edge`), so OC0's bundle-version case is not disturbed by them | 6 |
| T6.R4 | **Effects, not exit codes.** Every case's task writes one object per execution to `s3://airflow-effects/<dag>/<run>/<task>/<map index>/` (`dags/_oc_effects.py`, identical in both bundles, checked by `airflow_effects_copies_match`). The one-try checks need one try in `task_instance`, no row in `task_instance_history` and exactly one effect per task instance; the Edge cases also read `edge_job` (which worker ran which try) | 6 |
| T6.R5 | **No CeleryExecutor leg.** OC0 Task 6's `CeleryExecutor` run (with Task 4's `db+` backend) is not done: §74 replaced it with the Edge executor as the DX0 baseline and `LoamsResonateExecutor` for DX1A (D1525), and OC0 Task 4 has not landed. The comparison leg is `LocalExecutor` (`AIRFLOW_EXECUTOR=LocalExecutor run.sh`, its own `local/xfail.toml`) | 6 |
| T6.R6 | The overlay shortens Airflow's failure detection so the chaos cases finish in minutes: `task_instance_heartbeat_timeout` 60 s (detection every 10 s), `scheduler_health_check_threshold` 30 s, orphan checks every 20 s, edge heartbeat 10 s and job poll 2 s. `airflow_edge_worker_kill_retries` therefore allows 120 s from the `kill -9` to try 2's start (the heartbeat timeout, plus detection, the task's 5 s retry delay and 45 s for a poll and a cold task start). Finished `edge_job` rows are kept for a day so the checks can read them | 6 |
| T6.R7 | **The transaction-mode leg** moves only the two schedulers and the triggerer to PgDog (`pgdog-txn.yaml`, user `airflow`); the API server, the DAG processor and `airflow db migrate` (a session advisory lock) stay direct, and the edge workers never touch the database. It checks that the recreated services' `sql_alchemy_conn` names PgDog, that three new scheduler/triggerer jobs heartbeat, and that a deferrable sensor and 30 mapped tasks each ran once. `xfail.toml` starts empty: the first run decides whether the case is an expected failure (with the reason) or whether `endpoint-modes.md` may allow transaction mode for these processes | 2, 6 |
| T6.R8 | The edge workers skip the image entrypoint's database check (`CONNECTION_CHECK_MAX_COUNT=0`), so they start the way a remote worker does, with no route to Loams Postgres; they still carry the shared settings (`compose_compute_uses_direct_endpoint` is unchanged). `af.py log` asks for JSON: Airflow 3.3's log endpoint answers only `application/json` or `application/x-ndjson` (`api_fastapi/common/headers.py`, 406 otherwise) | 6 |
| T6.R9 | **Status, 2026-10-27: written, not yet run end to end.** The owner ruled out local runs for this wave ("do not run build etc"), so the suite has not run against a stack; only the self-test (`airflow_python_parses`, `airflow_effects_copies_match`, `airflow_af_log_lines`, `compose_airflow_test_overlay_valid`) runs per pull request. Both `xfail.toml` files are empty and strict, so the first nightly (Task 7) fails on any failure until its results are recorded with reasons and issues. OC0 Task 6 is marked landed in OC1's rulings (R0.3), with the same status | 6 |
| T7.R1 | **The nightly is `.github/workflows/compute-compat.yml`, dispatched by `nightly.yml`** (a `schedule:` runs only on `main`), and can be dispatched by hand with a subset of legs (`products`). One job per leg runs on GitHub-hosted `ubuntu-24.04`: `durable`, `temporal-pg16`, `temporal-pg17`, `corpus`, `dbos` (it notes that the harness is absent until Task 5 merges), `airflow` and `airflow-local`. There is also an online licence gate. A final job renders the matrix into the step summary and an artifact. The committed `docs/guides/compute/compatibility.md` is refreshed by a manual pull request, as OC0 specifies. The per-pull-request checks stay in `ci.yml`'s repo-checks (T1.R4) | 7 |
| T7.R2 | **The matrix's `durable` section** (`scripts/compute/matrix.py durable`, beside DX1 Task 11's `temporal-gateway` section, X11.R1) has one row per leg, spliced between `<!-- matrix:durable:begin/end -->` in `docs/guides/compute/compatibility.md`. It reads each leg's JUnit files by name anywhere under the downloaded artifacts and applies `xfail_check.py`'s strict check. The verdicts are **green**, **red**, **not run** (no JUnit) and **not merged** (the product's directory is absent). Each row shows the version from `pins.env`, the endpoint mode, the counts and the expected failures with their issues; the corpus line adds scenario counts per phase. `matrix_durable_from_fixture` renders `conformance/compute/testdata/matrix-durable/` and compares the result with its `expected.md`. Only the "Durable execution" section exists: OC0's other products (RisingWave, Sail, the queues) add their rows when they land | 7 |
| T7.R3 | The corpus's nightly leg is `conformance/compute/temporal/corpus/replay.sh`. It runs `corpus_replays_go` and `corpus_replays_ts` in the pinned Go and Node images, each on a copy inside the container, and checks them against the new, empty and strict `temporal/corpus/xfail.toml`. `corpus_regenerates_stably` needs a server and stays a manual `generate.sh` run until compat mode is go (T4.R1) | 4, 7 |
| T7.R4 | **Docs.** `docs/guides/durable/` has `README.md` (§74 §10's table, D1530, plus the status of each product), `temporal-compat-mode.md`, `dbos.md`, `airflow.md` and `snippets/`. `docs/guides/compute/` has `README.md`, `airflow.md` (OC0 Task 10's Airflow recipe, which the durable page links) and the generated `compatibility.md`. The other OC0 recipes are not written. The tests are in `selftest.sh`. `docs_compute_snippets_match_files` uses `<!-- snippet: path -->` markers, and every snippet file must be used. `docs_compute_wording` refuses "Temporal-compatible", "drop-in" and "production-ready", and needs a `Status:` line on each page. `recipe_endpoint_mode_matches_suite` compares each page's `Endpoint mode: **x**` with `endpoint-modes.md` and with the suite's `# Endpoint mode: x` line. `compute_compat_workflow_runs_every_suite` checks that the nightly runs every suite | 7 |
| T7.R5 | **`docs/plans/dx0-exit-report.md` is a draft** (owner rulings of 2026-10-27 recorded there: the matrix page is refreshed by hand, and compat mode becomes a documented option once go), and DX0 has not exited. No nightly has run, DBOS (Task 5) is not merged, and compat mode is not yet go (#1185). The report is refreshed from the first nightly's matrix before the owner reviews it. DX1 already runs in parallel (D1590) | 7 |
| T7.R6 | **Running the nightly before a release.** `workflow_dispatch` needs the workflow file on the default branch (`main`), which moves only at a release, so `gh workflow run compute-compat.yml` answered 404 on 2026-10-28. `compute-compat.yml` also runs on a push to a `compute-compat/**` branch, which runs every leg. `nightly.yml` warns instead of failing while the dispatch is refused | 7 |
| DX1F.R1 | **A history long poll that times out keeps a page token** (DX1 exit report §4 item 1, found by ostrium-labs/resonate#26). `GetWorkflowExecutionHistory` with `wait_new_event` and `HISTORY_EVENT_FILTER_TYPE_CLOSE_EVENT` that reaches its deadline (X1.R3: 60 s, or 5 s before the request's) before the run closes answers an empty page with a non-empty `next_page_token` (the 8-byte event cursor), as Temporal's server does; it used to copy the request's token, which is empty on the SDK's first call, and the Go SDK then failed with "could not get last history event" on any run longer than one poll. Test: `close_event_long_poll_keeps_a_page_token_past_its_deadline` (300 ms `history_wait`) | DX1 follow-up |
| DX1F.R2 | **X1.R7's limits are enforced as Temporal v1.32.1 enforces them** (DX1 exit report §4 item 2). *Blobs (2 MiB, `size > limit` on the encoded message):* start input and signal input → `InvalidArgument` "Blob data size exceeds limit." (`common/util.go` `ErrBlobSizeExceedsLimit`); an activity's result, cancel details or heartbeat details → the activity fails with a non-retryable server failure "Complete result exceeds size limit." / "Cancel details exceed size limit." / "Heartbeat details exceed size limit." (the heartbeat answers `cancel_requested`); an oversized activity or workflow-task failure is replaced by "Failure exceeds size limit." (Temporal also keeps a truncated copy as the cause; the gateway leaves that out); a command's payload (schedule-activity input, complete result, fail failure, marker details) fails the workflow task with Temporal's cause and a server failure `"<CAUSE>: <Attr>.<Field> exceeds size limit."` and terminates the run with that reason, identity `history-service` (`respondworkflowtaskcompleted/api.go`). *Pending activities (2,000):* the 2,001st fails the workflow task with `PENDING_ACTIVITIES_LIMIT_EXCEEDED`, "the number of pending activities, 2000, has reached the per-workflow limit of 2000"; a later attempt that breaks a limit again is refused with `InvalidArgument` and nothing is written, as Temporal does for attempt > 1. Child workflows, external signals and external cancel requests are `UNIMPLEMENTED` (§5.8), so their counts cannot be reached yet. *History (50 MiB, 51,200 events; `workflow/context.go` `UpdateWorkflowExecutionAsActive`):* a change to a run whose written history is over 50 MiB, or that would take it past 51,200 events, is discarded and the run is terminated instead, reason "Workflow history size exceeds limit." / "Workflow history count exceeds limit.", identity `history-service`. A non-retryable `ServerFailureInfo` now ends activity retries, as `retrypolicy.IsRetryable` does. Tests: `crates/loams-temporal-statemachine/tests/limits.rs` (one per limit) and `a_payload_over_2_mib_is_invalid_argument` | DX1 follow-up |
| DX1F.R3 | **Burst and chain load: the cause of "could not get last history event" is DX1F.R1, not the store** (resonate-bench 2026-10-26: 500 runs started at once, and a 5-activity + 5 × 1 s sleep chain, on a gateway doing 1.3 wf/s). Read on 2026-10-27: the engine's commit is one compare-and-set transaction holding the events, the mutable state and the outbox rows (`PgStore::commit`, `SELECT … FOR UPDATE` on the version, then the update and the event rows in one transaction; `MemStore` under one lock), and `close` sets `status` and `completion_event_id` in the same state, so a watcher that sees the run closed always loads a completion event. Under that load every run outlived the 60 s long poll, and every one of those polls took the timeout branch that answered an empty, tokenless page; the watcher-gone branch answered the same. Both now keep the token (DX1F.R1). Also: a client deadline of 5 s or less holds the poll for half of it rather than all of it, so the client gets the empty page and token before its own deadline fires (`wait_within`). Tests: `burst_close_event_waiters_all_get_their_close_event` (40 runs, the Go SDK's `run.Get` loop, closes spread across several 150 ms deadlines), `full_history_long_poll_keeps_a_token_while_the_run_is_open`, `a_long_poll_answers_before_the_clients_deadline`. Not yet re-run against the bench | DX1 follow-up |
| X11.R1 | **`scripts/compute/matrix.py` lands with DX1 Task 11**, because OC0's had not: subcommands per section, `temporal-gateway` first, and a `SECTIONS` table where DX0 Task 7 adds "Durable execution" (and OC0 its products). The gateway's matrix lives in `docs/guides/durable/temporal-gateway.md`, between `<!-- matrix:temporal-gateway:begin/end -->` markers, because the frontend's `MATRIX_URL` (the link in every `UNIMPLEMENTED` answer) already points there. `--splice` writes it and `--check` compares it. The JUnit it is generated from is committed (`conformance/compute/temporal/gateway/last-run.junit.xml`, with `last-run.txt` naming the run). `junit.py` gains `read()` | 11 |
| X11.R2 | **The matrix states what its run does not cover.** `methods.toml` takes a top-level `scope` (rendered as "Not covered by this run: …") and per-method `[[method.known]]` entries (`sdks`, `text`) for issues the SDK legs do not show, such as ones seen only under load; the named SDKs' cells read "pass, known issue N" and the issues are listed under the table. The gateway's first entry is the Go SDK's "could not get last history event" under the 2026-10-25 bench's burst and chain loads (DX1F.R1, fixed in #1291, bench not re-run) | 11 |
| X11.R2 | **One row per method, from cases that are not per method.** The SDK legs' cases are per scenario, so `conformance/compute/temporal/gateway/methods.toml` maps each DX1 method to the cases that exercise it (`{sdk}` per SDK, `only` for one SDK). A cell is the worst state of its cases, and a method with no case is "not exercised". The map was checked against a temporary request log on the gateway during the 2026-10-27 run (301 calls, 18 methods). The self-test `matrix_guide_current` checks that the guide matches a fresh render and that `methods.toml` lists exactly `capabilities::IMPLEMENTED`. `matrix_from_fixture` covers every cell state | 11 |
| X11.R3 | **The API-key path gets a test** (`api_keys_authenticate_and_bind_the_loams_namespace` in `loams-temporal-frontend/tests/standalone.rs`). Nothing tested `StaticKeys` before, and the SDK legs run without a key (`insecure_dev`). It is a test only; no gateway code changed | 11 |
| X11.R4 | **DX1 exit verdict** (`dx1-exit-report.md`): §74 §13.3's bar is met on loopback, with the run on the in-process Resonate (2026-10-27) and on the fork's server (X1.R21). The verdict for DX2 is a conditional go. Two defects come first: the close-event history long poll answers without a page token at its deadline (found by ostrium-labs/resonate#26; the Go SDK fails on runs that outlast the poll), and X1.R7's payload, history and pending-activity limits are defined but not enforced. Not delivered: the compose profile `temporal-gateway` (needs a gateway image), the legs' nightly, and the FW3 pin bump (FW3, FW7's fixes, FW8 and FW11 are merged on the fork's `loams/main` but not pinned). The two defects' issues could not be filed from the Task 11 session; the report has their text | 11 |
| X1.R22 | **`resonate-client` replaces the interim adapter's requests** (2026-10-27, #1292; D2040–D2042): `HttpDurability` is now a thin adapter on FW3's `resonate-client`, pinned at the workspace's fork revision 37b177cd (client and core identical to `loams/main` f29c04e1). What is left of `wire_interim` is the poll subscription (the client has no poll consumer), now a private module; the exit report lists it with FW3's state. `with_retry` takes `resonate_client::RetryPolicy` and returns a `Result`, and `TokenFn` moves to the crate root. The `profile_*` tests are unchanged in substance and still need a real server (`LOAMS_TEMPORAL_TEST_RESONATE_URL`); they have not been rerun on the new adapter yet | 6 |
| X1.R23 | **The interim adapter is gone** (2026-10-28; D2043): `HttpDurability::subscribe` reads the poll transport through `resonate-client`'s `Client::poll` (fork #28), mapping `execute` to `TaskReady`, `unblock` to `Settled` and `Reconnected` to `Delivery::Gap` (X1.R19 unchanged), and `wire_interim` is deleted. The fork pin moves to 53d38ab3 for every Resonate crate. `DelaySupport` stays `Defer` by default (D2042) even though the embedded server now honours `resonate:delay`, because the gateway may point at any server. The `profile_*` tests against a real server have not been rerun yet | 6 |
| X10.R1 | **The gateway image and the profile** (Task 10; D2030, D2031): `deploy/release/Containerfile.loams-temporal` is a release image (`ghcr.io/ostrium-labs/loams-temporal`, from the first release after 0.1.2), and the profile `temporal-gateway` is in `deploy/release/compose.compute.yaml` with its own `loams_temporal` database (`durable-init.sh`) and the engine's Resonate in the engine's network namespace. `conformance/compute/temporal-gateway.yaml` builds the pull request's image from that Containerfile and mounts the harness's `gateway.toml` (a test API key, a test task-token key). Self-tests: `gateway_profile_no_host_ports`, `gateway_profile_shape`, `gateway_image_is_release_image` | 10 |
| X10.R2 | **The SDK legs on pull requests** (Task 10): `.github/workflows/temporal-gateway.yml` runs `run.sh --profile` when a pull request touches the gateway's crates, the harness, the profile or the image; not in `CI required` yet. Each SDK container is killed after `LEG_TIMEOUT` (900 s), so a hung scenario fails its case, and the gateway's (debug for `loams_temporal_frontend` and `loams_temporal_resonate`) and the engine's logs are kept as the run's artifact. The corpus workers send `TEMPORAL_API_KEY` as `authorization: Bearer` in plain text (Go: `client.Options.HeadersProvider`; TypeScript: connection `metadata`), not the SDKs' API-key credentials, which turn TLS on. First profile run (2026-10-27, run 40441294397): the image built and the gateway served on Loams Postgres; the TypeScript leg passed `hello` and `two_activities_sequential` and then hung in `activity_parallel_fanout_10` twice (runs 40441294397 and 40491482849), with one worker process driving every scenario in turn. Since `run.sh` gives each TypeScript scenario its own worker process (`TS_SCENARIO_TIMEOUT`), run 40539982805 passed all 28 cases: Go and TypeScript, every DX1 scenario, both replayers and the differential, on Loams Postgres and the engine's Resonate. The hang in a long-lived TypeScript worker is not diagnosed (the gateway logged nothing for it, and the engine logged only `task.acquire` 409s); it is the first thing to look at under the TypeScript load runs | 10 |
| X10.R3 | **Task-token keys** are at least 32 bytes; an empty or shorter one is refused at start (D2032) | 9, 10 |
