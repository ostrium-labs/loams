# 49 — Loams House in Production: Serverless ClickHouse-Compatible Analytics on the Bucket

Status: **Proposed** · 2026-10-08. The direction is the owner's, from 2026-10-08: "chDB / ClickHouse on S3 — Loams House — production ready." This document turns that into decisions **D760–D779** and open questions **Q685–Q699**, recorded in the [decision log](13-decision-log.md) with status "Proposed". Plan: [HS1](../plans/2026-10-08-hs1-house-production.md). **No code is written by this document.**

It is an addendum to [§32](32-loams-flow-fabric-house.md) (Flow, Event Fabric, House; D330–D351, approved 2026-10-02) and builds on the [FL2 plan](../plans/2026-10-01-fl2-house-sql.md) and what it has built so far ([`fl2-dependency-spike.md`](../plans/fl2-dependency-spike.md), FL2 Rulings 1–14 during execution). Where it changes an approved decision (D346, Q336's answer, Q338's answer), §22 says so and the owner decides. It also draws on [§08](08-analytics.md) (Iceberg tables), [§04](04-hot-tier.md) (the hot tier and `loams-cache`), [§19](19-console-identity-and-agents.md) and [§38](38-knative-authentik-gitops.md) (identity; MT1's verifier, D451), [§27](27-usage-hooks.md) (usage hooks), [§44](44-unified-api-and-sdks.md) (the Connect API, one port) and [§37](37-desktop-and-mobile-apps.md) §19 (Loams Desktop on Electron, AP1e).

Markers, as in §32 and §47:

- **(verify)** means not checked against a primary source, or checked only by reading docs or code that were not run. The plan task that depends on it checks it first.
- **(estimate)** means computed or reasoned, not measured. Every number in §16 is an estimate until HS1e Task 34 replaces it with a measurement on the reference hardware.
- **(measured)** means measured against the pinned `libchdb.so` v26.9.0 by FL2 Tasks 0–1 (see the spike and FL2's rulings).

---

## 1. Summary

**Loams House is a serverless, ClickHouse-compatible analytics service whose data lives as open Iceberg tables in the customer's bucket.** Queries run in **chDB** (ClickHouse as a library, `libchdb`, Apache-2.0) inside short-lived, credential-free, network-sealed **worker processes**, driven by a small **front** (`loams-fabric house`) that speaks the ClickHouse HTTP and native protocols, authenticates with Loams identities, owns DDL and `INSERT`, and commits Iceberg snapshots through Lakekeeper. Nothing is stored in ClickHouse's own MergeTree format. Tenants pay for queries, not for idle servers: a namespace with no queries has no compute.

| # | Decision (short) |
|---|---|
| D760 | **What Loams House is** and what "production ready" means: a gate list (§21), labelled **beta** until it is all ticked |
| D761 | **chDB in Loams-owned worker processes, not `clickhouse-server` and not chDB in the front's process.** The front links no libchdb. Amends FL2's "one process, many sessions" and Q336's answer for production |
| D762 | **Iceberg (Parquet) on the bucket is the only storage of record.** MergeTree on S3 disks (`s3`, `s3_plain_rewritable`, zero-copy replication) is rejected. Reaffirms D346's "never chDB storage" |
| D763 | **Three table classes**: `lake` (House-written Iceberg; the default; needs no Fabric), `realtime` (Fluss-backed, D344–D346, when the Fabric runs) and `external` (any Iceberg table in the namespace's catalog, read-only). **Amends D346** (writes are no longer only to Fluss) |
| D764 | **The lake write path**: chDB parses and shapes the rows into Parquet bytes; the front uploads them and commits an Iceberg append with optimistic concurrency; inserts are group-committed and acknowledged after the commit; retries are deduplicated by token and block hash |
| D765 | **Replacing and Summing on lake tables merge the way ClickHouse merges**: lazily, in background compaction, and at read time with `FINAL`. `OPTIMIZE TABLE … FINAL` compacts |
| D766 | **Reads go only through `house-cache`**, a loopback S3 read proxy over `loams-cache` (RAM + NVMe, checksummed, bounded, shared by workers). chDB's own filesystem cache is off; workers hold no bucket credentials |
| D767 | **The production surface `chsurface-2.0`**: ClickHouse HTTP (8123/8443) and the **native protocol (9000/9440)**, pulled forward from FL5 (amends Q338's answer); distribution stays in FL5 |
| D768 | **The native protocol is a Loams-owned sans-I/O codec** (`loams-house-native`), with `opensrv-clickhouse` as a reference only; data blocks are chDB's `Native` output |
| D769 | **Ingest from Loams streams and Fabric topics is a House pipe**: `ENGINE = LoamsStream(…)` (or `IggyTopic(…)`) plus `CREATE MATERIALIZED VIEW … TO`, exactly-once by committing source offsets in the Iceberg snapshot that holds the rows |
| D770 | **Compute lifecycle**: a per-node pool of warm workers; a worker binds to one namespace for its life and is recycled after a budget; cancel and timeout kill the worker; dedicated pools scale pods to zero and wake on the first query |
| D771 | **Tenancy and naming**: a namespace is a Lakekeeper namespace `ns_<id>`, a ClickHouse database `d` is `ns_<id>__d`, data lives under `ns/<id>/house/`; isolation classes `shared` and `dedicated` |
| D772 | **Auth and grants**: MT1's verifier on every House listener; ClickHouse credentials carry a Loams API key or access token; scopes `house:read`, `house:write`, `house:ddl`, `house:admin`; per-database and per-table checks through the `Authorizer` (D66); ClickHouse `CREATE USER`/`GRANT` answer 48 |
| D773 | **Resource governance in three places**: chDB settings, the front's watchdog and admission control, and the worker's cgroup. Per-query caps, per-namespace concurrency queues, per-node memory admission |
| D774 | **The sandbox has three layers**: a deny list over chDB's analysed query tree, chDB settings that disable host access, and an OS sandbox around the worker (no network but the proxy, Landlock filesystem rules, seccomp, no credentials) |
| D775 | **HA, backups and recovery**: a stateless front with at least two replicas; disposable workers; the catalog's Postgres on CloudNativePG with PITR; Iceberg snapshot retention for table time travel and `RestoreTable`; ClickHouse `BACKUP`/`RESTORE` answer 48 |
| D776 | **Observability and quotas**: Prometheus metrics and OTLP traces per query, redacted operator logs, and a per-namespace `_house.query_log` lake table that backs `system.query_log` and the desktop's history; per-namespace quotas and §27 usage events |
| D777 | **Conformance and performance gates**: `chsurface-2.0` at 100 % minus the allowlist on HTTP and native; the upstream stateless subset gated; ClickBench and TPC-H SF100 correctness; a driver and BI matrix; the numeric gates of §16 |
| D778 | **`loams.house.v1`**: a Connect `HouseService` (query, cancel, browse, history, explain, restore) served by the front on the House listener and proxied by the engine's main port; it is the desktop Analytics page's contract (§18) and the SDKs' `loams.house` module |
| D779 | **Packaging and platforms**: libchdb by digest per platform (Linux x86_64 and aarch64 for production; macOS arm64 and x86_64 for development and the desktop; no Windows); two images, front and worker; the desktop downloads the House on demand |

## 2. What Loams House is, and what it is not (D760)

### 2.1 The product

- **ClickHouse SQL on your bucket.** A team points `clickhouse-client`, Grafana, Metabase, Superset, DBeaver or any ClickHouse driver at a Loams endpoint and gets ClickHouse's dialect, functions, formats and error codes over tables that are plain Iceberg in their own bucket, readable by DuckDB, Trino, Spark or a real ClickHouse through the same Lakekeeper catalog.
- **Serverless.** No cluster to size. Queries run in workers that exist while there is work; a namespace with no traffic costs storage only. Metering is per query (CPU-seconds and bytes scanned, §15).
- **One of three ways into the same data.** Loams engine tables (§08, M4) and Fabric tables (§32) are Iceberg in the same catalog; the House reads them all and writes its own.
- **Compatible by test, not by claim** (D347, D348): a versioned surface, a strict allowlist and a differential harness against a pinned `clickhouse-server`.

### 2.2 Non-goals

- **No ClickHouse storage engine, Keeper or replication.** Not now, not as an option (D762).
- **No full ClickHouse.** Out of `chsurface-2.0`: `Distributed` fan-out (FL5), Tier 3 engines (FL6), mutations and lightweight deletes, projections, TTL moves, ClickHouse users/roles/quotas DDL, row policies, `SYSTEM` commands, `BACKUP`/`RESTORE`, the MySQL/Postgres/gRPC interfaces of ClickHouse, and table functions that reach outside the namespace (§13).
- **Not a stream processor.** Windowed joins with watermarks stay RisingWave's (D341). House materialized views are per-insert-block transforms, as ClickHouse's are.
- **Not the engine's SQL.** DataFusion SQL over collections (`loams.sql.v1`, Flight SQL) is the engine's own surface and stays separate; the House never queries Lance collections (Q696).

### 2.3 What exists today (checked 2026-10-08 on `dev`)

| Piece | State |
|---|---|
| `fabric/crates/loams-chdb-sys` | Built (FL2 Task 1): bindgen over the vendored `chdb.h` of chdb-core v26.9.0; `build.rs` fetches `linux-<arch>-libchdb.tar.gz` by SHA-256 and fails closed (FL2 Rulings 1–3) |
| `fabric/crates/loams-chdb` | Built (FL2 Task 1): `Engine` (process-global), `Session`, `QueryStream`, `ArrowStream`, `ChdbError`; Arrow tests in a child process |
| `fabric/crates/loams-house` | Errors only (FL2 Task 3): `ChError`, `CODES`, `HouseError`, `MidStreamBody`. **No HTTP layer, classifier, catalog, reads or writes** |
| FL1 (Iggy, Fluss, Lakekeeper, RustFS stack; `deploy/fabric/`) | **Not built** (plan status Planned); there is no `deploy/fabric/` and no `loams-fabric` binary crate |
| `conformance/clickhouse/` | Does not exist |
| `crates/loams-cache` | Built (M0.1): foyer RAM + NVMe byte-range cache with crc32c, over `loams-store` |
| MT1 verifier | Planned, not built |
| Engine Iceberg tables (M4) | Not built |

Measured facts about the pinned library that shape this design (FL2 Rulings 10–14):

| Fact (measured) | Consequence here |
|---|---|
| One engine per process; a second `chdb_connect` with different arguments returns null | Per-tenant engine settings need separate processes (D761) |
| `chdb_stream_cancel_query` does not interrupt a statement (274 s for `count()` over `numbers(1e12)`); a cancel poisons its connection; there is no `query_id` setting | The only real cancellation is killing the process; `KILL QUERY` is Loams-cooperative (D770, D773) |
| `chdb_arrow_scan` never returns and the Arrow C stream output hangs; with signal handlers disabled both **SIGSEGV** the process | FFI faults must not take down other tenants' queries (D761); buffered `Arrow`/`ArrowStream`/`Parquet`/`Native` output works and is what the House uses |
| `filesystem_cache_size_limit` is not a 26.9.0 setting; unknown server options are silently ignored | chDB's cache cannot be bounded through the C ABI; caching moves to `house-cache` (D766) |
| libchdb x86_64 is a 180.5 MB tarball unpacking to a 554 MB shared object | Packaging (D779): a front image without it, a worker image with it |

## 3. Architecture

```
 clickhouse-client, clickhouse-go/native ── 9000/9440 ──┐
 HTTP drivers, JDBC v2, Grafana, Metabase,              │       Loams console / desktop / SDKs
 Superset, curl ─────────────────────────── 8123/8443 ──┤           │ Connect: loams.house.v1
                                                        ▼           ▼ (engine main port proxies, D778)
 ┌──────────────────────── loams-fabric house  (the FRONT; no libchdb) ─────────────────────────┐
 │ listeners: ClickHouse HTTP + loams.house.v1 on one port; native TCP                          │
 │ MT1 verifier → Authorizer (D66) → classifier (sqlparser + chdb classify, FL2 Ruling 6)       │
 │ sessions, settings, quotas, admission, watchdog          query log writer (_house.query_log) │
 │ catalog (Lakekeeper REST | local)   lake writer: upload + Iceberg commit (iceberg-rust, OCC) │
 │ pipes (LoamsStream / IggyTopic consumers)   compaction scheduler                             │
 │ worker supervisor ── UDS framed protocol ──┐                                                 │
 └────────────────────────────────────────────┼─────────────────────────────────────────────────┘
                                              ▼
   ┌──────────── house-worker × N per node (libchdb; one namespace per worker) ───────────┐
   │ sandbox: netns with lo only, Landlock (read-only libchdb, private tmp), seccomp,     │
   │ no-new-privs, cgroup v2 memory.max/cpu.max, no credentials                           │
   │ chDB executes the user's text over views: icebergS3('http://127.0.0.1:<p>/…')        │
   └──────────────┬───────────────────────────────────────────────────────────────────────┘
                  │ S3 GET/HEAD/List over a forwarder bound to the worker's identity
                  ▼
   house-cache (in the front's process): prefix-scoped S3 read proxy over loams-cache (RAM + NVMe)
                  │ signed requests with the namespace's credentials
                  ▼
   Bucket (RustFS / S3 / R2 / GCS):  ns/<id>/house/<db>/<table>/{data,metadata}/…   ◄── Lakekeeper (Postgres on CNPG)
                  ▲                                                                       ▲
   Fluss tiering (realtime tables, §32)        engine M4 tables (§08)  ─────── same catalog ┘
```

**Who does what.** The front never executes user SQL; it decides, admits, authorizes and commits. The worker executes, and only the worker links libchdb. `house-cache` is the worker's only door to data. Lakekeeper is the only catalog of record in production.

## 4. The engine: chDB in worker processes (D761)

### 4.1 Options

| Option | For | Against |
|---|---|---|
| **A. chDB in Loams-owned worker processes (proposed)** | Process isolation answers FL2's measured facts: a SIGSEGV, a runaway query or a memory blow-up kills one worker, not the node; killing a worker is a cancellation that works; per-namespace engine settings become possible; a worker can be sealed (§13) | Engine boot per worker (amortized by warm pools and reuse); a framed IPC between front and worker; more processes per node |
| B. chDB in the front's process (FL2 as planned) | Simplest; no IPC | A fault in libchdb kills every session on the node; no real cancellation; one set of engine settings for every tenant; the front would hold credentials in the same address space as user-controlled SQL |
| C. Unmodified `clickhouse-server` per tenant | Native protocol, `KILL QUERY`, `system.query_log` and memory tracking for free | A long-lived stateful server per tenant (its own users, metadata disk, config); idle cost contradicts serverless; DDL and `INSERT` still have to be intercepted for Iceberg commits; union reads with the Fluss tail impossible (D342) |
| D. `clickhouse-local` processes as workers | Official binary, same engine | A process per query with no warm state; stdin/stdout only; no session state. It stays the **fallback** if `chdb-core` stops tracking ClickHouse (§20 R-HS4) |

**Decision (D761): A.** The front and the worker are two binaries of the `fabric/` workspace: `loams-fabric` (roles `house`, `ingest`, `flow`; no libchdb) and `loams-house-worker` (libchdb; started only by the front). The worker keeps chDB's own signal handlers installed (it is chDB's process), which removes the "handlers disabled → SIGSEGV" path of FL2 Ruling 10 from the front. For development and the desktop, `loams-fabric house --workers=inproc` keeps FL2's single-process mode with an explicit warning that it is not isolated (Q686).

### 4.2 The front ↔ worker protocol

A Unix-domain socket per worker, length-prefixed frames, postcard-encoded, versioned (`hsw1`):

| Frame | Direction | Content |
|---|---|---|
| `Bind` | front → worker | namespace id, isolation class, engine settings, proxy endpoint, temp-dir quota |
| `Execute` | front → worker | query id, session settings (a `SET` list), the generated view DDL, the user's text, output format, limits |
| `Input` | front → worker | body chunks for an `INSERT` (streamed into a private temp file or `chdb_stream_insert`, HS1 Task 1 decides) |
| `Chunk` | worker → front | output bytes (any declared format) |
| `Progress`, `Stats` | worker → front | rows and bytes read, result rows and bytes, elapsed, peak memory |
| `Error` | worker → front | `ChdbError` parts (code, name, message) |
| `Done` | worker → front | end of statement |

There is no `Cancel` frame: cancellation is `SIGKILL` of the worker and a `394 QUERY_WAS_CANCELLED` answer from the front (§12).

## 5. Storage: Iceberg on the bucket, never MergeTree (D762)

### 5.1 Options

| Option | For | Against |
|---|---|---|
| **A. Iceberg v2 tables (Parquet) through Lakekeeper (proposed; approved direction of D6, D45, D346)** | Open format in the customer's bucket, readable by every lakehouse engine; one catalog with the engine (§08) and the Fabric (§32); immutable files cache without invalidation; atomic multi-writer commits through the REST catalog | Iceberg commits are slower than MergeTree part writes (seconds, not milliseconds); small inserts need group commit and compaction (D764, D765); chDB's Iceberg reader is less tuned than MergeTree (perf gates, §16) |
| B. MergeTree on an `s3` disk | ClickHouse's own format and speed | Metadata on the worker's local disk: incompatible with disposable workers; one writer per table; proprietary layout no other engine reads |
| C. MergeTree on `s3_plain_rewritable` | Metadata in the bucket | Still one writer per table and server-side merges; no multi-reader coordination in the open-source engine; same lock-in |
| D. MergeTree with zero-copy replication | Shared data across replicas | Needs Keeper; disabled by default and not recommended for production by ClickHouse itself (verify the 26.9 status); the multi-writer engine ClickHouse Cloud uses (`SharedMergeTree`) is not open source |
| E. Bare Parquet files under a prefix, no catalog | Simplest | No atomic commits, no snapshots, no schema evolution, no time travel |

**Decision (D762): A.** Every House-written table is an Iceberg v2 table registered in Lakekeeper, with data files in Parquet and metadata in the bucket. No `MergeTree` table is ever created inside chDB (FL2 Global Constraint, kept and tested). chDB is a stateless reader and transformer.

### 5.2 Table classes (D763)

| Class | Written by | Read path | Needs | Engines |
|---|---|---|---|---|
| **`lake`** (default) | The House front: Iceberg append commits (D764) | Pinned Iceberg snapshot through `house-cache` | Lakekeeper (or the local catalog, §17), a bucket | `MergeTree`, `ReplacingMergeTree`, `SummingMergeTree` (D765) |
| **`realtime`** | Fluss (`INSERT` through `fluss-rs`, D346) | Iceberg snapshot ∪ Fluss tail (D345) with consistency tokens | The Fabric (FL1) | As §32 §7.4 |
| **`external`** | Someone else: the engine (M4), Fluss tiering of a non-House table, Spark, Trino | Pinned Iceberg snapshot, read-only | The table in the namespace's catalog | Shown as `Iceberg` in `system.tables`; `INSERT` and DDL answer `497 ACCESS_DENIED` |

The class is chosen at `CREATE TABLE` by `SETTINGS loams_table_class = 'lake' | 'realtime'` (default `lake`; `realtime` answers `36 BAD_ARGUMENTS` naming the missing Fabric when FL1 is not deployed). `external` tables appear without DDL: every Iceberg table in the namespace's catalog that the House did not create is listed in its Lakekeeper namespace's ClickHouse database.

**Why amend D346.** D346 made Fluss the only write path. In production that ties every House write to a JVM, ZooKeeper and a Flink tiering job (§32 FL-R3) that do not exist yet (FL1 is Planned) and do not scale to zero. Lake tables honour D1 (the bucket is the only durable truth) with no extra service. Realtime tables stay the answer for second-level freshness and keyed upserts at event rates.

### 5.3 Layout

- Lakekeeper warehouse per cluster; namespace `ns_<id>`; ClickHouse database `d` ↔ Lakekeeper namespace `ns_<id>.d` (a nested namespace; `default` ↔ `ns_<id>`). FL2 Ruling 3's flat `ns_<id>__<db>` spelling is kept for Fluss databases; Iceberg uses the nesting the REST catalog supports (verify Lakekeeper's nested namespaces; fallback: the flat spelling).
- Table location `s3://<bucket>/ns/<id>/house/<db>/<table>/`, data in `data/`, Iceberg metadata in `metadata/`.
- Partitions: §32 §7.4's `PARTITION BY` transforms. `ORDER BY` becomes the Iceberg sort order, and the writer sorts each file by it so min/max statistics prune.
- The original DDL, engine and ClickHouse types are table properties `loams.ch.ddl`, `loams.ch.engine`, `loams.ch.types` (as §32 §7.4 for Fluss), so `SHOW CREATE TABLE` returns the user's text.
- Types: FL2 Task 5's ClickHouse ↔ Fluss table, restated ClickHouse ↔ Iceberg (`UInt64` → `decimal(20,0)` with the type kept, `LowCardinality(T)` → `T`, `Enum*` → `string`, `DateTime64(p, tz)` → `timestamptz` with `p` ≤ 6 or `timestamp_ns` (v3; verify chDB's reader), and so on).

