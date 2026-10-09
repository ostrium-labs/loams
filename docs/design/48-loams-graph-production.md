# 48 — Loams Graph in Production: GQL on Grafeo, Durable on the Bucket

Status: **Proposed** · 2026-10-08. Source: the owner's goal "the Grafeo-based graph database (Loams Graph) production ready". Decisions **D740–D759** and questions **Q670–Q684** are recorded in the [decision log](13-decision-log.md). Plan: [GR1](../plans/2026-10-08-gr1-graph-production.md).

This is an addendum to [§07](07-graph.md) (native GraphRAG) and to D634 (GQL on Grafeo, recorded with [§33](33-connectors.md)). It also builds on [§01](01-architecture.md) (D5, the five object kinds), [§09](09-links-and-workers.md) (links), [§18](18-metastore-backends-and-router.md) (D65, D66, D76), [§19](19-console-identity-and-agents.md) (identity and agents), [§32](32-loams-flow-fabric-house.md) (D343, D350), [§44](44-unified-api-and-sdks.md) (D600, the API rules) and [§37](37-desktop-and-mobile-apps.md) §19.10 (D674, the desktop Graph page). The sibling addenda [§45](45-loams-live-production.md), [§46](46-loams-postgres-production.md) and [§47](47-loams-sql-production.md) use the same structure, and §46 sets the precedent this document follows for where a service runs (a role of the `loams` binary behind a cargo feature, D704).

**The owner's goal changes an earlier answer.** Q339 was answered on 2026-10-02 with "§07's native surface only; a `grafeo-server` companion only when a customer asks" (D350). The owner asking for Loams Graph to be production ready is that ask, and it asks for more than a companion: a graph database Loams owns, operates and stands behind. §3 says how that fits with §07 without throwing §07's API away.

Markers: **(verify)** means not checked against a primary source; the plan task that depends on the fact checks it first. **(estimate)** means computed, not measured. **(target)** is a number this document sets as a gate.

---

## 1. Summary

| # | Decision | Status |
|---|---|---|
| D740 | **"Production ready" means the exit checklist in §21** is met on the reference topology (§17.3). Loams Graph is a **stored property graph** that Loams owns, queried and written with **GQL** (ISO/IEC 39075) over `loams.graph.v1`, executed by an embedded **Grafeo** engine, and made durable by Loams' own log and bucket. It amends D350 and reopens Q339: the owner's goal is the demand Q339 waited for | Proposed |
| D741 | **Where it runs: the `graph` role of the `loams` binary, behind the cargo feature `graph`**, not a `loams-fabric` binary. `loams-graph` and its proto move from `fabric/` to the root workspace. In a cluster, graph owners can run as separate `loams` processes with only the `graph` role, and the API node forwards to them over `loams.internal.v1`. Amends D634 (b)'s "embedded in the Fabric", D343's graph clause and D51 (§4) | Proposed |
| D742 | **Durability: the truth is the graph's log and the bucket, not Grafeo's files.** Every committed write transaction becomes one engine-neutral `GraphChangeSet` record in the graph's implicit stream in the Loams WAL, and is acknowledged only after that append is durable. Snapshots go to the bucket, and a manifest in the metastore records which snapshot covers which offset. Grafeo's local `.grafeo` file and WAL are a **rebuildable cache**. No read ever sees a write that is not yet durable (§6) | Proposed |
| D743 | **A graph is D5's graph object kind, in one of two modes.** An **owned** graph is written with GQL. A **linked** graph is written only by links, from collections (`collection → graph`) and from streams (`stream → graph`), using a declared mapping; GQL may read it but not write it. §07's API surfaces (`graph_expand` and the other SQL table functions, and the `expand` stage of `QueryService/Search`) keep their contracts and are **executed by Loams Graph**. §07's CSR/CSC sidecars and `ExpandExec` are deferred, not cancelled. Amends D44's implementation, not its API (§3, §10) | Proposed |
| D744 | **One writer per graph, fenced.** A graph's owner holds a lease with an epoch in the metastore, and its appends carry that epoch, so a stale owner's write is refused. Strong reads go to the owner. Followers tail the log and serve `eventual` and `at_least` reads. Consistency tokens have D76's shape over the graph's stream (§7) | Proposed |
| D745 | **Transactions:** a statement is a transaction, and `ExecuteBatch { atomic: true }` is the only multi-statement transaction. Isolation is Grafeo's snapshot isolation by default, with `SERIALIZABLE` available per batch. There are **no interactive transactions** across RPCs in v1. Every writing call takes an `idempotency_key`, and `ExecuteBatch` takes an optional `if_version` precondition for read-modify-write (§7) | Proposed |
| D746 | **The API is `loams.graph.v1`, reworked before its first publish.** It has two services, `GraphService` (data) and `GraphAdminService` (resources). Values are a typed GQL value model, with no float narrowing. Errors carry the GQLSTATUS code and the position in the statement. There is a server-streaming `ExecuteStream` for large results. The client-chosen `database_path` is removed. The package stays `unstable` until GR1e (§8) | Proposed |
| D747 | **The GQL surface is versioned as a declared profile, `gql-1`**, listed by GQL feature ids, the way `chsurface-1` declares the House's ClickHouse subset. `GetEngineInfo` reports the profile and the engine version. Statements reach the engine unchanged (D634's no-rewriting rule stands). A feature outside the profile is `UNIMPLEMENTED` with the reason `gql_feature_unsupported` (§9) | Proposed |
| D748 | **Cypher is an opt-in compatibility dialect**, not a second contract. It is openCypher 9 through Grafeo's `cypher` feature, enabled per graph (`languages: [GQL, CYPHER]`) and off by default. There is still **no Bolt** (D634 (c)), no APOC and no procedure shims. An openCypher TCK subset gates it. Amends D634 (a) and D44's "no Cypher" (§9.3) | Proposed |
| D749 | **One vector store.** Grafeo is built **without** its `ai` features (vector, text and hybrid indexes). Embeddings and text stay in collections (§06, D350). Graph vertices refer to documents by `(collection, key)`. GraphRAG is seed (collection search) → expand (Loams Graph) → rerank, in one `Search` request or one SQL query (§10) | Proposed |
| D750 | **Authorization is per graph.** Every RPC goes through the unified auth interceptor, and the `Authorizer` (D66) checks the object `graph:<ns>/<name>` with the relations `reader`, `writer` and `admin`, inherited from namespace grants. The engine enforces the decision too: the session gets Grafeo's matching `Role`, so classification is done by the engine's parser, and the keyword guard stays as a second check. Agent scopes are `graph:read`, `graph:write` and `graph:admin`. There is no label- or property-level security in v1 (§11) | Proposed |
| D751 | **Tenancy:** the namespace is the isolation boundary. Each graph has its own Grafeo instance, its own storage prefix `ns/<ns_id>/graphs/<graph_id>/` and its own memory budget. There are no cross-graph or cross-namespace queries in v1 (§12) | Proposed |
| D752 | **Limits** (defaults; per-namespace overrides come from the §41 limits record): a 30 s statement timeout (max 300 s); 1 GiB memory per graph, with spill to local disk; 10 000 rows or 16 MiB per unary response, after which the response is truncated and `ExecuteStream` serves the rest; 1 MiB per statement; 1 000 statements per batch; 1 MiB per property value; 64 concurrent statements per namespace; 100 graphs per namespace. Exceeding a limit is `RESOURCE_EXHAUSTED` with a named reason (§13) | Proposed |
| D753 | **HA, backups and DR:** RPO 0 for any node loss (acknowledged means in the log). RTO ≤ 30 s without a standby and ≤ 10 s with one (target). Point-in-time restore to any token or timestamp within 7 days, always into a new graph. A portable Parquet export of nodes and edges. A monthly automated restore drill (§14) | Proposed |
| D754 | **Observability:** the `loams_graph_*` metric families, OTel spans per statement, a slow-statement log with literals redacted, audit events for every admin and authorization decision, and `EXPLAIN`/`PROFILE` exposed as RPCs. None of it is billing-grade (D548) (§15) | Proposed |
| D755 | **Security:** no client-chosen paths; procedures and functions that reach the filesystem or the network are denied; panics are contained at the RPC boundary and the graph is poisoned and reloaded; the service, the GQL parser and the change-set codec are fuzzed; and an **external security review** is a GA gate (§16) | Proposed |
| D756 | **Conformance gates:** the `gql-1` corpus passes 100 % of the declared features; the openCypher TCK subset reaches ≥ 90 % of the scenarios selected for the compatibility dialect, with every failure on an allowlist; the LangChain, LlamaIndex and LightRAG graph-store suites pass (§17.1) | Proposed |
| D757 | **Performance gates** on the reference topology, using an LDBC SNB-derived workload at SF1 (not an audited LDBC result): IS1–IS7 short reads p99 ≤ 20 ms hot; the selected IC reads p99 ≤ 500 ms hot; a 2-hop expand from 10 seeds p99 ≤ 50 ms hot; GraphRAG seed → 2 hops → rerank p95 ≤ 150 ms hot; a durable write acknowledged within the WAL ack time + 10 ms at p99; ingest through a link ≥ 50 000 edges/s per graph; failover within D753's RTO (§17.2) | Proposed |
| D758 | **SDKs and clients:** TypeScript and Python first, generated by SDK1 with a hand-written `loams.graph` facade (typed values, streaming, transactions); the Python adapters for LangChain, LlamaIndex and LightRAG; the other languages through SDK2's generation. **The desktop Graph page** uses `loams.graph.v1` through the server proxy whenever `GetInstance` advertises it as available, which amends D674's "until `loams-fabric`" (§18) | Proposed; amends D674 |
| D759 | **Dependency policy for Grafeo:** an exact pin, at least 14 days old; all six crates (including `grafeo-storage`, which `connectors/licences.toml` misses) checked as Apache-2.0; Grafeo's `NOTICE` carried in Loams' `NOTICE`; Loams depends on the GQL standard and on its own log format, so the engine can be replaced (D634 (f)); a **fork trigger** (§19.3) decides when Loams maintains a fork | Proposed |

