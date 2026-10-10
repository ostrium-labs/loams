![Loams — Your data. Your bucket.](../../../docs/assets/loams-banner.svg)

# @loams-core/engine

Read-only analytics over a **Loams** deployment, over Loams' native HTTP/JSON
REST API.

---

## Maturity: read this first

Loams is early software and says so itself. `README.md:7` in the upstream
repository:

> **Early and moving fast.** Loams has no stable release yet, and APIs, formats
> and flags change without notice. The crates are `loams-*` and the binary is
> `loams`; nothing is published yet.

More specifically:

- Most of the upstream design documents carry `Status: **Proposed**`.
- The console API is **mock-only** (`README.md:57`); the web console is built
  against a mock, not the real thing.
- `loams.live.v1` and the streaming gRPC API are both **In progress**
  (`README.md:59-60`), and `loams.stream.v1` is opt-in. This adapter models
  neither and builds on neither.
- "Auth and tenancy — API keys, authorization, tenant quotas and a namespace
  router" is **Planned** (`README.md:62`).

**Consequences for this adapter.** It degrades gracefully rather than assuming a
stable contract: a field the server drops is read as absent, a missing array
degrades to `[]`, a missing numeric count degrades to `0` with a warning. It does
not assume a schema is frozen. Every field type below was read from a specific
`file:line` in the upstream source and carries that citation.

## What Loams is

A Rust **hybrid-retrieval / data engine** — not a plugin runtime, not a
Convex-style BaaS. Lance + Tantivy + DataFusion, with vector kNN fused with BM25
through reciprocal rank fusion. `README.md:3`:

> One bucket, every index: hybrid retrieval on object storage, with reactive data
> and durable agent runs beside it.

Loams is **Apache-2.0 with no copyleft**, and is treated here purely as an
external HTTP service dependency.

There is **no Rust Cordis**. `grep -ril cordis --include='*.rs'` over
`crates/` returns nothing. The "cordis" inside the Loams repository is
`web/packages/cordis`, a browser-side npm cordis `4.0.0-rc.10` that drives the
Rust server over HTTP. It has nothing to do with `@loams-core/host`'s cordis services
and this adapter does not attempt to bridge them.

## REST, not Connect-RPC

Loams exposes two service surfaces: the native REST route table at
`crates/loams/src/api/mod.rs:117-189`, and a smaller Connect-RPC surface
(`connectrpc = "0.9.1"`, generated from `proto/loams/*/v1/*.proto`).

**Plain REST is the right surface for a read-only adapter**, for four reasons:

1. **It is the documented read surface and the wider one.** `README.md:112-139`
   demonstrates namespaces, collections, documents, `/query` and `/sql` entirely
   over `curl`. The proto tree has no equivalent of collections, documents,
   `/query` or `/sql`.
2. **The proto tree is the unstable half.** `loams.live.v1` and the streaming
   gRPC API are In progress and `loams.stream.v1` is opt-in. REST is what the
   project's own docs treat as the contract.
3. **`@loams-core/http` is an HTTP client.** Reusing `UpstreamClient`
   means auth headers, timeouts and error normalisation are not reimplemented.
   Connect-RPC would need generated stubs and a second error model for less
   coverage.
4. `/health` and `/ready` are REST-only, and are the cheapest liveness answer a
   dashboard has.

## There is no code execution, and this adapter cannot reach any

Verified exhaustively, not assumed:

- No `/exec`, `/functions`, `/eval` or `/invoke` route exists in
  `crates/loams/src/api/mod.rs:117-189`.
- No interpreter is in the dependency graph: `wasmtime`, `wasmer`, `workerd`,
  `gvisor`, `extism`, `deno` and `rquickjs` are absent from **both** `Cargo.toml`
  and `Cargo.lock`.
- The only code-shaped RPC is **`loams.live.v1.Deploy`**
  (`proto/loams/live/v1/live.proto:32`), which takes a JavaScript ES module
  bundle — and it is **UNIMPLEMENTED**. `rquickjs` is absent, the intended
  `loams-live-js` crate does not exist, and the work is deferred at
  `crates/loams-live/src/txn.rs:644`. Its `max_js_cpu` limit is a declared field
  with nothing enforcing it.

