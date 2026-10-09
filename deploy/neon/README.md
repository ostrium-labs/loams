![Loams — Your data. Your bucket.](../../docs/assets/loams-banner.svg)

# Neon on RustFS (development)

The stack from the Neon + WeSQL spike ([§23](../../docs/design/23-neon-and-wesql.md)). It runs
Neon's storage (storage broker, one pageserver, one safekeeper) on a RustFS bucket and starts
Postgres computes against an explicit tenant and timeline. Loams does not link Neon; in the
design, Loams plays the part of the control plane that the commands below play by hand.

It is derived from `neondatabase/neon` `docker-compose/` (Apache-2.0). `compute/config.json` is
Neon's compute spec with one safekeeper; `compute/start.sh` replaces Neon's `compute.sh`, because
the `compute-node` image has no `curl`, `jq` or `nc`, and tenants and timelines are created by
the caller.

Not a production layout: no storage controller, no Neon proxy, no TLS, default credentials,
`fsync = off` in the compute spec, and the safekeeper keeps WAL in its container filesystem.
Images are pinned by digest (PG2 Task 2): Neon's last public build, 2025-08-26, git
`77e22e4b`. The fork `ostrium-labs/neon` publishes no images yet (PG2 ruling R2.1);
`NEON_IMAGE` and `COMPUTE_IMAGE` override the pins.

| Image | Digest (multi-arch index) | amd64 manifest |
|---|---|---|
| `ghcr.io/neondatabase/neon` | `sha256:7a4f124917bb929964b2d696d710f19584f80bb9bd51b2af4a6e2425434c761f` | `sha256:ead56a7b33925ca4df9f1ee0d29f55fa25e165a3fee6a4f19055050c68e8cad0` |
| `ghcr.io/neondatabase/compute-node-v17` | `sha256:13ab146d3e7bbabb25a8532f315ac443e7512351d1ede0bab586def5c70e26c3` | `sha256:9b86e3ecb2267fbdeb0fd2478db0e662959ccdfa4526efa4a558410a93c6c46f` |

Postgres is 17 (PG2 Task 0 ruling 7).

Resources in the spike: about 5 GB (`neon`) plus 1.3 GB (`compute-node-v17`) of images; storage
services under 0.5 GB of RAM together.

## Run

With Docker, or Podman plus `DOCKER_HOST=unix:///run/user/$UID/podman/podman.sock`:

```sh
docker compose up -d rustfs create-bucket storage_broker pageserver safekeeper1

# Create a tenant and its first timeline through the pageserver API. The timeline's
# Postgres version must match the compute image (compute-node-v17).
export TENANT_ID=$(openssl rand -hex 16) TIMELINE_ID=$(openssl rand -hex 16)
curl -X PUT -H 'Content-Type: application/json' \
  -d '{"mode":"AttachedSingle","generation":1,"tenant_conf":{}}' \
  localhost:9898/v1/tenant/$TENANT_ID/location_config
curl -X POST -H 'Content-Type: application/json' \
  -d "{\"new_timeline_id\":\"$TIMELINE_ID\",\"pg_version\":17}" \
  localhost:9898/v1/tenant/$TENANT_ID/timeline/

docker compose up -d compute1
# The compute takes a few seconds to accept connections.
until pg_isready -q -h 127.0.0.1 -p 55433; do sleep 1; done
PGPASSWORD=cloud_admin psql -h 127.0.0.1 -p 55433 -U cloud_admin postgres
```

## Branch

A branch is a new timeline whose ancestor is an existing one. It takes tens of milliseconds and
copies no data:

```sh
export BRANCH_TIMELINE_ID=$(openssl rand -hex 16)
curl -X POST -H 'Content-Type: application/json' \
  -d "{\"new_timeline_id\":\"$BRANCH_TIMELINE_ID\",\"ancestor_timeline_id\":\"$TIMELINE_ID\"}" \
  localhost:9898/v1/tenant/$TENANT_ID/timeline/
docker compose --profile branch up -d compute2
until pg_isready -q -h 127.0.0.1 -p 55434; do sleep 1; done
PGPASSWORD=cloud_admin psql -h 127.0.0.1 -p 55434 -U cloud_admin postgres
```

Add `"ancestor_start_lsn": "<lsn>"` to branch from a point in the past.

## Stop

```sh
docker compose --profile branch down
```

To delete the data as well (destructive: removes the RustFS bucket and every tenant):

```sh
docker compose --profile branch down -v
```
