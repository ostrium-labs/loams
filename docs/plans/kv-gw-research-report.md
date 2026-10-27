# KV Gateways Research Report (Phases 0 to 3)

Date: 2026-10-21. Input to the KV gateways plan (owner draft of 2026-10-21: Redis (RESP) and Cassandra (CQL) gateways on TiKV, a shared KV core, and a foyer hot tier) and to design 77, which is written separately. This report makes no design decisions. It records facts, with sources, and gives recommendations the design can take or leave.

Method:
- Licences and release facts were read on 2026-10-21 from the projects' own files at named tags or branches (GitHub raw files and the GitHub API), crates.io's API, and vendor docs. Every claim has a source. Anything not confirmed is marked **unverified**.
- Loams facts come from `origin/dev` at `bbb21ff0e` and from the local clone of our TiKV client fork at the pinned rev `41b3588`.
- Titan (`distributedio/titan` at `97868f5`) and Tidis (`tidb-incubator/tidis` at `5558562`) were read from the owner's read-only clones. Nothing was cloned, built or pulled for this report, and nothing was posted upstream.

**Summary**
- **Redis tests:** use **Valkey's** TCL suite (BSD-3-Clause, `valkey-io/valkey`, `tests/`), not Redis 7.4+ (RSALv2/SSPLv1, and AGPLv3 as a third option since Redis 8). The last BSD Redis is the 7.2 line, at **7.2.16** (2026-08-17), which still gets security patches. Either suite runs against an external server with `./runtest --host H --port P`.
- **Cassandra tests:** Cassandra and its drivers are Apache-2.0; the Rust `scylla` driver is MIT OR Apache-2.0. The Python driver (`USE_CASS_EXTERNAL`), gocql (`-cluster`) and the Rust driver (`SCYLLA_URI`) run their integration suites against an external cluster; the Java driver's suite always launches its own CCM cluster. `cassandra-stress` is Apache-2.0 and runs from the Cassandra 5.0 tarball with `-node`. **ScyllaDB core is no longer open source** (source-available since 2025.1), so none of its server code or tests may be reused.
- **Benchmarks:** `valkey-benchmark` (BSD-3) instead of `redis-benchmark`; `memtier_benchmark` is GPL-2.0 (fine to run, never vendor); YCSB is Apache-2.0 but barely maintained (last release 0.17.0, 2019). `go-ycsb` (Apache-2.0) is the practical YCSB.
- **TiKV:** RawKV CAS needs only a client in atomic mode; API v2 needs `enable-ttl = true`; keyspaces need API v2; RawKV TTL exists; **RawKV and TxnKV data in one keyspace are disjoint** (`r` vs `x` prefixes), which constrains the plan's "single-key ops on RawKV, multi-key on txns" split. TiCDC does not capture RawKV; TiKV-CDC does but sinks only to another TiKV, so an invalidation consumer has to call TiKV's `ChangeData/EventFeed` with `kv_api = RawKV` itself. Our fork exposes raw CAS, TTL, batch, keyspaces and the CDC protos.
- **Existing crates:** the names collide. `loams-kv` is Loams Live's transactional store seam (redb embedded + TiKV), not a raw KV with CAS and TTL. `loams-hot` is the **vector (HNSW) hot tier**, not a KV cache. `loams-cache` is a foyer RAM + NVMe **byte-range cache over immutable objects**. There is an order-preserving tuple codec (`loams-tuple`), but not the plan's per-type memcomparable codec.
- **Redis on TiKV prior art:** Tidis (Rust) is the porting base and Titan (Go) the second reference. Neither runs on our TiKV as configured: both send API v1 requests, which an API v2 TiKV rejects. Both test suites can point at any server.

---

## 1. Redis conformance suites

### 1.1 Licences

