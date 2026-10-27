# 66 — Loams × Mastra: Compatibility First, a Resonate Engine Upstream, Then a Native `@loams/mastra`

Status: **Approved** · 2026-10-20 (the owner approved every decision and question with its default on 2026-10-20, and on the same day moved the Resonate execution engine forward to run in parallel with Phase 0 and to be contributed upstream to Mastra). Source: the owner's request of 2026-10-20 for a Loams integration with [Mastra](https://github.com/mastra-ai/mastra), the TypeScript agent framework, with the instruction that once the plan is done a public repository `ostrium-labs/loams-mastra` is created and building starts. Decisions **D1202–D1227** and questions **Q1000–Q1014** are in the [decision log](13-decision-log.md). The numbers follow §65 (D1172–D1201, Q984–Q999). Plan: [MA1](../plans/2026-10-20-ma1-loams-mastra.md). **This document writes no code.**

Builds on:
- [§06](06-search-and-vector.md) §8 and the M1.4 plan: the Qdrant gateway and its divergence list (D112, D132–D137); the Elasticsearch gateway (D48, D57, D86, D87).
- [§21](21-durable-execution.md) and [§14](14-durable-execution.md): Loams Durable, the embedded Resonate server (D138–D147, D262), plan [D1](../plans/2026-09-27-d1-durable-execution.md).
- [§44](44-unified-api-and-sdks.md) and plan [SDKP](../plans/2026-10-19-sdk-parity.md): the typed app modules of `@loams/client` (open PR #884), SQL and streams on Connect (P0b, P0c), durable out of the SDKs (SDKP-1, SDKP-8).
- [§46](46-loams-postgres-production.md), [§51](51-loams-postgres-fork.md), [§57](57-loams-gateway.md): Loams Postgres (the Neon hard fork, D820), PgDog behind the gateway's `pg` module (D708, D1011).
- [§52](52-loams-memory.md) and plan [MM1](../plans/2026-10-13-mm1-loams-memory.md): Loams Memory (D870–D891), MM1 Task 50 (`@loams/memory-mastra`, D886).
- [§02](02-stream-engine.md) §7, [§49](49-loams-house-production.md): Streams (D72), OTLP logs only (D73), House pipes (D769).
- [§43](43-private-networking.md) and §65 §11.4: the engine is never on the public internet.
- [§63](63-loams-meter.md), [§64](64-loams-infra-stack.md): the open-source and services model (D1119), usage events from services only (D1133).

Research inputs (2026-10-20; the sources are listed in §16):
- **R-SURF**, Mastra's interfaces, licence and adapters, read at `mastra-ai/mastra@5758ba045b01c407a7414b32dbbdf5480596b31d`.
- **R-CAP**, what Loams has and lacks, read at `ostrium-labs/loams@77692a45f`.
- **R-PRIOR**, how other vendors integrate with Mastra and its neighbours, and what made integrations get adopted.

Markers, as in §64 and §65:
- **(verify)** means the claim has not been checked against a primary source. The task that depends on it checks it first.
- **(estimate)** means the number was judged, not measured.
- **(unverified)** means a search did not settle it.

---

## 1. Summary

### 1.1 The request and what it means

Mastra is the most used TypeScript agent framework: `@mastra/core` had about 1.33 million downloads a week and 8.0 million a month on 2026-10-20, with a minor release every 2–4 days (R-SURF §2.1, §8). Loams can serve Mastra in three ways:

1. **Through compatibility, with no new code.** Loams Postgres runs Mastra's own `@mastra/pg` (all storage domains plus pgvector), and the Loams Qdrant gateway runs `@mastra/qdrant`. This is how Neon and ClickHouse Managed Postgres got into Mastra's integrations grid: a docs page and no package (R-PRIOR §1.2, §3).
2. **Through a native package**, `@loams/mastra`, for what only Loams has: hybrid BM25 + vector search in one call, storage on collections, a `PubSub` on Streams, a span exporter into Streams and House, and Loams Memory's graph.
3. **Through a durable execution engine.** Mastra's `ExecutionEngine` is pluggable. Inngest (283k downloads a month) and an experimental Temporal engine are the only ones today, and Resonate has nothing (R-PRIOR §5). Loams Durable *is* a Resonate server, so a Resonate engine gives every Loams user durable Mastra workflows, and gives every Resonate user Mastra.

### 1.2 The recommendation in one paragraph

Prove compatibility first, in public, and let Mastra's own tests be the judge. Create **`ostrium-labs/loams-mastra`** (Apache-2.0). Its first deliverable (**MA0**) is a conformance gate that runs Mastra's pg and Qdrant adapter test suites, vendored at pinned release tags, against `loams dev` and Loams Postgres, plus a three-block quickstart and an issue-first docs PR to Mastra's integrations list. In parallel (**MAR**), build a **Resonate `ExecutionEngine`** in Mastra's own layout and contribute it upstream as **`@mastra/resonate`** in `workflows/resonate`, next to Inngest. Until Mastra accepts it, publish it from our repository as `@loams/mastra-resonate`. Fix the Loams API gaps the gate finds as small monorepo PRs (**MA1**): the Qdrant payload-index delete, Elasticsearch `GET /_cat/indices`, and the JS Qdrant client in CI. Then build **`@loams/mastra`** on the typed `@loams/client` (**MA2**): `LoamsVector`, `LoamsStore`, `LoamsPubSub`, `LoamsCache` and `LoamsExporter`. When Loams Memory exists, ship it as Mastra processors and tools, and show `loams-mcp` from Mastra's MCP client (**MA3**). Publish everything with npm provenance, exact pins in examples and templates, and a compatibility matrix against Mastra `latest` and `alpha`, because `@mastra/*` itself was compromised on npm in June 2026.

### 1.3 Phase order (D1204)

| Phase | Content | Where | Starts |
|---|---|---|---|
| **MA0** | Zero-package compatibility: the `loams-mastra` repository, the conformance gate (Mastra's pg and Qdrant suites against Loams), quickstarts, a template, the upstream docs page | `ostrium-labs/loams-mastra`, `mastra-ai/mastra` (docs) | Now |
| **MAR** | The Resonate execution engine: built in `loams-mastra/packages/resonate` in Mastra's layout, tested against an upstream Resonate server and Loams Durable, contributed upstream as `@mastra/resonate` | `ostrium-labs/loams-mastra`, `mastra-ai/mastra` (`workflows/resonate`) | Now, in parallel with MA0 (owner, 2026-10-20) |
| **MA1** | Loams API fixes found by the research and by MA0's gate | `ostrium-labs/loams` (monorepo PRs into `dev`) | Now; one task waits for MA0's first gate run |
| **MA2** | Native `@loams/mastra` on `@loams/client` | `ostrium-labs/loams-mastra` | After #884 merges |
| **MA3** | Loams Memory as Mastra processors and tools (replaces MM1 Task 50's location); `loams-mcp` from Mastra's `MCPClient` | `ostrium-labs/loams-mastra` | After MM1a's Memory MVP and #847 |
| **Adoption** | Template, example app, docs on loams.dev and upstream, compatibility matrix, provenance, LongMemEval once Memory exists | all of the above, plus `ostrium-labs/loams-site` | Throughout |

The lead's first ordering put the durable engine third. The owner moved it on 2026-10-20 to run in parallel with MA0 and to go upstream, and the later phases were renumbered: the former Phase 4 (Memory and MCP) is now **MA3**.

### 1.4 What changes

| Before | After |
|---|---|
| MM1 Task 50: `adapters/mastra/` → `@loams/memory-mastra` in the monorepo, "Mastra's memory or storage adapter interface" (D886) | Loams Memory for Mastra is `@loams/mastra/memory` in `ostrium-labs/loams-mastra`, shipped as processors and tools, not a storage adapter or `MastraMemory` subclass (**amends D886 for Mastra**, D1221) |
| No framework integration repository | `ostrium-labs/loams-mastra`, the pattern for later framework repositories (LangGraph, OpenAI Agents) |
| The JS Qdrant client is untested in Loams CI (R-CAP §2.1) | `@qdrant/js-client-rest` 1.18.0 and 1.19.0 run in the `qdrant-client` CI job (MA1) |
| `DELETE /collections/{c}/index/{field}` answers 501 | Served (D1210) |
| Elasticsearch has no `_cat` route | `GET /_cat/indices` served (MA1) |
| Resonate has no Mastra engine; Loams Durable has no framework integration | `@mastra/resonate` upstream, `@loams/mastra-resonate` meanwhile (D1211) |

## 2. Mastra in one page (R-SURF §1–§4)

- **Licence.** Everything is Apache-2.0 ("Copyright (c) 2025 Kepler Software, Inc.") except directories named `ee/`. Those are under the **Mastra Enterprise Edition License v2.0** (effective 2026-09-22): source-available, production use only with a written agreement and a key, no redistribution, and Kepler owns your modifications (§3.3). The `ee/` directories hold auth/RBAC/SSO, the agent builder and the editor/playground. `@mastra/connect`'s `src/providers/` are **Elastic-2.0** (adapted from Nango). Nothing an integration needs (core, storage, vector, memory, rag, mcp, pg, qdrant, elasticsearch, inngest, temporal, observability, the test utilities) is under `ee/`. There is no CLA and no DCO. A PR must link an issue that maintainers have approved, or it is closed automatically.
- **Versions** on 2026-10-20: `@mastra/core` 1.78.0 (`alpha` 1.79.0-alpha.6), `@mastra/pg` 1.31.2, `@mastra/qdrant` 1.1.4, `@mastra/elasticsearch` 1.4.4, `@mastra/memory` 1.38.0, `@mastra/inngest` 1.10.6. Every adapter takes `@mastra/core` as a ranged **peer** (pg: `>=1.76.0-0 <2.0.0-0`). Releases are tagged per package (`@mastra/pg@1.31.2`). Core needs Node ≥ 22.13.0.
- **One `new Mastra({...})`** takes `storage`, `vectors`, `pubsub`, `cache`, `observability`, `deployer`, `mcpServers`, `workers` and others. No registry: any subclass instance works, which is why third parties can publish their own adapters.
- **Extension points** (quoted from source in R-SURF §3):
  - `MastraCompositeStore` with about 24 **domains** (`memory`, `workflows`, `scores`, `observability`, `datasets`, `schedules`, …). Each domain can come from a different backend through `domains`, `default` and `editor`.
  - `MastraVector`: `createIndex`, `upsert`, `query`, `listIndexes`, `describeIndex`, `deleteIndex`, `updateVector`, `deleteVector(s)`, `isSelfEmbedding`, and a `BaseFilterTranslator` for the Mongo-style filter.
  - `ExecutionEngine` and `DefaultExecutionEngine`, whose hooks (`wrapDurableOperation`, `executeSleepDuration`, `executeSleepUntilDate`, `executeStepWithRetry`, `requiresDurableContextSerialization`) exist for durable backends.
  - `PubSub` (with offsets and replay) and `LeaseProvider`; `MastraServerCache` (KV plus lists); `ObservabilityExporter` and `BaseExporter`; `MastraDeployer`; `WorkspaceFilesystem` and `WorkspaceSandbox`; `MCPClient` and `MCPServer`.
  - Memory (`@mastra/memory`) is storage- and vector-agnostic; vendors plug in as **processors and tools**, not as new `MastraMemory` classes.
- **Shared test suites.** `stores/_test-utils` is the private `@internal/storage-test-utils` (`createTestSuite`, `createVectorTestSuite`). `workflows/_test-utils` covers engines. Neither is on npm, so third parties vendor them (Apache-2.0).

## 3. Principles

1. **Mastra's tests are the contract.** Loams is compatible when Mastra's own adapter suites pass against it, not when our tests say so (D13's rule, applied to a framework).
2. **Zero packages before packages.** Every user who can stay on `@mastra/pg` or `@mastra/qdrant` should. A native package exists only for what compatibility cannot give.
3. **Domains, not one more vector tab.** Adoption follows the default backend for memory, workflows and observability: pg has 3.75M downloads a month against 8k–45k for any pure vector store (R-PRIOR §3).
4. **Upstream what is generic, own what is Loams.** A Resonate engine is generic, so it goes to Mastra. Hybrid search on collections, the graph and Streams are Loams, so they stay in `@loams/mastra`.
5. **The licence boundary is a wall.** No `ee/` code, no Elastic-2.0 code, nothing from Mastra that needs a key, ever.
6. **Supply chain as a feature.** Provenance, exact pins and blocked install scripts, because the framework's own scope was poisoned once.
7. **The engine is never public.** Every example runs on loopback or a tailnet.

## 4. The map: Mastra extension point → Loams component → package → phase

| Mastra extension point | Loams component | Package | Phase | Notes |
|---|---|---|---|---|
| Storage, every domain (`PostgresStore`) | Loams Postgres (Neon fork) | `@mastra/pg`, unchanged | MA0 | The default path. Verification list in §5.3 |
| `MastraVector` (`PgVector`) | Loams Postgres + pgvector | `@mastra/pg`, unchanged | MA0 | Needs pgvector ≥ 0.7 |
| `MastraVector` (`QdrantVector`) | Qdrant gateway `:6333` | `@mastra/qdrant`, unchanged | MA0 | One 501 (`deletePayloadIndex`), fixed in MA1 |
| `MastraVector` + storage (`ElasticSearchVector`, ES domains) | Elasticsearch gateway `:9200` | `@mastra/elasticsearch`, unchanged | MA0 observed, MA1 fix | `listIndexes` needs `_cat/indices`; an observed cell, not a supported path (Q1009) |
| `ExecutionEngine` (`ResonateExecutionEngine`) | Loams Durable, or any Resonate server | `@mastra/resonate` upstream; `@loams/mastra-resonate` until accepted | **MAR** | Inngest's runtime-wrapper pattern (§7) |
| `MastraVector` (`LoamsVector`, hybrid) | Collections + `QueryService/Search` | `@loams/mastra/vector` | MA2 | BM25 + vector with RRF; Mongo filter → `Query` IR |
| `MastraCompositeStore` domains `memory`, `workflows`, `scores`, `observability` (`LoamsStore`) | Collections, `sql.query` for listings | `@loams/mastra/storage` | MA2 | Other domains composed from `@mastra/pg` |
| `PubSub` (`LoamsPubSub`) | Streams (`loams.stream.v1`) | `@loams/mastra/pubsub` | MA2 | Offsets and replay |
| `MastraServerCache` (`LoamsCache`) | Collections | `@loams/mastra/cache` | MA2 | `increment` atomic per process only until conditional writes (Q1003) |
| `ObservabilityExporter` (`LoamsExporter`) | Streams → House | `@loams/mastra/observability` | MA2 | No OTLP traces ingest needed |
| Memory processors and tools | Loams Memory (`loams.memory.v1`) | `@loams/mastra/memory` | MA3 | Zep's shape; amends D886 |
| `MCPClient` | `loams-mcp` (#847) | none: an example and a test | MA3 | Works without Mastra-specific code |
| `LeaseProvider` | — | not built | — | Needs compare-and-swap (D89 not built) |
| `MastraDeployer` | agentd, `loams-sandbox` | not built | — | Low value (R-SURF §3.7) |
| `WorkspaceFilesystem` / `WorkspaceSandbox` | Loams Git (§36), `loams-sandbox` (§56) | not built now | later | Outside the data platform; revisit after SB1 |
| `@mastra/core/auth/ee`, editor `ee`, agent-builder `ee` | — | **never** | — | EE licence (D1203) |

## 5. Phase 0 (MA0): compatibility with no new package

### 5.1 The quickstart

Two variants, each three code blocks, as Neon's page is (R-PRIOR §3.2):

```ts
// Variant A (default, Q1011): everything on Loams Postgres.
import { Mastra } from '@mastra/core/mastra'
import { PostgresStore, PgVector } from '@mastra/pg'

export const mastra = new Mastra({
  storage: new PostgresStore({ id: 'loams-pg', connectionString: process.env.DATABASE_URL! }),
  vectors: { loams: new PgVector({ id: 'loams-pgvector', connectionString: process.env.DATABASE_URL! }) },
})
```

```ts
// Variant B: storage on Loams Postgres, vectors on the Loams Qdrant gateway.
import { QdrantVector } from '@mastra/qdrant'

const vector = new QdrantVector({
  id: 'loams-qdrant',
  url: process.env.LOAMS_QDRANT_URL ?? 'http://127.0.0.1:6333',
  apiKey: process.env.LOAMS_API_KEY,                                  // an lk_ key; omitted on loopback with auth off
  headers: { 'Loams-Namespace': process.env.LOAMS_NAMESPACE ?? 'default' },
})
```

Both run against local `loams dev` plus the Loams Postgres development stack (`deploy/loams-postgres-dev`, shipped in the release's `loams-compose-<version>.tar.gz`), or against a compose deployment reached over a tailnet. No example points at a public endpoint (D1208).

### 5.2 The conformance gate (D1206)

What runs:

| Suite | Source | Against | Cell |
|---|---|---|---|
| Qdrant | `stores/qdrant/src` at tag `@mastra/qdrant@<pinned>`: `index.test.ts`, `filter.test.ts`, `payload-index.test.ts` and the `createVectorTestSuite` block | `loams dev` Qdrant REST on `127.0.0.1:6333` | Required |
| Qdrant with auth | The same, with `LOAMS_AUTH_MODE=apikey`, an `lk_` key in `apiKey` and `Loams-Namespace` set | `loams dev` with auth on | Required |
| pg storage | `stores/pg/src` at tag `@mastra/pg@<pinned>`: `createTestSuite` (every domain) and the pg-specific tests, minus performance and stress tests | Loams Postgres `compute1` (`127.0.0.1:55433`) | Required |
| pg vector | `stores/pg/src/vector` tests at the same tag | the same | Required |
| pg through a pooler | `*.pooler.test.ts` at the same tag | PgDog in transaction mode in front of `compute1` | Required once PgDog's pin is resolved (verify) |
| Elasticsearch | `stores/elasticsearch/src` at tag `@mastra/elasticsearch@<pinned>` | `loams dev` ES on `:9200` | Observed (Q1009) |
| Example end to end | `examples/quickstart-*` with a mock model | published `@mastra/*` tarballs at the pinned versions | Required |

How it judges:
- Each suite has an **expected-failures list** (`test/conformance/expected-failures/<suite>.json`). Every entry names the test, the reason and a tracking issue in `ostrium-labs/loams`.
- The gate **fails on an unexpected failure and on an unexpected pass**. A fix in Loams must shrink the list in the same week, so the list is always the truth.
- The pinned cell runs on every PR to `loams-mastra`. Mastra `latest` and `alpha` run nightly against the pinned Loams release, and against the next Loams release candidate when one exists. A nightly failure opens or updates one issue labelled `mastra-compat` (§10).
- The suites run against **their own source at the release tag** (vendored, §5.4). The example cell runs against the **published** tarballs. The first proves Loams' behaviour, the second proves what users install.

Expected failures before MA1 (from R-SURF §5.1–§5.2; the first gate run confirms them):
- Qdrant: `deletePayloadIndex` (501); any geo, `nested` or `has_vector` filter case (501, which Memory never uses); `describeIndex` on named vectors (a Mastra bug, also against real Qdrant).
- Elasticsearch: `listIndexes` (no `_cat/indices`), and every Memory path that discovers indexes through it.
- pg: none expected; the verification list below decides.

### 5.3 Loams Postgres for `@mastra/pg` (D1207)

Loams Postgres is what backs `@mastra/pg` compatibility. The fork's compute images (`loams-postgres-compute-v16`/`-v17`, release `lp-2026.11.0`) carry pgvector in the design (§51, NF1's manifest example shows 0.8.1). The gate verifies, and fails with a named check, on each of these:

| # | Check | How | If it fails |
|---|---|---|---|
| V1 | pgvector ≥ 0.7 (`halfvec`, `sparsevec`, `bit`; Mastra's tests use 0.8.4) | `SELECT extversion FROM pg_available_extensions WHERE name = 'vector'`; the pg vector suite's `halfvec` cases | A fix in `ostrium-labs/loams-postgres`'s extension manifest |
| V2 | `CREATE EXTENSION IF NOT EXISTS vector` as the application role | The suite's `PgVector` init as a non-superuser role created like pg-control's | Document a pre-created extension; pg-control creates it on branch creation |
| V3 | Advisory transaction locks (`pg_advisory_xact_lock`) and `FOR UPDATE SKIP LOCKED` **through the pooler** in transaction mode | The `*.pooler.test.ts` cell through PgDog | Document the session-mode or direct connection for workers; raise a PgDog issue |
| V4 | The observability vNext partition mode | The suite logs `PartitionMode`; with no TimescaleDB, `pg_partman` if present, else `native` | `native` is acceptable; record which mode Loams gets |
| V5 | `disableInit: true` plus `storage.init()` in CI, and the schema option | The suite's `schema-setup-per-database` and config tests | Documented divergence |
| V6 | Read and write pools (`PostgresStore`'s separate pools) against one compute | Config tests | Documented |
| V7 | Neon-specific behaviour: suspended computes waking on connect, `statement_timeout`, connection limits | The quickstart's e2e test after an idle period, run nightly (verify that a dev compute suspends) | Documented in the quickstart |

### 5.4 Vendoring Mastra's tests (D1203, D1206)

- `scripts/vendor-upstream.mjs --package <name> --tag <tag>` reads the git tree of `mastra-ai/mastra` at the tag through the GitHub API and fetches the listed directories from `raw.githubusercontent.com`. It does not clone. It writes them under `test/conformance/upstream/<package>@<version>/` with `UPSTREAM.json` (tag, commit, path list, SHA-256 per file).
- It **refuses** any path containing an `ee/` segment, `packages/connect`, or a file whose header names a licence other than Apache-2.0. A test asserts the refusal.
- Each vendored file gets a header line `Vendored from mastra-ai/mastra@<commit>:<path> (Apache-2.0). Changes: <list or "none">`, and `NOTICE` names Kepler Software, Inc.'s copyright and the vendored directories (Apache-2.0 §4).
- The `workspace:*` and `catalog:` dependencies of the vendored `package.json` files are rewritten to exact npm versions recorded in `test/conformance/pins.json`; `@internal/storage-test-utils` and `@internal/workflow-test-utils` (verify the name) resolve to their vendored copies.
- Nightly `latest` and `alpha` cells vendor into a scratch directory at run time. Only the pinned copy is committed.

### 5.5 The upstream docs page

Mastra's process is issue-first (R-SURF §4.3):
1. Open a feature-request issue in `mastra-ai/mastra`: "docs: add Loams to the Databases integrations (uses `@mastra/pg` and `@mastra/qdrant`, no new package)". Link the green conformance run.
2. When the issue is approved, open a PR adding `docs/src/content/en/integrations/databases/loams.mdx` (Neon's shape: Quickstart, Installation, Usage, Vector search with both variants, Production connections over a tailnet, Initialization, Related) and its `docs/src/content/en/integrations/sidebars.js` entry. Answer every CodeRabbit and Mastra Platform comment.
3. Keep the source of the page in `loams-mastra/docs/upstream/loams.mdx`, so it is versioned with the gate that backs it.

The page says plainly that the Loams engine is self-hosted and reached over loopback or a tailnet, and links the compatibility matrix.

## 6. Phase 1 (MA1): Loams API fixes (D1209, D1210)

Each fix is one PR into `dev` of `ostrium-labs/loams`, with tests in the monorepo and an expected-failures update in `loams-mastra`.

| Fix | Today | After | Tests |
|---|---|---|---|
| Qdrant `DELETE /collections/{c}/index/{field}` | On the `UNSUPPORTED` list, 501 (`crates/loams-qdrant/src/rest.rs:51`) | Served: retires the payload index (below) | Rust `it/schema.rs`; Python smoke check `payload_index_delete`; JS smoke |
| Elasticsearch `GET /_cat/indices[/{index}]` | No `_cat` route (`crates/loams-es/src/lib.rs`) | `format=json` and the default text table, with `h`, `v`, `s`, `bytes`, `health`, `expand_wildcards`; the columns `health status index uuid pri rep docs.count docs.deleted store.size pri.store.size` | Rust `it`; `conformance/es-client/test_client.py::test_cat_indices` against the ES 8.19 oracle |
| `@qdrant/js-client-rest` in CI | Only Python `qdrant-client` 1.15.1 and 1.19.1 run (`ci.yml` `qdrant-client` job) | A JS smoke (1.18.0, Mastra's floor, and 1.19.0) in the same job, covering every call `@mastra/qdrant` makes, plus `headers` and `api-key` | `crates/loams-qdrant/tests/js/` |
| Whatever MA0's gate finds | — | One PR per finding | Named per finding |

**Payload-index delete semantics (D1210).** A Qdrant payload index is a typed schema field `payload_index.<key>` (`crates/loams-qdrant/src/schema.rs:271-336`), and filters always read the catch-all `payload` field (D134). Collections have no field drop. So `DELETE` **retires** the index: it records the key in the collection annotation `qdrant.payload_index.retired` (a comma-separated set, written through `add_fields`' annotations), answers `{"status":"completed"}` with `operation_id` like Qdrant, and from then on `GET /collections/{c}` leaves the key out of `payload_schema`. Filters on the key keep working. Re-creating the index with the **same** type removes the key from the retired set. Re-creating it with a **different** type stays 501 "changing payload index type", as today, and is added to the divergence list in `crates/loams-qdrant/src/lib.rs`. Deleting an index that does not exist answers 200, as Qdrant does (verify against Qdrant 1.19.1 in the Python oracle check).

Not in MA1: Qdrant `geo`, `nested` and `has_vector` filters and geo indexes (Memory and RAG do not use them; on demand only); OTLP traces ingest (MA2's exporter does not need it); an OpenSearch `knn_vector` dialect (never, §13).

## 7. Phase R (MAR): the Resonate execution engine, upstream (D1211–D1213)

### 7.1 Why now, and why upstream

- **The slot is open.** Inngest is the only production engine for Mastra; Temporal's is experimental; Restate, DBOS, Trigger.dev and Resonate have nothing (R-PRIOR §5).
- **Loams Durable is a Resonate server** (§21): "the official Resonate SDKs run durable functions against Loams unchanged". A generic Resonate engine runs on Loams with no Loams code.
- **Upstream is where engines live.** Both existing engines are in Mastra's `workflows/` folder and released by Mastra. A Resonate engine there is listed in Mastra's docs, tested in Mastra's CI against every core change, and owned in maintenance by Mastra after merge (R-PRIOR §1.2). That is the right home for generic code (principle 4).
- **Our copy until it lands.** Mastra's issue approval and review take time. The same code, published as `@loams/mastra-resonate` from `loams-mastra`, lets Loams users start now. When `@mastra/resonate` is released, ours is deprecated with a pointer and re-exports it for one minor version.

### 7.2 Design (Inngest's runtime-wrapper pattern)

`ResonateExecutionEngine extends DefaultExecutionEngine` and overrides the hooks Mastra built for durable backends (R-SURF §3.4, §6):

| Hook | Resonate (`@resonatehq/sdk`, pinned 0.11.5; names verify) |
|---|---|
| `wrapDurableOperation(id, fn)` | `ctx.run(fn)` with a deterministic id derived from `operationId` (a durable child promise; memoised on replay) |
| `executeStepWithRetry` | Each attempt one `ctx.run`; errors surfaced as non-retryable to Resonate so **Mastra's `retryConfig` governs retries** (Inngest's choice, R-PRIOR §5 lesson 5) |
| `executeSleepDuration`, `executeSleepUntilDate` | `ctx.sleep(ms)` (durable timers) |
| `requiresDurableContextSerialization()` | `true` |
| `getEngineContext()` | The Resonate context and the root promise id |
| Span hooks | Unchanged; spans flow to Mastra observability |

Around it, following `@mastra/inngest`:
- `init(resonate)` returns `{ createWorkflow, createStep }`; each Mastra workflow becomes one registered Resonate function (`resonate.register(workflowId, fn)`), and a run is `resonate.run(runId, workflowId, input)` (verify the 0.11.5 names).
- A `ResonateWorkflow` and `ResonateRun` pair (start, `startAsync`, status, result, cancel).
- Snapshots stay in the `workflows` **storage domain** (`@mastra/pg` or any store). Resonate memoises step results; it is not a queryable run store (R-CAP §4).
- Workers are any process that constructs `Resonate.remote({ url, group })` and registers the workflows; no HTTP route is needed, unlike Inngest's `serve`.

**MVP (MAR):** start, run to completion, step memoisation and replay after a crash, sleep and sleep-until, nested workflows, status mapping, `onStart`/`onFinish`, deterministic operation ids, a JSON-safe codec for step results and errors.
**After MVP (still MAR, separate PRs):** suspend and resume as an externally settled durable promise (`resume()` settles it; the persisted resume claim deduplicates); then streaming through Mastra's `PubSub`. Time travel and restart come last, if Mastra's test utilities require them for acceptance.

### 7.3 Testing (D1213)

- Mastra's `workflows/_test-utils` vendored (Apache-2.0, §5.4) and run against the engine, as Inngest's tests do.
- Two backends in CI, both on GitHub-hosted runners: the **upstream Resonate server** at a pinned release (the target Mastra's CI will use), and **Loams Durable** (`loams dev` with the durable listener on `127.0.0.1:8001`; the release binary carries the `durable` feature, verify).
- A crash test against Loams: `kill -9` the worker mid-workflow, restart it, and expect the run to finish with each step executed once (the D1 examples gate's pattern).
- Determinism tests on operation ids (Mastra has `durable-operation-ids` and `mapping-id-determinism` tests; R-PRIOR §5).

### 7.4 Upstream process

1. A feature-request issue in `mastra-ai/mastra`: "Resonate execution engine (`@mastra/resonate`, `workflows/resonate`)", linking the green CI run and the design. Mention Resonate's maintainers so they can follow (Q1014).
2. Wait for approval. A PR without an approved issue is closed.
3. The PR moves `packages/resonate` into `workflows/resonate` with Mastra's conventions: `@internal/*` dev dependencies, `workspace:*` core, a changeset, `tsdown`, `docker-compose.yaml` for the Resonate server, and a docs page `docs/src/content/en/integrations/deploy/resonate.mdx` modelled on Inngest's.
4. Our package tracks Mastra's review; fixes land in both until merge, then only upstream.

### 7.5 Loams-specific caveat

Loams Durable is **instance-wide**: an environment-bound `lk_` key is refused (R-CAP §4; D142's per-namespace instance is designed only). On `loams dev` (auth off, loopback) nothing changes. On a shared deployment the engine needs an operator or org key, which is acceptable for a self-hosted single-tenant install and not for a multi-tenant one (Q1012). The upstream package says nothing about Loams; Loams' docs carry the caveat.

## 8. Phase 2 (MA2): the native `@loams/mastra` (D1214–D1220)

### 8.1 Package shape (D1214)

One package, `@loams/mastra`, ESM, Node ≥ 22.13, with subpath exports so a user pays only for what they import:

| Subpath | Exports | Extra peer (optional) |
|---|---|---|
| `@loams/mastra/vector` | `LoamsVector`, `LoamsFilterTranslator`, `LOAMS_PROMPT` | — |
| `@loams/mastra/storage` | `LoamsStore` and its domain classes | — |
| `@loams/mastra/pubsub` | `LoamsPubSub` | — |
| `@loams/mastra/cache` | `LoamsCache` | — |
| `@loams/mastra/observability` | `LoamsExporter` | `@mastra/observability` |
| `@loams/mastra/memory` (MA3) | processors and tools | `@mastra/memory` |
| `@loams/mastra` | re-exports and `createLoams(env)` | — |

Dependencies: `@loams/client` (exact minor range), peer `@mastra/core` `>=<floor> <2.0.0` where the floor is the oldest core whose interfaces the package uses (§10). The Resonate engine stays a separate package (§7) because it must be movable upstream unchanged.

### 8.2 Built only on the typed client (D1215)

MA2 starts after **#884** (the typed `namespaces`, `collections`, `documents`, `search`, `vector`, `graph`, `sql` and `streams` modules, `Loams.fromEnv()`, the `Filter` and `Search` builders) merges and `@loams/client` with it is released. No `loams.modules[...]` untyped calls. Gaps in the client found by MA2 are fixed in the monorepo first.

### 8.3 `LoamsVector` (D1216)

- **One collection per index**, named `<prefix><indexName>` (default prefix `mastra_`). Schema: a dense vector `embedding` (`dim` from `createIndex`; Mastra `cosine` → `Cosine`, `euclidean` → `Euclid`, `dotproduct` → `Dot`; f32), a `metadata` field of kind `json`, and a `text` field (BM25) filled from `metadata.text` when present (the `@mastra/rag` chunk convention) or from `documents` when given.
- `upsert` → `documents.writeBatch` (one atomic request per batch on one collection); `deleteFilter` → `deleteByFilter` first in the same call sequence (not atomic; documented).
- `query`:
  - `queryVector` alone → a `vector` retriever.
  - `queryVector` + `queryText` (a Loams extension, also accepted by the RAG tool wrapper) → **hybrid**: a vector retriever and a BM25 `text` retriever fused by RRF (`k = 60` default; `dbsf` or weights as options).
  - `sparseVector` → a `sparse` retriever.
  - `filter` → the translated `Query`; `includeVector` → `select`.
- `describeIndex` → `collections.get` (`dim`, distance, `live_doc_count` read at `STRONG`); `listIndexes` filters by prefix and keeps Memory's `memory<sep>messages` naming working.
- **Filter translation** (`LoamsFilterTranslator extends BaseFilterTranslator`): `$eq`/`$in` → `term`/`terms`; `$ne`/`$nin` → `bool.must_not`; `$gt…$lte` → `range`; `$exists` → `exists`; `$and`/`$or`/`$not`/`$nor` → `bool`; `$all` → `bool.must` of `term`s; `$size` → `values_count`; `$regex` → `prefix` or `wildcard` when the pattern is anchored and simple, otherwise refused with a clear error; `$elemMatch` refused (no nested). Paths become `metadata.<path>` through the `json` field's JSON-path resolution (R-CAP §1.2). `LOAMS_PROMPT` documents exactly this for LLM-generated filters.
- `isSelfEmbedding` is **false** until Loams collections can embed on write and on query (Q1002).
- Tests: Mastra's `createVectorTestSuite` (vendored), the translator's own table, a hybrid recall test, Memory semantic recall end to end.

### 8.4 `LoamsStore` (D1217)

- Domains `memory`, `workflows`, `scores` and a minimal `observability` (the span write and trace read methods Studio uses; verify the list against Studio at the pinned core). Every other domain is composed from `@mastra/pg` with `MastraCompositeStore`, and the docs show the composition.
- **Memory:** threads, messages and resources in **one collection per store** with a `kind` discriminator, so saving a thread and its messages is one atomic `WriteDocuments` (atomicity is per request on one collection; R-CAP §8). Ids are UUIDv7. Listings use `search` sorted on `createdAt` with `search_after`, or `sql.query` with `ORDER BY … LIMIT` where SQL is simpler; message context windows are two reads (hit, then range by `threadId` and `createdAt`). Reads use the session consistency store, so read-your-writes holds.
- **Workflows:** snapshots as documents keyed `<workflowName>:<runId>`, upserted whole; `updateWorkflowResults` and `updateWorkflowState` read, merge with Mastra's `mergeWorkflowStepResult` and write. Without compare-and-swap, `supportsConcurrentUpdates()` and `supportsRunFencing()` return **false**, and users who need fencing compose `@mastra/pg` for `workflows` (Q1003).
- **Scores:** one collection, filters on scorer, run, entity and span ids.
- **Retention:** Mastra's `prune()` with per-table `maxAge` maps to `deleteByFilter` on `createdAt`. That covers the TTL gap (R-CAP §8) for Mastra's purposes.
- Tests: Mastra's `createTestSuite` (vendored) with the domains above, capabilities declared, and a composite test with `@mastra/pg` for the rest.

### 8.5 `LoamsPubSub` (D1218)

- One stream per topic family (`mastra.workflows`, `mastra.agents`, configurable), records keyed by `runId`, CloudEvents JSON, idempotent producer ids.
- `supportedModes: ['pull']`, `supportsOffsets: true`; `subscribe` is a long-poll `fetch` loop (`max_wait_ms` ≤ 30 000); `getHistory` and `subscribeFromOffset` read from an offset; `subscribeWithReplay` replays then follows.
- `trimTopic` and `clearTopic` are no-ops that rely on stream retention (`max_age_ms`), and say so; per-record deletes do not exist.
- This unlocks Mastra's evented workflows and durable agents' resumable streams on Loams without Redis.

### 8.6 `LoamsCache` (D1219)

- Values in one collection (`mastra_cache`), keyed by the cache key, with `expiresAt`; a read past `expiresAt` is a miss, and a sweep deletes expired keys through `deleteByFilter`.
- Lists as documents `<key>#<index>`; `listFromTo` scrolls by id prefix.
- `increment` and `listPushIndexed` are atomic **within one process** (a mutex) but not across processes, because documents have no compare-and-swap yet. Multi-instance Mastra servers that rely on cache counters are told to keep the in-memory or Redis cache until conditional writes exist (Q1003).

### 8.7 `LoamsExporter` (D1220)

- `class LoamsExporter extends BaseExporter`: `span_started`, `span_updated` and `span_ended` events, plus scores and feedback, become CloudEvents (`io.loams.dev.mastra.span.v1` and siblings) produced to a stream (`mastra_traces` by default), batched and flushed on `flush()` and `shutdown()`.
- A House table fed by `ENGINE = LoamsStream(...)` (D769) materialises the spans; the `LoamsStore` observability domain reads from it for Studio once House's query path is on the typed client (verify).
- It is not usage metering: tenants cannot emit `usage.*` events (D1133).
- OTLP traces ingest is **not** needed for this; Mastra's `OtelExporter` pointed at Loams stays unsupported until Loams has OTLP traces (D73 covers logs only).

## 9. Phase 3 (MA3): Loams Memory and MCP (D1221, D1222)

- **Memory, Zep's shape** (R-PRIOR §4), in `@loams/mastra/memory`:
  - `createLoamsMemoryProcessors({ store, space })`: an **input processor** that calls `MemoryService.Search` and injects a framed context block (profile, facts, related entities), and an **output processor** that calls `Add` with the turn (extraction is asynchronous on the server), with `addMemory: 'always' | 'never'`.
  - `createLoamsMemoryTools()`: `loamsRemember`, `loamsRecall` (hybrid + graph expansion), `loamsContext`, and the code-graph tools of §52 §8 when a code space is mounted.
  - Identity from Mastra's `requestContext`: `resourceId` → the user space, `threadId` → the session space (§52 §4).
  - Mastra keeps threads and messages (on `@mastra/pg` or `LoamsStore`). Observational memory is ingested as episodes, not replaced.
  - This **replaces MM1 Task 50's location** (`adapters/mastra/` → `@loams/memory-mastra`) and amends D886 for Mastra. MM1 Task 50 points here.
  - Zep's warning applies: processors write into a privileged system message, so recalled content from untrusted sources goes through tools, not the input processor, by default.
- **MCP:** `loams-mcp` (#847) needs no Mastra code. MA3 adds an example and a test that list and call its tools through `@mastra/mcp`'s `MCPClient` over streamable HTTP, with an `lk_` key.
- **LongMemEval:** once MM1a's MVP gate passes, run `@mastra/longmemeval` with Loams Memory, observational memory on and off, and publish the numbers and the harness (Q1010).

## 10. Versioning against Mastra's cadence (D1223)

| Item | Rule |
|---|---|
| Peer range | `@mastra/core` `>=<floor> <2.0.0`. The floor moves only when the package starts to use a newer core API, in a minor release of ours with a changelog line |
| Pinned set | `test/conformance/pins.json` names `@mastra/core`, `@mastra/pg`, `@mastra/qdrant`, `@mastra/elasticsearch`, `@mastra/memory` and the Loams release; bumped weekly by a PR from Dependabot or the nightly job, after the `latest` cell is green for 3 days |
| Matrix | Mastra {pinned, `latest`, `alpha`} × Loams {pinned release, next candidate}; pinned × pinned is required on every PR, the rest nightly |
| Breakage | A nightly failure opens or updates one `mastra-compat` issue with the failing tests and versions; a fix ships in a patch release within 7 days for `latest` (best effort, Q1008); `alpha` failures are watched, not promised |
| `COMPATIBILITY.md` | Generated by `scripts/gen-compat.mjs` from the last green results; README links it |
| Our versions | `@loams/mastra` stays `0.x` until Loams 1.0 (Q1013 covers naming); changesets for releases |
| Upstream engine | Once `@mastra/resonate` is released, Mastra's cadence applies to it and `@loams/mastra-resonate` is deprecated |

## 11. Security (D1208, D1224)

### 11.1 Keys and exposure

- A Mastra server uses `LOAMS_ENDPOINT`, `LOAMS_API_KEY` (an **environment-scoped** `lk_` service-account key from `KeyService/CreateKey`) and `LOAMS_NAMESPACE` (`<project>/<environment>`). The key reaches only its namespace (`namespace_reach.rs`). With the Qdrant adapter the key travels in the `api-key` header and the namespace in `Loams-Namespace`.
- Postgres uses its own role and password from pg-control's connection string (`DATABASE_URL`), not an `lk_` key.
- Keys never appear in client bundles, query strings (refused by the engine), logs or test snapshots. Examples ship `.env.example` only. CI needs no secrets for conformance (loopback, auth off) except the auth cell, which mints a throwaway key with `loams auth new-key`.
- The engine is never public: every example and template runs against `loams dev` on loopback or a compose deployment over a tailnet (Tailscale or Headscale). The upstream docs page says so.
- Durable needs an operator or org key on shared deployments (§7.5).

### 11.2 Supply chain

The 2026-06-17 compromise published about 140 `@mastra/*` versions with an install-time infostealer dependency (`easy-day-js`) through a hijacked contributor account (R-SURF §8). Our controls:
- **Publishing:** npm trusted publishing (OIDC) with provenance from one workflow on GitHub-hosted runners, in a protected `npm` environment with required reviewers; no long-lived npm tokens anywhere.
- **Installing:** `pnpm install --frozen-lockfile`; dependency lifecycle scripts blocked except an explicit allowlist; `minimumReleaseAge` of 3 days for third-party packages (the conformance `alpha` cell is exempt by design and runs without secrets); `npm audit signatures` in CI.
- **Pins:** exact versions of every `@mastra/*` package in examples, templates and the conformance pins; ranges only in `@loams/mastra`'s peer field.
- **Workflows:** actions pinned by commit SHA, `permissions: read` by default, no `pull_request_target`, Dependabot with a cooldown.
- **Licences:** a CI check refuses GPL, AGPL, BSL, SSPL, Elastic-2.0 and the Mastra EE licence in the dependency tree and in vendored files.

## 12. Adoption plan (D1225)

| Item | What | When |
|---|---|---|
| Upstream docs page | §5.5 | After MA0's gate is green |
| Template | `templates/loams-starter` (agent with memory, RAG and a workflow on Loams), launched by `npx create-mastra@<pinned> --template <url>`; mirrored to `ostrium-labs/loams-mastra-template` if `create-mastra` cannot take a subdirectory (Q1000) | MA0 |
| Example app | `examples/support-agent`: memory across threads, hybrid RAG over docs, a durable workflow on Resonate, traces in House | MA0 (pg, Qdrant), extended in MAR and MA2 |
| Durable engine upstream | `@mastra/resonate` and its docs page | MAR |
| loams.dev | A Mastra page in Learn and a blog post per milestone (`ostrium-labs/loams-site`) | MA0 exit, MAR exit, MA2 exit |
| Compatibility matrix | `COMPATIBILITY.md` and a README badge | From MA0 |
| Provenance | Every published version | From the first publish |
| LongMemEval | Through `@mastra/longmemeval`, OM on and off | After MM1a |
| Agent-facing setup | `llms.txt` and a skill for coding agents in the template, as Neon does | MA0 |
| Neighbours | The same gate pattern for LangGraph `checkpoint-postgres` and LangChain.js on the Qdrant API, in their own repositories later | After MA2 |
| Business development | Mastra platform DB provider and co-marketing, only with usage to show (Q1007) | After MA2 |

## 13. What not to build (D1226)

- **No upstream `@mastra/loams` storage or vector package** before Loams 1.0 (Q1005). The upstream contributions are the docs page and the generic Resonate engine.
- **No `MastraMemory` subclass** and no Memory storage-domain adapter for the graph.
- **No copy or import of any `ee/` path**, no Elastic-2.0 code, no Mastra feature that needs a Kepler key.
- **No Mastra deployer, workspace filesystem or sandbox** now.
- **No `LeaseProvider`** until documents have compare-and-swap.
- **No OpenSearch `knn_vector` dialect** in the Elasticsearch gateway.
- **No OTLP traces ingest built for Mastra**; the exporter goes to Streams.
- **No Temporal-style build-time transform** for the durable engine.
- **No telemetry** in `@loams/mastra`.
- **No public engine endpoint** in any example, template or doc.

## 14. Risks

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R1 | Mastra's 2–4-day releases break us faster than we fix | High | Medium | Nightly `latest`/`alpha` cells, one issue per break, a 7-day patch target, ranged peer |
| R2 | Mastra rejects or stalls the docs page or `@mastra/resonate` | Medium | Medium | Issue-first; our package and our docs work without upstream; Q1013 keeps ours permanently if needed |
| R3 | Loams Postgres differs from vanilla pgvector Postgres in ways the suite exposes (extension version, pooler semantics) | Medium | High | §5.3's named checks; fixes in the fork's manifest or pg-control |
| R4 | The vendored test suites depend on internals that change with every tag | High | Low | Vendor per tag with a script; only the pinned copy is committed |
| R5 | Another `@mastra/*` compromise reaches our users through a template | Medium | High | Exact pins, release age, blocked scripts, signature audit (§11.2) |
| R6 | `ee/` creep into interfaces we use (for example storage or auth hooks) | Low | High | The licence check fails on any `ee/` path; watch `LICENSE.md` and `CODEOWNERS` changes in the nightly vendoring diff |
| R7 | Durable's instance-wide scope blocks multi-tenant use of the Resonate engine on Loams | High | Medium | Single-tenant first (Q1012); D142 for multi-tenant |
| R8 | The Resonate SDK's 0.x API changes under the engine | Medium | Medium | Pin 0.11.5; the engine's tests run against the pinned server and SDK |
| R9 | `LoamsStore` without CAS loses a racing working-memory or snapshot write | Medium | Medium | Capabilities declared false; compose `@mastra/pg` for fencing (Q1003) |
| R10 | `@loams/client` changes after #884 under MA2 | Medium | Low | One exact minor range; MA2 starts after the release |

## 15. Decisions (D1202–D1227, approved by the owner on 2026-10-20)

D1202–D1209 and D1223–D1225 record the lead's defaults; D1211–D1213 record the owner's 2026-10-20 change for the durable engine. All were approved with the rest.

| ID | Decision |
|---|---|
| D1202 | **Our own package:** publish `@loams/mastra` from the public repository `ostrium-labs/loams-mastra` (Apache-2.0), not an upstream `@mastra/loams`; send Mastra a docs-only page for its integrations list through its issue-first process, as Neon and ClickHouse did |
| D1203 | **Licence boundary:** never copy or depend on `ee/` code (Mastra Enterprise Edition License) or Elastic-2.0 code (`@mastra/connect` providers); Mastra's private `@internal/storage-test-utils` and other Apache-2.0 test code may be vendored with per-file provenance headers and a `NOTICE` entry; the vendoring script refuses `ee/` paths |
| D1204 | **Phase order:** MA0 (zero-package compatibility) and MAR (the Resonate engine) in parallel now; MA1 (Loams API fixes) alongside; MA2 (native `@loams/mastra`) after #884; MA3 (Memory processors and tools, `loams-mcp`) after MM1a and #847; adoption work throughout. The former Phase 4 is MA3 |
| D1205 | **Zero-package first:** unchanged `@mastra/pg` (`PostgresStore`, `PgVector`) on Loams Postgres and unchanged `@mastra/qdrant` on the Loams Qdrant gateway are the first supported integration, with a three-block quickstart |
| D1206 | **The conformance gate:** Mastra's own pg and Qdrant adapter suites, vendored at each adapter's release tag, run against `loams dev` and Loams Postgres in `loams-mastra` CI, with expected-failure lists that fail on an unexpected failure or an unexpected pass; pinned versions on every PR, Mastra `latest` and `alpha` nightly; Elasticsearch as an observed cell |
| D1207 | **Loams Postgres backs `@mastra/pg`;** the gate verifies pgvector ≥ 0.7, `CREATE EXTENSION` as the app role, advisory locks and `SKIP LOCKED` through PgDog in transaction mode, the observability partition mode (`native` acceptable), `disableInit`, split pools and suspended-compute wake-up (§5.3) |
| D1208 | **Auth and exposure:** a Mastra server authenticates with an environment-scoped `lk_` key plus `LOAMS_NAMESPACE` (and `LOAMS_ENDPOINT`); the Qdrant adapter sends `api-key` and `Loams-Namespace`; Postgres uses pg-control's role connection string; the engine is never public, so quickstarts use local `loams dev` or compose over a tailnet |
| D1209 | **MA1 fixes in the monorepo,** one PR each into `dev`: Qdrant `DELETE` payload index, Elasticsearch `GET /_cat/indices`, `@qdrant/js-client-rest` 1.18.0 and 1.19.0 in the `qdrant-client` CI job, one PR per further gate finding, and a nightly Mastra gate on `dev` builds (Q1001) |
| D1210 | **Qdrant payload-index delete retires the index:** the key is recorded in the `qdrant.payload_index.retired` annotation, left out of `payload_schema`, filters keep working (D134), re-creating with the same type revives it, a different type stays 501; added to the divergence list |
| D1211 | **The Resonate engine goes upstream:** `@mastra/resonate` in Mastra's `workflows/resonate`, next to Inngest, through the issue-first process; developed in `loams-mastra/packages/resonate` in Mastra's layout and published from our repository as `@loams/mastra-resonate` until Mastra releases it, then deprecated with a pointer; it runs in parallel with MA0 (owner, 2026-10-20) |
| D1212 | **Engine design:** `ResonateExecutionEngine extends DefaultExecutionEngine` (Inngest's runtime wrapper, not Temporal's build-time transform); `ctx.run` per durable operation with deterministic ids, `ctx.sleep` for sleeps, Mastra's `retryConfig` owns retries; `init(resonate)` returns `createWorkflow`/`createStep`; snapshots stay in the `workflows` storage domain; generic Resonate, no Loams code; MVP first, then suspend/resume as durable promises, then streaming |
| D1213 | **Engine tests:** Mastra's `workflows/_test-utils` vendored; CI against a pinned upstream Resonate server and Loams Durable on `loams dev`; a `kill -9` resume test on Loams; operation-id determinism tests |
| D1214 | **Package shape:** one `@loams/mastra`, ESM, Node ≥ 22.13, subpath exports `vector`, `storage`, `pubsub`, `cache`, `observability`, `memory`; depends on `@loams/client`; peer `@mastra/core` `>=<floor> <2.0.0`; the Resonate engine stays a separate package |
| D1215 | **MA2 is written only against the typed `@loams/client`** of #884 after it is released; no untyped `loams.modules[...]` calls; client gaps fixed in the monorepo first |
| D1216 | **`LoamsVector`:** a collection per index with `embedding`, `metadata` (`json`) and `text` (BM25 from `metadata.text` or `documents`); hybrid vector + BM25 with RRF when `queryText` is given; a Mongo-style → `Query` IR translator that refuses `$elemMatch` and unanchored `$regex`; `isSelfEmbedding` false until collections embed server side |
| D1217 | **`LoamsStore`:** `memory`, `workflows`, `scores` and a minimal `observability` domain on collections; threads, messages and resources in one collection per store with a `kind` discriminator for atomic saves; UUIDv7 ids; sorted search or `sql.query` for listings; `prune()` → `deleteByFilter`; run fencing and concurrent updates declared unsupported; other domains composed from `@mastra/pg` |
| D1218 | **`LoamsPubSub`** on Streams: a stream per topic family keyed by `runId`, CloudEvents, idempotent producers, pull by long-poll fetch, offsets and replay supported, trimming by retention only |
| D1219 | **`LoamsCache`:** values and lists in a collection with `expiresAt` and a sweep; `increment` and `listPushIndexed` atomic per process only until documents have compare-and-swap, and documented as such |
| D1220 | **`LoamsExporter`:** span, score and feedback events as CloudEvents to a Loams stream, materialised in House by a `LoamsStream` pipe; not usage metering; no OTLP traces ingest built for it |
| D1221 | **Loams Memory for Mastra** ships as input and output processors plus tools in `@loams/mastra/memory` (Zep's shape), with Mastra keeping threads and messages and observational memory ingested as episodes; it replaces MM1 Task 50's `adapters/mastra/` (`@loams/memory-mastra`) and **amends D886** for Mastra |
| D1222 | **MCP:** no Mastra-specific code for `loams-mcp`; an example and a test through `@mastra/mcp`'s `MCPClient` when #847 ships |
| D1223 | **Versioning:** a pinned set bumped weekly after 3 green `latest` nights; the matrix Mastra {pinned, latest, alpha} × Loams {release, candidate}; one `mastra-compat` issue per break with a 7-day patch target for `latest`; `COMPATIBILITY.md` generated from CI; `0.x` until Loams 1.0 |
| D1224 | **Supply chain:** npm trusted publishing with provenance from GitHub-hosted runners only, no npm tokens; frozen lockfiles, blocked lifecycle scripts with an allowlist, a 3-day minimum release age for third-party packages, `npm audit signatures`; exact `@mastra/*` pins in examples, templates and conformance; actions pinned by SHA; a licence check refusing GPL, AGPL, BSL, SSPL, Elastic-2.0 and the Mastra EE licence |
| D1225 | **Adoption:** a `create-mastra --template` starter, an example app, the upstream docs page, a loams.dev page and posts, the compatibility matrix and provenance from the first publish, LongMemEval through `@mastra/longmemeval` once Memory exists; business development only with usage to show |
| D1226 | **Not built:** an upstream storage or vector package before Loams 1.0, a `MastraMemory` subclass, any `ee/` dependency, a deployer, workspace filesystem or sandbox, a `LeaseProvider` without compare-and-swap, an OpenSearch dialect, OTLP traces for Mastra, a build-time durable transform, telemetry, public endpoints |
| D1227 | **Plan MA1** specifies MA0, MAR and MA1 task by task, and MA2 and MA3 at task level; MA2 and MA3 get full task specifications when their gates (#884 released; MM1a and #847) are met |

## 16. Open questions (Q1000–Q1014)

| ID | Question | Default (all answered by the owner on 2026-10-20: the default) |
|---|---|---|
| Q1000 | The template's home: `templates/loams-starter` in `loams-mastra` only, or mirrored to a template repository? | Keep the source in `templates/`; mirror to `ostrium-labs/loams-mastra-template` by a workflow if `create-mastra --template` cannot take a subdirectory URL (checked in MA0) |
| Q1001 | Should Loams' own CI run the Mastra gate, so a Loams change cannot break Mastra silently? | Yes: a nightly job in the monorepo's `nightly.yml` that runs `loams-mastra`'s pinned gate against a `dev` build; not a required PR check |
| Q1002 | Design collection-level server-side embedding (embed on write and on query through `loams-llm`) so `LoamsVector.isSelfEmbedding` can be true? | Not now; wait for MM1 Task 2 (`loams-llm`, #843) and Memory's embed path; `LoamsVector` embeds client side meanwhile |
| Q1003 | Pull D89's conditional writes forward for run fencing, atomic working-memory merges and cache counters? | No for MA2; capabilities declared false and `@mastra/pg` composed for fencing; revisit when D89 is scheduled |
| Q1004 | Mastra's observability domain on Loams: native on collections, `@mastra/clickhouse` on House's ClickHouse surface, or the exporter only? | The exporter to Streams and House plus a minimal native domain for Studio's trace views in MA2; `@mastra/clickhouse` on House as an experiment after House's 8123 surface covers `ReplacingMergeTree`, `FINAL`, `TTL` and `JSON` |
| Q1005 | Contribute more upstream later (a `stores/loams` package)? | Not before Loams 1.0; reconsider if Mastra asks or `@loams/mastra` passes 10k downloads a month |
| Q1006 | `loams-mastra`'s branch model: `main` only, or `dev` and `main` like the monorepo? | `main` only, protected, squash merges with DCO sign-off; releases cut from `main` by changesets |
| Q1007 | Approach Mastra about Loams as a Mastra platform database provider (as Neon and Turso are) and co-marketing? | After MA2 ships and the template has users; the owner decides |
| Q1008 | A compatibility promise for `@loams/mastra` against Mastra `latest`? | Best effort, a patch within 7 days; a contractual window only as part of Ostrium's support offering |
| Q1009 | Make the Elasticsearch suite a required cell after `_cat/indices` lands? | No; it stays observed; Qdrant and pg are the supported compatibility paths, and ES gaps are fixed only when cheap |
| Q1010 | Who pays for LongMemEval's model tokens, and does it run in CI? | The owner approves a budget before the first run; runs are manual and published with their harness, never in CI |
| Q1011 | The quickstart's default variant? | Variant A, everything on Loams Postgres (one connection string); the Qdrant variant second |
| Q1012 | Ship the Resonate engine on Loams for multi-tenant deployments before per-namespace Durable (D142)? | No; single-tenant self-hosted with an operator or org key is documented; multi-tenant waits for D142 |
| Q1013 | If Mastra declines `@mastra/resonate`, keep `@loams/mastra-resonate` permanently? | Yes, maintained in `loams-mastra`, still generic Resonate, listed on loams.dev and Resonate's community pages |
| Q1014 | Involve Resonate's maintainers in the upstream engine? | Notify them in the Mastra issue and invite review; no dependency on their approval |

## 17. Effort (estimate, engineer-weeks)

| Phase | Effort | Gate to start |
|---|---|---|
| MA0 | 2–3 | Now |
| MAR | 3–5 (MVP 2–3; suspend/resume and streaming 1–2), plus upstream review time | Now |
| MA1 | 1–2 | Now (the gate-finding task after MA0's first run) |
| MA2 | 6–8 | #884 merged and `@loams/client` released |
| MA3 | 3–5 | MM1a's MVP gate; #847 for MCP |
| Adoption | 0.5 a week, ongoing | — |

## 18. Sources

Research reports of 2026-10-20 (the session's scratchpad, not committed): R-SURF (`01-mastra-surfaces.md`), R-CAP (`02-loams-capabilities.md`), R-PRIOR (`03-prior-art.md`). Their primary sources, as cited there:

- Mastra at commit `5758ba045b01c407a7414b32dbbdf5480596b31d`: [`LICENSE.md`](https://github.com/mastra-ai/mastra/blob/main/LICENSE.md), [`ee/LICENSE`](https://github.com/mastra-ai/mastra/blob/main/ee/LICENSE), [`packages/connect/LICENSE-ELv2.txt`](https://github.com/mastra-ai/mastra/blob/main/packages/connect/LICENSE-ELv2.txt), [`CONTRIBUTING.md`](https://github.com/mastra-ai/mastra/blob/main/CONTRIBUTING.md); `packages/core/src/{storage/base.ts, storage/domains/*/base.ts, vector/vector.ts, vector/types.ts, vector/filter/base.ts, memory/memory.ts, workflows/execution-engine.ts, workflows/default.ts, events/pubsub.ts, cache/base.ts, observability/types/core.ts, deployer/index.ts}`; `stores/{pg,qdrant,elasticsearch,_test-utils}`; `workflows/{inngest,temporal}`; `observability/mastra/src/exporters/base.ts`; the docs pages [`integrations/databases/neon.mdx`](https://github.com/mastra-ai/mastra/blob/main/docs/src/content/en/integrations/databases/neon.mdx), [`integrations/deploy/inngest.mdx`](https://github.com/mastra-ai/mastra/blob/main/docs/src/content/en/integrations/deploy/inngest.mdx), `integrations/sidebars.js`. Release tags `@mastra/pg@1.31.2`, `@mastra/qdrant@1.1.4`, `@mastra/core@1.78.0` (GitHub API, 2026-10-20).
- Mastra PRs: Neon deploy guide [#22486](https://github.com/mastra-ai/mastra/pull/22486); ClickHouse Managed Postgres [#25733](https://github.com/mastra-ai/mastra/pull/25733); Apache-2.0 [#5765](https://github.com/mastra-ai/mastra/pull/5765); EE licence [#24708](https://github.com/mastra-ai/mastra/pull/24708); Temporal [#15789](https://github.com/mastra-ai/mastra/pull/15789); integrations removal [#7420](https://github.com/mastra-ai/mastra/pull/7420).
- npm registry and downloads API, 2026-10-20 (`@mastra/*`, `@qdrant/js-client-rest` 1.19.0, `@resonatehq/sdk` 0.11.5).
- The 2026-06-17 compromise: [Socket](https://www.socket.dev/blog/mastra-npm-packages-compromised), [The Hacker News](https://thehackernews.com/2026/06/144-mastra-npm-packages-compromised-via.html), [Orca](https://orca.security/resources/blog/mastra-npm-supply-chain-attack).
- Memory vendors: [Zep for Mastra](https://help.getzep.com/mastra-memory), [Supermemory for Mastra](https://supermemory.ai/docs/integrations/mastra.md), [Neo4j Agent Memory](https://neo4j.com/labs/agent-memory/how-to/typescript/mastra/), [mem0 cookbook](https://docs.mem0.ai/cookbooks/integrations/mastra-agent); [Mastra observational memory research](https://mastra.ai/research/observational-memory).
- Durable: [Mastra deploy/inngest](https://mastra.ai/integrations/deploy/inngest), [deploy/temporal](https://mastra.ai/integrations/deploy/temporal), [durable agents](https://mastra.ai/blog/introducing-durable-agents); [Resonate TS SDK](https://github.com/resonatehq/resonate-sdk-ts).
- Neighbours: [LangChain.js CONTRIBUTING](https://github.com/langchain-ai/langchainjs/blob/main/CONTRIBUTING.md), [Vercel AI SDK memory docs](https://github.com/vercel/ai/blob/main/content/docs/03-agents/06-memory.mdx), [LlamaIndexTS (archived)](https://github.com/run-llama/LlamaIndexTS).
- Company: [Techleap on Mastra's Series A](https://finder.techleap.nl/news/feed/mastra-raises-22m-series-a-to-help-developers-build-ai-agents) (approximate).
- Loams at `dev` (`77692a45f`, rechecked at `d8bda64b2` on 2026-10-20): `crates/loams-qdrant/src/{rest.rs,schema.rs,filter.rs,lib.rs}`, `crates/loams-es/src/lib.rs`, `.github/workflows/ci.yml` (`qdrant-client`, `es-client`), `conformance/es-client/`, `deploy/loams-postgres-dev/`, `deploy/pins/loams-postgres-images.toml`, `sdks/typescript/packages/client/src/{loams.ts,gen/facade.ts}`, `proto/loams/{collection,stream,memory}/v1/*.proto`, `crates/loams/src/auth/`, `docs/engine-quickstart.md`; the v0.1.0 release assets (`loams_0.1.0_amd64.deb`, `loams-compose-0.1.0.tar.gz`, `SHA256SUMS`).
