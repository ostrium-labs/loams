#!/usr/bin/env bash
# The ClickHouse oracle (HE0 Task 6, design §75 §15).
#
#   scripts/house/oracle.sh up              start the two reference servers
#   scripts/house/oracle.sh house <data-dir> start the chDB House on 127.0.0.1:28125
#   scripts/house/oracle.sh run [args…]     run `loams-house-mine oracle` (args passed on)
#   scripts/house/oracle.sh down            stop the servers and the House
#
# `run` targets every target in conformance/clickhouse/oracle/targets.toml;
# pass `--only clickhouse-server@24.3` (or `--only house-chdb`) for one. The
# House needs a built `loams-fabric` (LOAMS_FABRIC, default: on PATH) and its
# worker beside it.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
dir="$root/conformance/clickhouse/oracle"
# Docker Compose, or Podman's `podman compose`.
if command -v docker >/dev/null; then
  compose=(docker compose -f "$dir/compose.yaml")
else
  compose=(podman compose -f "$dir/compose.yaml")
fi
export LOAMS_ORACLE_PASSWORD="${LOAMS_ORACLE_PASSWORD:-oracle}"
house_state="${LOAMS_ORACLE_HOUSE_STATE:-${XDG_STATE_HOME:-$HOME/.local/state}/loams-oracle}"

case "${1:-}" in
  up)
    "${compose[@]}" up -d --wait
    ;;
  house)
    data="${2:?usage: oracle.sh house <data-dir>}"
    mkdir -p "$data" "$house_state"
    "${LOAMS_FABRIC:-loams-fabric}" house --single-node --data-dir "$data" \
      --house-listen 127.0.0.1:28125 >"$house_state/house.log" 2>&1 &
    echo $! >"$house_state/house.pid"
    echo "$data" >"$house_state/data-dir"
    for _ in $(seq 1 120); do
      if curl -fsS http://127.0.0.1:28125/ping >/dev/null 2>&1; then
        echo "house: up (log: $house_state/house.log)"
        exit 0
      fi
      sleep 1
    done
    echo "house: did not answer on 127.0.0.1:28125; see $house_state/house.log" >&2
    exit 1
    ;;
  run)
    shift
    if [[ -f "$house_state/data-dir" ]]; then
      key_file="$(cat "$house_state/data-dir")/house.key"
      [[ -f "$key_file" ]] && LOAMS_ORACLE_HOUSE_KEY="$(tr -d '[:space:]' <"$key_file")" && export LOAMS_ORACLE_HOUSE_KEY
    fi
    cd "$root"
    cargo run --quiet --manifest-path fabric/Cargo.toml -p loams-house-mine -- oracle "$@"
    ;;
  down)
    "${compose[@]}" down --volumes --remove-orphans
    if [[ -f "$house_state/house.pid" ]]; then
      kill "$(cat "$house_state/house.pid")" 2>/dev/null || true
      rm -rf "$house_state"
    fi
    ;;
  *)
    sed -n '2,13p' "$0"
    exit 2
    ;;
esac
