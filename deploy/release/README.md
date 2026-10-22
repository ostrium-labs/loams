# Loams: run it yourself

Everything in Loams 0.1.1 runs on your machine or on VMs you own. Ostrium hosts
two things only: the console (a React app) and the auth service. You can serve
the console yourself instead, from your own Vercel or Cloudflare project, or
from this stack (`--profile console`).

```
 browser ── console (hosted, or yours) ── auth.loams.dev (OIDC, PKCE)
    │
    └─ your device's Tailscale client ══ tailnet ══ tailscale sidecar ─┐
                                    (loams netns: engine + sidecar only)│
                         loams engine, tailscale listener :7781 ◄───────┘
    ─────────────── compose network ───────────────
      house · agentd · Postgres stack (pageserver, safekeeper, compute1)
      with profiles: pd · tikv · tidb (`tikv`) · rustfs (`rustfs`)
```

The engine is never on the public internet: the only way in is the Tailscale
sidecar, which joins your own Tailscale tailnet and proxies `https://loams.<tailnet>.ts.net` to the engine's
tailscale listener (`LOAMS_TAILSCALE_LISTEN`, `127.0.0.1:7781`). Only the
engine and the sidecar share that network namespace; every other service is on
the compose network and cannot reach the engine's loopback listeners.
No service binds a host port on `0.0.0.0` by default.

## Images

Built by [`build.sh`](build.sh) from `Containerfile.<name>` and published to
`ghcr.io/ostrium-labs` as `:<version>`, `:<major>.<minor>` and `:latest` (a
prerelease gets `:preview` instead), for linux/amd64 and
linux/arm64. Each runs as uid 10001 on debian-slim, with tini as PID 1 and a
healthcheck.

| Image | Contains |
|---|---|
| `loams` | the engine, features `live,live-tikv,durable,durable-tikv,graph,git,tikv,pgwire,mysql-wire,stream-grpc` |
| `loams-fabric` | `loams-fabric house` and `loams-house-worker`, with the digest-pinned libchdb; workers sealed with `--sandbox pods` by default |
| `loams-agentd` | the agent daemon |
| `loams-pg-control` | `loams` with features `postgres,tikv` and the standalone `docker-compose` CLI: `pg-control` and `pg-runner` (`--profile postgres-control`; from 0.1.0) |

```sh
deploy/release/build.sh loams                        # both platforms, local manifest list
deploy/release/build.sh --platforms linux/amd64 loams
deploy/release/build.sh --version 0.1.1 --push   # CI on a tag
```

## On this machine

```sh
cd deploy/release
cp .env.example .env            # set TS_AUTHKEY and LOAMS_SETUP_TOKEN; see the comments
docker compose up -d            # the light default (below)
docker compose logs tailscale   # the node name, or a login URL
```

The default stack is light enough for a laptop or one small VM: about 8 GB of
images and 1 GB of RAM at idle (measured on 0.1.0 images). It runs

- the engine (`loams`, with `migrate` first) on its embedded stores: the
  metastore on openraft + redb, Loams Live on redb, Durable on SQLite, Git and
  Graph, all in the `loams-data` volume, with the bucket a local folder
  (`LOAMS_BUCKET=file:///var/lib/loams/bucket`);
- Loams House (`house`), linked to the engine;
- Loams Postgres: `storage_broker`, `pageserver`, `safekeeper1`, the one-shot
  `pg-bootstrap` and `compute1`, with the pageserver's and the safekeeper's
  remote storage in local folders in their volumes;
- `agentd`, and the `tailscale` sidecar.

