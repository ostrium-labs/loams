# 82 — The Loams Stream Engine: Rust, Apache Flink-Compatible

Status: **Approved** (owner, 2026-10-29: "approve all defaults, start building asap"). Decisions **D2240–D2279** and questions **Q1430–Q1449** are in the [decision log](13-decision-log.md) with status **Approved**; every question is answered with its default (§20). D2280–D2299 and Q1450–Q1469 are kept free for track FC's plan rulings. Plan: [FC1](../plans/2026-10-29-fc1-flink-compat-engine.md). **No engine code is written by this document.**

Source: the owner, 2026-10-29: "like iggy for kafka, event fabric for trino, i want one for flink, full api compat but in rust", and earlier the same day: "fork rising wave latest apache 2.0 and strip components we do not need, or find alternatives like arroyo, fluss etc".

**The pattern.** Loams runs its own Rust engines that speak another system's API: the Iggy fork speaks the Kafka wire protocol (D1797), and Loams House speaks ClickHouse on DataFusion and Iceberg v3 with ClickHouse as the oracle ([§75](75-loams-house-engine.md)). This document applies the House's method to Apache Flink. (The owner's "event fabric for trino" has no counterpart in the repository: Loams Event Fabric is Iggy plus Fluss, D331, and nothing in Loams speaks Trino's protocol. The pattern, not that example, is what this design follows.)

**What it changes.** Design 81 runs real Apache Flink 2.2.1 per namespace through `loams-flink` (D1900, D1909). This document adds a second backend under the same managed-Flink product: a Rust engine that accepts Flink SQL, the Flink SQL Gateway REST API and the Flink REST API, and later the Table API, PyFlink and DataStream programs. Real Flink 2.2.1 stays as the **oracle** for every test and as the **fallback backend** for anything the Rust engine does not yet run. §17 lists what this supersedes; the main ones are:

- **D1261** and **Q1036's answer** ("no native continuous-query engine now"; L4 only behind a gate) are **superseded** by the owner's request: the L4 engine is built now, and it starts from Arroyo's code, as §68 §6.5 already anticipated (D2240, D2243).
- **D1272**'s "no stream processor before Q1036" and **D1930**'s "no Loams fork of any engine" are **amended**: Arroyo is hard-forked as a code donor (D2244).
- **§00 §7** ("Loams is not a stream processor") is **amended**: Loams ships a Flink-compatible engine; it still does not invent its own stream-processing dialect.
- **D212**'s operator path for DataStream jobs is **amended at FC3** (D2261).

Markers, as in §75 and §81:
- **(verify)**: not checked against a primary source; the task that depends on it checks it first.
- **(estimate)**: judged, not measured.
- **(checked 2026-10-29)**: read in a primary source on that date (§21).

Pins follow D1274: releases at least 14 days old when the PR opens.

---

## 1. Summary

**Loams Stream is a Rust stream processor that speaks Apache Flink's APIs.** Flink SQL scripts, the Flink SQL Client in gateway mode, the Flink JDBC driver, and tools that drive the Flink REST API run against it unchanged. State and checkpoints live in the customer's bucket, metadata in Loams Postgres. It sits behind `loams-gateway` as a backend of design 81's managed Flink. Real Flink 2.2.1 defines "correct" for every test. Loams does not fork Flink or rewrite its JVM runtime piece by piece. It takes Arroyo's Rust streaming runtime on DataFusion as the starting code (hard fork), writes its own Flink SQL front end and Flink REST surface, and puts keyed state on slatedb.

### 1.1 Decisions

