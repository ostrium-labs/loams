# SQ1 — Loams SQL to Production: Serverless MySQL 8.4, mywal on TiKV, Routed by Vitess — Implementation Plan

> **Superseded 2026-10-08 (Task 0).** The owner redirected Loams SQL to "Neon-like MySQL" on TiDB compute over Loams TiKV keyspaces. [§47](../design/47-loams-sql-production.md) is rewritten (D720–D739, Q655–Q669), and its §19 holds the revised milestone outline (SQ1a–SQ1h, SQ1s). The tasks below describe the withdrawn MySQL 8.4 + mywal + Vitess design and must not be executed. This plan is rewritten from §47 §19 once the blocking questions Q661 and Q667 are answered.

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Each task lists the interfaces it must produce and the tests that must exist and pass before it is done. Where this plan gives exact values (names, flags, error codes, paths), use them verbatim. Where it gives a contract and named tests, write the code to that contract, and record any deviation in "Rulings made during execution" at the end of this file. The code is not pre-written in this plan.
>
> **Status: Planned** (2026-10-08, revised the same day for the owner's directives: MySQL 8.4 from day one, TiKV-backed durability, mywal). **Track SQ** (design [§47](../design/47-loams-sql-production.md), D720–D739, Q655–Q669). Branches `sq1-t<N>`, stacked per milestone; PRs target `dev`. No MySQL fork is needed: MySQL 8.4, XtraBackup and Vitess are consumed as pinned images. SQ1 absorbs §31's RT3 for MySQL (re-targeted at MySQL 8.4); §29's WS1–WS4 are not part of SQ1 (D721, Q657).

**Goal:** Loams SQL generally available: a serverless MySQL 8.4 LTS database (stock MySQL Community with InnoDB) whose every acknowledged commit is durable in **mywal** — a Loams-owned service that receives the binlog as a lossless semi-sync replica and stores it on a TiKV quorum with the bucket as the cold tier — routed by Vitess, fronted by a Loams gate (TLS, auth, wake-on-connect), managed through `loams.sqldb.v1`, with §47's conformance, performance and security evidence.

| Milestone | Scope | Tasks | Exit |
|---|---|---|---|
| **SQ1a** | Reconcile, measure, images, the control plane API and records, the local runtime, lifecycle, the gate v0, the desktop on the new API | 0–9 (10) | `loams dev --features sqldb` creates, suspends and resumes a MySQL 8.4 database; the AP1e Loams SQL page runs on `loams.sqldb.v1` |
| **SQ1b** | **mywal**: protocol spec and TLA+, the replication codec, the shared TiKV store, the semi-sync replica session, crash recovery, readers and replicas, offload and retention, fencing and failover, **the mywal performance gate** | 10–18 (9) | §47 §8's gate passes on the reference topology, or the owner has chosen under Q666 |
| **SQ1c** | Vitess routing: reconcile, the Kubernetes driver, full gate auth, rendering, TLS, settings, roles, ORM pooling, the Vitess/8.4 compatibility gate, sharding beta | 19–28 (10) | A database is served through gate → vtgate → vttablet → MySQL 8.4 with mywal durability, auth, TLS and ACLs |
| **SQ1d** | Serverless operations: suspend/resume on Kubernetes, storage, classes, HA, resize, snapshots, PITR and branches, export, quotas, observability, upgrades | 29–39 (11) | Every §47 §10–§14, §19–§20 and §22 behaviour tested on kind and on the reference cluster |
| **SQ1e** | Evidence, docs, the GA flip | 40–46 (7) | "Exit criteria for production" all ticked |
| **SQ1f** | Engine query SQL (not Loams SQL): `loams.sql.v1`, the Data Studio SQL tab, removal of the engine's `mysql-wire` listener, RPC limits | 47–50 (4) | Independent of SQ1a–e |

**Architecture** (§47 §3):
- **Control plane**: a new crate `loams-sqldb` (records and store, sagas, runtime drivers, renderers) wired into `crates/loams` behind the cargo feature `sqldb`, serving `loams.sqldb.v1`; operations are `loams.operations.v1` operations run as Resonate sagas with deterministic step ids.
- **mywal**: a new crate `loams-mywal` (sans-I/O replication codec and session machines; a tokio service; the `reconcile-local` tool), feature `mywal` in `loams` and a standalone binary `loams-mywal`. Storage through a new crate `loams-walstore`, extracted from `loams-safekeeper` without behaviour change (fenced TiKV 1PC appends, offload, trim, leases), keyspace `loams_mywal`.
- **Gate**: a new crate `loams-sqlgate` (sans-I/O MySQL handshake and framing codec, tokio relay). Packets only, never SQL.
- **`loams-sqlrouter`** gains `Lifecycle`, `PrimaryFailover` (mywal variant) and Vitess `ReshardCutover` machines, trace-validated (D311); never on the query path.
- **Images**: `mysql:8.4.x`, Percona XtraBackup 8.4, Vitess (the pinned release that supports 8.4), etcd, `mysqld_exporter`, by digest in `release/sqldb-images.toml`.

**Tech Stack:** Rust 1.97.1, edition 2024, workspace lints; connect-rust and buffa; `loams-tikv` (the `client-rust` fork, with the `begin_at(ts)` lever of §28 §7.1 if Task 18 needs it); `rustls`; `argon2`; `mysql_common` (MIT/Apache-2.0) for packet and binlog event framing if Task 0 finds its coverage sufficient; `mysql_async` 0.37 for tests; `kube`/`k8s-openapi` (Task 0 pins, `deny.toml`-checked); MySQL Community 8.4.x; XtraBackup 8.4; Vitess; etcd v3.7.x; kind; sysbench 1.0.20, sysbench-tpcc, go-tpc.

**Spec:**
- [§47](../design/47-loams-sql-production.md) (all), D720–D739, Q655–Q669.
- [§28](../design/28-loams-postgres.md) §6.4–§6.7 (the TiKV WAL store, key layout, tuning, latency budget), §7.1 (the laptop results and levers), D234, D237–D240, D269.
- [§31](../design/31-loams-router-and-verification.md) §6, §7, §9, §10, §13–§15, §22; D300–D324, D412.
- [§19](../design/19-console-identity-and-agents.md) §5; [MT1](2026-10-02-mt1-authentik-identity.md); [§44](../design/44-unified-api-and-sdks.md) §4–§7.
- [AP1e](2026-10-08-ap1e-electron-desktop.md) Tasks 9, 21–23, R0.10.
- As built: `crates/loams-safekeeper/src/{store,tikv,acceptor,service}.rs`, `crates/loams-sqlrouter`, `crates/loams-compat`, `conformance/router/`, `spec/tla/router/`, `crates/loams/src/{mysql_wire,api/sql.rs}`, `proto/loams/operations/v1`.
- MySQL 8.4 Reference Manual: semisynchronous replication, the replication protocol, binary log format, crash recovery.

## Global Constraints

- **Worktree and branch.** `~/Documents/Ostriumlabs/loams-wt/sq1-<milestone>`, branch `feat/sq1-<milestone>` from `dev`. `git commit -s`. Commit areas: `sqldb`, `mywal`, `walstore`, `sqlgate`, `sqlrouter`, `proto`, `deploy`, `ci`, `docs`, `desktop`, `plugins`.
- **Licences (D11, D148, D318).** MySQL, XtraBackup and Vitess run only as images and are never linked or vendored. mywal and `reconcile-local` implement MySQL's documented protocols and file formats; no MySQL source is copied or translated into this repository. Every new crate passes `cargo deny`. If a server-side change ever becomes unavoidable, stop and ask the owner: it would go into a GPL fork `ostrium-labs/mysql-server`, outside this repository.
- **The commit rule.** mywal sends a semi-sync ACK for position *p* only after a TiKV transaction covering every byte up to *p* has committed. A PR that acknowledges earlier is wrong, whatever its benchmark says.
- **No asynchronous fallback.** No configuration path may leave `rpl_semi_sync_source_timeout` below the maximum or `rpl_semi_sync_source_wait_no_replica` off on a primary with `durability = mywal`.
- **No SQL parsing** in the gate or in mywal (D300, D731).
- **Secrets** are never in the metastore, logs, rendered ConfigMaps or RPC responses except the one reply that creates them; their Rust types print `"[redacted]"`.
- **Every control-plane mutation is a saga** with a crash-at-every-step test.
- **`loams-safekeeper`'s behaviour does not change.** Its test suite and §28's benchmark harness stay green on every SQ1 PR that touches `loams-walstore`.
- **Rust builds**: the shared target, one build at a time, never `CARGO_TARGET_DIR`, never `/tmp`. Container tests are `#[ignore]` unless `LOAMS_IT_SQLDB=1` (local runtime), `LOAMS_IT_TIKV=1` (local TiKV) or `LOAMS_IT_KIND=1`; CI runs them in the `sqldb` job.
- **Numbers** in §47 §8 and §16 are estimates until Task 1's report; measured numbers go into "Rulings made during execution" and `docs/sqldb/performance.md`.

## Review Focus

1. **An acknowledged commit is lost** — after a primary crash, volume loss, mywal instance kill, TiKV leader kill, failover or suspend. Tests: Task 14 `crash_at_every_point_loses_no_acknowledged_commit`, Task 17 `failover_loses_no_acknowledged_commit`, Task 18's durability run (200 trials), Task 29 `suspend_resume_cycles_lose_nothing`.
2. **mywal ACKs before TiKV is durable**, or a deposed primary's events are acknowledged. Tests: Task 13 `ack_never_precedes_tikv_commit` (fault-injected TiKV), Task 17 `deposed_primary_gets_no_ack`.
3. **Semi-sync silently falls back to asynchronous.** Tests: Task 13 `semi_sync_off_turns_primary_read_only`, `rendered_timeout_is_maximum`.
4. **`reconcile-local` truncates an acknowledged transaction.** Test: Task 14 `reconcile_never_truncates_below_committed_end` (property test).
5. **A client of database A reaches database B**, or a secret leaks. Tests: Task 25 `reader_of_db_a_cannot_select_db_b`, Task 7 `change_user_is_refused`, Task 8 `role_password_returned_once_never_stored`, Task 22 `rendered_configmaps_hold_no_secret`.

---

## File structure

```
proto/loams/sqldb/v1/               database.proto branch.proto role.proto backup.proto sharding.proto
crates/loams-walstore/              extracted from loams-safekeeper: fenced store trait, rules, Mem and TiKV backends, offload, trim, leases
crates/loams-mywal/
  src/codec/{packet,handshake,register,dump,semisync,events}.rs   sans-I/O replication codec
  src/session.rs                    the semi-sync replica session machine (receive → append → ACK)
  src/source.rs                     mywal as a binlog source for readers
  src/stream.rs                     StreamHead, virtual offsets, file map, GTID index
  src/reconcile.rs                  reconcile-local: compare and truncate a local binlog
  src/service.rs  src/bin/loams-mywal.rs
  tests/
crates/loams-sqldb/                 control plane (model, store, sagas, runtime/{local,kubernetes}, render/*, handlers, creds, usage)
crates/loams-sqlgate/               codec/*, auth.rs, server.rs, fuzz/
crates/loams-sqlrouter/src/machines/{lifecycle,primary_failover,vitess_reshard}.rs
spec/tla/router/                    Lifecycle.tla, MyWal.tla (new); PrimaryFailover.tla (mywal variant)
docs/specs/mywal.md                 the stream model, ACK rule, terms, reconcile rule
release/sqldb-images.toml
deploy/sqldb/                       compose (local parity), kind manifests, GitOps base (vtgate, vtctld, etcd, mywal pool), dashboards
scripts/sqldb/bench/                sysbench, tpcc, latency, resume, storm, mywal-gate drivers and the report generator
docs/sqldb/                         user and operator docs
```

## Shared contracts (all tasks use these names)

- **Ids and names**: `db_<16 base32>`, `br_<16 base32>`, `ro_<16 base32>`; role users `u_…`, ephemeral `t_…`, internal `ri_<db_id>_<role>`, replication users `mywal_<shard>`; keyspace `k_<db_id>_<branch_id>`; names `[a-z][a-z0-9-]{0,62}`.
- **States**: `CREATING, RUNNING, SUSPENDING, SUSPENDED, RESUMING, RESIZING, RESTORING, FAILING_OVER, FAILED, DELETING`. **Roles**: `READER, WRITER, DDL, ADMIN`. **Classes**: `XS, S, M, L, XL, XXL`. **Durability**: `MYWAL, LOCAL`.
- **mywal stream**: `stream_id = SHA-256("mysql/" + ns + "/" + db_id + "/" + branch_id + "/" + shard)`; `StreamHead { term, primary_epoch, committed_end, committed_gtids, file_map, backup_end, trim_floor, format_version }`; keys `H‖stream`, `W‖stream‖begin(u64 BE)` (≤ 128 KiB, whole events), `B‖stream` in keyspace `loams_mywal`; bucket `mysqlbinlog/<ns>/<db_id>/<branch_id>/<shard>/<begin>-<end>.lbin`.
- **`reason` values** (appended to `docs/api/reasons.md`): `sqldb_database_not_found`, `sqldb_name_taken`, `sqldb_state_conflict`, `sqldb_quota_exceeded`, `sqldb_point_outside_window`, `sqldb_sharding_beta_disabled`, `sqldb_runtime_unavailable`, `sqldb_durability_unavailable`.
- **Gate errors**: 1045, 3159, 1040 (`database is resuming, retry`; caps), 1235, 1053.
- **`SqlRuntime`**: `ensure_shard`, `stop_shard(keep_volume)`, `start_shard`, `shard_status`, `delete_shard`, `run_job`; `LocalRuntime`, `KubernetesRuntime`, `FakeRuntime`.
- **Internal RPCs** (`loams.internal.v1`): gate ↔ control plane `ResolveUser`, `EnsureRunning`, `ReportActivity`; control plane ↔ mywal `AssignStream(stream, primary_endpoint, term)`, `StreamStatus(stream)`, `BumpTerm(stream, expected_term)`.

---

## SQ1a — Foundations, the control plane API, the desktop

### Task 0: Reconcile with the code and the upstreams

**Files:** this plan's "Rulings made during execution" only. Record as rulings R0.N, with paths and versions:
1. MySQL 8.4: the latest 8.4.x image at least 14 days old; Oracle's 8.4 LTS end-of-support date; the semi-sync plugin and variable names in 8.4 (`semisync_source.so`, `rpl_semi_sync_source_*`, the replica-side `@rpl_semi_sync_replica` user variable); the maximum `rpl_semi_sync_source_timeout`; whether crash recovery rolls back prepared InnoDB transactions whose XIDs are absent from the binlog (manual reading only; Task 14 tests it).
2. Percona XtraBackup 8.4: version pairing with the server, licence, image.
3. Vitess: the newest release at least 14 days old that supports 8.4; every §47 flag marked (verify) for that release.
4. `mysql_common`: does it frame replication packets and the binlog events mywal needs (format description, rotate, GTID, query, XID, heartbeat, transaction payload for compression, checksums)? If not, the codec in Task 11 is hand-written from the manual.
5. `loams-safekeeper`: can `store.rs`'s trait and `tikv.rs` be generic over the head type without changing Postgres behaviour (Task 12's extraction)?
6. Metastore prefixes `x/`, `X/`, `xs/`, `xr/` against `crates/loams-meta-tikv/src/keys.rs`.
7. MT1's verifier state; AP1e's as-built WeSQL paths (`apps/desktop-electron/src/main/{stacks,sql}/*`, `web/plugins/wesql/`).
8. Pins: `kube`, `k8s-openapi`, `argon2`, sysbench, sysbench-tpcc, go-tpc, mydumper, `mysqld_exporter`, etcd, kind.

