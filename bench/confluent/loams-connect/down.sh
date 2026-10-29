#!/usr/bin/env bash
# Stop connect and the engine (SIGTERM, then SIGKILL), and the local Kafka
# container if up.sh started it. --purge also deletes $CFB_RUN (engine data,
# connect state, secrets) and removes the Kafka container.
set -uo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
stop_pid() {
  local f=$1 p
  [[ -f $f ]] || return 0
  p=$(cat "$f")
  if kill -0 "$p" 2>/dev/null; then
    kill -TERM "$p"
    for _ in $(seq 1 40); do kill -0 "$p" 2>/dev/null || break; sleep 0.25; done
    kill -0 "$p" 2>/dev/null && kill -KILL "$p"
    log "stopped $(basename "$f" .pid) ($p)"
  fi
  rm -f "$f"
}
stop_pid "$CFB_RUN/connect.pid"
stop_pid "$CFB_RUN/engine.pid"
if [[ $KAFKA_LOCAL == 1 ]] && podman container exists "$KAFKA_CONTAINER"; then
  if [[ ${1:-} == --purge ]]; then podman rm -f "$KAFKA_CONTAINER" >/dev/null; log "removed $KAFKA_CONTAINER"
  else podman stop "$KAFKA_CONTAINER" >/dev/null; log "stopped $KAFKA_CONTAINER"; fi
fi
if [[ ${1:-} == --purge ]]; then rm -rf "$CFB_RUN"; log "deleted $CFB_RUN"; fi
