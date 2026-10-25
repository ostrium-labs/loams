# HE1 — The Loams House Engine: Rust, ClickHouse-Compatible, Iceberg v3 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Where this plan gives exact names, paths, flags or tests, use them verbatim. Record any deviation in the task's rulings file, `docs/plans/he1-rulings/t<N>.md` (indexed in "Rulings made during execution" at the end of this file).
>
> **Status: Approved** (owner, 2026-10-21) for HE0. **HE1 and HE2 specified task by task on 2026-10-23 (Tasks 20–47 and 60–77), HE3–HE5 as milestone plans**, after the [HE0 exit report](he0-exit-report.md); their new decisions (D1576–D1579, D1800–D1805) and questions (Q1188, Q1189, Q1320–Q1322) are **Proposed**, with work proceeding on the defaults until the owner rules. Track HE, design [§75](../design/75-loams-house-engine.md) (D1540–D1569, Q1170–Q1187). §75 was approved by the owner on 2026-10-21 with every default (Q1170 as ruled). This plan specifies **HE0** (the owner's Phase 0), **HE1** and **HE2** task by task, and HE3–HE5 as milestone plans. HE0 writes **no engine code**: it builds the mining tool, the oracle harness and the evidence that decides HE1's scope. It starts after v0.1.1 ships (Q1186).

**Goal (HE0):** know, from real applications rather than guesses, which ClickHouse features Loams House must support; have a harness that compares any House engine against `clickhouse-server` and chDB on those features; and run Langfuse and OpenPanel on today's House (chDB, Iceberg v2). Exit, from §75 §16.1: a mining report for 7 or more applications; Langfuse and OpenPanel run on the Phase 0 House.

**Architecture** (§75 §7, §15):
- **`conformance/clickhouse/apps/<app>/`**: one compose file per mined application, pinned by digest, with a `clickhouse-server` (pinned, `query_log` on) and a driver script that exercises the application (its own end-to-end tests where it has them, else a recorded traffic script).
- **`fabric/crates/loams-house-mine`**: a Rust CLI, `loams-house-mine`, that reads `system.query_log` dumps and migration files and writes a feature inventory (functions, engines, types, formats, settings, syntax forms) with counts per application.
- **`conformance/clickhouse/profile/`**: the generated profile (`profile.toml`) and the report (`docs/house/compat-profile.md`, generated).
- **`conformance/clickhouse/oracle/`**: a runner that executes a corpus on N targets (`clickhouse-server`, the chDB House over HTTP, later the Rust engine), normalises results and writes a triage file.
- **`conformance/clickhouse/stateless/`**: the manifest that filters ClickHouse's `tests/queries/0_stateless` by profile features.

**Tech stack:**
- Rust 1.97, edition 2024, the `fabric/` workspace's lints; `sqlparser` at the version the House pins (0.63.0, HS1 R0.7), `serde`, `toml`, `serde_json`, `clap`.
- Docker Compose v2; images pinned by digest at releases at least 14 days old.
- Python 3.13 with `uv` only where an application's own test runner needs it.
- The reference `clickhouse/clickhouse-server` at the version chDB carries (ClickHouse 26.9.2.1, HS1 R1.1), digest-pinned, CI only; a second pin at 24.3 LTS for the version-range question (Q1170).

**Spec:** [§75](../design/75-loams-house-engine.md) (all of it); [§49](../design/49-loams-house-production.md) §3–§9 and §16; [§32](../design/32-loams-flow-fabric-house.md) §8 and §10 (the surface and the harness, D347–D349); the [HS1 plan](2026-10-08-hs1-house-production.md) and its rulings (what is built).

## Global Constraints

- **Worktrees and branches.** `~/Documents/Ostriumlabs/loams-wt/he0-<task>`, one branch and one PR per task into `dev`. CodeRabbit reviews; merge on green. Commit with `git commit -s`.
- **Light Rust builds.** `cargo build -p loams-house-mine` from `fabric/`; never `--workspace` locally. Build output goes to the shared target in `~/Documents/.cargo/config.toml`; never set `CARGO_TARGET_DIR`, never build in `/tmp`. Stop compose stacks during builds.
- **No engine code in HE0.** No DataFusion dependency enters a merged crate. The dialect spike (Task 10) is a report; its code stays on a branch that is not merged.
- **Licences (§75 §18; FSL as BSL, D1574).** Applications run as unmodified containers pinned by digest; AGPL applications (OpenPanel, Plausible) are never linked, vendored or redistributed. No application source is committed. ClickHouse `.sql`/`.reference` test files may be vendored with attribution (Apache-2.0). ClickBench query text is never committed (D414). Every new crate passes `cargo deny` in `fabric/`.
- **No query text from real users.** Mining runs only on synthetic traffic in CI or local stacks. Dumps are scrubbed of literals before they are committed (`loams-house-mine scrub`).
- **GitHub-hosted runners only.** Application suites that take more than 30 minutes run nightly, not per PR.

## Review Focus

1. **A feature counted for an application that never used it.** Expected: never; every count links to a query id or a migration file line. Test: `inventory_counts_trace_to_sources`.
2. **A literal from a query reaches a committed file.** Expected: never. Test: `scrub_removes_every_literal` (property test over the corpus).
3. **The oracle reports a match when outputs differ only in ways the normaliser should not hide** (for example a different number of rows). Expected: a mismatch. Test: `normaliser_never_hides_row_count_or_types`.
4. **An application's licence is not recorded** before it runs in CI. Expected: the run refuses. Test: `apps_manifest_requires_licence`.

## HE0 — Mining, the oracle and the real applications (Tasks 0–12)

### Task 0 — Rulings

Check and record in "Rulings made during execution":
- **The House as built** at the current `dev`: which HS1 tasks are merged (§75 §3 says Tasks 0–13 and 15–23 have rulings), and what the House answers today for `ALTER … DELETE`, `ALTER … UPDATE`, TTL clauses, `FINAL` on lake tables and `Distributed` engine DDL. These are Langfuse's and OpenPanel's likely gaps.
- **Pins**: `clickhouse-server` 26.9.2.1 and 24.3 LTS digests; each application's image tag and digest at a release at least 14 days old; Langfuse v3 and v4 tags (Q1180).
- **Each candidate application** (Q1185): does it use ClickHouse today, which ClickHouse client and protocol (HTTP or native), its licence (GitHub licence API), and whether it has a runnable end-to-end suite. Record a table; drop any candidate that no longer uses ClickHouse.
- **§75 §4's facts** that HE0 depends on: iceberg-rust's v3 status at its newest release; re-read before Task 9.

Files: this plan. Tests: none. Gate: rulings recorded.

### Task 1 — The apps manifest and compose harness

`conformance/clickhouse/apps/manifest.toml`: per application `name`, `image` (by digest), `licence` (SPDX), `clickhouse_protocol` (`http` | `native`), `driver` (path to its driver script), `suite` (its own test command, if any), `runtime_minutes`. One compose file per application under `conformance/clickhouse/apps/<app>/compose.yaml` that starts the application, its other stores (Postgres, Redis) and a `clickhouse-server` with `query_log` and `part_log` on. `scripts/house/apps.sh <app> up|drive|dump|down`.

Tests: `apps_manifest_requires_licence`, `apps_manifest_pins_by_digest` (static checks in `fabric/crates/loams-house-mine/tests/manifest.rs`); a CI smoke job that brings Langfuse up and down.

Commit: `conformance: the ClickHouse apps manifest and harness (HE0 Task 1)`.

### Task 2 — `loams-house-mine`: the extractor

A CLI with subcommands `extract`, `merge`, `scrub` and `report`:
- `extract --app <name> --query-log <file.tsv> --migrations <dir>`: parses each statement with sqlparser's `ClickHouseDialect`; statements it cannot parse are sent to a pinned `clickhouse-server` as `EXPLAIN AST` and walked from that output. Writes `inventory/<app>.json`: for each feature (`function:<name>`, `engine:<name>`, `type:<name>`, `format:<name>`, `setting:<name>`, `syntax:<form>`) the count and the source ids.
- `merge`: combines inventories and ranks features by application count, then by frequency. A feature is in the profile if 3 or more applications use it, or if a gate application (Langfuse, OpenPanel) uses it (§75 §7.1).
- `scrub`: replaces literals in kept query samples with typed placeholders.
- `report`: writes `conformance/clickhouse/profile/profile.toml` and `docs/house/compat-profile.md`.

Tests: unit tests per feature kind over a hand-written fixture (`fixtures/queries.tsv`); `inventory_counts_trace_to_sources`; `scrub_removes_every_literal`; `unparsed_statements_are_counted_not_dropped`.

Commit: `house: loams-house-mine, the ClickHouse feature extractor (HE0 Task 2)`.

### Task 3 — Mine Langfuse

Run Langfuse v3 and v4 (Q1180) through Task 1's harness: migrations (clustered and unclustered), the application's own end-to-end tests if they run against ClickHouse, plus a driver that ingests traces, observations and scores through its public API and opens the dashboards. Dump, extract, commit the scrubbed inventory.

Check and record: `ReplacingMergeTree` tables and their version columns, `FINAL` use, mutation statements, partitioning, TTL, the formats and settings its client sends.

Commit: `conformance: Langfuse mining run (HE0 Task 3)`.

### Task 4 — Mine OpenPanel

As Task 3 for OpenPanel: its migrations, an event-ingest driver (through its HTTP API so Redis batching runs), and its dashboards, funnels and retention pages. Record whether `windowFunnel`, `retention` or `sequenceMatch` appear (§75 §7.4).

Commit: `conformance: OpenPanel mining run (HE0 Task 4)`.

### Task 5 — Mine five or more further applications

From Task 0's confirmed list (default Q1185: Plausible CE, SigNoz, Umami, PostHog self-hosted, and one more). One commit per application. Then `merge` and `report`: the first `profile.toml` and `docs/house/compat-profile.md`, with Tier 0, Tier 1 and Tier 2 proposals checked against §75 §7.3–§7.5. Disagreements between the mining and §75's tiers are listed for the owner, not resolved silently.

Commit: `conformance: mining runs and the first compatibility profile (HE0 Task 5)`.

### Task 6 — The oracle runner

`conformance/clickhouse/oracle/`: a Rust test binary in `loams-house-mine` (`oracle` subcommand) that runs a corpus of `.sql` cases on a list of targets (`clickhouse-server@26.9`, `clickhouse-server@24.3`, `house-chdb`), each over HTTP with `FORMAT JSONCompactEachRowWithNamesAndTypes`, and compares results after normalisation: float formatting to a declared precision, row order when the case has no `ORDER BY`, and output-format quirks. Writes `oracle/triage.toml` with each mismatch as `bug`, `incompatibility` (documented, with a reason) or `untriaged`. The corpus seeds are the scrubbed mined queries of Tasks 3–5 over generated data of the right schemas.

Tests: `normaliser_never_hides_row_count_or_types`; `unordered_results_compare_as_multisets`; `untriaged_mismatch_fails_the_run`.

Commit: `house: the oracle runner for ClickHouse-compatible engines (HE0 Task 6)`.

### Task 7 — ClickHouse stateless replay manifest

`conformance/clickhouse/stateless/manifest.toml`: the ClickHouse tag (26.9.2.1's), and the filter that keeps a `0_stateless` test when every function, engine, type, format and setting it uses is in `profile.toml` (Task 2's extractor run on the test file). The runner from D349 (HS1 Task 32's, if built; else the oracle runner) runs the kept tests on the chDB House and records the pass rate per category as the HE1 baseline.

Commit: `conformance: stateless tests filtered by the mined profile (HE0 Task 7)`.

### Task 8 — The function and engine matrix

`loams-house-mine matrix`: reads `system.functions` and `system.table_engines` from the pinned chDB and writes `docs/house/compat-matrix.md` with each entry marked `profile` (in Tier 0 or 1), `refused` (Tier 2, `48`), `disabled` (security, `344`) or `unclassified`. A CI check fails on `unclassified` entries for features the mining found.

Commit: `house: the generated function and engine matrix (HE0 Task 8)`.

### Task 9 — Langfuse and OpenPanel on today's House

> **Amended by D1571 (owner, 2026-10-22; [R9.60](he1-rulings/t9.md)).** SigNoz (its MIT build, `signoz/signoz-community`) is the third gate application and runs fully on the House; its gate job is `signoz_on_house_smoke`.

> **Remaining (owner, 2026-10-22: House work stops after the open PRs; [R9.64](he1-rulings/t9.md)).** For SigNoz: refreshable materialized views (`REFRESH EVERY … APPEND TO`, designed in R9.64, not built), L's DDL items from the first SigNoz replay, and the gate job with its House overlay. Done: D1572 and R9.61 (nanosecond times), R9.62 (cluster `system` tables) and R9.63 (`JSON` columns).

> **Status 2026-10-24 ([R9.70](he1-rulings/t9.md)–[R9.73](he1-rulings/t9.md)).** Built: refreshable materialized views and `PREWHERE` on lake tables (R9.70), Distributed columns that follow their local table (R9.71), the Factory's DSN on the native TLS listener (R9.72), and the gate (`app-on-house.sh signoz`, the multi-database replay, `signoz/compare.py`, the nightly `signoz-on-house` job). The replay passes (373/373 DDL, 194 identical SELECTs plus 1 `insert-time`) and SigNoz's own migrations run on the House over native TLS. **Left:** one end-to-end run after R9.71 (collector ingest, then the API comparison); then make the job required or keep it nightly (about 17 min locally), and take off the preview label.

Point each application at the chDB House (lake tables, Iceberg v2) instead of `clickhouse-server` and run its suite and driver. For each failure: a fix in the House if HS1 has the piece and it is small; else a ruling that names the gap and the HE phase that closes it (for example mutations, D1552, HE2). Exit for this task: both applications start, ingest and render their main pages on the House, and the remaining gaps are listed.

Tests: `langfuse_on_house_smoke` and `openpanel_on_house_smoke` in a nightly job.

Commit: `conformance: Langfuse and OpenPanel against the House (HE0 Task 9)`.

### Task 10 — Spike: the ClickHouse dialect on DataFusion (report only)

On an unmerged branch: parse every mined statement with sqlparser's `ClickHouseDialect` and record the parse rate; plan 20 representative Tier 0 queries into DataFusion logical plans with stub functions; record what DataFusion lacks (`LIMIT BY`, `WITH TOTALS`, `ARRAY JOIN`, `FINAL`, the combinators) and the cost of each. Pick the versions of DataFusion and iceberg-rust that share an Arrow major (Q1175). Write the result to `docs/plans/he0-dialect-spike.md`.

Commit: `docs: the HE0 dialect spike report (HE0 Task 10)`.

### Task 11 — Spike: iceberg-rust v3 (report and upstream issues)

Write Iceberg v3 tables with Spark (deletion vectors, row lineage, Variant) and read them with the newest iceberg-rust; record what reads, what fails and what is missing for writes. File or link upstream issues for each gap (D1553), with the owner's consent before anything is posted. Record the interop baseline: today's House v2 tables read by DuckDB, Spark and Trino. Write `docs/plans/he0-iceberg-v3-spike.md`.

Commit: `docs: the HE0 Iceberg v3 spike report (HE0 Task 11)`.

### Task 12 — HE0 exit

> **Done 2026-10-23** ([R12.1](he1-rulings/t12.md)): [`he0-exit-report.md`](he0-exit-report.md), with go for HE1. HE1 and HE2 are specified below (Tasks 20–77).

An exit report in `docs/plans/he0-exit-report.md`: the profile and its tiers, the oracle's first triage, the stateless baseline, Langfuse and OpenPanel on the House, the two spikes, and a proposed HE1 task list with estimates. The owner reviews it before HE1 is specified.

Gate: Tasks 1–11 merged; the report written; §75's open questions that HE0 answers (Q1170, Q1175, Q1180, Q1185) proposed for the owner with evidence.

## Queued gate applications, after SigNoz (Tasks 13–15)

> **Queued by the owner 2026-10-22** ([R13.1](he1-rulings/t13.md), [R14.1](he1-rulings/t14.md), [R15.1](he1-rulings/t15.md)); Laminar replaces Helicone and the order is fixed (owner, 2026-10-22; [R14.2](he1-rulings/t14.md)). These are stubs. **None of them starts** until SigNoz's gate (Task 9, D1571) is done or the owner says so. They are not part of the HE0 exit (Task 12) unless the owner adds them. Evidence for every licence, protocol and stack fact below is in [`docs/research/clickhouse-apps.md`](../research/clickhouse-apps.md) (checked 2026-10-22). When a task starts, its application becomes a gate application under §75 §7.1's rule, and a decision from the reserved D1572–D1579 records that.

Each task follows Task 9's pattern: a manifest entry and compose stack (Task 1), a driver, a mining run (Tasks 2–5) against the 26.9.2.8 oracle, a replay of the oracle traffic against the House (R9.3), then fixes or rulings per gap, and a nightly `<app>_on_house_smoke` job. Supporting images follow R0.4 (Valkey for Redis, RustFS for MinIO).

**Order after SigNoz (owner, 2026-10-22; R13.2, R14.2, R15.2): Task 15 (BI tools) first, then Task 13 (OpenMeter), then Task 14 (Laminar).** Each starts when the one before it is done, unless the owner says otherwise.

### Task 13 — Queued gate: OpenMeter

**Why.** OpenMeter is the model and event format Loams Meter follows (§63 D1121, D1124, D1131). Running OpenMeter on the House dogfoods §63's House half: the same usage events and meter queries, on the store Loams Meter itself uses.

**Application.** `openmeterio/openmeter`, Apache-2.0, no `ee/` split; owned by Kong since 2025-09. Releases are weekly `v1.0.0-beta.N` tags (`v1.0.0-beta.236`, 2026-10-09); pin one at least 14 days old, image `ghcr.io/openmeterio/openmeter` by digest (one image, six commands).

**Protocol: native only.** OpenMeter opens ClickHouse with `clickhouse-go` v2 and no protocol option (`app/config/aggregation.go`, default address `127.0.0.1:9000`), for inserts, migrations and queries. So this gate **needs the native listener built for SigNoz** (D1571); it is the second native-only gate and reuses that work. Whether an `http://` address is accepted is unknown (verify; the config builds `clickhouse.Options` without `Protocol`).

**How OpenMeter's use maps onto §63.**

| OpenMeter | Loams Meter (§63) | What the gate shows |
|---|---|---|
| `om_events`: `MergeTree`, `PARTITION BY toYYYYMM(time)`, `ORDER BY (namespace, type, subject, toStartOfHour(time))`, `data String` (JSON text), `minmax` index on `stored_at` | `_meter.events` (§7.5): `ReplacingMergeTree`, `PARTITION BY toYYYYMM(time)`, `ORDER BY (subject, type, source, id)`, `data String` | The same CloudEvents layout on a lake table. OpenMeter deduplicates **before** insert (Redis; Valkey in the harness); §63 deduplicates in the table with `FINAL` (D1122) |
| Ingest: `POST /api/v1/events` → Kafka → sink worker → batched `INSERT` (optionally `async_insert`) | `_system/_meter` stream → House pipe (`LoamsStream`) exactly once (D769) | The House takes OpenMeter's batched native inserts; the pipe path is not exercised (OpenMeter keeps its Kafka) |
| Meter queries at read time: `tumbleStart`/`tumbleEnd` over minute, hour, day and month windows with a time zone; `sum`, `count`, `uniqExact`, `avg`, `min`, `max`, `argMax` (LATEST) over `JSON_VALUE(data, path)` with `toFloat64OrNull` or `toDecimal128OrNull`, `ifNotFinite`, `nullIf`; group-by on `JSON_VALUE`; optional `PREWHERE` | §8.5: closed windows from `_meter.windows FINAL` plus the open edge aggregated from raw events "directly"; §8.1's aggregations are OpenMeter's | OpenMeter's meter queries **are** the shape of §8.5's open-edge query and of §8.2's close job. Its answers on the House against the oracle check those queries before ME1b writes them |
| Entitlements, grants, plans and billing in Postgres (`ent`); the balance worker reads meter queries | §9 entitlements in the metastore; §12 reconciliation reads House totals | Only the metering path touches the House. Entitlement semantics stay with ME1c's OpenMeter API conformance test (§17.2), not this gate |
| REST `GET /api/v1/meters/{slug}/query` | §17.2's OpenMeter-compatible subset | Once ME1b exists: send the same events to OpenMeter-on-House and to Loams Meter and compare the two `query` answers. That is the dogfood check |

**Scope.** Server, sink worker, balance worker and jobs; the notification service and billing worker start but are not driven; Svix (webhooks) off. Driver: create meters of every aggregation; ingest CloudEvents (`application/cloudevents-batch+json`) including duplicates and late events; query every window size, two time zones, group-by and subject filters; create an entitlement with grants so the balance worker runs its meter queries. OpenMeter's own Go `e2e/` suite needs its source (R0.7: the driver is the default).

**Harness needs.** Kafka (their compose uses `confluentinc/cp-kafka:8.0.3`; use an Apache Kafka image if OpenMeter accepts it, verify), Postgres, Valkey for the Redis dedupe driver, the native port on the House, oracle ClickHouse 26.9.2.8 (OpenMeter pins 25.12.3).

**Expected House gaps** (from the source, to be measured): the native protocol (D1548, D1571); the `minmax` skip index in `CREATE TABLE` (skip indexes are stripped in R9.3's discovery runs); `PREWHERE` on the Rust engine (R10.5; chDB is fine); `tumbleStart`/`tumbleEnd`, `JSON_VALUE`, `toDecimal128OrNull` and `ifNotFinite` in the Tier 0 profile (chDB has them); `async_insert` settings when enabled (R4.5); deletes or mutations, if OpenMeter issues any (not seen; verify).

Tests: `openmeter_on_house_smoke` (nightly). Commit: `conformance: OpenMeter on the House (HE0 Task 13)`.

### Task 14 — Queued gate: Laminar (LLM and agent observability)

> **Laminar replaces Helicone** (owner, 2026-10-22; [R14.2](he1-rulings/t14.md)), with **Opik** as the next candidate. Helicone was queued first (R14.1) and dropped: its public images and releases stop at 2025-08-21 and its development moved to a private repository. Third in the order after SigNoz.

**Application.** `lmnr-ai/lmnr`, Apache-2.0 (`LICENSE.md`), no split in the repository; `frontend-ee` and `app-server-ee` images exist on GHCR from code that is not in the repository (verify), and are never used. Releases `v0.3.2` (2026-10-16), `v0.3.1`, `v0.3.0`; pin one at least 14 days old, `ghcr.io/lmnr-ai/app-server` and `ghcr.io/lmnr-ai/frontend` by digest (their compose uses unpinned tags).

**Protocol: HTTP only.** The Rust app-server uses the `clickhouse` crate 0.15 (HTTP, with `RowBinary` for inserts and typed reads); the Next.js frontend uses `@clickhouse/client` and runs the migrations (`frontend/lib/clickhouse/migrations/*.sql`, over HTTP; verify). Both use `CLICKHOUSE_URL=http://clickhouse:8123`. So this gate runs on today's House and does not wait for the native listener.

**Scope.** app-server and frontend; ingest spans through Laminar's OpenTelemetry endpoint and its SDK's HTTP API (traces, spans, evaluations, events), then open the traces, spans, sessions, evaluations and dashboard pages and the SQL editor (a read-only ClickHouse user, `CLICKHOUSE_RO_USER`).

**Harness needs.** Postgres, Quickwit (full-text search; Apache-2.0 at v0.9.1, verify), and for the full file RabbitMQ and Valkey. Oracle 26.9.2.8 (Laminar pins 26.5).

**Expected House gaps** (from its migrations, to be measured): the `RowBinary` input and output formats over HTTP (chDB has them; the House's HTTP front must pass them through); `ReplacingMergeTree` read with `FINAL` (supported); `tokenbf_v1` skip indexes (stripped in R9.3's discovery runs); a second, read-only user with its own grants (the House classifies `GRANT`/`REVOKE` as unsupported today).

**Measured on the chDB House (2026-10-25, [R14.3](he1-rulings/t14.md)).** Laminar v0.3.0 (`app-server` and `frontend` by digest, unmodified) against a single-node House (`loams-fabric house --single-node`, a dev build), the House reached over its TLS listener through a TLS-originating proxy. The House accepts the connection and the first three migrations (`1_squashed`, `2_labels-to-tags`, `3_eval-scores-span-tags`); the frontend's runner (`clickhouse-migrations`) stops at `4_views.sql` and the frontend exits, so the UI never serves. Applying all 72 migration files statement by statement leaves 58 failures. So neither ingest (no `spans` table) nor the trace pages (no `traces_v0`) can work. The Postgres side is fine: Laminar's Postgres migrations apply on Postgres 17.6.

**Laminar on the House: the work list after 0.1.2** (owner, 2026-10-25, [R14.4](he1-rulings/t14.md), D1815). The 0.1.2 Software Factory keeps Langfuse; Laminar replaces it once these pass, with `laminar_on_house_smoke` (migrations, an SDK trace ingested, the traces, spans and sessions pages loading) as the gate:

1. **Parameterised views read as table functions.** `CREATE VIEW … AS … FROM raw_traces_v0(project_id = {…}, …)` and `map_aggregate_evaluation_scores_v0(…)`: a view with `{name:Type}` parameters, called like a table function, is refused as an outside table function (344). Blocks `traces_v0`, `evaluation_datapoints_v0` and their successors (migrations 4, 8, 10, 13, 15, 26, 36, 49): the House should resolve a call to a view in the namespace as that view.
2. **`PROJECTION` in `CREATE TABLE`** (and `ALTER TABLE … ADD|MATERIALIZE PROJECTION`) is refused (457). Blocks `spans_v2` and so `spans`, the ingest table, and every later `ALTER TABLE spans`. Accepting and ignoring projections on lake tables is the likely ruling.
3. **`SimpleAggregateFunction(f, T)` columns** are refused (457). Blocks `traces_agg` and `traces_static` (migration 54) and the trace list built on them.
4. **`Array(BFloat16)`** has no lake-table type (48). Blocks `signal_event_clusters` and its 768-dimension successor (migrations 25, 28, 48, 57), with their `vector_similarity('hnsw', …)` index.
5. **Dictionaries.** `CREATE DICTIONARY`, `CREATE OR REPLACE DICTIONARY` and `DROP DICTIONARY` are disabled (344, 48), with `dictGet` in the views. Laminar creates them in its migrations (43) and again at every frontend start, with `SOURCE(CLICKHOUSE(USER … PASSWORD … DB … TABLE|QUERY …))` over the same server, `COMPLEX_KEY_CACHE`, `COMPLEX_KEY_DIRECT` and `COMPLEX_KEY_HASHED` layouts.
6. **`INSERT … SELECT`** cannot be parsed ("outside the House's ClickHouse surface", 62). Blocks the data-copy migrations (19 and 32 fail on it; later copies fail first on their missing tables).
7. **`EXCHANGE TABLES`** (344, the statement's tree is not read) and **SQL UDFs** (`CREATE FUNCTION … AS (…) -> …`, 344). Block the table swaps (35, 48) and `apply_pii_masks` (66).

Not reached yet, to measure once 1–7 pass: `RowBinary` inserts and reads over HTTP from the Rust app-server, `async_insert` with `wait_for_async_insert`, `ReplacingMergeTree … FINAL`, `CoalescingMergeTree` and `AggregatingMergeTree`, `tokenbf_v1`, `bloom_filter` and `minmax` skip indexes, `JSON` and `Map` columns, `TTL`, `ADD CONSTRAINT … CHECK`, and the read-only user (Laminar's own compose uses the same user for both, so no `GRANT` is needed). Connecting: the app-server's `clickhouse` crate (0.15, `rustls-tls`) trusts only the Web PKI roots, so it cannot verify the House's self-signed certificate; a TLS-originating proxy (Caddy, Apache-2.0) that trusts `house.crt` worked, and is the Factory overlay's likely shape.

**Next candidate: Opik** (`comet-ml/opik`, Apache-2.0, releases daily): HTTP only, but a clustered schema (`ON CLUSTER`, `ReplicatedReplacingMergeTree`) that needs D1571's cluster-DDL forms, `Map`, `JSON` and `Time` types, Liquibase migrations over JDBC, and up to 14 services with ZooKeeper. Taken up only if the owner adds it after Laminar.

Tests: `laminar_on_house_smoke` (nightly). Commit: `conformance: Laminar on the House (HE0 Task 14)`.

### Task 15 — Queued gate: BI tools (Grafana and Metabase; Superset and Redash optional)

**Why.** Every House user's dashboards and BI tools browse the schema through `system.*` and send generated SQL. These tools need no application stack, so the gate is cheap.

**Tools** (all unmodified containers, D1566):
- **Grafana** (AGPL-3.0) with **`grafana/clickhouse-datasource`** (Apache-2.0, v4.23.1 of 2026-10-21): `clickhouse-go` v2, **native by default, HTTP by its `protocol` setting**. Run it **twice, once per protocol**.
- **Metabase** `metabase/metabase` (the AGPL image; never `metabase-enterprise`), whose ClickHouse driver is in core since Metabase 54 (`modules/drivers/clickhouse`, `clickhouse-jdbc` 0.9.8): **HTTP**.
- Optional: **Superset** (Apache-2.0) with `clickhouse-connect` (HTTP); **Redash** (BSD-2-Clause, HTTP).

**Scope.**
1. Connect: each tool's "test connection" (`SELECT version()`, Metabase's `SELECT currentDatabase()` and `system.settings` `cloud_mode` read).
2. Browse the schema: `SHOW DATABASES`, `SHOW TABLES [FROM db]`, `DESC TABLE`, `SHOW CREATE TABLE`, `SHOW COLUMNS`, `EXISTS DATABASE`/`EXISTS TABLE`, `system.databases`, `system.tables` (`engine`, `engine_full`, `comment`; views and materialized views by `engine`), `system.columns` (`name`, `type`, `table` per database), `system.functions` (Grafana's and Superset's editor completion; Superset also `system.table_functions`), Metabase's JDBC `DatabaseMetaData` (`getTables`, `getColumns`, which `clickhouse-jdbc` answers from `system.*`; verify the exact queries).
3. Standard queries: Grafana's query builder (time series with `$__timeFilter` and `$__timeInterval` macros, tables, logs and traces modes, ad hoc filters, template variables, `Map` key discovery with `mapKeys`, JSON path discovery with `JSONAllPaths` and `distinctJSONPathsAndTypes`); Metabase's question builder (filters, breakouts by date unit, aggregations, joins, native questions) over the sample data loaded into lake tables.
4. Dashboards: Grafana's bundled dashboards (`data-analysis`, `query-analysis`, `cluster-analysis`, OTel logs, traces and service); one Metabase dashboard built by the driver.

**Harness needs.** No application stack: a seeded House namespace (lake tables of several engines and types, a view, a materialized view, the OTel schema for Grafana's OTel dashboards), the oracle with the same data, and a Playwright or API driver per tool (Grafana's `/api/ds/query`, Metabase's `/api/dataset`). Mining (Tasks 2–5) records each tool's statements; answers compared against the oracle as R9.3 does.

**Expected House gaps.**
- `system` tables outside `ALLOWED_SYSTEM_TABLES` (`deny.rs`): `system.clusters` (Metabase's single-node check, which it catches; Grafana's cluster dashboard), `system.merges`, `system.mutations`, `system.replicas`, `system.detached_parts`, `system.disks`. Grafana's `cluster-analysis` and `data-analysis` dashboards fail on these unless the House answers them (empty for a one-node House is the likely ruling).
- `system.query_log` is served from `_house.query_log`: Grafana's `query-analysis` dashboard reads columns that may be blank or missing there (verify).
- `system.functions` lists chDB's functions, including ones the House disables (`344`), so editors offer functions that then fail.
- `system.tables.engine` shows lake engines as the House names them; `engine_full` and `comment` may be default values (`system.rs` fills what it knows).
- The `JSON` type, `JSONAllPaths` and `distinctJSONPathsAndTypes` on lake tables (verify the House's JSON support).
- `SHOW CREATE TABLE` for lake tables (Metabase parses it for skip indexes); the House's answer must parse.
- Native protocol for Grafana's default mode (D1571's listener).

Tests: `bi_grafana_on_house_smoke` (both protocols) and `bi_metabase_on_house_smoke` (nightly). Commit: `conformance: Grafana and Metabase against the House (HE0 Task 15)`.

## HE1 and HE2 — shared rules

> **Specified 2026-10-23** by the HE1 lead after the [HE0 exit report](he0-exit-report.md), at the owner's word ("same for rust clickhouse engine bro, spawn one agent and plan fully", 2026-10-23). New decisions **D1576–D1579 and D1800–D1805** and questions **Q1188, Q1189 and Q1320–Q1322** are in the [decision log](../design/13-decision-log.md) as **Proposed**. Work goes ahead on their defaults until the owner rules. HE1 is Tasks 20–47 and HE2 is Tasks 60–77. The numbers leave room after HE0's Tasks 0–15, so the rulings files (`he1-rulings/t<N>.md`) never collide.

**Goal (HE1):** a read-only Rust engine behind today's House front. It reads the lake tables that the chDB House writes and answers SELECT the way the 26.9.2.8 oracle does. The order is chosen so that the gate applications' reads move to it as early as possible: OpenPanel first, then Langfuse, then SigNoz. Writes, DDL, mutations and materialized views stay on the chDB path in HE1 (D1577).

**Goal (HE2):** the Rust engine also writes (inserts, merges, mutations on deletion vectors, materialized views, compaction, Iceberg v3). All three gate applications then pass on it, and it becomes the House's default. chDB becomes the oracle, plus an opt-in per-namespace fallback until HE3 (D1801).

### Architecture (HE1 and HE2)

```
 client ─ HTTP 8123/8443, native 9000/9440 ─ House front (loams-house, unchanged policy path)
                                                │ engine = chdb | rust | prefer-rust   (D1577)
                       ┌────────────────────────┴───────────────────────┐
             chDB worker (loams-house-worker,                 Rust worker (loams-house-rworker,
             libchdb; writes in HE1, oracle later)            no libchdb; reads in HE1, all in HE2)
                                                                 │ hsw1 v2: Execute{tables: [TablePin]}, Resolve (D1578)
                                                     loams-house-engine  (session, catalog view, execute → bytes)
                                                     ├── loams-house-sql      parse → guard → rewrite → SqlToRel → CH analyzer rules
                                                     ├── loams-house-exec     UDF/UDAF/UDWF, combinators, FINAL / Totals / LimitBy / ArrayJoin / Fill operators
                                                     ├── loams-house-iceberg  TableProvider over pinned snapshots, House encodings, pruning, deletes
                                                     ├── loams-house-formats  ClickHouse output formats (HE1) and input formats (HE2)
                                                     └── loams-house-types    ClickHouse types ↔ Arrow ↔ Iceberg, defaults, names, value text
```

- **Crates** are in `fabric/crates/` (D1550). The engine crates never depend on `loams-house` (the front) or on libchdb. The front never depends on DataFusion: it talks to the Rust worker over `loams-house-ipc` only. The test `engine_crates_stay_apart` (`fabric/crates/loams-house-engine/tests/deps.rs`, reading `cargo metadata`) checks both rules.
- **Versions (D1576, Q1188):** DataFusion **55.2.0**, Arrow **59** (the workspace's), sqlparser **0.62** as DataFusion's re-export `datafusion::sql::sqlparser` (R10.3), and `iceberg` from **`ostrium-labs/iceberg-rust` pinned by commit** on `loams/row-delta`, which sits on Arrow 59.2 (R11.8, R11.9). In HE1, `loams-house-lake` keeps iceberg 0.10.1 on Arrow 58 for the chDB write path. HE2 Task 60 folds it into `loams-house-iceberg`.
- **Errors:** the engine answers ClickHouse codes and names (`loams_house_types::ChError { code, name, message }`). It converts them to `loams_house_ipc::EngineError` at the worker. Anything outside the profile is `48 NOT_IMPLEMENTED` naming the construct. The security list's functions are `344` (D1558).

### Global constraints (HE1 and HE2)

- **Worktrees and PRs.** Each task works in `~/Documents/Ostriumlabs/loams-wt/he1-t<N>` and opens one PR into `dev` (a task may split into stacked PRs named `T<N>a`, `T<N>b`). A PR merges on a green `CI required`. Label it `review:greptile` when it touches storage, security, the worker protocol or a new public API. Commit with `git commit -s`.
- **Light builds.** Build from `fabric/` with `cargo build -p <crate>` or `cargo test -p <crate>`, never `--workspace` locally. Never set `CARGO_TARGET_DIR` and never build in `/tmp`. Delete your own large test binaries when done. DataFusion is a heavy build, so test one crate at a time.
- **The oracle gates every task (D1804).** Each task adds its cases under `conformance/clickhouse/oracle/corpus/he1/<task>/` (HE2: `he2/<task>/`). Expected answers come from `clickhouse-server` 26.9.2.8 at run time, never hand-written. The per-PR job `house-engine-oracle` (Task 23) runs them against `house-rust`. A task merges with no `untriaged` entry and no `bug` entry that names it. A gap that belongs to a later task is a `bug` entry with `task = "<N>"`.
- **Licences (D1566, D1574).** ClickHouse's source (Apache-2.0) may be read and ported, with its notice kept in `NOTICE` and a comment naming the file it came from. No Databend `ee/`, no SigNoz `ee/` or `cmd/enterprise/`, no AGPL, BSL, FSL, SSPL or ELv2 code. Every new crate passes `cargo deny` in `fabric/`.
- **Upstream.** iceberg-rust changes go fork-first (R11.7–R11.10). sqlparser and DataFusion gaps are worked around in `loams-house-sql` until the owner answers Q1321. Nothing is posted upstream without the owner's consent.
- **Honest labels (D1568).** The Rust engine is "in development" until HE1's exit and "preview" until HE2's gate, and it is never the default before HE2. Neither the engine nor its dev server is ever exposed publicly.

### Review Focus (HE1 and HE2)

1. **A ClickHouse construct the planner silently ignores.** Expected: rewritten or `48`. Test: `no_ignored_field_reaches_sqltorel` (Task 22).
2. **A Rust answer that differs from the oracle's without a triage entry.** Expected: the oracle job fails. Test: `untriaged_mismatch_fails_the_run` (HE0) applied to `house-rust`.
3. **`prefer-rust` falling back after output has started, or on a wrong answer.** Expected: never. A fallback happens only on a planner refusal (`48`) before the first byte. Test: `fallback_only_before_the_first_byte` (Task 39).
4. **libchdb, or a credential, reaching the Rust worker.** Expected: never. Tests: `rworker_links_no_libchdb` (Task 39), `rworker_holds_no_credentials` (Task 40).
5. **A chDB-written table read differently by the Rust engine.** Expected: identical answers. Test: `chdb_written_tables_read_identically` (Task 26, extended in Task 70).

## HE1 — The read-only Rust engine (Tasks 20–47)

> **Progress (2026-10-25; resumed for an opt-in, read-only Rust-engine preview in 0.1.3).**
> - **Task 20** is merged (#1124).
> - **Task 21** is merged (#1156): `loams-house-types`.
> - **Task 22** (#1183) is the front end's core.
> - **Task 23** is the engine oracle: the dev server, seeded targets, the per-PR job "House engine: the oracle", and `scripts/house/engine-oracle.sh` (R23.1–R23.5).
> - **Next, in parallel, one implementer each:** Tasks 24, 25, 26, 28, 30, 31, 32, 33, 36, 37 and 38. Their briefs are in `~/Documents/Ostriumlabs/loams-wt/.he0-shared/he1-briefs/tNN.md`.
> - HE0's loose ends (R20.3) are still open.

**Exit (§75 §16.1, D1579):** the Tier 0 query set passes against the oracle; at least 50 % of the filtered stateless tests pass on `house-rust`; OpenPanel's, Langfuse v4's and v3's and SigNoz's replays (R9.3) give identical answers on `engine = rust` with no fallback; OpenPanel runs end to end on a House with `engine = rust`; the performance budget of D1803 holds.

**Order and parallelism.** Tasks 20–23 are the foundation and run in sequence (21 can start once 20 merges, 22 once 21's type parser exists). After Task 23, these are independent and can run in parallel, each with its own corpus: 24, 25, 26, 28, 30, 31, 32, 33, 36, 37 and 38. Task 27 needs 26. Task 29 needs 28. Tasks 34 and 35 need 24 and 25. Task 39 needs 26, 36 and 38. Tasks 41, 42 and 44 need 39, then take whatever gaps their replay shows. Task 43 can start after 26. Tasks 45 and 46 need 39.

| Task | Title | Effort (engineer-days, estimate) | Depends on |
|---|---|---|---|
| 20 | Rulings, versions and crate skeletons | 2–3 | — |
| 21 | The ClickHouse type system (`loams-house-types`) | 8–10 | 20 |
| 22 | The front end's core: parse, parameters, the silent-drop guard, names | 6–8 | 21 |
| 23 | The engine oracle: dev server, seeded fixtures, per-PR job | 4–5 | 22 |
| 24 | ClickHouse type rules: supertypes, arithmetic, comparisons, boolean contexts, NULLs | 10–12 | 23 |
| 25 | Name resolution: aliases, scalar `WITH`, tuple access, `COLUMNS`, asterisk settings | 5–7 | 23 |
| 26 | Lake-table reads: the Iceberg `TableProvider`, House encodings, pruning, deletes | 9–11 | 23 |
| 27 | Merge-family reads: `FINAL` for Replacing, Summing, VersionedCollapsing, Collapsing | 12–15 | 26 |
| 28 | Aggregates, combinators, parametric aggregates and `windowFunnel` | 12–14 | 23 |
| 29 | ClickHouse-exact sketches and states: `uniq*`, `quantile*`, `-State`/`-Merge`, `AggregateFunction` reads | 12–15 | 28 |
| 30 | Functions I: date and time | 8–10 | 23 |
| 31 | Functions II: strings, regular expressions, hashes, UUIDs, IPs | 6–8 | 23 |
| 32 | Functions III: arrays, maps, tuples, lambdas | 8–10 | 23 |
| 33 | Functions IV: conversions, conditionals, maths, JSON extraction, the rest of Tier 0 | 8–10 | 23 |
| 34 | Clauses I: `LIMIT BY`, `WITH FILL`, `WITH ROLLUP`/`CUBE`, `WITH TOTALS` | 16–20 | 24, 25 |
| 35 | Clauses II: `ARRAY JOIN`, join strictness and `join_use_nulls`, `IN` forms, subqueries, `SAMPLE`, `PREWHERE`, set operations, windows | 18–22 | 24, 25 |
| 36 | Output formats (`loams-house-formats`) | 10–12 | 23 |
| 37 | Settings on the Rust engine | 4–5 | 23 |
| 38 | Metadata: `system` tables, `SHOW`/`DESCRIBE`/`EXISTS`, safe table functions | 6–8 | 23 |
| 39 | The Rust worker, hsw1 v2 and the front's engine switch | 10–12 | 26, 36, 38 |
| 40 | Worker hardening: the sandbox and the in-process foyer cache | 6–8 | 39 |
| 41 | OpenPanel reads on the Rust engine | 4–6 | 39 (+ gaps) |
| 42 | Langfuse v4 and v3 reads on the Rust engine | 4–6 | 39 (+ gaps) |
| 43 | `JSON` columns on the Rust engine (interim text) | 6–8 | 26 |
| 44 | SigNoz reads on the Rust engine | 4–6 | 39, 43 |
| 45 | The stateless replay: HE0 Task 7 finished, the chDB baseline, the Rust pass rate | 4–5 | 39 |
| 46 | Performance: the budget and the benchmarks against chDB | 5–6 | 39 |
| 47 | HE1 exit | 2 | all |
| | **HE1 total** | **209–264** | |

**The longest chain to OpenPanel on Rust (estimate)** is 20 → 21 → 22 → 23 → 24 → 34 → 41: about 50–64 engineer-days, with Tasks 26, 27, 28, 29, 36, 38 and 39 running beside 24 and 34. That chain, not the total, sets how soon OpenPanel's reads move; more implementers shorten it only by splitting its tasks (Task 24's rule families, Task 34's four clauses). Langfuse (Task 42) follows OpenPanel by its own 4–6 days, and SigNoz (Task 44) needs Task 43 as well.

### Task 20 — Rulings, versions and crate skeletons

Check and record in `he1-rulings/t20.md`:
- **Versions (D1576).** Run `cargo tree -d -e normal -p loams-house-engine` and confirm one `arrow-array` 59.x and one `parquet` 59.x. Also confirm that the fork commit's own CI is green, and that DataFusion 55.2.0 links `sqlparser` 0.62.
- **The House at `dev`:** the hsw1 frames, the read views (`views.rs`), the merge views, and the encodings that `loams-house::types` writes (the conversions Task 26 must mirror).
- **HE0's loose ends.** Rebase `he0-t5-chlog`, re-mine and regenerate the profile and matrix (its own PR). Write a first diagnosis of the concurrent-load flake (`36 metadata file … does not exist`). Record the bucket-prefix bug S1 (R11.4) with a failing test in its own PR.

Then create `loams-house-types`, `loams-house-sql`, `loams-house-exec`, `loams-house-iceberg`, `loams-house-formats` and `loams-house-engine`. Each gets a `lib.rs` with crate docs, workspace lints and `cargo deny` passing. Add `datafusion = { version = "=55.2.0", default-features = false, features = [...] }` and the `iceberg` fork pin to `fabric/Cargo.toml`'s workspace dependencies, with comments that give the licence and the reason.

Tests: `engine_crates_stay_apart` (no `loams-house`, `loams-house-worker` or `loams-chdb-sys` in the engine crates' graph; no `datafusion` in `loams-house`'s graph).

Exit: the skeletons build with `cargo check -p loams-house-engine`; the rulings are recorded. Effort 2–3 days. Commit: `house: the Rust engine's crate skeletons and versions (HE1 Task 20)`.

### Task 21 — The ClickHouse type system (`loams-house-types`)

**Files:** `fabric/crates/loams-house-types/src/{lib.rs, parse.rs, arrow.rs, iceberg.rs, default.rs, name.rs, text.rs, error.rs}`.

**Interfaces:**
```rust
pub enum ChType { UInt8, UInt16, UInt32, UInt64, Int8, Int16, Int32, Int64, Float32, Float64, Bool,
    Decimal { precision: u8, scale: u8 }, String, FixedString(u32), Date, Date32,
    DateTime { tz: Option<String> }, DateTime64 { precision: u8, tz: Option<String> },
    Uuid, IPv4, IPv6, Enum8(Vec<(String, i8)>), Enum16(Vec<(String, i16)>),
    Nullable(Box<ChType>), LowCardinality(Box<ChType>), Array(Box<ChType>),
    Map(Box<ChType>, Box<ChType>), Tuple(Vec<(Option<String>, ChType)>), Json(JsonParams),
    AggregateFunction { function: AggFn, args: Vec<ChType> }, SimpleAggregateFunction { function: AggFn, arg: Box<ChType> },
    Nothing, Interval(IntervalKind) }
pub fn parse(text: &str) -> Result<ChType, ChError>;           // every spelling CH accepts: aliases, case rules
impl Display for ChType { /* canonical toTypeName() text */ }
pub fn to_arrow(name: &str, t: &ChType) -> Field;              // metadata "loams.ch.type" = canonical text
pub fn from_arrow(field: &Field) -> Result<ChType, ChError>;
pub fn default_value(t: &ChType) -> ScalarValue;               // join_use_nulls = 0, missing map keys, -OrDefault
pub fn storage(t: &ChType) -> Storage;                         // how the House stores it in Iceberg (mirrors loams-house::types)
pub mod text { pub fn write_value(t: &ChType, array: &dyn Array, row: usize, style: TextStyle, out: &mut Vec<u8>); }
```

**Mapping (engine Arrow layout):**

| ClickHouse | Arrow | Notes |
|---|---|---|
| `UInt8`…`UInt64`, `Int8`…`Int64`, `Float32`, `Float64` | the same widths | `Int8` is 8 bits (sqlparser reads Postgres's `int8`; R10.6's `TypePlanner` fix) |
| `UInt128`, `UInt256`, `Int128`, `Int256` | none | `48` (not mined) |
| `Bool` | `Boolean` | written as `true`/`false`; arithmetic treats it as `UInt8` (Task 24) |
| `Decimal(P, S)`, `Decimal32/64/128/256(S)` | `Decimal128(P, S)` up to P = 38, else `Decimal256` | the family's P is kept (`Decimal64(4)` is `Decimal(18, 4)`) |
| `String` | `Utf8` | invalid UTF-8 from external tables: a ruling (Binary view or `117`) |
| `FixedString(N)` | `FixedSizeBinary(N)` | |
| `Date`, `Date32` | `Date32` | `Date` keeps its 1970-01-01…2149-06-06 range on casts |
| `DateTime[(tz)]` | `Timestamp(Second, tz)` | no tz: the session's `session_timezone`, else the server's (UTC) |
| `DateTime64(p[, tz])` | `Timestamp(Second | Millisecond | Microsecond | Nanosecond, tz)` for p 0, 1–3, 4–6, 7–9 | the exact p is in the metadata; values are scaled ticks |
| `UUID` | `FixedSizeBinary(16)` | text as ClickHouse writes it; Iceberg `uuid` byte order (verify against chDB's Parquet) |
| `IPv4`, `IPv6` | `UInt32`, `FixedSizeBinary(16)` | |
| `Enum8`, `Enum16` | `Int8`, `Int16` | names in the metadata; compare with strings by name, order by value |
| `Nullable(T)` | T, nullable | |
| `LowCardinality(T)` | `Dictionary(Int32, T)` | dictionary pages kept end to end is HE3 work |
| `Array(T)`, `Map(K, V)`, `Tuple(…)` | `List`, `Map` (unsorted), `Struct` (unnamed elements are `1`, `2`, …) | |
| `JSON(…)` | `Utf8` | interim, as R9.63; subcolumns in Task 43; Variant in HE4 |
| `AggregateFunction(f, T…)` | `Binary` | ClickHouse's own state bytes (D1800, Task 29) |
| `SimpleAggregateFunction(f, T)` | T's layout | |
| `Nothing` | `Null` | |
| `Nested`, `Dynamic`, `Variant`, `Object`, `Time`, `Time64`, geo types | none | `48` naming the type |

**Value text** (shared by Task 36): `DateTime` as `YYYY-MM-DD hh:mm:ss` in the column's zone; `DateTime64` with p digits; floats with ClickHouse's shortest round-trip form (`inf`, `-inf`, `nan`); `Decimal` with its scale (check `output_format_decimal_trailing_zeros`); `Bool` as `true`/`false`; `UUID`, IP and `Enum` names; escaping per format family (TSV, CSV, JSON, Values).

**Tests:** `every_profile_type_parses_and_prints_as_clickhouse` (each Tier 0 type from `profile.toml`, with the canonical text compared to `toTypeName` on the oracle, recorded in `tests/fixtures/type_names.tsv` by `scripts/house/oracle-capture.sh`); `arrow_round_trip_keeps_the_clickhouse_type`; `default_values_match_the_oracle`; `value_text_matches_the_oracle` (a table of values per type, rendered in TSV, CSV and JSON on the oracle and here).

**Oracle:** `corpus/he1/21/` holds `SELECT toTypeName(…)`, `CAST` round trips and per-type literals.

**Exit:** every Tier 0 type maps both ways; text rendering matches the oracle in the three families. Effort 8–10 days. Commit: `house: loams-house-types, ClickHouse types for the Rust engine (HE1 Task 21)`.

### Task 22 — The front end's core (`loams-house-sql`)

**Files:** `fabric/crates/loams-house-sql/src/{lib.rs, lex.rs, parse.rs, params.rs, guard.rs, names.rs, context.rs, plan.rs}`.

**Steps:**
1. **Lexical pre-pass (`lex.rs`).** It handles the forms sqlparser 0.62 misreads or refuses (R2.3, spike §2.3) on tokens, recording each removal in a side table keyed by position:
   - `FROM t [AS a] FINAL`: the table is marked final, never aliased.
   - `ANY`, `ALL`, `SEMI`, `ANTI` and `ASOF` join strictness: removed and recorded per join.
   - `t.1`: becomes `tupleElement(t, 1)`.
   - `GLOBAL IN` and `GLOBAL JOIN`: `GLOBAL` is dropped (one bucket).
   - The `LIMIT n BY k LIMIT m` and `LIMIT m, n BY k` forms are normalised.
   - `ARRAY JOIN a AS x, b AS y` is regrouped, as the extractor does (R2.11).

   The same lexer as `loams-house-mine`'s `tokens.rs`, moved into `loams-house-sql`, which the miner then uses.
2. **Parameters (`params.rs`).** `{name:Type}` is bound as ClickHouse binds it (`_CAST('…', 'Type')`, R2.8), with the type parsed by Task 21. An unbound parameter is `456`.
3. **The silent-drop guard (`guard.rs`).** One exhaustive walk over sqlparser's AST destructures every struct **without `..`**, so a sqlparser upgrade that adds a field fails to compile. Every field that `SqlToRel` ignores (spike §3.1: `prewhere`, `GROUP BY` modifiers, function `parameters`, `sample`, table `version`, `settings`, `format_clause`, …) is either moved into the House's own structure for the later rewrite steps, or refused with `48` naming the construct.
4. **Names (`names.rs`).** Function lookup is case-sensitive, as ClickHouse's is. A name is case-insensitive only when `system.functions.case_insensitive` says so; the fixture `function_names.tsv` is captured from the oracle. Aliases (`DATE_DIFF` → `dateDiff`) come from the matrix. DataFusion's lower-casing is avoided by quoting every function identifier before `SqlToRel` sees it.
5. **Context and planning (`context.rs`, `plan.rs`).** A `HouseContextProvider` over a `Catalog` trait (`table(db, name) -> Option<Arc<dyn TableSource>>`). Functions are registered from `loams-house-exec`'s registry. A ClickHouse `TypePlanner` (Task 21's `parse`) and a `RelationPlanner` that turns the side table's marks into extension nodes (`Final`, filled in by Task 27). `SqlToRel` then produces the `LogicalPlan`. The House's analyzer rule list replaces DataFusion's `TypeCoercion` with a placeholder that Task 24 fills.
6. **`Resolution`.** The tables a statement reads (database, name, final or not) and the functions it calls, which the front needs for pinning and the deny list (Task 39).

**Interfaces:** `pub fn prepare(sql: &str, params: &Params, settings: &Settings, catalog: &dyn Catalog) -> Result<Prepared, ChError>`, where `Prepared { plan: LogicalPlan, resolution: Resolution, format: Option<String>, settings: Vec<(String, String)> }`.

**Tests:** `no_ignored_field_reaches_sqltorel` (compile-time exhaustiveness plus a runtime corpus of every spike §3.1 construct); `final_is_never_an_alias`; `function_names_keep_their_case`; `parameters_bind_as_clickhouse_binds_them`; and the spike's 20 queries, which must all produce a plan or a `48` naming what is missing, never a silent plan.

**Oracle:** `corpus/he1/22/`: `SELECT 1`, literals of every type, arithmetic over `system.one`, parameters. These are the cases Task 23's harness runs first.

**Exit:** the spike's silent drops are all refused or rewritten; the HS1 `select/` corpus plans. Effort 6–8 days. Commit: `house: loams-house-sql, the ClickHouse front end's core (HE1 Task 22)`.

### Task 23 — The engine oracle

**Files:** `fabric/crates/loams-house-engine/src/{lib.rs, catalog.rs, execute.rs, fixtures.rs}`, `src/bin/loams-house-engine-dev.rs`; `fabric/crates/loams-house-mine/src/oracle/seed.rs`; `conformance/clickhouse/oracle/targets.toml` (`house-rust`); `.github/workflows/house-engine-oracle.yml`.

**Design:**
- **`loams-house-engine-dev`** is a loopback-only HTTP server that speaks enough of ClickHouse's HTTP interface for the oracle runner: `POST /?query=…&database=…&param_…` plus settings, results in `JSONCompactEachRowWithNamesAndTypes` until Task 36 adds the rest, and errors as `Code: N. DB::Exception: …` with `X-ClickHouse-Exception-Code`. It is never built into a release image.
- **Seeded fixtures (`seed.rs`).** A target in `targets.toml` may say `seed_from = "clickhouse-server@26.9"`. After a corpus's `setup.sql` has run on the baseline, the runner snapshots every table the setup created: `system.columns` (names, types, defaults), `system.tables` (engine, sorting key, partition key) and the rows as `Parquet`. Each table goes into `<fixtures>/<db>/<table>/`. The dev server loads the snapshot as in-memory tables, with the ClickHouse types restored through Task 21 and the merge-family metadata kept for Task 27. A setup statement is not re-run on a seeded target, so the data is the oracle's by construction and a difference is the engine's.
- **The job.** `house-engine-oracle` runs per PR on paths `fabric/crates/loams-house-{types,sql,exec,iceberg,formats,engine}/**` and `conformance/clickhouse/oracle/**`. It pulls `clickhouse-server` 26.9.2.8 by digest, builds only `loams-house-engine-dev` and `loams-house-mine`, and runs `loams-house-mine oracle --target clickhouse-server@26.9 --target house-rust --corpus he1/*`. It needs no libchdb, and it joins `CI required` once green on `dev`.
- **Triage.** A `[[mismatch]]` entry takes an optional `task = "<N>"`. The runner's `--task <N>` check fails while any `bug` entry names that task. That is how a task's exit is checked (D1804).

**Tests:** `seeded_tables_read_back_as_the_oracle_wrote_them` (every Tier 0 type through a seed and a `SELECT *`); `a_seeded_target_never_runs_setup`; `task_bugs_block_their_task`.

**Exit:** the job runs on a PR in under 10 minutes; `corpus/he1/22/` passes. Effort 4–5 days. Commit: `house: the engine oracle and its per-PR job (HE1 Task 23)`.

### Task 24 — ClickHouse type rules

**Files:** `fabric/crates/loams-house-sql/src/coerce.rs` (an `AnalyzerRule` that replaces `TypeCoercion`) and `fabric/crates/loams-house-exec/src/ops/` (arithmetic and comparison kernels where Arrow's differ).

**Scope**, ported from ClickHouse's `getLeastSupertype`, `FunctionsArithmetic` and `FunctionsComparison` (attribution in NOTICE):
- **Supertypes.** `if`, `multiIf`, `CASE`, `coalesce`, arrays and `UNION` branches use the least supertype. Signed and unsigned widen as ClickHouse does (`UInt64` with `Int64` is `Int128` in ClickHouse, which is `48` here until a test needs it; ruling). `NULL` is `Nullable(Nothing)`.
- **Arithmetic.** `+`, `-` and `*` widen one step (`UInt8 + UInt8` is `UInt16`), up to 64 bits, and integers wrap on overflow. `/` is always `Float64`, `intDiv` is integer division, and `%` follows ClickHouse's sign rules. `Decimal` arithmetic follows ClickHouse's scale rules. Date and time arithmetic with integers and `INTERVAL` is shared with Task 30.
- **Comparisons.** A string literal compared with a typed column is converted to the column's type (`event_date = '2026-10-01'`). Comparisons work across signed and unsigned integers, `Enum` against string, and `Date` against `DateTime`. Tuples compare lexicographically.
- **Boolean contexts.** `WHERE`, `HAVING`, `if` and `and`/`or`/`not` take any number. `and`/`or` return `UInt8` (Nullable when an operand is), and comparisons return `UInt8`.
- **NULLs.** Functions propagate `Nullable` the way ClickHouse's default implementation does. `isNull`, `ifNull` and `coalesce` are exceptions per their definitions.

**Tests:** `arithmetic_result_types_match_the_oracle` (a generated matrix of types × operators, each checked with `toTypeName(a op b)` and the value on the oracle); `string_literals_convert_to_the_column_type`; `boolean_contexts_take_numbers`.

**Oracle:** `corpus/he1/24/`, about 400 generated cases (`scripts/house/gen-type-cases.py`, committed with the cases).

**Exit:** the matrix passes; the spike's q17 (`duration / 1000`) gives `Float64`. Effort 10–12 days. Commit: `house: ClickHouse type rules on the Rust engine (HE1 Task 24)`.

### Task 25 — Name resolution

**Files:** `fabric/crates/loams-house-sql/src/scope.rs`.

**Scope:**
- Aliases are visible in every clause: `WHERE`, `PREWHERE`, other select items, `GROUP BY`, `ORDER BY`, `LIMIT BY` and `JOIN … ON`.
- An alias shadows a column of the same name (`prefer_column_name_to_alias = 0`), and self-references (`f(x) AS x`) resolve as ClickHouse resolves them.
- Lambda parameters shadow columns.
- Scalar `WITH expr AS name` and `WITH (subquery) AS name` are substituted.
- `COLUMNS('regexp')`, `* EXCEPT (…)`, `* REPLACE (…)` and `t.*` are expanded.
- `asterisk_include_alias_columns` and `asterisk_include_materialized_columns` decide what `*` includes, which fixes R9.12's deviation on the Rust engine.
- `untuple` is supported, and duplicate output names are named as ClickHouse names them.

**Tests:** `aliases_resolve_as_clickhouse` (the analyzer's documented cases, each checked on the oracle); `alias_shadows_column`; `scalar_with_is_substituted`.

**Oracle:** `corpus/he1/25/`.

**Exit:** the cases pass; the spike's q17 plans and answers right. Effort 5–7 days. Commit: `house: ClickHouse name resolution on the Rust engine (HE1 Task 25)`.

### Task 26 — Lake-table reads (`loams-house-iceberg`)

**Files:** `fabric/crates/loams-house-iceberg/src/{lib.rs, pin.rs, provider.rs, encodings.rs, prune.rs, deletes.rs, virtual_columns.rs}`.

**Interfaces:**
```rust
pub struct TablePin { pub db: String, pub name: String, pub location: String, pub metadata_file: String,
    pub snapshot_id: Option<i64>, pub ch: ChTableInfo /* columns with ClickHouse types, defaults, ALIAS, TTL,
    engine and its parameters, sorting and partition keys */, pub mode: ReadMode /* Plain | Final | Sequenced */,
    pub external: bool }
pub fn provider(pin: &TablePin, store: Arc<dyn ObjectStore>) -> Result<Arc<dyn TableProvider>, ChError>;
```
`TablePin` is serialisable. The front sends it in hsw1 v2 (Task 39). Its fields mirror `loams-house`'s `ReadPin` plus what `views.rs` reads from the catalog.

**Scope:**
- **Snapshot reads.** The pinned metadata file and snapshot are read with the fork's `iceberg` crate (manifests and file planning). Parquet is read through DataFusion's Parquet source, with Iceberg field ids mapped to columns.
- **House encodings, the same conversions as `views.rs`.** ClickHouse types are restored from `loams.ch.types`, including `UInt*`, `Enum`, `LowCardinality`, `Bool` and `FixedString`. A column added by `ALTER` reads its `DEFAULT` for older files. `ALIAS` columns are computed. A delete `TTL` hides expired rows at read time (R9.12). `DateTime64(7..9)` comes from `long` nanoseconds (R9.61). `JSON` comes from its text (R9.63, as a string until Task 43). `AggregateFunction` columns are read as their state bytes (R9.14). An external table is read as Iceberg types, with no casts.
- **Virtual columns.** `__loams_seq` (the data file's sequence number) and `__loams_row` (the row's position in the file) back `Sequenced` mode and Task 27's tie-breaking. They are exactly what `views.rs` gives chDB.
- **Pruning.** Partition values, then file column statistics, then the Parquet page index, then bloom filters. Filters are pushed into the Parquet reader (`pushdown_filters`).
- **Deletes.** v2 position deletes and v3 deletion vectors are applied (the fork reads them, R11.1). `_row_id` and `_last_updated_sequence_number` are cast from run-end encoding (R11.5).
- **Time travel.** `loams_snapshot_id` and `loams_as_of` resolve in the front (`read.rs`) and arrive as pins.

**Tests:** `chdb_written_tables_read_identically`. It writes tables with the House's own writer (`loams-house-lake`, as the chDB House does) for every Tier 0 type and every encoding above, and compares the Rust read with chDB's read of the same files. It runs in the Fabric workflow's House job, which has libchdb. Further tests: `spark_v3_tables_read_as_spark` (the spike's fingerprints, with fixtures committed from `he0-iceberg-v3-spike`'s results); `pruning_skips_files` (counts files opened).

**Oracle:** `corpus/he1/26/` over a House-written fixture, through `loams-house-engine-dev --lake <dir>`.

**Exit:** both identity tests pass; pruning reads fewer files on partition and key filters. Effort 9–11 days. Commit: `house: lake-table reads on the Rust engine (HE1 Task 26)`.

### Task 27 — Merge-family reads: `FINAL`

**Files:** `fabric/crates/loams-house-sql/src/final_.rs` (the `Final` extension node); `fabric/crates/loams-house-exec/src/final_/{mod.rs, replacing.rs, summing.rs, collapsing.rs}` (the physical operator).

**Semantics** are the House's rules as built and measured on the oracle:
- **Replacing.** Keep the row with the greatest `(ver, __loams_seq, __loams_row)` per sorting key; drop it when `is_deleted = 1` (R0.2).
- **Summing.** Group by the key, sum the numeric non-key columns, keep the first row's other columns, and drop all-zero rows.
- **VersionedCollapsing.** The stack rule, by sign and version, in write order (R9.2).
- **Collapsing.** Implemented beside VersionedCollapsing, since it is cheap.
- **Partitions.** Merges happen within a partition only.
- **`final = 1`.** The setting applies `FINAL` to every merging table the statement reads (R9.16).

**Plan rules:**
- A predicate is pushed below the node only when it reads sorting-key columns alone (`prevent_predicate_push_down_columns`).
- `PREWHERE` is applied before deduplication.
- The node projects the key, version, sign and `is_deleted` columns plus the query's own.

The first operator sorts and merges per partition, by key, over the files' runs. A hash variant for unsorted files is a ruling.

**Tests:** `replacing_final_matches_the_oracle`, `collapsing_final_matches_the_oracle` (R9.2's measured rows), `collapsing_sessions_keep_their_latest_state` (OpenPanel's pattern), `final_pushdown_only_on_key_columns`.

**Oracle:** `corpus/he1/27/`, over seeded tables of each engine with interleaved inserts.

**Exit:** OpenPanel's `sessions` and `profiles` reads, Langfuse v3's `traces`, `observations` and `scores` reads with `FINAL`, and v4's `scores` reads all match. Effort 12–15 days. Commit: `house: FINAL on the Rust engine (HE1 Task 27)`.

### Task 28 — Aggregates, combinators, parametric aggregates and `windowFunnel`

**Files:** `fabric/crates/loams-house-exec/src/agg/{mod.rs, registry.rs, combinators.rs, core.rs, arrays.rs, maps.rs, topk.rs, funnel.rs}`; `fabric/crates/loams-house-sql/src/aggregates.rs` (name parsing and parameters).

**Scope:**
- **Core aggregates.** `count` (with `count(DISTINCT)` following `count_distinct_implementation`), `sum` (with ClickHouse's result types: `sum(UInt8)` is `UInt64`), `avg`, `min`, `max`, `any`, `anyLast`, `argMax`, `argMin`, `groupArray[(n)]`, `groupUniqArray`, `sumMap`, `maxMap`, `topK(n)` and `approx_top_k`. These are the Tier 0 aggregates from `profile.toml`.
- **The empty-input rule.** Empty input gives ClickHouse's answers (`sum` 0, `avg` `nan`) unless `aggregate_functions_null_for_empty` says otherwise.
- **Combinators.** A name parser handles compositions such as `sumIf`, `groupUniqArrayArrayIf` and `uniqMergeState`, and maps them onto `-If` (DataFusion `FILTER`), `-Distinct`, `-Array`, `-ForEach`, `-OrNull`, `-OrDefault` and `-SimpleState` wrappers.
- **Parametric aggregates.** Parameters such as `quantile(0.95)`, `topK(10)` and `windowFunnel(3600)` become validated leading literal arguments.
- **`windowFunnel(window[, mode…])(timestamp, cond1, …)`.** Its modes are `strict_deduplication`, `strict_order`, `strict_increase` and `strict_once` (plus `allow_reentry`). It is ported from ClickHouse's `AggregateFunctionWindowFunnel.h`, with attribution. OpenPanel's funnel and conversion pages use it.

**Tests:** `aggregate_result_types_match_the_oracle`; `combinator_names_parse_as_clickhouse`; `window_funnel_matches_the_oracle` (generated event streams in every mode).

**Oracle:** `corpus/he1/28/`.

**Exit:** every Tier 0 aggregate and combinator except the sketches and states (Task 29) matches. Effort 12–14 days. Commit: `house: aggregates, combinators and windowFunnel on the Rust engine (HE1 Task 28)`.

### Task 29 — ClickHouse-exact sketches and states

**Files:** `fabric/crates/loams-house-exec/src/agg/{uniq.rs, quantile.rs, state/{mod.rs, codec.rs, count.rs, sum.rs, minmax.rs, argminmax.rs, uniq.rs, quantiles.rs, group_array.rs}}`.

**Scope:**
- **`uniq`** is ClickHouse's `UniquesHashSet`: adaptive sampling over the same hash functions. ClickHouse picks a hash per argument type (`intHash64` for numbers, CityHash64 v1.0.2 for strings, verify per type in ClickHouse's `UniqVariadicHash`), and the engine must use the same ones. `uniqExact` and `countDistinct`. `uniqCombined` only if mined.
- **`quantile*`.** `quantile` and `quantiles` (`ReservoirSampler`, 8 192 samples, ClickHouse's interpolation; results at or under 8 192 rows are exact, above that the oracle compares with a tolerance), `quantileExact`, `quantilesExactInclusive`, `quantilesDD` (DDSketch) and `quantileTDigest` (only if mined).
- **States (D1800).** `-State` produces, and `-Merge` and `-MergeState` read, **ClickHouse's own serialisation** for `count`, `sum`, `min`, `max`, `any`, `argMax`, `argMin`, `uniq`, `quantiles` and `groupArray`, the functions the gate applications store (OpenPanel `countState`, `sumState`, `minState`, `maxState`, `uniqState`; Langfuse `argMaxState`; SigNoz `quantilesState`; Umami `argMin`/`argMax`/`groupArrayState`). Any other state function is `48`. `AggregateFunction` columns written by chDB (R9.14) are therefore read and merged as they are, and HE2's writes stay readable by the chDB oracle.
- **`finalizeAggregation`**, and reading `AggregateFunction` columns with plain aggregates, as ClickHouse allows for `SimpleAggregateFunction`.

**Tests:** `states_are_byte_identical_to_the_oracle` (`hex(fState(x))` on the oracle against the engine, per function and argument type); `chdb_written_states_merge` (an `AggregatingMergeTree` table written by the chDB House, read with `-Merge`); `uniq_matches_the_oracle_above_the_sampling_threshold` (1 M distinct values).

**Oracle:** `corpus/he1/29/`.

**Exit:** OpenPanel's six inner-table views read and match. Effort 12–15 days. Commit: `house: ClickHouse-exact uniq, quantiles and aggregate states (HE1 Task 29)`.

### Tasks 30–33 — The scalar function library

Each task takes one family of the Tier 0 functions in `profile.toml`, in mined frequency order. Each function is a DataFusion `ScalarUDF` with ClickHouse's return-type rule and its own oracle cases. A function's first case comes from its mined use (`samples` in the inventories, scrubbed), and its next cases from edge values (NULL, empty, out of range, zones). Files: `fabric/crates/loams-house-exec/src/func/<family>/`. Functions that ClickHouse documents as case-insensitive register their aliases from the matrix. Tests per task: `<family>_functions_match_the_oracle`, plus `every_tier0_<family>_function_is_registered` (against `profile.toml`).

- **Task 30, date and time (8–10 days).**
  - The conversion and truncation functions: `toDate`, `toDateTime`, `toDateTime64`, `toStartOf{Minute,Hour,Day,Interval,…}`, `toYYYYMM`, `toYYYYMMDD` and `toDayOfMonth`.
  - Intervals: `toInterval{Second,…,Year}`, `INTERVAL` literals, date arithmetic and `subtractHours`.
  - `dateDiff` (ClickHouse's boundary-counting rule), `now`, `now64`, `toUnixTimestamp`, `toUnixTimestamp64Milli`/`Nano` and `fromUnixTimestamp64Nano`/`Milli`.
  - Tier 1 when cheap: `formatDateTime`, `parseDateTimeBestEffort` and `toTimeZone`.
  - Time zones come from `session_timezone`, then the column's zone, then the server's, through `chrono-tz`. A ruling records which time-zone database version it carries against the oracle's.
- **Task 31, strings, regular expressions, hashes, UUIDs and IPs (6–8 days).**
  - Strings: `lower`/`upper` and their `UTF8` forms, `concat`, `length`/`lengthUTF8`, `leftUTF8`, `startsWith`/`endsWith`, `position*`, `replace*`, `like`/`notLike`/`ilike`, `match` and `multiSearchAny`.
  - Hashes: `cityHash64` (CityHash v1.0.2 64-bit; the `cityhash-rs` pin is 128-bit only, so this needs a port or a crate, ruling), `xxHash32`/`xxHash64`, `sipHash64` and `intHash64`.
  - `generateUUIDv4`, `toUUID` and the IP functions if mined.
  - Regular expressions use RE2 syntax in ClickHouse, while Rust's `regex` differs on lookarounds and backreferences, which RE2 lacks too. A ruling lists the differences, and any pattern Rust reads differently is `36`.
- **Task 32, arrays, maps, tuples and lambdas (8–10 days).**
  - Arrays: `array`, `arrayElement` (1-based, negative indexes, and the default value when out of range), `has`/`hasAll`/`hasAny`, `indexOf`, `length`/`empty`/`notEmpty`, `arraySlice`, `arrayConcat`, `arrayDistinct`, `arrayEnumerate`/`arrayEnumerateUniq`, `arrayReverse`, `arraySort`, `arrayStringConcat`, `arrayZip`, `range`, `arraySum` and `arrayJoin` as a function (Task 35 hoists it).
  - Maps: `map`, `mapKeys`, `mapValues`, `mapContains`, `mapFilter`, `mapFromArrays`, and `m['k']`, which gives the default value for a missing key.
  - Tuples: `tuple`, `tupleElement` and `untuple`.
  - Higher-order functions over one or more arrays, as our own `HigherOrderUDF`s: `arrayMap`, `arrayFilter`, `arrayExists`, `arrayAll`, `arrayFirst`, `arrayFirstIndex`, `arrayCount`, `arraySum(λ, …)` and `arraySort(λ, …)`.
- **Task 33, conversions, conditionals, maths, JSON extraction and the rest (8–10 days).**
  - Conversions: `toX`, `toXOrNull`, `toXOrZero`, `toXOrDefault`, `CAST(x AS T)`, `CAST(x, 'T')`, `::` and `accurateCast` if mined.
  - Conditionals: `if`, `multiIf`, `ifNull`, `nullIf`, `coalesce`, `greatest`/`least`, `isNull`/`isNotNull` and `isNaN`.
  - Maths: `round`, `floor`, `intDiv`, `bitAnd`, `negate`, `rand` (compared by type and range only) and `ifNotFinite`.
  - JSON: `JSONExtractString` and the `JSONExtract*` family, `simpleJSONExtractString`, `JSON_VALUE` and `JSONAllPaths` (for `JSON` columns, Task 43).
  - The rest: `toTypeName`, `version()` (answers the House's configured version, as today), `currentDatabase`, `timezone`, `in`/`notIn` as functions (sets in Task 35) and `exists`.

Commits: `house: <family> functions on the Rust engine (HE1 Task 3N)`.

### Task 34 — Clauses I: `LIMIT BY`, `WITH FILL`, `WITH ROLLUP`/`CUBE`, `WITH TOTALS`

**Files:** `fabric/crates/loams-house-sql/src/clauses/{limit_by.rs, fill.rs, rollup.rs, totals.rs}`; `fabric/crates/loams-house-exec/src/ops/{limit_by.rs, fill.rs, totals.rs}`.

**Scope:**
- **`LIMIT n [OFFSET m] BY keys`.** A `row_number()` rewrite first (spike q05). A per-group top-N operator comes in HE3.
- **`ORDER BY … WITH FILL [FROM] [TO] [STEP]` and `INTERPOLATE`.** A gap-filling sort node with ClickHouse's rules for numbers, dates and intervals. OpenPanel uses it 38 times.
- **`WITH ROLLUP` and `WITH CUBE`.** Planned as `GROUPING SETS`. The rolled-up keys hold the type's **default value**, not NULL, unless `group_by_use_nulls = 1`.
- **`WITH TOTALS`.** An extension node with two outputs: the rows, and one totals row computed before `ORDER BY` and `LIMIT`. It honours `totals_mode` and `totals_auto_threshold` together with `HAVING`. The totals reach the format writer as a separate block (Task 36: `"totals"` in `JSON*`, the blank-line block in TSV, the `Totals` packet over native).

**Tests:** `limit_by_matches_the_oracle`, `with_fill_matches_the_oracle`, `rollup_keys_are_defaults`, `totals_reach_every_format`.

**Oracle:** `corpus/he1/34/`.

**Exit:** OpenPanel's `WITH FILL` and `WITH ROLLUP` reads and Langfuse's `LIMIT BY` reads match. Effort 16–20 days. Commit: `house: LIMIT BY, WITH FILL, ROLLUP and TOTALS on the Rust engine (HE1 Task 34)`.

### Task 35 — Clauses II

**Files:** `fabric/crates/loams-house-sql/src/clauses/{array_join.rs, joins.rs, in_.rs, subquery.rs, sample.rs, prewhere.rs, setops.rs, window.rs}`; operators in `loams-house-exec/src/ops/`.

**Scope:**
- **`ARRAY JOIN` and `LEFT ARRAY JOIN`.** Several arrays zip and must have equal sizes (`190`). `LEFT` keeps rows with empty arrays and gives each element its type's default. `arrayJoin()` in the projection is hoisted.
- **Join strictness from the pre-pass.** `ANY` joins deduplicate the right side and `SEMI`/`ANTI` map to DataFusion's. `join_use_nulls = 0` fills the non-matching side with default values. `ASOF` is `48` unless mined. A `JOIN … USING` follows ClickHouse's column rules.
- **`IN`.** `IN` with a set literal, a tuple, a subquery, or `(a, b) IN (subquery)` (a semi join). `transform_null_in = 0`.
- **Subqueries.** A scalar subquery is `125` when it returns more than one row. `EXISTS` is supported.
- **`SAMPLE`.** It reads every row, with `_sample_factor = 1`, as R9.15 does on chDB.
- **`PREWHERE`.** Folded into `WHERE`, its conjuncts placed first. It is applied before `FINAL` when both are present.
- **Set operations.** `UNION ALL`/`DISTINCT` (with `union_default_mode`), `INTERSECT` and `EXCEPT`.
- **Window functions.** ClickHouse's names (`lagInFrame`/`leadInFrame` with their frame rules, `row_number`) and the `WINDOW` clause.

**Tests:** per construct, `<construct>_matches_the_oracle`.

**Oracle:** `corpus/he1/35/`.

**Exit:** the spike's q07–q09 match, and OpenPanel's `ANY JOIN` and Langfuse's `ARRAY JOIN` reads match. Effort 18–22 days. Commit: `house: ARRAY JOIN, joins, IN and subqueries on the Rust engine (HE1 Task 35)`.

### Task 36 — Output formats (`loams-house-formats`)

**Files:** `fabric/crates/loams-house-formats/src/{lib.rs, writer.rs, json.rs, text.rs, native.rs, binary.rs, parquet.rs, arrow.rs, pretty.rs}`.

**Interface:** `pub fn writer(format: &str, schema: &ChSchema, settings: &FormatSettings, out: impl Write) -> Result<Box<dyn FormatWriter>, ChError>`, where `FormatWriter` has `write_batch`, `write_totals`, `write_extremes` and `finish(stats)`.

**Scope:**
- **Formats.** `JSONEachRow`, `JSON` (`meta`, `data`, `totals`, `rows`, `rows_before_limit_at_least`, `statistics`), `JSONCompact`, `JSONCompactEachRow[WithNames[AndTypes]]`, `JSONStrings*`, `TabSeparated[WithNames[AndTypes]]` (and `TSV*`), `CSV[WithNames]`, `Values`, `RowBinary[WithNames[AndTypes]]`, `Native` (blocks through `loams-house-native`'s codec, with column headers and `LowCardinality` as the client's revision allows, R9.41), `Parquet`, `ArrowStream`/`Arrow` and `Pretty`/`PrettyCompact`.
- **Settings.** `output_format_json_quote_64bit_integers`, `output_format_json_quote_denormals`, `output_format_decimal_trailing_zeros`, `date_time_output_format` and `output_format_native_write_json_as_string`.
- **Streaming.** Each writer is streaming, so a result's first bytes leave before its last batch is computed.

**Tests:** `formats_match_the_oracle_byte_for_byte` for the text formats (same query, same data, compared as bytes after removing `statistics`); `native_blocks_match_the_oracle` (decoded with `loams-house-native`); `parquet_reads_back_identically`.

**Oracle:** the runner gains `-- oracle: format = <F>` per case, compared as bytes.

**Exit:** the gate applications' formats (`JSONEachRow`, `JSON`, `Native`, `Values` input aside) match byte for byte. Effort 10–12 days. Commit: `house: loams-house-formats, ClickHouse output formats (HE1 Task 36)`.

### Task 37 — Settings on the Rust engine (D1805)

**Files:** `fabric/crates/loams-house-engine/src/settings.rs` and its table `settings.tsv`.

Every setting that the front accepts (`loams_house_ipc::policy::CLIENT_SETTINGS`, `SATISFIED_SETTINGS` and the chDB-defaults rule, R9.11) is classed:
- **honoured:** it changes the engine's behaviour as ClickHouse's does, for example `join_use_nulls`, `prefer_column_name_to_alias`, `final`, `session_timezone`, `count_distinct_implementation`, `aggregate_functions_null_for_empty`, `group_by_use_nulls`, `union_default_mode`, `asterisk_include_*`, `output_format_*` and `date_time_output_format`;
- **no-op:** it affects only performance or chDB internals, with the reason given, for example `max_threads` (mapped to DataFusion's `target_partitions`), `query_plan_*`, `use_text_index_*`, `enable_producing_buckets_out_of_order_in_aggregation`, `timeout_before_checking_execution_speed` and `max_bytes_before_external_group_by` (mapped to spill);
- **refused:** `115` or `164`.

A setting with a value other than its default that changes results and is not honoured is refused. It is never ignored.

Tests: `every_front_setting_is_classed_by_the_engine` (it fails when the front adds a setting); `honoured_settings_match_the_oracle`. Effort 4–5 days. Commit: `house: settings on the Rust engine (HE1 Task 37)`.

### Task 38 — Metadata: `system` tables, `SHOW`, `DESCRIBE`, `EXISTS`, safe table functions

**Files:** `fabric/crates/loams-house-engine/src/system/`; on the front, `loams-house/src/system.rs` gains a structured form of what it gives chDB today.

**Scope:**
- **`system` tables.** The front already decides what each one shows: `system.tables`, `columns`, `databases`, `parts`, `settings`, `functions`, `clusters`, `macros`, `mutations`, `distributed_ddl_queue`, `query_log`, `one` and `numbers`. In Rust mode it sends them as Arrow batches in the pins (`TablePin::Inline`), and the engine serves them as `MemTable`s. `system.functions` lists the **Rust engine's** registry, which fixes Task 15's expected gap that editors offer disabled functions.
- **Statements.** `SHOW TABLES`, `SHOW DATABASES`, `SHOW CREATE TABLE` (the stored `loams.ch.ddl`), `DESCRIBE TABLE` (from the pin's columns, as ClickHouse prints them, including plain views, which fixes R9.13's gap), `EXISTS`, `SELECT version()`, `currentDatabase()` and `timezone()`.
- **Table functions.** `numbers`, `numbers_mt`, `zeros` and `system.numbers`. Everything that reaches outside the namespace stays `344`.

**Tests:** `system_tables_match_the_front_on_chdb` (the same catalog shown through both engines); `describe_matches_the_oracle`.

**Exit:** Langfuse's `system.parts` and `system.tables` reads and SigNoz's cluster tables answer as on chDB. Effort 6–8 days. Commit: `house: system tables and metadata statements on the Rust engine (HE1 Task 38)`.

### Task 39 — The Rust worker, hsw1 v2 and the front's engine switch (D1577, D1578)

**Files:**
- `fabric/crates/loams-house-engine/src/bin/loams-house-rworker.rs` and `src/worker.rs`.
- `fabric/crates/loams-house-ipc/src/lib.rs`: `PROTOCOL_VERSION = 2`, `Execute.tables: Vec<TablePin>`, and `Frame::Resolve` / `Frame::Resolved(Resolution)`.
- `fabric/crates/loams-house/src/{admission.rs, http.rs, read.rs, config.rs}`: the engine choice, the pool of Rust workers and pin building.
- `deploy/` and `docs/house/`: the configuration key.

**Design:**
- **The worker.** `loams-house-rworker` speaks hsw1 as `loams-house-worker` does: `Ready`, `Bind`, `Execute` with streamed `Chunk`s, `Progress`, `Stats`, then `Error` or `Done`. A cancel is `SIGKILL`, as today. It answers `Resolve` from Task 22's `Resolution` instead of chDB's `EXPLAIN AST`. The front's deny list, in Rust mode, checks the resolution's functions against the security list (`344`), and the planner refuses everything outside the profile (`48`, D1558). The same front code builds `TablePin`s in place of view DDL.
- **Engine choice (D1577).** `[house] engine = "chdb" | "rust" | "prefer-rust"` sets the default, and the namespace's own `engine` setting in the catalog overrides it. `shadow` is for tests only: chDB answers and the Rust engine's answer is compared and logged.
- **What runs where in HE1.** In `rust` and `prefer-rust`, `SELECT`, `SHOW`, `DESCRIBE`, `EXISTS` and the read halves of `INSERT … SELECT`, `CREATE TABLE … AS SELECT` and materialized views run on the Rust engine. Every write, DDL statement, mutation, compaction and MV insert stays on chDB. In HE1 a namespace on `rust` therefore keeps a chDB worker for writes. `prefer-rust` falls back to chDB only on a `48` from planning, before any output, and the query log records `loams_engine = 'rust' | 'chdb' | 'chdb-fallback'`.
- **The bucket.** The worker reads through the same `house-cache` forwarder as chDB (D766 for HE1), with dummy keys. Task 40 adds the in-process cache.

**Tests:** `rworker_links_no_libchdb` (`cargo tree -p loams-house-engine` and a symbol check of the built binary); `fallback_only_before_the_first_byte`; `rust_and_chdb_answer_alike_through_the_front` (the HS1 `select/` corpus through `loams-fabric house` in each mode); the existing House suites in `rust` mode.

**Oracle:** `targets.toml` gains `house-rust-front` (the House with `engine = rust`), which runs in the nightly job.

**Exit:** the House runs the HS1 corpus and Tasks 22–38's corpora through the front in `rust` mode. Effort 10–12 days. Commit: `house: the Rust worker and the engine switch (HE1 Task 39)`.

### Task 40 — Worker hardening: the sandbox and the in-process foyer cache (D1557, D1558)

**Scope:**
- **The sandbox.** The Rust worker runs under the House's existing worker sandbox (`loams-house/src/sandbox.rs` and the worker side's network namespace, Landlock, seccomp and cgroups), with a seccomp profile narrowed for a binary without libchdb. `loams-sandbox` (§56, SB1) replaces it when it exists.
- **The cache.** A foyer cache (RAM plus NVMe, through `loams-cache`) sits under the worker's `object_store`. It holds Parquet footers, page indexes and column chunks, with coalesced range reads (1–8 MB, §75 §11). A worker keeps its cache across statements of its namespace and drops it on `Bind` to another.

**Tests:** `rworker_sandbox_refuses_what_chdb_workers_refuse` (the HS1 sandbox probes); `rworker_holds_no_credentials`; `cache_serves_a_second_read_without_the_store` (counts store requests).

**Exit:** the sandbox suites pass for the Rust worker; a warm second read of a gate query makes no store request. Effort 6–8 days. Commit: `house: the Rust worker's sandbox and cache (HE1 Task 40)`.

### Task 41 — OpenPanel reads on the Rust engine

`scripts/house/app-on-house.sh openpanel --engine rust` runs the CI job "House apps: OpenPanel on the Rust engine". It is the R9.6 gate with the House set to `engine = rust`. The replay (R9.3) compares every distinct `SELECT`, and OpenPanel then runs end to end on the House, with writes on chDB and reads on Rust. A `prefer-rust` pass first measures coverage: the fraction of distinct `SELECT`s that Rust serves and that match. The task fixes the gaps it finds in whichever crate owns them, each with its own oracle case. It also extends the driver to the imports, cohorts, groups and Search Console pages (R9.7). The job starts as optional and joins `Connectors required` once it passes on `dev`.

Exit: the replay is identical with no fallback (`rust`, strict); OpenPanel runs with 0 errors. Effort 4–6 days plus gaps. Commit: `conformance: OpenPanel reads on the Rust engine (HE1 Task 41)`.

### Task 42 — Langfuse v4 and v3 reads on the Rust engine

This builds the missing Langfuse gate (t9 item 1): `compose.house.yaml`, a `langfuse-v4` case and a `langfuse-v3` case in `app-on-house.sh`, migrations over native once #1043 merges, and `pages.sh` as the page check. It runs first on chDB, then with `--engine rust`, and re-mines Langfuse through `chlog`. Gaps are fixed as in Task 41.

Exit: both lines replay identically on `rust` and render all 16 pages. Effort 4–6 days plus gaps. Commit: `conformance: Langfuse on the Rust engine (HE1 Task 42)`.

### Task 43 — `JSON` columns on the Rust engine (interim text)

A `JSON(…)` column is stored as its JSON text (R9.63), read as a lazily parsed document, and these work on it:
- subcolumns (`` c.`a.b` ``, `c.a.b`) that give a `Dynamic` value;
- typed paths from the column's parameters, and `max_dynamic_paths`;
- `::String` and other casts;
- `IS NULL` / `IS NOT NULL`, `JSONAllPaths` and `dynamicType`.

The engine type is `Utf8` with the `JSON(…)` text in the metadata (Task 21). A subcolumn read plans as an extraction UDF whose return type comes from the typed path, or is `String` for a dynamic one. That is SigNoz's use. The limits are R9.63's (an integral float reads back as `Int64`), recorded as `incompatibility` entries. Variant storage is HE4.

Tests: `json_columns_match_the_oracle` (R9.63's cases, on the Rust engine). Effort 6–8 days. Commit: `house: JSON columns on the Rust engine (HE1 Task 43)`.

### Task 44 — SigNoz reads on the Rust engine

This is the SigNoz replay from R9.64 (3): multi-database (`--database` repeated), with `chlog` on its HTTP traffic. It runs over native and HTTP against the House with `engine = rust`. `quantilesState` comes from Task 29, `DateTime64(9)` from Task 26, `Distributed` views and cluster `system` tables from Task 38. Refreshable materialized views are not needed to replay reads: their targets are copied like any table. The full SigNoz gate (writes, refreshable views) is HE2 Task 72.

Exit: SigNoz's replay is identical on `rust`. Effort 4–6 days plus gaps. Commit: `conformance: SigNoz reads on the Rust engine (HE1 Task 44)`.

### Task 45 — The stateless replay

This finishes HE0 Task 7. Rebase `he0-t7-stateless` and re-run the filter on the final profile (about 40 CPU minutes). Commit `conformance/clickhouse/stateless/manifest.toml`, and vendor the kept `0_stateless` `.sql`/`.reference` files from ClickHouse 26.9.2.8's tag, with NOTICE attribution, under `conformance/clickhouse/stateless/tests/`. Record the chDB House's pass rate per category as the baseline (HE0's missing number), then the Rust engine's pass rate through `loams-house-engine-dev`. The nightly job `house-conformance` reports both.

Exit: the manifest and both pass rates are recorded; HE1's exit needs at least 50 % on Rust. Effort 4–5 days. Commit: `conformance: the stateless replay and its baselines (HE1 Task 45, HE0 Task 7)`.

### Task 46 — Performance: the budget and the benchmarks (D1803)

**Files:** `fabric/crates/loams-house-mine/src/bench.rs` (subcommand `bench`) and `docs/house/performance.md` (new section).

**Measures:**
- Worker boot to the first answer, and idle memory.
- Every distinct gate-application `SELECT` from Tasks 41–44 on chDB and on Rust over the same lake tables: cold (no cache) and warm, p50 and p95 over 10 runs, plus peak memory.
- ClickBench (fetched at test time, never committed, D414) as an internal number, on chDB and on Rust, for HE3's baseline (Q1182).

**The budget for HE1 and HE2 (D1803, estimate):**
- per gate-application query, p50 at most 1.5 times chDB's;
- no single query slower than 3 times chDB's;
- peak memory no more than chDB's;
- worker boot under 20 ms p95.

A miss is a ruling that names the query and the HE3 work that closes it.

Exit: the numbers are recorded; the budget holds or each miss is ruled. Effort 5–6 days. Commit: `house: Rust engine benchmarks against chDB (HE1 Task 46)`.

### Task 47 — HE1 exit

`docs/plans/he1-exit-report.md` covers:
- Tier 0 results on the oracle, and the stateless pass rate;
- the three replays;
- OpenPanel end to end on `rust`;
- the performance figures;
- the open items for HE2.

The owner reviews it before HE2 starts. Effort 2 days. Commit: `docs: the HE1 exit report (HE1 Task 47)`.

## HE2 — Writes, deletion vectors and the release gate (Tasks 60–77)

**Exit (§75 §16.1 as amended by D1571 and D1801):**
- **The gate applications.** Langfuse v4 (plus v3 until 2027-01), OpenPanel and SigNoz pass in full on `engine = rust`: their own migrations (native included), ingest, page reads, and replays identical to the 26.9.2.8 oracle.
- **Interop.** The Iceberg interop matrix is green.
- **Conformance and speed.** At least 70 % of the filtered stateless tests pass (Q1320), and the performance budget (D1803) holds.
- **Then the switch.** `rust` becomes the House's default, and chDB becomes the oracle plus an opt-in fallback per namespace until HE3's exit (D1801).

The fresh tier, the NDJSON ingest API and endpoint pipes (Tasks 75–77) are HE2 work but not part of the gate.

**Order.**
- Task 60 comes first.
- Then these can run in parallel: Tasks 61, 63, 68 and 69, and Task 66 once its iceberg pieces are pinned.
- Task 62 needs 61, and Task 64 needs 62.
- Task 65 needs 64, and Task 67 needs 62 and 66.
- Task 70 needs 62 and 66.
- Task 71 can start any time after HE1.
- Task 72 needs 62–67 and 70, and Tasks 73–74 close the gate.

| Task | Title | Effort (engineer-days, estimate) | Depends on |
|---|---|---|---|
| 60 | Rulings; one iceberg-rust; v3 table writes | 5–6 | HE1 |
| 61 | Input formats in Rust | 12–15 | 60 |
| 62 | The Rust write path: `INSERT`, defaults, constraints, merge on insert, v3 data files | 12–15 | 61 |
| 63 | Statement analysis without chDB | 8–10 | 60 |
| 64 | Materialized views on the Rust engine, and AggregatingMergeTree merged on insert | 8–10 | 62 |
| 65 | Refreshable materialized views (R9.64's design) | 6–8 | 64 |
| 66 | Mutations on deletion vectors and position deletes | 12–15 | 60 |
| 67 | Compaction and merges on the Rust engine; TTL jobs; lineage through rewrites | 10–12 | 62, 66 |
| 68 | The commit protocol: TLA+ and deterministic simulation (D1554) | 8–10 | 60 |
| 69 | The Iceberg interop matrix per PR (suite 6) | 5–6 | 60 |
| 70 | Migration of chDB-written tables (D1802) | 6–8 | 62, 66 |
| 71 | Differential fuzzing against chDB (suite 4) | 6–8 | HE1 |
| 72 | The gate applications in full on the Rust engine | 8–12 | 62–67, 70 |
| 73 | chDB off the default path | 4–5 | 72 |
| 74 | The HE2 release gate and exit report | 3–4 | 72, 73 |
| | **HE2 total to the gate** | **113–144** | |
| 75 | The fresh tier (D1555): a WAL on a Loams stream, in-memory parts, L0 Parquet, batched commits under a lease | 15–20 | 62, 68 |
| 76 | The NDJSON ingest API with quarantine (D1559) | 6–8 | 62 |
| 77 | Endpoint pipes through `loams-gateway` (D1559, Q1187) | 8–10 | HE1 |
| | **HE2 total** | **142–182** | |

**HE1 + HE2: 351–446 engineer-days (estimate).**

### Task 60 — Rulings; one iceberg-rust; v3 table writes

Move `loams-house-lake`'s writer (`append`, `files`, `metadata`, `write`, `rewrite`, `expire`, `orphans`) into `loams-house-iceberg` on the fork pin and Arrow 59, and retire iceberg 0.10.1 and Arrow 58 from `fabric/`. The House front keeps its API through a re-export, so the chDB path keeps working. New tables are created as **v3** with row lineage (the fork's fast append assigns `first-row-id`, R11.1). `DateTime64(7..9)` columns get `timestamptz_ns` (D1572's switch) and `JSON` stays text (Variant is HE4). Before this task is done, record the fork commits it pins and their CI.

Tests: the House's lake suites unchanged; `new_tables_are_v3_with_lineage`; `v2_tables_still_written_when_pinned_to_v2` (for the fallback). Effort 5–6 days. Commit: `house: one iceberg-rust and v3 tables (HE2 Task 60)`.

### Task 61 — Input formats in Rust

**Files:** `fabric/crates/loams-house-formats/src/input/`.

**Scope:**
- **Formats.** `JSONEachRow` (with `input_format_skip_unknown_fields`, `date_time_input_format = best_effort`, `input_format_json_throw_on_bad_escape_sequence`, defaults for omitted fields, `input_format_try_infer_*`), `JSON`/`JSONCompact*`, `CSV`/`TSV` with names, `Values` (expressions in values, `input_format_values_interpret_expressions`), `RowBinary[WithNamesAndTypes]`, `Native` (from `loams-house-native` blocks), `Parquet` and `ArrowStream`.
- **Conversions.** Each format reads values with ClickHouse's conversion rules (Task 21 types, Task 33 conversions).
- **What it replaces.** These parsers replace `chdb_stream_insert` (R1.7) for the Rust path.

Tests: `inputs_match_the_oracle` (the same body inserted on the oracle and here, the rows compared); `bad_rows_fail_as_the_oracle_does` (codes and row numbers). Effort 12–15 days. Commit: `house: ClickHouse input formats in Rust (HE2 Task 61)`.

### Task 62 — The Rust write path

`INSERT … VALUES | FORMAT | SELECT` on `engine = rust` runs without chDB. The steps follow `insert.rs`:
1. **Staging and constraints.** Rows are staged in Arrow, `DEFAULT` and `MATERIALIZED` expressions are evaluated by the engine, and `CONSTRAINT … CHECK` is enforced (`469`).
2. **Merge on insert.** ClickHouse's merge on insert is applied per engine (R9.4: Replacing, Summing, VersionedCollapsing, and Aggregating with Task 29's states), after a stable sort by the key and then by insert order.
3. **Files.** Parquet data files are written with Iceberg field ids, statistics, the House's encodings and partition splits.
4. **Commit and answer.** The files go through the existing commit queue (group commit, deduplication, rebase-retry). Async inserts are answered as today.

The worker gets a write grant only for the table's `data/` prefix in the namespace. Uploads stay in the front (R1.5) until HE3 vends credentials.

Tests: `rust_inserts_match_chdb_inserts` (the same input through both engines gives the same rows and the same answers to the oracle's reads); `merge_on_insert_matches_the_oracle`. Effort 12–15 days. Commit: `house: inserts on the Rust engine (HE2 Task 62)`.

### Task 63 — Statement analysis without chDB

Everything the front still asks chDB moves to the Rust front end:
- `Classify`;
- the deny list's `EXPLAIN AST` (the front's own reader plus Task 22's `Resolution`);
- `DESCRIBE` of a `CREATE TABLE … AS SELECT` (columns inferred from the plan);
- `CREATE VIEW` validation;
- materialized-view source detection (R9.14).

After this task a namespace on `rust` needs no chDB worker at all. Tests: `classifier_agrees_with_chdb` (the classify and deny corpora); `ctas_columns_match_the_oracle`. Effort 8–10 days. Commit: `house: statement analysis on the Rust engine (HE2 Task 63)`.

### Task 64 — Materialized views on the Rust engine

These now run on the Rust engine:
- `TO` and inner-table views, fired on insert with the view's `SELECT` over the inserted block;
- `Null` sources;
- `ALTER … MODIFY QUERY`, and a view bound to its source table's identity (R4.4, R9.14);
- the AggregatingMergeTree merge on insert (the remaining item of R9.14);
- `parallel_view_processing`, satisfied.

Tests: `materialized_views_match_the_oracle` (Langfuse's `events_core_mv`, OpenPanel's six views, SigNoz's `TO` views); `views_follow_a_renamed_source`. Effort 8–10 days. Commit: `house: materialized views on the Rust engine (HE2 Task 64)`.

### Task 65 — Refreshable materialized views

R9.64's design is built on the Rust engine:
- the `REFRESH EVERY … [OFFSET] [RANDOMIZE FOR] APPEND TO … [EMPTY]` DDL;
- `loams.refresh.<id>` and `loams.refresh_at.<id>` on the target, committed together with the appended rows;
- the scheduler pass in `mv.rs`, with `now()` pinned per run.

`REFRESH AFTER`, `DEPENDS ON` and refreshes that replace rather than append are `48`. Tests: `refreshable_views_append_once_per_period` (several fronts, only the commit winner appends); `signoz_metric_reduction_views_match_the_oracle`. Effort 6–8 days. Commit: `house: refreshable materialized views (HE2 Task 65)`.

### Task 66 — Mutations on deletion vectors (D1552, Q1184)

**Scope:**
- **Deletes.** `ALTER TABLE … DELETE`, lightweight `DELETE FROM` and `DROP PARTITION` (a metadata delete) write **deletion vectors** on v3 tables and position deletes on v2 tables. They use the fork's `DeletionVectorWriter` and `RowDelta` (G2, G3) and do not rewrite files.
- **Updates.** `ALTER … UPDATE` and lightweight `UPDATE` write a deletion vector for the old rows and append the new rows.
- **`CLEAR COLUMN`.** Rewrites only the affected files.
- **Commit and answer.** A mutation commits as one `RowDelta` with `assert-ref-snapshot-id` and retry. It answers after the commit whatever `mutations_sync` says (Q1184's default), and `system.mutations` is filled from the commit summaries.
- **What it replaces.** R9.5's copy-on-write path is retired once both pass.
- **Semantics.** As R9.5 measured: every version of a key is mutated, and a key column is `420`.

Tests: `mutations_match_the_oracle` (R9.5's cases); `deletes_write_no_data_files`; `spark_trino_duckdb_read_house_deletion_vectors` (Task 69's stack). Effort 12–15 days. Commit: `house: mutations on deletion vectors (HE2 Task 66)`.

### Task 67 — Compaction and merges on the Rust engine

**Scope:**
- **Merges.** `OPTIMIZE [FINAL]` and the background scheduler merge with Task 27's operators: Replacing, Summing, (Versioned)Collapsing and Aggregating. Rows of one key are kept in write order across files by carrying `(seq, row)` through rewrites, which closes R9.2's and R9.4's ties across files.
- **Deletion-vector compaction.** Files with many deleted rows are rewritten.
- **TTL.** A partition is dropped when the TTL column is the partition column. Otherwise a maintenance job writes deletion vectors for expired rows.
- **`Null` tables.** Their stored rows expire.
- **Row lineage through rewrites (G5, apache/iceberg-rust#3401).** Built fork-first.

Tests: `compaction_keeps_answers` (every gate-application table, before and after `OPTIMIZE FINAL`, against the oracle); `ties_survive_compaction`. Effort 10–12 days. Commit: `house: compaction on the Rust engine (HE2 Task 67)`.

### Task 68 — The commit protocol (D1554)

**Model.** `spec/tla/HouseCommit.tla` models:
- one committer per table under a metastore lease;
- batched commits and optimistic `assert-ref-snapshot-id` retries;
- the cursors of pipes and refreshable views committed in the same snapshot;
- compaction against concurrent `RowDelta`s.

TLC checks safety (no lost commit, no double append, cursors move monotonically) on small models. **Simulation.** A deterministic simulation in `loams-house-iceberg` (seeded scheduler, failure injection on store and catalog calls) runs the House's commit queue, following §31 and §55. **Tests:** TLC in CI on the model's paths; `commit_simulation_holds_under_failures` (1 000 seeds per PR, 100 000 nightly). **Effort and commit:** 8–10 days; `spec: the House commit protocol in TLA+ and simulation (HE2 Task 68)`.

### Task 69 — The Iceberg interop matrix (suite 6)

`conformance/clickhouse/interop/`: the House writes v3 tables with every Tier 0 type, partitions, deletion vectors, lineage and nanosecond times, and Spark 4.0.4 (Iceberg 1.12.0), Trino (484 or later) and DuckDB 2.0.0 read them back through Lakekeeper v0.13.6, and the reverse. The answers are compared as fingerprints (the spike's method). The job runs per PR on storage paths. Tests: `interop_matrix_is_green`. Effort 5–6 days. Commit: `conformance: the Iceberg interop matrix (HE2 Task 69)`.

### Task 70 — Migration of chDB-written tables (D1802)

Nothing is rewritten at the switch.
- **What the Rust engine reads.** Every encoding the chDB House wrote: v2, ClickHouse types in `loams.ch.types`, nanosecond `long`s, JSON text, ClickHouse-serialised states and copy-on-write snapshots. HE1 Task 26 already does this.
- **The v3 upgrade.** A table moves to v3 in place (`UpgradeFormatVersion`, metadata only) at its first write on the Rust engine. A per-field `loams.ch.storage` property keeps each field's legacy encoding until a rewrite (`OPTIMIZE TABLE … FINAL` or compaction) writes the v3 type.
- **Going back to chDB.** A namespace may return to `engine = chdb` until HE3 as long as its tables carry no deletion vectors. After the first deletion vector, chDB answers `48` naming the table unless chDB 26.9 reads v3 deletion vectors (verify, Q1322).

Tests: `chdb_written_tables_read_identically` (Task 26's test, extended to tables from a 0.1.x House and every gate application's data, read and written on Rust, then compared with the oracle); `upgrade_keeps_legacy_encodings`. Effort 6–8 days. Commit: `house: chDB-written tables on the Rust engine (HE2 Task 70)`.

### Task 71 — Differential fuzzing against chDB (suite 4)

A grammar-based generator over the profile produces queries over seeded tables. SQLancer-style oracles check them: TLP (ternary logic partitioning) and NoREC (non-optimising reference engine), with chDB and the 26.9.2.8 server as references. The fuzzer runs nightly, files each new mismatch into triage as `untriaged`, and shrinks the cases. Tests: `fuzzer_finds_a_planted_bug`. Effort 6–8 days. Commit: `house: differential fuzzing of the Rust engine (HE2 Task 71)`.

### Task 72 — The gate applications in full on the Rust engine

These jobs run with writes, ingest and migrations on the Rust engine:
- "OpenPanel on the House" (Task 41's job);
- Langfuse v4 and v3 (Task 42's, migrations over native);
- SigNoz (`signoz_on_house_smoke`, with refreshable views, Task 65).

Each is replayed against the oracle. Any of Tasks 13–15's queued gates that have started also run on Rust. The task fixes the gaps it finds.

Exit: the three jobs are required and green on `rust`. Effort 8–12 days plus gaps. Commit: `conformance: the gate applications on the Rust engine (HE2 Task 72)`.

### Task 73 — chDB off the default path (D1801, D1567)

**The switch.**
- `[house] engine = "rust"` becomes the default.
- `engine = chdb` stays as an opt-in per namespace until HE3's exit.
- The chDB House stays an oracle target in CI.

**Images.** A House image without libchdb ships beside today's. The default image drops libchdb at HE3's exit (D1567); on Windows and the desktop the House runs natively from this task on.

**Docs and labels.** The docs, the compatibility page (`chsurface-3`, D1547, Q1174's badge with the passing application versions) and the labels change: Rust engine "preview", chDB "fallback".

Tests: the release compose with no libchdb passes the light-stack smoke. Effort 4–5 days. Commit: `house: the Rust engine is the default (HE2 Task 73)`.

### Task 74 — The HE2 release gate and exit report

`docs/plans/he2-exit-report.md` shows that the gate holds:
- the three applications pass;
- interop is green;
- stateless is at least 70 %;
- the performance budget holds.

It also lists what HE3 inherits. The owner reviews it, then D1542's switch is final. Effort 3–4 days. Commit: `docs: the HE2 exit report (HE2 Task 74)`.

### Tasks 75–77 — After the gate (HE2, not gating)

- **Task 75, the fresh tier (D1555, 15–20 days).** A Loams stream is the WAL by default, and an Iggy topic when the Event Fabric runs. Then:
  - In-memory parts, small L0 Parquet files, size-tiered compaction, and one Iceberg commit every N seconds per table under the lease (Task 68), with the offsets committed in the same snapshot.
  - Reads union the committed snapshot with the fresh tier, and `X-Loams-Consistency-Token` gives read-your-writes.
  - The commit rate pushes back on ingest.

  Tests: `fresh_rows_are_visible_before_the_commit`, `wal_replay_is_exactly_once`.
- **Task 76, the NDJSON ingest API (D1559, 6–8 days).** `POST /v1/house/ingest/<table>` with quarantine tables (`<table>_quarantine`, one month), tokens and rate limits through Loams Meter's gate (D1125).
- **Task 77, endpoint pipes (D1559, Q1187, 8–10 days).** Templated SQL published as authenticated routes in `loams-gateway`'s `http` module. Callers off the tailnet reach them only through §69's edge listener.

## HE3 — Distributed and serverless (milestone plan)

**Goal:** scale to zero and burst scale-out on object storage, with ClickBench within 60–80 % of ClickHouse warm (Q1182, internal). libchdb leaves the production images (D1567, D1801).

| # | Main task | Effort (estimate) | Depends on |
|---|---|---|---|
| H3.1 | The coordinator and worker split. A stateless coordinator (`loams-house-engine` in the front's process group) plans and fragments by file and row group; workers run fragments. DataFusion's physical plan is serialised to workers (the `datafusion-proto` approach, evaluated against Ballista's design and `datafusion-distributed`, licences checked) | 20–25 | HE2 |
| H3.2 | Exchanges: direct Arrow Flight streams between workers, and an S3 exchange where workers cannot reach each other (Lambda); work stealing for skew | 15–20 | H3.1 |
| H3.3 | Two-level aggregation, spill to NVMe, a per-group top-N for `LIMIT BY`, dictionary-preserving kernels for `LowCardinality`, runtime-dispatched SIMD where profiling shows the need (§75 §11) | 15–20 | HE1 Task 46's numbers |
| H3.4 | Cold start: per-table snapshot summaries beside the metadata, NVMe footer and manifest cache, warm pools, snapshot-restored microVM workers through `loams-sandbox` | 10–12 | SB1 (`loams-sandbox`) |
| H3.5 | Compute targets: pods and ECS Fargate first, Cloud Run, Cloudflare Containers (§70); the Lambda spike in a BYOC AWS account (Q1183); separate compute groups per tenant for ingest, queries and compaction (D1563) | 15–20 | H3.1, §70 |
| H3.6 | Vended, prefix-scoped credentials for workers (§75 §14), replacing the forwarder | 5–6 | H3.1 |
| H3.7 | Performance gates nightly (suite 7): ClickBench (fetched at test time), a Tinybird-style workload, an observability workload; regression alerts | 6–8 | H3.3 |
| H3.8 | Distributed correctness: the fragment and exchange protocol in simulation with injected failures (suite 8) | 8–10 | H3.2 |
| H3.9 | libchdb out of the production images; `engine = chdb` removed from production builds; chDB kept only in CI as the oracle | 3–4 | H3 exit |

**HE3 total: about 97–125 engineer-days (estimate).**

**Risks:**
- Lambda's limits (time, memory, payload) may rule it out for scans: the spike decides.
- Distributed plans multiply the places where ClickHouse semantics can drift (totals and `LIMIT BY` across fragments, `uniq` merges), so the oracle runs on the distributed path too.
- The ClickBench target may need operators DataFusion lacks. Q1182's bar is an estimate.

**Exit:**
- Scale to zero and burst scale-out are demonstrated on two targets.
- The ClickBench target is met (internal).
- The production images carry no libchdb.

## HE4 — Tier 1 (milestone plan)

**Goal:** the Tier 1 surface, and the product layer in beta.

| # | Main task | Effort (estimate) |
|---|---|---|
| H4.1 | Variant storage for `JSON` and `Dynamic` (iceberg-rust G8/G9 fork-first; #2880, #2546, #2581), replacing the interim text (R9.63) | 12–15 |
| H4.2 | Incremental materialized views on row lineage (`_row_id`, `_last_updated_sequence_number`), and CDC reads | 10–12 |
| H4.3 | `SummingMergeTree` and `AggregatingMergeTree` beyond the gate applications' functions: every mined `-State` function, `SimpleAggregateFunction` families | 8–10 |
| H4.4 | `retention`, `sequenceMatch`, `sequenceCount` when mined | 5–6 |
| H4.5 | Skip indexes built and used (`minmax`, `set`, `bloom_filter`, `ngrambf_v1`, `tokenbf_v1`; mined by 2–6 applications), stored as Puffin statistics | 12–15 |
| H4.6 | Full-text (`text` index) and vector indexes with the engine's Tantivy and vector code (Q1181) | 15–20 |
| H4.7 | Parquet and Iceberg table functions on the namespace's own prefix (D1558) | 4–5 |
| H4.8 | Tier 1 functions from the profile (58), formats (`RowBinaryWithNamesAndTypes` is already in HE1) and settings | 10–12 |
| H4.9 | The product layer in beta: copy pipes, endpoint analytics, Loams Meter metering of CPU time and bytes scanned (D1561) | 10–12 |

**HE4 total: about 86–107 engineer-days (estimate).**

**Risks:**
- Variant in iceberg-rust depends on upstream PRs.
- Index formats shared with the engine's collections must not couple the two products' release cycles (Q1181).

## HE5 — On demand (milestone plan)

Tier 2 items users ask for, each through the profile's rule (mined in 3 or more applications, or needed by a gate application). The remaining native-protocol pieces: external tables, `TableColumns`, `ProfileEvents`, `Log` packets, `Cancel` before the first result, and query stages other than `Complete` (R9.42). Dictionaries when an application needs them. `ASOF JOIN` if it is mined into Tier 0 earlier, it moves to Task 35. Each item gets a task when a user asks for it; HE5 is never a release gate.

## Rulings made during execution

Rulings live in one file per task under [`he1-rulings/`](he1-rulings/), so tasks running in parallel do not conflict on one table. Each file keeps this plan's columns (`#`, Ruling, Why, Cost if wrong) and its own numbering (R<task>.<n>).

| Task | Rulings |
|---|---|
| 0. Rulings | [t0.md](he1-rulings/t0.md) |
| 1. The apps manifest and compose harness | [t1.md](he1-rulings/t1.md) |
| 2. loams-house-mine, the extractor | [t2.md](he1-rulings/t2.md) |
| 3. Mine Langfuse | [t3.md](he1-rulings/t3.md) |
| 4. Mine OpenPanel | [t4.md](he1-rulings/t4.md) |
| 5. Mine five or more further applications | [t5.md](he1-rulings/t5.md) |
| 6. The oracle runner | [t6.md](he1-rulings/t6.md) |
| 7. ClickHouse stateless replay manifest | [t7.md](he1-rulings/t7.md) |
| 8. The function and engine matrix | [t8.md](he1-rulings/t8.md) |
| 9. Langfuse and OpenPanel on today's House | [t9.md](he1-rulings/t9.md) |
| 10. Spike: the ClickHouse dialect on DataFusion | [t10.md](he1-rulings/t10.md) |
| 11. Spike: iceberg-rust v3 | [t11.md](he1-rulings/t11.md) |
| 12. HE0 exit | [t12.md](he1-rulings/t12.md) |
| 13. Queued gate: OpenMeter | [t13.md](he1-rulings/t13.md) |
| 14. Queued gate: Laminar (was Helicone) | [t14.md](he1-rulings/t14.md) |
| 15. Queued gate: BI tools | [t15.md](he1-rulings/t15.md) |
| 20. Rulings, versions and crate skeletons | [t20.md](he1-rulings/t20.md) |
| 21. The ClickHouse type system (`loams-house-types`) | [t21.md](he1-rulings/t21.md) |
| 22. The front end's core (`loams-house-sql`) | [t22.md](he1-rulings/t22.md) |
| 23. The engine oracle | [t23.md](he1-rulings/t23.md) |
| 24. ClickHouse type rules | [t24.md](he1-rulings/t24.md) |
| 25. Name resolution | [t25.md](he1-rulings/t25.md) |
| 26. Lake-table reads (`loams-house-iceberg`) | [t26.md](he1-rulings/t26.md) |
| 27. Merge-family reads: `FINAL` | [t27.md](he1-rulings/t27.md) |
| 28. Aggregates, combinators, parametric aggregates and `windowFunnel` | [t28.md](he1-rulings/t28.md) |
| 29. ClickHouse-exact sketches and states | [t29.md](he1-rulings/t29.md) |
| 30. Functions I: date and time | [t30.md](he1-rulings/t30.md) |
| 31. Functions II: strings, regular expressions, hashes, UUIDs, IPs | [t31.md](he1-rulings/t31.md) |
| 32. Functions III: arrays, maps, tuples, lambdas | [t32.md](he1-rulings/t32.md) |
| 33. Functions IV: conversions, conditionals, maths, JSON extraction | [t33.md](he1-rulings/t33.md) |
| 34. Clauses I: `LIMIT BY`, `WITH FILL`, `WITH ROLLUP`/`CUBE`, `WITH TOTALS` | [t34.md](he1-rulings/t34.md) |
| 35. Clauses II | [t35.md](he1-rulings/t35.md) |
| 36. Output formats (`loams-house-formats`) | [t36.md](he1-rulings/t36.md) |
| 37. Settings on the Rust engine (D1805) | [t37.md](he1-rulings/t37.md) |
| 38. Metadata: `system` tables, `SHOW`, `DESCRIBE`, `EXISTS`, safe table functions | [t38.md](he1-rulings/t38.md) |
| 39. The Rust worker, hsw1 v2 and the front's engine switch (D1577, D1578) | [t39.md](he1-rulings/t39.md) |
| 40. Worker hardening: the sandbox and the in-process foyer cache (D1557, D1558) | [t40.md](he1-rulings/t40.md) |
| 41. OpenPanel reads on the Rust engine | [t41.md](he1-rulings/t41.md) |
| 42. Langfuse v4 and v3 reads on the Rust engine | [t42.md](he1-rulings/t42.md) |
| 43. `JSON` columns on the Rust engine (interim text) | [t43.md](he1-rulings/t43.md) |
| 44. SigNoz reads on the Rust engine | [t44.md](he1-rulings/t44.md) |
| 45. The stateless replay | [t45.md](he1-rulings/t45.md) |
| 46. Performance: the budget and the benchmarks (D1803) | [t46.md](he1-rulings/t46.md) |
| 47. HE1 exit | [t47.md](he1-rulings/t47.md) |
| 60. Rulings; one iceberg-rust; v3 table writes | [t60.md](he1-rulings/t60.md) |
| 61. Input formats in Rust | [t61.md](he1-rulings/t61.md) |
| 62. The Rust write path | [t62.md](he1-rulings/t62.md) |
| 63. Statement analysis without chDB | [t63.md](he1-rulings/t63.md) |
| 64. Materialized views on the Rust engine | [t64.md](he1-rulings/t64.md) |
| 65. Refreshable materialized views | [t65.md](he1-rulings/t65.md) |
| 66. Mutations on deletion vectors (D1552, Q1184) | [t66.md](he1-rulings/t66.md) |
| 67. Compaction and merges on the Rust engine | [t67.md](he1-rulings/t67.md) |
| 68. The commit protocol (D1554) | [t68.md](he1-rulings/t68.md) |
| 69. The Iceberg interop matrix (suite 6) | [t69.md](he1-rulings/t69.md) |
| 70. Migration of chDB-written tables (D1802) | [t70.md](he1-rulings/t70.md) |
| 71. Differential fuzzing against chDB (suite 4) | [t71.md](he1-rulings/t71.md) |
| 72. The gate applications in full on the Rust engine | [t72.md](he1-rulings/t72.md) |
| 73. chDB off the default path (D1801, D1567) | [t73.md](he1-rulings/t73.md) |
| 74. The HE2 release gate and exit report | [t74.md](he1-rulings/t74.md) |
| 75. The fresh tier | [t75.md](he1-rulings/t75.md) |
| 76. The NDJSON ingest API | [t76.md](he1-rulings/t76.md) |
| 77. Endpoint pipes | [t77.md](he1-rulings/t77.md) |

### Owner approvals (2026-10-24)

The owner approved the pending decisions below on 2026-10-24 ("approve all with your recommendations").

- **D1576 / Q1188:** DataFusion **55.2** (Arrow 59, the iceberg-rust fork) replaces DataFusion 54.1. **Approved (owner, 2026-10-24).** R12.2 stands as written.
- **D1577, D1579, D1800, D1801:** **Approved (owner, 2026-10-24).**
- **Q1189:** `prefer-rust` (a fall back to chDB on a planner refusal) is **not** offered to users, only to tests, gates and the owner's dogfooding. **Approved (owner, 2026-10-24).**
- **Q1320:** HE2's filtered stateless pass rate is **70 %**. **Approved (owner, 2026-10-24).**
- **Q1321:** sqlparser and DataFusion gaps are **worked around** in `loams-house-sql` for now, with no fork. A later fork still needs the owner's consent. **Approved (owner, 2026-10-24).**
- **Q1322:** after the switch, a namespace may go back to `engine = chdb` until HE3's exit, for tables without deletion vectors. **Approved (owner, 2026-10-24).**
- **R9.67, and the SigNoz equivalent in #1176:** SigNoz and the Factory skip certificate verification on the internal network for the House's self-signed certificate (`skip_verify=true` on the `house-signoz` network), and the native TLS listener on 9440 is reachable on the tailnet with a password. **Accepted (owner, 2026-10-24).** R9.67 is in #1176's copy of `he1-rulings/t9.md`; mark it "Approved (owner, 2026-10-24)" there.