## 2. What "production ready" means

Loams Graph is production ready when a team can do all of the following on a self-hosted cluster, on BYOC or on Loams Cloud, and the guarantees hold under faults:

1. Create a graph in a namespace, write it with GQL in atomic batches, and read it with GQL. Every acknowledged write survives the loss of any one node, with no read ever seeing a write that was later lost.
2. Keep a graph in step with collections and streams through links, exactly once, and run GraphRAG retrieval (vector or BM25 seeds → 1–2 hops → rerank) as one request.
3. Restrict who can read, write and administer each graph, with API keys, user tokens and agent tokens from the unified auth plan.
4. Operate it: quotas, timeouts and memory limits that protect neighbours; metrics, traces, logs and audit; failover inside the RTO; point-in-time restore; export and import; rolling upgrades of Loams and of Grafeo.
5. Use it from TypeScript, Python, LangChain, LlamaIndex, LightRAG and Loams Desktop, against a stable and documented `loams.graph.v1` and `gql-1`.
6. Trust that the gates in §17 ran: conformance, performance, fault injection, fuzzing and an external security review.

Out of scope for GA: graphs larger than one node's memory and local disk can serve (sharded graphs, Q677), interactive transactions (Q676), RDF and SPARQL, Gremlin, GraphQL-over-graph, SQL/PGQ `GRAPH_TABLE` (§9.4), Bolt (D634 (c)), and label- or property-level security (Q674).

## 3. Where Loams Graph sits: reconciling D44, D350 and D634

Three earlier decisions describe graphs, and they do not agree:

| Decision | What it says | What this document keeps |
|---|---|---|
| D44 (§07), approved | Graphs are mapped over collections, accelerated by CSR/CSC sidecars and DataFusion operators, and reached through SQL table functions and the `expand` stage. No Cypher, no Bolt, no native graph store | **The API:** the SQL table functions, the `expand` stage and the adapters' shapes (§07 §5, §6). **Not the engine:** sidecars and `ExpandExec` are deferred (D743) |
| D350 (§32 §7.7), approved | Grafeo is not the House graph engine; a `grafeo-server` companion only if Q339 asks | Superseded for the engine choice by the owner's goal (D740); its concerns become requirements here: a second storage engine (answered by D742: the bucket is the truth), a second HNSW (answered by D749: built without it), arrow 60 (answered by §4.3), one maintainer (answered by D759's fork trigger) |
| D634, recorded 2026-10-04 | GQL over Connect-RPC; Grafeo embedded in the Fabric; no Bolt; Grafeo is a test target, not a foundation | **Kept:** GQL, Connect, no Bolt, no rewriting, and (f)'s point that Loams depends on the standard. **Changed:** where it is embedded (D741), and Grafeo becomes the production engine *behind a Loams-owned log*, which is how (f) stays true |

**The model (D743).** D5 lists five object kinds: stream, table, collection, graph, link. A graph is a first-class object, so it has a catalog entry, a storage prefix, an implicit stream, quotas and an authorization object, like a collection. It comes in two modes:

| | Owned graph | Linked graph |
|---|---|---|
| Written by | GQL statements (`Execute`, `ExecuteBatch`) and bulk import | Links only: `collection → graph`, `stream → graph`, and Flow routes through `loams_sink` |
| Read by | GQL, the SQL table functions, `Search.expand` | The same |
| Source of truth | The graph's own log and snapshots | The source collections and streams; the graph's log records what the links applied, so the graph can be rebuilt from either |
| Typical use | A knowledge graph an application curates; a Neo4j-shaped workload | GraphRAG over chunks, entities and relations that already live in collections; §07's mapped graph |

A linked graph is §07's mapped graph with a stored projection instead of sidecars. The mapping is §07 §2.1's `CREATE GRAPH` declaration, unchanged, and it is stored on the graph. GQL writes to a linked graph are refused with `FAILED_PRECONDITION` and the reason `graph_is_linked`. That is what keeps a linked graph's state a pure function of its sources.

## 4. Where it runs (D741)

### 4.1 The options

| | (a) The `graph` role of `loams`, feature `graph` | (b) A role of `loams-fabric` (D343, D634 (b)) | (c) A separate `loams-graph` binary |
|---|---|---|---|
| One API, one port (D600) | Yes: a row in `CATALOGUE`, served on 8080 | No: a second public port, or a proxy in `loams` | No, as (b) |
| Unified auth, quotas, `Authorizer` | In process, the same interceptor as every `loams.*.v1` service | Re-implemented or called remotely | As (b) |
| Durability on Loams' log, metastore and bucket (D742) | `loams-log`, `loams-meta`, `loams-store` are in the same workspace | `fabric/` would path-depend on the root workspace, which D343 exists to prevent | As (b) |
| GraphRAG: collections and graph in one plan | In process | A network hop per expand | As (b) |
| Desktop (D674) | `loams dev` is the only process the desktop supervises; the page works locally | A second supervised binary | As (b) |
| Exists today | The binary and the Connect router exist | No `loams-fabric` crate exists; `.github/workflows/fabric.yml` only names its path | Nothing |
| Isolation from a young engine | Weaker in one process; mitigated by §4.2 | Strong | Strong |
| Dependency weight in the engine build | Grafeo's tree enters the root `Cargo.lock`, behind a non-default feature | None | None |

**Decision: (a).** Four of the six concerns that drove D343 do not apply to Grafeo: it is pure Rust with no C library, it is not process-global like libchdb, it is not 180 MB, and no Arrow type crosses its boundary (§4.3). The two that remain, isolation and build weight, have mitigations that cost less than a second binary, a second auth stack and a cross-workspace dependency.

### 4.2 Isolation inside one binary

- **Panics.** Every call into the engine runs inside `catch_unwind` at the service boundary. A panic answers `INTERNAL` with the reason `graph_engine_panic`, marks that graph `poisoned`, drops its engine instance and reloads it from the snapshot and the log (§6.5). Other graphs and the rest of `loams` are not affected. Grafeo must not be built with `panic = "abort"`; the plan checks this.
- **Memory.** Each graph has its own `GrafeoDB` with Grafeo's own `memory_limit` and spill path (Grafeo 0.5.43 `Config::with_memory_limit`, `with_spill_path`), and a per-node budget evicts idle graphs (§13.3).
- **Process separation where it matters.** `loams serve --roles graph` runs only graph owners. In a cluster the API nodes do not open graphs; they authenticate, authorize and forward to the owner over `loams.internal.v1` (D607), so an engine fault or a memory spike stays on graph nodes. A single-node deployment and `loams dev` run every role in one process.
- **Off by default.** The feature `graph` is not in `default`. It ships in the `full` variant first, and moves to `standard` only when the size gate (§17.2) and the exit checklist pass (Q675).

### 4.3 The dependency tree

