# @loams/studio-ui

Loams Studio's component library (design [§53](../../../docs/design/53-studio-ade.md) §9, D912; plan [SD1](../../../docs/plans/2026-10-13-sd1-studio-ade.md)). It holds components and logic vendored from [T3 Code](https://github.com/pingdotgg/t3code) (MIT) and rebuilt on Loams' stack: `@loams/ui` tokens and primitives, plain TypeScript, zustand for client-only state (Q819) and no Effect. It is a library, not a cordis plugin: only `@loams/plugin-studio` imports it (SD1 Ruling 4), and it takes plain view models rather than talking to agentd itself.

User guides: [`docs/guides/studio/`](../../../docs/guides/studio/index.md). This file is for maintainers.

## Layout

| Entry | What |
|---|---|
| `.` (`src/index.ts`) | The thread list, composer, terminal, git, keybindings, palette and generic hooks |
| `./chat`, `./markdown`, `./approvals` | The timeline, the sanitised markdown renderer, the approval and question cards |
| `./diff`, `./files` | The diff panel and review comments, the file tree and viewer (Shiki and `@pierre/diffs`, loaded lazily) |
| `./groups`, `./preview` | The run-group compare view (Loams' own code) and the preview chrome |

Heavy entries are separate subpath exports so Studio's first screen doesn't load Shiki or `@pierre/diffs` (SD1 T21-7).

## Licence and attribution (D913)

- The package is `Apache-2.0 AND MIT`: Loams' code is Apache-2.0 ([`LICENSE`](LICENSE)); the portions from T3 Code stay MIT ([`LICENSE.t3code`](LICENSE.t3code), T3 Code's licence verbatim). [`NOTICE`](NOTICE) says which is which. `@loams/ui` carries the same `LICENSE.t3code` for its 12 primitives from T3 Code.
- **Every vendored file starts with this header**, filled in by the import tool:

  ```ts
  // Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
  // <upstreamPath> at commit <short sha>, Copyright (c) 2026 T3 Tools Inc., MIT licence
  // (see web/packages/studio-ui/LICENSE.t3code). Modified by The Loams Authors:
  // <what changed>. Loams' changes are Apache-2.0.
  ```

- **[`provenance.json`](provenance.json) has one row per vendored file.** The file records the upstream, its licence and copyright, and the source archive: commit `6524a8d43795dc543f7cfb572388cdea3662ad48`, archive SHA-256 `4d13a8e3…14a3`. Each row has:
  - `path`, `upstreamPath`, `commit`;
  - `upstreamSha256` and `localSha256`;
  - `verdict` (`as-is`, `adapt` or `rewrite`), `group` and a `note`.
- **The bundles ship the attribution.** The console build emits the root `NOTICE` and `LICENSE.t3code` beside the bundle (`web/apps/console/vite.config.ts`). The desktop package ships them in its resources folder.
- **A PR that vendors files lists them in its description** (CONTRIBUTING rule 4).

## Rules for vendored code

- **Upstream is untrusted data.** Read the archive only: never run its scripts, install its dependencies or build it. Any Python over it runs as `python3 -I`, with the script kept outside the tree.
- **No Effect.** No `effect`, `@effect/*`, `@effect-atom/*`, nor T3 Code's `contracts`, `client-runtime` or `shared` packages (`test/imports.test.ts`).
- **Our UI rules.** `@loams/ui` tokens only (no colour literals), `@loams/ui` primitives only (never `@base-ui/react` directly), and no raw HTML from models, tools, files or diffs (D919). Every renderer has an XSS fixture test.
- **As-is files stay byte-identical** to upstream apart from the header and the mapped imports, so their `localSha256` stays meaningful. `web/biome.json` keeps upstream's double quotes for this package and exempts as-is files from import sorting and a few lints (SD1 T1-4).
- **Licences.** New npm dependencies go through the pnpm catalog, pinned exactly, at least 14 days old, and must pass `node scripts/ci/npm-licences.mjs` (D914). `heic-to` (LGPL-3.0) is never added.

## The import tool

[`scripts/studio/vendor.mjs`](../../../scripts/studio/vendor.mjs) only reads and copies bytes. Its zip reader refuses zip64, encrypted entries, absolute paths, drive letters, backslashes and `..`, checks every entry before writing any, writes without an executable bit and skips symbolic links (SD1 T1-5).

```sh
# 1. Fetch the archive into a new, empty directory outside the repository.
node scripts/studio/vendor.mjs fetch --commit <sha> --out <scratch> [--expect-sha256 <hex>]
#    (or --archive <zip> for a local copy)

# 2. Import files, one component group at a time.
node scripts/studio/vendor.mjs import --from <scratch>/<top> --group <g> \
  --map <upstream path>=<local path> [--map …] --verdict as-is|adapt|rewrite \
  [--rewrite <specifier>=<specifier> …] --change "<what changed>" [--note "<note>"] \
  [--package web/packages/ui]

# 3. Check headers, rows and as-is hashes (CI runs both).
node scripts/studio/vendor.mjs check
node scripts/studio/vendor.mjs check --package web/packages/ui
```

SD1's tasks committed each group twice: once as the upstream text (`studio: import T3 Code's … (upstream text)`), then adapted, so the adaptation reads as a diff.

## Re-syncing (D915)

Vendored code is a fork. Re-sync on demand, at most once a month, one component group per PR, and take upstream security fixes within 7 days. The `Studio upstream reminder` workflow (`.github/workflows/studio-upstream-reminder.yml`) opens an issue on the 1st of each month. It downloads nothing itself.

```sh
node scripts/studio/vendor.mjs fetch --commit <recorded sha> --out <old scratch>
node scripts/studio/vendor.mjs fetch --commit <new sha> --out <new scratch>
node scripts/studio/upstream-report.mjs --from <old scratch>/<top> --to <new scratch>/<top> [--package <dir>] [--json]
```

For each provenance row the report says `unchanged`, `changed upstream` (with the unified diff) or `gone upstream`. Port each change by hand into the adapted file, update the row's `commit`, `upstreamSha256` and `localSha256` (re-import as-is files with `import`), and run `check`.

## Tests

```sh
pnpm --filter @loams/studio-ui test        # Vitest: headers, provenance hashes, imports, every component
pnpm --filter @loams/studio-ui typecheck
node --test scripts/studio/vendor.test.mjs # the import tool
```
