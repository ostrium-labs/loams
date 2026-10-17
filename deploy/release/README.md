# Loams preview: run it yourself

Everything in the preview runs on your machine or on VMs you own. Ostrium hosts
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
      pd · tikv · tidb · house · agentd · Postgres stack
```

The engine is never on the public internet: the only way in is the Tailscale
sidecar, which joins your own Tailscale tailnet and proxies `https://loams.<tailnet>.ts.net` to the engine's
tailscale listener (`LOAMS_TAILSCALE_LISTEN`, `127.0.0.1:7781`). Only the
engine and the sidecar share that network namespace; every other service is on
the compose network and cannot reach the engine's loopback listeners.
No service binds a host port on `0.0.0.0` by default.

## Images

Built by [`build.sh`](build.sh) from `Containerfile.<name>` and published to
`ghcr.io/ostrium-labs` as `:<version>` and `:preview`, for linux/amd64 and
linux/arm64. Each runs as uid 10001 on debian-slim, with tini as PID 1 and a
healthcheck.

| Image | Contains |
|---|---|
| `loams` | the engine, features `live,live-tikv,durable,graph,tikv,pgwire,mysql-wire,stream-grpc` |
| `loams-fabric` | `loams-fabric house` and `loams-house-worker`, with the digest-pinned libchdb; workers sealed with `--sandbox pods` by default |
| `loams-agentd` | the agent daemon (optional) |
| `loams-pg-control` | `loams` with features `postgres,tikv` and the standalone `docker-compose` CLI: `pg-control` and `pg-runner` (`--profile postgres-control`; from the release after 0.1.0-preview.2) |

```sh
deploy/release/build.sh loams                        # both platforms, local manifest list
deploy/release/build.sh --platforms linux/amd64 loams
deploy/release/build.sh --version 0.1.0 --push   # CI on a tag
```

## On this machine

```sh
cd deploy/release
cp .env.example .env            # set TS_AUTHKEY; see the comments
docker compose up -d loams tailscale     # engine, pd, tikv and the sidecar
docker compose up -d                     # plus tidb and the Postgres stack
docker compose logs tailscale            # the node name, or a login URL
```

Set the console's `LOAMS_ENGINE_URL` to `https://loams.<tailnet>.ts.net`.
Profiles add `headscale` (coming soon, unsupported in 0.1.0-preview.2), `console` (the console from `LOAMS_CONSOLE_DIR`, with
`/config.json` built from `LOAMS_ENGINE_URL`, `LOAMS_AUTH_ISSUER` and
`LOAMS_AUTH_CLIENT_ID`), `agentd` and `house` (below). Stop with `docker compose --profile '*'
down`; add `--volumes` to delete the data.

### House

```sh
echo 'LOAMS_HOUSE_ENDPOINT=https://house:8443' >> .env          # wire the engine to it
echo 'HOUSE_PRINCIPALS=tailscale:you@example.com=read+write+ddl' >> .env   # who may use it
docker compose --profile house up -d house house-tailscale loams
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
in `/var/lib/loams-house/audit.log`. With `LOAMS_HOUSE_ENDPOINT` set,
`loams.house.v1` and the console's House page reach the House. The link needs `loams` and `loams-fabric` images built after
0.1.0-preview.2. The House is also on the `house` network, which nothing else
joins. Workers run sealed: `HOUSE_SANDBOX=pods` (default;
Landlock with TCP rules on Linux 6.7 or later, and seccomp) or `netns` (adds a
user and network namespace per worker; needs unprivileged user namespaces in the
container, as Podman allows). chDB's access control (R-ACL.1) and the statement
checks are always on.

### Postgres control plane

`--profile postgres-control` adds Loams Postgres's control plane: `pg-control`
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
docker compose --profile postgres-control up -d pg-control pg-runner
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

### Headscale (coming soon, unsupported in 0.1.0-preview.2)

0.1.0-preview.2 supports one setup: your own Tailscale tailnet and the hosted
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
loams upgrade check --to 0.1.0  # what it will do and what would stop it
loams upgrade --to 0.1.0        # snapshot, migrate, restart, verify
loams upgrade rollback          # back to the snapshot
```

Snapshots go to the `upgrade-snapshots` volume, or to `LOAMS_UPGRADE_SNAPSHOT_DIR`
on this host. Do not upgrade by editing `LOAMS_VERSION` and running `docker compose
up`. The [upgrade guide](../../docs/release/upgrades.md) has the steps, rollback and
every error code.

## On your own VM (BYOC)

```sh
deploy/release/byoc/install.sh ubuntu@203.0.113.10            # asks for TS_AUTHKEY
deploy/release/byoc/uninstall.sh ubuntu@203.0.113.10          # --purge deletes the data
```

`install.sh` installs Docker if it is missing (get.docker.com), copies this
directory and `deploy/loams-postgres-dev` to `~/loams`, writes `.env` with mode
0600 and starts `loams tailscale tidb` (`--all` for everything). The VM
needs SSH and outbound internet only.

```sh
deploy/release/byoc/upgrade.sh ubuntu@203.0.113.10 --to 0.1.0 --check  # loams upgrade check
deploy/release/byoc/upgrade.sh ubuntu@203.0.113.10 --to 0.1.0          # loams upgrade, on the VM
deploy/release/byoc/upgrade.sh ubuntu@203.0.113.10 --to 0.1.0 --rollback
```

`upgrade.sh` fetches the target release's `loams` CLI on the VM (its Linux
package, checked against the release's `SHA256SUMS`) and runs `loams upgrade` in
`~/loams/release`, streaming its output; its exit code is the CLI's.

## Known gaps in the preview

- With 0.1.0-preview.2 images the engine does not reach the House: its TLS link
  (HS1 Task 20) needs later images, so `loams.house.v1` answers
  `house_not_configured`; use the House's own tailnet URL (`--profile house`).
- `HOUSE_SANDBOX=netns` fails under Docker's default seccomp profile (no
  unprivileged user namespaces) and the House does not start; keep `pods` there.
- With 0.1.0-preview.2 images the House takes no `INSERT` into its tables (HS1 Task 12 is
  in later images). On a kernel without Landlock TCP rules (before 6.7) the workers refuse
  to boot.
- Live's own listener (7710), durable (8001), and the MySQL and Postgres wire
  listeners of the engine are loopback only and are not forwarded over the
  tailnet. Live itself (`loams.live.v1`, with `LiveAdminService`) is served on
  the engine's main port too, so it is reachable over the tailnet behind the
  engine's auth like every other service.
- TiDB starts with a passwordless root and the Postgres stack with default
  credentials; neither is forwarded over the tailnet by default.
- `compute1` needs `TENANT_ID` and `TIMELINE_ID` (see
  [`../loams-postgres-dev/README.md`](../loams-postgres-dev/README.md)).
- `pg-control` (`--profile postgres-control`) keeps its records in a local file
  (one node) and reaches the Postgres stack, its computes and `pg-runner` in
  plain text on the compose networks; no PgDog fronts the computes yet (PG2c),
  so clients connect to an endpoint's published port. Its image ships from
  the release after 0.1.0-preview.2; until then build it with
  `build.sh loams-pg-control`.
