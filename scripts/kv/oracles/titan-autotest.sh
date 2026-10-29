#!/usr/bin/env bash
# Titan's (distributedio/titan) tools/autotest against <host>:<port>
# (KV1 plan KV0 Task 10, step 4). Titan is unmodified except for one file
# the harness swaps in at run time: titan/main_test.go, which skips
# integration.Start() (an in-process Titan on mocktikv) and dials the
# address in LOAMS_TITAN_AUTOTEST_ADDR. The server must accept AUTH with
# TITAN_AUTOTEST_TOKEN (pins.sh). Writes <out>/titan-autotest.summary.
# Usage: titan-autotest.sh <host> <port> <out>
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=pins.sh
. "$here/pins.sh"
host="$1" port="$2" out="$3"
mkdir -p "$out"
work="${RUNNER_TEMP:-$out}/titan-autotest-src"
"$here/fetch.sh" "$TITAN_REPO" "$TITAN_COMMIT" "$work"
cp "$here/titan/main_test.go" "$work/tools/autotest/main_test.go"
export GOFLAGS=-mod=mod GOPROXY="${GOPROXY:-https://proxy.golang.org,direct}" GO111MODULE=on
set +e
(cd "$work" && LOAMS_TITAN_AUTOTEST_ADDR="$host:$port" go test -count=1 -v ./tools/autotest/) >"$out/titan-autotest.log" 2>&1
rc=$?
set -e
{
  echo "titan autotest: $TITAN_REPO@$TITAN_COMMIT against $host:$port, $(go version)"
  echo "exit: $rc"
  echo "pass: $(grep -c -- '--- PASS' "$out/titan-autotest.log" || true)"
  echo "fail: $(grep -c -- '--- FAIL' "$out/titan-autotest.log" || true)"
  grep -E "Error Trace:|^panic:" "$out/titan-autotest.log" | sort | uniq -c | head -20 || true
} | tee "$out/titan-autotest.summary"
