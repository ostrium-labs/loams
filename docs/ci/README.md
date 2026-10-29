# CI

GitHub Actions on this repository. The repository is public, so GitHub-hosted runners (Linux,
Windows, macOS) cost nothing; what is scarce is the organisation's concurrent-job limit (about 20
jobs, about 5 of them macOS). Blacksmith was removed (owner, 2026-10-19).

**Local CI (owner, 2026-10-24).** Two laptops on the owner's tailnet run the pull-request and
`dev`-push jobs when the repository variable `CI_RUNNER` is `local`: every Linux job of the PR/dev
workflows picks `[self-hosted, loams-local]` for a push to `dev` or a pull request into `dev` from a
branch in this repository (or by an organisation member). A pull request from anyone else's fork,
pull requests into `main`, pushes to `main`, dispatches, the nightly and every release workflow keep
GitHub-hosted runners, with the same check names. `CI_RUNNER=github` (or unset) falls back in one
step. `deploy/ci-hosts/route-workflows.py` writes the `runs-on:` expressions (run it after adding a
job; `--check` verifies). See [the local CI guide](../guides/local-ci.md). The `RUNNER_HEAVY` and `RUNNER_MAIN` repository variables select the label for the heavy
Rust jobs and the main-branch jobs, and default to `ubuntu-latest`.

Nx Cloud (D1667) caches the Nx tasks' results: only a push to `dev` in `monorepo.yml` connects
for now (`NX_CLOUD_CI=on`), and release, package, publish and deploy workflows never use a task
cache. Nx Agents, allowed for JS tasks only, are off. See [the Nx Cloud guide](../guides/nx-cloud.md).

## Tiers

| Event | What runs |
|---|---|
| Pull request into `dev`, push to `dev` | **Fast tier**, by changed path: DCO, repository checks, Rust fmt/clippy and tests, cargo-deny, console, protos, the seven supported SDKs, unit jobs |
| Pull request into `main` (the release gate) or a `release/*` branch, or one labelled `ci:full` (then a push) | **Full tier**, by changed path: the fast tier plus the engine, TiKV, Live, durable, Graph, fuzzing, specs, Windows check, connector stacks, mobile, Postgres e2e (not on the label), Git (not on the label), desktop on all five platforms |
| Push to `main`, manual dispatch, nightly | Everything, regardless of paths |

`nightly.yml` dispatches the full suites on `dev` every night (a `schedule:` only runs on the
default branch, `main`). A red nightly means: fix `dev` before the next `dev` → `main` pull request.

| Nightly workflow | What it checks |
|---|---|
| `mastra-gate.yml` (MA1 Task 27, Q1001) | Mastra's own Qdrant suites (with and without an API key) and the observed Elasticsearch suite from `ostrium-labs/loams-mastra` `main`, against `loams` built from `dev` (`LOAMS_VERSION=dev`, so entries fixed on dev must pass). A failure opens or updates the issue "Mastra gate fails on dev" (label `mastra-compat`). Not a required check. |

A draft pull request skips the heavy jobs until it is marked ready; the `ci:full` label (then a
push) runs them anyway, and runs the full tier. CodeRabbit does not review drafts.

On a pull request into `dev`:
- **Desktop** builds Linux x64 only, and only when the desktop app, the stacks it packages
  (`deploy/loams-postgres-dev`, `deploy/wesql`, `deploy/tikv`), `plugins/`, the release scripts or
  the lockfile change. A web-only change doesn't run it.
- **macOS and Windows desktop builds** run on the full tier only.
- **Release SDKs** runs the dry-run packaging only for the SDKs whose manifests the pull request
  touches. The Rust job also runs for `crates/loams-proto` and `crates/loams-live-proto`. The
  workflow file and `scripts/sdk/sdk-version.py` select every SDK, and a tag push builds them all.

## Required checks

`CI required`, `Connectors required`, `Web and mobile required` (each passes when every job its
event selected passed and the rest were skipped) and, on `dev`, `DCO sign-off`.

## The upgrade gate

