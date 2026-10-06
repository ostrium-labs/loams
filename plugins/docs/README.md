# Documentation

Guides for running, extending and trusting this plugin host. Everything here is
written against the code in this repository; where something is uncertain it says
so rather than guessing.

This is the Cordis plugin host, console, agent bus, A2A endpoint and auth layer
in the [Loams](https://github.com/ostrium-labs/loams) monorepo's `plugins/`
workspace. This workspace contains no Rust — see the [README](../README.md) for what that
means in practice.

| Guide                                   | What it covers                                                                              |
| --------------------------------------- | ------------------------------------------------------------------------------------------- |
| [Monorepo integration](monorepo.md) | Nx validation, isolated toolchain versions, pnpm dependencies, and protobuf lint/generation/drift |
| [Getting started](getting-started.md)   | Install, configure, run, first dashboard, first plugin enablement, troubleshooting          |
| [Writing a plugin](plugin-authoring.md) | `PluginManifest`, `PluginLoader`, the loader lifecycle, scopes, UI pages, agent skills, A2A |
| [Auth setup](auth-setup.md)             | Authentik OIDC, including the configuration mistakes that fail silently                     |
| [Security model](security.md)           | What is enforced where, and the traps that are easy to get wrong                            |
| [A2A endpoint](a2a.md)                  | Agent cards, `message:send`, Task/Artifact mapping, and the documented deviations           |
| [Upstream adapters](upstreams.md)       | One subsection per upstream: base URL, auth, env vars, API quirks, rate limits              |

Contributing and local validation commands are in
[CONTRIBUTING.md](../CONTRIBUTING.md).

## Where the code is

| Path                              | What lives there                                                                                                     |
| --------------------------------- | -------------------------------------------------------------------------------------------------------------------- |
| `packages/core`                   | The plugin platform: `types.ts`, `registry.ts`, `host.ts`, `bus.ts`, `router.ts`, `a2a.ts`, `api.ts`, `auth/`, `ui/` |
| `packages/types`                  | Shared zod schemas used across packages                                                                              |
| `packages/plugin-upstream-http`   | The shared HTTP client every adapter builds on (auth assembly, query encoding, timeouts, error normalisation)        |
| `packages/plugin-*-adapter`       | One read-only adapter per upstream, each exporting a manifest and usually a loader                                   |
| `packages/dashboard-ui`           | The console SPA                                                                                                      |
| `packages/plugin-*` (non-adapter) | Feature services: store, data, flint, render, dashboard-spec, agent-tools                                            |
| `packages/bi-rpc`                 | ConnectRPC bindings generated from `proto/bi/v1/` — see the note in the README about the historical name             |
| `apps/server`                     | Boot order, the mock upstream, the plugin catalog, the HTTP server                                                   |

| `proto/bi/v1/`                    | The `bi.v1` protobuf contract. The namespace predates the `@loams-plugins` scope and is deliberately not renamed.    |

## Conventions worth knowing before you read anything

- **Adapters are read-only.** Every adapter issues reads. This is load-bearing
  where the upstream derives scopes from the HTTP method — Forgejo does.
- **Registering is not enabling.** `PluginRegistry.register` puts a plugin in the
  catalog. `PluginHost` is what loads it, and only if it resolves to enabled. No
  adapter service is attached on the root context at boot, precisely so that
  "off" means off.
- **The agent bus is the only transport.** A2A translates onto the bus rather
  than dispatching separately, so there is one answer to "is this agent loaded?"
- **`requiredScopes` is enforced per request**, never at enable time. This is the
  one design decision worth reading [security.md](security.md) for.
- **A missing adapter is a supported configuration, not an error.** Its state is
  `unloaded` with a message naming the variables, never `error`.
- **The UI is mid-migration.** Describe routes and pages, not styling internals.
  Any documentation of Tailwind configuration or CSS variables here would be
  stale within a week.
