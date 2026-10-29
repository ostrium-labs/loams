# 82 — The Loams Stream Engine: Rust, Apache Flink-Compatible

Status: **Approved** (owner, 2026-10-29: "approve all defaults, start building asap"). Decisions **D2240–D2279** and questions **Q1430–Q1450** are in the [decision log](13-decision-log.md) with status **Approved**; every question is answered with its default (§20). D2280–D2299 and Q1451–Q1469 are kept free for track FC's plan rulings. Plan: [FC1](../plans/2026-10-29-fc1-flink-compat-engine.md). **No engine code is written by this document.**

Source: the owner, 2026-10-29: "like iggy for kafka, event fabric for trino, i want one for flink, full api compat but in rust", and earlier the same day: "fork rising wave latest apache 2.0 and strip components we do not need, or find alternatives like arroyo, fluss etc".

**The pattern.** Loams runs its own Rust engines that speak another system's API: the Iggy fork speaks the Kafka wire protocol (D1797), and Loams House speaks ClickHouse on DataFusion and Iceberg v3 with ClickHouse as the oracle ([§75](75-loams-house-engine.md)). This document applies the House's method to Apache Flink. (The owner's "event fabric for trino" has no counterpart in the repository: Loams Event Fabric is Iggy plus Fluss, D331, and nothing in Loams speaks Trino's protocol. The pattern, not that example, is what this design follows.)

**What it changes.** Design 81 runs real Apache Flink 2.2.1 per namespace through `loams-flink` (D1900, D1909). This document adds a second backend under the same managed-Flink product: a Rust engine that accepts Flink SQL, the Flink SQL Gateway REST API and the Flink REST API, and later the Table API, PyFlink and DataStream programs. Real Flink 2.2.1 stays as the **oracle** for every test and as the **fallback backend** for anything the Rust engine does not yet run. §17 lists what this supersedes; the main ones are:

- **D1261** and **Q1036's answer** ("no native continuous-query engine now"; L4 only behind a gate) are **superseded** by the owner's request: the L4 engine is built now, on a hard fork of StreamFusion, with runtime pieces ported from Arroyo (as §68 §6.5 anticipated) (D2240, D2243).
- **D1272**'s "no stream processor before Q1036" and **D1930**'s "no Loams fork of any engine" are **amended**: StreamFusion is hard-forked as the base, and Arroyo is a donor ported file by file (D2244).
- **§00 §7** ("Loams is not a stream processor") is **amended**: Loams ships a Flink-compatible engine; it still does not invent its own stream-processing dialect.
- **D212**'s operator path for DataStream jobs is **amended at FC3** (D2261).

Markers, as in §75 and §81:
- **(verify)**: not checked against a primary source; the task that depends on it checks it first.
- **(estimate)**: judged, not measured.
- **(checked 2026-10-29)**: read in a primary source on that date (§21).

Pins follow D1274: releases at least 14 days old when the PR opens.

---

## 1. Summary

**Loams Stream is a Rust stream processor that speaks Apache Flink's APIs.** Flink SQL scripts, the Flink SQL Client in gateway mode, the Flink JDBC driver, and tools that drive the Flink REST API run against it unchanged. State and checkpoints live in the customer's bucket, metadata in Loams Postgres. It sits behind `loams-gateway` as a backend of design 81's managed Flink. Real Flink 2.2.1 defines "correct" for every test. Loams does not fork Flink. Its base is a **hard fork of StreamFusion** (`ostrium-labs/streamfusion`, Apache-2.0): Rust operators on DataFusion 54 and Arrow 58, the same line as Loams' root workspace, that already reproduce Flink SQL's results exactly. StreamFusion runs inside Flink today. Loams adds the parts that make it a standalone engine: a Flink SQL front end, a scheduler, a network layer and a checkpoint coordinator (ported from Arroyo where they fit), both Flink REST surfaces, and the control plane.

### 1.1 Decisions