Commit `docs(sq1): task 0 rulings`.

### Task 1: Spike and baselines (measurement only)

**Files:** `scripts/sqldb/bench/{resume.sh,baseline.sh,semisync-baseline.sh,README.md}`, `docs/sqldb/performance.md`.

Measure on the reference topology (§47 §16) and the build machine:
- MySQL 8.4 baselines for every §47 §8 and §16 workload: (a) `sync_binlog = 1`, no replica; (b) lossless semi-sync to a stock 8.4 replica in another AZ.
- A **raw TiKV append probe** with `loams-safekeeper`'s TiKV store in keyspace `loams_mywal` at 1 KiB–1 MiB batches, with and without TSO reuse, to bound mywal's floor before any mywal code exists.
- Warm and cold resume of MySQL 8.4 (own volume restart; file download of a clean-shutdown datadir) at 1 and 10 GiB, 20 runs each.

*Exit:* measured numbers and a ruling that confirms or replaces §47 §8, §14 and §16's estimates (Q656, Q660). **If the TiKV probe's p99 for a 16 KiB append exceeds 5 ms on PLP NVMe with every lever in, report to the owner before SQ1b starts** (Q666).

### Task 2: Images and pins

**Files:** `release/sqldb-images.toml`, `crates/loams-sqldb/src/images.rs`, tests, `docs/sqldb/licensing.md`.

