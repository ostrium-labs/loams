#!/usr/bin/env bash
# Starts or stops the pinned Valkey on the host network.
# Usage: valkey.sh start <port> [valkey-server args...] | valkey.sh stop <port>
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=pins.sh
. "$here/pins.sh"
action="$1" port="$2"
shift 2
name="loams-kv-oracle-valkey-$port"
case "$action" in
  start)
    docker run -d --rm --name "$name" --network host "$VALKEY_IMAGE" \
      valkey-server --port "$port" --save '' --appendonly no "$@" >/dev/null
    for _ in $(seq 1 50); do
      if docker exec "$name" valkey-cli -p "$port" ${VALKEY_AUTH:+-a "$VALKEY_AUTH"} ping 2>/dev/null | grep -q PONG; then
        echo "valkey: up on $port"
        exit 0
      fi
      sleep 0.2
    done
    echo "valkey: not up on $port" >&2
    docker logs "$name" >&2 || true
    exit 1
    ;;
  stop) docker rm -f "$name" >/dev/null 2>&1 || true ;;
  *) echo "usage: valkey.sh start|stop <port>" >&2; exit 2 ;;
esac
