![Loams — Your data. Your bucket.](../../docs/assets/loams-banner.svg)

# Loams Postgres on local folders or RustFS (development)

This directory was the `neon` stack under `deploy/` until NF1 Task 1b (D823). The compose project is
still named `loams-neon`, because Docker names users' volumes after it.

The stack from the Neon + WeSQL spike ([§23](../../docs/design/23-neon-and-wesql.md)). It runs
Neon's storage (storage broker, one pageserver, one safekeeper) and starts Postgres computes
against an explicit tenant and timeline. The pageserver's and the safekeeper's remote storage is
a local folder in each one's volume by default (Neon's `LocalFs`: one disk, one copy); with the
`rustfs` profile (`COMPOSE_PROFILES=rustfs` in `.env` or the shell) it is RustFS's `neon` bucket
instead. Pick one before the first start: a timeline stays in the storage it was created in. Loams does not link Neon; in the
design, Loams plays the part of the control plane that the commands below play by hand.

It is derived from `neondatabase/neon` `docker-compose/` (Apache-2.0). `compute/config.json` is
Neon's compute spec with one safekeeper; `compute/start.sh` replaces Neon's `compute.sh`, because
the `compute-node` image has no `curl`, `jq` or `nc`, and tenants and timelines are created by
the caller: the one-shot `pg-bootstrap` service for `compute1` (below), Loams, or the curl calls
in [Run](#run).

Not a production layout: no storage controller, no Neon proxy, no TLS, default credentials,
`fsync = off` in the compute spec, and remote storage on the same disk as the data by default.

The compute spec keeps Postgres's default `shared_buffers` (128 MB) and turns on a 256 MB local
file cache, as Loams's own spec does (`crates/loams-pg-control/src/upcall/spec.rs`). Neon's
docker-compose uses `shared_buffers = 1MB` with no file cache, which sends every catalog read to
the pageserver; after DDL those reads wait for WAL ingest, sometimes for seconds, and clients with
short pool timeouts failed (#904).
Images are pinned by digest in [`deploy/pins/loams-postgres-images.toml`](../pins/loams-postgres-images.toml)
(NF1 Task 2). `scripts/nf1/pins.py set` writes that file and generates every consumer from it:
this stack's and `deploy/loams-pg-bench`'s `x-neon-image` and `x-compute-image`, the release
stack's `endpoint-storage` image (`deploy/release/compose.yaml`), `PG_HEADERS_IMAGE` in `pg2.yml`,
`pg2-e2e.yml` and `loams-pg-bench.yml`, and the table below; `pins.py check` fails on any
difference. `NEON_IMAGE` and `COMPUTE_IMAGE` still override the pins.

Since NF1 Task 14 the pins are our own builds: a Loams Postgres release (`lp-YYYY.MM.N`) of
`ostrium-labs/loams-postgres`, each image signed and attested by the fork's release workflow
(`scripts/nf1/verify-pins.sh` checks them in `nf1-pins.yml`). The first, `lp-2026.10.1`, has the
same Postgres revisions as Neon's last public build (16.9 and 17.5); `lp-2026.11.0` moves to the
minors 16.15 and 17.11 (NF1 Task 17). Until then, the pins were
that build, 2025-08-26, git `77e22e4b`, at Neon's own repositories, with `mirror:` sources; its
amd64 manifests were `sha256:ead56a7b33925ca4df9f1ee0d29f55fa25e165a3fee6a4f19055050c68e8cad0`
(`neon`) and `sha256:9b86e3ecb2267fbdeb0fd2478db0e662959ccdfa4526efa4a558410a93c6c46f`
(`compute-node-v17`). The mirrored packages (see Mirror) stay private and are never deployed.

<!-- pins.py: begin images (generated from deploy/pins/loams-postgres-images.toml) -->
Pinned in [`deploy/pins/loams-postgres-images.toml`](../pins/loams-postgres-images.toml): `fork_release` `lp-2026.11.0`, `fork_commit` `a213ec44ac29c31812712ffdd6de7c1f4d4a599c`.

| Pin | Image (multi-arch index) | Source | Postgres |
|---|---|---|---|
| `storage` | `ghcr.io/ostrium-labs/loams-postgres-storage@sha256:4dcec4cbef637b92b1088ea2c0bc857f3b5ef4318f02e8d2d4a0f9263ed4140e` | `ostrium-labs/loams-postgres@a213ec44ac29c31812712ffdd6de7c1f4d4a599c` | 16.15, 17.11 |
| `compute-v16` | `ghcr.io/ostrium-labs/loams-postgres-compute-v16@sha256:29842a21499b282b97420b141d11fd3923b9102ce0f37c14491581c5f6591434` | `ostrium-labs/loams-postgres@a213ec44ac29c31812712ffdd6de7c1f4d4a599c` | 16.15 |
| `compute-v17` | `ghcr.io/ostrium-labs/loams-postgres-compute-v17@sha256:210b71416c827b6cf7842853706e1dbdcc870fd4a7af8e235d6e7296b95edb0e` | `ostrium-labs/loams-postgres@a213ec44ac29c31812712ffdd6de7c1f4d4a599c` | 17.11 |

Postgres majors: supported 16, 17; beta none; maintenance none; default 17.
<!-- pins.py: end images -->

Postgres is 17 (PG2 Task 0 ruling 7).

Resources: about 1.9 GB (`loams-postgres-storage`) plus 0.4 GB (`loams-postgres-compute-v17`) of
compressed amd64 layers to pull; storage services under 0.5 GB of RAM together (measured in the
spike).

## Mirror

NF1 Task 1 copied Neon's last public build (git `77e22e4b`, Neon build `8464`, 2025-08-26)
to `ghcr.io/ostrium-labs/neon-archive/`, registry to registry with unchanged digests, so it
survives Neon deleting its packages. These are Neon's binaries under Neon's names (D822), not
Loams Postgres. The packages are **private** (Q715) until the owner has read the audit below.

| Source (Neon) | Source digest | Mirror | Mirror digest | Audit |
|---|---|---|---|---|
| `ghcr.io/neondatabase/neon` | `sha256:7a4f124917bb929964b2d696d710f19584f80bb9bd51b2af4a6e2425434c761f` | `ghcr.io/ostrium-labs/neon-archive/neon:mirror-77e22e4b` | `sha256:7a4f124917bb929964b2d696d710f19584f80bb9bd51b2af4a6e2425434c761f` | CDDL, private source ([audit](../pins/audit/sha256-7a4f124917bb929964b2d696d710f19584f80bb9bd51b2af4a6e2425434c761f.json)) |
| `ghcr.io/neondatabase/compute-node-v17` | `sha256:13ab146d3e7bbabb25a8532f315ac443e7512351d1ede0bab586def5c70e26c3` | `ghcr.io/ostrium-labs/neon-archive/compute-node-v17:mirror-77e22e4b` | `sha256:13ab146d3e7bbabb25a8532f315ac443e7512351d1ede0bab586def5c70e26c3` | CDDL ([audit](../pins/audit/sha256-13ab146d3e7bbabb25a8532f315ac443e7512351d1ede0bab586def5c70e26c3.json)) |
| `ghcr.io/neondatabase/compute-node-v16` | `sha256:b3e151661bd2ee11eb2843c8926001966cb23969227e9673c5f42fc3fbe14249` | `ghcr.io/ostrium-labs/neon-archive/compute-node-v16:mirror-77e22e4b` | `sha256:b3e151661bd2ee11eb2843c8926001966cb23969227e9673c5f42fc3fbe14249` | CDDL ([audit](../pins/audit/sha256-b3e151661bd2ee11eb2843c8926001966cb23969227e9673c5f42fc3fbe14249.json)) |
| `ghcr.io/neondatabase/build-tools` | `sha256:bd1fe230b33aab4d7c2b336311aa8f96733a32a4eed4d61eecfb1c6b6d8cf6e3` | `ghcr.io/ostrium-labs/neon-archive/build-tools:mirror-77e22e4b` | `sha256:bd1fe230b33aab4d7c2b336311aa8f96733a32a4eed4d61eecfb1c6b6d8cf6e3` | clean ([audit](../pins/audit/sha256-bd1fe230b33aab4d7c2b336311aa8f96733a32a4eed4d61eecfb1c6b6d8cf6e3.json)) |

`compute-node-v16` is the tag `8464` and `build-tools` the tag
`d4a4001a8381b2f2b33462941d4382ad5c71f4a39c12a4c66cec0397abd8d160-bookworm`; both were matched to
`77e22e4b` through the images' SLSA provenance (`GIT_VERSION`, and the `build-tools` material of
the `neon` build). Each index keeps BuildKit's attestation manifests (SPDX SBOM and SLSA
provenance), copied byte for byte with the images.

