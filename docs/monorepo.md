# Loams Nx monorepo

Loams engine, console, SDKs, desktop, native mobile applications, and plugin services now live in one repository. Nx **23.3.0** orchestrates existing build tools; pnpm **11.27.1** owns the JavaScript workspace and root lockfile.

## Layout and installation

```text
crates/                       Loams engine; root Cargo workspace
proto/                        Canonical Loams contracts
web/                          Console, design system, web plugin host
sdks/                         Existing language SDKs
apps/desktop/native/          Independent Rust/GPUI desktop workspace
apps/mobile/android/          Native Kotlin/Compose Gradle build
apps/mobile/ios/              Swift/Xcode application and packages
apps/mobile/mock/             Go mock service
plugins/                      Plugin runtime, adapters, dashboard, BI contracts
tools/monorepo/                Migration provenance and structural checks
```

Use Node 24 (`.nvmrc`; the toolchain needs 24.11 or newer, design 79 D1775) and the pinned pnpm version. Published packages such as `@loams/client` keep Node 22 as their runtime floor, which CI tests in one cell:

```sh
pnpm install --frozen-lockfile
pnpm projects
pnpm check:monorepo
pnpm build
pnpm test
pnpm typecheck
```

The default root build/test/typecheck scripts cover JavaScript projects only, so Linux developers do not need Xcode or the Android SDK. Existing commands from `web/` remain supported; dependency installation discovers the root workspace. Do not recreate nested web/npm lockfiles.

## Native commands

Run from the repository root:

| Purpose | Command | Prerequisites |
| --- | --- | --- |
| Engine build | `pnpm nx run loams-engine:build` | Root Rust toolchain, protoc, engine prerequisites |
| Engine tests | `pnpm nx run loams-engine:test` | Existing engine test prerequisites |
| Desktop build | `pnpm build:desktop` | Desktop Rust toolchain and platform GPUI dependencies |
| Desktop reduced check | `pnpm nx run loams-desktop:check-minimal` | Linux GPUI development libraries; browser excluded |
| Desktop focused tests | `pnpm nx run loams-desktop:test-focused` | Desktop Rust toolchain |
| Desktop provenance/identity checks | `pnpm nx run loams-desktop:verify-import` | Python 3.11+; no source sibling required |
| Android build | `pnpm build:android` | JDK 17+, Android SDK |
| Android unit tests | `pnpm nx run mobile-android:test` | JDK and Android SDK |
| Mobile conformance | `pnpm nx run mobile-android:conformance` | Go, JDK; starts/stops its own local mock |
| iOS build/test | `pnpm build:ios`; `pnpm nx run mobile-ios:test` | macOS, Xcode, XcodeGen, simulator |
| Mock checks | `pnpm nx run-many -p mobile-mock -t vet,test,build` | Go 1.25+, supported C toolchain for race tests |
| Plugin checks | `pnpm nx run plugins:validate` | Installed JavaScript dependencies |
| Canonical contract drift | `pnpm nx run loams-contracts:drift` | Installed JavaScript dependencies, Python |
| Mobile contract drift | `pnpm nx run mobile-contracts:drift` | Buf; remote generation requires network |
| BI contract drift | `pnpm nx run plugins:proto-drift` | Python, Buf, installed JavaScript dependencies |

Interactive development targets are noncacheable. Native builds remain separate Cargo/Gradle/Xcode/Go commands. The desktop workspace is intentionally **not** a member of the engine's Cargo workspace: they have different dependency sets, profiles, licenses, and toolchains. Core `loams-link` and desktop `loams-desktop-link` are distinct libraries.

Linux's normal desktop build includes the native browser and needs WebKitGTK 4.1 and JSON-GLib development libraries, in addition to GPUI libraries. A successful `check-minimal` is not a validation of the browser-enabled application or a linked GUI build.

## Dependency graph and caching

