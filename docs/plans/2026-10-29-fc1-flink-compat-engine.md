# FC1 — Loams Stream, the Rust Apache Flink-Compatible Engine — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Where this plan gives exact names, paths, flags or tests, use them verbatim. Record any deviation in the task's rulings file, `docs/plans/fc1-rulings/t<N>.md` in the Loams repository (indexed in "Rulings made during execution" at the end of this file).
>
> **Status: Approved** (owner, 2026-10-29: "approve all defaults, start building asap"). Track FC, design [§82](../design/82-loams-stream-flink-compatible-engine.md). FC0 (Tasks 0–9) and FC1 (Tasks 10–25) are specified task by task. **FC0 exited on 2026-10-29** ([report](fc0-exit-report.md)). FC2 and FC3 are planned at FC1's exit. Rulings made during execution take decision IDs from **D2280–D2299** and questions from **Q1451–Q1469**, the range §82 keeps free. **Nothing here is built yet.**

**Goal (FC0):** a standalone engine skeleton on the hard fork `ostrium-labs/streamfusion`, with its Flink-exact operators driven from Rust without Flink or JNI, a Flink SQL parser, and a differential harness that runs any corpus on Apache Flink 2.2.1 and on Loams Stream through the same SQL Gateway REST API. Exit, from §82 §16.1: the corpus runs on the oracle with 100 % of its own expectations; the parser accepts every statement in the corpus; Nexmark q0 runs end to end on the standalone engine.