Audit (`scripts/nf1/audit-image.sh`, 2026-10-10/11; every platform):

- **`neon`: CDDL and a private source.** The image has ten Rust binaries with `.dep-v0` per
  platform, plus `neon.so` for v14 to v17. Nine of the binaries (`pageserver`, `safekeeper`,
  `proxy`, `pg_sni_router`, `storage_broker`, `storage_controller`, `storage_scrubber`, `pagectl`
  and `neon_local`; all but `endpoint_storage`) link `inferno` 0.11.21 and 0.12.0 (CDDL-1.0,
  through `pprof`). `proxy` and `pg_sni_router` also link `subzero-core` 3.0.1 from git: Neon's
  `Dockerfile` replaces the public path stub with the private `neondatabase/subzero` (rev
  `39626461`) when its CI supplies `SUBZERO_ACCESS_TOKEN`. 40 findings in all.
- **`compute-node-v17` and `compute-node-v16`: CDDL.** `local_proxy` links `inferno` 0.11.21 and
  0.12.0 (CDDL-1.0, through `pprof`), on amd64 and arm64. Neon builds the compute image with plain
  `cargo build`, so none of its 14 Rust binaries per platform (`compute_ctl`, `fast_import`,
  `local_proxy` and the Rust extensions, such as `neon.so`, `pg_graphql.so`, `pg_session_jwt.so` and
  `rag.so`) has a `.dep-v0` section. Their lists come from the Cargo source paths left in each
  binary (`"evidence": "strings"`), which is a lower bound. No private `subzero` source. The git
  sources are `framed-websockets`, `rust-postgres` (both forked by us) and `tiktoken-rs`.
