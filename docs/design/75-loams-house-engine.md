# 75 — The Loams House Engine: Rust, ClickHouse-Compatible, Iceberg v3, Serverless

Status: **Approved** · 2026-10-21 (written and approved the same day; the owner approved every decision and question with its default, including the amendments in §17, except Q1170, answered as: target the latest ClickHouse 26.x stable release, pinned as the oracle, with supported ClickHouse versions listed per gate application). Source: the owner's draft of 2026-10-21, "Loam House — Plan" (draft v1), which the owner pasted with the instruction "add this". This document carries that draft into the repository's design format, renames "Loam" to "Loams", uses the product names the repository has, reconciles the draft with what is designed and built, and checks the draft's **verify** items where a primary source settles them. Decisions **D1540–D1569** and questions **Q1170–Q1187** are in the [decision log](13-decision-log.md) with status **Approved** (D1570 is used, amending §7.4, §7.5 and §16.1, D1571, amending §7.1 and §16.1, and D1572, amending D1551's interim rule; D1573 is used by the House's statement time cap (#1039, amending §49 §12), D1574 for FSL, and D1575 for a minimal native protocol now; D1576–D1579 and Q1188–Q1189 are used by plan HE1's task-level HE1 and HE2 (2026-10-23, **Proposed**), which also takes D1800–D1805 and Q1320–Q1322 from the block **D1800–D1819 and Q1320–Q1329 reserved for track HE**). Plan stub: [HE1](../plans/2026-10-21-he1-loams-house-engine.md). **No code is written by this document, and nothing in it is built.**

**What it changes.** Loams House today is chDB (ClickHouse as a library) in sealed worker processes behind a Rust front, over Iceberg v2 tables in the bucket ([§32](32-loams-flow-fabric-house.md), [§49](49-loams-house-production.md), HS1). This document proposes to keep that product surface and replace the engine under it: a Rust engine on Apache Arrow and Apache DataFusion that speaks the ClickHouse dialect for the subset real applications use, stores every table as **Iceberg v3**, and runs serverless with a foyer cache. chDB stays as the Phase 0 engine (what ships now) and as the permanent test oracle. §17 lists every earlier decision this supersedes or amends; the main ones are:

- **D342** (the House SQL engine is chDB, and "a ClickHouse dialect on DataFusion" is rejected) is **reversed** at the end of Phase 2 (D1540, D1542);
- **D762** (Iceberg **v2** is the only storage of record) is **amended** to Iceberg **v3** for new tables (D1551);
- **D761, D766, D774, D779** (chDB workers, `house-cache`, the three-layer sandbox, libchdb packaging) hold while chDB runs and are **amended** for the Rust engine (D1557, D1558, D1567);
- **D767/D768** (the native protocol in HS1) are **amended**: the native protocol moves to Phase 5, on demand (D1548);
- **D345/D763** (the Fluss `realtime` class) are **amended**: the House gets its own fresh-data tier and no longer needs Apache Fluss (D1555).

Markers, as in §49 and §64–§74:

- **(verify)** means not checked against a primary source. The plan task that depends on it checks it first.
- **(estimate)** means judged or computed, not measured.
- **(unverified vendor claim)** means a number or statement from a vendor's own marketing that Loams has not reproduced.
- **(checked 2026-10-21)** means read in a primary source on that date; §4 and §21 give the source.

---

## 1. Summary

**Loams House becomes a Rust, ClickHouse-compatible analytical engine whose tables are Iceberg v3 in the customer's bucket.** Real ClickHouse applications such as Langfuse and OpenPanel run against it unchanged over the ClickHouse HTTP interface. Spark, Trino and DuckDB read the same tables through Lakekeeper. Compute scales to zero when idle and out for heavy queries and ingest bursts. Loams does not fork ClickHouse or TiFlash. It builds on DataFusion and iceberg-rust, mines the ClickHouse features that real applications use (about 20 % of ClickHouse serves about 80 % of applications, the draft's estimate), and refuses the rest with errors that name the feature.