| # | Decision (short) | § | Status |
|---|---|---|---|
| D2240 | **What Loams Stream is**: a Rust engine with Flink's SQL, REST and (in phases) Table API, PyFlink and DataStream surfaces, state in the bucket, behind `loams-gateway` as a backend of §81's managed Flink. Supersedes D1261 and Q1036's answer; amends §00 §7 | 1, 6 | Approved (owner, 2026-10-29) |
| D2241 | **"Full Flink API compatibility" is six layers** (SQL, REST, Table API, DataStream, connectors, checkpoints), each with a measurable bar against the oracle; the claim names a layer, a phase and a Flink version, never "100 % Flink" | 3 | Approved (owner, 2026-10-29) |
| D2242 | **The oracle is Apache Flink 2.2.1**, the same pin as D1931, run through `loams-flink`; it moves only when D1931's pin moves | 3, 15 | Approved (owner, 2026-10-29) |
| D2243 | **Base: a hard fork of StreamFusion** (`ostrium-labs/streamfusion`). Its Rust operators, Flink expression semantics, state backends and formats, on DataFusion 54 and Arrow 58, are the core. Loams adds the standalone runtime, the Flink SQL front end, the REST surfaces and the control plane. Runtime machinery (network, barriers, checkpoint coordination, Kafka two-phase commit) is ported from **Arroyo** where it fits. RisingWave donates specific algorithms only. Fluss is not an engine and is not used | 5 | Approved (owner, 2026-10-29) |
| D2244 | **Hard-fork rules**: `ostrium-labs/streamfusion` (forked 2026-10-29) is Loams-owned with no upstream sync, PRs or posts (as the Neon and Iggy forks); Arroyo and RisingWave code is ported file by file with attribution and listed in `PORTS.md`; NOTICE carries every attribution. Amends D1930 and D1272 | 5.5 | Approved (owner, 2026-10-29) |
| D2245 | **Where the code lives**: the engine in the fork's `native/` Cargo workspace (bumped from DataFusion 54.0 and Arrow 58.3 to the root workspace's 54.1 and 58.4, and to Loams' toolchain), released from the fork as the `loams-stream` image; the product integration (`loams-flink` backend, compose, gateway, product conformance) in the Loams repository; no DataFusion fork; sqlparser from crates.io with Loams' own `FlinkDialect` | 7.6 | Approved (owner, 2026-10-29) |
| D2246 | **Crates**: StreamFusion's own crates (renamed only where Loams needs to), plus new `loams-stream-{sql,plan,runtime,coord,rest,connectors,udf,conformance}` and the binary `loams-stream`, all in the fork | 7.6 | Approved (owner, 2026-10-29) |
| D2247 | **The SQL layer (L1)**: Flink 2.2's DDL, DML, queries, `SET`, statement sets, catalogs, functions and `EXPLAIN`, with the window TVFs, joins, `MATCH_RECOGNIZE` and changelog semantics of §4.1, phased by §16 | 4.1 | Approved (owner, 2026-10-29) |
| D2248 | **The planner**: Flink SQL → a Flink-typed, validated tree (Flink's type and coercion rules, not DataFusion's) → a streaming logical plan with Flink's changelog and time-attribute inference → StreamFusion's operator descriptions (the physical IR) with expressions in Flink semantics; built-in functions from a generated matrix | 7.1 | Approved (owner, 2026-10-29) |
| D2249 | **Changelog model**: Flink's `RowKind` (`+I`, `-U`, `+U`, `-D`) as an Arrow column on every batch; Flink's changelog-mode inference (insert-only, upsert, retract), `ChangelogNormalize` and the upsert materializer at sinks | 7.2 | Approved (owner, 2026-10-29) |
| D2250 | **Time model**: event time with per-split watermarks, idleness and alignment; processing time; `TIMESTAMP_LTZ` and `table.local-time-zone`; late rows handled as Flink handles them | 7.2 | Approved (owner, 2026-10-29) |
| D2251 | **Runtime**: Arrow record batches with a row-kind column between operators, chaining, task slots, credit-based back pressure, a data plane over Arrow IPC streams (ported from Arroyo's network manager), mini-batch | 7.3 | Approved (owner, 2026-10-29) |
| D2252 | **State**: StreamFusion's state-backend model with three backends: memory and RocksDB (StreamFusion's own, with incremental SSTs uploaded to S3) in FC1, and **slatedb** (an LSM on object storage), which becomes the default if FC0's spike meets the bar; key groups as Flink defines them; a foyer cache through `loams-cache`; `table.exec.state.ttl` honoured | 7.4 | Approved (owner, 2026-10-29) |
| D2253 | **Checkpoints**: aligned barriers, exactly-once, incremental, every 60 s by default (as D1910), retained on cancel; checkpoint metadata in Loams Postgres and a `_metadata` object on S3 | 7.5 | Approved (owner, 2026-10-29) |
| D2254 | **Savepoints**: a canonical, version- and parallelism-portable format (each operator's logical state as Arrow IPC sections per key group, StreamFusion's representation, plus metadata); native savepoints are the backend's own checkpoint; Flink's own savepoints are not read (state migration is Q1438) | 7.5 | Approved (owner, 2026-10-29) |
| D2255 | **Rescaling** by key groups (Flink's max-parallelism rule): stop with a savepoint and restore at a new parallelism in FC1; in-place rescaling in FC2 | 7.5 | Approved (owner, 2026-10-29) |
| D2256 | **Exactly-once sinks** use a two-phase commit (pre-commit at the barrier, commit when the checkpoint completes, re-commit pending transactions on recovery); the guarantee per connector is §4.5's table, stated as such | 4.5, 7.5 | Approved (owner, 2026-10-29) |
| D2257 | **Connectors** keep Flink's connector names, option keys and formats: Kafka (Iggy's gateway and external Kafka), Iggy native, Iceberg on Lakekeeper, Postgres CDC, JDBC, filesystem, plus `datagen`, `print`, `blackhole` | 4.5 | Approved (owner, 2026-10-29) |
| D2258 | **REST**: the SQL Gateway REST API v1–v4 in full, and the Flink REST API subset of §4.2 (jobs, cancel, stop, savepoints, checkpoints, overview, task managers, jars), tested from Flink's own OpenAPI files; the stock Flink web dashboard works read-only in FC2 | 4.2 | Approved (owner, 2026-10-29) |
| D2259 | **Table API and PyFlink**: a client-side jar (`loams-stream-client`, `execution.target: loams`) that sends SQL or Flink's compiled plan JSON (FLIP-190) to the engine; the engine reads ExecNode JSON as a second front end. The JVM runs only in the client | 4.3, 7.1 | Approved (owner, 2026-10-29) |
| D2260 | **User functions**: Java UDFs in a JVM UDF host process and Python UDFs in a Python worker, both exchanging Arrow batches with the Rust operator; Rust and Wasm UDFs natively; user code only under §81's sandbox rule (D1905) | 4.3, 10 | Approved (owner, 2026-10-29) |
| D2261 | **DataStream (honest)**: arbitrary Java jobs need a JVM. FC3 runs user functions in an embedded JVM operator host while state, timers, checkpoints and shuffle stay in Rust; a Rust DataStream-style API ships in FC3 too. Until then DataStream and JAR jobs run on Apache Flink (§81). Amends D212 at FC3 | 4.4 | Approved (owner, 2026-10-29) |
| D2262 | **Catalogs**: Flink's catalog semantics; a persistent catalog store in Loams Postgres; `CREATE CATALOG` types `generic_in_memory`, `iceberg` (REST, Lakekeeper, named `loams` as D1913) and `jdbc` (`loams_pg`); `hive` refused | 4.1, 9 | Approved (owner, 2026-10-29) |
| D2263 | **Deployment**: one binary `loams-stream` with roles `jobmanager`, `taskmanager` and `all`; optional sidecar images for the JVM and Python UDF hosts; a compose profile `stream` (role `all`); Kubernetes objects rendered by `loams-flink` for `backend: loams`; BYOC as §81 | 8 | Approved (owner, 2026-10-29) |
| D2264 | **Coexistence with §81**: the managed Flink kind gets `backend: apache \| loams`; `apache` stays the default until FC3's gate, then the owner decides the flip (Q1437); no automatic fallback routing | 6 | Approved (owner, 2026-10-29) |
| D2265 | **Metadata and HA**: jobs, checkpoints, savepoints, sessions and the catalog store in Loams Postgres; JobManager leadership by a Postgres lease; recovery from the latest completed checkpoint in compose and k8s alike | 8.3 | Approved (owner, 2026-10-29) |
| D2266 | **Access and tenancy**: §81 unchanged — hostname routes in `loams-gateway` (D1918), OpenFGA `engine` relations (D1919), scoped service credentials (D1917), one instance per namespace (D1903, D1904), never public | 10 | Approved (owner, 2026-10-29) |
| D2267 | **Metering**: D1920's `engine.runtime.v1` and `engine.storage.v1` with `backend = "loams"`; the Meter gate also on job submit | 11 | Approved (owner, 2026-10-29) |
| D2268 | **Unsupported features fail loudly**: a REST error shaped like Flink's (exception class and message) that names the feature, the phase that adds it and the profile page; only options listed as accepted no-ops are ignored | 3.3 | Approved (owner, 2026-10-29) |
| D2269 | **Conformance**: eight suites (§15): Flink's own test programs, the SQL Client and Gateway `.q` files, differential runs against Flink 2.2.1, golden queries, REST contract and tooling, exactly-once and recovery, connector interop, performance | 15 | Approved (owner, 2026-10-29) |
| D2270 | **Performance**: Nexmark q0–q22 throughput per core, p99 latency, recovery time and state size against Flink 2.2.1 on the same machine in a quiet window; targets are estimates until measured; TPC-DS text fetched at test time | 15.3 | Approved (owner, 2026-10-29) |
| D2271 | **Phases FC0–FC4** (track FC): FC0 harness, oracle, fork bump and skeletons; FC1 preview (SQL core, REST, Kafka/Iggy/Iceberg/filesystem, checkpoints); FC2 beta (temporal, lookup, `MATCH_RECOGNIZE`, CDC, JDBC, catalogs, Table API, UDFs, HA, rescaling); FC3 GA (DataStream, JARs, state migration, default flip); FC4 on demand | 16 | Approved (owner, 2026-10-29) |
| D2272 | **Honest labels**: preview, then beta, then GA, per layer; a compatibility page generated from tests names the Flink version and pass counts per layer | 16.3 | Approved (owner, 2026-10-29) |
| D2273 | **Licences and trademarks**: Flink's test programs, `.q` files and OpenAPI files (Apache-2.0) vendored with NOTICE; Arroyo (Apache-2.0 and MIT) and RisingWave (Apache-2.0) code ported with attribution; no Feldera enterprise code; TPC-DS fetched at test time; the product is "Flink-compatible", never named Flink | 18 | Approved (owner, 2026-10-29) |
| D2274 | **Not built**: Hive dialect and catalog, the HiveServer2 endpoint, the Scala API, Flink ML, Gelly, the Flink Kubernetes Operator's CRDs, Arroyo's own SQL dialect, API and web UI | 16.4 | Approved (owner, 2026-10-29) |
| D2275 | **Batch mode**: `execution.runtime-mode = batch` runs on DataFusion's batch executor in FC2; FC1 runs bounded sources in streaming mode | 4.1 | Approved (owner, 2026-10-29) |
| D2276 | **No JVM planner in production**: Flink's own planner runs only in tests (the oracle and the ExecNode-JSON differential) and in the client jar; the engine's default SQL path is Rust end to end | 7.1 | Approved (owner, 2026-10-29) |
| D2277 | **Exactly-once into Iggy**: Iggy native sinks write with deterministic message ids and Iggy's deduplication (verify) in FC1; Kafka-gateway sinks are at-least-once (D1915) until the Iggy fork has Kafka transactions, then two-phase commit | 4.5 | Approved (owner, 2026-10-29) |
| D2278 | **Postgres CDC** is a native Rust pgoutput source with Flink CDC's `postgres-cdc` option names, slots and publications created as D1916 says; snapshot then stream, offsets in checkpoints | 4.5 | Approved (owner, 2026-10-29) |
| D2279 | **Track FC**, plan FC1; status file `loams-wt/.he0-shared/fc1-status.md` | 16 | Approved (owner, 2026-10-29) |

## 2. Goals and non-goals

### 2.1 Goals

- **Run existing Flink SQL pipelines unchanged**, submitted the ways Flink users submit them: the SQL Client in gateway mode, the SQL Gateway REST API, the Flink JDBC driver, `loams-flink`'s controller and API.
- **Drive jobs with existing Flink tooling**: list, cancel, stop with savepoint, trigger savepoints, read checkpoints through the Flink REST API.
- **Reach the rest of Flink's API in phases**: Table API (Java and PyFlink), Java and Python UDFs, then DataStream programs.
- **State and checkpoints in the bucket, exactly-once where the sink allows it**, with the guarantee stated per connector.
- **A lighter, faster engine than JVM Flink on Loams' own streams and tables**: no JVM in the runtime, a memory floor small enough for a namespace on the light stack, and throughput per core at least Flink's on Nexmark (a target, §15.3).

### 2.2 Non-goals

- **Reading Flink's own savepoints** in place. Migration is by source offsets (FC2) and, for SQL operators, by an exporter (FC3, Q1438).
- **Running unmodified Flink JVM runtime code** (JobManager, TaskManager, RocksDB backend) in the Rust engine. The JVM appears only to host user code (D2260, D2261) and in the client jar (D2259).
- **A new dialect.** Loams Stream speaks Flink's dialect only. Arroyo's SQL dialect, its REST API and its web UI are not exposed (D2274).
- **Replacing RisingWave.** RisingWave stays the Postgres-wire streaming database (D1260, §81); Loams Stream serves Flink users.

## 3. What "full Flink API compatibility" means (D2241, D2242, D2268)

### 3.1 The six layers

| Layer | What a user does | Measurable bar | First phase |
|---|---|---|---|
| **L1 Flink SQL** | DDL with `WITH` options, watermarks, window TVFs, joins, `MATCH_RECOGNIZE`, changelog and upsert, catalogs, `SET`, statement sets | (a) Flink 2.2.1's semantic and restore test programs in scope pass with identical changelogs; (b) the SQL Client and Gateway `.q` files in scope pass; (c) the differential suite finds no unexplained mismatch for 14 consecutive nightly runs; (d) Nexmark q0–q22 give the oracle's results | FC1 (core), FC2 (rest) |
| **L2 REST** | SQL Gateway REST v1–v4; the Flink REST API (jobs, savepoints, cancel) | (a) Contract tests generated from Flink's `rest_v1_dispatcher.yml` and `rest_v{1..4}_sql_gateway.yml` pass for every path in scope; (b) the stock SQL Client (gateway mode), the Flink JDBC driver and `loams-flink`'s own tests pass unchanged; (c) the Flink web dashboard renders jobs, checkpoints and exceptions (FC2) | FC1 (gateway and job paths), FC2 (dashboard) |
| **L3 Table API** | Java Table API and PyFlink Table API programs, Java and Python UDFs | Flink's Table API examples and the PyFlink Table API test subset run through the client jar with the oracle's results | FC2 |
| **L4 DataStream** | Java DataStream programs and JARs | A corpus of open DataStream jobs (Flink's examples, the training exercises, Flink CDC pipelines in DataStream form) gives the oracle's results, including after a kill and recovery | FC3 |
| **L5 Connectors** | Kafka, Iceberg, Postgres CDC, JDBC, filesystem with Flink's option names | Per connector: Flink's documented options in scope accepted; round trips with Flink, Spark, the House and RisingWave; the guarantee of §4.5 shown under failure injection | FC1 (Kafka, Iggy, Iceberg append, filesystem), FC2 (CDC, JDBC, Iceberg upsert) |
| **L6 Checkpoints and savepoints** | Exactly-once, stop with savepoint, restore, rescale | Kill -9 at random points during Nexmark and the golden queries; the sink's committed output equals the failure-free output (exactly-once sinks) or contains it (at-least-once sinks) | FC1 |

"Full compatibility" is the union of the six bars at GA. Each layer's status is published separately (D2272).

### 3.2 The oracle (D2242)

Apache Flink **2.2.1**, the image §81 already pins (D1931, D1970), run as a managed instance through `loams-flink` or as a test container. Every test input goes to both engines; Flink's output defines the expected result. The oracle version moves only with D1931 (Q1433). Flink 2.3.0 exists (tag committed 2026-06-14, checked 2026-10-29); the claim says "Flink 2.2" until the oracle moves.

### 3.3 Unsupported features (D2268)

An unsupported statement, function, option or REST path answers with Flink's error shape (for SQL, a `ValidationException`-style error in the SQL Gateway's error list; for REST, `{"errors": [...]}` with HTTP 501 for an unimplemented path) whose message names the feature, the phase that adds it and the compatibility page. Options Flink accepts that have no meaning in Loams Stream (for example `state.backend.type: rocksdb`, `taskmanager.memory.*`) are listed as accepted no-ops in the profile file; nothing else is silently ignored.