*Tests:* `images_are_pinned_by_digest`, `mysql_image_is_8_4`, `xtrabackup_matches_server_minor`, `no_gpl_crate_in_dependency_graph` (scoped `cargo deny check licenses` on `loams-sqldb`, `loams-mywal`, `loams-walstore`, `loams-sqlgate`).

### Task 3: `loams.sqldb.v1` protos

**Files:** `proto/loams/sqldb/v1/*.proto`, `crates/loams-proto`, `docs/api/reasons.md`.

**Produces:** §47 §9's services and `Database` message (with `durability` and `stage`), AP0 conventions, `module` option `sqldb`. Q655 "reassign `loams.sql.v1`" changes only the package line here.

*Tests:* `buf lint` clean; `reasons_are_snake_case_and_unique`; `sqldb_protos_mark_reads_no_side_effects`; `sqldb_mutations_carry_idempotency_key`.

### Task 4: Records and the store

**Files:** `crates/loams-sqldb/src/{model,store/*}.rs`, tests.

*Tests:* `sqldb_store_conformance!` over `MemoryStore` and `TikvStore`: `cas_rejects_stale_version`, `names_validate_grammar`, `ids_are_unique_and_prefixed`, `list_pages_are_stable`, `secrets_are_not_fields_of_any_record`.

