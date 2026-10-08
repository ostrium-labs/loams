# SQ1 — Loams SQL to Production: Neon-like MySQL on TiDB Compute over Loams TiKV Keyspaces — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first.
> - Each task lists the files it touches, the interfaces it must produce, and the tests that must exist and pass before it is done.
> - Where this plan gives exact values (names, flags, error codes, paths, ports), use them verbatim.
> - Where it gives a contract and named tests, write the code to that contract. Record any deviation in "Rulings made during execution" at the end of this file.
> - The code is not pre-written in this plan.
>
> **Status: Planned** (2026-10-08). **Track SQ** (design [§47](../design/47-loams-sql-production.md), D720–D739, Q655–Q669). **Supersedes** [the MySQL 8.4 + mywal + Vitess plan](2026-10-08-sq1-loams-sql-production.md), whose Task 0 produced §47's rewrite.
>
> **Owner answers in force (2026-10-08):**
> - Q667: GA on TiKV disks with BR snapshots, log backup to S3 and full-copy branches; bottomless storage later.
> - Q662: the S3 track is a spike first, comparing S-A with S-B, with TiDB X as the wait option.
> - Q661 and Q658: TiDB's compatibility (snapshot isolation, no gap locks, no SERIALIZABLE, no stored procedures, triggers or XA, MySQL 8.0-level dialect) is accepted, documented and tested against ORMs.

**Goal:** Loams SQL generally available: a serverless MySQL-compatible database.
- **Compute.** One stateless `tidb-server` pool per branch (TiDB v8.5.8, Apache-2.0) that scales to zero.
- **Storage.** The shared Loams TiKV, with one API v2 keyspace per branch.
- **Front end.** The Rust gate `loams-sqlgate` (TLS, auth, wake, accounting).
- **Management.** The `loams.sqldb.v1` control plane.
- **Backup and branches.** BR snapshot and log backup (PITR) to the bucket, and full-copy branches.
- **CDC.** Into Loams streams.
- **Evidence.** The conformance and performance evidence of §47 §17–§18.

| Milestone | Scope | Tasks | Exit |
|---|---|---|---|
| **SQ1a** | Gate and compute lifecycle with scale-to-zero | 1–6 (6) | Through the gate, a database scales to zero and wakes on connect, p95 ≤ 5 s on the desktop stack |
| **SQ1b** | Control plane, `loams.sqldb.v1`, keyspaces, full-copy branches, GC correctness, Kubernetes driver | 7–14 (8) | `CreateDatabase` and `CreateBranch` produce a branch whose checksum equals the parent's at `point_ts` |
| **SQ1c** | BR backup and PITR to S3 | 15–17 (3) | A branch restored at a point outside the GC window matches the recorded checksum |
| **SQ1d** | CDC to Loams streams | 18–19 (2) | Committed rows appear in Loams streams in commit order |
| **SQ1e** | Desktop single-node | 20–21 (2) | The AP1e Loams SQL page runs on `loams.sqldb.v1`; the WeSQL compose path is deleted |
| **SQ1f** | Conformance and performance gates, observability, security, GA | 22–26 (5) | "Exit criteria for production" ticked |
| **SQ1g** | Engine query SQL (carried over unchanged) | 27–30 (4) | Independent of SQ1a–f |
| **SQ1s** | S3 spike (gated; no production code) | 31–32 (2) | The owner chooses S-A, S-B or waiting for TiDB X (Q662) |

**Architecture** (§47 §3):
- **Clients to compute.** Clients connect over TLS to `loams-sqlgate`, which talks to the branch's `tidb-server` pool (`keyspace-name = <branch_id>`). The pool reaches PD and TiKV through client-go.
- **Control plane.** It lives in `crates/loams-sqldb` behind the `loams` cargo feature `sqldb`. It covers the records, keyspace lifecycle through PD's HTTP API, the runtime drivers (`local`, `kubernetes`), renderers, the branch copier, the BR job runner and the handlers.
- **Machines.** `crates/loams-sqlrouter` gains `Lifecycle` and `BranchCopy` machines (sans-I/O, trace-validated against TLA+, D311). Sagas run on Resonate with deterministic step ids.
- **GC.** `crates/loams-tikv`'s GC loop stays the cluster GC worker and learns TiDB's registrations (Task 10).
- **CDC.** `crates/loams-sqlcdc` is a Rust consumer of TiKV's CDC stream.

**Tech stack:**
- **Rust:** Rust 1.97.1, edition 2024, workspace lints. connect-rust and buffa; `loams-tikv` (the `client-rust` fork, which carries `cdcpb`); `rustls`; `argon2` 0.6.0; `kube` 4.2.0 with `k8s-openapi` 0.28.0 (feature `v1_36`); `mysql_async` 0.37 for tests; an etcd v3 client crate for PD's etcd API (Task 10 picks it and checks it with `cargo deny`).
- **Images by digest** (§47 §4): `pingcap/{tidb,tikv,pd,br}:v8.5.8`.
- **Tools:** sysbench 1.0.20, go-tpc v1.0.12, kind v0.33.0 with node v1.36.4.

**Spec:**
- [§47](../design/47-loams-sql-production.md) (all), D720–D739, Q655–Q669.
- [§20](../design/20-reactive-database-on-tikv.md) §9.3 (GC) and §10 (keyspace-mode TiDB), with `docs/plans/r1-dependency-spike.md`.
- [§31](../design/31-loams-router-and-verification.md) §11–§15 (verification method, D309).
- [§19](../design/19-console-identity-and-agents.md) §5; [MT1](2026-10-02-mt1-authentik-identity.md); [§44](../design/44-unified-api-and-sdks.md) §4–§7.
- [AP1e](2026-10-08-ap1e-electron-desktop.md) Tasks 9, 21–23.
- Upstream source, read-only, in `~/Documents/Ostriumlabs/{tidb,tikv,pd,rust-rocksdb,ticdc,tiproxy}` (§47 §23).

## Global Constraints

