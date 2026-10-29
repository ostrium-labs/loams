# HE0 exit report: what Phase 0 of the Loams House engine delivered

Task 12 of [plan HE1](2026-10-21-he1-loams-house-engine.md), track HE, design [§75](../design/75-loams-house-engine.md). Written 2026-10-23 at `dev` `d9a13f6e6` by the HE1 lead, after the owner stopped House work on 2026-10-22 and then asked for the Rust engine (HE1) to be planned in full and started ("same for rust clickhouse engine bro, spawn one agent and plan fully", 2026-10-23). Its rulings are R12.1 onward in [`he1-rulings/t12.md`](he1-rulings/t12.md). Numbers are measured unless marked **(estimate)**.

## Summary

- **Go for HE1**, with three conditions that the expanded plan turns into HE1 tasks: HE0 Task 7 (the stateless baseline) is finished as HE1 Task 45; the mined corpora join the oracle in HE1 Task 23; and the SigNoz gate on today's House is not a precondition for HE1, but its remaining items (R9.64) are scheduled before HE2's gate (Tasks 65 and 72).
- **§75 §16.1's HE0 exit is met for two of three gate applications.** The mining report covers 7 applications (D1543's rule needs 7 or more). OpenPanel runs fully on today's House as a **required** CI gate. Langfuse v4 runs fully on a local House (all 16 pages, no failing call), but its CI gate job was not built before the stop. SigNoz, added as the third gate application on 2026-10-22 (D1571), is **not passing yet**.
- **The two spikes say the plan is sound.** sqlparser plus DataFusion as libraries, with Loams' own front end and extension nodes, plan all 20 representative Tier 0 queries (6 as shipped). iceberg-rust 0.11's release candidate and `main` read Spark's deletion vectors and row lineage correctly. The deletion-vector writer and `RowDelta`, which no iceberg-rust build had, are built in Loams' fork and read identically by Spark, Trino, DuckDB 2.0.0 and iceberg-rust.
- **One version change is proposed** (D1576, Q1188): the engine crates start on DataFusion 55.2 with iceberg-rust from the fork, all on Arrow 59 (the `fabric/` workspace's Arrow), instead of DataFusion 54.1 with iceberg 0.11-rc.2 on Arrow 58. The fork's writer branches sit on iceberg-rust `main`, which is on Arrow 59.2, so the earlier pick would force a DataFusion major upgrade in the middle of HE2. DataFusion 55.2.0 (2026-10-07) passed the 14-day rule on 2026-10-21.
- **The effort is larger than the spike's figure, which counted only the dialect.** HE1 is **209–264** engineer-days and HE2 **142–182** (113–144 on the path to the release gate) **(estimate)**: 351–446 for both. The spike's 100–140 days for Tier 0 dialect work is about 110 days of HE1 (Tasks 22, 24, 25, 27, 28 and 34–35). The rest is what the spike did not price: the type system, the function library, output formats, lake reads, ClickHouse-exact sketches and state formats, the worker, the gates and, in HE2, the whole write path.

## 1. What HE0 delivered

| Task | Delivered | PRs |
|---|---|---|
| 0 Rulings | HS1 as built; what the House answered for the gate applications' likely gaps; the oracle pins (`clickhouse-server` 26.9.2.8, floor 24.3.18.7); application pins; candidate applications with licences and protocols | #994 (R0.1–R0.8) |
| 1 Apps manifest and harness | `conformance/clickhouse/apps/manifest.toml`, one compose stack per application with a `query_log`-enabled oracle, `scripts/house/apps.sh`; the `chlog` settings proxy that records settings sent at their default values | #1005, #1027 (R1.1) |
| 2 `loams-house-mine` | `extract`, `merge`, `scrub`, `report` over sqlparser 0.63 with `EXPLAIN AST` fallback; bound parameters, codecs, type arguments and migration templates | #992, #997, #1003, #1020 (R2.1–R2.13) |
| 3 Mine Langfuse | v4.54.0 (636 statements) and v3.225.11 (1 316), driver plus a 16-page browser walk; what Langfuse needs from ClickHouse | #1017 (R3.1–R3.3) |
| 4 Mine OpenPanel | 2.3.0, 181 statements, 167 features; `windowFunnel` used, `retention` and `sequenceMatch` not; VersionedCollapsing and six inner-table AggregatingMergeTree views | #1006 (R4.1–R4.6) |
| 5 Further applications and the profile | Plausible CE 3.2.1, SigNoz 0.145.0, Umami 3.4.0, HyperDX 2.40.0, OpenLIT 2.1.0 (PostHog skipped, R5.1); the first `profile.toml` and `docs/house/compat-profile.md` | #1040, #1045 (R5.1–R5.7) |
| 6 Oracle runner | `loams-house-mine oracle`: corpora over N targets, comparison rules that never hide row counts, types or error codes, a triage file | #1002 (R6.1–R6.6) |
| 7 Stateless manifest | **Not merged.** Code on branch `he0-t7-stateless` (the `stateless` subcommand and a nightly job), no manifest, no baseline | none (finished as HE1 Task 45) |
| 8 Function and engine matrix | `docs/house/compat-matrix.md` from chDB's dumps, with a CI check on unclassified mined features | #998, #1047 (R8.1–R8.5) |
| 9 Gate applications on today's House | OpenPanel fully on the House as a required gate; Langfuse v4 fully on a local House; SigNoz partly (350 of 373 DDL statements); 26 House changes (settings, ALTER reader, views, Distributed, readable AggregatingMergeTree, inner-table views, VersionedCollapsing, stable sorted inserts, copy-on-write mutations, SAMPLE, nanosecond times, JSON columns, cluster `system` tables, the 900 s cap); the native protocol (D1575) | #1008–#1046, #1055, #1030, #1041 (R9.1–R9.64) |
| 10 Dialect spike | `docs/plans/he0-dialect-spike-report.md`; the version pick; the front-end shape for HE1 | #999 (R10.1–R10.6) |
| 11 Iceberg v3 spike | `docs/plans/he0-iceberg-v3-spike-report.md`; the fork `ostrium-labs/iceberg-rust` with G2 (deletion-vector writer) and G3 (`RowDelta`) | #1004, #1012 (R11.1–R11.10) |
| 13–15 Queued gates | OpenMeter, Laminar (in place of Helicone), BI tools: stubs with licences, protocols and expected gaps; not started (owner's order: BI tools, OpenMeter, Laminar, after SigNoz) | #1042 (R13–R15) |

## 2. Measured numbers

### 2.1 The profile (Tasks 2–5)

| Application | Statements mined | sqlparser | `EXPLAIN AST` | Unparsed |
|---|---:|---:|---:|---:|
| HyperDX 2.40.0 | 79 | 40 | 39 | 0 |
| Langfuse v3 + v4 | 2 340 | 1 588 | 752 | 0 |
| OpenLIT 2.1.0 | 163 | 130 | 33 | 0 |
| OpenPanel 2.3.0 | 181 | 108 | 73 | 0 |
| Plausible CE 3.2.1 | 843 | 448 | 395 | 0 |
| SigNoz 0.145.0 | 1 635 | 1 155 | 480 | 0 |
| Umami 3.4.0 | 606 | 595 | 11 | 0 |
| **All** | **5 847** | **4 064** | **1 783** | **0** |

The profile has **440 features**. **337 are proposed for Tier 0**: 158 functions, 82 syntax forms, 50 settings, 29 types, 10 engines (with their `Replicated` forms), 4 formats and 4 combinators. 98 are proposed for Tier 1, 4 are refused (Tier 2) and 1 is disabled. sqlparser 0.63 reads 69.5 % of mined statements as they are; the rest need the server's `EXPLAIN AST`. That fraction is mostly DDL and `ALTER`, which the front reads with its own reader (R2.3, R9.12). `docs/house/compat-profile.md` lists 16 places where the mining and §75's tiers disagree. The ones that matter for HE1 are: materialized views, `AggregatingMergeTree` and skip indexes are used by 3 to 6 applications (Tier 1 in §75); `JSON` is used by SigNoz; and lightweight `DELETE` is used by Langfuse. D1579 below moves the gate applications' share of these up for the Rust engine.

### 2.2 The matrix (Task 8)

chDB 26.9.0 (ClickHouse 26.9.2.1) has 1 885 functions, 80 table engines and 93 table functions. Classified now: 273 functions `profile`, 145 `refused`, 28 `disabled` (1 439 unclassified, none of them mined); engines 14 / 12 / 49 / 5; table functions 0 / 0 / 78 / 15. CI fails on any unclassified **mined** feature (R8.4).

### 2.3 The oracle (Task 6)

The seed corpus is 20 cases (the House's `select/` corpus over 1 000 generated rows). They run on `clickhouse-server` 26.9.2.8 (baseline), 24.3.18.7 and `house-chdb`. **First triage: no mismatches** (R6.5). The scrubbed mined queries did **not** become oracle corpora (R6.4). Instead, the whole-traffic replay (`scripts/house/replay.py`, R9.3) became the per-application comparison, and it is what the OpenPanel gate runs. HE1 Task 23 adds per-task corpora and the mined corpora.

### 2.4 The gate applications on today's House (Task 9)

| Application | State on 2026-10-23 | Evidence |
|---|---|---|
| **OpenPanel 2.3.0** | **Passing, required CI gate** ("House apps: OpenPanel on the House", 5–8 min). Oracle drive: 332 events, 58 page reads, 0 errors. Replay: every DDL statement, 80 `SELECT`s identical, 2 differing only in tied order, none refused. OpenPanel itself on the House: migrations, ingest through its Redis buffers, 58 reads, 0 errors | R9.6, R9.7; #1055 |
| **Langfuse v4.54.0** | **Runs fully on a local House, no CI gate.** All 102 v4 migration statements apply over HTTP; OTLP and score ingest land and read back; all 16 main and detail pages render with 234 tRPC and public API calls and none fail. Native self-migration (golang-migrate) stops at `0022` until #1043 (open) merges | t9 "Remaining: Langfuse"; R9.40–R9.47 |
| Langfuse v3.225.11 | Mined, **never run on the House** | t9 item 3 |
| **SigNoz 0.145.0 (community)** | **Not passing.** 350 of 373 DDL statements apply; JSON columns (R9.63), nanosecond times (R9.61) and cluster `system` tables (R9.62) are served; refreshable materialized views are designed, not built; the gate job is not built | R9.16, R9.60–R9.64 |
| Native protocol | Gates 1 (clickhouse-go v2 and golang-migrate on v1, ports 9000 and 9440) and 4 (`clickhouse-client`) pass; LZ4/ZSTD compression is served. Gate 2 (Langfuse self-migration) waits for #1043; gate 3 (SigNoz) waits for the items above | R9.47, R9.48 |

### 2.5 The spikes (Tasks 10 and 11)

| Measure | Result |
|---|---|
| sqlparser 0.63 parse rate, hand-written 195-statement Tier 0 corpus | 77.9 % parse, 75.4 % faithful (`SELECT` 87.0 %, `CREATE` 46.4 %, `ALTER` 29.4 %); 0.62 one statement lower |
| DataFusion 54.1 planning, 20 representative queries | 6 correct, **5 silently wrong**, 9 refused; **20/20** with the spike's front end (90/112 of the corpus's `SELECT`s) |
| Silent drops in `SqlToRel` | `PREWHERE`, `GROUP BY … WITH TOTALS/ROLLUP/CUBE`, function parameters, `SAMPLE`, `FINAL` (parsed as an alias), function-name case |
| iceberg-rust 0.10.1 on Spark-written v3 tables | Fails loudly on deletion vectors (`Corrupt footer`), lineage columns and Variant metadata |
| iceberg-rust 0.11.0-rc.2 and `main` | Deletion vectors and `_row_id` / `_last_updated_sequence_number` correct on all tables, filters included; Variant columns refused |
| The fork's DV writer (G2) and `RowDelta` (G3) | Spark 4.0.4 (Iceberg 1.12.0), Trino 483, DuckDB 2.0.0 and iceberg-rust read the fork's commits identically in 8 cases |
| Interop baseline, today's House v2 tables | DuckDB 1.5.6, Spark 4.0.4 and Trino 483 give the House's answers |

### 2.6 Today's House engine costs, for comparison

chDB worker: 136.5 ms p50 from spawn to the first answer (1 068 ms with the 554 MB library cold); 554 MB library per image; no Windows build; cancellation only by killing the process (`docs/house/performance.md`, HS1). HE1 Task 46 measures the Rust worker against these figures.

## 3. Open items carried into HE1 and HE2

Every "remaining" list in the rulings is here, with the task that takes it.

| Item | Source | Taken by |
|---|---|---|
| The stateless manifest, the vendored tests with NOTICE, the chDB baseline | Task 7; branch `he0-t7-stateless` | HE1 Task 45 |
| Re-mine Umami, Plausible and HyperDX through `chlog`, then regenerate the profile and the matrix | Branch `he0-t5-chlog`; t9 Langfuse item 5 | HE1 Task 20 (Langfuse re-mined in Task 42) |
| The Langfuse gate job (`langfuse_on_house_smoke`, `compose.house.yaml`) | t9 Langfuse item 1 | HE1 Task 42 (chDB and Rust variants) |
| Langfuse v3 on the House | t9 Langfuse item 3 | HE1 Task 42 |
| `{columns:String}` / comments in a column list (Langfuse 0022 over native) | #1043 (open, agent L) | Agent L's PR; HE1 Task 42 checks it |
| A House flake under concurrent load (`36 metadata file … does not exist` after a pointer swap) | t9 Langfuse item 4 | HE1 Task 20 investigates; fix in its own PR |
| SigNoz: refreshable materialized views, the gate job, the multi-database replay, the private-network native listener | R9.64 | HE2 Task 65 (refreshable views on the Rust engine), HE1 Task 44 (replay and gate), HE2 Task 72 |
| Native protocol: DateTime time-zone names, `TableColumns`, `ProfileEvents`, `Totals`/`Extremes` packets, external tables, `Cancel` before the first result | R9.42, R9.47 | HE1 Task 36 (the engine sends typed headers and totals); the rest in HE5 on demand |
| Mutations rewrite whole tables | R9.5, R9.7 | HE2 Task 66 (deletion vectors) |
| Ties across files after a compaction or mutation | R9.2, R9.4, R9.7 | HE2 Task 67 (stable merges with sequence numbers kept) |
| AggregatingMergeTree is not merged on insert | R9.14, R9.7 | HE2 Task 64 |
| OpenPanel's imports, cohorts, groups and Search Console pages are not driven | R9.7 | HE1 Task 41 extends the driver |
| OpenPanel's views stay bound to a renamed table (upstream bug, not posted) | R4.4 | Needs the owner's consent to report |
| Recorded but never built: skip indexes, projections; `SELECT *` ignores `asterisk_include_*`; `SAMPLE` reads all rows; a `Null` table's rows stay in the bucket; `system.columns` is empty for plain views | t9 Langfuse item 7 | HE1 Tasks 25 and 38 (asterisk settings, view columns); skip indexes HE4; `Null` expiry HE2 Task 67 |
| `allow_dimensions_outside_sorting_key` is accepted, not enforced | R5.3, R9.5 | Kept: the House accepts what applications send (D1547 lists it) |
| DuckDB 1.5.x wrong `_row_id`: use DuckDB 2.0.0 in interop | R11.7 | HE2 Task 69 |
| iceberg-rust #3232 deadline, then our own upstream PR | R11.10 | 2026-10-28, under the owner's existing approval |
| The bucket-prefix bug: a House on `s3://bucket/prefix` is refused by `house-cache` (403) | R11.4 (S1) | HE1 Task 20 records it; fixed in its own PR |
| BI tools, OpenMeter and Laminar gates | Tasks 13–15 | Unchanged order, after SigNoz; on the chDB House first, then Rust (HE2 Task 72 runs whichever have started) |

## 4. Answers to §75's questions that HE0 was to inform

| Question | Owner's answer (2026-10-21) | What HE0 found |
|---|---|---|
| Q1170 (version range) | The latest 26.x stable, pinned as the oracle | The pin is `clickhouse-server` 26.9.2.8, the build of chDB's 26.9.2 line (R0.3); no image is tagged 26.9.2.1. Langfuse v4 needs ≥ 25.12; OpenPanel's and Umami's schemas need `allow_dimensions_outside_sorting_key` on ≥ 26.7 (R4.1, R5.3) |
| Q1175 (where and which versions) | `fabric/`, one Arrow | `fabric/` is right. The version set should move to Arrow 59 now (D1576, Q1188), because the fork's writer branches are on Arrow 59.2 |
| Q1180 (Langfuse line) | v4 the gate, v3 until 2027-01 | v4 reads `events_*` without `FINAL` and deletes with lightweight `DELETE`; v3 reads with `FINAL` on three tables (R3.2). Both are in HE1's Task 42 |
| Q1185 (applications) | The default list | Seven applications mined; PostHog was skipped for machine limits and OpenLIT taken instead (R5.1) |

## 5. Risks seen in HE0

- **Two engines behind one front (R8 in §75 §19) is now concrete.** The front relies on chDB for analysis as well as execution: `Classify`, the deny list's `EXPLAIN AST`, the read views, staging tables on insert, `DESCRIBE` and CTAS inference. The Rust engine replaces the read half in HE1 and the rest in HE2 (Tasks 39 and 63).
- **The gate applications reach past Tier 0.** OpenPanel reads `AggregateFunction` states that chDB wrote, so the Rust engine must decode ClickHouse's state serialization for the functions in use (D1800, Task 29). SigNoz needs `JSON` and refreshable views.
- **Semantic edge cases are the bulk of the work.** Type rules, alias scoping, default values in outer joins, `WITH TOTALS` placement and `uniq`/`quantile` algorithms will produce oracle mismatches that only careful porting removes. The per-task oracle (Task 23) catches them early.
- **The machine.** House stacks, oracle containers and DataFusion builds compete for one 15 GB development machine and a 91 %-full build disk. Tasks keep their builds to the engine crates and delete their test binaries.

## 6. Go / no-go

**Go**, with the HE1 task list, estimates and order in the [plan](2026-10-21-he1-loams-house-engine.md#he1--the-read-only-rust-engine-tasks-2047). HE1 starts with Tasks 20–23, the foundation that every other task builds on. chDB remains the shipped engine, labelled **Experimental**, until HE2's gate passes (D1542, D1568).