### Task 5: The runtime trait, the local driver, the `mysqld` config

**Files:** `crates/loams-sqldb/src/runtime/{mod,local}.rs`, `src/render/mysqld.rs`, tests.

**Produces:** `LocalRuntime` running the pinned `mysql:8.4.x` image under Podman or Docker with one volume per shard; `render::mysqld` with §47 §5.1's source settings (semi-sync only when `durability = MYWAL`) and §47 §20's hardening, as a golden file.

*Tests:* `mysqld_config_golden_mywal`, `mysqld_config_golden_local`, `rendered_timeout_is_maximum`, `mysqld_config_disables_local_infile_and_outfile`, `local_runtime_starts_and_stops_a_shard`, `local_runtime_keep_volume_resumes_warm` (`LOAMS_IT_SQLDB=1`).

### Task 6: Lifecycle sagas v0 and the `Lifecycle` machine

**Files:** `crates/loams-sqlrouter/src/machines/lifecycle.rs`, `spec/tla/router/Lifecycle.tla` (+ `MCLifecycle_Small.cfg`), `crates/loams-sqldb/src/sagas/{create,delete,suspend,resume}.rs`, tests.

**Produces:** §47 §14's state machine as a sans-I/O machine with `loams::spec` events; `CreateDatabase`, `DeleteDatabase`, `SuspendDatabase`, `ResumeDatabase` sagas over `SqlRuntime` (mywal steps are no-ops until SQ1b wires them).

*Tests:* `lifecycle_spec_holds_small_bounds` (TLC), `lifecycle_trace_validates`, `create_resumes_after_crash_at_each_step`, `suspend_resumes_after_crash_at_each_step`, `resume_resumes_after_crash_at_each_step`, `concurrent_suspend_and_resume_one_wins`, `ha_database_never_suspends`. `loams-detsim` is RT1's and not on `dev` yet (checked 2026-10-08); until it lands the machine runs under a seeded harness over `machine::Ctx`, and the detsim scenario `lifecycle_under_faults` follows RT1's merge.

### Task 7: The gate v0 (direct to `mysqld`)

**Files:** `crates/loams-sqlgate/**`, `crates/loams/src/{server,main}.rs` (`--sqlgate-listen`, feature `sqldb`).

**Produces:** §47 §12 steps 1–8 with `mysqld` as the upstream (vtgate in SQ1c): TLS, `caching_sha2_password` full auth over TLS against Argon2id hashes with the fast-auth cache, loopback-only plaintext, `ResolveUser`, `EnsureRunning` with a 30 s deadline, capability intersection, relay, refusals, activity reports excluding `COM_PING`, caps, 10 s handshake deadline; fuzz targets `fuzz_handshake_response`, `fuzz_packet_framing`.

*Tests:* `codec_roundtrips_handshake_v10`, `caching_sha2_full_auth_over_tls_succeeds`, `wrong_password_is_1045`, `plaintext_refused_off_loopback`, `change_user_is_refused`, `binlog_dump_is_refused`, `suspended_database_wakes_and_first_query_succeeds`, `resume_deadline_returns_1040`, `handshake_deadline_closes_slow_client`, `connection_cap_returns_1040`, `ping_is_not_activity`, `gate_errors_are_redacted`, `capabilities_never_exceed_upstream`; fuzzers 60 s each in CI.

### Task 8: Handlers in `loams dev`

**Files:** `crates/loams-sqldb/src/{handlers/*,creds}.rs`, `crates/loams/src/api/connect.rs`, `crates/loams/tests/connect_sqldb.rs`.

*Tests:* `create_database_returns_operation_then_running`, `role_password_returned_once_never_stored`, `ephemeral_credential_ttl_capped_at_one_hour`, `ephemeral_credential_expires`, `get_connection_info_has_no_secret`, `instance_lists_sqldb_when_enabled`, `agent_token_cannot_mint_admin_credentials`.

### Task 9: The desktop on `loams.sqldb.v1`

**Files:** `web/plugins/wesql/` (nav name "Loams SQL"), `apps/desktop-electron/src/main/sql/wesql.ts`, `src/main/stacks/stacks.ts`, `buf.gen.apps.yaml` (+ `loams/sqldb/v1`), `web/packages/proto` regenerated, tests. **Coordinate with the AP1e owner first**; change these files only along §47 §18.2's contract.

*Tests:* `uses_sqldb_api_when_available`, `falls_back_to_compose_without_sqldb`, `console_reads_with_reader_credential`, `write_requires_confirm_and_writer_credential`, `agent_tool_gets_reader_only`, `root_password_never_requested`, `resume_button_shows_progress_from_watch`, `durability_local_is_labelled`.

---

## SQ1b — mywal

### Task 10: The mywal specification and its TLA+ model

