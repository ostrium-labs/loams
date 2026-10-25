# Nx Cloud remote cache

The monorepo's Nx tasks (web packages, plugins, the desktop app) can share one remote cache on
Nx Cloud between CI and the owner's machine. Workspace: `6ada11220d72175d478305c0` (`nxCloudId` in
`nx.json`). Nx is 23.2.1.

**Current state (2026-10-23):**

- CI connects only on a **push to `dev`** in the monorepo workflow (`monorepo.yml`, jobs *Web* and
  *Plugins*). `NX_CLOUD_CI=on` and both tokens are set. The free plan's credits pay for about 3 runs
  a day, and CI makes over 100 (see [Credits](#credits)).
- Pull requests, every other workflow and every release job stay off the cloud.
- Nx Agents (Workers) are off.
- The owner is applying for Nx's free open-source plan ([below](#the-open-source-plan)). Once it is
  granted, set `NX_CLOUD_CI=all` to connect pull requests from this repository too.

## How CI connects

Nx 19.7 and later connect a workspace through `nxCloudId` in `nx.json`. CI authenticates with a
CI access token in `NX_CLOUD_ACCESS_TOKEN`, which takes precedence over `nx.json`. No token is ever
committed. `neverConnectToCloud: true` turns the cloud off entirely in Nx 23
(`isNxCloudDisabled`), so `nx.json` no longer has it; `NX_NO_CLOUD=true` turns it off for one run.

| Repository variable `NX_CLOUD_CI` | Push to `dev` | Pull request from this repository | Fork pull request |
|---|---|---|---|
| unset, `off`, anything else | off | off | off |
| `on` | read-write | off | off |
| `all` | read-write | read-only | off (no secrets) |

- **Read-write token:** Nx Cloud's "protected branch pipeline" CI token. It is stored as the secret
  `NX_CLOUD_ACCESS_TOKEN` of the `nx-cloud-write` environment, which may only be used from `dev` and
  `main`, so a pull request cannot read it even if it edits the workflow.
- **Read-only token** (used only with `all`): the "unprotected branch pipeline" token, stored as the
  repository secret `NX_CLOUD_ACCESS_TOKEN`. Nx Cloud keeps a read-only run's results in an isolated
  cache, so they never reach the shared cache.
- **Which token a job sees:** both secrets have the same name, and a job that uses the environment
  gets the environment's value. The jobs put the token in `NX_CLOUD_ACCESS_TOKEN` only when they
  connect; otherwise the variable is empty. Fork pull requests get no secrets at all.
- Every other workflow keeps `NX_NO_CLOUD: 'true'`. `desktop-electron.yml` and `studio-e2e.yml`
  still run their tasks through Nx, with the local cache only.

### Owner settings (done 2026-10-23)

**In Nx Cloud** (workspace settings):
- Workspace visibility: **organisation members only**.
- **Default access level: read-only.** This is the level for anyone who runs Nx without logging in,
  which Nx's docs call the *workspace ID access level*. It defaulted to read-write, and this
  repository is public.
- **Logged-in users: read-write.** The owner is the only member; give anyone added later
  read-only.
- Workspace data caching: on. This is a Workers feature, unused while Agents are off.
- **Two CI access tokens:**
  - "protected branch pipeline", read-write;
  - "unprotected branch pipeline", read-only.

**On GitHub:**
- the protected token: `gh secret set NX_CLOUD_ACCESS_TOKEN --env nx-cloud-write`;
- the unprotected token: `gh secret set NX_CLOUD_ACCESS_TOKEN` (repository);
- `gh variable set NX_CLOUD_CI --body on`.

`gh secret set` reads the value at a hidden prompt, so a token never appears in chat, in a file or in
shell history. The `nx-cloud-write` environment's deployment branches are `dev` and `main`, and a
connected dev push shows up as a deployment to it.

**Checked without a token (2026-10-23).** Each run used a throwaway `HOME` and `TMPDIR`, no
`NX_CLOUD_ACCESS_TOKEN`, a cleared `.nx/`, and a new hash (a one-line comment in
`@loams/slots`).
- Anonymous runs connect and are recorded.
- **Anonymous writes do not reach the cache.** A rerun of the same hash missed, from the same branch
  (`NX_BRANCH`) and from another.
- **Anonymous reads work.** After the first connected dev push (`4825182`) wrote its entries, a
  tokenless run on this machine (Node 26) at the same commit read 8 of 8 build and typecheck tasks
  from the remote cache. The machine and CI therefore share keys, as intended.
- One caution for anyone repeating the check: the Nx Cloud client keeps its own copy of downloaded
  results under `HOME`/`TMPDIR`. A test that reuses them sees false "hits".

**Later, once the open-source plan is granted:**
`! gh variable set NX_CLOUD_CI --body all --repo ostrium-labs/loams`

**To switch CI off:**
`! gh variable set NX_CLOUD_CI --body off --repo ostrium-labs/loams`

### Budget guard

If Nx Cloud fails a run itself (a bad or revoked token, used-up credits, an outage), Nx exits
non-zero. A bad token prints "Nx Cloud: Workspace is unable to be authorized. Exiting run." and exits
1. This was tested.

Every Nx step in the connected jobs runs through `scripts/ci/nx-cloud-run.sh`. On an Nx Cloud
error it reruns the same command once with `NX_NO_CLOUD=true` and leaves a warning. A real task
failure is not retried.

`scripts/ci/nx-cloud-run.test.sh` tests the wrapper and runs in the web job. A real Nx 23.2.1 run
with an invalid token, through the wrapper, fell back and passed.

I have not seen the exact wording Nx Cloud prints when the credits run out. The wrapper matches
"disabled", "exceeded", "out of credits", "usage limit" and similar. If a run fails on a message it
misses, add the message to `cloud_error` in the script.

### Measured in CI

These are the dev push of #1057's merge, `4825182`, run 39856110743. Attempt 1 was cancelled by a newer push, so I re-ran the run twice.

| Attempt | Web job | Plugins job |
|---|---|---|
| 2 (cold, writes) | 551 s; 1/111 hits | 47 s; 4/4 hits, from attempt 1's writes |
| 3 (warm) | **55 s; 110/111 hits** | 40 s; 4/4 hits |

The one miss in attempt 3 is `@loams/client:test`, which is `cache: false` by design: it writes the conformance report that CI then checks. That makes 100% of the cacheable tasks hits.

## Affected

On the fast tier, the web job runs `nx affected`. The base is the pull request's base commit, or on
a dev push the previous head (`github.event.before`). The checkout has full history
(`fetch-depth: 0`, `filter: blob:none`).

`nx run-many` still runs:
- on the full tier;
- on a manual dispatch;
- when the base commit is unknown;
- on a connected dev push, which warms every project's cache entry.

`scripts/ci/nx-scope.sh` makes the choice. Project selection uses `--exclude='*,!@loams/*'`, since
`nx affected` has no `--projects`.

## The owner's machine

To share the cache between your machine and CI, run this once in the repository:

```sh
pnpm nx login
```

It opens Nx Cloud in the browser and saves a personal access token in your home directory. The
command prints where. Nx then reads from and writes to the same cache as CI.

- `pnpm nx logout` removes the token.
- `NX_NO_CLOUD=true` runs one command without the cloud.
- Anyone else, including agents, should not log in with write access. Give other members
  `read-only`.

**The trade-off.** Your laptop can write entries that CI reads. A compromised laptop could poison
CI's *test* results: a cached "pass" for code that fails.

What limits the damage:
- **Releases never read the cache.** Every release, package, publish and deploy workflow sets
  `NX_NO_CLOUD=true` and `NX_SKIP_NX_CACHE=true`:
  - `desktop-electron-release.yml`
  - `release-package.yml`
  - `sdk-release.yml`
- The other release workflows run no Node or Nx.
- Entries are immutable once written.
- Removing a member revokes their personal token.

**Recommendation:** read-write only on your own machine, read-only for everyone else.

**Hashes shared with CI.**
- Build, typecheck and lint keys do not include the Node version, so your Node 26 shares them with
  CI's Node 22.
- Test keys include the Node major version and the platform (`testRuntime` in `nx.json`), so test
  results are shared only between the same Node major and OS. Use Node 22 locally to share them too.

## Cache inputs and outputs

Changes in `nx.json` and project files:

- `production` (build inputs) excludes Markdown, tests, stories, `test/`, `e2e/` and the Vitest and
  Playwright configs.
- `code` (typecheck) excludes Markdown.
- `default` also excludes `out/`, `coverage/` and test reports.
- `sharedGlobals` no longer has the Node version (only tests get it) or `web/biome.json`. Biome
  only affects `loams-web:lint`, whose inputs already cover `web/`.

**Missing inputs fixed (false-hit risks).** These tasks read files outside their project:

| Task | Extra input |
|---|---|
| `@loams/console:build` | the root `NOTICE` |
| `loams-desktop-electron:build` | `connectors/registry` and `connectors/schemas` (the connector catalog it generates) |
| `@loams/console-host:test` | `release/upgrade-path.toml`, the sandbox `frame.html` |
| `@loams/plugin-connectors:test` | the connector catalog |
| `@loams/plugin-graph:test`, `@loams/proto:test` | `conformance/graph/desktop` |
| `@loams/plugin-desktop-settings:test` | `NOTICE` |
| `@loams/ui:test`, `@loams/plugin-shell:test`, `@loams/plugin-data-studio:test`, `loams-desktop-electron:test` | all of `web/` (repository-wide checks) |

**Outputs.** `loams-desktop-electron:build` now also declares `resources/connectors.json`, which it
writes. Before, a cache hit left that file out.

**Checks run (2026-10-22, local cache, `NX_NO_CLOUD=true`):**

- **Miss, then hit**, with the tuned configuration. `nx run-many -t build typecheck test` over
  `@loams/*`, `plugins`, `plugins-dashboard-ui` and `loams-desktop-electron`: 117 tasks.
  - The first run, with an empty cache, took 271 s.
  - The second took 25 s, with 115/117 cache hits (98%).
  - The 2 misses are `@loams/client:test` and `@loams/console:test`, which fail on this machine's
    Node 26 (failed tasks are not cached). That is 100% of the tasks that passed.
  - Before tuning, the same measurement was 296 s, then 36 s with 114/117 hits.
- **Restore.** I deleted every build output and ran again from the cache. The restored files match
  the originals byte for byte: 998 files across the four builds, including `connectors.json`.
- **Determinism.** Two uncached builds of `@loams/ui`, `loams-desktop-electron` and
  `plugins-dashboard-ui` were identical.
  - The console's Vite 8 build differs in 3 of 431 chunks (`engine-oniguruma`, `codeTheme`,
    `Markdown`): Rolldown swaps export aliases (`export{L as n,…}` against `export{R as n,…}`).
  - Content hashes then cascade into chunk names.
  - The code is equivalent, and caching is unaffected (keys come from inputs). This is an upstream
    issue, not fixed here.

## Credits

Source: [credit pricing](https://nx.dev/docs/reference/nx-cloud/credits-pricing), the
[pricing page](https://nx.dev/pricing) and its FAQ (read 2026-10-22).

**What costs credits:**
- **A CI Pipeline Execution (CIPE): 500 credits.** On GitHub Actions a CIPE is one workflow run that
  contacts Nx Cloud. All jobs in one run share it.
- **Nx Agents (Workers):** 10 credits a minute for a 2-vCPU Linux agent. A 4-vCPU agent costs 20,
  and an 8-vCPU agent 40.
- **A resource-usage report:** 10 credits.
- **Self-healing CI:** priced per AI token.

**Not charged separately:**
- remote cache reads and writes ("Remote cache access: Unlimited");
- local runs after `nx login`, which are not CI runs.

**The Hobby plan:**
- 50,000 credits a month, about 100 CIPEs;
- up to 5 contributors (commit authors of CIPEs in a cycle);
- 10 concurrent CI connections.

Unused credits do not carry over. When they run out, Nx Cloud features turn off until the reset.
The budget guard keeps CI green when that happens.

**This repository's CI volume** (last 100 runs of each workflow, 2026-10-22, counting runs whose
jobs actually ran):

| Workflow | Runs a day | Credits a day if connected |
|---|---|---|
| `monorepo.yml` (runs with any job besides the path check; web and plugins are most) | up to about 48 | up to about 24,000 |
| `desktop-electron.yml` | about 34 | about 17,000 |
| `studio-e2e.yml` | about 32 | about 16,000 |
| `ci.yml` (Rust, if it went through Nx Cloud) | about 230 | about 115,000 |

Most dev pushes skip the web job or are cancelled by the next push. Of 60 dev pushes on 21–22
October, none ran it to completion. The dev-push-only mode therefore uses a few CIPEs a day at most,
well inside the 70,000 credits before 31 October. It also warms less of the cache than hoped. `all`
on the open-source plan is the useful setting.

## The open-source plan

The pricing FAQ says Nx Cloud is free for qualifying open-source organisations. To apply, use
[nx.dev/contact/engineering](https://nx.dev/contact/engineering) and give:

- **Organisation:** Ostrium Labs (ostriumlabs.org). The project is Loams,
  `github.com/ostrium-labs/loams`, a public repository under Apache-2.0, with no open core.
- **Nx Cloud:** workspace `6ada11220d72175d478305c0`, Nx 23.2.1, on GitHub Actions with
  GitHub-hosted runners.
- **Volume:** about 110 Nx-connected workflow runs a day for the JS projects (above), about 230 a
  day more if the Rust per-crate targets connect. That is roughly 55,000–170,000 credits a day,
  against Hobby's 50,000 a month.
- **Use:** the remote cache first; Nx Agents for JS tasks later.

## If the plan is refused: a self-hosted cache

Nx 23 can use any server that implements Nx's
[remote cache OpenAPI spec](https://nx.dev/docs/kb/self-hosted-caching):
- `PUT` and `GET /v1/cache/{hash}`, with tar bodies;
- a bearer token;
- 403 for a read-only token that writes.

Set `NX_SELF_HOSTED_REMOTE_CACHE_SERVER` (the URL) and `NX_SELF_HOSTED_REMOTE_CACHE_ACCESS_TOKEN`,
instead of the Nx Cloud token. This costs no credits, and the same `NX_CLOUD_CI` gating applies.

**A small fit:** a Cloudflare Worker in front of an R2 bucket.
- Two tokens (read-write for `dev` pushes, read-only for pull requests).
- Keys are immutable: refuse a `PUT` to an existing hash.
- A lifecycle rule expires old entries.
- R2's free tier (10 GB, free egress) fits task logs and the web build outputs.

**What you give up:** Nx Agents, Ultracache and the run analytics. This would be a service we
host, so it needs an owner decision before building.

## Nx Agents (Workers)

The owner approved Agents for JS and web tasks only (D1667): lint, typecheck, unit tests, Studio
and console e2e. They are not enabled while the cloud is limited to dev pushes.

**What enabling would take:**
- A launch template in `.nx/workflows/agents.yaml`, with:
  - image `ubuntu22.04-node22.22-v1`;
  - resource class `docker_linux_amd64/medium`;
  - init steps: checkout (`nrwl/nx-cloud-workflows` `v6.0.2`), pnpm 11.27.1 through corepack, then
    `pnpm install --frozen-lockfile`.
- `.gitignore` must then keep `.nx/workflows/`.
- In the web job, before any other Nx command:
  `pnpm dlx nx-cloud@19.1.3 start-ci-run --distribute-on="3 <template>" --require-explicit-completion`.
- An `if: always()` step at the end: `pnpm dlx nx-cloud@19.1.3 complete-ci-run`.
- Other jobs in the same run need their own `NX_CI_EXECUTION_ENV` (already set: `web`, `plugins`),
  so they do not join the distribution.
- Fork pull requests have no token and so never start agents.
- Studio e2e needs a `loams-agentd` built with Cargo. Rust stays on GitHub-hosted runners, so the
  e2e tasks need that binary as an artefact first. They stay on GitHub until then.

**Expected cost.**
- The web job's Nx steps take about 6 minutes on one 4-vCPU GitHub runner: about 200 s of build
  and typecheck, and 190 s of tests.
- On 3 medium agents, allow about 2 minutes of setup and 2–3 minutes of tasks each: about 150
  credits a run, plus the 500-credit CIPE.
- At up to about 48 web runs a day, that is about 216,000 agent credits a month on top of about 720,000
  for the CIPEs.
- **Only viable on the open-source plan.** The saving is about 2–3 minutes per web run.

## Rust

Each crate of the root Cargo workspace (65) and of `fabric/` (28) is an Nx project, tagged `cargo-crate`, `language:rust` and `cargo-workspace:root` or `cargo-workspace:fabric`. A local plugin, `tools/nx-cargo`, builds the projects from the Cargo manifests; it does not run Cargo, so JS-only jobs need no toolchain.

Each crate has three targets. None has `outputs`, so Nx caches only the result and the log; compilation stays in Cargo's `target/` and the CI Rust caches.

| Target | Command | Notes |
|---|---|---|
| `test` | `cargo nextest run --workspace --locked --no-tests=pass -E 'package(=<crate>)'` | Uses the workspace build and its feature unification, the same artefacts as today's workspace job, and runs only this crate's tests |
| `clippy` | `cargo clippy -p <crate> --all-targets --locked -- -D warnings` | |
| `fmt` | `cargo fmt -p <crate> --check` | |

`fabric/`'s CI runs `cargo test`, not nextest, so a fabric crate's `test` runs `cargo test -p <crate> --locked --no-fail-fast` (`"test": "cargo-test"` in `inputs.json`).

Usage:
- run them all: `pnpm nx run-many -t fmt clippy test --exclude='*,!tag:cargo-crate'`;
- only what a change reaches: `pnpm affected:rust`.

### Inputs

A false hit is worse than a miss. Each crate's inputs are:
- all files in the crate's directory;
- the files of its path dependencies (normal, dev and build, transitively);
- the workspace's `Cargo.toml`, `Cargo.lock` and `rust-toolchain.toml`, plus `proto/`, `clippy.toml` and `rustfmt.toml`;
- the compile settings printed by `tools/nx-cargo/rust-env.cjs`:
  - `rustc -vV`;
  - the `--cfg`, `opt-level`, `debug-assertions`, `overflow-checks`, `panic`, `target-cpu` and `target-feature` flags, from `RUSTFLAGS`, `CARGO_ENCODED_RUSTFLAGS` and Cargo config files;
  - the profile settings with the same effect, the build target and the host.
- for `test`, also `.config/nextest.toml` and `cargo nextest --version`.

The linker, link arguments and target directories are not inputs, so the owner's machine (`lld` through `~/.cargo/config.toml`) and CI share entries.

`tools/nx-cargo/inputs.json` declares the rest:

| Field | Meaning | Crates |
|---|---|---|
| `repoWide` | Code walks the repository (licence and source scans, compose files, `docs/`), so every file is an input | 13 in the root workspace, including `loams`; 6 in `fabric/` |
| `extra` | Files outside the crate that it reads | for example `conformance/graph/desktop/**` and `release/upgrade-path.toml` |
| `tools` | The version of a tool its tests run | `git`, `node`, `python3` |
| `noCache` | `cache: false`, because the tests depend on what the machine has | root: `loams-compat` (psql), `loams-sqldb` and `loams-sqlgate` (services, podman), `loams-upgrade` (downloads); fabric: `loams-flow` (kubectl, helm, promtool), `loams-house-worker` (kubectl) |

The feature-gated suites (TiKV, the cluster tests, the crash gate, the nightly suites) are not Nx targets, so they never use a cache.

`node tools/nx-cargo/check.cjs` runs in CI's *Repo: checks* job and checks two things:
- The crate graph matches `cargo metadata`.
- Every path a crate's code names outside itself and its dependencies is declared: `"../"` literals, `"/../"` after `CARGO_MANIFEST_DIR`, and `join("..")`. An undeclared read fails the check.

### Measured locally (2026-10-22, local cache)

| Target | Tasks | First run (miss) | Second run (hit) |
|---|---|---|---|
| `fmt` | 65 | 7 s | 1 s, 65/65 hits |
| `fmt` (`fabric/`) | 28 | 3 s | 1 s, 28/28 hits |
| `clippy` (`loams-common`, `loams-tuple`, `loams-store`) | 3 | 52 s | 1 s, 3/3 hits |

Invalidation, checked on the same targets:
- Editing `loams-store` reran `loams-store` and its dependant `loams-cache`; `loams-common` stayed a hit.
- `RUSTFLAGS="--cfg x"` is a miss.
- `RUSTFLAGS="-C link-arg=-s"` is a hit.

A local `test` run needs the whole workspace's test build (about 30 minutes, and more disk than this machine has free), so it is measured in CI.

### Where the time goes in CI (run of 2026-10-22)

- *Rust: tests* took 64 minutes: 30 minutes compiling, then 30.5 minutes running 6,695 tests.
- `loams-hot` is 2,440 s of the 6,860 s of test CPU time, then `loams` 811 s and `loams-query` 655 s.
- *Rust: fmt and clippy* took 44 minutes: clippy over the workspace 10.5 minutes, doc tests 13.5.

Caching per crate saves the test run of the crates a change does not reach, not the compile.

### Why not `@monodon/rust`

- Version 3.0.0 has had one release in 14 months.
- It depends on `@nx/devkit` ^22, and we are on Nx 23.
- Its GitHub repository shows no licence file (npm says MIT).
- Its targets cache Cargo outputs in Nx.

### Artefact sizes and credits

With no outputs, an entry is a log of a few KB, far inside any Nx Cloud limit; compiled artefacts are never sent.

The cost is the CIPEs: `ci.yml` runs about 230 times a day. CI therefore keeps the existing workspace commands until the open-source plan or a self-hosted cache is in place. Without a remote cache, a fresh runner gains nothing from per-crate targets and pays the per-invocation overhead.

**Switching the Rust jobs over (not done yet).** Once pull requests get a remote cache (`NX_CLOUD_CI=all` on the open-source plan), the switch is:
- *Rust: tests* runs `scripts/ci/nx-cloud-run.sh affected -t test --exclude='*,!tag:cargo-workspace:root'` in place of `cargo nextest run --workspace`.
- *Rust: fmt and clippy* runs the same with `-t fmt clippy`. Its feature-specific clippy, doc and doc-test steps stay as they are.

Both jobs keep their names, so `CI required` and the rulesets still match. They need the web job's Nx Cloud `environment` and `env` block, `pnpm install`, and `scripts/ci/nx-scope.sh`.

Measure the change on the first pull request that has it, against today's 64 and 44 minutes. Do not make the switch before then: on a runner with no remote cache, every per-crate task misses.