## 4. The surfaces in detail

### 4.1 L1: Flink SQL (D2247, D2262, D2275)

| Area | FC1 (preview) | FC2 (beta) | FC3+ |
|---|---|---|---|
| DDL | `CREATE/DROP/ALTER TABLE` with `WITH` options, physical, computed and metadata columns (`METADATA FROM … VIRTUAL`), `WATERMARK FOR … AS …`, `PRIMARY KEY … NOT ENFORCED`, `PARTITIONED BY`, `LIKE`; `CREATE VIEW`, `CREATE TEMPORARY TABLE` | `CREATE TABLE AS`, `REPLACE TABLE AS`, `CREATE CATALOG`, `CREATE DATABASE`, `CREATE FUNCTION … LANGUAGE JAVA\|PYTHON USING JAR`, `ALTER TABLE` forms | Materialized tables (FLIP-435), `CREATE MODEL` (on demand) |
| DML | `INSERT INTO`, `INSERT OVERWRITE` (filesystem), `EXECUTE STATEMENT SET BEGIN … END`, `SET`, `RESET`, `USE`, `SHOW`, `DESCRIBE`, `EXPLAIN` | `COMPILE PLAN`, `EXECUTE PLAN`, `CALL`, `TRUNCATE`, `DELETE`, `UPDATE` (batch, on sinks that allow it) | |
| Hints | `/*+ OPTIONS(...) */` | `LOOKUP`, join hints, `STATE_TTL` | |
| Windows | `TUMBLE`, `HOP`, `CUMULATE` TVFs (`TABLE(TUMBLE(TABLE t, DESCRIPTOR(ts), INTERVAL …))`) with window aggregation, window Top-N, window deduplication, window join | `SESSION` TVF (with `PARTITION BY`), legacy group windows (`GROUP BY TUMBLE(ts, …)`), `OVER` windows (`ROWS`/`RANGE`) | |
| Joins | Regular inner and outer joins (retracting), interval joins | Temporal joins (event-time versioned tables, `FOR SYSTEM_TIME AS OF`), lookup joins (processing time, JDBC and Postgres), `LATERAL` table functions | Multi-way joins (FLIP-516-style) on demand |
| Aggregation | Group aggregation with retraction, `DISTINCT`, `FILTER`, `GROUPING SETS`/`ROLLUP`/`CUBE`, mini-batch, two-phase (local-global) aggregation | Split distinct aggregation | |
| Ranking | Top-N (`ROW_NUMBER() OVER … WHERE rn <= N`), deduplication (first and last row) | | |
| Patterns | — | `MATCH_RECOGNIZE` (the Flink-supported subset: `PATTERN`, `DEFINE`, `MEASURES`, `AFTER MATCH`, quantifiers, `WITHIN`) | |
| Changelog | Insert-only, retract and upsert streams; `ChangelogNormalize`; upsert sinks with the materializer; `debezium-json` and `canal-json` changelog sources | `maxwell-json`, `ogg-json` | |
| Types | Flink's logical types (`CHAR`, `VARCHAR`/`STRING`, `BOOLEAN`, `BINARY`/`BYTES`, `DECIMAL(p,s)`, integers, floats, `DATE`, `TIME(p)`, `TIMESTAMP(p)`, `TIMESTAMP_LTZ(p)`, `INTERVAL`, `ARRAY`, `MAP`, `MULTISET`, `ROW`) with Flink's coercion and cast rules | `RAW`, structured types from Java classes (with the JVM host) | |
| Functions | The built-in functions ranked by use in the corpus (§15), at least the comparison, logical, arithmetic, string, temporal, conditional, type-conversion, collection, JSON and hashing families; aggregate functions | The rest of Flink 2.2's built-in catalog except the not-built list | Process table functions (FLIP-440), `ML_PREDICT` on demand |
| Modes | Streaming; bounded sources in streaming mode | `execution.runtime-mode = batch` on DataFusion's batch executor (D2275) | |
| Catalogs | `default_catalog` (in memory); the `loams` Iceberg catalog (Lakekeeper REST) | Persistent catalog store in Loams Postgres; `jdbc` catalog `loams_pg`; `CREATE CATALOG` types `generic_in_memory`, `iceberg`, `jdbc` (D2262) | |

The function list is a generated matrix (Flink's `SHOW FUNCTIONS` and `BuiltInFunctionDefinitions` at the pinned tag, each function marked supported, planned with its phase, or won't-do), as the House does for ClickHouse (§75 suite 5).

### 4.2 L2: REST (D2258)

**SQL Gateway REST API, v1–v4, in full (FC1).** v4 is Flink 2.2.1's default version (checked 2026-10-29) and has 13 paths: `/api_versions`, `/info`, `/sessions`, `/sessions/{session_handle}` and its `complete-statement`, `configure-session`, `heartbeat`, `operations/{operation_handle}/{cancel,close,result/{token},status}`, `scripts` and `statements`. Result rows use `rowFormat=JSON` and `PLAIN_TEXT` as Flink does. This is the surface `loams-flink`'s `SqlGatewayClient` (D1971) and the SQL Client and JDBC driver speak, so they work unchanged.

**The Flink REST API (dispatcher), in phases.** Flink 2.2.1's `rest_v1_dispatcher.yml` has 66 paths (checked 2026-10-29).

| Paths | Phase |
|---|---|
| `/config`, `/overview`, `/jobs`, `/jobs/overview`, `/jobs/{jobid}` (GET; PATCH to cancel), `/jobs/{jobid}/stop`, `/jobs/{jobid}/savepoints` and `/savepoints/{triggerid}`, `/jobs/{jobid}/checkpoints` (and `config`, `details/{checkpointid}`), `/jobs/{jobid}/exceptions`, `/jobs/{jobid}/plan`, `/jobs/{jobid}/config`, `/taskmanagers`, `/jobmanager/config` | FC1 |
| `/jobs/{jobid}/vertices/**` (metrics, back pressure, watermarks, subtasks), `/jobs/{jobid}/rescaling`, `/jobs/{jobid}/status`, `/jobs/{jobid}/checkpoints` trigger, `/jobmanager/metrics`, `/taskmanagers/{id}/**`, `/cluster`, `/datasets` | FC2 |
| `/jars/upload`, `/jars`, `/jars/{jarid}/run`, `/jars/{jarid}/plan`, `/jobs` POST (job graph submission) | FC3 (with the JVM host, D2261); before FC3 they answer 501 naming FC3 and the `apache` backend |

**The web dashboard.** Flink's dashboard (Angular, Apache-2.0) is built from the pinned tag and served read-only behind the gateway (D1918 already makes the dashboard read-only) in FC2. Its rendering of jobs, vertices, checkpoints and exceptions is part of L2's bar.

**Contract tests.** The OpenAPI files are vendored at the pinned tag (Apache-2.0) and generate request and response schema tests for every path in scope; a path not in scope must answer 501, never 404 (so tools see "not implemented", not "wrong server").

### 4.3 L3: Table API, PyFlink and UDFs (D2259, D2260)

A Java Table API or PyFlink program runs in a JVM (PyFlink drives Java through Py4J). It builds a pipeline with Flink's own planner in the client. Flink chooses the executor through `execution.target` and the `PipelineExecutorFactory` service interface (verify the exact SPI in 2.2.1, FC2 Task). Loams ships **`loams-stream-client`**, a jar with an executor for `execution.target: loams` that:

1. turns each `TablePipeline` or `StatementSet` into Flink's **compiled plan JSON** (FLIP-190: `compilePlan()`), or into SQL text where the program is SQL;
2. sends it to the engine's REST endpoint with the program's configuration and UDF jars;
3. returns a `JobClient` backed by the Flink REST API of §4.2.

The engine therefore has **two front ends** into one physical planner: Flink SQL text (the default, Rust) and ExecNode JSON (from the client jar and from the oracle in tests). The ExecNode reader also gives a sharper differential test: running Flink's own optimised plan on the Rust runtime separates runtime bugs from planner bugs (§15).

