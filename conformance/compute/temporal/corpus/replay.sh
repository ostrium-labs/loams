#!/usr/bin/env bash
# The corpus's nightly replays (DX1 DX0 Task 4's tests, wired by Task 7):
# every committed history replays with the pinned SDK's own replayer.
#
#   conformance/compute/temporal/corpus/replay.sh
#
# corpus_replays_go runs `go test ./...` of replay/go in the pinned Go image;
# corpus_replays_ts runs the TypeScript package's `npm run replay` in the
# pinned Node image. Both work on a copy of the corpus inside the container,
# so the checkout gets no node_modules, dist or Go cache. Needs no server and
# no stack. Writes $OC_OUT/corpus.junit.xml and checks it against xfail.toml.
# (`corpus_regenerates_stably` needs a Temporal server and stays a manual run
# of generate.sh until compat mode is go; DX1 ruling T7.R3.)
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=conformance/compute/lib.sh
. "$here/../../lib.sh"

suite=corpus
mkdir -p "$OC_OUT"
rm -f "$OC_OUT/$suite.junit.xml"
rt=$(oc_container_cmd)

one() { # <case> <image> <script>
  local name=$1 image=$2 script=$3 log
  log="$OC_OUT/$suite.$name.log"
  echo "---- $name"
  if "$rt" run --rm -v "$here:/corpus:ro" -e GOFLAGS=-p=4 -e GOMAXPROCS=4 "$image" \
    sh -ec "cp -r /corpus /w && $script" >"$log" 2>&1; then
    oc_junit_case "$suite" "$name" pass
    echo "ok   $name"
  else
    oc_junit_case "$suite" "$name" fail "$(tail -n 20 "$log")"
    echo "FAIL $name (log: $log)"
  fi
}

one corpus_replays_go "$GO_IMAGE" 'cd /w/replay/go && go test -count=1 ./...'
one corpus_replays_ts "$NODE_IMAGE" 'cd /w/workers/ts && npm ci --no-audit --no-fund && npm run build && npm run replay'

oc_xfail_check temporal/corpus "$OC_OUT/$suite.junit.xml"
