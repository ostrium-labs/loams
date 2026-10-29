# OC1 — Open-Protocol Compute: Airflow, Temporal, Streaming SQL, Spark and Job Queues on Loams (plan stub)

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Where this plan gives exact names, paths, branches or tests, use them verbatim. Record any deviation in "Rulings made during execution" at the end of this file.
>
> **Status: Stub** (2026-10-20). Track OC, design [§68](../design/68-open-protocol-compute.md) (D1251–D1275, Q1030–Q1045, **approved by the owner on 2026-10-21 with every default**). This plan specifies **OC0** (conformance of unmodified products on Loams primitives, recipes, an opt-in Compose file and a compatibility matrix; no engine code) task by task. OC1 (the Airflow provider, the gateway routes, guides), OC2 (managed companions), OC3 (protocol endpoints) and OC4 (a native continuous-query engine) are listed at milestone level and get full task specifications when their gates are met (D1273). **OC0 starts after v0.1.1 ships (Q1030).**

**Goal (OC0):**
- unmodified Airflow 3.3, the Temporal server, RisingWave, Sail and eight Postgres-backed job queues run against Loams Postgres, the bucket (RustFS) and Lakekeeper, each judged by **its own** test suite or documented smoke test plus the few HA and fault cases Loams' primitives must survive;
- every result is recorded in a strict expected-failure list per product, so a surprise in either direction fails;
- a reader follows `docs/guides/compute/<product>.md` and the opt-in `deploy/release/compose.compute.yaml` to reproduce each setup;
- `docs/guides/compute/compatibility.md` is generated from the last nightly run;
- the Temporal persistence spike answers whether "Temporal on Loams Postgres" can be a recipe (D1259).

**Architecture** (§68 §10.1, §12): no crate changes. One Compose override with a profile per companion, one conformance tree `conformance/compute/` (a shared library, a pins file, a directory per product), one generator script `scripts/compute/matrix.py`, one workflow `.github/workflows/compute-compat.yml`, and one docs section `docs/guides/compute/`.

**Tech stack:** Docker Compose v2, bash with `set -euo pipefail` and shellcheck, Python 3.12 (stdlib only for the generator), the products' own test runners (pytest, Node's test runners, `go test`, `mix test`), `psql`, the AWS CLI or `mc` for bucket checks, PySpark 4.x client for Sail.

## Global Constraints

- **Worktrees and branches.** `~/Documents/Ostriumlabs/loams-wt/oc0-<task>`, one branch and one PR per task into `dev` (CodeRabbit reviews; merge on green). `git commit -s`.
- **No Rust build.** OC0 touches no crate. If a task finds it needs one (for example a Loams Postgres fix), stop, open an issue against the owning track (§51 for the fork, GW1 for the gateway) and record a ruling; the product's row stays red in the matrix.
- **No forks, no patches to upstream products.** A product runs exactly as released; only configuration differs. A failure is recorded, not worked around in the product.
- **Pins (D1274).** Every image by digest, every package by exact version, at a release at least 14 days old at the time of the PR, except security fixes. Pins live in `conformance/compute/pins.env`, read by every script, the Compose file (through `--env-file`) and the workflow.
- **Licences (D1271).** No image or package under BSL, SSPL, ELv2, CCL, AGPL or RSAL; no vendor enterprise feature or licence key (RisingWave Premium, BullMQ Pro, Spice.ai Enterprise). Test `licence_gate_compute_pins` (Task 1) runs D359's licence check over every pin.
- **Never public.** No host port in `compose.compute.yaml` beyond those the base `compose.yaml` already publishes; companions reach Loams on the compose network only.
- **GitHub-hosted runners only.** Suites run nightly and on `workflow_dispatch`; per-PR jobs only lint and validate files.
- **Wording (D1275).** Docs say "runs X on Loams" and link the matrix; test `docs_compute_wording` (Task 10) fails on "Airflow-compatible", "Temporal-compatible" or "Spark replacement".

## Review Focus

