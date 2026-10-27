# MA1 — Loams × Mastra: the Compatibility Gate, the Resonate Engine Upstream, Loams API Fixes, then `@loams/mastra`

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. One agent per task. Work test first. Where this plan gives exact names, paths, branches, tests or commit messages, use them verbatim. Record any deviation under "Rulings made during execution" at the end of this file (a PR to `dev` of `ostrium-labs/loams`).
>
> **Status: Approved** (owner, 2026-10-20, all defaults). Track MA, design [§66](../design/66-loams-mastra.md) (D1202–D1227, Q1000–Q1014). The owner asked on 2026-10-20 for a Loams integration with Mastra and said that once the plan is done a public repository `ostrium-labs/loams-mastra` is created and building starts. The same day the owner moved the Resonate execution engine forward: it runs **in parallel with MA0** and is contributed upstream to Mastra as `@mastra/resonate`.
>
> This plan fully specifies **MA0** (Tasks 0–12), **MAR** (Tasks 13–22) and **MA1** (Tasks 23–27). **MA2** (Tasks 28–37) and **MA3** (Tasks 38–41) are at task level and get full specifications when their gates are met (D1227). Adoption tasks are 42–46.

**Goal:**
- Mastra's own pg and Qdrant adapter suites pass against Loams in public CI, with every remaining failure listed, explained and tracked;
- a developer goes from nothing to a Mastra agent with memory on Loams in three code blocks, on loopback or a tailnet;
- Mastra's integrations list carries a Loams page;
- Mastra workflows run durably on any Resonate server, Loams Durable included, through `@loams/mastra-resonate` now and `@mastra/resonate` upstream;
- the Loams API gaps the suites find are closed in the monorepo;
- later, `@loams/mastra` gives hybrid search, collection storage, Streams pubsub, a cache, a span exporter and Loams Memory.

