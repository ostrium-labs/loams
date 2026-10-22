#!/usr/bin/env bash
# The packaged `loams` serves Durable (#925): `loams dev` starts the embedded
# Resonate server on 127.0.0.1:8001 and it answers HTTP there. A build without
# the `durable` feature logs "this build has no durable execution" and leaves
# the port closed, which is what the 0.1.0 packages did.
#
#   scripts/release/smoke-durable.sh path/to/loams
set -euo pipefail

bin="${1:?usage: smoke-durable.sh path/to/loams}"
work="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/var/tmp}}/smoke-durable.XXXXXX")"
log="$work/loams.log"

"$bin" dev --data-dir "$work/data" >"$log" 2>&1 &
pid=$!
cleanup() {
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT

code=000
for _ in $(seq 1 60); do
  if ! kill -0 "$pid" 2>/dev/null; then
    echo "smoke-durable: loams dev exited early" >&2
    cat "$log" >&2
    exit 1
  fi
  # Any HTTP status means the Resonate server is there; 000 means no answer.
  code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 2 http://127.0.0.1:8001/promises || true)"
  [ "$code" != 000 ] && break
  sleep 1
done

if grep -q 'no durable execution' "$log"; then
  echo "smoke-durable: the binary was built without the durable feature" >&2
  cat "$log" >&2
  exit 1
fi
if [ "$code" = 000 ]; then
  echo "smoke-durable: nothing answered on 127.0.0.1:8001 within 60 s" >&2
  cat "$log" >&2
  exit 1
fi
echo "smoke-durable: Durable answers on 127.0.0.1:8001 (HTTP $code)"