Grafeo 0.5.43's default feature `embedded` turns on `ai` (vector, text and hybrid indexes and CDC), `algos`, `parallel`, `regex`, `grafeo-file` and `arrow-export`; `arrow-export` brings `arrow-array`, `arrow-ipc` and `arrow-schema` 60 (read from `fabric/Cargo.lock`, where `grafeo-engine` depends on them). The engine workspace is on arrow 58 (Lance 12 lockstep). The plan's Task 0 measures the narrowest feature set that keeps the LPG model and GQL: `default-features = false` on `grafeo`, with `gql`, `wal`, `grafeo-file`, `spill`, `mmap`, `parallel`, `regex`, `cdc`, `algos`, `metrics` and `tracing`, plus `grafeo-engine/lpg` named directly, since only `embedded`, `edge` and `lpg` turn it on through the facade, and `lpg` also turns on Cypher, Gremlin and SQL/PGQ. If arrow 60 still enters the tree, it is accepted as a private dependency: D51's problem was Arrow types exchanged across engines, and none are here. The cost is binary size, which §17.2 gates.

### 4.4 What moves

- `fabric/crates/loams-graph` → `crates/loams-graph` (engine wrapper, service, durability, links).
- `fabric/proto/loams/graph/v1/graph.proto` → `proto/loams/graph/v1/graph.proto`, in the root buf module, so `buf lint`, `buf breaking`, SDK1 generation and `@loams/proto` all see it. The types are generated into `loams-proto`, as the other packages are, and `fabric/crates/loams-graph-proto` is deleted. `go_package` becomes the §44 Go module path.
- The CN1 connector `connectors/registry/grafeo.yaml` stops claiming an in-process engine. It becomes a client of `loams.graph.v1` over Connect, with `auth: key`, since a Loams API key now protects the graph. Its `runtime.ref` points at the Flow-side client (CN2 or later).
- D343's statement that the graph is embedded in `loams-fabric` is withdrawn. `loams-fabric` keeps ingest, House and Flow.

## 5. As built on 2026-10-08 (reconciled with the code)

Read from `fabric/crates/loams-graph` (`engine.rs`, `service.rs`, `lib.rs`, `tests/graph.rs`), `fabric/proto/loams/graph/v1/graph.proto`, `fabric/Cargo.lock`, `crates/loams/src/api/connect.rs` and Grafeo 0.5.43's sources:

1. **No binary serves `loams.graph.v1`.** `service.rs` has free functions that a generated `GraphService` trait would call; nothing implements the trait or mounts it. `fabric/crates` has no `loams-fabric` crate. The `loams` binary's `CATALOGUE` has no graph row, although §44 §7 lists `loams.graph` as a placeholder module.
2. **`OpenRequest.database_path` is a client-chosen filesystem path.** Over a network this would let any caller create or open a Grafeo file anywhere the server can write. It must not be served as is (D746, D755).
3. **`ExecuteRequest` has no parameters**, so a single statement can only carry values by string interpolation, which is what the proto's own comment on `Statement.parameters` warns against.
4. **Non-atomic `ExecuteBatch` drops the parameters.** `service::execute_batch` with `atomic = false` calls `graph.execute(&statement.text, false)`, which binds nothing.
5. **`Statement.language` is never checked.** Only the batch's language is validated, so a per-statement override is silently ignored.
6. **Numbers are narrowed to `f64` in both directions** (`google.protobuf.Value`), so an `INT64` above 2^53 is corrupted and every bound integer is stored as a float.
7. **`rows_affected` and `bytes_read` are always 0**, and `rows_read` is the number of rows returned. The code says so honestly; the contract should drop or redefine them (§8).
8. **The read-only guard is a keyword scan.** It is conservative and well tested. Grafeo 0.5.43 also classifies statements in its parser (`auth::StatementKind`: Read, Write, Admin, Transaction) and enforces it per session through `session_with_role`. The engine's check becomes the primary one (D750).
9. **Durability is Grafeo's alone.** A persistent graph uses Grafeo's WAL with its default `DurabilityMode::Batch` (periodic fsync), so an acknowledged write can be lost on a crash, and nothing reaches the bucket.
10. **`ListGraphs` lists what is open in this process**; there is no catalog, so a restart forgets every graph.
11. **The build includes Grafeo's `ai` features and arrow 60** (§4.3), contrary to D350's one-vector-store rule, because the crate uses Grafeo's defaults.
12. **`connectors/licences.toml` lists five Grafeo crates; the build links six.** `grafeo-storage` 0.5.43 is missing. It is Apache-2.0 (crates.io, read 2026-10-08).

Grafeo 0.5.43 facts this design relies on, each re-checked by the plan's Task 0: `Session::set_viewing_epoch` and `execute_at_epoch` (reads pinned to an MVCC epoch); `Session::prepare_commit` returning a `PreparedCommit` whose `commit()` returns the commit `EpochId`; CDC events keyed by `NodeId`/`EdgeId` with the commit epoch, kept in memory only; `IsolationLevel::{ReadCommitted, SnapshotIsolation, Serializable}`; `Config::{memory_limit, spill_path, query_timeout, max_property_size, wal_durability, checkpoint_interval, encryption}`; `backup_full`, `backup_incremental` and `restore_to_epoch`; `auth::Role::{ReadOnly, ReadWrite, Admin}`. The upstream CDC module notes a planned 0.6 refactor into a `MutationListener` trait, which is the API churn §22 lists as a risk.

## 6. Storage and durability (D742)

### 6.1 Invariants

- **I1. Acknowledged means durable.** A write is acknowledged only after its change set is durable in the graph's stream (the Loams WAL's guarantee, §02).
- **I2. No read sees a non-durable write.** Every client session reads at an epoch no newer than the graph's durable epoch.
- **I3. The log is a total order.** Change sets are appended in commit-epoch order, and the order on replay is the order of commit.
- **I4. Rebuildable.** A graph's state is a function of (latest snapshot, log tail). Deleting every local file loses nothing.
- **I5. Fenced.** Only the current owner's epoch can append (D744).
- **I6. Engine-neutral at rest.** The log record and the portable snapshot do not depend on Grafeo's file format, so Grafeo can be upgraded or replaced by rebuilding.

### 6.2 The write path

```
client ─▶ API node: authn, authz (writer), quota, idempotency lookup
       ─▶ owner (forward over loams.internal.v1 if remote)
owner:  per-graph write lane (one committing transaction at a time)
        1. session(role=ReadWrite, viewing epoch = latest) ; begin_transaction(isolation)
        2. execute statement(s) with bound parameters
        3. prepare_commit()            ─ conflicts surface here or at commit
        4. commit()  → epoch E         ─ visible in Grafeo, NOT yet to clients (I2)
        5. capture change set for E    ─ mechanism in §6.3
        6. enqueue GraphChangeSet{E}   ─ release the write lane
        7. group commit to the graph stream (lease epoch fenced)
        8. durable_epoch := E ; answer { token, epoch }   ─ client sessions may now see E
```

- Releasing the lane at step 6 pipelines writes: the next transaction runs while earlier change sets are still being flushed, and group commit amortizes the WAL append. A later transaction may read an earlier, not-yet-durable commit; that is safe because I3 makes the later one durable only after the earlier one.
- **An append failure fences the graph.** If step 7 fails and cannot be retried (lease lost, a write refused as stale), the owner stops serving the graph, drops its engine instance and reloads from the snapshot and the log (§6.5). Every transaction after the last durable epoch fails with `UNAVAILABLE` and the reason `graph_reloading`, and none of them was ever visible to a client (I2).
- **Clients see durable state only.** Every client-facing session gets `set_viewing_epoch(durable_epoch)` before it runs (§5). The write lane's own session reads the latest state, so read-your-writes inside a batch work.

### 6.3 Capturing the change set: three mechanisms, one contract

The change set must contain every mutation of the committed transaction, in order, with stable element identities. GR1b Task 9 measures three mechanisms against the same tests and records the choice as a ruling (Q672):

| | Mechanism | For | Against |
|---|---|---|---|
| **B (preferred)** | Grafeo CDC (`cdc` feature, session-level `session_with_cdc(true)`): after `commit()` returns E, collect the events tagged with epoch E and convert them into a neutral `GraphChangeSet` | Engine-neutral by construction; the same events feed links out and a future `WatchGraph` | CDC is in memory and retention-bounded; schema and index DDL may not be in CDC (verify); ids are engine `NodeId`/`EdgeId` |
| A | Ship Grafeo's own WAL records for the transaction (`GrafeoDB::wal()`, replay through `apply_wal_records`), wrapped in a Loams envelope | Exact and complete; replay is Grafeo's own | Engine-specific at rest, which breaks I6 unless a neutral export is also kept; needs a WAL tap upstream does not expose yet (verify) |
| C (single node only) | `DurabilityMode::Sync` on local disk plus `backup_incremental` shipped to the bucket every second | No new hooks | RPO is the shipping interval, so it breaks I1; allowed only for `loams dev` and the desktop |