- **Worktree and branch.** `~/Documents/Ostriumlabs/loams-wt/sq1` (branch `backend/sq1`) unless the coordinator assigns another. Use `git commit -s`; PRs target `dev`. Commit areas: `sqldb`, `sqlgate`, `sqlcdc`, `sqlrouter`, `tikv`, `proto`, `deploy`, `ci`, `docs`, `desktop`, `plugins`.
- **Licences (D11, D126).** TiDB, TiKV, PD and BR run as images; nothing Go is linked. Every new crate passes `cargo deny`. GPL tools (sysbench, mydumper) run only as tools or images in benchmarks.
- **No SQL parsing** in the gate or the CDC consumer, beyond decoding TiDB's row format (D731).
- **Secrets** never appear in the metastore, logs, rendered config or RPC responses, except in the one reply that creates them. Their Rust types print `"[redacted]"`.
- **Every control-plane mutation is a saga** with a crash-at-every-step test.
- **No TiDB without `keyspace-name`** anywhere (D260 as amended; one GC worker, Loams').
- **The upstream clones are read-only.** Run every git command in them with `GIT_NO_LAZY_FETCH=1`. Version-specific files are read at tags from `raw.githubusercontent.com`.
- **Rust builds** use the shared target, one build at a time. Never set `CARGO_TARGET_DIR` and never build in `/tmp`.
- **Gated tests.** Container tests are `#[ignore]` unless one of these is set; CI runs them in the `sqldb` job:
  - `LOAMS_IT_SQLDB=1`: the spike or desktop stack plus TiDB;
  - `LOAMS_IT_TIKV=1`: TiKV only;
  - `LOAMS_IT_KIND=1`;
  - `LOAMS_IT_S3=1`: RustFS.
- **Ports.** Never 5180 or 8090. The SQL spike stack uses its own ports (Task 1) so it can run beside the desktop's `deploy/tikv` stack.
- **Numbers** in §47 §5.2, §6.3 and §18 are estimates until Task 1 and Task 24 replace them in "Rulings made during execution" and `docs/sqldb/performance.md`.

## Review Focus

1. **A read below the GC safe point returns wrong data silently,** or GC aborts a live transaction. Tests: Task 10 `long_transaction_holds_safe_point`, `saved_safe_point_written_and_honoured`, `stale_read_below_safe_point_errors`.
2. **A branch copy is not a consistent snapshot.** Tests: Task 13 `copy_branch_checksum_equals_parent_at_point`, `copy_resumes_after_crash_at_each_step`.
3. **A client of database A reaches database B,** or a secret leaks. Tests: Task 4 `user_of_db_a_cannot_reach_db_b`, Task 11 `role_password_returned_once_never_stored`, Task 2 `rendered_config_holds_no_secret`.
4. **Wake-on-connect loses or misroutes a connection.** Tests: Task 5 `suspended_database_wakes_and_first_query_succeeds`, `concurrent_suspend_and_connect_one_wins`.
5. **A restore lands in the wrong keyspace or overwrites one.** Tests: Task 16 `restore_into_new_keyspace_matches_checksum`, `restore_never_targets_existing_keyspace`.

---

## File structure

```
proto/loams/sqldb/v1/                 database.proto branch.proto role.proto backup.proto
crates/loams-sqldb/                   control plane
  src/{model,ids,images}.rs
  src/store/{mod,memory,tikv}.rs      records m/ mn/ M/ mr/
  src/pd.rs                           PD keyspace HTTP client
  src/render/{tidb,netpol}.rs         tidb.toml, NetworkPolicy
  src/runtime/{mod,local,kubernetes,fake}.rs
  src/sagas/{create,delete,suspend,resume,branch,backup,restore}.rs
  src/copier.rs                       full-copy branches over loams-tikv
  src/br.rs                           BR job runner
  src/handlers/*.rs  src/creds.rs
crates/loams-sqlgate/                 codec/*, auth.rs, server.rs, migrate.rs, fuzz/
crates/loams-sqlcdc/                  regions.rs, resolved.rs, rowcodec.rs, schema.rs, sink.rs
crates/loams-sqlrouter/src/machines/{lifecycle,branch_copy}.rs
crates/loams-tikv/src/gc.rs           + TiDB min-start-ts and the saved safe point (Task 10)
spec/tla/router/                      Lifecycle.tla, BranchCopy.tla (+ MC*.cfg)
release/sqldb-images.toml
deploy/sqldb/spike/                   own-port PD + TiKV + TiDB stack for Task 1 and IT tests
deploy/sqldb/kind/                    kind manifests
scripts/sqldb/spike/                  Task 1 measurement scripts
scripts/sqldb/bench/                  Task 24 drivers and report generator
conformance/sqldb/                    contract and ORM allowlists (*.tsv)
docs/sqldb/                           user and operator docs, performance.md, compatibility.md
```

## Shared contracts (all tasks use these names)

- **Ids and names.**
  - Ids: `db_<16 base32>`, `br_<16 base32>`, `ro_<16 base32>`.
  - Role users: `u_…`; ephemeral `t_…`.
  - Internal TiDB users per branch keyspace: `ri_reader`, `ri_writer`, `ri_ddl`, `ri_admin`, and the control plane's `ri_control`.
  - Database names: `[a-z][a-z0-9-]{0,62}`.
- **Keyspace name** = the branch id (`br_…`, 19 characters; PD master limits names to `^[-A-Za-z0-9_]{1,20}$`).
- **States.** `CREATING, RUNNING, SUSPENDING, SUSPENDED, RESUMING, BRANCHING, RESTORING, FAILED, DELETING`.
- **Roles.** `READER, WRITER, DDL, ADMIN`.
- **Classes.** `XS, S, M, L, XL, XXL`.
- **Records.** `m/<ns u64 BE>/<db_id>`, `mn/<ns>/<name>`, `M/<ns>/<db_id>/<branch_id>`, `mr/<ns>/<db_id>/<role_id>`.
- **`reason` values** (appended to `docs/api/reasons.md`): `sqldb_database_not_found`, `sqldb_name_taken`, `sqldb_state_conflict`, `sqldb_quota_exceeded`, `sqldb_point_outside_window`, `sqldb_runtime_unavailable`, `sqldb_branch_too_large_for_window`.
- **Gate errors.** 1045 (auth), 3159 (TLS required), 1040 (`database is resuming, retry`; caps), 1235 (`COM_CHANGE_USER`), 1053 (shutdown).
- **`SqlRuntime`**: `ensure_pool(branch, class, replicas)`, `scale(branch, n)`, `pool_status(branch)`, `delete_pool(branch)`, `run_job(spec)`, with `LocalRuntime`, `KubernetesRuntime` and `FakeRuntime`.
- **Internal RPCs** (`loams.internal.v1`), gate ↔ control plane: `ResolveUser`, `EnsureRunning`, `ReportActivity`.
- **Bucket layout.** `sqlbackup/<ns>/<db_id>/<branch_id>/{snap/<ts>/,log/}`.

---

## SQ1a — Gate and compute lifecycle

### Task 0: Study and rulings (done)
§47 rewritten, the decision log updated, the owner's answers recorded, and the source questions answered (§47 §23). Commits `docs: SQ1 task 0 — Neon-like TiDB direction` and `docs(sq1): owner answers, source findings and the TiDB plan`.

### Task 1: Compute baselines and pins (measurement only; runnable now, no Rust build)

**Files:**
- `release/sqldb-images.toml`: the v8.5.8 digests of §47 §4.
- `deploy/sqldb/spike/{compose.yaml,pd.toml,tikv.toml,tidb.toml.tmpl}`.
- `scripts/sqldb/spike/{up.sh,down.sh,keyspace.sh,coldstart.sh,regions.sh,report.sh,README.md}`.
- `docs/sqldb/performance.md`.
- This plan's rulings.

**The stack.**
- Compose project `loams-sqldb-spike` under Podman (`podman compose`) or Docker, host networking.
- Ports: PD client `127.0.0.1:29379`, peer `29380`; TiKV `127.0.0.1:30160`, status `30180`. TiDB containers get `24000+N` (MySQL) and `25000+N` (status).
- Configs copied from `deploy/tikv/{pd.toml,tikv.toml}` (API v2, TTL) with the ports changed and no pre-allocated keyspaces. `tidb.toml.tmpl` takes `keyspace-name`, `split-table` and `force-init-stats` as variables.
- Images pinned by digest from `release/sqldb-images.toml`.
- Volumes are named, and `down.sh -v` removes them.

**Measurements** (each script writes JSON lines to `target-spike/` under the worktree, which is added to `.gitignore`, and `report.sh` renders `docs/sqldb/performance.md`):
1. `keyspace.sh`: latency of `POST /pd/api/v2/keyspaces` (20 creates) and of `PUT /pd/api/v2/keyspaces/{name}/state` to `DISABLED` and `ARCHIVED`.
2. `coldstart.sh`: `tidb-server` start to the first `SELECT 1` answered through `mysql -h127.0.0.1 -P24000 -uroot`, split into container start, port open and first query.
   - (a) First bootstrap on a fresh keyspace, 5 runs.
   - (b) Warm restart with 0, 100 and 1 000 tables: 20 runs at 0 and 100 tables, 5 at 1 000.
   - (c) Each with `force-init-stats = true` (the v8.5.8 default) and `false`.
   - Also record the RSS (`podman stats`) idle and after `sysbench oltp_read_write` for 60 s on 10 tables × 10 000 rows, if sysbench is installed; otherwise a `mysql`-driven insert loop.
3. `regions.sh`: regions in the keyspace's range after bootstrap with `split-table = true` and with `false` (PD `GET /pd/api/v1/regions/key?…` over the keyspace's `x` prefix range). Then create and bootstrap 20, 50 and 100 keyspaces sequentially with `split-table = false`, recording the total region count, PD and TiKV RSS, and TiKV's `raftstore` heartbeat metrics.