| # | Decision (short) | Status |
|---|---|---|
| D1540 | **What Loams House becomes**: a Rust engine on Arrow and DataFusion with the ClickHouse dialect, Iceberg v3 as the table of record, serverless; the product surface (ClickHouse HTTP, `loams.house.v1`, Lakekeeper, Loams identities) stays. Supersedes D342 at the end of Phase 2 | Approved (owner, 2026-10-21) |
| D1541 | **Build, not fork**: no ClickHouse fork (rewriting pieces of it in Rust is rejected); TiFlash is not the base, only a design reference and an optional, separate HTAP companion for Loams SQL | Approved (owner, 2026-10-21) |
| D1542 | **chDB's role from now on**: the Phase 0 engine (what ships today) and the permanent differential oracle; libchdb leaves production images once the Rust engine passes Phase 2 | Approved (owner, 2026-10-21) |
| D1543 | **The compatibility profile is mined, not guessed**: run 7 or more open-source ClickHouse applications, collect `system.query_log` and migrations, rank features; a feature used by 3 or more applications is in the profile | Approved (owner, 2026-10-21) |
| D1544 | **Tier 0 (launch)**: `MergeTree`, `ReplacingMergeTree`, `PARTITION BY`, `ORDER BY`, TTL, `FINAL`, `ALTER … DELETE/UPDATE` on deletion vectors, HTTP with five formats, async inserts, parameters, the core types, joins, CTEs, windows, `ARRAY JOIN`, `LIMIT BY`, `WITH TOTALS`, the top ~200 mined functions plus `windowFunnel` (OpenPanel) and, if Langfuse v4 is the gate, its materialized view; topology syntax accepted and ignored | Approved (owner, 2026-10-21) |
| D1545 | **Tier 1 (next)**: `AggregatingMergeTree`, `SummingMergeTree`, incremental materialized views, `retention` and `sequenceMatch`, `JSON`/Variant, Parquet and Iceberg table functions, lightweight updates, skip, full-text and vector indexes | Approved (owner, 2026-10-21) |
| D1546 | **Tier 2 is not built**: queue and database table engines, legacy engines, deprecated and experimental features, geo, ML and exotic functions, dictionaries and the native protocol answer `48 NOT_IMPLEMENTED` naming the feature | Approved (owner, 2026-10-21) |
| D1547 | **The profile is the House's declared surface**: generated from tests, published as "a ClickHouse-compatible subset", with Langfuse and OpenPanel passing their own integration suites as the release gate; it is the next major version of D347's surface (`chsurface-3`) | Approved (owner, 2026-10-21) |
| D1548 | **No native protocol at launch**: the ClickHouse native TCP protocol (9000/9440) moves to Phase 5, on demand. Amends D767 and D768 | Approved (owner, 2026-10-21) |
| D1575 | **A minimal native protocol on the chDB House now** (owner, 2026-10-22; amends D1548, D767, D768 and Q1179's answer): handshake with password auth, `Query` with settings and parameters, `Native` blocks for `INSERT` and results, `Exception`, `Progress`, `ProfileInfo`, `Ping`, at revisions 54213–54460, no compression at first; 9000 loopback and 9440 TLS, never public; every statement on the HTTP interface's path. The rest of the native protocol stays in Phase 5, on demand (plan HE1, rulings R9.40 onward) | Approved (owner, 2026-10-22) |
| D1549 | **Architecture**: a stateless coordinator (parse, plan, prune, fragment), a disposable worker pool, a foyer cache, Iceberg v3 tables, Lakekeeper and the Loams metastore; state only in the bucket, the catalog and the log; one committer per table; maintenance never in the query path | Approved (owner, 2026-10-21) |
| D1550 | **Where the code lives**: new crates in the `fabric/` workspace beside the existing front (D343), with one Arrow major shared by DataFusion and iceberg-rust | Approved (owner, 2026-10-21) |
| D1551 | **Iceberg v3 is the table of record** for House-written tables; v2 tables stay readable; until iceberg-rust writes v3, the House writes v2 with position deletes. Amends D762 | Approved (owner, 2026-10-21) |
| D1552 | **Updates and deletes on deletion vectors**: `ReplacingMergeTree` deduplicates by deletion vectors plus compaction, `FINAL` deduplicates at read time, `ALTER … DELETE/UPDATE` write deletion vectors (and new rows for updates), never rewrites in the query path. Amends D765 and §49 §2.2 | Approved (owner, 2026-10-21) |
| D1553 | **Contribute the missing v3 pieces to iceberg-rust upstream** (deletion-vector write, row lineage, v3 commits, Variant as needed), with patches carried in Loams only until they merge | Approved (owner, 2026-10-21) |
| D1554 | **The commit protocol**: optimistic Iceberg commits with retry, a single committer per table under a metastore lease, batched commits; modelled in TLA+ and tested in deterministic simulation with injected failures | Approved (owner, 2026-10-21) |
| D1555 | **Ingest and fresh data**: log → in-memory parts and the log as WAL → small L0 Parquet → compacted sorted files → an Iceberg commit every N seconds per table; queries union committed data with the fresh tier; the log is a Loams stream by default and an Iggy topic when the Event Fabric runs. Amends D345 and D763 | Approved (owner, 2026-10-21) |
| D1556 | **Performance by not reading bytes**: prune by partition, file statistics, page index and bloom or skip index; coalesced range reads; dictionary encoding end to end; runtime-dispatched SIMD; `PREWHERE`-style filtering; two-level aggregation; spill to NVMe; a metrics loop that tunes cache admission, compaction and file sizes | Approved (owner, 2026-10-21) |
| D1557 | **The cache is foyer through `loams-cache`** (RAM + NVMe), in the worker process for the Rust engine; `house-cache`, the loopback S3 proxy, stays only for chDB workers. Amends D766 | Approved (owner, 2026-10-21) |
| D1558 | **Security for the Rust engine**: functions that reach outside the namespace are never implemented, so the deny list and chDB's grants become "not implemented"; workers run under `loams-sandbox`; bucket access by prefix-scoped, vended credentials. Amends D774 | Approved (owner, 2026-10-21) |
| D1559 | **The product layer (Tinybird-class)**: an NDJSON ingest API with quarantine, pipes published as authenticated endpoints, incremental materialized views and copy pipes, tokens and rate limits; endpoints go through `loams-gateway`, and callers off the tailnet reach them only through §69's edge listener | Approved (owner, 2026-10-21) |
| D1560 | **Observability on the House**: OTLP ingest into House tables, time partitioning with TTL tiering, Variant attributes, full-text through Loams' Tantivy | Approved (owner, 2026-10-21) |
| D1561 | **Metering**: CPU time plus bytes scanned, reported through Loams Meter (§63); an idle table costs only its storage. Proposes an answer to Q695 | Approved (owner, 2026-10-21) |
| D1562 | **Serverless compute**: containers by default (Kubernetes pods, ECS Fargate, Cloud Run, Cloudflare Containers), AWS Lambda for small and medium queries where the customer's cloud is AWS, microVMs through `loams-sandbox` for shared multi-tenant pools; Cloudflare Workers only route; cold-start work (cached footers and manifests, per-table snapshot summaries, warm pools) | Approved (owner, 2026-10-21) |
| D1563 | **Elasticity and isolation**: plans split into fragments by file and row group, work stealing for skew, shuffles over direct streams or an S3 exchange; separate compute groups for ingest, queries and compaction per tenant | Approved (owner, 2026-10-21) |
| D1564 | **Conformance and testing**: eight suites (real applications, the mined profile, ClickHouse stateless replay, chDB differential with SQLancer-style oracles, the function and engine matrix, Iceberg interop, performance, distributed correctness), with a per-PR, nightly cadence. Extends D349 | Approved (owner, 2026-10-21) |
| D1565 | **Phases 0–5** (track HE): Phase 0 is chDB behind the House API plus mining and the oracle harness; Phase 1 the read-only Rust engine; Phase 2 ingest, deletion vectors and the release gate; Phase 3 distributed serverless; Phase 4 Tier 1; Phase 5 on demand | Approved (owner, 2026-10-21) |
| D1566 | **Licence rules**: no BSL, GPL or AGPL code copied; ClickHouse and TiFlash code (Apache-2.0) only with attribution in NOTICE; Databend audited file by file before anything is reused; AGPL applications only as unmodified test containers | Approved (owner, 2026-10-21) |
| D1567 | **Packaging after Phase 2**: the House image no longer carries libchdb (554 MB unpacked), Windows and the desktop can run the House natively, and the light default stack gets lighter. Amends D779 | Approved (owner, 2026-10-21) |
| D1568 | **Honest labels**: the House stays **Experimental** on chDB until Phase 2's exit; the Rust engine is described as designed, then as preview, never as shipped before its gate | Approved (owner, 2026-10-21) |
| D1569 | **Vendor numbers are claims until reproduced**: ClickHouse Cloud, ObsessionDB and Tinybird figures are quoted only as their claims; Loams' own targets are estimates until measured | Approved (owner, 2026-10-21) |
| D1570 | **Gate-app engines on the chDB House now**: `VersionedCollapsingMergeTree` (read-time collapsing by sign and version, `FINAL` included) and readable `AggregatingMergeTree`, including materialized views with inner tables, move from Tier 2 and Tier 1 into the launch tier for the gate applications on today's chDB House, because OpenPanel needs them (its `sessions` table and six inner-table views, plan HE1 R4.3). The Rust engine gets them in its own later phase (HE4) | Approved (owner, 2026-10-21) |
| D1571 | **SigNoz is the third gate application** (after Langfuse and OpenPanel) and runs fully on the House, because the Software Factory uses SigNoz on the House for observability. Only SigNoz's MIT build (`signoz/signoz-community`) is used, never the `ee/` code. SigNoz speaks the native protocol and runs a clustered schema, so its gate needs the native listener and the cluster DDL forms on a one-node House (plan HE1 R9.60) | Approved (owner, 2026-10-22) |
| D1572 | **Nanosecond times in v2 lake tables**: until the House writes v3, `DateTime64(7..9)` is stored as a `long` of nanoseconds since the epoch, its ClickHouse type kept in `loams.ch.types` and restored by the read views (`fromUnixTimestamp64Nano`), so queries answer as ClickHouse does; `timestamptz_ns` once v3 is written (HE2, D1553). Amends D1551's interim rule | Approved (owner, 2026-10-22) |

## 2. Goals and non-goals

### 2.1 Goals (owner draft)

- **Run real ClickHouse applications unchanged** (Langfuse, OpenPanel and similar) against Loams House.
- **All data lives in the bucket as Iceberg v3 tables**, readable by Spark, Trino and DuckDB through the Iceberg REST catalog (Lakekeeper, D413, D1265).
- **Scale to zero** when idle; scale out for heavy queries and ingest bursts.
- **Beat managed ClickHouse** (ClickHouse Cloud, ObsessionDB) on cost per query and on cold-query latency, and match it on warm queries within a stated margin (§11). These are targets, not results.

### 2.2 Non-goals (owner draft)

- **100 % ClickHouse compatibility.** Loams targets the features most applications use and rejects the rest with clear errors (§7).
- **Deprecated, legacy and experimental ClickHouse features** (Tier 2, §7.5).
- **The native TCP protocol (port 9000) at launch.** The target clients use HTTP (8123). D1548.
- **Running ClickHouse's C++ code in production in the long run.** chDB is the Phase 0 engine and the oracle (D1542). This does not change what ships today: the House in 0.1.x runs chDB, and keeps running it until the Rust engine passes Phase 2.

Also out, as in §49 §2.2: ClickHouse's own storage engine, Keeper and replication (D762 stands); stream processing with watermarks (RisingWave's, D1260); queries over Lance collections (Q696, see §12.3).

## 3. Where Loams House stands today (origin/dev `b281c2dde`, 2026-10-21)

