#!/usr/bin/env bash
# Valkey 9.1.2's TCL suite, unmodified, against <host>:<port>, with the
# research report's §1.4 first subset (KV1 plan KV0 Task 10, step 1).
# Writes <out>/valkey-suite.log and <out>/valkey-suite.summary; exits 0 even
# when tests fail (the counts are the result), non-zero when it cannot run.
# Usage: valkey-suite.sh <host> <port> <out>
#   VALKEY_UNITS="unit/a unit/b"  run these units instead of the subset
#   VALKEY_SKIPFILE=<file>        skip the tests it lists (one per line;
#                                 text after " # " is the reason)
#   DOCKER=podman                 the container CLI (default: docker, else podman)
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=pins.sh
. "$here/pins.sh"
host="$1" port="$2" out="$3"
mkdir -p "$out"
work="${RUNNER_TEMP:-$out}/valkey-src"
docker="${DOCKER:-$(command -v docker || command -v podman)}"
if [[ ! -x "$work/runtest" ]] || [[ "$(cat "$work/.loams-commit" 2>/dev/null)" != "$VALKEY_COMMIT" ]]; then
  rm -rf "$work"
  "$here/fetch.sh" "$VALKEY_REPO" "$VALKEY_COMMIT" "$work"
  echo "$VALKEY_COMMIT" >"$work/.loams-commit"
fi
# The suite looks for the server's binaries under src/; take the pinned
# image's, so nothing is built.
mkdir -p "$work/src"
cid=$("$docker" create "$VALKEY_IMAGE")
for bin in valkey-server valkey-cli valkey-benchmark; do
  "$docker" cp "$cid:/usr/local/bin/$bin" "$work/src/$bin" 2>/dev/null || true
done
"$docker" rm "$cid" >/dev/null
units=(unit/info-command unit/type/incr unit/type/string unit/type/hash unit/type/set
  unit/type/zset unit/type/list-2 unit/type/list-3 unit/expire unit/multi unit/keyspace)
if [[ -n "${VALKEY_UNITS:-}" ]]; then
  read -ra units <<<"$VALKEY_UNITS"
fi
single=()
for u in "${units[@]}"; do single+=(--single "$u"); done
if [[ -n "${VALKEY_SKIPFILE:-}" ]]; then
  # runtest's --skipfile takes bare test names; drop comments and reasons.
  sed -E 's/[[:space:]]+# .*$//; /^[[:space:]]*(#|$)/d' "$VALKEY_SKIPFILE" >"$out/skip.txt"
  single+=(--skipfile "$out/skip.txt")
fi
set +e
(cd "$work" && ./runtest --host "$host" --port "$port" --singledb --ignore-encoding --ignore-digest \
  --tags "-needs:debug -needs:repl -needs:pfdebug -needs:save -needs:reset -needs:config-maxmemory -needs:config-resetstat -large-memory" \
  "${single[@]}") >"$out/valkey-suite.log" 2>&1
rc=$?
set -e
{
  echo "valkey suite: $VALKEY_REPO@$VALKEY_COMMIT ($VALKEY_TAG) against $host:$port"
  echo "exit: $rc"
  for kind in ok err skip ignore exception; do
    echo "$kind: $(grep -c "^\[$kind\]" "$out/valkey-suite.log" || true)"
  done
} | tee "$out/valkey-suite.summary"
