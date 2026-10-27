# KV1 — KV Gateways: Redis (RESP) and Cassandra (CQL) on `loams-kv` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, **test first**: write the named tests, run them and watch them fail, then implement until they pass. Where this plan gives exact names, paths, flags or tests, use them verbatim. Record any deviation in "Rulings made during execution" at the end of this file.
>
> **Status: Approved** (owner, 2026-10-21, all defaults; stub). Track **KV**, design [§77](../design/77-kv-gateways.md) (D1640–D1665, Q1220–Q1239). The owner gave the House engine ([HE1](2026-10-21-he1-loams-house-engine.md)) priority on 2026-10-21; KV0 started on 2026-10-23, after HE1 (Ruling T0-1). **KV0 progress (2026-10-24):** Tasks 0 to 10 merged (#1125, #1151, #1153, #1159, #1160, #1167, #1177, #1178, #1179, #1161, #1139). Task 11, the [KV0 report](kv0-report.md), 2026-10-26: KV0 is complete. **KV2 (the Redis gateway) is specified task by task below** (2026-10-26).
>
> **Coverage:** this plan specifies **KV0** (the owner's Phase 0, the `loams-kv` foundation) task by task. KV2 is specified task by task (after the KV0 report); KV1 and KV3–KV5 are outlined at milestone level.
>
> **Inputs:** the [research report](kv-gw-research-report.md) (#996) for suites, flags, subsets, benchmarks and the reference projects; the owner's draft of 2026-10-21.

**Goal (KV0):** make `loams-kv` the core both gateways need, without writing either gateway: an untagged memcomparable codec, one-shot operations, a paged scanner, an in-memory store, pessimistic mode, tenancy and metrics, all held to one conformance suite on the embedded, in-memory and TiKV backends; and find out which differential oracles actually run.

Exit, from §77 §14:

- the codec's property tests pass;
- the embedded, in-memory and TiKV stores pass the same conformance suite, the new cases included;
- the KV0 report (`docs/plans/kv0-report.md`) with the oracle findings and the answers to §77's **(verify)** items.

**Architecture** (§77 §5, §6):

- `crates/loams-tuple`: a new module `key` beside `tuple`.
- `crates/loams-kv`: new modules `ops` (one-shot operations), `scan` (the paged scanner), `tenant`, `metrics`; pessimistic mode in `runner`, `txn`, `tikv` and `embedded`; `EmbeddedConfig::in_memory`.
- `crates/loams-gw-test`: a new crate, `publish = false`, the harness skeleton.
- No gateway code, no `loams-resp`, `loams-cql` or `loams-kv-hot` in KV0.

**Tech stack:** Rust 1.97, edition 2024, workspace lints; `redb` 4 (its `InMemoryBackend`), `tikv-client` from `ostrium-labs/client-rust` at the workspace pin (`41b3588`), `proptest`, `tokio`. No new dependency enters `Cargo.lock` in KV0 except what `loams-gw-test` needs for its skeleton (none beyond the workspace).

**Spec:**

- [§77](../design/77-kv-gateways.md), §5–§8, §12–§14.
- [§45](../design/45-loams-live-production.md) §8 and the [LV1 plan](2026-10-08-lv1-live-production.md) Tasks 20–21 (the seam as built, rulings T21-1 to T21-15).
- [JB1](2026-10-10-jb1-jobs-and-usage.md) and D851 (pessimistic mode, approved).
- [§20](../design/20-reactive-database-on-tikv.md) §9 and D122 (keyspaces).

## Global Constraints

- **Worktrees and branches.** Work in `~/Documents/Ostriumlabs/loams-wt/kv0-<task>`, one branch and one PR per task into `dev`. CodeRabbit reviews; merge on green. Commit with `git commit -s`. Never stash.
- **Light Rust builds.** Build and test only the crate in hand (`cargo test -p loams-kv`), never `--workspace` locally. Build output goes to the shared target set in `~/Documents/.cargo/config.toml`. Never set `CARGO_TARGET_DIR`, and never build in `/tmp`.
- **TiKV tests** run where `LOAMS_TEST_PD` names a cluster (CI's TiKV job, v8.5.8 on API v2); locally they skip with the factory's `skipped:` line.
- **Compatibility of the seam.** Live is `loams-kv`'s user. No existing method changes signature or behaviour; the existing 15 conformance cases keep passing unchanged, except `pessimistic_mode_is_refused`, which Task 5 replaces.
- **Licences (§77 §12, D1649).** No code from Redis 7.4 or later, and none from `scylladb/scylladb`. Ported code keeps its origin's notice. Every new crate passes `cargo deny`.
- **No clones.** Reference projects are read from the owner's read-only clones (`~/Documents/Ostriumlabs/tidis`, `~/Documents/Ostriumlabs/titan`); test suites are fetched by CI at pinned tags, never copied into the repository.
- **GitHub-hosted runners only.**

## Review Focus

1. **The new codec reorders or aliases keys.** Expected: never. Tests: `key_order_matches_value_order` and `key_no_prefix_aliasing` (Task 1).
2. **A one-shot operation is not atomic.** Expected: never. Test: `compare_and_put_is_linearizable` (Task 2), checked with `loams-sim`'s linearizability checker.
3. **Pessimistic mode changes optimistic behaviour.** Expected: never. Tests: the existing conformance cases, unchanged (Task 5).
4. **A tenant reads another tenant's keys.** Expected: never. Test: `tenants_are_isolated` (Task 6).
5. **A metric carries a tenant.** Expected: never (D1021). Test: `metrics_have_no_tenant_label` (Task 7).

## KV0 — The `loams-kv` foundation (Tasks 0–11)

### Task 0 — Rulings and pins

Check each item and record the answers in "Rulings made during execution":

- **State of the dependencies.** Whether PR #973 (the light default, D1340–D1343, its single-node support row in particular) is merged; whether JB1 has started D851's pessimistic mode; whether GW1 has added a module seam to `loams-gateway`.
- **Pins.** Valkey `9.1.2`'s tag commit and the `valkey/valkey:9.1.2` image digest; the `cassandra:5.0.9` image digest; the Titan (`distributedio/titan` `97868f5`) and Tidis (`tidb-incubator/tidis` `5558562`) revisions.
- **§77's (verify) items:**
  1. How the pinned `tikv-client` chooses a transaction's primary key (first key written, smallest key, or settable), for D1653's "the meta key is the primary".
  2. Whether the fork's `Transaction::scanner` pages in reverse.
  3. Whether redb 4's `InMemoryBackend` supports everything the embedded backend's committer and GC threads do (file-free registry key, no `fsync`).
- **The dev playground (Q1239).** Change `deploy/tikv/compose.yaml`'s `TIKV_TAG` default from `v8.5.0` to `v8.5.8`, matching `deploy/release/compose.yaml` and CI.

Files: this plan; `deploy/tikv/compose.yaml`. Tests: `docker compose -f deploy/tikv/compose.yaml config -q`. Commit: `deploy(tikv): the dev playground runs TiKV v8.5.8, like release and CI (KV0 Task 0)`.

### Task 1 — `loams_tuple::key`

**What to write:** `crates/loams-tuple/src/key.rs`, re-exported as `loams_tuple::key` (and `loams_kv::key`):

- `put_u16`, `put_u32`, `put_u64` (big-endian), `put_i64` (sign bit flipped), `put_f64` (the `tuple` float transform, canonical NaN, `-0.0 == 0.0`), `put_bytes` (the `tuple` escape: `0x00` → `0x00 0xFF`, terminator `0x00 0x00`);
- matching `get_*` decoders returning `(value, consumed)` and `CodecError`;
- `desc(encoded: &mut [u8])`, bitwise inversion of one encoded component, and `get_*_desc`;
- `prefix_end(prefix) -> Vec<u8>` (the `tuple::successor` rule).

`tuple`'s encoding does not change.

**Tests** (`crates/loams-tuple/tests/key.rs`, proptest, 4,096 cases each; add `proptest` as a dev-dependency):

- `key_order_matches_value_order`: for each primitive, `a.cmp(b) == enc(a).cmp(enc(b))`, and the reverse for `desc`;
- `key_composites_order_lexicographically`: tuples of two and three components mixing primitives and `desc`;
- `key_roundtrip`: `decode(encode(x)) == x` for every primitive;
- `key_no_prefix_aliasing`: no `put_bytes` encoding is a proper prefix of another;
- `key_agrees_with_tuple`: for `i64`, `f64` and bytes, `key` order equals `tuple` order.

Commit: `tuple: an untagged memcomparable key module (KV0 Task 1)`.

### Task 2 — One-shot operations

**What to write:** `crates/loams-kv/src/ops.rs`:

```rust
impl Store {
    pub async fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, TxnError>;
    pub async fn put(&self, key: &[u8], value: impl Into<Vec<u8>>) -> Result<Ts, TxnError>;
    pub async fn insert(&self, key: &[u8], value: impl Into<Vec<u8>>) -> Result<Ts, TxnError>;
    pub async fn delete(&self, key: &[u8]) -> Result<Ts, TxnError>;
    /// Writes `new` (or deletes, for `None`) when the current value equals `expected`
    /// (`None`: absent). `Ok(Ok(ts))` on success, `Ok(Err(current))` on a mismatch.
    pub async fn compare_and_put(
        &self, key: &[u8], expected: Option<&[u8]>, new: Option<&[u8]>,
    ) -> Result<Result<Ts, Option<Vec<u8>>>, TxnError>;
}
```

Each runs one optimistic `Store::run` with the store's commit mode (`Async1pc` by default), `op` names `kv.get`, `kv.put`, `kv.insert`, `kv.delete`, `kv.cas`. `get` reads in a read-only transaction.

**Tests:** new cases in `kv_conformance!`: `one_shot_put_get_delete`, `one_shot_insert_existing_fails`, `compare_and_put_matches_and_mismatches`, `compare_and_put_absent_and_delete`; and `compare_and_put_is_linearizable` in `crates/loams-kv/tests/linearizable.rs` (8 tasks × 200 operations on 4 keys, checked as CAS registers with `loams-sim`'s checker; embedded always, TiKV when available).

Commit: `kv: one-shot get, put, insert, delete and compare_and_put (KV0 Task 2)`.

### Task 3 — The paged scanner

**What to write:** `crates/loams-kv/src/scan.rs`: `Txn::scanner(start, end, page) -> Scanner` and `Snap::scanner(…)`, forward and reverse, yielding `Pair`s page by page with at most one page buffered. TiKV: the fork's `Transaction::scanner` / `Snapshot::scanner` (upstream #554); if Task 0 finds it has no reverse mode, reverse pages use `scan_reverse` with a moving bound. Embedded: a redb range iterator over the MVCC versions, resolving each key at the read timestamp.

**Tests:** conformance cases `scanner_pages_forward`, `scanner_pages_reverse`, `scanner_sees_own_writes` (inside a `Txn`), `scanner_bounds` (empty, one key, page boundary on a key); and a 100,000-key case `scanner_memory_is_bounded` on embedded only (asserts the page buffer, not RSS).

Commit: `kv: a paged scanner on both backends (KV0 Task 3)`.

### Task 4 — The in-memory embedded store

**What to write:** `EmbeddedConfig::in_memory(name: &str)`, opening redb with `InMemoryBackend`; the registry keys it by `mem:<name>` instead of a canonical path; GC and committer threads unchanged. `testing::memory_factory()` for `kv_conformance!`.

**Tests:** `kv_conformance!(memory, loams_kv::testing::memory_factory())` in `crates/loams-kv/tests/conformance.rs`, running every case; `in_memory_stores_are_separate_by_name`; `in_memory_store_drops_with_last_handle`.

Commit: `kv: an in-memory embedded store on redb's InMemoryBackend (KV0 Task 4)`.

### Task 5 — Pessimistic mode (D851)

If JB1 has built it (Task 0), this task only adds the conformance cases below. Otherwise:

**What to write:** `Txn::get_for_update(key)` and `Txn::batch_get_for_update(keys)`; `Store::run` accepts `Mode::Pessimistic`. TiKV: `TransactionOptions::new_pessimistic()` with the runner's retries and lock-wait mapped to `Conflict` after the deadline. Embedded: a per-keyspace lock table taken at `get_for_update`, `put`, `insert`, `delete` and `lock_keys`, waiting up to `TxnOptions::deadline` (a lock wait and a deadlock both map to `Conflict`), and reads that return the latest committed value at lock time, so a read-modify-write cannot lose an update (the gap row T21-15 named). Remove `PESSIMISTIC_REFUSED` and its refusal.

**Tests:** replace `pessimistic_mode_is_refused` with `pessimistic_read_modify_write_loses_no_update` (16 tasks incrementing one counter 100 times each; the result is exactly 1,600), `pessimistic_second_holder_waits`, `pessimistic_lock_wait_times_out_as_conflict`, `pessimistic_and_optimistic_conflict`. All on embedded, in-memory and TiKV.

Commit: `kv: pessimistic mode with get_for_update on both backends (D851; KV0 Task 5)`.

### Task 6 — Tenancy

**What to write:** `crates/loams-kv/src/tenant.rs`:

- `enum Placement { Dedicated { keyspace: String }, Shared { keyspace: String, prefix: Vec<u8> } }` (D122's classes);
- `fn store_config(base: &StoreConfig, placement: &Placement) -> StoreConfig` (keyspace, and the root for a shared keyspace);
- `pub const PRODUCT_REDIS: u8 = b'R'; pub const PRODUCT_CQL: u8 = b'C';` and `fn product_root(config, product) -> StoreConfig` (D1645).

The directory that maps a namespace to its placement is not in KV0; tests pass placements directly.

**Tests:** `tenants_are_isolated` (two shared-keyspace tenants and one dedicated, each writing the same suffixes; scans see only their own); `product_roots_are_disjoint`.

Commit: `kv: tenant placements and product roots (KV0 Task 6)`.

### Task 7 — Op metrics and the observer

**What to write:** `crates/loams-kv/src/metrics.rs`: counters `loams_kv_ops_total{backend,op,outcome}` and histogram `loams_kv_op_seconds{backend,op}`, recorded by `Store::run` and the one-shot operations through the workspace's metrics facade; `trait KvObserver: Send + Sync { fn on_op(&self, keyspace: &str, op: &'static str, outcome: Outcome, elapsed: Duration); }` with a no-op default, set with `Store::with_observer`. `outcome` is `ok`, `conflict`, `not_applied`, `undetermined`, `already_exists`, `fatal` or `deadline`.

**Tests:** `metrics_count_each_outcome`; `metrics_have_no_tenant_label` (renders the registry and asserts no label named `keyspace`, `namespace`, `tenant` or `root`); `observer_sees_keyspace`.

Commit: `kv: op metrics without tenant labels, and a KvObserver hook (KV0 Task 7)`.

### Task 8 — `loams-gw-test`, the skeleton

**What to write:** `crates/loams-gw-test` (`publish = false`, Apache-2.0): a `Backend` enum (`Embedded`, `Memory`, `Tikv`) yielding `loams_kv::Store`s from `loams_kv::testing`; a `Scenario` trait (`async fn run(&self, store: &Store) -> Report`); a runner that runs one scenario on every available backend and compares the reports; a binary `loams-gw-test` with `list` and `run <scenario>`. Placeholders (documented, not empty) for the later runners: `valkey_tcl`, `tidis_py`, `titan_go`, `cql_drivers`, `differential`.

**Tests:** `same_scenario_same_report_on_every_backend` with a small CAS-and-scan scenario.

Commit: `gw-test: the KV gateways' harness skeleton (KV0 Task 8)`.

### Task 9 — Attribution groundwork

**What to write:**

- `NOTICE`: entries for Tidis (`tidb-incubator/tidis`, Apache-2.0, with its NOTICE content carried as Tidis's own section), tokio's mini-redis (MIT, its copyright line and licence text, for the Tidis files derived from it, §77 §8.1), Titan (`distributedio/titan`, Apache-2.0) and `tikv/migration` (Apache-2.0, for KV1's ported CDC consumer).
- `docs/licences/ported-code.md`: the header template for ported files (origin repository, revision, path, licence, "modified by Ostrium Labs for Loams"), and the rule that each ported file names its source files.
- `scripts/licences/check-ported-headers.sh`: fails when a file under `crates/loams-resp` or `crates/loams-cql` contains the marker `ported-from:` without the full header, and when a ported file lacks the marker but matches a listed origin path in `docs/licences/ported-code.md`. Wired into CI's `deny` job (`.github/workflows/ci.yml`); `--self-test` with planted good and bad files.

**Tests:** `scripts/licences/check-ported-headers.sh --self-test`.

Commit: `licences: NOTICE entries and a header check for code ported from Tidis, mini-redis, distributedio/titan and tikv/migration (KV0 Task 9)`.

### Task 10 — Oracle feasibility (spike; scripts merged, results in the report)

**What to do, in CI (a manual-dispatch workflow `kv-oracles.yml`):**

1. **Valkey suite against Valkey.** Fetch Valkey `9.1.2`'s `tests/` at its tag (unmodified, at run time), start `valkey/valkey:9.1.2`, and run the report §1.4 command line with the first subset. Record pass, fail and skip counts; this is the control the gateway must later match.
2. **Tidis's Python suite against Valkey.** Run `test/test_helper.py --ip --port` from Tidis at `5558562` against the same Valkey, `test_lua` excluded. Record which cases fail on real Valkey (Tidis's own deviations).
3. **Titan on `mocktikv://`.** Build `distributedio/titan` at `97868f5` unmodified (Go toolchain of its `go.mod` era, `GOFLAGS=-mod=mod`, the Go proxy for its 2020 versions) and run it with `pd-addrs = "mocktikv://"`. Record whether it builds and serves, and its image digest if it does.
4. **The auto tests of Titan (`distributedio/titan`) against Valkey**, through a harness-side patch in `loams-gw-test` (not in Titan) that skips `integration.Start()` and passes the address.

Tidis itself is not built or run (§77 §12.3).

Files: `.github/workflows/kv-oracles.yml`, `scripts/kv/oracles/*.sh`. Tests: the workflow's own steps. Commit: `ci: the KV oracle feasibility runs (KV0 Task 10)`.

### Task 11 — The KV0 report

**What to write:** `docs/plans/kv0-report.md`: the answers to Task 0's items; Task 10's results; the conformance counts per backend; anything that changes §77, as proposed amendments for the owner.

Commit: `docs: the KV0 report (KV0 Task 11)`.

## KV1–KV5 outline

| Milestone | Content | Exit | Depends on |
|---|---|---|---|
| **KV1** `loams-kv-hot` v1 | `loams-cache` exposes its foyer builder; `loams-kv-hot`: read-through cache, versioned values, TTL-bounded staleness, local write-through, the durability-mode switch (`durable`, `write-through`; `hot-only` behind a flag); `loams-tikv::cdc`, the `ChangeData/EventFeed` consumer ported from TiKV-CDC's `cdc/cdc/kv` in `tikv/migration` (attributed; `kv_api` a parameter, `TxnKV` by default), or the journal fallback (Q1229) | Hit-rate and staleness tests; invalidation latency on three gateways; the consumer survives splits, leader transfers and a TiKV restart without a missed change | KV0 |
| **KV2** `loams-resp` MVP | The `loams-gateway` module seam (`[resp]`, `[cql]` config sections); the Tidis port onto `loams-kv` (frames, parser, command handlers, encoding, GC) with headers; RESP3/HELLO; strings, counters, hashes, sets, zsets, lists, TTL with the bucketed expiry worker, the GC worker, leases; MULTI/EXEC/WATCH with the interactive transaction and a primary-key hint; AUTH and TLS; the compose `kv` profile on the embedded backend (D1647) | The Valkey 9.1.2 subset passes except the checked-in skip list; Tidis's and `distributedio/titan`'s corpora triaged; the differential run against Valkey (and Titan if KV0 found it runs) clean except documented deviations; `valkey-benchmark` per durability mode on both backends; the raw fast mode decided (Q1236) | KV0; KV1 for `write-through` |
| **KV3a** `loams-cql` core | Frames on `cassandra-protocol` behind a trait (Q1238), handshake, SASL `PLAIN`, `system.local`/`peers`/`peers_v2`, a `system_schema` subset, DDL, single-partition reads and writes, prepared statements, paging, `cassandra_token` with Cassandra 5.0.9 vectors | The Python driver's, gocql's and the Rust driver's selected subsets (report §2.2), each first run against Cassandra 5.0.9 | KV0, the module seam |
| **KV3b** `loams-cql` rest | LWT (pessimistic), batches, TTL, secondary indexes, materialized views, collections (Q1221 measured first) | The full driver subset; `cassandra-stress` write, read and mixed recorded | KV3a |
| **KV4** Hardening | `loams-sim` runs with injected failures on memory, embedded and TiKV; linearizability for counters, MULTI/EXEC and LWT; chaos on leader transfers and region splits; fuzzing of the RESP and CQL pre-auth parsers; export/import between backends | No linearizability violation; a week of clean nightly fuzzing | KV2, KV3b |
| **KV5** The hot-engine gate | go-ycsb A, B, C and F through `redis` (against `loams-resp`), `cassandra` (against `loams-cql`) and `tikv` (bare TiKV) per durability mode on tuned TiKV | A written decision on `loams-kv-hot` v2 (D1663) | KV2 |

## KV2 — `loams-resp`, the Redis gateway MVP (Tasks 0–17)

Expanded to task level on 2026-10-26, after the KV0 report ([`kv0-report.md`](kv0-report.md)). The coordinator's dispatch of 2026-10-26 called this milestone "KV1"; in this plan KV1 is `loams-kv-hot` and the CDC consumer, and the Redis gateway is **KV2** (Ruling K2-1).

**Goal (KV2):** a RESP2/RESP3 gateway, `loams-resp`, run as the `resp` module of `loams-gateway`, serving strings, counters, hashes, sets, sorted sets, lists, TTLs and MULTI/EXEC/WATCH on `loams-kv`, on the embedded (light default) and TiKV backends, held to Valkey 9.1.2's test suite.

Exit, from §77 §14:

- the Valkey 9.1.2 subset of report §1.4 passes against the gateway, except a checked-in skip list with one reason per entry;
- Tidis's and `distributedio/titan`'s corpora triaged (pass, deviation, or out of scope, per case);
- the differential run against Valkey clean except documented deviations;
- `valkey-benchmark` per durability mode on both backends recorded; the raw fast mode decided (Q1236).

**Architecture** (§77 §5, §8):

- `crates/loams-resp` (new, Apache-2.0, `publish = false` until the gate passes): `frame` (RESP2/RESP3 values, the streaming decoder, the encoder), `conn` (one client connection: read, dispatch, reply, pipelining), `server` (the listener, limits, shutdown), `cmd/` (the command table and handlers, one file per family), `layout` (D1653's keys and the meta record, D1654's versions), `types/` (strings, hashes, sets, zsets, lists on `loams_kv::Txn`), `expire` (the bucketed expiry index and its worker, D1655), `gc` (the GC queue and its worker, D1656), `lease` (the worker lease row), `config`.
- `crates/loams-kv`: an interactive transaction, `Store::begin(opts) -> OpenTxn` (D1657).
- `crates/loams-gateway`: a `resp` cargo feature and `--module resp`, the same seam as the `temporal` module (Ruling K2-2).
- Every file ported from Tidis or `distributedio/titan` carries the header of `docs/licences/ported-code.md` and a row in its register; Tidis files that follow mini-redis carry both blocks (§77 §8.1).

**Tech stack:** as KV0; plus `bytes` (workspace) for frames; `tokio-rustls` (already in the gateway) for TLS. No Lua, no `mlua`.

**Global constraints:** KV0's apply. In addition:

- **Worktrees** are `~/Documents/Ostriumlabs/loams-wt/kv2-<task>`; branches `kv2-<task>`; one PR per task into `dev`, stacked on the previous task's branch while it is open.
- **Valkey's suite is run, never copied.** `scripts/kv/oracles/fetch.sh` fetches `tests/` at the pinned commit at run time (T0-6). No Redis 7.4+ source is read or ported (D1649).
- **Gates per task.** Each data-type task names the Valkey unit file it must pass (with the skip list it adds to). Locally: `scripts/kv/resp-suite.sh <unit>` (Task 2) starts the gateway on the in-memory store and runs that unit; in CI the `kv-resp` job (Task 14) runs the whole subset.
- **Both backends.** Every handler test runs through `loams_kv::testing`'s factories, so it runs on embedded and in-memory always, and on TiKV where `LOAMS_TEST_PD` is set.

### Task 0 — Rulings

Recorded in "Rulings made during execution": K2-1 (the milestone's name), K2-2 (the module seam), K2-3 (durability modes before `loams-kv-hot`), K2-4 (the primary key, from T0-7), K2-5 (no `redis-protocol`). No code.

### Task 1 — RESP frames

**What to write:** `crates/loams-resp` with `src/frame.rs`, ported from Tidis `src/frame.rs` and `src/parse.rs` (and so from mini-redis; both header blocks):

- `enum Value` for replies: RESP2 (`Simple`, `Error`, `Integer`, `Bulk`, `Null`, `Array`) and RESP3 (`Map`, `Set`, `Double`, `Boolean`, `BigNumber`, `Verbatim`, `Push`, `Null`); `fn encode(&self, proto: Proto, out: &mut BytesMut)` that downgrades RESP3 types for RESP2 clients (maps to flat arrays, sets to arrays, doubles to bulk strings, booleans to integers, the null to `$-1`/`*-1` by context).
- `struct Decoder { max_bulk: usize, max_multibulk: usize, max_inline: usize }` with `fn decode(&mut self, buf: &mut BytesMut) -> Result<Option<Vec<Bytes>>, ProtoError>`: one request (an array of bulk strings, or an inline line split on spaces with quotes, as `redis-cli` and the TCL suite send), zero-copy into `buf`, limits checked **before** any allocation, Valkey's error texts (`Protocol error: invalid multibulk length`, `… invalid bulk length`, `… too big inline request`, `… unbalanced quotes in request`).

**Tests** (`crates/loams-resp/tests/frame.rs`): `request_roundtrip` (proptest, 4,096 cases: arbitrary binary arguments encoded as a multibulk request, split at arbitrary points, decode to the same arguments), `reply_roundtrip` (proptest: every `Value` encodes and decodes back, in RESP3; RESP2 downgrades as specified), `encode_resp2_and_resp3` (fixed vectors per type), `decoder_partial_input_is_incomplete` (every split point of a pipeline), `decoder_limits_before_allocation` (a `*` or `$` header over the limit fails with no buffer growth), `inline_commands` (quotes, escapes, empty lines), `pipelined_requests_decode_in_order`.

Commit: `resp: RESP2 and RESP3 frames, a bounded streaming decoder (KV2 Task 1)`.

### Task 2 — The server, the command table, connection commands, the gateway module

**What to write:**

- `config.rs`: `RespConfig { listen, store: StoreSection, requirepass: Option<String> (or a file), databases: u8 (16, Q1234), max_clients, proto_max_bulk_len (512 MiB), timeout, tls: Option<…>, durability: Durability }`, `deny_unknown_fields`, validated (a public listener needs `requirepass` or TLS; D1648's "never public" holds by the deployment, not this check).
- `server.rs`, `conn.rs`: accept, one task per connection, read → decode → dispatch → encode, replies flushed per read batch (pipelining), `QUIT`, idle timeout, graceful shutdown; per-connection state (`proto`, `db`, `name`, `id`, `authenticated`, the MULTI queue later).
- `cmd/mod.rs`: the command table (name, arity, flags `write`/`readonly`/`fast`/`noauth`/`noscript`, first/last/step key positions) and dispatch with Valkey's arity and unknown-command errors; `COMMAND` (`COUNT`, `INFO`, `DOCS` minimal, `GETKEYS`), out-of-scope commands answering `ERR unknown command` or `ERR <cmd> is not supported by Loams` (§77 §8.5).
- `cmd/conn.rs`: `PING`, `ECHO`, `HELLO` (2 and 3, `AUTH`, `SETNAME`), `AUTH`, `SELECT`, `RESET`, `QUIT`, `CLIENT` (`SETNAME`, `GETNAME`, `ID`, `INFO`, `LIST`, `KILL ID`, `SETINFO`, `NO-EVICT`, `NO-TOUCH`; `TRACKING` refused), `TIME`, `INFO` (`server`, `clients`, `keyspace`, `stats` probe sections), `CONFIG GET/SET/RESETSTAT` tolerant (unknown parameters echoed back, as report §1.2 requires for the suite's overrides), `DEBUG` (`SET-ACTIVE-EXPIRE`, `QUICKLIST-PACKED-THRESHOLD`, `SLEEP` answered; the rest refused), `FUNCTION FLUSH` (`+OK`, no functions).
- `crates/loams-gateway`: feature `resp`, `src/resp.rs` with `load` and `serve`, `--module resp` in `main.rs`; `deploy/release/resp-gateway.toml.example`.
- `scripts/kv/resp-suite.sh <unit…>`: builds the gateway with `resp`, starts it on an in-memory store, fetches Valkey's tests (T0-6) and runs the named units with report §1.4's flags plus `--skipfile crates/loams-resp/tests/valkey-skip.txt`.

**Tests:** `crates/loams-resp/tests/server.rs` over TCP with the crate's own encoder: `ping_echo_and_unknown_command`, `arity_errors_match_valkey`, `hello_switches_protocol`, `auth_required_when_configured`, `select_bounds`, `pipelined_replies_in_order`, `client_name_and_id`, `config_get_set_echoes_unknown`; `crates/loams-gateway`'s `module_config_rejects_public_listener_without_auth`. Gate: `unit/info-command` passes except its skip entries.

Commit: `resp: the server, the command table and the connection commands; loams-gateway --module resp (KV2 Task 2)`.

### Task 3 — The key layout, the meta record, strings and the generic key commands

**What to write:**

- `layout.rs` (D1653, D1654): keys of §77 §8.4 built with `loams_tuple::key` under `product_root(…, PRODUCT_REDIS)`; the meta record `[format][type][ver u64][expire_at_ms u64][trailer]` with inline string bytes; `ver` = the creating transaction's start timestamp; the type check every write does (`WRONGTYPE Operation against a key holding the wrong kind of value`); lazy expiry on read (an expired meta reads as absent; a write over it replaces it).
- Strings (`types/string.rs`, `cmd/string.rs`; Tidis `src/tikv/string.rs`, `src/cmd/{get,set,setnx,setex,mget,mset,incrdecr…}.rs` as the porting base): `GET`, `SET` with every option (`NX`, `XX`, `GET`, `EX`, `PX`, `EXAT`, `PXAT`, `KEEPTTL`), `SETNX`, `SETEX`, `PSETEX`, `GETSET`, `GETDEL`, `GETEX`, `MGET`, `MSET`, `MSETNX`, `APPEND`, `STRLEN`, `GETRANGE`/`SUBSTR`, `SETRANGE`, `LCS` refused; values over the inline limit in 1 MiB chunks (Q1227).
- Generic: `DEL`, `UNLINK`, `EXISTS`, `TYPE`, `EXPIRE`/`PEXPIRE`/`EXPIREAT`/`PEXPIREAT` (with `NX`/`XX`/`GT`/`LT`), `TTL`, `PTTL`, `EXPIRETIME`, `PEXPIRETIME`, `PERSIST`, `TOUCH`, `FLUSHDB`, `FLUSHALL` (a range delete per `db`, paged), `DBSIZE` (a counted scan of the meta range; documented as O(n)).
- The write order rule (T0-7): a handler that writes the meta key writes (or locks) it first.

**Tests:** `layout_keys_are_disjoint_by_kind` (proptest over user keys and dbs), `meta_record_roundtrip`, `meta_key_is_written_first` (a recording `Txn` wrapper), `set_options_matrix`, `wrongtype_on_every_write`, `lazy_expiry_hides_and_replaces`, `large_string_chunks_roundtrip`, `flushdb_spares_other_dbs`; on every backend. Gate: `unit/type/string` (with skip entries for `LCS`, `SETRANGE` over 512 MB and encoding-only tests).

Commit: `resp: the Redis key layout, the meta record, strings and the generic key commands (KV2 Task 3)`.

### Task 4 — Counters

`INCR`, `DECR`, `INCRBY`, `DECRBY`, `INCRBYFLOAT` with Valkey's overflow and parse errors (`ERR value is not an integer or out of range`, `ERR increment or decrement would overflow`, `ERR value is not a valid float`), Valkey's float formatting (17 significant digits, no trailing zeros, `inf` refused). **Tests:** `incr_overflow_and_parse_errors`, `incrbyfloat_formatting`, `concurrent_incr_loses_no_update` (16 clients × 100 `INCR` on one key over TCP; 1,600). Gate: `unit/type/incr`.

Commit: `resp: counters (KV2 Task 4)`.

### Task 5 — Hashes

Tidis `src/tikv/hash.rs` and `src/cmd/h*.rs` as the base, D1653's layout: `HSET`, `HSETNX`, `HMSET`, `HGET`, `HMGET`, `HGETALL`, `HKEYS`, `HVALS`, `HLEN`, `HEXISTS`, `HDEL`, `HSTRLEN`, `HINCRBY`, `HINCRBYFLOAT`, `HRANDFIELD`, `HSCAN`; the length in 16 sharded counters (Q1226) beside the data; deleting or overwriting a hash moves its version to the GC queue (D1656; the worker is Task 10). **Tests:** `hash_length_shards_sum`, `hash_overwrite_by_set_queues_gc` (Tidis #91 does not recur), `hset_on_string_is_wrongtype`, `hrandfield_counts`. Gate: `unit/type/hash`.

Commit: `resp: hashes with sharded length counters (KV2 Task 5)`.

### Task 6 — Sets

Tidis `src/tikv/set.rs`: `SADD`, `SREM`, `SMEMBERS`, `SISMEMBER`, `SMISMEMBER`, `SCARD`, `SPOP`, `SRANDMEMBER`, `SMOVE`, `SINTER`/`SINTERCARD`/`SINTERSTORE`, `SUNION`/`SUNIONSTORE`, `SDIFF`/`SDIFFSTORE`, `SSCAN`. **Tests:** `set_algebra_matches_model` (proptest against `BTreeSet`), `spop_count_and_srandmember_negative`. Gate: `unit/type/set`.

Commit: `resp: sets (KV2 Task 6)`.

### Task 7 — Sorted sets

Tidis `src/tikv/zset.rs`, D1653's member and score-index keys with `key::f64`: `ZADD` (`NX`/`XX`/`GT`/`LT`/`CH`/`INCR`), `ZINCRBY`, `ZREM`, `ZCARD`, `ZSCORE`, `ZMSCORE`, `ZRANK`/`ZREVRANK` (`WITHSCORE`), `ZRANGE` (`BYSCORE`/`BYLEX`/`REV`/`LIMIT`), `ZRANGESTORE`, `ZREVRANGE`, `ZRANGEBYSCORE`, `ZREVRANGEBYSCORE`, `ZRANGEBYLEX`, `ZREVRANGEBYLEX`, `ZCOUNT`, `ZLEXCOUNT`, `ZREMRANGEBYRANK`/`BYSCORE`/`BYLEX`, `ZPOPMIN`, `ZPOPMAX`, `ZRANDMEMBER`, `ZUNION`/`ZINTER`/`ZDIFF` and their `STORE` forms (`WEIGHTS`, `AGGREGATE`), `ZINTERCARD`, `ZSCAN`. Rank by counting through the score index (documented O(rank)). **Tests:** `zset_ranges_match_model` (proptest against a `BTreeMap` model), `zadd_nan_refused_and_negative_zero`, `zrange_bylex_bounds`. Gate: `unit/type/zset`.

Commit: `resp: sorted sets (KV2 Task 7)`.

### Task 8 — Lists

Tidis `src/tikv/list.rs` (integer indexes from 2^63, head and tail in the meta trailer): `LPUSH`, `RPUSH`, `LPUSHX`, `RPUSHX`, `LPOP`, `RPOP` (with count), `LLEN`, `LINDEX`, `LRANGE`, `LSET`, `LTRIM`, `LINSERT`, `LREM`, `LPOS`, `LMOVE`, `RPOPLPUSH`; `LINSERT` and `LREM` shift keys and refuse lists over a configured length (`list-max-shift`, default 10,000) with a documented error. Blocking forms (`BLPOP` family) are out (§77 §8.5). **Tests:** `list_ops_match_model` (proptest against `VecDeque`), `linsert_shift_limit`. Gate: `unit/type/list-2`, `unit/type/list-3`.

Commit: `resp: lists (KV2 Task 8)`.

### Task 9 — Keyspace commands

`KEYS` (glob), `SCAN` (an opaque numeric cursor mapped to a resume key per connection, `MATCH` glob, `COUNT`, `TYPE`), `RANDOMKEY` (a random start in the meta range, not a full load; `distributedio/titan` #230), `RENAME`, `RENAMENX` (copy the meta to the new key, re-keying the data under a fresh version, or for collections over a size limit an `ERR` documented), `COPY`, `OBJECT ENCODING` (a fixed answer per type), `OBJECT FREQ`/`IDLETIME` refused. Valkey's glob matching is re-implemented from its documented semantics, not ported. **Tests:** `glob_matches_valkey_cases`, `scan_cursor_survives_writes` (every key present for the whole scan is returned at least once), `rename_moves_ttl`. Gate: `unit/keyspace`.

Commit: `resp: KEYS, SCAN, RENAME and the keyspace commands (KV2 Task 9)`.

### Task 10 — The expiry and GC workers, leases

`lease.rs`: a lease row `R ‖ 'L' ‖ name` (holder, expiry), taken and renewed by compare-and-put, so one worker per keyspace runs on either backend. `expire.rs` (D1655, `distributedio/titan`'s design, bucketed): every TTL write maintains `R ‖ db ‖ 'x' ‖ bucket ‖ expire_ms ‖ enc(ukey) ‖ ver`; the worker scans each bucket up to now, deletes a key only when its meta still has that `ver`, then moves its version to GC. `gc.rs` (D1656): the worker pages the GC queue and deletes each version's data in transactions of at most 256 keys. Both report metrics (`loams_resp_expired_total`, `loams_resp_gc_keys_total`, no tenant labels). **Tests:** `expired_keys_are_reclaimed_without_reads`, `overwritten_key_is_not_expired_by_stale_index`, `gc_deletes_in_bounded_batches` (a 10,000-field hash), `one_lease_holder` (two workers on one store). Gate: `unit/expire`.

Commit: `resp: the bucketed expiry worker, the GC worker and their leases (KV2 Task 10)`.

### Task 11 — The interactive transaction in `loams-kv`

`Store::begin(opts: TxnOptions) -> Result<OpenTxn, TxnError>`; `OpenTxn` derefs to `Txn` and has `commit(self) -> Result<Ts, TxnError>` and `rollback(self)`; dropped without either, it rolls back; a lifetime cap (`TxnOptions::deadline`, default 10 s) after which `commit` returns `Deadline`. Embedded: `Handle::begin`; TiKV: the client's `Transaction` with the store's commit mode. No retry (the caller decides). Metrics as `Store::run`. **Tests** (in `kv_conformance!`): `open_txn_commits_and_rolls_back`, `open_txn_conflict_is_conflict`, `open_txn_lifetime_cap`, `open_txn_drop_rolls_back`.

Commit: `kv: an interactive transaction, Store::begin (D1657; KV2 Task 11)`.

### Task 12 — MULTI, EXEC, DISCARD, WATCH

D1657: `MULTI` queues; queue-time errors (unknown command, arity) mark the transaction (`EXECABORT`); `EXEC` runs the queue in one `Store::run` (retried on conflict) when nothing is watched, and each command's runtime error is its own reply while the others apply; `WATCH` opens an `OpenTxn` and records the watched keys' meta values; `EXEC` with watches re-reads them, locks their meta keys (`lock_keys`), runs the queue in that transaction and commits once, replying nil on any change or conflict; `DISCARD`, `UNWATCH`, `RESET` end it; `WATCH` inside `MULTI` is refused. **Tests:** `exec_runtime_error_does_not_abort_others`, `execabort_on_queue_error`, `watch_detects_concurrent_write`, `watch_on_expired_key`, `exec_retries_without_watch`. Gate: `unit/multi`.

Commit: `resp: MULTI, EXEC, DISCARD and WATCH on loams-kv transactions (KV2 Task 12)`.

### Task 13 — Durability modes, TLS, metrics

`durability = "durable"` (the default; every write commits before its reply) is the only mode until `loams-kv-hot` (KV1) exists; `write-through` and `hot-only` are parsed and refused with `requires loams-kv-hot (plan KV1)` (Ruling K2-3), so configs written now stay valid. TLS from `[tls]` (rustls, the gateway's cert loading). Metrics: `loams_resp_commands_total{command,outcome}`, `loams_resp_command_seconds{command}`, `loams_resp_connections`; no tenant labels (D1021). **Tests:** `durability_modes_parse_and_refuse`, `tls_handshake_and_ping`, `metrics_have_no_tenant_label`.

Commit: `resp: durability modes, TLS and command metrics (KV2 Task 13)`.

### Task 14 — The conformance gate in CI and the corpora

`.github/workflows/ci.yml` job `kv-resp` (path-filtered on `crates/loams-resp/**`, `crates/loams-kv/**`, `scripts/kv/**`): build the gateway, run report §1.4's subset with `crates/loams-resp/tests/valkey-skip.txt` (one reason per line) against it on the in-memory store, and in the TiKV job against the playground; fail on any failure not skipped. `scripts/kv/oracles/tidis-suite.sh` and the split Titan autotest cases (the owner's approval of 2026-10-24) run against the gateway, with their triage in `crates/loams-resp/tests/corpora.md`. The differential run: `loams-gw-test`'s `differential` runner replays a seeded command stream against Valkey and the gateway and diffs the replies, with an allowlist.

Commit: `ci: the Valkey subset gate and the reference corpora for loams-resp (KV2 Task 14)`.

### Task 15 — The compose `kv` profile

`deploy/release/compose.yaml`: a `loams-gateway-kv` service under profile `kv`, `--module resp`, on its own redb file (D1647, one replica), `requirepass` from the env file, port bound to the tailnet address as the other services are (D1648); the `tikv` profile variant points it at PD. Docs page `docs/kv/redis.md`: what works, what is out, the deviations. **Tests:** `docker compose … config -q` for each profile combination; the local e2e script's `kv` step (`PING`, `SET`, `GET`).

Commit: `deploy: the compose kv profile with the Redis gateway (KV2 Task 15)`.

### Task 16 — Benchmarks and Q1236

`scripts/kv/bench.sh`: `valkey-benchmark` (from the pinned image) `-t set,get,incr,lpush,rpop,sadd,hset,zadd -P 1,16 -c 50` against the gateway on embedded and on the TiKV playground, `durable` mode (the other modes once KV1 lands), and against Valkey as the reference; results in `docs/plans/kv2-report.md`. Q1236 (the raw fast mode) is decided from these numbers.

Commit: `kv: valkey-benchmark runs for loams-resp (KV2 Task 16)`.

### Task 17 — The KV2 report

`docs/plans/kv2-report.md`: the gate's counts and skip list, the corpora triage, the differential findings, the benchmarks, Q1236's answer, and any amendment to §77 for the owner.

Commit: `docs: the KV2 report (KV2 Task 17)`.

## Rulings made during execution

| # | Ruling | Why |
|---|---|---|
| T0-1 | **KV0 started 2026-10-23**, on the orchestrator's dispatch, once HE1 had started (the House engine priority of 2026-10-21 was "behind", not "instead of"). | The status line said KV0 waits for HE1; HE1 started 2026-10-23 |
| T0-2 | **Reviews: Greptile, by label**, not CodeRabbit. KV0 PRs that touch storage (`loams-kv`, `loams-tuple`), new public APIs or licences carry `review:greptile`; doc-only and CI-only PRs do not. | The loams repository's reviewer is Greptile on labelled PRs (2026-10-23); CodeRabbit reviews the site and Mastra repositories |
| T0-3 | **The light default is merged:** PR #973 (D1340–D1343) merged 2026-10-21. The single-node default is redb plus a local folder, so the gateways' compose `kv` profile runs on the embedded backend (D1647) with no TiKV. | `gh pr view 973` |
| T0-4 | **D851's pessimistic mode is half built.** `loams-tikv` has it: `Mode::Pessimistic` maps to `TransactionOptions::new_pessimistic()` (`runner.rs`), and its `Txn` has `get_for_update`, `batch_get_for_update` and `lock_keys` (`txn.rs`). `loams-kv` still refuses it (`PESSIMISTIC_REFUSED`, `store.rs`), and its embedded backend has no lock table. So Task 5 builds the `loams-kv` surface on TiKV by delegation and the embedded lock table in full. | `crates/loams-tikv/src/{runner,txn}.rs`, `crates/loams-kv/src/store.rs` on `dev` at `8919d32` |
| T0-5 | **GW1 has no module seam yet.** `GatewayConfig` has `admin`, `routes` and `pg` only, with `deny_unknown_fields`; the Postgres module is wired directly. KV2's first task adds the seam (`[resp]`, `[cql]`). | `crates/loams-gateway/src/config.rs` |
| T0-6 | **Pins.** Valkey `9.1.2` = tag commit `7f1dffedff6de73058b2c2a389422b6ecd56c8fb`; `valkey/valkey:9.1.2` = `sha256:418652cfb58ef879d4978c33553735d7147016032d5aefaa14c828e611eb9dfd` (the multi-arch index); `cassandra:5.0.9` = `sha256:3ea9333712c5692f3fe91d6c4dcc262df9e36338c4fdd1bc00a5e53f1137a1a0` (the index); Titan `distributedio/titan` = `97868f5d24e0c13621658fbcb712064fec59e8be` (2021-10-09); Tidis `tidb-incubator/tidis` = `555856206f248737b12d3f148b10f08a604bc821` (2024-01-06). | GitHub refs API; Docker Hub registry `HEAD` of each tag's index, 2026-10-23; the owner's reference clones |
| T0-7 | **(verify) 1, the primary key: the first key written or locked, not settable.** The pinned client sets the buffer's `primary_key` with `get_or_insert_with` on the first `put`/`insert`/`delete` (`insert_entry`, any entry except a cached read or `CheckNotExist`) or the first `lock` (`lock_keys`, `get_for_update`); plain reads never set it, and there is no setter (`src/transaction/buffer.rs` at `41b3588`). So D1653's "the meta key is the primary" holds when a command writes, or locks, its meta key first; KV2's handlers order their writes that way, and a test asserts it through the client's buffer if the fork exposes it, else by write order. | `client-rust` `41b3588`, `buffer.rs` lines 39–41, 200–203, 323–328 |
| T0-8 | **(verify) 2, the reverse scanner: no.** `Transaction::scanner` and `Snapshot::scanner` are forward only and refuse a reversed range ("scanner does not support reverse ranges … use scan_reverse"). Task 3's reverse pages use `scan_reverse` with a moving upper bound, as the plan's fallback says. | `client-rust` `41b3588`, `src/transaction/transaction/scanner.rs` lines 42–90 |
| T0-9 | **(verify) 3, redb 4's `InMemoryBackend`: yes.** redb 4.4.0's `InMemoryBackend` implements every `StorageBackend` method (`sync_data` is a no-op; the optional lock methods are unsupported, which only matters for cross-process file locks). The embedded backend's committer and GC threads use only `Core::db` and never the path; the path is used for the registry key and `Database::create`, which `Database::builder().create_with_backend` replaces (the `faults` feature already opens through a custom backend, `faulty.rs`). Task 4 keys the registry by `mem:<name>`. | `redb-4.4.0/src/tree_store/page_store/backends.rs`; `crates/loams-kv/src/embedded/{mod,gc,commit,faulty}.rs` |
| T0-10 | **Q1239 done:** `deploy/tikv/compose.yaml` defaults `TIKV_TAG` to `v8.5.8`, matching `deploy/release/compose.yaml` and CI. | `docker compose -f deploy/tikv/compose.yaml config -q` passes |
| T2-1 | **The linearizability checker is `loams_meta_conformance::linearizability`**, the Wing–Gong–Lowe checker `loams-sim` re-exports, taken directly as a dev-dependency of `loams-kv`, with a value register model (`get`, `put`, `delete`, `compare_and_put`) in `tests/linearizable.rs`. Depending on `loams-sim` would build its whole cluster simulation (its default `sim` feature) for one function. Operations that end in `Conflict` or `NotApplied` (certainly not applied) are left out of the history; `Undetermined` and `Deadline` are indeterminate. | `crates/loams-sim/src/lib.rs` re-exports the checker from `loams-meta-conformance` (M1.2a Ruling 14) |
| T3-1 | **The scanner pages through the seam's bounded scans on both backends**, not the fork's `Transaction::scanner`: each page is one `Txn::scan`/`scan_reverse` (or `Snap`'s) of `page` pairs from just past the last key returned (forward: `last ‖ 0x00`; reverse: `last` as the exclusive end), all at the same timestamp. The fork's scanner is forward only (T0-8), and paging through the seam keeps the transaction's write overlay and the error mapping of `Txn::scan` on both backends; `loams-tikv`'s scans already page by region internally. The API is `scanner(start, end, page)` and `scanner_reverse(…)`, mirroring `scan`/`scan_reverse`; a `Scanner` yields `next_page()` or `next()`. A page size of 0 is read as 1. | T0-8; `crates/loams-tikv/src/txn.rs` (`paged_scan`) |
| T4-1 | **`EmbeddedConfig::in_memory(name, keyspace)`** takes the keyspace too (every `EmbeddedConfig` constructor does), and `EmbeddedConfig` gains `memory: Option<String>` (functional-update users are unaffected). The registry key `mem:<name>` is a relative path, so it never equals a canonical file path. An in-memory store opens through `Database::builder().create_with_backend(InMemoryBackend::new())`; the `faults` feature's I/O faults do not apply to it (no file, no sync). Reopening a name whose last handle is gone starts a fresh, empty database at once. | T0-9 |
| T5-1 | **Pessimistic mode's semantics.** Lock waits (both backends) give up with `Conflict` at `start + deadline − min(deadline/10, 1 s)`, so the body sees the `Conflict` before the runner's own deadline; on TiKV the limit reaches `get_for_update`, `batch_get_for_update`, `put`, `insert`, `delete` and `lock_keys` through a task-local set around `run_as` (the body runs on that task), mapping an elapsed wait to `Conflict`. Embedded: a per-file lock table (key prefix → owner), waits woken on release, a waits-for check that refuses a wait closing a cycle (`Conflict` at once), and a committer check that refuses any commit writing or locking a key another transaction holds. `get_for_update` reads at a fresh timestamp after `wait_visible`, and the key's commit check starts at that timestamp, not the start. Plain `get` stays at the start timestamp, as on TiKV. In an optimistic transaction `get_for_update` is `get` then `lock_keys`. `pessimistic_second_holder_waits` allows the runner's retries, because TiKV may answer a woken lock wait with a write conflict. | D851; TiKV's pessimistic lock wait (`wait-for-lock-timeout`); row T21-15 |
| T10-1 | **Oracle feasibility results** (`kv-oracles.yml` on #1139, run 40044010136, 2026-10-24). (1) Valkey 9.1.2's suite, report §1.4 subset, against `valkey/valkey:9.1.2`: exit 0, **820 ok, 0 err, 0 skip, 89 ignored** (tags denied), about 2 minutes; this is the gateway's control. The suite runs in external mode with the image's binaries copied into `src/`, nothing built. (2) Tidis's Python suite minus Lua against Valkey (Python 3.11: `redis==3.5.3` needs `distutils`; the driver sends `RESET` before each test, because `test_multi_error` otherwise leaves the shared connection inside MULTI and poisons every later test): **171 run, 63 failures, 1 error, 1 skipped**; 59 of the failures are `InvalidTest` cases asserting Tidis's error text ("Invalid arguments") where Redis and Valkey say "wrong number of arguments for '…' command", the rest are Tidis's MULTI wording and three type tests. So Tidis's corpus is a semantics reference with a deviation list, not a pass/fail gate. (3) Titan (`97868f5`) **builds unmodified with Go 1.15.15** (`GOFLAGS=-mod=mod`, the Go proxy) and **serves on `mocktikv://`** (PING, SET, GET), binary sha256 `e16f6db0…3a2b`; no image was built or pushed, so there is no image digest: Titan is usable as a differential oracle in KV2. (4) Titan's autotest against Valkey (harness-side `TestMain`, Valkey with the autotest's token as `requirepass`) runs but is one `Test` function: it records 53 assertion failures in `ZSetCase` (Titan's score formatting and ZSCAN behaviour) and then panics in `ZScanEqual`, so nothing after it runs. To use it, KV2 must split its cases or keep it as a reading reference. | §77 §12.3; `scripts/kv/oracles/` |
| K2-1 | **The Redis gateway is KV2 in this plan.** The coordinator's dispatch of 2026-10-26 called it "KV1"; the plan's KV1 (`loams-kv-hot`, the CDC consumer) is unchanged and not started. KV2 depends on KV1 only for the `write-through` mode (K2-3). | The milestone table above; §77 §14 |
| K2-2 | **The module seam is `loams-gateway --module resp --config resp.toml`**, a `resp` cargo feature, as DX1 Task 9's `temporal` module does; not `[resp]` sections in `GatewayConfig` (supersedes T0-5's plan; proposed to the owner as amendment A1 in the KV0 report). | `crates/loams-gateway/src/{main.rs,temporal/mod.rs}`; D1646's one process per Deployment |
| K2-3 | **Durability modes before `loams-kv-hot`:** `durable` (the default) is built; `write-through` and `hot-only` are parsed and refused with a message naming plan KV1, so configuration written now stays valid. | D1651; KV1 not started |
| K2-4 | **The meta key is the primary by write order** (T0-7): handlers write or lock the meta key first; Task 3's `meta_key_is_written_first` holds them to it. No client change. | `client-rust` `41b3588` `buffer.rs` |
| K2-5 | **No `redis-protocol` dev-dependency.** Its 6.0.0 pins `cookie-factory = "=0.3.2"`, which would downgrade `age`'s `cookie-factory` 0.3.3 in the lockfile. Frame tests are model-based; the real-implementation oracle is the differential run against Valkey (Task 14). Proposed to the owner as amendment A2. | `redis-protocol-6.0.0/Cargo.toml`; `Cargo.lock` |
| K2-6 | **The KV0 report's amendments A1–A3 are accepted as coordinator rulings, pending the owner's confirmation** (2026-10-27; the owner was told and can override). A1: the `resp` and `cql` modules run as `loams-gateway --module <name>` with their own config files, as `temporal` does (K2-2). A2: no `redis-protocol`; Valkey is the oracle (K2-5). A3: Valkey 9.1.2's suite is the gate. | The coordinator's message of 2026-10-27; [KV0 report](kv0-report.md) §5 |
| K2-7 | **Task 2's suite runner.** `scripts/kv/resp-suite.sh [unit…]` starts `loams-gateway --module resp` on an in-memory store and runs `scripts/kv/oracles/valkey-suite.sh`, which now takes `VALKEY_UNITS`, `VALKEY_SKIPFILE` (reasons after ` # `, stripped before `--skipfile`) and `DOCKER` (podman locally), and reuses a fetched tree of the pinned commit. The gateway feature for TiKV is `resp-tikv`. | `scripts/kv/` |
| K2-8 | **At most 64 `SELECT` databases** (`MAX_DATABASES`; Q1234's default stays 16). A database's keys start with its index byte under `R`, beside the keyspace-wide rows `'L'` (0x4C, leases), `'g'` (0x67, the GC queue) and `'j'` (0x6A, the journal) of §77 §8.4, so an index of 76 or more would share a prefix with them and `FLUSHALL`'s range `[0, databases)` would reach them. 64 keeps every database byte below `'L'`; a compile-time assertion holds it. Titan's 0–255 is not needed by the suite (it uses 9 and 10). | §77 §8.4; `crates/loams-resp/src/config.rs` |
| K2-9 | **Task 3's shape.** The types take a crate-private `Kv` trait (get, batch_get, scan, put, delete, start_ts) implemented for `loams_kv::Txn`, so `meta_key_is_written_first` runs on a recording implementation. The string and generic-key handlers are written from Valkey's documented semantics with Tidis read as a reference, not ported, so they carry no ported-code header. A version is the creating transaction's start timestamp, or one past the version it replaces when that is not lower (`layout::next_version`), so a delete and a re-create in one transaction never share data keys. Large strings are rewritten whole on `APPEND`/`SETRANGE` (O(n), documented). | §77 §8.3–§8.4; `crates/loams-resp/src/{layout.rs,types/}` |
| K2-10 | **Collections and cursors (Task 5 on).** Collections share `types/collection.rs`: creation writes the meta key first; an item counts in the length shard `fnv1a(item) % 16` (so it is always added to and removed from one shard); a removal that would empty a collection deletes the meta key first and queues the version for GC rather than deleting items one by one (K2-4 holds). `*SCAN` cursors are opaque numbers mapped per connection to a resume item (`cursor.rs`, 64 kept, each used once; an unknown cursor answers an empty final page), the same design `SCAN` takes in Task 9. `HRANDFIELD` reads the whole hash (O(n), documented). Hot single-key updates (`INCR`, `HINCRBY*`) get 64 runner attempts. | §77 §8.3–§8.5 |
| K2-11 | **Multi-key writes and the primary (Task 6 on).** A command that writes several keys (`SMOVE`, the `*STORE` forms, later `RENAME`, `LMOVE`, `ZRANGESTORE`) writes each key's meta before that key's data, but only the first key's meta can be the transaction's primary; this matches the client's rule (T0-7) and costs only the lock-wait effect of distributedio/titan#180 on the later keys. `SPOP` and `SRANDMEMBER` read the whole set (O(n), documented, as `HRANDFIELD`); `SINTERCARD` reports no key positions to `COMMAND GETKEYS` (its keys follow `numkeys`). | T0-7, K2-4 |
| K2-12 | **Sorted sets (Task 7).** Members `'m' ‖ enc(member)` → score bits; the index `'z' ‖ key::f64(score) ‖ enc(member)`. By-score ranges read only their index range; by-rank ranges read from the near end to the last rank asked for; `ZRANK` counts through the index (O(rank)). By-lex ranges read the member keys (byte order), which equals index order for the equal-score sets Redis defines them on. `-0` is stored as `0` (documented deviation if the suite prints `-0`). `ZUNION`/`ZINTER`/`ZDIFF` read whole inputs (plain sets count as score 1) and treat a NaN from `inf + -inf` as 0, as Redis does. `ZRANDMEMBER` reads the whole set. | §77 §8.3–§8.4 |

### Owner approvals (2026-10-24)

The owner approved the pending decisions below on 2026-10-24 ("approve all with your recommendations").

- **T7-1 (proposed, now approved): the small metrics registry stays inside `loams-kv` for now.** It is extracted into its own crate when a second crate needs it. **Approved (owner, 2026-10-24).**
- **Titan's auto tests** (Task 0 item 4) become a split-up test corpus in KV2 rather than a harness run against Valkey. **Approved (owner, 2026-10-24).**