1. **A pass that proves nothing.** Expected: each product's own suite or documented smoke test runs, and the HA cases check the effect (no double-run, data read back), not only exit codes. Tests: `airflow_ha_no_double_run`, `rw_restart_reads_hummock`, `temporal_persistence_suite`.
2. **A silent workaround.** Expected: no patched product, no disabled upstream test without an `xfail.toml` entry with a reason and a tracking issue. Test: `xfail_entries_have_reason_and_issue`.
3. **An xfail that hides a fix.** Expected: an expected failure that passes fails the run. Test: `xfail_strict_unexpected_pass`.
4. **The wrong Postgres endpoint mode.** Expected: each recipe states transaction-pooled, session-pooled or direct, and the suite runs on exactly that mode. Test: `recipe_endpoint_mode_matches_suite`.
5. **A licence or enterprise feature slips in.** Expected: never. Tests: `licence_gate_compute_pins`, `no_licence_key_in_compute_files`.

## OC0 — Conformance, recipes and the matrix (Tasks 0–11)

### Task 0 — Rulings

Check and record, in "Rulings made during execution":
- the pins for `pins.env`: Airflow 3.3.x (newest patch at least 14 days old) and its constraints file; the official `apache/airflow` image digest; Temporal server and `admin-tools` images (1.32.x); RisingWave 3.1.x image; Sail (`pysail`) 0.7.x and a PySpark 4.x client; Lakekeeper (the version §49 pins); pg-boss, Graphile Worker, BullMQ (6.3.x, the Postgres backend), River, Oban, Hatchet, Procrastinate and Celery with SQLAlchemy;
- which Loams Postgres endpoint the release compose exposes (pg-control's compute endpoint through `loams-gateway` `pg`, its pooling mode, and whether a session-pooled or direct endpoint exists for `LISTEN`/`NOTIFY` and session advisory locks); if none exists, the suites that need one run against the direct compute endpoint and the gap is filed against GW1;
- whether Loams Git (Smart HTTP, D394) is served by the release `loams` binary; if not, Airflow's `GitDagBundle` case uses a plain `git daemon` container and the Loams Git case waits (xfail with the issue);
- whether Lakekeeper runs in the release compose; if not, `compose.compute.yaml` adds it under the `sail` and `risingwave` profiles with a Postgres database on Loams Postgres (D413's layout);
- for each Postgres-backed queue: the command that runs its own test suite against an external database URL (or, where the suite cannot target one, the documented smoke example to run instead);
- Temporal's SQL persistence test entry point (the `persistence` tests under `common/persistence/tests` with the `postgres12` plugin, or the `temporal-sql-tool` plus a documented soak), and whether it can run from the released `admin-tools` image or needs the source at the pinned tag.

Files: this plan. Tests: none. Gate: rulings recorded.

### Task 1 — The harness

Files:
- `conformance/compute/README.md` (layout, how to run one product, how to add one)
- `conformance/compute/pins.env`
- `conformance/compute/lib.sh`: `oc_up <profile>` and `oc_down <profile>` (Compose with the pins), `oc_wait_pg`, `oc_wait_http <url>`, `oc_bucket_ls <prefix>`, `oc_junit_case <suite> <name> <status> [message]`, `oc_xfail_check <product> <junit.xml>`
- `conformance/compute/xfail.schema.json` and the format of `conformance/compute/<product>/xfail.toml`: entries `{ id, tests, reason, issue, since }`; `issue` required
- `scripts/compute/xfail_check.py`: reads a JUnit file and an `xfail.toml`; fails on an unlisted failure and on a listed test that passed
- `scripts/compute/licence_gate.sh`: resolves each pin's licence (image label `org.opencontainers.image.licenses`, PyPI or npm metadata, or a `LICENSE` fetched at the tag) and applies D359's refused list

Tests (in `conformance/compute/selftest.sh`): `xfail_strict_unexpected_pass`, `xfail_unlisted_failure_fails`, `xfail_entries_have_reason_and_issue` (each with fixtures under `conformance/compute/testdata/`), `licence_gate_compute_pins`, `licence_gate_refuses_bsl_fixture`, `no_licence_key_in_compute_files` (greps `conformance/compute/` and `deploy/release/compose.compute.yaml` for `RW_LICENSE_KEY`, `BULLMQ_PRO`, `SPICE_` licence variables).

### Task 2 — `compose.compute.yaml`

Files: `deploy/release/compose.compute.yaml`, `deploy/release/compute/` (per-profile config: `airflow/airflow.cfg.env`, `risingwave/risingwave.toml`, `sail/env`, `lakekeeper/env`, `temporal/development-sql.yaml` if Task 7 passes).

- Profiles `airflow`, `temporal`, `risingwave`, `sail`, `lakekeeper` (pulled in by `sail` and `risingwave`); every service has `profiles:`; nothing starts without one.
- Databases are created on Loams Postgres by a one-shot `compute-init` service per profile (`CREATE DATABASE airflow`, `temporal`, `temporal_visibility`, `risingwave_meta`, `lakekeeper`), idempotent.
- Buckets on RustFS by the same one-shot (`airflow-logs`, `risingwave-hummock`, `warehouse`).
- No `ports:` except where Task 0 rules a UI must be reachable through the existing tailnet route; restricted users where the image allows.

Tests: `compose_compute_config_valid` (`docker compose -f compose.yaml -f compose.compute.yaml config` for each profile), `compose_compute_requires_profile` (no service without `profiles`), `compose_compute_no_new_host_ports`, `compose_compute_images_pinned_by_digest`.

### Task 3 — Postgres queues: TypeScript

Products: BullMQ v6 with its Postgres backend, pg-boss, Graphile Worker.
Files: `conformance/compute/{bullmq-pg,pg-boss,graphile-worker}/{run.sh,xfail.toml,README.md}`.
Each `run.sh` installs the pinned package, points its own test suite (or Task 0's documented smoke) at the Loams Postgres URL for the endpoint mode Task 0 chose, writes JUnit, and runs `oc_xfail_check`.

Tests: `bullmq_pg_suite`, `pgboss_suite`, `graphile_worker_suite`; plus Loams cases per product: `<p>_two_workers_no_double_run` (1,000 jobs, two workers, each job handled once), `<p>_worker_kill_redelivers` (`kill -9` mid-job, the job completes on the other worker), `<p>_notify_wakes_worker` (where the product uses `LISTEN`/`NOTIFY`; latency under 1 s).

### Task 4 — Postgres queues: Python

Products: Celery's `db+postgresql://` result backend (with an in-compose broker the base stack already has, or the `memory://` transport for single-process cases; Task 0 rules), Procrastinate.
Files: `conformance/compute/{celery-db-backend,procrastinate}/…`.
Tests: `celery_db_backend_results` (100 tasks, results and states read back through `AsyncResult`), `celery_db_backend_chord` (a chord's counter on the database backend), `procrastinate_suite`, `procrastinate_two_workers_no_double_run`.

### Task 5 — Postgres queues: Go and Elixir

Products: River, Oban, Hatchet.
Files: `conformance/compute/{river,oban,hatchet}/…` (River and Oban run their suites in their language images; Hatchet runs its documented quickstart and its engine against Loams Postgres).
Tests: `river_suite`, `river_leader_election`, `oban_suite`, `hatchet_quickstart`, and the `<p>_two_workers_no_double_run` case for each.

### Task 6 — Airflow on Loams (L0)

Files: `conformance/compute/airflow/{run.sh,dags/,xfail.toml,README.md}`, the `airflow` profile of Task 2.
Setup: Airflow 3.3 from the pinned image with §68 §4.3's settings (metadata DB on Loams Postgres in Task 0's mode, remote logging and `XComObjectStorageBackend` on RustFS, a `GitDagBundle`); `LocalExecutor` and then `CeleryExecutor` with the Celery `db+` backend of Task 4 if Task 0 confirms a broker; two schedulers.
Tests:
- `airflow_db_migrate_empty` (from an empty database; the advisory lock is released: `pg_locks` shows none afterwards)
- `airflow_ha_no_double_run` (two schedulers, a DAG with 500 mapped task instances; every task instance has exactly one try)
- `airflow_scheduler_kill` (`kill -9` one scheduler mid-run; the run finishes on the other)
- `airflow_remote_logs_after_worker_gone` (the task log is served from the bucket after the worker container is removed)
- `airflow_xcom_object_storage` (an XCom above the threshold is stored in the bucket and read by the next task)
- `airflow_git_bundle_version` (a run records the bundle commit; a later push does not change a running DAG run's version)
- `airflow_rest_api_v2_smoke` (trigger, list, and fetch task logs through `/api/v2` with a token from `/auth/token`)

### Task 7 — The Temporal persistence spike

Files: `conformance/compute/temporal/{run.sh,xfail.toml,README.md,SPIKE.md}`.
Steps: create `temporal` and `temporal_visibility` with `temporal-sql-tool` on Loams Postgres; run Temporal's SQL persistence tests at the pinned tag (Task 0's entry point) against it; then a soak: the stock server (four services, `numHistoryShards = 16`) with the `samples-go` load workflow for 30 minutes and a history-service restart halfway.
Tests: `temporal_schema_setup`, `temporal_persistence_suite`, `temporal_soak_no_failed_workflows`, `temporal_history_restart_recovers`.
Gate: `SPIKE.md` records the result and the recommendation. **Only if green**, Task 2's `temporal` profile and Task 10's recipe ship; otherwise the matrix shows Temporal red with the reasons, and D1259's recipe waits.

### Task 8 — RisingWave on Loams

Files: `conformance/compute/risingwave/{run.sh,sql/,xfail.toml,README.md}`, the `risingwave` profile.
Setup: RisingWave all-in-one with `hummock+s3://risingwave-hummock` on RustFS (`RW_S3_ENDPOINT`), the SQL meta backend on Loams Postgres, no licence key.
Tests:
- `rw_meta_on_loams_postgres` (the cluster starts and survives a restart with its catalog)
- `rw_restart_reads_hummock` (a materialized view's rows are identical after a full restart)
- `rw_mv_window_join` (a tumbling-window join over two `datagen` sources matches a precomputed answer)
- `rw_iceberg_sink_lakekeeper` (an Iceberg sink through Lakekeeper's REST catalog; the table reads back through Lakekeeper with the same rows)
- `rw_postgres_cdc_from_loams_postgres` (RisingWave's Postgres CDC source on a Loams Postgres table; inserts, updates and deletes arrive; xfail with an issue if logical replication through the fork is not yet available)
- `rw_clickhouse_sink_house` (RisingWave's ClickHouse sink into a House table; xfail if the House profile is not in the release compose)

### Task 9 — Sail corpus

Files: `conformance/compute/spark/{run.sh,corpus/,xfail.toml,README.md}`, the `sail` profile.
Setup: `pysail` server pinned, catalog `iceberg-rest` pointing at Lakekeeper with the warehouse on RustFS, a PySpark 4.x client.
Corpus (each a script with an expected output): `tpch_q01`–`tpch_q22` on generated SF0.1 Parquet registered as Iceberg tables; `df_joins_windows`; `pandas_udf_scalar`; `apply_in_pandas_grouped`; `iceberg_merge_into` (a table Loams does not maintain); `iceberg_overwrite_partition`; `delta_read`; `format_loams_read` (D54; xfail with the issue until the Python data source ships).
Tests: `sail_corpus` (each script a JUnit case), `sail_session_isolation` (two sessions' temp views do not leak).

### Task 10 — Recipes

Files: `docs/guides/compute/README.md` (the ladder in one paragraph, the matrix link), `airflow.md`, `temporal.md` (only if Task 7 passed; otherwise a short page that says why not yet), `risingwave.md`, `spark.md` (Sail; "Spark Connect is served by Sail"), `postgres-queues.md` (one section per framework with the endpoint mode), `celery-bullmq.md` (§26's plugins and the Postgres path side by side).
Each snippet is a file the CI runs (`docs/guides/compute/snippets/`).
Tests: `docs_compute_snippets_match_files`, `docs_compute_wording`, `recipe_endpoint_mode_matches_suite` (each recipe's stated mode equals the mode in that product's `run.sh`).

### Task 11 — Matrix and workflow

Files:
- `scripts/compute/matrix.py`: merges the JUnit files and `xfail.toml` lists into `docs/guides/compute/compatibility.md` (product, version, Loams version, endpoint mode, passed, expected failures with issues, date)
- `.github/workflows/compute-compat.yml`: `on: schedule` (nightly) and `workflow_dispatch`; one job per product, GitHub-hosted `ubuntu-latest`, each running `oc_up <profile>`, the product's `run.sh`, uploading JUnit; a final job that runs `matrix.py` and uploads the page as an artifact (the PR that updates the committed page is manual in OC0); a `pull_request` job limited to `conformance/compute/**`, `deploy/release/compose.compute.yaml`, `scripts/compute/**` and `docs/guides/compute/**` that runs Task 1's self-test, Task 2's validation tests and shellcheck
Tests: `matrix_from_fixture` (a fixture JUnit set renders the expected Markdown), `workflow_paths_filter`.

**OC0 exit:** every product has a run in the nightly with a committed `xfail.toml`; the matrix page is generated; the Temporal spike's `SPIKE.md` has a recommendation; the owner reviews the matrix before OC1 starts.

## OC1–OC4 (milestone level; full tasks when their gates are met)

| Milestone | Content (§68) | Gate |
|---|---|---|
| **OC1** | `airflow-provider-loams` in `ostrium-labs/loams-airflow` (Q1031): `LoamsHook`, operators, `LoamsStreamTrigger`, `LoamsSecretsBackend`, then `LoamsJobsExecutor` with `loams-airflow worker`, pinned to tested Airflow minors, tested against OC0's `airflow` profile; `loams-gateway` `spark` module (D1263) and the `pg` SNI route to a namespace's RisingWave (D1260); the Temporal → Resonate migration guide; Comet and Spice recipes | OC0 exit; JB1's J1 core (`loams.jobs.v1`, `KvJobStore`) for the executor; GW1's `pg` module for the routes |
| **OC2** | `LoamsCompanion` in `loams-operator-api` with `engine` `airflow`, `temporal`, `risingwave`, `sail`; per-namespace placement for engines that run tenant code; `io.loams.dev.usage.companion.runtime.v1`; managed Airflow; managed Temporal; RisingWave and Sail lifecycle shared with JB1 J3/J4 | OP1's `LoamsNamespace`; per engine Q1033 (Temporal), Q1034 (Airflow) |
| **OC3** | Each separately: Dapr pub/sub and state-store components (Q1040); CESQL filters (Q1041); a Celery RESP subset (Q1039); a Spark Connect endpoint on Loams' engine (Q1037); Serverless Workflow front end (Q1042) | Its own question |
| **OC4** | A native continuous-query engine on DataFusion | Q1036 |

## Owner actions

- §68 and its defaults (D1251–D1275, Q1030–Q1045) were approved on 2026-10-21; nothing further for the design.
- Q1031 (default approved): the provider lives in `ostrium-labs/loams-airflow`; the repository is created when OC1 starts.

## Rulings made during execution

| # | Ruling | Task |
|---|---|---|
| R0.1 | **Shared with DX0** (DX1 plan, rulings R0.1 and T1.R1–T1.R7). OC0 Task 1 (the harness) is landed in full by DX1's DX0 Task 1. OC0 Task 2 is landed for the `temporal` and `airflow` profiles (plus DX0's `dbos`); `risingwave`, `sail` and `lakekeeper`, their config under `deploy/release/compute/` and their databases and buckets stay with this plan's Task 2. The pins, licences and endpoint modes recorded there (DX1 R0.2–R0.14) answer Task 0's Temporal, Airflow and endpoint items; the Lakekeeper, RisingWave, Sail and queue items stay open | 0, 1, 2 |
| R0.2 | OC0 Task 7 (the Temporal persistence spike) is landed by DX1's DX0 Task 3: `conformance/compute/temporal/` with `SPIKE.md`. Result on 2026-10-24: Temporal's persistence suite green on Loams Postgres 16, soak green once and red once (#1185), so the recipe (Task 10's `temporal.md`) still waits | 7 |
| R0.3 | OC0 Task 6 (Airflow on Loams) is landed by DX1's DX0 Task 6 (#1294): `conformance/compute/airflow/` with OC0's seven cases, the `airflow` profile's migration, remote logging, object-storage XComs and `GitDagBundle`, plus the Edge executor's cases. The `CeleryExecutor` leg is not done (DX1 ruling T6.R5: Edge replaced it; the comparison leg is `LocalExecutor`). Status on 2026-10-27: written, not yet run end to end (T6.R9) | 6 |
