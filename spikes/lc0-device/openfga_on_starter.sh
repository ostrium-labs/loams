#!/usr/bin/env bash
# LC0 Task 5 (partial): does OpenFGA's MySQL datastore run on TiDB Cloud
# Starter (design 78 §17)? Runs OpenFGA at IA0's pin against a free Starter
# instance: migrate, serve on loopback only, write a small model and tuples,
# check, list-objects, revoke. Spike only; not product code.
#
# Usage: openfga_on_starter.sh <cluster-id> <user-prefix>
# Needs: podman, uv, and `ti db create-db-sql-users` run for the cluster.
set -euo pipefail
CLUSTER=$1 PREFIX=$2
HERE=$(cd "$(dirname "$0")" && pwd)
IMG=docker.io/openfga/openfga:v1.22.0@sha256:9cf9a20af32a40434cd3a788c429879439ee9ea9abf65c9c200ee17cd43f769e
ENVF=$(mktemp -p "${XDG_RUNTIME_DIR:-$HOME}" lc0-fga.XXXXXX)
PY="uv run --no-project --with pymysql==1.1.2 python"
cleanup() { podman rm -f lc0-openfga >/dev/null 2>&1 || true; rm -f "$ENVF"; $PY "$HERE/openfga_setup.py" "$CLUSTER" "$PREFIX" "$ENVF" --drop; }
trap cleanup EXIT
redact() { sed -E 's#[^ ]*@tcp\([^)]*\)#<dsn>#g'; }

$PY "$HERE/openfga_setup.py" "$CLUSTER" "$PREFIX" "$ENVF"
time podman run --rm --env-file "$ENVF" "$IMG" migrate 2>&1 | redact | tail -3
podman run -d --name lc0-openfga --env-file "$ENVF" -p 127.0.0.1:18080:8080 "$IMG" run --playground-enabled=false >/dev/null
for _ in $(seq 1 30); do curl -sf localhost:18080/healthz >/dev/null && break; sleep 1; done
python3 "$HERE/openfga_smoke.py" http://localhost:18080
podman logs lc0-openfga 2>&1 | redact | grep -i -E '"level":"error"|ERROR' | head -5 || true
