# API2 — The Unified Connect API and the SDKs in Production Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Each task lists the interfaces it must produce and the tests that must exist and pass before it is done. Where this plan gives exact names, paths, headers or defaults, use them verbatim. Where it gives a contract and named tests, write the code to that contract, and record any deviation in "Rulings made during execution" at the end of this file.
>
> **Status: Planned** (2026-10-10). Track API2, design [§44](../design/44-unified-api-and-sdks.md) (D600–D619, Q600–Q614). API2 **finishes and supersedes** the three 2026-10-02 plans, which stay as the record of what they delivered:
> - [API1](2026-10-02-api1-unified-connect.md): Tasks 0–4 are on `dev` (route map, options, one port, `loams.collection.v1` with Namespace, Collection, Document and Query services). Its Tasks 5–10 are API2 Tasks 1–12.
> - [SDK1](2026-10-02-sdk1-generation-pipeline.md): its TypeScript, Python, Go and Rust slices of Tasks 1–4 landed through SDK2 (generator, runtime contract R1–R10, 33-fixture corpus, drift and pin scripts). Its Tasks 0, 5–8 and the rest of 1–4 are API2 Tasks 13–18 and 24–26.
> - [SDK2](2026-10-02-sdk2-languages.md): all 13 languages have a runtime under `sdks/<lang>/` that passes the 28 required fixtures, but the corpus covers only `loams.instance.v1`, `loams.live.v1` and the AP0 mock packages. Making each language cover the whole API and ship is API2 Tasks 19–23.
>
> When a task of this plan lands, the matching row of API1, SDK1 or SDK2 gets a one-line pointer to it (Task 28). Do not edit those plans' rulings.

## Owner rulings 2026-10-10 (defaults)

The owner asked for the best default on every open question ("do the best for others"). These are binding for execution; the tasks below already follow them. Task 0 records them as rulings and Task 28 copies them into the decision log. Only the items under "Open questions" at the end still need a human.

| # | Question | Decision | Reason |
|---|---|---|---|
| Q600 | Legacy REST shims or delete outright | **Delete outright.** No `--legacy-rest`, no `legacy.rs`, no `Deprecation`/`Sunset` headers; removed paths 404 like any unknown path, and `docs/api/migrate.md` is the only migration aid | Pre-1.0 with no external users; a shim is code written only to be deleted |
| Q601 | Confirm 13 languages | Keep all 13; Objective-C is the first to drop if upkeep is too heavy (Q611) | Each already passes the 28 fixtures; dropping later is cheap, adding back is not |
| Q602 | OAuth/OIDC/well-known/health stay HTTP | Yes (D602) | They are protocol endpoints that standard clients expect at fixed HTTP paths |
| Q603 | Reflection scope | On in `loams dev`; elsewhere off unless `--reflection`, which requires auth | Developer convenience without exposing the schema on production ports |
| Q604 | Generator versus hand-written facades | **Generate** TypeScript, Python, Go, Rust, Swift, Kotlin, Java and C#; **hand-write** Dart, Ruby, PHP, C++ and Objective-C, each checked by `<lang>_facade_matches_index` | Generation where users are most numerous; the index test keeps the hand-written five from drifting |
| Q605 | Publish to the Buf Schema Registry | No; local plugins plus remote-plugin pins only | No extra account or dependency for a pre-1.0 API |
| Q606 | Ruby/PHP Connect-unary fallback counts as "no REST" | Yes | Same protos and messages; it is the Connect protocol, not a bespoke REST |
| Q607 | `loams.dev/go` vanity path | Yes; `sdks/go/go.mod` already uses it, and the `go-import` meta tag is a follow-up issue in `loams-cloud` | Stable import path independent of the repository host |
| Q608 | `QueryArrow` IPC with a cap | Yes, 16 MiB, `result_too_large` above it pointing to Flight | Arrow for small results without making the unary path a bulk channel |
| Q609 | Licence of generated code; DCO on mirrors | Generated code is Apache-2.0, the repository's licence (`LICENSE`, `Cargo.toml`); mirrors are bot-written, so no DCO there | Same licence as the source it is generated from; DCO certifies human contributions |
| Q610 | C++: own vcpkg/Conan first or source only | Ship a vcpkg registry (a git repository) and a Conan recipe in-repo; publishing a hosted Conan remote stays behind `SDK_PUBLISH_CONAN` | Users get a package manager path; the hosting is the only account step |
| Q611 | Objective-C: keep or drop | Keep generated sources plus SwiftPM; marked "community" if it fails conformance | Cheap to keep while it passes; honest label if it does not |
| Q612 | CODEOWNERS on `reasons.md` | Yes, plus `proto/**` | Reasons and protos are the public contract |
| Q613 | `loams.vector` module or view | Both names, one RPC (`vector.search` and `search.query`) | Discoverability for vector users without a second RPC |
| Q614 | Scroll: stream or unary with cursor | Whatever Task 0 finds built (expected unary with a cursor) | The built shape already works on every transport, including Connect-unary |
| API2-Q1 | Auth before MT1 | **Ship the main port with `ApiKeyAuthenticator` and `DevLoopbackAuthenticator` (loopback-only); REST removal does not wait for MT1.** `Authenticator` is the hook: MT1 adds `JwtAuthenticator` and the HTTP protocol endpoints without touching handlers | Unblocks API2b; the trait keeps MT1 a drop-in |
| API2-Q2 | Wave 2/3 CI jobs as required checks | **Nightly and on tags, never in `required.needs` in this plan.** Making one PR-blocking is a later, separate ruling per language | Keeps PR CI fast; regressions still surface within a day |
| API2-Q3 | `loams.durable.v1` scope | **No `loams.durable.v1` now. The Resonate HTTP protocol is the only durable API.** Task 8 ships `OperationsService` only | No second control surface to keep in step with Resonate; revisit when a Loams-specific durable need appears |
| API2-Q4 | Idempotency window | 24 h cluster-wide, as PG2's ledger; only keyed calls pay the metastore write, and expired entries are swept by the existing metastore TTL | Covers every realistic client retry window, matches the other ledger |
| API2-Q5 | Compat gRPC auth on the conventional ports | Unchanged until MT1; only the main-port mount goes through `Authenticator`. Recorded as an accepted pre-MT1 risk in Task 27's threat model | D603: compat ports keep their wire and auth behaviour; the main port is the new surface |