| # | Decision (short) | § | Status |
|---|---|---|---|
| D2240 | **What Loams Stream is**: a Rust engine with Flink's SQL, REST and (in phases) Table API, PyFlink and DataStream surfaces, state in the bucket, behind `loams-gateway` as a backend of §81's managed Flink. Supersedes D1261 and Q1036's answer; amends §00 §7 | 1, 6 | Approved (owner, 2026-10-29) |
| D2241 | **"Full Flink API compatibility" is six layers** (SQL, REST, Table API, DataStream, connectors, checkpoints), each with a measurable bar against the oracle; the claim names a layer, a phase and a Flink version, never "100 % Flink" | 3 | Approved (owner, 2026-10-29) |
| D2242 | **The oracle is Apache Flink 2.2.1**, the same pin as D1931, run through `loams-flink`; it moves only when D1931's pin moves | 3, 15 | Approved (owner, 2026-10-29) |
| D2243 | **Base: a hybrid.** Our own Flink SQL front end, type and function layer, REST surfaces and control plane; the streaming runtime starts from a hard fork of **Arroyo**'s operator, state, checkpoint and network code; RisingWave is a donor of specific algorithms, not the base; Fluss is not an engine and is not used | 5 | Approved (owner, 2026-10-29) |
| D2244 | **Hard-fork rules**: `ostrium-labs/arroyo` on GitHub, frozen at the import commit for provenance; the code is imported into Loams' `stream/` workspace and owned there; no upstream sync, PRs or posts; NOTICE carries Arroyo's and RisingWave's attributions. Amends D1930 and D1272 | 5.5 | Approved (owner, 2026-10-29) |
| D2245 | **Where the code lives**: a new `stream/` Cargo workspace on DataFusion 55.2 and Arrow 59 (the `fabric/` majors), with `ostrium-labs/datafusion` carrying the listed streaming patches ported from Arroyo; sqlparser from crates.io with Loams' own `FlinkDialect` | 7.6 | Approved (owner, 2026-10-29) |
| D2246 | **Crates** `loams-stream-{types,sql,plan,exec,state,runtime,coord,rest,connectors,client,udf,conformance}` and the binary `loams-stream` | 7.6 | Approved (owner, 2026-10-29) |
| D2247 | **The SQL layer (L1)**: Flink 2.2's DDL, DML, queries, `SET`, statement sets, catalogs, functions and `EXPLAIN`, with the window TVFs, joins, `MATCH_RECOGNIZE` and changelog semantics of §4.1, phased by §16 | 4.1 | Approved (owner, 2026-10-29) |
| D2248 | **The planner**: Flink SQL → a Flink-typed, validated tree (Flink's type and coercion rules, not DataFusion's) → a DataFusion logical plan with streaming extension nodes → a physical operator graph; Flink's built-in functions as DataFusion UDFs with Flink semantics, from a generated matrix | 7.1 | Approved (owner, 2026-10-29) |
| D2249 | **Changelog model**: Flink's `RowKind` (`+I`, `-U`, `+U`, `-D`) as an Arrow column on every batch; Flink's changelog-mode inference (insert-only, upsert, retract), `ChangelogNormalize` and the upsert materializer at sinks | 7.2 | Approved (owner, 2026-10-29) |
| D2250 | **Time model**: event time with per-split watermarks, idleness and alignment; processing time; `TIMESTAMP_LTZ` and `table.local-time-zone`; late rows handled as Flink handles them | 7.2 | Approved (owner, 2026-10-29) |
| D2251 | **Runtime**: Arrow record batches between operators, chaining, task slots, credit-based back pressure, a data plane over Arrow IPC streams (Arroyo's network manager, extended), mini-batch | 7.3 | Approved (owner, 2026-10-29) |
| D2252 | **State**: keyed state on **slatedb** (one database per subtask, keys prefixed by key group) with a foyer cache through `loams-cache`; small operator and window state in memory with snapshots to Parquet (Arroyo's tables); `table.exec.state.ttl` honoured | 7.4 | Approved (owner, 2026-10-29) |
| D2253 | **Checkpoints**: aligned barriers, exactly-once, incremental, every 60 s by default (as D1910), retained on cancel; checkpoint metadata in Loams Postgres and a `_metadata` object on S3 | 7.5 | Approved (owner, 2026-10-29) |
| D2254 | **Savepoints**: a canonical, version- and parallelism-portable format (Parquet per operator and key group plus metadata); native savepoints are slatedb checkpoints; Flink's own savepoints are not read (state migration is Q1438) | 7.5 | Approved (owner, 2026-10-29) |
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
| D2271 | **Phases FC0–FC4** (track FC): FC0 harness, oracle and import; FC1 preview (SQL core, REST, Kafka/Iggy/Iceberg/filesystem, checkpoints); FC2 beta (temporal, lookup, `MATCH_RECOGNIZE`, CDC, JDBC, catalogs, Table API, UDFs, HA, rescaling); FC3 GA (DataStream, JARs, state migration, default flip); FC4 on demand | 16 | Approved (owner, 2026-10-29) |
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

### 5.1 Candidates (checked 2026-10-29 on GitHub; §21)

| | Arroyo | RisingWave | Apache Fluss | Feldera | DataFusion alone (House-style) |
|---|---|---|---|---|---|
| Licence | Apache-2.0 and MIT (`LICENSE-APACHE`, `LICENSE-MIT`) | Apache-2.0 (Premium features gated by a licence key in the same code) | Apache-2.0 | MIT, with Feldera Enterprise code under commercial terms (GitHub reports `NOASSERTION`) | Apache-2.0 |
| Language and size | Rust, about 3.1 MB of Rust source, plus a TypeScript UI | Rust, about 32 MB of Rust source (about ten times Arroyo), plus Java connectors | Java (about 27 MB) with a Rust client (about 4.6 MB) | Rust (about 20 MB) plus a Java (Calcite) SQL compiler (about 9 MB) | — |
| Activity | Last release v0.15.0 (2025-12-01); 72 commits since 2026-07-29; Cloudflare-owned since 2025-04 (§68) | v3.1.0 (2026-09-21), v3.0.5 (2026-10-12); very active | v1.0.0 (2026-09-22) | Very frequent releases | DataFusion 55.2.0 (the `fabric/` pin) |
| Query layer | DataFusion 54.1 (forked: 15 commits over 54.1.0 across 103 files, including "streaming SQL AST", accumulator checkpoint APIs and sliding `DISTINCT` checkpoints), sqlparser 0.62 fork, arrow-rs 58.4 fork | Its own binder, optimiser and plan IR; Postgres dialect | Not a query engine: a streaming storage (log plus primary-key tables) that Flink reads | DBSP incremental circuits; SQL compiled by Calcite in Java | Batch only: no durable operator state, checkpoints or event-time windows in core (§68 §6.1) |
| Streaming semantics | Event-time watermarks, tumbling, sliding and session windows, updating (changelog) queries, joins, Chandy-Lamport checkpoints to object storage, Kafka exactly-once, Rust and Python UDFs (verify depth in FC0) | Close to Flink's: barriers and epochs, retractable changelog, watermarks, temporal and interval joins, `EMIT ON WINDOW CLOSE`; no `MATCH_RECOGNIZE` (verify) | — | Incremental view maintenance, not Flink's event-time and changelog model | None |
| State | In-memory tables with Parquet snapshots on object storage | Hummock, a shared LSM on S3 run by compactor and meta services | — | In-memory or local storage | — |
| Control plane | Its own API, controller, compiler service, web UI (not wanted) | Meta service on Postgres, frontend, compute, compactor (four roles, D1950) | — | Its own pipeline manager | — |
| Distance to Flink | Medium: the runtime concepts match (operators, barriers, watermarks, Kafka 2PC); the dialect and REST do not | Medium on semantics, far on dialect, plan IR and deployment shape | Not applicable | Far | Far: everything streaming is to be written |

### 5.2 Weighing the options

| | (a) Fork Arroyo whole | (b) Slim-fork RisingWave | (c) DataFusion plus our own runtime | (d) **Hybrid (chosen)** |
|---|---|---|---|---|
| What we write | Flink front end, REST, catalogs, state backend; remove Arroyo's API, UI and dialect | Flink parser and binder over RisingWave's plan IR; strip batch serving, Postgres wire and meta roles; replace the deployment model with Flink's | Everything: operators, watermarks, state, checkpoints, network, connectors | Flink front end, types and functions, REST, catalogs, control plane, keyed state on slatedb; the runtime, operators, checkpoints, network and Kafka connector start from Arroyo's code imported into Loams crates |
| Code we own | About 3 MB, most of it kept | About 32 MB, much of it coupled (the frontend, meta and Hummock assume each other) | Least imported, most written | About 1.5–2 MB imported (estimate: the runtime crates, not the API, controller, compiler service or UI) |
| Rebase burden (hard fork, no sync) | Low: a frozen tree we own | High: we own a 32 MB tree whose parts assume RisingWave's meta service and dialect | None | Low: imported crates, frozen provenance |
| Shares DataFusion with House, Sail and the engine | Yes (after a port from 54 to 55) | No (its own planner) | Yes | Yes |
| Time to a Flink SQL preview (estimate) | Medium | Long: the dialect, plan IR and deployment are RisingWave's to the core | Longest | Shortest of the four |

**Why not RisingWave as the base**, although the owner named it first and `ostrium-labs/risingwave` was forked today: its streaming semantics are the closest to Flink's, but everything around them — the Postgres dialect and binder, its own plan IR, the meta service, Hummock and its compactor, four roles at every size (D1950) — would have to be either kept (so the product is RisingWave with a Flink parser in front) or rewritten (so the fork buys little). Stripping it to a Flink-shaped engine means owning ten times Arroyo's code with no upstream sync. RisingWave stays the Postgres-wire streaming database of §81, and its fork is a **donor**: specific algorithms (for example its temporal join, over-window and degree-table join state) are read and, where useful, ported file by file with attribution (Q1447).

**Why not Fluss**: it is storage, not a processor. Loams already has its own logs (Loams streams, Iggy) and the House no longer needs Fluss (D1555). A Fluss connector is on demand (Q1445).

**Why not Feldera**: its SQL compiler is Java (Calcite), its model is incremental view maintenance rather than Flink's event-time and changelog semantics, and its enterprise boundary would need a file-by-file audit before any reuse (as Databend in §75).

**Why not only DataFusion (House-style)**: the House could start from DataFusion because a batch engine was most of what it needed. A stream processor's hard parts (operator state, barriers, watermarks, exactly-once sinks, the network layer) are exactly what DataFusion lacks, and Arroyo has written them on DataFusion under a permissive licence.

### 5.3 The ruling (D2243)

The hybrid: Loams writes the **Flink-facing layers** (parser, Flink types and coercion, the function library, the planner's Flink semantics, both REST APIs, catalogs, the client jar, the JVM and Python hosts, the control plane and the slatedb state backend) and **imports Arroyo's runtime** (operators, the watermark and time machinery, checkpoint barriers and coordination, the network manager, Parquet state tables, Kafka with two-phase commit, the filesystem sink) as the starting point for its own crates.

### 5.4 The research report

The research lane's report (`loams-wt/.he0-shared/stream-engine-options.md`, with a Flink-compatibility matrix for Arroyo, RisingWave, Fluss and others) was not complete when this design was written. The owner's instruction of 2026-10-29 was not to wait. The call above rests on §5.1's own checks. FC0 Task 1 reads the report when it lands and records any finding that changes §5's weighing as a ruling; the base choice is revisited only if the report shows a blocker in Arroyo's runtime (for example a checkpoint design that cannot give exactly-once with keyed state on slatedb).

### 5.5 Hard-fork rules (D2244)

- **Provenance.** `ostrium-labs/arroyo` is created as a GitHub fork and frozen at the import commit (recorded in `stream/NOTICE` and in FC0's ruling). A blobless clone at `~/Documents/Ostriumlabs/arroyo` is the source implementers copy from.
- **Ownership.** The imported code lives in Loams' `stream/` crates, renamed, under Loams' licence headers plus the original copyright notices. There is no upstream sync, no upstream PR and no upstream post (as with the Neon fork, `ostrium-labs/loams-postgres`).
- **Patches to dependencies.** Arroyo's DataFusion, arrow-rs and sqlparser patches are reviewed one by one in FC0: each is either dropped (fixed upstream by 55.2 or not needed), reimplemented through DataFusion's extension APIs, or carried in `ostrium-labs/datafusion` on a `55.2.0/loams-stream` branch. The target is the smallest carried patch set, listed in `stream/PATCHES.md`.
- **RisingWave.** Code from `ostrium-labs/risingwave` is ported only per file, with its Apache-2.0 header and a NOTICE entry, and never from a module gated by RisingWave's licence key.

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
   ├── loams-stream-exec: operators (windows, joins, aggregates, Top-N, dedup, MATCH_RECOGNIZE, changelog)
   ├── loams-stream-state: slatedb per subtask + foyer (loams-cache); Parquet tables for small state
   ├── loams-stream-connectors: Kafka, Iggy, Iceberg (Lakekeeper), Postgres CDC, JDBC, filesystem
   └── UDF hosts (optional sidecars): JVM (Java UDFs; DataStream operators in FC3), Python
          │
 The bucket: engines/<ns>/<id>/{checkpoints,savepoints,state,jobs}/ (D1907)   Iceberg tables in loams-lake via Lakekeeper
```

### 7.1 The planner (D2248, D2259, D2276)

1. **Parse.** `loams-stream-sql` implements a `FlinkDialect` on sqlparser-rs 0.63 (crates.io) with statement-level hooks for what sqlparser lacks: Flink's `CREATE TABLE` column forms and `WATERMARK`, `TABLE(...)`/`DESCRIPTOR(...)` in TVF calls, `FOR SYSTEM_TIME AS OF`, `EXECUTE STATEMENT SET`, `COMPILE`/`EXECUTE PLAN`, Flink's `SET 'k' = 'v'`, hints, `SHOW` and `DESCRIBE` forms. sqlparser already parses `MATCH_RECOGNIZE` (added for Snowflake; verify coverage against Flink's subset). The reference grammar is Flink's Calcite parser templates at the pinned tag (`flink-sql-parser`, Apache-2.0), and every `.q` and test-program statement is a parser test.
2. **Validate and type.** A binder resolves names against Flink's catalog model (catalog, database, object; temporary and permanent; `USE CATALOG`/`USE`) and types expressions with **Flink's** rules: Flink's logical types, implicit coercion, cast and `TRY_CAST` semantics, decimal precision and scale derivation, `STRING` versus `VARCHAR(n)`, `TIMESTAMP_LTZ` and the session time zone, nullability. These differ from DataFusion's defaults, so the binder emits explicit casts and Flink-semantics functions rather than leaning on DataFusion's coercion (Arroyo's patch "Reject implicit string-numeric coercion" is the same instinct).
3. **Streaming semantics.** The planner infers each node's changelog mode (insert-only, upsert with key, retract) and time attributes (rowtime, proctime) as Flink does, inserts `ChangelogNormalize`, upsert materialisation, watermark assigners and mini-batch, and rejects what Flink rejects (for example a non-windowed aggregate feeding an append-only sink).
4. **Lower.** Into a DataFusion logical plan whose streaming nodes are extension nodes (windows, joins with state, Top-N, deduplication, watermarks, `MATCH_RECOGNIZE`, sources and sinks), optimised with DataFusion's rules plus streaming-safe ones, then into a physical operator graph with parallelism, key-group partitioning and chaining.
5. **ExecNode JSON.** A second front end reads Flink's compiled plan JSON into the same physical graph: the path for the client jar (D2259) and for differential tests. Flink's planner (Java) runs only in the client and in tests, never in the engine (D2276, Q1435).

### 7.2 Changelog and time (D2249, D2250)

- Every batch carries `_rowkind` (`Int8`: `+I`, `-U`, `+U`, `-D`) as Flink's `RowKind`. Insert-only operators skip the column's checks. Sinks and formats map it (`debezium-json` `op`, upsert-kafka tombstones, JDBC upserts and deletes, Iceberg equality deletes).
- Watermarks are generated per source split from the DDL's `WATERMARK` expression, combined as the minimum across inputs, with `scan.watermark.idle-timeout`, emit strategies and alignment groups (`scan.watermark.alignment.*`) as Flink 2.2 defines them. Late rows in window aggregates are dropped as Flink drops them; other operators follow Flink's per-operator rules.
- Processing time uses the task manager's clock; `PROCTIME()` and `CURRENT_WATERMARK()` are Flink's. `TIMESTAMP_LTZ` arithmetic and windowing on it follow `table.local-time-zone`.

### 7.3 Runtime (D2251)

Arroyo's runtime is the starting point: operators process Arrow record batches; chains fuse stateless operators in one task; tasks run on Tokio in task slots; the network layer sends Arrow IPC between task managers with credit-based flow control so back pressure reaches sources. Loams adds Flink's knobs where they change results (`table.exec.mini-batch.*`, `parallelism.default`, `pipeline.max-parallelism`, per-operator parallelism through hints) and the metrics the Flink REST and dashboard expect (records in and out, back pressure, watermarks, checkpoint durations).

### 7.4 State (D2252, Q1440)

- **Keyed state** (regular joins, unbounded aggregates, deduplication, Top-N, `MATCH_RECOGNIZE` partial matches) goes into **slatedb** (Apache-2.0, an LSM on object storage, v0.17.0 released 2026-09-29): one database per operator subtask, keys prefixed by key group and namespace (state name), values in a compact row encoding. Reads go through a foyer RAM and NVMe cache (`loams-cache`, D1557).
- **Small and window state** (window panes waiting for the watermark, source offsets, sink transactions) stays in Arroyo's in-memory tables with Parquet snapshots at each checkpoint, which suits state that is dropped when the watermark passes.
- **TTL.** `table.exec.state.ttl` and the `STATE_TTL` hint expire keyed state by compaction filters and lazy checks, as Flink's RocksDB TTL does.
- **Why slatedb over RocksDB or an own LSM**: object storage as the primary store fits "state in the bucket", checkpoints become manifest pointers rather than uploads, and it is Rust. Whether its single-writer model and checkpoint and clone features fit per-subtask databases and rescaling is verified in FC0's spike (verify); the fallback is RocksDB through `rust-rocksdb` with incremental uploads, as Flink does.

### 7.5 Checkpoints, savepoints, two-phase commit, rescaling (D2253–D2256)

- **Barriers.** The checkpoint coordinator injects numbered barriers at the sources; operators align barriers across inputs, snapshot, and forward them (Arroyo's Chandy-Lamport implementation). Sources snapshot offsets (Kafka and Iggy offsets, CDC LSNs, file positions). Unaligned checkpoints are FC4 on demand (Q1449).
- **Snapshots.** slatedb state is checkpointed in place (a durable manifest per subtask), small state as Parquet; the coordinator writes the checkpoint's `_metadata` to `engines/<ns>/<id>/checkpoints/<job>/chk-<n>/` and records it in Postgres, then signals completion.
- **Two-phase commit.** A sink pre-commits at the barrier (Kafka transaction flushed, Iceberg data files written and recorded in state, file parts uploaded, JDBC staging rows written) and commits only when the coordinator reports the checkpoint complete; on recovery, pending commits recorded in the checkpoint are committed again idempotently (Iceberg checks the checkpoint id in the snapshot summary; Kafka resumes the transaction by its transactional id).
- **Savepoints.** Canonical savepoints write each operator's state as Parquet per key group plus metadata naming operator ids (stable `uid`s derived from the plan, as Flink derives them for SQL) and the engine version. They restore at any parallelism and across engine versions. Native savepoints (slatedb checkpoints) are fast but restore only at the same version.
- **Rescaling.** State is partitioned into key groups (`pipeline.max-parallelism`, Flink's default rule); a subtask owns a contiguous key-group range. FC1 rescales by stop-with-savepoint and restore. FC2 rescales in place: new subtasks read their key-group ranges from the last checkpoint's databases.
- **Verification.** The barrier and two-phase-commit protocol is modelled in TLA+ under `spec/tla/` and run in deterministic simulation with injected failures (the practice of §31, §55 and §75 suite 8).

### 7.6 Code layout (D2245, D2246)

A new **`stream/`** Cargo workspace, beside `fabric/`, because the engine carries a DataFusion patch set that must not reach the House's workspace, and because a stream processor does not belong in the engine binary's default build (the same reason as D343). It uses DataFusion 55.2 and Arrow 59, the `fabric/` majors, so Iceberg code can be shared (`iceberg` 0.10.1 and `ostrium-labs/iceberg-rust` as `fabric/` pins them).

| Crate | Holds |
|---|---|
| `loams-stream-types` | Flink's logical types, their Arrow mapping, casts and coercion |
| `loams-stream-sql` | `FlinkDialect`, the statement AST extensions, the binder |
| `loams-stream-plan` | Changelog and time-attribute inference, lowering to DataFusion, streaming optimiser rules, physical graph, ExecNode-JSON reader, the function library |
| `loams-stream-exec` | Streaming operators (imported from Arroyo and extended) |
| `loams-stream-state` | slatedb keyed state, Parquet tables, key groups, TTL, savepoint formats |
| `loams-stream-runtime` | Task manager: tasks, chains, network, timers, watermarks, barriers |
| `loams-stream-coord` | Job manager: scheduler, checkpoint and commit coordinators, rescaling, metadata in Postgres, leader lease |
| `loams-stream-rest` | SQL Gateway REST v1–v4 and the Flink REST API |
| `loams-stream-connectors` | Kafka, Iggy, Iceberg, Postgres CDC, JDBC, filesystem, test connectors, formats |
| `loams-stream-udf` | The JVM and Python UDF host protocols (Rust side); the hosts' own code under `stream/hosts/` |
| `loams-stream-client` | The Java client jar (`stream/client-java/`, Gradle or Maven, Apache-2.0); listed here for naming |
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
| R2 | Arroyo's runtime gaps (keyed state at scale, rescaling) | slatedb and key groups designed in from FC0; the spike verifies before FC1 |
| R3 | The DataFusion patch set grows | `stream/PATCHES.md` with an owner per patch; prefer extension APIs; one port per DataFusion major |
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

Every suite reads one manifest (`stream/conformance/profile.toml`) that marks each feature supported, planned with its phase, or won't-do. The manifest generates the compatibility page and the D2268 error messages, and a test fails when the manifest and the suites disagree (§75's surface-file rule, D347).

## 16. Phases (D2271): track FC

### 16.1 The phases

| Phase | Scope | Exit gate |
|---|---|---|
| **FC0** (harness) | Rulings and versions; `ostrium-labs/arroyo` fork and import into `stream/` on DataFusion 55.2 with Arroyo's own tests green; the DataFusion patch review; the oracle harness against Flink 2.2.1 via `loams-flink`; the corpus extractors (test programs, `.q` files, Nexmark); the function and type matrix; the slatedb state spike; the scope manifest | The corpus runs on the oracle with 100 % of its own expectations; the imported runtime runs Nexmark q0 end to end with a checkpoint and a restore |
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
| **D1930** (§81 §16.2) | No Loams fork of any engine | **Amended** by D2244: Arroyo is hard-forked as a code donor |
| **D212** (§26) | DataStream on the Flink Kubernetes Operator | **Amended at FC3** by D2261; until then unchanged (Apache Flink through §81) |
| **D1900, D1909** (§81) | Managed Flink SQL on Apache Flink | **Extended** by D2264: a `backend` field; `apache` stays the default until FC3 |
| **D1974** | Compose recovery record for Flink | Kept for `backend: apache`; `backend: loams` recovers from Postgres (D2265) |
| **D1260, D341** | RisingWave is the streaming SQL companion; Flow has no Flink targets | **Unchanged** |
| **D1915, D1916, D1917, D1918, D1919, D1920** | Kafka via Iggy, CDC, credentials, gateway, authorisation, metering | **Applied** to the new backend unchanged |

Amendment notes pointing here are added to §68 and §81 and to the amended decision-log rows.

## 18. Licences and trademarks (D2273)

- **Apache Flink** (Apache-2.0): the `TestProgram` sources, `.q` files, OpenAPI files and the web dashboard may be vendored or built with their headers and a NOTICE entry; no Flink Java code runs in the engine.
- **Arroyo** (Apache-2.0 and MIT): imported code keeps its copyright lines; NOTICE names Arroyo Systems and the import commit.
- **RisingWave** (Apache-2.0): per-file ports only, never from licence-key-gated modules.
- **slatedb, DataFusion, Arrow, sqlparser-rs, iceberg-rust, foyer** (Apache-2.0).
- **Feldera**: no code reused (enterprise boundary unaudited).
- **Nexmark** (Apache-2.0) queries vendored; **TPC-DS** query text fetched at test time, never committed (its EULA, as D414 treats ClickBench).
- **Trademarks**: "Apache Flink" and "Flink" are the ASF's. The product is "Loams Stream, compatible with Apache Flink's SQL and REST APIs"; it is never called Flink.
- No AGPL, BSL, FSL, SSPL or ELv2 code is copied (D11, D1574).

## 19. Not decided here

The FC2 and FC3 task plans; prices (none in preview, as Q1403); Loams Cloud hosting (as Q1408).

## 20. Open questions (Q1430–Q1449), answered 2026-10-29

Recorded in the decision log. The owner answered all of them on 2026-10-29 with the default ("approve all defaults").

| # | Question | Default (answered by the owner, 2026-10-29) |
|---|---|---|
| Q1430 | The product and binary name? | **Loams Stream**, binary `loams-stream`, crates `loams-stream-*`; described as "compatible with Apache Flink's APIs" |
| Q1431 | The base: hybrid on Arroyo, or a slim RisingWave fork? | **Hybrid on Arroyo's runtime** (§5); RisingWave as a donor |
| Q1432 | Where does the code live? | **A new `stream/` workspace** with a minimal DataFusion patch set in `ostrium-labs/datafusion` |
| Q1433 | The oracle version: 2.2.1 or 2.3.0? | **2.2.1**, moving with D1931's pin |
| Q1434 | DataStream: embedded JVM, Rust API, or out? | **Out until FC3; then both** the embedded JVM operator host and a Rust DataStream API |
| Q1435 | A JVM planner sidecar as a fidelity fallback in production? | **No**: Flink's planner only in tests and in the client jar |
| Q1436 | Route unsupported statements to Apache Flink automatically? | **No**: explicit `backend`; the error names the feature and the `apache` backend |
| Q1437 | Flip §81's default backend to `loams` at FC3's gate? | **Yes, after the gate passes and the owner reviews it** |
| Q1438 | Migrating state from Flink savepoints? | **Source offsets in FC2; SQL-operator state through a State Processor API exporter in FC3** |
| Q1439 | Exactly-once into Iggy and the Kafka gateway? | **Iggy native by deterministic ids and deduplication in FC1 (verify); Kafka-gateway sinks at-least-once until the Iggy fork has transactions** |
| Q1440 | The keyed state store? | **slatedb 0.17.0** (spike in FC0); fallback RocksDB through `rust-rocksdb` |
| Q1441 | Upstream PRs or posts (Arroyo, DataFusion, sqlparser)? | **None**: hard fork; patches stay in Loams' forks unless the owner approves a post |
| Q1442 | Publish the compatibility page? | **Yes, from beta**: per-layer pass counts against Flink 2.2, never "100 % Flink" |
| Q1443 | When does FC0 start? | **Now** (owner, 2026-10-29) |
| Q1444 | What is "the TPC-DS stream subset"? | **The TPC-DS queries Flink 2.2.1 runs in streaming mode over bounded sources**, text fetched at test time, results internal |
| Q1445 | A Fluss connector? | **On demand (FC4)** |
| Q1446 | The Hive dialect and catalog? | **Not built** |
| Q1447 | What is `ostrium-labs/risingwave` for? | **A frozen donor** for per-file ports; §81's managed RisingWave keeps upstream images |
| Q1448 | Serve the Flink web dashboard? | **Yes in FC2**, built from the pinned tag, read-only behind the gateway |
| Q1449 | Unaligned checkpoints and the changelog state backend? | **FC4, on demand** |

## 21. Sources (checked 2026-10-29)

- **Owner**: requests of 2026-10-29 quoted above; "approve all defaults, start building asap" (via the coordinator).
- **Loams**: §00, §26 (D212), §68 §6 (D1260, D1261, D1272, Q1036), §75, §81 and `loams-wt/.he0-shared/engines-81.md` (rulings R13, R21–R26), `crates/loams-flink`, `fabric/Cargo.toml` (DataFusion 55.2.0, Arrow 59, iceberg 0.10.1), the root `Cargo.toml` (DataFusion 54.1, Arrow 58.4), on `dev` at `2a0f4c914`.
- **Arroyo**: github.com/ArroyoSystems/arroyo (GitHub API: licence, languages, releases, commits since 2026-07-29; `Cargo.toml` and its `[patch.crates-io]`; `crates/`); the compare of `ArroyoSystems/arrow-datafusion` rev `0b17b6de…` against `apache/datafusion` 54.1.0.
- **RisingWave**: github.com/risingwavelabs/risingwave and github.com/ostrium-labs/risingwave (GitHub API: licence, languages, releases).
- **Fluss**: github.com/apache/fluss (licence, languages, v1.0.0).
- **Feldera**: github.com/feldera/feldera (licence, languages).
- **Apache Flink**: github.com/apache/flink at `release-2.2.1` and `release-2.3.0` tags; `docs/static/generated/rest_v1_dispatcher.yml`, `rest_v{1..4}_sql_gateway.yml`; `SqlGatewayRestAPIVersion.java` (V4 default); `flink-table/flink-sql-client/src/test/resources/sql/*.q`, `flink-table/flink-sql-gateway/src/test/resources/sql/*.q`; `flink-table/flink-table-planner/src/test/java/org/apache/flink/table/planner/plan/nodes/exec/stream/` (test programs).
- **slatedb**: github.com/slatedb/slatedb (v0.17.0, 2026-09-29).
- **Nexmark**: github.com/nexmark/nexmark (Apache-2.0).
- **Flink CDC**: github.com/apache/flink-cdc (Apache-2.0).
