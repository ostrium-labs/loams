# Upstream adapters

Ten adapters ship in this repository. Each one is a **read-only client for a
separate HTTP service**, registered as a Cordis plugin, loaded and unloaded by
`PluginHost`.

Nothing here is vendored. Every adapter talks to its upstream's HTTP API as a
client, so no upstream's licence obligations are inherited. See [NOTICE](../NOTICE).

## The shared client

Everything mechanical comes from `@loams-plugins/plugin-upstream-http`:

| export           | what it owns                                                                                   |
| ---------------- | ---------------------------------------------------------------------------------------------- |
| `UpstreamClient` | `fetch`, timeouts (default 30s), error normalisation, the 401/403 re-auth hook                 |
| `UpstreamAuth`   | auth-header assembly — discriminated by _which product wants which shape_, not by HTTP concept |
| `UpstreamError`  | one error type across nine products with nine error bodies                                     |
| `buildQuery`     | query-string array encoding (repeated, never `join(',')`)                                      |
| `loggerFrom`     | binds the client's logger to a Cordis context                                                  |

It is deliberately **not** a Cordis service. A service is a singleton per
container, and each adapter needs its own client with a different base URL and
credentials; making it a service would force all of them to share one
misconfigured client. Adapters instantiate it directly.

`request` also accepts an `onResponse` observer and hands out the live `Response`
before the body is read, because several upstreams put load-bearing data in
headers — Zulip's throttling budget, Forgejo's totals, all of GlitchTip's
pagination.

## The table

| id              | upstream        | required env                                                      | optional env                                                                                    | auto-enables | loader in package?     |
| --------------- | --------------- | ----------------------------------------------------------------- | ----------------------------------------------------------------------------------------------- | ------------ | ---------------------- |
| `control-plane` | Apache Superset | — (`alwaysOn`)                                                    | `SUPERSET_URL`, `SUPERSET_USER`, `SUPERSET_PASS`                                                | n/a          | n/a — attached at boot |
| `zulip`         | Zulip           | `ZULIP_URL`, `ZULIP_EMAIL`, `ZULIP_API_KEY`                       | `ZULIP_TIMEOUT_MS`, `ZULIP_RATE_LIMIT_FLOOR`, `ZULIP_MAX_PAGES`                                 | yes          | yes                    |
| `forgejo`       | Forgejo         | `FORGEJO_URL`, `FORGEJO_TOKEN`                                    | `FORGEJO_TIMEOUT_MS`, `FORGEJO_CONCURRENCY`, `FORGEJO_CACHE_TTL_MS`, `FORGEJO_LIMIT`            | yes          | yes                    |
| `matomo`        | Matomo 5        | `MATOMO_URL`, `MATOMO_API_TOKEN`                                  | `MATOMO_TIMEOUT_MS`, `MATOMO_DEFAULT_ROW_LIMIT`, `MATOMO_DEFAULT_PERIOD`, `MATOMO_DEFAULT_DATE` | yes          | yes                    |
| `itsaplan`      | It's a Plan     | `ITSAPLAN_URL`, `ITSAPLAN_API_KEY`                                | `ITSAPLAN_TIMEOUT_MS`                                                                           | yes          | yes                    |
| `loams`         | Loams           | `LOAMS_URL`                                                       | `LOAMS_NAMESPACE`, `LOAMS_TOKEN`, `LOAMS_ALLOW_SQL`, `LOAMS_TIMEOUT_MS`                         | yes          | yes                    |
| `langfuse`      | Langfuse        | `LANGFUSE_URL`, `LANGFUSE_PUBLIC_KEY`, `LANGFUSE_SECRET_KEY`      | `LANGFUSE_TIMEOUT_MS`                                                                           | **no**       | **no**                 |
| `openpanel`     | OpenPanel       | `OPENPANEL_URL`, `OPENPANEL_CLIENT_ID`, `OPENPANEL_CLIENT_SECRET` | `OPENPANEL_API_PREFIX`, `OPENPANEL_FUNNEL_STEP_ENCODING`, `OPENPANEL_TIMEOUT_MS`                | **no**       | **no**                 |
| `glitchtip`     | GlitchTip       | `GLITCHTIP_URL`, `GLITCHTIP_TOKEN`                                | `GLITCHTIP_TIMEOUT_MS`                                                                          | **no**       | **no**                 |

