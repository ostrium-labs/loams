# CI

GitHub Actions on this repository. The repository is public, so GitHub-hosted runners (Linux,
Windows, macOS) cost nothing; what is scarce is the organisation's concurrent-job limit (about 20
jobs, about 5 of them macOS). CI uses GitHub-hosted runners only (owner, 2026-10-19; Blacksmith
removed). The `RUNNER_HEAVY` and `RUNNER_MAIN` repository variables select the label for the heavy
Rust jobs and the main-branch jobs, and default to `ubuntu-latest`.

## Tiers

| Event | What runs |
|---|---|
| Pull request into `dev`, push to `dev` | **Fast tier**, by changed path: DCO, repository checks, Rust fmt/clippy and tests, cargo-deny, console, protos, the seven supported SDKs, unit jobs |
| Pull request into `main` (the release gate) | **Full tier**, by changed path: the fast tier plus the engine, TiKV, Live, durable, Graph, fuzzing, specs, Windows check, connector stacks, mobile, Postgres e2e, Git, desktop on all five platforms |
| Push to `main`, manual dispatch, nightly | Everything, regardless of paths |

`nightly.yml` dispatches the full suites on `dev` every night (a `schedule:` only runs on the
default branch, `main`). A red nightly means: fix `dev` before the next `dev` → `main` pull request.

| Nightly workflow | What it checks |
|---|---|
| `mastra-gate.yml` (MA1 Task 27, Q1001) | Mastra's own Qdrant suites (with and without an API key) and the observed Elasticsearch suite from `ostrium-labs/loams-mastra` `main`, against `loams` built from `dev` (`LOAMS_VERSION=dev`, so entries fixed on dev must pass). A failure opens or updates the issue "Mastra gate fails on dev" (label `mastra-compat`). Not a required check. |

A draft pull request skips the heavy jobs until it is marked ready; the `ci:full` label (then a
push) runs them anyway. CodeRabbit does not review drafts.

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
gate, and give the workflow a `concurrency` group that cancels superseded runs.
