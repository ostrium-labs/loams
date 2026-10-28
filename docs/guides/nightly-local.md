# The local nightly

Status: **preview**. The owner ruled on 2026-10-28: "do not run nightly in ci
only local". The nightly suites therefore run on the laptops through
[`scripts/nightly/run-local.sh`](../../scripts/nightly/run-local.sh). They do
not run on GitHub. `nightly.yml` and `compute-compat.yml` are no longer
scheduled.

## What it runs

`scripts/nightly/run-local.sh --list` prints the legs.

**Compute legs** (the DX0 conformance suites, each in its own compose project `ln-<leg>`):
- `durable`
- `corpus`
- `temporal-pg16` and `temporal-pg17`
- `dbos-py`, `dbos-ts`, `dbos-go` and `dbos-java`
- `airflow` (the Edge executor) and `airflow-local`

**Cargo legs** (the nightly-only Rust suites from `ci.yml` and `gt1-nightly.yml`):
- `spec` (TLA+ and the other specs at the nightly bounds)
- `sim` (1,000 seeds)
- `tail-merge` (256 cases)
- `git-seeds` (1,024 linearizability seeds)

The TiKV and TiDB nightly suites need their tiup clusters, and they are not
legs yet. The GitHub nightly also re-ran the per-PR workflows (fabric,
monorepo, pg2, Studio and the others). The local CI runners already run those
on every pull request, so they are not repeated here.

## Running it

```sh
scripts/nightly/run-local.sh                          # every leg (several hours)
scripts/nightly/run-local.sh --legs durable,corpus    # some legs
scripts/nightly/run-local.sh --keep --legs airflow    # leave the stack up to look at it
```

- Each leg runs whole under the shared build-slot limiter (`LOAMS_BUILDSLOT`,
  by default `.he0-shared/buildslot.sh`), with `CARGO_BUILD_JOBS=4`.
- After each compute leg, the script removes that leg's compose project and its
  volumes. At the end it also removes the harness's named volumes. It never
  prunes images, containers or volumes that it did not create.
- Results go to `target/nightly/<date>-<commit>/`. `--results DIR` writes them
  elsewhere, but never under `/tmp`.

## Results

| File | What |
|---|---|
| `summary.json` | every leg with its exit status, duration and JUnit files; `failed` lists the legs that failed |
| `compute-<leg>/` | the leg's JUnit, its log (`leg.log`), its exit status (`leg.exit`) and the suite's own logs |
| `compatibility-durable.md` | the durable section of the matrix, from this run |

The script splices the same section into
[`docs/guides/compute/compatibility.md`](compute/compatibility.md) and labels
it "local run <date> <commit>". The page changes in the working tree only.
Commit it by hand when a run should be published (owner ruling, 2026-10-27). A
leg that stops before its strict expected-failure check shows red in the
matrix (DX1 ruling T6.R11).

## Every night

[`deploy/nightly/`](../../deploy/nightly/README.md) has a systemd user timer
(02:17 local time, `Persistent=true`) and its service. Neither is installed
by anything; the README there says how to install them.
