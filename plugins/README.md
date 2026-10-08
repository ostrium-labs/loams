![Loams — Your data. Your bucket.](../docs/assets/loams-banner.svg)

# loams-plugins

The Cordis-based **plugin host, console, agent bus, A2A endpoint and auth layer
for [Loams](https://github.com/ostrium-labs/loams)**.

Loams is a Rust AI-native data platform, Apache-2.0 and open-core. This
`plugins/` workspace is its imported TypeScript layer (the original
`loams-plugins` checkout is preserved):
the thing that loads plugins as Cordis fibers, lists and toggles them in a web
console, genuinely unloads them when you turn them off, routes every loaded
plugin's HTTP surface, publishes every loaded plugin's agent skills over A2A,
and decides who is allowed to do any of it.

**Nothing in this repository is Rust.** There is no Rust engine, no data
warehouse, and no vendored Rust source. Loams' metering, billing and other
commercial APIs are proprietary to that project and are neither used nor
reachable from here; the only Loams surface this repository touches is the
adapter that calls a Loams deployment's public HTTP API, exactly as it calls
Zulip or Matomo.

A plugin here is a **manifest** plus a **loader**. The host attaches the
loader's service, registers its routes and subscribes its agent skills on
enable, and reverses all three on disable. That is the whole thesis: a plugin is
not a configuration flag.

## Status

Early and experimental. The plugin _host_, the agent bus, the scope enforcement
and the read-only adapter pattern are the settled part. The plugin platform's
public surface, the A2A layer and the web UI were all added very recently and
are still moving: manifest fields, REST payloads and the agent-card shape can
change between releases without a major-version bump. Pin an exact version if
you depend on them, and read the source before building against them.

## Architecture

Everything below is described from `core/host/src`.

### The manifest contract

A plugin declares itself with a `PluginManifest` (`types.ts`): `id`, `name`,
`description`, `version`, `uiPath`, plus optional `category`, `icon`, `alwaysOn`,
`order`, `defaultEnabled`, `requiredScopes`, `upstream` and an `agent` block
carrying the skills it advertises.

This matches the SF1 `PluginManifest` field for field, with one deliberate
addition: `requiredScopes`, described under [`requiredScopes`](#requiredscopes)
below. It is additive rather than a rename, so a host built against SF1 still
accepts every manifest this repository produces — but such a host has no
implementation for it, so a manifest that relies on it is only enforced where
`requiredScopes` is implemented. `PluginAgentSkill` matches SF1 exactly.

`PluginStatus` is the manifest plus live state (`enabled`, `state`, `error`,
`missingScopes`, `changedAt`) and is what `GET /api/plugins` returns. The two
types are deliberately separate so a UI cannot write `state` back into the
manifest.

### The registry

`PluginRegistry` (`registry.ts`) owns declarative state only: which plugins
exist, whether each is enabled, and whether one is in an error state. It loads
nothing. The enabled flag is persisted through the store so toggles survive a
restart; `defaultEnabled` is consulted only for a plugin that has never been
toggled. Console ordering is `alwaysOn` first, then `order`, then name.

`requiredScopes` produces a fourth state, `blocked`, which is **derived per
request** rather than stored. It is a property of the (plugin, session) pair, so
one under-privileged user's view must not bleed into everybody else's.

A plugin missing its configuration stays in the catalog, off, with a message
naming the variables that would configure it. Its state stays `unloaded`, never
`error`: a fresh checkout with no upstream credentials has broken nothing.

### The host: what "off" means

`PluginHost` (`host.ts`) is what makes enable/disable real. Loading a plugin
attaches its Cordis service, runs an optional free-form `attach` hook, registers
its routes, and subscribes its agent skills. Disabling reverses all four, in
reverse order:

1. routes are removed from the mutable route table,
2. bus subscriptions are dropped,
3. the Cordis service fiber is disposed,
4. any disposer returned by `attach` is called.

The observable proof is the 404: a disabled plugin's route is gone from the
table, so the request falls through to the server's catch-all. `host.spec.ts`
asserts exactly that, and also asserts that enable → disable → enable produces
one set of handlers rather than two. Every teardown resource is retained per
plugin and every step tolerates running twice.

A loader that throws marks its own plugin `error` and rolls back whatever it had
already done; boot continues for every other plugin.

### The agent bus

`AgentBus` (`bus.ts`) is the single internal transport. A message is an
`AgentMessage` with `from`, optional `to`, `messageId`, `skill`, `params`,
optional `text`/`data`, and a timestamp. Delivery is by `publish` (broadcast or
addressed) or `request` (addressed, awaited, with a default 10s timeout).
Addressing an agent with no subscribers raises `AgentNotLoadedError`, which is
how an unloaded plugin fails identically whether it is reached over REST, over
A2A or from another agent.

Delivery depth is tracked in an `AsyncLocalStorage` rather than on the message,
with a hop cap (default 8), so two broadcast agents that forward to each other
cannot loop forever.

### The A2A layer

`a2a.ts` is protocol translation only, layered on top of the bus — not a second
dispatch path. Every `message:send` becomes an `AgentMessage`, goes to the bus,
and the reply is translated back into a `Task` whose structured result rides in a
`DataPart` artifact. See [docs/a2a.md](docs/a2a.md) for the wire shape and for
the deliberate deviations from strict A2A v1.0 — including the two things this
implementation does **not** do (a JSON-RPC envelope, and card signing).

### `requiredScopes`

`requiredScopes` on a manifest is the authorization contract a plugin imposes.
It is enforced **at request time**, per request, by a guard the host wraps around
every route the plugin registers — and separately by the A2A layer before it
dispatches to a skill. Per-request rather than per-enable because a session
created before a plugin was deployed cannot have been granted that plugin's
scopes; an enable-time check would wave it straight through. Missing scopes come
back as `insufficient_scope` with the names listed.

The full model, including where admin gating and service tokens sit, is in
[docs/security.md](docs/security.md).

## The plugin catalog

Ten adapters are registered at boot. The first two are `alwaysOn`: the host loads
them, the registry refuses to disable them, and the console toggle is disabled.

| id          | reads                                                                | required env                                                      | always on                      |
| ----------- | -------------------------------------------------------------------- | ----------------------------------------------------------------- | ------------------------------ |
| `dashboard` | this console's own dashboards, widgets and their data                | —                                                                 | yes (pinned first, `order: 0`) |
| `bi`        | the BI backend over its REST API — datasets, charts, query results   | —                                                                 | yes (`order: 1`)               |
| `zulip`     | Zulip realm: channels, topics, message search, user metrics          | `ZULIP_URL`, `ZULIP_EMAIL`, `ZULIP_API_KEY`                       | no                             |
| `forgejo`   | Forgejo instance: repositories, issues, PRs, commits, contributors   | `FORGEJO_URL`, `FORGEJO_TOKEN`                                    | no                             |
| `langfuse`  | Langfuse LLM observability: observations, metrics, scores            | `LANGFUSE_URL`, `LANGFUSE_PUBLIC_KEY`, `LANGFUSE_SECRET_KEY`      | no                             |
| `openpanel` | OpenPanel product analytics: traffic, funnels, retention, events     | `OPENPANEL_URL`, `OPENPANEL_CLIENT_ID`, `OPENPANEL_CLIENT_SECRET` | no                             |
| `glitchtip` | GlitchTip error/performance monitoring: issues, events, releases     | `GLITCHTIP_URL`, `GLITCHTIP_TOKEN`                                | no                             |
| `matomo`    | Matomo 5 web analytics: visits, actions, referrers, devices, goals   | `MATOMO_URL`, `MATOMO_API_TOKEN`                                  | no                             |
| `itsaplan`  | It's a Plan delivery analytics: issues, throughput, burnup, activity | `ITSAPLAN_URL`, `ITSAPLAN_API_KEY`                                | no                             |
| `loams`     | a Loams deployment: collections, hybrid retrieval, documents, counts | `LOAMS_URL`                                                       | no                             |

An adapter is auto-enabled at boot **only if its required variables are present**
and its own manifest opts in with `defaultEnabled`. A fresh checkout therefore
boots with the credentials you actually supplied and nothing else. An explicit
console toggle always wins. `LOAMS_NAMESPACE` is optional but effectively
required for the Loams adapter to do anything, because the Loams API has no
route that lists namespaces.

Per-adapter details — base URL forms, auth mechanisms, required variables, API
quirks and rate limits — are in [docs/upstreams.md](docs/upstreams.md).

## Quickstart

Requires Node.js 22 and the pnpm version pinned by the Loams root. Install
from the monorepo root, then run plugin scripts with their package filter:

```sh
pnpm install
pnpm --filter @loams-plugins/root dev
```

See [docs/monorepo.md](docs/monorepo.md) for Nx validation, dependency/toolchain
isolation, and standalone protobuf generation/drift checks.

`pnpm --filter @loams-plugins/root dev` starts the server on `http://localhost:3001` and, on the same
process, an MSW mock upstream on `8088` so there is something to read on a fresh
checkout. A seeded six-widget dashboard is written to the store on first boot.

The console UI is a separate Vite dev server:

```sh
pnpm --filter @loams-plugins/root dev:ui   # dashboard-ui on http://localhost:5173
```

Server environment variables (all optional; defaults in brackets):

| variable                          | meaning                                                 |
| --------------------------------- | ------------------------------------------------------- |
| `PORT`                            | HTTP API port [`3001`]                                  |
| `LOAMS_BI_PORT`                   | port for the bundled mock BI backend [`8088`]           |
| `LOAMS_BI_URL`                    | BI backend base URL [`http://localhost:$LOAMS_BI_PORT`] |
| `LOAMS_BI_USER` / `LOAMS_BI_PASS` | BI backend credentials [`admin` / `admin`]              |
| `DATABASE_URL` | store connection string [`memory`] |
| `ENABLE_MCP` | `true` also serves the MCP stdio server on this process |
| `CORS_ALLOWED_ORIGINS` | comma-separated origin allowlist. `*` is rejected |

The old `SUPERSET_PORT`, `SUPERSET_URL`, `SUPERSET_USER` and `SUPERSET_PASS` names are
still read as deprecated fallbacks and log a one-time warning.

Adapters read their own variables — `ZULIP_URL`, `FORGEJO_TOKEN`,
`LANGFUSE_PUBLIC_KEY` and so on. The full list is in
[docs/upstreams.md](docs/upstreams.md).

Other scripts use `pnpm --filter @loams-plugins/root`: `build` (`tsc -b
 tsconfig.json`), `test` (`vp test --config vitest.config.ts`), `lint` (`vp lint
--config vite.config.ts`), and `mcp` (server with `ENABLE_MCP=true`). The dashboard
is built separately with `pnpm --filter @loams-core/dashboard-ui build`.

With `ENABLE_MCP=true` this process is also an MCP stdio server, so **stdout is
reserved for JSON-RPC framing and all logs go to stderr**. Writing a status line
to stdout corrupts the transport.

## Enabling plugins

The console is at `/console`. It lists every registered plugin with its load
state, its agent skills, and a toggle.

- Enabled plugins appear at `/plugins`, and each has its own page at
  `/plugins/:id`.
- A plugin whose environment variables are missing shows as **not configured**,
  with the missing variables named in the message — for example
  `Zulip is not configured: set ZULIP_URL, ZULIP_EMAIL, ZULIP_API_KEY to enable
it.` That is a supported state on a fresh checkout, not a failure.
- Toggling a plugin off fully unloads it: the service is disposed, its routes
  return 404, and its agent skills stop answering and disappear from the agent
  card.
- Toggling a plugin on for the first time re-runs the catalog resolution, so the
  variables you exported before starting the server are what get picked up.

## Package layout

| package                                    | what it is                                                                        |
| ------------------------------------------ | --------------------------------------------------------------------------------- |
| `@loams-plugins/root`                      | the workspace root; scripts and toolchain config                                  |
| `@loams-core/host`                         | the plugin platform: types, registry, host, bus, router, A2A, api, `auth/`, `ui/` |
| `@loams-core/types`                        | shared zod schemas (`PluginStatus`, theme specs)                                  |
| `@loams-core/bi`                           | the BI backend client (the bi backend speaks the Apache Superset REST API)        |
| `@loams-core/http`                         | the shared HTTP client every adapter builds on                                    |
| `@loams-core/store`                        | persistence for dashboards, widgets and plugin enable flags                       |
| `@loams-core/data`                         | widget data fetching                                                              |
| `@loams-core/charts`                       | widget → chart option compiler                                                    |
| `@loams-core/chart-specs`                  | chart specs and theme resolution                                                  |
| `@loams-core/dashboards`                   | dashboard document schema                                                         |
| `@loams-core/agent-tools`                  | agent tooling service                                                             |
| `@loams-plugins/plugin-<upstream>-adapter` | one read-only adapter per upstream                                                |
| `@loams-core/dashboard-ui`                 | the console SPA                                                                   |
| `@loams-core/bi-rpc`                       | ConnectRPC bindings for the `bi.v1` contract                                      |
| `@loams-plugins/server`                    | `apps/server`: boot order, catalog, mock upstream, HTTP server                    |

### About `bi-rpc` and the `bi.v1` namespace

`@loams-core/bi-rpc` holds the generated TypeScript for the `bi.v1` protobuf
contract in `proto/bi/v1/`. **The name is historical and it is deliberate.** The
scope rename was `@bi/*` → `@loams-plugins/*`; it did not rename the RPC
namespace, because `bi.v1` is a wire contract and renaming it would break every
client that has already compiled against it. Message names such as
`bi.v1.ListDashboardsRequest` are unchanged, and `core/bi-rpc/src/gen/bi/v1/`
is untouched. The remaining `bi:` string in this repository is the default
`ADMIN_SCOPE` value, which is overridable and documented in
[docs/auth-setup.md](docs/auth-setup.md).

## Authentication

Authentication is optional in development. With no `OIDC_ISSUER` and
`OIDC_CLIENT_ID` set, the server logs why and runs unauthenticated — every
request is an anonymous principal, no session can be created, and the console's
enable/disable endpoints refuse, so a dev-mode server cannot reconfigure itself.

Authentik is the expected identity provider. The full walkthrough, including the
several things that fail silently if you get them wrong, is in
[docs/auth-setup.md](docs/auth-setup.md). The security model is in
[docs/security.md](docs/security.md).

## Licence

Apache-2.0. See [LICENSE](LICENSE) for the full text and [NOTICE](NOTICE) for
required attribution — including the MIT-licensed UI primitives adapted from T3
Code. The upstreams this repository calls over HTTP are listed there too, with the
reason no copyleft obligation is inherited.

## Documentation

Guides live in [`docs/`](docs/README.md):

- [Getting started](docs/getting-started.md)
- [Writing a plugin](docs/plugin-authoring.md)
- [Auth setup (Authentik OIDC)](docs/auth-setup.md)
- [Security model](docs/security.md)
- [A2A endpoint](docs/a2a.md)
- [Upstream adapters](docs/upstreams.md)

The command CI runs, the project rules and how to add an adapter are in
[CONTRIBUTING.md](CONTRIBUTING.md). Security reports go to
[SECURITY.md](SECURITY.md), not to the public issue tracker.