The catalog lives in `apps/server/src/plugin-catalog.ts`; that file is the single
place the auto-enable policy exists.

### Why three of them never auto-enable

`langfuse`, `openpanel` and `glitchtip` export a manifest and a service class but
**no `PluginLoader`**. They are registered and toggleable, and attaching the
service keeps enable/disable real for them — but they are never auto-enabled,
because their manifests declare agent skills that have no handlers. Advertising
an agent that cannot answer is worse than advertising none. The server logs a
warning naming the missing export at boot.

### Auto-enable policy

An adapter is enabled at boot **iff** its required variables are present in the
environment **and** its manifest opts in with `defaultEnabled`. Both halves:

- A fresh checkout has no upstream credentials. Auto-enabling on
  `defaultEnabled` alone would boot several adapters pointed at nothing. Their
  constructors only build a client, so they would not throw — they would answer
  every agent skill with a connection error, and the console would look broken
  rather than unconfigured.
- Somebody who has exported `ZULIP_API_KEY` has already declared the intent to
  use that adapter, so waiting for a second toggle is friction with no
  information in it.

An explicit console toggle always wins. `defaultEnabled` is consulted only for a
plugin that has never been toggled.

The environment is resolved **once, at boot**. Exporting a variable into a
running process does nothing.

### Not-configured is not an error

A plugin missing configuration is registered, listed, left off, and carries a
message naming the variables. Its `state` stays `unloaded`, never `error`.

```
Zulip is not configured: set ZULIP_URL, ZULIP_EMAIL, ZULIP_API_KEY to enable it.
```

The refusal is implemented as a `refusingLoader` — a loader whose `attach`
throws — because `attach` is the first hook the host runs after the service. An
adapter with no configuration then fails with _"set ZULIP_URL…"_ rather than with
whatever the service constructor does when handed `undefined`. It only ever runs
if somebody explicitly enables the plugin, which is exactly when that answer is
worth an `error`.

---

## Per upstream

### Control plane (`alwaysOn`, `order: 1`)

The plugin id is `control-plane` and the client is
`@loams-plugins/plugin-control-plane`. The upstream it speaks is **Apache
Superset** over its REST API, so the env prefix stays `SUPERSET_` and the base
URL is still `/api/v1` — the id is this project's naming, the wire is Superset's.

Supplied by the repository as a **mock** MSW server on `SUPERSET_PORT` (default
`8088`) for local development; point `SUPERSET_URL` at a real deployment.

It is `alwaysOn` for the same reason the dashboard is, and by the mechanism
`host.ts` documents: the service is attached on the **root context at boot** (the
original `/api/*` routes read `ctx.controlPlane` directly), so a console toggle
could not unload it. A toggle that reports success while nothing changes is
worse than no toggle, so the toggle is disabled instead.

- Base: `/api/v1`. Login is `POST /api/v1/security/login`, then a CSRF token from
  `/api/v1/security/csrf_token/`.
- Guest tokens via `POST /api/v1/security/guest_token/`.
- Not configurable in the adapter catalog — `SUPERSET_*` is read by
  `apps/server/src/index.ts` and passed in at boot.
- Reads `POST /api/v1/chart/data` and `GET /api/v1/dataset/`.
- The mock is started unconditionally and occupies `SUPERSET_PORT` whether you
  use it or not.

### Zulip

- Prefix is `/api/v1`, **never** `/json/`. The `/json/` prefix is
  cookie-plus-CSRF and is not an API surface for a token client.
