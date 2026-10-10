#!/usr/bin/env bash
# Start the spike's PD and TiKV (project loams-sqldb-spike) and wait for both.
set -euo pipefail
# shellcheck source=lib.sh source-path=SCRIPTDIR
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

for p in 29379 29380 30160 30180; do
  if port_open "$p"; then log "port $p is already in use"; exit 1; fi
done
spike_compose up -d
log "waiting for PD on $PD_URL"
for _ in $(seq 120); do
  curl -fsS "$PD_URL/pd/api/v1/health" 2>/dev/null | jq -e '.[0].health' >/dev/null && break
  sleep 1
done
log "waiting for TiKV to register as Up"
for _ in $(seq 180); do
  [[ "$(curl -fsS "$PD_URL/pd/api/v1/stores" 2>/dev/null | jq -r '.stores[0].store.state_name // empty')" == Up ]] && { log "stack up"; exit 0; }
  sleep 1
done
log "TiKV did not come up"; exit 1
