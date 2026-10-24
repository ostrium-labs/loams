# KV1 — KV Gateways: Redis (RESP) and Cassandra (CQL) on `loams-kv` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, **test first**: write the named tests, run them and watch them fail, then implement until they pass. Where this plan gives exact names, paths, flags or tests, use them verbatim. Record any deviation in "Rulings made during execution" at the end of this file.
>
> **Status: Approved** (owner, 2026-10-21, all defaults; stub). Track **KV**, design [§77](../design/77-kv-gateways.md) (D1640–D1665, Q1220–Q1239). The owner gave the House engine ([HE1](2026-10-21-he1-loams-house-engine.md)) priority on 2026-10-21; KV0 started on 2026-10-23, after HE1 (Ruling T0-1). **KV0 progress (2026-10-24):** Tasks 0, 1, 3, 9 and 10 merged (#1125, #1151, #1159, #1161, #1139). Open: Task 2 (#1153), Task 4 (#1160), Task 5 (#1167), and drafts for Task 6 (#1177), Task 7 (#1178, merge after 2, 4 and 5) and Task 8 (#1179, stacked on #1160). Not started: Task 11 (the KV0 report; Task 10's results are in Ruling T10-1).
>
> **Coverage:** this plan specifies **KV0** (the owner's Phase 0, the `loams-kv` foundation) task by task. KV1–KV5 are outlined at milestone level and get full task specifications when KV0 has reported.
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
| T3-1 | **The scanner pages through the seam's bounded scans on both backends**, not the fork's `Transaction::scanner`: each page is one `Txn::scan`/`scan_reverse` (or `Snap`'s) of `page` pairs from just past the last key returned (forward: `last ‖ 0x00`; reverse: `last` as the exclusive end), all at the same timestamp. The fork's scanner is forward only (T0-8), and paging through the seam keeps the transaction's write overlay and the error mapping of `Txn::scan` on both backends; `loams-tikv`'s scans already page by region internally. The API is `scanner(start, end, page)` and `scanner_reverse(…)`, mirroring `scan`/`scan_reverse`; a `Scanner` yields `next_page()` or `next()`. A page size of 0 is read as 1. | T0-8; `crates/loams-tikv/src/txn.rs` (`paged_scan`) |
| T10-1 | **Oracle feasibility results** (`kv-oracles.yml` on #1139, run 40044010136, 2026-10-24). (1) Valkey 9.1.2's suite, report §1.4 subset, against `valkey/valkey:9.1.2`: exit 0, **820 ok, 0 err, 0 skip, 89 ignored** (tags denied), about 2 minutes; this is the gateway's control. The suite runs in external mode with the image's binaries copied into `src/`, nothing built. (2) Tidis's Python suite minus Lua against Valkey (Python 3.11: `redis==3.5.3` needs `distutils`; the driver sends `RESET` before each test, because `test_multi_error` otherwise leaves the shared connection inside MULTI and poisons every later test): **171 run, 63 failures, 1 error, 1 skipped**; 59 of the failures are `InvalidTest` cases asserting Tidis's error text ("Invalid arguments") where Redis and Valkey say "wrong number of arguments for '…' command", the rest are Tidis's MULTI wording and three type tests. So Tidis's corpus is a semantics reference with a deviation list, not a pass/fail gate. (3) Titan (`97868f5`) **builds unmodified with Go 1.15.15** (`GOFLAGS=-mod=mod`, the Go proxy) and **serves on `mocktikv://`** (PING, SET, GET), binary sha256 `e16f6db0…3a2b`; no image was built or pushed, so there is no image digest: Titan is usable as a differential oracle in KV2. (4) Titan's autotest against Valkey (harness-side `TestMain`, Valkey with the autotest's token as `requirepass`) runs but is one `Test` function: it records 53 assertion failures in `ZSetCase` (Titan's score formatting and ZSCAN behaviour) and then panics in `ZScanEqual`, so nothing after it runs. To use it, KV2 must split its cases or keep it as a reading reference. | §77 §12.3; `scripts/kv/oracles/` |