## 6. Writes on lake tables (D764, D765)

### 6.1 The path

1. The front classifies `INSERT INTO t [(cols)] VALUES … | FORMAT f <data> | SELECT …`, authorizes `house:write` on `t`, and loads `t`'s schema from the catalog.
2. A worker bound to the namespace receives the body (frames `Input`) and runs one statement that casts to `t`'s ClickHouse types, applies `DEFAULT` expressions, sorts by `ORDER BY` within each output file, and emits **Parquet** (`FORMAT Parquet`, buffered output works at 26.9.0, FL2 Ruling 14), split into files of at most `loams_lake_target_file_bytes` (default 256 MiB) and one file per partition value.
3. The front uploads the files through `loams-store` (multipart) to `data/<uuid>.parquet`, reads each footer for the Iceberg `DataFile` statistics, and commits a `fast_append` snapshot with the REST requirement `assert-ref-snapshot-id` on `main`. A conflicting concurrent commit is retried by rebasing (appends never conflict on data) up to 10 times with jitter.
4. The answer is sent after the commit: `X-ClickHouse-Summary` with `written_rows` and `written_bytes`, and `X-Loams-Snapshot-Id`.

**Group commit (async inserts).** Inserts into one table that arrive within `loams_lake_commit_interval_ms` (default 1 000 ms) or until `loams_lake_commit_bytes` (default 256 MiB) share one commit. Every waiting client is answered after that commit, which is ClickHouse's `async_insert = 1, wait_for_async_insert = 1`. With `wait_for_async_insert = 0` the client is answered when its rows are buffered, and a crash before the commit loses them, as in ClickHouse; the docs say so. Synchronous (`async_insert = 0`) inserts join the same group commit; the setting only changes when the client is answered, never whether data is durable when it is.

