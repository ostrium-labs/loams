#!/usr/bin/env bash
# The .NET corpus (TSDK-dotnet): the pinned Temporal .NET SDK's generator
# (workers/dotnet) against a running upstream Temporal server on the stack's
# network, the Go exporter for the histories (-prefix corpus-dotnet-), and
# manifest.json's dotnet entries (manifest.py build --languages dotnet; the
# Go and TypeScript entries are kept).
#
#   generate-dotnet.sh [--out DIR]   # default: this directory's histories/ (the committed corpus)
#
# Environment as generate.sh: TEMPORAL_ADDRESS (temporal-frontend:7233),
# CORPUS_SERVER (compat-mode: the stock server on Loams Postgres, the default;
# start-dev: the CLI's dev server), COMPUTE_PG_MAJOR (16), SCENARIOS
# (comma-separated; default all of the .NET worker's). Each generation uses a
# fresh namespace, so the fixed workflow IDs never collide with an earlier one.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=conformance/compute/lib.sh
. "$here/../../lib.sh"

out=$here/histories
[ "${1:-}" = --out ] && out=${2:?--out DIR}
: "${TEMPORAL_ADDRESS:=temporal-frontend:7233}" "${CORPUS_SERVER:=compat-mode}" "${COMPUTE_PG_MAJOR:=16}"
rt=$(oc_container_cmd)
ns=corpus-dotnet-$(date +%s)
mkdir -p "$out/dotnet"
rm -f "$out"/dotnet/*.json "$out"/dotnet/*.binpb "$out/generate-dotnet.json"
"$rt" volume create dx0-gocache >/dev/null 2>&1 || true
"$rt" volume create dx1-nugetcache >/dev/null 2>&1 || true

# The .NET generator: builds the worker in the pinned SDK image and runs it.
"$rt" run --rm --network "$OC_NETWORK" -v "$here:/c:ro" -v "$out:/out" -v dx1-nugetcache:/root/.nuget/packages \
  -e DOTNET_CLI_TELEMETRY_OPTOUT=1 -e DOTNET_NOLOGO=1 -e DOTNET_SKIP_FIRST_TIME_EXPERIENCE=1 "$DOTNET_IMAGE" \
  sh -ec "cp -r /c/workers/dotnet /tmp/d && cd /tmp/d && dotnet build -c Release -o /tmp/bin -v quiet -nologo && dotnet /tmp/bin/corpus-dotnet.dll generate $TEMPORAL_ADDRESS $ns /out/generate-dotnet.json ${SCENARIOS:-}"
list=$(python3 -c "import json,sys; print(','.join(s['scenario'] for s in json.load(open(sys.argv[1]))['scenarios']))" "$out/generate-dotnet.json")
"$rt" run --rm --network "$OC_NETWORK" -v "$here:/c:ro" -v "$out:/out" -v dx0-gocache:/go/pkg \
  -e GOMAXPROCS=4 -e GOFLAGS=-p=4 -e CGO_ENABLED=0 "$GO_IMAGE" \
  sh -ec "cp -r /c/workers/go /tmp/go && cd /tmp/go && go build -o /tmp/corpus . && /tmp/corpus export -address $TEMPORAL_ADDRESS -namespace $ns -prefix corpus-dotnet- -scenarios $list -out /out/dotnet"

if [ "$out" = "$here/histories" ]; then
  case $CORPUS_SERVER in
    compat-mode) version=$TEMPORAL_VERSION image=$TEMPORAL_SERVER_IMAGE pg=$COMPUTE_PG_MAJOR ;;
    start-dev) version=1.32.0 image="temporal server start-dev (CLI $TEMPORAL_CLI in $TEMPORAL_ADMIN_TOOLS_IMAGE)" pg=start-dev ;;
    *) echo "generate-dotnet.sh: CORPUS_SERVER is compat-mode or start-dev" >&2; exit 2 ;;
  esac
  python3 "$here/manifest.py" build --server-version "$version" --server-image "$image" --pg-major "$pg" --languages dotnet
fi
echo "generate-dotnet.sh: namespace $ns, $(find "$out/dotnet" -name "*.json" | wc -l) .NET histories in $out/dotnet"
