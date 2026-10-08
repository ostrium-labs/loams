# 45 — Loams Live: Production Readiness

Status: **Proposed** · 2026-10-08. The owner's goal for this document is "Loams Live production ready". It defines what that means for Loams Live ([§20](20-reactive-database-on-tikv.md)), in concrete, testable terms, and records the design choices that close the gap. Decisions are **D680–D699** and open questions **Q625–Q639**. Both are in the [decision log](13-decision-log.md), and every one of them is **Proposed** until the owner confirms it. The implementation plan is [LV1](../plans/2026-10-08-lv1-live-production.md). **No code is written by this document.**

This document amends §20 for production. Where the two disagree, this document wins for the topics it covers (§19 below lists each amendment). §20 stays the reference for the data model, the key layout, the journal, reactivity and the TiKV facts.

Markers: **(estimate)** means computed or guessed, not measured. **(target)** means a performance number that LV1 measures and then gates on (§15). **(verify)** means not checked against a primary source; the task that depends on it checks it.

---

## 1. Summary

| # | Decision | Section |
|---|---|---|
| D680 | "Production ready" means the LV1 exit checklist passes on a release candidate. Track **LV** runs beside R: LV1 finishes R1's unbuilt tasks and pulls the production-critical parts of R2 and R3 forward | §2, §18 |
| D681 | Functions run in QuickJS with **two isolation modes**. `in_process` serves dev and single-tenant self-hosting. `isolated` runs one sandboxed worker subprocess per app and is mandatory when a cluster is multi-tenant | §3.1 |
| D682 | Function, bundle, argument and log limits join the one limits table | §3.2 |
| D683 | **Versioned, immutable deployments.** Activation is a compare-and-swap of the catalog pointer. Rollback activates an earlier deployment through the same path. Subscriptions are keyed by deployment | §3.3 |
| D684 | Schema validators are enforced on every write. A deploy that tightens a validator scans the existing data and fails on a violation. Migrations are user mutations run in batches by the CLI | §4.1, §4.3 |
| D685 | **Online index changes** use two phases and wait out the maximum mutation lifetime. This replaces R1's empty-table rule and its per-node deploy quiesce | §4.2 |
| D686 | **Two kinds of principal.** Platform principals (Loams tokens and API keys) do admin, deploy and system access, with authorization per app and per table. App end users bring OIDC tokens from issuers configured per app and may call only public functions. Live consumes the unified auth plan's verifier and never issues tokens | §5 |
| D687 | Session identity uses a new `Authenticate` RPC and the `identity` part of `StateVersion`. Expired or revoked sessions are closed. Watch streams rotate every 24 h. `SubKey` gains the identity and the deployment | §5.4 |
| D688 | Live moves onto the **main Connect port** (D600), behind its TLS, auth and CORS layers. Apps are addressed by the `loams-live-app` header. The 7710 listener is deprecated for one release | §6 |
| D689 | A new **`LiveAdminService`** holds app lifecycle, deploy, rollback, logs and backups. `LiveService.Deploy` is removed | §6.3 |
| D690 | **Many apps per node.** The app directory lives in a system keyspace, each app gets a dedicated keyspace named by its id, and per-app engines start lazily. Shared keyspaces stay in R2 | §7 |
| D691 | **A store seam, `loams-kv`, with an embedded MVCC backend on redb** beside the TiKV backend. `loams-live` depends on the seam only | §8 |
| D692 | `loams dev` and Loams Desktop run Live on the embedded store by default. Cargo feature `live` (embedded) joins `standard`, and `live-tikv` joins `full`. Production means TiKV | §8.4 |
| D693 | **HA topology** is 3 PD, at least 3 TiKV across zones, and at least 2 stateless Live nodes behind a load balancer without stickiness. Session calls are forwarded to the node that owns the session. Catalog changes reach every node through the journal | §9 |
| D694 | **Per-app logical backup**: a snapshot plus a continuous journal export to the app's bucket, with point-in-time restore into a new app. It works on both backends. BR is the cluster disaster-recovery path | §10 |
| D695 | Prometheus metrics, OTLP traces and structured logs, with bounded labels and no document contents. Function console output is captured and tailable | §11 |
| D696 | **Quotas** per app and per principal, enforced with token buckets. Over a limit the answer is `RESOURCE_EXHAUSTED`, reason `live_quota_exceeded`, with `RetryInfo` | §12 |
| D697 | **SDKs:** the `@loams/live` reactive client, `@loams/live-react` hooks and the `@loams/live-cli` deploy tool. Other languages get generated stubs only in LV1 | §13 |
| D698 | **Release gating:** Live is labelled production only when every checklist row passes, including the security review, a restore drill, the performance gates and a stable `loams.live.v1`. Otherwise it ships labelled "preview" | §18 |
| D699 | Function API additions: `ctx.auth`, cursor `paginate`, public and internal functions, argument validators. Actions and scheduled functions stay in R2 (Q625) | §3.4 |

## 2. What "production ready" means

### 2.1 Where Live is today (checked in code on `dev`, 2026-10-08)

