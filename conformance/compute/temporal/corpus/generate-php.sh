#!/usr/bin/env bash
# Generate the PHP half of the history corpus (TSDK-php, D2148) against a
# running upstream Temporal server on the stack's network, into
# histories/php/, and rebuild the PHP entries of manifest.json (the other
# languages' entries stay as they are; generate.sh owns Go's and TypeScript's).
#
#   generate-php.sh [--out DIR]   # default: this directory's histories/ (the committed corpus)
#
# temporal/sdk (corpus/workers/php, locked in composer.lock) under RoadRunner
# runs the worker, and starter.php drives the scenarios; the Go exporter
# writes the histories, as for TypeScript; the SDK's WorkflowReplayer then
# replays them (replay.php through RoadRunner), so a generation the SDK cannot
# replay fails here.
#
# Environment: TEMPORAL_ADDRESS (temporal-frontend:7233), CORPUS_SERVER
# (compat-mode: the stock server on Loams Postgres, the default; start-dev:
# the CLI's dev server), COMPUTE_PG_MAJOR (16), OC_NETWORK (lib.sh).
# Each generation uses a fresh namespace, registered here with the Keyword
# search attribute CorpusKeyword (the `temporal` CLI of the pinned admin-tools
# image, as the Go generator does through its client), so the fixed
# workflow IDs (corpus-php-<scenario>) never collide with an earlier one.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=conformance/compute/lib.sh
. "$here/../../lib.sh"
# shellcheck source=conformance/compute/temporal/corpus/workers/php/image.sh
. "$here/workers/php/image.sh"

out=$here/histories
[ "${1:-}" = --out ] && out=${2:?--out DIR}
: "${TEMPORAL_ADDRESS:=temporal-frontend:7233}" "${CORPUS_SERVER:=compat-mode}" "${COMPUTE_PG_MAJOR:=16}"
rt=$(oc_container_cmd)
ns=corpus-php-$(date +%s)
mkdir -p "$out/php"
rm -f "$out"/php/*.json "$out"/php/*.binpb
"$rt" volume create dx0-gocache >/dev/null 2>&1 || true
"$rt" volume create dx1-composercache >/dev/null 2>&1 || true
php_image=$(php_corpus_image "$rt")

tctl() {
  "$rt" run --rm --network "$OC_NETWORK" "$TEMPORAL_ADMIN_TOOLS_IMAGE" temporal --address "$TEMPORAL_ADDRESS" "$@"
}
php_run() {
  "$rt" run --rm --network "$OC_NETWORK" -v "$here:/c:ro" -v "$out:/out" -v dx1-composercache:/composer/cache "$php_image" \
    sh -ec "cp -r /c /tmp/c && cd /tmp/c/workers/php && \
composer install --no-interaction --no-progress --prefer-dist --quiet && $*"
}
go_run() {
  "$rt" run --rm --network "$OC_NETWORK" -v "$here:/c" -v "$out:/out" -v dx0-gocache:/go/pkg \
    -e GOMAXPROCS=4 -e GOFLAGS=-p=4 -e CGO_ENABLED=0 "$GO_IMAGE" \
    sh -ec "cd /c/workers/go && go build -o /tmp/corpus . && /tmp/corpus $*"
}

# The namespace (72 h retention, DX1 R0.9) and CorpusKeyword, then a pause
# for the new namespace to reach every service's cache.
tctl operator namespace create --retention 72h --namespace "$ns"
for _ in $(seq 30); do
  tctl operator search-attribute create --namespace "$ns" --name CorpusKeyword --type Keyword && break
  sleep 2
done
sleep 3

# The worker and the starter, then the Go exporter.
php_run "./corpus-php.sh check && ./corpus-php.sh generate $TEMPORAL_ADDRESS $ns /out/generate-php.json"
list=$(python3 -c "import json,sys; print(','.join(s['scenario'] for s in json.load(open(sys.argv[1]))['scenarios']))" "$out/generate-php.json")
go_run export -address "$TEMPORAL_ADDRESS" -namespace "$ns" -prefix corpus-php- -scenarios "$list" -out /out/php
# The SDK's own replayer on what it just generated (RoadRunner dials the
# server at start, so on the stack's network).
php_run "CORPUS_HISTORIES=/out/php ./corpus-php.sh replay $TEMPORAL_ADDRESS $ns"

if [ "$out" = "$here/histories" ]; then
  case $CORPUS_SERVER in
    compat-mode) version=$TEMPORAL_VERSION image=$TEMPORAL_SERVER_IMAGE pg=$COMPUTE_PG_MAJOR ;;
    # CLI 1.9.1 embeds server 1.32.0 (`temporal --version`), not the pinned 1.32.1.
    start-dev) version=1.32.0 image="temporal server start-dev (CLI $TEMPORAL_CLI in $TEMPORAL_ADMIN_TOOLS_IMAGE)" pg=start-dev ;;
    *) echo "generate-php.sh: CORPUS_SERVER is compat-mode or start-dev" >&2; exit 2 ;;
  esac
  python3 "$here/manifest.py" build --server-version "$version" --server-image "$image" --pg-major "$pg" --languages php
fi
echo "generate-php.sh: namespace $ns, $(find "$out/php" -name "*.json" | wc -l) PHP histories in $out"
