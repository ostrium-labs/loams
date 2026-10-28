#!/usr/bin/env bash
# Start SigNoz (community build) for the CFB benchmark: UI http://127.0.0.1:${HE0_SIGNOZ_PORT:-3301},
# OTLP gRPC 127.0.0.1:4317, HTTP 127.0.0.1:4318. `up.sh down` stops it (volumes are kept
# unless CFB_SIGNOZ_WIPE=1).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"
export CFB_SIGNOZ_DIR="$here" HE0_SIGNOZ_PORT="${HE0_SIGNOZ_PORT:-3301}" HE0_SIGNOZ_CH_PORT="${HE0_SIGNOZ_CH_PORT:-28147}"
# HE0_CLICKHOUSE_IMAGE may point at a ClickHouse image already on the host (the
# compose pins clickhouse-server 26.9.2.8 by digest).
export CFB_REPO="$repo"
files=(-f "$repo/conformance/clickhouse/apps/signoz/compose.yaml" -f "$here/compose.cfb.yaml")
[[ -z "${HE0_CLICKHOUSE_IMAGE:-}" ]] || files+=(-f "$here/compose.ch-local.yaml")
if [[ "${1:-up}" == down ]]; then
  if [[ "${CFB_SIGNOZ_WIPE:-0}" == 1 ]]; then podman compose "${files[@]}" down -v; else podman compose "${files[@]}" down; fi
  exit
fi
podman compose "${files[@]}" up -d
for _ in $(seq 1 120); do
  curl -fsS "http://127.0.0.1:$HE0_SIGNOZ_PORT/api/v2/healthz" >/dev/null 2>&1 && { echo "SigNoz up: http://127.0.0.1:$HE0_SIGNOZ_PORT"; exit 0; }
  sleep 5
done
echo "SigNoz did not become healthy" >&2; exit 1
