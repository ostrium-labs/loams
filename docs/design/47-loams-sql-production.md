# 47 — Loams SQL in Production: Neon-like MySQL on TiDB Compute and the Loams TiKV

Status: **Proposed; owner answers recorded** · 2026-10-08. The owner answered Q658, Q661, Q662 and Q667 the same day (§22). The source questions are answered from read-only clones of the upstream repositories (§23). The implementation plan is [SQ1 (TiDB)](../plans/2026-10-08-sq1-loams-sql-tidb.md). Rewritten the same day for the owner's third directive, which replaces both earlier drafts (stock MySQL 8.4 with mywal and Vitess, then the PolarDB-X study):

> Loams SQL = "Neon-like MySQL" built on TiDB's code, adapted, on our TiKV, with Rust for everything Loams builds.

Decisions keep the numbers **D720–D739** and questions keep **Q655–Q669**; their content changes, and the [decision log](13-decision-log.md) rows change with it. Every entry is still "Proposed". The two earlier designs are summarised in Appendix A, "Alternatives considered". Plan: [SQ1 (TiDB)](../plans/2026-10-08-sq1-loams-sql-tidb.md), which supersedes [the earlier SQ1 plan](../plans/2026-10-08-sq1-loams-sql-production.md). **This document writes no code.**

Markers, as in §20, §28 and §31:

- **(verify)** means not checked against a primary source, or checked only from documentation and not run. The task that depends on the point checks it first.
- **(estimate)** means computed or reasoned, not measured.
- **(spike)** means measured in an earlier spike: §20 §10 and `docs/plans/r1-dependency-spike.md` (TiDB, TiKV and PD v8.5.8 with keyspaces, on a laptop).
- **(source)** means the answer needs source reading, which this study did not do. §23 lists the repositories the owner is asked to clone.

---

## 1. Summary

**Loams SQL is a serverless MySQL-compatible database built from three layers:**
- **Compute:** stateless `tidb-server` processes, one pool per database branch, unmodified or lightly patched. They scale to zero.
- **Storage:** the shared, multi-tenant Loams TiKV cluster, with **one TiKV API v2 keyspace per branch**.
- **Front end:** a Rust gate, `loams-sqlgate`, which terminates TLS, authenticates, wakes the compute and does the accounting.

Everything Loams builds is Rust: the gate, the control plane, the branch copier, the GC loop, and later the S3 tier as a TiKV fork. TiDB (Go) is consumed as an image. TiDB, TiKV, PD, TiCDC and BR are all Apache-2.0, so no GPL process remains anywhere on the Loams SQL path.

How Neon-like this is, honestly:

| Neon property | Loams SQL at GA (this design) | Later (behind §8 and §9's gates) |
|---|---|---|
| Stateless compute that scales to zero | **Yes.** `tidb-server` holds no data. Cold start is a process start plus schema load: **2–5 s (estimate)** | Pre-warmed pools |
| Shared multi-tenant storage | **Yes.** One TiKV cluster with a keyspace per branch | — |
| Bottomless storage on S3 | **No.** Data lives on TiKV's NVMe with three Raft replicas. S3 holds BR snapshots and the log backup (PITR) | An S3-tiered TiKV (a Rust fork; §8) |
| Instant copy-on-write branches | **No.** Branches are *copies* made at a timestamp, O(size) | CoW branches by sharing immutable SSTs, inside the S3-tiered fork (§9) |
| PITR | **Yes.** Through the GC window (snapshot reads) and BR log backup beyond it | — |

| # | Decision (short) |
|---|---|
| D720 | **What Loams SQL is**: serverless MySQL-compatible SQL with TiDB compute on Loams TiKV keyspaces, behind a Rust gate. Engine query SQL (DataFusion) stays a separate thing |
| D721 | **The engine is TiDB v8.5.x on TiKV**, unmodified or lightly patched. It amends D260, supersedes D301 and replaces the earlier D721 (stock MySQL 8.4) and Vitess (D302 for Loams SQL) |
| D722 | **Compute**: one stateless `tidb-server` pool per branch, scale to zero, woken by the gate |
| D723 | **Storage**: the shared Loams TiKV, one API v2 keyspace per branch, no per-table region splits |
| D724 | **GC**: Loams' GC loop remains the cluster's GC worker and honours every TiDB's minimum start ts. Keyspace-level GC is a prerequisite for CoW branches |
| D725 | **Durability and "our WAL"**: TiKV's Raft log is the WAL. mywal is withdrawn, and `loams-wal` and the safekeeper are not used for SQL |
| D726 | **Bottomless on S3, staged**: BR snapshots plus log backup at GA; an S3-tiered TiKV fork only after a spike and the owner's choice (Q662) |
| D727 | **Branches and PITR**: copy branches at a timestamp in v1; CoW branches in the S3 fork; restore always creates a new branch |
| D728 | **Control plane API `loams.sqldb.v1`** (kept), records under the metastore's `m` prefixes |
| D729 | **Resource model**: database → branches → keyspace + compute pool; classes size compute; storage is metered, not volume-bound |
| D730 | **`loams-sqlrouter` is the control plane's kernel** (the `Lifecycle` and `BranchCopy` machines), never a data path; no Vitess machines |
| D731 | **The gate `loams-sqlgate`** (Rust) in front of `tidb-server`: TLS, auth, wake, accounting, caps; packets only |
| D732 | **Authentication and authorization**: Loams roles rendered as TiDB users and grants inside each keyspace; tenants are isolated by keyspace |
| D733 | **Serverless lifecycle**: suspend stops the compute pool, and resume starts it. No data moves |
| D734 | **Compute classes** (tidb-server CPU and memory, connections, memory quotas); storage quotas per database |
| D735 | **The compatibility contract is TiDB's**: MySQL 8.0 protocol and syntax, snapshot-isolation RR, pessimistic locking, no stored procedures or triggers. Documented and tested, not hidden |
| D736 | **Kubernetes**: Loams' controller (`kube-rs`) owns the compute pods; PD and TiKV come from MT3's GitOps manifests |
| D737 | **Desktop single-node**: `tidb-server` on the desktop's existing local TiKV stack; unistore only in unit tests |
| D738 | **Observability, quotas, isolation, hardening and CDC** (a Rust consumer of TiKV's CDC stream into Loams streams; classic TiCDC cannot read keyspaces, §10) |
| D739 | **The GA bar is evidence**; engine query SQL keeps its own milestone (SQ1f) |

## 2. What Loams SQL is, and what it is not (D720, D721)

### 2.1 The product

| | Loams SQL | Loams Postgres | Loams Live | Engine query SQL |
|---|---|---|---|---|
| What | Serverless MySQL-compatible OLTP | Serverless Postgres (Neon fork) | Reactive application database | SQL over Loams' own objects |
| Compute | `tidb-server` v8.5.x per branch | Neon compute | `loams` | DataFusion inside `loams` |
| Storage | Loams TiKV, a keyspace per branch | pageserver + bucket | Loams TiKV | the bucket |
| Durable log | TiKV's Raft log; BR log backup to the bucket | `loams-wal` / safekeepers | TiKV | the Loams log |
| Design | this document | §28, §46 | §20 | §05, §44 |

### 2.2 Why TiDB, and what the owner overrules (D721)

D260 (2026-09-29, approved) said "TiKV only: no TiDB anywhere". D301 rejected an "InnoDB semantics on TiKV" engine because it is "TiDB rebuilt". The owner's directive takes the second argument at face value: if a MySQL engine on TiKV is TiDB, use TiDB. **D721 amends D260** to allow TiDB as Loams SQL's compute and nowhere else (metastore, Live, jobs and the durable store stay TiKV-only), and **supersedes D301 and, for Loams SQL, D302 and D320.**

What this buys over the earlier stock-MySQL-8.4 draft (Appendix A.1):
- **No GPL process.** TiDB is Apache-2.0, and so are TiKV, PD, TiCDC and BR (GitHub licence API, 2026-10-08).
- **No shard routing.** Vitess goes away, and so do mywal and its semi-sync protocol, reconcile-local, XtraBackup and per-shard volumes.
- **Storage is already shared and multi-tenant.** Loams already runs keyspace-mode TiDB on the dev TiKV (`deploy/tikv/tidb.toml`, keyspace `sql_dev`) **(spike)**.

What it costs (§13):
- **Transactions are not InnoDB's.** RR is snapshot isolation, and there are no gap locks and no SERIALIZABLE.
- **Missing MySQL features:** stored procedures, triggers, events, UDFs, `XA` and `SPATIAL`.
- **Syntax level:** MySQL 8.0, with nothing specific to 8.4.

### 2.3 Non-goals

- **A SQL parser or planner of Loams' own.** TiDB is the SQL layer, and the gate never parses SQL (D731).
- **Forking TiDB for features.** Patches are limited to §5.4's list, each upstreamable.
- **One TiDB serving several tenants.** A `tidb-server` serves exactly one keyspace (`tikv_driver.go`, §20 §10.2) **(spike)**.
- **Cross-database transactions.** Each branch is its own keyspace, and TiDB cannot span keyspaces.
- **TiFlash and full-text search in v1.** TiFlash stays an optional add-on (§20 §10.4); open-source full-text is not usable (§20 §10.4).

### 2.4 What exists today (checked 2026-10-08 on `dev`)

| Piece | State |
|---|---|
| `deploy/tikv/` | PD and TiKV `v8.5.0` (compose default `TIKV_TAG`), API v2 with TTL, pre-allocated keyspaces including `sql_dev` and `loams_test_sql`; `tidb.toml` / `tidb-test.toml` set `keyspace-name`. The R1 spike ran v8.5.8 |
| R1 spike (§20 §10, `r1-dependency-spike.md`) | Keyspace-mode TiDB v8.5.8 serves CREATE, INSERT, UPDATE, BEGIN/COMMIT and SELECT; a key in another keyspace is invisible; PD, TiKV and TiDB were ready 12 s after a playground start. TiDB RSS was 180–365 MB under a write workload. **TiKV v8.5.8 ignores keyspace-level safe points**, so Loams' GC loop is the cluster GC worker |
| `crates/loams-tikv` | Loams' `tikv-client` fork with the GC loop (service safe point `gc_worker`, resolve locks, `UpdateGCSafePoint`) |
| `crates/loams-sqlrouter`, `loams-compat` | RT0's machines and inventories (Vitess-era; the method is reused for TiDB, §17) |
| MT1 | Not built: no `loams-auth` crate, no token verifier, and the Connect port is unauthenticated (`crates/loams/src/api/connect.rs`, `who_am_i`) |
| Desktop | AP1e Tasks 21–23 run `deploy/wesql` as root through `mysql2`, behind the `MySqlBackend` seam (`apps/desktop-electron/src/main/sql/wesql.ts`). The local TiKV stack (AP1e Task 21) already exists |
| Metastore prefixes | `x/`, `X/`, `C/`, `E/`, `R/`, `D/` are claimed by PG2 (branch `backend/pg2`, Task 0); `keys.rs` uses `a c e h H i k K l L n N o p q r s S w W` |

## 3. Architecture

```
 MySQL clients ─TLS─► loams-sqlgate ×N (Rust) ───────► tidb-server pool of branch B (Go, stateless, 0..n pods)
                      auth · SNI/user → branch               keyspace-name = <branch_id>    
                      wake (EnsureRunning) · caps                    │ client-go: TSO, 2PC, coprocessor
                      activity accounting                            ▼
                                                    PD (TSO, keyspace meta) ── TiKV cluster (shared; Raft ×3)
                                                                                     │  keyspace per branch
                                    Loams GC loop (Rust, cluster GC worker) ─────────┤
                                    branch copier (Rust, snapshot at ts) ────────────┤
                                    loams-sqlcdc (Rust, TiKV CDC stream) ─► Loams streams
                                    BR: snapshot + log backup ─► bucket sqlbackup/<ns>/<db>/<branch>/…
 Loams control plane (`loams`, feature `sqldb`): loams.sqldb.v1 · metastore records · Resonate sagas ·
 loams-sqlrouter machines · runtime drivers (kubernetes | local) · keyspace lifecycle via PD's HTTP API
```

**Commit path.** The client's `COMMIT` reaches `tidb-server` through the gate. TiDB runs Percolator 2PC with async commit and 1PC where possible, against TiKV regions replicated by Raft. The commit is durable once a Raft majority has the log entry. **Neither Loams nor TiDB adds a log.**

**Read path.** TiDB asks PD for a start ts and reads TiKV, pushing coprocessor DAGs down to the regions of the branch's keyspace. A branch never reads another keyspace in v1.

## 4. Verified facts and pins (2026-10-08)

| Item | Finding | Source |
|---|---|---|
| TiDB, TiKV, PD, TiCDC, BR | **v8.5.8** (2026-08-27; ≥ 14 days old) is the newest GA release. v9.0.0-beta.1 (2025-03-31) is the only 9.x tag. Images: `pingcap/tidb:v8.5.8@sha256:df168c764bf2dfdb166dc37a5c3b0e210d29d5f3ab2d33317fd0fdf7b32037f5`, `pingcap/tikv:v8.5.8@sha256:ab84580b6795868940231aa778a34b082d19e4417e6d66f755c609bebdbbfd69`, `pingcap/pd:v8.5.8@sha256:424e896800e42e1b7eb585b604c8daa3454110d0f0df5ab41f7c5f49164d3aef`, `pingcap/ticdc:v8.5.8@sha256:e77e206caedf5fe03eb48f10ae3f92b4e32582d375708c660aead9291ce6d4e8`, `pingcap/br:v8.5.8@sha256:15beb655d3276fe64aa6a353f9bf3d828d34272d2470abdf5026457b754f1c45` (amd64 and arm64) | GitHub releases API; Docker Hub |
| Licences | TiDB, TiKV, PD, TiCDC, TiProxy, `tikv/rust-rocksdb`: Apache-2.0 | GitHub licence API |
| Keyspace mode in open-source TiDB | `keyspace-name` works on the classic v8.5.8 build **(spike)**. PingCAP documents keyspaces only for TiDB Cloud and next-gen, and tidb-operator v2 rejects `keyspace` for classic clusters (§20 §10.2), so this is a supported binary path but an unadvertised deployment mode | §20 §10.1–§10.2 |
| Keyspace-level GC | **Not in any release.** PD v8.5.8 answers the keyspace GC-state RPCs with `Unimplemented`, and TiKV v8.5.8 reads only the cluster safe point **(spike)**. PD **master** has a GC state manager with keyspace-level GC (`tikv/pd#11100`, merged 2026-08-05; `#10677`). TiKV's PR "Support keyspace level GC" (`tikv/tikv#16808`) is **open, marked DNM**, and was last updated 2025-12-01. BR master switches to per-keyspace GC barriers when given `--keyspace-name` (`pingcap/tidb#65483`, 2026-01-23) | GitHub PR metadata |
| Object-storage TiKV ("TiDB X") | **Closed source today.** PingCAP: "TiDB plans to open source it by the end of 2026" (blog, 2026-07-09). Architecture: object storage is "the single source of truth", with a new "RF engine" (Raft log) and an LSM "KV engine" replacing RocksDB, and local disk as a cache | pingcap.com/blog/multi-tenant-agent-database-tidb-scaile-europe; docs.pingcap.com/tidbcloud/tidb-x-architecture |
| Earlier open shared-storage engine | `tikv/tikv` branch **`cloud-engine`** (Apache-2.0, Rust): `components/{kvengine,kvenginepb,rfengine,rfstore,cloud_server,cloud}`, last commit 2022-09-26, 418 commits ahead of and 2046 behind master | GitHub branch and compare API |
| TiDB nextgen on the TiDB side | Public: release branches such as `release-nextgen-202603`, the `nextgen` kernel type (§20 §10.4) | GitHub |
| MySQL compatibility (v8.5 docs) | "Highly compatible with … MySQL 5.7 and MySQL 8.0". Unsupported: stored procedures and functions, triggers, events, UDFs, `FULLTEXT` indexes (self-managed), `SPATIAL`, `XA` syntax, `CREATE TABLE … AS SELECT`, `SKIP LOCKED`, descending indexes, `SELECT … INTO @var`, the MySQL replication protocol, and character sets beyond ascii, latin1, binary, utf8, utf8mb4 and gbk. `lower_case_table_names = 2` only | docs.pingcap.com/tidb/stable/mysql-compatibility |
| Isolation | RR = snapshot isolation, which allows write skew. RC only in pessimistic mode. Pessimistic is the default since v3.0.8. **No gap locks**: "concurrent DML statements within the range are not blocked". Autocommit `SELECT FOR UPDATE` does not wait. DDL is not blocked by open transactions | docs: transaction-isolation-levels, pessimistic-transaction |
| Foreign keys | Since v6.6.0, **GA in v8.5.0**; `CASCADE` and `SET NULL` supported; not on partitioned tables or `BLOB`/`TEXT` | docs: foreign-key |
| Savepoints | Since v6.2.0. `ROLLBACK TO SAVEPOINT` does not release locks taken after the savepoint | docs: sql-statement-savepoint |
| Desktop images | Same v8.5.8 digests. `deploy/tikv/compose.yaml` moves from v8.5.0 to v8.5.8 | — |
| Source findings | See §23: GC registrations, BR keyspace rewrite, TiCDC classic, `cloud-engine`, config defaults | Owner's clones, 2026-10-08 |

## 5. Compute: a `tidb-server` pool per branch (D722)

### 5.1 Shape
- **One pool per branch** of 0–n `tidb-server` pods (a Deployment in Kubernetes, a container on the desktop). Each pod has `keyspace-name = <branch_id>` and `--path` set to the shared PD.
- **Rendered `tidb.toml`:**
  - `split-table = false` (§6.3); `performance.force-init-stats = false`, because the v8.5.8 default is **`true`**, which blocks the port until statistics load (`pkg/config/config.go` at v8.5.8); `lite-init-stats = true` (already the default);
  - `server-version` advertised as `8.0.11-TiDB-v8.5.8-Loams` (Q658);
  - `proxy-protocol.networks` set to the gate's CIDR, so TiDB sees client addresses (verify);
  - `security.ssl-*` for gate → TiDB TLS;
  - `instance.tidb_enable_ddl` on every pod (TiDB elects a DDL owner through PD's etcd, per keyspace);
  - `tidb_server_memory_limit = '80%'` of the pod limit and `tidb_mem_quota_query` by class, `tidb_redact_log = OFF` (§13.2), all as bootstrap SQL because they are global variables in v8.5.8 (SQ1 R2.2, R2.8);
  - `enable-global-kill = true`;
  - `security.enable-sem = true` and `security.secure-bootstrap = true`, with `socket = ""`. Root is `auth_socket` with no socket, so it is locked out, and `ri_control` is created by the bootstrap SQL (SQ1 R2.9–R2.11; v8.5.8 panics on a socket connection while the PROXY protocol is on).
- **Bootstrap.** A new keyspace needs TiDB's bootstrap: it creates the `mysql.*` system tables and runs upgrade DDL. It runs **once, in `CreateDatabase`**, never on the connect path. A copy branch inherits the bootstrapped system tables from its parent, so branches never bootstrap again.

### 5.2 Cold start (estimate; measured in Task 1)
- **Process start and connection to PD and TiKV:** well under 1 s for a static Go binary on a warm node **(estimate)**.
- **Schema load.** On a warm keyspace, TiDB loads the information schema from TiKV's meta keys before it opens the port. This is **0.2–2 s** for a schema of up to a few hundred tables **(estimate)**; v8.4+ caches the schema and can load it lazily for very large schemas (`tidb_schema_cache_size`, docs v8.4).
- **Statistics.** Statistics are loaded asynchronously with `force-init-stats = false`. A forum report on v6.5 shows a 5-minute "init stats" phase *before* the port opened when it was forced (ask.pingcap.com #7137), so this setting is mandatory.
- **Pod scheduling.** With an image already on the node it costs 0.5–2 s; a node pull costs 5–20 s, so every node keeps the image (DaemonSet pre-pull).
- **Total.** Connect-to-first-result p50 **≈ 2 s**, p95 **≈ 5 s** for a warm node with a pre-pulled image **(estimate)**. No data is downloaded, and this does not depend on database size, unlike the earlier draft's 30–60 s cold resume.
- **Memory.** 224 MiB idle, 306 MiB after load and a 493 MiB peak per `tidb-server` (Task 1, R1.5). The smallest class is therefore 0.75 GiB (R2.1), and `xs` pools are packed densely.

### 5.3 More than one pod
TiDB pods of one keyspace coordinate through PD (DDL owner, global kill, auto-ID allocation). With more than one pod, AUTO_INCREMENT values are unique but not sequential across pods unless `AUTO_ID_CACHE 1` is set (docs). The gate balances connections across pods. Classes `xs`–`m` use one pod, and `l`+ autoscale on CPU.

### 5.4 Allowed TiDB patches ("lightly patched")
These are Go, kept as a patch queue on `ostrium-labs/tidb` against v8.5.x tags, each offered upstream:
1. **None for v1, if Task 1 confirms** that `keyspace-name`, PROXY protocol and the session-state statements behave as documented.
2. **Candidates if the spike shows a need:**
   - (a) registering the minimum start ts in a place Loams' GC loop reads, if keyspace-mode TiDB publishes it under a path the loop cannot see (§7.2, source);
   - (b) refusing `SET GLOBAL` of variables that must stay Loams-managed;
   - (c) an idle-timeout hint to the gate;
   - (d) **needed, pending the owner's decision:** the keyspace etcd prefix for the global DDL owner manager (upstream pingcap/tidb#60403, not in release-8.5). Without it every keyspace's TiDB on one PD elects one DDL owner between them, and DDL in the other keyspaces waits forever (SQ1 R5.3).

## 6. Storage: the shared TiKV, a keyspace per branch (D723)

### 6.1 Keyspaces
- **Naming.** The keyspace name is the branch id (`br_` + 16 base32 = 19 characters). Branch ids are globally unique, and PD master limits names to `^[-A-Za-z0-9_]{1,20}$` (`pkg/keyspace/util.go`), where v8.5.8 has no length limit. The keyspace is created through PD's keyspace HTTP API (`POST /pd/api/v2/keyspaces`, `PUT …/{name}/state`) by the control plane before the first TiDB start. PD's keyspace ids are 24-bit, about 16 M per cluster (verify).
- **Lifecycle.** The states are `ENABLED`, `DISABLED`, `ARCHIVED` and `TOMBSTONE`. Deleting a branch disables the keyspace, then archives it, and then Loams' GC loop destroys its key ranges (`UnsafeDestroyRange`). Whether PD or TiKV removes archived data by itself is **(source)**.
- **Isolation.** A TiDB in keyspace mode encodes every key under its keyspace prefix (`x` + 3-byte id) **(spike)**. A tenant cannot name another tenant's data from SQL, which is a stronger boundary than the earlier draft's table ACLs.

### 6.2 Placement
- **The cluster.** SQL keyspaces share the Loams TiKV cluster with the metastore, Live and `loams-wal`.
- **Placement rules.** These can put SQL keyspaces on their own stores (Q116), and large tenants get dedicated stores by rule.
- **Leaders.** They follow the region's majority, as for the rest of Loams.

### 6.3 Region overhead (the main scaling risk)
- **Regions per table.** TiDB's default `split-table = true` gives every table its own region. A fresh keyspace's `mysql.*` schema alone is about 60 tables (verify), so 10 000 small databases would be about 600 000 regions.
- **Rendered setting.** Loams renders **`split-table = false`**, so a small database lives in a handful of regions until size-based splits create more.
- **Idle regions.** Hibernate Region (TiKV's default) keeps idle regions quiet.
- **Gate.** Task 1 measures regions per empty database and PD heartbeat load at 1 000 and 10 000 keyspaces before the density targets of §15 are set.

### 6.4 TSO
All keyspaces share PD's TSO. PD can serve TSO through keyspace groups in its microservice mode (verify on v8.5.8). Until that is measured, one PD leader serves every tenant's TSO with batching; Task 1 measures the ceiling.

## 7. GC (D724)

### 7.1 Today's facts
- On v8.5.8, MVCC GC is **cluster-wide**: one safe point for every keyspace **(spike)**.
- Loams' GC loop in `loams-tikv` is the cluster's GC worker (§20 §9.3). TiDB in keyspace mode does only its own delete-ranges (dropped tables and indexes), driven by the safe point it reads (`gc_worker.go` at v8.5.8, quoted in the spike).

### 7.2 Consequences for SQL
1. **Long transactions (answered from source, §23).** Every TiDB v8.5.8 server, in keyspace mode too, writes its minimum active start ts to the **unprefixed** PD etcd key `/tidb/server/minstartts/<server-uuid>` under a session lease (`infosync.storeMinStartTS` always uses `unprefixedEtcdCli`). A cluster GC worker caps the safe point at `min(all) - 1` (`calcSafePointByMinStartTS`) and stores it at client-go's `GcSavedSafePoint` key (`saveSafePoint`), where TiDB's reads check visibility. **Loams' GC loop must do the same two things:**
   - read `/tidb/server/minstartts/` through PD's etcd API and cap the target;
   - write the saved safe point.

   Today it does neither (`crates/loams-tikv/src/gc.rs`). Values older than `GCMaxWaitTime` (24 h) are ignored by TiDB itself.
2. **One safe point for everyone.** It is the cluster minimum, so one tenant's long transaction, a branch copy or a BR backup delays GC for every tenant. Mitigations:
   - the gate and TiDB cap transaction duration by class (`tidb_gc_life_time` for SQL keyspaces, default 10 min);
   - the branch copier runs within that window, or splits its snapshot into windows (§9.1);
   - an alert fires when the safe point is more than 1 h behind (§20 §9.3).
3. **Delete-ranges.** A suspended database has no TiDB, so its delete-ranges wait until it resumes. The control plane wakes a branch's compute for a short GC pass if its pending delete-ranges exceed a threshold, or the GC loop does this itself after Task 2 reads the delete-range table format (source).

### 7.3 Keyspace-level GC
- **Status.** Upstream has the PD side on master but not in a release, and the TiKV side is unmerged (`#16808`).
- **What needs it.** Copy branches and PITR do not; they live inside the cluster safe point. **Copy-on-write branches through an MVCC overlay need it** (§9.2).
- **D724 ruling.** Cluster-wide GC at GA. Keyspace-level GC is carried as a TiKV patch only if Q666 chooses it, or comes with the S3 fork (§8.4), whose file-level sharing removes the need (§9.2).

## 8. Bottomless storage on S3 (D726)

### 8.1 What open source offers today
| Mechanism | What it gives | Limits |
|---|---|---|
| **BR snapshot backup** (`br backup full`, S3/GCS/Azure) | Consistent full backups of a cluster or (master, verify on v8.5.8) a keyspace | Copies data; restore is O(size) |
| **BR log backup + PITR** (TiKV `backup-stream`) | Continuous change logs to object storage; restore to any ts in the window. Mandatory for every Live cluster already (§20 §10.4) | RPO = the flush interval (minutes by default; verify); restore is O(size) |
| **TiDB X** (object storage as the truth) | Exactly the bottomless shape | **Closed**; open source "by the end of 2026" (stated plan) |
| **`tikv/tikv` `cloud-engine` branch** (2022, Apache-2.0, Rust) | `kvengine` (a Badger-style LSM with `dfs/s3.rs`, memtable, sstable, L0 tables, change-set apply; 410 KiB), `rfengine` (111 KiB), `rfstore` (481 KiB), `cloud_server` (294 KiB), `kvenginepb`: about 1.4 MiB of Rust | Forked from master at `1fb8980cc` (2022-05-19, v6.1 era) and merged up to 6.1.1. Tip `f219e75cb` (2022-09-26); 418 commits ahead, while master has 2 046 commits since the fork. Mostly two authors. It speaks v6.1's kvproto, so a port to v8.5 is substantial (§23) |
| TiFlash disaggregated mode on S3 | Columnar replicas on S3 | Columnar only, C++, not the row store |
| `rocksdb-cloud` (Rockset) | RocksDB with SSTs on S3 and cloning | C++, GPL-2.0 per GitHub's licence API, last pushed 2025-09-22: excluded |

### 8.2 GA: no fork
TiKV keeps its row data on NVMe with three replicas. For every SQL keyspace, BR **log backup** runs into `sqlbackup/<ns>/<db>/<branch>/log/…`, alongside a daily **snapshot backup**. That gives PITR beyond the GC window and a restorable copy in the bucket. It is not bottomless: capacity is TiKV's disks (Q667).

### 8.3 The Rust work: an S3-tiered TiKV fork
- **Fork, not plug-in.** TiKV has no runtime storage-engine plug-in API. `engine_traits` is an internal trait set, chosen at compile time (`engine_rocks`, and `kvengine` in `cloud-engine`). An S3 tier is therefore a fork, `ostrium-labs/tikv` (Apache-2.0, Rust), rebased on v8.5.x tags and kept as a patch queue. Its compatibility contract is the kvproto that TiDB v8.5.x speaks.
- **Two candidate designs**, chosen by a spike (SQ1s, Task 31) and the owner (Q662):

| | **S-A: tiered SSTs under RocksDB** (smallest) | **S-B: shared-storage engine** (TiDB X's shape; port `cloud-engine`'s `kvengine` + `rfengine`) |
|---|---|---|
| Change | An S3 tier under TiKV's RocksDB that uploads every sealed SST, keeps a local LRU file and block cache, evicts cold SSTs and fetches on miss. MANIFEST and WAL stay local. `tikv/rust-rocksdb` exposes only a byte-accounting `FileSystemInspector` and an encrypted env, not a pluggable `FileSystem`, so S-A needs a **small C++ `FileSystemWrapper` shim** in `crocksdb` that calls Rust callbacks (the pattern `FileSystemInspector` already uses). The tier logic is Rust | A new engine: one copy of immutable SSTs per region on S3 shared by all replicas; the Raft log on local disk with WAL chunks uploaded; followers apply by referencing files; compaction by one replica (or a separate pool) |
| Bottomless | Yes (capacity = S3; local disk = cache) | Yes |
| S3 copies | One per replica (3×), since replicas compact independently | One |
| Node replacement | Still a Raft snapshot transfer | Load metadata from S3; data on demand |
| CoW branches | No (no shared files) | **Yes**: a child region references the parent's SSTs (§9.2) |
| Size | Months (estimate) | A year or more (estimate), or less if TiDB X's source lands and is usable |
| Risk | Read latency on cache misses; RocksDB compaction reading from S3 | Everything in TiKV's storage path; long-term divergence |

- **D726 ruling.** GA ships §8.2 only. Task 31 is a two-week spike over the owner's clones (§23) that sizes S-A and S-B and checks the state of TiDB X's open-source release. Q662 then chooses between S-A, S-B and waiting for TiDB X. **No storage-engine code is written before that choice.**

### 8.4 Where "our WAL" fits (D725)
TiKV's Raft log (`raft-engine`) **is** the write-ahead log, replicated and fenced by Raft. Therefore:
- **mywal is withdrawn.** It existed only because stock MySQL's binlog needed an external durable home.
- **`loams-wal` and the safekeeper protocol add nothing in v1.** A second log in front of TiKV would double the commit latency for no durability gain. They stay Postgres's (§28, §46).
- **The one place a "Loams WAL" could matter later** is S-B. If TiKV nodes are to become stateless, the Raft log itself must live off-node, as Neon moved its WAL to safekeepers. That is what TiDB X's RF engine does with background uploads, and what `cloud-engine`'s `rfengine` was for. If S-B is chosen, the safekeeper-on-TiKV idea (D237) is **not** reusable as is: the log must be per-region and Raft-native. The candidate is `rfengine`, not `loams-wal`. This is recorded so that nobody wires `loams-wal` under TiKV by analogy with Neon.

## 9. Branches and PITR (D727)

### 9.1 v1: copy branches (no fork)
- **`CreateBranch(parent, point)`.** The point is `latest`, a timestamp, or a TSO.
  - If the point is **inside the GC window**, the Rust **branch copier** (`loams-sqldb`, over `loams-tikv`) creates the child's keyspace (named by its branch id). It reads the parent's whole keyspace at ts = point (a snapshot read; TiKV resolves locks) and writes the keys into the child keyspace in 1PC batches with fresh commit ts. TiDB's meta keys (`m…`) come along, so the child sees the same schema, table ids and system tables and needs no bootstrap.
  - If the point is **beyond the GC window**, the source is BR: restore the snapshot at or before the point, then the log backup, into the new keyspace. **BR v8.5.8 can restore into a different keyspace** (from source; to be confirmed by test). Snapshot restore decodes the backup's keyspace from a file key and rewrites every rule from the old keyspace prefix to the target TiDB's codec (`br/pkg/task/restore.go`, "keyspace rewrite mode"). Log restore applies the same old and new keyspace through `RewriteModeKeyspace` (`log_client/client.go`). The fallback, restoring into a scratch keyspace and copying from it, stays the plan B.
- **Cost.** O(size) in time and storage. Task 1 measures the copier's throughput; an estimate is 50–200 MB/s per copier over a LAN.
- **GC.** The copy holds a service safe point at the point for its duration. Copies of databases larger than the window can sustain (for example 10 GB at 100 MB/s ≈ 100 s, comfortably inside 10 min) are refused or go through BR.
- **Restore** is `CreateBranch(parent, point)` followed by an optional rename. It never overwrites (as before).

### 9.2 Later: copy-on-write branches
**Where the overlay can live:**

| Place | Verdict |
|---|---|
| `client-go` (TiDB's KV client) | **No.** TiDB pushes most reads to TiKV as coprocessor DAGs (aggregations, TopN, filters) per region. A client-side overlay would have to merge two keyspaces' partial aggregates, which is impossible in general, or disable pushdown, which is a performance cliff |
| TiKV coprocessor plug-in | **No.** TiKV's coprocessor plug-in API is RawKV-oriented and experimental, and cannot intercept TiDB's DAG reads (verify) |
| **TiKV storage layer, in the fork** | **Yes.** The child region's read snapshot is a merged view: the child's own keys, then the parent's keys at `ts ≤ branch_ts` under a prefix rewrite (parent keyspace prefix → child), with the child's tombstones masking the parent's keys |

**Two ways to build the storage-layer overlay:**
- **MVCC overlay (S-A-compatible).** Reads that miss in the child fall through to the parent keyspace's data. The parent's regions may live on other stores, so this becomes a remote read inside TiKV. That is heavy, and it **pins the parent's MVCC versions at `branch_ts`**, which on today's cluster-wide GC holds GC for the whole cluster for the branch's lifetime. So it needs keyspace-level GC (§7.3) and still costs remote reads. **Not recommended.**
- **SST sharing (S-B).**
  - At branch time, the child's regions are created from the parent's region metadata, referencing the parent's immutable SST files on S3 with a key-prefix rewrite and a `max_ts = branch_ts` filter. TiKV's SST importer already applies prefix-rewrite rules on ingest (BR restore), which is the same transform.
  - Writes go to the child's own memtable and SSTs. Parent compactions write new files, and the child keeps references to the old ones (refcounted on S3).
  - **The child pins files, not versions,** so the parent's GC is never held. This is the Neon-like branch, and it exists only with S-B or TiDB X.
- **D727 ruling.** Copy branches at GA; CoW branches only with S-B or TiDB X (Q662).

### 9.3 PITR window
- **The GC window** (`tidb_gc_life_time`, 10 min by default for SQL keyspaces, at most 24 h by class) serves instant branch-at-ts.
- **BR log backup** serves the rest of the PITR window: 7 days by default, 1–35 (Q665).

## 10. CDC into Loams streams (D738)
- **Classic TiCDC cannot read keyspaces (answered from source).** In `pingcap/ticdc`, `keyspace_manager.LoadKeyspace` returns the default keyspace whenever `kerneltype.IsClassic()`. `CreateTiStore` adds `keyspaceName` only for next-gen. The changefeed's `KeyspaceID` "in classic mode … will always be 0" (`pkg/config/changefeed.go`, master and v8.5.8). A next-gen TiCDC build expects next-gen TiKV and PD, so it is not an option on classic v8.5.8 **(estimate; the SQ1d spike confirms)**.
- **Design: `loams-sqlcdc`, a Rust consumer of TiKV's own CDC stream.**
  - **Upstream.** TiKV's `cdc` component accepts `kv_api = TiDb` on API v2 (`components/cdc/src/service.rs`, `validate_kv_api`), and subscriptions are per region and key range. The keyspace's `x`-prefixed txn range is therefore subscribable. The `cdcpb` bindings already exist in Loams' `client-rust` fork (`src/generated/cdcpb.rs`).
  - **What the consumer does:**
    - tracks the regions of the branch's keyspace;
    - merges the resolved ts;
    - decodes TiDB's row format (row codec v2 and the record and index keys of `t<table_id>_r…`), using table schemas read from the keyspace's meta keys at each schema version;
    - writes one Loams stream per table, in order, at commit-ts granularity.
- **Rust only.** No Go runs for CDC.
## 11. The control plane API: `loams.sqldb.v1` (D728)
The services and RPCs stay as in the earlier draft, with these changes:
- `ShardingService` is dropped, because TiKV splits regions automatically.
- `engine_version` becomes `tidb-v8.5.x`.
- `durability` is dropped (always TiKV).
- `Branch` gains `branch_kind` (`copy` | `cow`) and `point_ts`.
- `GetRestoreWindow` returns `{gc_window_start, backup_window_start, now}`.

| Service | RPCs |
|---|---|
| `DatabaseService` | `CreateDatabase`, `GetDatabase`, `ListDatabases`, `UpdateDatabase`, `DeleteDatabase`, `SuspendDatabase`, `ResumeDatabase`, `GetConnectionInfo`, `WatchDatabase` |
| `BranchService` | `CreateBranch(parent, point)`, `GetBranch`, `ListBranches`, `DeleteBranch` |
| `RoleService` | `CreateRole`, `ListRoles`, `DeleteRole`, `RotateRolePassword`, `CreateEphemeralCredential` |
| `BackupService` | `GetRestoreWindow`, `ListBackups`, `ExportDatabase` (Dumpling to a caller-named prefix) |

Mutations are `loams.operations.v1` operations run as Resonate sagas with deterministic step ids.

**Records** (metastore). PG2 has taken `x/` and `X/` (§2.4), so SQ1 uses the `m` family:
- `m/<ns>/<db_id>`: the database;
- `mn/<ns>/<name>`: the name index;
- `M/<ns>/<db_id>/<branch_id>`: the branch, keyspace id, compute pool state and `point_ts`;
- `mr/<ns>/<db_id>/<role_id>`: roles; secrets live in the credential store only.

`<ns>` is 8-byte big-endian, as in `keys.rs`. Task 4 adds `sqldb_prefixes_do_not_collide` against `keys.rs` and PG2's list.

## 12. The gate: `loams-sqlgate` (D731)
- **What carries over.** §12 of the earlier draft carries over with `tidb-server` as the upstream. That covers:
  - the greeting with the gate's own scramble and `caching_sha2_password`;
  - TLS 1.2+ with SNI per database;
  - identifying the branch from the role user (globally unique) or the SNI;
  - authentication (§13);
  - **wake**: `EnsureRunning`, holding the connection up to 30 s, or 1040 `database is resuming, retry`;
  - connecting upstream over TLS as the role's internal TiDB user, with the PROXY protocol header;
  - relaying packets, refusing `COM_CHANGE_USER` (1235) and replication commands, and counting every command except `COM_PING` as activity;
  - caps and a 10 s handshake deadline.
- **New.**
  - **Graceful compute replacement.** For scale-in, upgrades and suspend with idle sessions, the gate can migrate an idle session to another `tidb-server` with TiDB's session-state statements (`SHOW SESSION_STATES` / `SET SESSION_STATES`, the mechanism TiProxy uses; verify the statement names and restrictions on v8.5.8).
  - **A Rust reimplementation of TiProxy's design**, not a dependency. TiProxy is Go and is not used: the gate must also wake, account and verify Loams tokens.
- **Packets only, no SQL parsing** (unchanged).

## 13. Authentication, authorization and the compatibility contract (D732, D735)

### 13.1 Auth
- **Role passwords.** These are 32 random bytes, an Argon2id hash in the credential store, and full `caching_sha2_password` authentication over TLS at the gate. Loams identities (tokens or API keys in the password field) wait for MT1's verifier.
- **Upstream users.** The gate connects as the role's internal user `ri_<role>`, created by the control plane **inside the branch's keyspace** (TiDB's `mysql.user` is per keyspace).
- **Roles to grants.** The four roles map to TiDB grants on the database's schemas:
  - `reader`: `SELECT`, `SHOW VIEW`;
  - `writer`: DML;
  - `ddl`: plus DDL;
  - `admin`: plus `PROCESS` and `CONNECTION_ADMIN` (own sessions).
- **No user-management rights.** Application roles never get `SUPER`, `GRANT OPTION`, `CREATE USER`, `FILE`, `SYSTEM_VARIABLES_ADMIN` or `RESTRICTED_*`.
- **Cross-database isolation** is the keyspace (§6.1), so vttablet's table ACLs and query rules are gone.

### 13.2 What "InnoDB-like transactions" gets, and what it does not

| InnoDB behaviour | TiDB v8.5 | Consequence |
|---|---|---|
| ACID, durable on commit | Yes (Percolator 2PC, Raft majority) | Equal or stronger (synchronous replication) |
| REPEATABLE READ | **Snapshot isolation** advertised as RR; write skew possible; no phantoms in snapshot reads | Most apps unaffected; check-then-insert patterns that rely on InnoDB's next-key locks are not protected |
| Gap and next-key locks | **None**; range `SELECT … FOR UPDATE` does not block inserts into the range | Documented gap; apps use unique constraints or `SELECT … FOR UPDATE` on existing rows |
| READ COMMITTED | Yes (pessimistic mode) | Equal, without semi-consistent reads |
| SERIALIZABLE | **Not supported**; refused unless `tidb_skip_isolation_level_check` is set (verify), and then not serializable | Loams leaves the check on, so a client asking for SERIALIZABLE gets an error, not a silent downgrade |
| Pessimistic row locks | Yes (default) | Equal for row locks |
| Foreign keys | **GA since v8.5.0**, with cascades | Equal, except on partitioned tables and BLOB/TEXT |
| Savepoints | Yes; `ROLLBACK TO` keeps later locks | Minor |
| XA | **Not exposed** | Apps using XA are unsupported |
| Stored procedures, triggers, events, UDFs | **Not supported** | The largest compatibility gap; ORMs rarely need them, but legacy apps do (Q661) |
| DDL vs open transactions | Online, not blocked by open transactions | Different from MySQL's metadata locks; usually better |
| Version and syntax | MySQL 5.7/8.0, advertises `8.0.11-TiDB-…` | Nothing specific to 8.4 (Q658) |
| Error messages carry the offending values (`Duplicate entry 'x' for key …`) | Yes with `tidb_redact_log = OFF`. With `ON` or `MARKER`, TiDB redacts the error itself when it is created (pingcap/errors), so clients get `Duplicate entry '?'` | Loams renders `OFF` (SQ1 R2.8) to keep MySQL's error text. The trade-off is that literals reach TiDB's slow and general logs, so those logs stay on the pod and are not shipped by default (SQ1 Task 25) |

**D735:** this table is the contract. The docs publish it, the conformance suite (§17) tests every row, and the gate never pretends otherwise.

## 14. Serverless lifecycle (D733)
- **States:** `CREATING → RUNNING ⇄ SUSPENDING → SUSPENDED → RESUMING → RUNNING`, plus `BRANCHING`, `RESTORING`, `FAILED` and `DELETING`.
- **Idle.** A database is idle after no command except `COM_PING` for `suspend_after` (default 5 min; `0` disables).
- **Suspend.** The gate stops admitting connections, closes idle ones (1053) and waits for in-flight transactions (30 s, then kill). The pool scales to 0. **No data moves**; the keyspace stays `ENABLED`.
- **Resume.** The first connection makes the gate call `EnsureRunning`, which scales the pool to 1. The gate holds the connection until the pod's port is up and a probe query answers. Target (estimate, set by Task 1): p50 ≤ 2 s, p95 ≤ 5 s (§5.2).
- **No HA exception.** TiKV is HA by Raft, and compute failover is "start another pod".

## 15. Resource model and classes (D729, D734)
- **Database = branches**. Each branch is a keyspace plus a compute pool and a gate route.

| Class | vCPU | Memory | Gate connections | Pods (min–max) | `tidb_server_memory_limit` |
|---|---|---|---|---|---|
| `xs` | 0.25 | 0.75 GiB | 100 | 0–1 | 614 MiB |
| `s` | 0.5 | 1 GiB | 200 | 0–1 | 819 MiB |
| `m` | 1 | 2 GiB | 500 | 0–1 | 1 638 MiB |
| `l` | 2 | 4 GiB | 1 000 | 0–2 | 3 276 MiB |
| `xl` | 4 | 8 GiB | 2 000 | 1–4 | 6 553 MiB |
| `2xl` | 8 | 16 GiB | 4 000 | 1–8 | 13 107 MiB |

(Estimates; Task 24 tunes them. `xs` moved from 0.5 to 0.75 GiB and every `tidb_server_memory_limit` is 80 % of the pod limit, per SQ1 ruling R2.1: TiDB peaked at 493 MiB, and v8.5.8 clamps limits below 512 MiB.)
- **Storage.** It is metered per keyspace (PD region sizes, verify) and capped by a quota, not a volume. Above the quota the database turns read-only.
- **Density.** The target is ≥ 10 000 databases per TiKV cluster at the `xs`/`s` mix, after §6.3's measurement (Q660).

## 16. Kubernetes (D736)
- **Loams' controller** (`kube-rs`, in `loams-sqldb`) owns, per branch:
  - the compute Deployment;
  - its Service, Secret (TLS) and NetworkPolicy (the gate → TiDB → PD and TiKV only);
  - the BR log-backup task.
- **GitOps.** PD, TiKV (with placement rules) and the gate pool come from MT3's GitOps manifests. tidb-operator v2 may manage PD and TiKV only (no `TiDBGroup`), as D179 already does (Q664).
- **Images** are pinned by digest (§4) and pre-pulled.

## 17. Conformance (D739)
- **TiDB ↔ MySQL 8.0/8.4 differential.** Re-target `loams-compat`'s inventory method (D309) at `tidb v8.5.8` versus stock MySQL 8.4 and 8.0. Every row of §13.2 gets a test, and the classifications go to `conformance/sqldb/*.tsv`.
- **Clients and ORMs.** Tier 1: the `mysql` CLI, Dumpling/`mydumper`, Connector/J, go-sql-driver, `mysql2`, PyMySQL, `mysqlclient`, PDO, MySqlConnector. Prisma, Django, Rails and Laravel suites must reach ≥ 98 % of their pass count against TiDB itself (direct), and the gate must add no failures. Failures against stock MySQL are allowlisted with causes.
- **Isolation tests.** Elle/Jepsen-style list-append and bank workloads through the gate under TiKV leader kills and TiDB pod kills. These follow `loams-nemesis` (D315); the expected anomalies are exactly snapshot isolation's.
- **Branch and PITR tests.** A copy branch's checksum equals the parent's at `point_ts` (`ADMIN CHECKSUM TABLE` on both). A restore equals the recorded checksum.

## 18. Performance gates (D739)
Every number here is an estimate until Task 1. Measurements run on the reference topology of §46 §16.3 (Q653): three TiKV nodes with PLP NVMe, PD ×3, and one `xl` compute pod.

| Gate | Target (estimate) |
|---|---|
| sysbench `oltp_point_select` through the gate, 64 threads | p99 ≤ 3 ms; gate overhead ≤ 0.2 ms p50 and ≤ 5 % of throughput against a direct connection to TiDB |
| `oltp_write_only` commit p99 | ≤ 10 ms (Raft majority + 2PC; async commit / 1PC on) |
| TPC-C (go-tpc), 100 warehouses | ≥ 0.6× stock MySQL 8.4 single-node with `sync_binlog = 1` on the same hardware (honest: TiDB trades single-node speed for replication) |
| Resume (connect → first result), warm node | p50 ≤ 2 s, p95 ≤ 5 s |
| Copy branch throughput | ≥ 100 MB/s |
| Density | ≥ 10 000 idle databases per cluster without PD or TiKV saturation |

## 19. SQ1 milestone outline (the plan: [SQ1 (TiDB)](../plans/2026-10-08-sq1-loams-sql-tidb.md))

| Milestone | Scope | Exit |
|---|---|---|
| **SQ1a — Gate and compute lifecycle** | Baselines and pins (Task 1); `loams-sqlgate` codec and server; `SqlRuntime` with the local driver and `tidb.toml` rendering; the `Lifecycle` machine; suspend and resume sagas; wake on connect; idle-session migration | A database scales to zero and wakes on connect through the gate, p95 ≤ 5 s on the desktop stack |
| **SQ1b — Control plane, API, keyspaces, copy branches** | `loams.sqldb.v1`; records under `m/`; keyspace lifecycle and bootstrap; the GC loop reading TiDB's registrations; roles and credentials; handlers; the Rust branch copier; the Kubernetes driver | `CreateDatabase`/`CreateBranch` serve a branch whose checksum equals the parent's at `point_ts` |
| **SQ1c — BR backup and PITR to S3** | BR snapshot and log backup per branch; restore into a new keyspace; the restore window | A branch restored at a point outside the GC window matches its recorded checksum |
| **SQ1d — CDC to Loams streams** | Spike (next-gen TiCDC vs Rust); `loams-sqlcdc` | Committed rows appear in Loams streams in commit order |
| **SQ1e — Desktop single-node** | `loams dev --features sqldb` on the local TiKV stack; the AP1e page on `loams.sqldb.v1` | The desktop page runs on the API; WeSQL compose deleted |
| **SQ1f — Conformance and performance gates** | §13.2 contract tests; ORM suites; perf and density gates; nemesis; observability; security review; GA | GA checklist ticked |
| **SQ1g — Engine query SQL** | Carried over unchanged from the superseded plan | Independent |
| **SQ1s — S3 spike (gated)** | S-A vs S-B prototypes and the TiDB X check; a report to the owner | Owner's decision (Q662); no production code |

## 20. Observability, quotas, isolation and hardening (D738)
- **Metrics:**
  - the gate (`loams_sqlgate_*`) and the control plane (`loams_sqldb_*`: operations, resume seconds, branch copy bytes and seconds);
  - TiDB's and TiKV's own Prometheus metrics, labelled by keyspace;
  - the GC loop's safe-point lag;
  - BR log-backup checkpoint lag.
- **Alerts:**
  - the safe point more than 1 h behind;
  - log-backup checkpoint lag above the RPO;
  - resume p95 over target;
  - region count per store, or PD heartbeat latency, over threshold;
  - keyspace storage at 90 % of quota.
- **Quotas:** storage per database, branches (10), connections and connection rate, transaction duration by class (protects GC, §7.2), and per-query memory (`tidb_mem_quota_query`).
- **Hardening:**
  - no `FILE` privilege and `secure-file-priv` empty for application roles; `LOAD DATA LOCAL` off;
  - the TiDB status port (10080) reachable only from the control plane;
  - `enable-global-kill`;
  - no `tidb_skip_isolation_level_check`.

## 21. Risks

| # | Risk | Mitigation |
|---|---|---|
| 1 | **Keyspace mode is unadvertised for classic TiDB** (PingCAP documents it for Cloud and next-gen); upstream changes could break it | Pin v8.5.x LTS; the R1 spike and Task 1 test it; patch queue (§5.4); track PingCAP's TiDB X release |
| 2 | **Cluster-wide GC**: one tenant's long transaction or copy holds everyone's GC | Class caps on transaction duration; windowed copies; alerting; keyspace-level GC via Q666 |
| 3 | **Region and PD overhead at high database counts** | `split-table = false`, Hibernate Region, Task 1's density measurement before targets; placement by tenant size |
| 4 | **"InnoDB-like" expectations**: SI instead of gap locks, no SERIALIZABLE, no stored procedures or triggers | §13.2 published; conformance per row; Q661 |
| 5 | **No bottomless storage or CoW at GA** | Honest positioning (§1 table); §8's staged plan; Q667 |
| 6 | **An S3 fork of TiKV is a long-lived divergence** | Spike first; prefer upstream TiDB X if it opens under Apache-2.0 by the end of 2026; S-A's surface is one env layer |
| 7 | **TiDB process memory makes many tiny databases costly while awake** | `xs` at 0.5 GiB; aggressive suspend; measure |
| 8 | **One TiKV cluster carries metastore, Live, `loams-wal` and SQL** | Placement rules (Q116); SQL quotas; separate SQL stores for large tenants |
| 9 | **TSO as a shared bottleneck** | Batching; PD keyspace groups (verify); Task 1 measures |

## 22. Open questions

| # | Question | Needed by |
|---|---|---|
| Q655 | Package `loams.sqldb.v1` (proposed) or reassign `loams.sql.v1` to Loams SQL | SQ1b |
| Q656 | Accept measured resume times (Task 1), or fund pre-warmed compute pools | SQ1a, Task 1 report |
| Q657 | Retire WeSQL (§29) and the MySQL 8.4 + mywal design entirely (proposed), keeping §29 only as history | Founder, now |
| Q658 | ~~MySQL 8.4 syntax~~ **Answered 2026-10-08 by the owner: accept TiDB's MySQL 8.0-level dialect**; advertise `8.0.11-TiDB-v8.5.x`; document and test it | Answered |
| Q659 | Loams tokens only through the password field (proposed) | SQ1a (after MT1) |
| Q660 | Accept §18's targets and the density target after Task 1 | SQ1a report |
| Q661 | ~~Accept TiDB's transaction semantics as "InnoDB-like"~~ **Answered 2026-10-08 by the owner: accept** snapshot-isolation RR, no gap locks, no SERIALIZABLE, no stored procedures, triggers or XA; document §13.2 and test it against the ORM suites | Answered |
| Q662 | **The S3 path. Partly answered 2026-10-08 by the owner: spike first, then decide.** Compare S-A (SSTs on S3 with a local cache, in a TiKV fork) with S-B (revived `cloud-engine`), and record TiDB X's announced open-source release as the wait option. The choice itself is made after SQ1s's report | Founder, after SQ1s |
| Q663 | Remove the engine's read-only `mysql-wire` listener (proposed; answers Q260 "no") | SQ1g |
| Q664 | tidb-operator v2 for the shared PD/TiKV (as D179), or plain manifests | SQ1b |
| Q665 | PITR window default (7 days) and maximum (35); S3 Object Lock on the backup prefix | SQ1c |
| Q666 | **Keyspace-level GC.** Source reading shows it is impossible on v8.5.8 without a TiKV change (§23): TiKV reads only the cluster safe point, and master has no keyspace-level GC either. Carry a TiKV patch in the fork before S-B, or accept cluster-wide GC with class caps until the S3 engine (proposed) | Founder, with Q662 |
| Q667 | ~~GA without bottomless storage~~ **Answered 2026-10-08 by the owner: yes.** GA ships on TiKV disks with BR snapshots and log backup (PITR) to S3 and full-copy branches; bottomless storage comes later | Answered |
| Q668 | Desktop: `tidb-server` on the local TiKV stack (proposed) vs unistore (tests only) | SQ1e |
| Q669 | ~~MySQL 8.4 from day one~~ Superseded 2026-10-08 by the owner's TiDB direction (D721); the 8.4 *syntax* question moves to Q658 | Resolved |

## 23. Source findings (read-only clones under `~/Documents/Ostriumlabs/`, 2026-10-08)

The clones are partial (`blob:none`) and sit at their default branches (tidb, tikv, pd and ticdc at master; tiproxy at main). Version-specific files were read at the v8.5.8 tag from GitHub's raw file server, so the clones were not changed. One `git log -L` on the tidb clone fetched some blobs lazily before it was stopped. Every later command ran with `GIT_NO_LAZY_FETCH=1`.

| Question | Finding | Evidence |
|---|---|---|
| How does GC see TiDB's active transactions? | Each TiDB publishes its minimum start ts (sessions, cursors, internal sessions, recent schema reads; lower-bounded by `GCMaxWaitTime`) to **unprefixed** PD etcd `/tidb/server/minstartts/<uuid>` under a lease, keyspace mode included. The cluster GC worker caps the safe point at `min - 1`, sets service safe point `gc_worker`, resolves locks, and saves the safe point at client-go's `GcSavedSafePoint`. Keyspace-mode TiDBs run only their own delete-ranges, using PD's GC safe point | tidb v8.5.8 `pkg/domain/infosync/info.go` `storeMinStartTS`; `pkg/store/gcworker/gc_worker.go` `leaderTick`, `calcGlobalMinStartTS`, `calcSafePointByMinStartTS`, `saveSafePoint`, `runKeyspaceDeleteRange` |
| Can GC be per keyspace on v8.5.8 (keyspace config or service safe points)? | **No.** The v8.5.8 GC worker has no keyspace-level path. TiKV calls only `get_gc_safe_point` (cluster), and **TiKV master has no keyspace-level GC either**. Service safe points are cluster-wide. TiDB **master** and PD master have the keyspace-level design (`IsKeyspaceUsingKeyspaceLevelGC`, GC barriers, `AdvanceTxnSafePoint`, PD `gc_state_manager.go`, which still honours TiDB min-start-ts), but no release ships it and TiKV does not consume it | v8.5.8 `gc_worker.go`; tikv master `components/pd_client`; tidb master `gc_worker.go`; pd master `pkg/gc/gc_state_manager.go` |
| Can BR restore into a different keyspace? | **Yes, from source:** snapshot restore rewrites old to new keyspace prefixes (it requires the cluster's keyspace rewrite mode), and log restore uses `RewriteModeKeyspace`. Backup takes `--keyspace-name`. Per-keyspace GC barriers for BR are master-only (`#65483`). On v8.5.8, BR holds a cluster-wide service safe point | tidb v8.5.8 `br/pkg/task/restore.go` (keyspace rewrite), `br/pkg/restore/log_client/client.go`; master `br/pkg/task/backup*.go` |
| `cloud-engine`: state, size, divergence | Fork point `1fb8980cc` (2022-05-19, v6.1.0-alpha+102), merged to 6.1.1, tip 2022-09-26; 418 commits (2021-07-28 to 2022-09-26), mostly two authors. About 1.4 MiB of Rust in `kvengine`, `rfengine`, `rfstore`, `cloud_server` and `kvenginepb`, plus changes across `raftstore`, `src/storage`, `backup-stream` and the coprocessor. Master has 2 046 commits since the fork | tikv clone, `origin/cloud-engine`; GitHub tree API sizes |
| TiCDC with keyspaces on classic | **Not supported** (§10) | ticdc `pkg/keyspace/keyspace_manager.go`, `pkg/upstream/upstream.go`, `pkg/config/changefeed.go` |
| Session migration for the gate | TiDB parses `SHOW SESSION_STATES` / `SET SESSION_STATES`; TiProxy migrates with exactly those statements | tidb `pkg/parser/misc.go`, `ast/misc.go`; tiproxy `pkg/proxy/backend/backend_conn_mgr.go` |
| Config defaults (v8.5.8) | `split-table = true`, `lite-init-stats = true`, **`force-init-stats = true`**; `keyspace-name` or env `KEYSPACE_NAME`; `proxy-protocol` section present | tidb v8.5.8 `pkg/config/config.go` |
| Bootstrap size | About 59 `CREATE TABLE IF NOT EXISTS` system tables in `pkg/session` (master) | tidb clone |
| S-A hooks | `tikv/rust-rocksdb` offers `FileSystemInspector` (byte accounting) and an encrypted env, not a pluggable `FileSystem` | rust-rocksdb `src/file_system.rs`, `librocksdb_sys/crocksdb/c.cc` |

## 24. Contradictions with earlier decisions, and how they are resolved

| Earlier | Conflict | Resolution |
|---|---|---|
| **D260** "TiKV only: no TiDB anywhere" | TiDB is Loams SQL's compute | **Amended by D721** (owner directive 2026-10-08): TiDB is allowed as Loams SQL compute only; the metastore, Live, jobs and the durable store stay TiKV-only; no shared TiDB, no TiDB without `keyspace-name` (so no second GC worker, §20 R1 rows R6, X2) |
| **D301** "the MySQL shard is WeSQL", "InnoDB semantics over TiKV is TiDB rebuilt" | Loams SQL *is* TiDB on TiKV | **Superseded by D721** |
| D302, D320 (Vitess front ends WeSQL; vtgate replaces N6) | No Vitess | Superseded for Loams SQL; the gate fronts `tidb-server` (D731). §31's Vitess material stays as history |
| Earlier D721–D727, D735 (stock MySQL 8.4, mywal, Vitess, XtraBackup) | Withdrawn | Rewritten in place (same numbers, still Proposed) |
| D123 (TiDB SQL coexistence), superseded by D260 | Partly revived | Only §20 §10's facts are reused; Live tables stay invisible to SQL |
| D237, D264, D714 (Loams WAL on TiKV / Arm A for Postgres) | Not used for SQL | Unchanged for Postgres; §8.4 explains why they do not apply |
| D669 (desktop WeSQL page) | Moves to `loams.sqldb.v1` | Amended at SQ1e |
| D11, D126 (licences) | — | Satisfied: every component is Apache-2.0 |

## 25. Sources
- **This repository** at `6079e4d7` (2026-10-08):
  - `deploy/tikv/{compose.yaml,pd.toml,tidb.toml,tikv.toml}`;
  - `docs/design/20-reactive-database-on-tikv.md` §9.3, §10;
  - `docs/plans/r1-dependency-spike.md` (v8.5.8 keyspace and GC findings);
  - `crates/loams-meta-tikv/src/keys.rs`;
  - `apps/desktop-electron/src/main/{sql,stacks}/*`;
  - `crates/loams/src/api/connect.rs`;
  - PG2's Task 0 rulings (branch `backend/pg2`, `7a1b2fd4`).
- **PingCAP documentation (v8.5):** MySQL compatibility, transaction isolation levels, pessimistic transactions, foreign keys, SAVEPOINT; schema cache (v8.4 archive).
- **PingCAP on TiDB X:** docs.pingcap.com/tidbcloud/tidb-x-architecture; blog "The Making of TiDB X" (2025-12-16); blog "Multi-Tenant Agent Database: Inside TiDB X's Architecture" (2026-07-09, the open-source statement).
- **GitHub metadata** (releases, licences, branches, PRs) for `pingcap/{tidb,ticdc,tiproxy}`, `tikv/{tikv,pd,rust-rocksdb}`, `rockset/rocksdb-cloud`; Docker Hub tags for the v8.5.8 images.
- **Forum:** ask.pingcap.com thread #7137 (init-stats startup time).

---

## Appendix A. Alternatives considered

### A.1 Stock MySQL 8.4 + mywal on TiKV + Vitess (the first 2026-10-08 draft)
This was InnoDB unchanged, with every commit made durable by mywal acting as a lossless semi-sync replica that stored the binlog on TiKV, routed by Vitess v24. Task 0 had verified the following before the direction changed:
- **MySQL 8.4.** 8.4.11 is current. Oracle's support runs to 2029-04-30 premier and 2032-04-30 extended (endoflife.date). The maximum `rpl_semi_sync_source_timeout` is 4294967295 ms, and the source still falls back to async when it expires. 8.4 still ships the deprecated `semisync_master` plugin beside `semisync_source`. Crash recovery rolls back prepared transactions whose XIDs are not in the latest binlog file (manual).
- **Vitess v24.0.3, at the tag.** It is not true that "durability policy `none` leaves semi-sync alone": vttablet's `convertBoolToSemiSyncAction` turns the source semi-sync **off** on `InitPrimary`, `PromoteReplica`, `ChangeType` and other actions whenever the policy says no (`go/vt/vttablet/tabletmanager/rpc_actions.go`). That made the policy `semi_sync` mandatory.
- **XtraBackup 8.4.** It is numbered `8.4.0-N` independently of the server minor; GPL-2.0.
- **`mysql_common` 0.37.3.** It already covered the replication packets and binlog events mywal needed, except the semi-sync event prefix and `HEARTBEAT_LOG_EVENT_V2`.

**Why it lost:** GPL processes on the path, a Loams-built log protocol on the commit path, per-shard volumes (not bottomless), slow cold resume (30–60 s at 10 GiB), and three moving systems (MySQL, mywal, Vitess) instead of one.

### A.2 PolarDB-X (studied briefly, the second direction)
Facts checked on 2026-10-08 (GitHub metadata):

| Component | Release | Licence | Notes |
|---|---|---|---|
| CN `polardbx-sql` | `5.4.21-20260918` (2026-09-20) | Apache-2.0 | Releases roughly yearly (5.4.19: 2024-05 to 2025-08) |
| DN `polardbx-engine` | `2.5.0` (2026-09-15) | GPL-2.0 (Oracle's MySQL licence book) | **MySQL 8.0.42 base** (`MYSQL_VERSION` on branch `polardbx-8.0.42`); MySQL 8.0 reached end of life on 2026-04-30 |
| CDC `polardbx-cdc` | `5.4.21-20260918` | **SSPL-1.0** (relicensed 2024-11-11) | Incompatible with D11/D126 for a hosted service |
| Operator | `v1.7.1` (2026-09-23) | Apache-2.0 | After a two-year gap from v1.7.0 (2024-09-25) |
| Backup | `2.5.0` | GPL-2.0 (XtraBackup fork) | — |
| Columnar | **No public repository** | — | Cloud only |
| `polardbx-glue` | — | Connector/J licence (GPL-2.0 with the FOSS exception) | Inside the CN build |

**Why it lost:** SSPL CDC, a DN on an end-of-life MySQL 8.0 base (not 8.4), closed columnar, no object-storage row store, and the owner's later directive.

### A.3 Wait for TiDB X
TiDB X would deliver bottomless storage, CoW and stateless TiKV, but it is closed today. PingCAP's stated plan to open-source it by the end of 2026 is a reason to sequence the S3 work (Q662), not to wait with everything else.