| Area | As built | Gap |
|---|---|---|
| Reactive core | R1 Tasks 1–12: `loams-tikv`, `loams-meta-tikv`, `loams-live` (values, ids, catalog, documents, journal and tailer, `LiveTxn`, runner, subscriptions, sessions, sync service) | R1's gates (Task 16) were never run |
| Functions | Only the six `_system:*` functions (`crates/loams-live/src/system.rs`). `Deploy` answers `UNIMPLEMENTED` (`deploy::DEPLOY_UNIMPLEMENTED`). There is no `loams-live-js` crate, and `rquickjs` is not in the workspace | R1 Task 13 is unbuilt |
| Schema | Tables are created on first insert or by `define_table`. Indexes can change on empty tables only (R1 Ruling 5). There are no validators | No validators, no backfill |
| Correctness gates | `tests/{docs,journal,service,session,subs,txn,value}.rs`. No `reactive_checker.rs` or `txn_checker.rs`, no `scripts/tikv/nemesis.sh`, no `r1-exit-report.md` | R1 Task 16 is unbuilt |
| Listener | Its own axum listener on `127.0.0.1:7710`. `check_listen` refuses any non-loopback address. The main port registers `LiveAbsent` (`feature_not_in_variant`), and `GetInstance` reports `loams.live.v1` as `available: false, unstable: true` | No auth, no TLS, not on the main port |
| Auth | None. No `Authorizer`, `ControlStore` or token verifier exists in the engine yet | D111, Q30 |
| Tenancy | One app per process (`--live-app`, default `dev`, keyspace `loams_live_<app>`). `SubKey` is (function, argument digest) | No app directory, no identity in the cache key |
| Build | Cargo feature `live = ["tikv", "dep:loams-live"]`, off by default (owner ruling T8-2). It needs the git-pinned `tikv-client` fork. With the feature on, `loams dev` needs a PD at `127.0.0.1:19379` or `--no-live` | Dev needs TiKV |
| Durability | Commit mode `two_pc`, 64 journal shards, 16 mutation attempts. The cluster GC loop runs. No backup exists (D131's rule is unimplemented, and Q37 is open) | No backup or restore |
| Observability | `tracing` logs. `SubsStats.missed_invalidations` is a counter in memory. There is no metrics registry in the engine | No metrics, no traces |
| SDK | `sdks/typescript/packages/live` holds generated stubs only (`src/gen`, `index.ts`) | R1 Task 14 is unbuilt |
| Users | No app uses Live. AP1e Task 20 adds `_system:tables` and Task 24 a Live page (D670) | — |

### 2.2 The definition (D680)

Loams Live is **production ready** when an operator can do all of the following, and the claim is backed by the gates of §14–§18:

1. **Run** it highly available on TiKV, with no single point of failure (§9).
2. **Expose** it beyond loopback, behind TLS, with authenticated and authorized principals and with isolated tenants (§5–§7).
3. **Ship** application code: deploy TypeScript functions and schemas, roll back, change indexes on live data and see function logs (§3–§4).
4. **Recover:** restore an app to a point in time, and recover a cluster from object storage (§10).
5. **Operate** it: metrics, traces, logs and alerts; quotas that protect the cluster; a runbook (§11, §12, §17).
6. **Build** on it with a supported client: a TypeScript reactive client with reconnect and resume, React hooks and a deploy CLI (§13).
7. **Trust** it: the reactive and transaction checkers pass under faults, the nemesis runs green for 24 hours, the performance numbers are measured and gated, and a security review is complete (§14–§16).
8. **Depend** on the contract: `loams.live.v1` loses `unstable` and is covered by `buf breaking` (§18).

Development on a laptop or in Loams Desktop needs none of TiKV (§8).

### 2.3 Scope against the R track

LV1 builds R1's unbuilt Tasks 13, 14, 16 and 17. Task 15 (TiDB) is moot under D260. LV1 also pulls forward the parts of R2 and R3 that production cannot do without:

- from R2: online index backfill, multi-node sessions, backup with a tested restore, Live quotas, and `gc_blocked_seconds`;
- from R3: auth on the Live API, and React hooks.

R2 keeps the namespace router's shared keyspaces and size classes, app moves, the `ControlStore` on `_control` (D125), actions and scheduled functions (Q625), serializable range reads (Q31), and the Python and Go reactive layers. R3 keeps the collections bridge (D129) and the Swift and Kotlin clients. R4 keeps tidb-operator and BYOC.

## 3. Functions and Deploy

### 3.1 The sandbox: two isolation modes (D681)

The engine stays QuickJS through `rquickjs` (D120). The open question of engines (Q35) is not reopened here. The runtime model of §20 §6.3 stands: one runtime per (node, app, deployment), a pool of pre-evaluated contexts, one context per invocation, frozen globals, the interrupt handler for CPU, and the memory limit per runtime.

What changes is **where the runtime runs**:

| Mode | Where | When | Trust boundary |
|---|---|---|---|
| `in_process` (the default) | Inside the `loams` process | `loams dev`, Desktop, and single-tenant self-hosting, where the operator deploys their own code | The operator's own code. A QuickJS escape gains the operator's own process |
| `isolated` | One worker subprocess per (node, app): `loams live-worker`. Host calls go over a framed protobuf protocol on the stdio pipes | Required when `[live] tenancy = "multi"`. A cluster that serves more than one org refuses to start Live in `in_process` mode | Kernel-enforced. On Linux: seccomp-bpf (an allowlist with no `socket`, `open*` after start-up, `execve` or `ptrace`), landlock (no filesystem access), `RLIMIT_AS`, `RLIMIT_CPU`, `PR_SET_NO_NEW_PRIVS`, and a separate uid where the operator configures one. On macOS and Windows, `isolated` is unavailable and multi-tenant mode refuses to start |

- **Why not only in-process.** QuickJS is a C interpreter with a history of memory-safety bugs. In-process is fine when the code is the operator's own. It is not fine for running strangers' code next to other tenants' keys and data.
- **Why not gVisor or Firecracker now.** A subprocess with seccomp and landlock gives a kernel boundary for about 10–50 µs per host call (estimate, measured in LV1a). MT2 already runs gVisor for Knative functions. Whether multi-tenant Loams Cloud needs that stronger level for Live is Q628.
- **Worker failure.** If a worker crashes or is killed, the invocation fails with `FUNCTION_ERROR` (reason `live_worker_crashed`). A mutation is not retried after a worker crash, because its transaction never committed, so no side effect is lost. The worker is respawned with backoff. Five crashes in a minute mark the app's deployment `degraded` and raise an alert.

### 3.2 Limits (D682)

These limits are added to R1's `Limits` (`crates/loams-live/src/limits.rs`). All values are configurable per app (§12) and are rendered into the published limits page (D88):

| Limit | Default |
|---|---|
| Bundle size | 16 MiB (estimate) |
| Exported functions per bundle | 2 048 |
| Arguments of one call (encoded) | 4 MiB |
| Result of one call (encoded) | 8 MiB |
| JavaScript CPU per call (queries and mutations) | 1 s (R1) |
| Wall-clock deadline of a mutation, which is also its maximum lifetime (§4.2) | 10 s (R1) |
| Runtime memory | 64 MiB (R1) |
| `console.*` output per call | 64 lines × 4 KiB; the rest is dropped and counted |

### 3.3 Versioned deployments and rollback (D683)

- **A deployment is immutable.** It records `{id (ULID), bundle_sha256, schema, created_at, created_by (principal id), message, parent_id, state}`. `state` is one of `pending`, `validating`, `backfilling`, `active`, `superseded`, `failed` or `degraded`. The bundle is stored with `loams-store` at `live/<app>/deployments/<id>.js` (R1 Ruling 10). The record lives in the app's catalog.
- **Activation is a compare-and-swap** of the app's deployment pointer. `Deploy` carries `expected_active` (the deployment id the client built against). If another deploy activated in between, the call fails with `FAILED_PRECONDITION`, reason `live_deploy_conflict`. `dry_run` validates the bundle and diffs the schema without writing anything.
- **Order of a deploy:**
  1. Validate the bundle: load it in a scratch runtime and check its exports.
  2. Store the bundle and check its hash.
  3. Write the `pending` record.
  4. Apply the schema changes (validators and indexes, §4), which can move the deployment through `validating` and `backfilling`.
  5. Swap the pointer.
  6. Write a `catalog` journal entry so every node reloads (§9.3).
- **Rollback is activating an earlier deployment** (`ActivateDeployment{id}`). It goes through the same path, so the earlier schema is re-applied: an index that the later deployment added is dropped, and one it dropped is backfilled again. There is no second, special-case path to get wrong.
- **Integrity.** A node loading a bundle checks its SHA-256 against the record. A mismatch refuses the load, alerts, and keeps the previous deployment serving.
- **Retention.** The last 50 deployments are kept (Q636). Older bundles are deleted by the janitor. The active one and its parent are never deleted.
- **Subscriptions.** `SubKey` gains the deployment id. At the activation tick every subscription of the app reruns once on the new code. Sessions see one Transition at that tick, never a mix of old and new results.
- **Logs.** `console.*` output of each invocation is captured with its request id and function into a per-app ring buffer on each node (10 000 lines, Q636). `LiveAdminService.TailLogs` streams it, merged across nodes. It is not durable in LV1. Durable function logs go to the M2 OTLP logs pipeline (D73) when it exists.

### 3.4 The function API (D699)

R1 Task 13's `loams:server` surface stays (`query`, `mutation`, `ctx.db.get/query/insert/patch/replace/delete`, `withIndex`, `order`, `take`, `collect`, `first`), with these additions:

- **Visibility.** `query` and `mutation` are **public**: end users may call them (§5.3). `internalQuery` and `internalMutation` can be called only by platform principals with `live:call` on the app and the `internal` flag on the call. A function's kind and visibility come from the bundle's export metadata. The host checks them before the handler runs.
- **Argument validators.** `args: { name: v.string(), n: v.optional(v.int64()) }`, with the same `v.*` vocabulary as schema validators (§4.1). The host checks the arguments in Rust before any JavaScript runs. A failure is `INVALID_ARGUMENT` with field violations.
- **`ctx.auth.getUserIdentity()`** returns `null` or `{ issuer, subject, tokenIdentifier, email?, name?, claims }` (§5.3).
- **Pagination.** `ctx.db.query(t).withIndex(…).paginate({ cursor, numItems })` returns `{ page, continueCursor, isDone }`. The cursor is opaque and authenticated: it is the encoded last index key plus an HMAC under a per-app key, so a client cannot forge a cursor into another range. The read set ends at the last key returned (§20 §8.1), so inserts after the page do not invalidate it.
- **Actions** (`fetch`, the AI gateway) and **scheduled functions** stay in R2 (§20 §6.1). Q625 asks the owner whether LV1 should include them. LV1's answer for side effects is the client or the app's own backend calling `Mutate` with an API key.

## 4. Schema, indexes and migrations

### 4.1 Validators (D684)

- The deployed `Schema` gains a validator per table: `v.object({...})` over `v.null|int64|float64|boolean|string|bytes|array|object|literal|union|optional|any|id(table)`. `id(table)` checks the id's checksum and table (§20 §4.1). Validators are compiled to a Rust checker and enforced on **every write**, including `_system:*` writes. A violation is `INVALID_ARGUMENT`, reason `live_schema_violation`, naming the field path.
- **Tightening is validated against existing data.** A deploy whose validator is stricter than the active one first sets the validator to `enforcing` for new writes. It then scans the table at a timestamp after that state commits. The scan runs as a backfill-style job (§4.2), and the deployment is `validating` while it runs. Any violation fails the deployment, reverts the validator, and reports up to 100 offending ids. A table can opt out with `schemaValidation: false`, as in Convex. Then writes are still checked, but existing data is not.
- **Loosening** (a new optional field, a widened union) applies at once with no scan.

### 4.2 Online index changes (D685)

R1 refuses index changes on non-empty tables (Ruling 5). Row T9-1 refuses an index deploy while mutations are in flight, using `Runner::try_quiesce`, which is per node. Neither works in production: tables are never empty, and several Live nodes serve one app. Index changes therefore become **two-phase with a lifetime wait**, the standard schema-change argument (as in F1's online schema change), adapted to TiKV timestamps:

1. **`write_only` at `T_w`.** The deploy commits the index definition in state `write_only`. Every mutation that starts after `T_w` reads the table definition in its transaction, sees the index and maintains its entries. Queries cannot use the index yet: `FAILED_PRECONDITION`, reason `live_index_not_ready`.
2. **The lifetime rule.** Before prewrite, a mutation fetches a TSO timestamp and refuses to commit if more than `max_mutation_lifetime` has passed since its start timestamp, measured on TSO physical time. The refusal is retryable and invisible to the caller unless the deadline is gone. So after `T_w + max_mutation_lifetime` no mutation that read the old definition can still commit. The backfill waits **twice** that lifetime (20 s by default) as a margin against TSO and clock jumps.
3. **Backfill.** A job walks the table's document range in batches of 256. Each batch is its own transaction: it reads the batch's documents at its start timestamp, `lock_keys` the document keys, and writes their index entries. A concurrent write to one of those documents conflicts, and one side retries, so no batch writes an entry for a value that has since changed. Index keys are deterministic, so writing an entry twice is idempotent. The job checkpoints its last document key in the catalog. A node crash resumes from the checkpoint, and the job is leased like any worker task.
4. **`ready`.** The index becomes queryable, and the deployment continues to activation.
5. **Drops.** The index goes to `dropping`, so queries refuse it at once. After the lifetime wait, its entry range is deleted in batches, and the definition is removed.

**Why this is correct.** Every document key present at the backfill's batch timestamp gets its entry from the batch. Every write after `T_w` maintains the index itself. A mutation that started before `T_w` and commits before the backfill starts is seen by the batch read. Rule 2 guarantees that no such mutation commits after the backfill starts. The reactive checker (§14) runs with index changes in the middle of its workload to check this.

**Cost.** Backfill throughput is a gate (§15). It runs at a bounded rate (`backfill_docs_per_s`, default 5 000) so that foreground p99 latency stays within 2× of its baseline.

### 4.3 Migrations

Data migrations are **user mutations**. `loams-live migrate run <module:fn> --table t --batch 100` (in `@loams/live-cli`) calls an `internalMutation` repeatedly with a cursor until it returns `isDone`. Progress is printed and can be resumed with `--cursor`. There is no server-side migration framework in LV1: scheduled functions (R2) would host one. The documented pattern is the usual one: widen the validator, deploy, migrate, then tighten the validator, which §4.1's scan checks.

## 5. Authentication and authorization (D686, D687)

### 5.1 How Live plugs into the unified auth plan

The unified auth plan (D111, Q30) was narrowed by D451: MT1 is the identity half, and **each listener adopts MT1's verifier in its own plan**. LV1 is that plan for Live. Live **verifies** credentials and **never issues** them.

| Piece | Owner | What Live does with it |
|---|---|---|
| Token endpoint, sessions, users, API key issuance and revocation, the `ControlStore` records | §19 M2 identity work, MT1 | Consumes: it verifies tokens and keys and subscribes to revocations |
| Authentik as the identity provider, RFC 8693 exchange, groups mapped to teams and OpenFGA tuples | MT1 (D447–D452) | Nothing directly. People reach Live with exchanged Loams tokens. Authentik is also the documented default issuer for app end users (§5.3) |
| `Authorizer` (D66: `AllowAll`, then RBAC, then OpenFGA in M2.x) | M2 | Calls `check(principal, action, resource)` for every admin call and every `_system:*` call |
| The verifier core in `crates/loams-auth`: Loams JWT verification against the instance JWKS, API key hash lookup, the revocation set | MT1 Task 2 or M2. **If it has not merged by LV1b, LV1b builds only this core** (verification, no issuance), with the interfaces MT1's plan names, so MT1 and M2 extend it rather than duplicate it | Uses it for every request |

### 5.2 Platform principals

- **Credentials.** `Authorization: Bearer <Loams access token>` (an EdDSA JWT, §19 §5.3), or an API key `loams_<key_id>_<secret>` (§19 §5.5, §10 §4). Both are scoped to one environment (`env`), and environment equals namespace, which equals the Live app (§20 §4.1, Q639).
- **Scopes.** `live:call` (call public and internal functions), `live:read` and `live:write` (the `_system:*` functions), `live:deploy`, `live:admin` (app settings, backups, logs).
- **Authorization** goes through the `Authorizer`:

  | Resource | Actions |
  |---|---|
  | `live_app:<app>` | `call`, `deploy`, `admin`, `logs`, `backup` |
  | `live_table:<app>/<table>` | `read`, `write`, used by `_system:get/query/tables` and `_system:insert/patch/replace/delete`, so the Data Studio and support tooling get per-table access |

  The token's scopes narrow what the `Authorizer` grants (§19 §5.3). Both must allow the call. Whether `_system:*` writes stay available to principals with `write` on a table in production is Q638 (default: yes, audited).
- **Audit.** Deploys, rollbacks, app settings changes, backup and restore, and every `_system:*` write are audit events (§10 §4, D100). Until the `_audit` stream exists, they go to the structured log with `audit=true`.

### 5.3 App end users

An app's end users are not Loams principals. They sign in to the app's own identity provider.

- **Configuration.** The app's settings (`UpdateApp`) list trusted issuers: `{ issuer, audiences[], jwks_uri? }`. Authentik is the documented and tested default (D447). Any OIDC provider works: discovery through `/.well-known/openid-configuration`, and the JWKS cached for 10 minutes and refreshed on an unknown `kid` at most once per 30 s, MT1 Task 2's rules reused per app.
- **The credential.** The same `Authorization: Bearer` header. Live tells the kinds apart by token shape and `iss`: the instance's own issuer means a platform principal, an issuer configured on the addressed app means an end user, a `loams_` prefix means an API key, and anything else is `UNAUTHENTICATED`.
- **What an end user may do.** Call **public** functions of that app, through `Watch`, `Query` and `Mutate`. Never internal functions, `_system:*` or `LiveAdminService`. Authorization beyond that is the function's own code, using `ctx.auth`. Whether Live should also offer declarative per-table rules for end users is Q631 (default: no; function code is the policy, as in Convex).
- **Anonymous calls** are allowed to public functions when the app's `allow_anonymous` is true (the default for new apps is `false` in production and `true` in `loams dev`).

### 5.4 Sessions and identity (D687)

- `Watch` takes its identity from the `Authorization` header of the stream request. The identity's version is `StateVersion.identity`, which R1 already carries and never changes.
- **`Authenticate{session_id}`** is a new unary RPC. Its own `Authorization` header becomes the session's identity.
  - With the same subject (a refresh), it only extends the expiry.
  - With a different subject (sign-in or sign-out), it increments `identity` and reruns every query of the session at the next tick, and the client drops results that carry the old identity.
- **Expiry.** A session whose token expires without an `Authenticate` (60 s leeway, §19 §9) is closed with `UNAUTHENTICATED`, reason `token_expired`. The SDK refreshes and resumes (§13).
- **Revocation.** A revoked API key or a suspended principal closes its sessions within the revocation feed's latency (§19 §5.4).
- **Rotation.** A Watch stream is closed after 24 h with `UNAVAILABLE`, reason `session_rotate`, and the client resumes (Q637). This bounds how long a connection stays on one node and how long an identity goes without re-checking.
- **The cache key.** `SubKey` becomes (deployment, function, argument digest, identity), where identity is the end user's `tokenIdentifier`, the platform principal id, or `anonymous`. Sharing results across users is lost for authenticated queries. Q632 asks whether to recover it for functions that never read `ctx.auth`. Correctness comes first: a result computed for one user is never sent to another.

## 6. Network exposure (D688, D689)

### 6.1 The main port

Live is served on the **main Connect port** (`--listen`, D600) beside every other `loams.*.v1` service, by registering `LiveService` and `LiveAdminService` on the same connect-rust router that registers `LiveAbsent` today. It therefore inherits that port's:

- **TLS** (rustls) and the rule of MT1 Task 7: a non-loopback bind needs `[tls]` and `[auth]`. Without them startup fails with `<listener> listen on <addr>: non-loopback addresses need [tls] and [auth] (MT1)`. TLS terminated at a trusted reverse proxy is the port's `[tls] terminated_by_proxy` setting, not Live's.
- **Auth layer** (§5), **request limits**, **health** (`grpc.health.v1`), and `GetInstance`, which now reports `loams.live.v1` as `available: true` when the engine runs.
- **Protocols**: Connect, gRPC and gRPC-Web over HTTP/1.1 and HTTP/2. `Watch` stays a server stream (D420, §20 §7.1).

`--live-listen` stays for **one release**, deprecated, loopback only, serving the same services. It is then removed. The R1 refusal of non-loopback addresses on it stays until removal. Loams Desktop's proxy rule that sends `/loams.live.v1.` to a separate `liveUrl` (AP1e Task 24) then points at the engine URL. That is a one-line change for the desktop owner, noted in LV1's file list and not made by LV1.

### 6.2 Addressing an app

- The request header **`loams-live-app: <app>`** names the app.
- Without the header, the app is the one bound to the credential's environment.
- Without a credential (unauthenticated loopback dev only), it is the server's default app (`--live-app`, `dev`).
- **An environment-scoped credential can only address its own app.** A mismatch is `PERMISSION_DENIED`, reason `live_app_mismatch`. This is the tenant fence, and it is the first test of the authorization matrix (§14).
- Per-app hostnames (`<app>.live.<domain>`, as Convex's deployment URLs) are Q630. They would map a host to the header at the edge, so they need no protocol change.

### 6.3 `LiveAdminService` (D689)

`loams.live.v1.LiveAdminService` contains:

- **Apps:** `CreateApp`, `ListApps`, `GetApp`, `UpdateApp` (limits, issuers, allowed origins, `allow_anonymous`), `DeleteApp`.
- **Deployments:** `Deploy` (bundle, schema, `expected_active`, `message`, `dry_run`), `ListDeployments`, `GetDeployment`, `ActivateDeployment` (rollback).
- **Indexes:** `GetIndexStatus`, which reports backfill progress.
- **Logs:** `TailLogs`, a server stream.
- **Backups:** `CreateBackup`, `ListBackups`, `Restore`.

`LiveService.Deploy` is **removed**. The package is `unstable`, so no compatibility promise covers it, and §44 §10.3 lets an unstable package change. `LiveService` keeps `Watch`, `ModifyQuerySet`, `Query`, `Mutate` and gains `Authenticate`. In the facade, `loams.live` covers sessions, `loams.tables` covers `query` and `mutate`, and `loams.live.admin` covers the admin service (§44 §7.2).

### 6.4 Browsers, CORS and long streams

- **CORS.** Each app lists `allowed_origins`. A preflight cannot carry the app header's value, so the port answers it for the **union** of all apps' origins. The actual request, which carries the header, is then checked against **the addressed app's** list, and a mismatch is `PERMISSION_DENIED`. Credentials are bearer tokens, never cookies, so CSRF does not apply. `Access-Control-Allow-Credentials` is never sent.
- **Load balancers.** Heartbeats every 15 s (R1). The documented load-balancer idle timeout is at least 60 s, with HTTP/2 to the backends. A node being drained closes its streams with `UNAVAILABLE`, reason `node_draining`, so clients resume on another node (§9).

## 7. Apps and multi-tenancy (D690)

- **The directory.** A fixed system keyspace, `loams_live_system`, holds `apps/<id>`: `{name, namespace, keyspace, state, limits, issuers, allowed_origins, allow_anonymous, created_at}`, plus a name index. It is itself read through Live's own transaction layer. When D125 moves the `ControlStore` onto `_control` (R2), the directory moves there. Nodes watch the directory's change counter. An app's state is `active`, `disabled` (all calls `FAILED_PRECONDITION`, reason `live_app_disabled`) or `deleting`.
- **Keyspaces.** A new app gets a dedicated keyspace named by its cluster-unique id, `loams_live_a<id>`. Names are per org and can be renamed. Keyspace names are global and permanent. R1's name-based keyspace (`loams_live_<name>`) stays for `--live-app` dev apps. `CreateApp` calls `ensure_keyspace` (`loams-tikv`) before writing the directory record, so a crash leaves at worst an unused keyspace, which the janitor reports.
- **Deletion** is soft first: the app is `disabled` and its keyspace `DISABLED` in PD for 7 days. A backup is taken first by default. Then its range is deleted and the keyspace archived.
- **Per-app engines.** A node creates an app's runner, subscription manager, journal tailer and QuickJS runtime (or worker) on the first request for that app. They are stopped after 10 minutes with no session and no request. A node therefore serves many apps, with memory proportional to the active ones.
- **Isolation** between apps:
  - separate keyspaces, so separate GC and separate data;
  - separate runtimes or workers (§3.1);
  - per-app quotas (§12);
  - per-app keys for cursor HMACs;
  - app-labelled metrics.
- **Scale.** Shared keyspaces with app prefixes (§20 §9.2) stay in R2. LV1 measures how many dedicated keyspaces one cluster sustains (Q36). The measured number becomes `max_apps_per_cluster`, enforced by `CreateApp` (§15).

## 8. Local and dev mode without TiKV (D691, D692)

### 8.1 The problem

With the `live` feature on, `loams dev` connects to a PD at `127.0.0.1:19379` and fails without one, unless `--no-live` is passed. A TiKV playground needs about 3.2 GB of RSS and a ~500 MB download (§20 §14), and TiKV does not run natively on Windows. Loams Desktop therefore starts the engine with `--no-live` unless the TiKV compose stack is up (D670). Live cannot be the default for development, which is where developers first meet it.

### 8.2 Options

| Option | How | For | Against | Verdict |
|---|---|---|---|---|
| **A. TiKV playground as a sidecar** | `tiup playground` (or the compose stack) supervised by the engine or the desktop | Exactly the production engine | ~3.2 GB RSS, ~500 MB download, a minute to start, no native Windows TiKV, two more processes to supervise. Unacceptable as a desktop default | Keep as the opt-in for fidelity testing (`--live-store tikv://…`) |
| **B. `tikv-client`'s mock** | The client's `MockPdClient` and `MockKvClient` | No new store | It mocks RPCs for unit tests. It has no MVCC, no transactions, no conflicts and no persistence. Live's semantics would be fiction | Rejected |
| **C. TiKV linked as a library** | Build TiKV's engine in-process | Real engine | No supported embedding API, a RocksDB and C++ build that the one-build machine cannot afford, and a fork against D126 | Rejected |
| **D. Embedded MVCC on redb** | A `KvStore` backend implementing snapshot isolation over redb 4, a pure-Rust B-tree already in the workspace (openraft's log) | No new dependency. Pure Rust. Builds on Windows, macOS and Linux. Single-writer ACID with fsync. Range scans in key order. Small (estimate: 1 500–2 500 lines) | A second implementation of the transaction semantics, which can drift from TiKV's. Single process, no replication | **Chosen** |
| E. Embedded MVCC on fjall | Same design on an LSM | Better write throughput | A new dependency. LSM compaction tuning matters little at dev scale | Second choice if redb's write path measures too slow |
| F. MVCC table on SQLite | Same design on `rusqlite` | SQLite's maturity | A C dependency for one more crate. Slower ordered range scans over blobs (estimate) | Rejected |
| G. SlateDB on the bucket | Live data as an LSM on object storage, D1-style | Would remove D130's tension for single node | Commit latency equals object-store PUT latency (tens of ms) unless it is on a local filesystem. Its transaction support is unverified (verify). A third engine to qualify | Not for LV1; a note for single-node production (Q626) |

### 8.3 The chosen design

- **The seam: `loams-kv`.** A new crate holds what `loams-live` needs from a store:
  - `KvStore` (`now()`, `begin()`, `snapshot(ts)`, `run(opts, body)` with the runner's retry and error classes, keyspace and root);
  - `KvTxn` (`get`, `batch_get`, `scan`, `scan_reverse`, `put`, `insert`, `delete`, `lock_keys`, `start_ts`);
  - `KvSnap`;
  - `Ts` (a `u64` in TSO layout: physical ms << 18 | logical);
  - the order-preserving tuple codec, moved from `loams-tikv`;
  - `TxnError`, `FaultPlan` and `GcBarrier`.

  Dispatch is static, through `enum Store { Embedded(..), Tikv(..) }`, so there is no `async-trait` boxing on the hot path. The `Tikv` variant exists only with the feature `tikv`. `loams-live` depends on `loams-kv` alone. `loams-meta-tikv` keeps `loams-tikv` (the metastore's dev default stays openraft).
- **The embedded backend:**
  - **Versions.** One redb file per data directory (`<data_dir>/live/store.redb`). The table `versions` holds `keyspace_id:u32 ‖ key ‖ !commit_ts:u64 → value | tombstone`.
  - **Timestamps.** A hybrid timestamp oracle: physical ms from the wall clock, never going back, with an 18-bit logical counter. Its high-water mark is persisted every 1 000 allocations or 1 s, and on restart it starts above the mark.
  - **Snapshots.** A redb read transaction plus a timestamp filter: the newest version with `commit_ts ≤ ts`.
  - **Commits.** Optimistic: buffer the writes and lock keys. At commit, under the store's commit lock, any written or locked key with a version `commit_ts > start_ts` is a `Conflict` (first committer wins, the outcome TiKV's prewrite gives). Otherwise take the commit timestamp and write every version in one redb write transaction.
  - **Group commit** batches the commits waiting on the lock into one redb transaction and one fsync.
  - **GC** deletes versions below `min(now − gc_life_time, oldest open snapshot, barriers)`.
  - **Outcomes.** A commit's outcome is always known: redb's transaction is atomic across a crash, and there is no network. `Undetermined` never occurs, and the commit token path is a no-op that the conformance suite still exercises.
- **Parity is tested, not assumed.** A `kv_conformance!` suite (snapshot isolation, first-committer-wins, `lock_keys` conflicts, scan order and bounds, reverse scans, timestamp monotonicity across restarts, GC behind barriers) runs against both backends. All of `loams-live`'s suites and the reactive and transaction checkers (§14) run on **both** backends. The embedded runs need no PD, so they run on every PR.

### 8.4 Scope (D692)

| Use | Backend |
|---|---|
| `loams dev`, Loams Desktop, SDK conformance, CI | **Embedded by default**: `--live-store embedded` (the default), or `--live-store tikv://<pd>/<keyspace>` |
| `loams standalone` | Embedded allowed and labelled **not for production** in LV1. Whether single-node production on embedded is supported, with mandatory backup (§10), is Q626 |
| Production (`cluster`, Loams Cloud, BYOC) | **TiKV only**, per D130 |

- **Features.** `live` = Live plus the embedded store, with no `tikv-client`. It **joins the `standard` and `full` variants and the default feature set**, which reverses owner ruling T8-2 for the embedded part. `live-tikv` = `live` plus the TiKV backend (the git-pinned fork). It joins `full`. Publishing a crate with a git dependency to crates.io is impossible, so how `live-tikv` reaches crates.io builds is Q629. Binaries built from the repository are unaffected.
- **Desktop.** With embedded Live on by default, the desktop no longer needs the TiKV stack for its Live page or `--no-live` (amends D670). The TiKV stack stays available for users who want the production engine. The desktop change itself belongs to the desktop owner.

## 9. HA and durability on TiKV (D693)

- **Topology.**
  - **PD:** 3 nodes.
  - **TiKV:** at least 3 stores across at least 3 zones, with location labels and `max-replicas = 3`, on API v2 with TTL (§20 §9.1). Pinned to the version the R1 plan pins (v8.5.8) until a bump reruns the suites.
  - **Live:** at least 2 nodes behind an L7 load balancer, without stickiness.
- **Durability.** A mutation is acknowledged after its TiKV commit, which Raft has replicated to a majority. RPO is 0 for any loss within quorum. Beyond quorum, §10 applies.
- **Live nodes are stateless** (§20 §3). Losing one means its clients resume elsewhere. RTO is the client's reconnect plus one rerun of its query set, with a target under 10 s for 10 000 sessions (§15).
- **Session calls on another node.** `session_id` carries the owning node's id (R1). A `ModifyQuerySet` or `Authenticate` that lands on another node is **forwarded** over the cluster listener (M1.3's request forwarding, on the internal port only). If the owner is gone, the call fails with `NOT_FOUND`, reason `session_gone`, and the client resumes. The load balancer therefore needs no session affinity.
- **Catalog propagation.** Deploys, index state changes, validator changes and app settings write a journal entry of kind `catalog`. Every node tailing the app reloads its catalog at that tick, so all nodes switch deployments at the same timestamp. This replaces R1's per-node assumption (row T9-1).
- **Draining.** `SIGTERM` stops admitting sessions, closes streams with `node_draining`, finishes in-flight mutations within their deadline, and exits. Readiness goes false first, so the load balancer stops routing.
- **Commit mode** stays `two_pc` until the checkers pass with `async_1pc` (R1 row F4, Q634).
- **The GC loop** keeps its lease-elected single runner (R1 Task 3). `gc_blocked_seconds` per keyspace and an alert on repeated failures move here from R2 (R1 row T3-12), because a stuck keyspace holds back the cluster safe point for every app.
- **Kubernetes.** PD and TiKV through tidb-operator v2 (D179). The Live role through Loams' chart (R4 scope). LV1 ships a reference compose topology and the Live role's chart values and probes. The operator work stays in R4.

## 10. Backups and restore (D694)

D131 makes backup to object storage mandatory for every Live cluster. Q37 (does BR's log backup cover a txn-API keyspace?) is still unanswered, and BR is cluster-wide while apps need per-app restore. So there are two layers:

| Layer | What | Restores | Works on |
|---|---|---|---|
| **Per-app logical backup** (primary) | A **snapshot** of the app's keyspace at a timestamp `S`, read under a GC barrier and written as size-bounded protobuf segments. Then a **continuous export**: a journal consumer (checkpointed, holding the janitor like the bridge, §20 §12) that, for each tick `T`, reads the documents named by the new journal entries at `T` and appends `{T, upserts, deletes, catalog changes}` to `live/<app>/backups/<backup_id>/changes/` | The app as of any exported tick ≥ `S`, into a **new app**. Then an atomic directory swap if asked (`Restore{replace: true}`) | Embedded and TiKV |
| **Cluster disaster recovery** | BR full backups (`br backup txn` or `raw` of the Live keyspaces, verify) and, if Q37 and Q627 confirm support for txn-API keyspaces, BR log backup | The whole cluster to a point in time | TiKV |

- **Recovery point.** The export flushes every 10 s or 8 MiB, so RPO ≤ 15 s (target). The PITR window is 7 days by default, with daily snapshots kept for 7 days (Q636).
- **Consistency.** A restore reproduces the app's documents, indexes (rebuilt from documents through the backfill path), catalog and active deployment, exactly as of tick `T`. The check is the reactive checker's state comparison: a fresh snapshot of the restored app equals the source's snapshot at `T`. This is the R2 gate of §20 §18, pulled into LV1.
- **Catalog changes in the journal.** A backup needs catalog changes in order with data. That is the same `catalog` journal entry that §9 uses.
- **Operating rule** (keeps D131's rule): a Live cluster serves external traffic only while every app's export is running (export lag is an alert past 60 s) and a restore drill has passed on that release. The drill is a nightly CI job and a documented runbook step.
- **The bucket** is the app's namespace bucket, or the operator's bucket for apps without one. Backups are encrypted by the bucket's encryption (SSE, or CMEK through D96 when it exists).

## 11. Observability (D695)

- **Metrics** (Prometheus, `/metrics` on the admin listener, §10 §5; if M2 has not built that listener, LV1 adds the registry and the route), all prefixed `loams_live_`:

  | Area | Metrics |
  |---|---|
  | Calls | `requests_total{app,rpc,code}`, `request_seconds{app,rpc}` |
  | Mutations | `mutation_attempts_total{app,result}` (committed, conflict, refused_lifetime), `mutation_commit_seconds{app}` |
  | Reactivity | `tick_lag_seconds{app}`, `journal_lag_entries{app}`, `reruns_total{app}`, `rerun_seconds{app}`, `missed_invalidation_total{app}` (R1 row T12-8; it alerts), `sessions{app}`, `subscriptions{app}`, `transition_bytes_total{app}` |
  | Functions | `function_cpu_seconds_total{app,kind}`, `function_errors_total{app,code}`, `worker_restarts_total{app}` |
  | Schema | `deploys_total{app,result}`, `backfill_docs_total{app}`, `backfill_remaining{app}` |
  | Operations | `quota_rejections_total{app,quota}`, `backup_export_lag_seconds{app}`, `gc_blocked_seconds{keyspace}`, `gc_safe_point_lag_seconds` |

  **Cardinality** is bounded. `app` is the only high-cardinality label, and per-function and per-table series are opt-in per app (`metrics.per_function = true`) and capped at the top 50.
- **Traces** (OTLP): one span per RPC, then per function invocation (attempt number, kind, deployment), then per host call batch, then per TiKV transaction (begin, prewrite, commit, with conflict and retry events), and the tick with its rerun spans linked to the commits that caused them.
- **Logs:** structured JSON with `request_id`, `trace_id`, `app`, `function`, `deployment` and `principal` (an id, never a token). **Never** document contents, arguments or results. A test greps for a canary value. Raw TiKV keys are scrubbed from errors (§20 §11.4).
- **Function logs:** §3.3. **Debug dump:** `GET /debug/dump` gains a Live section with apps, sessions per app, tick lag, backfills, workers and export lag.
- **Alerts** (shipped as Prometheus rules in `deploy/live/alerts.yaml`):
  - `missed_invalidation_total` increases;
  - tick lag above 1 s for 5 minutes;
  - backup export lag above 60 s;
  - GC safe point more than 1 h behind;
  - worker crash loop;
  - quota rejection rate above 5% for 10 minutes;
  - TSO errors.

## 12. Quotas and rate limits (D696)

| Quota | Scope | Default (estimate; owner-tunable) |
|---|---|---|
| Requests/s | per app, per principal | 1 000 / 200 |
| Mutations/s | per app | 500 |
| Concurrent mutations | per app per node | 64 |
| Sessions | per app per node | 10 000 |
| Subscriptions per session | per session | 1 000 (R1) |
| Reruns/s | per app per node | 2 000 |
| Function CPU | per app per minute | 120 CPU-s |
| Storage | per app | 10 GiB, soft (alert, then refuse inserts) |
| Documents | per app | 10 M, soft |

- Enforced by token buckets **per node**, so a cluster of `n` nodes admits up to `n` times the rate. That is documented, and exact global limits wait for the `ControlStore`'s quota service (D65).
- Limits come from the app directory (§7) and later from the `ControlStore`.
- Over a limit: `RESOURCE_EXHAUSTED`, reason `live_quota_exceeded`, `metadata.quota`, `RetryInfo`. The SDK honours `RetryInfo` (D610).
- Every quota and limit is in the one limits table, rendered into the limits page, with an at-the-limit test and a past-the-limit test (D88).

## 13. Client SDKs (D697)

- **`@loams/live`** (TypeScript, browsers and Node ≥ 22) implements R1 Task 14's contract:
  - one `Watch` per client;
  - versioned query-set changes;
  - resume on a version gap;
  - reconnect with jittered backoff (250 ms to 10 s);
  - optimistic updates dropped at `ts ≥ commitTs`;
  - lossless `int64`.

  It adds:
  - auth through §44's `TokenSource` (D608), with `Authenticate` called before expiry, and on `token_expired` a refresh and resume;
  - `paginate` support;
  - idempotency keys generated once per logical mutation (UUIDv7, D610);
  - the `loams-live-app` header;
  - `RetryInfo`;
  - `session_rotate` and `node_draining` resumes that are invisible to the caller.
- **`@loams/live-react`** provides `LiveProvider`, `useQuery(fn, args | "skip")`, `useMutation(fn)` (with `.withOptimisticUpdate`), `usePaginatedQuery(fn, args, { initialNumItems })` and `useLiveAuth()`. It targets React 19 with `useSyncExternalStore` and suspense-free defaults.
- **`@loams/live-cli`** (`npx loams-live`) provides `init`, `dev` (watch, bundle, deploy to the dev app), `deploy`, `deployments`, `rollback`, `logs`, `migrate` and `codegen`. Codegen writes typed function references (`_generated/api.ts`) from the bundle's export metadata and argument validators. Bundling uses esbuild (MIT, D120's choice), so the Rust binary needs no JavaScript toolchain. The `loams` binary gains only `loams live deploy --bundle <file.js> --schema <schema.json>` for CI without Node.
- **Conformance.** The session state machine's fixtures (`sdks/fixtures/live/`) are scripted Transitions, gaps, resumes, identity changes and rotations, run against the real server (embedded backend) and through the client's fake transport (§44 §10.4).
- **Other languages** get the generated `loams.live.v1` stubs (unary calls and the raw `Watch` stream) in LV1. The reactive layers for Python and Go stay in R2, and Swift and Kotlin in R3.

## 14. Conformance, correctness and chaos tests

| Suite | What it proves | Where it runs |
|---|---|---|
| `kv_conformance!` | Embedded and TiKV backends give the same transaction semantics (§8.3) | Every PR (embedded); TiKV job (PD) |
| **Reactive checker** (R1 Task 16) | Every pushed result equals a fresh snapshot evaluation at its timestamp; versions strictly increase; every committed write touching a subscribed range is reflected by the first tick at or after its commit; resumes converge. Variants: deploys and rollbacks, index backfills, identity changes and node failover in the middle of the workload | Every PR (embedded, 2 min); TiKV job; nightly (30 min) |
| **Transaction checker** (R1 Task 16) | Elle-style list-append histories are snapshot-isolated; `db.get` promotion shows no write skew; range write skew is documented (Q31) | Same |
| **Mutation fault matrix** (§20 §14) | Every `FaultPlan` cell ends `Retried`, `SurfacedUnknown` or `NoEffect`; no acknowledged mutation is lost; no idempotent mutation applies twice | TiKV job |
| **Authorization matrix** | A blessed table of (credential kind × scope × app match × function visibility × RPC) → allowed or the exact error reason, generated and compared like the fault matrices | Every PR |
| **Sandbox suite** | Determinism (R1 Task 13 tests), limits, and in `isolated` mode no file, network or exec syscalls (seccomp kills are observed) | Every PR (Linux) |
| **Nemesis** (R1 Task 16) | On a 3-PD, 3-TiKV, 2-Live playground: kill and restart TiKV stores, the PD leader and Live nodes; a PD stall (added to Task 16 by the R1 Task 2 rulings); toxiproxy partitions between Live and TiKV; process pauses. The checkers must pass throughout | Nightly; a 24 h run before each release (gate) |
| **Restore drill** | Snapshot plus export, restore into a new app, state equality at `T` (§10) | Nightly; release gate |
| **SDK conformance** | §13's fixtures | Every PR touching `sdks/typescript/packages/live*` or `proto/loams/live` |
| **Fuzzing** | Value decoding, the tuple codec, cursor decoding, the worker IPC framing, JWT and key parsing | Nightly, 10 min per target; release gate: no open crash |

## 15. Performance gates

All numbers are **targets** until the LV1d baseline (Q633). After the baseline, a nightly regression past the threshold fails the job. The reference rig is 3 PD, 3 TiKV (8 vCPU, 32 GiB, NVMe each), 2 Live nodes (8 vCPU, 16 GiB) and 1 load generator, all in one zone. The numbers below are measured on that rig unless a row says otherwise.

| # | Measure | Target | Regression threshold |
|---|---|---|---|
| P1 | Single-document insert mutation, commit latency p50 / p99 | ≤ 15 ms / ≤ 50 ms | +20% |
| P2 | Mutation throughput per app, 64 shards, uniform keys | ≥ 2 000/s | −15% |
| P3 | Commit-to-client latency (mutation commit → Transition received) p50 / p99, `tick_read_lag` 50 ms | ≤ 100 ms / ≤ 300 ms | +20% |
| P4 | One-shot `Query` on an index range of 100 docs, p50 / p99 | ≤ 10 ms / ≤ 30 ms | +20% |
| P5 | Fan-out: 10 000 sessions on one query, one mutation → all pushed, p99 | ≤ 1 s, with 1 rerun | +25% |
| P6 | Idle sessions per Live node within 2 GiB RSS | ≥ 50 000 | −15% |
| P7 | Distinct subscriptions per node (read-set index plus cache) within 4 GiB | ≥ 100 000 | −15% |
| P8 | Trivial function overhead (in_process / isolated) per call | ≤ 0.5 ms / ≤ 1.5 ms | +25% |
| P9 | Deploy of a 1 MiB bundle with no schema change → first call on all nodes | ≤ 2 s | +50% |
| P10 | Reconnect storm: 10 000 clients after one Live node is killed → all resumed | ≤ 30 s, with no TiKV error spike | +25% |
| P11 | Index backfill on a 1 M-doc table | ≥ 5 000 docs/s with foreground p99 ≤ 2× baseline | −20% |
| P12 | Backup export lag at P2 load | ≤ 15 s | +50% |
| P13 | Restore of a 10 GiB app | ≤ 30 min | +25% |
| P14 | Dedicated keyspaces on one cluster before region overhead dominates (sets `max_apps_per_cluster`, Q36) | measure; ≥ 1 000 expected | — |
| P15 | Embedded store on a laptop SSD: insert p50, and the extra RSS of Live in `loams dev` | ≤ 2 ms; ≤ 150 MiB | +25% |

The harness is `bench/live/`: a Rust load generator over the generated client, with scenario files, JSON results and a comparison script against the stored baseline. Its results go into the exit report.

## 16. Security review

The threat model (`docs/security/live-threat-model.md`, written in LV1e) covers at least:

1. **Tenant isolation.** The app header against the credential's environment; cross-app cursors (HMAC); keyspace separation; cache keys with identity (§5.4).
2. **Sandbox escape.** QuickJS memory bugs in `in_process` mode (the accepted risk is single-tenant only); seccomp and landlock coverage in `isolated`; the worker IPC as an attack surface (fuzzed).
3. **Denial of service.** Expensive queries (scan and range limits), read-set amplification (huge ranges), reconnect storms, giant arguments, slow clients (R1 backpressure), quota bypass across nodes (documented).
4. **Credentials.** Long-lived streams outliving tokens (§5.4), revocation latency, JWKS refresh abuse (rate-limited), algorithm confusion (EdDSA only for Loams tokens; RS256 and ES256 for issuers, `none` refused), clock skew.
5. **Leakage.** Errors (raw keys scrubbed), logs (no document data), traces (no arguments), metrics (no user values in labels).
6. **Supply chain.** esbuild runs in the user's CLI only; the server never bundles. Bundle integrity by SHA-256. `cargo deny`. The pinned `tikv-client` fork (D126, R1 row F1).
7. **Browser.** CORS per app, no cookies, no `Allow-Credentials`.

The review has two parts: an internal review against this list in LV1e, and an **external review before GA** (who and how is Q635). An open high or critical finding blocks the production label (§18).

## 17. Documentation

LV1 ships:

- **Developer docs:**
  - quickstart (`npx loams-live init`, `loams dev`, a React app);
  - functions and the determinism rules;
  - schema, validators and indexes, with the online backfill behaviour;
  - auth with Authentik and with any OIDC provider;
  - pagination;
  - optimistic updates;
  - migrations;
  - limits and quotas (generated).
- **Operator docs:**
  - topology and sizing;
  - TLS and auth configuration;
  - multi-tenant mode and the isolated worker;
  - backup, restore and the drill;
  - metrics, alerts and dashboards;
  - upgrade and rollback of the server;
  - the TiKV version pin;
  - GC;
  - troubleshooting (tick lag, conflicts, backfills).
- **Reference:** the generated `loams.live.v1` reference (§44 §10.5) and the SDK API reference.

§20 and the R1 plan are updated to the as-built state.

## 18. Release gating (D698)

A release labels Live **production** only if every row of the LV1 exit checklist (at the end of the [LV1 plan](../plans/2026-10-08-lv1-live-production.md)) passes on the release candidate. In summary:

1. correctness suites green, including the 24 h nemesis;
2. performance gates measured, and none regressed past its threshold;
3. restore drill passed;
4. security review complete with no open high or critical finding;
5. docs published;
6. `loams.live.v1` stable: `unstable` dropped and `buf breaking` on;
7. the TypeScript SDKs published at 1.0 with 100% of the required fixtures passing.

If any row fails, Live ships in that release **labelled "preview"** in `GetInstance` (`unstable: true`), in the docs and in the release notes. It is not withheld, so development users keep getting it.

## 19. Amendments to earlier decisions

| Earlier | Change | By |
|---|---|---|
| §20 §2.2 "No auth in R1"; §20 §7.1 loopback-only listener; D121's listener | Live is served on the main port with TLS and auth. `--live-listen` is deprecated, then removed | D686, D688 |
| §20 §18: auth and React hooks in R3; online backfill, multi-node sessions, backup and quotas in R2 | Pulled into LV1 | D680 |
| R1 Ruling 5 (index changes need an empty table) and row T9-1 (deploy refused while mutations are in flight, per-node quiesce) | Online two-phase index changes with a lifetime wait | D685 |
| R1 Task 13's `Deploy` on `LiveService` | Moved to `LiveAdminService`, versioned, with rollback | D683, D689 |
| `SubKey` = (function, argument digest), "the caller's identity joins it in R3" | (deployment, function, argument digest, identity) | D683, D687 |
| Owner ruling T8-2 (`live` off by default and implying `tikv`) | `live` (embedded) is on by default; `live-tikv` implies `tikv` | D692 |
| D670 (desktop runs `--no-live` unless the TiKV stack is up) | Embedded Live is on in the desktop by default | D692 |
| §20 §9.4 (R1: one app per process) | Many apps per node through the directory, with dedicated keyspaces | D690 |
| D131 (BR log backup is mandatory) | Per-app logical backup is mandatory and primary. BR is the cluster DR layer, conditional on Q37 and Q627 | D694 |

## 20. Open questions

| # | Question | Default if unanswered |
|---|---|---|
| Q625 | Include actions (`fetch`) and scheduled functions in LV1, or keep them in R2? | R2 |
| Q626 | Is the embedded store a supported single-node production backend (with mandatory logical backup), or dev and Desktop only? | Dev and Desktop only |
| Q627 | On v8.5.8, do BR full backup and log backup (PITR) work for API v2 txn keyspaces, and per keyspace? (Narrows Q37 into a spike) | Logical backup only; BR full backup if it works |
| Q628 | Is seccomp plus landlock enough for multi-tenant Loams Cloud, or does Live need gVisor or microVM isolation? | Subprocess sandbox for LV1, revisited by the security review |
| Q629 | crates.io refuses git dependencies: publish the `tikv-client` fork under its own crate name, or keep `live-tikv` binary-only? | Binary-only |
| Q630 | Per-app hostnames (`<app>.live.<domain>`) in addition to the header? | Header only |
| Q631 | Declarative per-table access rules for end users, or function code only? | Function code only |
| Q632 | Share subscription results across identities for functions that never read `ctx.auth`? | No; always keyed by identity |
| Q633 | Confirm or reset §15's targets after the baseline run | Baseline numbers become the gates |
| Q634 | Switch Live to `async_1pc` once the checkers pass with it? | Stay on `two_pc` |
| Q635 | Who performs the external security review before GA, and with what budget? | Owner action |
| Q636 | Retention defaults: 50 deployments, 7-day PITR with daily snapshots, 10 000 log lines per app per node | As listed |
| Q637 | Is a 24 h Watch stream rotation right for mobile clients and load balancers? | 24 h |
| Q638 | `_system:*` writes in production: keep them for principals with table `write`, or disable them? | Keep, audited |
| Q639 | Is a Live app 1:1 with an environment or namespace (§20 §4.1), or may an environment hold several apps? | 1:1 |

## 21. Sources

- Code on `dev` at `db911cda` (2026-10-08):
  - `crates/loams-live/src/{lib,config,service,deploy,system,subs,session,txn,limits,docs}.rs`;
  - `crates/loams-live/tests/`;
  - `crates/loams-tikv/src/{lib,txn,runner}.rs`;
  - `crates/loams-live-proto`;
  - `proto/loams/live/v1/live.proto`;
  - `crates/loams/src/{main,server}.rs` (`LiveArgs`, `LiveRuntime`);
  - `crates/loams/src/api/connect.rs` (`CATALOGUE`, `LiveAbsent`);
  - `crates/loams/Cargo.toml` (features);
  - `sdks/typescript/packages/live`.
- Design: §10 §3–§5, §12 (M2), §18 §6–§7, §19 §5–§7, §20 (all), §37 §19.10 (D670), §38 §4, §44 §4, §7, §10.
- Plans: R1 (Tasks 13–17, the rulings rows), MT1 (Tasks 2, 3, 7), AP1e (Tasks 20, 24), API1.
- Convex (concepts only, §20 §15): docs.convex.dev on auth (`ctx.auth`, `auth.config`), schemas and validation, internal functions, pagination, limits.