`LoamsAdapterService` therefore exposes no `Deploy` and no code-deployment path.
Every path it builds passes through `_assertSafe`, which throws on any path
containing `live` or `deploy`; `tests/loams-adapter.spec.ts` asserts that no
request this adapter issues ever targets one.

## Authentication

**Loams has no authentication today.** This is not an assumption — it is what the
source says:

- `README.md:62` lists "Auth and tenancy — API keys, authorization, tenant
  quotas and a namespace router" as **Planned**.
- The native router (`crates/loams/src/api/mod.rs:107-176`) has no auth layer,
  no auth middleware and no credential extractor.
- The internal routes are documented as "Unauthenticated, like every listener in
  M1" (`crates/loams/src/api/internal.rs:5`).

So this adapter uses `{kind: "none"}` by default and sends no `Authorization`
header at all. It supports an **optional** bearer token (`LoamsConfig.token`,
`LOAMS_TOKEN`) for the deployment shape this will actually have: a gateway or
reverse proxy in front of Loams enforcing auth. No auth scheme is invented.

## How a namespace is resolved

**From configuration, always. There is no discovery, because the server cannot do
it.**

The route table has `POST /v1/namespaces` — which **creates** a namespace
(`crates/loams/src/api/mod.rs:114`, handler `mod.rs:276-287`). There is no `GET`
counterpart anywhere. `/ready` is a bare status code with no body
(`mod.rs:260-266`) and `/health` likewise (`mod.rs:257-259`). Nothing on the
server will enumerate namespaces.

`LoamsConfig.namespace` (`LOAMS_NAMESPACE`) is therefore resolved from config,
and an unset namespace throws with an explicit message naming the reason and the
fix (`LOAMS_NO_NAMESPACE_MESSAGE`). The adapter never guesses a namespace.

The README's own example uses `demo` (`README.md:118`).

## What is implemented, and what is deliberately omitted

Implemented — all reads:

| Method                         | Route                                              |
| ------------------------------ | -------------------------------------------------- |
| `health()`                     | `GET /health`                                      |
| `ready()`                      | `GET /ready`                                       |
| `listCollections()`            | `GET /v1/namespaces/{ns}/collections`              |
| `describeCollection(c)`        | `GET /v1/namespaces/{ns}/collections/{c}`          |
| `listVersions(c)`              | `GET /v1/namespaces/{ns}/collections/{c}/versions` |
| `search(input)`                | `POST /v1/namespaces/{ns}/query`                   |
| `getDocuments(c, ids)`         | `POST …/collections/{c}/documents/get`             |
| `scrollDocuments(c, opts)`     | `POST …/collections/{c}/documents/scroll`          |
| `countDocuments(c, opts)`      | `POST …/collections/{c}/documents/count`           |
| `sql(query)` — **opt-in only** | `POST /v1/namespaces/{ns}/sql`                     |

**Omitted, and why:**

- **`documents/patch_by_filter` and `documents/delete_by_filter`** — both mutate
  documents. This adapter is read-only.
- **`POST …/collections`** (create), **`DELETE …/collections/{c}`** (drop),
  **`POST …/fields`** (add fields), **`POST …/documents`** (write),
  **`POST …/aliases`**, **`POST /v1/namespaces`** (create namespace),
  **`POST …/streams/*`** (produce, events), **`POST …/links`** — all writes.
- **`PUT …/collections/{c}/hot` and `POST …/collections/{c}/warm`** — these
  mutate server-side hot-tier state even though they read like operations
  (`crates/loams/src/api/hot.rs:32-37`).
- **`POST …/scan`** — it returns a read plan, but it also takes a _pin_ on the
  server (`crates/loams/src/api/collections.rs:136-166`), which is state. Left
  out to keep the "this adapter changes nothing" claim literally true.
- **`streams`, `links`, `events`** reads — real and harmless, but outside the
  dashboard surface this adapter exists for. `GET …/streams/{stream}` and
  `GET …/links/{link}` are available if that changes.
- **`loams.live.v1` and the streaming gRPC API** — In progress, and `Deploy` is
  a code-deployment path. Never built on.