| Suite | Licence | Evidence |
|---|---|---|
| Redis 7.2.x (`redis/redis`, tag `7.2.16`, 2026-08-17) | **BSD-3-Clause** | `COPYING` at `7.2.16` is the BSD-3 text; no `LICENSE.txt` in the tag ([COPYING@7.2.16](https://raw.githubusercontent.com/redis/redis/7.2.16/COPYING)). Tags run `7.2.0`..`7.2.16` ([tags](https://api.github.com/repos/redis/redis/git/matching-refs/tags/7.2)) |
| Redis 7.4.x | RSALv2 or SSPLv1 | "Starting on March 20th, 2024 … version 7.4 and subsequent releases … RSALv2 or SSPLv1" ([LICENSE.txt@7.4.0](https://raw.githubusercontent.com/redis/redis/7.4.0/LICENSE.txt)) |
| Redis 8.x and `unstable` | RSALv2, SSPLv1 or **AGPLv3** | Same file on `unstable`, which also says "Redis Open Source 7.2 and prior releases remain subject to the BSDv3 clause license" ([LICENSE.txt@unstable](https://raw.githubusercontent.com/redis/redis/unstable/LICENSE.txt)) |
| Valkey (`valkey-io/valkey`) | **BSD-3-Clause** | SPDX `BSD-3-Clause` ([repo API](https://api.github.com/repos/valkey-io/valkey)); `COPYING`: "BSD 3-Clause License / Copyright (c) 2024-present, Valkey contributors / Copyright (c) 2006-2020, Redis Ltd." ([COPYING](https://raw.githubusercontent.com/valkey-io/valkey/unstable/COPYING)) |

Neither repo has a separate licence under `tests/`, so the top-level licence covers the suite (from the tree listings; not every file header was checked).

- **The Redis 7.2 line is still patched under BSD.** 7.2.13 (2026-02-23), 7.2.14 (2026-05-05), 7.2.15 (2026-07-23) and 7.2.16 (2026-08-17, "Update urgency: SECURITY") shipped alongside 7.4.x; the `7.2` branch's last commit is 2026-10-16 ([releases](https://api.github.com/repos/redis/redis/releases), [7.2.16](https://api.github.com/repos/redis/redis/releases/tags/7.2.16), [branch](https://api.github.com/repos/redis/redis/branches/7.2)). No official end-of-life date for open-source 7.2 was found (**unverified**). Redis Software, the commercial product, lists 7.2 EOL as 2026-02-28 ([lifecycle](https://redis.io/docs/latest/operate/rs/8.0/installing-upgrading/product-lifecycle/)).
- **Valkey releases.** Stable `9.1.2` (2026-09-01); `9.2.0-rc1` (2026-09-16); maintained lines `9.0.6`, `8.1.10`, `8.0.11` ([releases](https://api.github.com/repos/valkey-io/valkey/releases)).

**Recommendation.** Take tests from **Valkey** at a pinned release tag (for example `9.1.2`), as an unmodified checkout used by CI at test time, not copied into the repo. Use Redis `7.2.16` only as a second opinion; never take anything from Redis 7.4 or later. Run reference servers as Valkey images (BSD-3), not Redis 8 images.

### 1.2 Running the suite against an external server

From Valkey's [`tests/README.md`](https://raw.githubusercontent.com/valkey-io/valkey/unstable/tests/README.md), [`test_helper.tcl`](https://raw.githubusercontent.com/valkey-io/valkey/unstable/tests/test_helper.tcl), [`support/server.tcl`](https://raw.githubusercontent.com/valkey-io/valkey/unstable/tests/support/server.tcl) and [`support/util.tcl`](https://raw.githubusercontent.com/valkey-io/valkey/unstable/tests/support/util.tcl):

- `./runtest --host <h> --port <p>` sets external mode; tests tagged `external:skip` are dropped automatically.
- `--singledb`: use only DB 0 (otherwise every client runs `SELECT 9`).
- `--ignore-encoding`: makes `assert_encoding` a no-op (it otherwise calls `OBJECT ENCODING`). No unit file calls `object encoding` directly, so this flag neutralises every encoding check.
- `--ignore-digest`: skips `DEBUG DIGEST`.
- Selection: `--single unit/type/hash` (repeatable), `--only <name|/regexp>`, `--skipunit`, `--skiptest <name|/regexp>`, `--skipfile`, `--list-tests`.
- `--tags "-needs:debug -needs:repl …"` denies tags. **Valkey `unstable` restricts the allow side**: a non-`-` tag must be one of `large-memory`, `needs:other-server`, `compatible-redis` or `network`. Redis `7.2.16` has no such restriction ([test_helper.tcl@7.2.16](https://raw.githubusercontent.com/redis/redis/7.2.16/tests/test_helper.tcl)). So with Valkey, select by `--single` and exclude by `-tag`.
- Capability tags to deny for a non-Redis engine: `needs:debug` (`DEBUG`, `OBJECT REFCOUNT`), `needs:repl` (`SYNC`), `needs:pfdebug`, `needs:config-maxmemory`, `needs:config-resetstat`, `needs:reset`, `needs:save`, `cluster:skip` is irrelevant outside cluster mode, `large-memory`.

What a non-Redis server must still answer, even with those tags denied:
- **`FLUSHALL` and `FUNCTION FLUSH`** at the start of every `start_server` block in external mode.
- **`CONFIG GET` / `CONFIG SET`** for each `overrides` entry of a block (for example `save ""` in `list.tcl`, `list-max-ziplist-size` in `list-3.tcl`, `hz 100` in `expire.tcl`). The gateway needs a tolerant `CONFIG` that accepts and echoes unknown parameters.
- Inline `r debug set-active-expire …` and `r debug quicklist-packed-threshold …` calls. Excluding these depends on the test being tagged `needs:debug`; that was not checked test by test. A gateway can answer `DEBUG SET-ACTIVE-EXPIRE` with `+OK`.

### 1.3 Which files cover the plan's types

All exist in Valkey `unstable` ([tests/unit](https://api.github.com/repos/valkey-io/valkey/contents/tests/unit?ref=unstable), [tests/unit/type](https://api.github.com/repos/valkey-io/valkey/contents/tests/unit/type?ref=unstable)). Counts are grep matches on the `unstable` files; "debug calls" counts lines with `r debug` or the `debug_*` helpers.

| Area | File | `needs:debug` | debug calls | `assert_encoding` | `external:skip` | `config set` | Notes |
|---|---|---|---|---|---|---|---|
| Strings | `unit/type/string.tcl` | 7 | 21 | 13 | 0 | 2 | 8 `needs:repl` |
| Counters | `unit/type/incr.tcl` | 1 | 2 | 3 | 0 | 2 | |
| Hashes | `unit/type/hash.tcl` | 1 | 1 | 12 | 1 | 15 | |
| Sets | `unit/type/set.tcl` | 2 | 3 | 57 | 1 | 8 | top block has overrides |
| Sorted sets | `unit/type/zset.tcl` | 3 | 14 | 75 | 2 | 33 | |
| Lists | `unit/type/list.tcl` | 16 | 32 | 46 | 0 | 23 | quicklist/listpack heavy |
| Lists | `unit/type/list-2.tcl` | 0 | 0 | 0 | 0 | 0 | |
| Lists | `unit/type/list-3.tcl` | 0 | 0 | 0 | 0 | 6 | `list-max-ziplist-size` override |
| Expire | `unit/expire.tcl` | 11 | 24 | 0 | 4 | 20 | `debug set-active-expire`; `hz` override |
| MULTI/EXEC | `unit/multi.tcl` | 9 | 16 | 0 | 5 | 19 | 14 `needs:repl`, 7 `singledb:skip` |
| Keys | `unit/keyspace.tcl` | 1 | 23 | 7 | 1 | 7 | 18 `singledb:skip` |
| Protocol | `unit/protocol.tcl` | 6 | 9 | 0 | 0 | 0 | uses `debug protocol` for RESP3 |
| Info | `unit/info-command.tcl` | 0 | 0 | 0 | 0 | 0 | cleanest file |

The most frequent `DEBUG` subcommands are `set-active-expire` (52), `quicklist-packed-threshold` (22), `protocol` (9), `object` (8), `htstats-key` (7), `populate` (6) and `reload` (5). How each file fares on a non-Redis engine is inferred from these counts, not run.

### 1.4 Proposed first subset (Phase 2 exit gate)

Run against Valkey `9.1.2`'s `tests/`, with Valkey itself as the control (the same command line must pass on Valkey):

```
./runtest --host 127.0.0.1 --port 6379 --singledb --ignore-encoding --ignore-digest \
  --tags "-needs:debug -needs:repl -needs:pfdebug -needs:save -needs:reset \
          -needs:config-maxmemory -needs:config-resetstat -large-memory" \
  --single unit/info-command \
  --single unit/type/incr \
  --single unit/type/string \
  --single unit/type/hash \
  --single unit/type/set \
  --single unit/type/zset \
  --single unit/type/list-2 \
  --single unit/type/list-3 \
  --single unit/expire \
  --single unit/multi \
  --single unit/keyspace
```

- **Order of adoption:** `incr`, `string`, `hash`, `set` first (simplest data models, fewest debug calls); then `zset`, `expire`, `multi`, `keyspace`; then `list-2`/`list-3`; `unit/type/list.tcl` last (16 `needs:debug` blocks, heavy on encodings).
- **Out of the first subset:** `unit/protocol.tcl` (needs `DEBUG PROTOCOL` for RESP3; cover RESP3 with our own tests until then), `unit/type/stream*`, pub/sub, scripting, `unit/info.tcl` (mostly `external:skip`), all `integration/` and `cluster/`.
- **Expected-failure list.** Keep a checked-in skip list (`--skiptest` entries) with one reason per test, for example `SETRANGE` beyond 512 MB, `OBJECT FREQ`, or a `WATCH` edge case that the txn mapping changes. The gate is "every test not on the skip list passes", and the skip list only shrinks.
- Also run Tidis's and Titan's suites (§7.2, §8.2) and the `resp-compatibility` tool (MIT, [tair-opensource/resp-compatibility](https://github.com/tair-opensource/resp-compatibility), active, last commit 2026-10-19): `resp_compatibility.py --host H --port P --testfile cts.json --specific-version 7.2.0 --show-failed` ([README](https://raw.githubusercontent.com/tair-opensource/resp-compatibility/main/README.md)). It reports a pass rate per Redis version, which is a useful headline number. No dedicated RESP3 compliance suite was found (**unverified**).

---

## 2. Cassandra conformance

### 2.1 Licences

| Item | Licence | Evidence |
|---|---|---|
| Apache Cassandra (`apache/cassandra`), including `test/unit`, `test/distributed` (in-JVM dtests), burn, harry, simulator | Apache-2.0 | [repo API](https://api.github.com/repos/apache/cassandra); ASF headers in test files (for example [ReadRepairTest.java](https://github.com/apache/cassandra/blob/trunk/test/distributed/org/apache/cassandra/distributed/test/ReadRepairTest.java)) |
| `apache/cassandra-dtest` (Python, CCM-driven) | Apache-2.0 | [LICENSE](https://github.com/apache/cassandra-dtest/blob/trunk/LICENSE) |
| Latest releases | **4.1.12** and **5.0.9** (tagged 2026-07-28, on the mirror 2026-08-07); 6.0-alpha2 exists | [downloads.apache.org/cassandra](https://downloads.apache.org/cassandra/) |

Cassandra's own tests run against an in-process or CCM-launched Cassandra; they are a semantics reference, not a suite to point at another server. The **drivers'** suites are the gate.

### 2.2 Driver suites that can target an external cluster

| Driver | Licence | External cluster? | How |
|---|---|---|---|
| Apache Cassandra Java driver 4.x (`apache/cassandra-java-driver`) | Apache-2.0 | **No.** `BaseCcmRule.before()` always runs `ccmBridge.create()` and `start()`; properties `ccm.version`, `ccm.distribution` (CASSANDRA, DSE, HCD), `ccm.directory`, `ccm.branch` only choose what CCM launches. Pointing it elsewhere needs a custom rule | [CcmBridge.java](https://github.com/apache/cassandra-java-driver/blob/trunk/test-infra/src/main/java/com/datastax/oss/driver/api/testinfra/ccm/CcmBridge.java), [BaseCcmRule.java](https://github.com/apache/cassandra-java-driver/blob/trunk/test-infra/src/main/java/com/datastax/oss/driver/api/testinfra/ccm/BaseCcmRule.java) |
| Python driver (`datastax/python-driver`, now `apache/cassandra-python-driver`) | Apache-2.0 | **Yes.** `USE_CASS_EXTERNAL` skips CCM (any non-empty value, even `0`, turns it on: `bool(os.getenv(...))`); `CLUSTER_IP` sets the contact point; `PROTOCOL_VERSION`, `CASSANDRA_VERSION` pick features. Tests marked `@local` are skipped | [tests/integration/__init__.py](https://github.com/apache/cassandra-python-driver/blob/trunk/tests/integration/__init__.py), [README-dev.rst](https://github.com/apache/cassandra-python-driver/blob/trunk/README-dev.rst): `USE_CASS_EXTERNAL=1 CASSANDRA_VERSION=4.0.1 pytest tests/integration/standard` |
| gocql (`apache/cassandra-gocql-driver`) | Apache-2.0 | **Yes**, by flags: `-cluster host1,host2` (default 127.0.0.1), `-proto`, `-cql`, `-rf`, `-clusterSize`, `-runssl`, `-runauth`, `-gocql.cversion`; build tags `integration`, `cassandra`, `ccm`, `unit`. CI builds a CCM cluster and passes `-cluster=$(ccm liveset)` | [common_test.go](https://github.com/apache/cassandra-gocql-driver/blob/trunk/common_test.go), [integration.sh](https://github.com/apache/cassandra-gocql-driver/blob/trunk/integration.sh). Inferred command: `go test -tags "integration" -proto=4 -rf=1 -clusterSize=1 -cluster=HOST ./...` (leave out the `ccm` tag) |
| Rust `scylla` driver (`scylladb/scylla-rust-driver`; crates `scylla` 1.9.0, `scylla-cql` 2.0.0, 2026-09-15) | **MIT OR Apache-2.0** (LICENSE-MIT and LICENSE-APACHE) | **Yes.** Integration tests read `SCYLLA_URI` (default `172.42.0.2:9042`; `SCYLLA_URI2`/`3` for multi-node tests). A Cassandra CI job brings up `image: cassandra` and runs `RUSTFLAGS="--cfg cassandra_tests" cargo nextest run` | [cassandra.yml](https://github.com/scylladb/scylla-rust-driver/blob/main/.github/workflows/cassandra.yml); `scylla/tests/integration/utils.rs` |

**Proposed Phase 3 subset** (each run first against Cassandra 5.0.9 as the control):
- **Python driver**, `tests/integration/standard/`: `test_connection.py`, `test_control_connection.py`, `test_cluster.py` (handshake, `system.local`/`system.peers`), `test_metadata.py` (schema tables; Phase 3a needs a subset of `system_schema`), `test_query.py`, `test_prepared_statements.py`, `test_query_paging.py`, `test_types.py`, `test_routing.py` (token-aware routing, so it tests our Murmur3), then for 3b `test_udts.py` and the LWT and batch cases in `test_query.py`. Skip `test_dse.py`, `test_authentication*.py` until auth exists, `test_cython_protocol_handlers.py`, `column_encryption`. File list from [the directory](https://github.com/apache/cassandra-python-driver/tree/trunk/tests/integration/standard).
- **gocql** with `-tags integration -clusterSize=1 -rf=1`: `session_test.go`, `cassandra_test.go`, `batch_test.go`, `token_test.go`, `keyspace_table_test.go`, `metadata_test.go`, `tuple_test.go`, `udt_test.go`, `vector_test.go` (skip until supported), `protocol_negotiation_test.go`. Skip `*_ccm_test.go`, `topology_test.go` and `ring_test.go` unless our `system.peers` emulation covers them.
- **Rust driver** with `SCYLLA_URI` and `--cfg cassandra_tests`: `scylla/tests/integration/{session,statements,types,metadata}`. It is also the driver our own tests will use, so it doubles as the CQL client in `loam-gw-test`.
- Gate: "every selected test not on the checked-in skip list passes", as for Redis.

### 2.3 `cassandra-stress`

- Part of Apache Cassandra, Apache-2.0; source in `tools/stress`, launcher `tools/bin/cassandra-stress` in the binary tarball, which runs `org.apache.cassandra.stress.Stress` with the tarball's `lib/` on the classpath ([README.txt](https://github.com/apache/cassandra/blob/cassandra-5.0/tools/stress/README.txt), [launcher](https://github.com/apache/cassandra/blob/cassandra-5.0/tools/bin/cassandra-stress)).
- No separate Apache distribution: CASSANDRA-10441 ("Move stress tool into its own repository") was closed Won't Fix ([JIRA](https://issues.apache.org/jira/browse/CASSANDRA-10441)). It is still in trunk.
- **Standalone use:** unpack the 5.0.9 binary tarball (Java 11 or 17) and run `tools/bin/cassandra-stress write n=1000000 -node <host> -port native=9042` then `read n=1000000 …` and `mixed ratio\(write=1,read=3\) …`. On 5.0, `-mode native cql3` is obsolete (CASSANDRA-18529) and only prints "ignoring deprecated parameter"; `native=` defaults to 9042 ([SettingsMode.java](https://github.com/apache/cassandra/blob/cassandra-5.0/tools/stress/src/org/apache/cassandra/stress/settings/SettingsMode.java), [SettingsPort.java](https://github.com/apache/cassandra/blob/cassandra-5.0/tools/stress/src/org/apache/cassandra/stress/settings/SettingsPort.java)). The `cassandra:5.0.9` image also contains it, so `docker run --rm --network host cassandra:5.0.9 cassandra-stress …` avoids a local install (not tried).
- ScyllaDB's fork `scylladb/cassandra-stress` is Apache-2.0 (v4.0.0, 2026-10-15) and ships a standalone image; usable as a tool, but the Apache original is the reference. `latte` (`pkolaczk/latte`, Apache-2.0, `latte-cli` 0.29.0, 2025-08-18) is a Rust alternative.

### 2.4 ScyllaDB's licence

- On 2024-12-18 ScyllaDB announced that 6.2 is the last AGPL open-source release and that from **2025.1** (shipped 2025-04-08) the database is source-available ([blog](https://www.scylladb.com/2024/12/18/license/), [2025.1](https://www.scylladb.com/2025/04/08/announcing-scylladb-2025-1), [FAQ](https://www.scylladb.com/source-available-faq)). The repo commit "treewide: relicense to ScyllaDB-Source-Available-1.0" is dated 2024-12-18.
- The licence is now **version 1.1 (2026-04-12)** ([LICENSE-ScyllaDB-Source-Available.md](https://github.com/scylladb/scylladb/blob/master/LICENSE-ScyllaDB-Source-Available.md)): only for "Never Customers"; capped at 10 TB and 50 vCPUs; no SaaS or DBaaS; no use "to compete with Licensor's existing or future products or services"; modified versions "shall be the sole property of the Licensor"; revocable.
- Scylla's dtest fork now lives inside the core repo (`test/cluster/dtest`), so it is under the same licence. `scylla-cluster-tests` is AGPL-3.0.
- Still permissive: the drivers (including the Rust driver, MIT OR Apache-2.0), `scylla-ccm`, `cassandra-stress` fork, `scylla-tools-java` (Apache-2.0).
- **Rule for Loams:** never read, copy or port code or tests from `scylladb/scylladb` (any version after 6.2, and preferably none at all, since Loams competes with it). Driver code is fine to depend on and to learn from.

---

## 3. Benchmarks

| Tool | Licence | Status | Use |
|---|---|---|---|
| `valkey-benchmark` (`src/valkey-benchmark.c`) | BSD-3 (file header "Copyright (c) 2009-2012, Redis Ltd.") ([source](https://raw.githubusercontent.com/valkey-io/valkey/unstable/src/valkey-benchmark.c)) | Ships with Valkey; `make install` also creates `redis-benchmark` and other `redis-*` symlinks unless `USE_REDIS_SYMLINKS=no` ([Makefile](https://raw.githubusercontent.com/valkey-io/valkey/unstable/src/Makefile)) | The plan's "redis-benchmark per durability mode": use `valkey-benchmark` from the Valkey image |
| `memtier_benchmark` (moved from `RedisLabs/` to `redis/memtier_benchmark`) | **GPL-2.0** ([COPYING](https://raw.githubusercontent.com/redis/memtier_benchmark/master/COPYING)) | Active: 2.5.2 (2026-10-07) ([releases](https://api.github.com/repos/redis/memtier_benchmark/releases)) | Fine to run as a separate binary; never vendor or link |
| YCSB (`brianfrankcooper/YCSB`) | Apache-2.0 ([repo](https://api.github.com/repos/brianfrankcooper/YCSB)) | Last release **0.17.0 (2019-10-06)**; master `0.18.0-SNAPSHOT`; recent commits only touch cloud bindings ([releases](https://api.github.com/repos/brianfrankcooper/YCSB/releases), [commits](https://api.github.com/repos/brianfrankcooper/YCSB/commits)) | Bindings: `redis` (Jedis 2.9.0; each record a hash, plus a global zset `_indices` for scans), `cassandra` (DataStax driver-core 3.0.0), `scylla` (scylla-driver-core 3.10.2) ([pom.xml](https://raw.githubusercontent.com/brianfrankcooper/YCSB/master/pom.xml), [RedisClient.java](https://raw.githubusercontent.com/brianfrankcooper/YCSB/master/redis/src/main/java/site/ycsb/db/RedisClient.java), [cassandra/pom.xml](https://raw.githubusercontent.com/brianfrankcooper/YCSB/master/cassandra/pom.xml)) |
| `go-ycsb` (`pingcap/go-ycsb`) | Apache-2.0 | Lightly maintained: v1.0.3 (2025-12-31) ([releases](https://api.github.com/repos/pingcap/go-ycsb/releases)) | Bindings include `redis` (go-redis v9, `redis.datatype=hash`), `cassandra` (gocql from 2018), `tikv` (client-go v2 from 2022) ([db/](https://api.github.com/repos/pingcap/go-ycsb/contents/db), [go.mod](https://raw.githubusercontent.com/pingcap/go-ycsb/master/go.mod)) |

**Recommendation.**
- Phase 2: `valkey-benchmark` (`-t set,get,incr,lpush,rpop,sadd,hset,zadd -P 1` and `-P 16`) per durability mode, against Valkey as the baseline.
- Phase 5: **go-ycsb** workloads A, B, C, F, through its `redis` binding against `loams-resp` and its `tikv` binding against raw TiKV, so the gateway's overhead over TiKV is visible. YCSB's Java `redis` binding is a fallback; its `_indices` zset makes every insert a two-key write, so it measures our MULTI path, not single-key ops.
- CQL: `cassandra-stress` (§2) and go-ycsb's `cassandra` binding.

---

## 4. TiKV facts for Phase 0

Pins on `dev`: `deploy/release/compose.yaml` and `deploy/release/.env.example` run PD and TiKV **v8.5.8**; `deploy/tikv/compose.yaml` (dev playground) still defaults `TIKV_TAG` to `v8.5.0`; CI's playground is v8.5.8 on API v2 (`.github/workflows/ci.yml`, the TiKV job). `deploy/tikv/tikv.toml` sets `storage.api-version = 2` and `storage.enable-ttl = true` ("Every TiKV config in the repo runs API v2 with TTL: keyspaces need both"). The client is `tikv-client` from `ostrium-labs/client-rust` at rev `41b3588` (workspace `Cargo.toml`), version 0.4.0, Apache-2.0.

| Question | Answer | Source |
|---|---|---|
| Does RawKV CAS need server config or API v2? | **No server config.** CAS needs a **client in atomic mode** (`RawClient::with_atomic_for_cas()`; `compare_and_swap` fails `assert_atomic` otherwise). There is no TiKV server option named `enable-atomic-for-cas`; the Java client calls it `setEnableAtomicForCAS`. Atomic-mode writes cost more, and a key must not be written by both atomic and non-atomic clients if CAS is to be linearizable | fork `src/raw/client.rs` (`with_atomic_for_cas`, `compare_and_swap`) at `41b3588`; [TiKV CAS doc](https://tikv.org/docs/7.1/concepts/explore-tikv-features/cas/); `crates/loams-safekeeper/src/tikv_raw.rs` module doc |
| Is it already used? | **Yes.** `loams-safekeeper`'s raw WAL store keeps blind puts on a plain `RawClient` and swaps its head key on an atomic one, on an API v2 keyspace. CI runs it (`cargo test -p loams-safekeeper --features tikv --test tikv`) | `crates/loams-safekeeper/src/tikv_raw.rs` (`TikvRawKv::connect`); `.github/workflows/ci.yml` |
| CAS retry safety | The pinned client treats an undetermined outcome as terminal for CAS (no blind retry that would report `false` for a swap that happened). This is upstream (#555, 2026-08-26), not fork-only | fork `src/raw/client.rs`; `git log -S retry_multi_region_terminal_on_ambiguous_outcome` |
| Do keyspaces need API v2? | **Yes.** The keyspace is part of the API v2 key layout (mode byte + 3-byte keyspace id); the client only prefixes keys for `Keyspace::Enable`, which maps to `ApiVersion::V2` | [RFC 0069](https://github.com/tikv/rfcs/blob/master/text/0069-api-v2.md); fork `src/request/keyspace.rs` |
| Does API v2 need `enable-ttl`? | **Yes.** "When API V2 is used, you are expected to set `storage.enable-ttl = true` at the same time"; `enable-ttl` can be set only when a cluster is created | [TiKV configuration file](https://docs.pingcap.com/tidb/stable/tikv-configuration-file/) |
| Does RawKV TTL exist? | **Yes, RawKV only.** "The TTL feature is only available for the RawKV interface." The fork has `put_with_ttl`, `batch_put_with_ttl` and `get_key_ttl_secs`. TxnKV has no per-key TTL, so TTL on transactional keys stays ours (expiry index + GC, as the plan says) | [TiKV config (7.1)](https://tikv.org/docs/7.1/deploy/configure/tikv-configuration-file/); fork `src/raw/client.rs` |
| Can RawKV and TxnKV see each other's keys? | **No.** API v2 prefixes raw keys with `r` and transactional keys with `x` (then the keyspace id); the server accepts only raw-prefixed keys on raw requests and txn-prefixed keys on txn requests. In one keyspace they are two disjoint key spaces | [RFC 0069](https://github.com/tikv/rfcs/blob/master/text/0069-api-v2.md); fork `src/request/keyspace.rs` (`RAW_KEY_PREFIX = b'r'`, `TXN_KEY_PREFIX = b'x'`) |
| Do API v1 clients work on our cluster? | **No.** On an API v2 server a V1 request is rejected "unless it's a TxnKV request and the keys starting with `m` or `t`" (TiDB's data); the compatibility matrix lists V1 RawClient and V1 TxnClient as "Error" | [RFC 0069](https://github.com/tikv/rfcs/blob/master/text/0069-api-v2.md) |
| Does TiCDC capture RawKV? | **No.** TiCDC's docs list "a TiKV cluster that uses RawKV alone" as unsupported | [TiCDC overview v7.4](https://docs-archive.pingcap.com/tidb/v7.4/ticdc-overview/) |
| Is there a RawKV change feed? | **TiKV-CDC** (`tikv/migration`, `cdc/`, Apache-2.0) replicates RawKV with API v2 on TiKV ≥ v6.2, but its only sink is another TiKV (`--sink-uri tikv://…`). Releases are betas (`cdc-v1.3.0-beta`, 2024-06-22); last commit 2026-03-09. A TiKV 8.5 compatibility statement was not found (**unverified**) | [TiKV-CDC doc](https://tikv.org/docs/7.1/concepts/explore-tikv-features/cdc/cdc/); [README](https://github.com/tikv/migration/blob/main/cdc/README.md); [releases](https://api.github.com/repos/tikv/migration/releases) |
| How would `loams-hot` invalidate then? | Call TiKV's own CDC service: `cdcpb.ChangeData/EventFeed` (or `EventFeedV2`) with `ChangeDataRequest.kv_api = RawKV` (or `TxnKV`), per region, from a checkpoint ts. The fork's `tikv_client::proto` module exports the generated `ChangeDataClient` for exactly this. The consumer must handle region registration, splits and epoch changes, and resolved-ts, which is what TiCDC's kv client does; budget for it | fork `proto/cdcpb.proto` (`KvAPI { TiDB, RawKV, TxnKV }`, `service ChangeData`); fork `src/proto.rs` doc |
| MVCC GC | API v2 TiKV GC is cluster-wide; Loams is the cluster's GC worker (`loams-tikv` `GcLoop`). RawKV values in API v2 carry timestamps (needed for CDC), so raw keys are GC'd with the cluster too. A gateway must not run its own GC worker (Tidis expects a TiDB to trigger GC; §7) | `docs/plans/r1-dependency-spike.md` (Q32); [RFC 0069](https://github.com/tikv/rfcs/blob/master/text/0069-api-v2.md) |

What the fork exposes (at `41b3588`):

| Need | Fork API | Notes |
|---|---|---|
| Raw get/put/delete | `RawClient::{get, put, delete}` | |
| Raw batch | `batch_get`, `batch_put`, `batch_put_with_ttl`, `batch_delete`, `delete_range`, `batch_scan`, `batch_scan_keys` | |
| Raw scan | `scan`, `scan_reverse`, `scan_keys`, `scan_keys_reverse` | |
| Raw CAS | `with_atomic_for_cas()` then `compare_and_swap(key, prev: Option, new) -> (Option<prev>, swapped)` | no CAS-with-TTL and no CAS delete; a delete must be modelled as swapping to a tombstone value |
| Raw TTL | `put_with_ttl`, `batch_put_with_ttl`, `get_key_ttl_secs` | seconds resolution; Redis `PEXPIRE` needs our own ms expiry on top |
| Keyspace (API v2) | `Config::with_keyspace(name)` | used by `loams-tikv` and `loams-safekeeper` |
| Txn options | `TransactionOptions::{new_optimistic, new_pessimistic, use_async_commit, try_one_pc, read_only, heartbeat_option}`, `lock_keys`, `batch_get_for_update` | `loams-tikv`'s runner wraps these |
| CDC | `tikv_client::proto::cdcpb` (generated; no wrapper) | §4 row above |

**Consequences for the plan (for design 77):**
1. "Single-key ops on RawKV, multi-key on txns" needs care: a key written by RawKV is invisible to a transaction in the same keyspace. Either all types of one user key live in one mode (a Redis keyspace is either raw-mode or txn-mode), or MULTI/EXEC, RENAME and `*STORE` read raw keys outside the transaction and lose atomicity. Tidis's design shows the trade-off: it keeps strings on RawKV only when `use_txn_api = false`, and every type on TxnKV otherwise (§7).
2. Raw CAS and blind raw writes to the same key must both go through the atomic client, which makes plain `SET` more expensive on keys that also see `SETNX`/`INCR`.
3. A tenant is a keyspace (PD's `/pd/api/v2/keyspaces`, `loams_tikv::ensure_keyspace`) plus, inside it, `loams-kv`'s root prefix. The plan's `[tenant_keyspace][namespace_byte]` prefix then needs no tenant bytes of its own.

---

## 5. Existing code on `dev`

### 5.1 What the four crates are

- **`crates/loams-kv`**: "The store seam of Loams Live". `Store` is an enum over an **embedded** backend (MVCC on redb, `src/embedded/`) and **tikv** (feature `tikv`, wrapping `loams-tikv`). `Store::run(opts, body)` runs a transaction with the runner's retries; `Store::snapshot(ts)` reads at a timestamp. `Txn` has `get`, `batch_get`, `scan`, `scan_reverse`, `put`, `insert`, `delete`, `lock_keys` (`src/txn.rs`); values are capped at 2 MiB (`MAX_VALUE_BYTES`). `CommitMode::Async1pc` is the default (async commit + 1PC). `Mode::Pessimistic` exists but **`Store::run` refuses it** until the seam has `get_for_update` (row T21-15, `src/runner.rs`). `conformance.rs` + `kv_conformance!` hold both backends to the same snapshot-isolation, scan, root-isolation, runner, fault and GC-barrier cases (`tests/conformance.rs`). No raw API, no CAS, no TTL, no metrics beyond the runner's.
- **`crates/loams-tikv`**: the keyspace-scoped TiKV handle (`Tikv`), the transaction runner, the TSO clock and supervisor, keyspace bootstrap through PD's HTTP API (`src/keyspace.rs`), the cluster MVCC GC loop (`src/gc.rs`), commit tokens for unknown outcomes (`src/token.rs`), `TikvStats` counters, and the cluster test harness (`testing::cluster`, skipped unless `LOAMS_TEST_PD` is set). It re-exports the pinned `tikv_client` for raw use.
- **`crates/loams-hot`**: **the vector hot tier** (design §04, plan M1.3): HNSW artifacts, per-manifest views with delta indexes, pinned splits on NVMe, budgets, heat, node registry, rendezvous placement and read forwarding (`src/lib.rs`). It uses `loams-cache` and `moka`, not foyer directly. It has nothing to do with KV caching.
- **`crates/loams-cache`**: `RangeCache`, a **read-through RAM + NVMe byte-range cache over immutable objects** in `loams-store`, built on foyer 0.22 `HybridCache` (`src/range_cache.rs`). Its key is (object path, block index); `forget(path)` drops an object; the disk tier always starts cold. It has no TTL, no versioning and no invalidation feed, because its objects never change.
- Also relevant: **`crates/loams-tuple`** (the order-preserving tuple codec), **`crates/loams-safekeeper/src/tikv_raw.rs`** (a `RawKv` trait with a TiKV impl and a linearizable in-memory `MemRawKv`), **`crates/loams-sim`** (seeded simulation and a linearizability checker for CAS registers), and **`crates/loams-gateway`** (the Rust gateway, one module per protocol, Postgres SNI first; design 57, plan GW1).

### 5.2 Codec

`loams-tuple` (`crates/loams-tuple/src/lib.rs`, re-exported by `loams-kv` and `loams-tikv`) is an order-preserving, typed, self-delimiting tuple codec: type tags (null `0x05` < int64 `0x10` < float64 `0x20` < bool < string `0x40` < bytes `0x50` < array `0x60`), `0x00 0xFF` escaping with a terminator for strings and bytes, sign-flipped big-endian int64, IEEE float64 with sign flip or full inversion and one canonical NaN, nested arrays, and `successor(prefix)` for range ends. Property tests are in `crates/loams-tikv/tests/codec.rs` (proptest).

Against the plan's §5 list: escape-encoded byte strings, signed ints, floats: **yes**. Unsigned BE ints and descending order: **no** (no `u64` tag; nothing inverts for `DESC`). Each element also carries a type tag byte, which the plan's untagged layout does not have.

**Recommendation:** reuse the escaping and float rules, but give `loams-kv` (the gateway core) an untagged `memcmp` module with `u64`, `i64`, `f64`, bytes, and a `desc` wrapper, property-tested against `loams-tuple` for the shared types. CQL clustering columns need `DESC` and every CQL type (§6.2).

### 5.3 Plan needs vs what exists

| Plan item | Exists on `dev` | Where | Gap |
|---|---|---|---|
| Crate name `loam-kv` / `loams-kv` | Name taken by Live's store seam | `crates/loams-kv` | Pick a new name (for example `loams-kvgw-core` or `loams-kvcore`) or extend `loams-kv` with a raw module; design 77 decides |
| Crate name `loams-hot` | Name taken by the vector hot tier | `crates/loams-hot` | Pick a new name (for example `loams-kvcache`) |
| Memcomparable codec | Partly: typed tuple codec, property-tested | `crates/loams-tuple/src/lib.rs`, `crates/loams-tikv/tests/codec.rs` | No `u64`, no `DESC`, typed tags |
| `KvStore::txn` | Yes: `Store::run`, `Txn`, `Snap`, async commit + 1PC default | `crates/loams-kv/src/{store,txn,runner}.rs` | Pessimistic mode refused (LWT and `WATCH` want `get_for_update`) |
| `KvStore::{get, put, delete, scan}` single-key, non-txn | Only inside a txn; raw ops exist only in safekeeper | `crates/loams-safekeeper/src/tikv_raw.rs` (`RawKv`) | Lift `RawKv` (get, batch_put, CAS, scan, delete_range) into the core, add TTL |
| `KvStore::cas` | Raw CAS on TiKV atomic client | `crates/loams-safekeeper/src/tikv_raw.rs` | Same |
| In-memory impl for tests and DST | Embedded backend is redb on disk (`EmbeddedConfig::new(path, …)`); `MemRawKv` is in-memory for raw | `crates/loams-kv/src/embedded/`, `crates/loams-kv/src/testing.rs`, `tikv_raw.rs` | A txn in-memory backend does not exist; redb on a temp dir serves tests today |
| redb impl | Yes (embedded MVCC) | `crates/loams-kv/src/embedded/` | Raw mode, TTL |
| TiKV impl | Yes | `crates/loams-kv/src/tikv.rs`, `crates/loams-tikv` | |
| Same trait suite on both impls | Yes for txns: `kv_conformance!` | `crates/loams-kv/src/conformance.rs` | Extend with raw, CAS and TTL cases |
| Tenancy (API v2 keyspaces) | Yes: PD keyspace create/state, keyspace-scoped clients, root prefix | `crates/loams-tikv/src/keyspace.rs`, `crates/loams-kv/src/store.rs` (`with_keyspace`, `with_root`) | Quotas and per-tenant rate limits |
| TTL index + GC | MVCC GC loop and barriers exist; no user-TTL index | `crates/loams-tikv/src/gc.rs`, `crates/loams-kv/src/gc.rs` | Build the `x` expiry index and its worker |
| Metrics | `TikvStats` (rebuilds, page halvings, restarts, unknown outcomes) | `crates/loams-tikv/src/runner.rs` | Per-tenant op counters and latency; Iceberg export (Loams Meter, design 63) |
| foyer hybrid cache | Yes, for immutable byte ranges | `crates/loams-cache/src/range_cache.rs` | Mutable keys: TTL, versioned values, invalidation |
| CDC invalidation consumer | No | fork `tikv_client::proto::cdcpb` | Build (§4) |
| RESP frontend | No | | §7, §9 |
| CQL frontend | No | | §6, §9 |
| Gateway host process | `loams-gateway` (Postgres SNI module) | `crates/loams-gateway` | Add RESP and CQL listener modules |
| Linearizability checker (Phase 4) | Yes, for CAS registers and sequencers | `crates/loams-sim` | Reuse for counters and LWT histories |

---

## 6. Rust building blocks

### 6.1 RESP

| Crate | Licence | Latest | Status | Notes |
|---|---|---|---|---|
| `redis-protocol` ([aembke/redis-protocol.rs](https://github.com/aembke/redis-protocol.rs)) | MIT in `Cargo.toml` (the repo also carries an Apache-2.0 file) | 6.0.0, 2024-11-15; ~2.56M recent downloads | Quiet since 2025-02, widely used (fred, shotover) | RESP2 and RESP3 decode and encode (features `resp2`, `resp3`), zero-copy `Bytes` frames, built on nom and cookie-factory |
| `redis` ([redis-rs](https://github.com/redis-rs/redis-rs)) | BSD-3-Clause | 1.7.1, 2026-09-25 | Very active | `parse_redis_value` and `Parser` are public; parses RESP3 values. A client-side value parser; a server-side encoder was not confirmed (**unverified**) |
| `resp` (iorust) | MIT/Apache-2.0 | 1.0.3, 2022-08-19 | Dead | |
| `resp3` | none listed | 0.1.0, 2022 | Abandoned | |

Sources: crates.io API (`/api/v1/crates/<name>`), checked 2026-10-21.

**Recommendation: build.** A server needs a streaming request decoder for arrays of bulk strings (plus inline commands for `redis-cli` and the TCL suite), hard limits (`proto-max-bulk-len`, multibulk length) enforced before allocation, and a reply encoder for RESP2 and RESP3 chosen per connection by `HELLO`. That is a few hundred lines, and owning it gives pipelining and limits under our control. Use `redis-protocol` (MIT, compatible with Apache-2.0) as the **differential oracle** in property tests (encode with one, decode with the other), and Tidis's `src/frame.rs`/`src/parse.rs` (Apache-2.0, mini-redis shape) as the starting structure.

### 6.2 CQL

| Crate | Licence | Latest | Notes |
|---|---|---|---|
| `scylla-cql` | MIT OR Apache-2.0 | 2.0.0, 2026-09-15; ~1.9M downloads | Very active; frame, types and (de)serialisation for the Rust `scylla` driver, CI-tested against Cassandra. Client-oriented API (requests encode, responses decode) |
| `cassandra-protocol` ([krojew/cdrs-tokio](https://github.com/krojew/cdrs-tokio)) | MIT/Apache-2.0 | 4.0.1, 2026-05-21 | Active; frames and types in both directions; shotover's proxy decodes client requests with it. Protocol v4 and v5 (with v5 framing segments) |
| `cql3-parser` ([shotover/rust-cql3-parser](https://github.com/shotover/rust-cql3-parser)) | Apache-2.0 | 0.4.2, 2024-09-13; ~2.2k downloads | Built on `tree-sitter-cql` 0.2.0 (MIT), whose listed upstream returns 404 (**unverified** provenance); low traffic |
| shotover-proxy ([shotover/shotover-proxy](https://github.com/shotover/shotover-proxy)) | Apache-2.0 | v0.7.4, 2026-08-27 | A Rust proxy for CQL4/5, RESP2 and Kafka; uses `redis-protocol` 6, `cassandra-protocol` 4, `cql3-parser` 0.4 ([Cargo.toml](https://github.com/shotover/shotover-proxy/blob/main/shotover/Cargo.toml)). The best working example of server-side CQL decoding in Rust |

**Recommendation.**
- **Framing: reuse `cassandra-protocol`** for frames, v5 segments, compression and value types, since it decodes requests and encodes responses, which is the server's direction; wrap it behind our own trait so that switching to `scylla-cql` stays possible. Use `scylla-cql`/the `scylla` driver on the client side of tests.
- **Grammar: build** a hand-written recursive-descent parser for the CQL subset of Phase 3, following Cassandra's Apache-2.0 ANTLR grammar (`Parser.g`, `Lexer.g`) as the spec. Use `cql3-parser` only as a test oracle. A tree-sitter grammar of unclear origin is a poor base for a query engine's error messages and prepared-statement bind markers.

### 6.3 Murmur3 for `token()`

Cassandra's partitioner hash is *not* standard MurmurHash3 x64_128:
- `MurmurHash.hash3_x64_128` is described in the source as "*almost* MurmurHash 3.0 … a sign bug": the 16-byte blocks are read correctly, but the tail bytes (cases 15 to 1) are read as `(long) key.get(i) << n`, so each byte is **sign-extended** ([MurmurHash.java](https://github.com/apache/cassandra/blob/trunk/src/java/org/apache/cassandra/utils/MurmurHash.java)).
- `Murmur3Partitioner`: seed 0; the token is `hash[0]` (h1) as a signed `long`; `normalize` maps `Long.MIN_VALUE` to `Long.MAX_VALUE`; an empty key gives the minimum token ([Murmur3Partitioner.java](https://github.com/apache/cassandra/blob/trunk/src/java/org/apache/cassandra/dht/Murmur3Partitioner.java)). A composite partition key is hashed over its serialised composite form.

| Crate | Licence | Matches Cassandra? |
|---|---|---|
| `murmur3` 0.5.2 | MIT/Apache-2.0 | **No** (standard tail) |
| `fastmurmur3` 0.2.0 | MIT | **No** (fuzzed against the reference C) |
| `mur3` 0.1.0 (tikv) | MIT | **No** |
| `scylla` driver, `routing/partitioner.rs` | MIT OR Apache-2.0 | **Yes**: tail read as `buf[i] as i8 as i64` "for compatibility with Cassandra's buggy implementation"; `Token::new` maps `i64::MIN` to `i64::MAX` ([partitioner.rs](https://github.com/scylladb/scylla-rust-driver/blob/main/scylla/src/routing/partitioner.rs)) |
| `cassandra-protocol`, `src/token.rs` | MIT/Apache-2.0 | **Yes** ("based on buggy Cassandra implementation") ([token.rs](https://github.com/krojew/cdrs-tokio/blob/master/cassandra-protocol/src/token.rs)) |

**Recommendation: build** a ~100-line `cassandra_token(bytes) -> i64` in the CQL crate (or take `cassandra-protocol`'s if that crate is adopted anyway), with test vectors generated from a real Cassandra 5.0.9 (`SELECT token(k) FROM t`) for keys with tails of 0 to 15 bytes and bytes ≥ `0x80` (which is where standard Murmur3 diverges), plus the empty key and a composite key. The token goes into the row key as `token ^ 0x8000…` big-endian so that byte order equals token order.

### 6.4 Summary

| Building block | Decision | Base |
|---|---|---|
| RESP parser and encoder | **Build** | Tidis's frame/parse structure; `redis-protocol` as the test oracle |
| CQL frames and types | **Reuse** `cassandra-protocol` (behind a trait) | `scylla-cql` on the test-client side |
| CQL grammar | **Build** | Cassandra's ANTLR grammar as spec; `cql3-parser` as oracle |
| Murmur3 token | **Build** (or reuse `cassandra-protocol::token`) | Vectors from Cassandra 5.0.9 |
| Memcomparable codec | **Build** an untagged module | `loams-tuple`'s escaping and float rules (§5.2) |
| TiKV client | **Reuse** our fork | §4 |
| foyer cache | **Reuse** foyer via the workspace pin (0.22) | `loams-cache` as the pattern |

---

## 7. Tidis (`tidb-incubator/tidis`, the primary porting base)

Read at `5558562` (2024-01-06, "fix get_txn_client check (#106)"). Apache-2.0 (`LICENSE`, `NOTICE`, `Cargo.toml`). 529 stars, not archived, last push 2024-01-05 ([repo](https://api.github.com/repos/tidb-incubator/tidis)).

### 7.1 Shape and command coverage

- One binary `tidis-server` (`src/bin/server.rs`). The frame and parser (`src/frame.rs`, `src/parse.rs`) follow the `mini-redis` structure; one file per command in `src/cmd/`; data-type logic per type in `src/tikv/{string,hash,list,set,zset,lua}.rs`; key encoding in `src/tikv/encoding/`; a background GC of old versions in `src/gc.rs`.
- **Two backends.** With `use_txn_api = false`, strings go to RawKV (`do_async_rawkv_*`); with `use_txn_api = true` (the documented default, README), every type goes through TxnKV with `use_async_commit`, `try_one_pc_commit`, optional pessimistic mode, and retry/backoff settings (`config.toml`).
- **Keys** (`src/tikv/encoding/encode.rs`): Tidis writes the API v2-looking layout itself: `x` or `r`, a 2-byte `instance_id` (as a poor man's keyspace), a type byte (`m` meta, `h`, `l`, `s`, `z`, `S` score), the user key in TiDB's 8-byte-group memcomparable encoding (`encode_bytes`), then a `u16` version and the field. Meta holds type, TTL and version, so deletes bump the version and old data is GC'd asynchronously, as in Titan.
- **Commands** (`src/cmd/mod.rs`): `get set setnx setex mget mset strlen incr decr incrby decrby`; `del exists type ttl pttl expire expireat pexpire pexpireat persist scan`; `hset hsetnx hmset hget hmget hlen hgetall hdel hkeys hvals hincrby hexists hstrlen`; `lpush rpush lpop rpop lrange llen lindex lset ltrim lrem linsert`; `sadd scard sismember smismember smembers srandmember spop srem`; `zadd zcard zscore zrem zremrangebyscore zremrangebyrank zrange zrevrange zrangebyscore zrevrangebyscore zcount zpopmin zpopmax zrank zincrby`; `multi exec discard`; `eval evalsha script` (Lua via `mlua`); `auth ping info client debug cluster readwrite readonly`; `publish subscribe unsubscribe` (single node).

Coverage against the plan's Phase 2 types and Titan:

| Area | Tidis has, Titan lacks | Titan has, Tidis lacks | Neither (in Valkey subset) |
|---|---|---|---|
| Strings / counters | | `append setrange getrange incrbyfloat setbit getbit bitcount bitpos getset msetnx psetex` | `getdel getex lcs` |
| Keys / TTL | | `unlink keys randomkey touch object` | `rename renamenx copy expiretime pexpiretime` |
| Hashes | | `hincrbyfloat hscan` | `hrandfield` |
| Lists | | `lpushx rpushx rpoplpush` | `lpos lmove blpop brpop` |
| Sets | `smismember srandmember` | `sunion sinter sdiff smove` | `sunionstore sinterstore sdiffstore sscan sintercard` |
| Sorted sets | `zremrangebyscore zremrangebyrank zpopmin zpopmax zrank zincrby` | `zrangebylex zrevrangebylex zlexcount zscan` | `zrevrank zunionstore zinterstore zrandmember` |
| Transactions | `exec discard` | `watch unwatch` (and `exec`, `discard` through the dispatcher) | |
| Other | `eval evalsha script` (out of plan v1), pub/sub | `select swapdb flushdb flushall monitor time command` | `hello` (RESP3) |

Neither speaks **RESP3** (`HELLO 3`). That is ours to build.

### 7.2 Tests

- `cargo test --all` (Makefile `unit-test`) runs the in-crate unit tests.
- `test/` is an end-to-end suite in Python `unittest` over `redis==3.5.3` (`test/requirements.txt`): `test_generic` (8 tests), `test_string` (22), `test_hash` (21), `test_list` (20), `test_set` (17), `test_zset` (24), `test_invalid` (59, argument and type errors), `test_lua` (98).
- **Against an external server:** `python3 test/test_helper.py --ip <host> --port <port>` (README "Run E2E tests"; `test/rediswrap.py` defaults to `127.0.0.1:6379`). It runs all classes in one suite; select a class with `python3 -m unittest test_string` from `test/`. Because it targets any server, it can run against Valkey (the reference) and `loams-resp` alike. `test_lua` is out of plan v1.

### 7.3 Dependencies and licences

Direct dependencies at their `Cargo.lock` versions (licences from crates.io's API): MIT (`async-stream`, `atoi`, `bytes`, `tokio`, `tokio-stream`, `tokio-util`, `hyper`, `mlua`); MIT or Apache-2.0 (`chrono`, `crc`, `structopt`, `lazy_static`, `thiserror`, `async-std`, `async-tls`, `rand`, `regex`, `toml`, `serde`, `futures`, `sha1`, `hex`); MPL-2.0 or MIT or Apache-2.0 (`slog`, `slog-term`); Apache-2.0 (`prometheus` 0.12, `pprof` 0.9, `grpcio` 0.10.3, `grpcio-sys` 0.10.3); Apache-2.0/ISC/MIT (`rustls` 0.19). `mlua` vendors Lua 5.1 (MIT) when built with `vendored`.

- Nothing copyleft. `cargo deny` was not run: it needs the git dependency on `yongman/client-rust`, which would mean fetching a repo (out of this task's rules). The pins are old (`rustls` 0.19, `hyper` 0.14, `prometheus` 0.12, `pprof` 0.9), so expect advisories; the port replaces all of them with the workspace's versions anyway.

### 7.4 Its tikv-client fork vs ours

| | Tidis: `yongman/client-rust` `dev-scanner` (pinned `a773980`, 2022-09-07) | Loams: `ostrium-labs/client-rust` `loam` (pinned `41b3588`) |
|---|---|---|
| Base | `tikv/client-rust` `027a7dff` (2022-07-27); 25 commits ahead, 119 behind upstream master ([compare](https://github.com/tikv/client-rust/compare/master...yongman:client-rust:dev-scanner)) | upstream `1b883bf` (2026-10-19) plus our topics: TSO stream reconnect, public proto, async-commit `max_commit_ts`, `fail` 0.5, read past pushed locks |
| Version, gRPC | 0.1.0, **`grpcio` 0.10 (gRPC C core, vendored OpenSSL)** | 0.4.0, `tonic` 0.12 / `prost` 0.13 |
| API v2 / keyspace | **None.** No `Keyspace`, no `api_version` in the raw client; requests go out as API v1. Tidis fakes the `x`/`r` prefix itself | `Config::with_keyspace`, `Keyspace::{Disable, Enable, ApiV2NoPrefix}` |
| Raw TTL | **None** (no `put_with_ttl`/`get_key_ttl`); Tidis keeps TTL in its own value header | `put_with_ttl`, `batch_put_with_ttl`, `get_key_ttl_secs` |
| Raw CAS | `with_atomic_for_cas`, `compare_and_swap` | same, plus terminal-on-undetermined retries (#555) |
| Fork additions | A "super batch" request worker (batching RPCs per store, `max_batch_size`, `max_batch_wait_time`, `max_inflight_requests`), `u64::MAX` timestamps, a streaming txn scanner (`scan stream`, batch size), `*_opt` variants taking a per-call `Backoff` | Upstream since 2022 has the batching rework, lock resolution fixes (#557, #561) and keyspaces |
| What to carry over | The streaming scan and per-call backoff ideas; the RPC batching is worth a benchmark, not a port | |

Sources: Tidis `Cargo.toml` / `Cargo.lock`; [yongman fork `src/raw/client.rs` at a773980](https://github.com/yongman/client-rust/blob/a773980806e3ca0540f15e063323b706b51b8e8f/src/raw/client.rs) and its `Cargo.toml`; ours from the local clone.

**Against our TiKV:** Tidis cannot run on an API v2 cluster: its V1 requests are rejected (§4). Even on API v1 it needs `enable-ttl = false`: issue [#109](https://github.com/tidb-incubator/tidis/issues/109) shows a `tiup playground v8.0.0 --mode tikv-slim` cluster (V1 with TTL) refusing its prewrites with `ApiVersionNotMatched { cmd: prewrite, storage_api_version: V1ttl, req_api_version: V1 }`. So using Tidis as a differential oracle means a separate throwaway TiKV with `api-version = 1`, `enable-ttl = false`, and its own GC trigger (Tidis's README asks for a TiDB node to drive GC).

### 7.5 Current Rust and edition 2024

Not built (see §7.3). From the source:
- `rust-toolchain` pins **`nightly-2022-07-27`**; `edition = "2018"`.
- `structopt` is unmaintained (superseded by `clap` derive); `grpcio-sys` needs CMake and a C++ toolchain and compiles gRPC C core; `mlua` 0.7 builds Lua with `cc`.
- **Edition 2024 blockers:** 6 `static mut` items (for example `TIKV_RAW_CLIENT`, `TIKV_TXN_CLIENTS` in `src/tikv/mod.rs`) with 69 `unsafe` uses; references to `static mut` are an error under the 2024 edition's `static_mut_refs` lint, so these become `OnceLock`/`LazyLock` in the port. `lazy_static!` should go the same way.
- Whether it compiles on stable 1.97 as edition 2018 is **unverified**; it does not matter for the port, which is a rewrite into workspace crates on the workspace's toolchain (edition 2024, resolver 3), the fork client and `tonic`.

### 7.6 Known bugs and limits (19 open issues)

From [open issues](https://github.com/tidb-incubator/tidis/issues) (17 issues plus 2 PRs):
- **#91: `SET` overwrites a hash.** String and hash meta keys share one format, and `SET` does not check the existing type; the hash's sub-keys are orphaned. Our port must type-check meta on every write.
- **#97: gRPC message too large** (`RESOURCE_EXHAUSTED … 9612720 vs. 4194304`) after inserting millions of members into one zset; the server then failed to restart. Big collections need paged deletes and scans with bounded RPC sizes (`loams-tikv` already halves pages on `OutOfRange`).
- **#109: `ApiVersionNotMatched`** on a TTL-enabled cluster (above).
- **#110:** write conflicts under pressure on one key with pessimistic txns (open question, no answer).
- **#102, #103, #104, #74:** `select`, `unlink`, `exists` (at the time), `lpushx`/`rpushx` missing. **#94, #85:** pub/sub roadmap. **#70:** RDB export. **#71:** `CLIENT KILL`. **#98, #101:** "is it maintained / production-ready" with no answer.
- From the code: user keys are parsed as UTF-8 `String`s (`Parse::next_string`), so **binary keys are not supported**; versions are `u16` (`src/tikv/client.rs` `get_version_for_new`), so a key's version wraps after 65,535 overwrites and can alias old, not-yet-GC'd data; global `static mut` clients.

---

## 8. Titan (`distributedio/titan`, the second reference)

Read at `97868f5` (2021-10-09, "Merge pull request #226 … feature/zsets"). Apache-2.0. 1,403 stars, not archived, last push 2023-10-21 ([repo](https://api.github.com/repos/distributedio/titan)).

### 8.1 Shape and command coverage

- Go; Redis commands in `command/` (one file per type plus `transactions.go`), storage in `db/` (`string.go`, `hash.go`, `list.go`, `set.go`, `zset.go`, `expire.go`, `gc.go`, `tikvgc.go`, `lease.go`).
- **Everything is TxnKV** through TiDB's `store/tikv` package (`db/store/store.go`, `tikv.Driver{}.Open`), from `github.com/pingcap/tidb v1.1.0-beta.0.20201210…` and `kvproto` of 2020-12-08 (`go.mod`, `go 1.13`). There is no RawKV path.
- **Keys** (`db/db.go`): `namespace ":" dbid ":" "M" ":" userkey` for meta and `namespace ":" dbid ":" "D" ":" objectID` for data; the namespace comes from the `AUTH` token (multi-tenancy by prefix). The object ID is a UUID, so overwriting a key changes its data prefix and old data is GC'd later (`db/gc.go`, a GC key list); the expiry list is `db/expire.go`; `db/tikvgc.go` can drive TiKV's MVCC GC itself.
- **Commands** (`command/init.go`): strings and counters `get set setnx setex psetex mget mset msetnx strlen append setrange getrange incr decr incrby decrby incrbyfloat setbit getbit bitcount bitpos getset`; keys `type exists keys del unlink expire expireat pexpire pexpireat persist ttl pttl object scan randomkey touch`; hashes `hdel hset hget hgetall hexists hincrby hincrbyfloat hkeys hvals hlen hstrlen hsetnx hmget hmset hscan`; lists `lindex linsert llen lpop lpush lpushx lrange ltrim lrem lset rpop rpoplpush rpush rpushx`; sets `sadd smembers scard sismember spop srem sunion sinter sdiff smove`; zsets `zadd zrange zrevrange zrangebyscore zrevrangebyscore zrangebylex zrevrangebylex zrem zremrangebylex zcard zcount zlexcount zscore zscan`; transactions `multi exec discard watch unwatch`; server `auth echo ping quit select swapdb monitor client debug command flushdb flushall time info`; and an extension `escan`.
- Against the plan: Titan covers more strings, keys and lists than Tidis and has `WATCH`; it lacks `zincrby`, `zrank`, `zremrangebyscore`/`byrank`, `zpopmin`/`max` and `srandmember` (§7.1 table).

### 8.2 Tests

- **Go unit tests:** 146 `Test*` functions in `command/*_test.go` and `db/*_test.go`; `make test` runs `go test -short ./...`. They run on TiDB's `mockstore` (`db/store/mock_store.go`, `mocktikv://`), in process, with no TiKV.
- **Auto tests** (`tools/autotest/`): `TestMain` starts an embedded etcd, starts Titan in process on `127.0.0.1:17369` over mocktikv (`tools/integration/titan.go`), and drives it with `redigo`. Cases: `StringCase`, `ListCase`, `ZSetCase`, `KeyCase`, `SystemCase`, `MultiCase`, each in a normal and an abnormal (error-path) variant (`auto.go`, `abnormal.go`). No hash or set cases.
- **Against an external server:** not supported as is. The address is the package variable `integration.ServerAddr`, and `TestMain` always starts its own server. Pointing it at another server needs a small patch: skip `integration.Start()` and pass the address to `at.Start(addr)` and `an.Start(addr)`. Being Go tests that assert on replies, they would then run against Valkey and `loams-resp`. Issue #125 ("Add the tcl test cases into the integration test") was never done.

### 8.3 Does its client work against our TiKV 8.5?

- **Not on our API v2 cluster.** Titan's 2020 client predates API v2 (TiKV 6.1, 2022) and sends API v1 transactional requests with keys that start with its namespace, not `m` or `t`, so an API v2 server rejects them (§4). It has no keyspace support.
- On a separate API v1 TiKV 8.5 (`enable-ttl = false`), it might work: kvproto keeps wire compatibility, but this pairing of a 2020 client with an 8.5 server is **unverified**, and the 2020 client lacks the lock-resolution fixes our fork carries.
- **The practical oracle is Titan on `mocktikv://`**: one container, no TiKV at all, deterministic. Running Titan unmodified with `pd-addrs = "mocktikv://"` (its documented default in `conf/titan.toml`) answers "what did Titan reply" for a differential test without a second cluster. Valkey stays the semantic oracle; Titan answers "what does a Redis-on-TiKV layout do" (for example around TTL and GC timing).

### 8.4 Known bugs and limits to avoid

From the [open issues](https://github.com/distributedio/titan/issues) (39 issues; the repo's 44 count includes open PRs):
- **#180: wrong primary key in 2PC.** Percolator commits secondaries asynchronously; when the lazily updated meta key is a secondary, the next request on the object waits on its lock. Make the **meta key the primary** of every Redis transaction.
- **#85: hot meta.** Keeping `len`/`updatedAt` in the meta key makes every `HSET`/`SADD` rewrite and conflict on it. Keep counts out of the meta key, or shard them.
- **#160: expiry list hotspot.** A single time-ordered expiry list makes every TTL write hit the tail region, and sequential deletes leave a run of tombstones that slows RocksDB. Bucket or hash the expiry index (the plan's `x | expire_ms_be | …` has exactly this shape; add a bucket prefix).
- **#162 / #84:** `BatchGet` scans over runs of deleted keys; `MULTI` reads fields one by one. Batch reads and bound iterators.
- **#230: `RANDOMKEY`** loads the whole keyspace into memory and can return expired keys.
- **#28:** no configurable global prefix (we get it from keyspaces). **#208, #184:** connection and session handling under review. **#87:** the TiDB-embedded SDK drags TiDB's metrics in.

---

## 9. Take from Tidis vs Titan

| Concern | Take from | Why |
|---|---|---|
| Language, async structure, frame/parse, one-file-per-command layout | **Tidis** | Already Rust and tokio; maps onto workspace crates |
| Key encoding: memcomparable user key, type byte, versioned data keys | **Tidis** (with a `u64` version, not `u16`; binary keys) | Range scans stay ordered; closest to the plan's §5.1 |
| Meta-key-as-primary, keeping counters out of meta | **Titan** (issues #180, #85) | Learned the hard way |
| Expiry index and GC of old versions | **Titan** design, **bucketed** (issue #160) | Titan's is the more complete design; both need bucketing |
| Command breadth for strings, keys, lists, `WATCH` | **Titan** | Tidis lacks `append`, `getrange`, `setrange`, bit ops, `rpoplpush`, `watch` |
| Sorted-set breadth (`zincrby`, `zrank`, `zpop*`, `zremrange*`) | **Tidis** | Titan lacks them |
| Type checks on every write | Neither (Tidis #91) | Ours |
| TiKV client, API v2, keyspaces, TTL, GC | **Neither**: our fork, `loams-tikv`, `loams-kv` | Both predate API v2 |
| RESP3 (`HELLO`) | Neither | Ours |
| Tests | Both: Tidis's Python suite (runs against any server), Titan's auto tests (after a small patch) | Plus Valkey's TCL subset as the gate |
| Differential oracle | Valkey (semantics); Titan on `mocktikv://` (layout behaviour, no cluster) | Tidis as an oracle needs its own API v1 TiKV |
| Lua, pub/sub | Neither in v1 | Plan non-goal |

---

## 10. Open questions for design 77

1. Crate names, since `loams-kv` and `loams-hot` are taken (§5.3).
2. Raw vs txn per Redis keyspace, given that raw and txn keys are disjoint (§4, consequence 1).
3. `get_for_update` / pessimistic mode in `loams-kv` before LWT and `WATCH` (§5.1).
4. The CDC consumer: own `EventFeed` client vs running TiKV-CDC (which only sinks to TiKV) (§4).
5. The dev playground's `TIKV_TAG` default (`v8.5.0`) vs release and CI (`v8.5.8`).
