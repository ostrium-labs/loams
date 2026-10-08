# 47 — Loams SQL in Production: Serverless MySQL 8.4 with mywal on TiKV, Routed by Vitess

Status: **Proposed** · 2026-10-08, revised the same day for the owner's directives. The direction is the owner's:

> "Loams SQL production ready." — "loam sql should be a serverless mysql db on s3 with vitess router". — "for loams sql use latest 8.4 and use TiKV for innodb like transaction, i want mywal".

This document turns that into decisions **D720–D739** and open questions **Q655–Q669**, recorded in the [decision log](13-decision-log.md) with status "Proposed". Plan: [SQ1](../plans/2026-10-08-sq1-loams-sql-production.md). **No code is written by this document.**

It builds on [§28](28-loams-postgres.md) (the Loams WAL on TiKV: D234, D237–D240, the §7 gate and its laptop results), [§31](31-loams-router-and-verification.md) (the router control plane, Vitess, the verification program), [§29](29-wesql-oltp.md) and [§23](23-neon-and-wesql.md) (WeSQL, which this document stops using for Loams SQL), [§19](19-console-identity-and-agents.md) and [§38](38-knative-authentik-gitops.md) (identity, MT1's verifier), [§44](44-unified-api-and-sdks.md) (Connect conventions) and [§37](37-desktop-and-mobile-apps.md) §19 (the desktop's WeSQL page, D669).

Markers, as in §23, §28, §29 and §31:

- **(verify)** means not checked against a primary source, or checked only by reading code that was not run. The task that depends on it checks it first.
- **(estimate)** means computed or reasoned, not measured. Every number in §16 is an estimate until SQ1 Task 1 replaces it with a measured baseline.
- **(spike)** means measured in an earlier spike: §23 §9, §28 §7.1 and §7.3 (laptop, not gate data), §31 §22.

---

## 1. Summary

**Loams SQL is a serverless MySQL 8.4 LTS database whose durable state lives in TiKV and the bucket, never only on a local disk.** Each database is one or more shards of **stock MySQL Community 8.4 with InnoDB** (Oracle's GPL-2.0 server, unmodified, a separate process Loams never links), fronted by **Vitess** (vtgate and unmanaged vttablets, Apache-2.0, unmodified). Every commit is made durable by **mywal**, a Loams-owned Apache-2.0 service that receives the server's **binlog** as a **lossless semi-synchronous replica**, writes it to a **TiKV quorum hot tier** before acknowledging, and offloads it to the **bucket** as the cold tier. Physical snapshots go to the bucket too, so a database is fully recoverable — to any point in its window — from TiKV and the bucket alone. A small Loams **gate** terminates TLS and authentication and wakes suspended databases; Loams’ **control plane** (`loams.sqldb.v1`) owns databases, branches, roles, lifecycle, failover, backups and restores.

| # | Decision (short) |
|---|---|
| D720 | **What Loams SQL is**: serverless MySQL 8.4 + InnoDB + mywal on TiKV + bucket, routed by Vitess. "Engine query SQL" (DataFusion) is a different thing |
| D721 | **Stock MySQL 8.4 LTS from day one, not a WeSQL rebase**; WeSQL leaves the Loams SQL path. Answers Q669 |
| D722 | **mywal ships the binlog, not the redo log** |
| D723 | **mywal's protocol is MySQL's own lossless semi-sync replication** (`AFTER_SYNC`); the server waits for mywal's ACK, which mywal sends only after TiKV has the events |
| D724 | **mywal's storage**: TiKV fenced 1PC appends (D237's shape) in keyspace `loams_mywal`, a bucket cold tier, trim and RPO rules; a shared `loams-walstore` crate with `loams-safekeeper` |
| D725 | **Fencing, crash recovery and failover through mywal**: terms on the stream head; replicas fed from mywal; local binlog truncation before any restart |
| D726 | **The mywal performance gate must pass before GA**; a failed gate does not silently switch designs |
| D727 | **Snapshots, PITR and branches** from physical snapshots in the bucket plus mywal's binlog; restore always into a new branch |
| D728 | **Control plane API `loams.sqldb.v1`** with operations as Resonate sagas |
| D729 | **Resource model**: database = Vitess keyspace; branch = its own keyspace; shard = one MySQL primary (+ replicas) |
| D730 | **`loams-sqlrouter` is the control plane's kernel, never a data path** |
| D731 | **The gate (`loams-sqlgate`)** in front of vtgate: TLS, auth, wake, accounting; packets only, no SQL parsing |
| D732 | **Authentication, authorization and TLS** |
| D733 | **Serverless lifecycle** with warm and cold resume targets |
| D734 | **Compute classes and storage limits** |
| D735 | **Vitess configuration**: a release that supports MySQL 8.4, `transaction_mode = SINGLE` at GA, semi-sync-backed Vitess 2PC evaluated after GA |
| D736 | **Kubernetes**: Loams’ own controller; vtgate, vtctld, etcd and mywal from GitOps manifests |
| D737 | **Single-node desktop mode** on the same API; the AP1e page moves to it |
| D738 | **Observability, quotas, isolation and hardening** |
| D739 | **The GA bar is evidence**; engine query SQL is its own milestone and the engine's `mysql-wire` listener is removed |

## 2. What Loams SQL is, and what it is not (D720, D721)

### 2.1 The product

| | Loams SQL | Loams Postgres | Loams Live | Engine query SQL |
|---|---|---|---|---|
| What | Serverless MySQL 8.4 OLTP | Serverless Postgres (Neon fork) | Reactive application database | SQL over Loams’ own objects |
| Engine | Stock MySQL Community 8.4 LTS, InnoDB | Neon compute + pageserver | TiKV | DataFusion inside `loams` |
| Router | Vitess vtgate + vttablet | PgDog | — | — |
| Durable log | **mywal**: binlog on TiKV, then the bucket | stock safekeepers at GA; `loams-wal` Arm A behind its gate (D714) | TiKV | the Loams log |
| Design | this document, §31 | §28, §46 | §20 | §05, §44 |

"Engine query SQL" names the DataFusion surface (REST `/v1/namespaces/{ns}/sql`, Flight SQL, the read-only pg and MySQL listeners, `loams.sql.v1`) everywhere from this document on (§17).

### 2.2 The engine: stock MySQL 8.4, not WeSQL rebased onto 8.4 (D721)

The owner asked for "latest 8.4" and "InnoDB-like transactions". Two ways to get both were evaluated.