**Exit:**
- `docs/sqldb/performance.md` holds the medians and p95s.
- Rulings R1.N confirm or replace §47 §5.2 (cold start), §5.1 (`force-init-stats = false`) and §6.3 (regions per empty database).
- `shellcheck scripts/sqldb/spike/*.sh` is clean.
- `down.sh -v` leaves no container, volume or listening port.

**Needs:**
- Podman (or Docker) with compose, the `mysql` client, `curl`, `jq`; optionally `sysbench`.
- About 6 GB of free RAM, about 2 GB for images, and the ports above.
- 1–2 hours of wall time.
- It must not touch `deploy/tikv`'s running desktop stack or ports 5180/8090.

Commit `bench(sqldb): compute baselines and image pins`.

### Task 2: Images, `tidb.toml` rendering and the runtime trait

**Files:** `crates/loams-sqldb/{Cargo.toml,src/{lib,images,model}.rs,src/render/tidb.rs,src/runtime/{mod,local,fake}.rs}`, tests. `docs/sqldb/licensing.md`.

**Interfaces:**
- `Images::load()` reads `release/sqldb-images.toml`.
- `render::tidb(branch, class, endpoints) -> String`:
  - `keyspace-name`, `path` (PD), `split-table = false`, `[performance] force-init-stats = false`, `lite-init-stats = true`;
  - `server-version = "8.0.11-TiDB-v8.5.8-Loams"`;
  - `[proxy-protocol] networks` = the gate CIDR, `fallbackable = false`;
  - `[security]` TLS paths, `enable-global-kill = true`, memory limits by class;
  - `[log]` without slow-log literals (`tidb_redact_log = MARKER`).
- `SqlRuntime` (shared contracts); `LocalRuntime` runs one `pingcap/tidb@digest` container per pool member under Podman or Docker; `FakeRuntime` is for sagas.

Tests:
- `images_are_pinned_by_digest`, `tidb_image_is_v8_5`.
- `tidb_config_golden_xs`, `tidb_config_golden_xl`.
- `rendered_config_holds_no_secret`.
- `config_never_omits_keyspace_name` (property: every rendered config has a non-empty `keyspace-name`).
- `local_runtime_starts_and_stops_a_pool` (`LOAMS_IT_SQLDB=1`).
- `no_gpl_crate_in_dependency_graph` (scoped `cargo deny check licenses`).

Commit `feat(sqldb): images, tidb config rendering and the local runtime`.

### Task 3: The gate codec (sans-I/O)

**Files:** `crates/loams-sqlgate/{Cargo.toml,src/codec/{packet,handshake,auth,command}.rs}`, `fuzz/`.

**Interfaces:**
- MySQL packet framing (16 MiB splits, sequence ids).
- `HandshakeV10` encode; `SSLRequest` and `HandshakeResponse41` decode with capability intersection.
- `caching_sha2_password` fast and full auth exchanges and `AuthSwitchRequest`.
- ERR/OK packets; command classification by first byte only (`COM_PING`, `COM_QUIT`, `COM_CHANGE_USER`, `COM_BINLOG_DUMP*`, `COM_REGISTER_SLAVE`, others).

