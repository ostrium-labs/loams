# GR1 — Loams Graph in Production Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Each task lists the interfaces it must produce and the tests that must exist and pass before it is done. Where this plan gives exact code, use it. Where it gives a contract and named tests, write the code to that contract, and record any deviation in "Rulings made during execution" at the end of this file.
>
> **Status: Planned** (2026-10-08). Track GR, design [§48](../design/48-loams-graph-production.md) (D740–D759, Q670–Q684), building on [§07](../design/07-graph.md) (D44) and D634. GR1a and GR1b can start before the owner answers Q670 and Q673; GR1c Tasks 20–21 wait for Q670, and GR1e Task 33 waits for Q673. Q671 (where it runs) must be answered before Task 1 merges.

**Goal:** Loams Graph GA. A stored property graph that Loams owns, queried and written with GQL over `loams.graph.v1`, executed by an embedded Grafeo engine inside the `loams` binary, made durable by Loams' log and bucket, fed by links from collections and streams, used for GraphRAG, protected by the unified auth plan, operable (quotas, HA, PITR, observability), and proven by conformance and performance gates. The exit is §48 §21's checklist, repeated at the end of this plan with the owning tasks.

**Architecture** (§48 §4, §6, §7):
- **`crates/loams-graph`** (moved from `fabric/`) holds the engine wrapper, the service handlers, the change-set codec, the durability pipeline, the ownership logic, the link targets and the retrieval adapters. It is a dependency of `loams` behind the cargo feature `graph` (off by default).
- **`loams.graph.v1`** (`proto/loams/graph/v1/graph.proto`, generated into `loams-proto`) has `GraphAdminService` and `GraphService`. It is a row in `crates/loams/src/api/connect.rs`'s `CATALOGUE`, `available` when `graph` is compiled in, `unstable` until Task 39.
- **Durability:** every committed write transaction becomes one `GraphChangeSet` in the graph's implicit stream in the Loams WAL and is acknowledged after that append. Client sessions read at the durable epoch. Snapshots go to the bucket; a manifest in the metastore names the snapshot and the offset it covers. Grafeo's local files are a cache.
- **Ownership:** one fenced owner per graph (a lease with an epoch), followers for `eventual` and `at_least` reads, forwarding over `loams.internal.v1`.
- **Retrieval:** linked graphs are fed by `LinkTarget`s; `QueryService/Search`'s `expand` stage and the `graph_*` SQL table functions call the in-process engine.

**Tech Stack:**
- Rust 1.97 (workspace `rust-version`), edition 2024, workspace lints.
- `grafeo` **0.5.43** exact (D759: 0.5.44 of 2026-10-04 is under 14 days old on 2026-10-08), `default-features = false` with the feature set Task 0 measures; `grafeo-engine` named directly for `lpg` if Task 0 confirms it is needed; `grafeo-common` 0.5.43.
- `connectrpc` 0.9 and `buffa` through `loams-proto` (§44); `buf` for protos.
- Workspace crates: `loams-log` (implicit streams), `loams-meta` (catalog, leases, manifest CAS), `loams-store` (bucket), `loams-link` (`LinkTarget`), `loams-worker` (leases and jobs), `loams-query` (DataFusion UDTFs, `SearchRequest`), `loams-sim` (failpoints and the seeded harness).
- `parquet` and `arrow` at the **workspace's** version (58) for the portable snapshot; never Grafeo's Arrow.
- Tests: `proptest` (workspace), `cargo-fuzz` (nightly, CI only), Python 3.12 with `networkx` (differential), the openCypher TCK (pinned commit), the LDBC SNB datagen (pinned image), `pytest` for the framework suites.
- TypeScript: the console's `@connectrpc/connect-web`, Vitest; React 19.3.0 for the Graph plugin (exact pins as AP1e's Global Constraints).

**Spec:**
- [§48](../design/48-loams-graph-production.md) (all of it), D740–D759 and Q670–Q684 in the [decision log](../design/13-decision-log.md).
- Kept contracts: §07 §2.1 (the mapping DDL), §5 (SQL table functions and `expand`), §6 (adapter method sets), §7 (write semantics, amended by §48 §10.1), §9 (gates).
- §44 (API rules, catalogue, `feature_not_in_variant`), §19 §5 and D66 (auth), D76 (tokens), D607 (`loams.internal.v1`), §37 §19.10 and D674/D758 (the Graph page).
- The as-built reconciliation in §48 §5 is the starting point for Task 0.

## Global Constraints

- **Worktree and branch.** Work in `~/Documents/Ostriumlabs/loams-wt/gr1-graph-production`, on branch `feat/gr1-graph-production`, based on `dev`. One PR per milestone (GR1a … GR1e) unless a task says otherwise. Use `git commit -s` (DCO). Commit areas: `graph`, `proto`, `api`, `link`, `query`, `sdk`, `plugins`, `bench`, `conformance`, `docs`, `ci`.
- **Rust builds** use the shared target directory (`~/Documents/.cargo/config.toml`). Never set `CARGO_TARGET_DIR`, never build in `/tmp`. Build one crate graph at a time (D127): `cargo test -p loams-graph` while iterating, `cargo test -p loams --features graph` before a commit that touches `loams`. Never `cargo build --workspace --all-features` locally.
- **The feature is off by default.** `cargo build -p loams` without `graph` must not compile Grafeo; CI checks it with `cargo tree -p loams -e normal | grep -c grafeo` = 0.
- **Statements are never rewritten** (D634). Loams may read a statement to classify it and to redact literals for logs; the bytes the engine runs are the bytes the client sent. Loams-generated statements (expand templates, link writes) bind every value as a parameter.
- **No client-chosen paths.** No RPC field is a filesystem path or a URL. Import and export take object keys under the namespace's own prefix.
- **Pins.** Exact versions, at least 14 days old (`cargo info`, `npm view <pkg> time`). Record versions in the commit that adds them.
- **Licences.** Every new crate or package passes `cargo deny check licenses` (root `deny.toml`) and `connectors/licences.toml` is kept in step (Task 1).
- **Secrets and literals.** No statement text with literals and no parameter value reaches a log, a span or a metric label. Use `redact_literals` (Task 6) everywhere.
- **Names.** Product "Loams Graph"; crate `loams-graph`; feature and role `graph`; proto package `loams.graph.v1`; metric prefix `loams_graph_`; error reasons as §48 §8.3 lists them; ids `gr_<ULID>`.

## Review Focus

1. **A write is acknowledged but lost, or seen and then lost.** Kill the owner between Grafeo commit and the log append, and between the append and the answer. Expected: an unacknowledged write may vanish but was never visible to a client session; an acknowledged one is always recovered. Tests: Task 11 `no_read_sees_non_durable_write`, Task 16 `kill_matrix_acked_writes_survive`.
2. **Two owners commit.** Partition the old owner after a new one takes the lease. Expected: the old owner's append is refused by epoch and it fences itself. Test: Task 12 `stale_owner_append_is_fenced`.
3. **A reader runs a write.** A `reader` sends `MATCH (n) SET n.x = 1`, a `CALL` of a writing procedure, a statement hidden behind comments, or a Cypher `MERGE` on a GQL-only graph. Expected: `PERMISSION_DENIED` or `FAILED_PRECONDITION`, nothing written. Tests: Task 24 `reader_cannot_write_any_corpus_write`, Task 3 `guard_and_engine_agree_on_corpus`.
4. **A client touches the server's filesystem.** Any field that could name a path or URL. Expected: none exists; import/export keys outside the namespace prefix are refused. Tests: Task 2 `proto_has_no_path_fields`, Task 29 `import_outside_namespace_prefix_denied`.
5. **One graph hurts another.** A runaway statement (cartesian product, unbounded path) or a panic. Expected: that statement fails with `graph_statement_timeout` or `graph_memory_limit`, or that graph reloads; other graphs keep serving. Tests: Task 26 `runaway_statement_fails_not_process`, Task 3 `engine_panic_poisons_one_graph`.
6. **Linked graphs drift from their sources.** Kill a link worker mid-batch, replay. Expected: the graph equals the mapping applied to the collections at the link's offset. Test: Task 18 `link_apply_exactly_once_under_kill`.

---

## File structure

```
proto/loams/graph/v1/graph.proto                 (moved + reworked, Task 2)
crates/loams-proto/                              (generates loams.graph.v1; Task 2)
crates/loams-graph/
  Cargo.toml                                     (moved from fabric/, Task 1)
  src/lib.rs
  src/engine.rs                                  (Grafeo wrapper; GraphHandle, sessions, roles)
  src/classify.rs                                (keyword guard + engine StatementKind; Task 3)
  src/value.rs                                   (typed value model <-> grafeo::Value <-> proto; Task 3)
  src/service/{mod.rs,admin.rs,data.rs,stream.rs,errors.rs}   (handlers; Tasks 2-6)
  src/catalog.rs                                 (GraphMeta in the metastore; Task 4)
  src/limits.rs                                  (statement limits, semaphores; Task 6)
  src/redact.rs                                  (literal redaction; Task 6)
  src/changeset/{mod.rs,codec.rs,capture.rs}     (GraphChangeSet v1; Tasks 9-10)
  src/durable/{lane.rs,append.rs,visibility.rs,idempotency.rs}  (write path; Task 11)
  src/owner.rs                                   (lease, fencing, forwarding; Task 12)
  src/snapshot/{native.rs,portable.rs,manifest.rs,checkpoint.rs}  (Task 13)
  src/recovery.rs, src/evict.rs                  (Task 14)
  src/replica.rs, src/consistency.rs             (Task 15)
  src/mapping.rs                                 (GraphMapping, §07 §2.1; Task 17)
  src/link/{collection.rs,stream.rs}             (LinkTarget impls; Tasks 18-19)
  src/retrieval/{expand.rs,udtf.rs,algos.rs}     (Tasks 20-22)
  src/authz.rs                                   (Task 24)
  src/quota.rs, src/memory.rs                    (Tasks 25-26)
  src/metrics.rs, src/audit.rs                   (Task 27)
  src/ops/{restore.rs,export.rs,import.rs}       (Task 29)
  tests/{graph.rs,service.rs,durability.rs,ownership.rs,recovery.rs,links.rs,retrieval.rs,authz.rs,limits.rs,ops.rs}
  fuzz/                                          (cargo-fuzz targets; Task 31)
crates/loams/Cargo.toml                          (feature `graph`; Task 5)
crates/loams/src/api/connect.rs                  (catalogue row + mount; Task 5)
crates/loams/src/api/graph.rs                    (wiring only; Task 5)
crates/loams/src/server.rs                       (role `graph`, config; Tasks 5, 12)
crates/loams-apps-mock/                          (seeded loams.graph.v1; Task 7)
crates/loams-query/src/                          (graph UDTFs, expand stage hook; Tasks 20-21)
conformance/graph/{desktop,gql,tck,frameworks}/  (Tasks 7, 32, 33, 37)
bench/graph/{ldbc,graphrag}/                     (Tasks 34-35)
spec/graph/gql-1.md                              (Task 32)
sdks/{typescript,python}/…/graph*                (Tasks 36-37; paths per SDK1 Task 0)
web/plugins/graph/                               (Task 8)
deploy/{helm,dashboards,alerts}/graph*           (Task 30)
docs/…                                           (Task 38)
fabric/crates/loams-graph*, fabric/proto/loams/graph/   (deleted, Task 1)
```

## Shared contracts (all tasks use these names)

```rust
// crates/loams-graph/src/lib.rs (public surface; bodies per task)
pub struct GraphId(pub ulid::Ulid);                       // "gr_<ULID>" on the wire
pub enum GraphMode { Owned, Linked(GraphMapping) }
pub struct GraphMeta {
    pub id: GraphId, pub namespace: String, pub name: String, pub mode: GraphMode,
    pub languages: Vec<QueryLanguage>, pub limits: GraphLimits, pub replicas: u32,
    pub state: GraphState, pub created_at: Timestamp, pub version: u64,      // CAS version
}
pub enum Consistency { Strong, Eventual, AtLeast(ConsistencyToken) }
pub struct ConsistencyToken { pub stream: StreamId, pub offset: u64 }        // D76 shape, one partition
pub enum Access { Read, Write, Admin }                                        // classify.rs
pub struct ExecOptions { pub read_only: bool, pub consistency: Consistency, pub timeout: Duration,
                         pub max_rows: u32, pub idempotency_key: Option<String>, pub isolation: Isolation }
pub struct ExecOutcome { pub rows: RowSet, pub truncated: bool, pub counters: Counters,
                         pub token: Option<ConsistencyToken>, pub commit_epoch: Option<u64>, pub elapsed: Duration }

#[async_trait::async_trait]
pub trait GraphRuntime: Send + Sync {                    // what the handlers call; owner or forwarder
    async fn execute(&self, g: &GraphMeta, stmt: &Statement, opts: ExecOptions) -> Result<ExecOutcome, GraphError>;
    async fn execute_batch(&self, g: &GraphMeta, stmts: &[Statement], atomic: bool, opts: ExecOptions,
                           if_version: Option<u64>) -> Result<BatchOutcome, GraphError>;
    async fn schema(&self, g: &GraphMeta) -> Result<GraphSchema, GraphError>;
}

// changeset/mod.rs (GraphChangeSet v1, §48 §6.4)
pub enum ChangeOp {
    UpsertNode { id: u64, labels: Vec<String>, props: PropMap },
    DeleteNode { id: u64 },
    UpsertEdge { id: u64, ty: String, src: u64, dst: u64, props: PropMap },
    DeleteEdge { id: u64 },
    SetProps   { id: u64, kind: ElementKind, set: PropMap, removed: Vec<String> },
}
pub struct GraphChangeSet { pub graph: GraphId, pub lease_epoch: u64, pub commit_epoch: u64,
                            pub txn_seq: u64, pub idempotency_key: Option<String>, pub ops: Vec<ChangeOp> }
pub struct SchemaChange { pub graph: GraphId, pub lease_epoch: u64, pub commit_epoch: u64,
                          pub statement: String, pub language: QueryLanguage }
pub enum GraphRecord { ChangeSet(GraphChangeSet), Schema(SchemaChange) }   // version byte 1
```

Error reasons and their codes are §48 §8.3's list; `src/service/errors.rs` is the only place that maps `GraphError` to `ConnectError`.

## Milestones

| Milestone | Tasks | Outcome |
|---|---|---|
| **GR1a** Serve GraphService + desktop contract | 0–8 (9 tasks) | `loams --features graph` serves `loams.graph.v1` (reworked proto, catalog, limits v1); the desktop Graph page works against `loams dev` and the mock. Graphs persist only through Grafeo's own files (dev quality, labelled so) |
| **GR1b** Durable storage on Loams | 9–16 (8 tasks) | Change sets in the log, ack after append, durable-epoch reads, fenced owners, snapshots and manifest, recovery, eviction, consistency tokens, the fault gate |
| **GR1c** Links + GraphRAG | 17–23 (7 tasks) | Linked graphs from collections and streams, `Search.expand`, SQL `graph_*` UDTFs, algorithms, the GraphRAG example |
| **GR1d** Auth, ops, HA | 24–31 (8 tasks) | Per-graph authz, quotas, memory governance, observability, standby failover, PITR/export/import, deployment, security hardening |
| **GR1e** Conformance, perf, SDKs | 32–39 (8 tasks) | `gql-1`, openCypher TCK subset, LDBC SNB-derived and GraphRAG benchmarks, TS and Python SDKs, framework adapters, docs, the production exit |

---

## GR1a — Serve GraphService and the desktop contract