**Element identity.** A change set names elements by Loams element ids (`u64`, assigned by the owner in commit order and stable across rebuilds). If Grafeo can create a node or an edge with a caller-chosen id (verify), the Loams id and the engine id are the same. Otherwise the owner keeps a bidirectional id map inside the engine as a hidden property `_lid` with a unique index, and replay resolves through it. Task 9 decides which.

**DDL** (`CREATE INDEX`, `CREATE TYPE`, constraints) is recorded as a `SchemaChange` record carrying the statement text, which is deterministic, and runs through the same lane.

### 6.4 Records, snapshots and the manifest

```
GraphChangeSet  v1  { graph_id, lease_epoch, commit_epoch, txn_seq, idempotency_key?,
                      ops: [ UpsertNode{id, labels[], props}, DeleteNode{id},
                             UpsertEdge{id, type, src, dst, props}, DeleteEdge{id},
                             SetProps{id, kind, set{}, removed[]} ] }
SchemaChange    v1  { graph_id, lease_epoch, commit_epoch, statement, language }
```

- The encoding is the Loams record codec with an explicit version byte. Property values use the same typed value model as the API (§8.3), so a value written over the wire is the value replayed.
- **Snapshots.** A checkpoint job on the owner runs every 15 minutes or every 64 MiB of log, whichever comes first (defaults). It writes a native snapshot (Grafeo `backup_full` of a checkpointed database) to `ns/<ns_id>/graphs/<graph_id>/snap/<epoch>/`, then CAS-commits the manifest `{snapshot, covered_offset, covered_epoch, format, engine_version}` in the metastore. Weekly, and before any engine upgrade, it also writes a **portable snapshot**: Parquet files of nodes and edges with the typed values, which is the I6 escape hatch and the export format (§14.3).
- **Retention.** The log is trimmed below the oldest retained snapshot's offset, never below the PITR window (7 days by default). GC follows §09's rules for unreferenced objects.

### 6.5 Opening, recovery and eviction

- **Open** = read the manifest → fetch the snapshot into the local cache directory (`<data>/graphs/<graph_id>/`) → open it with Grafeo → replay the change sets after `covered_offset` → durable_epoch := the last replayed epoch → serve.
- Replay is idempotent: a change set whose `commit_epoch` is not above the last applied one is skipped.
- **Idle eviction.** A graph with no statement for `graph.idle_evict_after` (1 h by default) is checkpointed and dropped from memory. Its local files stay as a cache until disk pressure removes them. The next statement reopens it (cold-start target: §17.2).
- **Engine upgrades.** If the new Grafeo cannot read the native snapshot, the owner rebuilds from the latest portable snapshot plus the log. The manifest's `engine_version` tells it which path to take.

## 7. Transactions and consistency (D744, D745)

### 7.1 Ownership