## The `/sql` endpoint is disabled by default

`POST /v1/namespaces/{ns}/sql` takes **raw SQL text** and executes it against
DataFusion. `README.md:138`:

```json
{
  "query": "SELECT _id, body, _score FROM rrf(vector_search('kb', [1.0, 0.0, 0.0], 'embedding', 10), text_search('kb', 'refund', 'body', 10)) LIMIT 3"
}
```

An arbitrary-SQL passthrough is an injection surface by design, so:

- `sql()` throws unless `LoamsConfig.allowSql` is **explicitly `true`**.
- There is **no skill** for it, so no agent can reach it. Editing config is the
  only way in, which is deliberate.
- The statement is passed through **verbatim**. This client does **not**
  sanitise it, does not parse it, and does not check that it starts with
  `SELECT`. `SELECT`-only enforcement happens **server-side** —
  `run_read_only` (`crates/loams-query/src/sql/mod.rs:176-182`) verifies the
  logical plan and refuses anything else (`sql/mod.rs:206-221`). A client-side
  string check would be theatre layered on a real control, and would manufacture
  false confidence that it was not.

The genuine mitigating factor: DataFusion accepts no user-supplied code here. Its
extensions are a fixed set of internally-registered table functions
(`vector_search`, `text_search`, `hybrid_search`, `rrf`, `rerank`) and a few
scalar UDFs (`sql/mod.rs:127-144`). What remains is arbitrary _read_ — data
exfiltration, and the query cost of whatever the caller wrote, bounded by the
server's own `max_rows`/`timeout`.

## Notable wire details

- **`pk` vs `id`.** A search hit carries the key under `pk`
  (`crates/loams-query/src/ir.rs:695-696`); `documents/get` and
  `documents/scroll` rename it to `id`
  (`crates/loams/src/api/collections.rs:529-546`). Same value, different name,
  different endpoint.
- **`limit` defaults to 10** on a search (`ir.rs:28-30`, `ir.rs:52-54`) and to
  **100** on a scroll (`crates/loams/src/api/collections.rs:29`).
- **`fusion` absent means NO fusion**, not RRF. `SearchRequest.fusion` is
  `Option<Fusion>` with `#[serde(default)]` (`ir.rs:44-45`); RRF's `k = 60`
  default applies only once a fusion is present (`ir.rs:28-30`, `ir.rs:250-255`).
  `buildSearchRequest` therefore does **not** add one behind a caller's back.
- **`consistency` defaults to `"strong"`** (`ir.rs:113-115`), so a read sees every
  write acknowledged before it began. `at_least`/`pinned` carry a consistency
  token; the same token can also travel as the `Loams-Consistency-Token` request
  header (`crates/loams/src/api/mod.rs:57-59`).
- **Errors are uniform**: `{"error": <code>, "message": <text>}` plus extra
  fields (`crates/loams/src/api/errors.rs:64-79`). `LoamsApiError` surfaces the
  code, message, status and extras.
- **An unknown namespace yields `[]`, not a 404**:
  `list_collections` returns an empty vec when the namespace does not resolve
  (`crates/loams-query/src/service.rs:564-567`). An empty collection list is
  therefore ambiguous, and this adapter does not claim to have distinguished it.

## Shared HTTP client

Everything mechanical comes from `@loams-core/http`: `UpstreamClient`,
`UpstreamAuth`, `UpstreamError`, `buildQuery`, `loggerFrom`. This package does
not reimplement HTTP, auth headers, or pagination. With no token configured the
auth is `{kind: "none"}`; with one it is `{kind: "bearer"}`.

Unlike the Zulip adapter, this one does **not** wrap `fetch` to observe response
headers. The shared client exposes no header hook, and the only headers Loams
returns on a read — `Loams-Consistency-Token` (`mod.rs:57-59`) and
`Retry-After` on a 503 (`api/errors.rs:75-77`) — are not needed to render a
dashboard. The consistency token is read from the response body, which every read
endpoint carries explicitly as `read_token`.

## Verify

```sh
npx tsc -p core/engine/tsconfig.json
npx vitest run core/engine
grep -rn "console\." core/engine/src   # must be empty
```
