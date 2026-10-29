#!/usr/bin/env bash
# Tidis's Python suite (minus Lua) against <host>:<port> (KV1 plan KV0
# Task 10, step 2). On real Valkey its failures are Tidis's own deviations.
# Writes <out>/tidis-suite.log and <out>/tidis-suite.summary.
# Usage: tidis-suite.sh <host> <port> <out>
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=pins.sh
. "$here/pins.sh"
host="$1" port="$2" out="$3"
mkdir -p "$out"
work="${RUNNER_TEMP:-$out}/tidis-src"
"$here/fetch.sh" "$TIDIS_REPO" "$TIDIS_COMMIT" "$work"
python3 -m venv "$work/.venv"
"$work/.venv/bin/pip" install --quiet -r "$work/test/requirements.txt"
"$work/.venv/bin/python" -I "$here/tidis_suite.py" "$work" "$host" "$port" >"$out/tidis-suite.log" 2>&1 || true
{
  echo "tidis suite: $TIDIS_REPO@$TIDIS_COMMIT against $host:$port"
  grep -E '^(tidis suite|deviation):' "$out/tidis-suite.log" || echo "no summary: see tidis-suite.log"
} | tee "$out/tidis-suite.summary"