It is one node on one disk: nothing is replicated, so back up the volumes (see
[Durability](../../docs/release/preview.md#durability-of-the-default-stack)).

Profiles add the rest. Set them in `.env` as `COMPOSE_PROFILES`
(comma-separated), not with `--profile` on the command line, so that the
engine, `migrate` and `loams upgrade` all see the same set:

| Profile | Adds |
|---|---|
| `tikv` | PD, TiKV and TiDB: Loams SQL (MySQL compatible), and the engine's metastore and Live on TiKV (keyspaces `loams_meta`, `loams_live_dev`) for scale. About 2.8 GB more of images and 2 GB more of RAM at idle (TiKV alone idles near 1.8 GB). Durable stays on SQLite unless `LOAMS_DURABLE_STORE=tikv://pd:2379/loams_durable` |
| `rustfs` | RustFS, an S3 bucket (`neon`) for Loams Postgres's pageserver, safekeeper and endpoint storage instead of local folders |
| `house-tailnet` | the House's own tailnet node (below) |
| `postgres-control` | Loams Postgres's control plane (below) |
| `console` | the console from `LOAMS_CONSOLE_DIR`, with `/config.json` built from `LOAMS_ENGINE_URL`, `LOAMS_AUTH_ISSUER` and `LOAMS_AUTH_CLIENT_ID` |
| `headscale` | coming soon, unsupported in 0.1.1 |

Choose `tikv` and `rustfs` before the first start: a stack's data stays on the
stores it started on. The engine records its store in the `loams-data` volume
and refuses to start on the other one (it would see an empty catalog beside
the data), and the Postgres timelines stay in the remote storage they were
created in. Moving a stack between them is a dump and restore, not a setting.

Set the console's `LOAMS_ENGINE_URL` to `https://loams.<tailnet>.ts.net`.
Stop with `docker compose --profile '*' down`; add `--volumes` to delete the
data.

### House

The House runs by default, linked to the engine (`LOAMS_HOUSE_ENDPOINT`,
`https://house:8443`). Its own tailnet node is the `house-tailnet` profile:

```sh
echo 'HOUSE_PRINCIPALS=tailscale:you@example.com=read+write+ddl' >> .env   # who may use it
# .env: COMPOSE_PROFILES=house-tailnet (with any others, comma-separated)
docker compose up -d
docker compose exec house cat /var/lib/loams-house/house.key   # user `default`'s password
curl -u "default:$PASSWORD" https://loams-house.<tailnet>.ts.net/ --data-binary 'SELECT 1'
```

The House's plaintext listener is on its loopback only; `house-tailscale`, in its
network namespace, serves it on the tailnet as `HOUSE_TS_HOSTNAME`
(`loams-house`), with `HOUSE_TS_AUTHKEY` (or a reusable `TS_AUTHKEY`). Its TLS
listener (8443, HS1 Task 20) is the engine's link: on `house-link`, a network
only the engine and the House join, with a certificate the House makes on first
start. The engine trusts that certificate alone and signs in with the link key,
both in the `house-link` volume, after its own auth has let the caller in, and
names that caller's Loams principal. The House gives each principal the scopes
(`read`, `write`, `ddl`, `admin`) `HOUSE_PRINCIPALS` lists; a principal not
listed has no House access, and every statement is recorded with its principal
in `/var/lib/loams-house/audit.log`. With `LOAMS_HOUSE_ENDPOINT` set (the
default), `loams.house.v1` and the console's House page reach the House; the
engine starts once the House is healthy, since it reads the link certificate
at start. An empty `LOAMS_HOUSE_ENDPOINT=` leaves the link off. The link needs `loams` and `loams-fabric` 0.1.0 images or later. The House is also on the `house` network, which nothing else
joins. Workers run sealed: `HOUSE_SANDBOX=pods` (default;
Landlock with TCP rules on Linux 6.7 or later, and seccomp) or `netns` (adds a
user and network namespace per worker; needs unprivileged user namespaces in the
container, as Podman allows). chDB's access control (R-ACL.1) and the statement
checks are always on.

### Postgres control plane

The `postgres-control` profile adds Loams Postgres's control plane: `pg-control`
serves `loams.postgres.v1` on `127.0.0.1:7680` and provisions projects,
branches, endpoints and their computes on the Postgres stack (which must be
included), suspending computes that sit idle.

```sh
# .env: CONTAINER_SOCKET and CONTAINER_SOCKET_GID (below)
echo '{"keys": []}' > secrets/pg-api-keys.json
out=$(docker compose run --rm --no-deps pg-control pg-api-key \
        --subject you@example.com --grant admin:namespace:default)
jq '{keys: [.entry]}' <<<"$out" > secrets/pg-api-keys.json
jq -r .api_key <<<"$out"        # shown only this once
# .env: COMPOSE_PROFILES=postgres-control (with any others, comma-separated)
docker compose up -d pg-control pg-runner
curl -sS -H "authorization: Bearer $KEY" -H 'content-type: application/json' \
  -d '{"namespace": "default", "name": "app", "idempotency_key": "1"}' \
  http://127.0.0.1:7680/loams.postgres.v1.PostgresService/CreateProject
```

**The container runtime's socket goes to `pg-runner` alone.** Whatever holds a
Docker or Podman socket can do anything its owner can, so neither the engine
nor `pg-control` gets it. `pg-runner`, the compute runner, holds it and serves
`pg-control` only:

- its network, `pg-runner`, is internal (no route out) and only `pg-control`
  joins it; every call needs the token it writes into the `pg-runner-token`
  volume, which `pg-control` mounts read-only;
- it starts only the compute images pinned in
  [`deploy/pins/loams-postgres-images.toml`](../pins/loams-postgres-images.toml),
  never a `neon-archive` copy, from compose files it renders itself: no mounts,
  no privileges, no host network, the compose project `loams-pg-<namespace>`,
  on `PG_COMPUTE_NETWORK` (`loams_default`), each endpoint's port (55500 to
  55999) published on `PG_ENDPOINT_BIND` (`127.0.0.1`);
- it runs as uid 10001 with every capability dropped, and the socket's group
  (`CONTAINER_SOCKET_GID`).

Prefer **rootless Podman's** socket: then the runner can do at most what your
user can. `systemctl --user enable --now podman.socket`, then in `.env`
`CONTAINER_SOCKET=$XDG_RUNTIME_DIR/podman/podman.sock` (written out) and
`CONTAINER_SOCKET_GID=0`. With rootful Docker, `CONTAINER_SOCKET` is
`/var/run/docker.sock` (root on this machine) and `CONTAINER_SOCKET_GID` is the
docker group's id (`stat -c %g /var/run/docker.sock`). Under SELinux, add
`security_opt: [label=disable]` to `pg-runner` (a socket cannot be relabelled).

The computes belong to their own compose projects: before `docker compose down`
of the stack, suspend or delete the endpoints (or `docker compose -p
loams-pg-<namespace> down`), or the stack's network stays in use.

### Auth

The engine reads `LOAMS_AUTH_MODE` (`tailscale`, `oidc`, `apikey`, or `off`,
which serves loopback only). The default is `tailscale`: the engine trusts the
`Tailscale-User-Login` headers that the sidecar's `tailscale serve` proxy adds,
on `LOAMS_TAILSCALE_LISTEN` (`127.0.0.1:7781`) only. That listener is the serve
target and nothing else, and no container but the sidecar shares the engine's
network namespace. `oidc` checks JWTs from
`LOAMS_OIDC_ISSUER` (default `https://auth.loams.dev`) against its JWKS with
audience `LOAMS_OIDC_AUDIENCE`; `apikey` checks hashed keys in
`LOAMS_API_KEYS_FILE`, which compose points at `secrets/api-keys`. See
`.env.example`.

With any mode but `off`, first-run setup (the console's setup page, which
creates the org and its owner) needs `LOAMS_SETUP_TOKEN` in `.env` or in
`secrets/engine.env` (`openssl rand -hex 32`); without it setup is refused. Setup runs once. See
"First-run setup" in [docs/release/preview.md](../../docs/release/preview.md#first-run-setup).

`console/dist` holds a placeholder page, in a clone and in the compose bundle.
For `--profile console`, build the console (`cd web && pnpm build`) and set
`LOAMS_CONSOLE_DIR` to `web/apps/console/dist`.

### Headscale (coming soon, unsupported in 0.1.1)

0.1.1 supports one setup: your own Tailscale tailnet and the hosted
console. Headscale issues no `ts.net` HTTPS certificates, so the hosted console
(served over HTTPS) cannot reach an engine on a Headscale tailnet. The profile
stays in the bundle for a later release; the steps below are unsupported.

```sh
# .env: HEADSCALE_SERVER_URL=https://hs.example.com, TS_LOGIN_SERVER=the same,
#       TS_SERVE_MODE=http, HEADSCALE_BIND=0.0.0.0 (behind TLS) to let devices join
docker compose --profile headscale up -d headscale
docker compose exec headscale headscale users create loams
docker compose exec headscale headscale users list            # its id
docker compose exec headscale headscale preauthkeys create --user <id> --reusable
# put the key in TS_AUTHKEY, then
docker compose up -d loams tailscale
```

Headscale issues no HTTPS certificates, so the engine is served over plain
HTTP on the tailnet (`TS_SERVE_MODE=http`, inside WireGuard). A console served
over HTTPS cannot call it (mixed content); serve the console over HTTP from the
tailnet or this machine.

## Upgrading

From 0.1.0-preview.2, `loams upgrade` upgrades this stack in place, one release at a
time, with a snapshot first:

```sh
loams upgrade check --to 0.1.1  # what it will do and what would stop it
loams upgrade --to 0.1.1        # snapshot, migrate, restart, verify
loams upgrade rollback          # back to the snapshot
```

Snapshots go to the `upgrade-snapshots` volume, or to `LOAMS_UPGRADE_SNAPSHOT_DIR`
on this host. Do not upgrade by editing `LOAMS_VERSION` and running `docker compose
up`. The [upgrade guide](../../docs/release/upgrades.md) has the steps, rollback and
every error code.

An upgrade keeps a stack on the stores it runs on. A stack from 0.1.0 or
earlier runs PD, TiKV and RustFS, which the light default leaves out: before
upgrading it, add `COMPOSE_PROFILES=tikv,rustfs` to its `.env`. Without them
the new compose file starts the embedded stores, with an empty catalog, beside
its data (which stays in its volumes, unread), and Loams Postgres finds no
timelines. `loams upgrade`
upgrades a stack with the `tikv` profile; one on the embedded stores is
refused for now (`stack_unrecognised`), because `loams migrate` cannot open
the embedded metastore while the engine runs. Upgrade such a stack by hand:
back up the volumes, `docker compose stop`, set `LOAMS_VERSION`, and
`docker compose up -d` (`migrate` runs before the engine, and the engine's
startup gate refuses data with a migration still pending).

### Durable execution's store

The engine keeps Loams Durable (the embedded Resonate server) in the SQLite
file `durable/default.db` in the `loams-data` volume, with or without the
`tikv` profile (D1342). For several engines on one TiKV, set
`LOAMS_DURABLE_STORE=tikv://pd:2379/loams_durable` (with the `tikv` profile):
a fresh keyspace that the engine creates on first start when PD has none.
**Nothing copies the SQLite state into TiKV** (`loams durable migrate` brings a
store's schema up to date; it does not move data between stores), so set it
before the first start, or drain first: let pending promises finish
(`promise.search` with `"state": "pending"`), note your schedules
(`schedule.search`) and create them again afterwards. The SQLite file stays in
`loams-data`, unread. BR snapshots cover the keyspace with the rest of TiKV.

## On your own VM (BYOC)

```sh
deploy/release/byoc/install.sh ubuntu@203.0.113.10            # asks for TS_AUTHKEY
deploy/release/byoc/uninstall.sh ubuntu@203.0.113.10          # --purge deletes the data
```

`install.sh` installs Docker if it is missing (get.docker.com), copies this
directory and `deploy/loams-postgres-dev` to `~/loams`, writes `.env` with mode
0600 and starts the light default stack. `--tikv` adds PD, TiKV and TiDB,
`--rustfs` RustFS, `--all` both, `--profiles` any others, and `--no-postgres`
leaves Loams Postgres out; they go into the new `.env` as `COMPOSE_PROFILES`,
so pick them on the first install. `--services` starts only the services
named. The VM needs SSH and outbound internet only.

```sh
deploy/release/byoc/upgrade.sh ubuntu@203.0.113.10 --to 0.1.1 --check  # loams upgrade check
deploy/release/byoc/upgrade.sh ubuntu@203.0.113.10 --to 0.1.1          # loams upgrade, on the VM
deploy/release/byoc/upgrade.sh ubuntu@203.0.113.10 --to 0.1.1 --rollback
```

`upgrade.sh` fetches the target release's `loams` CLI on the VM (its Linux
package, checked against the release's `SHA256SUMS`) and runs `loams upgrade` in
`~/loams/release`, streaming its output; its exit code is the CLI's.

## Known gaps in 0.1.1

- With `LOAMS_HOUSE_ENDPOINT` empty the engine does not reach the House, so
  `loams.house.v1` answers `house_not_configured`; use the House's own tailnet
  URL (the `house-tailnet` profile).
- The default stack is one node on one disk: the embedded metastore, Live,
  Durable, the bucket and Loams Postgres's remote storage are local folders in
  volumes, with no replica. Back up the volumes; use the `tikv` and `rustfs`
  profiles, or an `s3://` bucket, for more.
- `HOUSE_SANDBOX=netns` fails under Docker's default seccomp profile (no
  unprivileged user namespaces) and the House does not start; keep `pods` there.
- On a kernel without Landlock TCP rules (before 6.7) the House's workers refuse
  to boot.
- Live's own listener (7710), durable (8001), and the MySQL and Postgres wire
  listeners of the engine are loopback only and are not forwarded over the
  tailnet. Live itself (`loams.live.v1`, with `LiveAdminService`) is served on
  the engine's main port too, so it is reachable over the tailnet behind the
  engine's auth like every other service.
- TiDB starts with a passwordless root and the Postgres stack with default
  credentials; neither is forwarded over the tailnet by default.
- `compute1`'s tenant and timeline are created on the first `up` by the one-shot
  `pg-bootstrap` service, which keeps their ids in the `pg-bootstrap` volume and
  exits 0 (so `docker compose ps` does not list it). Set both `TENANT_ID` and
  `TIMELINE_ID` in `.env` to bring your own; see
  [`../loams-postgres-dev/README.md`](../loams-postgres-dev/README.md). The
  `v0.1.0` tag predates `pg-bootstrap`: a checkout of the tag needs those two ids
  by hand (the 0.1.0 compose bundle, `v0.1.1` and later do not).
- `pg-control` (`--profile postgres-control`) keeps its records in a local file
  (one node) and reaches the Postgres stack, its computes and `pg-runner` in
  plain text on the compose networks; no PgDog fronts the computes yet (PG2c),
  so clients connect to an endpoint's published port. Its image,
  `loams-pg-control`, ships from 0.1.0.
