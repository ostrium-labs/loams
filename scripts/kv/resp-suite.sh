#!/usr/bin/env bash
# Runs Valkey 9.1.2's TCL suite (fetched unmodified at run time, never
# copied) against the Redis gateway on an in-memory store (plan KV2 Task 2).
# Each data-type task gates on its own unit file; Task 14 runs the whole
# report §1.4 subset in CI.
#
# Usage: scripts/kv/resp-suite.sh [unit ...]     (default: the §1.4 subset)
#   LOAMS_GATEWAY=<binary>  a built gateway (default: cargo build it)
#   RESP_PORT=<port>        the gateway's port (default 21379)
# Writes target/kv-resp-suite/; exits non-zero when a test that the skip
# list (crates/loams-resp/tests/valkey-skip.txt) does not name fails.
set -euo pipefail
repo="$(cd "$(dirname "$0")/../.." && pwd)"
out="$repo/target/kv-resp-suite"
port="${RESP_PORT:-21379}"
mkdir -p "$out"
if [[ -z "${LOAMS_GATEWAY:-}" ]]; then
  (cd "$repo" && cargo build -j "${JOBS:-4}" -p loams-gateway --features resp --bin loams-gateway)
  target="$(cd "$repo" && cargo metadata --format-version 1 --no-deps |
    python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
  LOAMS_GATEWAY="$target/debug/loams-gateway"
fi
cat >"$out/resp.toml" <<TOML
listen = "127.0.0.1:$port"
[store]
kind = "memory"
name = "resp-suite"
TOML
"$LOAMS_GATEWAY" --module resp --config "$out/resp.toml" >"$out/gateway.log" 2>&1 &
gw=$!
trap 'kill "$gw" 2>/dev/null || true' EXIT
up=0
for _ in $(seq 1 100); do
  if (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null; then up=1; break; fi
  sleep 0.1
done
[[ "$up" == 1 ]] || { cat "$out/gateway.log" >&2; echo "resp-suite: the gateway did not start" >&2; exit 1; }
VALKEY_UNITS="$*" VALKEY_SKIPFILE="$repo/crates/loams-resp/tests/valkey-skip.txt" \
  "$repo/scripts/kv/oracles/valkey-suite.sh" 127.0.0.1 "$port" "$out"
if grep -qE '^\[(err|exception)\]' "$out/valkey-suite.log" || ! grep -qx 'exit: 0' "$out/valkey-suite.summary"; then
  grep -E '^\[(err|exception)\]' "$out/valkey-suite.log" | head -50 >&2 || true
  tail -20 "$out/valkey-suite.log" >&2
  echo "resp-suite: failures outside the skip list (see $out/valkey-suite.log)" >&2
  exit 1
fi
echo "resp-suite: passed"