**Architecture** (§66 §4): two repositories, plus upstream contributions.
- `ostrium-labs/loams-mastra`, new and public, Apache-2.0, a pnpm workspace:
  - `packages/mastra` (`@loams/mastra`, empty until MA2);
  - `packages/resonate` (`@loams/mastra-resonate`, laid out like Mastra's `workflows/inngest` so it can move upstream unchanged);
  - `examples/`, `templates/`, `test/conformance/` (vendored Mastra suites at release tags, a Loams harness, expected-failure lists, a reporter), `docs/`.
- `ostrium-labs/loams`: MA1's fixes in `crates/loams-qdrant`, `crates/loams-es`, the `qdrant-client` CI job and a nightly gate job.
- `mastra-ai/mastra`: an issue and a PR for the docs page, and an issue and a PR for `workflows/resonate`.

**Tech stack** (pins checked in Task 0):
- Node 22.13 or newer (CI uses 22); pnpm 11.27.1 (the monorepo's); TypeScript 5.9.3; Biome 2.5.14; vitest and tsdown at the versions the vendored Mastra suites use; changesets.
- Mastra: `@mastra/core` 1.78.0, `@mastra/pg` 1.31.2, `@mastra/qdrant` 1.1.4, `@mastra/elasticsearch` 1.4.4, `@mastra/memory` 1.38.0, `@mastra/inngest` 1.10.6 (reference only).
- `@qdrant/js-client-rest` 1.18.0 and 1.19.0; `@resonatehq/sdk` 0.11.5.
- Loams v0.1.0: `loams_0.1.0_amd64.deb` and `loams-compose-0.1.0.tar.gz`, checked against `SHA256SUMS`.

## Global Constraints

- **Repositories and worktrees.**
  - Monorepo work: `~/Documents/Ostriumlabs/loams-wt/ma1-<task>`, one branch and one PR per task into `dev`.
  - `loams-mastra` work: the clone at `~/Documents/Ostriumlabs/loams-mastra`, worktrees at `~/Documents/Ostriumlabs/loams-mastra-wt/<task>`, one branch and one PR per task into `main` (Q1006).
  - CodeRabbit reviews every PR; merge on green required checks; no reviewer subagents.
  - Every commit uses `git commit -s`. Never `git stash`.
- **Cloning.** The only clone is our own new repository after Task 1 creates it (the owner asked for it on 2026-10-20). Nothing else is cloned: Mastra code is fetched file by file at a pinned tag by `scripts/vendor-upstream.mjs`, and the Loams Postgres stack comes from the release tarball.
- **Licence.**
  - Never read into the repository, copy or import any path with an `ee/` segment, `packages/connect/src/providers`, or anything under the Mastra Enterprise Edition License or Elastic-2.0 (D1203).
  - Vendored or adapted Apache-2.0 files keep a first-line header `Vendored from mastra-ai/mastra@<commit>:<path> (Apache-2.0). Changes: <list or none>.` and appear in `NOTICE`.
  - No GPL, AGPL, BSL, SSPL or Elastic-2.0 dependency; `scripts/licences.mjs` enforces it.
- **Runners.** GitHub-hosted runners only (`ubuntu-24.04`). No self-hosted, Blacksmith or `loam-bench` runners in this track.
- **Never public.** No example, template, test or doc points at a public Loams endpoint. Loopback (`127.0.0.1`) or a tailnet hostname only. Test `no_public_endpoints` (Task 3) greps examples, templates and docs for `http(s)://` hosts that are not loopback, `*.ts.net`, `localhost` or documentation hosts.
- **Secrets.** No secrets in conformance CI except a throwaway `lk_` key minted per run. Publishing uses npm trusted publishing (OIDC) only. Test `no_secret_in_artifacts` greps uploaded logs and reports for `lk_[0-9A-HJKMNP-TV-Z]{26}_`.
- **Pins.** Exact versions for every `@mastra/*` package in examples, templates and `test/conformance/pins.json`. Third-party packages must be at least 3 days old (`minimumReleaseAge`), except in the nightly `alpha` cell, which runs without secrets.
- **Product name and email.** "Loams". Contacts are `@ostriumlabs.org` (security@ostriumlabs.org for vulnerabilities only). There is no Ostrium support address (D1323): users get help through GitHub issues and Discord (https://discord.gg/eK6NanHQq).
- **Builds in the monorepo.** Shared target directory, one cargo build at a time, touched crates only (`cargo test -j 4 -p <crate>`); never build in `/tmp`.

## Review Focus

1. **A vendored file from a forbidden licence.** Expected: impossible. Tests: `vendor_refuses_ee_paths`, `vendor_refuses_connect_providers`, `vendor_refuses_non_apache_header`, and the `licences` CI step.
2. **The gate passes while hiding a failure.** Expected: an unexpected failure **or** an unexpected pass fails the job. Tests: `reporter_fails_on_unexpected_failure`, `reporter_fails_on_unexpected_pass`, `reporter_requires_issue_link`.
3. **Something public, or a key leaked.** Expected: never. Tests: `no_public_endpoints`, `no_secret_in_artifacts`.
4. **An unpinned `@mastra/*` in an example or template.** Expected: refused. Test: `examples_pin_exact_versions`.
5. **The Resonate engine runs a step twice after a crash, or retries twice (Resonate and Mastra both).** Expected: once per step; Mastra's `retryConfig` alone decides retries. Tests: `crash_mid_workflow_resumes_without_rerunning_steps`, `retries_follow_mastra_retry_config_only`.
6. **The payload-index delete breaks filtering or silently changes types.** Expected: filters unchanged, a type change still refused. Tests: `delete_field_index_keeps_filters`, `recreate_with_other_type_unsupported`.

## Execution order and launching

- **Wave 1 (now, in parallel):**
  - Task 0 (rulings);
  - Task 1 (create the repository; the other `loams-mastra` tasks wait for it);
  - Tasks 23 and 24 (MA1, monorepo, no dependency on the new repository).
- **Wave 2 (after Task 2):**
  - MA0: Tasks 3–6, then 7–9, then 10–12;
  - MAR: Tasks 13–16, then 17–22.

  MA0 and MAR touch different directories (`test/conformance`, `examples`, `templates` against `packages/resonate`, `test/resonate`) and only share `.github/workflows/ci.yml`. One small rebase is expected there.
- **Task 25** (the JS Qdrant client) may start at once.
- **Task 26** (gate findings) starts after Tasks 7–9's first runs.
- **Task 27** (the monorepo nightly) after Task 10.
- **MA2** waits for #884 to merge and `@loams/client` to be released.
- **MA3** waits for MM1a's MVP gate (and #847 for Task 40).

---

## MA0 — Zero-package compatibility (Tasks 0–12)

### Task 0: Rulings

**Repo:** `ostrium-labs/loams` (this file only). **Branch:** `ma1-t0-rulings`.

Check each item and record the answer as a ruling (`MA-R0.n`) at the end of this file:

1. **Mastra tag commits.**
   - Resolve `@mastra/core@1.78.0`, `@mastra/pg@1.31.2`, `@mastra/qdrant@1.1.4`, `@mastra/elasticsearch@1.4.4`, `@mastra/memory@1.38.0` and `@mastra/inngest@1.10.6` to commit SHAs. Peel annotated tags.
   - Record the `stores/_test-utils/package.json` and `workflows/_test-utils/package.json` names, versions and dependencies at the pg, qdrant and inngest commits.
2. **Loams release.**
   - The SHA-256 of `loams_0.1.0_amd64.deb` and `loams-compose-0.1.0.tar.gz` from `SHA256SUMS`, and whether `SHA256SUMS.asc` verifies with the published key.
   - That the `.deb`'s `loams dev` serves Qdrant on `127.0.0.1:6333` and ES on `127.0.0.1:9200` by default, and Resonate on `--durable-listen 127.0.0.1:8001` (the release build has the `durable` feature).
   - The `loams auth new-key` invocation that mints a key into `LOAMS_API_KEYS_FILE`.
3. **Loams Postgres on a GitHub-hosted runner.**
   - `loams-compose-0.1.0/loams-postgres-dev` comes up with `compute1` on `127.0.0.1:55433` within the job's disk and time (about 2.3 GB of layers).
   - Record the credentials, database name and the pgvector version (`SELECT extversion FROM pg_available_extensions WHERE name='vector'`).
4. **PgDog for the pooler cell.** If PgDog has an image pin anywhere in the monorepo (PG2), use it by digest. Otherwise pin `ghcr.io/pgdogdev/pgdog` at a release at least 14 days old, by digest, in transaction mode. If neither works on a runner, defer the pooler cell to Task 26 and record why.
5. **`create-mastra --template`.** Does it accept a GitHub subdirectory URL (`https://github.com/ostrium-labs/loams-mastra/tree/main/templates/loams-starter`), and does it accept a local path for CI (`packages/cli/src/commands/create/create.ts` at the pinned CLI)? This decides Q1000's mirror.
6. **pnpm 11.27.1 settings.** The exact `pnpm-workspace.yaml` keys for the minimum release age, its exclusions, and the build-script allowlist (`onlyBuiltDependencies` or its pnpm 11 successor).
7. **`@mastra/qdrant` 1.1.4's `QdrantVector`** passes `headers` through to `QdrantClient` (the constructor spreads `QdrantClientParams`).
8. **Resonate.**
   - The 0.11.5 TypeScript API: `Resonate.remote`/`Resonate.local`, `register`, `run`/`rpc`, `ctx.run`, `ctx.sleep`, `ctx.promise`, `ctx.date`, and how a durable promise is settled externally.
   - The upstream Resonate server release and image to pin (at least 14 days old), and its listen flags.
9. **A mock model for the end-to-end tests** at `@mastra/core` 1.78.0: Mastra's own test model or `ai`'s mock language model. Name the import.
10. **Monorepo CodeRabbit.** Confirm that CodeRabbit's organisation install covers new repositories (D406).

**Acceptance:** every item has a ruling, or an explicit "deferred to Task N" with the reason.
**Commit:** `docs(plans): MA1 Task 0 rulings`.

### Task 1: Create `ostrium-labs/loams-mastra`

**Repo:** GitHub organisation settings; local clone. **Owner approval:** given on 2026-10-20.

**Steps:**
1. Create the repository:
   ```
   gh repo create ostrium-labs/loams-mastra --public \
     --description "Loams for Mastra: compatibility gate, Resonate execution engine, @loams/mastra" \
     --homepage https://loams.dev --disable-wiki
   ```
   Default branch `main`.
2. Repository settings:
   - squash merge only, auto-delete head branches;
   - private vulnerability reporting on, Discussions on;
   - topics `loams`, `mastra`, `ai-agents`, `typescript`, `resonate`, `durable-execution`.
3. Clone it to `~/Documents/Ostriumlabs/loams-mastra` and create `~/Documents/Ostriumlabs/loams-mastra-wt/`. Add both to `.git/info/exclude`-style local ignores, not to any tracked file.
4. After Task 3 lands, add a branch ruleset on `main`:
   - required checks `CI required` and `DCO sign-off`;
   - linear history, no force pushes, one approving review or a CodeRabbit pass as the monorepo does.
5. Create the GitHub environment `npm` with required reviewers (owner action: name them) and no secrets.
6. Configure npm trusted publishing for `@loams/mastra` and `@loams/mastra-resonate`: repository `ostrium-labs/loams-mastra`, workflow `release.yml`, environment `npm`. The `@loams` npm scope already exists, since `@loams/client` 0.1.0 is published.

**Acceptance:** `gh repo view ostrium-labs/loams-mastra --json visibility,defaultBranchRef` shows `PUBLIC` and `main`.
**Commit:** none (the scaffold is Task 2). Record the settings as ruling `MA-R1.1` in this file.

### Task 2: The repository scaffold

**Repo:** `ostrium-labs/loams-mastra`. **Branch:** `scaffold`.

**Files (create):**
- `LICENSE`: the full Apache-2.0 text.
- `NOTICE`:
  ```
  Loams for Mastra
  Copyright 2026 The Loams Authors

  This product includes software developed by The Loams Authors.

  Portions of this product are derived from Mastra (https://github.com/mastra-ai/mastra),
  Copyright (c) 2025 Kepler Software, Inc., licensed under the Apache License 2.0.
  Each derived file names its source commit and path in its first line, and the
  vendored directories are listed in test/conformance/upstream/*/UPSTREAM.json.
  Nothing in this product is taken from Mastra's ee/ directories (Mastra Enterprise
  Edition License) or from Elastic-2.0 code.

  Some Mastra packages omit a "license" field in package.json; Mastra's root
  LICENSE.md licenses them under Apache-2.0.
  ```
- `README.md`: what the repository is (§66 §1.2 in four sentences), the phase table, a quickstart link, the compatibility badge placeholder, the "engine is never public" note, the licence, the contact.
- `CONTRIBUTING.md`:
  - the Developer Certificate of Origin 1.1 text and the rule that every commit carries `Signed-off-by` (`git commit -s`), with the retroactive sign-off format the DCO check accepts;
  - setup (`corepack enable`, `pnpm install --frozen-lockfile`);
  - tests (`pnpm test`, `pnpm conformance:pinned` with `loams dev` running);
  - the licence rules of D1203;
  - how to update the vendored suites (`pnpm vendor -- --package @mastra/pg --tag @mastra/pg@<v>`);
  - "upstream first" for generic fixes (Mastra issues, `ostrium-labs/loams` issues).
- `SECURITY.md`: report through GitHub's private advisory form or `security@ostriumlabs.org`; supported versions; the supply-chain controls of §66 §11.2.
- `CODE_OF_CONDUCT.md`: copied from `ostrium-labs/loams` (same authors).
- `.github/CODEOWNERS`: `* @ostrium-labs/maintainers` (verify the team name in Task 0's GitHub checks).
- `.gitignore`, `.editorconfig`, `.nvmrc` (`22`).
- `package.json` (root):
  ```json
  {
    "name": "loams-mastra",
    "private": true,
    "license": "Apache-2.0",
    "packageManager": "pnpm@11.27.1",
    "engines": { "node": ">=22.13.0" },
    "scripts": {
      "build": "pnpm -r --filter ./packages/* build",
      "test": "pnpm -r --filter ./packages/* --filter ./scripts test",
      "typecheck": "pnpm -r typecheck",
      "lint": "biome check .",
      "licences": "node scripts/licences.mjs",
      "vendor": "node scripts/vendor-upstream.mjs",
      "conformance:pinned": "pnpm --filter @loams-internal/conformance run pinned",
      "compat": "node scripts/gen-compat.mjs"
    },
    "devDependencies": { "@biomejs/biome": "2.5.14", "typescript": "5.9.3", "@changesets/cli": "<exact, Task 0>" }
  }
  ```
- `pnpm-workspace.yaml`:
  - packages `packages/*`, `examples/*`, `templates/*`, `test/conformance`, `test/conformance/upstream/*/*`, `test/resonate`, `scripts`;
  - the minimum release age of 4320 minutes, excluding `@loams/*`;
  - the build-script allowlist, empty unless a dependency proves it needs one.

  Use Task 0's exact key names for both settings.
- `.npmrc`: `engine-strict=true`, `save-exact=true`.
- `biome.json`, `tsconfig.base.json` (strict, `module`/`moduleResolution` `NodeNext`, target ES2023).
- `.changeset/config.json` (`baseBranch: main`, `access: public`).
- `packages/mastra/`:
  - `package.json`: `@loams/mastra`, `version 0.0.0`, `private: true` until MA2, `type: module`, `exports` `"."`, peer `@mastra/core` `">=1.78.0 <2.0.0"`, `license Apache-2.0`, `repository.directory packages/mastra`;
  - `src/index.ts` (exports `VERSION`), `src/version.ts`, `tsdown.config.ts`, `vitest.config.ts`, `README.md`;
  - `test/version.test.ts`.
- `scripts/package.json` (private `@loams-internal/scripts`, `type: module`, `test` runs `node --test`).

**Tests:** `version_is_semver` (`packages/mastra/test/version.test.ts`).
**Acceptance:** `pnpm install --frozen-lockfile && pnpm lint && pnpm typecheck && pnpm test` pass locally.
**Commit:** `repo: scaffold the pnpm workspace, licence, NOTICE, README, CONTRIBUTING with DCO`.

### Task 3: CI workflows, DCO and the repository checks

**Repo:** `ostrium-labs/loams-mastra`. **Branch:** `ci`.

**Files (create):**
- `.github/workflows/ci.yml` (on `pull_request`, `push` to `main`; `permissions: contents: read`; every action pinned by commit SHA with the version in a comment; `runs-on: ubuntu-24.04`). Jobs:
  1. `checks`: `pnpm install --frozen-lockfile`, `pnpm lint`, `pnpm typecheck`, `pnpm test`, `pnpm licences`, `npm audit signatures`, `node scripts/repo-checks.mjs`.
  2. `actionlint`.
  3. `CI required`: an aggregator over every job, the shape of the monorepo's `required` job. Tasks 7–10 and 16–18 add their jobs to it.
- `.github/workflows/dco.yml`, `scripts/check-dco.sh`, `scripts/test-dco.sh`: copied from `ostrium-labs/loams` (`.github/workflows/dco.yml`, `scripts/ci/check-dco.sh`, `scripts/ci/test-dco.sh`), first-line provenance comment `Copied from ostrium-labs/loams@<sha>:<path> (Apache-2.0)`.
- `.github/dependabot.yml`:
  - `npm` and `github-actions`, weekly;
  - `cooldown` 3 days, and 7 days for semver-major;
  - `@mastra/*` grouped;
  - the `test/conformance/upstream/**` directories ignored, because the vendoring script updates them.
- `.github/workflows/release.yml`:
  - `workflow_dispatch` and changesets' version PR merge;
  - environment `npm`, `permissions: id-token: write, contents: write`;
  - `pnpm publish -r --provenance --access public` for non-private packages;
  - no `NPM_TOKEN`. It publishes nothing while every package is `private`.
- `.github/ISSUE_TEMPLATE/mastra-compat.yml`, `.github/pull_request_template.md` (with the DCO and "no public endpoints" checkboxes).
- `scripts/licences.mjs`:
  - walks the installed tree (`pnpm licenses list --json`);
  - fails on `GPL-*`, `AGPL-*`, `LGPL-*` (allowed only as a dependency with an exception recorded in `scripts/licences.exceptions.json`), `BUSL-*`, `SSPL-*`, `Elastic-2.0` and any licence text matching "Mastra Enterprise";
  - fails on any vendored file under `test/conformance/upstream` whose path has an `ee/` segment.
- `scripts/repo-checks.mjs`, with three named checks, each also a `node --test` case in `scripts/repo-checks.test.mjs`:
  - `no_public_endpoints` (Global Constraints);
  - `examples_pin_exact_versions` (every `@mastra/*`, `@loams/*`, `@qdrant/*` and `@resonatehq/*` dependency in `examples/*/package.json` and `templates/*/package.json` is an exact version);
  - `no_secret_in_artifacts` (a function also used by the conformance jobs on their artifacts).

**Tests:**
- `scripts/test-dco.sh`;
- `scripts/repo-checks.test.mjs`: `no_public_endpoints_flags_public_url`, `no_public_endpoints_allows_loopback_and_tailnet`, `examples_pin_exact_versions_flags_caret`, `no_secret_in_artifacts_flags_lk_key`;
- `scripts/licences.test.mjs`: `licences_refuses_elastic`, `licences_refuses_ee_path`.

**Acceptance:** the PR's own run is green; `CI required` reports; a test commit without sign-off fails `DCO sign-off` (then dropped).
**Commit:** `ci: lint, typecheck, tests, licences, DCO and the release workflow on GitHub-hosted runners`.

### Task 4: Vendoring Mastra's suites at release tags

**Repo:** `ostrium-labs/loams-mastra`. **Branch:** `vendor`.

**Files (create):**
- `scripts/vendor-upstream.mjs`:
  - CLI `--package <npm name> --tag <git tag> [--out <dir>] [--paths <comma list>]`.
  - Reads `GET https://api.github.com/repos/mastra-ai/mastra/git/trees/<commit>?recursive=1` (with `GH_TOKEN` if set) and fetches each file from `https://raw.githubusercontent.com/mastra-ai/mastra/<commit>/<path>`. It never runs `git clone`.
  - Default path sets per package:
    - `@mastra/pg` → `stores/pg/{package.json,src/**}` and `stores/_test-utils/{package.json,src/**}`;
    - `@mastra/qdrant` → `stores/qdrant/{package.json,src/**}` and `stores/_test-utils/**`;
    - `@mastra/elasticsearch` → `stores/elasticsearch/{package.json,src/**}` and `stores/_test-utils/**`;
    - `@mastra/inngest` → `workflows/_test-utils/**` (MAR).
  - Excludes `**/*.perf.test.ts`, `**/*.stress.test.ts`, `**/performance-test.ts`, `**/*benchmark*`, `**/row-number-performance.test.ts`.
  - Refuses (exit 2) any path with an `ee` segment, any path under `packages/connect`, and any file whose leading comment names a licence other than Apache-2.0.
  - Prepends the provenance header to `.ts`, `.mjs` and `.js` files (`// Vendored from mastra-ai/mastra@<commit>:<path> (Apache-2.0). Changes: imports rewritten.`).
  - Rewrites `workspace:*` and `catalog:` versions in vendored `package.json` files to the exact versions in `test/conformance/pins.json`, renames packages to `@loams-internal/upstream-<name>`, marks them `private`, and points `@internal/storage-test-utils` / `@internal/workflow-test-utils` at the vendored copies (`workspace:`).
  - Writes `UPSTREAM.json`: package, tag, commit, the file list with SHA-256, the exclusions, and the date.
- `scripts/vendor-upstream.test.mjs` (fetches mocked).
- `test/conformance/pins.json`:
  ```json
  {
    "mastra": {
      "@mastra/core": "1.78.0", "@mastra/pg": "1.31.2", "@mastra/qdrant": "1.1.4",
      "@mastra/elasticsearch": "1.4.4", "@mastra/memory": "1.38.0"
    },
    "tags": { "@mastra/pg": "@mastra/pg@1.31.2", "@mastra/qdrant": "@mastra/qdrant@1.1.4", "@mastra/elasticsearch": "@mastra/elasticsearch@1.4.4" },
    "commits": { "<from Task 0>": "" },
    "loams": { "version": "0.1.0", "deb_sha256": "<Task 0>", "compose_sha256": "<Task 0>" },
    "qdrantJs": ["1.18.0", "1.19.0"],
    "resonate": { "sdk": "0.11.5", "server": "<Task 0>" }
  }
  ```
- The vendored copies:
  - `test/conformance/upstream/mastra-pg@1.31.2/`, `test/conformance/upstream/mastra-qdrant@1.1.4/`, `test/conformance/upstream/mastra-elasticsearch@1.4.4/`, each with its `UPSTREAM.json`;
  - inside each, the `stores/<name>` and `stores/_test-utils` trees.
- `NOTICE`: list the three vendored directories.

**Tests** (`scripts/vendor-upstream.test.mjs`): `vendor_refuses_ee_paths`, `vendor_refuses_connect_providers`, `vendor_refuses_non_apache_header`, `vendor_writes_provenance_header`, `vendor_rewrites_workspace_versions`, `vendor_is_reproducible` (two runs, identical `UPSTREAM.json` file hashes).

**Acceptance:** `pnpm install --frozen-lockfile` resolves the vendored packages; `pnpm --filter @loams-internal/upstream-qdrant exec vitest list` lists the suite.
**Commit:** `conformance: vendor Mastra's pg, Qdrant and Elasticsearch suites at their release tags`.

### Task 5: The Loams harness

**Repo:** `ostrium-labs/loams-mastra`. **Branch:** `loams-harness`.

**Files (create, all `bash`, `set -euo pipefail`):**
- `test/conformance/loams/install-loams.sh`:
  - downloads `loams_<v>_amd64.deb`, `loams-compose-<v>.tar.gz` and `SHA256SUMS` from the GitHub release `v<v>` of `ostrium-labs/loams` (`gh release download` or `curl -fsSL`);
  - checks the SHA-256 against `SHA256SUMS` and against `pins.json`, and refuses on a mismatch;
  - installs the `.deb` with `dpkg -i`, or extracts it with `dpkg-deb -x` into `$RUNNER_TEMP/loams` when not root;
  - unpacks the compose tarball into `$RUNNER_TEMP/loams-compose`.
- `test/conformance/loams/start-loams.sh [--auth]`:
  - runs `loams dev --data-dir "$RUNNER_TEMP/loams-data"` on loopback, with Qdrant `127.0.0.1:6333`, ES `127.0.0.1:9200` and Durable `127.0.0.1:8001`;
  - with `--auth`, sets `LOAMS_AUTH_MODE=apikey`, mints a key with Task 0's `loams auth new-key` command into `LOAMS_API_KEYS_FILE`, and exports `LOAMS_API_KEY` through `$GITHUB_ENV` with `::add-mask::`;
  - waits for `GET http://127.0.0.1:6333/` (60 s).
- `test/conformance/loams/start-postgres.sh`: `docker compose -f "$RUNNER_TEMP/loams-compose/loams-compose-<v>/loams-postgres-dev/compose.yaml" up -d`, waits for `pg_isready -h 127.0.0.1 -p 55433` (180 s), and exports `POSTGRES_HOST=127.0.0.1`, `POSTGRES_PORT=55433` and the user, password and database from Task 0.
- `test/conformance/loams/stop.sh`: stops both and prints the last 200 lines of each log on failure.
- `test/conformance/loams/README.md`: running the gate locally against your own `loams dev`, with `LOAMS_BIN` to use a local build.
- `test/conformance/package.json`:
  - private `@loams-internal/conformance`;
  - scripts `pinned:qdrant`, `pinned:qdrant-auth`, `pinned:pg`, `pinned:pg-pooler`, `pinned:es`, and `pinned` (all required cells).

**Tests:** `start_loams_refuses_bad_checksum` (a `bats`-free shell test in `test/conformance/loams/test.sh` run by CI: corrupt a byte, expect exit 1).
**Acceptance:** in a scratch workflow run, `install-loams.sh && start-loams.sh && start-postgres.sh` brings up both on a GitHub-hosted runner within 10 minutes.
**Commit:** `conformance: a Loams harness that installs the pinned release and starts loams dev and Loams Postgres`.

### Task 6: The reporter, expected failures and `COMPATIBILITY.md`

**Repo:** `ostrium-labs/loams-mastra`. **Branch:** `reporter`.

**Files (create):**
- `test/conformance/reporter/compat-reporter.ts`, a vitest reporter:
  - reads `test/conformance/expected-failures/<suite>.json`:
    ```json
    [{ "test": "<full test name>", "reason": "...", "issue": "https://github.com/ostrium-labs/loams/issues/<n>", "since": "2026-10-2x" }]
    ```
  - after the run, fails the process when a test **failed** but is not listed, when a listed test **passed**, or when an entry has no `issue` URL;
  - writes `results/<suite>.json` (counts, failures, unexpected items, versions from `pins.json`, and the Loams `GET /` version).
- `test/conformance/reporter/compat-reporter.test.ts`.
- `test/conformance/expected-failures/{qdrant,qdrant-auth,pg,pg-pooler,elasticsearch,examples}.json`, all `[]`.
- `scripts/gen-compat.mjs`: renders `COMPATIBILITY.md` from `results/*.json` (rows: Mastra version × Loams version × suite → passed, expected failures, link to the run).
- `COMPATIBILITY.md`: the generated empty table.

**Tests:** `reporter_fails_on_unexpected_failure`, `reporter_fails_on_unexpected_pass`, `reporter_requires_issue_link`, `reporter_passes_when_failures_match`, `gen_compat_renders_matrix`.
**Commit:** `conformance: a reporter that fails on unexpected failures and unexpected passes`.

### Task 7: The Qdrant cells

**Repo:** `ostrium-labs/loams-mastra`. **Branch:** `gate-qdrant`.

**Files:**
- Create `.github/workflows/conformance.yml`: on `pull_request` and `push` to `main`; job `qdrant` (matrix `auth: [false, true]`).
  - Steps: Task 5's scripts (`start-loams.sh` or `start-loams.sh --auth`), then `pnpm --filter @loams-internal/conformance run pinned:qdrant` or `pinned:qdrant-auth`.
  - Environment for the auth cell: `QDRANT_URL=http://127.0.0.1:6333`, `QDRANT_API_KEY=$LOAMS_API_KEY`, `LOAMS_NAMESPACE=default`.
  - Upload `results/` after the `no_secret_in_artifacts` check. Add both cells to `CI required`.
- Create `test/conformance/suites/qdrant/vitest.config.ts`:
  - includes the vendored `stores/qdrant/src/vector/*.test.ts`;
  - `reporters: ['default', '../../reporter/compat-reporter.ts']`;
  - a `setupFiles` entry that, in the auth cell, wraps the vendored tests' `new QdrantVector({...})` options to add `apiKey` and `headers: { 'Loams-Namespace': ... }`.

  The wrapping is a vitest module mock of `@qdrant/js-client-rest`'s constructor that merges the two fields. It is not an edit to the vendored file.
- Update `test/conformance/expected-failures/qdrant.json` and `qdrant-auth.json` with the first run's failures.
  - Each entry links an issue filed in `ostrium-labs/loams` (label `mastra-compat`): one issue per root cause, not per test.
  - Expected (verify): `deletePayloadIndex` → MA1 Task 23; geo, `nested` and `has_vector` cases → "not planned" issue with the D1226 reference; named-vector `describeIndex` → a Mastra bug, linked to a Mastra issue instead.

**Tests:** the suite itself; the reporter's verdict.
**Acceptance:** both cells green with the recorded expected failures.
**Commit:** `conformance: run Mastra's Qdrant suite against loams dev, with and without API keys`.

### Task 8: The pg cells and the Loams Postgres checks

**Repo:** `ostrium-labs/loams-mastra`. **Branch:** `gate-pg`.

**Files:**
- Modify `conformance.yml`: job `pg`, after `start-postgres.sh`, runs `pinned:pg`, then `test/conformance/checks/postgres.test.ts`. Job `pg-pooler` (if Task 0 item 4 resolved it) starts PgDog by digest in transaction mode in front of `compute1` on `127.0.0.1:6432` and runs the vendored `*.pooler.test.ts` with `POSTGRES_PORT=6432`. Add both to `CI required`.
- Create `test/conformance/suites/pg/vitest.config.ts`: the vendored `stores/pg/src/**/*.test.ts` without the excluded perf files; `POSTGRES_*` from the harness; reporter as in Task 7.
- Create `test/conformance/checks/postgres.test.ts`, one test per §66 §5.3 check:
  - `v1_pgvector_at_least_0_7`;
  - `v2_create_extension_as_app_role` (creates a role like pg-control's application role, without superuser);
  - `v3_advisory_xact_lock_and_skip_locked` (two connections, a `pg_advisory_xact_lock` held by one blocks the other; `FOR UPDATE SKIP LOCKED` skips; run directly and, in `pg-pooler`, through PgDog);
  - `v4_partition_mode_recorded` (initialises observability vNext and writes the detected `PartitionMode` into `results/pg.json`; passes for `native`, `partman` or `timescale`);
  - `v5_disable_init_then_init`;
  - `v6_split_pools`.

  V7 (suspended-compute wake-up) runs nightly in Task 10.
- Update `expected-failures/pg.json` and `pg-pooler.json` from the first run, each failure with an issue in `ostrium-labs/loams` (or `ostrium-labs/loams-postgres` for an extension-manifest finding).

**Acceptance:** `pg` green, with any expected failures linked; `results/pg.json` records the pgvector version and partition mode.
**Commit:** `conformance: run Mastra's pg suite against Loams Postgres and check pgvector, locks through the pooler and partitioning`.

### Task 9: The Elasticsearch observed cell

**Repo:** `ostrium-labs/loams-mastra`. **Branch:** `gate-es`.

**Files:**
- Modify `conformance.yml`: job `elasticsearch`, `continue-on-error: true`, **not** in `CI required` (Q1009); `pinned:es` against `http://127.0.0.1:9200`.
- Create `test/conformance/suites/elasticsearch/vitest.config.ts`.
- Update `expected-failures/elasticsearch.json` from the first run. Expected: `listIndexes` and the Memory paths that use it → MA1 Task 24.

**Commit:** `conformance: observe Mastra's Elasticsearch suite against loams dev`.

### Task 10: Quickstarts, end-to-end tests and the nightly matrix

**Repo:** `ostrium-labs/loams-mastra`. **Branch:** `quickstarts`.

**Files (create):**
- `examples/quickstart-postgres/`:
  - `package.json`, exact pins: `@mastra/core` 1.78.0, `@mastra/pg` 1.31.2, `@mastra/memory` 1.38.0, `zod` (exact);
  - `src/mastra/index.ts` (§66 §5.1 variant A, plus an agent with `Memory` using `PgVector` for semantic recall);
  - `.env.example` (`DATABASE_URL=postgresql://<user>:<password>@127.0.0.1:55433/<db>`);
  - `README.md` (three steps: start Loams, set `DATABASE_URL`, run);
  - `test/e2e.test.ts`.
- `examples/quickstart-qdrant/`: the same, with variant B (`QdrantVector` with `apiKey` and `Loams-Namespace`), `.env.example` adding `LOAMS_QDRANT_URL`, `LOAMS_API_KEY`, `LOAMS_NAMESPACE`.
- `docs/quickstart.md`: both variants, local `loams dev` and compose over a tailnet (Tailscale or Headscale, linking the monorepo's `docs/release/preview.md`), with the "never public" rule stated.
- `.github/workflows/conformance-nightly.yml`:
  - `schedule` daily, plus `workflow_dispatch`;
  - matrix `mastra: [pinned, latest, alpha]`; for `latest` and `alpha`, `scripts/vendor-upstream.mjs --out "$RUNNER_TEMP/upstream"` at the tags matching `npm view @mastra/<pkg>@<dist-tag> version`, and examples installed with those versions;
  - `loams: [pinned]`, plus `candidate` when a pre-release of `ostrium-labs/loams` newer than the pin exists;
  - runs every required cell, the ES cell and V7 (`v7_suspended_compute_wakes`: idle past the compute's suspend timeout from Task 0, then a query succeeds);
  - on failure, `scripts/compat-issue.sh` opens or updates one issue titled `Mastra <dist-tag> <version> fails against Loams <version>` with the label `mastra-compat` and the failing tests;
  - on success, `scripts/gen-compat.mjs` updates `COMPATIBILITY.md` through a bot PR.
- `scripts/compat-issue.sh` (modelled on the monorepo's `scripts/ci/sdk-nightly-issue.sh`, with provenance).

**Tests:**
- `quickstart_postgres_remembers_across_threads` and `quickstart_qdrant_remembers_across_threads`: the mock model of Task 0 item 9. Save a fact in thread A and recall it in thread B of the same resource through semantic recall. Assert the recalled message ids.
- Both run in `conformance.yml` (job `examples`, in `CI required`) against published packages.

**Acceptance:** `examples` green; one manual `workflow_dispatch` of the nightly with all three Mastra cells.
**Commit:** `examples: Postgres and Qdrant quickstarts with memory, and the nightly Mastra latest/alpha matrix`.

### Task 11: The starter template and agent-facing docs

**Repo:** `ostrium-labs/loams-mastra`. **Branch:** `template`.

**Files (create):**
- `templates/loams-starter/`, a Mastra project in Mastra's template shape:
  - `package.json` with exact pins;
  - `src/mastra/index.ts`: an agent with memory on Loams Postgres, a RAG tool over `PgVector`, and a two-step workflow;
  - `.env.example`;
  - `README.md` with `npx create-mastra@<pinned CLI> --template <url>` using Task 0 item 5's URL form;
  - `llms.txt` (what the project is, where Loams runs, the env vars, the never-public rule);
  - `.claude/skills/loams/SKILL.md`, a short skill for coding agents: how to start `loams dev`, the env vars, where the docs are.
- `.github/workflows/template.yml`: on changes under `templates/`, runs `create-mastra --template` against the local path (or the PR's branch URL), installs, typechecks, and runs the template's test against `loams dev` and Loams Postgres.
- If Task 0 found that subdirectories are not accepted:
  - `.github/workflows/template-mirror.yml` pushes `templates/loams-starter` to `ostrium-labs/loams-mastra-template` (a GitHub template repository; creating it is an owner action) on each release, with a deploy key scoped to that repository;
  - otherwise no mirror (Q1000).

**Tests:** `template_creates_and_passes` (the workflow above); `examples_pin_exact_versions` covers the template.
**Commit:** `templates: the loams-starter template, llms.txt and a coding-agent skill`.

### Task 12: The upstream docs page (MA0 exit)

**Repos:** `ostrium-labs/loams-mastra` (source of the page); `mastra-ai/mastra` (issue, then PR from a fork under `ostrium-labs`, created only when the issue is approved).

**Files:**
- Create `docs/upstream/loams.mdx` in the shape of `docs/src/content/en/integrations/databases/neon.mdx` at the pinned commit:
  - front matter (`title: "Loams | Database"`, `description`, `packages: ["@mastra/core", "@mastra/pg", "@mastra/qdrant"]`);
  - sections: Quickstart (variant A), Vector search with Qdrant (variant B), Running Loams (local `loams dev`, or compose over a tailnet; never public), Production connections (pg-control's role connection string, `lk_` keys, namespaces), Initialization (`storage.init()`, `disableInit`), Compatibility (link to `COMPATIBILITY.md`), Related.
- Create `docs/upstream/sidebars-entry.md`: the exact `sidebars.js` object to add under Databases, with `customProps.icon` pointing at a Loams icon URL on loams.dev (verify the asset exists, or add it in `ostrium-labs/loams-site` first).

**Steps:**
1. When Tasks 7, 8 and 10 are green, open an issue in `mastra-ai/mastra` with the feature-request template: "docs: add Loams to the Databases integrations (uses `@mastra/pg` and `@mastra/qdrant`; no new package)". Link the green run and `COMPATIBILITY.md`, and note the Neon and ClickHouse precedents (#22486, #25733).
2. Wait for the issue to leave `status: needs triage` and `status: needs approval`. Do not open the PR before then.
3. Fork into `ostrium-labs`, branch `docs/loams-integration`, add the page as `docs/src/content/en/integrations/databases/loams.mdx` and the sidebar entry, and open the PR with `Fixes #<issue>`. Answer every CodeRabbit and Mastra Platform comment.
4. Record the issue and PR numbers as ruling `MA-R12.1`.

**Acceptance (MA0 exit):** Tasks 2–11 merged; the required cells green; the upstream issue open (merging is Mastra's decision and not a gate).
**Commit (in `loams-mastra`):** `docs: the upstream Mastra integrations page for Loams`.

---

## MAR — The Resonate execution engine, upstream (Tasks 13–22)

Runs in parallel with MA0 from Task 2 (owner, 2026-10-20). Layout and code follow Mastra's `workflows/inngest` at `@mastra/inngest@1.10.6` so the package can move into `workflows/resonate` unchanged. Inngest's source may be read as the reference and adapted where it is generic (Apache-2.0, provenance header on any adapted file).

### Task 13: Package skeleton and engine rulings

**Repo:** `ostrium-labs/loams-mastra`. **Branch:** `resonate-skeleton`.

**Files (create):**
- `packages/resonate/package.json`:
  - `@loams/mastra-resonate`, `0.0.0`, `private: true` until Task 21, `type: module`;
  - exports `"."` (import and require, as `@mastra/inngest` ships);
  - dependency `@resonatehq/sdk` `0.11.5` (exact until Task 21, then `^0.11.5`);
  - peer `@mastra/core` `">=1.78.0-0 <2.0.0-0"`;
  - `repository.directory packages/resonate`; `license Apache-2.0`.
- `packages/resonate/{tsdown.config.ts,vitest.config.ts,README.md,CHANGELOG.md}`.
- `packages/resonate/src/{index.ts,execution-engine.ts,workflow.ts,run.ts,codec.ts,types.ts}` with exported names and `TODO(Task 15)` bodies:
  - `init(resonate: Resonate): { createWorkflow, createStep }`;
  - `class ResonateExecutionEngine extends DefaultExecutionEngine`;
  - `class ResonateWorkflow`, `class ResonateRun`.
- `packages/resonate/docker-compose.yaml`: the upstream Resonate server image pinned by digest from Task 0 item 8 (what Mastra's CI would use).
- Vendored `workflows/_test-utils` at the `@mastra/inngest@1.10.6` commit (Task 4's script, `--package @mastra/inngest`) into `test/conformance/upstream/mastra-inngest@1.10.6/`.

**Rulings to record (`MA-R13.n`):**
- the Resonate 0.11.5 call for each hook of §66 §7.2;
- how Mastra's `workflows/_test-utils` is driven: the factory name and its engine adapter interface;
- which test-utils groups are MVP and which wait for Tasks 18–19.

**Tests:** `exports_init_and_engine` (types only).
**Commit:** `resonate: package skeleton in Mastra's workflows layout`.

### Task 14: The Resonate harness (upstream server and Loams Durable)

**Repo:** `ostrium-labs/loams-mastra`. **Branch:** `resonate-harness`.

**Files (create):**
- `test/resonate/start-resonate.sh`: `docker compose -f packages/resonate/docker-compose.yaml up -d`, then waits for the server's health endpoint. Exports `RESONATE_URL` (Task 0's port).
- `test/resonate/start-loams-durable.sh`: reuses `test/conformance/loams/install-loams.sh` and `start-loams.sh`, waits for `127.0.0.1:8001`, and exports `RESONATE_URL=http://127.0.0.1:8001`.
- `test/resonate/package.json` (private `@loams-internal/resonate-tests`), `test/resonate/vitest.config.ts` (reads `RESONATE_BACKEND=upstream|loams`).
- Modify `.github/workflows/ci.yml`: job `resonate` (matrix `backend: [upstream, loams]`, in `CI required`).

**Tests:** `resonate_backend_answers` (creates and reads a durable promise through the SDK on each backend).
**Commit:** `resonate: CI against a pinned upstream Resonate server and Loams Durable`.

### Task 15: Engine MVP

**Repo:** `ostrium-labs/loams-mastra`. **Branch:** `resonate-mvp`.

**Files:** `packages/resonate/src/*` (Task 13's), `packages/resonate/test/*.test.ts`.

**Behaviour (D1212):**
- `init(resonate)` registers each created workflow as one Resonate function under the workflow id. A run is `resonate.run(<runId>, <workflowId>, input)` (Task 13's ruling names).
- `wrapDurableOperation(operationId, fn)` calls `ctx.run` with a deterministic id `<runId>/<operationId>`. Replays return the memoised value.
- `executeStepWithRetry` runs each attempt inside `ctx.run` with Resonate's retry policy set to **no retry**. The outer loop follows Mastra's `retryConfig` (`attempts`, `delay`) and uses `ctx.sleep` between attempts.
- `executeSleepDuration` and `executeSleepUntilDate` call `ctx.sleep`.
- `requiresDurableContextSerialization()` returns `true`, so the request context is serialised with `serializeRequestContext`.
- `codec.ts` gives JSON-safe encoding of step results and errors (name, message, stack, `cause` chain) and refuses functions, symbols and cycles with a typed error.
- Snapshots persist through the Mastra instance's `workflows` storage domain, as the default engine does.
- Status mapping: Resonate promise states `pending | resolved | rejected | rejected_canceled | rejected_timedout` → Mastra `running | success | failed | canceled | failed` (verify the state names at 0.11.5).
- `onStart` and `onFinish` callbacks fire once per run, not once per replay.

**Tests** (`packages/resonate/test/`, each run on both backends):
- `runs_to_completion`;
- `steps_are_memoised_on_replay`;
- `sleep_and_sleep_until_are_durable`;
- `nested_workflow_runs_as_child`;
- `retries_follow_mastra_retry_config_only` (a step failing twice with `attempts: 3` runs 3 times in total, never 9);
- `operation_ids_are_deterministic` (two runs with the same graph produce the same ids);
- `codec_round_trips_errors`;
- `on_finish_fires_once`;
- `status_maps_each_state`.

**Commit:** `resonate: ResonateExecutionEngine MVP: memoised steps, durable sleeps, Mastra-owned retries`.

### Task 16: Mastra's workflow test utilities

**Repo:** `ostrium-labs/loams-mastra`. **Branch:** `resonate-testutils`.

**Files:**
- Create `test/resonate/suites/testutils.test.ts`: drives the vendored `workflows/_test-utils` with the Resonate engine, the way `workflows/inngest`'s tests do (Task 13's ruling).
- Create `test/conformance/expected-failures/resonate-{upstream,loams}.json`. List suspend/resume, streaming, time travel and restart groups, linked to Tasks 18–19 issues in `ostrium-labs/loams-mastra`.
- Modify `ci.yml`: the `resonate` job also runs this suite with the compat reporter.

**Commit:** `resonate: run Mastra's workflow test utilities on both Resonate backends`.

### Task 17: Crash and resume on Loams

**Repo:** `ostrium-labs/loams-mastra`. **Branch:** `resonate-crash`.

**Files:** create `test/resonate/crash.test.ts` and `test/resonate/fixtures/worker.ts` (a separate worker process).

**Test** `crash_mid_workflow_resumes_without_rerunning_steps`:
1. Start the worker.
2. Start a 3-step workflow whose second step blocks on a file.
3. `kill -9` the worker after step 1 has completed.
4. Restart the worker and release the file.
5. Expect success, with step 1's side-effect counter at 1 and steps 2 and 3 at 1.

A second variant `kill -9`s `loams` itself, restarts `loams dev` on the same data directory, and expects the same result (Loams backend only).

**Commit:** `resonate: crash-resume tests for worker and engine restarts`.

### Task 18: Suspend and resume

**Repo:** `ostrium-labs/loams-mastra`. **Branch:** `resonate-suspend`.

**Behaviour:**
- A step that calls `suspend(payload)` creates an externally settled durable promise `<runId>/<stepPath>/resume` (`ctx.promise`, Task 13's ruling) and awaits it.
- `run.resume({ step, resumeData })` settles it with `resumeData`. A second resume of the same step is refused through Mastra's persisted resume claim.
- Snapshot `suspendedPaths` and `resumeLabels` are kept, as the default engine keeps them.

**Tests:** `suspend_then_resume`, `double_resume_is_refused`, `resume_after_worker_restart`, `nested_suspend_resume`. The matching expected failures are removed from Task 16's lists.
**Commit:** `resonate: suspend and resume as externally settled durable promises`.

### Task 19: Streaming and watch

**Repo:** `ostrium-labs/loams-mastra`. **Branch:** `resonate-stream`.

**Behaviour:** `run.stream()` and `watch()` publish step events through the Mastra instance's `PubSub` (any implementation), using the event shapes the default engine emits. Events emitted inside a replayed `ctx.run` are not re-published (the publish itself is wrapped in `ctx.run`).

**Tests:** `stream_emits_each_step_once`, `watch_sees_suspend_and_resume`, `no_duplicate_events_after_replay`. Expected failures shrink accordingly.
**Commit:** `resonate: streaming and watch through Mastra's PubSub without duplicates on replay`.

### Task 20: Docs and an example

**Repo:** `ostrium-labs/loams-mastra`. **Branch:** `resonate-docs`.

**Files (create):**
- `packages/resonate/README.md` (install, `init`, workers, retries owned by Mastra, limits);
- `docs/resonate.md` (Loams Durable specifics: `loams dev` on 8001; on shared deployments Durable is instance-wide and needs an operator or org key, Q1012; never public);
- `examples/durable-workflow/` (exact pins; a workflow with a sleep and a human-approval suspend; `test/e2e.test.ts` against `loams dev`);
- `docs/upstream/resonate.mdx` (the page for Mastra's `integrations/deploy/`, Inngest's shape).

**Tests:** `durable_workflow_example_completes` (in the `examples` job).
**Commit:** `resonate: README, Loams Durable guide, example and the upstream docs page`.

### Task 21: Publish `@loams/mastra-resonate` 0.1.0

**Repo:** `ostrium-labs/loams-mastra`. **Branch:** `resonate-release`.

**Steps:**
1. Set `private: false`, version through a changeset (`0.1.0`), and `@resonatehq/sdk` `^0.11.5`.
2. Merge the changesets version PR. `release.yml` publishes with provenance from the `npm` environment, after an owner-named reviewer approves.
3. Check `npm view @loams/mastra-resonate@0.1.0 dist.attestations` shows the provenance attestation, and `npm audit signatures` passes in a clean project.

**Acceptance:** the package is on npm with provenance; the `durable-workflow` example installs it from npm and passes.
**Commit:** `resonate: release 0.1.0` (the changeset).

### Task 22: Contribute `@mastra/resonate` upstream (MAR exit)

**Repos:** `mastra-ai/mastra` (issue, then PR from the `ostrium-labs` fork); `ostrium-labs/loams-mastra` (tracking).

**Steps:**
1. Open a feature-request issue: "Resonate execution engine: `@mastra/resonate` in `workflows/resonate`".
   - Content: what Resonate is (Apache-2.0, durable promises), the engine design (§66 §7.2), the MVP scope and what follows, and the green CI on an upstream Resonate server.
   - Offer to maintain it with Mastra.
   - Mention Resonate's maintainers (Q1014).
2. Wait for approval. No PR before it.
3. On the fork, branch `workflows/resonate`:
   - copy `packages/resonate` to `workflows/resonate`;
   - rename the package to `@mastra/resonate`;
   - switch to `@internal/lint`, `@internal/types-builder`, `@internal/workflow-test-utils` (`workspace:*`) and `@mastra/core` `workspace:*`;
   - keep `docker-compose.yaml`, `pretest`/`posttest` like `stores/qdrant`;
   - add a changeset, and `docs/src/content/en/integrations/deploy/resonate.mdx` from `docs/upstream/resonate.mdx`, with its sidebar entry;
   - add a short `workflows/README.md` line if Mastra lists engines there.
4. Answer every review comment. Fixes land in both copies until merge.
5. When `@mastra/resonate` is released:
   - publish `@loams/mastra-resonate` one last minor that re-exports it;
   - `npm deprecate @loams/mastra-resonate "Use @mastra/resonate"`;
   - switch the example and docs to `@mastra/resonate`.
6. If Mastra declines, keep `@loams/mastra-resonate` (Q1013) and record ruling `MA-R22.1`.

**Acceptance (MAR exit):** `@loams/mastra-resonate` published; the upstream issue open with a linked PR or a recorded decision.

---

## MA1 — Loams API fixes in the monorepo (Tasks 23–27)

### Task 23: Qdrant `DELETE /collections/{c}/index/{field}`

**Repo:** `ostrium-labs/loams`. **Worktree and branch:** `~/Documents/Ostriumlabs/loams-wt/ma1-qdrant-index-delete`, `ma1-qdrant-index-delete`.

**Files:**
- `crates/loams-qdrant/src/rest.rs`:
  - remove `("DELETE", "/collections/{collection_name}/index/{field_name}")` from `UNSUPPORTED`;
  - add the route `.route("/collections/{collection_name}/index/{field_name}", delete(delete_field_index))`;
  - add a handler `delete_field_index` taking `wait` and `ordering` query parameters like `create_field_index`.
- `crates/loams-qdrant/src/schema.rs`:
  - `pub(crate) async fn delete_field_index(gw, ctx, collection, field_name, wait) -> Result<UpdateResult, GatewayError>`: normalise the key as `payload_index_field` does; if a `payload_index.<key>` field exists, add the key to the annotation `qdrant.payload_index.retired` (a sorted, comma-separated set) through `add_fields(ns, coll, vec![], vec![], annotations)`; answer `Completed`;
  - a missing index answers `Completed` too (verify against the oracle);
  - `fn retired_payload_indexes(schema: &CollectionSchema) -> BTreeSet<String>`;
  - collection info (around `:613`) leaves retired keys out of `payload_schema`;
  - `create_field_index` with the same kind on a retired key removes it from the set; a different kind stays `Unsupported("changing payload index type")`.
- `crates/loams-qdrant/src/grpc.rs`: `Points/DeleteFieldIndex` (`:955`) calls the same `schema::delete_field_index`.
- `crates/loams-qdrant/src/lib.rs`: the divergence list gains "deleting a payload index retires it; filters on the key still work (D134); re-creating it with another type is 501".
- `docs/design/06-search-and-vector.md` §8: the same line.

**Tests:**
- `crates/loams/tests/qdrant/collections.rs`: `delete_field_index_hides_it_from_payload_schema`, `delete_field_index_keeps_filters`, `recreate_same_type_after_delete`, `recreate_with_other_type_unsupported`, `delete_missing_field_index_completes`, `delete_field_index_grpc`;
- `crates/loams-qdrant/tests/it/schema.rs`: `retired_set_round_trips`;
- `crates/loams-qdrant/tests/python/smoke.py`: new check `payload_index_delete` in both modes.

**Acceptance:** `cargo test -j 4 -p loams-qdrant`; `cargo test -j 4 -p loams --test qdrant`; the `qdrant-client` CI job green; then in `loams-mastra`, the `deletePayloadIndex` entries leave `expected-failures/qdrant*.json` in the release after the next Loams release (the reporter flags the unexpected pass against a candidate first).
**Commit:** `qdrant: serve DELETE payload index by retiring it (MA1 Task 23, D1210)`.

### Task 24: Elasticsearch `GET /_cat/indices`

**Repo:** `ostrium-labs/loams`. **Worktree and branch:** `~/Documents/Ostriumlabs/loams-wt/ma1-es-cat-indices`, `ma1-es-cat-indices`.

**Files:**
- Create `crates/loams-es/src/cat.rs`: `pub(crate) async fn cat_indices(ctx, Query<CatParams>, Option<Path<String>>) -> Response`.
  - One row per index (alias-resolved names, wildcard and comma lists through `names.rs`), with `health` (`green`), `status` (`open`), `index`, `uuid` (a stable id from the collection), `pri` (`1`), `rep` (`0`), `docs.count` (live documents at `STRONG`), `docs.deleted` (`0`), `store.size` and `pri.store.size` (from `size_bytes`; with `bytes=b|kb|mb|gb` as ES formats).
  - `format=json` returns an array of objects with string values, as ES does. The default is the aligned text table, with `v` adding the header row.
  - `h=<cols>` selects columns, `s=<col>[:asc|desc]` sorts, `health=` filters, and `expand_wildcards` is accepted.
  - Unknown parameters answer 400 the way the rest of the gateway does.
- `crates/loams-es/src/lib.rs`: `mod cat;` and routes `.route("/_cat/indices", get(cat::cat_indices))` and `.route("/_cat/indices/{index}", get(cat::cat_indices))`, inside the existing layers (`X-Elastic-Product`).
- `crates/loams-es/src/lib.rs` divergence list: `pri`/`rep` are fixed, `health` is always green, and sizes are Loams' `size_bytes`.

**Tests:**
- `crates/loams/tests/es/admin.rs`: `cat_indices_json_lists_indices`, `cat_indices_text_with_header`, `cat_indices_h_and_s`, `cat_indices_one_index_and_wildcard`, `cat_indices_bytes_units`;
- `conformance/es-client/test_client.py`: `test_cat_indices_json` (`es.cat.indices(format="json")` sees the test index and its `docs.count`);
- `conformance/es-client/oracle.py`: a probe `cat_indices_json_columns` comparing column names and the `docs.count` of a fresh index with the 8.19 oracle (values that cannot match, such as `uuid` and sizes, are masked).

**Acceptance:** `cargo test -j 4 -p loams-es`; `cargo test -j 4 -p loams --test es`; the `es-client` job green.
**Commit:** `es: GET /_cat/indices with json and text formats (MA1 Task 24)`.

### Task 25: `@qdrant/js-client-rest` in the `qdrant-client` CI job

**Repo:** `ostrium-labs/loams`. **Worktree and branch:** `~/Documents/Ostriumlabs/loams-wt/ma1-qdrant-js`, `ma1-qdrant-js`.

**Files:**
- Create `crates/loams-qdrant/tests/js/package.json`:
  - private, `type: module`;
  - dependencies `"qdrant-js-1.18": "npm:@qdrant/js-client-rest@1.18.0"` and `"qdrant-js-1.19": "npm:@qdrant/js-client-rest@1.19.0"`.
- Create `crates/loams-qdrant/tests/js/pnpm-lock.yaml`, installed with `pnpm install --ignore-workspace --frozen-lockfile --ignore-scripts`.
- Create `crates/loams-qdrant/tests/js/smoke.mjs`. Plain asserts, one function per check, fresh collections named `smoke_js_<check>_<uuid>`; every call `@mastra/qdrant` 1.1.4 makes:
  - the client's server-version check;
  - `getCollections`, `getCollection`, `createCollection` (unnamed and named vectors), `collectionExists`, `deleteCollection`;
  - `upsert` with UUID and integer ids; `query` with filter, `with_payload`, `with_vector` and `using`;
  - `scroll`, `setPayload` (by ids and by filter), `updateVectors`, `delete` (by ids and by filter);
  - `createPayloadIndex` for every type Mastra sends; `deletePayloadIndex` (after Task 23);
  - `apiKey` plus `headers: {'Loams-Namespace': ...}` reaching a non-default namespace (runs only when `LOAMS_QDRANT_API_KEY` is set).

  It prints `qdrant-js <version>: <n> checks passed` last.
- Create `crates/loams-qdrant/tests/js/run.sh <version>`, which selects the alias and runs `node smoke.mjs --version <v> --url "${LOAMS_QDRANT_URL:-http://127.0.0.1:6333}"`.
- Modify `.github/workflows/ci.yml`, job `qdrant-client`:
  - add `actions/setup-node` (Node 22) and `pnpm/action-setup` (pinned by SHA, as elsewhere in `ci.yml`);
  - add steps `qdrant-js 1.18.0` and `qdrant-js 1.19.0`, each `tee`d and `grep -q`'d for its last line, like the Python steps.
- `scripts/ci/npm-licences.mjs`: if it walks every lockfile, confirm the new lockfile passes (Apache-2.0).

**Tests:** the smoke itself.
**Acceptance:** the `qdrant-client` job green with both JS steps. Before Task 23 merges, `deletePayloadIndex` is skipped with a printed `skip: deletePayloadIndex (MA1 Task 23)`; Task 23's PR removes the skip.
**Commit:** `ci: run @qdrant/js-client-rest 1.18 and 1.19 against the Qdrant gateway (MA1 Task 25)`.

### Task 26: Gate findings

**Repos:** `ostrium-labs/loams` (fixes), `ostrium-labs/loams-mastra` (expected-failure lists), `ostrium-labs/loams-postgres` (extension-manifest findings only).

**Procedure:**
- Every `mastra-compat` issue opened by Tasks 7–9 (or the nightly) gets a triage comment:
  - **fix**: a Loams bug or gap worth closing; one monorepo PR shaped like Task 23 (files, Rust tests, the client smoke, a divergence-list update);
  - **divergence**: documented, stays in the expected-failure list with the reason;
  - **upstream**: a Mastra bug, with a Mastra issue linked.
- Every fix PR is titled `<area>: <behaviour> (MA1 Task 26, #<issue>)`.
- After the next Loams release candidate, the `candidate` nightly cell shows the unexpected pass, and the matching `loams-mastra` PR removes the entry.
- Pgvector or extension findings go to `ostrium-labs/loams-postgres`'s extension manifest, and the pin bump follows NF1's process.

**Acceptance:** no `mastra-compat` issue older than 14 days without a triage label.

### Task 27: The monorepo nightly runs the Mastra gate (Q1001)

**Repo:** `ostrium-labs/loams`. **Worktree and branch:** `~/Documents/Ostriumlabs/loams-wt/ma1-mastra-nightly`, `ma1-mastra-nightly`.

**Files:**
- Modify `.github/workflows/nightly.yml`: a job `mastra-gate`.
  1. Build `loams` (`cargo build -p loams --features durable --locked`).
  2. `actions/checkout` `ostrium-labs/loams-mastra` at `main` into `.loams-mastra`. This is a CI checkout of our own public repository.
  3. Run its `pinned` cells with `LOAMS_BIN=$GITHUB_WORKSPACE/target/debug/loams` (Task 5's harness honours `LOAMS_BIN` instead of installing the `.deb`).
  4. On failure, open or update an issue `Mastra gate fails on dev` with the label `mastra-compat`.

  It is not a required PR check.
- `docs/ci/README.md` (or the nightly section that lists jobs): one row.

**Acceptance:** one manual `workflow_dispatch` run green, or failing only on listed expected failures.
**Commit:** `ci: nightly Mastra gate against a dev build (MA1 Task 27, Q1001)`.

---

## MA2 — Native `@loams/mastra` (Tasks 28–37; task level)

Gate: #884 merged and `@loams/client` released with the typed app modules (D1215). Each task: `ostrium-labs/loams-mastra`, one PR, tests first, Mastra's vendored suites where they exist.

| Task | Content | Files | Tests | Gate |
|---|---|---|---|---|
| 28 | Rulings: `@loams/client` version and the calls used; the pinned core's Studio trace-read methods (Q1004); filter semantics of `json` paths; `search_after` on `createdAt`; whether `sql.query` is released | this file | — | #884 released |
| 29 | `LoamsFilterTranslator` and `LOAMS_PROMPT` | `packages/mastra/src/vector/{filter.ts,prompt.ts}` | an operator table test per Mongo operator; `elem_match_refused`; `unanchored_regex_refused` | — |
| 30 | `LoamsVector` (§66 §8.3): a collection per index, `embedding`/`metadata`/`text`, hybrid with `queryText`, `describeIndex`, `listIndexes` with Memory's naming | `packages/mastra/src/vector/index.ts` | vendored `createVectorTestSuite`; `hybrid_beats_vector_on_keyword_query`; `memory_semantic_recall_e2e` | 29 |
| 31 | `LoamsStore` memory domain: one collection per store with `kind`, atomic thread + message saves, UUIDv7, listings, context windows, resources and working memory | `packages/mastra/src/storage/domains/memory.ts` | vendored `createTestSuite` memory group; `thread_and_messages_saved_atomically` | 28 |
| 32 | `workflows` and `scores` domains; capabilities false for fencing and concurrent updates | `packages/mastra/src/storage/domains/{workflows,scores}.ts` | vendored groups; `fencing_declared_unsupported` | 31 |
| 33 | `prune()` retention; composition with `@mastra/pg` for the other domains; docs | `packages/mastra/src/storage/{index.ts,retention.ts}`, `docs/storage.md` | `prune_deletes_older_than_max_age`; `composite_with_pg_passes_suite` | 32 |
| 34 | `LoamsPubSub` on Streams | `packages/mastra/src/pubsub/index.ts` | `subscribe_from_offset_replays`; `idempotent_publish`; Mastra's evented-workflow test with `LoamsPubSub` | 28 |
| 35 | `LoamsCache` | `packages/mastra/src/cache/index.ts` | `ttl_expires`; `lists_round_trip`; `increment_atomic_in_process` | 28 |
| 36 | `LoamsExporter` and a House table for spans; minimal observability domain for Studio (Q1004) | `packages/mastra/src/observability/`, `examples/traces-in-house/` | `exports_span_lifecycle_once`; `flush_on_shutdown`; `studio_trace_list_reads_house` | 28 |
| 37 | Publish `@loams/mastra` 0.1.0 with provenance; README, docs and the quickstart's native variant; COMPATIBILITY rows | `packages/mastra/`, `docs/`, `examples/quickstart-native/` | the examples job; `npm audit signatures` | 29–36 |

## MA3 — Loams Memory and MCP (Tasks 38–41; task level)

Gate: MM1a's MVP gate (MM1 Task 13) passed and MM1 Task 44's TypeScript memory facade released; #847 for Task 40.

| Task | Content | Files | Tests |
|---|---|---|---|
| 38 | `createLoamsMemoryProcessors` (input: `Search` → framed context; output: `Add` with `addMemory`), identity from `requestContext` (`resourceId` → user space, `threadId` → session space) | `packages/mastra/src/memory/processors.ts` | `injects_framed_context`; `adds_turn_after_response`; `add_memory_never_skips`; `mastra_agent_remembers_across_threads` (MM1 Task 50's test, now here) |
| 39 | `createLoamsMemoryTools` (`loamsRemember`, `loamsRecall`, `loamsContext`, code-graph tools when mounted); observational-memory observations ingested as episodes | `packages/mastra/src/memory/tools.ts` | `tools_validate`; `om_observation_ingested_as_episode` |
| 40 | `loams-mcp` from `@mastra/mcp`'s `MCPClient` over streamable HTTP with an `lk_` key | `examples/mcp-client/` | `lists_and_calls_loams_mcp_tools` |
| 41 | LongMemEval through `@mastra/longmemeval`, observational memory on and off, harness and results published (after the owner's budget, Q1010) | `bench/longmemeval/` | a reproducible run script; numbers in the README and on loams.dev |

## Adoption (Tasks 42–46)

| Task | Content | Repo | When |
|---|---|---|---|
| 42 | loams.dev: a Mastra page in Learn (quickstarts, Resonate engine, compatibility matrix) and a post per milestone (MA0 exit, MAR exit, MA2 exit) | `ostrium-labs/loams-site` | After Task 12, 22, 37 |
| 43 | `examples/support-agent`: memory across threads, hybrid RAG, a durable approval workflow on Resonate, traces in House | `ostrium-labs/loams-mastra` | MA0 version after Task 10; extended after Tasks 20 and 37 |
| 44 | `COMPATIBILITY.md` badge in the README and on loams.dev | `ostrium-labs/loams-mastra`, `ostrium-labs/loams-site` | After Task 10 |
| 45 | `ECOSYSTEM.md` in the monorepo lists `loams-mastra` under framework integrations | `ostrium-labs/loams` | After Task 12 |
| 46 | Business development (Mastra platform DB provider, co-marketing) only with usage to show | Owner | After MA2 (Q1007) |

## Owner actions

- §66 and its defaults (D1202–D1227, Q1000–Q1014) were approved on 2026-10-20; nothing further for the design.
- Name the reviewers of the `npm` environment in `loams-mastra` (Task 1).
- Create `ostrium-labs/loams-mastra-template` only if Task 0 item 5 finds that `create-mastra` cannot take a subdirectory (Task 11, Q1000).
- Approve a model-token budget before LongMemEval (Task 41, Q1010).
- Add `loams-mastra` to the workspace's list of active repositories, with its worktree folder `loams-mastra-wt/`.

## Rulings made during execution

### Task 0 (2026-10-20)

- **MA-R0.1 Mastra tag commits.** Annotated tags, peeled:
  - `@mastra/core@1.78.0`, `@mastra/pg@1.31.2`, `@mastra/memory@1.38.0`, `@mastra/inngest@1.10.6` → `6a465f9d1da730e179c44bfb4ef3e6ec398ba81f` (one release commit).
  - `@mastra/qdrant@1.1.4` → `8b35a6c1d6148b24aea94a5d0493d7d7f3c7a5d5`.
  - `@mastra/elasticsearch@1.4.4` → `81299c921d6f499d4c62f14c38e4d4d88b4fbfae`.
  - `stores/_test-utils/package.json`: `@internal/storage-test-utils`, private, no `dependencies`, peers `@mastra/core: *` and `vitest: catalog:`; version 0.0.140 at `6a465f9`, 0.0.135 at the qdrant commit, 0.0.138 at the elasticsearch commit.
  - `workflows/_test-utils/package.json`: `@internal/workflow-test-utils` 0.0.31 at all three commits; peers as above; dev `@ai-sdk/provider-v5` (`npm:@ai-sdk/provider@2.0.3`), `@internal/ai-sdk-v4`, `@internal/ai-sdk-v5`, `@mastra/libsql` (workspace) and `zod ^3.25.76`.
  - Mastra's catalog at `6a465f9`: `vitest` 4.1.11, `@vitest/coverage-v8` 4.1.11, `typescript ^7.0.2`, `tsx ^4.23.1`; `packageManager` pnpm 11.21.0. The vendored suites run unchanged under vitest 5.0.5 against Loams (checked locally with the qdrant and pg vector suites), so `loams-mastra` may use vitest 4.1.11 (to match) or 5.x.
  - The vendored qdrant suite comes from the `@mastra/pg` commit `6a465f9` in practice too: `stores/qdrant` there is still version 1.1.4.
- **MA-R0.2 Loams release v0.1.0.**
  - `loams_0.1.0_amd64.deb`: `38858af4414074ccceb2cd6b1f67034ed69255737dd2b0df19584dcba1ba13b0`. `loams-compose-0.1.0.tar.gz`: `1a0df7c916dcfc94974462f1d4a2a8c26a4e9ea3ed938503c600deef19f95cc2`. Both match `SHA256SUMS` and the downloads.
  - `SHA256SUMS.asc` verifies ("Good signature") with `docs/release/loams-release-key.asc`, fingerprint `333A C945 99D5 A1EB 2A41 6D1E 83DC 134D 8CAF 895D` (ed25519, Loams Release Signing Key, expires 2028-10-15). The `.deb` is also debsigned (`signing.json`: `nfpm-debsign`).
  - The `.deb` holds `/usr/bin/loams` (0.1.0), `/usr/bin/git-remote-loams` and systemd units. `loams dev` defaults: Qdrant `127.0.0.1:6333`, ES `127.0.0.1:9200`, Durable `--durable-listen` `127.0.0.1:8001` (the release build has `durable`; `--no-durable` exists). Extracting needs `dpkg-deb -x` on the runner (ubuntu has it).
  - Key minting: `loams auth new-key --label <label> --append "$LOAMS_API_KEYS_FILE"` prints the `lk_` key once on stdout and appends the hashed line to the file (created 0600); run with `LOAMS_AUTH_MODE=apikey LOAMS_API_KEYS_FILE=<file>`.
  - With apikey auth, the Qdrant gateway answers `GET /` without a key and `401` on `/collections` without one; namespaces are created on first use; `Loams-Namespace` must be a plain name (`a/b` is refused with 400).
- **MA-R0.3 Loams Postgres.** The compose tarball unpacks to `loams-compose-0.1.0/{loams-postgres-dev,release,upgrade-path.toml}`. `loams-postgres-dev/compose.yaml` (project `loams-neon`) pins `loams-postgres-storage@sha256:4dcec4cb…` and `loams-postgres-compute-v17@sha256:210b7141…` and now has a one-shot `pg-bootstrap` service that creates the tenant and timeline, so `docker compose up -d` alone starts `compute1` on `127.0.0.1:55433`. Host ports: 9000 (RustFS), 9898 (pageserver), 7676 (safekeeper), 55433/55434 (computes). Credentials `cloud_admin` / `cloud_admin`, database `postgres`. Checked locally on the same images (ports remapped): PostgreSQL 17.11, pgvector **0.8.0** (`halfvec`, `sparsevec`, HNSW work), a non-superuser role can `CREATE EXTENSION vector`; `pg_advisory_xact_lock`, `FOR UPDATE SKIP LOCKED` and `CREATE SCHEMA` work; pg_partman 5.1.0, timescaledb 2.17.1 and pg_cron 1.6 are available but not installed. The time and disk on a GitHub-hosted runner are **deferred to Task 5's acceptance run** (not measurable without a workflow).
- **MA-R0.4 PgDog.** The monorepo pins PgDog v0.1.60 as `ghcr.io/pgdogdev/pgdog@sha256:25d1908886f595a2e3b00ec59783326712e3c0f75f91d29f032ff712b30f2266` (`deploy/pgdog/compose.yaml`, licence record in `deploy/pgdog/LICENSE-NOTICE.md`). Use it by that digest in transaction mode. It is AGPL-3.0 and used only as a CI service container, not a dependency, so `scripts/licences.mjs` does not see it; the workflow must not redistribute it. Whether it runs on a runner is checked in Task 8.
- **MA-R0.5 `create-mastra --template`.** At CLI 1.34.2 (`mastra@1.34.2` → `6a465f9`), `parseGitHubRepositoryUrl` in `packages/cli/src/commands/create/create.ts` accepts only `https://github.com/<owner>/<repo>` (exactly two path parts, no query or hash); a `/tree/main/templates/…` URL is refused ("Invalid GitHub repository URL"), and anything else is looked up by name in Mastra's template list, so a local path is not accepted either. Cloning is `npx degit owner/repo[#branch]`, falling back to `git clone`. So Q1000's mirror is needed: `ostrium-labs/loams-mastra-template` (owner action), and CI for Task 11 tests the template by copying it (not through `create-mastra`) plus one `create-mastra --template https://github.com/ostrium-labs/loams-mastra-template` run once the mirror exists.
- **MA-R0.6 pnpm 11.27.1 keys** (from pnpm 11.27.1's own sources and changelog): `minimumReleaseAge: 4320` (minutes), `minimumReleaseAgeExclude: ['@loams/*']`, and the build-script allowlist is **`allowBuilds`** (a map of package name to `true`/`false`); `onlyBuiltDependencies`, `neverBuiltDependencies` and `ignoredBuiltDependencies` were replaced by it in pnpm 11 and are ignored. `strictDepBuilds` is on by default in pnpm 11, so an unlisted build script fails the install. Also available: `minimumReleaseAgeStrict`, `trustPolicy`.
- **MA-R0.7 `QdrantVector` headers.** `@mastra/qdrant` 1.1.4: `constructor({ id, ...qdrantParams }: QdrantClientParams & { id: string })` passes everything to `new QdrantClient(qdrantParams)`, so `apiKey` and `headers: { 'Loams-Namespace': … }` reach Loams. Verified: Mastra's Qdrant suite run with an `lk_` key and a namespace header against Loams 0.1.0.
- **MA-R0.8 Resonate.**
  - `@resonatehq/sdk` 0.11.5 (published 2026-09-03). There are no `Resonate.remote`/`Resonate.local` factories in 0.11.5: `new Resonate({ url, group, pid, ttl, token, timeout, … })` is remote when a URL is given (or `RESONATE_URL`) and uses an in-memory `LocalNetwork` otherwise. Client: `register(name, fn)`, `run(id, fn|name, …args)`, `rpc(id, …)`, `beginRun`, `beginRpc`, `get(id)`, `schedule(name, cron, …)`, `options(…)`, `stop()`, and `promises` (`create`, `get`, `resolve`, `reject`, `cancel`). Context: `ctx.run`, `ctx.rpc`, `ctx.beginRun`, `ctx.beginRpc`, `ctx.detached`, `ctx.sleep(ms | { … })`, `ctx.promise({ timeout, data, tags })`, `ctx.date`, `ctx.options`, `ctx.panic`, `ctx.assert`. A durable promise from `ctx.promise()` is settled externally with `resonate.promises.resolve(id, { data })` (or `reject`/`cancel`), which against Loams goes through `POST /durable/`.
  - Upstream server: `ghcr.io/resonatehq/resonate:v0.9.8@sha256:cc226dbe25d6ad64db637d5a5e5f407fedd4cf011f4e77f4d8883ba5415289c4` (2026-06-04, the latest release; binaries `resonate_linux_x86_64.tar.gz` on the release). `resonate serve` listens on `0.0.0.0:8001` (HTTP) and `0.0.0.0:9090` (metrics) by default. That SDK 0.11.5 interoperates with server v0.9.8 is **unverified; Task 14 checks it** (the SDK is three months newer than the server).
- **MA-R0.9 Mock model.** `import { createMockModel } from '@mastra/core/test-utils/llm-mock'` (an exported subpath of `@mastra/core` 1.78.0); `createMockModel({ mockText, version: 'v2' })` returns a `MockLanguageModelV2` (provider `mock-provider`, model `mock-model-id`). The same module exports `MastraLanguageModelV2Mock`, `MockProvider` and `simulateReadableStream`. Semantic recall in the e2e tests still needs an embedder: use a deterministic local embedder in the test, not a network model.
- **MA-R0.10 CodeRabbit.** The organisation install covers `loams-mastra`: CodeRabbit (plan Advanced) commented on PR #1, but skipped the review because "This repository does not receive automatic reviews because it has fewer than 10 stars". Until the repository has 10 stars, a review is requested per PR with the comment's "Trigger review" checkbox or `@coderabbitai review`; the Task 1 ruleset therefore cannot require a CodeRabbit pass and requires the CI checks only.

### MA0 execution (2026-10-20)

- **MA-R1.1 Repository settings.** `ostrium-labs/loams-mastra` is public, default branch `main`, squash merge only, head branches auto-deleted, private vulnerability reporting and Discussions on, topics `loams`, `mastra`, `ai-agents`, `typescript`, `resonate`, `durable-execution`. Rulesets: `main: branch safety` (no deletion, no force push, linear history, required checks `CI required`, `DCO sign-off`, `Conformance required`; organisation admins bypass) and `release tags: immutable` (`refs/tags/v*`, no deletion or update; organisation admins bypass). No required review (MA-R0.10). The GitHub environment `npm` exists with no reviewers: naming them is an owner action. npm trusted publishing for `@loams/mastra` and `@loams/mastra-resonate` is not configured yet (nothing is published while both packages are private).
- **MA-R2.1 Scaffold.** As specified, with `allowBuilds` (MA-R0.6) instead of `onlyBuiltDependencies`; `allowBuilds: { esbuild: false }` was added with the quickstarts (tsx's esbuild postinstall only checks its platform binary). NOTICE also names Resonate as a planned source for `packages/resonate`.
- **MA-R2.2 TypeScript 7** (owner, 2026-10-20). The repository moves from TypeScript 5.9.3 to **7.0.2**, Mastra's own toolchain (`typescript: ^7.0.2` in its pnpm catalog at the pinned tags), because we contribute upstream (loams-mastra PR #34). This supersedes the Tech stack's "TypeScript 5.9.3". Minor TypeScript bumps stay ignored by Dependabot and follow Mastra.
- **MA-R3.1 CI.** `CI required` aggregates `checks` and `actionlint` (and `harness` from Task 5). A workflow cannot `needs:` a job in another file, so `conformance.yml` has its own aggregator, `Conformance required`, and the ruleset requires both. actionlint is the release binary 1.7.12, checksum-verified. Dependabot ignores semver-major updates for every ecosystem, and minors of `typescript`, `vitest` and `tsdown`, because test tooling follows the versions Mastra's own suites use (PR #17; Dependabot's TypeScript 7, vitest 5, @types/node 26 and tsdown 0.23 PRs were closed; TypeScript 7 then landed by hand, MA-R2.2).
- **MA-R4.1 Vendoring.** The workspace globs are `test/conformance/upstream/*/{stores,packages,workflows}/*`: the plan's `test/conformance/upstream/*/*` does not reach `stores/<name>`. `@mastra/pg` also vendors `packages/_internal-core` (its source imports the unpublished `@internal/core`). Imports Mastra's monorepo hoists (`zod`, `dotenv`) are exact devDependencies from `pins.json` `thirdParty`; Mastra's lint and build tooling is dropped from vendored `package.json` files; `UPSTREAM.json` records the commit date rather than the vendoring date, so a re-vendor is byte-identical.
- **MA-R5.1 Harness.** The release ships `loams_0.1.0_amd64.deb` (MA-R0.2); off Debian the harness unpacks it with `ar` + `tar`. `start-postgres.sh` creates a `postgres` superuser when missing, because Mastra's pg suites `REASSIGN OWNED BY … TO postgres` and Loams Postgres's superuser is `cloud_admin`. `LOAMS_DEV_ARGS` and `LOAMS_*_LISTEN` move every listener for local runs.
- **MA-R6.1 Reporter.** The Loams version in `results/*.json` comes from `pins.json` (the Qdrant gateway's `GET /` reports the Qdrant compatibility version, 1.19.1, recorded separately as `server.qdrantCompat`).
- **MA-R7.1 Qdrant cells.** `@qdrant/js-client-rest` reaches the vendored suite through a vitest `resolve.alias` to `suites/qdrant/qdrant-client.ts` (a subclass of `QdrantClient` that merges the URL, the `lk_` key and `Loams-Namespace`), not a `vi.mock` in `setupFiles`: same effect, and it also applies to the vendored source's own import. 11 expected failures in both cells: ostrium-labs/loams#897 (payload-index delete, Task 23), #898 (geo), #899 (`has_vector`), #900 (`nested`). The named-vector `describeIndex` case does not fail at 1.1.4.
- **MA-R8.1 pg cells.** The pg cell passes with an empty expected-failures list (3314 passed, 210 skipped locally) and runs with `retry: 2`, because tests that give a one-connection pool a 500 ms connect timeout fail intermittently on Loams Postgres (ostrium-labs/loams#904). The checks' findings go to `results/checks/postgres.json` (the reporter owns `results/pg.json`): pgvector 0.8.0, PostgreSQL 17.11, partition mode `native`. The `pg-pooler` cell is **deferred to Task 26**: the vendored `*.pooler.test.ts` hard-code PgBouncer on `localhost:6433` and a direct Postgres on `localhost:5437` with `postgres:postgres`, so PgDog needs a port shim first; V3 runs direct only.
- **MA-R9.1 Elasticsearch cell.** 9 expected failures: 6 × `GET /_cat/indices` (#901, Task 24) and 3 × `_update_by_query` refusing an object-valued `params` assignment (`ctx._source.metadata = params.metadata`, #903, not in the plan's expected list; one more MA1 fix per D1209).
- **MA-R10.1 Quickstarts and nightly.** The e2e tests use a deterministic hashed bag-of-words embedder (MA-R0.9). The nightly `alpha` cell installs with `--config.minimum-release-age=0`. Deferred: the `loams: candidate` cell (no release candidate after 0.1.0 yet) and V7, because the development compose compute has no autosuspend; both return with Task 27.
- **MA-R11.1 Template.** `create-mastra` takes only a whole repository (MA-R0.5), so `template.yml` copies `templates/loams-starter` out of the workspace and installs it standalone, and `template-mirror.yml` pushes it to `ostrium-labs/loams-mastra-template` with a deploy key (`TEMPLATE_DEPLOY_KEY`); creating that repository and key is an owner action, and the mirror is a no-op until then.
- **MA-R12.1 Upstream docs page.** Issue mastra-ai/mastra#27809 ("docs: add Loams to the Databases integrations (uses @mastra/pg and @mastra/qdrant; no new package)"), opened after the Qdrant, pg and examples cells were green (run 39408994468). The page is ready on the fork's branch `ostrium-labs/mastra:docs/loams-integration` (`docs/src/content/en/integrations/databases/loams.mdx` and the `sidebars.js` entry after `databases/libsql`, icon `https://loams.dev/icon.svg`); its source is `loams-mastra/docs/upstream/loams.mdx`. The PR waits for the issue to leave `status: needs triage` / `needs approval`. The first green gate's matrix is in `COMPATIBILITY.md`: qdrant and qdrant-auth 224/235 (11 expected), pg 3316/3316, elasticsearch 385/394 (9 expected, observed).

### MAR (Tasks 13–22)

- **MA-R13.1, the Resonate calls** (verified at `@resonatehq/sdk` 0.11.5, protocol `2026-04-01`).
  - The engine uses the SDK's async entry point, `@resonatehq/sdk/async` (`new Resonate({ url, group })`, `register`, `run`, `ctx.run`, `ctx.rpc`, `ctx.sleep`, `ctx.promise`). The generator API (`Resonate.remote`) does not fit Mastra's async engine.
  - Durable ids are lineage positions (`<runId>:3.1`), not `<runId>/<operationId>`. An explicit id starts its own origin, and the Loams Durable server refuses to settle such a child ("Action must belong to the task's origin"); upstream v0.9.8 accepts it.
  - Positions are deterministic because every concurrent step execution (parallel, conditional, foreach item) runs in its own child context. The Mastra operation id is a `mastra:op` tag and part of the memoised value; a replay that asks for a different operation fails with `NondeterminismError`.
  - Each pass of a run is a `ctx.rpc`, a global promise at `<runId>:2k`. Loams refuses listeners on local-scope children ("Awaited promise is not awaitable"), and the caller must wait on the pass.
- **MA-R13.2, driving Mastra's test utilities:** `createWorkflowTestSuite` with `init(resonate)`, as `workflows/inngest` does.
  - Vendoring leaves out `workflows/_test-utils/src/domains/fga.ts`, which imports `@mastra/core/auth/ee`, and patches its two references; the patches are recorded in `UPSTREAM.json`.
  - It also vendors Mastra's `packages/_vendored/ai_v4` and `ai_v5` for `@internal/ai-sdk-v4`/`v5`.
- **MA-R13.3, test-utils groups:** all domains run.
  - 259 of 290 tests pass on both backends.
  - Expected failures, with issues: foreach resume by index or label (loams-mastra#29), resume through a workflow rebuilt without Mastra (#30), and `restart()` (#32).
  - Skipped: suspend and resume through `streamLegacy`, which hangs (#31).
- **MA-R13.4, suspend (Task 18):** Mastra's `suspend()` ends the pass, and a resume re-enters the graph from the snapshot, so a step cannot block on `ctx.promise`.
  - After a suspended pass, the run's root awaits a latent `ctx.promise` at `<runId>:2k+1`, and `run.resume()` settles it.
  - A second settle returns the existing record without an error, so each request carries a `resumeId` and the losing caller is refused (`double_resume_is_refused`).
- **MA-R13.5, upstream server pin:** `ghcr.io/resonatehq/resonate:v0.9.8`, pinned by digest (2026-06-04).
- **MA-R13.6, toolchain:** TypeScript 5.9.3, vitest 4.1.11 and tsdown 0.22.9. The package compiles with `moduleResolution: bundler` and extensionless imports, as Mastra's packages do.
- **MA-R14.1, Loams Durable in CI:** the `resonate` job runs Loams Durable from the 0.1.0 release **image** (pinned by digest), not Task 5's `.deb`. The 0.1.0 `.deb` is built without the `durable` feature (#925). It accepts `--durable-listen`, as MA-R0.2 found, but `loams dev` logs "this build has no durable execution (the durable feature is off)" and serves nothing on it. Move to `install-loams.sh` once a package with Durable ships.
- **MA-R16.1:** the compat reporter's `ISSUE_URL` also accepts `ostrium-labs/loams-mastra` issues, since this task's expected failures are tracked there.
- **MA-R20.1:** `examples/durable-workflow` moves to Task 21. `examples_pin_exact_versions` needs a published `@loams/mastra-resonate`, which `workspace:*` cannot stand in for.