`Upgrade: previous release → candidate` (`upgrade-gate`, design §54 §14) is in `CI required`. It runs on the
full tier, in the nightly on `dev`, and on a pull request into `dev` labelled `ci:upgrade` (use the label for a
third-party pin bump). `scripts/release/upgrade-gate.sh` does the work:
- it starts the previous release (the candidate's `from` in `release/upgrade-path.toml`) from its published
  bundle, with `deploy/release/ci/compose.upgrade.yaml`;
- it writes `conformance/upgrade`'s fixtures;
- it builds the candidate's `loams` image from the checkout, then runs `loams-upgrade` to upgrade, check, roll
  back and check again;
- when every migration is reversible, it also runs `rollback --keep-writes`.

The artifact `upgrade-gate` holds the ledger, `check`'s JSON, the reports, the timings and every service's logs.
Run it locally with `scripts/release/upgrade-gate.sh <workdir>`. It needs Docker with Compose (or Podman), `gh`
and about 15 GB of free disk.

## Names

Workflows are named for their product area (`CI`, `Connectors`, `Web and mobile`, `Postgres`,
`Git`, `Desktop`, `Release …`). Jobs are sentence case; in a workflow that covers several areas
they carry an `Area: what` prefix (`Rust: tests (1/2)`, `SDK: Swift`, `Mobile: iOS`). A gate is
`<Workflow> required`.

## Adding a job

Give it a path filter in its workflow's `changes` job, decide its tier (fast only if it is quick
and catches what a pull request into `dev` must not break), add it to the workflow's `required`
gate, and give the workflow a `concurrency` group that cancels superseded runs (see below).

## Concurrency and triggers

Every workflow has a `concurrency` group keyed on the event and on the pull request number (or the
ref), so a push to `dev` never cancels the nightly's dispatch on `dev`:

| Run | Superseded by a newer one |
|---|---|
| A pull request | Cancelled |
| A push to `dev` (CI, Connectors, Web and mobile) | Cancelled |
| A push to `dev` that saves a cache (Postgres, Gateway, Platform) | Runs to completion |
| A push to `main`, a tag, a release or publish workflow, a dispatch, the nightly, a schedule | Runs to completion |

A pull request branch never runs a workflow on `push`: the `push` triggers name `dev`, `main` or
tags only, so one commit runs each workflow once. On `dev`, `push` runs only the three required
workflows (the merge result), and the path-filtered workflows that save a cache for pull requests.
Postgres pins runs on pull requests (not for a `docs/` only change) and in the nightly; Release
compose runs on pull requests and on `main`. Studio e2e does not restart when a label is added.

A `docs/`-only pull request runs DCO, the repository checks (`Repo: checks`, which include the
decision-ID checks) and the three required gates, each of which passes with its other jobs
skipped. `docs/house/**` (the generated compatibility profile and matrix) and the docs that tests
read (`docs/api/reasons.md`, `docs/api/route-map.md`, `docs/sdk/**`) still select their jobs.

## Caches

The repository has 10 GB of Actions cache, and GitHub evicts the least recently used entries
beyond that. A pull request's cache is visible to that pull request alone. A cache saved on
`dev` is visible to every pull request into `dev`.
- **Rust (`Swatinem/rust-cache`):** saves on `dev` only: its pushes, and the nightly's dispatch.
  Pull requests restore from it.
  - Before 2026-10-23, desktop, `sdk-release`'s dry run and a few Connectors and Postgres jobs saved
    on every pull request, about 0.4–1.6 GB each. With 15 agents pushing, those filled the 10 GB and
    evicted `dev`'s caches, so most pull requests built cold (`Rust: tests` took 47 min at the
    median, `Rust: fmt and clippy` 33 min).
- **Desktop:** saves Linux x64 on `dev`, and every platform on a release-gate pull request (into
  `main` or `release/*`). A release-gate pull request cannot read `dev`'s caches, and is pushed
  many times.
- **Debug info:** CI builds dev and test profiles with `CARGO_PROFILE_DEV_DEBUG=0` and
  `CARGO_INCREMENTAL=0`. That gives smaller caches and faster links, and a panic still names its
  file and line.