Tests:
- `codec_roundtrips_handshake_v10`.
- `handshake_response_decodes_captured_clients` (fixtures captured from `mysql` 8.4, `mysql2`, Connector/J, go-sql-driver against TiDB in Task 1's stack).
- `packet_split_at_16mib`.
- `capabilities_never_exceed_upstream`.
- `fuzz_handshake_response` and `fuzz_packet_framing` for 60 s each in CI.

Commit `feat(sqlgate): sans-io mysql handshake and framing`.

### Task 4: The gate server

**Files:** `crates/loams-sqlgate/src/{server,auth,upstream,limits}.rs`, `crates/loams/src/{server,main}.rs` (`--sqlgate-listen`, feature `sqldb`), tests.

**Interfaces:**
- **Listener and TLS.** A tokio listener with `rustls` TLS 1.2+ and SNI certificates. Plaintext only on loopback.
- **Identity.** `ResolveUser(user) -> {branch, role, argon2 hash}`, with the fast-auth cache keyed by user and SHA-256 of the password.
- **Upstream.** Connect to one pool member over TLS with a PROXY v2 header, as `ri_<role>` with the internal password from the credential store.
- **Relay.** Relay with refusals (`COM_CHANGE_USER` → 1235; replication commands → 1235).
- **Limits.** Per-database connection cap and rate (1040), a 10 s handshake deadline, `ReportActivity` counting every command except `COM_PING`.

Tests:
- `caching_sha2_full_auth_over_tls_succeeds`, `wrong_password_is_1045`, `plaintext_refused_off_loopback`.
- `change_user_is_refused`, `binlog_dump_is_refused`.
- `handshake_deadline_closes_slow_client`, `connection_cap_returns_1040`, `ping_is_not_activity`.
- `gate_errors_are_redacted`.
- `tidb_sees_client_address_via_proxy_protocol`.
- `user_of_db_a_cannot_reach_db_b` (`LOAMS_IT_SQLDB=1`).

Commit `feat(sqlgate): tls, auth, relay and limits`.

### Task 5: The `Lifecycle` machine, suspend and resume, wake on connect

**Files:** `crates/loams-sqlrouter/src/machines/{mod,lifecycle}.rs`, `spec/tla/router/{Lifecycle.tla,MCLifecycle_Small.cfg}`, `spec/tla/router/specs.toml`, `crates/loams-sqldb/src/sagas/{suspend,resume}.rs`, `crates/loams-sqldb/src/idle.rs`, tests.

**Interfaces:**
- §47 §14's states as a sans-I/O machine with `loams::spec` events.
- **Suspend saga:** drain at the gate (new connections held), close idle sessions (1053), wait up to 30 s for transactions, `scale(branch, 0)`.
- **Resume saga:** `scale(branch, n)`, wait for the port and a probe `SELECT 1`, release held connections.
- `EnsureRunning` with a 30 s deadline.
- The idle detector reads `ReportActivity` against `suspend_after`.

Tests:
- `lifecycle_spec_holds_small_bounds` (TLC), `lifecycle_trace_validates`.
- `suspend_resumes_after_crash_at_each_step`, `resume_resumes_after_crash_at_each_step`.
- `concurrent_suspend_and_connect_one_wins`.
- `zero_suspend_after_never_suspends`, `idle_database_suspends_after_suspend_after`.
- `suspended_database_wakes_and_first_query_succeeds` and `resume_deadline_returns_1040` (`LOAMS_IT_SQLDB=1`).
- `resume_p95_under_5s_on_spike_stack` (`LOAMS_IT_SQLDB=1`, 20 cycles, records the numbers).

Commit `feat(sqldb): lifecycle machine, suspend and resume, wake on connect`.

### Task 6: Idle-session migration (graceful compute replacement)

**Files:** `crates/loams-sqlgate/src/migrate.rs`, tests.

**Interfaces:**
- A session with no open transaction can move to another pool member, for scale-in, rolling upgrade or pod replacement.
- The gate runs `SHOW SESSION_STATES` on the old member, connects to the new one as the same `ri_<role>` (no TiDB session token needed), runs `SET SESSION_STATES '<json>'`, then switches the relay.
- Sessions inside a transaction, or holding prepared-statement cursors open, are drained instead.

Tests:
- `idle_session_survives_member_replacement` (variables, current database and prepared statements preserved).
- `session_in_transaction_is_not_migrated`.
- `migration_failure_closes_with_1053` (`LOAMS_IT_SQLDB=1`).

Commit `feat(sqlgate): migrate idle sessions between tidb members`.

---

## SQ1b — Control plane, API, keyspaces, full-copy branches

### Task 7: `loams.sqldb.v1` protos

**Files:** `proto/loams/sqldb/v1/{database,branch,role,backup}.proto`, `crates/loams-proto`, `docs/api/reasons.md`.

**Produces:**
- §47 §11's services, with the `module` option `sqldb` and AP0 conventions.
- `Database` with `stage`.
- `Branch` with `branch_kind` (`COPY`; `COW` reserved) and `point_ts`.
- `GetRestoreWindow` → `{gc_window_start, backup_window_start, now}`.

Tests: `buf lint` clean; `reasons_are_snake_case_and_unique`; `sqldb_protos_mark_reads_no_side_effects`; `sqldb_mutations_carry_idempotency_key`.

Commit `feat(proto): loams.sqldb.v1`.

### Task 8: Records and the store

**Files:** `crates/loams-sqldb/src/{ids,model}.rs`, `src/store/{mod,memory,tikv}.rs`, tests.

**Interfaces:** the records of the shared contracts (postcard behind a format byte, CAS on `version`); `SqldbStore` with `MemoryStore` and `TikvStore` (keyspace `loams_meta`).

Tests:
- `sqldb_store_conformance!` over both stores: `cas_rejects_stale_version`, `names_validate_grammar`, `ids_are_unique_and_prefixed`, `list_pages_are_stable`, `secrets_are_not_fields_of_any_record`.
- `sqldb_prefixes_do_not_collide` (against `crates/loams-meta-tikv/src/keys.rs` and PG2's `x/ X/ C/ E/ R/ D/`).

Commit `feat(sqldb): records and store`.

### Task 9: Keyspace lifecycle and bootstrap at create

**Files:** `crates/loams-sqldb/src/{pd.rs,sagas/{create,delete}.rs}`, tests.

**Interfaces:**
- `PdKeyspaces`: `create(name)`, `get`, `set_state(DISABLED|ARCHIVED|TOMBSTONE)`, over PD's `/pd/api/v2/keyspaces`.
- **Create saga:** record `CREATING`, then the keyspace, then `ensure_pool(1)` with the bootstrap waited on (TiDB's `mysql.tidb` `bootstrapped` row read through `ri_control`), then the internal users (Task 11), then `scale(0)` unless `min_class` keeps it warm.
- **Delete saga:** pool → 0, keyspace `DISABLED` → `ARCHIVED`, the GC loop's range destroy (Task 10), then `TOMBSTONE`.

Tests:
- `create_resumes_after_crash_at_each_step`, `delete_resumes_after_crash_at_each_step`.
- `keyspace_name_is_branch_id_and_valid_for_pd_master` (≤ 20 characters).
- `create_database_bootstraps_once` (`LOAMS_IT_SQLDB=1`).
- `deleted_keyspace_range_is_empty_after_gc` (`LOAMS_IT_SQLDB=1`).

Commit `feat(sqldb): keyspace lifecycle and bootstrap`.

### Task 10: The GC loop learns TiDB's registrations

**Files:** `crates/loams-tikv/src/{gc.rs,etcd.rs}`, `crates/loams-tikv/tests/gc_tidb.rs`.

**Interfaces** (§47 §7.2; v8.5.8 behaviour read from source):
- Each round reads every key under PD etcd `/tidb/server/minstartts/` and caps the target at `min - 1` when `min` is newer than `now - GCMaxWaitTime` (24 h).
- After `UpdateGCSafePoint` it writes the safe point to client-go's `GcSavedSafePoint` key `/tidb/store/gcworker/saved_safe_point` in PD's etcd (Task 10 confirms the exact key in the client-go version that TiDB v8.5.8 pins, from `raw.githubusercontent.com`).
- It runs `UnsafeDestroyRange` for keyspaces in `ARCHIVED`.
- It drives delete-ranges for suspended keyspaces: either by waking the pool (`EnsureRunning` with a GC reason) or by applying `mysql.gc_delete_range` itself, whichever Task 10's source reading of the delete-range table rules safe.

Tests (`LOAMS_IT_SQLDB=1`):
- `long_transaction_holds_safe_point` (an open `BEGIN; SELECT` in a keyspace TiDB survives 3 GC rounds with `gc_life_time` = 1 min).
- `saved_safe_point_written_and_honoured`.
- `stale_read_below_safe_point_errors` (`SET tidb_snapshot` below the safe point returns TiDB's GC error, not data).
- `archived_keyspace_range_destroyed`.
- `delete_range_runs_for_suspended_keyspace`.

Commit `feat(tikv): gc loop honours tidb min start ts and saves the safe point`.

### Task 11: Roles and credentials

**Files:** `crates/loams-sqldb/src/{creds.rs,sagas/roles.rs}`, tests.

**Interfaces:**
- Role passwords: 32 random bytes in base62, an Argon2id hash in the credential store, returned once.
- The internal users `ri_*` are created in the branch's keyspace through `ri_control` with §47 §13.1's grants.
- Ephemeral credentials last at most 1 h.
- A copy branch re-keys its internal users at creation, because the parent's `mysql.user` comes along with the copy.

Tests:
- `role_password_returned_once_never_stored`, `ephemeral_credential_ttl_capped_at_one_hour`, `ephemeral_credential_expires`.
- `reader_cannot_insert`, `writer_cannot_create_table`, `ddl_can_alter_table`, `admin_can_kill_own_connections_only`.
- `grant_and_create_user_refused_for_app_roles`.
- `branch_copy_rekeys_internal_users` (`LOAMS_IT_SQLDB=1`).

Commit `feat(sqldb): roles, credentials and grants`.

### Task 12: Handlers in `loams dev`

**Files:** `crates/loams-sqldb/src/handlers/*.rs`, `crates/loams/src/api/connect.rs`, `crates/loams/tests/connect_sqldb.rs`.

Tests:
- `create_database_returns_operation_then_running`.
- `get_connection_info_has_no_secret`.
- `instance_lists_sqldb_when_enabled`.
- `agent_principal_cannot_mint_admin_credentials` (a test principal behind a `Principal` seam until MT1).
- `watch_database_streams_state_changes`.

Commit `feat(sqldb): loams.sqldb.v1 handlers in loams dev`.

### Task 13: Full-copy branches

**Files:** `crates/loams-sqldb/src/{copier.rs,sagas/branch.rs}`, `crates/loams-sqlrouter/src/machines/branch_copy.rs`, `spec/tla/router/{BranchCopy.tla,MCBranchCopy_Small.cfg}`, tests.

**Interfaces:**
- `CreateBranch(parent, point)`: `point` is `latest`, a timestamp or a TSO, inside the GC window (outside it, Task 16 serves it).
- The copier holds a service safe point at the point, with a TTL refreshed while it runs. It reads the parent keyspace at that ts in key order and writes the child keyspace in 1PC batches of ≤ 1 MiB, with key-prefix rewrite. Progress is checkpointed in the `M/` record, so a restart resumes from the last committed key.
- Copies whose estimated time exceeds the GC window minus a margin are refused with `sqldb_branch_too_large_for_window` and pointed to the BR path.

Tests:
- `branch_copy_spec_holds_small_bounds`, `branch_copy_trace_validates`.
- `copy_resumes_after_crash_at_each_step`.
- `copy_branch_checksum_equals_parent_at_point` (`ADMIN CHECKSUM TABLE` on every table; `LOAMS_IT_SQLDB=1`).
- `branch_is_independent_of_parent_writes`.
- `copy_releases_service_safe_point`.
- `copier_throughput_recorded` (writes to `docs/sqldb/performance.md`).

Commit `feat(sqldb): full-copy branches`.

### Task 14: The Kubernetes runtime driver

**Files:** `crates/loams-sqldb/src/runtime/kubernetes.rs`, `src/render/netpol.rs`, `deploy/sqldb/kind/`, tests.

**Interfaces:**
- Per branch: a Deployment (replicas 0..n), a Service, a TLS Secret and a NetworkPolicy (gate → TiDB; TiDB → PD and TiKV only); `run_job` for BR.
- PD and TiKV come from `deploy/sqldb/kind/` (tidb-operator v2 without `TiDBGroup`, or plain manifests, per Q664).

Tests (`LOAMS_IT_KIND=1`):
- `scale_zero_then_one_serves`.
- `netpol_admits_only_gate_and_store`.
- `job_runs_and_reports`.
- `image_pre_pulled_on_every_node`.

Commit `feat(sqldb): kubernetes runtime`.

---

## SQ1c — BR backup and PITR to S3

### Task 15: Snapshot and log backup per branch

**Files:** `crates/loams-sqldb/src/{br.rs,sagas/backup.rs}`, `deploy/sqldb/spike/rustfs.yaml` (RustFS for tests), tests.

**Interfaces:**
- BR (`pingcap/br@digest`) runs as a job: `br backup full --keyspace-name <branch_id> --storage s3://…/snap/<ts>/` daily, and `br log start --keyspace-name …` per branch into `log/`.
- Task status is read with `br log status`. Each backup's ts and checksum are recorded.

Tests (`LOAMS_IT_SQLDB=1 LOAMS_IT_S3=1`):
- `snapshot_backup_lands_under_branch_prefix`.
- `log_backup_checkpoint_advances`.
- `backup_job_resumes_after_crash`.
- `log_backup_lag_metric_exported`.

Commit `feat(sqldb): br snapshot and log backup per branch`.

### Task 16: Restore into a new keyspace (PITR branch)

**Files:** `crates/loams-sqldb/src/sagas/restore.rs`, tests.

**Interfaces:**
- `CreateBranch(parent, point)` with `point` older than the GC window does this: create the child keyspace, then `br restore point --keyspace-name <child> --full-backup-storage … --storage …/log/ --restored-ts <point>`. BR v8.5.8 rewrites the old keyspace prefix to the target's (`restore.go`, `RewriteModeKeyspace`; §47 §23).
- **Plan B** if the test fails: restore into a scratch keyspace, then the Task 13 copier.

Tests (`LOAMS_IT_SQLDB=1 LOAMS_IT_S3=1`):
- `restore_into_new_keyspace_matches_checksum`.
- `restore_never_targets_existing_keyspace`.
- `point_outside_window_is_refused`.
- `restore_resumes_after_crash_at_each_step`.

Commit `feat(sqldb): pitr restore into a new branch`.

### Task 17: Window, retention and drills

**Files:** `crates/loams-sqldb/src/{window.rs,sagas/retention.rs}`, `scripts/sqldb/restore-drill.sh`, tests.

**Interfaces:**
- `GetRestoreWindow`.
- Retention: 7 days by default, 1–35 (Q665). `br log truncate` and snapshot expiry keep every object the window needs.
- Optional S3 Object Lock on the prefix.
- A nightly restore drill: 1 GiB at a random point.

Tests: `gc_keeps_objects_needed_by_window`, `window_reports_both_bounds`, `restore_drill_nightly` (nightly job).

Commit `feat(sqldb): restore window, retention and drills`.

---

## SQ1d — CDC to Loams streams

### Task 18: CDC spike and ruling

**Files:** this plan's rulings, `docs/sqldb/cdc.md`.

**Work:**
- (a) Confirm that classic TiCDC v8.5.8 cannot follow a keyspace (§47 §10, from `ticdc` source) by running it against the spike stack.
- (b) Prototype a Rust subscription of one region of a keyspace with `cdcpb.ChangeData` (`kv_api = TiDb`) through `loams-tikv`'s `client-rust` fork.
- (c) Record whether TiDB's row codec v2 and schema decoding from meta keys cover the types of §47 §13.2.

*Exit:* a ruling that fixes `loams-sqlcdc`'s design, or names the blocker.

Commit `docs(sq1): task 18 cdc ruling`.

### Task 19: `loams-sqlcdc`

**Files:** `crates/loams-sqlcdc/**`, `crates/loams/src/server.rs` (feature `sqldb-cdc`), tests.

**Interfaces:**
- Region tracking over the keyspace range.
- Resolved-ts merge.
- Row decoding (row codec v2, record keys `t<id>_r<handle>`, with schema versions from meta keys).
- One Loams stream per table, at-least-once with idempotent writes keyed by `(commit_ts, key)`.
- Per-branch enable through `UpdateDatabase`.

Tests (`LOAMS_IT_SQLDB=1`):
- `inserts_updates_deletes_arrive_in_commit_order`.
- `ddl_add_column_is_decoded_after_schema_change`.
- `consumer_restart_resumes_from_checkpoint`.
- `region_split_and_leader_change_lose_nothing`.
- `rowcodec_roundtrip_all_types` (fixtures captured from TiDB).

Commit `feat(sqlcdc): tidb keyspace changes into loams streams`.

---

## SQ1e — Desktop single-node

### Task 20: `loams dev` with `sqldb` on the local TiKV stack

**Files:**
- `deploy/tikv/compose.yaml`: `TIKV_TAG` default `v8.5.8`. Coordinate with the AP1e owner.
- `crates/loams/src/{server,main}.rs`: `--sqldb-pd`, defaulting to the desktop's `127.0.0.1:19379`.
- `crates/loams-sqldb/src/runtime/local.rs`, tests.

**Produces:** `loams dev --features sqldb` creates, suspends, resumes and branches databases against the desktop's TiKV. The gate listens on `127.0.0.1:3306`, loopback plaintext is allowed, and suspend defaults to 15 min.

Tests: `desktop_create_connect_suspend_resume` (`LOAMS_IT_SQLDB=1`); `desktop_requires_tikv_stack_and_says_so`.

Commit `feat(sqldb): desktop single-node mode`.

### Task 21: The desktop's Loams SQL page on `loams.sqldb.v1`

**Files:** `web/plugins/wesql/` (page title and nav "Loams SQL"), `apps/desktop-electron/src/main/sql/wesql.ts` (a second `MySqlBackend` behind the existing seam), `src/main/stacks/stacks.ts`, `buf.gen.apps.yaml` (+ `loams/sqldb/v1`), `web/packages/proto` regenerated, tests. **Coordinate with the AP1e owner first.**

Tests:
- `uses_sqldb_api_when_available`, `falls_back_to_compose_without_sqldb`.
- `console_reads_with_reader_credential`, `write_requires_confirm_and_writer_credential`.
- `agent_tool_gets_reader_only`, `root_password_never_requested`.
- `resume_button_shows_progress_from_watch`.
- `compatibility_notes_link_shown` (§47 §13.2).

Commit `feat(desktop): loams sql page on loams.sqldb.v1`.

---

## SQ1f — Conformance and performance gates

### Task 22: The compatibility contract

**Files:** `conformance/sqldb/contract.tsv`, `crates/loams-compat` (a TiDB target), `crates/loams-sqldb/tests/contract.rs`, `docs/sqldb/compatibility.md`.

**Interfaces:** one test per row of §47 §13.2, run through the gate against TiDB and against stock MySQL 8.4 (a `mysql:8.4` container as the reference), classified with D309's method. Rows:
- write skew allowed;
- no gap lock (an insert into a locked range succeeds);
- `SERIALIZABLE` refused;
- FK cascades;
- `ROLLBACK TO SAVEPOINT` lock retention;
- stored procedure, trigger and XA refusals;
- DDL not blocked by an open transaction;
- the version string.

Tests: `contract_rows_match_published_table`; `contract_tsv_is_blessed`.

Commit `test(sqldb): published compatibility contract`.

### Task 23: Clients and ORM suites

**Files:** `.github/workflows/sqldb-clients.yml`, `conformance/sqldb/{clients,prisma,django,rails,laravel}.tsv`, `scripts/sqldb/orm/*.sh`.

**Exit:**
- Tier-1 clients are green: password connect, prepared statements, transactions, TLS verify-full, streaming, Dumpling/mydumper round trips.
- Each ORM suite is at ≥ 98 % of its pass count against TiDB direct, and the gate adds zero failures.
- Remaining failures are allowlisted with causes.

Commit `ci(sqldb): client and orm suites`.

### Task 24: Performance, density and fault evidence

**Files:** `scripts/sqldb/bench/{sysbench,tpcc,resume,density,copier,report}.sh`, `docs/sqldb/performance.md`, `.github/workflows/sqldb-perf.yml` (nightly on the reference hardware).

**Work:**
- §47 §18's gates on the reference topology (Q653), three interleaved repeats.
- Density at 1 000 and 10 000 idle databases.
- Class tuning, which updates the class table and `docs/sqldb/classes.md` (`class_table_matches_docs`).
- `loams-nemesis` list-append and bank through the gate under TiKV leader kills and TiDB pod kills, for 2 hours. The expected anomalies are snapshot isolation's only.

*Exit:* targets met, or the owner's acceptance recorded (Q660).

Commit `bench(sqldb): performance, density and nemesis evidence`.

### Task 25: Observability, quotas and hardening

**Files:** `crates/loams-sqldb/src/{metrics,quota}.rs`, `crates/loams-sqlgate/src/metrics.rs`, `deploy/sqldb/dashboards/`, alert rules, tests.

**Produces:**
- **Metrics** (§47 §20): `loams_sqlgate_*`, `loams_sqldb_*`, GC safe-point lag, log-backup lag.
- **Alerts:** safe point more than 1 h behind; log-backup lag above the RPO; resume p95; regions per store; quota at 90 %.
- **Quotas:** storage (read-only at 100 %), branches, connections, transaction duration by class.
- **Hardening:** no `FILE` privilege, `LOAD DATA LOCAL` off, status port restricted.

Tests: `gate_metrics_have_documented_names`, `storage_quota_turns_read_only`, `transaction_duration_cap_by_class`, `alert_rules_lint`, `status_port_not_reachable_from_gate_network`.

Commit `feat(sqldb): observability, quotas and hardening`.

### Task 26: Security review, docs and the GA flip

**Files:** `docs/sqldb/**` (quickstart, connecting, roles, compatibility, serverless behaviour, branches and PITR, CDC, runbooks, performance, licensing), `docs/sqldb/security-review.md`.

**Work:**
- The §47 threat table, one test per row.
- 24 h fuzzing of the gate codec.
- `cargo deny` and image scans.
- A review written by someone other than the implementer.
- Flip `Database.stage` to `GA`.

Test: `docs_examples_run`.

Commit `docs(sq1): loams sql ga`.

---

## SQ1g — Engine query SQL (carried over unchanged)

These tasks are copied from the superseded plan's Tasks 47–50 with their files, tests and commits unchanged. Q663 still gates Task 29.

### Task 27: `loams.sql.v1` `SqlService` (API1 Task 5)
**Files:** `proto/loams/sql/v1/sql.proto`, `crates/loams/src/api/connect_sql.rs`, `crates/loams/tests/connect_sql.rs`.

Tests: the ported `native_sql` tests with `_rpc`, `execute_query_streams_arrow_ipc`, `execute_query_json_rows_are_lossless_for_int64`, `client_disconnect_cancels_query`, `cancel_query_by_id`, `flight_sql_still_serves_8082`.

### Task 28: The Data Studio SQL tab on `loams.sql.v1`
**Files:** `web/plugins/data-studio/src/{client.ts,pages/sql.tsx}`, `buf.gen.apps.yaml`, tests. Coordinate with the AP1e owner.

Tests: `sql_uses_connect_when_available`, `sql_falls_back_to_rest`, `first_batch_renders_before_trailer`, `cancel_aborts_and_calls_cancel_query`, `sql_error_shown`.

### Task 29: Remove the engine's read-only `mysql-wire` listener (after Q663)
Tests: `no_mysql_wire_feature`, `variant_full_lists_sqldb`.

### Task 30: Limits on the engine SQL RPC
Tests: `sql_rpc_requires_query_scope`, `memory_limit_returns_resource_exhausted`.

---

## SQ1s — The S3 spike (gated; no production code)

### Task 31: S-A vs S-B vs TiDB X

**Files:** `docs/sqldb/s3-spike.md`, prototypes under `spikes/sqldb-s3/` (not workspace members, never shipped).

**Work** (over the read-only clones, with builds in the shared target only):
- **S-A.**
  - Sketch the C++ `FileSystemWrapper` shim in `tikv/rust-rocksdb`'s `crocksdb` with Rust callbacks.
  - Measure, on a single-node TiKV built from v8.5.8 with the shim: read p99 on cache miss, compaction read amplification against S3 (RustFS), and S3 bytes per replica.
- **S-B.**
  - Size the port of `cloud-engine`'s `kvengine`, `rfengine`, `rfstore` and `cloud_server` from v6.1 to v8.5: kvproto deltas, raftstore and storage-layer conflicts, and coprocessor and CDC compatibility.
  - Build the branch at its own fork point only if a toolchain from that era builds within a day; otherwise estimate from the diff.
- **TiDB X.** Record PingCAP's open-source status and licence at the time of the report.
- **Keyspace-level GC (Q666).** Size a TiKV patch (PR #16808's approach against PD master's GC-state API).

*Exit:* `docs/sqldb/s3-spike.md` with measured S-A numbers, an S-B effort estimate with its risks, the TiDB X status, and a recommendation.

Commit `docs(sq1): s3 spike report`.

### Task 32: The owner's decision and the follow-up plan

Record the owner's Q662 and Q666 choices in §47 and the decision log, then write the follow-up plan (`docs/plans/<date>-sq2-loams-sql-s3.md`) for the chosen path only.

Commit `docs(sq1): s3 path decision`.

---

## Exit criteria for production (all ticked for Task 26)

Compute and lifecycle
- [ ] Scale to zero and wake on connect through the gate; resume p95 within the target accepted under Q656 (Tasks 5, 24).
- [ ] No TiDB without `keyspace-name`; images pinned by digest (Tasks 1, 2).

Data correctness
- [ ] The GC loop honours TiDB's min start ts and writes the saved safe point; stale reads error (Task 10).
- [ ] Copy branches equal the parent at `point_ts`; PITR restores into new keyspaces equal recorded checksums; nightly drill green 14 nights in a row (Tasks 13, 16, 17).
- [ ] Nemesis runs show only snapshot-isolation anomalies (Task 24).

Security
- [ ] TLS-only off loopback; roles and grants; cross-database denial; secrets never stored in clear (Tasks 4, 11).
- [ ] Security review written with no open high or critical finding; 24 h fuzzing clean (Task 26).

Compatibility and performance
- [ ] §47 §13.2 published and every row tested (Task 22); ORM suites ≥ 98 % (Task 23).
- [ ] §47 §18 targets met or accepted (Task 24).

Operations
- [ ] BR snapshot and log backup for every branch; window and retention (Tasks 15, 17).
- [ ] CDC into Loams streams (Task 19).
- [ ] Observability and quotas (Task 25); the desktop page on the API, WeSQL compose deleted (Task 21).

## Self-review

- **Decisions to tasks.** Every §47 decision has tasks:
  - D720/D721: 0–2.
  - D722: 1, 2, 5, 6.
  - D723: 1, 9.
  - D724: 10.
  - D725: 0 (no code).
  - D726: 15–17, 31–32.
  - D727: 13, 16.
  - D728: 7, 8, 12.
  - D729/D734: 2, 24.
  - D730: 5, 13.
  - D731: 3, 4, 6.
  - D732: 4, 11.
  - D733: 5.
  - D735: 22, 23.
  - D736: 14.
  - D737: 20, 21.
  - D738: 18, 19, 25.
  - D739: 22–26.
- **Owner answers.** Q661, Q658, Q667 and Q662 shape Tasks 22–23, 15–17 and 31–32.
- **Review Focus.** Every row names its tests.
- **Task 1** needs no code changes outside scripts and configs, so it can run before any crate exists.

## Rulings made during execution

Task 1 numbers are in [`docs/sqldb/performance.md`](../sqldb/performance.md) (one PD, one TiKV and keyspace-mode TiDB v8.5.8 under Podman on a shared 15 GiB desktop host; images pre-pulled).

- **R1.1 Image pins (§47 §4): confirmed.** `release/sqldb-images.toml` holds the five v8.5.8 digests of §47 §4 unchanged. Docker Hub's `v8.5.8` tags still resolve to them (2026-10-08), each is a linux/amd64 + linux/arm64 index, and the pulled `tidb`, `pd` and `tikv` binaries report v8.5.8.
- **R1.2 Cold start (§47 §5.2): the estimate is confirmed, with margin; bootstrap is too slow for the connect path.**
  - A warm restart (start the process, connect to PD and TiKV, load the schema, open the port, answer `SELECT 1`) took a median of **0.6–0.8 s** and a p95 of **1.2–1.8 s**, for 0, 100 and 1 000 tables alike. About 0.25–0.3 s of that is `podman run` itself. Schema load is not visible at 1 000 tables.
  - The connect-to-first-result targets (p50 ≤ 2 s, p95 ≤ 5 s) stay as they are. What remains open is Kubernetes pod scheduling and the gate's own wake path, which Tasks 5 and 24 measure.
  - **A fresh keyspace's first start (bootstrap) took a median of 11–17 s and a p95 of up to 21 s** (single runs reached 65 s under host load). This confirms §5.1: bootstrap runs once in `CreateDatabase` and is never on the connect path. `CreateDatabase` needs a timeout of at least 120 s.
- **R1.3 `force-init-stats = false` (§47 §5.1): kept.** Up to 1 000 analyzed tables of 20 rows each, `true` and `false` gave the same warm start (medians within noise). The spike could not reproduce the slow-init-stats case, which needs large statistics. Loams still renders `false`, because it costs nothing and guards against that case per the source and the forum report. Task 24 retests it with large analyzed tables.
- **R1.4 Regions per empty database (§47 §6.3): replaced.**
  - With `split-table = true`, a bootstrapped empty database has **61 txn regions** (its `mysql` schema holds 59 tables).
  - With `split-table = false` it has **1 txn region**. Keyspace creation adds 1 raw region (`r|id`), so each database costs **2 regions**, measured exactly as the marginal cost over 100 keyspaces.
  - So 10 000 small databases are about 20 000 regions, not 600 000.
  - Up to 100 bootstrapped keyspaces (337 regions), every peer was hibernated, TiKV → PD heartbeats stayed at about 1 per second, and PD (about 115 MiB) and TiKV (about 340 MiB) RSS stayed flat.
  - **The 1 000 and 10 000 keyspace points of §6.3 were not run.** At about 12 s per bootstrap that is hours, and the host is shared. They stay a gate for Task 24 on the reference topology, before §15's density targets are set.
- **R1.5 TiDB memory (§47 §5.2): 224 MiB idle, 306 MiB after 60 s of load, and a 493 MiB peak (VmHWM)** under 8 clients. That is close to the 0.5 GiB `xs` class. Task 2's `xs` rendering must set `tidb_server_memory_limit` below the pod limit, or `xs` moves to 0.75 GiB. This is flagged for the owner, not decided here.
- **R1.6 PD keyspace API.** `POST /pd/api/v2/keyspaces` took a median of 131 ms and a p95 of 166 ms (PD waits for the region split). State changes to `DISABLED` and `ARCHIVED` took a median of about 20 ms and a p95 of about 31 ms. `ARCHIVED → TOMBSTONE` is accepted through the same API (HTTP 200).
- **R1.7 Deviations in Task 1.**
  - Added `scripts/sqldb/spike/lib.sh`, the helpers shared by the scripts.
  - TiDB containers are started by the scripts (`podman run`, label `io.loams.spike`) rather than as compose services, so each run can be timed.
  - sysbench was not installed, so the RSS load used the `mysql`-loop fallback the task allows.
  - `shellcheck` was not installed and was not fetched, so `bash -n` passes, but the "shellcheck clean" exit item is **still open**.
  - `bootstrap_ms` in `regions.jsonl` includes TiDB's graceful stop.
- **R2.1 Classes (controller ruling on R1.5, 2026-10-09): `xs` is 0.75 GiB.** `tidb_server_memory_limit` is 80 % of the pod memory limit. `split-table = false` and `force-init-stats = false` are kept (R1.3, R1.4).
  - **Why.** TiDB peaked at 493 MiB under load (R1.5). TiDB v8.5.8 also clamps any `tidb_server_memory_limit` below 512 MiB up to 512 MiB (`parseMemoryLimit`, `pkg/sessionctx/variable/varsutil.go`), so the 0.5 GiB `xs` of §47 §15 could not have had its 400 MiB limit.
  - **Cost.** Lower `xs` density. The owner may revise this.
  - **Limits.** `tidb_mem_quota_query` is 40 % of the pod memory limit (an estimate; Task 24 tunes both). Values are whole MiB, rounded down.
  - **Class table** (this plan's source of truth for `model::Class`; §47 §15 is updated to match; Task 23's `class_table_matches_docs` checks `docs/sqldb/classes.md` against it):

    | Class | vCPU | Memory | `tidb_server_memory_limit` | `tidb_mem_quota_query` | Gate connections | Pods (min–max) |
    |---|---|---|---|---|---|---|
    | `xs` | 0.25 | 0.75 GiB (768 MiB) | 614 MiB | 307 MiB | 100 | 0–1 |
    | `s` | 0.5 | 1 GiB | 819 MiB | 409 MiB | 200 | 0–1 |
    | `m` | 1 | 2 GiB | 1 638 MiB | 819 MiB | 500 | 0–1 |
    | `l` | 2 | 4 GiB | 3 276 MiB | 1 638 MiB | 1 000 | 0–2 |
    | `xl` | 4 | 8 GiB | 6 553 MiB | 3 276 MiB | 2 000 | 1–4 |
    | `2xl` | 8 | 16 GiB | 13 107 MiB | 6 553 MiB | 4 000 | 1–8 |
- **R2.2 Memory limits and log redaction are bootstrap SQL, not `tidb.toml`** (amended in fix round 1).
  - **The constraint.** In v8.5.8, `tidb_server_memory_limit`, `tidb_mem_quota_query` and `tidb_redact_log` are global system variables. They are not config items, and `mem-quota-query` is a removed config item.
  - **How they are applied.** `render::tidb` sets `initialize-sql-file = "/etc/tidb/init.sql"`. `render::tidb_init_sql(class)` writes `SET GLOBAL` for all three, and TiDB runs that file once, at the keyspace's first bootstrap.
  - **The values.**
    - `tidb_server_memory_limit = '80%'`, not a fixed size. TiDB resolves the percentage against the memory it sees (its cgroup limit, the class's container or pod limit) each time it starts, so the limit follows a class change by itself. The class table in R2.1 gives what 80 % comes to.
    - `tidb_mem_quota_query` is 40 % of the class memory, in bytes, from `tidb_globals(class)`.
    - `tidb_redact_log = 'OFF'` (R2.8).
  - **Re-applying them is unconditional.** The values live in the keyspace's `mysql.global_variables`. A statement that fails in `initialize-sql-file` is only logged as a warning (`InitializeSQLFile error`, `pkg/session/bootstrap.go`). So the control plane re-applies `render::tidb_globals(class)` through `ri_control` after every bootstrap (Task 9), every copy (Task 13) and every class change. The `SET GLOBAL` statements are idempotent.
  - **Task 9 checks them after bootstrap.** It reads the three values back and fails `CreateDatabase` if any differs. The `LOAMS_IT_SQLDB` test reads them back too, and checks that the member's log has no `InitializeSQLFile error`.
- **R2.3 `enable-global-kill` is a top-level key.** In v8.5.8 it is a top-level key, not one under `[security]` (`experimental.enable-global-kill` is a removed key). It is rendered at the top level. TiDB's own `--config-check --config-strict` accepts both golden configs (`rendered_config_passes_tidb_config_check`, `LOAMS_IT_SQLDB=1`).
- **R2.4 Rendering details** that the task did not spell out:
  - **Kept in the config, not on the command line.** `store = "tikv"` and `path` live in the config. Per-member `--host`, `-P`, `--status-host`, `--status` and `--advertise-address` stay on the command line, so every member shares one file.
  - **Other keys.** `[instance] tidb_enable_ddl = true` (§47 §5.1). `[security] tls-version = "TLSv1.2"`. `[proxy-protocol] header-timeout = 5`.
  - **Cluster TLS.** `cluster-ssl-*` is rendered only when `Endpoints::with_cluster_tls(true)`.
  - **Fixed container paths** (Kubernetes TLS Secret layout): `/etc/tidb/{tidb.toml,init.sql}`, `/etc/tidb/tls/{ca.crt,tls.crt,tls.key}` and `/etc/tidb/cluster-tls/…`.
  - **Gate networks.** They are `Cidr` values with host bits cleared. `*`, `0.0.0.0/0` and `::/0` are refused.
- **R2.5 Branch ids in Task 2.** `model::BranchId` accepts `br_` + 16 of `[0-9a-z]`. That covers every lower-case base32 alphabet and fits PD's keyspace-name rule. Task 7's `ids.rs` may narrow it to the chosen alphabet.
- **R2.6 Runtime contract details.**
  - **Class pod limits.** `SqlRuntime` does not enforce them (§47 §15). That is control-plane policy, and the IT scales an `xs` pool to 2 to exercise a warm second member.
  - **`LocalRuntime` state.** It keeps `state_dir/<branch>/{pool.json,tidb.toml,init.sql}`, with stable per-member port pairs from 24000+N and 25000+N. It labels containers `io.loams.sqldb.*` and replaces a member whose fingerprint changed (rendered config, image, class).
  - **CPU limits.** `LocalRuntime` passes `--cpus` by class unless `cpu_limits = false`. The IT turns it off, because a first bootstrap at 0.25 vCPU takes minutes. Memory is always limited.
  - **Bootstrap time.** The first bootstrap took about 8–45 s here, and it stays in `CreateDatabase` (R1.2). Task 7 should bootstrap at a larger class, or without a CPU limit.
- **R2.8 Redaction is OFF (controller ruling, fix round 1; reverses the earlier ON ruling and Task 2's MARKER).**
  - **Why not ON or MARKER.** TiDB redacts error messages when they are created (pingcap/errors), not only when it logs them. So clients would see `Duplicate entry '?'` instead of MySQL's text, which breaks the compatibility contract (D735, §47 §13.2).
  - **What renders.** `tidb_redact_log = 'OFF'` is in `tidb_globals` and `init.sql` (`redaction_is_off_for_mysql_compatibility`).
  - **How logs are protected instead.** The log pipeline does it: the slow and general logs, which carry statement literals, are not shipped off the pod by default. **Note for Task 25 (observability):** the log shipper's default excludes `tidb-slow.log` and the general log, and turning them on for a database is an explicit, audited choice.
- **R2.9 Security Enhanced Mode is on (controller ruling, fix round 1).** `[security] enable-sem = true` (`sem_and_secure_bootstrap_are_on`). Under SEM, SUPER does not imply the `RESTRICTED_*` privileges, so restricted variables (including `tidb_redact_log`) and tables are hidden even from root. The IT checks that root cannot read `@@global.tidb_redact_log`.
  - **Note for Task 11 (`ri_control`'s grants).** `ri_control` needs:
    - `RESTRICTED_VARIABLES_ADMIN`, to re-apply `tidb_redact_log` (R2.2);
    - `RESTRICTED_TABLES_ADMIN`, because `mysql.tidb`, which holds the `bootstrapped` row Task 9 reads, and `mysql.global_variables` are hidden under SEM;
    - `RESTRICTED_USER_ADMIN`, so tenant ADMIN roles cannot alter the `ri_*` users;
    - `SYSTEM_VARIABLES_ADMIN`, `CREATE USER` and `GRANT OPTION` for the `ri_*` roles.

    The tenant roles get none of the `RESTRICTED_*` privileges.
- **R2.10 Root lockdown (controller ruling, fix round 1).** `[security] secure-bootstrap = true`, so the first bootstrap creates `root@localhost` with `auth_socket` (OS user `root`) instead of an open `root@%`. `socket = "/var/run/tidb/tidb-{Port}.sock"`. The IT checks that root over TCP is refused.
  - **Who creates `ri_control` (Tasks 9 and 11).** The create saga does, right after bootstrap, as root over that socket:
    - **desktop:** `LocalRuntime` mounts `state_dir/<branch>/run` there and exposes `socket_path(branch, member)`. Under rootless Podman the host user is the container's root, so `auth_socket` accepts it. Under rootful Docker it would not, so the desktop needs Podman or an `exec`.
    - **Kubernetes (Task 14):** an `exec` into the pod, or a sidecar sharing the socket `emptyDir`.

    Task 11 may instead create `ri_control` in `init.sql` as `IDENTIFIED WITH tidb_auth_token`, with the JWKS public key mounted (`[security] auth-token-jwks`), which keeps every secret out of rendered config.
  - **Tests owed.** Task 9 owes `root_is_unreachable_over_tcp`, and Task 11 owes `ri_control_created_over_socket_only`.
- **R2.7 Deviation.** `docs/sqldb/licensing.md` and the crate's `build.rs` were added. `build.rs` turns `LOAMS_IT_SQLDB=1` into the cfg `loams_it_sqldb`, so container tests are `#[ignore]` unless it is set.