- Auth is HTTP Basic: `email` as the username, `api_key` as the password.
- Rate limit window is **60 s**. `X-RateLimit-Limit` / `-Remaining` / `-Reset`
  are read off every response; `ZULIP_RATE_LIMIT_FLOOR` sets how much budget must
  remain before the client waits out the reset (it never sleeps past the window).
- Stream ids are resolved with `GET /get_stream_id`, cached.
- `listMessages` across several streams is sequential on purpose: it is the
  largest source of rate-limit pressure, and fanning out would spend the whole
  budget on the first page.

### Forgejo

- Prefix is `/api/v1`, **not** `/api/forgejo/v1`. The latter exists in 9.0.2 but
  carries only a root and a version document, and its `Retry-After` handling
  differs. No throttle support is present, and the actual protection is a local
  concurrency limit.
- Auth is `Authorization: token <token>` — not Bearer, not Basic, so the shared
  client passes a raw `authorization-raw` header.
- **Forgejo clamps `limit` to 50 silently.** `limit=1000` returns 50 items with
  no error and no indication. `FORGEJO_LIMIT` raises the local ceiling but a
  larger value will still be clamped upstream, so responses are walked via the
  `Link` header.
- Forgejo does not rate limit, so `FORGEJO_CONCURRENCY` is the only brake on the
  adapter.
- `FORGEJO_CACHE_TTL_MS` bounds a small response cache for the reference-list
  endpoints that otherwise dominate the request budget.

### Langfuse

- Base for every request is host + `/api/public`.
- `GET /api/public/health` → `{ version, status }` — the cheapest liveness
  answer.
- Observations paginate with `limit` 1..1000 (default 50); traces with `limit`
  1..100 (default 50) — above 100 is a 400, not a clamp.
- Cursor pagination walks to `maxPages` and then **stops and says so** in a log
  line, rather than pretending the result is complete.

### OpenPanel

- `OPENPANEL_API_PREFIX` defaults to `/api`; set it to `""` for the
  direct-container address.
- Auth is `client_id` / `client_secret`; the adapter exchanges them for a bearer
  token and refreshes it.
- Rate limits are documented per endpoint group and applied client-side:
  `export` 100/10 s, `insights` 100/10 s, `manage` 20/10 s.
- `page` is 1-based, `limit` is clamped to 1..1000 (default 50).
- `OPENPANEL_FUNNEL_STEP_ENCODING` selects `repeated` or `csv` for funnel step
  parameters; it exists because deployments behind different proxies accept
  different encodings.
- `/healthcheck` sits under the `/api` prefix only on some deployment shapes, so
  the adapter probes both and treats absence as "unreachable", not "broken".

### GlitchTip

- Base is `/api/0/` (the leading zero is part of the path).
- Auth is a bearer token.
- The response shape differs per endpoint: the top-level list endpoints take
  `limit` on the query string, but `GET /api/0/organizations/{org}/issues/`
  wraps it in a **cursor** and uses `nextCursor` rather than page numbers. The
  adapter does not paper over that — the cursor is walked explicitly.
- Cursor walks stop at `maxPages` (default 50) and log that the result is
  truncated.
- `GET /api/0/users/` is read as **STAFF** users only; that is what the upstream
  endpoint returns.

### Matomo

- **There is no `/api/v1`.** `MATOMO_ENDPOINT` is `/index.php` and every request
  is a form POST or a GET with query parameters.
- Auth is a query parameter, `token_auth` (or `api_key`), because that is Matomo's
  design rather than an expedient.
- `filter_limit` is always sent explicitly, because the upstream default is not
  optional.
- `MATOMO_DEFAULT_PERIOD` is only forwarded for the literals the adapter
  declares (`day`, `week`, `month`, `year`, `range`); an unrecognised value is
  dropped so the adapter keeps its own default rather than sending a period
  Matomo will reject.
- `listSites` is the one method that reads `filter_limit` / `filter_offset`
  **directly** and disables the generic limit/sort filters, so both are sent for
  it.
- `token_auth` does not expire, so there is no refresh path.

### It's a Plan