| Piece | State |
|---|---|
| The product | Shipped in 0.1.0 as **Experimental** (README, `docs/release/preview.md`): ClickHouse HTTP from `loams-fabric` with chDB workers, `loams.house.v1` proxied by the engine's main port, lake tables that take `INSERT` in every declared format, `CREATE TABLE … AS SELECT`, `TRUNCATE` and `OPTIMIZE TABLE` |
| Engine | chDB (`chdb-core` v26.9.0, whose `SELECT version()` is ClickHouse 26.9.2.1, HS1 R1.1) in `loams-house-worker` processes, sealed (network namespace, Landlock, seccomp, cgroups) and credential-free (D761, D774). No ClickHouse fork (owner, 2026-10-12): security is chDB's own access control first, the syntax deny list second, the OS sandbox as the boundary |
| Front | `loams-fabric house` (`fabric/crates/loams-house`): HTTP on hyper, classifier, sessions, settings, deny list, catalog, DDL, reads, writes, compaction, retention, pipes, materialized views, `S3Queue`, quotas, usage events, query log, audit, TLS (HS1 Tasks 0–13 and 15–23 have rulings in the plan) |
| Storage | Iceberg **v2** (Parquet) written by the front with iceberg-rust 0.10.1 (arrow 58, isolated in `loams-house-lake`); the local SQLite catalog on a single node, Lakekeeper in clusters |
| Cache | `house-cache` (`fabric/crates/loams-house-cache`): a loopback S3 read proxy over `loams-cache`, which is foyer 0.22 (RAM + NVMe) |
| Not built | HS1 Task 14 (Fluss `realtime` tables, blocked on FL1, which is not built), Tasks 24–38 (autoscaling, HA drills, packaging, upgrades, desktop page, `chsurface-2.0`, the native protocol, stateless/ClickBench/TPC-H gates, driver matrix, performance gates, security review, GA) |
| Default stack | The light compose default of 0.1.1 (PR #973, open) runs the House **on by default** next to the engine on redb and a local-folder bucket |
| Users of the House inside Loams | Loams Meter aggregates usage in House tables (`_meter.windows`, a `ReplacingMergeTree` read with `FINAL`, D1123); §74 reads run and task summaries in the House; RisingWave sinks into it (D1260); the factory's traces land in it (§72, open PR) |

So the owner's **Phase 0** ("embedded chDB behind the Loams House API; Iceberg v2 reads") is largely what exists. What Phase 0 adds is the mining tool, the oracle harness and the two real applications.

## 4. Facts checked on 2026-10-21

The owner's draft marked several claims **verify**. These were checked in primary sources on 2026-10-21 (GitHub repositories and the GitHub API, crates.io, official documentation and release notes; §21 lists them). What could not be settled stays marked.

### 4.1 iceberg-rust and Iceberg v3

| Item | Finding (checked 2026-10-21) |
|---|---|
| Latest release | `iceberg` 0.10.1 (2026-08-01), a packaging patch on 0.10.0 (2026-07-21). The House pins 0.10.1 today |
| v3 tracking | Epic apache/iceberg-rust#2411, "v3 Support Tracking", open |
| Deletion vectors, read | Done on `main` (the read epic #2792 closed 2026-09-03; PRs #2866, #2868, #3035), **not in any release** |
| Deletion vectors, write | **Not done**: #2580 (a deletion-vector writer and a `RowDelta` action) is open; an earlier PR (#2678) closed unmerged |
| Puffin | `PuffinReader`, `PuffinWriter` and the `deletion-vector-v1` blob type exist; a bug writing compressed footers (#2419) is open |
| Row lineage | **Partial**: table metadata tracks `next-row-id`; setting `first-row-id` (#1652), reading `_row_id` and `_last_updated_sequence_number` (#2879) and keeping lineage through rewrites (#3401) are open |
| Variant | Merged on `main` (#2188, 2026-07-16), **not in a release**; the Parquet writer's logical-type annotation is open (#2546, #2581) |
| v3 table metadata | `FormatVersion::V3` exists; the REST catalog client ignores the requested format version at create (#2923, open) |
| RisingWave | Listed as a user in the README; the draft's "RisingWave's Rust Iceberg work was upstreamed there" could not be confirmed for v3 (unverified) |

**Consequence.** The draft's caution holds: iceberg-rust reads deletion vectors on `main` but cannot write them, and row lineage is incomplete. D1551's fallback (v2 with position deletes) is needed for HE2 unless the writer lands first, and D1553's upstream work is real work, not a formality.

**The v3 specification and other engines.**

| Engine | v3 status (checked 2026-10-21) |
|---|---|
| The specification | Versions 1, 2 and 3 are "complete and adopted by the community"; version 4 is in development |
| Iceberg Java and Spark | Deletion vectors and the Variant API from 1.8.0 (2025-02-13), Variant readers and writers from 1.9.0, row lineage with Spark `MERGE` and `UPDATE` from 1.10.0 (2025-09-11); latest 1.12.0 (2026-09-30) |
| DuckDB (`iceberg` extension) | Reads **and writes** v3 Variant, `timestamp_ns`, default values, deletion vectors and row lineage; writes are merge-on-read only; rows inserted inside a transaction have a null `_row_id`. DuckDB 1.5.6 (2026-09-28) |
| Trino 484 (2026-10-16) | "Support for format version 3 is experimental"; row-level updates and deletes on v3 use deletion vectors; Variant only on v3; row lineage columns not documented |

This confirms the draft's "engine support is uneven (open-source Trino treats v3 as experimental)" and makes DuckDB and Spark the strongest interop partners for suite 6 (§15).

### 4.2 TiFlash

| Item | Finding (checked 2026-10-21) |
|---|---|
| Licence | Apache-2.0 (GitHub API); latest releases v8.5.8 (2026-08-27) and v7.5.8 (2026-09-17) |
| ClickHouse base | `ThirdPartyNotices.txt` names the ClickHouse source base as commit `30fcaeb2a3ff…`, "Auto version update to [54381]", dated 2018-04-26: **ClickHouse v1.1.54381**. The draft expected "most post-2020 features" to be missing; the base predates 2019 |
| Disaggregated storage and compute on S3 | Experimental in TiDB v7.0.0 and v7.1.0; **GA from v7.4.0** (its release notes). The current (v8.5) page describes write nodes that turn Raft logs into columnar data and upload it to S3, and stateless compute nodes that read from S3 through a local SSD cache; it works with S3-compatible stores; the two architectures cannot be switched in place or mixed in one cluster; encryption at rest is not supported |
| Rust only in the proxy | Could not confirm from the README, which lists Rust as a build prerequisite without saying where it is used. The proxy is the `contrib/tiflash-proxy` submodule from `pingcap/tidb-engine-ext`; a `contrib/tiflash-proxy-next-gen` submodule points at a private repository |

### 4.3 Databend's licence split

The root `LICENSE` is Apache-2.0. The README says "a small set of enterprise features is under Elastic 2.0" (`licenses/Elastic.txt`, `LICENSING.md`). The licence-header configuration (`.config/licenserc-ee.toml`) puts Elastic License 2.0 headers on `src/query/ee` (attach table, data masking, data sharing, fail-safe, licence, materialized views, resource management, row access policies, storage encryption, storages, streams, table refs), `src/meta/ee`, `src/bendsave` and the two `ee_main.rs` binaries. The `src/query/ee_features/*` interface crates carry Apache-2.0 headers. Elastic License 2.0 is on D11's banned list, so those directories are never read for porting (D1566).

### 4.4 Langfuse

| Item | Finding (checked 2026-10-21) |
|---|---|
| v3 and ClickHouse | "Langfuse v3 supports ClickHouse versions >= 24.3" (its self-hosting docs). `traces`, `observations` and `scores` are `ReplacingMergeTree(event_ts, is_deleted)`. Migrations are written once (`canonical/`) and rendered as `clustered/` (`ON CLUSTER`, `Replicated*MergeTree`) or `unclustered/`, chosen by `CLICKHOUSE_CLUSTER_ENABLED` (default true) |
| Where `FINAL` is used | Could not confirm by code search; HE0 Task 3 records it from `query_log` |
| v4 | Launched 2026-08-17: "an observations-first data model built on a new, wide, and (mostly) immutable ClickHouse table". `events_full` (still `ReplacingMergeTree(event_ts, is_deleted)`) and `events_core`, a lighter copy **filled by a materialized view**; write modes `legacy`, `dual` and `events_only` (the default). **v4 requires ClickHouse 25.12 or later (26.4 recommended).** v3 gets security patches until the end of January 2027. Latest release v4.66.0 (2026-10-20) |
| Licence | MIT, except `ee/`, `web/src/ee/` and `worker/src/ee/` under `ee/LICENSE`; the copyright line reads "ClickHouse, Inc." |

**Consequences.** (1) The draft's version claim "24.3 through current" fits Langfuse v3 but not v4, which needs 25.12; Q1170's default changes accordingly. (2) Langfuse v4 needs a materialized view, which the draft placed in Tier 1; by §7.1's rule (a gate application's needs are in the profile), the views Langfuse uses move into Tier 0 if v4 is the gate (Q1180).

### 4.5 OpenPanel

| Item | Finding (checked 2026-10-21) |
|---|---|
| Licence | AGPL-3.0 (GitHub API); latest v3.9.0 (a pre-release, 2026-10-21) |
| How it reaches ClickHouse | `@clickhouse/client` with `CLICKHOUSE_URL`, so HTTP |
| Buffering and metadata | Redis buffers for events, sessions, profiles and more (`packages/core/src/buffers/`); Postgres through Prisma for metadata |
| Funnel and retention functions | **`windowFunnel` is used** (`funnel.sql.ts`, `conversion.sql.ts`); the `retention()` function is not used (its retention page computes otherwise); `sequenceMatch` had no hits |

**Consequence.** `windowFunnel` is in Tier 0 by §7.1's rule (OpenPanel is a gate application); `retention` and `sequenceMatch` stay in Tier 1 until mining finds them.

### 4.6 ObsessionDB (unverified vendor claims)

From its own website: a managed ClickHouse service on AWS, Google Cloud and Azure ("Backed by Y Combinator"), "The Fastest & Most Scalable Data Warehouse for AI"; claims "sub-400 ms p99 on 18 TB" and "no performance lost at 500 QPS"; an NVMe cache mesh where reads "never leave the cache", with S3 for durability only; compute and storage separated, and compute-compute separation that gives agents isolated read-only compute; billing by compute hours; a pricing example of 10 TB on three 64 GB nodes at $4,090 a month against $7,162 elsewhere, and "20–40 % lower cost" than other managed services. **None of this is reproduced by Loams** (D1569).

### 4.7 Tinybird

Tinybird still runs managed ClickHouse ("Tinybird combines managed ClickHouse®"). Pipes are its building block and can be published as token-secured endpoints, materialized views, copy pipes (scheduled or on demand) or sinks. Every data source has a `<name>_quarantine` table for rows that do not match the schema, kept for one month. §12.1's product layer follows that shape.

### 4.8 The Rust building blocks and versions

| Item | Finding (checked 2026-10-21) |
|---|---|
| DataFusion | 55.2.0 (2026-10-07), Apache-2.0 |
| sqlparser-rs | 0.63.0 (2026-09-13), Apache-2.0; its `ClickHouseDialect` parses `PREWHERE`, `LIMIT BY`, `WITH FILL`/`INTERPOLATE`, `SETTINGS`, `FORMAT`, `ARRAY JOIN`, `GLOBAL IN`, lambdas, `Array(...)`/`Map(...)` types, `OPTIMIZE TABLE`, `INSERT … FUNCTION`/`FORMAT` and more. Parsing only: it gives no function semantics |
| foyer | 0.22.6 (2026-09-09), Apache-2.0, a hybrid RAM and disk cache; Loams pins 0.22 |
| chDB | `chdb-core` stable is still v26.9.0 (2026-09-28); v26.9.3-rc.3 (2026-10-14) is a pre-release; Apache-2.0 |
| ClickHouse | LTS lines 26.3 and 26.8, newest stable line 26.9 (all with releases on 2026-10-20); **24.3 was an LTS line** |
| AWS Lambda limits | Not checked here (verify in HE3, Q1183) |

## 5. Positioning (owner draft)

| Competitor | What it is | Loams' angle |
|---|---|---|
| ClickHouse Cloud | The reference managed ClickHouse | Open tables (Iceberg v3), scale to zero, lower idle cost |
| ObsessionDB | Managed ClickHouse with compute and storage separated and an NVMe "cache mesh"; claims sub-400 ms p99 at high concurrency, aimed at agent workloads; ClickHouse only (unverified vendor claim, §4.6) | The same cache-over-bucket idea (foyer), plus open Iceberg tables and Loams' search, vector and Postgres engines beside it |
| Tinybird | Managed ClickHouse plus a product layer (ingest API, pipes, endpoints, materialized views) | An equivalent product layer (§12) on a serverless engine |
| Observability databases | Log, trace and metric stores | OTLP ingest, Variant columns, full-text through Loams' Tantivy (§12.2) |

Vendor numbers, including ObsessionDB's pricing example, are marketing until Loams reproduces them on its own workloads (D1569). The comparison is about the design, not a benchmark claim; ClickBench results stay internal (D414).

## 6. Build or fork (D1541, D1542)

### 6.1 Fork ClickHouse and rewrite pieces in Rust: rejected

More than a million lines of C++, constant upstream churn, and a partial rewrite leaves a hybrid that is hard to build, test and ship serverless. This agrees with the owner's ruling of 2026-10-12 (no ClickHouse fork) and goes further: the long-run House runs no ClickHouse code at all.

### 6.2 Fork TiFlash: not as the base

TiFlash is the columnar analytical component of TiDB, derived from ClickHouse, with an MPP architecture, Apache-2.0 (§4.2). Reasons not to fork it for Loams House:

1. **Wrong language and an old base.** It is C++ on ClickHouse v1.1.54381, a source base from April 2018 (§4.2), before most of what ClickHouse applications use today (JSON and Variant, lightweight updates and deletes, the analyzer, most of the modern function library).
2. **Coupled to TiKV and TiDB.** Data arrives through a Raft learner via the TiFlash proxy, and queries arrive as TiDB coprocessor and MPP plans. The ClickHouse SQL surface is not its product interface. A standalone ClickHouse-compatible server would mean removing the core of what makes it TiFlash.
3. **Storage model mismatch.** Its DeltaTree engine is built for row-store replication with transactional snapshot reads. The House's table format is Iceberg v3 on the bucket. TiFlash's disaggregated storage-and-compute mode on S3 (§4.2) is the closest part to this design, and is still a TiDB-internal format.
4. **A fork means owning a large C++ codebase** that Loams then plans to replace.

**What to take from it (read, don't fork):** the delta and stable merge design (it maps to L0 → compacted Parquet and deletion vectors, §10); MPP exchange and fragment scheduling (§13.3); the disaggregated write-node and compute-node split with an S3 cache layer; its test strategy for a columnar engine. Any code read closely enough to be ported carries its Apache-2.0 attribution in NOTICE (D1566).

**Where TiFlash fits, separately.** Loams SQL is a patched TiDB (§47). TiFlash can be an optional HTAP columnar replica for it. That is an integration project independent of Loams House, and today's designs do not run it: §20 made TiFlash an optional add-on (D131), §65 left it out of the operator, and §58 lists it only as a reference for synced columnar replicas. Q1172 asks whether to start that project; the default is not now.

### 6.3 Chosen: Rust on Arrow and DataFusion

- A **ClickHouse-dialect SQL frontend** that produces DataFusion logical plans. `sqlparser-rs` has a `ClickHouseDialect` (§4.8); the House front already parses the statements it owns with it (HS1 R4.1). Queries the frontend cannot plan answer `48` naming the construct.
- **Custom operators where ClickHouse's design wins**: specialised aggregation hash tables, the `-State`/`-Merge`/`-If` combinators, `LowCardinality` dictionaries kept end to end.
- **Storage through iceberg-rust** (Apache-2.0) with Parquet. Its v3 support is incomplete (§4.1); Loams budgets time to contribute the missing pieces upstream (D1553).
- **Databend** (Rust, object-storage first, ClickHouse-like) is a source of ideas only. Its repository is Apache-2.0 except enterprise features under the Elastic License 2.0 (§4.3), so nothing is reused before a file-by-file licence audit (D1566).

**Why the earlier rejection no longer holds.** §32 (D342) rejected "a ClickHouse dialect on DataFusion" because it "rebuilds ClickHouse's function library, type rules and formats by hand; the differential harness would fail for years". The owner's plan answers both halves. It does not rebuild the whole library, only the mined profile (about 200 functions at launch, §7.3). And the harness does not fail for years, because it gates on the profile and on real applications rather than on all of ClickHouse, with chDB kept as the oracle that defines "correct" for each case (§15). What chDB in production costs is now measured: a 554 MB library per worker image, no Windows build, cancellation only by killing the process, Arrow input that crashes, and a three-layer sandbox around an engine whose surface is far larger than anything the House needs (§49 §2.3, HS1 R1.x, R5.x).

### 6.4 chDB from now on (D1542)

| Role | When | What it means |
|---|---|---|
| **The Phase 0 engine** | Now until the Rust engine passes Phase 2's gate | The House as shipped. HS1's front-side work continues where the Rust engine reuses it (§16.2). chDB-only work that the Rust engine would discard (the native protocol codec, chDB performance gates) pauses (Q1179) |
| **The oracle** | Permanently | Every mined query and every generated query runs on chDB and on the Rust engine; a mismatch is a bug or a documented incompatibility (§15) |
| **A fallback engine** | Until Phase 3's exit | A namespace can be pinned to the chDB engine behind the same front while the Rust engine matures; removed from production images after Phase 3 (D1567) |

## 7. The compatibility profile (D1543–D1547)

### 7.1 Method: mine real SQL, don't guess

1. Run Langfuse, OpenPanel and 5–10 other open-source ClickHouse applications against a pinned `clickhouse-server` (and, as a second recording, the chDB House). Candidates from the draft: Plausible, PostHog, SigNoz, Highlight, Umami, RudderStack. Each candidate's ClickHouse use and licence is checked in HE0 Task 3 (verify); AGPL applications run only as unmodified test containers (D1566).
2. Dump `system.query_log` and collect each application's migration files.
3. Extract functions, table engines, types, formats, settings and syntax forms with counts.
4. Rank by the number of applications that use each, then by frequency. **A feature used by 3 or more applications is in the profile.** A feature one gate application (Langfuse or OpenPanel) needs is in the profile regardless.

   > **Amended by D1571 (owner, 2026-10-22).** SigNoz is the third gate application, after Langfuse and OpenPanel, and runs fully on the House (its MIT build only). Its features are in the profile by this rule.

Mining and the oracle harness start in Phase 0 and never stop: each new application version is mined again, and the diff is reviewed.

### 7.2 What is known so far

- **Langfuse v3** (§4.4): supports ClickHouse 24.3 and later; `traces`, `observations` and `scores` are `ReplacingMergeTree(event_ts, is_deleted)`, so updates are central; deletes produce mutations; it partitions by month for retention; its migrations come in clustered and unclustered forms. The draft says it reads with `FINAL`; where it does was not confirmed by code search, and HE0 Task 3 records it from `query_log`. v3 gets security patches until the end of January 2027.
- **Langfuse v4** (§4.4, launched 2026-08-17): a wide, mostly immutable observations table (`events_full`, still `ReplacingMergeTree`) plus a lighter `events_core` filled by a **materialized view**; it **requires ClickHouse 25.12 or later**. ClickHouse, Inc. acquired Langfuse in January 2026 (§39; Langfuse's licence now names ClickHouse, Inc.), so Langfuse may adopt new ClickHouse features faster than other applications; the mining diff on each release catches it.
- **OpenPanel** (§4.5): events in ClickHouse, metadata in Postgres (Prisma), Redis buffers events for batched inserts, charts query the raw events table, and it connects over HTTP (`@clickhouse/client`). Its funnel and conversion queries use **`windowFunnel`**; the `retention()` function and `sequenceMatch` were not found.
- **Loams' own users** (§3): the Meter's `_meter.windows` (`ReplacingMergeTree` with `FINAL`), §74's run summaries and the factory's traces are mined like any application.

### 7.3 Tier 0: launch (D1544)

| Area | In Tier 0 |
|---|---|
| Engines | `MergeTree`, `ReplacingMergeTree` (version, `is_deleted`); `PARTITION BY`, `ORDER BY`, `PRIMARY KEY`, TTL (row deletion by partition or deletion vector), `FINAL` |
| Mutations | `ALTER … DELETE` and `ALTER … UPDATE`, mapped to deletion vectors (and appended rows for updates), not rewrites (§9) |
| Interface | HTTP (8123, and 8443 with TLS); formats `JSONEachRow`, `JSON`, `CSV`, `TSV`, `Parquet` (plus what the mining shows the gate applications use, for example `RowBinary` for some drivers); async inserts; query parameters |
| Types | integers, floats, `Decimal`, `String`, `DateTime64`, `UUID`, `Array`, `Map`, `Nullable`, `LowCardinality`, `Enum` |
| SQL | joins, CTEs, window functions, `ARRAY JOIN`, `LIMIT BY`, `WITH TOTALS` |
| Functions | the top ~200 by mined usage: date and time, string, array, JSON extraction, conditional, the `-If`/`-State`/`-Merge` combinators, `uniq*`, `quantile*`, `argMax`/`argMin`; and `windowFunnel`, because OpenPanel uses it (§4.5, moved up from the draft's Tier 1 by §7.1's rule) |
| Materialized views | Only if Langfuse v4 is a gate (Q1180): the views it uses to fill `events_core` (§4.4), moved up from Tier 1 by §7.1's rule |
| Topology syntax | `ON CLUSTER`, `ReplicatedMergeTree(...)` and `Distributed` accepted and ignored: the bucket is the replica, so clustered migrations work unchanged (D344 already accepts the first two) |

### 7.4 Tier 1: next (D1545)

- `AggregatingMergeTree`, `SummingMergeTree`, incremental materialized views.

> **Amended by D1570 (owner, 2026-10-21).** Readable `AggregatingMergeTree`, including materialized views with inner tables (`CREATE MATERIALIZED VIEW … ENGINE = AggregatingMergeTree … AS SELECT`), is in the launch tier for the gate applications on the chDB House now, because OpenPanel reads six such views; the Rust engine gets it in HE4 as this row says.

> **Amended by D1579 (Approved by the owner 2026-10-24, plan HE1).** For the Rust engine, readable `AggregatingMergeTree` (ClickHouse's own state bytes, D1800) comes in HE1 (Task 29), and its merges on insert and inner-table views firing in HE2 (Task 64), because HE2's gate needs all three gate applications fully on the Rust engine. HE4 keeps the rest of the Tier 1 aggregating surface.

- Retention and sequence functions (`retention`, `sequenceMatch`); `windowFunnel` is already Tier 0 (§7.3).
- `JSON` and Variant types, Parquet and Iceberg table functions (on the namespace's own prefix only, D1558), lightweight updates.
- Skip indexes (bloom, minmax), full-text and vector indexes, built once and shared with Loams' Tantivy and vector engines (§12.2).

### 7.5 Tier 2: not built (D1546)

These answer `48 NOT_IMPLEMENTED` with a message that names the feature and the profile page:

- The `Kafka`, `RabbitMQ`, `MySQL` and `PostgreSQL` table engines and `MaterializedMySQL`. Loams Event Fabric, Loams streams, Loams Postgres and the connectors (§33) cover these.
- Legacy engines: `Log`, `TinyLog`, `StripeLog`, `Memory`, `Buffer`, `Set`, `Join`, `GraphiteMergeTree`, `VersionedCollapsingMergeTree`.

> **Amended by D1570 (owner, 2026-10-21).** `VersionedCollapsingMergeTree` leaves Tier 2: OpenPanel, a gate application, keeps its `sessions` table in it, so the chDB House supports it now (read-time collapsing by sign and version, `FINAL` included) and the Rust engine in HE4.

> **Amended by D1579 (Approved by the owner 2026-10-24, plan HE1).** The Rust engine reads `VersionedCollapsingMergeTree` with `FINAL` in HE1 (Task 27) and merges it on insert and at compaction in HE2 (Tasks 62 and 67), not in HE4.

- Deprecated and experimental features: the old `Object('json')`, `WINDOW VIEW`, `LIVE VIEW`, the `Ordinary` database engine, the old `MergeTree` syntax.
- Geo, ML and exotic functions; dictionaries (until an application needs them); the native protocol (D1548).

Functions that reach outside the namespace (`url`, `file`, `remote`, `s3` on arbitrary URLs, `executable`, …) are not Tier 2: they are never built, for security (D1558), and answer `344 SUPPORT_IS_DISABLED` as they do today.

### 7.6 Rules (D1547)

- **Publish the profile** as "a ClickHouse-compatible subset", with the version range of Q1170, generated from tests. The draft proposed "targeting 24.3 through the current release"; Langfuse v4 needs 25.12 or later (§4.4), so the claim has to name versions per application. It is the House's declared surface in D347's sense: a file in code from which the docs page is generated, with a test that fails on drift, and the allowlist process of D348. It becomes `chsurface-3`, a major version because results change (§49 §22 set the rule).
- **Unsupported features fail loudly** and name the feature. Nothing is silently ignored except the topology syntax of §7.3.
- **Release gate**: Langfuse and OpenPanel pass their own integration tests against Loams House.

## 8. Architecture (D1549, D1550)

```
 ClickHouse clients (HTTP 8123 / 8443)        console, desktop, SDKs (loams.house.v1)
          │                                                │
 loams-gateway (http module; §69 edge listener for off-tailnet callers) ── pipes and endpoints (§12)
          │
 House front, loams-fabric house: Loams identities, sessions, admission, quotas, query log, audit
          │
 Coordinator (stateless): parse the ClickHouse dialect → DataFusion plan → prune → fragments
          │
 Worker pool (containers, Lambda or microVMs; disposable)
          │     ├── foyer cache through loams-cache (RAM + NVMe): footers, page indexes, hot column chunks
          │     └── object_store reads, coalesced range requests
          ▼
 Iceberg v3 tables (Parquet + Puffin deletion vectors) in the bucket
          ▲                                   ▲
 Lakekeeper (Iceberg REST catalog)      the Loams metastore (leases, table placement, fresh-tier offsets)
          ▲
 Ingest: Loams streams or Iggy (Loams Event Fabric) → in-memory parts, the log as WAL → L0 Parquet → compaction
```

**Principles (owner draft).**
- State lives only in the bucket, the catalog and the log. Compute is disposable.
- One committer per table; commits are batched.
- Compaction and maintenance run as separate jobs, never in the query path.

**Names.** "The Loam metastore" in the draft is the Loams metastore: the `MetaStore` trait with the embedded openraft and redb backend in the light stack and the TiKV backend at scale (D10 as amended). "The Iceberg REST catalog" is Lakekeeper in clusters (D413, D1265) and the House's local SQLite catalog on a single node (§49 §17). "Iggy (Event Fabric)" is Loams Event Fabric (Apache Iggy and Apache Fluss, D331–D333), which is designed but not built (FL1). "Gateway/auth" is `loams-gateway` (§57) in front of the House's existing front, which keeps Loams identities, scopes and grants (D772).

**What stays from today's House.** The front keeps its listeners, authentication, classifier for statements Loams owns (DDL, `INSERT`, `SET`, `USE`), sessions, admission, quotas, usage events, query log, audit, the catalog and the commit path. What changes is what executes a query: a DataFusion plan in a Rust worker instead of the user's text in a chDB worker. The `hsw1` worker protocol (HS1 R2.3) is reused for the Rust worker where it fits, so the front can run both engines during Phases 1–3.

**Where the code lives (D1550, Q1175).** New crates in `fabric/` (D343): `loams-house-sql` (the ClickHouse-dialect frontend and the planner glue), `loams-house-exec` (operators, functions, combinators), `loams-house-iceberg` (v3 reads and writes, deletion vectors; it grows from `loams-house-lake`), `loams-house-ingest` (the fresh tier and compaction). DataFusion and iceberg-rust must agree on one Arrow major; HE1 Task 0 picks a coherent set of versions at least 14 days old. The engine workspace's DataFusion (the engine's own SQL, `loams.sql.v1`) stays separate, as §49 §2.2 says.

## 9. Iceberg v3 as the table of record (D1551–D1554)

Iceberg v3 adds deletion vectors, row lineage, the Variant type, default values, geometry and geography, nanosecond timestamps and multi-argument partition transforms. Support across engines is uneven (§4.1), so interop testing matters.

| ClickHouse concept | Iceberg v3 mapping |
|---|---|
| `ORDER BY` | The table's sort order, plus the Parquet page index, bloom filters and Puffin statistics |
| `PARTITION BY toYYYYMM(...)` | A partition spec (month transform); `DROP PARTITION` is a metadata delete |
| `ReplacingMergeTree` deduplication | Deletion vectors plus compaction; `FINAL` deduplicates at read time |
| `ALTER … DELETE` | Deletion vectors (Puffin `deletion-vector-v1` blobs) |
| `ALTER … UPDATE` | A deletion vector for the old rows and appended new rows |
| `JSON` / `Dynamic` | Variant |
| Incremental materialized views and CDC | Row lineage (`_row_id`, `_last_updated_sequence_number`) |
| TTL | Partition drops where the TTL column is the partition column, else deletion vectors in a maintenance job |

Today's House already stores the ClickHouse DDL, engine and types as table properties (`loams.ch.ddl`, `loams.ch.engine`, `loams.ch.types`, §49 §5.3), so `SHOW CREATE TABLE` returns the user's text. That stays.

**Work items.**
1. Confirm iceberg-rust's v3 gaps (§4.1) and contribute Puffin deletion-vector writes and row lineage upstream (D1553).
2. An interop matrix: round-trip every v3 feature with DuckDB, Spark and Trino (§15, suite 6).
3. The commit protocol (D1554): optimistic commits with `assert-ref-snapshot-id` and retry (as today, §49 §6.1), a single committer per table holding a lease in the Loams metastore, batched commits. It is modelled in TLA+ under `spec/tla/` and tested in deterministic simulation with injected failures, the practice of §31 and §55.

**Fallback (risk R1, §19).** Until iceberg-rust writes v3, the House writes v2 tables with position deletes (merge-on-read), which today's readers already apply (HS1 R1.3), and upgrades each table to v3 in place when the writer lands. Tables created by today's House stay v2 until they are upgraded; v2 tables remain readable for good.

> **Amended by D1572 (owner, 2026-10-22).** Under this fallback a `DateTime64(7..9)` column is a `long` of nanoseconds since the epoch in a v2 table, its ClickHouse type kept in `loams.ch.types` and restored by the read views, because SigNoz (D1571) keeps its trace times in `DateTime64(9)`; it becomes `timestamptz_ns` when the House writes v3 (plan HE1 R9.61).

## 10. Ingest and fresh data (D1555)

Path: the log → in-memory parts, with the log as the WAL → small L0 Parquet files → compacted, sorted files → an Iceberg commit every N seconds per table.

- Queries union the committed Iceberg snapshot with the in-memory and L0 tier, so fresh rows are visible before the next commit. A consistency token (D345's idea, the `X-Loams-Consistency-Token` header) lets a client read its own writes.
- Small files are controlled by batched commits, size-tiered compaction and scheduled deletion-vector compaction.
- The commit rate pushes back on ingest.
- Append-only is the fast path (it matches Langfuse v4's direction); `ReplacingMergeTree` is correct first and optimised second.

**Which log (Q1177).** The draft names Iggy. Iggy is part of Loams Event Fabric, which is not built (FL1), and the light default stack does not run it. So: the WAL is a **Loams stream** (the engine's own log, §02) by default, and an **Iggy topic** when the Event Fabric runs. Either way the committed offsets are written in the same Iceberg snapshot as the rows, which is the exactly-once mechanism today's pipes already use (D769, HS1 R16.1). A direct HTTP `INSERT` is appended to the table's stream and acknowledged when the stream acknowledges it (with `wait_for_async_insert = 1` it is acknowledged after the commit, as today).

**Fluss (Q1176).** The draft does not use Apache Fluss. With its own fresh tier the House no longer needs Fluss `realtime` tables (D345, D763; HS1 Task 14, still blocked). Fluss stays in §32 as part of the Event Fabric for its own uses; the House stops depending on it.

## 11. Performance (D1556)

- **Prune before reading**: partition → file statistics → page index → bloom or skip index. Bytes not read beat SIMD.
- **Coalesce range reads** (aim for 1–8 MB), prefetch asynchronously, overlap IO and CPU.
- **Keep dictionary encoding end to end** (`LowCardinality`).
- **Runtime-dispatched SIMD kernels** (AVX2, AVX-512, NEON) for filters, hashing and string search.
- **`PREWHERE`-style filtering** on cheap columns first; **two-level aggregation**; **spill to NVMe**.
- **A metrics loop**: each query's scan, prune and cache statistics go to Loams' own Iceberg tables (today's `_house.query_log`, D776, extended); workers tune cache admission, compaction and file sizing from them.

**Targets (estimates, to be validated, never promises).**
- Warm ClickBench within 60–80 % of ClickHouse's speed through Phases 1–3, improving after (Q1182). Results stay internal (D414).
- Cold-query p99 and cost per query better than managed ClickHouse on object storage.
- A concurrency target for agent workloads, measured against ObsessionDB's published sub-400 ms p99 claim (an unverified vendor claim, §4.6).

§49 §16's gates (the House over Iceberg against `clickhouse-server` over the same tables) stay as the measuring method; the ratios are re-set for the Rust engine by Q1182, which supersedes Q693's numbers for it.

## 12. The product layer (D1559–D1561)

### 12.1 Tinybird-class features

- A streaming ingest HTTP API (NDJSON), with quarantine tables for rows that fail to parse or cast.
- Pipes (templated SQL) published as authenticated API endpoints.
- Incremental materialized views and scheduled copy pipes. (Today's House "pipes" are ingest pipes from streams and `S3Queue`, D769; the draft's pipes are query pipes. The plan names them `ingest pipe` and `endpoint pipe` to keep the two apart.)
- Tokens, rate limits and usage analytics: Loams API keys and tokens with House scopes (D772), limits through Loams Meter's gate (D1125), usage in the House.

**Where endpoints are served.** Endpoints are HTTP routes in `loams-gateway`'s `http` module (D1022). Callers on the tailnet reach them directly. Callers off the tailnet (Workers, Vercel, Lambda) reach them only through §69's edge listener, over a private link or a brokered tunnel, with environment-bound credentials (D1311). Public endpoints wait for the deferred public edge profile (D1312, Q1060); Q1187 confirms this.

### 12.2 Observability

OTLP ingest into House tables, time partitioning with TTL tiering, Variant attributes, and full-text search through Loams' Tantivy. The full-text and vector indexes of Tier 1 are built with the engine's index crates, so one index format serves both. This touches Q696 (should the House read Lance collections; the default was no): this document keeps that default and proposes sharing index **code**, not reading the engine's collections (Q1181).

### 12.3 Metering (D1561)

CPU time plus bytes scanned, reported as the House's existing usage events (`house.query`, `house.insert`, `house.storage`, `house.compaction`) to Loams Meter (§63). A table nobody queries costs only its storage. This proposes an answer to Q695 (the billing unit): both, priced by whoever runs the platform. Loams is fully open source (D1119), so the meters are open too.

## 13. Serverless, scale to zero, elastic (D1562, D1563)

### 13.1 Cold start

- Footers and manifests cached on NVMe.
- A per-table snapshot summary (file list, partition ranges, statistics) kept beside the metadata, so planning avoids reading many manifests.
- An optional warm pool of snapshot-restored workers (the `microvm` adapter of `loams-sandbox` has snapshot restore, §56).

### 13.2 Compute targets, reconciled with the BYOC designs

| Target | Use | Where it comes from |
|---|---|---|
| **Containers** (Kubernetes pods; ECS Fargate; Cloud Run; Azure Container Apps; Cloudflare Containers) | The default for coordinators and workers, and for large scans | The operator (§65); the BYOC container tier of §70 (open PR #979), where Cloudflare already places the House in its own Container (`HouseNode`) |
| **AWS Lambda** | Small and medium queries in a customer's AWS account: memory and CPU are large but bounded, and a function has a hard time limit (verify the current limits in HE3) | New for the House. §71 (open PR #979) uses Lambda for **applications** on free tiers, not for the House; a House on Lambda is a data-plane target and a Phase 3 spike |
| **microVMs** (Firecracker or Cloud Hypervisor through `loams-sandbox`, §56) | Shared multi-tenant pools in a hosted deployment | `loams-sandbox`'s `microvm` adapter (D991) |
| **Cloudflare Workers** | Edge and API routing only; too small for analytical queries | §69 and §70 already use Workers this way |

The default for large scans is Q1173 (containers, with microVMs only for shared multi-tenant pools).

### 13.3 Elasticity and isolation

- A plan is split into fragments by file and row group; workers steal work to absorb skew; shuffles go over direct streams between workers or through an S3 exchange when workers cannot reach each other (Lambda).
- Separate compute groups per tenant for ingest, queries and compaction, so a burst of one never starves the others. Compaction is billed to the namespace it serves (as §49 Q692 proposed).
- The cache tier is foyer through `loams-cache` (D1557), with pluggable backends as planned for Loams (§04).

### 13.4 Scale to zero in the light stack

On a single machine (the light default stack, and the desktop) the coordinator and one worker run in the front's process group; "scale to zero" means no worker process while no query runs. A Rust worker starts in milliseconds rather than chDB's 136 ms p50 plus a cold 884 ms `dlopen` (HS1 R1.12) (estimate, measured in HE1).

## 14. Security and tenancy for the Rust engine (D1558)

Today's three layers (§49 §13) exist because ClickHouse can do far more than the House allows: read host files, open URLs, run executables. The Rust engine has no such functions: they are never implemented, and the planner has no way to reach the network or the file system except through the namespace's scoped object store. So:

- **L1 (the deny list)** becomes a refusal in the planner for anything outside the profile; the `344` answers for the security-denied functions stay so clients see the same codes.
- **L2 (chDB's grants and settings)** has no counterpart; there is nothing to withhold.
- **L3 (the OS sandbox)** stays, through `loams-sandbox` (§56, D991, D1002), because the engine still parses untrusted SQL and decodes untrusted Parquet. Single-tenant installs may run with `sandbox: off` behind an explicit flag, as §70 allows for Cloudflare Containers (open PR #979).
- **Credentials.** Workers are Loams code, so they may hold prefix-scoped, short-lived credentials vended for the namespace (`ns/<id>/house/`) instead of reading through the `house-cache` proxy. Hosted multi-tenant pools still vend per query.
- Authentication, scopes, grants and audit stay in the front (D772), unchanged.

## 15. Conformance and testing (D1564)

| # | Suite | What | Cadence |
|---|---|---|---|
| 1 | **Real-application gate** | Langfuse and OpenPanel run their own integration suites against Loams House | Every PR that touches the House |
| 2 | **Mined-profile suite** | Queries extracted by the mining (§7.1), with expected outputs recorded from `clickhouse-server` | Every PR |
| 3 | **ClickHouse stateless replay** | `tests/queries/0_stateless` `.sql` and `.reference` files (Apache-2.0), filtered to Tier 0 and 1 features; the runner normalises float formatting, ordering and output format; pass rate per category on a dashboard (D349's harness, reused) | Every PR |
| 4 | **Differential testing against chDB** (and optionally a hosted ClickHouse) | Grammar-based query generation plus SQLancer-style oracles (TLP, NoREC); each mismatch is triaged as a bug or a documented incompatibility | Nightly |
| 5 | **Function and engine matrix** | Generated from chDB's `system.functions` and `system.table_engines`, each marked supported, unsupported or won't-do | On each chDB pin |
| 6 | **Iceberg conformance** | Cross-engine round trips per v3 feature (DuckDB, Spark, Trino) | Every PR that touches storage |
| 7 | **Performance gates** | ClickBench (fetched at test time, D414) plus a Tinybird-style workload and an observability workload, tracked per commit | Nightly, with regression alerts |
| 8 | **Distributed correctness** | TLA+ for the commit and compaction protocol; deterministic simulation with injected failures (§31, §55) | Every PR that touches the protocol; simulation nightly |

Suites 1–3 and 6 run per PR; 4 and 7 nightly. All of it runs on GitHub-hosted runners; Langfuse and OpenPanel run as unmodified containers pinned by digest.

## 16. Phases (D1565): track HE

### 16.1 The phases

| Phase | Scope | Exit criteria |
|---|---|---|
| **HE0** | chDB behind the House API (largely built, §3); Iceberg v2 reads; the mining tool; the oracle harness | A mining report for 7 or more applications; Langfuse and OpenPanel run on the Phase 0 House |
| **HE1** | The DataFusion engine with the ClickHouse dialect; read-only on Iceberg v2 and v3; the HTTP interface | The Tier 0 query set passes; at least 50 % of the filtered stateless tests pass |
| **HE2** | Ingest, compaction, deletion-vector writes, `ReplacingMergeTree`, mutations; the HTTP ingest API and endpoint pipes | Langfuse and OpenPanel pass fully; the Iceberg interop matrix is green. The Rust engine becomes the House's default (D1542) |
| **HE3** | The distributed serverless executor; cold-start work; aggregation and join optimisations | Scale to zero and burst scale-out demonstrated; the ClickBench target met (internal) |
| **HE4** | Tier 1: `AggregatingMergeTree`, `SummingMergeTree`, materialized views, funnel functions, Variant, skip, full-text and vector indexes; `VersionedCollapsingMergeTree` (D1570). On the chDB House, `VersionedCollapsingMergeTree` and readable `AggregatingMergeTree` with inner-table views are built before HE1 for the gate applications (D1570); HE4 brings them to the Rust engine | The Tier 1 suite passes; the product layer in beta |
| **HE5** | Tier 2 items on demand and, optionally, the native protocol | Driven by user requests |

> **Amended by D1571 (owner, 2026-10-22).** SigNoz joins Langfuse and OpenPanel in the HE0 and HE2 exit criteria: it runs on the Phase 0 House in HE0 and passes fully in HE2. Because SigNoz speaks only the native protocol, the native listener HE5 lists is built for the chDB House in HE0 for this gate application, with the cluster DDL forms (`ON CLUSTER`, `Replicated*`, `Distributed`) a one-node House accepts.

> **Amended by D1579 and D1801 (Approved by the owner 2026-10-24, plan HE1).** The Rust engine gets the gate applications' Tier 1 pieces early: `VersionedCollapsingMergeTree` and readable `AggregatingMergeTree` in HE1, their writes and the views in HE2 (D1579). At HE2's gate the Rust engine becomes the default. chDB is then the oracle, and an opt-in engine per namespace until HE3's exit, when libchdb leaves the production images (D1801, reconciling D1542 with §6.4 and D1567). The fresh tier, the NDJSON ingest API and endpoint pipes finish in HE2 after the gate, not as part of it.

Mining and the oracle harness start in HE0 and never stop.

### 16.2 How HE meets HS1

HS1 (design §49) is the plan that built today's House. It continues for the engine-independent parts the Rust engine reuses: the front, authentication and TLS, the catalog and commit path, retention, quotas and usage, the query log, observability, HA and DR, packaging, upgrades and the desktop's Analytics page on `loams.house.v1` (HS1 Tasks 20–28 where unfinished). These chDB-specific HS1 tasks pause until the owner decides (Q1179): Task 14 (Fluss tables, superseded by D1555), Tasks 30–31 (the native protocol, moved to HE5 by D1548) and Task 34's chDB performance gates (which become HE3's baselines). `chsurface-2.0` (HS1 Task 29) is replaced by the mined profile (`chsurface-3`, D1547).

## 17. What this supersedes or amends

| Earlier | What it says | Change (approved 2026-10-21) |
|---|---|---|
| **D342** (§32 §7.2) | The House SQL engine is chDB; "a ClickHouse dialect on DataFusion" is rejected | **Superseded at HE2's exit** by D1540 and D1542; chDB stays as the Phase 0 engine and the oracle. §6.3 gives the reasons |
| **D343** | The House runs in `loams-fabric`, never in the engine binary | Kept; D1550 places the new crates in `fabric/` |
| **D344** | ClickHouse engines map onto Fluss and Iceberg | The Iceberg half kept and extended to v3 (§9); the Fluss half superseded with D345 |
| **D345, D763 (`realtime` class)** | House reads union Iceberg and the Fluss tail; a Fluss-backed `realtime` table class | **Amended** by D1555: the House's own fresh tier on a Loams stream or Iggy topic replaces the Fluss tail (Q1176) |
| **D346** | Writes and DDL go to Fluss | Already amended by D763 (lake tables); D1555 removes the remaining Fluss path from the House |
| **D347, D348, D349** | The declared surface, the allowlist, differential proof against `clickhouse-server` | Kept as mechanisms; the surface's content becomes the mined profile (`chsurface-3`, D1547); D1564 adds chDB as an oracle, SQLancer-style oracles and the real-application gate |
| **D351, FL5** | Track FL; distribution through a chDB coordinator with `-State`/`-Merge` | FL5's distribution is replaced by HE3's DataFusion executor |
| **D760** | What Loams House is: chDB in sealed workers | **Amended** by D1540: the same product, on a Rust engine after HE2 |
| **D761, D770** | chDB only in Loams-owned worker processes; per-namespace worker binding | Hold for chDB workers; the Rust worker reuses the pool and `hsw1` where it fits (§8) |
| **D762** | Iceberg **v2** is the only storage of record; MergeTree on S3 rejected | **Amended** by D1551: Iceberg **v3**, v2 as the fallback and for old tables; MergeTree stays rejected |
| **D765** and §49 §2.2, §6.3 | Replacing and Summing merge lazily; mutations, lightweight deletes and TTL are out | **Amended** by D1552 and D1544: deletion vectors, `ALTER … DELETE/UPDATE` and TTL are Tier 0 |
| **D766** | Workers read only through `house-cache` | **Amended** by D1557: the Rust worker uses `loams-cache` (foyer) in process |
| **D767, D768** | The native protocol in HS1 (Q687) | **Amended** by D1548: HE5, on demand; Q687's default flips |
| **D774** | Three security layers around chDB | **Amended** by D1558 for the Rust engine |
| **D777** | `chsurface-2.0` and §49 §16's gates | Replaced by D1564's suites; Q1182 re-sets the ratios (Q693 for chDB) |
| **D779** | libchdb packaging, no Windows House | **Amended** by D1567 after HE2 |
| **Q695** | The billing unit | D1561 proposes both CPU time and bytes scanned |
| **Q696** | House reads of Lance collections | Kept at "no"; Q1181 proposes sharing index code instead |
| **§20 D131, §47, §58** | TiFlash as an optional add-on for SQL tenants; a synced columnar replica as an HTAP reference | Unchanged; Q1172 asks whether to start a TiFlash HTAP project for Loams SQL |
| **§57 (`loams-gateway`), §69 (edge reach, D1311, D1312)** | One gateway; off-tailnet callers only through the edge listener; public profile deferred | Applied: endpoints are `http`-module routes, off-tailnet only through the edge listener (D1559, Q1187) |
| **§70, §71 (BYOC, open PR #979)** | House as a second Container on Cloudflare; Lambda for applications | Consistent; D1562 adds Lambda as a House worker target (an HE3 spike) |
| **§63 (Loams Meter), §74, §72** | Usage aggregation, run summaries and traces in the House | Unchanged contracts; their queries join the mined profile (§7.2) |
| **Light compose default (0.1.1, PR #973)** | House on by default beside redb and a local-folder bucket | Unchanged now; after HE2 the default House image drops libchdb (D1567) |

Amendment notes pointing here are in §32, §49, §47 and the HS1 plan, and on the amended rows of the decision log. The owner approved them with this document on 2026-10-21.

## 18. Licences (D1566)

- **No BSL, GPL or AGPL code is copied** into Loams (D11).

  > **Amended by D1574 (owner, 2026-10-22).** FSL is treated like BSL: FSL code is never copied or ported, and FSL software runs only where BSL software would be allowed (an unmodified container, never redistributed).

- **ClickHouse** and **TiFlash** are Apache-2.0 (§4.2, §49 §19). Code ported from either keeps its copyright header, and NOTICE carries their notices. ClickHouse's stateless tests (`.sql` and `.reference`) are Apache-2.0 and may be vendored with attribution, filtered by a manifest (as D349 already allows).
- **Databend**: Apache-2.0 for most of the repository, with enterprise features under a separate licence (§4.3). Nothing is reused before a file-by-file audit that names the licence of each file; enterprise directories are never read for porting.
- **DataFusion, Arrow, iceberg-rust, foyer, sqlparser-rs**: Apache-2.0 (§4.8).
- **Test applications**: Langfuse (MIT, with an `ee/` directory under its own licence) and OpenPanel (AGPL-3.0) run as unmodified containers pinned by digest, never linked or distributed, as §49 did for Grafana and Metabase (Q694).
- **ClickBench** query text is CC BY-NC-SA 4.0: fetched at test time, never committed (D414).

## 19. Risks

| # | Risk | Mitigation |
|---|---|---|
| R1 | Iceberg v3 gaps in iceberg-rust | Contribute upstream (D1553); fall back to v2 with position deletes for now (§9) |
| R2 | Commit-rate limits and small files | Batching, a single committer per table, the fresh tier for reads (§10) |
| R3 | Cold-start latency on Lambda | NVMe footer and manifest caching, snapshot summaries, a warm pool (§13.1) |
| R4 | ClickHouse semantic edge cases (NULLs, float formatting, type coercion) | Differential testing against chDB (suite 4) |
| R5 | Scope creep toward "full ClickHouse" | The profile rules of §7; Tier 2 stays rejected without a user request |
| R6 | A performance gap against ClickHouse on warm queries | Accept 60–80 % early; compete on cold queries, elasticity and cost |
| R7 | Licence mistakes when reusing code | §18; a licence check in review for any ported file |
| R8 | **Two engines at once** (added): chDB and the Rust engine behind one front through HE3 doubles the test matrix | One front, one surface file, the oracle harness compares them on every change; the chDB engine leaves production images at HE3's exit |
| R9 | **Langfuse is owned by ClickHouse, Inc.** (added): the gate application may adopt features outside the profile | Mine every Langfuse release; pin the tested version; the compatibility page names the versions that pass |
| R10 | **Scope against the team's size** (added): a query engine is a multi-year effort, and other tracks compete for the same build machine and reviewers | HE0 writes no engine code; each phase has an exit gate the owner reviews; the chDB House keeps shipping meanwhile |

## 20. Open questions

Recorded in the decision log as Q1170–Q1187. The owner answered all of them on 2026-10-21 with the default, except Q1170: target the latest ClickHouse 26.x stable release, pinned as the oracle, with supported ClickHouse versions listed per gate application.

| # | Question | Default (answered by the owner on 2026-10-21: the default; Q1170 as ruled) |
|---|---|---|
| Q1170 | Which ClickHouse version range does the House claim? | ~~Behaviour of the oracle's pinned ClickHouse (26.9 today); the page lists, per gate application, the ClickHouse versions it supports (Langfuse v3: 24.3 and later; v4: 25.12 and later) and that the House passes its suite. The draft's "24.3 through current" is kept as the target for queries both lines send |~~ **Owner, 2026-10-21: target the latest ClickHouse 26.x stable release, pinned as the oracle, with supported ClickHouse versions listed per gate application.**
| Q1171 | How are iceberg-rust's missing v3 pieces handled? | Upstream PRs first; patches carried in `loams-house-iceberg` only while a PR is open; no long-lived fork |
| Q1172 | Start a TiFlash HTAP project for Loams SQL? | Not now; revisit when a Loams SQL user asks for analytical queries inside SQL |
| Q1173 | Default compute for large scans: containers or microVMs? | Containers; microVMs only for shared multi-tenant pools |
| Q1174 | Publish the real-application gate as a public compatibility badge? | Yes, after HE2: a generated page naming the Langfuse and OpenPanel versions that pass, never a percentage of ClickHouse |
| Q1175 | Where does the engine's code live? | The `fabric/` workspace (D343), one Arrow major shared by DataFusion and iceberg-rust |
| Q1176 | Drop Fluss `realtime` tables from the House in favour of its own fresh tier? | Yes; Fluss stays in §32 for the Event Fabric |
| Q1177 | Which log is the House's ingest WAL? | A Loams stream by default; an Iggy topic when the Event Fabric runs |
| Q1178 | Move the native protocol to HE5, on demand (amends D767, flips Q687's default)? | Yes |
| Q1179 | Which HS1 tasks pause? | Task 14, Tasks 30–31 and Task 34's chDB gates pause; the engine-independent tasks continue |
| Q1180 | Which Langfuse line is the gate: v3, v4 or both? | v4 (current upstream) as the gate, so its materialized view is Tier 0; v3 too until its security support ends (January 2027) |
| Q1181 | Share full-text and vector index code with the engine (not its collections)? | Yes, in HE4; Q696's "no reads of Lance collections" stands |
| Q1182 | Warm-query performance bar for the Rust engine through HE3? | 60–80 % of ClickHouse on warm ClickBench, internal only; cold queries and cost must beat managed ClickHouse on object storage |
| Q1183 | AWS Lambda as a House worker target: when? | An HE3 spike in a BYOC AWS account; not a default |
| Q1184 | Are `ALTER … DELETE/UPDATE` synchronous? | The answer waits for the deletion-vector commit (as `mutations_sync = 1`); `mutations_sync = 0` is accepted and answers after enqueueing |
| Q1185 | Which applications are mined in HE0? | Langfuse, OpenPanel, Plausible CE, SigNoz, Umami, PostHog (self-hosted) and one more from the draft's list once its ClickHouse use is confirmed; licence checked first |
| Q1186 | When does HE0 start? | After v0.1.1 ships; HE0 writes no engine code, so it does not compete with the release |
| Q1187 | Are endpoint pipes ever public? | Not until the public edge profile (D1312) is approved; until then only through §69's edge listener |

## 21. Sources

- **Owner**: the draft "Loam House — Plan" (draft v1, pasted 2026-10-21).
- **Loams**: §32 (D330–D351), §49 (D760–D779, Q685–Q699), the HS1 plan and its rulings, §47, §20 §10.4 (D131), §56, §57, §58, §63, §65, §69, §74; open PRs #973 (light compose), #979 (§70, §71), #982 (§72, §73), #983 (§74); `fabric/Cargo.toml`, `fabric/crates/loams-house*`, `crates/loams-cache`, `deploy/release/compose.yaml`, `docs/release/preview.md` (read 2026-10-21 on `dev` at `b281c2dde`).
- **iceberg-rust**: github.com/apache/iceberg-rust (release v0.10.1; issues #2411, #2792, #2580, #2419, #1652, #2879, #3401, #2546, #2581, #2923; PRs #2866, #2868, #3035, #2188, #2678); crates.io `iceberg`.
- **Iceberg specification and Java**: github.com/apache/iceberg `format/spec.md`; Iceberg release notes 1.8.0, 1.9.0, 1.10.0, 1.12.0.
- **TiFlash**: github.com/pingcap/tiflash (`ThirdPartyNotices.txt`, `.gitmodules`, README, releases; licence via the GitHub API); docs.pingcap.com/tidb/stable/release-7.4.0/ and docs.pingcap.com/tidb/stable/tiflash-disaggregated-and-s3.
- **Databend**: github.com/databendlabs/databend (`LICENSE`, README, `LICENSING.md`, `.config/licenserc-ee.toml`, `.config/licenserc.toml`).
- **Trino**: trino.io/docs/current/connector/iceberg.html (Trino 484).
- **DuckDB**: duckdb.org/docs/current/core_extensions/iceberg/writing_to_iceberg.html; github.com/duckdb/duckdb-iceberg README.
- **Langfuse**: langfuse.com/self-hosting/deployment/infrastructure/clickhouse; langfuse.com/changelog/2026-08-17-langfuse-v4; langfuse.com/self-hosting/upgrade/upgrade-guides/upgrade-v3-to-v4; github.com/langfuse/langfuse (migrations under `packages/shared/clickhouse/migrations/`, `LICENSE`, `ee/LICENSE`).
- **OpenPanel**: github.com/Openpanel-dev/openpanel (`packages/db/src/clickhouse/client.ts`, `packages/core/src/buffers/`, `funnel.sql.ts`, `conversion.sql.ts`, `retention.sql.ts`; licence via the GitHub API).
- **ObsessionDB**: obsessiondb.com (vendor claims, read 2026-10-21).
- **Tinybird**: tinybird.co/docs/forward, /docs/forward/work-with-data, /docs/forward/guides/quarantine.
- **DataFusion, sqlparser-rs, foyer**: crates.io and their GitHub repositories (`sqlparser` `src/dialect/clickhouse.rs`).
- **chDB and ClickHouse**: github.com/chdb-io/chdb-core releases; github.com/ClickHouse/ClickHouse release tags (26.3 and 26.8 LTS, 26.9 stable, 24.3 LTS).
