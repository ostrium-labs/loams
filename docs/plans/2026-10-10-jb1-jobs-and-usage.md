# JB1 — Loams Jobs and the Usage Hooks in Production Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Each task lists the interfaces it must produce and the tests that must exist and pass before it is done. Where this plan gives exact names, paths, ports or defaults, use them verbatim. Where it gives a contract and named tests, write the code to that contract, and record any deviation in "Rulings made during execution" at the end of this file.
>
> **Status: Planned** (2026-10-10). Track J, design [§26](../design/26-jobs-api.md) (D204–D216, Q95–Q104) and the open half of design [§27](../design/27-usage-hooks.md) (D200–D202, D549, Q-UH-1 to Q-UH-3). JB1 implements §26 §15's milestones J1 (Celery), J2 (BullMQ), J3 (PySpark on Sail) and J4 (Flink), and §27 §3's generic observability. It was reconciled with `dev` at `1dc6e8a3`; the reconciliation is in "Rulings made while writing this plan" below. Task 0 re-checks it against the code at the time work starts.
>
> **What this plan does not build.** It builds no billing-grade metering. The per-invocation record, its socket and its reporter (§27 §3.3 and §3.6) live in the private `loams-platform` repository (D548). `InvocationObserver` is RN1's ([RN1](2026-10-01-rn1-runner-usage.md) Task 3, D549). The runtime supervisor and its tiers are F1's (§24). The Knative runner is MT2's. JB1 builds the hooks contract those consume, and the families the engine, the durable server and the jobs service own.

**Goal:** Loams Jobs GA and the usage-hooks contract v1:
- `loams-jobs`, the `Jobs` trait (§26 §5) over one job store on `loams-kv` (embedded redb in `loams dev`, TiKV everywhere else), served as `loams.jobs.v1` over Connect, gRPC and gRPC-Web;
- at-least-once delivery with fenced, exactly-once outcome records, priorities, delays, stalled sweep, rate limits, global concurrency, dedup, DLQs, retention, an outbox-fed event log and a resumable `watch`;
- schedules and flows on the embedded Resonate server, with queue mode free of promises;
- `loams-celery` (kombu transport, result backend, beat scheduler) passing Celery's integration suite;
- `@loams/bullmq` (BullMQ v6's `IQueueBackend`) passing BullMQ's backend-neutral suite;
- the `loams` Resonate helpers for Python and TypeScript;
- PySpark on Sail per namespace and Flink jobs (RisingWave for SQL, the Flink operator for DataStream), gated on their dependencies;
- the usage-hooks contract v1 (§27 §3): metric families and labels, the cgroup layout and pod labels, the tenant header and the Envoy access-log sink, the engine's, the durable server's and the jobs service's families, and the cardinality guard;
- auth, quotas, erasure, chaos, benchmarks, packaging, runbooks.

The exit is the checklist at the end of this plan, with the owning tasks.

**Architecture** (§26 §3, §27 §3):
- **`loams-jobs` is a library crate** with the `Jobs` trait, `KvJobStore` (its only store), the background loops (stalled sweep, delayed promoter, janitor, outbox relay) and the Connect service. The `loams` binary serves it behind the feature `jobs` on the main API port (`--listen`), like Graph; before MT1 a non-loopback `--listen` is refused while jobs are enabled (Q103 ruling).
- **State** sits in a `loams-kv` keyspace `loams_jobs`, with the tenant prefix `t/<ns>/`. Payloads and results over 16 KiB go to the object store with a `Freshness`. The event log is a Loams stream per queue. Schedules and flows are Resonate's (feature `durable`).
- **Adapters are thin.** `loams-celery` (Python) and `@loams/bullmq` (TypeScript) wrap the generated `loams.jobs.v1` clients of the existing SDKs. No Redis, no AMQP, no Lua.
- **Engines are separate processes** (D51). An `EngineRunner` (local process in dev, kube-rs in cloud) drives Sail, RisingWave and the Flink operator, and each lifecycle is a Loams durable workflow.
- **`loams-hooks`** is a small dependency-free crate: the contract's names, the cgroup paths, the pod labels, the tenant header layer and the active-namespace cardinality guard. Producers in this repository and consumers anywhere read the same constants.

**Tech Stack:**
- Rust 1.97 (workspace `rust-version`), edition 2024, workspace lints.
- `connectrpc` 0.9 and `buffa` 0.9 through `loams-proto` (§44, D600).
- `loams-kv` (embedded redb and TiKV, `kv_conformance!`, `FaultPlan`), `loams-tikv`, `loams-durable` (Resonate fork `ostrium-labs/resonate` at `loams/0.10.1`), `loams-store` (object store), `loams-log` and `loams-query`'s `StreamProducer` (event streams).
- `tokio`, `ulid` 3, `sha2`, `postcard`, `serde`, `proptest`, `tracing`, `axum` 0.8.
- `prometheus-client` 0.25.1 (Apache-2.0 OR MIT) and `opentelemetry`/`opentelemetry-otlp` 0.33.0, shared with LV1 Tasks 27–28 (whichever lands first adds them).
- `kube` (version picked by Task 0, shared with MT2/MT4 if present) behind the feature `jobs-k8s`, from Task 48 only.
- Python: `celery` 5.6.3, `kombu` 5.6.2 (BSD-3-Clause), the `loams` SDK's `connect-python` 0.9.0 and `protoc-gen-connectrpc` 0.12.1, Resonate Python SDK 0.8.1, `uv`, `pytest`.
- TypeScript: `bullmq` `>=6.3.9 <6.4` (peer, MIT), `@connectrpc/connect` 2.2.0, `@bufbuild/protobuf` 2.15.0, Resonate TypeScript SDK 0.11.5.
- External, pinned by digest: Sail 0.7.1, RisingWave 3.1, Flink Kubernetes Operator 1.16.1, Apache Spark 4 and the Kubeflow Spark Operator (fallback), Envoy (Task 39), `tiup playground` for TiKV.

**Spec:**
- [§26](../design/26-jobs-api.md) (all), and D204–D216, Q95–Q104 in the [decision log](../design/13-decision-log.md).
- [§27](../design/27-usage-hooks.md) §3 (the hooks), §3.5 (what the engine keeps), §4 (what is not here), §6 (Q-UH-1 to Q-UH-3); D200–D202, D549.
- [§21](../design/21-durable-execution.md) §3.5 (in-process functions), §5.1 (tenancy), §6, §8 (promise retention, Q40).
- [§44](../design/44-unified-api-and-sdks.md) §4–§10 (API rules, facades, SDK generation).
- [§41](../design/41-multitenant-byoc-control-plane.md) §9–§11 (limits, the open-core boundary), [open-core.md](../open-core.md).
- [LV1](2026-10-08-lv1-live-production.md) Tasks 27–28 (the metrics registry, admin listener, OTLP), [RN1](2026-10-01-rn1-runner-usage.md) Task 3 (`InvocationObserver`), [MT1](2026-10-02-mt1-authentik-identity.md) (auth), [MT2](2026-10-02-mt2-knative.md) (pod labels), [MT4](2026-10-02-mt4-byoc-control-plane.md) Task 8 (the no-metering guard).

## Owner rulings 2026-10-10 (defaults)

The owner asked for the best default on every question that does not need money, a legal choice or an external account. These rulings settle them; the tasks below are written to them, and they amend W3, W4 and W13. The decision log is not edited here: the integrator copies these rows when it numbers the JB1 questions. No JB1 question needs the owner any more (see "Open questions").

| # | Question | Ruling | Reason |
|---|---|---|---|
| JB1-Q1 | Per-function metric names | **One rule: a family is named after what its series are keyed by.** Series keyed by a function (labels `org`, `namespace`, `function`, `tier`, plus `runner`) are `loams_function_*`, as §27 §3.1 has them. Series about a runner's own machinery, with no `function` label (instances, cold starts, pool size, observer panics), are `loams_runner_*`. Amends W13. **Cross-track note for RN1 Task 3:** `MetricsObserver` exports `loams_function_invocations_total`, `loams_function_wall_seconds_total` and `loams_function_cpu_seconds_total`, adds the `tier` label, and keeps `runner` as a label | Users and dashboards think in functions; the runner is an implementation detail, so it is a label on function series and a prefix only for its own internals |
| JB1-Q2 | Lease concurrency, and §26 §6.9's 2,000 leases/s | **Add `get_for_update` and pessimistic mode to `loams-kv`** (shared with LV1; closes T21-15's "the first pessimistic caller adds it"), and lease pessimistically per shard (Task 5). **2,000 leases/s at S = 1 is a published benchmark, not a gate** (Task 18). Amends W4 | Optimistic windows waste work under contention exactly on hot queues; one seam method serves both tracks. A bench number depends on hardware and must not block a merge |
| JB1-Q3 | Per-shard outbox sequence | **Accept**; shards scale it, Task 18 reports it | Simplest correct ordering; a resolved-ts design can come later behind the same record |
| JB1-Q4 | Queue names | **Case-sensitive `[A-Za-z0-9._:-]{1,128}`** (W6) | BullMQ and Celery imports must not refuse existing names |
| JB1-Q5 | §26's proposed details (D205, D207, D208, D210, D213, D214) | **Accepted as this plan amends them** (W1–W13 and these rulings); Task 47 writes the amendments back into §26 | The direction is approved and the plan already reconciles the details with the code |
| JB1-Q6 | `org` label before MT4 | **The constant `default`** (W12) | Stable series now; one relabel when orgs exist |
| JB1-Q7 | Large payload transport | **Server-streamed `GetBlob`** (W9); presigning later | `loams-store` has no presigned URLs; one transport is enough for v1 |
| Q103 | Jobs listener | **The main API port (`--listen`), like Graph.** No `--jobs-listen`; before MT1 the server refuses a non-loopback `--listen` with jobs enabled unless `--no-jobs`, as Graph does (D750). Amends W3 | One port to expose, secure and route; the same loopback rule as Graph until auth exists |
| Q-UH-1 | Per-namespace cardinality | **Both, capped:** per-namespace series always pass the active-namespace cap (10,000, 15 min idle) on `/metrics`; with `--metrics-per-namespace otlp` they go out as OTLP delta instead | Bounded Prometheus series by default; delta export for large fleets |
| Q-UH-2 | Contract versioning and conformance | **`docs/api/usage-hooks-v1.md` + `loams-hooks` + the kit** (Tasks 33, 40) | Same rules as `reasons.md`; one place to read, one test to run |
| Q-UH-3 | Final cgroup reading for removed T2 pods | **Not in JB1**; F2's plan owns it, JB1 ships `pod_labels` only | It belongs to the supervisor, which JB1 does not build |
| Q98 | `LeaseStream` | **Not in JB1**; revisit after Task 18's numbers | Long-poll `Lease` is enough until measured otherwise |
| Q99 | FIFO groups | **Out of scope** for JB1 | Not needed for Celery or BullMQ parity |
| Q101 | Celery `solar` and custom schedules | **Refused with a message** | Cron and `every` cover the common cases; refusing beats silent drift |
| Q102 | Loams-hosted Celery and BullMQ workers | **User processes in JB1** | Hosting workers is the functions runtime's job (F1, MT2), not the jobs service's |
| Q95 | Arroyo | **Documented only** | RisingWave and Flink cover streaming SQL and DataStream |
| Q96 | Engine tenancy | **Sail per namespace; RisingWave shared with a database per namespace, a dedicated cluster above the size threshold Task 53 measures and records** | Isolation where it is cheap, sharing where a cluster per namespace is not |
| Q104 | Spark fallback | **Loams-managed** (Spark Connect per namespace, Kubeflow Spark Operator) | A documented-only fallback is not a fallback when Sail lacks a feature |

## Global Constraints