- **`build-tools`: clean.** It has 26 binaries with package data (the `cargo auditable install`
  tools, `rustup`, and the Rust parts of the Python wheels), and no CDDL or private source.
  It is a build image and is never deployed.

So `neon`, `compute-node-v17` and `compute-node-v16` stay private: Q715's default, "a
package with a finding stays private". Making `build-tools` public is the owner's decision (step 5).
Our own builds drop `inferno` (Task 7, `pprof` without `flamegraph`).

Runbook (registry to registry; nothing is pulled into a local image store):

1. Log in with a token that has `write:packages`, into a private auth file:
   `skopeo login --authfile "$REGISTRY_AUTH_FILE" -u <user> --password-stdin ghcr.io`.
2. Copy and verify each image with `scripts/nf1/mirror-images.sh <src>@sha256:<digest> ghcr.io/ostrium-labs/neon-archive/<name> mirror-77e22e4b`; it fails unless the index and every child manifest are byte-identical to the source.
3. Run `scripts/nf1/audit-image.sh ghcr.io/ostrium-labs/neon-archive/<name>@sha256:<digest> > deploy/pins/audit/sha256-<hex>.json` on each mirrored index (exit 3 is a finding: CDDL or a private source).
4. Record both digests and the findings in the table above and in the NF1 plan's rulings; check that each package is still private (`gh api orgs/ostrium-labs/packages/container/neon-archive%2F<name> -q .visibility`).
5. Only after the audit, and only by the owner: make a package public. A package with a finding stays private.

## Run

With Docker, or Podman plus `DOCKER_HOST=unix:///run/user/$UID/podman/podman.sock`:

```sh
docker compose up -d
# The compute takes a few seconds to accept connections.
until pg_isready -q -h 127.0.0.1 -p 55433; do sleep 1; done
PGPASSWORD=cloud_admin psql -h 127.0.0.1 -p 55433 -U cloud_admin postgres
```

`compute1` attaches to a tenant and a timeline that must exist on the pageserver. The one-shot
`pg-bootstrap` service ([`bootstrap/pg-bootstrap.sh`](bootstrap/pg-bootstrap.sh), in the storage
image, on the compose network only) makes sure they do, then exits 0, and `compute1` starts only
after it succeeded (`service_completed_successfully`):

- With `TENANT_ID` and `TIMELINE_ID` empty (the default), the first run creates a tenant
  (`PUT /v1/tenant/<id>/location_config`, `AttachedSingle`, generation 1) and its first timeline
  (`POST /v1/tenant/<id>/timeline/`, `pg_version` 17, which must match the compute image; set
  `COMPUTE_PG_MAJOR` with a `COMPUTE_IMAGE` of another major) and writes their ids to `ids.env`
  in the `pg-bootstrap` volume. `compute/start.sh` reads them from there.
- Every later run reuses the stored ids, so `down` (without `-v`) and `up` come back to the same
  data. An existing tenant is not attached again, and an existing timeline is not created again.
- With both set in `.env` (or the shell), those win: `pg-bootstrap` only creates any that are
  missing and leaves `ids.env` alone, and `compute1` uses them. Setting only one is refused.

`docker compose logs pg-bootstrap` prints the ids. To read them into the shell, for the branch
below:

```sh
eval "$(docker compose exec compute1 cat /var/lib/pg-bootstrap/ids.env | sed 's/^/export /')"
```

By hand, without `pg-bootstrap` (what it does, for a tenant of your own):

```sh
docker compose up -d storage_broker pageserver safekeeper1   # with the rustfs profile, rustfs and create-bucket too

# Create a tenant and its first timeline through the pageserver API. The timeline's
# Postgres version must match the compute image (loams-postgres-compute-v17).
export TENANT_ID=$(openssl rand -hex 16) TIMELINE_ID=$(openssl rand -hex 16)
curl -X PUT -H 'Content-Type: application/json' \
  -d '{"mode":"AttachedSingle","generation":1,"tenant_conf":{}}' \
  localhost:9898/v1/tenant/$TENANT_ID/location_config
curl -X POST -H 'Content-Type: application/json' \
  -d "{\"new_timeline_id\":\"$TIMELINE_ID\",\"pg_version\":17}" \
  localhost:9898/v1/tenant/$TENANT_ID/timeline/

docker compose up -d compute1
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

## Another major

`compute1` and `compute2` run the default major. The stack also has one `compute-v<N>` service
per major in the pin file (generated by `scripts/nf1/pins.py`, design 62 §9.4), each behind its
profile `pg<N>` (a beta major's also behind `beta`), on `127.0.0.1:554<N>` (`PG<N>_PORT`). It
attaches only to the timeline that `PG<N>_TENANT_ID` and `PG<N>_TIMELINE_ID` name, which must be a
timeline of that major: `loams-wal` refuses a compute of another major.

```sh
export PG16_TENANT_ID=$TENANT_ID PG16_TIMELINE_ID=$(openssl rand -hex 16)
curl -X POST -H 'Content-Type: application/json' \
  -d "{\"new_timeline_id\":\"$PG16_TIMELINE_ID\",\"pg_version\":16}" \
  localhost:9898/v1/tenant/$PG16_TENANT_ID/timeline/
docker compose --profile pg16 up -d compute-v16
until pg_isready -q -h 127.0.0.1 -p 55416; do sleep 1; done
```

## Stop

```sh
docker compose --profile "*" down
```

To delete the data as well (destructive: removes the remote storage, the RustFS bucket with the
profile, every tenant and `pg-bootstrap`'s stored ids):

```sh
docker compose --profile branch down -v
```
