# Contributing

Thanks for looking. This repository is early; the plugin platform's public
surface, the A2A layer and the web UI can all change between releases without a
major-version bump.

## The gate

Run from the Loams monorepo root before opening a review:

```sh
pnpm install
pnpm exec nx run plugins:validate
pnpm exec nx run plugins:lint

# Standalone contract checks; Buf 1.73.0 is required.
pnpm --filter @loams-plugins/root proto:lint
pnpm --filter @loams-plugins/root proto:drift
```

`plugins:validate` runs the single composite build (including `bi-rpc` and the
server), the aggregate tests using `vitest.config.ts`, the separate dashboard
build/typecheck, and workspace invariant checks. Package Nx builds depend on the
same aggregate compilation; do not run recursive `tsc -b` processes in parallel.
Nx caches both compiler outputs and `tsconfig.tsbuildinfo` files. For a clean
compiler rebuild, run `pnpm --filter @loams-plugins/root exec tsc -b tsconfig.json
--clean`, then use Nx with `--skip-nx-cache`.

Formatting is available via `pnpm --filter @loams-plugins/root format`. Avoid
blanket `--fix` changes while migrating. From `plugins/`, also check for bare
console calls in shipped source:

```sh
grep -rn "console\." packages/*/src apps/*/src   # must be empty
```

Cordis services log through `this.ctx.logger.*`; with `ENABLE_MCP=true`, stdout
is JSON-RPC framing and a single `console.log` corrupts the transport. See
[docs/monorepo.md](docs/monorepo.md) for tooling isolation and proto prerequisites.

## Project rules

- **No bare `console.*` in `packages/*/src` or `apps/*/src`.** Cordis services
  use `this.ctx.logger.*`. Upstream HTTP clients take a `loggerFrom(ctx)`
  adapter. This is enforced by the sweep above, not by the linter.
- **`stdout` is reserved** when `ENABLE_MCP=true`. The server installs a stderr
  log exporter in that mode precisely so that a stray write cannot corrupt the
  JSON-RPC stream.
- **Adapters are read-only.** No adapter issues a mutating request. This is
  load-bearing for Forgejo, which derives the required scope from the HTTP
  method.
- **Registering is not enabling.** `PluginRegistry.register` puts a plugin in the
  catalog; `PluginHost` starts it. Do not attach an adapter service on the root
  context at boot — that is what would make "off" a lie.
- **Manifest and loader must agree on skill ids.** A `PluginAgentSkill.id` with no
  handler is an advertised agent that cannot answer.
- **Do not rename the `bi.v1` protobuf namespace.** It is the RPC contract, not a
  package name. The generated tree is `packages/bi-rpc/src/gen/bi/v1/`.
- **Vendored MIT files keep their headers.** The nine primitives under
  `packages/core/src/ui/primitives/` carry the attribution required by T3 Code's
  licence, and a test enforces it. If you touch them, keep the header.

## Writing a plugin

Read [docs/plugin-authoring.md](docs/plugin-authoring.md) first. The short
version: a plugin is a `PluginManifest` (declarative) plus a `PluginLoader`
(`service` / `attach` / `routes` / `skills`). All four loader hooks are optional
and all four are torn down on disable.

The properties you can rely on:

- **Idempotent.** Enable → disable → enable produces one set of handlers, not
  two.
- **Isolated.** A loader that throws marks _your_ plugin `error`, rolls back what
  it had already done, and lets boot continue for every other plugin.
- **Teardown failures are logged, not thrown.** A plugin that refuses to unload
  cleanly must not block the others.

The observable proof that unloading worked is the **404**: a disabled plugin's
route is gone from the table, so the request falls through to the catch-all. If
you can make "off" leave a trace, that is a bug.

## Adding an upstream adapter

1. Create `packages/plugin-<name>-adapter/` with a `package.json` named
   `@loams-plugins/plugin-<name>-adapter`.
2. Build on `UpstreamClient` from `@loams-plugins/plugin-upstream-http`. Do not
   reimplement header assembly, query encoding, timeouts or error normalisation.
3. Export **both** a `PluginManifest` and a `PluginLoader`. If you can only ship
   the manifest, set `autoEnable: false` in the catalog and the boot warning will
   tell you which export is missing.
4. Register it in `apps/server/src/plugin-catalog.ts` with its `requiredEnv` and
   a `buildConfig` that maps the environment onto the service's config type
   **field for field**, so an upstream rename surfaces as a type error rather
   than a silently ignored variable.
5. Add `docs/upstreams.md`: base URL form, auth mechanism, required variables,
   API quirks, rate limits.
6. Add a test that asserts every path the adapter builds is a read.

## Commit messages

Conventional-ish, imperative, and the subject says what changed:

```
fix(host): roll back route registration when attach throws
feat(loams-adapter): gate /sql behind LOAMS_ALLOW_SQL
docs(a2a): record the unsigned-card deviation
```

Sign your commits (`git commit -s`).

## Licence

Apache-2.0. See [LICENSE](LICENSE). Contributions are accepted under the same
licence. If you vendor third-party source, add it to [NOTICE](NOTICE) in the same
commit — an unrecorded vendored file is a licence bug.