- **Worktree and branches.** Work in `~/Documents/Ostriumlabs/loams-wt/jb1-jobs-and-usage`. Use one branch per milestone, `feat/jb1a-core`, `feat/jb1b-events-durable`, `feat/jb1c-celery`, `feat/jb1d-bullmq`, `feat/jb1e-usage-hooks`, `feat/jb1f-production` and `feat/jb1g-engines`, each based on `dev`, with stacked PRs targeting `dev`. Use `git commit -s` (DCO). Commit areas: `jobs`, `celery`, `bullmq`, `hooks`, `durable`, `engines`, `sdk`, `deploy`, `ci`, `docs`.
- **Rust builds** use the shared target directory (`~/Documents/.cargo/config.toml`). Never set `CARGO_TARGET_DIR` and never build in `/tmp`. Run one cargo build at a time (jobs and linker from `~/.cargo/config.toml`). Build the touched crates (`cargo test -p loams-jobs`), not the workspace. The `durable` feature rebuilds about 480 crates (D262): batch the tasks that need it (15, 16, 30, 36) so the feature is toggled once per session. TiKV, kind and compose stacks run in CI jobs, or locally only when no cargo build is running.
- **The default build does not change.** `loams-jobs` is reachable only through the `loams` feature `jobs`. `jobs` does not imply `durable`; without `durable`, `Schedule` and `Flow` answer `failed_precondition` with reason `durable_unavailable`. `cargo tree -p loams -e normal` on default features must not list `loams-jobs` or `kube` (Task 12 test). The `full` release variant adds `jobs` (D286).
- **Jobs require TiKV outside `loams dev`** (Q97, owner). `standalone` and `cluster` refuse the embedded job store and name `--jobs-store tikv://<pd>[,<pd>]/loams_jobs`.
- **AP0 API rules** (§44): every non-`NO_SIDE_EFFECTS` RPC has `string idempotency_key = 15`; errors are `loams.errors.v1` with a reason registered in `docs/api/reasons.md` before it is returned; pagination is `page_size`/`page_token`; watch streams send a snapshot, then changes, then a heartbeat every 15 s.
- **The namespace comes from the credential.** `Ctx` is built by the transport layer. No request message has a namespace field that the server trusts (Task 1 test).
- **Delivery wording.** No code comment, doc, error message or API description says "exactly once" about execution. Outcome records are exactly once; execution is at least once (§26 §6.1).
- **Payloads are opaque.** No payload byte, task argument or result appears in a log line, span attribute, metric label or error message. Ids, queue names and task names may.
- **No billing** (D548, D552). No field, metric, table or endpoint named `plan`, `price`, `invoice`, `credit`, `billable` or `meter` in JB1 code, and none of the guard's marker strings anywhere. `scripts/ci/no-metering.sh` runs on every PR and already scans every tracked file; JB1 adds nothing to its allow-list. The hooks are observability; nothing here claims to be the source of truth for a charge (§27 §4).
- **Pins.** Exact versions for every new crate, package and image; images by digest. A new dependency must be at least 14 days old. Record each pin in the task's commit message.
- **Upstream behaviour is checked, not remembered.** Every Celery, kombu, BullMQ, Resonate, Sail or Flink-operator behaviour a task relies on is checked against the pinned version and cited (path and line) in a test comment or fixture README, as §26 does.
- **No Redis, no AMQP, no Lua**, and no BullMQ Pro feature (§26 §2.2).

## Rulings made while writing this plan (reconciled with `dev` at `1dc6e8a3`)

