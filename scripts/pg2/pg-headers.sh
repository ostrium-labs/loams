#!/usr/bin/env bash
# Extract the Postgres server headers that the Neon fork's postgres_ffi
# builds its bindings from (PG2 Task 31, ruling R31.6), from a pinned image,
# into <out>/vNN/include/postgresql/server. Point POSTGRES_INSTALL_DIR at
# <out>.
#
#   scripts/pg2/pg-headers.sh ghcr.io/neondatabase/neon@sha256:<digest> pg_install
#
# The `neon` image holds every supported major version under
# /usr/local/vNN/include/postgresql/server; the compute-node images hold
# none, so the `neon` image is the source. The image must be pinned by
# digest. CONTAINER picks the engine (docker by default; podman works).
set -euo pipefail

image=${1:?usage: pg-headers.sh <image@sha256:digest> <out-dir>}
out=${2:?usage: pg-headers.sh <image@sha256:digest> <out-dir>}
engine=${CONTAINER:-docker}

case "$image" in
  *@sha256:[0-9a-f]*) ;;
  *) echo "pg-headers: $image is not pinned by digest" >&2; exit 2 ;;
esac

id=$("$engine" create "$image" /bin/true)
trap '"$engine" rm -f "$id" >/dev/null' EXIT

found=0
for v in v14 v15 v16 v17 v18; do
  src=/usr/local/$v/include/postgresql/server
  if "$engine" cp "$id:$src" - >/dev/null 2>&1; then
    mkdir -p "$out/$v/include/postgresql"
    rm -rf "$out/$v/include/postgresql/server"
    "$engine" cp "$id:$src" "$out/$v/include/postgresql/server"
    echo "pg-headers: $v -> $out/$v/include/postgresql/server"
    found=$((found + 1))
  fi
done
if [ ! -f "$out/v17/include/postgresql/server/postgres.h" ]; then
  echo "pg-headers: no Postgres 17 server headers in $image" >&2
  exit 1
fi
echo "pg-headers: $found major version(s)"
