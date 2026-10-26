# Local CI

Loams CI for pull requests and pushes to `dev` runs on two laptops on the owner's tailnet. GitHub-hosted
runners are kept for releases (owner, 2026-10-24). Everything on the laptops is set up by one
idempotent command, [`deploy/ci-hosts/setup-host.sh`](../../deploy/ci-hosts/setup-host.sh).

## What runs where

| Host | Role | Services |
|---|---|---|
| Laptop A (`loams-ci-a`, CachyOS, 14 cores, 15 GB) | primary | NativeLink scheduler + CAS + action cache + 1 worker; sccache store; runners `loams-ci-a-loams-1..2` and `loams-ci-a-iggy` |
| Laptop B (`keshav-dell-15-d15260`, Linux Mint, 14 cores, 15 GB) | worker | NativeLink worker (joins A over the tailnet); runners `<host>-loams-1..3` |

| Service | Unit | Listens on | Auth | Cap |
|---|---|---|---|---|
| NativeLink clients (CAS, AC, ByteStream, Capabilities, Execution; instance `main`) | `loams-nativelink.service` | `127.0.0.1:50051`, `<tailnet-ip>:50051` | tailnet: mutual TLS | A: 20 GiB (CAS 15 + worker 4 + AC 1); B: 80 GiB worker store |
| NativeLink workers + health | same | `127.0.0.1:50061`, `<tailnet-ip>:50061` | tailnet: mutual TLS | |
| sccache store (dufs, WebDAV) | `loams-sccache-store.service` + `loams-sccache-evict.timer` | `127.0.0.1:50081`, `<tailnet-ip>:50081` | password | 8 GiB, LRU every 10 min |
| GitHub runners | `~ci/.config/systemd/user/loams-runner-<repo>[-<i>].service` | outbound only | GitHub registration | A: 5 GiB and 5 cargo jobs each; B: 4.5 GiB and 4 jobs each |

Nothing listens on `0.0.0.0` or the LAN; ufw opens the three ports on `tailscale0` only. The tailnet
also has other people's devices, so the tailnet listeners need a credential too.

## Security model

- Everything runs as the unprivileged `ci` user: locked password, no sudo, home mode 700. It cannot
  read the owner's home (mode 700), where the credential files live.
- Jobs run in a rootless podman container (Ubuntu 24.04). `sudo apt-get` inside a job is root in
  the container only, which is the `ci` user on the host.
- Fork pull requests from outside contributors need a maintainer's approval before any workflow runs
  (repository setting "Require approval for all external contributors", on `loams` and `iggy`).
