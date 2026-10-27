#!/usr/bin/env bash
# Generate the Ruby half of the history corpus (TSDK-ruby, D2151) against a
# running upstream Temporal server on the stack's network, into
# histories/ruby/, and rebuild the Ruby entries of manifest.json (the Go and
# TypeScript entries stay as they are; generate.sh owns those).
#
#   generate-ruby.sh [--out DIR]   # default: this directory's histories/ (the committed corpus)
#
# The temporalio gem (corpus/workers/ruby, pinned in its Gemfile.lock) runs
# its worker and starter (generate.rb); the Go exporter writes the
# histories, as for TypeScript; the gem's WorkflowReplayer then replays
# them (replay.rb), so a generation the SDK cannot replay fails here.
#
# Environment: TEMPORAL_ADDRESS (temporal-frontend:7233), CORPUS_SERVER
# (compat-mode: the stock server on Loams Postgres, the default; start-dev:
# the CLI's dev server), COMPUTE_PG_MAJOR (16), OC_NETWORK (lib.sh).
# Each generation uses a fresh namespace, so the fixed workflow IDs
# (corpus-ruby-<scenario>) never collide with an earlier one.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=conformance/compute/lib.sh
. "$here/../../lib.sh"

out=$here/histories
[ "${1:-}" = --out ] && out=${2:?--out DIR}
: "${TEMPORAL_ADDRESS:=temporal-frontend:7233}" "${CORPUS_SERVER:=compat-mode}" "${COMPUTE_PG_MAJOR:=16}"
rt=$(oc_container_cmd)
ns=corpus-ruby-$(date +%s)
mkdir -p "$out/ruby"
rm -f "$out"/ruby/*.json "$out"/ruby/*.binpb
"$rt" volume create dx0-gocache >/dev/null 2>&1 || true
"$rt" volume create dx1-gemcache >/dev/null 2>&1 || true

ruby_run() {
  local net=$1
  shift
  "$rt" run --rm --network "$net" -v "$here:/c:ro" -v "$out:/out" -v dx1-gemcache:/usr/local/bundle \
    -e BUNDLE_FROZEN=true -e "BUNDLER_VERSION=$BUNDLER_VERSION" "$@" "$RUBY_IMAGE" \
    sh -ec "cp -r /c/workers/ruby /tmp/rb && cd /tmp/rb && \
(gem list -i bundler -v $BUNDLER_VERSION >/dev/null || gem install --no-document bundler -v $BUNDLER_VERSION >/dev/null) && \
(bundle install --quiet --local 2>/dev/null || bundle install --quiet) && $RUBY_CMD"
}
go_run() {
  "$rt" run --rm --network "$OC_NETWORK" -v "$here:/c" -v "$out:/out" -v dx0-gocache:/go/pkg \
    -e GOMAXPROCS=4 -e GOFLAGS=-p=4 -e CGO_ENABLED=0 "$GO_IMAGE" \
    sh -ec "cd /c/workers/go && go build -o /tmp/corpus . && /tmp/corpus $*"
}

# The worker and the starter (registering the namespace and CorpusKeyword
# first, as the Go generator does), then the Go exporter.
RUBY_CMD="bundle exec ruby generate.rb all $TEMPORAL_ADDRESS $ns /out/generate-ruby.json" ruby_run "$OC_NETWORK" -e CORPUS_REGISTER_NAMESPACE=1
list=$(python3 -c "import json,sys; print(','.join(s['scenario'] for s in json.load(open(sys.argv[1]))['scenarios']))" "$out/generate-ruby.json")
go_run export -address "$TEMPORAL_ADDRESS" -namespace "$ns" -prefix corpus-ruby- -scenarios "$list" -out /out/ruby
# The SDK's own replayer on what it just generated (offline).
RUBY_CMD="CORPUS_HISTORIES=/out/ruby bundle exec ruby replay.rb" ruby_run none

if [ "$out" = "$here/histories" ]; then
  case $CORPUS_SERVER in
    compat-mode) version=$TEMPORAL_VERSION image=$TEMPORAL_SERVER_IMAGE pg=$COMPUTE_PG_MAJOR ;;
    # CLI 1.9.1 embeds server 1.32.0 (`temporal --version`), not the pinned 1.32.1.
    start-dev) version=1.32.0 image="temporal server start-dev (CLI $TEMPORAL_CLI in $TEMPORAL_ADMIN_TOOLS_IMAGE)" pg=start-dev ;;
    *) echo "generate-ruby.sh: CORPUS_SERVER is compat-mode or start-dev" >&2; exit 2 ;;
  esac
  python3 "$here/manifest.py" build --server-version "$version" --server-image "$image" --pg-major "$pg" --langs ruby
fi
echo "generate-ruby.sh: namespace $ns, $(find "$out/ruby" -name "*.json" | wc -l) Ruby histories in $out"