| | **A. Rebase WeSQL (SmartEngine on S3) onto 8.4** | **B. Stock MySQL 8.4 + InnoDB + mywal + bucket snapshots (chosen)** |
|---|---|---|
| Transactions | SmartEngine: RC and RR only, first-committer-wins, point locks, no gap locks, no SERIALIZABLE, no `ROLLBACK TO SAVEPOINT` after writes, untested XA, no foreign keys (§29 §4). Each needs fork work (WS1, T2, …) | **InnoDB itself**: every isolation level, gap and next-key locks, foreign keys, savepoints, XA, atomic DDL. Nothing to build |
| Work in a GPL fork | Port SmartEngine's handler and WeSQL's object-store and recovery patches from 8.0.46 to 8.4's server APIs, then WS1, T2 and WS2's client on top; every 8.4 minor re-merged | **None.** mywal speaks the replication protocol from outside the server (§5) |
| Upstream | WeSQL: one company, dormant April 2025–July 2026, HA removed in August 2026 (§23 §4.2) | Oracle's 8.4 LTS with quarterly security releases until its end of life (Oracle's lifecycle policy; verify the date in Task 0) |
| Vitess | RT0 found 158 `differs` rows on WeSQL (§31 §22.4) | MySQL with InnoDB is Vitess's native target; Vitess v24 supports 8.0 and 8.4 (§31 §3) |
| Storage on the bucket | Extents live on S3 ("bottomless") | Data pages live on the pod's volume; the bucket holds snapshots and the binlog, TiKV holds the tail. A database is bounded by its volume (§11, Q667) |
| Cold start | Restore from S3 measured 75–90 s for *initialization* (§23 §9.2); resume not measured | Download the snapshot's files (§10.3); estimate 30–60 s at 10 GiB |
| Durability | WS2 needed a new GPL client in the fork | mywal (§4–§6), Apache-2.0, no server change |

**B is chosen.** A is feasible but strictly worse on every axis the owner named: it would spend most of the track rebuilding, inside a GPL fork, transaction features InnoDB already has. "TiKV for InnoDB-like transactions" is read as **TiKV as the transactional durability tier under InnoDB's own transactions**. The other reading — a MySQL storage engine whose rows live in TiKV — is TiDB rebuilt, which D260 and D301 rule out.

**What happens to WeSQL.** D273/D409 (WeSQL as the MySQL-on-the-bucket engine) and D301's "the MySQL shard is WeSQL" are **amended for Loams SQL** by D721: Loams SQL's shards are stock MySQL 8.4. WeSQL stays a development option (D156), §29's WS1–WS3 leave every Loams SQL plan, and WS4's Iceberg bridge reads the binlog from mywal instead (§5.4). §29 itself is not deleted; whether WS1–WS3 are ever built becomes a separate owner question (Q657).

### 2.3 Non-goals

- **A MySQL fork, a new storage engine, a SQL parser, planner or router data path** (D300). If a server-side change ever becomes unavoidable, it lives in a GPL fork `ostrium-labs/mysql-server` and speaks to Loams only over a documented protocol (D11, D148).
- **Distributed transactions across shards** in v1 (D412), and never across engines (D306).
- **Bottomless storage per database** in v1 (§11).
- **Live vertical resizing** of a running `mysqld`, and **cross-region replication**, in v1 (Q666).

### 2.4 What exists today (checked 2026-10-08 on `dev`)

| Piece | State |
|---|---|
| `deploy/wesql/` | One WeSQL 8.0.35 beta on RustFS; development only. Not used by Loams SQL after D721 |
| `crates/loams-safekeeper` | Neon's safekeeper protocol over a `WalStore` trait with fenced TiKV 1PC appends (`tikv.rs`, keyspace `loams_pgwal`), Arm A's NVMe journal, offload and trim. mywal reuses its store layer (§6.1) |
| §28 §7.1 (spike, laptop) | TiKV on the commit path: `commit-1` p50/p99 6.5/43.1 ms against a safekeeper's 2.9/20.0 ms; the levers not yet in: pipelined appends, no PD round trip per append, PLP NVMe |
| §31 RT0 | Done: specs, Lean ranges, `loams-sqlrouter`, `loams-compat`, inventories (Vitess against MySQL 8.0.46 and WeSQL) |
| MT1 | Planned, not built |
| Desktop | AP1e Tasks 21–23 run `deploy/wesql` and talk to it as root through `mysql2` (D669) |
| Engine `mysql-wire` listener | Read-only SELECT over collections; removed by D739 |

## 3. Architecture

```
 MySQL clients ── TLS ──► loams-sqlgate ×N ── TLS ──► vtgate ×N ──► vttablet ── unix socket ──► mysqld 8.4 (InnoDB)
                          (auth, wake,                (Vitess)        (sidecar, D324)            primary of a shard
                           accounting)                                                                 │
                                                                                    binlog dump, semi-sync (AFTER_SYNC)
                                                                                                       ▼
                 replicas (mysqld 8.4 + vttablet) ◄── binlog dump ── mywal ×N (stateless pool, Apache-2.0)
                 VReplication, CDC, WS4 bridge    ◄──────────────────  │ fenced 1PC appends (term on the head)
                 recovery and PITR replay         ◄──────────────────  ▼
                                                               TiKV (keyspace loams_mywal; Raft quorum across AZs)
                                                                       │ offload every 250 ms (lease)
                                                                       ▼
                                                   bucket: mysqlbinlog/<ns>/<db>/<branch>/<shard>/… and snapshots/…
                                                                       ▲
 Loams control plane (`loams`, feature `sqldb`): loams.sqldb.v1 · metastore records · sagas on Resonate ·
 loams-sqlrouter machines · runtime drivers (kubernetes | local) · renders Vitess, mysqld and mywal configuration
```

**Commit path.** The client's `COMMIT` reaches `mysqld` through the gate, vtgate and vttablet. InnoDB prepares; the binlog group is written and synced locally (`sync_binlog = 1`); the semi-sync source plugin then **waits** (`AFTER_SYNC`) until a semi-sync replica acknowledges the group's end position. mywal is that replica: it receives the events on its dump connection, appends them to TiKV in one fenced transaction, and only then sends the ACK. InnoDB commits; the client is answered. A transaction is therefore acknowledged only once it is on a TiKV Raft quorum, and no other session can see it earlier, because the engine commit waits too (lossless semi-sync).

## 4. mywal: what it is (D722)

### 4.1 Binlog, not redo

| | **Binlog (chosen)** | InnoDB redo log |
|---|---|---|
| A hook that makes the commit wait, without a fork | **Yes**: the semi-sync source plugin's `AFTER_SYNC` wait, shipped with MySQL 8.4 | No. Redo is written by InnoDB's internal log writer; shipping it before the commit needs a patched server |
| Order and boundaries | The transaction coordinator's order, GTIDs, explicit transaction boundaries (`GTID … XID`) | Physical mini-transactions, no transaction boundaries visible outside InnoDB |
| Consumers | Replicas, Vitess VReplication and VDiff, CDC (Debezium, D357), the WS4 bridge, `mysqlbinlog` PITR | InnoDB of the same version only |
| Version tolerance | Logical row events (`binlog_format = ROW`, `FULL` image) | Format changes across minor versions (8.0.30 changed the redo files) |
| Recovery speed | Replay is slower than redo apply; bounded by snapshot cadence (§10) | Fast |

### 4.2 Shape