**Deduplication.** Lake tables deduplicate inserts by default, as ClickHouse's replicated and shared tables do: the key is `insert_deduplication_token` when given, else a SHA-256 of the insert block's canonical bytes. Keys of the last `loams_dedup_window` blocks (default 1 000, verify ClickHouse's `replicated_deduplication_window` default at 26.9) or 7 days are kept in the table's snapshot summaries (`loams.dedup.<n>`) and in a front-side cache rebuilt from them; a duplicate answers success with `written_rows = 0`, as ClickHouse does. `insert_deduplicate = 0` turns it off.

**Who commits, and with what.** The front commits with `iceberg` (iceberg-rust 0.10, Apache-2.0). It requires arrow 58 while the `fabric/` workspace is on arrow 59 (CN1 Ruling 4); the commit path exchanges no Arrow types with the rest of the binary, so both majors can be linked (HS1 Task 1 measures the build cost). chDB's own Iceberg writer (`INSERT INTO icebergS3(…)`, experimental at 26.9, verify) is rejected for production: the commit protocol, deduplication, idempotency and exactly-once pipes (D769) must be Loams' code, and the worker must not hold write credentials. Q688.

### 6.2 Engines (D765)

| Engine | On lake tables | ClickHouse's own semantics |
|---|---|---|
| `MergeTree` | Append | Same |
| `ReplacingMergeTree([ver[, is_deleted]])` | Append; **reads without `FINAL` see unmerged rows**; reads with `FINAL` (or setting `final = 1`) merge at read time (the view keeps the row with the greatest `(ver, _file_seq, _row)` per `ORDER BY` key and drops `is_deleted = 1` rows); background compaction rewrites merged files | Same: merges are eventual, `FINAL` forces them. This is closer to ClickHouse than Fluss's eager merge (§32 `merge-timing`), so the allowlist's `merge-timing` category shrinks for lake tables |
| `SummingMergeTree([cols])` | Append; `FINAL` sums at read time; compaction sums | Same |
| `Replicated*`, `ON CLUSTER` | Accepted and ignored (D344) | — |
| Others | As §32 §8.3 tiers | — |

**Compaction.** A per-cluster scheduler in the front picks tables whose small-file count, file-size skew or unmerged-row ratio crosses a threshold, runs a worker (in the namespace's pool, billed to it, Q692) that reads the chosen files and writes merged, sorted files, and commits a `replace` snapshot with `assert-ref-snapshot-id`; a conflict with a concurrent `replace` on the same files abandons the work. `OPTIMIZE TABLE t [PARTITION p] FINAL` runs compaction now and answers after the commit (no longer a no-op for lake tables; FL2 Ruling 5 stands for realtime tables). Snapshot expiry and orphan-file deletion run on the same scheduler (§11).

### 6.3 DDL

`CREATE DATABASE`, `CREATE TABLE` (Tier 1, `AS SELECT`), `DROP` (an Iceberg `purge` after the retention of §11, so `UNDROP TABLE` works within it; verify `UNDROP` parsing), `TRUNCATE` (a snapshot that removes every file), `ALTER TABLE … ADD|DROP|RENAME COLUMN` and `MODIFY COLUMN` widening (Iceberg schema evolution), `RENAME TABLE` within a namespace (catalog rename). Out: `ALTER … UPDATE|DELETE`, lightweight `DELETE`, projections, TTL, `ATTACH`/`DETACH`, dictionaries other than §32's Tier 2 plan.

## 7. Reads (D766)

For each query the front: classifies; resolves the referenced tables through chDB's own analysis (FL2 Task 7: `EXPLAIN QUERY TREE`); authorizes `house:read` per table; **pins** each table's current snapshot (or the one named by `SETTINGS loams_snapshot_id` / `loams_as_of`, Iceberg time travel); and sends the worker view DDL that maps each name to `icebergS3('http://127.0.0.1:<p>/<bucket>/<table location>', SETTINGS iceberg_metadata_file_path = '<pinned>')` with ClickHouse types restored (plus the Fluss tail stream for realtime tables, §32 §7.5, and the `FINAL` merge view for Replacing and Summing when asked). The user's text runs unchanged (FL2 Ruling 4).

**`house-cache` (D766).** A role inside the front's process, listening on a loopback port that only the per-worker forwarders reach (§13.3):

- S3 `GetObject` (with ranges), `HeadObject` and `ListObjectsV2`, path style; every request names a worker identity and is refused unless the key is under that worker's namespace prefix (`ns/<id>/`, plus the external tables' locations the catalog vouches for);
- reads through `loams-cache`'s `RangeCache` (fixed blocks, crc32c verified on every hit, RAM then NVMe; size per node from config, default 80 % of the NVMe volume); objects are immutable (Iceberg files never change), so nothing is invalidated;
- upstream requests signed with the namespace's credentials (or vended, prefix-scoped STS credentials where the store supports them), which never leave the front's process;
- metrics: hit ratio, bytes served, upstream latency, per namespace.

chDB's own filesystem cache is disabled (no `filesystem_caches_path` given; verify that chDB then caches nothing on disk). The page cache and chDB's in-memory metadata caches (`use_iceberg_metadata_files_cache`, verify) remain.

`loams-cache` and `loams-store` live in the engine workspace; the `fabric/` workspace depends on them by path, as FL1 already plans for `loams-cloudevents`. HS1 Task 0 checks that their dependency graph unifies with the Fabric's (no arrow, `object_store` version).

## 8. The ClickHouse surface `chsurface-2.0` (D767, D768)

### 8.1 Interfaces

| Interface | Port | `chsurface-2.0` |
|---|---|---|
| HTTP | 8123 (plaintext, loopback only) and 8443 (TLS) | All of §32 §8.1, plus `Authorization: Bearer <Loams token>`; progress headers sent during the query (`send_progress_in_http_headers`), not only at the end |
| Native TCP | 9000 (plaintext, loopback only) and 9440 (TLS) | Hello and handshake (server revision = the ClickHouse version chDB carries), `Query` with settings and parameters, `Data` blocks in, `Data`/`Progress`/`ProfileInfo`/`Totals`/`Extremes`/`Exception`/`EndOfStream` out, `Cancel`, `Ping`; LZ4 and ZSTD block compression with ClickHouse's checksummed framing; external tables in a query; `INSERT … VALUES` and `INSERT … FORMAT Native` |
| `loams.house.v1` Connect | the HTTP listener's paths `/loams.house.v1.*`, and the engine's main port by proxy | §18 |
| ClickHouse's MySQL, Postgres, gRPC, Arrow Flight interfaces | — | Out |

**Why the native protocol now (amends Q338's answer).** `clickhouse-client`, `clickhouse-go`'s default protocol, Grafana's ClickHouse plugin's default, `clickhouse-cpp`, Python's `clickhouse-driver` and most migration tooling speak only or first the native protocol. A production "ClickHouse on S3" that `clickhouse-client` cannot open is not one users accept. The `Distributed` and coordinator half of FL5 stays in FL5.

**How (D768).** A Loams-owned, sans-I/O codec crate `loams-house-native` (packets, varints, block framing, compression with CityHash128 checksums) and a tokio server in the front. `opensrv-clickhouse` (Apache-2.0, crates.io 0.7.0, last released 2024-02-21) is read as a reference and for its tests, not depended on: it is unmaintained and its server traits assume an in-process engine. Data blocks are chDB's `Native` output re-framed; `INSERT` blocks from the client are concatenated into a `FORMAT Native` body for the lake write path. The codec is fuzzed (§19).

### 8.2 Driver and tool matrix (D777)

| Tier | Client | Protocol | Gate |
|---|---|---|---|
| 1 | `clickhouse-client` (from the pinned server image) | native | Interactive and `--query` suites, `--multiquery` scripts, `INSERT … FORMAT` from stdin |
| 1 | `clickhouse-rs` (Rust, `clickhouse` crate) | HTTP RowBinary | FL2 Task 12's suite |
| 1 | `clickhouse-connect` (Python) | HTTP Native | FL2 Task 12's suite, plus SQLAlchemy `clickhousedb` dialect used by Superset |
| 1 | `clickhouse-go` v2 | native and HTTP | Its examples suite on both protocols |
| 1 | `clickhouse-java` JDBC v2 | HTTP | JDBC metadata calls (DBeaver and Metabase use them), prepared statements, batch inserts (verify the v2 artefact name) |
| 1 | Grafana ClickHouse data source plugin (Apache-2.0) in Grafana (AGPL-3.0, CI container only) | native and HTTP | Its query builder's generated SQL for time series, logs and tables, recorded and replayed as corpus cases |
| 1 | Metabase ClickHouse driver (Apache-2.0) in Metabase (AGPL core, CI container only) | HTTP (JDBC) | Sync of databases, tables and fields; the generated queries of a question set |
| 2 | Superset (Apache-2.0) | HTTP (`clickhouse-connect`) | Dataset sync and a chart set |
| 2 | DBeaver, Tableau (ODBC), `clickhouse-cpp`, Python `clickhouse-driver` | JDBC, HTTP, native | Manual smoke per release, recorded in the release checklist |

Grafana and Metabase are AGPL; they run unmodified as test containers that are never linked or distributed, which D11 allows (Q694 asks the owner to confirm).

## 9. Ingest (D769)

| Source | How | Delivery |
|---|---|---|
| `INSERT` over HTTP, native or `loams.house.v1` | §6 | Exactly-once with deduplication tokens or block hashes |
| **Loams streams** (the engine's WAL, §02) | A **pipe**: `CREATE TABLE q (…) ENGINE = LoamsStream('<stream>', '<format>'[, '<consumer>'])` and `CREATE MATERIALIZED VIEW mv TO t AS SELECT … FROM q`. The front consumes the stream through the engine's stream fetch API (`loams.stream.v1`, M0.3's native fetch until API1 lands), in batches of `loams_pipe_max_rows` (default 1 M) or `loams_pipe_max_delay_ms` (default 2 000) | **Exactly-once into `t`**: the consumed offsets per partition are written in the same Iceberg snapshot as the rows (summary `loams.pipe.<mv>.<partition> = <offset>`), committed with `assert-ref-snapshot-id`; on restart the pipe resumes from the last committed snapshot's offsets. Two consumers of one pipe cannot both commit: the second's requirement fails and it re-reads the offsets |
| CloudEvents on Loams streams (D270) | As above with format `CloudEvents`: the envelope's attributes become `_ce_*` columns (§32 §5.4's layout) | Same |
| **Iggy topics** (the Fabric, when deployed) | `ENGINE = IggyTopic('<stream>', '<topic>', '<format>')` with the same pipe and offsets-in-snapshot mechanism, through the Iggy SDK | Same |
| Fluss tables (realtime) | Not a pipe: a `realtime` table is already queryable (D345), and `INSERT INTO lake_t SELECT … FROM realtime_t` moves data | Exactly-once per statement with a token |
| Engine tables (M4) and links | Not ingest: engine Iceberg tables are `external` and read in place, zero-copy | — |
| Objects in the namespace's own prefix | `ENGINE = S3Queue` restricted to `ns/<id>/` paths, processed files recorded in the snapshot summary | Exactly-once per file |

Materialized views are per-insert-block transforms: a pipe batch, or an `INSERT` into a source lake table, runs each attached MV's `SELECT` over that batch (the batch is the MV's input in a worker) and commits the result to the MV's target in its own snapshot, recording the source snapshot id so a retry does not apply twice. MVs over `realtime` sources stay FL3's (§32 §8.3). Pipe lag, last error and throughput are in `system.loams_pipes` and the metrics of §15.

## 10. Compute lifecycle (D770)

### 10.1 Workers

- **Warm pool.** Each House pod keeps `min_idle_workers` (default 2) booted and unbound. Boot is libchdb load plus `chdb_connect` (HS1 Task 1 measures it; (estimate) 300–800 ms).
- **Binding.** The first query for a namespace takes an idle worker and sends `Bind`; the worker then serves only that namespace. Queries for a namespace go to its bound idle workers first.
- **Recycling.** A worker exits after `max_queries_per_worker` (default 500), `idle_unbind_after` (default 60 s idle), any cancellation or timeout, an RSS above `worker_rss_ceiling`, or any FFI error class listed as poisoning (FL2 Ruling 11). Exit is a `SIGKILL` from the front, so there is no shutdown path in libchdb to hang (the pinned header warns that close/reopen cycles are slow).
- **Per node.** At most `max_workers` (default the node's cores ÷ 2) and at most `max_workers_per_namespace` (default 8, the namespace's class may raise it).
- **Placement.** A namespace's queries go to the pods that own `(ns, table)` by D75's rendezvous hashing with bounded load, so `house-cache` warms per table (§32 §9).

### 10.2 Scale to zero

- **Tenant level** (always): a namespace with no queries has no bound workers and no cost but storage.
- **Pod level**: the `shared` pool runs at least one pod per zone (the fronts are the endpoints) and scales on `loams_house_queue_depth` and CPU through the HorizontalPodAutoscaler (KEDA where installed, verify the chart); the `dedicated` pools (one per namespace that buys isolation) scale to zero pods, and the shared front holds a query in its queue while a dedicated pod starts, up to `loams_wake_timeout_ms` (default 30 000), answering `202 TOO_MANY_SIMULTANEOUS_QUERIES` with a retry hint past it.
- **Not Knative.** Knative Serving (D441) is HTTP-only and the native protocol is TCP; the House is a plain Deployment with its own autoscaling.

### 10.3 Sessions

Session state (`session_id`: settings, current database, last consistency token) lives in the front, keyed by session id, and session-affine routing (a hash of `session_id`) keeps it on one front replica; a lost front loses the session, and the client gets `372 SESSION_NOT_FOUND` with `session_check = 1`, as after a ClickHouse restart. **Temporary tables** (`CREATE TEMPORARY TABLE`) pin one worker to the session until the session ends; at most `max_pinned_workers_per_namespace` (default 2), past which `CREATE TEMPORARY TABLE` answers `202` (Q690).

## 11. HA, backups and recovery (D775)

| Concern | Design |
|---|---|
| Front | Stateless but for sessions (§10.3); ≥ 2 replicas across zones in production; L4 balancing for 9000/9440, L7 for 8123/8443; graceful drain stops accepting, finishes running queries up to 60 s, then kills workers |
| Workers | Disposable; a lost worker fails its query with `210 NETWORK_ERROR` (FL2 Task 3 code list) and the client retries |
| Catalog | Lakekeeper ≥ 2 replicas; its Postgres on CloudNativePG (D413) with continuous archiving to the bucket and PITR; a catalog outage fails DDL and commits with `210` and reads of uncached metadata; reads of pinned, cached metadata continue |
| Data | Iceberg files and metadata in the bucket. Bucket versioning and cross-region replication are the operator's (documented); Loams never deletes a file still referenced by a retained snapshot |
| Time travel | Snapshots retained `loams_snapshot_retention` (default 7 days, at least 24 h, Q691); `SELECT … SETTINGS loams_as_of = '<timestamp>'` reads the past |
| Restore | `loams.house.v1.HouseService/RestoreTable(table, snapshot_id \| timestamp)` makes that snapshot current (an Iceberg `set-ref`), as an operation; `UNDROP TABLE` within the retention |
| Disaster recovery | Restore the catalog database to a point (CNPG PITR), then `RegisterTables` reconciles any table whose newest metadata file is newer than the catalog's pointer (Iceberg REST `register`); a runbook and a quarterly drill (HS1 Task 25) |
| ClickHouse `BACKUP`/`RESTORE` | `48 NOT_IMPLEMENTED`, with the message naming `RestoreTable` and the docs page |

## 12. Resource governance (D773)

| Limit | Default (shared class) | Enforced by | Past it |
|---|---|---|---|
| `max_memory_usage` per query | 4 GiB (class may raise to 64 GiB) | chDB setting; the worker's cgroup `memory.max` = the cap + 512 MiB | `241 MEMORY_LIMIT_EXCEEDED` (chDB's, or the front's on a cgroup OOM kill) |
| `max_execution_time` | 300 s | chDB setting; the front's watchdog kills the worker at the limit + 2 s | `159 TIMEOUT_EXCEEDED` |
| `max_threads` | 8 (class) | chDB setting; cgroup `cpu.max` | — |
| `max_bytes_to_read` per query | 1 TiB | chDB setting | chDB's code (307 `TOO_MANY_BYTES`, verify) |
| `max_result_bytes` / `max_result_rows` | 10 GiB / unlimited | chDB settings; the front counts streamed bytes | chDB's code |
| Concurrent queries per namespace | 16 running, 64 queued for ≤ 30 s | Front admission queue | `202 TOO_MANY_SIMULTANEOUS_QUERIES` |
| Node memory | Sum of admitted queries' caps ≤ 85 % of the pod's memory | Front admission | Queued, then `202` |
| Request body (decompressed) | 16 GiB per `INSERT`, decompression ratio ≤ 100 | Front | `36 BAD_ARGUMENTS` |
| Pinned workers, pipes, MVs per namespace | 2, 32, 64 | Front | `202`, `36` |

Settings a user sends above a cap are refused with `164 READONLY` naming the cap (FL2 Ruling 10, never clamped silently). `KILL QUERY WHERE query_id = '…'` (own namespace, `house:admin` for another principal's query) kills the worker and answers at once; it is declared in the surface as Loams-cooperative (FL2 Ruling 11's question, answered here).

## 13. Security (D774)

### 13.1 Threats

| Threat | Example | Layer that stops it |
|---|---|---|
| SSRF | `url('http://169.254.169.254/…')`, `s3('http://internal:9000/…')`, `remote('10.0.0.5', …)`, dictionaries with `SOURCE(HTTP(…))`, `ENGINE = URL(…)` | L1 deny list; L3 no network |
| Local file read | `file('/etc/passwd')`, `format_schema = '/…'` for Protobuf/CapnProto, `INFILE` on the server | L1; L2 `user_files_path` empty; L3 Landlock |
| Local file write | `INTO OUTFILE` (executes in the server for chDB), `file()` as an `INSERT` target | L1; L3 Landlock (write only to the private temp dir) |
| Code execution | `executable()`, `CREATE FUNCTION … EXECUTABLE`, `ENGINE = Executable` | L1; L2; L3 seccomp denies `execve` after start |
| Cross-tenant read | A qualified name for another namespace; a crafted `icebergS3` path | Front resolves names only within the namespace; L1 refuses user-written `iceberg*`; `house-cache` refuses keys outside the worker's prefix |
| Credential theft | Reading the worker's environment or memory | Workers hold no credentials; `house-cache` signs in the front |
| Host fingerprinting | `hostName()`, `system.disks`, `system.server_settings`, `getSetting` of server settings, `system.stack_trace` | L1 rewrite or `344`; L2 |
| Resource exhaustion | `numbers(1e15)`, `sleepEachRow`, zip-bomb bodies, regexp blow-up | §12 limits; chDB uses re2 |
| FFI memory corruption | A libchdb bug triggered by input | Process isolation (D761); the worker is sealed (L3), so a compromised worker reaches nothing |

### 13.2 Layers

1. **L1, the deny list** (§32 §7.8, extended): checked over chDB's analysed query tree (`EXPLAIN QUERY TREE`), so table functions in subqueries, views, CTEs, `JOIN`, `IN`, `INSERT … SELECT` and `CREATE … AS SELECT` are all caught; plus `INTO OUTFILE`, `FROM INFILE`, `format_schema*`, `CREATE FUNCTION`, `CREATE DICTIONARY` with a non-Loams source, `ATTACH`, `SYSTEM`, and the host functions. Answers `344 SUPPORT_IS_DISABLED`. Every entry has a test (FL2 Task 8).
2. **L2, chDB's own controls**: an empty `user_files_path`, `readonly = 2` for GET and read-only principals, `allow_introspection_functions = 0`, `allow_ddl = 0` inside the worker (DDL is the front's), and ClickHouse's source privileges (`GRANT READ ON URL`, `S3`, `FILE`, `REMOTE`) revoked from the worker's user if chDB's access control can be configured (verify; HS1 Task 1).
3. **L3, the OS sandbox**: the worker starts in a new network namespace whose only interface is loopback, with a Loams forwarder (no credentials) relaying `127.0.0.1:<p>` over an inherited Unix socket to `house-cache`, which identifies the worker by the socket; Landlock rules (read-only libchdb and system libraries, read-write only the worker's private temp dir); seccomp (no `ptrace`, `mount`, `bpf`, `perf_event_open`, `keyctl`, `execve` after start); `PR_SET_NO_NEW_PRIVS`; a distinct UID; rlimits; cgroup v2 limits. Where unprivileged user namespaces are unavailable to the pod, the fallback is **worker pods**: workers in their own pods with a NetworkPolicy that allows egress only to the front's forwarder port (HS1 Task 6 decides with the cluster). macOS development builds run without L3 and say so at start.

### 13.3 FFI safety

`unsafe` stays confined to `loams-chdb-sys` (FL2 Ruling 8); `loams-chdb` and everything above forbid it. The worker treats every FFI return as untrusted: lengths bounded, null buffers handled (measured), errors parsed defensively. Paths measured to hang or crash (`chdb_arrow_scan`, streaming Arrow output) are not called in production code, and a test asserts it (HS1 Task 2).

### 13.4 Supply chain

libchdb fetched by digest from `chdb-io/chdb-core` releases with `SHA256SUMS` cross-checked (FL2 Ruling 3), the header vendored (Ruling 1), an SBOM per image, ClickHouse security advisories tracked against the ClickHouse version chDB carries, and a bump within 14 days of a fixed advisory that affects the surface.

## 14. Authentication and authorization (D772)

- **One verifier.** Every House listener uses MT1's verifier (D451): Loams access tokens (§19 §5.3) and API keys (`loams_<key_id>_<secret>`, §19 §5.5). Plaintext listeners bind loopback only; non-loopback listeners require TLS (8443, 9440) — MT1's rule.
- **Mapping ClickHouse credentials.** HTTP: `Authorization: Bearer <token>`; or `X-ClickHouse-Key` / Basic password / `password` parameter carrying an API key or a token, with the user name either `default` or the key id. Native: the Hello packet's password carries the same; the user name is ignored except for display. Agents use tokens only (§19 §5.5: keys are never issued to agents).
- **The environment is the namespace.** A token's `env` audience fixes the namespace; the `database` parameter only selects a ClickHouse database inside it.
- **Scopes.** `house:read` (`SELECT`, `SHOW`, `DESCRIBE`, `EXPLAIN`, system tables), `house:write` (`INSERT`, `OPTIMIZE`), `house:ddl` (`CREATE`, `ALTER`, `DROP`, `TRUNCATE`, `RENAME`, pipes, MVs), `house:admin` (`KILL QUERY` for others, `RestoreTable`, quotas view). A principal without `house:write` gets ClickHouse's `readonly = 1` behaviour.
- **Grants.** Per-database and per-table checks through the `Authorizer` (D66; RBAC in M2, OpenFGA in M2.x) with object types `house_database` and `house_table`; `system.tables` lists only what the principal may read. Protected environments (§19 §4) need the project `admin` role for `DROP` and `TRUNCATE`.
- **ClickHouse access DDL** (`CREATE USER`, `GRANT`, `ROLE`, `ROW POLICY`, `QUOTA`, `SETTINGS PROFILE`) answers `48`, naming Loams' console and API.
- **Audit.** DDL, grants checks that fail, `KILL`, `RestoreTable` and every write's principal go to the audit log (D100).

## 15. Observability, quotas and usage (D776)

- **Metrics** (`/metrics` on the front's admin port): `loams_house_queries_total{ns,kind,status}`, `loams_house_query_seconds` (histogram), `loams_house_read_bytes_total`, `loams_house_written_bytes_total`, `loams_house_queue_depth`, `loams_house_workers{state}`, `loams_house_worker_boot_seconds`, `loams_house_worker_kills_total{reason}`, `loams_house_cache_{hits,misses,bytes}_total`, `loams_house_commit_seconds`, `loams_house_commit_conflicts_total`, `loams_house_pipe_lag_records{pipe}`, `loams_house_compaction_*`. Namespace labels only on the per-namespace series the operator enables.
- **Traces** (OTLP): one trace per query, spans for admission, worker execution, `house-cache` upstream reads and commits; the ClickHouse `query_id` is an attribute.
- **Logs.** Operator logs carry the query id, the namespace, timings and the SQL **with literals redacted**; full text only in the tenant's own query log.
- **`_house.query_log`.** A lake table per namespace (in database `_house`, hidden from `SHOW DATABASES` unless asked) written by the front in group commits every 5 s: query id, principal, start and end, kind, database, normalized query hash, full text, status, error code, rows and bytes read and written, result rows, peak memory, CPU time. It backs `system.query_log` (the namespace's own rows), the desktop's history (§18) and the `loams.house.v1` history RPC. Retention 30 days (namespace setting). A front crash loses at most the unflushed 5 s; it is a log, not a ledger.
- **`system.processes`** lists the namespace's running queries from the front.
- **Quotas** per namespace, configured by the control plane: concurrent queries, queries per minute, bytes scanned per day, CPU-seconds per day, lake storage bytes, insert bytes per day, pipes. Past a rate or concurrency quota the answer is `202`; past a volume quota (bytes scanned, CPU-seconds, storage, insert bytes) it is `164 READONLY` for writes and `202` for reads. Every quota answer names `house_quota_exceeded` and the quota in its message, and never uses `241`, which means a single query's memory.
- **Usage events** (§27): `house.query` (CPU-ms, bytes scanned, result bytes, worker-seconds), `house.insert` (rows, bytes), `house.storage` (daily bytes per table), `house.compaction` (CPU-ms). Metering and prices stay in `loams-platform` (D220); Q695 asks for the billing unit.

## 16. Performance gates (D777)

**Reference hardware** (estimate, fixed in HS1e Task 34): one House pod on a node with 16 vCPU, 64 GiB, 2 × 1.9 TB NVMe, 25 GbE; RustFS on three nodes of the same cluster for the gate runs; AWS S3 runs are informational. Baselines: `clickhouse-server` at the same ClickHouse version on the same node, (a) with data in local MergeTree and (b) reading the **same Iceberg tables** through its own `icebergS3`. ClickBench runs follow D414: queries fetched at test time, results internal, never published as ClickBench results.

| Gate | Workload | Target (estimate) |
|---|---|---|
| Iceberg overhead of the House | ClickBench 43 queries, full `hits` (≈ 100 M rows), hot cache, sum of best-of-3 | ≤ 1.3 × baseline (b) |
| Versus native MergeTree | Same | ≤ 2.5 × baseline (a) hot; recorded, not gated, cold |
| Cold cache | Same, `house-cache` and page cache dropped | ≤ 1.5 × baseline (b) cold |
| TPC-H SF100 | 22 queries | Correct; total ≤ 1.5 × baseline (b) |
| Small query latency | `SELECT 1` and a 100-row partition-pruned query on a 1 B-row table, warm worker, 32 clients | HTTP p50 ≤ 5 ms, p99 ≤ 25 ms; native same; pruned query p95 ≤ 300 ms |
| Worker boot | Spawn, load libchdb, connect, first `SELECT 1` | p95 ≤ 1 s |
| Wake a dedicated pool | Zero pods → first answer | p95 ≤ 20 s (image cached on node) |
| Bulk insert | `INSERT … FORMAT Parquet` and `RowBinary`, 10-column rows, one client | ≥ 300 MB/s and ≥ 1.5 M rows/s per pod to committed |
| Small inserts | 1 000 clients × 1 insert/s × 100 rows into one table, `wait_for_async_insert = 1` | ack p99 ≤ 2.5 s; ≤ 1 commit/s/table; 0 lost or duplicated rows over 1 h |
| Pipe throughput | `LoamsStream` → lake table, 1 KiB JSON CloudEvents | ≥ 100 000 events/s per pipe; end-to-end lag p99 ≤ 5 s |
| Concurrency | 64 concurrent ClickBench-mix clients, 30 min | No error but `202` by policy; pod RSS within its limit; p99 ≤ 4 × single-client |
| `house-cache` | Warm reads vs local NVMe file reads of the same Parquet | ≤ 10 % slower |

A target missed by more than 20 % blocks GA unless the owner accepts the measured number (Q693).

## 17. Single-node mode and the desktop (D779)

`loams-fabric house --single-node` runs the front, `house-cache` and workers on one machine with:

- the **local catalog**: an Iceberg catalog in a SQLite file beside the data (metadata pointers with compare-and-swap; tables are standard Iceberg and can be registered into Lakekeeper later with the REST `register` call). Lakekeeper is not required on a laptop (Q689);
- the store: a directory, the engine's local RustFS, or any bucket;
- loopback listeners `127.0.0.1:8123` (HTTP and Connect) and `127.0.0.1:9000` (native); auth through the local engine's tokens when the desktop runs one, else a generated local key;
- `--workers=inproc` allowed (no isolation, labelled).

Loams Desktop (AP1e) does not bundle libchdb in its installers (114–181 MB compressed per platform, §19): the Analytics page offers "Install the analytics engine", which downloads `loams-fabric`, `loams-house-worker` and `libchdb` for the platform by digest, verifies the desktop manifest signature (Q494's key) and supervises `loams-fabric house --single-node` as a second sidecar beside `loams dev` (the §37 §6.2 restart policy). On Windows, the page works against remote servers only (no Windows libchdb). Q698.

## 18. The `loams.house.v1` API and the desktop Analytics page (D778)

### 18.1 Where it is served

The front serves `loams.house.v1` on the House HTTP listener (the Connect paths do not collide with ClickHouse's `/`, `/ping` and `/?query=` handlers). The engine's main port proxies `/loams.house.v1.*` to the configured House endpoint (`[house] endpoint`), so the console, the desktop and the SDKs see one origin (§44 §4), and the engine's catalogue (`crates/loams/src/api/connect.rs`) lists `loams.house.v1` as available when an endpoint is configured and healthy, `unstable: true` until GA. `GetInstance.services[]` is how the desktop discovers it.

### 18.2 Services (AP0 conventions: `NO_SIDE_EFFECTS` reads, `idempotency_key` on mutations, `ErrorInfo.reason`, AIP-158 pages, server-streaming only)

| RPC | Request → response | Notes |
|---|---|---|
| `ExecuteQuery` | `{sql, database, settings map<string,string>, params map<string,string>, format: ARROW_IPC \| JSON_ROWS, max_rows, max_bytes, timeout_ms, query_id?, read_only bool}` → stream of `{header {columns [{name, clickhouse_type}]} \| batch {arrow_ipc bytes \| rows_json bytes} \| progress {read_rows, read_bytes, total_rows_to_read, elapsed_ms} \| summary {result_rows, result_bytes, read_rows, read_bytes, written_rows, elapsed_ms, truncated bool, query_id, snapshot_ids}}` | Same classifier, auth, limits and deny list as the ClickHouse surfaces; `read_only = true` refuses anything but reads with `house_read_only`; `max_rows` default 1 000, at most 100 000; truncation is reported, never silent |
| `CancelQuery` | `{query_id}` → `{}` | Own queries; others need `house:admin` |
| `ExplainQuery` | `{sql, database, kind: PLAN \| PIPELINE \| SYNTAX \| ESTIMATE}` → `{text}` | `NO_SIDE_EFFECTS` |
| `ListDatabases` | page → `[{name, table_count, comment}]` | |
| `ListTables` | `{database, filter, page}` → `[{database, name, class: LAKE \| REALTIME \| EXTERNAL, engine, total_rows, total_bytes, partition_by, order_by, last_modified, comment}]` | From catalog metadata, never a scan |
| `DescribeTable` | `{database, table}` → `{columns [{name, clickhouse_type, default_kind, default_expression, comment}], create_table_query, snapshots [{id, committed_at, operation, added_rows}] (latest 20), partitions [{value, rows, bytes, files}] (latest 100)}` | |
| `PreviewTable` | `{database, table, limit ≤ 1 000}` → as `ExecuteQuery` | A `SELECT * … LIMIT` the server writes, so the page never builds SQL text |
| `ListQueryHistory` | `{scope: MINE \| NAMESPACE, status?, since?, text_contains?, page}` → `[{query_id, principal, started_at, duration_ms, status, error_code, query (truncated to 4 KiB), read_rows, read_bytes, result_rows}]` | From `_house.query_log`; `NAMESPACE` needs `house:admin` |
| `GetQuery` | `{query_id}` → the full row, full text | |
| `ListPipes` | page → `[{name, source, target, lag_records, last_commit_at, last_error}]` | |
| `RestoreTable` | `{database, table, snapshot_id \| timestamp, idempotency_key}` → `loams.operations.v1.Operation` | `house:admin`, protected environments need project `admin` |

Error reasons (appended to `docs/api/reasons.md`): `house_syntax_error`, `house_unknown_table`, `house_access_denied`, `house_read_only`, `house_timeout`, `house_memory_limit`, `house_too_many_queries`, `house_quota_exceeded`, `house_disabled_function`, `house_cancelled`, `house_not_configured`; `ErrorInfo.metadata` carries `clickhouse_code` and `clickhouse_name`.

### 18.3 The desktop Analytics page (AP1e plugin `@loams/plugin-analytics`)

| Need | Contract |
|---|---|
| Is it available? | `GetInstance.services[]` lists `loams.house.v1.HouseService`; else the page shows "not configured" with the install action (§17) for the local engine, or the server's message for a remote one |
| SQL console | A ClickHouse-dialect editor (Monaco or CodeMirror with ClickHouse keywords and the function list from `system.functions` fetched once through `ExecuteQuery`); runs `ExecuteQuery` with `format = ARROW_IPC`, `read_only = true` by default, `max_rows = 1 000`; results in a virtualized grid with ClickHouse type names in headers; progress from `progress` frames; Cancel calls `CancelQuery` |
| Writes and DDL | Off until the user turns "Allow writes" on for the tab; each statement the page classifies as a write (by the server's `house_read_only` answer to a read-only run) asks for confirmation, then re-runs with `read_only = false` — the same write-confirm pattern as AP1e's SQL consoles |
| Table browser | `ListDatabases` → `ListTables` tree with class badges (lake, realtime, external); `DescribeTable` for columns, DDL, snapshots and partitions; `PreviewTable` for data |
| Query history | `ListQueryHistory(scope = MINE)` with status and text filters; open a row with `GetQuery` and load it into the editor; history is server-side and shared across devices; saved queries are local to the desktop in v0.1; a server-side store waits for demand |
| Agent tool | `house_query`: `ExecuteQuery` with `read_only = true`, `max_rows = 200`; the agent never gets `read_only = false` |
| Export | CSV and Parquet export of a result through `ExecuteQuery` with `format = JSON_ROWS` or a ClickHouse HTTP `FORMAT Parquet` download through the main-process proxy, capped at 1 GiB |

## 19. Packaging, platforms and upgrades (D779)

| Artefact | Platforms | Size (measured or from release assets, chdb-core v26.9.0) |
|---|---|---|
| `libchdb.so` (dynamic, Ruling 2) | linux-x86_64 (production), linux-aarch64 (production), macos-arm64 and macos-x86_64 (development, desktop) | tarballs 180.5 MB, 160.0 MB, 114.2 MB, 132.2 MB; x86_64 unpacks to 554 MB (measured) |
| Static archive, debuginfo | — | Not used (328 MB static; 1.5 GB debuginfo) |
| Windows | **None** — chdb-core publishes no Windows build | The House does not run on Windows; Windows clients use a remote House |
| Image `loams-house-front` | linux/amd64, linux/arm64 | `loams-fabric` without libchdb (estimate 40–60 MB) |
| Image `loams-house-worker` | linux/amd64, linux/arm64 | `loams-house-worker` + `libchdb.so` + the forwarder (estimate ≈ 600 MB uncompressed); the front and the worker can also ship as one image with both binaries for simple installs |
| Helm | subchart of the umbrella chart (D186): front Deployment, worker pods (fallback mode), HPA, PodDisruptionBudget, NetworkPolicies, ServiceMonitor | — |

**Upgrades.** `chdb-core`, the reference `clickhouse-server` image and the corpus's expected outputs move together in one PR (FL2 Global Constraint). Production pins **stable** chdb-core releases only (never `-rc`; v26.9.1-rc.1 appeared 2026-10-08) at least 14 days old, re-runs the whole conformance and performance suite, and rolls workers by draining (fronts stay, new workers start on the new library, old ones are recycled). Because storage is Iceberg, an engine upgrade never migrates data. Q697 asks whether to follow ClickHouse LTS lines instead.

**Licences (D11).** chDB (`chdb-io/chdb`, `chdb-io/chdb-core`) and ClickHouse are Apache-2.0 (GitHub licence API, read 2026-10-08), as are `clickhouse-rs`, `clickhouse-go`, `clickhouse-java`, `clickhouse-connect`, the Grafana ClickHouse plugin, the Metabase ClickHouse driver, Superset, iceberg-rust, Lakekeeper and `opensrv-clickhouse`. Grafana (AGPL-3.0) and Metabase (AGPL core) are CI-only containers. ClickBench's queries are CC BY-NC-SA 4.0 and follow D414. The NOTICE carries ClickHouse's and chDB's notices. Nothing AGPL, BSL, SSPL or ELv2 is linked or shipped.

## 20. Risks

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R-HS1 | chDB's Iceberg reader is slow or wrong on Lakekeeper-written tables (deletes, partition pruning, schema evolution) | Medium | High | HS1 Task 1 measures; the differential harness compares against `clickhouse-server` reading the same tables; upstream fixes (Q699) |
| R-HS2 | Iceberg commit latency and contention under many small inserts | Medium | Medium | Group commit (D764); per-table commit coalescing; the small-insert gate (§16) |
| R-HS3 | Worker boot cost makes per-namespace binding expensive with many small tenants | Medium | Medium | Warm pools, binding on demand, `idle_unbind_after`; measured in Task 1; if boot > 1 s, a reset-and-rebind path is evaluated |
| R-HS4 | `chdb-core` lags ClickHouse, changes its ABI, or stops | Medium | High | Pinned header and ABI tests; `clickhouse-local` workers as the fallback (§4.1 D) behind the same worker protocol |
| R-HS5 | The OS sandbox is not available in customers' Kubernetes (no unprivileged user namespaces) | Medium | Medium | Worker pods with NetworkPolicy (§13.2) |
| R-HS6 | Two Arrow majors (58 for iceberg-rust, 59 for the Fabric) inflate builds and confuse contributors | High | Low | Commit path isolated in one crate; removed when iceberg-rust moves (Q688) |
| R-HS7 | The native protocol drifts between client versions | Medium | Medium | Recorded packet captures per client version as `Bytes` corpus cases; the server revision pinned to chDB's |
| R-HS8 | "ClickHouse compatible" read as "everything ClickHouse does" | High | Medium | The generated surface page with pass rates; `48` naming what is out (FL-R6) |
| R-HS9 | Lake tables' Replacing semantics surprise users used to Fluss-merged realtime tables | Low | Low | Documented per class; `FINAL` behaves as in ClickHouse on both |
| R-HS10 | The FL1 Fabric never ships, leaving `realtime` empty | Medium | Low | Lake tables need no Fabric; `realtime` is optional in `chsurface-2.0` |

## 21. "Production ready" (D760)

Loams House leaves **beta** when every item of the HS1 plan's "Exit criteria for production" is ticked: the security review with no open high or critical finding and 24 h of fuzzing clean; `chsurface-2.0` at 100 % minus the allowlist on HTTP and native; the stateless subset at or above its gate; ClickBench and TPC-H SF100 correct; Tier 1 drivers and tools green; §16 met or accepted; HA, restore and DR drills green; quotas, usage events and observability in place; docs complete; and the desktop's Analytics page on `loams.house.v1`.

## 22. Contradictions with earlier decisions, and how they are resolved

| Earlier | Conflict | Resolution |
|---|---|---|
| **D346** (writes and DDL go to Fluss, never chDB's storage) | Lake tables are written by the House directly | **Amended by D763** (proposed): writes go to Fluss for `realtime` tables and to Iceberg through the House's commit path for `lake` tables; "never chDB's storage" stands (D762). Owner decides (Q685) |
| **Q336's answer** (one `house` process per node with per-namespace sessions; FL2 "one process, many sessions") | Workers are separate processes | **Amended by D761** (proposed) for production; the in-process mode stays for development (Q686) |
| **Q338's answer** (native protocol in FL5) | Native protocol in HS1 | **Amended by D767** (proposed): the native protocol moves to HS1; `Distributed` stays FL5 (Q687) |
| FL2 Ruling 5 (`OPTIMIZE … FINAL` is a no-op) | Compaction on lake tables | Holds for `realtime`; lake tables compact (D765) |
| FL2 Ruling 10 (session limits) | New limits table | §12 replaces it for production; FL2's values stay the defaults of the shared class where they agree |
| FL2 Ruling 11 (`KILL QUERY` cannot work as in ClickHouse) | — | Answered: Loams-cooperative kill of the worker (D773) |
| FL2 Ruling 13 (cache size not settable) | — | Answered: chDB's cache off; `house-cache` (D766) |
| §32 §7.8 deny list | Extended | §13.2 L1 is a superset |
| §32 §8.9 (users/roles/quotas DDL out; backups out) | — | Kept: `48` with pointers (D772, D775) |
| D45 / D347 | — | Unchanged; `chsurface-2.0` is a major bump of the declared surface (§32 §8.10: additions are minor, but changing `OPTIMIZE` and Replacing read semantics for lake tables is a result change, hence major, with the owner's approval through this document) |
| D51 / D343 | The `fabric/` workspace depends on `loams-cache` and `loams-store` by path | Allowed: the engine binary links nothing new; D343's direction (the engine never links the Fabric) holds |
| D441 (Knative for functions) | The House does not use Knative | Not a conflict: Knative runs D375's `http-port` contract; the House has a TCP protocol |
| D414 (ClickBench internal only) | Perf gates use ClickBench | Kept: fetched at test time, results internal |

## 23. Open questions

Recorded in the decision log as Q685–Q699 (status Proposed): lake tables as the default class (Q685); the front/worker split (Q686); the native protocol in HS1 (Q687); the Iceberg commit library and the two Arrow majors (Q688); the single-node local catalog (Q689); temporary tables on pinned workers (Q690); snapshot retention default (Q691); who pays for compaction (Q692); accepting the §16 ratios (Q693); AGPL BI tools as CI-only containers (Q694); the billing unit (Q695); House reads of Lance collections (Q696); chDB version policy (Q697); bundling libchdb in desktop packages (Q698); upstream contributions to chDB (Q699).

## 24. Sources

- **Loams**: §32 (all), §08 §1, §04 §3, §19 §4–§5, §38 (D447–D452), §27, §44 §4–§8, §37 §19, §47 (format); decisions D1, D6, D11, D45, D51, D66, D75, D100, D111, D186, D220, D270, D342–D351, D413, D414, D441, D451; FL1 and FL2 plans; `docs/plans/fl2-dependency-spike.md`; FL2 Rulings 1–14 during execution; `fabric/Cargo.toml`, `fabric/crates/{loams-chdb-sys,loams-chdb,loams-house}`, `crates/loams-cache`, `crates/loams/src/api/connect.rs` (read 2026-10-08 on `dev`).
- **chDB**: `github.com/chdb-io/chdb-core` releases v26.9.0 (2026-09-28) and v26.9.1-rc.1 (2026-10-08) asset lists and sizes, licence Apache-2.0 (GitHub API, read 2026-10-08); `github.com/chdb-io/chdb` licence Apache-2.0.
- **ClickHouse**: `github.com/ClickHouse/ClickHouse` licence Apache-2.0; HTTP and native protocol documentation; ports 8123/8443/9000/9440 (verify per row in HS1 Task 1).
- **Drivers and tools** (licences via the GitHub API, 2026-10-08): `ClickHouse/clickhouse-{rs,go,java,connect}`, `grafana/clickhouse-datasource`, `ClickHouse/metabase-clickhouse-driver`, `apache/superset` (Apache-2.0); `grafana/grafana` (AGPL-3.0); `metabase/metabase` (AGPL core, NOASSERTION in the API).
- **Iceberg**: crate `iceberg` 0.10.1 (2026-08-01; `arrow-array ^58`, `parquet ^58`); the Iceberg REST catalog spec (`assert-ref-snapshot-id`, `register`).
- **`opensrv-clickhouse`** 0.7.0 (crates.io, Apache-2.0, updated 2024-02-21).