**Files:** `docs/specs/mywal.md`, `spec/tla/router/MyWal.tla` (+ `MCMyWal_Small.cfg`, `MCMyWal_Nightly.cfg`), `spec/tla/router/specs.toml`.

**Produces:** the stream model (virtual offsets, file map, GTID index), the ACK rule, terms and fencing, the two-session rule, offload and trim, and `reconcile-local`'s rule, written so that every later task cites a section. The TLA+ model: a primary, two mywal sessions, a TiKV register with linearizable CAS, crashes and a term bump; invariants `AckedIsDurable` (every acknowledged position ≤ the stored `committed_end`), `NoAckAtStaleTerm`, `ReconcileNeverDropsAcked`, `ReplicasNeverAhead`.

*Tests:* `mywal_spec_holds_small_bounds` (TLC, PR CI), nightly bounds in the `tla` job; an action-to-code table in the spec header (D311).

### Task 11: The replication codec (sans-I/O)

**Files:** `crates/loams-mywal/src/codec/*.rs`, tests and fuzz targets.

**Produces:** the client side of `COM_REGISTER_REPLICA` and `COM_BINLOG_DUMP_GTID` (with the GTID set encoding), the semi-sync event header and ACK packet, event framing with CRC32 checksums, format-description, rotate, GTID, XID, heartbeat and transaction-payload (compressed) events; the server side of the same dump for readers (Task 15). From the MySQL manual and `mysql_common` where Task 0 allows.

*Tests:* golden packets captured from a real 8.4 server in `tests/fixtures/` (`codec_decodes_captured_dump`, `semisync_header_roundtrip`, `ack_packet_matches_capture`, `gtid_set_encoding_roundtrip`, `checksum_mismatch_is_error`, `compressed_payload_boundaries`); `fuzz_event_framing` 60 s in CI.

### Task 12: `loams-walstore` and the mywal TiKV store

**Files:** `crates/loams-walstore/**` (extracted), `crates/loams-safekeeper` (now depends on it), `crates/loams-mywal/src/stream.rs`, tests.

**Produces:** the store trait generic over the head type; the TiKV backend with the keyspace as a parameter; `MywalStore` over it for `StreamHead` (keys of the shared contracts), idempotent appends by offset, `bump_term`, `record_backup_end`, `trim`. If Task 0 ruled the extraction unsafe, `loams-mywal` depends on `loams-safekeeper`'s store module and the extraction is recorded as follow-up.

*Tests:* every existing `loams-safekeeper` test unchanged and green; `walstore_conformance!` over Mem and TiKV for both head types; `append_at_stale_term_conflicts`, `duplicate_append_is_idempotent`, `two_writers_same_offsets_no_duplicate`, `bump_term_cas`, `trim_keeps_unoffloaded` (`LOAMS_IT_TIKV=1` for TiKV).

### Task 13: The semi-sync replica session, end to end

**Files:** `crates/loams-mywal/src/{session,service}.rs`, `src/bin/loams-mywal.rs`, `crates/loams/src/server.rs` (feature `mywal`), tests.

**Produces:** a session that connects to a primary over TLS as `mywal_<shard>`, registers, enables semi-sync, dumps from the stream's committed GTID set, appends everything received to TiKV in one fenced transaction per batch (group commit; one transaction in flight per stream, batches up to 1 MiB), and ACKs the last durable event; two sessions per primary from different instances; `AssignStream`, `StreamStatus`, `BumpTerm`; the control plane's 1 s `Rpl_semi_sync_source_status` guard.

*Tests (`LOAMS_IT_SQLDB=1 LOAMS_IT_TIKV=1`):* `commit_waits_for_mywal_ack`, `ack_never_precedes_tikv_commit` (TiKV commit delayed and failed by injection), `either_session_ack_suffices`, `killing_one_session_does_not_stall_commits`, `heartbeat_keeps_idle_session_alive`, `rotate_updates_file_map`, `compressed_transactions_are_stored_whole`, `semi_sync_off_turns_primary_read_only`, `no_session_blocks_commits_not_acks_them`.

### Task 14: Crash recovery and `reconcile-local`

**Files:** `crates/loams-mywal/src/reconcile.rs`, the init step in `LocalRuntime` and (later) `KubernetesRuntime`, tests.

**Produces:** §47 §7.2: compare the local binlog with the stream; truncate at `committed_end`'s event boundary, remove later files, rewrite the index; or report "behind" so the runtime restores and catches up from mywal.

*Tests:* `reconcile_never_truncates_below_committed_end` (property test over generated binlogs); on a real 8.4 server with fault points before sync, after sync before ACK, after ACK before engine commit, and after engine commit: `crash_at_every_point_loses_no_acknowledged_commit`, `unacknowledged_prepared_transactions_roll_back_after_truncate`, `volume_loss_recovers_from_snapshot_and_mywal`. These run before any lifecycle code depends on recovery.

### Task 15: mywal as a binlog source; replicas from mywal

**Files:** `crates/loams-mywal/src/source.rs`, `crates/loams-sqldb/src/render/mysqld.rs` (replica settings), tests.

**Produces:** the dump server side from TiKV then the bucket, authenticated read-only readers; replicas configured with `SOURCE_HOST` = the mywal pool, GTID auto-position, `replica_preserve_commit_order`, parallel workers.

