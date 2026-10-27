# 74 — Durable Execution Compatibility: Temporal Protocol, DBOS and Apache Airflow on Resonate and Loams Postgres

> **Amendment (owner, 2026-10-24) by D1590:** DX1 (the Temporal gateway MVP) starts now, in parallel with DX0 and the fork work, instead of after DX0's exit (owner: "start DX1 temporal gateway too, run in parallel"). It amends D1534's ordering for DX1 and the DX1 gate in §14. Until FW3 and FW7 land, the gateway reaches Resonate through its own port with an interim adapter (D1591, proposed). [DX1](../plans/2026-10-21-dx1-durable-execution-compat.md) now specifies DX1 task by task.
>
> **Amendment (approved 2026-10-21) by [§76](76-identity-and-authorization.md): superseded in part by D1600** (owner: full cutover from Authentik to Zitadel, with no migration path, no compatibility period and no dual running, because there are no users). D1507's "Authentik-issued OIDC tokens" are Zitadel-issued, and Q1158's Airflow FAB OAuth signs in against Zitadel.

Status: **Approved** · 2026-10-21 (written and approved the same day; the owner approved every decision and question with its default, including the amendments to §68, except Q1165, which stays the owner's). Source: the owner's draft of 2026-10-21, "Loam Durable Execution: Temporal Protocol Compat + DBOS + Apache Airflow" (rev 2), with the owner's notes: **"very important for onboarding ResonateHQ as our design partner"**, **"do all TiKV and this work in our fork"**, and **"large companies like Stripe use their internal Temporal, so Temporal compat is large for ResonateHQ cloud; same for Airflow"**. Decisions **D1500–D1539** and questions **Q1150–Q1165** are in the [decision log](13-decision-log.md). The numbers are chosen to stay clear of open work: #973 (D1340–D1343), #979 (D1350–D1410, Q1080–Q1109) and the Software Factory and Agent Marketplace designs being written now (D1420–D1479, Q1110–Q1139). Plan stub: [DX1](../plans/2026-10-21-dx1-durable-execution-compat.md), track **DX**. **This document writes no code.**

**It amends [§68](68-open-protocol-compute.md)** (approved 2026-10-21). §68 ruled out a Temporal-compatible frontend and gated hosted Temporal and managed Airflow on demand. The owner now wants a Temporal-protocol frontend backed by Resonate, DBOS on Loams Postgres, and Airflow with a Resonate-backed executor. §17 names each §68 decision this changes. Every amendment is marked **owner direction, 2026-10-21**. §68's findings still hold, and this document reuses them: Temporal's SQL plugins take shared read locks that TiDB parses but ignores, and `airflow-provider-loams` lives in `ostrium-labs/loams-airflow` (Q1031, approved).

Status of the rows. The owner approved every decision (D1500–D1539) and every question's default (Q1150–Q1164) on 2026-10-21, including the amendments to §68 in §17. Q1165, the design-partner terms with ResonateHQ, stays open for the owner.

Builds on:
- [§21](21-durable-execution.md) and [§14](14-durable-execution.md): Loams Durable. The Resonate server is embedded in `loams` (D138–D147). Its native TiKV store is D261 (PR #796). The light 0.1.1 compose (PR #973) keeps Durable on SQLite by default and makes TiKV opt-in.
- [§68](68-open-protocol-compute.md): the protocol ladder (D1251) and OC0's conformance harness (`conformance/compute/`, D1253). §68 also holds the Airflow findings (§4) and the Temporal findings (§5).
- [§57](57-loams-gateway.md): `loams-gateway` (D1010–D1024), its core, modules and observability rule (D1021). [§69](69-edge-reach.md) adds edge reach.
- [§46](46-loams-postgres-production.md), [§51](51-loams-postgres-fork.md), [§62](62-postgres-major-versions.md): Loams Postgres. It is the hard fork of Neon, fronted by unmodified PgDog (D708, D318), with `loams-wal` as its WAL (D714). It runs majors 16 and 17.
- [§58](58-data-access-and-htap.md): Postgres wire first (D1030).
- [§38](38-knative-authentik-gitops.md) §4 and [§19](19-console-identity-and-agents.md): identity. Authentik's open-source edition is the default IdP (D447). Loams principals, `lk_` API keys and OIDC tokens. **There is no Keycloak in Loams.** The draft's Keycloak mentions are replaced throughout.
- [§63](63-loams-meter.md): the Meter (CloudEvents usage events, enforcement). [§49](49-loams-house-production.md): the House on chDB over Lakekeeper's Iceberg tables.
- [§02](02-stream-engine.md) §7.4 and [§34](34-protocol-gateway-and-standards.md): CloudEvents on every stream (D270, D364).
- [§31](31-loams-router-and-verification.md), [§55](55-commit-path-determinism.md) and `spec/`: the TLA+, Lean and simulation practice (TLC and Apalache in CI job `tla`, Lean in CI job `lean`).
- [§56](56-loams-sandbox.md): `loams-sandbox`, where the Airflow worker agent runs task processes.

Markers, as in §64–§69:
- **(verify)** means the claim has not been checked against a primary source. The task that depends on it checks it first.
- **(estimate)** means the number was judged, not measured.
- **(unverified)** means a search on 2026-10-21 did not settle it.

---

## 1. Summary

### 1.1 The owner's direction and how this document reads it

The owner's draft adds three ways to run durable work on Loams. Native Resonate stays as it is.

| Surface | What the user runs | What Loams provides | Effort (draft) |
|---|---|---|---|
| **Resonate (native)** | Resonate SDKs | The embedded Resonate server, from the fork `ostrium-labs/resonate` (D138, D140) | Done; ongoing |
| **Temporal protocol** | Official Temporal SDKs and CLI, unchanged (Go, TypeScript, Java, Python, .NET, Ruby, Rust) | A `WorkflowService` gRPC frontend in `loams-gateway`. It owns a Temporal-shaped history in Loams Postgres, and Resonate is the durability layer underneath | High: a new protocol surface |
| **DBOS** | DBOS Transact in the application process | A Loams Postgres that behaves exactly like Postgres, with the right endpoint mode | Low: conformance, endpoint and operations guidance |
| **Apache Airflow 3.3** | Real Airflow with Python Dags (and later Go or Java tasks) | The metadata DB on Loams Postgres, a Resonate-backed executor, and `airflow-provider-loams` | Low to medium |

**Why it matters (owner).** Large companies run Temporal, often internally. Stripe is the owner's example. Airflow is the same. A Temporal-protocol surface backed by Resonate lets those teams move existing workers without rewriting them. ResonateHQ's own hosted offering, Resonate Cloud (§4.5), could then serve Temporal users too. That is why ResonateHQ is the design partner, and why the Resonate-side work lives in our Resonate fork in a shape ResonateHQ can upstream (§7).

### 1.2 The architecture in one paragraph

**Temporal history is the source of truth, not Resonate** (D1501). Temporal workers rebuild state by replaying history. So the gateway owns an exact, append-only, Temporal-shaped event log, plus each run's mutable state, in **its own database on Loams Postgres** (D1502). Every state change is one Postgres transaction: a compare-and-set on the run's record version, the new history events, and outbox rows. A relay drains the outbox into **Resonate**, which gives durable timers, timeouts, leases, retries and wake-ups. Each outbox row creates a Resonate promise with a **deterministic ID** derived from namespace, workflow, run and event ID, so a retried row is idempotent (D1503). Resonate callbacks come back in through the same compare-and-set path, and stale ones are dropped. If Resonate's state is lost, the gateway rebuilds it from history. **DBOS** needs no protocol work. It needs a Loams Postgres that passes DBOS's own suites, behind an endpoint that keeps session features (D1519). **Airflow** runs as real Airflow. Its executor turns each task into a Resonate-leased job that a small Rust worker agent runs (D1525, D1526). **Each engine has its own database, never a shared schema.** All of them share the control plane: tenants, identity, audit, metering, metrics and the console (D1529).

### 1.3 Top recommendations

1. **History is owned by the gateway, in Loams Postgres, with a transactional outbox to Resonate and deterministic promise IDs** (owner draft; D1501–D1503).
2. **Phase 0 runs the baselines first** (owner draft; D1534): the upstream Temporal server on Loams Postgres, DBOS conformance, a corpus of real Temporal histories, and Airflow 3.3 with the Edge executor on Loams Postgres. They check the Postgres fork, give the differential tests their oracle, and are a shippable fallback ("compat mode", D1509).
3. **Verification is the product** (owner draft; D1510). The checks are differential tests against a real Temporal server, the SDKs' own replayers, porcupine, TLA+ and deterministic simulation. The key oracle is each SDK's replayer, fed histories recorded from a real server.
4. **All Resonate-side work, the TiKV store included, goes to `ostrium-labs/resonate`** as upstreamable branches, one per change, each offered to resonatehq (owner note; D1513–D1517). The Loams monorepo keeps the embedding, the gateway crates, Postgres conformance and the worker agent. The Airflow provider stays in `ostrium-labs/loams-airflow`.
5. **Don't reimplement Airflow; don't route DBOS through Resonate** (owner draft; D1523, D1519).
6. **Tell users which engine to use** (§10; D1530).

### 1.4 What changes

| Before (§68, §21) | After (§74) |
|---|---|
| No Temporal-compatible frontend, ever (D1259, D1272) | A Temporal-protocol frontend in `loams-gateway`, backed by Resonate and Loams Postgres, phased and checked against real Temporal (D1500, D1504) |
| Hosted Temporal on Loams Postgres only after a spike and three users ask (D1259, Q1033) | The upstream Temporal server on Loams Postgres is Phase 0 and runs from the start. It is the **reference oracle** and the **compat-mode fallback** until the gateway passes DX2 (D1509). Managed Temporal as a product still waits for demand (Q1033 stands for L2 hosting) |
| Docs never say "Temporal-compatible" (D1275) | Allowed only for the gateway, which Loams wrote, and only with the per-method, per-SDK matrix linked (D1512) |
| The Airflow executor is `LoamsJobsExecutor` on Loams Jobs, after JB1 (D1257) | `LoamsResonateExecutor` on Resonate directly, in DX1A, with a Rust worker agent. `LoamsJobsExecutor` is deferred (D1525, Q1160) |
| DBOS is not mentioned | DBOS Transact is supported on Loams Postgres through its own suites (D1519–D1522) |
| The Resonate TiKV store lives in the monorepo (`crates/loams-durable/src/tikv.rs`, D261) | It moves into the fork as a `resonate-server-blob` store, upstreamable, keeping the 0.1.0 on-disk format (D1515) |
| The fork is `dina-kar/resonate` (D140) | The fork is `ostrium-labs/resonate` (Cargo already pins it), with a branch model for upstreaming (D1514) |

## 2. Goals and non-goals

### 2.1 Goals

1. **Existing Temporal workers run unchanged.** Pinned SDKs connect to the Loams endpoint with only a host change and an API key. Within the published method set, histories replay in the SDKs' own replayers.
2. **DBOS apps and Airflow deployments run on Loams Postgres** with no code change, proven by their own test suites.
3. **One durability layer.** Timers, leases, retries and wake-ups for Temporal workflows and Airflow tasks come from Resonate. Loams writes no second scheduler.
4. **Upstreamable Resonate work.** Every change to the Resonate server is a branch in the fork, with tests and a spec check, offered upstream to resonatehq.
5. **A measured claim.** A per-method and per-SDK compatibility matrix, generated from CI, is the only statement of Temporal compatibility.

### 2.2 Non-goals

- **No Temporal fork**, no Temporal server code in Loams, and no Temporal on Loams SQL (D1259's finding stands).
- **No Temporal Cloud-only features** in DX0–DX4 (owner draft): no multi-region replication, Nexus endpoints or cloud ops APIs.
- **No Airflow look-alike** (D1256 stands), and no DAG-to-Resonate translator.
- **No DBOS fork or Conductor clone.** DBOS Conductor is proprietary (§4.4) and Loams neither runs nor imitates it.
- **No PgDog fork.** The draft's "router (pgdog fork)" is corrected. PgDog is AGPL-3.0 and runs unmodified as a separate service (D318, D708). Endpoint modes come from the gateway and pg-control, not from patches to PgDog.
- **No shared schema between engines.**

## 3. Where Loams stands today (origin/dev `b281c2dde`, 2026-10-21)

| Piece | State |
|---|---|
| **Resonate server** | A **Rust** server, upstream `resonatehq/resonate` `impl/server/core`, workspace version 0.10.1. It is plugin-based: SQLite, Postgres, MySQL, ScyllaDB, Neo4j and blob servers. It is **embedded** in `loams` by `crates/loams-durable` (D138), as git dependencies on `ostrium-labs/resonate` at `31ffb201` (branch `loam/0.10.1`). It speaks protocol `2026-04-01` on `127.0.0.1:8001`, loopback-only until the unified auth plan (D138, D111). **Nothing is vendored.** Every Resonate crate in `Cargo.lock` comes from the fork, including `resonate-sql`, the SQL layer shared by the SQLite, MySQL and Postgres plugins. The only Resonate-facing code in the monorepo is `crates/loams-durable` (`embed.rs`, `gateway.rs`, `inproc.rs`, `registry.rs`, `runtime.rs`, `config.rs`, `tikv.rs`) |
| **The fork** | `ostrium-labs/resonate`, created 2026-09-27. `main` equals upstream `28dfd01` (2026-09-22). `loam/0.10.1` is upstream plus 7 carries: dependency hygiene, the GCP ID-token feature gate, MySQL errno classification, TiDB support, `reqwest` without default TLS, and two `jsonwebtoken` 10 auth commits. `loam/tikv-dapr` is upstream plus 14 commits: a TiKV and a redb store for the blob server, a store contract test, and Dapr and kind validation scripts. **Five of the carries are open upstream PRs from 2026-09-27 with no review yet:** [#1162](https://github.com/resonatehq/resonate/pull/1162) errnos, [#1163](https://github.com/resonatehq/resonate/pull/1163) TiDB, [#1164](https://github.com/resonatehq/resonate/pull/1164) advisories, [#1167](https://github.com/resonatehq/resonate/pull/1167) GCP ID token, [#1168](https://github.com/resonatehq/resonate/pull/1168) `reqwest` |
| **TiKV store** | **Two diverged implementations.** The monorepo's `crates/loams-durable/src/tikv.rs` (512 lines) is the one that shipped in 0.1.0 (#796). Keys are under `resonate/blob/`, each value is a random 16-byte revision plus the body, and it uses a pessimistic lock, through `loams-tikv` over `ostrium-labs/client-rust` `8126c49`. The fork's `store_tikv.rs` on `loam/tikv-dapr` uses the TSO start timestamp as the version and optimistic transactions, through `dina-kar/client-rust` `1f8962b`. Neither is upstream |
| **Durable stores in compose** | 0.1.0's release stack ran Durable on TiKV (D261's release-stack exception). The light 0.1.1 default (PR #973, open) runs Durable on SQLite, even under the `tikv` profile, unless `LOAMS_DURABLE_STORE` names a TiKV store |
| **Resonate's spec** | `spec/` in the Resonate repository holds a Lean 4 abstract machine with 92 properties (31 proved), a TLA+ model (`spec/tlap`) and a trace checker (`lake exe checktrace`) for recorded server traces. This is the conformance oracle for every fork change (§7.3) |
| **Gateway** | `crates/loams-gateway`: the `pg` module (SNI routing to per-namespace PgDog) is built. `http`, `llm` and `mcp` are later modules on Pingora (D1013, D1022). There is no gRPC server module yet |
| **Loams Postgres** | Neon hard fork, majors 16 and 17. PgDog in transaction-pooling mode is the front door (D708). Wake-on-connect goes through pg-control (D709) |
| **Conformance harness** | §68's `conformance/compute/` is specified in OC1's OC0 (Tasks 0–11) and is **not built**. OC0 Task 6 (Airflow) and Task 7 (the Temporal persistence spike) overlap this document's Phase 0. DX0 reuses them (§14) |

## 4. Facts checked on 2026-10-21

Sources are listed in §20. Each was read on 2026-10-21 through the GitHub API, PyPI or the projects' documentation.

### 4.1 Temporal

- **Licences.** `temporalio/api` (the protos) is MIT, and so are `temporalio/api-go` and the server `temporalio/temporal`. `sdk-core` (Rust) is MIT. §68 §5.1 records the Java SDK as Apache-2.0 and the other SDKs as MIT. Vendoring the protos into Apache-2.0 Loams is allowed, with the MIT notice kept (D1536).
- **Server:** v1.32.1, 2026-10-08. Tested persistence: PostgreSQL 13.18, 14.15, 15.10 and 16.6; MySQL 5.7 and 8.0.19+; Cassandra 3.11, 4.0 and 5.0.4+; SQLite for development only. **PostgreSQL 17 is not listed** (unverified for 17 and 18). The SQL plugins are `postgres12`, `postgres12_pgx`, `mysql8` and `sqlite`.
- **Read locks.** The MySQL plugin's `execution.go` and `shard.go` use `LOCK IN SHARE MODE`. The PostgreSQL plugin uses `FOR SHARE` in the same two places. TiDB parses `LOCK IN SHARE MODE` and ignores it (§68 §5.3). **So Temporal persistence never runs on Loams SQL**, and nor does the gateway's store.
- **SDKs (latest):** Go v1.50.0 (2026-10-15), TypeScript v1.24.0 (2026-09-15), Java v1.40.0 (2026-09-29), Python 1.34.0 (2026-09-30), .NET 1.21.0 (2026-10-20), Ruby v1.10.0 (2026-10-19), Rust `sdk-core` v1.1.0 (2026-10-06). The Rust SDK went GA with v1.0.0 on 2026-09-04, as crates `temporalio-sdk` and `temporalio-client` 1.1.0. Under D1274's 14-day rule, Go 1.50.0, .NET, Ruby and Rust 1.1.0 are still too new to pin today.
- **CLI:** `temporalio/cli` v1.9.1 (2026-09-14, MIT). `temporal server start-dev` runs an in-memory SQLite dev server, with the API on :7233 and the UI on :8233.
- **Protocol size** (§68 §5.1, counted 2026-10-20): 121 `WorkflowService` RPCs, 61 history event types and 18 command types.

### 4.2 DBOS Transact

- **Licence: MIT** for every SDK: `dbos-transact-py` 3.3.0 (2026-10-15), `dbos-transact-ts` v5.3 (2026-10-15), `dbos-transact-golang` v1.7.0 (2026-10-15), `dbos-transact-java` 1.2.0 (2026-10-01), and `dbos-transact-rust`, tag v0.5.0 (2026-09-09, created 2026-08-04).
- **`LISTEN`/`NOTIFY`** is the default but not required. Python has `use_listen_notify` (default true) and polls when it is false. The option "may not be changed after the system database is first created". Whether the Go, TypeScript, Java and Rust SDKs have an equivalent switch is **unverified**. Python's CI also runs on CockroachDB.
- **Postgres versions:** not stated in DBOS's docs (**unverified**). CI uses an unpinned `sibedge/postgres-plv8` image.
- **Conductor** (the management service) is **proprietary**. Self-hosting it for production needs a licence key with call-home checks (terms of December 2025, paraphrased from search results; **verify**). Loams does not depend on it (D1522).

### 4.3 Apache Airflow 3.3

- **Releases:** 3.3.0 (2026-07-06), 3.3.1 (2026-08-12), 3.3.2 (2026-09-17, latest). `apache-airflow-task-sdk` 1.3.2 (2026-09-17). `apache-airflow-providers-edge3` 5.0.0 (2026-10-13, too new to pin under D1274).
- **Metadata DB (3.3.2):** PostgreSQL 14–18; MySQL 8.0, 8.4 and Innovation; SQLite for tests. Python 3.10–3.14.
- **AIPs**, all "Completed" on the Apache wiki:
  - AIP-72, Task Execution Interface (the Task SDK): 3.0.
  - AIP-69, Edge Executor: in the `edge3` provider.
  - AIP-103, Task State Management: 3.3.0.
  - AIP-105, Pluggable Retry Policies: 3.3.0.
  - AIP-108, Language Task SDK and the Language Coordinator Layer: 3.3. It is **experimental**: `@task.stub(queue=…)`, `JavaCoordinator` (JRE 17), and `ExecutableCoordinator` for native binaries, with Go as the reference.
- The draft's "ResumableJobMixin (3.3)" is **unverified**.
- **Executor interface (3.x):** subclass `BaseExecutor`. Work arrives as workloads (`ExecuteTask` with a `TaskInstanceDTO`, `dag_rel_path`, `bundle_info` and `log_path`). `sync`, `execute_async` and `_process_workloads` are mandatory. `terminate`, `try_adopt_task_instances` and `get_task_log` are optional. Workers reach the API server's Execution API with short-lived per-task JWTs (§68 §4.1). **The Execution API is not a declared public interface** (§68 §4.1), so the executor pins Airflow minors (D1525).

### 4.4 ResonateHQ

- **Company:** "Resonate HQ, Inc." (footer of resonatehq.io). The founders and any funding are **unverified** from primary sources.
- **Resonate Cloud** (resonatehq.cloud) is **pre-launch**. Sign-ups were to open on 2026-09-09 for a "founding cohort" of 25 teams, free during the cohort, with no SLA or prices. The page says: "Run on your infra today. Run on ours tomorrow." Whether sign-ups opened is **unverified**.
- **Licences:** Apache-2.0 for `resonate` and the SDKs: TypeScript v0.11.5, Python v0.8.1, Rust 0.6.0, Go 0.1.0 and Java v0.1.1. The NATS and ScyllaDB servers (`resonate-on-nats`, `resonate-on-scylladb`) are BUSL-1.1 per the homepage. **Loams uses neither** (D1271's rule; the `Cargo.toml` comment already forbids `resonate-server-scylladb` and the NATS packages).
- **Server:** rewritten in Rust as a monorepo (PR #1154, 2026-09-22). The latest tag is still v0.9.8 (2026-06-04). There is no v0.10 tag. The old Go server is `resonate-legacy-server`.
- **Backends upstream:** SQLite (default), PostgreSQL and MySQL in the core server. There are also blob, ScyllaDB and Neo4j crates, a one-SQL-file Postgres server (`impl/server/postgres`, Postgres 16+ with `pg_cron`) and a Zig S3 server. **Upstream has no TiDB or TiKV backend.** Ours would be the first (FW1, FW2).
- **The Durable Promises specification** now lives inside the Distributed Async Await spec. Promise states are `pending`, `resolved`, `rejected`, `rejectedCanceled` and `rejectedTimedout`, the last set only by the server. Creation carries idempotency keys and a `strict` flag.
  - **Tasks:** `pending`, `acquired`, `suspended`, `halted`, `fulfilled`, with version-based optimistic concurrency.
  - **Leases:** refreshed by a per-process heartbeat. On expiry the task returns to `pending`.
  - **Schedules:** cron-driven promise factories.
  - **Locks:** there are none; leases and fencing replace them.
  - **The protocol surface** (`spec/spec/01-protocol/types.lean`): `promise.{get, create, settle, register_callback, register_listener, search}`, `schedule.{get, create, delete, search}`, and `task.{get, create, acquire, fence, heartbeat, suspend, fulfill, release, halt, continue, search}`. The server's own steps are promise timeout, task lease timeout, task retry timeout and schedule timeout. Routing is by tags: `resonate:target`, `resonate:timer`, `resonate:delay` and others.

## 5. Temporal protocol through `loams-gateway`

### 5.1 The core constraint

Temporal workflow code runs in the user's worker. A worker polls for a workflow task, receives history, replays it deterministically, and answers with commands. So the server must produce protobuf history that is **exactly** what the SDK expects: event IDs, ordering, batching, attributes and failure shapes. It must treat payloads as opaque bytes, and it must implement Temporal's task state machines. Resonate cannot run workflow code, so it cannot be the history. It is the durability layer underneath.

### 5.2 Responsibility split

| Concern | Owner |
|---|---|
| `WorkflowService` gRPC surface (later `OperatorService`) | Gateway module `temporal` (tonic) |
| History events, event-ID allocation, mutable state, command → event translation | Gateway (`loams-temporal-history`, `-statemachine`) + Loams Postgres |
| Namespace resolution, authentication, authorization, rate limits | Gateway core. Loams principals: `lk_` API keys or OIDC access tokens issued by Authentik (D447), sent by the SDKs as `authorization: Bearer` (their `api_key` option). Optional mTLS. **Not Keycloak** (D1507) |
| Timers, timeouts, retry backoff wake-ups, schedules | Resonate: a promise with `timeoutAt` and the `resonate:timer` tag; Resonate schedules later |
| Claiming work with leases | Resonate: a task with a lease (`task.acquire` with a TTL, `task.heartbeat`) |
| Long-poll matching | Gateway: an in-memory registry for wake-ups, with Resonate as the durable backstop and the authority for each claim (D1505) |
| Visibility (list, count, search attributes) | Gateway projection tables in Loams Postgres |
| Audit, metering, metrics | Control plane (§11) |

### 5.3 Concept mapping onto the Resonate protocol

The exact calls are confirmed against the fork in DX1 Task 0 (**verify**). FW7 (§7.4) makes them a tested profile in the fork.

| Temporal | Resonate (fork protocol `2026-04-01`) | Notes |
|---|---|---|
| Workflow task scheduled | `task.create` on promise `wft:…`. Tag `resonate:target = poll://any@ltq.<shard>.<tq-hash>`. TTL = the workflow task's start-to-close | The poller's claim is `task.acquire`. Completion is `task.fulfill`, which settles the promise |
| Workflow task timed out | Lease expiry returns the task to `pending`, and the gateway sees it redelivered | The gateway appends `WorkflowTaskTimedOut` and a new `WorkflowTaskScheduled`, settles the old promise `rejected` and creates a new one. **It never lets Resonate re-deliver the same attempt** |
| Activity task | `task.create` on promise `act:…:<attempt>`. Lease TTL = start-to-close | `RecordActivityTaskHeartbeat` maps to `task.heartbeat` for the gateway's `pid`. The heartbeat timeout is its own `to:` promise, re-armed by the gateway |
| Schedule-to-start, start-to-close, schedule-to-close, heartbeat timeouts | Each is one promise `to:…:<kind>` with `timeoutAt` and tag `resonate:timer`, plus a listener to the gateway | On `rejectedTimedout` the gateway runs the compare-and-set path. A stale timeout is dropped |
| Activity retry policy | The gateway computes the next attempt and backoff, appends nothing (Temporal records only the last failure), and creates `act:…:<attempt+1>` with tag `resonate:delay` | The retry state lives in mutable state |
| `workflow.sleep` / timer | Promise `tmr:…` with `timeoutAt` = fire time and tag `resonate:timer` | The callback appends `TimerFired`. `CancelTimer` settles the promise `rejectedCanceled` |
| Signal | No Resonate call. The gateway appends `WorkflowExecutionSignaled` and schedules a workflow task if none is outstanding | |
| Query | No Resonate call. A query task goes to a live poller through the in-memory registry, or is answered from a sticky worker later | Queries are never durable in Temporal either |
| Cancel and terminate | Cancel: the gateway appends `WorkflowExecutionCancelRequested` and schedules a workflow task. Terminate: it appends `WorkflowExecutionTerminated` and settles every outstanding promise of the run `rejectedCanceled` | |
| Temporal Schedules | Resonate schedules that create a `start:` promise. The gateway listens and starts the workflow (DX4, Q1161) | |
| Workflow history | The gateway's event log in Loams Postgres. **Never in Resonate** | |

### 5.4 Deterministic promise IDs (D1503)

The owner's draft gives the shapes. This document fixes the grammar:

```
wft:{ns}:{wf}:{run}:{scheduled_event_id}
act:{ns}:{wf}:{run}:{scheduled_event_id}:{attempt}
tmr:{ns}:{wf}:{run}:{started_event_id}
to:{ns}:{wf}:{run}:{event_id}:{kind}           kind ∈ s2s | s2c | sc2c | hb | wft
start:{ns}:{schedule_id}:{nominal_time_unix_ms}
af:{ns}:{dag_id}:{run_id}:{task_id}:{map_index}:{try_number}      (Airflow, §9.3)
```

- `{ns}` is the **Loams namespace ID** (not its name) joined with the Temporal namespace ID (`<loams-ns-id>.<temporal-ns-id>`). IDs never carry tenant names or personal data (D147).
- `{wf}` is the Temporal workflow ID, percent-encoded so that `:` and `%` cannot appear raw. `{run}` is the run ID, a UUID. Event IDs and attempts are decimal.
- **Length:** if the encoded ID exceeds the fork's promise-ID limit (**verify** in FW7), `{wf}` is replaced by `h` plus 32 hex characters of SHA-256 of the raw workflow ID. The full workflow ID goes in the promise tag `loams:wf`. The rule is a pure function, tested both ways.
- **Tags:** every promise carries `loams:ns`, `loams:run` and `loams:kind`. FW5's retention and FW6's indexed search use them.
- **Idempotency:** a relay retry repeats `promise.create` with the same ID and parameters. Resonate answers with the existing promise. A different payload for the same ID is a bug and alerts (`loams_temporal_outbox_conflict_total`).
- **Amended by X1.R13** (plan DX1, 2026-10-25): the IDs above are replaced by FW7's run-as-origin form, `<ns>.<wf>.<run>:<kind>.<event>[.<attempt>]` (a `to` ID ends `.<kind>.<attempt>`) and `<ns>.<schedule_id>.<nominal_time_unix_ms>`, with exactly one `:`, so Resonate's origin is the run, not the kind.

### 5.5 Storage model (Loams Postgres; D1502)

One database, `loams_temporal`, per region's gateway deployment. Namespaces are columns, and rows are partitioned by `shard_id = hash(namespace_id, workflow_id) mod S`. `S` is fixed at creation: default 64, the Temporal `numHistoryShards` analogue (**estimate**, Q1152). It lives in a Loams Postgres project of its own, never in a tenant's database.

```
namespaces      (namespace_id, loams_namespace_id, name, retention, state, config jsonb, db_record_version)
executions      (shard_id, namespace_id, workflow_id, run_id, status, next_event_id, db_record_version,
                 state_blob bytea, last_write_ts)                         PK (shard_id, namespace_id, workflow_id, run_id)
current_runs    (shard_id, namespace_id, workflow_id, run_id, status, start_request_id)  -- ID reuse/conflict policies
history_events  (shard_id, namespace_id, workflow_id, run_id, event_id, batch_first_event_id, event_type,
                 event_blob bytea, created_at)                            PK (... , run_id, event_id)
outbox          (id bigserial, shard_id, namespace_id, workflow_id, run_id, kind, resonate_id, request jsonb,
                 created_at, dispatched_at)
task_queues     (namespace_id, name, kind, partitions, metadata jsonb)
visibility      (namespace_id, workflow_id, run_id, workflow_type, status, start_time, close_time,
                 execution_time, task_queue, search_attrs jsonb, memo bytea)
schema_version  (version, applied_at)
```

The rules:
1. **One transaction per state change.** Read `executions` `FOR UPDATE`. Check `db_record_version` and `next_event_id`. Insert the history batch, update mutable state, and insert the outbox rows, then commit. If the version check fails, the request is retried or rejected. This is the only write path.
2. **The relay** reads undispatched outbox rows per shard (`FOR UPDATE SKIP LOCKED`), calls Resonate idempotently, and sets `dispatched_at`. Delivery is at least once. A crash between the Resonate call and `dispatched_at` repeats an idempotent create.
3. **Callbacks re-enter through rule 1.** Callbacks are Resonate listener deliveries and lease-expiry redeliveries. Each one carries the promise ID, which names the event it belongs to. If mutable state has moved past that event (the timer was cancelled, or the attempt superseded), the callback is a no-op and is acknowledged.
4. **Payloads are opaque.** Limits follow Temporal's defaults: 2 MB per payload, 4 MB per gRPC message, 50 MB of history and 51,200 events per run (**verify** in DX1 Task 0). Each namespace can lower them.
5. **Endpoint mode.** The gateway reaches its database through a **session-pooled or direct** endpoint, never PgDog transaction mode. Its transactions hold row locks across several statements, and it uses `LISTEN`/`NOTIFY` for the relay's fast wake-up.
6. **Never on Loams SQL** (§4.1). **History is never stored in Resonate's store**: that would couple Loams to Resonate's schema (owner draft: considered and rejected).
7. **Rebuild.** Resonate state for a shard can be rebuilt by scanning open runs and re-emitting the outbox rows their mutable state implies. Deterministic IDs make this idempotent. This is the `temporal-admin rebuild-resonate --shard` tool (DX3).

### 5.6 Matching and long polls (D1505)

- **Partitions.** Each task queue has `P` partitions (default 4; **estimate**). A partition is owned by one gateway replica at a time, through a lease row in `task_queues` that the owner renews.
- **Fast path.** A poll lands on any replica, which forwards it to the partition owner over the replicas' internal gRPC. The owner keeps a FIFO of waiting polls. When the relay creates a `wft:` or `act:` task, the owner is notified (Postgres `NOTIFY` on the shard channel, or the relay itself when they are on the same replica). The owner claims the task with `task.acquire` and answers the oldest waiting poll. Only Resonate grants the claim, so two replicas can never hand out the same task.
- **Backstop.** Resonate delivers each task to the `poll://` group of its partition. If no fast-path wake-up came, for example because the owner died, the new owner gets the delivery from Resonate after its lease timeout.
- **Polls time out** at 60 s with an empty response, which the SDKs expect (**verify** per SDK).
- **Sticky task queues** are not in the MVP. `ResetStickyTaskQueue` is a no-op and `GetSystemInfo` does not advertise them. They come in DX3 (Q1156).

### 5.7 Crate layout (D1504)

The gateway crates live in the Loams monorepo. They are linked into `loams-gateway` behind the cargo feature `temporal` and served by the module `temporal`, which has its own listener: `:7233`, or `<ns>.temporal.<base-domain>:443`. The module runs as **its own Deployment**, separate from the Postgres front door (Q1151):

```
crates/
  loams-temporal-proto/        # temporalio/api vendored at a pinned tag (MIT, NOTICE); prost + tonic codegen
  loams-temporal-history/      # event builders, event-ID allocator, batching, mutable-state fold
  loams-temporal-statemachine/ # pure state machines: workflow task, activity, timer, child, signal, cancel (no I/O)
  loams-temporal-store/        # Store trait + Postgres impl (sqlx): CAS append, outbox, visibility, migrations
  loams-temporal-matching/     # partitions, poll registry, partition leases, forwarding
  loams-temporal-resonate/     # adapter over resonate-client (fork, FW3): outbox relay, listeners, lease expiry
  loams-temporal-frontend/     # WorkflowService impl; namespace and auth interceptors from the gateway core
  loams-temporal-testkit/      # replay harness, history differ and normaliser, DST hooks, corpus loader
crates/loams-gateway/src/temporal/   # the module: listener, wiring, config, metrics
```

The design rule is the owner's. **State machines are pure, and all I/O is injected**: Postgres, Resonate, the clock, randomness and ID generation each sit behind a trait. That serves the TLA+ alignment (one action per transition) and deterministic simulation (§13.4). The crates depend on nothing Loams-internal except `loams-gateway`'s core traits, so they can be built into a standalone binary (`loams-temporal`) that serves any Resonate server. That matters for ResonateHQ (Q1153).

### 5.8 Method set by phase (owner draft; D1506)

| Phase | Methods |
|---|---|
| **DX1 (MVP)** | `GetSystemInfo`, `DescribeNamespace`, `RegisterNamespace` (admin only), `ListNamespaces`, `StartWorkflowExecution`, `DescribeWorkflowExecution`, `GetWorkflowExecutionHistory` (paged, long-poll for close), `TerminateWorkflowExecution`, `RequestCancelWorkflowExecution`, `PollWorkflowTaskQueue`, `RespondWorkflowTaskCompleted`, `RespondWorkflowTaskFailed`, `PollActivityTaskQueue`, `RespondActivityTaskCompleted`, `RespondActivityTaskFailed`, `RespondActivityTaskCanceled`, `RecordActivityTaskHeartbeat`, `SignalWorkflowExecution`, `QueryWorkflow` and `RespondQueryTaskCompleted`, `ResetStickyTaskQueue` (no-op) |
| **DX2** | Timers, retries and every timeout kind. `SignalWithStartWorkflowExecution`, child workflows, continue-as-new, ID reuse and conflict policies, local activities as recorded markers, `…ById` activity responses, `RecordActivityTaskHeartbeatById` |
| **DX3** | Visibility: `ListWorkflowExecutions`, `CountWorkflowExecutions`, `ScanWorkflowExecutions`, search attributes, Temporal's list-filter grammar on Postgres (Q1162). Sticky queues, shard and partition scaling, outbox batching, load tests against real Temporal |
| **DX4** | `UpdateWorkflowExecution` and `PollWorkflowExecutionUpdate`, worker versioning and build IDs, Schedules (Q1161), `ResetWorkflowExecution`, `OperatorService` (search attributes, namespaces). Nexus and the Temporal Cloud APIs are out (§2.2) |

`GetSystemInfo` advertises only the capabilities that are built. A CI test compares that list with the methods the frontend implements and fails if they differ (`capabilities_match_impl`). Unimplemented methods answer `UNIMPLEMENTED` with a link to the matrix.

### 5.9 The hard parts, and the answer to each

| Hard part | Answer |
|---|---|
| History fidelity | A corpus of real histories (DX0), a normalised differ, and every SDK's replayer in CI (§13.2) |
| Atomic append plus scheduling | One transaction with an outbox, deterministic IDs, and an idempotent relay (§5.5) |
| Timeouts | One generic timeout registration, `to:…:<kind>`. Stale timeouts are dropped by the compare-and-set |
| Long-poll latency | The in-memory fast path. Resonate redelivery only after a failure (§5.6). Target p99 under 50 ms from schedule to poll answer (**estimate**) |
| Task tokens | Opaque and HMAC-signed with a rotating gateway key. They carry namespace, workflow, run, scheduled event ID, attempt and kind, and are checked against mutable state by the compare-and-set |
| Capability negotiation | Start minimal and pin SDK versions (D1511) |
| Multi-tenancy | Temporal namespace inside a Loams namespace. The credential gives the Loams namespace, and the request names the Temporal namespace (Q1150) |
| Throughput | Shards by workflow-ID hash, a compare-and-set per run, a batched outbox, and Resonate's batch request if measurements need it (FW4) |

### 5.10 Compat mode: the upstream Temporal server on Loams Postgres (D1509)

Phase 0 runs the stock Temporal server (four services, `postgres12` plugin) on Loams Postgres 16 through the session or direct endpoint. It is in the compose profile `temporal` and the same Kubernetes recipe §68 planned. It is three things at once:
1. **the oracle** for the corpus and the differential harness;
2. **a fallback** that ships as "compat mode" until the gateway passes DX2's exit for a given SDK;
3. **a check of the Postgres fork** under Temporal's own persistence tests.

Postgres 17 runs as a second leg. Temporal does not list it, so its failures are expected failures, not blockers. Managed hosting of compat mode as a product keeps §68's gate (Q1033): this document does not sell hosted Temporal, it uses it.

### 5.11 Standards, licence and naming

- gRPC over HTTP/2 (TLS, or h2c on the compose network). CloudEvents for opt-in lifecycle events (§11.5).
- The protos are vendored from `temporalio/api` at a pinned tag, with the MIT licence text in `crates/loams-temporal-proto/LICENSE-temporalio-api` and a `NOTICE` entry. The Temporal **server** is MIT and may be read as a reference. Its code is not copied: the gateway is a clean implementation of the protocol, which keeps provenance simple. The CLA question in §68 §11 does not arise (D1536).
- **Naming (D1512):** "Loams speaks the Temporal API" or "works with the Temporal SDKs", always with the matrix linked. "Temporal" is Temporal Technologies' trademark. Loams never says "Temporal Cloud", never uses Temporal's logo, and never implies endorsement. The product name is **Loams Durable (Temporal API)**.

## 6. Resonate as the durability layer

### 6.1 How the gateway reaches Resonate

- **Cluster and Loams cloud:** Resonate is embedded in the `loams` engine (D138), one instance per namespace (D142), on the **TiKV store** (D261, moving to the fork by FW1). The gateway calls the durable listener with a service token. **This needs the durable listener's authentication** (D111, D142's dispatcher), which does not exist yet. DX1 cannot reach a non-loopback Resonate without it. It is a DX1 prerequisite, tracked by FW2 and FW3 on the fork side (the public router and auth hooks) and by D111's plan on the Loams side.
- **Light compose (0.1.1):** Durable runs on SQLite in the engine container. The `temporal-gateway` profile runs `loams-gateway` in the engine container's network namespace (`network_mode: service:loams`), so it reaches the loopback listener with no new exposure. That is enough for single-node setups and CI.
- **Store choice (owner draft: "Resonate store on TiDB/TiKV per the fork; validate throughput in Phase 3"):** the native TiKV store is primary. TiDB through the MySQL plugin stays legacy (D261). DX3's load test measures promises per second per keyspace on a three-node TiKV, against real Temporal on the same hardware, before any throughput claim (D1518).

### 6.2 What the gateway needs from Resonate

| Need | In the protocol today | Fork work |
|---|---|---|
| A promise with a deadline that settles `rejectedTimedout` and notifies the gateway | `promise.create` with `timeoutAt`, `resonate:timer`, `promise.register_listener` | FW7: a scenario test |
| A task with a lease, heartbeat and redelivery after expiry | `task.create`, `task.acquire` (TTL), `task.heartbeat`, `task.fulfill`, `task.release` | FW7 |
| Delayed creation for retry backoff | `resonate:delay` | FW7 |
| Idempotent create by ID | `promise.create` on an existing ID | FW7 (the exact answer codes) |
| Many gateway replicas sharing a poll group, each delivery to one replica | `http-poll` transport groups (**verify**) | FW8 |
| A typed Rust client for every request | Not a crate today (the Rust SDK carries its own network layer) | FW3: `resonate-client` |
| Search by tag without a full scan on TiKV | `promise.search` scans the store on the blob server | FW6 |
| Pruning settled promises of closed runs | **No delete in the protocol** (D147, Q40) | FW5 |
| Several requests in one round trip for the relay | None | FW4 (only if DX3 measures a need) |
| Horizontal scale: several server instances on one TiKV keyspace | The blob server serializes per origin through compare-and-set (**verify** for timers and delivery across instances) | FW9 |

## 7. Fork work: what goes to `ostrium-labs/resonate`

### 7.1 What "do all TiKV and this work in our fork" means (D1513)

**A change to Resonate's server, its stores, its protocol, its client or its spec goes to the fork.** It is written as an upstream contribution: one branch per change, DCO sign-off, tests in the fork's CI, and a Lean or TLA+ check where the spec is touched. It is opened as a PR to `resonatehq/resonate`. The Loams monorepo consumes the fork at a pinned revision and keeps only Loams code:

| `ostrium-labs/resonate` (upstreamable) | Loams monorepo (`ostrium-labs/loams`) | `ostrium-labs/loams-airflow` |
|---|---|---|
| The TiKV store (moved from `loams-durable`), the TiDB fixes, the store contract tests, the `resonate-client` crate, the gateway-facing protocol profile and its tests, settled-promise retention, tag-indexed search, multi-instance scale on TiKV, benchmarks, spec and trace-checker legs for every store | The embedding (`loams-durable`: configuration, listener, dispatcher wiring, the in-process network), the `loams-temporal-*` crates and the gateway module, the Temporal corpus and the differential harness, Postgres conformance for Temporal, DBOS and Airflow, the `loams-worker` agent, console, Meter and House integration | `airflow-provider-loams` (Python): `LoamsResonateExecutor`, hooks, operators, triggers (Q1031) |

`ostrium-labs/client-rust`, the TiKV Rust client fork, follows the same rule for TiKV client fixes. They go upstream to `tikv/client-rust`.

### 7.2 Branch model (D1514)

- `main` mirrors `resonatehq/resonate` `main`, fast-forward only.
- **One branch per upstream PR:** `feat/<topic>` or `fix/<topic>`, on `main`, one logical change. This matches the existing `feat/tidb-backend` and `fix/mysql-retryable-errno` branches.
- **Integration branches** `loams/<upstream-version>`: `main` plus the carries, rebuilt on each upstream bump. The existing `loam/0.10.1` keeps its name until the next bump. New ones use `loams/`.
- `CARRIES.md` on each integration branch lists every carry, its upstream PR and its state.
- Loams bumps its pin in one PR that changes `Cargo.toml`'s `rev` and nothing else, except the code that needs the new revision.
- The fork's CI runs on GitHub-hosted runners: the engine and port differentials, porcupine, the Lean trace checker, and one leg per store.

### 7.3 How fork work is checked

- **The Lean trace checker** (`spec/valid`, `lake exe checktrace`) accepts traces recorded from the server on each store. The fork records them with the `work/` generators.
- **Porcupine** (`conctrace` and `conccheck`) runs 16 × 400 operations with three seeds for each store (D144's shape).
- **Store contract tests** (`resonate-server-blob/tests/store_contract.rs`, written on `loam/tikv-dapr` and upstreamed with FW1) and the differential test against SQLite run for every `Store`.
- **Spec changes.** A change that adds a request or a state carries a Lean change and a TLA+ change in `spec/`, or a written argument that it adds no state (FW4 is that case).

### 7.4 The fork-work list (D1517)

Each task is a branch and an upstream PR. The acceptance criteria are what the PR must show. FW0–FW3 come before DX1. FW4–FW10 run beside DX1–DX3.

| ID | Task | Acceptance criteria | Needed by | Effort (estimate) |
|---|---|---|---|---|
| **FW0** | **Shepherd the open PRs and set up the fork's process.** Rebase #1162, #1163, #1164, #1167 and #1168 on upstream `main`, answer review, and add `CARRIES.md` and the fork CI on GitHub-hosted runners | Each PR rebased with green upstream CI; `CARRIES.md` lists every carry on `loam/0.10.1` with its PR or a "not upstreamable" reason; the fork's CI runs the differentials, porcupine and `checktrace` on SQLite, Postgres and MySQL/TiDB | Everything | 1 week |
| **FW1** | **The TiKV store moves into the fork.** One `Store` for `resonate-server-blob` behind the feature `tikv`, replacing both the monorepo's `crates/loams-durable/src/tikv.rs` and the fork's `loam/tikv-dapr` `store_tikv.rs`. It uses TiKV's transactional API with API v2 keyspaces and a configurable key prefix. A server plugin `resonate-server-tikv` (`--storage-type tikv`) is registered with the plugin system | It keeps the **0.1.0 on-disk format byte for byte** (`resonate/blob/` prefix, 16-byte random revision plus body); test `tikv_store_reads_loams_010_fixture` loads a dump taken from a 0.1.0 stack. `store_contract.rs` passes on a PD + TiKV v8.5.x CI service. The differential against SQLite, porcupine (16 × 400, three seeds) and `checktrace` pass. Chaos: `kill -9` of the server mid-run, TiKV leader transfer, and a PD restart, each with no lost or duplicated settlement. Undetermined commits are re-read, never assumed. Loams deletes `loams-durable/src/tikv.rs` and uses the plugin, with `--durable-store tikv://…` unchanged | DX1 (cluster), Resonate Cloud pitch | 2–3 weeks |
| **FW2** | **TiDB through the MySQL plugin, finished upstream** (#1162, #1163): the session pin, errno classification and the xtask and CI leg on TiDB v8.5.x | Merged upstream, or carried with the reviewer's reason recorded. The CI leg is green on TiDB v8.5.x. A test shows the plugin takes **no** `LOCK IN SHARE MODE` or `FOR SHARE` lock whose loss would break exclusion on TiDB (**verify**; if one exists, the plugin uses `FOR UPDATE` or documents it) | Legacy support; Resonate Cloud users on TiDB | 0.5–1 week |
| **FW3** | **`resonate-client`, a typed Rust client crate** for every protocol request (`promise.*`, `task.*`, `schedule.*`), with retries on 503 and an auth-header hook. Also the **public router entry** so an embedder can dispatch per tenant (§21's "PR 4") | Published from the fork's workspace and used by `resonate-sdk` (Rust) or alongside it. The round-trip tests cover every request kind against the SQLite server. `checktrace` accepts traces from a client-driven workload. The router entry lets an embedder mount `api_routes()` per tenant with no copy of private code. The `loams-temporal-resonate` crate and `loams-worker` build on it | DX1, DX1A | 1–2 weeks |
| **FW4** | **A batch request** that carries up to N independent requests (`promise.create`, `task.create`, `promise.settle`) and answers each independently. It is not atomic and adds no state | A written spec note that it is a sequence of independent steps, plus a Lean test that a batch trace equals the unbatched trace. Throughput on TiKV improves by at least 3× for 100-item relay batches (**estimate**; the PR is closed if it does not). **Only built if DX3's measurements show per-request overhead dominates** | DX3 | 1 week |
| **FW5** | **Settled-promise retention** (§21's "PR 5", Q40): a server setting `retention` that prunes promises settled before a horizon, per tag selector (`loams:run`, `loams:ns`), leaving a tombstone that makes a late re-create of a pruned ID answer `expired` instead of re-running the work | A spec change (Lean and TLA+), with a property that no pruned ID is re-created within the tombstone horizon. It works on SQLite, Postgres and TiKV. Store size stays flat in a 24 h soak that settles 1 M promises (**estimate**) | DX2 (Temporal retention), Airflow | 2 weeks |
| **FW6** | **Tag-indexed search on the blob and TiKV store.** A secondary index for `promise.search` and `task.search` by tag and state, with cursors | The cost is O(results), not O(store): a test counts store reads. Results are identical to SQLite's search on the differential workload. The index is updated in the same compare-and-set as the document, or rebuilt idempotently | DX3 visibility backstop, console | 1–2 weeks |
| **FW7** | **The gateway-facing protocol profile:** a documented subset (timer promise, task with lease and heartbeat, delayed create, idempotent create, cancel, listener delivery, schedule) with scenario tests in `resonate-oracle` that mirror §5.3. The exact answer codes and the promise-ID length limit are written down | The scenario tests pass on SQLite, Postgres and TiKV. The profile document is in the fork's `docs/`, and `checktrace` accepts each scenario's trace | DX1 Task 0 | 1 week |
| **FW8** | **Poll groups with many consumers:** many gateway replicas or worker agents share one `poll://any@<group>`. Each delivery goes to one consumer and is redelivered after lease expiry, and a consumer that disconnects is dropped | A test with 3 consumers and 10,000 tasks: each task is acquired once per attempt, and redelivery happens within TTL + 1 s after a consumer is killed. Behaviour is the same on all stores | DX1, DX1A | 1 week |
| **FW9** | **Several server instances on one TiKV keyspace:** origin ownership, timers and delivery when 2–3 instances share the store, so one instance's failure delays timers by at most a lease | Porcupine and `checktrace` pass with 3 instances and one killed mid-run. Timers fire at least once and settle once. The worst-case added timer delay after an instance dies is measured and documented | Cluster HA, Resonate Cloud | 2–3 weeks |
| **FW10** | **Benchmarks and observability:** `xtask bench` for promise create and settle per second, timer accuracy and task claim latency per store, and OTel trace propagation (`traceparent` carried in a tag), with Prometheus metrics that have no tenant labels | Reproducible bench numbers on SQLite, Postgres and TiKV (three nodes), published in the fork's `docs/bench.md` with hardware and commit. `traceparent` crosses create → task → settle in a test | DX3, the ResonateHQ pitch | 1 week |

**Total (estimate):** 13–18 engineer-weeks, with FW0, FW1, FW3 and FW7 (5–7 weeks) first.

**What does not go to the fork:** anything about Temporal, DBOS or Airflow semantics. Resonate stays protocol-pure, and the Temporal crates talk to it only through `resonate-client`.

### 7.5 The design-partner offer (D1539)

The fork list is what Loams offers ResonateHQ:
- the first TiKV and TiDB backends for Resonate (FW1, FW2);
- the operational pieces a hosted Resonate needs: retention, indexed search, multi-instance scale, benchmarks (FW5, FW6, FW9, FW10);
- a typed Rust client (FW3).

All of it is Apache-2.0 and upstreamable, and checked by Resonate's own Lean and TLA+ spec. The **Temporal-protocol crates** stay in Loams, but they are built so that Resonate Cloud could run them against its own servers (§5.7, Q1153). That is the "Temporal compat for ResonateHQ cloud" the owner names. The commercial terms of the partnership are the owner's and still open (Q1165).

## 8. DBOS on Loams Postgres

### 8.1 Support means conformance plus the right endpoint (D1519)

DBOS Transact is a library (Python, TypeScript, Go, Java and Rust) that keeps workflow state in a Postgres system database. Loams provides three things:
1. a Loams Postgres that passes each SDK's own test suite;
2. an endpoint that keeps session features;
3. operations guidance and a console view.

**DBOS is never routed through Resonate** (owner draft).

**The conformance checklist (DX0 Task 5):**
- transactions at every isolation level, including `SERIALIZABLE`;
- `FOR UPDATE SKIP LOCKED` (DBOS queues);
- `LISTEN`/`NOTIFY` (workflow events and notifications);
- `INSERT … ON CONFLICT`;
- `CREATE DATABASE` and `CREATE SCHEMA` as the application role, and DBOS's migrations;
- advisory locks, `jsonb`, `bytea`, `timestamptz`;
- long-lived connections across compute suspend and resume.

On top of the suites, Loams adds a crash-recovery test: `kill -9` of the app mid-workflow, then a restart, after which every step completed exactly once.

### 8.2 The endpoint (corrects the draft's "pgdog fork")

PgDog runs unmodified (D318, D708). Its **transaction pooling** breaks `LISTEN` across transactions and session-scoped state. So DBOS apps use the **session-pooled or direct endpoint** that pg-control and the gateway expose (§68 §8.2's rule). DX0 Task 2 checks which of those exist in the release compose and on Kubernetes. A PgDog transaction-mode leg runs as an expected failure, so a future PgDog change that fixes it is noticed (D1519).

- **One database per DBOS app** holds the system schema and the app tables that transactional steps touch, unsharded on one primary (D1520).
- DBOS's system database is never placed on a shard of §31's router.

### 8.3 Scale-to-zero (D1521)

DBOS's listener and its queue polling keep a compute awake. The defaults:
- a DBOS-labelled project uses the **always-on** compute class;
- where the user accepts a wake-up delay, Python apps can set `use_listen_notify = false` when the database is first created and tune the polling interval (Q1157);
- a later wake-on-`NOTIFY` integration is a direction, not a commitment.

DX0 Task 5 tests that `NOTIFY` delivery survives a compute restart, once the listener reconnects.

### 8.4 What Loams builds for DBOS (D1522)

| Phase | Content |
|---|---|
| DX0 | Conformance CI: each SDK's suite, the crash-recovery test, and a polling leg (Python) |
| DX2 | A "DBOS database" preset in the console (always-on, session endpoint). A read-only console view of DBOS's system tables (workflows, steps, queues) through pinned views, never writes. Docs and templates |
| Later, if asked | A Rust client, and compatibility with DBOS's management protocol **only** if its licence allows an open implementation (**verify**). Conductor is proprietary and Loams never imitates it |

Queues guidance: DBOS queues for durable work bound to a workflow; Loams streams (CloudEvents) for high-volume fan-out.

## 9. Apache Airflow 3.3

### 9.1 Scope (owner draft; D1523)

- **Airflow 3.3.x only**, pinned to a patch release at least 14 days old (D1274), with a version matrix in CI. Real Airflow, never a look-alike (D1256 reconfirmed).
- **One Airflow deployment per environment first**, each with its own database on Loams Postgres.

| Option | Decision |
|---|---|
| A. Metadata DB on Loams Postgres | Do first (DX0) |
| B. The stock Edge executor (`edge3`) | The baseline and fallback (DX0) |
| C. A Resonate-backed executor | Build in DX1A (D1525) |
| D. `airflow-provider-loams` | Build incrementally (D1527) |
| E. Rust tasks through AIP-108's `ExecutableCoordinator` | Later, after upstream leaves experimental (D1528) |
| F. Reimplement Airflow | Never |

### 9.2 The metadata DB (DX0)

These are §68 §4.3's gate tests:
- `airflow db migrate` from empty, with the advisory lock released afterwards;
- two schedulers with row-level locking: 500 mapped task instances, no double run;
- `kill -9` of one scheduler mid-run;
- remote logs and XComs in the bucket;
- a `GitDagBundle`.

DX0 adds the **Edge executor**: edge workers register, run tasks, survive a worker kill (the task is retried), and enter maintenance mode. Workers do not connect to the DB in 3.x. The scheduler, the triggerer and the API server use the **session or direct endpoint** unless DX0 shows they pass under transaction pooling.

### 9.3 `LoamsResonateExecutor` and the worker agent (D1525, D1526)

```
scheduler ──(ExecuteTask workload)──▶ LoamsResonateExecutor (Python, in the provider)
   │                                     │ promise.create af:{ns}:{dag}:{run}:{task}:{map}:{try}
   │                                     │   param = workload + Execution API URL; tags resonate:target = poll://any@af.<ns>.<queue>
   │                                     ▼
   │                                  Resonate (task + lease)
   │                                     │ delivery to the queue's group
   │                                     ▼
   │                                  loams-worker (Rust) ── task.acquire / task.heartbeat
   │                                     │ spawns the Task SDK supervisor in loams-sandbox, with a per-task JWT
   │                                     ▼
   └──────── Execution API (HTTP, mTLS) ◀── the task reports its own state
```

- **Who reports what.** The task reports its own state (running, success, failed, deferred, up for retry) through the Execution API, exactly as on any executor. The executor reports only **infrastructure outcomes** from Resonate:
  - no claim before `[loams] claim_timeout`;
  - lease expiry, when the worker died;
  - spawn failure;
  - the sandbox refusing the task.

  `sync` maps those to `fail` or `queued` with a reason, through `get_event_buffer`.
- **Mapping.** An Airflow queue maps to a Resonate target group. An Airflow pool maps to a concurrency limit in the executor. `try_adopt_task_instances` re-reads open promises by tag after a scheduler restart, so tasks are never orphaned. `terminate` and `on_kill` cancel the lease (`task.halt`), and the worker sends SIGTERM and then SIGKILL after `[loams] kill_grace`.
- **Credentials.** The workload's JWT is short-lived and never logged. The worker reaches the Execution API over mTLS on the private network.
- **Images.** Worker images are pinned per Dag bundle version, and the worker pulls the bundle at the recorded version.
- **Metering.** The worker reports CPU time per task. It comes from cgroup accounting in the sandbox, as `io.loams.dev.usage.jobs.run.v1` (D1268).
- **The worker agent** is `crates/loams-worker`, a small Rust binary. It is a **generic Resonate task runner**: claim a task from a group, run a command in `loams-sandbox`, heartbeat, and report outcomes. Airflow is its first client, and Loams Jobs can reuse it (Q1159).
- **Fallback.** The stock Edge executor stays supported for any deployment where the Resonate executor is not wanted.

### 9.4 The provider: `airflow-provider-loams` (D1527, amends D1257's contents)

The provider lives in `ostrium-labs/loams-airflow` (Q1031, approved) and is published on PyPI as `airflow-provider-loams`. §68 §4.4's contents stay: `LoamsHook`, the operators, `LoamsStreamTrigger` for `AssetWatcher` with `loams://<namespace>/<object>` asset URIs, and `LoamsSecretsBackend`. This document adds:
- `LoamsResonateExecutor` (§9.3), which **replaces `LoamsJobsExecutor` as the first executor** (Q1160);
- `LoamsPostgresHook`, for the session-endpoint discovery;
- `TemporalHook`, `StartTemporalWorkflowOperator` and `TemporalWorkflowTrigger` (deferrable). They work against the Loams gateway or any Temporal server;
- `ResonatePromiseOperator`, `ResonatePromiseSensor` and `ResonatePromiseTrigger` (deferrable). These supersede §68's `LoamsDurableOperator` name;
- remote logging to the bucket, and an AIP-103 state-store backend on Loams Postgres (**verify** the extension point in 3.3);
- OpenTelemetry export to Loams;
- an auth manager for Authentik and Loams principals (Q1158; FAB OAuth with Authentik in DX0's recipe; **not** Airflow's Keycloak auth manager).

### 9.5 Rust tasks (later; D1528)

AIP-108's `ExecutableCoordinator` runs native binaries, with Go as the reference. A Rust task SDK is prototyped only after upstream drops the "experimental" label, and in `ostrium-labs/loams-airflow`, not the monorepo.

## 10. Which engine to use (D1530)

| Use | When |
|---|---|
| **Airflow** | Data pipelines, backfills, lineage, asset-driven schedules and human-in-the-loop on data jobs; teams that already have Dags |
| **Temporal API (gateway)** | Long-running business workflows; teams with existing Temporal code or skills; strict event history and replay semantics |
| **DBOS** | Durable functions inside an application, whose steps are database transactions in the same Postgres; no extra server |
| **Resonate (native)** | Lightweight durable promises and functions, agent runtimes, and cross-language RPC with durable callbacks; Loams' own long operations |

This table goes into the docs (`docs/guides/durable/which-engine.md`) and the console's create flow.

## 11. Shared control plane

### 11.1 Tenancy

- A Temporal namespace, an Airflow deployment and a DBOS database all belong to **one Loams namespace**.
- **A Temporal namespace is not a Loams namespace.** A Loams namespace holds zero or more Temporal namespaces (Q1150).
- Each engine gets its own databases and its own Resonate store (per-namespace instances, D142). **Never a shared schema.**

### 11.2 Identity and authorization (D1507)

- **Humans** sign in through Authentik (OIDC, D447). **Services** use `lk_` API keys or workload-identity tokens.
- **The gateway** checks the bearer token and resolves the Loams namespace. It authorizes `durable:temporal:*` actions per Temporal namespace (namespace admin, workflow writer, reader).
- **Airflow's API server** uses the provider's auth manager. Workers carry only the per-task JWT.
- **Audit.** Every admin action, and every start, signal, terminate and reset, goes to Loams' audit stream.

### 11.3 Metering (§63)

| Engine | Event | Attributes |
|---|---|---|
| Temporal API | A new event type, `io.loams.dev.usage.durable.temporal.v1` (D1531) | Temporal-style actions (starts, signals, timers, activities, workflow tasks) and history bytes, per Temporal namespace |
| Compat mode (upstream server) | `io.loams.dev.usage.companion.runtime.v1` (D1268) | — |
| Airflow tasks on `loams-worker` | `io.loams.dev.usage.jobs.run.v1` | — |
| DBOS | Ordinary Loams Postgres compute and storage | — |

The Meter gate reserves before a start and refuses at hard limits, like any other workload.

### 11.4 Metrics and history analytics (maps the draft's "Iceberg tables")

- **Operational metrics:** `loams_temporal_*` and `loams_worker_*` in Prometheus and OTel, **never labelled by tenant** (D1021's rule). Per-namespace numbers go through the observer trait.
- **Analytics:** closed-run summaries, task-run records and Airflow task outcomes are exported nightly, or on close, as **Iceberg tables in Lakekeeper**. They are read in the **House** (chDB), never written by the engines into Loams-maintained tables (D1265). Retention follows the namespace.

### 11.5 Events

Opt-in CloudEvents on a Loams stream, one per lifecycle step (D1533):
- `io.loams.dev.durable.temporal.workflow.started.v1`, and likewise `…closed.v1`;
- `io.loams.dev.durable.airflow.task.finished.v1`.

They feed `LoamsStreamTrigger`, Flow and Knative sinks.

### 11.6 Console

One console section, **Durable**, with tabs for Resonate (the existing page), Temporal (namespaces, workflows, history viewer, compat matrix), Airflow (deployments, link to its UI) and DBOS (read-only workflow view). There is one conformance dashboard for all four.

## 12. Deployment

| Where | What |
|---|---|
| **Compose** (`deploy/release/compose.compute.yaml`, §68's opt-in file) | Profiles: `temporal` (compat mode: the upstream server on Loams Postgres; DX0); `temporal-gateway` (`loams-gateway` with the `temporal` module, in the engine's network namespace; DX1); `airflow` (Edge executor in DX0, `LoamsResonateExecutor` and `loams-worker` in DX1A); `dbos` (test harness only). Nothing starts without a profile, and no new host ports |
| **Kubernetes** | `loams-gateway-temporal`: a Deployment of `loams-gateway` with only the `temporal` module, at least two replicas across zones, a PDB, exposure `tailnet` or `internal` (D1195) behind the edge rules of §69. Airflow keeps §68's `LoamsCompanion` path (OC2) for managed hosting. A self-hosted Airflow uses the official chart with the recipe |
| **Databases** | One `loams_temporal` Loams Postgres project per region; one per Airflow deployment; one per DBOS app |

## 13. Testing and verification (owner draft; D1510)

### 13.1 Layers

| Layer | What | Where |
|---|---|---|
| Unit | Pure state machines (every command and event pair), the event-ID allocator, the ID grammar | `loams-temporal-statemachine`, `-history` |
| History fidelity | Gateway histories against the corpus from real Temporal, through a **normaliser**. It masks timestamps, run IDs, identities and task tokens, and keeps event IDs, types, ordering, batch boundaries and attributes | `loams-temporal-testkit`, CI per PR on the corpus |
| Determinism | Each pinned SDK's **replayer** replays gateway histories: Go `worker.WorkflowReplayer`, TypeScript `Worker.runReplayHistories`, Java `WorkflowReplayer`, Python `Replayer` | Nightly, per SDK |
| End to end | The SDKs' own samples (`samples-go`, `samples-typescript`, then Java and Python) and the Temporal CLI against the gateway | Nightly |
| Differential | The same workload against compat mode and against the gateway, with histories compared after normalisation | Nightly; required at each phase exit |
| Linearizability | Porcupine over the gateway's client-visible operations (start, signal, describe, close) | Nightly |
| Formal | TLA+ specs in `spec/tla/temporal/`: `WorkflowTask`, `Activity`, `Timer`, `OutboxCas` (the transaction, relay and callback race). TLC in CI job `tla`, with a `selftest` that must fail | Per PR (path-filtered) |
| Simulation | Deterministic simulation of gateway + store + Resonate behind the injected traits: seeded clock, network and crash faults, and Resonate state loss followed by rebuild (§5.5 rule 7) | Nightly, 1,000 seeds |
| Performance | Starts per second, schedule-to-poll latency and history-append latency, against real Temporal on the same hardware | DX3 |
| Airflow | §9.2's suite, executor tests per minor, chaos (worker kill, lease expiry, scheduler failover), HA soak, version-skew matrix | Nightly |
| DBOS | Each SDK's suite plus crash recovery and the polling leg | Nightly |

### 13.2 Invariants (checked in TLA+, in simulation, and as runtime assertions in debug builds)

1. Event IDs in a run are contiguous and increase monotonically.
2. A run has at most one outstanding workflow task.
3. Every scheduled activity and every started timer gets exactly one terminal event.
4. A stale callback (a superseded attempt, a cancelled timer) never changes state.
5. The outbox drains at least once, and every Resonate call is idempotent by ID.
6. Losing Resonate's state and rebuilding it from history reaches the same set of outstanding promises.

### 13.3 Phase exits

| Exit | Bar |
|---|---|
| **DX1** | Hello-world, an activity and a signal pass on Go and TypeScript; the replayers accept the gateway's histories |
| **DX2** | The corpus scenarios for DX2's methods match after normalisation on Go, TypeScript, Java and Python, with zero replay failures in 7 nightly runs |
| **DX3** | The load test is within 2× of real Temporal's p99 at the same throughput (**estimate**) |

Compat mode stays the recommended path for an SDK until that SDK passes DX2's exit.

## 14. Phases (D1534)

| Phase | Content | Effort (estimate) | Gate |
|---|---|---|---|
| **DX0** | Baselines: compat mode (Temporal on Loams Postgres 16, with 17 as a leg), DBOS conformance, the history corpus (Go and TypeScript), Airflow 3.3 + Edge executor on Loams Postgres. **Fully specified in [DX1](../plans/2026-10-21-dx1-durable-execution-compat.md)**. Reuses OC0 Tasks 1, 2, 6 and 7 | **3–3.5 engineer-weeks** (about 2 if OC0 Tasks 1, 2, 6 and 7 landed first; the draft said 1–2) | v0.1.1 shipped (Q1030's timing) |
| **FW0–FW3, FW7** | Fork process, TiKV store, TiDB upstream, `resonate-client`, gateway profile | 5–7 | Parallel with DX0 |
| **DX1** | Gateway MVP: protos, tonic module, store with compare-and-set and outbox, Resonate adapter, matching fast path, MVP methods; Go and TypeScript hello-world; replayers accept gateway histories | 3–5 (owner draft), **6–8 (estimate, with the store and matching)** | DX0 corpus; FW3, FW7; durable-listener auth or the compose network-namespace path. **Amended by D1590:** starts in parallel with DX0; FW3 and FW7 no longer gate its start, and its exit report states their state (D1591) |
| **DX1A** | `LoamsResonateExecutor`, `loams-worker`, provider v0, chaos tests, Airflow version matrix | 4–6 (owner draft) | DX0 Airflow; FW3, FW8 |
| **DX2** | Timers, timeouts, retries, signals, queries, cancel and terminate, child workflows, continue-as-new, ID reuse; differential harness in CI; Java and Python | 4–8 (owner draft) | DX1 exit |
| **DX3** | Visibility, sticky queues, sharding and partitions, outbox batching, load tests against Temporal; Resonate throughput on TiKV | 4–6 | DX2 exit; FW6, FW9, FW10 (FW4 if measured) |
| **DX4** | Updates, versioning, Schedules, reset, operator APIs; Airflow asset watchers, auth manager, state store, console; Coordinator and Rust prototype | 8–12 | DX3; upstream AIP-108 for the Rust part |
| **DX5** | TLA+ specs, deterministic simulation with fault injection, porcupine, soak with SDK suites | 4–6, **parallel from DX1** | — |

**Near-term total (DX0–DX2, with FW0–FW3 and FW7–FW8):** about 24–35 engineer-weeks (**estimate**: DX0 3–3.5, FW0–FW3 with FW7 and FW8 6.5–9, DX1 6–8, DX1A 4–6, DX2 4–8). §68 estimated a minimal Temporal frontend at 13–26 engineer-weeks and parity at multi-year. That stands as the risk in §15 (R1).

## 15. Risks

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R1 | Temporal API scope creep: 121 RPCs, growing every release | High | High | The phased method set; `GetSystemInfo` advertises only what is built; a per-method, per-SDK matrix is the only claim; compat mode as the fallback |
| R2 | History diverges late, in rare paths (resets, continue-as-new, failures inside batches) | Medium | High | The corpus covers failure paths from DX0; the differential and replayers run every night; normaliser rules are reviewed against SDK source |
| R3 | Upstream drift: protos and SDK behaviour | High | Medium | Pinned protos and SDKs; a nightly against SDK `latest`; `buf breaking` on the vendored protos |
| R4 | Resonate becomes a bottleneck at one promise per activity and timeout | Medium | High | Measure early (DX3, FW10); coalesce timeouts per run (one `to:` promise for the earliest deadline); FW4's batch; partitioned keyspaces |
| R5 | The durable listener's auth (D111) lands late, which blocks a cluster gateway | High | Medium | Compose network-namespace path for single node; FW3's router entry; DX1's cluster leg gated on it |
| R6 | ResonateHQ does not merge the fork work | Medium | Medium | Carries with `CARRIES.md`; small PRs; spec-checked changes; Q1154's carry rule |
| R7 | DBOS changes its schema or needs features that the endpoint modes break | Medium | Medium | Pinned SDKs; read-only console views only; the transaction-mode xfail leg |
| R8 | Four engines are hard to operate | Medium | Medium | One console section and one conformance dashboard; the which-engine guide; compat mode never default in production |
| R9 | Airflow churn: the Execution API is not public | High | Medium | Pin minors; nightly against `latest`; the Edge executor as fallback |
| R10 | Airflow's footprint against scale-to-zero | Medium | Low | The always-on class for the scheduler and API server; shared control-plane tier |
| R11 | Trademark or wording complaints ("Temporal-compatible") | Low | Medium | D1512's wording; matrix-backed claims; no logos |
| R12 | The two TiKV stores' formats conflict when FW1 unifies them | Medium | High | FW1 keeps the 0.1.0 format, with a fixture test; no migration without a recorded decision |

## 16. What not to build (D1535)

- A Temporal fork, Temporal server code in Loams, or Temporal persistence or gateway history on Loams SQL.
- Temporal Cloud-only features (Nexus endpoints, multi-region namespaces, cloud ops API) in DX0–DX4.
- An Airflow look-alike or a DAG-to-Resonate translator.
- DBOS routed through Resonate, a DBOS fork, or a Conductor clone.
- A PgDog fork, or PgDog patches for endpoint modes.
- Temporal or Airflow semantics inside the Resonate fork.
- Any BUSL Resonate server (`resonate-on-nats`, `resonate-on-scylladb`).

## 17. Amendments to design 68 (owner direction, 2026-10-21)

| §68 decision or question | Was | Now | By |
|---|---|---|---|
| **D1259** (Temporal) | Durable stays Resonate; **no Temporal-compatible frontend**; Temporal hosted only after the spike; never on Loams SQL; a migration guide | **Durable stays Resonate, and Loams adds a Temporal-protocol frontend** in `loams-gateway` backed by Resonate and Loams Postgres (D1500, D1504). The upstream server on Loams Postgres runs from DX0 as oracle and compat mode (D1509). "Never on Loams SQL" stands. The Temporal → Resonate migration guide stays, as one option among two | D1500, D1509 |
| **D1272** (not built) | "A Temporal-compatible frontend, a Temporal fork, or Temporal on Loams SQL" | The frontend is removed from the list. "A Temporal fork, or Temporal on Loams SQL" stays (D1535) | D1535 |
| **D1275** (wording) | Never "Temporal-compatible" | Allowed for the gateway, which Loams wrote, only with the matrix linked; "Airflow-compatible" and "Spark replacement" stay forbidden (D1512) | D1512 |
| **§68 §2.2** (non-goals: "no Temporal frontend") | Out in OC0–OC2 | Struck for Temporal; the other look-alike non-goals stand | D1500 |
| **D1257** (provider) | `LoamsJobsExecutor` on Loams Jobs | `LoamsResonateExecutor` on Resonate directly, with `loams-worker`; `LoamsJobsExecutor` deferred (Q1160); Temporal and Resonate operators added (D1525, D1527) | D1525, D1527 |
| **Q1033** (managed Temporal's gate) | Only after the spike passes and three users ask | Unchanged for **managed hosting** of the upstream server; compat mode as a self-hosted recipe and an oracle is no longer gated, and runs in DX0 | D1509 |
| **Q1034** (managed Airflow's gate) | Three deployments on `LoamsJobsExecutor`, or three users ask | Reads `LoamsResonateExecutor` in place of `LoamsJobsExecutor` | D1525 |
| **OC0 Tasks 6 and 7** | The Airflow L0 case; the Temporal spike, whose failure kept the recipe out | DX0 Tasks 6 and 3 extend them (Edge executor; compat mode; the corpus). They are run once, shared by both plans | D1534 |
| **D1256, D1258, D1271, Q1031** | — | Reconfirmed: real Airflow; managed Airflow gated; licences; `ostrium-labs/loams-airflow` | — |

Other amendments:

| Decision | Change |
|---|---|
| **D140** (the fork) | The fork is `ostrium-labs/resonate` (the Cargo pin already says so); the branch model is D1514 |
| **D261** (TiKV store) | The store's code moves from `crates/loams-durable/src/tikv.rs` to the fork (FW1, D1515); `--durable-store tikv://…` and the on-disk format are unchanged |
| **D147, Q40** (retention) | Answered in direction by FW5: retention with tombstones in the fork, upstreamable |
| **§21 §11.2** (upstream PRs "0a–5") | Tracked as FW0 (0a, 0c, 1), FW3 (4) and FW5 (5) |

## 18. Decisions (D1500–D1539, approved by the owner on 2026-10-21)

| ID | Decision | Status |
|---|---|---|
| D1500 | **Durable execution compatibility:** beside native Resonate, Loams adds a Temporal-protocol frontend in `loams-gateway` backed by Resonate and Loams Postgres, DBOS on Loams Postgres, and Airflow 3.3 with a Resonate-backed executor; amends D1259, D1272, D1275 and D1257 (§17) | Approved (owner, 2026-10-21) |
| D1501 | **Temporal history is the source of truth**, owned by the gateway; Resonate supplies timers, timeouts, leases, retries and wake-ups, and is rebuildable from history | Approved (owner, 2026-10-21) |
| D1502 | **History and mutable state in Loams Postgres** (`loams_temporal`, one per region), one transaction per change with a compare-and-set and a transactional outbox; session or direct endpoint; never on Loams SQL; never in Resonate's store | Approved (owner, 2026-10-21) |
| D1503 | **Deterministic promise IDs** with §5.4's grammar (`wft`, `act`, `tmr`, `to`, `start`, `af`), Loams namespace IDs, percent-encoding, a hash fallback for length, and `loams:*` tags | Approved (owner, 2026-10-21) |
| D1504 | **Crate layout:** `crates/loams-temporal-{proto,history,statemachine,store,matching,resonate,frontend,testkit}`, pure state machines with injected I/O, served by `loams-gateway`'s `temporal` module behind feature `temporal`, as its own Deployment | Approved (owner, 2026-10-21) |
| D1505 | **Matching:** in-memory fast path on partition owners, with Resonate as the durable backstop and the sole authority for claims; partition leases in Postgres | Approved (owner, 2026-10-21) |
| D1506 | **Method set by phase** (§5.8); `GetSystemInfo` advertises only built capabilities, checked in CI; `UNIMPLEMENTED` elsewhere | Approved (owner, 2026-10-21) |
| D1507 | **Identity:** Temporal SDKs authenticate with Loams `lk_` keys or Authentik-issued OIDC tokens as bearer tokens (optional mTLS); `durable:temporal:*` actions per Temporal namespace; no Keycloak | Approved (owner, 2026-10-21) |
| D1508 | **Limits:** payloads opaque; Temporal's default size and count limits (verified in DX1 Task 0), lowerable per namespace | Approved (owner, 2026-10-21) |
| D1509 | **Compat mode:** the upstream Temporal server on Loams Postgres (16; 17 as a leg) from DX0, as the reference oracle and as the fallback until the gateway passes DX2 per SDK; managed hosting keeps Q1033's gate | Approved (owner, 2026-10-21) |
| D1510 | **Verification:** §13's layers and invariants (corpus differential, SDK replayers, porcupine, TLA+ in `spec/tla/temporal/`, deterministic simulation, performance against Temporal) | Approved (owner, 2026-10-21) |
| D1511 | **SDK order:** Go and TypeScript first, then Java and Python, then .NET, Ruby and Rust; each pinned at a release at least 14 days old | Approved (owner, 2026-10-21) |
| D1512 | **Wording:** "Loams speaks the Temporal API" or "works with the Temporal SDKs", with the matrix linked, for the gateway only; no "Temporal Cloud", no logo, no implied endorsement; product name Loams Durable (Temporal API); amends D1275 | Approved (owner, 2026-10-21) |
| D1513 | **Fork rule:** all Resonate server, store, protocol, client and spec work, TiKV included, is done in `ostrium-labs/resonate` as upstreamable branches; the monorepo keeps the embedding, gateway crates, conformance and the worker agent; the provider stays in `ostrium-labs/loams-airflow` | Approved (owner, 2026-10-21) |
| D1514 | **Fork branch model:** `main` mirrors upstream; one `feat/` or `fix/` branch per upstream PR; integration branches `loams/<upstream-version>` with `CARRIES.md`; pin bumps in their own PR; fork CI on GitHub-hosted runners | Approved (owner, 2026-10-21) |
| D1515 | **The TiKV store moves to the fork** (FW1) as a `resonate-server-blob` store and `resonate-server-tikv` plugin, keeping the 0.1.0 on-disk format; `loams-durable/src/tikv.rs` is deleted after the pin bump; amends D261's location | Approved (owner, 2026-10-21) |
| D1516 | **`resonate-client`** (FW3), a typed Rust protocol client in the fork, is the only way Loams' gateway and worker agent call Resonate | Approved (owner, 2026-10-21) |
| D1517 | **The fork-work list FW0–FW10** (§7.4), each with its acceptance criteria, checked by Resonate's Lean trace checker, porcupine and store contract tests | Approved (owner, 2026-10-21) |
| D1518 | **Resonate store for the gateway:** the native TiKV store in clusters and Loams cloud, SQLite in single-node compose; TiDB through the MySQL plugin stays legacy (D261); DX3 measures throughput before any claim | Approved (owner, 2026-10-21) |
| D1519 | **DBOS:** no protocol work; Loams Postgres passes each SDK's suite plus crash recovery; DBOS uses the session or direct endpoint; PgDog stays unmodified (corrects the draft's "pgdog fork"); never routed through Resonate | Approved (owner, 2026-10-21) |
| D1520 | **One database per DBOS app**, unsharded on one primary, holding the system schema and the app tables its transactional steps touch | Approved (owner, 2026-10-21) |
| D1521 | **DBOS and scale-to-zero:** always-on class by default; polling (`use_listen_notify = false`) where the SDK supports it and the user accepts wake-up delay | Approved (owner, 2026-10-21) |
| D1522 | **DBOS in the console:** a preset, read-only views of system tables, docs and templates; no dependency on or imitation of Conductor (proprietary) | Approved (owner, 2026-10-21) |
| D1523 | **Airflow 3.3.x only**, real Airflow (D1256 reconfirmed), one deployment per environment first, one database per deployment, a version matrix in CI | Approved (owner, 2026-10-21) |
| D1524 | **The Edge executor (`edge3`) is the baseline** (DX0) and the fallback; the Resonate executor follows in DX1A | Approved (owner, 2026-10-21) |
| D1525 | **`LoamsResonateExecutor`** (§9.3): workloads as Resonate promises `af:…` with tasks and leases; tasks report their own state, the executor only infrastructure outcomes; adoption by tag after restart; short-lived JWT and mTLS; replaces `LoamsJobsExecutor` as the first executor (amends D1257) | Approved (owner, 2026-10-21) |
| D1526 | **`loams-worker`**, a generic Rust Resonate task runner in the monorepo: claims from a group, runs the command in `loams-sandbox`, heartbeats, reports outcomes and CPU time | Approved (owner, 2026-10-21) |
| D1527 | **Provider additions** to D1257: `LoamsResonateExecutor`, `LoamsPostgresHook`, Temporal hook, operator and trigger, Resonate promise operator, sensor and trigger, remote logging, an AIP-103 state store (verify), OTel, an Authentik and Loams auth manager | Approved (owner, 2026-10-21) |
| D1528 | **Rust tasks through AIP-108** only after upstream leaves experimental, prototyped in `ostrium-labs/loams-airflow` | Approved (owner, 2026-10-21) |
| D1529 | **Engines stay separate** with their own databases and Resonate stores, never a shared schema; one shared control plane (tenancy, identity, audit, metering, metrics, console) | Approved (owner, 2026-10-21) |
| D1530 | **Which-engine guidance** (§10) in the docs and the console's create flow | Approved (owner, 2026-10-21) |
| D1531 | **Metering:** `io.loams.dev.usage.durable.temporal.v1` for Temporal-style actions and history bytes; compat mode under `…companion.runtime.v1`; Airflow tasks under `…jobs.run.v1`; DBOS as ordinary Postgres | Approved (owner, 2026-10-21) |
| D1532 | **Metrics and analytics:** no tenant labels on metrics (D1021); run and task summaries exported as Iceberg tables in Lakekeeper and read in the House | Approved (owner, 2026-10-21) |
| D1533 | **Lifecycle CloudEvents** (opt-in) on Loams streams: `io.loams.dev.durable.temporal.workflow.{started,closed}.v1`, `io.loams.dev.durable.airflow.task.finished.v1` | Approved (owner, 2026-10-21) |
| D1534 | **Phases DX0–DX5** (§14) with the fork tasks alongside; plan [DX1](../plans/2026-10-21-dx1-durable-execution-compat.md) (stub) fully specifies DX0, which shares OC0's harness and Tasks 6 and 7 | Approved (owner, 2026-10-21) |
| D1535 | **Not built:** §16's list; amends D1272 by removing "a Temporal-compatible frontend" | Approved (owner, 2026-10-21) |
| D1536 | **Licence and provenance:** `temporalio/api` protos vendored at a pinned tag with the MIT text and a NOTICE entry; Temporal server code read, never copied; all new code Apache-2.0 | Approved (owner, 2026-10-21) |
| D1537 | **Amendments to §68** as tabled in §17, each marked owner direction 2026-10-21 | Approved (owner, 2026-10-21) |
| D1538 | **CI:** GitHub-hosted runners only; pure tests and the corpus differential per PR (path-filtered); SDK replayers, end to end, simulation, Airflow and DBOS suites nightly | Approved (owner, 2026-10-21) |
| D1539 | **Design-partner framing:** the fork work is offered to resonatehq; the `loams-temporal-*` crates have no Loams-internal dependencies beyond the gateway's traits, so a standalone `loams-temporal` binary can serve any Resonate server | Approved (owner, 2026-10-21) |

## 19. Open questions (Q1150–Q1165), each with a default

| ID | Question | Default (Q1150–Q1164 answered by the owner on 2026-10-21: the default; Q1165 open) |
|---|---|---|
| Q1150 | How do Temporal namespaces map to Loams namespaces? | Many Temporal namespaces per Loams namespace; the credential gives the Loams namespace, the request the Temporal one; promise IDs use both IDs |
| Q1151 | Does the frontend run as a `loams-gateway` module or a separate binary? | A module (`temporal`, feature `temporal`) in the `loams-gateway` binary, deployed as its own Deployment; a standalone `loams-temporal` build target from the same crates |
| Q1152 | One gateway database per region, or per Loams namespace? | One per region with namespace columns and 64 shards; a large tenant can get its own later |
| Q1153 | Should the Temporal crates move to the Resonate fork or upstream, for Resonate Cloud? | No for now; they stay in Loams under Apache-2.0 with no Loams-internal dependencies; revisit if ResonateHQ asks to co-maintain them |
| Q1154 | What if resonatehq declines a fork task? | Keep it as a carry on the integration branch, with the reason in `CARRIES.md`; rethink the fork strategy past ten carries |
| Q1155 | Which TiKV client does FW1 use in the fork? | `ostrium-labs/client-rust` at Loams' pinned revision until its fixes are released upstream; the upstream Resonate PR switches to crates.io `tikv-client` once a release has them |
| Q1156 | Sticky task queues in the MVP? | No (owner draft); a no-op `ResetStickyTaskQueue`; in DX3 |
| Q1157 | `LISTEN`/`NOTIFY` or polling for DBOS on Loams Postgres? | `LISTEN`/`NOTIFY` on the session or direct endpoint; polling recommended where a project scales to zero |
| Q1158 | Which Airflow auth manager for Authentik? | FAB with OAuth against Authentik in DX0's recipe; a `LoamsAuthManager` over Loams principals in DX4 |
| Q1159 | Where does the worker agent live? | The monorepo (`crates/loams-worker`), as a generic Resonate task runner that Loams Jobs can reuse |
| Q1160 | What happens to `LoamsJobsExecutor` (D1257)? | Deferred; if Loams Jobs runs on Resonate when JB1 lands, it becomes an alias of `LoamsResonateExecutor` |
| Q1161 | Temporal Schedules: Resonate schedules or the gateway's own? | Resonate schedules that create `start:` promises, which the gateway turns into starts (DX4) |
| Q1162 | Visibility queries: how much of Temporal's list-filter grammar, and on what? | The full grammar on Postgres (`jsonb` and generated columns for search attributes); no Elasticsearch |
| Q1163 | Retention of closed runs? | The gateway deletes closed runs after the namespace's retention (default 3 days, verify against Temporal's default); FW5 prunes their promises; until FW5 lands, promises accumulate and are counted |
| Q1164 | Publish the Temporal compat matrix on loams.dev, and offer it to ResonateHQ as a shared conformance page? | Yes, after DX2's exit |
| Q1165 | What are the design-partner terms with ResonateHQ (shared roadmap, review turnaround for fork PRs, co-maintenance of the TiKV store, Resonate Cloud use of the Temporal crates)? | The owner decides; engineering's input is §7.4 and D1539 |

## 20. Sources (read 2026-10-21)

- **Temporal:** [temporalio/api](https://github.com/temporalio/api) (MIT), [api-go](https://github.com/temporalio/api-go) (MIT), [server](https://github.com/temporalio/temporal) (MIT) and [v1.32.1](https://github.com/temporalio/temporal/releases/tag/v1.32.1) (2026-10-08); [persistence](https://docs.temporal.io/temporal-service/persistence); [`plugin_names.go`](https://github.com/temporalio/temporal/blob/main/common/persistence/sql/sqlplugin/plugin_names.go); MySQL plugin [`execution.go`](https://github.com/temporalio/temporal/blob/main/common/persistence/sql/sqlplugin/mysql/execution.go) and [`shard.go`](https://github.com/temporalio/temporal/blob/main/common/persistence/sql/sqlplugin/mysql/shard.go); SDK releases: [Go](https://github.com/temporalio/sdk-go/releases), [TypeScript](https://github.com/temporalio/sdk-typescript/releases), [Java](https://github.com/temporalio/sdk-java/releases), [Python](https://github.com/temporalio/sdk-python/releases), [.NET](https://github.com/temporalio/sdk-dotnet/releases), [Ruby](https://github.com/temporalio/sdk-ruby/releases), [Rust `sdk-core`](https://github.com/temporalio/sdk-core/releases) ([v1.0.0 GA](https://github.com/temporalio/sdk-core/releases/tag/v1.0.0)); [CLI](https://github.com/temporalio/cli/releases) and [`server start-dev`](https://docs.temporal.io/cli/command-reference/server); [TiDB `SELECT`](https://docs.pingcap.com/tidb/stable/sql-statement-select) (via §68).
- **DBOS:** [dbos-transact-py](https://github.com/dbos-inc/dbos-transact-py), [-ts](https://github.com/dbos-inc/dbos-transact-ts), [-golang](https://github.com/dbos-inc/dbos-transact-golang), [-java](https://github.com/dbos-inc/dbos-transact-java), [-rust](https://github.com/dbos-inc/dbos-transact-rust); [configuration (`use_listen_notify`)](https://docs.dbos.dev/python/reference/configuration); [hosting Conductor](https://docs.dbos.dev/production/hosting-conductor), [Conductor licence](https://dbos.dev/conductor-license).
- **Airflow:** [release notes](https://airflow.apache.org/docs/apache-airflow/stable/release_notes.html), [PyPI](https://pypi.org/pypi/apache-airflow/json), [prerequisites](https://airflow.apache.org/docs/apache-airflow/stable/installation/prerequisites.html), [executors (3.3.2)](https://airflow.apache.org/docs/apache-airflow/3.3.2/core-concepts/executor/index.html), [language SDKs (3.3.2)](https://airflow.apache.org/docs/apache-airflow/3.3.2/authoring-and-scheduling/language-sdks/index.html), [edge3](https://pypi.org/project/apache-airflow-providers-edge3/); AIP pages on [cwiki.apache.org](https://cwiki.apache.org/confluence/display/AIRFLOW) (AIP-69, -72, -103, -105, -108).
- **Resonate:** [resonatehq/resonate](https://github.com/resonatehq/resonate) (Apache-2.0; monorepo PR #1154, 2026-09-22; v0.9.8 2026-06-04), [core README](https://github.com/resonatehq/resonate/blob/main/impl/server/core/Readme.md), [resonatehq.io](https://resonatehq.io), [Resonate Cloud](https://resonatehq.cloud), [durable-promise-specification](https://github.com/resonatehq/durable-promise-specification) and [distributed-async-await.io](https://github.com/resonatehq/distributed-async-await.io); the fork [ostrium-labs/resonate](https://github.com/ostrium-labs/resonate) (branches read 2026-10-21; `spec/` and `impl/server/core` read at `28dfd01` from a shallow clone); upstream PRs [#1162](https://github.com/resonatehq/resonate/pull/1162), [#1163](https://github.com/resonatehq/resonate/pull/1163), [#1164](https://github.com/resonatehq/resonate/pull/1164), [#1167](https://github.com/resonatehq/resonate/pull/1167), [#1168](https://github.com/resonatehq/resonate/pull/1168).
- **Loams** at `origin/dev` `b281c2dde`: designs §14, §21, §31, §38, §46, §51, §55–§58, §62–§69; plan OC1; `crates/loams-durable`, `crates/loams-gateway`, `Cargo.toml`'s Resonate and `tikv-client` pins; PRs #796, #973, #979, #981.