**Goal:** one public API and thirteen shippable SDKs.
- Every application call is a Connect/gRPC RPC in `loams.<service>.v1` on the `loams` main port. The native REST under `/v1/namespaces/...`, the console OpenAPI `/api/v1` and `/internal/*` are gone, deleted outright with no shims (Q600).
- Gone too: `api/console/openapi.json`, `openapi-fetch` and `openapi-typescript`.
- The compatibility surfaces of §44 §6 are unchanged and green.
- Every stable package carries the facade annotations, and the SDKs expose it as `loams.<module>.<call>`.
- Every language passes the required fixtures of a corpus that covers every stable package and every runtime-contract clause (R1–R10), against `loams dev` and the fault mock.
- Each language publishes from CI (dry run until the owner's registry actions, §44 §13), with reference docs generated from the protos.

The exit is "Exit criteria for production" at the end of this plan, with the owning tasks.

**Architecture** (§44 §4, §7, §10):
- **One router.** `crates/loams/src/api/connect.rs` owns the catalogue (`CATALOGUE`), `GetInstance.services[]`, health and reflection. Each package's handlers live in their own `api/connect_<package>.rs` and call the same service traits the REST handlers call today. The REST handlers' logic moves; it is not rewritten.
- **Auth is a layer, not a handler concern.** One `Authenticator` on the main router (Task 6) turns `Authorization: Bearer` into a `Principal` in the request extensions. Handlers read the principal; none parses a header.
- **Compat gRPC on the main port** (D601) is routed by gRPC service-name prefix to the existing tonic servers. connect-rust does not serve them.
- **`loams.internal.v1`** is a second buf module (`proto-internal/`), generated into `loams-proto` behind a non-default feature and served only by `internal_router`.
- **The facade has one source.** The `loams.options.v1` annotations drive `protoc-gen-loams-facade`, which renders TypeScript, Python, Go and Rust. It also emits `sdks/fixtures/facade-index.json`, the module/call/retry-class/pagination index. A language with a hand-written facade (D730's fallback, Q604) is checked against that index by a test, so no SDK can drift from the protos silently.
- **The fixture corpus is the specification.** It is recorded from `loams dev`, plus `loams-apps-mock` and the fault server for what a real server cannot produce on demand. `sdks/fixtures/manifest.json` decides what is required.

**Tech Stack:**
- Rust 1.97 (workspace `rust-version`), edition 2024, workspace lints.
- `connectrpc` 0.9.1, `connectrpc-build` 0.9, `buffa` 0.9.2, `connectrpc-health`/`-reflection` 0.9.0 (as `docs/api/route-map.md` records); `tonic` 0.14 for the compat servers only; `arrow-flight` 58.4; `axum` 0.8.
- `buf` 1.73.0 and the remote plugins pinned in `scripts/sdk/pins.lock`.
- Per language, the versions in each `sdks/<lang>/DEPENDENCIES.md` or lockfile. Task 0 records the ones that have neither.
- Test tools: `sdks/conformance/run.sh` (recorded corpus plus live `loams dev`), `loams-apps-mock`, `connectrpc/conformance` (Task 23), `actionlint`.

**Spec:**
- [§44](../design/44-unified-api-and-sdks.md) (all), and D600–D619, Q600–Q614 in the [decision log](../design/13-decision-log.md).
- [§05](../design/05-query-engine.md) §4 (hybrid IR), §5 (consistency), §8 (Flight).
- [§19](../design/19-console-identity-and-agents.md) §5 (tokens, sessions), P9 (console API).
- [§30](../design/30-loams-cli.md) §9 (variants).
- [§37](../design/37-desktop-and-mobile-apps.md) §8 (apps on the same protos).
- [MT1](2026-10-02-mt1-authentik-identity.md) (identity; Task 7 TLS on the main port), [LV1](2026-10-08-lv1-live-production.md) (Live on the main port), [GR1](2026-10-08-gr1-graph-production.md) Task 5, [PG2](2026-10-08-pg2-postgres-production.md) Tasks 1, 9 and 57 (package rules and served-only reflection, R1.3, R1.4).
- `docs/api/route-map.md`, `docs/api/reasons.md`, `docs/sdk/runtime-contract.md`, `docs/sdk/fixtures.md`, `docs/release/publishing.md`; issue #254.

## Global Constraints

- **Worktree and branches.** Work in `~/Documents/Ostriumlabs/loams-wt/api2-unified-api-sdks`. Use one branch per milestone, `feat/api2a-surface`, `feat/api2b-rest-removal`, `feat/api2c-pipeline`, `feat/api2d-languages` and `feat/api2e-release`, each based on `dev`, with stacked PRs targeting `dev`. A language task in API2d may take its own branch, `feat/api2d-<lang>`. Use `git commit -s` (DCO). Commit areas: `proto`, `api`, `loams`, `console`, `desktop`, `sdk-<lang>`, `sdks`, `facade`, `ci`, `docs`.
- **Rust builds** use the shared target directory (`~/Documents/.cargo/config.toml`). Never set `CARGO_TARGET_DIR` and never build in `/tmp`. Run one cargo build at a time (jobs and linker from `~/.cargo/config.toml`). Build the touched crates (`cargo test -p loams --test connect_sql`), not the workspace. `sdks/rust` is its own workspace: test it there (`cd sdks/rust && cargo test`). `scripts/sdk/gen.sh` builds `loams-facade-gen` at `LOAMS_BUILD_JOBS` (default 4); do not run it next to another cargo build.
- **Compatibility surfaces are not touched** (D603). The Qdrant REST and gRPC, Elasticsearch, Postgres wire, Kafka gateway, Flight SQL, Resonate, MySQL, Git Smart HTTP and CloudEvents suites stay green on every PR (`qdrant-client`, `es-client` and the rest of `ci.yml`). A PR that changes their wire output is wrong.
- **Behaviour first, deletion last.** A REST route or OpenAPI path is deleted only in Task 11, and only after its RPC passes the same tests, ported by name with an `_rpc` suffix (API1's rule).
- **AP0 API rules** (§44 §8):
  - Every mutation that is not idempotent by construction has `idempotency_key` (API1 ruling 2.4 stands: create-by-name and set-state RPCs do not).
  - Reads are `NO_SIDE_EFFECTS`.
  - Errors are `loams.errors.v1.ErrorInfo` with a reason registered in `docs/api/reasons.md`. A reason is never renamed or removed within a major version.
  - Pagination is `page_size`/`page_token`/`next_page_token`.
  - Streams are server-streaming only (D420), with a cursor and a 15 s heartbeat.
  - Resource names are message fields, never URL paths.
- **`buf lint` (STANDARD) is clean on every package.** `buf breaking` (FILE) runs against `dev` until the first release tag and against the tag after it (Task 12). Packages with `ModuleOptions.unstable` are excluded through `buf.yaml`'s `breaking.ignore`, and the option and the ignore line must agree (Task 12 test).
- **Generated code is never hand-edited.** `scripts/sdk/drift.sh` fails on a diff in any committed output. A pin bump is its own PR with the regenerated output and its `pins.lock` row.
- **One required-fixture bar** (D617). `sdks/fixtures/manifest.json` is the only definition. A language skips a required fixture only when the fixture is `transport: grpc-only` and the language runs the Connect-unary fallback (D613).
- **Secrets.** Bearer tokens, API keys and refresh tokens never appear in a URL, a log line, an error message, an `ErrorInfo.metadata` value, a span attribute or a fixture. SDK `Debug`/`toString`/`repr` of a `TokenSource` prints `[redacted]`. Recorded fixtures carry the placeholder `Bearer <redacted>`.
- **No billing** (D220, D552). No RPC, field or SDK module named `meter`, `usage`, `invoice`, `credit`, `plan` or `price`. `scripts/ci/no-metering.sh` gains `proto/` and `sdks/` (Task 12).
- **Pins.** Exact versions for every new crate, package and plugin, at least 14 days old (registry date). Record each pin in the task's commit message and, for the generation toolchain, in `scripts/sdk/pins.lock`.
- **Do not publish** to any registry until the owner has done §44 §13's account actions for it. Until then every release job is a dry run (Task 25).

## Review Focus

1. **A retried mutation applies twice.** Paths: a keyed `WriteDocuments` retried on another gateway node; a `Produce` without a producer key auto-retried by an SDK; an SDK that mints a fresh key per attempt. Expected: never. Tests: Task 4 `keyed_write_replays_across_nodes`; Task 16 `produce_without_key_never_retried`; Task 20 `<lang>_retry_reuses_idempotency_key` for every language (Tasks 20–22).
2. **A token leaks** into a URL, a log, an error, a fixture or an SDK's debug output. Expected: never. Tests: Task 6 `bearer_never_logged`, `query_string_token_refused`; Task 15 `fixtures_carry_no_credentials`; Task 27 `sdk_token_source_debug_redacted` (every language).
3. **An unauthenticated call reaches data.** Paths: a Connect, gRPC or gRPC-Web call with no bearer on the main port; compat gRPC on the main port; `loams.internal.v1` on the public listener. Expected: refused with `unauthenticated` or not routed at all. Tests: Task 6 `every_served_rpc_requires_auth_except_allowlist`; Task 5 `compat_grpc_on_main_port_requires_auth`; Task 4 `internal_service_not_on_public_port`.
4. **A REST route survives Task 11.** Expected: no; a removed path is a 404 like any unknown path (Q600: no shims). Tests: Task 11 `no_native_rest_route_remains`, `removed_rest_paths_are_404`.
5. **An SDK's surface drifts from the protos**: a call with no annotation, a wrong retry class, or a missing module. Expected: CI fails. Tests: Task 14 `<lang>_facade_matches_index` (every hand-written facade); Task 13 `every_stable_service_has_module`, `every_stable_rpc_has_facade_call`.
6. **A stale or wrong consistency token is used silently.** Paths: the session store merges tokens from two namespaces; a token from another instance is accepted. Expected: an error, never a stale read. Tests: Task 16 `token_merge_is_per_stream_partition_max`, `foreign_instance_token_refused`.
7. **A compat surface's wire output changes.** Expected: never. Tests: the existing compat suites, plus Task 5 `qdrant_grpc_bytes_unchanged_on_main_port`.
8. **Reflection lists an unserved service, or is on in production.** Expected: no. Tests: Task 9 `reflection_lists_only_served_services`, `reflection_off_by_default_outside_dev`.
9. **A breaking change ships in a stable package.** Expected: CI fails. Tests: Task 12 `breaking_change_fails_ci`, `unstable_flag_and_buf_ignore_agree`.

---

## File structure

```
proto/loams/sql/v1/sql.proto                                      Task 1
proto/loams/stream/v1/stream.proto                                Task 2 (moved from crates/loams-stream-grpc/proto)
proto/loams/link/v1/link.proto                                    Task 3
proto-internal/loams/internal/v1/internal.proto, buf.yaml         Task 4 (second buf module, D607)
proto/loams/auth/v1/auth.proto                                    Task 6
proto/loams/admin/v1/{org,project,agent,key,audit}.proto          Task 7
proto/loams/errors/v1/errors.proto                                Task 16 (RetryInfo use, request id)
crates/loams-proto/{build.rs,src/lib.rs}                          Tasks 1–8 (package list; `internal` feature)
crates/loams-stream-grpc/                                         Task 2 (tonic service kept as the CloudEvents gRPC binding only)
crates/loams/src/api/
  connect.rs                                                      Tasks 1–9 (CATALOGUE rows, registration, reflection)
  connect_sql.rs connect_streams.rs connect_links.rs              Tasks 1–3
  connect_internal.rs                                             Task 4
  connect_compat.rs                                               Task 5 (service-name routing to tonic)
  auth.rs connect_auth.rs connect_admin.rs                        Tasks 6–7
  connect_operations.rs                                           Task 8 (no connect_durable.rs: API2-Q3)
  idempotency_store.rs                                            Task 4 (cluster ledger; replaces connect_idempotency.rs's map)
  mod.rs collections.rs query.rs sql.rs streams.rs events.rs hot.rs internal.rs   Task 11 (REST deleted)
crates/loams/tests/connect_{sql,streams,links,internal,compat,auth,admin,operations}.rs
crates/loams/tests/{route_map.rs,curl_examples.rs}
crates/loams-console-mock/                                        Task 7 (Connect, not OpenAPI)
crates/loams-apps-mock/                                           Task 15 (fault endpoints on Connect)
crates/loams-facade-gen/src/{index.rs,lib.rs}                     Tasks 13–14
api/console/openapi.json                                          Task 11 (deleted)
web/apps/console/src/api/                                         Task 10 (Connect clients)
web/plugins/data-studio/src/client.ts                             Task 10
web/packages/proto/                                               Tasks 1–8 (regenerated @loams/proto)
apps/desktop-electron/src/main/protocol/local-shim.ts            Task 10
sdks/fixtures/{manifest.json,index.json,faults.json,facade-index.json,recorded/}   Tasks 13–15
sdks/conformance/                                                 Tasks 15, 23
sdks/templates/<lang>/template.env                                Task 14 (all 13)
sdks/<lang>/                                                      Tasks 17–22, 24
docs/api/{reasons.md,route-map.md,migrate.md,curl.md,reference/,snippets/}   Tasks 1–12, 26
docs/sdk/{runtime-contract.md,fixtures.md,toolchain-2026-10.md,versioning.md}   Tasks 0, 16, 24
docs/release/publishing.md                                        Task 25
scripts/sdk/{gen.sh,drift.sh,check-pins.sh,pins.lock,release-dry-run.sh}       Tasks 13–14, 25
scripts/ci/no-metering.sh                                         Task 12
.github/workflows/{ci.yml,sdk-release.yml,sdk-mirror.yml,api-protos.yml}        Tasks 12, 19, 23, 25
.github/CODEOWNERS                                                Task 12 (reasons.md, Q612)
```

## Shared contracts (all tasks use these names)

### The package catalogue (`CATALOGUE` in `crates/loams/src/api/connect.rs`)

`GetInstance.services[]` reports this table, with `available` decided per build and per runtime. A package with no proto has no row (the existing rule).

| Package | Services | Module(s) (`ModuleOptions.name`) | `unstable` | Served by | Task |
|---|---|---|---|---|---|
| `loams.instance.v1` | `InstanceService` | `instance` | no | always | exists |
| `loams.collection.v1` | `NamespaceService`, `CollectionService`, `DocumentService`, `QueryService` | `collections`, `documents`, `search` + `vector` | no | always | exists; facade calls Task 13 |
| `loams.sql.v1` | `SqlService` | `sql` | no | always | 1 |
| `loams.stream.v1` | `StreamService` | `streams` | no | always | 2 |
| `loams.link.v1` | `LinkService` | `links` | no | always | 3 |
| `loams.auth.v1` | `AuthService` | `auth` | no | always | 6 |
| `loams.admin.v1` | `OrgService`, `ProjectService`, `AgentService`, `KeyService`, `AuditService` | `admin.org`, `admin.projects`, `admin.agents`, `admin.keys`, `admin.audit` | no | always | 7 |
| `loams.operations.v1` | `OperationsService` | `operations` | no | always | 8 |
| `loams.live.v1` | `LiveService` | `live`, `tables` | yes | feature `live` (LV1) | 9 (wiring only) |
| `loams.graph.v1` | `GraphAdminService`, `GraphService` | `graph` | yes (GR1e) | feature `graph` | exists |
| `loams.postgres.v1` | `PostgresService` | `postgres` | yes (PG2 GA) | feature `postgres` (PG2 Task 9) | 9 (wiring only) |
| `loams.approvals.v1`, `loams.devices.v1`, `loams.notifications.v1` | AP0 | `approvals`, `devices`, `notifications` | no | `loams-apps-mock`; the server when AP1 serves them | no change |
| `loams.internal.v1` | `InternalService` | none (no SDK) | n/a | `internal_router` only | 4 |

Not in this table, and never in a public proto or SDK: the meter protocol, `loams.durable.v1` (API2-Q3: Resonate is the durable API), `loams.live.worker.v1` (LV1, internal), connector admin.

### Proto rules (Tasks 1–8 write the protos; this is the contract)

```proto
// Every new service:
service SqlService {
  option (loams.options.v1.module) = { name: "sql" summary: "SQL over collections and streams." };
  rpc Query(QueryRequest) returns (QueryResponse) {
    option idempotency_level = NO_SIDE_EFFECTS;      // a read: retry-safe, Connect GET allowed
    option (loams.options.v1.facade) = { name: "query" };
  }
  rpc QueryArrow(QueryArrowRequest) returns (QueryArrowResponse) {   // Q608; Task 1
    option idempotency_level = NO_SIDE_EFFECTS;
    option (loams.options.v1.facade) = { name: "query_arrow" };
  }
}
// A mutation that is not idempotent by construction: `string idempotency_key = 15;`
//   (field 15 in every package from API2 on; DocumentService's existing field 5 stays).
// A list RPC: `int32 page_size = 1; string page_token = 2;` in, `string next_page_token` out,
//   and `facade = { pagination: "<items>:next_page_token" }`.
// A read that takes consistency: `loams.collection.v1.Consistency consistency = <n>;`
// A write that produces state: `string consistency_token = <n>;` in the response, and the
//   same value in the `loams-consistency-token` response header.
```

### Headers (exact names)

| Header | Direction | Meaning |
|---|---|---|
| `authorization: Bearer <token>` | request | the only credential carrier (D608); a token in the query string is refused (Task 6) |
| `loams-consistency-token` | both | the write's token (response); the read's `at_least` (request, when the message field is empty) |
| `loams-request-id` | response | a ULID per call, also in `ErrorInfo.metadata["request_id"]` (Task 16) |
| `Loams-Hot` | request | the existing hot-tier hint (unchanged) |
| `loams-test-fault`, `loams-test-fault-after` | request | fault injection, **fixture server and `loams-apps-mock` only**; the real server ignores them (Task 15 test) |

### Rust seams (`crates/loams`)

```rust
/// Main-port authentication (Task 6). One implementation per deployment kind.
#[async_trait]
pub trait Authenticator: Send + Sync + 'static {
    /// `Ok(None)` only for an RPC on the unauthenticated allowlist.
    async fn authenticate(&self, rpc: &RpcPath, headers: &HeaderMap) -> Result<Option<Principal>, AuthError>;
}
pub struct Principal { pub subject: String, pub org: String, pub kind: PrincipalKind, pub scopes: Vec<String> }
pub enum PrincipalKind { User, Agent, ServiceAccount, ApiKey, DevLoopback } // API2-Q1: ApiKey and DevLoopback ship first; User/Agent/ServiceAccount arrive with MT1's JwtAuthenticator
pub enum AuthError { Missing, Malformed, Expired, Revoked, Unknown } // -> unauthenticated + reason

/// The unauthenticated allowlist (Task 6), the only RPCs answered without a principal.
pub const PUBLIC_RPCS: &[&str] = &[
    "/loams.instance.v1.InstanceService/GetInstance",
    "/loams.auth.v1.AuthService/ListProviders",
    "/loams.auth.v1.AuthService/CompleteSetup",   // first-run only; loopback or setup token
    "/grpc.health.v1.Health/Check",
    "/grpc.health.v1.Health/Watch",
];

/// The cluster-wide idempotency ledger (Task 4). Replaces the per-process map.
#[async_trait]
pub trait IdempotencyStore: Send + Sync + 'static {
    /// Takes the key's gate; returns the stored answer if the same fingerprint already completed.
    async fn begin(&self, scope: &IdemScope, key: &str, fingerprint: [u8; 32]) -> Result<IdemBegin, IdemError>;
    async fn complete(&self, ticket: IdemTicket, answer: Bytes) -> Result<(), IdemError>;
}
pub enum IdemBegin { Fresh(IdemTicket), Replay(Bytes), FingerprintMismatch }
// Window: 24 h. Backed by the metastore (`loams-meta`; TiKV when `--meta tikv://`).
```

### The facade index (`sdks/fixtures/facade-index.json`, Task 13)

Generated by `protoc-gen-loams-facade lang=index`, committed, and drift-checked. One entry per facade call:

```json
{ "protoRev": "<git sha of proto/>",
  "calls": [ { "module": "vector", "call": "search", "rpc": "loams.collection.v1.QueryService/Search",
               "retry": "safe|keyed|never", "idempotencyKeyField": "", "pagination": null,
               "stream": false, "unstable": false } ] }
```

`retry` follows D610: `safe` for `NO_SIDE_EFFECTS`/`IDEMPOTENT` or `retry_safe: true`; `keyed` when the request has `idempotency_key`; `never` otherwise. A hand-written facade's test (Task 14) loads this file and asserts its module/call set and retry classes are equal to it.

### The runtime contract

`docs/sdk/runtime-contract.md` clauses R1–R10 are the contract. API2 changes only what Task 16 says: R2 gains `RetryInfo`, R4 gains the token encoding and the session store, and R8 gains `request_id`. A clause number is never reused.

---

## Execution order

1. Task 0.
2. **API2a** (Tasks 1–9). Tasks 1–5 and 8 are independent of each other. Task 6 does not wait for MT1 (API2-Q1: API keys and dev-loopback first). Task 7 depends on Task 6. Task 9 runs last, after LV1's main-port task and PG2 Task 9 if they have merged, and again when either merges.
3. **API2b** (Tasks 10–12) after Tasks 6 and 7, whether or not MT1 has merged (API2-Q1). Task 11 after Task 10.
4. **API2c** (Tasks 13–18) can start at once, beside API2a. Task 13 needs each package's proto, so it lands per package. Task 15's recordings follow the packages they record.
5. **API2d** (Tasks 19–23). Task 19 at once. Tasks 20–22 after Tasks 13–16. Task 23 after Task 9.
6. **API2e** (Tasks 24–28) after API2d's wave 1 (Task 20). Task 28 last.

---

### Task 0: Reconcile with the code as built

**Files:** this plan's "Rulings made during execution" only.

Steps:
1. Answer each of the following and record the answer, with file paths and the `dev` commit, as a ruling:
   - Is this plan's "as built" still true? Check API1 Task 4's status: `QueryService/Search` in `crates/loams/src/api/connect_query.rs`, and whether `ScrollDocuments` is unary-with-cursor. If so, record it as the answer to Q614 unless the owner has ruled otherwise.
   - Which REST routes exist on `dev` today (`crates/loams/tests/route_map.rs`'s inventory)? `docs/api/route-map.md` has 27 `/v1` pairs plus `list_streams` and `list_links`, which §44 §5.1 omits. Tasks 2 and 3 add `ListStreams` and `ListLinks` for them.
   - Has MT1 merged anything? Look for a token endpoint (`/oauth/token`), sessions, a principal store and main-port TLS (MT1 Task 7). Either way (API2-Q1), Task 6 ships `ApiKeyAuthenticator` (keys in the metastore, hashed with argon2id) and `DevLoopbackAuthenticator` (`loams dev` on loopback only); `JwtAuthenticator` and the HTTP protocol endpoints are added only if MT1's issuer has merged, otherwise MT1 adds them through the `Authenticator` hook.
   - Has LV1's "Live on the main port" task merged (`LiveAbsent` replaced)? Has PG2 Task 9 merged (`PostgresService` registered, `Reflector::with_services`)? Record which wiring Task 9 still owns.
   - Which durable control surface exists in `crates/loams-durable`? Record it for reference only: API2-Q3 rules that the Resonate HTTP protocol stays the only durable API, so Task 8 adds no durable RPC.
   - Where is the MCP server (§44 §6 lists it on 8083)? On 2026-10-10 no crate under `crates/` serves MCP. Record that MCP stays out of scope here (M1.6).
   - Which SDKs have a generated facade and which a hand-written one? On 2026-10-10: TypeScript, Python and Rust are generated. Go generates `gen/facade` and hand-writes `modules.go` (D730). The other nine are hand-written. Record the list, and record which `sdks/<lang>` have a CI job: TypeScript, Rust, Python, Go, C++, C# and Swift do; Java, Kotlin, Ruby, PHP, Dart and Objective-C do not.
   - Naming drift. `sdks/README.md` says the Rust crate is `loams-sdk` and the Go module is `github.com/ostrium-labs/loams/sdks/go`. `sdks/rust/Cargo.toml` says `loams` and `sdks/go/go.mod` says `loams.dev/go`. Record that the manifests win, and add the README fix to Task 28.
   - Which `sdks/fixtures/manifest.json` notes are stale? For example, R4's "No RPC carries a `consistency_token`" was true before API1 Task 3 and is not now. List them for Task 15.
   - Q600–Q614 and API2-Q1–Q5: record the "Owner rulings 2026-10-10 (defaults)" table as rulings, plus any later owner answer that overrides one.
2. Write `docs/sdk/toolchain-2026-10.md` (SDK1 Task 0): one row per language with the Connect or gRPC library, its latest release and date, the remote plugin and its version, the registry, and whether it has trusted publishing. Apply D612's 12-month rule and note any language that changes column. Test: `toolchain_table_has_row_per_language` (a node test in `sdks/conformance/check-languages.mjs`).
3. Commit `docs(api2): task 0 rulings and the toolchain table`.

## API2a — The API surface, complete (Tasks 1–9)

### Task 1: `loams.sql.v1`

**Files:** create `proto/loams/sql/v1/sql.proto`, `crates/loams/src/api/connect_sql.rs` and `crates/loams/tests/connect_sql.rs`. Modify `crates/loams-proto/build.rs` (package list), `connect.rs` (catalogue row) and `docs/api/{reasons.md,route-map.md}`.

**Interfaces:**
- `SqlService/Query`: `namespace`, `sql`, `params` (`repeated google.protobuf.Value`), `consistency`, `max_rows` (default 10 000, cap 100 000). It answers `columns[]` (name, Arrow type string), `rows[]` (`google.protobuf.ListValue`), `truncated` and `consistency_token`. It calls the same function `api/sql.rs::sql` calls.
- `SqlService/QueryArrow` (Q608: yes). It answers `bytes arrow_ipc` (an Arrow IPC stream), with a cap of `--sql-arrow-max-bytes` (default 16 MiB). Over the cap it refuses with `resource_exhausted` and reason `result_too_large`, and `metadata.use = "flight"`.
- Flight SQL is unchanged.

Tests:
- Each test of `it/` that covers `POST /v1/namespaces/{ns}/sql`, ported with an `_rpc` suffix.
- `query_rows_match_rest`: the same SQL gives equal rows through REST and RPC while both exist.
- `query_arrow_roundtrips`: the IPC bytes decode with `arrow-ipc` to the same batches.
- `query_arrow_over_cap_refused`
- `query_is_get_cacheable`: a Connect `GET` with `encoding=json` works (`NO_SIDE_EFFECTS`).
- `flight_sql_still_serves`: the existing Flight test, unchanged, still passes.

Steps: tests (FAIL: package missing) → proto → generate (`@loams/proto` too) → handler → PASS → route-map rows → commit `feat(api): loams.sql.v1`.

### Task 2: `loams.stream.v1` on connect-rust

**Files:** move `crates/loams-stream-grpc/proto/loams/stream/v1/stream.proto` to `proto/loams/stream/v1/stream.proto`, keeping `Produce` and `ProduceCloudEvents` wire-compatible. Create `crates/loams/src/api/connect_streams.rs` and `crates/loams/tests/connect_streams.rs`. Modify `crates/loams-stream-grpc` so that it keeps only the CloudEvents gRPC binding (`io.cloudevents.v1`), which is compat.

**Interfaces:**
- `CreateStream` (idempotent by name), `DescribeStream`, `ListStreams` (paged), `Produce` (`producer_id`, `producer_seq`; never auto-retried without them), `Fetch` (offset, `max_records`, `max_bytes`; unary), `ProduceCloudEvents`, `FetchCloudEvents`.
- Records are `bytes`.
- The tonic `StreamService` on `--stream-grpc-listen` is removed, and the flag prints `--stream-grpc-listen is removed; StreamService is served on --listen`.

Tests:
- Ported REST stream tests.
- `stream_grpc_events.rs`, re-targeted at the main port and passing.
- `produce_without_key_is_not_retried_by_server`: two identical unkeyed produces append twice.
- `produce_with_producer_seq_dedupes`
- `fetch_paginates_by_offset`
- `cloudevents_http_binding_unchanged`: compat.
- `proto_wire_compatible_with_tonic_era`: messages encoded by the old generated types decode with the new ones.

Commit `feat(api): loams.stream.v1 on the main port`.

### Task 3: `loams.link.v1`

**Files:** `proto/loams/link/v1/link.proto`, `crates/loams/src/api/connect_links.rs` and `crates/loams/tests/connect_links.rs`.

**Interfaces:** `CreateLink` (idempotent by name; the link spec of `api/mod.rs::create_link` as a message, not a `Struct`, unless Task 0 finds it open-ended), `DescribeLink` and `ListLinks` (paged).

Tests: the ported REST link tests, plus `create_link_repeat_is_safe` and `list_links_paginates`.

Commit `feat(api): loams.link.v1`.

### Task 4: `loams.internal.v1` and the cluster-wide idempotency ledger

**Files:** create `proto-internal/buf.yaml` and `proto-internal/loams/internal/v1/internal.proto`. Modify `crates/loams-proto` (feature `internal`, off by default; `crates/loams` enables it). Create `crates/loams/src/api/connect_internal.rs` and `crates/loams/src/api/idempotency_store.rs`. Modify `api/internal.rs` (deleted at the end of this task), `cluster.rs` (callers) and `connect_documents.rs` (switch to the store). Tests: `crates/loams/tests/connect_internal.rs`, plus the cluster suite (`tests/cluster.rs`).

**Interfaces:**
- `InternalService`: `ForwardRead`, `NodeStats`, `HotStatus` and `HotWarm`, mapped one to one from `HOT_STATUS_PATH`, `HOT_WARM_PATH`, `NODE_STATS_PATH` and the forwarded-read route.
- It is served only by `internal_router`, and it keeps today's cluster-token check (mTLS when MT adds it).
- The metastore Raft transport moves to `loams.internal.v1` only if openraft's wire types allow it (API1 rulings 0.1–0.4). Otherwise it stays and is recorded.
- `IdempotencyStore` as in the shared contracts, with a 24 h window. It is backed by `loams-meta`, and by TiKV under `--meta tikv://`. `WriteDocuments` uses it. The per-process `Ledger` stays only as the store's in-flight gate.

Tests:
- `cluster_tests_use_internal_rpc`: `tests/cluster.rs` passes with `/internal/*` deleted.
- `internal_service_not_on_public_port`: every `/loams.internal.v1.*` path on `--listen` is a 404 (not routed), and the descriptor is not in public reflection.
- `internal_requires_cluster_token`
- `keyed_write_replays_across_nodes`: two gateways; the retry lands on the second and replays the first answer.
- `same_key_other_fingerprint_is_fresh_write` (the existing rule, kept).
- `ledger_window_expires_at_24h` (with a mock clock).
- `internal_module_excluded_from_sdk_generation`: no `sdks/**` output names `loams.internal`.

Commit `feat(api): loams.internal.v1 on the cluster listener; cluster-wide idempotency`.

### Task 5: Compat gRPC on the main port (D601)

**Files:** `crates/loams/src/api/connect_compat.rs` and `crates/loams/tests/connect_compat.rs`. Modify `server.rs` to build the tonic `Routes` for Qdrant gRPC and Flight SQL once, and to mount them on both their own port and the main router.

**Interfaces:**
- Routed by path prefix on the main router: `/qdrant.` goes to Qdrant's tonic server and `/arrow.flight.protocol.` goes to Flight. This happens before the connect router; the Connect paths cannot collide (`/loams.`).
- It uses gRPC only. gRPC-Web is not offered for compat.
- The main-port mount passes through the `Authenticator` (Task 6). The conventional ports keep their current auth until MT1 (API2-Q5), recorded as an accepted risk in Task 27.

Tests:
- `qdrant_grpc_client_works_on_main_port`: the Qdrant Rust client from `tests/qdrant`.
- `flight_client_works_on_main_port`
- `qdrant_grpc_bytes_unchanged_on_main_port`: the same request on both ports gives byte-equal responses.
- `compat_grpc_on_main_port_requires_auth`
- `conventional_ports_unchanged`: the existing suites.

Commit `feat(api): Qdrant gRPC and Flight SQL on the main port`.

### Task 6: Authentication on the main port, and `loams.auth.v1`

**Files:** `crates/loams/src/api/{auth.rs,connect_auth.rs}`, `proto/loams/auth/v1/auth.proto` and `crates/loams/tests/connect_auth.rs`. Modify `connect.rs` (`WhoAmI` answers the principal), `server.rs` (the layer) and `main.rs` (`--auth`).

**Interfaces:**
- `Authenticator`, `Principal` and `PUBLIC_RPCS` as in the shared contracts.
- Implementations:
  - `ApiKeyAuthenticator`: keys `lk_<26-char ULID>_<32 base62>`, stored as argon2id hashes in the metastore under the org.
  - `DevLoopbackAuthenticator`: only when `loams dev` listens on loopback. It yields `PrincipalKind::DevLoopback` and is refused at startup on any other address.
  - `JwtAuthenticator`: verifies the instance's own access tokens against its JWKS (D447, D449), when MT1 provides the issuer. Not required for this task to land (API2-Q1).
- `--auth {api-key,jwt,dev}`: `dev` is the default for `loams dev` (loopback only), and `api-key` otherwise; `api-key,jwt` becomes the default once MT1's issuer is configured. `--auth jwt` without an issuer refuses to start with a clear message.
- The `Authenticator` trait is the only hook MT1 needs: no handler, router or test in this task assumes which implementations exist.
- `AuthService`: `ListProviders`, `CompleteSetup`, `GetSession`, `CreateSession`, `DeleteSession` and `DecideConsent` (§44 §5.2). Sessions are cookie or bearer. `CreateSession` sets an `HttpOnly; Secure; SameSite=Lax` cookie on the Connect response.
- **Protocol endpoints that stay HTTP** (D602): `/oauth/token`, `/oauth/authorize`, `/.well-known/{oauth-protected-resource,oauth-authorization-server,jwks.json}` and `/auth/oidc/{provider}/{start,callback}`. They are served only when MT1's issuer is present. Otherwise they are absent, not stubbed.
- Errors are `unauthenticated` with one of the reasons `token_missing`, `token_malformed`, `token_expired` (exists), `token_revoked` and `credential_in_query_string`, and the `WWW-Authenticate: Bearer` header on HTTP.

Tests:
- `every_served_rpc_requires_auth_except_allowlist`: walks `CATALOGUE` and the descriptor set, calls every served RPC with no credential over Connect, gRPC and gRPC-Web, and asserts `unauthenticated` except for `PUBLIC_RPCS`.
- `api_key_round_trip`
- `expired_jwt_gets_token_expired`: the reason that R1's refresh keys on.
- `query_string_token_refused`: `?access_token=` and `?token=` are refused with `credential_in_query_string`.
- `bearer_never_logged`: captures `tracing` output and error bodies across the suite, and asserts the token bytes are absent.
- `dev_loopback_refused_on_public_address`
- `session_cookie_and_bearer_both_work`
- `oidc_start_remains_http_redirect`: only with MT1; `#[ignore]` with the reason otherwise.
- `jwt_without_issuer_refused_at_startup`
- `authenticator_is_swappable`: the suite passes with a test `Authenticator` injected, proving MT1 can add one without handler changes.

Commit `feat(api): main-port authentication and loams.auth.v1`.

### Task 7: `loams.admin.v1`, and the console mock on Connect

**Files:** `proto/loams/admin/v1/{org,project,agent,key,audit}.proto`, `crates/loams/src/api/connect_admin.rs` and `crates/loams/tests/connect_admin.rs`. Modify `crates/loams-console-mock` to serve the same services over Connect, removing its `/api/v1` routes in Task 11 (not here).

**Interfaces:**
- The RPCs of §44 §5.2's table. `KeyService/CreateKey` returns the key once; a replay returns `secret_already_issued` (the PG2 reason).
- `AuditService/WatchAuditEvents` streams a snapshot cursor, then events, then a heartbeat every 15 s.
- Authorization: every admin RPC checks the principal's org and role through an `Authorizer` trait. It is OpenFGA when MT1 lands; until then it is `OrgRoleAuthorizer` (owner/admin/member from the metastore).

Tests:
- Each console-mock route test, ported with `_rpc`.
- `create_key_returns_secret_once`
- `member_cannot_create_key`: `permission_denied`, reason `role_required`.
- `watch_audit_resumes_from_cursor`
- `mock_and_server_answer_alike`: the console-mock and the server give equal responses (up to volatile fields) on a scripted session, so the console can test against either.

Commit `feat(api): loams.admin.v1`.

### Task 8: Operations on the main port

**Files:** `crates/loams/src/api/connect_operations.rs` and tests `crates/loams/tests/connect_operations.rs`. No `connect_durable.rs` and no `proto/loams/durable/` (API2-Q3).

**Interfaces:**
- `OperationsService` (`GetOperation`, `ListOperations`, `WatchOperations`, `CancelOperation`) is served on the main port over one `OperationStore` that graph and postgres write to. The `loams.operations.v1` row goes into `CATALOGUE`.
- **No `loams.durable.v1`** (API2-Q3). The Resonate HTTP protocol stays the only durable API and is unchanged (compat). SDK users reach durable functions through the Resonate SDKs; a Loams durable package is a future plan under this plan's rules.

Tests:
- `operations_list_includes_graph_and_postgres_ops` (the features on).
- `watch_operations_snapshot_then_changes`
- `cancel_is_idempotent`
- `resonate_protocol_unchanged`: the existing durable suite.
- `no_durable_package_served`: `GetInstance.services[]` and reflection list no `loams.durable.*`.

Commit `feat(api): operations on the main port`.

### Task 9: Catalogue completeness, served-only reflection and variants

**Files:** `crates/loams/src/api/connect.rs` and `crates/loams/tests/connect_api.rs`.

**Interfaces:**
- `CATALOGUE` holds every row of the shared table.
- Reflection uses `Reflector::with_services(served_services())` (PG2 R1.4), so an unserved service is never listed. Reflection stays on in `loams dev` and off elsewhere unless `--reflection` is passed (Q603, API1 ruling 1.4).
- `--reflection` is added, and requires authentication.
- Live and Postgres: if LV1's and PG2 Task 9's wiring has merged, only the catalogue and test rows change here. If not, this task leaves `LiveAbsent` and the Postgres absent stub in place and records it.
- `VARIANT` follows CLI2's `release/variants.toml` once it exists (API1 ruling 1.5).

Tests:
- `reflection_lists_only_served_services`
- `reflection_off_by_default_outside_dev`
- `catalogue_matches_design_and_protos`: extends `route_map.rs`'s design guard. Every proto package under `proto/` that has a service has a row, and every row has a proto.
- `unavailable_service_reports_reason`: for every row whose feature is off.
- `get_instance_reports_unstable`

Commit `feat(api): catalogue complete, served-only reflection`.

## API2b — Removing REST and OpenAPI (Tasks 10–12)

### Task 10: The console, data studio and the desktop on Connect

**Files:**
- `web/apps/console/src/api/{client.ts,use.ts}`, the pages that call it (`shell.tsx`, `session.tsx`, `pages/{home,org,project,environment,agents,auth}.tsx`) and `runtime-config.ts`.
- `web/apps/console/package.json`: remove `openapi-fetch`, `openapi-typescript` and the `gen:api` script.
- Delete `web/apps/console/src/api/schema.d.ts`.
- `web/plugins/data-studio/src/client.ts`.
- `apps/desktop-electron/src/main/protocol/local-shim.ts`: it answers the Connect paths `InstanceService/GetInstance` and `AuthService/GetSession` instead of `/api/v1/instance` and `/api/v1/session`.

**Interfaces:** one `createLoamsTransport(runtimeConfig)` in `web/apps/console/src/api/client.ts`, over `@connectrpc/connect-web`, with the clients from `@loams/proto`. Data studio uses `QueryService`, `SqlService` and `CollectionService` through the same transport.

Tests:
- The console's existing tests, re-targeted, run in `web` and `web-browser`.
- `console_has_no_openapi_dependency`: `package.json` and the lockfile have no `openapi-*`.
- `data_studio_uses_connect`: no `/v1/namespaces` string under `web/`.
- `desktop_local_shim_answers_connect`: in `apps/desktop-electron`'s test suite.

Commit `feat(console): Connect clients replace the OpenAPI client`.

### Task 11: Delete the native REST and the OpenAPI (no shims)

**Files:**
- Delete the REST handlers in `crates/loams/src/api/{mod.rs,collections.rs,query.rs,sql.rs,streams.rs,events.rs,hot.rs}`, keeping the helpers the Connect handlers use, moved next to them. `/health` and `/ready` stay.
- Delete `api/console/openapi.json` and the console mock's `/api/v1` routes.
- Create `docs/api/migrate.md` (generated from the route map by `scripts/api/migrate-page.sh`). No `legacy.rs` (Q600).
- Amend the status lines of M1.6 and AP1a, one line each.

**Interfaces:**
- Delete outright (Q600). There is no `--legacy-rest` flag and no shim: a removed path is answered by the framework's unknown-path 404, with the JSON error body.
- `docs/api/migrate.md` maps every removed route to its RPC, and the release notes link it.

Tests:
- `no_native_rest_route_remains`: the router inventory equals the protocol endpoints of D602 plus the Connect and compat paths.
- `route_map_covers_every_route`: still green, every row now "removed" or "protocol".
- `removed_rest_paths_are_404`: every removed route of the route map answers 404, never data.
- `no_legacy_rest_flag`: `--legacy-rest` is not a recognised flag.
- `migrate_page_covers_every_removed_route`
- `framework_rejections_use_the_json_error_body`: still holds for unknown paths.
- `compat_suites_green`
- The console's tests.

Commit `feat(api): remove the native REST and the console OpenAPI`.

### Task 12: Contract gates

**Files:** `.github/workflows/api-protos.yml` (or a job in `ci.yml`, as `app-protos` is), `docs/api/curl.md`, `crates/loams/tests/curl_examples.rs`, `.github/CODEOWNERS`, `scripts/ci/no-metering.sh` and `buf.yaml`.

**Interfaces:**
- `buf lint` on `proto/` and `proto-internal/`.
- `buf breaking` against `dev` until a tag `v*` exists, then against the latest tag. It covers every package not listed as `unstable`.
- CODEOWNERS on `docs/api/reasons.md` and `proto/**` (Q612).
- `no-metering.sh` covers `proto/` and `sdks/`.

Tests:
- `breaking_change_fails_ci`: a fixture proto pair under `crates/loams-proto/tests/breaking/`.
- `unstable_flag_and_buf_ignore_agree`: every `ModuleOptions.unstable` package is in `breaking.ignore`, and nothing else is.
- `curl_examples_run`: runs every block of `docs/api/curl.md` against `loams dev`.
- `reasons_are_snake_case_and_unique` (exists).
- `no_reason_removed_since_last_tag`

Commit `ci(api): breaking-change, curl and reason gates`.

## API2c — The pipeline, complete (Tasks 13–18)

### Task 13: Facade annotations on every stable package, and the facade index

**Files:** the protos of `loams.collection.v1`, `sql`, `stream`, `link`, `auth`, `admin` and `operations`. Also `crates/loams-facade-gen/src/{index.rs,lib.rs}`, `sdks/fixtures/facade-index.json`, every `sdks/<lang>/buf.gen*.yaml` package map, and the regenerated `facade.ts`, `facade.py`, `facade.rs` and Go `gen/facade`.

**Interfaces:**
- Each stable RPC carries `FacadeOptions`. API1 ruling 2.5's blocker (stubs before options) is resolved by adding the package map entries and the stubs in the same PR.
- `QueryService/Search` carries two entries: `{module:"vector" name:"search"}` and `{module:"search" name:"query"}` (Q613: both).
- List RPCs carry `pagination`.
- `lang=index` renders `facade-index.json`.

Tests:
- `every_stable_service_has_module`
- `every_stable_rpc_has_facade_call`
- `vector_and_search_share_one_rpc` (exists)
- `retry_class_follows_idempotency_level` (exists)
- `pagination_iterator_for_list_rpcs`
- `golden_index`
- `drift.sh` clean for all four generated languages.

Commit `feat(facade): every stable package annotated; the facade index`.

### Task 14: The other nine languages against the index (Q604)

**Files:** `sdks/templates/<lang>/template.env` for Swift, Kotlin, Java, C#, Dart, Ruby, PHP, C++ and Objective-C; `crates/loams-facade-gen/src/{swift,kotlin,java,csharp}.rs`; and a facade-index test in each hand-written SDK's test tree.

**Interfaces:**
- Q604 (ruled): Swift, Kotlin, Java and C# are **generated** (a renderer in `loams-facade-gen`, `golden_<lang>`); Dart, Ruby, PHP, C++ and Objective-C are **hand-written against the index** (D730's fallback).
- Go's `modules.go` is replaced by a generated file when `go.rs` renders modules, which is part of this task.

Tests:
- `<lang>_facade_matches_index`, for every hand-written language: the SDK's module/call set and per-call retry classes equal `facade-index.json`'s.
- `golden_<lang>`, for every generated language.
- `go_modules_generated`: `modules.go` carries the generated header, and `drift.sh` covers it.

Commit `feat(facade): nine languages checked against the index` (one commit per language is fine).

### Task 15: The fixture corpus covers the API

**Files:** `sdks/fixtures/{manifest.json,index.json,faults.json,recorded/}`, `sdks/conformance/{record-fixtures.mjs,faults.mjs,fixture-server.mjs,verify-corpus.mjs}`, `crates/loams-apps-mock` (fault RPCs on Connect) and `docs/sdk/fixtures.md`.

**Interfaces:**
- Recordings from `loams dev`, in the four encodings (Connect JSON, Connect proto, gRPC-Web, gRPC-Web JSON), for every stable package: namespaces and collections; documents (keyed write and replay; consistency token returned); search (dense, text, hybrid, filters); SQL; streams (produce/fetch); links; admin (with a dev principal); operations.
- Faults: retryable `unavailable` with `RetryInfo`, `resource_exhausted` with `RetryInfo`, `deadline_exceeded` before the response starts, a mid-stream disconnect after *n* frames, and `token_expired` followed by a good call. These make `mock_injects_retryable_errors` real.
- The stale manifest notes from Task 0 are rewritten. R2, R4 and R6 become `covered: true`.

Tests:
- `fixtures_pass_against_real_server`: a Rust test runs every non-fault fixture against `loams dev`.
- `mock_injects_retryable_errors`
- `every_clause_has_a_fixture`: every clause R1–R10 has at least one required fixture.
- `fixtures_carry_no_credentials`: no recorded header or body holds a bearer, key or cookie value.
- `real_server_ignores_test_fault_header`
- `verify-corpus.mjs --drift` clean.

Commit `test(sdks): the corpus covers every stable package and every clause`.

### Task 16: The runtime contract, v1

**Files:** `docs/sdk/runtime-contract.md`, `proto/loams/errors/v1/errors.proto` (documentation of the `google.rpc.RetryInfo` detail and of `request_id` in `metadata`), `crates/loams/src/api/connect_errors.rs` (emit them), and the consistency-token encoder in `crates/loams/src/api/connect_documents.rs` / `connect_query.rs`.

**Interfaces:**
- **R2.** The server attaches `google.rpc.RetryInfo` to `unavailable` and `resource_exhausted` when it knows a delay. SDKs honour it up to 30 s.
- **R4.**
  - The token is `v1:<base64url(postcard(TokenV1{instance_id, entries: [(stream_id, partition, offset)]}))>`.
  - `ConsistencyToken::merge` takes the maximum offset per `(stream_id, partition)`, and a token from another `instance_id` is refused with `invalid_argument`, reason `consistency_token_foreign`.
  - The session store is off by default (`session_consistency`).
- **R8.** Every error carries `metadata.request_id`, which equals the `loams-request-id` header.
- **R10.** Unchanged.

Tests:
- `token_merge_is_per_stream_partition_max`: a property test, server side, with the same vectors under `sdks/fixtures/tokens.json`.
- `foreign_instance_token_refused`
- `retry_info_present_on_unavailable`
- `request_id_header_matches_metadata`
- `produce_without_key_never_retried`: a fixture with `retry: never` that every SDK must pass.

Commit `feat(api): RetryInfo, request ids and the v1 consistency token`.

### Task 17: Typed query builders

**Files:** `sdks/{typescript/packages/client,python/src/loams,go,rust/src,java,kotlin,swift,csharp}/…/query_builder.*` and a shared fixture set `sdks/fixtures/builders/`.

**Interfaces:**
- Each builder is a thin layer over the generated `SearchRequest`: `dense(field, vector)`, `sparse(field, indices, values)`, `text(field, query)`, `filter(...)`, `fuse(rrf|weighted)`, `expand(graph)`, `limit`, and `consistency`.
- Each builder's output must equal the fixture's request JSON byte for byte, after canonical JSON.
- Dart, Ruby, PHP, C++ and Objective-C expose the generated messages, with documented examples (§44 §7.6).

Tests: `<lang>_builder_matches_fixtures` for each of the eight languages.

Commit `feat(sdks): typed hybrid-query builders` (one per language is fine).

### Task 18: Bulk data

**Files:** a `bulk` module in Python, Go, Java, C++, Rust and C#, and a `write_batch` helper in every language. Fixtures `sdks/fixtures/bulk_*`.

**Interfaces:**
- `write_batch` chunks `WriteDocuments`: 10 000 rows or 4 MiB, whichever comes first, one chunk in flight, and each chunk keyed (`<batch key>/<chunk index>`) so a retry replays.
- `bulk` uses Flight SQL `DoPut`/`DoGet` on the main port (Task 5) with the same bearer.
- `query_arrow` exists only where an Arrow IPC reader is available (TypeScript with `apache-arrow`, plus the six Flight languages).

Tests:
- `chunking_whole_or_nothing`
- `chunk_retry_replays_not_rewrites`
- `<lang>_bulk_put_get_roundtrip`: the six Flight languages.
- `query_arrow_matches_rows`

Commit `feat(sdks): write_batch and bulk`.

## API2d — Every language to production (Tasks 19–23)

### Task 19: A CI job for every language

**Files:** `.github/workflows/ci.yml` (jobs `sdk-java`, `sdk-kotlin`, `sdk-ruby`, `sdk-php`, `sdk-dart` and `sdk-objc`; the `changes` filter; `required.needs`).

**Interfaces:**
- Each job builds `loams` once and downloads it (the `sdk-typescript` pattern), runs `sdks/conformance/run.sh <lang>`, and checks the report against `required.mjs`.
- Wave 1 jobs run on every PR that touches `proto/**`, `sdks/<lang>/**`, `sdks/fixtures/**` or `crates/loams/src/api/**`, and are in `required.needs`.
- Waves 2 and 3 run nightly and on tags only, and are never in `required.needs` in this plan (API2-Q2). A nightly failure opens or updates one issue per language. Promoting a language to PR-blocking is a later, separate ruling.
- A PR that touches only `sdks/<lang>/**` of a wave 2 or 3 language runs that language's job on the PR as an advisory (non-required) check.
- The PHP job runs twice: with `ext-grpc`, and without it (Connect-unary).
- Ruby likewise.

Tests:
- `workflow_lint`: `actionlint` clean.
- `every_sdk_dir_has_a_job`: a script that fails when an `sdks/<lang>` directory has no `sdk-<lang>` job.
- `only_wave1_is_required`: `required.needs` holds exactly the wave 1 jobs.

Commit `ci(sdks): a conformance job for every language`.

### Task 20: Wave 1 (TypeScript, Python, Go, Rust) on the whole API

**Files:** `sdks/{typescript,python,go,rust}`, with their READMEs and examples.

**Interfaces:**
- Every module in `facade-index.json`.
- Python: sync and async clients, `mypy --strict`, and extras `flight`, `arrow` and `polars`. It replaces M1.6's `loams-client`.
- Go: context-first, with `*Stream` helpers.
- Rust: an async `Stream`, and no `tonic`.
- TypeScript: Node ≥ 22, browsers, Deno and Bun. `@loams/live` re-exports.
- Every `TokenSource` of D608 exists. `OidcExchange` and `WorkloadIdentity` are tested against MT1's token endpoint when it exists, and against the fault server's `token_expired` script otherwise.

Tests (per language, named `<lang>_…`):
- `conformance_all_required_fixtures`
- `retry_reuses_idempotency_key`
- `error_reason_mapping`
- `stream_resume_with_cursor`
- `token_source_refresh`
- `pagination_iterator`
- `session_consistency_read_your_writes`
- `request_id_on_error`

Commit `feat(sdk-<lang>): the whole API` (one per language).

### Task 21: Wave 2 (Swift, Kotlin, Java, C#)

**Files:** `sdks/{swift,kotlin,java,csharp}`.

**Interfaces:**
- Everything Task 20 lists, in each language's idiom (§44 §7.1).
- Swift: `AsyncSequence`, iOS 15+ / macOS 12+ / Linux.
- Kotlin: `Flow`, JVM and Android. It shares generation with AP2's `buf.gen.kotlin.yaml`, which this task folds into `sdks/kotlin/`.
- Java: Java 17+, blocking and async, `Iterator` streams, and `grpc-netty-shaded` as an optional artifact.
- C#: net8.0, `IAsyncEnumerable`, `AddLoams` DI, and gRPC-Web for Blazor WASM.
- `buf.gen.swift.yaml` and `buf.gen.kotlin.yaml` at the root move under `sdks/<lang>/` (SDK1 Task 1's fold).

Tests: the eight `<lang>_…` tests of Task 20, for each language.

Commit `feat(sdk-<lang>): the whole API` (one per language).

### Task 22: Wave 3 (Dart, Ruby, PHP, C++, Objective-C)

**Files:** `sdks/{dart,ruby,php,cpp,objc}`.

**Interfaces:**
- Task 20's list, with these per-language details:
  - Dart: the Connect library if Task 0's check passes, otherwise `grpc`.
  - Ruby and PHP: both transports (D613). `transport: grpc-only` fixtures are skipped on unary.
  - C++: C++17 and a CMake config package; a vcpkg registry (git) and an in-repo Conan recipe (Q610). Uploading to a hosted Conan remote is gated by `SDK_PUBLISH_CONAN` (Task 25).
  - Objective-C: generated sources and a SwiftPM target, and no CocoaPods (D614). If it fails conformance (Q611), the matrix row says "community".
- `sdks/php/vendor/` stays untracked (it is not in git on 2026-10-10) and is built by Composer in CI from `composer.lock`.

Tests: the eight `<lang>_…` tests, plus `ruby_unary_transport_conformance` and `php_unary_transport_conformance`.

Commit `feat(sdk-<lang>): the whole API` (one per language).

### Task 23: Protocol conformance

**Files:** `sdks/conformance/connect-conformance/` (config and runner), and a nightly CI job `connect-conformance`.

**Interfaces:**
- `connectrpc/conformance`, at a version pinned in `pins.lock`, runs in server mode against the main port's handler, and in client mode for every Connect SDK (TypeScript, Python, Go, Rust, Swift, Kotlin, Dart if Connect).
- Known failures go in an allowlist file, one line per case with a reason. The allowlist may only shrink.

Tests:
- `server_conformance_passes_or_allowlisted`
- `<lang>_client_conformance_passes_or_allowlisted`
- `allowlist_only_shrinks`

Commit `test(api): Connect protocol conformance`.

## API2e — Release (Tasks 24–28)

### Task 24: Versioning

**Files:** `docs/sdk/versioning.md` and `sdks/<lang>/LOAMS_PROTO_REV` (one line: the `proto/` tree's git sha), plus the R9 check in each runtime.

**Interfaces:**
- Each SDK declares `LOAMS_PROTO_REV` and the supported `api_versions` range.
- At first call, `GetInstance` is compared: a minor mismatch is a warning and a major mismatch is a `failed_precondition` error with reason `api_version_unsupported`.
- SDKs stay `0.y.z` while the server is pre-1.0.
- Each SDK keeps its own changelog under `sdks/<lang>/CHANGELOG.md`.

Tests:
- `<lang>_version_check` (all 13).
- `proto_rev_matches_tree`: CI fails when `LOAMS_PROTO_REV` is not the sha the generated code came from.

Commit `feat(sdks): proto revision and version checks`.

### Task 25: Release workflows and mirrors

**Files:** `.github/workflows/{sdk-release.yml,sdk-mirror.yml}`, `scripts/sdk/release-dry-run.sh` and `docs/release/publishing.md`.

**Interfaces:**
- Tags are `sdk-<lang>-v<semver>`, signed.
- There is one job per registry: npm, PyPI, crates.io, RubyGems, NuGet and pub.dev use OIDC trusted publishing; Maven Central uses a Portal token and a GPG key; Packagist uses a webhook from the mirror; Go and SwiftPM use mirror tags; vcpkg uses its own git registry and Conan its own remote (Q610).
- Every job runs `--dry-run` on PRs, and runs for real only when `vars.SDK_PUBLISH_<REGISTRY> == 'true'`. The owner sets that variable after §44 §13's action.
- Generated code is Apache-2.0 (Q609); each package's licence field says so.
- `sdk-mirror.yml` subtree-splits `sdks/go`, `sdks/swift` and `sdks/php` into `ostrium-labs/loams-{go,swift,php}`. The mirrors reject human pushes (a branch rule, recorded in the doc).
- `publishing.md`'s table is rewritten. Its "D400 defers Java", "no Go module" and "no Python SDK" rows are no longer true.

Tests:
- `workflow_lint`
- `dry_run_all_languages`: a PR run of `sdk-release.yml` in dry-run mode, green.
- `no_long_lived_token_where_oidc_exists`: a script check of the workflow's secrets usage.

Commit `ci(sdks): release and mirror workflows`.

### Task 26: Reference docs and snippets

**Files:** `docs/api/reference/` (generated), `docs/api/snippets/` (from the fixtures) and `buf.gen.docs.yaml`.

**Interfaces:**
- The doc plugin is pinned in `pins.lock`. It writes one Markdown page per package.
- Snippets are rendered from the fixtures for curl, Python, TypeScript and Go, and are used by the README quickstarts.
- The `loams-cloud` site rendering is a follow-up issue in that repository, not here.

Tests:
- `every_rpc_documented`: every public RPC has a non-empty comment.
- `snippets_match_fixtures`
- `docs_snippets_compile`: for TypeScript, Python, Go and Rust, without a server.

Commit `docs(api): generated reference and tested snippets`.

### Task 27: Security review

**Files:** `docs/security/api-and-sdks-threat-model.md`, plus the tests below in their packages.

**Interfaces:** a threat model covering:
- the main port: auth (API keys and dev-loopback before MT1, API2-Q1), reflection, gRPC-Web CORS, compat mounts and `loams.internal.v1` exposure;
- the conventional compat ports' pre-MT1 auth, as an owner-accepted risk (API2-Q5);
- the SDKs: token storage, redaction, TLS defaults (verify on; `http://` only to loopback unless `allow_insecure` is set), retry amplification and idempotency;
- release: OIDC scopes, mirror write rights, signing.

Tests:
- `sdk_token_source_debug_redacted` (all 13).
- `sdk_refuses_plain_http_to_non_loopback`: every language.
- `cors_allows_only_configured_origins`
- `retry_budget_caps_amplification`: every SDK retries at most 3 times per call, and the client-wide budget (10% of calls over 10 s) holds.
- Every high or critical finding is closed or recorded as an owner-accepted risk.

Commit `docs(security): API and SDK threat model, and its tests`.

### Task 28: Docs, plan statuses and the decision log

**Files:**
- `sdks/README.md` (the naming fix from Task 0).
- The language matrix page `docs/sdk/languages.md` (SDK2 Task 13).
- `CONTRIBUTING.md` ("adding a community SDK").
- The status lines of API1, SDK1 and SDK2, each with a pointer to API2.
- `docs/plans/README.md`.
- In `docs/design/13-decision-log.md`, the "Owner rulings 2026-10-10 (defaults)" for Q600–Q614 and API2-Q1–Q5 (the latter under new IDs), each dated 2026-10-10, plus any later override.

Tests:
- `docs` (existing CI job): links resolve.
- `languages_page_matches_manifest`: every language's row matches its conformance report.

Commit `docs(api2): status, matrix, and decisions`.

---

## Exit criteria for production (§44, with the owning tasks)

- [ ] **One API:** every application call is an RPC on the main port, the catalogue is complete, reflection is served-only, and every unavailable package refuses with a reason: Tasks 1–3, 6–9.
- [ ] **No bespoke REST:** the native REST, the console OpenAPI and `/internal/*` are deleted, the console, data studio and desktop use Connect, and no shim exists (Q600): Tasks 4, 10, 11.
- [ ] **Compat unchanged:** every compat suite is green, and Qdrant gRPC and Flight are also on the main port: Task 5.
- [ ] **Auth:** every served RPC needs a principal except the allowlist, with no token in a URL or a log: Task 6.
- [ ] **Idempotency** holds across gateway nodes: Task 4.
- [ ] **Contract gates:** `buf lint`, `buf breaking` against the tag, `unstable` agreement, reasons never removed, and the curl examples run: Task 12.
- [ ] **Facade from one source:** every stable RPC is annotated, and every SDK matches the index: Tasks 13, 14.
- [ ] **Corpus:** every stable package and every clause R1–R10 has a required fixture, and faults are injected: Task 15.
- [ ] **Runtime contract v1:** RetryInfo, request ids and the v1 token: Task 16.
- [ ] **Thirteen languages:** each passes 100% of the required fixtures in a CI job (wave 1 PR-required; waves 2 and 3 nightly, API2-Q2), with Connect protocol conformance for the Connect SDKs: Tasks 19–23.
- [ ] **Builders and bulk** where §44 §7.5–§7.6 say so: Tasks 17, 18.
- [ ] **Release:** version checks, dry-run publishing of every language, mirrors, and signed tags: Tasks 24, 25.
- [ ] **Docs:** the reference is generated, the snippets are tested, and there is a migration page: Tasks 11, 26, 28.
- [ ] **Security:** the threat model, the tests, and no open high or critical finding: Task 27.

## Self-review

- **Spec coverage.**

  | §44 section | Task(s) |
  |---|---|
  | §4 One API, one port | 5, 6, 9 |
  | §5 Removal and migration | 1–4, 7, 10, 11 |
  | §6 Compat surfaces | 5, (all: constraint) |
  | §7.1–§7.3 SDK shape, catalogue, annotations | 13, 14, 20–22 |
  | §7.4 Cross-cutting behaviour | 4, 6, 15, 16 |
  | §7.5 Arrow and bulk | 1, 18 |
  | §7.6 Builders | 17 |
  | §8 Services to add | 1–4, 6–8 |
  | §9 Language matrix | 0, 14, 19–22 |
  | §10 Generation, conformance, docs | 12–15, 23, 24, 26 |
  | §11 Publishing | 25 |
  | §12 Risks | 14 (risk 6), 19 (1), 22 (8, 10), 23 (2, 3) |
  | §13 Owner actions | 25 (gated by `SDK_PUBLISH_*`) |

- **Types.** `Authenticator`, `Principal`, `PUBLIC_RPCS`, `IdempotencyStore`, the facade index and the header table are defined once, in the shared contracts.
- **Review Focus.** Items 1–9 each name an owning test (Tasks 4, 16, 20–22, 6, 15, 27, 5, 11, 14, 13, 9, 12).
- **Not in this plan.** MCP (no server exists; M1.6), `loams.durable.v1` (API2-Q3), `loams.jobs.v1`, `loams.flow.v1`, `loams.git.v1`, `loams.systemone.v1`, `loams.collab/bot/factory.v1` and `loams.console.v1`. Each of those plans adds its package under this plan's rules (a proto, a catalogue row, the facade annotations and fixtures), and Task 13's tests catch an omission.

## Open questions

Every design question is ruled in "Owner rulings 2026-10-10 (defaults)" at the top. What is left needs the owner personally, because it involves accounts, money or legal standing. None blocks a code task: each only gates a real publish, and every release job stays a dry run until then (Task 25).

| # | Owner action | Why a human | Gates |
|---|---|---|---|
| API2-H1 | Create or reserve the registry accounts and namespaces of §44 §13 (npm `@loams`, PyPI, crates.io, RubyGems, NuGet, pub.dev, Maven Central with a GPG key, Packagist), configure OIDC trusted publishing, then set each `vars.SDK_PUBLISH_<REGISTRY>` | External accounts in the company's name | Task 25 real publishes |
| API2-H2 | Host a Conan remote (and decide whether to pay for one) for Q610's C++ packages, or leave C++ at the vcpkg git registry plus the in-repo recipe | External account, possible cost | `SDK_PUBLISH_CONAN` |
| API2-H3 | Create the mirror repositories `ostrium-labs/loams-{go,swift,php}` with the bot-only push rule, and serve the `loams.dev/go` `go-import` meta tag (Q607) from the `loams-cloud` site | GitHub org and domain administration | Task 25 mirrors, Go module path |
| API2-H4 | Confirm Q609 (generated code under Apache-2.0, no DCO on bot-written mirrors) with whoever handles the company's legal review | Legal | First real publish |

## Rulings made during execution

(Task 0 and later tasks append here.)