- **There is no `/api/v1`.** The app takes `API_URL` and appends the versioned
  path itself.
- Auth is an API key created with `POST /api/auth/api-key/create` while signed
  in; the value starts with `itp_`.
- The documented per-key rate limit is exposed on the service
  (`rateLimitPerSecond`) so a caller can budget a fan-out.
- `/analytics/activity` paginates by cursor and defaults `limit` to 25, clamped
  1..100. Walks stop at `maxPages` (default 20) — an unbounded cursor walk over
  a live activity feed would never terminate.
- `/analytics/agent-runs` returns a **bare array**, not the envelope the other
  endpoints use.
- Other endpoints `limit` 1..500 (default 50) and **truncate**: a response of
  exactly `limit` rows is not proof there are no more.

### Loams

The platform this console sits alongside. Full detail in
[`packages/plugin-loams-adapter/README.md`](../packages/plugin-loams-adapter/README.md),
which cites the upstream source file and line for every field. Summary:

- Plain REST, not Connect-RPC. Loams' REST table is the documented read surface
  and the wider one; its proto tree is the unstable half (`loams.live.v1` and the
  streaming gRPC API are in progress, `loams.stream.v1` is opt-in).
- `/health` and `/ready` are **unversioned**.
- **Loams has no authentication today.** Auth and tenancy are planned upstream.
  The adapter therefore sends no `Authorization` header by default, and its
  optional `LOAMS_TOKEN` exists for a gateway or proxy-fronted deployment. No
  auth scheme is invented.
- `LOAMS_NAMESPACE` is optional in the catalog but effectively required: the
  server has `POST /v1/namespaces` (which **creates**) and **no** `GET`
  counterpart, so there is no namespace discovery anywhere. An unset namespace
  throws with a message naming the reason and the fix; the adapter never guesses.
- `LOAMS_ALLOW_SQL` gates `POST /v1/namespaces/{ns}/sql`, which takes raw SQL.
  It is off unless explicitly `true`, and there is **no agent skill** for it, so
  editing config is the only way in. The statement is passed through verbatim —
  this client does not sanitise, parse, or `SELECT`-prefix it, because the
  read-only enforcement is real and server-side (`run_read_only` verifies the
  logical plan). A client-side string check would be theatre layered on a real
  control.
- No code-execution surface is reachable: `_assertSafe` throws on any path
  containing `live` or `deploy`, and `tests/loams-adapter.spec.ts` asserts that
  no request the adapter issues ever targets one.
- `search` defaults `limit` to 10; `scrollDocuments` to 100.
- `pk` and `id` are the same value under different names on different
  endpoints.
- `consistency` defaults to `"strong"`.
- An unknown namespace yields `[]`, not a 404, so an empty collection list is
  ambiguous. The adapter does not claim to have distinguished that case.

---

## Conventions worth knowing before you read anything

- **Adapters are read-only.** Every one of them issues reads only, and the ones
  whose upstreams make that a claim worth arguing for say so at the top of their
  file. Forgejo derives the required scope from the HTTP method, so a `GET`-only
  adapter can never ask for a write scope; Superset and Matomo similarly. Three
  endpoints issue `POST`, and each is read-shaped rather than a write:
  Superset's login / CSRF / guest-token calls, Matomo's report reads (the
  Reporting API is `GET` or form `POST` — both are reads), and It's a Plan's
  `agent-runs`, which takes a body and returns a bare array. Loams additionally
  routes every path through `_assertSafe`, which throws on any path containing
  `live` or `deploy`.
- **Registering is not enabling.** `PluginRegistry.register` puts a plugin in
  the catalog. `PluginHost` loads it, and only if it resolves to enabled. No
  adapter service is attached on the root context at boot, precisely so that
  "off" means off.
- **Per-adapter quirks belong in the adapter.** The shared client owns the
  mechanics; the differences are the adapter's problem. If you find yourself
  reimplementing header assembly or pagination encoding inside an adapter, it
  belongs upstream.
