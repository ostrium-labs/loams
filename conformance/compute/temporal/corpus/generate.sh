#!/usr/bin/env bash
# Generate the history corpus (DX0 Task 4) against a running Temporal server
# on the stack's network, into histories/{go,ts}/, and rebuild manifest.json.
#
#   generate.sh [--out DIR]   # default: this directory's histories/ (the committed corpus)
#
# Environment: TEMPORAL_ADDRESS (temporal-frontend:7233), CORPUS_SERVER
# (compat-mode: the stock server on Loams Postgres, the default; start-dev:
# the CLI's dev server, Task 3's fallback oracle), COMPUTE_PG_MAJOR (16).
# Each generation uses a fresh namespace, so the fixed workflow IDs
# (corpus-<lang>-<scenario>) never collide with an earlier one.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=conformance/compute/lib.sh
. "$here/../../lib.sh"

out=$here/histories
[ "${1:-}" = --out ] && out=${2:?--out DIR}
: "${TEMPORAL_ADDRESS:=temporal-frontend:7233}" "${CORPUS_SERVER:=compat-mode}" "${COMPUTE_PG_MAJOR:=16}"
rt=$(oc_container_cmd)
ns=corpus-$(date +%s)
mkdir -p "$out/go" "$out/ts"
rm -f "$out"/go/*.json "$out"/go/*.binpb "$out"/ts/*.json "$out"/ts/*.binpb
"$rt" volume create dx0-gocache >/dev/null 2>&1 || true
"$rt" volume create dx0-npmcache >/dev/null 2>&1 || true

go_run() {
  "$rt" run --rm --network "$OC_NETWORK" -v "$here:/c" -v "$out:/out" -v dx0-gocache:/go/pkg \
    -e GOMAXPROCS=4 -e GOFLAGS=-p=4 -e CGO_ENABLED=0 "$GO_IMAGE" \
    sh -ec "cd /c/workers/go && go build -o /tmp/corpus . && /tmp/corpus $*"
}

# Go: generate and export.
go_run generate -address "$TEMPORAL_ADDRESS" -namespace "$ns" -out /out/go
# TypeScript: run its worker and drivers, then export with the Go exporter.
"$rt" run --rm --network "$OC_NETWORK" -v "$here:/c" -v "$out:/out" -v dx0-npmcache:/root/.npm \
  -w /c/workers/ts "$NODE_IMAGE" \
  sh -ec "npm ci --no-audit --no-fund >/dev/null && npx tsc -p . && node dist/generate.js $TEMPORAL_ADDRESS $ns /out/generate-ts.json"
list=$(python3 -c "import json,sys; print(','.join(s['scenario'] for s in json.load(open(sys.argv[1]))['scenarios']))" "$out/generate-ts.json")
go_run export -address "$TEMPORAL_ADDRESS" -namespace "$ns" -prefix corpus-ts- -scenarios "$list" -out /out/ts

if [ "$out" = "$here/histories" ]; then
  case $CORPUS_SERVER in
    compat-mode) image=$TEMPORAL_SERVER_IMAGE pg=$COMPUTE_PG_MAJOR ;;
    start-dev) image="temporal server start-dev (CLI in $TEMPORAL_ADMIN_TOOLS_IMAGE)" pg=start-dev ;;
    *) echo "generate.sh: CORPUS_SERVER is compat-mode or start-dev" >&2; exit 2 ;;
  esac
  python3 "$here/manifest.py" build --server-version "$TEMPORAL_VERSION" --server-image "$image" --pg-major "$pg"
fi
echo "generate.sh: namespace $ns, $(ls "$out"/go/*.json | wc -l) Go and $(ls "$out"/ts/*.json | wc -l) TypeScript histories in $out"
