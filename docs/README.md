![Loams — Your data. Your bucket.](assets/loams-banner.svg)

# Loams documentation

Build, understand, and contribute to Loams: hybrid retrieval on object storage, with reactive data and durable execution alongside it.

## Start with your task

| You want to… | Read |
|---|---|
| Run your first query | [Repository quickstart](../README.md#quick-start) |
| Build on Windows or macOS | [Build from source](build-from-source/README.md) |
| Understand the architecture | [Design overview](design/README.md), [architecture](design/01-architecture.md), and [decision log](design/13-decision-log.md) |
| Work across engine, apps, and plugins | [Monorepo guide](monorepo.md) |
| Integrate an API or SDK | [Route map](api/route-map.md), [error reasons](api/reasons.md), and [SDK runtime contract](sdk/runtime-contract.md) |
| Contribute a change | [Contributing](../CONTRIBUTING.md), [implementation plans](plans/README.md), and [wiki](wiki/Home.md) |
| Package or release Loams | [Packaging](release/packaging.md), [publishing](release/publishing.md), and [signing](release/signing.md) |
| Build a plugin | [Plugin documentation](../plugins/docs/README.md) and [marketplace publishing](marketplace/publishing.md) |

## Explore the platform

| Directory | Scope |
|---|---|
| [design/](design/README.md) | Architecture, formats, invariants, decisions, and proposed platform tracks |
| [plans/](plans/README.md) | Implementation tasks, milestone evidence, and dependency studies |
| [api/](api/) | Native routes and stable error reasons |
| [sdk/](sdk/) | Runtime contract and cross-language fixtures |
| [build-from-source/](build-from-source/README.md) | Toolchains and platform prerequisites |
| [release/](release/) | Distribution, signing, and publishing policy |
| [ecosystem/](ecosystem/) | Deploy buttons and ecosystem integrations |
| [wiki/](wiki/README.md) | Contributor and maintainer guides |

Additional references: [open-core boundaries](open-core.md), [remote browser provider](remote-browser-provider.md), [architecture review](architecture-review-and-recommendations.md), and [backlog analysis](open-issues-analysis.md).

## Read status in context

The code and [root README](../README.md#features) describe current implementation. Design documents explain intended behavior; plans and reviews are dated snapshots and may describe work that has since landed. A proposal is not a shipped feature. Loams has no stable release; verify flags with the binary from your checkout and keep the selected source ref with any deployment instructions.

The public site at [loams.dev/docs](https://loams.dev/docs) combines reader guides with generated copies of `docs/design`. Update design originals here; the website syncs them with `LOAMS_REPO`, `LOAMS_REF`, or `LOAMS_DESIGN_DIR`.

Use **Loams** in prose, `loams` for the binary, `loams-*` / `loams_*` for Rust crates and paths, `@loams/*` for npm packages, and `loams.*.v1` for protobuf packages. Keep examples aligned with actual source paths and distinguish unpublished packages from installable releases.
