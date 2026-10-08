# Getting started

This is the Cordis plugin host, console, agent bus, A2A endpoint and auth layer
for [Loams](https://github.com/ostrium-labs/loams). This is the monorepo's
`plugins/` workspace; nothing in this workspace is Rust and nothing here
stores your data. What you get is a console you can load plugins into, a set of
read-only upstream adapters, and an agent bus you can talk to over HTTP.

## Requirements

- Node.js 22 (this is the `@types/node` target in the root `package.json`). The
  repository does not currently declare an `engines` field, so your package
  manager will not enforce it for you.
- The pnpm version pinned by the Loams root `package.json`.

## Install

From the Loams monorepo root:

```sh
pnpm install
```

`plugins`, `plugins/packages/*`, and `plugins/apps/*` participate in the root
pnpm workspace. Package names retain the `@loams-plugins/` scope. There is no
plugin-local lockfile; the root lockfile controls the install. See
[monorepo.md](monorepo.md) for Nx and protobuf validation.

## Configure

Nothing is required. On a fresh checkout with no environment variables set, the
server boots, an MSW mock of Apache Superset starts alongside it, and a seeded
six-widget dashboard is written to the in-memory store. You get a working
dashboard with no upstream credentials at all — which is deliberate, because a
missing adapter is a supported configuration and not an error.

The variables the server reads:

| variable        | default                           | meaning                                                           |
| --------------- | --------------------------------- | ----------------------------------------------------------------- |
| `PORT`          | `3001`                            | HTTP API port                                                     |
| `SUPERSET_PORT` | `8088`                            | port the bundled mock Superset listens on                         |
| `SUPERSET_URL`  | `http://localhost:$SUPERSET_PORT` | Superset base URL                                                 |
| `SUPERSET_USER` | `admin`                           | Superset username                                                 |
| `SUPERSET_PASS` | `admin`                           | Superset password                                                 |
| `DATABASE_URL`  | `memory`                          | store connection string                                           |
| `ENABLE_MCP`    | unset                             | `true` additionally serves the MCP stdio server from this process |

To point at a real Superset, set `SUPERSET_URL`, `SUPERSET_USER` and
`SUPERSET_PASS`. The mock server is started unconditionally at boot and will
occupy `SUPERSET_PORT` regardless of whether you use it; change that port if it
collides with something.

Upstream adapters read their own variables — `ZULIP_URL`, `FORGEJO_TOKEN`,
`LANGFUSE_PUBLIC_KEY` and so on. The full list, per adapter, is in the README
table and in [upstreams.md](upstreams.md).

To set up login, see [auth-setup.md](auth-setup.md). Auth is off unless you set
`OIDC_ISSUER` and `OIDC_CLIENT_ID`.

## Run

Two processes, or one:

```sh
pnpm --filter @loams-plugins/root dev       # API server on http://localhost:3001
pnpm --filter @loams-plugins/root dev:ui    # dashboard SPA on http://localhost:5173
```

`dev:ui` delegates to the `@loams-core/dashboard-ui` workspace's own `dev`
script. The server's CORS allowlist already includes
`http://localhost:5173` and `http://127.0.0.1:5173`, so the SPA can talk to the
API from the default dev port without any extra configuration.

Other scripts, selected from the plugin root `package.json`:

```sh
pnpm --filter @loams-plugins/root build     # tsc -b tsconfig.json
pnpm --filter @loams-plugins/root test      # vp test --config vitest.config.ts
pnpm --filter @loams-plugins/root lint      # vp lint --config vite.config.ts
pnpm --filter @loams-plugins/root mcp       # ENABLE_MCP=true tsx apps/server/src/index.ts
```

If you run with `ENABLE_MCP=true`, this process is also an MCP **stdio** server.
stdout is reserved for JSON-RPC framing and every log line goes to stderr. A
single stray `console.log` will corrupt the transport — see
[CONTRIBUTING.md](../CONTRIBUTING.md).

## Your first dashboard

Boot seeds dashboard `e3b0c442-98fc-1c14-9afbf4c8996fb924`
("Executive Analytics & Performance") with five widgets. The seed is
idempotent in the way that matters: it re-seeds only when the stored dashboard is
absent, is a pre-1.0 placeholder, or has lost the `widget-revenue-trend` widget.

Useful endpoints:

| request                     | returns                                                          |
| --------------------------- | ---------------------------------------------------------------- |
| `GET /api/dashboards`       | every stored dashboard                                           |
| `GET /api/dashboards/:id`   | one dashboard spec                                               |
| `POST /api/dashboards`      | create a dashboard                                               |
| `PATCH /api/dashboards/:id` | JSON-Patch a spec, with `baseVersion` for optimistic concurrency |
| `GET /api/datasets`         | Superset datasets                                                |
| `POST /api/widgets/data`    | the rows behind one widget                                       |
| `POST /api/widgets/preview` | compiled ECharts option plus sample rows                         |
| `GET /api/themes`           | the theme catalogue                                              |

## Your first plugin enablement

1. Export the adapter's variables **before** starting the server, for example:

   ```sh
   export ZULIP_URL=https://chat.example.com
   export ZULIP_EMAIL=you@example.com
   export ZULIP_API_KEY=...
   pnpm --filter @loams-plugins/root dev
   ```

2. Open `/console`. Zulip should be loaded, because its required variables were
   present at boot. Every adapter you did not configure is listed as not
   configured with the variables it is waiting for.
3. Open `/plugins` and `/plugins/zulip`.

The equivalent over HTTP:

```sh
curl localhost:3001/api/plugins | jq '.plugins[] | {id, enabled, state}'

curl -X POST localhost:3001/api/plugins/zulip/enable
curl -X POST localhost:3001/api/plugins/zulip/disable
```

Toggling off is observable from outside: after disabling, the plugin's routes
404, and it disappears from `/.well-known/agent-card.json`.

If auth is configured, both toggle endpoints require the admin scope
(`ADMIN_SCOPE`, default `bi:admin`) and return 403 without it. If auth is _not_
configured they return 403 with `admin_unavailable`, because no principal can
then be proven to hold the admin scope.

## Your first agent call

Every loaded plugin that declares an `agent` block publishes its skills over
A2A. The aggregate card lists **loaded** plugins only:

```sh
curl -s localhost:3001/.well-known/agent-card.json | jq '.skills[].id'
```

Call one:

```sh
curl -s -X POST localhost:3001/a2a/v1/message:send \
  -H 'content-type: application/json' \
  -d '{"agent":"dashboard","skill":"listDashboards"}' | jq '.status.state'
```

Skill ids on the aggregate card are namespaced `<pluginId>.<skillId>`; per-plugin
cards at `/.well-known/agent-card/<id>` use the bare id. The wire shape and the
documented deviations from strict A2A v1.0 are in [a2a.md](a2a.md).

## Troubleshooting

**The console is a wall of "not configured".** Expected on a fresh checkout.
Each entry names the exact variables that would enable it.

**A plugin that I configured is still not configured.** The catalog resolves the
environment once, at boot. Exporting variables into an already-running process
does nothing; restart it.

**`pnpm --filter @loams-plugins/root mcp` produces protocol errors on stdout.** Something wrote a
non-JSON-RPC line to stdout. With `ENABLE_MCP=true` the server installs a stderr
log exporter instead of the console one precisely to avoid this — a bare
`console.log` in any loaded plugin will break it.

**Stale build artifacts produce phantom `TS2339` / `TS6305` errors.** Stale
`lib/*.d.ts` files from a previous build make composite projects resolve
declarations instead of sources. Delete `packages/*/lib`, `packages/*/dist` and
the `tsconfig.tsbuildinfo` files before `tsc -b`. The exact sequence is in
[CONTRIBUTING.md](../CONTRIBUTING.md).

**`vp check` reports an error I did not introduce.** Run `vp env doctor` and
include the output when asking for help — that is what the tool's own
review checklist asks for.

**A route 404s after I disabled a plugin.** That is the contract, not a bug. See
[plugin-authoring.md](plugin-authoring.md) for why the fall-through is the point.

**An adapter is listed but says "not configured".** Expected on a fresh checkout,
and the message names the exact variables. The environment is read **once, at
boot** — exporting a variable into a running process does nothing; restart it.
