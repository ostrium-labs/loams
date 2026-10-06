# Plugin workspace in Loams

The migration copied the 257 files reported by `git ls-files -z` in the source
`loams-plugins` checkout using Python `shutil.copy2`. No untracked files,
`node_modules`, build products, or `.git` directory were imported. The source
checkout was not modified. The copied npm lockfile was subsequently removed.
Package names remain `@loams-plugins/*`; the RPC namespace remains `bi.v1`.

## Root integration contract

The Loams root owns pnpm, its lockfile, Nx (pinned to **23.2.1**), and workflows.
Its workspace must include `plugins`, `plugins/packages/*`, and `plugins/apps/*`.
All plugin-local package dependencies use `workspace:*`.

The plugin root deliberately has no npm `workspaces`, `devEngines`, or
`overrides`. pnpm overrides belong at the monorepo root and must be scoped:

- Plugin `vite-plus` stays **1.0.0**.
- Plugin `vite` stays **`npm:@voidzero-dev/vite-plus-core@1.0.0`** (declared in
  both the plugin root and dashboard manifests).
- Plugin `vitest` and `@vitest/coverage-v8` stay **5.0.1**.
- Scope transitive overrides to plugin consumers, for example
  `vite-plus@1.0.0>vite` and `vite-plus@1.0.0>vitest`, where needed. Do not use
  unqualified global `vite` / `vitest` overrides: Loams core keeps Vite **8.3.1**
  and Vitest **5.0.3**. Verify the resolved dependency graph after installing;
  consumer-specific peer/transitive selectors are the root owner's responsibility.

Core UI directly declares its React/query/router/primitives dependencies rather
than borrowing the dashboard's hoisted modules. The dashboard declares its
`@loams-plugins/core` dependency and directly imported `react-resizable` CSS.
The dev/build config aliases the core UI subpath to sibling source, with no
assumption about pnpm's symlink placement, and deduplicates shared React state.

## Validation

From the Loams root, after its install finishes:

```sh
pnpm exec nx run plugins:validate
pnpm exec nx run plugins:lint
pnpm exec nx run plugins-dashboard-ui:typecheck
```

`plugins:validate` runs the composite build, aggregate tests, separate dashboard
build, and `scripts/check-workspace.py`. Lint is separate so imported baseline
lint findings are not disguised as compiler failures. There is one Nx project
per existing package/app, named `plugins-<directory>`, plus `plugins`.

`plugins:build` is the sole Nx task running `tsc -b tsconfig.json`. It includes
`bi-rpc`, every other composite library, and the server. Individual library and
server Nx `build`/`typecheck` targets depend on that same task and then perform
no additional compilation. Nx deduplicates that dependency within a task graph,
so `nx run-many --target=build --projects='plugins-*'` cannot launch overlapping
TypeScript project-reference builds. The dashboard is non-composite/noEmit;
its build separately typechecks `tsconfig.json` and calls Vite+ with
`--config vite.config.ts`. Do not run recursive package build scripts in parallel
outside Nx: they intentionally delegate to the aggregate compiler.

Nx's aggregate inputs explicitly include `plugins/packages/**/*` and
`plugins/apps/**/*` because nested projects are not covered reliably by
`{projectRoot}/**/*`. Generated build directories, dependencies, and build-info
files are excluded from source inputs. Cache outputs include every composite
`lib`/`dist` directory and its explicitly configured `tsconfig.tsbuildinfo`.
Dashboard `dist` and coverage are declared separately.

The authoritative test command is:

```sh
pnpm --filter @loams-plugins/root test
```

It uses `vp test --config vitest.config.ts`, which includes both libraries and
server tests. Per-project Nx test targets append a file-path filter such as
`packages/types/tests` to that same aggregate command, not an unsupported
`--project` name. Packages without tests have no Nx test target.

Tool-independent checks can run before installation:

```sh
python3 plugins/scripts/check-workspace.py
```

## Protobuf tooling

Schemas and `bi.v1` are unchanged. Existing checked-in bindings permit baseline
build/test even if Buf or the generator is absent. Proto targets do **not** run
implicitly during `plugins:validate` or `plugins:build`.

Install Buf **1.73.0** locally, and install the workspace dependencies (which pin
`@bufbuild/protoc-gen-es` to **2.16.0**). Then run:

```sh
pnpm --filter @loams-plugins/root proto:lint
pnpm --filter @loams-plugins/root proto:gen
pnpm --filter @loams-plugins/root proto:drift
# Equivalent Nx targets: plugins:proto-lint / plugins:proto-gen / plugins:proto-drift
```

`buf.yaml` uses MINIMAL lint rules to check the imported contracts without
changing schema conventions. `buf.gen.yaml` documents the local TypeScript
plugin and `target=ts`. The Python driver validates tool versions, generates into
a temporary directory, formats with the pinned local Vite+ formatter and an
explicit temporary two-space Vite formatter config matching the imported bindings, and either
updates bindings or compares the full generated file set. Parent monorepo
formatting settings do not create false drift. Drift never edits the
checked-in bindings. There are no remote Buf plugins, credentials, or generation
network calls. Dependency installation still needs registry access. Proto Nx
targets are uncached so missing or incorrectly versioned local tools cannot be
hidden by a restored result.