Nx discovers the JS, mobile and native projects, including existing package script targets, and one project per crate of the root and `fabric/` Cargo workspaces (tag `cargo-crate`, from `tools/nx-cargo`; see [the Nx Cloud guide](guides/nx-cloud.md#rust)). Use `pnpm nx graph` locally to inspect them. `pnpm affected --base=<base-revision> --head=HEAD` runs affected checks; CI supplies explicit refs and platform scheduling.

- TypeScript project/package dependencies retain their existing boundaries.
- Plugins' composite compiler runs once through a shared aggregate target, avoiding concurrent writes from recursive `tsc -b` tasks.
- Native projects explicitly declare contract/fixture dependencies but do not automatically schedule every platform's build.
- JS lock/config files and runtime versions participate in cache keys.
- SDK conformance tests remain noncacheable because they produce required coverage reports outside their package directory.
- Signing, installation, services, generation/drift, releases, and most native tasks are noncacheable.
- `nx.json` names the Nx Cloud workspace (`nxCloudId`) for the remote cache; CI connects only where [the Nx Cloud guide](guides/nx-cloud.md) says, and never for a release. Set `NX_NO_CLOUD=true` locally to run without it.

**Vite+ is the JavaScript toolchain** (design 79 D1772, plan VV1, owner direction 2026-10-23 D1770). Nx stays the orchestrator; inside each target, `vp` does the work:
- One exact `vite-plus` pin lives in the root `pnpm-workspace.yaml` catalog, with `vite` aliased to the matching `@voidzero-dev/vite-plus-core` and `vitest` at the version `vite-plus` depends on. Packages name all three as `"catalog:"`; never pin them per package.
- `peerDependencyRules.allowedVersions.vite` names the same version as the pin, because `vite-plus-core` identifies as its own version and Vite plugins' `vite: ^8` peer ranges would otherwise fail. A bump changes both lines in one PR, and only to a release at least 14 days old.
- Test files import from `vite-plus/test`, and configs (`vite.config.ts`) from `vite-plus`. Scripts call `vp dev`, `vp build`, `vp preview` and `vp test run`: with `vite` aliased there is no `vite` binary.
- Lint and format are `vp check` (Oxlint and Oxfmt; design 79 D1773), configured in the `lint` and `fmt` sections of `web/vite.config.ts` and `agents/factory/vite.config.ts`. The rules are VV0 Task 1's Biome parity table ([vv0-report.md](plans/vv0-report.md)); suppressions are `// oxlint-disable-next-line <rule> -- <reason>`. `pnpm format` (`vp fmt`) rewrites. Formatting-only commits go in `.git-blame-ignore-revs`. Biome and ESLint are gone.
- The desktop is the one exception until it moves to electron-vite 6.0.0 (from 2026-10-28): it keeps Vite 7.3.7 and Vitest 5.0.3 of its own.

MSW's nonessential postinstall is explicitly disabled.

## Desktop fork cleanup and policy

The shipping executable and internal packages are Loams Desktop-branded. Product presentation, application identity, service/data names, installer paths, and bot executable discovery are coordinated. Unused inherited mobile/client/text components and upstream release/deployment automation were omitted. Original repository files were not changed.

Self-updating and upstream feed installation are disabled. Optional remote WorkOS/edge protocols remain internal feature dependencies, with an invalid default endpoint; they are **not** automatically replaced by Loams Connect or Loams' identity provider (Zitadel, design 76). No upstream cloud deployment is activated. Browser, terminals, sessions, dictation, and provider adapters remain because they are wired into functioning desktop features, not merely branding.

Pinned GPUI git dependency URLs, required copyright attribution, synthetic protocol fixtures, and documented persisted-format compatibility tokens may still mention the upstream project. Removing those indiscriminately would break functionality or attribution. See [desktop policy](../apps/desktop/native/LOAMS.md) and [validation](../apps/desktop/VALIDATION.md).

The desktop has a distinct installation/data identity. Existing upstream user data is not silently moved or overwritten. Legal notices remain scoped under `apps/desktop/native/`; the root Apache license does not relicense inherited MIT code.

## Contract ownership

`proto/` is authoritative for the Loams engine and existing web/SDK bindings. Canonical generated TypeScript is checked without rewriting committed files.

The imported mobile placeholder schemas and desktop pinned schemas remain **isolated compatibility snapshots**. Their protobuf packages overlap canonical packages, but their current clients/mock implementations were written against those snapshots. They are not treated as interchangeable or automatically regenerated from root contracts. Reconciliation is separate API integration work, not a prerequisite for this repository move. Mobile fixture/lock and generated-code checks remain enforced.

The plugins' `bi.v1` namespace is preserved, with reproducible generation and drift checks. Server handlers are not merged into browser-only bindings. Native security implementations are not replaced by shared TypeScript code.

## CI and releases

Root `.github/workflows/monorepo.yml` runs path-selected JS/plugin and native checks with separate Linux/macOS jobs and an always-running **Web and mobile required** gate. Desktop CI currently covers Linux minimal checks/focused tests; its comprehensive core tests are manual opt-in. The imported desktop's former macOS/Windows CI coverage is not yet restored. macOS CI runs mobile iOS tests, not desktop tests. Existing engine CI remains intact, updated to the root JavaScript lockfile. Nested mobile workflows are historical templates, not active workflows.

No inherited desktop publishing, updater, signing, or Cloudflare deployment workflow was enabled. Existing engine releases and console deployment retain their existing behavior. Branch protection should require the new summary in addition to existing required engine checks; repository settings cannot be changed by these local file edits.

## Import provenance and validation

This is a tracked-file snapshot import, not a destructive move or a history rewrite. Original sibling repositories and their histories are untouched and are not needed to build the monorepo. Revisions/counts are recorded in [`tools/monorepo/imports.json`](../tools/monorepo/imports.json); desktop records detailed source hashes separately. No branch or commit was created.

Validated on this Linux host:

- Root frozen dependency installation and Nx project discovery.
- Existing web/TypeScript SDK builds, typechecks, and tests across 16 runnable projects.
- Plugin aggregate and per-package tests: 46 files, **1,069 tests**; composite/dashboard builds and proto drift.
- Go mock vet/race tests/build; mobile conformance: **7 tests, zero skips**; mobile proto lint/drift and Python regressions.
- Desktop workspace/all-target minimal check, formatting, **144 focused tests**, executable/path checks, installer fixture tests, and source-independent import verification.
- Canonical app generated bindings: **7 files**, no drift.
- Root structural checks and regression tests.

Not validated here: native iOS, Android application build/lint requiring the SDK, browser-enabled desktop/linked GUI and macOS/Windows packaging, full desktop core suite, full engine suite, and hosted CI execution. Cold desktop compilation exceeded bounded timeouts before focused incremental checks passed. The existing SDK pin checker has 11 pre-existing non-npm pin issues; npm lock integrity verification and its regression tests pass with the root lock. See component migration/validation documents for exact commands.
