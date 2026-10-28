#!/usr/bin/env bash
# Generate the history corpus (DX0 Task 4) against a running Temporal server
# on the stack's network, into histories/<lang>/, and rebuild manifest.json.
#
#   generate.sh [--out DIR] [--langs go,ts,python]
#     --out    default: this directory's histories/ (the committed corpus)
#     --langs  default: $CORPUS_LANGS, else go,ts. Go always runs first: it
#              registers the namespace and the search attribute, and its
#              exporter writes every language's files. Only the languages
#              named are regenerated; the others' files are kept.
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
langs=${CORPUS_LANGS:-go,ts}
while [ $# -gt 0 ]; do
  case $1 in
    --out) out=${2:?--out DIR}; shift 2 ;;
    --langs) langs=${2:?--langs a,b}; shift 2 ;;
    *) echo "usage: generate.sh [--out DIR] [--langs go,ts,python]" >&2; exit 2 ;;
  esac
done
want() { [[ ",go,$langs," == *",$1,"* ]]; }
: "${TEMPORAL_ADDRESS:=temporal-frontend:7233}" "${CORPUS_SERVER:=compat-mode}" "${COMPUTE_PG_MAJOR:=16}"
rt=$(oc_container_cmd)
ns=corpus-$(date +%s)
for l in go ts python; do
  want "$l" || continue
  mkdir -p "$out/$l"
  rm -f "$out/$l"/*.json "$out/$l"/*.binpb
done
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
if want ts; then
"$rt" run --rm --network "$OC_NETWORK" -v "$here:/c" -v "$out:/out" -v dx0-npmcache:/root/.npm \
  -w /c/workers/ts "$NODE_IMAGE" \
  sh -ec "npm ci --no-audit --no-fund >/dev/null && npx tsc -p . && node dist/generate.js $TEMPORAL_ADDRESS $ns /out/generate-ts.json"
list=$(python3 -c "import json,sys; print(','.join(s['scenario'] for s in json.load(open(sys.argv[1]))['scenarios']))" "$out/generate-ts.json")
go_run export -address "$TEMPORAL_ADDRESS" -namespace "$ns" -prefix corpus-ts- -scenarios "$list" -out /out/ts
fi
# Python (lane TSDK-python, D2137): its worker and starters under uv, then
# the Go exporter, as for TypeScript.
if want python; then
"$rt" volume create dx0-uvcache >/dev/null 2>&1 || true
"$rt" run --rm --network "$OC_NETWORK" -v "$here:/c:ro" -v "$out:/out" -v dx0-uvcache:/root/.cache \
  -e UV_PYTHON_DOWNLOADS=never -e UV_LINK_MODE=copy -e PYTHONUNBUFFERED=1 "$PYTHON_IMAGE" \
  sh -ec "pip install --quiet --disable-pip-version-check --root-user-action=ignore uv==$UV_VERSION && cp -r /c /tmp/c && cd /tmp/c/workers/python && uv sync --frozen --quiet && uv run --frozen python -m corpus_py generate $TEMPORAL_ADDRESS $ns /out/generate-python.json"
list=$(python3 -c "import json,sys; print(','.join(s['scenario'] for s in json.load(open(sys.argv[1]))['scenarios']))" "$out/generate-python.json")
go_run export -address "$TEMPORAL_ADDRESS" -namespace "$ns" -prefix corpus-python- -scenarios "$list" -out /out/python
fi

if [ "$out" = "$here/histories" ]; then
  case $CORPUS_SERVER in
    compat-mode) version=$TEMPORAL_VERSION image=$TEMPORAL_SERVER_IMAGE pg=$COMPUTE_PG_MAJOR ;;
    # CLI 1.9.1 embeds server 1.32.0 (`temporal --version`), not the pinned 1.32.1.
    start-dev) version=1.32.0 image="temporal server start-dev (CLI $TEMPORAL_CLI in $TEMPORAL_ADMIN_TOOLS_IMAGE)" pg=start-dev ;;
    *) echo "generate.sh: CORPUS_SERVER is compat-mode or start-dev" >&2; exit 2 ;;
  esac
  python3 "$here/manifest.py" build --server-version "$version" --server-image "$image" --pg-major "$pg"
fi
for l in go ts python; do
  want "$l" && echo "generate.sh: namespace $ns, $(find "$out/$l" -name "*.json" | wc -l) $l histories in $out/$l"
done