### Task 0: Reconcile with the code as built

**Files:** this plan's "Rulings made during execution" section only.

Steps:
1. Answer each of the following and record the answer, with file paths and commands, as a ruling R0.n:
   - **As built.** Re-read §48 §5's twelve findings against `dev`; mark each still true or fixed.
   - **Grafeo features.** With a scratch manifest in the worktree (never `/tmp`), find the narrowest feature set that keeps LPG + GQL + WAL + CDC + spill + algos + metrics + tracing and drops `ai`, `arrow-export`, `cypher` and the other languages. Record `cargo tree -e features -i grafeo-engine` and whether `arrow-array 60` is still in the tree. Then the same with `cypher` added (for Task 33).
   - **Grafeo APIs** at 0.5.43, each with a 10-line probe test in the scratch crate: `Session::set_viewing_epoch` isolates a session from later commits; `prepare_commit().commit()` returns the commit epoch; CDC events for a committed transaction are retrievable by epoch and include label changes and edge endpoints; whether DDL appears in CDC; whether a node/edge can be created with a caller-chosen id; `Config::query_timeout` cancels a running statement (and whether it is per session); `memory_limit` fails the statement, not the process; `session_with_role(Role::ReadOnly)` refuses every write in Task 3's corpus; `backup_full` on a checkpointed database; `panic = "abort"` is not forced by any Grafeo crate.
   - **Shared state.** Does Grafeo keep any process-global cache holding data (plan cache, string interner, CDC)? List each `static`/`OnceLock` in the six crates.
   - **Procedures.** Which procedures and functions Grafeo 0.5.43 exposes to GQL; which touch the filesystem or the network (for Task 31's allowlist).
   - **Engine side of the API.** How `loams-proto` generates a package (its `build.rs`), how a `CATALOGUE` row is added and mounted (API1's last task), whether an auth interceptor exists yet (MT1 state), how `Roles` in `crates/loams/src/server.rs` parse, and how `loams.internal.v1` is served.
   - **Link framework.** `loams-link`'s `LinkTarget` and `LinkTargetFactory` signatures and the exactly-once contract (`crates/loams-link/src/target.rs`, `registry.rs`).
   - **Search IR.** Whether `crates/loams-query/src/ir.rs`'s `SearchRequest` already has §07's `expand` and `rerank` fields.
   - **Desktop.** The state of `web/plugins/graph` (AP1e Task 27) and how a plugin calls a Connect service through the proxy.
   - **Licences.** Re-read the six Grafeo crates' licences and Grafeo's `NOTICE` at the pinned version.
   - **Pin.** The newest Grafeo release at least 14 days old today.
2. Record any finding that changes a later task as a ruling, and the task it changes.
3. Commit: `docs(gr1): task 0 rulings`.

### Task 1: Move `loams-graph` into the root workspace

**Files:**
- Move: `fabric/crates/loams-graph/` → `crates/loams-graph/`; `fabric/proto/loams/graph/v1/graph.proto` → `proto/loams/graph/v1/graph.proto` (content unchanged in this task).
- Delete: `fabric/crates/loams-graph-proto/`; the graph entries in `fabric/Cargo.lock` (regenerate).
- Modify: root `Cargo.toml` (member, `grafeo*` workspace deps with Task 0's features), `crates/loams-graph/Cargo.toml` (depend on `loams-proto`, not `loams-graph-proto`), `connectors/licences.toml` (add `grafeo-storage`), root `NOTICE` (Grafeo's notice), `deny.toml` if Task 0 found a licence to allow, `.github/workflows/ci.yml` (a `graph` job: `cargo test -p loams-graph`; the "no grafeo without the feature" check), `fabric.yml` path filters if they name graph.

**Interfaces produced:** none new; `loams_graph::{Engine, Graph, service::*}` compile from the new location.

Tests:
- The existing `crates/loams-graph/tests/graph.rs` passes unchanged except for imports.
- `ci: loams_default_build_has_no_grafeo` (the `cargo tree` check in Global Constraints).
- `licences_cover_every_grafeo_crate`: a test in `tools/` or the existing licence check that every `grafeo*` package in `Cargo.lock` has a stanza in `connectors/licences.toml`.

Steps: write the licence check first and see it fail on `grafeo-storage`; move; regenerate both lockfiles; run `cargo test -p loams-graph`; `cargo deny check`; commit `refactor(graph): move loams-graph into the engine workspace (D741)`.

### Task 2: Rework `loams.graph.v1`

**Files:** `proto/loams/graph/v1/graph.proto`; `buf.yaml` (ignores only if forced, with a comment); `crates/loams-proto` (generation); `crates/loams-graph/src/value.rs`; `crates/loams-graph/tests/proto.rs`.

**Interfaces produced:** §48 §8.2's services and messages, with `ModuleOptions { unstable: true }`, `NO_SIDE_EFFECTS` on reads, `idempotency_key` on mutations, AIP-158 pagination on `ListGraphs`; `go_package` per §44's Go module rule. `value.rs`: `to_proto(&grafeo::Value) -> pb::Value`, `from_proto(&pb::Value) -> Result<grafeo::Value, GraphError>`, lossless for every GQL type Grafeo has.

Tests:
- `buf lint` and `buf format --diff` clean.
- `proto_has_no_path_fields`: walks the descriptor and fails on any field named `*path*`, `*url*`, `*dir*` or `*file*` outside `ImportGraphRequest.object_key`/`ExportGraphRequest.object_prefix`.
- `int64_round_trips_exactly` (`i64::MAX`, `2^53 + 1`), `uint64_round_trips`, `temporal_values_round_trip`, `node_relationship_path_round_trip`, `nested_list_map_round_trip`.
- `connect_json_mapping_golden`: JSON forms of `Value` against `conformance/graph/desktop/values.json` (the fixture the page uses, Task 7).

Commit `feat(proto): loams.graph.v1 reworked for production (D746)`.

### Task 3: Engine wrapper fixes and statement classification

**Files:** `src/engine.rs`, `src/classify.rs`, `src/service/data.rs`, `src/service/errors.rs`, `tests/graph.rs`, `tests/service.rs`.

**Interfaces produced:**
- `classify(stmt: &str, language: QueryLanguage) -> Access` (keyword guard, kept from `writes`/`bare_words`) and `engine_classify(db, stmt) -> Result<Access, GraphError>` (Grafeo's `StatementKind`); `Graph::session_for(access: Access)` returning a Grafeo session with the matching `Role`.
- Execute binds `parameters`; per-statement `language` is checked; non-atomic batches bind parameters; every engine call is wrapped in `catch_unwind` and a panic poisons the graph (`GraphState::Poisoned`).
- Storage path derives from `<data_dir>/graphs/<graph_id>/` only.

Tests:
- `execute_binds_parameters`, `non_atomic_batch_binds_parameters` (fails on today's code), `statement_language_override_is_checked`, `transaction_statements_refused` (`START TRANSACTION`, `COMMIT` → `graph_transaction_statement`).
- `guard_and_engine_agree_on_corpus`: for every statement in `conformance/graph/gql/classify/*.gql` (reads, writes, DDL, comment-hidden writes, string-literal keywords, `CALL`), the guard never says Read when the engine says Write or Admin; disagreements in the other direction are listed in the fixture's `expected_conservative` set.
- `read_session_refuses_every_corpus_write` (engine role `ReadOnly`).
- `engine_panic_poisons_one_graph`: a test-only failpoint panics inside one graph's call; that RPC answers `INTERNAL`/`graph_engine_panic`; a second graph keeps answering; the first reopens on the next call.

Commit `fix(graph): parameters, language checks, engine roles and panic containment`.

### Task 4: The graph catalog

**Files:** `src/catalog.rs`, `src/service/admin.rs`, `tests/service.rs`; metastore key additions in `loams-meta` if Task 0 shows they need a type (otherwise the generic object API).

**Interfaces produced:** `GraphCatalog { create, get, get_by_name, list(ns, page), update(cas), mark_deleting, purge }` over the metastore, keyed `graphs/<ns>/<name>` → `GraphMeta` with a CAS `version`; name rules (`[a-z][a-z0-9_-]{0,62}`); `CreateGraph`, `GetGraph`, `ListGraphs`, `UpdateGraph`, `DeleteGraph` (an Operation that closes the engine, then purges storage after the retention hold), `GetEngineInfo`, `GetSchema`.

Tests:
- `create_is_idempotent_by_key`, `create_duplicate_name_already_exists`, `list_paginates_aip158` (page tokens stable under inserts), `update_cas_conflict_aborted`, `delete_then_get_not_found`, `graphs_survive_restart` (catalog reloads; the engine reopens lazily), `invalid_names_refused`.
- `get_schema_reports_labels_types_counts`.

Commit `feat(graph): graph catalog and admin service`.

### Task 5: Mount in `loams` behind `graph`

> **From Task 4 review (M8):** `GraphAdmin::open` opens a graph from disk (`GrafeoDB::open`, WAL replay) while holding the engine registry lock, on the async path, and the data-plane wrappers run statements synchronously inside async functions. Task 5 mounts them through `spawn_blocking` (or Task 6's pool), and opening moves out from under the registry lock (a per-graph opening latch), so one slow open blocks neither the runtime nor other graphs.

**Files:** `crates/loams/Cargo.toml` (`graph = ["dep:loams-graph"]`), `crates/loams/src/api/connect.rs` (catalogue row `loams.graph.v1`, services `GraphService`, `GraphAdminService`, `available: cfg!(feature = "graph")`, `unstable: true`; a `GraphAbsent` stub answering `feature_not_in_variant`), `crates/loams/src/api/graph.rs`, `crates/loams/src/server.rs` (role `graph`; config `[graph] data_dir, idle_evict_after, node_memory, limits`), `release/` variant lists (`full` gets `graph`), `crates/loams/tests/it/graph.rs`.

Tests:
- `instance_advertises_graph_when_feature_on` and `…_unavailable_when_off` (two builds in CI).
- `graph_rpcs_answer_not_in_variant_without_feature`: every RPC of both services.
- `connect_json_grpc_and_grpc_web_reach_execute`: one statement over each protocol on the main port.
- `graph_requires_a_data_dir`: `loams serve` with `graph` and no `[graph] data_dir` refuses to start, unless it runs in an explicit dev or in-memory mode (`loams dev`, or a flag that says graphs are ephemeral). An in-memory graph fails rather than reopening after an engine panic (Task 3 review I2), so a served graph must be persistent.
- `non_loopback_listen_without_authorizer_refused`: `loams serve` with `graph` on a non-loopback address and the `AllowAll` authorizer exits with a clear message (D750; until MT1).
- `reflection_lists_only_served_or_stubbed_services`: every service `grpc.reflection.v1` lists (from `loams_proto::FILE_DESCRIPTOR_SET`) is either served or answered by a `not_in_variant` stub, with and without `graph` (Task 1 R1.1 made reflection list `GraphService` before it is mounted). A package that `loams-proto` compiles only because a served package imports it is exempt, and the test names it: `loams.operations.v1` (imported by `loams.graph.v1`, R2.3) until API1 serves `OperationsService`.

Commit `feat(api): serve loams.graph.v1 behind the graph feature (D741)`.

### Task 6: Limits v1, streaming and redaction

> **From Task 4 review (M8):** see the Task 5 note; the statement pool also takes the disk open.

> **From Task 3 re-review (2c):** every parse and engine call now runs on a freshly spawned 256 MiB-stack thread (`classify::on_big_stack`), and `gate` refuses a statement with more than `MAX_CHAIN_TOKENS` = 4000 operator-chain links before anything parses. Task 6 replaces the per-statement spawn with its blocking pool, sized with the same stack, and moves the limit into `StatementLimits`.

> **From Task 3 review (M2):** shortest-path searches (`ANY`/`ALL SHORTEST`, `shortestPath`, `allShortestPaths`, and a `ShortestPath` operator anywhere in the plan) are refused with `graph_unbounded_path`, because Grafeo's `ShortestPathOp` has no hop bound. Task 6 adds a bound and serves them within `StatementLimits`. The `MAX_PATH_HOPS = 10` constant in `classify.rs` also becomes `StatementLimits.max_path_hops`.

**Files:** `src/limits.rs`, `src/redact.rs`, `src/service/stream.rs`, `tests/limits.rs`.

**Interfaces produced:** `StatementLimits` (§48 §13.1 defaults and maxima), a per-namespace concurrency semaphore, deadline propagation into the engine (Task 0's mechanism), `ExecuteStream` in chunks of ≤ 1 000 rows or 1 MiB, `Explain`; `redact_literals(stmt) -> String` (strings and numbers replaced by `?`, using `bare_words`'s scanner) and `fingerprint(stmt) -> u64`.

Tests:
- `timeout_returns_statement_timeout` (an unbounded path query against a 10k-node fixture, timeout 200 ms; answer within 1 s), `client_cancel_stops_statement`.
- `unary_truncates_at_max_rows`, `unary_bytes_cap_result_too_large`, `stream_returns_all_rows_in_order`.
- `oversized_statement_rejected`, `batch_over_limit_rejected`, `namespace_concurrency_limit_resource_exhausted`.
- `redact_literals_strips_strings_and_numbers`, `fingerprint_ignores_literals`.

Commit `feat(graph): statement limits, streaming results and literal redaction`.

### Task 7: The desktop contract fixtures and the mock

**Files:** `conformance/graph/desktop/{instance.json,list_graphs.json,schema.json,execute_table.json,execute_graph.json,execute_truncated.json,error_syntax.json,error_denied.json,values.json}`; `crates/loams-apps-mock/src/graph.rs` (seeded `loams.graph.v1`: namespace `default`, graphs `movies` (owned) and `kg` (linked), deterministic answers from the fixtures); `crates/loams-graph/tests/desktop_contract.rs`.

**The contract** (§48 §18.2): detection from `GetInstance.services[]`; `ListGraphs` page size 50; `Execute` with `max_rows = 1000`, `timeout_ms = 30000`, `consistency = strong`, `read_only` default true; `Explain`/`Profile`; `GetSchema`; errors with `reason`, `gqlstatus`, `line`, `column`, `length`; `truncated` + `ExecuteStream`; typed values including `Node`, `Relationship`, `Path`.

Tests:
- `server_answers_match_desktop_fixtures`: the real service, seeded with the `movies` fixture graph, answers each fixture request with a response equal to the fixture (ignoring `elapsed_nanos`).
- `mock_answers_match_desktop_fixtures`: the mock does too.
- `syntax_error_has_position`: a broken statement's error carries `line`/`column`/`length` and a `gqlstatus` when Grafeo reports one.

Commit `feat(graph): desktop contract fixtures and seeded mock`.

### Task 8: The Graph page

**Files:** `web/plugins/graph/` (`@loams/plugin-graph`): `src/{GraphPage.tsx,GraphList.tsx,Editor.tsx,Results.tsx,GraphView.tsx,Schema.tsx,detect.ts,client.ts,history.ts}`, tests. Coordinate with the AP track owner before editing: AP1e Task 27 created the empty state; this task replaces it.

**Behaviour:** §48 §18.2 exactly. `detect.ts`: `graphAvailability(instance) -> 'absent' | 'not_in_variant' | 'available'`. `history.ts`: per-server, 100 entries, statement text only. Graph view: at most 500 nodes, deterministic layout seed so screenshots are stable. Delete needs the graph's name typed.

Tests (Vitest, against the fixtures of Task 7 through a fake transport):
- `availability_states_render` (three states), `list_and_create_graph`, `run_renders_table_with_typed_cells` (INT64 as bigint text), `run_renders_graph_view_capped_at_500`, `truncated_banner_offers_stream_all`, `syntax_error_underlines_position`, `read_only_toggle_default_on`, `history_never_stores_parameters`, `delete_requires_typed_name`, `permission_denied_hides_admin_actions`.
- E2E (AP1e's Playwright `_electron` smoke, extended): `graph_page_runs_query_against_loams_dev` — the desktop's bundled `loams` built with `graph` creates `movies`, runs `MATCH (n) RETURN count(n)`, sees one row.

Commit `feat(plugins): graph page with GQL editor (D758)`.

---

## GR1b — Durable storage on Loams

### Task 9: Spike: change capture, identity and visibility

**Files:** `crates/loams-graph/tests/capture_spike.rs` (kept as a regression test), rulings.

Steps:
1. Implement mechanism B (§48 §6.3) behind `trait ChangeCapture { fn begin(&self, s: &mut Session); fn take(&self, commit_epoch: u64) -> Result<Vec<ChangeOp>, GraphError>; }` as `CdcCapture`.
2. A model-based proptest: random GQL write transactions (insert, set, remove, delete, detach delete, label add/remove, edge insert/delete, multi-statement batches, rollbacks) applied to Grafeo; after each commit, `take(epoch)` applied to a pure in-memory model must equal a canonical dump of Grafeo. 10 000 cases.
3. Replay the captured ops into a fresh Grafeo; dumps must be equal, including element ids (try caller-chosen ids; else the `_lid` map).
4. Check that `set_viewing_epoch(e)` hides commits after `e` for the whole session, and that GC does not remove versions at a pinned epoch.
5. If any of 2–4 fails and cannot be fixed by Loams-side code, prototype mechanism A and repeat; record which mechanism passes (answers Q672) and what to contribute upstream (Q679).

Tests (kept): `cdc_capture_matches_model` (proptest), `replay_reproduces_dump_and_ids`, `viewing_epoch_hides_later_commits`, `pinned_epoch_survives_gc`, `ddl_capture_or_schema_record` (DDL either appears in capture or is routed to `SchemaChange`).

Commit `test(graph): change-capture spike and ruling (Q672)`.

### Task 10: The `GraphRecord` codec

**Files:** `src/changeset/{mod.rs,codec.rs}`, `tests/codec.rs`, `conformance/graph/records/v1/*.bin` (golden vectors).

**Interfaces produced:** `encode(&GraphRecord) -> Bytes`, `decode(&[u8]) -> Result<GraphRecord, CodecError>`; version byte `1`; property values in the typed model of Task 2; a CRC per record if the log record does not already carry one (Task 0 says).

Tests: `golden_vectors_decode` (frozen bytes → expected records), `round_trip_proptest`, `unknown_version_refused`, `truncated_input_refused_not_panics`, `decode_is_total_on_random_bytes` (proptest; also a fuzz target in Task 31).

Commit `feat(graph): GraphChangeSet v1 codec`.

### Task 11: The durable write path

**Files:** `src/durable/{lane.rs,append.rs,visibility.rs,idempotency.rs}`, `src/engine.rs`, `tests/durability.rs`.

**Interfaces produced:**
- Per graph: an implicit stream created with the graph (`loams-log`), id recorded in `GraphMeta`.
- `WriteLane::submit(stmts, opts) -> ExecOutcome`: §48 §6.2 steps 1–8; group commit (flush at 2 ms or 256 KiB, defaults); `durable_epoch` published through a watch channel.
- `Visibility::client_session(&Graph) -> Session` sets the viewing epoch to `durable_epoch`.
- `Idempotency`: `key → (token, outcome summary)` for 24 h, rebuilt from records on open.
- Append failure → `GraphState::Reloading`, all pending writers answered `UNAVAILABLE`/`graph_reloading`.

Tests:
- `acked_write_is_in_log`: after `Execute` answers, the record at the token's offset decodes to the write's ops.
- `no_read_sees_non_durable_write`: a failpoint delays the append; a concurrent strong read and a read on a new session both see the old state until the append completes.
- `failed_append_reloads_and_hides`: a failpoint fails the append; the writer gets `graph_reloading`; no client ever reads the value; after reload the graph equals the log.
- `idempotent_retry_returns_first_token`; `different_key_runs_again`.
- `batch_atomic_one_record`: a 50-statement atomic batch is one record.
- `group_commit_preserves_epoch_order` (proptest over concurrent writers).
- `write_conflict_aborted_and_retryable` (two `SERIALIZABLE` batches with write skew).
- `if_version_mismatch_fails_precondition`.

Commit `feat(graph): durable write path through the Loams log (D742)`.

### Task 12: Ownership, fencing and forwarding

> **From Task 4 review (M3):** `GraphAdmin::purge_expired` and `sweep_documents` run per node today, and remove `<data_dir>/graphs/<id>/` only on the node that runs them. With several `graph` nodes, the purge must run under a lease (one sweeper per cluster, or per graph owner), close the graph on whichever node owns it (forwarding), and remove the bucket snapshot and manifest (Task 13) as well as local caches on every node. A per-namespace failure is logged and skipped, which is already the case.

**Files:** `src/owner.rs`, `crates/loams/src/server.rs` (role wiring), `loams.internal.v1` additions (`GraphForward { Execute, ExecuteBatch }`, internal only), `tests/ownership.rs`.

**Interfaces produced:** lease `graph/<graph_id>/owner` (TTL 10 s, renew 3 s); placement by rendezvous over nodes with role `graph`; appends carry the lease epoch and the log refuses a stale one; a non-owner forwards; an owner that loses its lease stops serving and drops its engine.

Tests:
- `two_nodes_one_writer`: two in-process nodes; both receive writes; one log, one epoch sequence.
- `stale_owner_append_is_fenced`: partition (sim) the owner, the other node takes the lease, the old owner's pending append is refused and it fences; no acknowledged write is lost.
- `forwarded_write_answers_owner_token`.
- `owner_moves_on_node_leave` (graceful: checkpoint, release, new owner opens).

Commit `feat(graph): fenced single owner per graph (D744)`.

### Task 13: Snapshots and the manifest

**Files:** `src/snapshot/{native.rs,portable.rs,manifest.rs,checkpoint.rs}`, `tests/recovery.rs`.

**Interfaces produced:** `Checkpointer` (every 15 min or 64 MiB of log; on demand; before eviction and engine upgrades); native snapshot (Grafeo `backup_full` of a checkpointed database) uploaded to `ns/<ns_id>/graphs/<graph_id>/snap/<epoch>/`; portable snapshot (Parquet `nodes/`, `edges/` with the typed model, plus `manifest.json`) weekly and on demand; `GraphManifest { snapshots: [{epoch, offset, kind, keys, engine_version}], log_trim_offset }` CAS-committed in the metastore; log trim below the oldest retained snapshot and never inside the PITR window.

Tests:
- `snapshot_manifest_cas_conflict_retries`, `trim_never_passes_oldest_snapshot`, `trim_never_inside_pitr_window`.
- `portable_snapshot_round_trip`: export, import into a fresh engine, canonical dumps equal.
- `checkpoint_concurrent_with_writes_is_consistent`: the snapshot at epoch E plus records after E reproduces the live state.

Commit `feat(graph): snapshots in the bucket and the graph manifest`.

### Task 14: Recovery, rehydration and eviction

> **From Task 4 review (M3):** recovery must not reopen a graph whose catalog record is `deleting`, and eviction must release a deleted graph's local cache even when `purge_expired` has not run on this node (see the Task 12 note).

**Files:** `src/recovery.rs`, `src/evict.rs`, `tests/recovery.rs`.

**Interfaces produced:** `open(meta) -> Graph`: manifest → newest usable snapshot (native if `engine_version` readable, else portable) → replay after its offset → serve; `Evictor` (idle 1 h; node memory pressure, LRU) that checkpoints first.

Tests:
- `recover_equals_live`: random workload, drop the engine and every local file, reopen, dumps equal.
- `replay_twice_is_identity`.
- `evicted_graph_rehydrates_on_first_read` (and the first read's latency is recorded for §17.2's cold gate).
- `engine_version_mismatch_uses_portable_snapshot` (simulated by tagging the native snapshot with an unknown version).
- `local_cache_corruption_triggers_rebuild` (flip bytes in the `.grafeo` file).

Commit `feat(graph): recovery, rehydration and idle eviction`.

### Task 15: Consistency tokens and followers

**Files:** `src/replica.rs`, `src/consistency.rs`, `tests/ownership.rs`.

**Interfaces produced:** followers (`replicas` on `GraphMeta`) tail the graph stream and apply records; `Consistency::{Strong, Eventual, AtLeast}` routing (§48 §7.2); `consistency_wait_ms` default 5 000; token in the response and in the `loams-consistency-token` header; the header accepted on requests.

Tests:
- `token_read_on_other_node_sees_write` (D76's case), `token_read_during_owner_move_sees_write`, `token_wait_times_out_with_reason`, `eventual_read_served_by_follower`, `strong_read_routed_to_owner`.

Commit `feat(graph): consistency tokens and follower reads`.

### Task 16: The durability fault gate

**Files:** `crates/loams-graph/tests/faults.rs` (with `loams-sim` failpoints), CI job `graph-faults` (nightly + on PRs touching `crates/loams-graph/src/{durable,owner,snapshot,recovery}`).

Tests:
- `kill_matrix_acked_writes_survive`: failpoints at each of §48 §6.2's eight steps, the checkpoint upload, the manifest CAS and the log trim; for each, a seeded workload, a kill, a recovery on another node; every acknowledged write present, no client ever observed a value later lost (checked against a recorded history with a linearizability check on per-key registers, `loams-sim`'s checker).
- `bucket_errors_are_retried_not_lost` (S3 503 and timeouts injected through `loams-store`'s fault injection).
- `metastore_unavailable_stops_writes_not_reads` (eventual reads continue).

Commit `test(graph): durability fault gate`.

---

## GR1c — Links and GraphRAG

### Task 17: `GraphMapping`

**Files:** `src/mapping.rs`, `tests/links.rs`; `CreateGraph` accepts a mapping for `LINKED`; SQL `CREATE GRAPH … FROM COLLECTION …` (§07 §2.1) parsed in `loams-query` into the same `GraphMapping`.

**Interfaces produced:** `GraphMapping { vertices: [VertexMap { label, collection, key_field, props: Projection }], edges: [EdgeMap { ty | type_from_field, collection, src: (label, field), dst: (label, field), props }] }`; validation against the collections' schemas.

Tests: `mapping_validates_against_schema`, `type_from_column_maps_many_types`, `sql_create_graph_equals_rpc_mapping`, `linked_graph_refuses_gql_writes` (`FAILED_PRECONDITION`/`graph_is_linked`).

Commit `feat(graph): graph mappings over collections`.

### Task 18: The `collection → graph` link

**Files:** `src/link/collection.rs` (implements `LinkTarget` and a `LinkTargetFactory`), `tests/links.rs`.

**Interfaces produced:** DocOps → change sets per §48 §10.1 (vertex upsert/delete with incident edges; edge upsert/delete; stub vertices with `_stub = true`; `_collection` and `_key` on vertices), written through the same durable path as GQL writes, keyed by the link's offset for exactly-once.

Tests:
- `link_apply_exactly_once_under_kill` (kill the worker at each step; final graph = mapping applied to collections at the link offset).
- `stub_vertex_filled_by_later_upsert`, `vertex_delete_removes_incident_edges`, `pk_upsert_is_entity_merge`, `link_lag_metric_reported`.

Commit `feat(link): collection to graph link (D743)`.

### Task 19: The `stream → graph` link

**Files:** `src/link/stream.rs`, `tests/links.rs`; docs snippet for Flow's `loams_sink` into a graph's source collections.

**Interfaces produced:** `StreamGraphMapping` (field paths → node/edge ops, CloudEvents `type` routing), exactly-once by stream offset.

Tests: `cloudevents_map_to_nodes_and_edges`, `malformed_record_dead_lettered_not_blocking`, `stream_link_exactly_once_under_kill`.

Commit `feat(link): stream to graph link`.

### Task 20: `Search.expand` on Loams Graph

**Files:** `crates/loams-graph/src/retrieval/expand.rs`, `crates/loams-query/src/ir.rs` (add §07 §5.2's `expand`/`rerank` if Task 0 found them missing), the search planner hook, `tests/retrieval.rs`. Waits for Q670.

**Interfaces produced:** `GraphExpander::expand(graph, seeds, Expand, consistency) -> Vec<Neighbor>` built on parameterized GQL templates; §07 §4's deterministic truncation (hop asc, weight desc, canonical key bytes asc) applied in Loams; `Rerank::{Inherit, Vector, Model}` as §07 specifies.

Tests:
- `expand_matches_networkx_reference` (fixture graphs, 1 and 2 hops, directions, edge-type and property filters, `limit_per_seed`, deletes, link-applied updates; Python reference in `conformance/graph/reference/`).
- `truncation_is_deterministic` (same answer across 100 runs and after a reload).
- `expand_respects_consistency_token`, `seed_without_vertex_is_skipped_and_counted`.

Commit `feat(query): search expand stage on Loams Graph`.

### Task 21: SQL `graph_*` table functions

**Files:** `crates/loams-graph/src/retrieval/udtf.rs`, registration in `loams-query`, `tests/retrieval.rs`. Waits for Q670.

**Interfaces produced:** `graph_expand`, `graph_neighbors`, `graph_degree`, `graph_shortest_path` with §07 §5.1's exact signatures and output columns, returning Arrow (workspace arrow 58) batches.

Tests: `udtf_signatures_match_section_07`, `udtf_results_match_reference`, `lateral_join_with_collection` (or the `seeds => 'SELECT …'` fallback if Task 0 found `LATERAL` unsupported), `shortest_path_depth_bound`.

Commit `feat(query): graph table functions on Loams Graph`.

### Task 22: Algorithms

**Files:** `src/retrieval/algos.rs`, jobs via `loams-worker`, `tests/retrieval.rs`.

**Interfaces produced:** `pagerank`, `wcc`, `leiden` (§07 §5.3 signatures) on a snapshot; Grafeo `algos` where available, `graspologic-native` otherwise (licence check first, recorded); write-back job to the vertex source collection (linked) or as properties via a change set (owned).

Tests: `algorithms_deterministic_for_seed_and_snapshot`, `pagerank_matches_networkx_within_1e-6`, `wcc_matches_reference`, `leiden_modularity_within_reference_tolerance`, `write_back_is_one_atomic_job`.

Commit `feat(graph): graph algorithms and write-back`.

### Task 23: The GraphRAG example and harness

**Files:** `examples/graphrag/` (Python: chunk → entities/relations → collections → linked graph → `Search` with `expand` → answer), `bench/graph/graphrag/harness.py` (used by Task 35).

Tests: `example_runs_end_to_end_in_ci` (a small corpus, a fixed fake embedder, deterministic answers), `harness_reports_recall_and_latency`.

Commit `docs(graph): GraphRAG example and harness`.

---

## GR1d — Auth, ops and HA

### Task 24: Authentication and per-graph authorization

> **From Task 4 review (M6):** `CreateGraph` creates its namespace when the namespace does not exist (as `CreateCollection` does). Task 24 decides who may do that: creating a namespace implicitly needs the namespace-create permission, or the RPC must refuse a namespace that does not exist.

> **From Task 3 review (M1):** schema DDL does not run today. Grafeo's `execute_with_params`, which every Loams path uses, answers "Schema DDL commands cannot be executed as queries", so the gate files DDL as Admin and the engine then refuses it. Task 24, with Task 9's `SchemaChange` routing (R0.6 (d)), decides who may run DDL and runs it on Grafeo's non-parameterised path; Task 31 reviews that path.

**Files:** `src/authz.rs`, the OpenFGA model file MT1 owns (add `type graph`, §48 §11.1), RBAC role expansion, `tests/authz.rs`. Depends on MT1's interceptor; until it lands, test against the `Authorizer` trait with the built-in RBAC.

**Interfaces produced:** per-RPC checks per §48 §11.1's table; `Access` from `classify` + engine; Grafeo session role from the caller's strongest relation; agent scopes `graph:read|write|admin`; `ListGraphs` (which requires a namespace, controller ruling M5) through `filter_visible` over that namespace's graphs; `graph_classifier_disagreement` logged and counted.

Tests: `reader_cannot_write_any_corpus_write`, `writer_cannot_ddl`, `writer_cannot_delete_graph`, `admin_can_delete`, `cross_namespace_denied`, `agent_scope_graph_read_blocks_writes`, `protected_environment_requires_project_admin`, `list_filters_invisible_graphs`, `denials_are_audited`.

Commit `feat(graph): per-graph authorization (D750)`.

### Task 25: Quotas

**Files:** `src/quota.rs`, the §41 limits record fields, `tests/limits.rs`.

**Interfaces produced:** per-namespace graphs, elements, stored bytes, concurrent statements, statements/s, import bytes/day; `RESOURCE_EXHAUSTED`/`quota_exceeded` with the quota name; counts kept from change sets, not by scanning.

Tests: `graph_count_quota`, `element_quota_blocks_write_and_link`, `stored_bytes_quota`, `rate_quota_429_retry_after`, `quota_counts_survive_recovery`.

Commit `feat(graph): namespace quotas`.

### Task 26: Memory governance

> **From Task 3 review (M4):** reopening a poisoned graph waits for every holder of its handle to drop it (`graph_reloading` until then, `a_held_poisoned_graph_answers_reloading_until_released`). A long-running statement on another thread can therefore starve the reopen indefinitely. Task 26's watchdog, which detaches runaway statements, must also bound how long a poisoned graph may stay unopened, and fail it (`GraphState::Failed`) past that.

**Files:** `src/memory.rs`, `tests/limits.rs`.

**Interfaces produced:** per-graph `memory_limit` and spill path; node budget with LRU eviction of idle graphs; a watchdog that poisons and reloads a graph whose resident memory exceeds 1.5× its limit for 10 s (only if Task 0 showed Grafeo's limit can be exceeded).

Tests: `runaway_statement_fails_not_process` (a cartesian product on a 50k-node graph with a 64 MiB limit: the statement fails with `graph_memory_limit`, the process survives, another graph answers), `spill_lets_large_sort_complete`, `node_budget_evicts_idle_lru`, `open_refused_when_budget_exhausted`.

Commit `feat(graph): memory limits and node budget`.

### Task 27: Observability and audit

**Files:** `src/metrics.rs`, `src/audit.rs`, tracing spans in handlers and the write path, `tests/observability.rs`.

**Interfaces produced:** §48 §15's metric families (bounded labels), spans with fingerprints, the slow log (threshold config), audit events for admin RPCs, denials, restores and exports; Grafeo's metrics snapshot exported as `loams_graph_engine_*`.

Tests: `metric_families_present`, `labels_bounded_by_graph_quota`, `slow_log_redacts_literals`, `spans_never_contain_parameters`, `audit_event_per_admin_rpc`.

Commit `feat(graph): metrics, traces, slow log and audit`.

### Task 28: HA: standbys, failover and rolling upgrades

**Files:** `src/replica.rs` (standby promotion), `src/owner.rs`, `tests/ha.rs` (multi-node, sim and process level).

**Interfaces produced:** `replicas ≥ 1` keeps a warm follower that takes the lease on owner loss; graceful handover on drain; rolling upgrade order (followers first).

Tests: `failover_within_rto_no_standby` (≤ 30 s for a 1 GiB snapshot, measured), `failover_within_rto_with_standby` (≤ 10 s), `drain_hands_over_without_errors`, `rolling_upgrade_no_acked_write_lost`, `engine_upgrade_rebuilds_from_portable_snapshot`.

Commit `feat(graph): standby failover and rolling upgrades (D753)`.

### Task 29: PITR, export and import

**Files:** `src/ops/{restore.rs,export.rs,import.rs}`, `tests/ops.rs`.

**Interfaces produced:** `RestoreGraph` (into a new graph at a token, epoch or timestamp), `ExportGraph` (portable Parquet to `ns/<ns_id>/exports/graphs/<graph_id>/<ts>/`), `ImportGraph` (portable Parquet, CSV or JSONL with a column mapping, from the namespace's import prefix) as Operations; imports write change sets in batches.

Tests: `restore_to_token_matches_history`, `restore_never_overwrites_live_graph`, `export_import_round_trip`, `import_outside_namespace_prefix_denied`, `csv_import_with_mapping`, `import_respects_quotas`, `restore_drill_script_passes` (`scripts/graph/restore-drill.sh`, run monthly by CI on the reference topology).

Commit `feat(graph): PITR restore, export and import`.

### Task 30: Deployment, dashboards, alerts and runbooks

**Files:** `deploy/helm/…` values for the `graph` role (resources, local-cache volume, PodDisruptionBudget, anti-affinity), `deploy/dashboards/graph.json`, `deploy/alerts/graph.yaml`, `docs/operations/graph-runbook.md`.

Tests: `helm_template_renders_graph_role` (helm unittest or the repo's chart test), `alerts_reference_existing_metrics` (a script cross-checking alert expressions against Task 27's families), `dashboard_queries_valid`.

Commit `feat(deploy): graph role, dashboards, alerts and runbook`.

### Task 31: Security hardening

> **From Task 3 re-review (2c):** Grafeo's GQL parser recurses once per operator-chain link with no limit of its own, and a stack overflow aborts the process. Task 31's `cargo-fuzz` targets include the gate and `translate_full` on adversarial nesting (chains, brackets, `CASE`, subqueries, `NEXT`), run on the production stack size. The upstream ask (Q679) is a depth limit for chains, like the 128 it already applies to brackets.

> **From Task 3 review:** (1) **Id forging via `RETURN n` (security note, from Task 2 N3).** Grafeo's own projection of a bare node or relationship inserts properties after the reserved keys, so a node with a property `_id` or `_labels` is answered on the wire with the forged id or labels. Path elements are safe (resolved by Loams, real fields win). Task 31 either refuses writes of `_`-prefixed reserved property names or rebuilds projected elements from the store. (2) **M1:** see Task 24 on DDL.

**Files:** `src/classify.rs` (procedure/function allowlist), `crates/loams-graph/fuzz/fuzz_targets/{execute.rs,changeset_decode.rs,portable_snapshot.rs,value_from_proto.rs}`, `docs/security/graph-threat-model.md`, `supply-chain/` (cargo-vet entries for the six Grafeo crates), CI job `graph-fuzz` (nightly, 1 h per target; 24 h before GA).

Tests: `procedure_outside_allowlist_denied`, `file_and_network_functions_unavailable`, `unsafe_inventory_recorded` (a script listing `unsafe` blocks per Grafeo crate into `docs/security/grafeo-unsafe.txt`, failing if the count grows without a review note), fuzz targets run clean for the CI budget.

Commit `chore(graph): security hardening, fuzzing and threat model (D755)`.

---

## GR1e — Conformance, performance and SDKs

### Task 32: `gql-1` and its corpus

**Files:** `spec/graph/gql-1.md` (feature ids and section numbers only), `conformance/graph/gql/{features/*.gql,expected/*.json,negative/*.gql}`, runner `crates/loams-graph/tests/gql_corpus.rs`.

Tests: `gql1_declared_features_pass` (100 %), `undeclared_features_answer_unsupported`, `engine_info_reports_gql_1_0`, `grafeo_deviation_cases_pinned` (each known deviation has a test asserting Loams' declared behaviour).

Commit `feat(conformance): gql-1 profile and corpus (D747)`.

### Task 33: The Cypher dialect and the openCypher TCK subset

Waits for Q673. If the owner says GQL only, this task records that and only adds the `graph_language_disabled` tests.

**Files:** `crates/loams-graph/Cargo.toml` (`cypher` feature behind `graph-cypher`, on in the `graph` build if Q673 says so), `conformance/graph/tck/{selection.txt,allowlist.toml}`, runner (Rust Cucumber or a Python runner over the service; Task 0 picks), CI job.

Tests: `tck_subset_pass_rate_at_least_90`, `every_tck_failure_allowlisted_with_reason`, `cypher_disabled_graph_refuses`, `bolt_not_listening` (no Bolt port in any config).

Commit `feat(conformance): openCypher compatibility subset (D748)`.

### Task 34: LDBC SNB-derived performance harness and gates

**Files:** `bench/graph/ldbc/{README.md,datagen.sh,load.py,queries/*.gql,run.py,gates.toml}`, CI job `graph-bench` (nightly, SF0.1 in CI; SF1 on the reference topology weekly), results into `bench/results/graph/`.

Steps: measure the baseline first on the reference topology and record it as a ruling; compare with §48 §17.2; escalate any target missed by more than 2× to the owner before tuning.

Tests (gates in `gates.toml`, checked by `run.py`): IS1–IS7 p50/p99, IC1/IC2/IC7/IC8/IC13 p99, 1- and 2-hop expand p99, durable write p99 vs WAL ack p99, write throughput, link ingest, SF1 import time, open-from-snapshot time, SF1 resident memory, binary size delta. `results_never_labelled_ldbc_benchmark` (a lint over the output and docs).

Commit `bench(graph): LDBC SNB-derived harness and gates (D757)`.

### Task 35: GraphRAG benchmark

**Files:** `bench/graph/graphrag/` (uses Task 23's harness; dataset per §07 §9, licence recorded), nightly job.

Tests: `graphrag_p95_hot_within_target`, `graphrag_p95_cold_within_target`, `recall_not_below_naive_reference`, `s3_gets_per_query_tracked`.

Commit `bench(graph): GraphRAG retrieval benchmark`.

### Task 36: TypeScript and Python SDKs

**Files:** per SDK1 Task 0's layout: the generated `loams.graph.v1` clients and the hand-written `loams.graph` facade in `sdks/typescript` and `sdks/python`; examples; SDK1 runtime fixtures for graph.

**Interfaces produced:** §48 §18.1's facade: `graph.query`, `graph.stream`, `graph.transaction`, `graphs.create/get/list/delete`, typed `Node`/`Relationship`/`Path`, `bigint`/`int` for `INT64`, automatic idempotency keys on writes, retry on `ABORTED` and `UNAVAILABLE/graph_reloading` with backoff.

Tests: SDK1 conformance fixtures for every graph RPC in both languages; `ts_int64_is_bigint`, `py_int64_is_int`, `stream_iterates_all_chunks`, `transaction_retries_on_conflict_same_key`, `query_never_interpolates` (the facade has no string-format path).

Commit `feat(sdk): loams.graph in TypeScript and Python (D758)`.

### Task 37: LangChain, LlamaIndex and LightRAG adapters

**Files:** `sdks/python/…/graph_stores/{langchain.py,llama_index.py,lightrag.py}`, extras in the Python package, `conformance/graph/frameworks/` (pinned framework versions, test runners), CI job.

**Interfaces produced:** `LoamsGraph(GraphStore)`, `LoamsPropertyGraphStore(PropertyGraphStore)`, `LoamsGraphStorage(BaseGraphStorage)` per §48 §17.1; `structured_query` accepts GQL, and Cypher when the graph enables it.

Tests: each framework's own graph-store tests at the pinned version (Task 0 of this task lists which exist and records the selection), plus `adapter_round_trip_*` Loams-written tests; `vector_query_uses_collections_not_grafeo`.

Commit `feat(sdk): graph store adapters for LangChain, LlamaIndex and LightRAG`.

### Task 38: Docs

**Files:** the docs pages §48 §20 lists (in this repository's docs tree or `loams-cloud` per the docs owner's ruling; Task 0 records which), the limits page entries, the API reference generated from the protos.

Tests: `docs_examples_execute` (every GQL and SDK snippet runs against `loams dev` in CI), `limits_page_matches_defaults` (a script comparing the page with `StatementLimits`), link check.

Commit `docs(graph): Loams Graph documentation`.

### Task 39: The production exit

**Files:** `docs/plans/gr1-exit-report.md`, `crates/loams/src/api/connect.rs` (`unstable: false`), `buf.yaml` (breaking checks on), variant lists (Q675's answer), `CHANGELOG.md`.

Steps: run every gate; fill the checklist below with evidence links; open the external security review (§48 §16) and record its findings and their status; ask the owner to accept any waived target; flip `unstable`; tag.

Tests: `buf_breaking_enforced_for_graph`, `catalogue_graph_stable`, the full CI matrix green.

Commit `docs(gr1): production exit report`.

---

## Exit checklist for production (§48 §21, with owning tasks)

| Item | Tasks |
|---|---|
| `loams.graph.v1` served by `loams` with `graph`, in the catalogue, stable, `buf breaking` on | 2, 5, 39 |
| I1–I6: durability tests, recovery differential and fault matrix in CI | 9–14, 16 |
| Fenced ownership; failover within RTO with and without a standby | 12, 28 |
| D76 token conformance on the graph stream | 15 |
| Linked graphs exactly once; `Search.expand` and the four UDTFs pass the differential | 17–21 |
| Unified auth and per-graph authz; non-loopback refusal without an authorizer; classifier disagreements 0 on the corpus | 3, 5, 24 |
| Limits and quotas with named reasons; a statement fails, never the process | 6, 25, 26 |
| Snapshots, PITR, export and import; the first restore drill passed | 13, 29 |
| Metrics, traces, slow log, audit, dashboards and alerts; redaction tested | 6, 27, 30 |
| Security controls tested; fuzzing 24 h clean; external review closed or accepted | 31, 39 |
| `gql-1` 100 %; openCypher subset ≥ 90 % with allowlist; framework suites pass | 32, 33, 37 |
| §48 §17.2 targets met or waived by the owner | 34, 35, 39 |
| TypeScript and Python SDKs and the three adapters released | 36, 37 |
| Desktop Graph page against `loams dev` and a remote server | 7, 8 |
| Licences and NOTICE; Grafeo pin ≥ 14 days old | 1, 39 |
| Docs published | 38 |

## Rulings made during execution

### Task 0 (2026-10-08, on `dev` at `6079e4d7`)

How these were measured: a scratch crate in the worktree (`scratch/probe`, standalone `[workspace]`, never committed) built with the shared target and `-j 4`, its tests run one at a time under `systemd-run --user --scope -p MemoryMax=3G`; three more scratch manifests (`scratch/feat-{narrow,strict,cypher}`) for `cargo tree --offline` only. Grafeo sources were read from `~/.cargo/registry/src/index.crates.io-*/grafeo*-0.5.43/`. Quoted lines starting `P<n>` are probe output. Work happened in `~/Documents/Ostriumlabs/loams-wt/gr1` on branch `backend/gr1`, not the `gr1-graph-production` worktree or `feat/gr1-graph-production` branch the Global Constraints name (R0.0).

**R0.0 Worktree and branch.** GR1 runs in `~/Documents/Ostriumlabs/loams-wt/gr1` on `backend/gr1` (the orchestrator's choice). Read the Global Constraints' path and branch as these.

**R0.1 As built: all twelve of §48 §5's findings are still true on `dev`.**
1. Still true. `fabric/crates/` holds `loams-chdb`, `loams-chdb-sys`, `loams-flow`, `loams-flow-proto`, `loams-graph`, `loams-graph-proto` and `loams-house`, with no `loams-fabric`. `service.rs` is still free functions. `CATALOGUE` in `crates/loams/src/api/connect.rs` has three rows (`instance`, `collection`, `live`) and none for graph.
2. Still true. `string database_path = 3;` is in `fabric/proto/loams/graph/v1/graph.proto:104`, and `service::open` passes it to `OpenSpec`.
3. Still true. `ExecuteRequest` has fields 1–5 (`namespace`, `name`, `statement`, `language`, `read_only`) and no parameters.
4. Still true. `service.rs` `execute_batch` with `atomic = false` calls `graph.execute(&statement.text, false)`.
5. Still true. Only `req.language` is passed to `check_language`.
6. Still true. `to_pb_value` uses `n.as_f64()` and `to_gql_value` uses `Value::from(*number)` (f64).
7. Still true. `rows_affected` and `bytes_read` are `None`, so the wire gets 0 (`engine.rs:131-153`).
8. Still true. The guard is still `writes`/`bare_words` (`engine.rs:485,513`). Nothing calls `session_with_role`.
9. Still true. `GrafeoDB::open(path)` uses the default `Config`. P11 prints `wal_durability=Batch { max_delay_ms: 100, max_records: 1000 }`.
10. Still true. `Engine.graphs` is a `Mutex<HashMap<String, Arc<Graph>>>` (`engine.rs:386`), and `list_graphs` reads it.
11. Still true. Fabric depends on `grafeo = { version = "0.5", features = ["wal","spill","mmap"] }` with defaults (`embedded` = `ai` + `arrow-export`). `fabric/Cargo.lock` has `grafeo-engine 0.5.43 -> arrow-array 60.0.0, arrow-ipc 60.0.0, arrow-schema 60.0.0`.
12. Still true. `connectors/licences.toml` has `[components.grafeo|grafeo-core|grafeo-engine|grafeo-adapters|grafeo-common]` and no `grafeo-storage`, but `fabric/Cargo.lock` links `grafeo-storage 0.5.43`.

**R0.2 Grafeo features (changes Task 1).** The narrowest set that keeps LPG, GQL, WAL, CDC, spill, algos, metrics and tracing:
```toml
grafeo        = { version = "=0.5.43", default-features = false, features = ["gql", "wal", "grafeo-file", "spill", "cdc", "algos", "metrics", "tracing", "parallel", "regex"] }
grafeo-engine = { version = "=0.5.43", default-features = false, features = ["lpg"] }   # named directly: grafeo's own `lpg` turns on cypher, gremlin and sql-pgq
grafeo-common = { version = "=0.5.43", default-features = false }
```
- `grafeo-file` is required: `backup_full` and `backup_incremental` are `#[cfg(all(feature = "wal", feature = "grafeo-file", feature = "lpg"))]` (`grafeo-engine/src/database/mod.rs:2742,2767`).
- `mmap` is not needed. Outside `compact-store` and `vector-index` it gates nothing, and spill does not need it (`grafeo-core` `spill = []`).
- `parallel` (rayon) and `regex` (GQL `=~`, matching fabric's choice against `regex-lite`) are kept on purpose. Without them, `scratch/feat-strict` has 57 packages instead of 65.
- The probe crate compiles and runs with this set.
- `cargo tree -e features -i grafeo-engine` (narrow) shows only `algos, cdc, crossbeam, gql, grafeo-file, grafeo-storage, lpg, metrics, parallel, rayon, regex, spill, tracing, wal`. The grafeo-side features are exactly the ten listed. `grafeo-adapters` gets `algos, gql, parallel, rayon, tracing`.
- `cargo tree --offline -e features -i grafeo-engine`, run in `scratch/feat-narrow`, trimmed to grafeo-engine's own features:
  ```
  ├── grafeo-engine feature "algos"
  │   └── grafeo feature "algos" (*)
  ├── grafeo-engine feature "cdc"
  │   └── grafeo feature "cdc" (*)
  ├── grafeo-engine feature "crossbeam"
  │   └── grafeo-engine feature "parallel"
  │       └── grafeo feature "parallel" (*)
  ├── grafeo-engine feature "gql"
  │   └── grafeo feature "gql" (*)
  ├── grafeo-engine feature "grafeo-file"
  │   └── grafeo feature "grafeo-file" (*)
  ├── grafeo-engine feature "grafeo-storage"
  │   ├── grafeo-engine feature "grafeo-file" (*)
  │   └── grafeo-engine feature "wal"
  │       └── grafeo feature "wal" (*)
  ├── grafeo-engine feature "lpg"
  │   └── feat-narrow v0.0.0 (scratch/feat-narrow) (*)
  ├── grafeo-engine feature "metrics"
  │   └── grafeo feature "metrics" (*)
  ├── grafeo-engine feature "parallel" (*)
  ├── grafeo-engine feature "rayon"
  │   └── grafeo-engine feature "parallel" (*)
  ├── grafeo-engine feature "regex"
  │   └── grafeo feature "regex" (*)
  ├── grafeo-engine feature "spill"
  │   └── grafeo feature "spill" (*)
  ├── grafeo-engine feature "tracing"
  │   └── grafeo feature "tracing" (*)
  └── grafeo-engine feature "wal" (*)
  ```
- **arrow-array 60 is not in the tree.** The narrow tree has no `arrow*` package at all.
- The tree's grafeo-specific packages are the six grafeo crates plus `arcstr bincode bumpalo byteorder bytes crc32fast crossbeam dashmap foldhash fs2 hashbrown(0.14, 0.17) indexmap memmap2 parking_lot rayon regex smallvec thiserror tokio unicode-normalization`. `tokio` (with `macros`) comes in through `grafeo-storage`'s `wal`, which cannot be turned off.
- **With `cypher` added** (`scratch/feat-cypher`), the tree gains only the features `grafeo/cypher -> grafeo-engine/cypher -> grafeo-adapters/cypher` and **no package** (159 tree lines either way, no arrow). Task 33's `graph-cypher` feature therefore costs code size only.
- The features `jemalloc` and `mimalloc-allocator` install a `#[global_allocator]` in `grafeo/src/lib.rs:61,65`. They must never be enabled. Task 1 adds a comment saying so.

**R0.3 Pin: 0.5.43 is not yet 14 days old (changes Task 1 and D759; owner decision Q-T0-1).** From crates.io (`/api/v1/crates/<c>/versions`, read 2026-10-08), all six crates were published on the same dates: 0.5.44 on 2026-10-04, 0.5.43 on 2026-09-27, and 0.5.42 on 2026-05-04.
- The newest release at least 14 days old today (published on or before 2026-09-24) is **0.5.42**.
- 0.5.43 becomes eligible on **2026-10-11**, and 0.5.44 on 2026-10-18.
- D759's premise ("0.5.43 exact"; only 0.5.44 is too new) is wrong by three days. Fabric adopted 0.5.43 on 2026-10-04, when it was 7 days old.
- Ruling: every probe here ran against 0.5.43, so the plan keeps 0.5.43. Task 1's commit that adds the pin to the root workspace must not merge before 2026-10-11, unless the owner says otherwise. Do not re-probe on 0.5.42.

**R0.4 `Session::set_viewing_epoch` isolates a session.** The pin holds across statements: `P1 pinned=Int64(1) pinned_rows_2nd_stmt=1 execute_at_epoch=Int64(1) latest=Int64(2) pin=EpochId(1) now=EpochId(2)`. `execute_at_epoch` agrees. GC at a pinned epoch is left to Task 9 step 4.

**R0.5 `prepare_commit().commit()` does not reliably return the transaction's own commit epoch (changes Tasks 9 and 11; upstream Q679).**
- `PreparedCommit::commit` is `self.session.commit()?; Ok(self.session.transaction_manager().current_epoch())` (`grafeo-engine/src/transaction/prepared.rs:124-128`). It reads the global epoch *after* committing and drops the epoch that `TransactionManager::commit` returns (`transaction/manager.rs:264`).
- Sequential commits get the right value: `P2 commit_epoch=EpochId(1) current=EpochId(1)`.
- With 4 threads and a barrier, 80 transactions returned only 70 distinct epochs, and 10 were later than the transaction's real epoch: `P2c ... distinct_returned_epochs=70 final_epoch=EpochId(80) event_epoch_matches=70 mismatches=[(0,0,3,[1]), (0,2,11,[10]), …]`.
- The CDC event epoch *is* the real commit epoch: each transaction's events sit at exactly one epoch.
- **A committed explicit read-only transaction also advances the global epoch.** Autocommit reads and rollbacks do not: `P2d after write=EpochId(1) autocommit-read=EpochId(1) ro-txn-commit=EpochId(2) rollback=EpochId(2) ro-session-read=EpochId(2)`. Epochs therefore have gaps with no change set.
- `PreparedCommit::info()` reports `nodes_written: 0, edges_written: 0` until commit, as upstream documents.
- Ruling: Task 11's `WriteLane` holds one per-graph commit mutex around `prepare_commit()` … `commit()`. Every *explicit transaction* on a graph's engine commits through the lane, read-only ones included: GQL batches, link targets, imports and algorithm write-back. Client reads run as autocommit statements on a viewing-epoch session and never `begin_transaction`. Under those two rules, `current_epoch()` read under the mutex is the transaction's epoch.
- Task 11's durable-epoch tracking must accept epochs that have no record.
- Task 9's spike adds `commit_epoch_is_exact_under_lane` (concurrent submitters through the lane; the returned epoch equals the CDC epoch). Upstream ask: return the manager's epoch.

**R0.6 CDC (changes Tasks 9 and 10).** Events for a committed transaction can be retrieved by epoch (`changes_between(e, e)`), and a rollback records none. What they contain:
- **Node create**: two events, `Create labels=Some(["Person"]) after=None`, then `Update after=Some({props})`, both at the commit epoch.
- **Edge create**: `Create et=Some("KNOWS") src=Some(0) dst=Some(1)`, then `Update after=Some({"w": Int64(1)})`. **Edge endpoints are present on create.**
- **Label add**: `Update labels=Some(["Person","Admin"]) before=None after=None`.
- **Label remove**: `Update labels=Some(["Person","Admin"])`. This is the *same shape*, so an event does not say whether a label was added or removed, or what the resulting set is.
- **Property remove**: `Update before={"name": String("b")} after={"name": Null}`.
- **Delete** (`DETACH DELETE`): the node `Delete labels=Some(["Person"]) before={props}`, and the edge `Delete et=None src=None dst=None before={props}`. **Edge type and endpoints are missing on delete.**
- **DDL** (`CREATE NODE TYPE`, `CREATE INDEX`, `CREATE GRAPH`, `DROP NODE TYPE`) produces **no CDC event and does not advance the epoch** (`cdc events 12->12; epoch EpochId(5)->EpochId(5)`).
- `session_with_cdc(false)` writes nothing to CDC. A plain `session()` on a `Config::with_cdc()` database does.
- The CDC log is in memory and per database, with default retention `max_epochs: Some(1000), max_events: Some(100_000)` (`cdc.rs:259-260`). `changes_between` scans every entity's history, so it costs O(events retained) (`cdc.rs:534`).

Rulings:
- (a) Task 9's `CdcCapture::take` resolves each touched element's final labels (and, for an edge, type and endpoints) from the store at the commit epoch, inside the lane, rather than from the events. A deleted edge's type and endpoints are taken from a pre-commit read or from Loams' own element map.
- (b) Capture runs before CDC retention can prune: it takes and then prunes per commit through `CdcLog::prune_before`, and retention is set high.
- (c) Sessions are always created with CDC on. `session_with_cdc(false)` is never used.
- (d) DDL is routed to `SchemaChange` by classification (R0.10), never through CDC. Task 9's `ddl_capture_or_schema_record` takes the schema-record branch.

**R0.7 Caller-chosen ids: only below GQL (affects Tasks 9 and 14).**
- `LpgStore::create_node_with_id(NodeId, &[&str])` and `create_edge_with_id(EdgeId, src, dst, type)` exist, documented as "used for WAL recovery" (`grafeo-core/src/graph/lpg/store/versioning.rs:439,525`).
- They work (`P4 ... true true; create_edge_with_id: "Ok(())"`) and GQL sees the ids (`[[Int64(1000)], [Int64(1001)]]`, edge `500`).
- They **bypass MVCC and CDC**: they record no events and no epoch.
- The allocator moves past them: the next GQL insert gets 1002, then 1003.
- GQL has no way to choose an id.
- Ruling: replay (Task 9 step 3, Task 14 recovery, followers in Task 15) may build a fresh, not-yet-serving engine with these calls, plus a direct store property API, which Task 9 confirms. On a live engine, writes go through GQL, and the `_lid` map stays the fallback.

**R0.8 `query_timeout` does not bound a running statement and is not per session (changes Tasks 6 and 26; upstream Q679).**
- The timeout is a database `Config` field, copied into each session's private `SessionConfig.query_timeout` when the session is created (`database/mod.rs:1773`). There is no public per-session setter and no cancel handle.
- The deadline is checked only between pipeline chunks (`query/executor/mod.rs:87-111,205,350`).
- Measured with a 300 ms timeout:
  - a 300³ cartesian `count(*)` answered `timeout error` after **8.04 s**;
  - a 300⁴ cartesian did not end within the 120 s harness budget;
  - `MATCH p = (a:T {i:1})-[:N*]->(b) RETURN count(p)` on a 40-node `+1/+2` chain (about 10⁸ paths) **did not end within 120 s**;
  - with a 5 s timeout, a cartesian `ORDER BY` returned the timeout error after **39.3 s**.
- `SESSION SET PARAMETER $timeout` has no effect.

Rulings:
- (a) Task 6's deadline is enforced on the Loams side. The statement runs on a blocking thread. At the deadline the RPC answers `graph_statement_timeout`, and the thread is detached and counted (`loams_graph_detached_statements`). Per graph, at most `max_detached` (default 2) can exist; past that the graph's semaphore refuses new statements. The engine's own `query_timeout` is set to the largest allowed per-request timeout as a backstop.
- (b) `classify` refuses an unbounded variable-length quantifier (`*`, `+`, or `{n,}` with no upper bound) and quantifiers above `StatementLimits.max_path_hops` (default 10) with `INVALID_ARGUMENT`/`graph_unbounded_path`. That is refusal, not rewriting, so it is allowed under D634.
- (c) `client_cancel_stops_statement` becomes `client_cancel_answers_and_detaches`.
- (d) `timeout_returns_statement_timeout` keeps "answer within 1 s".
- Upstream asks: a per-session timeout, a cancel token, and deadline checks inside expand, cartesian-product and sort.

**R0.9 `memory_limit` does not fail a statement (changes Task 26; owner question Q-T0-2).**
- `memory_limit` is the budget of the `BufferManager`, a pressure signal for eviction and spill. The thresholds are 70/85/95 %, and "Critical (block allocations)" (`grafeo-common/src/memory/buffer/mod.rs:18-23`). No path returns a memory error to a statement.
- Probe: `with_memory_limit(8 MiB)`, 1 500 nodes with 2 KB strings, then a cartesian `ORDER BY`.
  - The statement ended only by **timeout** (`after 39.27s`). There was no memory error.
  - **Peak RSS was 3 131 564 kB** (`VmHWM`, pinned at the 3 GiB cgroup cap).
  - `memory_usage().total_bytes` said 1 252 501 for at least 3 MB of payload, so it under-counts.
  - The process survived and later queries answered.
- With no `memory_limit`, the budget defaults to 75 % of system RAM per database (`database/mod.rs:360`).

Rulings:
- (a) Loams always sets `memory_limit` and `spill_path` per graph (under `<data_dir>/graphs/<id>/spill`).
- (b) Task 26's watchdog is **required**. The plan's "only if Task 0 showed Grafeo's limit can be exceeded" is answered yes. Because R0.8 shows a statement cannot be stopped, the watchdog's action on a graph over budget is to poison and reload it (fresh `GrafeoDB`), not just evict it.
- (c) Grafeo shares the process heap, so `runaway_statement_fails_not_process` cannot be guaranteed in process. See Q-T0-2.

**R0.10 Roles and engine classification (changes Tasks 3 and 24).**
- `session_with_role(Role::ReadOnly)` refused every parseable write in the probe corpus, and nothing changed: `P7 ro writes that succeeded and changed state: []`. The corpus covered `INSERT`, `SET` property and label, `REMOVE`, `DELETE`, `DETACH DELETE`, comment-hidden `SET`, a string literal `'RETURN'`, `FILTER … SET`, `MERGE`, `NEXT`-chained writes in both orders, `UNWIND/FOR … INSERT`, `CREATE (:Cy)`, `CALL { INSERT }`, `CREATE/DROP NODE TYPE`, `CREATE INDEX`, and `CREATE/DROP GRAPH`.
- Inside a transaction begun on a read-only session, the write is refused too.
- `Role::ReadWrite` is refused DDL (Admin) but **may `CREATE GRAPH`/`DROP GRAPH`**. These are named graphs inside the database, gated at `StatementKind::Write` (`session/mod.rs:803-810`).
- `CREATE PROCEDURE` needs Admin.
- Grafeo 0.5.43 has no public "classify this statement" function. `StatementKind` is used only inside `Session`. But `grafeo_engine::query::translators::gql::translate_full(&str) -> GqlTranslationResult::{Plan, SessionCommand, SchemaCommand}` plus `LogicalPlan.root.has_mutations()` is public, and it agreed with the role check on every corpus statement.
- `//` line comments are a syntax error in 0.5.43's GQL parser, so the Task 3 fixture lists them as parse errors.

Rulings:
- (a) Task 3's `engine_classify` is `translate_full`: a `Plan` with mutations is Write, a `Plan` without mutations is Read, `SchemaCommand` is Admin, and `SessionCommand` is mapped per command.
- (b) `CREATE GRAPH`, `DROP GRAPH`, `USE GRAPH`/`SESSION SET GRAPH` and `CREATE/DROP PROJECTION` are refused outright with `FAILED_PRECONDITION`/`graph_statement_not_allowed`. One Loams graph is one `GrafeoDB`'s default graph.
- (c) Task 24's "writer cannot DDL" holds through the engine role.

**R0.11 `LOAD DATA` reads any file on the server, read-only role included (changes Tasks 3, 5 and 31; Review Focus 4).**
- GQL `LOAD DATA FROM '<path>' FORMAT CSV|JSONL|PARQUET [WITH HEADERS] AS v` opens a server path with `std::fs::File::open` (`grafeo-core/src/execution/operators/load_data.rs:82,212`). It is not feature-gated for CSV and JSONL, and Parquet needs `parquet-import`, which is off.
- It classifies as Read in both the engine role check and `translate_full`.
- Probe: a `ReadOnly` session read a probe file (`Ok([[String("hunter2")]])`) and `/etc/hostname` (`Ok(1)`).
- A `file://` prefix and `http://` URLs fail as "No such file", so there is no network access.
- Ruling: Task 3's guard refuses any statement whose `translate_full` plan contains `LogicalOperator::LoadData`, and also the keyword pair `LOAD DATA` as a backstop. It uses `PERMISSION_DENIED`/`graph_statement_not_allowed`, before the engine runs anything. This is a **precondition for Task 5** (nothing is served over the network before it lands). Task 31's allowlist keeps it refused, and the classify corpus gets the `LOAD DATA` cases.

**R0.12 `backup_full` on a checkpointed database works (Task 13).**
- `wal_checkpoint()` → `Ok`. `backup_full` itself checkpoints the database (`checkpoint_to_file`) and writes `backup_full_0000.grafeo` (`start_epoch 0, end_epoch 1`).
- `backup_incremental` writes `backup_incr_0001.wal`.
- `backup_manifest.json` is kept.
- `GrafeoDB::restore_to_epoch(dir, e, out)` restored both epochs correctly (`A-count` 1 at e1, 2 at e2) and refuses to overwrite an existing output.
- An in-memory database refuses with `backup requires a persistent database`. Graphs are therefore always opened persistent under `<data_dir>/graphs/<id>/`, and the native snapshot is `backup_full` into a local staging directory and then uploaded.

**R0.13 No Grafeo crate forces `panic = "abort"`.**
- None of the six `Cargo.toml` files has a `[profile]` (and dependency profiles are ignored anyway).
- No `std::process::abort`, `process::exit` or `panic::set_hook` appears in their sources.
- `P9` (catch_unwind around a panic inside an open Grafeo transaction): `panic strategy unwind=true caught=true after-panic count=Int64(0)`, then `db usable after panic: Int64(1)`. The uncommitted insert stayed invisible, and the database kept working.
- `parking_lot` locks do not poison. A caught panic therefore leaves Grafeo's locks usable, but its in-memory state is whatever the panicking call left. Task 3 keeps "a panic poisons the graph and it reopens".

**R0.14 Shared state: no process-global cache holds data.** Every `static`, `OnceLock`, `LazyLock` and `thread_local!` in the six crates at 0.5.43:
- `grafeo/src/lib.rs:61,65`: `GLOBAL` allocator, only with `jemalloc`/`mimalloc-allocator`, which are off.
- `grafeo-adapters/src/plugins/algorithms/*.rs`: 25 `*_PARAMS: OnceLock<Vec<ParameterDef>>` (static parameter descriptions).
- `grafeo-engine/src/query/planner/lpg/mutation.rs:700`: `PROCEDURES: OnceLock<BuiltinProcedures>` (the code registry).
- `grafeo-common/src/utils/hash.rs:27`: `HASH_STATE: OnceLock<foldhash::fast::RandomState>` (process hash seed).
- `grafeo-engine/src/query/translators/gql/mod.rs:2104`: `COUNTER: AtomicU32` (anonymous-variable names).
- Under RDF and SPARQL, which are not compiled: `grafeo-core/src/graph/rdf/turtle/parser.rs:16`, `grafeo-engine/src/query/planner/rdf/mod.rs:5127-5216`, `query/translators/sparql.rs:20`.
- Under `testing-*`, off: `grafeo-common/src/testing/{crash,statement_failure}.rs` thread-locals.
- `grafeo-core/src/execution/operators/filter.rs:240`: per-operator `Arc<OnceLock<Value>>`, which is not static.
- The plan cache (`Arc<QueryCache>`), the CDC log, the catalog, the buffer manager and the transaction manager are all fields of `GrafeoDB`, so they are per graph.
- Per-graph isolation holds. Thread pools are shared (rayon's global pool).

**R0.15 Procedures and functions (Task 31).**
- `CALL grafeo.procedures()` lists 26 built-ins: `articulation_points, bellman_ford, betweenness_centrality, bfs, bridges, closeness_centrality, clustering_coefficient, connected_components, degree_centrality, dfs, dijkstra, floyd_warshall, kcore, kruskal, label_propagation, labels, louvain, max_flow, min_cost_max_flow, pagerank, prim, propertyKeys, relationshipTypes, sssp, strongly_connected_components, topological_sort`. All are `grafeo.`-prefixed, with `db.*` aliases for the three catalogue ones.
- The `grafeo.search.*` procedures are absent without `vector-index`/`text-index`.
- User procedures (`CREATE PROCEDURE`) need Admin.
- **No procedure or function touches the filesystem or network.** The only `std::fs` reachable from a statement is `LOAD DATA` (R0.11). The other files that use `std::fs` are spill (under the server's `spill_path`), backup, import (Rust API only), compact-tiered/section (off) and the buffer manager.
- No file in the six crates uses `std::net`, `TcpStream` or `std::process::Command`.
- The algorithms run over the whole graph with no deadline checks, so Task 31 gives `CALL` its own statement-limit class.
- Baseline counts of lines containing `unsafe ` for Task 31's inventory: grafeo 0, grafeo-core 42, grafeo-engine 6, grafeo-adapters 0, grafeo-common 22, grafeo-storage 3.

**R0.16 The engine side of the API (changes Tasks 2, 5 and 12).**
- `crates/loams-proto/build.rs` compiles a fixed `FILES` list (options, errors, instance, collection ×3) with `connectrpc_build::Config::new().files(..).includes(&[proto/]).include_file("_connectrpc.rs").emit_descriptor_set("loams_api_descriptor.bin").gate_client_feature(true)`. `lib.rs` `include_generated!`s them and exports `FILE_DESCRIPTOR_SET`. Task 2 appends `"loams/graph/v1/graph.proto"` to `FILES`. Nothing else is needed.
- A catalogue row is a `Package { package, services, available, unstable }` in `CATALOGUE` (`connect.rs:96`). The handler is registered in `connect::routes()` (`rpc = …::register(rpc, state)`), and every path becomes an axum `route_service`.
- An absent package follows the `LiveAbsent` pattern: every method returns `not_in_variant("<svc>/<Method>")` (`connect.rs:242ff`, `not_in_variant` at about `:318`).
- API1's last tasks (sql, link, admin, auth, internal) have **not** landed. `loams-proto` has none of them.
- `VARIANT` is computed from cargo features (`connect.rs:55`, `cfg!(any(feature = "tikv", "mysql-wire", "stream-grpc"))`), and `release/` holds only `aur`, `nfpm.yaml` and `systemd`, with no variant lists. Task 5 therefore adds `feature = "graph"` to `VARIANT`'s `full` list rather than editing release files.
- **No auth interceptor or `Authorizer` exists** (no `trait Authorizer`, `AllowAll` or interceptor anywhere in `crates/`). MT1 is not in code. Task 5's `non_loopback_listen_without_authorizer_refused` therefore refuses every non-loopback `graph` listen until MT1 lands.
- **`Roles` is not in `server.rs`.** It is `loams_hot::Roles` (`crates/loams-hot/src/registry.rs:27`): `{meta, log, query, worker, gateway}` booleans, a closed `parse` that errors with `unknown role`, `Display` as a comma list, and encoding into the node lease descriptor `v1;<incarnation>;<addr>;<roles>;<zone>`. It is parsed from `--roles` in `crates/loams/src/main.rs:847`.
- Task 5 changes `crates/loams-hot/src/registry.rs`, not `server.rs`. A node from before `graph` existed fails to decode a peer descriptor that lists `graph`, so Task 28's rolling upgrade ships the parser change one release before any node advertises `graph`. Alternatively, Task 5 makes `decode` ignore unknown roles. Task 5 picks this and records it.
- **`loams.internal.v1` is not served.** Node-to-node calls are axum REST under `/internal/v1/...` (`crates/loams/src/api/internal.rs`), on the main port and on `internal_router` (`api/mod.rs:267`). The move to `loams.internal.v1` is API1 Task 8 (WIP `c89a362a` did not do it).
- Task 12's `GraphForward` goes in `loams.internal.v1` only if API1 Task 8 has landed by then. Otherwise it is an `/internal/v1/graph/...` route on `internal::routes()`, with a note to move it. Task 12 records which.

**R0.17 Link framework (changes Tasks 10 and 18).**
- `#[async_trait] trait LinkTarget: Send + Sync { async fn load(&self) -> Result<TargetState, LinkError>; async fn commit(&self, expected_version: u64, batch: ApplyBatch, fence: &Fence) -> Result<u64, CommitError>; }`, where `TargetState { version: u64, applied: BTreeMap<u32, u64> }`, `ApplyBatch { records: Vec<(u32, OffsetRecord)>, applied_after: BTreeMap<u32, u64> }`, and `CommitError::{Conflict, Fenced, Other}` (`crates/loams-link/src/target.rs`).
- `trait LinkTargetFactory: Send + Sync + Debug { fn kind(&self) -> &str; fn open(&self, meta: &Arc<dyn MetaStore>, link: &Link) -> Result<Arc<dyn LinkTarget>, LinkError>; async fn retain(&self, links: &BTreeSet<LinkId>) {} }`, registered in `TargetRegistry::with` (`registry.rs`).
- The exactly-once contract (`lib.rs`): each commit carries the batch's data and the offsets it applied, atomically, under the target's version (optimistic concurrency) and the task lease's fence. A crash re-reads the committed offsets. A zombie's commit fails.
- Ruling: a graph link target's `commit` is one `GraphChangeSet` through the write lane. Its `load` must recover `version` and `applied` from the graph's durable state. Task 10's v1 record therefore carries an optional `link: Option<LinkApply { link: LinkId, version: u64, applied_after: BTreeMap<u32, u64> }>` from the start, so it is not a v2. The graph keeps the latest `LinkApply` per link in its manifest and snapshot. `expected_version` is checked under the lane mutex, and the fence is checked by the append (R0.18).

**R0.18 The log has no fencing, but it has a CRC (changes Tasks 10 and 12).**
- `MetaStore::commit_wal(WalCommit { object, created_at_ms, chunks: Vec<WalChunk { stream, partition, records, byte_range, max_timestamp_ms }> })` has **no fence or epoch** (`crates/loams-common/src/meta/`).
- Leases (`acquire_lease`, which returns an epoch-bearing `LeaseGrant`) and `cas_pointer(PointerCas { …, fence: Option<Fence>, … })` are fenced. Appends are not.
- Task 12's "the log refuses a stale epoch" therefore needs a metastore change: an optional `fence` on `WalCommit`, checked atomically with the offset assignment, in `loams-meta` and `loams-meta-tikv`, with a `loams-meta-conformance` case. That change is now part of Task 12 and needs the log owner's review.
- Log batches already carry a crc32c over the batch (`crates/loams-log/src/batch.rs:9-11,129`), so **Task 10 adds no per-record CRC.**

**R0.19 Search IR (changes Tasks 20 and 21).**
- `crates/loams-query/src/ir.rs`'s `SearchRequest` has **no `expand` or `rerank`**.
- `proto/loams/collection/v1/query.proto:230-232` declares `google.protobuf.Struct rerank = 17; google.protobuf.Struct expand = 18;` as "always `invalid_argument`", in `loams.collection.v1`, which is `unstable: false`.
- Changing those fields' type to typed messages is a `buf breaking` FIELD_SAME_TYPE violation in a stable package. Task 20 therefore keeps 17/18 as they are, deprecated, and adds typed `Rerank rerank_spec`/`Expand expand_spec` fields with new numbers.
- **Answered (owner, 2026-10-08, Q-T0-4):** `loams.collection.v1` stays stable. Fields 17 and 18 keep their `Struct` type and are deprecated, and the typed `rerank_spec`/`expand_spec` fields under new numbers are deferred to Task 20.
- DataFusion is 54.1. `TableFunctionImpl::call(&self, &[Expr])` takes its arguments at plan time (`datafusion-catalog-54.1.0/src/table.rs:557`), so a UDTF cannot see a `LATERAL` outer column. This was read from the signature, not executed. Task 21 implements the `seeds => 'SELECT …'` form and makes `lateral_join_with_collection` a documented refusal test.

**R0.20 Desktop (changes Task 8).**
- `web/plugins/graph` is AP1e Task 27's empty state: `src/graph-page.tsx` and `index.tsx`, and one test. It detects with `flags.has('loams.graph.v1')`, which reads `GetInstance.api_versions`, so it shows available packages only and cannot tell `not_in_variant` apart from `absent`. Task 8's `detect.ts` reads `services[]` (status) instead.
- The pattern for a plugin to call Connect: inject `transport`, then `createClient(<pkg>.<Service>, transport)` from `@loams/proto`, as `plugins/data-studio/src/client.ts:102` does.
- In Electron the transport is `createConnectTransport({ baseUrl: page origin, useBinaryFormat: true, fetch: desktopFetch })` (`web/packages/platform-electron/src/index.ts:103`).
- The main process proxies every path that is not `/durable/` or `/loams.live.v1.` to the active server (`apps/desktop-electron/src/main/protocol/route.ts`), so `/loams.graph.v1.*` needs **no proxy change**.
- Task 8 adds `./graph` to `web/packages/proto` (generation plus `exports`), adds `transport` to the plugin's `inject`, and adds `graph` to `apps/desktop-electron/scripts/fetch-engine.mjs`'s `--features live,durable`, its README, and `test/e2e/smoke.spec.ts`'s message. Coordination with the AP owner still applies.

**R0.21 Licences (changes Task 1).**
- All six crates declare `license = "Apache-2.0"` in their published `Cargo.toml`, and crates.io agrees for 0.5.42 through 0.5.44.
- **None of the six `.crate` archives ships a `LICENSE` or `NOTICE` file.** Their `repository` is `https://github.com/GrafeoDB/grafeo`, and `grafeo-storage` lists `authors = ["S.T. Grond"]`.
- Grafeo's `NOTICE` (if there is one) lives only in that repository. It was not read, because this task fetches no external repository (Q-T0-3).
- Task 1 adds the `grafeo-storage` stanza (Apache-2.0) to `connectors/licences.toml`. It adds Grafeo's notice to the root `NOTICE` once the owner supplies it, or a "no NOTICE upstream at <commit>" line.
- `cargo deny` needs no new allow entry. Apache-2.0 is already allowed.

**R0.22 Smaller findings for later tasks.**
- `connectors/registry/grafeo.yaml` and `fabric/crates/loams-flow/tests/registry.rs:83-88` assert that the runtime reference is `loams_flow::connectors::graph`, a module that does not exist. Task 1 points it at the engine's new home (`loams_graph`) or records why not.
- In `fabric/`, nothing but the two graph crates depends on `loams-graph*`, so deleting them only touches `fabric/Cargo.lock` and `fabric.yml`. `fabric.yml` does not mention graph.
- Grafeo's default `max_property_size` is 16 MiB and its default `query_timeout` is 30 s (P11). Task 6's `StatementLimits` sets both explicitly.

**Questions for the owner raised by Task 0**
- **Q-T0-1 (Task 1):** keep 0.5.43 and merge Task 1 on or after 2026-10-11, or drop to 0.5.42 now? (R0.3)
- **Q-T0-2 (Task 26, §48 Review Focus 5):** Grafeo neither stops a runaway statement (R0.8) nor fails it on memory (R0.9). In process, Loams can only answer, detach and reload. Is that acceptable for GA, with upstream fixes asked for (Q679)? Or should each graph's engine run in a child process, which would change D741?
- **Q-T0-3 (Task 1):** the crates ship no NOTICE. Supply Grafeo's upstream `NOTICE`/`LICENSE` at the 0.5.43 tag, or approve fetching it.
- ~~Q-T0-4 (Task 20)~~ **Answered 2026-10-08:** keep `loams.collection.v1` stable; deprecate 17/18 and add the typed `rerank_spec`/`expand_spec` under new numbers in Task 20 (R0.19).

### Task 1 (2026-10-08, on `backend/gr1`)

**R1.1 `loams-proto` generates `loams.graph.v1` from Task 1, not Task 2.** Task 1's manifest change ("depend on `loams-proto`, not `loams-graph-proto`") cannot compile otherwise, so `"loams/graph/v1/graph.proto"` is appended to `crates/loams-proto/build.rs`'s `FILES` here, with the file's content unchanged. The generated path is `loams_proto::loams::graph::v1`; `src/service.rs` and `tests/graph.rs` import it `as pb`, which is the only change to either. Consequences that land with it:
- `loams_proto::FILE_DESCRIPTOR_SET` now carries `loams.graph.v1`, so `grpc.reflection.v1` lists `GraphService` on a build that does not serve it, until Task 5 mounts the package (or its `not_in_variant` stub).
- The SDK facades list `loams.graph.v1` in `PROTO_PACKAGES` (the generator lists every package in `proto/`, as it already does for `loams.collection.v1`). `sdks/{typescript/packages/client/src/gen/facade.ts,rust/src/facade.rs,python/src/loams/_gen/facade.py}` and `crates/loams-facade-gen/tests/golden/{go,python,rust}.golden` are regenerated (one line each). `sdks/go/gen/facade/facade.go` lists no packages and is unchanged.
- `sdks/cpp/buf.gen.yaml` and `sdks/go/buf.gen.go.yaml` take the whole module. C++'s drift check is `git diff --exit-code`, which ignores new untracked files, so `graph.pb.*` (like `collection`) is not committed. Task 2 sets `go_package` per §44.

**R1.2 `buf lint` ignores `proto/loams/graph` until Task 2.** The as-built file fails STANDARD (`google.protobuf.Empty` request/response, non-standard RPC message names). `buf.yaml` gets a commented `lint.ignore: [proto/loams/graph]`; Task 2 deletes it and must pass `buf lint` without it. `buf breaking --against dev` is clean (a new file).

**R1.3 The licence check is a `loams-graph` integration test.** `licences_cover_every_grafeo_crate` (`crates/loams-graph/tests/licences.rs`) reads the root and `fabric/` `Cargo.lock`s with `toml` and fails on any `grafeo*` package without a `[components.<name>]` stanza; it also requires the root lock to link `grafeo` (no vacuous pass) and every Grafeo stanza to be an Apache-2.0 `library`. It failed on `grafeo-storage` before the stanza was added. `fabric/crates/loams-flow/tests/registry.rs`'s component count moves from 47 to 48.

**R1.4 `connectors/registry/grafeo.yaml` keeps `runtime.ref: loams_flow::connectors::graph` (R0.22).** The ref names the Flow *connector* module (planned, not written), not the engine. After D741 the engine is in the engine workspace, which `fabric/`'s `loams-flow` cannot link, so a future Flow connector reaches Loams Graph over `loams.graph.v1`; pointing the ref at `loams_graph` would name a crate the connector can never depend on. Changing it would also mean changing `gen_registry.py`'s `LOAMS_OWNED_NATIVE` rule, `catalog.csv` and §33 for no gain. The manifest's prose about "embedded in the Fabric" is stale and is left for Task 38's docs pass.

**R1.5 CI.** A `graph` job in `ci.yml` (filter: `crates/loams-graph/**`, `crates/loams-proto/**`, `proto/loams/graph/**`, `crates/loams/Cargo.toml`, `connectors/licences.toml`, `fabric/Cargo.lock`, plus the toolchain set) runs `loams_default_build_has_no_grafeo` (the `cargo tree -p loams -e normal --prefix none --locked` output is captured under `pipefail`, must contain the `loams v` root line, and must have 0 `grafeo` lines; it has 0 today) and `cargo test -p loams-graph --locked`, and is in `required`. `fabric.yml` names no graph path, so it is unchanged.

**R1.6 Lockfiles.** Root `Cargo.lock` gains nine packages: `grafeo`, `grafeo-adapters`, `grafeo-common`, `grafeo-core`, `grafeo-engine`, `grafeo-storage` (all 0.5.43), `arcstr` 1.2.0, `crossbeam` 0.8.5 and `fs2` 0.4.3; no `arrow*`. `fabric/Cargo.lock` drops every grafeo crate and arrow 60, which lets its arrow 59 entries lose their version qualifiers. `cargo deny check licenses bans sources` is clean in both workspaces; `advisories` was not run (no local advisory database, and this task fetches nothing).

### Task 2 (2026-10-08, on `backend/gr1`)

**R2.1 The proto follows §48 §8.2, with these additions and choices.**
- `Graph.version` (12) and `UpdateGraphRequest.expected_version` carry Task 4's catalog CAS.
- `ExecuteStream` takes `ExecuteStreamRequest { ExecuteRequest request; uint32 chunk_rows }` instead of the bare `ExecuteRequest`. This keeps `RPC_REQUEST_STANDARD_NAME` clean without an ignore.
- `ExecuteBatchRequest` has `isolation`, `optional if_version`, `timeout_ms` and `idempotency_key`. `ExecuteBatchResponse` has `committed_through` and `consistency_token` (§48 §7.3).
- `Consistency` is a oneof: `strong`, `eventual`, or `at_least` holding a token string.
- `GraphMapping` is `VertexMapping`/`EdgeMapping` per §07 §2.1. Task 17 may refine it while the package is unstable.
- No `go_package` option, as for every other file in `proto/`. The Go stubs template maps it with managed mode (`go_package_prefix loams.dev/go/gen`), and `sdks/go/buf.gen.go.yaml` has an `Mloams/graph/v1/graph.proto=loams.dev/go/gen/loams/graph/v1;graphv1` line in both plugins (fix round 1).
- `ExecuteStream` takes `ExecuteStreamRequest`. §48 §8.2 is amended to match, and §8.3 gains `graph_statement_not_allowed` (R0.10), `graph_unbounded_path` (R0.8) and `graph_catalog_version_mismatch` (`UpdateGraphRequest.expected_version`, now `optional`).
- Both services carry `ModuleOptions { name: "graph", unstable: true }` and no `FacadeOptions` yet (SDK tasks 36–37), so the SDK facades are unchanged.

**R2.2 buf.** R1.2's temporary lint ignore is gone. `graph.proto` joins `collection.proto`'s per-file `RPC_RESPONSE_STANDARD_NAME` and `RPC_REQUEST_RESPONSE_UNIQUE` exceptions. These are forced by §48 §8.2: `Graph` and `Operation` each answer several RPCs, and `EngineInfo`, `GraphSchema`, `Plan` and `ResultChunk` are answers without a Response suffix. `buf breaking` ignores `proto/loams/graph` while the package is `unstable` (§48 §8.1); Task 39 removes that line. `buf lint` is clean, and `buf format -d --path proto/loams/graph/v1/graph.proto` is clean. Other files have formatting diffs that were already there before this task.

**R2.3 `loams-proto` also compiles `loams/operations/v1/operations.proto`,** because `graph.proto` imports it. `loams-apps-mock` generates the same package for its own server, as both crates already do for `loams.errors.v1`. As a result, `FILE_DESCRIPTOR_SET`, and so reflection, lists `loams.operations.v1.OperationsService`. Also, `loams-facade-gen`'s `every_mapped_package_is_generated_by_the_crate_it_names` now checks `loams.operations.v1` (its pinned list is updated), and the Rust map already names `loams_proto::loams::operations::v1`.

**R2.4 Value mapping (`src/value.rs`).**
- Grafeo has no unsigned integer. A `uint64` up to `i64::MAX` decodes to INT64, and a larger one is refused (`uint64_round_trips` pins both).
- Grafeo has no decimal, so a `decimal` is refused.
- Grafeo's `Timestamp` (what `datetime()`/`localdatetime()` produce) is answered as `local_datetime`. Datetimes are microsecond-precise, and a sub-microsecond nanosecond part is refused.
- `Time` without an offset is `local_time`, and with an offset is `zoned_time`.
- Two Grafeo values that are not in §8.2's list get fields of their own: `Vector` (20) and `Counter` (21, GCounter/OnCounter).
- Nodes and relationships are recognised by the engine's projected-map shape: `_id` + `_labels` for a node, and `_id` + `_type` + `_source` + `_target` for a relationship.
- **Measured: Grafeo 0.5.43 builds a `Value::Path` of element ids (`Int64`), not maps.** Controller decision (fix round 1): the server resolves them, so the proto promises full elements.
  - `Graph::resolved` (`engine.rs`) runs on every result Execute and ExecuteBatch build. It looks each id up (`GrafeoDB::get_node`/`get_edge`) and replaces it with the engine's own projected-map shape, so `value.rs` answers full `Node`s and `Relationship`s. Path order and each relationship's stored `src`/`dst` are kept (`path_elements_are_full_and_keep_direction`).
  - **Gap (Task 11):** the lookup reads the current state, not the read's epoch. Grafeo 0.5.43 without its `temporal` feature does not version labels or properties per epoch, so `get_node_at_epoch` would not help. A write committed between the statement and the lookup is visible, and an element deleted in that window stays an id. Task 11's lane (reads pinned, commits serialised) closes it.
- **Shape ambiguities.** A user map whose keys are exactly a projected node's (`_id` INT64 plus `_labels` list of strings) or relationship's is answered as a `Node`/`Relationship`; it still decodes to the same map (`a_map_shaped_like_a_node_is_answered_as_one`). In a path, a node with no labels and no properties decodes to its id, not a map.
- **Reserved property names (N3).** A user property named `_id`, `_labels`, `_type`, `_source` or `_target` is shadowed in a resolved path element: the real field wins (`reserved_property_names_do_not_override_path_elements`). Grafeo's own projection of a bare node or relationship (`RETURN n`, `grafeo-core` `project.rs`) inserts properties *after* the reserved keys, so there the property overwrites the real field, and the wire would carry the wrong id or labels. Task 31 decides whether writes of such property names are refused or the projection is worked around.
- UTC offsets are bounded to ±64800 s on decode, and the zoned subtraction is checked (`offsets_round_trip_and_are_bounded`). `local_datetime` is Grafeo's TIMESTAMP as its UTC wall clock.

**R2.5 The engine answers `grafeo::Value` rows.** `GraphRow.values` is `Vec<grafeo::Value>`. The JSON conversion, `node_ids`, `relationship_ids` and the `rows_read`/`bytes_read`/`rows_affected` wire fields are gone. `Graph::execute_with_params` is new, and `Execute` binds `parameters` through it.

**R2.6 The handlers are transitional until Tasks 3–6.**
- `service.rs` keeps the fabric-era in-memory registry behind the new messages: `create_graph`, `get_graph`, `list_graphs` (one page), `delete_graph` (closes the graph and answers a finished `Operation` with no id), `execute` and `execute_batch`.
- Non-atomic batch parameters are still Task 3's (`non_atomic_batch_binds_parameters`).
- The service takes no storage path, so `tests/graph.rs` exercises persistence and open-conflicts through the engine API.

**R2.7 (superseded by R2.9) `proto_has_no_path_fields` matched words, not substrings.** It splits a field name on `_` and flags `path`, `url`, `uri`, `dir`, `directory`, `file`, `filename`, `folder` or `location`. Matching substrings would flag `ExplainRequest.profile` and `EngineInfo.gql_profile` (they contain "file"). Two graph-semantic fields are allowed beside `object_key`/`object_prefix`: `Value.path` (GQL PATH) and `GraphLimits.max_path_hops`.

**R2.8 Q-T0-4 (fields 17/18 of `loams.collection.v1`) stays with Task 20.** That task adds the typed `rerank_spec`/`expand_spec` fields and deprecates 17 and 18.

**Shared-target hazard.** Worktrees share build-script output in the shared target, so a build can run another worktree's generated code. Run `touch crates/*/build.rs proto/loams/graph/v1/graph.proto` after switching branches or worktrees.

**R2.9 Fix round 1 (review).**
- `ResultChunk`'s last chunk carries `truncated`, `commit_epoch` and `notifications`.
- `ExecuteBatchResponse` documents the atomic and non-atomic contracts, and gains `StatementError error` and `commit_epoch`. A non-atomic batch stops at its first failure and answers the committed results, `committed_through` and the error (`non_atomic_batch_reports_the_failed_statement`).
- Each `Statement.language` is checked (`batch_checks_each_statements_language`).
- `ExplainRequest` gains `timeout_ms` and `consistency`. `service::explain` refuses a writing PROFILE with `graph_read_only`; the plan itself is still Task 6's (`profile_refuses_a_write`).
- Every refusal from `service.rs` now carries an `ErrorInfo` reason.
- `proto_has_no_path_fields` uses substring matching over every file of the package, with the six-field allowlist named in the proto header. This replaces R2.7.

### Task 3 (2026-10-08, on `backend/gr1`)

**R3.1 The gate (`src/classify.rs`).** Every execution path calls `gate(statement, language)` before the engine runs anything: `Execute`, both kinds of batch (an atomic batch gates every statement before the first runs), and `Explain`. The gate:
- refuses an empty statement;
- refuses file access. **The load-bearing check is the plan walk** (next item): Grafeo's own translator produces the plan the engine runs, and a `LoadData`/`LoadGraph` operator anywhere in it is refused. A **keyword backstop** adds defence in depth (corrected in review M3). It reads every alphabetic word, including those inside strings and comments, and refuses `LOAD` followed by `DATA`/`CSV`/`GRAPH`/`JSON`/`JSONL`/`PARQUET`/`FROM`. It catches a statement the translator cannot parse but a future engine might run, and a `CREATE PROCEDURE` whose body loads a file. Its cost is a refusal for a statement that merely mentions "load data" in a string;
- runs `engine_classify` (Grafeo's `translate_full`). The plan check has two passes: the operator tree, and since review C1 the plan's derived `Debug` rendering. The second pass prints every operator and expression, including `EXISTS`/`COUNT`/`VALUE` subqueries, and fails closed. It refuses `LoadData`/`LoadGraph` (file access → `PERMISSION_DENIED`/`graph_statement_not_allowed`, R0.11); `CreateGraph`/`DropGraph`/`CopyGraph`/`MoveGraph`/`AddGraph`/`ClearGraph`/`CreatePropertyGraph` (→ `FAILED_PRECONDITION`/`graph_statement_not_allowed`, R0.10 (b)); and an `Expand` with no `max_hops` or one above `MAX_PATH_HOPS` = 10 (→ `INVALID_ARGUMENT`/`graph_unbounded_path`, R0.8 (b); Task 6 makes the limit per graph);
- refuses session commands: the transaction ones with `graph_transaction_statement`, and every other one (`USE GRAPH`, `SESSION SET …`, `SESSION RESET`, projections, `CREATE`/`DROP GRAPH`) with `graph_statement_not_allowed`. Measured: Grafeo's parameterised path refuses session commands anyway ("Session commands cannot be executed as queries");
- answers `max(guard, engine)`. When the translator cannot parse a statement, the guard's answer stands, and the engine then reports the syntax error.

**R3.2 Engine roles.** `Graph::session_for(Access)` gives `ReadOnly`, `ReadWrite` or `Admin`, and every statement runs on the session for its gated access. A read-only request or graph refuses any access above Read (`graph_read_only`). `read_session_refuses_every_corpus_write` pins that the `ReadOnly` role refuses every Write and Admin corpus case and changes nothing. Statements no longer run through `GrafeoDB::execute`, whose one-shot session wrote `USE GRAPH` back to the database.

**R3.3 `engine_classify(stmt)` takes no database.** The plan says `engine_classify(db, stmt)`. Grafeo's translator needs no database (R0.10), so the parameter is dropped.

**R3.4 The guard (`classify`)** is the fabric-era keyword guard, moved here. It now treats `_` as part of a word, so `n.insert_time` is no longer read as `INSERT`. It answers Admin for `CREATE|DROP|ALTER` followed by `NODE|EDGE|INDEX|CONSTRAINT|TYPE|PROCEDURE|SCHEMA`. The corpus `conformance/graph/gql/classify/*.gql` has 52 cases in four files, in the README's format. The only conservative disagreements are read-only `CALL`s (guard Write, engine Read).

**R3.5 Storage** is `OpenSpec::in_memory()` or `OpenSpec::persistent(engine, GraphId)`, which is `<data_dir>/graphs/gr_<ULID>/graph.grafeo`. `OpenSpec` has no path field, so nothing can name another location. `Engine::with_data_dir` sets the data directory. `CreateGraph` creates a persistent graph when the engine has a data directory and an in-memory one otherwise; it answers an already-open name idempotently and reports `Graph.id` from the directory. Task 4's catalog persists the id.

**R3.6 Panic containment.**
- Every engine call runs inside `catch_unwind`. A panic sets the graph's `poisoned` flag and answers `INTERNAL`/`graph_engine_panic`; the graph's state is `GraphState::Poisoned` (wire `RELOADING`). A direct call on a poisoned graph answers `UNAVAILABLE`/`graph_reloading`.
- The service's next statement on the graph runs `Engine::reopen_if_poisoned`: it closes the poisoned engine and reopens it from the same spec. A persistent graph comes back with what it committed, and an in-memory one comes back empty.
- The test-only failpoint is `loams_graph::engine_call`, behind the `failpoints` feature (the `fail` crate, as `loams-log` uses), in `tests/failpoints.rs`. CI's `graph` job runs it.

**R3.7 Reasons.** `GraphError::reason()` gives the `ErrorInfo.reason`: an engine error whose text contains "syntax error" is `gql_syntax_error`, and the rest are as listed in `graph.proto`'s header. New `GraphError` variants: `TransactionStatement`, `StatementNotAllowed { file_access, what }`, `UnboundedPath`, `EnginePanic` and `Reloading`.

**R3.8 Files.** `service.rs` is split into `service/{mod.rs, data.rs, errors.rs}`; admin handlers stay in `mod.rs` until Task 4's `admin.rs`. New tests are `tests/classify.rs` (corpus, LOAD DATA, management, unbounded paths, storage) and `tests/failpoints.rs`, beside the planned `tests/service.rs` and `tests/graph.rs`. `grafeo-adapters` (=0.5.43, no features) is named for the AST's `SessionCommand`.

**R3.9 Task 3 security review, fix round 1** (one commit each):
- **C1:** the `Debug` pass bounds variable-length patterns inside subqueries (the reviewer's four payloads are in `refused.gql`).
- **I1:** any `CallProcedure` in the plan is at least Write. `bare_words` follows Grafeo's lexer: `-- ` comments only, backslash escapes in strings, doubled backquotes, no `//` comment, Unicode `to_uppercase`. The two read-only `CALL` corpus cases are now `engine=write`.
- **I2:** an in-memory graph, or a persistent one whose poisoned engine will not close (the close flushes the WAL), moves to terminal `GraphState::Failed` (wire `GRAPH_STATE_FAILED`, `FAILED_PRECONDITION`/`graph_engine_panic`) instead of reopening short. Task 5 requires a data directory outside dev mode.
- **I3:** the registry is keyed by `(namespace, name)`, and `validate_names` refuses graph names outside `[a-z][a-z0-9_-]{0,62}` and namespaces outside `[A-Za-z0-9_-]{1,63}`.
- **I4:** the ReadOnly-role corpus test runs on Loams's own `run_engine` path through `Graph::execute_forced` (feature `test-hooks`, enabled only by the crate's self dev-dependency).
- **M2:** shortest-path searches are refused (Task 6 note).
- **M5:** the gate and path resolution run inside `catch_unwind` (failpoints `loams_graph::gate`, `loams_graph::resolve`).
- **M6:** `Graph::open_or_existing` makes `CreateGraph` idempotent under concurrency.
- **M7:** quantifier and held-handle tests.
- **M1, M4 and the N3 id-forging note** are recorded under Tasks 24, 26 and 31.

**R3.10 Task 3 re-review** (one commit each):
- **2a:** to the guard, `<--` and `---` are edges, not comments (Grafeo's lexer reads them as arrows).
- **2b:** `grafeo_debug_format_canary` pins the `Debug` names the plan check reads (`ExistsSubquery(`, `max_hops: None`/`Some(n)`, `CallProcedure(`, `LoadData(`), and fails with "Grafeo Debug format changed; re-verify the classifier (D759)". The plan's `Debug` text is now rendered once.
- **2c (fixed, not deferred):** measured on 0.5.43 in a debug build:
  - Grafeo's parser recurses once per chain link (`NOT`, `AND`/`OR`, `+`, `||`, `[i]`, `NEXT`/`UNION`). That costs about 27 KiB of stack per link (about 70 links on a 2 MiB thread), with no limit.
  - Bracket, `CASE` and subquery nesting costs about 90 KiB per level, and Grafeo caps it at 128 ("Maximum nesting depth of 128 exceeded").
  - Fix: `gate` refuses more than 4000 chain links (a conservative count of operator characters and chaining keywords outside strings and comments: `INVALID_ARGUMENT`/`invalid_argument`, `GraphError::TooComplex`). `translate_full` and every engine call (which re-parses) run on a 256 MiB-stack thread. That stack is virtual and touched only as deep as a statement goes.
  - The panic containment is now that thread's join (it replaces `catch_unwind`).
  - Test: `deep_operator_chains_do_not_crash_the_process`. 2000 chained `NOT`s, 2000 `+ 1`s, 1300 `AND`s and 127 nested parentheses run; 5000-link chains are refused; 500 nested parentheses are Grafeo's own syntax error.
  - Task 6 (pool) and Task 31 (fuzzing) carry the rest.


### Task 4 (2026-10-09, on `backend/gr1`)

**R4.1 Where the catalog lives (deviation).** The plan keyed `graphs/<ns>/<name>` → `GraphMeta` in the metastore's generic object API. That API is its pointers. A pointer value is at most 1 KiB (`MAX_KEY_LEN`, enforced in `loams-meta` `validate_key`), and pointers can be neither listed nor deleted, so per-graph pointers could hold neither a LINKED graph's mapping nor answer `ListGraphs`. The catalog therefore uses the manifest idiom of §03 §3.3:
- each namespace has one document, `{ name → GraphMeta }`, written to the bucket as an immutable object `graphs/<namespace_id>/catalog/<ULID>.json`;
- it is committed by CAS of the namespace's metastore pointer `graph-catalog` to that key;
- a write that loses the CAS retries on the winner's document (up to 32 times);
- a lost acknowledgement is recognised by the pointer already naming the write's own object;
- superseded documents are deleted best effort.

This needs no new metastore type and works on every `MetaStore` backend, TiKV included. Creating a graph creates its namespace when it is absent, as creating a collection does. If a namespace ever needs thousands of graphs, a typed metastore table would replace the single document; Task 25's quota (100 graphs per namespace) keeps the document small.

**R4.2 Records.**
- `GraphMeta` holds `id` (`gr_<ULID>`), `namespace`, `name`, `mode`, `languages` (wire names), `limits`, `replicas`, `state` (`ready` | `deleting {since_ms}`), `created_at_ms`, `version` (CAS; 1 at creation, +1 per update), and the create and delete idempotency keys.
- `CreateGraph` is idempotent by `idempotency_key`. A different key, or none, on a taken name is `ALREADY_EXISTS`.
- `UpdateGraph` takes `languages`, `limits` and `replicas` by `update_mask` (an empty mask means all three; any other path is `INVALID_ARGUMENT`). `optional expected_version` mismatch is `ABORTED`/`graph_catalog_version_mismatch`.
- Name rules are R3.9 I3's (`validate_names`).

**R4.3 Listing.** `ListGraphs` requires a namespace. Pages are by name. `page_size` 0 means 50, the cap is 1000, and a negative size is `INVALID_ARGUMENT`. The token is opaque (`base64url("g1\n<ns>\n<last name>")`), bound to its namespace, and names the last graph returned, so inserts and deletes never shift a page. The last page has no token.

**R4.4 Delete and purge.**
- `DeleteGraph` moves the record to `deleting`, under its id, so the name is free at once and the graph leaves `GetGraph`/`ListGraphs`/`Execute`. It closes the engine (a held handle closes when released) and answers a `SUCCEEDED` operation `op-<26 hex>` whose target is `{graph, graph_id}`.
- A retry with the same `idempotency_key` answers again.
- `GraphAdmin::purge_expired(hold)` removes `<data_dir>/graphs/<id>/` and the record once `hold` (default 24 h) has passed. Task 5 runs it periodically.

**R4.5 Lazy opening.** `GraphAdmin::open` reads the catalog and opens the graph through `Graph::open_or_existing` with `OpenSpec::for_catalog(engine, id)`: persistent under the data directory, or in memory for an engine without one (dev). An engine graph of the same name but another id (a deleted graph a holder kept open) is closed and replaced. `GraphAdmin::{execute, execute_batch, explain, get_schema}` open lazily; the sync `service::data` functions stay the engine-level path. A listed graph that is not open reports `READY`, since it opens on its next statement; `EVICTED` is left for Task 14.

**R4.6 GetSchema** reads Grafeo's `schema()` and `list_indexes()` inside the panic and big-stack containment. Labels, edge types, keys and indexes are sorted.

**R4.7 Kept sync helpers.** `service::{create_graph, get_graph, list_graphs, delete_graph}` remain as engine-registry helpers (tests, the mock). The RPC surface is `service::admin::GraphAdmin`, which Task 5 mounts.

**R4.8 Task 4 review, fix round 1** (one commit each):
- **I1 (with M2):** nothing in the catalog is deleted at once. A read whose GET finds its document gone re-reads the pointer and follows it. `sweep_documents(grace)` (default 10 min) removes superseded documents and lost-CAS orphans, keeping the pointer's target.
- **I2:** writes retry on `VersionMismatch` and on unknown-outcome metastore errors, and each change recognises its own committed effect: create by its graph id, update and delete by a per-call token kept in the record's `recent_writes`, delete also by its `since_ms`. Tested with a "committed, ack lost, another writer on top" hook for each mutation.
- **I3:** concurrency tests run over a bucket that delays every call, assert CAS retries happened, and check an exact final document after racing creates, deletes and `expected_version` updates against readers.
- **I4 (amended by re-review 2):** namespace documents are cached by pointer version, so a statement makes exactly one up-to-date pointer read and no document fetch while the catalog is unchanged. The 1 s `VALIDATION_TTL` was dropped (controller ruling), so a delete takes effect for every statement that starts after `DeleteGraph` returns (`statement_after_delete_returns_is_refused`). With no validation map there is nothing for `remember`/`forget` to race. A statement that read the catalog just before a delete and then has to open the graph is caught by the re-check after the open (M1).
- **M1:** after a lazy open the catalog is re-checked, and the validated graph is passed to execution.
- **M3:** the purge sweep survives a failing namespace (notes under Tasks 12 and 14).
- **M4:** CAS retries use jittered exponential backoff (5 ms × 2^n, cap 200 ms).
- **M5:** `ListGraphs` requires a namespace (§48 §11.1, Task 24, proto).
- **M6:** `replicas` ≤ 8, limits capped, keys ≤ 128 bytes, and a replay under one key with other settings is `INVALID_ARGUMENT` (note under Task 24).
- **M7:** backend errors answer generically.
- **M8:** a note under Tasks 5 and 6.

