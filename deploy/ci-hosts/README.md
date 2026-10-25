# Local CI hosts

Two laptops on the owner's tailnet run Loams CI; GitHub-hosted runners are kept for releases
(owner, 2026-10-24). The guide is [docs/guides/local-ci.md](../../docs/guides/local-ci.md).

| File | What |
|---|---|
| `setup-host.sh` | One idempotent command that sets a host up (`sudo deploy/ci-hosts/setup-host.sh --help`) |
| `provision-remote.sh` | From the primary: turns another laptop into a worker over SSH in one command |
| `versions.env` | Pinned versions and SHA-256 digests of everything downloaded |
| `lib/` | The pieces `setup-host.sh` sources: Tailscale, the `ci` user, toolchains, the private CA and worker bundles, NativeLink, sccache, podman, runners |
| `runner/` | The runner image (Ubuntu 24.04 `Containerfile`), its entrypoint and the job hooks |
| `sccache-evict.sh` | Least-recently-used eviction for sccache's store (run by a timer) |

What `setup-host.sh --role primary` sets up today:

- the `ci` user (no sudo, locked password, home mode 700, subordinate ids for rootless podman);
- a private CA (key in the owner's `~/.config/loams-ci/ca`, mode 600) and a client certificate for
  the owner (`~/.config/loams-ci/tls`);
- NativeLink as `loams-nativelink.service` (user `ci`): CAS + action cache with LRU eviction under
  `/mnt/Projects/ci/nativelink` (30 GiB cap), the scheduler and one worker. It listens on
  127.0.0.1 (plain gRPC) and on the Tailscale address (mutual TLS) only, ports 50051 (clients)
  and 50061 (workers, health), and opens those two ports on `tailscale0` in ufw;
- sccache 0.18.0 in `/opt/loams-ci/bin`, and its shared store: dufs (WebDAV) as
  `loams-sccache-store.service` (user `ci`) on 127.0.0.1 and the Tailscale address, port 50081,
  password-protected, under `/mnt/Projects/ci/sccache` with a 10 GiB cap enforced every 10 minutes
  by `loams-sccache-evict.timer`. sccache has no Remote Execution API backend, so it cannot share
  NativeLink's CAS. Clients load `/etc/loams-ci/sccache.env` (the runners) or
  `~/.config/loams-ci/sccache.env` (the owner, opt-in); `~/.cargo/config.toml` is not touched;
- rootless podman for `ci` (storage on `/mnt/Projects/ci/containers`, its API socket, unlimited
  locked memory and a seccomp profile that allows io_uring, for Iggy's real-server tests);
- GitHub self-hosted runners, several for `ostrium-labs/loams` (`--runners N`, names
  `<host>-loams-1..N`, sized from RAM) and one for `ostrium-labs/iggy` (labels
  `self-hosted, loams-local, linux, x64`), each a rootless container of the Ubuntu 24.04 image in
  `runner/`, run by a user unit of `ci` (`loams-runner-<repo>.service`). Jobs get `sudo`, `docker`
  (the `ci` user's podman), sccache via `RUSTC_WRAPPER`, a shared tool/cargo/rustup cache, and a
  10 GiB memory cap. A job refuses to start below 20 GiB free disk, and the job-completed hook
  removes the checkout, temp files and the compose stacks the job started (never images).

Secrets are generated on the host, kept in files of mode 600 (or 640 for group `ci`), and never
printed or committed.

Also: Tailscale (installed and joined with `--tailscale-authkey-file` when missing), and the host
toolchains for the `ci` user (Rust per `rust-toolchain.toml` and Node per `.nvmrc`, pnpm per
`packageManager`, in the caches the runners share).

## A second laptop (role worker)

Once it is reachable over SSH (the owner's account there, with sudo), from the primary:

```sh
deploy/ci-hosts/provision-remote.sh USER@HOST --tailscale-authkey-file ~/tailscale.txt
```

It issues the worker's credentials bundle (a client certificate signed by the primary's CA and
the sccache settings), mints a runner registration token, copies this directory there and runs
`sudo setup-host.sh --role worker`: Tailscale (`loams-ci-b`), the `ci` user, Rust/Node/pnpm,
NativeLink as a worker of the primary's scheduler over mutual TLS (80 GiB local cache), sccache
pointed at the primary's store, rootless podman and a `loams-local` runner for `ostrium-labs/loams`
(12 GiB, 10 cargo jobs, refuses jobs below 30 GiB free). The same paths (`/mnt/Projects/ci`) as on
the primary, so sccache entries are shared.
