#!/usr/bin/env bash
# Valkey 9.1.2's TCL suite, unmodified, against <host>:<port>, with the
# research report's §1.4 first subset (KV1 plan KV0 Task 10, step 1).
# Writes <out>/valkey-suite.log and <out>/valkey-suite.summary; exits 0 even
# when tests fail (the counts are the result), non-zero when it cannot run.
# Usage: valkey-suite.sh <host> <port> <out>
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=pins.sh
. "$here/pins.sh"
host="$1" port="$2" out="$3"
mkdir -p "$out"
work="${RUNNER_TEMP:-$out}/valkey-src"
"$here/fetch.sh" "$VALKEY_REPO" "$VALKEY_COMMIT" "$work"
# The suite looks for the server's binaries under src/; take the pinned
# image's, so nothing is built.
mkdir -p "$work/src"
cid=$(docker create "$VALKEY_IMAGE")
for bin in valkey-server valkey-cli valkey-benchmark; do
  docker cp "$cid:/usr/local/bin/$bin" "$work/src/$bin" 2>/dev/null || true
done
docker rm "$cid" >/dev/null
units=(unit/info-command unit/type/incr unit/type/string unit/type/hash unit/type/set
  unit/type/zset unit/type/list-2 unit/type/list-3 unit/expire unit/multi unit/keyspace)
single=()
for u in "${units[@]}"; do single+=(--single "$u"); done
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
