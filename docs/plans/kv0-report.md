# KV0 report — the `loams-kv` foundation for the KV gateways

Plan: [KV1 plan](2026-10-21-kv1-kv-gateways.md), KV0 Task 11. Design: [§77](../design/77-kv-gateways.md). Written 2026-10-26 on `dev` at `817d138`.

**Status: KV0 is complete.** Tasks 0 to 10 are merged (#1125, #1151, #1153, #1159, #1160, #1167, #1177, #1178, #1179, #1161, #1139). All three of §77 §14's Phase 0 exit criteria are met:

- the codec's property tests pass;
- the embedded, in-memory and TiKV stores pass the same 26-case conformance suite;
- this report.

Three amendments to §77 are proposed for the owner (section 5).

## 1. Exit criteria, measured

All counts are from local runs on 2026-10-26: `cargo test -p loams-kv -p loams-tuple -p loams-gw-test`, then `LOAMS_TEST_PD=127.0.0.1:19379 cargo test -p loams-kv --test conformance --test linearizable --test tenant --test metrics` against `scripts/tikv/playground.sh` (TiKV and PD v8.5.8, API v2, one store).

| Suite | Embedded (redb file) | In-memory (redb `InMemoryBackend`) | TiKV v8.5.8 |
|---|---|---|---|
| `kv_conformance!` (26 cases each) | 26 / 26 | 26 / 26 | 26 / 26 (a real cluster, 133 s; without `LOAMS_TEST_PD` they skip) |
| `compare_and_put_is_linearizable` (8 tasks × 200 ops, 4 keys) | pass | — | pass (35.7 s) |
| `tenants_are_isolated`, `product_roots_are_disjoint` | pass | — | pass |
| `metrics_count_each_outcome`, `metrics_have_no_tenant_label`, `observer_sees_keyspace` | pass | — | pass |
| `loams-kv` unit tests | 22 / 22 | | |
| `loams-kv` `tests/embedded.rs`, `tests/crash.rs` | 17 / 17, 2 / 2 | | |
| `loams-kv` `tests/conformance.rs` (all of it) | 84 / 84 in each run: 3 × 26 cases plus 6 others | | |
| `loams-tuple` `tests/key.rs` (proptest, 4,096 cases each) | 5 / 5: `key_order_matches_value_order`, `key_composites_order_lexicographically`, `key_roundtrip`, `key_no_prefix_aliasing`, `key_agrees_with_tuple` | | |
| `loams-gw-test` `harness` (`same_scenario_same_report_on_every_backend` and two more) | 3 / 3 | | |

The 26 conformance cases are the 15 from LV1, with `pessimistic_mode_is_refused` replaced by Task 5's four pessimistic cases, plus Task 2's four one-shot cases, Task 3's four scanner cases and the barrier case. The plan's five review-focus checks each have a passing test: key order and aliasing (Task 1), CAS linearizability (Task 2), pessimistic mode leaving optimistic behaviour unchanged (Task 5), tenant isolation (Task 6), and metrics without tenant labels (Task 7).

**About CI.** CI's `TiKV: suites` job runs `cargo test -p loams-kv` against a playground. On the KV0 PRs it was either skipped by the path filter or cancelled (#1167, #1178: the runs were cancelled, not failed). So the TiKV column above is the first full run on record. When the job next runs on `dev`, it will confirm the result in CI.

## 2. Task 0: dependencies, pins and §77's (verify) items

| Item | Answer | Ruling |
|---|---|---|
| The light default (PR #973, D1340–D1343) | Merged on 2026-10-21. The gateways' compose `kv` profile runs on the embedded backend with no TiKV (D1647) | T0-3 |
| D851's pessimistic mode | `loams-tikv` already had it. `loams-kv` now has it on both backends: Task 5 built an embedded lock table with a waits-for check | T0-4, T5-1 |
| The module seam in `loams-gateway` | There was none on 2026-10-23. Since then, DX1 Task 9 added one: a `--module <name>` process with its own config file, behind a cargo feature (the `temporal` module) | T0-5, and amendment A1 below |
| Pins | Valkey 9.1.2 `7f1dffe…`, image `sha256:418652cf…`; `cassandra:5.0.9` `sha256:3ea93337…`; Titan `97868f5…`; Tidis `5558562…` | T0-6 |
| (verify) 1: how the transaction's primary key is chosen | It is the first key written or locked; it cannot be set. So KV2's handlers write (or lock) the meta key first | T0-7 |
| (verify) 2: does the fork's scanner run in reverse? | No. Reverse pages use `scan_reverse` with a moving bound, and the scanner pages through the seam on both backends | T0-8, T3-1 |
| (verify) 3: redb 4's `InMemoryBackend` | It is complete for our use. The in-memory store is keyed `mem:<name>` | T0-9, T4-1 |
| The dev playground's TiKV version (Q1239) | Now v8.5.8 | T0-10 |

§77's other (verify) items belong to later milestones and stay open:

- **§12.1, `tikv/migration`'s RawKV BR** for raw fast-mode keyspaces. Belongs to KV2. It only matters if Q1236 chooses the raw mode.
- **§13.2, TiKV-CDC on TiKV 8.5.** Belongs to KV1. Moot unless TiKV-CDC is ever run, because we port its consumer (D1652).
- **§12.3, Titan's build from the Go proxy.** Answered by Task 10: it builds (section 3).

## 3. Task 10: oracle feasibility

All four runs are from `kv-oracles.yml` on #1139 (run 40044010136). The full numbers are in Ruling T10-1.

| Oracle | Result | Use in KV2 |
|---|---|---|
| Valkey 9.1.2's TCL suite (report §1.4 subset) against `valkey/valkey:9.1.2` | 820 ok, 0 err, 0 skip, 89 ignored, in about 2 minutes. It runs in external mode with the image's binaries, so nothing is built | **The gate.** The gateway must match this control, apart from a checked-in skip list |
| Tidis's Python suite, minus Lua, against Valkey | 171 run, 63 failures, 1 error. 59 of the failures assert Tidis's own error text | **A semantics reference**, with a deviation list. Not a pass/fail gate |
| Titan `97868f5` on `mocktikv://` | Builds unmodified with Go 1.15.15 and serves `PING`/`SET`/`GET`. No image was published | **A second differential oracle** (§12.3) |
| Titan's autotest against Valkey | One `Test` function: 53 assertion failures in `ZSetCase`, then a panic. Nothing after the panic runs | Split into a case corpus, as the owner approved on 2026-10-24 |

## 4. What KV0 leaves for later milestones

| What | Where in §77 | Lands in |
|---|---|---|
| An interactive transaction (`Store::begin`) for WATCH | §6, D1657 | KV2 |
| TTL and GC workers | §6, D1655–D1656 | KV2 |
| The CDC consumer and `loams-kv-hot` | §11 | KV1 |
| The opt-in raw fast mode | Q1236 | Decided by KV2's benchmarks |

None of these blocks the Redis gateway's `durable` mode.

## 5. Proposed amendments to §77, for the owner

- **A1. The module seam is the `--module` process, not config sections.** §77 §10.1 and the plan's KV2 row call for `[resp]` and `[cql]` sections in `GatewayConfig`. Since then, DX1 Task 9 built the gateway's module seam differently: `loams-gateway --module temporal --config temporal.toml`, one module per process, behind a cargo feature. That is exactly §77's "one process per gateway Deployment" (D1646). **Proposal:** `resp` and `cql` follow the `temporal` pattern (`--module resp --config resp.toml`). `GatewayConfig`, the Postgres front door's file, stays unchanged.
- **A2. Valkey is the decoder's oracle, not `redis-protocol`.** Report §6.1 recommended `redis-protocol` (MIT) as a property-test oracle. But `redis-protocol` 6.0.0 pins `cookie-factory = "=0.3.2"`. Adding it would downgrade the lockfile's `cookie-factory` from 0.3.3, which `age` (encryption) uses. **Proposal:**
  - `loams-resp`'s frame tests use their own model-based property tests;
  - the cross-check against a real implementation is the differential run against Valkey (KV2 Task 14).
- **A3. Valkey's suite fits as the gate (no change needed; confirming §12).** The suite needs only a tolerant `CONFIG GET/SET`, `FLUSHALL`, `FUNCTION FLUSH` and `DEBUG SET-ACTIVE-EXPIRE`, and the control run is clean (section 3). §77 §12's choice therefore stands. The KV2 tasks add these four commands early (Task 2), so each data-type task can be gated on its own unit file.

No KV0 finding changes §77's key layout (§8.4), its decisions D1640–D1665, or the phase order.