- A **stateless pool** of mywal instances (like D239's WAL pool): all state is in TiKV and the bucket, so any instance serves any stream and a crash is a reconnect.
- **One stream per shard primary lineage**: `stream_id = SHA-256("mysql/" + ns + "/" + db_id + "/" + branch_id + "/" + shard)`; offsets are a **virtual binlog offset** (cumulative bytes since the stream's origin across files, as §29 §6.3 defined for WeSQL), with the `(file, position)` map kept from the `ROTATE` events.
- **Two mywal sessions per primary**, from instances in different AZs, both registered as semi-sync replicas; the source waits for **one** ACK (`rpl_semi_sync_source_wait_for_replica_count = 1`). Either session's ACK means the events are on TiKV's quorum; the second session keeps commits flowing when one mywal instance dies.
- **Crate**: `crates/loams-mywal` (Apache-2.0), built into the `loams` binary behind the feature `mywal` and also as a standalone binary `loams-mywal` for its own pods. It links nothing GPL: it implements the replication protocol from MySQL's public protocol documentation, with `mysql_common` (MIT/Apache-2.0) for packet and binlog event framing if its event coverage suffices (Task 0, verify).

## 5. mywal's protocol (D723)

### 5.1 To the primary: a lossless semi-sync replica

| Step | What mywal does |
|---|---|
| Connect | To the primary's `mysqld` (through a dedicated replication user `mywal_<shard>` with `REPLICATION SLAVE` only, over TLS), never through vtgate |
| Register | `COM_REGISTER_REPLICA` with a server id from the control plane (`mywal` ids are a reserved range) |
| Enable semi-sync | `SET @rpl_semi_sync_replica = 1` (the variable the source plugin checks for a semi-sync replica; verify the 8.4 name) and the checksum and heartbeat session variables (`@source_binlog_checksum`, `@source_heartbeat_period`) |
| Dump | `COM_BINLOG_DUMP_GTID` from the stream's committed GTID set in TiKV (§6) |
| Receive | Events with the semi-sync header; for events flagged "needs ACK", mywal appends every event received so far to TiKV (one fenced transaction, group commit) and then sends the semi-sync ACK packet with the binlog file and position of the last durable event |
| Refuse | If its append is fenced (a higher term exists, §7), it sends no ACK and closes the connection |

**Source settings** (rendered by the control plane, `mysqld` 8.4):

| Setting | Value | Why |
|---|---|---|
| `plugin-load-add` | `semisync_source.so` | 8.4 ships only the `source`/`replica` plugin names (verify) |
| `rpl_semi_sync_source_enabled` | `ON` | — |
| `rpl_semi_sync_source_wait_point` | `AFTER_SYNC` | Lossless: no session sees a transaction before it is durable in TiKV |
| `rpl_semi_sync_source_wait_for_replica_count` | `1` | Either mywal session's ACK is enough (§4.2) |
| `rpl_semi_sync_source_timeout` | the maximum the server accepts (verify the 8.4 range) | Semi-sync **falls back to asynchronous replication when the timeout expires**. With the maximum, a primary that cannot reach mywal blocks commits instead of acknowledging undurable ones |
| `rpl_semi_sync_source_wait_no_replica` | `ON` | Keep waiting when no semi-sync replica is connected |
| `sync_binlog`, `innodb_flush_log_at_trx_commit` | `1`, `1` | The local copy stays crash-consistent with InnoDB (§7.2) |
| `gtid_mode`, `enforce_gtid_consistency` | `ON`, `ON` | Stream positions and replica catch-up by GTID |
| `binlog_format`, `binlog_row_image` | `ROW`, `FULL` | Vitess (§31 §9.1), CDC, WS4 |
| `binlog_transaction_compression` | `ON` (zstd) | Fewer bytes to TiKV and the bucket (measure in Task 13) |
| `replica_preserve_commit_order`, `replica_parallel_workers` | `ON`, by class | Replicas apply from mywal in order, in parallel (WRITESET) |

**The asynchronous-fallback guard.** Even with the maximum timeout, the control plane polls `Rpl_semi_sync_source_status` every second; if it is ever `OFF` on a primary, the shard is set `super_read_only = ON` and an alert fires. A shard never runs with acknowledged commits that are not in mywal.

### 5.2 To readers: mywal as a binlog source

mywal also serves the stream as a **replication source** (`COM_BINLOG_DUMP_GTID` server side, from TiKV and then the bucket) to: replicas (§7.3), recovering primaries (§7.2), Vitess VReplication (verify that a vttablet can stream from a non-tablet source; otherwise VReplication reads the primary as usual), CDC and the WS4 bridge. Readers therefore never see a transaction that is not durable.

### 5.3 What mywal never does

It never parses SQL, never applies events, never decides a commit's outcome. It stores and serves bytes in order, at a term.

### 5.4 Relation to `loams-wal` and the safekeeper protocol

mywal does not speak Neon's safekeeper protocol: MySQL's own replication protocol is the boundary, so the server needs no plugin of Loams’ making and nothing GPL is written. It shares the **store layer** (fenced appends, offload, trim, leases) with `loams-safekeeper` through a new crate `loams-walstore` (§6.1).

## 6. mywal's storage: TiKV hot tier, bucket cold tier (D724)

### 6.1 The shared store

`crates/loams-walstore` is extracted from `loams-safekeeper` without behaviour change: the `WalStore`-style trait whose every mutating call is one atomic step that re-checks the term against the stored head (`store.rs`), the pure transition rules, `MemWalStore`, the TiKV backend (`tikv.rs`: one optimistic 1PC transaction per call that reads and writes the head), and the offload, trim and lease helpers. The head type is a parameter, so Postgres's `AcceptorState` and mywal's `StreamHead` are both stored by it. If the extraction cannot be done without behaviour change on the Postgres side, mywal depends on `loams-safekeeper`'s store module directly and the extraction waits (Task 12 decides).

### 6.2 Keys (keyspace `loams_mywal`, on the Loams TiKV cluster with its own stores by placement rules, as Q116)

| Key | Value |
|---|---|
| `H ‖ stream` | `StreamHead { term, primary_epoch, committed_end (virtual offset), committed_gtids, file_map, backup_end, trim_floor }`, postcard |
| `W ‖ stream ‖ begin (u64 BE)` | Binlog bytes `[begin, begin + len)`, at most 128 KiB per chunk, whole events only |
| `B ‖ stream` | The offloader lease and `backup_end` (D269's shape) |

One stream's head and tail are pre-split into one region so appends are 1PC (D238). An append is **idempotent by offset**: the second mywal session writing the same events finds them stored and only advances nothing, so two sessions never duplicate.

### 6.3 Durability and retention

- **Ack rule**: mywal ACKs position *p* only after a TiKV transaction that wrote every byte up to *p* has committed; TiKV commits only after a Raft quorum (`sync-log` is always on since v5.0, §28 §6.6).
- **Offload**: one offloader per stream (lease `B`) writes committed bytes every **250 ms** to `mysqlbinlog/<ns>/<db_id>/<branch_id>/<shard>/<begin>-<end>.lbin` (header, zstd body, CRC32C trailer, `loams-log` conventions, as D269) and records `backup_end`.
- **Trim**: TiKV chunks below `min(backup_end, the oldest position any registered reader still needs)` are deleted. Bucket objects are kept for the PITR window (§10).
- **RPO**: 0 for the loss of any TiKV store or AZ (Raft quorum); **≤ 250 ms** for the loss of the entire TiKV cluster (the offload interval); 0 for the loss of a primary's volume.
- **Placement**: per-stream placement rules put the region leader in the primary's AZ and followers in two others (§28 §6.6), so one ACK costs one cross-AZ round trip plus TiKV's apply.

### 6.4 Latency budget (estimate, as §28 §6.6)

| Step | Estimate |
|---|---|
| `mysqld` → mywal (in AZ), event delivery | 0.1–0.2 ms |
| TSO for `start_ts` | 0 (reuse the previous commit timestamp, §28 §7.1 lever 2) to 0.5 ms |
| Fenced head read + prewrite/commit (1PC) with a cross-AZ follower | 0.8–2.0 ms |
| ACK back to `mysqld` | 0.1 ms |
| **Added to a commit, p50** | **about 1.0–2.8 ms**, on PLP NVMe |

The laptop spike of §28 §7.1 measured 6.5 ms p50 / 43 ms p99 per TiKV append on a consumer SSD with every lever missing. **That is the risk this design carries**, and the gate in §8 is where it is settled.

## 7. Fencing, crash recovery and failover (D725)

### 7.1 Terms

The stream head carries `term`. The control plane bumps it (a fenced compare-and-set in TiKV) whenever the primary changes or a primary is restarted after a crash, and tells mywal sessions the current term. **An append at a lower term conflicts on the head and fails**, so a deposed primary's events are never acknowledged: its commits block in the semi-sync wait and are never answered. The control plane then stops it (and vtgate's buffering hides the switch, §15).

### 7.2 Restarting a primary on its own volume (crash, suspend, upgrade)

Before `mysqld` starts, an init step (`loams-mywal reconcile-local`, Apache-2.0) compares the local binlog with the stream:

- **Local binlog ahead of mywal** (events written and synced locally whose ACK never came): the local binlog is **truncated at the event boundary of mywal's `committed_end`**, later files removed and the index rewritten. MySQL's crash recovery then rolls back every InnoDB-prepared transaction whose XID is not in the binlog (the binlog is the transaction coordinator; verify on 8.4 in Task 14). Those transactions were never acknowledged, so nothing acknowledged is lost.
- **Local binlog behind mywal, or the volume is gone**: the server starts from the newest snapshot (§10) or its volume, then **replicates from mywal** (§5.2) until it reaches `committed_end`, and only then opens for writes.

`reconcile-local` works on binlog files with public framing; it does not link or modify MySQL.

### 7.3 Replicas and failover

- **Replicas replicate from mywal, never from the primary**, so a replica never holds a transaction that is not durable, and any replica is a correct promotion candidate once it has applied up to `committed_end`.
- **Promotion** (a Resonate saga, the `PrimaryFailover` machine of §31 RT4 filled in for mywal): (1) mark the primary suspect (missed health checks, lease, operator); (2) **bump the term**, which fences the old primary at mywal; (3) the candidate applies from mywal up to `committed_end`; (4) `STOP REPLICA; RESET REPLICA ALL; SET GLOBAL super_read_only = OFF`; (5) mywal sessions connect to the new primary at the new term; (6) `vtctldclient TabletExternallyReparented` and the records' compare-and-set; (7) the old primary, when it returns, is reconciled (§7.2) as a replica or rebuilt.
- **Non-HA shards** (no replica): a new pod restores the snapshot and catches up from mywal (cold), or the same pod restarts with `reconcile-local` (warm).

### 7.4 What this gives the owner's "InnoDB-like transactions"

InnoDB's own ACID semantics on the primary; commit durability equal to a TiKV Raft quorum; no acknowledged transaction lost on a crash, a volume loss, an AZ loss or a failover; no transaction visible before it is durable.

## 8. The mywal performance gate (D726)

Run on the §16 reference topology (three nodes, PLP NVMe, real or `netem` cross-AZ delays, TiKV with §28 §6.6's tuning and the levers of §28 §7.1 in place). Baselines on the same hardware: (a) MySQL 8.4 with `sync_binlog = 1` and no replica; (b) MySQL 8.4 with lossless semi-sync to **a stock MySQL 8.4 replica** in another AZ.

| Gate | Workload | Must hold (estimate until Task 1) |
|---|---|---|
| Commit latency | sysbench `oltp_insert`, 1 thread, 5 min | p99 ≤ 5 ms (the owner's Postgres target, §28 §6.6) **and** p99 ≤ 1.3 × baseline (b) |
| Group commit | `oltp_write_only`, 16 and 64 threads | TPS ≥ 0.9 × baseline (b); p99 ≤ 1.3 × (b) |
| Bulk | `LOAD DATA` of 10 GiB, and `mydumper`/`myloader` restore | binlog MB/s ≥ 0.7 × baseline (b) — §28's `bulk` failure is the known trap |
| mywal instance kill | Kill one mywal session's instance under `oltp_write_only` 64 threads | No commit waits longer than 1 s; zero acknowledged loss |
| TiKV leader kill | Kill the stream region's leader | Commit stall ≤ the tuned election time (target 2 s, §28 §6.6); zero acknowledged loss |
| Durability | Kill `-9` the primary and delete its volume, 200 runs at random points | Every acknowledged row present after recovery; no row applied twice |
| Fencing | Pause the old primary past a failover, resume it | It acknowledges no commit |

**The gate must pass before GA.** If it fails after the levers, the result goes to the owner with the measured numbers; the fallback the owner may choose is mywal on Arm A's local-NVMe acceptors (`loams-safekeeper`'s journal, D264–D269) behind the same replication protocol (Q666). The design does not switch on its own.

## 9. The control plane API: `loams.sqldb.v1` (D728)

§44 already allocates `loams.sql.v1` (SDK module `loams.sql`) to engine query SQL (D606, API1 Task 5). Loams SQL's data plane is the MySQL protocol; its RPCs are management calls, so it takes **`loams.sqldb.v1`** (SDK module `loams.sqldb`), Q655.

| Service | RPCs | Notes |
|---|---|---|
| `DatabaseService` | `CreateDatabase`, `GetDatabase`, `ListDatabases`, `UpdateDatabase`, `DeleteDatabase`, `SuspendDatabase`, `ResumeDatabase`, `GetConnectionInfo`, `WatchDatabase` | Mutations return a `loams.operations.v1` `Operation`. `GetConnectionInfo` returns host, port, schema, `tls_required`, the CA bundle and the SNI name, never a secret |
| `BranchService` | `CreateBranch`, `GetBranch`, `ListBranches`, `DeleteBranch` | `CreateBranch(parent, point)`: `latest`, a timestamp, or a GTID set inside the PITR window |
| `RoleService` | `CreateRole`, `ListRoles`, `DeleteRole`, `RotateRolePassword`, `CreateEphemeralCredential` | Roles `reader`, `writer`, `ddl`, `admin`; a password is returned once; ephemeral credentials last at most 1 h |
| `BackupService` | `GetRestoreWindow`, `ListSnapshots`, `ExportDatabase` | Export = `mydumper` to a caller-named prefix |
| `ShardingService` (beta) | `GetShardMap`, `UpdateVSchema`, `Reshard` | Vitess workflows as sagas |

`Database`: `name` (`[a-z][a-z0-9-]{0,62}`), `id` (`db_` + 16 base32), `environment`, `region`, `engine_version` (`8.4.x`), `state`, `compute` (`min_class`, `max_class`, `suspend_after`, `0s` = never), `ha.replicas` (0–2), `pitr_window`, `shards`, `storage_bytes`, `volume_limit_bytes`, `durability` (`mywal` | `local`, the latter only in the desktop mode), `stage` (`beta` | `ga`), `etag`. API authorization: `sqldb:read`, `sqldb:write`, `sqldb:delete`, `sqldb:credentials` (MT1); agents may mint only `reader`/`writer` ephemeral credentials.

**Records** (metastore; TiKV in clusters): `x/<ns>/<db_id>` (database), `X/<ns>/<db_id>/<branch_id>` (branch, per-shard primary, term mirror), `xs/…` (§31 §6.1 shard map, Vitess mirror), `xr/…` (roles; secrets in the credential store only). Prefixes are fixed in SQ1 Task 0 against `crates/loams-meta-tikv/src/keys.rs`. mywal's stream heads live in its own keyspace (§6.2), not here.

## 10. Snapshots, PITR and branches (D727)

### 10.1 Snapshots

- **Online**: Percona XtraBackup 8.4 (GPL-2.0, a separate job container, never linked; verify its 8.4 support and licence in Task 0) streams a physical backup to `snapshots/<ns>/<db_id>/<branch_id>/<shard>/<ts>/` on a replica when one exists, else on the primary; incrementals between fulls. Each snapshot records the GTID set it contains.
- **At suspend**: a clean shutdown's data directory is uploaded file by file (changed files only, by size and checksum), which is consistent without `--prepare` and is what cold resume downloads.
- **Cadence**: a full snapshot daily, incrementals every 15 minutes or 1 GiB of binlog (estimate; tuned so replay after a snapshot stays under a minute).

### 10.2 PITR

The window (default 7 days, 1–35, Q665) is `[oldest retained snapshot, committed_end]`. **Restore always creates a new branch**: restore the newest snapshot at or before the point, then replicate from mywal and the bucket's binlog up to the point (`START REPLICA UNTIL SQL_BEFORE_GTIDS`, or a timestamp mapped to a GTID by the stream's index), then open. Never in place.

### 10.3 Branches

v1 branches are **copies made by that restore** (D727): own keyspace, own stream, own compute. Copy-on-write branches would need page-level sharing that InnoDB on local volumes does not have; volume snapshots of the CSI driver (where available) can make branch creation faster without changing the model (a later optimisation).

## 11. Resource model, compute and storage (D729, D734)

- **Database = Vitess keyspace** `k_<db_id>_<branch_id>`; **branch = keyspace**; **shard = one MySQL 8.4 primary + 0–2 replicas**, each in its own pod with a vttablet sidecar (D324) and `mysqld_exporter`.
- **Size classes**

| Class | vCPU | Memory | Gate connections | vttablet pool | `innodb_buffer_pool_size` | Volume limit |
|---|---|---|---|---|---|---|
| `xs` | 0.25 | 1 GiB | 100 | 8 | 384 MiB | 50 GiB |
| `s` | 0.5 | 2 GiB | 200 | 16 | 1 GiB | 100 GiB |
| `m` | 1 | 4 GiB | 500 | 32 | 2.5 GiB | 250 GiB |
| `l` | 2 | 8 GiB | 1 000 | 64 | 5.5 GiB | 500 GiB |
| `xl` | 4 | 16 GiB | 2 000 | 128 | 11 GiB | 1 TiB |
| `2xl` | 8 | 32 GiB | 4 000 | 256 | 23 GiB | 2 TiB |

(Estimates, tuned in SQ1 Task 31.) **Data lives on the pod's volume**, bounded by the class's volume limit; the bucket holds snapshots and binlog, TiKV the tail. A database that outgrows `2xl`'s 2 TiB shards (beta) or waits for Q667.
- **Resize**: HA databases promote a replica started at the new class (§7.3, planned); non-HA databases resize by suspend and resume. No autoscaling in v1.
- **Bucket credentials per shard, prefix-scoped**, for snapshots and mywal's offload separately; bucket encryption at rest.

## 12. The gate: `loams-sqlgate` (D731)

Vitess cannot wake a suspended keyspace, verify Loams tokens or account idleness per database (verify vtgate buffering's scope in Task 0). §23 §6.3's N6 design (a Loams front end that completes the MySQL handshake itself) returns **in front of vtgate**, which stays the only router (amends D320).

1. **Greet** with the gate's own scramble, server version `8.4.x-Loams-SQL`, `caching_sha2_password` as the default plugin, no compression in v1.
2. **TLS** 1.2+ on `SSLRequest`, certificate by SNI (`<db>.<region>.sql.<domain>`); a non-TLS `HandshakeResponse` gets 3159 except on loopback in the desktop mode.
3. **Identify** the database: role users are globally unique (`u_…`, ephemeral `t_…`); for a Loams identity, SNI or the handshake schema, which must be in the token's environment.
4. **Authenticate** (§13).
5. **Wake**: if the branch is not `running`, call the control plane's `EnsureRunning` and hold the connection up to `resume_deadline` (30 s), else error 1040 `database is resuming, retry`.
6. **Connect upstream** to vtgate (TLS, verified) as the internal user of the client's role; advertise to the client only capabilities the upstream has.
7. **Relay** packets; refuse `COM_CHANGE_USER` (1235) and replication commands; count every command except `COM_PING` as activity.
8. **Enforce** per-database connection caps and connection rate, a 10 s handshake deadline, a per-pod cap.

The gate **never parses SQL**; cross-database access is stopped by vttablet's table ACLs. It runs in `loams` (feature `sqldb`, `--sqlgate-listen`, default `127.0.0.1:3306` in `loams dev`; non-loopback only with `[tls]` and `[auth]`, MT1's rule) and scales horizontally. Escape hatch: an always-on database with only password roles can be reached on vtgate directly.

## 13. Authentication, authorization and TLS (D732)

- **Role passwords**: 32 random bytes (base62), shown once, Argon2id hash in the credential store. `caching_sha2_password` full authentication over TLS with a per-pod fast-auth cache; `mysql_clear_password` opt-in; `mysql_native_password` refused (8.4 no longer loads it by default; Q658).
- **Loams identities**: an access token (JWT, §19 §5.3) or API key in the password field, user = role kind or `token`; verified by MT1's verifier (`env`, `scp` `sqldb:connect:<kind>`, revocation). Drivers that truncate long passwords use API keys (checked per driver, Q659).
- **Upstream**: the gate connects as `ri_<db_id>_<role>`, rendered into vtgate's static auth file; **authorization is four roles** — `reader` (`SELECT`, `SHOW`, `EXPLAIN`), `writer` (+ DML, transactions, savepoints, `GET_LOCK`), `ddl` (+ DDL), `admin` (+ `KILL` own, Online DDL control) — enforced by strict vttablet table ACLs per keyspace (flag names verify in Task 19). `GRANT`, `REVOKE`, `CREATE USER`, `SET PASSWORD`, `INTO OUTFILE`, `LOAD DATA LOCAL`, `INSTALL PLUGIN`, `CREATE FUNCTION … SONAME` and `ALTER VSCHEMA` through vtgate are refused by query rules. Row- and column-level grants are not in v1.
- **TLS on every hop**: client → gate, gate → vtgate, vtgate → vttablet, mywal → `mysqld` (replication over TLS), mywal → TiKV (TiKV's TLS), readers → mywal; cert-manager issued; plaintext only on loopback in the desktop mode.
- **Audit**: every connection's verified principal, every credential change, every auth failure (D100).

## 14. Serverless lifecycle (D733)

States: `CREATING → RUNNING ⇄ SUSPENDING → SUSPENDED → RESUMING → RUNNING`; `RESIZING`, `RESTORING`, `FAILING_OVER`, `FAILED`, `DELETING`.

- **Idle**: no command other than `COM_PING` for `suspend_after` (default 5 min; `0` disables). HA databases never suspend.
- **Suspend**: the gate holds new connections and closes idle ones (1053); `super_read_only = ON`; wait for in-flight transactions (30 s, then `KILL`); confirm mywal's `committed_end` equals the local binlog end; clean shutdown; upload changed files (§10.1); stop vttablet and the pod; tablet `DRAINED` or not serving (Task 19 decides); keep the volume for `warm_retention` (24 h).
- **Resume**: schedule (prefer the node with the volume); `reconcile-local` (§7.2); start `mysqld` (warm: own volume; cold: download the suspend snapshot, then catch up from mywal); vttablet serving; mywal sessions attach at the current term; release held connections.

| Target (estimate, replaced by Task 1) | Warm | Cold, 10 GiB |
|---|---|---|
| p50 connect-to-first-result | ≤ 2 s | ≤ 30 s |
| p95 | ≤ 5 s | ≤ 60 s |

Cold resume is dominated by download bandwidth and InnoDB's start; Q656 asks the owner to accept the measured numbers or fund faster paths (lazy restore, pre-warmed pools).

## 15. Vitess configuration (D735, D730)

- **The newest Vitess release that supports MySQL 8.4** and is at least 14 days old (v24 supports 8.0 and 8.4, §31 §3; Task 0 pins the exact release), unmodified, by image digest; dedicated etcd (Q301); one cell per region; vttablet sidecar per `mysqld` (D324), unmanaged.
- **`transaction_mode = SINGLE`** at GA (D412; D323's `MULTI` was for WeSQL and is superseded). With mywal acting as a semi-sync replica, the "two-pc is enabled, but semi-sync is not" refusal (§31 §9.2 C-2) may no longer apply; whether Vitess atomic 2PC can then be enabled is evaluated after GA (Q662).
- **Durability policy**: Vitess's `semi_sync` policy assumes replica *tablets* acknowledge; mywal is not a tablet, so the policy is `none` and the control plane manages the semi-sync settings itself (verify that vttablet does not reset them; Task 19).
- Strict table ACLs; buffering on for reparents; query timeouts and result-size limits by class; `ALTER VSCHEMA` disabled.
- **Authority** (D730): Vitess's topology for serving state; Loams’ record for the VSchema of keyspaces it creates (applied through vtctld; drift re-applied with an alert); Loams for primaries (the term in mywal and the records). `loams-sqlrouter` gains `Lifecycle`, `PrimaryFailover` (mywal variant) and Vitess `ReshardCutover` machines, trace-validated against TLA+ (D311). **It is never on the query path.**
- **Sharding** (2–64 shards, `hash`/`xxhash` vindexes) is beta until a nemesis resharding run under load is clean (Q661). Each shard has its own mywal stream.

## 16. Performance gates (D739, D726)

**Reference topology** (estimate, fixed in Task 1): three nodes of 16 vCPU, 64 GiB, PLP NVMe, 25 GbE, cross-AZ delay by placement or `netem`; TiKV and mywal on the same nodes, separate drives for TiKV's raft-engine; RustFS in-cluster; class `xl`. Baseline: MySQL 8.4 InnoDB on the same pod class with `sync_binlog = 1` and lossless semi-sync to a stock replica (§8's baseline b).

| Gate | Target (estimate) |
|---|---|
| **mywal gate** | All of §8 |
| Point read through gate + vtgate (`oltp_point_select`, 64 threads) | p99 ≤ 3 ms; gate overhead ≤ 0.2 ms p50 and ≤ 5 % throughput against vtgate direct |
| OLTP mix (`oltp_read_write`, 10 × 1 M rows, 64 threads, 30 min) | ≥ 0.85 × baseline TPS |
| TPC-C (100 warehouses, 64 threads, 30 min) | ≥ 0.85 × baseline tpmC; consistency checks pass |
| Connection storm (5 000 connections in 10 s) | All succeed or get 1040 by policy; gate RSS ≤ 1 GiB |
| Resume | §14 |
| Failover under the OLTP mix (1 replica) | writable ≤ 15 s; zero acknowledged loss over 200 runs |
| PITR restore of 10 GiB to a point 1 h ago | ≤ 10 min |
| Resharding 1 → 2 (beta) | VDiff clean; write unavailability ≤ 5 s |

With InnoDB as the engine, parity is a realistic goal; the remaining gap is mywal's ACK, which §8 bounds.

## 17. Engine query SQL is not Loams SQL (D739)

The DataFusion surface over collections is **engine query SQL**. Its production items are SQ1f: `loams.sql.v1` (API1 Task 5) with streamed JSON rows or Arrow IPC batches (answering Q608), cancellation and parameters; the desktop Data Studio SQL tab on it; and **removal of the engine's read-only `mysql-wire` listener** (proposed answer "no" to Q260, Q663), so Loams has one MySQL surface. The read-only Postgres listener stays PG1's.

## 18. The desktop (D737)

### 18.1 Single-node mode

`loams dev` with feature `sqldb` serves `loams.sqldb.v1` through the **`local` runtime driver**: the pinned `mysql:8.4.x` image under Podman or Docker with a local volume, the gate on `127.0.0.1:3306`, snapshots to the engine's local bucket. **mywal is off by default** (`durability = local`, shown in the UI); `[sqldb] local_mywal = true` runs it against a local TiKV (the desktop's existing `tikv` stack) for parity. Vitess is optional (`local_vitess = true`), off by default (Q668).

### 18.2 Contract for the AP1e page (replaces D669's compose-based WeSQL page)

| Need | Today (AP1e Tasks 21–23) | With `loams.sqldb.v1` |
|---|---|---|
| Availability | Stack phase from the compose manager | `GetInstance.api_versions` lists `loams.sqldb.v1` |
| List and create | One fixed WeSQL container | `ListDatabases`, `CreateDatabase` (class `s`, `suspend_after = 15m`) |
| Start and stop | `docker compose up/down` | `ResumeDatabase`, `SuspendDatabase`, state from `WatchDatabase` |
| Connection | Root password from the compose env | `GetConnectionInfo` + `CreateEphemeralCredential(kind, ttl ≤ 1 h)`; the root password is never shown |
| SQL console | `mysql2` as root, `runCapped`, write confirm | `mysql2` against the gate with an ephemeral `reader`; a `writer` credential only after the existing write-confirm dialog; the agent tool `reader` only |
| Restore, branch | — | `GetRestoreWindow`, `CreateBranch(point)` |
| Remote | Not possible | The same page against any server serving `loams.sqldb.v1` |

The page is renamed **Loams SQL**. The WeSQL compose path stays only while the active server lacks `loams.sqldb.v1`, and is deleted at SQ1's exit.

## 19. Kubernetes (D736)

Loams’ own controller (`kube-rs`, a runtime driver) owns one StatefulSet per shard (MySQL 8.4, vttablet, exporter; socket `emptyDir`), PVCs, per-shard Secrets and NetworkPolicies, and XtraBackup Jobs. vtgate, vtctld, etcd and the **mywal pool** (a Deployment across AZs) come from MT3's GitOps manifests. **The Vitess operator is not used**: it runs `mysqld` through `mysqlctld` with its own backups and reparenting, which would fight mywal's terms and §14's suspend (Q664). Knative is not used (TCP).

## 20. Observability, quotas, isolation and hardening (D738)

- **Metrics**: gate (`loams_sqlgate_*`: connections, handshakes, auth failures, resume wait, commands, bytes); control plane (`loams_sqldb_*`: operations, states, resume seconds by kind, storage); **mywal** (`loams_mywal_append_seconds`, `loams_mywal_ack_lag_bytes`, `loams_mywal_fenced_total`, `loams_mywal_offload_lag_seconds`, `loams_mywal_trim_floor`, `loams_mywal_sessions{state}`); Vitess's own; `mysqld_exporter` including `Rpl_semi_sync_source_*`.
- **Logs**: slow query log and vtgate query log through OTLP with **literals redacted**; error logs as-is. **Traces**: OpenTelemetry at the gate, the control plane's saga steps and mywal's append path. **Audit**: control-plane mutations, credentials, auth failures, fencing events.
- **Alerts**: semi-sync status `OFF` on any primary (page), mywal ACK lag, offload lag, TiKV region leader outside the primary's AZ, resume p95 over target, replica lag, PITR window short, storage at 90 % of the volume limit.
- **Quotas**: volume bytes (read-only at 100 % of the limit, warning at 90 %), branches (10), connections and connection rate (gate), query time and result size (vttablet), roles (50).
- **Isolation**: keyspace per branch; pod per shard; NetworkPolicy (vttablet socket, mywal, bucket only); per-shard bucket keys; per-database internal users; table ACLs.
- **`mysqld` hardening**: `local_infile = OFF`; `secure_file_priv` set to an empty unwritable directory; no `SUPER`, `FILE`, plugin or user-management privileges for application roles; strict `sql_mode`; `skip_name_resolve = ON`; `mysqlx = OFF`.

## 21. Conformance (D739)

- **mywal**: §8's gate; protocol tests against MySQL 8.4's own semi-sync behaviour (ACK positions, heartbeats, rotate, checksum, compression); `reconcile-local` against every crash point.
- **Vitess with MySQL 8.4**: the RT0 inventory re-captured against 8.4 (a new `conformance/router/vitess-mysql84-*.tsv`); the Vitess end-to-end subset (`vtgate/queries/*`, VReplication `MoveTables`/`Reshard`) at ≥ 95 % of the reference's pass count.
- **Clients and ORMs**: tier 1 — `mysql` 8.4 CLI, `mysqldump`, `mydumper`/`myloader`, Connector/J 9.x, go-sql-driver, `mysql2` (Node and Ruby), PyMySQL, `mysqlclient`, PDO MySQL, MySqlConnector; **Prisma, Django, Rails (ActiveRecord), Laravel** test suites at ≥ 98 % of their pass count against stock MySQL 8.4 with remaining failures allowlisted with causes; tier 2 (smoke) — Hibernate, SQLAlchemy, TypeORM, GORM, Sequelize, Doctrine, Workbench, DBeaver, TablePlus. Session `SET`s that make vttablet reserve connections are measured and documented per ORM.
- **Fault evidence**: `loams-nemesis` (D315) bank and list-append workloads for 2 hours with primary, mywal and TiKV kills, pauses, partitions between the primary and mywal, and clock skew; 10 000 suspend/resume cycles under pooled clients.

## 22. Versions and upgrades

- **MySQL 8.4 LTS**: the official `mysql:8.4.x` image (or Oracle's `community-server`), pinned by digest; minor upgrades within 8.4 roll replica-first then a planned reparent; non-HA databases take the new image at their next resume or in a maintenance window. Moving to the next LTS is a restore-into-branch, never in place.
- **Vitess** upgrades follow its documented component order (verify for the pinned release). **mywal** is versioned with `loams`; its stream format is versioned in the head and readers accept the previous version.
- **XtraBackup** tracks the MySQL minor it backs up (an XtraBackup older than the server refuses; Task 0 pins the pair).

## 23. Security review (D739)

| Surface | Threat | Mitigation and test |
|---|---|---|
| Gate parsing | Malformed handshakes, oversized packets, slowloris | Fuzzing; 10 s handshake deadline; packet caps |
| Gate auth | Stuffing, cross-environment tokens, timing leaks | Rate limits; `env` check; constant-time comparison; revocation; audit |
| mywal | A rogue or deposed primary appending; a rogue reader | Terms on the head; per-shard replication users over TLS; readers authenticated with read-only rights; mywal → TiKV TLS |
| Asynchronous fallback | Semi-sync silently disabled | Maximum timeout, `wait_no_replica`, the 1 s status guard to `super_read_only` (§5.1); test `semi_sync_off_turns_primary_read_only` |
| `reconcile-local` | Truncating an acknowledged transaction | Truncate only above mywal's `committed_end`; property test over random crash points |
| Cross-tenant | Qualified names into another keyspace | Table ACLs per keyspace; test `reader_of_db_a_cannot_select_db_b` |
| SQL privilege escalation | `GRANT`, plugins, UDFs, file statements | Query rules and hardening; a test per statement |
| Bucket | One shard reading another's prefix | Prefix-scoped keys; a wrong-prefix test |
| Licensing | GPL reaching Loams | MySQL, XtraBackup and Vitess are images; `cargo deny`; mywal implements the protocol from documentation |

## 24. Risks

| # | Risk | Mitigation |
|---|---|---|
| 1 | **TiKV on the commit path misses the gate** (§28 §7.1's laptop data: about 2× a local replica at p50) | Levers first (TSO reuse, group commit, placement, PLP NVMe); the gate decides; Q666 is the owner's fallback choice |
| 2 | **Semi-sync corner cases** (ACK position semantics, heartbeats, plugin name changes in 8.4, behaviour when both mywal sessions reconnect) | Protocol tests against the real 8.4 server in Task 13; the status guard |
| 3 | **Crash recovery with a truncated binlog** behaves differently than modelled on 8.4 | Task 14 tests every crash point before any lifecycle code depends on it |
| 4 | **Cold resume is slow for large databases** (full download) | Warm volume retention; snapshot cadence; Q656 |
| 5 | **Per-database size is bounded by the volume** (no bottomless storage) | Class limits; sharding; Q667 |
| 6 | **The gate becomes a bottleneck** | Packet relay only; overhead gate; vtgate-direct escape hatch |
| 7 | **ORM session settings defeat vttablet pooling** | Measured per ORM; documented settings |
| 8 | **Vitess's tablet manager fights the semi-sync settings Loams sets** | Durability policy `none`; Task 19 verifies |
| 9 | **One TiKV cluster carries metastore, Live, `loams-wal` and mywal** | Separate stores by placement rules (Q116); mywal's own keyspace and alerts |

## 25. Open questions

| # | Question | Needed by |
|---|---|---|
| Q655 | Package `loams.sqldb.v1` (proposed) or reassign `loams.sql.v1` to Loams SQL | SQ1 Task 3 |
| Q656 | Accept measured warm and cold resume times, or fund faster restore paths | SQ1 Task 1 report |
| Q657 | Are §29's WeSQL milestones (WS1–WS3) still wanted for anything now that Loams SQL uses stock MySQL 8.4 (D721); WS4's bridge moves to mywal | Founder, after SQ1a |
| Q658 | `mysql_native_password` per database for old drivers (re-enabling the plugin in 8.4), or never (proposed) | SQ1 Task 21 |
| Q659 | Tokens only through the password field (proposed), or also a vtgate auth plugin in a Vitess fork | SQ1 Task 21 |
| Q660 | Accept §16's targets, or set others, after Task 1's baselines | SQ1 Task 1 report |
| Q661 | When sharded keyspaces leave beta | SQ1 Task 46 |
| Q662 | `transaction_mode = SINGLE` at GA (proposed); after GA, evaluate Vitess atomic 2PC now that mywal is a semi-sync replica | SQ1 Task 24 |
| Q663 | Remove the engine's read-only `mysql-wire` listener (proposed; answers Q260 "no") | SQ1 Task 49 |
| Q664 | Could the Vitess operator own vtgate/vtctld only without managing `mysqld` | SQ1 Task 39 |
| Q665 | PITR window default (7 days) and maximum (35); Object Lock by default | SQ1 Task 35 |
| Q666 | If the mywal gate fails on TiKV after the levers: keep tuning TiKV, or put mywal on Arm A's local-NVMe acceptors behind the same replication protocol | Founder, SQ1 Task 18 report |
| Q667 | Is a per-database volume limit (2 TiB at `2xl`) acceptable for GA, or is bottomless storage required (which would reopen §2.2's option A or a page-server design) | Founder, before SQ1e |
| Q668 | Desktop defaults: mywal and Vitess off (proposed) or on for parity | SQ1 Task 9 |
| Q669 | ~~GA waits for the 8.4 rebase~~ Answered 2026-10-08 by the owner: MySQL 8.4 LTS from day one (D721) | Resolved |

## 26. Contradictions with earlier decisions, and how they are resolved

| Earlier | Conflict | Resolution |
|---|---|---|
| D301 "the MySQL shard is WeSQL"; D273, D409 | Loams SQL shards are stock MySQL 8.4 | **Amended for Loams SQL** by D721 (owner directive 2026-10-08); "Loams SQL names the product" stands |
| D276–D279 (WS2 ships the binlog to `loams-wal` through a GPL client in WeSQL) | mywal ships the binlog through MySQL's own semi-sync protocol into TiKV | D722–D724 replace WS2's design for Loams SQL; §29 is unchanged for WeSQL (Q657) |
| D264, D714 (Arm A local-NVMe acceptors for Postgres) | mywal uses TiKV on the commit path | Owner directive for MySQL; the Postgres decisions are untouched. Arm A is mywal's documented fallback only by owner choice (Q666) |
| D320 (vtgate replaces N6's front end) | The gate is a Loams front end | Amended: the gate sits in front of vtgate; vtgate is still the only router |
| D323 (`MULTI`) vs D412 (`SINGLE`) | — | `SINGLE` (D412, owner) |
| D306, D412, §31 §9.2 C-2 (no semi-sync, so no Vitess 2PC) | mywal is a semi-sync replica | No change at GA; Q662 evaluates after |
| D669 | The desktop page moves to `loams.sqldb.v1` | Amended once SQ1a lands |
| D286 (`full` includes `mysql-wire`) | D739 removes it | `full` loses `mysql-wire`, gains `sqldb` |
| D1 (bucket is the only durable truth) | TiKV holds the tail | As §28's D234: TiKV holds only the window not yet offloaded (≤ 250 ms) |
| Q260 (read-only MySQL over DataFusion) | D739 | Proposed answer "no" (Q663) |

## 27. Sources

- This repository at `db911cda` (2026-10-08): `crates/loams-safekeeper/src/{store,tikv,acceptor,service}.rs` and `Cargo.toml`; `crates/loams-sqlrouter`; `crates/loams-compat`; `conformance/router/`; `deploy/wesql/`; `crates/loams/src/{mysql_wire,api/sql.rs}`; `proto/loams/operations/v1`; `docs/plans/2026-10-08-ap1e-electron-desktop.md` (Tasks 9, 21–23, R0.10).
- Design: §19 §5, §23 §3.3, §6.2–§6.3, §9.2, §28 §6.4–§6.7, §7.1–§7.3, §29 §4, §6, §31 §1, §3, §4, §6, §9, §10, §17, §19, §22, §37 §19.10, §38, §44 §4–§7, §46 §9 (D714).
- MySQL 8.4 Reference Manual: semisynchronous replication (`AFTER_SYNC`, timeout fallback, plugin names), the client/server and replication protocols, binary log format and crash recovery with the binlog as transaction coordinator — every point marked (verify) is checked in SQ1 Task 0 or Task 13 against the pinned 8.4 release.
- Vitess documentation for the pinned release: unmanaged tablets, static auth, table ACLs, buffering, durability policies, `Reshard`/`SwitchTraffic`, query rules and upgrade order (verify in Task 19).
