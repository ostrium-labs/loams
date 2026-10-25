#!/usr/bin/env bash
# Builds Titan (distributedio/titan) unmodified and runs it on mocktikv://
# (KV1 plan KV0 Task 10, step 3): does it build, does it serve?
# Needs the Go toolchain of its go.mod era on PATH. Writes
# <out>/titan-mocktikv.summary.
# Usage: titan-mocktikv.sh <port> <out>
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=pins.sh
. "$here/pins.sh"
port="$1" out="$2"
mkdir -p "$out"
work="${RUNNER_TEMP:-$out}/titan-src"
"$here/fetch.sh" "$TITAN_REPO" "$TITAN_COMMIT" "$work"
summary="$out/titan-mocktikv.summary"
echo "titan: $TITAN_REPO@$TITAN_COMMIT, $(go version)" >"$summary"
export GOFLAGS=-mod=mod GOPROXY="${GOPROXY:-https://proxy.golang.org,direct}" GO111MODULE=on
if (cd "$work" && go build -o titan ./bin/titan/) >"$out/titan-build.log" 2>&1; then
  echo "build: ok, sha256 $(sha256sum "$work/titan" | cut -d' ' -f1)" >>"$summary"
else
  echo "build: failed (titan-build.log)" >>"$summary"
  tail -20 "$out/titan-build.log" >>"$summary"
  cat "$summary"
  exit 0
fi
cat >"$work/oracle.toml" <<TOML
[server]
auth = ""
listen = "127.0.0.1:$port"
[status]
listen = "127.0.0.1:$((port + 1))"
[tikv]
pd-addrs = "mocktikv://"
TOML
(cd "$work" && ./titan -c oracle.toml) >"$out/titan-serve.log" 2>&1 &
pid=$!
served=no
for _ in $(seq 1 50); do
  if reply=$(docker run --rm --network host "$VALKEY_IMAGE" valkey-cli -p "$port" ping 2>&1) && [[ "$reply" == *PONG* ]]; then
    served=yes
    break
  fi
  sleep 0.2
done
if [[ "$served" == yes ]]; then
  set_reply=$(docker run --rm --network host "$VALKEY_IMAGE" valkey-cli -p "$port" set oracle-key v 2>&1 || true)
  get_reply=$(docker run --rm --network host "$VALKEY_IMAGE" valkey-cli -p "$port" get oracle-key 2>&1 || true)
  echo "serve: ok (PING; SET -> $set_reply; GET -> $get_reply)" >>"$summary"
else
  echo "serve: no PONG on $port (titan-serve.log)" >>"$summary"
  tail -20 "$out/titan-serve.log" >>"$summary"
fi
kill "$pid" 2>/dev/null || true
cat "$summary"