| # | Ruling | Evidence | Cost if wrong |
|---|---|---|---|
| W1 | **One store implementation, `KvJobStore`, over `loams-kv::Store`**, instead of §26 §6.8's `TikvJobStore` and `LocalJobStore`. `loams-kv` already gives one transaction surface over embedded redb (MVCC) and TiKV, with retries, commit tokens, `FaultPlan` and `kv_conformance!`, and `loams-live` and `loams-pg-control` use it. `JobStore` stays a trait so the service can be tested against a fake. D214's TiKV requirement is unchanged | `crates/loams-kv/src/lib.rs`; `crates/loams-pg-control/Cargo.toml` | Low: a second backend could still be added behind the trait |
| W2 | **`loams.jobs.v1` is generated in `loams-proto`**, not in a new `loams-jobs-proto` (D206). `loams-proto/build.rs` says new public packages go there and a package's types are generated once; GR1 and PG2 did the same. Live is the one exception, and it predates §44 | `crates/loams-proto/build.rs` header | None |
| W3 | **Amended by the Q103 ruling: jobs are served on the main API port (`--listen`, default `127.0.0.1:8080`), as Graph is under D750,** not on D206's `--jobs-listen 127.0.0.1:7720`. Before MT1, a non-loopback `--listen` with jobs enabled is refused unless `--no-jobs`. The adapters' default URL is `http://127.0.0.1:8080` | `crates/loams/src/main.rs` (`--live-listen`), `server.rs` (Graph's loopback check) | Low |
| W4 | **Pessimistic transactions are refused by `loams-kv` today** (`PESSIMISTIC_REFUSED`, row T21-15: no `get_for_update` yet). §26 §6.8 wants `lease` pessimistic. **Amended by the JB1-Q2 ruling:** Task 5 adds `get_for_update` and pessimistic mode to `loams-kv` (shared with LV1) unless Task 0 finds it already landed, and `lease` is pessimistic per shard | `crates/loams-kv/src/runner.rs` (`Mode::Pessimistic` doc) | Throughput on hot queues; measured by Task 18 |
| W5 | **Stream names cannot carry `/` or `:`**, and `_`-prefixed names are refused for users (`loams-meta` `validate_name`, `refuse_reserved`). The event stream of queue `q` is `_jobs.<q>` when `q` is stream-safe, else `_jobs.h<first 32 hex of sha256(q)>`. The name is stored in `QueueRec`, never recomputed. Task 13 adds an internal create path for reserved names | `crates/loams-meta/src/state/mod.rs:228-255` | None |
| W6 | **Queue names are case-sensitive `[A-Za-z0-9._:-]{1,128}`**, not §26 §5.1's lower-case set, because BullMQ queue names are commonly camelCase and a drop-in import must not refuse them | BullMQ docs and examples | A rename at the API level if the owner prefers lower case (JB1-Q4) |
| W7 | **The idempotency ledger is durable, in the job store** (`I/` keys, TTL 24 h), not `crates/loams/src/api/connect_idempotency.rs`'s per-process ledger, which says itself that a retry on another node is a fresh write. `Lease` is a mutation and has `idempotency_key` too: a retried `Lease` with the same key, while its leases are held, returns the same leases, so a lost answer does not strand jobs until their deadline | `connect_idempotency.rs` header | None |
| W8 | **No metrics registry exists yet.** LV1 Task 27 plans `prometheus-client` 0.25.1 and an admin listener, and is not built. Whichever of LV1 Task 27 and JB1 Task 34 runs first builds the registry and the admin listener to LV1 Task 27's text, in `crates/loams/src/metrics.rs`; the other reuses it | LV1 ruling T0-4; no `prometheus` in any `Cargo.toml` | None |
| W9 | **`loams-store` has no presigned URLs.** Large payloads and results are fetched through a server-streamed `GetBlob` RPC (64 KiB chunks), not a presigned URL. Presigning is a later optimization (JB1-Q7) | grep of `crates/loams-store/src` | One more hop for large payloads |
| W10 | **D72's idempotent producers are not built** (no producer ids in `loams-log`). The relay writes at least once, every record carries its outbox sequence, and `watch` and the adapters drop duplicates by it, as §26 §6.6 anticipates | grep of `crates/loams-log` | None; Task 13 switches to the idempotent producer when D72 lands |
| W11 | **Of §27 §3's producers, only the engine and the durable server exist.** There is no `loams-gateway`, no `loams-dapr`, no runtime supervisor, no `loams-runner` and no Envoy or Helm configuration in the repository. JB1 builds the contract (`loams-hooks`, the contract page and its conformance test), the engine's, the durable server's and the jobs service's families, the tenant-header layer and an Envoy access-log fragment. The gateway, Dapr, supervisor and Knative families are emitted by their own plans, against `loams-hooks` | `ls crates`, `ls deploy` | None |
| W12 | **The engine has no organisation concept yet.** The `org` label is filled from `Ctx.principal`'s organisation once MT1/MT4 provide it; until then it is the constant `default` (JB1-Q6) | grep for `org` in `crates/loams/src`, `loams-common` | A relabel when MT4 lands |
| W13 | **Per-function metric names disagree.** §27 §3.1 names `loams_function_*` with a `tier` label; D549 and RN1 Task 3 name `loams_runner_*` with a `runner` label. **Amended by the JB1-Q1 ruling:** function-keyed series are `loams_function_*` (labels `org`, `namespace`, `function`, `tier`, `runner`); `loams_runner_*` is only for a runner's own internals with no `function` label. RN1 Task 3 renames its three `MetricsObserver` families; Task 47 records the D549 amendment for the integrator | §27 §3.1; D549; RN1 line 124 | A rename in RN1 (not yet built) |

## Review Focus

1. **An acknowledged `enqueue` is lost.** Expected: never, through crashes, unknown commit outcomes or node kills. Tests: Task 11 `model_no_acked_enqueue_lost`; Task 17 `undetermined_enqueue_resolves_by_token`; Task 44 `nemesis_no_lost_job`.
2. **A job gets two outcomes, or a fenced write is applied.** Expected: never. Tests: Task 5 `stale_epoch_complete_is_fenced`, `late_complete_before_reclaim_succeeds`; Task 11 `model_one_outcome_per_job`; Task 44 `nemesis_no_double_completion`.
3. **Two workers hold live leases on one job.** Expected: never. Tests: Task 5 `concurrent_leases_are_disjoint`; Task 11 `model_at_most_one_live_lease`.
4. **A namespace is taken from a request body, or one tenant reaches another's queue, job, result or event.** Expected: never. Tests: Task 1 `no_request_has_a_namespace_field`; Task 12 `namespace_comes_from_ctx`; Task 41 `other_namespace_token_sees_not_found`, `lease_token_useless_across_namespaces`.
5. **A schedule tick enqueues twice or is skipped.** Expected: one job per tick. Tests: Task 15 `tick_processed_twice_enqueues_once`, `restart_between_ticks_misses_none`; Task 24 `beat_twice_no_duplicate_ticks`.
6. **A chord body is sent twice, or a count is lost under `acks_late`.** Expected: neither. Tests: Task 9 `incr_counts_member_once`; Task 22 `chord_body_once_under_redelivery`, `acks_late_crash_before_incr_recounts`.
7. **`watch` misses or repeats an event across reconnects.** Expected: neither within the stream's retention. Tests: Task 14 `nothing_between_snapshot_and_subscribe`, `resume_from_cursor_no_gap_no_dup`, `vector_cursor_resumes_every_partition`.
8. **A long ETA causes redelivery.** Expected: never (delayed index, not a lease). Test: Task 21 `long_eta_not_redelivered`.
9. **The default `loams` build gains jobs or Kubernetes code, or a billing name appears.** Expected: no. Tests: Task 12 `default_features_exclude_jobs`; Task 48 `default_features_exclude_kube`; `scripts/ci/no-metering.sh`.
10. **A payload leaks into a log, span, label or error; per-namespace series grow without bound.** Expected: never; series capped. Tests: Task 37 `canary_payload_never_observed`; Task 34 `active_namespace_cap_bounds_series`.
11. **A client-supplied tenant header reaches an access log.** Expected: stripped before the gateway sets it. Test: Task 39 `client_tenant_header_stripped`.
12. **A Spark Connect session or engine run reaches another namespace's engine.** Expected: refused. Tests: Task 50 `session_routed_only_to_own_namespace`; Task 46 `engine_network_policy_denies_cross_namespace`.

---

## File structure

```
proto/loams/jobs/v1/{jobs.proto,types.proto,events.proto}      Task 1
proto/loams/jobs/v1/engines.proto                               Task 48
crates/loams-proto/build.rs, tests/jobs.rs                      Tasks 1, 48
crates/loams-jobs/                                              Tasks 2–18, 37, 41–43
  src/lib.rs  ctx.rs  ids.rs  names.rs  error.rs  types.rs  model.rs  keys.rs  token.rs
  src/store/{mod.rs,kv.rs,enqueue.rs,lease.rs,complete.rs,admin.rs,query.rs,results.rs,conformance.rs}
  src/loops/{mod.rs,owner.rs,sweep.rs,promote.rs,janitor.rs,relay.rs}
  src/{blob.rs,watch.rs,wake.rs,schedule.rs,flow.rs,quota.rs,erase.rs,metrics.rs,authz.rs}
  src/service/{mod.rs,convert.rs,stream.rs}
  tests/{store_embedded.rs,store_tikv.rs,model.rs,service.rs,watch.rs,durable.rs}
crates/loams-jobs-engines/                                      Tasks 48–54 (feature jobs-k8s for kube)
  src/{lib.rs,runner.rs,local.rs,k8s.rs,sail.rs,spark_proxy.rs,spark_batch.rs,spark_fallback.rs,risingwave.rs,flink.rs}
crates/loams-hooks/                                             Tasks 33, 34, 38, 39
  src/{lib.rs,families.rs,active.rs,cgroup.rs,labels.rs,tenant.rs}
  tests/contract.rs
crates/loams/src/{main.rs,server.rs,metrics.rs}                 Tasks 12, 34 (flags, wiring, registry)
crates/loams/src/api/                                           Task 35 (namespace families on the ingest and query paths)
crates/loams-durable/src/{metrics.rs,embed.rs}                  Task 36
docs/api/{reasons.md,route-map.md,usage-hooks-v1.md}            Tasks 1, 33
sdks/python/src/loams/jobs/, sdks/python/src/loams/resonate.py  Tasks 19, 30
sdks/typescript/packages/client/src/jobs/                       Task 19
sdks/typescript/packages/durable/                               Task 30 (@loams/durable)
integrations/celery/                                            Tasks 20–25 (PyPI loams-celery)
  pyproject.toml  src/loams_celery/{__init__.py,transport.py,backend.py,beat.py,cli.py,priorities.py,bindings.py}
  tests/  ci/celery-integration.sh
integrations/bullmq/                                            Tasks 26–32 (npm @loams/bullmq; Python backend in python/)
  package.json  src/{index.ts,backend.ts,tokens.ts,events.ts,getters.ts,schedulers.ts,flows.ts}
  test/  exclusions.tsv  python/
deploy/envoy/access-log/{envoy.yaml,README.md}                  Task 39
deploy/jobs/{alerts.yaml,dashboards/}                           Task 37
deploy/engines/{sail/,risingwave/,flink-operator/,spark-fallback/}   Tasks 49–54
scripts/jb1/{bench.sh,nemesis/,engines-kind.sh}                 Tasks 18, 44, 54
bench/results/jobs/                                             Task 18
docs/guides/jobs/{semantics.md,celery.md,bullmq.md,durable-mode.md,spark.md,flink-porting.md}   Tasks 47, 55
docs/runbooks/jobs/                                             Task 47
docs/security/loams-jobs-threat-model.md                        Task 46
.github/workflows/{jb1.yml,jb1-adapters.yml,jb1-engines.yml}
```

## Shared contracts (all tasks use these names)

### Proto (Task 1 writes it; this is the contract, not the full file)

```proto
syntax = "proto3";
package loams.jobs.v1;

service JobsService {
  rpc Enqueue(EnqueueRequest) returns (EnqueueResponse);
  rpc EnqueueBulk(EnqueueBulkRequest) returns (EnqueueBulkResponse);
  rpc Lease(LeaseRequest) returns (LeaseResponse);               // long-poll, wait <= 30 s
  rpc Extend(ExtendRequest) returns (ExtendResponse);
  rpc Complete(CompleteRequest) returns (CompleteResponse);
  rpc Report(ReportRequest) returns (ReportResponse);            // progress | log | data
  rpc Schedule(ScheduleRequest) returns (ScheduleResponse);      // upsert by id
  rpc Unschedule(UnscheduleRequest) returns (UnscheduleResponse);
  rpc ListSchedules(ListSchedulesRequest) returns (ListSchedulesResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc Flow(FlowRequest) returns (FlowResponse);
  rpc QueueAdmin(QueueAdminRequest) returns (QueueAdminResponse);  // oneof op (§26 §5.4)
  rpc JobAdmin(JobAdminRequest) returns (JobAdminResponse);        // oneof op (§26 §5.4)
  rpc GetJob(GetJobRequest) returns (GetJobResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc Query(QueryRequest) returns (QueryResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc GetBlob(GetBlobRequest) returns (stream GetBlobResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc ResultGet(ResultGetRequest) returns (ResultGetResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc ResultMGet(ResultMGetRequest) returns (ResultMGetResponse) { option idempotency_level = NO_SIDE_EFFECTS; }
  rpc ResultSet(ResultSetRequest) returns (ResultSetResponse);
  rpc ResultDelete(ResultDeleteRequest) returns (ResultDeleteResponse);
  rpc ResultIncr(ResultIncrRequest) returns (ResultIncrResponse);  // member set; returns {size, reached}
  rpc ResultExpire(ResultExpireRequest) returns (ResultExpireResponse);
  rpc Watch(WatchRequest) returns (stream WatchResponse);           // snapshot, events, heartbeat 15 s
  // Task 48 adds SubmitEngineJob, ControlEngineJob, GetEngineRun, ListEngineRuns (engines.proto).
}
// Every mutating request has `string idempotency_key = 15;`. No request has a namespace field.
// Ids: JobId is a 26-char Crockford ULID; ScheduleId and FlowId are client strings
// ([A-Za-z0-9._:-]{1,256}); RunId is "run-" + ULID.
// LeaseToken is `bytes` on the wire (token.rs format, below); clients treat it as opaque.
// JobState: JOB_STATE_{WAITING,PRIORITIZED,DELAYED,ACTIVE,WAITING_CHILDREN,COMPLETED,FAILED,DEAD_LETTERED,CANCELED}.
// Outcome: oneof {ok{result?}, retry{error, after?}, fail{error, dead_letter}, release{after?},
//   delay{until}, wait_children{}, rate_limited{for}}.
// Payload: {content_type, oneof {inline bytes (<= 16 KiB), blob_ref string}}.
// Watch cursor: opaque string (S = 1: the partition offset; S > 1: every partition's offset).
```

### Rust traits and types (`loams-jobs`)

```rust
pub struct Ctx { pub namespace: NamespaceId, pub principal: Principal,
                 pub deadline: Option<Instant>, pub request_id: RequestId }

/// §26 §5.1, unchanged in shape. Engines' two methods live on `EngineJobs`
/// (Task 48) so `loams-jobs` does not depend on `loams-jobs-engines`.
#[async_trait]
pub trait Jobs: Send + Sync + 'static { /* enqueue … result_expire, exactly §26 §5.1 minus the engine pair */ }

/// The transactions of §26 §5.6. One implementation, `KvJobStore` (W1).
#[async_trait]
pub trait JobStore: Send + Sync + 'static {
    async fn enqueue(&self, cx: &Ctx, q: &QueueId, items: Vec<(TaskSpec, EnqueueOpts)>, key: Option<&str>)
        -> Result<Vec<Result<Enqueued, JobsError>>, JobsError>;
    async fn lease(&self, cx: &Ctx, req: &LeaseRequest, key: Option<&str>) -> Result<Vec<Lease>, JobsError>;
    async fn extend(&self, cx: &Ctx, t: &LeaseToken, by: Duration) -> Result<Deadline, JobsError>;
    async fn complete(&self, cx: &Ctx, t: &LeaseToken, o: Outcome) -> Result<Completed, JobsError>;
    async fn report(&self, cx: &Ctx, t: &LeaseToken, r: Report) -> Result<(), JobsError>;
    async fn queue_admin(&self, cx: &Ctx, q: &QueueId, op: QueueOp) -> Result<QueueOpResult, JobsError>;
    async fn job_admin(&self, cx: &Ctx, q: &QueueId, j: JobRef, op: JobOp) -> Result<JobOpResult, JobsError>;
    async fn query(&self, cx: &Ctx, q: JobQuery) -> Result<Page<JobView>, JobsError>;
    async fn results(&self, cx: &Ctx, op: ResultOp) -> Result<ResultAnswer, JobsError>;
    async fn snapshot(&self, cx: &Ctx, sel: &Selector) -> Result<(Vec<JobView>, OutboxHeads, Ts), JobsError>;
    async fn acquire_owner(&self, scope: &str, holder: &str, ttl: Duration) -> Result<OwnerFence, JobsError>;
    // the loops (Task 7, 13) take an OwnerFence and work one shard at a time
}

pub struct LeaseToken { pub queue: QueueId, pub job: JobId, pub epoch: u64 }
// token.rs: wire = 0x01 ‖ u8 len ‖ queue ‖ 16-byte ULID ‖ u64 BE epoch. Decode errors → invalid_argument.

pub enum JobsError { InvalidArgument(String), NotFound(&'static str), AlreadyExists { existing: JobId },
    IdempotencyConflict, Fenced { current_epoch: u64 }, InvalidState { from: JobState, to: JobState },
    QueuePaused, RateLimited { retry_after: Duration }, PayloadTooLarge { limit: u64 },
    DurableUnavailable, Unavailable(String), DeadlineExceeded, Unauthenticated, PermissionDenied,
    Internal(String) }
// error.rs maps each variant to one Connect code and one reason (§26 §5.3 table; Task 1 registers them).
```

### Keys (`keys.rs`, under the `loams-kv` root of keyspace `loams_jobs`; tuple-encoded)

```
t/<ns>/Q/<queue>                                       QueueRec (config, shards S, stream name, DLQ, retention)
t/<ns>/J/<queue>/<job>                                 JobRec (state, epoch, worker, deadline, attempts, stalled, payload, opts)
t/<ns>/R/<queue>/<shard:u8>/<band:u8><prio:u32 BE><seq:u64 BE>   → job      ready (band 0 FIFO, 1 prioritized; LIFO seq = u64::MAX − seq)
t/<ns>/D/<queue>/<shard>/<due_ms:u64 BE><job>          → ()       delayed
t/<ns>/A/<queue>/<shard>/<deadline_ms:u64 BE><job>     → epoch    active, by deadline
t/<ns>/P/<queue>/<parent>/<child>                      → state    waiting-children dependencies
t/<ns>/T/<queue>/<state:u8>/<inv_ts:u64 BE><job>       → ()       finished-state index for Query, newest first
t/<ns>/K/<queue>/<job_key>                             → job      job_key index
t/<ns>/I/<sha256(rpc ‖ principal ‖ key)[..16]>         IdempotencyRec {request_hash, answer, expires_ts}
t/<ns>/U/<queue>/<dedup_id>                            DedupRec
t/<ns>/B/<queue>[/<shard>]                             bucket (rate limit)
t/<ns>/C/<queue>                                       active counter (global concurrency)
t/<ns>/S/<queue>/<shard>                               outbox head (u64 sequence)
t/<ns>/O/<queue>/<shard>/<seq:u64 BE>                  OutboxRow {event | settle_promise}
t/<ns>/X/<result_key>                                  ResultRec {value | members, expires_ts}
t/<ns>/E/<schedule_id>                                 ScheduleRec
t/<ns>/G/<exchange>/<routing_key>/<queue>              kombu binding
t/<ns>/L/<scope>                                       owner lease (sweep, relay, janitor) {holder, epoch, deadline_ms}
t/<ns>/N/<run_id>                                      EngineRunRec (Task 48)
```
Values are postcard with a leading format byte, as `loams-pg-control/src/model.rs`. Inline payloads ≤ 16 KiB; larger ones at `ns/<ns>/jobs/<queue>/<job>/{payload,result}` with a `Freshness`.

### Event record (outbox → stream `QueueRec.stream`, partition = shard)

JSON, one record per event: `{"v":1,"seq":<outbox seq>,"shard":n,"ts_ms":…,"event":"<name>","queue":…,"job":…,"name":…,"attempt":…,"data":{…}}`, header `loams-jobs-seq: <seq>`. Event names are BullMQ's 18 (`added`, `waiting`, `delayed`, `active`, `progress`, `completed`, `failed`, `stalled`, `retries-exhausted`, `removed`, `duplicated`, `deduplicated`, `debounced`, `drained`, `paused`, `resumed`, `cleaned`, `waiting-children`; Task 26 checks the list at the pin) plus `dead-lettered` and `canceled`. `data` never contains a payload or a result, only sizes and error codes.

### Usage-hooks contract v1 (`loams-hooks`, Task 33; `docs/api/usage-hooks-v1.md` is its text)

```rust
pub const CONTRACT: &str = "loams.usage-hooks/v1";
pub struct Family { pub name: &'static str, pub kind: Kind, pub labels: &'static [&'static str],
                    pub per_namespace: bool, pub owner: Owner /* Engine | Durable | Jobs | Runner | Gateway | Dapr */ }
pub const FAMILIES: &[Family];          // every row of §27 §3.1 and §26 §6.11 (with W13's names)
pub mod labels { pub const ORG: &str = "loams.dev/org"; pub const NAMESPACE: &str = "loams.dev/namespace";
                 pub const FUNCTION: &str = "loams.dev/function"; pub const TIER: &str = "loams.dev/tier";
                 pub const VERSION: &str = "loams.dev/version"; pub const RUNNER: &str = "loams.dev/runner";
                 pub fn pod_labels(id: &SandboxIdentity) -> BTreeMap<String, String>; }
pub mod cgroup { pub fn t0_scope(org: &str) -> PathBuf;  // loams.slice/tenant-<org>.slice/workerd.scope
                 pub fn t1_scope() -> PathBuf;           // loams.slice/wasm-host.scope
                 pub struct FinishedScopeReaper { pub retention: Duration /* 10 min */ } }
pub mod tenant { pub const HEADER: &str = "x-loams-tenant"; pub struct TenantHeaderLayer; }
pub struct ActiveNamespaces { /* cap (default 10 000), idle TTL (default 15 min), LRU eviction */ }
```

### Engines (`loams-jobs-engines`, Task 48)

```rust
#[async_trait]
pub trait EngineRunner: Send + Sync + 'static {
    fn kind(&self) -> EngineKind;                                  // Sail | SparkSubmit | SparkConnect | RisingWave | Flink
    async fn apply(&self, run: &EngineRunRec) -> Result<Applied, EngineError>;      // idempotent by run id + spec hash
    async fn status(&self, run: &RunId) -> Result<EngineStatus, EngineError>;
    async fn action(&self, run: &RunId, a: EngineAction) -> Result<ActionOutcome, EngineError>; // savepoint, suspend, resume, upgrade, rollback, cancel
    async fn delete(&self, run: &RunId) -> Result<(), EngineError>;                 // idempotent
}
#[async_trait]
pub trait EngineJobs { async fn submit_engine_job(&self, cx: &Ctx, job: EngineJob, opts: SubmitOpts) -> Result<RunId, JobsError>;
                       async fn control_engine_job(&self, cx: &Ctx, run: &RunId, a: EngineAction) -> Result<ActionId, JobsError>; }
```

---

## Execution order

1. Task 0.
2. **JB1a** (Tasks 1–12), in order. Task 18's harness can start after Task 5.
3. **JB1b** (13–18) after Task 12. Tasks 15–16 need the `durable` feature; run them in one build session.
4. **JB1c** (19–25) after Task 13 (Celery's fanout needs streams) and Task 15 (beat).
5. **JB1d** (26–32): Task 26 (the BullMQ spike) runs beside JB1a, because its answers shape Tasks 27–29. Tasks 27–31 after Tasks 14 and 16.
6. **JB1e** (33–40) beside JB1b: Tasks 33–34 need nothing from JB1; Task 37 needs Task 12; Task 36 batches with Tasks 15–16.
7. **JB1f** (41–47): Task 41 after MT1 merges; Tasks 42–46 after JB1d; Task 47 last.
8. **JB1g** (48–55) only when its entry gates hold: MT1 merged (the Spark Connect proxy beyond loopback), M2's `format("loams")`, M4's Iceberg through Lakekeeper (J3), M5's Kafka gateway (J4 reading Loams streams). Task 48 may start after JB1b.

---

### Task 0: Reconcile with the code as built

**Files:** this plan's "Rulings made during execution" only.

Steps:
1. Answer each of the following and record the answer, with file paths, as a ruling:
   - Are rulings W1–W13 still true at the current `dev`? In particular: has `loams-kv` gained `get_for_update` (W4; if not, Task 5 adds it, and the LV1 owner is told before the PR)? Has LV1 Task 27 built the registry and admin listener, and on which address (W8)? Has D72 landed (W10)? Do `loams-gateway`, `loams-dapr`, `loams-runner` or a supervisor exist (W11)?
   - How does a crate create a stream with a reserved (`_`) name in process, and produce to it (`StreamProducer` in `loams-query/src/flight_ingest.rs`)? Name the existing caller to copy (a collection's implicit stream).
   - Does the embedded Resonate server at the fork pin serve `schedule.create` with a promise template, and does its cron parser take a time zone? Which group name does `DurableRuntime` register Loams' own functions under (`inproc://any@loams`, §21 §3.5)?
   - Which lease pattern to copy for the loops' owners: `loams-pg-control`'s `acquire_lease` over `loams-kv`, or `loams-meta-tikv/src/leases.rs`'s `check_fence`? If both fit, lift one into `loams-kv` only with the LV1 owner's agreement; otherwise copy into `loams-jobs/src/loops/owner.rs`.
   - Is MT1's `Authorizer` on `dev`? If not, Task 12 uses the loopback guard only, and Task 41 waits.
   - Which `kube` version does MT2/MT4 use, if any? Else the newest release at least 14 days old (needed from Task 48).
   - Python SDK: confirm `connect-python` 0.9.0 and `protoc-gen-connectrpc` 0.12.1 (§26 §12.1 says `connectrpc` 0.12.1 on PyPI). TypeScript: the package manager and workspace layout of `sdks/typescript/packages`.
   - Celery 5.6.x, kombu 5.6.x and BullMQ 6.3.x: still the latest patch on their minors? Record the pins.
   - The questions are settled in "Owner rulings 2026-10-10 (defaults)". Record only a code fact that contradicts a ruling, as a new ruling, and stop for the owner if it does.
2. Commit `docs(jb1): task 0 rulings`.

## JB1a — The jobs core and service (Tasks 1–12)

### Task 1: `loams.jobs.v1` protos, reasons and route map

**Files:** create `proto/loams/jobs/v1/{jobs.proto,types.proto,events.proto}`. Modify `crates/loams-proto/build.rs` (`FILES`), `docs/api/reasons.md`, `docs/api/route-map.md`, `buf.yaml` (`breaking.ignore` while unstable). Tests in `crates/loams-proto/tests/jobs.rs`.

**Interfaces:** the service and messages of the shared contract, complete, with `loams.options.v1` facade annotations (module `jobs`) and `unstable: true`. Reasons added: `fenced`, `idempotency_conflict`, `job_key_conflict`, `invalid_job_state`, `queue_paused`, `payload_too_large`, `jobs_rate_limited`, `durable_unavailable`, `schedule_rule_invalid`, `flow_too_large`.

Tests:
- `buf lint` (STANDARD) passes in `jb1.yml`; `buf breaking` runs with the package ignored until Task 47 removes `unstable`.
- `every_mutation_has_idempotency_key` (jobs): every non-`NO_SIDE_EFFECTS` RPC's request has field 15 `idempotency_key` of type string, `Lease` included (W7).
- `no_request_has_a_namespace_field`: no request message, at any depth, has a field named `namespace`, `ns` or `tenant`.
- `lease_token_is_bytes`: every `lease_token` field is `bytes`.
- `jobs_reasons_registered`: every reason listed above appears in `docs/api/reasons.md` with its Connect code.
- `event_names_match_contract`: the `EventName` enum's values are the 20 names of the shared contract.

Steps: tests (FAIL: package missing) → protos → generate → PASS → route-map rows → commit `feat(jobs): loams.jobs.v1 protos`.

### Task 2: `loams-jobs` types, names, tokens and errors

**Files:** create `crates/loams-jobs/` with `src/{lib.rs,ctx.rs,ids.rs,names.rs,error.rs,types.rs,token.rs}`; add the crate to the workspace. Tests inline and `tests/names.rs`.

**Interfaces:** `Ctx`, `QueueId`, `JobId`, `WorkerId`, `LeaseToken`, `TaskSpec`, `EnqueueOpts`, `Enqueued`, `LeaseRequest`, `Lease`, `Outcome`, `JobState`, `Priority` (`None` | 1…2,097,151), `Backoff`, `Dedup`, `Retention`, `Mode`, `JobsError` (§26 §5.1–§5.3 and the shared contract). `names::validate_queue` (W6), `names::stream_name(&QueueId) -> String` (W5). `token::{encode, decode}`. `error::to_connect(&JobsError) -> ConnectError` with `ErrorInfo.reason`.

Tests:
- `queue_name_rules`: accepts `celery`, `myQueue`, `a.b:c-d`; refuses empty, 129 bytes, `/`, space, non-ASCII.
- `stream_name_is_stream_safe`: for 10,000 random valid queue names (proptest), `stream_name` passes `loams-meta`'s name rule and starts with `_jobs.`; two distinct queues never share a stream name.
- `lease_token_roundtrip` and `lease_token_rejects_tampered_length`.
- `priority_bounds`: 0 is `None`, 2,097,151 accepted, 2,097,152 refused.
- `each_error_maps_to_one_code_and_reason`: the §26 §5.3 table, row by row.
- `retryable_errors_are_marked`: `Unavailable`, `RateLimited`, `QueuePaused` are retryable; the rest are not.

Commit `feat(jobs): types, names, lease tokens and errors`.

### Task 3: Keys and records

**Files:** `src/{keys.rs,model.rs}`. Tests `tests/keys.rs`.

**Interfaces:** the key layout of the shared contract, built with `loams_kv::tuple`; `QueueRec`, `JobRec`, `IdempotencyRec`, `DedupRec`, `OutboxRow`, `ResultRec`, `ScheduleRec`, `OwnerRec`, each with `FORMAT = 1` and postcard encoding. `QueueConfig` defaults: `S = 1`, visibility 30 s, attempts 1, `max_stalled` 1, log cap 1,000 lines, result TTL 1 day, events retention 7 days.

Tests:
- `ready_order_fifo_band_first`: band 0 keys sort before every band 1 key.
- `ready_order_priority_then_seq`, `lifo_sorts_newest_first`.
- `delayed_and_active_sort_by_time`.
- `finished_index_newest_first`.
- `records_golden`: one value of each record encodes to `tests/fixtures/<record>.bin`.
- `unknown_format_byte_is_corrupt`.
- `tenant_prefixes_are_disjoint`: no key of namespace `a` is a prefix of, or inside the range of, namespace `ab`.

Commit `feat(jobs): key layout and records`.

### Task 4: `KvJobStore`: enqueue, idempotency, `job_key` and dedup

**Files:** `src/store/{mod.rs,kv.rs,enqueue.rs,conformance.rs}`. Tests `tests/store_embedded.rs` and `tests/store_tikv.rs` (feature `tikv`, skipped when `LOAMS_TEST_PD` is unset; CI's TiKV job runs it).

**Interfaces:** `JobStore` and `KvJobStore::open(loams_kv::Store)`. `jobs_store_conformance!($factory)` runs every store test of Tasks 4–10 on both backends. `enqueue` is one optimistic transaction per job (bulk: one per 64 jobs), with `TxnOptions::new("jobs.enqueue").with_token()`.

Tests (inside the macro):
- `enqueue_then_get_waiting`
- `job_key_same_params_returns_existing` (`created: false`) and `job_key_other_params_is_already_exists`
- `idempotency_replay_returns_first_answer` and `idempotency_other_request_is_conflict`
- `idempotency_expires_after_ttl` (clock injected)
- `dedup_simple_while_unfinished`, `dedup_throttle_ttl`, `dedup_debounce_replaces_data`, `dedup_debounce_extend`, `dedup_keep_last_if_active`
- `enqueue_on_unknown_queue_creates_it` (kombu's declare-on-first-use, §26 §7.1.2)
- `bulk_partial_failure_reports_each`
- `enqueue_writes_outbox_row_in_same_txn`

Steps: tests first → embedded PASS → TiKV PASS → commit `feat(jobs): enqueue with idempotency, job keys and dedup`.

### Task 5: Lease, extend, complete and report with fencing

**Files:** `src/store/{lease.rs,complete.rs}`; and, unless Task 0 found it landed, `crates/loams-kv/src/{lib.rs,runner.rs,embedded.rs}` and `crates/loams-tikv` (pessimistic mode and `get_for_update`, JB1-Q2). Tests in the conformance macro and in `loams-kv`'s `kv_conformance!`.

**Interfaces:**
- **`loams-kv` first (JB1-Q2):** `Txn::get_for_update(key)` and `Mode::Pessimistic` on both backends (TiKV: `loams_tikv::Txn::get_for_update`; embedded: a per-key lock table with a 1 s wait that fails with `Conflict`). `PESSIMISTIC_REFUSED` and the case `pessimistic_mode_is_refused` are removed, which supersedes LV1's T21-15; the LV1 owner reviews the PR. New `kv_conformance!` cases: `get_for_update_blocks_concurrent_writer`, `pessimistic_read_modify_write_loses_no_update`, `pessimistic_lock_wait_times_out_as_conflict`.
- `lease`: visits the requested queues in order and their shards in a rotating order; per shard, one pessimistic transaction that takes `get_for_update` on the shard's active counter (which serializes lessees of one shard without wasted work), scans the ready head, checks the bucket and moves up to `max` jobs to active with `epoch + 1` and a deadline by the store's clock (`Store::now`). A lock-wait timeout moves on to the next shard. Respects `names`, pauses, `expires` (an expired job is discarded with a `removed` event) and `timeout`.
- `extend`: checks the epoch; never past `timeout`.
- `complete`: checks the epoch, applies the `Outcome` per §26 §6.4, decrements the active counter, writes the result or error, the finished-state index, parent bookkeeping and the outbox row, in one transaction.
- `report`: progress, a log line (capped, the oldest dropped), a data update.
- A retried `Lease` with the same `idempotency_key` returns the same leases while they are held (W7).

Tests:
- `lease_takes_fifo_then_priority`
- `concurrent_leases_are_disjoint`: 16 tasks lease from one queue of 1,000 jobs; no job appears twice.
- `stale_epoch_complete_is_fenced` (`Fenced { current_epoch }`)
- `late_complete_before_reclaim_succeeds` (expiry alone does not break the fence, §26 §6.3)
- `extend_past_timeout_refused`
- `retry_moves_to_delayed_with_backoff`, `attempts_exhausted_fails`, `fail_with_dead_letter_moves_to_dlq`
- `release_does_not_spend_an_attempt`
- `wait_children_then_children_done_moves_to_waiting`
- `rate_limited_outcome_empties_bucket`
- `global_concurrency_caps_active`
- `paused_queue_leases_nothing`
- `expired_job_is_discarded_not_run`
- `lease_retry_same_key_returns_same_leases`
- `hot_shard_leases_without_conflict_retries` (16 workers, one shard: zero aborted lease transactions)
- `long_poll_wakes_on_enqueue` (service-level, with `wake.rs`'s notifier)

Commit `feat(jobs): leases, completion and fencing`.

### Task 6: Priorities, delays, LIFO and shards

**Files:** `src/store/{enqueue.rs,lease.rs}`, `src/loops/promote.rs`. Tests in the macro and `tests/order_prop.rs`.

**Interfaces:** `QueueConfig.shards` (1…64, fixed at creation; changing it is `invalid_argument`). Shard choice: hash of the job id. The delayed promoter moves due jobs to ready, one shard per pass.

Tests:
- `priority_order_matches_bullmq_scores`: the order of 1,000 random `(priority, seq)` pairs equals BullMQ's `priority × 2^32 + counter` order.
- `delayed_not_leased_before_due`, `delayed_promoted_when_due`
- `lifo_served_first_in_band`
- `order_holds_per_shard_prop` (proptest): with `S = 4`, within each shard jobs start in index order.
- `best_head_of_two_shards`: with `S > 1`, a lease takes the better head of the first two shards it visits (§26 §6.5 estimate).

Commit `feat(jobs): priorities, delays, lifo and shards`.

### Task 7: The loops: owners, stalled sweep, janitor and DLQ redrive

**Files:** `src/loops/{mod.rs,owner.rs,sweep.rs,janitor.rs}`. Tests `tests/loops.rs` (both backends).

**Interfaces:** `LoopSet::start(store, node_id, clock) -> LoopHandle`. One owner per `(ns, queue, shard)` through an owner lease `L/jobs/sweep/<queue>/<shard>` (TTL 10 s, renewed at a third), placed by rendezvous hashing over live nodes (§18 §5) when Task 0 finds the router's helper, else first-come. The sweep: expired active jobs → waiting with `stalled + 1` and a `stalled` event; past `max_stalled` → failed with `"job stalled more than allowable limit"`; jobs from schedulers are exempt; a `timeout` breach is a retryable failure with `error.code = "timeout"`. The janitor: retention by count and age per state, payload objects with their records, idempotency and result TTLs. `RedriveDeadLetters { limit }`.

Tests:
- `stalled_job_returns_to_waiting`, `stalled_past_limit_fails_with_bullmq_message`
- `celery_queue_unlimited_stalls` (`max_stalled = None`)
- `scheduler_jobs_exempt_from_stall_fail`
- `timeout_is_retryable_failure`
- `one_owner_per_shard`: two loop sets; each shard has one owner; killing one hands its shards over within 2 × TTL.
- `fenced_owner_writes_nothing` (an old owner's sweep transaction is refused)
- `retention_by_count_and_age`, `janitor_deletes_payload_objects`
- `redrive_moves_back_with_history`

Commit `feat(jobs): stalled sweep, janitor and dead-letter redrive`.

### Task 8: Large payloads and results

**Files:** `src/blob.rs`. Tests `tests/blob.rs` (local object store).

**Interfaces:** `Blobs::put(ns, queue, job, kind, bytes) -> BlobRef` writes the object before the transaction, with a `Freshness` (§03), so GC deletes an object whose transaction never committed. Inline at ≤ 16 KiB. `GetBlob` streams 64 KiB chunks after checking that the `BlobRef` belongs to the caller's namespace (W9). The namespace payload limit defaults to 64 MiB.

Tests:
- `inline_at_16k_blob_above`
- `get_blob_streams_whole_object`
- `get_blob_other_namespace_not_found`
- `uncommitted_blob_collected_after_grace`
- `payload_over_limit_refused` (`payload_too_large`)

Commit `feat(jobs): object-store payloads and results`.

### Task 9: The result store

**Files:** `src/store/results.rs`. Tests in the macro.

**Interfaces:** `ResultOp::{Get, MGet, Set{ttl}, Delete, Incr{member, ttl}, Expire{ttl}}` over `X/` keys (§26 §7.1.4). `Incr` stores the member set and returns `{size, reached}` where `reached` is true only on the call that first moves `size` to the counter's target (the target is set by the first `Incr` that carries it, or by `Set`). Default TTL 1 day.

Tests:
- `incr_counts_member_once`: the same member twice → size 1.
- `only_one_call_reaches`: 100 concurrent `Incr` with distinct members to target 100; exactly one `reached`.
- `ttl_expires_result`, `expire_resets_ttl`
- `mget_preserves_order_and_missing`
- `results_are_namespaced`

Commit `feat(jobs): result store with idempotent counters`.

### Task 10: Admin and query

**Files:** `src/store/{admin.rs,query.rs}`. Tests in the macro.

**Interfaces:** every `QueueOp` and `JobOp` of §26 §5.4; `Query` by queue, state, name, time range and tag, newest first, `page_size` (default 100, max 1,000) and `page_token`; `Counts` per state (waiting and prioritized separately). `Obliterate { force }` refuses with active jobs unless `force`. `Workers` lists workers seen leasing in the last `2 × visibility`.

Tests: `pause_resume`, `drain_keeps_active`, `drain_delayed_flag`, `clean_by_state_grace_limit`, `obliterate_refuses_active_without_force`, `counts_match_states`, `retry_all_failed`, `promote_all_delayed`, `change_priority_reorders`, `change_delay`, `update_data`, `logs_paged`, `cancel_removes_waiting_for_good`, `remove_dedup_key`, `query_pages_are_stable`, `query_by_name_and_time_range`.

Commit `feat(jobs): admin operations and queries`.

### Task 11: The model test

**Files:** `tests/model.rs`, `src/store/conformance.rs` (the reference model). 

**Interfaces:** a reference model of §26 §6 in plain Rust; a proptest driver of random interleavings of enqueue, lease, extend, complete (every outcome), sweep, promote, crash (drop in-flight futures), and clock advance; on TiKV with `FaultPlan` (region errors, unknown commit outcomes, TSO restarts).

Tests:
- `model_no_acked_enqueue_lost`
- `model_one_outcome_per_job`
- `model_at_most_one_live_lease`
- `model_fenced_write_always_refused`
- `model_priorities_and_delays_respected_per_shard`
- `model_counters_never_negative_or_leaked` (active counter equals active jobs at quiescence)

Embedded: 1,000 cases per PR; TiKV with faults: 200 cases per PR, 10,000 nightly. Commit `test(jobs): model test on both stores`.

### Task 12: `JobsService`, the listener and the `jobs` feature

**Files:** `src/service/{mod.rs,convert.rs,stream.rs}`, `src/wake.rs`; `crates/loams/{Cargo.toml,src/main.rs,src/server.rs}`. Tests `crates/loams-jobs/tests/service.rs`, `crates/loams/tests/jobs_feature.rs`.

**Interfaces:**
- `JobsService` implements the generated trait over `Jobs`; `Ctx` from the transport (the loopback principal `local` until MT1).
- `loams` feature `jobs = ["dep:loams-jobs"]`, not in `default`; `jobs-tikv = ["jobs", "tikv", "loams-jobs/tikv"]`.
- Served on the main API port (`--listen`) with the other Connect services (Q103 ruling). Flags on `dev` and `standalone`: `--jobs-store` (`embedded` default on `dev`; `tikv://…` required on `standalone` and `cluster`), `--no-jobs`.
- `wake.rs`: one in-process notifier per `(ns, queue)`, fired by local enqueue and promote; across nodes, one shared poller per `(ns, queue)` per node while any lease waits (default every 200 ms).
- The instance catalogue row for `loams.jobs.v1` (`available` when served).

Tests:
- `default_features_exclude_jobs` (`cargo tree -p loams -e normal`).
- `standalone_refuses_embedded_store`.
- `non_loopback_listen_with_jobs_refused_before_mt1` (and accepted with `--no-jobs`), as Graph's `GraphListenNotLoopback`.
- `jobs_served_on_main_api_port`.
- `namespace_comes_from_ctx`: a request whose payload headers name another namespace is served in the caller's.
- `connect_grpc_and_grpc_web_all_served`.
- `lease_long_poll_returns_on_enqueue_within_50ms` and `lease_wait_capped_at_30s`.
- `errors_carry_registered_reasons`.
- `schedule_without_durable_is_durable_unavailable` (built without `durable`).

Commit `feat(jobs): JobsService on the main API port`.

## JB1b — Events, `watch`, schedules, flows, TiKV and throughput (Tasks 13–18)

### Task 13: The outbox relay and the event streams

**Files:** `src/loops/relay.rs`. Tests `tests/relay.rs`.

**Interfaces:** the relay shares the sweep's owner per shard. It reads `O/<queue>/<shard>/` in order, appends records (shared contract) to `QueueRec.stream` partition `shard` through `StreamProducer`, then deletes the rows. The stream is created with `S` partitions and 7-day retention through the internal reserved-name path (Task 0). Until D72 the append is at least once (W10). `settle_promise` rows (Task 16) are settled against Resonate instead and retried until accepted.

Tests:
- `events_in_outbox_order_per_shard`
- `crash_after_append_before_delete_duplicates_with_same_seq`
- `consumer_dedup_by_seq_sees_each_once`
- `stream_created_with_reserved_name`
- `users_cannot_create_jobs_streams` (`_jobs.x` refused through the public API)
- `relay_lag_metric_reported` (Task 37 names it)

Commit `feat(jobs): outbox relay to per-queue event streams`.

### Task 14: `watch`

**Files:** `src/watch.rs`, `src/service/stream.rs`. Tests `tests/watch.rs`.

**Interfaces:** `watch(cx, selector, cursor)`: without a cursor, one snapshot at one `Ts` (`Store::snapshot`) of the selected jobs and each shard's outbox head, then subscribe each partition and skip records with `seq ≤` the snapshot's head; with a cursor, resume each partition after its offset. Cursor: S = 1 the offset; S > 1 an opaque base64 vector of offsets, merged by `ts_ms` then shard. Heartbeat every 15 s. In `loams dev` without a stream engine, tail the outbox (§26 §6.8). Selectors: job, queue, flow, schedule.

Tests:
- `nothing_between_snapshot_and_subscribe`: writes injected between the snapshot and the subscription are all delivered, once.
- `resume_from_cursor_no_gap_no_dup`
- `vector_cursor_resumes_every_partition`
- `heartbeat_every_15s` (paused clock)
- `cursor_beyond_retention_is_out_of_range` (`failed_precondition`, reason `cursor_expired`, added to the registry)
- `watch_other_namespace_not_found`

Commit `feat(jobs): resumable watch over snapshot and event log`.

### Task 15: Schedules on Resonate

**Files:** `src/schedule.rs`. Tests `tests/durable.rs` (feature `durable`; the durable debug clock).

**Interfaces:**
- `schedule(cx, ScheduleSpec{id, rule: Cron{expr, tz} | Every{interval}, target: Enqueue{queue, task, opts} | Durable{group, function, args}, limit?, start?, end?, offset?, immediately})`, upsert by id: delete and create in one Loams operation.
- Cron: a Resonate schedule with promise id template `jobs:tick:<ns>:<id>:{{.timestamp}}`, targeting `inproc://any@loams`; the Loams function `loams.jobs.tick` enqueues with the tick's promise id as idempotency key. `tz` is converted to UTC cron by Loams (Task 0 checks Resonate's parser); a DST shift is handled by re-registering at the transition.
- `Every`: a durable function looping `ctx.sleep(interval)` → enqueue with key `jobs:every:<ns>:<id>:<n>`.
- `ScheduleRec` keeps the rule, limits and counters for listing and BullMQ's getters.

Tests:
- `tick_processed_twice_enqueues_once`
- `restart_between_ticks_misses_none`
- `upsert_replaces_rule`
- `every_loop_survives_restart`
- `limit_and_end_date_stop_schedule`
- `tz_dst_transition_fires_once` (Europe/Berlin, last Sunday of October)
- `durable_target_starts_function_with_tick_id`
- `queue_mode_creates_no_promises` (§26 §8.3): 1,000 plain jobs leave the durable store's promise count unchanged.

Commit `feat(jobs): schedules on the embedded durable server`.

### Task 16: Flows

**Files:** `src/flow.rs`. Tests in `tests/durable.rs`.

**Interfaces:** `flow(cx, FlowSpec{id?, nodes, on_child_failure per node})` (§26 §8.2): the root promise id is the flow id; the function `loams.jobs.flow` enqueues ready nodes with the step's promise id as idempotency key and `parent` set; each node has a completion promise `flow:<id>:<node>` in the root's origin, settled by an outbox `settle_promise` row written by `complete`. Failure policies `Fail | Continue | Ignore | Remove`. A flow over 1,000 nodes per origin is split into child flows (§21 Q42); over 100,000 nodes is `flow_too_large`.

Tests:
- `chain_runs_in_order`, `group_runs_in_parallel`, `chord_body_after_all`
- `bullmq_tree_parent_waits_children`
- `fail_parent_on_failure`, `continue_parent_on_failure`, `ignore_dependency_on_failure`, `remove_dependency_on_failure`
- `flow_survives_kill_of_node_running_it` (in-process restart of the runtime)
- `flow_id_is_idempotent`
- `large_flow_split_into_child_origins`

Commit `feat(jobs): flows with promise joins`.

### Task 17: TiKV in production

**Files:** `src/store/kv.rs`, `crates/loams/src/server.rs`. Tests `tests/store_tikv.rs`, `tests/faults_tikv.rs`.

**Interfaces:** keyspace `loams_jobs` on the Live cluster's PD; a namespace may be given its own keyspace (`loams_jobs_<ns-hash>`) by configuration (large tenants, §21 §5.1). Commit tokens on every mutating transaction. GC barrier held by `watch` snapshots for at most 30 s.

Tests:
- `undetermined_enqueue_resolves_by_token`
- `region_error_during_lease_retried`
- `tso_restart_does_not_move_deadlines_back`
- `own_keyspace_namespace_isolated`
- the conformance macro and the model test under the fault matrix (nightly, 10,000 cases).

Commit `feat(jobs): tikv store under the fault plan`.

### Task 18: Throughput and the hot-key bench

**Files:** `scripts/jb1/bench.sh`, `crates/loams-jobs/benches/lease.rs`, `bench/results/jobs/README.md`. Runs on the bench runner, not on PRs.

**Interfaces:** a closed-loop bench: P producers, W workers with batch `max`, queue shards S ∈ {1, 4, 16}, with and without a rate limit and global concurrency, on a 3-node TiKV. It reports enqueues/s, leases/s, p50/p99 lease latency, conflict retries per lease, and the outbox relay lag.

Tests:
- `bench_smoke` (PR CI, embedded store, 10 s): runs and writes a result file.
- The bench run records numbers next to §26 §6.9's estimate (2,000 leases/s at S = 1) and publishes them, with the hardware, in `bench/results/jobs/README.md` and `docs/guides/jobs/semantics.md`. **The number is a published benchmark, not a gate** (JB1-Q2 ruling): no merge or GA waits on it, and the docs claim only what was measured.

Commit `bench(jobs): lease throughput and hot keys`.

## JB1c — Clients and Celery (Tasks 19–25)

### Task 19: Generated Python and TypeScript clients

**Files:** `sdks/python/buf.gen.yaml` and `buf.gen.proto.yaml` (package map `loams.jobs.v1`), `sdks/python/src/loams/jobs/`, `sdks/typescript/packages/client/src/jobs/`, `sdks/conformance/` (jobs fixtures). Tests `sdks/python/tests/test_jobs.py`, `sdks/typescript/packages/client/test/jobs.test.ts`.

**Interfaces:** `loams.jobs` module in both SDKs from the facade annotations: `enqueue`, `lease`, `extend`, `complete`, `report`, `schedule`, `flow`, admin, results, `watch` (async iterator), `get_blob`. Every `enqueue` adds an idempotency key (UUIDv7) made once per call and reused across its retries (§26 §5.3).

Tests:
- `enqueue_retry_reuses_idempotency_key` (both languages, with the conformance fault server)
- `watch_resumes_with_cursor` (both)
- `errors_map_to_reasons` (both)
- CI regenerates and fails on a diff.

Commit `feat(sdk): loams.jobs clients for python and typescript`.

### Task 20: `loams-celery`: the kombu transport

**Files:** `integrations/celery/{pyproject.toml,src/loams_celery/{__init__.py,transport.py}}`, `tests/test_transport.py`. CI job `jb1-adapters.yml` (`celery` matrix) starts `loams dev --jobs`.

**Interfaces:** `loams_celery.transport:Transport` (kombu virtual transport, `driver_type = "loams"`, `implements` direct/topic/fanout, `asynchronous = False`) and `Channel` mapping per §26 §7.1.2: `_put` → `enqueue` (content type `application/x-kombu+json`), `_get`/`_get_many` → `lease` with `wait` and `max` = free prefetch, `basic_ack` → `complete(Ok)`, `basic_reject(requeue=True)` → `Release`, `basic_reject(requeue=False)` → `Fail{dead_letter}`, `basic_recover` → `Release` for all, `_size`/`_purge`/`_delete`/`_new_queue`/`_has_queue` → admin. An extender thread calls `extend` every `visibility_timeout / 3` for every unacked tag. `import loams_celery` registers `TRANSPORT_ALIASES["loams"]`; the docs recommend `broker_transport = "loams_celery.transport:Transport"`. Token from `broker_transport_options["token_env"]`, default `LOAMS_TOKEN`; never from the URL.

Tests:
- `put_get_ack_roundtrip`
- `reject_requeue_releases_without_attempt`
- `reject_no_requeue_dead_letters`
- `unacked_extended_while_alive` (task runs 3 × visibility; not redelivered)
- `killed_worker_jobs_redelivered`
- `alias_and_explicit_transport_both_work`
- `token_never_read_from_url`

Commit `feat(celery): kombu transport over loams.jobs.v1`.

### Task 21: Priorities, ETA and retries

**Files:** `integrations/celery/src/loams_celery/priorities.py`, `transport.py`. Tests `tests/test_semantics.py`.

**Interfaces:** priority 0 or unset → FIFO band; 1–9 → Loams 1–9 (Redis order); transport option `priority_order = "amqp"` reverses it. `eta`/`countdown` → `not_before`; `expires` → `expires`; `job_key` = `"<id>:<retries>"`.

Tests:
- `priority_redis_order`, `priority_amqp_order_option`
- `long_eta_not_redelivered` (ETA = 3 × visibility; delivered once, when due)
- `retry_same_id_new_job_key` (`Task.retry` under `acks_late` while the original is active)
- `expired_task_not_run`

Commit `feat(celery): priorities, eta and retries`.

### Task 22: The result backend and chord counter

**Files:** `integrations/celery/src/loams_celery/backend.py`; entry point `celery.result_backends: loams = loams_celery.backend:LoamsBackend`. Tests `tests/test_backend.py`.

**Interfaces:** `LoamsBackend(BaseKeyValueStoreBackend)` with `get`, `mget`, `set`, `delete`, `incr` (member = task id), `expire`; `implements_incr = True`; token from `result_backend_transport_options["token_env"]`. `AsyncResult.get` polls through `wait_for_pending`; push waiting over `watch` is a follow-up (J1.x).

Tests:
- `result_roundtrip`, `group_result_save_restore`
- `chord_body_once_under_redelivery`
- `acks_late_crash_before_incr_recounts`
- `result_expires_honoured`
- `no_chord_unlock_task_used`

Commit `feat(celery): result backend with native chord counter`.

### Task 23: Bindings, fanout, remote control and events

**Files:** `integrations/celery/src/loams_celery/bindings.py`, `transport.py`. Tests `tests/test_control.py`.

**Interfaces:** `get_table`/`queue_bind` over `G/` keys through `QueueAdmin(Bind/Unbind/Bindings)` (Task 10 adds the three ops); `_put_fanout` to a broadcast stream `_celery.<exchange>` (W5 mapping), each consumer subscribing from latest.

Tests:
- `topic_routing_across_processes`
- `inspect_ping_reaches_all_workers`, `revoke_broadcast_reaches_running_worker`
- `celery_events_received_by_monitor`
- `mingle_and_gossip_off_documented` (asserts `driver_type` is not in Celery's lists, so a Celery change is noticed)

Commit `feat(celery): bindings, fanout and remote control`.

### Task 24: Beat on Loams schedules

**Files:** `integrations/celery/src/loams_celery/{beat.py,cli.py}`; entry point `celery.beat_schedulers: loams = loams_celery.beat:LoamsScheduler`; console script `loams-celery`. Tests `tests/test_beat.py`.

**Interfaces:** `LoamsScheduler` upserts every `beat_schedule` entry as a schedule (id = entry name) on start and on change; `crontab` → cron, `timedelta` → `every`; `solar` and custom schedule classes refused with a message (Q101 ruling). `loams-celery sync-schedules <app>` does the same from CI.

Tests:
- `beat_twice_no_duplicate_ticks`
- `beat_absent_after_sync_still_ticks`
- `crontab_and_timedelta_mapped`
- `solar_refused_with_message`
- `removed_entry_unscheduled`

Commit `feat(celery): beat scheduler on loams schedules`.

### Task 25: The Celery gate and durable mode

**Files:** `integrations/celery/ci/celery-integration.sh`, `integrations/celery/tests/test_durable.py`, `jb1-adapters.yml`. Python helper `sdks/python/src/loams/resonate.py` lands in Task 30; this task uses it.

**Interfaces:** Celery's `t/integration` at the pinned tag with `broker_url = loams://…`, `result_backend = loams://…`; an exclusions file `integrations/celery/ci/exclusions.tsv`, each line justified (only Redis- or AMQP-specific tests).

Tests:
- The Celery integration suite green, canvas tests included.
- `durable_task_resumes_after_worker_kill` (§26 §7.1.8: a killed worker mid-step; finished steps not repeated).
- `exclusions_are_justified` (every line has a reason and an upstream path).

Commit `ci(celery): celery integration suite on loams dev`.

## JB1d — BullMQ (Tasks 26–32)

### Task 26: BullMQ spike

**Files:** this plan's rulings; `integrations/bullmq/SPIKE.md` is not written (findings go in the rulings).

Steps:
1. At the pinned BullMQ version, list `IQueueBackend`'s methods (§26 says about 81) and map each to a `loams.jobs.v1` call or to "not needed" with a reason.
2. Run the backend-neutral suite (`tests/*.test.ts` minus `*.redis.test.ts`) against BullMQ's own Postgres backend; record which tests fail there (Redis-specific in practice).
3. Check that `QueueEvents` passes `lastEventId` back unparsed (decides the vector cursor, §26 §7.2.1).
4. Check the event-name list against the shared contract.
5. Commit `docs(jb1): task 26 rulings`.

### Task 27: `@loams/bullmq`: the backend core

**Files:** `integrations/bullmq/{package.json,src/{index.ts,backend.ts,tokens.ts}}`, `test/core.test.ts`.

**Interfaces:** `loamsBackend({url, token, namespace?}): BackendFactory`; `addJob`/`addJobs` → `enqueue`/`enqueueBulk`; `waitForJob`/`moveToActive` → `lease`; `extendLock(s)` → `extend`; `moveToFinished`, `moveToDelayed`, `moveToWaitingChildren`, `retryJob` → `complete`; `moveStalledJobsToWait` a no-op. `tokens.ts` maps BullMQ's worker token string to the `LeaseToken` in memory. `index.ts` re-exports BullMQ's classes bound with `withBackend`. Peer `bullmq` `>=6.3.9 <6.4`.

Tests: `add_and_process`, `retry_with_backoff`, `stalled_job_retried_by_server`, `lock_extended_by_bullmq_worker`, `rate_limit_error_pauses_queue`, `option_a_and_option_b_equivalent`.

Commit `feat(bullmq): IQueueBackend core over loams.jobs.v1`.

### Task 28: Getters, admin, metrics, logs and schedulers

**Files:** `integrations/bullmq/src/{getters.ts,schedulers.ts}`, `test/getters.test.ts`.

**Interfaces:** `getCounts`, `getRanges`, job getters, logs, `getMetrics`, workers → `query`/`job_admin`/`queue_admin`; pause, resume, drain, clean, obliterate, promote, retry-all; `upsertJobScheduler` (pattern, every, limit, startDate/endDate, tz, offset, immediately) → `schedule`; scheduler listing → `ListSchedules`.

Tests: `counts_by_state`, `get_jobs_types_range_asc`, `job_logs`, `metrics_completed_failed`, `upsert_job_scheduler_replaces`, `scheduler_every_and_pattern`, `obliterate_force`.

Commit `feat(bullmq): getters, admin and job schedulers`.

### Task 29: Flows and events

**Files:** `integrations/bullmq/src/{flows.ts,events.ts}`, `test/flows.test.ts`.

**Interfaces:** `addFlow` → `flow`; `getChildrenValues` from the job store; `publishEvent`/`readEvents(id, blockTimeout)` → `watch`, with BullMQ's event id = the cursor (vector for S > 1 if Task 26 allows, else BullMQ events on one partition). `trimEvents` maps to a retention change.

Tests: `flow_parent_after_children`, `children_values`, `four_failure_flags`, `queue_events_receive_all_names`, `queue_events_resume_last_event_id`.

Commit `feat(bullmq): flows and queue events`.

### Task 30: The `loams` Resonate helpers

**Files:** `sdks/python/src/loams/resonate.py`, `sdks/typescript/packages/durable/` (`@loams/durable`). Tests in both SDKs.

**Interfaces:** `loams.resonate(group=…)` and `loamsResonate({ group })` return Resonate clients configured from `LOAMS_URL`, `LOAMS_TOKEN`, `LOAMS_NAMESPACE`: the durable endpoint, the token, the group. Nothing else (§26 §8.4).

Tests: `helper_sets_url_token_group_only`, `token_from_env_never_logged`, `durable_bullmq_job_resumes_after_kill` (§26 §7.2.6).

Commit `feat(sdk): loams resonate helpers for python and typescript`.

### Task 31: The BullMQ gate

**Files:** `integrations/bullmq/exclusions.tsv`, `jb1-adapters.yml` (`bullmq` matrix).

**Interfaces:** BullMQ's backend-neutral suite at the pinned version with `setDefaultBackendFactory(loamsBackend(...))` against `loams dev --jobs`; exclusions are Task 26's Postgres failures plus any justified Loams difference.

Tests: the suite green; `exclusions_are_justified`; a weekly job runs the suite against the newest BullMQ 6.x minor and opens an issue on failure (J-R1).

Commit `ci(bullmq): backend-neutral suite on loams dev`.

### Task 32: The Python BullMQ backend (J2.x)

**Files:** `integrations/bullmq/python/` (PyPI `loams-bullmq`), tests there.

**Interfaces:** BullMQ Python's backend interface (`python/bullmq/backends`) over the Python `loams.jobs` client.

Tests: BullMQ Python's backend-neutral tests that exist at the pin, with exclusions justified.

Commit `feat(bullmq): python backend`.

## JB1e — The usage hooks (Tasks 33–40)

### Task 33: The hooks contract v1

**Files:** create `crates/loams-hooks/` (`src/{lib.rs,families.rs,labels.rs}`, `tests/contract.rs`) and `docs/api/usage-hooks-v1.md`.

**Interfaces:** the shared contract's `CONTRACT`, `FAMILIES`, `labels`. The page lists every family (name, kind, unit, labels, owner, per-namespace or not), the cgroup layout, the pod labels, the tenant header and the access-log fields, with rules modelled on `reasons.md`: within v1 a name or label is never renamed or removed; additions are allowed; a removal is v2 (Q-UH-2 ruling). `loams-hooks` has no dependency outside `std` except `http` (Task 39).

Tests:
- `contract_page_matches_families`: parses the page's tables and compares them with `FAMILIES`, both ways.
- `names_are_prometheus_valid` and `counters_end_in_total`.
- `no_billing_names`: no family, label or constant contains a billing word (the Global Constraints list).
- `per_namespace_families_have_org_and_namespace_labels`.

Commit `feat(hooks): usage-hooks contract v1`.

### Task 34: The registry, the admin listener, the cardinality guard and OTLP delta export

**Files:** `crates/loams/src/metrics.rs` (W8), `crates/loams-hooks/src/active.rs`, `crates/loams/src/{main.rs,server.rs}`. Tests `crates/loams/tests/metrics.rs`, `crates/loams-hooks/tests/active.rs`.

**Interfaces:** the registry and admin listener per LV1 Task 27 (or reused). `ActiveNamespaces { cap: 10_000, idle_ttl: 15 min }`: a node exports a per-namespace series only for namespaces active on it; an idle namespace's series are removed after the TTL; past the cap the least recently active are evicted and `loams_hooks_namespaces_evicted_total` counts it. Flags: `--metrics-per-namespace prometheus|otlp|off` (default `prometheus`), `--otlp-endpoint`; with `otlp`, per-namespace families are exported as OTLP metrics with delta temporality and omitted from `/metrics` (Q-UH-1 ruling: both, capped).

Tests:
- `active_namespace_cap_bounds_series`
- `idle_namespace_series_removed_after_ttl`
- `otlp_mode_omits_per_namespace_from_scrape`
- `otlp_delta_temporality` (in-memory exporter)
- `metrics_scrape_on_admin_listener_only`

Commit `feat(hooks): metrics registry, cardinality guard and otlp delta`.

### Task 35: The engine's per-namespace families

**Files:** `crates/loams/src/api/{collections.rs,streams.rs,query.rs,sql.rs,hot.rs}` and the ingest paths of Flight, ES, Qdrant and `stream-grpc` (Task 0 lists them). Tests `crates/loams/tests/namespace_metrics.rs`.

**Interfaces:** `loams_namespace_logical_bytes_written_total`, `loams_namespace_logical_bytes_stored` (gauge, from the metastore's catalog sizes, refreshed every 60 s), `loams_namespace_bytes_queried_total`, `loams_namespace_queries_total`, `loams_namespace_hot_gb_hours_total`, labels `org` (W12) and `namespace`. Logical bytes are D103's units: counted once per acknowledged write, not per retry or replica.

Tests:
- `each_ingest_path_counts_once` (REST, Flight, ES `_bulk`, Qdrant upsert, stream produce)
- `idempotent_retry_not_double_counted`
- `queried_bytes_counted_per_query`
- `hot_gb_hours_accumulate` (paused clock)
- `labels_exact`

Commit `feat(hooks): engine per-namespace usage families`.

### Task 36: The durable server's families

**Files:** `crates/loams-durable/src/{metrics.rs,embed.rs}`. Tests `crates/loams-durable/tests/metrics.rs` (feature `durable` build session).

**Interfaces:** `loams_durable_promises_created_total` and `loams_durable_timers_scheduled_total`, labels `org`, `namespace`, counted on the create path (a wrapper around `DurableServer::process` and the HTTP listener's handler) only when the answer says the promise was created, never on a replayed create. Namespace from the promise's tenant tag (§21 §5.1).

Tests: `create_counts_once`, `replayed_create_not_counted`, `timer_counted_on_schedule`, `inproc_and_http_both_counted`.

Commit `feat(durable): promise and timer counters`.

### Task 37: The jobs service's metrics, traces and logs

**Files:** `crates/loams-jobs/src/metrics.rs`, `deploy/jobs/{alerts.yaml,dashboards/jobs.json}`. Tests `crates/loams-jobs/tests/observability.rs`.

**Interfaces:** §26 §6.11's families, exactly named, plus `loams_jobs_relay_lag_seconds` and `loams_jobs_lease_conflicts_total`; `task` label capped at 100 per queue (the rest as `_other`). Spans `jobs.rpc`, `jobs.lease`, `jobs.complete`, `kv.txn`; a W3C `traceparent` from the task headers is the parent of the job's spans. Log fields `request_id`, `trace_id`, `queue`, `job`, `task`, `principal`. Alerts: oldest waiting age, DLQ depth growth, relay lag, stalled rate, fenced rate.

Tests:
- `metrics_exposed_with_expected_names` (against `FAMILIES`)
- `task_label_capped`
- `traceparent_propagated_to_job_spans`
- `canary_payload_never_observed`: a payload, a result, an error message and a log line containing `CANARY-5c1e` pass through every RPC; no log line, span attribute or metric label contains it (job log lines are stored, not logged).
- `alerts_yaml_is_valid` (`promtool check rules` in CI)

Commit `feat(jobs): metrics, traces, logs and alerts`.

### Task 38: The cgroup layout and pod labels

**Files:** `crates/loams-hooks/src/{cgroup.rs,labels.rs}`. Tests `crates/loams-hooks/tests/cgroup.rs`.

**Interfaces:** `cgroup::t0_scope`, `cgroup::t1_scope` (§27 §3.2), `FinishedScopeReaper`: given a delegated subtree, removes a T0 or T1 scope whose `cgroup.events` says `populated 0` only after `retention` (default 10 min), so an outside reader can take a final reading. `labels::pod_labels(&SandboxIdentity{org, namespace, function, tier, version, runner})`. The supervisor (F1) and `KnativeRunner` (MT2) call these; JB1 adds a line to each of those plans' "consumes" lists (Task 47).

Tests:
- `paths_match_contract`
- `org_with_unsafe_characters_refused` (no `/`, `.slice` injection)
- `reaper_keeps_scope_for_retention` and `reaper_removes_after_retention` (a fake cgroupfs in a temp dir under the target directory, not `/tmp`)
- `reaper_never_removes_populated_scope`
- `pod_labels_complete` (`loams.dev/tier = t2`, `loams.dev/runner = knative` for MT2's case)

Commit `feat(hooks): cgroup layout, finished-scope reaper and pod labels`.

### Task 39: The tenant header and the Envoy access-log sink

**Files:** `crates/loams-hooks/src/tenant.rs`, `deploy/envoy/access-log/{envoy.yaml,README.md}`, `jb1.yml` (an Envoy validate step). Tests `crates/loams-hooks/tests/tenant.rs`.

**Interfaces:** `TenantHeaderLayer` (tower): removes every client-supplied `x-loams-tenant` on entry, and after the route resolves sets `x-loams-tenant: <org>/<namespace>` from the resolved route. The Envoy fragment: a route-level `request_headers_to_remove: [x-loams-tenant]` on the listener, metadata `filter_metadata["loams"]`, and an access-log sink (gRPC ALS or OpenTelemetry) **off by default**, with the fields: tenant, route, status, bytes in, bytes out, duration. `loams-gateway` uses the layer when it exists (W11).

Tests:
- `client_tenant_header_stripped`
- `header_set_after_route_resolution`
- `multiple_client_headers_all_stripped`
- `envoy_config_validates` (`envoy --mode validate` with the pinned image)
- `access_log_sink_off_by_default`

Commit `feat(hooks): tenant header layer and envoy access-log fragment`.

### Task 40: The hooks conformance kit

**Files:** `crates/loams-hooks/src/lib.rs` (`conformance` module behind feature `conformance`), `crates/loams-hooks/tests/kit.rs`.

**Interfaces:** `hooks_conformance::scrape_check(text, owner)`: given a scrape and an owner, every family that owner emits is present with exactly the contract's labels and no others; `cgroup_check(root)`; `pod_labels_check(map)`. RN1, F1, MT2 and the gateway run it against their own output.

Tests: `kit_accepts_engine_scrape`, `kit_rejects_extra_label`, `kit_rejects_renamed_family`, `kit_accepts_runner_families_with_w13_names`.

Commit `feat(hooks): conformance kit for producers`.

## JB1f — Production (Tasks 41–47)

### Task 41: Authentication and authorization

**Files:** `crates/loams-jobs/src/authz.rs`, `crates/loams/src/server.rs`. Tests `tests/authz.rs`. Needs MT1 on `dev`.

**Interfaces:** actions `jobs:enqueue`, `jobs:consume`, `jobs:admin`, `jobs:schedule`, `jobs:engine` (D213), checked per queue; an agent policy may grant `jobs:enqueue` on one queue (§19 §5.1). `Ctx.principal` from MT1's verifier. With MT1, `--listen` may bind non-loopback with jobs enabled when TLS and auth are configured (MT1 Task 7's rule, as for Graph). The adapters pass the token as a bearer header.

Tests: `consume_token_cannot_lease_other_queue`, `enqueue_only_agent_cannot_admin`, `other_namespace_token_sees_not_found`, `lease_token_useless_across_namespaces`, `non_loopback_requires_tls_and_auth`, `celery_and_bullmq_send_bearer_token`.

Commit `feat(jobs): authorization per queue and action`.

### Task 42: Quotas

**Files:** `crates/loams-jobs/src/quota.rs`. Tests `tests/quota.rs`.

**Interfaces:** per-namespace limits (D65), from configuration or the open control plane's limits API (§41 §9; read through MT4's limits record when present): enqueue rate, payload bytes per job (64 MiB), stored jobs, active leases, schedules, flows, concurrent engine runs and their CPU and memory. Refusals are `jobs_rate_limited` (with `retry_after`) or `payload_too_large`. A queue's own rate limit never errors (§26 §6.9).

Tests: one refusal test per limit (`enqueue_rate_quota_refused`, `stored_jobs_quota_refused`, `active_leases_quota_refused`, `schedules_quota_refused`, `flows_quota_refused`, `engine_runs_quota_refused`), `queue_rate_limit_returns_fewer_not_error`.

Commit `feat(jobs): namespace quotas`.

### Task 43: Namespace deletion and erasure

**Files:** `crates/loams-jobs/src/erase.rs`. Tests `tests/erase.rs`.

**Interfaces:** `erase_namespace(ns)`: deletes the `t/<ns>/` range in bounded transactions with a resume cursor, payload and result objects, the `_jobs.*` and `_celery.*` streams, schedules (Resonate) and engine runs (Task 48's `delete`), and records progress so a crash resumes. Called from the namespace deletion path (D68).

Tests: `erase_removes_every_prefix_and_object`, `erase_resumes_after_crash`, `erase_cancels_schedules`, `erase_leaves_other_namespaces`.

Commit `feat(jobs): namespace erasure`.

### Task 44: Chaos and the nemesis gate

**Files:** `scripts/jb1/nemesis/{run.sh,checker.py,workload.py}`, `jb1.yml` (nightly). 

**Interfaces:** a 3-node `standalone` cluster on TiKV, a workload of Celery and BullMQ workers and producers with idempotency keys, and a nemesis that kills engine nodes, TiKV stores and PD leaders, partitions, and pauses processes. The checker reads the event streams and the final store.

Tests: `nemesis_no_lost_job`, `nemesis_no_double_completion`, `nemesis_no_stuck_lease` (every job finished or in a defined state at quiescence), `nemesis_schedules_one_job_per_tick`, `nemesis_events_complete_per_shard`.

Commit `test(jobs): nemesis gate`.

### Task 45: Packaging and release

**Files:** `crates/loams/Cargo.toml` (variant features), the release workflows, `integrations/celery/pyproject.toml`, `integrations/bullmq/package.json`, `release/nfpm.yaml` if it lists features.

**Interfaces:** the `full` variant gains `jobs` and `jobs-tikv` (D286); `loams-celery` on PyPI, `@loams/bullmq` and `@loams/durable` on npm, Apache-2.0, BullMQ's MIT notice kept for anything adapted (§26 §12.2); `loams pkg add celery|bullmq|durable` mappings (D296) where the CLI's package table lives (Task 0 locates it; CLI2 owns the table).

Tests: `full_variant_serves_jobs`, `standard_variant_has_no_jobs`, `packages_have_licence_and_notice`, `pkg_add_maps_logical_names`.

Commit `build(jobs): release variants and adapter packages`.

### Task 46: Security review

**Files:** `docs/security/loams-jobs-threat-model.md`, tests where findings need them.

**Interfaces:** a threat model of the listener, tokens, lease tokens, payload handling, result keys, event streams, the Spark Connect proxy and engine namespaces (from Task 48 on); each threat has a mitigation and a test or a tracked issue.

Tests: `lease_token_forgery_cannot_complete_without_epoch` (a guessed token with a wrong epoch is fenced), `result_key_namespaced`, `blob_ref_cannot_cross_namespace`, `engine_network_policy_denies_cross_namespace` (kind, from Task 49), plus any test a finding adds.

Commit `docs(jobs): threat model and security tests`.

### Task 47: Docs, runbooks and plan status

**Files:**
- `docs/guides/jobs/{semantics.md,celery.md,bullmq.md,durable-mode.md}`: §26 §6.1's wording on delivery, the migration steps, the documented differences (mingle, gossip, early-ack chord counts, retention by time for events);
- `docs/runbooks/jobs/{stuck-queue.md,dlq.md,relay-lag.md,tikv-hot-key.md,erase.md}`;
- `docs/api/route-map.md`; remove `unstable: true` and the `buf.yaml` ignore at GA so `buf breaking` protects the package;
- §26 (as built: W1–W7, W9, and the details accepted as amended under JB1-Q5), §27 §3.1 (W13: keep `loams_function_*`, add the `runner` label, and state the naming rule), §27 §3.7 (the open observer exports `loams_function_*`), §27 §6 (Q-UH-1, Q-UH-2 rulings); a note for the integrator that D549's `loams_runner_*` names are amended by JB1-Q1;
- the "consumes" lines in RN1, F1's plan when written, MT2 (Task 38, Task 40);
- `docs/plans/README.md` (a JB1 row under a new "Track J" heading) and `docs/design/12-roadmap-testing-risks.md`.

Steps: each runbook step is executed once on `loams dev` or kind and marked verified. Commit `docs(jobs): guides, runbooks and status`.

## JB1g — Engines: PySpark on Sail and Flink (Tasks 48–55)

Entry gates: MT1 merged (Task 50 beyond loopback), M2's `format("loams")` (Task 49's data examples), M4's Iceberg through Lakekeeper (Tasks 49–52's table gates), M5's Kafka gateway (Task 54's reading of Loams streams). A task whose gate is not met stops at its tests that need it and marks them `#[ignore = "gate: <M>"]`.

### Task 48: The engine-runner framework

**Files:** `proto/loams/jobs/v1/engines.proto`, `crates/loams-jobs-engines/src/{lib.rs,runner.rs,local.rs,k8s.rs}`, `crates/loams/Cargo.toml` (feature `jobs-engines`, and `jobs-k8s` for kube). Tests `crates/loams-jobs-engines/tests/runner.rs`.

**Interfaces:** `EngineRunner`, `EngineJobs` (shared contract); `EngineJob::{SparkBatch, SparkSubmit, StreamingSql, FlinkDeployment}`; `EngineRunRec` under `N/`; each lifecycle is a durable workflow (`loams.jobs.engine`) of declarative applies, so a replay converges. `LocalRunner` runs child processes in `loams dev`; `K8sRunner` applies objects in the namespace's Kubernetes namespace with a NetworkPolicy that allows only the Loams endpoints, the object store and Lakekeeper. Run events go to the run's event stream.

Tests: `apply_is_idempotent_by_spec_hash`, `workflow_replay_converges`, `cancel_deletes_resources`, `run_events_streamed`, `default_features_exclude_kube`.

Commit `feat(engines): engine-runner framework`.

### Task 49: Sail per namespace

**Files:** `crates/loams-jobs-engines/src/sail.rs`, `deploy/engines/sail/`. Tests `tests/sail.rs` (local process; kind in `jb1-engines.yml`).

**Interfaces:** one Sail server per namespace (Q96 ruling): a child process in dev (`sail spark server --port <p>`), a `kubernetes-cluster` mode Deployment in cloud, scaled to zero after 15 min idle and started on first connection; its Iceberg REST catalog points at Lakekeeper with credential vending. Sail 0.7.1 pinned by digest.

Tests: `sail_starts_on_demand`, `sail_scales_to_zero_after_idle`, `pyspark_examples_pass_on_sail` (§17's examples), `iceberg_written_by_sail_reads_in_loams` (gate M4), `format_loams_source_works` (gate M2).

Commit `feat(engines): sail per namespace`.

### Task 50: The Spark Connect proxy

**Files:** `crates/loams-jobs-engines/src/spark_proxy.rs`, `crates/loams/src/server.rs`. Tests `tests/spark_proxy.rs`.

**Interfaces:** a gRPC proxy for `sc://<ns>.spark.<domain>:443`: authenticates `authorization: Bearer` metadata (never a URI token), resolves the namespace, wakes its Sail, and forwards the stream, sticky by Spark Connect's `session_id`. Loopback-only before MT1. `loams.spark_session()` in the Python SDK builds the session with a `ChannelBuilder` that adds the header from `LOAMS_TOKEN`.

Tests: `session_routed_only_to_own_namespace`, `token_in_uri_refused`, `sticky_session_same_server`, `wake_on_first_connection`, `pyspark_versions_3_5_to_4_2_send_bearer` (one client per version).

Commit `feat(engines): spark connect proxy`.

### Task 51: Spark batch jobs and `loams spark check`

**Files:** `crates/loams-jobs-engines/src/spark_batch.rs`, the CLI's `spark check` subcommand (CLI1's crate). Tests `tests/spark_batch.rs`.

**Interfaces:** `SparkBatch { entrypoint, args, conf, python_deps }`: uploads to the object store, runs a short-lived driver container against the namespace's Sail, streams its logs into run events, records the exit status, retries by policy; `schedule` can target it. `loams spark check <script>` runs it on Sail and reports the unsupported calls from Sail's errors.

Tests: `batch_runs_and_records_exit`, `batch_retried_by_policy`, `nightly_schedule_targets_batch`, `spark_check_reports_rdd_use`.

Commit `feat(engines): spark batch jobs and spark check`.

### Task 52: The Spark fallback

**Files:** `crates/loams-jobs-engines/src/spark_fallback.rs`, `deploy/engines/spark-fallback/`. Tests `tests/spark_fallback.rs` (kind).

**Interfaces:** per the Q104 ruling (Loams-managed): an Apache Spark 4 Spark Connect server per namespace (port 15002) and `SparkSubmit` through the Kubeflow Spark Operator, both reading Lakekeeper's tables.

Tests: `sail_unsupported_job_runs_on_fallback_unchanged`, `rdd_job_runs_via_spark_operator`, `fallback_reads_same_iceberg_tables` (gate M4).

Commit `feat(engines): apache spark fallback`.

### Task 53: Streaming SQL on RisingWave

**Files:** `crates/loams-jobs-engines/src/risingwave.rs`, `deploy/engines/risingwave/`. Tests `tests/risingwave.rs`.

**Interfaces:** `StreamingSql { engine: RisingWave, statements }`: a durable workflow applying the statements in order on the namespace's RisingWave database (Q96 ruling: shared cluster, one database per namespace, dedicated cluster above a size threshold this task measures and records), idempotent by statement hash per step. Arroyo is documented only (Q95 ruling).

Tests: `statements_applied_once_across_replays`, `ported_flink_sql_example_matches_flink_output`, `namespace_database_isolated`.

Commit `feat(engines): streaming sql on risingwave`.

### Task 54: The Flink operator lifecycle

**Files:** `crates/loams-jobs-engines/src/flink.rs`, `deploy/engines/flink-operator/`, `scripts/jb1/engines-kind.sh`. Tests `tests/flink.rs` (kind, nightly).

**Interfaces:** `FlinkDeployment` (application mode, `upgradeMode: savepoint`) with checkpoints and savepoints on RustFS; actions deploy, savepoint (`FlinkStateSnapshot`), upgrade (patch the spec), rollback (`kubernetes.operator.deployment.rollback.enabled: true`, or a recorded spec from a named savepoint), suspend, resume, cancel; `status.lifecycleState` and `status.jobStatus` mapped to run events. Loams never checkpoints inside Flink.

Tests: `stateful_job_keeps_state_across_upgrade`, `rollback_restores_last_stable_spec`, `savepoint_path_recorded`, `suspend_resume_from_savepoint`, `reads_loams_via_kafka_gateway` (gate M5).

Commit `feat(engines): flink operator lifecycle`.

### Task 55: Engine docs and the J3/J4 gates

**Files:** `docs/guides/jobs/{spark.md,flink-porting.md}`, `jb1-engines.yml`.

**Interfaces:** the porting table with a worked example for each Flink SQL construct in Flink's examples; the run-on-both migration advice; the J3 and J4 exit gates of §26 §15 as CI jobs.

Tests: J3 gate (`pyspark_examples_pass_on_sail`, `sail_unsupported_job_runs_on_fallback_unchanged`, `iceberg_written_by_sail_reads_in_loams`) and J4 gate (`ported_flink_sql_example_matches_flink_output`, `stateful_job_keeps_state_across_upgrade`, `reads_loams_via_kafka_gateway`) green in `jb1-engines.yml`.

Commit `docs(engines): spark and flink guides and gates`.

---

## Exit criteria for production (with the owning tasks)

- [ ] **API** served, linted and breaking-checked, every mutation idempotent, reasons registered: Tasks 1, 4, 12, 47.
- [ ] **Semantics:** the model test and the nemesis gate clean (no lost job, one outcome, one live lease, schedules one job per tick): Tasks 5, 11, 15, 44.
- [ ] **Stores:** the conformance macro green on embedded and TiKV, the fault matrix nightly: Tasks 4–11, 17.
- [ ] **Events:** relay, `watch` and cursors with no gap and no duplicate: Tasks 13, 14.
- [ ] **Durable:** schedules, flows, queue mode promise-free: Tasks 15, 16.
- [ ] **Throughput:** numbers published and accepted by the owner: Task 18.
- [ ] **J1 Celery:** the integration suite green, beat twice without duplicates, durable mode resumes: Tasks 19–25.
- [ ] **J2 BullMQ:** the backend-neutral suite green at the pin, exclusions justified: Tasks 26–32.
- [ ] **Usage hooks v1:** contract page and crate, registry and cardinality guard, engine, durable and jobs families, cgroup and pod labels, tenant header and Envoy sink, conformance kit: Tasks 33–40.
- [ ] **Security:** auth per queue and action, threat model closed for high and critical: Tasks 41, 46.
- [ ] **Quotas** every limit refused in a test; **erasure** complete: Tasks 42, 43.
- [ ] **Packaging and docs:** variants, packages, guides, runbooks verified: Tasks 45, 47.
- [ ] **J3 and J4** (when their gates hold): Tasks 48–55.

## Open questions

None needs the owner. Every question this plan carried (Q95, Q96, Q98, Q99, Q101–Q104, JB1-Q1 to JB1-Q7, Q-UH-1 to Q-UH-3) is settled in "Owner rulings 2026-10-10 (defaults)" near the top. A question that a later task finds involves money, a licence or an external account is added here, not ruled by an agent.

## Self-review

- **Spec coverage.**

  | Section | Task(s) |
  |---|---|
  | §26 §3 Architecture, state | 3, 4, 8, 12, 17 |
  | §26 §5 Rust API, errors, admin, protobuf | 1, 2, 10, 12 |
  | §26 §6.1–§6.5 Delivery, idempotency, leases, states, priorities | 4, 5, 6, 7, 11 |
  | §26 §6.6 Event log and `watch` | 13, 14 |
  | §26 §6.7 Payloads and results | 8, 9 |
  | §26 §6.8 Stores | 4, 12, 17 (W1) |
  | §26 §6.9 Rate limits and concurrency | 5, 18, 42 |
  | §26 §6.10 DLQs and retention | 5, 7 |
  | §26 §6.11 Observability | 37 |
  | §26 §7.1 Celery | 20–25 |
  | §26 §7.2 BullMQ | 26–32 |
  | §26 §8 Resonate | 15, 16, 30 |
  | §26 §9 PySpark on Sail | 49–52 |
  | §26 §10 Flink | 53, 54 |
  | §26 §11 Tenancy and security | 12, 41, 43, 46 |
  | §26 §12 Licences and packages | 45 |
  | §26 §14 Testing | 11, 25, 31, 44, 55 |
  | §27 §3.1 Metrics | 33, 34, 35, 36, 37 |
  | §27 §3.2 Cgroups and labels | 38 |
  | §27 §3.4 Envoy and the tenant header | 39 |
  | §27 §3.5 Quota enforcement | 42 |
  | §27 §3.7 `InvocationObserver` | RN1 (consumes Tasks 33, 40) |
  | §27 §6 Open questions | 33, 34 (Q-UH-1, Q-UH-2); Q-UH-3 deferred |

- **Types.** `Jobs`, `JobStore`, `LeaseToken`, `JobsError`, the key layout, the event record, the hooks contract and `EngineRunner` are defined once, in the shared contracts.
- **Review Focus.** Items 1–12 each name an owning test (Tasks 1, 5, 9, 11, 12, 14, 15, 17, 21, 22, 24, 34, 37, 39, 41, 44, 46, 48, 50).
- **Not built here, by design.** The per-invocation record and reporter (`loams-platform`), `InvocationObserver` (RN1), the supervisor (F1), `KnativeRunner` (MT2), `loams-gateway` and `loams-dapr`.
- **Decisions the owner must make:** none; every question is settled in "Owner rulings 2026-10-10 (defaults)".

## Rulings made during execution

(Task 0 and later tasks append here.)
