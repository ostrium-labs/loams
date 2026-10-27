# The tidb-incubator, pingcap and tikv repos: what is useful to Loams

Survey date: 2026-10-21. Asked for by the owner, who pointed at `tidb-incubator/weir`.

**Method.** We listed every public repo in the three GitHub orgs with `gh api orgs/<org>/repos --paginate`: 25 in tidb-incubator, 166 in pingcap and 66 in tikv, 257 in all. For each candidate we read the README, the licence (the API's SPDX id, and the LICENSE file whenever the API said `NOASSERTION` or nothing), the last push date, the language and the star count. Nothing was cloned.

**A note on push dates.** Many tikv repos show a last push of 2026-09-14 or 2026-08-31. That comes from org-wide housekeeping (the CNCF `.project` metadata work), not from real development. Treat those dates as "the repo is alive", not "the code changed".

**Use categories.**
- **adopt:** run it unmodified, or depend on it.
- **port:** adapt the code into Rust, with attribution and a provenance note (Apache-2.0 section 4).
- **reference:** take the ideas only.
- **skip:** not useful.

## Ranked shortlist

| # | Repo | Licence | Last push | Lang | Stars | Use | Loams track | What we take |
|---|---|---|---|---|---|---|---|---|
| 1 | [tikv/tikv](https://github.com/tikv/tikv) | Apache-2.0 | 2026-10-21 | Rust | 16.9k | adopt | Metastore, Live (20, 45), Loams SQL (47), KV gateways (77) | The storage engine we already run. Keep tracking 7.5/8.x releases (latest tag v7.5.8 on 2026-09-17). Its keyspace API v2 is the isolation primitive for per-tenant SQL and KV. |
| 2 | [tikv/client-rust](https://github.com/tikv/client-rust) | Apache-2.0 | 2026-10-19 | Rust | 430 | adopt (fork) | Every TiKV user in Loams | Already forked as `ostrium-labs/client-rust`. Upstream is active again, so keep sending our read-past and topics patches upstream (tikv/client-rust#578). |
| 3 | [pingcap/tidb](https://github.com/pingcap/tidb) | Apache-2.0 | 2026-10-21 | Go | 40.7k | adopt (adapted) | Loams SQL (23, 47, 58) | The SQL layer we adapt on TiKV keyspaces. Its `tests/integrationtest` and `integrationtest2` trees (mysql-tester `.test`/`.result` pairs) are the best public SQL conformance corpus in these orgs. `tests/realtikvtest` covers TiKV-backed behaviour. |
| 4 | [tidb-incubator/tidis](https://github.com/tidb-incubator/tidis) | Apache-2.0 (verified in Cargo.toml) | 2024-01-05 | Rust | 529 | port | KV gateways, Redis (77) | Already the design 77 base: Redis data types on TiKV transactions, Lua, TLS, lock optimisation. Unmaintained since January 2024, so we own the port: move it onto our client-rust fork and the `loams-gateway` RESP module. |
| 5 | [pingcap/tiproxy](https://github.com/pingcap/tiproxy) | Apache-2.0 | 2026-10-21 | Go | 73 | port | Gateway MySQL module (57) | The live successor of weir (its README says it is forked from Weir). Take: moving a session to another backend without dropping the client (session-state capture and replay); score-based balancing whose factors (`pkg/balance/factor`) are connection count, health, status, CPU, memory, labels and location; finding backends through PD/etcd; PROXY protocol; keepalive. Port the session-migration state machine into Rust; rewrite the rest. |
| 6 | [pingcap/ticdc](https://github.com/pingcap/ticdc) | Apache-2.0 | 2026-10-21 | Go | 56 | adopt | Change feeds (58), connectors | The new TiCDC architecture, used from v8.5.4 on (latest v8.5.8, 2026-08-27). It scales to more than a million tables and has sinks for MySQL, Kafka, Pulsar and S3. Run it for Loams SQL feeds. The TiCDC in `pingcap/tiflow` is the old line. |
| 7 | [tikv/migration](https://github.com/tikv/migration) | Apache-2.0 | 2026-03-09 | Go | 38 | adopt / reference | Metastore and Live backup, KV gateway CDC (45, 77) | TiKV-level **RawKV CDC** and **RawKV BR**, plus online bulk load and a Spark SST reader. TiDB's BR and TiCDC only cover TiDB tables, so these are the tools for our non-SQL keyspaces (metastore, Live, Redis and CQL data). Check API v2 and keyspace support before adopting. |
| 8 | [tikv/pd](https://github.com/tikv/pd) | Apache-2.0 | 2026-10-21 | Go | 1.2k | adopt; reference | Cluster control, Meter (63), operator (65) | We run it. Read `pkg/keyspace` (the keyspace manager), `pkg/mcs/resourcemanager` (Request Units and resource groups), `pkg/metering`, `pkg/gc` (keyspace-level GC safepoints) and the microservice split (`pkg/mcs/tso`, `scheduling`). They are the closest public look at how TiDB Serverless does multi-tenancy and metering. |
| 9 | [pingcap/tidb-operator](https://github.com/pingcap/tidb-operator) | Apache-2.0 | 2026-10-21 | Go | 1.3k | port | Loams Operator (65) | As design 65 says: port the PD/TiKV procedures (quorum-aware upgrade, scale in, store eviction, failover) from the **`feature/v2`** branch, which holds v2. The default `main` branch is v1 (latest v1.6.6). v2 also serves as the bridge driver. |
| 10 | [pingcap/mysql-tester](https://github.com/pingcap/mysql-tester) | Apache-2.0 | 2026-01-21 | Go | 73 | adopt | Loams SQL conformance (47) | The runner for TiDB's `.test`/`.result` corpus (a Go version of the MySQL Test Framework). Run it in CI against Loams SQL with the corpus from rank 3, and against the gateway's MySQL module. |
| 11 | [pingcap/kvproto](https://github.com/pingcap/kvproto) | Apache-2.0 | 2026-10-20 | Protobuf | 171 | adopt | All TiKV clients, gateways | The wire contract (kvrpcpb, cdcpb, brpb, keyspacepb, resource_manager). Pin one version across the client-rust fork and any Rust CDC or BR code. |
| 12 | [tikv/fail-rs](https://github.com/tikv/fail-rs) | Apache-2.0 | 2026-09-14 | Rust | 377 | adopt | Commit-path determinism and DST (55), tests | Rust fail points with low overhead. Use them for fault injection in the commit path, `loams-gateway` and the operator conformance harness (LOC). |
| 13 | [tikv/tlaplus-specs](https://github.com/tikv/tlaplus-specs) and [pingcap/tla-plus](https://github.com/pingcap/tla-plus) | Apache-2.0 (both) | 2026-09-14 / 2024-06-09 | TLA+ | 3 / 313 | reference | DT1 commit path (55), durable store, Live | Specs for Percolator, OptimizedCommitTS (async commit and 1PC), ResolvedTS, Raft, RaftMerge, TwoPC and CollapseRollbacks. Use them as starting models for our own specs (read-past, the Resonate TiKV store, CDC resolved-ts ordering). |
| 14 | [pingcap/go-ycsb](https://github.com/pingcap/go-ycsb) | Apache-2.0 | 2025-12-31 | Go | 642 | adopt | KV gateways (77), benchmarks | It has drivers for `tikv`, `redis` and `cassandra` side by side, so one tool can benchmark the Redis and CQL gateways against TiKV directly and against real Redis and Cassandra. |
| 15 | [pingcap/go-tpc](https://github.com/pingcap/go-tpc) | Apache-2.0 | 2026-01-13 | Go | 212 | adopt | Loams SQL, HTAP (47, 58), House (75) | TPC-C, TPC-H and CH-benCHmark (`ch`) over MySQL and Postgres wire. Use them as the SQL performance gate, and use CH for HTAP with the House engine. |
| 16 | [tikv/raft-rs](https://github.com/tikv/raft-rs) and [tikv/raft-engine](https://github.com/tikv/raft-engine) | Apache-2.0 (both) | 2026-05-13 / 2026-09-10 | Rust | 3.4k / 642 | reference (adopt if needed) | `loams-wal`, Live (45, 58) | Proven Rust Raft and a log-structured Raft log store. Adopt them only if `loams-wal` or another component needs in-process consensus. Otherwise use them as the reference for log GC, purge and recovery. |
| 17 | [tikv/pprof-rs](https://github.com/tikv/pprof-rs) | Apache-2.0 | 2026-09-14 | Rust | 1.7k | adopt | Observability, console | An in-process CPU profiler that outputs flamegraphs and pprof protobuf. Expose it on a debug endpoint in every Rust service, as TiKV does. |
| 18 | [tikv/minitrace-rust](https://github.com/tikv/minitrace-rust) → [fast/fastrace](https://github.com/fast/fastrace) | Apache-2.0 (both) | 2026-09-14 / 2026-10-17 | Rust | 750 / 1.1k | adopt fastrace | Tracing | The README says minitrace is no longer maintained and has become fastrace. Use fastrace on hot paths (gateway, commit path) where `tracing` costs too much. Do not depend on minitrace. |
| 19 | [tikv/async-speed-limit](https://github.com/tikv/async-speed-limit) | Apache-2.0 | 2026-08-31 | Rust | 69 | adopt | Backups, Git on S3, gateway | Rate-limits async byte streams. Use it for backup uploads, restores and per-tenant egress. |
| 20 | [pingcap/tidb-engine-ext](https://github.com/pingcap/tidb-engine-ext) | Apache-2.0 | 2026-09-08 | Rust | 33 | reference | HTAP (58), House (75) | TiFlash's "proxy": TiKV's multi-raft packaged as a C library, so another engine can join as a Raft learner fed by PD placement rules. It is the pattern for a later House engine that reads a fresh copy of the data as a Raft learner, with no CDC hop. A heavy fork of TiKV, so ideas only. |
| 21 | [pingcap/tiflash](https://github.com/pingcap/tiflash) | Apache-2.0 | 2026-10-21 | C++ | 1.0k | adopt (optional) | HTAP (58) | Unchanged from the existing plan: an optional HTAP companion for Loams SQL. |
| 22 | [tidb-incubator/weir](https://github.com/tidb-incubator/weir) | Apache-2.0 | 2022-01-16 | Go | 96 | reference | Gateway MySQL module (57) | The repo the owner pointed to. It is a MySQL L7 proxy with a backend connection pool, namespaces that reload at runtime (one per tenant), rate limits and circuit breakers, and SQL block-lists. Take the namespace and multi-tenant config model only. TiProxy (rank 5) replaces it, and its code has not moved since January 2022. |
| 23 | [pingcap/tidb-tools](https://github.com/pingcap/tidb-tools) | Apache-2.0 | 2026-09-17 | Go | 321 | adopt | Upgrades (54), SQL migration | `sync-diff-inspector` checks that two MySQL-compatible databases hold the same data, chunk by chunk with checksums. Use it as the gate when moving data between Loams SQL versions or clusters. |
| 24 | [pingcap/metering_sdk](https://github.com/pingcap/metering_sdk) | Apache-2.0 | 2026-08-14 | Go | 0 | reference | Loams Meter (63) | PingCAP's metering record format and S3/OSS layout (shared pools, pagination, gzip). Compare it with the Meter design's usage-record schema. |
| 25 | [tidb-incubator/Serverlessdb-for-HTAP](https://github.com/tidb-incubator/Serverlessdb-for-HTAP) | Apache-2.0 | 2021-10-25 | Go | 28 | reference | Serverless SQL, scale to zero (47, 69) | The only public "TiDB serverless" architecture in these orgs: a proxy that runs light queries itself and scales TiDB compute nodes by load, separating TP and AP. Stale; ideas only. |

### Also worth knowing (lower priority)

- **[tikv/titan](https://github.com/tikv/titan)** (Apache-2.0, C++): a **RocksDB plugin for key-value separation**, not a Redis layer. If design 77's "Titan as the second reference" means the Redis-on-TiKV project, that is `distributedio/titan` (Meitu), a different repo outside these orgs. Check which one design 77 means.
- **[tikv/mock-tikv](https://github.com/tikv/mock-tikv)** (Apache-2.0, Go): a fake TiKV server for testing clients. It could back fast unit tests in our client-rust fork.
- **[tikv/jepsen-test](https://github.com/tikv/jepsen-test)** (Apache-2.0, Clojure) and **[chaos-mesh/chaos-mesh](https://github.com/chaos-mesh/chaos-mesh)** (Apache-2.0, now in its own org): Jepsen checks for TiKV through a Rust client server, and Kubernetes fault injection for the LOC HA level.
- **[pingcap/advanced-statefulset](https://github.com/pingcap/advanced-statefulset)** (Apache-2.0, Go): a StatefulSet that can delete arbitrary ordinals. Design 65 uses a `LoamsInstance` per member instead, so take the idea only.
- **[pingcap/ng-monitoring](https://github.com/pingcap/ng-monitoring)** and **[pingcap/tidb-dashboard](https://github.com/pingcap/tidb-dashboard)** (Apache-2.0): Top SQL, continuous profiling and key visualiser. These are UX references for the console's database pages.
- **[pingcap/agfs](https://github.com/pingcap/agfs)** (Apache-2.0, C++ fork of c4pt0r/agfs): "Agent FS", a Plan 9-style RESTful filesystem for coordinating agents. A reference for the Studio and agent-daemon file surfaces (50, 53).
- **Small Rust crates, all Apache-2.0:** `tikv/yatp` (thread pool, reference), `tikv/minstant` (fast TSC time), `tikv/crc64fast`, `tikv/tracing-active-tree` (async call trees on demand), `tikv/match-template`. Adopt any of them only where a profile shows the need.
- **[tikv/rfcs](https://github.com/tikv/rfcs)** (Apache-2.0) and **[tikv/sig-transaction](https://github.com/tikv/sig-transaction)** (no licence): design records for API v2, keyspaces, async commit and resolved-ts. Read them as references. There is no licence on sig-transaction, so do not copy its text.
- **TiDB Cloud / Serverless:** none of these orgs has a public architecture repo. The serverless pieces live in the main repos: keyspaces and API v2 in tikv, pd and client-go; the resource manager and metering in pd; the keyspace-aware TiDB in pingcap/tidb.

## Skip list

| Repo(s) | Why |
|---|---|
| pingcap/br, pingcap/dumpling, pingcap/tidb-lightning, pingcap/dm | Archived. BR, Lightning and Dumpling now live in `pingcap/tidb` (`br/`, `dumpling/`), and DM lives in `pingcap/tiflow`. Use those copies. |
| pingcap/tipocket, pingcap/chaos, pingcap/octopus, pingcap/tidb-bench, pingcap/tidiff | Archived test tools. Use chaos-mesh, jepsen-test and go-tpc instead. |
| tikv/tikv-operator | Archived (2022). tidb-operator v2 replaces it. |
| tikv/auto-tikv, tikv/deep-dive-tikv, tikv/client-validator, tikv/skiplist-rs | Archived. |
| pingcap/parser | Merged into `pingcap/tidb` (`pkg/parser`); last push 2023. Duplicate. |
| pingcap/tiflow (TiCDC part) | The old TiCDC line; `pingcap/ticdc` replaces it from v8.5.4. Keep tiflow only if we want DM. |
| tikv/minitrace-rust, tikv/minitrace-go | Moved to fastrace; no longer maintained. |
| tidb-incubator/SHAREdis | Redis on TiKV in Go, 0 stars, last push 2022. Tidis is the better base. |
| tidb-incubator/tidb-lite, tidb-incubator/tidb-wasm, tidb-incubator/TiDBOverRoCE, tidb-incubator/hugegraph-on-tikv, tidb-incubator/Doris, tidb-incubator/java-client-cdc | Stale, too narrow or Java-only. java-client-cdc is a useful map of the CDC gRPC flow, but the Rust CDC work should follow `tikv/migration/cdc`. |
| tikv/client-java, client-c, client-cpp, client-node, client-py; pingcap/tikv-client-lib-java | Other-language clients; we use Rust. client-py and client-node wrap client-rust, which helps only if a Loams SDK ever needs raw TiKV. |
| Forks of third-party crates (tikv/rust-rocksdb, tikv/rocksdb, tikv/grpc, tikv/jemallocator, tikv/tokio, tikv/crossbeam, tikv/rusoto, tikv/azure-sdk-for-rust, pingcap/rust-protobuf, pingcap/tantivy, pingcap/sarama, pingcap/badger and others) | Forks carrying TiKV's patches. Depend on the upstream crates. |
| tikv/grpc-rs, tikv/rust-prometheus | Fine licences, but Loams uses tonic/connect-rust and its own metrics stack. Reference only, if at all. |
| pingcap/tispark, pingcap/TiBigData | Spark, Flink, Hive and Presto connectors in Scala and Java; not our stack. |
| pingcap/ossinsight, pingcap/autoflow, pingcap/pytidb, pingcap/tidb-vector-python, ORM adapters (django, activerecord, sqlalchemy, dbt, hibernate), demos and templates | Product and marketing apps, not infrastructure. |
| Docs, community, blog, website, meetup, training and course repos | Content only. See the licence warnings. |
| pingcap/talent-plan | Training courses with no licence file. Fine to read, not to copy. |

## Licence warnings

- **No BSL, AGPL or other source-available licence turned up in any of the three orgs.** Every code repo we would adopt or port is Apache-2.0 (tidis's licence is in its Cargo.toml; GitHub's API reports it correctly too).
- **GPL, so never copy the code:** pingcap/mydumper (GPL-3.0), pingcap/sqlsmith (GPL-3.0), pingcap/tidb-loadbalance (GPL-3.0), pingcap/wordpress-tidb-plugin (GPL-3.0) and pingcap/discourse (GPL-2.0). For random SQL generation, write our own or use a permissive tool, not sqlsmith.
- **tikv/rocksdb:** the API reports GPL-2.0 because it found `COPYING`, but the repo also ships `LICENSE.Apache`. RocksDB is dual-licensed GPL-2.0 or Apache-2.0, so use it under Apache-2.0.
- **Creative Commons share-alike, so never copy text into Apache-2.0 docs:** pingcap/docs and pingcap/docs-cn (CC BY-SA 3.0, checked in the LICENSE file) and tidb-incubator/tidb-in-action (CC BY-SA 4.0). We can link to them and paraphrase facts. Copying text means relicensing it under CC BY-SA.
- **No licence means all rights reserved:** pingcap/go-randgen (a SQL fuzz generator, so read it for ideas only), pingcap/talent-plan, pingcap/benchmarksql (an unofficial mirror; check upstream's terms), pingcap/jepsen (a fork; the API detects no licence, so check upstream jepsen-io/jepsen's README terms before reusing anything), tidb-incubator/tide, tikv/sig-transaction, pingcap/tidb-dev-guide and pingcap/monitoring.
- **MPL-2.0:** pingcap/mysqlx-driver is archived and not needed.
- **"BSL-1.0" on pingcap/boost-extra is the Boost Software License,** a permissive licence, not the Business Source License. It is archived and irrelevant anyway.
- **`NOASSERTION` on tikv/jemallocator** only means two licence files sit side by side (`LICENSE-APACHE` and MIT). It is permissive, but depend on upstream `tikv-jemallocator` from crates.io anyway.
- **Porting rule for tiproxy, tidis and tidb-operator:** keep each file's copyright notice, add a NOTICE entry and a provenance header naming the source repo and commit, and say in the file that it was changed (Apache-2.0 section 4).
