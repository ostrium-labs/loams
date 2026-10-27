# 77 — KV Gateways: Redis (RESP) and Cassandra (CQL) on `loams-kv`, with a KV Hot Tier

Status: **Approved** · 2026-10-21. The owner approved the design with **all defaults** on 2026-10-21: every decision (D1640–D1665) and every question's recommended default (Q1220–Q1239). D1641 (`loams-resp` ports Tidis's code as its primary base and uses Titan, `distributedio/titan`, as the second reference) was the owner's ruling earlier the same day. **Implementation (KV0) does not start yet:** the owner gave the House engine (§75, plan HE1) priority.

Source: the owner's draft of 2026-10-21, "Loam KV Gateways Plan: Redis (RESP) + Cassandra (CQL) on TiKV, with `loams-hot`" (draft v1). The owner asked for two agents on it. This document is the design agent's work: it carries the draft into the repository's format, renames "Loam" to "Loams", reconciles the draft with the crates that already exist on `dev`, corrects the licensing section, and checks the draft's verify items against primary sources. The second agent's research report on conformance suites, benchmarks, TiKV facts and the two reference projects is merged: [`docs/plans/kv-gw-research-report.md`](../plans/kv-gw-research-report.md) (#996, "the report" below). Its five open questions are Q1229, Q1230, Q1236, Q1237 and Q1239 here.

Decisions **D1640–D1665** and questions **Q1220–Q1239** are in the [decision log](13-decision-log.md). D1666–D1679 and Q1240–Q1244 are kept free for this document. The numbers start above design 76's reservation (D1600–D1639, Q1200–Q1219). Open PRs #990 and #993 use only numbers inside that reservation, and #973 uses D1340–D1343. Plan stub: [KV1](../plans/2026-10-21-kv1-kv-gateways.md), track **KV**.

Naming: "Titan" in this document always means **`distributedio/titan`** (Redis on TiKV, Go), never `tikv/titan` (an unrelated RocksDB blob-storage plugin).

Markers: **(source)** means read in the code or upstream repository at the revision in §17. **(estimate)** means computed or guessed, not measured. **(verify)** means not settled by a primary source yet; the task named next to it checks it.

**This document writes no code, and nothing in it is built.**

---

## 1. Summary

The owner wants Redis and Cassandra clients to work against Loams unchanged: `redis-cli`, `redis-py`, `ioredis` and the other Redis clients over RESP2 and RESP3, and the Apache Cassandra drivers over CQL native protocol v4 and v5. Both protocols are served by Rust code on the key-value store Loams already has. A hot tier makes reads fast and lets a keyspace choose a write path. Aerospike is not a target.

Most of the storage core the draft asks for already exists, under the names the draft wants to use for new crates:

- **The draft's `KvStore` trait is `loams-kv`.** `crates/loams-kv` (D691) is the store seam of Loams Live: `Store`, `Txn`, `Snap` and `Ts`, the transaction runner with retries and commit tokens, fault hooks, GC barriers and a conformance suite. It runs on two backends: an embedded MVCC store on redb, and TiKV through `loams-tikv`. This design extends it; it does not add a second `loams-kv` (D1642).
- **The draft's memcomparable codec is mostly `loams-tuple`.** It is order-preserving, with escaped byte strings, and property-tested. It lacks unsigned and descending forms, which this design adds in a new module (D1644).
- **The draft's `loams-hot` name is taken.** `crates/loams-hot` on `dev` is the vector and search hot tier (HNSW artifacts, delta indexes, placement and read forwarding; design §04, plan M1.3). The KV hot tier is a new crate, **`loams-kv-hot`**, layered on `crates/loams-cache` (the foyer RAM + NVMe cache over immutable objects), whose hybrid-cache builder it reuses; TTL, versioned values, write paths and invalidation are `loams-kv-hot`'s own (D1650, Q1230).
- **The gateways run in `loams-gateway`** (design §57), as the modules `resp` and `cql`, not in a "router" process (D1646).
- **Everything is Apache-2.0.** There is no AGPL router and no closed platform (D1649; D1119).

Five findings change the draft:

1. **Transactional keys by default, RawKV only as an opt-in fast mode (D1643, Q1236).** Under TiKV API v2, which every Loams cluster runs (D122), raw and transactional keys live in different key modes and are invisible to each other: TiKV refuses a request whose keys are in the other mode (`check_api_version`, `src/storage/mod.rs` at v8.5.0 **(source)**). The draft's split (single-key commands on RawKV, MULTI/EXEC on transactions) would make a key written by `SET` invisible to the next `MULTI`. So every Redis keyspace is transactional by default: correct MULTI/EXEC, with one-phase commit and async commit keeping single-key commands cheap (D237 measured 1PC faster than RawKV CAS plus put). A RawKV "fast" mode is an opt-in per keyspace, on TiKV only, for keyspaces that never use multi-key commands; it refuses them. The embedded backend has no RawKV.
2. **TTL is Loams' own on both backends.** TiKV's TTL applies to RawKV only, so expiry is an index plus a worker in `loams-kv`'s key space (D1655), as Titan (`distributedio/titan`) does it.
3. **TiKV-CDC cannot feed the cache as it is, and TiCDC does not capture RawKV.** TiKV-CDC (`tikv/migration`) subscribes with `kv_api = RawKV` hard-coded, so it never sees the transactional keyspaces that are the default here; its sinks are `tikv`, `kafka` and `blackhole` (the Kafka sink since `cdc-v1.3.0-beta`; tikv.org's "tikv only" page is out of date) (§13.2). Cross-gateway cache invalidation uses Loams' own change-feed consumer on TiKV's `ChangeData/EventFeed` service, **ported from TiKV-CDC's stream consumer** (`cdc/cdc/kv`, Apache-2.0) to Rust on the generated protos the client fork already exports, and parameterised by `kv_api` (D1652, Q1229).
4. **On the light default, the gateways run on the embedded backend.** PR #973 makes redb and a local folder the default compose stack, with TiKV behind a profile. The gateway process owns its own redb file and runs as one replica; TiKV is the path to more replicas (D1647).
5. **Redis's tests are not BSD any more.** Redis moved to RSALv2 or SSPLv1 with 7.4 and added AGPLv3 as a third option with 8.0. Loams uses **Valkey's** BSD-3 test suite and `valkey-benchmark` instead (D1661).

The owner ruled on Redis (D1641, approved): `loams-resp` ports **Tidis** (`tidb-incubator/tidis`, Rust, Apache-2.0) as its primary base and uses **Titan** (`distributedio/titan`, Go, Apache-2.0) as the second reference. §8 compares the two area by area and says which approach Loams takes and why.

## 2. Relation to earlier designs

| Design or decision | What it says | What this design does |
|---|---|---|
| [§00](00-pitch.md) **D42** | Loams speaks its native API, Flight SQL, Qdrant and an Elasticsearch subset. "Emulating five products' protocols is a five-product trap" | **Amendment (approved 2026-10-21)** (D1640): RESP and CQL join the list as gateway protocols on `loams-kv`. The trap argument is why §10 and §11 keep tight scopes and out-of-scope lists |
| [§26](26-jobs-api.md) §1, **D209** | "No Redis protocol. Loams does not speak RESP or run Lua." BullMQ gets `@loams/bullmq`, Celery gets a kombu transport | **Amendment (approved 2026-10-21)** (D1640): Loams speaks RESP for key-value workloads. **D209 and D208 stand**: BullMQ and Celery keep their plugins, because both need Lua (BullMQ's 49 scripts; kombu's `EVALSHA`), which `loams-resp` v1 does not run (Q1223) |
| [§68](68-open-protocol-compute.md) **D1255**, **D1272**, Q1039 | No RESP server and no managed Redis-compatible service in OC0–OC2; "Redis or AMQP servers" on the not-built list | **Amendment (approved 2026-10-21)** (D1640): a RESP gateway is built, as a key-value product, not as a Celery or BullMQ broker. D1255's Celery and BullMQ rulings stand; Q1039's answer (no Celery-only RESP subset) stands. D1271 (Redis 8 is never run, linked or copied) is unchanged and is why the conformance suite is Valkey's |
| [§57](57-loams-gateway.md) **D1010**, **D1022** | One Rust gateway, modules `pg`, then `http`, `llm`, `mcp` | **Amendment (approved 2026-10-21)** (D1646): two more modules, `resp` and `cql`. They terminate their protocols and run on `loams-kv`, like §74's `temporal` module |
| [§45](45-loams-live-production.md) §8, **D691**, **D692** | The store seam `loams-kv`; embedded not for production in LV1 | Extended, not replaced (D1642). D692's production clause is amended by #973 (open; its rows D1340–D1343 make the embedded stores supported for single-node installs), which this design depends on for the light default |
| [§04](04-hot-tier.md) principle | "The hot tier can be lost at any time without affecting correctness" | **Amendment (approved 2026-10-21)** (D1651): true for `loams-kv-hot`'s default modes. The opt-in `hot-only` and (v2) `hot-replicated` modes hold acknowledged writes in the hot tier and can lose them inside a documented window |
| [§20](20-reactive-database-on-tikv.md) §9, **D122**, **D260** | Multi-tenancy by API v2 keyspaces with size classes; TiKV is the store for Loams' own state | Reused for gateway tenancy (D1645) |
| [§28](28-loams-postgres.md) **D237** | TxnKV 1PC + async commit, not RawKV, for the WAL; spike numbers | Reused as evidence for D1643 |
| [JB1](../plans/2026-10-10-jb1-jobs-and-usage.md) **D851** | `get_for_update` and pessimistic mode added to `loams-kv` (approved, not built) | Built in KV0, before LWT and WATCH (Q1237) |
| [§58](58-data-access-and-htap.md) **D1057** | TiKV Raft is the substrate for small transactions and keyed records; `loams-wal` is for Postgres WAL | Gateway data is keyed records, so it is TiKV's role. `loams-wal` is not involved |
| [§69](69-edge-reach.md) **D1311**, **D1195** | The engine is never public; off-tailnet callers reach Loams only through the gateway's edge listener | The RESP and CQL listeners are never public and are not behind the edge listener in v1 (D1648) |
| [§74](74-durable-execution-compat.md) **D1504**, **D1513** | The Temporal frontend is a `loams-gateway` module with its own Deployment; Resonate's TiKV work happens in the Resonate fork | The same deployment shape (D1646). No Resonate change |
| [§18](18-metastore-backends-and-router.md), **D60** | Metastore backends; ScyllaDB Alternator 6.2.3 (AGPL-3.0) as an unmodified CI service for the DynamoDB backend | No Scylla code or tests are reused here (D1661). D60 is unchanged |
| [§63](63-loams-meter.md) | Usage reaches Meter as events from service identities; metrics never carry tenants | The draft's "per-tenant metrics to Iceberg" becomes an observer hook into Meter (D1662) |
| [§76](76-identity-and-authorization.md) | Zitadel and OpenFGA; one `Authorizer` | Gateway credentials are Loams tokens checked through it (Q1224, Q1232) |
| [r1-dependency-spike](../plans/r1-dependency-spike.md) §(i) | The `ostrium-labs/client-rust` fork, pinned at `41b3588` | The gateways use the same pin. §13.3 lists what Tidis's client fork has that ours lacks |

## 3. Goals and non-goals

**Goals (from the draft, corrected):**

- Wire-compatible Redis (RESP2 and RESP3) and Cassandra (CQL native protocol v4 and v5) frontends in Rust, as `loams-gateway` modules.
- One storage core, `loams-kv`, so each gateway is protocol parsing plus a data-model encoding.
- A KV hot tier, `loams-kv-hot`, that makes reads microsecond-class on a hit and lets a keyspace choose its write path.
- Multi-tenancy by TiKV keyspace and key prefix (D122), so tenants are isolated below the gateways.

**Non-goals (from the draft, kept):** rewriting ScyllaDB, Cassandra, Redis or Aerospike engines; matching Aerospike's sub-millisecond durable writes out of the box; Redis modules, Lua scripting (Q1223), Cassandra UDFs and UDAs, triggers, and Scylla-only extensions in v1.

**Not built:**

- A fork of Tidis or Titan (`distributedio/titan`) as a product, or either one shipped as a separate service (D1641).
- An Aerospike gateway (D1665).
- A DynamoDB-API gateway. The draft mentions one as a cheaper alternative to Aerospike; nothing is planned (D1665).
- A mixed raw-and-transactional keyspace (D1643).

## 4. Where things are today (checked on `dev` at `bbb21ff0e`, 2026-10-21)

| Piece | State | In the draft | This design |
|---|---|---|---|
| `crates/loams-kv` | **Built** (LV1 Tasks 20–21, D691). `Store` is an enum over `Embedded(embedded::Handle)` and `Tikv(loams_tikv::Tikv)` (LV1 Ruling 1: an enum, not a trait object). `Store::run(opts, body)` runs a closure in an optimistic transaction with retries, a deadline and optional commit tokens. `Txn` has `get`, `batch_get`, `scan`, `scan_reverse`, `put`, `insert`, `delete`, `lock_keys`. `Snap` reads at a `Ts`. `Store::barrier` holds GC. `kv_conformance!` holds both backends to 15 cases. Values are capped at 2 MiB (`MAX_VALUE_BYTES`). `Mode::Pessimistic` is refused until the seam has `get_for_update` (row T21-15); D851 (approved) adds it for JB1, and it is not built yet | `loam-kv` `store`: a `KvStore` trait with TiKV and in-memory impls | **Extend** (D1642, §6) |
| Embedded backend | **Built.** MVCC on one redb file: `versions` keyed `keyspace_id ‖ esc(root ‖ key) ‖ !commit_ts`, a persisted hybrid timestamp oracle, snapshot isolation with first-committer-wins, group commit, GC behind barriers. **redb locks its file, so one process opens it** (`embedded/mod.rs`) | Not in the draft | **The light default's backend** (D1647) |
| `crates/loams-tikv` | **Built.** Keyspace-scoped TiKV handle on API v2 (it refuses a cluster that is not), the runner, TSO clock, keyspace bootstrap through PD, the cluster MVCC GC loop (Loams is the cluster's GC worker, D122 as amended by Q32), commit tokens, the test harness. Re-exports `tikv_client` for raw use and its generated protos | `tikv-client` directly | **Reuse**; add a `cdc` module, the EventFeed consumer (D1652) |
| `crates/loams-tuple` | **Built.** The order-preserving tuple codec shared by `loams-kv` and `loams-tikv`: null < int64 < float64 < bool < string < bytes < array, sign-flipped big-endian integers, IEEE floats with canonical NaN, byte strings escaped (`0x00` → `0x00 0xFF`, terminator `0x00 0x00`), `successor` for prefix ends. Property-tested in `loams-tikv/tests/codec.rs` (4,096 cases). No unsigned, no descending | `codec` (memcomparable) | **Extend** with a `key` module (D1644) |
| `crates/loams-meta-tikv` | **Built.** The `MetaStore` on TiKV (D124). Per-record-type key tags, postcard values; its change feed polls (100 ms) and wakes on its own writes, with no TiCDC | Not in the draft | A pattern for the invalidation journal's poll (D1652) |
| `crates/loams-safekeeper/src/tikv_raw.rs` | **Built.** The only RawKV user: `RawKv` with `compare_and_swap` on `with_atomic_for_cas()`, an in-memory `MemRawKv` model, selected by `--store tikv-raw` (the §28 §7.3 alternative) | RawKV for single-key ops | **Lifted into `loams-kv`** as a `raw` module only if the opt-in fast mode is built (D1643, Q1236) |
| `crates/loams-hot` | **Built.** The vector and search hot tier: HNSW artifacts, `DeltaIndex` per manifest, pinned Tantivy splits, budgets with namespace fair share, a TinyLFU `HeatSketch`, node registry, rendezvous placement and read forwarding (§04, M1.3). No get/put of bytes | `loams-hot`: "cache crate around foyer (existing)" | **Unchanged.** Its `budget`, `heat`, `placement` and `registry` patterns may be reused by `loams-kv-hot` |
| `crates/loams-cache` | **Built.** `RangeCache`: a read-through RAM + NVMe byte-range cache over immutable objects, `foyer::HybridCache<String, Bytes>` (foyer 0.22.6, the workspace's only foyer user), 1 MiB blocks with crc32c trailers, cold start (`RecoverMode::None`). No put, no TTL, no invalidation beyond `forget` | (the draft's "existing" foyer cache) | **The base layer** of `loams-kv-hot`: its foyer builder becomes public; `RangeCache` is unchanged (D1650) |
| `crates/loams-gateway` | **Built (GW1 in progress).** Listeners, sniffers (Postgres startup, TLS ClientHello through rustls), the hot-reloaded route table, `io::Prefixed`, admin, `ConnectionObserver`. **The `pg` module is wired in directly**: there is no module trait or enum yet, and `GatewayConfig` and the routes file use `deny_unknown_fields` | "the Loam router process" | **Host** for `resp` and `cql` (D1646); the module seam is a KV2 task |
| `fabric/crates/loams-connector-kv` | **Built** (CN2). Redis/Valkey and Cassandra **source and sink connectors**, as clients (`redis` 1.7.1, `scylla` 1.9.0) | Not in the draft | Unrelated: connectors move data between external systems and Loams; the gateways make Loams look like those systems |
| `crates/loams-sim` | **Built.** Seeded in-process cluster simulation with a linearizability checker and an Elle list-append checker. madsim and turmoil are not used (D313; `loams-detsim` is planned, not built) | "DST" | **Reuse** for Phase 4 (§14) |
| Per-tenant metrics | `platform/crates/loams-meter` sends usage events to `_system/_meter`; the Iceberg tables of §63 are not built | "metrics to Iceberg for the optimizer" | An observer hook into Meter (D1662) |

## 5. Architecture

```
 clients: redis-cli, redis-py, ioredis, Lettuce, go-redis | Cassandra drivers (Java, Python, gocql, C++), cqlsh
        |                                                    |
   loams-gateway  module `resp`  (crate loams-resp)     module `cql`  (crate loams-cql)
        |      RESP2/3 frames, command table,                native protocol v4/v5 frames, CQL parser,
        |      type handlers (ported from Tidis)             schema manager, executor, paging
        \___________________________  ___________________________/
                                    \/
                     loams-kv-hot  (new; on loams-cache's foyer builder; TTL, durability modes, invalidation)
                                    |
                     loams-kv  (existing; extended: one-shot ops, scanner, TTL index, tenant, metrics)
                       /                                   \
          embedded backend (redb, one file,           tikv backend (loams-tikv, API v2 keyspaces,
          one process; the light default)             client-rust fork 41b3588; the `tikv` profile)
```

**Crates.** New: `loams-resp`, `loams-cql`, `loams-kv-hot` and `loams-gw-test` (the conformance and simulation harness). Extended: `loams-kv` (§6), `loams-tuple` (§7), `loams-gateway` (the module seam), `loams-tikv` (a `cdc` module, D1652) and `loams-cache` (its foyer builder made public, D1650). Unchanged: `loams-hot`. Every crate is Apache-2.0.

**One process per gateway Deployment** (D1646). Like §74's `temporal` module, `resp` and `cql` are compiled into the `loams-gateway` binary behind cargo features (`resp`, `cql`) and deployed separately from the Postgres front door: on Kubernetes a Deployment `loams-gateway-kv` with only those modules; in compose a `kv` profile (Q1228).

## 6. `loams-kv`: what exists, what is extended, what is new (D1642)

The draft's trait was `KvStore { get, put, cas, delete, scan, txn }`, with a TiKV impl, an in-memory impl, a TTL index, tenancy and metrics. The table maps each part onto the crate as it is.

| Draft item | Already in `loams-kv` | Extension (KV0 task) |
|---|---|---|
| `txn` | `Store::run(TxnOptions, body)`: optimistic, retried on conflict, `CommitMode::Async1pc` by default (async commit, and one-phase commit when the transaction fits one region) | None in Phase 0. **Later (KV2):** an interactive transaction, `Store::begin(opts) -> OpenTxn` with an explicit `commit`/`rollback` and a lifetime cap (default 10 s), for WATCH (D1657). The embedded backend already has `Handle::begin()`; the TiKV backend wraps `tikv-client`'s `Transaction` |
| `get`, `put`, `delete` | Inside a `Txn` | **One-shot helpers** `Store::get`, `put`, `insert`, `delete` and `compare_and_put`, each one optimistic transaction (1PC on TiKV when the key's region is one), with conformance cases on both backends (KV0 Task 2) |
| `cas` | `Txn::get` then `put` in one transaction; `insert` fails on a present key | `Store::compare_and_put(key, expected: Option<&[u8]>, new: Option<&[u8]>) -> Result<Option<Vec<u8>>, TxnError>` returning the current value on mismatch. Transactional by default (D1643). The opt-in raw fast mode (Q1236) uses the client's atomic `compare_and_swap`, lifted from `loams-safekeeper`'s `RawKv` (`tikv_raw.rs`) into a `loams-kv` `raw` module, TiKV only |
| Pessimistic reads | `Mode::Pessimistic` exists and **`Store::run` refuses it** (row T21-15); D851 (approved for JB1) adds `get_for_update` and pessimistic mode, not built yet | **Built in KV0**, before LWT and WATCH need it (Q1237): `Txn::get_for_update` and `batch_get_for_update` on both backends (TiKV through the fork's `TransactionOptions::new_pessimistic`; embedded through a lock table with a wait bound), the refusal and the `pessimistic_mode_is_refused` case replaced by pessimistic conformance cases |
| `scan` | `scan`/`scan_reverse` with a `limit` | **A paged scanner** on `Txn` and `Snap`, backed on TiKV by the fork's `Snapshot::scanner` / `Transaction::scanner` (upstream #554, in our pin) and on embedded by a redb range iterator (KV0 Task 3) |
| In-memory impl for tests and DST | None. Tests use the embedded backend in a temporary directory | **An in-memory embedded store**: `EmbeddedConfig::in_memory(name)` on redb's `InMemoryBackend` (redb 4 has it), so the same MVCC code runs with no file (KV0 Task 4). No third backend |
| `tenant` (API v2 keyspaces) | `StoreConfig::with_keyspace`, `with_root`, `ensure_keyspace`, `set_keyspace_state`; D122's classes | **A `tenant` module**: namespace → `StoreConfig` (dedicated keyspace, or shared keyspace + root) and the product tags of D1645 (KV0 Task 6) |
| `ttl` (expiry index + GC) | None. TiKV TTL is RawKV-only, so nothing in the seam uses it | **New, in KV2** (D1655, D1656), built on the one-shot helpers and the scanner |
| `metrics` | `TxnOptions::op` names an operation for fault plans | **Op metrics** `loams_kv_ops_total{backend,op,outcome}` and `loams_kv_op_seconds{backend,op}`, never labelled by tenant (D1021), and a `KvObserver` hook for per-namespace counts (KV0 Task 5; D1662) |
| `codec` | `loams-tuple` (re-exported as `loams_kv::tuple`) | `loams_tuple::key` (D1644; KV0 Task 1) |

**Does the light default fit the existing traits?** Yes, with one constraint. The embedded backend implements every operation the gateways need, with the same snapshot isolation and first-committer-wins as TiKV, and both backends pass the same conformance suite. The constraint is redb's file lock: one process opens a file. So a gateway on the embedded backend owns its own redb file, separate from the engine's Live file, and runs as **one replica** (D1647). Scaling out means the `tikv` profile.

## 7. The codec (D1644)

`loams-tuple`'s `tuple` encoding is the on-disk format of Live's indexes, so it does not change. A new module `loams_tuple::key` adds untagged primitives for fixed-layout keys:

- `u64` and `u16` big-endian; `i64` with the sign bit flipped; `f64` with the existing order-preserving transform (canonical NaN, `-0.0 == 0.0`);
- `bytes`: the existing escape with terminator, so a key component is prefix-free;
- `desc(..)`: bitwise inversion of any encoded component, for Cassandra `CLUSTERING ORDER BY ... DESC`;
- in KV3, Cassandra's orderings: `timeuuid` (time first, then the rest, as Cassandra compares it), `uuid`, `varint`, `decimal`, `inet`, and `frozen` collections and tuples;
- `successor` and `prefix_end` for range bounds.

Property tests (proptest, as `loams-tikv/tests/codec.rs` does): for every primitive and every composite of two or three, `cmp(a, b) == cmp(enc(a), enc(b))`; `decode(encode(x)) == x`; and no encoding is a proper prefix of another of the same composite shape.

## 8. Redis: Tidis first, Titan (`distributedio/titan`) second (D1641, D1653–D1658)

### 8.1 The owner's ruling (D1641, approved 2026-10-21)

As relayed to this design on 2026-10-21, in two steps (Titan (`distributedio/titan`) first, then Tidis as the primary base):

- **`loams-resp` ports Tidis** (`tidb-incubator/tidis`, Rust, Apache-2.0, a NOTICE file, last commit 2024-01-06) as its primary base. Being Rust, its code is ported and adapted, not only its design: the RESP frame and parse code (`src/frame.rs`, `src/parse.rs`, `src/connection.rs`), the command handlers (`src/cmd/`), the TiKV encoding and type logic (`src/tikv/`, `src/tikv/encoding/`) and the GC (`src/gc.rs`), onto `loams-kv` (redb and TiKV backends) and Loams' client fork.
- **Titan** (`distributedio/titan`, Go, Apache-2.0, last code commit 2021-07-31) is the second reference, for encoding ideas and command semantics where it is better.
- Attribution: Tidis's Apache-2.0 notice and its NOTICE content are kept, and every ported file gets a header naming its origin. `distributedio/titan`'s attribution goes in NOTICE and in the headers of files whose logic follows Titan.
- Both projects' tests become a reference corpus. A Tidis container (built unmodified from source) or an unmodified Titan (`distributedio/titan`) container can act as a differential oracle (§12.3).
- Loams does not fork Tidis or Titan (`distributedio/titan`) as a product and does not ship either as a separate service.

**A finding the ruling has to absorb: Tidis is partly derived from tokio's mini-redis (MIT), without attribution.** `src/frame.rs:1-2` is mini-redis's module doc verbatim; `src/connection.rs:57`, `src/server.rs`, `src/shutdown.rs`, `src/parse.rs`, `src/db.rs` and `src/cmd/{get,set,publish,subscribe,ping,unknown}.rs` follow mini-redis, and `src/cmd/set.rs:128` keeps a mini-redis error string **(source)**. mini-redis is MIT (`tokio-rs/mini-redis`, "Copyright (c) 2020 Tokio Contributors"). A port of those files carries the MIT notice as well as Tidis's (D1649). Tidis's own source files have no headers, and its NOTICE lists its dependencies, not mini-redis.

### 8.2 The two references at a glance

| | Tidis (`tidb-incubator/tidis` `5558562`, 2024-01-06) | Titan (`distributedio/titan` `97868f5`, 2021-10-09) |
|---|---|---|
| Language, client | Rust 2018, `nightly-2022-07-27`; `tikv-client` 0.1.0 from `yongman/client-rust` `dev-scanner` (`a773980`), on grpcio | Go 1.13; TiDB's in-tree `store/tikv` from 2020-12 |
| TiKV API | Transactional (default `use_txn_api = true`) plus a RawKV mode for a string subset. API v1 only: no keyspace, a 2-byte `instance_id` prefix | Transactional only. API v1 only; an ASCII namespace from the AUTH token |
| RESP | RESP2 only; no inline commands; no HELLO | RESP2 with inline commands |
| Commands | Keys, strings, hashes, lists, sets, zsets (incl. ZPOPMIN/MAX, ZRANK, ZINCRBY), EVAL/EVALSHA (mlua, Lua 5.1), MULTI/EXEC, a fake CLUSTER topology. No WATCH, SELECT, KEYS, RENAME, FLUSHDB | Keys, strings (incl. bit ops), hashes, lists, sets, zsets, MULTI/EXEC/WATCH, SELECT 0–255, FLUSHDB/FLUSHALL, ESCAN. No Lua |
| Tests | 269 Python `unittest` cases (`redis==3.5.3`) in `test/` against a live tidis + TiKV (98 of them Lua); no Rust tests | 28 `*_test.go` files (~7,250 lines) on `mocktikv`; `tools/autotest` drives a full server with redigo |

### 8.3 Area by area: whose approach, and why

| Area | Tidis | Titan (`distributedio/titan`) | Loams (D1653) | Why |
|---|---|---|---|---|
| Tenant prefix | `x ‖ inst(u16) ‖ …` | `{ns}:{db3}:…` ASCII, from the AUTH token | `root ‖ 'R' ‖ db(u8) ‖ …` under the namespace's keyspace (D1645) | Keyspaces already isolate tenants (D122); `root` already scopes shared keyspaces. `distributedio/titan`'s ASCII form breaks on `:` in names and drops `-` (`command/common.go:55`) |
| User-key encoding | memcomparable 8-byte groups (`encode_bytes`) | raw bytes, then `:` | `key::bytes` (escaped, terminated) | Prefix-free, already property-tested; `distributedio/titan`'s raw form is ambiguous for keys containing `:` |
| Meta and data placement | Meta `u ‖ enc(ukey) ‖ m`, data under the same user key: adjacent | Meta `M:{ukey}`; data `D:{objID}` by a random UUID: far apart | **Meta in `m`, data in `d ‖ enc(ukey) ‖ ver`**, both prefixed by the user key | `SCAN` and `KEYS` read only the `m` range (Tidis scans every data key and filters, `src/tikv/string.rs:843-933`); data stays keyed by user key, so a collection's data is contiguous (`distributedio/titan`'s UUID keys scatter it) |
| Versioned deletes | `u16` version in meta, `v+1` from a GC key; wraps; `KEY_VERSION_EXHUSTED_ERR` | 16-byte UUID object id | **`u64` version = the start timestamp of the creating transaction** (D1654) | Unique and increasing per key with no extra read, never exhausts, and costs 8 bytes. Tidis's `u16` wraps after 65,535 overwrites and can alias old data not yet collected. The idea is `distributedio/titan`'s (a fresh id makes old data unreachable at once); the size is ours |
| Collection length | Sharded sub-meta counters (`meta_key_number = 100`), summed on HLEN/SCARD/ZCARD | None for hashes (HLEN iterates); an 8-byte length in meta for sets and zsets | **Tidis's sharded counters**, 16 shards by default (Q1226), stored in `d ‖ enc(ukey) ‖ ver ‖ '#' ‖ shard` | A single length in meta makes every HSET on one key conflict (`distributedio/titan`'s own `proposals/hash-design.md` proposed slots and never built them). Placing counters next to the data keeps HSET in one region, so 1PC applies |
| Strings | Inline in meta | Inline in meta | Inline in meta up to `MAX_VALUE_BYTES` minus the header; larger values in 1 MiB chunks under `d` (Q1227) | Both inline; `loams-kv` caps a value at 2 MiB, so large strings need chunks |
| Lists | `u64` indexes from `2^32`; LINSERT and LREM shift keys, with length limits | `float64` indexes; LINSERT takes the midpoint and fails when precision runs out; a ziplist encoding plus a converter worker | **Tidis's integer indexes**, from `2^63`, with LINSERT/LREM length limits | No precision failure; no second encoding and no background converter |
| Zsets | Member → score; score index `S ‖ ver ‖ score ‖ member` → member | Member `D:{id}:M:{member}` → score; score `D:{id}:S:{score}:{member}` | Member `d ‖ … ‖ 'm' ‖ member` → score; score `d ‖ … ‖ 's' ‖ key::f64(score) ‖ member` → empty | The same shape in both; Loams uses its own codec. Redis forbids NaN scores, and `-0`/`0` compare equal in both Redis and `loams-tuple` |
| Expiry | Lazy only: a key that is never read again is never reclaimed | Lazy, plus an expiry index `$sys:0:at:{ts}:{metakey}` → object id, scanned in time order by a worker that checks the id before deleting | **`distributedio/titan`'s design, bucketed**: lazy check on read plus `x ‖ bucket ‖ key::u64(expire_ms) ‖ enc(ukey) ‖ ver`, 16 buckets by a hash of the user key, scanned by a worker (D1655) | Without an index, expired keys leak storage. The version check handles overwrites and stale entries, as Titan's id check does. A single time-ordered index makes every TTL write hit one tail region and leaves runs of tombstones (Titan issue #160); buckets spread both |
| GC of old versions | GC key + GC version key written with the delete; a worker per node over a CRC16 slot split, deleting a whole version in one transaction | A GC queue key per data prefix; a worker deletes up to `batch-limit` (256) keys per transaction | **`distributedio/titan`'s batched deletes, Tidis's queue keys**, one worker per keyspace holding a `loams-kv` lease (D1656) | A whole version in one transaction fails on large collections; a lease needs no etcd (Titan uses PD's embedded etcd) and no topology keys |
| Background worker coordination | Topology keys `x ‖ inst ‖ t ‖ addr` with expiry, slots split across nodes | etcd elections on PD's embedded etcd | A lease row in the same keyspace, renewed by the holder | Works on the embedded backend, which has no PD |
| TiKV MVCC GC | Not done (README: run a TiDB for GC) | `gcworker.RunGCJob` from 2020 TiDB pushes the safe point | **Loams' cluster GC loop** (`loams-tikv` `GcLoop`, D122 as amended by Q32); the embedded GC thread | Already built and running |
| MULTI/EXEC | One transaction; any error aborts all; no retry | One transaction; retried on conflict when no WATCH | One transaction; retried by the runner when no WATCH; **Redis semantics**: a command's runtime error is its reply, the others still apply (D1657) | Clients rely on Redis semantics, which both references bend |
| WATCH | Not implemented | A transaction begun at WATCH, `LockKeys` on the raw user keys (not the stored keys) | An interactive `loams-kv` transaction begun at WATCH, `lock_keys` on the watched keys' **meta** keys, EXEC without retry, nil on conflict | `distributedio/titan`'s idea, with the keys corrected |
| Lua | mlua 0.7.4, Lua 5.1, one VM per connection, script in one transaction | None | **Out of v1** (Q1223) | The draft's non-goal; D209 and D1255 already route BullMQ and Celery elsewhere |
| Cluster mode | Fake `CLUSTER SLOTS/NODES` over the tidis nodes | None | **Out of v1**: one logical node (Q1225) | The draft's non-goal |
| Raw mode | A string subset on RawKV (SET EX silently ignored) | None | **An opt-in fast mode per keyspace, TiKV only**, refusing multi-key commands; built only if KV2's benchmarks show a gain (D1643, Q1236) | API v2 key modes are disjoint, so a keyspace is wholly raw or wholly transactional, never mixed |
| Type checks | `SET` overwrites a hash without checking its type, orphaning the hash's keys (issue #91) | Checks types | **Every write reads and checks the meta key's type** | Tidis #91 is a data-loss bug |
| Binary keys | User keys parsed as UTF-8 `String` (`Parse::next_string`) | Binary | **Binary keys and values everywhere** | Redis keys are byte strings |
| Transaction primary | Whatever the client picks | Percolator primary picked by the client; a lazily updated meta key as a secondary made the next request wait on its lock (issue #180) | **The meta key is the primary** of every transaction that writes it; `loams-kv` gains a primary-key hint (KV2) | `distributedio/titan`'s lesson; with this layout, `d` keys sort before `m` keys, so the client's default choice would be a data key (**verify** in KV2 how the pinned client chooses its primary) |
| RESP3 | None | None | **New**: HELLO 2/3, RESP3 maps, sets, doubles, nulls and push frames (no push sources in v1) | The draft asks for RESP2/3 |
| Parser and encoder | mini-redis-shaped `frame.rs`/`parse.rs` (RESP2, no inline commands) | `encoding/resp/resp.go` with inline commands | **Port Tidis's structure, then harden it**: a streaming decoder with `proto-max-bulk-len` and multibulk limits enforced before allocation, inline commands (`redis-cli`, the TCL suite), a RESP2/RESP3 encoder chosen per connection; `redis-protocol` (MIT) as the property-test oracle (report §6.1) | The ruling's port, plus what a server facing untrusted bytes needs |

**Tidis bugs and limits the port does not copy** (report §7.6, from Tidis's open issues and code): `SET` overwriting a hash (#91); UTF-8-only keys; `u16` versions; deleting a whole old version in one transaction, which fails on big collections with `RESOURCE_EXHAUSTED` (#97; GC deletes in bounded batches and scans page, as `loams-tikv` already halves pages on `OutOfRange`); global `static mut` clients (69 `unsafe` uses, an error under edition 2024's `static_mut_refs`); EXEC aborting every command on one error; `SET` accepting one option only; a key-string SCAN cursor and regex `MATCH`. From `distributedio/titan`'s (report §8.4): a hot meta key (#85), the meta key as a secondary (#180), the single expiry list (#160), unbounded iterators over deleted runs (#162, #84), and a `RANDOMKEY` that loads the keyspace (#230).

### 8.4 The Redis key layout (D1653)

All keys are under the namespace's keyspace and `root` (D1645). `R` is the product tag, `db` the SELECT index (0–15 by default, Q1234), `ver` the 8-byte version of D1654, `enc` is `key::bytes`.

| Item | Key | Value |
|---|---|---|
| Meta | `R ‖ db ‖ 'm' ‖ enc(ukey)` | `[format u8][type u8][ver u64][expire_at_ms u64 or 0][type trailer]`; a small string's bytes follow inline |
| Large string chunk | `R ‖ db ‖ 'd' ‖ enc(ukey) ‖ ver ‖ 'c' ‖ key::u32(i)` | 1 MiB of bytes |
| Hash field | `… 'd' ‖ enc(ukey) ‖ ver ‖ 'h' ‖ enc(field)` | bytes |
| Set member | `… 'd' ‖ enc(ukey) ‖ ver ‖ 's' ‖ enc(member)` | empty |
| Zset member | `… 'd' ‖ enc(ukey) ‖ ver ‖ 'm' ‖ enc(member)` | `f64` score |
| Zset score index | `… 'd' ‖ enc(ukey) ‖ ver ‖ 'z' ‖ key::f64(score) ‖ enc(member)` | empty |
| List element | `… 'd' ‖ enc(ukey) ‖ ver ‖ 'l' ‖ key::u64(index)` | bytes; head and tail in the meta trailer |
| Length shard | `… 'd' ‖ enc(ukey) ‖ ver ‖ '#' ‖ key::u16(shard)` | `i64` partial count |
| Expiry index | `R ‖ db ‖ 'x' ‖ key::u8(bucket) ‖ key::u64(expire_at_ms) ‖ enc(ukey) ‖ ver` | empty |
| GC queue | `R ‖ 'g' ‖ enc(db ‖ ukey) ‖ ver` | type byte |
| Invalidation journal (fallback only, Q1229) | `R ‖ 'j' ‖ key::u64(commit_bucket) ‖ …` | changed meta keys (D1652) |
| Worker leases | `R ‖ 'L' ‖ name` | lease record |

`DEL` or an overwrite of a collection deletes its meta key and writes a GC queue key in the same transaction; the old version is unreachable at once and its keys go later (D1656). Strings are deleted inline.

### 8.5 RESP v1 scope (D1658)

- **Protocol:** RESP2 and RESP3 (HELLO), inline commands, pipelining, `CLIENT SETNAME/GETNAME/ID/INFO/LIST/KILL`, `CLIENT TRACKING` refused, `RESET`, `QUIT`.
- **Commands:** the union of Tidis's and `distributedio/titan`'s data commands in §8.2, plus `SELECT`, `FLUSHDB`, `RENAME`, `KEYS`, `SCAN` with a numeric-looking opaque cursor and glob `MATCH` (Tidis uses a key cursor and a regex), `OBJECT ENCODING`, `TYPE`, `DBSIZE` (estimated, documented), `INFO` sections that clients probe, `COMMAND`, `TIME`, `PING`, `ECHO`.
- **Out of v1:** Lua and functions, pub/sub and keyspace notifications (route to Loams Streams, D1218), streams (`X*`), blocking commands (`BLPOP` family), cluster commands, modules, `MONITOR`, `DEBUG`, `SWAPDB`, `WAIT`, bitfield and HyperLogLog and geo. Each answers `ERR unknown command` or a specific `ERR not supported by Loams` message, listed in the docs.

## 9. Cassandra: CQL (D1659, D1660)

The draft's CQL design is kept, with Loams' codec and names. No reference implementation is ported: Zetta (`zhihu/zetta`, Apache-2.0, last push 2021-04-06) is a table store with a Spanner-like API, not CQL, and is a design reference only. Apache Cassandra (Apache-2.0) is the semantic reference and the source of the protocol specifications (`doc/native_protocol_v4.spec`, `native_protocol_v5.spec`). Cassandra 5.0.9 is the newest stable tag and 6.0 is in alpha (2026-10-21) **(source)**.

| Item | Key (`C` is the product tag) | Value |
|---|---|---|
| Schema | `C ‖ 's' ‖ enc(keyspace) ‖ enc(table) ‖ …` | schema record (versioned; DDL in one transaction) |
| Row | `C ‖ 'c' ‖ key::u32(table_id) ‖ key::u64(token) ‖ pk ‖ ck` | row record: per cell `(write_ts, ttl, value)`; frozen collections inline |
| Non-frozen collection element | `… ‖ pk ‖ ck ‖ key::u16(column_id) ‖ elem` | `(write_ts, ttl, value)` (Q1221) |
| Secondary index | `C ‖ 'i' ‖ table_id ‖ index_id ‖ value ‖ pk ‖ ck` | empty |
| Materialized view row | `C ‖ 'c' ‖ view_table_id ‖ …` | row record, written in the same transaction |
| Expiry index | `C ‖ 'x' ‖ key::u64(expire_at_ms) ‖ table_id ‖ token ‖ pk ‖ ck` | empty |

- `token` is Cassandra's Murmur3 token, so `token()` and token-aware drivers work. **Cassandra's hash is not standard MurmurHash3 x64_128**: `MurmurHash.hash3_x64_128` reads the tail bytes sign-extended (`(long) key.get(i) << n`, "a sign bug" in its own comment), the token is `h1` as a signed `long` with seed 0, `Long.MIN_VALUE` normalises to `Long.MAX_VALUE`, and an empty key gives the minimum token (report §6.3). The `murmur3`, `fastmurmur3` and `mur3` crates all hash the tail correctly and so give different tokens for keys whose tail has a byte ≥ `0x80`. `loams-cql` has its own `cassandra_token(bytes) -> i64`, checked against vectors taken from Cassandra 5.0.9 (`SELECT token(k)`) for tails of 0–15 bytes, high bytes, the empty key and a composite key. In the row key it is `key::u64(token ^ 0x8000_0000_0000_0000)`, so byte order is token order. `system.local` and `system.peers` / `system.peers_v2` list every gateway replica with many virtual tokens, so a driver can route to any of them.
- A partition read is a range scan over `C ‖ 'c' ‖ table_id ‖ token ‖ pk`; TiKV's region splits handle wide partitions.
- **Consistency:** every consistency level is linearizable in v1, and the docs say so. Cell timestamps resolve last-write-wins inside a row record; nothing needs tombstones, because a delete is a transactional delete.
- **LWT** (`IF NOT EXISTS`, `IF col = …`) runs as one **pessimistic** transaction: `get_for_update` on the row key, check, write (D1660, Q1237). Pessimistic mode (D851, built in KV0) makes concurrent LWTs on one row queue instead of retrying, which is what Cassandra users expect of Paxos-backed LWT under contention. An optimistic transaction with `lock_keys` on the row key is also correct and is the fallback if KV0's pessimistic mode slips.
- `BATCH` (logged and unlogged) is one transaction. Paging state is the last key read. Prepared statements are cached per connection and per gateway.
- **Building blocks** (report §6.2, Q1238): frames, v5 segments, compression and value types from **`cassandra-protocol`** (MIT/Apache-2.0, 4.0.1; it decodes requests and encodes responses, the server's direction, and shotover-proxy uses it so), wrapped behind `loams-cql`'s own trait; a **hand-written recursive-descent parser** for the Phase 3 subset, following Cassandra's ANTLR grammar (`Parser.g`, `Lexer.g`, Apache-2.0) as the specification, with `cql3-parser` only as a test oracle; the `scylla` Rust driver (MIT OR Apache-2.0) as the CQL client in `loams-gw-test`.
- **Out of v1:** UDFs and UDAs, triggers, CDC tables, Scylla extensions (shard-aware ports, Alternator), `ALTER TYPE` on a used type, `SASI`/`SAI` index types beyond the plain secondary index, and the DSE protocol versions.

## 10. Where the gateways run (D1646, D1647, D1648)

### 10.1 In `loams-gateway`

`resp` and `cql` are modules of `loams-gateway`, behind the cargo features `resp` and `cql`, served from their own listeners (6379 and 9042 by default). They terminate their protocols and call `loams-kv` in process; there is no upstream. The gateway has no module seam today (`listeners.rs` builds `PgFrontDoor` directly), so KV2's first task adds one: a `Module` enum with a listener per module, and config sections `[resp]` and `[cql]` (the config keeps `deny_unknown_fields`). Gateway metrics follow D1021: `loams_gateway_connections_total{module="resp",…}` with no tenant label.

The draft's "router AGPL" process does not exist. Loams' router is the namespace router of §18, and nothing in Loams is AGPL (D11, D1119).

### 10.2 Deployments

| Install | Shape |
|---|---|
| **Light compose (0.1.1 default, #973)** | Opt-in `kv` profile: one `loams-gateway` service with the `resp` and `cql` modules on the **embedded backend**, its redb file in its own volume (`kv-data`), **one replica**. Listeners on the compose network, published to the host on `127.0.0.1` only. Reached from other machines over the tailnet |
| **Compose with `tikv`** | The same service with `--kv-store tikv://pd:2379/<keyspace>`; more than one replica is possible |
| **Kubernetes (operator, §65)** | A Deployment `loams-gateway-kv`, at least two replicas across zones, a PDB, on TiKV; Service exposure `tailnet` or `internal` only (D1195) |
| **BYOC (§70)** | As compose or Kubernetes in the customer's account; the same exposure rules |

A gateway on the embedded backend refuses to start a second replica on the same file (redb's lock already does this; the gateway reports it plainly). Moving a keyspace from embedded to TiKV is an export and import, not a live migration (a KV4 tool).

### 10.3 Never public (D1648)

The RESP and CQL listeners are never on the public internet, in every install. They bind to loopback, the compose network, the tailnet or an internal Service, as the engine does (D1195, D1311). They are not offered through §69's edge listener in v1: that listener is HTTP, and a raw TCP path from serverless runtimes is a separate design. Off loopback, TLS (rustls) and authentication are required: RESP `AUTH`/`HELLO … AUTH` and CQL SASL `PLAIN`, with a Loams token as the password, checked through §76's `Authorizer` (Q1224, Q1232). The pre-auth parsers are fuzzed, with size limits and a handshake deadline, as D1020 requires of the `pg` module.

## 11. `loams-kv-hot` (D1650–D1652)

### 11.1 The name and the crate (D1650)

A new crate, **`loams-kv-hot`**, layered on `loams-cache`. `loams-cache` gains one small public piece, its foyer hybrid-cache builder (RAM bytes, an NVMe directory and capacity, cold start, foyer 0.22 from the workspace pin), which `RangeCache` keeps using; `loams-kv-hot` builds its own `HybridCache<Vec<u8>, Bytes>` with it and adds what immutable objects never needed: TTL, versioned values, the write paths of §11.3 and invalidation. `RangeCache`'s API and behaviour do not change. **`loams-hot` is not renamed or reused**: it is the vector tier. Q1230 records the name and the alternatives (`loams-kvcache`, or a `kv` module inside `loams-cache`).

### 11.2 v1: a read-through cache

Hit in RAM or NVMe; on a miss, read through `loams-kv` and populate. Cached values carry the meta key's version and a fill timestamp. Coherence is chosen per keyspace (D1652):

1. **Local write-through** (the default when one gateway owns the keyspace, which is always true on the embedded backend): the gateway updates or drops its cache entry after the commit returns.
2. **Versioned values with a short TTL**: for data that tolerates seconds of staleness. The TTL bounds staleness; the version prevents an older fill from replacing a newer one.
3. **A change-feed consumer of Loams' own** (when several gateways share a keyspace on TiKV): each gateway subscribes to TiKV's `cdcpb.ChangeData/EventFeed` (or `EventFeedV2`) for the keyspace's regions, with `kv_api = TxnKV` (or `RawKV` for a raw fast-mode keyspace), from a checkpoint timestamp, and drops the cache entries of the meta keys that changed. The client fork already exports the generated `ChangeDataClient` (`tikv_client::proto::cdcpb`); the consumer is new code that handles region registration, splits, epoch changes and resolved timestamps, which is what TiCDC's kv client does, so it is budgeted as real work (KV1). Its design is **ported from TiKV-CDC's stream consumer** in `tikv/migration` (`cdc/cdc/kv/client.go`, `region_worker.go`, `matcher.go`, `resolvedts_heap.go`, `token_region.go`, `grpc_pool*.go`; Apache-2.0, Go, itself descended from TiCDC's kv client), with the attribution D1649 requires, and differs in one parameter: TiKV-CDC hard-codes `KvApi: cdcpb.ChangeDataRequest_RawKV` (`client.go:716`), ours takes `TxnKV` or `RawKV`. It lives in `loams-tikv` (a `cdc` module), so other Loams components can use it, and is offered upstream to `tikv/client-rust`. **Running TiKV-CDC unmodified is not recommended:** it captures RawKV only, so it would cover only opt-in fast-mode keyspaces, and its one sink an application can consume is Kafka, which no Loams install runs (Iggy is designed, not built), so the gateways would need Kafka plus a Kafka consumer for a minority of keyspaces (§13.2). **Fallback:** if the consumer is not ready, an invalidation journal written in the same transaction (`R ‖ 'j' ‖ commit_bucket ‖ …`) and polled at 100 ms, as `loams-meta-tikv`'s change feed polls (Q1229).

Reads that miss use TiKV's default lease reads. Follower and stale reads are a KV4 option for keyspaces marked non-critical.

### 11.3 Durability modes (D1651)

| Mode | Write path | Typical use | Durability | Backends |
|---|---|---|---|---|
| `durable` (**default**, Q1220) | `loams-kv` transaction, acknowledged after commit (Raft on TiKV, redb fsync on embedded); the cache entry is dropped | everything by default | the store's | both |
| `write-through` | as `durable`, then the cache entry is populated with the committed value | read-heavy hot keys | the store's | both |
| `hot-only` | written to the hot tier, acknowledged, drained to `loams-kv` asynchronously | ephemeral counters and caps | **a bounded loss window**, documented, and reported per keyspace | both; opt-in per keyspace |
| `hot-replicated` (v2, Phase 5 gate) | a 2-replica acknowledgement inside `loams-kv-hot`, then the drain | low-latency writes | survives one node, not an AZ | TiKV installs only |

`hot-only` and `hot-replicated` are never defaults, are refused for CQL tables with LWT and for Redis keys inside MULTI/EXEC, and are listed by `INFO`/`system_views` so an operator can see them. This is the §04 amendment: the hot tier can lose acknowledged writes in these two modes, and nowhere else.

### 11.4 v2: an engine, only behind the gate

A sharded in-memory index, a log-structured NVMe write buffer, a 2-replica acknowledgement and an asynchronous drain to TiKV: the draft's design, unchanged. It is built only if Phase 5's measurement on tuned TiKV misses a real user's p99 write target (D1663).

## 12. Conformance, benchmarks and their licences (D1661)

The report's §1–§3 give the suites, their external-server flags, the first subsets and the benchmark matrix; this design adopts them (D1661). This section fixes the licence rules they must follow; each row was read at the source on 2026-10-21, and agrees with the report.

### 12.1 Licences, verified

| Project | Licence (source) | Use |
|---|---|---|
| **Redis** (`redis/redis`) | **7.2 and earlier: BSD-3.** **7.4: RSALv2 or SSPLv1. 8.0 and later: RSALv2, SSPLv1 or AGPLv3** (`LICENSE.txt` and `REDISCONTRIBUTIONS.txt` on `unstable`; GitHub reports `NOASSERTION`) | **Not used** from 7.4 on: its `tests/` TCL suite, `redis-benchmark` and server are under the same terms. D1271 already rules Redis 8 out. The BSD 7.2 line (7.2.16, 2026-08-17, still patched) may serve as an occasional second opinion, never as the gate (report §1.1) |
| **Valkey** (`valkey-io/valkey`) | **BSD-3-Clause** (`COPYING`: "Copyright (c) 2024-present, Valkey contributors; Copyright (c) 2006-2020, Redis Ltd.") | **The Redis conformance suite, at a pinned tag** (`9.1.2`, 2026-09-01; re-pinned deliberately): an unmodified checkout of `tests/` fetched by CI at test time, never copied into the repository, run with `./runtest --host --port --singledb --ignore-encoding --ignore-digest`, the report §1.4's first subset and tag denials, and a checked-in skip list with one reason per test that only shrinks. The same command line must pass on Valkey itself. Also `valkey-benchmark`, and Valkey 9.1.2 as the semantic oracle |
| **Apache Cassandra** (`apache/cassandra`) | Apache-2.0 | Semantics, protocol specs, `tools/stress` (**cassandra-stress** is in-tree under `tools/stress`) |
| `apache/cassandra-dtest` | Apache-2.0 | Selected CQL tests (Python) |
| Drivers: `apache/cassandra-java-driver`, `apache/cassandra-python-driver` (was `datastax/python-driver`), `apache/cassandra-gocql-driver`, `apache/cassandra-cpp-driver` | Apache-2.0 each | Their integration-test subsets as the CQL gate: the Python driver (`USE_CASS_EXTERNAL`), gocql (`-cluster`) and the Rust driver (`SCYLLA_URI`) target an external cluster; the Java driver's suite always launches CCM, so it needs a custom rule or is left out (report §2.2) |
| **go-ycsb** (`pingcap/go-ycsb`) | Apache-2.0 | **The benchmark for all three**: workloads A, B, C and F through its `redis` binding against `loams-resp`, its `cassandra` binding against `loams-cql`, and its `tikv` binding against bare TiKV, so each gateway's overhead over the store is visible with one tool and one workload definition; `cassandra-stress` and `valkey-benchmark` stay as protocol-native second opinions. YCSB itself (Apache-2.0) has had no release since 0.17.0 (2019), and its Java `redis` binding makes every insert a two-key write (report §3) |
| **ScyllaDB** (`scylladb/scylladb`) | **ScyllaDB Software License Agreement** v1.1 (updated 2026-04-12) for 2025.1 and later: source-available, restricted to "Never Customers", modifications assigned to ScyllaDB. 6.2.x and earlier: AGPL-3.0 (§11 §1.1) | **No code, tests or binaries** reused here. D60's unmodified Alternator 6.2.3 CI service is unrelated and unchanged |
| `scylladb/scylla-rust-driver` (crates `scylla`, `scylla-cql`: MIT OR Apache-2.0; GitHub reports Apache-2.0), `scylladb/cassandra-stress` (Apache-2.0) | permissive | Usable as clients and load tools. **The rule for ScyllaDB:** never read, copy or port code or tests from `scylladb/scylladb` (any version, since Loams competes with it); its drivers and tools under permissive licences are fine to depend on and learn from (report §2.4) |
| **Titan** (`distributedio/titan`) | Apache-2.0 (`LICENSE`; no NOTICE) | Reference, tests as corpus, optional oracle (D1641) |
| **Tidis** (`tidb-incubator/tidis`) | Apache-2.0 with a NOTICE | Primary base for `loams-resp` (D1641) |
| **mini-redis** (`tokio-rs/mini-redis`) | MIT | Inherited by the Tidis files derived from it; attributed (D1649) |
| **Zetta** (`zhihu/zetta`) | Apache-2.0 | Design reference only |
| `tikv/migration` (TiKV-CDC in `cdc/`, RawKV BR in `br/`) | Apache-2.0 | TiKV-CDC's stream consumer is **ported** for D1652's `loams-tikv::cdc` (attributed); TiKV-CDC itself is not run. Its RawKV BR is the candidate backup tool for opt-in raw fast-mode keyspaces (**verify** in KV2; transactional keyspaces follow §54's BR snapshot) |
| `memtier_benchmark` (`redis/memtier_benchmark`) | GPL-2.0 | **Run only**, as an unmodified separate binary in benchmark jobs; never vendored, linked or copied (Q1231). `valkey-benchmark` and go-ycsb are the defaults |
| `resp-compatibility` (`tair-opensource/resp-compatibility`) | MIT | A per-Redis-version pass rate as a headline number, beside the TCL gate |
| `cassandra-protocol` (`krojew/cdrs-tokio`), `redis-protocol` (`aembke/redis-protocol.rs`), `cql3-parser` | MIT/Apache-2.0, MIT, Apache-2.0 | Framing dependency (Q1238) and test oracles (§8.3, §9) |
| Client test drivers: `redis-py`, `valkey-py` | MIT | Test clients |

### 12.2 Tidis's dependencies

`cargo deny` was **not run** on Tidis: it resolves the git dependency `yongman/client-rust`, which means cargo cloning that repository, and the workspace rule is not to clone without asking. The same check was done from `Cargo.lock` (301 packages; licences read from the local cargo registry, else crates.io's API). Findings:

- **`inferno` 0.11.7 is CDDL-1.0**, through `pprof`'s `flamegraph` feature. D808 removed CDDL from Loams' builds and `deny.toml` does not allow it.
- `ring` 0.16.20 and `webpki` 0.21.4 have non-SPDX licence texts; `rustls` 0.19.1 is far behind the workspace's 0.23 (with known advisories in that line).
- `grpcio` 0.10.3 with `grpcio-sys` (C++ gRPC) and `openssl-sys`: Loams' client fork is on tonic.
- `mlua` 0.7.4 (MIT) with vendored Lua 5.1 (MIT): needed only if Lua comes in (Q1223).
- Everything else is MIT, Apache-2.0, BSD, ISC, Zlib, MPL-2.0-or-MIT, BSL-1.0 (Boost) or Unicode.

**None of these dependencies is carried into `loams-resp`.** The port uses the workspace's `tokio`, `rustls` 0.23, `bytes`, `thiserror`, `tracing` and `loams-kv`; `loams-resp` passes the workspace `cargo deny`.

### 12.3 The differential oracle

| Candidate | Works against Loams' TiKV? | Use |
|---|---|---|
| **Valkey 9.1.2**, unmodified official image | Needs no TiKV | **Primary semantic oracle** (Q1222). It is Redis's behaviour; Tidis and Titan (`distributedio/titan`) both bend it (§8.3) |
| **Titan** (`distributedio/titan`) `97868f5`, unmodified, on `mocktikv://` | **Not on an API v2 cluster** (keys `default:000:M:…` and `$sys:…`; a 2020 client with no `api_version`, refused by `check_api_version`). It **needs no TiKV** in its documented `mocktikv://` mode (`conf/titan.toml`): one container, in-process, deterministic. `go.sum` is git-ignored, so its build resolves 2020 module versions from the Go proxy (**verify**, KV0 Task 9) | **Second oracle**: "what does a Redis-on-TiKV layout reply", with an allowlist of Titan's known deviations |
| **Tidis** `5558562`, built unmodified from source | **No, not on an API v2 cluster**: its 2022 client sends API v1 requests with `x`-prefixed keys, and TiKV v8.5 accepts v1 transactional requests on a v2 store only for TiDB-mode keys (`m`, `t`). It also fails on API v1 with `enable-ttl = true` (issue #109, `ApiVersionNotMatched … V1ttl`), and expects a TiDB to drive MVCC GC. It would need a throwaway TiKV with `api-version = 1`, `enable-ttl = false` and a GC trigger. Its `Dockerfile` builds on `centos:7.6.1810`, which is end of life, and no image is published | **Not run by default.** Its Python `test/` suite runs against any server (`test_helper.py --ip --port`), so its tests are used without running Tidis itself |

Both projects' tests are the reference corpus (D1641): Tidis's 269 Python cases run against Valkey (the control) and `loams-resp`; `distributedio/titan`'s Go auto tests need a small patch to target an external server (report §8.2), which lives in `loams-gw-test`'s harness, not in Titan.

## 13. TiKV specifics, verified

### 13.1 RawKV CAS

- **There is no server setting for atomic CAS.** TiKV v8.5's `[storage]` has `api-version` and `enable-ttl` (API v2 requires `enable-ttl = true`); there is no `enable-atomic-for-cas` (`src/storage/config.rs` at v8.5.0) **(source)**. "Atomic mode" is a **client** mode: `RawClient::with_atomic_for_cas()` in client-rust (client-go's `SetAtomicForCAS`), under which puts and deletes take the CAS path, and every writer of a CAS'd key must use it.
- RawKV CAS works on API v1, v1-TTL and v2. Loams' fork has it (`src/raw/client.rs:204` `with_atomic_for_cas`, `:684` `compare_and_swap`, keyspace-encoded) and `loams-safekeeper` uses it.
- **The deciding fact is API v2's key modes.** Raw keys start with `r` and transactional keys with `x` (plus the keyspace id); a raw command on an `x` key or a transactional command on an `r` key is refused (`check_api_version`, `src/storage/mod.rs:469-526` at v8.5.0) **(source)**. Single-key commands on RawKV and MULTI/EXEC on transactions cannot share keys. Hence D1643: a keyspace is transactional, or (opt-in) wholly raw.
- In a raw fast-mode keyspace every write goes through the atomic client, because plain puts and CAS on the same key must not mix; atomic-mode writes cost more (the client's own doc), which is one reason the mode waits for KV2's numbers. RawKV TTL has seconds resolution, so `PEXPIRE` still needs Loams' own millisecond expiry.
- Loams' clusters already run `api-version = 2` and `enable-ttl = true` (`deploy/release/config/tikv.toml`, D122), TiKV v8.5.8 in compose and CI.

### 13.2 A RawKV change feed

- **TiKV-CDC** (`tikv/migration/cdc`, Apache-2.0, Go, a fork of TiCDC) is RawKV's change-data capture. It needs TiKV v6.2.0 or later and API v2 (README; tikv.org CDC docs) **(source)**. Its latest release is `cdc-v1.3.0-beta` (2024-06-22); the repository's last commit is 2026-03-09, and no TiKV 8.5 compatibility statement was found (**verify** if it is ever run).
- **Sinks, corrected:** tikv.org's page says "currently the scheme supports `tikv` only", and the research report repeated it. The code on `main` registers three sink schemes, `tikv`, `kafka` (and `kafka+ssl`) and `blackhole` (`cdc/cdc/sink/sink.go`), and the Kafka sink shipped in `cdc-v1.3.0-beta` (its release notes: "Support Kafka sink", PR #380) **(source)**.
- **RawKV only:** its kv client subscribes with `KvApi: cdcpb.ChangeDataRequest_RawKV` (`cdc/cdc/kv/client.go:716`) **(source)**, so it does not capture transactional keyspaces. TiCDC replicates TiDB tables, not bare TxnKV keyspaces.
- TiCDC's own docs list "a TiKV cluster that uses RawKV alone" as unsupported (report §4).
- **TiKV's `ChangeData` service is the feed to consume directly.** `cdcpb.ChangeData/EventFeed` takes `ChangeDataRequest.kv_api` of `TiDB`, `RawKV` or `TxnKV` per region from a checkpoint timestamp (`proto/cdcpb.proto` in the fork). Loams' client fork exports the generated client (`tikv_client::proto`, from the `feat/public-proto` topic) but has no wrapper; Tidis's fork has neither. D1652 builds the consumer; Loams' §20 R1 chose an in-transaction journal for Live, which stays this design's fallback.

### 13.3 Loams' client fork against Tidis's

Loams pins `ostrium-labs/client-rust` `41b3588`: upstream `1b883bf` (2026-10-19) plus TSO-stream reconnect, public protos, bounded async-commit commit timestamps, `fail` 0.5 and read-past-pushed-locks (r1-dependency-spike §(i)). Tidis pins `yongman/client-rust` `dev-scanner` `a773980` (2022-09-07): 24 commits on upstream `027a7dff` (2022-07-27), 119 behind upstream master **(source: GitHub compare)**.

| Tidis's fork adds | In Loams' pin? | Gap for the port |
|---|---|---|
| Streaming transactional scans (`scan_stream`, `scan_keys_stream`, `scan_reverse_stream`, scan batch size) | **Yes, differently**: upstream's paginated `scanner()` (#554, 2026-08-27) is in the pin. **Not exposed by `loams-kv`** | KV0 Task 3 exposes a scanner on both backends; reverse paging is checked there |
| Client-side request batching over TiKV's `BatchCommands` stream ("super batch", `tikv-client-store/src/batch.rs`, `with_kv_allow_batch` and its knobs) | **No** (only the generated protos) | A throughput item for Phase 5 if measurements need it, as a topic branch offered upstream |
| `Timestamp::from_version(u64::MAX)` read-only transactions with no TSO call (`begin_with_latest`) | **No** | **Not adopted**: a read at the maximum timestamp is not a snapshot and skips the TSO ordering that MULTI/EXEC relies on. TSO cost is measured in KV2 instead |
| `TransactionClient::new_transaction(ts, opts)` made public | No | Not needed |
| Raw CAS, raw TTL, async commit, 1PC, pessimistic transactions | Yes (upstream) | Not a gap |
| A CDC client | Generated `cdcpb` protos only (ours); nothing (Tidis's) | The EventFeed consumer of D1652 (KV1) |

## 14. Phases (D1663)

The owner's phases, mapped onto the crates and the plan's milestones. Estimates are **(estimate)**.

| Phase | Milestone | Content | Exit |
|---|---|---|---|
| **0** | **KV0** | `loams_tuple::key` with property tests; `loams-kv` one-shot ops, scanner, in-memory embedded store, pessimistic mode (D851), op metrics, `tenant`; `loams-gw-test` skeleton; attribution groundwork; the oracle feasibility spike; the dev playground's TiKV default to v8.5.8 (Q1239) | Codec property tests pass; the embedded, in-memory and TiKV stores pass the same conformance suite, extended cases included; the spike report. ~3–4 engineer-weeks |
| **1** | **KV1** | `loams-kv-hot` v1 on `loams-cache`'s builder: read-through cache, the three coherence options, TTL-bounded staleness; the `ChangeData` EventFeed consumer in `loams-tikv` (or the journal fallback) | Hit rate and staleness tests; invalidation latency measured on three gateways; the consumer survives region splits, leader transfers and a TiKV restart without missing a change. ~4–5 weeks. Can run in parallel with KV2, because `durable` needs no cache |
| **2** | **KV2** | The gateway module seam; `loams-resp` MVP ported from Tidis: RESP2/3, strings, counters, hashes, sets, zsets, lists, TTL with the expiry worker, GC worker, MULTI/EXEC/WATCH, durability modes per keyspace, auth and TLS; the interactive `loams-kv` transaction | The chosen Valkey `tests/` subset passes; Tidis's and `distributedio/titan`'s corpora triaged; the differential run against Valkey clean except documented deviations; `valkey-benchmark` per durability mode on both backends. ~5–7 weeks |
| **3a** | **KV3a** | `loams-cql`: frames, handshake, SASL, `system.local`/`peers`, schema DDL, single-partition reads and writes, prepared statements, paging | The drivers' integration subset for these features (Java, Python, gocql). ~5–6 weeks |
| **3b** | **KV3b** | LWT, batches, TTL, secondary indexes, materialized views, collections (Q1221 decided at its start) | The full driver subset; `cassandra-stress` recorded. ~4–6 weeks |
| **4** | **KV4** | Hardening: `loams-sim` runs with injected failures on the in-memory, embedded and TiKV stores; linearizability checks for counters, MULTI/EXEC and LWT; chaos on leader transfers and region splits; optional TLA+ for MULTI/EXEC and LWT; fuzzing of both parsers; export/import between backends | No linearizability violation; fuzzers clean for a week of nightly runs |
| **5** | **KV5** | The gate for `loams-kv-hot` v2: go-ycsb A, B, C and F (`redis`, `cassandra` and bare `tikv` bindings) on tuned TiKV against each durability mode | A written decision: build v2 only if a real user's p99 write target is missed after tuning |

## 15. Risks

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R1 | Users expect Redis latency on durable writes; TiKV's floor is milliseconds | High | Medium | Docs say plainly: hits are fast, durable writes are ms-class; `write-through` for hot reads; the Phase 5 gate |
| R2 | The protocol-trap D42 warned about: every Redis or Cassandra feature becomes a request | High | High | §8.5 and §9's out-of-scope lists; `ERR not supported by Loams` messages; a public compatibility matrix |
| R3 | Porting Tidis imports its bugs | Medium | Medium | Valkey is the semantic oracle, not Tidis; Tidis's deviations are an explicit allowlist that shrinks |
| R4 | A collection's meta key becomes a hot key | Medium | Medium | Sharded length counters; meta written only on create, expire and delete; load-based region split |
| R5 | The embedded backend's one-process limit surprises operators | Medium | Low | One replica by design (D1647); a plain start-up error; the docs point to the `tikv` profile |
| R6 | CQL semantic drift (cell timestamps, collections, TTL interplay) | Medium | Medium | Pin to Cassandra 5.0.x behaviour; the drivers' subsets; dtests where they apply |
| R7 | The EventFeed consumer is harder than it looks (region registration, splits, epoch changes, resolved timestamps) | Medium | Medium | Budgeted as real work in KV1; chaos tests on splits and leader transfers; the in-transaction journal as the fallback; only needed when several gateways share a keyspace |
| R8 | Licence slips: Redis-licensed tests, Scylla code or a CDDL dependency come in by accident | Low | High | D1661's table; `cargo deny`; a CI check that ported files carry their headers |
| R9 | Noisy neighbours on shared keyspaces | Medium | Medium | D122's size classes; per-namespace limits through Meter's gate (§63 D1125) |

## 16. Decisions and questions

### 16.1 Decisions (D1640–D1665)

| ID | Decision | § | Status |
|---|---|---|---|
| D1640 | **Scope:** Redis (RESP2/3) and Cassandra (CQL native protocol v4/v5) gateways on `loams-kv`; no Aerospike. Amends D42 (protocol footprint), §26's "No Redis protocol", D1255 and D1272 (no RESP server); D208, D209, D1271 and Q1039's answer stand | 1, 2 | Approved (owner, 2026-10-21) |
| D1641 | **`loams-resp` ports Tidis's code (primary base) and uses Titan (`distributedio/titan`) as the second reference**, onto `loams-kv` (redb and TiKV) and Loams' client fork; attribution kept (Tidis's NOTICE content, Titan, and mini-redis for the files Tidis derived from it) with file headers on ported files; both projects' tests are a reference corpus; an unmodified Tidis or Titan build may serve as a differential oracle; neither is forked as a product or shipped | 8.1 | **Approved** (owner, 2026-10-21) |
| D1642 | **No second `loams-kv`:** the draft's `KvStore` is `loams-kv`'s `Store`/`Txn`/`Snap`, extended in place (Q1230). Extensions: one-shot `get`/`put`/`insert`/`delete`/`compare_and_put`, a paged scanner, an in-memory embedded store, pessimistic mode (D851), op metrics with a `KvObserver`, a `tenant` module; later an interactive transaction and a primary-key hint | 6 | Approved (owner, 2026-10-21) |
| D1643 | **Transactional keys by default for every Redis and CQL keyspace**, with 1PC and async commit: API v2's raw and transactional key modes are mutually invisible, the embedded backend has no RawKV, and D237 measured RawKV CAS slower than 1PC. A RawKV "fast" mode is an opt-in per Redis keyspace, TiKV only, for keyspaces that never use multi-key commands, which it refuses (Q1236). Never both modes in one keyspace | 1, 13.1 | Approved (owner, 2026-10-21) |
| D1644 | **Codec:** `loams_tuple::key`, untagged memcomparable primitives (u16/u64, i64, f64, escaped bytes, descending, Cassandra orderings later) with property tests; `tuple`'s encoding unchanged | 7 | Approved (owner, 2026-10-21) |
| D1645 | **Tenancy:** a namespace maps to a dedicated keyspace or a shared keyspace plus root (D122); under the root, product tags `R` (Redis) and `C` (CQL) | 6, 8.4, 9 | Approved (owner, 2026-10-21) |
| D1646 | **Where:** modules `resp` and `cql` of `loams-gateway` (features `resp`, `cql`; crates `loams-resp`, `loams-cql`), in their own Deployment `loams-gateway-kv`; no "router" process. Amends D1010 and D1022's module list | 5, 10.1 | Approved (owner, 2026-10-21) |
| D1647 | **The light default:** the gateways run on the embedded backend with their own redb file and one replica, behind a `kv` compose profile; TiKV (`tikv` profile, Kubernetes) for more replicas. Depends on #973 (open; D1340–D1343) | 6, 10.2 | Approved (owner, 2026-10-21) |
| D1648 | **Never public:** RESP and CQL listeners bind to loopback, the compose network, the tailnet or an internal Service; not behind the edge listener in v1; TLS and authentication off loopback; fuzzed pre-auth parsers with limits | 10.3 | Approved (owner, 2026-10-21) |
| D1649 | **Licensing:** every crate here is Apache-2.0; there is no AGPL router and no closed platform; ported files keep their origin's notice (Tidis Apache-2.0 + NOTICE, mini-redis MIT, Titan (`distributedio/titan`) Apache-2.0) in NOTICE and file headers | 8.1, 12 | Approved (owner, 2026-10-21) |
| D1650 | **The KV hot tier is a new crate, `loams-kv-hot`**, layered on `loams-cache` (whose foyer hybrid-cache builder becomes public), adding TTL, versioned values, write paths and invalidation; `loams-hot` (the vector tier) is unchanged, and `RangeCache` keeps its API | 11.1 | Approved (owner, 2026-10-21) |
| D1651 | **Durability modes:** `durable` (default), `write-through`, `hot-only` (opt-in, bounded loss window) and `hot-replicated` (v2, behind the gate, TiKV only); the two lossy modes are refused for LWT and MULTI/EXEC. Amends §04's principle for those two modes only | 11.3 | Approved (owner, 2026-10-21) |
| D1652 | **Coherence:** local write-through (default when one gateway owns the keyspace), versioned values with a short TTL, and for shared keyspaces Loams' own consumer of TiKV's `ChangeData/EventFeed` (a `cdc` module in `loams-tikv`, on the fork's generated protos), ported from TiKV-CDC's stream consumer in `tikv/migration` and parameterised by `kv_api` (TxnKV by default); TiKV-CDC itself is not run; an in-transaction journal as the fallback | 11.2 | Approved (owner, 2026-10-21) |
| D1653 | **The Redis key layout of §8.4** and the per-area choices of §8.3 (binary escaped keys, meta in `m` and data under the user key in `d`, a type check on every write, sharded length counters, integer list indexes, `distributedio/titan`'s expiry index bucketed, the meta key as the transaction primary) | 8.3, 8.4 | Approved (owner, 2026-10-21) |
| D1654 | **Versions are the creating transaction's start timestamp** (u64), not Tidis's u16 or `distributedio/titan`'s UUID | 8.3 | Approved (owner, 2026-10-21) |
| D1655 | **Expiry:** a lazy check on every read plus an expiry index in 16 hash buckets, each scanned in time order by a leased worker that checks the version before deleting | 8.3 | Approved (owner, 2026-10-21) |
| D1656 | **GC:** an overwrite or delete makes the old version unreachable at once and queues it; one leased worker per keyspace deletes queued versions in batches (default 256 keys per transaction); TiKV MVCC GC stays Loams' cluster GC loop | 8.3 | Approved (owner, 2026-10-21) |
| D1657 | **MULTI/EXEC/WATCH with Redis semantics:** one transaction; a command's runtime error is its reply; runner retries without WATCH; WATCH begins an optimistic interactive transaction that locks the watched meta keys, and EXEC returns nil on conflict (WATCH never blocks other writers, so it does not use pessimistic locks) | 8.3 | Approved (owner, 2026-10-21) |
| D1658 | **RESP v1 scope** as in §8.5 | 8.5 | Approved (owner, 2026-10-21) |
| D1659 | **The CQL key layout of §9**, Murmur3 tokens, `system.local`/`peers` listing every gateway | 9 | Approved (owner, 2026-10-21) |
| D1660 | **CQL semantics:** all consistency levels linearizable; cell timestamps last-write-wins; no tombstones; LWT as a pessimistic transaction (`get_for_update` on the row key), with optimistic `lock_keys` as the fallback; BATCH as one transaction; Cassandra's sign-extended Murmur3 tokens, checked against Cassandra 5.0.9 vectors | 9 | Approved (owner, 2026-10-21) |
| D1661 | **Conformance and licences:** Valkey's BSD-3 TCL suite at a pinned tag (9.1.2), fetched unmodified at test time, with a shrinking skip list, and `valkey-benchmark`; nothing from Redis 7.4 or later; Cassandra, dtest, the Apache drivers and cassandra-stress (Apache-2.0); go-ycsb (Apache-2.0) as the YCSB and the one benchmark across Redis, CQL and bare TiKV; `memtier_benchmark` (GPL-2.0) run only, never vendored or linked; ScyllaDB's server code and tests off-limits; Valkey as the primary semantic oracle and Titan (`distributedio/titan`) on `mocktikv` as the second | 12 | Approved (owner, 2026-10-21) |
| D1662 | **Metrics:** `loams_kv_*` and `loams_gateway_*` without tenant labels (D1021); per-namespace counts only through observer hooks into Meter (§63), not written to Iceberg by the gateways | 6 | Approved (owner, 2026-10-21) |
| D1663 | **Phases 0–5 as KV0–KV5** in plan KV1, with the v2 hot engine behind the Phase 5 gate | 14 | Approved (owner, 2026-10-21) |
| D1664 | **`loams-gw-test`** is the harness crate: conformance runners (Valkey `tests/`, driver subsets, the corpora), the differential runner and the simulation entry points | 14 | Approved (owner, 2026-10-21) |
| D1665 | **No Aerospike gateway, and no DynamoDB-API gateway planned**; revisit only on demand from users | 3 | Approved (owner, 2026-10-21) |

### 16.2 Questions (Q1220–Q1239), answered 2026-10-21 by the owner: the default

| ID | Question | Default (answered 2026-10-21 by the owner: the default) |
|---|---|---|
| Q1220 | Redis's default durability mode (the owner's open question)? | **`durable`**, with the cache off until a keyspace opts in; `write-through` becomes the default only if KV2's benchmarks show a read gain with no correctness cost on both backends |
| Q1221 | CQL collections: one KV per row, or per-element keys for non-frozen collections (the owner's open question)? | **Per-element keys for non-frozen collections** (blind appends and element updates, as Cassandra does them, without rewriting the row); frozen collections and scalars inline in the row; confirmed by a measurement at KV3b's start |
| Q1222 | The differential oracle? | **Valkey 9.1.2** as the semantic oracle; **Titan (`distributedio/titan`) unmodified on `mocktikv://`** as the second (no cluster needed), if KV0 Task 9 shows it builds; Tidis not run (it needs a throwaway API v1 TiKV with `enable-ttl = false` and a GC trigger), but its Python tests are run |
| Q1223 | Lua (`EVAL`/`EVALSHA`) in `loams-resp`? | **Not in v1.** Port Tidis's mlua code later only on user demand; D209 and D1255 stand either way |
| Q1224 | RESP authentication? | `AUTH <user> <token>` and `HELLO 3 AUTH <user> <token>`, user = namespace, token = a Loams token checked by §76's `Authorizer`; `AUTH <token>` alone takes the namespace from the token; no unauthenticated access off loopback |
| Q1225 | Redis Cluster emulation (Tidis's fake `CLUSTER SLOTS`)? | **Not in v1**: one logical node; revisit for cluster-only clients |
| Q1226 | Length-counter shards per collection? | **16** (Tidis uses 100); configurable per keyspace |
| Q1227 | Large Redis strings? | Inline up to the value cap; **1 MiB chunks** above it; a **64 MiB** maximum in v1 (Redis allows 512 MB) |
| Q1228 | On the light default, do the modules run in the gateway process or in the engine? | **The gateway process**, with its own redb file, under a `kv` profile |
| Q1229 | The change feed for cross-gateway invalidation: run `tikv/migration`'s TiKV-CDC unmodified (into its Kafka sink), port its stream consumer, write our own from scratch, or an in-transaction journal? | **Port TiKV-CDC's stream consumer** (`cdc/cdc/kv`) to Rust as `loams-tikv::cdc`, on the fork's generated protos, parameterised by `kv_api` (`TxnKV` by default, `RawKV` for fast-mode keyspaces), attributed. Not running TiKV-CDC: it hard-codes RawKV, so it misses the default transactional keyspaces, and its consumable sink is Kafka, which Loams does not run. The journal is the fallback |
| Q1230 | Crate names, since `loams-kv` and `loams-hot` are taken: what is the KV core extension called, and what is the KV cache called? | **Core:** no new crate; extend `loams-kv` in place (the alternative is a gateway-only crate such as `loams-kvcore`). **Cache:** a new crate **`loams-kv-hot`** layered on `loams-cache` (its foyer builder made public), keeping the owner's word "hot" for a tier that also gets write paths (the alternatives are `loams-kvcache` or a `kv` module inside `loams-cache`) |
| Q1231 | `memtier_benchmark` (GPL-2.0) as a CI-run tool? | **Run only**, as an unmodified separate binary in benchmark jobs; never vendored, linked or copied; `valkey-benchmark` and go-ycsb are the defaults |
| Q1232 | CQL authentication? | SASL `PLAIN` with namespace and Loams token, through the same `Authorizer`; `AllowAllAuthenticator` only on loopback |
| Q1233 | The track's name? | **KV** (plan KV1, milestones KV0–KV5) |
| Q1234 | How many Redis databases (`SELECT`)? | **16**, as Redis; configurable up to 256 |
| Q1235 | The interactive transaction's lifetime cap (WATCH to EXEC)? | **10 s**, then EXEC returns nil |
| Q1236 | Raw or transactional keys per Redis keyspace, given that RawKV and transactional keys are mutually invisible in a keyspace? | **Transactional by default** for every keyspace (correct MULTI/EXEC; 1PC and async commit keep single-key commands cheap). A RawKV **fast mode** only as an opt-in for keyspaces that never use multi-key commands (it refuses MULTI/EXEC, WATCH, RENAME, `*STORE`), TiKV only, built only if KV2's `valkey-benchmark` and go-ycsb numbers show a gain worth a second code path |
| Q1237 | Pessimistic reads: `loams-kv` refuses pessimistic transactions today. Add them, and when? | **Yes, in KV0**, before LWT and WATCH: D851's `get_for_update` and pessimistic mode on both backends, with conformance cases. LWT uses it; WATCH stays optimistic for Redis semantics; hot-key `INCR` may use it if KV2 measures retry storms |
| Q1238 | CQL framing: reuse `cassandra-protocol`, use `scylla-cql`, or build? | **Reuse `cassandra-protocol`** (MIT/Apache-2.0; server-direction decode and encode, v4 and v5 segments) behind `loams-cql`'s own trait; build the grammar by hand from Cassandra's ANTLR grammar; `scylla-cql`/`scylla` on the test-client side |
| Q1239 | The dev playground (`deploy/tikv/compose.yaml`) defaults `TIKV_TAG` to v8.5.0, while release and CI run v8.5.8. Bump it? | **Yes**, to v8.5.8, in KV0 Task 0, so every TiKV a developer or CI runs is the same version |

## 17. Sources

- **Loams** at `origin/dev` `bbb21ff0e` (2026-10-21): `crates/loams-kv` (`lib.rs`, `store.rs`, `txn.rs`, `runner.rs`, `conformance.rs`, `embedded/mod.rs`), `crates/loams-tuple/src/lib.rs`, `crates/loams-tikv` (`lib.rs`, `tests/codec.rs`), `crates/loams-hot`, `crates/loams-cache`, `crates/loams-gateway`, `crates/loams-meta-tikv` (`keys.rs`, `changes.rs`), `crates/loams-safekeeper/src/tikv_raw.rs`, `deploy/release/config/tikv.toml`, `deploy/release/compose.yaml`, `deny.toml`, `Cargo.toml` (the `tikv-client` pin); designs §00, §04, §11, §18, §20, §26, §28, §45, §57, §58, §63, §68, §69, §70, §74, §76; [r1-dependency-spike](../plans/r1-dependency-spike.md) §(i); PR #973 (open).
- **Client fork** `ostrium-labs/client-rust` `41b3588`: `src/raw/client.rs` (`with_atomic_for_cas`, `compare_and_swap`, `put_with_ttl`), `src/transaction/transaction/scanner.rs`, `src/transaction/snapshot.rs`.
- **`tikv/migration`** `main` (GitHub API, 2026-10-21): `cdc/cdc/sink/sink.go` (sink schemes), `cdc/cdc/kv/client.go` (`KvApi`, line 716), `cdc/cdc/kv/`, release `cdc-v1.3.0-beta` notes. The PingCAP ecosystem survey, PR #1000 (`docs/research/pingcap-tikv-ecosystem.md`, merged), row 7.
- **TiKV** `v8.5.0`: `src/storage/mod.rs` (`check_api_version`, lines 469–526), `src/storage/config.rs` (`api_version`, `enable_ttl`). **TiKV-CDC:** `tikv/migration` `cdc/README.md`; tikv.org "RawKV CDC" (concepts/explore-tikv-features/cdc).
- **Tidis** `tidb-incubator/tidis` `5558562` (local read-only clone): `NOTICE`, `LICENSE`, `Cargo.toml`, `Cargo.lock`, `src/frame.rs`, `src/parse.rs`, `src/connection.rs`, `src/server.rs`, `src/cmd/mod.rs`, `src/cmd/multi.rs`, `src/cmd/eval.rs`, `src/tikv/client.rs`, `src/tikv/mod.rs`, `src/tikv/encoding/encode.rs`, `src/tikv/{string,hash,list,set,zset,lua}.rs`, `src/gc.rs`, `src/cluster.rs`, `src/config.rs`, `test/`, `Dockerfile`, `.github/workflows/ci.yml`. `yongman/client-rust` compared with `tikv/client-rust` master through the GitHub API (24 ahead, 119 behind, base `027a7dff`).
- **Titan** `distributedio/titan` `97868f5` (local read-only clone): `LICENSE`, `go.mod`, `db/db.go`, `db/object.go`, `db/codec.go`, `db/string.go`, `db/hash.go`, `db/list.go`, `db/llist.go`, `db/zlist.go`, `db/set.go`, `db/zset.go`, `db/expire.go`, `db/gc.go`, `db/tikvgc.go`, `db/task.go`, `db/ztransfer.go`, `db/kv.go`, `db/store/store.go`, `command/init.go`, `command/command.go`, `command/transactions.go`, `command/common.go`, `encoding/resp/resp.go`, `conf/config.go`, `docs/`, `proposals/hash-design.md`, `proposals/set-design-en.md`, `proposals/zset.md`, `Dockerfile`, `docker-compose.yml`.
- **Licences** (GitHub API and files, 2026-10-21): `redis/redis` `LICENSE.txt`, `REDISCONTRIBUTIONS.txt`; `valkey-io/valkey` `COPYING`, `tests/`, release 9.1.2; `scylladb/scylladb` `LICENSE-ScyllaDB-Source-Available.md`; `apache/cassandra` (`tools/stress`, tags), `apache/cassandra-dtest`, `apache/cassandra-java-driver`, `apache/cassandra-python-driver`, `apache/cassandra-gocql-driver`, `apache/cassandra-cpp-driver`; `brianfrankcooper/YCSB`; `scylladb/scylla-rust-driver`; `scylladb/cassandra-stress`; `distributedio/titan`; `zhihu/zetta`; `tidb-incubator/tidis`; `tokio-rs/mini-redis`; `tikv/migration`; `redis/memtier_benchmark`; `redis/redis-py`; `valkey-io/valkey-py`.
- **The research report:** [`docs/plans/kv-gw-research-report.md`](../plans/kv-gw-research-report.md) (#996): Valkey and Redis suites (§1), Cassandra drivers and stress (§2), benchmarks (§3), TiKV facts (§4), existing crates (§5), Rust building blocks and Murmur3 (§6), Tidis (§7), Titan, `distributedio/titan` (§8), the take-from table (§9).