*Tests:* `replica_from_mywal_matches_primary_checksum`, `replica_never_ahead_of_committed_end`, `reader_falls_back_to_bucket_after_trim`, `reader_without_rights_is_refused`, `vreplication_reads_from_mywal_or_primary` (records which, per Task 0's ruling).

### Task 16: Offload, retention and RPO

**Files:** `crates/loams-mywal/src/offload.rs` (over `loams-walstore`'s helpers), tests.

*Tests:* `offload_every_250ms_under_load`, `offload_objects_have_crc_and_zstd`, `trim_below_min_backup_and_readers`, `total_tikv_loss_rpo_within_offload_interval` (stop TiKV, recover from the bucket; at most 250 ms of acknowledged commits missing, and the test records the exact count), `bucket_objects_kept_for_pitr_window`.

### Task 17: Fencing and failover through mywal

**Files:** `crates/loams-sqlrouter/src/machines/primary_failover.rs`, `spec/tla/router/PrimaryFailover.tla` (mywal variant), `crates/loams-sqldb/src/sagas/failover.rs`, tests.

**Produces:** §47 §7.3's promotion saga with the term bump first.

*Tests:* `deposed_primary_gets_no_ack`, `failover_loses_no_acknowledged_commit`, `two_promotions_one_wins`, `promotion_resumes_after_control_plane_crash`, `old_primary_rejoins_after_reconcile`, `primary_failover_trace_validates`.

### Task 18: The mywal performance gate

**Files:** `scripts/sqldb/bench/mywal-gate.sh`, `docs/sqldb/performance.md`.

Apply §28 §7.1's levers as needed (TSO reuse via `begin_at(ts)` in the `client-rust` fork, batching, placement rules with the leader in the primary's AZ, raft-engine on its own PLP drive, tuned election ticks), then run every row of §47 §8 against baselines (a) and (b) on the reference topology, three interleaved repeats. *Exit:* the gate passes and the results are committed, **or** the measured results go to the owner under Q666 and SQ1 waits for the answer. GA cannot happen without one of the two.

---

## SQ1c — Vitess routing, auth, TLS

### Task 19: Reconcile Vitess at the pinned release

Confirm every Vitess flag and behaviour §47 marks (verify); confirm that durability policy `none` leaves the semi-sync settings Loams renders untouched and that vttablet's health checks tolerate a semi-sync replica that is not a tablet; record the privileges vttablet's users need. Commit `docs(sq1): task 19 rulings`.

### Task 20: The Kubernetes runtime driver

**Files:** `crates/loams-sqldb/src/runtime/kubernetes.rs`, `src/render/netpol.rs`, `deploy/sqldb/kind/`, tests.

*Tests (`LOAMS_IT_KIND=1`):* `shard_statefulset_has_tablet_sidecar_and_socket_volume`, `init_step_runs_reconcile_local`, `netpol_admits_only_tablet_mywal_and_bucket`, `stop_keep_volume_then_start_is_warm`, `job_runs_and_reports`.

### Task 21: Gate auth, complete

**Files:** `crates/loams-sqlgate/src/auth.rs`, tests.

**Produces:** §47 §13: Loams identities via MT1's verifier; `mysql_clear_password` over TLS; `mysql_native_password` refused unless Q658 says otherwise; upstream as `ri_<db_id>_<role>`; rate limits; per-connection audit.

*Tests:* `jwt_in_password_field_connects_as_role`, `api_key_in_password_field_connects`, `token_for_other_env_is_refused`, `token_scope_caps_role`, `revoked_token_is_refused_within_feed_latency`, `native_password_refused`, `auth_failures_are_rate_limited`, and the per-driver matrix `driver_accepts_long_token_password` (Q659).

### Task 22: Vitess fleet and rendering

**Files:** `deploy/sqldb/vitess/` (vtgate, vtctld, etcd; compose for local parity), `deploy/sqldb/mywal/` (the mywal pool Deployment), `crates/loams-sqldb/src/render/{vschema,static_auth,table_acl,query_rules}.rs`, create-saga steps for keyspaces, tablets and VSchema, the drift reconciler.

*Tests:* `vschema_golden_unsharded`, `vschema_golden_hash_vindex`, `static_auth_holds_only_internal_users`, `table_acl_golden_four_roles`, `query_rules_deny_grant_and_outfile`, `rendered_configmaps_hold_no_secret`, `vschema_drift_is_reapplied_and_alerted`, `create_database_serves_through_vtgate_with_mywal` (`LOAMS_IT_KIND=1`).

### Task 23: TLS on every hop

*Tests:* `plaintext_refused_off_loopback`, `gate_serves_sni_certificate_per_database`, `gate_verifies_vtgate_certificate`, `vtgate_to_vttablet_is_tls`, `mywal_replication_is_tls`, `mywal_to_tikv_is_tls`, `tls_below_1_2_refused`.

### Task 24: Settings and isolation through vtgate

*Tests:* `cross_shard_transaction_is_refused`, `query_timeout_by_class`, `result_size_cap_returns_error`, `buffering_hides_planned_reparent`; InnoDB's isolation through the gate: `serializable_is_honoured`, `gap_lock_blocks_phantom_insert`, `foreign_key_violation_is_1452`, `savepoint_rollback_after_write_works`, `xa_transaction_through_vtgate` (records Vitess's behaviour).

### Task 25: Roles and authorization end to end

*Tests (`LOAMS_IT_KIND=1`):* `reader_cannot_insert`, `writer_cannot_create_table`, `ddl_can_alter_table`, `admin_can_kill_own_connections_only`, `reader_of_db_a_cannot_select_db_b`, `grant_statement_is_refused`, `alter_vschema_through_vtgate_is_refused`, `role_deleted_disconnects_within_a_minute`.

### Task 26: ORM session settings and pooling

Measure which session `SET`s Django, Rails, Laravel and Prisma send, whether vttablet reserves a connection for each, and pool usage at 500 client connections. *Exit:* `docs/sqldb/orm-settings.md` and a ruling.

### Task 27: The Vitess and MySQL 8.4 compatibility gate (RT3, re-targeted)

Re-capture §31's inventory against MySQL 8.4 (`conformance/router/vitess-mysql84-*.tsv` through `crates/loams-compat`) with mywal attached; run the Vitess end-to-end subset. *Exit:* no `error` rows; ≥ 95 % of the reference's pass count, or D317's trigger recorded and the owner told.