**UDFs.** Java `ScalarFunction`, `TableFunction`, `AggregateFunction` and `AsyncScalarFunction` run in a **JVM UDF host**, a separate process per task manager (a sidecar image) that receives Arrow batches over shared memory or a local socket through the Arrow C data interface and calls the function per row or per batch; aggregate accumulators are serialised into Loams state. Python UDFs (PyFlink's `udf`, `udtf`, `udaf`, pandas UDFs) run in a **Python worker** over Arrow IPC, the shape PyFlink already uses for its vectorised UDFs. Rust UDFs (and Wasm, via the `loams-sandbox` Wasm path, on demand) run in process. User code follows D1905: in multi-tenant clusters the hosts run only on gVisor nodes; the Rust engine itself runs the `linux` profile.

### 4.4 L4: DataStream (D2261)

Arbitrary Java DataStream jobs are Java code calling Flink's runtime classes; running them needs a JVM. The three options:

| | (a) Embedded JVM for user functions | (b) A Rust DataStream-like API | (c) Out of scope for v1 |
|---|---|---|---|
| What users get | Existing jobs and JARs, unchanged | A new API in Rust (and later Python), Flink's concepts and names | DataStream stays on Apache Flink (§81, D212) |
| How | The user's `main()` builds a `StreamGraph` in a JVM; the Loams executor translates it into the Rust operator graph; each user operator (`MapFunction`, `ProcessFunction`, `KeyedProcessFunction`, `CoProcessFunction`, `WindowFunction`, sources and sinks via the FLIP-27/FLIP-143 interfaces) runs in the JVM operator host; keyed state, timers, watermarks, checkpoints and shuffle stay in Rust, reached from Java through a Loams `KeyedStateBackend` and timer service over JNI or shared memory | A crate built from Arroyo's `arroyo-datastream` program graph, with keyed process functions, timers and state handles | Nothing |
| Cost (estimate) | Large: Flink's operator and state interfaces are wide, and each call crossing the JVM boundary costs | Medium | None |
| Compatibility | The real goal ("full API compat") | None for existing jobs | None |

**Ruling:** (c) until FC3, then **(a) and (b) together in FC3** (Q1434). The owner's "full compat" is the goal; DataStream is the last layer because it needs the runtime, state and checkpoint layers to be stable first, and because existing DataStream users already have a working path on Apache Flink through §81. The embedded JVM hosts **only user functions**: the operator runtime, state, checkpoints and shuffle stay Rust, so the JVM host scales with user code, not with the engine.

### 4.5 L5: Connectors (D2257, D2256, D2277, D2278)

Connector names, option keys and formats are Flink's, so DDL written for Flink runs unchanged. Formats: `json`, `csv`, `avro`, `avro-confluent` (with a schema registry if configured), `raw`, `parquet` (filesystem), `debezium-json`, `canal-json`; `protobuf` in FC2.

| Connector | Flink `connector =` | Source | Sink | Guarantee | Phase |
|---|---|---|---|---|---|
| Kafka through Iggy's Kafka gateway | `kafka`, `upsert-kafka` | Consumer groups, offsets in checkpoints | Produce | Sink **at-least-once**: the gateway has no transactions (D1915); exactly-once once the Iggy fork ships them | FC1 |
| External Kafka (Confluent, MSK) | `kafka`, `upsert-kafka` | As above | Transactional producer, two-phase commit (Arroyo has this, verify) | Exactly-once | FC1 |
| Iggy native | `iggy` (Loams' own name) | Iggy consumer, offsets in checkpoints | Deterministic message ids per (job, subtask, checkpoint, sequence) with Iggy's deduplication (verify) | Exactly-once within the dedup window (D2277) | FC1 |
| Iceberg via Lakekeeper | `iceberg` (catalog `loams`) | Incremental snapshot reads | One commit per checkpoint carrying the checkpoint id in the snapshot summary (as Flink's Iceberg sink does with `flink.max-committed-checkpoint-id`) | Exactly-once; append in FC1, upsert (v2 equality deletes; v3 deletion vectors once the House's iceberg-rust work lands, D1553) in FC2 | FC1/FC2 |
| Postgres CDC | `postgres-cdc` (Flink CDC option names) | Snapshot then pgoutput stream; slot LSN in checkpoints | — | Exactly-once into exactly-once sinks | FC2 |
| JDBC (Postgres, MySQL) | `jdbc` | Scan and lookup | Upsert with the primary key; append with a staging table committed at checkpoint completion | Exactly-once for upsert and staged append; at-least-once for plain append | FC2 |
| Filesystem (S3, local) | `filesystem` | Bounded and monitored | Pending files committed at checkpoint completion (multipart upload completed on commit), partition commit policies | Exactly-once | FC1 |
| Test connectors | `datagen`, `print`, `blackhole` | | | — | FC1 |

Credentials follow D1917 (the instance's scoped Iggy user, Lakekeeper client, CDC role and S3 keys) and D1985's secret refs; a user's key never appears in DDL.

### 4.6 L6: Checkpoints and savepoints

See §7.5. The user-visible contract is Flink's: `execution.checkpointing.interval`, `execution.checkpointing.mode` (`EXACTLY_ONCE`, `AT_LEAST_ONCE`), externalised retention, `STOP JOB '<id>' WITH SAVEPOINT`, `execution.state-recovery.path`, the REST savepoint and stop paths. Paths outside the instance's prefix are refused (D1977's protected keys carry over).

## 5. The base engine (D2243, D2244)

### 5.1 Candidates (checked 2026-10-29 on GitHub; §21; the research lane's report, §5.4)

| | StreamFusion | Arroyo | RisingWave | Apache Fluss | Feldera | DataFusion alone (House-style) |
|---|---|---|---|---|---|---|
| Licence | Apache-2.0 (`datafusion-contrib`; "not part of Apache Flink or Apache DataFusion") | Apache-2.0 and MIT (`LICENSE-APACHE`, `LICENSE-MIT`) | Apache-2.0 (Premium features gated by a licence key in the same code) | Apache-2.0 | MIT, with Feldera Enterprise code under commercial terms (GitHub reports `NOASSERTION`) | Apache-2.0 |
| Language and size | Rust, about 6.8 MB in `native/` (engine, bridge, formats, Kafka, Iceberg, Paimon, Fluss), plus about 11 MB of Java (the Flink plugin) | Rust, about 3.1 MB of Rust source, plus a TypeScript UI | Rust, about 32 MB of Rust source (about ten times Arroyo), plus Java connectors | Java (about 27 MB) with a Rust client (about 4.6 MB) | Rust (about 20 MB) plus a Java (Calcite) SQL compiler (about 9 MB) | — |
| Activity | v0.1.0-rc4 (2026-10-06), release candidates only; at least 100 commits since 2026-07-29; one main maintainer (about 1,875 commits) and one regular second (about 280) | Last release v0.15.0 (2025-12-01); 72 commits since 2026-07-29; Cloudflare-owned since 2025-04 (§68) | v3.1.0 (2026-09-21), v3.0.5 (2026-10-12); very active | v1.0.0 (2026-09-22) | Very frequent releases | DataFusion 55.2.0 (the `fabric/` pin) |
| Query layer | None of its own: **Flink plans the query**; supported ExecNodes are swapped for native Rust operators over DataFusion 54.0 and Arrow 58, with no DataFusion patch | DataFusion 54.1 (forked: 15 commits over 54.1.0 across 103 files, including "streaming SQL AST", accumulator checkpoint APIs and sliding `DISTINCT` checkpoints), sqlparser 0.62 fork, arrow-rs 58.4 fork | Its own binder, optimiser and plan IR; Postgres dialect | Not a query engine: a streaming storage (log plus primary-key tables) that Flink reads | DBSP incremental circuits; SQL compiled by Calcite in Java | Batch only: no durable operator state, checkpoints or event-time windows in core (§68 §6.1) |
| Streaming semantics | **Flink SQL's, reproduced exactly** ("anything else falls back to Flink with identical results"): Calc, unions, grouping sets, `TUMBLE`/`HOP`/`SESSION`/`CUMULATE` window aggregates, `OVER`, regular, interval, window, event-time temporal and lookup joins, group aggregates, Top-N, deduplication, changelog normalise, the sink upsert materializer; JVM scalar UDFs by upcall; Java-exact float and decimal text (`jdk_double`, `jdk_decimal`). Falls back for `MATCH_RECOGNIZE`, lateral table functions, PyFlink UDFs | Event-time watermarks, tumbling, sliding and session windows, updating (changelog) queries, joins, Chandy-Lamport checkpoints to object storage, Kafka exactly-once, Rust and Python UDFs (verify depth in FC0) | Close to Flink's: barriers and epochs, retractable changelog, watermarks, temporal and interval joins, `EMIT ON WINDOW CLOSE`; no `MATCH_RECOGNIZE` (verify) | — | Incremental view maintenance, not Flink's event-time and changelog model | None |
| State | Memory and RocksDB backends; a canonical savepoint payload of Arrow IPC sections per key group inside Flink's savepoint container | In-memory tables with Parquet snapshots on object storage | Hummock, a shared LSM on S3 run by compactor and meta services | — | In-memory or local storage | — |
| Control plane | Flink's (it is a plugin): no scheduler, network, checkpoint coordinator or REST of its own | Its own API, controller, compiler service, web UI (not wanted) | Meta service on Postgres, frontend, compute, compactor (four roles, D1950) | — | Its own pipeline manager | — |
| Distance to Flink | Closest of all on operator semantics; nothing of the runtime around them | Medium: the runtime concepts match (operators, barriers, watermarks, Kafka 2PC); the dialect and REST do not | Medium on semantics, far on dialect, plan IR and deployment shape | Not applicable | Far | Far: everything streaming is to be written |

### 5.2 Weighing the options

| | (a) Fork Arroyo whole | (b) Slim-fork RisingWave | (c) StreamFusion as a plugin inside Apache Flink | (d) **Hard fork of StreamFusion, made standalone (chosen)** |
|---|---|---|---|---|
| What runs | Arroyo's runtime with a Flink front end added | RisingWave with a Flink parser and binder over its plan IR, stripped of batch serving, Postgres wire and meta roles | Apache Flink, with supported operators in Rust | StreamFusion's operators in Loams' own runtime, no JVM |
| What we write | Flink front end, REST, catalogs; replace Arroyo's non-Flink operator semantics, API, UI and dialect | Flink parser, binder and deployment shape, against RisingWave's IR and meta service | Nothing | Flink SQL front end, scheduler, network and checkpoint coordinator (ported from Arroyo where they fit), REST, control plane; the operators that StreamFusion leaves to Flink (`MATCH_RECOGNIZE`, lateral table functions, three-phase distinct) |
| Code we own | About 3 MB, much of it then rewritten for Flink semantics | About 32 MB, coupled (frontend, meta and Hummock assume each other) | None, but the JVM stays | About 6.8 MB of StreamFusion Rust plus what we add |
| DataFusion and Arrow | 54.1 with a 15-commit fork | Not DataFusion | 54.0 and 58.3 | 54.0 and 58.3, bumped to the root workspace's 54.1 and 58.4: a patch-level and a minor-level bump |
| Flink semantics | Must be rebuilt operator by operator | Close on changelog and barriers, far on dialect and types | Exact (it is Flink) | Exact where StreamFusion's operators are native, checked by its own divergence suite and by Loams' oracle |
| A Rust engine without a JVM | Yes | Yes | **No** | Yes |

**Why StreamFusion.** It is the only Rust code base whose operators were written to reproduce Flink SQL's results exactly ("anything else falls back to Flink with identical results"). That includes changelog handling, Java-exact number formatting, Flink's key hashing and Flink's window calendars. It builds on crates.io DataFusion 54.0 and Arrow 58.3 with no DataFusion patch, which is Loams' line (root workspace: 54.1 and 58.4). Its weaknesses are release-candidate maturity and essentially one maintainer. They matter little once Loams owns the fork, and the oracle, not the donor's claims, decides correctness. What it lacks is everything Flink does around the operators, and that is §7's work.

**Why not RisingWave as the base**, although the owner named it first and `ostrium-labs/risingwave` was forked today: its streaming semantics are close to Flink's, but everything around them (the Postgres dialect and binder, its own plan IR, the meta service, Hummock and its compactor, four roles at every size, D1950) would have to be either kept, so the product is RisingWave with a Flink parser in front, or rewritten, so the fork buys little. The research report rates its Flink compatibility low. RisingWave stays the Postgres-wire streaming database of §81, and its fork is a **donor** of specific algorithms, ported file by file with attribution (Q1447).

**Why not fork Arroyo whole**: it has its own DataFusion SQL dialect and no DataStream or Table API, its operators implement Arroyo's semantics rather than Flink's, it carries a 15-commit DataFusion fork, and it has had no release since 2025-12-01. Its runtime pieces (the network manager, barrier alignment, the checkpoint coordinator, Kafka two-phase commit) are what StreamFusion lacks, so those are ported where they fit.

**Why not Fluss**: it is storage, not a processor. Loams already has its own logs (Loams streams, Iggy) and the House no longer needs Fluss (D1555). A Fluss connector is on demand (Q1445). StreamFusion's own Fluss path comes with the fork.

**Why not Feldera**: its SQL compiler is Java (Calcite), its model is incremental view maintenance rather than Flink's event-time and changelog semantics, and its enterprise boundary would need a file-by-file audit before any reuse (as Databend in §75).

### 5.3 The ruling (D2243)

The base is the hard fork **`ostrium-labs/streamfusion`**. Loams keeps the following from it, adds to it, and ports into it:

| | What |
|---|---|
| **Kept from StreamFusion** | The operators (`calc`, `window_agg`, `session_agg`, `over_agg`, `group_agg`, `updating_join`, `interval_join`, `window_join`, `temporal_join`, lookup join, `topn`, `dedup`, `normalizer`, `sink_materializer`, `mini_batch`, `rowtime`, `window_calendar`, `exchange`). The Flink expression and function semantics (`flink_functions`, `flink_float`, `flink_key`, `jdk_decimal`, `jdk_double`). The memory and RocksDB state backends and the canonical state sections. The Kafka, JSON, CSV, Avro, protobuf, Parquet, ORC, Iceberg, Paimon and Fluss format code. Its divergence suite against Flink. Its Java plugin also stays in the fork: it builds the operator descriptions from Flink's ExecNodes, so it is the reference for the ExecNode-JSON reader (§7.1) |
| **Written by Loams** | The Flink SQL front end and planner, the standalone scheduler (job manager and task managers), the REST surfaces, catalogs, the client jar, the JVM and Python UDF hosts, the control plane and Postgres metadata, and the operators StreamFusion leaves to Flink |
| **Ported from Arroyo** (file by file) | The network manager (Arrow IPC between workers, flow control), barrier alignment and the checkpoint coordinator, the Kafka transactional sink, the filesystem two-phase sink, and the program-graph shape for the Rust DataStream API (FC3) |
| **Ported from RisingWave** | Specific algorithms on demand (for example degree-table join state), per file |

StreamFusion's native plan is a description of Flink ExecNodes that the Java plugin sends over JNI. That description becomes the engine's **physical plan IR**. The Rust planner emits it, the ExecNode-JSON reader produces it, and the operators consume it unchanged.

### 5.4 The research report

The research lane's report (`loams-wt/.he0-shared/stream-engine-options.md`, 2026-10-29) arrived after the first draft of this design. Its findings, adopted here:
- only Apache Flink runs Flink jobs unchanged;
- Arroyo has its own SQL dialect and no DataStream or Table API;
- RisingWave's Flink compatibility is low;
- StreamFusion (Apache-2.0) runs Flink SQL natively in Rust inside Flink 2.2.x and matches the oracle's pin, but it is RC-only with essentially one maintainer.

The first draft's "hard fork of Arroyo's runtime as the base" is withdrawn. The owner then asked (2026-10-29, via the coordinator): "is streamfusion using datafusion, just fork it if it matches our loams engines". It does: DataFusion 54.0.0 and Arrow 58.3.0 in its lockfile, against Loams' 54.1 and 58.4, and Apache-2.0 (checked 2026-10-29). So `ostrium-labs/streamfusion` was forked on 2026-10-29 and is the base (D2243). Real Flink 2.2.1 stays the oracle and the fallback.

### 5.5 Hard-fork rules (D2244)

- **The fork.** `ostrium-labs/streamfusion` (forked from `datafusion-contrib/StreamFusion` on 2026-10-29) is Loams-owned. There is no upstream sync, no upstream PR and no upstream post, as with the Neon fork (`ostrium-labs/loams-postgres`) and the Iggy fork. A blobless clone lives at `~/Documents/Ostriumlabs/streamfusion`, with worktrees in `~/Documents/Ostriumlabs/streamfusion-wt/`. PRs go to the fork's `main`.
- **Versions.** FC0 moves the fork's `native/` workspace to the root workspace's DataFusion 54.1 and Arrow 58.4 and to Loams' Rust toolchain. Later DataFusion moves happen together with the House where they can.
- **Ports.** Code from Arroyo (`ostrium-labs/arroyo`, frozen for provenance) and from `ostrium-labs/risingwave` is ported per file, keeping its original copyright lines, and listed in `PORTS.md` in the fork with its source path and commit. Nothing is ported from a RisingWave module gated by its licence key.
- **No dependency forks.** No DataFusion or Arrow fork; Arroyo's DataFusion patches are not taken.

## 6. Coexistence with design 81 (D2264)

Design 81's managed Flink stays the product: one instance per namespace, the facade, the CLI, the console, metering and authorisation. An instance gains a field:

| `backend` | What runs | Default |
|---|---|---|
| `apache` | Apache Flink 2.2.1 session cluster plus SQL Gateway (§81 §8.3) | Yes, until FC3's gate |
| `loams` | Loams Stream (roles `jobmanager` and `taskmanager`, or `all` in compose) | Opt-in from FC1 (preview) |

- The `loams-flink` adapter renders either backend; the SQL Gateway client, the pipeline store, the recovery logic and the gateway routes are shared, because Loams Stream speaks the same REST APIs.
- **No automatic fallback routing** (Q1436): a statement the Rust engine does not support fails with D2268's error naming the feature and the `apache` backend. Silent routing would hide which engine holds a job's state.
- **Switching an existing pipeline** between backends is stop-with-savepoint on one side and restart from source offsets on the other (FC2), or from exported state (FC3, Q1438).
- **The default flips** to `loams` only when FC3's gate passes and the owner approves (Q1437). `apache` then stays available as the compatibility fallback, as Apache Spark stays beside Sail (D1911).

## 7. Architecture

```
 Flink tooling: SQL Client (gateway mode), JDBC driver, REST clients, web dashboard, loams-flink, client jar (Table API, PyFlink)
          │
 loams-gateway (hostname routes per instance, Loams principals, OpenFGA `engine`; never public)
          │
 loams-stream jobmanager ── Loams Postgres: jobs, sessions, checkpoints, savepoints, catalog store, leader lease
   ├── loams-stream-rest: SQL Gateway REST v1–v4, Flink REST (dispatcher)
   ├── loams-stream-sql: FlinkDialect parser → Flink-typed validated tree
   ├── loams-stream-plan: → DataFusion logical plan + streaming nodes → optimise → physical graph; ExecNode-JSON reader
   └── loams-stream-coord: scheduler, checkpoint coordinator, two-phase-commit coordinator, rescaling
          │  (task deployment, barriers, acks: gRPC)
 loams-stream taskmanager × N (task slots)
   ├── loams-stream-runtime: operator chains, network (Arrow IPC streams, credit-based), timers, watermarks
   ├── StreamFusion's operators (windows, joins, aggregates, Top-N, dedup, changelog) + Loams' (MATCH_RECOGNIZE, lateral)
   ├── state backends (StreamFusion's): memory, RocksDB with incremental SSTs to S3; slatedb after the spike; foyer cache (loams-cache)
   ├── loams-stream-connectors: Kafka, Iggy, Iceberg (Lakekeeper), Postgres CDC, JDBC, filesystem
   └── UDF hosts (optional sidecars): JVM (Java UDFs; DataStream operators in FC3), Python
          │
 The bucket: engines/<ns>/<id>/{checkpoints,savepoints,state,jobs}/ (D1907)   Iceberg tables in loams-lake via Lakekeeper
```

### 7.1 The planner (D2248, D2259, D2276)

1. **Parse.** `loams-stream-sql` implements a `FlinkDialect` on sqlparser-rs 0.63 (crates.io) with statement-level hooks for what sqlparser lacks: Flink's `CREATE TABLE` column forms and `WATERMARK`, `TABLE(...)`/`DESCRIPTOR(...)` in TVF calls, `FOR SYSTEM_TIME AS OF`, `EXECUTE STATEMENT SET`, `COMPILE`/`EXECUTE PLAN`, Flink's `SET 'k' = 'v'`, hints, `SHOW` and `DESCRIBE` forms. sqlparser already parses `MATCH_RECOGNIZE` (added for Snowflake; verify coverage against Flink's subset). The reference grammar is Flink's Calcite parser templates at the pinned tag (`flink-sql-parser`, Apache-2.0), and every `.q` and test-program statement is a parser test.
2. **Validate and type.** A binder resolves names against Flink's catalog model (catalog, database, object; temporary and permanent; `USE CATALOG`/`USE`) and types expressions with **Flink's** rules: Flink's logical types, implicit coercion, cast and `TRY_CAST` semantics, decimal precision and scale derivation, `STRING` versus `VARCHAR(n)`, `TIMESTAMP_LTZ` and the session time zone, nullability. These differ from DataFusion's defaults, so the binder emits explicit casts and Flink-semantics functions rather than leaning on DataFusion's coercion (Arroyo's patch "Reject implicit string-numeric coercion" is the same instinct).
3. **Streaming semantics.** The planner infers each node's changelog mode (insert-only, upsert with key, retract) and time attributes (rowtime, proctime) as Flink does, inserts `ChangelogNormalize`, upsert materialisation, watermark assigners and mini-batch, and rejects what Flink rejects (for example a non-windowed aggregate feeding an append-only sink).
4. **Lower.** Into StreamFusion's operator descriptions (the same physical IR its Java plugin builds from Flink's ExecNodes), with parallelism, key-group partitioning and chaining. DataFusion evaluates expressions in Flink semantics (StreamFusion's `flink_functions`) and runs batch mode (D2275).
5. **ExecNode JSON.** A second front end reads Flink's compiled plan JSON into the same physical graph: the path for the client jar (D2259) and for differential tests. Flink's planner (Java) runs only in the client and in tests, never in the engine (D2276, Q1435).

### 7.2 Changelog and time (D2249, D2250)

- Every batch carries `_rowkind` (`Int8`: `+I`, `-U`, `+U`, `-D`) as Flink's `RowKind`. Insert-only operators skip the column's checks. Sinks and formats map it (`debezium-json` `op`, upsert-kafka tombstones, JDBC upserts and deletes, Iceberg equality deletes).
- Watermarks are generated per source split from the DDL's `WATERMARK` expression, combined as the minimum across inputs, with `scan.watermark.idle-timeout`, emit strategies and alignment groups (`scan.watermark.alignment.*`) as Flink 2.2 defines them. Late rows in window aggregates are dropped as Flink drops them; other operators follow Flink's per-operator rules.
- Processing time uses the task manager's clock; `PROCTIME()` and `CURRENT_WATERMARK()` are Flink's. `TIMESTAMP_LTZ` arithmetic and windowing on it follow `table.local-time-zone`.

### 7.3 Runtime (D2251)

Ported pieces (§5.3) fill a Loams-owned runtime: operators process Arrow record batches; chains fuse stateless operators in one task; tasks run on Tokio in task slots; the network layer sends Arrow IPC between task managers with credit-based flow control so back pressure reaches sources. Loams adds Flink's knobs where they change results (`table.exec.mini-batch.*`, `parallelism.default`, `pipeline.max-parallelism`, per-operator parallelism through hints) and the metrics the Flink REST and dashboard expect (records in and out, back pressure, watermarks, checkpoint durations).

### 7.4 State (D2252, Q1440)

- **One state-backend trait** with key groups (Flink's `pipeline.max-parallelism` rule), typed per-operator tables, timers and TTL. Operators are written against the trait, as StreamFusion's are.
- **FC1 backends**: memory (small state, tests) and **RocksDB** through `rust-rocksdb`, both ported from StreamFusion, with checkpoints uploading incremental SST files to `engines/<ns>/<id>/state/` (Flink's incremental RocksDB model).
- **slatedb** (Apache-2.0, an LSM on object storage, v0.17.0 released 2026-09-29) is spiked in FC0: one database per subtask, keys prefixed by key group, reads through a foyer RAM and NVMe cache (`loams-cache`, D1557). Object storage as the primary store fits "state in the bucket", and checkpoints become manifest pointers rather than uploads. If the spike shows its single-writer model, checkpoint and clone features fit per-subtask databases and rescaling at acceptable cost (verify), slatedb becomes the default in FC2.
- **TTL.** `table.exec.state.ttl` and the `STATE_TTL` hint expire keyed state by compaction filters and lazy checks, as Flink's RocksDB TTL does.

### 7.5 Checkpoints, savepoints, two-phase commit, rescaling (D2253–D2256)

- **Barriers.** The checkpoint coordinator injects numbered barriers at the sources; operators align barriers across inputs, snapshot, and forward them (Arroyo's Chandy-Lamport implementation). Sources snapshot offsets (Kafka and Iggy offsets, CDC LSNs, file positions). Unaligned checkpoints are FC4 on demand (Q1449).
- **Snapshots.** RocksDB state is checkpointed as incremental SSTs (slatedb, if adopted, in place as a durable manifest per subtask), memory state as Arrow IPC sections; the coordinator writes the checkpoint's `_metadata` to `engines/<ns>/<id>/checkpoints/<job>/chk-<n>/` and records it in Postgres, then signals completion.
- **Two-phase commit.** A sink pre-commits at the barrier (Kafka transaction flushed, Iceberg data files written and recorded in state, file parts uploaded, JDBC staging rows written) and commits only when the coordinator reports the checkpoint complete; on recovery, pending commits recorded in the checkpoint are committed again idempotently (Iceberg checks the checkpoint id in the snapshot summary; Kafka resumes the transaction by its transactional id).
- **Savepoints.** Canonical savepoints write each operator's logical state as Arrow IPC sections per key group (StreamFusion's representation) plus metadata naming operator ids (stable `uid`s derived from the plan, as Flink derives them for SQL) and the engine version. They restore at any parallelism and across engine versions. Native savepoints (the backend's own checkpoint) are fast but restore only at the same version.
- **Rescaling.** State is partitioned into key groups (`pipeline.max-parallelism`, Flink's default rule); a subtask owns a contiguous key-group range. FC1 rescales by stop-with-savepoint and restore. FC2 rescales in place: new subtasks read their key-group ranges from the last checkpoint.
- **Verification.** The barrier and two-phase-commit protocol is modelled in TLA+ under `spec/tla/` and run in deterministic simulation with injected failures (the practice of §31, §55 and §75 suite 8).

### 7.6 Code layout (D2245, D2246)

The engine lives in the fork's **`native/`** Cargo workspace, as the Kafka-compatible broker lives in the Iggy fork. It is on **DataFusion 54.1 and Arrow 58.4**, the root workspace's pins, after FC0's bump from 54.0 and 58.3. The fork builds and releases the `loams-stream` binary and image (`ghcr.io/ostrium-labs/loams-stream`). The Loams repository holds the integration: the `backend: loams` support in `loams-flink`, the compose profile, the gateway routes and the product conformance under `conformance/stream/` (Q1432). StreamFusion's existing crates keep their names; the new crates are:

| Crate | Holds |
|---|---|
| `loams-stream-sql` | `FlinkDialect`, the statement AST extensions, the binder with Flink's types and coercion |
| `loams-stream-plan` | Changelog and time-attribute inference, streaming optimiser rules, lowering to StreamFusion's operator descriptions, the ExecNode-JSON reader |
| `loams-stream-runtime` | Task manager: tasks, chains, network, timers, watermarks, barriers |
| `loams-stream-coord` | Job manager: scheduler, checkpoint and commit coordinators, rescaling, metadata in Postgres, leader lease |
| `loams-stream-rest` | SQL Gateway REST v1–v4 and the Flink REST API |
| `loams-stream-connectors` | Kafka, Iggy, Iceberg, Postgres CDC, JDBC, filesystem, test connectors, formats |
| `loams-stream-udf` | The JVM and Python UDF host protocols (Rust side; the JVM host builds on StreamFusion's JVM scalar-UDF upcall) |
| `loams-stream-client` | The Java client jar (Maven, beside the fork's existing Java modules) |
| `loams-stream-conformance` | The corpus runner, the differential harness, the REST contract tests |
| `loams-stream` (bin) | One binary with roles `jobmanager`, `taskmanager`, `all` |

The unrelated `crates/loams-stream-grpc` (native gRPC ingest into Loams streams) keeps its name.

## 8. Deployment (D2263, D2265)

### 8.1 Compose

A profile **`stream`** in `deploy/release/compose.flink.yaml` (beside §81's Flink services) runs one `loams-stream` container with role `all`, needing `rustfs` and Loams Postgres, on `engine-gateway`'s loopback port for Flink (8083, D1918). Its memory floor is the target that makes a namespace affordable on the light stack (estimate: under 512 MiB idle, measured in FC1).

### 8.2 Kubernetes and BYOC

`loams-flink` renders, for `backend: loams`, a JobManager Deployment (two replicas with the Postgres lease for HA in FC2) and a TaskManager Deployment, in namespace `eng-<26>` (D1907, R24), with the sandbox profile of D1905 and optional UDF host sidecars on gVisor nodes. BYOC follows §81 §7.3.

### 8.3 Metadata and HA (D2265)

Jobs (their SQL or plan JSON, configuration with secret refs, operator uids), sessions, checkpoints, savepoints and the catalog store live in Loams Postgres (the direct endpoint, one database per instance, as D1908 does for RisingWave). The active JobManager holds a Postgres lease; on loss, a standby (or the restarted container in compose) takes the lease and restores each running job from its latest completed checkpoint. This replaces, for `backend: loams`, the compose recovery record of D1974.

## 9. Catalogs and data paths

- **`loams`** (Iceberg REST, Lakekeeper, vended credentials; D1913): tables written by Loams Stream are read by the House, Spark, RisingWave and Flink (D1914's rule holds: the House reads them as Iceberg).
- **`loams_pg`** (JDBC catalog for Loams Postgres, D2262).
- **Streams**: Iggy native and the Iggy Kafka gateway (D1915), external Kafka.
- **The catalog store** in Postgres persists `CREATE CATALOG` and permanent objects of the in-memory catalog across sessions (Flink's FLIP-295 semantics); credentials in catalog options are secret refs only.

## 10. Security (D2266, D2260)

§81's model applies unchanged: hostname routes per instance in `loams-gateway`, Loams principals with OpenFGA `engine` relations (`can_view`, `can_operate`, `can_admin`, `can_connect`), scoped service credentials per instance in env or mounted files, never in argv, and the protected `SET` keys of D1977. The engine never listens publicly. The Rust engine runs the `linux` sandbox profile; the JVM and Python hosts run user code and so need gVisor on shared nodes (D1905). Jar uploads (FC3) are stored under the instance's prefix and scanned for size and type only; running them is what the sandbox is for.

## 11. Metering (D2267)

D1920's events, unchanged in shape, with `backend = "loams"`: `io.loams.dev.usage.engine.runtime.v1` (`cpu_ms`, `memory_mib_ms`) every 60 s per running instance and the hourly `engine.storage.v1` (`state_bytes`, now including slatedb and savepoints). The Meter gate is called on create, resume, scale up and job submit; a hard limit stops jobs with a savepoint, never deletes state.

## 12. Surfaces

No new API family: §81's facade (`/v1/namespaces/{ns}/engines`, `/flink/...`), the `loams flink` CLI and the console's Flink pages serve both backends, with `backend` shown and settable on create. The raw Flink REST and SQL Gateway REST are reached as D1918 says.

## 13. Performance (D2270)

Targets, estimates until measured, never promises:
- Nexmark q0–q22 throughput per core at least Flink 2.2.1's (RocksDB, the same checkpoint interval) on the same machine.
- p99 end-to-end latency at a fixed input rate below Flink's.
- Recovery time from kill to first committed output below Flink's.
- Idle memory floor per instance under 512 MiB.

Measured in the owner's quiet window (the 1B QPS push's rule), per commit nightly, results internal until the owner publishes them.

## 14. Risks

| # | Risk | Mitigation |
|---|---|---|
| R1 | Flink's SQL semantics are wide (type rules, functions, changelog inference) | The oracle on every case; the generated function matrix; the planner's Flink binder rather than DataFusion's coercion |
| R2 | StreamFusion's operators assume Flink drives them (Flink's timers, key context, checkpoint calls, JNI bridge) | FC0's first task drives them from a Rust harness without Flink; the runtime implements the same contract; the operators' own tests and the divergence suite stay green on every PR |
| R3 | The base is RC quality with essentially one maintainer | Loams owns the fork; the oracle, not the upstream's claims, decides correctness |
| R4 | DataStream through a JVM is slow at the boundary | Batch calls where the API allows; per-record calls only for process functions; measured in FC3 before the default flips |
| R5 | Exactly-once into Iggy depends on Iggy's deduplication and the gateway's missing transactions | Stated per connector (§4.5); Iceberg for end-to-end exactly-once; transactions in the Iggy fork |
| R6 | Two Flink backends double the test matrix | One conformance harness, both backends nightly; the `apache` backend is also the oracle |
| R7 | Scope against the team's size | Phases with gates; preview labels; FC0 builds the harness before the engine |
| R8 | Trademark | "Flink-compatible" only; no Flink logos; the dashboard rebranded in its header only as Apache-2.0 allows |

## 15. Conformance and testing (D2269)

### 15.1 Suites

| # | Suite | What | Cadence |
|---|---|---|---|
| 1 | **Flink's test programs** | Flink 2.2.1's `TestProgram`s in `flink-table-planner` (`*SemanticTests`, `*TestPrograms`, `*RestoreTest`: calc, joins, interval and lookup joins, window aggregates, deduplication, rank, over windows, `MATCH_RECOGNIZE`, changelog normalise, sinks, and more; checked 2026-10-29) exported by a small JVM extractor at the pinned tag into a language-neutral corpus (sources with changelog rows, SQL, expected sink changelog). Restore programs become stop-with-savepoint and restore cases on Loams' own savepoints | Every PR |
| 2 | **SQL Client and Gateway `.q` files** | The `.q` files of `flink-sql-client` (14) and `flink-sql-gateway` (9) at the pinned tag, vendored, run through the SQL Gateway REST against Loams Stream | Every PR |
| 3 | **Differential against Flink 2.2.1** | The same deterministic inputs (seeded `datagen`, files, Kafka topics replayed) into both engines; final upserted results and, where deterministic, changelog multisets compared; grammar-based query generation over the supported surface; a second mode runs Flink's compiled ExecNode JSON on the Rust runtime to separate planner from runtime bugs | Nightly |
| 4 | **Golden queries** | Nexmark q0–q22 (Apache-2.0, the `nexmark/nexmark` queries), Flink's documentation examples, and the TPC-DS queries that Flink 2.2.1 runs in streaming mode over bounded sources (Q1444; text fetched at test time, never committed) | Nightly |
| 5 | **REST contract and tooling** | Schema tests from the vendored OpenAPI files; the SQL Client in gateway mode; the Flink JDBC driver; `loams-flink`'s own tests with `backend: loams`; the web dashboard (FC2) | Every PR (schemas), nightly (tools) |
| 6 | **Exactly-once and recovery** | Kill -9 of task managers and the job manager at random points; network partitions; the committed output compared with the failure-free run per connector guarantee; TLA+ model checking and deterministic simulation of the barrier and commit protocol | Nightly; TLA+ per PR touching the protocol |
| 7 | **Connector interop** | Iceberg round trips with the House, Spark, Flink and RisingWave (v2, then v3); Kafka through the Iggy gateway and external Kafka; Iggy native; Postgres CDC compared with the source | Nightly |
| 8 | **Performance** | Nexmark per core, latency, recovery and memory against Flink 2.2.1 (§13) | Nightly in the quiet window |

Rules as in §81 §14: strict `xfail.toml` (an unexpected pass fails), effects checked rather than exit codes, local runners for nightlies (owner rule of 2026-10-28), GitHub-hosted runners only for releases.

### 15.2 The scope manifest

Every suite reads one manifest (`conformance/profile.toml` in the fork) that marks each feature supported, planned with its phase, or won't-do. The manifest generates the compatibility page and the D2268 error messages, and a test fails when the manifest and the suites disagree (§75's surface-file rule, D347).

## 16. Phases (D2271): track FC

### 16.1 The phases

| Phase | Scope | Exit gate |
|---|---|---|
| **FC0** (harness and skeleton) | Rulings and versions; the fork's version bump and the new crate skeletons; the operators driven without Flink; `PORTS.md`; the Flink SQL parser; the oracle harness against Flink 2.2.1 via `loams-flink`; the corpus extractors (test programs, `.q` files, Nexmark); the function and type matrix; the slatedb state spike; the scope manifest | The corpus runs on the oracle with 100 % of its own expectations; the parser accepts every statement in the corpus; a ported stateless pipeline (Nexmark q0) runs end to end |
| **FC1** (preview) | L1 core (§4.1 FC1 column), L2 SQL Gateway v1–v4 and the FC1 REST paths, L5 Kafka (gateway and external), Iggy native, Iceberg append, filesystem, test connectors; L6 aligned checkpoints, savepoints, stop-and-restore rescaling; compose profile `stream`; `backend: loams` in `loams-flink` | Nexmark q0–q22 match the oracle; at least 80 % of the in-scope test programs and `.q` files pass (estimate, re-set in FC0); the SQL Client, JDBC driver and `loams-flink` tests pass unchanged; kill -9 recovery is exactly-once for Iceberg and filesystem; preview label |
| **FC2** (beta) | L1 rest (temporal and lookup joins, `MATCH_RECOGNIZE`, `OVER`, `SESSION`, legacy group windows, batch mode, catalogs and the catalog store); L3 Table API and PyFlink via the client jar, Java and Python UDF hosts; L5 Postgres CDC, JDBC, Iceberg upsert; in-place rescaling; JobManager HA; FC2 REST paths and the web dashboard; Kubernetes | At least 95 % of the in-scope test programs and `.q` files; 14 nightly differential runs without an unexplained mismatch; the Table API and PyFlink corpus pass; chaos suite green; beta label |
| **FC3** (GA) | L4 DataStream via the JVM operator host; JAR upload and run; job-graph submission through the client jar; the Rust DataStream API; state migration from Flink savepoints for SQL operators (Q1438); performance gates; security review | All six bars of §3.1; Nexmark targets met; then the owner decides the default flip (Q1437); GA label per layer |
| **FC4** (on demand) | Unaligned checkpoints, changelog state backend, process table functions, materialized tables, `ML_PREDICT`, a Fluss connector, multi-way joins | User requests |

The plan [FC1](../plans/2026-10-29-fc1-flink-compat-engine.md) breaks FC0 and FC1 into tasks with files, tests and gates; FC2 onward are planned at FC1's exit.

### 16.2 Order against other tracks

FC0 starts now (owner, 2026-10-29). It shares the build machine with HE (the House engine) and BQ1, so it builds through `buildslot.sh` at `-j 4` and writes its status to `loams-wt/.he0-shared/fc1-status.md` (D2279).

### 16.3 Labels (D2272)

Every surface says **preview** in FC1, **beta** in FC2, and GA per layer only after FC3's bar for that layer. Docs say "not yet run end to end" until a case has run. The compatibility page, generated from the manifest and the last nightly, names Flink 2.2 and the pass counts per layer, never a single percentage of "Flink".

### 16.4 Not built (D2274)

The Hive dialect, Hive catalog and HiveServer2 endpoint; the Scala API; Flink ML; Gelly; the Flink Kubernetes Operator's CRDs (Loams' own operator and `loams-flink` manage instances); Arroyo's SQL dialect, REST API, controller, compiler service and web UI. The DataSet API was removed in Flink 2.0 and is not a goal.

## 17. What this supersedes or amends

| Earlier | What it says | Change (approved 2026-10-29) |
|---|---|---|
| **§00 §7** | Loams is not a stream processor | **Amended**: Loams ships a Flink-compatible stream processor; it still invents no dialect of its own |
| **D1261**, **Q1036** (§68 §6.5) | No native continuous-query engine now; an L4 engine only behind a gate, starting from Arroyo's code | **Superseded** by D2240 and D2243 on the owner's request; the "start from Arroyo" direction is kept |
| **D1272** | No stream processor before Q1036 | **Amended** by D2240 |
| **D1930** (§81 §16.2) | No Loams fork of any engine | **Amended** by D2244: StreamFusion is hard-forked as the base; Arroyo is a donor ported file by file |
| **D212** (§26) | DataStream on the Flink Kubernetes Operator | **Amended at FC3** by D2261; until then unchanged (Apache Flink through §81) |
| **D1900, D1909** (§81) | Managed Flink SQL on Apache Flink | **Extended** by D2264: a `backend` field; `apache` stays the default until FC3 |
| **D1974** | Compose recovery record for Flink | Kept for `backend: apache`; `backend: loams` recovers from Postgres (D2265) |
| **D1260, D341** | RisingWave is the streaming SQL companion; Flow has no Flink targets | **Unchanged** |
| **D1915, D1916, D1917, D1918, D1919, D1920** | Kafka via Iggy, CDC, credentials, gateway, authorisation, metering | **Applied** to the new backend unchanged |

Amendment notes pointing here are added to §68 and §81 and to the amended decision-log rows.

## 18. Licences and trademarks (D2273)

- **Apache Flink** (Apache-2.0): the `TestProgram` sources, `.q` files, OpenAPI files and the web dashboard may be vendored or built with their headers and a NOTICE entry; no Flink Java code runs in the engine.
- **StreamFusion** (Apache-2.0): the fork keeps its LICENSE and copyright lines. **Arroyo** (Apache-2.0 and MIT): ported code keeps its copyright lines; NOTICE names each project and commit, and `PORTS.md` lists every ported file.
- **RisingWave** (Apache-2.0): per-file ports only, never from licence-key-gated modules.
- **slatedb, DataFusion, Arrow, sqlparser-rs, iceberg-rust, foyer** (Apache-2.0).
- **Feldera**: no code reused (enterprise boundary unaudited).
- **Nexmark** (Apache-2.0) queries vendored; **TPC-DS** query text fetched at test time, never committed (its EULA, as D414 treats ClickBench).
- **Trademarks**: "Apache Flink" and "Flink" are the ASF's. The product is "Loams Stream, compatible with Apache Flink's SQL and REST APIs"; it is never called Flink.
- No AGPL, BSL, FSL, SSPL or ELv2 code is copied (D11, D1574).

## 19. Not decided here

The FC2 and FC3 task plans; prices (none in preview, as Q1403); Loams Cloud hosting (as Q1408).

## 20. Open questions (Q1430–Q1450), answered 2026-10-29

Recorded in the decision log. The owner answered all of them on 2026-10-29 with the default ("approve all defaults").

| # | Question | Default (answered by the owner, 2026-10-29) |
|---|---|---|
| Q1430 | The product and binary name? | **Loams Stream**, binary `loams-stream`, crates `loams-stream-*`; described as "compatible with Apache Flink's APIs" |
| Q1431 | The base? | **A hard fork of StreamFusion** (owner, 2026-10-29: "just fork it if it matches"), made standalone with runtime pieces ported from Arroyo (§5); RisingWave as a donor |
| Q1432 | Where does the code live, on which DataFusion? | **In the fork's `native/` workspace on DataFusion 54.1 and Arrow 58.4**, no DataFusion fork; the product integration in the Loams repository |
| Q1433 | The oracle version: 2.2.1 or 2.3.0? | **2.2.1**, moving with D1931's pin |
| Q1434 | DataStream: embedded JVM, Rust API, or out? | **Out until FC3; then both** the embedded JVM operator host and a Rust DataStream API |
| Q1435 | A JVM planner sidecar as a fidelity fallback in production? | **No**: Flink's planner only in tests and in the client jar |
| Q1436 | Route unsupported statements to Apache Flink automatically? | **No**: explicit `backend`; the error names the feature and the `apache` backend |
| Q1437 | Flip §81's default backend to `loams` at FC3's gate? | **Yes, after the gate passes and the owner reviews it** |
| Q1438 | Migrating state from Flink savepoints? | **Source offsets in FC2; SQL-operator state through a State Processor API exporter in FC3** |
| Q1439 | Exactly-once into Iggy and the Kafka gateway? | **Iggy native by deterministic ids and deduplication in FC1 (verify); Kafka-gateway sinks at-least-once until the Iggy fork has transactions** |
| Q1440 | The keyed state store? | **RocksDB (ported from StreamFusion) in FC1**, with slatedb 0.17.0 spiked in FC0 and made the default in FC2 if it meets the bar |
| Q1441 | Upstream PRs or posts (StreamFusion, Arroyo, DataFusion, sqlparser)? | **None**: hard fork; patches stay in Loams' forks unless the owner approves a post |
| Q1442 | Publish the compatibility page? | **Yes, from beta**: per-layer pass counts against Flink 2.2, never "100 % Flink" |
| Q1443 | When does FC0 start? | **Now** (owner, 2026-10-29) |
| Q1444 | What is "the TPC-DS stream subset"? | **The TPC-DS queries Flink 2.2.1 runs in streaming mode over bounded sources**, text fetched at test time, results internal |
| Q1445 | A Fluss connector? | **On demand (FC4)** |
| Q1446 | The Hive dialect and catalog? | **Not built** |
| Q1447 | What is `ostrium-labs/risingwave` for? | **A frozen donor** for per-file ports; §81's managed RisingWave keeps upstream images |
| Q1448 | Serve the Flink web dashboard? | **Yes in FC2**, built from the pinned tag, read-only behind the gateway |
| Q1449 | Unaligned checkpoints and the changelog state backend? | **FC4, on demand** |
| Q1450 | Run the fork's Flink plugin as an accelerator inside §81's `apache` backend? | **Not now**: revisit once the fork's operators pass FC1's gate standalone |

## 21. Sources (checked 2026-10-29)

- **Owner**: requests of 2026-10-29 quoted above; "approve all defaults, start building asap" (via the coordinator).
- **Loams**: §00, §26 (D212), §68 §6 (D1260, D1261, D1272, Q1036), §75, §81 and `loams-wt/.he0-shared/engines-81.md` (rulings R13, R21–R26), `crates/loams-flink`, `fabric/Cargo.toml` (DataFusion 55.2.0, Arrow 59, iceberg 0.10.1), the root `Cargo.toml` (DataFusion 54.1, Arrow 58.4), on `dev` at `2a0f4c914`.
- **Research report**: `loams-wt/.he0-shared/stream-engine-options.md` (2026-10-29).
- **StreamFusion**: github.com/datafusion-contrib/StreamFusion (GitHub API: licence, languages, releases, contributors, commits since 2026-07-29; `readme.md`, `native/Cargo.toml`, `native/engine/src`, `docs/backends/canonical-state.md`).
- **Arroyo**: github.com/ArroyoSystems/arroyo (GitHub API: licence, languages, releases, commits since 2026-07-29; `Cargo.toml` and its `[patch.crates-io]`; `crates/`); the compare of `ArroyoSystems/arrow-datafusion` rev `0b17b6de…` against `apache/datafusion` 54.1.0.
- **RisingWave**: github.com/risingwavelabs/risingwave and github.com/ostrium-labs/risingwave (GitHub API: licence, languages, releases).
- **Fluss**: github.com/apache/fluss (licence, languages, v1.0.0).
- **Feldera**: github.com/feldera/feldera (licence, languages).
- **Apache Flink**: github.com/apache/flink at `release-2.2.1` and `release-2.3.0` tags; `docs/static/generated/rest_v1_dispatcher.yml`, `rest_v{1..4}_sql_gateway.yml`; `SqlGatewayRestAPIVersion.java` (V4 default); `flink-table/flink-sql-client/src/test/resources/sql/*.q`, `flink-table/flink-sql-gateway/src/test/resources/sql/*.q`; `flink-table/flink-table-planner/src/test/java/org/apache/flink/table/planner/plan/nodes/exec/stream/` (test programs).
- **slatedb**: github.com/slatedb/slatedb (v0.17.0, 2026-09-29).
- **Nexmark**: github.com/nexmark/nexmark (Apache-2.0).
- **Flink CDC**: github.com/apache/flink-cdc (Apache-2.0).