- Even approved, a pull request from a fork whose author is not an organisation member or owner
  never runs on the laptops: the `runs-on:` expression sends it to GitHub-hosted runners, with the
  same check names. Only pushes to `dev` and pull requests into `dev` from branches in the
  repository (or from members' forks) run locally. A job-level `if:` that skipped those jobs instead
  would report the required checks as passed, so the guard is in `runs-on:`.
- Secrets: the CA key (`~/.config/loams-ci/ca/ca.key`), the client certificates and the sccache
  password are generated on the host, kept in files of mode 600 (640 for group `ci`), and never
  printed or committed.

## Measured effect of sccache (2026-10-24)

`cargo build -p loams-gateway --locked` from an empty `target/`, in the runner container on
laptop A (6 jobs): **63 s cold** (124 compilations, all misses, written to the store) and **16 s
warm** (124 of 124 cacheable compilations hit; the rest is build scripts, linking and the 21
non-cacheable calls). From laptop B's setup over the tailnet, a small crate went from 36 s to 3 s.
sccache keys include absolute paths, so both laptops use the same paths (`/mnt/Projects/ci/...`).

## Switching back to GitHub-hosted runners

The repository variable `CI_RUNNER` picks the runners:

| `CI_RUNNER` | Pull requests into `dev`, pushes to `dev` |
|---|---|
| `local` | the laptops (`[self-hosted, loams-local]`) |
| `github` or unset | GitHub-hosted |

```sh
gh variable set CI_RUNNER --repo ostrium-labs/loams --body github   # fall back
gh variable set CI_RUNNER --repo ostrium-labs/loams --body local    # back to the laptops
```

**Laptop asleep → CI waits.** A job routed to `loams-local` waits in the queue until a runner with
that label is online (GitHub cancels it after 24 hours). If both laptops are asleep or offline, flip
`CI_RUNNER=github`; re-run the waiting workflows (`gh run rerun <id>`) so they pick up the new value.

Pull requests into `main`, pushes to `main`, tags, dispatches, the nightly and every release,
image, signing and SDK publishing workflow always run on GitHub-hosted runners.

## Day to day

```sh
deploy/ci-hosts/status.sh             # or /opt/loams-ci/bin/loams-ci-status; exit 1 on a problem
deploy/ci-hosts/status.sh --github    # also: runners online on GitHub, the CI_RUNNER value
deploy/ci-hosts/status.sh --json
journalctl -u loams-nativelink -f
journalctl -u loams-ci-monitor        # the health report every 15 minutes
sudo systemctl --user -M ci@ status 'loams-runner-*'
sudo systemctl --user -M ci@ restart loams-runner-loams-1
```

`status.sh` needs no sudo: it checks the units, the health endpoints, the listening addresses,
free disk, the runner processes and the cache sizes against their caps. Cache sizes come from
`/run/loams-ci/sizes`, which `loams-ci-monitor.timer` (root) refreshes every 15 minutes; the same
timer logs the report and fails its unit on a problem, so `systemctl --failed` shows it.

| Cache | Where | Cap | Eviction |
|---|---|---|---|
| NativeLink CAS + AC + worker store | `/mnt/Projects/ci/nativelink` | 20 GiB (A), 80 GiB (B) | NativeLink's LRU, on insert |
| sccache | `/mnt/Projects/ci/sccache` (A only) | 8 GiB | `loams-sccache-evict.timer`, every 10 min, down to 90% |
| runner work dirs | `/mnt/Projects/ci/runners/*/work` | per job | `job-completed` hook removes the checkout and temp files |
| cargo registry, rustup, tool cache | `/mnt/Projects/ci/cache` | none (a few GiB) | none |
| runner image | `/mnt/Projects/ci/containers` | ~1.5 GiB | rebuilt only when `deploy/ci-hosts` changes |

Disk floor: a job refuses to start (with an error naming the host) when its filesystem has less
than 5 GiB per loams runner on A (10 GiB), 10 GiB per runner on B (30 GiB); `--runner-disk-gb`
sets the per-runner amount, `--min-free-gb` the total. On A, `/mnt/Projects` is shared with the
owner's builds, and the shared Rust target dominates it, so A's caches are kept small (20 + 8 GiB)
and the floor low. The work directories stay on `/mnt/Projects` (ext4) rather than `/`: the root
filesystem is btrfs with snapper snapshots, which would keep deleted job output alive. A low-disk
warning does not fail the health check; only failed checks do. When jobs fail with "Local CI host disk low", free disk or flip
`CI_RUNNER=github`.

## Several runners per laptop

A pull request fans out to about 30 jobs, so each laptop runs several runners of
`ostrium-labs/loams`: `--runners N` (or `--runner ostrium-labs/loams:N`) makes `<host>-loams-1..N`,
units `loams-runner-loams-1..N.service`. Defaults: 2 on the primary (plus `<host>-iggy`), 3 on a
worker. An unnumbered `<host>-loams` left from an older setup is retired: its unit stopped, its
GitHub registration deleted (with `--gh-token-file`; otherwise GitHub drops it after 14 days
offline) and its files removed.

Sizing, so that N runners cannot exhaust the RAM: what is left after `--runner-reserve-gb` (5 GiB
on A, whose owner works on it; 1.5 GiB on B) is split over the loams runners, rounded down to
0.5 GiB, and each gets one cargo job per 1.25 GiB (`RUNNER_GB_PER_JOB`). On a 15 GiB laptop that is
2 x 5 GiB with 4 jobs on A and 3 x 4.5 GiB with 3 jobs on B. Each container may also use swap up to
half its cap, so a spike swaps instead of being OOM-killed. `--runner-memory` and `--runner-jobs`
override; `sudo deploy/ci-hosts/setup-host.sh --dry-run ...` prints the plan without changing
anything.

Measured 2026-10-26 on A, in the runner image: `cargo test -p loams-query --no-run --locked`
(DataFusion; sccache off) from an empty target. Peaks are cgroup memory, page cache included.

| Cap | Swap | Jobs | Result |
|---|---|---|---|
| none | | 5 | ok, 399 s, peak 6.0 GiB |
| 5 GiB | none | 5 | ok, 448 s (page cache reclaimed at the cap) |
| 4.5 GiB | none | 4 | **rustc for `loams-proto` OOM-killed** |
| 4.5 GiB | none | 3 | **rustc for `loams-proto` OOM-killed** |
| 4.5 GiB | up to 6.75 GiB | 4 | ok, 751 s (1.9 GiB swapped; the host was busy) |

The peak comes from one crate (`loams-proto`'s generated code needs about 4 GiB in one rustc), not
from the number of jobs, so a cap below about 5 GiB needs the swap allowance; B has 32 GiB of swap.

## Runner self-healing

GitHub can drop a runner's registration (it was deleted in the repository settings or through the
API, for example). The runner then deletes its local credentials and stops with "Not
configured". Its container exits with status 78, which stops systemd's restart loop, and
`loams-ci-runner-heal.timer` (root, every 5 minutes) registers it again with a fresh registration
token and starts it; it also restarts a runner that stopped for any other reason. Tested
2026-10-25 by deleting `loams-ci-a-iggy` through the API: online again 5.5 minutes later.

The heal timer mints registration tokens with the GitHub token given to `setup-host.sh
--gh-token-file` (A: the owner's `~/gh.txt`, read by root only). For a worker, pass
`provision-remote.sh ... --gh-token-file FILE`: the token is stored root-only at
`/etc/loams-ci/github-token` (use a fine-grained token with Administration read/write on
`ostrium-labs/loams` only). Without a token the timer cannot re-register, and the health check
reports the runner as failed. `journalctl -u loams-ci-runner-heal` shows what it did.

`setup-host.sh` saves the settings of each full run (`/etc/loams-ci/setup.env`, root) and uses
them as the defaults of the next run and of `--heal`, so options such as `--data-root` need not
be repeated.

## Changing workflows

Every Linux job of the PR/dev workflows carries the routing expression in `runs-on:`. After adding
a job or rebasing a workflow:

```sh
python3 deploy/ci-hosts/route-workflows.py          # rewrites runs-on, guards rust-cache
python3 deploy/ci-hosts/route-workflows.py --check  # exit 1 if a job is not routed
```

On the laptops, `Swatinem/rust-cache` and the "Free disk space" steps are skipped: sccache
(`RUSTC_WRAPPER`, set by the runner) replaces rust-cache, and the disk step would delete the
runners' shared tool cache. Jobs on the laptops can use NativeLink at `grpc://127.0.0.1:50051`
(instance `main`), for example for Buck2 ([Buck2 guide](buck2.md)); on GitHub-hosted runners
there is none, so remote caching must stay optional there.

## The job environment

| | |
|---|---|
| Userland | Ubuntu 24.04 container (`deploy/ci-hosts/runner/Containerfile`): build-essential, clang/lld, protoc, Python, git, gh, docker CLI with buildx and compose |
| User | root in the container (`sudo` works) = the `ci` user on the host |
| `docker` | the `ci` user's rootless podman (service containers, `docker run`, compose) |
| Rust | `CARGO_HOME`/`RUSTUP_HOME` shared and persistent; the toolchain follows `rust-toolchain.toml`; `RUSTC_WRAPPER=sccache`, `CARGO_INCREMENTAL=0`, `CARGO_BUILD_JOBS` 5 (A) / 4 (B) |
| Limits | memory 5 GiB (A) / 4.5 GiB (B) per runner, plus up to half that again in swap; `RLIMIT_MEMLOCK` unlimited and io_uring allowed (Iggy's real-server tests) |
| Network | the host's (loopback services published by service containers are on `localhost`) |
| `LOAMS_CI_HOST` | `loams-ci-a` or `loams-ci-b` |

## Setting up a host

Laptop A (primary), from a checkout, as the owner:

```sh
sudo deploy/ci-hosts/setup-host.sh --gh-token-file ~/gh.txt
```

Laptop B (worker), from laptop A, once B is reachable over SSH:

```sh
deploy/ci-hosts/provision-remote.sh USER@HOST --tailscale-authkey-file ~/tailscale.txt
```

Both are idempotent: run them again after pulling changes to `deploy/ci-hosts`, or to repair a
host. Re-running restarts only what changed (a runner restart cancels its running job). See
`sudo deploy/ci-hosts/setup-host.sh --help` for the options (caps, memory, jobs, components).

Removing a host: `sudo systemctl disable --now loams-nativelink loams-sccache-store
loams-sccache-evict.timer loams-ci-monitor.timer`, `sudo systemctl --user -M ci@ disable --now
'loams-runner-*'`, then remove the runners on GitHub (Settings, Actions, Runners).