### Task 28: Sharding beta

**Files:** `crates/loams-sqlrouter/src/machines/vitess_reshard.rs`, `spec/tla/router/ReshardCutover.tla` (Vitess variant), `crates/loams-sqldb/src/{sagas/reshard,handlers/sharding}.rs`.

**Produces:** `ShardingService` behind `[sqldb] sharding_beta`; one mywal stream per new shard created before `SwitchTraffic`.

*Tests:* `reshard_one_to_two_vdiff_clean`, `new_shards_have_mywal_streams_before_switch`, `reshard_reverse_before_complete`, `ranges_cover_keyspace_before_switch`, `vitess_reshard_trace_validates`, `reshard_resumes_after_crash_at_each_step`.

---

## SQ1d — Serverless operations

### Task 29: Suspend and resume on Kubernetes, through the gate

*Tests:* `idle_database_suspends_after_suspend_after`, `zero_suspend_after_never_suspends`, `suspend_waits_for_mywal_to_match_local_end`, `warm_resume_within_target`, `cold_resume_within_target` (Task 1's targets), `suspend_resume_cycles_lose_nothing` (10 000 cycles, HikariCP, `mysql2`, SQLAlchemy; nightly), `volume_released_after_warm_retention`.

### Task 30: Storage and the mywal pool in the cluster

*Tests:* `shard_keys_cannot_read_other_prefix` (RustFS, S3-compatible), `mywal_sessions_span_two_azs`, `stream_region_leader_in_primary_az` (placement rule rendered and observed), `missing_mywal_marks_durability_unavailable`.

### Task 31: Size classes, tuned

sysbench per class; adjust §47 §11's table in `model.rs` and `docs/sqldb/classes.md`. *Test:* `class_table_matches_docs`.

### Task 32: HA

Replicas as `replica` tablets fed from mywal; health and lease; the Task 17 saga wired into Kubernetes and the gate.

*Tests:* `failover_under_oltp_writable_within_15s`, `failover_through_gate_clients_reconnect`, `cold_replica_catches_up_to_same_checksum`.

### Task 33: Resize

*Tests:* `resize_ha_database_by_replica_and_reparent`, `resize_non_ha_by_suspend_resume`, `resize_during_failover_aborts_cleanly`.

### Task 34: Snapshots

**Produces:** XtraBackup Jobs (full daily, incrementals by time or binlog volume) on a replica when one exists; the clean-shutdown file upload at suspend; snapshot records with their GTID sets.

*Tests:* `snapshot_restores_to_recorded_gtid_set`, `incremental_chain_restores`, `suspend_upload_sends_changed_files_only`, `snapshot_runs_on_replica_when_present`.

### Task 35: PITR and branches

*Tests:* `restore_to_timestamp_matches_recorded_checksum`, `restore_to_gtid_set_matches`, `point_outside_window_is_refused`, `branch_is_independent_of_parent_writes`, `gc_keeps_objects_needed_by_window`, `restore_drill_nightly` (1 GiB, random point).

### Task 36: Logical export

*Tests:* `export_then_myloader_roundtrip_checksum`, `export_uses_replica_or_temporary_branch`.

### Task 37: Quotas and usage events

*Tests:* `volume_quota_turns_read_only`, `branch_quota_enforced`, `usage_events_emitted_per_class`, `new_connection_rate_limited`.

### Task 38: Observability

**Produces:** §47 §20's metrics (gate, control plane, mywal, Vitess, exporter), redacted slow and query logs through OTLP, spans for connect, auth, resume, saga steps and mywal appends, audit events, dashboards and alert rules (semi-sync `OFF` pages).

*Tests:* `gate_and_mywal_metrics_have_documented_names`, `slow_log_entries_are_redacted`, `saga_steps_emit_spans`, `audit_event_per_credential_auth_failure_and_fence`, `alert_rules_lint`.

### Task 39: Upgrades

**Produces:** rolling minor upgrades within 8.4 (replica-first, planned reparent; non-HA at resume or in a window), mywal stream-format versioning, the Vitess upgrade runbook, the next-LTS path as restore-into-branch; Q664's answer after reading the Vitess operator's external-datastore support.

*Tests:* `rolling_upgrade_keeps_writes_available_except_reparent`, `suspended_database_starts_on_new_image`, `mywal_reads_previous_stream_format`, `xtrabackup_upgraded_with_server_minor`.

---

## SQ1e — Evidence, docs and GA

### Task 40: Tier-1 clients and tools in CI

A `sqldb-clients` job with §47 §21's tier-1 list: connect (password and token), prepared statements, transactions, TLS verify-full, streaming, `mysqldump --single-transaction` and `mydumper` round trips. *Exit:* green, or failures filed and allowlisted with causes.

### Task 41: ORM suites

Prisma, Django, Rails and Laravel suites against Loams SQL and stock MySQL 8.4; allowlists in `conformance/sqldb/<orm>.tsv`. *Exit:* each ≥ 98 % of its reference pass count.

### Task 42: Performance gates

`scripts/sqldb/bench/` runs §47 §16 (including §8's mywal gate re-run on the release build) and renders `docs/sqldb/performance.md`; a nightly `sqldb-perf` job on the reference hardware. *Exit:* targets met or the owner's acceptance recorded (Q656, Q660).

### Task 43: Fault evidence

`loams-nemesis` runs of §47 §21 (primary, mywal and TiKV kills, pauses, primary–mywal partitions, clock skew; bank and list-append; unsharded and 2-shard; 2 hours), the 10 000-cycle pool test. *Exit:* clean runs in `docs/sqldb/evidence.md`.

### Task 44: Security review

Every §47 §23 row as a test, 24 h fuzzing per gate and mywal codec target, `cargo deny`, image scans, and a written review by someone other than the implementer in `docs/sqldb/security-review.md`. *Exit:* no open high or critical finding.

### Task 45: Docs

`docs/sqldb/`: quickstart (desktop and cluster), connecting, roles, transactions and durability (mywal explained, the semi-sync guard), limits and classes, serverless behaviour, PITR and branches, ORM guides, runbooks (failover, restore, upgrade, quota, mywal and TiKV incidents), observability, licensing, performance, evidence, migrating from MySQL. *Test:* `docs_examples_run`.

### Task 46: The GA flip

Tick the checklist below with evidence links; flip `Database.stage` to `GA` in the API, console and docs; update the plans README, §47's status and the decision-log statuses the owner approved. Commit `docs(sq1): loams sql ga`.

---

## SQ1f — Engine query SQL (not Loams SQL)

### Task 47: `loams.sql.v1` `SqlService` (API1 Task 5)

**Files:** `proto/loams/sql/v1/sql.proto`, `crates/loams/src/api/connect_sql.rs`, `crates/loams/tests/connect_sql.rs`.

**Produces:** `Query` (unary, capped) and `ExecuteQuery` (server-streaming: schema and `query_id`, then typed JSON rows or Arrow IPC batches ≤ 4 MiB, then `truncated`, the consistency token and timings), `$n` parameters, cancellation on disconnect and by `CancelQuery`. Answers Q608. Flight SQL unchanged.

*Tests:* the ported `native_sql` tests with `_rpc`, `execute_query_streams_arrow_ipc`, `execute_query_json_rows_are_lossless_for_int64`, `client_disconnect_cancels_query`, `cancel_query_by_id`, `flight_sql_still_serves_8082`.

### Task 48: The Data Studio SQL tab on `loams.sql.v1`

**Files:** `web/plugins/data-studio/src/{client.ts,pages/sql.tsx}`, `buf.gen.apps.yaml`, tests. Coordinate with the AP1e owner.

*Tests:* `sql_uses_connect_when_available`, `sql_falls_back_to_rest`, `first_batch_renders_before_trailer`, `cancel_aborts_and_calls_cancel_query`, `sql_error_shown`.

### Task 49: Remove the engine's read-only `mysql-wire` listener

Only after Q663 is answered "remove": delete `crates/loams/src/mysql_wire/`, the feature, the flags, `opensrv-mysql`, doc references; `full` variant loses `mysql-wire` and gains `sqldb` (D286, `api::connect::VARIANT`). *Tests:* `no_mysql_wire_feature`, `variant_full_lists_sqldb`.

### Task 50: Limits on the engine SQL RPC

MT1's verifier (`query` action), `SqlConfig`'s timeout and row cap, a per-query `MemoryPool` limit (default 512 MiB). *Tests:* `sql_rpc_requires_query_scope`, `memory_limit_returns_resource_exhausted`.

---

## Exit criteria for production (all ticked for Task 46)

mywal and durability
- [ ] §47 §8's mywal gate passed on the reference topology (Task 18, re-run in Task 42), or the owner's Q666 choice implemented and its gate passed.
- [ ] TLA+ `MyWal` and `PrimaryFailover` (mywal variant) hold at nightly bounds; trace validation green (Tasks 10, 17).
- [ ] Crash at every point loses no acknowledged commit; `reconcile-local` property test green (Task 14).
- [ ] Total-TiKV-loss RPO measured within the offload interval (Task 16).
- [ ] Semi-sync guard: no primary with `durability = MYWAL` can run asynchronous (Tasks 5, 13).

Engine, routing and security
- [ ] MySQL 8.4 LTS on every production shard (D721); images pinned by digest (Task 2).
- [ ] Vitess/8.4 compatibility gate met (Task 27); `transaction_mode = SINGLE` (Q662).
- [ ] InnoDB isolation, foreign keys, savepoints verified through the gate (Task 24).
- [ ] Gate: TLS-only off loopback, passwords, tokens, API keys, rate limits, audit (Tasks 7, 21, 23); roles and cross-database denial (Task 25).
- [ ] Security review written, no open high or critical finding; 24 h fuzzing clean (Task 44); no GPL code in the dependency graph.

Serverless and operations
- [ ] Warm and cold resume targets met or accepted (Q656); 10 000-cycle pool test clean (Task 29).
- [ ] HA failover ≤ 15 s with zero acknowledged loss (Tasks 17, 32).
- [ ] Snapshots, PITR, branches, export, quotas, usage events, upgrades: all tests green; nightly restore drill green 14 nights in a row (Tasks 34–39).
- [ ] Observability with the semi-sync page alert (Task 38).

Evidence and docs
- [ ] Tier-1 clients green (Task 40); ORM suites ≥ 98 % (Task 41).
- [ ] §47 §16 targets met or accepted (Task 42).
- [ ] Nemesis runs clean (Task 43).
- [ ] `docs/sqldb/` complete, `docs_examples_run` green (Task 45).
- [ ] The desktop's Loams SQL page runs on `loams.sqldb.v1`; the WeSQL compose fallback deleted (Tasks 9, 46).
- [ ] Sharding labelled beta unless Q661 is answered.

## Self-review

- Every §47 decision has tasks: D720/D739 (45–46, 49), D721 (0, 2, 5), D722–D724 (10–13, 16), D725 (14, 17, 32), D726 (1, 18, 42), D727 (34, 35), D728 (3, 8), D729 (4, 22), D730 (6, 22, 28), D731 (7), D732 (21, 23, 25), D733 (6, 29), D734 (31, 33), D735 (19, 24, 28), D736 (20, 22, 30), D737 (5, 9), D738 (37, 38).
- The mywal gate (Task 18) sits before SQ1c so a failing gate is known before the Vitess and serverless work builds on it; SQ1a's local runtime is independent of mywal (`durability = LOCAL`).
- `loams-safekeeper` is protected by its own unchanged suite (Task 12).
- Every Review Focus row names its tests.

## Rulings made during execution

(none yet)
