# Mobile import / integrated Nx CI

Imported 389 Git-tracked files using Python `git ls-files -z` and `shutil.copy2`
from the former mobile repository. Internal Android, iOS, mock, fixture, proto,
and script paths and executable modes are preserved. No source sibling is needed
for Nx discovery, generation, tests, or validation. The tracked generated clients
and Gradle wrapper JAR are intentional inputs, not build artifacts.

## Contracts remain local placeholders

`conformance/proto-ref.lock` retains its original upstream ref, tree hash, and
`placeholder` labels. Generation reads **apps/mobile/proto**, never root `proto`.
Do not run `sync-protos.sh` or reconcile contracts until the main agent compares
them with canonical contracts. The Go module identity remains
`github.com/ostrium-labs/loams-mobile/mock`.

`mobile-contracts:drift` checks the local proto lock, regenerates in an isolated
ignored directory, and compares all three generated file trees without rewriting
sources or fixtures. It works before the import is committed. Remote plugins on
buf.build require network access and can rate-limit anonymous requests; no keys
are required.

## Commands from the workspace root

Root pnpm/Nx setup is integrated. Prefix commands with `NX_DAEMON=false` for
bounded CI-style execution:

| Project | Commands | Requirements |
| --- | --- | --- |
| mobile-android | `pnpm exec nx run mobile-android:build`, `pnpm exec nx run mobile-android:test`, `pnpm exec nx run mobile-android:lint` | Java 17+, Android SDK; aggregate Gradle tasks |
| mobile-android | `pnpm exec nx run mobile-android:conformance` | Go, Java; owns a mock on free loopback port 8084; rejects skipped tests |
| mobile-android | `pnpm exec nx run mobile-android:install` | Android SDK and running emulator/device |
| mobile-ios | `pnpm exec nx run mobile-ios:generate`, `pnpm exec nx run mobile-ios:build`, `pnpm exec nx run mobile-ios:test` | macOS, Xcode, XcodeGen, available iPhone simulator |
| mobile-mock | `pnpm exec nx run mobile-mock:build`, `pnpm exec nx run mobile-mock:test`, `pnpm exec nx run mobile-mock:vet` | Go 1.25+; race tests require a supported C toolchain |
| mobile-mock | `pnpm exec nx run mobile-mock:serve` | Persistent local mock on loopback port 8084 |
| mobile-contracts | `pnpm exec nx run mobile-contracts:lint`, `pnpm exec nx run mobile-contracts:gen`, `pnpm exec nx run mobile-contracts:drift` | Buf; gen/drift use remote plugins |
| mobile-fixtures | No runnable targets | Shared fixtures and proto-ref lock inputs |

Every runnable target sets an explicit workspace-root-relative `cwd`. Android
unit tests exclude live conformance, which runs separately. iOS build/test
regenerate the Xcode project and build unsigned for a simulator; optionally set
`LOAMS_IOS_DESTINATION` to choose a destination.

Install, serve, conformance, native builds/tests, generation, and drift are
noncacheable. Only pure-Go build/vet and Buf lint are Nx-cacheable with explicit
file/tool/environment inputs. Go build/vet use `CGO_ENABLED=0`; build disables
VCS stamping. Race tests remain noncacheable. Conformance builds its own mock
in a temporary directory, cleans up its process/binary, and never overwrites
`mobile-mock:build`'s cached output.

## Dependency graph and task isolation

Explicit `implicitDependencies` track consumed contracts, shared fixture/lock
inputs, and Android's mock-backed network tests:

- `mobile-contracts` → `mobile-fixtures` (proto-ref lock).
- `mobile-mock` → `mobile-contracts`, `mobile-fixtures`.
- `mobile-android` → `mobile-contracts`, `mobile-fixtures`, `mobile-mock`.
- `mobile-ios` → `mobile-contracts`, `mobile-fixtures`.

`mobile-fixtures` is a targetless library, not another native app or generator.
These edges support affected detection; they do not schedule generation or
native toolchains. Android, iOS, and mock build targets explicitly set
`dependsOn: []`, overriding root `targetDefaults.build.dependsOn: ["^build"]`.
Native builds also override irrelevant root `dist`/`lib` outputs with `outputs: []`.
Actual Nx task graphs were checked: each build contains exactly its own task and
no prerequisite builds. No old unprefixed mobile project names or duplicate
mobile graph nodes remain.

## Active CI versus historical templates

**Root `.github/workflows/monorepo.yml` owns active mobile CI**, using the
`mobile-*` Nx projects. It handles Android build/test/lint, mock vet/race
 tests/build, conformance, macOS iOS tests, and mobile contract lint/drift.
Root configuration/workflows are owned by the main agent and were not edited.

The seven files in this directory's `.github/workflows/mobile-*.yml` are inactive
historical source-CI templates. Their headers state this explicitly. Do not copy
or activate them alongside `monorepo.yml`, which would duplicate jobs/statuses.
Their workflow-selection tests use local temporary fixtures, not a source sibling
or the active monorepo's workflow graph.

Nested Dependabot configuration is also inactive. Root policy and branch
protection changes, canonical contract reconciliation, and any desired Linux
Swift-core CI job remain decisions for the root owner.

## Integrated validation on Linux

All requested root Nx commands passed with `NX_DAEMON=false`:

- `pnpm exec nx run mobile-mock:vet --outputStyle=static --skip-nx-cache`
- `pnpm exec nx run mobile-mock:test --outputStyle=static` (race tests, fresh run)
- `pnpm exec nx run mobile-mock:build --outputStyle=static --skip-nx-cache`
- `pnpm exec nx run mobile-android:conformance --outputStyle=static`
  (7 tests, zero skips/failures/errors; Gradle reran all 10 tasks).
- `pnpm exec nx run mobile-contracts:lint --outputStyle=static --skip-nx-cache`
- `pnpm exec nx run mobile-contracts:drift --outputStyle=static`
  (all generated trees matched exactly).

Each command group had a 150-second terminal bound. Root Nx project/task graph
exports verified all five unique mobile projects, the dependency edges above,
and isolated Android/iOS/mock build task graphs without executing native builds.
Python migration/workflow-selection regression tests and static JSON/YAML/shell
checks also passed (4 migration and 4 workflow-selection regression tests).
`nx show projects --affected --files=...` verified fixture/lock changes affect
all five mobile projects, contract changes affect its three consumers, and mock
changes affect Android but not iOS. SHA-256 manifests confirmed all 198 local proto, generated,
fixture, and lock files unchanged after integrated execution; temporary drift
and conformance directories were cleaned. Validation caches/build outputs under
`apps/mobile` are removed before handoff; root Nx cache remains root-managed.

## Residual limits

- Native iOS cannot build/test on Linux; its helper fails clearly off macOS.
- Full Android build/test/lint was previously blocked by a missing Android SDK;
  these were not rerun in this integrated validation. Root CI prepares the SDK.
- Android install, persistent mock serve, and in-place contract generation were
  intentionally not run.
- Buf remote generation depends on network/service availability.
- Gradle and protobuf report upstream Gradle-10/`sun.misc.Unsafe` deprecations;
  unrelated source/API rewrites remain out of scope.