**Goal (FC1, preview):** Flink SQL core, the SQL Gateway REST API v1–v4, the FC1 Flink REST paths, Kafka (Iggy gateway and external), Iggy native, Iceberg append, filesystem, aligned checkpoints and savepoints on S3, the compose profile `stream`, and `backend: loams` in `loams-flink`. Exit, from §82 §16.1: Nexmark q0–q22 match the oracle; at least 80 % of the in-scope test programs and `.q` files pass (the bar is re-set at FC0's exit); the SQL Client, the JDBC driver and `loams-flink`'s tests pass unchanged; kill -9 recovery is exactly-once into Iceberg and the filesystem.

**Architecture** (§82 §5.3, §7):
- **The fork, `ostrium-labs/streamfusion`** (`~/Documents/Ostriumlabs/streamfusion`, worktrees in `~/Documents/Ostriumlabs/streamfusion-wt/<branch>`, PRs to the fork's `main`). Its `native/` Cargo workspace holds StreamFusion's crates (`streamfusion` (engine), `streamfusion-bridge`, the format crates, `kafka`, `iceberg`, `parquet`, …) and the new crates `loams-stream-sql`, `loams-stream-plan`, `loams-stream-runtime`, `loams-stream-coord`, `loams-stream-rest`, `loams-stream-connectors`, `loams-stream-udf`, `loams-stream-conformance` and the binary crate `loams-stream`. The Java plugin (Maven, `src/`) stays and keeps building; it is the reference for how operators are parameterised.
- **The Loams repository** (`~/Documents/Ostriumlabs/loams-wt/<branch>`, PRs to `dev`): the rulings files, `backend: loams` in `crates/loams-flink`, the compose profile, the gateway route and the product-level conformance.
- **The oracle**: Apache Flink 2.2.1 with its SQL Gateway, in `dev/oracle/compose.yaml` in the fork (the image and connector jars §81 pins, D1931), reached only through the SQL Gateway REST API, exactly as Loams Stream is.

**Tech stack:**
- Rust 1.97.1 (Loams' toolchain), edition 2024 for new crates; DataFusion **54.1** and Arrow **58.4** (the root workspace's pins; StreamFusion is on 54.0.0 and 58.3.0); sqlparser 0.62.0, the version DataFusion 54.1 links (Task 0).
- Java 17 and Maven only for the fork's existing plugin, the TestProgram extractor (Task 6) and, later, the client jar.
- Podman with compose for the oracle; images pinned by digest, at least 14 days old.
- slatedb 0.17.0 for the spike only (Task 8).

**Spec:** [§82](../design/82-loams-stream-flink-compatible-engine.md) (all of it); [§81](../design/81-managed-stream-batch-engines.md) §8.3, §9, §10 and §14 (managed Flink, connectors, access, tests); `loams-wt/.he0-shared/engines-81.md` rulings R13 and R21–R26; the fork's `docs/operators/` and `docs/backends/` pages.

## Global Constraints

- **Worktrees and branches.** Fork work: `~/Documents/Ostriumlabs/streamfusion-wt/fc1-t<N>-<slug>` from `origin/main`, one branch and one PR per task into the fork's `main`. Loams work: `~/Documents/Ostriumlabs/loams-wt/fc1-t<N>-<slug>` from `origin/dev`, one PR per task into `dev`. Commit with `git commit -s`. Never `git stash`.
- **Hard fork (D2244).** No PR, issue, comment or post to `datafusion-contrib/StreamFusion`, `ArroyoSystems/arroyo`, `risingwavelabs/risingwave`, `apache/flink`, `apache/datafusion` or `sqlparser-rs`. Every file ported from Arroyo or RisingWave keeps its copyright lines and gets a row in the fork's `PORTS.md` (source repository, path, commit, licence).
- **Light builds.** Only through `~/Documents/Ostriumlabs/loams-wt/.he0-shared/buildslot.sh`, with `-j 4`, one package at a time (`cargo test -p loams-stream-sql -j 4`); never `--workspace` locally. The shared target comes from `~/Documents/.cargo/config.toml`: never set `CARGO_TARGET_DIR`, never build in `/tmp`. Delete your own test binaries and artefacts when done.
- **Licences (§82 §18).** Flink's test programs, `.q` files and OpenAPI files (Apache-2.0) may be vendored with attribution under `conformance/vendor/flink-2.2.1/`. TPC-DS query text is never committed (fetched at test time). No AGPL, BSL, FSL, SSPL or ELv2 code.
- **The JVM.** No JVM in the `loams-stream` binary or its image. The fork's JNI entry points stay behind the `jni` cargo feature of the `streamfusion` crate, so the standalone build links no `jni`.
- **Honest labels.** Everything is **preview**. Docs and PR descriptions say "not yet run end to end" until a test runs it. The compatibility page reports measured counts only.
- **Runners.** Per-PR CI runs the fork's existing jobs plus the new crates' unit tests. Corpus, differential and Nexmark runs are nightly and local (owner rule of 2026-10-28), never scheduled on GitHub.
- **Status.** Each task appends to `~/Documents/Ostriumlabs/loams-wt/.he0-shared/fc1-status.md`: what landed, the PR, what is next.

## Review Focus

1. **A result accepted as equal when Flink's differs.** Expected: never. The normaliser may sort rows and fold a changelog into its final upserted table only where the case declares itself order-insensitive. It never hides a row count, a type, a `RowKind` sequence the case marks significant, or a float's text form. Test: `normaliser_never_hides_rows_types_or_rowkinds`.
2. **A ported operator whose behaviour changed when moved off JNI.** Expected: none. Each operator's existing tests run unchanged through the Rust driver. Test: `ported_operator_tests_pass_without_jni`.
3. **A statement the parser accepts silently but the planner ignores.** Expected: either planned or rejected with D2268's error. Test: `every_parsed_statement_plans_or_names_feature`.
4. **A JVM dependency in the standalone binary.** Expected: none. Test: `standalone_graph_has_no_jni` (`cargo tree -p loams-stream -e normal` contains no `jni`).

## FC0 — Skeleton, operators without Flink, parser, oracle (Tasks 0–9)

### Task 0 — Rulings and versions (lead)

Record in `docs/plans/fc1-rulings/t0.md` in the Loams repository:
- The fork commit the work starts from (`e9d8e347977cae834d20a3599e7d1be86f610c3f`, 2026-10-28) and the commit `ostrium-labs/arroyo` is frozen at, once forked (D2244).
- The fork's CI workflows (`ci.yml`, `flink-suite.yml`, `flink-suite-line.yml`, `jvm-aggregates.yml`, `nested-scalars.yml`, `paimon1.yml`, `docs.yml`, `release.yml`, `fork-ci-status.yml`). Which ones stay per-PR, which become manual, and that none run on a schedule (owner rule).
- The sqlparser version DataFusion 54.1 links, which `loams-stream-sql` must use.
- Whether the fork's `main` has a ruleset that needs the owner's bypass to merge (tell the coordinator; never bypass it yourself).

Exit: the rulings file merged in the Loams repository with this plan's index updated. Effort: half a day.

### Task 1 — Version bump, toolchain and crate skeletons (fork)

- Bump `native/Cargo.toml`: `datafusion` 54.0.0 → 54.1.x, `arrow`/`arrow-*`/`parquet` 58.3 → 58.4.x. `object_store` stays at 0.13.2. Bump `rust-toolchain.toml` to `1.97.1` with `rustfmt` and `clippy`. Fix whatever breaks; record API changes in `docs/plans/fc1-rulings/t1.md`.
- Put the engine's JNI entry points behind a default-on cargo feature `jni` on the `streamfusion` crate (the Maven build keeps it on; `loams-stream` turns it off).
- Add the crates `loams-stream-sql`, `loams-stream-plan`, `loams-stream-runtime`, `loams-stream-coord`, `loams-stream-rest`, `loams-stream-connectors`, `loams-stream-udf` and `loams-stream-conformance`, and the binary crate `loams-stream` (`native/loams-stream/`), each with crate docs and an empty public API. They are edition 2024 with `license = "Apache-2.0"`.
- Add `PORTS.md` (empty table: source, path, commit, licence, ported to) and a `NOTICE` entry naming StreamFusion's authors, and later Arroyo and RisingWave as files arrive.
- `loams-stream` has `--role jobmanager|taskmanager|all` and prints its version; nothing else yet.

Tests: the fork's existing Rust tests pass on the new versions (`buildslot.sh cargo test -p streamfusion -j 4`, then the format crates one by one); `standalone_graph_has_no_jni`; `loams_stream_prints_version_and_roles`.

Exit: CI green on the fork; the Maven plugin still builds (`mvn -q -DskipTests package` once, recorded). Effort: 2–3 days. Commit: `native: DataFusion 54.1, Arrow 58.4, Rust 1.97.1 and the loams-stream crate skeletons (FC1 Task 1)`.

### Task 2 — Operators without Flink: the operator spec and the Rust driver (fork)

StreamFusion's Java matchers build each native operator through JNI calls with its parameters. Make those parameters a serialisable **operator spec**, the engine's physical IR (§82 §5.3).
- In `loams-stream-plan`, define `OpSpec` (serde, versioned): one variant per native operator in `native/engine/src` (`Calc`, `WindowAgg`, `SessionAgg`, `OverAgg`, `GroupAgg`, `UpdatingJoin`, `IntervalJoin`, `WindowJoin`, `TemporalJoin`, `LookupJoin`, `TopN`, `Dedup`, `ChangelogNormalize`, `SinkMaterializer`, `MiniBatch`, `Rowtime`/`WatermarkAssigner`, `Exchange`, `Union`, `Expand`, `Flatten`), with exactly the fields the JNI constructors take today (read them from `bridge.rs` and the Java matchers under `src/main/java/tech/streamfusion/planner/`).
- In `loams-stream-runtime`, a `Driver` that builds an operator from an `OpSpec` without JNI and feeds it Arrow batches (with the `RowKind` column), watermarks, timers and a key context. This is the contract the runtime will implement in full later: what Flink's operator chain gives a native operator today (current key, key group, timer service, checkpoint calls).
- Factor the engine's constructors so both the JNI path and the `Driver` call the same Rust function. Do not change operator code paths.

Tests: `ported_operator_tests_pass_without_jni` (the existing operator tests, re-run through the `Driver` for `Calc`, `WindowAgg` (tumble, hop, cumulate), `GroupAgg`, `TopN`, `Dedup`, `IntervalJoin`); `opspec_round_trips` (serde, for every variant); `driver_matches_jni_path` (same inputs through both paths give identical batches, run where the JVM is available, otherwise `#[ignore]` with a reason).

Exit: Nexmark q0 (a projection) and q5-like tumble-window aggregation run in the `Driver` from hand-written `OpSpec`s on generated Nexmark rows. Effort: 1 week.

### Task 3 — The Flink SQL parser (`loams-stream-sql`, fork)

- `FlinkDialect` on sqlparser (the version of Task 0), with statement-level hooks for what sqlparser lacks: Flink `CREATE TABLE` (physical, computed `AS expr` and metadata `METADATA [FROM '…'] [VIRTUAL]` columns, `WATERMARK FOR col AS expr`, `PRIMARY KEY (…) NOT ENFORCED`, `PARTITIONED BY`, `WITH ('k' = 'v', …)`, `LIKE … (INCLUDING|EXCLUDING …)`), `CREATE [TEMPORARY] VIEW`, `CREATE CATALOG`, `CREATE DATABASE`, `CREATE FUNCTION … AS '…' [LANGUAGE JAVA|PYTHON] [USING JAR '…']`, `DROP`/`ALTER` forms, `INSERT INTO|OVERWRITE`, `EXECUTE STATEMENT SET BEGIN … END;` and `BEGIN STATEMENT SET; … END;`, `SET 'k' = 'v'`, `RESET ['k']`, `USE [CATALOG]`, `SHOW …`, `DESCRIBE`/`DESC`, `EXPLAIN [PLAN FOR|CHANGELOG_MODE|…]`, `COMPILE PLAN`, `EXECUTE PLAN`, `CALL`, query hints `/*+ OPTIONS(…) */`, `LOOKUP(…)`, `STATE_TTL(…)`, table-valued calls `TABLE(TUMBLE|HOP|CUMULATE|SESSION(TABLE t [PARTITION BY …], DESCRIPTOR(ts), INTERVAL …))`, `FOR SYSTEM_TIME AS OF`, `MATCH_RECOGNIZE` (Flink's subset), Flink's interval literals (`INTERVAL '5' SECOND`, `INTERVAL '1' MINUTES` plural forms) and backquoted identifiers.
- The reference grammar is Flink 2.2.1's `flink-table/flink-sql-parser` (`parserImpls.ftl`, `Parser.tdd`); cite the production a hook mirrors in a comment.
- The AST is a Loams `Statement` enum wrapping sqlparser's where it fits; every node keeps its source span for D2268's errors.
- Unsupported syntax answers a `ParseError` naming the construct.

Tests: `parses_every_q_file_statement` (the 23 `.q` files of `flink-sql-client` and `flink-sql-gateway` at `release-2.2.1`, vendored under `native/loams-stream-sql/tests/vendor/flink-2.2.1/sql/` with Flink's licence header and a NOTICE row; statements Flink itself rejects in the `.q` expectations must be rejected too); `parses_nexmark_ddl_and_queries` (q0–q22 and their DDL); `round_trips_display` (parse → print → parse is stable); `rejects_with_named_feature` (Hive dialect, `ADD JAR`, and other not-built forms name the feature).

Exit: all of the above green; a report of statement counts by kind in the task's rulings file. Effort: 1–1.5 weeks.

### Task 4 — Binder and Flink's type system (fork)

- `loams-stream-sql::types`: Flink's logical types (§82 §4.1) with nullability, precision and scale; mapping to Arrow (record it in a table in the crate docs; `TIMESTAMP(p)` → `Timestamp(unit, None)`, `TIMESTAMP_LTZ(p)` → `Timestamp(unit, Some("UTC"))` with the session zone applied at the edges, `DECIMAL(p,s)` → `Decimal128`/`Decimal256`, `STRING` → `Utf8`, `MAP`/`MULTISET`/`ROW`/`ARRAY`).
- Flink's implicit coercion, `CAST` and `TRY_CAST` rules, decimal result precision rules, comparison and arithmetic typing (Flink's `LogicalTypeCasts`, `LogicalTypeMerging` at the pinned tag as the reference).
- The binder: catalog → database → object resolution with temporary objects shadowing permanent ones, `USE`, column resolution, the time attributes (`rowtime` from `WATERMARK`, `PROCTIME()`), and validation errors in Flink's wording where the `.q` files show it.
- Reuse StreamFusion's `flink_functions`, `flink_float`, `jdk_decimal` and `jdk_double` for value semantics; do not duplicate them.

Tests: `cast_matrix_matches_flink` (a generated table of every pair of logical types: allowed, explicit-only or refused, from the oracle via Task 5 once it lands, and checked into `tests/golden/cast_matrix.tsv`); `decimal_arithmetic_matches_flink`; `binder_errors_match_q_files`.

Exit: the binder types every query of the Task 3 corpus or rejects it with a named feature. Effort: 1.5 weeks.

### Task 5 — The differential harness against Flink 2.2.1 (fork)

- `dev/oracle/compose.yaml`: Flink 2.2.1 JobManager, one TaskManager and the SQL Gateway (the image digest and connector jars §81 pins, D1931), with checkpoints on a local RustFS bucket. Started and stopped by the harness, loopback only.
- `loams-stream-conformance`, a library and a CLI `loams-stream-oracle`:
  - a **SQL Gateway REST v4 client** (sessions, statements, operation status, paged JSON results, `STOP JOB … WITH SAVEPOINT`), target-agnostic: the same code drives Flink and, later, Loams Stream;
  - **cases**: a directory per case with `case.toml` (name, mode `streaming|batch`, `order_sensitive`, `changelog_significant`, timeout), `setup.sql` (DDL and source data, with sources only `datagen` with a fixed seed and row count, `filesystem` on files the case ships, or the bounded `values` connector), `query.sql`, and an optional `expected.jsonl`;
  - **runs**: each case on N targets; results collected from a `filesystem` sink in `debezium-json` (so `RowKind` survives) or from the gateway's result stream for bounded queries;
  - **the normaliser** (Review Focus 1): sort only when the case is order-insensitive; fold a changelog into its final table only when the case says the changelog is not significant; compare types and float text exactly;
  - **triage**: a `triage.jsonl` per run (case, targets, verdict `match|mismatch|error-both|error-one`, the first differing row) and a summary table.
- With Loams Stream not yet answering, the harness runs **oracle-only** mode: it runs a corpus on Flink and records `expected.jsonl`, which is how the corpus of Task 6 gets its expectations.

Tests: `normaliser_never_hides_rows_types_or_rowkinds` (property tests over generated result pairs); `client_speaks_v1_to_v4` (against recorded gateway responses in `tests/fixtures/`); `oracle_records_expectations` (an `#[ignore]` integration test that needs Podman: three cases recorded from the live oracle, re-run, and matched).

Exit: `loams-stream-oracle record dev/corpus/smoke` records 20 hand-written smoke cases (windows, joins, Top-N, dedup, changelog into upsert) from Flink 2.2.1 and a second run matches them all. Effort: 1–1.5 weeks.

### Task 6 — The corpus: Flink's test programs, `.q` files and Nexmark (fork)

- `dev/corpus-extractor/`: a Maven module that depends on Flink 2.2.1's `flink-table-planner` test jar and walks every `TestProgram` in the `*SemanticTests`, `*TestPrograms` and `*RestoreTest` classes of `org.apache.flink.table.planner.plan.nodes.exec.stream`, writing each into the Task 5 case format: sources (schemas and changelog rows, before and after the savepoint for restore programs), the SQL, the expected sink changelog. Restore programs become stop-with-savepoint-and-restore cases (`case.toml`: `restore = true`).
- `dev/corpus/flink-q/`: the `.q` files as cases (statement, expected output or error).
- `dev/corpus/nexmark/`: the Nexmark DDL and q0–q22 (Apache-2.0, `nexmark/nexmark`), with a seeded Rust generator (`loams-stream-conformance nexmark-gen --events N --seed S`) writing the three streams to files so both engines read identical inputs.
- Each case records the Flink feature it exercises (operator kinds from `EXPLAIN`'s plan on the oracle), so Task 7's profile can map features to cases.

Tests: `extractor_covers_every_program` (the count of exported programs equals the count found by reflection); `nexmark_gen_is_deterministic`.

Exit: the full corpus recorded on the oracle, 100 % of Flink's own expectations met by Flink (any program that fails on the oracle itself is listed in `dev/corpus/oracle-xfail.toml` with a reason). Effort: 1 week.

### Task 7 — The function and type matrix, and the scope manifest (fork)

- `loams-stream-oracle matrix`: run `SHOW FUNCTIONS` and `DESCRIBE FUNCTION EXTENDED` on the oracle; with the type table of Task 4, write `conformance/profile.toml`: every function, statement form, connector option and REST path marked `supported`, `planned = "FC1|FC2|FC3|FC4"` or `wont_do` with a reason.
- Generate `docs/compat/flink.md` from it (counts per layer, never one percentage), and the D2268 error messages (feature name, phase, link).

Tests: `profile_and_corpus_agree` (every corpus case's features exist in the profile; a case for a `wont_do` feature is an error).

Exit: the profile merged, with FC1's scope matching §82 §4.1's FC1 column. Effort: 3 days.

### Task 8 — Spike: slatedb as the keyed-state backend (report only)

On a branch that is not merged: one slatedb 0.17.0 database per subtask on RustFS, keys prefixed by key group, behind the fork's state-backend trait; run the `GroupAgg` and `UpdatingJoin` driver tests on it; measure get/put latency with and without a foyer cache, checkpoint time (manifest pointer versus RocksDB SST upload), and restore of a key-group range into a new database. Report in `docs/plans/fc1-rulings/t8.md`: does slatedb meet RocksDB's bar for FC2's default (D2252, Q1440)? Effort: 4 days.

### Task 9 — FC0 exit (lead)

Check §82 §16.1's FC0 gate. Re-set FC1's pass bar from the corpus's size and the profile. Write `docs/plans/fc0-exit-report.md` and update this plan's status.

## FC1 — Preview (Tasks 10–25)

Each FC1 task is one PR. The ordering constraint: 10 → 11 → 12 → 13 → 14; 15–16 need 12; 17–19 need 11 and 13; 20–21 need 15; 22–24 need everything before them.

| Task | Repo | What | Files | Tests | Gate |
|---|---|---|---|---|---|
| **10 Planner** | fork | Changelog-mode and time-attribute inference (Flink's rules: insert-only, upsert with key, retract); `ChangelogNormalize`, upsert materializer, watermark assigners, mini-batch, local-global aggregation; lowering to `OpSpec` graphs for every FC1 construct; `EXPLAIN` output in Flink's shape | `loams-stream-plan/src/{changelog,time,lower,explain}.rs` | `plans_every_fc1_case` (the corpus's FC1 cases lower or name a feature); `changelog_modes_match_flink_explain` (against `EXPLAIN CHANGELOG_MODE` recorded from the oracle) | All FC1 cases plan |
| **11 Runtime** | fork | Task managers: tasks, operator chains, task slots, key-group routing (`flink_key`), timers, watermarks, the network (ported from Arroyo's network manager: Arrow IPC between task managers, credit-based flow control), back pressure, metrics | `loams-stream-runtime/src/{task,chain,network,timers,watermarks,metrics}.rs`; `PORTS.md` rows | `two_taskmanagers_shuffle_by_key`; `back_pressure_reaches_source`; the Task 2 driver tests through real tasks | Nexmark q0–q8 match the oracle on 2 task managers, no checkpoints |
| **12 Coordinator** | fork | Job manager: job graph from the plan, scheduling to slots, job lifecycle (created, running, cancelling, finished, failed), restart strategies (`restart-strategy.*` exponential delay as §81), metadata in Postgres (`jobs`, `job_vertices`, `checkpoints`, `savepoints`, `sessions`), the leader lease | `loams-stream-coord/src/{graph,scheduler,lifecycle,store,lease}.rs`; `migrations/` | `job_lifecycle_state_machine`; `store_round_trips` (Postgres in a fresh database when `LOAMS_STREAM_TEST_PG_URL` is set) | A job survives a task-manager restart by full restart |
| **13 Checkpoints** | fork | Barriers and alignment, the checkpoint coordinator (ported from Arroyo where it fits), RocksDB incremental SSTs to S3 (`engines/<ns>/<id>/state/`), `_metadata` objects, recovery from the latest completed checkpoint, `execution.checkpointing.*` options; the TLA+ model `spec/tla/StreamCheckpoint.tla` | `loams-stream-runtime/src/barrier.rs`, `loams-stream-coord/src/checkpoint.rs`, `streamfusion` state backend glue | `aligned_barrier_snapshot_is_consistent`; `recovery_replays_from_offsets`; TLC on the model (safety: no output committed twice; liveness under fair scheduling) | Kill -9 of a task manager during Nexmark q5: final results equal the failure-free run |
| **14 Savepoints and rescale** | fork | Canonical savepoints (Arrow IPC sections per operator and key group, §82 D2254), native savepoints, `STOP JOB … WITH SAVEPOINT`, `execution.state-recovery.path`, restore at a new parallelism | `loams-stream-coord/src/savepoint.rs`, `loams-stream-runtime/src/restore.rs` | The corpus's restore cases; `rescale_2_to_3_preserves_results` | All restore cases pass |
| **15 SQL Gateway REST** | fork | v1–v4 in full (§82 §4.2): sessions, statements, operations, results paging, `complete-statement`, `configure-session`, `scripts`, heartbeat; error bodies as Flink's | `loams-stream-rest/src/gateway/` | Schema tests generated from the vendored `rest_v{1..4}_sql_gateway.yml`; the harness of Task 5 drives Loams Stream | The smoke corpus matches through the REST API |
| **16 Flink REST (FC1 paths)** | fork | The FC1 dispatcher paths of §82 §4.2; 501 with the D2268 body for every other path in `rest_v1_dispatcher.yml` | `loams-stream-rest/src/dispatcher/` | `every_dispatcher_path_answers_or_501` (from the OpenAPI file); `cancel_stop_savepoint_round_trip` | `loams-flink`'s `rest.rs` tests pass against it |
| **17 Kafka and Iggy** | fork | Kafka source and sink with Flink's option names (StreamFusion's format code; consumption and production in Rust, `rdkafka` or the crate StreamFusion's Kafka path uses, Task 0 records which), transactional two-phase sink for external Kafka (ported from Arroyo), at-least-once through the Iggy gateway (D1915), `upsert-kafka`; Iggy native source and sink with deterministic message ids (D2277, verify Iggy's deduplication first) | `loams-stream-connectors/src/{kafka,iggy}/` | `kafka_exactly_once_under_kill` (external Kafka in a container); `iggy_dedup_suppresses_replays` | Nexmark from Kafka through the Iggy gateway matches the oracle |
| **18 Iceberg and filesystem** | fork | Iceberg append sink on Lakekeeper (catalog `loams`, D1913) committing once per checkpoint with the checkpoint id in the snapshot summary; incremental Iceberg source; filesystem source and two-phase sink (pending → committed, partition commit) | `loams-stream-connectors/src/{iceberg,filesystem}/` | `iceberg_commit_is_idempotent_on_recovery`; `filesystem_sink_exactly_once_under_kill`; a House read of the written table (cross-engine, nightly) | Kill -9 recovery exactly-once into both |
| **19 Catalogs and session statements** | fork | `default_catalog` in memory, the `loams` Iceberg REST catalog, `SET`/`RESET`/`USE`/`SHOW`/`DESCRIBE`, statement sets, the protected keys of D1977 | `loams-stream-plan/src/catalog/`, `loams-stream-rest/src/session.rs` | The `.q` cases for catalogs, `set`, `statement_set`, `table`, `view` | Those `.q` files pass |
| **20 Binary, image, compose** | fork, Loams | `loams-stream --role` wiring; the image `ghcr.io/ostrium-labs/loams-stream` from the fork's `release.yml` (preview tag); in Loams, the `stream` profile in `deploy/release/compose.flink.yaml` (role `all`, Postgres, RustFS, loopback 8083 through `engine-gateway`) | fork `native/loams-stream/`, `.github/workflows/release.yml`; Loams `deploy/release/compose.flink.yaml` | `compose_stream_profile_renders` (Loams golden); idle memory measured and recorded | The SQL Client in gateway mode runs the smoke corpus against compose |
| **21 `backend: loams` in `loams-flink`** | Loams | The managed Flink kind gets `backend: apache\|loams` (D2264) in the API, CLI and adapter; k8s objects for JobManager and TaskManager Deployments; gateway hostname routing unchanged | `crates/loams-flink/src/{engine,k8s,controller}.rs`, `api/engines/openapi.json` | `loams-flink`'s whole test suite parameterised by backend; k8s goldens | `loams-flink` tests pass for both backends |
| **22 Conformance wiring** | fork, Loams | The nightly local run: corpus, `.q` files and differential on both engines; strict `xfail.toml` (unexpected pass fails); the compatibility page regenerated; in Loams, `conformance/stream/` runs the product-level cases through the gateway | fork `dev/nightly.sh`; Loams `conformance/stream/` | The nightly itself | Three consecutive nightlies without an unexplained mismatch |
| **23 Nexmark and performance** | fork | q0–q22 on both engines from identical inputs; throughput per core, p99 latency, recovery time and idle memory, in the quiet window (`.he0-shared/quiet`), results in `docs/benchmarks/` of the fork, internal | `dev/bench/nexmark.sh` | — | q0–q22 match; numbers recorded (targets are §82 §13's estimates, not gates in FC1) |
| **24 Chaos** | fork | Random kill -9 of task managers and the job manager, network partitions (RustFS and Postgres), over the golden queries; committed output compared per connector guarantee | `dev/chaos/` | The suite | Green for 3 nights |
| **25 FC1 exit** | Loams | Check §82 §16.1's FC1 gate; labels; `docs/plans/fc1-exit-report.md`; plan FC2 | — | — | Owner review |

## Starting order (2026-10-29)

Three implementers at once (owner cap):
1. **Task 1** (version bump and skeletons), first, because Tasks 2–4 build on its crates.
2. **Task 3** (parser) in parallel. It adds `loams-stream-sql` to the workspace itself if Task 1 has not merged yet, then rebases (the `members` list is the only overlap).
3. **Task 5** (differential harness) in parallel. It adds `loams-stream-conformance` the same way.

Then Task 2 after Task 1, Task 4 after Task 3, Task 6 after Task 5, and Tasks 7–9.

## Rulings made during execution

| Task | File | Summary |
|---|---|---|
| 0 | [fc1-rulings/t0.md](fc1-rulings/t0.md) | Fork base commit; pins DataFusion 54.1.0, Arrow 58.4.0, sqlparser 0.62.0, Rust 1.97.1; the fork's CI loses its schedules; merges by the coordinator |
| 1 | [fc1-rulings/t1.md](fc1-rulings/t1.md) | Pins applied; the `jni` feature and uninhabited JVM objects; crate skeletons; CI trimmed to Rust tests and the Flink 2.2 Java build |
| 3 | [fc1-rulings/t3.md](fc1-rulings/t3.md) | sqlparser 0.62.0; named refusals (`ADD JAR`, `WITHIN`, SQL/JSON clauses, materialized tables, models, Hive) |
| 4 | [fc1-rulings/t4.md](fc1-rulings/t4.md) | CAST follows the SQL validator plus runtime rules; Arrow mapping follows the engine's transport; inherited lints allowed per crate; permissive binder |
| 5 | [fc1-rulings/t5.md](fc1-rulings/t5.md) | Deterministic sources only; debezium changelog folding; the wire's `isQueryResult` |
| 6 | [fc1-rulings/t6.md](fc1-rulings/t6.md) | 345 programs, 330 cases; Beam-style Nexmark seeding; the restore flow; the self-check; executable `.q` cases deferred to Task 22 |
| 8 | [fc1-rulings/t8.md](fc1-rulings/t8.md) | slatedb spike: zero-copy rescale and pointer checkpoints, slower naive writes; RocksDB stays FC1's backend, re-measure on S3 in FC2 |
| 9 | [fc0-exit-report.md](fc0-exit-report.md) | FC0 gate met; the FC1 bar is at least 144 of the 180 in-scope recorded test programs, plus Nexmark |
| 10 | [fc1-rulings/t10.md](fc1-rulings/t10.md) | No JVM fallback in the expression port; Loams engine kernels from op 900; UTC session zone; string and timestamp comparisons admitted; local executor until Task 11; the 180-case bar; legacy group windows in FC1's bar |
