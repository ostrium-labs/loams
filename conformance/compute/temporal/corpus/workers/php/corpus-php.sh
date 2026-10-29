#!/bin/sh
# The PHP corpus's entry point inside its image (DX1 TSDK-php): starts
# RoadRunner (`rr serve`, .rr.yaml, worker.php), waits until its temporal
# plugin has connected, then runs the starter or the replayer against it,
# and stops RoadRunner.
#
#   corpus-php.sh generate <address> <namespace> <out.json> [scenario,...]
#   corpus-php.sh replay   <address> <namespace>     # CORPUS_HISTORIES=dir
#   corpus-php.sh check                              # no server: tests/scenarios_check.php
#
# RoadRunner's client dials the server at start (GetSystemInfo), so the
# replay needs a reachable frontend too, although it sends it nothing else.
# RR_RPC_PORT (6001) is RoadRunner's loopback RPC port, which the replayer
# calls. RoadRunner's log is printed at the end (and on failure), so the leg's log
# shows the plugin's side.
set -eu
cd "$(dirname "$0")"
mode=${1:?usage: corpus-php.sh generate|replay|check ...}
shift
if [ "$mode" = check ]; then
  exec php tests/scenarios_check.php
fi
TEMPORAL_ADDRESS=${1:?address}
TEMPORAL_NAMESPACE=${2:?namespace}
export TEMPORAL_ADDRESS TEMPORAL_NAMESPACE
shift 2
log=${RR_LOG:-/tmp/rr.log}
: "${RR_RPC_PORT:=6001}"
export RR_RPC_PORT
rr serve -c .rr.yaml >"$log" 2>&1 &
rr_pid=$!
stop() {
  kill "$rr_pid" 2>/dev/null || true
  wait "$rr_pid" 2>/dev/null || true
  echo "--- RoadRunner log (tail) ---"
  tail -n 80 "$log" || true
}
trap stop EXIT

ready=
for _ in $(seq 120); do
  if ! kill -0 "$rr_pid" 2>/dev/null; then break; fi
  if grep -q 'connected to temporal server' "$log"; then ready=1; break; fi
  sleep 0.5
done
if [ -z "$ready" ]; then
  echo "corpus-php.sh: RoadRunner's temporal plugin did not connect to $TEMPORAL_ADDRESS" >&2
  exit 1
fi
# The workers poll a moment after the client connects.
sleep 2

case $mode in
  generate) php starter.php "$TEMPORAL_ADDRESS" "$TEMPORAL_NAMESPACE" "$@" ;;
  replay) RR_RPC=tcp://127.0.0.1:$RR_RPC_PORT php replay.php ;;
  *) echo "corpus-php.sh: unknown mode $mode" >&2; exit 2 ;;
esac