- A graph's owner is chosen by rendezvous hashing over nodes with the `graph` role (§04 §5's affinity), and holds a lease `graph/<graph_id>/owner` with an epoch in the metastore (M0.4's lease framework). Lease TTL is 10 s, renewed every 3 s (defaults).
- Every append carries the lease epoch, and the log refuses a stale epoch, so two owners can never both commit (I5).
- Writes that reach a non-owner are forwarded to the owner; if no owner holds the lease, the receiving node attempts to acquire it.

### 7.2 Reads and tokens

| `consistency` | Served by | Meaning |
|---|---|---|
| `strong` (default) | The owner | Reads at the owner's durable epoch, which includes every acknowledged write |
| `at_least { token }` | Any replica | Waits until the replica has applied the token's offset (up to `consistency_wait_ms`, 5 000 ms by default), then reads; otherwise `UNAVAILABLE` with the reason `consistency_wait_timeout` |
| `eventual` | Any replica | Whatever the replica has applied |

The token is D76's `{(stream, partition, offset)}` over the graph's implicit stream (one partition), returned in the response and in the `loams-consistency-token` header, as `loams.collection.v1` does. A replica is a follower that tails the stream and applies change sets to its own Grafeo instance (the same replay as §6.5).

### 7.3 Transactions

- One statement is one transaction. `ExecuteBatch { atomic: true }` runs every statement in one Grafeo transaction, produces one change set and is acknowledged once; `atomic: false` runs each statement as its own transaction and reports per-statement results, with `committed_through` saying how far it got.
- **Isolation:** `SNAPSHOT` (Grafeo's default) or `SERIALIZABLE` per batch. A conflict is `ABORTED` with the reason `graph_write_conflict` and is safe to retry with the same `idempotency_key`.
- **Idempotency.** The owner keeps `idempotency_key → token` per graph for 24 h, in the change set itself and in a bounded in-memory index rebuilt on replay. A retried key answers the first result's token without running the statement again (AP0's rule).
- **`if_version`.** `ExecuteBatch` may carry `if_version: <offset>`; the batch runs only if the graph's last durable offset equals it, otherwise `FAILED_PRECONDITION` with the reason `graph_version_mismatch`. That is the v1 answer to read-modify-write without interactive transactions.
- GQL's own transaction statements (`START TRANSACTION`, `COMMIT`, `ROLLBACK`) are refused inside `Execute` and `ExecuteBatch` with `INVALID_ARGUMENT` and the reason `graph_transaction_statement`, since the RPC owns the transaction.

## 8. The API (D746)

### 8.1 Rules

The AP0 rules apply: an `idempotency_key` on every mutation, `NO_SIDE_EFFECTS` on reads, `loams.errors.v1.ErrorInfo` reasons, AIP-158 pagination, and slow operations as `loams.operations.v1.Operation`. The package is `unstable: true` in `CATALOGUE` and in `ModuleOptions` until GR1e, so `buf breaking` and the SDKs treat it as experimental; it is frozen at GA. Nothing has published clients of the fabric-era proto, so it is reworked rather than versioned.

### 8.2 Outline

```proto
package loams.graph.v1;

service GraphAdminService {
  rpc GetEngineInfo(GetEngineInfoRequest) returns (EngineInfo);           // NO_SIDE_EFFECTS
  rpc CreateGraph(CreateGraphRequest) returns (Graph);                    // idempotency_key
  rpc GetGraph(GetGraphRequest) returns (Graph);                          // NO_SIDE_EFFECTS
  rpc ListGraphs(ListGraphsRequest) returns (ListGraphsResponse);         // AIP-158
  rpc UpdateGraph(UpdateGraphRequest) returns (Graph);                    // field mask: languages, limits, replicas
  rpc DeleteGraph(DeleteGraphRequest) returns (loams.operations.v1.Operation);
  rpc GetSchema(GetSchemaRequest) returns (GraphSchema);                  // labels, edge types, property keys, counts, indexes
  rpc RestoreGraph(RestoreGraphRequest) returns (loams.operations.v1.Operation);  // into a new graph, at a token or timestamp
  rpc ExportGraph(ExportGraphRequest) returns (loams.operations.v1.Operation);    // portable Parquet to the namespace's export prefix
  rpc ImportGraph(ImportGraphRequest) returns (loams.operations.v1.Operation);    // Parquet / CSV / JSONL from the namespace's import prefix
}

service GraphService {
  rpc Execute(ExecuteRequest) returns (ExecuteResponse);
  rpc ExecuteBatch(ExecuteBatchRequest) returns (ExecuteBatchResponse);
  rpc ExecuteStream(ExecuteStreamRequest) returns (stream ResultChunk);   // server streaming only (D420); { ExecuteRequest request; uint32 chunk_rows } (GR1 R2.1)
  rpc Explain(ExplainRequest) returns (Plan);                             // EXPLAIN, or PROFILE with profile = true
}

message Graph {
  string namespace = 1; string name = 2; string id = 3;                   // id: "gr_<ULID>"
  GraphMode mode = 4;                                                     // OWNED | LINKED
  repeated QueryLanguage languages = 5;                                   // default [GQL]
  GraphMapping mapping = 6;                                               // LINKED only (§07 §2.1)
  GraphLimits limits = 7; uint32 replicas = 8;
  GraphState state = 9;                                                   // READY | OPENING | EVICTED | RELOADING | DELETING
  GraphStats stats = 10;                                                  // nodes, edges, stored_bytes, durable_offset
  google.protobuf.Timestamp create_time = 11;
}

message ExecuteRequest {
  string namespace = 1; string graph = 2;
  string statement = 3; QueryLanguage language = 4;
  map<string, Value> parameters = 5;
  bool read_only = 6;                                                     // an assertion: a write is refused
  Consistency consistency = 7;                                            // strong | eventual | at_least(token)
  uint32 timeout_ms = 8; uint32 max_rows = 9;                             // capped by D752
  string idempotency_key = 10;                                            // required when the statement writes
}

message ExecuteResponse {
  RowSet rows = 1; bool truncated = 2;
  Counters counters = 3;                                                  // nodes_created, nodes_deleted, edges_created, …, properties_set
  string consistency_token = 4; uint64 commit_epoch = 5;
  uint64 elapsed_nanos = 6;
  repeated Notification notifications = 7;                                // GQLSTATUS warnings (e.g. 01xxx)
}

message Value {                                                           // typed GQL values (§8.3)
  oneof kind {
    NullValue null = 1; bool boolean = 2; int64 int64 = 3; uint64 uint64 = 4; double float64 = 5;
    string string = 6; bytes bytes = 7; Decimal decimal = 8;
    Date date = 9; LocalTime local_time = 10; ZonedTime zoned_time = 11;
    LocalDateTime local_datetime = 12; ZonedDateTime zoned_datetime = 13; Duration duration = 14;
    ListValue list = 15; MapValue map = 16;
    Node node = 17; Relationship relationship = 18; Path path = 19;
  }
}
message Node { uint64 id = 1; repeated string labels = 2; map<string, Value> properties = 3; }
message Relationship { uint64 id = 1; string type = 2; uint64 src = 3; uint64 dst = 4; map<string, Value> properties = 5; }
message Path { repeated Node nodes = 1; repeated Relationship relationships = 2; }
```

`RowSet` keeps the positional shape the fabric proto chose (columns, column types, rows of `Value`), because a graph result repeats keys. `rows_read` and `bytes_read` are dropped; `Counters` holds what the change set can count exactly.

### 8.3 Errors

- `ErrorInfo.reason` values: `gql_syntax_error`, `gql_feature_unsupported`, `graph_not_found`, `graph_is_linked`, `graph_read_only`, `graph_write_conflict`, `graph_version_mismatch`, `graph_statement_timeout`, `graph_memory_limit`, `graph_result_too_large`, `graph_transaction_statement`, `graph_language_disabled`, `graph_reloading`, `graph_engine_panic`, `consistency_wait_timeout`, `feature_not_in_variant`; and, from GR1 execution, `graph_statement_not_allowed` (R0.10, R0.11), `graph_unbounded_path` (R0.8) and `graph_catalog_version_mismatch` (`UpdateGraph`'s `expected_version`, R2.9).
- `ErrorInfo.metadata` carries `gqlstatus` (the five-character GQLSTATUS code of ISO/IEC 39075, when the engine reports one; verify Grafeo's mapping in `docs/user-guide/error-codes.md`), and `line`, `column` and `length` for a syntax error, so an editor can underline it.

### 8.4 REST

The console REST routes, where a page needs them, map 1:1 under `/v1/namespaces/{ns}/graphs/…` as §46 does for Postgres. The Connect JSON form is the primary HTTP surface; no separate REST contract is designed.

## 9. GQL surface, versioning and dialects (D747, D748)

### 9.1 `gql-1`

- `gql-1` is a document in `spec/graph/gql-1.md` listing, by ISO/IEC 39075 feature id (verify the id scheme against the published standard), what Loams Graph supports: the mandatory core plus the optional features Grafeo implements and the corpus tests. The standard's text is copyrighted, so the document cites feature ids and section numbers and does not copy prose.
- `EngineInfo { surface: "gql-1.0", engine: "grafeo", engine_version: "0.5.43", languages, … }`. `gql-1.x` minors only add features. A feature removed or changed is `gql-2`.
- A statement is passed to the engine **unchanged** (D634). Loams does not parse it except to classify (§11.2) and to redact literals for logs (§15).

### 9.2 Grafeo's GQL as the reference

Grafeo is the engine of record for `gql-1`, and its behaviour on the corpus defines the profile. Where Grafeo deviates from the standard and Loams knows it, the profile lists the deviation, and the corpus test asserts Loams' declared behaviour so a Grafeo upgrade that changes it fails CI.

### 9.3 Cypher as a compatibility dialect (D748)

- Built with Grafeo's `cypher` feature in the `graph` build, and enabled per graph by `languages`. A graph without it answers `FAILED_PRECONDITION` with `graph_language_disabled`.
- Scope: openCypher 9 read and write clauses (`MATCH`, `OPTIONAL MATCH`, `WHERE`, `WITH`, `UNWIND`, `RETURN`, `ORDER BY`, `SKIP`, `LIMIT`, aggregation, `CREATE`, `MERGE`, `SET`, `REMOVE`, `DELETE`, `DETACH DELETE`), which is what LangChain's Cypher QA chains and LlamaIndex's `structured_query` generate (verify against the pinned framework versions).
- Not in scope: Bolt, `neo4j` drivers, APOC, `CALL db.*` procedures beyond the schema introspection the adapters need (served by `GetSchema` instead), `LOAD CSV`, `USING PERIODIC COMMIT`, multi-database commands.
- Its own gate (D756): the openCypher TCK subset.

### 9.4 Other Grafeo languages

Gremlin, GraphQL, SPARQL, SQL/PGQ and RDF stay compiled out. `QueryLanguage` keeps their enum values so a client gets `UNIMPLEMENTED` rather than a parse error.

## 10. Links and GraphRAG (D743, D749)

### 10.1 `collection → graph`

- The mapping is §07 §2.1's declaration: vertex labels over keyed collections, edge types over collections with `(src_key, dst_key)` fields, `TYPE FROM (column)`, and the property projection. Stored on the graph as `GraphMapping`.
- The link reads the source collection's implicit stream through M0.4's link framework (exactly-once apply by offsets) and turns each `DocOp` into change-set operations: an upsert of a vertex document is `UpsertNode` keyed by `(label, key)`; a delete is `DeleteNode` with its incident edges; an edge document is `UpsertEdge`.
- **Dangling edges** (an edge whose endpoint does not exist yet, which LightRAG does): the link creates a **stub vertex** of the declared endpoint label with only its key and `_stub = true`, and the vertex's later upsert fills it. Q681 asks whether to skip instead, as §07 §7 did.
- Vertices carry `_collection` and `_key`, which is how GraphRAG joins back to documents.

### 10.2 `stream → graph` and Flow

- A `stream → graph` link applies records from a Loams stream (JSON or CloudEvents) through a mapping of field paths to node and edge operations, with the same exactly-once apply.
- Flow routes write graphs through `loams_sink` into a linked graph's source collections (§32 §6 row 4), or directly as a `stream → graph` link on a Loams stream the route fills. The CN1 `grafeo` connector reads and writes graphs through `loams.graph.v1` (§4.4).

### 10.3 Retrieval

- **`Search.expand`** (§07 §5.2's contract, unchanged): retrievers → fusion → `Limit(seeds)` → **expand in Loams Graph** → document fetch → rerank → limit. The expand step is a Loams-generated, parameterized GQL template (seed keys and filters as parameters, never interpolated), run at the request's consistency, with §07 §4's deterministic truncation order (hop, weight, canonical key) enforced by Loams after the engine returns.
- **SQL table functions** `graph_expand`, `graph_neighbors`, `graph_degree` and `graph_shortest_path` (§07 §5.1, same signatures) are DataFusion UDTFs that call the in-process graph engine and return Arrow batches. Their output joins collection tables as before.
- **Algorithms** `pagerank`, `wcc` and `leiden` (§07 §5.3) run on a snapshot through Grafeo's `algos` feature where it provides them (verify Leiden), otherwise `graspologic-native` (licence to verify), as a job that writes results back to the vertex source collection (linked graphs) or as properties (owned graphs).
- **GQL seeded by vector search** inside one GQL statement (a procedure such as `CALL loams.search(...)`) is not in v1 (Q683 covers the related change feed); the composition lives in `Search` and SQL, where the planner can see both halves.

### 10.4 Why not Grafeo's own vector index

Two sources of truth for one set of embeddings are what D350 refused: two HNSW indexes over the same embeddings drift, and the collection one is the one with Loams' recall gates, hot tier and quotas. Building without `ai` also removes code from the binary and the attack surface.

## 11. Authentication and authorization (D750)

### 11.1 Model

- Authentication is the unified auth interceptor every `loams.*.v1` service uses (D111, Q30, §19 §5): API keys `loams_<key_id>_<secret>`, user tokens, and short-lived agent tokens. Until MT1 lands the dev `Authorizer` is `AllowAll` on loopback only, and a non-loopback listener with `graph` enabled refuses to start without an authorizer, as §20 does for Live.
- The authorization object is `graph:<ns>/<name>`. OpenFGA model additions (D66, Lakekeeper-derived model):

```
type graph
  relations
    define namespace: [namespace]
    define reader: [user, service_account, agent] or reader from namespace
    define writer: [user, service_account, agent] or writer from namespace
    define admin:  [user, service_account] or admin from namespace
    define can_read:  reader or writer or admin
    define can_write: writer or admin
    define can_admin: admin
```

| RPC | Check |
|---|---|
| `Execute`, `ExecuteStream`, `Explain` with a read statement | `can_read` |
| The same with a write statement, `ExecuteBatch` with any write | `can_write` |
| DDL statements (index, type, constraint) | `can_admin` |
| `GetGraph`, `GetSchema`, `ListGraphs` | `can_read` (`ListGraphs` requires a namespace and filters that namespace's graphs with `filter_visible`; listing across namespaces would be a separate RPC, if one is ever needed. Controller ruling, GR1 Task 4 review M5) |
| `CreateGraph` | `admin` on the namespace |
| `UpdateGraph`, `DeleteGraph`, `RestoreGraph`, `ExportGraph`, `ImportGraph` | `can_admin`; protected environments (§19 §2) require the project `admin` role |

### 11.2 Classifying a statement

The statement's kind decides which check applies, and the classifier must never call a write a read:

1. Grafeo's parser classifies (`StatementKind::{Read, Write, Admin, Transaction}`) and the session runs with the Grafeo `Role` matching the caller's strongest granted relation (`ReadOnly`, `ReadWrite`, `Admin`), so the engine refuses a write in a read session regardless of what Loams thought.
2. The existing keyword guard runs first, as a cheap pre-check whose errors are conservative.
3. Agent token scopes narrow further: `graph:read`, `graph:write`, `graph:admin`.

Disagreements between (1) and (2) are logged as `graph_classifier_disagreement` and counted, because each one is either a guard bug or an engine bug.

## 12. Multi-tenancy (D751)

- The namespace is the boundary (§01 principle 5; an environment is one namespace, §19 P2). A graph's id, storage prefix, quotas and engine instance are its own.
- One `GrafeoDB` per graph means no engine-level structure (caches, the CDC log, the plan cache) is shared between tenants (verify that Grafeo has no process-global caches holding data; Task 0).
- No statement can name another graph (GQL's `USE` and catalog references are refused unless they name the request's own graph; verify how Grafeo resolves graph references).
- Encryption: bucket objects follow the namespace's key policy (§18); the local cache can use Grafeo's `EncryptionConfig` with a per-graph data key derived from the namespace key (verify the key-chain API), so evicted files at rest on a shared node are not readable across tenants.

## 13. Limits, quotas and memory (D752)

### 13.1 Per statement

| Limit | Default | Max | Enforced by | Reason |
|---|---|---|---|---|
| Timeout | 30 s | 300 s | Request deadline + Grafeo `query_timeout` (verify per-session control and cancellation) | `graph_statement_timeout` |
| Result rows (unary) | 10 000 | 100 000 | Loams, after the engine | `truncated = true`; `ExecuteStream` for more |
| Result bytes (unary) | 16 MiB | 64 MiB | Loams | `graph_result_too_large` |
| Statement size | 1 MiB | 1 MiB | Loams | `INVALID_ARGUMENT` |
| Parameters | 1 000 | 10 000 | Loams | `INVALID_ARGUMENT` |
| Statements per batch | 1 000 | 10 000 | Loams | `INVALID_ARGUMENT` |
| Property value | 1 MiB | 16 MiB | Grafeo `max_property_size` | `INVALID_ARGUMENT` |

### 13.2 Per namespace (the §41 limits record)

Graphs (100), total elements (100 M) (target), stored bytes, concurrent statements (64), statements per second, memory across its graphs, import bytes per day. Over quota is `RESOURCE_EXHAUSTED` with `quota_exceeded` and the quota's name in metadata (D65).

### 13.3 Memory

- Each graph's `GrafeoDB` gets `memory_limit` (1 GiB default, set per graph within the namespace's budget) and a spill path under its cache directory. A statement that exceeds it after spilling fails with `graph_memory_limit` (verify that Grafeo fails the statement rather than the process; Task 0 and the GR1d memory tests).
- Each graph node has a total budget (`graph.node_memory`, 70 % of the container limit by default). When it is exceeded, idle graphs are evicted least-recently-used first; if none are idle, new opens are refused with `UNAVAILABLE` and the router places the graph elsewhere.

## 14. HA, backups, DR and upgrades (D753)

### 14.1 Targets

| | Target |
|---|---|
| RPO, any single node or AZ loss | 0 (I1) |
| RTO, owner loss, no standby | ≤ 30 s for a graph whose snapshot is ≤ 1 GiB (lease expiry 10 s + snapshot load + tail replay) |
| RTO, owner loss, `replicas ≥ 1` | ≤ 10 s (a warm follower takes the lease) |
| PITR window | 7 days (configurable per namespace) |
| Restore drill | Monthly, automated, on the reference topology |

### 14.2 Backups are the storage

The bucket already holds the snapshots and the log, so a backup is the retention policy plus bucket versioning and, where configured, cross-region replication. `RestoreGraph` restores **into a new graph** at a consistency token, an epoch or a timestamp, by loading the newest snapshot before the target and replaying the log up to it. It never overwrites a live graph.

### 14.3 Export and import

`ExportGraph` writes the portable snapshot (§6.4) as Parquet with a JSON manifest; `ImportGraph` reads that format, and CSV or JSONL with a column mapping (Grafeo's importers, verify), from the namespace's own prefix only (§16). Import of more than 1 M elements runs as an operation that writes change sets in batches, so the log stays the source of truth.

### 14.4 Upgrades

- Loams: rolling, with the lease moving to an upgraded node; change-set and manifest formats are versioned and a new reader reads every older version.
- Grafeo: a minor or patch upgrade is a dependency bump that must pass the corpus, the TCK subset, the differential and the perf gates. If its file format changes, graphs rebuild from the portable snapshot on open (§6.5), and the release notes say so.
- API: `buf breaking` after GA.

## 15. Observability (D754)

- **Metrics:** `loams_graph_statements_total{kind, outcome, language}`, `loams_graph_statement_seconds` (histogram), `loams_graph_rows_returned`, `loams_graph_active_statements`, `loams_graph_memory_bytes{graph}`, `loams_graph_spill_bytes_total`, `loams_graph_changeset_append_seconds`, `loams_graph_durable_lag_epochs`, `loams_graph_replica_lag_offsets`, `loams_graph_snapshot_age_seconds`, `loams_graph_open_seconds`, `loams_graph_evictions_total`, `loams_graph_classifier_disagreements_total`, `loams_graph_panics_total`. Per-graph labels are bounded by the namespace's graph quota; per-namespace aggregates are always present. Grafeo's own `metrics` snapshot is exported under `loams_graph_engine_*`.
- **Traces:** one span per RPC, a child per engine call (`graph.execute`, `graph.commit`, `graph.append`), carrying the graph id, the statement fingerprint (a hash of the literal-stripped statement), the kind and the row count. Never the statement text with literals, and never parameter values.
- **Slow log:** statements above 1 s (configurable) with the literal-stripped text, the plan summary from `PROFILE` if requested, and the counters.
- **Audit:** every admin RPC, every denied authorization, every restore and export, as events in the §19 audit stream.
- **Dashboards and alerts** in `deploy/`: append latency, durable lag, replica lag, snapshot age above 2× the interval, memory near the limit, panics > 0.

## 16. Security (D755)

| Threat | Control |
|---|---|
| A client chooses a server path (as built: `database_path`) | Removed from the API; storage paths derive from the graph id only; import and export only touch the namespace's prefix in the bucket |
| GQL injection through string-built statements | Parameters on every RPC; the SDK facades take parameters, and the docs show only parameterized examples; linked-graph expand templates bind every value |
| Procedures, functions or `LOAD` reaching files or the network | Grafeo features that do so are compiled out; an allowlist of procedures and functions is checked at classification; anything else is `PERMISSION_DENIED` with `graph_procedure_denied` (verify what Grafeo 0.5.43 exposes) |
| Resource exhaustion | §13 limits; namespace concurrency semaphores; deadlines propagated into the engine |
| A crash in the engine takes the server down | `catch_unwind` at the boundary; graph poisoning and reload; `graph`-only processes in clusters (§4.2) |
| Cross-tenant data | One engine per graph; no shared engine caches (verified in Task 0); per-graph encryption of the local cache (§12) |
| A stale owner writes | Lease epochs on every append (I5) |
| Secrets in logs | Literal redaction; parameters never logged |
| Supply chain | Exact pin; `cargo deny` and `cargo vet` over Grafeo's tree; an `unsafe` inventory of the six crates; Dependabot alerts watched; the fork trigger (§19.3) |
| Parser and codec bugs | `cargo-fuzz` targets for the service's `Execute` (statement and parameters), the change-set decoder and the portable-snapshot reader; 24 h clean before GA |

An **external security review** of the graph service, the durability path and the authorization model is a GA gate, as D718 makes it for Postgres.

## 17. Conformance and performance (D756, D757)

### 17.1 Conformance

| Suite | What | Gate |
|---|---|---|
| `gql-1` corpus | `conformance/graph/gql/`: statements and expected results per declared feature, plus negative cases for undeclared ones | 100 % of declared features; every undeclared feature answers `gql_feature_unsupported` |
| Differential | Expand, neighbors, degree and shortest path vs a `networkx` reference on fixture graphs, including link-applied updates, deletes and truncation (§07 §9's gate, kept) | Identical results |
| Recovery differential | Random workload, kill at failpoints, recover, compare a canonical dump with a model | Identical |
| Token conformance | D76's cases on the graph stream: write with a token, read with it on another node, during an owner move | Pass |
| openCypher TCK subset | Selected `.feature` scenarios for §9.3's clauses (openCypher TCK, Apache-2.0) | ≥ 90 %, every failure allowlisted with a reason |
| LangChain | `LoamsGraph`, a `langchain_community` `GraphStore` (`query`, `refresh_schema`, `get_schema`, `get_structured_schema`, `add_graph_documents`), against the framework's own graph-store tests at the pinned version (verify which exist) and a Loams-written suite | Pass |
| LlamaIndex | `LoamsPropertyGraphStore` (`PropertyGraphStore`), against the framework's property-graph store tests (verify) | Pass |
| LightRAG | `LoamsGraphStorage` (`BaseGraphStorage`), §07 §6's method set | Pass |

### 17.2 Performance (targets)

Workload: LDBC SNB Interactive-derived, generated with the LDBC SNB datagen (Apache-2.0) at SF1 (about 3 M nodes and 17 M edges, estimate) and SF0.1 for CI. These are **not audited LDBC results** and are never published as "LDBC benchmark results" (LDBC's naming rules).

| Gate | Target |
|---|---|
| IS1–IS7 short reads, hot | p50 ≤ 2 ms, p99 ≤ 20 ms |
| IC1, IC2, IC7, IC8, IC13 (shortest path), hot | p99 ≤ 500 ms each |
| 1-hop expand, 10 seeds, `limit_per_seed` 50, hot | p99 ≤ 10 ms |
| 2-hop expand, same, hot | p99 ≤ 50 ms |
| GraphRAG: ANN seeds (10) → 2 hops → vector rerank, hot | p95 ≤ 150 ms |
| The same, cold (graph evicted) | p95 ≤ 2 s |
| Single durable write | p99 ≤ WAL ack p99 + 10 ms |
| Write throughput, one graph, 1-statement batches with group commit | ≥ 5 000 transactions/s |
| Link ingest, one graph | ≥ 50 000 edges/s |
| Import, SF1 | ≤ 10 min |
| Open from snapshot, 1 GiB | ≤ 20 s |
| Memory, SF1 resident | ≤ 8 GiB |
| Binary size, `graph` feature on vs off, stripped | ≤ +15 MiB |

Grafeo's self-reported graph-bench numbers (SNB Interactive SF0.1, embedded: 2 904 ms total, 136 MB) are context, not evidence; GR1e measures the baseline first, and a target missed by more than 2× is escalated to the owner rather than quietly relaxed.

### 17.3 Reference topology

Three nodes of 8 vCPU, 32 GiB and local NVMe, the metastore on its default backend, an S3-compatible bucket in the same region; the graph role on all three, one standby per graph for the HA gates; a fourth machine as the load generator.

## 18. SDKs, integrations and the desktop contract (D758)

### 18.1 SDKs

- **TypeScript and Python first**, through SDK1's generation with a hand-written facade in the `loams.graph` module (§44 §7): `graph.query(statement, params?, opts?)` → typed rows (`Node`, `Relationship`, `Path` classes, `bigint` / `int` for `INT64`); `graph.stream(...)` → an async iterator over `ExecuteStream`; `graph.transaction([...], { isolation, ifVersion })`; `graphs.create/get/list/delete`; automatic `idempotency_key` on writes and retry on `ABORTED`.
- Python extras `loams[langchain]`, `loams[llama-index]`, `loams[lightrag]` hold the three adapters (§17.1). They replace §07 §6's adapters with the same class names, now backed by Loams Graph. Grafeo's own `grafeo-langchain` and `grafeo-llamaindex` packages are references, not dependencies.
- The other eleven languages get the generated client through SDK2, with no facade until demand.

### 18.2 The desktop Graph page

D674 made the page an empty state "until a binary serves `loams.graph.v1`". With D741 the binary is `loams`, and the page's contract is:

| Server state | Detected by | Page |
|---|---|---|
| No `loams.graph.v1` row in `GetInstance.services[]` | Instance call | D674's empty state, reworded: "Graph is not served by this server" |
| Row present, `available = false` | Instance call | "Graph is not in this server's variant", with the variant from `ErrorInfo.metadata` |
| Row present, `available = true` | Instance call | The Graph page below |

- **Graphs list:** `ListGraphs` (page size 50), with Create (owned, name, languages) and Delete (typed confirmation; hidden without `can_admin`, which the page learns from a `PERMISSION_DENIED` rather than guessing).
- **Editor:** GQL (and Cypher when the graph enables it) with parameters as a JSON object, a `read_only` toggle that is on by default, Run, Explain and Profile. Errors underline `line`/`column`/`length` and show `gqlstatus` and the reason.
- **Results:** a table of `RowSet` with typed cells, and a graph view drawn from `Node`, `Relationship` and `Path` values, capped at 500 nodes (the rest stay in the table). A `truncated` banner offers "Stream all" through `ExecuteStream`, capped at 100 000 rows in the page.
- **Schema sidebar:** `GetSchema` (labels, edge types, property keys, counts, indexes).
- **Calls:** `max_rows = 1000`, `timeout_ms = 30000`, `consistency = strong`; Cancel aborts the request, and the server cancels the statement on disconnect.
- **History:** the last 100 statements per server in the renderer's local storage, statement text only, never parameters.
- **Transport:** the console's Connect client through `loams-app://` (D657's proxy). No new IPC channel and no main-process code.
- **Local edition:** the desktop's bundled `loams` is built with `graph` (the `full` variant, or `standard` once Q675 says so), so the page works against `loams dev`.

The fixtures that pin this contract (`conformance/graph/desktop/*.json`) are shared by the server's tests, `loams-apps-mock`'s seeded graph and the page's tests (GR1a).

## 19. Licensing and the Grafeo dependency (D759)

### 19.1 Licences

| Component | Licence | Use |
|---|---|---|
| `grafeo`, `grafeo-core`, `grafeo-engine`, `grafeo-adapters`, `grafeo-common`, `grafeo-storage` 0.5.43 | Apache-2.0 (crates.io; `grafeo-storage` re-read 2026-10-08) | Linked into `loams` behind `graph` |
| Grafeo `NOTICE` ("Grafeo, Copyright 2025-2026 S.T. Grond") | Apache-2.0 §4(d) | Reproduced in Loams' `NOTICE` |
| Grafeo's transitive tree | Checked by the root `deny.toml` once the crate moves | Any refused licence blocks the move |
| openCypher TCK | Apache-2.0 | Test data, not shipped |
| LDBC SNB datagen | Apache-2.0 | Bench tooling, not shipped; results are not called LDBC results |
| ISO/IEC 39075 | Copyrighted standard | Cited by section and feature id, never copied |

"Grafeo" is not used as a product name. Docs say "Loams Graph, built on the Grafeo engine".

### 19.2 Pins

Exact pin, at least 14 days old. On 2026-10-08 that is 0.5.43 (2026-09-27); 0.5.44 (2026-10-04) is too new. Every bump runs the full gate set in §17.

### 19.3 The fork trigger

Grafeo's repository is eight months old (created 2026-01-26) and one author wrote about 94 % of its commits (1 734 of the top contributors' 1 847, GitHub API, 2026-10-08). D634 (f) and I6 are why that is acceptable: Loams' data is in Loams' format, and the engine can be rebuilt or replaced. Loams forks Grafeo, under Apache-2.0 and with its NOTICE, when any of these holds: a security issue affecting Loams is unfixed upstream for 30 days; no release for 90 days while Loams needs a fix; an upstream licence change; or an upstream API change (such as the planned 0.6 `MutationListener` refactor) that Loams cannot follow within one release. Before that, Loams contributes the hooks it needs upstream (Q679).

## 20. Docs

Before GA, on the docs site: a Graph overview (owned vs linked), a quickstart (TypeScript and Python), the `gql-1` reference, the Cypher compatibility page with its exact scope, the limits page entries (§13), GraphRAG cookbook pages (seed-and-expand, community summaries, temporal edges, as §07 §8 lists), the LangChain / LlamaIndex / LightRAG guides, "Migrating from Neo4j" (export, Cypher dialect, what is not supported), the operations runbook (failover, restore, eviction, memory), and the security model.

## 21. Exit checklist for production (D740)

- [ ] `loams.graph.v1` served by `loams` with `graph` on, in the catalogue, `unstable` removed, `buf breaking` enforced.
- [ ] I1–I6 hold: the durability tests, the recovery differential and the fault matrix pass in CI.
- [ ] Ownership fenced; failover within D753's RTO on the reference topology, with and without a standby.
- [ ] Consistency tokens pass D76's conformance cases on the graph stream.
- [ ] Linked graphs: `collection → graph` and `stream → graph` links exactly once under faults; `Search.expand` and the four SQL table functions pass the differential.
- [ ] Unified auth and per-graph authorization in place; a non-loopback listener refuses to start without an authorizer; classifier disagreement count is 0 on the corpus.
- [ ] Limits and quotas enforced with the named reasons; the memory tests show a statement failing, never the process.
- [ ] Snapshots, PITR restore, export and import tested; the first monthly restore drill passed.
- [ ] Metrics, traces, slow log, audit, dashboards and alerts shipped; literal redaction tested.
- [ ] Security controls in §16 tested; fuzzing 24 h clean; external review findings closed or accepted by the owner.
- [ ] `gql-1` corpus 100 %; openCypher TCK subset ≥ 90 % with an allowlist; the three framework suites pass.
- [ ] Every §17.2 target met or explicitly waived by the owner.
- [ ] TypeScript and Python SDKs released with the `loams.graph` facade; the three Python adapters released.
- [ ] The desktop Graph page works against `loams dev` and a remote server.
- [ ] Licences and NOTICE updated; the Grafeo pin is at least 14 days old.
- [ ] Docs in §20 published.

## 22. Risks

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| 1 | Grafeo's CDC misses mutations or DDL, so mechanism B cannot satisfy I1/I4 | Medium | High | Task 9 tests completeness with a model-based fuzzer before anything depends on it; mechanism A as the fallback; contribute a commit hook upstream |
| 2 | Element ids cannot be preserved on replay | Medium | Medium | The `_lid` map (§6.3) |
| 3 | `set_viewing_epoch` does not isolate reads as I2 needs, or GC removes the pinned versions | Low–Medium | High | Tested in Task 9; fallback: a visibility gate that holds new client sessions until the in-flight group is durable, at a latency cost |
| 4 | Grafeo's memory limit or timeout does not stop a runaway statement | Medium | High | GR1d memory and timeout tests; a watchdog that poisons and reloads the graph; graph-only processes |
| 5 | Single maintainer; upstream API churn (0.6 `MutationListener`) | High | Medium | I6, the fork trigger, upstream contributions |
| 6 | Performance targets out of reach on Grafeo | Medium | Medium | Measure first (GR1e Task 0 of the perf work); escalate past 2×; §07's sidecars remain a deferred alternative for expand |
| 7 | Two graph stories (linked vs owned) confuse users | Medium | Low | One object kind, one API, one docs page that explains modes |
| 8 | The single-writer lane caps write throughput per graph | Medium | Medium | Group commit; many graphs per namespace; partitioned graphs are Phase C (Q677) |
| 9 | Arrow 60 and Grafeo's tree slow the engine build | High | Low | Off by default; measured; built only in `full` and in CI's graph job |

## 23. Open questions (Q670–Q684)

| # | Question | Owner | Needed by |
|---|---|---|---|
| Q670 | Approve D743: Loams Graph also executes §07's `expand` stage and SQL table functions, with §07's CSR sidecars and `ExpandExec` deferred? Default: yes | Owner | GR1c |
| Q671 | Approve D741: the `graph` role of `loams` (feature `graph`), not `loams-fabric`? Default: yes | Owner | GR1a Task 1 |
| Q672 | Which change-capture mechanism (§6.3 A, B or C)? Default: B, if Task 9's completeness tests pass | Eng | GR1b Task 9 |
| Q673 | Ship the Cypher compatibility dialect at GA (opt-in per graph), or GQL only? Default: ship it opt-in | Owner | GR1e |
| Q674 | Is label- or property-level security needed for v1? Default: no; per-graph only | Owner | GR1d |
| Q675 | Put `graph` in the `standard` variant at GA, or keep it in `full`? Default: `standard` if the size gate passes | Owner | GR1e exit |
| Q676 | Interactive transactions across RPCs (session handles) in v1? Default: no; `ExecuteBatch` and `if_version` | Owner | Before GA |
| Q677 | Largest supported graph at GA, and when partitioned graphs come? Default: one node's memory plus spill, 100 M elements (target); partitioning in Phase C | Owner | GR1e |
| Q678 | Fork Grafeo now, or track upstream under §19.3's trigger? Default: track upstream | Owner | GR1a Task 0 |
| Q679 | Contribute a commit hook (change-set listener) and caller-chosen ids upstream? Default: yes, as soon as Task 9 knows what is missing | Eng | GR1b |
| Q680 | Pursue an audited LDBC SNB run? Default: not before GA | Owner | After GA |
| Q681 | Dangling edges in linked graphs: stub vertices (default) or skip, as §07 §7 had it? | Eng | GR1c |
| Q682 | Offer a Bolt endpoint or a `grafeo-server` companion for Neo4j-driver users? Default: no (D634 (c)) | Owner | After GA |
| Q683 | A public graph change feed (`WatchGraph`, server streaming) and GQL procedures that call collection search? Default: Phase B | Owner | After GA |
| Q684 | The metering units for graphs (statement CPU-seconds, stored elements, stored bytes) for `loams-platform` | Owner | Before Cloud GA |

## 24. Contradictions with earlier decisions, and how they are resolved

| Earlier | Conflict | Resolution |
|---|---|---|
| D44 (no Cypher, mapped graphs only, sidecars and `ExpandExec`) | Loams Graph is a stored graph, with an opt-in Cypher dialect | D743 keeps D44's API and defers its engine; D748 makes Cypher opt-in. Proposed; needs the owner (Q670, Q673) |
| D350 and Q339's answer (Grafeo not adopted; companion only on demand) | Grafeo becomes the production graph engine | D740: the owner's goal is the demand; D742, D749 and D759 answer D350's four concerns |
| D343 (graph in `loams-fabric`), D634 (b) (embedded in the Fabric) | The graph runs in `loams` | D741; `loams-fabric` keeps ingest, House and Flow |
| D51 (no embedded engines in the engine binary) | Grafeo linked into `loams` | D741 amends D51 for a pure-Rust engine with no Arrow types crossing its boundary, behind a non-default feature |
| D674 (Graph page empty until `loams-fabric`) | The page can be live when `loams` advertises the service | D758 amends D674 |
| §07 §7 (dangling edges are skipped) | Stub vertices in linked graphs | Q681 |
| §33 A.3's Grafeo row (`auth: none`, embedded) | A networked, authenticated service | §4.4: the connector becomes a Connect client with key auth |

## 25. Sources

- Code on `dev`, 2026-10-08: `fabric/crates/loams-graph/{Cargo.toml,src/lib.rs,src/engine.rs,src/service.rs,tests/graph.rs}`, `fabric/proto/loams/graph/v1/graph.proto`, `fabric/Cargo.lock`, `crates/loams/src/api/connect.rs`, `crates/loams/Cargo.toml`, `connectors/registry/grafeo.yaml`, `connectors/licences.toml`, `.github/workflows/fabric.yml`.
- Grafeo at tag `v0.5.43`: `crates/grafeo-engine/src/{database/mod.rs,config.rs,session/mod.rs,cdc.rs,auth.rs,transaction/{mod.rs,prepared.rs}}`, `NOTICE`; crates.io API for `grafeo` (versions, features, licences) and `grafeo-storage`, `grafeo-engine`, `grafeo-adapters`, `grafeo-common`; GitHub API for the repository's creation date and contributors; the README (backup, CDC, resource limits, graph-bench numbers, integrations). All read 2026-10-08.
- Design: §01, §07, §09, §12, §18, §19, §32 §7.3 and §7.7, §33 A.3, §37 §19.10, §44, §46; decisions D5, D44, D51, D65, D66, D76, D111, D343, D350, D600, D607, D634, D674; the [graph DB spike](../plans/graph-db-rust-spike.md).
