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

**Files:** `crates/loams/Cargo.toml` (`graph = ["dep:loams-graph"]`), `crates/loams/src/api/connect.rs` (catalogue row `loams.graph.v1`, services `GraphService`, `GraphAdminService`, `available: cfg!(feature = "graph")`, `unstable: true`; a `GraphAbsent` stub answering `feature_not_in_variant`), `crates/loams/src/api/graph.rs`, `crates/loams/src/server.rs` (role `graph`; config `[graph] data_dir, idle_evict_after, node_memory, limits`), `release/` variant lists (`full` gets `graph`), `crates/loams/tests/it/graph.rs`.

Tests:
- `instance_advertises_graph_when_feature_on` and `…_unavailable_when_off` (two builds in CI).
- `graph_rpcs_answer_not_in_variant_without_feature`: every RPC of both services.
- `connect_json_grpc_and_grpc_web_reach_execute`: one statement over each protocol on the main port.
- `non_loopback_listen_without_authorizer_refused`: `loams serve` with `graph` on a non-loopback address and the `AllowAll` authorizer exits with a clear message (D750; until MT1).

Commit `feat(api): serve loams.graph.v1 behind the graph feature (D741)`.

### Task 6: Limits v1, streaming and redaction

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

**Files:** `src/authz.rs`, the OpenFGA model file MT1 owns (add `type graph`, §48 §11.1), RBAC role expansion, `tests/authz.rs`. Depends on MT1's interceptor; until it lands, test against the `Authorizer` trait with the built-in RBAC.

**Interfaces produced:** per-RPC checks per §48 §11.1's table; `Access` from `classify` + engine; Grafeo session role from the caller's strongest relation; agent scopes `graph:read|write|admin`; `ListGraphs` through `filter_visible`; `graph_classifier_disagreement` logged and counted.

Tests: `reader_cannot_write_any_corpus_write`, `writer_cannot_ddl`, `writer_cannot_delete_graph`, `admin_can_delete`, `cross_namespace_denied`, `agent_scope_graph_read_blocks_writes`, `protected_environment_requires_project_admin`, `list_filters_invisible_graphs`, `denials_are_audited`.

Commit `feat(graph): per-graph authorization (D750)`.

### Task 25: Quotas

**Files:** `src/quota.rs`, the §41 limits record fields, `tests/limits.rs`.

**Interfaces produced:** per-namespace graphs, elements, stored bytes, concurrent statements, statements/s, import bytes/day; `RESOURCE_EXHAUSTED`/`quota_exceeded` with the quota name; counts kept from change sets, not by scanning.

Tests: `graph_count_quota`, `element_quota_blocks_write_and_link`, `stored_bytes_quota`, `rate_quota_429_retry_after`, `quota_counts_survive_recovery`.

Commit `feat(graph): namespace quotas`.

### Task 26: Memory governance

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

(Empty. Task 0 starts it.)
